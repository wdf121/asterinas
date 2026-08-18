// SPDX-License-Identifier: MPL-2.0

//! Linux Device Mapper 控制设备。
//!
//! 该模块实现 `/dev/mapper/control` 的第一版 ioctl 控制面。所有变长 ABI
//! 数据先复制到有上限的内核缓冲区，再使用显式的小端字段读写进行解析，避免
//! 直接解释未对齐或畸形的用户空间结构。

use alloc::{format, vec};

use aster_block::{BlockDevice, id::Sid, lookup_lease};
use aster_device_mapper::{
    DmDevice, DmError, DmManager, DmTable, TableError, target::linear::LinearTarget,
};
use device_id::{DeviceId, MajorId, MinorId};
use ostd::mm::VmIo;
use spin::Once;

use crate::{
    context::current_userspace,
    device::{
        Device, DeviceType, DevtmpfsInodeMeta, block_open_count, register_block_mapper,
        registry::char, rename_block_mapper, unregister_block_mapper,
    },
    events::IoEvents,
    fs::{
        file::{PerOpenFileOps, StatusFlags},
        vfs::{inode::FileOps, path::Path},
    },
    prelude::*,
    process::signal::{PollHandle, Pollable},
    util::ioctl::RawIoctl,
};

const DM_CONTROL_MINOR: u32 = 236;
const DM_IOCTL_MAGIC: u32 = 0xfd;
// `struct dm_ioctl` 的可变 data 成员在 C 布局中的起始偏移是 305，但
// `sizeof(struct dm_ioctl)` 会因 u64 对齐扩展到 312。第一版只接受完整的
// 对齐 envelope，并将 305..312 的 padding 作为响应 header 的一部分清零。
const DM_IOCTL_FIXED_PREFIX_SIZE: usize = 305;
const DM_IOCTL_HEADER_SIZE: usize = 312;
const DM_IOCTL_MAX_SIZE: usize = 1024 * 1024;
const DM_IOCTL_COMMAND_PREFIX: u32 = 0xc138_fd00;

const DM_VERSION_CMD: u8 = 0;
const DM_REMOVE_ALL_CMD: u8 = 1;
const DM_LIST_DEVICES_CMD: u8 = 2;
const DM_DEV_CREATE_CMD: u8 = 3;
const DM_DEV_REMOVE_CMD: u8 = 4;
const DM_DEV_RENAME_CMD: u8 = 5;
const DM_DEV_SUSPEND_CMD: u8 = 6;
const DM_DEV_STATUS_CMD: u8 = 7;
const DM_TABLE_LOAD_CMD: u8 = 9;
const DM_TABLE_CLEAR_CMD: u8 = 10;
const DM_TABLE_DEPS_CMD: u8 = 11;
const DM_TABLE_STATUS_CMD: u8 = 12;
const DM_LIST_VERSIONS_CMD: u8 = 13;
const DM_GET_TARGET_VERSION_CMD: u8 = 17;

const DM_VERSION: [u32; 3] = [4, 48, 0];

struct TargetVersion {
    name: &'static str,
    version: [u32; 3],
}

const TARGET_VERSIONS: &[TargetVersion] = &[
    TargetVersion {
        name: "linear",
        version: [1, 4, 0],
    },
    // LVM2 在创建 LV 前会检查 striped target 是否可用，即使单盘 VG 最终
    // 使用 linear target（use_linear_target=1，默认开启）。此处仅声明版本
    // 以通过 LVM2 预检；table_load 仍会拒绝 striped target 的实际加载。
    TargetVersion {
        name: "striped",
        version: [1, 6, 0],
    },
];

const DM_EXISTS_FLAG: u32 = 1 << 2;
const DM_SUSPEND_FLAG: u32 = 1 << 1;
const DM_PERSISTENT_DEV_FLAG: u32 = 1 << 3;
const DM_STATUS_TABLE_FLAG: u32 = 1 << 4;
const DM_ACTIVE_PRESENT_FLAG: u32 = 1 << 5;
const DM_INACTIVE_PRESENT_FLAG: u32 = 1 << 6;
const DM_BUFFER_FULL_FLAG: u32 = 1 << 8;
const DM_QUERY_INACTIVE_TABLE_FLAG: u32 = 1 << 12;
const DM_UUID_FLAG: u32 = 1 << 14;

const DM_NAME_LIST_FLAG_HAS_UUID: u32 = 1;
const DM_NAME_LIST_FLAG_DOESNT_HAVE_UUID: u32 = 2;

const OFF_VERSION: usize = 0;
const OFF_DATA_SIZE: usize = 12;
const OFF_DATA_START: usize = 16;
const OFF_TARGET_COUNT: usize = 20;
const OFF_OPEN_COUNT: usize = 24;
const OFF_FLAGS: usize = 28;
const OFF_EVENT_NR: usize = 32;
const OFF_DEV: usize = 40;
const OFF_NAME: usize = 48;
const DM_NAME_LEN: usize = 128;
const OFF_UUID: usize = 176;
const DM_UUID_LEN: usize = 129;

const DM_TARGET_SPEC_SIZE: usize = 40;
const DM_TARGET_TYPE_LEN: usize = 16;

static DM_MANAGER: Once<DmManager> = Once::new();
static DM_CONTROL_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug)]
struct DmControlDevice {
    id: DeviceId,
}

impl DmControlDevice {
    fn new() -> Arc<Self> {
        let major = super::MISC_MAJOR.get().unwrap().get();
        Arc::new(Self {
            id: DeviceId::new(major, MinorId::new(DM_CONTROL_MINOR)),
        })
    }
}

impl Device for DmControlDevice {
    fn type_(&self) -> DeviceType {
        DeviceType::Char
    }

    fn id(&self) -> DeviceId {
        self.id
    }

    fn devtmpfs_meta(&self) -> Option<DevtmpfsInodeMeta<'_>> {
        Some(DevtmpfsInodeMeta::new("mapper/control"))
    }

    fn open(&self) -> Result<Box<dyn PerOpenFileOps>> {
        Ok(Box::new(DmControlFile))
    }
}

struct DmControlFile;

impl Pollable for DmControlFile {
    fn poll(&self, mask: IoEvents, _poller: Option<&mut PollHandle>) -> IoEvents {
        mask & (IoEvents::IN | IoEvents::OUT)
    }
}

impl FileOps for DmControlFile {
    fn read_at(
        &self,
        _offset: usize,
        _writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        return_errno_with_message!(Errno::EINVAL, "Device Mapper 控制设备不支持 read")
    }

    fn write_at(
        &self,
        _offset: usize,
        _reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        return_errno_with_message!(Errno::EINVAL, "Device Mapper 控制设备不支持 write")
    }
}

impl PerOpenFileOps for DmControlFile {
    fn check_seekable(&self) -> Result<()> {
        return_errno_with_message!(Errno::ESPIPE, "Device Mapper 控制设备不支持 seek")
    }

    fn is_offset_aware(&self) -> bool {
        false
    }

    fn ioctl(&self, _path: &Path, raw_ioctl: RawIoctl) -> Result<i32> {
        let raw_cmd = raw_ioctl.cmd();
        let command = decode_command(raw_cmd)?;

        ostd::info!(
            "[dm-ioctl] raw_cmd=0x{:08x} decoded_cmd={} arg={}",
            raw_cmd,
            command,
            raw_ioctl.arg()
        );

        let mut header = vec![0u8; DM_IOCTL_HEADER_SIZE];
        current_userspace!().read_bytes(raw_ioctl.arg(), &mut header)?;

        let data_size = read_u32(&header, OFF_DATA_SIZE)? as usize;
        let data_start = read_u32(&header, OFF_DATA_START)? as usize;

        let name_in = optional_c_string(&header, OFF_NAME, DM_NAME_LEN, "name")?;
        let uuid_in = optional_c_string(&header, OFF_UUID, DM_UUID_LEN, "uuid")?;
        let dev_in = read_u64(&header, OFF_DEV)?;
        let target_count = read_u32(&header, OFF_TARGET_COUNT)?;
        let flags = read_u32(&header, OFF_FLAGS)?;

        ostd::info!(
            "[dm-ioctl] header: data_size={} data_start={} target_count={} flags=0x{:x} dev=0x{:x} name={:?} uuid={:?}",
            data_size,
            data_start,
            target_count,
            flags,
            dev_in,
            name_in,
            uuid_in
        );

        validate_ioctl_buffer_layout(data_size, data_start)?;

        let mut buffer = vec![0u8; data_size];
        current_userspace!().read_bytes(raw_ioctl.arg(), &mut buffer)?;

        let version_result = validate_client_version(&buffer);
        write_version(&mut buffer)?;
        let command_result = version_result.and_then(|_| {
            let _guard = DM_CONTROL_LOCK.lock();
            let result = handle_command(command, &mut buffer);
            ostd::info!(
                "[dm-ioctl] command={} result={:?} registered_devices={:?}",
                command,
                result.as_ref().err().map(|e| e.error()),
                aster_block::list()
            );
            result
        });
        let write_result = current_userspace!().write_bytes(raw_ioctl.arg(), &buffer);
        match command_result {
            Ok(()) => {
                write_result?;
                Ok(0)
            }
            Err(error) => {
                let _ = write_result;
                Err(error)
            }
        }
    }
}

