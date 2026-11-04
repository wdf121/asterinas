// SPDX-License-Identifier: MPL-2.0

//! Device Mapper policy for publishing its primary block node.
//!
//! The generic block registry owns the publication transaction. The DM layer
//! chooses the `dm-N` name. LOAD validates a table before asking this module
//! to publish the primary node.

use aster_dm::DmDevice;

use crate::{device::registry::block, prelude::*};

/// Publishes `/dev/dm-N` for a mapper after its table has been validated.
///
/// Repeated publication of the same device is a no-op. A failed publication
/// remains the responsibility of the generic block lifecycle to clean up.
pub(super) fn ensure_primary(device: &Arc<DmDevice>) -> Result<()> {
    if block::is_runtime_primary_registered(device.id()) {
        return Ok(());
    }
    block::register_runtime_primary(device.clone(), device.primary_name().into())
}

#[cfg(ktest)]
mod tests {
    use ostd::prelude::ktest;

    use super::*;
    use crate::fs::devtmpfs;

    #[ktest]
    fn publishes_only_dm_primary_and_is_idempotent() {
        devtmpfs::init_for_ktest();
        let manager = aster_dm::DmManager::new().unwrap();
        let device = manager.create("phase1-primary".into(), None, None).unwrap();
        let path = device.primary_name();
        assert!(!block::is_runtime_primary_registered(device.id()));
        ensure_primary(&device).unwrap();
        assert!(block::is_runtime_primary_registered(device.id()));
        assert!(devtmpfs::lookup_path_for_ktest(path).is_ok());
        assert!(devtmpfs::lookup_path_for_ktest("mapper/phase1-primary").is_err());
        ensure_primary(&device).unwrap();
        block::unregister_runtime_primary(device.id()).unwrap();
        assert!(devtmpfs::lookup_path_for_ktest(path).is_err());
        manager.remove("phase1-primary").unwrap();
    }
}
