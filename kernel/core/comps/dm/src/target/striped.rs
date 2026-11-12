// SPDX-License-Identifier: MPL-2.0

//! Linux striped target geometry, chunk mapping, and per-device health reporting.

use alloc::{format, string::String, vec::Vec};
use core::{
    fmt::Write,
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

use aster_block::{BlockDevice, BlockDeviceLease, bio::BioStatus, id::Sid};
use device_id::DeviceId;

use super::{
    DmTarget, DmTargetMetadata, DmTargetParseError, TargetIoAction, TargetIoCompletion,
    TargetRange, TargetStatusMode, args::Arguments,
};
use crate::TableError;

// Bound both descriptor storage and escaped-token scratch space per target.
const MAX_STRIPES: u32 = 4_096;
const MAX_PARAMS_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
struct Stripe {
    backing: BlockDeviceLease,
    backing_start: Sid,
    failed: AtomicBool,
}

/// A target that distributes successive chunks across its backing devices.
#[derive(Debug)]
pub(super) struct StripedTarget {
    range: TargetRange,
    chunk_sectors: u64,
    stripes: Vec<Stripe>,
}

/// Factory for the built-in striped target.
pub(super) struct TargetType;

pub(super) static TARGET_TYPE: TargetType = TargetType;

impl super::DmTargetType for TargetType {
    fn metadata(&self) -> DmTargetMetadata {
        DmTargetMetadata::new("striped", [1, 6, 0])
    }

    fn create(
        &self,
        logical_start: Sid,
        length: u64,
        params: &str,
        parse_backing: &mut dyn FnMut(&str) -> Result<DeviceId, super::BackingLookupError>,
        resolve_backing: &mut dyn FnMut(
            DeviceId,
        )
            -> Result<BlockDeviceLease, super::BackingLookupError>,
    ) -> Result<super::DmTargetBox, DmTargetParseError<super::BackingLookupError>> {
        Ok(alloc::boxed::Box::new(StripedTarget::parse_with(
            logical_start,
            length,
            params,
            parse_backing,
            resolve_backing,
        )?))
    }
}

impl StripedTarget {
    /// Parses Linux stripe parameters and retains every resolved backing lease.
    pub(super) fn parse_with<E>(
        logical_start: Sid,
        length: u64,
        params: &str,
        parse_backing: &mut (impl FnMut(&str) -> Result<DeviceId, E> + ?Sized),
        resolve_backing: &mut (impl FnMut(DeviceId) -> Result<BlockDeviceLease, E> + ?Sized),
    ) -> Result<Self, DmTargetParseError<E>> {
        if params.len() > MAX_PARAMS_BYTES {
            return Err(TableError::InvalidTargetParams.into());
        }
        let range = TargetRange::new(logical_start, length)?;
        let mut fields = Arguments::new(params);
        let count = fields
            .try_next()
            .map_err(|_| TableError::OutOfMemory)?
            .ok_or(TableError::InvalidTargetParams)?
            .parse::<u32>()
            .map_err(|_| TableError::InvalidTargetParams)?;
        let chunk = fields
            .try_next()
            .map_err(|_| TableError::OutOfMemory)?
            .ok_or(TableError::InvalidTargetParams)?
            .parse::<u32>()
            .map_err(|_| TableError::InvalidTargetParams)?;
        if count == 0
            || count > MAX_STRIPES
            || chunk == 0
            || !length.is_multiple_of(u64::from(count))
        {
            return Err(TableError::InvalidTargetParams.into());
        }
        let width = length / u64::from(count);
        if !width.is_multiple_of(u64::from(chunk)) {
            return Err(TableError::InvalidTargetParams.into());
        }
        // Validate all tokens and arithmetic before performing environment lookups.
        // Keep Argument values alive so escaped scratch storage is wiped on drop.
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(count as usize)
            .map_err(|_| TableError::OutOfMemory)?;
        for _ in 0..count {
            let device = fields
                .try_next()
                .map_err(|_| TableError::OutOfMemory)?
                .ok_or(TableError::InvalidTargetParams)?;
            let start = fields
                .try_next()
                .map_err(|_| TableError::OutOfMemory)?
                .ok_or(TableError::InvalidTargetParams)?
                .parse::<u64>()
                .map_err(|_| TableError::InvalidTargetParams)?;
            start
                .checked_add(width)
                .ok_or(TableError::BackingRangeOverflow)?;
            entries.push((device, start));
        }
        if fields
            .try_next()
            .map_err(|_| TableError::OutOfMemory)?
            .is_some()
        {
            return Err(TableError::InvalidTargetParams.into());
        }
        let mut stripes = Vec::new();
        stripes
            .try_reserve_exact(count as usize)
            .map_err(|_| TableError::OutOfMemory)?;
        for (device, start) in entries {
            let id = parse_backing(&device).map_err(DmTargetParseError::ResolveBacking)?;
            let backing = resolve_backing(id).map_err(DmTargetParseError::ResolveBacking)?;
            let capacity = u64::try_from(backing.metadata().nr_sectors)
                .map_err(|_| TableError::BackingRangeOverflow)?;
            if start + width > capacity {
                return Err(TableError::BackingRangeOutOfBounds.into());
            }
            stripes.push(Stripe {
                backing,
                backing_start: Sid::new(start),
                failed: AtomicBool::new(false),
            });
        }
        Ok(Self {
            range,
            chunk_sectors: u64::from(chunk),
            stripes,
        })
    }
}

impl DmTarget for StripedTarget {
    #[cfg(ktest)]
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn metadata(&self) -> DmTargetMetadata {
        super::DmTargetType::metadata(&TARGET_TYPE)
    }

    fn logical_range(&self) -> &Range<Sid> {
        self.range.logical_range()
    }

    fn for_each_backing<'a>(&'a self, f: &mut dyn FnMut(&'a dyn BlockDevice)) {
        for stripe in &self.stripes {
            f(stripe.backing.device().as_ref());
        }
    }

    fn map_io_range(&self, logical: Range<Sid>) -> Option<Vec<TargetIoAction<'_>>> {
        if !self.range.contains_range(&logical) {
            return None;
        }
        let mut actions = Vec::new();
        let mut cursor = logical.start.to_raw();
        while cursor < logical.end.to_raw() {
            // Linux dm-stripe.c: stripe_map_sector(), including non-power-of-two geometry.
            let relative = cursor - self.logical_range().start.to_raw();
            let chunk = relative / self.chunk_sectors;
            let offset = relative % self.chunk_sectors;
            let count = self.stripes.len() as u64;
            let stripe = &self.stripes[(chunk % count) as usize];
            let size = (logical.end.to_raw() - cursor).min(self.chunk_sectors - offset);
            actions.push(TargetIoAction {
                logical_range: Sid::new(cursor)..Sid::new(cursor + size),
                backing_start: Sid::new(
                    stripe.backing_start.to_raw() + chunk / count * self.chunk_sectors + offset,
                ),
                backing: stripe.backing.device().as_ref(),
            });
            cursor += size;
        }
        Some(actions)
    }

    fn io_completion(&self) -> Option<&dyn TargetIoCompletion> {
        Some(self)
    }

    fn status_params(&self, mode: TargetStatusMode) -> Result<String, TableError> {
        let mut output = format!("{}", self.stripes.len());
        if mode == TargetStatusMode::Table {
            write!(output, " {}", self.chunk_sectors).unwrap();
        }
        for stripe in &self.stripes {
            let id = stripe.backing.id();
            write!(output, " {}:{}", id.major().get(), id.minor().get()).unwrap();
            if mode == TargetStatusMode::Table {
                write!(output, " {}", stripe.backing_start.to_raw()).unwrap();
            }
        }
        if mode == TargetStatusMode::Status {
            output.push_str(" 1 ");
            for stripe in &self.stripes {
                output.push(if stripe.failed.load(Ordering::Relaxed) {
                    'D'
                } else {
                    'A'
                });
            }
        }
        Ok(output)
    }
}

