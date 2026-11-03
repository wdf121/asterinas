// SPDX-License-Identifier: MPL-2.0

//! Common copy, validation, secure wiping, and copyout for DM control commands.

use ostd::mm::VmIo;

use super::abi::{self, FIXED_PREFIX_SIZE, OFF_DATA_SIZE, OFF_FLAGS, SECURE_DATA_FLAG};
use crate::{context::current_userspace, prelude::*};

struct IoctlBuffer {
    bytes: [u8; FIXED_PREFIX_SIZE],
    secure: bool,
}

impl Drop for IoctlBuffer {
    fn drop(&mut self) {
        if self.secure {
            VmWriter::from(self.bytes.as_mut_slice()).fill_zeros(FIXED_PREFIX_SIZE);
        }
    }
}

/// Runs after the entry point has completed the version handshake and selected
/// a supported command. Only fixed-prefix commands are supported at this stage;
/// commands with variable-length parameters need an extended copy path.
pub(super) fn process_ioctl_parameters(
    arg: usize,
    version: &[u8; 12],
    dispatch: impl FnOnce(&mut [u8; FIXED_PREFIX_SIZE]) -> Result<()>,
) -> Result<i32> {
    let mut buffer = IoctlBuffer {
        bytes: [0; FIXED_PREFIX_SIZE],
        secure: false,
    };
    buffer.bytes[..version.len()].copy_from_slice(version);
    let addr = arg.checked_add(version.len()).ok_or_else(|| {
        Error::with_message(Errno::EFAULT, "device-mapper ioctl address overflow")
    })?;
    current_userspace!().read_bytes(addr, &mut buffer.bytes[version.len()..])?;
    abi::validate_ioctl_size(abi::read_u32(&buffer.bytes, OFF_DATA_SIZE))?;

    buffer.secure = abi::read_u32(&buffer.bytes, OFF_FLAGS) & SECURE_DATA_FLAG != 0;
    if buffer.secure {
        current_userspace!()
            .writer(arg, FIXED_PREFIX_SIZE)?
            .fill_zeros(FIXED_PREFIX_SIZE)
            .map_err(|(error, _)| error)?;
    }
    abi::validate_ioctl_params(&mut buffer.bytes)?;
    abi::write_u32(&mut buffer.bytes, OFF_DATA_SIZE, FIXED_PREFIX_SIZE as u32);
    dispatch(&mut buffer.bytes)?;
    current_userspace!().write_bytes(arg, &buffer.bytes)?;
    Ok(0)
}
