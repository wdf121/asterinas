// SPDX-License-Identifier: MPL-2.0

//! Immutable Device Mapper table topology and I/O planning.
//!
//! Targets own mapping semantics; the I/O module executes the resulting plan.

use alloc::{sync::Arc, vec::Vec};

use aster_block::{
    BlockDevice,
    bio::{BioEnqueueError, BioType, SubmittedBio},
};
use device_id::DeviceId;

use crate::{
    TableError, io,
    reclaim::CompletionTable,
    target::{DmTargetBox, TargetIoAction},
};

/// A contiguous, immutable Device Mapper table installed on a mapped device.
///
/// Targets retain their backing devices for the table's lifetime,
/// including while the table is inactive or retained by an older I/O generation.
/// Immutability covers mapping topology and read-only mode,
/// not target-specific runtime state such as completion health counters.
#[derive(Debug)]
pub struct DmTable {
    /// Ordered targets whose logical ranges must start at zero and be contiguous.
    targets: Vec<DmTargetBox>,
    /// Total table capacity in 512-byte sectors, equal to the last target end.
    length: u64,
    /// Linux `DM_READONLY_FLAG` mode captured when this table was loaded.
    readonly: bool,
}

/// A mapping action paired with its owning target, retained through execution.
pub(super) struct PlannedIo<'a> {
    pub(super) mapping: TargetIoAction<'a>,
    pub(super) target_index: usize,
}

/// One distinct flush destination and the targets that reference it.
pub(super) struct PlannedFlush<'a> {
    pub(super) backing: &'a dyn BlockDevice,
    pub(super) target_indices: Vec<usize>,
}

impl DmTable {
    /// Creates a writable table after enforcing Linux-visible table invariants not owned by targets.
    pub fn new_targets(targets: Vec<DmTargetBox>) -> Result<Self, TableError> {
        Self::new_targets_with_readonly(targets, false)
    }

    /// Creates a table with the mode captured from `DM_TABLE_LOAD`.
    ///
    /// Requires at least one non-empty target,
    /// with ordered logical ranges covering a contiguous interval from sector zero.
    /// Direct mapper backings are rejected because stacked DM devices are unsupported.
    /// Each target remains responsible for validating its own backing geometry.
    /// Construction neither installs the table nor changes any existing generation.
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
            if target.logical_range().start >= target.logical_range().end {
                return Err(TableError::ZeroLength);
            }
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
            let mut collect_id = |backing: &dyn BlockDevice| {
                let id = backing.id();
                if !ids.contains(&id) {
                    ids.push(id);
                }
            };
            target.for_each_backing(&mut collect_id);
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

    /// Forwards one BIO, splitting at target and chunk boundaries.
    ///
    /// All ordinary mappings are validated before any backing admission.
    /// This does not reserve every execution resource in advance
    /// or make writes atomic across backings.
    /// Planning allocations follow the crate's allocation policy.
    ///
    /// A single mapping can return a backing enqueue error directly.
    /// Once split children or flush destinations are dispatched,
    /// their admission failures become completion errors on the parent BIO;
    /// success therefore does not imply successful backing I/O.
    /// Flushes visit each distinct backing device ID once.
    ///
    /// This low-level entry point bypasses mapper suspension, drain accounting,
    /// and read-only enforcement; normal device I/O must go through [`crate::DmDevice`].
    /// Completion-owned table references are reclaimed in task context,
    /// so backing leases may outlive the BIO's logical completion.
    pub(crate) fn enqueue(self: &Arc<Self>, mut bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        if bio.type_() != BioType::Flush {
            // Reject out-of-range I/O before taking completion ownership.
            let parts = self.plan_normal_io(&bio)?;
            let table = CompletionTable::new(self);
            bio.chain_complete_fn(move |_| drop(table));
            return io::execute_normal_io(self, bio, parts);
        }
        let plan = self.plan_flush();
        let table = CompletionTable::new(self);
        bio.chain_complete_fn(move |_| drop(table));
        io::execute_flush(self, bio, plan)
    }

