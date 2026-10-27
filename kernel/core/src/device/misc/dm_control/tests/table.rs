// SPDX-License-Identifier: MPL-2.0

//! Table parsing, backing resolution, query records, and list encoding contracts.

use super::*;

#[ktest]
fn table_record_cursor_remains_stable_after_table_mutation() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-query-snapshot-stable".to_string(), None, None)
        .unwrap();
    device.load_table(single_zero_table(8));

    let mut records = QueryWorkflow::table_records(&device, true, TargetStatusMode::Table);
    device.clear_inactive_table().unwrap();
    let target = records.next_target().unwrap().unwrap();

    assert!(records.device().status.has_inactive_table);
    assert_eq!(target.logical_start, 0);
    assert_eq!(target.length, 8);
    assert_eq!(target.name, "zero");
    assert_eq!(target.params, "");
    assert!(records.next_target().unwrap().is_none());
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
    assert_eq!(read_u32(&status, OFF_FLAGS).unwrap(), 0);

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
        DM_QUERY_INACTIVE_TABLE_FLAG
    );

    let mut deps = test_buffer(DM_IOCTL_HEADER_SIZE + 16);
    table_deps_for_device(&mut deps, &device).unwrap();
    assert_eq!(read_u32(&deps, DM_IOCTL_HEADER_SIZE).unwrap(), 0);
    assert_eq!(read_u32(&deps, DM_IOCTL_HEADER_SIZE + 4).unwrap(), 0);
}

#[ktest]
fn table_status_stops_materializing_records_at_full_output_buffer() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-table-status-lazy-buffer".to_string(), None, None)
        .unwrap();
    let status_calls = Arc::new(Mutex::new(0));
    device.load_table(Arc::new(
        DmTable::new_targets(vec![
            boxed(CountingStatusTarget::new(0, 4, status_calls.clone())),
            boxed(CountingStatusTarget::new(4, 4, status_calls.clone())),
        ])
        .unwrap(),
    ));
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
    write_u32(
        &mut buffer,
        OFF_FLAGS,
        DM_QUERY_INACTIVE_TABLE_FLAG | DM_STATUS_TABLE_FLAG,
    )
    .unwrap();

    table_status_for_device(&mut buffer, &device).unwrap();

    assert_eq!(*status_calls.lock(), 0);
    assert_ne!(
        read_u32(&buffer, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG,
        0
    );
}

#[ktest]
fn reports_error_table_status_with_empty_params_and_no_deps() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-error-status-test".to_string(), None, None)
        .unwrap();
    device.load_table(single_error_table(8));

    for flags in [
        DM_QUERY_INACTIVE_TABLE_FLAG,
        DM_QUERY_INACTIVE_TABLE_FLAG | DM_STATUS_TABLE_FLAG,
    ] {
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 49);
        write_u32(&mut buffer, OFF_FLAGS, flags).unwrap();
        table_status_for_device(&mut buffer, &device).unwrap();

        assert_eq!(read_u32(&buffer, OFF_TARGET_COUNT).unwrap(), 1);
        assert_eq!(read_u64(&buffer, DM_IOCTL_HEADER_SIZE).unwrap(), 0);
        assert_eq!(read_u64(&buffer, DM_IOCTL_HEADER_SIZE + 8).unwrap(), 8);
        assert_eq!(read_u32(&buffer, DM_IOCTL_HEADER_SIZE + 20).unwrap(), 48);
        assert_eq!(
            c_string_until(
                &buffer,
                DM_IOCTL_HEADER_SIZE + 24,
                DM_IOCTL_HEADER_SIZE + 40
            )
            .unwrap(),
            "error"
        );
        assert_eq!(
            c_string_until(
                &buffer,
                DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE,
                DM_IOCTL_HEADER_SIZE + 48
            )
            .unwrap(),
            ""
        );
    }

    let mut deps = test_buffer(DM_IOCTL_HEADER_SIZE + 8);
    write_u32(&mut deps, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
    table_deps_for_device(&mut deps, &device).unwrap();
    assert_eq!(read_u32(&deps, DM_IOCTL_HEADER_SIZE).unwrap(), 0);
    assert_eq!(read_u32(&deps, DM_IOCTL_HEADER_SIZE + 4).unwrap(), 0);
}

#[ktest]
fn reports_zero_table_status_with_empty_params_and_no_deps() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-zero-status-test".to_string(), None, None)
        .unwrap();
    device.load_table(single_zero_table(8));

    for flags in [
        DM_QUERY_INACTIVE_TABLE_FLAG,
        DM_QUERY_INACTIVE_TABLE_FLAG | DM_STATUS_TABLE_FLAG,
    ] {
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 49);
        write_u32(&mut buffer, OFF_FLAGS, flags).unwrap();
        table_status_for_device(&mut buffer, &device).unwrap();

        assert_eq!(read_u32(&buffer, OFF_TARGET_COUNT).unwrap(), 1);
        assert_eq!(read_u64(&buffer, DM_IOCTL_HEADER_SIZE).unwrap(), 0);
        assert_eq!(read_u64(&buffer, DM_IOCTL_HEADER_SIZE + 8).unwrap(), 8);
        assert_eq!(read_u32(&buffer, DM_IOCTL_HEADER_SIZE + 20).unwrap(), 48);
        assert_eq!(
            c_string_until(
                &buffer,
                DM_IOCTL_HEADER_SIZE + 24,
                DM_IOCTL_HEADER_SIZE + 40
            )
            .unwrap(),
            "zero"
        );
        assert_eq!(
            c_string_until(
                &buffer,
                DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE,
                DM_IOCTL_HEADER_SIZE + 48
            )
            .unwrap(),
            ""
        );
    }

    let mut deps = test_buffer(DM_IOCTL_HEADER_SIZE + 8);
    write_u32(&mut deps, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
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

    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 3 * 48 + 1);
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
            c_string_until(&buffer, record + 24, record + 40).unwrap(),
            "linear"
        );
        assert_eq!(
            c_string_until(&buffer, record + 40, record + 48).unwrap(),
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

    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 49);
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
            DM_IOCTL_HEADER_SIZE + 40
        )
        .unwrap(),
        "linear"
    );
    assert_eq!(
        c_string_until(
            &buffer,
            DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE,
            DM_IOCTL_HEADER_SIZE + 48
        )
        .unwrap(),
        ""
    );
}

#[ktest]
fn reports_striped_table_status_with_canonical_params() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-striped-table-status-test".to_string(), None, None)
        .unwrap();
    let params = "2 4 1:1 10 1:2 20";
    let record_len = table_status_record_len(params.len()).unwrap();
    device.load_table(single_striped_table(16, params, &[1, 2]));

    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + record_len);
    write_u32(
        &mut buffer,
        OFF_FLAGS,
        DM_QUERY_INACTIVE_TABLE_FLAG | DM_STATUS_TABLE_FLAG,
    )
    .unwrap();
    table_status_for_device(&mut buffer, &device).unwrap();

    assert_eq!(read_u32(&buffer, OFF_TARGET_COUNT).unwrap(), 1);
    assert_eq!(read_u64(&buffer, DM_IOCTL_HEADER_SIZE).unwrap(), 0);
    assert_eq!(read_u64(&buffer, DM_IOCTL_HEADER_SIZE + 8).unwrap(), 16);
    assert_eq!(
        read_u32(&buffer, DM_IOCTL_HEADER_SIZE + 20).unwrap(),
        record_len as u32
    );
    assert_eq!(
        c_string_until(
            &buffer,
            DM_IOCTL_HEADER_SIZE + 24,
            DM_IOCTL_HEADER_SIZE + 40
        )
        .unwrap(),
        "striped"
    );
    assert_eq!(
        c_string_until(
            &buffer,
            DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE,
            DM_IOCTL_HEADER_SIZE + record_len
        )
        .unwrap(),
        params
    );
}

