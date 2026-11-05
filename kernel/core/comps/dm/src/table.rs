// SPDX-License-Identifier: MPL-2.0

//! Validated Device Mapper tables and linear I/O forwarding.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use aster_block::{
    BlockDevice, BlockDeviceLease,
    bio::{Bio, BioEnqueueError, BioStatus, BioType, SubmittedBio},
    id::Sid,
};
use device_id::DeviceId;
use ostd::sync::SpinLock;

use crate::{
    TableError,
    target::{DmTarget, DmTargetBox, TargetIoAction, linear::LinearTarget},
};

type BioPart<'a> = (core::ops::Range<Sid>, &'a dyn DmTarget);

/// A contiguous, immutable Device Mapper table installed on a mapped device.
#[derive(Debug)]
pub struct DmTable {
    /// Ordered targets whose logical ranges must start at zero and be contiguous.
    targets: Vec<DmTargetBox>,
    /// Total table capacity in 512-byte sectors, equal to the last target end.
    length: u64,
    /// Linux `DM_READONLY_FLAG` mode captured when this table was loaded.
    readonly: bool,
}

impl DmTable {
    /// Creates a compatibility table for tests and legacy callers that still build linear lists.
    pub fn new_linear(targets: Vec<LinearTarget>) -> Result<Self, TableError> {
        Self::new_targets(
            targets
                .into_iter()
                .map(|target| Box::new(target) as DmTargetBox)
                .collect(),
        )
    }

    /// Creates a writable table after enforcing Linux-visible table invariants not owned by targets.
    pub fn new_targets(targets: Vec<DmTargetBox>) -> Result<Self, TableError> {
        Self::new_targets_with_readonly(targets, false)
    }

    /// Creates a table with the mode captured from `DM_TABLE_LOAD`.
    pub fn new_targets_with_readonly(
        targets: Vec<DmTargetBox>,
        readonly: bool,
    ) -> Result<Self, TableError> {
        if targets.is_empty() {
            return Err(TableError::UnsupportedTargetCount);
        }

        let mut has_unsupported_backing = false;
        let mut expected_start = 0_u64;
        for target in &targets {
            if target.logical_range().start.to_raw() != expected_start {
                return Err(TableError::UnsupportedLogicalStart);
            }
            let mut check_backing = |backing: &dyn BlockDevice| {
                if backing.downcast_ref::<crate::DmDevice>().is_some() {
                    has_unsupported_backing = true;
                }
            };
            target.for_each_backing(&mut check_backing);
            if has_unsupported_backing {
                return Err(TableError::UnsupportedBackingDevice);
            }
            expected_start = target.logical_range().end.to_raw();
        }

        Ok(Self {
            targets,
            length: expected_start,
            readonly,
        })
    }

    /// Creates a mapping table from a single `linear` target.
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

    /// Returns the mapped device capacity in 512-byte sectors.
    pub fn length(&self) -> u64 {
        self.length
    }

    /// Returns whether this table rejects write-like BIOs after activation.
    pub fn is_readonly(&self) -> bool {
        self.readonly
    }

    /// Returns all targets.
    pub fn targets(&self) -> &[DmTargetBox] {
        &self.targets
    }

    /// Returns the number of targets.
    pub fn target_count(&self) -> usize {
        self.targets.len()
    }

    /// Returns unique backing block IDs in first-seen order for `DM_TABLE_DEPS`.
    pub fn backing_ids(&self) -> Vec<DeviceId> {
        let mut ids = Vec::new();
        for target in &self.targets {
            let mut collect_id = |id| {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            };
            target.for_each_backing_id(&mut collect_id);
        }
        ids
    }

    /// Returns block capacity and the strictest backing segment limit.
    pub fn metadata(&self) -> aster_block::BlockDeviceMeta {
        let mut max_nr_segments_per_bio = None;
        for target in &self.targets {
            let mut update = |backing: &dyn BlockDevice| {
                let value = backing.metadata().max_nr_segments_per_bio;
                max_nr_segments_per_bio = Some(
                    max_nr_segments_per_bio.map_or(value, |current: usize| current.min(value)),
                );
            };
            target.for_each_backing(&mut update);
        }
        aster_block::BlockDeviceMeta {
            max_nr_segments_per_bio: max_nr_segments_per_bio.unwrap_or(usize::MAX),
            nr_sectors: usize::try_from(self.length).unwrap_or(usize::MAX),
        }
    }

