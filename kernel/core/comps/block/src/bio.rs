// SPDX-License-Identifier: MPL-2.0

use alloc::boxed::Box;

use align_ext::AlignExt;
use aster_util::mem_obj_slice::Slice;
use bitvec::array::BitArray;
use int_to_c_enum::TryFromInt;
use io_util::{
    IoError,
    batch::{IoBatch, IoCompletion},
};
use ostd::{
    Error,
    mm::{
        HasSize, Infallible, USegment, VmReader, VmWriter,
        dma::DmaStream,
        io::util::{HasVmReaderWriter, VmReaderWriterResult},
    },
    sync::{LocalIrqDisabled, SpinLock, WaitQueue},
};
use spin::Once;

use super::{BlockDevice, id::Sid};
use crate::{BLOCK_SIZE, SECTOR_SIZE, impl_block_device::general_complete_fn, prelude::*};

/// The unit for block I/O.
///
/// Each `Bio` packs the following information:
/// (1) The type of the I/O,
/// (2) The target sectors on the device for doing I/O,
/// (3) The memory locations (`BioSegment`) from/to which data are read/written,
/// (4) The optional callback function that will be invoked when the I/O is completed.
///
/// Before submission, a `Bio` owns its segments and completion callback.
/// After submission, that ownership is transferred to `SubmittedBio`.
pub struct Bio {
    metadata: Arc<BioMetadata>,
    complete_fn: Option<BioCompleteFn>,
    segments: Vec<BioSegment>,
}

/// The completion function type for BIO operations.
///
/// The function receives the final `BioStatus` of the I/O operation.
pub type BioCompleteFn = Box<dyn FnOnce(BioStatus) + Send>;

impl Bio {
    /// Constructs a new `Bio`.
    ///
    /// The `type_` describes the type of the I/O.
    /// The `start_sid` is the starting sector id on the device.
    /// The `segments` describes the memory segments.
    /// The `complete_fn` is the optional callback function that will be invoked
    /// when the I/O is completed, receiving the final `BioStatus`.
    pub fn new(
        type_: BioType,
        start_sid: Sid,
        segments: Vec<BioSegment>,
        complete_fn: Option<BioCompleteFn>,
    ) -> Self {
        let nsectors = segments
            .iter()
            .map(|segment| segment.nsectors().to_raw())
            .try_fold(0_u64, u64::checked_add)
            .expect("BIO sector count overflow");

        Self::new_with_nsectors_unchecked(type_, start_sid, nsectors, segments, complete_fn)
    }

    /// Constructs a range-only `Bio`.
    ///
    /// Range-only operations carry only a sector interval. They intentionally
    /// own no memory segments because the device either discards the range or
    /// synthesizes zeroes without copying data from the caller.
    pub fn new_range(
        type_: BioType,
        start_sid: Sid,
        nsectors: u64,
        complete_fn: Option<BioCompleteFn>,
    ) -> Self {
        assert!(type_.is_range_only());
        Self::new_with_nsectors_unchecked(type_, start_sid, nsectors, Vec::new(), complete_fn)
    }

    fn new_with_nsectors_unchecked(
        type_: BioType,
        start_sid: Sid,
        nsectors: u64,
        segments: Vec<BioSegment>,
        complete_fn: Option<BioCompleteFn>,
    ) -> Self {
        let end = start_sid
            .to_raw()
            .checked_add(nsectors)
            .expect("BIO sector range overflow");
        Self::new_with_sid_range(type_, start_sid..Sid::new(end), segments, complete_fn)
    }

    fn new_with_sid_range(
        type_: BioType,
        sid_range: Range<Sid>,
        segments: Vec<BioSegment>,
        complete_fn: Option<BioCompleteFn>,
    ) -> Self {
        let metadata = Arc::new(BioMetadata {
            type_,
            sid_range: sid_range.clone(),
            status: AtomicU32::new(BioStatus::Init as u32),
            wait_queue: WaitQueue::new(),
        });
        Self {
            metadata,
            complete_fn,
            segments,
        }
    }

    /// Returns the type.
    pub fn type_(&self) -> BioType {
        self.metadata.type_()
    }

    /// Returns the range of target sectors on the device.
    pub fn sid_range(&self) -> &Range<Sid> {
        self.metadata.sid_range()
    }

    /// Returns the slice to the memory segments currently owned by this handle.
    pub fn segments(&self) -> &[BioSegment] {
        &self.segments
    }

    /// Returns the status.
    pub fn status(&self) -> BioStatus {
        self.metadata.status()
    }

    #[cfg(ktest)]
    pub fn submit_for_test(self) -> SubmittedBio {
        let Self {
            metadata,
            complete_fn,
            segments,
        } = self;
        let result = metadata.status.compare_exchange(
            BioStatus::Init as u32,
            BioStatus::Submit as u32,
            Ordering::Release,
            Ordering::Relaxed,
        );
        assert!(result.is_ok());
        let current_sid_range = metadata.sid_range().clone();
        SubmittedBio {
            metadata,
            current_sid_range,
            complete_fn,
            segments,
            complete_as_io_error_on_drop: false,
        }
    }

