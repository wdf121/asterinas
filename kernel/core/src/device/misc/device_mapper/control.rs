// SPDX-License-Identifier: MPL-2.0

//! Typed lifecycle workflows behind the Linux Device Mapper ioctl façade.
//!
//! The parent module owns raw `dm_ioctl` decoding, Linux errno mapping, and
//! response encoding. This module accepts already-decoded requests and
//! coordinates the existing manager, runtime publisher, and `DmDevice` state
//! machine without introducing VFS or ABI dependencies into the DM core crate.

use aster_block::BlockDevice;
use aster_device_mapper::{
    DmDevice, DmDeviceStatus, DmManager, DmTable,
    target::{TARGET_CATALOG, TargetStatusMode},
};
use device_id::DeviceId;

use super::{map_dm_error, map_table_error, runtime::MapperRuntimeCoordinator};
use crate::prelude::*;

/// Decoded parameters for `DM_DEV_CREATE`.
pub(super) struct CreateRequest {
    pub(super) name: String,
    pub(super) uuid: Option<String>,
    pub(super) requested_minor: Option<u32>,
}

/// Decoded mutation requested by `DM_DEV_RENAME`.
pub(super) enum RenameRequest {
    /// Replaces the mapper's user-visible name and, when published, its alias.
    Name(String),
    /// Replaces the mapper UUID without changing its name or alias.
    Uuid(String),
}

impl RenameRequest {
    /// Returns the stable diagnostic label for this mutation kind.
    pub(super) fn kind(&self) -> &'static str {
        match self {
            Self::Name(_) => "name",
            Self::Uuid(_) => "uuid",
        }
    }
}

/// Decoded action requested by `DM_DEV_SUSPEND`.
pub(super) enum LifecycleRequest {
    /// Stops new mapping I/O and optionally preserves assigned I/O in flight.
    Suspend { noflush: bool },
    /// Resumes a suspended mapper or activates its first loaded table.
    Resume,
}

/// Reports the lifecycle state transition committed by one workflow.
pub(super) enum LifecycleOutcome {
    /// A suspend transition committed using the requested drain policy.
    Suspended { noflush: bool },
    /// A resume transition committed; `initial` means the mapper alias published.
    Resumed { initial: bool },
}

/// Fully validated table state to install through `DM_TABLE_LOAD`.
///
/// The ioctl façade owns target-spec decoding and builds the immutable table before
/// constructing this request. The workflow publishes a first primary node before
/// committing the inactive table, so a runtime failure remains retryable.
pub(super) struct TableLoadRequest {
    table: Arc<DmTable>,
}

impl TableLoadRequest {
    /// Creates a request from a table that already passed all target validation.
    pub(super) fn new(table: Arc<DmTable>) -> Self {
        Self { table }
    }
}

/// Snapshot of mappers targeted by one best-effort `DM_REMOVE_ALL` request.
pub(super) struct RemoveAllRequest {
    devices: Vec<Arc<DmDevice>>,
}

impl RemoveAllRequest {
    /// Captures the manager snapshot before individual lifecycle guards are acquired.
    pub(super) fn new(devices: Vec<Arc<DmDevice>>) -> Self {
        Self { devices }
    }
}

/// Aggregates the visible result of one best-effort removal pass.
pub(super) struct RemoveAllOutcome {
    pub(super) attempted: usize,
    pub(super) removed: usize,
}

/// Immutable domain state needed to encode one Linux `dm_ioctl` device header.
pub(super) struct DeviceSnapshot {
    pub(super) id: DeviceId,
    pub(super) name: String,
    pub(super) uuid: Option<String>,
    pub(super) status: DmDeviceStatus,
}

/// Immutable count-only state for one selected Device Mapper table.
pub(super) struct TableMetadataSnapshot {
    pub(super) device: DeviceSnapshot,
    pub(super) target_count: usize,
}

/// Immutable dependency state for one selected Device Mapper table.
pub(super) struct TableDepsSnapshot {
    pub(super) device: DeviceSnapshot,
    pub(super) backing_ids: Vec<DeviceId>,
}

/// Lazily captures Linux-visible target records from one selected table.
///
/// The cursor retains the selected table generation while emitting detached target
/// snapshots one at a time. It lets the ABI façade stop at a full output buffer
/// without formatting parameters for later target records.
pub(super) struct TableRecordCursor {
    device: DeviceSnapshot,
    table: Option<Arc<DmTable>>,
    mode: TargetStatusMode,
    next_index: usize,
}

impl TableRecordCursor {
    /// Returns the selected mapper identity and lifecycle snapshot.
    pub(super) fn device(&self) -> &DeviceSnapshot {
        &self.device
    }

    /// Returns the target count captured from the selected table slot.
    pub(super) fn target_count(&self) -> usize {
        self.table.as_ref().map_or(0, |table| table.target_count())
    }

