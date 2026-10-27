// SPDX-License-Identifier: MPL-2.0

//! Linux DM command decoding, mapper selection, and response encoding.
//!
//! Commands translate copied ABI buffers into typed workflow requests. Mutation
//! admission retains the lifecycle guard through response encoding; WAIT retains
//! its lookup reference across removal without reapplying mutation admission.
//! Backing paths are resolved in the calling process's VFS context.

use core::ops::Deref;

use aster_block::{BlockDevice, BlockDeviceLease, id::Sid, lookup_lease};
use aster_device_mapper::{
    DmControlRef, DmDevice, DmManager, DmTable,
    target::{TargetStatusMode, parse_target_with},
};
use device_id::{DeviceId, MajorId, MinorId};

use super::{
    DeviceIdLogLabel,
    abi::{
        DM_ACTIVE_PRESENT_FLAG, DM_DEFERRED_REMOVE, DM_DEV_CREATE_CMD, DM_DEV_REMOVE_CMD,
        DM_DEV_RENAME_CMD, DM_DEV_STATUS_CMD, DM_DEV_SUSPEND_CMD, DM_DEV_WAIT_CMD,
        DM_GET_TARGET_VERSION_CMD, DM_INACTIVE_PRESENT_FLAG, DM_INTERNAL_SUSPEND_FLAG,
        DM_IOCTL_FIXED_PREFIX_SIZE, DM_LIST_DEVICES_CMD, DM_LIST_VERSIONS_CMD, DM_NAME_LEN,
        DM_NAME_LIST_FLAG_DOESNT_HAVE_UUID, DM_NAME_LIST_FLAG_HAS_UUID, DM_NOFLUSH_FLAG,
        DM_PERSISTENT_DEV_FLAG, DM_QUERY_INACTIVE_TABLE_FLAG, DM_READONLY_FLAG, DM_REMOVE_ALL_CMD,
        DM_STATUS_TABLE_FLAG, DM_SUSPEND_FLAG, DM_TABLE_CLEAR_CMD, DM_TABLE_DEPS_CMD,
        DM_TABLE_LOAD_CMD, DM_TABLE_STATUS_CMD, DM_TARGET_SPEC_SIZE, DM_TARGET_TYPE_LEN,
        DM_UUID_FLAG, DM_UUID_LEN, DM_VERSION_CMD, OFF_DATA_SIZE, OFF_DATA_START, OFF_DEV,
        OFF_EVENT_NR, OFF_FLAGS, OFF_NAME, OFF_OPEN_COUNT, OFF_TARGET_COUNT, OFF_UUID, align_up,
        available_from, c_bytes_until, c_string_until, data_start, decode_ioctl_device,
        fixed_bytes, invalid_buffer, map_table_error, map_target_parse_error,
        name_list_record_layout, optional_c_string, optional_uuid, read_u32, read_u64,
        require_range, required_c_string, result_start, set_buffer_full, table_status_record_len,
        validate_target_spec_next, write_c_string, write_c_string_fixed, write_u32, write_u64,
    },
    manager,
    runtime::MapperRuntimeCoordinator,
    workflow::{
        ControlWorkflow, CreateRequest, DeviceSnapshot, LifecycleOutcome, LifecycleRequest,
        QueryWorkflow, RemoveAllRequest, RenameRequest, TableLoadRequest, TargetVersionSnapshot,
    },
};
use crate::{
    device::block_open_count,
    fs::{file::InodeType, vfs::path::FsPath},
    prelude::*,
};

