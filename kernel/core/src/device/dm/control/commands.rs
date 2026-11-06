// SPDX-License-Identifier: MPL-2.0

//! Dispatch of the supported Device Mapper control commands.

use aster_block::id::Sid;
use aster_dm::{
    DmDevice, DmError, DmTable, TableError,
    target::{DmTargetParseError, TargetStatusMode, parse_target_with, supported_target_metadata},
};
use device_id::{DeviceId, MajorId, MinorId, decode_device_numbers};

use super::{
    super::{DM_MANAGER, runtime},
    abi::*,
};
use crate::{
    device::registry::block,
    fs::{file::InodeType, vfs::path::FsPath},
    prelude::*,
};

type CommandHandler = fn(&mut [u8]) -> Result<()>;

/// Selects only commands implemented by this control device.
pub(super) fn handler_for(command: u8) -> Result<CommandHandler> {
    match command {
        DM_DEV_CREATE_CMD => Ok(create_device),
        DM_DEV_REMOVE_CMD => Ok(remove_device),
        DM_LIST_DEVICES_CMD => Ok(list_devices),
        DM_LIST_VERSIONS_CMD => Ok(list_versions),
        DM_DEV_STATUS_CMD => Ok(device_status),
        DM_TABLE_STATUS_CMD => Ok(table_status),
        DM_DEV_SUSPEND_CMD => Ok(resume_device),
        DM_TABLE_LOAD_CMD => Ok(table_load),
        _ => return_errno_with_message!(Errno::ENOTTY, "unsupported device-mapper ioctl command"),
    }
}

fn ensure_control_live(device: &DmDevice) -> Result<()> {
    if device.removal_isolated() {
        return_errno_with_message!(Errno::ENXIO, "mapper removal is in progress");
    }
    Ok(())
}

/// Removes a mapper only after runtime unregistration succeeds. A partially
/// unregistered wrapper stays indexed and reserved for a cleanup-only retry.
fn remove_device(buffer: &mut [u8]) -> Result<()> {
    if read_u32(buffer, OFF_FLAGS) & DEFERRED_REMOVE_FLAG != 0 {
        return_errno_with_message!(Errno::EINVAL, "deferred mapper removal is not supported");
    }
    let device = lookup_load_device(buffer)?;
    let _lifecycle = device.lock_lifecycle();
    let isolated = device.removal_isolated();
    if isolated && !runtime::primary_isolated(&device) {
        return_errno_with_message!(Errno::EBUSY, "mapper removal is in progress");
    }
    if (isolated
        || block::is_runtime_primary_registered(device.id())
        || runtime::primary_isolated(&device))
        && let Err(error) = runtime::unregister_primary(&device)
    {
        if runtime::primary_isolated(&device) {
            device.isolate_removal();
        }
        return Err(error);
    }
    device.isolate_removal();
    device.stop_and_drain_io();
    fill_device_header(buffer, &device, false);
    device.clear_removed_tables();
    DM_MANAGER
        .get()
        .unwrap()
        .remove(&device.mapper_name())
        .map_err(|_| Error::new(Errno::ENXIO))?;
    device.drain_removed();
    Ok(())
}

/// Creates a mapped device and writes its assigned identity back into the ioctl header.
fn create_device(buffer: &mut [u8]) -> Result<()> {
    let name = required_name(buffer)?;
    let uuid = optional_uuid(buffer);
    let flags = read_u32(buffer, OFF_FLAGS);
    let requested_minor = if flags & PERSISTENT_DEV_FLAG != 0 {
        Some(decode_device_numbers(read_u64(buffer, OFF_DEV)).1)
    } else {
        None
    };

    validate_mapper_name(&name)?;
    let device = DM_MANAGER
        .get()
        .unwrap()
        .create(name, uuid, requested_minor)
        .map_err(map_create_error)?;
    fill_device_header(buffer, &device, false);
    Ok(())
}

fn required_name(buffer: &[u8]) -> Result<String> {
    let name = &buffer[OFF_NAME..OFF_NAME + NAME_LEN];
    let end = name.iter().position(|byte| *byte == 0).unwrap();
    core::str::from_utf8(&name[..end])
        .map(String::from)
        .map_err(|_| Error::with_message(Errno::EINVAL, "mapper name is not UTF-8"))
}

fn optional_uuid(buffer: &[u8]) -> Option<Vec<u8>> {
    let uuid = &buffer[OFF_UUID..OFF_UUID + UUID_LEN];
    let end = uuid.iter().position(|byte| *byte == 0).unwrap();
    (end != 0).then(|| uuid[..end].to_vec())
}

fn validate_mapper_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() >= NAME_LEN
        || name.contains('/')
        || matches!(name, "." | ".." | "control")
    {
        return_errno_with_message!(Errno::EINVAL, "invalid mapper name");
    }
    Ok(())
}

fn fill_device_header(buffer: &mut [u8], device: &DmDevice, prefer_inactive: bool) {
    let (active, inactive) = device.table_snapshot();
    // LOAD reports the candidate; RESUME describes the table currently serving I/O.
    let selected = if prefer_inactive {
        inactive.as_ref().or(active.as_ref())
    } else {
        active.as_ref().or(inactive.as_ref())
    };
    write_u32(
        buffer,
        OFF_TARGET_COUNT,
        selected.map_or(0, |table| table.target_count() as u32),
    );
    write_u32(buffer, OFF_OPEN_COUNT, 0);
    write_u32(buffer, OFF_EVENT_NR, 0);
    write_u64(buffer, OFF_DEV, device.id().as_encoded_u64());
    let mut flags = read_u32(buffer, OFF_FLAGS)
        & !(READONLY_FLAG
            | SUSPEND_FLAG
            | ACTIVE_PRESENT_FLAG
            | INACTIVE_PRESENT_FLAG
            | INTERNAL_SUSPEND_FLAG);
    if active.is_some() {
        flags |= ACTIVE_PRESENT_FLAG;
    }
    if inactive.is_some() {
        flags |= INACTIVE_PRESENT_FLAG;
    }
    if selected.is_some_and(|table| table.is_readonly()) {
        flags |= READONLY_FLAG;
    }
    if device.is_suspended() {
        flags |= SUSPEND_FLAG;
    }
    write_u32(buffer, OFF_FLAGS, flags);
    let name = device.mapper_name();
    buffer[OFF_NAME..OFF_NAME + NAME_LEN].fill(0);
    buffer[OFF_NAME..OFF_NAME + name.len()].copy_from_slice(name.as_bytes());
    buffer[OFF_UUID..OFF_UUID + UUID_LEN].fill(0);
    if let Some(uuid) = device.uuid() {
        buffer[OFF_UUID..OFF_UUID + uuid.len()].copy_from_slice(&uuid);
    }
}

