// SPDX-License-Identifier: MPL-2.0

//! Validated Device Mapper tables and BIO mapping logic.
//!
//! A table owns the ordered target list installed on one mapped block device. It
//! validates table-wide invariants, exposes block-layer metadata, splits incoming
//! BIOs at target boundaries, delegates target-local mapping, and aggregates child
//! completions back into the original BIO.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{
    ops::Range,
    sync::atomic::{AtomicU32, AtomicUsize, Ordering},
};

use aster_block::{
    BlockDevice, BlockDeviceLease,
    bio::{Bio, BioEnqueueError, BioStatus, BioType, SubmittedBio},
    id::Sid,
};
use device_id::DeviceId;
use ostd::{mm::io::util::HasVmReaderWriter, sync::SpinLock};

use crate::{
    TableError,
    target::{DmTarget, DmTargetBox, TargetIoAction, linear::LinearTarget},
};

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

    /// Returns block-layer metadata derived from table length and backing queue limits.
    pub fn metadata(&self) -> aster_block::BlockDeviceMeta {
        let mut max_nr_segments_per_bio = None;
        for target in &self.targets {
            let mut update_queue_limit = |backing: &dyn BlockDevice| {
                let value = backing.metadata().max_nr_segments_per_bio;
                max_nr_segments_per_bio = Some(
                    max_nr_segments_per_bio.map_or(value, |current: usize| current.min(value)),
                );
            };
            target.for_each_backing(&mut update_queue_limit);
        }
        aster_block::BlockDeviceMeta {
            max_nr_segments_per_bio: max_nr_segments_per_bio.unwrap_or(usize::MAX),
            nr_sectors: usize::try_from(self.length()).unwrap_or(usize::MAX),
        }
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

    /// Maps one non-flush BIO into child actions, preserving one completion for the caller.
    pub fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        if bio.type_() == BioType::Flush {
            return self.enqueue_flush(bio);
        }

        let plan = self.plan_normal_io(&bio)?;
        Self::execute_normal_io(bio, plan)
    }

    /// Builds a non-flush mapping plan without changing the submitted BIO.
    fn plan_normal_io(&self, bio: &SubmittedBio) -> Result<NormalIoPlan<'_>, BioEnqueueError> {
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
        let mut parts = Vec::new();
        for (range, target) in self.bio_parts(range.start, logical_end)? {
            parts.extend(target.map_io_range(range).ok_or(BioEnqueueError::Refused)?);
        }
        Ok(NormalIoPlan { parts })
    }

    /// Executes one completed normal-I/O plan and aggregates split child completion.
    fn execute_normal_io(
        mut bio: SubmittedBio,
        plan: NormalIoPlan<'_>,
    ) -> Result<(), BioEnqueueError> {
        let parts = plan.parts;
        if parts.len() == 1 {
            let part = parts.into_iter().next().unwrap();
            match part {
                TargetIoAction::Remap {
                    backing_start,
                    backing,
                    ..
                } => {
                    bio.remap_sid_start(backing_start)?;
                    return backing.enqueue(bio);
                }
                TargetIoAction::Error { .. } => {
                    bio.complete(BioStatus::IoError);
                    return Ok(());
                }
                TargetIoAction::Zero { .. } => {
                    complete_zero_bio(bio);
                    return Ok(());
                }
            }
        }

        let ranges = parts
            .iter()
            .map(|part| part.logical_range().clone())
            .collect::<Vec<_>>();
        // Each child covers exactly one already-mapped part. Completion is
        // aggregated back into the original `Bio` so stacked-device callers see
        // a single Linux-style result even when the table crosses targets.
        let (children, completion) = bio.split(ranges)?;
        for (mut child, part) in children.into_iter().zip(parts) {
            match part {
                TargetIoAction::Remap {
                    backing_start,
                    backing,
                    ..
                } => {
                    if child.remap_sid_start(backing_start).is_err() {
                        completion.complete_child(BioStatus::IoError);
                        continue;
                    }
                    if backing.enqueue(child).is_err() {
                        completion.complete_child(BioStatus::IoError);
                    }
                }
                TargetIoAction::Error { .. } => child.complete(BioStatus::IoError),
                TargetIoAction::Zero { .. } => complete_zero_bio(child),
            }
        }
        Ok(())
    }

    /// Splits a logical range at DM target boundaries before target-local remapping.
    fn bio_parts(
        &self,
        start: Sid,
        end: u64,
    ) -> Result<Vec<(Range<Sid>, &dyn DmTarget)>, BioEnqueueError> {
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

    /// Finds the target covering one sector; this is the scan left for later optimization.
    fn target_at(&self, sector: u64) -> Option<&dyn DmTarget> {
        self.targets
            .iter()
            .map(|target| target.as_ref())
            .find(|target| {
                sector >= target.logical_range().start.to_raw()
                    && sector < target.logical_range().end.to_raw()
            })
    }

    /// Fans out a flush to each unique backing device and aggregates completion status.
    fn enqueue_flush(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        let mut flushed = Vec::new();
        for target in &self.targets {
            let mut collect_id = |id| {
                if !flushed.contains(&id) {
                    flushed.push(id);
                }
            };
            target.for_each_backing_id(&mut collect_id);
        }

        if flushed.is_empty() {
            bio.complete(BioStatus::Complete);
            return Ok(());
        }

        let completion = Arc::new(FlushCompletion::new(flushed.len(), bio));
        let mut submitted = Vec::new();
        for target in &self.targets {
            let mut submit_flush = |backing: &dyn BlockDevice| {
                let backing_id = backing.id();
                if submitted.contains(&backing_id) {
                    return;
                }
                submitted.push(backing_id);

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
            };
            target.for_each_backing(&mut submit_flush);
        }
        Ok(())
    }
}

