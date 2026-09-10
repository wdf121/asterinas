// SPDX-License-Identifier: MPL-2.0

use alloc::{string::String, sync::Arc};
use core::{
    fmt::Debug,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use aster_block::{
    BlockDevice, BlockDeviceMeta,
    bio::{BioEnqueueError, SubmittedBio},
};
use device_id::DeviceId;
use ostd::sync::{Mutex, WaitQueue};

use crate::{DmError, DmTable, manager::DmDeviceIdOwner};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DmDevicePhase {
    Running,
    Suspending,
    Suspended,
}

struct DmIoState {
    in_flight: AtomicUsize,
    drained: WaitQueue,
}

impl DmIoState {
    fn new() -> Self {
        Self {
            in_flight: AtomicUsize::new(0),
            drained: WaitQueue::new(),
        }
    }

    fn finish(&self) {
        let previous = self.in_flight.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
        if previous == 1 {
            self.drained.wake_all();
        }
    }
}

struct DmDeviceState {
    active: Option<Arc<DmTable>>,
    inactive: Option<Arc<DmTable>>,
    phase: DmDevicePhase,
    event_nr: u32,
}

impl Default for DmDeviceState {
    fn default() -> Self {
        Self {
            active: None,
            inactive: None,
            phase: DmDevicePhase::Running,
            event_nr: 0,
        }
    }
}

/// A Device Mapper device status snapshot exposed to the control plane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmDeviceStatus {
    pub suspended: bool,
    pub readonly: bool,
    pub has_active_table: bool,
    pub has_inactive_table: bool,
    pub event_nr: u32,
}

/// Rolls back an initial table activation unless committed.
///
/// This guard is only for first activation after the primary block node is
/// registered. It lets the control plane publish the mapper alias after the
/// table becomes usable, while restoring the exact table and phase if alias
/// publication fails.
pub struct InitialResumeGuard<'a> {
    device: &'a DmDevice,
    table: Arc<DmTable>,
    previous_phase: DmDevicePhase,
    committed: bool,
}

impl InitialResumeGuard<'_> {
    /// Keeps the activated table and disables rollback.
    pub fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for InitialResumeGuard<'_> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }

        let mut state = self.device.state.lock();
        let active = state
            .active
            .take()
            .expect("initial resume guard lost its active table");
        assert!(Arc::ptr_eq(&active, &self.table));
        assert!(state.inactive.is_none());
        state.inactive = Some(active);
        state.phase = self.previous_phase;
    }
}

/// A runtime Device Mapper block device.
pub struct DmDevice {
    id_owner: DmDeviceIdOwner,
    name: Mutex<String>,
    uuid: Mutex<Option<String>>,
    readonly: AtomicBool,
    state: Mutex<DmDeviceState>,
    io: Arc<DmIoState>,
    events: WaitQueue,
}

impl DmDevice {
    pub(crate) fn new(
        id_owner: DmDeviceIdOwner,
        name: String,
        uuid: Option<String>,
        readonly: bool,
    ) -> Self {
        Self {
            id_owner,
            name: Mutex::new(name),
            uuid: Mutex::new(uuid),
            readonly: AtomicBool::new(readonly),
            state: Mutex::new(DmDeviceState::default()),
            io: Arc::new(DmIoState::new()),
            events: WaitQueue::new(),
        }
    }

    /// Waits until every BIO accepted by this device has completed.
    fn wait_for_io_drain(&self) {
        self.io
            .drained
            .wait_until(|| (self.io.in_flight.load(Ordering::Acquire) == 0).then_some(()));
    }

    /// Returns a cloned device name, including updates after rename.
    pub fn name(&self) -> String {
        self.name.lock().clone()
    }

    /// Renames the DM device.
    ///
    /// This only updates the device's internal name and does not affect I/O
    /// state. The control plane is responsible for synchronizing the manager
    /// index and block device registration.
    pub fn rename(&self, new_name: String) {
        let mut name = self.name.lock();
        *name = new_name;
    }

    /// Returns the Device Mapper UUID, or `None` if creation omitted it.
    pub fn uuid(&self) -> Option<String> {
        self.uuid.lock().clone()
    }

    /// Updates the DM device UUID.
    pub(crate) fn rename_uuid(&self, new_uuid: String) {
        let mut uuid = self.uuid.lock();
        *uuid = Some(new_uuid);
    }