    /// Submits self to the `block_device` asynchronously.
    ///
    /// This method consumes `self` and transfers its segments and completion
    /// callback to the submitted request.
    ///
    /// Pushes the completion record into `io_batch`.
    ///
    /// # Panics
    ///
    /// The caller must not submit a `Bio` more than once. Otherwise, a panic shall be triggered.
    pub fn submit(
        self,
        block_device: &dyn BlockDevice,
        io_batch: &mut IoBatch,
    ) -> Result<(), BioEnqueueError> {
        let Self {
            metadata,
            complete_fn,
            segments,
        } = self;

        // Change the status from "Init" to "Submit".
        let result = metadata.status.compare_exchange(
            BioStatus::Init as u32,
            BioStatus::Submit as u32,
            Ordering::Release,
            Ordering::Relaxed,
        );
        assert!(result.is_ok());

        let waiter_metadata = metadata.clone();
        let current_sid_range = metadata.sid_range().clone();
        let submitted_bio = SubmittedBio {
            metadata,
            current_sid_range,
            complete_fn,
            segments,
            complete_as_io_error_on_drop: false,
        };
        if let Err(e) = block_device.enqueue(submitted_bio) {
            // Fail to submit, revert the status.
            let result = waiter_metadata.status.compare_exchange(
                BioStatus::Submit as u32,
                BioStatus::Init as u32,
                Ordering::Release,
                Ordering::Relaxed,
            );
            assert!(result.is_ok());
            return Err(e);
        }

        io_batch.push(waiter_metadata);
        Ok(())
    }

    /// Submits self to the `block_device` and waits for the result synchronously.
    ///
    /// Returns the result status of the `Bio`.
    ///
    /// # Panics
    ///
    /// The caller must not submit a `Bio` more than once. Otherwise, a panic shall be triggered.
    pub fn submit_and_wait(
        self,
        block_device: &dyn BlockDevice,
    ) -> Result<BioStatus, BioEnqueueError> {
        let mut io_batch = IoBatch::with_capacity(1);
        self.submit(block_device, &mut io_batch)?;
        let _ = io_batch.wait_all();
        let metadata = io_batch[0].downcast_ref::<BioMetadata>().unwrap();
        Ok(metadata.status())
    }
}

impl Debug for Bio {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.debug_struct("Bio")
            .field("metadata", &self.metadata)
            .field("segments", &self.segments)
            .finish()
    }
}

/// The error type returned when enqueueing the `Bio`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BioEnqueueError {
    /// The request queue is full
    IsFull,
    /// Refuse to enqueue the bio
    Refused,
    /// Too big bio
    TooBig,
}

impl From<BioEnqueueError> for Error {
    fn from(_error: BioEnqueueError) -> Self {
        Error::NotEnoughResources
    }
}

/// A submitted `Bio` object.
///
/// The request queue of block device only accepts a `SubmittedBio` into the queue.
pub struct SubmittedBio {
    metadata: Arc<BioMetadata>,
    current_sid_range: Range<Sid>,
    complete_fn: Option<BioCompleteFn>,
    segments: Vec<BioSegment>,
    // Deferred block layers have already accepted this BIO before replay. If a
    // later mapping or backing enqueue error drops it, completion must still
    // reach the original submitter instead of leaving it in Submit forever.
    complete_as_io_error_on_drop: bool,
}

impl SubmittedBio {
    /// Returns the type.
    pub fn type_(&self) -> BioType {
        self.metadata.type_()
    }

    /// Returns the sector range seen by the current block device layer.
    pub fn sid_range(&self) -> &Range<Sid> {
        &self.current_sid_range
    }

    /// Remaps the current sector range to a new start while preserving the `Bio` length.
    pub fn remap_sid_start(&mut self, new_start: Sid) -> Result<(), BioEnqueueError> {
        let length = self
            .current_sid_range
            .end
            .to_raw()
            .checked_sub(self.current_sid_range.start.to_raw())
            .ok_or(BioEnqueueError::Refused)?;
        let new_end = new_start
            .to_raw()
            .checked_add(length)
            .ok_or(BioEnqueueError::Refused)?;
        self.current_sid_range = new_start..Sid::new(new_end);
        Ok(())
    }

    /// Adds a sector offset to the current mapping result.
    pub fn add_sid_offset(&mut self, offset: u64) -> Result<(), BioEnqueueError> {
        let new_start = self
            .current_sid_range
            .start
            .to_raw()
            .checked_add(offset)
            .map(Sid::new)
            .ok_or(BioEnqueueError::Refused)?;
        self.remap_sid_start(new_start)
    }

