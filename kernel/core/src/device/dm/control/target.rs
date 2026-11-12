// SPDX-License-Identifier: MPL-2.0

//! Control-side discovery and version encoding for registered target types.
//!
//! Registration and instance creation remain in the lower-level DM component.

use aster_dm::target::reported_target_metadata;

use super::{abi::*, device::required_name};
use crate::prelude::*;

/// Reports versions of registered target types.
pub(super) fn list_versions(buffer: &mut [u8]) -> Result<()> {
    encode_target_versions(buffer, &reported_target_metadata())
}

/// Reports the version of one advertised target by its Linux-visible name.
pub(super) fn get_target_version(buffer: &mut [u8]) -> Result<()> {
    let name = required_name(buffer)?;
    let metadata = reported_target_metadata();
    let target = metadata
        .iter()
        .find(|target| target.name() == name)
        .ok_or_else(|| Error::with_message(Errno::EINVAL, "unknown mapper target"))?;
    encode_target_versions(buffer, core::slice::from_ref(target))
}

fn encode_target_versions(
    buffer: &mut [u8],
    targets: &[aster_dm::target::DmTargetMetadata],
) -> Result<()> {
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

#[cfg(ktest)]
mod tests {
    use ostd::prelude::ktest;

    use super::*;

    #[ktest]
    fn versions_report_linear_and_striped_and_short_buffer() {
        let linear_len = (16 + "linear".len() + 1 + 7) & !7;
        let striped_len = (16 + "striped".len() + 1 + 7) & !7;
        let needed = linear_len + striped_len;
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
        assert_eq!(read_u32(&exact, start), linear_len as u32);
        assert_eq!(read_u32(&exact, start + linear_len), 0);
        for (offset, name, version) in [
            (0, b"linear\0".as_slice(), [1, 4, 0]),
            (linear_len, b"striped\0".as_slice(), [1, 6, 0]),
        ] {
            assert_eq!(
                (0..3)
                    .map(|i| read_u32(&exact, start + offset + 4 + i * 4))
                    .collect::<Vec<_>>(),
                version
            );
            assert_eq!(
                &exact[start + offset + 16..start + offset + 16 + name.len()],
                name
            );
        }
        assert!(
            exact[start + linear_len + 16 + "striped".len() + 1..]
                .iter()
                .all(|byte| *byte == 0)
        );
    }

    #[ktest]
    fn get_target_version_filters_name_and_rejects_unknown_targets() {
        let mut request = vec![0xa5; DM_IOCTL_HEADER_SIZE + 24];
        write_u32(&mut request, OFF_FLAGS, 0);
        request[OFF_NAME..OFF_NAME + NAME_LEN].fill(0);
        request[OFF_NAME..OFF_NAME + 7].copy_from_slice(b"striped");
        get_target_version(&mut request).unwrap();
        assert_eq!(read_u32(&request, OFF_FLAGS) & BUFFER_FULL_FLAG, 0);
        assert_eq!(
            read_u32(&request, OFF_DATA_SIZE),
            (DM_IOCTL_HEADER_SIZE + 24) as u32
        );
        assert_eq!(read_u32(&request, DM_IOCTL_HEADER_SIZE), 0);
        assert_eq!(read_u32(&request, DM_IOCTL_HEADER_SIZE + 4), 1);
        assert_eq!(read_u32(&request, DM_IOCTL_HEADER_SIZE + 8), 6);
        assert_eq!(read_u32(&request, DM_IOCTL_HEADER_SIZE + 12), 0);
        assert_eq!(
            &request[DM_IOCTL_HEADER_SIZE + 16..DM_IOCTL_HEADER_SIZE + 24],
            b"striped\0"
        );
        let mut short = vec![0; DM_IOCTL_HEADER_SIZE + 23];
        short[OFF_NAME..OFF_NAME + 7].copy_from_slice(b"striped");
        get_target_version(&mut short).unwrap();
        assert_ne!(read_u32(&short, OFF_FLAGS) & BUFFER_FULL_FLAG, 0);
        assert_eq!(read_u32(&short, OFF_DATA_SIZE), DM_IOCTL_HEADER_SIZE as u32);
        short[OFF_NAME..OFF_NAME + 7].copy_from_slice(b"unknown");
        assert_eq!(
            get_target_version(&mut short).unwrap_err().error(),
            Errno::EINVAL
        );
    }
}
