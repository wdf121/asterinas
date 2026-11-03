// SPDX-License-Identifier: MPL-2.0

//! Tableless mapper identity. Its number remains owned while references exist,
//! unless completed removal explicitly releases it after the control-plane drain.

use alloc::{string::String, vec::Vec};

use device_id::DeviceId;
use ostd::sync::Mutex;

use crate::manager::DmDeviceIdOwner;

/// An unregistered Device Mapper identity without an active or inactive table.
#[derive(Debug)]
pub struct DmDevice {
    id_owner: DmDeviceIdOwner,
    primary_name: String,
    mapper_name: Mutex<String>,
    uuid: Mutex<Option<Vec<u8>>>,
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

    /// Releases the number after the caller has drained control and I/O users.
    /// Structural references can continue to observe the immutable identity.
    pub fn drain_removed(&self) {
        self.id_owner.release();
    }
}