    fn plan_flush(&self) -> Vec<PlannedFlush<'_>> {
        let mut plan: Vec<PlannedFlush<'_>> = Vec::new();
        for (target_index, target) in self.targets.iter().enumerate() {
            target.for_each_backing(&mut |backing| {
                if let Some(part) = plan
                    .iter_mut()
                    .find(|part| part.backing.id() == backing.id())
                {
                    // A striped target may reference the same backing more than once.
                    if part.target_indices.last() != Some(&target_index) {
                        part.target_indices.push(target_index);
                    }
                } else {
                    plan.push(PlannedFlush {
                        backing,
                        target_indices: alloc::vec![target_index],
                    });
                }
            });
        }
        plan
    }

    fn plan_normal_io(&self, bio: &SubmittedBio) -> Result<Vec<PlannedIo<'_>>, BioEnqueueError> {
        let range = bio.sid_range();
        if range.start >= range.end || range.end.to_raw() > self.length {
            return Err(BioEnqueueError::Refused);
        }

        // Table construction guarantees ordered, contiguous target ranges.
        let first = self
            .targets
            .partition_point(|target| target.logical_range().end <= range.start);
        let mut cursor = range.start;
        let mut parts = Vec::new();
        for (target_index, target) in self.targets.iter().enumerate().skip(first) {
            let end = range.end.min(target.logical_range().end);
            let mappings = target
                .map_io_range(cursor..end)
                .ok_or(BioEnqueueError::Refused)?;
            for mapping in mappings {
                // Validate before any backing admission. The single-action fast
                // path does not pass through SubmittedBio::split validation.
                if mapping.logical_range.start != cursor
                    || mapping.logical_range.end <= cursor
                    || mapping.logical_range.end > end
                    || mapping
                        .backing_start
                        .to_raw()
                        .checked_add(mapping.logical_range.end.to_raw() - cursor.to_raw())
                        .is_none()
                {
                    return Err(BioEnqueueError::Refused);
                }
                cursor = mapping.logical_range.end;
                parts.push(PlannedIo {
                    mapping,
                    target_index,
                });
            }
            if cursor != end {
                return Err(BioEnqueueError::Refused);
            }
            if cursor == range.end {
                break;
            }
        }
        Ok(parts)
    }
}

#[cfg(ktest)]
mod tests {
    use alloc::{boxed::Box, string::ToString, vec};
    use core::ops::Range;

    use aster_block::{
        BlockDeviceLease, BlockDeviceMeta,
        bio::{Bio, BioDirection, BioSegment, BioStatus},
        id::Sid,
        lookup_lease, register, unregister,
    };
    use device_id::{MajorId, MinorId};
    use ostd::{
        prelude::ktest,
        sync::{Mutex, SpinLock},
    };

    use super::*;
    use crate::{
        DmManager,
        target::{DmTarget, DmTargetMetadata, TargetStatusMode, linear::LinearTarget},
    };

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

    fn linear_table(targets: Vec<LinearTarget>) -> Result<DmTable, TableError> {
        DmTable::new_targets(
            targets
                .into_iter()
                .map(|target| Box::new(target) as DmTargetBox)
                .collect(),
        )
    }

    fn single_linear_table(
        logical_start: Sid,
        length: u64,
        backing_start: Sid,
        backing: BlockDeviceLease,
    ) -> Result<DmTable, TableError> {
        linear_table(vec![LinearTarget::new(
            logical_start,
            length,
            backing_start,
            backing,
        )?])
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

    #[derive(Debug)]
    struct InvalidMappingTarget {
        inner: LinearTarget,
        ranges: Vec<Range<Sid>>,
        backing_start: Sid,
    }

    impl DmTarget for InvalidMappingTarget {
        fn as_any(&self) -> &dyn core::any::Any {
            self
        }

        fn metadata(&self) -> DmTargetMetadata {
            self.inner.metadata()
        }

        fn logical_range(&self) -> &Range<Sid> {
            self.inner.logical_range()
        }

        fn for_each_backing<'a>(&'a self, f: &mut dyn FnMut(&'a dyn BlockDevice)) {
            self.inner.for_each_backing(f);
        }

        fn map_io_range(&self, _logical: Range<Sid>) -> Option<Vec<TargetIoAction<'_>>> {
            Some(
                self.ranges
                    .iter()
                    .map(|range| TargetIoAction {
                        logical_range: range.clone(),
                        backing_start: self.backing_start,
                        backing: self.inner.backing(),
                    })
                    .collect(),
            )
        }

        fn status_params(
            &self,
            mode: TargetStatusMode,
        ) -> Result<alloc::string::String, TableError> {
            self.inner.status_params(mode)
        }
    }

