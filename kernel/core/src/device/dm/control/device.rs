// SPDX-License-Identifier: MPL-2.0

//! Mapper device commands, identity selection, and status response encoding.

use aster_dm::{DmDevice, DmError, DmTable};
use device_id::{DeviceId, MajorId, MinorId, decode_device_numbers};

use super::{
    super::{DM_MANAGER, runtime},
    abi::*,
    table::selected_table,
};
use crate::{device::registry::block, prelude::*};

pub(super) fn ensure_control_live(device: &DmDevice) -> Result<()> {
    if device.removal_isolated() {
        return_errno_with_message!(Errno::ENXIO, "mapper removal is in progress");
    }
    Ok(())
}

/// Removes a mapper only after runtime unregistration succeeds. A partially
/// unregistered wrapper stays indexed and reserved for a cleanup-only retry.
pub(super) fn remove_device(buffer: &mut [u8]) -> Result<()> {
    if read_u32(buffer, OFF_FLAGS) & DEFERRED_REMOVE_FLAG != 0 {
        return_errno_with_message!(Errno::EINVAL, "deferred mapper removal is not supported");
    }
    let device = lookup_load_device(buffer)?;
    let mut lifecycle = device.lock_lifecycle();
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
            lifecycle.isolate_removal();
        }
        return Err(error);
    }
    lifecycle.isolate_removal();
    lifecycle.stop_and_drain_io();
    fill_device_header(buffer, &device);
    lifecycle
        .clear_removed_tables()
        .map_err(|_| Error::new(Errno::EINVAL))?;
    DM_MANAGER
        .get()
        .unwrap()
        .remove(&device.mapper_name())
        .map_err(|_| Error::new(Errno::ENXIO))?;
    lifecycle
        .drain_removed()
        .map_err(|_| Error::new(Errno::EINVAL))?;
    Ok(())
}

/// Creates a mapped device and writes its assigned identity back into the ioctl header.
pub(super) fn create_device(buffer: &mut [u8]) -> Result<()> {
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
    fill_device_header(buffer, &device);
    Ok(())
}

pub(super) fn required_name(buffer: &[u8]) -> Result<String> {
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

pub(super) fn fill_device_header(buffer: &mut [u8], device: &DmDevice) {
    let (active, inactive) = device.table_snapshot();
    let selected = selected_table(
        &active,
        &inactive,
        read_u32(buffer, OFF_FLAGS) & QUERY_INACTIVE_TABLE_FLAG != 0,
    );
    query_header(buffer, device, active.as_ref(), inactive.as_ref(), selected);
}

pub(super) fn query_header(
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
    // Report zero opens when there is no published primary device.
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

pub(super) fn device_status(buffer: &mut [u8]) -> Result<()> {
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

pub(super) fn list_devices(buffer: &mut [u8]) -> Result<()> {
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
pub(super) fn lookup_load_device(buffer: &[u8]) -> Result<Arc<DmDevice>> {
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

#[cfg(ktest)]
mod tests {
    use ostd::prelude::ktest;

    use super::*;

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
}
