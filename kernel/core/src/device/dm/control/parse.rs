// SPDX-License-Identifier: MPL-2.0

//! Parses DM ioctl mapping configuration into validated candidate tables.
//!
//! This is not LVM on-disk metadata parsing. Backing lookup and lease acquisition
//! are allowed here; device publication and table installation are not.

use aster_block::id::Sid;
use aster_dm::{
    DmTable, TableError,
    target::{DmTargetParseError, parse_target_with},
};
use device_id::{DeviceId, MajorId, MinorId};

use super::abi::*;
use crate::{
    fs::{file::InodeType, vfs::path::FsPath},
    prelude::*,
};

/// Builds a fully validated inactive table before any runtime publication.
pub(super) fn parse_table_load_request(buffer: &[u8]) -> Result<Arc<DmTable>> {
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

pub(super) fn map_table_error(error: TableError) -> Error {
    match error {
        TableError::OutOfMemory => {
            Error::with_message(Errno::ENOMEM, "cannot allocate mapper target")
        }
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
    use ostd::prelude::ktest;

    use super::{super::test_support::*, *};

    #[ktest]
    fn target_allocation_failure_reports_out_of_memory() {
        assert_eq!(
            map_table_error(TableError::OutOfMemory).error(),
            Errno::ENOMEM
        );
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
}
