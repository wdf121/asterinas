// SPDX-License-Identifier: MPL-2.0

mod evdev;
mod fb;
mod mem;
pub(crate) mod misc;
mod pty;
mod registry;
mod shm;
pub(crate) mod tty;

use alloc::borrow::Cow;

use device_id::DeviceId;
pub(crate) use mem::{getrandom, geturandom};
pub(crate) use pty::{PtyMaster, PtySlave, new_pty_pair};
pub(crate) use registry::{
    block_mapper_alias_is_published, block_open_count, lookup, publish_block_mapper_alias,
    register_block_mapper_primary, rename_block_mapper, unregister_block_mapper,
};
use spin::Once;

use crate::{
    fs::{
        file::{InodeMode, InodeType, PerOpenFileOps, mkmod},
        ramfs::RamFs,
        utils::{NAME_MAX, PATH_MAX},
        vfs::{
            inode::{MknodType, RenameMode},
            path::{FsPath, Path, PathResolver, PerMountFlags},
            registry::FsAndRoot,
        },
    },
    prelude::*,
};

struct DevtmpfsLocation {
    // stage-1 会把 /dev 移动到新根；保存 devtmpfs 自身的根路径即可继续
    // 在同一个挂载实例中创建运行时节点，不能依赖移动前的挂载拓扑。
    root: Path,
}

static DEVTMPFS_ROOT: Once<DevtmpfsLocation> = Once::new();

/// The abstraction of a device.
pub(crate) trait Device: Send + Sync + 'static {
    /// Returns the device type.
    fn type_(&self) -> DeviceType;

    /// Returns the device ID.
    fn id(&self) -> DeviceId;

    /// Returns the metadata that specifies a device inode to be created in devtmpfs, if any.
    fn devtmpfs_meta(&self) -> Option<DevtmpfsInodeMeta<'_>>;

    /// Opens the device, returning a file-like object that the userspace can interact with by
    /// doing I/O.
    fn open(&self) -> Result<Box<dyn PerOpenFileOps>>;
}

impl Debug for dyn Device {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.debug_struct("Device")
            .field("type", &self.type_())
            .field("id", &self.id())
            .field("devtmpfs_meta", &self.devtmpfs_meta())
            .finish_non_exhaustive()
    }
}

/// Device type
#[derive(Debug)]
pub(crate) enum DeviceType {
    Char,
    Block,
}

/// The metadata that describes a device inode in devtmpfs.
///
/// The metadata contains the inode path relative to `/dev` and the
/// permission bits used when creating the inode. Device subsystems can use this
/// type to override the default mode.
///
/// If a device does not specify a mode explicitly, we use `mkmod!(u+rw)`,
/// matching Linux devtmpfs's default device inode permissions.
/// Reference: <https://elixir.bootlin.com/linux/v6.18/source/drivers/base/devtmpfs.c#L11>.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DevtmpfsInodeMeta<'a> {
    path: Cow<'a, str>,
    mode: InodeMode,
}

impl<'a> DevtmpfsInodeMeta<'a> {
    /// Creates the metadata for a devtmpfs inode with the default mode (`u+rw`).
    pub(crate) fn new(path: impl Into<Cow<'a, str>>) -> Self {
        Self {
            path: path.into(),
            mode: mkmod!(u+rw),
        }
    }

    /// Creates the metadata for a devtmpfs inode with the specified path and mode.
    pub(crate) fn with_mode(path: impl Into<Cow<'a, str>>, mode: InodeMode) -> Self {
        Self {
            path: path.into(),
            mode,
        }
    }

    /// Returns the device inode path relative to `/dev`.
    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    /// Returns the permission bits of the device inode.
    pub(crate) fn mode(&self) -> InodeMode {
        self.mode
    }
}

/// Adds a device node in `/dev`.
///
/// If the parent path does not exist, it will be created as a directory.
/// This function should be called when registering a device.
//
// TODO: Figure out what should happen when unregistering the device.
pub(crate) fn add_node(
    dev_type: DeviceType,
    dev_id: u64,
    meta: &DevtmpfsInodeMeta<'_>,
    path_resolver: &PathResolver,
) -> Result<Path> {
    let dev_path = path_resolver.lookup(&FsPath::try_from("/dev").unwrap())?;
    add_node_at(&dev_path, dev_type, dev_id, meta)
}

fn runtime_devtmpfs_root() -> Result<&'static Path> {
    DEVTMPFS_ROOT
        .get()
        .map(|location| &location.root)
        .ok_or_else(|| Error::with_message(Errno::ENODEV, "devtmpfs is not initialized"))
}

fn rollback_created_dirs(created_dirs: Vec<(Path, String, Path)>) {
    for (parent, name, created) in created_dirs.into_iter().rev() {
        let _ = parent.rmdir_if_matches(&name, &created);
    }
}

