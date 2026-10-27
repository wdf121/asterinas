// SPDX-License-Identifier: MPL-2.0

//! Mapper identity indexes, runtime name reservations, and device-number ownership.
//! Failed creation releases its temporary minor; completed removal releases the
//! number after control and I/O drain, independently of structural references.

use alloc::{
    collections::{BTreeMap, BTreeSet},
    string::String,
    sync::Arc,
    vec::Vec,
};

use aster_block::{BlockDevice, MajorIdOwner, allocate_major_with_name};
use device_id::{DeviceId, MinorId};
use id_alloc::IdAlloc;
use ostd::sync::Mutex;

use crate::{DmDevice, DmError};

struct ManagerInner {
    by_name: BTreeMap<String, Arc<DmDevice>>,
    name_by_uuid: BTreeMap<Vec<u8>, String>,
    reserved_runtime_names: BTreeSet<String>,
}

/// Owns a minor allocation independently of the immutable device identity.
///
/// Completed removal releases the allocation explicitly. Drop remains a fallback
/// for failed creation and component users that only detach a device.
pub(crate) struct DmDeviceIdOwner {
    id: DeviceId,
    _major: Arc<MajorIdOwner>,
    minors: Mutex<Option<Arc<Mutex<IdAlloc>>>>,
}

impl DmDeviceIdOwner {
    fn new(id: DeviceId, major: Arc<MajorIdOwner>, minors: Arc<Mutex<IdAlloc>>) -> Self {
        Self {
            id,
            _major: major,
            minors: Mutex::new(Some(minors)),
        }
    }

    /// Releases the minor once; an old object's eventual drop cannot free a
    /// replacement device's allocation of the same number.
    pub(crate) fn release(&self) {
        if let Some(minors) = self.minors.lock().take() {
            minors.lock().free(self.id.minor().get() as usize);
        }
    }

    pub(crate) fn id(&self) -> DeviceId {
        self.id
    }
}

impl Drop for DmDeviceIdOwner {
    fn drop(&mut self) {
        self.release();
    }
}

/// Manages Device Mapper devices in the current boot session.
pub struct DmManager {
    major: Arc<MajorIdOwner>,
    minors: Arc<Mutex<IdAlloc>>,
    inner: Mutex<ManagerInner>,
}

/// Reserves a runtime mapper name while its devtmpfs alias is being moved.
///
/// The caller must serialize conflicting lifecycle operations for `device` until
/// this reservation is committed or dropped.
pub struct RuntimeRenameReservation<'a> {
    manager: &'a DmManager,
    device: Arc<DmDevice>,
    old_name: String,
    new_name: String,
    committed: bool,
}

impl RuntimeRenameReservation<'_> {
    /// Commits the reserved name after its mapper alias has moved successfully.
    pub fn commit(mut self) {
        let mut inner = self.manager.inner.lock();
        let device = inner
            .by_name
            .remove(&self.old_name)
            .expect("runtime rename reservation lost its source device");
        assert!(Arc::ptr_eq(&device, &self.device));
        assert!(inner.reserved_runtime_names.remove(&self.new_name));
        if let Some(uuid) = device.uuid() {
            inner.name_by_uuid.insert(uuid, self.new_name.clone());
        }
        device.rename_mapper(self.new_name.clone());
        inner.by_name.insert(self.new_name.clone(), device);
        self.committed = true;
    }
}

impl Drop for RuntimeRenameReservation<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.manager
                .inner
                .lock()
                .reserved_runtime_names
                .remove(&self.new_name);
        }
    }
}

impl DmManager {
    /// Creates a manager and dynamically allocates a block major.
    pub fn new() -> Result<Self, DmError> {
        let major = Arc::new(
            allocate_major_with_name("device-mapper").map_err(|_| DmError::MajorExhausted)?,
        );
        Ok(Self {
            major,
            minors: Arc::new(Mutex::new(IdAlloc::with_capacity(
                MinorId::MAX.get() as usize + 1,
            ))),
            inner: Mutex::new(ManagerInner {
                by_name: BTreeMap::new(),
                name_by_uuid: BTreeMap::new(),
                reserved_runtime_names: BTreeSet::new(),
            }),
        })
    }

    /// Returns the dynamic block major used by this manager.
    pub fn major(&self) -> u16 {
        self.major.get().get()
    }