fn validate_ioctl_buffer_layout(data_size: usize, data_start: usize) -> Result<()> {
    if !(DM_IOCTL_HEADER_SIZE..=DM_IOCTL_MAX_SIZE).contains(&data_size) {
        return_errno_with_message!(Errno::EINVAL, "dm_ioctl.data_size 超出允许范围");
    }
    if data_start < DM_IOCTL_HEADER_SIZE || data_start > data_size || data_start % 8 != 0 {
        return_errno_with_message!(Errno::EINVAL, "dm_ioctl.data_start 无效或未按 8 字节对齐");
    }
    Ok(())
}

fn decode_command(raw: u32) -> Result<u8> {
    if raw & !0xff != DM_IOCTL_COMMAND_PREFIX || (raw >> 8) & 0xff != DM_IOCTL_MAGIC {
        return_errno_with_message!(Errno::ENOTTY, "未知的 Device Mapper ioctl 命令");
    }
    let command = raw as u8;
    match command {
        DM_VERSION_CMD
        | DM_REMOVE_ALL_CMD
        | DM_LIST_DEVICES_CMD
        | DM_DEV_CREATE_CMD
        | DM_DEV_REMOVE_CMD
        | DM_DEV_RENAME_CMD
        | DM_DEV_SUSPEND_CMD
        | DM_DEV_STATUS_CMD
        | DM_TABLE_LOAD_CMD
        | DM_TABLE_CLEAR_CMD
        | DM_TABLE_DEPS_CMD
        | DM_TABLE_STATUS_CMD
        | DM_LIST_VERSIONS_CMD
        | DM_GET_TARGET_VERSION_CMD => Ok(command),
        _ => return_errno_with_message!(Errno::ENOTTY, "尚未支持该 Device Mapper ioctl 命令"),
    }
}

fn validate_client_version(buffer: &[u8]) -> Result<()> {
    if read_u32(buffer, OFF_VERSION)? != DM_VERSION[0] {
        return_errno_with_message!(Errno::EINVAL, "Device Mapper ioctl 主版本不兼容");
    }
    Ok(())
}

fn handle_command(command: u8, buffer: &mut [u8]) -> Result<()> {
    match command {
        DM_VERSION_CMD => Ok(()),
        DM_LIST_VERSIONS_CMD => list_versions(buffer),
        DM_GET_TARGET_VERSION_CMD => get_target_version(buffer),
        DM_DEV_CREATE_CMD => create_device(buffer),
        DM_DEV_REMOVE_CMD => remove_device(buffer),
        DM_REMOVE_ALL_CMD => remove_all(buffer),
        DM_DEV_STATUS_CMD => device_status(buffer),
        DM_DEV_RENAME_CMD => device_rename(buffer),
        DM_TABLE_LOAD_CMD => table_load(buffer),
        DM_DEV_SUSPEND_CMD => device_suspend(buffer),
        DM_TABLE_CLEAR_CMD => table_clear(buffer),
        DM_TABLE_DEPS_CMD => table_deps(buffer),
        DM_TABLE_STATUS_CMD => table_status(buffer),
        DM_LIST_DEVICES_CMD => list_devices(buffer),
        _ => unreachable!(),
    }
}

fn create_device(buffer: &mut [u8]) -> Result<()> {
    let name = required_c_string(buffer, OFF_NAME, DM_NAME_LEN, "设备名称")?;
    let uuid = optional_c_string(buffer, OFF_UUID, DM_UUID_LEN, "设备 UUID")?;
    let flags = read_u32(buffer, OFF_FLAGS)?;
    let requested_minor = if flags & DM_PERSISTENT_DEV_FLAG != 0 {
        let dev = DeviceId::from_encoded_u64(read_u64(buffer, OFF_DEV)?)
            .ok_or_else(|| Error::with_message(Errno::EINVAL, "persistent dev 编码无效"))?;
        Some(dev.minor().get())
    } else {
        None
    };

    ostd::info!(
        "[dm] create_device: name={} uuid={:?} requested_minor={:?}",
        name,
        uuid,
        requested_minor
    );

    let manager = manager();
    let device = manager
        .create(name.clone(), uuid, requested_minor)
        .map_err(map_dm_error)?;
    ostd::info!("[dm] create_device: created id={:?}", device.id());
    if let Err(error) = register_block_mapper(device.clone(), &name) {
        ostd::warn!(
            "[dm] create_device: register_block_mapper failed: {:?}",
            error
        );
        let _ = manager.remove(&name);
        return Err(error);
    }
    ostd::info!("[dm] create_device: registered successfully");
    fill_device_header(buffer, &device)
}

fn remove_device(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_device(buffer)?;
    unregister_block_mapper(device.id(), &&device.name())?;
    manager().remove(&&device.name()).map_err(map_dm_error)?;
    clear_device_header(buffer)
}

fn remove_all(buffer: &mut [u8]) -> Result<()> {
    // Linux DM_REMOVE_ALL 是 best-effort：busy 设备保留，其余设备继续删除，
    // 单个设备无法删除不会令整个 ioctl 失败。
    for device in manager().devices() {
        if unregister_block_mapper(device.id(), &&device.name()).is_err() {
            continue;
        }
        let _ = manager().remove(&&device.name());
    }
    clear_device_header(buffer)
}

fn device_status(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_device(buffer)?;
    let flags = read_u32(buffer, OFF_FLAGS)?;
    let target_count = selected_table(&device, flags)
        .map(|table| table.linears().len())
        .unwrap_or(0);
    fill_device_header(buffer, &device)?;
    write_u32(buffer, OFF_TARGET_COUNT, target_count)
}

fn device_rename(buffer: &mut [u8]) -> Result<()> {
    let flags = read_u32(buffer, OFF_FLAGS)?;
    if flags & DM_UUID_FLAG != 0 {
        return_errno_with_message!(Errno::EOPNOTSUPP, "第一版不支持修改 Device Mapper UUID");
    }

    let device = lookup_device(buffer)?;
    let old_name = device.name();
    let data_start = data_start(buffer)?;
    let new_name = c_string_until(buffer, data_start, buffer.len(), "新设备名称")?;

    if new_name == old_name {
        return fill_device_header(buffer, &device);
    }

    // manager 索引先更新，若 alias 移动失败则立即回滚，避免 /dev/dm-N、块注册、
    // open gate 或 backing lease 被注销重建。
    manager()
        .rename(&old_name, &new_name)
        .map_err(map_dm_error)?;
    if let Err(error) = rename_block_mapper(device.id(), &old_name, &new_name) {
        if let Err(rollback_error) = manager().rename(&new_name, &old_name) {
            ostd::error!(
                "failed to roll back Device Mapper rename {} -> {}: {:?}",
                new_name,
                old_name,
                rollback_error
            );
        }
        return Err(error);
    }

    fill_device_header(buffer, &device)
}

fn table_load(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_device(buffer)?;
    table_load_for_device(buffer, &device)
}

