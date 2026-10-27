// SPDX-License-Identifier: MPL-2.0

//! Mapper transitions, runtime publication, event waits, and rollback contracts.

use super::*;

#[ktest]
fn remove_unregistration_failure_preserves_device_state() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-remove-unregister-failure".to_string(), None, None)
        .unwrap();
    let status_before = device.status();
    let workflow = ControlWorkflow::new(&manager);

    assert_eq!(
        with_current_device_lifecycle_in(&manager, &device, |device| {
            workflow.remove_with_unregistration(device, |_| {
                Err(Error::with_message(
                    Errno::EIO,
                    "injected unregistration failure",
                ))
            })
        })
        .unwrap_err()
        .error(),
        Errno::EIO
    );

    assert!(Arc::ptr_eq(
        &manager.lookup_name(&device.mapper_name()).unwrap(),
        &device
    ));
    assert_eq!(device.status(), status_before);
}

#[ktest]
fn remove_detaches_current_device_after_runtime_unregistration() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-remove-detach".to_string(), None, None)
        .unwrap();
    let workflow = ControlWorkflow::new(&manager);

    with_current_device_lifecycle_in(&manager, &device, |device| {
        workflow.remove_with_unregistration(device, |_| Ok(()))
    })
    .unwrap();

    assert!(manager.lookup_name(&device.mapper_name()).is_none());
    assert_eq!(device.status().event_nr, 0);
}

#[ktest]
fn remove_all_retains_failed_devices_and_continues_snapshot() {
    let manager = DmManager::new().unwrap();
    let retained = manager
        .create("dm-remove-all-retained".to_string(), None, None)
        .unwrap();
    let removed = manager
        .create("dm-remove-all-removed".to_string(), None, None)
        .unwrap();
    let workflow = ControlWorkflow::new(&manager);
    let outcome = workflow.remove_all(
        RemoveAllRequest::new(vec![retained.clone(), removed.clone()]),
        |device| {
            if Arc::ptr_eq(device, &retained) {
                Err(Error::with_message(Errno::EBUSY, "injected busy mapper"))
            } else {
                with_current_device_lifecycle_in(&manager, device, |device| {
                    workflow.remove_with_unregistration(device, |_| Ok(()))
                })
            }
        },
    );

    assert_eq!(outcome.attempted, 3);
    assert_eq!(outcome.removed, 1);
    assert!(manager.lookup_name(&retained.mapper_name()).is_some());
    assert!(manager.lookup_name(&removed.mapper_name()).is_none());
}

#[ktest]
fn initial_alias_publication_failure_keeps_table_inactive() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-initial-alias-failure-test".to_string(), None, None)
        .unwrap();
    let table = single_linear_table(4, 100, 1);
    device.load_table(table.clone());

    assert_eq!(
        MapperRuntimeCoordinator::new(&manager)
            .activate_initial_with(&device, |id, name| {
                assert_eq!(id, device.id());
                assert_eq!(name, "dm-initial-alias-failure-test");
                Err(Error::with_message(Errno::EIO, "injected alias failure"))
            })
            .unwrap_err()
            .error(),
        Errno::EIO
    );

    assert!(device.active_table().is_none());
    assert!(Arc::ptr_eq(&device.inactive_table().unwrap(), &table));
    assert!(!device.status().suspended);
}

#[ktest]
fn lifecycle_guard_allows_unrelated_mapper_operations_during_drain() {
    let manager = Arc::new(DmManager::new().unwrap());
    let first = manager
        .create("dm-lifecycle-first".to_string(), None, None)
        .unwrap();
    let second = manager
        .create("dm-lifecycle-second".to_string(), None, None)
        .unwrap();
    let deferred = DeferredBacking::new();
    let active = Arc::new(
        DmTable::new_single_linear(
            Sid::new(0),
            128,
            Sid::new(16),
            BlockDeviceLease::new_untracked(deferred.clone()),
        )
        .unwrap(),
    );
    first.load_table(active);
    first.resume().unwrap();
    first.load_table(single_linear_table(64, 32, 2));

    let mut old_batch = io_util::batch::IoBatch::with_capacity(1);
    Bio::new(aster_block::bio::BioType::Flush, Sid::new(0), vec![], None)
        .submit(first.as_ref(), &mut old_batch)
        .unwrap();
    assert!(deferred.has_submitted_bio());

    let suspend_entered = Arc::new(Mutex::new(false));
    let suspend_finished = Arc::new(Mutex::new(false));
    {
        let manager = manager.clone();
        let first = first.clone();
        let suspend_entered = suspend_entered.clone();
        let suspend_finished = suspend_finished.clone();
        TaskOptions::new(move || {
            with_current_device_lifecycle_in(manager.as_ref(), &first, |device| {
                *suspend_entered.lock() = true;
                device.suspend().map_err(map_dm_error)?;
                *suspend_finished.lock() = true;
                Ok(())
            })
            .unwrap();
        })
        .spawn()
        .unwrap();
    }

    while !*suspend_entered.lock() || !first.status().suspended {
        Task::yield_now();
    }
    assert!(!*suspend_finished.lock());

    let mut second_status = test_buffer(DM_IOCTL_HEADER_SIZE);
    device_status_for_device(&mut second_status, &second).unwrap();
    assert_eq!(
        read_u64(&second_status, OFF_DEV).unwrap(),
        second.id().as_encoded_u64()
    );
    assert!(
        manager
            .create("dm-lifecycle-third".to_string(), None, None)
            .is_ok()
    );

    let clear_entered = Arc::new(Mutex::new(false));
    let clear_finished = Arc::new(Mutex::new(false));
    {
        let manager = manager.clone();
        let first = first.clone();
        let clear_entered = clear_entered.clone();
        let clear_finished = clear_finished.clone();
        TaskOptions::new(move || {
            *clear_entered.lock() = true;
            with_current_device_lifecycle_in(manager.as_ref(), &first, |device| {
                device.clear_inactive_table().map_err(map_dm_error)?;
                *clear_finished.lock() = true;
                Ok(())
            })
            .unwrap();
        })
        .spawn()
        .unwrap();
    }

    while !*clear_entered.lock() {
        Task::yield_now();
    }
    Task::yield_now();
    assert!(!*clear_finished.lock());

    deferred.complete();
    while !*suspend_finished.lock() || !*clear_finished.lock() {
        Task::yield_now();
    }
    old_batch.wait_all().unwrap();
    assert!(first.inactive_table().is_none());
}

