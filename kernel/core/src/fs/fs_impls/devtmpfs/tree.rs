// SPDX-License-Identifier: MPL-2.0

//! Filesystem-tree operations for creating, validating, and removing devtmpfs nodes.
//!
//! The singleton devtmpfs backing is a tmpfs-flavored `RamFs`. Each successful
//! creation returns a [`DevtmpfsHandle`] carrying the exact `RamInode` identity
//! installed in the tree. Destructive operations recheck that identity while the
//! relevant directory locks are held, so an externally replaced path is never
//! removed or renamed as though it were still devtmpfs-owned.

use alloc::borrow::Cow;

use device_id::DeviceId;

use crate::{
    device::DeviceType,
    fs::{
        file::{InodeMode, InodeType, mkmod},
        fs_impls::ramfs::RamInode,
        utils::{NAME_MAX, PATH_MAX},
        vfs::{
            file_system::FileSystem,
            inode::{Inode, MknodType},
            path::{self, SplitPath},
        },
    },
    prelude::*,
};

/// The metadata that describes a devtmpfs node.
///
/// The metadata contains the inode path relative to `/dev` and the permission
/// bits used when creating the inode. Device subsystems can use this type to
/// override the default mode.
///
/// If a device does not specify a mode explicitly, we use `mkmod!(u+rw)`,
/// matching Linux devtmpfs's default device inode permissions.
/// Reference: <https://elixir.bootlin.com/linux/v6.18/source/drivers/base/devtmpfs.c#L11>.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevtmpfsNodeMeta {
    path: Cow<'static, str>,
    mode: InodeMode,
}

/// An error returned by [`DevtmpfsNodeMeta::new`] and [`DevtmpfsNodeMeta::with_mode`].
#[derive(Clone, Copy, Debug)]
pub struct InvalidDevtmpfsPath;

impl DevtmpfsNodeMeta {
    /// Creates the metadata for a devtmpfs inode with the default mode (`u+rw`).
    ///
    /// `path` must be a non-empty, relative, well-formed path. For example,
    /// `a/b/c` is valid, whereas `/`, `/abc`, `a//b`, and `a/b/` are invalid.
    pub fn new(path: impl Into<Cow<'static, str>>) -> Result<Self, InvalidDevtmpfsPath> {
        Self::with_mode(path, mkmod!(u+rw))
    }

    /// Creates the metadata for a devtmpfs inode with the specified mode.
    ///
    /// The path follows the same requirements as [`Self::new`].
    pub(crate) fn with_mode(
        path: impl Into<Cow<'static, str>>,
        mode: InodeMode,
    ) -> Result<Self, InvalidDevtmpfsPath> {
        let path = path.into();
        if path.is_empty()
            || path.starts_with('/')
            || path.len() > PATH_MAX
            || path.as_bytes().contains(&0)
            || path.split('/').any(|component| {
                component.is_empty()
                    || component.len() > NAME_MAX
                    || path::is_dot_or_dotdot(component)
            })
        {
            return Err(InvalidDevtmpfsPath);
        }
        Ok(Self { path, mode })
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn mode(&self) -> InodeMode {
        self.mode
    }
}

/// The complete description of a device node managed by devtmpfs.
pub(crate) struct DevtmpfsNode {
    device_type: DeviceType,
    device_id: DeviceId,
    meta: DevtmpfsNodeMeta,
}

impl DevtmpfsNode {
    pub(crate) fn new(
        device_type: DeviceType,
        device_id: DeviceId,
        meta: DevtmpfsNodeMeta,
    ) -> Self {
        Self {
            device_type,
            device_id,
            meta,
        }
    }
}

/// An owned devtmpfs tree entry.
///
/// The inode identity is private so callers can only remove, validate, or move
/// the entry through devtmpfsd. `path` is relative to the devtmpfs root and is
/// updated after a successful identity-checked rename.
#[derive(Clone)]
pub(crate) struct DevtmpfsHandle {
    inode: Arc<RamInode>,
    path: String,
}

impl core::fmt::Debug for DevtmpfsHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DevtmpfsHandle")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl DevtmpfsHandle {
    fn new(inode: Arc<RamInode>, path: &str) -> Self {
        Self {
            inode,
            path: path.into(),
        }
    }
}

pub(super) fn create_node(node: &DevtmpfsNode) -> Result<DevtmpfsHandle> {
    let (parent_path, node_name) = node.meta.path().split_dirname_and_basename().unwrap();
    let parent_inode = lookup_or_create_path(parent_path)?;
    let inode = create_device_node(parent_inode.as_ref(), node_name, node)?;
    Ok(DevtmpfsHandle::new(inode, node.meta.path()))
}

pub(super) fn create_symlink(path: &str, target: &str) -> Result<DevtmpfsHandle> {
    validate_relative_path(path)?;
    if target.is_empty() || target.len() > PATH_MAX || target.as_bytes().contains(&0) {
        return_errno_with_message!(Errno::EINVAL, "the symbolic link target is invalid");
    }
    let (parent_path, name) = path
        .split_dirname_and_basename()
        .map_err(|_| Error::with_message(Errno::EINVAL, "the symbolic link path is invalid"))?;
    let parent = lookup_or_create_path(parent_path)?;
    let parent = Arc::downcast::<RamInode>(parent).unwrap();
    let inode = parent.create_symlink_with_revalidation(name, target, mkmod!(a+rwx))?;
    Ok(DevtmpfsHandle::new(
        Arc::downcast::<RamInode>(inode).unwrap(),
        path,
    ))
}

