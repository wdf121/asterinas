// SPDX-License-Identifier: MPL-2.0

//! The block devices of Asterinas.
//！
//！This crate provides a number of base components for block devices, including
//! an abstraction of block devices, as well as the registration and lookup of block devices.
//!
//! Block devices use a queue-based model for asynchronous I/O operations. It is necessary
//! for a block device to maintain a queue to handle I/O requests. The users (e.g., fs)
//! submit I/O requests to this queue and wait for their completion. Drivers implementing
//! block devices can create their own queues as needed, with the possibility to reorder
//! and merge requests within the queue.
//!
//! This crate also offers the `Bio` related data structures and APIs to accomplish
//! safe and convenient block I/O operations, for example:
//!
//! ```no_run
//! // Creates a bio request.
//! let bio = Bio::new(BioType::Write, sid, segments, None);
//! // Submits to the block device.
//! let mut io_batch = IoBatch::new();
//! bio.submit(block_device, &mut io_batch)?;
//! // Waits for the the completion.
//! io_batch.wait_all()?;
//! ```
//!
#![no_std]
#![deny(unsafe_code)]
#![feature(step_trait)]

extern crate alloc;
#[macro_use]
extern crate ostd_pod;

// Set this crate's log prefix for `ostd::log`.
macro_rules! __log_prefix {
    () => {
        "block: "
    };
}

pub mod bio;
mod device_id;
pub mod id;
mod impl_block_device;
mod partition;
mod prelude;
pub mod request_queue;

use ::device_id::DeviceId;
use component::{ComponentInitError, init_component};
use ostd::sync::Mutex;

/// A registered device and the number of long-term users holding it.
#[derive(Debug)]
struct RegisteredBlockDevice {
    device: Arc<dyn BlockDevice>,
    lease_count: Mutex<usize>,
}

/// A lease that prevents a registered block device from being unregistered while
/// it is in long-term use.
#[derive(Debug)]
pub struct BlockDeviceLease {
    device: Arc<dyn BlockDevice>,
    registered: Option<Arc<RegisteredBlockDevice>>,
}

impl BlockDeviceLease {
    /// Returns the block device protected by this lease.
    pub fn device(&self) -> &Arc<dyn BlockDevice> {
        &self.device
    }

    /// Creates a lease for a pre-resolved device that does not participate in
    /// registry lifetime tracking.
    pub fn new_untracked(device: Arc<dyn BlockDevice>) -> Self {
        Self {
            device,
            registered: None,
        }
    }
}

impl core::ops::Deref for BlockDeviceLease {
    type Target = dyn BlockDevice;

    fn deref(&self) -> &Self::Target {
        self.device.as_ref()
    }
}

impl Clone for BlockDeviceLease {
    fn clone(&self) -> Self {
        if let Some(registered) = &self.registered {
            let mut count = registered.lease_count.lock();
            debug_assert!(*count > 0);
            *count = count
                .checked_add(1)
                .expect("block device lease count overflow");
        }

        Self {
            device: self.device.clone(),
            registered: self.registered.clone(),
        }
    }
}

impl Drop for BlockDeviceLease {
    fn drop(&mut self) {
        let Some(registered) = &self.registered else {
            return;
        };
        let mut count = registered.lease_count.lock();
        debug_assert!(*count > 0);
        *count -= 1;
    }
}

use self::{
    bio::{BioEnqueueError, SubmittedBio},
    prelude::*,
};
pub use self::{
    device_id::{
        MAX_MAJOR, MajorIdOwner, acquire_major, acquire_major_with_name, allocate_major,
        allocate_major_with_name, major_devices,
    },
    partition::PartitionManager,
};

pub const BLOCK_SIZE: usize = ostd::mm::PAGE_SIZE;
pub const SECTOR_SIZE: usize = 512;

/// The number of minor device numbers allocated for each whole-disk device,
/// including the whole disk and its partitions.
///
/// If a disk has more than 16 partitions, we will allocate device IDs for
/// remaining partitions via `EXTENDED_DEVICE_ID_ALLOCATOR`.
pub const DEVICE_MINORS: u32 = 16;

pub trait BlockDevice: Send + Sync + Any + Debug {
    /// Enqueues a new `SubmittedBio` to the block device.
    fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError>;

    /// Returns the metadata of the block device.
    fn metadata(&self) -> BlockDeviceMeta;

    /// Returns the name of the block device.
    fn name(&self) -> &str;

    /// Returns the device ID of the block device.
    fn id(&self) -> DeviceId;

    /// Returns whether the block device is a partition.
    fn is_partition(&self) -> bool {
        false
    }

    /// Returns the partition manager of the block device.
    ///
    /// Whole-disk devices return their manager, which owns the device's
    /// partitions. Partition devices and devices without partition support
    /// return `None`.
    fn partition_manager(&self) -> Option<&PartitionManager> {
        None
    }
}

/// Metadata for a block device.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlockDeviceMeta {
    /// The upper limit for the number of segments per bio.
    pub max_nr_segments_per_bio: usize,
    /// The total number of sectors of the block device.
    pub nr_sectors: usize,
    // Additional useful metadata can be added here in the future.
}

impl dyn BlockDevice {
    pub fn downcast_ref<T: BlockDevice>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref::<T>()
    }
}

