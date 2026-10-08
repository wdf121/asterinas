// SPDX-License-Identifier: MPL-2.0

use align_ext::AlignExt;
use io_util::batch::IoBatch;
use ostd::mm::{VmIo, VmReader, VmWriter};

use super::{
    BLOCK_SIZE, BlockDevice, SECTOR_SIZE,
    bio::{Bio, BioCompleteFn, BioEnqueueError, BioSegment, BioStatus, BioType},
    id::{Bid, Sid},
};
use crate::{
    bio::{BioDirection, is_sector_aligned},
    prelude::*,
};

/// Implements several commonly used APIs for the block device to conveniently
/// read and write block(s).
impl dyn BlockDevice {
    /// Synchronously reads contiguous blocks starting from the `bid`.
    pub fn read_blocks(
        &self,
        bid: Bid,
        bio_segments: Vec<BioSegment>,
    ) -> Result<BioStatus, BioEnqueueError> {
        let bio = Bio::new(BioType::Read, Sid::from(bid), bio_segments, None);
        let status = bio.submit_and_wait(self)?;
        Ok(status)
    }

    /// Asynchronously reads contiguous blocks into multiple memory segments.
    pub fn read_blocks_async(
        &self,
        bid: Bid,
        bio_segments: Vec<BioSegment>,
        complete_fn: Option<BioCompleteFn>,
        io_batch: &mut IoBatch,
    ) -> Result<(), BioEnqueueError> {
        let bio = Bio::new(BioType::Read, Sid::from(bid), bio_segments, complete_fn);
        bio.submit(self, io_batch)
    }

    /// Synchronously writes contiguous blocks starting from the `bid`.
    pub fn write_blocks(
        &self,
        bid: Bid,
        bio_segments: Vec<BioSegment>,
    ) -> Result<BioStatus, BioEnqueueError> {
        let bio = Bio::new(BioType::Write, Sid::from(bid), bio_segments, None);
        let status = bio.submit_and_wait(self)?;
        Ok(status)
    }

    /// Asynchronously writes contiguous blocks starting from the `bid`.
    pub fn write_blocks_async(
        &self,
        bid: Bid,
        bio_segments: Vec<BioSegment>,
        complete_fn: Option<BioCompleteFn>,
        io_batch: &mut IoBatch,
    ) -> Result<(), BioEnqueueError> {
        let bio = Bio::new(BioType::Write, Sid::from(bid), bio_segments, complete_fn);
        bio.submit(self, io_batch)
    }

    /// Synchronously discards contiguous sectors starting from the `sid`.
    pub fn discard_sectors(&self, sid: Sid, nsectors: u64) -> Result<BioStatus, BioEnqueueError> {
        let bio = Bio::new_range(BioType::Discard, sid, nsectors, None);
        let status = bio.submit_and_wait(self)?;
        Ok(status)
    }

    /// Asynchronously discards contiguous sectors starting from the `sid`.
    pub fn discard_sectors_async(
        &self,
        sid: Sid,
        nsectors: u64,
        complete_fn: Option<BioCompleteFn>,
        io_batch: &mut IoBatch,
    ) -> Result<(), BioEnqueueError> {
        let bio = Bio::new_range(BioType::Discard, sid, nsectors, complete_fn);
        bio.submit(self, io_batch)
    }

    /// Synchronously writes zeroes to contiguous sectors starting from the `sid`.
    pub fn write_zeroes_sectors(
        &self,
        sid: Sid,
        nsectors: u64,
    ) -> Result<BioStatus, BioEnqueueError> {
        let bio = Bio::new_range(BioType::WriteZeroes, sid, nsectors, None);
        let status = bio.submit_and_wait(self)?;
        Ok(status)
    }

    /// Asynchronously writes zeroes to contiguous sectors starting from the `sid`.
    pub fn write_zeroes_sectors_async(
        &self,
        sid: Sid,
        nsectors: u64,
        complete_fn: Option<BioCompleteFn>,
        io_batch: &mut IoBatch,
    ) -> Result<(), BioEnqueueError> {
        let bio = Bio::new_range(BioType::WriteZeroes, sid, nsectors, complete_fn);
        bio.submit(self, io_batch)
    }

    /// Issues a sync request
    pub fn sync(&self) -> Result<BioStatus, BioEnqueueError> {
        let bio = Bio::new(BioType::Flush, Sid::from(Bid::from_offset(0)), vec![], None);
        let status = bio.submit_and_wait(self)?;
        Ok(status)
    }
}

