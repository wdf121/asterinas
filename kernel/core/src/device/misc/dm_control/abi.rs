// SPDX-License-Identifier: MPL-2.0

//! Linux DM wire layout, bounded field access, validation, and errno mapping.
//!
//! Helpers operate only on copied byte slices. They neither access userspace nor
//! acquire mapper lifecycle locks or publish runtime devices.

use aster_device_mapper::{DmError, TableError, target::DmTargetParseError};

use crate::prelude::*;

pub(super) const DM_IOCTL_MAGIC: u32 = 0xfd;
// Input ends at `offsetof(dm_ioctl, data)`; output records start after the
// aligned `sizeof(dm_ioctl)`. These sizes serve different ABI contracts.
pub(super) const DM_IOCTL_FIXED_PREFIX_SIZE: usize = 305;
pub(super) const DM_IOCTL_HEADER_SIZE: usize = 312;
pub(super) const DM_IOCTL_MAX_SIZE: usize = 1024 * 1024;
#[cfg(ktest)]
pub(super) const DM_IOCTL_COMMAND_PREFIX: u32 = 0xc138_fd00;

pub(super) const DM_VERSION_CMD: u8 = 0;
pub(super) const DM_REMOVE_ALL_CMD: u8 = 1;
pub(super) const DM_LIST_DEVICES_CMD: u8 = 2;
pub(super) const DM_DEV_CREATE_CMD: u8 = 3;
pub(super) const DM_DEV_REMOVE_CMD: u8 = 4;
pub(super) const DM_DEV_RENAME_CMD: u8 = 5;
pub(super) const DM_DEV_SUSPEND_CMD: u8 = 6;
pub(super) const DM_DEV_STATUS_CMD: u8 = 7;
pub(super) const DM_DEV_WAIT_CMD: u8 = 8;
pub(super) const DM_TABLE_LOAD_CMD: u8 = 9;
pub(super) const DM_TABLE_CLEAR_CMD: u8 = 10;
pub(super) const DM_TABLE_DEPS_CMD: u8 = 11;
pub(super) const DM_TABLE_STATUS_CMD: u8 = 12;
pub(super) const DM_LIST_VERSIONS_CMD: u8 = 13;
pub(super) const DM_GET_TARGET_VERSION_CMD: u8 = 17;

pub(super) const DM_VERSION: [u32; 3] = [4, 48, 0];

pub(super) const DM_READONLY_FLAG: u32 = 1 << 0;
pub(super) const DM_SUSPEND_FLAG: u32 = 1 << 1;
pub(super) const DM_PERSISTENT_DEV_FLAG: u32 = 1 << 3;
pub(super) const DM_STATUS_TABLE_FLAG: u32 = 1 << 4;
pub(super) const DM_ACTIVE_PRESENT_FLAG: u32 = 1 << 5;
pub(super) const DM_INACTIVE_PRESENT_FLAG: u32 = 1 << 6;
pub(super) const DM_BUFFER_FULL_FLAG: u32 = 1 << 8;
#[cfg(ktest)]
pub(super) const DM_SKIP_BDGET_FLAG: u32 = 1 << 9;
#[cfg(ktest)]
pub(super) const DM_SKIP_LOCKFS_FLAG: u32 = 1 << 10;
pub(super) const DM_NOFLUSH_FLAG: u32 = 1 << 11;
pub(super) const DM_QUERY_INACTIVE_TABLE_FLAG: u32 = 1 << 12;
pub(super) const DM_UEVENT_GENERATED_FLAG: u32 = 1 << 13;
pub(super) const DM_UUID_FLAG: u32 = 1 << 14;
pub(super) const DM_SECURE_DATA_FLAG: u32 = 1 << 15;
pub(super) const DM_DATA_OUT_FLAG: u32 = 1 << 16;
pub(super) const DM_DEFERRED_REMOVE: u32 = 1 << 17;
pub(super) const DM_INTERNAL_SUSPEND_FLAG: u32 = 1 << 18;
pub(super) const DM_IMA_MEASUREMENT_FLAG: u32 = 1 << 19;

