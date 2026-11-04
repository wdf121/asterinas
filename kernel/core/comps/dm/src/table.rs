// SPDX-License-Identifier: MPL-2.0

//! Validated Device Mapper tables; I/O forwarding follows in RESUME.

use alloc::{boxed::Box, vec::Vec};

use aster_block::{BlockDevice, BlockDeviceLease, id::Sid};
use device_id::DeviceId;

use crate::{
    TableError,
    target::{DmTargetBox, linear::LinearTarget},
};

/// A contiguous, immutable Device Mapper table installed on a mapped device.
#[derive(Debug)]
pub struct DmTable {
    /// Ordered targets whose logical ranges must start at zero and be contiguous.
    targets: Vec<DmTargetBox>,
    /// Total table capacity in 512-byte sectors, equal to the last target end.
    length: u64,
    /// Linux `DM_READONLY_FLAG` mode captured when this table was loaded.
    readonly: bool,
}

impl DmTable {
    /// Creates a compatibility table for tests and legacy callers that still build linear lists.
    pub fn new_linear(targets: Vec<LinearTarget>) -> Result<Self, TableError> {
        Self::new_targets(
            targets
                .into_iter()
                .map(|target| Box::new(target) as DmTargetBox)
                .collect(),
        )
    }

    /// Creates a writable table after enforcing Linux-visible table invariants not owned by targets.
    pub fn new_targets(targets: Vec<DmTargetBox>) -> Result<Self, TableError> {
        Self::new_targets_with_readonly(targets, false)
    }

    /// Creates a table with the mode captured from `DM_TABLE_LOAD`.
    pub fn new_targets_with_readonly(
        targets: Vec<DmTargetBox>,
        readonly: bool,
    ) -> Result<Self, TableError> {
        if targets.is_empty() {
            return Err(TableError::UnsupportedTargetCount);
        }

        let mut has_unsupported_backing = false;
        let mut expected_start = 0_u64;
        for target in &targets {
            if target.logical_range().start.to_raw() != expected_start {
                return Err(TableError::UnsupportedLogicalStart);
            }
            let mut check_backing = |backing: &dyn BlockDevice| {
                if backing.downcast_ref::<crate::DmDevice>().is_some() {
                    has_unsupported_backing = true;
                }
            };
            target.for_each_backing(&mut check_backing);
            if has_unsupported_backing {
                return Err(TableError::UnsupportedBackingDevice);
            }
            expected_start = target.logical_range().end.to_raw();
        }

        Ok(Self {
            targets,
            length: expected_start,
            readonly,
        })
    }

    /// Creates a mapping table from a single `linear` target.
    pub fn new_single_linear(
        logical_start: Sid,
        length: u64,
        backing_start: Sid,
        backing: BlockDeviceLease,
    ) -> Result<Self, TableError> {
        Self::new_linear(alloc::vec![LinearTarget::new(
            logical_start,
            length,
            backing_start,
            backing,
        )?])
    }

    /// Returns the mapped device capacity in 512-byte sectors.
    pub fn length(&self) -> u64 {
        self.length
    }

    /// Returns whether this table rejects write-like BIOs after activation.
    pub fn is_readonly(&self) -> bool {
        self.readonly
    }

    /// Returns all targets.
    pub fn targets(&self) -> &[DmTargetBox] {
        &self.targets
    }

    /// Returns the number of targets.
    pub fn target_count(&self) -> usize {
        self.targets.len()
    }

    /// Returns unique backing block IDs in first-seen order for `DM_TABLE_DEPS`.
    pub fn backing_ids(&self) -> Vec<DeviceId> {
        let mut ids = Vec::new();
        for target in &self.targets {
            let mut collect_id = |id| {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            };
            target.for_each_backing_id(&mut collect_id);
        }
        ids
    }
}

#[cfg(ktest)]
mod tests {
    use alloc::{string::ToString, sync::Arc, vec};

    use aster_block::{
        BlockDeviceMeta,
        bio::{Bio, BioEnqueueError, BioType, SubmittedBio},
        lookup_lease, register, unregister,
    };
    use device_id::{MajorId, MinorId};
    use ostd::prelude::ktest;

    use super::*;
    use crate::DmManager;

    #[derive(Debug)]
    struct TestBlockDevice(DeviceId);

    impl BlockDevice for TestBlockDevice {
        fn enqueue(&self, _bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            unreachable!()
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 8,
                nr_sectors: 1024,
            }
        }

        fn name(&self) -> &str {
            "dm-table-test"
        }