fn create_parent_dir(parent: &Path, name: &str) -> Result<(Path, bool)> {
    match parent.lookup_child(name) {
        Ok(path) => {
            if path.type_() != InodeType::Dir {
                return_errno_with_message!(
                    Errno::ENOTDIR,
                    "the device path parent is not a directory"
                );
            }
            Ok((path, false))
        }
        Err(error) if error.error() == Errno::ENOENT => {
            match parent.new_fs_child(name, InodeType::Dir, mkmod!(a+rx, u+w)) {
                Ok(path) => Ok((path, true)),
                Err(error) if error.error() == Errno::EEXIST => {
                    let path = parent.lookup_child(name)?;
                    if path.type_() != InodeType::Dir {
                        return_errno_with_message!(
                            Errno::ENOTDIR,
                            "the device path parent is not a directory"
                        );
                    }
                    Ok((path, false))
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

/// Adds a device node in the mounted devtmpfs at runtime.
pub fn add_runtime_node(
    dev_type: DeviceType,
    dev_id: u64,
    meta: &DevtmpfsInodeMeta<'_>,
) -> Result<Path> {
    let root = runtime_devtmpfs_root()?;
    add_node_at(root, dev_type, dev_id, meta)
}

fn add_node_at(
    root: &Path,
    dev_type: DeviceType,
    dev_id: u64,
    meta: &DevtmpfsInodeMeta<'_>,
) -> Result<Path> {
    let mut dev_path = root.clone();
    let mut relative_path = normalize_devtmpfs_path(meta.path())?;
    let mut created_dirs = Vec::new();

    while !relative_path.is_empty() {
        let (next_name, path_remain) = split_devtmpfs_path(relative_path);

        if path_remain.is_empty() {
            match dev_path.lookup_child(next_name) {
                Ok(_) => {
                    rollback_created_dirs(created_dirs);
                    return_errno_with_message!(Errno::EEXIST, "the device node already exists");
                }
                Err(error) if error.error() == Errno::ENOENT => {}
                Err(error) => {
                    rollback_created_dirs(created_dirs);
                    return Err(error);
                }
            }
            let mknod_type = match dev_type {
                DeviceType::Block => MknodType::BlockDevice(dev_id),
                DeviceType::Char => MknodType::CharDevice(dev_id),
            };
            return match dev_path.mknod(next_name, meta.mode(), mknod_type) {
                Ok(node) => Ok(node),
                Err(error) => {
                    rollback_created_dirs(created_dirs);
                    Err(error)
                }
            };
        }

        let parent = dev_path.clone();
        match create_parent_dir(&parent, next_name) {
            Ok((next_path, created)) => {
                if created {
                    created_dirs.push((parent, next_name.to_string(), next_path.clone()));
                }
                dev_path = next_path;
            }
            Err(error) => {
                rollback_created_dirs(created_dirs);
                return Err(error);
            }
        }
        relative_path = path_remain;
    }

    unreachable!()
}

/// Adds a symbolic link in the mounted devtmpfs at runtime.
pub fn add_runtime_symlink(path: &str, target: &str) -> Result<Path> {
    if target.is_empty() || target.len() > PATH_MAX || target.as_bytes().contains(&0) {
        return_errno_with_message!(Errno::EINVAL, "the symbolic link target is invalid");
    }
    let root = runtime_devtmpfs_root()?;
    let (parent, name, created_dirs) = lookup_or_create_parent(root, path)?;
    match parent.lookup_child(name) {
        Ok(_) => {
            rollback_created_dirs(created_dirs);
            return_errno_with_message!(Errno::EEXIST, "the symbolic link already exists");
        }
        Err(error) if error.error() == Errno::ENOENT => {}
        Err(error) => {
            rollback_created_dirs(created_dirs);
            return Err(error);
        }
    }

    let link = match parent.new_fs_child(name, InodeType::SymLink, mkmod!(a+rwx)) {
        Ok(link) => link,
        Err(error) => {
            rollback_created_dirs(created_dirs);
            return Err(error);
        }
    };
    if let Err(error) = link.inode().write_link(target) {
        let _ = parent.unlink_if_matches(name, &link);
        rollback_created_dirs(created_dirs);
        return Err(error);
    }
    Ok(link)
}

pub fn runtime_node(path: &str) -> Result<Path> {
    let root = runtime_devtmpfs_root()?;
    let (parent, name) = lookup_parent(root, path)?;
    parent.lookup_child(name)
}

/// 仅当节点仍是注册时创建的那个路径对象时删除它。
pub fn remove_owned_runtime_node(path: &str, expected: &Path) -> Result<()> {
    let root = runtime_devtmpfs_root()?;
    let (parent, name) = lookup_parent(root, path)?;
    parent.unlink_if_matches(name, expected)
}

/// 仅当旧名称仍指向预期节点时，以 no-replace 语义重命名运行时节点。
pub(crate) fn rename_runtime_node(old_path: &str, expected: &Path, new_path: &str) -> Result<()> {
    let root = runtime_devtmpfs_root()?;
    let (old_parent, old_name) = lookup_parent(root, old_path)?;
    let current = old_parent.lookup_child(old_name)?;
    if current != *expected {
        return_errno_with_message!(Errno::ESTALE, "the runtime device node is no longer owned");
    }
    let (new_parent, new_name) = lookup_parent(root, new_path)?;
    old_parent.rename(old_name, &new_parent, new_name, RenameMode::NoReplace)
}

fn lookup_or_create_parent<'a>(
    root: &Path,
    path: &'a str,
) -> Result<(Path, &'a str, Vec<(Path, String, Path)>)> {
    let mut parent = root.clone();
    let mut relative_path = normalize_devtmpfs_path(path)?;
    let mut created_dirs = Vec::new();

    loop {
        let (name, remain) = split_devtmpfs_path(relative_path);
        if remain.is_empty() {
            return Ok((parent, name, created_dirs));
        }
        let current_parent = parent.clone();
        match create_parent_dir(&current_parent, name) {
            Ok((next, created)) => {
                if created {
                    created_dirs.push((current_parent, name.to_string(), next.clone()));
                }
                parent = next;
            }
            Err(error) => {
                rollback_created_dirs(created_dirs);
                return Err(error);
            }
        }
        relative_path = remain;
    }
}

fn lookup_parent<'a>(root: &Path, path: &'a str) -> Result<(Path, &'a str)> {
    let mut parent = root.clone();
    let mut relative_path = normalize_devtmpfs_path(path)?;

    loop {
        let (name, remain) = split_devtmpfs_path(relative_path);
        if remain.is_empty() {
            return Ok((parent, name));
        }
        parent = parent.lookup_child(name)?;
        relative_path = remain;
    }
}

fn normalize_devtmpfs_path(path: &str) -> Result<&str> {
    if path.is_empty()
        || path.len() > PATH_MAX
        || path.as_bytes().contains(&0)
        || path.starts_with('/')
        || path.ends_with('/')
    {
        return_errno_with_message!(Errno::EINVAL, "the device path is invalid");
    }
    if path
        .split('/')
        .any(|name| name.is_empty() || name == "." || name == ".." || name.len() > NAME_MAX)
    {
        return_errno_with_message!(Errno::EINVAL, "the device path is invalid");
    }
    Ok(path)
}

fn split_devtmpfs_path(path: &str) -> (&str, &str) {
    path.split_once('/').unwrap_or((path, ""))
}

#[cfg(ktest)]
mod tests {
    use ostd::prelude::ktest;

    use super::*;

    #[ktest]
    fn validates_runtime_devtmpfs_paths() {
        assert_eq!(normalize_devtmpfs_path("dm-0").unwrap(), "dm-0");
        assert_eq!(
            normalize_devtmpfs_path("mapper/test-device").unwrap(),
            "mapper/test-device"
        );
        for path in [
            "",
            "/dm-0",
            "dm-0/",
            "mapper//test",
            ".",
            "..",
            "mapper/../dm-0",
            "mapper/bad\0name",
        ] {
            assert_eq!(
                normalize_devtmpfs_path(path).unwrap_err().error(),
                Errno::EINVAL
            );
        }
        let overlong_name = "x".repeat(NAME_MAX + 1);
        assert_eq!(
            normalize_devtmpfs_path(&overlong_name).unwrap_err().error(),
            Errno::EINVAL
        );
        let overlong_path = "x".repeat(PATH_MAX + 1);
        assert_eq!(
            normalize_devtmpfs_path(&overlong_path).unwrap_err().error(),
            Errno::EINVAL
        );
    }
}

pub(crate) fn init_in_first_kthread() {
    registry::init_in_first_kthread();
    mem::init_in_first_kthread();
    misc::init_in_first_kthread();
    evdev::init_in_first_kthread();
    fb::init_in_first_kthread();
}

/// Initializes the device nodes in devtmpfs after mounting rootfs.
pub(crate) fn init_in_first_process(ctx: &Context) -> Result<()> {
    let fs = ctx.thread_local.borrow_fs();
    let path_resolver = fs.resolver().read();

    // Mount devtmpfs.
    let dev_path = path_resolver.lookup(&FsPath::try_from("/dev")?)?;
    let devtmpfs_mount = dev_path.mount(
        FsAndRoot::new(RamFs::new()),
        PerMountFlags::default(),
        Some("ramfs".to_string()),
        ctx,
    )?;
    DEVTMPFS_ROOT.call_once(|| DevtmpfsLocation {
        root: Path::new_fs_root(devtmpfs_mount),
    });

    tty::init_in_first_process()?;
    pty::init_in_first_process(&path_resolver, ctx)?;
    shm::init_in_first_process(&path_resolver, ctx)?;
    registry::init_in_first_process(&path_resolver)?;

    Ok(())
}
