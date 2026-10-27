// SPDX-License-Identifier: MPL-2.0

//! Ioctl envelope, identity selectors, flags, and user-copy failure contracts.

use super::*;
use crate::{device::misc::dm_control::DmControlFile, events::IoEvents, process::signal::Pollable};

#[ktest]
fn control_without_events_is_not_ready() {
    let file = DmControlFile::new();
    for mask in [
        IoEvents::empty(),
        IoEvents::IN,
        IoEvents::OUT,
        IoEvents::IN | IoEvents::OUT,
    ] {
        assert_eq!(file.poll(mask, None), IoEvents::empty());
    }
}

#[ktest]
fn control_events_are_per_open_and_wrap_like_linux() {
    use core::sync::atomic::Ordering;

    use crate::device::misc::dm_control::ControlEvents;

    let events = ControlEvents::new();
    events.generation.store(u32::MAX, Ordering::Release);
    let first = DmControlFile::new_with_events(&events);
    let second = DmControlFile::new_with_events(&events);
    events.notify();
    assert_eq!(first.poll_events(&events, IoEvents::IN, None), IoEvents::IN);
    assert_eq!(
        first.poll_events(&events, IoEvents::OUT, None),
        IoEvents::empty()
    );
    first.arm(&events);
    assert_eq!(
        first.poll_events(&events, IoEvents::IN, None),
        IoEvents::empty()
    );
    assert_eq!(
        second.poll_events(&events, IoEvents::IN, None),
        IoEvents::IN
    );
    let fresh = DmControlFile::new_with_events(&events);
    assert_eq!(
        fresh.poll_events(&events, IoEvents::IN, None),
        IoEvents::empty()
    );
    events.notify();
    assert_eq!(first.poll_events(&events, IoEvents::IN, None), IoEvents::IN);
}

#[ktest]
fn opaque_uuid_roundtrips_through_identity_and_list_records() {
    let manager = DmManager::new().unwrap();
    let uuid = b"opaque-\xff\x80";
    let device = manager
        .create("dm-opaque-uuid".to_string(), Some(uuid.to_vec()), None)
        .unwrap();
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
    write_c_string_fixed(&mut buffer, OFF_UUID, DM_UUID_LEN, uuid).unwrap();
    assert!(Arc::ptr_eq(
        &lookup_device_in_manager(&buffer, &manager).unwrap(),
        &device
    ));
    fill_device_header(&mut buffer, &device).unwrap();
    assert_eq!(
        optional_uuid(&buffer, OFF_UUID, DM_UUID_LEN)
            .unwrap()
            .unwrap(),
        uuid
    );

    let mut output = test_buffer(DM_IOCTL_HEADER_SIZE + 128);
    write_c_string_fixed(&mut output, OFF_UUID, DM_UUID_LEN, b"opaque-\xff").unwrap();
    list_devices_for_devices(&mut output, vec![device.clone()]).unwrap();
    let (extension, _) =
        name_list_record_layout(device.mapper_name().len() + 1, uuid.len() + 1).unwrap();
    assert_eq!(
        c_bytes_until(&output, DM_IOCTL_HEADER_SIZE + extension + 8, output.len()).unwrap(),
        uuid
    );
    manager
        .rename("dm-opaque-uuid", "dm-opaque-renamed")
        .unwrap();
    assert!(Arc::ptr_eq(&manager.lookup_uuid(uuid).unwrap(), &device));
    manager.remove("dm-opaque-renamed").unwrap();
    assert!(manager.lookup_uuid(uuid).is_none());

    buffer[OFF_UUID..OFF_UUID + DM_UUID_LEN].fill(0xff);
    assert_eq!(
        optional_uuid(&buffer, OFF_UUID, DM_UUID_LEN)
            .unwrap_err()
            .error(),
        Errno::EINVAL
    );
}

#[ktest]
fn arm_acknowledges_events_even_when_result_copyout_faults() {
    use crate::device::misc::dm_control::ControlEvents;

    let events = ControlEvents::new();
    let file = DmControlFile::new_with_events(&events);
    events.notify();
    let input = test_buffer(DM_IOCTL_HEADER_SIZE);
    let mut version = [0; 12];
    write_version(&mut version).unwrap();
    let result = process_ioctl_parameters(
        DM_DEV_ARM_POLL_CMD,
        &version,
        |offset, bytes| {
            bytes.copy_from_slice(&input[offset..offset + bytes.len()]);
            Ok(())
        },
        |_| panic!("ordinary request must not clear input"),
        |_| Err(Error::new(Errno::EFAULT)),
        |_| {
            file.arm(&events);
            Ok(())
        },
    );
    assert_eq!(result.unwrap_err().error(), Errno::EFAULT);
    assert_eq!(
        file.poll_events(&events, IoEvents::IN, None),
        IoEvents::empty()
    );
}