pub(super) const DM_NAME_LIST_FLAG_HAS_UUID: u32 = 1;
pub(super) const DM_NAME_LIST_FLAG_DOESNT_HAVE_UUID: u32 = 2;

const OFF_VERSION: usize = 0;
pub(super) const OFF_DATA_SIZE: usize = 12;
pub(super) const OFF_DATA_START: usize = 16;
pub(super) const OFF_TARGET_COUNT: usize = 20;
pub(super) const OFF_OPEN_COUNT: usize = 24;
pub(super) const OFF_FLAGS: usize = 28;
pub(super) const OFF_EVENT_NR: usize = 32;
pub(super) const OFF_DEV: usize = 40;
pub(super) const OFF_NAME: usize = 48;
pub(super) const DM_NAME_LEN: usize = 128;
pub(super) const OFF_UUID: usize = 176;
pub(super) const DM_UUID_LEN: usize = 129;

pub(super) const DM_TARGET_SPEC_SIZE: usize = 40;
pub(super) const DM_TARGET_TYPE_LEN: usize = 16;

/// Bounds allocations independently of the command's data offset.
///
/// The 1 MiB ceiling is a local resource policy, not Linux's 1 GiB limit.
pub(super) fn validate_ioctl_size(data_size: usize) -> Result<()> {
    if !(DM_IOCTL_FIXED_PREFIX_SIZE..=DM_IOCTL_MAX_SIZE).contains(&data_size) {
        return_errno_with_message!(Errno::EINVAL, "dm_ioctl.data_size exceeds supported bounds");
    }
    Ok(())
}

/// Validates the aligned table-load envelope, not header-only or output commands.
pub(super) fn validate_ioctl_buffer_layout(data_size: usize, data_start: usize) -> Result<()> {
    validate_ioctl_size(data_size)?;
    if data_start < DM_IOCTL_HEADER_SIZE || data_start > data_size || !data_start.is_multiple_of(8)
    {
        return_errno_with_message!(Errno::EINVAL, "dm_ioctl.data_start 无效或未按 8 字节对齐");
    }
    Ok(())
}

/// Decodes the Linux ioctl number and rejects command IDs outside this DM subset.
pub(super) fn decode_command(raw: u32) -> Result<u8> {
    if (raw >> 8) & 0xff != DM_IOCTL_MAGIC {
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
        | DM_DEV_WAIT_CMD
        | DM_TABLE_LOAD_CMD
        | DM_TABLE_CLEAR_CMD
        | DM_TABLE_DEPS_CMD
        | DM_TABLE_STATUS_CMD
        | DM_LIST_VERSIONS_CMD
        | DM_GET_TARGET_VERSION_CMD => Ok(command),
        _ => return_errno_with_message!(Errno::ENOTTY, "尚未支持该 Device Mapper ioctl 命令"),
    }
}

/// Returns a stable Linux Device Mapper command name for learning diagnostics.
pub(super) fn command_name(command: u8) -> &'static str {
    match command {
        DM_VERSION_CMD => "DM_VERSION",
        DM_REMOVE_ALL_CMD => "DM_REMOVE_ALL",
        DM_LIST_DEVICES_CMD => "DM_LIST_DEVICES",
        DM_DEV_CREATE_CMD => "DM_DEV_CREATE",
        DM_DEV_REMOVE_CMD => "DM_DEV_REMOVE",
        DM_DEV_RENAME_CMD => "DM_DEV_RENAME",
        DM_DEV_SUSPEND_CMD => "DM_DEV_SUSPEND",
        DM_DEV_STATUS_CMD => "DM_DEV_STATUS",
        DM_DEV_WAIT_CMD => "DM_DEV_WAIT",
        DM_TABLE_LOAD_CMD => "DM_TABLE_LOAD",
        DM_TABLE_CLEAR_CMD => "DM_TABLE_CLEAR",
        DM_TABLE_DEPS_CMD => "DM_TABLE_DEPS",
        DM_TABLE_STATUS_CMD => "DM_TABLE_STATUS",
        DM_LIST_VERSIONS_CMD => "DM_LIST_VERSIONS",
        DM_GET_TARGET_VERSION_CMD => "DM_GET_TARGET_VERSION",
        _ => "DM_UNKNOWN",
    }
}

