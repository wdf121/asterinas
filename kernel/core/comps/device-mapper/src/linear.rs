// SPDX-License-Identifier: MPL-2.0

//! Fixed linear mappings over a single backing block device.
//!
//! This module validates mapping geometry and translates logical sector ranges
//! into backing-device ranges. Device registration and userspace control are
//! outside its scope.

use alloc::sync::Arc;

use aster_block::{
    BlockDevice, BlockDeviceMeta,
    bio::{BioEnqueueError, BioType, SubmittedBio},
    id::Sid,
};

/// A fixed linear mapping over a sector range of a backing block device.
#[derive(Debug)]
pub struct LinearTarget {
    backing: Arc<dyn BlockDevice>,
    backing_start: Sid,
    length: u64,
}

/// The error type returned when constructing a [`LinearTarget`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinearTargetError {
    /// The mapping length is zero.
    ZeroLength,
    /// Adding the mapping length to the start sector overflows.
    BackingRangeOverflow,
    /// The mapping range extends past the backing device's capacity.
    BackingRangeOutOfBounds,
}

impl LinearTarget {
    /// Constructs a new `LinearTarget`.
    ///
    /// The target maps `length` sectors starting at `backing_start` of the
    /// backing device. Returns an error if the length is zero or if the mapped
    /// range extends past the backing device's capacity.
    pub fn new(
        backing: Arc<dyn BlockDevice>,
        backing_start: Sid,
        length: u64,
    ) -> Result<Self, LinearTargetError> {
        if length == 0 {
            return Err(LinearTargetError::ZeroLength);
        }

        let backing_end = backing_start
            .to_raw()
            .checked_add(length)
            .ok_or(LinearTargetError::BackingRangeOverflow)?;

        let capacity = backing.metadata().nr_sectors as u64;

        if backing_end > capacity {
            return Err(LinearTargetError::BackingRangeOutOfBounds);
        }

        Ok(Self {
            backing,
            backing_start,
            length,
        })
    }

    pub(super) fn metadata(&self) -> BlockDeviceMeta {
        let mut metadata = self.backing.metadata();
        // Construction guarantees that the mapping lies within the backing
        // device, whose sector count fits in `usize`, so this cannot truncate.
        metadata.nr_sectors = self.length as usize;
        metadata
    }

    pub(super) fn enqueue(&self, mut bio: SubmittedBio) -> Result<(), BioEnqueueError> {
        match bio.type_() {
            BioType::Flush => return self.backing.enqueue(bio),
            BioType::Read | BioType::Write => {}
        }

        let range = bio.sid_range();
        let start = range.start.to_raw();
        let end = range.end.to_raw();

        if start >= end || end > self.length {
            return Err(BioEnqueueError::Refused);
        }

        bio.offset_mapped_sid_range(self.backing_start.to_raw())?;

        self.backing.enqueue(bio)
    }
}

#[cfg(ktest)]
mod tests {
    use alloc::vec;
    use core::ops::Range;

    use aster_block::{
        SECTOR_SIZE,
        bio::{Bio, BioDirection, BioSegment},
    };
    use device_id::{DeviceId, MajorId, MinorId};
    use ostd::{prelude::ktest, sync::Mutex};

    use super::*;

    #[derive(Debug)]
    struct GeometryBacking {
        nr_sectors: usize,
    }

    impl BlockDevice for GeometryBacking {
        fn enqueue(&self, _bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            panic!("geometry tests must not submit I/O");
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 16,
                nr_sectors: self.nr_sectors,
            }
        }

        fn name(&self) -> &str {
            "linear-geometry-test"
        }

