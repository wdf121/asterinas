// SPDX-License-Identifier: MPL-2.0

//! Mapper identity indexes and device-number ownership.
//! Failed creation releases its temporary minor; completed removal can release
//! the number after control and I/O drain, independently of structural references.

use alloc::{collections::BTreeMap, string::String, sync::Arc, vec::Vec};

use aster_block::{MajorIdOwner, allocate_major};
use device_id::{DeviceId, MinorId};
use id_alloc::IdAlloc;
use ostd::sync::Mutex;

use crate::{DmDevice, DmError};

struct ManagerInner {
    by_name: BTreeMap<String, Arc<DmDevice>>,
    name_by_uuid: BTreeMap<Vec<u8>, String>,
}

/// Owns a minor allocation independently of the immutable device identity.
/// Drop releases the number after failed creation or after the last device reference.
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

    /// Releases the minor once after runtime removal has drained its users.
    /// Old structural references cannot free a replacement's allocation later.
    pub(crate) fn release(&self) {
        if let Some(minors) = self.minors.lock().take() {
            minors.lock().free(self.id.minor().get() as usize);
        }
    }

    pub(crate) fn id(&self) -> DeviceId {
        self.id
    }
}

impl core::fmt::Debug for DmDeviceIdOwner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DmDeviceIdOwner")
            .field("id", &self.id)
            .finish()
    }
}

impl Drop for DmDeviceIdOwner {
    fn drop(&mut self) {
        self.release();
    }
}

/// Manages Device Mapper identities in the current boot session.
pub struct DmManager {
    major: Arc<MajorIdOwner>,
    minors: Arc<Mutex<IdAlloc>>,
    inner: Mutex<ManagerInner>,
}

impl DmManager {
    /// Creates a manager and dynamically allocates a block major.
    pub fn new() -> Result<Self, DmError> {
        // This branch does not yet register names for /proc/devices.
        let major = Arc::new(allocate_major().map_err(|_| DmError::MajorExhausted)?);
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

    /// Creates a tableless device without assigning a read/write mode.
    pub fn create(
        &self,
        name: String,
        uuid: Option<Vec<u8>>,
        requested_minor: Option<u32>,
    ) -> Result<Arc<DmDevice>, DmError> {
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
        if inner.by_name.contains_key(&name) {
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
        Ok(device)
    }

    /// Looks up a device by name without holding the manager lock afterward.
    pub fn lookup_name(&self, name: &str) -> Option<Arc<DmDevice>> {
        self.inner.lock().by_name.get(name).cloned()
    }

    /// Looks up an opaque UUID by its exact bytes.
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

    /// Returns a snapshot of all devices.
    pub fn devices(&self) -> Vec<Arc<DmDevice>> {
        self.inner.lock().by_name.values().cloned().collect()
    }

    /// Detaches a device from the manager indexes. The number remains reserved
    /// until the final reference is dropped; runtime removal is not yet exposed.
    pub fn remove(&self, name: &str) -> Result<Arc<DmDevice>, DmError> {
        let mut inner = self.inner.lock();
        let device = inner.by_name.remove(name).ok_or(DmError::DeviceNotFound)?;
        if let Some(uuid) = device.uuid() {
            inner.name_by_uuid.remove(&uuid);
        }
        Ok(device)
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
            assert_eq!(
                manager.lookup_uuid("existing-uuid").unwrap().id(),
                device.id()
            );
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
        assert_eq!(device.primary_name(), "dm-7");
        assert_eq!(device.mapper_name(), "dm-test");
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
                .create("dm-other".to_string(), Some(b"uuid-test".to_vec()), None)
                .unwrap_err(),
            DmError::UuidExists
        );
        assert_eq!(
            manager
                .create("dm-other".to_string(), None, Some(7))
                .unwrap_err(),
            DmError::MinorBusy
        );
        assert_eq!(
            manager
                .create("dm-other".to_string(), None, Some(u32::MAX))
                .unwrap_err(),
            DmError::MinorExhausted
        );
    }

    #[ktest]
    fn completed_removal_releases_minor_without_dropping_old_snapshots() {
        let manager = DmManager::new().unwrap();
        let old = manager
            .create("dm-reuse".to_string(), None, Some(7))
            .unwrap();
        let id = old.id();
        manager.remove("dm-reuse").unwrap();
        old.drain_removed();
        let replacement = manager
            .create("dm-reuse".to_string(), None, Some(7))
            .unwrap();
        assert_eq!(replacement.id(), id);
        assert_eq!(old.id(), id);
        assert_eq!(manager.lookup_id(id).unwrap().id(), replacement.id());
        // Repeated cleanup and the old object's drop must not free the new allocation.
        old.drain_removed();
        drop(old);
        assert_eq!(
            manager
                .create("dm-collision".to_string(), None, Some(7))
                .unwrap_err(),
            DmError::MinorBusy
        );
        manager.remove("dm-reuse").unwrap();
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
        assert!(manager.lookup_name("dm-persistent-minor").is_none());
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
        drop(manager);
        assert!(aster_block::acquire_major(MajorId::new(major)).is_err());
        drop(device);
        drop(removed);
        let _major_owner = aster_block::acquire_major(MajorId::new(major)).unwrap();
    }
}