/// Accepts the same major and a minor no newer than this implementation's ABI.
///
/// Linux v6.18.56 `check_version` treats minor revisions as backward compatible
/// and ignores patch differences. The caller must report the kernel version to
/// userspace even when this check rejects the client's requested version.
pub(super) fn validate_client_version(buffer: &[u8]) -> Result<()> {
    if read_u32(buffer, OFF_VERSION)? != DM_VERSION[0]
        || read_u32(buffer, OFF_VERSION + 4)? > DM_VERSION[1]
    {
        return_errno_with_message!(
            Errno::EINVAL,
            "incompatible device-mapper interface version"
        );
    }
    Ok(())
}

/// Normalizes flags and fixed strings before any command interprets parameters.
///
/// Matches Linux `validate_params`: list filters are bounded byte prefixes,
/// while other commands receive terminated identity fields. Unsupported active
/// features are rejected by command, rather than rejecting every ignored bit.
pub(super) fn validate_ioctl_params(command: u8, buffer: &mut [u8]) -> Result<()> {
    let flags = read_u32(buffer, OFF_FLAGS)?
        & !(DM_BUFFER_FULL_FLAG
            | DM_UEVENT_GENERATED_FLAG
            | DM_SECURE_DATA_FLAG
            | DM_DATA_OUT_FLAG);
    write_u32(buffer, OFF_FLAGS, flags)?;
    if matches!(
        command,
        DM_REMOVE_ALL_CMD | DM_LIST_DEVICES_CMD | DM_LIST_VERSIONS_CMD
    ) {
        return validate_input_flags(command, buffer);
    }
    if command == DM_DEV_CREATE_CMD {
        if buffer[OFF_NAME] == 0 {
            return_errno_with_message!(Errno::EINVAL, "missing mapper name");
        }
    } else if buffer[OFF_NAME] != 0 && buffer[OFF_UUID] != 0 {
        return_errno_with_message!(
            Errno::EINVAL,
            "name and UUID selectors are mutually exclusive"
        );
    }
    buffer[OFF_NAME + DM_NAME_LEN - 1] = 0;
    buffer[OFF_UUID + DM_UUID_LEN - 1] = 0;
    validate_input_flags(command, buffer)
}

/// Rejects unsupported features only on commands that would act on them.
pub(super) fn validate_input_flags(command: u8, buffer: &[u8]) -> Result<()> {
    let flags = read_u32(buffer, OFF_FLAGS)?;
    if flags & DM_DEFERRED_REMOVE != 0 && matches!(command, DM_DEV_REMOVE_CMD | DM_REMOVE_ALL_CMD) {
        return_errno_with_message!(Errno::EOPNOTSUPP, "deferred remove is not supported");
    }
    if flags & DM_IMA_MEASUREMENT_FLAG != 0
        && matches!(command, DM_TABLE_STATUS_CMD | DM_DEV_WAIT_CMD)
    {
        return_errno_with_message!(Errno::EOPNOTSUPP, "IMA measurement output is not supported");
    }
    Ok(())
}

/// Reads a bounded byte field for list prefix filtering without allocating.
pub(super) fn fixed_bytes(buffer: &[u8], offset: usize, len: usize) -> Result<&[u8]> {
    require_range(buffer, offset, len)?;
    let bytes = &buffer[offset..offset + len];
    Ok(&bytes[..bytes.iter().position(|byte| *byte == 0).unwrap_or(len)])
}

