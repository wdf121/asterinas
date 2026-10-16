// SPDX-License-Identifier: MPL-2.0

//! Shared fixtures and entry point for the DM control-plane ktests.
//!
//! Test groups follow ABI, table, and lifecycle responsibilities rather than
//! implementation batches. All groups remain under `dm_control::tests`.

use alloc::{format, vec};
use core::{
    cell::{Cell, RefCell},
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

use aster_block::{
    BlockDevice, BlockDeviceLease, BlockDeviceMeta,
    bio::{Bio, BioDirection, BioSegment, BioStatus},
    id::Sid,
    register, unregister,
};
use aster_device_mapper::{
    DmDevice, DmError, DmManager, DmTable, TableError,
    target::{
        DmTarget, DmTargetBox, DmTargetMetadata, TargetIoAction, TargetStatusMode,
        error::ErrorTarget,
        linear::LinearTarget,
        striped::{StripedTarget, StripedTargetParams},
        zero::ZeroTarget,
    },
};
use device_id::{DeviceId, MajorId, MinorId};
use ostd::{
    prelude::ktest,
    task::{Task, TaskOptions},
};

use super::{
    DM_MANAGER, abi::*, commands::*, ioctl::*, manager, runtime::MapperRuntimeCoordinator,
    workflow::*,
};
use crate::{
    device::{block_open_count, register_block_mapper_primary_with_node_creator},
    prelude::*,
};

mod ioctl;
mod lifecycle;
mod table;

fn boxed<T: DmTarget + 'static>(target: T) -> DmTargetBox {
    Box::new(target)
}

#[derive(Debug)]
struct CountingStatusTarget {
    range: Range<Sid>,
    status_calls: Arc<Mutex<usize>>,
}

impl CountingStatusTarget {
    fn new(start: u64, length: u64, status_calls: Arc<Mutex<usize>>) -> Self {
        Self {
            range: Sid::new(start)..Sid::new(start + length),
            status_calls,
        }
    }
}

impl DmTarget for CountingStatusTarget {
    #[cfg(ktest)]
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn metadata(&self) -> DmTargetMetadata {
        DmTargetMetadata::new("counting", [1, 0, 0])
    }

    fn logical_range(&self) -> &Range<Sid> {
        &self.range
    }

    fn for_each_backing_id(&self, _f: &mut dyn FnMut(DeviceId)) {}

    fn for_each_backing<'a>(&'a self, _f: &mut dyn FnMut(&'a dyn BlockDevice)) {}

    fn status_params(&self, _mode: TargetStatusMode) -> Result<String, TableError> {
        *self.status_calls.lock() += 1;
        Ok(String::new())
    }

    fn map_io_range(&self, _logical: Range<Sid>) -> Option<Vec<TargetIoAction<'_>>> {
        None
    }
}

fn test_buffer(size: usize) -> Vec<u8> {
    let mut buffer = vec![0u8; size];
    write_u32(&mut buffer, OFF_DATA_SIZE, size).unwrap();
    write_u32(&mut buffer, OFF_DATA_START, DM_IOCTL_HEADER_SIZE).unwrap();
    buffer
}

#[derive(Debug)]
struct StatusBacking {
    id: DeviceId,
    submitted_ranges: Mutex<Vec<Range<Sid>>>,
}

impl StatusBacking {
    fn new(minor: u32) -> Arc<Self> {
        Self::new_with_major(1, minor)
    }

    fn new_with_major(major: u16, minor: u32) -> Arc<Self> {
        Arc::new(Self {
            id: DeviceId::new(MajorId::new(major), MinorId::new(minor)),
            submitted_ranges: Mutex::new(Vec::new()),
        })
    }
}

impl BlockDevice for StatusBacking {
    fn enqueue(
        &self,
        bio: aster_block::bio::SubmittedBio,
    ) -> core::result::Result<(), aster_block::bio::BioEnqueueError> {
        self.submitted_ranges.lock().push(bio.sid_range().clone());
        bio.complete(BioStatus::Complete);
        Ok(())
    }

    fn metadata(&self) -> BlockDeviceMeta {
        BlockDeviceMeta {
            max_nr_segments_per_bio: 8,
            nr_sectors: 1_024,
        }
    }

    fn name(&self) -> &str {
        "dm-status-test-backing"
    }

    fn id(&self) -> DeviceId {
        self.id
    }
}

#[derive(Debug)]
struct DeferredBacking {
    submitted: Mutex<Option<aster_block::bio::SubmittedBio>>,
}

impl DeferredBacking {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            submitted: Mutex::new(None),
        })
    }

    fn has_submitted_bio(&self) -> bool {
        self.submitted.lock().is_some()
    }

    fn complete(&self) {
        self.submitted
            .lock()
            .take()
            .expect("deferred backing has no submitted BIO")
            .complete(BioStatus::Complete);
    }
}

impl BlockDevice for DeferredBacking {
    fn enqueue(
        &self,
        bio: aster_block::bio::SubmittedBio,
    ) -> core::result::Result<(), aster_block::bio::BioEnqueueError> {
        let mut submitted = self.submitted.lock();
        if submitted.is_some() {
            return Err(aster_block::bio::BioEnqueueError::IsFull);
        }
        *submitted = Some(bio);
        Ok(())
    }

    fn metadata(&self) -> BlockDeviceMeta {
        BlockDeviceMeta {
            max_nr_segments_per_bio: 8,
            nr_sectors: 1_024,
        }
    }

    fn name(&self) -> &str {
        "dm-deferred-control-test-backing"
    }

