// SPDX-License-Identifier: MPL-2.0

//! Mapper identity and inactive table ownership. Its number remains owned while
//! references exist, unless removal releases it after the control-plane drain.

use alloc::{string::String, sync::Arc, vec::Vec};

use aster_block::{
    BlockDevice, BlockDeviceMeta,
    bio::{BioEnqueueError, SubmittedBio},
};
use device_id::DeviceId;
use ostd::sync::Mutex;

use crate::{DmTable, manager::DmDeviceIdOwner};

/// A mapper identity with an optional inactive table, not yet registered as a block device.
#[derive(Debug)]
pub struct DmDevice {
    id_owner: DmDeviceIdOwner,
    primary_name: String,
    mapper_name: Mutex<String>,
    uuid: Mutex<Option<Vec<u8>>>,
    inactive: Mutex<Option<Arc<DmTable>>>,
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
            inactive: Mutex::new(None),
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

    /// Installs a fully validated mapping table as the inactive table.
    pub fn load_table(&self, table: Arc<DmTable>) {
        *self.inactive.lock() = Some(table);
    }

    /// Returns the inactive table, if one has been installed.
    pub fn inactive_table(&self) -> Option<Arc<DmTable>> {
        self.inactive.lock().clone()
    }

    /// Releases the number after the caller has drained control and I/O users.
    /// Structural references can continue to observe the immutable identity.
    pub fn drain_removed(&self) {
        self.id_owner.release();
    }
}

impl BlockDevice for DmDevice {
    fn enqueue(&self, _bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        // No table is active before the initial resume.
        Err(BioEnqueueError::Refused)
    }

    fn metadata(&self) -> BlockDeviceMeta {
        BlockDeviceMeta::default()
    }

    fn name(&self) -> &str {
        self.primary_name.as_str()
    }

    fn id(&self) -> DeviceId {
        self.id_owner.id()
    }
}
