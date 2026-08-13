// SPDX-License-Identifier: MPL-2.0

pub(crate) use block::{
    open_count as block_open_count, register_mapper as register_block_mapper,
    rename_mapper as rename_block_mapper, unregister_mapper as unregister_block_mapper,
};
use device_id::DeviceId;

use crate::{
    device::{Device, DeviceType},
    prelude::*,
};

mod block;
pub mod char;

pub(super) fn init_in_first_kthread() {
    block::init_in_first_kthread();
}

pub(super) fn init_in_first_process() -> Result<()> {
    block::init_in_first_process()?;

    Ok(())
}

pub(crate) fn lookup(device_type: DeviceType, device_id: DeviceId) -> Option<Arc<dyn Device>> {
    match device_type {
        DeviceType::Char => char::lookup(device_id),
        DeviceType::Block => block::lookup(device_id),
    }
}