#[ktest]
fn lifecycle_guard_serializes_running_resume_drain() {
    let manager = Arc::new(DmManager::new().unwrap());
    let device = manager
        .create("dm-lifecycle-resume".to_string(), None, None)
        .unwrap();
    let deferred = DeferredBacking::new();
    let active = Arc::new(
        DmTable::new_single_linear(
            Sid::new(0),
            128,
            Sid::new(16),
            BlockDeviceLease::new_untracked(deferred.clone()),
        )
        .unwrap(),
    );
    let replacement = single_linear_table(64, 32, 2);
    device.load_table(active);
    device.resume().unwrap();
    device.load_table(replacement.clone());

    let mut old_batch = io_util::batch::IoBatch::with_capacity(1);
    Bio::new(aster_block::bio::BioType::Flush, Sid::new(0), vec![], None)
        .submit(device.as_ref(), &mut old_batch)
        .unwrap();
    assert!(deferred.has_submitted_bio());

    let resume_entered = Arc::new(Mutex::new(false));
    let resume_finished = Arc::new(Mutex::new(false));
    {
        let manager = manager.clone();
        let device = device.clone();
        let resume_entered = resume_entered.clone();
        let resume_finished = resume_finished.clone();
        TaskOptions::new(move || {
            with_current_device_lifecycle_in(manager.as_ref(), &device, |device| {
                *resume_entered.lock() = true;
                device.resume().map_err(map_dm_error)?;
                *resume_finished.lock() = true;
                Ok(())
            })
            .unwrap();
        })
        .spawn()
        .unwrap();
    }

    while !*resume_entered.lock() || !device.status().suspended {
        Task::yield_now();
    }
    assert!(!*resume_finished.lock());

    let clear_entered = Arc::new(Mutex::new(false));
    let clear_finished = Arc::new(Mutex::new(false));
    {
        let manager = manager.clone();
        let device = device.clone();
        let clear_entered = clear_entered.clone();
        let clear_finished = clear_finished.clone();
        TaskOptions::new(move || {
            *clear_entered.lock() = true;
            with_current_device_lifecycle_in(manager.as_ref(), &device, |device| {
                device.clear_inactive_table().map_err(map_dm_error)?;
                *clear_finished.lock() = true;
                Ok(())
            })
            .unwrap();
        })
        .spawn()
        .unwrap();
    }

    while !*clear_entered.lock() {
        Task::yield_now();
    }
    Task::yield_now();
    assert!(!*clear_finished.lock());

    deferred.complete();
    while !*resume_finished.lock() || !*clear_finished.lock() {
        Task::yield_now();
    }
    old_batch.wait_all().unwrap();
    assert!(Arc::ptr_eq(&device.active_table().unwrap(), &replacement));
}

#[ktest]
fn lifecycle_guard_rejects_device_removed_before_operation() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-lifecycle-stale".to_string(), None, None)
        .unwrap();
    manager.remove(&device.mapper_name()).unwrap();

    assert_eq!(
        with_current_device_lifecycle_in(&manager, &device, |_| Ok(()))
            .unwrap_err()
            .error(),
        Errno::ENXIO
    );
}

#[ktest]
fn persistent_create_reports_allocation_before_identity_errors() {
    let manager = DmManager::new().unwrap();
    let workflow = ControlWorkflow::new(&manager);
    let device = workflow
        .create(CreateRequest {
            name: "dm-create-order".to_string(),
            uuid: Some(b"dm-create-order-uuid".to_vec()),
            requested_minor: Some(7),
        })
        .unwrap();
    let before = device.status();
    for (name, uuid, minor, expected) in [
        ("dm-create-new", None, 7, Errno::ENXIO),
        ("dm-create-order", None, 7, Errno::ENXIO),
        (
            "dm-create-new",
            Some("dm-create-order-uuid"),
            7,
            Errno::ENXIO,
        ),
        ("dm-create-order", None, 8, Errno::EBUSY),
        (
            "dm-create-new",
            Some("dm-create-order-uuid"),
            8,
            Errno::EBUSY,
        ),
    ] {
        let result = workflow.create(CreateRequest {
            name: name.to_string(),
            uuid: uuid.map(|value| value.as_bytes().to_vec()),
            requested_minor: Some(minor),
        });
        assert_eq!(result.unwrap_err().error(), expected);
        assert_eq!(manager.devices().len(), 1);
        assert_eq!(device.status(), before);
        assert!(Arc::ptr_eq(
            &manager.lookup_name("dm-create-order").unwrap(),
            &device
        ));
        assert!(Arc::ptr_eq(
            &manager.lookup_uuid("dm-create-order-uuid").unwrap(),
            &device
        ));
        assert!(manager.lookup_name("dm-create-new").is_none());
    }
    let retry = workflow
        .create(CreateRequest {
            name: "dm-create-new".to_string(),
            uuid: None,
            requested_minor: Some(8),
        })
        .unwrap();
    assert_eq!(retry.id().minor().get(), 8);
    assert!(!MapperRuntimeCoordinator::new(&manager).is_registered(&retry));
    assert_eq!(
        map_create_error(DmError::MinorExhausted).error(),
        Errno::ENXIO
    );
}

#[ktest]
fn create_device_does_not_persist_readonly_flag_without_a_table() {
    DM_MANAGER.call_once(|| DmManager::new().unwrap());
    let name = "dm-readonly-create-test";
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
    write_u32(&mut buffer, OFF_FLAGS, DM_READONLY_FLAG).unwrap();
    write_c_string_fixed(&mut buffer, OFF_NAME, DM_NAME_LEN, name).unwrap();

    create_device(&mut buffer).unwrap();
    let device = manager().lookup_name(name).unwrap();

    assert!(!device.is_readonly());
    assert!(!device.status().readonly);
    assert_eq!(read_u32(&buffer, OFF_FLAGS).unwrap(), 0);

    assert_eq!(block_open_count(device.id()), None);

    manager().remove(name).unwrap();
}

#[ktest]
fn resume_transition_activates_inactive_table_for_active_queries() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-resume-activates-table-test".to_string(), None, None)
        .unwrap();
    device.load_table(single_linear_table(4, 100, 1));

    let mut active_before = test_buffer(DM_IOCTL_HEADER_SIZE + 49);
    table_status_for_device(&mut active_before, &device).unwrap();
    assert_eq!(read_u32(&active_before, OFF_TARGET_COUNT).unwrap(), 0);

    let mut inactive_before = test_buffer(DM_IOCTL_HEADER_SIZE + 49);
    write_u32(
        &mut inactive_before,
        OFF_FLAGS,
        DM_QUERY_INACTIVE_TABLE_FLAG,
    )
    .unwrap();
    table_status_for_device(&mut inactive_before, &device).unwrap();
    assert_eq!(read_u32(&inactive_before, OFF_TARGET_COUNT).unwrap(), 1);
    assert_eq!(device.metadata().nr_sectors, 0);

    device.resume().unwrap();
    assert!(device.active_table().is_some());
    assert!(device.inactive_table().is_none());
    assert_eq!(device.metadata().nr_sectors, 4);

    let mut active_after = test_buffer(DM_IOCTL_HEADER_SIZE + 49);
    table_status_for_device(&mut active_after, &device).unwrap();
    assert_eq!(read_u32(&active_after, OFF_TARGET_COUNT).unwrap(), 1);
    assert_eq!(
        read_u64(&active_after, DM_IOCTL_HEADER_SIZE + 8).unwrap(),
        4
    );

    let mut inactive_after = test_buffer(DM_IOCTL_HEADER_SIZE + 49);
    write_u32(&mut inactive_after, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
    table_status_for_device(&mut inactive_after, &device).unwrap();
    assert_eq!(read_u32(&inactive_after, OFF_TARGET_COUNT).unwrap(), 0);
}

