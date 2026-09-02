// SPDX-License-Identifier: MPL-2.0

use core::ops::Range;

use aster_block::{BlockDevice, BlockDeviceLease, id::Sid};
use device_id::DeviceId;

use self::{error::ErrorTarget, linear::LinearTarget, striped::StripedTarget, zero::ZeroTarget};
use crate::TableError;

pub mod error;
pub mod linear;
pub mod striped;
pub mod zero;

#[derive(Clone, Copy, Debug)]
pub struct DmTargetMetadata {
    name: &'static str,
    version: [u32; 3],
}

impl DmTargetMetadata {
    pub const fn new(name: &'static str, version: [u32; 3]) -> Self {
        Self { name, version }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn version(&self) -> [u32; 3] {
        self.version
    }
}

const ERROR_METADATA: DmTargetMetadata = DmTargetMetadata::new("error", [1, 6, 0]);
const LINEAR_METADATA: DmTargetMetadata = DmTargetMetadata::new("linear", [1, 4, 0]);
const STRIPED_METADATA: DmTargetMetadata = DmTargetMetadata::new("striped", [1, 6, 0]);
const ZERO_METADATA: DmTargetMetadata = DmTargetMetadata::new("zero", [1, 1, 0]);

pub const SUPPORTED_TARGETS: &[DmTargetMetadata] = &[
    ERROR_METADATA,
    LINEAR_METADATA,
    STRIPED_METADATA,
    ZERO_METADATA,
];

/// Keeps target validation separate from environment-specific backing lookup
/// failures so callers can preserve their native error semantics.
#[derive(Debug, Eq, PartialEq)]
pub enum DmTargetParseError<E> {
    UnsupportedTarget,
    Table(TableError),
    ResolveBacking(E),
}

impl<E> From<TableError> for DmTargetParseError<E> {
    fn from(error: TableError) -> Self {
        Self::Table(error)
    }
}

#[derive(Debug)]
pub enum DmTarget {
    Error(ErrorTarget),
    Linear(LinearTarget),
    Striped(StripedTarget),
    Zero(ZeroTarget),
}

impl DmTarget {
    /// The resolver keeps environment-specific device lookup outside this
    /// component while target parsing remains responsible for target semantics.
    pub fn parse_with<E>(
        target_type: &str,
        logical_start: Sid,
        length: u64,
        params: &str,
        parse_backing: &mut impl FnMut(&str) -> Result<DeviceId, E>,
        resolve_backing: &mut impl FnMut(DeviceId) -> Result<BlockDeviceLease, E>,
    ) -> Result<Self, DmTargetParseError<E>> {
        match target_type {
            "error" => {
                require_empty_params(params)?;
                Ok(Self::Error(ErrorTarget::new(logical_start, length)?))
            }
            "linear" => {
                let (backing, backing_start) = parse_linear_params(params)?;
                let backing_start = Sid::new(backing_start);
                LinearTarget::validate_geometry(logical_start, length, backing_start)?;
                let backing = parse_backing(backing).map_err(DmTargetParseError::ResolveBacking)?;
                let backing =
                    resolve_backing(backing).map_err(DmTargetParseError::ResolveBacking)?;
                Ok(Self::Linear(LinearTarget::new(
                    logical_start,
                    length,
                    backing_start,
                    backing,
                )?))
            }
            "striped" => Ok(Self::Striped(StripedTarget::parse_with(
                logical_start,
                length,
                params,
                parse_backing,
                resolve_backing,
            )?)),
            "zero" => {
                require_empty_params(params)?;
                Ok(Self::Zero(ZeroTarget::new(logical_start, length)?))
            }
            _ => Err(DmTargetParseError::UnsupportedTarget),
        }
    }

    pub fn metadata(&self) -> DmTargetMetadata {
        match self {
            Self::Error(_) => ERROR_METADATA,
            Self::Linear(_) => LINEAR_METADATA,
            Self::Striped(_) => STRIPED_METADATA,
            Self::Zero(_) => ZERO_METADATA,
        }
    }

