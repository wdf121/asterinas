// SPDX-License-Identifier: MPL-2.0

//! Shared request and backing-device fixtures for DM control tests.

use aster_block::{
    BlockDevice, BlockDeviceMeta,
    bio::{BioEnqueueError, SubmittedBio},
};
pub(super) use device_id::{DeviceId, MajorId, MinorId};

use super::abi::*;
use crate::prelude::*;

#[derive(Debug)]
pub(super) struct Backing(pub(super) DeviceId);

impl BlockDevice for Backing {
    fn enqueue(&self, _bio: SubmittedBio) -> core::result::Result<(), BioEnqueueError> {
        unreachable!()
    }

    fn metadata(&self) -> BlockDeviceMeta {
        BlockDeviceMeta {
            max_nr_segments_per_bio: 8,
            nr_sectors: 1024,
        }
    }

    fn name(&self) -> &str {
        "load-test-backing"
    }
    fn id(&self) -> DeviceId {
        self.0
    }
}

pub(super) fn buffer(name: &str, backing: DeviceId) -> Vec<u8> {
    let mut bytes = vec![0u8; DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE + 32];
    bytes[OFF_NAME..OFF_NAME + name.len()].copy_from_slice(name.as_bytes());
    write_u32(&mut bytes, OFF_DATA_START, DM_IOCTL_HEADER_SIZE as u32);
    write_u32(&mut bytes, OFF_TARGET_COUNT, 1);
    let spec = DM_IOCTL_HEADER_SIZE;
    write_u64(&mut bytes, spec, 0);
    write_u64(&mut bytes, spec + 8, 8);
    bytes[spec + 24..spec + 30].copy_from_slice(b"linear");
    let params = alloc::format!("{}:{} 0", backing.major().get(), backing.minor().get());
    let start = spec + DM_TARGET_SPEC_SIZE;
    bytes[start..start + params.len()].copy_from_slice(params.as_bytes());
    bytes
}