#[ktest]
fn resume_transition_replaces_active_table_while_running() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-resume-replaces-table-test".to_string(), None, None)
        .unwrap();
    device.load_table(single_linear_table(4, 100, 1));
    device.resume().unwrap();

    device.load_table(single_linear_table(8, 200, 2));
    let mut inactive = test_buffer(DM_IOCTL_HEADER_SIZE + 49);
    write_u32(&mut inactive, OFF_FLAGS, DM_QUERY_INACTIVE_TABLE_FLAG).unwrap();
    table_status_for_device(&mut inactive, &device).unwrap();
    assert_eq!(read_u32(&inactive, OFF_TARGET_COUNT).unwrap(), 1);
    assert_eq!(read_u64(&inactive, DM_IOCTL_HEADER_SIZE + 8).unwrap(), 8);

    device.resume().unwrap();
    assert!(device.active_table().is_some());
    assert!(device.inactive_table().is_none());

    let mut active = test_buffer(DM_IOCTL_HEADER_SIZE + 49);
    write_u32(&mut active, OFF_FLAGS, DM_STATUS_TABLE_FLAG).unwrap();
    table_status_for_device(&mut active, &device).unwrap();
    assert_eq!(read_u32(&active, OFF_TARGET_COUNT).unwrap(), 1);
    assert_eq!(read_u64(&active, DM_IOCTL_HEADER_SIZE + 8).unwrap(), 8);
    assert_eq!(
        c_string_until(
            &active,
            DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE,
            DM_IOCTL_HEADER_SIZE + 48
        )
        .unwrap(),
        "1:2 200"
    );
}

#[ktest]
fn device_wait_for_device_returns_updated_event_header() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-wait-event-test".to_string(), None, None)
        .unwrap();
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
    write_u32(&mut buffer, OFF_EVENT_NR, 0).unwrap();

    device.load_table(single_linear_table(4, 100, 1));
    device.notify_event();
    device_wait_for_device(&mut buffer, &device).unwrap();

    assert_eq!(read_u32(&buffer, OFF_EVENT_NR).unwrap(), 1);
    assert_eq!(
        read_u32(&buffer, OFF_FLAGS).unwrap(),
        DM_INACTIVE_PRESENT_FLAG
    );
}

#[ktest]
fn device_wait_returns_current_header_after_rename_events() {
    DM_MANAGER.call_once(|| DmManager::new().unwrap());
    let device = manager()
        .create("dm-wait-rename-old".to_string(), None, None)
        .unwrap();
    device.load_table(single_linear_table(4, 100, 1));
    device.resume().unwrap();
    let id = device.id();

    let mut rename = test_buffer(DM_IOCTL_HEADER_SIZE + 32);
    write_c_string_fixed(&mut rename, OFF_NAME, DM_NAME_LEN, "dm-wait-rename-old").unwrap();
    write_c_string(&mut rename, DM_IOCTL_HEADER_SIZE, "dm-wait-rename-new").unwrap();
    device_rename(&mut rename).unwrap();

    let mut name_wait = test_buffer(DM_IOCTL_HEADER_SIZE);
    write_u64(&mut name_wait, OFF_DEV, id.as_encoded_u64()).unwrap();
    write_u32(&mut name_wait, OFF_EVENT_NR, 0).unwrap();
    device_wait(&mut name_wait).unwrap();
    assert_eq!(read_u32(&name_wait, OFF_EVENT_NR).unwrap(), 1);
    assert_eq!(
        required_c_string(&name_wait, OFF_NAME, DM_NAME_LEN).unwrap(),
        "dm-wait-rename-new"
    );
    assert_eq!(
        optional_c_string(&name_wait, OFF_UUID, DM_UUID_LEN).unwrap(),
        None
    );

    let mut set_uuid = test_buffer(DM_IOCTL_HEADER_SIZE + 32);
    write_u32(&mut set_uuid, OFF_FLAGS, DM_UUID_FLAG).unwrap();
    write_c_string_fixed(&mut set_uuid, OFF_NAME, DM_NAME_LEN, "dm-wait-rename-new").unwrap();
    write_c_string(
        &mut set_uuid,
        DM_IOCTL_HEADER_SIZE,
        "dm-wait-rename-uuid-new",
    )
    .unwrap();
    device_rename(&mut set_uuid).unwrap();

    let mut uuid_wait = test_buffer(DM_IOCTL_HEADER_SIZE);
    write_u64(&mut uuid_wait, OFF_DEV, id.as_encoded_u64()).unwrap();
    write_u32(&mut uuid_wait, OFF_EVENT_NR, 1).unwrap();
    device_wait(&mut uuid_wait).unwrap();
    assert_eq!(read_u32(&uuid_wait, OFF_EVENT_NR).unwrap(), 2);
    assert_eq!(
        required_c_string(&uuid_wait, OFF_NAME, DM_NAME_LEN).unwrap(),
        "dm-wait-rename-new"
    );
    assert_eq!(
        required_c_string(&uuid_wait, OFF_UUID, DM_UUID_LEN).unwrap(),
        "dm-wait-rename-uuid-new"
    );
}

#[ktest]
fn blocking_device_wait_returns_updated_header_after_rename_and_setuuid() {
    DM_MANAGER.call_once(|| DmManager::new().unwrap());
    let device = manager()
        .create("dm-blocking-wait-old".to_string(), None, None)
        .unwrap();
    device.load_table(single_linear_table(4, 100, 1));
    device.resume().unwrap();
    let id = device.id();
    let first_started = Arc::new(Mutex::new(false));
    let first_finished = Arc::new(Mutex::new(false));
    let first_header = Arc::new(Mutex::new(None));
    {
        let first_started = first_started.clone();
        let first_finished = first_finished.clone();
        let first_header = first_header.clone();
        let waiter = device.clone();
        TaskOptions::new(move || {
            let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
            write_u64(&mut buffer, OFF_DEV, id.as_encoded_u64()).unwrap();
            write_u32(&mut buffer, OFF_EVENT_NR, 0).unwrap();
            let mut pending_checks = 0;
            device_wait_with_pending(&mut buffer, &waiter, || {
                pending_checks += 1;
                // The second condition check runs after waker registration.
                if pending_checks >= 2 {
                    *first_started.lock() = true;
                }
            })
            .unwrap();
            *first_header.lock() = Some(buffer);
            *first_finished.lock() = true;
        })
        .spawn()
        .unwrap();
    }
    while !*first_started.lock() {
        Task::yield_now();
    }
    assert!(!*first_finished.lock());

    let mut rename = test_buffer(DM_IOCTL_HEADER_SIZE + 32);
    write_c_string_fixed(&mut rename, OFF_NAME, DM_NAME_LEN, "dm-blocking-wait-old").unwrap();
    write_c_string(&mut rename, DM_IOCTL_HEADER_SIZE, "dm-blocking-wait-new").unwrap();
    device_rename(&mut rename).unwrap();

    while !*first_finished.lock() {
        Task::yield_now();
    }
    let first_header = first_header.lock().take().unwrap();
    assert_eq!(read_u32(&first_header, OFF_EVENT_NR).unwrap(), 1);
    assert_eq!(
        required_c_string(&first_header, OFF_NAME, DM_NAME_LEN).unwrap(),
        "dm-blocking-wait-new"
    );
    assert_eq!(
        optional_c_string(&first_header, OFF_UUID, DM_UUID_LEN).unwrap(),
        None
    );

    let second_started = Arc::new(Mutex::new(false));
    let second_finished = Arc::new(Mutex::new(false));
    let second_header = Arc::new(Mutex::new(None));
    {
        let second_started = second_started.clone();
        let second_finished = second_finished.clone();
        let second_header = second_header.clone();
        let waiter = device.clone();
        TaskOptions::new(move || {
            let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
            write_u64(&mut buffer, OFF_DEV, id.as_encoded_u64()).unwrap();
            write_u32(&mut buffer, OFF_EVENT_NR, 1).unwrap();
            let mut pending_checks = 0;
            device_wait_with_pending(&mut buffer, &waiter, || {
                pending_checks += 1;
                if pending_checks >= 2 {
                    *second_started.lock() = true;
                }
            })
            .unwrap();
            *second_header.lock() = Some(buffer);
            *second_finished.lock() = true;
        })
        .spawn()
        .unwrap();
    }
    while !*second_started.lock() {
        Task::yield_now();
    }
    assert!(!*second_finished.lock());

    let mut set_uuid = test_buffer(DM_IOCTL_HEADER_SIZE + 32);
    write_u32(&mut set_uuid, OFF_FLAGS, DM_UUID_FLAG).unwrap();
    write_c_string_fixed(&mut set_uuid, OFF_NAME, DM_NAME_LEN, "dm-blocking-wait-new").unwrap();
    write_c_string(
        &mut set_uuid,
        DM_IOCTL_HEADER_SIZE,
        "dm-blocking-wait-uuid-new",
    )
    .unwrap();
    device_rename(&mut set_uuid).unwrap();

    while !*second_finished.lock() {
        Task::yield_now();
    }
    let second_header = second_header.lock().take().unwrap();
    assert_eq!(read_u32(&second_header, OFF_EVENT_NR).unwrap(), 2);
    assert_eq!(
        required_c_string(&second_header, OFF_NAME, DM_NAME_LEN).unwrap(),
        "dm-blocking-wait-new"
    );
    assert_eq!(
        required_c_string(&second_header, OFF_UUID, DM_UUID_LEN).unwrap(),
        "dm-blocking-wait-uuid-new"
    );

    manager().remove("dm-blocking-wait-new").unwrap();
}