#[ktest]
fn reports_striped_info_status_with_linux_params() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-striped-info-status-test".to_string(), None, None)
        .unwrap();
    let params = "2 1:1 1:2 1 AA";
    let record_len = table_status_record_len(params.len()).unwrap();
    device.load_table(single_striped_table(16, "2 4 1:1 0 1:2 0", &[1, 2]));

    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + record_len);
    write_u32(&mut buffer, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
    table_status_for_device(&mut buffer, &device).unwrap();

    assert_eq!(read_u32(&buffer, OFF_TARGET_COUNT).unwrap(), 1);
    assert_eq!(read_u64(&buffer, DM_IOCTL_HEADER_SIZE + 8).unwrap(), 16);
    assert_eq!(
        read_u32(&buffer, DM_IOCTL_HEADER_SIZE + 20).unwrap(),
        record_len as u32
    );
    assert_eq!(
        c_string_until(
            &buffer,
            DM_IOCTL_HEADER_SIZE + 24,
            DM_IOCTL_HEADER_SIZE + 40
        )
        .unwrap(),
        "striped"
    );
    assert_eq!(
        c_string_until(
            &buffer,
            DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE,
            DM_IOCTL_HEADER_SIZE + record_len
        )
        .unwrap(),
        params
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
fn reports_table_deps_from_mixed_linear_and_striped_targets() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-mixed-striped-deps-test".to_string(), None, None)
        .unwrap();
    let first = status_backing_lease(1);
    let second = status_backing_lease(2);
    let table = DmTable::new_targets(vec![
        boxed(LinearTarget::new(Sid::new(0), 4, Sid::new(100), first.clone()).unwrap()),
        boxed(
            StripedTarget::new(
                Sid::new(4),
                16,
                StripedTargetParams::parse("2 4 1:1 0 1:2 0").unwrap(),
                vec![first.clone(), second.clone()],
            )
            .unwrap(),
        ),
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
fn marks_striped_table_status_buffer_full_when_params_do_not_fit() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-striped-status-buffer-full-test".to_string(), None, None)
        .unwrap();
    let params = "2 4 1:1 0 1:2 0";
    let record_len = table_status_record_len(params.len()).unwrap();
    device.load_table(single_striped_table(16, params, &[1, 2]));

    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + record_len - 1);
    write_u32(
        &mut buffer,
        OFF_FLAGS,
        DM_QUERY_INACTIVE_TABLE_FLAG | DM_STATUS_TABLE_FLAG,
    )
    .unwrap();
    table_status_for_device(&mut buffer, &device).unwrap();

    assert_eq!(read_u32(&buffer, OFF_TARGET_COUNT).unwrap(), 1);
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
fn reports_table_status_with_linux_string_termination_slack() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-status-exact-fit-test".to_string(), None, None)
        .unwrap();
    let table = DmTable::new_linear(vec![
        LinearTarget::new(Sid::new(0), 4, Sid::new(100), status_backing_lease(1)).unwrap(),
        LinearTarget::new(Sid::new(4), 4, Sid::new(200), status_backing_lease(2)).unwrap(),
    ])
    .unwrap();
    device.load_table(Arc::new(table));

    // Each table string uses all eight bytes after its fixed spec. Linux
    // requires one additional byte of capacity beyond the final string's NUL.
    for capacity in [48, 49, 96, 97] {
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + capacity);
        write_u32(
            &mut buffer,
            OFF_FLAGS,
            DM_QUERY_INACTIVE_TABLE_FLAG | DM_STATUS_TABLE_FLAG,
        )
        .unwrap();
        // Direct handler calls must start with the same output length as dispatch.
        write_u32(&mut buffer, OFF_DATA_SIZE, DM_IOCTL_FIXED_PREFIX_SIZE).unwrap();
        table_status_for_device(&mut buffer, &device).unwrap();

        assert_eq!(read_u32(&buffer, OFF_TARGET_COUNT).unwrap(), 2);
        assert_eq!(
            read_u32(&buffer, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG != 0,
            capacity < 97
        );
        let output_size = match capacity {
            48 => DM_IOCTL_FIXED_PREFIX_SIZE,
            49 | 96 => DM_IOCTL_HEADER_SIZE + 48,
            97 => DM_IOCTL_HEADER_SIZE + 96,
            _ => unreachable!(),
        };
        assert_eq!(
            read_u32(&buffer, OFF_DATA_SIZE).unwrap() as usize,
            output_size
        );
        if capacity == 48 {
            continue;
        }
        assert_eq!(read_u64(&buffer, DM_IOCTL_HEADER_SIZE).unwrap(), 0);
        assert_eq!(read_u32(&buffer, DM_IOCTL_HEADER_SIZE + 20).unwrap(), 48);
        assert_eq!(
            c_string_until(
                &buffer,
                DM_IOCTL_HEADER_SIZE + 40,
                DM_IOCTL_HEADER_SIZE + 48
            )
            .unwrap(),
            "1:1 100"
        );
        if capacity == 97 {
            assert_eq!(read_u64(&buffer, DM_IOCTL_HEADER_SIZE + 48).unwrap(), 4);
            assert_eq!(read_u32(&buffer, DM_IOCTL_HEADER_SIZE + 68).unwrap(), 96);
            assert_eq!(
                c_string_until(
                    &buffer,
                    DM_IOCTL_HEADER_SIZE + 88,
                    DM_IOCTL_HEADER_SIZE + 96
                )
                .unwrap(),
                "1:2 200"
            );
        }
    }
}

#[ktest]
fn marks_table_status_buffer_full_after_complete_record() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-status-partial-record-test".to_string(), None, None)
        .unwrap();
    let table = DmTable::new_linear(vec![
        LinearTarget::new(Sid::new(0), 4, Sid::new(100), status_backing_lease(1)).unwrap(),
        LinearTarget::new(Sid::new(4), 4, Sid::new(200), status_backing_lease(2)).unwrap(),
    ])
    .unwrap();
    device.load_table(Arc::new(table));

    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 49);
    write_u32(
        &mut buffer,
        OFF_FLAGS,
        DM_QUERY_INACTIVE_TABLE_FLAG | DM_STATUS_TABLE_FLAG,
    )
    .unwrap();
    table_status_for_device(&mut buffer, &device).unwrap();

    assert_eq!(read_u32(&buffer, OFF_TARGET_COUNT).unwrap(), 2);
    assert_ne!(
        read_u32(&buffer, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG,
        0
    );
    assert_eq!(read_u64(&buffer, DM_IOCTL_HEADER_SIZE).unwrap(), 0);
    assert_eq!(read_u64(&buffer, DM_IOCTL_HEADER_SIZE + 8).unwrap(), 4);
    assert_eq!(read_u32(&buffer, DM_IOCTL_HEADER_SIZE + 20).unwrap(), 48);
    assert_eq!(
        c_string_until(
            &buffer,
            DM_IOCTL_HEADER_SIZE + 40,
            DM_IOCTL_HEADER_SIZE + 48
        )
        .unwrap(),
        "1:1 100"
    );
}

#[ktest]
fn reports_table_deps_without_buffer_full_on_exact_fit() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-deps-exact-fit-test".to_string(), None, None)
        .unwrap();
    let first = status_backing_lease(1);
    let second = status_backing_lease(2);
    let table = DmTable::new_linear(vec![
        LinearTarget::new(Sid::new(0), 4, Sid::new(100), first.clone()).unwrap(),
        LinearTarget::new(Sid::new(4), 4, Sid::new(200), second.clone()).unwrap(),
    ])
    .unwrap();
    device.load_table(Arc::new(table));

    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 24);
    write_u32(&mut buffer, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
    table_deps_for_device(&mut buffer, &device).unwrap();

    assert_eq!(
        read_u32(&buffer, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG,
        0
    );
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
fn deps_ignores_input_data_start_and_reports_selected_count() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-invalid-deps-start-test".to_string(), None, None)
        .unwrap();
    device.load_table(single_linear_table(4, 100, 1));
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 24);
    write_u32(&mut buffer, OFF_DATA_START, DM_IOCTL_HEADER_SIZE + 1).unwrap();
    write_u32(&mut buffer, OFF_TARGET_COUNT, u32::MAX).unwrap();
    write_u32(&mut buffer, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();

    table_deps_for_device(&mut buffer, &device).unwrap();
    assert_eq!(read_u32(&buffer, OFF_DATA_START).unwrap(), 312);
    assert_eq!(read_u32(&buffer, OFF_DATA_SIZE).unwrap(), 328);
    assert_eq!(read_u32(&buffer, OFF_TARGET_COUNT).unwrap(), 1);
    assert_eq!(
        read_u32(&buffer, OFF_FLAGS).unwrap(),
        DM_QUERY_INACTIVE_TABLE_FLAG | DM_INACTIVE_PRESENT_FLAG
    );
}

#[ktest]
fn reports_status_target_count_for_selected_table() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-device-status-test".to_string(), None, None)
        .unwrap();

    let mut fresh_active = test_buffer(DM_IOCTL_HEADER_SIZE);
    device_status_for_device(&mut fresh_active, &device).unwrap();
    assert_eq!(read_u32(&fresh_active, OFF_TARGET_COUNT).unwrap(), 0);
    assert_eq!(read_u32(&fresh_active, OFF_FLAGS).unwrap(), 0);

    let mut fresh_inactive = test_buffer(DM_IOCTL_HEADER_SIZE);
    write_u32(&mut fresh_inactive, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
    device_status_for_device(&mut fresh_inactive, &device).unwrap();
    assert_eq!(read_u32(&fresh_inactive, OFF_TARGET_COUNT).unwrap(), 0);
    assert_eq!(
        read_u32(&fresh_inactive, OFF_FLAGS).unwrap(),
        DM_QUERY_INACTIVE_TABLE_FLAG
    );

    let first = status_backing_lease(1);
    let second = status_backing_lease(2);
    let table = DmTable::new_linear(vec![
        LinearTarget::new(Sid::new(0), 4, Sid::new(100), first).unwrap(),
        LinearTarget::new(Sid::new(4), 4, Sid::new(200), second).unwrap(),
    ])
    .unwrap();
    device.load_table(Arc::new(table));

    let mut loaded_active = test_buffer(DM_IOCTL_HEADER_SIZE);
    device_status_for_device(&mut loaded_active, &device).unwrap();
    assert_eq!(read_u32(&loaded_active, OFF_TARGET_COUNT).unwrap(), 0);
    assert_eq!(
        read_u32(&loaded_active, OFF_FLAGS).unwrap(),
        DM_INACTIVE_PRESENT_FLAG
    );

    let mut loaded_inactive = test_buffer(DM_IOCTL_HEADER_SIZE);
    write_u32(
        &mut loaded_inactive,
        OFF_FLAGS,
        DM_QUERY_INACTIVE_TABLE_FLAG,
    )
    .unwrap();
    device_status_for_device(&mut loaded_inactive, &device).unwrap();
    assert_eq!(read_u32(&loaded_inactive, OFF_TARGET_COUNT).unwrap(), 2);
    assert_eq!(
        read_u32(&loaded_inactive, OFF_FLAGS).unwrap(),
        DM_QUERY_INACTIVE_TABLE_FLAG | DM_INACTIVE_PRESENT_FLAG
    );

    device.resume().unwrap();

    let mut resumed_active = test_buffer(DM_IOCTL_HEADER_SIZE);
    device_status_for_device(&mut resumed_active, &device).unwrap();
    assert_eq!(read_u32(&resumed_active, OFF_TARGET_COUNT).unwrap(), 2);
    assert_eq!(
        read_u32(&resumed_active, OFF_FLAGS).unwrap(),
        DM_ACTIVE_PRESENT_FLAG
    );

    let mut resumed_inactive = test_buffer(DM_IOCTL_HEADER_SIZE);
    write_u32(
        &mut resumed_inactive,
        OFF_FLAGS,
        DM_QUERY_INACTIVE_TABLE_FLAG,
    )
    .unwrap();
    device_status_for_device(&mut resumed_inactive, &device).unwrap();
    assert_eq!(read_u32(&resumed_inactive, OFF_TARGET_COUNT).unwrap(), 0);
    assert_eq!(
        read_u32(&resumed_inactive, OFF_FLAGS).unwrap(),
        DM_QUERY_INACTIVE_TABLE_FLAG | DM_ACTIVE_PRESENT_FLAG
    );
}

#[ktest]
fn query_flags_select_active_or_inactive_tables_consistently() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-query-flags-test".to_string(), None, None)
        .unwrap();
    let active_backing = status_backing_lease(1);
    let inactive_first = status_backing_lease(2);
    let inactive_second = status_backing_lease(3);

    device.load_table(Arc::new(
        DmTable::new_linear(vec![
            LinearTarget::new(Sid::new(0), 4, Sid::new(100), active_backing.clone()).unwrap(),
        ])
        .unwrap(),
    ));
    device.resume().unwrap();
    device.load_table(Arc::new(
        DmTable::new_linear(vec![
            LinearTarget::new(Sid::new(0), 4, Sid::new(200), inactive_first.clone()).unwrap(),
            LinearTarget::new(Sid::new(4), 8, Sid::new(300), inactive_second.clone()).unwrap(),
        ])
        .unwrap(),
    ));

    let mut active_status = test_buffer(DM_IOCTL_HEADER_SIZE + 49);
    device_status_for_device(&mut active_status, &device).unwrap();
    assert_eq!(read_u32(&active_status, OFF_TARGET_COUNT).unwrap(), 1);

    let mut inactive_status = test_buffer(DM_IOCTL_HEADER_SIZE + 97);
    write_u32(
        &mut inactive_status,
        OFF_FLAGS,
        DM_QUERY_INACTIVE_TABLE_FLAG,
    )
    .unwrap();
    device_status_for_device(&mut inactive_status, &device).unwrap();
    assert_eq!(read_u32(&inactive_status, OFF_TARGET_COUNT).unwrap(), 2);

    let mut active_table_status = test_buffer(DM_IOCTL_HEADER_SIZE + 49);
    write_u32(&mut active_table_status, OFF_FLAGS, DM_STATUS_TABLE_FLAG).unwrap();
    table_status_for_device(&mut active_table_status, &device).unwrap();
    assert_eq!(read_u32(&active_table_status, OFF_TARGET_COUNT).unwrap(), 1);
    assert_eq!(
        read_u64(&active_table_status, DM_IOCTL_HEADER_SIZE + 8).unwrap(),
        4
    );
    assert_eq!(
        c_string_until(
            &active_table_status,
            DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE,
            DM_IOCTL_HEADER_SIZE + 48
        )
        .unwrap(),
        "1:1 100"
    );

    let mut inactive_table_status = test_buffer(DM_IOCTL_HEADER_SIZE + 97);
    write_u32(
        &mut inactive_table_status,
        OFF_FLAGS,
        DM_QUERY_INACTIVE_TABLE_FLAG | DM_STATUS_TABLE_FLAG,
    )
    .unwrap();
    table_status_for_device(&mut inactive_table_status, &device).unwrap();
    assert_eq!(
        read_u32(&inactive_table_status, OFF_TARGET_COUNT).unwrap(),
        2
    );
    assert_eq!(
        read_u64(&inactive_table_status, DM_IOCTL_HEADER_SIZE + 8).unwrap(),
        4
    );
    assert_eq!(
        c_string_until(
            &inactive_table_status,
            DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE,
            DM_IOCTL_HEADER_SIZE + 48
        )
        .unwrap(),
        "1:2 200"
    );
    assert_eq!(
        read_u64(&inactive_table_status, DM_IOCTL_HEADER_SIZE + 48 + 8).unwrap(),
        8
    );
    assert_eq!(
        c_string_until(
            &inactive_table_status,
            DM_IOCTL_HEADER_SIZE + 48 + DM_TARGET_SPEC_SIZE,
            DM_IOCTL_HEADER_SIZE + 96
        )
        .unwrap(),
        "1:3 300"
    );

    let mut active_deps = test_buffer(DM_IOCTL_HEADER_SIZE + 16);
    table_deps_for_device(&mut active_deps, &device).unwrap();
    assert_eq!(read_u32(&active_deps, DM_IOCTL_HEADER_SIZE).unwrap(), 1);
    assert_eq!(
        read_u64(&active_deps, DM_IOCTL_HEADER_SIZE + 8).unwrap(),
        active_backing.id().as_encoded_u64()
    );

    let mut inactive_deps = test_buffer(DM_IOCTL_HEADER_SIZE + 24);
    write_u32(&mut inactive_deps, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
    table_deps_for_device(&mut inactive_deps, &device).unwrap();
    assert_eq!(read_u32(&inactive_deps, DM_IOCTL_HEADER_SIZE).unwrap(), 2);
    assert_eq!(
        read_u64(&inactive_deps, DM_IOCTL_HEADER_SIZE + 8).unwrap(),
        inactive_first.id().as_encoded_u64()
    );
    assert_eq!(
        read_u64(&inactive_deps, DM_IOCTL_HEADER_SIZE + 16).unwrap(),
        inactive_second.id().as_encoded_u64()
    );
}

#[ktest]
fn rejects_empty_target_table_load_before_state_changes() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-zero-target-test".to_string(), None, None)
        .unwrap();
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);

    assert_eq!(
        table_load_for_test(&mut buffer, &device)
            .unwrap_err()
            .error(),
        Errno::EINVAL
    );
    assert_eq!(
        device.status(),
        aster_device_mapper::DmDeviceStatus {
            suspended: false,
            readonly: false,
            has_active_table: false,
            has_inactive_table: false,
            event_nr: 0,
        }
    );
}

