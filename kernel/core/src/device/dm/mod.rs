// SPDX-License-Identifier: MPL-2.0

//! Device Mapper device entry points.

mod control;

pub(super) fn init_in_first_kthread() {
    control::init_in_first_kthread();
}
