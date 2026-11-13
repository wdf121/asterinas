// SPDX-License-Identifier: MPL-2.0

//! VFS filesystem-type registration for ext2.
//!
//! `Ext2Type` implements the `FsType` trait so the VFS layer can
//! discover and mount ext2 volumes by name (`"ext2"`).

use aster_systree::SysNode;
use device_id::DeviceId;

use super::{fs::Ext2, prelude::*};
use crate::fs::vfs::{
    file_system::FileSystem,
    registry::{FsCache, FsCreationCtx, FsProperties, FsType},
};

/// VFS-visible Ext2 filesystem type.
pub(in crate::fs) struct Ext2Type {
    cache: FsCache<DeviceId>,
}

/// The VFS filesystem type descriptor for Ext2.
pub(in crate::fs) static EXT2_TYPE: Ext2Type = Ext2Type {
    cache: FsCache::new(),
};

impl FsType for Ext2Type {
    type Key = DeviceId;

    fn name(&self) -> &'static str {
        "ext2"
    }

    fn properties(&self) -> FsProperties {
        FsProperties::NEED_DISK
    }

    fn create(&self, fs_creation_ctx: &mut FsCreationCtx) -> Result<Arc<dyn FileSystem>> {
        let disk = fs_creation_ctx.resolve_block_device()?.clone();
        let flags = fs_creation_ctx.flags();
        let args = fs_creation_ctx.args();
        Ext2::open(disk, flags, args).map(|fs| fs as Arc<dyn FileSystem>)
    }

    fn obtain_key_and_cache(
        &self,
        fs_creation_ctx: &mut FsCreationCtx,
    ) -> Option<(DeviceId, &FsCache<DeviceId>)> {
        let key = fs_creation_ctx
            .resolve_block_device()
            .ok()
            .map(|disk| disk.id())?;

        Some((key, &self.cache))
    }

    fn sysnode(&self) -> Option<Arc<dyn SysNode>> {
        None
    }
}

#[cfg(ktest)]
mod test {
    use aster_block::{
        BlockDeviceMeta,
        bio::{BioEnqueueError, SubmittedBio},
    };
    use device_id::{MajorId, MinorId};
    use ostd::prelude::*;

    use super::*;
    use crate::fs::{
        fs_impls::ext2::test_utils::{Ext2FixtureBuilder, Ext2MemoryDisk},
        vfs::{file_system::FsFlags, registry::DynFsType},
    };

    #[derive(Debug)]
    struct MountDisk {
        disk: Arc<Ext2MemoryDisk>,
        id: DeviceId,
    }

    impl BlockDevice for MountDisk {
        fn enqueue(&self, bio: SubmittedBio) -> core::result::Result<(), BioEnqueueError> {
            self.disk.enqueue(bio)
        }

        fn metadata(&self) -> BlockDeviceMeta {
            self.disk.metadata()
        }

        fn name(&self) -> &str {
            "ext2-mount-lease-test"
        }

        fn id(&self) -> DeviceId {
            self.id
        }
    }

    fn mount_disk(minor: u32) -> Arc<MountDisk> {
        let fixture = Ext2FixtureBuilder::new(1, 256).build().unwrap();
        Arc::new(MountDisk {
            disk: fixture.disk,
            id: DeviceId::new(MajorId::new(240), MinorId::new(minor)),
        })
    }

    #[ktest]
    fn mounted_instance_and_cache_reuse_hold_device_lease() {
        let disk = mount_disk(100);
        aster_block::register(disk.clone()).unwrap();
        let fs_type = Ext2Type {
            cache: FsCache::new(),
        };
        let mut ctx = FsCreationCtx::from_block_device(
            aster_block::lookup_lease(disk.id()).unwrap(),
            FsFlags::empty(),
            None,
        );
        // Lease acquisition wins before filesystem construction or mount attachment.
        assert!(matches!(
            aster_block::begin_unregister(disk.id()),
            Err(aster_block::Error::Busy)
        ));
        let mounted = fs_type.get_or_create(&mut ctx).unwrap();
        drop(ctx);
        assert!(matches!(
            aster_block::begin_unregister(disk.id()),
            Err(aster_block::Error::Busy)
        ));

        let mut ctx = FsCreationCtx::from_block_device(
            aster_block::lookup_lease(disk.id()).unwrap(),
            FsFlags::empty(),
            None,
        );
        let cached = fs_type.get_or_create(&mut ctx).unwrap();
        assert!(Arc::ptr_eq(mounted.fs(), cached.fs()));
        drop(ctx);
        let old = Arc::downgrade(mounted.fs());
        drop(mounted);
        assert!(matches!(
            aster_block::begin_unregister(disk.id()),
            Err(aster_block::Error::Busy)
        ));
        drop(cached);
        assert!(old.upgrade().is_none());
        aster_block::unregister(disk.id()).unwrap();

        // Reuse the same device number while keeping the old cache entry.
        let replacement = mount_disk(100);
        aster_block::register(replacement.clone()).unwrap();
        let mut ctx = FsCreationCtx::from_block_device(
            aster_block::lookup_lease(replacement.id()).unwrap(),
            FsFlags::empty(),
            None,
        );
        let mounted = fs_type.get_or_create(&mut ctx).unwrap();
        assert!(old.upgrade().is_none());
        assert!(matches!(
            aster_block::begin_unregister(replacement.id()),
            Err(aster_block::Error::Busy)
        ));
        drop(ctx);
        drop(mounted);
        aster_block::unregister(replacement.id()).unwrap();
    }

    #[ktest]
    fn removal_isolation_prevents_mount_lease_acquisition() {
        let disk = mount_disk(101);
        aster_block::register(disk.clone()).unwrap();
        let removal = aster_block::begin_unregister(disk.id()).unwrap();
        assert!(aster_block::lookup_lease(disk.id()).is_none());
        aster_block::abort_unregister(removal).unwrap();
        assert!(aster_block::lookup_lease(disk.id()).is_some());
        aster_block::unregister(disk.id()).unwrap();
    }

    #[ktest]
    fn failed_filesystem_creation_releases_device_lease() {
        let disk = Arc::new(MountDisk {
            disk: Arc::new(Ext2MemoryDisk::new(256)),
            id: DeviceId::new(MajorId::new(240), MinorId::new(102)),
        });
        aster_block::register(disk.clone()).unwrap();
        let mut ctx = FsCreationCtx::from_block_device(
            aster_block::lookup_lease(disk.id()).unwrap(),
            FsFlags::empty(),
            None,
        );
        let fs_type = Ext2Type {
            cache: FsCache::new(),
        };
        assert!(fs_type.get_or_create(&mut ctx).is_err());
        drop(ctx);
        aster_block::unregister(disk.id()).unwrap();
    }
}