    #[ktest]
    fn rejects_invalid_target_mappings_before_any_backing_admission() {
        let cases = [
            (vec![], Sid::new(0)),
            (
                core::iter::once(Sid::new(4)..Sid::new(7)).collect(),
                Sid::new(0),
            ),
            (
                core::iter::once(Sid::new(5)..Sid::new(8)).collect(),
                Sid::new(0),
            ),
            (
                vec![Sid::new(4)..Sid::new(6), Sid::new(5)..Sid::new(8)],
                Sid::new(0),
            ),
            (
                core::iter::once(Sid::new(4)..Sid::new(9)).collect(),
                Sid::new(0),
            ),
            (
                core::iter::once(Sid::new(4)..Sid::new(4)).collect(),
                Sid::new(0),
            ),
            (
                core::iter::once(Sid::new(8)..Sid::new(4)).collect(),
                Sid::new(0),
            ),
            (
                core::iter::once(Sid::new(4)..Sid::new(8)).collect(),
                Sid::new(u64::MAX),
            ),
        ];
        for (ranges, backing_start) in cases {
            let backing = RecordingBlockDevice::new(85, false, 8);
            let target = InvalidMappingTarget {
                inner: linear(backing.clone(), 4, 0),
                ranges,
                backing_start,
            };
            let table = Arc::new(
                DmTable::new_targets(vec![
                    Box::new(linear(backing.clone(), 0, 0)),
                    Box::new(target),
                ])
                .unwrap(),
            );
            // Exercise both the single-action path and a preceding valid target.
            for start in [4, 0] {
                let notified = Arc::new(SpinLock::<bool>::new(false));
                let on_complete = notified.clone();
                let bio = Bio::new(
                    BioType::Read,
                    Sid::new(start),
                    vec![BioSegment::alloc_exact(
                        1,
                        ((8 - start) * 512) as usize,
                        BioDirection::FromDevice,
                    )],
                    Some(Box::new(move |_| *on_complete.lock() = true)),
                );
                assert_eq!(
                    bio.submit_and_wait(&TableDevice(table.clone())),
                    Err(BioEnqueueError::Refused)
                );
                assert!(!*notified.lock());
                assert!(backing.ranges.lock().is_empty());
                assert!(backing.pending.lock().is_empty());
            }
        }
    }

    fn striped_table(backings: &[Arc<RecordingBlockDevice>]) -> Arc<DmTable> {
        let target = crate::target::parse_target_with(
            "striped",
            Sid::new(0),
            12,
            "3 2 a 100 b 200 c 300",
            &mut |token| {
                Ok::<_, ()>(
                    backings[match token {
                        "a" => 0,
                        "b" => 1,
                        _ => 2,
                    }]
                    .id(),
                )
            },
            &mut |id| {
                Ok(BlockDeviceLease::new_untracked(
                    backings.iter().find(|b| b.id() == id).unwrap().clone(),
                ))
            },
        )
        .unwrap();
        Arc::new(DmTable::new_targets(vec![target]).unwrap())
    }

