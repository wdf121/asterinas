// SPDX-License-Identifier: MPL-2.0

use alloc::borrow::ToOwned;

use aster_block::{BLOCK_SIZE, BlockDevice, SECTOR_SIZE, bio::BioStatus};
use aster_device::{
    class::{self, Class, ClassDevice, ClassHandle},
    common::{Attr, DevNum},
};
use aster_nvme::NvmeBlockDevice;
use aster_virtio::device::block::device::BlockDevice as VirtIoBlockDevice;
use device_id::DeviceId;
use ostd::mm::VmIo;
use spin::Once;

use crate::{
    context::current_userspace,
    device::{Device, DeviceType},
    dispatch_ioctl,
    events::IoEvents,
    fs::{
        devtmpfs::{self, DevtmpfsHandle, DevtmpfsNode, DevtmpfsNodeMeta},
        file::{PerOpenFileOps, SettableStatusFlags, StatusFlags, SyncMode},
        vfs::{inode::FileOps, path::Path},
    },
    prelude::*,
    process::signal::{PollHandle, Pollable},
    thread::kernel_thread::ThreadOptions,
    util::ioctl::RawIoctl,
};

pub(super) fn init_in_first_kthread() {
    for device in aster_block::collect_all() {
        if device.is_partition() {
            continue;
        }

        // Spawn threads for virtio block devices
        if device.downcast_ref::<VirtIoBlockDevice>().is_some() {
            let device_clone = device.clone();
            let task_fn = move || {
                info!("spawn the virtio-block thread");
                let virtio_block_device = device_clone.downcast_ref::<VirtIoBlockDevice>().unwrap();
                loop {
                    virtio_block_device.handle_requests();
                }
            };
            ThreadOptions::new(task_fn).spawn();
        }
        // Spawn threads for NVMe block devices
        else if device.downcast_ref::<NvmeBlockDevice>().is_some() {
            let device_clone = device.clone();
            let task_fn = move || {
                info!("spawn the nvme-block thread");
                let nvme_block_device = device_clone.downcast_ref::<NvmeBlockDevice>().unwrap();
                loop {
                    nvme_block_device.handle_requests();
                }
            };
            ThreadOptions::new(task_fn).spawn();
        }
    }

    // Partition scanning performs synchronous block I/O, so the request
    // handling threads above must be available first.
    aster_block::scan_partitions();
}

pub(super) fn init_in_first_process() -> Result<()> {
    for device in aster_block::collect_all() {
        let block_file = register_wrapper(device)?;
        if let Some(meta) = block_file.devtmpfs_meta() {
            let node = match devtmpfs::create_node(DevtmpfsNode::new(
                block_file.type_(),
                block_file.id(),
                meta,
            )) {
                Ok(node) => node,
                Err(error) => {
                    remove_wrapper_if_matches(&block_file)?;
                    return Err(error);
                }
            };
            block_file.set_node(node);
        }
    }

    Ok(())
}

mod ioctl_defs {
    use crate::{
        ioc,
        util::ioctl::{NoData, OutData},
    };

    // Reference: <https://elixir.bootlin.com/linux/v6.18/source/include/uapi/linux/fs.h>

    /// Returns the device size in bytes.
    pub(super) type BlkGetSize64 = ioc!(BLKGETSIZE64, 0x12, 114, OutData<u64>);

    /// Returns the logical sector size of the block device.
    ///
    /// This is the smallest unit of I/O the device can address and,
    /// importantly, the minimum alignment required for `O_DIRECT` I/O on
    /// files backed by this device. Both buffer address and offset must be a
    /// multiple of this value.
    ///
    /// Benchmarks and filesystem tests (for example, `xfstests`, LTP
    /// `preadv03`/`pwritev03`) rely on this ioctl to size `O_DIRECT` buffers.
    /// If the effective alignment enforced by the filesystem layered on top
    /// is larger than the hardware sector, such as `ext2`'s 4 KiB block, this
    /// ioctl must return that larger value. Otherwise user programs will align
    /// correctly for the device but still hit `EINVAL` at the filesystem.
    pub(super) type BlkGetSectorSize = ioc!(BLKSSZGET, 0x12, 104, NoData);
}

