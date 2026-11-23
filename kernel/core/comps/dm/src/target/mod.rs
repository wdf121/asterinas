// SPDX-License-Identifier: MPL-2.0

//! Target parsing, geometry, and BIO mapping.

use alloc::{boxed::Box, string::String, vec::Vec};
#[cfg(ktest)]
use core::any::Any;
use core::{fmt::Debug, ops::Range};

use aster_block::{BlockDevice, bio::BioStatus, id::Sid};
use device_id::DeviceId;

use crate::TableError;

mod args;
mod registry;

pub use registry::{
    BackingLookupError, DmTargetType, TargetRegistrationError, parse_target_with,
    register_target_type, reported_target_metadata,
};
pub mod linear;
mod striped;

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
///
/// A target owns its geometry and backing lifetimes;
/// the table validates topology and the I/O layer submits the resulting actions.
/// Mapping and status queries may run concurrently with backing completions.
/// Implementations must keep mapping geometry stable for the instance's lifetime
/// and synchronize any mutable runtime state internally.
pub trait DmTarget: Debug + Send + Sync {
    /// Returns this target as `Any` for ktest-only downcasting of boxed targets.
    #[cfg(ktest)]
    fn as_any(&self) -> &dyn Any;

    /// Returns static Linux-visible metadata for this target implementation.
    ///
    /// The name and version must remain unchanged and match the registered factory.
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

    /// Visits resolved backing devices in Linux table order, including duplicates.
    ///
    /// The enumeration order and device identities must remain stable while the
    /// target belongs to a table. The target must retain references or leases that
    /// keep these devices alive. Within a table, equal `DeviceId`s must identify
    /// the same effective device; consumers such as flush may deduplicate by ID.
    /// Device identities are obtained from these objects rather than enumerated separately.
    fn for_each_backing<'a>(&'a self, f: &mut dyn FnMut(&'a dyn BlockDevice));

    /// Maps a table-relative logical subrange owned by this target to backing actions.
    ///
    /// Actions must cover the requested range exactly, in order and without gaps or
    /// overlap. Each action maps an equal-length backing range without sector
    /// overflow. Each backing must be one of the devices exposed by this target's
    /// `for_each_backing()`, retained by the target throughout execution.
    /// Returns `None` when the requested range cannot be mapped.
    fn map_io_range(&self, logical: Range<Sid>) -> Option<Vec<TargetIoAction<'_>>>;

    /// Returns target-specific completion processing, if this target needs it.
    ///
    /// Whether this returns `Some` or `None` must remain stable for the instance's
    /// lifetime. DM may query it again when an asynchronous request completes.
    fn io_completion(&self) -> Option<&dyn TargetIoCompletion> {
        None
    }

    /// Formats target-specific Linux table/status parameters.
    fn status_params(&self, mode: TargetStatusMode) -> Result<String, TableError>;
}

/// Target-specific processing of terminal backing I/O results.
///
/// DM invokes this before completing the corresponding logical request. Admission
/// failures are not terminal backing results and do not invoke this interface.
/// Notifications report only completed backing operations belonging to this target.
/// This observer cannot replace the result, retry or requeue the request,
/// or take over asynchronous completion.
/// It must finish before the parent is notified.
/// Callbacks may run synchronously during submission or in interrupt context,
/// and may execute concurrently for different backing operations.
/// Implementations must not sleep or acquire the mapper's lifecycle lock.
/// Each completed flush destination notifies every target that references it once.
pub trait TargetIoCompletion: Send + Sync {
    /// Processes a completed operation on a backing device used by this target.
    fn complete_io(&self, backing: DeviceId, status: BioStatus);
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

#[cfg(ktest)]
mod tests {
    use alloc::sync::Arc;

    use aster_block::{
        BlockDeviceMeta,
        bio::{BioEnqueueError, SubmittedBio},
    };
    use device_id::{MajorId, MinorId};
    use ostd::prelude::ktest;

    use super::{linear::LinearTarget, *};

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
                "unknown",
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