impl TargetIoCompletion for StripedTarget {
    fn complete_io(&self, backing: DeviceId, status: BioStatus) {
        if !matches!(status, BioStatus::IoError | BioStatus::NoSpace) {
            return;
        }
        // Repeated stripes on the same device share Linux's device-level error state.
        for stripe in &self.stripes {
            if stripe.backing.id() == backing {
                stripe.failed.store(true, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(ktest)]
mod tests {
    use alloc::sync::Arc;
    use core::cell::Cell;

    use aster_block::{
        BlockDeviceMeta,
        bio::{BioEnqueueError, SubmittedBio},
    };
    use device_id::{MajorId, MinorId};
    use ostd::prelude::ktest;

    use super::*;

    #[derive(Debug)]
    struct Backing;

    impl BlockDevice for Backing {
        fn enqueue(&self, _: SubmittedBio) -> Result<(), BioEnqueueError> {
            unreachable!()
        }
        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 8,
                nr_sectors: 128,
            }
        }
        fn id(&self) -> DeviceId {
            DeviceId::new(MajorId::new(1), MinorId::new(1))
        }
        fn name(&self) -> &str {
            "stripe-test"
        }
    }

    fn parse(
        params: &str,
        start: u64,
        length: u64,
    ) -> Result<StripedTarget, DmTargetParseError<()>> {
        StripedTarget::parse_with(
            Sid::new(start),
            length,
            params,
            &mut |_| Ok(Backing.id()),
            &mut |_| Ok(BlockDeviceLease::new_untracked(Arc::new(Backing))),
        )
    }

    #[ktest]
    fn maps_non_power_of_two_stripes_relative_to_target_start() {
        let target = parse("3 3 a 10 b 20 c 30", 7, 36).unwrap();
        let parts = target.map_io_range(Sid::new(9)..Sid::new(20)).unwrap();
        let observed: Vec<_> = parts
            .iter()
            .map(|part| {
                (
                    part.logical_range.start.to_raw(),
                    part.logical_range.end.to_raw(),
                    part.backing_start.to_raw(),
                )
            })
            .collect();
        assert_eq!(
            observed,
            alloc::vec![
                (9, 10, 12),
                (10, 13, 20),
                (13, 16, 30),
                (16, 19, 13),
                (19, 20, 23)
            ]
        );
        assert!(target.map_io_range(Sid::new(6)..Sid::new(8)).is_none());
        assert!(target.map_io_range(Sid::new(42)..Sid::new(44)).is_none());
        assert!(target.map_io_range(Sid::new(7)..Sid::new(7)).is_none());
        assert_eq!(
            target.status_params(TargetStatusMode::Table).unwrap(),
            "3 3 1:1 10 1:1 20 1:1 30"
        );
        assert_eq!(
            target.status_params(TargetStatusMode::Status).unwrap(),
            "3 1:1 1:1 1:1 1 AAA"
        );
        target.complete_io(Backing.id(), BioStatus::IoError);
        assert_eq!(
            target.status_params(TargetStatusMode::Status).unwrap(),
            "3 1:1 1:1 1:1 1 DDD"
        );
    }

    #[ktest]
    fn rejects_invalid_geometry_and_tokens_before_lookup() {
        for params in [
            "",
            "0 1",
            "1 0 a 0",
            "2 3 a 0 b 0",
            "3 2 a 0 b 0 c 0",
            "1 1",
            "1 1 a",
            "1 1 a 0 extra",
            "1 1 a -1",
            "1 4294967296 a 0",
            "4294967296 1",
            "1 1 a 18446744073709551615",
        ] {
            let calls = Cell::new(0);
            let result = StripedTarget::parse_with(
                Sid::new(0),
                8,
                params,
                &mut |_| {
                    calls.set(calls.get() + 1);
                    Ok::<_, ()>(Backing.id())
                },
                &mut |_| {
                    calls.set(calls.get() + 1);
                    Ok(BlockDeviceLease::new_untracked(Arc::new(Backing)))
                },
            );
            assert!(result.is_err());
            assert_eq!(calls.get(), 0);
        }
        assert_eq!(
            parse("1 1 a 0", 0, 0).unwrap_err(),
            DmTargetParseError::Table(TableError::ZeroLength)
        );
        assert_eq!(
            parse("1 1 a 0", u64::MAX, 8).unwrap_err(),
            DmTargetParseError::Table(TableError::LogicalRangeOverflow)
        );
        assert_eq!(
            parse("2 2 a 125 b 0", 0, 8).unwrap_err(),
            DmTargetParseError::Table(TableError::BackingRangeOutOfBounds)
        );
        assert!(parse("1 1 a 120", 0, 8).is_ok());
    }

    #[ktest]
    fn rejects_stripe_resource_limits_before_lookup() {
        let oversized = String::from_utf8(alloc::vec![b' '; MAX_PARAMS_BYTES + 1]).unwrap();
        let too_many = format!("{} 1", MAX_STRIPES + 1);
        for (params, length) in [
            (oversized.as_str(), 8),
            (too_many.as_str(), u64::from(MAX_STRIPES + 1)),
        ] {
            let result = StripedTarget::parse_with(
                Sid::new(0),
                length,
                params,
                &mut |_| -> Result<DeviceId, ()> { panic!("must reject before lookup") },
                &mut |_| -> Result<BlockDeviceLease, ()> {
                    panic!("must reject before resolution")
                },
            );
            assert_eq!(
                result.unwrap_err(),
                DmTargetParseError::Table(TableError::InvalidTargetParams)
            );
        }
    }

    #[ktest]
    fn releases_partial_backing_leases_on_lookup_failure() {
        let backing = Arc::new(Backing);
        let calls = Cell::new(0);
        let result = StripedTarget::parse_with(
            Sid::new(0),
            8,
            "2 2 a 0 b 0",
            &mut |_| Ok(Backing.id()),
            &mut |_| {
                calls.set(calls.get() + 1);
                if calls.get() == 2 {
                    return Err(());
                }
                Ok(BlockDeviceLease::new_untracked(backing.clone()))
            },
        );
        assert_eq!(result.unwrap_err(), DmTargetParseError::ResolveBacking(()));
        assert_eq!(Arc::strong_count(&backing), 1);
    }
}
