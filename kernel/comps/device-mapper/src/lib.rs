// SPDX-License-Identifier: MPL-2.0

//! Asterinas Device Mapper core component.
//!
//! This crate supports mapping tables composed of `error`, `linear`, `striped`,
//! and `zero` targets. The Linux ioctl ABI and `/dev/mapper/control` live in the
//! kernel device layer; this component only owns mapping tables, device state,
//! and I/O forwarding.

#![no_std]
#![deny(unsafe_code)]

extern crate alloc;

mod device;
mod manager;
mod table;
pub mod target;

pub use device::{DmDevice, DmDeviceStatus};
pub use manager::DmManager;
pub use table::DmTable;

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
    /// The current device state does not allow the operation.
    InvalidState,
    /// The mapping table is invalid.
    InvalidTable(TableError),
}

impl From<TableError> for DmError {
    fn from(error: TableError) -> Self {
        Self::InvalidTable(error)
    }
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
    /// The `Bio` is not fully contained in a single target.
    BioOutOfRange,
}
