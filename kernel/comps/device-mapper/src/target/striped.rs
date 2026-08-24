// SPDX-License-Identifier: MPL-2.0

use alloc::vec::Vec;
use core::ops::Range;

use aster_block::{BlockDevice, BlockDeviceLease, id::Sid};
use device_id::{DeviceId, MajorId, MinorId};

use crate::TableError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StripedBacking {
    id: DeviceId,
    backing_start: Sid,
}

impl StripedBacking {
    pub fn id(&self) -> DeviceId {
        self.id
    }

    pub fn backing_start(&self) -> Sid {
        self.backing_start
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct StripedTargetParams {
    stripe_count: usize,
    chunk_size: u64,
    stripes: Vec<StripedBacking>,
}

impl StripedTargetParams {
    pub fn parse(params: &str) -> Result<Self, TableError> {
        let fields = params.split_ascii_whitespace().collect::<Vec<_>>();
        if fields.len() < 2 {
            return Err(TableError::InvalidTargetParams);
        }

        let stripe_count = fields[0]
            .parse::<usize>()
            .map_err(|_| TableError::InvalidTargetParams)?;
        if stripe_count == 0 {
            return Err(TableError::InvalidTargetParams);
        }

        let chunk_size = fields[1]
            .parse::<u64>()
            .map_err(|_| TableError::InvalidTargetParams)?;
        if chunk_size == 0 {
            return Err(TableError::InvalidTargetParams);
        }

        let expected_fields = stripe_count
            .checked_mul(2)
            .and_then(|value| value.checked_add(2))
            .ok_or(TableError::InvalidTargetParams)?;
        if fields.len() != expected_fields {
            return Err(TableError::InvalidTargetParams);
        }

        let mut stripes = Vec::new();
        for index in 0..stripe_count {
            let dev = fields[2 + index * 2];
            let backing_start = fields[3 + index * 2]
                .parse::<u64>()
                .map_err(|_| TableError::InvalidTargetParams)?;
            stripes.push(StripedBacking {
                id: parse_device_id(dev)?,
                backing_start: Sid::new(backing_start),
            });
        }

        Ok(Self {
            stripe_count,
            chunk_size,
            stripes,
        })
    }

    pub fn stripe_count(&self) -> usize {
        self.stripe_count
    }

    pub fn chunk_size(&self) -> u64 {
        self.chunk_size
    }

    pub fn stripes(&self) -> &[StripedBacking] {
        &self.stripes
    }

    pub fn required_sectors(&self, stripe_index: usize, length: u64) -> Result<u64, TableError> {
        if length == 0 {
            return Err(TableError::ZeroLength);
        }
        if stripe_index >= self.stripe_count {
            return Err(TableError::InvalidTargetParams);
        }

        let stripe_count =
            u64::try_from(self.stripe_count).map_err(|_| TableError::InvalidTargetParams)?;
        let stripe_width = stripe_count
            .checked_mul(self.chunk_size)
            .ok_or(TableError::BackingRangeOverflow)?;
        let full_rows = length / stripe_width;
        let remainder = length % stripe_width;
        let base = full_rows
            .checked_mul(self.chunk_size)
            .ok_or(TableError::BackingRangeOverflow)?;
        let stripe_remainder_start = u64::try_from(stripe_index)
            .map_err(|_| TableError::InvalidTargetParams)?
            .checked_mul(self.chunk_size)
            .ok_or(TableError::BackingRangeOverflow)?;
        let extra = if remainder > stripe_remainder_start {
            core::cmp::min(self.chunk_size, remainder - stripe_remainder_start)
        } else {
            0
        };
        base.checked_add(extra)
            .ok_or(TableError::BackingRangeOverflow)
    }

    pub fn validate_backing_ranges(
        &self,
        logical_start: Sid,
        length: u64,
        backing_capacities: &[u64],
    ) -> Result<(), TableError> {
        if length == 0 {
            return Err(TableError::ZeroLength);
        }
        logical_start
            .to_raw()
            .checked_add(length)
            .ok_or(TableError::LogicalRangeOverflow)?;
        if backing_capacities.len() != self.stripe_count {
            return Err(TableError::InvalidTargetParams);
        }

        for (index, stripe) in self.stripes.iter().enumerate() {
            let required = self.required_sectors(index, length)?;
            let backing_end = stripe
                .backing_start
                .to_raw()
                .checked_add(required)
                .ok_or(TableError::BackingRangeOverflow)?;
            if backing_end > backing_capacities[index] {
                return Err(TableError::BackingRangeOutOfBounds);
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct StripedTarget {
    logical_range: Range<Sid>,
    chunk_size: u64,
    stripe_width: u64,
    stripes: Vec<StripedTargetStripe>,
}

#[derive(Debug)]
struct StripedTargetStripe {
    backing_start: Sid,
    backing_id: DeviceId,
    backing: BlockDeviceLease,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StripedSectorMap {
    stripe_index: usize,
    backing_id: DeviceId,
    backing_sector: Sid,
}

impl StripedSectorMap {
    pub fn stripe_index(&self) -> usize {
        self.stripe_index
    }

    pub fn backing_id(&self) -> DeviceId {
        self.backing_id
    }

    pub fn backing_sector(&self) -> Sid {
        self.backing_sector
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StripedRangeMap {
    logical_range: Range<Sid>,
    stripe_index: usize,
    backing_id: DeviceId,
    backing_range: Range<Sid>,
}

impl StripedRangeMap {
    pub fn logical_range(&self) -> &Range<Sid> {
        &self.logical_range
    }

    pub fn stripe_index(&self) -> usize {
        self.stripe_index
    }

    pub fn backing_id(&self) -> DeviceId {
        self.backing_id
    }

    pub fn backing_range(&self) -> &Range<Sid> {
        &self.backing_range
    }
}

impl StripedTarget {
    pub fn new(
        logical_start: Sid,
        length: u64,
        params: StripedTargetParams,
        backings: Vec<BlockDeviceLease>,
    ) -> Result<Self, TableError> {
        if length == 0 {
            return Err(TableError::ZeroLength);
        }
        let logical_end = logical_start
            .to_raw()
            .checked_add(length)
            .ok_or(TableError::LogicalRangeOverflow)?;
        if backings.len() != params.stripe_count() {
            return Err(TableError::InvalidTargetParams);
        }

        let stripe_count =
            u64::try_from(params.stripe_count()).map_err(|_| TableError::InvalidTargetParams)?;
        let stripe_width = stripe_count
            .checked_mul(params.chunk_size())
            .ok_or(TableError::BackingRangeOverflow)?;
        let mut capacities = Vec::new();
        for (stripe, backing) in params.stripes().iter().zip(backings.iter()) {
            if backing.id() != stripe.id() {
                return Err(TableError::InvalidTargetParams);
            }
            capacities.push(
                u64::try_from(backing.metadata().nr_sectors)
                    .map_err(|_| TableError::BackingRangeOverflow)?,
            );
        }
        params.validate_backing_ranges(logical_start, length, &capacities)?;

        let stripes = params
            .stripes()
            .iter()
            .zip(backings)
            .map(|(stripe, backing)| StripedTargetStripe {
                backing_start: stripe.backing_start(),
                backing_id: stripe.id(),
                backing,
            })
            .collect();

        Ok(Self {
            logical_range: logical_start..Sid::new(logical_end),
            chunk_size: params.chunk_size(),
            stripe_width,
            stripes,
        })
    }

    pub fn logical_range(&self) -> &Range<Sid> {
        &self.logical_range
    }

    pub fn length(&self) -> u64 {
        self.logical_range.end.to_raw() - self.logical_range.start.to_raw()
    }

    pub fn chunk_size(&self) -> u64 {
        self.chunk_size
    }

    pub fn stripe_count(&self) -> usize {
        self.stripes.len()
    }

    pub fn stripe_width(&self) -> u64 {
        self.stripe_width
    }

    pub fn backing_id(&self, stripe_index: usize) -> Option<DeviceId> {
        self.stripes
            .get(stripe_index)
            .map(|stripe| stripe.backing_id)
    }

    pub fn for_each_backing_id(&self, mut f: impl FnMut(DeviceId)) {
        for stripe in &self.stripes {
            f(stripe.backing_id);
        }
    }

    pub fn for_each_stripe(&self, mut f: impl FnMut(DeviceId, Sid)) {
        for stripe in &self.stripes {
            f(stripe.backing_id, stripe.backing_start);
        }
    }

    pub fn backing(&self, stripe_index: usize) -> Option<&dyn BlockDevice> {
        self.stripes
            .get(stripe_index)
            .map(|stripe| stripe.backing.device().as_ref())
    }

    pub fn for_each_backing<'a>(&'a self, mut f: impl FnMut(&'a dyn BlockDevice)) {
        for stripe in &self.stripes {
            f(stripe.backing.device().as_ref());
        }
    }

    pub fn map_sector(&self, logical: Sid) -> Option<StripedSectorMap> {
        if !self.logical_range.contains(&logical) {
            return None;
        }

        let offset = logical
            .to_raw()
            .checked_sub(self.logical_range.start.to_raw())?;
        let row = offset / self.stripe_width;
        let within_row = offset % self.stripe_width;
        let stripe_index = usize::try_from(within_row / self.chunk_size).ok()?;
        let chunk_offset = within_row % self.chunk_size;
        let backing_offset = row
            .checked_mul(self.chunk_size)?
            .checked_add(chunk_offset)?;
        let stripe = self.stripes.get(stripe_index)?;
        let backing_sector = stripe
            .backing_start
            .to_raw()
            .checked_add(backing_offset)
            .map(Sid::new)?;
        Some(StripedSectorMap {
            stripe_index,
            backing_id: stripe.backing_id,
            backing_sector,
        })
    }

    pub fn map_range(&self, logical: Range<Sid>) -> Option<Vec<StripedRangeMap>> {
        let start = logical.start.to_raw();
        let end = logical.end.to_raw();
        if start >= end {
            return None;
        }
        if start < self.logical_range.start.to_raw() || end > self.logical_range.end.to_raw() {
            return None;
        }

        let mut cursor = start;
        let mut parts = Vec::new();
        while cursor < end {
            let sector = self.map_sector(Sid::new(cursor))?;
            let offset = cursor.checked_sub(self.logical_range.start.to_raw())?;
            let within_row = offset % self.stripe_width;
            let chunk_offset = within_row % self.chunk_size;
            let remaining_in_chunk = self.chunk_size.checked_sub(chunk_offset)?;
            let max_part_end = cursor.checked_add(remaining_in_chunk)?;
            let part_end = core::cmp::min(end, max_part_end);
            let part_len = part_end.checked_sub(cursor)?;
            let backing_start = sector.backing_sector().to_raw();
            let backing_end = backing_start.checked_add(part_len)?;
            parts.push(StripedRangeMap {
                logical_range: Sid::new(cursor)..Sid::new(part_end),
                stripe_index: sector.stripe_index(),
                backing_id: sector.backing_id(),
                backing_range: Sid::new(backing_start)..Sid::new(backing_end),
            });
            cursor = part_end;
        }
        Some(parts)
    }
}

fn parse_device_id(dev: &str) -> Result<DeviceId, TableError> {
    let (major, minor) = dev.split_once(':').ok_or(TableError::InvalidTargetParams)?;
    let major = major
        .parse::<u16>()
        .map_err(|_| TableError::InvalidTargetParams)?;
    let minor = minor
        .parse::<u32>()
        .map_err(|_| TableError::InvalidTargetParams)?;
    let major = MajorId::try_from(major).map_err(|_| TableError::InvalidTargetParams)?;
    let minor = MinorId::try_from(minor).map_err(|_| TableError::InvalidTargetParams)?;
    Ok(DeviceId::new(major, minor))
}

#[cfg(ktest)]
mod tests {
    use alloc::{string::String, sync::Arc, vec};

    use aster_block::{
        BlockDeviceMeta,
        bio::{BioEnqueueError, SubmittedBio},
    };
    use ostd::prelude::ktest;

    use super::*;

    #[derive(Debug)]
    struct TestBlockDevice {
        id: DeviceId,
        nr_sectors: usize,
    }

    impl TestBlockDevice {
        fn new(minor: u32, nr_sectors: usize) -> Arc<Self> {
            Arc::new(Self {
                id: DeviceId::new(MajorId::new(510), MinorId::new(minor)),
                nr_sectors,
            })
        }
    }

    impl BlockDevice for TestBlockDevice {
        fn enqueue(&self, _bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            unreachable!()
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 8,
                nr_sectors: self.nr_sectors,
            }
        }

        fn name(&self) -> String {
            String::from("striped-test")
        }

        fn id(&self) -> DeviceId {
            self.id
        }
    }

    fn backing(minor: u32, nr_sectors: usize) -> BlockDeviceLease {
        BlockDeviceLease::new_untracked(TestBlockDevice::new(minor, nr_sectors))
    }

    fn mapped(target: &StripedTarget, logical: u64) -> Option<(usize, u32, u64)> {
        target.map_sector(Sid::new(logical)).map(|map| {
            (
                map.stripe_index(),
                map.backing_id().minor().get(),
                map.backing_sector().to_raw(),
            )
        })
    }

    fn mapped_range(
        target: &StripedTarget,
        start: u64,
        end: u64,
    ) -> Option<Vec<(u64, u64, usize, u32, u64, u64)>> {
        target
            .map_range(Sid::new(start)..Sid::new(end))
            .map(|parts| {
                parts
                    .into_iter()
                    .map(|part| {
                        (
                            part.logical_range().start.to_raw(),
                            part.logical_range().end.to_raw(),
                            part.stripe_index(),
                            part.backing_id().minor().get(),
                            part.backing_range().start.to_raw(),
                            part.backing_range().end.to_raw(),
                        )
                    })
                    .collect()
            })
    }

    fn two_stripe_target() -> StripedTarget {
        let params = StripedTargetParams::parse("2 4 510:1 100 510:2 200").unwrap();
        StripedTarget::new(
            Sid::new(10),
            24,
            params,
            vec![backing(1, 128), backing(2, 228)],
        )
        .unwrap()
    }

    #[ktest]
    fn parses_exact_striped_parameter_set() {
        let params = StripedTargetParams::parse("2 128 510:1 0 510:2 2048").unwrap();

        assert_eq!(params.stripe_count(), 2);
        assert_eq!(params.chunk_size(), 128);
        assert_eq!(params.stripes().len(), 2);
        assert_eq!(params.stripes()[0].id().major().get(), 510);
        assert_eq!(params.stripes()[0].id().minor().get(), 1);
        assert_eq!(params.stripes()[0].backing_start(), Sid::new(0));
        assert_eq!(params.stripes()[1].id().major().get(), 510);
        assert_eq!(params.stripes()[1].id().minor().get(), 2);
        assert_eq!(params.stripes()[1].backing_start(), Sid::new(2048));

        let params = StripedTargetParams::parse("  2\t128\n510:1 0\t510:2 0  ").unwrap();
        assert_eq!(params.stripe_count(), 2);
        assert_eq!(params.chunk_size(), 128);
    }

    #[ktest]
    fn rejects_invalid_striped_parameter_sets() {
        for params in [
            "",
            "2",
            "0 128 510:1 0",
            "2 0 510:1 0 510:2 0",
            "2 128 510:1 0 510:2",
            "2 128 510:1 0 510:2 0 extra",
            "many 128 510:1 0",
            "2 many 510:1 0 510:2 0",
            "2 128 bad 0 510:2 0",
            "2 128 510 0 510:2 0",
            "2 128 65536:1 0 510:2 0",
            "2 128 510:4294967296 0 510:2 0",
            "2 128 510:1 -1 510:2 0",
            "2 128 510:1 18446744073709551616 510:2 0",
        ] {
            assert_eq!(
                StripedTargetParams::parse(params).unwrap_err(),
                TableError::InvalidTargetParams,
                "params={params:?}"
            );
        }
    }

    #[ktest]
    fn calculates_required_sectors_for_uneven_stripes() {
        let two = StripedTargetParams::parse("2 128 510:1 0 510:2 0").unwrap();
        assert_eq!(two.required_sectors(0, 300).unwrap(), 172);
        assert_eq!(two.required_sectors(1, 300).unwrap(), 128);

        let three = StripedTargetParams::parse("3 64 510:1 0 510:2 0 510:3 0").unwrap();
        assert_eq!(three.required_sectors(0, 400).unwrap(), 144);
        assert_eq!(three.required_sectors(1, 400).unwrap(), 128);
        assert_eq!(three.required_sectors(2, 400).unwrap(), 128);
    }

    #[ktest]
    fn rejects_invalid_required_sector_inputs() {
        let params = StripedTargetParams::parse("2 128 510:1 0 510:2 0").unwrap();

        assert_eq!(
            params.required_sectors(0, 0).unwrap_err(),
            TableError::ZeroLength
        );
        assert_eq!(
            params.required_sectors(2, 1).unwrap_err(),
            TableError::InvalidTargetParams
        );

        let overflowing =
            StripedTargetParams::parse("2 9223372036854775808 510:1 0 510:2 0").unwrap();
        assert_eq!(
            overflowing.required_sectors(0, 1).unwrap_err(),
            TableError::BackingRangeOverflow
        );
    }

    #[ktest]
    fn validates_backing_ranges_for_striped_targets() {
        let params = StripedTargetParams::parse("2 128 510:1 10 510:2 20").unwrap();

        params
            .validate_backing_ranges(Sid::new(0), 300, &[182, 148])
            .unwrap();
        assert_eq!(
            params
                .validate_backing_ranges(Sid::new(0), 300, &[181, 148])
                .unwrap_err(),
            TableError::BackingRangeOutOfBounds
        );
        assert_eq!(
            params
                .validate_backing_ranges(Sid::new(u64::MAX), 1, &[u64::MAX, u64::MAX])
                .unwrap_err(),
            TableError::LogicalRangeOverflow
        );
        assert_eq!(
            params
                .validate_backing_ranges(Sid::new(0), 0, &[10, 10])
                .unwrap_err(),
            TableError::ZeroLength
        );
        assert_eq!(
            params
                .validate_backing_ranges(Sid::new(0), 1, &[10])
                .unwrap_err(),
            TableError::InvalidTargetParams
        );

        let overflowing = StripedTargetParams::parse("1 1 510:1 18446744073709551615").unwrap();
        assert_eq!(
            overflowing
                .validate_backing_ranges(Sid::new(0), 1, &[u64::MAX])
                .unwrap_err(),
            TableError::BackingRangeOverflow
        );
    }

    #[ktest]
    fn constructs_striped_target_with_resolved_backings() {
        let params = StripedTargetParams::parse("2 128 510:1 10 510:2 20").unwrap();
        let target = StripedTarget::new(
            Sid::new(0),
            300,
            params,
            vec![backing(1, 182), backing(2, 148)],
        )
        .unwrap();

        assert_eq!(target.logical_range(), &(Sid::new(0)..Sid::new(300)));
        assert_eq!(target.length(), 300);
        assert_eq!(target.chunk_size(), 128);
        assert_eq!(target.stripe_count(), 2);
        assert_eq!(target.stripe_width(), 256);
        assert_eq!(target.backing_id(0).unwrap().minor().get(), 1);
        assert_eq!(target.backing_id(1).unwrap().minor().get(), 2);
        assert!(target.backing(2).is_none());
    }

    #[ktest]
    fn rejects_invalid_striped_target_construction() {
        let params = || StripedTargetParams::parse("2 128 510:1 10 510:2 20").unwrap();

        assert_eq!(
            StripedTarget::new(
                Sid::new(0),
                0,
                params(),
                vec![backing(1, 182), backing(2, 148)]
            )
            .unwrap_err(),
            TableError::ZeroLength
        );
        assert_eq!(
            StripedTarget::new(
                Sid::new(u64::MAX),
                1,
                params(),
                vec![backing(1, 182), backing(2, 148)]
            )
            .unwrap_err(),
            TableError::LogicalRangeOverflow
        );
        assert_eq!(
            StripedTarget::new(Sid::new(0), 300, params(), vec![backing(1, 182)]).unwrap_err(),
            TableError::InvalidTargetParams
        );
        assert_eq!(
            StripedTarget::new(
                Sid::new(0),
                300,
                params(),
                vec![backing(1, 182), backing(3, 148)]
            )
            .unwrap_err(),
            TableError::InvalidTargetParams
        );
        assert_eq!(
            StripedTarget::new(
                Sid::new(0),
                300,
                params(),
                vec![backing(1, 181), backing(2, 148)]
            )
            .unwrap_err(),
            TableError::BackingRangeOutOfBounds
        );

        let overflowing = StripedTargetParams::parse("1 1 510:1 18446744073709551615").unwrap();
        assert_eq!(
            StripedTarget::new(Sid::new(0), 1, overflowing, vec![backing(1, usize::MAX)])
                .unwrap_err(),
            TableError::BackingRangeOverflow
        );
    }

    #[ktest]
    fn iterates_all_striped_backings() {
        let target = two_stripe_target();

        let mut ids = Vec::new();
        target.for_each_backing_id(|id| ids.push(id.minor().get()));
        assert_eq!(ids, vec![1, 2]);

        let mut backing_ids = Vec::new();
        target.for_each_backing(|backing| backing_ids.push(backing.id().minor().get()));
        assert_eq!(backing_ids, vec![1, 2]);

        let mut stripes = Vec::new();
        target.for_each_stripe(|id, backing_start| {
            stripes.push((id.minor().get(), backing_start.to_raw()))
        });
        assert_eq!(stripes, vec![(1, 100), (2, 200)]);
    }

    #[ktest]
    fn maps_two_stripe_chunk_boundaries() {
        let target = two_stripe_target();

        assert_eq!(mapped(&target, 9), None);
        assert_eq!(mapped(&target, 10), Some((0, 1, 100)));
        assert_eq!(mapped(&target, 13), Some((0, 1, 103)));
        assert_eq!(mapped(&target, 14), Some((1, 2, 200)));
        assert_eq!(mapped(&target, 17), Some((1, 2, 203)));
        assert_eq!(mapped(&target, 18), Some((0, 1, 104)));
        assert_eq!(mapped(&target, 21), Some((0, 1, 107)));
        assert_eq!(mapped(&target, 22), Some((1, 2, 204)));
        assert_eq!(mapped(&target, 33), Some((1, 2, 211)));
        assert_eq!(mapped(&target, 34), None);
    }

    #[ktest]
    fn maps_range_inside_single_stripe_chunk() {
        let target = two_stripe_target();

        assert_eq!(
            mapped_range(&target, 11, 13).unwrap(),
            vec![(11, 13, 0, 1, 101, 103)]
        );
    }

    #[ktest]
    fn splits_range_at_stripe_chunk_boundaries() {
        let target = two_stripe_target();

        assert_eq!(
            mapped_range(&target, 12, 23).unwrap(),
            vec![
                (12, 14, 0, 1, 102, 104),
                (14, 18, 1, 2, 200, 204),
                (18, 22, 0, 1, 104, 108),
                (22, 23, 1, 2, 204, 205),
            ]
        );
    }

    #[ktest]
    fn maps_range_ending_at_target_end() {
        let target = two_stripe_target();

        assert_eq!(
            mapped_range(&target, 30, 34).unwrap(),
            vec![(30, 34, 1, 2, 208, 212)]
        );
    }

    #[ktest]
    fn rejects_empty_and_out_of_range_ranges() {
        let target = two_stripe_target();

        assert_eq!(target.map_range(Sid::new(10)..Sid::new(10)), None);
        assert_eq!(target.map_range(Sid::new(9)..Sid::new(10)), None);
        assert_eq!(target.map_range(Sid::new(33)..Sid::new(35)), None);
        assert_eq!(target.map_range(Sid::new(34)..Sid::new(35)), None);
        assert_eq!(target.map_range(Sid::new(35)..Sid::new(34)), None);
    }

    #[ktest]
    fn map_range_parts_match_single_sector_mapping() {
        let target = two_stripe_target();
        let parts = target.map_range(Sid::new(12)..Sid::new(23)).unwrap();

        for part in parts {
            let sector = target.map_sector(part.logical_range().start).unwrap();
            assert_eq!(part.stripe_index(), sector.stripe_index());
            assert_eq!(part.backing_id(), sector.backing_id());
            assert_eq!(part.backing_range().start, sector.backing_sector());
        }
    }

    #[ktest]
    fn maps_three_stripe_partial_final_row() {
        let params = StripedTargetParams::parse("3 3 510:1 10 510:2 20 510:3 30").unwrap();
        let target = StripedTarget::new(
            Sid::new(0),
            20,
            params,
            vec![backing(1, 32), backing(2, 29), backing(3, 36)],
        )
        .unwrap();

        assert_eq!(mapped(&target, 0), Some((0, 1, 10)));
        assert_eq!(mapped(&target, 2), Some((0, 1, 12)));
        assert_eq!(mapped(&target, 3), Some((1, 2, 20)));
        assert_eq!(mapped(&target, 5), Some((1, 2, 22)));
        assert_eq!(mapped(&target, 6), Some((2, 3, 30)));
        assert_eq!(mapped(&target, 8), Some((2, 3, 32)));
        assert_eq!(mapped(&target, 9), Some((0, 1, 13)));
        assert_eq!(mapped(&target, 17), Some((2, 3, 35)));
        assert_eq!(mapped(&target, 18), Some((0, 1, 16)));
        assert_eq!(mapped(&target, 19), Some((0, 1, 17)));
        assert_eq!(mapped(&target, 20), None);
    }

    #[ktest]
    fn splits_three_stripe_range_across_partial_final_row() {
        let params = StripedTargetParams::parse("3 3 510:1 10 510:2 20 510:3 30").unwrap();
        let target = StripedTarget::new(
            Sid::new(0),
            20,
            params,
            vec![backing(1, 32), backing(2, 29), backing(3, 36)],
        )
        .unwrap();

        assert_eq!(
            mapped_range(&target, 2, 20).unwrap(),
            vec![
                (2, 3, 0, 1, 12, 13),
                (3, 6, 1, 2, 20, 23),
                (6, 9, 2, 3, 30, 33),
                (9, 12, 0, 1, 13, 16),
                (12, 15, 1, 2, 23, 26),
                (15, 18, 2, 3, 33, 36),
                (18, 20, 0, 1, 16, 18),
            ]
        );
    }
}