    /// Creates a tableless device without assigning a read/write mode.
    ///
    /// Linux Device Mapper associates `DM_READONLY_FLAG` with a loaded table rather
    /// than with a mapper identity.
    pub fn create(
        &self,
        name: String,
        uuid: Option<Vec<u8>>,
        requested_minor: Option<u32>,
    ) -> Result<Arc<DmDevice>, DmError> {
        self.create_control(name, uuid, requested_minor)
            .map(|device| Arc::clone(&device))
    }

    /// Creates a device with a control reference admitted before publication.
    /// This keeps its number reserved through the CREATE response snapshot even
    /// if another control operation immediately removes the new identity.
    pub fn create_control(
        &self,
        name: String,
        uuid: Option<Vec<u8>>,
        requested_minor: Option<u32>,
    ) -> Result<crate::DmControlRef, DmError> {
        let mut inner = self.inner.lock();
        let minor = {
            let mut minors = self.minors.lock();
            match requested_minor {
                Some(minor) => {
                    let minor = usize::try_from(minor).map_err(|_| DmError::MinorExhausted)?;
                    if minor > MinorId::MAX.get() as usize {
                        return Err(DmError::MinorExhausted);
                    }
                    minors.alloc_specific(minor).ok_or(DmError::MinorBusy)?
                }
                None => minors.alloc().ok_or(DmError::MinorExhausted)?,
            }
        };
        let id = DeviceId::new(self.major.get(), MinorId::new(minor as u32));
        let id_owner = DmDeviceIdOwner::new(id, self.major.clone(), self.minors.clone());
        // Linux allocates the device number before inserting the identity. The
        // owner returns this minor if a name or UUID collision rejects creation.
        if inner.by_name.contains_key(&name) || inner.reserved_runtime_names.contains(&name) {
            return Err(DmError::NameExists);
        }
        if uuid
            .as_ref()
            .is_some_and(|uuid| inner.name_by_uuid.contains_key(uuid))
        {
            return Err(DmError::UuidExists);
        }
        let device = Arc::new(DmDevice::new(id_owner, name.clone(), uuid.clone()));
        inner.by_name.insert(name.clone(), device.clone());
        if let Some(uuid) = uuid {
            inner.name_by_uuid.insert(uuid, name);
        }
        Ok(crate::DmControlRef::new(device))
    }

    /// Looks up a device by name and returns an independent `Arc` without holding the manager lock.
    pub fn lookup_name(&self, name: &str) -> Option<Arc<DmDevice>> {
        self.inner.lock().by_name.get(name).cloned()
    }

    /// Looks up an opaque UUID by its exact bytes, without Unicode normalization.
    pub fn lookup_uuid(&self, uuid: impl AsRef<[u8]>) -> Option<Arc<DmDevice>> {
        let inner = self.inner.lock();
        let name = inner.name_by_uuid.get(uuid.as_ref())?;
        inner.by_name.get(name).cloned()
    }

    /// Looks up a device by device ID.
    pub fn lookup_id(&self, id: DeviceId) -> Option<Arc<DmDevice>> {
        if id.major() != self.major.get() {
            return None;
        }
        self.inner
            .lock()
            .by_name
            .values()
            .find(|device| device.id() == id)
            .cloned()
    }

    /// Returns whether this manager still owns the exact device instance.
    ///
    /// Callers that release a global lookup lock before waiting on a per-device
    /// lifecycle operation use this to reject an `Arc` that was removed and
    /// replaced before their operation began.
    pub fn is_current(&self, device: &Arc<DmDevice>) -> bool {
        self.inner
            .lock()
            .by_name
            .values()
            .any(|current| Arc::ptr_eq(current, device))
    }

    /// Admits a control reference only while this exact instance is indexed.
    /// The shared index lock closes the lookup-versus-removal admission race.
    pub fn acquire_control(&self, device: &Arc<DmDevice>) -> Option<crate::DmControlRef> {
        let inner = self.inner.lock();
        inner
            .by_name
            .values()
            .any(|current| Arc::ptr_eq(current, device))
            .then(|| crate::DmControlRef::new(device.clone()))
    }

