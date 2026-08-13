// SPDX-License-Identifier: MPL-2.0

#[cfg(ktest)]
use alloc::sync::Arc;

use aster_block::{
    BlockDeviceLease,
    bio::{BioEnqueueError, BioType, SubmittedBio},
    id::Sid,
};
use device_id::DeviceId;

use crate::{TableError, target::linear::LinearTarget};

/// 一份通过完整验证且安装后不可变的 Device Mapper 映射表。
#[derive(Debug)]
pub struct DmTable {
    linear: LinearTarget,
}

impl DmTable {
    /// 创建第一版支持的单段 linear 映射表。
    pub fn new_linear(
        logical_start: Sid,
        length: u64,
        backing_start: Sid,
        backing: BlockDeviceLease,
    ) -> Result<Self, TableError> {
        if logical_start.to_raw() != 0 {
            return Err(TableError::UnsupportedLogicalStart);
        }
        if backing
            .device()
            .as_ref()
            .downcast_ref::<crate::DmDevice>()
            .is_some()
        {
            return Err(TableError::UnsupportedBackingDevice);
        }

        Ok(Self {
            linear: LinearTarget::new(logical_start, length, backing_start, backing)?,
        })
    }

    /// 返回映射设备容量，单位为 512 字节扇区。
    pub fn length(&self) -> u64 {
        self.linear.length()
    }

    /// 返回映射设备的块层能力。
    pub fn metadata(&self) -> aster_block::BlockDeviceMeta {
        let backing = self.linear.backing().metadata();
        aster_block::BlockDeviceMeta {
            max_nr_segments_per_bio: backing.max_nr_segments_per_bio,
            nr_sectors: usize::try_from(self.length()).unwrap_or(usize::MAX),
        }
    }

    /// 返回唯一底层块设备的 ID。
    pub fn backing_id(&self) -> DeviceId {
        self.linear.backing_id()
    }

    /// 返回唯一的 linear target。
    pub fn linear(&self) -> &LinearTarget {
        &self.linear
    }

    /// 重映射并转发一个 BIO。
    pub fn enqueue(&self, mut bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        if bio.type_() == BioType::Flush {
            return self.linear.backing().enqueue(bio);
        }

        let range = bio.sid_range();
        let length = range
            .end
            .to_raw()
            .checked_sub(range.start.to_raw())
            .ok_or(BioEnqueueError::Refused)?;
        let logical_end = range
            .start
            .to_raw()
            .checked_add(length)
            .ok_or(BioEnqueueError::Refused)?;
        if range.start < self.linear.logical_range().start
            || logical_end > self.linear.logical_range().end.to_raw()
        {
            return Err(BioEnqueueError::Refused);
        }

        let backing_start = self
            .linear
            .map_sector(range.start)
            .ok_or(BioEnqueueError::Refused)?;
        bio.remap_sid_start(backing_start)?;
        self.linear.backing().enqueue(bio)
    }
}

#[cfg(ktest)]
mod tests {
    use alloc::{string::String, vec};

    use aster_block::{
        BlockDevice, BlockDeviceMeta,
        bio::{Bio, BioStatus},
    };
    use device_id::{MajorId, MinorId};
    use ostd::{prelude::ktest, sync::Mutex};

    use super::*;

    #[derive(Debug)]
    struct RecordingBlockDevice {
        last_range: Mutex<Option<core::ops::Range<Sid>>>,
    }

    impl RecordingBlockDevice {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                last_range: Mutex::new(None),
            })
        }
    }

    impl BlockDevice for RecordingBlockDevice {
        fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            *self.last_range.lock() = Some(bio.sid_range().clone());
            bio.complete(BioStatus::Complete);
            Ok(())
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 8,
                nr_sectors: 1_024,
            }
        }

        fn name(&self) -> String {
            String::from("dm-table-test")
        }

        fn id(&self) -> DeviceId {
            DeviceId::new(MajorId::new(1), MinorId::new(1))
        }
    }

    #[ktest]
    fn remaps_bio_start_to_backing_device() {
        let backing = RecordingBlockDevice::new();
        let table = Arc::new(
            DmTable::new_linear(
                Sid::new(0),
                128,
                Sid::new(100),
                BlockDeviceLease::new_untracked(backing.clone() as Arc<dyn BlockDevice>),
            )
            .unwrap(),
        );
        let read = Bio::new(BioType::Read, Sid::new(8), vec![], None);

        assert_eq!(
            read.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::Complete
        );
        assert_eq!(
            *backing.last_range.lock(),
            Some(Sid::new(108)..Sid::new(108))
        );
    }

    #[ktest]
    fn refuses_nonzero_logical_start() {
        let backing = RecordingBlockDevice::new() as Arc<dyn BlockDevice>;

        assert_eq!(
            DmTable::new_linear(
                Sid::new(1),
                8,
                Sid::new(0),
                BlockDeviceLease::new_untracked(backing)
            )
            .unwrap_err(),
            TableError::UnsupportedLogicalStart
        );
    }

    struct TableDevice(Arc<DmTable>);

    impl core::fmt::Debug for TableDevice {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str("TableDevice")
        }
    }

    impl BlockDevice for TableDevice {
        fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            self.0.enqueue(bio)
        }

        fn metadata(&self) -> BlockDeviceMeta {
            self.0.metadata()
        }

        fn name(&self) -> String {
            String::from("dm-table-wrapper")
        }

        fn id(&self) -> DeviceId {
            DeviceId::new(MajorId::new(2), MinorId::new(1))
        }
    }
}
