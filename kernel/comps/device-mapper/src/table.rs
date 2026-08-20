// SPDX-License-Identifier: MPL-2.0

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{
    ops::Range,
    sync::atomic::{AtomicU32, AtomicUsize, Ordering},
};

use aster_block::{
    BlockDeviceLease,
    bio::{Bio, BioEnqueueError, BioStatus, BioType, SubmittedBio},
    id::Sid,
};
use device_id::DeviceId;
use ostd::sync::SpinLock;

use crate::{
    TableError,
    target::{DmTarget, linear::LinearTarget},
};

/// 一份通过完整验证且安装后不可变的 Device Mapper 映射表。
#[derive(Debug)]
pub struct DmTable {
    targets: Vec<DmTarget>,
    length: u64,
}

impl DmTable {
    /// 创建由一段或多段 linear target 组成的映射表。
    pub fn new_linear(targets: Vec<LinearTarget>) -> Result<Self, TableError> {
        Self::new_targets(targets.into_iter().map(DmTarget::Linear).collect())
    }

    fn new_targets(targets: Vec<DmTarget>) -> Result<Self, TableError> {
        if targets.is_empty() {
            return Err(TableError::UnsupportedTargetCount);
        }

        let mut expected_start = 0_u64;
        for target in &targets {
            if target.logical_range().start.to_raw() != expected_start {
                return Err(TableError::UnsupportedLogicalStart);
            }
            if target.backing().downcast_ref::<crate::DmDevice>().is_some() {
                return Err(TableError::UnsupportedBackingDevice);
            }
            expected_start = target.logical_range().end.to_raw();
        }

        Ok(Self {
            targets,
            length: expected_start,
        })
    }

    /// 创建单段 linear 映射表。
    pub fn new_single_linear(
        logical_start: Sid,
        length: u64,
        backing_start: Sid,
        backing: BlockDeviceLease,
    ) -> Result<Self, TableError> {
        Self::new_linear(alloc::vec![LinearTarget::new(
            logical_start,
            length,
            backing_start,
            backing,
        )?])
    }

    /// 返回映射设备容量，单位为 512 字节扇区。
    pub fn length(&self) -> u64 {
        self.length
    }

    /// 返回映射设备的块层能力。
    pub fn metadata(&self) -> aster_block::BlockDeviceMeta {
        let max_nr_segments_per_bio = self
            .targets
            .iter()
            .map(|target| target.backing().metadata().max_nr_segments_per_bio)
            .min()
            .unwrap_or(0);
        aster_block::BlockDeviceMeta {
            max_nr_segments_per_bio,
            nr_sectors: usize::try_from(self.length()).unwrap_or(usize::MAX),
        }
    }

    /// 返回所有 target。
    pub fn targets(&self) -> &[DmTarget] {
        &self.targets
    }

    /// 返回 target 数量。
    pub fn target_count(&self) -> usize {
        self.targets.len()
    }