#[ktest]
fn loads_single_error_target_through_table_load_facade() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-error-load-test".to_string(), None, None)
        .unwrap();
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + table_status_record_len(0).unwrap());
    write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
    write_error_target_spec(&mut buffer, DM_IOCTL_HEADER_SIZE, 0, 8, 0);

    table_load_for_test(&mut buffer, &device).unwrap();
    let table = device.inactive_table().unwrap();
    assert_eq!(table.target_count(), 1);
    let target = error_target(&table, 0);
    assert_eq!(target.logical_range(), &(Sid::new(0)..Sid::new(8)));
    assert_eq!(table.metadata().nr_sectors, 8);
    assert_eq!(table.metadata().max_nr_segments_per_bio, usize::MAX);
}

#[ktest]
fn loads_single_zero_target_through_table_load_facade() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-zero-load-test".to_string(), None, None)
        .unwrap();
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + table_status_record_len(0).unwrap());
    write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
    write_zero_target_spec(&mut buffer, DM_IOCTL_HEADER_SIZE, 0, 8, 0);

    table_load_for_test(&mut buffer, &device).unwrap();
    let table = device.inactive_table().unwrap();
    assert_eq!(table.target_count(), 1);
    let target = zero_target(&table, 0);
    assert_eq!(target.logical_range(), &(Sid::new(0)..Sid::new(8)));
    assert_eq!(table.metadata().nr_sectors, 8);
    assert_eq!(table.metadata().max_nr_segments_per_bio, usize::MAX);
}