#[ktest]
fn device_wait_maps_interrupt_to_restartsys() {
    assert_eq!(
        restart_interrupted_wait(Error::new(Errno::EINTR)).error(),
        Errno::ERESTARTSYS
    );
    assert_eq!(
        restart_interrupted_wait(Error::new(Errno::ENXIO)).error(),
        Errno::ENXIO
    );
}

#[ktest]
fn rename_runtime_updates_manager_and_device_name() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create(
            "dm-rename-old".to_string(),
            Some(b"dm-rename-uuid".to_vec()),
            None,
        )
        .unwrap();
    let id = device.id();

    MapperRuntimeCoordinator::new(&manager)
        .rename_with(&device, "dm-rename-new", |rename_id, old, new| {
            assert_eq!(rename_id, id);
            assert_eq!(old, "dm-rename-old");
            assert_eq!(new, "dm-rename-new");
            Ok(())
        })
        .unwrap();

    assert!(manager.lookup_name("dm-rename-old").is_none());
    assert_eq!(manager.lookup_name("dm-rename-new").unwrap().id(), id);
    assert_eq!(manager.lookup_uuid("dm-rename-uuid").unwrap().id(), id);
    assert_eq!(device.mapper_name(), "dm-rename-new");
}

#[ktest]
fn rename_runtime_rejects_same_name_without_state_change() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-rename-same".to_string(), None, None)
        .unwrap();
    let status_before = device.status();
    let mut called = false;

    assert_eq!(
        MapperRuntimeCoordinator::new(&manager)
            .rename_with(&device, "dm-rename-same", |_, _, _| {
                called = true;
                Ok(())
            })
            .unwrap_err()
            .error(),
        Errno::EBUSY
    );

    assert!(!called);
    assert_eq!(
        manager.lookup_name("dm-rename-same").unwrap().id(),
        device.id()
    );
    assert_eq!(device.mapper_name(), "dm-rename-same");
    assert_eq!(device.status(), status_before);
}

#[ktest]
fn rename_runtime_rolls_back_manager_when_alias_rename_fails() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create(
            "dm-rollback-old".to_string(),
            Some(b"dm-rollback-uuid".to_vec()),
            None,
        )
        .unwrap();
    device.load_table(single_linear_table(4, 100, 1));
    let status_before = device.status();
    let id = device.id();

    assert_eq!(
        MapperRuntimeCoordinator::new(&manager)
            .rename_with(&device, "dm-rollback-new", |rename_id, old, new| {
                assert_eq!(rename_id, id);
                assert_eq!(old, "dm-rollback-old");
                assert_eq!(new, "dm-rollback-new");
                assert_eq!(
                    manager
                        .create("dm-rollback-old".to_string(), None, None)
                        .unwrap_err(),
                    DmError::NameExists
                );
                assert_eq!(
                    manager
                        .create("dm-rollback-new".to_string(), None, None)
                        .unwrap_err(),
                    DmError::NameExists
                );
                assert!(
                    manager
                        .create("dm-rollback-unrelated".to_string(), None, None)
                        .is_ok()
                );
                Err(Error::with_message(
                    Errno::EEXIST,
                    "mapper alias already exists",
                ))
            })
            .unwrap_err()
            .error(),
        Errno::EEXIST
    );

    assert_eq!(manager.lookup_name("dm-rollback-old").unwrap().id(), id);
    assert!(manager.lookup_name("dm-rollback-new").is_none());
    assert_eq!(manager.lookup_uuid("dm-rollback-uuid").unwrap().id(), id);
    assert_eq!(device.mapper_name(), "dm-rollback-old");
    assert_eq!(device.status(), status_before);
    assert!(
        manager
            .create("dm-rollback-new".to_string(), None, None)
            .is_ok()
    );
}

#[ktest]
fn device_rename_with_uuid_flag_updates_uuid_not_name() {
    DM_MANAGER.call_once(|| DmManager::new().unwrap());
    let device = manager()
        .create("dm-uuid-rename-name".to_string(), None, None)
        .unwrap();
    let status_before = device.status();
    let id = device.id();
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + 32);
    write_u32(&mut buffer, OFF_FLAGS, DM_UUID_FLAG).unwrap();
    write_c_string_fixed(&mut buffer, OFF_NAME, DM_NAME_LEN, "dm-uuid-rename-name").unwrap();
    write_c_string(&mut buffer, DM_IOCTL_HEADER_SIZE, "dm-uuid-rename-new").unwrap();

    device_rename(&mut buffer).unwrap();

    assert_eq!(device.mapper_name(), "dm-uuid-rename-name");
    assert_eq!(device.uuid().unwrap(), b"dm-uuid-rename-new");
    assert!(manager().lookup_uuid("dm-uuid-rename-old").is_none());
    assert_eq!(
        manager().lookup_uuid("dm-uuid-rename-new").unwrap().id(),
        id
    );
    assert_eq!(
        manager().lookup_name("dm-uuid-rename-name").unwrap().id(),
        id
    );
    let status_after = device.status();
    assert_eq!(status_after.event_nr, status_before.event_nr);
    assert_eq!(status_after.suspended, status_before.suspended);
    assert_eq!(
        status_after.has_active_table,
        status_before.has_active_table
    );
    assert_eq!(
        status_after.has_inactive_table,
        status_before.has_inactive_table
    );
    assert_eq!(read_u64(&buffer, OFF_DEV).unwrap(), id.as_encoded_u64());
    assert_eq!(
        required_c_string(&buffer, OFF_UUID, DM_UUID_LEN).unwrap(),
        "dm-uuid-rename-new"
    );
}