#[ktest]
fn rejects_invalid_names_before_identity_changes() {
    for name in ["", ".", "..", "control", "a/b"] {
        assert_eq!(
            validate_mapper_name(name).unwrap_err().error(),
            Errno::EINVAL
        );
    }
    validate_mapper_name(&"a".repeat(DM_NAME_LEN - 1)).unwrap();
    assert_eq!(
        validate_mapper_name(&"a".repeat(DM_NAME_LEN))
            .unwrap_err()
            .error(),
        Errno::EINVAL
    );
    assert_eq!(map_rename_error(DmError::NameExists).error(), Errno::EBUSY);
    assert_eq!(map_rename_error(DmError::UuidExists).error(), Errno::EBUSY);
    assert_eq!(map_dm_error(DmError::NameExists).error(), Errno::EEXIST);
}

#[ktest]
fn preserves_ioctl_header_when_userspace_changes_before_payload_copy() {
    let mut version = [0u8; 12];
    write_version(&mut version).unwrap();
    let mut user = vec![0u8; DM_IOCTL_FIXED_PREFIX_SIZE + 8];
    let size = user.len();
    write_u32(&mut user, OFF_DATA_SIZE, size as u32).unwrap();
    write_u32(&mut user, OFF_DATA_START, DM_IOCTL_FIXED_PREFIX_SIZE as u32).unwrap();
    write_u32(&mut user, OFF_FLAGS, DM_SECURE_DATA_FLAG).unwrap();
    user[OFF_NAME..OFF_NAME + 6].copy_from_slice(b"alpha\0");
    let header = read_ioctl_header(&version, |offset, bytes| {
        assert_eq!(offset, 12);
        assert_eq!(bytes.len(), DM_IOCTL_FIXED_PREFIX_SIZE - 12);
        bytes.copy_from_slice(&user[offset..offset + bytes.len()]);
        Ok(())
    })
    .unwrap();
    validate_ioctl_size(size).unwrap();

    // Mutate every header field after capture, not just the device name.
    user[..DM_IOCTL_FIXED_PREFIX_SIZE].fill(0xff);
    user[DM_IOCTL_FIXED_PREFIX_SIZE..].fill(0x5a);
    let buffer = copy_ioctl_payload(&header.bytes, size, |offset, bytes| {
        assert_eq!(offset, DM_IOCTL_FIXED_PREFIX_SIZE);
        assert_eq!(bytes.len(), 8);
        bytes.copy_from_slice(&user[offset..offset + bytes.len()]);
        Ok(())
    })
    .unwrap();
    assert_eq!(&buffer[..DM_IOCTL_FIXED_PREFIX_SIZE], &header.bytes);
    assert_eq!(&buffer[..12], &version);
    assert_eq!(read_u32(&buffer, OFF_FLAGS).unwrap(), DM_SECURE_DATA_FLAG);
    assert_eq!(&buffer[OFF_NAME..OFF_NAME + 6], b"alpha\0");
    assert_eq!(&buffer[DM_IOCTL_FIXED_PREFIX_SIZE..], &[0x5a; 8]);
}

#[ktest]
fn ioctl_snapshot_propagates_copy_faults() {
    let version = [0u8; 12];
    let header_error = read_ioctl_header(&version, |_, _| {
        Err(Error::with_message(Errno::EFAULT, "injected header fault"))
    })
    .unwrap_err();
    assert_eq!(header_error.error(), Errno::EFAULT);

    let header = [0u8; DM_IOCTL_FIXED_PREFIX_SIZE];
    let mut dispatched = false;
    let result = copy_ioctl_payload(&header, header.len() + 8, |_, _| {
        Err(Error::with_message(Errno::EFAULT, "injected payload fault"))
    })
    .map(|_| dispatched = true);
    assert_eq!(result.unwrap_err().error(), Errno::EFAULT);
    assert!(!dispatched);
}

#[ktest]
fn header_only_ioctl_does_not_read_payload() {
    let header = [0u8; DM_IOCTL_FIXED_PREFIX_SIZE];
    let buffer = copy_ioctl_payload(&header, header.len(), |_, _| {
        panic!("header-only request must not trigger another user read")
    })
    .unwrap();
    assert_eq!(&buffer[..], &header);
}

