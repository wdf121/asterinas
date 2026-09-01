// SPDX-License-Identifier: MPL-2.0

use core::ops::Range;

use aster_block::{BlockDevice, id::Sid};
use device_id::DeviceId;

use self::{error::ErrorTarget, linear::LinearTarget, striped::StripedTarget, zero::ZeroTarget};

pub mod error;
pub mod linear;
pub mod striped;
pub mod zero;

#[derive(Debug)]
pub enum DmTarget {
    Error(ErrorTarget),
    Linear(LinearTarget),
    Striped(StripedTarget),
    Zero(ZeroTarget),
}

impl DmTarget {
    pub fn logical_range(&self) -> &Range<Sid> {
        match self {
            Self::Error(target) => target.logical_range(),
            Self::Linear(target) => target.logical_range(),
            Self::Striped(target) => target.logical_range(),
            Self::Zero(target) => target.logical_range(),
        }
    }

    pub fn length(&self) -> u64 {
        match self {
            Self::Error(target) => target.length(),
            Self::Linear(target) => target.length(),
            Self::Striped(target) => target.length(),
            Self::Zero(target) => target.length(),
        }
    }

    pub fn backing_start(&self) -> Option<Sid> {
        match self {
            Self::Linear(target) => Some(target.backing_start()),
            Self::Error(_) | Self::Striped(_) | Self::Zero(_) => None,
        }
    }

    pub fn backing_id(&self) -> Option<DeviceId> {
        match self {
            Self::Linear(target) => Some(target.backing_id()),
            Self::Error(_) | Self::Striped(_) | Self::Zero(_) => None,
        }
    }

    pub fn for_each_backing_id(&self, mut f: impl FnMut(DeviceId)) {
        match self {
            Self::Error(_) | Self::Zero(_) => {}
            Self::Linear(target) => f(target.backing_id()),
            Self::Striped(target) => target.for_each_backing_id(f),
        }
    }

    pub fn backing(&self) -> Option<&dyn BlockDevice> {
        match self {
            Self::Linear(target) => Some(target.backing()),
            Self::Error(_) | Self::Striped(_) | Self::Zero(_) => None,
        }
    }

    pub fn for_each_backing<'a>(&'a self, mut f: impl FnMut(&'a dyn BlockDevice)) {
        match self {
            Self::Error(_) | Self::Zero(_) => {}
            Self::Linear(target) => f(target.backing()),
            Self::Striped(target) => target.for_each_backing(f),
        }
    }

    pub fn map_sector(&self, logical: Sid) -> Option<Sid> {
        match self {
            Self::Linear(target) => target.map_sector(logical),
            Self::Error(_) | Self::Striped(_) | Self::Zero(_) => None,
        }
    }

    pub fn is_linear(&self) -> bool {
        matches!(self, Self::Linear(_))
    }
}