#[ktest]
fn command_errors_preserve_linux_lookup_and_validation_order() {
    DM_MANAGER.call_once(|| DmManager::new().unwrap());
    let name = "dm-error-order-existing";
    let uuid = "dm-error-order-uuid";
    let missing = "dm-error-order-missing";
    let device = manager()
        .create(name.to_string(), Some(uuid.as_bytes().to_vec()), None)
        .unwrap();
    let before = device.status();

    // Linux dev_rename validates the destination, then dm_hash_rename checks
    // destination collisions before looking up the source by name.
    for (flags, source, destination, expected) in [
        (0, missing, "bad/name", Errno::EINVAL),
        (0, missing, name, Errno::EBUSY),
        (0, missing, "dm-error-order-unused", Errno::ENXIO),
        (DM_UUID_FLAG, missing, "", Errno::EINVAL),
        (DM_UUID_FLAG, missing, uuid, Errno::EBUSY),
        (
            DM_UUID_FLAG,
            missing,
            "dm-error-order-unused-uuid",
            Errno::ENXIO,
        ),
        (DM_UUID_FLAG, name, uuid, Errno::EBUSY),
        (
            DM_UUID_FLAG,
            name,
            "dm-error-order-unused-uuid",
            Errno::EINVAL,
        ),
    ] {
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + DM_UUID_LEN);
        write_u32(&mut buffer, OFF_FLAGS, flags).unwrap();
        write_c_string_fixed(&mut buffer, OFF_NAME, DM_NAME_LEN, source).unwrap();
        write_c_string(&mut buffer, DM_IOCTL_HEADER_SIZE, destination).unwrap();
        assert_eq!(device_rename(&mut buffer).unwrap_err().error(), expected);
        assert_eq!(device.status(), before);
        assert_eq!(device.mapper_name(), name);
        assert_eq!(device.uuid().as_deref(), Some(uuid.as_bytes()));
        assert!(Arc::ptr_eq(&manager().lookup_name(name).unwrap(), &device));
        assert!(Arc::ptr_eq(&manager().lookup_uuid(uuid).unwrap(), &device));
    }

    // Unlike rename, TABLE_LOAD resolves the source before parsing its payload.
    // The same empty table is ENXIO for a missing source and EINVAL for this one.
    for (source, expected) in [(missing, Errno::ENXIO), (name, Errno::EINVAL)] {
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
        write_c_string_fixed(&mut buffer, OFF_NAME, DM_NAME_LEN, source).unwrap();
        assert_eq!(
            handle_command(DM_TABLE_LOAD_CMD, &mut buffer)
                .unwrap_err()
                .error(),
            expected
        );
        assert_eq!(device.status(), before);
        assert!(device.active_table().is_none());
        assert!(device.inactive_table().is_none());
        assert!(!MapperRuntimeCoordinator::new(manager()).is_registered(&device));
    }
    manager().remove(name).unwrap();
}

#[ktest]
fn device_rename_rejects_duplicate_and_empty_uuid_without_state_change() {
    DM_MANAGER.call_once(|| DmManager::new().unwrap());
    let first = manager()
        .create(
            "dm-uuid-rename-first".to_string(),
            Some(b"dm-uuid-rename-first-old".to_vec()),
            None,
        )
        .unwrap();
    let second = manager()
        .create(
            "dm-uuid-rename-second".to_string(),
            Some(b"dm-uuid-rename-second-uuid".to_vec()),
            None,
        )
        .unwrap();
    let id = first.id();
    let status_before = first.status();

    let mut duplicate = test_buffer(DM_IOCTL_HEADER_SIZE + 32);
    write_u32(&mut duplicate, OFF_FLAGS, DM_UUID_FLAG).unwrap();
    write_c_string_fixed(&mut duplicate, OFF_NAME, DM_NAME_LEN, first.mapper_name()).unwrap();
    write_c_string(
        &mut duplicate,
        DM_IOCTL_HEADER_SIZE,
        "dm-uuid-rename-second-uuid",
    )
    .unwrap();
    assert_eq!(
        device_rename(&mut duplicate).unwrap_err().error(),
        Errno::EBUSY
    );

    assert_eq!(first.uuid().unwrap(), b"dm-uuid-rename-first-old");
    assert_eq!(first.status(), status_before);
    assert_eq!(
        manager()
            .lookup_uuid("dm-uuid-rename-first-old")
            .unwrap()
            .id(),
        id
    );
    assert_eq!(
        manager()
            .lookup_uuid("dm-uuid-rename-second-uuid")
            .unwrap()
            .id(),
        second.id()
    );

    let mut empty = test_buffer(DM_IOCTL_HEADER_SIZE + 1);
    write_u32(&mut empty, OFF_FLAGS, DM_UUID_FLAG).unwrap();
    write_c_string_fixed(&mut empty, OFF_NAME, DM_NAME_LEN, first.mapper_name()).unwrap();
    write_c_string(&mut empty, DM_IOCTL_HEADER_SIZE, "").unwrap();
    assert_eq!(
        device_rename(&mut empty).unwrap_err().error(),
        Errno::EINVAL
    );
    assert_eq!(first.uuid().unwrap(), b"dm-uuid-rename-first-old");
}

#[ktest]
fn primary_node_creation_failure_keeps_first_table_unpublished() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-primary-node-failure-test".to_string(), None, None)
        .unwrap();
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + table_status_record_len(0).unwrap());
    write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
    write_error_target_spec(&mut buffer, DM_IOCTL_HEADER_SIZE, 0, 8, 0);
    let status_before = device.status();

    assert_eq!(
        table_load_for_device_with_primary(&mut buffer, &device, |device| {
            register_block_mapper_primary_with_node_creator(device.clone(), |_| {
                Err(Error::with_message(
                    Errno::EIO,
                    "injected primary node failure",
                ))
            })
        })
        .unwrap_err()
        .error(),
        Errno::EIO
    );

    assert_eq!(device.status(), status_before);
    assert!(device.active_table().is_none());
    assert!(device.inactive_table().is_none());
    assert_eq!(block_open_count(device.id()), None);
}

#[ktest]
fn readonly_table_mode_remains_inactive_until_resume() {
    let backing = StatusBacking::new_with_major(510, 201);
    let backing_id = backing.id();
    register(backing as Arc<dyn BlockDevice>).unwrap();

    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-readonly-table-load-test".to_string(), None, None)
        .unwrap();
    let record_len = table_status_record_len("510:201 0".len()).unwrap();
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + record_len);
    write_u32(&mut buffer, OFF_FLAGS, DM_READONLY_FLAG).unwrap();
    write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
    write_linear_target_spec(&mut buffer, DM_IOCTL_HEADER_SIZE, 0, 4, 0, "510:201 0");

    table_load_for_test(&mut buffer, &device).unwrap();

    assert!(!device.is_readonly());
    assert!(!device.status().readonly);
    assert!(device.inactive_table().unwrap().is_readonly());
    assert_eq!(
        read_u32(&buffer, OFF_FLAGS).unwrap(),
        DM_INACTIVE_PRESENT_FLAG
    );

    let mut inactive_status = test_buffer(DM_IOCTL_HEADER_SIZE);
    write_u32(
        &mut inactive_status,
        OFF_FLAGS,
        DM_QUERY_INACTIVE_TABLE_FLAG,
    )
    .unwrap();
    device_status_for_device(&mut inactive_status, &device).unwrap();
    assert_eq!(
        read_u32(&inactive_status, OFF_FLAGS).unwrap(),
        DM_QUERY_INACTIVE_TABLE_FLAG | DM_READONLY_FLAG | DM_INACTIVE_PRESENT_FLAG
    );

    device.resume().unwrap();
    let mut active_status = test_buffer(DM_IOCTL_HEADER_SIZE);
    device_status_for_device(&mut active_status, &device).unwrap();
    assert!(device.is_readonly());
    assert_eq!(
        read_u32(&active_status, OFF_FLAGS).unwrap(),
        DM_READONLY_FLAG | DM_ACTIVE_PRESENT_FLAG
    );

    manager.remove("dm-readonly-table-load-test").unwrap();
    drop(device);
    unregister(backing_id).unwrap();
}

