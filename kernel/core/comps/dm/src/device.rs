// SPDX-License-Identifier: MPL-2.0

//! Mapper identity and table ownership. Its number remains owned while references
//! exist, unless removal releases it after the control-plane drain.

use alloc::{collections::VecDeque, string::String, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};

use aster_block::{
    BlockDevice, BlockDeviceMeta,
    bio::{BioEnqueueError, BioStatus, BioType, SubmittedBio},
};
use device_id::DeviceId;
use ostd::sync::{Mutex, WaitQueue};

use crate::{DmError, DmTable, manager::DmDeviceIdOwner};

/// A mapper identity with separate active and inactive table generations.
#[derive(Debug)]
pub struct DmDevice {
    id_owner: DmDeviceIdOwner,
    primary_name: String,
    mapper_name: Mutex<String>,
    uuid: Mutex<Option<Vec<u8>>>,
    /// Selected under one lock so BIO admission cannot race first activation.
    tables: Mutex<DmDeviceTables>,
    /// Serializes LOAD, suspend, resume, and removal transitions for this mapper.
    lifecycle: Mutex<()>,
    io: Arc<DmIoState>,
    removal_isolated: AtomicBool,
}

struct DmIoState {
    inner: Mutex<DmIoInner>,
    drained: WaitQueue,
}

#[derive(Default)]
struct DmIoInner {
    stopped: bool,
    suspended: bool,
    admitting: usize,
    in_flight: usize,
    postponed: VecDeque<SubmittedBio>,
}

impl core::fmt::Debug for DmIoState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DmIoState").finish_non_exhaustive()
    }
}

impl DmIoState {
    fn complete(&self) {
        let mut inner = self.inner.lock();
        inner.in_flight -= 1;
        if inner.in_flight == 0 && inner.admitting == 0 {
            self.drained.wake_all();
        }
    }

    fn leave_enqueue(&self) {
        let mut inner = self.inner.lock();
        inner.admitting -= 1;
        if inner.in_flight == 0 && inner.admitting == 0 {
            self.drained.wake_all();
        }
    }
}

#[derive(Debug, Default)]
struct DmDeviceTables {
    active: Option<Arc<DmTable>>,
    inactive: Option<Arc<DmTable>>,
}

impl DmDevice {
    pub(crate) fn new(
        id_owner: DmDeviceIdOwner,
        mapper_name: String,
        uuid: Option<Vec<u8>>,
    ) -> Self {
        let primary_name = alloc::format!("dm-{}", id_owner.id().minor().get());
        Self {
            id_owner,
            primary_name,
            mapper_name: Mutex::new(mapper_name),
            uuid: Mutex::new(uuid),
            tables: Mutex::new(DmDeviceTables::default()),
            lifecycle: Mutex::new(()),
            io: Arc::new(DmIoState {
                inner: Mutex::new(DmIoInner::default()),
                drained: WaitQueue::new(),
            }),
            removal_isolated: AtomicBool::new(false),
        }
    }

    /// Returns the immutable block-device number.
    pub fn id(&self) -> DeviceId {
        self.id_owner.id()
    }

    /// Returns the immutable primary device name (`dm-N`).
    pub fn primary_name(&self) -> &str {
        &self.primary_name
    }

    /// Returns the mapper alias used by the control plane.
    pub fn mapper_name(&self) -> String {
        self.mapper_name.lock().clone()
    }

    /// Returns the opaque UUID bytes, if specified.
    pub fn uuid(&self) -> Option<Vec<u8>> {
        self.uuid.lock().clone()
    }

