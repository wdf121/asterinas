// SPDX-License-Identifier: MPL-2.0

use alloc::{collections::VecDeque, format, string::String, sync::Arc};
use core::{
    fmt::{Debug, Display, Formatter},
    sync::atomic::{AtomicUsize, Ordering},
};

use aster_block::{
    BlockDevice, BlockDeviceMeta,
    bio::{BioEnqueueError, BioStatus, SubmittedBio},
};
use device_id::DeviceId;
use ostd::sync::{Mutex, MutexGuard, WaitQueue};

use crate::{DmError, DmTable, manager::DmDeviceIdOwner};

/// Formats one mapper block-device identity as its Linux-visible major:minor pair.
struct DeviceIdLogLabel(DeviceId);

impl Display for DeviceIdLogLabel {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "{}:{}",
            self.0.major().get(),
            self.0.minor().get()
        )
    }
}

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
    // BIOs accepted while mapping is suspended, before any table generation
    // has been selected for them. Resume replays them through its new active table.
    postponed: VecDeque<SubmittedBio>,
    phase: DmDevicePhase,
    event_nr: u32,
}

impl Default for DmDeviceState {
    fn default() -> Self {
        Self {
            active: None,
            inactive: None,
            postponed: VecDeque::new(),
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

/// Holds the initial activation state private until alias publication commits.
///
/// The first primary node is already openable before its mapper alias exists.
/// Keeping `state` locked prevents BIO admission from selecting the inactive
/// table while the control plane publishes that alias. Dropping the guard leaves
/// the table and phase untouched, so a publication failure is retryable.
pub struct InitialResumeGuard<'a> {
    device: &'a DmDevice,
    state: Option<MutexGuard<'a, DmDeviceState>>,
}

impl InitialResumeGuard<'_> {
    /// Atomically exposes the first active table and reserves postponed BIO replay.
    pub fn commit(mut self) {
        let replay = {
            let state = self
                .state
                .as_mut()
                .expect("initial resume guard lost its state lock");
            debug_assert!(state.active.is_none());
            let table = state
                .inactive
                .take()
                .expect("initial resume guard lost its inactive table");
            state.active = Some(table);
            state.phase = DmDevicePhase::Running;
            self.device.take_postponed_for_replay(state)
        };
        let replay_count = replay.as_ref().map_or(0, |(_, bios)| bios.len());
        drop(self.state.take());
        ostd::error!(
            "[dm-debug] state initial-resume committed name={} dev={} replay_postponed={}",
            self.device.mapper_name(),
            DeviceIdLogLabel(self.device.id()),
            replay_count
        );
        self.device.replay_postponed(replay);
    }
}

/// A runtime Device Mapper block device.
///
/// Its [`BlockDevice`] identity is the immutable `dm-<minor>` primary name.
/// The separately locked mapper alias is used only by the control plane for
/// manager indexes, UUID resolution, and `/dev/mapper/<alias>` publication.
pub struct DmDevice {
    id_owner: DmDeviceIdOwner,
    primary_name: String,
    mapper_name: Mutex<String>,
    uuid: Mutex<Option<String>>,
    lifecycle: Mutex<()>,
    state: Mutex<DmDeviceState>,
    io: Arc<DmIoState>,
    events: WaitQueue,
}

impl DmDevice {
    pub(crate) fn new(
        id_owner: DmDeviceIdOwner,
        mapper_name: String,
        uuid: Option<String>,
    ) -> Self {
        let primary_name = format!("dm-{}", id_owner.id().minor().get());
        Self {
            id_owner,
            primary_name,
            mapper_name: Mutex::new(mapper_name),
            uuid: Mutex::new(uuid),
            lifecycle: Mutex::new(()),
            state: Mutex::new(DmDeviceState::default()),
            io: Arc::new(DmIoState::new()),
            events: WaitQueue::new(),
        }
    }

    /// Serializes control-plane operations that change this mapper's lifecycle.
    ///
    /// The guard may be held while an operation drains assigned BIOs. It must be
    /// acquired before `state`, and never while a global manager lock is held.
    pub fn lock_lifecycle(&self) -> MutexGuard<'_, ()> {
        self.lifecycle.lock()
    }

