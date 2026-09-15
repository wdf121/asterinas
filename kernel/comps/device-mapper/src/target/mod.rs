// SPDX-License-Identifier: MPL-2.0

//! Target metadata, parsing, and dispatch for supported Device Mapper targets.
//!
//! This module is the boundary between Linux table-load target strings and typed
//! target implementations. It keeps target names, target-version metadata, common
//! accessors, and parse-time error classification together so the ioctl layer does
//! not duplicate target-specific knowledge.

use alloc::{boxed::Box, string::String, vec::Vec};
#[cfg(ktest)]
use core::any::Any;
use core::{fmt::Debug, ops::Range};

use aster_block::{BlockDevice, BlockDeviceLease, id::Sid};
use device_id::DeviceId;

use self::{error::ErrorTarget, linear::LinearTarget, striped::StripedTarget, zero::ZeroTarget};
use crate::TableError;

pub mod error;
pub mod linear;
pub mod striped;
pub mod zero;

/// Common logical sector range owned by every resolved target variant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetRange {
    /// Half-open logical sector range exposed by the target inside its table.
    logical_range: Range<Sid>,
}

impl TargetRange {
    /// Builds a non-empty target range and rejects logical end overflow.
    pub fn new(logical_start: Sid, length: u64) -> Result<Self, TableError> {
        if length == 0 {
            return Err(TableError::ZeroLength);
        }

        let logical_end = logical_start
            .to_raw()
            .checked_add(length)
            .ok_or(TableError::LogicalRangeOverflow)?;
        Ok(Self {
            logical_range: logical_start..Sid::new(logical_end),
        })
    }

    /// Returns the half-open logical sector range covered by the target.
    pub fn logical_range(&self) -> &Range<Sid> {
        &self.logical_range
    }

    /// Returns the range length in 512-byte sectors.
    pub fn length(&self) -> u64 {
        self.logical_range.end.to_raw() - self.logical_range.start.to_raw()
    }

    /// Returns the offset of an in-range sector from the target's logical start.
    pub fn offset_of(&self, logical: Sid) -> Option<u64> {
        if !self.logical_range.contains(&logical) {
            return None;
        }
        logical
            .to_raw()
            .checked_sub(self.logical_range.start.to_raw())
    }

    /// Checks whether a non-empty logical subrange is fully owned by this target.
    pub fn contains_range(&self, logical: &Range<Sid>) -> bool {
        logical.start < logical.end
            && logical.start.to_raw() >= self.logical_range.start.to_raw()
            && logical.end.to_raw() <= self.logical_range.end.to_raw()
    }
}

/// Target-local result that tells `DmTable` how to complete or submit one BIO part.
pub enum TargetIoAction<'a> {
    /// Remap the logical part to a backing device and submit it there.
    Remap {
        /// Logical range covered by this child BIO on the mapped device.
        logical_range: Range<Sid>,
        /// Backing sector corresponding to `logical_range.start`.
        backing_start: Sid,
        /// Resolved backing block device that receives the remapped BIO.
        backing: &'a dyn BlockDevice,
    },
    /// Complete the logical part with a stable I/O error.
    Error {
        /// Logical range covered by this child BIO on the mapped device.
        logical_range: Range<Sid>,
    },
    /// Complete the logical part without submitting backing I/O.
    Zero {
        /// Logical range covered by this child BIO on the mapped device.
        logical_range: Range<Sid>,
    },
}

impl TargetIoAction<'_> {
    /// Returns the logical range used when splitting the original BIO into children.
    pub(crate) fn logical_range(&self) -> &Range<Sid> {
        match self {
            Self::Remap { logical_range, .. }
            | Self::Error { logical_range }
            | Self::Zero { logical_range } => logical_range,
        }
    }
}

/// Selects the Linux table/status parameter flavor requested by `DM_TABLE_STATUS`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetStatusMode {
    /// Report reloadable target parameters for `dmsetup table`-style queries.
    Table,
    /// Report runtime target status parameters for `dmsetup status`-style queries.
    Status,
}