fn query_header(
    buffer: &mut [u8],
    device: &DmDevice,
    active: Option<&Arc<DmTable>>,
    inactive: Option<&Arc<DmTable>>,
    selected: Option<&Arc<DmTable>>,
) {
    write_u32(
        buffer,
        OFF_TARGET_COUNT,
        selected.map_or(0, |table| table.target_count() as u32),
    );
    // CREATE has no published block wrapper; after LOAD, absence is a transient registry state.
    // Queries report zero opens when there is no published primary device.
    write_u32(
        buffer,
        OFF_OPEN_COUNT,
        block::open_count(device.id()).unwrap_or(0) as u32,
    );
    write_u32(buffer, OFF_EVENT_NR, 0);
    write_u64(buffer, OFF_DEV, device.id().as_encoded_u64());
    let mut flags = read_u32(buffer, OFF_FLAGS)
        & !(READONLY_FLAG
            | SUSPEND_FLAG
            | ACTIVE_PRESENT_FLAG
            | INACTIVE_PRESENT_FLAG
            | INTERNAL_SUSPEND_FLAG);
    if active.is_some() {
        flags |= ACTIVE_PRESENT_FLAG;
    }
    if inactive.is_some() {
        flags |= INACTIVE_PRESENT_FLAG;
    }
    if selected.is_some_and(|table| table.is_readonly()) {
        flags |= READONLY_FLAG;
    }
    if device.is_suspended() {
        flags |= SUSPEND_FLAG;
    }
    write_u32(buffer, OFF_FLAGS, flags);
    let name = device.mapper_name();
    buffer[OFF_NAME..OFF_NAME + NAME_LEN].fill(0);
    buffer[OFF_NAME..OFF_NAME + name.len()].copy_from_slice(name.as_bytes());
    buffer[OFF_UUID..OFF_UUID + UUID_LEN].fill(0);
    if let Some(uuid) = device.uuid() {
        buffer[OFF_UUID..OFF_UUID + uuid.len()].copy_from_slice(&uuid);
    }
}

fn selected_table<'a>(
    active: &'a Option<Arc<DmTable>>,
    inactive: &'a Option<Arc<DmTable>>,
    query_inactive: bool,
) -> Option<&'a Arc<DmTable>> {
    if query_inactive {
        inactive.as_ref()
    } else {
        active.as_ref()
    }
}

fn device_status(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_load_device(buffer)?;
    let _lifecycle = device.lock_lifecycle();
    ensure_control_live(&device)?;
    let (active, inactive) = device.table_snapshot();
    let table = selected_table(
        &active,
        &inactive,
        read_u32(buffer, OFF_FLAGS) & QUERY_INACTIVE_TABLE_FLAG != 0,
    );
    query_header(buffer, &device, active.as_ref(), inactive.as_ref(), table);
    Ok(())
}

fn output_start(buffer: &mut [u8]) -> usize {
    let end = buffer.len().min(DM_IOCTL_HEADER_SIZE);
    buffer[FIXED_PREFIX_SIZE..end].fill(0);
    write_u32(buffer, OFF_DATA_START, DM_IOCTL_HEADER_SIZE as u32);
    DM_IOCTL_HEADER_SIZE
}

fn table_status(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_load_device(buffer)?;
    let _lifecycle = device.lock_lifecycle();
    ensure_control_live(&device)?;
    let flags = read_u32(buffer, OFF_FLAGS);
    let (active, inactive) = device.table_snapshot();
    let table = selected_table(&active, &inactive, flags & QUERY_INACTIVE_TABLE_FLAG != 0);
    query_header(buffer, &device, active.as_ref(), inactive.as_ref(), table);
    let Some(table) = table else {
        return Ok(());
    };
    let mode = if flags & STATUS_TABLE_FLAG != 0 {
        TargetStatusMode::Table
    } else {
        TargetStatusMode::Status
    };
    encode_table_records(buffer, table, mode)
}

fn encode_table_records(buffer: &mut [u8], table: &DmTable, mode: TargetStatusMode) -> Result<()> {
    let start = output_start(buffer);
    write_u32(buffer, OFF_DATA_SIZE, start as u32);
    let mut cursor = start;
    for target in table.targets() {
        if buffer.len().saturating_sub(cursor) <= DM_TARGET_SPEC_SIZE {
            write_u32(
                buffer,
                OFF_FLAGS,
                read_u32(buffer, OFF_FLAGS) | BUFFER_FULL_FLAG,
            );
            break;
        }
        let params = target.status_params(mode).map_err(map_table_error)?;
        let used = DM_TARGET_SPEC_SIZE
            .checked_add(params.len())
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| Error::new(Errno::EINVAL))?;
        if buffer.len().saturating_sub(cursor) <= used {
            write_u32(
                buffer,
                OFF_FLAGS,
                read_u32(buffer, OFF_FLAGS) | BUFFER_FULL_FLAG,
            );
            break;
        }
        let record_len = used
            .checked_add(7)
            .ok_or_else(|| Error::new(Errno::EINVAL))?
            & !7;
        let end = buffer.len().min(cursor + record_len);
        buffer[cursor..end].fill(0);
        write_u64(buffer, cursor, target.logical_range().start.to_raw());
        write_u64(buffer, cursor + 8, target.length());
        write_u32(buffer, cursor + 20, (cursor + record_len - start) as u32);
        let name = target.name().as_bytes();
        if name.len() >= DM_TARGET_SPEC_SIZE - 24 {
            return_errno_with_message!(Errno::EINVAL, "target type exceeds ABI field");
        }
        buffer[cursor + 24..cursor + 24 + name.len()].copy_from_slice(name);
        buffer[cursor + DM_TARGET_SPEC_SIZE..cursor + DM_TARGET_SPEC_SIZE + params.len()]
            .copy_from_slice(params.as_bytes());
        write_u32(buffer, OFF_DATA_SIZE, (cursor + used) as u32);
        cursor += record_len;
    }
    Ok(())
}

