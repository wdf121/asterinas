# Asterinas Device Mapper 技术设计与维护文档

本文档说明 Asterinas Device Mapper 当前设计、实现边界、验证矩阵和后续维护规则。它面向继续开发、审查和排障的维护者，不记录阶段流水账。

## 1. 当前状态

Asterinas Device Mapper 当前是一个最小 Linux DM 兼容子集，核心目标是让 `dmsetup`、LVM2、ext2 和真实块设备路径在没有完整 udev/sysfs DM 生态的前提下可用。

### 1.1 已支持能力

| 领域 | 当前状态 |
|---|---|
| 控制设备 | 支持 `/dev/mapper/control` misc 设备。 |
| Linux DM ioctl header | 支持 `struct dm_ioctl` 固定头解析、版本校验、header 写回、`data_start`/`data_size` 边界检查。 |
| 设备管理 | 支持 create/remove/remove_all/rename/status/list/wait。 |
| table 生命周期 | 支持 active table / inactive table、load、clear、suspend、resume。 |
| target | 支持 `linear` 和 `striped`。 |
| table status/deps | 支持 active/inactive 查询；`DM_STATUS_TABLE_FLAG` 下输出 canonical table params。 |
| 数据面 | 支持 Read/Write/Flush；支持 BIO remap、跨 table target split、striped chunk split、completion 聚合。 |
| flush | 按 backing `DeviceId` 去重，异步 fan-out，聚合完成状态。 |
| 设备节点 | 支持 `/dev/dm-N` 和 `/dev/mapper/<name>` runtime node。 |
| LVM2 适配 | 通过 `activation { udev_rules=0 }` 避开 udev 依赖；linear resize、striped create/grow/shrink/reboot 和 striped 3PV/3-way reboot 路径已系统验收。 |
| 测试盘定位 | VirtIO block serial 暴露给 guest，脚本用 locator 稳定定位测试盘。 |

### 1.2 target 支持矩阵

| Target | 控制面 | 数据面 | 系统验收 | 当前边界 |
|---|---|---|---|---|
| `linear` | table load/status/deps 已支持 | Read/Write/Flush 已支持；跨 target BIO split 已支持 | control smoke、raw cross-target BIO、LVM2 large write、LVM2 resize、single-guest full flow 已覆盖 | 不支持 discard/write zeroes；queue stacking 只做现有块层能力的保守汇总。 |
| `striped` | table load/status/deps 已支持；version 为 `1.6.0` | Read/Write/Flush 已支持；按 stripe chunk 拆分并 remap 到对应 backing | raw BIO 分布验收、LVM2 2-way create/grow/shrink/reboot 验收和 LVM2 3PV/3-way reboot 验收已覆盖 | 不支持 discard/write zeroes；不承诺 Linux striped 周边扩展语义。 |

### 1.3 明确不做的内容

当前不实现或不承诺：

- 完整 udev/uevent/poll 生态；
- 完整 sysfs DM 层级；
- target registry 泛化框架；
- `crypt`、`snapshot`、`thin`、`mirror`、`multipath` 等 target；
- `DM_TARGET_MSG`、`DM_DEV_SET_GEOMETRY`、`DM_DEV_ARM_POLL`；
- discard / write zeroes BIO 语义；
- Linux DM 完整 queue stacking 规则；
- DM-on-DM backing。

## 2. 设计目标与原则

### 2.1 设计目标

1. 对齐 Linux DM 核心 ABI，而不是复刻完整 Linux DM 生态。
2. 保持 active/inactive table、suspend/resume、table status/deps 等核心语义可预测。
3. 数据面优先保证 sector 映射、BIO split/remap、flush fan-out 和 completion 正确。
4. 用户态优先服务 `dmsetup`、LVM2、ext2 的真实系统路径。
5. 所有重要语义都要有 ktest 或 NixOS 系统测试覆盖。

### 2.2 设计取舍