    /// Captures the next target record only when the ABI encoder needs it.
    pub(super) fn next_target(&mut self) -> Result<Option<TargetSnapshot>> {
        let Some(table) = self.table.as_ref() else {
            return Ok(None);
        };
        let Some(target) = table.targets().get(self.next_index) else {
            return Ok(None);
        };
        self.next_index += 1;
        Ok(Some(TargetSnapshot {
            logical_start: target.logical_range().start.to_raw(),
            length: target.length(),
            name: target.name(),
            params: target.status_params(self.mode).map_err(map_table_error)?,
        }))
    }
}

/// One target record detached from a `DmTarget` trait object.
pub(super) struct TargetSnapshot {
    pub(super) logical_start: u64,
    pub(super) length: u64,
    pub(super) name: &'static str,
    pub(super) params: String,
}

/// One static Linux target-version entry detached from the catalogue slice.
pub(super) struct TargetVersionSnapshot {
    pub(super) name: &'static str,
    pub(super) version: [u32; 3],
}

/// Captures immutable query state before the ioctl façade encodes ABI records.
pub(super) struct QueryWorkflow;

impl QueryWorkflow {
    /// Captures the current identity and active-table lifecycle state of one mapper.
    pub(super) fn device(device: &DmDevice) -> DeviceSnapshot {
        Self::selected_table_snapshot(device, false).0
    }

    /// Captures count-only state for the requested active or inactive table slot.
    pub(super) fn table_metadata(device: &DmDevice, inactive: bool) -> TableMetadataSnapshot {
        let (device, table) = Self::selected_table_snapshot(device, inactive);
        TableMetadataSnapshot {
            device,
            target_count: table.as_ref().map_or(0, |table| table.target_count()),
        }
    }

    /// Captures dependency IDs for the requested active or inactive table slot.
    pub(super) fn table_deps(device: &DmDevice, inactive: bool) -> TableDepsSnapshot {
        let (device, table) = Self::selected_table_snapshot(device, inactive);
        TableDepsSnapshot {
            device,
            backing_ids: table.map_or_else(Vec::new, |table| table.backing_ids()),
        }
    }

    /// Creates a lazy target-record cursor for the requested table slot.
    pub(super) fn table_records(
        device: &DmDevice,
        inactive: bool,
        mode: TargetStatusMode,
    ) -> TableRecordCursor {
        let (device, table) = Self::selected_table_snapshot(device, inactive);
        TableRecordCursor {
            device,
            table,
            mode,
            next_index: 0,
        }
    }

    /// Captures one selected table generation and its matching header status.
    fn selected_table_snapshot(
        device: &DmDevice,
        inactive: bool,
    ) -> (DeviceSnapshot, Option<Arc<DmTable>>) {
        let (status, table) = device.table_snapshot(inactive);
        (
            DeviceSnapshot {
                id: device.id(),
                name: device.mapper_name(),
                uuid: device.uuid(),
                status,
            },
            table,
        )
    }

    /// Captures a manager-selected sequence of visible mapper identities.
    pub(super) fn devices(devices: impl IntoIterator<Item = Arc<DmDevice>>) -> Vec<DeviceSnapshot> {
        devices
            .into_iter()
            .map(|device| Self::device(&device))
            .collect()
    }

    /// Captures the ordered static target-version catalogue.
    pub(super) fn target_versions() -> Vec<TargetVersionSnapshot> {
        TARGET_CATALOG
            .iter()
            .map(|target| TargetVersionSnapshot {
                name: target.name(),
                version: target.version(),
            })
            .collect()
    }

    /// Captures one static target-version entry by its Linux-visible name.
    pub(super) fn target_version(name: &str) -> Option<TargetVersionSnapshot> {
        TARGET_CATALOG
            .iter()
            .find(|target| target.name() == name)
            .map(|target| TargetVersionSnapshot {
                name: target.name(),
                version: target.version(),
            })
    }
}

/// Coordinates typed control-plane mutations for one manager.
///
/// Callers must hold the mapper lifecycle guard before invoking per-device
/// methods. The ioctl façade retains that guard because it also owns response
/// header encoding, which must observe the same current mapper instance.
pub(super) struct ControlWorkflow<'a> {
    manager: &'a DmManager,
}

impl<'a> ControlWorkflow<'a> {
    /// Creates a workflow bound to one manager.
    pub(super) fn new(manager: &'a DmManager) -> Self {
        Self { manager }
    }

    /// Creates a tableless mapper from validated `DM_DEV_CREATE` parameters.
    pub(super) fn create(&self, request: CreateRequest) -> Result<Arc<DmDevice>> {
        self.manager
            .create(request.name, request.uuid, request.requested_minor)
            .map_err(map_dm_error)
    }