#[ktest]
fn rejects_error_target_params_before_state_changes() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-error-param-test".to_string(), None, None)
        .unwrap();
    let params = "unexpected";
    let mut buffer =
        test_buffer(DM_IOCTL_HEADER_SIZE + table_status_record_len(params.len()).unwrap());
    write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
    write_target_spec(&mut buffer, DM_IOCTL_HEADER_SIZE, 0, 8, 0, "error", params);

    assert_failed_table_load_preserves_state(&mut buffer, &device, Errno::EINVAL);
}

#[ktest]
fn rejects_zero_target_params_before_state_changes() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-zero-param-test".to_string(), None, None)
        .unwrap();
    let params = "unexpected";
    let mut buffer =
        test_buffer(DM_IOCTL_HEADER_SIZE + table_status_record_len(params.len()).unwrap());
    write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
    write_target_spec(&mut buffer, DM_IOCTL_HEADER_SIZE, 0, 8, 0, "zero", params);

    assert_failed_table_load_preserves_state(&mut buffer, &device, Errno::EINVAL);
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

    table_load_for_test(&mut buffer, &device).unwrap();
    let table = device.inactive_table().unwrap();
    assert_eq!(table.target_count(), 2);
    let first_target = linear_target(&table, 0);
    assert_eq!(first_target.logical_range(), &(Sid::new(0)..Sid::new(4)));
    assert_eq!(first_target.backing_id(), first_id);
    assert_eq!(first_target.backing_start(), Sid::new(100));
    let second_target = linear_target(&table, 1);
    assert_eq!(second_target.logical_range(), &(Sid::new(4)..Sid::new(12)));
    assert_eq!(second_target.backing_id(), second_id);
    assert_eq!(second_target.backing_start(), Sid::new(200));

    drop(table);
    device.clear_inactive_table().unwrap();
    unregister(first_id).unwrap();
    unregister(second_id).unwrap();
}

#[ktest]
fn loads_single_striped_target_through_table_load_facade() {
    let first = StatusBacking::new_with_major(510, 220);
    let second = StatusBacking::new_with_major(510, 221);
    let first_id = first.id();
    let second_id = second.id();
    register(first as Arc<dyn BlockDevice>).unwrap();
    register(second as Arc<dyn BlockDevice>).unwrap();

    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-striped-load-test".to_string(), None, None)
        .unwrap();
    let params = "2 128 510:220 0 510:221 0";
    let mut buffer =
        test_buffer(DM_IOCTL_HEADER_SIZE + table_status_record_len(params.len()).unwrap());
    write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
    write_target_spec(
        &mut buffer,
        DM_IOCTL_HEADER_SIZE,
        0,
        300,
        0,
        "striped",
        params,
    );

    table_load_for_test(&mut buffer, &device).unwrap();
    let table = device.inactive_table().unwrap();
    assert_eq!(table.target_count(), 1);
    let target = striped_target(&table, 0);
    assert_eq!(target.logical_range(), &(Sid::new(0)..Sid::new(300)));
    assert_eq!(target.stripe_count(), 2);
    assert_eq!(target.chunk_size(), 128);
    let mut ids = Vec::new();
    target.for_each_backing_id(|id| ids.push(id));
    assert_eq!(ids, vec![first_id, second_id]);

    drop(table);
    device.clear_inactive_table().unwrap();
    unregister(first_id).unwrap();
    unregister(second_id).unwrap();
}

