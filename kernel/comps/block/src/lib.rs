// SPDX-License-Identifier: MPL-2.0

//! The block devices of Asterinas.
//!
//! This crate provides a number of base components for block devices, including
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
pub use device_id::{
    EXTENDED_DEVICE_ID_ALLOCATOR, MajorIdOwner, acquire_major, acquire_major_with_name,
    allocate_major, allocate_major_with_name, major_devices,
};
use ostd::sync::Mutex;
pub use partition::{PartitionInfo, PartitionNode};

#[derive(Debug)]
struct RegisteredBlockDevice {
    id: DeviceId,
    device: Arc<dyn BlockDevice>,
    state: Mutex<RegisteredBlockDeviceState>,
}

#[derive(Debug)]
struct RegisteredBlockDeviceState {
    status: RegistrationStatus,
    lease_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegistrationStatus {
    Pending,
    Live,
    Removing,
}

/// 阻止一个已注册块设备在长期使用期间被注销的租约。
#[derive(Debug)]
pub struct BlockDeviceLease {
    device: Arc<dyn BlockDevice>,
    registered: Option<Arc<RegisteredBlockDevice>>,
}

impl BlockDeviceLease {
    /// 返回租约保护的底层块设备。
    pub fn device(&self) -> &Arc<dyn BlockDevice> {
        &self.device
    }

    /// 为内核测试创建一个不参与注册表生命周期的租约。
    #[cfg(ktest)]
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
            let mut state = registered.state.lock();
            debug_assert!(state.lease_count > 0);
            state.lease_count = state
                .lease_count
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
        let mut state = registered.state.lock();
        debug_assert!(state.lease_count > 0);
        state.lease_count -= 1;
    }
}

/// 尚未向查找者发布的块设备注册令牌。
#[derive(Debug)]
pub struct PendingBlockDeviceRegistration {
    registered: Arc<RegisteredBlockDevice>,
}

impl PendingBlockDeviceRegistration {
    /// 返回待发布设备的 ID。
    pub fn id(&self) -> DeviceId {
        self.registered.id
    }
}

/// 已停止新查找、等待外部资源清理的块设备注销令牌。
#[derive(Debug)]
pub struct PendingBlockDeviceUnregistration {
    registered: Arc<RegisteredBlockDevice>,
}

impl PendingBlockDeviceUnregistration {
    /// 返回待注销设备的 ID。
    pub fn id(&self) -> DeviceId {
        self.registered.id
    }
}

impl Drop for PendingBlockDeviceRegistration {
    fn drop(&mut self) {
        let mut registry = DEVICE_REGISTRY.lock();
        let id = self.id().to_raw();
        let Some(current) = registry.get(&id) else {
            return;
        };
        if !Arc::ptr_eq(current, &self.registered) {
            return;
        }
        let state = current.state.lock();
        if state.status != RegistrationStatus::Pending || state.lease_count != 0 {
            return;
        }
        drop(state);
        registry.remove(&id);
    }
}

impl Drop for PendingBlockDeviceUnregistration {
    fn drop(&mut self) {
        let registry = DEVICE_REGISTRY.lock();
        let Some(current) = registry.get(&self.id().to_raw()) else {
            return;
        };
        if !Arc::ptr_eq(current, &self.registered) {
            return;
        }
        let mut state = current.state.lock();
        if state.status == RegistrationStatus::Removing && state.lease_count == 0 {
            state.status = RegistrationStatus::Live;
        }
    }
}

use self::{
    bio::{BioEnqueueError, SubmittedBio},
    prelude::*,
};

pub const BLOCK_SIZE: usize = ostd::mm::PAGE_SIZE;
pub const SECTOR_SIZE: usize = 512;

pub trait BlockDevice: Send + Sync + Any + Debug {
    /// Enqueues a new `SubmittedBio` to the block device.
    fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError>;

    /// Returns the metadata of the block device.
    fn metadata(&self) -> BlockDeviceMeta;

    /// Returns the name of the block device.
    fn name(&self) -> String;

    /// Returns the device ID of the block device.
    fn id(&self) -> DeviceId;

    /// Returns whether the block device is a partition.
    fn is_partition(&self) -> bool {
        false
    }

    /// Sets the partitions of the block device.
    fn set_partitions(&self, _infos: Vec<Option<PartitionInfo>>) {}