#[ktest]
fn inactive_table_lease_follows_replacement_and_clear() {
    let first = StatusBacking::new_with_major(510, 246);
    let second = StatusBacking::new_with_major(510, 247);
    let first_id = first.id();
    let second_id = second.id();
    register(first as Arc<dyn BlockDevice>).unwrap();
    register(second as Arc<dyn BlockDevice>).unwrap();

    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-inactive-lease-lifecycle-test".to_string(), None, None)
        .unwrap();

    let first_params = "510:246 0";
    let mut first_load =
        test_buffer(DM_IOCTL_HEADER_SIZE + table_status_record_len(first_params.len()).unwrap());
    write_u32(&mut first_load, OFF_TARGET_COUNT, 1).unwrap();
    write_linear_target_spec(&mut first_load, DM_IOCTL_HEADER_SIZE, 0, 4, 0, first_params);
    table_load_for_test(&mut first_load, &device).unwrap();

    assert!(!device.status().has_active_table);
    assert!(device.status().has_inactive_table);
    assert_eq!(unregister(first_id).unwrap_err(), aster_block::Error::Busy);

    let second_params = "510:247 0";
    let mut second_load =
        test_buffer(DM_IOCTL_HEADER_SIZE + table_status_record_len(second_params.len()).unwrap());
    write_u32(&mut second_load, OFF_TARGET_COUNT, 1).unwrap();
    write_linear_target_spec(
        &mut second_load,
        DM_IOCTL_HEADER_SIZE,
        0,
        4,
        0,
        second_params,
    );
    table_load_for_test(&mut second_load, &device).unwrap();

    unregister(first_id).unwrap();
    assert_eq!(unregister(second_id).unwrap_err(), aster_block::Error::Busy);

    device.clear_inactive_table().unwrap();
    assert!(!device.status().has_inactive_table);
    unregister(second_id).unwrap();
}

#[ktest]
fn failed_table_load_preserves_nonempty_state_and_releases_temporary_leases() {
    for failure in ["missing", "capacity", "continuity"] {
        let active = StatusBacking::new_with_major(510, 248);
        let inactive = StatusBacking::new_with_major(510, 249);
        let temporary = StatusBacking::new_with_major(510, 250);
        let active_id = active.id();
        let inactive_id = inactive.id();
        let temporary_id = temporary.id();
        register(active as Arc<dyn BlockDevice>).unwrap();
        register(inactive as Arc<dyn BlockDevice>).unwrap();
        register(temporary as Arc<dyn BlockDevice>).unwrap();

        let manager = DmManager::new().unwrap();
        let device = manager
            .create(format!("dm-preserve-{failure}-test"), None, None)
            .unwrap();

        let active_params = "510:248 0";
        let mut active_load = test_buffer(
            DM_IOCTL_HEADER_SIZE + table_status_record_len(active_params.len()).unwrap(),
        );
        write_u32(&mut active_load, OFF_FLAGS, DM_READONLY_FLAG).unwrap();
        write_u32(&mut active_load, OFF_TARGET_COUNT, 1).unwrap();
        write_linear_target_spec(
            &mut active_load,
            DM_IOCTL_HEADER_SIZE,
            0,
            4,
            0,
            active_params,
        );
        table_load_for_test(&mut active_load, &device).unwrap();
        device.resume().unwrap();

        let inactive_params = "510:249 0";
        let mut inactive_load = test_buffer(
            DM_IOCTL_HEADER_SIZE + table_status_record_len(inactive_params.len()).unwrap(),
        );
        write_u32(&mut inactive_load, OFF_TARGET_COUNT, 1).unwrap();
        write_linear_target_spec(
            &mut inactive_load,
            DM_IOCTL_HEADER_SIZE,
            0,
            4,
            0,
            inactive_params,
        );
        table_load_for_test(&mut inactive_load, &device).unwrap();
        device.suspend().unwrap();

        assert!(device.status().has_active_table);
        assert!(device.status().has_inactive_table);
        assert!(device.status().readonly);
        assert!(device.status().suspended);
        assert_eq!(unregister(active_id).unwrap_err(), aster_block::Error::Busy);
        assert_eq!(
            unregister(inactive_id).unwrap_err(),
            aster_block::Error::Busy
        );

        let (mut failed_load, errno) = match failure {
            "missing" => {
                let params = "2 4 510:250 0 510:251 0";
                let mut buffer = test_buffer(
                    DM_IOCTL_HEADER_SIZE + table_status_record_len(params.len()).unwrap(),
                );
                write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
                write_target_spec(
                    &mut buffer,
                    DM_IOCTL_HEADER_SIZE,
                    0,
                    8,
                    0,
                    "striped",
                    params,
                );
                (buffer, Errno::ENODEV)
            }
            "capacity" => {
                let params = "510:250 1020";
                let mut buffer = test_buffer(
                    DM_IOCTL_HEADER_SIZE + table_status_record_len(params.len()).unwrap(),
                );
                write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
                write_linear_target_spec(&mut buffer, DM_IOCTL_HEADER_SIZE, 0, 8, 0, params);
                (buffer, Errno::EINVAL)
            }
            "continuity" => {
                let params = "510:250 0";
                let first_next = table_status_record_len(params.len()).unwrap();
                let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE + first_next * 2);
                write_u32(&mut buffer, OFF_TARGET_COUNT, 2).unwrap();
                write_linear_target_spec(
                    &mut buffer,
                    DM_IOCTL_HEADER_SIZE,
                    0,
                    4,
                    first_next as u32,
                    params,
                );
                write_linear_target_spec(
                    &mut buffer,
                    DM_IOCTL_HEADER_SIZE + first_next,
                    5,
                    4,
                    0,
                    params,
                );
                (buffer, Errno::EINVAL)
            }
            _ => unreachable!(),
        };

        assert_failed_table_load_preserves_state(&mut failed_load, &device, errno);
        device.suspend().unwrap();
        assert_eq!(unregister(active_id).unwrap_err(), aster_block::Error::Busy);
        assert_eq!(
            unregister(inactive_id).unwrap_err(),
            aster_block::Error::Busy
        );
        let temporary = unregister(temporary_id).unwrap();

        device.clear_inactive_table().unwrap();
        unregister(inactive_id).unwrap();
        assert_eq!(unregister(active_id).unwrap_err(), aster_block::Error::Busy);
        device.resume().unwrap();

        register(temporary).unwrap();
        let temporary_params = "510:250 0";
        let mut replacement_load = test_buffer(
            DM_IOCTL_HEADER_SIZE + table_status_record_len(temporary_params.len()).unwrap(),
        );
        write_u32(&mut replacement_load, OFF_TARGET_COUNT, 1).unwrap();
        write_linear_target_spec(
            &mut replacement_load,
            DM_IOCTL_HEADER_SIZE,
            0,
            4,
            0,
            temporary_params,
        );
        table_load_for_test(&mut replacement_load, &device).unwrap();
        device.resume().unwrap();

        unregister(active_id).unwrap();
        assert_eq!(
            unregister(temporary_id).unwrap_err(),
            aster_block::Error::Busy
        );
        drop(device);
        drop(manager);
        unregister(temporary_id).unwrap();
    }
}