/// Dispatches one validated Device Mapper ioctl command.
///
/// Per-mapper lifecycle mutations acquire `DmDevice::lock_lifecycle` inside
/// their handlers. This keeps unrelated mapper control commands independent
/// while a suspend or table replacement waits for assigned BIOs to drain.
pub(super) fn handle_command(command: u8, buffer: &mut [u8]) -> Result<()> {
    match command {
        DM_VERSION_CMD => Ok(()),
        DM_LIST_VERSIONS_CMD => list_versions(buffer),
        DM_GET_TARGET_VERSION_CMD => get_target_version(buffer),
        DM_DEV_CREATE_CMD => create_device(buffer),
        DM_DEV_REMOVE_CMD => remove_device(buffer),
        DM_REMOVE_ALL_CMD => remove_all(buffer),
        DM_DEV_STATUS_CMD => device_status(buffer),
        DM_DEV_WAIT_CMD => unreachable!("DM_DEV_WAIT must not hold the control lock while waiting"),
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

/// Creates a mapped device and writes its assigned identity back into the ioctl header.
pub(super) fn create_device(buffer: &mut [u8]) -> Result<()> {
    let name = required_c_string(buffer, OFF_NAME, DM_NAME_LEN)?;
    let uuid = optional_uuid(buffer, OFF_UUID, DM_UUID_LEN)?;
    let flags = read_u32(buffer, OFF_FLAGS)?;
    let requested_minor = if flags & DM_PERSISTENT_DEV_FLAG != 0 {
        let dev = decode_ioctl_device(read_u64(buffer, OFF_DEV)?);
        Some(dev.minor().get())
    } else {
        None
    };

    validate_mapper_name(&name)?;
    let request = CreateRequest {
        name,
        uuid,
        requested_minor,
    };
    let device = ControlWorkflow::new(manager()).create(request)?;
    ostd::error!(
        "[dm-debug] control create committed name={} dev={} requested_minor={:?}",
        device.mapper_name(),
        DeviceIdLogLabel(device.id()),
        requested_minor
    );
    fill_device_header(buffer, &device)
}

/// Removes one mapped device after unregistering its runtime block-device alias.
fn remove_device(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_device_allow_isolated(buffer)?;
    let workflow = ControlWorkflow::new(manager());
    with_current_device_lifecycle_allow_isolated(&device, |device| {
        // Linux find_device fills identity and the pre-removal inactive flag;
        // dev_remove does not call __dev_status or synthesize a final event.
        write_c_string_fixed(buffer, OFF_NAME, DM_NAME_LEN, device.mapper_name())?;
        write_c_string_fixed(
            buffer,
            OFF_UUID,
            DM_UUID_LEN,
            device.uuid().as_deref().unwrap_or(b""),
        )?;
        let mut flags =
            read_u32(buffer, OFF_FLAGS)? & !(DM_INACTIVE_PRESENT_FLAG | DM_DEFERRED_REMOVE);
        if device.inactive_table().is_some() {
            flags |= DM_INACTIVE_PRESENT_FLAG;
        }
        write_u32(buffer, OFF_FLAGS, flags)?;
        workflow.remove(device)?;
        ostd::error!(
            "[dm-debug] control remove committed name={} dev={}",
            device.mapper_name(),
            DeviceIdLogLabel(device.id())
        );
        Ok(())
    })?;
    // WAIT and mutations admitted before detach may need the lifecycle lock.
    // Never retain it while waiting for their Linux-style control references.
    device.drain_removed();
    Ok(())
}

/// Implements Linux-style best-effort removal across all mapped devices.
pub(super) fn remove_all(buffer: &mut [u8]) -> Result<()> {
    let workflow = ControlWorkflow::new(manager());
    let request = RemoveAllRequest::new(manager().devices());
    let outcome = workflow.remove_all(request, |device| {
        with_current_device_lifecycle_allow_isolated(device, |device| workflow.remove(device))?;
        device.drain_removed();
        Ok(())
    });
    ostd::error!(
        "[dm-debug] control remove-all committed attempted={} removed={}",
        outcome.attempted,
        outcome.removed
    );
    write_u32(buffer, OFF_DATA_SIZE, 0)
}

pub(super) fn device_status(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_device(buffer)?;
    device_status_for_device(buffer, &device)
}

/// Fills status fields that depend on the selected active or inactive table.
pub(super) fn device_status_for_device(buffer: &mut [u8], device: &DmDevice) -> Result<()> {
    let flags = read_u32(buffer, OFF_FLAGS)?;
    let snapshot = QueryWorkflow::table_metadata(device, flags & DM_QUERY_INACTIVE_TABLE_FLAG != 0);
    fill_device_header_snapshot(buffer, &snapshot.device)?;
    write_u32(buffer, OFF_TARGET_COUNT, snapshot.target_count as u32)
}

/// Waits on the lookup reference, which remains valid after manager detachment.
pub(super) fn device_wait(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_device(buffer)?;
    device_wait_with_pending(buffer, &device, || {}).map_err(restart_interrupted_wait)
}

pub(super) fn restart_interrupted_wait(error: Error) -> Error {
    match error.error() {
        Errno::EINTR => Error::new(Errno::ERESTARTSYS),
        _ => error,
    }
}

#[cfg(ktest)]
pub(super) fn device_wait_for_device(buffer: &mut [u8], device: &DmDevice) -> Result<()> {
    device_wait_with_pending(buffer, device, || {})
}

/// Waits for a changed event and encodes the retained table under lifecycle exclusion.
///
/// The pending hook lets tests observe condition checks without replacing the
/// real signal-aware wait queue. Production uses a no-op hook. The final snapshot
/// is serialized with removal, but must not reapply current-instance admission.
pub(super) fn device_wait_with_pending(
    buffer: &mut [u8],
    device: &DmDevice,
    mut pending: impl FnMut(),
) -> Result<()> {
    let event_nr = read_u32(buffer, OFF_EVENT_NR)?;
    device.event_queue().pause_until(|| {
        if device.status().event_nr != event_nr {
            Some(())
        } else {
            pending();
            None
        }
    })?;
    let _lifecycle = device.lock_lifecycle();
    table_status_for_device(buffer, device)
}

/// Renames a mapped device or updates its UUID according to `DM_UUID_FLAG`.
pub(super) fn device_rename(buffer: &mut [u8]) -> Result<()> {
    let flags = read_u32(buffer, OFF_FLAGS)?;
    let data_start = read_u32(buffer, OFF_DATA_START)? as usize;
    if data_start < DM_IOCTL_FIXED_PREFIX_SIZE {
        return Err(invalid_buffer());
    }
    let value = c_bytes_until(buffer, data_start, buffer.len())?;
    let request = if flags & DM_UUID_FLAG != 0 {
        if value.is_empty() || value.len() >= DM_UUID_LEN {
            return_errno_with_message!(Errno::EINVAL, "invalid new mapper UUID length");
        }
        RenameRequest::Uuid(value)
    } else {
        let name = c_string_until(buffer, data_start, buffer.len())?;
        validate_mapper_name(name)?;
        RenameRequest::Name(name)
    };
    // dm_hash_rename checks the destination before resolving the source by
    // name. Unlike ordinary lookups, rename does not select by dev or UUID.
    let destination = match request {
        RenameRequest::Uuid(uuid) => manager().lookup_uuid(uuid),
        RenameRequest::Name(name) => manager().lookup_name(name),
    };
    if destination.is_some() {
        return_errno_with_message!(Errno::EBUSY, "mapper rename destination already exists");
    }
    let name = optional_c_string(buffer, OFF_NAME, DM_NAME_LEN)?;
    let device = name
        .and_then(|name| manager().lookup_name(&name))
        .ok_or_else(|| Error::with_message(Errno::ENXIO, "mapper rename source does not exist"))?;
    let device = manager()
        .acquire_control(&device)
        .ok_or_else(|| Error::new(Errno::ENXIO))?;
    let workflow = ControlWorkflow::new(manager());

    with_current_device_lifecycle(&device, |device| {
        let request = if flags & DM_UUID_FLAG != 0 {
            RenameRequest::Uuid(c_bytes_until(buffer, data_start, buffer.len())?)
        } else {
            RenameRequest::Name(c_string_until(buffer, data_start, buffer.len())?)
        };
        workflow.rename(device, &request)?;
        let kind = request.kind();
        ostd::error!(
            "[dm-debug] control rename committed kind={} name={} dev={}",
            kind,
            device.mapper_name(),
            DeviceIdLogLabel(device.id())
        );
        fill_device_header(buffer, device)
    })
}

fn table_load(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_device(buffer)?;
    let target_count = read_u32(buffer, OFF_TARGET_COUNT)?;
    let readonly = read_u32(buffer, OFF_FLAGS)? & DM_READONLY_FLAG != 0;
    with_current_device_lifecycle(&device, |device| {
        table_load_for_device(buffer, device)?;
        ostd::error!(
            "[dm-debug] control table-load committed name={} dev={} targets={} readonly={} primary_registered={}",
            device.mapper_name(),
            DeviceIdLogLabel(device.id()),
            target_count,
            readonly,
            MapperRuntimeCoordinator::new(manager()).is_registered(device)
        );
        Ok(())
    })
}

/// Parses `DM_TABLE_LOAD` target specs before the typed workflow publishes runtime state.
fn table_load_for_device(buffer: &mut [u8], device: &CurrentDevice<'_>) -> Result<()> {
    let request = parse_table_load_request(buffer)?;
    ControlWorkflow::new(manager()).load_table(device, request)?;
    fill_device_header(buffer, device)
}

/// Parses and validates a table before handing its runtime commit to the workflow.
#[cfg(ktest)]
pub(super) fn table_load_for_device_with_primary<F>(
    buffer: &mut [u8],
    device: &Arc<DmDevice>,
    register_primary: F,
) -> Result<()>
where
    F: FnOnce(&Arc<DmDevice>) -> Result<()>,
{
    let request = parse_table_load_request(buffer)?;
    let guarded = CurrentDevice {
        device,
        _guard: device.lock_lifecycle(),
    };
    ControlWorkflow::load_table_with_primary(&guarded, request, register_primary)?;
    fill_device_header(buffer, device)
}

/// Decodes `DM_TABLE_LOAD` target specs into a fully validated workflow request.
pub(super) fn parse_table_load_request(buffer: &[u8]) -> Result<TableLoadRequest> {
    parse_table_load_request_with(buffer, parse_backing_device_id)
}

/// Keeps path resolution injectable for kernel-thread tests without a VFS context.
/// Production resolves names in the calling process; lookup errors propagate
/// before any runtime publication or inactive-table replacement.
pub(super) fn parse_table_load_request_with(
    buffer: &[u8],
    mut parse_backing: impl FnMut(&str) -> Result<DeviceId>,
) -> Result<TableLoadRequest> {
    let target_count = read_u32(buffer, OFF_TARGET_COUNT)?;
    let flags = read_u32(buffer, OFF_FLAGS)?;

    if target_count == 0 {
        return_errno_with_message!(Errno::EINVAL, "映射表至少需要一个 target");
    }

    let mut cursor = data_start(buffer)?;
    let mut targets = Vec::new();
    let mut resolve_backing = lookup_backing_device;
    for index in 0..target_count {
        let spec_end = cursor
            .checked_add(DM_TARGET_SPEC_SIZE)
            .ok_or_else(invalid_buffer)?;
        require_range(buffer, cursor, DM_TARGET_SPEC_SIZE)?;

        let logical_start = read_u64(buffer, cursor)?;
        let length = read_u64(buffer, cursor + 8)?;
        let next = read_u32(buffer, cursor + 20)? as usize;
        let target_type = c_string_until(buffer, cursor + 24, cursor + DM_TARGET_SPEC_SIZE)?;
        // Linux parses the current target before consuming its next offset.
        // Search the remaining input for NUL, then reject overlap when advancing.
        let params = c_string_until(buffer, spec_end, buffer.len())?;
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
            if next < DM_TARGET_SPEC_SIZE + params.len() + 1 {
                return_errno_with_message!(
                    Errno::EINVAL,
                    "next target overlaps current parameters"
                );
            }
            cursor = validate_target_spec_next(cursor, next, buffer.len())?;
        }
    }

    let readonly = flags & DM_READONLY_FLAG != 0;
    let table =
        Arc::new(DmTable::new_targets_with_readonly(targets, readonly).map_err(map_table_error)?);
    Ok(TableLoadRequest::new(table))
}

fn device_suspend(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_device(buffer)?;
    let flags = read_u32(buffer, OFF_FLAGS)?;
    let request = if flags & DM_SUSPEND_FLAG != 0 {
        LifecycleRequest::Suspend {
            noflush: flags & DM_NOFLUSH_FLAG != 0,
        }
    } else {
        LifecycleRequest::Resume
    };
    let workflow = ControlWorkflow::new(manager());

    with_current_device_lifecycle(&device, |device| {
        match workflow.transition(device, request)? {
            LifecycleOutcome::Suspended { noflush } => ostd::error!(
                "[dm-debug] control suspend committed name={} dev={} noflush={}",
                device.mapper_name(),
                DeviceIdLogLabel(device.id()),
                noflush
            ),
            LifecycleOutcome::Resumed { initial: true } => ostd::error!(
                "[dm-debug] control resume committed name={} dev={} initial=true alias_published=true",
                device.mapper_name(),
                DeviceIdLogLabel(device.id())
            ),
            LifecycleOutcome::Resumed { initial: false } => ostd::error!(
                "[dm-debug] control resume committed name={} dev={} initial=false",
                device.mapper_name(),
                DeviceIdLogLabel(device.id())
            ),
        }
        fill_device_header(buffer, device)
    })
}

fn table_clear(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_device(buffer)?;
    let workflow = ControlWorkflow::new(manager());
    with_current_device_lifecycle(&device, |device| {
        workflow.clear_inactive_table(device)?;
        fill_device_header(buffer, device)
    })
}

pub(super) fn table_deps(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_device(buffer)?;
    table_deps_for_device(buffer, &device)
}

pub(super) fn table_deps_for_device(buffer: &mut [u8], device: &DmDevice) -> Result<()> {
    let flags = read_u32(buffer, OFF_FLAGS)?;
    let snapshot = QueryWorkflow::table_deps(device, flags & DM_QUERY_INACTIVE_TABLE_FLAG != 0);
    fill_device_header_snapshot(buffer, &snapshot.device)?;
    if snapshot.device.target_count == 0 {
        return Ok(());
    }
    let start = result_start(buffer)?;

    let deps_len = 8usize
        .checked_add(
            snapshot
                .backing_ids
                .len()
                .checked_mul(8)
                .ok_or_else(invalid_buffer)?,
        )
        .ok_or_else(invalid_buffer)?;
    if available_from(buffer, start) < deps_len {
        set_buffer_full(buffer)?;
        return Ok(());
    }
    write_u32(buffer, start, snapshot.backing_ids.len() as u32)?;
    write_u32(buffer, start + 4, 0)?;
    for (index, id) in snapshot.backing_ids.into_iter().enumerate() {
        write_u64(buffer, start + 8 + index * 8, id.as_encoded_u64())?;
    }
    write_u32(buffer, OFF_DATA_SIZE, start + deps_len)
}

fn table_status(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_device(buffer)?;
    table_status_for_device(buffer, &device)
}

/// Emits Linux `DM_TABLE_STATUS` records for active or inactive table selection flags.
pub(super) fn table_status_for_device(buffer: &mut [u8], device: &DmDevice) -> Result<()> {
    let flags = read_u32(buffer, OFF_FLAGS)?;
    let mode = if flags & DM_STATUS_TABLE_FLAG != 0 {
        TargetStatusMode::Table
    } else {
        TargetStatusMode::Status
    };
    let mut records =
        QueryWorkflow::table_records(device, flags & DM_QUERY_INACTIVE_TABLE_FLAG != 0, mode);
    fill_device_header_snapshot(buffer, records.device())?;
    write_u32(buffer, OFF_TARGET_COUNT, records.target_count() as u32)?;
    if records.target_count() == 0 {
        return Ok(());
    }
    let start = result_start(buffer)?;
    let mut cursor = start;
    for _ in 0..records.target_count() {
        // Linux does not invoke a target's status callback unless the output
        // has space beyond the fixed spec, even when that target emits no text.
        if available_from(buffer, cursor) <= DM_TARGET_SPEC_SIZE {
            set_buffer_full(buffer)?;
            break;
        }
        let target = records.next_target()?.unwrap();
        let record_len = table_status_record_len(target.params.len())?;
        let used = DM_TARGET_SPEC_SIZE + target.params.len() + 1;
        // Linux treats a string that exactly fills the remaining space as
        // truncated. Its returned length excludes final alignment padding.
        if available_from(buffer, cursor) <= used {
            set_buffer_full(buffer)?;
            return Ok(());
        }
        // Padding between returned records must not expose copied input bytes.
        let end = (cursor + record_len).min(buffer.len());
        buffer[cursor..end].fill(0);
        write_u64(buffer, cursor, target.logical_start)?;
        write_u64(buffer, cursor + 8, target.length)?;
        write_u32(buffer, cursor + 16, 0)?;
        write_u32(buffer, cursor + 20, cursor + record_len - start)?;
        write_c_string_fixed(buffer, cursor + 24, DM_TARGET_TYPE_LEN, target.name)?;
        write_c_string(buffer, cursor + DM_TARGET_SPEC_SIZE, &target.params)?;
        write_u32(buffer, OFF_DATA_SIZE, cursor + used)?;
        cursor += record_len;
    }
    Ok(())
}

/// Computes the aligned ABI record length for one target-version entry.
fn target_version_record_len(target: &TargetVersionSnapshot) -> Result<usize> {
    align_up(16 + target.name.len() + 1, 8)
}

/// Writes one target-version record into the variable ioctl output area.
fn write_target_version(
    buffer: &mut [u8],
    offset: usize,
    next: usize,
    target: &TargetVersionSnapshot,
) -> Result<()> {
    let record_len = target_version_record_len(target)?;
    require_range(buffer, offset, record_len)?;
    write_u32(buffer, offset, next)?;
    for (index, value) in target.version.into_iter().enumerate() {
        write_u32(buffer, offset + 4 + index * 4, value)?;
    }
    write_c_string(buffer, offset + 16, target.name)
}

/// Logs one target-version record after its Linux `next` field is final.
fn log_target_version_record(
    operation: &str,
    offset: usize,
    next: usize,
    target: &TargetVersionSnapshot,
    record_len: usize,
) {
    ostd::error!(
        "[dm-debug] {} record offset_bytes={} next_bytes={} version={}.{}.{} name={} record_len_bytes={}",
        operation,
        offset,
        next,
        target.version[0],
        target.version[1],
        target.version[2],
        target.name,
        record_len
    );
}

/// Lists supported versions only when the complete conservative size fits.
pub(super) fn list_versions(buffer: &mut [u8]) -> Result<()> {
    encode_target_versions(buffer, &QueryWorkflow::target_versions())
}

/// Uses Linux's unaligned per-record size plus seven bytes of slack for sizing.
///
/// The reported size is this conservative bound, not the final aligned cursor.
/// Zeroing the whole result prevents unused input parameters being copied back.
fn encode_target_versions(buffer: &mut [u8], targets: &[TargetVersionSnapshot]) -> Result<()> {
    let start = result_start(buffer)?;
    let needed = targets.iter().try_fold(0usize, |total, target| {
        total
            .checked_add(16 + target.name.len() + 1 + 7)
            .ok_or_else(invalid_buffer)
    })?;
    if available_from(buffer, start) < needed {
        return set_buffer_full(buffer);
    }
    buffer[start..start + needed].fill(0);
    let mut cursor = start;
    for (index, target) in targets.iter().enumerate() {
        let record_len = target_version_record_len(target)?;
        let next = if index + 1 == targets.len() {
            0
        } else {
            record_len
        };
        write_target_version(buffer, cursor, next, target)?;
        log_target_version_record("target-versions", cursor, next, target, record_len);
        cursor += record_len;
    }
    write_u32(buffer, OFF_DATA_SIZE, start + needed)
}

/// Looks up one target-version entry by Linux target type name.
pub(super) fn get_target_version(buffer: &mut [u8]) -> Result<()> {
    let name = required_c_string(buffer, OFF_NAME, DM_NAME_LEN)?;
    let target = QueryWorkflow::target_version(&name)
        .ok_or_else(|| Error::with_message(Errno::EINVAL, "未知的 Device Mapper target"))?;
    encode_target_versions(buffer, &[target])
}

fn list_devices(buffer: &mut [u8]) -> Result<()> {
    let flags = read_u32(buffer, OFF_FLAGS)?;
    let include_uuid = flags & DM_UUID_FLAG != 0;
    let devices = manager()
        .devices()
        .into_iter()
        .filter_map(|device| manager().acquire_control(&device))
        .filter(|device| !aster_block::is_removing(device.id()))
        .map(|device| QueryWorkflow::device(&device));
    list_devices_for_snapshots(buffer, devices, include_uuid)
}

/// Lists mapped device names and UUID extensions in Linux `DM_LIST_DEVICES` format.
#[cfg(ktest)]
pub(super) fn list_devices_for_devices<I>(buffer: &mut [u8], devices: I) -> Result<()>
where
    I: IntoIterator<Item = Arc<DmDevice>>,
{
    list_devices_for_snapshots(buffer, QueryWorkflow::devices(devices), true)
}

/// Encodes Linux name-list records with event fields regardless of UUID mode.
///
/// Both filters are bounded byte prefixes, not necessarily terminated UTF-8.
/// Like Linux `list_devices`, capacity is checked for the whole filtered list;
/// empty output still requires room for a dev sentinel but reports data_start.
pub(super) fn list_devices_for_snapshots(
    buffer: &mut [u8],
    devices: impl IntoIterator<Item = DeviceSnapshot>,
    include_uuid: bool,
) -> Result<()> {
    let name_prefix = fixed_bytes(buffer, OFF_NAME, DM_NAME_LEN)?;
    let uuid_prefix = fixed_bytes(buffer, OFF_UUID, DM_UUID_LEN)?;
    let devices: Vec<_> = devices
        .into_iter()
        .filter(|device| {
            device.name.as_bytes().starts_with(name_prefix)
                && device
                    .uuid
                    .as_deref()
                    .unwrap_or(b"")
                    .starts_with(uuid_prefix)
        })
        .collect();
    let needed = devices.iter().try_fold(0usize, |total, device| {
        let uuid_len = if include_uuid {
            device.uuid.as_ref().map_or(0, |uuid| uuid.len() + 1)
        } else {
            0
        };
        let (_, len) = name_list_record_layout(device.name.len() + 1, uuid_len)?;
        total.checked_add(len).ok_or_else(invalid_buffer)
    })?;
    let start = result_start(buffer)?;
    if available_from(buffer, start) < needed.max(8) {
        return set_buffer_full(buffer);
    }
    buffer[start..start + needed.max(8)].fill(0);
    let mut cursor = start;
    let mut previous = None;
    for device in devices {
        let uuid_len = if include_uuid {
            device.uuid.as_ref().map_or(0, |uuid| uuid.len() + 1)
        } else {
            0
        };
        let (extension_offset, record_len) =
            name_list_record_layout(device.name.len() + 1, uuid_len)?;
        if let Some(previous) = previous {
            write_u32(buffer, previous + 8, cursor - previous)?;
        }
        write_u64(buffer, cursor, device.id.as_encoded_u64())?;
        write_u32(buffer, cursor + 8, 0)?;
        write_c_string(buffer, cursor + 12, &device.name)?;
        let extension = cursor + extension_offset;
        write_u32(buffer, extension, device.status.event_nr)?;
        let flags = if !include_uuid {
            0
        } else if device.uuid.is_some() {
            DM_NAME_LIST_FLAG_HAS_UUID
        } else {
            DM_NAME_LIST_FLAG_DOESNT_HAVE_UUID
        };
        write_u32(buffer, extension + 4, flags)?;
        if include_uuid && let Some(uuid) = device.uuid {
            write_c_string(buffer, extension + 8, &uuid)?;
        }
        previous = Some(cursor);
        cursor += record_len;
    }
    write_u32(buffer, OFF_DATA_SIZE, start + needed)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DeviceLookupPolicy {
    RejectIsolated,
    AllowIsolated,
}

fn lookup_device(buffer: &[u8]) -> Result<DmControlRef> {
    let device = lookup_device_with_policy(buffer, DeviceLookupPolicy::RejectIsolated)?;
    manager()
        .acquire_control(&device)
        .ok_or_else(|| Error::new(Errno::ENXIO))
}

fn lookup_device_allow_isolated(buffer: &[u8]) -> Result<Arc<DmDevice>> {
    lookup_device_with_policy(buffer, DeviceLookupPolicy::AllowIsolated)
}

fn lookup_device_with_policy(buffer: &[u8], policy: DeviceLookupPolicy) -> Result<Arc<DmDevice>> {
    lookup_device_in_manager_with_policy(buffer, manager(), policy)
}

#[cfg(ktest)]
pub(super) fn lookup_device_in_manager(
    buffer: &[u8],
    manager: &DmManager,
) -> Result<Arc<DmDevice>> {
    lookup_device_in_manager_with_policy(buffer, manager, DeviceLookupPolicy::RejectIsolated)
}

fn lookup_device_in_manager_with_policy(
    buffer: &[u8],
    manager: &DmManager,
    policy: DeviceLookupPolicy,
) -> Result<Arc<DmDevice>> {
    let raw_dev = read_u64(buffer, OFF_DEV)?;
    let name = optional_c_string(buffer, OFF_NAME, DM_NAME_LEN)?;
    let uuid = optional_uuid(buffer, OFF_UUID, DM_UUID_LEN)?;

    // validate_params rejects name+UUID first; lookup rejects a textual
    // selector combined with dev. Linux returns ENXIO for lookup ambiguity.
    if name.is_some() && uuid.is_some() {
        return_errno_with_message!(
            Errno::EINVAL,
            "name and UUID selectors are mutually exclusive"
        );
    }
    if raw_dev != 0 && (name.is_some() || uuid.is_some()) {
        return_errno_with_message!(Errno::ENXIO, "ambiguous mapper selector");
    }
    let device = if let Some(uuid) = uuid {
        manager.lookup_uuid(&uuid)
    } else if let Some(name) = name {
        manager.lookup_name(&name)
    } else if raw_dev != 0 {
        manager.lookup_id(decode_ioctl_device(raw_dev))
    } else {
        return_errno_with_message!(Errno::ENXIO, "missing mapper selector");
    };

    let device =
        device.ok_or_else(|| Error::with_message(Errno::ENXIO, "Device Mapper 设备不存在"))?;
    if policy == DeviceLookupPolicy::RejectIsolated {
        // Keep this exact instance's minor allocated while consulting the block
        // registry. A structural lookup alone can outlive removal and ID reuse.
        let _control = manager
            .acquire_control(&device)
            .ok_or_else(|| Error::new(Errno::ENXIO))?;
        if aster_block::is_removing(device.id()) {
            return_errno_with_message!(Errno::ENXIO, "Device Mapper 设备正在删除");
        }
    }
    Ok(device)
}

/// Proof that a mapper is current and its lifecycle lock remains held.
///
/// Only the checked lifecycle wrapper constructs this context. It lives through
/// response encoding, not merely through the workflow's state mutation.
pub(super) struct CurrentDevice<'a> {
    device: &'a Arc<DmDevice>,
    _guard: MutexGuard<'a, ()>,
}

impl Deref for CurrentDevice<'_> {
    type Target = Arc<DmDevice>;

    fn deref(&self) -> &Self::Target {
        self.device
    }
}