- 不依赖 udev：LVM2 测试命令显式使用 `activation { udev_rules=0 }`。
- 不补空壳 ioctl：没有真实语义或当前框架支撑时，命令保持拒绝，而不是 no-op 假支持。
- 不提前泛化 target 框架：当前 `DmTarget` enum 只承载已实现的 `Linear` 和 `Striped`。
- 不做同步 flush 等待：DM enqueue 路径不能同步等待底层 I/O，否则 suspend/drain 和测试设备延迟完成时容易卡死。
- 不把 raw striped 验收等同于 LVM2 striped 验收：两者覆盖不同风险面，当前都已有独立脚本。

## 3. 总体架构

### 3.1 分层

```text
用户态 dmsetup / LVM2 / ext2
        ↓ ioctl / block I/O
/dev/mapper/control 和 /dev/mapper/<name>
        ↓
kernel/src/device/misc/device_mapper.rs
        ↓
aster-device-mapper crate
        ↓
DmManager / DmDevice / DmTable / DmTarget
        ↓
aster-block BlockDevice / BIO
        ↓
VirtIO block / NVMe / raw disk
```

核心边界：

- `device_mapper.rs` 处理 Linux DM ioctl ABI。
- `aster-device-mapper` 处理内核 DM 对象、table、target 和数据面。
- `aster-block` 提供 `BlockDevice`、`BlockDeviceLease`、BIO remap/split/completion。
- devtmpfs/procfs/block registry 提供真实用户态可见设备节点和 open count。

### 3.2 控制面流程

典型 LVM2 创建 LV 的流程：

```text
DM_DEV_CREATE
        ↓
DmManager 创建 DmDevice，注册 /dev/dm-N 和 /dev/mapper/<name>
        ↓
DM_TABLE_LOAD
        ↓
解析 dm_target_spec 和 target params
        ↓
lookup_lease(backing major:minor)
        ↓
构造 LinearTarget / StripedTarget
        ↓
DmTable::new_targets 校验 table
        ↓
load_table 安装为 inactive table
        ↓
DM_DEV_SUSPEND without DM_SUSPEND_FLAG
        ↓
resume 激活 inactive table 为 active table
```

关键语义：

- `DM_TABLE_LOAD` 只更新 inactive table。
- `resume()` 才替换 active table。
- running device 上 reload + resume 会用新的 inactive table 替换 active table。
- failed table load 不能改变 active/inactive table、`event_nr` 或 readonly 状态。
- `DM_QUERY_INACTIVE_TABLE_FLAG` 只影响 `DM_DEV_STATUS`、`DM_TABLE_STATUS`、`DM_TABLE_DEPS` 的 table 选择。

### 3.3 数据面流程

普通 Read/Write BIO：

```text
SubmittedBio 到达 DmDevice
        ↓
检查设备 running/readonly/table 状态
        ↓
active DmTable::enqueue
        ↓
按 table target 边界拆分 logical range
        ↓
linear: 直接映射到 backing sector
striped: 按 stripe chunk 边界继续拆分
        ↓
单 part：remap 原 BIO 后提交 backing
多 part：SubmittedBio::split 生成 child BIO
        ↓
每个 child remap 后提交对应 backing
        ↓
SplitBioCompletion 聚合 child completion
```

Flush BIO：

```text
Flush BIO 到达 DmTable
        ↓
遍历所有 target 的所有 backing
        ↓
按 DeviceId 去重
        ↓
给每个 backing 异步提交 Flush BIO
        ↓
FlushCompletion 聚合结果
        ↓
任一 backing 失败则原 flush 失败，否则成功
```

## 4. 核心对象设计

### 4.1 DmManager

文件：[kernel/comps/device-mapper/src/manager.rs](../kernel/comps/device-mapper/src/manager.rs)

职责：

- 分配 device-mapper block major 和 minor；
- 管理 name → device、uuid → name 索引；
- 支持指定 persistent minor；
- 实现 create、lookup、rename、remove、remove_all；
- remove 时依赖 block registry open count 拒绝 busy mapper；
- remove_all best-effort 删除非 busy mapper，跳过 busy mapper。

维护要点：

- name rename 和 UUID rename 要保持索引一致。
- runtime node rename 失败时必须回滚 manager 状态。
- busy remove 不能删除 runtime node，也不能移除 manager 索引。

### 4.2 DmDevice

文件：[kernel/comps/device-mapper/src/device.rs](../kernel/comps/device-mapper/src/device.rs)