fn table_load_for_device(buffer: &mut [u8], device: &DmDevice) -> Result<()> {
    let target_count = read_u32(buffer, OFF_TARGET_COUNT)?;
    ostd::info!("[dm] table_load: target_count={}", target_count);
    ostd::info!("[dm] table_load: device name={} found", device.name());

    if target_count == 0 {
        return_errno_with_message!(Errno::EINVAL, "映射表至少需要一个 target");
    }

    let mut cursor = data_start(buffer)?;
    let mut targets = Vec::new();
    for index in 0..target_count {
        let spec_end = cursor
            .checked_add(DM_TARGET_SPEC_SIZE)
            .ok_or_else(invalid_buffer)?;
        require_range(buffer, cursor, DM_TARGET_SPEC_SIZE)?;

        let logical_start = read_u64(buffer, cursor)?;
        let length = read_u64(buffer, cursor + 8)?;
        let next = read_u32(buffer, cursor + 20)? as usize;
        if index + 1 < target_count && next == 0 {
            return_errno_with_message!(Errno::EINVAL, "非最后一个 target 的 next 不能为 0");
        }
        let next_spec = validate_target_spec_next(cursor, next, buffer.len())?;
        let target_type =
            required_c_string(buffer, cursor + 24, DM_TARGET_TYPE_LEN, "target 类型")?;
        if target_type != "linear" {
            return_errno_with_message!(Errno::EINVAL, "当前仅支持 linear target");
        }
        let params = c_string_until(buffer, spec_end, next_spec, "linear 参数")?;
        let (backing_id, backing_start) = parse_linear_params(&params)?;
        ostd::info!(
            "[dm] table_load: name={}, backing={}:{}, start_sector={}, logical_start={}, length={}",
            device.name(),
            backing_id.major().get(),
            backing_id.minor().get(),
            backing_start,
            logical_start,
            length
        );
        let backing = lookup_lease(backing_id).ok_or_else(|| {
            ostd::warn!(
                "[dm] lookup_lease failed for {}:{}, registered devices: {:?}",
                backing_id.major().get(),
                backing_id.minor().get(),
                aster_block::list()
            );
            Error::with_message(Errno::ENODEV, "linear backing 设备不存在或正在移除")
        })?;
        targets.push(
            LinearTarget::new(
                Sid::new(logical_start),
                length,
                Sid::new(backing_start),
                backing,
            )
            .map_err(map_table_error)?,
        );
        cursor = next_spec;
    }

    let table = Arc::new(DmTable::new_linear(targets).map_err(map_table_error)?);
    device.load_table(table);
    ostd::info!("[dm] table_load: table loaded for {}", device.name());
    fill_device_header(buffer, &device)
}

fn device_suspend(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_device(buffer)?;
    let flags = read_u32(buffer, OFF_FLAGS)?;
    if flags & DM_SUSPEND_FLAG != 0 {
        device.suspend().map_err(map_dm_error)?;
    } else {
        device.resume().map_err(map_dm_error)?;
    }
    fill_device_header(buffer, &device)
}

fn table_clear(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_device(buffer)?;
    device.clear_inactive_table().map_err(map_dm_error)?;
    fill_device_header(buffer, &device)
}

fn table_deps(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_device(buffer)?;
    table_deps_for_device(buffer, &device)
}

fn table_deps_for_device(buffer: &mut [u8], device: &DmDevice) -> Result<()> {
    let flags = read_u32(buffer, OFF_FLAGS)?;
    let table = selected_table(device, flags);
    fill_device_header(buffer, device)?;

    let start = data_start(buffer)?;
    let backing_ids = table.map(|table| table.backing_ids()).unwrap_or_default();
    let deps_len = 8usize
        .checked_add(
            backing_ids
                .len()
                .checked_mul(8)
                .ok_or_else(invalid_buffer)?,
        )
        .ok_or_else(invalid_buffer)?;
    if available_from(buffer, start) < deps_len {
        set_buffer_full(buffer)?;
        return Ok(());
    }
    write_u32(buffer, start, backing_ids.len() as u32)?;
    write_u32(buffer, start + 4, 0)?;
    for (index, id) in backing_ids.into_iter().enumerate() {
        write_u64(buffer, start + 8 + index * 8, id.as_encoded_u64())?;
    }
    Ok(())
}

fn table_status(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_device(buffer)?;
    table_status_for_device(buffer, &device)
}

fn table_status_for_device(buffer: &mut [u8], device: &DmDevice) -> Result<()> {
    let flags = read_u32(buffer, OFF_FLAGS)?;
    let table = selected_table(device, flags);
    fill_device_header(buffer, device)?;

    let Some(table) = table else {
        return Ok(());
    };
    write_u32(buffer, OFF_TARGET_COUNT, table.linears().len() as u32)?;

    let start = data_start(buffer)?;
    let mut cursor = start;
    for target in table.linears() {
        let params = if flags & DM_STATUS_TABLE_FLAG != 0 {
            let backing = target.backing_id();
            format!(
                "{}:{} {}",
                backing.major().get(),
                backing.minor().get(),
                target.backing_start().to_raw()
            )
        } else {
            String::new()
        };
        let record_len = table_status_record_len(params.len())?;
        if available_from(buffer, cursor) < record_len {
            set_buffer_full(buffer)?;
            return Ok(());
        }
        write_u64(buffer, cursor, target.logical_range().start.to_raw())?;
        write_u64(buffer, cursor + 8, target.length())?;
        write_u32(buffer, cursor + 16, 0)?;
        write_u32(buffer, cursor + 20, cursor + record_len - start)?;
        write_c_string_fixed(buffer, cursor + 24, DM_TARGET_TYPE_LEN, "linear")?;
        write_c_string(buffer, cursor + DM_TARGET_SPEC_SIZE, &params)?;
        cursor += record_len;
    }
    Ok(())
}

fn target_version_record_len(target: &TargetVersion) -> Result<usize> {
    align_up(16 + target.name.len() + 1, 8)
}

fn write_target_version(
    buffer: &mut [u8],
    offset: usize,
    next: usize,
    target: &TargetVersion,
) -> Result<()> {
    let record_len = target_version_record_len(target)?;
    require_range(buffer, offset, record_len)?;
    write_u32(buffer, offset, next)?;
    for (index, value) in target.version.into_iter().enumerate() {
        write_u32(buffer, offset + 4 + index * 4, value)?;
    }
    write_c_string(buffer, offset + 16, target.name)
}

fn list_versions(buffer: &mut [u8]) -> Result<()> {
    let start = data_start(buffer)?;
    let mut cursor = start;
    let mut previous = None;

    for target in TARGET_VERSIONS {
        let record_len = target_version_record_len(target)?;
        if available_from(buffer, cursor) < record_len {
            set_buffer_full(buffer)?;
            break;
        }
        if let Some(previous) = previous {
            write_u32(buffer, previous, cursor - previous)?;
        }
        write_target_version(buffer, cursor, 0, target)?;
        previous = Some(cursor);
        cursor += record_len;
    }
    Ok(())
}

fn get_target_version(buffer: &mut [u8]) -> Result<()> {
    let name = required_c_string(buffer, OFF_NAME, DM_NAME_LEN, "target 名称")?;
    let target = TARGET_VERSIONS
        .iter()
        .find(|target| target.name == name)
        .ok_or_else(|| Error::with_message(Errno::EINVAL, "未知的 Device Mapper target"))?;
    let start = data_start(buffer)?;
    let record_len = target_version_record_len(target)?;
    if available_from(buffer, start) < record_len {
        set_buffer_full(buffer)?;
        return Ok(());
    }
    write_target_version(buffer, start, 0, target)
}

fn list_devices(buffer: &mut [u8]) -> Result<()> {
    let devices = manager().devices();
    let start = data_start(buffer)?;
    let mut cursor = start;
    let mut previous = None;

    for device in devices {
        let name_len = device.name().len() + 1;
        let uuid_len = device.uuid().map_or(0, |uuid| uuid.len() + 1);
        let (extension_offset, record_len) = name_list_record_layout(name_len, uuid_len)?;
        if available_from(buffer, cursor) < record_len {
            set_buffer_full(buffer)?;
            break;
        }
        if let Some(previous) = previous {
            write_u32(buffer, previous + 8, cursor - previous)?;
        }
        write_u64(buffer, cursor, device.id().as_encoded_u64())?;
        write_u32(buffer, cursor + 8, 0)?;
        write_c_string(buffer, cursor + 12, &&device.name())?;
        let extension = cursor + extension_offset;
        write_u32(buffer, extension, device.status().event_nr)?;
        let name_list_flags = if device.uuid().is_some() {
            DM_NAME_LIST_FLAG_HAS_UUID
        } else {
            DM_NAME_LIST_FLAG_DOESNT_HAVE_UUID
        };
        write_u32(buffer, extension + 4, name_list_flags)?;
        if let Some(uuid) = device.uuid() {
            write_c_string(buffer, extension + 8, uuid)?;
        }
        previous = Some(cursor);
        cursor += record_len;
    }
    Ok(())
}