/// Reports only targets accepted by the table parser.
fn list_versions(buffer: &mut [u8]) -> Result<()> {
    let targets = supported_target_metadata();
    let start = output_start(buffer);
    let needed = targets.iter().try_fold(0usize, |total, target| {
        let record_len = 16usize
            .checked_add(target.name().len())
            .and_then(|len| len.checked_add(1 + 7))
            .ok_or_else(|| Error::new(Errno::EINVAL))?
            & !7;
        total
            .checked_add(record_len)
            .ok_or_else(|| Error::new(Errno::EINVAL))
    })?;
    write_u32(buffer, OFF_DATA_SIZE, start as u32);
    if buffer.len().saturating_sub(start) < needed {
        write_u32(
            buffer,
            OFF_FLAGS,
            read_u32(buffer, OFF_FLAGS) | BUFFER_FULL_FLAG,
        );
        return Ok(());
    }
    buffer[start..start + needed].fill(0);
    let mut cursor = start;
    for (index, target) in targets.iter().enumerate() {
        let record_len = (16 + target.name().len() + 1 + 7) & !7;
        write_u32(
            buffer,
            cursor,
            if index + 1 == targets.len() {
                0
            } else {
                record_len as u32
            },
        );
        for (word, version) in target.version().into_iter().enumerate() {
            write_u32(buffer, cursor + 4 + word * 4, version);
        }
        buffer[cursor + 16..cursor + 16 + target.name().len()]
            .copy_from_slice(target.name().as_bytes());
        cursor += record_len;
    }
    write_u32(buffer, OFF_DATA_SIZE, (start + needed) as u32);
    Ok(())
}

fn list_devices(buffer: &mut [u8]) -> Result<()> {
    let name_filter = optional_bytes(buffer, OFF_NAME, NAME_LEN).map(<[u8]>::to_vec);
    let uuid_filter = optional_bytes(buffer, OFF_UUID, UUID_LEN).map(<[u8]>::to_vec);
    let include_uuid = read_u32(buffer, OFF_FLAGS) & UUID_FLAG != 0;
    let devices: Vec<_> = DM_MANAGER
        .get()
        .unwrap()
        .devices()
        .into_iter()
        .filter(|device| {
            !device.removal_isolated()
                && device
                    .mapper_name()
                    .as_bytes()
                    .starts_with(name_filter.as_deref().unwrap_or_default())
                && device
                    .uuid()
                    .as_deref()
                    .unwrap_or_default()
                    .starts_with(uuid_filter.as_deref().unwrap_or_default())
        })
        .collect();
    encode_device_list(buffer, &devices, include_uuid)
}

fn encode_device_list(
    buffer: &mut [u8],
    devices: &[Arc<DmDevice>],
    include_uuid: bool,
) -> Result<()> {
    let start = output_start(buffer);
    let mut needed = 0usize;
    for device in devices {
        let uuid_len = if include_uuid {
            device.uuid().map_or(0, |uuid| uuid.len() + 1)
        } else {
            0
        };
        let extension = (12 + device.mapper_name().len() + 1 + 7) & !7;
        needed = needed
            .checked_add((extension + 8 + uuid_len + 7) & !7)
            .ok_or_else(|| Error::new(Errno::EINVAL))?;
    }
    if buffer.len().saturating_sub(start) < needed.max(8) {
        write_u32(
            buffer,
            OFF_FLAGS,
            read_u32(buffer, OFF_FLAGS) | BUFFER_FULL_FLAG,
        );
        write_u32(buffer, OFF_DATA_SIZE, start as u32);
        return Ok(());
    }
    buffer[start..start + needed.max(8)].fill(0);
    let mut cursor = start;
    for device in devices {
        let name = device.mapper_name();
        let uuid = device.uuid();
        let extension = (12 + name.len() + 1 + 7) & !7;
        let uuid_len = if include_uuid {
            uuid.as_ref().map_or(0, |uuid| uuid.len() + 1)
        } else {
            0
        };
        let record_len = (extension + 8 + uuid_len + 7) & !7;
        write_u64(buffer, cursor, device.id().as_encoded_u64());
        write_u32(
            buffer,
            cursor + 8,
            if cursor + record_len < start + needed {
                record_len as u32
            } else {
                0
            },
        );
        buffer[cursor + 12..cursor + 12 + name.len()].copy_from_slice(name.as_bytes());
        write_u32(
            buffer,
            cursor + extension + 4,
            if include_uuid {
                if uuid.is_some() { 1 } else { 2 }
            } else {
                0
            },
        );
        if include_uuid && let Some(uuid) = uuid {
            buffer[cursor + extension + 8..cursor + extension + 8 + uuid.len()]
                .copy_from_slice(&uuid);
        }
        cursor += record_len;
    }
    write_u32(buffer, OFF_DATA_SIZE, (start + needed) as u32);
    Ok(())
}

/// Resolves a LOAD selector against the current manager without publishing it.
fn lookup_load_device(buffer: &[u8]) -> Result<Arc<DmDevice>> {
    let manager = DM_MANAGER.get().unwrap();
    let name = optional_field(buffer, OFF_NAME, NAME_LEN)?;
    let uuid = optional_bytes(buffer, OFF_UUID, UUID_LEN);
    let raw = read_u64(buffer, OFF_DEV);
    if name.is_some() && uuid.is_some() {
        return_errno_with_message!(
            Errno::EINVAL,
            "name and UUID selectors are mutually exclusive"
        );
    }
    if raw != 0 && (name.is_some() || uuid.is_some()) {
        return_errno_with_message!(Errno::ENXIO, "ambiguous mapper selector");
    }
    let device = if let Some(uuid) = uuid {
        manager.lookup_uuid(uuid)
    } else if let Some(name) = name {
        manager.lookup_name(name)
    } else if raw != 0 {
        let (major, minor) = decode_device_numbers(raw);
        let major = u16::try_from(major)
            .ok()
            .and_then(|major| MajorId::try_from(major).ok())
            .ok_or_else(|| Error::new(Errno::ENXIO))?;
        let minor = MinorId::try_from(minor).map_err(|_| Error::new(Errno::ENXIO))?;
        manager.lookup_id(DeviceId::new(major, minor))
    } else {
        return_errno_with_message!(Errno::ENXIO, "missing mapper selector");
    };
    device.ok_or_else(|| Error::with_message(Errno::ENXIO, "mapper device not found"))
}

