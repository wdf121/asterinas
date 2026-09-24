// SPDX-License-Identifier: MPL-2.0

use alloc::format;

use aster_block::{BLOCK_SIZE, BlockDevice, SECTOR_SIZE, bio::BioStatus, id::Sid};
use aster_nvme::NvmeBlockDevice;
use aster_virtio::device::block::device::BlockDevice as VirtIoBlockDevice;
use device_id::DeviceId;
use ostd::mm::VmIo;

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
                    remove_wrapper_if_matches(&block_file);
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

    /// Returns the device size in bytes (modern 64-bit interface).
    pub(super) type BlkGetSize64 = ioc!(BLKGETSIZE64, 0x12, 114, OutData<u64>);

    /// Returns the device size in 512-byte sectors (legacy interface).
    /// Linux: _IO(0x12, 96).
    pub(super) type BlkGetSize = ioc!(BLKGETSIZE, 0x1260, OutData<u64>);

    /// Returns the readahead value in sectors.
    /// Linux: _IO(0x12, 99).
    pub(super) type BlkRaGet = ioc!(BLKRAGET, 0x1263, OutData<u64>);

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
    pub(super) type BlkGetSectorSize = ioc!(BLKSSZGET, 0x12, 104, OutData<i32>);

    /// Returns the logical sector size of the block device.
    ///
    /// Linux: _IO(0x12, 104). Some user programs, including util-linux
    /// `blkdiscard`, use this legacy command encoding instead of the typed
    /// `_IOR(0x12, 104, int)` form above.
    pub(super) type BlkGetSectorSizeLegacy = ioc!(BLKSSZGET_LEGACY, 0x1268, NoData);

    /// Discards a byte range described by two u64 values: start and length.
    /// Linux: _IO(0x12, 119).
    pub(super) type BlkDiscard = ioc!(BLKDISCARD, 0x1277, NoData);

    /// Writes zeroes to a byte range described by two u64 values: start and length.
    /// Linux: _IO(0x12, 127).
    pub(super) type BlkZeroout = ioc!(BLKZEROOUT, 0x127F, NoData);
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
    mapper_alias: Mutex<Option<(String, DevtmpfsHandle)>>,
    removing: Mutex<Option<aster_block::PendingBlockDeviceUnregistration>>,
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
            mapper_alias: Mutex::new(None),
            removing: Mutex::new(None),
        }
    }

    fn set_node(&self, node: DevtmpfsHandle) {
        let mut owned_node = self.node.lock();
        assert!(owned_node.is_none());
        *owned_node = Some(node);
    }

    fn delete_node<F>(&self, delete: F) -> Result<()>
    where
        F: FnOnce(&DevtmpfsHandle) -> Result<()>,
    {
        let mut node = self.node.lock();
        if let Some(handle) = node.as_ref() {
            delete(handle)?;
            node.take();
        }
        Ok(())
    }

    fn delete_mapper_alias<F>(&self, delete: F) -> Result<()>
    where
        F: FnOnce(&DevtmpfsHandle) -> Result<()>,
    {
        let mut alias = self.mapper_alias.lock();
        if let Some((_, handle)) = alias.as_ref() {
            delete(handle)?;
            alias.take();
        }
        Ok(())
    }

    fn has_node(&self) -> bool {
        self.node.lock().is_some()
    }

    fn validate_node(&self) -> Result<()> {
        let node = self.node.lock();
        devtmpfs::validate(node.as_ref().ok_or_else(|| {
            Error::with_message(Errno::ESTALE, "the mapper primary node is missing")
        })?)
    }

    fn take_removing(&self) -> Option<aster_block::PendingBlockDeviceUnregistration> {
        self.removing.lock().take()
    }

    fn retain_removing(&self, unregistration: aster_block::PendingBlockDeviceUnregistration) {
        let mut removing = self.removing.lock();
        assert!(removing.is_none());
        *removing = Some(unregistration.retain_removing());
    }

    fn set_mapper_alias(&self, path: String, alias: DevtmpfsHandle) {
        let mut owned_alias = self.mapper_alias.lock();
        assert!(owned_alias.is_none());
        *owned_alias = Some((path, alias));
    }

    fn has_mapper_alias(&self) -> bool {
        self.mapper_alias.lock().is_some()
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

fn write_sector_size_ioctl(device_range_addr: usize) -> Result<()> {
    let sector_size = SECTOR_SIZE.max(BLOCK_SIZE) as i32;
    current_userspace!().write_val(device_range_addr, &sector_size)?;
    Ok(())
}

fn read_range_ioctl_arg(device_range_addr: usize, device: &dyn BlockDevice) -> Result<(Sid, u64)> {
    let [start, len] = current_userspace!().read_val::<[u64; 2]>(device_range_addr)?;
    let sector_size = SECTOR_SIZE as u64;
    if start % sector_size != 0 || len % sector_size != 0 {
        return_errno_with_message!(Errno::EINVAL, "the block range is not sector-aligned");
    }

    let end = start
        .checked_add(len)
        .ok_or_else(|| Error::with_message(Errno::EINVAL, "the block range overflows"))?;
    let device_size = (device.metadata().nr_sectors as u64)
        .checked_mul(sector_size)
        .ok_or_else(|| Error::with_message(Errno::EINVAL, "the block device size overflows"))?;
    if end > device_size {
        return_errno_with_message!(Errno::EINVAL, "the block range is beyond the device");
    }

    Ok((Sid::new(start / sector_size), len / sector_size))
}

fn complete_range_ioctl(status: BioStatus) -> Result<()> {
    match status {
        BioStatus::Complete => Ok(()),
        BioStatus::NotSupported | BioStatus::NoSpace | BioStatus::IoError => Err(status.into()),
        _ => return_errno_with_message!(Errno::EIO, "the range I/O did not complete"),
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
            cmd @ BlkGetSectorSize => {
                // TODO: Query the per-device logical block size once block device metadata
                // exposes it. For now, report the effective minimum I/O granularity enforced
                // by Asterinas filesystems so userspace can use `BLKSSZGET` for `O_DIRECT`
                // alignment.
                let sector_size = SECTOR_SIZE.max(BLOCK_SIZE) as i32;
                cmd.write(&sector_size)?;
                Ok(0)
            }
            BlkGetSectorSizeLegacy => {
                write_sector_size_ioctl(raw_ioctl.arg())?;
                Ok(0)
            }
            cmd @ BlkGetSize64 => {
                let size = (self.device.metadata().nr_sectors * SECTOR_SIZE) as u64;
                cmd.write(&size)?;
                Ok(0)
            }
            cmd @ BlkGetSize => {
                let sectors = self.device.metadata().nr_sectors as u64;
                cmd.write(&sectors)?;
                Ok(0)
            }
            cmd @ BlkRaGet => {
                let readahead = 0_u64;
                cmd.write(&readahead)?;
                Ok(0)
            }
            BlkDiscard => {
                // Linux passes discard ranges as byte offsets, while the block
                // layer uses 512-byte sectors. Validate the user ABI boundary
                // here so drivers only receive sector-aligned `Bio`s.
                let (start_sid, nsectors) =
                    read_range_ioctl_arg(raw_ioctl.arg(), self.device.as_ref())?;
                if nsectors != 0 {
                    complete_range_ioctl(self.device.discard_sectors(start_sid, nsectors)?)?;
                }
                Ok(0)
            }
            BlkZeroout => {
                // `BLKZEROOUT` shares the same `[start, len]` byte-range ABI as
                // `BLKDISCARD`, but maps to write-zeroes rather than discard.
                let (start_sid, nsectors) =
                    read_range_ioctl_arg(raw_ioctl.arg(), self.device.as_ref())?;
                if nsectors != 0 {
                    complete_range_ioctl(self.device.write_zeroes_sectors(start_sid, nsectors)?)?;
                }
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

pub(super) fn lookup(id: DeviceId) -> Option<Arc<dyn Device>> {
    let block_file = DEVICE_REGISTRY.lock().get(&id.to_raw()).cloned()?;
    Some(block_file)
}

/// Registers the `/dev/dm-N` runtime node without publishing a mapper alias.
///
/// A first `DM_TABLE_LOAD` calls this before its table enters the inactive slot.
/// The resulting block file accepts opens, but a `DmDevice` without an active
/// table reports zero capacity and cannot dispatch mapping I/O.
pub(crate) fn register_mapper_primary(device: Arc<dyn BlockDevice>) -> Result<()> {
    let primary_path = format!("dm-{}", device.id().minor().get());
    register_runtime_primary(device, primary_path)
}

#[cfg(ktest)]
pub(crate) fn register_mapper_primary_with_node_creator<F>(
    device: Arc<dyn BlockDevice>,
    create_node: F,
) -> Result<()>
where
    F: FnOnce(DevtmpfsNode) -> Result<DevtmpfsHandle>,
{
    let primary_path = format!("dm-{}", device.id().minor().get());
    register_runtime_primary_with_node_creator(device, primary_path, create_node)
}

/// Publishes `/dev/mapper/<name>` for a mapper with an existing primary node.
///
/// The primary node must still be owned by this registration. Keeping that
/// check here prevents an alias from being attached to a replaced devtmpfs node.
pub(crate) fn publish_mapper_alias(id: DeviceId, mapper_name: &str) -> Result<()> {
    validate_mapper_name(mapper_name)?;

    let block_file = lookup_runtime_block_file(id)?;
    let _lifecycle = block_file.lifecycle.lock();
    if block_file.has_mapper_alias() {
        return_errno_with_message!(Errno::EEXIST, "the mapper alias already exists");
    }
    block_file.validate_node()?;

    let alias_path = format!("mapper/{mapper_name}");
    let alias_target = format!("../{}", block_file.path);
    let alias = devtmpfs::create_symlink(alias_path.clone(), alias_target)?;
    block_file.set_mapper_alias(alias_path, alias);
    Ok(())
}

/// Reports whether this mapper runtime registration has published its alias.
pub(crate) fn has_mapper_alias(id: DeviceId) -> Result<bool> {
    Ok(lookup_runtime_block_file(id)?.has_mapper_alias())
}

pub(crate) fn rename_mapper(id: DeviceId, old_name: &str, new_name: &str) -> Result<()> {
    validate_mapper_name(old_name)?;
    validate_mapper_name(new_name)?;
    if old_name == new_name {
        return Ok(());
    }

    let block_file = lookup_runtime_block_file(id)?;
    let _lifecycle = block_file.lifecycle.lock();
    let old_path = format!("mapper/{old_name}");
    let new_path = format!("mapper/{new_name}");
    let mut alias = block_file.mapper_alias.lock();
    let (alias_path, handle) = alias.take().ok_or_else(|| {
        Error::with_message(Errno::ESTALE, "the mapper alias registration is missing")
    })?;
    if alias_path != old_path {
        *alias = Some((alias_path, handle));
        return_errno_with_message!(Errno::ENODEV, "the mapper name does not match the device");
    }

    let retained = handle.clone();
    match devtmpfs::rename_no_replace(handle, new_path.clone()) {
        Ok(handle) => {
            *alias = Some((new_path, handle));
            Ok(())
        }
        Err(error) => {
            *alias = Some((alias_path, retained));
            Err(error)
        }
    }
}

pub(crate) fn unregister_mapper(id: DeviceId, mapper_name: &str) -> Result<Arc<dyn BlockDevice>> {
    unregister_mapper_with_node_remover(id, mapper_name, remove_owned_node_or_accept_stale)
}

fn unregister_mapper_with_node_remover<F>(
    id: DeviceId,
    mapper_name: &str,
    remove_node: F,
) -> Result<Arc<dyn BlockDevice>>
where
    F: Fn(&DevtmpfsHandle) -> Result<()> + Copy,
{
    validate_mapper_name(mapper_name)?;
    let block_file = lookup_runtime_block_file(id)?;
    let _lifecycle = block_file.lifecycle.lock();
    let expected_alias_path = format!("mapper/{mapper_name}");
    let alias_path = {
        let alias = block_file.mapper_alias.lock();
        if let Some((current_path, _)) = alias.as_ref()
            && current_path != &expected_alias_path
        {
            return_errno_with_message!(Errno::ENODEV, "the mapper name does not match the device");
        }
        alias.as_ref().map(|(path, _)| path.clone())
    };

    let unregistration = begin_runtime_unregistration(&block_file)?;
    if let Err(error) = block_file.delete_mapper_alias(remove_node) {
        abort_runtime_unregistration(unregistration, &block_file);
        return Err(error);
    }

    if let Err(error) = block_file.delete_node(remove_node) {
        return recover_primary_removal_failure(
            unregistration,
            &block_file,
            alias_path,
            error,
            restore_mapper_alias,
        );
    }

    match aster_block::commit_unregister(unregistration) {
        Ok(device) => {
            remove_wrapper_if_matches(&block_file);
            Ok(device)
        }
        Err((unregistration, error)) => {
            let primary_recovery = if !block_file.has_node() {
                restore_runtime_node(&block_file)
            } else {
                Ok(())
            };
            let alias_recovery = if let Some(alias_path) = alias_path {
                if !block_file.has_mapper_alias() {
                    restore_mapper_alias(&block_file, alias_path)
                } else {
                    Ok(())
                }
            } else {
                Ok(())
            };
            if let Err(recovery_error) = primary_recovery.and(alias_recovery) {
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

fn register_runtime_primary(device: Arc<dyn BlockDevice>, path: String) -> Result<()> {
    register_runtime_primary_with_node_creator(device, path, devtmpfs::create_node)
}

fn register_runtime_primary_with_node_creator<F>(
    device: Arc<dyn BlockDevice>,
    path: String,
    create_node: F,
) -> Result<()>
where
    F: FnOnce(DevtmpfsNode) -> Result<DevtmpfsHandle>,
{
    let registration =
        aster_block::register_pending(device.clone()).map_err(map_block_registry_error)?;

    let block_file = match register_pending_wrapper(device, path.clone()) {
        Ok(block_file) => block_file,
        Err(error) => {
            let _ = aster_block::abort_registration(registration);
            return Err(error);
        }
    };

    let node = match create_node(DevtmpfsNode::new(
        DeviceType::Block,
        block_file.id(),
        DevtmpfsNodeMeta::new(path.clone()).unwrap(),
    )) {
        Ok(node) => node,
        Err(error) => {
            remove_wrapper_if_matches(&block_file);
            let _ = aster_block::abort_registration(registration);
            return Err(error);
        }
    };

    if let Err(error) = aster_block::commit_registration(&registration) {
        let _ = remove_owned_node_or_accept_stale(&node);
        remove_wrapper_if_matches(&block_file);
        let _ = aster_block::abort_registration(registration);
        return Err(map_block_registry_error(error));
    }
    block_file.set_node(node);
    block_file.start_accepting_opens();

    Ok(())
}

/// Removes nodes that still belong to this registration; missing or replaced
/// nodes are already externally cleaned up and are left untouched.
fn remove_owned_node_or_accept_stale(handle: &DevtmpfsHandle) -> Result<()> {
    match devtmpfs::delete(handle) {
        Ok(()) => Ok(()),
        Err(error) if matches!(error.error(), Errno::ENOENT | Errno::ESTALE) => Ok(()),
        Err(error) => Err(error),
    }
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

fn recover_primary_removal_failure<F>(
    unregistration: aster_block::PendingBlockDeviceUnregistration,
    block_file: &BlockFile,
    alias_path: Option<String>,
    operation_error: Error,
    restore_alias: F,
) -> Result<Arc<dyn BlockDevice>>
where
    F: FnOnce(&BlockFile, String) -> Result<()>,
{
    if let Some(alias_path) = alias_path
        && let Err(recovery_error) = restore_alias(block_file, alias_path)
    {
        return Err(isolate_runtime_unregistration(
            unregistration,
            block_file,
            &operation_error,
            &recovery_error,
        ));
    }
    abort_runtime_unregistration(unregistration, block_file);
    Err(operation_error)
}

fn isolate_runtime_unregistration(
    unregistration: aster_block::PendingBlockDeviceUnregistration,
    block_file: &BlockFile,
    operation_error: &Error,
    recovery_error: &Error,
) -> Error {
    ostd::error!(
        "failed to recover Device Mapper runtime removal for {}: operation={:?}, recovery={:?}",
        block_file.path,
        operation_error,
        recovery_error
    );
    block_file.retain_removing(unregistration);
    Error::with_message(
        Errno::EIO,
        "the Device Mapper runtime removal could not be recovered",
    )
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

fn restore_mapper_alias(block_file: &BlockFile, alias_path: String) -> Result<()> {
    let alias_target = format!("../{}", block_file.path);
    let restored = devtmpfs::create_symlink(alias_path.clone(), alias_target)?;
    block_file.set_mapper_alias(alias_path, restored);
    Ok(())
}

fn validate_mapper_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > crate::fs::utils::NAME_MAX
        || name.as_bytes().contains(&0)
        || name.contains('/')
        || name == "."
        || name == ".."
    {
        return_errno_with_message!(Errno::EINVAL, "the mapper name is invalid");
    }
    Ok(())
}

fn remove_wrapper_if_matches(block_file: &Arc<BlockFile>) {
    let mut registry = DEVICE_REGISTRY.lock();
    if registry
        .get(&block_file.id().to_raw())
        .is_some_and(|current| Arc::ptr_eq(current, block_file))
    {
        registry.remove(&block_file.id().to_raw());
    }
}

fn register_wrapper(device: Arc<dyn BlockDevice>) -> Result<Arc<BlockFile>> {
    let path = device.name().to_string();
    register_wrapper_with_path(device, path)
}

fn register_pending_wrapper(device: Arc<dyn BlockDevice>, path: String) -> Result<Arc<BlockFile>> {
    let id = device.id().to_raw();
    let block_file = Arc::new(BlockFile::new_pending(device, path));
    let mut registry = DEVICE_REGISTRY.lock();
    if registry.contains_key(&id) {
        return_errno_with_message!(Errno::EEXIST, "the block device wrapper already exists");
    }
    registry.insert(id, block_file.clone());
    Ok(block_file)
}

fn register_wrapper_with_path(
    device: Arc<dyn BlockDevice>,
    path: String,
) -> Result<Arc<BlockFile>> {
    let id = device.id().to_raw();
    let block_file = Arc::new(BlockFile::new(device, path));
    let mut registry = DEVICE_REGISTRY.lock();
    if registry.contains_key(&id) {
        return_errno_with_message!(Errno::EEXIST, "the block device wrapper already exists");
    }
    registry.insert(id, block_file.clone());
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
    struct TestBlockDevice {
        id: DeviceId,
    }

    impl TestBlockDevice {
        fn new(minor: u32) -> Arc<Self> {
            Arc::new(Self {
                id: DeviceId::new(MajorId::new(511), MinorId::new(minor)),
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
            "runtime-block-test"
        }

        fn id(&self) -> DeviceId {
            self.id
        }
    }

    #[ktest]
    fn blocks_new_opens_while_removing_and_tracks_open_handles() {
        let block_file = Arc::new(BlockFile::new(
            TestBlockDevice::new(1),
            "runtime-block-test".to_string(),
        ));
        let opened = block_file.open().unwrap();

        assert_eq!(
            block_file.stop_accepting_opens().unwrap_err().error(),
            Errno::EBUSY
        );
        drop(opened);
        block_file.stop_accepting_opens().unwrap();
        let error = match block_file.open() {
            Ok(_) => panic!("open unexpectedly succeeded"),
            Err(error) => error,
        };
        assert_eq!(error.error(), Errno::ENODEV);
    }

    #[ktest]
    fn pending_wrapper_rejects_opens_until_registration_commits() {
        let block_file = Arc::new(BlockFile::new_pending(
            TestBlockDevice::new(2),
            "runtime-block-pending".to_string(),
        ));

        let error = match block_file.open() {
            Ok(_) => panic!("open unexpectedly succeeded"),
            Err(error) => error,
        };
        assert_eq!(error.error(), Errno::ENODEV);
        block_file.start_accepting_opens();
        assert!(block_file.open().is_ok());
    }

    #[ktest]
    fn isolated_runtime_unregistration_stays_hidden_until_retry_commits() {
        let device = TestBlockDevice::new(3);
        let id = device.id();
        aster_block::register(device).unwrap();
        let block_file = Arc::new(BlockFile::new(
            TestBlockDevice::new(3),
            "runtime-block-removing".to_string(),
        ));

        let unregistration = begin_runtime_unregistration(&block_file).unwrap();
        block_file.retain_removing(unregistration);
        assert!(aster_block::lookup(id).is_none());
        assert!(aster_block::lookup_lease(id).is_none());
        let error = match block_file.open() {
            Ok(_) => panic!("open unexpectedly succeeded"),
            Err(error) => error,
        };
        assert_eq!(error.error(), Errno::ENODEV);

        let unregistration = begin_runtime_unregistration(&block_file).unwrap();
        aster_block::commit_unregister(unregistration).unwrap();
        assert!(aster_block::lookup(id).is_none());
    }

    #[ktest]
    fn failed_primary_removal_and_alias_recovery_keeps_device_isolated() {
        let device = TestBlockDevice::new(4);
        let id = device.id();
        aster_block::register(device).unwrap();
        let block_file = Arc::new(BlockFile::new(
            TestBlockDevice::new(4),
            "runtime-block-recovery-failure".to_string(),
        ));
        let unregistration = begin_runtime_unregistration(&block_file).unwrap();
        let primary_error = Error::with_message(Errno::EIO, "injected primary remove failure");

        assert_eq!(
            recover_primary_removal_failure(
                unregistration,
                &block_file,
                Some("mapper/runtime-block-recovery-failure".to_string()),
                primary_error,
                |_, _| {
                    Err(Error::with_message(
                        Errno::EIO,
                        "injected alias restore failure",
                    ))
                },
            )
            .unwrap_err()
            .error(),
            Errno::EIO
        );
        assert!(aster_block::lookup(id).is_none());
        assert!(aster_block::lookup_lease(id).is_none());
        let error = match block_file.open() {
            Ok(_) => panic!("open unexpectedly succeeded"),
            Err(error) => error,
        };
        assert_eq!(error.error(), Errno::ENODEV);

        let unregistration = begin_runtime_unregistration(&block_file).unwrap();
        aster_block::commit_unregister(unregistration).unwrap();
        assert!(aster_block::lookup(id).is_none());
    }

    #[ktest]
    fn failed_primary_removal_and_alias_recovery_restores_live_device() {
        let device = TestBlockDevice::new(5);
        let id = device.id();
        aster_block::register(device).unwrap();
        let block_file = Arc::new(BlockFile::new(
            TestBlockDevice::new(5),
            "runtime-block-recovery-success".to_string(),
        ));
        let unregistration = begin_runtime_unregistration(&block_file).unwrap();
        let primary_error = Error::with_message(Errno::EIO, "injected primary remove failure");
        let mut alias_restored = false;

        assert_eq!(
            recover_primary_removal_failure(
                unregistration,
                &block_file,
                Some("mapper/runtime-block-recovery-success".to_string()),
                primary_error,
                |_, _| {
                    alias_restored = true;
                    Ok(())
                },
            )
            .unwrap_err()
            .error(),
            Errno::EIO
        );
        assert!(alias_restored);
        assert!(aster_block::lookup(id).is_some());
        assert!(aster_block::lookup_lease(id).is_some());
        let opened = block_file.open().unwrap();
        drop(opened);

        let unregistration = begin_runtime_unregistration(&block_file).unwrap();
        aster_block::commit_unregister(unregistration).unwrap();
    }

    #[ktest]
    fn failed_runtime_primary_node_creation_leaves_no_registry_state() {
        let device = TestBlockDevice::new(3);
        let id = device.id();

        assert_eq!(
            register_mapper_primary_with_node_creator(device.clone(), |_| {
                Err(Error::with_message(
                    Errno::EIO,
                    "injected primary node failure",
                ))
            })
            .unwrap_err()
            .error(),
            Errno::EIO
        );

        assert!(lookup(id).is_none());
        assert_eq!(open_count(id), None);
        assert!(aster_block::lookup(id).is_none());
        let retry = aster_block::register_pending(device).unwrap();
        drop(retry);
    }

    #[ktest]
    fn failed_mapper_alias_removal_keeps_handle_for_retry() {
        let device = TestBlockDevice::new(91_337);
        let id = device.id();
        let mapper_name = "runtime-block-alias-delete-failure";
        devtmpfs::init_for_ktest();
        register_mapper_primary(device).unwrap();
        publish_mapper_alias(id, mapper_name).unwrap();

        let error = unregister_mapper_with_node_remover(id, mapper_name, |_| {
            Err(Error::with_message(
                Errno::EIO,
                "injected mapper alias delete failure",
            ))
        })
        .unwrap_err();
        assert_eq!(error.error(), Errno::EIO);
        assert!(has_mapper_alias(id).unwrap());

        unregister_mapper(id, mapper_name).unwrap();
        assert!(lookup(id).is_none());
    }

    #[ktest]
    fn validates_mapper_names() {
        assert!(validate_mapper_name("test-volume").is_ok());
        assert!(validate_mapper_name("vg-lv").is_ok());
        for name in ["", ".", "..", "vg/lv", "bad\0name"] {
            assert_eq!(
                validate_mapper_name(name).unwrap_err().error(),
                Errno::EINVAL
            );
        }
        let overlong = "x".repeat(crate::fs::utils::NAME_MAX + 1);
        assert_eq!(
            validate_mapper_name(&overlong).unwrap_err().error(),
            Errno::EINVAL
        );
    }
}

pub(crate) fn open_count(id: DeviceId) -> Option<usize> {
    let block_file = DEVICE_REGISTRY.lock().get(&id.to_raw()).cloned()?;
    Some(block_file.state.lock().open_count)
}

static DEVICE_REGISTRY: Mutex<BTreeMap<u32, Arc<BlockFile>>> = Mutex::new(BTreeMap::new());