fn lookup_device(buffer: &[u8]) -> Result<Arc<DmDevice>> {
    let raw_dev = read_u64(buffer, OFF_DEV)?;
    let name = optional_c_string(buffer, OFF_NAME, DM_NAME_LEN, "设备名称")?;
    let uuid = optional_c_string(buffer, OFF_UUID, DM_UUID_LEN, "设备 UUID")?;

    // Linux ABI 规定 UUID selector 优先于 name；dev 仅在两者均未指定时使用。
    // 因此不能把多个 selector 的并存视为畸形输入，否则会拒绝标准客户端的
    // 冗余字段。每次仅按最高优先级的 selector 查找。
    let device = if let Some(uuid) = uuid {
        manager().lookup_uuid(&uuid)
    } else if let Some(name) = name {
        manager().lookup_name(&name)
    } else if raw_dev != 0 {
        let id = DeviceId::from_encoded_u64(raw_dev)
            .ok_or_else(|| Error::with_message(Errno::EINVAL, "Device Mapper dev 编码无效"))?;
        manager().lookup_id(id)
    } else {
        return_errno_with_message!(Errno::EINVAL, "必须指定 Device Mapper 设备");
    };

    device.ok_or_else(|| Error::with_message(Errno::ENXIO, "Device Mapper 设备不存在"))
}

fn selected_table(device: &DmDevice, flags: u32) -> Option<Arc<DmTable>> {
    if flags & DM_QUERY_INACTIVE_TABLE_FLAG != 0 {
        device.inactive_table()
    } else {
        device.active_table()
    }
}

fn fill_device_header(buffer: &mut [u8], device: &DmDevice) -> Result<()> {
    let status = device.status();
    let mut flags = DM_EXISTS_FLAG;
    if status.suspended {
        flags |= DM_SUSPEND_FLAG;
    }
    if status.has_active_table {
        flags |= DM_ACTIVE_PRESENT_FLAG;
    }
    if status.has_inactive_table {
        flags |= DM_INACTIVE_PRESENT_FLAG;
    }
    write_u32(buffer, OFF_TARGET_COUNT, 0)?;
    write_u32(buffer, OFF_FLAGS, flags)?;
    write_u32(buffer, OFF_EVENT_NR, status.event_nr)?;
    write_u64(buffer, OFF_DEV, device.id().as_encoded_u64())?;
    write_u32(
        buffer,
        OFF_OPEN_COUNT,
        block_open_count(device.id()).unwrap_or(0),
    )?;
    write_c_string_fixed(buffer, OFF_NAME, DM_NAME_LEN, &&device.name())?;
    write_c_string_fixed(buffer, OFF_UUID, DM_UUID_LEN, device.uuid().unwrap_or(""))?;
    buffer[DM_IOCTL_FIXED_PREFIX_SIZE..DM_IOCTL_HEADER_SIZE].fill(0);
    Ok(())
}

fn clear_device_header(buffer: &mut [u8]) -> Result<()> {
    write_u32(buffer, OFF_TARGET_COUNT, 0)?;
    write_u32(buffer, OFF_OPEN_COUNT, 0)?;
    write_u32(buffer, OFF_EVENT_NR, 0)?;
    write_u64(buffer, OFF_DEV, 0)
}

fn parse_linear_params(params: &str) -> Result<(DeviceId, u64)> {
    let mut fields = params.split_ascii_whitespace();
    let dev = fields
        .next()
        .ok_or_else(|| Error::with_message(Errno::EINVAL, "linear 参数缺少 backing 设备"))?;
    let offset_str = fields
        .next()
        .ok_or_else(|| Error::with_message(Errno::EINVAL, "linear 参数缺少起始扇区"))?;

    // 可选的 "sectors" 后缀（某些 LVM2 版本添加）
    if let Some(unit) = fields.next() {
        if unit != "sectors" || fields.next().is_some() {
            return_errno_with_message!(Errno::EINVAL, "linear 参数中的单位字段无效");
        }
    }

    let (major, minor) = dev
        .split_once(':')
        .ok_or_else(|| Error::with_message(Errno::EINVAL, "linear backing 必须采用 major:minor"))?;
    let major = major
        .parse::<u16>()
        .map_err(|_| Error::with_message(Errno::EINVAL, "linear backing major 无效"))?;
    let minor = minor
        .parse::<u32>()
        .map_err(|_| Error::with_message(Errno::EINVAL, "linear backing minor 无效"))?;
    let major = MajorId::try_from(major)
        .map_err(|_| Error::with_message(Errno::EINVAL, "linear backing major 超出范围"))?;
    let minor = MinorId::try_from(minor)
        .map_err(|_| Error::with_message(Errno::EINVAL, "linear backing minor 超出范围"))?;
    let backing_start = offset_str
        .parse::<u64>()
        .map_err(|_| Error::with_message(Errno::EINVAL, "linear backing 起始扇区无效"))?;
    Ok((DeviceId::new(major, minor), backing_start))
}

fn validate_target_spec_next(data_start: usize, next: usize, buffer_len: usize) -> Result<usize> {
    // next == 0 表示这是最后一个 target spec，参数延伸到缓冲区末尾
    if next == 0 {
        return Ok(buffer_len);
    }
    if next < DM_TARGET_SPEC_SIZE || next % 8 != 0 {
        return_errno_with_message!(Errno::EINVAL, "dm_target_spec.next 无效");
    }
    let next_spec = data_start.checked_add(next).ok_or_else(invalid_buffer)?;
    if next_spec > buffer_len {
        return_errno_with_message!(Errno::EINVAL, "dm_target_spec.next 超出缓冲区范围");
    }
    Ok(next_spec)
}

fn table_status_record_len(params_len: usize) -> Result<usize> {
    let unaligned = DM_TARGET_SPEC_SIZE
        .checked_add(params_len)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(invalid_buffer)?;
    align_up(unaligned, 8)
}

fn name_list_record_layout(name_len: usize, uuid_len: usize) -> Result<(usize, usize)> {
    let extension_offset = align_up(12usize.checked_add(name_len).ok_or_else(invalid_buffer)?, 8)?;
    let record_end = extension_offset
        .checked_add(8)
        .and_then(|value| value.checked_add(uuid_len))
        .ok_or_else(invalid_buffer)?;
    Ok((extension_offset, align_up(record_end, 8)?))
}

fn manager() -> &'static DmManager {
    DM_MANAGER.get().unwrap()
}

fn data_start(buffer: &[u8]) -> Result<usize> {
    Ok(read_u32(buffer, OFF_DATA_START)? as usize)
}

fn available_from(buffer: &[u8], start: usize) -> usize {
    buffer.len().saturating_sub(start)
}

fn align_up(value: usize, alignment: usize) -> Result<usize> {
    value
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
        .ok_or_else(invalid_buffer)
}

fn set_buffer_full(buffer: &mut [u8]) -> Result<()> {
    let flags = read_u32(buffer, OFF_FLAGS)? | DM_BUFFER_FULL_FLAG;
    write_u32(buffer, OFF_FLAGS, flags)
}

fn write_version(buffer: &mut [u8]) -> Result<()> {
    for (index, value) in DM_VERSION.into_iter().enumerate() {
        write_u32(buffer, OFF_VERSION + index * 4, value)?;
    }
    Ok(())
}

fn required_c_string(buffer: &[u8], offset: usize, len: usize, field: &str) -> Result<String> {
    optional_c_string(buffer, offset, len, field)?
        .ok_or_else(|| Error::with_message(Errno::EINVAL, "Device Mapper 字符串字段不能为空"))
}

fn optional_c_string(
    buffer: &[u8],
    offset: usize,
    len: usize,
    _field: &str,
) -> Result<Option<String>> {
    require_range(buffer, offset, len)?;
    let bytes = &buffer[offset..offset + len];
    let Some(end) = bytes.iter().position(|byte| *byte == 0) else {
        return Err(Error::with_message(
            Errno::EINVAL,
            "Device Mapper 字符串字段缺少 NUL 终止符",
        ));
    };
    if end == 0 {
        return Ok(None);
    }
    let value = core::str::from_utf8(&bytes[..end]).map_err(|_| {
        Error::with_message(Errno::EINVAL, "Device Mapper 字符串字段不是有效 UTF-8")
    })?;
    Ok(Some(value.to_string()))
}