    /// Splits the current `Bio` into children that cover the same range.
    pub fn split(
        self,
        ranges: Vec<Range<Sid>>,
    ) -> Result<(Vec<Self>, SplitBioCompletionHandle), BioEnqueueError> {
        self.validate_split_ranges(&ranges)?;

        let type_ = self.type_();
        let complete_as_io_error_on_drop = self.complete_as_io_error_on_drop;
        let child_segments = ranges
            .iter()
            .map(|range| self.segments_for_child_range(range))
            .collect::<Result<Vec<_>, _>>()?;
        let completion = Arc::new(SplitBioCompletion {
            remaining: AtomicUsize::new(ranges.len()),
            status: AtomicU32::new(BioStatus::Complete as u32),
            original: SpinLock::new(Some(self)),
        });

        let children = ranges
            .into_iter()
            .zip(child_segments)
            .map(|(range, segments)| {
                let completion = completion.clone();
                Self {
                    metadata: Arc::new(BioMetadata {
                        type_,
                        sid_range: range.clone(),
                        status: AtomicU32::new(BioStatus::Submit as u32),
                        wait_queue: WaitQueue::new(),
                    }),
                    current_sid_range: range,
                    complete_fn: Some(Box::new(move |status| completion.complete_child(status))),
                    segments,
                    complete_as_io_error_on_drop,
                }
            })
            .collect();
        Ok((children, SplitBioCompletionHandle { inner: completion }))
    }

    fn validate_split_ranges(&self, ranges: &[Range<Sid>]) -> Result<(), BioEnqueueError> {
        if ranges.is_empty() || ranges[0].start != self.current_sid_range.start {
            return Err(BioEnqueueError::Refused);
        }

        let mut expected_start = self.current_sid_range.start;
        for range in ranges {
            if range.start != expected_start || range.start >= range.end {
                return Err(BioEnqueueError::Refused);
            }
            expected_start = range.end;
        }
        if expected_start != self.current_sid_range.end {
            return Err(BioEnqueueError::Refused);
        }
        Ok(())
    }

    fn segments_for_child_range(
        &self,
        range: &Range<Sid>,
    ) -> Result<Vec<BioSegment>, BioEnqueueError> {
        if self.type_().is_range_only() {
            // A range-only child keeps the logical sector subrange but has no
            // data payload to slice; the lower driver receives the range from
            // the `Bio` metadata.
            return Ok(Vec::new());
        }

        let start_sectors = range
            .start
            .to_raw()
            .checked_sub(self.current_sid_range.start.to_raw())
            .ok_or(BioEnqueueError::Refused)?;
        let end_sectors = range
            .end
            .to_raw()
            .checked_sub(self.current_sid_range.start.to_raw())
            .ok_or(BioEnqueueError::Refused)?;
        let start = sectors_to_bytes(start_sectors)?;
        let end = sectors_to_bytes(end_sectors)?;

        let mut segments = Vec::new();
        let mut cursor = 0usize;
        for segment in &self.segments {
            let segment_start = cursor;
            let segment_end = cursor
                .checked_add(segment.nbytes())
                .ok_or(BioEnqueueError::Refused)?;
            cursor = segment_end;

            let overlap_start = core::cmp::max(start, segment_start);
            let overlap_end = core::cmp::min(end, segment_end);
            if overlap_start >= overlap_end {
                continue;
            }

            let relative_start = overlap_start - segment_start;
            let relative_end = overlap_end - segment_start;
            segments.push(segment.slice(relative_start..relative_end));
        }

        let total_len = segments.iter().map(BioSegment::nbytes).sum::<usize>();
        if total_len != end - start {
            return Err(BioEnqueueError::Refused);
        }
        Ok(segments)
    }

    /// Returns the slice to the memory segments.
    pub fn segments(&self) -> &[BioSegment] {
        &self.segments
    }

    /// Returns the status.
    pub fn status(&self) -> BioStatus {
        self.metadata.status()
    }

    /// Arms completion with I/O error if a deferred replay path drops this BIO.
    ///
    /// Normal immediate submission must not enable this: its caller still owns
    /// enqueue failure and restores the BIO from `Submit` to `Init`.
    pub fn complete_as_io_error_on_drop(&mut self) {
        self.complete_as_io_error_on_drop = true;
    }

    /// Chains an additional completion callback after the original one.
    ///
    /// Stacked block devices use this to release their own in-flight references
    /// when the lower-level `Bio` really completes, without changing the
    /// original submitter's completion notification.
    pub fn chain_complete_fn<F>(&mut self, complete_fn: F)
    where
        F: FnOnce(BioStatus) + Send + 'static,
    {
        let previous = self.complete_fn.take();
        self.complete_fn = Some(Box::new(move |status| {
            if let Some(previous) = previous {
                previous(status);
            }
            complete_fn(status);
        }));
    }

