// SPDX-License-Identifier: MPL-2.0

//! Linux Device Mapper `linear` target support.
//!
//! A linear target maps a contiguous logical sector range to one backing block
//! device with a fixed sector offset. This file owns range arithmetic, capacity
//! validation, and single-sector remapping for that target; table-wide ordering
//! remains in `DmTable`.

use alloc::{format, string::String, vec::Vec};
use core::{fmt::Debug, ops::Range};

use aster_block::{BlockDevice, BlockDeviceLease, id::Sid};
use device_id::DeviceId;

use crate::{
    TableError,
    target::{
        DmTarget, DmTargetMetadata, DmTargetParseError, LINEAR_METADATA, TargetIoAction,
        TargetRange, TargetStatusMode,
    },
};

/// A `linear` target maps one contiguous logical range to one backing range.
#[derive(Debug)]
pub struct LinearTarget {
    /// Shared logical geometry used by table split and remap decisions.
    range: TargetRange,
    /// First backing sector that corresponds to `logical_range.start`.
    backing_start: Sid,
    /// Stable backing device identity used by deps/status reporting.
    backing_id: DeviceId,
    /// Lease that keeps the backing block device alive while the table is installed.
    backing: BlockDeviceLease,
}

impl LinearTarget {
    /// Parses one Linux `linear` target and resolves its single backing lease.
    pub(super) fn parse_with<E>(
        logical_start: Sid,
        length: u64,
        params: &str,
        parse_backing: &mut impl FnMut(&str) -> Result<DeviceId, E>,
        resolve_backing: &mut impl FnMut(DeviceId) -> Result<BlockDeviceLease, E>,
    ) -> Result<Self, DmTargetParseError<E>> {
        let (backing, backing_start) = parse_linear_params(params)?;
        let backing_start = Sid::new(backing_start);
        Self::validate_geometry(logical_start, length, backing_start)?;
        let backing = parse_backing(backing).map_err(DmTargetParseError::ResolveBacking)?;
        let backing = resolve_backing(backing).map_err(DmTargetParseError::ResolveBacking)?;
        Self::new(logical_start, length, backing_start, backing).map_err(Into::into)
    }

    /// Builds a resolved target and rejects zero length, overflow, or over-capacity ranges.
    pub fn new(
        logical_start: Sid,
        length: u64,
        backing_start: Sid,
        backing: BlockDeviceLease,
    ) -> Result<Self, TableError> {
        let (range, backing_end) = Self::validate_geometry(logical_start, length, backing_start)?;
        let backing_capacity = u64::try_from(backing.metadata().nr_sectors)
            .map_err(|_| TableError::BackingRangeOverflow)?;
        if backing_end > backing_capacity {
            return Err(TableError::BackingRangeOutOfBounds);
        }

        Ok(Self {
            range,
            backing_start,
            backing_id: backing.id(),
            backing,
        })
    }

    /// Checks pure range arithmetic so parser can fail before VFS or lease lookup.
    pub(super) fn validate_geometry(
        logical_start: Sid,
        length: u64,
        backing_start: Sid,
    ) -> Result<(TargetRange, u64), TableError> {
        let range = TargetRange::new(logical_start, length)?;
        let backing_end = backing_start
            .to_raw()
            .checked_add(range.length())
            .ok_or(TableError::BackingRangeOverflow)?;
        Ok((range, backing_end))
    }

    /// Returns the logical sector range covered by this target.
    pub fn logical_range(&self) -> &Range<Sid> {
        self.range.logical_range()
    }

    /// Returns the target length in 512-byte sectors.
    pub fn length(&self) -> u64 {
        self.range.length()
    }

    /// Returns the backing mapping start sector.
    pub fn backing_start(&self) -> Sid {
        self.backing_start
    }

    /// Returns the backing block device ID.
    pub fn backing_id(&self) -> DeviceId {
        self.backing_id
    }

    /// Returns the backing block device.
    pub fn backing(&self) -> &dyn BlockDevice {
        self.backing.device().as_ref()
    }

    /// Checks whether an I/O subrange can be remapped by this target.
    pub(super) fn contains_range(&self, logical: &Range<Sid>) -> bool {
        self.range.contains_range(logical)
    }

    /// Maps an in-range logical sector by adding the fixed backing offset.
    pub fn map_sector(&self, logical: Sid) -> Option<Sid> {
        let offset = self.range.offset_of(logical)?;
        self.backing_start
            .to_raw()
            .checked_add(offset)
            .map(Sid::new)
    }
}