职责：

- 保存 active table 和 inactive table；
- 管理 Running / Suspending / Suspended 状态；
- 实现 `load_table()`、`clear_inactive_table()`、`suspend()`、`resume()`；
- 作为 mapper block device 实现 `BlockDevice`；
- 维护 in-flight I/O，suspend 时阻止新 I/O 并等待旧 I/O drain；
- 维护 readonly 状态和 `event_nr`；
- 支持 `DM_DEV_WAIT` 的最小 event 等待语义。

维护要点：

- fresh device 没有 active table 时，空 suspend 不应递增 `event_nr`。
- `Suspending` 对 status 表现为 suspended。
- `DM_DEV_WAIT` 等待时不能持有全局 control lock。
- readonly mapper 允许 read/flush，拒绝 write。

### 4.3 DmTable

文件：[kernel/comps/device-mapper/src/table.rs](../kernel/comps/device-mapper/src/table.rs)

职责：

- 保存 `Vec<DmTarget>`；
- 校验 table 非空、从 logical sector 0 开始、target 连续无空洞；
- 拒绝 DM-on-DM backing；
- 计算 mapper capacity；
- 汇总 queue metadata，目前取所有 backing 的 `max_nr_segments_per_bio` 最小值；
- 输出 backing deps，按首次出现顺序去重；
- 执行 Read/Write BIO remap/split；
- 执行 Flush BIO backing 去重和异步 fan-out。

维护要点：

- `bio_parts()` 只按 table target 边界拆分。
- striped chunk 边界由 `StripedTarget::map_range()` 展开。
- split 后任一 child remap/enqueue 失败，都必须完成该 child 为 I/O error。
- original BIO 只能完成一次。

### 4.4 DmTarget

文件：[kernel/comps/device-mapper/src/target/mod.rs](../kernel/comps/device-mapper/src/target/mod.rs)

当前 variants：

```text
DmTarget::Linear(LinearTarget)
DmTarget::Striped(StripedTarget)
```

职责：

- 暴露 target logical range 和 length；
- 暴露多 backing 遍历 helper；
- 保留 linear single-backing helper；
- 不把 striped 伪装成 single-backing target。

维护要点：

- deps、queue limit、flush、DM-on-DM backing 检查都应通过多 backing helper。
- `backing()`、`backing_id()`、`map_sector()` 对 striped 返回 `None` 是刻意设计，避免调用方误用第一个 stripe。

## 5. Target 语义

### 5.1 Linear target

文件：[kernel/comps/device-mapper/src/target/linear.rs](../kernel/comps/device-mapper/src/target/linear.rs)

参数格式：

```text
<logical_start> <length> linear <major>:<minor> <backing_start>
```

语义：

- 参数严格为 `<major>:<minor> <backing_start>` 两个字段。
- `length` 不能为 0。
- logical range 是 end-exclusive。
- logical range 和 backing range 都不能溢出。
- backing range 不能超过 backing device capacity。
- sector 单位为 512 字节。
- table status info 模式输出空 params。
- table status table 模式输出 `<major>:<minor> <backing_start>`。

映射公式：

```text
backing_sector = backing_start + (logical_sector - logical_start)
```

### 5.2 Striped target

文件：[kernel/comps/device-mapper/src/target/striped.rs](../kernel/comps/device-mapper/src/target/striped.rs)

参数格式：

```text
<logical_start> <length> striped <stripe_count> <chunk_size> <dev1> <offset1> <dev2> <offset2> ...
```

其中 target params 部分为：

```text
<stripe_count> <chunk_size> <dev1> <offset1> <dev2> <offset2> ...
```

语义：

- `stripe_count` 不能为 0。
- `chunk_size` 不能为 0，单位为 512 字节 sector。
- backing 参数数量必须等于 `stripe_count`。
- 每个 backing id 必须能通过 block registry 找到 lease。
- 构造时校验 backing lease id 与 params 中的 `major:minor` 一致。
- 每个 stripe 的 required sectors 按真实 logical length 计算，允许最后一行不完整。
- backing range 不能溢出，也不能超过对应 backing capacity。
- table status info 模式输出空 params。
- table status table 模式输出 canonical striped params。