    /// Returns whether the device is a read-only mapper.
    pub fn is_readonly(&self) -> bool {
        self.readonly.load(Ordering::Acquire)
    }

    /// Switches the device to a read-only mapper.
    pub fn set_readonly(&self) {
        self.readonly.store(true, Ordering::Release);
    }

    /// Installs a fully validated mapping table as the inactive table.
    pub fn load_table(&self, table: Arc<DmTable>) {
        let mut state = self.state.lock();
        state.inactive = Some(table);
    }

    /// Clears the inactive table.
    ///
    /// Linux DM treats clearing a missing inactive table as success, so this
    /// operation is idempotent for an empty inactive slot and does not change
    /// the event number.
    pub fn clear_inactive_table(&self) -> Result<(), DmError> {
        let mut state = self.state.lock();
        state.inactive.take();
        Ok(())
    }

    /// Suspends new I/O and waits until I/O submitted to the old table finishes.
    pub fn suspend(&self) -> Result<(), DmError> {
        {
            let mut state = self.state.lock();
            match state.phase {
                DmDevicePhase::Suspended => return Ok(()),
                DmDevicePhase::Suspending => return Err(DmError::InvalidState),
                DmDevicePhase::Running => {
                    state.phase = DmDevicePhase::Suspending;
                }
            }
        };

        self.wait_for_io_drain();

        let mut state = self.state.lock();
        debug_assert_eq!(state.phase, DmDevicePhase::Suspending);
        state.phase = DmDevicePhase::Suspended;
        Ok(())
    }

    /// Starts a rollback-capable first activation for mapper alias publication.
    ///
    /// The device must not already have an active table. If this guard is
    /// dropped before [`InitialResumeGuard::commit`], it restores the same
    /// table to inactive and restores the previous phase.
    pub fn begin_initial_resume(&self) -> Result<InitialResumeGuard<'_>, DmError> {
        let mut state = self.state.lock();
        if state.phase == DmDevicePhase::Suspending || state.active.is_some() {
            return Err(DmError::InvalidState);
        }
        let table = state.inactive.take().ok_or(DmError::InvalidState)?;
        let previous_phase = state.phase;
        state.active = Some(table.clone());
        if state.phase == DmDevicePhase::Suspended {
            state.phase = DmDevicePhase::Running;
        }

        Ok(InitialResumeGuard {
            device: self,
            table,
            previous_phase,
            committed: false,
        })
    }

    /// Activates the inactive table, if any, and resumes I/O.
    ///
    /// Replacing a running active table blocks new BIOs and waits for every BIO
    /// already accepted by the old table to complete. This creates a table
    /// generation barrier: after this function succeeds, newly accepted BIOs can
    /// only use the replacement table. Resuming without a replacement table keeps
    /// its existing idempotent or phase-restoration behavior.
    pub fn resume(&self) -> Result<(), DmError> {
        self.resume_with_drain(true)
    }

    /// Activates the inactive table, if any, without waiting for old BIOs.
    ///
    /// The old table remains alive through BIO-held `Arc` references while new
    /// BIOs begin using the replacement table immediately.
    pub fn resume_no_flush(&self) -> Result<(), DmError> {
        self.resume_with_drain(false)
    }

    fn resume_with_drain(&self, drain_replacement: bool) -> Result<(), DmError> {
        let replacement = {
            let mut state = self.state.lock();
            if state.phase == DmDevicePhase::Suspending {
                return Err(DmError::InvalidState);
            }
            if state.active.is_none() && state.inactive.is_none() {
                return Err(DmError::InvalidState);
            }

            if drain_replacement
                && state.phase == DmDevicePhase::Running
                && state.active.is_some()
                && state.inactive.is_some()
            {
                state.phase = DmDevicePhase::Suspending;
                state.inactive.take().expect("inactive table disappeared")
            } else {
                if let Some(table) = state.inactive.take() {
                    state.active = Some(table);
                }
                if state.phase == DmDevicePhase::Suspended {
                    state.phase = DmDevicePhase::Running;
                }
                return Ok(());
            }
        };

        self.wait_for_io_drain();

        let old_active = {
            let mut state = self.state.lock();
            debug_assert_eq!(state.phase, DmDevicePhase::Suspending);
            let old_active = state.active.replace(replacement);
            state.phase = DmDevicePhase::Running;
            old_active
        };
        drop(old_active);
        Ok(())
    }

    /// Returns an immutable snapshot of the active table.
    pub fn active_table(&self) -> Option<Arc<DmTable>> {
        self.state.lock().active.clone()
    }

    /// Returns an immutable snapshot of the inactive table.
    pub fn inactive_table(&self) -> Option<Arc<DmTable>> {
        self.state.lock().inactive.clone()
    }

    /// Waits until the event number differs from the given value and returns a
    /// new status snapshot.
    pub fn wait_event(&self, event_nr: u32) -> DmDeviceStatus {
        self.events.wait_until(|| {
            let status = self.status();
            (status.event_nr != event_nr).then_some(status)
        })
    }

    /// Returns the event wait queue so the control plane can choose
    /// signal-aware waiting.
    pub fn event_queue(&self) -> &WaitQueue {
        &self.events
    }

    /// Records one control-plane lifecycle event and wakes waiters.
    pub fn notify_event(&self) {
        {
            let mut state = self.state.lock();
            state.event_nr = state.event_nr.wrapping_add(1);
        }
        self.events.wake_all();
    }

    /// Returns the current status snapshot.
    pub fn status(&self) -> DmDeviceStatus {
        let state = self.state.lock();
        DmDeviceStatus {
            suspended: state.phase != DmDevicePhase::Running,
            readonly: self.is_readonly(),
            has_active_table: state.active.is_some(),
            has_inactive_table: state.inactive.is_some(),
            event_nr: state.event_nr,
        }
    }
}