/// Validates the offset to a subsequent spec; the final spec's next is ignored.
pub(super) fn validate_target_spec_next(
    data_start: usize,
    next: usize,
    buffer_len: usize,
) -> Result<usize> {
    if next < DM_TARGET_SPEC_SIZE || !next.is_multiple_of(8) {
        return_errno_with_message!(Errno::EINVAL, "dm_target_spec.next 无效");
    }
    let next_spec = data_start.checked_add(next).ok_or_else(invalid_buffer)?;
    if next_spec > buffer_len {
        return_errno_with_message!(Errno::EINVAL, "dm_target_spec.next 超出缓冲区范围");
    }
    Ok(next_spec)
}

/// Computes the aligned ABI record length for one table/status target entry.
pub(super) fn table_status_record_len(params_len: usize) -> Result<usize> {
    let unaligned = DM_TARGET_SPEC_SIZE
        .checked_add(params_len)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(invalid_buffer)?;
    align_up(unaligned, 8)
}

/// Computes aligned offsets for one `DM_LIST_DEVICES` name-list record.
pub(super) fn name_list_record_layout(name_len: usize, uuid_len: usize) -> Result<(usize, usize)> {
    let extension_offset = align_up(12usize.checked_add(name_len).ok_or_else(invalid_buffer)?, 8)?;
    let record_end = extension_offset
        .checked_add(8)
        .and_then(|value| value.checked_add(uuid_len))
        .ok_or_else(invalid_buffer)?;
    Ok((extension_offset, align_up(record_end, 8)?))
}

/// Selects Linux's aligned result area independently of the input data_start.
pub(super) fn result_start(buffer: &mut [u8]) -> Result<usize> {
    // The seven padding bytes between the fixed prefix and result records were
    // copied from userspace. Never return that input padding in secure output.
    let end = buffer.len().min(DM_IOCTL_HEADER_SIZE);
    buffer[DM_IOCTL_FIXED_PREFIX_SIZE..end].fill(0);
    write_u32(buffer, OFF_DATA_START, DM_IOCTL_HEADER_SIZE)?;
    Ok(DM_IOCTL_HEADER_SIZE)
}

/// Reads and validates the variable data offset from a `dm_ioctl` buffer.
pub(super) fn data_start(buffer: &[u8]) -> Result<usize> {
    let start = read_u32(buffer, OFF_DATA_START)? as usize;
    validate_ioctl_buffer_layout(buffer.len(), start)?;
    Ok(start)
}

pub(super) fn available_from(buffer: &[u8], start: usize) -> usize {
    buffer.len().saturating_sub(start)
}

pub(super) fn align_up(value: usize, alignment: usize) -> Result<usize> {
    value
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
        .ok_or_else(invalid_buffer)
}

/// Sets `DM_BUFFER_FULL_FLAG` when the caller-provided output area is too small.
pub(super) fn set_buffer_full(buffer: &mut [u8]) -> Result<()> {
    let flags = read_u32(buffer, OFF_FLAGS)? | DM_BUFFER_FULL_FLAG;
    write_u32(buffer, OFF_FLAGS, flags)
}

pub(super) fn write_version(buffer: &mut [u8]) -> Result<()> {
    for (index, value) in DM_VERSION.into_iter().enumerate() {
        write_u32(buffer, OFF_VERSION + index * 4, value)?;
    }
    Ok(())
}

/// Reads a required fixed-size NUL-terminated string from an ABI field.
pub(super) fn required_c_string(buffer: &[u8], offset: usize, len: usize) -> Result<String> {
    optional_c_string(buffer, offset, len)?
        .ok_or_else(|| Error::with_message(Errno::EINVAL, "required mapper field is empty"))
}

