// SPDX-License-Identifier: MPL-2.0

//! Device Mapper identities and their device-number ownership.
//!
//! Creating a mapper here does not install a table or register a block device.

#![no_std]
#![deny(unsafe_code)]

extern crate alloc;

mod device;
mod manager;

pub use device::DmDevice;
pub use manager::DmManager;

/// Reasons why a core Device Mapper operation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmError {
    /// The name already exists.
    NameExists,
    /// The UUID already exists.
    UuidExists,
    /// The specified device was not found.
    DeviceNotFound,
    /// The specified minor number is busy.
    MinorBusy,
    /// Minor numbers are exhausted or out of range.
    MinorExhausted,
    /// Block device major numbers are exhausted.
    MajorExhausted,
}