    /// Reserves a new name while a runtime mapper alias moves outside this lock.
    ///
    /// The caller must hold the device lifecycle guard so the source mapping
    /// cannot be removed or renamed before [`RuntimeRenameReservation::commit`].
    pub fn reserve_runtime_rename<'a>(
        &'a self,
        device: Arc<DmDevice>,
        old_name: &str,
        new_name: &str,
    ) -> Result<RuntimeRenameReservation<'a>, DmError> {
        let mut inner = self.inner.lock();
        let current = inner.by_name.get(old_name).ok_or(DmError::DeviceNotFound)?;
        if !Arc::ptr_eq(current, &device) {
            return Err(DmError::DeviceNotFound);
        }
        if inner.by_name.contains_key(new_name) || inner.reserved_runtime_names.contains(new_name) {
            return Err(DmError::NameExists);
        }
        inner.reserved_runtime_names.insert(String::from(new_name));
        Ok(RuntimeRenameReservation {
            manager: self,
            device,
            old_name: String::from(old_name),
            new_name: String::from(new_name),
            committed: false,
        })
    }

    /// Returns a snapshot of all devices.
    pub fn devices(&self) -> Vec<Arc<DmDevice>> {
        self.inner.lock().by_name.values().cloned().collect()
    }

    /// Removes a device from the manager indexes.
    ///
    /// The control plane must stop new opens and unregister the block registry
    /// entry and device nodes before calling this method. The minor number is
    /// released automatically after the last device reference is destroyed.
    pub fn remove(&self, name: &str) -> Result<Arc<DmDevice>, DmError> {
        let mut inner = self.inner.lock();
        let device = inner.by_name.remove(name).ok_or(DmError::DeviceNotFound)?;
        if let Some(uuid) = device.uuid() {
            inner.name_by_uuid.remove(&uuid);
        }
        Ok(device)
    }

    /// Atomically updates the device name and all indexes that depend on it.
    pub fn rename(&self, old_name: &str, new_name: &str) -> Result<(), DmError> {
        let mut inner = self.inner.lock();
        if !inner.by_name.contains_key(old_name) {
            return Err(DmError::DeviceNotFound);
        }
        if inner.by_name.contains_key(new_name) || inner.reserved_runtime_names.contains(new_name) {
            return Err(DmError::NameExists);
        }
        let device = inner.by_name.remove(old_name).unwrap();
        if let Some(uuid) = device.uuid() {
            inner.name_by_uuid.insert(uuid, String::from(new_name));
        }
        device.rename_mapper(String::from(new_name));
        inner.by_name.insert(String::from(new_name), device);
        Ok(())
    }

    /// Assigns a UUID once, checking destination conflicts before source identity.
    ///
    /// Like Linux `dm_hash_rename`, an existing destination (including the
    /// device's own UUID) is a conflict; an established UUID is immutable.
    pub fn rename_uuid(&self, name: &str, new_uuid: Vec<u8>) -> Result<(), DmError> {
        let mut inner = self.inner.lock();
        if inner.name_by_uuid.contains_key(&new_uuid) {
            return Err(DmError::UuidExists);
        }
        let device = inner
            .by_name
            .get(name)
            .cloned()
            .ok_or(DmError::DeviceNotFound)?;
        if device.uuid().is_some() {
            return Err(DmError::InvalidState);
        }
        inner
            .name_by_uuid
            .insert(new_uuid.clone(), String::from(name));
        device.rename_uuid(new_uuid);
        Ok(())
    }

    /// Removes all devices from the manager indexes.
    pub fn remove_all(&self) -> Vec<Arc<DmDevice>> {
        let mut inner = self.inner.lock();
        let devices: Vec<_> = inner.by_name.values().cloned().collect();
        inner.by_name.clear();
        inner.name_by_uuid.clear();
        devices
    }
}

#[cfg(ktest)]
mod tests {
    use alloc::string::ToString;

    use device_id::MajorId;
    use ostd::prelude::ktest;

    use super::*;