    pub fn name(&self) -> &'static str {
        self.metadata().name()
    }

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

fn require_empty_params<E>(params: &str) -> Result<(), DmTargetParseError<E>> {
    if params.trim().is_empty() {
        Ok(())
    } else {
        Err(TableError::InvalidTargetParams.into())
    }
}

fn parse_linear_params(params: &str) -> Result<(&str, u64), TableError> {
    let mut fields = params.split_ascii_whitespace();
    let backing = fields.next().ok_or(TableError::InvalidTargetParams)?;
    let backing_start = fields
        .next()
        .ok_or(TableError::InvalidTargetParams)?
        .parse::<u64>()
        .map_err(|_| TableError::InvalidTargetParams)?;
    if fields.next().is_some() {
        return Err(TableError::InvalidTargetParams);
    }

    Ok((backing, backing_start))
}

#[cfg(ktest)]
mod tests {
    use alloc::{string::String, sync::Arc, vec, vec::Vec};

    use aster_block::{
        BlockDeviceMeta,
        bio::{BioEnqueueError, SubmittedBio},
    };
    use device_id::{MajorId, MinorId};
    use ostd::prelude::ktest;

    use super::*;

    #[derive(Debug)]
    struct TestBlockDevice {
        id: DeviceId,
    }

    impl TestBlockDevice {
        fn new(minor: u32) -> Arc<Self> {
            Arc::new(Self {
                id: DeviceId::new(MajorId::new(510), MinorId::new(minor)),
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
                nr_sectors: 4096,
            }
        }

        fn name(&self) -> String {
            String::from("target-parser-test")
        }

        fn id(&self) -> DeviceId {
            self.id
        }
    }

    fn backing(minor: u32) -> BlockDeviceLease {
        BlockDeviceLease::new_untracked(TestBlockDevice::new(minor))
    }

    #[ktest]
    fn parses_targets_before_resolving_backings() {
        use core::cell::RefCell;

        let parsed = RefCell::new(Vec::new());
        let events = RefCell::new(Vec::new());
        let mut parse_backing = |token: &str| {
            events.borrow_mut().push("parse");
            parsed.borrow_mut().push(String::from(token));
            match token {
                "/dev/first" => Ok(DeviceId::new(MajorId::new(510), MinorId::new(1))),
                "510:2" => Ok(DeviceId::new(MajorId::new(510), MinorId::new(2))),
                _ => Err("missing"),
            }
        };
        let mut resolve_backing = |id: DeviceId| -> Result<BlockDeviceLease, &'static str> {
            events.borrow_mut().push("resolve");
            Ok(backing(id.minor().get()))
        };

        let linear = DmTarget::parse_with(
            "linear",
            Sid::new(0),
            8,
            "/dev/first 16",
            &mut parse_backing,
            &mut resolve_backing,
        )
        .unwrap();
        assert_eq!(linear.backing_id().unwrap().minor().get(), 1);
        assert_eq!(linear.backing_start(), Some(Sid::new(16)));

        let striped = DmTarget::parse_with(
            "striped",
            Sid::new(8),
            16,
            "2 4 /dev/first 32 510:2 64",
            &mut parse_backing,
            &mut resolve_backing,
        )
        .unwrap();
        let DmTarget::Striped(striped) = striped else {
            panic!("expected striped target");
        };
        assert_eq!(striped.stripe_count(), 2);
        assert_eq!(striped.chunk_size(), 4);
        assert_eq!(striped.backing_id(0).unwrap().minor().get(), 1);
        assert_eq!(striped.backing_id(1).unwrap().minor().get(), 2);
        assert_eq!(
            parsed.into_inner(),
            vec!["/dev/first", "/dev/first", "510:2"]
        );
        assert_eq!(
            events.into_inner(),
            vec!["parse", "resolve", "parse", "parse", "resolve", "resolve"]
        );
    }