#[ktest]
fn rejects_invalid_ioctl_buffer_layouts() {
    for (data_size, data_start) in [
        (DM_IOCTL_HEADER_SIZE - 1, DM_IOCTL_HEADER_SIZE),
        (DM_IOCTL_MAX_SIZE + 1, DM_IOCTL_HEADER_SIZE),
        (DM_IOCTL_HEADER_SIZE, DM_IOCTL_HEADER_SIZE - 8),
        (DM_IOCTL_HEADER_SIZE, DM_IOCTL_HEADER_SIZE + 8),
        (DM_IOCTL_HEADER_SIZE + 8, DM_IOCTL_HEADER_SIZE + 1),
    ] {
        assert_eq!(
            validate_ioctl_buffer_layout(data_size, data_start)
                .unwrap_err()
                .error(),
            Errno::EINVAL
        );
    }
    validate_ioctl_buffer_layout(DM_IOCTL_HEADER_SIZE, DM_IOCTL_HEADER_SIZE).unwrap();
    validate_ioctl_buffer_layout(DM_IOCTL_MAX_SIZE, DM_IOCTL_HEADER_SIZE).unwrap();
}

#[ktest]
fn rejects_invalid_utf8_strings() {
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
    buffer[OFF_NAME] = 0xff;
    buffer[OFF_NAME + 1] = 0;
    assert_eq!(
        required_c_string(&buffer, OFF_NAME, DM_NAME_LEN)
            .unwrap_err()
            .error(),
        Errno::EINVAL
    );

    let params_start = DM_IOCTL_HEADER_SIZE - 8;
    buffer[params_start] = 0xff;
    buffer[params_start + 1] = 0;
    assert_eq!(
        c_string_until(&buffer, params_start, buffer.len())
            .unwrap_err()
            .error(),
        Errno::EINVAL
    );
}

#[ktest]
fn rejects_unterminated_and_out_of_bounds_strings() {
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
    buffer[OFF_NAME..OFF_NAME + DM_NAME_LEN].fill(b'x');
    assert_eq!(
        required_c_string(&buffer, OFF_NAME, DM_NAME_LEN)
            .unwrap_err()
            .error(),
        Errno::EINVAL
    );
    assert_eq!(
        c_string_until(&buffer, buffer.len(), buffer.len())
            .unwrap_err()
            .error(),
        Errno::EINVAL
    );
}

#[ktest]
fn fills_device_header_from_runtime_state() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create(
            "dm-control-header-test".to_string(),
            Some(b"dm-control-header-uuid".to_vec()),
            None,
        )
        .unwrap();
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
    write_u32(
        &mut buffer,
        OFF_FLAGS,
        DM_READONLY_FLAG | DM_SUSPEND_FLAG | DM_ACTIVE_PRESENT_FLAG | DM_INACTIVE_PRESENT_FLAG,
    )
    .unwrap();
    write_u32(&mut buffer, OFF_TARGET_COUNT, u32::MAX).unwrap();
    buffer[DM_IOCTL_FIXED_PREFIX_SIZE..DM_IOCTL_HEADER_SIZE].fill(0xa5);

    fill_device_header(&mut buffer, &device).unwrap();

    assert_eq!(read_u32(&buffer, OFF_FLAGS).unwrap(), 0);
    assert_eq!(read_u32(&buffer, OFF_TARGET_COUNT).unwrap(), 0);
    assert_eq!(read_u32(&buffer, OFF_EVENT_NR).unwrap(), 0);
    assert_eq!(read_u32(&buffer, OFF_OPEN_COUNT).unwrap(), 0);
    assert_eq!(
        read_u64(&buffer, OFF_DEV).unwrap(),
        device.id().as_encoded_u64()
    );
    assert_eq!(
        required_c_string(&buffer, OFF_NAME, DM_NAME_LEN).unwrap(),
        device.mapper_name()
    );
    assert_eq!(
        optional_uuid(&buffer, OFF_UUID, DM_UUID_LEN)
            .unwrap()
            .unwrap(),
        device.uuid().unwrap()
    );
    assert!(
        buffer[DM_IOCTL_FIXED_PREFIX_SIZE..DM_IOCTL_HEADER_SIZE]
            .iter()
            .all(|byte| *byte == 0xa5)
    );
}