impl DmTarget for LinearTarget {
    #[cfg(ktest)]
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn metadata(&self) -> DmTargetMetadata {
        LINEAR_METADATA
    }

    fn logical_range(&self) -> &Range<Sid> {
        self.logical_range()
    }

    fn length(&self) -> u64 {
        self.length()
    }

    fn for_each_backing_id(&self, f: &mut dyn FnMut(DeviceId)) {
        f(self.backing_id());
    }

    fn for_each_backing<'a>(&'a self, f: &mut dyn FnMut(&'a dyn BlockDevice)) {
        f(self.backing());
    }

    fn status_params(&self, mode: TargetStatusMode) -> Result<String, TableError> {
        match mode {
            TargetStatusMode::Table => {
                let backing = self.backing_id();
                Ok(format!(
                    "{}:{} {}",
                    backing.major().get(),
                    backing.minor().get(),
                    self.backing_start().to_raw()
                ))
            }
            TargetStatusMode::Status => Ok(String::new()),
        }
    }

    fn map_io_range(&self, logical: Range<Sid>) -> Option<Vec<TargetIoAction<'_>>> {
        if !self.contains_range(&logical) {
            return None;
        }
        Some(alloc::vec![TargetIoAction::Remap {
            backing_start: self.map_sector(logical.start)?,
            backing: self.backing(),
            logical_range: logical,
        }])
    }
}

/// Parses the two-field Linux `linear` parameter string without resolving the backing.
fn parse_linear_params(params: &str) -> Result<(&str, u64), TableError> {
    let mut fields = params.split_ascii_whitespace();
    let backing = fields.next().ok_or(TableError::InvalidTargetParams)?;
    let backing_start = fields
        .next()
        .ok_or(TableError::InvalidTargetParams)?
        .parse::<u64>()
        .map_err(|_| TableError::InvalidTargetParams)?;
    if fields.next().is_some() {
        return Err(TableError::InvalidTargetParams);
    }

    Ok((backing, backing_start))
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
    struct TestBlockDevice {
        id: DeviceId,
        nr_sectors: usize,
    }

    impl TestBlockDevice {
        fn new(nr_sectors: usize) -> Arc<Self> {
            Arc::new(Self {
                id: DeviceId::new(MajorId::new(1), MinorId::new(1)),
                nr_sectors,
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
                nr_sectors: self.nr_sectors,
            }
        }

        fn name(&self) -> &str {
            "linear-test"
        }

        fn id(&self) -> DeviceId {
            self.id
        }
    }

    #[ktest]
    fn validates_target_ranges() {
        let backing = TestBlockDevice::new(1_024);

        assert_eq!(
            LinearTarget::new(
                Sid::new(0),
                0,
                Sid::new(0),
                BlockDeviceLease::new_untracked(backing.clone())
            )
            .unwrap_err(),
            TableError::ZeroLength
        );
        assert_eq!(
            LinearTarget::new(
                Sid::new(u64::MAX),
                1,
                Sid::new(0),
                BlockDeviceLease::new_untracked(backing.clone())
            )
            .unwrap_err(),
            TableError::LogicalRangeOverflow
        );
        assert_eq!(
            LinearTarget::new(
                Sid::new(0),
                2,
                Sid::new(u64::MAX),
                BlockDeviceLease::new_untracked(backing.clone())
            )
            .unwrap_err(),
            TableError::BackingRangeOverflow
        );
        assert_eq!(
            LinearTarget::new(
                Sid::new(0),
                25,
                Sid::new(1_000),
                BlockDeviceLease::new_untracked(backing)
            )
            .unwrap_err(),
            TableError::BackingRangeOutOfBounds
        );
    }

    #[ktest]
    fn maps_end_exclusive_sector_range() {
        let backing = TestBlockDevice::new(1_024);
        let target = LinearTarget::new(
            Sid::new(10),
            8,
            Sid::new(100),
            BlockDeviceLease::new_untracked(backing),
        )
        .unwrap();

        assert_eq!(target.length(), 8);
        assert_eq!(target.map_sector(Sid::new(9)), None);
        assert_eq!(target.map_sector(Sid::new(10)), Some(Sid::new(100)));
        assert_eq!(target.map_sector(Sid::new(17)), Some(Sid::new(107)));
        assert_eq!(target.map_sector(Sid::new(18)), None);
    }
}