单 sector 映射公式：

```text
offset = logical - logical_start
stripe_width = stripe_count * chunk_size
row = offset / stripe_width
within_row = offset % stripe_width
stripe_index = within_row / chunk_size
chunk_offset = within_row % chunk_size
backing_offset = row * chunk_size + chunk_offset
backing_sector = backing_start[stripe_index] + backing_offset
```

range 映射：

- `map_range()` 要求 range 非空且完全位于 striped target 内。
- 输出按 chunk 边界拆分后的 `StripedRangeMap` 列表。
- 每个 part 携带 logical range、stripe index、backing id、backing range。
- `DmTable::enqueue()` 使用这些 part 生成 child BIO 并提交到对应 backing。

示例：2-way striped，chunk size 为 4 sectors，logical range `2..10` 会拆成：

```text
2..4  -> stripe0
4..8  -> stripe1
8..10 -> stripe0
```

## 6. Linux DM ioctl 支持矩阵

Asterinas 当前对齐 Linux DM ioctl command 编号，但只实现有真实语义支撑的命令。

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

### 6.1 flags 策略

输入 flags 先统一校验：未知位拒绝；危险未实现语义拒绝；已实现或无害兼容位才允许。

已实现或参与语义的 flag：

- `DM_READONLY_FLAG`：create 时创建 readonly mapper；table load 成功后若携带该位也置 readonly。
- `DM_SUSPEND_FLAG`：用于 `DM_DEV_SUSPEND` 区分 suspend/resume。
- `DM_PERSISTENT_DEV_FLAG`：create 时指定 persistent minor；其它命令上作为 libdevmapper 陈旧位兼容忽略。
- `DM_STATUS_TABLE_FLAG`：仅允许 `DM_TABLE_STATUS`。
- `DM_QUERY_INACTIVE_TABLE_FLAG`：仅允许 `DM_DEV_STATUS`、`DM_TABLE_STATUS`、`DM_TABLE_DEPS`。
- `DM_UUID_FLAG`：仅允许 `DM_DEV_RENAME`。

允许并忽略的兼容 flag：

- `DM_SKIP_BDGET_FLAG`
- `DM_SKIP_LOCKFS_FLAG`
- `DM_NOFLUSH_FLAG`
- `DM_SECURE_DATA_FLAG`

显式拒绝：

- `DM_DEFERRED_REMOVE`
- `DM_IMA_MEASUREMENT_FLAG`
- Linux 6.6 已知范围外未知位

输出-only flags 由内核写回，用户输入中的旧输出位不决定最终状态。

## 7. 系统集成设计

### 7.1 block registry 和 runtime node

相关文件：

- [kernel/src/device/registry/block.rs](../kernel/src/device/registry/block.rs)
- [kernel/src/device/mod.rs](../kernel/src/device/mod.rs)
- [kernel/src/fs/fs_impls/procfs/devices.rs](../kernel/src/fs/fs_impls/procfs/devices.rs)

职责：

- 注册 mapper block device；
- 维护 block open count；
- 创建和删除 `/dev/dm-N`；
- 创建、rename、删除 `/dev/mapper/<name>`；
- 在 `/proc/devices` 暴露 `device-mapper` major；
- 提供 legacy block ioctl 支撑 LVM2、mkfs、blkid。

### 7.2 BlockDeviceLease

DM table 持有 backing 时使用 `BlockDeviceLease`，不是裸 `Arc<dyn BlockDevice>`。

原因：

- backing 正在卸载或被移除时，lease 负责生命周期约束；
- filesystem mount、DM backing、raw disk 使用同一套 block device 生命周期模型；
- 后续维护不要把 lease 退回裸引用。

### 7.3 NixOS guest 测试环境

相关文件：

- [distro/etc_nixos/configuration.nix](../distro/etc_nixos/configuration.nix)
- [tools/nixos/run.sh](../tools/nixos/run.sh)
- [myshell/lib/dm_nixos_test.sh](../myshell/lib/dm_nixos_test.sh)

NixOS guest 提供：

- LVM2；
- dmsetup；
- e2fsprogs；
- util-linux；
- strace；
- 测试盘 locator。