    #[ktest]
    fn striped_read_preserves_chunk_data_and_waits_for_all_children() {
        use ostd::mm::VmIo;

        let devices = [
            RecordingBlockDevice::new(71, false, 8),
            RecordingBlockDevice::new(72, false, 4),
            RecordingBlockDevice::new(73, false, 6),
        ];
        let table = striped_table(&devices);
        assert_eq!(
            table.backing_ids(),
            devices.iter().map(|d| d.id()).collect::<Vec<_>>()
        );
        assert_eq!(table.metadata().max_nr_segments_per_bio, 4);
        let segment = BioSegment::alloc_exact(1, 4096, BioDirection::FromDevice);
        let completed = Arc::new(Mutex::new(None));
        let completion = completed.clone();
        let bio = Bio::new(
            BioType::Read,
            Sid::new(1),
            vec![segment.clone()],
            Some(Box::new(move |status| *completion.lock() = Some(status))),
        );
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(&TableDevice(table), &mut batch).unwrap();
        assert_eq!(
            *devices[0].ranges.lock(),
            vec![Sid::new(101)..Sid::new(102), Sid::new(102)..Sid::new(104)]
        );
        assert_eq!(
            *devices[1].ranges.lock(),
            vec![Sid::new(200)..Sid::new(202), Sid::new(202)..Sid::new(203)]
        );
        assert_eq!(
            *devices[2].ranges.lock(),
            vec![Sid::new(300)..Sid::new(302)]
        );
        for (device_index, device) in devices.iter().enumerate() {
            for (index, child) in device.pending.lock().iter().enumerate() {
                let value = (device_index + index * 3 + 1) as u8;
                for part in child.segments() {
                    part.dma_slice()
                        .write_bytes(0, &alloc::vec![value; part.nbytes()])
                        .unwrap();
                }
            }
        }
        devices[2].finish(BioStatus::Complete);
        devices[1].finish(BioStatus::Complete);
        devices[1].finish(BioStatus::Complete);
        devices[0].finish(BioStatus::Complete);
        assert_eq!(*completed.lock(), None);
        devices[0].finish(BioStatus::Complete);
        assert!(batch.wait_all().is_ok());
        let mut data = [0; 4096];
        segment.dma_slice().read_bytes(0, &mut data).unwrap();
        for (range, value) in [
            (0..512, 1),
            (512..1536, 2),
            (1536..2560, 3),
            (2560..3584, 4),
            (3584..4096, 5),
        ] {
            assert!(data[range].iter().all(|byte| *byte == value));
        }
    }

    #[ktest]
    fn striped_flush_deduplicates_devices_and_tracks_failures() {
        use crate::target::TargetStatusMode;

        let first = RecordingBlockDevice::new(74, false, 8);
        let second = RecordingBlockDevice::new(75, false, 8);
        let table = striped_table(&[first.clone(), second.clone(), first.clone()]);
        assert_eq!(table.backing_ids(), vec![first.id(), second.id()]);
        let bio = Bio::new(BioType::Flush, Sid::new(0), Vec::new(), None);
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(&TableDevice(table.clone()), &mut batch).unwrap();
        assert_eq!(first.pending.lock().len(), 1);
        assert_eq!(second.pending.lock().len(), 1);
        first.finish(BioStatus::IoError);
        second.finish(BioStatus::Complete);
        assert!(batch.wait_all().is_err());
        assert_eq!(
            table.targets()[0]
                .status_params(TargetStatusMode::Status)
                .unwrap(),
            "3 1:74 1:75 1:74 1 DAD"
        );
    }

    #[ktest]
    fn striped_write_enqueue_failure_waits_without_marking_device_failed() {
        use crate::target::TargetStatusMode;

        let first = RecordingBlockDevice::new(76, false, 8);
        let second = RecordingBlockDevice::new(77, true, 8);
        let third = RecordingBlockDevice::new(78, false, 8);
        let table = striped_table(&[first.clone(), second, third.clone()]);
        let completed = Arc::new(Mutex::new(None));
        let completion = completed.clone();
        let bio = Bio::new(
            BioType::Write,
            Sid::new(0),
            vec![BioSegment::alloc_exact(1, 3072, BioDirection::ToDevice)],
            Some(Box::new(move |status| *completion.lock() = Some(status))),
        );
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(&TableDevice(table.clone()), &mut batch).unwrap();
        first.finish(BioStatus::NotSupported);
        assert_eq!(*completed.lock(), None);
        third.finish(BioStatus::Complete);
        assert!(batch.wait_all().is_err());
        assert_eq!(
            table.targets()[0]
                .status_params(TargetStatusMode::Status)
                .unwrap(),
            "3 1:76 1:77 1:78 1 AAA"
        );
    }

