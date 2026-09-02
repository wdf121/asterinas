// SPDX-License-Identifier: MPL-2.0

use core::{fmt::Debug, ops::Range};

use aster_block::{BlockDevice, BlockDeviceLease, id::Sid};
use device_id::DeviceId;

use crate::TableError;

/// A parsed `linear` target with a resolved backing device reference.
#[derive(Debug)]
pub struct LinearTarget {
    logical_range: Range<Sid>,
    backing_start: Sid,
    backing_id: DeviceId,
    backing: BlockDeviceLease,
}

impl LinearTarget {
    /// Creates a `linear` target and validates its logical and backing ranges.
    pub fn new(
        logical_start: Sid,
        length: u64,
        backing_start: Sid,
        backing: BlockDeviceLease,
    ) -> Result<Self, TableError> {
        let (logical_end, backing_end) =
            Self::validate_geometry(logical_start, length, backing_start)?;
        let backing_capacity = u64::try_from(backing.metadata().nr_sectors)
            .map_err(|_| TableError::BackingRangeOverflow)?;
        if backing_end > backing_capacity {
            return Err(TableError::BackingRangeOutOfBounds);
        }

        Ok(Self {
            logical_range: logical_start..Sid::new(logical_end),
            backing_start,
            backing_id: backing.id(),
            backing,
        })
    }

    pub(super) fn validate_geometry(
        logical_start: Sid,
        length: u64,
        backing_start: Sid,
    ) -> Result<(u64, u64), TableError> {
        if length == 0 {
            return Err(TableError::ZeroLength);
        }

        let logical_end = logical_start
            .to_raw()
            .checked_add(length)
            .ok_or(TableError::LogicalRangeOverflow)?;
        let backing_end = backing_start
            .to_raw()
            .checked_add(length)
            .ok_or(TableError::BackingRangeOverflow)?;
        Ok((logical_end, backing_end))
    }

    /// Returns the logical sector range covered by this target.
    pub fn logical_range(&self) -> &Range<Sid> {
        &self.logical_range
    }

    /// Returns the target length in 512-byte sectors.
    pub fn length(&self) -> u64 {
        self.logical_range.end.to_raw() - self.logical_range.start.to_raw()
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

    /// Maps a logical sector inside this target to a backing sector.
    pub fn map_sector(&self, logical: Sid) -> Option<Sid> {
        if !self.logical_range.contains(&logical) {
            return None;
        }

        let offset = logical
            .to_raw()
            .checked_sub(self.logical_range.start.to_raw())?;
        self.backing_start
            .to_raw()
            .checked_add(offset)
            .map(Sid::new)
    }
}

#[cfg(ktest)]
mod tests {
    use alloc::{string::String, sync::Arc};

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

        fn name(&self) -> String {
            String::from("linear-test")
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