fn optional_field(buffer: &[u8], offset: usize, len: usize) -> Result<Option<&str>> {
    optional_bytes(buffer, offset, len)
        .map(|bytes| core::str::from_utf8(bytes).map_err(|_| Error::new(Errno::EINVAL)))
        .transpose()
}

fn optional_bytes(buffer: &[u8], offset: usize, len: usize) -> Option<&[u8]> {
    let field = &buffer[offset..offset + len];
    let end = field.iter().position(|&byte| byte == 0).unwrap_or(len);
    (end != 0).then_some(&field[..end])
}

/// Parses the complete linear table before publishing the primary device.
fn table_load(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_load_device(buffer)?;
    let _lifecycle = device.lock_lifecycle();
    ensure_control_live(&device)?;
    table_load_for_device_with_primary(buffer, &device, runtime::ensure_primary)
}

fn table_load_for_device_with_primary(
    buffer: &mut [u8],
    device: &Arc<DmDevice>,
    publish_primary: impl FnOnce(&Arc<DmDevice>) -> Result<()>,
) -> Result<()> {
    let table = parse_table_load_request(buffer)?;
    publish_primary(device)?;
    device.load_table(table);
    fill_device_header(buffer, device, true);
    Ok(())
}

/// Suspends a running mapping or resumes its existing active table.
fn resume_device(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_load_device(buffer)?;
    let _lifecycle = device.lock_lifecycle();
    ensure_control_live(&device)?;
    resume_device_for(buffer, &device)
}

fn resume_device_for(buffer: &mut [u8], device: &DmDevice) -> Result<()> {
    let flags = read_u32(buffer, OFF_FLAGS);
    if flags & (NOFLUSH_FLAG | SKIP_LOCKFS_FLAG) != 0 {
        return_errno_with_message!(
            Errno::EINVAL,
            "noflush and lockfs control are not supported"
        );
    }
    if flags & SUSPEND_FLAG != 0 {
        device.suspend().map_err(|_| Error::new(Errno::EINVAL))?;
    } else if device.is_suspended() {
        device
            .resume_suspended()
            .map_err(|_| Error::new(Errno::EINVAL))?;
    } else {
        device.resume_initial().map_err(|_| {
            Error::with_message(
                Errno::EINVAL,
                "mapper has no initial table or requires suspend",
            )
        })?;
    }
    fill_device_header(buffer, device, false);
    Ok(())
}

/// Builds a fully validated inactive table before any runtime publication.
fn parse_table_load_request(buffer: &[u8]) -> Result<Arc<DmTable>> {
    parse_table_load_request_with(buffer, parse_backing_device_id)
}

fn parse_table_load_request_with(
    buffer: &[u8],
    mut parse_backing: impl FnMut(&str) -> Result<DeviceId>,
) -> Result<Arc<DmTable>> {
    let target_count = read_u32(buffer, OFF_TARGET_COUNT);
    if target_count == 0 {
        return_errno_with_message!(Errno::EINVAL, "mapping table requires a target");
    }
    let data_start = read_u32(buffer, OFF_DATA_START) as usize;
    if data_start < DM_IOCTL_HEADER_SIZE
        || data_start > buffer.len()
        || !data_start.is_multiple_of(8)
    {
        return_errno_with_message!(Errno::EINVAL, "invalid target data offset");
    }
    let spec_count = usize::try_from(target_count).map_err(|_| Error::new(Errno::EINVAL))?;
    if spec_count > buffer.len().saturating_sub(data_start) / (DM_TARGET_SPEC_SIZE + 1) {
        return_errno_with_message!(Errno::EINVAL, "target count exceeds payload");
    }
    let mut cursor = data_start;
    let mut targets = Vec::new();
    let mut resolve_backing = |id| {
        aster_block::lookup_lease(id)
            .ok_or_else(|| Error::with_message(Errno::ENODEV, "backing device is unavailable"))
    };
    for index in 0..target_count {
        let spec_end = cursor
            .checked_add(DM_TARGET_SPEC_SIZE)
            .ok_or_else(|| Error::new(Errno::EINVAL))?;
        if spec_end > buffer.len() {
            return_errno_with_message!(Errno::EINVAL, "truncated target specification");
        }
        let logical_start = read_u64(buffer, cursor);
        let length = read_u64(buffer, cursor + 8);
        let next = read_u32(buffer, cursor + 20) as usize;
        let target_type = bounded_string(&buffer[cursor + 24..spec_end])?;
        let params_end = buffer[spec_end..]
            .iter()
            .position(|&byte| byte == 0)
            .ok_or_else(|| Error::new(Errno::EINVAL))?
            + spec_end;
        let params = core::str::from_utf8(&buffer[spec_end..params_end])
            .map_err(|_| Error::new(Errno::EINVAL))?;
        let target = parse_target_with(
            target_type,
            Sid::new(logical_start),
            length,
            params,
            &mut parse_backing,
            &mut resolve_backing,
        )
        .map_err(map_target_parse_error)?;
        targets.push(target);
        if index + 1 < target_count {
            let min_next = DM_TARGET_SPEC_SIZE
                .checked_add(params.len())
                .and_then(|length| length.checked_add(1))
                .ok_or_else(|| Error::new(Errno::EINVAL))?;
            if next < min_next || !next.is_multiple_of(8) {
                return_errno_with_message!(Errno::EINVAL, "invalid next target offset");
            }
            cursor = cursor
                .checked_add(next)
                .ok_or_else(|| Error::new(Errno::EINVAL))?;
            if cursor > buffer.len() {
                return_errno_with_message!(Errno::EINVAL, "next target exceeds payload");
            }
        }
    }
    let readonly = read_u32(buffer, OFF_FLAGS) & READONLY_FLAG != 0;
    Ok(Arc::new(
        DmTable::new_targets_with_readonly(targets, readonly).map_err(map_table_error)?,
    ))
}

