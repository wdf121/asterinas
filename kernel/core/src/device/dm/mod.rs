// SPDX-License-Identifier: MPL-2.0

//! Device Mapper device entry points.

use aster_dm::DmManager;
use spin::Once;

mod control;
mod runtime;

static DM_MANAGER: Once<DmManager> = Once::new();

/// Starts table reclamation and reserves the mapper block major before exposing control.
///
/// The misc subsystem must already hold the character major used by the control device.
pub(super) fn init_in_first_kthread() {
    crate::thread::kernel_thread::ThreadOptions::new(|| aster_dm::run_table_reclaimer()).spawn();
    // Reserve the block major before exposing the control device to userspace.
    DM_MANAGER.call_once(|| DmManager::new().unwrap());
    control::init_in_first_kthread();
}
