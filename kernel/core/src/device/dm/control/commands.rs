// SPDX-License-Identifier: MPL-2.0

//! Dispatch of the supported Device Mapper control commands.

use super::{abi::*, device, table, target};
use crate::prelude::*;

type CommandHandler = fn(&mut [u8]) -> Result<()>;

/// Selects only commands implemented by this control device.
pub(super) fn handler_for(command: u8) -> Result<CommandHandler> {
    match command {
        DM_DEV_CREATE_CMD => Ok(device::create_device),
        DM_DEV_REMOVE_CMD => Ok(device::remove_device),
        DM_LIST_DEVICES_CMD => Ok(device::list_devices),
        DM_LIST_VERSIONS_CMD => Ok(target::list_versions),
        DM_GET_TARGET_VERSION_CMD => Ok(target::get_target_version),
        DM_TABLE_DEPS_CMD => Ok(table::table_deps),
        DM_DEV_STATUS_CMD => Ok(device::device_status),
        DM_TABLE_STATUS_CMD => Ok(table::table_status),
        DM_DEV_SUSPEND_CMD => Ok(table::resume_device),
        DM_TABLE_LOAD_CMD => Ok(table::table_load),
        DM_TABLE_CLEAR_CMD => Ok(table::table_clear),
        _ => return_errno_with_message!(Errno::ENOTTY, "unsupported device-mapper ioctl command"),
    }
}