/// Represents a block device inode in the filesystem.
//
// TODO: This type wraps an `Arc<dyn BlockDevice>` in another `Arc` just to implement the `Device`
// trait. It leads to redundant vtable dispatch, reference counting, and heap allocation. We should
// devise a better strategy to eliminate the unnecessary intermediate `Arc`.
#[derive(Debug)]
struct BlockFile {
    id: DeviceId,
    path: String,
    state: Arc<Mutex<BlockFileState>>,
    lifecycle: Mutex<()>,
    node: Mutex<Option<DevtmpfsHandle>>,
    model_device: Mutex<Option<Arc<BlockModelDevice>>>,
    removing: Mutex<Option<aster_block::PendingBlockDeviceUnregistration>>,
    failed_registration: Mutex<Option<aster_block::PendingBlockDeviceRegistration>>,
}

#[derive(Debug)]
struct BlockFileState {
    device: Arc<dyn BlockDevice>,
    accepting_opens: bool,
    open_count: usize,
}

impl BlockFile {
    fn new(device: Arc<dyn BlockDevice>, path: String) -> Self {
        Self::new_with_open_state(device, path, true)
    }

    fn new_pending(device: Arc<dyn BlockDevice>, path: String) -> Self {
        Self::new_with_open_state(device, path, false)
    }

    fn new_with_open_state(
        device: Arc<dyn BlockDevice>,
        path: String,
        accepting_opens: bool,
    ) -> Self {
        Self {
            id: device.id(),
            path,
            state: Arc::new(Mutex::new(BlockFileState {
                device,
                accepting_opens,
                open_count: 0,
            })),
            lifecycle: Mutex::new(()),
            node: Mutex::new(None),
            model_device: Mutex::new(None),
            removing: Mutex::new(None),
            failed_registration: Mutex::new(None),
        }
    }

    fn set_node(&self, node: DevtmpfsHandle) {
        let mut owned_node = self.node.lock();
        assert!(owned_node.is_none());
        *owned_node = Some(node);
    }

    fn delete_node(&self) -> Result<()> {
        let mut node = self.node.lock();
        if let Some(handle) = node.as_ref() {
            remove_owned_node_or_accept_stale(handle)?;
            node.take();
        }
        Ok(())
    }

    fn has_node(&self) -> bool {
        self.node.lock().is_some()
    }

    fn take_removing(&self) -> Option<aster_block::PendingBlockDeviceUnregistration> {
        self.removing.lock().take()
    }

    fn retain_removing(&self, unregistration: aster_block::PendingBlockDeviceUnregistration) {
        let mut removing = self.removing.lock();
        assert!(removing.is_none());
        *removing = Some(unregistration.retain_removing());
    }

    fn try_start_accepting_opens(&self) -> bool {
        let mut state = self.state.lock();
        if state.accepting_opens || state.open_count != 0 {
            return false;
        }
        state.accepting_opens = true;
        true
    }

    fn start_accepting_opens(&self) {
        assert!(self.try_start_accepting_opens());
    }

    fn stop_accepting_opens(&self) -> Result<()> {
        let mut state = self.state.lock();
        if !state.accepting_opens {
            return_errno_with_message!(Errno::EBUSY, "the block device lifecycle is changing");
        }
        if state.open_count != 0 {
            return_errno_with_message!(Errno::EBUSY, "the block device is still open");
        }
        state.accepting_opens = false;
        Ok(())
    }
}

impl Device for BlockFile {
    fn type_(&self) -> DeviceType {
        DeviceType::Block
    }

    fn id(&self) -> DeviceId {
        self.id
    }

    fn devtmpfs_meta(&self) -> Option<DevtmpfsNodeMeta> {
        Some(DevtmpfsNodeMeta::new(self.path.clone()).unwrap())
    }

    fn open(&self) -> Result<Box<dyn PerOpenFileOps>> {
        let mut state = self.state.lock();
        if !state.accepting_opens {
            return_errno_with_message!(Errno::ENODEV, "the block device is being removed");
        }
        state.open_count += 1;
        let device = state.device.clone();
        drop(state);

        Ok(Box::new(OpenBlockFile {
            device,
            state: self.state.clone(),
        }))
    }
}