fn bounded_string(buffer: &[u8]) -> Result<&str> {
    let end = buffer
        .iter()
        .position(|&byte| byte == 0)
        .ok_or_else(|| Error::new(Errno::EINVAL))?;
    core::str::from_utf8(&buffer[..end]).map_err(|_| Error::new(Errno::EINVAL))
}

fn parse_backing_device_id(dev: &str) -> Result<DeviceId> {
    if let Some((major, minor)) = dev.split_once(':')
        && let (Ok(major), Ok(minor)) = (major.parse::<u16>(), minor.parse::<u32>())
        && let (Ok(major), Ok(minor)) = (MajorId::try_from(major), MinorId::try_from(minor))
    {
        return Ok(DeviceId::new(major, minor));
    }
    let current_task = ostd::task::Task::current().unwrap();
    let thread_local = AsThreadLocal::as_thread_local(&current_task).unwrap();
    let fs = thread_local.borrow_fs();
    let resolver = fs.resolver().read();
    let path = resolver.lookup(&FsPath::try_from(dev)?)?;
    if path.type_() != InodeType::BlockDevice {
        return_errno_with_message!(Errno::ENOTBLK, "backing path is not a block device");
    }
    path.metadata()?
        .self_dev_id
        .ok_or_else(|| Error::new(Errno::ENOTBLK))
}

fn map_table_error(error: TableError) -> Error {
    match error {
        TableError::UnsupportedBackingDevice | TableError::BackingRangeOutOfBounds => {
            Error::with_message(Errno::EINVAL, "unsupported or out-of-range backing device")
        }
        _ => Error::with_message(Errno::EINVAL, "invalid mapper table"),
    }
}

fn map_target_parse_error(error: DmTargetParseError<Error>) -> Error {
    match error {
        DmTargetParseError::UnsupportedTarget => {
            Error::with_message(Errno::EINVAL, "unsupported target")
        }
        DmTargetParseError::Table(error) => map_table_error(error),
        DmTargetParseError::ResolveBacking(error) => error,
    }
}

#[cfg(ktest)]
mod tests {
    use aster_block::{
        BlockDevice, BlockDeviceMeta,
        bio::{BioEnqueueError, SubmittedBio},
    };
    use ostd::prelude::ktest;

    use super::*;

    #[derive(Debug)]
    struct Backing(DeviceId);

    impl BlockDevice for Backing {
        fn enqueue(&self, _bio: SubmittedBio) -> core::result::Result<(), BioEnqueueError> {
            unreachable!()
        }

        fn metadata(&self) -> BlockDeviceMeta {
            BlockDeviceMeta {
                max_nr_segments_per_bio: 8,
                nr_sectors: 1024,
            }
        }

        fn name(&self) -> &str {
            "load-test-backing"
        }
        fn id(&self) -> DeviceId {
            self.0
        }
    }

    fn buffer(name: &str, backing: DeviceId) -> Vec<u8> {
        let mut bytes = vec![0u8; DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE + 32];
        bytes[OFF_NAME..OFF_NAME + name.len()].copy_from_slice(name.as_bytes());
        write_u32(&mut bytes, OFF_DATA_START, DM_IOCTL_HEADER_SIZE as u32);
        write_u32(&mut bytes, OFF_TARGET_COUNT, 1);
        let spec = DM_IOCTL_HEADER_SIZE;
        write_u64(&mut bytes, spec, 0);
        write_u64(&mut bytes, spec + 8, 8);
        bytes[spec + 24..spec + 30].copy_from_slice(b"linear");
        let params = alloc::format!("{}:{} 0", backing.major().get(), backing.minor().get());
        let start = spec + DM_TARGET_SPEC_SIZE;
        bytes[start..start + params.len()].copy_from_slice(params.as_bytes());
        bytes
    }

    #[ktest]
    fn versions_report_only_supported_target_and_short_buffer() {
        let needed = (16 + "linear".len() + 1 + 7) & !7;
        for size in [
            FIXED_PREFIX_SIZE,
            DM_IOCTL_HEADER_SIZE,
            DM_IOCTL_HEADER_SIZE + needed - 1,
        ] {
            let mut short = vec![0u8; size];
            list_versions(&mut short).unwrap();
            assert_eq!(
                read_u32(&short, OFF_FLAGS) & BUFFER_FULL_FLAG,
                BUFFER_FULL_FLAG
            );
            assert_eq!(read_u32(&short, OFF_DATA_SIZE), DM_IOCTL_HEADER_SIZE as u32);
        }
        let mut exact = vec![0xa5; DM_IOCTL_HEADER_SIZE + needed];
        write_u32(&mut exact, OFF_FLAGS, 0);
        list_versions(&mut exact).unwrap();
        assert_eq!(read_u32(&exact, OFF_FLAGS) & BUFFER_FULL_FLAG, 0);
        let start = DM_IOCTL_HEADER_SIZE;
        assert_eq!(read_u32(&exact, OFF_DATA_SIZE) as usize, exact.len());
        assert_eq!(read_u32(&exact, start), 0);
        assert_eq!(
            (0..3)
                .map(|i| read_u32(&exact, start + 4 + i * 4))
                .collect::<Vec<_>>(),
            [1, 4, 0]
        );
        assert_eq!(&exact[start + 16..start + 23], b"linear\0");
        assert!(exact[start + 23..].iter().all(|byte| *byte == 0));
    }