#[ktest]
fn accepts_only_complete_aligned_ioctl_envelopes() {
    for data_size in [DM_IOCTL_FIXED_PREFIX_SIZE, DM_IOCTL_HEADER_SIZE - 1] {
        assert_eq!(
            validate_ioctl_buffer_layout(data_size, DM_IOCTL_HEADER_SIZE)
                .unwrap_err()
                .error(),
            Errno::EINVAL
        );
    }
    validate_ioctl_buffer_layout(DM_IOCTL_HEADER_SIZE, DM_IOCTL_HEADER_SIZE).unwrap();
}

#[ktest]
fn device_selectors_ignore_high_encoding_bits() {
    let manager = DmManager::new().unwrap();
    let device = manager
        .create("dm-selector-high-bits".to_string(), None, Some(0xfffff))
        .unwrap();
    for high in [0, 1u64 << 32, u64::MAX << 32] {
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
        write_u64(&mut buffer, OFF_DEV, device.id().as_encoded_u64() | high).unwrap();
        assert!(Arc::ptr_eq(
            &lookup_device_in_manager(&buffer, &manager).unwrap(),
            &device
        ));
    }
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
    write_u64(&mut buffer, OFF_DEV, 1u64 << 32).unwrap();
    assert_eq!(
        lookup_device_in_manager(&buffer, &manager)
            .unwrap_err()
            .error(),
        Errno::ENXIO
    );
}

#[ktest]
fn rejects_ambiguous_selectors_without_selecting_either_device() {
    let manager = DmManager::new().unwrap();
    let first = manager
        .create(
            "dm-selector-first".to_string(),
            Some(b"dm-selector-uuid".to_vec()),
            None,
        )
        .unwrap();
    let second = manager
        .create("dm-selector-second".to_string(), None, None)
        .unwrap();
    for mask in 0..8 {
        let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
        if mask & 1 != 0 {
            write_c_string_fixed(&mut buffer, OFF_NAME, DM_NAME_LEN, second.mapper_name()).unwrap();
        }
        if mask & 2 != 0 {
            write_c_string_fixed(&mut buffer, OFF_UUID, DM_UUID_LEN, "dm-selector-uuid").unwrap();
        }
        if mask & 4 != 0 {
            write_u64(&mut buffer, OFF_DEV, second.id().as_encoded_u64()).unwrap();
        }
        let result = lookup_device_in_manager(&buffer, &manager);
        match mask {
            1 | 4 => assert!(Arc::ptr_eq(&result.unwrap(), &second)),
            2 => assert!(Arc::ptr_eq(&result.unwrap(), &first)),
            3 | 7 => assert_eq!(result.unwrap_err().error(), Errno::EINVAL),
            _ => assert_eq!(result.unwrap_err().error(), Errno::ENXIO),
        }
        assert!(manager.is_current(&first));
        assert!(manager.is_current(&second));
    }
}

#[ktest]
fn normalizes_output_flags_and_rejects_only_active_unsupported_features() {
    let mut buffer = test_buffer(DM_IOCTL_HEADER_SIZE);
    write_u32(&mut buffer, OFF_FLAGS, u32::MAX).unwrap();
    validate_ioctl_params(DM_DEV_STATUS_CMD, &mut buffer).unwrap();
    assert_eq!(
        read_u32(&buffer, OFF_FLAGS).unwrap(),
        !(DM_BUFFER_FULL_FLAG | DM_UEVENT_GENERATED_FLAG | DM_SECURE_DATA_FLAG | DM_DATA_OUT_FLAG)
    );
    for command in [DM_DEV_REMOVE_CMD, DM_REMOVE_ALL_CMD] {
        assert_eq!(
            validate_input_flags(command, &buffer).unwrap_err().error(),
            Errno::EOPNOTSUPP
        );
    }
    for command in [DM_TABLE_STATUS_CMD, DM_DEV_WAIT_CMD] {
        assert_eq!(
            validate_input_flags(command, &buffer).unwrap_err().error(),
            Errno::EOPNOTSUPP
        );
    }
    write_u32(
        &mut buffer,
        OFF_FLAGS,
        DM_STATUS_TABLE_FLAG | DM_QUERY_INACTIVE_TABLE_FLAG | DM_NOFLUSH_FLAG,
    )
    .unwrap();
    validate_input_flags(DM_DEV_WAIT_CMD, &buffer).unwrap();
    validate_input_flags(DM_TABLE_LOAD_CMD, &buffer).unwrap();
}