/// Borrowed target actions for one normal BIO before child submission begins.
///
/// The plan is intentionally scoped to `DmTable::enqueue`: each action borrows a
/// table-owned target and backing lease, so it cannot outlive the table generation
/// already retained by `DmDevice` until the original BIO completes.
struct NormalIoPlan<'a> {
    parts: Vec<TargetIoAction<'a>>,
}

/// Completes a BIO for the `zero` target without submitting backing I/O.
fn complete_zero_bio(bio: SubmittedBio) {
    // A `zero` target has no backing store. Reads synthesize zero-filled data;
    // writes, discards, write-zeroes, and flushes can complete successfully
    // because the target's visible contents are permanently zero.
    if bio.type_() == BioType::Read {
        for segment in bio.segments() {
            let mut writer = segment.dma_slice().writer().unwrap();
            let written = writer.fill_zeros(segment.nbytes());
            debug_assert_eq!(written, segment.nbytes());
        }
    }
    bio.complete(BioStatus::Complete);
}

/// Shared state for completing the original flush after all child flushes finish.
struct FlushCompletion {
    /// Number of backing flush BIOs that have not reported completion yet.
    remaining: AtomicUsize,
    /// First non-success status observed from any child flush BIO.
    status: AtomicU32,
    /// Original BIO held until the final child determines the aggregate status.
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
    use alloc::{format, string::String, vec};

    use aster_block::{
        BlockDeviceMeta,
        bio::{BioDirection, BioSegment},
    };
    use device_id::{MajorId, MinorId};
    use ostd::{mm::VmIo, prelude::ktest, sync::Mutex};

    use super::*;
    use crate::{
        DmManager,
        target::{
            error::ErrorTarget,
            striped::{StripedTarget, StripedTargetParams},
            zero::ZeroTarget,
        },
    };