    #[ktest]
    fn striped_admission_errors_preserve_health() {
        use crate::target::TargetStatusMode;

        #[derive(Debug)]
        struct RejectingDevice(BioEnqueueError);
        impl BlockDevice for RejectingDevice {
            fn enqueue(&self, _: SubmittedBio) -> Result<(), BioEnqueueError> {
                Err(self.0)
            }
            fn metadata(&self) -> BlockDeviceMeta {
                BlockDeviceMeta {
                    nr_sectors: 1024,
                    max_nr_segments_per_bio: 1,
                }
            }
            fn name(&self) -> &str {
                "striped-admission-test"
            }
            fn id(&self) -> DeviceId {
                DeviceId::new(MajorId::new(1), MinorId::new(81))
            }
        }
        for error in [BioEnqueueError::TooBig, BioEnqueueError::IsFull] {
            let backing = Arc::new(RejectingDevice(error));
            let target = crate::target::parse_target_with(
                "striped",
                Sid::new(0),
                8,
                "1 2 a 0",
                &mut |_| Ok::<_, ()>(backing.id()),
                &mut |_| Ok(BlockDeviceLease::new_untracked(backing.clone())),
            )
            .unwrap();
            let table = Arc::new(DmTable::new_targets(vec![target]).unwrap());
            for sectors in [1, 8] {
                let bio = Bio::new(
                    BioType::Write,
                    Sid::new(0),
                    vec![BioSegment::alloc_exact(
                        1,
                        sectors * 512,
                        BioDirection::ToDevice,
                    )],
                    None,
                );
                let mut batch = io_util::batch::IoBatch::with_capacity(1);
                let result = bio.submit(&TableDevice(table.clone()), &mut batch);
                if sectors == 1 {
                    assert_eq!(result, Err(error));
                } else {
                    result.unwrap();
                    assert!(batch.wait_all().is_err());
                }
                assert_eq!(
                    table.targets()[0]
                        .status_params(TargetStatusMode::Status)
                        .unwrap(),
                    "1 1:81 1 A"
                );
            }
            let flush = Bio::new(BioType::Flush, Sid::new(0), Vec::new(), None);
            let mut batch = io_util::batch::IoBatch::with_capacity(1);
            flush
                .submit(&TableDevice(table.clone()), &mut batch)
                .unwrap();
            assert!(batch.wait_all().is_err());
            assert_eq!(
                table.targets()[0]
                    .status_params(TargetStatusMode::Status)
                    .unwrap(),
                "1 1:81 1 A"
            );
        }
    }

    #[ktest]
    fn striped_terminal_error_updates_health_before_single_or_split_callback() {
        use crate::target::TargetStatusMode;

        for sectors in [1, 6] {
            let backing = RecordingBlockDevice::new(82, false, 8);
            let table = striped_table(&[backing.clone(), backing.clone(), backing.clone()]);
            let observed = Arc::new(Mutex::new(None));
            let callback_observed = observed.clone();
            let callback_table = table.clone();
            let bio = Bio::new(
                BioType::Read,
                Sid::new(0),
                vec![BioSegment::alloc_exact(
                    1,
                    sectors * 512,
                    BioDirection::FromDevice,
                )],
                Some(Box::new(move |_| {
                    *callback_observed.lock() = Some(
                        callback_table.targets()[0]
                            .status_params(TargetStatusMode::Status)
                            .unwrap(),
                    );
                })),
            );
            let mut batch = io_util::batch::IoBatch::with_capacity(1);
            bio.submit(&TableDevice(table), &mut batch).unwrap();
            while backing.pending.lock().len() > 1 {
                backing.finish(BioStatus::Complete);
                assert!(observed.lock().is_none());
            }
            // The last child's error must be published before parent notification.
            backing.finish(BioStatus::IoError);
            assert!(batch.wait_all().is_err());
            assert_eq!(observed.lock().as_deref(), Some("3 1:82 1:82 1:82 1 DDD"));
        }
    }