    #[ktest]
    fn validates_target_type_and_parameter_shapes() {
        let mut parse_calls = 0;
        let mut resolve_calls = 0;
        let mut parse_backing = |_token: &str| -> Result<DeviceId, ()> {
            parse_calls += 1;
            Ok(DeviceId::new(MajorId::new(510), MinorId::new(1)))
        };
        let mut resolve_backing = |_id: DeviceId| -> Result<BlockDeviceLease, ()> {
            resolve_calls += 1;
            Ok(backing(1))
        };

        assert!(matches!(
            DmTarget::parse_with(
                "error",
                Sid::new(0),
                1,
                " \t",
                &mut parse_backing,
                &mut resolve_backing,
            )
            .unwrap(),
            DmTarget::Error(_)
        ));
        assert!(matches!(
            DmTarget::parse_with(
                "zero",
                Sid::new(0),
                1,
                "",
                &mut parse_backing,
                &mut resolve_backing,
            )
            .unwrap(),
            DmTarget::Zero(_)
        ));
        for (target_type, params) in [
            ("error", "unexpected"),
            ("zero", "unexpected"),
            ("linear", "510:1"),
            ("linear", "510:1 invalid"),
            ("linear", "510:1 0 extra"),
            ("striped", "2 4 510:1 0"),
        ] {
            assert_eq!(
                DmTarget::parse_with(
                    target_type,
                    Sid::new(0),
                    1,
                    params,
                    &mut parse_backing,
                    &mut resolve_backing,
                )
                .unwrap_err(),
                DmTargetParseError::Table(TableError::InvalidTargetParams),
                "target_type={target_type:?}, params={params:?}"
            );
        }
        assert_eq!(
            DmTarget::parse_with(
                "snapshot",
                Sid::new(0),
                1,
                "",
                &mut parse_backing,
                &mut resolve_backing,
            )
            .unwrap_err(),
            DmTargetParseError::UnsupportedTarget
        );
        assert_eq!(parse_calls, 0);
        assert_eq!(resolve_calls, 0);
    }

    #[ktest]
    fn validates_geometry_before_backing_lookup() {
        let mut parse_calls = 0;
        let mut resolve_calls = 0;
        let mut parse_backing = |_token: &str| -> Result<DeviceId, ()> {
            parse_calls += 1;
            Ok(DeviceId::new(MajorId::new(510), MinorId::new(1)))
        };
        let mut resolve_backing = |_id: DeviceId| -> Result<BlockDeviceLease, ()> {
            resolve_calls += 1;
            Ok(backing(1))
        };

        assert_eq!(
            DmTarget::parse_with(
                "linear",
                Sid::new(u64::MAX),
                1,
                "510:1 0",
                &mut parse_backing,
                &mut resolve_backing,
            )
            .unwrap_err(),
            DmTargetParseError::Table(TableError::LogicalRangeOverflow)
        );
        assert_eq!(
            DmTarget::parse_with(
                "striped",
                Sid::new(0),
                0,
                "1 4 510:1 0",
                &mut parse_backing,
                &mut resolve_backing,
            )
            .unwrap_err(),
            DmTargetParseError::Table(TableError::ZeroLength)
        );
        assert_eq!(
            DmTarget::parse_with(
                "striped",
                Sid::new(0),
                1,
                "1 4 510:1 18446744073709551615",
                &mut parse_backing,
                &mut resolve_backing,
            )
            .unwrap_err(),
            DmTargetParseError::Table(TableError::BackingRangeOverflow)
        );
        assert_eq!(parse_calls, 0);
        assert_eq!(resolve_calls, 0);
    }

    #[ktest]
    fn preserves_backing_parse_and_lookup_errors() {
        let mut parse_backing = |_token: &str| -> Result<DeviceId, &'static str> { Err("parse") };
        let mut resolve_backing =
            |_id: DeviceId| -> Result<BlockDeviceLease, &'static str> { Ok(backing(1)) };
        assert_eq!(
            DmTarget::parse_with(
                "linear",
                Sid::new(0),
                1,
                "/dev/missing 0",
                &mut parse_backing,
                &mut resolve_backing,
            )
            .unwrap_err(),
            DmTargetParseError::ResolveBacking("parse")
        );

        let mut parse_backing = |_token: &str| -> Result<DeviceId, &'static str> {
            Ok(DeviceId::new(MajorId::new(510), MinorId::new(1)))
        };
        let mut resolve_backing =
            |_id: DeviceId| -> Result<BlockDeviceLease, &'static str> { Err("lookup") };
        assert_eq!(
            DmTarget::parse_with(
                "striped",
                Sid::new(0),
                8,
                "1 4 /dev/missing 0",
                &mut parse_backing,
                &mut resolve_backing,
            )
            .unwrap_err(),
            DmTargetParseError::ResolveBacking("lookup")
        );
    }
}