#[ktest]
fn loads_mixed_linear_and_striped_targets_through_table_load_facade() {
    let linear = StatusBacking::new_with_major(510, 222);
    let striped_first = StatusBacking::new_with_major(510, 223);
    let striped_second = StatusBacking::new_with_major(510, 224);
    let linear_id = linear.id();
    let striped_first_id = striped_first.id();
    let striped_second_id = striped_second.id();
    register(linear as Arc<dyn BlockDevice>).unwrap();
    register(striped_first as Arc<dyn BlockDevice>).unwrap();
    register(striped_second as Arc<dyn BlockDevice>).unwrap();

    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-mixed-striped-load-test".to_string(), None, None)
        .unwrap();
    let linear_params = "510:222 100";
    let striped_params = "2 4 510:223 0 510:224 0";
    let first_next = table_status_record_len(linear_params.len()).unwrap();
    let mut buffer = test_buffer(
        DM_IOCTL_HEADER_SIZE + first_next + table_status_record_len(striped_params.len()).unwrap(),
    );
    write_u32(&mut buffer, OFF_TARGET_COUNT, 2).unwrap();
    write_linear_target_spec(
        &mut buffer,
        DM_IOCTL_HEADER_SIZE,
        0,
        4,
        first_next as u32,
        linear_params,
    );
    write_target_spec(
        &mut buffer,
        DM_IOCTL_HEADER_SIZE + first_next,
        4,
        16,
        0,
        "striped",
        striped_params,
    );

    table_load_for_test(&mut buffer, &device).unwrap();
    let table = device.inactive_table().unwrap();
    assert_eq!(table.target_count(), 2);
    assert_eq!(table.length(), 20);
    assert_eq!(linear_target(&table, 0).backing_id(), linear_id);
    let striped = striped_target(&table, 1);
    assert_eq!(striped.logical_range(), &(Sid::new(4)..Sid::new(20)));
    let mut ids = Vec::new();
    striped.for_each_backing_id(|id| ids.push(id));
    assert_eq!(ids, vec![striped_first_id, striped_second_id]);
    drop(table);

    let mut device_status = test_buffer(DM_IOCTL_HEADER_SIZE);
    write_u32(&mut device_status, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
    device_status_for_device(&mut device_status, &device).unwrap();
    assert_eq!(read_u32(&device_status, OFF_TARGET_COUNT).unwrap(), 2);
    assert_eq!(
        read_u32(&device_status, OFF_FLAGS).unwrap(),
        DM_QUERY_INACTIVE_TABLE_FLAG | DM_INACTIVE_PRESENT_FLAG
    );

    let linear_status_len = table_status_record_len(linear_params.len()).unwrap();
    let striped_status_len = table_status_record_len(striped_params.len()).unwrap();
    let total_status_len = linear_status_len + striped_status_len;
    let mut short = test_buffer(DM_IOCTL_HEADER_SIZE + total_status_len);
    write_u32(
        &mut short,
        OFF_FLAGS,
        DM_QUERY_INACTIVE_TABLE_FLAG | DM_STATUS_TABLE_FLAG,
    )
    .unwrap();
    write_u32(&mut short, OFF_DATA_SIZE, DM_IOCTL_FIXED_PREFIX_SIZE).unwrap();
    table_status_for_device(&mut short, &device).unwrap();
    assert_ne!(
        read_u32(&short, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG,
        0
    );
    assert_eq!(read_u32(&short, OFF_TARGET_COUNT).unwrap(), 2);
    assert_eq!(
        read_u32(&short, OFF_DATA_SIZE).unwrap() as usize,
        DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE + linear_params.len() + 1
    );
    // The final string exactly fills the short buffer; Linux requires slack
    // beyond its NUL before counting this record in the returned output.
    let mut status = test_buffer(DM_IOCTL_HEADER_SIZE + total_status_len + 1);
    write_u32(
        &mut status,
        OFF_FLAGS,
        DM_QUERY_INACTIVE_TABLE_FLAG | DM_STATUS_TABLE_FLAG,
    )
    .unwrap();
    table_status_for_device(&mut status, &device).unwrap();
    assert_eq!(
        read_u32(&status, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG,
        0
    );
    assert_eq!(
        read_u32(&status, OFF_DATA_SIZE).unwrap() as usize,
        DM_IOCTL_HEADER_SIZE + linear_status_len + DM_TARGET_SPEC_SIZE + striped_params.len() + 1
    );
    assert_eq!(read_u32(&status, OFF_TARGET_COUNT).unwrap(), 2);
    assert_eq!(read_u64(&status, DM_IOCTL_HEADER_SIZE).unwrap(), 0);
    assert_eq!(read_u64(&status, DM_IOCTL_HEADER_SIZE + 8).unwrap(), 4);
    assert_eq!(
        read_u32(&status, DM_IOCTL_HEADER_SIZE + 20).unwrap(),
        linear_status_len as u32
    );
    assert_eq!(
        c_string_until(
            &status,
            DM_IOCTL_HEADER_SIZE + 24,
            DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE
        )
        .unwrap(),
        "linear"
    );
    assert_eq!(
        c_string_until(
            &status,
            DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE,
            DM_IOCTL_HEADER_SIZE + linear_status_len
        )
        .unwrap(),
        linear_params
    );

    let striped_record = DM_IOCTL_HEADER_SIZE + linear_status_len;
    assert_eq!(read_u64(&status, striped_record).unwrap(), 4);
    assert_eq!(read_u64(&status, striped_record + 8).unwrap(), 16);
    assert_eq!(
        read_u32(&status, striped_record + 20).unwrap(),
        total_status_len as u32
    );
    assert_eq!(
        c_string_until(
            &status,
            striped_record + 24,
            striped_record + DM_TARGET_SPEC_SIZE
        )
        .unwrap(),
        "striped"
    );
    assert_eq!(
        c_string_until(
            &status,
            striped_record + DM_TARGET_SPEC_SIZE,
            DM_IOCTL_HEADER_SIZE + total_status_len
        )
        .unwrap(),
        striped_params
    );

    let mut deps = test_buffer(DM_IOCTL_HEADER_SIZE + 32);
    write_u32(&mut deps, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
    table_deps_for_device(&mut deps, &device).unwrap();
    assert_eq!(read_u32(&deps, DM_IOCTL_HEADER_SIZE).unwrap(), 3);
    assert_eq!(read_u32(&deps, DM_IOCTL_HEADER_SIZE + 4).unwrap(), 0);
    assert_eq!(
        read_u64(&deps, DM_IOCTL_HEADER_SIZE + 8).unwrap(),
        linear_id.as_encoded_u64()
    );
    assert_eq!(
        read_u64(&deps, DM_IOCTL_HEADER_SIZE + 16).unwrap(),
        striped_first_id.as_encoded_u64()
    );
    assert_eq!(
        read_u64(&deps, DM_IOCTL_HEADER_SIZE + 24).unwrap(),
        striped_second_id.as_encoded_u64()
    );

    device.clear_inactive_table().unwrap();
    unregister(linear_id).unwrap();
    unregister(striped_first_id).unwrap();
    unregister(striped_second_id).unwrap();
}

#[ktest]
fn submits_bio_across_ioctl_loaded_mixed_linear_and_striped_targets() {
    let linear = StatusBacking::new_with_major(510, 240);
    let striped_first = StatusBacking::new_with_major(510, 241);
    let striped_second = StatusBacking::new_with_major(510, 242);
    let linear_id = linear.id();
    let striped_first_id = striped_first.id();
    let striped_second_id = striped_second.id();
    register(linear.clone() as Arc<dyn BlockDevice>).unwrap();
    register(striped_first.clone() as Arc<dyn BlockDevice>).unwrap();
    register(striped_second.clone() as Arc<dyn BlockDevice>).unwrap();

    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-mixed-striped-bio-test".to_string(), None, None)
        .unwrap();
    let linear_params = "510:240 100";
    let striped_params = "2 4 510:241 0 510:242 0";
    let first_next = table_status_record_len(linear_params.len()).unwrap();
    let mut buffer = test_buffer(
        DM_IOCTL_HEADER_SIZE + first_next + table_status_record_len(striped_params.len()).unwrap(),
    );
    write_u32(&mut buffer, OFF_TARGET_COUNT, 2).unwrap();
    write_linear_target_spec(
        &mut buffer,
        DM_IOCTL_HEADER_SIZE,
        0,
        4,
        first_next as u32,
        linear_params,
    );
    write_target_spec(
        &mut buffer,
        DM_IOCTL_HEADER_SIZE + first_next,
        4,
        16,
        0,
        "striped",
        striped_params,
    );

    table_load_for_test(&mut buffer, &device).unwrap();
    device.resume().unwrap();
    let read = Bio::new(
        aster_block::bio::BioType::Read,
        Sid::new(2),
        vec![BioSegment::alloc(1, BioDirection::FromDevice)],
        None,
    );

    assert_eq!(
        read.submit_and_wait(device.as_ref()).unwrap(),
        BioStatus::Complete
    );
    assert_eq!(
        *linear.submitted_ranges.lock(),
        vec![Sid::new(102)..Sid::new(104)]
    );
    assert_eq!(
        *striped_first.submitted_ranges.lock(),
        vec![Sid::new(0)..Sid::new(4)]
    );
    assert_eq!(
        *striped_second.submitted_ranges.lock(),
        vec![Sid::new(0)..Sid::new(2)]
    );

    drop(device);
    drop(manager);
    unregister(linear_id).unwrap();
    unregister(striped_first_id).unwrap();
    unregister(striped_second_id).unwrap();
}

#[ktest]
fn rejects_unsupported_table_targets_without_changing_device_state() {
    for target_type in ["unknown", "snapshot"] {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create(format!("dm-unsupported-{target_type}-test"), None, None)
            .unwrap();
        let params = "2 128 510:220 0 510:221 0";
        let mut buffer =
            test_buffer(DM_IOCTL_HEADER_SIZE + table_status_record_len(params.len()).unwrap());
        write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
        write_target_spec(
            &mut buffer,
            DM_IOCTL_HEADER_SIZE,
            0,
            8,
            0,
            target_type,
            params,
        );

        assert_failed_table_load_preserves_state(&mut buffer, &device, Errno::EINVAL);
    }
}

#[ktest]
fn rejects_invalid_target_type_strings_without_changing_device_state() {
    let params = "510:230 0";
    for name in ["empty", "missing-nul"] {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create(format!("dm-invalid-target-type-{name}-test"), None, None)
            .unwrap();
        let mut buffer =
            test_buffer(DM_IOCTL_HEADER_SIZE + table_status_record_len(params.len()).unwrap());
        write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
        write_target_spec(&mut buffer, DM_IOCTL_HEADER_SIZE, 0, 4, 0, "linear", params);

        match name {
            "empty" => {
                write_c_string_fixed(
                    &mut buffer,
                    DM_IOCTL_HEADER_SIZE + 24,
                    DM_TARGET_TYPE_LEN,
                    "",
                )
                .unwrap();
            }
            "missing-nul" => {
                buffer[DM_IOCTL_HEADER_SIZE + 24..DM_IOCTL_HEADER_SIZE + 24 + DM_TARGET_TYPE_LEN]
                    .fill(b'x');
            }
            _ => unreachable!(),
        }

        assert_failed_table_load_preserves_state(&mut buffer, &device, Errno::EINVAL);
    }
}

#[ktest]
fn striped_relative_backing_propagates_path_lookup_errors() {
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 64);
    write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
    write_target_spec(
        &mut buffer,
        DM_IOCTL_HEADER_SIZE,
        0,
        8,
        0,
        "striped",
        "1 128 bad 0",
    );
    // A nonnumeric token is a relative path, not malformed target syntax.
    // Inject the VFS boundary because the ktest task has no process FS context.
    for errno in [Errno::ENOENT, Errno::ENOTBLK, Errno::EACCES] {
        let mut lookups = 0;
        let result = parse_table_load_request_with(&buffer, |token| {
            parse_backing_device_id_with(token, |path| {
                assert_eq!(path, "bad");
                lookups += 1;
                Err(Error::new(errno))
            })
        });
        assert_eq!(result.err().unwrap().error(), errno);
        assert_eq!(lookups, 1);
    }
}

#[ktest]
fn rejects_invalid_striped_params_without_changing_device_state() {
    for (name, params, length) in [
        ("few-fields", "2 128 510:227 0", 8),
        ("extra-fields", "1 128 510:227 0 extra", 8),
        ("zero-stripes", "0 128", 8),
        ("zero-chunk", "1 0 510:227 0", 8),
        ("bad-stripe-count", "many 128 510:227 0", 8),
        ("bad-chunk-size", "1 many 510:227 0", 8),
        ("bad-start", "1 128 510:227 -1", 8),
    ] {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create(format!("dm-invalid-striped-{name}-test"), None, None)
            .unwrap();
        let mut buffer =
            test_buffer(DM_IOCTL_HEADER_SIZE + table_status_record_len(params.len()).unwrap());
        write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
        write_target_spec(
            &mut buffer,
            DM_IOCTL_HEADER_SIZE,
            0,
            length,
            0,
            "striped",
            params,
        );

        assert_failed_table_load_preserves_state(&mut buffer, &device, Errno::EINVAL);
    }
}

#[ktest]
fn rejects_striped_range_errors_through_table_load_facade_without_changing_device_state() {
    let backing = StatusBacking::new_with_major(510, 227);
    let backing_id = backing.id();
    register(backing as Arc<dyn BlockDevice>).unwrap();

    for (name, start, length, backing_start) in [
        ("zero", 0, 0, 0),
        ("logical-overflow", u64::MAX, 1, 0),
        ("backing-overflow", 0, 1, u64::MAX),
        ("backing-out-of-bounds", 0, 2_048, 0),
    ] {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create(format!("dm-striped-range-{name}-test"), None, None)
            .unwrap();
        let params = format!("1 128 510:227 {backing_start}");
        let mut buffer =
            test_buffer(DM_IOCTL_HEADER_SIZE + table_status_record_len(params.len()).unwrap());
        write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
        write_target_spec(
            &mut buffer,
            DM_IOCTL_HEADER_SIZE,
            start,
            length,
            0,
            "striped",
            &params,
        );

        assert_failed_table_load_preserves_state(&mut buffer, &device, Errno::EINVAL);
    }

    unregister(backing_id).unwrap();
}

#[ktest]
fn rejects_linear_range_errors_through_table_load_facade_without_changing_device_state() {
    let backing = StatusBacking::new_with_major(510, 231);
    let backing_id = backing.id();
    register(backing as Arc<dyn BlockDevice>).unwrap();

    for (name, start, length, backing_start) in [
        ("zero", 0, 0, 0),
        ("logical-overflow", u64::MAX, 1, 0),
        ("backing-overflow", 0, 2, u64::MAX),
        ("backing-out-of-bounds", 0, 8, 1_020),
    ] {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create(format!("dm-range-{name}-test"), None, None)
            .unwrap();
        let params = format!("510:231 {backing_start}");
        let mut buffer =
            test_buffer(DM_IOCTL_HEADER_SIZE + table_status_record_len(params.len()).unwrap());
        write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
        write_linear_target_spec(&mut buffer, DM_IOCTL_HEADER_SIZE, start, length, 0, &params);

        assert_failed_table_load_preserves_state(&mut buffer, &device, Errno::EINVAL);
    }

    unregister(backing_id).unwrap();
}

#[ktest]
fn rejects_non_contiguous_linear_tables_through_table_load_facade_without_changing_device_state() {
    let backing = StatusBacking::new_with_major(510, 232);
    let backing_id = backing.id();
    register(backing as Arc<dyn BlockDevice>).unwrap();
    let params = "510:232 0";
    let first_next = table_status_record_len(params.len()).unwrap();

    for (name, first_start, first_len, second_start, second_len) in [
        ("nonzero-first", 1, 4, 5, 4),
        ("gap", 0, 4, 5, 4),
        ("overlap", 0, 8, 4, 4),
    ] {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create(format!("dm-layout-{name}-test"), None, None)
            .unwrap();
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + first_next * 2);
        write_u32(&mut buffer, OFF_TARGET_COUNT, 2).unwrap();
        write_linear_target_spec(
            &mut buffer,
            DM_IOCTL_HEADER_SIZE,
            first_start,
            first_len,
            first_next as u32,
            params,
        );
        write_linear_target_spec(
            &mut buffer,
            DM_IOCTL_HEADER_SIZE + first_next,
            second_start,
            second_len,
            0,
            params,
        );

        assert_failed_table_load_preserves_state(&mut buffer, &device, Errno::EINVAL);
    }

    unregister(backing_id).unwrap();
}

#[ktest]
fn rejects_invalid_target_next_through_table_load_without_changing_device_state() {
    for (name, target_count, next, buffer_len) in [
        ("non-final-zero", 2, 0, DM_IOCTL_HEADER_SIZE + 48),
        ("too-small", 2, 39, DM_IOCTL_HEADER_SIZE + 48),
        ("unaligned", 2, 41, DM_IOCTL_HEADER_SIZE + 48),
        ("out-of-bounds", 2, 56, DM_IOCTL_HEADER_SIZE + 48),
    ] {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create(format!("dm-next-{name}-test"), None, None)
            .unwrap();
        let mut buffer = test_buffer(buffer_len);
        write_u32(&mut buffer, OFF_TARGET_COUNT, target_count).unwrap();
        write_error_target_spec(&mut buffer, DM_IOCTL_HEADER_SIZE, 0, 4, next);

        assert_failed_table_load_preserves_state(&mut buffer, &device, Errno::EINVAL);
    }
}

#[ktest]
fn parses_numeric_backing_device_ids() {
    let id = parse_backing_device_id("8:1").unwrap();
    assert_eq!(id.major().get(), 8);
    assert_eq!(id.minor().get(), 1);

    let id = parse_backing_device_id("4095:1048575").unwrap();
    assert_eq!(id.major().get(), 0x0fff);
    assert_eq!(id.minor().get(), 0x000f_ffff);

    for dev in [
        "",
        "bad",
        ":1",
        "8:",
        "8:1:2",
        "4096:1",
        "65536:1",
        "8:1048576",
        "8:4294967296",
        "-1:0",
        "8:-1",
    ] {
        assert_eq!(
            parse_backing_device_id_with(dev, |path| {
                assert_eq!(path, dev);
                Err(Error::new(Errno::ENOENT))
            })
            .unwrap_err()
            .error(),
            Errno::ENOENT,
            "dev={dev:?}"
        );
    }
}

#[ktest]
fn rejects_invalid_linear_params_without_changing_device_state() {
    for (name, params) in [
        ("missing-dev", ""),
        ("missing-start", "8:1"),
        ("extra-fields", "8:1 0 extra"),
        ("bad-start", "8:1 -1"),
        ("overflow-start", "8:1 18446744073709551616"),
    ] {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create(format!("dm-invalid-linear-{name}-test"), None, None)
            .unwrap();
        let mut buffer =
            test_buffer(DM_IOCTL_HEADER_SIZE + table_status_record_len(params.len()).unwrap());
        write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
        write_linear_target_spec(&mut buffer, DM_IOCTL_HEADER_SIZE, 0, 4, 0, params);

        assert_failed_table_load_preserves_state(&mut buffer, &device, Errno::EINVAL);
    }
}

#[ktest]
fn output_commands_choose_their_own_data_start() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-output-start".to_string(), None, None)
        .unwrap();
    device.load_table(single_linear_table(4, 100, 1));
    let mut status = test_buffer(DM_IOCTL_HEADER_SIZE + 49);
    write_u32(&mut status, OFF_DATA_START, u32::MAX).unwrap();
    write_u32(&mut status, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
    table_status_for_device(&mut status, &device).unwrap();
    assert_eq!(read_u32(&status, OFF_DATA_START).unwrap(), 312);
    assert_eq!(read_u32(&status, OFF_TARGET_COUNT).unwrap(), 1);
    let mut versions = test_buffer(DM_IOCTL_HEADER_SIZE + 128);
    write_u32(&mut versions, OFF_DATA_START, 1).unwrap();
    list_versions(&mut versions).unwrap();
    assert_eq!(read_u32(&versions, OFF_DATA_START).unwrap(), 312);
    assert_eq!(read_u32(&versions, OFF_FLAGS).unwrap(), 0);
}

#[ktest]
fn lists_devices_with_linux_next_offsets_and_uuid_flags() {
    let manager = DmManager::new().unwrap();
    let first = manager
        .create(
            "dm-list-first".to_string(),
            Some(b"dm-list-first-uuid".to_vec()),
            None,
        )
        .unwrap();
    first.load_table(single_linear_table(4, 100, 1));
    let second = manager
        .create("dm-list-second".to_string(), None, None)
        .unwrap();

    let first_name = first.mapper_name();
    let first_uuid = first.uuid().unwrap();
    let (first_extension, first_len) =
        name_list_record_layout(first_name.len() + 1, first_uuid.len() + 1).unwrap();
    let second_name = second.mapper_name();
    let (second_extension, second_len) = name_list_record_layout(second_name.len() + 1, 0).unwrap();
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + first_len + second_len);

    list_devices_for_devices(&mut buffer, vec![first.clone(), second.clone()]).unwrap();

    let first_record = DM_IOCTL_HEADER_SIZE;
    assert_eq!(
        read_u64(&buffer, first_record).unwrap(),
        first.id().as_encoded_u64()
    );
    assert_eq!(
        read_u32(&buffer, first_record + 8).unwrap(),
        first_len as u32
    );
    assert_eq!(
        c_string_until(&buffer, first_record + 12, first_record + first_extension).unwrap(),
        first_name
    );
    assert_eq!(
        read_u32(&buffer, first_record + first_extension).unwrap(),
        first.status().event_nr
    );
    assert_eq!(
        read_u32(&buffer, first_record + first_extension + 4).unwrap(),
        DM_NAME_LIST_FLAG_HAS_UUID
    );
    assert_eq!(
        c_bytes_until(
            &buffer,
            first_record + first_extension + 8,
            first_record + first_len
        )
        .unwrap(),
        first_uuid
    );

    let second_record = DM_IOCTL_HEADER_SIZE + first_len;
    assert_eq!(
        read_u64(&buffer, second_record).unwrap(),
        second.id().as_encoded_u64()
    );
    assert_eq!(read_u32(&buffer, second_record + 8).unwrap(), 0);
    assert_eq!(
        c_string_until(
            &buffer,
            second_record + 12,
            second_record + second_extension
        )
        .unwrap(),
        second_name
    );
    assert_eq!(
        read_u32(&buffer, second_record + second_extension).unwrap(),
        second.status().event_nr
    );
    assert_eq!(
        read_u32(&buffer, second_record + second_extension + 4).unwrap(),
        DM_NAME_LIST_FLAG_DOESNT_HAVE_UUID
    );
}

#[ktest]
fn marks_short_output_buffers_without_overwriting_records() {
    let mut versions = test_buffer(DM_IOCTL_HEADER_SIZE);
    list_versions(&mut versions).unwrap();
    assert_ne!(
        read_u32(&versions, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG,
        0
    );

    let mut target = test_buffer(DM_IOCTL_HEADER_SIZE + 23);
    write_c_string_fixed(&mut target, OFF_NAME, DM_NAME_LEN, "linear").unwrap();
    target[DM_IOCTL_HEADER_SIZE..].fill(0xa5);
    get_target_version(&mut target).unwrap();
    assert_ne!(
        read_u32(&target, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG,
        0
    );
    assert!(
        target[DM_IOCTL_HEADER_SIZE..]
            .iter()
            .all(|byte| *byte == 0xa5)
    );

    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-short-list-device".to_string(), None, None)
        .unwrap();
    let record_len = name_list_record_layout(device.mapper_name().len() + 1, 0)
        .unwrap()
        .1;
    let mut devices = test_buffer(DM_IOCTL_HEADER_SIZE + record_len - 1);
    devices[DM_IOCTL_HEADER_SIZE..].fill(0xa5);
    list_devices_for_devices(&mut devices, vec![device]).unwrap();
    assert_ne!(
        read_u32(&devices, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG,
        0
    );
    assert!(
        devices[DM_IOCTL_HEADER_SIZE..]
            .iter()
            .all(|byte| *byte == 0xa5)
    );
}

#[ktest]
fn lists_error_linear_striped_and_zero_target_versions() {
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 128);
    list_versions(&mut buffer).unwrap();

    for (index, (name, version, next)) in [
        ("error", [1, 6, 0], 24),
        ("linear", [1, 4, 0], 24),
        ("striped", [1, 6, 0], 24),
        ("zero", [1, 1, 0], 0),
    ]
    .into_iter()
    .enumerate()
    {
        let record = DM_IOCTL_HEADER_SIZE + index * 24;
        assert_eq!(read_u32(&buffer, record).unwrap(), next);
        assert_eq!(
            [
                read_u32(&buffer, record + 4).unwrap(),
                read_u32(&buffer, record + 8).unwrap(),
                read_u32(&buffer, record + 12).unwrap(),
            ],
            version
        );
        assert_eq!(
            c_string_until(&buffer, record + 16, record + 24).unwrap(),
            name
        );
    }
}

#[ktest]
fn rejects_partial_target_list_without_writing_records() {
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
    for (name, version) in [
        ("error", [1, 6, 0]),
        ("linear", [1, 4, 0]),
        ("striped", [1, 6, 0]),
        ("zero", [1, 1, 0]),
    ] {
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 32);
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
            c_string_until(&buffer, DM_IOCTL_HEADER_SIZE + 16, buffer.len()).unwrap(),
            name
        );
    }

    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 32);
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
    assert_eq!(DM_READONLY_FLAG, 1 << 0);
    assert_eq!(DM_SKIP_BDGET_FLAG, 1 << 9);
    assert_eq!(DM_NOFLUSH_FLAG, 1 << 11);
    assert_eq!(DM_SECURE_DATA_FLAG, 1 << 15);
    assert_eq!(DM_DEFERRED_REMOVE, 1 << 17);
    assert_eq!(DM_IMA_MEASUREMENT_FLAG, 1 << 19);
    assert_eq!(DM_NAME_LIST_FLAG_HAS_UUID, 1);
    assert_eq!(DM_NAME_LIST_FLAG_DOESNT_HAVE_UUID, 2);
}