impl BlockDevice for DmDevice {
    fn enqueue(&self, mut bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        let table = {
            let state = self.state.lock();
            if state.phase != DmDevicePhase::Running {
                return Err(BioEnqueueError::Refused);
            }
            if self.is_readonly() && bio.type_().is_write_like() {
                // Discard and write-zeroes are write-like because they can
                // change persistent contents even though they carry no data
                // segments.
                return Err(BioEnqueueError::Refused);
            }
            let table = state.active.clone().ok_or(BioEnqueueError::Refused)?;
            self.io.in_flight.fetch_add(1, Ordering::AcqRel);
            table
        };

        let io = self.io.clone();
        let table_for_completion = table.clone();
        bio.chain_complete_fn(move |_status| {
            // This `Arc` also keeps the replaced table and its backing leases alive
            // until the actual lower-level completion.
            let _table = table_for_completion;
            io.finish();
        });
        if let Err(error) = table.enqueue(bio) {
            self.io.finish();
            return Err(error);
        }
        Ok(())
    }

    fn metadata(&self) -> BlockDeviceMeta {
        self.active_table()
            .map(|table| table.metadata())
            .unwrap_or_default()
    }

    fn name(&self) -> String {
        self.name.lock().clone()
    }

    fn id(&self) -> DeviceId {
        self.id_owner.id()
    }
}

impl Debug for DmDevice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DmDevice")
            .field("id", &self.id())
            .field("name", &self.name)
            .field("uuid", &self.uuid)
            .field("status", &self.status())
            .finish()
    }
}

#[cfg(ktest)]
mod tests {
    use alloc::{string::ToString, vec};

    use aster_block::{
        BlockDeviceLease,
        bio::{Bio, BioDirection, BioSegment, BioStatus, BioType},
        id::Sid,
    };
    use device_id::{MajorId, MinorId};
    use ostd::{
        prelude::ktest,
        task::{Task, TaskOptions},
    };

    use super::*;
    use crate::DmManager;

    #[derive(Debug)]
    struct TestBlockDevice;

    #[derive(Debug)]
    struct DeferredBlockDevice {
        submitted: Mutex<Option<SubmittedBio>>,
    }

