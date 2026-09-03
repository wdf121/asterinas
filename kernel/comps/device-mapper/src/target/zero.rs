// SPDX-License-Identifier: MPL-2.0

//! Linux Device Mapper `zero` target support.
//!
//! The zero target owns a logical sector range but has no backing device. Reads
//! synthesize zero-filled data, write-like operations complete successfully, and
//! table metadata still exposes a finite target range for split decisions.

use alloc::{string::String, vec::Vec};
use core::{fmt::Debug, ops::Range};

use aster_block::{BlockDevice, id::Sid};
use device_id::DeviceId;

use super::{
    DmTarget, DmTargetMetadata, TargetIoAction, TargetRange, TargetStatusMode, ZERO_METADATA,
};
use crate::TableError;

/// A `zero` target that owns a range and completes I/O without backing storage.
#[derive(Debug)]
pub struct ZeroTarget {
    /// Shared logical geometry used by table split and zero-complete decisions.
    range: TargetRange,
}

impl ZeroTarget {
    /// Parses Linux `zero` target parameters and builds the resolved target.
    pub(super) fn parse(logical_start: Sid, length: u64, params: &str) -> Result<Self, TableError> {
        if !params.trim().is_empty() {
            return Err(TableError::InvalidTargetParams);
        }
        Self::new(logical_start, length)
    }

    /// Builds a zero target after rejecting zero length and logical overflow.
    pub fn new(logical_start: Sid, length: u64) -> Result<Self, TableError> {
        Ok(Self {
            range: TargetRange::new(logical_start, length)?,
        })
    }

    /// Returns the logical range that is visible to table-level split decisions.
    pub fn logical_range(&self) -> &Range<Sid> {
        self.range.logical_range()
    }

    /// Returns the target length in 512-byte sectors for table/status output.
    pub fn length(&self) -> u64 {
        self.range.length()
    }

    /// Checks whether an I/O subrange should use this target's zero action.
    pub(super) fn contains_range(&self, logical: &Range<Sid>) -> bool {
        self.range.contains_range(logical)
    }
}

impl DmTarget for ZeroTarget {
    #[cfg(ktest)]
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn metadata(&self) -> DmTargetMetadata {
        ZERO_METADATA
    }

    fn logical_range(&self) -> &Range<Sid> {
        self.logical_range()
    }

    fn length(&self) -> u64 {
        self.length()
    }

    fn for_each_backing_id(&self, _f: &mut dyn FnMut(DeviceId)) {}

    fn for_each_backing<'a>(&'a self, _f: &mut dyn FnMut(&'a dyn BlockDevice)) {}

    fn status_params(&self, _mode: TargetStatusMode) -> Result<String, TableError> {
        Ok(String::new())
    }

    fn map_io_range(&self, logical: Range<Sid>) -> Option<Vec<TargetIoAction<'_>>> {
        if !self.contains_range(&logical) {
            return None;
        }
        Some(alloc::vec![TargetIoAction::Zero {
            logical_range: logical,
        }])
    }
}

#[cfg(ktest)]
mod tests {
    use ostd::prelude::ktest;

    use super::*;

    #[ktest]
    fn validates_target_range() {
        assert_eq!(
            ZeroTarget::new(Sid::new(0), 0).unwrap_err(),
            TableError::ZeroLength
        );
        assert_eq!(
            ZeroTarget::new(Sid::new(u64::MAX), 1).unwrap_err(),
            TableError::LogicalRangeOverflow
        );

        let target = ZeroTarget::new(Sid::new(4), 8).unwrap();
        assert_eq!(target.logical_range(), &(Sid::new(4)..Sid::new(12)));
        assert_eq!(target.length(), 8);
    }
}
