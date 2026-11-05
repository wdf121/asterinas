// SPDX-License-Identifier: MPL-2.0

//! Common copy, validation, secure wiping, and copyout for DM control commands.

use ostd::mm::VmIo;

use super::abi::{
    self, DM_DEV_CREATE_CMD, DM_DEV_SUSPEND_CMD, DM_TABLE_LOAD_CMD, FIXED_PREFIX_SIZE,
    OFF_DATA_SIZE, OFF_FLAGS, SECURE_DATA_FLAG,
};
use crate::{context::current_userspace, prelude::*};

/// Owns copied ioctl input and wipes the full payload of secure requests.
struct IoctlBuffer {
    bytes: Vec<u8>,
    secure: bool,
}

impl Drop for IoctlBuffer {
    fn drop(&mut self) {
        if self.secure {
            let len = self.bytes.len();
            VmWriter::from(self.bytes.as_mut_slice()).fill_zeros(len);
        }
    }
}

/// Copies a bounded LOAD payload, or only the prefix for CREATE and RESUME, before dispatch.
pub(super) fn process_ioctl_parameters(
    arg: usize,
    version: &[u8; 12],
    command: u8,
    dispatch: impl FnOnce(&mut [u8]) -> Result<()>,
) -> Result<i32> {
    let mut header = [0u8; FIXED_PREFIX_SIZE];
    header[..version.len()].copy_from_slice(version);
    let addr = arg.checked_add(version.len()).ok_or_else(|| {
        Error::with_message(Errno::EFAULT, "device-mapper ioctl address overflow")
    })?;
    current_userspace!().read_bytes(addr, &mut header[version.len()..])?;
    let input_size = abi::read_u32(&header, OFF_DATA_SIZE);
    abi::validate_ioctl_size(input_size)?;
    let size = if command == DM_TABLE_LOAD_CMD {
        input_size as usize
    } else {
        debug_assert!(matches!(command, DM_DEV_CREATE_CMD | DM_DEV_SUSPEND_CMD));
        FIXED_PREFIX_SIZE
    };
    let secure = abi::read_u32(&header, OFF_FLAGS) & SECURE_DATA_FLAG != 0;
    let mut bytes = Vec::new();
    if bytes.try_reserve_exact(size).is_err() {
        if secure {
            current_userspace!()
                .writer(arg, size)?
                .fill_zeros(size)
                .map_err(|(error, _)| error)?;
        }
        return_errno_with_message!(Errno::ENOMEM, "cannot allocate mapper ioctl buffer");
    }
    bytes.resize(size, 0);
    let mut buffer = IoctlBuffer { bytes, secure };
    buffer.bytes[..header.len()].copy_from_slice(&header);
    if size > FIXED_PREFIX_SIZE {
        let payload_addr = arg.checked_add(FIXED_PREFIX_SIZE).ok_or_else(|| {
            Error::with_message(Errno::EFAULT, "device-mapper ioctl address overflow")
        })?;
        current_userspace!().read_bytes(payload_addr, &mut buffer.bytes[FIXED_PREFIX_SIZE..])?;
    }
    if secure {
        current_userspace!()
            .writer(arg, size)?
            .fill_zeros(size)
            .map_err(|(error, _)| error)?;
    }
    abi::validate_ioctl_params(
        buffer.bytes[..FIXED_PREFIX_SIZE]
            .as_mut()
            .try_into()
            .unwrap(),
        command,
    )?;
    abi::write_u32(&mut buffer.bytes, OFF_DATA_SIZE, FIXED_PREFIX_SIZE as u32);
    dispatch(&mut buffer.bytes)?;
    current_userspace!().write_bytes(arg, &buffer.bytes[..FIXED_PREFIX_SIZE])?;
    Ok(0)
}