    /// Returns the partitions of the block device.
    fn partitions(&self) -> Option<Vec<Arc<dyn BlockDevice>>> {
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
    /// Id Acquired
    IdAcquired,
    /// Id Exhausted
    IdExhausted,
    /// 设备仍在使用中，或正在切换生命周期状态。
    Busy,
}

/// 开始注册块设备，但暂不向查询者发布。
pub fn register_pending(
    device: Arc<dyn BlockDevice>,
) -> Result<PendingBlockDeviceRegistration, Error> {
    // 公开 trait 回调必须在注册表锁外执行，避免实现重入注册表时自死锁。
    let id = device.id();
    let registered = Arc::new(RegisteredBlockDevice {
        id,
        device,
        state: Mutex::new(RegisteredBlockDeviceState {
            status: RegistrationStatus::Pending,
            lease_count: 0,
        }),
    });
    let mut registry = DEVICE_REGISTRY.lock();
    if registry.contains_key(&id.to_raw()) {
        return Err(Error::Registered);
    }
    registry.insert(id.to_raw(), registered.clone());

    Ok(PendingBlockDeviceRegistration { registered })
}

/// 发布一个待提交的块设备注册。
pub fn commit_registration(registration: &PendingBlockDeviceRegistration) -> Result<(), Error> {
    let registry = DEVICE_REGISTRY.lock();
    let current = registry
        .get(&registration.id().to_raw())
        .ok_or(Error::NotFound)?;
    if !Arc::ptr_eq(current, &registration.registered) {
        return Err(Error::Registered);
    }

    let mut state = current.state.lock();
    if state.status != RegistrationStatus::Pending {
        return Err(Error::Busy);
    }
    state.status = RegistrationStatus::Live;
    Ok(())
}

/// 取消一个尚未发布的块设备注册。
pub fn abort_registration(
    registration: PendingBlockDeviceRegistration,
) -> Result<Arc<dyn BlockDevice>, Error> {
    let mut registry = DEVICE_REGISTRY.lock();
    let id = registration.id().to_raw();
    let current = registry.get(&id).ok_or(Error::NotFound)?;
    if !Arc::ptr_eq(current, &registration.registered) {
        return Err(Error::Registered);
    }

    let state = current.state.lock();
    if state.status != RegistrationStatus::Pending || state.lease_count != 0 {
        return Err(Error::Busy);
    }
    drop(state);

    let registered = registry.remove(&id).unwrap();
    Ok(registered.device.clone())
}

/// 注册新的块设备。
pub fn register(device: Arc<dyn BlockDevice>) -> Result<(), Error> {
    let registration = register_pending(device)?;
    commit_registration(&registration)
}

/// 开始注销一个已发布的块设备，并阻止新的查询和租约。
pub fn begin_unregister(id: DeviceId) -> Result<PendingBlockDeviceUnregistration, Error> {
    let registry = DEVICE_REGISTRY.lock();
    let registered = registry.get(&id.to_raw()).ok_or(Error::NotFound)?;
    let mut state = registered.state.lock();
    if state.status != RegistrationStatus::Live || state.lease_count != 0 {
        return Err(Error::Busy);
    }
    state.status = RegistrationStatus::Removing;
    drop(state);

    Ok(PendingBlockDeviceUnregistration {
        registered: registered.clone(),
    })
}

/// 提交一个待完成的块设备注销。
pub fn commit_unregister(
    unregistration: PendingBlockDeviceUnregistration,
) -> Result<Arc<dyn BlockDevice>, Error> {
    let mut registry = DEVICE_REGISTRY.lock();
    let id = unregistration.id().to_raw();
    let current = registry.get(&id).ok_or(Error::NotFound)?;
    if !Arc::ptr_eq(current, &unregistration.registered) {
        return Err(Error::Registered);
    }
    let state = current.state.lock();
    if state.status != RegistrationStatus::Removing || state.lease_count != 0 {
        return Err(Error::Busy);
    }
    drop(state);

    let registered = registry.remove(&id).unwrap();
    Ok(registered.device.clone())
}

/// 取消待完成的注销，并重新发布同一个设备。
pub fn abort_unregister(unregistration: PendingBlockDeviceUnregistration) -> Result<(), Error> {
    let registry = DEVICE_REGISTRY.lock();
    let current = registry
        .get(&unregistration.id().to_raw())
        .ok_or(Error::NotFound)?;
    if !Arc::ptr_eq(current, &unregistration.registered) {
        return Err(Error::Registered);
    }
    let mut state = current.state.lock();
    if state.status != RegistrationStatus::Removing || state.lease_count != 0 {
        return Err(Error::Busy);
    }
    state.status = RegistrationStatus::Live;
    Ok(())
}

/// 注销现有块设备，并在成功时返回该设备。
pub fn unregister(id: DeviceId) -> Result<Arc<dyn BlockDevice>, Error> {
    let unregistration = begin_unregister(id)?;
    commit_unregister(unregistration)
}

/// 收集所有已发布的块设备。
pub fn collect_all() -> Vec<Arc<dyn BlockDevice>> {
    DEVICE_REGISTRY
        .lock()
        .values()
        .filter_map(|registered| {
            let state = registered.state.lock();
            (state.status == RegistrationStatus::Live).then(|| registered.device.clone())
        })
        .collect()
}

/// 按设备 ID 查找已发布的块设备。
pub fn lookup(id: DeviceId) -> Option<Arc<dyn BlockDevice>> {
    let registry = DEVICE_REGISTRY.lock();
    let registered = registry.get(&id.to_raw())?;
    let state = registered.state.lock();
    (state.status == RegistrationStatus::Live).then(|| registered.device.clone())
}

/// 获取一个已发布块设备的租约。
///
/// 挂载文件系统等长期使用者必须在整个可发起 I/O 的期间持有该租约。
pub fn lookup_lease(id: DeviceId) -> Option<BlockDeviceLease> {
    let registry = DEVICE_REGISTRY.lock();
    let registered = registry.get(&id.to_raw())?;
    let mut state = registered.state.lock();
    if state.status != RegistrationStatus::Live {
        return None;
    }
    state.lease_count = state.lease_count.checked_add(1)?;
    drop(state);

    Some(BlockDeviceLease {
        device: registered.device.clone(),
        registered: Some(registered.clone()),
    })
}

static DEVICE_REGISTRY: Mutex<BTreeMap<u32, Arc<RegisteredBlockDevice>>> =
    Mutex::new(BTreeMap::new());

/// 返回所有已注册块设备的 major:minor 列表，用于诊断。
pub fn list() -> Vec<(u16, u32)> {
    let registry = DEVICE_REGISTRY.lock();
    registry
        .values()
        .map(|r| {
            let id = r.id;
            (id.major().get(), id.minor().get())
        })
        .collect()
}

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