/// Represents an opened block device file ready for I/O operations.
//
// TODO: This type wraps an `Arc<dyn BlockDevice>` in another `Box` just to implement the
// `PerOpenFileOps` trait. It leads to redundant vtable dispatch and heap allocation. We should
// devise a better strategy to eliminate the unnecessary intermediate `Box`.
struct OpenBlockFile {
    device: Arc<dyn BlockDevice>,
    state: Arc<Mutex<BlockFileState>>,
}

impl Drop for OpenBlockFile {
    fn drop(&mut self) {
        let mut state = self.state.lock();
        debug_assert!(state.open_count > 0);
        state.open_count -= 1;
    }
}

impl FileOps for OpenBlockFile {
    fn read_at(
        &self,
        offset: usize,
        writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        let total = writer.avail();
        if total == 0 {
            return Ok(0);
        }

        let device_size = self.device.metadata().nr_sectors * SECTOR_SIZE;
        if offset >= device_size {
            return Ok(0);
        }

        let read_len = total.min(device_size - offset);
        {
            // `VmIo::read` does not allow short writes,
            // so the writer must be precisely limited here.
            let mut limited_writer = writer.clone_exclusive();
            limited_writer.limit(read_len);
            self.device.read(offset, &mut limited_writer)?;
        }
        writer.skip(read_len);
        Ok(read_len)
    }

    fn write_at(
        &self,
        offset: usize,
        reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        let total = reader.remain();
        if total == 0 {
            return Ok(0);
        }

        let device_size = self.device.metadata().nr_sectors * SECTOR_SIZE;
        if offset >= device_size {
            return_errno_with_message!(
                Errno::ENOSPC,
                "the write offset is beyond the block device"
            );
        }

        let write_len = total.min(device_size - offset);
        {
            // `VmIo::write` does not allow short writes,
            // so the reader must be precisely limited here.
            let mut limited_reader = reader.clone();
            limited_reader.limit(write_len);
            self.device.write(offset, &mut limited_reader)?;
        }
        reader.skip(write_len);
        Ok(write_len)
    }
}

impl Pollable for OpenBlockFile {
    fn poll(&self, mask: IoEvents, _: Option<&mut PollHandle>) -> IoEvents {
        let events = IoEvents::IN | IoEvents::OUT;
        events & mask
    }
}

impl PerOpenFileOps for OpenBlockFile {
    fn check_seekable(&self) -> Result<()> {
        Ok(())
    }

    fn is_offset_aware(&self) -> bool {
        true
    }

    fn seek_end(&self) -> Result<Option<usize>> {
        Ok(Some(self.device.metadata().nr_sectors * SECTOR_SIZE))
    }

    fn ioctl(&self, _path: &Path, raw_ioctl: RawIoctl) -> Result<i32> {
        use ioctl_defs::*;

        dispatch_ioctl!(match raw_ioctl {
            _cmd @ BlkGetSectorSize => {
                // TODO: Query the per-device logical block size once block device metadata
                // exposes it. For now, report the effective minimum I/O granularity enforced
                // by Asterinas filesystems so userspace can use `BLKSSZGET` for `O_DIRECT`
                // alignment.
                let sector_size = SECTOR_SIZE.max(BLOCK_SIZE) as i32;
                current_userspace!().write_val(raw_ioctl.arg(), &sector_size)?;
                Ok(0)
            }
            cmd @ BlkGetSize64 => {
                let size = (self.device.metadata().nr_sectors * SECTOR_SIZE) as u64;
                cmd.write(&size)?;
                Ok(0)
            }
            _ => return_errno_with_message!(
                Errno::ENOTTY,
                "the ioctl command is not supported by block devices"
            ),
        })
    }

    fn settable_status_flags(&self) -> SettableStatusFlags {
        SettableStatusFlags::minimal().with_o_direct()
    }

    fn sync(&self, _mode: SyncMode) -> Result<()> {
        match self.device.sync()? {
            // Linux treats an unsupported block-device flush as success.
            // Reference: <https://github.com/torvalds/linux/blob/v6.16/block/fops.c#L609-L611>
            BioStatus::Complete | BioStatus::NotSupported => Ok(()),
            status @ (BioStatus::NoSpace | BioStatus::IoError) => Err(status.into()),
            BioStatus::Init | BioStatus::Submit | BioStatus::Zeros => {
                return_errno_with_message!(Errno::EIO, "invalid block device flush status")
            }
        }
    }
}