#[ktest]
fn accepts_backward_compatible_interface_versions() {
    for version in [[4, 0, 0], [4, 48, 0], [4, 48, u32::MAX]] {
        let mut buffer = [0u8; 12];
        for (index, value) in version.into_iter().enumerate() {
            write_u32(&mut buffer, index * 4, value).unwrap();
        }
        validate_client_version(&buffer).unwrap();
    }
}

#[ktest]
fn rejects_incompatible_interface_versions() {
    for version in [
        [3, 0, 0],
        [5, 0, 0],
        [4, 49, 0],
        [4, 50, 0],
        [4, u32::MAX, 0],
    ] {
        let mut buffer = [0u8; 12];
        for (index, value) in version.into_iter().enumerate() {
            write_u32(&mut buffer, index * 4, value).unwrap();
        }
        assert_eq!(
            validate_client_version(&buffer).unwrap_err().error(),
            Errno::EINVAL
        );
    }
}

#[ktest]
fn validates_ioctl_encoding_and_alignment() {
    assert_eq!(DM_IOCTL_HEADER_SIZE, 312);
    assert_eq!(decode_command(0xc138_fd00).unwrap(), DM_VERSION_CMD);
    assert_eq!(decode_command(0xc138_fd05).unwrap(), DM_DEV_RENAME_CMD);
    assert_eq!(decode_command(0xc138_fd08).unwrap(), DM_DEV_WAIT_CMD);
    assert_eq!(decode_command(0xc138_fd09).unwrap(), DM_TABLE_LOAD_CMD);
    assert_eq!(
        decode_command(0xc138_fd11).unwrap(),
        DM_GET_TARGET_VERSION_CMD
    );
    assert_eq!(decode_command(0xc138_fd10).unwrap(), DM_DEV_ARM_POLL_CMD);
    for unsupported_command in [DM_IOCTL_COMMAND_PREFIX + 14, DM_IOCTL_COMMAND_PREFIX + 15] {
        assert_eq!(
            decode_command(unsupported_command).unwrap_err().error(),
            Errno::ENOTTY
        );
    }
    assert_eq!(decode_command(0x0000_fd00).unwrap(), DM_VERSION_CMD);
    assert_eq!(decode_command(0x8004_fd05).unwrap(), DM_DEV_RENAME_CMD);
    assert_eq!(decode_command(0).unwrap_err().error(), Errno::ENOTTY);
    assert_eq!(command_name(DM_VERSION_CMD), "DM_VERSION");
    assert_eq!(command_name(DM_TABLE_LOAD_CMD), "DM_TABLE_LOAD");
    assert_eq!(
        command_name(DM_GET_TARGET_VERSION_CMD),
        "DM_GET_TARGET_VERSION"
    );
    assert_eq!(command_name(0xff), "DM_UNKNOWN");
    assert_eq!(align_up(313, 8).unwrap(), 320);
}