    /// Waits until every BIO accepted by this device has completed.
    fn wait_for_io_drain(&self) {
        self.io
            .drained
            .wait_until(|| (self.io.in_flight.load(Ordering::Acquire) == 0).then_some(()));
    }

    /// Takes postponed BIOs after the caller has selected a running active table.
    ///
    /// Reserving all completions while holding `state` prevents a following
    /// suspend from observing zero in-flight BIOs before replay begins.
    fn take_postponed_for_replay(
        &self,
        state: &mut DmDeviceState,
    ) -> Option<(Arc<DmTable>, VecDeque<SubmittedBio>)> {
        let table = state.active.clone()?;
        let postponed = core::mem::take(&mut state.postponed);
        self.io
            .in_flight
            .fetch_add(postponed.len(), Ordering::AcqRel);
        Some((table, postponed))
    }

    /// Dispatches a BIO that has already been assigned to one table generation.
    fn dispatch_assigned_bio(
        &self,
        table: Arc<DmTable>,
        mut bio: SubmittedBio,
        deferred_replay: bool,
    ) -> Result<(), BioEnqueueError> {
        if deferred_replay {
            bio.complete_as_io_error_on_drop();
        }

        let io = self.io.clone();
        let table_for_completion = table.clone();
        bio.chain_complete_fn(move |_status| {
            // This `Arc` keeps a replaced table and its backing leases alive
            // until the lower-level BIO has completed.
            let _table = table_for_completion;
            io.finish();
        });
        if let Err(error) = table.enqueue(bio) {
            if deferred_replay {
                // Deferred BIOs have already been accepted. Their drop guard
                // completes them with I/O error and releases `in_flight`.
                return Ok(());
            }
            self.io.finish();
            return Err(error);
        }
        Ok(())
    }

    /// Replays postponed BIOs in submission order through the current active table.
    fn replay_postponed(&self, replay: Option<(Arc<DmTable>, VecDeque<SubmittedBio>)>) {
        let Some((table, postponed)) = replay else {
            return;
        };
        for bio in postponed {
            if table.is_readonly() && bio.type_().is_write_like() {
                bio.complete(BioStatus::IoError);
                self.io.finish();
                continue;
            }
            self.dispatch_assigned_bio(table.clone(), bio, true)
                .expect("deferred replay must complete enqueue failures internally");
        }
    }

    /// Completes BIOs that were accepted while suspended but cannot be replayed.
    pub fn fail_postponed_bios(&self) {
        let postponed = core::mem::take(&mut self.state.lock().postponed);
        let count = postponed.len();
        for bio in postponed {
            bio.complete(BioStatus::IoError);
        }
        if count != 0 {
            ostd::error!(
                "[dm-debug] state postponed-bios failed name={} dev={} count={}",
                self.mapper_name(),
                DeviceIdLogLabel(self.id()),
                count
            );
        }
    }

    /// Returns a cloned snapshot of the mutable mapper alias.
    pub fn mapper_name(&self) -> String {
        self.mapper_name.lock().clone()
    }