/// Registers a runtime block device with an owned primary node.
///
/// The caller chooses the relative path under `/dev`; no mapper naming policy
/// belongs to this transaction. Sysfs may become visible before the primary
/// node, and the node may become visible before block lookups become Live.
/// Until publication finishes, the wrapper rejects opens. If node creation
/// fails and sysfs cleanup cannot complete, the pending registration and
/// wrapper remain reserved for a later `unregister_runtime_primary` retry.
pub(crate) fn register_runtime_primary(device: Arc<dyn BlockDevice>, path: String) -> Result<()> {
    // Validate before building the sysfs device: its name cannot contain '/'.
    let meta = DevtmpfsNodeMeta::new(path.clone())
        .map_err(|_| Error::with_message(Errno::EINVAL, "invalid block device node path"))?;
    if path.contains('/') || path.contains('\0') {
        return_errno_with_message!(Errno::EINVAL, "invalid block device model name");
    }
    let registration =
        aster_block::register_pending(device.clone()).map_err(map_block_registry_error)?;
    let block_file = match register_wrapper_with_path(device, path, true) {
        Ok(block_file) => block_file,
        Err(error) => {
            // The pending token is exclusive; no public lookup can acquire a lease.
            aster_block::abort_registration(registration)
                .expect("exclusive pending block registration must be abortable");
            return Err(error);
        }
    };
    let node =
        match devtmpfs::create_node(DevtmpfsNode::new(DeviceType::Block, block_file.id(), meta)) {
            Ok(node) => node,
            Err(error) => {
                if let Err(cleanup_error) = remove_wrapper_if_matches(&block_file) {
                    ostd::error!(
                        "failed to clean up block model for {}: {:?}",
                        block_file.path,
                        cleanup_error
                    );
                    *block_file.failed_registration.lock() = Some(registration);
                    return_errno_with_message!(
                        Errno::EIO,
                        "block model cleanup failed; retry removal"
                    );
                }
                aster_block::abort_registration(registration)
                    .expect("exclusive pending block registration must be abortable");
                return Err(error);
            }
        };
    // There is no external operation that can change this exclusive Pending entry.
    // Treat a failed commit as an invariant violation rather than dropping a node
    // handle and pretending that a best-effort rollback was atomic.
    aster_block::commit_registration(&registration)
        .expect("exclusive pending block registration must commit");
    block_file.set_node(node);
    block_file.start_accepting_opens();
    Ok(())
}

