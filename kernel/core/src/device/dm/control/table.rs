// SPDX-License-Identifier: MPL-2.0

//! Table commands, suspend/resume orchestration, and table response encoding.
//!
//! Device state transitions remain in `DmDevice`; candidate construction stays in
//! the configuration parser and precedes publication and inactive installation.

use aster_dm::{DmDevice, DmTable, target::TargetStatusMode};

use super::{
    super::runtime,
    abi::*,
    device::{ensure_control_live, fill_device_header, lookup_load_device, query_header},
    parse::{map_table_error, parse_table_load_request},
};
use crate::prelude::*;

pub(super) fn selected_table<'a>(
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

pub(super) fn table_deps(buffer: &mut [u8]) -> Result<()> {
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
    let Some(table) = table else {
        return Ok(());
    };
    encode_table_deps(buffer, table)
}

fn encode_table_deps(buffer: &mut [u8], table: &DmTable) -> Result<()> {
    let start = output_start(buffer);
    let ids = table.backing_ids();
    let needed = ids
        .len()
        .checked_mul(8)
        .and_then(|size| size.checked_add(8))
        .ok_or_else(|| Error::new(Errno::EINVAL))?;
    if buffer.len().saturating_sub(start) < needed {
        write_u32(
            buffer,
            OFF_FLAGS,
            read_u32(buffer, OFF_FLAGS) | BUFFER_FULL_FLAG,
        );
        return Ok(());
    }
    buffer[start..start + needed].fill(0);
    write_u32(buffer, start, ids.len() as u32);
    for (index, id) in ids.into_iter().enumerate() {
        write_u64(buffer, start + 8 + index * 8, id.as_encoded_u64());
    }
    write_u32(buffer, OFF_DATA_SIZE, (start + needed) as u32);
    Ok(())
}

