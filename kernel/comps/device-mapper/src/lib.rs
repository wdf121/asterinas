// SPDX-License-Identifier: MPL-2.0

//! Asterinas 的 Device Mapper 核心组件。
//!
//! 第一版仅支持由单条 linear target 组成的映射表。Linux ioctl ABI 和
//! `/dev/mapper/control` 位于内核设备层，本组件只负责映射表、设备状态与 I/O 转发。

#![no_std]
#![deny(unsafe_code)]

extern crate alloc;

mod device;
mod manager;
mod table;
pub mod target;

pub use device::{DmDevice, DmDeviceStatus};
pub use manager::DmManager;
pub use table::DmTable;

/// Device Mapper 核心操作失败的原因。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmError {
    /// 名称已经存在。
    NameExists,
    /// UUID 已经存在。
    UuidExists,
    /// 找不到指定设备。
    DeviceNotFound,
    /// 指定 minor 已被占用。
    MinorBusy,
    /// minor 编号耗尽或超出范围。
    MinorExhausted,
    /// 块设备 major 编号耗尽。
    MajorExhausted,
    /// 设备当前状态不允许该操作。
    InvalidState,
    /// 映射表无效。
    InvalidTable(TableError),
}

impl From<TableError> for DmError {
    fn from(error: TableError) -> Self {
        Self::InvalidTable(error)
    }
}

/// 映射表验证失败的原因。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableError {
    /// 第一版只接受一条 target。
    UnsupportedTargetCount,
    /// 第一版要求逻辑范围从扇区 0 开始。
    UnsupportedLogicalStart,
    /// target 长度不能为零。
    ZeroLength,
    /// 逻辑范围发生整数溢出。
    LogicalRangeOverflow,
    /// 底层范围发生整数溢出。
    BackingRangeOverflow,
    /// 底层范围超过块设备容量。
    BackingRangeOutOfBounds,
    /// 底层设备类型不受第一版支持。
    UnsupportedBackingDevice,
    /// BIO 不完整地位于唯一 target 中。
    BioOutOfRange,
}