    /// 返回底层块设备 ID 列表，按首次出现顺序去重。
    pub fn backing_ids(&self) -> Vec<DeviceId> {
        let mut ids = Vec::new();
        for target in &self.targets {
            let id = target.backing_id();
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids
    }

    /// 重映射并转发一个 BIO。
    pub fn enqueue(&self, mut bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        if bio.type_() == BioType::Flush {
            return self.enqueue_flush(bio);
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

        let parts = self.bio_parts(range.start, logical_end)?;
        if parts.len() == 1 {
            let (range, target) = parts.into_iter().next().unwrap();
            let backing_start = target
                .map_sector(range.start)
                .ok_or(BioEnqueueError::Refused)?;
            bio.remap_sid_start(backing_start)?;
            return target.backing().enqueue(bio);
        }

        let ranges = parts
            .iter()
            .map(|(range, _)| range.clone())
            .collect::<Vec<_>>();
        let (children, completion) = bio.split(ranges)?;
        for (mut child, (range, target)) in children.into_iter().zip(parts) {
            let Some(backing_start) = target.map_sector(range.start) else {
                completion.complete_child(BioStatus::IoError);
                continue;
            };
            if child.remap_sid_start(backing_start).is_err() {
                completion.complete_child(BioStatus::IoError);
                continue;
            }
            if target.backing().enqueue(child).is_err() {
                completion.complete_child(BioStatus::IoError);
            }
        }
        Ok(())
    }

    fn bio_parts(
        &self,
        start: Sid,
        end: u64,
    ) -> Result<Vec<(Range<Sid>, &DmTarget)>, BioEnqueueError> {
        let mut cursor = start.to_raw();
        let mut parts = Vec::new();
        while cursor < end {
            let target = self
                .targets
                .iter()
                .find(|target| {
                    cursor >= target.logical_range().start.to_raw()
                        && cursor < target.logical_range().end.to_raw()
                })
                .ok_or(BioEnqueueError::Refused)?;
            let part_end = core::cmp::min(end, target.logical_range().end.to_raw());
            parts.push((Sid::new(cursor)..Sid::new(part_end), target));
            cursor = part_end;
        }
        Ok(parts)
    }

    fn enqueue_flush(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        let mut flushed = Vec::new();
        let mut backings = Vec::new();
        for target in &self.targets {
            let backing_id = target.backing_id();
            if flushed.contains(&backing_id) {
                continue;
            }
            flushed.push(backing_id);
            backings.push(target.backing());
        }

        if backings.is_empty() {
            bio.complete(BioStatus::Complete);
            return Ok(());
        }

        let completion = Arc::new(FlushCompletion::new(backings.len(), bio));
        for backing in backings {
            let completion = completion.clone();
            let callback_completion = completion.clone();
            let flush = Bio::new(
                BioType::Flush,
                Sid::new(0),
                Vec::new(),
                Some(Box::new(move |status| {
                    callback_completion.complete_one(status)
                })),
            );
            let mut io_batch = io_util::batch::IoBatch::with_capacity(1);
            if flush.submit(backing, &mut io_batch).is_err() {
                completion.complete_one(BioStatus::IoError);
            }
        }
        Ok(())
    }
}

struct FlushCompletion {
    remaining: AtomicUsize,
    status: AtomicU32,
    original: SpinLock<Option<SubmittedBio>>,
}

impl FlushCompletion {
    fn new(remaining: usize, original: SubmittedBio) -> Self {
        Self {
            remaining: AtomicUsize::new(remaining),
            status: AtomicU32::new(BioStatus::Complete as u32),
            original: SpinLock::new(Some(original)),
        }
    }

    fn complete_one(&self, status: BioStatus) {
        if status != BioStatus::Complete {
            let _ = self.status.compare_exchange(
                BioStatus::Complete as u32,
                status as u32,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
        }

        let previous = self.remaining.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
        if previous == 1 {
            let status = BioStatus::try_from(self.status.load(Ordering::Acquire)).unwrap();
            let original = self
                .original
                .disable_irq()
                .lock()
                .take()
                .expect("flush BIO original must complete exactly once");
            original.complete(status);
        }
    }
}

#[cfg(ktest)]
mod tests {
    use alloc::{string::String, vec};

    use aster_block::{
        BlockDevice, BlockDeviceMeta,
        bio::{BioDirection, BioSegment},
    };
    use device_id::{MajorId, MinorId};
    use ostd::{prelude::ktest, sync::Mutex};

    use super::*;

    #[derive(Debug)]
    struct RecordingBlockDevice {
        id: DeviceId,
        last_range: Mutex<Option<Range<Sid>>>,
        submitted_ranges: Mutex<Vec<Range<Sid>>>,
        flush_count: Mutex<usize>,
        flush_status: BioStatus,
        fail_flush_enqueue: bool,
    }

    impl RecordingBlockDevice {
        fn new(minor: u32) -> Arc<Self> {
            Self::new_with_flush(minor, BioStatus::Complete, false)
        }

        fn new_with_flush(
            minor: u32,
            flush_status: BioStatus,
            fail_flush_enqueue: bool,
        ) -> Arc<Self> {
            Arc::new(Self {
                id: DeviceId::new(MajorId::new(1), MinorId::new(minor)),
                last_range: Mutex::new(None),
                submitted_ranges: Mutex::new(Vec::new()),
                flush_count: Mutex::new(0),
                flush_status,
                fail_flush_enqueue,
            })
        }
    }

    impl BlockDevice for RecordingBlockDevice {
        fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            if bio.type_() == BioType::Flush {
                if self.fail_flush_enqueue {
                    return Err(BioEnqueueError::Refused);
                }
                *self.flush_count.lock() += 1;
                *self.last_range.lock() = Some(bio.sid_range().clone());
                self.submitted_ranges.lock().push(bio.sid_range().clone());
                bio.complete(self.flush_status);
                return Ok(());
            }
            *self.last_range.lock() = Some(bio.sid_range().clone());
            self.submitted_ranges.lock().push(bio.sid_range().clone());
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
            self.id
        }
    }

    #[ktest]
    fn remaps_bio_start_to_backing_device() {
        let backing = RecordingBlockDevice::new(1);
        let table = Arc::new(
            DmTable::new_single_linear(
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
    fn splits_bio_across_linear_target_boundary() {
        let first = RecordingBlockDevice::new(1);
        let second = RecordingBlockDevice::new(2);
        let table = Arc::new(
            DmTable::new_linear(vec![
                LinearTarget::new(
                    Sid::new(0),
                    4,
                    Sid::new(100),
                    BlockDeviceLease::new_untracked(first.clone() as Arc<dyn BlockDevice>),
                )
                .unwrap(),
                LinearTarget::new(
                    Sid::new(4),
                    4,
                    Sid::new(200),
                    BlockDeviceLease::new_untracked(second.clone() as Arc<dyn BlockDevice>),
                )
                .unwrap(),
            ])
            .unwrap(),
        );
        let read = Bio::new(
            BioType::Read,
            Sid::new(0),
            vec![BioSegment::alloc(1, BioDirection::FromDevice)],
            None,
        );

        assert_eq!(
            read.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::Complete
        );
        assert_eq!(
            *first.submitted_ranges.lock(),
            vec![Sid::new(100)..Sid::new(104)]
        );
        assert_eq!(
            *second.submitted_ranges.lock(),
            vec![Sid::new(200)..Sid::new(204)]
        );
    }

    #[ktest]
    fn supports_multiple_contiguous_linear_targets() {
        let first = RecordingBlockDevice::new(1);
        let second = RecordingBlockDevice::new(2);
        let table = DmTable::new_linear(vec![
            LinearTarget::new(
                Sid::new(0),
                128,
                Sid::new(100),
                BlockDeviceLease::new_untracked(first.clone() as Arc<dyn BlockDevice>),
            )
            .unwrap(),
            LinearTarget::new(
                Sid::new(128),
                64,
                Sid::new(200),
                BlockDeviceLease::new_untracked(second.clone() as Arc<dyn BlockDevice>),
            )
            .unwrap(),
        ])
        .unwrap();

        assert_eq!(table.length(), 192);
        assert_eq!(table.backing_ids(), vec![first.id(), second.id()]);
    }

    #[ktest]
    fn stores_linear_targets_as_ordered_dm_targets() {
        let first = RecordingBlockDevice::new(1);
        let second = RecordingBlockDevice::new(2);
        let table = DmTable::new_linear(vec![
            LinearTarget::new(
                Sid::new(0),
                128,
                Sid::new(100),
                BlockDeviceLease::new_untracked(first.clone() as Arc<dyn BlockDevice>),
            )
            .unwrap(),
            LinearTarget::new(
                Sid::new(128),
                64,
                Sid::new(200),
                BlockDeviceLease::new_untracked(second.clone() as Arc<dyn BlockDevice>),
            )
            .unwrap(),
        ])
        .unwrap();

        assert_eq!(table.target_count(), 2);
        let DmTarget::Linear(first_target) = &table.targets()[0];
        assert_eq!(first_target.backing_id(), first.id());
        assert_eq!(first_target.logical_range(), &(Sid::new(0)..Sid::new(128)));
        let DmTarget::Linear(second_target) = &table.targets()[1];
        assert_eq!(second_target.backing_id(), second.id());
        assert_eq!(
            second_target.logical_range(),
            &(Sid::new(128)..Sid::new(192))
        );
    }

    #[ktest]
    fn flushes_each_backing_device_once() {
        let first = RecordingBlockDevice::new(1);
        let second = RecordingBlockDevice::new(2);
        let table = Arc::new(
            DmTable::new_linear(vec![
                LinearTarget::new(
                    Sid::new(0),
                    128,
                    Sid::new(100),
                    BlockDeviceLease::new_untracked(first.clone() as Arc<dyn BlockDevice>),
                )
                .unwrap(),
                LinearTarget::new(
                    Sid::new(128),
                    64,
                    Sid::new(228),
                    BlockDeviceLease::new_untracked(first.clone() as Arc<dyn BlockDevice>),
                )
                .unwrap(),
                LinearTarget::new(
                    Sid::new(192),
                    64,
                    Sid::new(200),
                    BlockDeviceLease::new_untracked(second.clone() as Arc<dyn BlockDevice>),
                )
                .unwrap(),
            ])
            .unwrap(),
        );
        let flush = Bio::new(BioType::Flush, Sid::new(0), vec![], None);

        assert_eq!(
            flush.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::Complete
        );
        assert_eq!(*first.flush_count.lock(), 1);
        assert_eq!(*second.flush_count.lock(), 1);
    }

    #[ktest]
    fn propagates_flush_completion_failure() {
        let first = RecordingBlockDevice::new(1);
        let second = RecordingBlockDevice::new_with_flush(2, BioStatus::IoError, false);
        let table = Arc::new(
            DmTable::new_linear(vec![
                LinearTarget::new(
                    Sid::new(0),
                    128,
                    Sid::new(100),
                    BlockDeviceLease::new_untracked(first as Arc<dyn BlockDevice>),
                )
                .unwrap(),
                LinearTarget::new(
                    Sid::new(128),
                    64,
                    Sid::new(200),
                    BlockDeviceLease::new_untracked(second as Arc<dyn BlockDevice>),
                )
                .unwrap(),
            ])
            .unwrap(),
        );
        let flush = Bio::new(BioType::Flush, Sid::new(0), vec![], None);

        assert_eq!(
            flush.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::IoError
        );
    }

    #[ktest]
    fn completes_flush_with_error_when_backing_enqueue_fails() {
        let backing = RecordingBlockDevice::new_with_flush(1, BioStatus::Complete, true);
        let table = Arc::new(
            DmTable::new_single_linear(
                Sid::new(0),
                128,
                Sid::new(100),
                BlockDeviceLease::new_untracked(backing as Arc<dyn BlockDevice>),
            )
            .unwrap(),
        );
        let flush = Bio::new(BioType::Flush, Sid::new(0), vec![], None);

        assert_eq!(
            flush.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::IoError
        );
    }

    #[ktest]
    fn reports_capacity_and_queue_limits_from_linear_targets() {
        let first = RecordingBlockDevice::new(1);
        let second = RecordingBlockDevice::new(2);
        let table = DmTable::new_linear(vec![
            LinearTarget::new(
                Sid::new(0),
                128,
                Sid::new(100),
                BlockDeviceLease::new_untracked(first as Arc<dyn BlockDevice>),
            )
            .unwrap(),
            LinearTarget::new(
                Sid::new(128),
                64,
                Sid::new(200),
                BlockDeviceLease::new_untracked(second as Arc<dyn BlockDevice>),
            )
            .unwrap(),
        ])
        .unwrap();

        assert_eq!(table.length(), 192);
        assert_eq!(table.metadata().nr_sectors, 192);
        assert_eq!(table.metadata().max_nr_segments_per_bio, 8);
    }

    #[ktest]
    fn refuses_bio_outside_linear_table_range() {
        let backing = RecordingBlockDevice::new(1);
        let table = Arc::new(
            DmTable::new_single_linear(
                Sid::new(0),
                4,
                Sid::new(100),
                BlockDeviceLease::new_untracked(backing as Arc<dyn BlockDevice>),
            )
            .unwrap(),
        );
        let read = Bio::new(
            BioType::Read,
            Sid::new(4),
            vec![BioSegment::alloc(1, BioDirection::FromDevice)],
            None,
        );

        assert_eq!(
            read.submit_and_wait(&TableDevice(table)).unwrap_err(),
            BioEnqueueError::Refused
        );
    }

    #[ktest]
    fn refuses_nonzero_or_gapped_logical_start() {
        let backing = RecordingBlockDevice::new(1) as Arc<dyn BlockDevice>;

        assert_eq!(
            DmTable::new_single_linear(
                Sid::new(1),
                8,
                Sid::new(0),
                BlockDeviceLease::new_untracked(backing.clone())
            )
            .unwrap_err(),
            TableError::UnsupportedLogicalStart
        );

        assert_eq!(
            DmTable::new_linear(vec![
                LinearTarget::new(
                    Sid::new(0),
                    8,
                    Sid::new(0),
                    BlockDeviceLease::new_untracked(backing.clone())
                )
                .unwrap(),
                LinearTarget::new(
                    Sid::new(9),
                    8,
                    Sid::new(0),
                    BlockDeviceLease::new_untracked(backing)
                )
                .unwrap(),
            ])
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