/// Common behavior implemented by every resolved Device Mapper target instance.
pub trait DmTarget: Debug + Send + Sync {
    /// Returns this target as `Any` for ktest-only downcasting of boxed targets.
    #[cfg(ktest)]
    fn as_any(&self) -> &dyn Any;

    /// Returns static Linux-visible metadata for this target implementation.
    fn metadata(&self) -> DmTargetMetadata;

    /// Returns the Linux-visible target type name for table/status output.
    fn name(&self) -> &'static str {
        self.metadata().name()
    }

    /// Returns the logical sector range owned by this target in its table.
    fn logical_range(&self) -> &Range<Sid>;

    /// Returns the target length in 512-byte sectors.
    fn length(&self) -> u64 {
        let range = self.logical_range();
        range.end.to_raw() - range.start.to_raw()
    }

    /// Visits backing IDs referenced by this target in Linux table order.
    fn for_each_backing_id(&self, f: &mut dyn FnMut(DeviceId));

    /// Visits resolved backing devices referenced by this target in Linux table order.
    fn for_each_backing<'a>(&'a self, f: &mut dyn FnMut(&'a dyn BlockDevice));

    /// Formats target-specific Linux table/status parameters.
    fn status_params(&self, mode: TargetStatusMode) -> Result<String, TableError>;

    /// Maps a target-local logical range into table-level I/O actions.
    fn map_io_range(&self, logical: Range<Sid>) -> Option<Vec<TargetIoAction<'_>>>;
}

/// Owned target instance installed in a `DmTable`.
pub type DmTargetBox = Box<dyn DmTarget>;

#[cfg(ktest)]
impl dyn DmTarget + '_ {
    /// Returns the concrete target type when the boxed target has that implementation.
    pub fn downcast_ref<T: DmTarget + 'static>(&self) -> Option<&T> {
        self.as_any().downcast_ref::<T>()
    }
}

/// Static identity exported through `dmsetup targets` and target-version ioctls.
#[derive(Clone, Copy, Debug)]
pub struct DmTargetMetadata {
    /// Linux target type name used in table-load, table/status, and target-version records.
    name: &'static str,
    /// Linux-compatible target-version triplet reported to libdevmapper.
    version: [u32; 3],
}

impl DmTargetMetadata {
    /// Creates a compile-time target metadata entry shared by parser and ioctl code.
    pub const fn new(name: &'static str, version: [u32; 3]) -> Self {
        Self { name, version }
    }

    /// Returns the Linux-visible target type name stored in `dm_target_spec` records.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Returns the target-version triplet reported by `DM_LIST_VERSIONS`.
    pub fn version(&self) -> [u32; 3] {
        self.version
    }
}

const ERROR_METADATA: DmTargetMetadata = DmTargetMetadata::new("error", [1, 6, 0]);
const LINEAR_METADATA: DmTargetMetadata = DmTargetMetadata::new("linear", [1, 4, 0]);
const STRIPED_METADATA: DmTargetMetadata = DmTargetMetadata::new("striped", [1, 6, 0]);
const ZERO_METADATA: DmTargetMetadata = DmTargetMetadata::new("zero", [1, 1, 0]);

/// Parser implementation selected by one static target-catalogue entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TargetKind {
    Error,
    Linear,
    Striped,
    Zero,
}

/// One Linux-visible target declaration and its parser implementation.
///
/// The catalogue is the single source for discovery order, target-version records,
/// name lookup, and parser selection. Target implementations retain their own
/// parameter and backing-resolution rules.
#[derive(Clone, Copy, Debug)]
pub struct TargetCatalogEntry {
    metadata: DmTargetMetadata,
    kind: TargetKind,
}

impl TargetCatalogEntry {
    const fn new(metadata: DmTargetMetadata, kind: TargetKind) -> Self {
        Self { metadata, kind }
    }

