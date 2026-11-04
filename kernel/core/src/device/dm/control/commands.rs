// SPDX-License-Identifier: MPL-2.0

//! Dispatch of the supported Device Mapper control commands.

use aster_block::id::Sid;
use aster_dm::{
    DmDevice, DmError, DmTable, TableError,
    target::{DmTargetParseError, parse_target_with},
};
use device_id::{DeviceId, MajorId, MinorId, decode_device_numbers};

use super::{
    super::{DM_MANAGER, runtime},
    abi::*,
};
use crate::{
    fs::{file::InodeType, vfs::path::FsPath},
    prelude::*,
};

type CommandHandler = fn(&mut [u8]) -> Result<()>;

/// Selects only commands implemented by this control device.
pub(super) fn handler_for(command: u8) -> Result<CommandHandler> {
    match command {
        DM_DEV_CREATE_CMD => Ok(create_device),
        DM_TABLE_LOAD_CMD => Ok(table_load),
        _ => return_errno_with_message!(Errno::ENOTTY, "unsupported device-mapper ioctl command"),
    }
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
    fill_device_header(buffer, &device);
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

fn fill_device_header(buffer: &mut [u8], device: &DmDevice) {
    write_u32(
        buffer,
        OFF_TARGET_COUNT,
        device
            .inactive_table()
            .as_ref()
            .map_or(0, |table| table.target_count() as u32),
    );
    write_u32(buffer, OFF_OPEN_COUNT, 0);
    write_u32(buffer, OFF_EVENT_NR, 0);
    write_u64(buffer, OFF_DEV, device.id().as_encoded_u64());
    let flags = read_u32(buffer, OFF_FLAGS)
        & !(READONLY_FLAG
            | SUSPEND_FLAG
            | ACTIVE_PRESENT_FLAG
            | INACTIVE_PRESENT_FLAG
            | INTERNAL_SUSPEND_FLAG);
    let flags = if let Some(table) = device.inactive_table() {
        flags
            | INACTIVE_PRESENT_FLAG
            | if table.is_readonly() {
                READONLY_FLAG
            } else {
                0
            }
    } else {
        flags
    };
    write_u32(buffer, OFF_FLAGS, flags);
    let name = device.mapper_name();
    buffer[OFF_NAME..OFF_NAME + NAME_LEN].fill(0);
    buffer[OFF_NAME..OFF_NAME + name.len()].copy_from_slice(name.as_bytes());
    buffer[OFF_UUID..OFF_UUID + UUID_LEN].fill(0);
    if let Some(uuid) = device.uuid() {
        buffer[OFF_UUID..OFF_UUID + uuid.len()].copy_from_slice(&uuid);
    }
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
    fill_device_header(buffer, device);
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
        crate::device::registry::block::unregister_runtime_primary(device.id()).unwrap();
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