    /// Updates the mutable mapper alias without changing the stable primary name.
    ///
    /// The control plane synchronizes manager indexes and devtmpfs aliases before
    /// calling this method, so data-plane block device identity remains immutable.
    pub fn rename_mapper(&self, new_name: String) {
        *self.mapper_name.lock() = new_name;
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

    /// Returns whether the current active table is read-only.
    pub fn is_readonly(&self) -> bool {
        self.status().readonly
    }

    /// Captures one selected table generation together with its Linux-visible status.
    ///
    /// The table and its read-only mode must come from the same state-lock snapshot,
    /// so table replacement cannot expose one generation with another generation's mode.
    pub fn table_snapshot(&self, inactive: bool) -> (DmDeviceStatus, Option<Arc<DmTable>>) {
        let state = self.state.lock();
        let table = if inactive {
            state.inactive.clone()
        } else {
            state.active.clone()
        };
        let status = DmDeviceStatus {
            suspended: state.phase != DmDevicePhase::Running,
            readonly: table.as_ref().is_some_and(|table| table.is_readonly()),
            has_active_table: state.active.is_some(),
            has_inactive_table: state.inactive.is_some(),
            event_nr: state.event_nr,
        };
        (status, table)
    }

    /// Installs a fully validated mapping table as the inactive table.
    pub fn load_table(&self, table: Arc<DmTable>) {
        let target_count = table.target_count();
        let capacity = table.length();
        self.state.lock().inactive = Some(table);
        ostd::error!(
            "[dm-debug] state table-loaded name={} dev={} slot=inactive targets={} sectors={}",
            self.mapper_name(),
            DeviceIdLogLabel(self.id()),
            target_count,
            capacity
        );
    }

    /// Clears the inactive table.
    ///
    /// Linux DM treats clearing a missing inactive table as success, so this
    /// operation is idempotent for an empty inactive slot and does not change
    /// the event number.
    pub fn clear_inactive_table(&self) -> Result<(), DmError> {
        let cleared = self.state.lock().inactive.take().is_some();
        ostd::error!(
            "[dm-debug] state table-cleared name={} dev={} cleared={}",
            self.mapper_name(),
            DeviceIdLogLabel(self.id()),
            cleared
        );
        Ok(())
    }

    /// Suspends new mapping I/O after every assigned BIO has completed.
    pub fn suspend(&self) -> Result<(), DmError> {
        {
            let mut state = self.state.lock();
            match state.phase {
                DmDevicePhase::Suspended => return Ok(()),
                DmDevicePhase::Suspending => return Err(DmError::InvalidState),
                DmDevicePhase::Running => state.phase = DmDevicePhase::Suspending,
            }
        }

        self.wait_for_io_drain();

        let mut state = self.state.lock();
        debug_assert_eq!(state.phase, DmDevicePhase::Suspending);
        state.phase = DmDevicePhase::Suspended;
        drop(state);
        ostd::error!(
            "[dm-debug] state suspended name={} dev={} mode=flush",
            self.mapper_name(),
            DeviceIdLogLabel(self.id())
        );
        Ok(())
    }

    /// Suspends new mapping I/O without draining already assigned BIOs.
    ///
    /// This is the `DM_NOFLUSH_FLAG` suspend policy. Old BIOs retain their
    /// assigned table through completion-held `Arc` references, while later
    /// BIOs are postponed until a resume selects the next active table.
    pub fn suspend_no_flush(&self) -> Result<(), DmError> {
        let mut state = self.state.lock();
        match state.phase {
            DmDevicePhase::Suspended => Ok(()),
            DmDevicePhase::Suspending => Err(DmError::InvalidState),
            DmDevicePhase::Running => {
                state.phase = DmDevicePhase::Suspended;
                drop(state);
                ostd::error!(
                    "[dm-debug] state suspended name={} dev={} mode=noflush",
                    self.mapper_name(),
                    DeviceIdLogLabel(self.id())
                );
                Ok(())
            }
        }
    }

    /// Starts a first activation transaction while keeping table state private.
    ///
    /// The device must have no active table and must not be draining. The returned
    /// guard retains the state lock until [`InitialResumeGuard::commit`] installs
    /// the inactive table as active. Dropping it leaves the table and phase intact.
    pub fn begin_initial_resume(&self) -> Result<InitialResumeGuard<'_>, DmError> {
        let state = self.state.lock();
        if state.phase == DmDevicePhase::Suspending
            || state.active.is_some()
            || state.inactive.is_none()
        {
            return Err(DmError::InvalidState);
        }

        Ok(InitialResumeGuard {
            device: self,
            state: Some(state),
        })
    }

