// SPDX-License-Identifier: MPL-2.0

//! Block-device identity and interface for a fixed linear mapping.
//!
//! Device registration and device-number allocation are handled by callers.

use alloc::string::String;

use aster_block::{
    BlockDevice, BlockDeviceMeta,
    bio::{BioEnqueueError, SubmittedBio},
};
use device_id::DeviceId;

use crate::linear::LinearTarget;

/// A block device backed by a Device Mapper target.
///
/// It implements [`BlockDevice`] by forwarding to the underlying target, so
/// the mapped device can be used wherever a block device is expected.
#[derive(Debug)]
pub struct DmDevice {
    id: DeviceId,
    name: String,
    target: LinearTarget,
}

impl DmDevice {
    /// Constructs a new `DmDevice` from the given device identity, name, and
    /// target.
    pub fn new(id: DeviceId, name: String, target: LinearTarget) -> Self {
        Self { id, name, target }
    }
}

impl BlockDevice for DmDevice {
    fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        self.target.enqueue(bio)
    }

    fn metadata(&self) -> BlockDeviceMeta {
        self.target.metadata()
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> DeviceId {
        self.id
    }
}
