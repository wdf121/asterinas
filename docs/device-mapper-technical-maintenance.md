# Asterinas Device Mapper 技术与设计文档

## 文档信息

| 项目 | 内容 |
|---|---|
| 版本 | v2.0 |
| 状态 | 草案，基于当前 `dm` 分支实现整理 |
| 目标读者 | 内核开发人员、架构评审人员、测试与集成维护人员 |
| 更新时间 | 2026-08-25 |
| 相关进度 | [Device Mapper 项目进度](../log/device-mapper-progress.md) |

## 执行摘要

Asterinas Device Mapper 的设计目标不是一次性复刻完整 Linux Device Mapper 生态，而是在 Asterinas 当前块设备、VFS、devtmpfs、procfs 和 NixOS guest 测试框架内，实现一个能被真实 `dmsetup`、LVM2、ext2 和 reboot recovery 路径验证的 Linux DM 核心 ABI 兼容子集。

本文重新按“整体路线 + 功能路线”组织设计说明：先说明 DM 分支从兼容目标到系统验收的全路线，再分别展开控制面、数据面、target 能力、mixed table、striped 几何、flush、LVM2 场景和测试矩阵。这样可以避免只看局部流程图时误解当前实现范围，也能清楚看到每个能力从用户态入口到内核语义再到验收出口的闭环。

当前已支持的核心能力包括：

- `/dev/mapper/control` 控制设备和 Linux DM ioctl 核心命令；
- mapper device 创建、删除、重命名、状态查询、wait、table load/status/deps；
- active / inactive table 生命周期，以及 suspend / resume 激活语义；
- `linear`、`striped` 和同一 table 内 `linear + striped` mixed target；
- Read / Write / Flush BIO remap、跨 target split、striped chunk split 和 completion 聚合；
- LVM2 linear 基础 / cross-segment、striped N-way 基础 / N-to-2N cross-segment、linear+striped mixed table 的系统验收路径。

设计结论：后续新增 target 或新增 BIO 语义时，不能只补局部代码；必须同时定义 Linux DM control ABI、table/status/deps 输出、数据面映射、flush 行为、错误传播和验收路径，保证能力从用户态到块层闭环。

## 目录