测试盘约定：

```text
vdmtest
vdmtest2
vdmtest3
...
```

脚本通过 `aster-dm-disk-locator` 根据 serial 定位测试盘，不硬编码 `/dev/vd*`。

## 8. 验证策略

验证分两层：ktest 锁内核语义，NixOS 系统脚本锁真实用户态路径。

### 8.1 ktest 覆盖范围

核心命令：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && cargo fmt --all --check'
docker exec myAsterinas bash -lc 'cd /root/asterinas && timeout 1200 cargo osdk test device_mapper'
```

重点覆盖：

- ioctl buffer layout、C 字符串、`data_start`、buffer-full；
- command decode 和 flags 校验；
- create/remove/remove_all/rename/status/list/wait；
- name/uuid/dev selector；
- readonly mapper；
- active/inactive table selector；
- load/clear/suspend/resume 状态机；
- wait event 唤醒；
- linear params 解析、status 输出、range 校验；
- striped params 解析、required sectors、range map；
- table 连续性、capacity、queue metadata、deps 去重；
- linear BIO remap；
- 跨 linear target BIO split；
- striped chunk split/remap；
- mixed linear+striped split；
- flush backing 去重和错误传播。

注意：定向跑 DM ktest 时可以临时缩小根 [Cargo.toml](../Cargo.toml) 的 `default-members`，但测试结束必须恢复，并确认 `git diff -- Cargo.toml` 无输出。

### 8.2 系统测试脚本

组合入口：[myshell/run_dm_system_tests.sh](../myshell/run_dm_system_tests.sh)

| 入口 | 覆盖范围 |
|---|---|
| `--quick` | linear control ABI smoke + raw cross-target BIO。 |
| `--data` | raw cross-target BIO regression。 |
| `--striped` | raw `dmsetup striped` BIO split/remap 和 backing 分布。 |
| `--striped-lvm2` | LVM2 striped create、ext2 I/O、grow、shrink、reboot recovery。 |
| `--striped-lvm2-3pv` | LVM2 3PV / 3-way striped create、ext2 I/O、reboot recovery。 |
| `--lvm2` | linear LVM2 cross-PV large write + resize。 |
| `--linear-flow` | 单 guest linear control、raw BIO、LVM2 create/extend/shrink/cleanup 全流程。 |
| `--full` | 当前 linear 全量系统回归；不默认包含 striped。 |

子脚本：

- [myshell/dm_linear/run_control_abi_test.sh](../myshell/dm_linear/run_control_abi_test.sh)
- [myshell/dm_linear/run_cross_target_bio_regression.sh](../myshell/dm_linear/run_cross_target_bio_regression.sh)
- [myshell/dm_linear/run_cross_pv_large_write_test.sh](../myshell/dm_linear/run_cross_pv_large_write_test.sh)
- [myshell/dm_linear/run_lvm2_resize_test.sh](../myshell/dm_linear/run_lvm2_resize_test.sh)
- [myshell/dm_linear/run_linear_full_flow_test.sh](../myshell/dm_linear/run_linear_full_flow_test.sh)
- [myshell/dm_striped/run_raw_striped_bio_test.sh](../myshell/dm_striped/run_raw_striped_bio_test.sh)
- [myshell/dm_striped/run_lvm2_striped_io_reboot_test.sh](../myshell/dm_striped/run_lvm2_striped_io_reboot_test.sh)
- [myshell/dm_striped/run_lvm2_striped_3pv_reboot_test.sh](../myshell/dm_striped/run_lvm2_striped_3pv_reboot_test.sh)

### 8.3 当前已通过的系统验收

raw striped：

```text
CHECK_PASS_STRIPED_RAW_SETUP
CHECK_PASS_STRIPED_RAW_TABLE_STATUS_DEPS
CHECK_PASS_STRIPED_RAW_MAPPER_READBACK_MD5
CHECK_PASS_STRIPED_RAW_BACKING_DISTRIBUTION_MD5
TEST_PASS_DM_STRIPED_RAW_BIO
HOST_PASS_DM_STRIPED_RAW_BIO
HOST_PASS_DM_SYSTEM_TESTS --striped
```

LVM2 striped：

```text
CHECK_PASS_STRIPED_LVM2_SETUP
CHECK_PASS_STRIPED_LVM2_INITIAL_TABLE_STATUS_DEPS
CHECK_PASS_STRIPED_LVM2_BASE_FILE_MD5
CHECK_PASS_STRIPED_LVM2_EXTENDED_TABLE_STATUS_DEPS
CHECK_PASS_STRIPED_LVM2_GROW_FILE_MD5
CHECK_PASS_STRIPED_LVM2_SHRUNK_TABLE_STATUS_DEPS
CHECK_PASS_STRIPED_LVM2_SHRINK_FILE_MD5
TEST_PASS_DM_STRIPED_LVM2_IO_REBOOT_FIRST
CHECK_PASS_STRIPED_LVM2_RECOVERED_TABLE_STATUS_DEPS
CHECK_PASS_STRIPED_LVM2_RECOVERED_FILE_MD5
TEST_PASS_DM_STRIPED_LVM2_IO_REBOOT_SECOND
HOST_PASS_DM_STRIPED_LVM2_IO_REBOOT
HOST_PASS_DM_SYSTEM_TESTS --striped-lvm2
```

LVM2 striped 3PV / 3-way：

```text
CHECK_PASS_STRIPED3_LVM2_SETUP
CHECK_PASS_STRIPED3_LVM2_CREATED_TABLE_STATUS_DEPS
CHECK_PASS_STRIPED3_LVM2_FILE_MD5
TEST_PASS_DM_STRIPED_LVM2_3PV_REBOOT_FIRST
CHECK_PASS_STRIPED3_LVM2_RECOVERED_TABLE_STATUS_DEPS
CHECK_PASS_STRIPED3_LVM2_RECOVERED_FILE_MD5
TEST_PASS_DM_STRIPED_LVM2_3PV_REBOOT_SECOND
HOST_PASS_DM_STRIPED_LVM2_3PV_REBOOT
HOST_PASS_DM_SYSTEM_TESTS --striped-lvm2-3pv
```

LVM2 striped table 示例：

```text
0 524288 striped 2 8 253:64 2048 253:80 2048
0 1048576 striped 2 8 253:64 2048 253:80 2048
0 786432 striped 3 8 253:64 2048 253:80 2048 253:96 2048
```

含义：前两行是 2-way stripe、4 KiB chunk、两个 PV data offset 均为 2048 sectors；第三行是 3-way stripe、4 KiB chunk、三个 PV data offset 均为 2048 sectors。

### 8.4 按改动范围选择验证

| 改动范围 | 必跑验证 |
|---|---|
| Rust 格式或 DM core 小改 | `cargo fmt --all --check` + `cargo osdk test device_mapper` |
| ioctl/control/flags/event/wait | ktest + `myshell/run_dm_system_tests.sh --quick` |
| linear BIO split/remap | ktest + `myshell/run_dm_system_tests.sh --data` |
| striped BIO split/remap | ktest + `myshell/run_dm_system_tests.sh --striped` |
| LVM2 linear create/resize/recovery | ktest + `myshell/run_dm_system_tests.sh --lvm2` |
| LVM2 striped create/grow/shrink/recovery | ktest + `myshell/run_dm_system_tests.sh --striped-lvm2` |
| LVM2 striped stripe_count > 2/recovery | ktest + `myshell/run_dm_system_tests.sh --striped-lvm2-3pv` |
| 阶段验收或发版前 linear 回归 | ktest + `myshell/run_dm_system_tests.sh --full` |

系统测试前如果内核或 NixOS image 相关内容变更，先执行：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && make nixos'
```