/// Runs one lifecycle mutation only if its looked-up mapper is still current.
///
/// A caller may wait on this mapper's lifecycle guard without blocking other
/// mappers. Revalidating the `Arc` after acquiring that guard prevents a stale
/// lookup from mutating a device that a concurrent remove already detached.
fn with_current_device_lifecycle<T>(
    device: &Arc<DmDevice>,
    operation: impl FnOnce(&CurrentDevice<'_>) -> Result<T>,
) -> Result<T> {
    with_current_device_lifecycle_with_policy(device, DeviceLookupPolicy::RejectIsolated, operation)
}

fn with_current_device_lifecycle_allow_isolated<T>(
    device: &Arc<DmDevice>,
    operation: impl FnOnce(&CurrentDevice<'_>) -> Result<T>,
) -> Result<T> {
    with_current_device_lifecycle_with_policy(device, DeviceLookupPolicy::AllowIsolated, operation)
}

fn with_current_device_lifecycle_with_policy<T>(
    device: &Arc<DmDevice>,
    policy: DeviceLookupPolicy,
    operation: impl FnOnce(&CurrentDevice<'_>) -> Result<T>,
) -> Result<T> {
    with_current_device_lifecycle_in_with_policy(manager(), device, policy, operation)
}

#[cfg(ktest)]
pub(super) fn with_current_device_lifecycle_in<T>(
    manager: &DmManager,
    device: &Arc<DmDevice>,
    operation: impl FnOnce(&CurrentDevice<'_>) -> Result<T>,
) -> Result<T> {
    with_current_device_lifecycle_in_with_policy(
        manager,
        device,
        DeviceLookupPolicy::RejectIsolated,
        operation,
    )
}

fn with_current_device_lifecycle_in_with_policy<T>(
    manager: &DmManager,
    device: &Arc<DmDevice>,
    policy: DeviceLookupPolicy,
    operation: impl FnOnce(&CurrentDevice<'_>) -> Result<T>,
) -> Result<T> {
    let _lifecycle = device.lock_lifecycle();
    if !manager.is_current(device) {
        return_errno_with_message!(Errno::ENXIO, "Device Mapper 设备不存在");
    }
    if policy == DeviceLookupPolicy::RejectIsolated && aster_block::is_removing(device.id()) {
        return_errno_with_message!(Errno::ENXIO, "Device Mapper 设备正在删除");
    }
    operation(&CurrentDevice {
        device,
        _guard: _lifecycle,
    })
}

pub(super) fn fill_device_header(buffer: &mut [u8], device: &DmDevice) -> Result<()> {
    let inactive = read_u32(buffer, OFF_FLAGS)? & DM_QUERY_INACTIVE_TABLE_FLAG != 0;
    fill_device_header_snapshot(
        buffer,
        &QueryWorkflow::table_metadata(device, inactive).device,
    )
}

/// Encodes ABI-owned header fields from one immutable mapper snapshot.
fn fill_device_header_snapshot(buffer: &mut [u8], device: &DeviceSnapshot) -> Result<()> {
    let mut flags = read_u32(buffer, OFF_FLAGS)?
        & !(DM_READONLY_FLAG
            | DM_SUSPEND_FLAG
            | DM_ACTIVE_PRESENT_FLAG
            | DM_INACTIVE_PRESENT_FLAG
            | DM_INTERNAL_SUSPEND_FLAG);
    if device.status.readonly {
        flags |= DM_READONLY_FLAG;
    }
    if device.status.suspended {
        flags |= DM_SUSPEND_FLAG;
    }
    if device.status.has_active_table {
        flags |= DM_ACTIVE_PRESENT_FLAG;
    }
    if device.status.has_inactive_table {
        flags |= DM_INACTIVE_PRESENT_FLAG;
    }
    write_u32(buffer, OFF_TARGET_COUNT, device.target_count)?;
    write_u32(buffer, OFF_FLAGS, flags)?;
    write_u32(buffer, OFF_EVENT_NR, device.status.event_nr)?;
    write_u64(buffer, OFF_DEV, device.id.as_encoded_u64())?;
    write_u32(
        buffer,
        OFF_OPEN_COUNT,
        block_open_count(device.id).unwrap_or(0),
    )?;
    write_c_string_fixed(buffer, OFF_NAME, DM_NAME_LEN, &device.name)?;
    write_c_string_fixed(
        buffer,
        OFF_UUID,
        DM_UUID_LEN,
        device.uuid.as_deref().unwrap_or(b""),
    )?;
    Ok(())
}

/// Resolves a previously parsed backing device ID into a live block-device lease.
fn lookup_backing_device(backing_id: DeviceId) -> Result<BlockDeviceLease> {
    lookup_lease(backing_id).ok_or_else(|| {
        ostd::warn!(
            "[dm] lookup_lease failed for {}:{}, registered devices: {:?}",
            backing_id.major().get(),
            backing_id.minor().get(),
            aster_block::list()
        );
        Error::with_message(Errno::ENODEV, "Device Mapper backing 设备不存在或正在移除")
    })
}

/// Parses a backing token from table parameters without taking the backing lease.
pub(super) fn parse_backing_device_id(dev: &str) -> Result<DeviceId> {
    parse_backing_device_id_with(dev, |dev| {
        // Relative paths use the calling process's root and current directory.
        let current_task = ostd::task::Task::current().unwrap();
        let thread_local = AsThreadLocal::as_thread_local(&current_task).unwrap();
        let fs_ref = thread_local.borrow_fs();
        let path_resolver = fs_ref.resolver().read();
        let path = path_resolver.lookup(&FsPath::try_from(dev)?)?;
        if path.type_() != InodeType::BlockDevice {
            return_errno_with_message!(Errno::ENOTBLK, "mapper backing path is not a block device");
        }
        path.metadata()?.self_dev_id.ok_or_else(|| {
            Error::with_message(Errno::ENOTBLK, "mapper backing path has no device ID")
        })
    })
}

/// Falls back to path lookup whenever the complete token is not a device ID.
pub(super) fn parse_backing_device_id_with(
    dev: &str,
    lookup_path: impl FnOnce(&str) -> Result<DeviceId>,
) -> Result<DeviceId> {
    if let Some((major, minor)) = dev.split_once(':')
        && let (Ok(major), Ok(minor)) = (major.parse::<u16>(), minor.parse::<u32>())
        && let (Ok(major), Ok(minor)) = (MajorId::try_from(major), MinorId::try_from(minor))
    {
        return Ok(DeviceId::new(major, minor));
    }
    lookup_path(dev)
}

/// Rejects names that Linux cannot publish, before any identity mutation.
///
/// The length is the byte length excluding NUL. Runtime NAME_MAX validation is
/// insufficient because the DM ABI has a smaller fixed name field.
pub(super) fn validate_mapper_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() >= DM_NAME_LEN
        || name.contains('/')
        || matches!(name, "." | ".." | "control")
    {
        return_errno_with_message!(Errno::EINVAL, "invalid mapper name");
    }
    Ok(())
}