#[ktest]
fn resume_replaces_active_linear_table_with_ioctl_loaded_mixed_table() {
    let linear = StatusBacking::new_with_major(510, 243);
    let striped_first = StatusBacking::new_with_major(510, 244);
    let striped_second = StatusBacking::new_with_major(510, 245);
    let linear_id = linear.id();
    let striped_first_id = striped_first.id();
    let striped_second_id = striped_second.id();
    register(linear as Arc<dyn BlockDevice>).unwrap();
    register(striped_first as Arc<dyn BlockDevice>).unwrap();
    register(striped_second as Arc<dyn BlockDevice>).unwrap();

    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-mixed-replace-active-test".to_string(), None, None)
        .unwrap();
    device.load_table(single_linear_table(4, 100, 1));
    device.resume().unwrap();

    let linear_params = "510:243 0";
    let striped_params = "2 4 510:244 0 510:245 0";
    let first_next = table_status_record_len(linear_params.len()).unwrap();
    let total_len = first_next + table_status_record_len(striped_params.len()).unwrap();
    let mut load = test_buffer(DM_IOCTL_HEADER_SIZE + total_len);
    write_u32(&mut load, OFF_TARGET_COUNT, 2).unwrap();
    write_linear_target_spec(
        &mut load,
        DM_IOCTL_HEADER_SIZE,
        0,
        4,
        first_next as u32,
        linear_params,
    );
    write_target_spec(
        &mut load,
        DM_IOCTL_HEADER_SIZE + first_next,
        4,
        16,
        0,
        "striped",
        striped_params,
    );
    table_load_for_test(&mut load, &device).unwrap();

    let mut active_before_resume = test_buffer(DM_IOCTL_HEADER_SIZE + 49);
    write_u32(&mut active_before_resume, OFF_FLAGS, DM_STATUS_TABLE_FLAG).unwrap();
    table_status_for_device(&mut active_before_resume, &device).unwrap();
    assert_eq!(
        read_u32(&active_before_resume, OFF_TARGET_COUNT).unwrap(),
        1
    );
    assert_eq!(
        c_string_until(
            &active_before_resume,
            DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE,
            DM_IOCTL_HEADER_SIZE + 48
        )
        .unwrap(),
        "1:1 100"
    );

    // Linux requires spare capacity after the last status string's NUL; the
    // returned data_size excludes that spare byte and final alignment padding.
    let status_data_size =
        DM_IOCTL_HEADER_SIZE + first_next + DM_TARGET_SPEC_SIZE + striped_params.len() + 1;
    let mut inactive_before_resume = test_buffer(DM_IOCTL_HEADER_SIZE + total_len + 1);
    write_u32(
        &mut inactive_before_resume,
        OFF_FLAGS,
        DM_QUERY_INACTIVE_TABLE_FLAG | DM_STATUS_TABLE_FLAG,
    )
    .unwrap();
    table_status_for_device(&mut inactive_before_resume, &device).unwrap();
    assert_eq!(
        read_u32(&inactive_before_resume, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG,
        0
    );
    assert_eq!(
        read_u32(&inactive_before_resume, OFF_DATA_SIZE).unwrap() as usize,
        status_data_size
    );
    assert_eq!(
        read_u32(&inactive_before_resume, OFF_TARGET_COUNT).unwrap(),
        2
    );
    assert_eq!(
        c_string_until(
            &inactive_before_resume,
            DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE,
            DM_IOCTL_HEADER_SIZE + first_next
        )
        .unwrap(),
        linear_params
    );
    assert_eq!(
        c_string_until(
            &inactive_before_resume,
            DM_IOCTL_HEADER_SIZE + first_next + DM_TARGET_SPEC_SIZE,
            status_data_size
        )
        .unwrap(),
        striped_params
    );

    device.resume().unwrap();
    assert!(device.active_table().is_some());
    assert!(device.inactive_table().is_none());

    let mut active_after_resume = test_buffer(DM_IOCTL_HEADER_SIZE + total_len + 1);
    write_u32(&mut active_after_resume, OFF_FLAGS, DM_STATUS_TABLE_FLAG).unwrap();
    table_status_for_device(&mut active_after_resume, &device).unwrap();
    assert_eq!(
        read_u32(&active_after_resume, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG,
        0
    );
    assert_eq!(
        read_u32(&active_after_resume, OFF_DATA_SIZE).unwrap() as usize,
        status_data_size
    );
    assert_eq!(read_u32(&active_after_resume, OFF_TARGET_COUNT).unwrap(), 2);
    assert_eq!(
        c_string_until(
            &active_after_resume,
            DM_IOCTL_HEADER_SIZE + DM_TARGET_SPEC_SIZE,
            DM_IOCTL_HEADER_SIZE + first_next
        )
        .unwrap(),
        linear_params
    );
    assert_eq!(
        c_string_until(
            &active_after_resume,
            DM_IOCTL_HEADER_SIZE + first_next + DM_TARGET_SPEC_SIZE,
            DM_IOCTL_HEADER_SIZE + total_len
        )
        .unwrap(),
        striped_params
    );

    let mut inactive_after_resume = test_buffer(DM_IOCTL_HEADER_SIZE + total_len);
    write_u32(
        &mut inactive_after_resume,
        OFF_FLAGS,
        DM_QUERY_INACTIVE_TABLE_FLAG,
    )
    .unwrap();
    table_status_for_device(&mut inactive_after_resume, &device).unwrap();
    assert_eq!(
        read_u32(&inactive_after_resume, OFF_TARGET_COUNT).unwrap(),
        0
    );

    let mut deps = test_buffer(DM_IOCTL_HEADER_SIZE + 32);
    table_deps_for_device(&mut deps, &device).unwrap();
    assert_eq!(read_u32(&deps, DM_IOCTL_HEADER_SIZE).unwrap(), 3);
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
    assert_eq!(unregister(linear_id).unwrap_err(), aster_block::Error::Busy);
    assert_eq!(
        unregister(striped_first_id).unwrap_err(),
        aster_block::Error::Busy
    );
    assert_eq!(
        unregister(striped_second_id).unwrap_err(),
        aster_block::Error::Busy
    );

    drop(device);
    drop(manager);
    unregister(linear_id).unwrap();
    unregister(striped_first_id).unwrap();
    unregister(striped_second_id).unwrap();
}

#[ktest]
fn rejects_missing_backing_without_setting_readonly_or_table() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-missing-backing-test".to_string(), None, None)
        .unwrap();
    let params = "510:230 0";
    let mut buffer =
        test_buffer(DM_IOCTL_HEADER_SIZE + table_status_record_len(params.len()).unwrap());
    write_u32(&mut buffer, OFF_FLAGS, DM_READONLY_FLAG).unwrap();
    write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
    write_linear_target_spec(&mut buffer, DM_IOCTL_HEADER_SIZE, 0, 4, 0, params);

    assert_failed_table_load_preserves_state(&mut buffer, &device, Errno::ENODEV);
    assert!(!device.is_readonly());
}

#[ktest]
fn rejects_missing_striped_backing_without_setting_readonly_or_table() {
    let first = StatusBacking::new_with_major(510, 225);
    let first_id = first.id();
    register(first as Arc<dyn BlockDevice>).unwrap();

    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-missing-striped-backing-test".to_string(), None, None)
        .unwrap();
    let params = "2 128 510:225 0 510:226 0";
    let mut buffer =
        test_buffer(DM_IOCTL_HEADER_SIZE + table_status_record_len(params.len()).unwrap());
    write_u32(&mut buffer, OFF_FLAGS, DM_READONLY_FLAG).unwrap();
    write_u32(&mut buffer, OFF_TARGET_COUNT, 1).unwrap();
    write_target_spec(
        &mut buffer,
        DM_IOCTL_HEADER_SIZE,
        0,
        256,
        0,
        "striped",
        params,
    );

    assert_failed_table_load_preserves_state(&mut buffer, &device, Errno::ENODEV);
    assert!(!device.is_readonly());

    unregister(first_id).unwrap();
}

#[ktest]
fn wait_returns_selected_table_records_and_preserves_query_flags() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("v5-wait-status".to_string(), None, None)
        .unwrap();
    device.load_table(zero_table(1, false));
    device.resume().unwrap();
    device.load_table(zero_table(2, true));
    device.notify_event();
    for flags in [
        0,
        DM_STATUS_TABLE_FLAG,
        DM_QUERY_INACTIVE_TABLE_FLAG,
        DM_QUERY_INACTIVE_TABLE_FLAG | DM_STATUS_TABLE_FLAG | DM_NOFLUSH_FLAG,
    ] {
        for capacity in [312, 408] {
            let mut output = request(capacity);
            write_u32(&mut output, OFF_FLAGS, flags).unwrap();
            write_u32(&mut output, OFF_DATA_SIZE, DM_IOCTL_FIXED_PREFIX_SIZE).unwrap();
            device_wait_for_device(&mut output, &device).unwrap();
            let count = if flags & DM_QUERY_INACTIVE_TABLE_FLAG != 0 {
                2
            } else {
                1
            };
            assert_eq!(read_u32(&output, OFF_TARGET_COUNT).unwrap(), count);
            assert_eq!(read_u32(&output, OFF_EVENT_NR).unwrap(), 1);
            assert_eq!(read_u32(&output, OFF_FLAGS).unwrap() & flags, flags);
            if capacity == 312 {
                assert_eq!(
                    read_u32(&output, OFF_DATA_SIZE).unwrap() as usize,
                    DM_IOCTL_FIXED_PREFIX_SIZE
                );
                assert_ne!(
                    read_u32(&output, OFF_FLAGS).unwrap() & DM_BUFFER_FULL_FLAG,
                    0
                );
            } else {
                assert_eq!(&output[336..341], b"zero\0");
                assert_eq!(
                    read_u32(&output, OFF_DATA_SIZE).unwrap(),
                    312 + (count - 1) * 48 + 41
                );
            }
        }
    }
}