    #[ktest]
    fn striped_terminal_error_only_updates_owning_target() {
        use crate::target::TargetStatusMode;

        let backing = RecordingBlockDevice::new(84, false, 8);
        let mut targets = Vec::new();
        for start in [0, 4] {
            targets.push(
                crate::target::parse_target_with(
                    "striped",
                    Sid::new(start),
                    4,
                    "1 2 a 0",
                    &mut |_| Ok::<_, ()>(backing.id()),
                    &mut |_| Ok(BlockDeviceLease::new_untracked(backing.clone())),
                )
                .unwrap(),
            );
        }
        let table = Arc::new(DmTable::new_targets(targets).unwrap());
        let bio = Bio::new(
            BioType::Read,
            Sid::new(4),
            vec![BioSegment::alloc_exact(1, 512, BioDirection::FromDevice)],
            None,
        );
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(&TableDevice(table.clone()), &mut batch).unwrap();
        backing.finish(BioStatus::IoError);
        assert!(batch.wait_all().is_err());
        assert_eq!(
            table.targets()[0]
                .status_params(TargetStatusMode::Status)
                .unwrap(),
            "1 1:84 1 A"
        );
        assert_eq!(
            table.targets()[1]
                .status_params(TargetStatusMode::Status)
                .unwrap(),
            "1 1:84 1 D"
        );
    }

    #[ktest]
    fn striped_replacement_changes_capacity_without_remapping_old_io() {
        let first = RecordingBlockDevice::new(79, false, 8);
        let second = RecordingBlockDevice::new(80, false, 8);
        register(first.clone()).unwrap();
        register(second.clone()).unwrap();
        let make_table_fn = |length| {
            let target = crate::target::parse_target_with(
                "striped",
                Sid::new(0),
                length,
                "2 2 a 10 b 20",
                &mut |token| {
                    Ok::<_, ()>(if token == "a" {
                        first.id()
                    } else {
                        second.id()
                    })
                },
                &mut |id| Ok(lookup_lease(id).unwrap()),
            )
            .unwrap();
            Arc::new(DmTable::new_targets(vec![target]).unwrap())
        };
        let manager = DmManager::new().unwrap();
        let mapper = manager.create("striped-resize".into(), None, None).unwrap();
        mapper
            .lock_lifecycle()
            .load_table(make_table_fn(8))
            .unwrap();
        mapper.lock_lifecycle().resume_initial().unwrap();
        let old = mapper.active_table().unwrap();
        let bio = Bio::new(
            BioType::Write,
            Sid::new(2),
            vec![BioSegment::alloc_exact(1, 1024, BioDirection::ToDevice)],
            None,
        );
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(&TableDevice(old.clone()), &mut batch).unwrap();
        // Direct table submission deliberately retains an old-table user outside
        // mapper admission, isolating table lifetime from suspend/drain behavior.
        mapper
            .lock_lifecycle()
            .load_table(make_table_fn(16))
            .unwrap();
        mapper.lock_lifecycle().suspend().unwrap();
        mapper.lock_lifecycle().resume_suspended().unwrap();
        assert_eq!(mapper.metadata().nr_sectors, 16);
        mapper
            .lock_lifecycle()
            .load_table(make_table_fn(4))
            .unwrap();
        mapper.lock_lifecycle().suspend().unwrap();
        mapper.lock_lifecycle().resume_suspended().unwrap();
        assert_eq!(mapper.metadata().nr_sectors, 4);
        let beyond = Bio::new(
            BioType::Read,
            Sid::new(4),
            vec![BioSegment::alloc_exact(1, 512, BioDirection::FromDevice)],
            None,
        );
        assert_eq!(
            beyond.submit(&TableDevice(mapper.active_table().unwrap()), &mut batch),
            Err(BioEnqueueError::Refused)
        );
        assert_eq!(*second.ranges.lock(), vec![Sid::new(20)..Sid::new(22)]);
        drop(old);
        manager.remove("striped-resize").unwrap();
        drop(mapper);
        assert_eq!(
            unregister(first.id()).unwrap_err(),
            aster_block::Error::Busy
        );
        assert_eq!(
            unregister(second.id()).unwrap_err(),
            aster_block::Error::Busy
        );
        second.finish(BioStatus::Complete);
        assert!(batch.wait_all().is_ok());
        crate::reclaim::reclaim_for_test();
        unregister(first.id()).unwrap();
        unregister(second.id()).unwrap();
    }