    impl DeferredBlockDevice {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                submitted: Mutex::new(None),
            })
        }

        fn has_submitted_bio(&self) -> bool {
            self.submitted.lock().is_some()
        }

        fn complete(&self) {
            self.submitted
                .lock()
                .take()
                .expect("deferred backing has no submitted BIO")
                .complete(BioStatus::Complete);
        }
    }

    impl BlockDevice for DeferredBlockDevice {
        fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            let mut submitted = self.submitted.lock();
            if submitted.is_some() {
                return Err(BioEnqueueError::IsFull);
            }
            *submitted = Some(bio);
            Ok(())
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 16,
                nr_sectors: 1_024,
            }
        }

        fn name(&self) -> String {
            String::from("dm-deferred-backing-test")
        }

        fn id(&self) -> DeviceId {
            DeviceId::new(MajorId::new(1), MinorId::new(2))
        }
    }

    impl BlockDevice for TestBlockDevice {
        fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            bio.complete(BioStatus::Complete);
            Ok(())
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 16,
                nr_sectors: 1_024,
            }
        }

        fn name(&self) -> String {
            String::from("dm-backing-test")
        }

        fn id(&self) -> DeviceId {
            DeviceId::new(MajorId::new(1), MinorId::new(1))
        }
    }

    fn create_device_and_table() -> (Arc<DmDevice>, Arc<DmTable>) {
        let manager = DmManager::new().unwrap();
        let device = manager.create("dm-test".to_string(), None, None).unwrap();
        let backing = Arc::new(TestBlockDevice) as Arc<dyn BlockDevice>;
        let table = Arc::new(
            DmTable::new_single_linear(
                Sid::new(0),
                128,
                Sid::new(16),
                BlockDeviceLease::new_untracked(backing),
            )
            .unwrap(),
        );
        (device, table)
    }

    #[ktest]
    fn wait_event_returns_when_event_has_already_changed() {
        let (device, _table) = create_device_and_table();

        device.notify_event();
        let status = device.wait_event(0);

        assert_eq!(status.event_nr, 1);
    }

    #[ktest]
    fn wait_event_blocks_until_event_changes() {
        let (device, _table) = create_device_and_table();
        let started = Arc::new(Mutex::new(false));
        let finished = Arc::new(Mutex::new(false));
        let observed = Arc::new(Mutex::new(None));

        {
            let device = device.clone();
            let started = started.clone();
            let finished = finished.clone();
            let observed = observed.clone();
            TaskOptions::new(move || {
                *started.lock() = true;
                let status = device.wait_event(0);
                *observed.lock() = Some(status.event_nr);
                *finished.lock() = true;
            })
            .spawn()
            .unwrap();
        }

        while !*started.lock() {
            Task::yield_now();
        }
        Task::yield_now();
        assert!(!*finished.lock());

        device.notify_event();
        while !*finished.lock() {
            Task::yield_now();
        }

        assert_eq!(*observed.lock(), Some(1));
    }

    #[ktest]
    fn enforces_suspend_load_resume_state_machine() {
        let (device, table) = create_device_and_table();

        assert_eq!(
            device.status(),
            DmDeviceStatus {
                suspended: false,
                readonly: false,
                has_active_table: false,
                has_inactive_table: false,
                event_nr: 0,
            }
        );
        assert_eq!(device.resume(), Err(DmError::InvalidState));
        device.suspend().unwrap();

        device.load_table(table.clone());
        assert_eq!(device.status().event_nr, 0);
        assert!(device.inactive_table().is_some());
        device.resume().unwrap();
        assert_eq!(device.status().event_nr, 0);
        assert!(!device.status().suspended);
        assert!(device.active_table().is_some());
        assert!(device.inactive_table().is_none());
        assert_eq!(device.metadata().max_nr_segments_per_bio, 16);
        assert_eq!(device.metadata().nr_sectors, 128);

        device.resume().unwrap();
        assert_eq!(device.status().event_nr, 0);

        device.suspend().unwrap();
        device.load_table(table);
        device.clear_inactive_table().unwrap();
        assert_eq!(device.status().event_nr, 0);
        device.clear_inactive_table().unwrap();
        assert_eq!(device.status().event_nr, 0);
    }

    #[ktest]
    fn initial_resume_guard_restores_running_device_table() {
        let (device, table) = create_device_and_table();
        device.load_table(table.clone());

        {
            let _guard = device.begin_initial_resume().unwrap();
            assert!(Arc::ptr_eq(&device.active_table().unwrap(), &table));
            assert!(device.inactive_table().is_none());
            assert!(!device.status().suspended);
        }

        assert!(device.active_table().is_none());
        assert!(Arc::ptr_eq(&device.inactive_table().unwrap(), &table));
        assert!(!device.status().suspended);
    }

    #[ktest]
    fn initial_resume_guard_restores_suspended_device_phase() {
        let (device, table) = create_device_and_table();
        device.suspend().unwrap();
        device.load_table(table.clone());

        {
            let _guard = device.begin_initial_resume().unwrap();
            assert!(Arc::ptr_eq(&device.active_table().unwrap(), &table));
            assert!(!device.status().suspended);
        }

        assert!(device.active_table().is_none());
        assert!(Arc::ptr_eq(&device.inactive_table().unwrap(), &table));
        assert!(device.status().suspended);
    }

    #[ktest]
    fn initial_resume_guard_commit_keeps_active_table() {
        let (device, table) = create_device_and_table();
        device.load_table(table.clone());

        device.begin_initial_resume().unwrap().commit();

        assert!(Arc::ptr_eq(&device.active_table().unwrap(), &table));
        assert!(device.inactive_table().is_none());
        assert!(!device.status().suspended);
    }

    #[ktest]
    fn running_resume_replaces_active_table() {
        let (device, first) = create_device_and_table();
        let backing = Arc::new(TestBlockDevice) as Arc<dyn BlockDevice>;
        let replacement = Arc::new(
            DmTable::new_single_linear(
                Sid::new(0),
                64,
                Sid::new(32),
                BlockDeviceLease::new_untracked(backing),
            )
            .unwrap(),
        );

        device.load_table(first);
        device.resume().unwrap();
        device.load_table(replacement.clone());
        device.resume().unwrap();

        assert_eq!(
            device.active_table().unwrap().length(),
            replacement.length()
        );
        assert!(device.inactive_table().is_none());
        assert_eq!(device.status().event_nr, 0);
    }

    #[ktest]
    fn suspend_waits_for_submitted_io_and_blocks_new_io() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-deferred-test".to_string(), None, None)
            .unwrap();
        let backing = DeferredBlockDevice::new();
        let table = Arc::new(
            DmTable::new_single_linear(
                Sid::new(0),
                128,
                Sid::new(16),
                BlockDeviceLease::new_untracked(backing.clone()),
            )
            .unwrap(),
        );
        device.load_table(table);
        device.resume().unwrap();

        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        Bio::new(BioType::Flush, Sid::new(0), vec![], None)
            .submit(device.as_ref(), &mut batch)
            .unwrap();
        assert!(backing.has_submitted_bio());

        let suspend_finished = Arc::new(Mutex::new(false));
        {
            let device = device.clone();
            let suspend_finished = suspend_finished.clone();
            TaskOptions::new(move || {
                device.suspend().unwrap();
                *suspend_finished.lock() = true;
            })
            .spawn()
            .unwrap();
        }
        while !device.status().suspended {
            Task::yield_now();
        }
        assert!(!*suspend_finished.lock());
        let mut refused_batch = io_util::batch::IoBatch::with_capacity(1);
        assert_eq!(
            Bio::new(BioType::Flush, Sid::new(0), vec![], None)
                .submit(device.as_ref(), &mut refused_batch),
            Err(BioEnqueueError::Refused)
        );

        backing.complete();
        while !*suspend_finished.lock() {
            Task::yield_now();
        }
        assert!(*suspend_finished.lock());
    }

    #[ktest]
    fn readonly_device_refuses_write_like_bios_but_allows_read_and_flush() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create_with_readonly("dm-readonly-test".to_string(), None, None, true)
            .unwrap();
        let backing = Arc::new(TestBlockDevice) as Arc<dyn BlockDevice>;
        let table = Arc::new(
            DmTable::new_single_linear(
                Sid::new(0),
                128,
                Sid::new(16),
                BlockDeviceLease::new_untracked(backing),
            )
            .unwrap(),
        );
        device.load_table(table);
        device.resume().unwrap();

        assert!(device.status().readonly);
        assert_eq!(
            Bio::new(
                BioType::Read,
                Sid::new(0),
                vec![BioSegment::alloc(1, BioDirection::FromDevice)],
                None,
            )
            .submit_and_wait(device.as_ref())
            .unwrap(),
            BioStatus::Complete
        );
        assert_eq!(
            Bio::new(BioType::Flush, Sid::new(0), vec![], None)
                .submit_and_wait(device.as_ref())
                .unwrap(),
            BioStatus::Complete
        );
        assert_eq!(
            Bio::new(
                BioType::Write,
                Sid::new(0),
                vec![BioSegment::alloc(1, BioDirection::ToDevice)],
                None,
            )
            .submit_and_wait(device.as_ref()),
            Err(BioEnqueueError::Refused)
        );
        assert_eq!(
            Bio::new_range(BioType::Discard, Sid::new(0), 8, None).submit_and_wait(device.as_ref()),
            Err(BioEnqueueError::Refused)
        );
        assert_eq!(
            Bio::new_range(BioType::WriteZeroes, Sid::new(0), 8, None)
                .submit_and_wait(device.as_ref()),
            Err(BioEnqueueError::Refused)
        );
    }
}