        fn id(&self) -> DeviceId {
            DeviceId::new(MajorId::new(510), MinorId::new(1))
        }
    }

    #[ktest]
    fn accepts_mapping_ending_at_backing_capacity() {
        let backing = Arc::new(GeometryBacking { nr_sectors: 1000 });

        let target = match LinearTarget::new(backing, Sid::new(900), 100) {
            Ok(target) => target,
            Err(_) => panic!("a mapping ending at backing capacity must be valid"),
        };

        let metadata = target.metadata();
        assert_eq!(metadata.nr_sectors, 100);
        assert_eq!(metadata.max_nr_segments_per_bio, 16);
    }

    #[ktest]
    fn rejects_zero_length_mapping() {
        let backing = Arc::new(GeometryBacking { nr_sectors: 1000 });

        let err = match LinearTarget::new(backing, Sid::new(0), 0) {
            Err(err) => err,
            Ok(_) => panic!("a zero-length mapping must be rejected"),
        };

        assert!(matches!(err, LinearTargetError::ZeroLength));
    }

    #[ktest]
    fn rejects_overflowing_backing_range() {
        let backing = Arc::new(GeometryBacking { nr_sectors: 1000 });

        let err = match LinearTarget::new(backing, Sid::new(u64::MAX), 1) {
            Err(err) => err,
            Ok(_) => panic!("a start+length overflow must be rejected"),
        };

        assert!(matches!(err, LinearTargetError::BackingRangeOverflow));
    }

    #[ktest]
    fn rejects_mapping_beyond_backing_capacity() {
        let backing = Arc::new(GeometryBacking { nr_sectors: 1000 });

        let err = match LinearTarget::new(backing, Sid::new(900), 101) {
            Err(err) => err,
            Ok(_) => panic!("a mapping past backing capacity must be rejected"),
        };

        assert!(matches!(err, LinearTargetError::BackingRangeOutOfBounds));
    }

    /// Records the sector range of every enqueued BIO instead of submitting it.
    #[derive(Debug)]
    struct RecordingBacking {
        nr_sectors: usize,
        last_range: Mutex<Option<Range<Sid>>>,
    }

    impl RecordingBacking {
        fn new(nr_sectors: usize) -> Arc<Self> {
            Arc::new(Self {
                nr_sectors,
                last_range: Mutex::new(None),
            })
        }
    }

    impl BlockDevice for RecordingBacking {
        fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            *self.last_range.lock() = Some(bio.sid_range().clone());
            Ok(())
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 16,
                nr_sectors: self.nr_sectors,
            }
        }

        fn name(&self) -> &str {
            "linear-forwarding-test"
        }

        fn id(&self) -> DeviceId {
            DeviceId::new(MajorId::new(510), MinorId::new(2))
        }
    }

    fn recording_target() -> (LinearTarget, Arc<RecordingBacking>) {
        let backing = RecordingBacking::new(1000);
        let target = match LinearTarget::new(backing.clone(), Sid::new(900), 100) {
            Ok(target) => target,
            Err(_) => panic!("a mapping within backing capacity must be valid"),
        };
        (target, backing)
    }

    #[ktest]
    fn forwards_flush_beyond_mapping_to_backing() {
        let (target, backing) = recording_target();

        // A flush carries an empty range past the mapping end, which would fail
        // the data-range check; it must still be forwarded unchanged.
        let bio = Bio::new(BioType::Flush, Sid::new(5000), vec![], None).submit_for_test();

        target.enqueue(bio).unwrap();
        assert_eq!(
            *backing.last_range.lock(),
            Some(Sid::new(5000)..Sid::new(5000))
        );
    }

    #[ktest]
    fn remaps_data_bio_by_backing_start() {
        let (target, backing) = recording_target();

        let bio = Bio::new(
            BioType::Read,
            Sid::new(10),
            vec![BioSegment::alloc_exact(
                1,
                4 * SECTOR_SIZE,
                BioDirection::FromDevice,
            )],
            None,
        )
        .submit_for_test();

        target.enqueue(bio).unwrap();
        assert_eq!(
            *backing.last_range.lock(),
            Some(Sid::new(910)..Sid::new(914))
        );
    }

    #[ktest]
    fn accepts_write_bio_ending_at_mapping_end() {
        let (target, backing) = recording_target();

        // 96..100 ends exactly at the mapping length and must be accepted,
        // remapping onto the last sectors of the backing range.
        let bio = Bio::new(
            BioType::Write,
            Sid::new(96),
            vec![BioSegment::alloc_exact(
                1,
                4 * SECTOR_SIZE,
                BioDirection::ToDevice,
            )],
            None,
        )
        .submit_for_test();

        target.enqueue(bio).unwrap();
        assert_eq!(
            *backing.last_range.lock(),
            Some(Sid::new(996)..Sid::new(1000))
        );
    }

    #[ktest]
    fn refuses_data_bio_outside_mapping_range() {
        let (target, backing) = recording_target();

        // An empty range (`start >= end`) carries no data and must be refused.
        let empty = Bio::new(BioType::Write, Sid::new(0), vec![], None).submit_for_test();
        let err = match target.enqueue(empty) {
            Err(err) => err,
            Ok(()) => panic!("an empty data BIO must be refused"),
        };
        assert!(matches!(err, BioEnqueueError::Refused));

        // A range ending past the mapping length (97..101 > 100) must be refused.
        let beyond = Bio::new(
            BioType::Write,
            Sid::new(97),
            vec![BioSegment::alloc_exact(
                1,
                4 * SECTOR_SIZE,
                BioDirection::FromDevice,
            )],
            None,
        )
        .submit_for_test();
        let err = match target.enqueue(beyond) {
            Err(err) => err,
            Ok(()) => panic!("a BIO past the mapping end must be refused"),
        };
        assert!(matches!(err, BioEnqueueError::Refused));

        assert_eq!(*backing.last_range.lock(), None);
    }
}