/// Validates that `handle` still owns the entry at its recorded path.
pub(super) fn validate(handle: &DevtmpfsHandle) -> Result<()> {
    let current = lookup_path(handle.path.as_str())?;
    let current = Arc::downcast::<RamInode>(current).unwrap();
    if !Arc::ptr_eq(&current, &handle.inode) {
        return_errno_with_message!(Errno::ESTALE, "the devtmpfs entry was replaced");
    }
    Ok(())
}

/// Deletes an entry only if it is still the inode created for `handle`.
pub(super) fn delete(handle: DevtmpfsHandle) -> Result<()> {
    let (parent_path, name) = handle.path.split_dirname_and_basename().unwrap();
    let parent = Arc::downcast::<RamInode>(lookup_path(parent_path)?).unwrap();
    match handle.inode.type_() {
        InodeType::Dir => parent.rmdir_if_same(name, &handle.inode)?,
        _ => parent.unlink_if_same(name, &handle.inode)?,
    }
    remove_empty_parent_dirs(parent_path);
    Ok(())
}

/// Renames an entry with no-replace semantics after verifying its identity.
pub(super) fn rename_no_replace(handle: &mut DevtmpfsHandle, new_path: &str) -> Result<()> {
    validate_relative_path(new_path)?;
    let (old_parent_path, old_name) = handle.path.split_dirname_and_basename().unwrap();
    let (new_parent_path, new_name) = new_path.split_dirname_and_basename().unwrap();
    let old_parent = Arc::downcast::<RamInode>(lookup_path(old_parent_path)?).unwrap();
    let new_parent = Arc::downcast::<RamInode>(lookup_path(new_parent_path)?).unwrap();
    old_parent.rename_no_replace_if_same(old_name, &handle.inode, &new_parent, new_name)?;
    handle.path = new_path.into();
    Ok(())
}

/// Retains the legacy best-effort deletion semantics for ordinary device callers.
pub(super) fn delete_node(node: &DevtmpfsNode) -> Result<()> {
    let (parent_path, node_name) = node.meta.path().split_dirname_and_basename().unwrap();
    let parent_inode = lookup_path(parent_path)?;
    let parent_ram_inode = parent_inode.downcast_ref::<RamInode>().unwrap();

    if parent_ram_inode.unlink_if(node_name, |inode| matches_device(inode, node))? {
        remove_empty_parent_dirs(parent_path);
    }
    Ok(())
}

pub(super) fn lookup_path(path: &str) -> Result<Arc<dyn Inode>> {
    let mut current = super::fs::singleton().root_inode();
    for name in path.split('/').filter(|name| !name.is_empty()) {
        current = current.lookup(name)?;
    }
    Ok(current)
}

fn lookup_or_create_path(path: &str) -> Result<Arc<dyn Inode>> {
    let mut current = super::fs::singleton().root_inode();
    for name in path.split('/').filter(|name| !name.is_empty()) {
        current = lookup_or_create_dir(current.as_ref(), name)?;
    }
    Ok(current)
}

fn lookup_or_create_dir(parent_inode: &dyn Inode, name: &str) -> Result<Arc<dyn Inode>> {
    let parent_ram_inode = parent_inode.downcast_ref::<RamInode>().unwrap();

    loop {
        let error = match parent_ram_inode.mkdir_with_revalidation(name, mkmod!(a+rx, u+w)) {
            Ok(inode) => return Ok(inode),
            Err(error) => error,
        };
        if error.error() != Errno::EEXIST {
            return Err(error);
        }

        match parent_inode.lookup(name) {
            Ok(inode) if inode.type_() == InodeType::Dir => return Ok(inode),
            Ok(_) => {
                return_errno_with_message!(Errno::ENOTDIR, "the parent path is not a directory")
            }
            Err(error) if error.error() == Errno::ENOENT => continue,
            Err(error) => return Err(error),
        }
    }
}

fn create_device_node(
    parent_inode: &dyn Inode,
    name: &str,
    node: &DevtmpfsNode,
) -> Result<Arc<RamInode>> {
    let rdev = node.device_id.as_encoded_u64();
    let mknod_type = match node.device_type {
        DeviceType::Block => MknodType::BlockDevice(rdev),
        DeviceType::Char => MknodType::CharDevice(rdev),
    };

    let parent_ram_inode = parent_inode.downcast_ref::<RamInode>().unwrap();
    let inode = parent_ram_inode.mknod_with_revalidation(name, node.meta.mode(), mknod_type)?;
    Ok(Arc::downcast::<RamInode>(inode).unwrap())
}

fn remove_empty_parent_dirs(path: &str) {
    let mut path = path;

    while let Ok((parent_path, name)) = path.split_dirname_and_basename() {
        let parent_inode = match lookup_path(parent_path) {
            Ok(inode) => inode,
            Err(_) => break,
        };
        let parent_ram_inode = parent_inode.downcast_ref::<RamInode>().unwrap();
        match parent_ram_inode.rmdir_if(name, |inode| inode.to_be_revalidated()) {
            Ok(true) => {}
            _ => break,
        }
        path = parent_path;
    }
}

fn validate_relative_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.starts_with('/')
        || path.len() > PATH_MAX
        || path
            .split('/')
            .any(|component| component.is_empty() || path::is_dot_or_dotdot(component))
    {
        return_errno_with_message!(Errno::EINVAL, "the devtmpfs path is invalid");
    }
    Ok(())
}

fn matches_device(inode: &RamInode, node: &DevtmpfsNode) -> bool {
    if !inode.to_be_revalidated() {
        return false;
    }

    let expected_type = match node.device_type {
        DeviceType::Block => InodeType::BlockDevice,
        DeviceType::Char => InodeType::CharDevice,
    };

    inode.type_() == expected_type && inode.metadata().unwrap().self_dev_id == Some(node.device_id)
}