    /// Completes the `Bio` by consuming `self`, releasing the segments, invoking
    /// the callback function, and publishing the final status.
    ///
    /// The final status becomes visible only after the callback returns.
    ///
    /// When the driver finishes the request for this `Bio`, it will call this method.
    pub fn complete(mut self, status: BioStatus) {
        assert!(status != BioStatus::Init && status != BioStatus::Submit);

        self.complete_as_io_error_on_drop = false;
        let complete_fn = self.complete_fn.take();

        // Complete the `complete_fn` before publishing the status change,
        // so that the effects of the callback function are visible to users.
        general_complete_fn(self.metadata.type_(), status, complete_fn);
        let result = self.metadata.status.compare_exchange(
            BioStatus::Submit as u32,
            status as u32,
            Ordering::Release,
            Ordering::Relaxed,
        );
        assert!(result.is_ok());

        self.metadata.wait_queue.wake_all();
    }
}

impl Drop for SubmittedBio {
    fn drop(&mut self) {
        if !self.complete_as_io_error_on_drop || self.status() != BioStatus::Submit {
            return;
        }

        self.complete_as_io_error_on_drop = false;
        let complete_fn = self.complete_fn.take();
        general_complete_fn(self.metadata.type_(), BioStatus::IoError, complete_fn);
        let result = self.metadata.status.compare_exchange(
            BioStatus::Submit as u32,
            BioStatus::IoError as u32,
            Ordering::Release,
            Ordering::Relaxed,
        );
        debug_assert!(result.is_ok());
        self.metadata.wait_queue.wake_all();
    }
}

impl Debug for SubmittedBio {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.debug_struct("SubmittedBio")
            .field("metadata", &self.metadata)
            .field("current_sid_range", &self.current_sid_range)
            .field("segments", &self.segments)
            .finish()
    }
}

pub struct SplitBioCompletionHandle {
    inner: Arc<SplitBioCompletion>,
}

impl SplitBioCompletionHandle {
    pub fn complete_child(&self, status: BioStatus) {
        self.inner.complete_child(status);
    }
}

struct SplitBioCompletion {
    remaining: AtomicUsize,
    status: AtomicU32,
    original: SpinLock<Option<SubmittedBio>, LocalIrqDisabled>,
}

impl SplitBioCompletion {
    fn complete_child(&self, status: BioStatus) {
        if status != BioStatus::Complete {
            let _ = self.status.compare_exchange(
                BioStatus::Complete as u32,
                status as u32,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
        }

        let previous = self.remaining.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
        if previous == 1 {
            let status = BioStatus::try_from(self.status.load(Ordering::Acquire)).unwrap();
            let original = self
                .original
                .lock()
                .take()
                .expect("split BIO original must complete exactly once");
            original.complete(status);
        }
    }
}

fn sectors_to_bytes(sectors: u64) -> Result<usize, BioEnqueueError> {
    usize::try_from(sectors)
        .ok()
        .and_then(|sectors| sectors.checked_mul(SECTOR_SIZE))
        .ok_or(BioEnqueueError::Refused)
}

/// The metadata and waitable state shared by submitted `Bio`s and their waiter handles.
struct BioMetadata {
    /// The type of the I/O
    type_: BioType,
    /// The logical range of target sectors on device
    sid_range: Range<Sid>,
    /// The I/O status
    status: AtomicU32,
    /// The wait queue for I/O completion
    wait_queue: WaitQueue,
}

impl BioMetadata {
    pub fn type_(&self) -> BioType {
        self.type_
    }

    pub fn sid_range(&self) -> &Range<Sid> {
        &self.sid_range
    }

    pub fn status(&self) -> BioStatus {
        BioStatus::try_from(self.status.load(Ordering::Acquire)).unwrap()
    }
}

impl IoCompletion for BioMetadata {
    fn wait(&self) -> Result<(), IoError> {
        let status = self.wait_queue.wait_until(|| {
            let status = self.status();
            (status != BioStatus::Submit).then_some(status)
        });

        match status {
            BioStatus::Complete => Ok(()),
            BioStatus::NotSupported => Err(IoError::Unsupported),
            BioStatus::NoSpace => Err(IoError::OutOfSpace),
            BioStatus::IoError => Err(IoError::Failed),
            BioStatus::Init | BioStatus::Submit | BioStatus::Zeros => unreachable!(),
        }
    }
}

impl Debug for BioMetadata {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.debug_struct("BioMetadata")
            .field("type", &self.type_())
            .field("sid_range", &self.sid_range())
            .field("status", &self.status())
            .finish()
    }
}

/// The type of `Bio`.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, TryFromInt)]
pub enum BioType {
    /// Read sectors from the device.
    Read = 0,
    /// Write sectors into the device.
    Write = 1,
    /// Flush the volatile write cache.
    Flush = 2,
    /// Discard sectors on the device.
    Discard = 3,
    /// Write zeroes to sectors on the device.
    WriteZeroes = 4,
}

impl BioType {
    /// Returns whether this operation may change the device contents.
    pub fn is_write_like(self) -> bool {
        matches!(self, Self::Write | Self::Discard | Self::WriteZeroes)
    }