    /// Activates the inactive table, if any, resumes mapping I/O, and replays postponed BIOs.
    ///
    /// A direct running replacement establishes a generation barrier by draining
    /// the old table first. A suspended no-flush replacement does not repeat that
    /// drain: old BIOs retain their table while postponed BIOs use the new table.
    pub fn resume(&self) -> Result<(), DmError> {
        let (replacement, replay) = {
            let mut state = self.state.lock();
            if state.phase == DmDevicePhase::Suspending {
                return Err(DmError::InvalidState);
            }
            if state.active.is_none() && state.inactive.is_none() {
                return Err(DmError::InvalidState);
            }

            if state.phase == DmDevicePhase::Running
                && state.active.is_some()
                && state.inactive.is_some()
            {
                state.phase = DmDevicePhase::Suspending;
                (
                    Some(state.inactive.take().expect("inactive table disappeared")),
                    None,
                )
            } else {
                if let Some(table) = state.inactive.take() {
                    state.active = Some(table);
                }
                state.phase = DmDevicePhase::Running;
                (None, self.take_postponed_for_replay(&mut state))
            }
        };

        let replaced_active = replacement.is_some();
        let replay = if let Some(replacement) = replacement {
            self.wait_for_io_drain();

            let old_active = {
                let mut state = self.state.lock();
                debug_assert_eq!(state.phase, DmDevicePhase::Suspending);
                let old_active = state.active.replace(replacement);
                state.phase = DmDevicePhase::Running;
                let replay = self.take_postponed_for_replay(&mut state);
                (old_active, replay)
            };
            drop(old_active.0);
            old_active.1
        } else {
            replay
        };
        let replay_count = replay.as_ref().map_or(0, |(_, bios)| bios.len());
        self.replay_postponed(replay);
        ostd::error!(
            "[dm-debug] state resumed name={} dev={} replaced_active={} replay_postponed={}",
            self.mapper_name(),
            DeviceIdLogLabel(self.id()),
            replaced_active,
            replay_count
        );
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
        let event_nr = {
            let mut state = self.state.lock();
            state.event_nr = state.event_nr.wrapping_add(1);
            state.event_nr
        };
        ostd::error!(
            "[dm-debug] state event-published name={} dev={} event_nr={}",
            self.mapper_name(),
            DeviceIdLogLabel(self.id()),
            event_nr
        );
        self.events.wake_all();
    }

    /// Returns the Linux-visible status for the active table.
    pub fn status(&self) -> DmDeviceStatus {
        self.table_snapshot(false).0
    }
}

impl BlockDevice for DmDevice {
    fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        let table = {
            let mut state = self.state.lock();
            match state.phase {
                DmDevicePhase::Running => {
                    let table = state.active.clone().ok_or(BioEnqueueError::Refused)?;
                    if table.is_readonly() && bio.type_().is_write_like() {
                        // Discard and write-zeroes can change persistent contents without data segments.
                        return Err(BioEnqueueError::Refused);
                    }
                    self.io.in_flight.fetch_add(1, Ordering::AcqRel);
                    table
                }
                DmDevicePhase::Suspending | DmDevicePhase::Suspended => {
                    if state
                        .active
                        .as_ref()
                        .is_some_and(|table| table.is_readonly())
                        && bio.type_().is_write_like()
                    {
                        return Err(BioEnqueueError::Refused);
                    }
                    state.postponed.push_back(bio);
                    return Ok(());
                }
            }
        };

        self.dispatch_assigned_bio(table, bio, false)
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

impl Debug for DmDevice {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DmDevice")
            .field("id", &self.id())
            .field("primary_name", &self.primary_name)
            .field("mapper_name", &self.mapper_name)
            .field("uuid", &self.uuid)
            .field("status", &self.status())
            .finish()
    }
}

#[cfg(ktest)]
mod tests {
    use alloc::{boxed::Box, string::ToString, vec};