QEMU 系统测试必须串行运行，避免测试盘和 `test/initramfs/build/ext2.img` 锁冲突。

## 9. 文件地图

### 9.1 DM core

| 文件 | 职责 |
|---|---|
| [kernel/comps/device-mapper/src/lib.rs](../kernel/comps/device-mapper/src/lib.rs) | crate 入口，导出核心类型和错误。 |
| [kernel/comps/device-mapper/src/manager.rs](../kernel/comps/device-mapper/src/manager.rs) | device 索引、minor、name/uuid、remove 生命周期。 |
| [kernel/comps/device-mapper/src/device.rs](../kernel/comps/device-mapper/src/device.rs) | active/inactive table、suspend/resume、BlockDevice 实现。 |
| [kernel/comps/device-mapper/src/table.rs](../kernel/comps/device-mapper/src/table.rs) | table 校验、metadata/deps、BIO remap/split、flush。 |
| [kernel/comps/device-mapper/src/target/mod.rs](../kernel/comps/device-mapper/src/target/mod.rs) | `DmTarget` enum 和 target 公共 helper。 |
| [kernel/comps/device-mapper/src/target/linear.rs](../kernel/comps/device-mapper/src/target/linear.rs) | linear target 参数和 sector 映射。 |
| [kernel/comps/device-mapper/src/target/striped.rs](../kernel/comps/device-mapper/src/target/striped.rs) | striped 参数、容量校验、sector/range 映射。 |