    /// Returns whether this operation is described entirely by a sector range.
    pub fn is_range_only(self) -> bool {
        matches!(self, Self::Discard | Self::WriteZeroes)
    }
}

/// The status of `Bio`.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromInt)]
pub enum BioStatus {
    /// The initial status for a newly created `Bio`.
    Init = 0,
    /// After a `Bio` is submitted, its status will be changed to "Submit".
    Submit = 1,
    /// The I/O operation has been successfully completed.
    Complete = 2,
    /// The I/O operation is not supported.
    NotSupported = 3,
    /// Insufficient space is available to perform the I/O operation.
    NoSpace = 4,
    /// An error occurred while doing I/O.
    IoError = 5,
    /// No I/O operation is needed because the sectors to read only contain zeros.
    Zeros = 6,
}

/// `BioSegment` is the basic memory unit of a block I/O request.
#[derive(Clone, Debug)]
pub struct BioSegment {
    inner: Arc<BioSegmentInner>,
}

/// The inner part of `BioSegment`.
// TODO: Decouple `BioSegmentInner` with DMA-related buffers.
#[derive(Debug)]
struct BioSegmentInner {
    /// Internal DMA slice.
    // TODO: The direction is currently `FromAndToDevice`. Implement compile-time checking.
    dma_slice: Slice<Arc<DmaStream>>,
    direction: BioDirection,
    /// Whether the segment is allocated from the pool.
    from_pool: bool,
}

/// The direction of a bio request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BioDirection {
    /// Read from the backed block device.
    FromDevice,
    /// Write to the backed block device.
    ToDevice,
}

impl BioSegment {
    /// Allocates a new `BioSegment` with the wanted blocks count and
    /// the bio direction.
    pub fn alloc(nblocks: usize, direction: BioDirection) -> Self {
        Self::alloc_inner(nblocks, 0, nblocks * BLOCK_SIZE, direction)
    }

    /// Allocates a sector-aligned test segment with an exact byte length.
    ///
    /// This is available only to kernel tests that need to exercise BIO splitting
    /// at a sector boundary that does not coincide with a filesystem block.
    #[cfg(ktest)]
    pub fn alloc_exact(nblocks: usize, len: usize, direction: BioDirection) -> Self {
        Self::alloc_inner(nblocks, 0, len, direction)
    }

    /// The inner function that do the real segment allocation.
    ///
    /// Support two extended parameters:
    /// 1. `offset_within_first_block`: the offset (in bytes) within the first block.
    /// 2. `len`: the exact length (in bytes) of the wanted segment. (May
    ///    less than `nblocks * BLOCK_SIZE`)
    ///
    /// # Panics
    ///
    /// If the `offset_within_first_block` or `len` is not sector aligned,
    /// this method will panic.
    pub(super) fn alloc_inner(
        nblocks: usize,
        offset_within_first_block: usize,
        len: usize,
        direction: BioDirection,
    ) -> Self {
        let offset = offset_within_first_block;
        assert!(
            is_sector_aligned(offset)
                && offset < BLOCK_SIZE
                && is_sector_aligned(len)
                && offset + len <= nblocks * BLOCK_SIZE
        );

        // The target segment is whether from the pool or newly-allocated
        let bio_segment_inner = target_pool(direction)
            .and_then(|pool| pool.alloc(nblocks, offset, len))
            .unwrap_or_else(|| {
                let dma_stream = DmaStream::alloc_uninit(nblocks, false).unwrap();
                BioSegmentInner {
                    dma_slice: Slice::new(Arc::new(dma_stream), offset..offset + len),
                    direction,
                    from_pool: false,
                }
            });

        Self {
            inner: Arc::new(bio_segment_inner),
        }
    }

    /// Constructs a new `BioSegment` with a given `USegment` and the bio direction.
    ///
    /// # Panics
    ///
    /// If the segment length is not sector aligned, this method will panic.
    pub fn new_from_segment(segment: USegment, direction: BioDirection) -> Self {
        let len = segment.size();
        assert!(is_sector_aligned(len));
        let dma_stream = DmaStream::map(segment, false).unwrap();
        Self {
            inner: Arc::new(BioSegmentInner {
                dma_slice: Slice::new(Arc::new(dma_stream), 0..len),
                direction,
                from_pool: false,
            }),
        }
    }

    /// Returns the number of bytes.
    pub fn nbytes(&self) -> usize {
        self.inner.dma_slice.size()
    }

    /// Returns the number of sectors.
    pub fn nsectors(&self) -> Sid {
        Sid::from_offset(self.nbytes())
    }

    /// Returns the number of blocks.
    pub fn nblocks(&self) -> usize {
        self.nbytes().align_up(BLOCK_SIZE) / BLOCK_SIZE
    }

    /// Returns the offset (in bytes) within the first block.
    pub fn offset_within_first_block(&self) -> usize {
        self.inner.dma_slice.offset().start % BLOCK_SIZE
    }