    #[ktest]
    fn empty_device_list_and_exact_record_boundaries() {
        let mut empty = vec![0xa5; DM_IOCTL_HEADER_SIZE + 8];
        write_u32(&mut empty, OFF_FLAGS, 0);
        encode_device_list(&mut empty, &[], false).unwrap();
        assert_eq!(read_u32(&empty, OFF_DATA_SIZE), DM_IOCTL_HEADER_SIZE as u32);
        assert_eq!(&empty[DM_IOCTL_HEADER_SIZE..], &[0; 8]);
        let manager = aster_dm::DmManager::new().unwrap();
        let device = manager.create("query-exact".into(), None, None).unwrap();
        let record_len = (12 + "query-exact".len() + 1 + 7) & !7;
        let record_len = (record_len + 8 + 7) & !7;
        let devices = [device.clone()];
        let mut short = vec![0; DM_IOCTL_HEADER_SIZE + record_len - 1];
        encode_device_list(&mut short, &devices, false).unwrap();
        assert_ne!(read_u32(&short, OFF_FLAGS) & BUFFER_FULL_FLAG, 0);
        let mut exact = vec![0; DM_IOCTL_HEADER_SIZE + record_len];
        encode_device_list(&mut exact, &devices, false).unwrap();
        assert_eq!(read_u32(&exact, OFF_FLAGS) & BUFFER_FULL_FLAG, 0);
        assert_eq!(read_u32(&exact, OFF_DATA_SIZE) as usize, exact.len());
        assert_eq!(read_u32(&exact, DM_IOCTL_HEADER_SIZE + 8), 0);
        drop(devices);
        manager.remove("query-exact").unwrap();
    }

    #[ktest]
    fn query_header_reports_no_primary_as_zero_opens() {
        let manager = aster_dm::DmManager::new().unwrap();
        let device = manager
            .create("query-no-primary".into(), None, None)
            .unwrap();
        let mut header = vec![0; FIXED_PREFIX_SIZE];
        query_header(&mut header, &device, None, None, None);
        assert_eq!(block::open_count(device.id()), None);
        assert_eq!(read_u32(&header, OFF_OPEN_COUNT), 0);
        assert_eq!(read_u32(&header, OFF_TARGET_COUNT), 0);
        manager.remove("query-no-primary").unwrap();
    }

    #[ktest]
    fn queries_select_active_and_inactive_tables_without_fallback() {
        let manager = aster_dm::DmManager::new().unwrap();
        let device = manager
            .create("query-table-test".into(), None, None)
            .unwrap();
        let backing = DeviceId::new(MajorId::new(510), MinorId::new(164));
        aster_block::register(Arc::new(Backing(backing))).unwrap();
        let mut status = vec![0u8; DM_IOCTL_HEADER_SIZE + 128];
        write_u32(&mut status, OFF_DATA_START, DM_IOCTL_HEADER_SIZE as u32);
        let (active, inactive) = device.table_snapshot();
        query_header(
            &mut status,
            &device,
            active.as_ref(),
            inactive.as_ref(),
            active.as_ref(),
        );
        assert_eq!(read_u32(&status, OFF_TARGET_COUNT), 0);
        let mut request = buffer("query-table-test", backing);
        table_load_for_device_with_primary(&mut request, &device, |_| Ok(())).unwrap();
        let (active, inactive) = device.table_snapshot();
        query_header(
            &mut status,
            &device,
            active.as_ref(),
            inactive.as_ref(),
            active.as_ref(),
        );
        assert_eq!(read_u32(&status, OFF_TARGET_COUNT), 0);
        assert_eq!(
            read_u32(&status, OFF_FLAGS) & INACTIVE_PRESENT_FLAG,
            INACTIVE_PRESENT_FLAG
        );
        let table = selected_table(&active, &inactive, true);
        query_header(
            &mut status,
            &device,
            active.as_ref(),
            inactive.as_ref(),
            table,
        );
        assert_eq!(read_u32(&status, OFF_TARGET_COUNT), 1);
        assert_eq!(
            table.unwrap().targets()[0]
                .status_params(TargetStatusMode::Status)
                .unwrap(),
            ""
        );
        assert_eq!(
            table.unwrap().targets()[0]
                .status_params(TargetStatusMode::Table)
                .unwrap(),
            "510:164 0"
        );
        drop(active);
        drop(inactive);
        device.resume_initial().unwrap();
        let (active, inactive) = device.table_snapshot();
        assert!(selected_table(&active, &inactive, true).is_none());
        assert!(selected_table(&active, &inactive, false).is_some());
        drop(status);
        drop(request);
        drop(active);
        drop(inactive);
        manager.remove("query-table-test").unwrap();
        drop(device);
        aster_block::unregister(backing).unwrap();
    }

    #[ktest]
    fn list_devices_encodes_records_and_buffer_full() {
        let manager = aster_dm::DmManager::new().unwrap();
        let first = manager.create("query-list-one".into(), None, None).unwrap();
        let second = manager
            .create(
                "query-list-two".into(),
                Some(b"query-list-uuid".to_vec()),
                None,
            )
            .unwrap();
        let mut short = vec![0u8; DM_IOCTL_HEADER_SIZE + 8];
        let mut long = vec![0u8; DM_IOCTL_HEADER_SIZE + 256];
        let devices = [first.clone(), second.clone()];
        encode_device_list(&mut short, &devices, true).unwrap();
        assert_ne!(read_u32(&short, OFF_FLAGS) & BUFFER_FULL_FLAG, 0);
        encode_device_list(&mut long, &devices, true).unwrap();
        assert_eq!(read_u32(&long, OFF_FLAGS) & BUFFER_FULL_FLAG, 0);
        let start = read_u32(&long, OFF_DATA_START) as usize;
        assert_eq!(read_u64(&long, start), first.id().as_encoded_u64());
        let next = read_u32(&long, start + 8) as usize;
        assert!(next > 0);
        assert_eq!(&long[start + 12..start + 26], b"query-list-one");
        assert_eq!(read_u64(&long, start + next), second.id().as_encoded_u64());
        assert_eq!(read_u32(&long, start + next + 8), 0);
        assert_eq!(read_u32(&long, start + next + 36), 1);
        assert_eq!(
            &long[start + next + 40..start + next + 56],
            b"query-list-uuid\0"
        );
        assert_eq!(read_u32(&long, OFF_DATA_SIZE) as usize, start + next + 56);
        drop(devices);
        drop(first);
        drop(second);
        manager.remove("query-list-one").unwrap();
        manager.remove("query-list-two").unwrap();
    }