impl VmIo for dyn BlockDevice {
    /// Reads consecutive bytes of several sectors in size.
    fn read(&self, offset: usize, writer: &mut VmWriter) -> ostd::Result<()> {
        let read_len = writer.avail();
        if read_len == 0 {
            return Ok(());
        }

        let request_end = offset.checked_add(read_len).ok_or(ostd::Error::Overflow)?;
        let device_size = self.metadata().nr_sectors * SECTOR_SIZE;
        if request_end > device_size {
            return Err(ostd::Error::InvalidArgs);
        }

        let aligned_offset = offset.align_down(SECTOR_SIZE);
        let aligned_end = request_end.align_up(SECTOR_SIZE);
        let aligned_len = aligned_end - aligned_offset;

        let (bio, bio_segment) = {
            let num_blocks = {
                let first = Bid::from_offset(aligned_offset).to_raw();
                let last = Bid::from_offset(aligned_end - 1).to_raw();
                (last - first + 1) as usize
            };
            let bio_segment = BioSegment::alloc_inner(
                num_blocks,
                aligned_offset % BLOCK_SIZE,
                aligned_len,
                BioDirection::FromDevice,
            );

            (
                Bio::new(
                    BioType::Read,
                    Sid::from_offset(aligned_offset),
                    vec![bio_segment.clone()],
                    None,
                ),
                bio_segment,
            )
        };

        let status = bio.submit_and_wait(self)?;
        match status {
            BioStatus::Complete => {
                let segment_offset = offset - aligned_offset;
                bio_segment.read(segment_offset, writer)?;

                Ok(())
            }
            _ => Err(ostd::Error::IoError),
        }
    }

    /// Writes consecutive bytes of several sectors in size.
    fn write(&self, offset: usize, reader: &mut VmReader) -> ostd::Result<()> {
        let write_len = reader.remain();
        if write_len == 0 {
            return Ok(());
        }

        let request_end = offset.checked_add(write_len).ok_or(ostd::Error::Overflow)?;
        let device_size = self.metadata().nr_sectors * SECTOR_SIZE;
        if request_end > device_size {
            return Err(ostd::Error::InvalidArgs);
        }

        let aligned_offset = offset.align_down(SECTOR_SIZE);
        let aligned_end = request_end.align_up(SECTOR_SIZE);

        // If the write range is not sector-aligned, preserve the bytes in the
        // surrounding sectors that are outside the user-requested range.
        // The request is split into at most three segments: a read-modify-write
        // first sector, sector-aligned middle sectors written directly from the
        // reader, and a read-modify-write last sector. Each segment consumes
        // only the bytes that belong to it so later segments see the remaining
        // input bytes.

        let need_read_first_sector = !is_sector_aligned(offset);
        let mut middle_sector_offset = aligned_offset;
        let last_sector_offset = (request_end - 1).align_down(SECTOR_SIZE);
        let need_read_last_sector = {
            let is_last_sector_aligned = is_sector_aligned(request_end);
            let is_the_same_sector = last_sector_offset == aligned_offset;
            !(is_last_sector_aligned || need_read_first_sector && is_the_same_sector)
        };
        let middle_end = if need_read_last_sector {
            last_sector_offset
        } else {
            aligned_end
        };

        let mut bio_segments = Vec::new();

        if need_read_first_sector {
            let first_segment = self.read_sector_for_write(aligned_offset)?;
            let first_segment_offset = offset - aligned_offset;
            let first_write_len = (SECTOR_SIZE - first_segment_offset).min(write_len);
            let mut first_reader = reader.clone();
            first_reader.limit(first_write_len);
            first_segment.write(first_segment_offset, &mut first_reader)?;
            reader.skip(first_write_len);
            bio_segments.push(first_segment);
            middle_sector_offset += SECTOR_SIZE;
        }
        if middle_sector_offset < middle_end {
            let middle_len = middle_end - middle_sector_offset;
            let middle_segment = alloc_write_segment(middle_sector_offset, middle_len);
            let mut middle_reader = reader.clone();
            middle_reader.limit(middle_len);
            middle_segment.write(0, &mut middle_reader)?;
            reader.skip(middle_len);
            bio_segments.push(middle_segment);
        }
        if need_read_last_sector {
            let last_segment = self.read_sector_for_write(last_sector_offset)?;
            last_segment.write(0, reader)?;
            bio_segments.push(last_segment);
        }
        debug_assert!(!reader.has_remain());

        let bio = Bio::new(
            BioType::Write,
            Sid::from_offset(aligned_offset),
            bio_segments,
            None,
        );
        let status = bio.submit_and_wait(self)?;
        match status {
            BioStatus::Complete => Ok(()),
            _ => Err(ostd::Error::IoError),
        }
    }
}

