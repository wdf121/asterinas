// SPDX-License-Identifier: MPL-2.0

//! Dispatch of the supported Device Mapper control commands.

use aster_dm::{DmDevice, DmError};
use device_id::decode_device_numbers;

use super::{super::DM_MANAGER, abi::*};
use crate::prelude::*;

type CommandHandler = fn(&mut [u8; FIXED_PREFIX_SIZE]) -> Result<()>;

/// Selects only commands implemented by this control device.
pub(super) fn handler_for(command: u8) -> Result<CommandHandler> {
    match command {
        DM_DEV_CREATE_CMD => Ok(create_device),
        _ => return_errno_with_message!(Errno::ENOTTY, "unsupported device-mapper ioctl command"),
    }
}

/// Creates a mapped device and writes its assigned identity back into the ioctl header.
fn create_device(buffer: &mut [u8; FIXED_PREFIX_SIZE]) -> Result<()> {
    let name = required_name(buffer)?;
    let uuid = optional_uuid(buffer);
    let flags = read_u32(buffer, OFF_FLAGS);
    let requested_minor = if flags & PERSISTENT_DEV_FLAG != 0 {
        Some(decode_device_numbers(read_u64(buffer, OFF_DEV)).1)
    } else {
        None
    };

    validate_mapper_name(&name)?;
    let device = DM_MANAGER
        .get()
        .unwrap()
        .create(name, uuid, requested_minor)
        .map_err(map_create_error)?;
    fill_device_header(buffer, &device);
    Ok(())
}

fn required_name(buffer: &[u8; FIXED_PREFIX_SIZE]) -> Result<String> {
    let name = &buffer[OFF_NAME..OFF_NAME + NAME_LEN];
    let end = name.iter().position(|byte| *byte == 0).unwrap();
    core::str::from_utf8(&name[..end])
        .map(String::from)
        .map_err(|_| Error::with_message(Errno::EINVAL, "mapper name is not UTF-8"))
}

fn optional_uuid(buffer: &[u8; FIXED_PREFIX_SIZE]) -> Option<Vec<u8>> {
    let uuid = &buffer[OFF_UUID..OFF_UUID + UUID_LEN];
    let end = uuid.iter().position(|byte| *byte == 0).unwrap();
    (end != 0).then(|| uuid[..end].to_vec())
}

fn validate_mapper_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() >= NAME_LEN
        || name.contains('/')
        || matches!(name, "." | ".." | "control")
    {
        return_errno_with_message!(Errno::EINVAL, "invalid mapper name");
    }
    Ok(())
}

fn fill_device_header(buffer: &mut [u8; FIXED_PREFIX_SIZE], device: &DmDevice) {
    write_u32(buffer, OFF_TARGET_COUNT, 0);
    write_u32(buffer, OFF_OPEN_COUNT, 0);
    write_u32(buffer, OFF_EVENT_NR, 0);
    write_u64(buffer, OFF_DEV, device.id().as_encoded_u64());
    let flags = read_u32(buffer, OFF_FLAGS)
        & !(READONLY_FLAG
            | SUSPEND_FLAG
            | ACTIVE_PRESENT_FLAG
            | INACTIVE_PRESENT_FLAG
            | INTERNAL_SUSPEND_FLAG);
    write_u32(buffer, OFF_FLAGS, flags);
    let name = device.mapper_name();
    buffer[OFF_NAME..OFF_NAME + NAME_LEN].fill(0);
    buffer[OFF_NAME..OFF_NAME + name.len()].copy_from_slice(name.as_bytes());
    buffer[OFF_UUID..OFF_UUID + UUID_LEN].fill(0);
    if let Some(uuid) = device.uuid() {
        buffer[OFF_UUID..OFF_UUID + uuid.len()].copy_from_slice(&uuid);
    }
}

fn map_create_error(error: DmError) -> Error {
    match error {
        DmError::NameExists | DmError::UuidExists => {
            Error::with_message(Errno::EBUSY, "mapper identity already exists")
        }
        DmError::MinorBusy | DmError::MinorExhausted => {
            Error::with_message(Errno::ENXIO, "cannot allocate mapper device number")
        }
        _ => Error::with_message(Errno::EINVAL, "cannot create mapper"),
    }
}