    fn boxed<T: DmTarget + 'static>(target: T) -> DmTargetBox {
        Box::new(target)
    }

    #[derive(Debug)]
    struct RecordingBlockDevice {
        id: DeviceId,
        last_range: Mutex<Option<Range<Sid>>>,
        submitted_ranges: Mutex<Vec<Range<Sid>>>,
        submitted_types: Mutex<Vec<BioType>>,
        flush_count: Mutex<usize>,
        bio_status: BioStatus,
        fail_bio_enqueue: bool,
        flush_status: BioStatus,
        fail_flush_enqueue: bool,
        max_nr_segments_per_bio: usize,
    }

    impl RecordingBlockDevice {
        fn new(minor: u32) -> Arc<Self> {
            Self::new_with_options(
                minor,
                BioStatus::Complete,
                false,
                BioStatus::Complete,
                false,
                8,
            )
        }

        fn new_with_queue_limit(minor: u32, max_nr_segments_per_bio: usize) -> Arc<Self> {
            Self::new_with_options(
                minor,
                BioStatus::Complete,
                false,
                BioStatus::Complete,
                false,
                max_nr_segments_per_bio,
            )
        }

        fn new_failing_enqueue(minor: u32) -> Arc<Self> {
            Self::new_with_options(
                minor,
                BioStatus::Complete,
                true,
                BioStatus::Complete,
                false,
                8,
            )
        }

        fn new_with_bio_status(minor: u32, bio_status: BioStatus) -> Arc<Self> {
            Self::new_with_options(minor, bio_status, false, BioStatus::Complete, false, 8)
        }

        fn new_with_flush(
            minor: u32,
            flush_status: BioStatus,
            fail_flush_enqueue: bool,
        ) -> Arc<Self> {
            Self::new_with_options(
                minor,
                BioStatus::Complete,
                false,
                flush_status,
                fail_flush_enqueue,
                8,
            )
        }

        fn new_with_options(
            minor: u32,
            bio_status: BioStatus,
            fail_bio_enqueue: bool,
            flush_status: BioStatus,
            fail_flush_enqueue: bool,
            max_nr_segments_per_bio: usize,
        ) -> Arc<Self> {
            Arc::new(Self {
                id: DeviceId::new(MajorId::new(1), MinorId::new(minor)),
                last_range: Mutex::new(None),
                submitted_ranges: Mutex::new(Vec::new()),
                submitted_types: Mutex::new(Vec::new()),
                flush_count: Mutex::new(0),
                bio_status,
                fail_bio_enqueue,
                flush_status,
                fail_flush_enqueue,
                max_nr_segments_per_bio,
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
                self.submitted_types.lock().push(bio.type_());
                bio.complete(self.flush_status);
                return Ok(());
            }
            if self.fail_bio_enqueue {
                return Err(BioEnqueueError::Refused);
            }
            *self.last_range.lock() = Some(bio.sid_range().clone());
            self.submitted_ranges.lock().push(bio.sid_range().clone());
            self.submitted_types.lock().push(bio.type_());
            bio.complete(self.bio_status);
            Ok(())
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: self.max_nr_segments_per_bio,
                nr_sectors: 1_024,
            }
        }

        fn name(&self) -> &str {
            "dm-table-test"
        }

        fn id(&self) -> DeviceId {
            self.id
        }
    }

    #[derive(Debug)]
    struct DeferredRecordingBlockDevice {
        id: DeviceId,
        submitted_ranges: Mutex<Vec<Range<Sid>>>,
        submitted_types: Mutex<Vec<BioType>>,
        submitted_lengths: Mutex<Vec<usize>>,
        pending: Mutex<Vec<SubmittedBio>>,
    }

    impl DeferredRecordingBlockDevice {
        fn new(minor: u32) -> Arc<Self> {
            Arc::new(Self {
                id: DeviceId::new(MajorId::new(1), MinorId::new(minor)),
                submitted_ranges: Mutex::new(Vec::new()),
                submitted_types: Mutex::new(Vec::new()),
                submitted_lengths: Mutex::new(Vec::new()),
                pending: Mutex::new(Vec::new()),
            })
        }

        fn complete_pending(&self, index: usize) {
            self.pending
                .lock()
                .remove(index)
                .complete(BioStatus::Complete);
        }
    }

    impl BlockDevice for DeferredRecordingBlockDevice {
        fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            self.submitted_ranges.lock().push(bio.sid_range().clone());
            self.submitted_types.lock().push(bio.type_());
            self.submitted_lengths
                .lock()
                .push(bio.segments().iter().map(BioSegment::nbytes).sum());
            self.pending.lock().push(bio);
            Ok(())
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 8,
                nr_sectors: 1_024,
            }
        }

        fn name(&self) -> &str {
            "dm-table-deferred-test"
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
        let read = Bio::new(
            BioType::Read,
            Sid::new(8),
            vec![BioSegment::alloc(1, BioDirection::FromDevice)],
            None,
        );

        assert_eq!(
            read.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::Complete
        );
        assert_eq!(
            *backing.last_range.lock(),
            Some(Sid::new(108)..Sid::new(116))
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
    fn splits_discard_across_linear_target_boundary() {
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
        let discard = Bio::new_range(BioType::Discard, Sid::new(0), 8, None);

        assert_eq!(
            discard.submit_and_wait(&TableDevice(table)).unwrap(),
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
        assert_eq!(*first.submitted_types.lock(), vec![BioType::Discard]);
        assert_eq!(*second.submitted_types.lock(), vec![BioType::Discard]);
    }

    #[ktest]
    fn reports_io_error_when_split_child_enqueue_fails() {
        let first = RecordingBlockDevice::new(1);
        let second = RecordingBlockDevice::new_failing_enqueue(2);
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
            BioStatus::IoError
        );
        assert_eq!(
            *first.submitted_ranges.lock(),
            vec![Sid::new(100)..Sid::new(104)]
        );
        assert!(second.submitted_ranges.lock().is_empty());
    }

    #[ktest]
    fn waits_for_later_children_after_nonfinal_enqueue_failure() {
        let refused = RecordingBlockDevice::new_failing_enqueue(1);
        let second = DeferredRecordingBlockDevice::new(2);
        let third = DeferredRecordingBlockDevice::new(3);
        let table = DmTable::new_linear(vec![
            LinearTarget::new(
                Sid::new(0),
                4,
                Sid::new(100),
                BlockDeviceLease::new_untracked(refused.clone() as Arc<dyn BlockDevice>),
            )
            .unwrap(),
            LinearTarget::new(
                Sid::new(4),
                4,
                Sid::new(200),
                BlockDeviceLease::new_untracked(second.clone() as Arc<dyn BlockDevice>),
            )
            .unwrap(),
            LinearTarget::new(
                Sid::new(8),
                4,
                Sid::new(300),
                BlockDeviceLease::new_untracked(third.clone() as Arc<dyn BlockDevice>),
            )
            .unwrap(),
        ])
        .unwrap();
        let completions = Arc::new(Mutex::new(Vec::new()));
        let callback_completions = completions.clone();
        let read = Bio::new(
            BioType::Read,
            Sid::new(0),
            vec![BioSegment::alloc_exact(
                2,
                12 * 512,
                BioDirection::FromDevice,
            )],
            Some(Box::new(move |status| {
                callback_completions.lock().push(status);
            })),
        );

        table.enqueue(read.submit_for_test()).unwrap();

        assert!(refused.submitted_ranges.lock().is_empty());
        assert_eq!(
            *second.submitted_ranges.lock(),
            vec![Sid::new(200)..Sid::new(204)]
        );
        assert_eq!(
            *third.submitted_ranges.lock(),
            vec![Sid::new(300)..Sid::new(304)]
        );
        assert!(completions.lock().is_empty());

        second.complete_pending(0);
        assert!(completions.lock().is_empty());
        third.complete_pending(0);
        assert_eq!(*completions.lock(), vec![BioStatus::IoError]);
    }

    #[ktest]
    fn reports_io_error_when_split_child_completes_with_error() {
        let first = RecordingBlockDevice::new(1);
        let second = RecordingBlockDevice::new_with_bio_status(2, BioStatus::IoError);
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
            BioStatus::IoError
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
        let first_target = &table.targets()[0];
        assert_eq!(first_target.name(), "linear");
        assert_eq!(first_target.logical_range(), &(Sid::new(0)..Sid::new(128)));
        let mut first_backing_ids = Vec::new();
        let mut collect_first_backing_id = |id| first_backing_ids.push(id);
        first_target.for_each_backing_id(&mut collect_first_backing_id);
        assert_eq!(first_backing_ids, vec![first.id()]);
        let mut first_backings = Vec::new();
        let mut collect_first_backing =
            |backing: &dyn BlockDevice| first_backings.push(backing.id());
        first_target.for_each_backing(&mut collect_first_backing);
        assert_eq!(first_backings, vec![first.id()]);
        let second_target = &table.targets()[1];
        assert_eq!(second_target.name(), "linear");
        assert_eq!(
            second_target.logical_range(),
            &(Sid::new(128)..Sid::new(192))
        );
        let mut second_backing_ids = Vec::new();
        let mut collect_second_backing_id = |id| second_backing_ids.push(id);
        second_target.for_each_backing_id(&mut collect_second_backing_id);
        assert_eq!(second_backing_ids, vec![second.id()]);
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
        let first = RecordingBlockDevice::new_with_queue_limit(1, 8);
        let second = RecordingBlockDevice::new_with_queue_limit(2, 4);
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
        assert_eq!(table.metadata().max_nr_segments_per_bio, 4);
    }

    #[ktest]
    fn rejects_dm_device_backing_for_linear_and_striped_targets() {
        let manager = DmManager::new().unwrap();
        let backing = manager
            .create(String::from("dm-table-backing-test"), None, None)
            .unwrap();
        let inner = RecordingBlockDevice::new(9);
        let inner_table = Arc::new(
            DmTable::new_single_linear(
                Sid::new(0),
                128,
                Sid::new(0),
                BlockDeviceLease::new_untracked(inner as Arc<dyn BlockDevice>),
            )
            .unwrap(),
        );
        backing.load_table(inner_table);
        backing.resume().unwrap();
        assert_eq!(backing.metadata().nr_sectors, 128);

        let linear = LinearTarget::new(
            Sid::new(0),
            8,
            Sid::new(0),
            BlockDeviceLease::new_untracked(backing.clone() as Arc<dyn BlockDevice>),
        )
        .unwrap();
        assert!(matches!(
            DmTable::new_linear(vec![linear]),
            Err(TableError::UnsupportedBackingDevice)
        ));

        let params = StripedTargetParams::parse(&format!(
            "1 4 {}:{} 0",
            backing.id().major().get(),
            backing.id().minor().get()
        ))
        .unwrap();
        let striped = StripedTarget::new(
            Sid::new(0),
            8,
            params,
            vec![BlockDeviceLease::new_untracked(
                backing as Arc<dyn BlockDevice>,
            )],
        )
        .unwrap();
        assert!(matches!(
            DmTable::new_targets(vec![boxed(striped)]),
            Err(TableError::UnsupportedBackingDevice)
        ));
    }

    #[ktest]
    fn stores_striped_targets_and_reports_all_backings() {
        let first = RecordingBlockDevice::new_with_queue_limit(1, 8);
        let second = RecordingBlockDevice::new_with_queue_limit(2, 4);
        let params = StripedTargetParams::parse("2 4 1:1 0 1:2 0").unwrap();
        let striped = StripedTarget::new(
            Sid::new(0),
            16,
            params,
            vec![
                BlockDeviceLease::new_untracked(first.clone() as Arc<dyn BlockDevice>),
                BlockDeviceLease::new_untracked(second.clone() as Arc<dyn BlockDevice>),
            ],
        )
        .unwrap();
        let table = DmTable::new_targets(vec![boxed(striped)]).unwrap();

        assert_eq!(table.length(), 16);
        assert_eq!(table.metadata().nr_sectors, 16);
        assert_eq!(table.metadata().max_nr_segments_per_bio, 4);
        assert_eq!(table.backing_ids(), vec![first.id(), second.id()]);
        let target = &table.targets()[0];
        assert_eq!(target.name(), "striped");
        assert_eq!(target.logical_range(), &(Sid::new(0)..Sid::new(16)));
        assert_eq!(target.length(), 16);
    }

    #[ktest]
    fn accepts_mixed_linear_and_striped_targets() {
        let linear_backing = RecordingBlockDevice::new(1);
        let striped_first = RecordingBlockDevice::new(2);
        let striped_second = RecordingBlockDevice::new(3);
        let striped = StripedTarget::new(
            Sid::new(4),
            16,
            StripedTargetParams::parse("2 4 1:2 0 1:3 0").unwrap(),
            vec![
                BlockDeviceLease::new_untracked(striped_first.clone() as Arc<dyn BlockDevice>),
                BlockDeviceLease::new_untracked(striped_second.clone() as Arc<dyn BlockDevice>),
            ],
        )
        .unwrap();
        let table = DmTable::new_targets(vec![
            boxed(
                LinearTarget::new(
                    Sid::new(0),
                    4,
                    Sid::new(100),
                    BlockDeviceLease::new_untracked(linear_backing.clone() as Arc<dyn BlockDevice>),
                )
                .unwrap(),
            ),
            boxed(striped),
        ])
        .unwrap();

        assert_eq!(table.length(), 20);
        assert_eq!(
            table.backing_ids(),
            vec![linear_backing.id(), striped_first.id(), striped_second.id()]
        );
    }

    #[ktest]
    fn error_target_reports_capacity_without_backing_limits() {
        let table =
            DmTable::new_targets(vec![boxed(ErrorTarget::new(Sid::new(0), 8).unwrap())]).unwrap();

        assert_eq!(table.length(), 8);
        assert_eq!(table.metadata().nr_sectors, 8);
        assert_eq!(table.metadata().max_nr_segments_per_bio, usize::MAX);
        assert!(table.backing_ids().is_empty());
    }

    #[ktest]
    fn error_target_completes_read_and_write_with_io_error() {
        let table = Arc::new(
            DmTable::new_targets(vec![boxed(ErrorTarget::new(Sid::new(0), 8).unwrap())]).unwrap(),
        );
        let read = Bio::new(
            BioType::Read,
            Sid::new(0),
            vec![BioSegment::alloc(1, BioDirection::FromDevice)],
            None,
        );
        let write = Bio::new(
            BioType::Write,
            Sid::new(0),
            vec![BioSegment::alloc(1, BioDirection::ToDevice)],
            None,
        );

        assert_eq!(
            read.submit_and_wait(&TableDevice(table.clone())).unwrap(),
            BioStatus::IoError
        );
        assert_eq!(
            write.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::IoError
        );
    }

    #[ktest]
    fn error_target_completes_flush_successfully() {
        let table = Arc::new(
            DmTable::new_targets(vec![boxed(ErrorTarget::new(Sid::new(0), 8).unwrap())]).unwrap(),
        );
        let flush = Bio::new(BioType::Flush, Sid::new(0), vec![], None);

        assert_eq!(
            flush.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::Complete
        );
    }

    #[ktest]
    fn zero_target_reports_capacity_without_backing_limits() {
        let table =
            DmTable::new_targets(vec![boxed(ZeroTarget::new(Sid::new(0), 8).unwrap())]).unwrap();

        assert_eq!(table.length(), 8);
        assert_eq!(table.metadata().nr_sectors, 8);
        assert_eq!(table.metadata().max_nr_segments_per_bio, usize::MAX);
        assert!(table.backing_ids().is_empty());
    }

    #[ktest]
    fn zero_target_completes_read_with_zeroes_and_write_successfully() {
        let table = Arc::new(
            DmTable::new_targets(vec![boxed(ZeroTarget::new(Sid::new(0), 8).unwrap())]).unwrap(),
        );
        let read_segment = BioSegment::alloc(1, BioDirection::FromDevice);
        let read = Bio::new(BioType::Read, Sid::new(0), vec![read_segment.clone()], None);
        let write = Bio::new(
            BioType::Write,
            Sid::new(0),
            vec![BioSegment::alloc(1, BioDirection::ToDevice)],
            None,
        );

        assert_eq!(
            read.submit_and_wait(&TableDevice(table.clone())).unwrap(),
            BioStatus::Complete
        );
        let mut buffer = [0xffu8; 512];
        read_segment.dma_slice().read_bytes(0, &mut buffer).unwrap();
        assert!(buffer.iter().all(|byte| *byte == 0));
        assert_eq!(
            write.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::Complete
        );
    }

    #[ktest]
    fn zero_target_completes_flush_successfully() {
        let table = Arc::new(
            DmTable::new_targets(vec![boxed(ZeroTarget::new(Sid::new(0), 8).unwrap())]).unwrap(),
        );
        let flush = Bio::new(BioType::Flush, Sid::new(0), vec![], None);

        assert_eq!(
            flush.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::Complete
        );
    }

    #[ktest]
    fn zero_target_completes_discard_and_write_zeroes_successfully() {
        let table = Arc::new(
            DmTable::new_targets(vec![boxed(ZeroTarget::new(Sid::new(0), 8).unwrap())]).unwrap(),
        );
        let discard = Bio::new_range(BioType::Discard, Sid::new(0), 8, None);
        let write_zeroes = Bio::new_range(BioType::WriteZeroes, Sid::new(0), 8, None);

        assert_eq!(
            discard
                .submit_and_wait(&TableDevice(table.clone()))
                .unwrap(),
            BioStatus::Complete
        );
        assert_eq!(
            write_zeroes.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::Complete
        );
    }

    #[ktest]
    fn normal_io_plan_splits_at_target_boundaries_before_execution() {
        let backing = RecordingBlockDevice::new(1);
        let table = DmTable::new_targets(vec![
            boxed(
                LinearTarget::new(
                    Sid::new(0),
                    4,
                    Sid::new(100),
                    BlockDeviceLease::new_untracked(backing as Arc<dyn BlockDevice>),
                )
                .unwrap(),
            ),
            boxed(ErrorTarget::new(Sid::new(4), 4).unwrap()),
        ])
        .unwrap();
        let bio = Bio::new(
            BioType::Write,
            Sid::new(0),
            vec![BioSegment::alloc(1, BioDirection::ToDevice)],
            None,
        )
        .submit_for_test();

        let plan = table.plan_normal_io(&bio).unwrap();

        assert_eq!(plan.parts.len(), 2);
        assert_eq!(plan.parts[0].logical_range(), &(Sid::new(0)..Sid::new(4)));
        assert_eq!(plan.parts[1].logical_range(), &(Sid::new(4)..Sid::new(8)));
        assert!(matches!(&plan.parts[0], TargetIoAction::Remap { .. }));
        assert!(matches!(&plan.parts[1], TargetIoAction::Error { .. }));
    }

    #[ktest]
    fn split_bio_across_linear_and_error_targets_reports_io_error() {
        let backing = RecordingBlockDevice::new(1);
        let table = Arc::new(
            DmTable::new_targets(vec![
                boxed(
                    LinearTarget::new(
                        Sid::new(0),
                        4,
                        Sid::new(100),
                        BlockDeviceLease::new_untracked(backing.clone() as Arc<dyn BlockDevice>),
                    )
                    .unwrap(),
                ),
                boxed(ErrorTarget::new(Sid::new(4), 4).unwrap()),
            ])
            .unwrap(),
        );
        let write = Bio::new(
            BioType::Write,
            Sid::new(0),
            vec![BioSegment::alloc(1, BioDirection::ToDevice)],
            None,
        );

        assert_eq!(
            write.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::IoError
        );
        assert_eq!(
            *backing.submitted_ranges.lock(),
            vec![Sid::new(100)..Sid::new(104)]
        );
    }

    #[ktest]
    fn write_zeroes_across_linear_and_error_targets_reports_io_error() {
        let backing = RecordingBlockDevice::new(1);
        let table = Arc::new(
            DmTable::new_targets(vec![
                boxed(
                    LinearTarget::new(
                        Sid::new(0),
                        4,
                        Sid::new(100),
                        BlockDeviceLease::new_untracked(backing.clone() as Arc<dyn BlockDevice>),
                    )
                    .unwrap(),
                ),
                boxed(ErrorTarget::new(Sid::new(4), 4).unwrap()),
            ])
            .unwrap(),
        );
        let write_zeroes = Bio::new_range(BioType::WriteZeroes, Sid::new(0), 8, None);

        assert_eq!(
            write_zeroes.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::IoError
        );
        assert_eq!(
            *backing.submitted_ranges.lock(),
            vec![Sid::new(100)..Sid::new(104)]
        );
        assert_eq!(*backing.submitted_types.lock(), vec![BioType::WriteZeroes]);
    }

    fn striped_dm_target<T: BlockDevice + 'static>(
        logical_start: u64,
        length: u64,
        params: &str,
        backings: &[Arc<T>],
    ) -> DmTargetBox {
        boxed(
            StripedTarget::new(
                Sid::new(logical_start),
                length,
                StripedTargetParams::parse(params).unwrap(),
                backings
                    .iter()
                    .map(|backing| {
                        BlockDeviceLease::new_untracked(backing.clone() as Arc<dyn BlockDevice>)
                    })
                    .collect(),
            )
            .unwrap(),
        )
    }

    #[ktest]
    fn maps_bio_within_single_striped_chunk() {
        let first = RecordingBlockDevice::new(1);
        let second = RecordingBlockDevice::new(2);
        let table = Arc::new(
            DmTable::new_targets(vec![striped_dm_target(
                0,
                32,
                "2 16 1:1 100 1:2 200",
                &[first.clone(), second.clone()],
            )])
            .unwrap(),
        );
        let read = Bio::new(
            BioType::Read,
            Sid::new(1),
            vec![BioSegment::alloc(1, BioDirection::FromDevice)],
            None,
        );

        assert_eq!(
            read.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::Complete
        );
        assert_eq!(
            *first.submitted_ranges.lock(),
            vec![Sid::new(101)..Sid::new(109)]
        );
        assert!(second.submitted_ranges.lock().is_empty());
    }

    #[ktest]
    fn splits_bio_across_striped_chunk_boundaries() {
        let first = RecordingBlockDevice::new(1);
        let second = RecordingBlockDevice::new(2);
        let table = Arc::new(
            DmTable::new_targets(vec![striped_dm_target(
                0,
                16,
                "2 4 1:1 100 1:2 200",
                &[first.clone(), second.clone()],
            )])
            .unwrap(),
        );
        let read = Bio::new(
            BioType::Read,
            Sid::new(2),
            vec![BioSegment::alloc(1, BioDirection::FromDevice)],
            None,
        );

        assert_eq!(
            read.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::Complete
        );
        assert_eq!(
            *first.submitted_ranges.lock(),
            vec![Sid::new(102)..Sid::new(104), Sid::new(104)..Sid::new(106)]
        );
        assert_eq!(
            *second.submitted_ranges.lock(),
            vec![Sid::new(200)..Sid::new(204)]
        );
    }

    #[ktest]
    fn splits_twelve_sector_write_across_four_striped_children() {
        let first = DeferredRecordingBlockDevice::new(1);
        let second = DeferredRecordingBlockDevice::new(2);
        let table = DmTable::new_targets(vec![striped_dm_target(
            0,
            24,
            "2 4 1:1 0 1:2 0",
            &[first.clone(), second.clone()],
        )])
        .unwrap();
        let completion_status = Arc::new(Mutex::new(None));
        let callback_status = completion_status.clone();
        let write = Bio::new(
            BioType::Write,
            Sid::new(2),
            vec![BioSegment::alloc_exact(2, 12 * 512, BioDirection::ToDevice)],
            Some(Box::new(move |status| {
                *callback_status.lock() = Some(status)
            })),
        );

        table.enqueue(write.submit_for_test()).unwrap();

        assert_eq!(
            *first.submitted_ranges.lock(),
            vec![Sid::new(2)..Sid::new(4), Sid::new(4)..Sid::new(8)]
        );
        assert_eq!(
            *second.submitted_ranges.lock(),
            vec![Sid::new(0)..Sid::new(4), Sid::new(4)..Sid::new(6)]
        );
        assert_eq!(
            *first.submitted_types.lock(),
            vec![BioType::Write, BioType::Write]
        );
        assert_eq!(
            *second.submitted_types.lock(),
            vec![BioType::Write, BioType::Write]
        );
        assert_eq!(*first.submitted_lengths.lock(), vec![2 * 512, 4 * 512]);
        assert_eq!(*second.submitted_lengths.lock(), vec![4 * 512, 2 * 512]);
        assert_eq!(*completion_status.lock(), None);

        first.complete_pending(1);
        second.complete_pending(0);
        first.complete_pending(0);
        assert_eq!(*completion_status.lock(), None);

        second.complete_pending(0);
        assert_eq!(*completion_status.lock(), Some(BioStatus::Complete));
    }

    #[ktest]
    fn splits_write_zeroes_across_striped_chunk_boundaries() {
        let first = RecordingBlockDevice::new(1);
        let second = RecordingBlockDevice::new(2);
        let table = Arc::new(
            DmTable::new_targets(vec![striped_dm_target(
                0,
                16,
                "2 4 1:1 100 1:2 200",
                &[first.clone(), second.clone()],
            )])
            .unwrap(),
        );
        let write_zeroes = Bio::new_range(BioType::WriteZeroes, Sid::new(2), 8, None);

        assert_eq!(
            write_zeroes.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::Complete
        );
        assert_eq!(
            *first.submitted_ranges.lock(),
            vec![Sid::new(102)..Sid::new(104), Sid::new(104)..Sid::new(106)]
        );
        assert_eq!(
            *second.submitted_ranges.lock(),
            vec![Sid::new(200)..Sid::new(204)]
        );
        assert_eq!(
            *first.submitted_types.lock(),
            vec![BioType::WriteZeroes, BioType::WriteZeroes]
        );
        assert_eq!(*second.submitted_types.lock(), vec![BioType::WriteZeroes]);
    }

    #[ktest]
    fn splits_striped_bio_ending_at_target_end() {
        let first = RecordingBlockDevice::new(1);
        let second = RecordingBlockDevice::new(2);
        let table = Arc::new(
            DmTable::new_targets(vec![striped_dm_target(
                0,
                16,
                "2 4 1:1 100 1:2 200",
                &[first.clone(), second.clone()],
            )])
            .unwrap(),
        );
        let read = Bio::new(
            BioType::Read,
            Sid::new(8),
            vec![BioSegment::alloc(1, BioDirection::FromDevice)],
            None,
        );

        assert_eq!(
            read.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::Complete
        );
        assert_eq!(
            *first.submitted_ranges.lock(),
            vec![Sid::new(104)..Sid::new(108)]
        );
        assert_eq!(
            *second.submitted_ranges.lock(),
            vec![Sid::new(204)..Sid::new(208)]
        );
    }

    #[ktest]
    fn splits_bio_across_linear_and_striped_targets() {
        let linear_backing = RecordingBlockDevice::new(1);
        let striped_first = RecordingBlockDevice::new(2);
        let striped_second = RecordingBlockDevice::new(3);
        let table = Arc::new(
            DmTable::new_targets(vec![
                boxed(
                    LinearTarget::new(
                        Sid::new(0),
                        4,
                        Sid::new(100),
                        BlockDeviceLease::new_untracked(
                            linear_backing.clone() as Arc<dyn BlockDevice>
                        ),
                    )
                    .unwrap(),
                ),
                striped_dm_target(
                    4,
                    16,
                    "2 4 1:2 0 1:3 0",
                    &[striped_first.clone(), striped_second.clone()],
                ),
            ])
            .unwrap(),
        );
        let read = Bio::new(
            BioType::Read,
            Sid::new(2),
            vec![BioSegment::alloc(1, BioDirection::FromDevice)],
            None,
        );

        assert_eq!(
            read.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::Complete
        );
        assert_eq!(
            *linear_backing.submitted_ranges.lock(),
            vec![Sid::new(102)..Sid::new(104)]
        );
        assert_eq!(
            *striped_first.submitted_ranges.lock(),
            vec![Sid::new(0)..Sid::new(4)]
        );
        assert_eq!(
            *striped_second.submitted_ranges.lock(),
            vec![Sid::new(0)..Sid::new(2)]
        );
    }

    #[ktest]
    fn flushes_each_striped_backing_once() {
        let first = RecordingBlockDevice::new(1);
        let second = RecordingBlockDevice::new(2);
        let table = Arc::new(
            DmTable::new_targets(vec![striped_dm_target(
                0,
                16,
                "2 4 1:1 0 1:2 0",
                &[first.clone(), second.clone()],
            )])
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
    fn deduplicates_flush_backings_across_linear_and_striped_targets() {
        let shared = RecordingBlockDevice::new(1);
        let second = RecordingBlockDevice::new(2);
        let table = Arc::new(
            DmTable::new_targets(vec![
                boxed(
                    LinearTarget::new(
                        Sid::new(0),
                        4,
                        Sid::new(100),
                        BlockDeviceLease::new_untracked(shared.clone() as Arc<dyn BlockDevice>),
                    )
                    .unwrap(),
                ),
                striped_dm_target(4, 16, "2 4 1:1 0 1:2 0", &[shared.clone(), second.clone()]),
            ])
            .unwrap(),
        );
        let flush = Bio::new(BioType::Flush, Sid::new(0), vec![], None);

        assert_eq!(
            flush.submit_and_wait(&TableDevice(table)).unwrap(),
            BioStatus::Complete
        );
        assert_eq!(*shared.flush_count.lock(), 1);
        assert_eq!(*second.flush_count.lock(), 1);
    }

    #[ktest]
    fn refuses_bio_outside_striped_table_range() {
        let backing = RecordingBlockDevice::new(1);
        let table = Arc::new(
            DmTable::new_targets(vec![striped_dm_target(
                0,
                4,
                "1 4 1:1 0",
                &[backing.clone()],
            )])
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
        assert!(backing.submitted_ranges.lock().is_empty());
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

        fn name(&self) -> &str {
            "dm-table-wrapper"
        }

        fn id(&self) -> DeviceId {
            DeviceId::new(MajorId::new(2), MinorId::new(1))
        }
    }
}