#[ktest]
fn no_params_reads_and_returns_only_the_fixed_prefix() {
    for command in [
        DM_DEV_CREATE_CMD,
        DM_DEV_REMOVE_CMD,
        DM_DEV_SUSPEND_CMD,
        DM_DEV_STATUS_CMD,
        DM_TABLE_CLEAR_CMD,
        DM_DEV_ARM_POLL_CMD,
        DM_REMOVE_ALL_CMD,
    ] {
        let mut input = request(DM_IOCTL_FIXED_PREFIX_SIZE);
        write_c_string_fixed(&mut input, OFF_NAME, DM_NAME_LEN, "v5-prefix").unwrap();
        write_u32(&mut input, OFF_DATA_SIZE, DM_IOCTL_MAX_SIZE).unwrap();
        write_u32(&mut input, OFF_DATA_START, u32::MAX).unwrap();
        let reads = Cell::new(0);
        let writes = Cell::new(0);
        process_ioctl_parameters(
            command,
            &version(),
            |offset, bytes| {
                reads.set(reads.get() + 1);
                assert_eq!(offset, 12);
                assert_eq!(bytes.len(), 293);
                bytes.copy_from_slice(&input[offset..]);
                Ok(())
            },
            |_| panic!("nonsecure input must not be cleared"),
            |bytes| {
                writes.set(writes.get() + 1);
                assert_eq!(bytes.len(), 305);
                assert_eq!(read_u32(bytes, OFF_DATA_SIZE).unwrap(), 305);
                Ok(())
            },
            |bytes| {
                assert_eq!(bytes.len(), 305);
                if command == DM_REMOVE_ALL_CMD {
                    write_u32(bytes, OFF_DATA_SIZE, 0)?;
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(reads.get(), 1);
        assert_eq!(writes.get(), usize::from(command != DM_REMOVE_ALL_CMD));
    }
}

#[ktest]
fn secure_copy_faults_and_dispatch_exits_wipe_owned_parameters() {
    // 0: success, 1: partial copyin, 2: clear fault, 3: validation failure,
    // 4: handler error, 5: copyout fault. Observe bytes before deallocation.
    for fault in 0..6 {
        let mut input = request(384);
        write_u32(&mut input, OFF_FLAGS, DM_SECURE_DATA_FLAG).unwrap();
        input[305..].fill(0xa5);
        if fault == 3 {
            input[OFF_NAME] = b'n';
            input[OFF_UUID] = b'u';
        }
        let input = RefCell::new(input);
        let cleared = Cell::new(false);
        let dispatched = Cell::new(false);
        let written = Cell::new(false);
        let observed = Arc::new(Mutex::new(None));
        let result = process_ioctl_parameters_with_allocator(
            DM_TABLE_LOAD_CMD,
            &version(),
            |offset, bytes| {
                if offset == 305 && fault == 1 {
                    bytes[..3].fill(0xa5);
                    return Err(Error::new(Errno::EFAULT));
                }
                bytes.copy_from_slice(&input.borrow()[offset..offset + bytes.len()]);
                Ok(())
            },
            |len| {
                cleared.set(true);
                assert_eq!(len, 384);
                if fault == 2 {
                    return Err(Error::new(Errno::EFAULT));
                }
                input.borrow_mut()[..len].fill(0);
                Ok(())
            },
            |bytes| {
                written.set(true);
                assert!(cleared.get());
                assert!(dispatched.get());
                assert_eq!(bytes.len(), 305);
                if fault == 5 {
                    return Err(Error::new(Errno::EFAULT));
                }
                input.borrow_mut()[..bytes.len()].copy_from_slice(bytes);
                Ok(())
            },
            |bytes| {
                dispatched.set(true);
                assert!(cleared.get());
                assert_eq!(read_u32(bytes, OFF_FLAGS)? & DM_SECURE_DATA_FLAG, 0);
                if fault == 4 {
                    return Err(Error::new(Errno::EINVAL));
                }
                Ok(())
            },
            |size, secure| {
                let mut buffer = IoctlBuffer::new(size, secure)?;
                buffer.wipe_observer = Some(observed.clone());
                Ok(buffer)
            },
        );
        if fault == 0 {
            result.unwrap();
        } else {
            let expected = if fault == 3 || fault == 4 {
                Errno::EINVAL
            } else {
                Errno::EFAULT
            };
            assert_eq!(result.unwrap_err().error(), expected);
        }
        assert_eq!(cleared.get(), fault != 1);
        assert_eq!(dispatched.get(), matches!(fault, 0 | 4 | 5));
        assert_eq!(written.get(), matches!(fault, 0 | 5));
        let observed = observed.lock();
        let bytes = observed.as_ref().unwrap();
        assert_eq!(bytes.len(), 384);
        assert!(bytes.iter().all(|byte| *byte == 0));
        if !matches!(fault, 1 | 2) {
            assert!(input.borrow()[305..].iter().all(|byte| *byte == 0));
        }
    }
}

#[ktest]
fn allocation_failure_obeys_secure_clear_and_errno_precedence() {
    for secure in [false, true] {
        for clear_fails in [false, true] {
            let mut input = request(384);
            write_u32(
                &mut input,
                OFF_FLAGS,
                if secure { DM_SECURE_DATA_FLAG } else { 0 },
            )
            .unwrap();
            let cleared = Cell::new(false);
            let result = process_ioctl_parameters_with_allocator(
                DM_TABLE_LOAD_CMD,
                &version(),
                |offset, bytes| {
                    assert_eq!(offset, 12);
                    bytes.copy_from_slice(&input[offset..offset + bytes.len()]);
                    Ok(())
                },
                |len| {
                    assert_eq!(len, input.len());
                    cleared.set(true);
                    if clear_fails {
                        Err(Error::new(Errno::EFAULT))
                    } else {
                        Ok(())
                    }
                },
                |_| panic!("allocation failed"),
                |_| panic!("allocation failed"),
                |_, _| Err(Error::new(Errno::ENOMEM)),
            );
            assert_eq!(cleared.get(), secure);
            assert_eq!(
                result.unwrap_err().error(),
                if secure && clear_fails {
                    Errno::EFAULT
                } else {
                    Errno::ENOMEM
                }
            );
        }
    }
}

#[ktest]
fn secure_no_params_failure_clears_only_the_fixed_prefix() {
    for allocation_fails in [false, true] {
        let mut input = request(DM_IOCTL_HEADER_SIZE);
        write_u32(&mut input, OFF_DATA_SIZE, DM_IOCTL_MAX_SIZE).unwrap();
        write_u32(&mut input, OFF_FLAGS, DM_SECURE_DATA_FLAG).unwrap();
        input[DM_IOCTL_FIXED_PREFIX_SIZE..].fill(0xa5);
        let input = RefCell::new(input);
        let cleared = Cell::new(false);
        let observed = Arc::new(Mutex::new(None));
        let result = process_ioctl_parameters_with_allocator(
            DM_DEV_CREATE_CMD,
            &version(),
            |offset, bytes| {
                assert_eq!(offset, 12);
                assert_eq!(offset + bytes.len(), DM_IOCTL_FIXED_PREFIX_SIZE);
                bytes.copy_from_slice(&input.borrow()[offset..offset + bytes.len()]);
                Ok(())
            },
            |len| {
                assert_eq!(len, DM_IOCTL_FIXED_PREFIX_SIZE);
                cleared.set(true);
                input.borrow_mut()[..len].fill(0);
                Ok(())
            },
            |_| panic!("failed secure create must not copyout"),
            |_| panic!("empty create name must fail before dispatch"),
            |size, secure| {
                assert_eq!(size, DM_IOCTL_FIXED_PREFIX_SIZE);
                assert!(secure);
                if allocation_fails {
                    return Err(Error::new(Errno::ENOMEM));
                }
                let mut buffer = IoctlBuffer::new(size, secure)?;
                buffer.wipe_observer = Some(observed.clone());
                Ok(buffer)
            },
        );
        assert_eq!(
            result.unwrap_err().error(),
            if allocation_fails {
                Errno::ENOMEM
            } else {
                Errno::EINVAL
            }
        );
        assert!(cleared.get());
        let input = input.borrow();
        assert!(
            input[..DM_IOCTL_FIXED_PREFIX_SIZE]
                .iter()
                .all(|byte| *byte == 0)
        );
        assert!(
            input[DM_IOCTL_FIXED_PREFIX_SIZE..]
                .iter()
                .all(|byte| *byte == 0xa5)
        );
        let observed = observed.lock();
        if allocation_fails {
            assert!(observed.is_none());
        } else {
            let bytes = observed.as_ref().unwrap();
            assert_eq!(bytes.len(), DM_IOCTL_FIXED_PREFIX_SIZE);
            assert!(bytes.iter().all(|byte| *byte == 0));
        }
    }
}

#[ktest]
fn ordinary_failure_does_not_copyout_or_clear_input() {
    let input = request(384);
    let result = process_ioctl_parameters(
        DM_TABLE_LOAD_CMD,
        &version(),
        |offset, bytes| {
            bytes.copy_from_slice(&input[offset..offset + bytes.len()]);
            Ok(())
        },
        |_| panic!("ordinary input must remain intact"),
        |_| panic!("failed handler must not copyout"),
        |_| Err(Error::new(Errno::EINVAL)),
    );
    assert_eq!(result.unwrap_err().error(), Errno::EINVAL);
}

#[ktest]
fn fixed_fields_are_normalized_only_for_commands_that_use_them() {
    let mut input = request(305);
    input[OFF_NAME..OFF_NAME + DM_NAME_LEN].fill(b'n');
    input[OFF_UUID..OFF_UUID + DM_UUID_LEN].fill(b'u');
    validate_ioctl_params(DM_DEV_CREATE_CMD, &mut input).unwrap();
    assert_eq!(
        required_c_string(&input, OFF_NAME, DM_NAME_LEN)
            .unwrap()
            .len(),
        127
    );
    assert_eq!(
        required_c_string(&input, OFF_UUID, DM_UUID_LEN)
            .unwrap()
            .len(),
        128
    );
    for command in [DM_REMOVE_ALL_CMD, DM_LIST_DEVICES_CMD, DM_LIST_VERSIONS_CMD] {
        input[OFF_NAME..OFF_NAME + DM_NAME_LEN].fill(0xff);
        input[OFF_UUID..OFF_UUID + DM_UUID_LEN].fill(0xff);
        validate_ioctl_params(command, &mut input).unwrap();
        assert_eq!(input[OFF_NAME + DM_NAME_LEN - 1], 0xff);
        assert_eq!(input[OFF_UUID + DM_UUID_LEN - 1], 0xff);
    }
    for size in [305, 311, 312, DM_IOCTL_MAX_SIZE] {
        validate_ioctl_size(size).unwrap();
    }
    for size in [0, 304, DM_IOCTL_MAX_SIZE + 1, u32::MAX as usize] {
        assert_eq!(
            validate_ioctl_size(size).unwrap_err().error(),
            Errno::EINVAL
        );
    }
}

#[ktest]
fn ignored_flags_survive_while_output_only_flags_are_normalized() {
    let manager = DmManager::new().unwrap();
    let device = manager.create("v5-flags".to_string(), None, None).unwrap();
    let kept = DM_SKIP_BDGET_FLAG
        | DM_SKIP_LOCKFS_FLAG
        | DM_NOFLUSH_FLAG
        | DM_STATUS_TABLE_FLAG
        | (1 << 31);
    let cleared =
        DM_BUFFER_FULL_FLAG | DM_UEVENT_GENERATED_FLAG | DM_SECURE_DATA_FLAG | DM_DATA_OUT_FLAG;
    let mut buffer = request(305);
    write_u32(&mut buffer, OFF_FLAGS, kept | cleared).unwrap();
    validate_ioctl_params(DM_DEV_STATUS_CMD, &mut buffer).unwrap();
    device_status_for_device(&mut buffer, &device).unwrap();
    assert_eq!(read_u32(&buffer, OFF_FLAGS).unwrap(), kept);
    for (command, flag) in [
        (DM_DEV_REMOVE_CMD, DM_DEFERRED_REMOVE),
        (DM_REMOVE_ALL_CMD, DM_DEFERRED_REMOVE),
        (DM_TABLE_STATUS_CMD, DM_IMA_MEASUREMENT_FLAG),
        (DM_DEV_WAIT_CMD, DM_IMA_MEASUREMENT_FLAG),
    ] {
        let mut buffer = request(305);
        write_u32(&mut buffer, OFF_FLAGS, flag).unwrap();
        assert_eq!(
            validate_ioctl_params(command, &mut buffer)
                .unwrap_err()
                .error(),
            Errno::EOPNOTSUPP
        );
    }
}

#[ktest]
fn fixed_header_is_wiped_after_success_and_partial_read_faults() {
    for copied in [0, 8, 32, DM_IOCTL_FIXED_PREFIX_SIZE - 12] {
        let observed = Arc::new(Mutex::new(None));
        let header = IoctlHeader {
            bytes: [0; DM_IOCTL_FIXED_PREFIX_SIZE],
            wipe_observer: Some(observed.clone()),
        };
        let result = capture_ioctl_header(header, &version(), |offset, bytes| {
            assert_eq!(offset, 12);
            bytes[..copied].fill(0xa5);
            if copied == bytes.len() {
                Ok(())
            } else {
                Err(Error::new(Errno::EFAULT))
            }
        });
        if copied == DM_IOCTL_FIXED_PREFIX_SIZE - 12 {
            let header = result.unwrap();
            assert!(observed.lock().is_none());
            assert_eq!(&header.bytes[..12], &version());
            assert!(header.bytes[12..].iter().all(|byte| *byte == 0xa5));
            drop(header);
        } else {
            assert_eq!(result.unwrap_err().error(), Errno::EFAULT);
        }
        let observed = observed.lock();
        let bytes = observed.as_ref().unwrap();
        assert_eq!(bytes.len(), DM_IOCTL_FIXED_PREFIX_SIZE);
        assert!(bytes.iter().all(|byte| *byte == 0));
    }
}

#[ktest]
fn partial_fixed_header_fault_never_allocates_or_dispatches() {
    let result = process_ioctl_parameters_with_allocator(
        DM_TABLE_LOAD_CMD,
        &version(),
        |offset, bytes| {
            assert_eq!(offset, 12);
            bytes[..32].fill(0xa5);
            Err(Error::new(Errno::EFAULT))
        },
        |_| panic!("incomplete header must not clear user memory"),
        |_| panic!("incomplete header must not copyout"),
        |_| panic!("incomplete header must not dispatch"),
        |_, _| panic!("incomplete header must not allocate payload"),
    );
    assert_eq!(result.unwrap_err().error(), Errno::EFAULT);
}