impl dyn BlockDevice {
    /// Asynchronously writes consecutive bytes of several sectors in size.
    pub fn write_bytes_async(
        &self,
        offset: usize,
        buf: &[u8],
        io_batch: &mut IoBatch,
    ) -> ostd::Result<()> {
        let write_len = buf.len();
        if !is_sector_aligned(offset) || !is_sector_aligned(write_len) {
            return Err(ostd::Error::InvalidArgs);
        }
        if write_len == 0 {
            return Ok(());
        }

        let bio = {
            let num_blocks = {
                let first = Bid::from_offset(offset).to_raw();
                let last = Bid::from_offset(offset + write_len - 1).to_raw();
                (last - first + 1) as usize
            };
            let bio_segment = BioSegment::alloc_inner(
                num_blocks,
                offset % BLOCK_SIZE,
                write_len,
                BioDirection::ToDevice,
            );
            bio_segment.write(0, &mut VmReader::from(buf).to_fallible())?;
            Bio::new(
                BioType::Write,
                Sid::from_offset(offset),
                vec![bio_segment],
                None,
            )
        };

        bio.submit(self, io_batch)?;
        Ok(())
    }

    fn read_sector_for_write(&self, sector_offset: usize) -> ostd::Result<BioSegment> {
        // The segment will be submitted by the later write bio, so keep it writable
        // from the CPU side and read the preserved sector directly into it.
        let write_segment = alloc_write_segment(sector_offset, SECTOR_SIZE);
        let read_bio = Bio::new(
            BioType::Read,
            Sid::from_offset(sector_offset),
            vec![write_segment.clone()],
            None,
        );
        if read_bio.submit_and_wait(self)? != BioStatus::Complete {
            return Err(ostd::Error::IoError);
        }

        Ok(write_segment)
    }
}

fn alloc_write_segment(offset: usize, len: usize) -> BioSegment {
    let num_blocks = {
        let first = Bid::from_offset(offset).to_raw();
        let last = Bid::from_offset(offset + len - 1).to_raw();
        (last - first + 1) as usize
    };

    BioSegment::alloc_inner(num_blocks, offset % BLOCK_SIZE, len, BioDirection::ToDevice)
}

pub(super) fn general_complete_fn(
    bio_type: BioType,
    bio_status: BioStatus,
    complete_fn: Option<BioCompleteFn>,
) {
    if bio_status != BioStatus::Complete {
        ostd::error!(
            "failed to do {:?} on the device with error status: {:?}",
            bio_type,
            bio_status
        );
    }
    if let Some(complete_fn) = complete_fn {
        complete_fn(bio_status);
    }
}

#[cfg(ktest)]
mod tests {
    use alloc::boxed::Box;

    use device_id::{DeviceId, MajorId, MinorId};
    use io_util::IoError;
    use ostd::{prelude::ktest, sync::Mutex};

    use super::*;
    use crate::{BlockDeviceMeta, bio::SubmittedBio};

    #[derive(Clone, Copy, Debug)]
    enum EnqueueMode {
        Reject,
        Complete(BioStatus),
        Defer,
    }

    #[derive(Debug, PartialEq)]
    struct RecordedBio {
        type_: BioType,
        range: Range<Sid>,
        segment_count: usize,
    }

    #[derive(Debug)]
    struct RangeBlockDevice {
        mode: EnqueueMode,
        recorded: Mutex<Vec<RecordedBio>>,
        pending: Mutex<Vec<SubmittedBio>>,
    }

    impl RangeBlockDevice {
        fn new(mode: EnqueueMode) -> Arc<Self> {
            Arc::new(Self {
                mode,
                recorded: Mutex::new(Vec::new()),
                pending: Mutex::new(Vec::new()),
            })
        }

        fn complete_next(&self, status: BioStatus) {
            self.pending.lock().remove(0).complete(status);
        }
    }

    impl BlockDevice for RangeBlockDevice {
        fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            if matches!(self.mode, EnqueueMode::Reject) {
                return Err(BioEnqueueError::Refused);
            }