    /// Returns the inner DMA slice.
    pub fn inner_dma_slice(&self) -> &Slice<Arc<DmaStream>> {
        &self.inner.dma_slice
    }

    fn slice(&self, range: Range<usize>) -> Self {
        assert!(is_sector_aligned(range.start) && is_sector_aligned(range.end - range.start));
        if range.start == 0 && range.end == self.nbytes() {
            return self.clone();
        }
        Self {
            inner: Arc::new(BioSegmentInner {
                dma_slice: self.inner.dma_slice.slice(range),
                direction: self.inner.direction,
                from_pool: false,
            }),
        }
    }

    /// Returns the inner DMA object.
    ///
    /// Note that the slicing will be ignored. This is only for testing.
    #[cfg(ktest)]
    pub fn inner_dma(&self) -> &Arc<DmaStream> {
        self.inner.dma_slice.mem_obj()
    }
}

impl HasVmReaderWriter for BioSegment {
    type Types = VmReaderWriterResult;

    fn reader(&self) -> Result<VmReader<'_, Infallible>, Error> {
        if self.inner.direction != BioDirection::FromDevice {
            return Err(Error::AccessDenied);
        }
        self.inner.dma_slice.reader()
    }

    fn writer(&self) -> Result<VmWriter<'_, Infallible>, Error> {
        if self.inner.direction != BioDirection::ToDevice {
            return Err(Error::AccessDenied);
        }
        self.inner.dma_slice.writer()
    }
}

// The timing for free the segment to the pool.
impl Drop for BioSegmentInner {
    fn drop(&mut self) {
        if !self.from_pool {
            return;
        }
        if let Some(pool) = target_pool(self.direction()) {
            pool.free(self);
        }
    }
}

impl BioSegmentInner {
    /// Returns the bio direction.
    fn direction(&self) -> BioDirection {
        self.direction
    }
}

/// A pool of managing segments for block I/O requests.
///
/// Inside the pool, it's a large chunk of `DmaStream` which
/// contains the mapped segment. The allocation/free is done by slicing
/// the `DmaStream`.
// TODO: Use a more advanced allocation algorithm to replace the naive one to improve efficiency.
struct BioSegmentPool {
    pool: Arc<DmaStream>,
    total_blocks: usize,
    direction: BioDirection,
    manager: SpinLock<PoolSlotManager, LocalIrqDisabled>,
}

/// Manages the free slots in the pool.
struct PoolSlotManager {
    /// A bit array to manage the occupied slots in the pool (Bit
    /// value 1 represents "occupied"; 0 represents "free").
    /// The total size is currently determined by `POOL_DEFAULT_NBLOCKS`.
    occupied: BitArray<[u8; POOL_DEFAULT_NBLOCKS.div_ceil(8)]>,
    /// The first index of all free slots in the pool.
    min_free: usize,
}

impl BioSegmentPool {
    /// Creates a new pool given the bio direction. The total number of
    /// managed blocks is currently set to `POOL_DEFAULT_NBLOCKS`.
    ///
    /// The new pool will be allocated and mapped for later allocation.
    pub fn new(direction: BioDirection) -> Self {
        let total_blocks = POOL_DEFAULT_NBLOCKS;
        let pool = DmaStream::alloc_uninit(total_blocks, false).unwrap();
        let manager = SpinLock::new(PoolSlotManager {
            occupied: BitArray::ZERO,
            min_free: 0,
        });

        Self {
            pool: Arc::new(pool),
            total_blocks,
            direction,
            manager,
        }
    }

    /// Allocates a bio segment with the given count `nblocks`
    /// from the pool.
    ///
    /// Support two extended parameters:
    /// 1. `offset_within_first_block`: the offset (in bytes) within the first block.
    /// 2. `len`: the exact length (in bytes) of the wanted segment. (May
    ///    less than `nblocks * BLOCK_SIZE`)
    ///
    /// If there is no enough space in the pool, this method
    /// will return `None`.
    ///
    /// # Panics
    ///
    /// If the `offset_within_first_block` exceeds the block size, or the `len`
    /// exceeds the total length, this method will panic.
    pub fn alloc(
        &self,
        nblocks: usize,
        offset_within_first_block: usize,
        len: usize,
    ) -> Option<BioSegmentInner> {
        assert!(
            offset_within_first_block < BLOCK_SIZE
                && offset_within_first_block + len <= nblocks * BLOCK_SIZE
        );
        let mut manager = self.manager.lock();
        if nblocks > self.total_blocks - manager.min_free {
            return None;
        }

        // Find the free range
        let (start, end) = {
            let mut start = manager.min_free;
            let mut end = start;
            while end < self.total_blocks && end - start < nblocks {
                if manager.occupied[end] {
                    start = end + 1;
                    end = start;
                } else {
                    end += 1;
                }
            }
            if end - start < nblocks {
                return None;
            }
            (start, end)
        };

        manager.occupied[start..end].fill(true);
        manager.min_free = manager.occupied[end..]
            .iter()
            .position(|i| !i)
            .map(|pos| end + pos)
            .unwrap_or(self.total_blocks);

        let dma_slice = {
            let offset = start * BLOCK_SIZE + offset_within_first_block;
            Slice::new(self.pool.clone(), offset..offset + len)
        };
        let bio_segment = BioSegmentInner {
            dma_slice,
            direction: self.direction,
            from_pool: true,
        };
        Some(bio_segment)
    }