#[ktest]
fn final_target_next_is_ignored_but_nonfinal_overlap_is_rejected() {
    for next in [0, 1, 39, 41, u32::MAX] {
        let mut input = request(360);
        write_u32(&mut input, OFF_TARGET_COUNT, 1).unwrap();
        write_u64(&mut input, 320, 8).unwrap();
        write_u32(&mut input, 332, next).unwrap();
        write_c_string_fixed(&mut input, 336, 16, "zero").unwrap();
        assert!(parse_table_load_request(&input).is_ok());
        write_u32(&mut input, OFF_TARGET_COUNT, 2).unwrap();
        assert_eq!(
            parse_table_load_request(&input).err().unwrap().error(),
            Errno::EINVAL
        );
    }
}

#[ktest]
fn name_lists_filter_and_keep_events_without_uuid_output() {
    let manager = DmManager::new().unwrap();
    let first = manager
        .create("v5-list-a".to_string(), Some(b"v5-uuid-a".to_vec()), None)
        .unwrap();
    let second = manager.create("v5-list-b".to_string(), None, None).unwrap();
    first.notify_event();
    for include_uuid in [false, true] {
        let mut output = request(512);
        write_c_string_fixed(&mut output, OFF_NAME, DM_NAME_LEN, "v5-list-").unwrap();
        write_c_string_fixed(&mut output, OFF_UUID, DM_UUID_LEN, "v5-uuid-").unwrap();
        output[305..].fill(0xa5);
        list_devices_for_snapshots(
            &mut output,
            QueryWorkflow::devices(vec![first.clone(), second.clone()]),
            include_uuid,
        )
        .unwrap();
        // Independent byte offsets: name is 10 bytes including NUL, extension
        // is align8(12 + 10) == 24, then event and flags occupy 8 bytes.
        assert_eq!(read_u64(&output, 312).unwrap(), first.id().as_encoded_u64());
        assert_eq!(read_u32(&output, 320).unwrap(), 0);
        assert_eq!(read_u32(&output, 336).unwrap(), 1);
        assert_eq!(read_u32(&output, 340).unwrap(), u32::from(include_uuid));
        assert_eq!(
            read_u32(&output, OFF_DATA_SIZE).unwrap(),
            if include_uuid { 360 } else { 344 }
        );
        assert!(output[305..312].iter().all(|byte| *byte == 0));
    }
    let mut empty = request(320);
    write_c_string_fixed(&mut empty, OFF_NAME, DM_NAME_LEN, "no-match").unwrap();
    empty[312..].fill(0xa5);
    list_devices_for_snapshots(&mut empty, QueryWorkflow::devices(manager.devices()), false)
        .unwrap();
    assert_eq!(read_u32(&empty, OFF_DATA_SIZE).unwrap(), 312);
    assert_eq!(read_u64(&empty, 312).unwrap(), 0);
    // The sentinel is prepared in kernel memory but lies outside Linux's empty
    // response length. Raw C tests check the corresponding copyout boundary.
    let mut short = request(343);
    short[312..].fill(0xa5);
    list_devices_for_snapshots(&mut short, QueryWorkflow::devices(vec![first]), false).unwrap();
    assert_ne!(
        read_u32(&short, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG,
        0
    );
    assert!(short[312..].iter().all(|byte| *byte == 0xa5));
}

#[ktest]
fn target_versions_use_conservative_capacity_not_compact_record_size() {
    for capacity in [28, 29, 30] {
        let mut output = request(312 + capacity);
        write_c_string_fixed(&mut output, OFF_NAME, DM_NAME_LEN, "linear").unwrap();
        output[305..].fill(0xa5);
        get_target_version(&mut output).unwrap();
        let full = read_u32(&output, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG != 0;
        assert_eq!(full, capacity < 30);
        if !full {
            assert_eq!(read_u32(&output, OFF_DATA_SIZE).unwrap(), 342);
            assert!(output[336..342].iter().all(|byte| *byte == 0));
        } else {
            assert!(output[312..].iter().all(|byte| *byte == 0xa5));
        }
    }
}