    /// Publishes any missing primary runtime node and installs a validated table.
    pub(super) fn load_table(
        &self,
        device: &Arc<DmDevice>,
        request: TableLoadRequest,
    ) -> Result<()> {
        let runtime = MapperRuntimeCoordinator::new(self.manager);
        Self::load_table_with_primary(device, request, |device| runtime.ensure_primary(device))
    }

    /// Installs a table after an injectable primary publication step.
    ///
    /// Publication completes before the inactive slot changes. Because the table
    /// captures `DM_READONLY_FLAG`, a publication failure also preserves the current
    /// active I/O mode and leaves the request retryable.
    pub(super) fn load_table_with_primary<F>(
        device: &Arc<DmDevice>,
        request: TableLoadRequest,
        publish_primary: F,
    ) -> Result<()>
    where
        F: FnOnce(&Arc<DmDevice>) -> Result<()>,
    {
        publish_primary(device)?;
        device.load_table(request.table);
        Ok(())
    }

    /// Unregisters one current mapper before detaching its domain state.
    pub(super) fn remove(&self, device: &Arc<DmDevice>) -> Result<()> {
        let runtime = MapperRuntimeCoordinator::new(self.manager);
        self.remove_with_unregistration(device, |device| runtime.unregister_if_registered(device))
    }

    /// Removes one mapper after an injectable runtime unregistration step.
    ///
    /// Runtime unregistration is the only fallible action before event publication
    /// and manager detach. A failure therefore leaves postponed BIOs, event number,
    /// and manager indexes untouched for retry or `Removing` isolation handling.
    pub(super) fn remove_with_unregistration<F>(
        &self,
        device: &Arc<DmDevice>,
        unregister_runtime: F,
    ) -> Result<()>
    where
        F: FnOnce(&DmDevice) -> Result<()>,
    {
        unregister_runtime(device)?;
        device.fail_postponed_bios();
        device.notify_event();
        self.manager
            .remove(&device.mapper_name())
            .map_err(map_dm_error)?;
        Ok(())
    }

    /// Runs best-effort removal for a mapper snapshot.
    ///
    /// The caller retains lifecycle-guard ownership and supplies one guarded remove
    /// operation. Failures deliberately retain that mapper while the remaining
    /// snapshot entries continue, matching Linux `DM_REMOVE_ALL` behavior.
    pub(super) fn remove_all<F>(
        &self,
        request: RemoveAllRequest,
        mut remove_current: F,
    ) -> RemoveAllOutcome
    where
        F: FnMut(&Arc<DmDevice>) -> Result<()>,
    {
        let attempted = request.devices.len();
        let removed = request
            .devices
            .into_iter()
            .filter(|device| remove_current(device).is_ok())
            .count();
        RemoveAllOutcome { attempted, removed }
    }

    /// Commits a name or UUID mutation for a current mapper.
    pub(super) fn rename(&self, device: &Arc<DmDevice>, request: &RenameRequest) -> Result<()> {
        match request {
            RenameRequest::Uuid(uuid) => {
                if uuid.is_empty() {
                    return Err(Error::with_message(
                        Errno::EINVAL,
                        "Device Mapper UUID 不能为空",
                    ));
                }
                self.manager
                    .rename_uuid(&device.mapper_name(), uuid.clone())
                    .map_err(map_dm_error)
            }
            RenameRequest::Name(new_name) => {
                let runtime = MapperRuntimeCoordinator::new(self.manager);
                if runtime.is_registered(device)
                    && (device.active_table().is_some() || runtime.is_alias_published(device)?)
                {
                    runtime.rename(device, new_name)
                } else {
                    self.manager
                        .rename(&device.mapper_name(), new_name)
                        .map_err(map_dm_error)
                }
            }
        }
    }

    /// Commits one suspend or resume state transition for a current mapper.
    pub(super) fn transition(
        &self,
        device: &Arc<DmDevice>,
        request: LifecycleRequest,
    ) -> Result<LifecycleOutcome> {
        match request {
            LifecycleRequest::Suspend { noflush } => {
                (if noflush {
                    device.suspend_no_flush()
                } else {
                    device.suspend()
                })
                .map_err(map_dm_error)?;
                Ok(LifecycleOutcome::Suspended { noflush })
            }
            LifecycleRequest::Resume => {
                let runtime = MapperRuntimeCoordinator::new(self.manager);
                if !runtime.is_registered(device) || runtime.is_alias_published(device)? {
                    device.resume().map_err(map_dm_error)?;
                    Ok(LifecycleOutcome::Resumed { initial: false })
                } else {
                    runtime.activate_initial(device)?;
                    Ok(LifecycleOutcome::Resumed { initial: true })
                }
            }
        }
    }

    /// Clears the current mapper's inactive table.
    pub(super) fn clear_inactive_table(&self, device: &Arc<DmDevice>) -> Result<()> {
        device.clear_inactive_table().map_err(map_dm_error)
    }
}
