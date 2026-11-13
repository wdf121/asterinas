// SPDX-License-Identifier: MPL-2.0

//! Executes validated table mappings using the native block request contract.
//!
//! This module owns remapping, split completion, flush aggregation, and optional
//! target completion processing. Table geometry and target mapping stay outside it.
//!
//! Execution can allocate after earlier children have been dispatched; these
//! allocations follow the kernel's global allocation-failure policy, not the
//! recoverable enqueue-error path. For recoverable failures, each split child
//! discharges one completion responsibility and flush counts each rejected
//! destination once. Already dispatched operations must finish before their parent
//! completes. Immediate rejection does not notify target completion observers.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use aster_block::{
    BlockDevice,
    bio::{Bio, BioEnqueueError, BioStatus, BioType, SubmittedBio},
    id::Sid,
};
use ostd::sync::SpinLock;

use crate::{
    DmTable,
    reclaim::CompletionTable,
    table::{PlannedFlush, PlannedIo},
};

pub(super) fn execute_normal_io(
    table: &Arc<DmTable>,
    mut bio: SubmittedBio,
    parts: Vec<PlannedIo<'_>>,
) -> Result<(), BioEnqueueError> {
    if parts.len() == 1 {
        let part = parts.into_iter().next().unwrap();
        bio.remap_sid_start(part.mapping.backing_start)?;
        return enqueue_part(table, bio, part);
    }

    let ranges = parts
        .iter()
        .map(|part| part.mapping.logical_range.clone())
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
        if child.remap_sid_start(part.mapping.backing_start).is_ok() {
            // Rejected split children still discharge their parent completion duty.
            let _ = enqueue_part(table, child, part);
        }
    }
    Ok(())
}

fn enqueue_part(
    table: &Arc<DmTable>,
    bio: SubmittedBio,
    part: PlannedIo<'_>,
) -> Result<(), BioEnqueueError> {
    if table.targets()[part.target_index].io_completion().is_none() {
        return part.mapping.backing.enqueue(bio);
    }

    // Completion capability is stable for the target's lifetime. Retaining the
    // table keeps both the observer and its backing references alive until callback.
    let table = CompletionTable::new(table);
    let backing = part.mapping.backing.id();
    enqueue_with_completion(bio, part.mapping.backing, move |status| {
        table.targets()[part.target_index]
            .io_completion()
            .unwrap()
            .complete_io(backing, status);
    })
}