    use aster_block::{
        BlockDeviceLease, allocate_major,
        bio::{Bio, BioDirection, BioSegment, BioType},
        id::Sid,
        lookup, lookup_lease, register, unregister,
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
    struct RejectingBlockDevice;

    #[derive(Debug)]
    struct DeferredBlockDevice {
        id: DeviceId,
        submitted: Mutex<Option<SubmittedBio>>,
    }

    impl DeferredBlockDevice {
        fn new() -> Arc<Self> {
            Self::with_id(DeviceId::new(MajorId::new(1), MinorId::new(2)))
        }

        fn with_id(id: DeviceId) -> Arc<Self> {
            Arc::new(Self {
                id,
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

        fn name(&self) -> &str {
            "dm-deferred-backing-test"
        }

        fn id(&self) -> DeviceId {
            self.id
        }
    }

    impl BlockDevice for RejectingBlockDevice {
        fn enqueue(&self, _bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            Err(BioEnqueueError::Refused)
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 16,
                nr_sectors: 1_024,
            }
        }

        fn name(&self) -> &str {
            "dm-rejecting-backing-test"
        }

        fn id(&self) -> DeviceId {
            DeviceId::new(MajorId::new(1), MinorId::new(3))
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

        fn name(&self) -> &str {
            "dm-backing-test"
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
    fn initial_resume_guard_drop_keeps_running_device_table_inactive() {
        let (device, table) = create_device_and_table();
        device.load_table(table.clone());

        {
            let _guard = device.begin_initial_resume().unwrap();
        }

        assert!(device.active_table().is_none());
        assert!(Arc::ptr_eq(&device.inactive_table().unwrap(), &table));
        assert!(!device.status().suspended);
    }

    #[ktest]
    fn initial_resume_guard_drop_keeps_suspended_device_phase_and_table_inactive() {
        let (device, table) = create_device_and_table();
        device.suspend().unwrap();
        device.load_table(table.clone());

        {
            let _guard = device.begin_initial_resume().unwrap();
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
    fn initial_resume_guard_blocks_bios_until_commit() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-initial-resume-commit-test".to_string(), None, None)
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
        let guard = device.begin_initial_resume().unwrap();
        let started = Arc::new(Mutex::new(false));
        let accepted = Arc::new(Mutex::new(None));
        let finished = Arc::new(Mutex::new(false));

        {
            let device = device.clone();
            let started = started.clone();
            let accepted = accepted.clone();
            let finished = finished.clone();
            TaskOptions::new(move || {
                *started.lock() = true;
                let mut batch = io_util::batch::IoBatch::with_capacity(1);
                let result = Bio::new(BioType::Flush, Sid::new(0), vec![], None)
                    .submit(device.as_ref(), &mut batch);
                *accepted.lock() = Some(result.is_ok());
                if result.is_ok() {
                    batch.wait_all().unwrap();
                }
                *finished.lock() = true;
            })
            .spawn()
            .unwrap();
        }

        while !*started.lock() {
            Task::yield_now();
        }
        Task::yield_now();
        assert!(!backing.has_submitted_bio());
        assert_eq!(device.io.in_flight.load(Ordering::Acquire), 0);

        guard.commit();
        while accepted.lock().is_none() {
            Task::yield_now();
        }
        assert_eq!(*accepted.lock(), Some(true));
        assert!(backing.has_submitted_bio());

        backing.complete();
        while !*finished.lock() {
            Task::yield_now();
        }
        assert_eq!(device.io.in_flight.load(Ordering::Acquire), 0);
    }

    #[ktest]
    fn initial_resume_guard_drop_refuses_blocked_bios_without_dispatch() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-initial-resume-failure-test".to_string(), None, None)
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
        device.load_table(table.clone());
        let guard = device.begin_initial_resume().unwrap();
        let started = Arc::new(Mutex::new(false));
        let accepted = Arc::new(Mutex::new(None));

        {
            let device = device.clone();
            let started = started.clone();
            let accepted = accepted.clone();
            TaskOptions::new(move || {
                *started.lock() = true;
                let mut batch = io_util::batch::IoBatch::with_capacity(1);
                let result = Bio::new(BioType::Flush, Sid::new(0), vec![], None)
                    .submit(device.as_ref(), &mut batch);
                *accepted.lock() = Some(result.is_ok());
            })
            .spawn()
            .unwrap();
        }

        while !*started.lock() {
            Task::yield_now();
        }
        Task::yield_now();
        assert!(!backing.has_submitted_bio());
        assert_eq!(device.io.in_flight.load(Ordering::Acquire), 0);

        drop(guard);
        while accepted.lock().is_none() {
            Task::yield_now();
        }
        assert_eq!(*accepted.lock(), Some(false));
        assert!(device.active_table().is_none());
        assert!(Arc::ptr_eq(&device.inactive_table().unwrap(), &table));
        assert!(!backing.has_submitted_bio());
        assert_eq!(device.io.in_flight.load(Ordering::Acquire), 0);
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
    fn suspend_drains_old_bios_and_replays_postponed_bios_after_resume() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-suspend-postpone-test".to_string(), None, None)
            .unwrap();
        let old_backing = DeferredBlockDevice::new();
        let replacement_backing = DeferredBlockDevice::new();
        let first = Arc::new(
            DmTable::new_single_linear(
                Sid::new(0),
                128,
                Sid::new(16),
                BlockDeviceLease::new_untracked(old_backing.clone()),
            )
            .unwrap(),
        );
        let replacement = Arc::new(
            DmTable::new_single_linear(
                Sid::new(0),
                128,
                Sid::new(32),
                BlockDeviceLease::new_untracked(replacement_backing.clone()),
            )
            .unwrap(),
        );
        device.load_table(first);
        device.resume().unwrap();

        let mut old_batch = io_util::batch::IoBatch::with_capacity(1);
        Bio::new(BioType::Flush, Sid::new(0), vec![], None)
            .submit(device.as_ref(), &mut old_batch)
            .unwrap();
        assert!(old_backing.has_submitted_bio());

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

        let mut postponed_batch = io_util::batch::IoBatch::with_capacity(1);
        Bio::new(BioType::Flush, Sid::new(0), vec![], None)
            .submit(device.as_ref(), &mut postponed_batch)
            .unwrap();
        assert_eq!(device.state.lock().postponed.len(), 1);
        assert!(!replacement_backing.has_submitted_bio());

        old_backing.complete();
        while !*suspend_finished.lock() {
            Task::yield_now();
        }
        device.load_table(replacement);
        device.resume().unwrap();
        assert!(replacement_backing.has_submitted_bio());
        replacement_backing.complete();
        while device.io.in_flight.load(Ordering::Acquire) != 0 {
            Task::yield_now();
        }
    }

    #[ktest]
    fn running_resume_waits_for_old_bios_before_replacing_active_table() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-resume-drain-test".to_string(), None, None)
            .unwrap();
        let old_backing = DeferredBlockDevice::new();
        let first = Arc::new(
            DmTable::new_single_linear(
                Sid::new(0),
                128,
                Sid::new(16),
                BlockDeviceLease::new_untracked(old_backing.clone()),
            )
            .unwrap(),
        );
        let replacement = Arc::new(
            DmTable::new_single_linear(
                Sid::new(0),
                64,
                Sid::new(32),
                BlockDeviceLease::new_untracked(Arc::new(TestBlockDevice)),
            )
            .unwrap(),
        );
        device.load_table(first.clone());
        device.resume().unwrap();

        let mut old_batch = io_util::batch::IoBatch::with_capacity(1);
        Bio::new(BioType::Flush, Sid::new(0), vec![], None)
            .submit(device.as_ref(), &mut old_batch)
            .unwrap();
        assert!(old_backing.has_submitted_bio());
        assert_eq!(device.io.in_flight.load(Ordering::Acquire), 1);

        device.load_table(replacement.clone());
        let resume_finished = Arc::new(Mutex::new(false));
        {
            let device = device.clone();
            let resume_finished = resume_finished.clone();
            TaskOptions::new(move || {
                device.resume().unwrap();
                *resume_finished.lock() = true;
            })
            .spawn()
            .unwrap();
        }

        while !device.status().suspended {
            Task::yield_now();
        }
        assert!(!*resume_finished.lock());
        assert!(Arc::ptr_eq(&device.active_table().unwrap(), &first));
        assert!(device.inactive_table().is_none());
        let mut postponed_batch = io_util::batch::IoBatch::with_capacity(1);
        Bio::new(BioType::Flush, Sid::new(0), vec![], None)
            .submit(device.as_ref(), &mut postponed_batch)
            .unwrap();
        assert_eq!(device.state.lock().postponed.len(), 1);

        old_backing.complete();
        while !*resume_finished.lock() {
            Task::yield_now();
        }
        assert!(Arc::ptr_eq(&device.active_table().unwrap(), &replacement));
        assert!(!device.status().suspended);
        assert_eq!(device.io.in_flight.load(Ordering::Acquire), 0);
    }

    #[ktest]
    fn suspend_no_flush_replays_postponed_bios_without_waiting_for_old_bios() {
        let major_owner = allocate_major().unwrap();
        let old_id = DeviceId::new(major_owner.get(), MinorId::new(1));
        let replacement_id = DeviceId::new(major_owner.get(), MinorId::new(2));
        let old_backing = DeferredBlockDevice::with_id(old_id);
        let replacement_backing = DeferredBlockDevice::with_id(replacement_id);
        register(old_backing.clone() as Arc<dyn BlockDevice>).unwrap();
        register(replacement_backing.clone() as Arc<dyn BlockDevice>).unwrap();

        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-suspend-noflush-test".to_string(), None, None)
            .unwrap();
        let first = Arc::new(
            DmTable::new_single_linear(
                Sid::new(0),
                128,
                Sid::new(16),
                lookup_lease(old_id).unwrap(),
            )
            .unwrap(),
        );
        let replacement = Arc::new(
            DmTable::new_single_linear(
                Sid::new(0),
                64,
                Sid::new(32),
                lookup_lease(replacement_id).unwrap(),
            )
            .unwrap(),
        );
        device.load_table(first);
        device.resume().unwrap();

        let old_completions = Arc::new(AtomicUsize::new(0));
        let old_completions_for_callback = old_completions.clone();
        let mut old_batch = io_util::batch::IoBatch::with_capacity(1);
        Bio::new(
            BioType::Flush,
            Sid::new(0),
            vec![],
            Some(Box::new(move |status| {
                assert_eq!(status, BioStatus::Complete);
                old_completions_for_callback.fetch_add(1, Ordering::AcqRel);
            })),
        )
        .submit(device.as_ref(), &mut old_batch)
        .unwrap();
        assert!(old_backing.has_submitted_bio());
        assert_eq!(device.io.in_flight.load(Ordering::Acquire), 1);

        device.suspend_no_flush().unwrap();
        assert!(device.status().suspended);
        assert_eq!(device.io.in_flight.load(Ordering::Acquire), 1);

        let replacement_completions = Arc::new(AtomicUsize::new(0));
        let replacement_completions_for_callback = replacement_completions.clone();
        let mut postponed_batch = io_util::batch::IoBatch::with_capacity(1);
        Bio::new(
            BioType::Flush,
            Sid::new(0),
            vec![],
            Some(Box::new(move |status| {
                assert_eq!(status, BioStatus::Complete);
                replacement_completions_for_callback.fetch_add(1, Ordering::AcqRel);
            })),
        )
        .submit(device.as_ref(), &mut postponed_batch)
        .unwrap();
        assert_eq!(device.state.lock().postponed.len(), 1);
        assert!(!replacement_backing.has_submitted_bio());

        device.load_table(replacement.clone());
        device.resume().unwrap();

        assert!(Arc::ptr_eq(&device.active_table().unwrap(), &replacement));
        drop(replacement);
        assert!(!device.status().suspended);
        assert!(old_backing.has_submitted_bio());
        assert!(replacement_backing.has_submitted_bio());
        assert_eq!(device.io.in_flight.load(Ordering::Acquire), 2);
        assert_eq!(old_completions.load(Ordering::Acquire), 0);
        assert_eq!(replacement_completions.load(Ordering::Acquire), 0);
        assert_eq!(unregister(old_id).unwrap_err(), aster_block::Error::Busy);
        assert_eq!(
            unregister(replacement_id).unwrap_err(),
            aster_block::Error::Busy
        );

        replacement_backing.complete();
        postponed_batch.wait_all().unwrap();
        assert_eq!(replacement_completions.load(Ordering::Acquire), 1);
        assert_eq!(old_completions.load(Ordering::Acquire), 0);
        assert_eq!(device.io.in_flight.load(Ordering::Acquire), 1);
        assert!(!replacement_backing.has_submitted_bio());
        assert!(old_backing.has_submitted_bio());
        assert_eq!(unregister(old_id).unwrap_err(), aster_block::Error::Busy);
        assert_eq!(
            unregister(replacement_id).unwrap_err(),
            aster_block::Error::Busy
        );

        old_backing.complete();
        old_batch.wait_all().unwrap();
        assert_eq!(old_completions.load(Ordering::Acquire), 1);
        assert_eq!(replacement_completions.load(Ordering::Acquire), 1);
        assert_eq!(device.io.in_flight.load(Ordering::Acquire), 0);
        assert!(!old_backing.has_submitted_bio());
        assert!(!replacement_backing.has_submitted_bio());
        assert_eq!(device.state.lock().postponed.len(), 0);

        drop(unregister(old_id).unwrap());
        assert!(lookup(old_id).is_none());
        assert_eq!(
            unregister(replacement_id).unwrap_err(),
            aster_block::Error::Busy
        );

        drop(device);
        drop(manager);
        drop(unregister(replacement_id).unwrap());
        assert!(lookup(replacement_id).is_none());
        assert_eq!(old_completions.load(Ordering::Acquire), 1);
        assert_eq!(replacement_completions.load(Ordering::Acquire), 1);
    }

    #[ktest]
    fn replay_failure_completes_postponed_bio_with_io_error() {
        let (device, first) = create_device_and_table();
        let rejecting = Arc::new(RejectingBlockDevice) as Arc<dyn BlockDevice>;
        let replacement = Arc::new(
            DmTable::new_single_linear(
                Sid::new(0),
                128,
                Sid::new(32),
                BlockDeviceLease::new_untracked(rejecting),
            )
            .unwrap(),
        );
        device.load_table(first);
        device.resume().unwrap();
        device.suspend().unwrap();

        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        Bio::new(
            BioType::Read,
            Sid::new(0),
            vec![BioSegment::alloc(1, BioDirection::FromDevice)],
            None,
        )
        .submit(device.as_ref(), &mut batch)
        .unwrap();
        assert_eq!(device.state.lock().postponed.len(), 1);

        device.load_table(replacement);
        device.resume().unwrap();

        assert!(batch.wait_all().is_err());
        assert_eq!(device.io.in_flight.load(Ordering::Acquire), 0);
    }

    #[ktest]
    fn fail_postponed_bios_completes_waiters_with_io_error() {
        let (device, table) = create_device_and_table();
        device.load_table(table);
        device.resume().unwrap();
        device.suspend().unwrap();

        let mut batch = io_util::batch::IoBatch::with_capacity(1);
        Bio::new(
            BioType::Read,
            Sid::new(0),
            vec![BioSegment::alloc(1, BioDirection::FromDevice)],
            None,
        )
        .submit(device.as_ref(), &mut batch)
        .unwrap();
        assert_eq!(device.state.lock().postponed.len(), 1);

        device.fail_postponed_bios();

        assert!(batch.wait_all().is_err());
        assert!(device.state.lock().postponed.is_empty());
        assert_eq!(device.io.in_flight.load(Ordering::Acquire), 0);
    }

    #[ktest]
    fn readonly_device_refuses_write_like_bios_but_allows_read_and_flush() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-readonly-test".to_string(), None, None)
            .unwrap();
        let backing = Arc::new(TestBlockDevice) as Arc<dyn BlockDevice>;
        let table = Arc::new(
            DmTable::new_targets_with_readonly(
                vec![Box::new(
                    crate::target::linear::LinearTarget::new(
                        Sid::new(0),
                        128,
                        Sid::new(16),
                        BlockDeviceLease::new_untracked(backing.clone()),
                    )
                    .unwrap(),
                ) as crate::target::DmTargetBox],
                true,
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

        device.suspend_no_flush().unwrap();
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
        device.resume().unwrap();

        let writable_table = Arc::new(
            DmTable::new_single_linear(
                Sid::new(0),
                128,
                Sid::new(16),
                BlockDeviceLease::new_untracked(backing),
            )
            .unwrap(),
        );
        device.load_table(writable_table);
        device.resume().unwrap();

        assert!(!device.status().readonly);
        assert_eq!(
            Bio::new(
                BioType::Write,
                Sid::new(0),
                vec![BioSegment::alloc(1, BioDirection::ToDevice)],
                None,
            )
            .submit_and_wait(device.as_ref())
            .unwrap(),
            BioStatus::Complete
        );
    }
}