### 9.2 ioctl 和设备集成

| 文件 | 职责 |
|---|---|
| [kernel/src/device/misc/device_mapper.rs](../kernel/src/device/misc/device_mapper.rs) | `/dev/mapper/control`、Linux DM ioctl ABI、table load/status/deps。 |
| [kernel/src/device/misc/mod.rs](../kernel/src/device/misc/mod.rs) | 初始化 DM control misc device。 |
| [kernel/src/device/registry/block.rs](../kernel/src/device/registry/block.rs) | block registry、mapper 注册、open count、legacy block ioctl。 |
| [kernel/src/device/mod.rs](../kernel/src/device/mod.rs) | runtime `/dev/dm-N` 和 `/dev/mapper/<name>` node 管理。 |
| [kernel/src/fs/fs_impls/procfs/devices.rs](../kernel/src/fs/fs_impls/procfs/devices.rs) | `/proc/devices` 输出 block major。 |

### 9.3 block/BIO 支撑

| 文件 | 职责 |
|---|---|
| [kernel/comps/block/src/lib.rs](../kernel/comps/block/src/lib.rs) | `BlockDevice`、registry、lease、lookup。 |
| [kernel/comps/block/src/bio.rs](../kernel/comps/block/src/bio.rs) | BIO remap、split、segment slice、completion 聚合。 |
| [kernel/comps/block/src/request_queue.rs](../kernel/comps/block/src/request_queue.rs) | 用 remap 后 sector 构建底层 request。 |
| [kernel/comps/block/src/partition.rs](../kernel/comps/block/src/partition.rs) | partition block device 与 lease/name 适配。 |

### 9.4 driver、filesystem、NixOS

| 文件 | 职责 |
|---|---|
| [kernel/comps/virtio/src/device/block/device.rs](../kernel/comps/virtio/src/device/block/device.rs) | VirtIO block serial/id 暴露。 |
| [kernel/comps/nvme/src/device/block_device.rs](../kernel/comps/nvme/src/device/block_device.rs) | NVMe block device name/id 适配。 |
| [kernel/src/fs/fs_impls/ext2/fs.rs](../kernel/src/fs/fs_impls/ext2/fs.rs) | ext2 mount 持有 `BlockDeviceLease`。 |
| [distro/etc_nixos/configuration.nix](../distro/etc_nixos/configuration.nix) | guest 内 LVM2/dmsetup/e2fsprogs 环境。 |
| [tools/nixos/run.sh](../tools/nixos/run.sh) | NixOS guest 启动和测试盘挂载。 |
| [myshell/lib/dm_nixos_test.sh](../myshell/lib/dm_nixos_test.sh) | DM 系统测试公共 harness。 |

## 10. 维护规则