fn c_string_until(buffer: &[u8], start: usize, end: usize, _field: &str) -> Result<String> {
    if start >= end || end > buffer.len() {
        return Err(invalid_buffer());
    }
    let bytes = &buffer[start..end];
    let nul = bytes.iter().position(|byte| *byte == 0).ok_or_else(|| {
        Error::with_message(Errno::EINVAL, "Device Mapper 字符串字段缺少 NUL 终止符")
    })?;
    let value = core::str::from_utf8(&bytes[..nul]).map_err(|_| {
        Error::with_message(Errno::EINVAL, "Device Mapper 字符串字段不是有效 UTF-8")
    })?;
    Ok(value.to_string())
}

fn write_c_string_fixed(buffer: &mut [u8], offset: usize, len: usize, value: &str) -> Result<()> {
    if value.len() >= len {
        return_errno_with_message!(Errno::EINVAL, "输出字符串超过 ABI 字段长度");
    }
    require_range(buffer, offset, len)?;
    buffer[offset..offset + len].fill(0);
    buffer[offset..offset + value.len()].copy_from_slice(value.as_bytes());
    Ok(())
}

fn write_c_string(buffer: &mut [u8], offset: usize, value: &str) -> Result<()> {
    let len = value.len().checked_add(1).ok_or_else(invalid_buffer)?;
    require_range(buffer, offset, len)?;
    buffer[offset..offset + value.len()].copy_from_slice(value.as_bytes());
    buffer[offset + value.len()] = 0;
    Ok(())
}

fn require_range(buffer: &[u8], offset: usize, len: usize) -> Result<()> {
    let end = offset.checked_add(len).ok_or_else(invalid_buffer)?;
    if end > buffer.len() {
        return Err(invalid_buffer());
    }
    Ok(())
}

fn read_u32(buffer: &[u8], offset: usize) -> Result<u32> {
    require_range(buffer, offset, 4)?;
    Ok(u32::from_le_bytes(
        buffer[offset..offset + 4].try_into().unwrap(),
    ))
}

fn read_u64(buffer: &[u8], offset: usize) -> Result<u64> {
    require_range(buffer, offset, 8)?;
    Ok(u64::from_le_bytes(
        buffer[offset..offset + 8].try_into().unwrap(),
    ))
}