    #[ktest]
    fn creation_allocates_minor_before_identity_checks_and_releases_on_failure() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create(
                "existing".to_string(),
                Some(b"existing-uuid".to_vec()),
                Some(7),
            )
            .unwrap();
        for (name, uuid, identity_error) in [
            ("existing", None, DmError::NameExists),
            ("other", Some("existing-uuid"), DmError::UuidExists),
        ] {
            assert_eq!(
                manager
                    .create(
                        name.to_string(),
                        uuid.map(|value| value.as_bytes().to_vec()),
                        Some(7)
                    )
                    .unwrap_err(),
                DmError::MinorBusy
            );
            assert_eq!(
                manager
                    .create(
                        name.to_string(),
                        uuid.map(|value| value.as_bytes().to_vec()),
                        Some(8)
                    )
                    .unwrap_err(),
                identity_error
            );
            assert_eq!(manager.devices().len(), 1);
            assert!(Arc::ptr_eq(
                &manager.lookup_name("existing").unwrap(),
                &device
            ));
            assert!(Arc::ptr_eq(
                &manager.lookup_uuid("existing-uuid").unwrap(),
                &device
            ));
            assert!(manager.lookup_name("other").is_none());
            let retry = manager.create("retry".to_string(), None, Some(8)).unwrap();
            assert_eq!(retry.id().minor().get(), 8);
            drop(manager.remove("retry").unwrap());
            drop(retry);
        }
    }

    #[ktest]
    fn manages_indexes_and_requested_minors() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-test".to_string(), Some(b"uuid-test".to_vec()), Some(7))
            .unwrap();

        assert_eq!(device.id().minor().get(), 7);
        assert_eq!(manager.lookup_name("dm-test").unwrap().id(), device.id());
        assert_eq!(manager.lookup_uuid("uuid-test").unwrap().id(), device.id());
        assert_eq!(manager.lookup_id(device.id()).unwrap().id(), device.id());
        assert_eq!(manager.devices().len(), 1);
        assert_eq!(
            manager
                .create("dm-test".to_string(), None, None)
                .unwrap_err(),
            DmError::NameExists
        );
        assert_eq!(
            manager
                .create("dm-other".to_string(), Some(b"uuid-test".to_vec()), None,)
                .unwrap_err(),
            DmError::UuidExists
        );
        assert_eq!(
            manager
                .create("dm-other".to_string(), None, Some(7))
                .unwrap_err(),
            DmError::MinorBusy
        );
    }

    #[ktest]
    fn creates_tableless_device_without_readonly_mode() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-readonly".to_string(), None, None)
            .unwrap();

        assert!(!device.is_readonly());
        assert!(!device.status().readonly);
        assert_eq!(
            manager.lookup_name("dm-readonly").unwrap().id(),
            device.id()
        );
    }

    #[ktest]
    fn renames_name_and_uuid_indexes_together() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-old".to_string(), Some(b"dm-uuid".to_vec()), None)
            .unwrap();

        manager.rename("dm-old", "dm-new").unwrap();

        assert!(manager.lookup_name("dm-old").is_none());
        assert_eq!(manager.lookup_name("dm-new").unwrap().id(), device.id());
        assert_eq!(manager.lookup_uuid("dm-uuid").unwrap().id(), device.id());
        assert_eq!(device.mapper_name(), "dm-new");
        assert_eq!(BlockDevice::name(device.as_ref()), "dm-0");
    }

    #[ktest]
    fn runtime_rename_reservation_commits_name_and_uuid_indexes_together() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create(
                "dm-reservation-old".to_string(),
                Some(b"dm-reservation-uuid".to_vec()),
                None,
            )
            .unwrap();

        let reservation = manager
            .reserve_runtime_rename(device.clone(), "dm-reservation-old", "dm-reservation-new")
            .unwrap();
        assert_eq!(
            manager
                .create("dm-reservation-new".to_string(), None, None)
                .unwrap_err(),
            DmError::NameExists
        );
        reservation.commit();

        assert!(manager.lookup_name("dm-reservation-old").is_none());
        assert_eq!(
            manager.lookup_name("dm-reservation-new").unwrap().id(),
            device.id()
        );
        assert_eq!(
            manager.lookup_uuid("dm-reservation-uuid").unwrap().id(),
            device.id()
        );
        assert_eq!(device.mapper_name(), "dm-reservation-new");
    }

    #[ktest]
    fn runtime_rename_reservation_drop_keeps_old_name_and_releases_new_name() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-reservation-old".to_string(), None, None)
            .unwrap();

        let reservation = manager
            .reserve_runtime_rename(device.clone(), "dm-reservation-old", "dm-reservation-new")
            .unwrap();
        drop(reservation);

        assert_eq!(
            manager.lookup_name("dm-reservation-old").unwrap().id(),
            device.id()
        );
        assert!(manager.lookup_name("dm-reservation-new").is_none());
        assert_eq!(device.mapper_name(), "dm-reservation-old");
        assert!(
            manager
                .create("dm-reservation-new".to_string(), None, None)
                .is_ok()
        );
    }

    #[ktest]
    fn rejects_replacing_an_established_uuid() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create(
                "dm-uuid-device".to_string(),
                Some(b"old-uuid".to_vec()),
                None,
            )
            .unwrap();
        let id = device.id();

        assert_eq!(
            manager.rename_uuid("dm-uuid-device", b"new-uuid".to_vec()),
            Err(DmError::InvalidState)
        );
        assert_eq!(
            manager.rename_uuid("dm-uuid-device", b"old-uuid".to_vec()),
            Err(DmError::UuidExists)
        );
        assert_eq!(manager.lookup_uuid("old-uuid").unwrap().id(), id);
        assert!(manager.lookup_uuid("new-uuid").is_none());
        assert_eq!(manager.lookup_name("dm-uuid-device").unwrap().id(), id);
        assert_eq!(device.mapper_name(), "dm-uuid-device");
        assert_eq!(device.uuid().unwrap(), b"old-uuid");
    }

    #[ktest]
    fn rejects_duplicate_uuid_rename_without_changing_state() {
        let manager = DmManager::new().unwrap();
        let first = manager
            .create("dm-first".to_string(), Some(b"first-uuid".to_vec()), None)
            .unwrap();
        let second = manager
            .create("dm-second".to_string(), Some(b"second-uuid".to_vec()), None)
            .unwrap();

        assert_eq!(
            manager
                .rename_uuid("dm-first", b"second-uuid".to_vec())
                .unwrap_err(),
            DmError::UuidExists
        );

        assert_eq!(manager.lookup_uuid("first-uuid").unwrap().id(), first.id());
        assert_eq!(
            manager.lookup_uuid("second-uuid").unwrap().id(),
            second.id()
        );
        assert_eq!(first.uuid().unwrap(), b"first-uuid");
        assert_eq!(second.uuid().unwrap(), b"second-uuid");
    }

    #[ktest]
    fn sets_uuid_on_device_created_without_uuid() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-no-uuid".to_string(), None, None)
            .unwrap();

        manager
            .rename_uuid("dm-no-uuid", b"created-uuid".to_vec())
            .unwrap();

        assert_eq!(
            manager.lookup_uuid("created-uuid").unwrap().id(),
            device.id()
        );
        assert_eq!(device.uuid().unwrap(), b"created-uuid");
    }

    #[ktest]
    fn completed_removal_releases_minor_without_dropping_old_snapshots() {
        let manager = DmManager::new().unwrap();
        let old = manager
            .create("dm-reuse".to_string(), None, Some(7))
            .unwrap();
        let id = old.id();
        {
            let _lifecycle = old.lock_lifecycle();
            old.stop_removed_io();
            manager.remove("dm-reuse").unwrap();
        }
        old.drain_removed();
        let replacement = manager
            .create("dm-reuse".to_string(), None, Some(7))
            .unwrap();
        assert_eq!(replacement.id(), id);
        assert_eq!(old.id(), id);
        assert!(!manager.is_current(&old));
        assert!(manager.acquire_control(&old).is_none());
        assert!(Arc::ptr_eq(&manager.lookup_id(id).unwrap(), &replacement));
        // Repeated cleanup and the old object's drop must not release the new
        // allocation. Structural snapshots keep identity, not minor ownership.
        old.drain_removed();
        drop(old);
        assert_eq!(
            manager
                .create("dm-collision".to_string(), None, Some(7))
                .unwrap_err(),
            DmError::MinorBusy
        );
        {
            let _lifecycle = replacement.lock_lifecycle();
            replacement.stop_removed_io();
            manager.remove("dm-reuse").unwrap();
        }
        replacement.drain_removed();
        assert!(manager.create("dm-next".to_string(), None, Some(7)).is_ok());
    }

    #[ktest]
    fn releases_persistent_minor_after_last_device_reference_drops() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-persistent-minor".to_string(), None, Some(7))
            .unwrap();
        let removed = manager.remove("dm-persistent-minor").unwrap();

        assert_eq!(
            manager
                .create("dm-persistent-minor-retry".to_string(), None, Some(7))
                .unwrap_err(),
            DmError::MinorBusy
        );

        drop(device);
        drop(removed);
        assert!(
            manager
                .create("dm-persistent-minor-retry".to_string(), None, Some(7))
                .is_ok()
        );
    }

    #[ktest]
    fn keeps_device_ids_owned_until_last_device_reference_drops() {
        let manager = DmManager::new().unwrap();
        let major = manager.major();
        let device = manager
            .create("dm-test".to_string(), None, Some(7))
            .unwrap();
        let removed = manager.remove("dm-test").unwrap();

        assert!(manager.lookup_name("dm-test").is_none());
        assert_eq!(
            manager
                .create("dm-replacement".to_string(), None, Some(7))
                .unwrap_err(),
            DmError::MinorBusy
        );

        drop(manager);
        assert!(aster_block::acquire_major(MajorId::new(major)).is_err());
        drop(device);
        drop(removed);

        let _major_owner = aster_block::acquire_major(MajorId::new(major)).unwrap();
    }
}