    /// Returns the Linux target type name used by table-load and version ioctls.
    pub fn name(&self) -> &'static str {
        self.metadata.name()
    }

    /// Returns the Linux target-version triplet in catalogue order.
    pub fn version(&self) -> [u32; 3] {
        self.metadata.version()
    }
}

/// Ordered target declarations currently accepted by table-load and version ioctls.
pub const TARGET_CATALOG: &[TargetCatalogEntry] = &[
    TargetCatalogEntry::new(ERROR_METADATA, TargetKind::Error),
    TargetCatalogEntry::new(LINEAR_METADATA, TargetKind::Linear),
    TargetCatalogEntry::new(STRIPED_METADATA, TargetKind::Striped),
    TargetCatalogEntry::new(ZERO_METADATA, TargetKind::Zero),
];

impl TargetKind {
    /// Parses one target while preserving caller-owned backing token and lease hooks.
    fn parse_with<E>(
        self,
        logical_start: Sid,
        length: u64,
        params: &str,
        parse_backing: &mut impl FnMut(&str) -> Result<DeviceId, E>,
        resolve_backing: &mut impl FnMut(DeviceId) -> Result<BlockDeviceLease, E>,
    ) -> Result<DmTargetBox, DmTargetParseError<E>> {
        match self {
            Self::Error => Ok(Box::new(ErrorTarget::parse(logical_start, length, params)?)),
            Self::Linear => Ok(Box::new(LinearTarget::parse_with(
                logical_start,
                length,
                params,
                parse_backing,
                resolve_backing,
            )?)),
            Self::Striped => Ok(Box::new(StripedTarget::parse_with(
                logical_start,
                length,
                params,
                parse_backing,
                resolve_backing,
            )?)),
            Self::Zero => Ok(Box::new(ZeroTarget::parse(logical_start, length, params)?)),
        }
    }
}

/// Error boundary between target semantics and environment-specific backing lookup.
#[derive(Debug, Eq, PartialEq)]
pub enum DmTargetParseError<E> {
    /// The requested target type is not implemented by this Device Mapper subset.
    UnsupportedTarget,
    /// Target parameters were syntactically valid enough to parse but semantically invalid.
    Table(TableError),
    /// Backing device token parsing or lease lookup failed outside the target component.
    ResolveBacking(E),
}

impl<E> From<TableError> for DmTargetParseError<E> {
    fn from(error: TableError) -> Self {
        Self::Table(error)
    }
}

/// Parses one Linux table target while keeping VFS/registry lookup in caller hooks.
pub fn parse_target_with<E>(
    target_type: &str,
    logical_start: Sid,
    length: u64,
    params: &str,
    parse_backing: &mut impl FnMut(&str) -> Result<DeviceId, E>,
    resolve_backing: &mut impl FnMut(DeviceId) -> Result<BlockDeviceLease, E>,
) -> Result<DmTargetBox, DmTargetParseError<E>> {
    let entry = TARGET_CATALOG
        .iter()
        .find(|entry| entry.name() == target_type)
        .ok_or(DmTargetParseError::UnsupportedTarget)?;
    entry.kind.parse_with(
        logical_start,
        length,
        params,
        parse_backing,
        resolve_backing,
    )
}

#[cfg(ktest)]
mod tests {
    use alloc::{sync::Arc, vec};

    use aster_block::{
        BlockDeviceMeta,
        bio::{BioEnqueueError, SubmittedBio},
    };
    use device_id::{MajorId, MinorId};
    use ostd::prelude::ktest;

    use super::*;

    #[derive(Debug)]
    struct TestBlockDevice {
        id: DeviceId,
    }

    impl TestBlockDevice {
        fn new(minor: u32) -> Arc<Self> {
            Arc::new(Self {
                id: DeviceId::new(MajorId::new(510), MinorId::new(minor)),
            })
        }
    }