    /// Returns an allocated bio segment to the pool,
    /// free the space. This method is not public and should only
    /// be called automatically by `BioSegmentInner::drop()`.
    ///
    /// # Panics
    ///
    /// If the target bio segment is not allocated from the pool
    /// or not the same direction, this method will panic.
    fn free(&self, bio_segment: &BioSegmentInner) {
        assert!(bio_segment.from_pool && bio_segment.direction() == self.direction);
        let (start, end) = {
            let dma_slice = &bio_segment.dma_slice;
            let start = dma_slice.offset().start.align_down(BLOCK_SIZE) / BLOCK_SIZE;
            let end = dma_slice.offset().end.align_up(BLOCK_SIZE) / BLOCK_SIZE;

            if end <= start || end > self.total_blocks {
                return;
            }
            (start, end)
        };

        let mut manager = self.manager.lock();
        debug_assert!(manager.occupied[start..end].iter().all(|i| *i));
        manager.occupied[start..end].fill(false);
        if start < manager.min_free {
            manager.min_free = start;
        }
    }
}

/// A pool of segments for read bio requests only.
static BIO_SEGMENT_RPOOL: Once<Arc<BioSegmentPool>> = Once::new();
/// A pool of segments for write bio requests only.
static BIO_SEGMENT_WPOOL: Once<Arc<BioSegmentPool>> = Once::new();
/// The default number of blocks in each pool. (16MB each for now)
const POOL_DEFAULT_NBLOCKS: usize = 4096;

/// Initializes the bio segment pool.
pub fn bio_segment_pool_init() {
    BIO_SEGMENT_RPOOL.call_once(|| Arc::new(BioSegmentPool::new(BioDirection::FromDevice)));
    BIO_SEGMENT_WPOOL.call_once(|| Arc::new(BioSegmentPool::new(BioDirection::ToDevice)));
}

/// Gets the target pool with the given `direction`.
fn target_pool(direction: BioDirection) -> Option<&'static Arc<BioSegmentPool>> {
    match direction {
        BioDirection::FromDevice => BIO_SEGMENT_RPOOL.get(),
        BioDirection::ToDevice => BIO_SEGMENT_WPOOL.get(),
    }
}

/// Checks if the given offset is aligned to sector.
pub fn is_sector_aligned(offset: usize) -> bool {
    offset.is_multiple_of(SECTOR_SIZE)
}

#[cfg(ktest)]
mod tests {
    use alloc::vec;

    use ostd::prelude::ktest;

    use super::*;

    fn submitted_bio(start: u64, end: u64) -> SubmittedBio {
        let sid_range = Sid::new(start)..Sid::new(end);
        SubmittedBio {
            metadata: Arc::new(BioMetadata {
                type_: BioType::Read,
                sid_range: sid_range.clone(),
                status: AtomicU32::new(BioStatus::Submit as u32),
                wait_queue: WaitQueue::new(),
            }),
            current_sid_range: sid_range,
            complete_fn: None,
            segments: Vec::new(),
            complete_as_io_error_on_drop: false,
        }
    }

    #[ktest]
    fn remap_sid_start_preserves_length_and_original_range() {
        let mut bio = submitted_bio(10, 18);

        bio.remap_sid_start(Sid::new(100)).unwrap();

        assert_eq!(bio.sid_range(), &(Sid::new(100)..Sid::new(108)));
        assert_eq!(bio.metadata.sid_range(), &(Sid::new(10)..Sid::new(18)));
    }

    #[ktest]
    fn add_sid_offset_composes_multiple_block_layers() {
        let mut bio = submitted_bio(10, 18);

        bio.add_sid_offset(100).unwrap();
        bio.add_sid_offset(1_000).unwrap();

        assert_eq!(bio.sid_range(), &(Sid::new(1_110)..Sid::new(1_118)));
    }

    #[ktest]
    fn remap_sid_start_rejects_overflow_without_changing_range() {
        let mut bio = submitted_bio(10, 18);
        let original = bio.sid_range().clone();

        assert_eq!(
            bio.remap_sid_start(Sid::new(u64::MAX - 3)),
            Err(BioEnqueueError::Refused)
        );
        assert_eq!(bio.sid_range(), &original);
    }

    #[ktest]
    fn new_range_constructs_range_only_bio() {
        let bio = Bio::new_range(BioType::Discard, Sid::new(10), 8, None);

        assert_eq!(bio.type_(), BioType::Discard);
        assert_eq!(bio.sid_range(), &(Sid::new(10)..Sid::new(18)));
        assert!(bio.segments().is_empty());
    }