/// The error type which is returned from the APIs of this crate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// Device registered
    Registered,
    /// Device not found
    NotFound,
    /// Invalid arguments
    InvalidArgs,
    /// ID acquired
    IdAcquired,
    /// ID exhausted
    IdExhausted,
    /// The device is held by a lease.
    Busy,
}

/// Registers a new block device.
pub fn register(device: Arc<dyn BlockDevice>) -> Result<(), Error> {
    // Callbacks may reenter the registry, so resolve the ID before locking it.
    let id = device.id();
    let mut registry = DEVICE_REGISTRY.lock();
    if registry.contains_key(&id.to_raw()) {
        return Err(Error::Registered);
    }
    registry.insert(
        id.to_raw(),
        Arc::new(RegisteredBlockDevice {
            device,
            lease_count: Mutex::new(0),
        }),
    );

    Ok(())
}

/// Unregisters an existing block device, returning the device if found.
pub fn unregister(id: DeviceId) -> Result<Arc<dyn BlockDevice>, Error> {
    let mut registry = DEVICE_REGISTRY.lock();
    let registered = registry.get(&id.to_raw()).ok_or(Error::NotFound)?;
    if *registered.lease_count.lock() != 0 {
        return Err(Error::Busy);
    }
    Ok(registry.remove(&id.to_raw()).unwrap().device.clone())
}

/// Collects all block devices.
pub fn collect_all() -> Vec<Arc<dyn BlockDevice>> {
    DEVICE_REGISTRY
        .lock()
        .values()
        .map(|registered| registered.device.clone())
        .collect()
}

/// Looks up a block device of a given device ID.
pub fn lookup(id: DeviceId) -> Option<Arc<dyn BlockDevice>> {
    DEVICE_REGISTRY
        .lock()
        .get(&id.to_raw())
        .map(|registered| registered.device.clone())
}

/// Gets a lease for a published block device.
///
/// Long-term users such as mounted filesystems must hold the lease for the whole
/// period in which they can issue I/O.
pub fn lookup_lease(id: DeviceId) -> Option<BlockDeviceLease> {
    let registry = DEVICE_REGISTRY.lock();
    let registered = registry.get(&id.to_raw())?;
    let mut count = registered.lease_count.lock();
    *count = count.checked_add(1)?;
    drop(count);

    Some(BlockDeviceLease {
        device: registered.device.clone(),
        registered: Some(registered.clone()),
    })
}

/// Looks up a block device by its kernel device name.
pub fn lookup_by_name(name: &str) -> Option<Arc<dyn BlockDevice>> {
    DEVICE_REGISTRY
        .lock()
        .values()
        .find(|registered| registered.device.name() == name)
        .map(|registered| registered.device.clone())
}

/// Scans registered whole-disk devices and updates their partitions.
pub fn scan_partitions() {
    let devices = collect_all();
    for device in devices {
        if device.is_partition() {
            continue;
        }

        let Some(partition_manager) = device.partition_manager() else {
            continue;
        };

        let Some(partition_infos) = partition::parse(&device) else {
            continue;
        };

        partition_manager.update(&device, partition_infos);
    }
}

static DEVICE_REGISTRY: Mutex<BTreeMap<u32, Arc<RegisteredBlockDevice>>> =
    Mutex::new(BTreeMap::new());

#[cfg(ktest)]
mod tests {
    use ::device_id::{MajorId, MinorId};
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
            BlockDeviceMeta::default()
        }

        fn name(&self) -> &str {
            "block-registry-test"
        }

        fn id(&self) -> DeviceId {
            self.id
        }
    }

    #[derive(Debug)]
    struct ReentrantIdBlockDevice {
        id: DeviceId,
        id_calls: AtomicUsize,
    }

    impl ReentrantIdBlockDevice {
        fn new(minor: u32) -> Arc<Self> {
            Arc::new(Self {
                id: DeviceId::new(MajorId::new(510), MinorId::new(minor)),
                id_calls: AtomicUsize::new(0),
            })
        }
    }

    impl BlockDevice for ReentrantIdBlockDevice {
        fn enqueue(&self, _bio: SubmittedBio) -> Result<(), BioEnqueueError> {
            unreachable!()
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta::default()
        }

        fn name(&self) -> &str {
            "block-registry-reentrant-id-test"
        }

        fn id(&self) -> DeviceId {
            self.id_calls.fetch_add(1, Ordering::Relaxed);
            let _ = lookup(self.id);
            self.id
        }
    }

    #[ktest]
    fn caches_device_id_before_locking_registry() {
        let device = ReentrantIdBlockDevice::new(6);
        let id = device.id;
        register(device.clone()).unwrap();
        assert_eq!(device.id_calls.load(Ordering::Relaxed), 1);
        unregister(id).unwrap();
        assert_eq!(device.id_calls.load(Ordering::Relaxed), 1);
    }

    #[ktest]
    fn lease_blocks_unregistration_until_last_holder_drops() {
        let device = TestBlockDevice::new(1);
        let id = device.id();
        register(device).unwrap();

        let lease = lookup_lease(id).unwrap();
        let cloned = lease.clone();
        assert_eq!(unregister(id).unwrap_err(), Error::Busy);
        assert!(lookup(id).is_some());

        drop(lease);
        assert_eq!(unregister(id).unwrap_err(), Error::Busy);
        drop(cloned);
        assert!(unregister(id).is_ok());
        assert!(lookup_lease(id).is_none());
    }
}

#[init_component]
fn init() -> Result<(), ComponentInitError> {
    device_id::init();

    Ok(())
}