fn write_u32(buffer: &mut [u8], offset: usize, value: impl TryInto<u32>) -> Result<()> {
    let value = value
        .try_into()
        .map_err(|_| Error::with_message(Errno::EINVAL, "数值无法写入 u32 ABI 字段"))?;
    require_range(buffer, offset, 4)?;
    buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_u64(buffer: &mut [u8], offset: usize, value: u64) -> Result<()> {
    require_range(buffer, offset, 8)?;
    buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn invalid_buffer() -> Error {
    Error::with_message(Errno::EINVAL, "Device Mapper ioctl 缓冲区布局无效")
}

fn map_dm_error(error: DmError) -> Error {
    match error {
        DmError::NameExists | DmError::UuidExists | DmError::MinorBusy => {
            Error::with_message(Errno::EEXIST, "Device Mapper 设备标识已存在")
        }
        DmError::DeviceNotFound => Error::with_message(Errno::ENXIO, "Device Mapper 设备不存在"),
        DmError::MinorExhausted | DmError::MajorExhausted => {
            Error::with_message(Errno::ENOSPC, "Device Mapper 设备号已耗尽")
        }
        DmError::InvalidState => {
            Error::with_message(Errno::EINVAL, "Device Mapper 设备状态不允许该操作")
        }
        DmError::InvalidTable(error) => map_table_error(error),
    }
}

fn map_table_error(error: TableError) -> Error {
    match error {
        TableError::BackingRangeOutOfBounds => {
            Error::with_message(Errno::EINVAL, "linear 映射超过 backing 设备容量")
        }
        TableError::UnsupportedBackingDevice => {
            Error::with_message(Errno::EINVAL, "当前不支持 DM-on-DM backing")
        }
        _ => Error::with_message(Errno::EINVAL, "Device Mapper 映射表无效"),
    }
}

pub(super) fn init_in_first_kthread() {
    DM_MANAGER.call_once(|| DmManager::new().unwrap());
    char::register(DmControlDevice::new()).unwrap();
}

#[cfg(ktest)]
mod tests {
    use alloc::format;

    use aster_block::{BlockDeviceLease, BlockDeviceMeta, register, unregister};
    use ostd::prelude::ktest;

    use super::*;

    fn test_buffer(size: usize) -> Vec<u8> {
        let mut buffer = vec![0u8; size];
        write_u32(&mut buffer, OFF_DATA_SIZE, size).unwrap();
        write_u32(&mut buffer, OFF_DATA_START, DM_IOCTL_HEADER_SIZE).unwrap();
        buffer
    }

    #[derive(Debug)]
    struct StatusBacking {
        id: DeviceId,
    }

    impl StatusBacking {
        fn new(minor: u32) -> Arc<Self> {
            Self::new_with_major(1, minor)
        }

        fn new_with_major(major: u16, minor: u32) -> Arc<Self> {
            Arc::new(Self {
                id: DeviceId::new(MajorId::new(major), MinorId::new(minor)),
            })
        }
    }

    impl BlockDevice for StatusBacking {
        fn enqueue(
            &self,
            bio: aster_block::bio::SubmittedBio,
        ) -> core::result::Result<(), aster_block::bio::BioEnqueueError> {
            bio.complete(aster_block::bio::BioStatus::Complete);
            Ok(())
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 8,
                nr_sectors: 1_024,
            }
        }

        fn name(&self) -> String {
            String::from("dm-status-test-backing")
        }

        fn id(&self) -> DeviceId {
            self.id
        }
    }

    fn status_backing_lease(minor: u32) -> BlockDeviceLease {
        BlockDeviceLease::new_untracked(StatusBacking::new(minor) as Arc<dyn BlockDevice>)
    }

    fn write_linear_target_spec(
        buffer: &mut [u8],
        record: usize,
        start: u64,
        length: u64,
        next: u32,
        params: &str,
    ) {
        write_u64(buffer, record, start).unwrap();
        write_u64(buffer, record + 8, length).unwrap();
        write_u32(buffer, record + 16, 0).unwrap();
        write_u32(buffer, record + 20, next).unwrap();
        write_c_string_fixed(buffer, record + 24, DM_TARGET_TYPE_LEN, "linear").unwrap();
        write_c_string(buffer, record + DM_TARGET_SPEC_SIZE, params).unwrap();
    }

    fn single_linear_table(length: u64, backing_start: u64, backing_minor: u32) -> Arc<DmTable> {
        Arc::new(
            DmTable::new_linear(vec![
                LinearTarget::new(
                    Sid::new(0),
                    length,
                    Sid::new(backing_start),
                    status_backing_lease(backing_minor),
                )
                .unwrap(),
            ])
            .unwrap(),
        )
    }

    #[ktest]
    fn rejects_invalid_ioctl_buffer_layouts() {
        for (data_size, data_start) in [
            (DM_IOCTL_HEADER_SIZE - 1, DM_IOCTL_HEADER_SIZE),
            (DM_IOCTL_MAX_SIZE + 1, DM_IOCTL_HEADER_SIZE),
            (DM_IOCTL_HEADER_SIZE, DM_IOCTL_HEADER_SIZE - 8),
            (DM_IOCTL_HEADER_SIZE, DM_IOCTL_HEADER_SIZE + 8),
            (DM_IOCTL_HEADER_SIZE + 8, DM_IOCTL_HEADER_SIZE + 1),
        ] {
            assert_eq!(
                validate_ioctl_buffer_layout(data_size, data_start)
                    .unwrap_err()
                    .error(),
                Errno::EINVAL
            );
        }
        validate_ioctl_buffer_layout(DM_IOCTL_HEADER_SIZE, DM_IOCTL_HEADER_SIZE).unwrap();
        validate_ioctl_buffer_layout(DM_IOCTL_MAX_SIZE, DM_IOCTL_HEADER_SIZE).unwrap();
    }

    #[ktest]
    fn rejects_invalid_utf8_strings() {
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
        buffer[OFF_NAME] = 0xff;
        buffer[OFF_NAME + 1] = 0;
        assert_eq!(
            required_c_string(&buffer, OFF_NAME, DM_NAME_LEN, "名称")
                .unwrap_err()
                .error(),
            Errno::EINVAL
        );

        let params_start = DM_IOCTL_HEADER_SIZE - 8;
        buffer[params_start] = 0xff;
        buffer[params_start + 1] = 0;
        assert_eq!(
            c_string_until(&buffer, params_start, buffer.len(), "参数")
                .unwrap_err()
                .error(),
            Errno::EINVAL
        );
    }

    #[ktest]
    fn rejects_unterminated_and_out_of_bounds_strings() {
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
        buffer[OFF_NAME..OFF_NAME + DM_NAME_LEN].fill(b'x');
        assert_eq!(
            required_c_string(&buffer, OFF_NAME, DM_NAME_LEN, "名称")
                .unwrap_err()
                .error(),
            Errno::EINVAL
        );
        assert_eq!(
            c_string_until(&buffer, buffer.len(), buffer.len(), "参数")
                .unwrap_err()
                .error(),
            Errno::EINVAL
        );
    }

    #[ktest]
    fn fills_device_header_from_runtime_state() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create(
                "dm-control-header-test".to_string(),
                Some("dm-control-header-uuid".to_string()),
                None,
            )
            .unwrap();
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
        write_u32(&mut buffer, OFF_FLAGS, u32::MAX).unwrap();
        write_u32(&mut buffer, OFF_TARGET_COUNT, u32::MAX).unwrap();
        buffer[DM_IOCTL_FIXED_PREFIX_SIZE..DM_IOCTL_HEADER_SIZE].fill(0xa5);

        fill_device_header(&mut buffer, &device).unwrap();

        assert_eq!(read_u32(&buffer, OFF_FLAGS).unwrap(), DM_EXISTS_FLAG);
        assert_eq!(read_u32(&buffer, OFF_TARGET_COUNT).unwrap(), 0);
        assert_eq!(read_u32(&buffer, OFF_EVENT_NR).unwrap(), 0);
        assert_eq!(read_u32(&buffer, OFF_OPEN_COUNT).unwrap(), 0);
        assert_eq!(
            read_u64(&buffer, OFF_DEV).unwrap(),
            device.id().as_encoded_u64()
        );
        assert_eq!(
            required_c_string(&buffer, OFF_NAME, DM_NAME_LEN, "名称").unwrap(),
            device.name()
        );
        assert_eq!(
            required_c_string(&buffer, OFF_UUID, DM_UUID_LEN, "UUID").unwrap(),
            device.uuid().unwrap()
        );
        assert!(
            buffer[DM_IOCTL_FIXED_PREFIX_SIZE..DM_IOCTL_HEADER_SIZE]
                .iter()
                .all(|byte| *byte == 0)
        );
    }

    #[ktest]
    fn accepts_only_complete_aligned_ioctl_envelopes() {
        for data_size in [DM_IOCTL_FIXED_PREFIX_SIZE, DM_IOCTL_HEADER_SIZE - 1] {
            assert_eq!(
                validate_ioctl_buffer_layout(data_size, DM_IOCTL_HEADER_SIZE)
                    .unwrap_err()
                    .error(),
                Errno::EINVAL
            );
        }
        validate_ioctl_buffer_layout(DM_IOCTL_HEADER_SIZE, DM_IOCTL_HEADER_SIZE).unwrap();
    }

    #[ktest]
    fn reports_empty_tables_for_existing_fresh_device() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-empty-table-test".to_string(), None, None)
            .unwrap();

        let mut status = test_buffer(DM_IOCTL_HEADER_SIZE);
        table_status_for_device(&mut status, &device).unwrap();
        assert_eq!(read_u32(&status, OFF_TARGET_COUNT).unwrap(), 0);
        assert_eq!(read_u32(&status, OFF_FLAGS).unwrap(), DM_EXISTS_FLAG);

        let mut inactive_status = test_buffer(DM_IOCTL_HEADER_SIZE);
        write_u32(
            &mut inactive_status,
            OFF_FLAGS,
            DM_QUERY_INACTIVE_TABLE_FLAG,
        )
        .unwrap();
        table_status_for_device(&mut inactive_status, &device).unwrap();
        assert_eq!(read_u32(&inactive_status, OFF_TARGET_COUNT).unwrap(), 0);
        assert_eq!(
            read_u32(&inactive_status, OFF_FLAGS).unwrap(),
            DM_EXISTS_FLAG
        );

        let mut deps = test_buffer(DM_IOCTL_HEADER_SIZE + 16);
        table_deps_for_device(&mut deps, &device).unwrap();
        assert_eq!(read_u32(&deps, DM_IOCTL_HEADER_SIZE).unwrap(), 0);
        assert_eq!(read_u32(&deps, DM_IOCTL_HEADER_SIZE + 4).unwrap(), 0);
    }

    #[ktest]
    fn reports_three_table_status_records_with_linux_next_offsets() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-three-target-status-test".to_string(), None, None)
            .unwrap();
        let table = DmTable::new_linear(vec![
            LinearTarget::new(Sid::new(0), 4, Sid::new(100), status_backing_lease(1)).unwrap(),
            LinearTarget::new(Sid::new(4), 4, Sid::new(200), status_backing_lease(2)).unwrap(),
            LinearTarget::new(Sid::new(8), 4, Sid::new(300), status_backing_lease(3)).unwrap(),
        ])
        .unwrap();
        device.load_table(Arc::new(table));

        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 3 * 48);
        write_u32(
            &mut buffer,
            OFF_FLAGS,
            DM_QUERY_INACTIVE_TABLE_FLAG | DM_STATUS_TABLE_FLAG,
        )
        .unwrap();
        table_status_for_device(&mut buffer, &device).unwrap();

        assert_eq!(read_u32(&buffer, OFF_TARGET_COUNT).unwrap(), 3);
        for (index, (record, start, backing_minor, backing_start, next)) in [
            (DM_IOCTL_HEADER_SIZE, 0, 1, 100, 48),
            (DM_IOCTL_HEADER_SIZE + 48, 4, 2, 200, 96),
            (DM_IOCTL_HEADER_SIZE + 96, 8, 3, 300, 144),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(read_u64(&buffer, record).unwrap(), start);
            assert_eq!(read_u64(&buffer, record + 8).unwrap(), 4);
            assert_eq!(read_u32(&buffer, record + 16).unwrap(), 0);
            assert_eq!(read_u32(&buffer, record + 20).unwrap(), next);
            assert_eq!(
                c_string_until(&buffer, record + 24, record + 40, "target 类型").unwrap(),
                "linear"
            );
            assert_eq!(
                c_string_until(&buffer, record + 40, record + 48, "target 参数").unwrap(),
                format!("1:{} {}", backing_minor, backing_start)
            );
            assert_eq!(record, DM_IOCTL_HEADER_SIZE + index * 48);
        }
    }

    #[ktest]
    fn reports_linear_info_status_with_empty_params() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-linear-info-status-test".to_string(), None, None)
            .unwrap();
        device.load_table(single_linear_table(4, 100, 1));

        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 48);
        write_u32(&mut buffer, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
        table_status_for_device(&mut buffer, &device).unwrap();

        assert_eq!(read_u32(&buffer, OFF_TARGET_COUNT).unwrap(), 1);
        assert_eq!(read_u64(&buffer, DM_IOCTL_HEADER_SIZE).unwrap(), 0);
        assert_eq!(read_u64(&buffer, DM_IOCTL_HEADER_SIZE + 8).unwrap(), 4);
        assert_eq!(read_u32(&buffer, DM_IOCTL_HEADER_SIZE + 20).unwrap(), 48);
        assert_eq!(
            c_string_until(
                &buffer,
                DM_IOCTL_HEADER_SIZE + 24,
                DM_IOCTL_HEADER_SIZE + 40,
                "target 类型"
            )
            .unwrap(),
            "linear"
        );
        assert_eq!(
            c_string_until(
                &buffer,
                DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE,
                DM_IOCTL_HEADER_SIZE + 48,
                "target 参数"
            )
            .unwrap(),
            ""
        );
    }

    #[ktest]
    fn reports_table_deps_with_duplicate_backing_devices_deduplicated() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-dedup-deps-test".to_string(), None, None)
            .unwrap();
        let first = status_backing_lease(1);
        let second = status_backing_lease(2);
        let table = DmTable::new_linear(vec![
            LinearTarget::new(Sid::new(0), 4, Sid::new(100), first.clone()).unwrap(),
            LinearTarget::new(Sid::new(4), 4, Sid::new(104), first.clone()).unwrap(),
            LinearTarget::new(Sid::new(8), 4, Sid::new(200), second.clone()).unwrap(),
        ])
        .unwrap();
        device.load_table(Arc::new(table));

        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 24);
        write_u32(&mut buffer, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
        table_deps_for_device(&mut buffer, &device).unwrap();

        assert_eq!(read_u32(&buffer, DM_IOCTL_HEADER_SIZE).unwrap(), 2);
        assert_eq!(read_u32(&buffer, DM_IOCTL_HEADER_SIZE + 4).unwrap(), 0);
        assert_eq!(
            read_u64(&buffer, DM_IOCTL_HEADER_SIZE + 8).unwrap(),
            first.id().as_encoded_u64()
        );
        assert_eq!(
            read_u64(&buffer, DM_IOCTL_HEADER_SIZE + 16).unwrap(),
            second.id().as_encoded_u64()
        );
    }

    #[ktest]
    fn marks_table_status_buffer_full_when_first_record_does_not_fit() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-status-buffer-full-test".to_string(), None, None)
            .unwrap();
        let table = DmTable::new_linear(vec![
            LinearTarget::new(Sid::new(0), 4, Sid::new(100), status_backing_lease(1)).unwrap(),
            LinearTarget::new(Sid::new(4), 4, Sid::new(200), status_backing_lease(2)).unwrap(),
        ])
        .unwrap();
        device.load_table(Arc::new(table));

        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE - 1);
        write_u32(&mut buffer, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
        table_status_for_device(&mut buffer, &device).unwrap();

        assert_eq!(read_u32(&buffer, OFF_TARGET_COUNT).unwrap(), 2);
        assert_ne!(
            read_u32(&buffer, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG,
            0
        );
    }

    #[ktest]
    fn marks_table_deps_buffer_full_when_deps_do_not_fit() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-deps-buffer-full-test".to_string(), None, None)
            .unwrap();
        let table = DmTable::new_linear(vec![
            LinearTarget::new(Sid::new(0), 4, Sid::new(100), status_backing_lease(1)).unwrap(),
            LinearTarget::new(Sid::new(4), 4, Sid::new(200), status_backing_lease(2)).unwrap(),
        ])
        .unwrap();
        device.load_table(Arc::new(table));

        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 16);
        write_u32(&mut buffer, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
        table_deps_for_device(&mut buffer, &device).unwrap();

        assert_ne!(
            read_u32(&buffer, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG,
            0
        );
        assert_eq!(read_u32(&buffer, DM_IOCTL_HEADER_SIZE).unwrap(), 0);
    }

    #[ktest]
    fn reports_status_target_count_for_selected_table() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-device-status-test".to_string(), None, None)
            .unwrap();

        let mut fresh_status = test_buffer(DM_IOCTL_HEADER_SIZE);
        write_u64(&mut fresh_status, OFF_DEV, device.id().as_encoded_u64()).unwrap();
        device_status(&mut fresh_status).unwrap();
        assert_eq!(read_u32(&fresh_status, OFF_TARGET_COUNT).unwrap(), 0);
        assert_eq!(read_u32(&fresh_status, OFF_FLAGS).unwrap(), DM_EXISTS_FLAG);
    }

    #[ktest]
    fn resume_ioctl_activates_inactive_table_for_active_queries() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-resume-activates-table-test".to_string(), None, None)
            .unwrap();
        device.load_table(single_linear_table(4, 100, 1));

        let mut active_before = test_buffer(DM_IOCTL_HEADER_SIZE + 48);
        table_status_for_device(&mut active_before, &device).unwrap();
        assert_eq!(read_u32(&active_before, OFF_TARGET_COUNT).unwrap(), 0);

        let mut inactive_before = test_buffer(DM_IOCTL_HEADER_SIZE + 48);
        write_u32(
            &mut inactive_before,
            OFF_FLAGS,
            DM_QUERY_INACTIVE_TABLE_FLAG,
        )
        .unwrap();
        table_status_for_device(&mut inactive_before, &device).unwrap();
        assert_eq!(read_u32(&inactive_before, OFF_TARGET_COUNT).unwrap(), 1);

        device.resume().unwrap();
        assert!(device.active_table().is_some());
        assert!(device.inactive_table().is_none());

        let mut active_after = test_buffer(DM_IOCTL_HEADER_SIZE + 48);
        table_status_for_device(&mut active_after, &device).unwrap();
        assert_eq!(read_u32(&active_after, OFF_TARGET_COUNT).unwrap(), 1);
        assert_eq!(
            read_u64(&active_after, DM_IOCTL_HEADER_SIZE + 8).unwrap(),
            4
        );

        let mut inactive_after = test_buffer(DM_IOCTL_HEADER_SIZE + 48);
        write_u32(&mut inactive_after, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
        table_status_for_device(&mut inactive_after, &device).unwrap();
        assert_eq!(read_u32(&inactive_after, OFF_TARGET_COUNT).unwrap(), 0);
    }

    #[ktest]
    fn resume_ioctl_replaces_active_table_while_running() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-resume-replaces-table-test".to_string(), None, None)
            .unwrap();
        device.load_table(single_linear_table(4, 100, 1));
        device.resume().unwrap();

        device.load_table(single_linear_table(8, 200, 2));
        let mut inactive = test_buffer(DM_IOCTL_HEADER_SIZE + 48);
        write_u32(&mut inactive, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
        table_status_for_device(&mut inactive, &device).unwrap();
        assert_eq!(read_u32(&inactive, OFF_TARGET_COUNT).unwrap(), 1);
        assert_eq!(read_u64(&inactive, DM_IOCTL_HEADER_SIZE + 8).unwrap(), 8);

        device.resume().unwrap();
        assert!(device.active_table().is_some());
        assert!(device.inactive_table().is_none());

        let mut active = test_buffer(DM_IOCTL_HEADER_SIZE + 48);
        write_u32(&mut active, OFF_FLAGS, DM_STATUS_TABLE_FLAG).unwrap();
        table_status_for_device(&mut active, &device).unwrap();
        assert_eq!(read_u32(&active, OFF_TARGET_COUNT).unwrap(), 1);
        assert_eq!(read_u64(&active, DM_IOCTL_HEADER_SIZE + 8).unwrap(), 8);
        assert_eq!(
            c_string_until(
                &active,
                DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE,
                DM_IOCTL_HEADER_SIZE + 48,
                "target 参数"
            )
            .unwrap(),
            "1:2 200"
        );
    }

    #[ktest]
    fn selects_uuid_then_name_then_device_id() {
        let manager = DmManager::new().unwrap();
        let uuid_device = manager
            .create(
                "dm-selector-uuid".to_string(),
                Some("dm-selector-uuid-value".to_string()),
                None,
            )
            .unwrap();
        let name_device = manager
            .create("dm-selector-name".to_string(), None, None)
            .unwrap();

        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
        write_c_string_fixed(&mut buffer, OFF_NAME, DM_NAME_LEN, &name_device.name()).unwrap();
        write_c_string_fixed(
            &mut buffer,
            OFF_UUID,
            DM_UUID_LEN,
            uuid_device.uuid().unwrap(),
        )
        .unwrap();
        write_u64(&mut buffer, OFF_DEV, name_device.id().as_encoded_u64()).unwrap();
        assert_eq!(lookup_device(&buffer).unwrap().id(), uuid_device.id());

        write_c_string_fixed(&mut buffer, OFF_UUID, DM_UUID_LEN, "").unwrap();
        assert_eq!(lookup_device(&buffer).unwrap().id(), name_device.id());

        write_c_string_fixed(&mut buffer, OFF_NAME, DM_NAME_LEN, "").unwrap();
        assert_eq!(lookup_device(&buffer).unwrap().id(), name_device.id());
    }

    #[ktest]
    fn rejects_zero_target_table_load_before_state_changes() {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-zero-target-test".to_string(), None, None)
            .unwrap();
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);

        assert_eq!(
            table_load_for_device(&mut buffer, &device)
                .unwrap_err()
                .error(),
            Errno::EINVAL
        );
        assert_eq!(
            device.status(),
            aster_device_mapper::DmDeviceStatus {
                suspended: false,
                has_active_table: false,
                has_inactive_table: false,
                event_nr: 0,
            }
        );
    }

    #[ktest]
    fn loads_multiple_targets_using_dm_target_spec_next_offsets() {
        let first = StatusBacking::new_with_major(510, 101);
        let second = StatusBacking::new_with_major(510, 102);
        let first_id = first.id();
        let second_id = second.id();
        register(first as Arc<dyn BlockDevice>).unwrap();
        register(second as Arc<dyn BlockDevice>).unwrap();

        let manager = DmManager::new().unwrap();
        let device = manager
            .create("dm-table-load-next-test".to_string(), None, None)
            .unwrap();
        let first_next = table_status_record_len("510:101 100".len()).unwrap();
        let total_len = first_next + table_status_record_len("510:102 200".len()).unwrap();
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + total_len);
        write_u32(&mut buffer, OFF_TARGET_COUNT, 2).unwrap();
        write_linear_target_spec(
            &mut buffer,
            DM_IOCTL_HEADER_SIZE,
            0,
            4,
            first_next as u32,
            "510:101 100",
        );
        write_linear_target_spec(
            &mut buffer,
            DM_IOCTL_HEADER_SIZE + first_next,
            4,
            8,
            0,
            "510:102 200",
        );

        table_load_for_device(&mut buffer, &device).unwrap();
        let table = device.inactive_table().unwrap();
        assert_eq!(table.linears().len(), 2);
        assert_eq!(
            table.linears()[0].logical_range(),
            &(Sid::new(0)..Sid::new(4))
        );
        assert_eq!(table.linears()[0].backing_id(), first_id);
        assert_eq!(table.linears()[0].backing_start(), Sid::new(100));
        assert_eq!(
            table.linears()[1].logical_range(),
            &(Sid::new(4)..Sid::new(12))
        );
        assert_eq!(table.linears()[1].backing_id(), second_id);
        assert_eq!(table.linears()[1].backing_start(), Sid::new(200));

        drop(table);
        device.clear_inactive_table().unwrap();
        unregister(first_id).unwrap();
        unregister(second_id).unwrap();
    }

    #[ktest]
    fn parses_exact_linear_parameter_set() {
        let (id, start) = parse_linear_params("8:1 2048").unwrap();
        assert_eq!(id.major().get(), 8);
        assert_eq!(id.minor().get(), 1);
        assert_eq!(start, 2048);
        for params in [
            "8:1",
            "8:1 x",
            "8:1 0 extra",
            "8:1 0 sectors trailing",
            "bad 0",
        ] {
            assert_eq!(
                parse_linear_params(params).unwrap_err().error(),
                Errno::EINVAL
            );
        }
    }

    #[ktest]
    fn marks_short_output_buffers_without_overwriting_records() {
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
        list_versions(&mut buffer).unwrap();
        assert_ne!(
            read_u32(&buffer, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG,
            0
        );
    }

    #[ktest]
    fn validates_ioctl_encoding_and_alignment() {
        assert_eq!(decode_command(0xc138_fd00).unwrap(), DM_VERSION_CMD);
        assert_eq!(decode_command(0xc138_fd05).unwrap(), DM_DEV_RENAME_CMD);
        assert_eq!(decode_command(0xc138_fd09).unwrap(), DM_TABLE_LOAD_CMD);
        assert_eq!(
            decode_command(0xc138_fd11).unwrap(),
            DM_GET_TARGET_VERSION_CMD
        );
        assert_eq!(
            decode_command(0xc138_fd10).unwrap_err().error(),
            Errno::ENOTTY
        );
        assert_eq!(decode_command(0).unwrap_err().error(), Errno::ENOTTY);
        assert_eq!(align_up(313, 8).unwrap(), 320);
    }

    #[ktest]
    fn lists_linear_and_striped_target_versions() {
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 48);
        list_versions(&mut buffer).unwrap();

        assert_eq!(read_u32(&buffer, DM_IOCTL_HEADER_SIZE).unwrap(), 24);
        assert_eq!(
            [
                read_u32(&buffer, DM_IOCTL_HEADER_SIZE + 4).unwrap(),
                read_u32(&buffer, DM_IOCTL_HEADER_SIZE + 8).unwrap(),
                read_u32(&buffer, DM_IOCTL_HEADER_SIZE + 12).unwrap(),
            ],
            [1, 4, 0]
        );
        assert_eq!(
            c_string_until(
                &buffer,
                DM_IOCTL_HEADER_SIZE + 16,
                DM_IOCTL_HEADER_SIZE + 24,
                "target 名称"
            )
            .unwrap(),
            "linear"
        );

        let striped = DM_IOCTL_HEADER_SIZE + 24;
        assert_eq!(read_u32(&buffer, striped).unwrap(), 0);
        assert_eq!(
            [
                read_u32(&buffer, striped + 4).unwrap(),
                read_u32(&buffer, striped + 8).unwrap(),
                read_u32(&buffer, striped + 12).unwrap(),
            ],
            [1, 6, 0]
        );
        assert_eq!(
            c_string_until(&buffer, striped + 16, striped + 24, "target 名称").unwrap(),
            "striped"
        );
    }

    #[ktest]
    fn marks_partial_target_list_without_dangling_next() {
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 24);
        list_versions(&mut buffer).unwrap();
        assert_ne!(
            read_u32(&buffer, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG,
            0
        );
        assert_eq!(read_u32(&buffer, DM_IOCTL_HEADER_SIZE).unwrap(), 0);
    }

    #[ktest]
    fn gets_named_target_version_record() {
        for (name, version) in [("linear", [1, 4, 0]), ("striped", [1, 6, 0])] {
            let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 24);
            write_c_string_fixed(&mut buffer, OFF_NAME, DM_NAME_LEN, name).unwrap();
            get_target_version(&mut buffer).unwrap();
            assert_eq!(read_u32(&buffer, DM_IOCTL_HEADER_SIZE).unwrap(), 0);
            assert_eq!(
                [
                    read_u32(&buffer, DM_IOCTL_HEADER_SIZE + 4).unwrap(),
                    read_u32(&buffer, DM_IOCTL_HEADER_SIZE + 8).unwrap(),
                    read_u32(&buffer, DM_IOCTL_HEADER_SIZE + 12).unwrap(),
                ],
                version
            );
            assert_eq!(
                c_string_until(
                    &buffer,
                    DM_IOCTL_HEADER_SIZE + 16,
                    buffer.len(),
                    "target 名称"
                )
                .unwrap(),
                name
            );
        }

        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 24);
        write_c_string_fixed(&mut buffer, OFF_NAME, DM_NAME_LEN, "unsupported").unwrap();
        assert_eq!(
            get_target_version(&mut buffer).unwrap_err().error(),
            Errno::EINVAL
        );
    }

    #[ktest]
    fn rejects_invalid_target_spec_next_values() {
        assert_eq!(
            validate_target_spec_next(DM_IOCTL_HEADER_SIZE, 39, DM_IOCTL_HEADER_SIZE + 48)
                .unwrap_err()
                .error(),
            Errno::EINVAL
        );
        assert_eq!(
            validate_target_spec_next(DM_IOCTL_HEADER_SIZE, 41, DM_IOCTL_HEADER_SIZE + 48)
                .unwrap_err()
                .error(),
            Errno::EINVAL
        );
        assert_eq!(
            validate_target_spec_next(DM_IOCTL_HEADER_SIZE, 56, DM_IOCTL_HEADER_SIZE + 48)
                .unwrap_err()
                .error(),
            Errno::EINVAL
        );
        assert_eq!(
            validate_target_spec_next(DM_IOCTL_HEADER_SIZE, 48, DM_IOCTL_HEADER_SIZE + 48).unwrap(),
            DM_IOCTL_HEADER_SIZE + 48
        );
    }

    #[ktest]
    fn calculates_table_status_record_end_offset() {
        assert_eq!(table_status_record_len(0).unwrap(), 48);
        assert_eq!(table_status_record_len("8:1 2048".len()).unwrap(), 56);
        assert_eq!(
            table_status_record_len(usize::MAX).unwrap_err().error(),
            Errno::EINVAL
        );
    }

    #[ktest]
    fn calculates_name_list_extension_and_record_padding() {
        assert_eq!(name_list_record_layout(1, 0).unwrap(), (16, 24));
        assert_eq!(name_list_record_layout(4, 0).unwrap(), (16, 24));
        assert_eq!(name_list_record_layout(5, 0).unwrap(), (24, 32));
        assert_eq!(name_list_record_layout(4, 5).unwrap(), (16, 32));
        assert_eq!(DM_NAME_LIST_FLAG_HAS_UUID, 1);
        assert_eq!(DM_NAME_LIST_FLAG_DOESNT_HAVE_UUID, 2);
    }
}
