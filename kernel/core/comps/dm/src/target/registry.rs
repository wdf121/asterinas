// SPDX-License-Identifier: MPL-2.0

//! Target type registration and name-based construction.
//!
//! Registrations live for the boot session. Lookup copies a static factory
//! reference so no registry lock is held during parsing or backing lookup.

use alloc::vec::Vec;

use aster_block::{BlockDeviceLease, id::Sid};
use device_id::DeviceId;
use ostd::sync::Mutex;

use super::{DmTargetBox, DmTargetMetadata, DmTargetParseError, linear, striped};

/// Opaque failure propagated from an environment-specific backing lookup.
#[derive(Debug)]
pub struct BackingLookupError {
    source: LookupSource,
}

#[derive(Debug)]
enum LookupSource {
    Parse,
    Resolve,
}

/// Construction and identity of a registered target type, not a target instance.
pub trait DmTargetType: Send + Sync {
    /// Returns the name and version shared by registration and instance reporting.
    ///
    /// This identity must remain unchanged for the entire boot session.
    fn metadata(&self) -> DmTargetMetadata;

    /// Creates an instance using caller-provided backing lookup hooks.
    ///
    /// The instance must report the registered name and version. On the first
    /// lookup hook error, immediately return `DmTargetParseError::ResolveBacking`
    /// containing that error. Do not retry, continue probing, replace the error,
    /// or retain it for a later construction call.
    fn create(
        &self,
        logical_start: Sid,
        length: u64,
        params: &str,
        parse_backing: &mut dyn FnMut(&str) -> Result<DeviceId, BackingLookupError>,
        resolve_backing: &mut dyn FnMut(DeviceId) -> Result<BlockDeviceLease, BackingLookupError>,
    ) -> Result<DmTargetBox, DmTargetParseError<BackingLookupError>>;
}

/// Failure to register a target type.
#[derive(Debug, Eq, PartialEq)]
pub enum TargetRegistrationError {
    /// A type with the same Linux-visible name is already registered.
    DuplicateName,
    /// The name is empty, contains a NUL, or exceeds Linux's target name field.
    InvalidName,
}

#[derive(Clone, Copy)]
struct RegisteredType {
    metadata: DmTargetMetadata,
    factory: &'static dyn DmTargetType,
}

impl RegisteredType {
    fn create(
        &self,
        logical_start: Sid,
        length: u64,
        params: &str,
        parse_backing: &mut dyn FnMut(&str) -> Result<DeviceId, BackingLookupError>,
        resolve_backing: &mut dyn FnMut(DeviceId) -> Result<BlockDeviceLease, BackingLookupError>,
    ) -> Result<DmTargetBox, DmTargetParseError<BackingLookupError>> {
        if self.factory.metadata() != self.metadata {
            return Err(crate::TableError::InvalidTargetParams.into());
        }
        let target = self.factory.create(
            logical_start,
            length,
            params,
            parse_backing,
            resolve_backing,
        )?;
        if target.metadata() != self.metadata {
            return Err(crate::TableError::InvalidTargetParams.into());
        }
        Ok(target)
    }
}

struct TargetRegistry {
    types: Mutex<Vec<RegisteredType>>,
}

impl TargetRegistry {
    const fn new() -> Self {
        Self {
            types: Mutex::new(Vec::new()),
        }
    }

    fn register(&self, factory: &'static dyn DmTargetType) -> Result<(), TargetRegistrationError> {
        let metadata = factory.metadata();
        let name = metadata.name();
        // Linux DM_MAX_TYPE_NAME includes the terminating NUL.
        if name.is_empty() || name.len() >= 16 || name.as_bytes().contains(&0) {
            return Err(TargetRegistrationError::InvalidName);
        }
        let mut types = self.types.lock();
        if types.iter().any(|entry| entry.metadata.name() == name) {
            return Err(TargetRegistrationError::DuplicateName);
        }
        types.push(RegisteredType { metadata, factory });
        Ok(())
    }

    fn lookup(&self, name: &str) -> Option<RegisteredType> {
        self.types
            .lock()
            .iter()
            .find(|entry| entry.metadata.name() == name)
            .copied()
    }

    fn metadata(&self) -> Vec<DmTargetMetadata> {
        self.types
            .lock()
            .iter()
            .map(|entry| entry.metadata)
            .collect()
    }
}

static REGISTRY: TargetRegistry = TargetRegistry::new();
static INITIALIZED: Mutex<bool> = Mutex::new(false);

fn registry() -> &'static TargetRegistry {
    let mut initialized = INITIALIZED.lock();
    if !*initialized {
        REGISTRY.register(&linear::TARGET_TYPE).unwrap();
        REGISTRY.register(&striped::TARGET_TYPE).unwrap();
        *initialized = true;
    }
    &REGISTRY
}