/// Reads an optional fixed-size NUL-terminated string from an ABI field.
pub(super) fn optional_c_string(
    buffer: &[u8],
    offset: usize,
    len: usize,
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

/// Reads a variable-length NUL-terminated parameter string between two offsets.
pub(super) fn c_string_until(buffer: &[u8], start: usize, end: usize) -> Result<&str> {
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
    Ok(value)
}

/// Writes and zero-pads a fixed-size NUL-terminated ABI string field.
pub(super) fn write_c_string_fixed(
    buffer: &mut [u8],
    offset: usize,
    len: usize,
    value: &str,
) -> Result<()> {
    if value.len() >= len {
        return_errno_with_message!(Errno::EINVAL, "输出字符串超过 ABI 字段长度");
    }
    require_range(buffer, offset, len)?;
    buffer[offset..offset + len].fill(0);
    buffer[offset..offset + value.len()].copy_from_slice(value.as_bytes());
    Ok(())
}

/// Writes a variable-length NUL-terminated ABI string.
pub(super) fn write_c_string(buffer: &mut [u8], offset: usize, value: &str) -> Result<()> {
    let len = value.len().checked_add(1).ok_or_else(invalid_buffer)?;
    require_range(buffer, offset, len)?;
    buffer[offset..offset + value.len()].copy_from_slice(value.as_bytes());
    buffer[offset + value.len()] = 0;
    Ok(())
}

/// Verifies an ABI field range is fully contained in the copied buffer.
pub(super) fn require_range(buffer: &[u8], offset: usize, len: usize) -> Result<()> {
    let end = offset.checked_add(len).ok_or_else(invalid_buffer)?;
    if end > buffer.len() {
        return Err(invalid_buffer());
    }
    Ok(())
}

/// Reads one little-endian `u32` from an aligned or unaligned ABI offset.
pub(super) fn read_u32(buffer: &[u8], offset: usize) -> Result<u32> {
    require_range(buffer, offset, 4)?;
    Ok(u32::from_le_bytes(
        buffer[offset..offset + 4].try_into().unwrap(),
    ))
}

/// Reads one little-endian `u64` from an aligned or unaligned ABI offset.
pub(super) fn read_u64(buffer: &[u8], offset: usize) -> Result<u64> {
    require_range(buffer, offset, 8)?;
    Ok(u64::from_le_bytes(
        buffer[offset..offset + 8].try_into().unwrap(),
    ))
}

/// Writes one little-endian `u32` value after range and conversion checks.
pub(super) fn write_u32(buffer: &mut [u8], offset: usize, value: impl TryInto<u32>) -> Result<()> {
    let value = value
        .try_into()
        .map_err(|_| Error::with_message(Errno::EINVAL, "数值无法写入 u32 ABI 字段"))?;
    require_range(buffer, offset, 4)?;
    buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

/// Writes one little-endian `u64` value into the copied ABI buffer.
pub(super) fn write_u64(buffer: &mut [u8], offset: usize, value: u64) -> Result<()> {
    require_range(buffer, offset, 8)?;
    buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

pub(super) fn invalid_buffer() -> Error {
    Error::with_message(Errno::EINVAL, "Device Mapper ioctl 缓冲区布局无效")
}

/// Preserves the allocation-error boundary of Linux `dev_create`/`dm_create`.
/// Device-number allocation failures become ENXIO before identity insertion;
/// name and UUID collisions still report EEXIST.
pub(super) fn map_create_error(error: DmError) -> Error {
    match error {
        DmError::MinorBusy | DmError::MinorExhausted => {
            Error::with_message(Errno::ENXIO, "cannot allocate mapper device number")
        }
        other => map_dm_error(other),
    }
}

/// Maps rename destination conflicts separately from create-time collisions.
pub(super) fn map_rename_error(error: DmError) -> Error {
    match error {
        DmError::NameExists | DmError::UuidExists => {
            Error::with_message(Errno::EBUSY, "mapper rename destination already exists")
        }
        other => map_dm_error(other),
    }
}

pub(super) fn map_dm_error(error: DmError) -> Error {
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
        DmError::Interrupted => Error::new(Errno::ERESTARTSYS),
        DmError::InvalidTable(error) => map_table_error(error),
    }
}

pub(super) fn map_target_parse_error(error: DmTargetParseError<Error>) -> Error {
    match error {
        DmTargetParseError::UnsupportedTarget => {
            Error::with_message(Errno::EINVAL, "不支持的 target 类型")
        }
        DmTargetParseError::Table(error) => map_table_error(error),
        DmTargetParseError::ResolveBacking(error) => error,
    }
}

pub(super) fn map_table_error(error: TableError) -> Error {
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
