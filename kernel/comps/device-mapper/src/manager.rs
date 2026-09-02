// SPDX-License-Identifier: MPL-2.0

use alloc::{collections::BTreeMap, string::String, sync::Arc, vec::Vec};

use aster_block::{BlockDevice, MajorIdOwner, allocate_major_with_name};
use device_id::{DeviceId, MinorId};
use id_alloc::IdAlloc;
use ostd::sync::Mutex;

use crate::{DmDevice, DmError};

struct ManagerInner {
    by_name: BTreeMap<String, Arc<DmDevice>>,
    name_by_uuid: BTreeMap<String, String>,
}

/// Keeps a DM device number allocated until the device object is finally destroyed.
pub(crate) struct DmDeviceIdOwner {
    id: DeviceId,
    _major: Arc<MajorIdOwner>,
    minors: Arc<Mutex<IdAlloc>>,
}

impl DmDeviceIdOwner {
    fn new(id: DeviceId, major: Arc<MajorIdOwner>, minors: Arc<Mutex<IdAlloc>>) -> Self {
        Self {
            id,
            _major: major,
            minors,
        }
    }

    pub(crate) fn id(&self) -> DeviceId {
        self.id
    }
}

impl Drop for DmDeviceIdOwner {
    fn drop(&mut self) {
        self.minors.lock().free(self.id.minor().get() as usize);
    }
}

/// Manages Device Mapper devices in the current boot session.
pub struct DmManager {
    major: Arc<MajorIdOwner>,
    minors: Arc<Mutex<IdAlloc>>,
    inner: Mutex<ManagerInner>,
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
            }),
        })
    }

    /// Returns the dynamic block major used by this manager.
    pub fn major(&self) -> u16 {
        self.major.get().get()
    }

    /// Creates a device without a loaded mapping table.
    pub fn create(
        &self,
        name: String,
        uuid: Option<String>,
        requested_minor: Option<u32>,
    ) -> Result<Arc<DmDevice>, DmError> {
        self.create_with_readonly(name, uuid, requested_minor, false)
    }

    /// Creates a device without a loaded mapping table and sets its read-only mode.
    pub fn create_with_readonly(
        &self,
        name: String,
        uuid: Option<String>,
        requested_minor: Option<u32>,
        readonly: bool,
    ) -> Result<Arc<DmDevice>, DmError> {
        let mut inner = self.inner.lock();
        if inner.by_name.contains_key(&name) {
            return Err(DmError::NameExists);
        }
        if uuid
            .as_ref()
            .is_some_and(|uuid| inner.name_by_uuid.contains_key(uuid))
        {
            return Err(DmError::UuidExists);
        }

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
        let device = Arc::new(DmDevice::new(
            id_owner,
            name.clone(),
            uuid.clone(),
            readonly,
        ));
        inner.by_name.insert(name.clone(), device.clone());
        if let Some(uuid) = uuid {
            inner.name_by_uuid.insert(uuid, name);
        }
        Ok(device)
    }

    /// Looks up a device by name and returns an independent `Arc` without holding the manager lock.
    pub fn lookup_name(&self, name: &str) -> Option<Arc<DmDevice>> {
        self.inner.lock().by_name.get(name).cloned()
    }

    /// Looks up a device by UUID.
    pub fn lookup_uuid(&self, uuid: &str) -> Option<Arc<DmDevice>> {
        let inner = self.inner.lock();
        let name = inner.name_by_uuid.get(uuid)?;
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
        if inner.by_name.contains_key(new_name) {
            return Err(DmError::NameExists);
        }
        let device = inner.by_name.remove(old_name).unwrap();
        if let Some(uuid) = device.uuid() {
            inner.name_by_uuid.insert(uuid, String::from(new_name));
        }
        device.rename(String::from(new_name));
        inner.by_name.insert(String::from(new_name), device);
        Ok(())
    }

    /// Atomically updates the device UUID and UUID index.
    pub fn rename_uuid(&self, name: &str, new_uuid: String) -> Result<(), DmError> {
        let mut inner = self.inner.lock();
        let device = inner
            .by_name
            .get(name)
            .cloned()
            .ok_or(DmError::DeviceNotFound)?;
        if let Some(owner_name) = inner.name_by_uuid.get(&new_uuid) {
            if owner_name != name {
                return Err(DmError::UuidExists);
            }
            return Ok(());
        }
        if let Some(old_uuid) = device.uuid() {
            inner.name_by_uuid.remove(&old_uuid);
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
    fn manages_indexes_and_requested_minors() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create(
                "dm-test".to_string(),
                Some("uuid-test".to_string()),
                Some(7),
            )
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
                .create("dm-other".to_string(), Some("uuid-test".to_string()), None,)
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
    fn creates_readonly_device_when_requested() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create_with_readonly("dm-readonly".to_string(), None, None, true)
            .unwrap();

        assert!(device.is_readonly());
        assert!(device.status().readonly);
        assert_eq!(
            manager.lookup_name("dm-readonly").unwrap().id(),
            device.id()
        );
    }

    #[ktest]
    fn renames_name_and_uuid_indexes_together() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-old".to_string(), Some("dm-uuid".to_string()), None)
            .unwrap();

        manager.rename("dm-old", "dm-new").unwrap();

        assert!(manager.lookup_name("dm-old").is_none());
        assert_eq!(manager.lookup_name("dm-new").unwrap().id(), device.id());
        assert_eq!(manager.lookup_uuid("dm-uuid").unwrap().id(), device.id());
        assert_eq!(device.name(), "dm-new");
    }

    #[ktest]
    fn renames_uuid_index_without_touching_name_or_id() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create(
                "dm-uuid-device".to_string(),
                Some("old-uuid".to_string()),
                None,
            )
            .unwrap();
        let id = device.id();

        manager
            .rename_uuid("dm-uuid-device", "new-uuid".to_string())
            .unwrap();

        assert!(manager.lookup_uuid("old-uuid").is_none());
        assert_eq!(manager.lookup_uuid("new-uuid").unwrap().id(), id);
        assert_eq!(manager.lookup_name("dm-uuid-device").unwrap().id(), id);
        assert_eq!(device.name(), "dm-uuid-device");
        assert_eq!(device.uuid().unwrap(), "new-uuid");
    }

    #[ktest]
    fn rejects_duplicate_uuid_rename_without_changing_state() {
        let manager = DmManager::new().unwrap();
        let first = manager
            .create("dm-first".to_string(), Some("first-uuid".to_string()), None)
            .unwrap();
        let second = manager
            .create(
                "dm-second".to_string(),
                Some("second-uuid".to_string()),
                None,
            )
            .unwrap();

        assert_eq!(
            manager
                .rename_uuid("dm-first", "second-uuid".to_string())
                .unwrap_err(),
            DmError::UuidExists
        );

        assert_eq!(manager.lookup_uuid("first-uuid").unwrap().id(), first.id());
        assert_eq!(
            manager.lookup_uuid("second-uuid").unwrap().id(),
            second.id()
        );
        assert_eq!(first.uuid().unwrap(), "first-uuid");
        assert_eq!(second.uuid().unwrap(), "second-uuid");
    }

    #[ktest]
    fn sets_uuid_on_device_created_without_uuid() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-no-uuid".to_string(), None, None)
            .unwrap();

        manager
            .rename_uuid("dm-no-uuid", "created-uuid".to_string())
            .unwrap();

        assert_eq!(
            manager.lookup_uuid("created-uuid").unwrap().id(),
            device.id()
        );
        assert_eq!(device.uuid().unwrap(), "created-uuid");
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