/// Registers a target type for this boot session without replacing existing types.
///
/// The factory must remain available permanently. Runtime unloading is unsupported.
pub fn register_target_type(
    factory: &'static dyn DmTargetType,
) -> Result<(), TargetRegistrationError> {
    registry().register(factory)
}

/// Returns a snapshot of all registered target names and versions.
pub fn reported_target_metadata() -> Vec<DmTargetMetadata> {
    registry().metadata()
}

/// Parses a registered target while preserving the caller's backing lookup errors.
pub fn parse_target_with<E>(
    target_type: &str,
    logical_start: Sid,
    length: u64,
    params: &str,
    parse_backing: &mut impl FnMut(&str) -> Result<DeviceId, E>,
    resolve_backing: &mut impl FnMut(DeviceId) -> Result<BlockDeviceLease, E>,
) -> Result<DmTargetBox, DmTargetParseError<E>> {
    let factory = registry()
        .lookup(target_type)
        .ok_or(DmTargetParseError::UnsupportedTarget)?;
    let mut parse_error = None;
    let mut resolve_error = None;
    let result = factory.create(
        logical_start,
        length,
        params,
        &mut |token| {
            parse_backing(token).map_err(|error| {
                parse_error = Some(error);
                BackingLookupError {
                    source: LookupSource::Parse,
                }
            })
        },
        &mut |id| {
            resolve_backing(id).map_err(|error| {
                resolve_error = Some(error);
                BackingLookupError {
                    source: LookupSource::Resolve,
                }
            })
        },
    );
    result.map_err(|error| match error {
        DmTargetParseError::UnsupportedTarget => DmTargetParseError::UnsupportedTarget,
        DmTargetParseError::Table(error) => DmTargetParseError::Table(error),
        DmTargetParseError::ResolveBacking(error) => {
            // The create contract requires immediate propagation of the first
            // hook error, so this slot still contains that token's original error.
            let original = match error.source {
                LookupSource::Parse => parse_error,
                LookupSource::Resolve => resolve_error,
            };
            DmTargetParseError::ResolveBacking(original.unwrap())
        }
    })
}