    fn id(&self) -> DeviceId {
        DeviceId::new(MajorId::new(1), MinorId::new(99))
    }
}

fn status_backing_lease(minor: u32) -> BlockDeviceLease {
    BlockDeviceLease::new_untracked(StatusBacking::new(minor) as Arc<dyn BlockDevice>)
}

fn write_target_spec(
    buffer: &mut [u8],
    record: usize,
    start: u64,
    length: u64,
    next: u32,
    target_type: &str,
    params: &str,
) {
    write_u64(buffer, record, start).unwrap();
    write_u64(buffer, record + 8, length).unwrap();
    write_u32(buffer, record + 16, 0).unwrap();
    write_u32(buffer, record + 20, next).unwrap();
    write_c_string_fixed(buffer, record + 24, DM_TARGET_TYPE_LEN, target_type).unwrap();
    write_c_string(buffer, record + DM_TARGET_SPEC_SIZE, params).unwrap();
}

fn write_linear_target_spec(
    buffer: &mut [u8],
    record: usize,
    start: u64,
    length: u64,
    next: u32,
    params: &str,
) {
    write_target_spec(buffer, record, start, length, next, "linear", params);
}

fn write_error_target_spec(buffer: &mut [u8], record: usize, start: u64, length: u64, next: u32) {
    write_target_spec(buffer, record, start, length, next, "error", "");
}

fn write_zero_target_spec(buffer: &mut [u8], record: usize, start: u64, length: u64, next: u32) {
    write_target_spec(buffer, record, start, length, next, "zero", "");
}

fn single_error_table(length: u64) -> Arc<DmTable> {
    Arc::new(
        DmTable::new_targets(vec![boxed(ErrorTarget::new(Sid::new(0), length).unwrap())]).unwrap(),
    )
}

fn single_zero_table(length: u64) -> Arc<DmTable> {
    Arc::new(
        DmTable::new_targets(vec![boxed(ZeroTarget::new(Sid::new(0), length).unwrap())]).unwrap(),
    )
}

fn single_linear_table(length: u64, backing_start: u64, backing_minor: u32) -> Arc<DmTable> {
    Arc::new(
        DmTable::new_linear(vec![
            LinearTarget::new(
                Sid::new(0),
                length,
                Sid::new(backing_start),
                status_backing_lease(backing_minor),
            )
            .unwrap(),
        ])
        .unwrap(),
    )
}

fn single_striped_table(length: u64, params: &str, backing_minors: &[u32]) -> Arc<DmTable> {
    let backings = backing_minors
        .iter()
        .map(|minor| status_backing_lease(*minor))
        .collect();
    Arc::new(
        DmTable::new_targets(vec![boxed(
            StripedTarget::new(
                Sid::new(0),
                length,
                StripedTargetParams::parse(params).unwrap(),
                backings,
            )
            .unwrap(),
        )])
        .unwrap(),
    )
}

fn error_target(table: &DmTable, index: usize) -> &ErrorTarget {
    table.targets()[index]
        .downcast_ref::<ErrorTarget>()
        .expect("expected error target")
}

fn zero_target(table: &DmTable, index: usize) -> &ZeroTarget {
    table.targets()[index]
        .downcast_ref::<ZeroTarget>()
        .expect("expected zero target")
}

fn linear_target(table: &DmTable, index: usize) -> &LinearTarget {
    table.targets()[index]
        .downcast_ref::<LinearTarget>()
        .expect("expected linear target")
}

fn striped_target(table: &DmTable, index: usize) -> &StripedTarget {
    table.targets()[index]
        .downcast_ref::<StripedTarget>()
        .expect("expected striped target")
}

fn table_load_for_test(buffer: &mut [u8], device: &Arc<DmDevice>) -> Result<()> {
    table_load_for_device_with_primary(buffer, device, |_| Ok(()))
}

fn assert_failed_table_load_preserves_state(
    buffer: &mut [u8],
    device: &Arc<DmDevice>,
    errno: Errno,
) {
    let status_before = device.status();
    let active_before = device.active_table();
    let inactive_before = device.inactive_table();

    assert_eq!(
        table_load_for_test(buffer, device).unwrap_err().error(),
        errno
    );

    assert_eq!(device.status(), status_before);
    match (active_before, device.active_table()) {
        (Some(before), Some(after)) => assert!(Arc::ptr_eq(&before, &after)),
        (None, None) => {}
        _ => panic!("active table changed after failed table load"),
    }
    match (inactive_before, device.inactive_table()) {
        (Some(before), Some(after)) => assert!(Arc::ptr_eq(&before, &after)),
        (None, None) => {}
        _ => panic!("inactive table changed after failed table load"),
    }
}

fn request(size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    write_version(&mut bytes).unwrap();
    write_u32(&mut bytes, OFF_DATA_SIZE, size).unwrap();
    write_u32(&mut bytes, OFF_DATA_START, DM_IOCTL_HEADER_SIZE).unwrap();
    bytes
}

fn version() -> [u8; 12] {
    let mut bytes = [0; 12];
    write_version(&mut bytes).unwrap();
    bytes
}

fn zero_table(count: usize, readonly: bool) -> Arc<DmTable> {
    let targets: Vec<DmTargetBox> = (0..count)
        .map(|index| {
            Box::new(ZeroTarget::new(Sid::new(index as u64 * 8), 8).unwrap()) as DmTargetBox
        })
        .collect();
    Arc::new(DmTable::new_targets_with_readonly(targets, readonly).unwrap())
}
