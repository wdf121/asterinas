// SPDX-License-Identifier: MPL-2.0

//! Minimal Device Mapper support.
//!
//! This crate provides fixed, single-target linear mappings over existing
//! block devices. Device registration and the userspace control interface
//! are outside its scope.

#![no_std]
#![deny(unsafe_code)]

extern crate alloc;
mod device;
mod linear;

pub use device::DmDevice;
pub use linear::{LinearTarget, LinearTargetError};