    /// Holds the mapper lifecycle across LOAD and first RESUME.
    pub fn lock_lifecycle(&self) -> ostd::sync::MutexGuard<'_, ()> {
        self.lifecycle.lock()
    }

    /// Installs a fully validated mapping table as the inactive table.
    pub fn load_table(&self, table: Arc<DmTable>) {
        self.tables.lock().inactive = Some(table);
    }

    /// Returns the inactive table, if one has been installed.
    pub fn inactive_table(&self) -> Option<Arc<DmTable>> {
        self.tables.lock().inactive.clone()
    }

    /// Returns a snapshot of the active table, if one has been activated.
    pub fn active_table(&self) -> Option<Arc<DmTable>> {
        self.tables.lock().active.clone()
    }

    /// Returns both table generations from one snapshot for control responses.
    pub fn table_snapshot(&self) -> (Option<Arc<DmTable>>, Option<Arc<DmTable>>) {
        let tables = self.tables.lock();
        (tables.active.clone(), tables.inactive.clone())
    }

    /// Activates the first loaded table. Lifecycle callers serialize this with LOAD.
    /// Repeated RESUME is a no-op, but replacing a running table requires a later
    /// suspend/reload implementation and must not silently discard the candidate.
    pub fn resume_initial(&self) -> Result<(), DmError> {
        let mut tables = self.tables.lock();
        if tables.active.is_some() {
            return if tables.inactive.is_some() {
                Err(DmError::InvalidState)
            } else {
                Ok(())
            };
        }
        let table = tables.inactive.take().ok_or(DmError::InvalidState)?;
        tables.active = Some(table);
        Ok(())
    }

    /// Marks a partially removed device as cleanup-only under its lifecycle lock.
    pub fn isolate_removal(&self) {
        self.removal_isolated.store(true, Ordering::Release);
    }

    pub fn removal_isolated(&self) -> bool {
        self.removal_isolated.load(Ordering::Acquire)
    }

    /// Pauses dispatch, waits for already assigned BIOs, and postpones new ones.
    /// The lifecycle guard serializes this with RESUME, LOAD and REMOVE.
    pub fn suspend(&self) -> Result<(), DmError> {
        if self.active_table().is_none() {
            return Err(DmError::InvalidState);
        }
        {
            let mut io = self.io.inner.lock();
            if io.stopped {
                return Err(DmError::InvalidState);
            }
            if io.suspended {
                return Ok(());
            }
            io.suspended = true;
        }
        self.io.drained.wait_until(|| {
            let io = self.io.inner.lock();
            (io.admitting == 0 && io.in_flight == 0).then_some(())
        });
        Ok(())
    }

    /// Returns whether new BIOs are postponed rather than dispatched.
    pub fn is_suspended(&self) -> bool {
        self.io.inner.lock().suspended
    }

    /// Resumes the active table, replacing it with an inactive candidate if present.
    /// A caller performing an implicit replacement must suspend and drain first.
    /// The I/O admission lock covers the swap, replay reservation, and reopening,
    /// so a new BIO cannot select the old table after the switch.
    pub fn resume_suspended(&self) -> Result<(), DmError> {
        let (table, postponed) = {
            let mut io = self.io.inner.lock();
            if io.stopped || !io.suspended || io.admitting != 0 || io.in_flight != 0 {
                return Err(DmError::InvalidState);
            }
            let mut tables = self.tables.lock();
            if tables.active.is_none() {
                return Err(DmError::InvalidState);
            }
            if let Some(replacement) = tables.inactive.take() {
                tables.active = Some(replacement);
            }
            let table = tables.active.as_ref().unwrap().clone();
            // Reserve replay completions before opening admission to new requests.
            let postponed = core::mem::take(&mut io.postponed);
            io.admitting += postponed.len();
            io.in_flight += postponed.len();
            io.suspended = false;
            (table, postponed)
        };
        for bio in postponed {
            if table.is_readonly() && bio.type_() == BioType::Write {
                bio.complete(BioStatus::IoError);
                self.io.complete();
                self.io.leave_enqueue();
            } else {
                self.forward_assigned(table.clone(), bio, true)
                    .expect("replayed BIO must complete errors internally");
            }
        }
        Ok(())
    }

    /// Stops admitting new I/O and waits for both forwarding calls and accepted
    /// BIO completions. The lifecycle caller must first unregister the primary
    /// device so this transition cannot need rollback.
    pub fn stop_and_drain_io(&self) {
        self.io.inner.lock().stopped = true;
        self.io.drained.wait_until(|| {
            let inner = self.io.inner.lock();
            (inner.admitting == 0 && inner.in_flight == 0).then_some(())
        });
        // Suspended BIOs were accepted but never sent to a table. Removal must
        // complete them before dropping the table and releasing the identity.
        let postponed = core::mem::take(&mut self.io.inner.lock().postponed);
        for bio in postponed {
            bio.complete(BioStatus::IoError);
        }
    }

    /// Drops both generations only after no accepted BIO can reference them.
    pub fn clear_removed_tables(&self) {
        let mut tables = self.tables.lock();
        tables.active = None;
        tables.inactive = None;
    }

    /// Releases the number after the caller has drained control and I/O users.
    /// Structural references can continue to observe the immutable identity.
    pub fn drain_removed(&self) {
        self.id_owner.release();
    }

    /// Forwards an already accounted BIO. Replay owns completion responsibility
    /// even when the lower layer refuses submission.
    fn forward_assigned(
        &self,
        table: Arc<DmTable>,
        mut bio: SubmittedBio,
        replay: bool,
    ) -> Result<(), BioEnqueueError> {
        if replay {
            bio.complete_as_io_error_on_drop();
        }
        let accounted = Arc::new(AtomicBool::new(true));
        let completion_io = self.io.clone();
        let completion_accounted = accounted.clone();
        bio.chain_complete_fn(move |_| {
            if completion_accounted.swap(false, Ordering::AcqRel) {
                completion_io.complete();
            }
        });
        let result = table.enqueue(bio);
        if result.is_err() && accounted.swap(false, Ordering::AcqRel) {
            self.io.complete();
        }
        self.io.leave_enqueue();
        if replay { Ok(()) } else { result }
    }
}