            self.recorded.lock().push(RecordedBio {
                type_: bio.type_(),
                range: bio.sid_range().clone(),
                segment_count: bio.segments().len(),
            });
            match self.mode {
                EnqueueMode::Complete(status) => bio.complete(status),
                EnqueueMode::Defer => self.pending.lock().push(bio),
                EnqueueMode::Reject => unreachable!(),
            }
            Ok(())
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 8,
                nr_sectors: 1_024,
            }
        }

        fn name(&self) -> &str {
            "range-wrapper-test"
        }

        fn id(&self) -> DeviceId {
            DeviceId::new(MajorId::new(510), MinorId::new(1))
        }
    }

    #[ktest]
    fn synchronous_range_wrappers_submit_expected_bios() {
        let device = RangeBlockDevice::new(EnqueueMode::Complete(BioStatus::Complete));

        assert_eq!(
            (device.as_ref() as &dyn BlockDevice)
                .discard_sectors(Sid::new(8), 4)
                .unwrap(),
            BioStatus::Complete
        );
        assert_eq!(
            (device.as_ref() as &dyn BlockDevice)
                .write_zeroes_sectors(Sid::new(20), 6)
                .unwrap(),
            BioStatus::Complete
        );
        assert_eq!(
            *device.recorded.lock(),
            vec![
                RecordedBio {
                    type_: BioType::Discard,
                    range: Sid::new(8)..Sid::new(12),
                    segment_count: 0,
                },
                RecordedBio {
                    type_: BioType::WriteZeroes,
                    range: Sid::new(20)..Sid::new(26),
                    segment_count: 0,
                },
            ]
        );
    }

    #[ktest]
    fn asynchronous_range_wrappers_share_batch_and_complete_once() {
        let device = RangeBlockDevice::new(EnqueueMode::Defer);
        let completions = Arc::new(Mutex::new(Vec::new()));
        let mut batch = IoBatch::with_capacity(2);

        let discard_completions = completions.clone();
        (device.as_ref() as &dyn BlockDevice)
            .discard_sectors_async(
                Sid::new(32),
                3,
                Some(Box::new(move |status| {
                    discard_completions.lock().push(status);
                })),
                &mut batch,
            )
            .unwrap();
        let write_zeroes_completions = completions.clone();
        (device.as_ref() as &dyn BlockDevice)
            .write_zeroes_sectors_async(
                Sid::new(48),
                5,
                Some(Box::new(move |status| {
                    write_zeroes_completions.lock().push(status);
                })),
                &mut batch,
            )
            .unwrap();

        assert_eq!(batch.len(), 2);
        assert!(completions.lock().is_empty());
        assert_eq!(
            *device.recorded.lock(),
            vec![
                RecordedBio {
                    type_: BioType::Discard,
                    range: Sid::new(32)..Sid::new(35),
                    segment_count: 0,
                },
                RecordedBio {
                    type_: BioType::WriteZeroes,
                    range: Sid::new(48)..Sid::new(53),
                    segment_count: 0,
                },
            ]
        );

        device.complete_next(BioStatus::Complete);
        assert_eq!(*completions.lock(), vec![BioStatus::Complete]);
        device.complete_next(BioStatus::Complete);
        batch.wait_all().unwrap();
        assert_eq!(
            *completions.lock(),
            vec![BioStatus::Complete, BioStatus::Complete]
        );
    }

    #[ktest]
    fn range_wrappers_distinguish_enqueue_and_completion_errors() {
        let rejecting = RangeBlockDevice::new(EnqueueMode::Reject);
        let mut rejected_batch = IoBatch::new();
        assert_eq!(
            (rejecting.as_ref() as &dyn BlockDevice).discard_sectors(Sid::new(1), 1),
            Err(BioEnqueueError::Refused)
        );
        assert_eq!(
            (rejecting.as_ref() as &dyn BlockDevice).write_zeroes_sectors_async(
                Sid::new(2),
                1,
                None,
                &mut rejected_batch,
            ),
            Err(BioEnqueueError::Refused)
        );
        assert!(rejected_batch.is_empty());
        assert!(rejecting.recorded.lock().is_empty());

        let completing = RangeBlockDevice::new(EnqueueMode::Complete(BioStatus::IoError));
        assert_eq!(
            (completing.as_ref() as &dyn BlockDevice)
                .write_zeroes_sectors(Sid::new(4), 2)
                .unwrap(),
            BioStatus::IoError
        );

        let deferred = RangeBlockDevice::new(EnqueueMode::Defer);
        let completions = Arc::new(Mutex::new(Vec::new()));
        let callback_completions = completions.clone();
        let mut batch = IoBatch::new();
        (deferred.as_ref() as &dyn BlockDevice)
            .discard_sectors_async(
                Sid::new(6),
                2,
                Some(Box::new(move |status| {
                    callback_completions.lock().push(status);
                })),
                &mut batch,
            )
            .unwrap();
        assert!(completions.lock().is_empty());

        deferred.complete_next(BioStatus::IoError);
        assert_eq!(batch.wait_all(), Err(IoError::Failed));
        assert_eq!(*completions.lock(), vec![BioStatus::IoError]);
    }
}