    #[ktest]
    fn forwards_single_linear_bio_and_reports_metadata() {
        let backing = RecordingBlockDevice::new(1, false, 3);
        let table = Arc::new(linear_table(vec![linear(backing.clone(), 0, 100)]).unwrap());
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
            linear_table(vec![
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
            linear_table(vec![
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
            linear_table(vec![linear(first.clone(), 0, 100), linear(second, 4, 200)]).unwrap(),
        );
        assert_eq!(table.metadata().max_nr_segments_per_bio, 2);
        let completed = Arc::new(Mutex::new(None));
        let on_complete = completed.clone();
        let bio = Bio::new(
            BioType::Read,
            Sid::new(0),
            vec![BioSegment::alloc_exact(1, 4096, BioDirection::FromDevice)],
            Some(Box::new(move |status| {
                let mut completed = on_complete.lock();
                assert!(completed.is_none(), "parent completed more than once");
                *completed = Some(status);
            })),
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
            linear_table(vec![
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
            linear_table(vec![
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
            Some(Box::new(move |status| {
                let mut completed = on_complete.lock();
                assert!(completed.is_none(), "parent completed more than once");
                *completed = Some(status);
            })),
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
        let table = Arc::new(linear_table(vec![linear(backing.clone(), 0, 0)]).unwrap());
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
        let table = Arc::new(linear_table(vec![linear(backing.clone(), 0, 0)]).unwrap());
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
            single_linear_table(Sid::new(0), 4, Sid::new(0), lookup_lease(id).unwrap()).unwrap(),
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
        crate::reclaim::reclaim_for_test();
        unregister(id).unwrap();
    }

    #[ktest]
    fn irq_completion_defers_final_tracked_table_reference() {
        for type_ in [BioType::Read, BioType::Flush] {
            let backing = RecordingBlockDevice::new(49, false, 8);
            let id = backing.id();
            register(backing.clone()).unwrap();
            let table = Arc::new(
                single_linear_table(Sid::new(0), 4, Sid::new(0), lookup_lease(id).unwrap())
                    .unwrap(),
            );
            let segments = if type_ == BioType::Read {
                vec![BioSegment::alloc_exact(1, 512, BioDirection::FromDevice)]
            } else {
                vec![]
            };
            let mut batch = io_util::batch::IoBatch::with_capacity(1);
            Bio::new(type_, Sid::new(0), segments, None)
                .submit(&TableDevice(table), &mut batch)
                .unwrap();
            let pending = backing.pending.lock().remove(0);
            {
                let _irq_guard = ostd::irq::disable_local();
                pending.complete(BioStatus::Complete);
            }
            assert!(batch.wait_all().is_ok());
            // Logical completion does not release the tracked lease in IRQ context.
            assert_eq!(unregister(id).unwrap_err(), aster_block::Error::Busy);
            crate::reclaim::reclaim_for_test();
            unregister(id).unwrap();
        }
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
            single_linear_table(Sid::new(0), 8, Sid::new(0), lookup_lease(id).unwrap()).unwrap();
        mapper.lock_lifecycle().load_table(Arc::new(table)).unwrap();
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
        let make_table_fn = |id| {
            Arc::new(
                single_linear_table(Sid::new(0), 8, Sid::new(0), lookup_lease(id).unwrap())
                    .unwrap(),
            )
        };
        mapper
            .lock_lifecycle()
            .load_table(make_table_fn(old_id))
            .unwrap();
        assert_eq!(unregister(old_id).unwrap_err(), aster_block::Error::Busy);
        mapper
            .lock_lifecycle()
            .load_table(make_table_fn(new_id))
            .unwrap();
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
            single_linear_table(Sid::new(0), 8, Sid::new(0), lookup_lease(old_id).unwrap())
                .unwrap(),
        );
        mapper
            .lock_lifecycle()
            .load_table(old_table.clone())
            .unwrap();
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
            linear_table(vec![first, gap]).unwrap_err(),
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
            linear_table(vec![first, gap]).unwrap_err(),
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
        let table = Arc::new(linear_table(vec![first, second]).unwrap());
        assert_eq!(table.length(), 16);
        assert_eq!(table.target_count(), 2);
        assert_eq!(table.backing_ids(), vec![id]);
        mapper.lock_lifecycle().load_table(table).unwrap();
        assert_eq!(mapper.inactive_table().unwrap().length(), 16);
        assert_eq!(unregister(id).unwrap_err(), aster_block::Error::Busy);
        drop(manager);
        drop(mapper);
        unregister(id).unwrap();
    }
}