/// Sends a flush once to every planned backing and aggregates all completions.
pub(super) fn execute_flush(
    table: &Arc<DmTable>,
    bio: SubmittedBio,
    plan: Vec<PlannedFlush<'_>>,
) -> Result<(), BioEnqueueError> {
    // Count and dispatch the same immutable plan, including synchronous completions.
    if plan.is_empty() {
        bio.complete(BioStatus::Complete);
        return Ok(());
    }

    let completion = Arc::new(FlushCompletion::new(plan.len(), bio));
    for part in plan {
        let callback_completion = completion.clone();
        let table = CompletionTable::new(table);
        let backing_id = part.backing.id();
        let flush = Bio::new(
            BioType::Flush,
            Sid::new(0),
            Vec::new(),
            Some(Box::new(move |status| {
                for index in part.target_indices {
                    if let Some(processor) = table.targets()[index].io_completion() {
                        processor.complete_io(backing_id, status);
                    }
                }
                callback_completion.complete_one(status);
            })),
        );
        let mut io_batch = io_util::batch::IoBatch::with_capacity(1);
        if flush.submit(part.backing, &mut io_batch).is_err() {
            // Admission failure completes the logical request, not target health.
            completion.complete_one(BioStatus::IoError);
        }
    }
    Ok(())
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

/// Forwards a request and processes terminal results before completing its caller.
///
/// An admission rejection drops the original request without calling `complete_fn`.
/// Its existing ownership contract determines whether that drop completes a split
/// or replayed request, or leaves an immediate submission to return the error.
pub(super) fn enqueue_with_completion(
    bio: SubmittedBio,
    backing: &dyn BlockDevice,
    complete_fn: impl FnOnce(BioStatus) + Send + 'static,
) -> Result<(), BioEnqueueError> {
    // Segment clones share DMA storage; no payload is copied. Unlike linear's
    // direct path, completion-aware targets need a separate backing BIO here.
    let type_ = bio.type_();
    let start = bio.sid_range().start;
    let segments = bio.segments().to_vec();
    // Both synchronous completion and immediate rejection must be able to take
    // the request. Only a successful terminal callback invokes target processing.
    let original = Arc::new(SpinLock::new(Some(bio)));
    let completion = original.clone();
    let forwarded = Bio::new(
        type_,
        start,
        segments,
        Some(Box::new(move |status| {
            complete_fn(status);
            let original = completion.disable_irq().lock().take().unwrap();
            original.complete(status);
        })),
    );
    let mut batch = io_util::batch::IoBatch::with_capacity(1);
    let result = forwarded.submit(backing, &mut batch);
    if result.is_err() {
        // Never complete or drop a request while holding the ownership lock:
        // its callback can reenter DM or complete a parent request.
        let rejected = original.disable_irq().lock().take();
        drop(rejected);
    }
    result
}

#[cfg(ktest)]
mod tests {
    use alloc::vec;
    use core::ops::Range;

    use aster_block::{
        BlockDeviceMeta,
        bio::{BioDirection, BioSegment},
    };
    use device_id::{DeviceId, MajorId, MinorId};
    use ostd::{prelude::ktest, sync::Mutex};

    use super::*;
    use crate::target::{
        DmTarget, DmTargetMetadata, TargetIoAction, TargetIoCompletion, TargetStatusMode,
    };

    #[derive(Debug)]
    struct FlushDevice {
        id: DeviceId,
        result: Option<Result<BioStatus, BioEnqueueError>>,
        submissions: AtomicUsize,
        pending: Mutex<Vec<SubmittedBio>>,
    }

    impl BlockDevice for FlushDevice {
        fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            assert!(matches!(bio.type_(), BioType::Flush | BioType::Read));
            self.submissions.fetch_add(1, Ordering::Relaxed);
            if let Some(result) = self.result {
                bio.complete(result?);
            } else {
                self.pending.lock().push(bio);
            }
            Ok(())
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                nr_sectors: 16,
                max_nr_segments_per_bio: 8,
            }
        }

        fn name(&self) -> &str {
            "dm-flush-plan-test"
        }
        fn id(&self) -> DeviceId {
            self.id
        }
    }

    #[derive(Debug)]
    struct FlushTarget {
        range: Range<Sid>,
        backings: Vec<Arc<FlushDevice>>,
        results: Arc<Mutex<Vec<(DeviceId, BioStatus)>>>,
    }

    impl DmTarget for FlushTarget {
        fn as_any(&self) -> &dyn core::any::Any {
            self
        }
        fn metadata(&self) -> DmTargetMetadata {
            DmTargetMetadata::new("flush-test", [1, 0, 0])
        }
        fn logical_range(&self) -> &Range<Sid> {
            &self.range
        }
        fn for_each_backing<'a>(&'a self, f: &mut dyn FnMut(&'a dyn BlockDevice)) {
            for backing in &self.backings {
                f(backing.as_ref());
            }
        }
        fn map_io_range(&self, logical: Range<Sid>) -> Option<Vec<TargetIoAction<'_>>> {
            let backing_start = logical.start;
            Some(vec![TargetIoAction {
                logical_range: logical,
                backing_start,
                backing: self.backings.first()?.as_ref(),
            }])
        }
        fn io_completion(&self) -> Option<&dyn TargetIoCompletion> {
            Some(self)
        }
        fn status_params(
            &self,
            _: TargetStatusMode,
        ) -> Result<alloc::string::String, crate::TableError> {
            Ok(alloc::string::String::new())
        }
    }

    impl TargetIoCompletion for FlushTarget {
        fn complete_io(&self, backing: DeviceId, status: BioStatus) {
            // Deliberately do not filter: execution must deliver only owned results.
            self.results.lock().push((backing, status));
        }
    }

    fn flush_device(
        minor: u32,
        result: Option<Result<BioStatus, BioEnqueueError>>,
    ) -> Arc<FlushDevice> {
        Arc::new(FlushDevice {
            id: DeviceId::new(MajorId::new(1), MinorId::new(minor)),
            result,
            submissions: AtomicUsize::new(0),
            pending: Mutex::new(Vec::new()),
        })
    }

    #[ktest]
    fn flush_notifies_only_dependent_targets_once_before_parent_completion() {
        let shared = flush_device(86, None);
        let other = flush_device(87, None);
        let results: Vec<_> = (0..3).map(|_| Arc::new(Mutex::new(Vec::new()))).collect();
        let targets = vec![
            vec![shared.clone(), shared.clone()],
            vec![shared.clone()],
            vec![other.clone()],
        ];
        let table = Arc::new(
            DmTable::new_targets(
                targets
                    .into_iter()
                    .enumerate()
                    .map(|(index, backings)| {
                        Box::new(FlushTarget {
                            range: Sid::new(index as u64)..Sid::new(index as u64 + 1),
                            backings,
                            results: results[index].clone(),
                        }) as crate::target::DmTargetBox
                    })
                    .collect(),
            )
            .unwrap(),
        );
        let notified = Arc::new(AtomicUsize::new(0));
        let notification = notified.clone();
        let observed = results.clone();
        let bio = Bio::new(
            BioType::Flush,
            Sid::new(0),
            Vec::new(),
            Some(Box::new(move |status| {
                assert_eq!(status, BioStatus::IoError);
                for result in &observed {
                    assert_eq!(result.lock().len(), 1);
                }
                notification.fetch_add(1, Ordering::Relaxed);
            })),
        );
        table.enqueue(bio.submit_for_test()).unwrap();
        assert_eq!(shared.submissions.load(Ordering::Relaxed), 1);
        assert_eq!(other.submissions.load(Ordering::Relaxed), 1);
        let child = shared.pending.lock().pop().unwrap();
        child.complete(BioStatus::IoError);
        for result in &results[..2] {
            assert_eq!(*result.lock(), vec![(shared.id(), BioStatus::IoError)]);
        }
        assert!(results[2].lock().is_empty());
        assert_eq!(notified.load(Ordering::Relaxed), 0);
        let child = other.pending.lock().pop().unwrap();
        child.complete(BioStatus::Complete);
        assert_eq!(*results[2].lock(), vec![(other.id(), BioStatus::Complete)]);
        assert_eq!(notified.load(Ordering::Relaxed), 1);
    }

    #[ktest]
    fn flush_aggregates_synchronous_completion_and_rejection_without_false_notifications() {
        for outcome in [
            Ok(BioStatus::Complete),
            Ok(BioStatus::IoError),
            Err(BioEnqueueError::IsFull),
        ] {
            let immediate = flush_device(88, Some(outcome));
            let deferred = flush_device(89, None);
            let results = Arc::new(Mutex::new(Vec::new()));
            let table = Arc::new(
                DmTable::new_targets(vec![Box::new(FlushTarget {
                    range: Sid::new(0)..Sid::new(1),
                    backings: vec![immediate.clone(), deferred.clone()],
                    results: results.clone(),
                })])
                .unwrap(),
            );
            let notified = Arc::new(AtomicUsize::new(0));
            let notification = notified.clone();
            let observed = results.clone();
            let bio = Bio::new(
                BioType::Flush,
                Sid::new(0),
                Vec::new(),
                Some(Box::new(move |status| {
                    assert_eq!(status, outcome.unwrap_or(BioStatus::IoError));
                    assert_eq!(observed.lock().len(), if outcome.is_ok() { 2 } else { 1 });
                    notification.fetch_add(1, Ordering::Relaxed);
                })),
            );
            table.enqueue(bio.submit_for_test()).unwrap();
            assert_eq!(notified.load(Ordering::Relaxed), 0);
            assert_eq!(results.lock().len(), usize::from(outcome.is_ok()));
            assert_eq!(immediate.submissions.load(Ordering::Relaxed), 1);
            let child = deferred.pending.lock().pop().unwrap();
            child.complete(BioStatus::Complete);
            assert_eq!(notified.load(Ordering::Relaxed), 1);
        }
    }

    #[ktest]
    fn normal_io_retains_target_and_backing_until_terminal_notification() {
        for status in [
            BioStatus::Complete,
            BioStatus::IoError,
            BioStatus::NotSupported,
        ] {
            let backing = flush_device(90, None);
            let weak_backing = Arc::downgrade(&backing);
            let results = Arc::new(Mutex::new(Vec::new()));
            let table = Arc::new(
                DmTable::new_targets(vec![Box::new(FlushTarget {
                    range: Sid::new(0)..Sid::new(1),
                    backings: vec![backing.clone(), backing.clone()],
                    results: results.clone(),
                })])
                .unwrap(),
            );
            let weak_table = Arc::downgrade(&table);
            // Repeated enumeration preserves order and duplicates; mappings use
            // the same owned backing rather than introducing another device.
            for _ in 0..2 {
                let target = &table.targets()[0];
                let mut ids = Vec::new();
                target.for_each_backing(&mut |device| ids.push(device.id()));
                assert_eq!(ids, vec![backing.id(), backing.id()]);
                let actions = target.map_io_range(Sid::new(0)..Sid::new(1)).unwrap();
                assert_eq!(actions.len(), 1);
                assert_eq!(actions[0].backing.id(), ids[0]);
                assert_eq!(actions[0].logical_range, Sid::new(0)..Sid::new(1));
                assert!(target.io_completion().is_some());
            }
            let notified = Arc::new(AtomicUsize::new(0));
            let notification = notified.clone();
            let observed = results.clone();
            let id = backing.id();
            let bio = Bio::new(
                BioType::Read,
                Sid::new(0),
                vec![BioSegment::alloc_exact(1, 512, BioDirection::FromDevice)],
                Some(Box::new(move |result| {
                    assert_eq!(result, status);
                    assert_eq!(*observed.lock(), vec![(id, status)]);
                    notification.fetch_add(1, Ordering::Relaxed);
                })),
            );
            table.enqueue(bio.submit_for_test()).unwrap();
            let pending = backing.pending.lock().pop().unwrap();
            drop(table);
            drop(backing);
            assert!(weak_table.upgrade().is_some());
            assert!(weak_backing.upgrade().is_some());
            assert!(results.lock().is_empty());
            assert_eq!(notified.load(Ordering::Relaxed), 0);
            pending.complete(status);
            assert_eq!(notified.load(Ordering::Relaxed), 1);
            crate::reclaim::reclaim_for_test();
            assert!(weak_table.upgrade().is_none());
            assert!(weak_backing.upgrade().is_none());
        }
    }

    #[ktest]
    fn normal_io_rejection_does_not_notify_target() {
        for error in [
            BioEnqueueError::TooBig,
            BioEnqueueError::IsFull,
            BioEnqueueError::Refused,
        ] {
            let backing = flush_device(91, Some(Err(error)));
            let results = Arc::new(Mutex::new(Vec::new()));
            let table = Arc::new(
                DmTable::new_targets(vec![Box::new(FlushTarget {
                    range: Sid::new(0)..Sid::new(1),
                    backings: vec![backing],
                    results: results.clone(),
                })])
                .unwrap(),
            );
            let bio = Bio::new(
                BioType::Read,
                Sid::new(0),
                vec![BioSegment::alloc_exact(1, 512, BioDirection::FromDevice)],
                Some(Box::new(|_| panic!("rejected request must not complete"))),
            );
            assert_eq!(table.enqueue(bio.submit_for_test()), Err(error));
            assert!(results.lock().is_empty());
        }
    }

    #[derive(Debug)]
    struct ImmediateDevice(Result<BioStatus, BioEnqueueError>);

    impl BlockDevice for ImmediateDevice {
        fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            let status = self.0?;
            bio.complete(status);
            Ok(())
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                nr_sectors: 16,
                max_nr_segments_per_bio: 8,
            }
        }

        fn name(&self) -> &str {
            "dm-immediate-completion-test"
        }

        fn id(&self) -> DeviceId {
            DeviceId::new(MajorId::new(1), MinorId::new(83))
        }
    }

    #[derive(Debug)]
    struct ForwardingDevice {
        backing: ImmediateDevice,
        order: Arc<AtomicUsize>,
    }

    impl BlockDevice for ForwardingDevice {
        fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            let order = self.order.clone();
            enqueue_with_completion(bio, &self.backing, move |_| {
                assert_eq!(order.fetch_add(1, Ordering::Relaxed), 0);
            })
        }

        fn metadata(&self) -> BlockDeviceMeta {
            self.backing.metadata()
        }

        fn name(&self) -> &str {
            "dm-completion-forwarding-test"
        }

        fn id(&self) -> DeviceId {
            self.backing.id()
        }
    }

    #[ktest]
    fn synchronous_completion_processes_result_before_notifying_submitter() {
        for status in [
            BioStatus::Complete,
            BioStatus::IoError,
            BioStatus::NotSupported,
        ] {
            let order = Arc::new(AtomicUsize::new(0));
            let notified = order.clone();
            let device = ForwardingDevice {
                backing: ImmediateDevice(Ok(status)),
                order: order.clone(),
            };
            let bio = Bio::new(
                BioType::Read,
                Sid::new(0),
                vec![BioSegment::alloc_exact(1, 512, BioDirection::FromDevice)],
                Some(Box::new(move |result| {
                    assert_eq!(result, status);
                    assert_eq!(notified.fetch_add(1, Ordering::Relaxed), 1);
                })),
            );
            assert_eq!(bio.submit_and_wait(&device), Ok(status));
            assert_eq!(order.load(Ordering::Relaxed), 2);
        }
    }

    #[ktest]
    fn immediate_rejection_does_not_process_or_notify_completion() {
        for error in [
            BioEnqueueError::TooBig,
            BioEnqueueError::IsFull,
            BioEnqueueError::Refused,
        ] {
            let order = Arc::new(AtomicUsize::new(0));
            let notified = order.clone();
            let device = ForwardingDevice {
                backing: ImmediateDevice(Err(error)),
                order: order.clone(),
            };
            let bio = Bio::new(
                BioType::Flush,
                Sid::new(0),
                Vec::new(),
                Some(Box::new(move |_| {
                    notified.fetch_add(1, Ordering::Relaxed);
                })),
            );
            assert_eq!(bio.submit_and_wait(&device), Err(error));
            assert_eq!(order.load(Ordering::Relaxed), 0);
        }
    }
}