impl BlockDevice for DmDevice {
    fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        let table = {
            let mut io = self.io.inner.lock();
            if io.stopped {
                return Err(BioEnqueueError::Refused);
            }
            let table = self.active_table().ok_or(BioEnqueueError::Refused)?;
            // BIOs that reach a suspended mapper belong to the postponed queue,
            // regardless of the old table's mode. Judge writes against the new
            // active table on replay; the block-file or filesystem layer may
            // still reject a write before it reaches this method.
            if io.suspended {
                io.postponed.push_back(bio);
                return Ok(());
            }
            // Flush must remain permitted on a read-only table.
            if table.is_readonly() && bio.type_() == BioType::Write {
                return Err(BioEnqueueError::Refused);
            }
            io.admitting += 1;
            io.in_flight += 1;
            table
        };
        self.forward_assigned(table, bio, false)
    }

    fn metadata(&self) -> BlockDeviceMeta {
        self.active_table()
            .map(|table| table.metadata())
            .unwrap_or_default()
    }

    fn name(&self) -> &str {
        self.primary_name.as_str()
    }

    fn id(&self) -> DeviceId {
        self.id_owner.id()
    }
}

#[cfg(ktest)]
mod tests {
    use alloc::{string::ToString, vec};

    use aster_block::{
        BlockDeviceLease,
        bio::{Bio, BioDirection, BioSegment},
        id::Sid,
    };
    use device_id::{MajorId, MinorId};
    use ostd::prelude::ktest;

    use super::*;
    use crate::{DmManager, target::linear::LinearTarget};

    #[derive(Debug)]
    struct PendingBacking {
        id: DeviceId,
        pending: Mutex<Option<SubmittedBio>>,
    }

