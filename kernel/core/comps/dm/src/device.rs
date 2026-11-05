// SPDX-License-Identifier: MPL-2.0

//! Mapper identity and table ownership. Its number remains owned while references
//! exist, unless removal releases it after the control-plane drain.

use alloc::{string::String, sync::Arc, vec::Vec};

use aster_block::{
    BlockDevice, BlockDeviceMeta,
    bio::{BioEnqueueError, BioType, SubmittedBio},
};
use device_id::DeviceId;
use ostd::sync::Mutex;

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
    /// Serializes LOAD and first RESUME control transitions for this mapper.
    lifecycle: Mutex<()>,
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

    /// Releases the number after the caller has drained control and I/O users.
    /// Structural references can continue to observe the immutable identity.
    pub fn drain_removed(&self) {
        self.id_owner.release();
    }
}

impl BlockDevice for DmDevice {
    fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        let table = self.active_table().ok_or(BioEnqueueError::Refused)?;
        // Only Read, Write and Flush exist today; Flush must remain permitted on
        // a read-only table so callers can drain backing write caches.
        if table.is_readonly() && bio.type_() == BioType::Write {
            return Err(BioEnqueueError::Refused);
        }
        table.enqueue(bio)
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
        bio::{Bio, BioDirection, BioSegment, BioStatus},
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

    fn table(backing: Arc<PendingBacking>, readonly: bool) -> Arc<DmTable> {
        let target = LinearTarget::new(
            Sid::new(0),
            8,
            Sid::new(100),
            BlockDeviceLease::new_untracked(backing),
        )
        .unwrap();
        Arc::new(
            DmTable::new_targets_with_readonly(vec![alloc::boxed::Box::new(target)], readonly)
                .unwrap(),
        )
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