    #[ktest]
    fn table_records_report_short_buffer_without_truncating_target() {
        let manager = aster_dm::DmManager::new().unwrap();
        let device = manager
            .create("query-short-test".into(), None, None)
            .unwrap();
        let backing = DeviceId::new(MajorId::new(510), MinorId::new(165));
        aster_block::register(Arc::new(Backing(backing))).unwrap();
        let mut request = buffer("query-short-test", backing);
        table_load_for_device_with_primary(&mut request, &device, |_| Ok(())).unwrap();
        let table = device.inactive_table().unwrap();
        let mut short = vec![0u8; DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE];
        let mut long = vec![0u8; DM_IOCTL_HEADER_SIZE + 128];
        encode_table_records(&mut short, &table, TargetStatusMode::Table).unwrap();
        assert_ne!(read_u32(&short, OFF_FLAGS) & BUFFER_FULL_FLAG, 0);
        assert_eq!(read_u32(&short, OFF_DATA_SIZE), DM_IOCTL_HEADER_SIZE as u32);
        encode_table_records(&mut long, &table, TargetStatusMode::Table).unwrap();
        assert_eq!(read_u32(&long, OFF_FLAGS) & BUFFER_FULL_FLAG, 0);
        assert_eq!(read_u64(&long, DM_IOCTL_HEADER_SIZE + 8), 8);
        assert_eq!(
            &long[DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE
                ..DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE + 9],
            b"510:165 0"
        );
        drop(table);
        drop(request);
        manager.remove("query-short-test").unwrap();
        drop(device);
        aster_block::unregister(backing).unwrap();
    }