1. 不要让 DM 依赖 udev 才能工作。
2. LVM2 测试命令继续使用 `activation { udev_rules=0 }`。
3. 不要把 `DM_TABLE_LOAD` 做成直接替换 active table。
4. `resume()` 是 inactive table 激活点。
5. failed load 不能污染 device 状态。
6. suspend 必须阻止新 I/O，并等待已进入 DM 的 I/O drain。
7. `DM_DEV_WAIT` 不能持有全局 control lock 等待。
8. ioctl flags 必须显式分类：支持、兼容忽略、拒绝。
9. unknown flags 不能静默吞掉。
10. output-only flags 由内核写回，不受用户输入旧值控制。
11. `DM_TABLE_STATUS` 的 `next` 输出语义不同于 `DM_TABLE_LOAD` 输入语义。
12. `DM_QUERY_INACTIVE_TABLE_FLAG` 的 table 选择要在 status/deps/table_status 中保持一致。
13. `DmTarget` 新增 variant 时，必须同步审视 deps、metadata、flush、DM-on-DM backing 检查。
14. 多 backing target 不能通过 single-backing helper 偷懒接入。
15. BIO split 后每个 child 都必须完成。
16. original BIO 只能完成一次。
17. DM enqueue 路径不能同步等待底层 I/O 完成。
18. 当前块层没有 discard/write zeroes BIO 类型，DM 不应提前伪造语义。
19. queue metadata 当前只保守汇总已有字段，不复制 Linux 完整 queue stacking。
20. `BlockDeviceLease` 不要退回裸 `Arc<dyn BlockDevice>`。
21. raw BIO 脚本会覆盖测试盘开头，不能和需要保留 LVM2 元数据的场景并发或混跑。
22. QEMU/NixOS 系统测试串行运行。
23. 临时修改根 `Cargo.toml default-members` 后必须恢复，并确认 `git diff -- Cargo.toml` 无输出。
24. 不要未经明确需要把 `--full` 扩展为 striped 全量；当前 `--full` 是 linear 全量回归。

## 11. 后续工作

### 11.1 多 segment 组合

striped table 校验和系统验收应继续覆盖更复杂的 LVM2 generated table：

- 多个 striped segment；
- 不同 segment 使用不同 backing offset；
- grow/shrink 后 table 保持连续但不强制单行；
- deps 输出按 backing device 去重且顺序稳定。

这类验收应校验 table 语义：logical start 连续、总长度正确、target type 正确、stripe count/chunk size 正确、deps 完整。不应绑定 LVM2 某一次输出是否单行。

### 11.2 异常路径与边界输入

需要继续补强的边界包括：

- striped target 参数非法：stripe count、chunk size、device/offset 数量不匹配；
- table length 与 backing required sectors 不匹配；
- BIO 覆盖多个 target 且同时跨 striped chunk；
- backing enqueue 失败后的 split BIO completion 聚合；
- flush fan-out 中部分 backing 失败；
- running device reload table 时 active/inactive table 状态保持一致。

### 11.3 系统测试矩阵稳定化

系统测试入口保持分层：

- `--striped` 验证 raw `dmsetup striped` 的 target-specific 数据分布；
- `--striped-lvm2` 验证真实 LVM2/libdevmapper、ext2、resize 和 reboot recovery 路径；
- `--striped-lvm2-3pv` 验证真实 LVM2/libdevmapper、ext2 和 `stripe_count > 2` 的 reboot recovery 路径；
- `--full` 保持 linear 全量语义；
- QEMU/NixOS 系统测试串行运行。

### 11.4 暂不优先的项目

以下内容只有在真实用户态路径需要或底层框架补齐后再做：

- `DM_TARGET_MSG`；
- `DM_DEV_SET_GEOMETRY`；
- `DM_DEV_ARM_POLL`；
- discard/write zeroes；
- 完整 sysfs/uevent/udev；
- target registry 泛化；
- 更多 DM target。

## 12. 设计结论

Asterinas Device Mapper 维护一个可运行、可验收、边界明确的 Linux DM 兼容子集：

- Linux DM ioctl 核心命令和 table 生命周期；
- active/inactive table、suspend/resume、status/deps/table 输出；
- linear target 数据面和 LVM2 resize 系统验收；
- striped target 数据面和 LVM2 create/grow/shrink/reboot 系统验收；
- BIO split/remap/completion 和 flush fan-out；
- block registry、devtmpfs、procfs、VirtIO serial、NixOS guest 测试链路。

维护重点是保持语义闭环：control ABI、target params、table 校验、BIO 映射、系统验收必须同步演进。新 target 或新 BIO 语义应先定义 table/status/deps、数据面映射、flush 行为和验收脚本，再接入用户态入口。