        fn name(&self) -> String {
            String::from("block-registry-test")
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

        fn name(&self) -> String {
            String::from("block-registry-reentrant-id-test")
        }

        fn id(&self) -> DeviceId {
            self.id_calls.fetch_add(1, Ordering::Relaxed);
            let _ = lookup(self.id);
            self.id
        }
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

    #[ktest]
    fn pending_registration_is_hidden_until_commit() {
        let device = TestBlockDevice::new(2);
        let id = device.id();
        let pending = register_pending(device).unwrap();

        assert!(lookup(id).is_none());
        assert!(lookup_lease(id).is_none());
        commit_registration(&pending).unwrap();
        assert!(lookup(id).is_some());
        assert!(unregister(id).is_ok());
    }

    #[ktest]
    fn pending_unregistration_blocks_new_holders_and_can_abort() {
        let device = TestBlockDevice::new(3);
        let id = device.id();
        register(device).unwrap();

        let pending = begin_unregister(id).unwrap();
        assert!(lookup(id).is_none());
        assert!(lookup_lease(id).is_none());
        abort_unregister(pending).unwrap();
        let lease = lookup_lease(id).unwrap();
        assert_eq!(unregister(id).unwrap_err(), Error::Busy);
        drop(lease);
        assert!(unregister(id).is_ok());
    }

    #[ktest]
    fn dropped_pending_registration_rolls_back_automatically() {
        let device = TestBlockDevice::new(4);
        let id = device.id();
        drop(register_pending(device.clone()).unwrap());

        assert!(lookup(id).is_none());
        assert!(register(device).is_ok());
        assert!(unregister(id).is_ok());
    }

    #[ktest]
    fn caches_device_id_before_locking_registry() {
        let device = ReentrantIdBlockDevice::new(6);
        let id = device.id;
        let pending = register_pending(device.clone()).unwrap();

        assert_eq!(device.id_calls.load(Ordering::Relaxed), 1);
        assert_eq!(pending.id(), id);
        commit_registration(&pending).unwrap();
        assert_eq!(device.id_calls.load(Ordering::Relaxed), 1);

        let pending = begin_unregister(id).unwrap();
        assert_eq!(pending.id(), id);
        commit_unregister(pending).unwrap();
        assert_eq!(device.id_calls.load(Ordering::Relaxed), 1);
    }

    #[ktest]
    fn dropped_tokens_use_cached_device_id() {
        let registration_device = ReentrantIdBlockDevice::new(7);
        drop(register_pending(registration_device.clone()).unwrap());
        assert_eq!(registration_device.id_calls.load(Ordering::Relaxed), 1);

        let unregistration_device = ReentrantIdBlockDevice::new(8);
        let id = unregistration_device.id;
        register(unregistration_device.clone()).unwrap();
        drop(begin_unregister(id).unwrap());
        assert!(lookup(id).is_some());
        assert_eq!(unregistration_device.id_calls.load(Ordering::Relaxed), 1);
        unregister(id).unwrap();
    }

    #[ktest]
    fn dropped_pending_unregistration_restores_live_state() {
        let device = TestBlockDevice::new(5);
        let id = device.id();
        register(device).unwrap();
        drop(begin_unregister(id).unwrap());

        assert!(lookup(id).is_some());
        assert!(unregister(id).is_ok());
    }
}

#[init_component]
fn init() -> Result<(), ComponentInitError> {
    device_id::init();

    Ok(())
}

#[init_component(process)]
fn init_in_first_process() -> Result<(), ComponentInitError> {
    let devices = collect_all();
    for device in devices {
        let Some(partition_info) = partition::parse(&device) else {
            continue;
        };

        device.set_partitions(partition_info);
    }

    Ok(())
}