    /// Forwards one BIO through the linear targets, splitting at target boundaries.
    pub fn enqueue(self: &Arc<Self>, mut bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        if bio.type_() != BioType::Flush {
            // Reject out-of-range I/O before taking completion ownership.
            let parts = self.plan_normal_io(&bio)?;
            let table = self.clone();
            bio.chain_complete_fn(move |_| drop(table));
            return Self::execute_normal_io(bio, parts);
        }
        let table = self.clone();
        bio.chain_complete_fn(move |_| drop(table));
        self.enqueue_flush(bio)
    }

    fn plan_normal_io(
        &self,
        bio: &SubmittedBio,
    ) -> Result<Vec<TargetIoAction<'_>>, BioEnqueueError> {
        let range = bio.sid_range();
        let mut parts = Vec::new();
        for (logical, target) in self.bio_parts(range.start, range.end.to_raw())? {
            parts.extend(
                target
                    .map_io_range(logical)
                    .ok_or(BioEnqueueError::Refused)?,
            );
        }
        Ok(parts)
    }

    fn execute_normal_io(
        mut bio: SubmittedBio,
        parts: Vec<TargetIoAction<'_>>,
    ) -> Result<(), BioEnqueueError> {
        if parts.len() == 1 {
            let part = parts.into_iter().next().unwrap();
            bio.remap_sid_start(part.backing_start)?;
            return part.backing.enqueue(bio);
        }

        let ranges = parts
            .iter()
            .map(|part| part.logical_range.clone())
            .collect();
        let children = match bio.split(ranges) {
            Ok(children) => children,
            Err((error, original)) => {
                // No child has been accepted; the immediate submitter owns this failure.
                drop(original);
                return Err(error);
            }
        };
        for (mut child, part) in children.into_iter().zip(parts) {
            if child.remap_sid_start(part.backing_start).is_ok() {
                // Split children complete as IoError on drop if a backing refuses them.
                let _ = part.backing.enqueue(child);
            }
        }
        Ok(())
    }

    fn bio_parts(&self, start: Sid, end: u64) -> Result<Vec<BioPart<'_>>, BioEnqueueError> {
        let mut cursor = start.to_raw();
        let mut parts = Vec::new();
        while cursor < end {
            let target = self.target_at(cursor).ok_or(BioEnqueueError::Refused)?;
            let part_end = core::cmp::min(end, target.logical_range().end.to_raw());
            parts.push((Sid::new(cursor)..Sid::new(part_end), target));
            cursor = part_end;
        }
        Ok(parts)
    }

    fn target_at(&self, sector: u64) -> Option<&dyn DmTarget> {
        self.targets
            .iter()
            .map(|target| target.as_ref())
            .find(|target| {
                sector >= target.logical_range().start.to_raw()
                    && sector < target.logical_range().end.to_raw()
            })
    }

    /// Sends a flush once to every distinct backing and aggregates all completions.
    fn enqueue_flush(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        let mut flushed = Vec::new();
        for target in &self.targets {
            target.for_each_backing_id(&mut |id| {
                if !flushed.contains(&id) {
                    flushed.push(id);
                }
            });
        }
        if flushed.is_empty() {
            bio.complete(BioStatus::Complete);
            return Ok(());
        }

        let completion = Arc::new(FlushCompletion::new(flushed.len(), bio));
        let mut submitted = Vec::new();
        for target in &self.targets {
            let mut submit_flush = |backing: &dyn BlockDevice| {
                if submitted.contains(&backing.id()) {
                    return;
                }
                submitted.push(backing.id());
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
            };
            target.for_each_backing(&mut submit_flush);
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
            let original = self.original.disable_irq().lock().take().unwrap();
            original.complete(status);
        }
    }
}

#[cfg(ktest)]
mod tests {
    use alloc::{string::ToString, vec};
    use core::ops::Range;

    use aster_block::{
        BlockDeviceMeta,
        bio::{BioDirection, BioSegment},
        lookup_lease, register, unregister,
    };
    use device_id::{MajorId, MinorId};
    use ostd::{prelude::ktest, sync::Mutex};

    use super::*;
    use crate::DmManager;

    #[derive(Debug)]
    struct TestBlockDevice(DeviceId);

    impl BlockDevice for TestBlockDevice {
        fn enqueue(&self, _bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            unreachable!()
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 8,
                nr_sectors: 1024,
            }
        }

