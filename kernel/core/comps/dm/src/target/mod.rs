// SPDX-License-Identifier: MPL-2.0

//! Linear target parsing, geometry, and BIO mapping.

use alloc::{boxed::Box, string::String, vec::Vec};
#[cfg(ktest)]
use core::any::Any;
use core::{fmt::Debug, ops::Range};

use aster_block::{BlockDevice, BlockDeviceLease, id::Sid};
use device_id::DeviceId;

use self::linear::LinearTarget;
use crate::TableError;

mod args;
pub mod linear;

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

/// One linear mapping action for a table-local part of a BIO.
pub struct TargetIoAction<'a> {
    /// Logical range covered by this child BIO.
    pub(crate) logical_range: Range<Sid>,
    /// Backing sector corresponding to the start of the logical range.
    pub(crate) backing_start: Sid,
    /// Resolved backing device retained by the owning table.
    pub(crate) backing: &'a dyn BlockDevice,
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

    /// Maps a target-local logical range to backing I/O actions.
    fn map_io_range(&self, logical: Range<Sid>) -> Option<Vec<TargetIoAction<'_>>>;

    /// Formats target-specific Linux table/status parameters.
    fn status_params(&self, mode: TargetStatusMode) -> Result<String, TableError>;
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

const LINEAR_METADATA: DmTargetMetadata = DmTargetMetadata::new("linear", [1, 4, 0]);

/// Target types actually supported by the table parser and reported to userspace.
pub fn supported_target_metadata() -> &'static [DmTargetMetadata] {
    &[LINEAR_METADATA]
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
    if target_type != LINEAR_METADATA.name() {
        return Err(DmTargetParseError::UnsupportedTarget);
    }
    Ok(Box::new(LinearTarget::parse_with(
        logical_start,
        length,
        params,
        parse_backing,
        resolve_backing,
    )?))
}

#[cfg(ktest)]
mod tests {
    use alloc::sync::Arc;

    use aster_block::{
        BlockDeviceMeta,
        bio::{BioEnqueueError, SubmittedBio},
    };
    use device_id::{MajorId, MinorId};
    use ostd::prelude::ktest;

    use super::*;

    #[derive(Debug)]
    struct TestBlockDevice(DeviceId);

    impl BlockDevice for TestBlockDevice {
        fn enqueue(&self, _bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            unreachable!()
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 8,
                nr_sectors: 64,
            }
        }

        fn name(&self) -> &str {
            "linear-parser-test"
        }

        fn id(&self) -> DeviceId {
            self.0
        }
    }

    #[ktest]
    fn parses_linear_after_geometry_validation_and_holds_backing_lease() {
        use core::cell::Cell;

        let id = DeviceId::new(MajorId::new(510), MinorId::new(33));
        aster_block::register(Arc::new(TestBlockDevice(id))).unwrap();
        let parses = Cell::new(0);
        let resolves = Cell::new(0);
        let mut parse_backing = |token: &str| {
            parses.set(parses.get() + 1);
            assert_eq!(token, "510:33");
            Ok::<_, ()>(id)
        };
        let mut resolve_backing = |id| {
            resolves.set(resolves.get() + 1);
            Ok::<_, ()>(aster_block::lookup_lease(id).unwrap())
        };

        assert!(matches!(
            parse_target_with(
                "linear",
                Sid::new(0),
                0,
                "510:33 1",
                &mut parse_backing,
                &mut resolve_backing
            ),
            Err(DmTargetParseError::Table(TableError::ZeroLength))
        ));
        assert_eq!((parses.get(), resolves.get()), (0, 0));
        assert!(matches!(
            parse_target_with(
                "striped",
                Sid::new(0),
                1,
                "510:33 1",
                &mut parse_backing,
                &mut resolve_backing
            ),
            Err(DmTargetParseError::UnsupportedTarget)
        ));
        let target = parse_target_with(
            "linear",
            Sid::new(0),
            8,
            "510:33 1",
            &mut parse_backing,
            &mut resolve_backing,
        )
        .unwrap();
        assert_eq!(
            target.downcast_ref::<LinearTarget>().unwrap().backing_id(),
            id
        );
        assert_eq!((parses.get(), resolves.get()), (1, 1));
        assert_eq!(
            aster_block::unregister(id).unwrap_err(),
            aster_block::Error::Busy
        );
        drop(target);
        aster_block::unregister(id).unwrap();
    }
}
