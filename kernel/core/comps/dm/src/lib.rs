// SPDX-License-Identifier: MPL-2.0

//! Device Mapper identities, device-number ownership, and linear table activation.
//!
//! Creating a mapper here does not install a table or register a block device.

#![no_std]
#![deny(unsafe_code)]

extern crate alloc;

mod device;
mod manager;
mod table;
pub mod target;

pub use device::DmDevice;
pub use manager::DmManager;
pub use table::DmTable;

/// Reasons why a core Device Mapper operation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmError {
    /// The requested table transition cannot be performed in the current state.
    InvalidState,
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

/// Reasons why mapping table validation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableError {
    /// A mapping table needs at least one target.
    UnsupportedTargetCount,
    /// Logical targets must start at sector 0 and be contiguous.
    UnsupportedLogicalStart,
    /// Target length cannot be zero.
    ZeroLength,
    /// The logical range overflowed.
    LogicalRangeOverflow,
    /// The backing range overflowed.
    BackingRangeOverflow,
    /// The backing range exceeds the block device capacity.
    BackingRangeOutOfBounds,
    /// The backing device type is not currently supported.
    UnsupportedBackingDevice,
    /// Target parameters or geometry are invalid.
    InvalidTargetParams,
}