#[ktest]
fn removal_keeps_live_table_for_existing_wait_reference_only() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("v5-wait-remove".to_string(), None, None)
        .unwrap();
    let active = zero_table(1, false);
    device.load_table(active.clone());
    device.resume().unwrap();
    device.load_table(zero_table(2, false));
    let registered = Arc::new(AtomicBool::new(false));
    let result = Arc::new(Mutex::new(None));
    {
        let waiter = manager.acquire_control(&device).unwrap();
        let registered = registered.clone();
        let result = result.clone();
        TaskOptions::new(move || {
            let mut output = request(360);
            let mut pending_checks = 0;
            device_wait_with_pending(&mut output, &waiter, || {
                pending_checks += 1;
                // Pause for WaitQueue checks once before allocating a waiter,
                // then enqueues its waker before each later condition check.
                if pending_checks >= 2 {
                    registered.store(true, Ordering::Release);
                }
            })
            .unwrap();
            *result.lock() = Some(output);
        })
        .spawn()
        .unwrap();
    }
    while !registered.load(Ordering::Acquire) {
        Task::yield_now();
    }
    assert!(result.lock().is_none());
    with_current_device_lifecycle_in(&manager, &device, |current| {
        ControlWorkflow::new(&manager).remove_with_unregistration(current, |_| Ok(()))
    })
    .unwrap();
    assert!(Arc::ptr_eq(&device.active_table().unwrap(), &active));
    assert!(device.inactive_table().is_none());
    assert!(manager.acquire_control(&device).is_none());
    device.drain_removed();
    assert!(device.active_table().is_none());
    while result.lock().is_none() {
        Task::yield_now();
    }
    let output = result.lock().take().unwrap();
    assert_eq!(read_u32(&output, OFF_EVENT_NR).unwrap(), 1);
    assert_eq!(read_u32(&output, OFF_TARGET_COUNT).unwrap(), 1);
    assert_eq!(&output[336..341], b"zero\0");
    let mut new_request = request(305);
    write_u64(&mut new_request, OFF_DEV, device.id().as_encoded_u64()).unwrap();
    assert_eq!(
        lookup_device_in_manager(&new_request, &manager)
            .unwrap_err()
            .error(),
        Errno::ENXIO
    );
}

#[ktest]
fn rename_and_remove_emit_events_only_for_live_tables() {
    for table_state in 0..3 {
        let manager = DmManager::new().unwrap();
        let device = manager
            .create("v5-event-old".to_string(), None, None)
            .unwrap();
        if table_state != 0 {
            device.load_table(zero_table(1, false));
        }
        if table_state == 2 {
            device.resume().unwrap();
        }
        with_current_device_lifecycle_in(&manager, &device, |current| {
            let workflow = ControlWorkflow::new(&manager);
            workflow.rename(current, &RenameRequest::Name("v5-event-new"))?;
            workflow.rename(current, &RenameRequest::Uuid(b"v5-event-uuid"))?;
            assert_eq!(
                device.status().event_nr,
                if table_state == 2 { 2 } else { 0 }
            );
            workflow.remove_with_unregistration(current, |_| Ok(()))
        })
        .unwrap();
        assert_eq!(
            device.status().event_nr,
            if table_state == 2 { 3 } else { 0 }
        );
    }
}

#[ktest]
fn remove_all_retries_newly_unbusy_and_newly_created_devices() {
    let manager = DmManager::new().unwrap();
    let first = manager
        .create("v5-remove-a".to_string(), None, None)
        .unwrap();
    let second = manager
        .create("v5-remove-b".to_string(), None, None)
        .unwrap();
    let busy = Cell::new(true);
    let outcome = ControlWorkflow::new(&manager).remove_all(
        RemoveAllRequest::new(vec![first.clone(), second.clone()]),
        |device| {
            if Arc::ptr_eq(device, &first) && busy.get() {
                return Err(Error::new(Errno::EBUSY));
            }
            if Arc::ptr_eq(device, &second) {
                busy.set(false);
                manager
                    .create("v5-remove-c".to_string(), None, None)
                    .unwrap();
            }
            with_current_device_lifecycle_in(&manager, device, |current| {
                ControlWorkflow::new(&manager).remove_with_unregistration(current, |_| Ok(()))
            })
        },
    );
    assert_eq!(outcome.removed, 3);
    assert_eq!(outcome.attempted, 4);
    assert!(manager.devices().is_empty());
}