fn remove_owned_node_or_accept_stale(handle: &DevtmpfsHandle) -> Result<()> {
    match devtmpfs::delete(handle) {
        Ok(()) => Ok(()),
        Err(error) if matches!(error.error(), Errno::ENOENT | Errno::ESTALE) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Whether a runtime primary wrapper remains reserved after a failed removal.
/// A non-live wrapper must only be retried for cleanup, never used for LOAD.
pub(crate) fn is_runtime_primary_isolated(id: DeviceId) -> bool {
    let Ok(file) = lookup_runtime_block_file(id) else {
        return false;
    };
    file.failed_registration.lock().is_some()
        || file.removing.lock().is_some()
        || (aster_block::lookup(id).is_none() && !file.state.lock().accepting_opens)
}

/// Reports whether a runtime primary node has completed publication.
pub(crate) fn is_runtime_primary_registered(id: DeviceId) -> bool {
    DEVICE_REGISTRY
        .lock()
        .get(&id.to_raw())
        .is_some_and(|file| file.state.lock().accepting_opens)
}

fn lookup_runtime_block_file(id: DeviceId) -> Result<Arc<BlockFile>> {
    DEVICE_REGISTRY
        .lock()
        .get(&id.to_raw())
        .cloned()
        .ok_or_else(|| Error::with_message(Errno::ENOENT, "the block device does not exist"))
}

fn begin_runtime_unregistration(
    block_file: &Arc<BlockFile>,
) -> Result<aster_block::PendingBlockDeviceUnregistration> {
    if let Some(unregistration) = block_file.take_removing() {
        return Ok(unregistration);
    }
    block_file.stop_accepting_opens()?;
    match aster_block::begin_unregister(block_file.id()) {
        Ok(unregistration) => Ok(unregistration),
        Err(error) => {
            let _ = block_file.try_start_accepting_opens();
            Err(map_block_registry_error(error))
        }
    }
}

fn abort_runtime_unregistration(
    unregistration: aster_block::PendingBlockDeviceUnregistration,
    block_file: &BlockFile,
) {
    let _ = aster_block::abort_unregister(unregistration);
    let _ = block_file.try_start_accepting_opens();
}

fn restore_runtime_node(block_file: &BlockFile) -> Result<()> {
    let restored = devtmpfs::create_node(DevtmpfsNode::new(
        DeviceType::Block,
        block_file.id(),
        DevtmpfsNodeMeta::new(block_file.path.clone()).unwrap(),
    ))?;
    block_file.set_node(restored);
    Ok(())
}

fn isolate_runtime_unregistration(
    unregistration: aster_block::PendingBlockDeviceUnregistration,
    block_file: &BlockFile,
    operation_error: &Error,
    recovery_error: &Error,
) -> Error {
    ostd::error!(
        "failed to recover block device removal for {}: operation={:?}, recovery={:?}",
        block_file.path,
        operation_error,
        recovery_error
    );
    block_file.retain_removing(unregistration);
    Error::with_message(Errno::EIO, "block device removal could not be recovered")
}

/// Removes a runtime primary node and unregisters its device.
/// Failed restoration leaves the registration isolated for a later retry.
/// A failed publication with a retained pending reservation can also be
/// cleaned up through this entry point.
pub(crate) fn unregister_runtime_primary(id: DeviceId) -> Result<Arc<dyn BlockDevice>> {
    let block_file = lookup_runtime_block_file(id)?;
    let _lifecycle = block_file.lifecycle.lock();
    // A failed publication has no node and was never made Live. Retry sysfs
    // teardown before releasing its reserved ID; never publish it on retry.
    if block_file.failed_registration.lock().is_some() {
        remove_wrapper_if_matches(&block_file)?;
        let registration = block_file.failed_registration.lock().take().unwrap();
        let device = aster_block::abort_registration(registration)
            .expect("exclusive pending block registration must be abortable");
        return Ok(device);
    }
    // A committed removal may still have an attached sysfs model if teardown
    // failed. Retry it without reopening the already unregistered block device.
    if aster_block::lookup(id).is_none()
        && !block_file.has_node()
        && !block_file.state.lock().accepting_opens
    {
        let device = block_file.state.lock().device.clone();
        remove_wrapper_if_matches(&block_file)?;
        return Ok(device);
    }
    let unregistration = begin_runtime_unregistration(&block_file)?;
    if let Err(error) = block_file.delete_node() {
        abort_runtime_unregistration(unregistration, &block_file);
        return Err(error);
    }
    match aster_block::commit_unregister(unregistration) {
        Ok(device) => {
            remove_wrapper_if_matches(&block_file)?;
            Ok(device)
        }
        Err((unregistration, error)) => {
            let recovery = if block_file.has_node() {
                Ok(())
            } else {
                restore_runtime_node(&block_file)
            };
            if let Err(recovery_error) = recovery {
                return Err(isolate_runtime_unregistration(
                    unregistration,
                    &block_file,
                    &map_block_registry_error(error),
                    &recovery_error,
                ));
            }
            abort_runtime_unregistration(unregistration, &block_file);
            Err(map_block_registry_error(error))
        }
    }
}

/// Returns the current open count of a registered block device, if published.
pub(crate) fn open_count(id: DeviceId) -> Option<usize> {
    let block_file = DEVICE_REGISTRY.lock().get(&id.to_raw()).cloned()?;
    let count = block_file.state.lock().open_count;
    Some(count)
}

pub(super) fn lookup(id: DeviceId) -> Option<Arc<dyn Device>> {
    let block_file = DEVICE_REGISTRY.lock().get(&id.to_raw()).cloned()?;
    Some(block_file)
}

/// The block device class supplies the sysfs view of a registered device.
/// Devtmpfs primary nodes are created by the runtime registration transaction.
struct BlockClass;

impl Class for BlockClass {
    const NAME: &'static str = "block";
    type Device = DeviceId;
    const CREATES_DEVNODE: bool = false;

    fn dev_attrs(&self) -> &'static [Attr<ClassDevice<Self>>] {
        &[]
    }
}

type BlockModelDevice = ClassDevice<BlockClass>;
static BLOCK_CLASS: Once<Arc<ClassHandle<BlockClass>>> = Once::new();

fn register_model_device(block_file: &Arc<BlockFile>) -> Result<()> {
    let handle = BLOCK_CLASS.call_once(|| {
        #[cfg(ktest)]
        aster_device::init_for_ktest();
        class::register(BlockClass).expect("the block class name is valid")
    });
    let device = BlockModelDevice::builder(handle, block_file.path.clone(), block_file.id())
        .devnum(DevNum::block(block_file.id()))
        .build();
    aster_device::add_device(&device)?;
    *block_file.model_device.lock() = Some(device);
    Ok(())
}

/// Removes a wrapper only after its sysfs model has been detached. On failure,
/// both the wrapper and model handle remain available for a retry.
fn remove_wrapper_if_matches(block_file: &Arc<BlockFile>) -> Result<()> {
    let mut model_device = block_file.model_device.lock();
    if let Some(device) = model_device.as_ref() {
        aster_device::remove_device(device)
            .map_err(|_| Error::with_message(Errno::EBUSY, "block device model removal failed"))?;
        model_device.take();
    }
    drop(model_device);
    let mut registry = DEVICE_REGISTRY.lock();
    if registry
        .get(&block_file.id().to_raw())
        .is_some_and(|current| Arc::ptr_eq(current, block_file))
    {
        registry.remove(&block_file.id().to_raw());
    }
    Ok(())
}

fn register_wrapper(device: Arc<dyn BlockDevice>) -> Result<Arc<BlockFile>> {
    let path = device.name().to_owned();
    register_wrapper_with_path(device, path, false)
}

fn register_wrapper_with_path(
    device: Arc<dyn BlockDevice>,
    path: String,
    pending: bool,
) -> Result<Arc<BlockFile>> {
    let id = device.id().to_raw();
    let block_file = Arc::new(if pending {
        BlockFile::new_pending(device, path)
    } else {
        BlockFile::new(device, path)
    });
    let mut registry = DEVICE_REGISTRY.lock();
    if registry.contains_key(&id) {
        return_errno_with_message!(Errno::EEXIST, "the block device wrapper already exists");
    }
    registry.insert(id, block_file.clone());
    drop(registry);
    if let Err(error) = register_model_device(&block_file) {
        remove_wrapper_if_matches(&block_file)?;
        return Err(error);
    }
    Ok(block_file)
}

fn map_block_registry_error(error: aster_block::Error) -> Error {
    match error {
        aster_block::Error::Registered | aster_block::Error::IdAcquired => {
            Error::with_message(Errno::EEXIST, "the block device already exists")
        }
        aster_block::Error::NotFound => {
            Error::with_message(Errno::ENOENT, "the block device does not exist")
        }
        aster_block::Error::InvalidArgs => {
            Error::with_message(Errno::EINVAL, "the block device arguments are invalid")
        }
        aster_block::Error::IdExhausted => {
            Error::with_message(Errno::ENOSPC, "no block device ID is available")
        }
        aster_block::Error::Busy => {
            Error::with_message(Errno::EBUSY, "the block device is still in use")
        }
    }
}

#[cfg(ktest)]
mod tests {
    use aster_block::{
        BlockDeviceMeta,
        bio::{BioEnqueueError, SubmittedBio},
    };
    use device_id::{MajorId, MinorId};
    use ostd::prelude::ktest;

    use super::*;

    #[derive(Debug)]
    struct TestBlockDevice(DeviceId);

    impl TestBlockDevice {
        fn new(minor: u32) -> Arc<Self> {
            Arc::new(Self(DeviceId::new(MajorId::new(511), MinorId::new(minor))))
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
            "runtime-block-test"
        }

        fn id(&self) -> DeviceId {
            self.0
        }
    }

    #[ktest]
    fn pending_wrapper_rejects_opens_until_registration_commits() {
        let block_file = Arc::new(BlockFile::new_pending(
            TestBlockDevice::new(2),
            "runtime-block-pending".to_string(),
        ));
        assert_eq!(block_file.open().err().unwrap().error(), Errno::ENODEV);
        block_file.start_accepting_opens();
        assert!(block_file.open().is_ok());
    }

    #[ktest]
    fn runtime_registration_publishes_and_unpublishes_primary() {
        devtmpfs::init_for_ktest();
        let device = TestBlockDevice::new(42);
        let id = device.id();
        let path = "__runtime_block_ktest_primary";
        register_runtime_primary(device, path.to_string()).unwrap();
        let block_file = lookup_runtime_block_file(id).unwrap();
        assert!(block_file.model_device.lock().is_some());
        assert!(aster_block::lookup(id).is_some());
        assert!(devtmpfs::lookup_path_for_ktest(path).is_ok());
        assert!(block_file.open().is_ok());
        unregister_runtime_primary(id).unwrap();
        assert!(lookup(id).is_none());
        assert!(aster_block::lookup(id).is_none());
        assert!(block_file.model_device.lock().is_none());
        assert!(devtmpfs::lookup_path_for_ktest(path).is_err());
    }

    #[ktest]
    fn invalid_runtime_path_does_not_publish_any_state() {
        let device = TestBlockDevice::new(44);
        let id = device.id();
        assert_eq!(
            register_runtime_primary(device.clone(), "bad/path".to_string())
                .unwrap_err()
                .error(),
            Errno::EINVAL
        );
        assert!(lookup(id).is_none());
        assert!(aster_block::lookup(id).is_none());
        assert_eq!(
            register_runtime_primary(device, "".to_string())
                .unwrap_err()
                .error(),
            Errno::EINVAL
        );
    }

    #[ktest]
    fn existing_node_does_not_leave_model_or_block_registration() {
        devtmpfs::init_for_ktest();
        let path = "__runtime_block_ktest_collision";
        let first = TestBlockDevice::new(45);
        let second = TestBlockDevice::new(46);
        let occupying = devtmpfs::create_node(DevtmpfsNode::new(
            DeviceType::Block,
            first.id(),
            DevtmpfsNodeMeta::new(path).unwrap(),
        ))
        .unwrap();
        assert_eq!(
            register_runtime_primary(second.clone(), path.to_string())
                .unwrap_err()
                .error(),
            Errno::EEXIST
        );
        assert!(lookup(second.id()).is_none());
        assert!(aster_block::lookup(second.id()).is_none());
        assert!(devtmpfs::lookup_path_for_ktest(path).is_ok());
        devtmpfs::delete(&occupying).unwrap();
        // A failed registration may reuse the same device ID after cleanup.
        register_runtime_primary(second.clone(), path.to_string()).unwrap();
        unregister_runtime_primary(second.id()).unwrap();
    }

    #[ktest]
    fn duplicate_model_name_rolls_back_pending_block_id() {
        devtmpfs::init_for_ktest();
        let path = "__runtime_block_ktest_model_conflict";
        let first = TestBlockDevice::new(47);
        let second = TestBlockDevice::new(48);
        register_runtime_primary(first.clone(), path.to_string()).unwrap();
        assert!(register_runtime_primary(second.clone(), path.to_string()).is_err());
        assert!(lookup(second.id()).is_none());
        assert!(aster_block::lookup(second.id()).is_none());
        unregister_runtime_primary(first.id()).unwrap();
        register_runtime_primary(second.clone(), path.to_string()).unwrap();
        unregister_runtime_primary(second.id()).unwrap();
    }

    #[ktest]
    fn open_handle_prevents_runtime_unregistration() {
        devtmpfs::init_for_ktest();
        let device = TestBlockDevice::new(43);
        let id = device.id();
        register_runtime_primary(device, "__runtime_block_ktest_open".to_string()).unwrap();
        let block_file = lookup_runtime_block_file(id).unwrap();
        let opened = block_file.open().unwrap();
        assert_eq!(
            unregister_runtime_primary(id).unwrap_err().error(),
            Errno::EBUSY
        );
        assert!(aster_block::lookup(id).is_some());
        drop(opened);
        unregister_runtime_primary(id).unwrap();
    }
}

static DEVICE_REGISTRY: Mutex<BTreeMap<u32, Arc<BlockFile>>> = Mutex::new(BTreeMap::new());