        fn id(&self) -> DeviceId {
            self.0
        }
    }

    #[ktest]
    fn inactive_table_refuses_direct_bio() {
        let id = DeviceId::new(MajorId::new(510), MinorId::new(36));
        register(Arc::new(TestBlockDevice(id))).unwrap();
        let manager = DmManager::new().unwrap();
        let mapper = manager
            .create("inactive-io".to_string(), None, None)
            .unwrap();
        let table =
            DmTable::new_single_linear(Sid::new(0), 8, Sid::new(0), lookup_lease(id).unwrap())
                .unwrap();
        mapper.load_table(Arc::new(table));
        let bio = Bio::new(BioType::Flush, Sid::new(0), vec![], None);
        assert_eq!(
            bio.submit_and_wait(mapper.as_ref()),
            Err(BioEnqueueError::Refused)
        );
        assert_eq!(mapper.metadata().nr_sectors, 0);
        drop(manager);
        drop(mapper);
        unregister(id).unwrap();
    }

    #[ktest]
    fn inactive_table_replacement_releases_old_backing() {
        let old_id = DeviceId::new(MajorId::new(510), MinorId::new(34));
        let new_id = DeviceId::new(MajorId::new(510), MinorId::new(35));
        register(Arc::new(TestBlockDevice(old_id))).unwrap();
        register(Arc::new(TestBlockDevice(new_id))).unwrap();
        let manager = DmManager::new().unwrap();
        let mapper = manager
            .create("table-replace".to_string(), None, None)
            .unwrap();
        let make_table = |id| {
            Arc::new(
                DmTable::new_single_linear(Sid::new(0), 8, Sid::new(0), lookup_lease(id).unwrap())
                    .unwrap(),
            )
        };
        mapper.load_table(make_table(old_id));
        assert_eq!(unregister(old_id).unwrap_err(), aster_block::Error::Busy);
        mapper.load_table(make_table(new_id));
        unregister(old_id).unwrap();
        assert_eq!(unregister(new_id).unwrap_err(), aster_block::Error::Busy);
        drop(manager);
        drop(mapper);
        unregister(new_id).unwrap();
    }

    #[ktest]
    fn failed_candidate_preserves_existing_inactive_table() {
        let old_id = DeviceId::new(MajorId::new(510), MinorId::new(37));
        let candidate_id = DeviceId::new(MajorId::new(510), MinorId::new(38));
        register(Arc::new(TestBlockDevice(old_id))).unwrap();
        register(Arc::new(TestBlockDevice(candidate_id))).unwrap();
        let manager = DmManager::new().unwrap();
        let mapper = manager
            .create("table-failed-replace".to_string(), None, None)
            .unwrap();

        let old_table = Arc::new(
            DmTable::new_single_linear(Sid::new(0), 8, Sid::new(0), lookup_lease(old_id).unwrap())
                .unwrap(),
        );
        mapper.load_table(old_table.clone());
        let first = LinearTarget::new(
            Sid::new(0),
            8,
            Sid::new(0),
            lookup_lease(candidate_id).unwrap(),
        )
        .unwrap();
        let gap = LinearTarget::new(
            Sid::new(9),
            8,
            Sid::new(8),
            lookup_lease(candidate_id).unwrap(),
        )
        .unwrap();
        assert_eq!(
            DmTable::new_linear(vec![first, gap]).unwrap_err(),
            TableError::UnsupportedLogicalStart
        );
        assert!(Arc::ptr_eq(&mapper.inactive_table().unwrap(), &old_table));
        assert_eq!(mapper.inactive_table().unwrap().backing_ids(), vec![old_id]);
        unregister(candidate_id).unwrap();
        assert_eq!(unregister(old_id).unwrap_err(), aster_block::Error::Busy);
        drop(old_table);
        drop(manager);
        drop(mapper);
        unregister(old_id).unwrap();
    }

    #[ktest]
    fn holds_backing_leases_in_inactive_table_and_releases_on_failure() {
        let id = DeviceId::new(MajorId::new(510), MinorId::new(32));
        register(Arc::new(TestBlockDevice(id))).unwrap();
        let manager = DmManager::new().unwrap();
        let mapper = manager
            .create("table-test".to_string(), None, None)
            .unwrap();

        assert_eq!(
            LinearTarget::new(Sid::new(0), 2, Sid::new(1023), lookup_lease(id).unwrap())
                .unwrap_err(),
            TableError::BackingRangeOutOfBounds
        );
        let first =
            LinearTarget::new(Sid::new(0), 8, Sid::new(1), lookup_lease(id).unwrap()).unwrap();
        let gap =
            LinearTarget::new(Sid::new(9), 8, Sid::new(16), lookup_lease(id).unwrap()).unwrap();
        assert_eq!(
            DmTable::new_linear(vec![first, gap]).unwrap_err(),
            TableError::UnsupportedLogicalStart
        );
        // A rejected candidate drops both leases and leaves the inactive slot empty.
        assert!(mapper.inactive_table().is_none());
        assert!(unregister(id).is_ok());
        register(Arc::new(TestBlockDevice(id))).unwrap();

        let first =
            LinearTarget::new(Sid::new(0), 8, Sid::new(1), lookup_lease(id).unwrap()).unwrap();
        let second =
            LinearTarget::new(Sid::new(8), 8, Sid::new(16), lookup_lease(id).unwrap()).unwrap();
        let table = Arc::new(DmTable::new_linear(vec![first, second]).unwrap());
        assert_eq!(table.length(), 16);
        assert_eq!(table.target_count(), 2);
        assert_eq!(table.backing_ids(), vec![id]);
        mapper.load_table(table);
        assert_eq!(mapper.inactive_table().unwrap().length(), 16);
        assert_eq!(unregister(id).unwrap_err(), aster_block::Error::Busy);
        drop(manager);
        drop(mapper);
        unregister(id).unwrap();
    }
}