pub(super) fn table_status(buffer: &mut [u8]) -> Result<()> {
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

/// Parses the complete linear table before publishing the primary device.
pub(super) fn table_load(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_load_device(buffer)?;
    table_load_for_device_with_primary(buffer, &device, runtime::ensure_primary)
}

fn table_load_for_device_with_primary(
    buffer: &mut [u8],
    device: &Arc<DmDevice>,
    publish_primary: impl FnOnce(&Arc<DmDevice>) -> Result<()>,
) -> Result<()> {
    let mut lifecycle = device.lock_lifecycle();
    ensure_control_live(device)?;
    let table = parse_table_load_request(buffer)?;
    publish_primary(device)?;
    lifecycle
        .load_table(table)
        .map_err(|_| Error::new(Errno::EINVAL))?;
    fill_device_header(buffer, device, true);
    Ok(())
}

/// Suspends a running mapping or resumes it, switching a pending table if present.
pub(super) fn resume_device(buffer: &mut [u8]) -> Result<()> {
    let device = lookup_load_device(buffer)?;
    resume_device_for(buffer, &device)
}

fn resume_device_for(buffer: &mut [u8], device: &DmDevice) -> Result<()> {
    let mut lifecycle = device.lock_lifecycle();
    ensure_control_live(device)?;
    let flags = read_u32(buffer, OFF_FLAGS);
    // Filesystem freezing is not implemented; accept SKIP_LOCKFS (the caller
    // explicitly requests no freeze), but reject NOFLUSH: it needs distinct I/O
    // semantics rather than the draining suspend implemented here.
    if flags & NOFLUSH_FLAG != 0 {
        return_errno_with_message!(Errno::EINVAL, "noflush suspend is not supported");
    }
    if flags & SUSPEND_FLAG != 0 {
        lifecycle.suspend().map_err(|_| Error::new(Errno::EINVAL))?;
    } else if device.is_suspended() {
        lifecycle
            .resume_suspended()
            .map_err(|_| Error::new(Errno::EINVAL))?;
    } else if device.active_table().is_some() && device.inactive_table().is_some() {
        // Linux implicitly suspends a running mapper before installing a candidate.
        lifecycle.suspend().map_err(|_| Error::new(Errno::EINVAL))?;
        lifecycle
            .resume_suspended()
            .map_err(|_| Error::new(Errno::EINVAL))?;
    } else {
        lifecycle.resume_initial().map_err(|_| {
            Error::with_message(
                Errno::EINVAL,
                "mapper has no initial table or requires suspend",
            )
        })?;
    }
    fill_device_header(buffer, device, false);
    Ok(())
}

#[cfg(ktest)]
mod tests {
    use ostd::prelude::ktest;

    use super::{super::test_support::*, *};
    use crate::device::registry::block;

    #[ktest]
    fn deps_select_inactive_and_active_tables_and_report_buffer_full() {
        let manager = aster_dm::DmManager::new().unwrap();
        let device = manager
            .create("deps-query-test".into(), None, None)
            .unwrap();
        let backing = DeviceId::new(MajorId::new(510), MinorId::new(165));
        aster_block::register(Arc::new(Backing(backing))).unwrap();
        let mut request = buffer("deps-query-test", backing);
        table_load_for_device_with_primary(&mut request, &device, |_| Ok(())).unwrap();
        let make_query = |size, flags| {
            let mut bytes = vec![0u8; size];
            write_u32(&mut bytes, OFF_DATA_SIZE, FIXED_PREFIX_SIZE as u32);
            write_u32(&mut bytes, OFF_FLAGS, flags);
            bytes
        };
        let mut empty = make_query(DM_IOCTL_HEADER_SIZE + 16, 0);
        let (active, inactive) = device.table_snapshot();
        let selected = selected_table(&active, &inactive, false);
        query_header(
            &mut empty,
            &device,
            active.as_ref(),
            inactive.as_ref(),
            selected,
        );
        assert_eq!(read_u32(&empty, OFF_TARGET_COUNT), 0);
        assert_eq!(read_u32(&empty, OFF_DATA_SIZE), FIXED_PREFIX_SIZE as u32);
        let mut short = make_query(DM_IOCTL_HEADER_SIZE + 15, QUERY_INACTIVE_TABLE_FLAG);
        let selected = selected_table(&active, &inactive, true);
        query_header(
            &mut short,
            &device,
            active.as_ref(),
            inactive.as_ref(),
            selected,
        );
        encode_table_deps(&mut short, selected.unwrap()).unwrap();
        assert_eq!(read_u32(&short, OFF_TARGET_COUNT), 1);
        assert_eq!(
            read_u32(&short, OFF_FLAGS) & BUFFER_FULL_FLAG,
            BUFFER_FULL_FLAG
        );
        assert_eq!(read_u32(&short, OFF_DATA_SIZE), FIXED_PREFIX_SIZE as u32);
        let mut exact = make_query(DM_IOCTL_HEADER_SIZE + 16, QUERY_INACTIVE_TABLE_FLAG);
        query_header(
            &mut exact,
            &device,
            active.as_ref(),
            inactive.as_ref(),
            selected,
        );
        encode_table_deps(&mut exact, selected.unwrap()).unwrap();
        assert_eq!(read_u32(&exact, OFF_FLAGS) & BUFFER_FULL_FLAG, 0);
        assert_eq!(
            read_u32(&exact, OFF_DATA_SIZE),
            (DM_IOCTL_HEADER_SIZE + 16) as u32
        );
        assert_eq!(read_u32(&exact, DM_IOCTL_HEADER_SIZE), 1);
        assert_eq!(read_u32(&exact, DM_IOCTL_HEADER_SIZE + 4), 0);
        assert_eq!(
            read_u64(&exact, DM_IOCTL_HEADER_SIZE + 8),
            backing.as_encoded_u64()
        );
        drop(active);
        drop(inactive);
        device.lock_lifecycle().resume_initial().unwrap();
        let (active, inactive) = device.table_snapshot();
        let selected = selected_table(&active, &inactive, false);
        let mut live = make_query(DM_IOCTL_HEADER_SIZE + 16, 0);
        query_header(
            &mut live,
            &device,
            active.as_ref(),
            inactive.as_ref(),
            selected,
        );
        encode_table_deps(&mut live, selected.unwrap()).unwrap();
        assert_eq!(
            read_u64(&live, DM_IOCTL_HEADER_SIZE + 8),
            backing.as_encoded_u64()
        );
        assert!(selected_table(&active, &inactive, true).is_none());
        drop(active);
        drop(inactive);
        drop(request);
        manager.remove("deps-query-test").unwrap();
        drop(device);
        aster_block::unregister(backing).unwrap();
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
        device.lock_lifecycle().resume_initial().unwrap();
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
    fn loads_striped_table_and_reports_dependencies_and_status() {
        let manager = aster_dm::DmManager::new().unwrap();
        let device = manager
            .create("striped-load-test".into(), None, None)
            .unwrap();
        let first = DeviceId::new(MajorId::new(510), MinorId::new(170));
        let second = DeviceId::new(MajorId::new(510), MinorId::new(171));
        aster_block::register(Arc::new(Backing(first))).unwrap();
        aster_block::register(Arc::new(Backing(second))).unwrap();
        let mut request = buffer("striped-load-test", first);
        request.resize(DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE + 64, 0);
        let spec = DM_IOCTL_HEADER_SIZE;
        request[spec + 24..spec + 31].copy_from_slice(b"striped");
        let start = spec + DM_TARGET_SPEC_SIZE;
        request[start..].fill(0);
        let params = b"2 2 510:170 1 510:171 3";
        request[start..start + params.len()].copy_from_slice(params);
        table_load_for_device_with_primary(&mut request, &device, |_| Ok(())).unwrap();
        let table = device.inactive_table().unwrap();
        assert_eq!(table.length(), 8);
        assert_eq!(table.backing_ids(), vec![first, second]);
        assert_eq!(
            table.targets()[0]
                .status_params(TargetStatusMode::Table)
                .unwrap(),
            "2 2 510:170 1 510:171 3"
        );
        assert_eq!(
            table.targets()[0]
                .status_params(TargetStatusMode::Status)
                .unwrap(),
            "2 510:170 510:171 1 AA"
        );
        let mut output = vec![0; DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE + 64];
        encode_table_records(&mut output, &table, TargetStatusMode::Table).unwrap();
        assert_eq!(&output[spec + 24..spec + 32], b"striped\0");
        assert_eq!(&output[start..start + params.len()], params);
        assert_eq!(
            aster_block::unregister(first).unwrap_err(),
            aster_block::Error::Busy
        );
        assert_eq!(
            aster_block::unregister(second).unwrap_err(),
            aster_block::Error::Busy
        );
        drop(table);
        manager.remove("striped-load-test").unwrap();
        drop(device);
        aster_block::unregister(first).unwrap();
        aster_block::unregister(second).unwrap();
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
        let mut striped = buffer("load-parse-test", backing);
        striped[DM_IOCTL_HEADER_SIZE + 24..DM_IOCTL_HEADER_SIZE + 31].copy_from_slice(b"striped");
        assert_eq!(
            table_load_for_device_with_primary(&mut striped, &device, |_| {
                publications += 1;
                Ok(())
            })
            .unwrap_err()
            .error(),
            Errno::EINVAL
        );
        assert_eq!(publications, 1);
        assert!(Arc::ptr_eq(&old, &device.inactive_table().unwrap()));
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
        drop(striped);
        drop(invalid);
        drop(request);
        drop(old);
        manager.remove("load-parse-test").unwrap();
        drop(device);
        aster_block::unregister(backing).unwrap();
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
        // LVM may request SKIP_LOCKFS for an unmounted raw LV. It must still
        // take the normal draining suspend path with the flags LVM sends.
        write_u32(
            &mut resume,
            OFF_FLAGS,
            SUSPEND_FLAG | SKIP_LOCKFS_FLAG | PERSISTENT_DEV_FLAG | (1 << 9),
        );
        write_u64(&mut resume, OFF_DEV, device.id().as_encoded_u64());
        resume_device_for(&mut resume, &device).unwrap();
        assert!(device.is_suspended());
        assert_ne!(read_u32(&resume, OFF_FLAGS) & SUSPEND_FLAG, 0);
        write_u32(&mut resume, OFF_FLAGS, 0);
        resume_device_for(&mut resume, &device).unwrap();
        assert!(!device.is_suspended());
        write_u64(&mut resume, OFF_DEV, 0);
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
        resume_device_for(&mut resume, &device).unwrap();
        assert!(!device.is_suspended());
        assert!(device.inactive_table().is_none());
        assert!(!device.active_table().unwrap().is_readonly());
        assert_eq!(read_u32(&resume, OFF_FLAGS) & READONLY_FLAG, 0);
        // A loaded candidate on a running mapper also triggers an implicit drain.
        let mut implicit = buffer("first-resume-test", backing);
        write_u32(&mut implicit, OFF_FLAGS, READONLY_FLAG);
        table_load_for_device_with_primary(&mut implicit, &device, |_| Ok(())).unwrap();
        write_u32(&mut resume, OFF_FLAGS, 0);
        resume_device_for(&mut resume, &device).unwrap();
        assert!(device.active_table().unwrap().is_readonly());
        assert!(device.inactive_table().is_none());
        assert_eq!(read_u32(&resume, OFF_FLAGS) & READONLY_FLAG, READONLY_FLAG);
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