    #[ktest]
    fn splits_range_only_bio_without_segments() {
        let bio = Bio::new_range(BioType::WriteZeroes, Sid::new(10), 8, None).submit_for_test();
        let (children, _completion) = bio
            .split(vec![Sid::new(10)..Sid::new(14), Sid::new(14)..Sid::new(18)])
            .unwrap();

        assert_eq!(children[0].type_(), BioType::WriteZeroes);
        assert_eq!(children[0].sid_range(), &(Sid::new(10)..Sid::new(14)));
        assert!(children[0].segments().is_empty());
        assert_eq!(children[1].sid_range(), &(Sid::new(14)..Sid::new(18)));
        assert!(children[1].segments().is_empty());
    }

    #[ktest]
    fn split_bio_keeps_first_error_until_all_children_complete() {
        let completions = Arc::new(SpinLock::<Vec<BioStatus>, LocalIrqDisabled>::new(Vec::new()));
        let callback_completions = completions.clone();
        let bio = Bio::new_range(
            BioType::WriteZeroes,
            Sid::new(10),
            12,
            Some(Box::new(move |status| {
                callback_completions.lock().push(status);
            })),
        )
        .submit_for_test();
        let (children, _completion) = bio
            .split(vec![
                Sid::new(10)..Sid::new(14),
                Sid::new(14)..Sid::new(18),
                Sid::new(18)..Sid::new(22),
            ])
            .unwrap();
        let mut children = children.into_iter();
        let first = children.next().unwrap();
        let second = children.next().unwrap();
        let third = children.next().unwrap();

        third.complete(BioStatus::NoSpace);
        first.complete(BioStatus::IoError);
        assert!(completions.lock().is_empty());

        second.complete(BioStatus::Complete);
        assert_eq!(*completions.lock(), vec![BioStatus::NoSpace]);
    }

    #[ktest]
    fn write_like_includes_discard_and_write_zeroes() {
        assert!(BioType::Write.is_write_like());
        assert!(BioType::Discard.is_write_like());
        assert!(BioType::WriteZeroes.is_write_like());
        assert!(!BioType::Read.is_write_like());
        assert!(!BioType::Flush.is_write_like());
    }

    #[ktest]
    fn add_sid_offset_rejects_overflow_without_changing_range() {
        let mut bio = submitted_bio(u64::MAX - 8, u64::MAX);
        let original = bio.sid_range().clone();

        assert_eq!(bio.add_sid_offset(9), Err(BioEnqueueError::Refused));
        assert_eq!(bio.sid_range(), &original);
    }
}

/// An aligned unsigned integer number.
///
/// An instance of `AlignedUsize<const N: u16>` is guaranteed to have a value that is a multiple
/// of `N`, a predetermined const value. It is preferable to express an unsigned integer value
/// in type `AlignedUsize<_>` instead of `usize` if the value must satisfy an alignment requirement.
/// This helps readability and prevents bugs.
///
/// # Examples
///
/// ```rust
/// const SECTOR_SIZE: u16 = 512;
///
/// let sector_num = 1234; // The 1234-th sector
/// let sector_offset: AlignedUsize<SECTOR_SIZE> = {
///     let sector_offset = sector_num * (SECTOR_SIZE as usize);
///     AlignedUsize::<SECTOR_SIZE>::new(sector_offset).unwrap()
/// };
/// assert!(sector_offset.value().is_multiple_of(sector_offset.align()));
/// ```
///
/// # Limitation
///
/// Currently, the alignment const value must be expressed in `u16`;
/// it is not possible to use a larger or smaller type.
/// This limitation is inherited from that of Rust's const generics:
/// your code can be generic over the _value_ of a const, but not the _type_ of the const.
/// We choose `u16` because it is reasonably large to represent any alignment value
/// used in practice.
#[derive(Clone, Debug)]
pub struct AlignedUsize<const N: u16>(usize);

impl<const N: u16> AlignedUsize<N> {
    /// Constructs a new instance of aligned integer if the given value is aligned.
    pub fn new(val: usize) -> Option<Self> {
        if val.is_multiple_of(N as usize) {
            Some(Self(val))
        } else {
            None
        }
    }

    /// Returns the value.
    pub fn value(&self) -> usize {
        self.0
    }

    /// Returns the corresponding ID.
    ///
    /// The so-called "ID" of an aligned integer is defined to be `self.value() / self.align()`.
    /// This value is named ID because one common use case is using `Aligned` to express
    /// the byte offset of a sector, block, or page. In this case, the `id` method returns
    /// the ID of the corresponding sector, block, or page.
    pub fn id(&self) -> usize {
        self.value() / self.align()
    }

    /// Returns the alignment.
    pub fn align(&self) -> usize {
        N as usize
    }
}
