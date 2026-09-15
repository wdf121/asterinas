// SPDX-License-Identifier: MPL-2.0

//! Runtime resource workflows for Device Mapper control-plane mutations.
//!
//! This module coordinates a mapper's domain state with its runtime block-device
//! registration and devtmpfs nodes. It deliberately reuses the manager's name
//! reservation, the device's initial-resume guard, and the block registry's
//! registration transactions instead of duplicating their ownership or rollback
//! rules. Linux `dm_ioctl` decoding and response encoding remain in the parent
//! control-device module.

use aster_block::BlockDevice;
use aster_device_mapper::{DmDevice, DmManager};
use device_id::DeviceId;

use super::map_dm_error;
use crate::{
    device::{
        block_mapper_alias_is_published, block_open_count, publish_block_mapper_alias,
        register_block_mapper_primary, rename_block_mapper, unregister_block_mapper,
    },
    prelude::*,
};

/// Coordinates runtime node operations for one Device Mapper manager.
///
/// The coordinator owns no mapper or registry state. Each method preserves the
/// existing ordering between domain-state commits and external devtmpfs effects.
pub(super) struct MapperRuntimeCoordinator<'a> {
    manager: &'a DmManager,
}

impl<'a> MapperRuntimeCoordinator<'a> {
    /// Creates a coordinator for mutations belonging to `manager`.
    pub(super) fn new(manager: &'a DmManager) -> Self {
        Self { manager }
    }

    /// Returns whether the mapper has a registered runtime primary block node.
    pub(super) fn is_registered(&self, device: &DmDevice) -> bool {
        block_open_count(device.id()).is_some()
    }

    /// Returns whether the mapper's runtime registration currently owns an alias.
    pub(super) fn is_alias_published(&self, device: &DmDevice) -> Result<bool> {
        block_mapper_alias_is_published(device.id())
    }

    /// Publishes the primary node after the first fully validated table load.
    ///
    /// The block registry owns the pending-to-live transaction. On failure, this
    /// method leaves the mapper unregistered and does not alter its table state.
    pub(super) fn ensure_primary(&self, device: &Arc<DmDevice>) -> Result<()> {
        if !self.is_registered(device) {
            register_block_mapper_primary(device.clone())?;
        }
        Ok(())
    }

    /// Publishes the first mapper alias before committing inactive-to-active state.
    ///
    /// `InitialResumeGuard` keeps the device state private while the alias is
    /// created. Failure therefore leaves the table inactive and permits retry.
    pub(super) fn activate_initial(&self, device: &DmDevice) -> Result<()> {
        self.activate_initial_with(device, publish_block_mapper_alias)
    }

    /// Performs first activation with an injectable alias publisher for ktests.
    pub(super) fn activate_initial_with<F>(&self, device: &DmDevice, publish_alias: F) -> Result<()>
    where
        F: FnOnce(DeviceId, &str) -> Result<()>,
    {
        let name = device.name();
        let activation = device.begin_initial_resume().map_err(map_dm_error)?;
        publish_alias(device.id(), &name)?;
        activation.commit();
        Ok(())
    }

    /// Moves a published mapper alias and commits manager indexes atomically.
    ///
    /// The manager reserves `new_name` while the alias moves without holding its
    /// global lock. A failed move drops the reservation and preserves the old
    /// manager indexes, device name, and alias.
    pub(super) fn rename(&self, device: &Arc<DmDevice>, new_name: &str) -> Result<()> {
        self.rename_with(device, new_name, rename_block_mapper)
    }

    /// Performs runtime rename with an injectable alias mover for ktests.
    pub(super) fn rename_with<F>(
        &self,
        device: &Arc<DmDevice>,
        new_name: &str,
        rename_alias: F,
    ) -> Result<()>
    where
        F: FnOnce(DeviceId, &str, &str) -> Result<()>,
    {
        let old_name = device.name();
        if new_name == old_name {
            return Err(Error::with_message(
                Errno::EBUSY,
                "Device Mapper device name is unchanged",
            ));
        }

        let reservation = self
            .manager
            .reserve_runtime_rename(device.clone(), &old_name, new_name)
            .map_err(map_dm_error)?;
        rename_alias(device.id(), &old_name, new_name)?;
        reservation.commit();
        Ok(())
    }

    /// Removes runtime nodes and unregisters the primary block device if present.
    ///
    /// The block registry owns failure compensation and its `Removing` isolation
    /// state; this method must not remove the mapper from `DmManager` itself.
    pub(super) fn unregister_if_registered(&self, device: &DmDevice) -> Result<()> {
        if self.is_registered(device) {
            let name = device.name();
            unregister_block_mapper(device.id(), &name)?;
        }
        Ok(())
    }
}
