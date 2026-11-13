// SPDX-License-Identifier: MPL-2.0

//! Device Mapper ioctl ABI for CREATE, LOAD, suspend/resume, REMOVE and static queries.

use crate::prelude::*;

pub(super) const DM_LIST_DEVICES_CMD: u8 = 2;
pub(super) const DM_DEV_CREATE_CMD: u8 = 3;
pub(super) const DM_DEV_REMOVE_CMD: u8 = 4;
pub(super) const DM_DEV_SUSPEND_CMD: u8 = 6;
pub(super) const DM_DEV_STATUS_CMD: u8 = 7;
pub(super) const DM_TABLE_LOAD_CMD: u8 = 9;
pub(super) const DM_TABLE_CLEAR_CMD: u8 = 10;
pub(super) const DM_TABLE_DEPS_CMD: u8 = 11;
pub(super) const DM_TABLE_STATUS_CMD: u8 = 12;
pub(super) const DM_LIST_VERSIONS_CMD: u8 = 13;
pub(super) const DM_GET_TARGET_VERSION_CMD: u8 = 17;
pub(super) const DM_IOCTL_HEADER_SIZE: usize = 312;
pub(super) const DM_TARGET_SPEC_SIZE: usize = 40;
pub(super) const FIXED_PREFIX_SIZE: usize = 305;
pub(super) const MAX_IOCTL_SIZE: u32 = 1024 * 1024 * 1024;
pub(super) const OFF_DATA_SIZE: usize = 12;
pub(super) const OFF_DATA_START: usize = 16;
pub(super) const OFF_TARGET_COUNT: usize = 20;
pub(super) const OFF_OPEN_COUNT: usize = 24;
pub(super) const OFF_FLAGS: usize = 28;
pub(super) const OFF_EVENT_NR: usize = 32;
pub(super) const OFF_DEV: usize = 40;
pub(super) const OFF_NAME: usize = 48;
pub(super) const NAME_LEN: usize = 128;
pub(super) const OFF_UUID: usize = 176;
pub(super) const UUID_LEN: usize = 129;
pub(super) const READONLY_FLAG: u32 = 1;
pub(super) const SUSPEND_FLAG: u32 = 1 << 1;
#[cfg(ktest)]
pub(super) const SKIP_LOCKFS_FLAG: u32 = 1 << 10;
pub(super) const NOFLUSH_FLAG: u32 = 1 << 11;
pub(super) const PERSISTENT_DEV_FLAG: u32 = 1 << 3;
pub(super) const STATUS_TABLE_FLAG: u32 = 1 << 4;
pub(super) const ACTIVE_PRESENT_FLAG: u32 = 1 << 5;
pub(super) const INACTIVE_PRESENT_FLAG: u32 = 1 << 6;
pub(super) const BUFFER_FULL_FLAG: u32 = 1 << 8;
pub(super) const QUERY_INACTIVE_TABLE_FLAG: u32 = 1 << 12;
pub(super) const UEVENT_GENERATED_FLAG: u32 = 1 << 13;
pub(super) const UUID_FLAG: u32 = 1 << 14;
pub(super) const SECURE_DATA_FLAG: u32 = 1 << 15;
pub(super) const DATA_OUT_FLAG: u32 = 1 << 16;
pub(super) const DEFERRED_REMOVE_FLAG: u32 = 1 << 17;
pub(super) const INTERNAL_SUSPEND_FLAG: u32 = 1 << 18;

pub(super) fn validate_ioctl_size(size: u32) -> Result<()> {
    if !(FIXED_PREFIX_SIZE as u32..=MAX_IOCTL_SIZE).contains(&size) {
        return_errno_with_message!(Errno::EINVAL, "invalid device-mapper data_size");
    }
    Ok(())
}

pub(super) fn validate_ioctl_params(
    buffer: &mut [u8; FIXED_PREFIX_SIZE],
    command: u8,
) -> Result<()> {
    let flags = read_u32(buffer, OFF_FLAGS)
        & !(BUFFER_FULL_FLAG | UEVENT_GENERATED_FLAG | SECURE_DATA_FLAG | DATA_OUT_FLAG);
    write_u32(buffer, OFF_FLAGS, flags);
    if command == DM_DEV_CREATE_CMD {
        if buffer[OFF_NAME] == 0 {
            return_errno_with_message!(Errno::EINVAL, "missing mapper name");
        }
    } else if command != DM_LIST_DEVICES_CMD && buffer[OFF_NAME] != 0 && buffer[OFF_UUID] != 0 {
        return_errno_with_message!(
            Errno::EINVAL,
            "name and UUID selectors are mutually exclusive"
        );
    }
    buffer[OFF_NAME + NAME_LEN - 1] = 0;
    buffer[OFF_UUID + UUID_LEN - 1] = 0;
    Ok(())
}

pub(super) fn read_u32(buffer: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buffer[offset..offset + 4].try_into().unwrap())
}

pub(super) fn read_u64(buffer: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(buffer[offset..offset + 8].try_into().unwrap())
}

pub(super) fn write_u32(buffer: &mut [u8], offset: usize, value: u32) {
    buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub(super) fn write_u64(buffer: &mut [u8], offset: usize, value: u64) {
    buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

pub(super) fn output_start(buffer: &mut [u8]) -> usize {
    let end = buffer.len().min(DM_IOCTL_HEADER_SIZE);
    buffer[FIXED_PREFIX_SIZE..end].fill(0);
    write_u32(buffer, OFF_DATA_START, DM_IOCTL_HEADER_SIZE as u32);
    DM_IOCTL_HEADER_SIZE
}
