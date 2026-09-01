// SPDX-License-Identifier: MPL-2.0

//! Misc devices.
//!
//! Character device with major number 10.

use device_id::MajorId;
use spin::Once;

use super::registry::char::{MajorIdOwner, acquire_major};

mod device_mapper;
mod hwrng;
#[cfg(all(target_arch = "x86_64", feature = "cvm_guest"))]
pub(crate) mod tdxguest;

static MISC_MAJOR: Once<MajorIdOwner> = Once::new();

// 内核启动后  初始化misc设备子系统 默认占用major 10
pub(super) fn init_in_first_kthread() {
    MISC_MAJOR.call_once(|| acquire_major(MajorId::new(10)).unwrap());

    //把device mapper control 作为 misc子设备接入 启动期初始化
    device_mapper::init_in_first_kthread();
    hwrng::init_in_first_kthread();

    #[cfg(target_arch = "x86_64")]
    ostd::if_tdx_enabled!({
        tdxguest::init().unwrap();
    });
}