#[cfg(ktest)]
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use ostd::prelude::ktest;

    use super::*;
    use crate::TableError;

    struct TestType(&'static str);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static CUSTOM: TestType = TestType("registry-test");
    static DUPLICATE: TestType = TestType("registry-test");
    static EMPTY: TestType = TestType("");
    static LONG: TestType = TestType("0123456789abcdef");
    static NUL: TestType = TestType("bad\0name");

    impl DmTargetType for TestType {
        fn metadata(&self) -> DmTargetMetadata {
            DmTargetMetadata::new(self.0, [2, 3, 4])
        }

        fn create(
            &self,
            logical_start: Sid,
            length: u64,
            params: &str,
            _parse_backing: &mut dyn FnMut(&str) -> Result<DeviceId, BackingLookupError>,
            _resolve_backing: &mut dyn FnMut(
                DeviceId,
            )
                -> Result<BlockDeviceLease, BackingLookupError>,
        ) -> Result<DmTargetBox, DmTargetParseError<BackingLookupError>> {
            assert_eq!(logical_start, Sid::new(7));
            assert_eq!(length, 8);
            assert_eq!(params, "private parameters");
            // Reentrant registry access must not run under the lookup lock.
            assert!(
                reported_target_metadata()
                    .iter()
                    .any(|entry| entry.name() == "linear")
            );
            CALLS.fetch_add(1, Ordering::Relaxed);
            Err(TableError::InvalidTargetParams.into())
        }
    }

    #[derive(Debug)]
    struct IdentityTarget(DmTargetMetadata);

    impl super::super::DmTarget for IdentityTarget {
        fn as_any(&self) -> &dyn core::any::Any {
            self
        }

        fn metadata(&self) -> DmTargetMetadata {
            self.0
        }

        fn logical_range(&self) -> &core::ops::Range<Sid> {
            unreachable!()
        }

        fn for_each_backing<'a>(&'a self, _: &mut dyn FnMut(&'a dyn aster_block::BlockDevice)) {}

        fn map_io_range(
            &self,
            _: core::ops::Range<Sid>,
        ) -> Option<Vec<super::super::TargetIoAction<'_>>> {
            unreachable!()
        }

        fn status_params(
            &self,
            _: super::super::TargetStatusMode,
        ) -> Result<alloc::string::String, TableError> {
            unreachable!()
        }
    }

    struct IdentityType;
    static IDENTITY: IdentityType = IdentityType;

    impl DmTargetType for IdentityType {
        fn metadata(&self) -> DmTargetMetadata {
            DmTargetMetadata::new("identity-test", [1, 0, 0])
        }

        fn create(
            &self,
            _: Sid,
            _: u64,
            params: &str,
            _: &mut dyn FnMut(&str) -> Result<DeviceId, BackingLookupError>,
            _: &mut dyn FnMut(DeviceId) -> Result<BlockDeviceLease, BackingLookupError>,
        ) -> Result<DmTargetBox, DmTargetParseError<BackingLookupError>> {
            let metadata = match params {
                "name" => DmTargetMetadata::new("wrong-name", [1, 0, 0]),
                "version" => DmTargetMetadata::new("identity-test", [2, 0, 0]),
                "match" => self.metadata(),
                _ => unreachable!(),
            };
            Ok(alloc::boxed::Box::new(IdentityTarget(metadata)))
        }
    }

    #[ktest]
    fn creation_checks_registered_factory_and_instance_identity() {
        let registry = TargetRegistry::new();
        registry.register(&IDENTITY).unwrap();
        let entry = registry.lookup("identity-test").unwrap();
        for params in ["name", "version"] {
            assert!(matches!(
                entry.create(
                    Sid::new(0),
                    8,
                    params,
                    &mut |_| unreachable!(),
                    &mut |_| unreachable!()
                ),
                Err(DmTargetParseError::Table(TableError::InvalidTargetParams))
            ));
        }
        let target = entry
            .create(
                Sid::new(0),
                8,
                "match",
                &mut |_| unreachable!(),
                &mut |_| unreachable!(),
            )
            .unwrap();
        assert_eq!(target.metadata(), entry.metadata);

        // Model a factory whose current identity differs from its registration snapshot.
        let stale = RegisteredType {
            metadata: DmTargetMetadata::new("identity-test", [0, 0, 0]),
            factory: &IDENTITY,
        };
        assert!(matches!(
            stale.create(
                Sid::new(0),
                8,
                "must not construct",
                &mut |_| unreachable!(),
                &mut |_| unreachable!()
            ),
            Err(DmTargetParseError::Table(TableError::InvalidTargetParams))
        ));
    }

    #[ktest]
    fn registers_types_rejects_duplicates_and_dispatches_factory() {
        let registry = TargetRegistry::new();
        assert_eq!(registry.register(&CUSTOM), Ok(()));
        assert_eq!(
            registry.register(&DUPLICATE),
            Err(TargetRegistrationError::DuplicateName)
        );
        for invalid in [&EMPTY, &LONG, &NUL] {
            assert_eq!(
                registry.register(invalid),
                Err(TargetRegistrationError::InvalidName)
            );
        }
        let metadata = registry.metadata();
        assert_eq!(metadata.len(), 1);
        assert_eq!(metadata[0].name(), "registry-test");
        assert_eq!(metadata[0].version(), [2, 3, 4]);
        assert!(registry.lookup("missing").is_none());
        let factory = registry.lookup("registry-test").unwrap();
        assert!(matches!(
            factory.create(
                Sid::new(7),
                8,
                "private parameters",
                &mut |_| unreachable!(),
                &mut |_| unreachable!()
            ),
            Err(DmTargetParseError::Table(TableError::InvalidTargetParams))
        ));
        assert_eq!(CALLS.load(Ordering::Relaxed), 1);
        assert_eq!(registry.metadata().len(), 1);
    }

    #[ktest]
    fn dynamic_creation_preserves_backing_hook_errors() {
        for name in ["linear", "striped"] {
            let params = if name == "linear" {
                "disk 0"
            } else {
                "2 1 disk 0 other 0"
            };
            let mut parse_calls = 0;
            let result = parse_target_with(
                name,
                Sid::new(0),
                8,
                params,
                &mut |_| {
                    parse_calls += 1;
                    Err::<DeviceId, _>("parse failure")
                },
                &mut |_| unreachable!(),
            );
            assert!(matches!(
                result,
                Err(DmTargetParseError::ResolveBacking("parse failure"))
            ));
            assert_eq!(parse_calls, 1);
            parse_calls = 0;
            let mut resolve_calls = 0;
            let result = parse_target_with(
                name,
                Sid::new(0),
                8,
                params,
                &mut |_| {
                    parse_calls += 1;
                    Ok(DeviceId::new(
                        device_id::MajorId::new(1),
                        device_id::MinorId::new(1),
                    ))
                },
                &mut |_| {
                    resolve_calls += 1;
                    Err::<BlockDeviceLease, _>("lookup failure")
                },
            );
            assert!(matches!(
                result,
                Err(DmTargetParseError::ResolveBacking("lookup failure"))
            ));
            assert_eq!(parse_calls, 1);
            assert_eq!(resolve_calls, 1);
        }
        assert_eq!(
            register_target_type(&linear::TARGET_TYPE),
            Err(TargetRegistrationError::DuplicateName)
        );
    }
}