    impl PendingBacking {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                id: DeviceId::new(MajorId::new(1), MinorId::new(127)),
                pending: Mutex::new(None),
            })
        }

        fn finish(&self) {
            self.pending
                .lock()
                .take()
                .unwrap()
                .complete(BioStatus::Complete);
        }
    }

    impl BlockDevice for PendingBacking {
        fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            let mut pending = self.pending.lock();
            if pending.is_some() {
                return Err(BioEnqueueError::IsFull);
            }
            *pending = Some(bio);
            Ok(())
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 4,
                nr_sectors: 1024,
            }
        }

        fn name(&self) -> &str {
            "dm-device-test-backing"
        }

        fn id(&self) -> DeviceId {
            self.id
        }
    }

    fn table_with_range(
        backing: Arc<PendingBacking>,
        sectors: u64,
        offset: u64,
        readonly: bool,
    ) -> Arc<DmTable> {
        let target = LinearTarget::new(
            Sid::new(0),
            sectors,
            Sid::new(offset),
            BlockDeviceLease::new_untracked(backing),
        )
        .unwrap();
        Arc::new(
            DmTable::new_targets_with_readonly(vec![alloc::boxed::Box::new(target)], readonly)
                .unwrap(),
        )
    }

    fn table(backing: Arc<PendingBacking>, readonly: bool) -> Arc<DmTable> {
        table_with_range(backing, 8, 100, readonly)
    }

    fn mapper() -> (DmManager, Arc<DmDevice>) {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("mapper-io-test".to_string(), None, None)
            .unwrap();
        (manager, device)
    }

    #[ktest]
    fn mapper_admits_only_active_table_and_reports_its_metadata() {
        let (manager, device) = mapper();
        let backing = PendingBacking::new();
        let active = table(backing.clone(), false);
        device.load_table(active.clone());
        assert!(device.active_table().is_none());
        assert_eq!(device.metadata().nr_sectors, 0);
        let flush = Bio::new(BioType::Flush, Sid::new(0), vec![], None);
        assert_eq!(
            flush.submit_and_wait(device.as_ref()),
            Err(BioEnqueueError::Refused)
        );

        device.resume_initial().unwrap();
        assert!(Arc::ptr_eq(&device.active_table().unwrap(), &active));
        assert_eq!(device.metadata().nr_sectors, 8);
        assert_eq!(device.metadata().max_nr_segments_per_bio, 4);
        let bio = Bio::new(
            BioType::Read,
            Sid::new(1),
            vec![BioSegment::alloc_exact(1, 512, BioDirection::FromDevice)],
            None,
        );
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(device.as_ref(), &mut batch).unwrap();
        assert_eq!(
            backing.pending.lock().as_ref().unwrap().sid_range(),
            &(Sid::new(101)..Sid::new(102))
        );
        backing.finish();
        assert!(batch.wait_all().is_ok());
        drop(active);
        drop(manager);
        drop(device);
    }

    #[ktest]
    fn mapper_keeps_active_generation_during_inactive_load_and_pending_io() {
        let (manager, device) = mapper();
        let backing = PendingBacking::new();
        let active = table(backing.clone(), false);
        device.load_table(active.clone());
        device.resume_initial().unwrap();
        let bio = Bio::new(
            BioType::Write,
            Sid::new(0),
            vec![BioSegment::alloc_exact(1, 512, BioDirection::ToDevice)],
            None,
        );
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(device.as_ref(), &mut batch).unwrap();
        let weak = Arc::downgrade(&active);
        device.load_table(table(backing.clone(), true));
        assert!(Arc::ptr_eq(&device.active_table().unwrap(), &active));
        assert!(device.inactive_table().unwrap().is_readonly());
        device.tables.lock().active = None;
        drop(active);
        assert!(weak.upgrade().is_some());
        backing.finish();
        assert!(batch.wait_all().is_ok());
        assert!(weak.upgrade().is_none());
        drop(manager);
        drop(device);
    }

    #[ktest]
    fn first_resume_requires_table_and_rejects_hot_replacement() {
        let (manager, device) = mapper();
        assert_eq!(device.resume_initial(), Err(DmError::InvalidState));
        let backing = PendingBacking::new();
        let active = table(backing.clone(), false);
        device.load_table(active.clone());
        device.resume_initial().unwrap();
        device.resume_initial().unwrap();
        assert!(device.inactive_table().is_none());
        let replacement = table(backing, true);
        device.load_table(replacement.clone());
        assert_eq!(device.resume_initial(), Err(DmError::InvalidState));
        assert!(Arc::ptr_eq(&device.active_table().unwrap(), &active));
        assert!(Arc::ptr_eq(&device.inactive_table().unwrap(), &replacement));
        drop(manager);
        drop(device);
    }

    #[ktest]
    fn removal_waits_for_forwarded_io_and_rejects_later_io() {
        let (manager, device) = mapper();
        let backing = PendingBacking::new();
        let active = table(backing.clone(), false);
        device.load_table(active.clone());
        device.resume_initial().unwrap();
        let out_of_range = Bio::new(
            BioType::Read,
            Sid::new(1024),
            vec![BioSegment::alloc_exact(1, 512, BioDirection::FromDevice)],
            None,
        );
        assert_eq!(
            out_of_range.submit_and_wait(device.as_ref()),
            Err(BioEnqueueError::Refused)
        );
        assert_eq!(device.io.inner.lock().in_flight, 0);
        let bio = Bio::new(BioType::Flush, Sid::new(0), vec![], None);
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(device.as_ref(), &mut batch).unwrap();
        assert!(backing.pending.lock().is_some());
        let finished = Arc::new(AtomicBool::new(false));
        let worker_device = device.clone();
        let worker_finished = finished.clone();
        ostd::task::TaskOptions::new(move || {
            worker_device.stop_and_drain_io();
            worker_finished.store(true, Ordering::Release);
        })
        .spawn()
        .unwrap();
        wait_for_test_condition(|| device.io.inner.lock().stopped);
        assert!(!finished.load(Ordering::Acquire));
        assert!(device.active_table().is_some());
        let rejected = Bio::new(BioType::Flush, Sid::new(0), vec![], None);
        assert_eq!(
            rejected.submit_and_wait(device.as_ref()),
            Err(BioEnqueueError::Refused)
        );
        assert_eq!(device.io.inner.lock().in_flight, 1);
        backing.finish();
        assert!(batch.wait_all().is_ok());
        wait_for_test_condition(|| finished.load(Ordering::Acquire));
        assert_eq!(device.io.inner.lock().in_flight, 0);
        device.clear_removed_tables();
        assert!(device.active_table().is_none());
        manager.remove("mapper-io-test").unwrap();
        device.drain_removed();
    }

    #[ktest]
    fn suspended_bios_replay_and_removal_completes_postponed() {
        let (manager, device) = mapper();
        let backing = PendingBacking::new();
        device.load_table(table(backing.clone(), false));
        device.resume_initial().unwrap();
        assert!(device.suspend().is_ok());
        assert!(device.is_suspended());
        assert!(device.suspend().is_ok());
        let completed = Arc::new(Mutex::new(Vec::new()));
        let statuses = completed.clone();
        let bio = Bio::new(
            BioType::Flush,
            Sid::new(0),
            vec![],
            Some(alloc::boxed::Box::new(move |status| {
                statuses.lock().push(status)
            })),
        );
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(device.as_ref(), &mut batch).unwrap();
        assert!(backing.pending.lock().is_none());
        device.resume_suspended().unwrap();
        assert!(!device.is_suspended());
        assert!(backing.pending.lock().is_some());
        backing.finish();
        assert!(batch.wait_all().is_ok());
        assert_eq!(*completed.lock(), vec![BioStatus::Complete]);

        device.suspend().unwrap();
        let invalid = Bio::new(
            BioType::Read,
            Sid::new(1024),
            vec![BioSegment::alloc_exact(1, 512, BioDirection::FromDevice)],
            None,
        );
        let mut rejected_batch = io_util::batch::IoBatch::with_capacity(1);
        invalid
            .submit(device.as_ref(), &mut rejected_batch)
            .unwrap();
        device.resume_suspended().unwrap();
        assert!(rejected_batch.wait_all().is_err());
        assert_eq!(device.io.inner.lock().in_flight, 0);
        assert_eq!(device.io.inner.lock().admitting, 0);

        device.suspend().unwrap();
        let pending = Bio::new(BioType::Flush, Sid::new(0), vec![], None);
        let mut removed_batch = io_util::batch::IoBatch::with_capacity(1);
        pending.submit(device.as_ref(), &mut removed_batch).unwrap();
        device.stop_and_drain_io();
        assert!(removed_batch.wait_all().is_err());
        assert!(backing.pending.lock().is_none());
        device.clear_removed_tables();
        manager.remove("mapper-io-test").unwrap();
        device.drain_removed();
    }

    #[ktest]
    fn suspend_waits_for_old_bio_and_replays_postponed_on_new_table() {
        use ostd::task::TaskOptions;

        let (manager, device) = mapper();
        let old_backing = PendingBacking::new();
        let new_backing = PendingBacking::new();
        let old_table = table_with_range(old_backing.clone(), 8, 100, false);
        let old_weak = Arc::downgrade(&old_table);
        device.load_table(old_table.clone());
        device.resume_initial().unwrap();
        let mut old_batch = io_util::batch::IoBatch::with_capacity(1);
        Bio::new(
            BioType::Read,
            Sid::new(1),
            vec![BioSegment::alloc_exact(1, 512, BioDirection::FromDevice)],
            None,
        )
        .submit(device.as_ref(), &mut old_batch)
        .unwrap();
        assert_eq!(
            old_backing.pending.lock().as_ref().unwrap().sid_range(),
            &(Sid::new(101)..Sid::new(102))
        );

        let finished = Arc::new(AtomicBool::new(false));
        let result = Arc::new(Mutex::new(None));
        let worker_device = device.clone();
        let worker_finished = finished.clone();
        let worker_result = result.clone();
        TaskOptions::new(move || {
            *worker_result.lock() = Some(worker_device.suspend());
            worker_finished.store(true, Ordering::Release);
        })
        .spawn()
        .unwrap();
        // Observe that suspend has closed admission before testing its drain.
        wait_for_test_condition(|| device.is_suspended());
        assert!(!finished.load(Ordering::Acquire));
        assert_eq!(device.io.inner.lock().in_flight, 1);

        let mut postponed_batch = io_util::batch::IoBatch::with_capacity(1);
        Bio::new(
            BioType::Read,
            Sid::new(2),
            vec![BioSegment::alloc_exact(1, 512, BioDirection::FromDevice)],
            None,
        )
        .submit(device.as_ref(), &mut postponed_batch)
        .unwrap();
        assert_eq!(device.io.inner.lock().postponed.len(), 1);
        assert!(new_backing.pending.lock().is_none());
        old_backing.finish();
        assert!(old_batch.wait_all().is_ok());
        wait_for_test_condition(|| finished.load(Ordering::Acquire));
        assert_eq!(result.lock().take().unwrap(), Ok(()));
        assert!(old_backing.pending.lock().is_none());

        device.load_table(table_with_range(new_backing.clone(), 8, 200, false));
        device.resume_suspended().unwrap();
        assert_eq!(
            new_backing.pending.lock().as_ref().unwrap().sid_range(),
            &(Sid::new(202)..Sid::new(203))
        );
        assert!(old_backing.pending.lock().is_none());
        drop(old_table);
        assert!(old_weak.upgrade().is_none());
        new_backing.finish();
        assert!(postponed_batch.wait_all().is_ok());
        assert_eq!(device.io.inner.lock().in_flight, 0);
        drop(manager);
        drop(device);
    }

    fn wait_for_test_condition(mut condition: impl FnMut() -> bool) {
        // A stalled worker must fail the test rather than hang the entire ktest run.
        for _ in 0..1_000_000 {
            if condition() {
                return;
            }
            ostd::task::Task::yield_now();
        }
        panic!("device-mapper test worker did not reach the expected state");
    }

    #[ktest]
    fn suspend_switches_table_and_replays_on_new_generation() {
        let (manager, device) = mapper();
        let old_backing = PendingBacking::new();
        let new_backing = PendingBacking::new();
        let active = table(old_backing.clone(), false);
        device.load_table(active.clone());
        device.resume_initial().unwrap();
        device.suspend().unwrap();
        let replacement = table(new_backing.clone(), false);
        device.load_table(replacement.clone());
        let bio = Bio::new(BioType::Flush, Sid::new(0), vec![], None);
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        bio.submit(device.as_ref(), &mut batch).unwrap();
        assert!(old_backing.pending.lock().is_none());
        device.resume_suspended().unwrap();
        assert!(Arc::ptr_eq(&device.active_table().unwrap(), &replacement));
        assert!(device.inactive_table().is_none());
        assert!(new_backing.pending.lock().is_some());
        new_backing.finish();
        assert!(batch.wait_all().is_ok());
        drop(manager);
        drop(device);
    }

    #[ktest]
    fn suspended_shrink_replays_only_new_in_range_requests() {
        let (manager, device) = mapper();
        let backing = PendingBacking::new();
        device.load_table(table_with_range(backing.clone(), 16, 100, false));
        device.resume_initial().unwrap();
        assert_eq!(device.metadata().nr_sectors, 16);
        device.suspend().unwrap();
        let make_read = |sector| {
            Bio::new(
                BioType::Read,
                Sid::new(sector),
                vec![BioSegment::alloc_exact(1, 512, BioDirection::FromDevice)],
                None,
            )
        };
        let mut invalid_batch = io_util::batch::IoBatch::with_capacity(1);
        make_read(12)
            .submit(device.as_ref(), &mut invalid_batch)
            .unwrap();
        let mut valid_batch = io_util::batch::IoBatch::with_capacity(1);
        make_read(3)
            .submit(device.as_ref(), &mut valid_batch)
            .unwrap();
        device.load_table(table_with_range(backing.clone(), 8, 200, false));
        device.resume_suspended().unwrap();
        assert_eq!(device.metadata().nr_sectors, 8);
        assert!(invalid_batch.wait_all().is_err());
        assert_eq!(
            backing.pending.lock().as_ref().unwrap().sid_range(),
            &(Sid::new(203)..Sid::new(204))
        );
        backing.finish();
        assert!(valid_batch.wait_all().is_ok());
        assert_eq!(device.io.inner.lock().in_flight, 0);
        device.suspend().unwrap();
        device.load_table(table_with_range(backing.clone(), 24, 300, false));
        device.resume_suspended().unwrap();
        assert_eq!(device.metadata().nr_sectors, 24);
        let mut grown_batch = io_util::batch::IoBatch::with_capacity(1);
        make_read(20)
            .submit(device.as_ref(), &mut grown_batch)
            .unwrap();
        assert_eq!(
            backing.pending.lock().as_ref().unwrap().sid_range(),
            &(Sid::new(320)..Sid::new(321))
        );
        backing.finish();
        assert!(grown_batch.wait_all().is_ok());
        drop(manager);
        drop(device);
    }

    #[ktest]
    fn suspend_switches_inactive_candidate() {
        let (manager, device) = mapper();
        let backing = PendingBacking::new();
        let active = table(backing.clone(), false);
        device.load_table(active.clone());
        device.resume_initial().unwrap();
        let replacement = table(backing, true);
        device.load_table(replacement.clone());
        device.suspend().unwrap();
        device.resume_suspended().unwrap();
        assert!(!device.is_suspended());
        assert!(Arc::ptr_eq(&device.active_table().unwrap(), &replacement));
        assert!(device.inactive_table().is_none());
        device.stop_and_drain_io();
        device.clear_removed_tables();
        manager.remove("mapper-io-test").unwrap();
        device.drain_removed();
    }

    #[ktest]
    fn suspended_writes_obey_old_and_new_readonly_tables() {
        let (manager, device) = mapper();
        let backing = PendingBacking::new();
        device.load_table(table(backing.clone(), false));
        device.resume_initial().unwrap();
        device.suspend().unwrap();
        let write = || {
            Bio::new(
                BioType::Write,
                Sid::new(0),
                vec![BioSegment::alloc_exact(1, 512, BioDirection::ToDevice)],
                None,
            )
        };
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        write().submit(device.as_ref(), &mut batch).unwrap();
        device.load_table(table(backing.clone(), true));
        device.resume_suspended().unwrap();
        assert!(batch.wait_all().is_err());
        assert!(backing.pending.lock().is_none());
        assert_eq!(device.io.inner.lock().in_flight, 0);
        device.suspend().unwrap();
        device.load_table(table(backing.clone(), false));
        let mut queued = io_util::batch::IoBatch::with_capacity(1);
        write().submit(device.as_ref(), &mut queued).unwrap();
        assert_eq!(device.io.inner.lock().postponed.len(), 1);
        assert!(backing.pending.lock().is_none());
        device.resume_suspended().unwrap();
        assert!(backing.pending.lock().is_some());
        backing.finish();
        assert!(queued.wait_all().is_ok());
        device.suspend().unwrap();
        device.load_table(table(backing.clone(), true));
        let mut still_readonly = io_util::batch::IoBatch::with_capacity(1);
        write()
            .submit(device.as_ref(), &mut still_readonly)
            .unwrap();
        device.resume_suspended().unwrap();
        assert!(still_readonly.wait_all().is_err());
        assert!(backing.pending.lock().is_none());
        assert_eq!(
            write().submit_and_wait(device.as_ref()),
            Err(BioEnqueueError::Refused)
        );
        device.suspend().unwrap();
        device.load_table(table(backing.clone(), false));
        device.resume_suspended().unwrap();
        let mut writable = io_util::batch::IoBatch::with_capacity(1);
        write().submit(device.as_ref(), &mut writable).unwrap();
        backing.finish();
        assert!(writable.wait_all().is_ok());
        drop(manager);
        drop(device);
    }

    #[ktest]
    fn mapper_refuses_writes_to_readonly_active_table() {
        let (manager, device) = mapper();
        let backing = PendingBacking::new();
        device.load_table(table(backing.clone(), true));
        device.resume_initial().unwrap();
        let write = Bio::new(
            BioType::Write,
            Sid::new(0),
            vec![BioSegment::alloc_exact(1, 512, BioDirection::ToDevice)],
            None,
        );
        assert_eq!(
            write.submit_and_wait(device.as_ref()),
            Err(BioEnqueueError::Refused)
        );
        assert!(backing.pending.lock().is_none());
        let read = Bio::new(
            BioType::Read,
            Sid::new(0),
            vec![BioSegment::alloc_exact(1, 512, BioDirection::FromDevice)],
            None,
        );
        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        read.submit(device.as_ref(), &mut batch).unwrap();
        backing.finish();
        assert!(batch.wait_all().is_ok());
        let flush = Bio::new(BioType::Flush, Sid::new(0), vec![], None);
        flush.submit(device.as_ref(), &mut batch).unwrap();
        backing.finish();
        assert!(batch.wait_all().is_ok());
        drop(manager);
        drop(device);
    }
}