        fn name(&self) -> &str {
            "dm-table-test"
        }

        fn id(&self) -> DeviceId {
            self.0
        }
    }

    #[derive(Debug)]
    struct RecordingBlockDevice {
        id: DeviceId,
        ranges: Mutex<Vec<Range<Sid>>>,
        pending: Mutex<Vec<SubmittedBio>>,
        fail_enqueue: bool,
        queue_limit: usize,
    }

    impl RecordingBlockDevice {
        fn new(minor: u32, fail_enqueue: bool, queue_limit: usize) -> Arc<Self> {
            Arc::new(Self {
                id: DeviceId::new(MajorId::new(1), MinorId::new(minor)),
                ranges: Mutex::new(Vec::new()),
                pending: Mutex::new(Vec::new()),
                fail_enqueue,
                queue_limit,
            })
        }

        fn finish(&self, status: BioStatus) {
            self.pending.lock().remove(0).complete(status);
        }
    }

    impl BlockDevice for RecordingBlockDevice {
        fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            if self.fail_enqueue {
                return Err(BioEnqueueError::Refused);
            }
            self.ranges.lock().push(bio.sid_range().clone());
            self.pending.lock().push(bio);
            Ok(())
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: self.queue_limit,
                nr_sectors: 1024,
            }
        }

        fn name(&self) -> &str {
            "dm-recording-test"
        }

        fn id(&self) -> DeviceId {
            self.id
        }
    }

    #[derive(Debug)]
    struct TableDevice(Arc<DmTable>);

    impl BlockDevice for TableDevice {
        fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            self.0.enqueue(bio)
        }

        fn metadata(&self) -> BlockDeviceMeta {
            self.0.metadata()
        }

        fn name(&self) -> &str {
            "dm-table-wrapper"
        }

        fn id(&self) -> DeviceId {
            DeviceId::new(MajorId::new(1), MinorId::new(255))
        }
    }

    fn linear(backing: Arc<RecordingBlockDevice>, start: u64, offset: u64) -> LinearTarget {
        LinearTarget::new(
            Sid::new(start),
            4,
            Sid::new(offset),
            BlockDeviceLease::new_untracked(backing as Arc<dyn BlockDevice>),
        )
        .unwrap()
    }

    #[ktest]
    fn forwards_single_linear_bio_and_reports_metadata() {
        let backing = RecordingBlockDevice::new(1, false, 3);
        let table = Arc::new(DmTable::new_linear(vec![linear(backing.clone(), 0, 100)]).unwrap());
        assert_eq!(table.metadata().nr_sectors, 4);
        assert_eq!(table.metadata().max_nr_segments_per_bio, 3);
        let bio = Bio::new(
            BioType::Write,
            Sid::new(1),
            vec![BioSegment::alloc_exact(1, 1024, BioDirection::ToDevice)],
            None,
        );
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(&TableDevice(table), &mut batch).unwrap();
        assert_eq!(*backing.ranges.lock(), vec![Sid::new(101)..Sid::new(103)]);
        backing.finish(BioStatus::Complete);
        assert!(batch.wait_all().is_ok());
    }

    #[ktest]
    fn maps_both_sides_of_linear_target_boundary() {
        let first = RecordingBlockDevice::new(11, false, 8);
        let second = RecordingBlockDevice::new(12, false, 8);
        let table = Arc::new(
            DmTable::new_linear(vec![
                linear(first.clone(), 0, 100),
                linear(second.clone(), 4, 200),
            ])
            .unwrap(),
        );
        let bio = Bio::new(
            BioType::Write,
            Sid::new(2),
            vec![BioSegment::alloc_exact(1, 2048, BioDirection::ToDevice)],
            None,
        );
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(&TableDevice(table), &mut batch).unwrap();
        assert_eq!(*first.ranges.lock(), vec![Sid::new(102)..Sid::new(104)]);
        assert_eq!(*second.ranges.lock(), vec![Sid::new(200)..Sid::new(202)]);
        first.finish(BioStatus::Complete);
        second.finish(BioStatus::Complete);
        assert!(batch.wait_all().is_ok());
    }

    #[ktest]
    fn partial_cross_target_read_preserves_data_order() {
        use ostd::mm::VmIo;

        let first = RecordingBlockDevice::new(21, false, 8);
        let second = RecordingBlockDevice::new(22, false, 8);
        let table = Arc::new(
            DmTable::new_linear(vec![
                linear(first.clone(), 0, 100),
                linear(second.clone(), 4, 200),
            ])
            .unwrap(),
        );
        let segment = BioSegment::alloc_exact(1, 1024, BioDirection::FromDevice);
        segment.dma_slice().write_bytes(0, &[0x6d; 1024]).unwrap();
        let bio = Bio::new(BioType::Read, Sid::new(3), vec![segment.clone()], None);
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(&TableDevice(table), &mut batch).unwrap();
        assert_eq!(*first.ranges.lock(), vec![Sid::new(103)..Sid::new(104)]);
        assert_eq!(*second.ranges.lock(), vec![Sid::new(200)..Sid::new(201)]);
        {
            let pending = first.pending.lock();
            assert_eq!(pending[0].segments()[0].nbytes(), 512);
            pending[0].segments()[0]
                .dma_slice()
                .write_bytes(0, &[0x31; 512])
                .unwrap();
        }
        {
            let pending = second.pending.lock();
            assert_eq!(pending[0].segments()[0].nbytes(), 512);
            pending[0].segments()[0]
                .dma_slice()
                .write_bytes(0, &[0x92; 512])
                .unwrap();
        }
        second.finish(BioStatus::Complete);
        first.finish(BioStatus::Complete);
        assert!(batch.wait_all().is_ok());
        let mut observed = [0; 1024];
        segment.dma_slice().read_bytes(0, &mut observed).unwrap();
        assert_eq!(&observed[..512], &[0x31; 512]);
        assert_eq!(&observed[512..], &[0x92; 512]);
    }

    #[ktest]
    fn splits_linear_bio_and_waits_for_all_children_on_failure() {
        let first = RecordingBlockDevice::new(1, false, 8);
        let second = RecordingBlockDevice::new(2, true, 2);
        let table = Arc::new(
            DmTable::new_linear(vec![linear(first.clone(), 0, 100), linear(second, 4, 200)])
                .unwrap(),
        );
        assert_eq!(table.metadata().max_nr_segments_per_bio, 2);
        let completed = Arc::new(Mutex::new(None));
        let on_complete = completed.clone();
        let bio = Bio::new(
            BioType::Read,
            Sid::new(0),
            vec![BioSegment::alloc_exact(1, 4096, BioDirection::FromDevice)],
            Some(Box::new(move |status| *on_complete.lock() = Some(status))),
        );
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(&TableDevice(table), &mut batch).unwrap();
        assert_eq!(*first.ranges.lock(), vec![Sid::new(100)..Sid::new(104)]);
        assert_eq!(first.pending.lock().len(), 1);
        assert_eq!(*completed.lock(), None);
        first.finish(BioStatus::Complete);
        assert!(batch.wait_all().is_err());
        assert_eq!(*completed.lock(), Some(BioStatus::IoError));
    }

    #[ktest]
    fn flushes_distinct_linear_backings_once_and_aggregates_errors() {
        let first = RecordingBlockDevice::new(1, false, 8);
        let second = RecordingBlockDevice::new(2, false, 8);
        let table = Arc::new(
            DmTable::new_linear(vec![
                linear(first.clone(), 0, 0),
                linear(first.clone(), 4, 4),
                linear(second.clone(), 8, 8),
            ])
            .unwrap(),
        );
        let bio = Bio::new(BioType::Flush, Sid::new(0), vec![], None);
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(&TableDevice(table), &mut batch).unwrap();
        assert_eq!(first.pending.lock().len(), 1);
        assert_eq!(second.pending.lock().len(), 1);
        first.finish(BioStatus::IoError);
        second.finish(BioStatus::Complete);
        assert!(batch.wait_all().is_err());
    }

    #[ktest]
    fn flush_enqueue_failure_waits_for_other_backings() {
        let refusing = RecordingBlockDevice::new(1, true, 8);
        let pending = RecordingBlockDevice::new(2, false, 8);
        let table = Arc::new(
            DmTable::new_linear(vec![
                linear(refusing.clone(), 0, 0),
                linear(pending.clone(), 4, 4),
            ])
            .unwrap(),
        );
        let completed = Arc::new(Mutex::new(None));
        let on_complete = completed.clone();
        let bio = Bio::new(
            BioType::Flush,
            Sid::new(0),
            vec![],
            Some(Box::new(move |status| *on_complete.lock() = Some(status))),
        );
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(&TableDevice(table), &mut batch).unwrap();
        assert!(refusing.pending.lock().is_empty());
        assert_eq!(pending.pending.lock().len(), 1);
        assert_eq!(*completed.lock(), None);

        pending.finish(BioStatus::Complete);
        assert!(batch.wait_all().is_err());
        assert_eq!(*completed.lock(), Some(BioStatus::IoError));
    }

    #[ktest]
    fn refuses_bio_past_linear_table_end() {
        let backing = RecordingBlockDevice::new(1, false, 8);
        let table = Arc::new(DmTable::new_linear(vec![linear(backing.clone(), 0, 0)]).unwrap());
        let bio = Bio::new(
            BioType::Read,
            Sid::new(3),
            vec![BioSegment::alloc_exact(1, 1024, BioDirection::FromDevice)],
            None,
        );
        assert_eq!(
            bio.submit_and_wait(&TableDevice(table)),
            Err(BioEnqueueError::Refused)
        );
        assert!(backing.ranges.lock().is_empty());
    }

    #[ktest]
    fn split_failure_returns_enqueue_error_without_completing_parent() {
        let backing = RecordingBlockDevice::new(42, false, 8);
        let table = Arc::new(DmTable::new_linear(vec![linear(backing.clone(), 0, 0)]).unwrap());
        let completed = Arc::new(Mutex::new(false));
        let on_complete = completed.clone();
        // A zero-length normal BIO produces no mapping parts, so splitting it fails
        // before any backing can accept a child.
        let bio = Bio::new(
            BioType::Read,
            Sid::new(0),
            vec![],
            Some(Box::new(move |_| *on_complete.lock() = true)),
        );
        assert_eq!(
            bio.submit_and_wait(&TableDevice(table)),
            Err(BioEnqueueError::Refused)
        );
        assert!(!*completed.lock());
        assert!(backing.pending.lock().is_empty());
    }

    #[ktest]
    fn pending_bio_keeps_table_backing_lease_alive() {
        let backing = RecordingBlockDevice::new(41, false, 8);
        let id = backing.id();
        register(backing.clone()).unwrap();
        let table = Arc::new(
            DmTable::new_single_linear(Sid::new(0), 4, Sid::new(0), lookup_lease(id).unwrap())
                .unwrap(),
        );
        let bio = Bio::new(
            BioType::Read,
            Sid::new(0),
            vec![BioSegment::alloc_exact(1, 512, BioDirection::FromDevice)],
            None,
        );
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(&TableDevice(table), &mut batch).unwrap();
        assert_eq!(unregister(id).unwrap_err(), aster_block::Error::Busy);
        backing.finish(BioStatus::Complete);
        assert!(batch.wait_all().is_ok());
        unregister(id).unwrap();
    }

    #[ktest]
    fn inactive_table_refuses_direct_bio() {
        let id = DeviceId::new(MajorId::new(510), MinorId::new(36));
        register(Arc::new(TestBlockDevice(id))).unwrap();
        let manager = DmManager::new().unwrap();
        let mapper = manager
            .create("inactive-io".to_string(), None, None)
            .unwrap();
        let table =
            DmTable::new_single_linear(Sid::new(0), 8, Sid::new(0), lookup_lease(id).unwrap())
                .unwrap();
        mapper.load_table(Arc::new(table));
        let bio = Bio::new(BioType::Flush, Sid::new(0), vec![], None);
        assert_eq!(
            bio.submit_and_wait(mapper.as_ref()),
            Err(BioEnqueueError::Refused)
        );
        assert_eq!(mapper.metadata().nr_sectors, 0);
        drop(manager);
        drop(mapper);
        unregister(id).unwrap();
    }

    #[ktest]
    fn inactive_table_replacement_releases_old_backing() {
        let old_id = DeviceId::new(MajorId::new(510), MinorId::new(34));
        let new_id = DeviceId::new(MajorId::new(510), MinorId::new(35));
        register(Arc::new(TestBlockDevice(old_id))).unwrap();
        register(Arc::new(TestBlockDevice(new_id))).unwrap();
        let manager = DmManager::new().unwrap();
        let mapper = manager
            .create("table-replace".to_string(), None, None)
            .unwrap();
        let make_table = |id| {
            Arc::new(
                DmTable::new_single_linear(Sid::new(0), 8, Sid::new(0), lookup_lease(id).unwrap())
                    .unwrap(),
            )
        };
        mapper.load_table(make_table(old_id));
        assert_eq!(unregister(old_id).unwrap_err(), aster_block::Error::Busy);
        mapper.load_table(make_table(new_id));
        unregister(old_id).unwrap();
        assert_eq!(unregister(new_id).unwrap_err(), aster_block::Error::Busy);
        drop(manager);
        drop(mapper);
        unregister(new_id).unwrap();
    }

    #[ktest]
    fn failed_candidate_preserves_existing_inactive_table() {
        let old_id = DeviceId::new(MajorId::new(510), MinorId::new(37));
        let candidate_id = DeviceId::new(MajorId::new(510), MinorId::new(38));
        register(Arc::new(TestBlockDevice(old_id))).unwrap();
        register(Arc::new(TestBlockDevice(candidate_id))).unwrap();
        let manager = DmManager::new().unwrap();
        let mapper = manager
            .create("table-failed-replace".to_string(), None, None)
            .unwrap();

        let old_table = Arc::new(
            DmTable::new_single_linear(Sid::new(0), 8, Sid::new(0), lookup_lease(old_id).unwrap())
                .unwrap(),
        );
        mapper.load_table(old_table.clone());
        let first = LinearTarget::new(
            Sid::new(0),
            8,
            Sid::new(0),
            lookup_lease(candidate_id).unwrap(),
        )
        .unwrap();
        let gap = LinearTarget::new(
            Sid::new(9),
            8,
            Sid::new(8),
            lookup_lease(candidate_id).unwrap(),
        )
        .unwrap();
        assert_eq!(
            DmTable::new_linear(vec![first, gap]).unwrap_err(),
            TableError::UnsupportedLogicalStart
        );
        assert!(Arc::ptr_eq(&mapper.inactive_table().unwrap(), &old_table));
        assert_eq!(mapper.inactive_table().unwrap().backing_ids(), vec![old_id]);
        unregister(candidate_id).unwrap();
        assert_eq!(unregister(old_id).unwrap_err(), aster_block::Error::Busy);
        drop(old_table);
        drop(manager);
        drop(mapper);
        unregister(old_id).unwrap();
    }

    #[ktest]
    fn holds_backing_leases_in_inactive_table_and_releases_on_failure() {
        let id = DeviceId::new(MajorId::new(510), MinorId::new(32));
        register(Arc::new(TestBlockDevice(id))).unwrap();
        let manager = DmManager::new().unwrap();
        let mapper = manager
            .create("table-test".to_string(), None, None)
            .unwrap();

        assert_eq!(
            LinearTarget::new(Sid::new(0), 2, Sid::new(1023), lookup_lease(id).unwrap())
                .unwrap_err(),
            TableError::BackingRangeOutOfBounds
        );
        let first =
            LinearTarget::new(Sid::new(0), 8, Sid::new(1), lookup_lease(id).unwrap()).unwrap();
        let gap =
            LinearTarget::new(Sid::new(9), 8, Sid::new(16), lookup_lease(id).unwrap()).unwrap();
        assert_eq!(
            DmTable::new_linear(vec![first, gap]).unwrap_err(),
            TableError::UnsupportedLogicalStart
        );
        // A rejected candidate drops both leases and leaves the inactive slot empty.
        assert!(mapper.inactive_table().is_none());
        assert!(unregister(id).is_ok());
        register(Arc::new(TestBlockDevice(id))).unwrap();

        let first =
            LinearTarget::new(Sid::new(0), 8, Sid::new(1), lookup_lease(id).unwrap()).unwrap();
        let second =
            LinearTarget::new(Sid::new(8), 8, Sid::new(16), lookup_lease(id).unwrap()).unwrap();
        let table = Arc::new(DmTable::new_linear(vec![first, second]).unwrap());
        assert_eq!(table.length(), 16);
        assert_eq!(table.target_count(), 2);
        assert_eq!(table.backing_ids(), vec![id]);
        mapper.load_table(table);
        assert_eq!(mapper.inactive_table().unwrap().length(), 16);
        assert_eq!(unregister(id).unwrap_err(), aster_block::Error::Busy);
        drop(manager);
        drop(mapper);
        unregister(id).unwrap();
    }
}