1. [背景与现状](#1-背景与现状)
2. [整体路线图](#2-整体路线图)
3. [目标与非目标](#3-目标与非目标)
4. [术语与定义](#4-术语与定义)
5. [需求分析](#5-需求分析)
6. [总体架构设计](#6-总体架构设计)
7. [控制面功能路线](#7-控制面功能路线)
8. [数据面功能路线](#8-数据面功能路线)
9. [Target 能力路线](#9-target-能力路线)
10. [系统集成路线](#10-系统集成路线)
11. [测试与验收路线](#11-测试与验收路线)
12. [技术选型与设计决策汇总](#12-技术选型与设计决策汇总)
13. [风险与待确认事项](#13-风险与待确认事项)
14. [验收标准](#14-验收标准)
15. [附录](#15-附录)

## 1. 背景与现状

Linux Device Mapper 是 LVM2、dmsetup 等用户态工具依赖的块设备映射基础设施。它将一个或多个底层块设备组合成新的逻辑块设备，并通过 table 描述逻辑 sector 到 backing device sector 的映射关系。

Asterinas 当前没有完整 Linux udev、sysfs DM 层级、uevent 自动联动和完整 block queue stacking 生态。因此本设计选择实现一个最小但真实可用的 DM 兼容子集：让 `dmsetup`、LVM2、ext2、真实块设备和 reboot recovery 测试路径能够闭环，而不是声明支持完整 Linux DM。

当前实现已经完成上游 `kernel/core` 目录迁移适配，DM core 位于 [kernel/core/comps/device-mapper](../kernel/core/comps/device-mapper)，控制面入口位于 [kernel/core/src/device/misc/device_mapper.rs](../kernel/core/src/device/misc/device_mapper.rs)。

## 2. 整体路线图

### 2.1 DM 分支端到端全路线

DM 分支的端到端路线是：用户态 `dmsetup` / LVM2 通过 `/dev/mapper/control` 进入 DM ioctl 控制面，控制面创建 mapper、装载 inactive table 并在 resume 后切换为 active table；文件系统和应用 I/O 通过 `/dev/mapper/<name>` 或 `/dev/dm-N` 进入 mapper 块设备，随后由 DM table 按 segment 和 target 规则 remap 到 backing block device。

这条路线依赖 Asterinas 通用内核框架提供设备节点、block registry、BIO submit/completion、VFS mount 和 backing 生命周期保护；Device Mapper 专属实现只负责 Linux DM ABI、mapper/table 生命周期、target 参数解析、status/deps 查询和 BIO 数据面映射。

### 2.2 能力边界与非目标关系

当前设计目标是让 `dmsetup`、LVM2、ext2、真实 backing block device 和 reboot recovery 测试路径闭环；不承诺完整 Linux DM 生态。完整 udev/uevent/systemd 自动联动、完整 DM sysfs 层级、复杂 target、discard/write zeroes、完整 queue stacking 和 DM-on-DM backing 都属于非目标或后续独立阶段。

## 3. 目标与非目标

### 3.1 项目目标

1. 支持 Linux DM 核心控制面 ABI，使 `dmsetup` 和 LVM2 可以完成基础 mapper 生命周期操作。
2. 支持 `linear` 与 `striped` target 的 table load、status、deps 和数据面映射。
3. 支持同一 DM table 内多个 target 的连续逻辑地址空间，包括 `linear + striped` mixed table。
4. 保证 active / inactive table、suspend / resume、failed load、readonly、event wait 等状态机语义可预测。
5. 保证 Read / Write / Flush BIO 的 remap、split、fan-out 和 completion 聚合正确。
6. 通过 ktest 和 NixOS/LVM2 系统脚本验证关键内核语义与真实用户态路径。

### 3.2 非目标

当前不实现或不承诺：

- 完整 udev / uevent / systemd 自动联动；
- 完整 Linux DM sysfs 层级；
- target registry 泛化框架；
- `crypt`、`snapshot`、`thin`、`mirror`、`multipath` 等复杂 target；
- `DM_TARGET_MSG`、`DM_DEV_SET_GEOMETRY`、`DM_DEV_ARM_POLL`；
- discard / write zeroes BIO 语义；
- Linux DM 完整 queue stacking 规则；
- DM-on-DM backing。

### 3.3 假设与边界

- 假设 sector 单位为 512 字节，与当前 block 层和 DM table 参数保持一致。
- 假设 LVM2 测试路径可通过 `activation { udev_rules=0 }` 避开 udev 依赖。
- 当前系统验收结论来自 `dm` 分支已有测试记录；本文不新增测试结果，也不虚构性能数据。
- 当前已实现的是 `/dev/dm-*` 与 `/dev/mapper/*` runtime node 的最小 devtmpfs 集成；udev rules、uevent 驱动的自动发现和 systemd/LVM 自动激活仍属后续独立阶段。
- 本文描述当前实现与设计边界，不代表完整 Linux Device Mapper 兼容承诺。

## 4. 术语与定义

| 术语 | 定义 |
|---|---|
| DM | Device Mapper，将逻辑块设备请求映射到底层块设备的内核框架。 |
| mapper device | DM 创建出的逻辑块设备，例如 `/dev/dm-0` 或 `/dev/mapper/<name>`。 |
| backing device | mapper table 中引用的底层块设备。 |
| table | 描述逻辑 sector range 到 target 的映射表。 |
| active table | 当前正在服务 I/O 的 table。 |
| inactive table | `DM_TABLE_LOAD` 写入、等待 resume 激活的 table。 |
| target | table 中的一段映射规则，例如 `linear` 或 `striped`。 |
| BIO | 块 I/O 请求对象，包含操作类型、sector range、segments 和 completion。 |
| split | 将一个跨映射边界的 BIO 拆成多个 child BIO。 |
| deps | table 依赖的 backing device 集合。 |
| mixed table | 同一个 table 中同时包含不同 target 类型，例如前半段 linear、后半段 striped。 |

## 5. 需求分析

### 5.1 功能需求

| 编号 | 需求 | 当前状态 |
|---|---|---|
| FR-1 | 提供 `/dev/mapper/control` 控制设备 | 已支持 |
| FR-2 | 支持 Linux DM ioctl header 解析、版本校验和 header 写回 | 已支持 |
| FR-3 | 支持 mapper create/remove/remove_all/rename/status/list/wait | 已支持 |
| FR-4 | 支持 active/inactive table 生命周期 | 已支持 |
| FR-5 | 支持 `linear` target | 已支持 |
| FR-6 | 支持 `striped` target | 已支持 |
| FR-7 | 支持 mixed linear + striped table | 已支持 |
| FR-8 | 支持 table status/deps 的 active/inactive 查询 | 已支持 |
| FR-9 | 支持 Read/Write/Flush 数据面映射 | 已支持 |
| FR-10 | 支持 BIO 跨 target 和跨 striped chunk 拆分 | 已支持 |
| FR-11 | 支持 LVM2 create/grow/shrink/reboot recovery 验收路径 | 已覆盖主要路径 |

### 5.2 非功能需求

| 类型 | 要求 | 设计响应 |
|---|---|---|
| 正确性 | table 生命周期、sector 映射和 completion 必须可验证 | ktest 覆盖内核语义，系统脚本覆盖真实用户态路径 |
| 安全性 | kernel 侧保持 safe Rust，不绕过块设备生命周期 | 使用 `BlockDeviceLease` 持有 backing 生命周期 |
| 兼容性 | 对齐 Linux DM 核心 ABI，但不假支持未实现语义 | flags 分类处理；未实现命令显式拒绝 |
| 可维护性 | target 与 table 逻辑边界清晰 | `DmManager` / `DmDevice` / `DmTable` / `DmTarget` 分层 |
| 可测试性 | 每类关键行为有窄测试入口 | ktest + 分层系统验收脚本 |
| 可用性 | LVM2、dmsetup、ext2 能走真实路径 | NixOS guest 提供实际工具链和测试盘 locator |

### 5.3 优先级

| 优先级 | 内容 |
|---|---|
| P0 | DM core 语义正确性：ioctl、table 生命周期、BIO remap/split/completion。 |
| P1 | 真实用户态路径：dmsetup、LVM2、ext2、reboot recovery。 |
| P2 | 测试矩阵稳定化和文档化。 |
| P3 | udev / systemd / LVM 自动扫描与自动激活、完整 sysfs DM 层级等框架集成。 |

## 6. 总体架构设计

总体架构采用四层路线：用户态工具通过控制设备进入 ioctl 适配层，ioctl 适配层操作 DM core，DM core 通过 block registry 和 BIO 接入底层块设备，devtmpfs/procfs/VFS 提供用户态可见性。

### 6.1 代码位置

| 层级 | 文件或目录 | 职责 |
|---|---|---|
| DM crate | [kernel/core/comps/device-mapper](../kernel/core/comps/device-mapper) | DM core crate。 |
| 控制面 | [kernel/core/src/device/misc/device_mapper.rs](../kernel/core/src/device/misc/device_mapper.rs) | `/dev/mapper/control`、Linux DM ioctl ABI、table load/status/deps。 |
| block 支撑 | [kernel/core/comps/block/src/lib.rs](../kernel/core/comps/block/src/lib.rs) | `BlockDevice`、registry、lease、lookup。 |
| BIO 支撑 | [kernel/core/comps/block/src/bio.rs](../kernel/core/comps/block/src/bio.rs) | BIO remap、split、segment slice、completion。 |
| runtime node | [kernel/core/src/device/mod.rs](../kernel/core/src/device/mod.rs) | `/dev/dm-N` 和 `/dev/mapper/<name>` node 管理。 |
| procfs | [kernel/core/src/fs/fs_impls/procfs/devices.rs](../kernel/core/src/fs/fs_impls/procfs/devices.rs) | `/proc/devices` 输出 block major。 |
| VFS mount | [kernel/core/src/fs/vfs/fs_apis/registry.rs](../kernel/core/src/fs/vfs/fs_apis/registry.rs) | mount source 到 `BlockDeviceLease` 的解析。 |
| 系统测试入口 | [myshell/run_dm_system_tests.sh](../myshell/run_dm_system_tests.sh) | DM 系统验收组合入口。 |

### 6.2 内核模块协作路线

控制面协作路线：`device_mapper.rs` 解析 Linux DM ioctl 和用户 buffer，调用 DM core 创建 `DmDevice`、装载 `DmTable`、切换 active/inactive table，并通过 devtmpfs 暴露 `/dev/mapper/<name>` 和 `/dev/dm-N`。

数据面协作路线：VFS 和文件系统把 mapper 设备上的请求转换为 BIO；DM core 根据 active table 查找 segment，再由 `linear` 或 `striped` target 计算 backing sector，最后提交到底层 `BlockDevice` 并聚合 completion。

生命周期协作路线：table load 时通过 block registry 查找 backing device 并持有 `BlockDeviceLease`；table 被替换、清理或 mapper 删除后释放对应 lease，避免 backing 生命周期被 DM 绕过。

### 6.3 架构边界

- [kernel/core/src/device/misc/device_mapper.rs](../kernel/core/src/device/misc/device_mapper.rs) 不承载 target 数据面算法；它只做 ABI、buffer、flags 和 ioctl 分发。
- [kernel/core/comps/device-mapper](../kernel/core/comps/device-mapper) 不直接依赖用户态测试脚本；它只表达 DM 内核语义。
- `DmTable` 负责 table 连续性、capacity、deps、metadata、BIO split 和 flush fan-out。
- `DmTarget` 负责 target 内部映射规则；多 backing target 不能被伪装成 single backing target。
- block registry 负责 block device 生命周期和 open count，不应被 DM 绕过。

## 7. 控制面功能路线

### 7.1 DM ioctl 生命周期路线

DM ioctl 层只处理 mapper 生命周期和 table 状态机，不承载 target 数据面算法。核心边界是：`DM_TABLE_LOAD` 写 inactive table，resume 才让 inactive 替换 active。

设计要点：

- `DM_DEV_CREATE` 创建 mapper 对象和 runtime node。
- `DM_TABLE_LOAD` 解析 target params，查找并持有 backing lease。
- `DM_TABLE_LOAD` 成功后只更新 inactive table。
- `DM_DEV_SUSPEND` 带 `DM_SUSPEND_FLAG` 时挂起并等待在途 I/O；不带该 flag 时表示 resume。
- failed table load 不得污染 active/inactive table、`event_nr` 或 readonly 状态。
- `DM_DEV_WAIT` 等待时不能持有全局 control lock。

### 7.2 Linux DM ioctl 兼容子集地图

| 编号 | ioctl | 当前状态 | 说明 |
|---:|---|---|---|
| 0 | `DM_VERSION` | 支持 | 返回兼容版本 `4.48.0`。 |
| 1 | `DM_REMOVE_ALL` | 支持 | best-effort 删除非 busy mapper。 |
| 2 | `DM_LIST_DEVICES` | 支持 | 输出 name、dev、event、uuid flag。 |
| 3 | `DM_DEV_CREATE` | 支持 | 支持 name、uuid、persistent minor、readonly。 |
| 4 | `DM_DEV_REMOVE` | 支持 | busy mapper 返回 `EBUSY`。 |
| 5 | `DM_DEV_RENAME` | 支持 | 支持 name rename 和 `DM_UUID_FLAG` 下 UUID rename。 |
| 6 | `DM_DEV_SUSPEND` | 支持 | `DM_SUSPEND_FLAG` 表示 suspend，否则 resume。 |
| 7 | `DM_DEV_STATUS` | 支持 | 输出 exists、suspended、readonly、active/inactive present、open count、event。 |
| 8 | `DM_DEV_WAIT` | 支持最小语义 | 等待 `event_nr` 变化；等待期间不持有全局 control lock。 |
| 9 | `DM_TABLE_LOAD` | 支持 | 支持 `linear` 和 `striped`。 |
| 10 | `DM_TABLE_CLEAR` | 支持 | 清理 inactive table。 |
| 11 | `DM_TABLE_DEPS` | 支持 | 支持 active/inactive table selector，backing 去重。 |
| 12 | `DM_TABLE_STATUS` | 支持 | 支持 info/table 两种输出格式。 |
| 13 | `DM_LIST_VERSIONS` | 支持 | 当前返回 `linear 1.4.0`、`striped 1.6.0`。 |
| 14 | `DM_TARGET_MSG` | 不支持 | 当前命令解码拒绝。 |
| 15 | `DM_DEV_SET_GEOMETRY` | 不支持 | 当前命令解码拒绝；现有 LVM2 路径不依赖它。 |
| 16 | `DM_DEV_ARM_POLL` | 不支持 | 当前命令解码拒绝。 |
| 17 | `DM_GET_TARGET_VERSION` | 支持 | 支持按 target name 查询 version。 |

### 7.3 flags 策略

| 类型 | 处理方式 | 代表 flag |
|---|---|---|
| 已实现或参与语义 | 正常处理 | `DM_READONLY_FLAG`、`DM_SUSPEND_FLAG`、`DM_STATUS_TABLE_FLAG`、`DM_QUERY_INACTIVE_TABLE_FLAG`、`DM_UUID_FLAG` |
| 兼容忽略 | 显式允许但不改变核心语义 | `DM_SKIP_BDGET_FLAG`、`DM_SKIP_LOCKFS_FLAG`、`DM_NOFLUSH_FLAG`、`DM_SECURE_DATA_FLAG` |
| 显式拒绝 | 返回错误，避免假支持 | `DM_DEFERRED_REMOVE`、`DM_IMA_MEASUREMENT_FLAG`、未知 flag |

设计结论：unknown flags 不能静默吞掉；output-only flags 由内核写回，用户输入中的旧输出位不决定最终状态。

## 8. 数据面功能路线

### 8.1 Read / Write BIO 数据路线

Read 和 Write 共享同一套 range 选择、target 映射、split 和 completion 聚合路径；差异只在 readonly mapper 上，Read 允许继续，Write 需要拒绝。

设计要点：

- readonly mapper 允许 Read / Flush，拒绝 Write。
- 单 part 可以 remap 原 BIO 后直接提交 backing。
- 多 part 通过 split 生成 child BIO，分别 remap 到对应 backing。
- 任一 child remap/enqueue 失败，都应完成对应 child 为 I/O error。
- original BIO 只能完成一次，最终状态由 child completion 聚合得到。

### 8.2 Flush fan-out 与错误聚合路线

Flush 不读取 data range，也不按 logical sector 做 target remap；它只关心 active table 依赖哪些 backing device，并对这些 backing 去重后下发 Flush。

## 9. Target 能力路线

### 9.1 Target 能力演进路线

Target 能力按映射复杂度递进：先用 single linear 验证基础 sector remap，再用 multi-segment linear 验证 table-level split；随后用 striped 验证多 backing、chunk split 和 deps；最后用 mixed table 验证同一 table 内 linear segment 与 striped segment 的组合。

系统验收也按这个顺序分层：raw linear / raw striped 覆盖窄数据面，linear / striped 基础 LVM2 覆盖单 segment grow/shrink，linear / striped cross-segment 覆盖追加 segment 和 shrink 回单段，mixed 覆盖 linear + striped table 组合。

### 9.2 Linear target 映射路线

文件：[kernel/core/comps/device-mapper/src/target/linear.rs](../kernel/core/comps/device-mapper/src/target/linear.rs)

参数格式：

```text
<logical_start> <length> linear <major>:<minor> <backing_start>
```

核心语义：

- 参数严格为 `<major>:<minor> <backing_start>` 两个字段。
- `length` 不能为 0。
- logical range 是 end-exclusive。
- logical range 和 backing range 都不能溢出。
- backing range 不能超过 backing device capacity。
- table status info 模式输出空 params。
- table status table 模式输出 `<major>:<minor> <backing_start>`。

选择理由：linear 是 Linux DM 和 LVM2 最基础 target，能够支撑 grow 后生成的多 segment/table、raw cross-target BIO 和 ext2 I/O 路径。

替代方案：可以先只支持单 target linear，但无法覆盖 LVM2 grow 后生成的多 segment/table 和跨 target BIO split，因此当前选择支持多 target linear table。

### 9.3 Striped target 几何与边界路线

文件：[kernel/core/comps/device-mapper/src/target/striped.rs](../kernel/core/comps/device-mapper/src/target/striped.rs)

参数格式：

```text
<logical_start> <length> striped <stripe_count> <chunk_size> <dev1> <offset1> <dev2> <offset2> ...
```

核心语义：

- `stripe_count` 不能为 0。
- `chunk_size` 不能为 0，单位为 512 字节 sector。
- backing 参数数量必须等于 `stripe_count`。
- 每个 backing id 必须能通过 block registry 找到 lease。
- 构造时校验 backing lease id 与 params 中的 `major:minor` 一致。
- 每个 stripe 的 required sectors 按真实 logical length 计算，允许最后一行不完整。
- backing range 不能溢出，也不能超过对应 backing capacity。
- table status info 模式输出空 params。
- table status table 模式输出 canonical striped params。

示例：2-way striped，chunk size 为 4 sectors，logical range `2..10` 会拆成：

```text
2..4  -> stripe0
4..8  -> stripe1
8..10 -> stripe0
```

选择理由：striped 是 LVM2 常见 target，可验证多 backing、chunk split、deps 去重和 reboot recovery 路径。

替代方案：只支持 raw striped 或只支持固定 2-way striped。当前没有采用，因为参数化 N-way 基础脚本和 N-to-2N cross-segment 能覆盖更多真实 LVM2 输出形态。

### 9.4 Mixed table 跨 target 拆分路线

mixed table 允许同一个 mapper 内相邻 target 使用不同类型，例如前半段 `linear`、后半段 `striped`。这里的“跨 target”只表示同一个 DM device / 同一个 LV 的逻辑地址跨过相邻 target 边界，不表示一个 BIO 跨两个不同 LV。

mixed table 示例：

```text
0 524288 linear 253:64 2048
524288 524288 striped 2 8 253:80 2048 253:96 2048
```

含义：第一段是 PV1 上的 linear segment，第二段是 PV2+PV3 上的 2-way stripe、4 KiB chunk。

选择理由：真实 LVM2 grow 场景可能在同一个 LV 中先有 linear segment，再追加 striped segment。支持 mixed table 能验证同一 mapper 内跨 target 类型的数据面一致性。

权衡：mixed table 增加了 table-level split 与 target-level split 的组合复杂度，需要 ktest 和系统验收共同覆盖。

## 10. 系统集成路线

### 10.1 block registry 与 runtime node

DM 与 Asterinas block registry 集成，用于：

- 注册 mapper block device；
- 维护 block open count；
- 创建和删除 `/dev/dm-N`；
- 创建、rename、删除 `/dev/mapper/<name>`；
- 在 `/proc/devices` 暴露 `device-mapper` major；
- 提供 legacy block ioctl 支撑 LVM2、mkfs、blkid。

设计结论：runtime node 和 manager 索引必须保持一致。runtime node rename 失败时，manager 状态必须回滚。

### 10.2 BlockDeviceLease 生命周期设计

DM table 持有 backing 时使用 `BlockDeviceLease`，不是裸 `Arc<dyn BlockDevice>`。

选择理由：

- backing 正在卸载或移除时，lease 负责生命周期约束；
- filesystem mount、DM backing、raw disk 使用同一套 block device 生命周期模型；
- 避免 DM table 持有已从 registry 生命周期中移除的 backing。

替代方案：直接保存 `Arc<dyn BlockDevice>`。该方案实现更简单，但会绕过 block registry 生命周期约束，因此不采用。

### 10.3 VFS mount 集成

VFS mount source 解析到 block device 时，通过 [kernel/core/src/fs/vfs/fs_apis/registry.rs](../kernel/core/src/fs/vfs/fs_apis/registry.rs) 获取 `BlockDeviceLease`。

- 普通 mount：通过 block registry `lookup_lease` 获取受生命周期保护的 lease。
- rootfs 预解析设备：通过 `BlockDeviceLease::new_untracked` 包装，不参与 registry lease count。

设计结论：mount 与 DM backing 共享同一套块设备生命周期模型，避免同一设备在 filesystem 或 DM 使用期间被注销。

### 10.4 LVM2 系统功能路线

NixOS guest 提供真实用户态工具链，包括 LVM2、dmsetup、e2fsprogs、util-linux、strace 和测试盘 locator。测试盘通过 VirtIO block serial 暴露给 guest，脚本使用 locator 稳定定位测试盘，不硬编码 `/dev/vd*`。

## 11. 测试与验收路线

验证按三层组织：ktest 锁内核语义，dmsetup/NixOS smoke 锁最小用户态路径，LVM2/reboot 系统验收锁真实用户态链路。`linear` 和 `striped` 不强行做脚本完全对称，而是按 target 特性分层：基础脚本验证单 segment 内扩容/缩容，进阶脚本独立验证 cross-segment/table，mixed 脚本验证真实 LVM2 linear + striped 组合。

写测试和 review 结论时需要先区分改动归属：DM 专属修改指 Device Mapper 自己的 ioctl 适配、table 生命周期、target 参数解析、status/deps、linear/striped 映射、BIO split/flush 等能力；Asterinas 内核框架修改指 DM 为跑通真实用户态而依赖或补齐的通用内核能力，例如 block registry/lease、devtmpfs runtime node、procfs、VFS/mount 生命周期、设备号和测试基础设施。前者应优先用 DM ktest 和 DM 系统入口证明，后者还需要说明为什么不是只服务某个 target，以及是否会影响非 DM 路径。

### 11.1 ktest 的目的、分层和执行方法

ktest 是在 QEMU 内核环境里运行的 Rust 测试，用来验证不依赖真实 LVM2/NixOS 脚本也能判断的 DM 内核语义。它回答的是：table 参数是否合法、ioctl 是否按 Linux DM ABI 返回、linear/striped 映射是否正确、BIO split/flush/readonly/suspend/resume 是否符合预期，以及失败的 table-load 是否会污染已有 active/inactive table。

系统脚本证明真实用户态链路能跑通；ktest 证明内核语义本身是对的。做 review 时应先看改动属于哪一层，再选对应 ktest。

| ktest 类别 | 对应代码层 | 主要验证什么 | 什么时候跑 |
|---|---|---|---|
| DM core / target ktest | `kernel/core/comps/device-mapper` | table、linear target、striped target、BIO split/remap、flush、deps 去重 | 修改 table、target、数据面时跑 |
| ioctl 层 ktest | `kernel/core` | `/dev/mapper/control` ioctl ABI、flags、active/inactive、status/deps、remove/rename/wait | 修改 ioctl、控制面、状态机时跑 |

当前 ktest 受根 [Cargo.toml](../Cargo.toml) 的 `default-members` 影响。为了只跑目标 crate 的内核测试，通常需要临时收窄 `default-members`；否则可能跑到无关 crate、耗时过长，或者测试过滤没有命中目标。

标准流程：

1. 临时修改 [Cargo.toml](../Cargo.toml) 的 `default-members`；
2. 运行对应 ktest；
3. 立刻还原 [Cargo.toml](../Cargo.toml)；
4. 执行 `git diff -- Cargo.toml`，预期无输出。

修改 table / target / BIO 数据面时，先临时把根 [Cargo.toml](../Cargo.toml) 收窄为：

```toml
default-members = [
    "kernel/core/comps/device-mapper",
]
```

示例命令：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && timeout -k 10s 180s make ktest CARGO_OSDK_TEST_ARGS="--kcmd-args=loglevel=error --kcmd-args=earlycon --kcmd-args=console=ttyS0 --boot-method=grub-rescue-iso --grub-boot-protocol=multiboot2 aster_device_mapper::<test_path>"'
```

适用改动：

- linear / striped 参数解析；
- table 连续性、容量和 deps 去重；
- linear sector remap；
- striped chunk split/remap；
- mixed table split；
- flush fan-out 和错误传播。

修改 ioctl / 控制面状态机时，先临时把根 [Cargo.toml](../Cargo.toml) 收窄为：

```toml
default-members = [
    "kernel/core",
]
```

示例命令：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && timeout -k 10s 180s make ktest CARGO_OSDK_TEST_ARGS="--kcmd-args=loglevel=error --kcmd-args=earlycon --kcmd-args=console=ttyS0 --boot-method=grub-rescue-iso --grub-boot-protocol=multiboot2 aster_core::device::misc::device_mapper::tests::<test_name>"'
```

适用改动：

- `DM_DEV_CREATE` / `DM_DEV_REMOVE` / `DM_DEV_RENAME`；
- `DM_TABLE_LOAD` / `DM_TABLE_CLEAR` / `DM_TABLE_STATUS` / `DM_TABLE_DEPS`；
- `DM_DEV_SUSPEND` / resume；
- active / inactive table；
- readonly flag、wait event、flags 校验、buffer layout。

每次临时修改 [Cargo.toml](../Cargo.toml) 后，收尾必须还原并确认：

```bash
git diff -- Cargo.toml
```

预期结果是无输出；如果有输出，说明临时收窄 `default-members` 没还原，不能提交。

### 11.2 按 target 特性分层的系统验收矩阵

ktest 不放进本矩阵。它是公共内核语义测试层，内部通过 case 覆盖 linear、striped 和 mixed；本矩阵只描述需要启动 NixOS guest、调用 dmsetup/LVM2/ext2 的系统验收脚本。

| 验收层级 | linear | striped | mixed |
|---|---|---|---|
| dmsetup raw 数据面 | `--linear-data`：raw linear cross-target BIO split/remap | `--striped-data`：raw striped chunk split/remap 和 backing 分布 | 暂不单独设 raw mixed 主入口 |
| 基础 LVM2 单 segment | `--linear-lvm2`：单 PV、单 linear segment、同盘 grow/shrink、ext2 I/O、reboot recovery | `--striped-lvm2`：N PV / N-way、单 striped segment、同一组 PV grow/shrink、ext2 I/O、reboot recovery；默认 2-way，可用 `STRIPED_PV_COUNT=3` 等参数扩展 | 不单独设基础入口 |
| 进阶 LVM2 cross-segment | `--linear-lvm2-cross-segment`：独立 2PV，先建单段 linear，再扩到第二块 PV 形成两段 linear table，reboot 后验证再 shrink 回单段 | `--striped-lvm2-cross-segment`：独立 2N PV，前 N 盘建基础 striped 段，后 N 盘扩容形成第二个 striped 段；默认 2→4 盘，可用 `STRIPED_CS_PV_COUNT=3` 验证 3→6 盘 | mixed 暂不做扩容/缩容专项 |
| mixed 组合 | 参与 linear 段 | 参与 striped 段 | `--mixed-lvm2`：基础 linear 单盘段 + 基础 striped 双盘段，验证同一 LV 内 mixed table 读写和 reboot recovery |
| 阶段验收 | 显式运行 linear 相关入口 | 显式运行 striped 相关入口 | 显式运行 mixed 入口 |

矩阵原则：基础 LVM2 脚本负责“同一段/同一组盘内扩缩容”，进阶 cross-segment 脚本负责“独立从初始段扩出第二段”。PV 数量是 LVM2 生成目标 table 的触发条件；DM review 的核心是多 segment/table、跨 target 边界、status/deps、flush 和 reboot recovery 是否正确。

### 11.3 系统测试入口

组合入口：[myshell/run_dm_system_tests.sh](../myshell/run_dm_system_tests.sh)

| 规范入口 | 覆盖范围 |
|---|---|
| `--quick` | control ABI smoke + raw linear cross-target BIO + raw striped BIO distribution。 |
| `--linear-data` | raw linear cross-target BIO split/remap。 |
| `--striped-data` | raw striped BIO split/remap 和 backing 分布。 |
| `--linear-lvm2` | 单 PV、单 linear segment、同盘 grow/shrink、ext2 I/O、reboot recovery。 |
| `--striped-lvm2` | N PV / N-way、单 striped segment、同组盘 grow/shrink、ext2 I/O、reboot recovery；默认 2-way，可通过 `STRIPED_PV_COUNT` 参数扩展。 |
| `--linear-lvm2-cross-segment` | 独立 linear cross-PV segment、reboot recovery、shrink 回单段。 |
| `--striped-lvm2-cross-segment` | 独立 striped N-to-2N cross-segment、reboot recovery、shrink 回单段；默认 2→4 盘，可通过 `STRIPED_CS_PV_COUNT` 参数扩展。 |
| `--mixed-lvm2` | LVM2 linear + striped mixed table create、跨 segment/table ext2 I/O、reboot recovery；暂不做 mixed 扩容/缩容专项。 |

### 11.4 按改动范围选择验证

| 改动范围 | 必跑验证 |
|---|---|
| Rust 格式或 DM core 小改 | `cargo fmt --check` + 对应窄 ktest |
| ioctl/control/flags/event/wait | ioctl 层 ktest + `--quick` |
| linear BIO split/remap | DM table/target ktest + `--linear-data` |
| striped BIO split/remap | DM table/target ktest + `--striped-data` |
| linear LVM2 基础单段扩缩容/恢复 | ktest + `--linear-lvm2` |
| striped LVM2 基础单段扩缩容/恢复 | ktest + `--striped-lvm2` |
| linear LVM2 cross-segment/recovery/shrink | ktest + `--linear-lvm2-cross-segment` |
| striped LVM2 N-to-2N cross-segment/recovery/shrink | ktest + `--striped-lvm2-cross-segment` |
| LVM2 linear + striped mixed table/recovery | ktest + `--mixed-lvm2` |
| 阶段验收或发版前 DM 系统回归 | 按改动范围显式运行对应入口；quick 包含 control、linear raw 和 striped raw；linear 常用 `--linear-lvm2`、`--linear-lvm2-cross-segment`，striped 常用 `--striped-lvm2`、`--striped-lvm2-cross-segment` |

系统测试属于慢速验收，应只在对应链路变更或阶段验收时运行。QEMU、ktest、NixOS 系统测试需要串行运行，避免镜像和测试盘锁冲突。

## 12. 技术选型与设计决策汇总

| 决策 | 选择 | 替代方案 | 理由 | 权衡 |
|---|---|---|---|---|
| 兼容范围 | Linux DM 核心 ABI 子集 | 完整 Linux DM | 当前 Asterinas 不具备完整 udev/sysfs/queue stacking 生态 | 需要明确拒绝未实现语义 |
| target 表达 | `DmTarget` enum | target registry 泛化 | 当前仅支持 linear/striped，enum 更直接 | 新增 target 时需要同步修改匹配逻辑 |
| table 激活 | load 写 inactive，resume 激活 | load 直接替换 active | 对齐 Linux DM table 生命周期 | 状态机更复杂，但语义正确 |
| backing 生命周期 | `BlockDeviceLease` | 裸 `Arc<dyn BlockDevice>` | 避免 backing 被注销时仍被 DM 使用 | rootfs 预解析设备需要 untracked lease |
| 数据面 split | table-level split + target-level split | 每个 target 自己处理完整 BIO | 能统一处理 mixed table 和 completion 聚合 | table 与 target 边界需要清晰 |
| Flush | backing 去重后异步 fan-out | 同步等待或逐 target 重复 flush | 避免阻塞 DM enqueue 路径，减少重复 flush | completion 聚合逻辑更复杂 |
| flags 策略 | 支持/兼容忽略/拒绝三分法 | 未知位静默忽略 | 避免用户态误以为已支持高级语义 | 需要持续维护 flag 分类 |
| 系统验收 | ktest + NixOS/LVM2 分层 | 只跑 ktest 或只跑系统脚本 | 覆盖内核语义与真实用户态路径 | 验收成本更高，需按需运行 |
| 系统验收入口 | 显式选择对应 suite | 默认运行历史全量套件 | linear/striped 执行方式对齐，避免默认入口隐藏覆盖范围 | 阶段验收需要手动列出要跑的入口 |

## 13. 风险与待确认事项

### 13.1 主要风险

| 风险 | 影响 | 应对措施 |
|---|---|---|
| 误声明完整 Linux DM 兼容 | 用户态依赖未实现语义导致错误预期 | 文档和 ioctl 行为都明确拒绝非目标能力 |
| table 生命周期状态污染 | failed load 或 reload 破坏 active/inactive 语义 | ktest 覆盖 failed load、active/inactive 查询和 resume 替换 |
| BIO split completion 错误 | original BIO 重复完成或漏完成 | split 聚合测试覆盖成功和失败路径 |
| striped range 边界错误 | 数据写入错误 backing 或错误 sector | 覆盖 chunk 边界、partial final row、target end 边界 |
| mixed table 语义被误解为跨 LV | 错误支持不可能的 BIO 范围 | 文档和测试都强调同一 mapper / 同一 LV 内跨 target |
| backing 生命周期绕过 | backing 被注销后仍被 table 使用 | table 持有 `BlockDeviceLease` |
| 系统测试互相干扰 | QEMU 镜像锁或测试盘残留导致假失败 | 系统测试串行运行，异常时检查进程和输出 |

### 13.2 待确认事项

- 是否需要在后续阶段补充更多 mixed table 组合，例如三段以上 target 或更复杂 grow/shrink 形态。
- 是否需要为 `patches/` 重新生成适配当前 `main` 基线的 patch 集。
- 是否有真实用户态路径要求 udev / systemd 自动联动、LVM 自动扫描与自动激活或完整 sysfs DM 层级；若有，应作为独立阶段评估。
- 是否需要进一步抽象 target registry，以支撑更多 target 类型。

## 14. 验收标准

### 14.1 功能验收

- `dmsetup` 可创建、加载、激活、查询、删除 mapper。
- LVM2 可在无 udev 规则依赖的配置下完成 linear 和 striped 相关路径。
- `DM_TABLE_STATUS` 能输出 canonical table params。
- `DM_TABLE_DEPS` 能按 backing 首次出现顺序去重输出依赖。
- readonly mapper 允许 Read/Flush，拒绝 Write。
- failed table load 不改变已有 active/inactive table 和 device 状态。

### 14.2 数据面验收

- linear BIO remap 到正确 backing sector。
- striped BIO 按 chunk 映射到正确 stripe backing。
- 跨 target BIO 能拆分到相邻 target。
- mixed linear + striped BIO 能先按 target 拆分，再按 striped chunk 拆分。
- split child 全部完成后 original BIO 才完成。
- 任一 child 或 flush backing 失败时，原 BIO 返回错误。

### 14.3 系统验收

- `--quick` 覆盖 control ABI smoke、raw linear cross-target BIO 和 raw striped BIO distribution。
- `--linear-data` / `--striped-data` 分别覆盖 raw dmsetup 数据面 I/O。
- `--linear-lvm2` / `--striped-lvm2` 分别覆盖基础 LVM2 单 segment 内 grow/shrink、ext2 I/O 和 reboot recovery。
- `--linear-lvm2-cross-segment` / `--striped-lvm2-cross-segment` 覆盖独立 cross-segment LVM2 grow、reboot recovery 和 shrink 回单段。
- `--mixed-lvm2` 覆盖同一 LV 内基础 linear 段 + 基础 striped 段的 mixed table I/O 和 reboot recovery；暂不做 mixed 扩容/缩容专项。
- 系统测试需要显式选择入口；不保留历史别名或默认全量套件，linear 和 striped 均按基础/进阶入口对齐运行。

## 15. 附录

### 15.1 当前 target 支持矩阵

| Target | 控制面 | 数据面 | 系统验收 | 当前边界 |
|---|---|---|---|---|
| `linear` | table load/status/deps 已支持 | Read/Write/Flush 已支持；跨 target BIO split 已支持 | ktest、`--linear-data`、`--linear-lvm2`、`--linear-lvm2-cross-segment` 已覆盖 | 不支持 discard/write zeroes；queue stacking 只做现有块层能力的保守汇总。 |
| `striped` | table load/status/deps 已支持；version 为 `1.6.0` | Read/Write/Flush 已支持；按 stripe chunk 拆分并 remap 到对应 backing | ktest、`--striped-data`、`--striped-lvm2`、`--striped-lvm2-cross-segment` 已覆盖 | 不支持 discard/write zeroes；不承诺 Linux striped 周边扩展语义。 |
| `linear + striped mixed` | table load/status/deps 已支持 | 跨 target split + striped chunk split 已支持 | ktest、`--mixed-lvm2` 已覆盖 | 只表示同一 mapper table 内跨 target，不表示跨 LV。 |

### 15.2 系统验收脚本地图

| 脚本 | 规范入口 | 作用 |
|---|---|---|
| [myshell/dm_linear/run_control_abi_test.sh](../myshell/dm_linear/run_control_abi_test.sh) | `--quick` | linear control ABI smoke。 |
| [myshell/dm_linear/run_cross_target_bio_regression.sh](../myshell/dm_linear/run_cross_target_bio_regression.sh) | `--linear-data`、`--quick` | raw linear cross-target BIO regression。 |
| [myshell/dm_linear/run_lvm2_linear_reboot_test.sh](../myshell/dm_linear/run_lvm2_linear_reboot_test.sh) | `--linear-lvm2` | 单 PV、单 segment linear，同盘 grow/shrink、ext2 I/O、reboot recovery。 |
| [myshell/dm_linear/run_lvm2_linear_cross_segment_test.sh](../myshell/dm_linear/run_lvm2_linear_cross_segment_test.sh) | `--linear-lvm2-cross-segment` | 独立 2PV linear cross-segment、reboot recovery、shrink 回单段。 |
| [myshell/dm_striped/run_raw_striped_bio_test.sh](../myshell/dm_striped/run_raw_striped_bio_test.sh) | `--striped-data` | raw striped BIO distribution。 |
| [myshell/dm_striped/run_lvm2_striped_reboot_test.sh](../myshell/dm_striped/run_lvm2_striped_reboot_test.sh) | `--striped-lvm2` | N PV / N-way 单 segment striped，同组盘 grow/shrink、ext2 I/O、reboot recovery；默认 2-way，可用 `STRIPED_PV_COUNT` 参数扩展。 |
| [myshell/dm_striped/run_lvm2_striped_cross_segment_test.sh](../myshell/dm_striped/run_lvm2_striped_cross_segment_test.sh) | `--striped-lvm2-cross-segment` | 独立 striped N-to-2N cross-segment、reboot recovery、shrink 回单段；默认 2→4 盘，可用 `STRIPED_CS_PV_COUNT` 参数扩展。 |
| [myshell/dm_mixed/run_lvm2_linear_striped_mixed_reboot_test.sh](../myshell/dm_mixed/run_lvm2_linear_striped_mixed_reboot_test.sh) | `--mixed-lvm2` | LVM2 linear + striped mixed table reboot。 |

### 15.3 质量检查清单

提交或评审 DM 相关改动前，应确认：

- 是否影响 Linux DM ioctl ABI 或 flags 分类；
- 是否影响 active/inactive table 生命周期；
- 是否影响 backing lease 或 block registry 生命周期；
- 是否影响 target status/deps 输出；
- 是否影响 BIO remap/split/completion；
- 是否影响 Flush fan-out；
- 是否需要新增或调整 ktest；
- 是否需要运行对应系统验收入口；
- 是否显式选择了对应系统验收入口，避免默认套件隐藏覆盖范围；
- 是否仍在当前边界内，未把 udev/systemd 自动联动或完整 sysfs DM 层级混入本阶段；
- 是否需要更新 [Device Mapper 项目进度](../log/device-mapper-progress.md) 或阶段日志。