    impl BlockDevice for TestBlockDevice {
        fn enqueue(&self, _bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            unreachable!()
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 8,
                nr_sectors: 4096,
            }
        }

        fn name(&self) -> String {
            String::from("target-parser-test")
        }

        fn id(&self) -> DeviceId {
            self.id
        }
    }

    fn backing(minor: u32) -> BlockDeviceLease {
        BlockDeviceLease::new_untracked(TestBlockDevice::new(minor))
    }

    #[ktest]
    fn target_catalogue_preserves_linux_discovery_order() {
        let entries = TARGET_CATALOG
            .iter()
            .map(|entry| (entry.name(), entry.version()))
            .collect::<Vec<_>>();
        assert_eq!(
            entries,
            vec![
                ("error", [1, 6, 0]),
                ("linear", [1, 4, 0]),
                ("striped", [1, 6, 0]),
                ("zero", [1, 1, 0]),
            ]
        );
    }

    #[ktest]
    fn parses_targets_before_resolving_backings() {
        use core::cell::RefCell;

        let parsed = RefCell::new(Vec::new());
        let events = RefCell::new(Vec::new());
        let mut parse_backing = |token: &str| {
            events.borrow_mut().push("parse");
            parsed.borrow_mut().push(String::from(token));
            match token {
                "/dev/first" => Ok(DeviceId::new(MajorId::new(510), MinorId::new(1))),
                "510:2" => Ok(DeviceId::new(MajorId::new(510), MinorId::new(2))),
                _ => Err("missing"),
            }
        };
        let mut resolve_backing = |id: DeviceId| -> Result<BlockDeviceLease, &'static str> {
            events.borrow_mut().push("resolve");
            Ok(backing(id.minor().get()))
        };

        let linear = parse_target_with(
            "linear",
            Sid::new(0),
            8,
            "/dev/first 16",
            &mut parse_backing,
            &mut resolve_backing,
        )
        .unwrap();
        let linear = linear
            .downcast_ref::<LinearTarget>()
            .expect("expected linear target");
        assert_eq!(linear.backing_id().minor().get(), 1);
        assert_eq!(linear.backing_start(), Sid::new(16));

        let striped = parse_target_with(
            "striped",
            Sid::new(8),
            16,
            "2 4 /dev/first 32 510:2 64",
            &mut parse_backing,
            &mut resolve_backing,
        )
        .unwrap();
        let striped = striped
            .downcast_ref::<StripedTarget>()
            .expect("expected striped target");
        assert_eq!(striped.stripe_count(), 2);
        assert_eq!(striped.chunk_size(), 4);
        assert_eq!(striped.backing_id(0).unwrap().minor().get(), 1);
        assert_eq!(striped.backing_id(1).unwrap().minor().get(), 2);
        assert_eq!(
            parsed.into_inner(),
            vec!["/dev/first", "/dev/first", "510:2"]
        );
        assert_eq!(
            events.into_inner(),
            vec!["parse", "resolve", "parse", "parse", "resolve", "resolve"]
        );
    }

    #[ktest]
    fn validates_target_type_and_parameter_shapes() {
        let mut parse_calls = 0;
        let mut resolve_calls = 0;
        let mut parse_backing = |_token: &str| -> Result<DeviceId, ()> {
            parse_calls += 1;
            Ok(DeviceId::new(MajorId::new(510), MinorId::new(1)))
        };
        let mut resolve_backing = |_id: DeviceId| -> Result<BlockDeviceLease, ()> {
            resolve_calls += 1;
            Ok(backing(1))
        };

        assert_eq!(
            parse_target_with(
                "error",
                Sid::new(0),
                1,
                " \t",
                &mut parse_backing,
                &mut resolve_backing,
            )
            .unwrap()
            .name(),
            "error"
        );
        assert_eq!(
            parse_target_with(
                "zero",
                Sid::new(0),
                1,
                "",
                &mut parse_backing,
                &mut resolve_backing,
            )
            .unwrap()
            .name(),
            "zero"
        );
        for (target_type, params) in [
            ("error", "unexpected"),
            ("zero", "unexpected"),
            ("linear", "510:1"),
            ("linear", "510:1 invalid"),
            ("linear", "510:1 0 extra"),
            ("striped", "2 4 510:1 0"),
        ] {
            assert_eq!(
                parse_target_with(
                    target_type,
                    Sid::new(0),
                    1,
                    params,
                    &mut parse_backing,
                    &mut resolve_backing,
                )
                .unwrap_err(),
                DmTargetParseError::Table(TableError::InvalidTargetParams),
                "target_type={target_type:?}, params={params:?}"
            );
        }
        assert_eq!(
            parse_target_with(
                "snapshot",
                Sid::new(0),
                1,
                "",
                &mut parse_backing,
                &mut resolve_backing,
            )
            .unwrap_err(),
            DmTargetParseError::UnsupportedTarget
        );
        assert_eq!(parse_calls, 0);
        assert_eq!(resolve_calls, 0);
    }

    #[ktest]
    fn validates_geometry_before_backing_lookup() {
        let mut parse_calls = 0;
        let mut resolve_calls = 0;
        let mut parse_backing = |_token: &str| -> Result<DeviceId, ()> {
            parse_calls += 1;
            Ok(DeviceId::new(MajorId::new(510), MinorId::new(1)))
        };
        let mut resolve_backing = |_id: DeviceId| -> Result<BlockDeviceLease, ()> {
            resolve_calls += 1;
            Ok(backing(1))
        };

        assert_eq!(
            parse_target_with(
                "linear",
                Sid::new(u64::MAX),
                1,
                "510:1 0",
                &mut parse_backing,
                &mut resolve_backing,
            )
            .unwrap_err(),
            DmTargetParseError::Table(TableError::LogicalRangeOverflow)
        );
        assert_eq!(
            parse_target_with(
                "striped",
                Sid::new(0),
                0,
                "1 4 510:1 0",
                &mut parse_backing,
                &mut resolve_backing,
            )
            .unwrap_err(),
            DmTargetParseError::Table(TableError::ZeroLength)
        );
        assert_eq!(
            parse_target_with(
                "striped",
                Sid::new(0),
                1,
                "1 4 510:1 18446744073709551615",
                &mut parse_backing,
                &mut resolve_backing,
            )
            .unwrap_err(),
            DmTargetParseError::Table(TableError::BackingRangeOverflow)
        );
        assert_eq!(parse_calls, 0);
        assert_eq!(resolve_calls, 0);
    }

    #[ktest]
    fn preserves_backing_parse_and_lookup_errors() {
        let mut parse_backing = |_token: &str| -> Result<DeviceId, &'static str> { Err("parse") };
        let mut resolve_backing =
            |_id: DeviceId| -> Result<BlockDeviceLease, &'static str> { Ok(backing(1)) };
        assert_eq!(
            parse_target_with(
                "linear",
                Sid::new(0),
                1,
                "/dev/missing 0",
                &mut parse_backing,
                &mut resolve_backing,
            )
            .unwrap_err(),
            DmTargetParseError::ResolveBacking("parse")
        );

        let mut parse_backing = |_token: &str| -> Result<DeviceId, &'static str> {
            Ok(DeviceId::new(MajorId::new(510), MinorId::new(1)))
        };
        let mut resolve_backing =
            |_id: DeviceId| -> Result<BlockDeviceLease, &'static str> { Err("lookup") };
        assert_eq!(
            parse_target_with(
                "striped",
                Sid::new(0),
                8,
                "1 4 /dev/missing 0",
                &mut parse_backing,
                &mut resolve_backing,
            )
            .unwrap_err(),
            DmTargetParseError::ResolveBacking("lookup")
        );
    }
}