    #[ktest]
    fn load_validates_before_publication_and_keeps_old_table_on_failure() {
        let manager = aster_dm::DmManager::new().unwrap();
        let device = manager
            .create("load-parse-test".into(), None, None)
            .unwrap();
        let backing = DeviceId::new(MajorId::new(510), MinorId::new(156));
        aster_block::register(Arc::new(Backing(backing))).unwrap();
        let mut request = buffer("load-parse-test", backing);
        let mut publications = 0;
        table_load_for_device_with_primary(&mut request, &device, |_| {
            publications += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(publications, 1);
        let old = device.inactive_table().unwrap();
        assert_eq!(old.target_count(), 1);
        assert!(!old.is_readonly());
        let mut invalid = buffer("load-parse-test", backing);
        write_u32(&mut invalid, OFF_TARGET_COUNT, 0);
        assert_eq!(
            table_load_for_device_with_primary(&mut invalid, &device, |_| {
                publications += 1;
                Ok(())
            })
            .unwrap_err()
            .error(),
            Errno::EINVAL
        );
        assert_eq!(publications, 1);
        assert!(Arc::ptr_eq(&old, &device.inactive_table().unwrap()));
        assert_eq!(device.inactive_table().unwrap().length(), old.length());
        assert!(!device.inactive_table().unwrap().is_readonly());
        assert!(device.active_table().is_none());
        let mut replacement = buffer("load-parse-test", backing);
        write_u32(&mut replacement, OFF_FLAGS, READONLY_FLAG);
        table_load_for_device_with_primary(&mut replacement, &device, |_| Ok(())).unwrap();
        assert!(device.inactive_table().unwrap().is_readonly());
        assert_eq!(
            read_u32(&replacement, OFF_FLAGS) & INACTIVE_PRESENT_FLAG,
            INACTIVE_PRESENT_FLAG
        );
        assert_eq!(read_u32(&replacement, OFF_TARGET_COUNT), 1);
        drop(replacement);
        drop(invalid);
        drop(request);
        drop(old);
        manager.remove("load-parse-test").unwrap();
        drop(device);
        aster_block::unregister(backing).unwrap();
    }

    #[ktest]
    fn load_parses_two_linear_targets_and_rejects_overlap() {
        let first = DeviceId::new(MajorId::new(510), MinorId::new(158));
        let second = DeviceId::new(MajorId::new(510), MinorId::new(159));
        aster_block::register(Arc::new(Backing(first))).unwrap();
        aster_block::register(Arc::new(Backing(second))).unwrap();
        let mut request = vec![0u8; DM_IOCTL_HEADER_SIZE + 2 * 64];
        write_u32(&mut request, OFF_DATA_START, DM_IOCTL_HEADER_SIZE as u32);
        write_u32(&mut request, OFF_TARGET_COUNT, 2);
        for (index, id) in [first, second].into_iter().enumerate() {
            let spec = DM_IOCTL_HEADER_SIZE + index * 64;
            write_u64(&mut request, spec, index as u64 * 8);
            write_u64(&mut request, spec + 8, 8);
            write_u32(&mut request, spec + 20, 64);
            request[spec + 24..spec + 30].copy_from_slice(b"linear");
            let params = alloc::format!("{}:{} 0", id.major().get(), id.minor().get());
            request[spec + DM_TARGET_SPEC_SIZE..spec + DM_TARGET_SPEC_SIZE + params.len()]
                .copy_from_slice(params.as_bytes());
        }
        let table = parse_table_load_request(&request).unwrap();
        assert_eq!(table.target_count(), 2);
        assert_eq!(table.length(), 16);
        drop(table);
        write_u32(&mut request, DM_IOCTL_HEADER_SIZE + 20, 40);
        assert_eq!(
            parse_table_load_request(&request).unwrap_err().error(),
            Errno::EINVAL
        );
        aster_block::unregister(first).unwrap();
        aster_block::unregister(second).unwrap();
    }

    #[ktest]
    fn first_resume_reports_active_table_and_preserves_failed_transitions() {
        let manager = aster_dm::DmManager::new().unwrap();
        let device = manager
            .create("first-resume-test".into(), None, None)
            .unwrap();
        let backing = DeviceId::new(MajorId::new(510), MinorId::new(161));
        aster_block::register(Arc::new(Backing(backing))).unwrap();
        let mut resume = vec![0u8; FIXED_PREFIX_SIZE];
        resume[OFF_NAME..OFF_NAME + 17].copy_from_slice(b"first-resume-test");
        assert_eq!(
            resume_device_for(&mut resume, &device).unwrap_err().error(),
            Errno::EINVAL
        );
        let mut request = buffer("first-resume-test", backing);
        write_u32(&mut request, OFF_FLAGS, READONLY_FLAG);
        table_load_for_device_with_primary(&mut request, &device, |_| Ok(())).unwrap();
        assert_eq!(
            read_u32(&request, OFF_FLAGS) & INACTIVE_PRESENT_FLAG,
            INACTIVE_PRESENT_FLAG
        );
        write_u32(&mut resume, OFF_FLAGS, SUSPEND_FLAG);
        assert_eq!(
            resume_device_for(&mut resume, &device).unwrap_err().error(),
            Errno::EINVAL
        );
        assert!(device.active_table().is_none());
        assert!(device.inactive_table().is_some());
        write_u32(&mut resume, OFF_FLAGS, 0);
        resume_device_for(&mut resume, &device).unwrap();
        assert_eq!(read_u32(&resume, OFF_TARGET_COUNT), 1);
        assert_eq!(
            read_u32(&resume, OFF_FLAGS) & (ACTIVE_PRESENT_FLAG | READONLY_FLAG),
            ACTIVE_PRESENT_FLAG | READONLY_FLAG
        );
        assert_eq!(read_u32(&resume, OFF_FLAGS) & INACTIVE_PRESENT_FLAG, 0);
        write_u32(&mut resume, OFF_FLAGS, SUSPEND_FLAG | NOFLUSH_FLAG);
        assert_eq!(
            resume_device_for(&mut resume, &device).unwrap_err().error(),
            Errno::EINVAL
        );
        assert!(!device.is_suspended());
        write_u32(&mut resume, OFF_FLAGS, SUSPEND_FLAG);
        resume_device_for(&mut resume, &device).unwrap();
        assert_eq!(read_u32(&resume, OFF_FLAGS) & SUSPEND_FLAG, SUSPEND_FLAG);
        let mut status = vec![0u8; FIXED_PREFIX_SIZE];
        let (active, inactive) = device.table_snapshot();
        query_header(
            &mut status,
            &device,
            active.as_ref(),
            inactive.as_ref(),
            active.as_ref(),
        );
        assert_eq!(read_u32(&status, OFF_FLAGS) & SUSPEND_FLAG, SUSPEND_FLAG);
        write_u32(&mut resume, OFF_FLAGS, 0);
        resume_device_for(&mut resume, &device).unwrap();
        assert_eq!(read_u32(&resume, OFF_FLAGS) & SUSPEND_FLAG, 0);
        resume_device_for(&mut resume, &device).unwrap();
        let mut replacement = buffer("first-resume-test", backing);
        table_load_for_device_with_primary(&mut replacement, &device, |_| Ok(())).unwrap();
        assert_eq!(
            read_u32(&replacement, OFF_FLAGS) & (ACTIVE_PRESENT_FLAG | INACTIVE_PRESENT_FLAG),
            ACTIVE_PRESENT_FLAG | INACTIVE_PRESENT_FLAG
        );
        assert_eq!(read_u32(&replacement, OFF_FLAGS) & READONLY_FLAG, 0);
        write_u32(&mut resume, OFF_FLAGS, SUSPEND_FLAG);
        resume_device_for(&mut resume, &device).unwrap();
        assert!(device.is_suspended());
        write_u32(&mut resume, OFF_FLAGS, 0);
        assert_eq!(
            resume_device_for(&mut resume, &device).unwrap_err().error(),
            Errno::EINVAL
        );
        assert!(device.is_suspended());
        assert!(device.inactive_table().is_some());
        assert!(device.active_table().unwrap().is_readonly());
        assert!(!device.inactive_table().unwrap().is_readonly());
        // The status snapshot holds a table (and its backing lease) independently
        // of the mapper; release it before unregistering the backing device.
        drop(active);
        drop(inactive);
        drop(replacement);
        drop(request);
        manager.remove("first-resume-test").unwrap();
        drop(device);
        aster_block::unregister(backing).unwrap();
    }

    #[ktest]
    fn first_load_publishes_dm_primary_without_alias() {
        crate::fs::devtmpfs::init_for_ktest();
        let manager = aster_dm::DmManager::new().unwrap();
        let device = manager
            .create("load-primary-test".into(), None, None)
            .unwrap();
        let backing = DeviceId::new(MajorId::new(510), MinorId::new(160));
        aster_block::register(Arc::new(Backing(backing))).unwrap();
        let mut request = buffer("load-primary-test", backing);
        table_load_for_device_with_primary(&mut request, &device, runtime::ensure_primary).unwrap();
        assert!(crate::fs::devtmpfs::lookup_path_for_ktest(device.primary_name()).is_ok());
        assert!(crate::fs::devtmpfs::lookup_path_for_ktest("mapper/load-primary-test").is_err());
        assert_eq!(device.inactive_table().unwrap().target_count(), 1);
        block::unregister_runtime_primary(device.id()).unwrap();
        manager.remove("load-primary-test").unwrap();
        drop(device);
        aster_block::unregister(backing).unwrap();
    }

    #[ktest]
    fn load_publication_failure_preserves_inactive_table() {
        let manager = aster_dm::DmManager::new().unwrap();
        let device = manager
            .create("load-publish-test".into(), None, None)
            .unwrap();
        let backing = DeviceId::new(MajorId::new(510), MinorId::new(157));
        aster_block::register(Arc::new(Backing(backing))).unwrap();
        let mut request = buffer("load-publish-test", backing);
        let error = table_load_for_device_with_primary(&mut request, &device, |_| {
            Err(Error::new(Errno::EIO))
        })
        .unwrap_err();
        assert_eq!(error.error(), Errno::EIO);
        assert!(device.inactive_table().is_none());
        drop(request);
        manager.remove("load-publish-test").unwrap();
        drop(device);
        aster_block::unregister(backing).unwrap();
    }
}
fn map_create_error(error: DmError) -> Error {
    match error {
        DmError::NameExists | DmError::UuidExists => {
            Error::with_message(Errno::EBUSY, "mapper identity already exists")
        }
        DmError::MinorBusy | DmError::MinorExhausted => {
            Error::with_message(Errno::ENXIO, "cannot allocate mapper device number")
        }
        _ => Error::with_message(Errno::EINVAL, "cannot create mapper"),
    }
}
