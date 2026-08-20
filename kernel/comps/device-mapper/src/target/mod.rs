// SPDX-License-Identifier: MPL-2.0

use core::ops::Range;

use aster_block::{BlockDevice, id::Sid};
use device_id::DeviceId;

use self::linear::LinearTarget;

pub mod linear;

#[derive(Debug)]
pub enum DmTarget {
    Linear(LinearTarget),
}

impl DmTarget {
    pub fn logical_range(&self) -> &Range<Sid> {
        match self {
            Self::Linear(target) => target.logical_range(),
        }
    }

    pub fn length(&self) -> u64 {
        match self {
            Self::Linear(target) => target.length(),
        }
    }

    pub fn backing_start(&self) -> Sid {
        match self {
            Self::Linear(target) => target.backing_start(),
        }
    }

    pub fn backing_id(&self) -> DeviceId {
        match self {
            Self::Linear(target) => target.backing_id(),
        }
    }

    pub fn backing(&self) -> &dyn BlockDevice {
        match self {
            Self::Linear(target) => target.backing(),
        }
    }

    pub fn map_sector(&self, logical: Sid) -> Option<Sid> {
        match self {
            Self::Linear(target) => target.map_sector(logical),
        }
    }
}
