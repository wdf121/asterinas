# Asterinas Device Mapper 技术与设计文档

本文档面向后续继续开发、审查和维护 Asterinas Device Mapper 的开发者。它不是简单的维护清单，而是说明当前 Asterinas DM 做到了什么程度、为什么这样设计、如何验证、各个文件在整体中的作用，以及下一阶段应该优先补什么。

当前目标不是完整复刻 Linux Device Mapper 生态，而是先实现一个 **kernel-only、linear-only、尽量对齐 Linux DM ioctl/control 核心 ABI、但避免 udev/sysfs 等成熟用户态框架依赖** 的最小可用版本。

---

## 1. 目标与边界

### 1.1 对标对象

对标的是标准 Linux 内核中的 Device Mapper，尤其是这些核心语义：

- `/dev/mapper/control` 控制设备；
- Linux DM ioctl envelope；
- DM device create/remove/rename/status；
- active table / inactive table；
- table load；
- suspend / resume；
- table status / table deps；
- target version 查询；
- linear target 的 sector 映射；
- BIO remap、split 和 completion 聚合；
- flush 向底层 backing device 转发。

### 1.2 当前明确不做的内容

Asterinas 当前设备、udev、sysfs、devtmpfs、块层生态还没有 Linux 那么成熟，所以本阶段避免把 Linux DM 周边生态照搬进来。

当前不做或只做最小兼容：

- 不依赖 udev；
- 不实现完整 sysfs DM 层级；
- 不实现完整 dmsetup 生态；
- 不实现 target registry 泛化框架；
- 不实现 crypt、snapshot、thin、mirror、multipath 等 target；
- 不做复杂 queue stacking；
- 当前不支持 discard / write zeroes BIO 语义；
- 不做完整 uevent、udev 和 poll/select 事件生态；
- `striped` 只用于回应 LVM2 target version 预检，不支持实际 table load。

### 1.3 当前实现策略

当前策略是：

1. 内核里先做好 DM core；
2. 只支持 linear target；
3. 尽量保持 Linux DM 控制面和 table 语义；
4. 通过 Asterinas 当前已有 misc device、block registry、devtmpfs runtime node 能力对接用户态；
5. LVM2 侧使用 `activation { udev_rules=0 }`，显式避开 udev 依赖；
6. 用 ktest 锁住内核语义，用 NixOS + LVM2 实测验证真实系统路径。

---

## 2. 当前做到什么地步

### 2.1 已支持的功能

当前 Asterinas DM 已支持：

1. `/dev/mapper/control` 字符设备；
2. Linux `struct dm_ioctl` 固定 header 解析和响应写回；
3. ioctl 命令分发；
4. DM device create/remove/remove_all/rename/status；
5. name、uuid、dev selector 查询；
6. selector 优先级：UUID 优先于 name，name 优先于 dev；
7. remove 时通过 block registry open count 阻止删除 busy mapper；
8. remove_all best-effort 删除非 busy mapper，跳过 busy mapper；
9. active table / inactive table；
10. table load；
11. table clear；
12. suspend / resume；
13. `event_nr` 状态变化计数和 `DM_DEV_WAIT` 最小等待语义；
14. table status；
15. table deps；
16. list devices；
17. list target versions；
18. get target version；
19. DM ioctl flags 最小兼容校验；
20. 输出类 ioctl 的 `data_start` / `DM_BUFFER_FULL_FLAG` 边界处理；
21. linear target 参数解析；
22. 一段或多段连续 linear target；
23. logical sector 到 backing sector 的映射；
24. BIO remap；
25. 跨 linear target 边界的 BIO split；
26. child BIO completion 聚合；
27. flush 按 backing device 去重后异步 fan-out；
28. `/dev/dm-N` 和 `/dev/mapper/<name>` runtime 节点；
29. `/proc/devices` 暴露 block major；
30. LVM2 依赖的 legacy block ioctl；
31. VirtIO block serial 查询，用于稳定定位测试盘；
32. NixOS guest 中 LVM2 创建、扩容、缩容、重启恢复测试脚本。

### 2.2 当前 linear target 语义

当前只支持 Linux DM 的 linear target 子集，table 行为按以下格式理解：

```text
<logical_start> <length> linear <major>:<minor> <backing_start>
```

语义：

- linear 参数严格为 `<major>:<minor> <backing_start>` 两个字段，不接受额外单位或兼容后缀；
- `DM_TABLE_STATUS` 不带 `DM_STATUS_TABLE_FLAG` 时，对齐 Linux `linear_status(STATUSTYPE_INFO)`，linear target 参数为空；
- `DM_TABLE_STATUS` 带 `DM_STATUS_TABLE_FLAG` 时，对齐 Linux `linear_status(STATUSTYPE_TABLE)`，linear target 参数为 `<major>:<minor> <backing_start>`；
- 所有 sector 均为 512 字节扇区；
- 每条 target 的 `length` 不能为 0；
- 第一条 target 必须从 logical sector 0 开始；
- 多条 target 必须连续排列，不允许空洞；
- logical range 是 end-exclusive；
- backing range 不能整数溢出；
- backing range 不能超过 backing device capacity；
- 同一 backing device 可以被多条 linear target 引用；
- table deps 按 backing device 去重；
- flush 也按 backing device 去重；
- 当前 DM 只处理 Read / Write / Flush；
- mapper capacity 来自所有 linear target 的连续 logical range 总长度；
- queue limits 当前只汇总 `max_nr_segments_per_bio`，取所有 backing device 的最小值；
- 暂不建模 Linux DM 更完整的 queue stacking 能力，例如 alignment、discard、write zeroes、optimal I/O size 等；
- 当前拒绝 DM-on-DM backing，避免递归 mapper 语义尚未成熟时引入复杂生命周期问题。

### 2.3 当前 suspend/resume 语义

当前状态机核心语义：

- `load_table()` 只更新 inactive table；
- `resume()` 才把 inactive table 切换成 active table；
- `suspend()` 阻止新 I/O；
- `suspend()` 等待已进入 DM 的 in-flight I/O drain；
- `Suspending` 阶段对 control/status 语义也表现为 suspended；
- fresh device 上没有 active table 时，`suspend()` 不应错误增加 `event_nr`；
- `load_table()`、`clear_inactive_table()`、`suspend()`、`resume()`、remove 成功路径会在实际状态变化时唤醒 `DM_DEV_WAIT` 等待者；
- `DM_DEV_WAIT` 只等待 `event_nr` 不同于输入 header 中的 `event_nr`，不实现完整 uevent/udev 机制；
- running device reload 后再次 `resume()` 会用 inactive table 替换 active table。

### 2.4 当前 DM ioctl flags 策略

当前 flags 策略是最小兼容子集：已实现语义的 flag 正常处理，无害兼容 flag 显式允许，可能造成语义误导的未实现 flag 显式拒绝，未知位拒绝。

已实现或参与当前语义的输入 flag：

- `DM_READONLY_FLAG`：`DM_DEV_CREATE` 创建只读 mapper；真实 libdevmapper 也可能在 `DM_TABLE_LOAD` 等后续 ioctl 中携带该位，当前在 table load 成功后将设备置为只读，其它查询类命令上兼容忽略；
- `DM_SUSPEND_FLAG`：`DM_DEV_SUSPEND` 中选择 suspend/resume，status 输出中也反映 suspended 状态；
- `DM_PERSISTENT_DEV_FLAG`：`DM_DEV_CREATE` 中指定 persistent minor；其它命令上可能由 libdevmapper 携带为陈旧输出位，当前兼容忽略；
- `DM_STATUS_TABLE_FLAG`：仅允许 `DM_TABLE_STATUS` 输出 table 格式参数；
- `DM_QUERY_INACTIVE_TABLE_FLAG`：仅允许 `DM_DEV_STATUS`、`DM_TABLE_DEPS`、`DM_TABLE_STATUS` 查询 inactive table；
- `DM_UUID_FLAG`：仅允许 `DM_DEV_RENAME`，data 区字符串解释为新 UUID；成功时只更新 UUID 索引和设备 UUID，name/dev/table 状态保持不变。

显式允许并按当前实现视为无害兼容的 flag：

- `DM_SKIP_BDGET_FLAG`：Linux 已忽略，Asterinas 也忽略；
- `DM_SKIP_LOCKFS_FLAG`：当前 suspend 不冻结文件系统，因此忽略；
- `DM_NOFLUSH_FLAG`：当前 suspend/table status/wait 不主动 flush thin 或底层队列，因此忽略；
- `DM_SECURE_DATA_FLAG`：当前只支持 linear target，没有 crypt key 等敏感 target 参数；ioctl 结束后会清零内核临时 buffer。

显式拒绝的 flag：

- `DM_DEFERRED_REMOVE`：当前 remove/open_count 只支持立即删除或 `EBUSY`，返回 `EOPNOTSUPP`；
- `DM_IMA_MEASUREMENT_FLAG`：当前不支持返回 IMA measurement 原始 table 信息，返回 `EOPNOTSUPP`；
- 任何 Linux 6.6 已知范围外的未知 flag 位：返回 `EINVAL`。

输出-only flag 由内核写回，用户输入中的旧输出位不决定最终状态。

### 2.5 标准 Linux DM ioctl 对标矩阵

Linux 6.6 `dm-ioctl.h` 中标准命令编号如下。Asterinas 的扩展原则是：**命令表和 header 行为尽量对齐 Linux；如果功能依赖 Asterinas 当前没有的框架或非 linear target 语义，则做兼容 stub 或明确拒绝，不为了 ioctl 表面完整而引入 udev/sysfs/新 target 框架。**

| 编号 | Linux ioctl | 当前 Asterinas 状态 | 后续策略 |
|---:|---|---|---|
| 0 | `DM_VERSION` | 已支持 | 保持返回 Linux DM ioctl 版本兼容信息。 |
| 1 | `DM_REMOVE_ALL` | 已支持 | 保持 best-effort：busy mapper 跳过，非 busy mapper 删除。 |
| 2 | `DM_LIST_DEVICES` | 已支持 | 保持输出 device list、event number 和 UUID 标记。 |
| 3 | `DM_DEV_CREATE` | 已支持 | 保持 name/uuid/persistent minor/readonly 创建语义。 |
| 4 | `DM_DEV_REMOVE` | 已支持 | 保持 open count gate；暂不支持 deferred remove。 |
| 5 | `DM_DEV_RENAME` | 已支持 name rename 和 UUID rename | UUID rename 只更新 UUID 索引与设备 UUID，不移动 `/dev/mapper/<name>` alias。 |
| 6 | `DM_DEV_SUSPEND` | 已支持 | 保持 suspend/resume 与 in-flight I/O drain；不引入 lockfs/udev。 |
| 7 | `DM_DEV_STATUS` | 已支持 | 保持 header/status flags/open count/table presence/event number。 |
| 8 | `DM_DEV_WAIT` | 已支持最小 event 等待 | 保持不持有全局 control lock；不扩展完整 uevent/poll 生态。 |
| 9 | `DM_TABLE_LOAD` | 已支持 linear-only | 继续只接受 linear target；其它 target table load 返回错误。 |
| 10 | `DM_TABLE_CLEAR` | 已支持 | 保持清理 inactive table 的幂等语义。 |
| 11 | `DM_TABLE_DEPS` | 已支持 | 保持 active/inactive selector 与 backing deps 去重。 |
| 12 | `DM_TABLE_STATUS` | 已支持 | 保持 info/table 两种 linear status 输出。 |
| 13 | `DM_LIST_VERSIONS` | 已支持 | `linear` 真实支持；`striped` 仅用于 LVM2 预检兼容。 |
| 14 | `DM_TARGET_MSG` | 未支持，当前命令解码拒绝 | 待定；linear target 没有真实 message 语义，单补入口价值低。 |
| 15 | `DM_DEV_SET_GEOMETRY` | 未支持，当前命令解码拒绝 | 待定；Asterinas 当前不消费 legacy geometry，no-op 容易形成假支持。 |
| 16 | `DM_DEV_ARM_POLL` | 未支持，当前命令解码拒绝 | 待定；没有完整 poll/uevent 生态前，单补入口不解决实际能力。 |
| 17 | `DM_GET_TARGET_VERSION` | 已支持 | 保持单 target version 查询。 |

因此，当前不为了覆盖 ioctl 编号而硬补入口。`DM_TARGET_MSG`、`DM_DEV_SET_GEOMETRY`、`DM_DEV_ARM_POLL` 先保持待定；只有当真实 dmsetup/LVM2 路径需要，或 Asterinas 后续具备对应框架能力时，再作为独立小阶段评估。

---

## 3. 如何验证

当前验证分两类：**ktest 内核语义验证** 和 **NixOS + LVM2 系统实测**。

### 3.1 ktest 验证

ktest 用于锁住内核内部语义，尤其是普通用户态实测不容易稳定覆盖的边界条件。

本阶段已在容器 `myAsterinas` 内跑过并通过：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && cargo fmt --all --check && timeout 1200 cargo osdk test device_mapper'
```

最近一次复核时，为了避免 `cargo osdk test` 遍历所有 workspace default-members 造成大量 QEMU 并发和 `ext2.img` 锁冲突，ktest 阶段临时把根 [Cargo.toml](file:///root/atom/asterinas/Cargo.toml) 的 `default-members` 缩小为：

```toml
default-members = [
    "kernel",
    "kernel/comps/device-mapper",
]
```

验证完成后必须恢复 [Cargo.toml](file:///root/atom/asterinas/Cargo.toml)。这个改动只是测试提速手段，不属于功能改动。

重点 ktest 覆盖：

- ioctl buffer layout；
- C 字符串解析；
- dm_ioctl header 写回；
- selector 优先级；
- name rename 与 UUID rename 的索引一致性、重复 UUID 拒绝和失败不改状态；
- readonly create/header 输出、table load 携带 readonly flag 时置位，以及只读 mapper 允许 read/flush、拒绝 write；
- tableless device status；
- zero target table load 拒绝；
- multi-target `dm_target_spec.next` 解析；
- table status 的 Linux-style `next` offset；
- linear info status 输出空参数；
- linear table status 输出 `<major>:<minor> <backing_start>`；
- table deps backing 去重；
- table status/deps buffer-full 语义；
- list devices、list target versions、get target version 的短输出 buffer 语义；
- 输出类 helper 对畸形 `data_start` 的自校验；
- device/table status/deps 按 `DM_QUERY_INACTIVE_TABLE_FLAG` 一致选择 active 或 inactive table；
- resume 激活 inactive table；
- running 状态下 reload + resume 替换 active table；
- linear 参数精确解析，严格接受 `<major>:<minor> <backing_start>` 两字段格式；
- linear target range 校验；
- logical range end-exclusive；
- table 从 0 开始且连续；
- mapper capacity 和 queue limit 汇总；
- BIO 越过 table 范围时拒绝；
- 单 target BIO remap；
- 跨 target BIO split；
- flush 每个 backing device 只转发一次；
- flush child completion 失败传播；
- backing enqueue 同步失败时原始 flush BIO 完成；
- suspend 等待 submitted I/O drain 并阻止新 I/O。

### 3.2 NixOS + LVM2 系统实测

系统实测用于验证真实用户态路径：dmsetup、LVM2、ext2、block registry、devtmpfs、procfs、VirtIO block、QEMU raw image 一起工作。

当前系统测试脚本按分层套件维护：

- [run_dm_control_abi_test.sh](file:///root/atom/asterinas/myshell/run_dm_control_abi_test.sh)：轻量 control ABI smoke，覆盖 `/dev/mapper/control`、`dmsetup version/targets/create/table/status/deps/info/wait`、name rename、多 target table/status/deps、多设备 list、readonly mapper 读写拒绝、busy remove、remove_all、`--noflush` suspend/resume；
- [run_cross_target_bio_regression.sh](file:///root/atom/asterinas/myshell/run_cross_target_bio_regression.sh)：raw DM 数据面回归，验证单个 4KiB BIO 跨两个 linear target 后能正确 split/remap/聚合 completion；
- [run_cross_pv_large_write_test.sh](file:///root/atom/asterinas/myshell/run_cross_pv_large_write_test.sh)：LVM2 跨 PV 大文件回归，验证 900MiB LV 跨两块 PV、700MiB 文件写入和重启后 md5 校验；
- [run_lvm2_resize_test.sh](file:///root/atom/asterinas/myshell/run_lvm2_resize_test.sh)：LVM2 扩缩容回归，验证 400MiB 创建、700MiB 跨 PV 扩容、300MiB 缩容、ext2 resize、重启恢复和只读挂载读文件；
- [run_dm_system_tests.sh](file:///root/atom/asterinas/myshell/run_dm_system_tests.sh)：组合入口，支持 `--quick`、`--data`、`--lvm2`、`--full`。

这些脚本共享 [dm_nixos_test.sh](file:///root/atom/asterinas/myshell/lib/dm_nixos_test.sh) 中的 host-side 公共逻辑，包括 QEMU 占用检查、NixOS 镜像检查、测试盘重置、guest shell 等待、FIFO 注入、summary 和失败上下文输出。

推荐按改动范围选择系统测试：

| 改动范围 | 推荐脚本 |
|---|---|
| ioctl/control、flags、event/wait | `myshell/run_dm_system_tests.sh --quick` |
| BIO split/remap、linear target 数据面 | `myshell/run_dm_system_tests.sh --data` |
| LVM2 交互、table/status/deps、resize | `myshell/run_dm_system_tests.sh --lvm2` |
| 阶段验收或发版前验收 | `myshell/run_dm_system_tests.sh --full` |

系统测试和 ktest 的职责不同：ktest 精确覆盖内核内部语义；系统测试确认这些语义能通过真实用户态 ABI 和工具链走通，或至少没有破坏真实 LVM2 linear 路径。

本轮 ioctl/control 收紧后已重新跑过系统级验收：

- `myshell/run_dm_system_tests.sh --quick`：通过，输出 `HOST_PASS_DM_SYSTEM_TESTS --quick`；其中 `run_dm_control_abi_test.sh` 输出 `TEST_PASS_DM_CONTROL_ABI`，覆盖 create/status/deps/info、name rename、多 target table/status/deps、多设备 list、readonly mapper 读成功且写失败、busy remove/remove_all、wait/noflush；`run_cross_target_bio_regression.sh` 输出 `TEST_PASS_CROSS_TARGET_BIO`；
- `myshell/run_dm_system_tests.sh --lvm2`：通过，输出 `HOST_PASS_DM_SYSTEM_TESTS --lvm2`；其中跨 PV 大文件和 LVM2 扩缩容两个子场景均输出对应 `TEST_PASS_*`；
- `myshell/run_cross_pv_large_write_test.sh`：验证 900MiB LV 跨两块 PV，重启后 md5 校验通过；
- `myshell/run_lvm2_resize_test.sh`：验证 400MiB 创建、700MiB 跨 PV 扩容、300MiB 缩容、重启恢复和只读挂载读文件均通过。

系统测试曾暴露 `DM_TABLE_LOAD` 上 libdevmapper 会携带 `DM_PERSISTENT_DEV_FLAG` 陈旧位；当前已修正为 create 时使用该 flag，create 之外兼容忽略该陈旧位。

系统测试也暴露 `dmsetup --readonly create` 会在后续 reload/table load 路径携带 `DM_READONLY_FLAG`；当前 create 可直接创建只读设备，table load 成功后如果看到 readonly flag 也会把设备置为只读，查询类 ioctl 上的旧 readonly 位则不改变状态。

系统实测中 `mkfs.ext2`/`blkid` 会打印 `Unable to get device geometry` 警告，但不影响 linear LV 创建、挂载、读写、扩缩容和恢复。这说明 `DM_DEV_SET_GEOMETRY` 暂不补入口不会阻断当前 LVM2 linear 路径。

后续若继续修改 ioctl ABI、block registry、devtmpfs、BIO split/remap 或 LVM2 交互，应按上表重新跑对应系统测试。

---

## 4. 整体设计

### 4.1 分层结构

当前 DM 设计分为四层：

```text
用户态 LVM2 / dmsetup
        ↓ ioctl
/dev/mapper/control
        ↓
kernel/src/device/misc/device_mapper.rs
        ↓
aster-device-mapper crate
        ↓
DmManager / DmDevice / DmTable / LinearTarget
        ↓
aster-block BlockDevice / BIO
        ↓
VirtIO block / NVMe / raw disk
```

### 4.2 控制面路径

典型 LVM2 创建 linear LV 的控制面路径：

```text
lvcreate --type linear ...
        ↓
DM_DEV_CREATE
        ↓
DmManager::create
        ↓
register_block_mapper
        ↓
/dev/dm-N + /dev/mapper/<name>
        ↓
DM_TABLE_LOAD
        ↓
parse dm_target_spec + linear params
        ↓
lookup_lease(backing major:minor)
        ↓
LinearTarget::new
        ↓
DmTable::new_linear
        ↓
DmDevice::load_table(inactive)
        ↓
DM_DEV_SUSPEND without suspend flag
        ↓
DmDevice::resume
        ↓
inactive table becomes active table
```

关键点：

- table load 不直接改变 active table；
- active 切换由 resume 完成；
- 这与 Linux DM 的 inactive/active table 模型对齐；
- LVM2 可以先 load table，再 resume 激活；
- reload 时可以在 running device 上准备新的 inactive table，再 resume 替换 active table。

### 4.3 数据面路径

普通文件 I/O 的数据面路径：

```text
ext2 file I/O
        ↓
filesystem block I/O
        ↓
/dev/mapper/<vg>-<lv>
        ↓
DmDevice::enqueue
        ↓
active DmTable::enqueue
        ↓
LinearTarget::map_sector
        ↓
SubmittedBio::remap_sid_start
        ↓
backing BlockDevice::enqueue
```

如果 BIO 完整位于一个 target：

```text
logical range
        ↓
找到唯一 LinearTarget
        ↓
计算 backing_start + logical_offset
        ↓
remap 原 BIO
        ↓
提交到底层 backing device
```

如果 BIO 跨 target：

```text
logical range
        ↓
DmTable::bio_parts 拆成多个连续 part
        ↓
SubmittedBio::split 生成 child BIO
        ↓
每个 child 独立 remap
        ↓
分别提交到底层 backing device
        ↓
SplitBioCompletion 聚合所有 child 结果
        ↓
完成原始 BIO
```

### 4.4 Flush 路径

flush 是特殊 BIO，不按普通 sector remap 处理。

当前策略：

```text
Flush BIO 到达 DmTable
        ↓
遍历所有 linear target
        ↓
按 backing DeviceId 去重
        ↓
给每个 backing 异步提交一个 Flush BIO
        ↓
FlushCompletion 等待所有 backing flush 完成
        ↓
任意 backing 失败则原 flush 失败
        ↓
全部成功则原 flush 成功
```

这里刻意避免在 DM enqueue 路径里调用同步 `submit_and_wait()`。原因是 suspend/drain 场景里底层 BIO 可能被测试设备或真实设备延迟完成，同步等待会导致 DM 路径卡死。当前使用异步 fan-out + 聚合完成，更符合 stacked block device 的 I/O 模型。

---

## 5. 主要模块和文件作用

### 5.1 Device Mapper 核心 crate

#### [Cargo.toml](file:///root/atom/asterinas/kernel/comps/device-mapper/Cargo.toml)

定义 `aster-device-mapper` crate。

它把 DM core 做成 kernel component，依赖：

- `aster-block`：BlockDevice、BIO、BlockDeviceLease；
- `device-id`：major/minor/device id；
- `id-alloc`：minor 分配；
- `io-util`：I/O batch；
- `ostd`：同步原语和 no_std 支撑。

#### [lib.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/lib.rs)

DM core 的公开入口。

作用：

- 声明 `device`、`manager`、`table`、`target` 模块；
- 导出 `DmDevice`、`DmDeviceStatus`、`DmManager`、`DmTable`；
- 定义 `DmError` 和 `TableError`；
- 明确 kernel device layer 和 DM core 的边界。

这里不处理 Linux ioctl ABI。ABI 解析放在 [device_mapper.rs](file:///root/atom/asterinas/kernel/src/device/misc/device_mapper.rs)，core crate 只处理已经解析好的内核对象。

#### [manager.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/manager.rs)

DM device 管理层。

作用：

- 分配 device-mapper block major；
- 分配 minor；
- 管理 name → device；
- 管理 uuid → name；
- 支持指定 minor；
- 支持 create、lookup、rename、remove、remove_all；
- remove 通过 block registry 的 open count gate 拒绝 busy mapper；
- remove_all 采用 best-effort 语义，busy mapper 保留，其余 mapper 继续删除；
- 维护 `DmDeviceIdOwner`，保证 minor 生命周期。

它对应 Linux DM 中“控制面对象索引”的一部分，但不绑定 udev。

#### [device.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/device.rs)

运行期 DM block device。

作用：

- 保存 active table；
- 保存 inactive table；
- 管理 Running / Suspending / Suspended；
- 提供 `load_table()` / `clear_inactive_table()` / `suspend()` / `resume()`；
- 实现 `BlockDevice`；
- 在 enqueue 路径维护 in-flight I/O；
- suspend 时阻止新 I/O 并等待旧 I/O 完成。

本阶段修正点：

- 没有 active table 的 fresh device 执行 suspend 时，不再错误递增 `event_nr`；
- `status().suspended` 在 `Suspending` 阶段也为 true，控制面能观察到设备已经进入暂停屏障。

#### [table.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/table.rs)

DM table 和数据面转发层。

作用：

- 保存一组 linear target；
- 验证 table 从 logical sector 0 开始；
- 验证 target 连续无空洞；
- 计算 mapper capacity，结果等于所有连续 linear target 的总 logical sector 数；
- 聚合 backing queue limit，目前只取 `max_nr_segments_per_bio` 的最小值；
- 返回 backing deps；
- 普通 BIO remap；
- 跨 target BIO split；
- flush 去重并异步 fan-out。

本阶段修正点：

- flush 从同步 `submit_and_wait()` 改成异步提交；
- 新增 `FlushCompletion` 聚合多个 backing flush 的完成状态；
- 补充 ktest 锁住 mapper capacity、queue limit、out-of-range BIO 拒绝、flush 失败传播，以及 backing enqueue 失败时原始 flush BIO 的完成语义。

#### [target/mod.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/target/mod.rs)

target 模块入口。

当前只声明 linear target。后续新增 target 时，不能只在这里加模块，还必须同步修改 table 表示、ioctl parser、target version、status/deps 和测试。

#### [linear.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/target/linear.rs)

linear target 实现。

作用：

- 保存 logical range；
- 保存 backing start；
- 保存 backing device id；
- 持有 `BlockDeviceLease`；
- 校验 length、logical overflow、backing overflow、backing capacity；
- 把 logical sector 映射成 backing sector。

它是当前唯一真正支持的数据面 target。

### 5.2 Linux DM ioctl 控制面

#### [device_mapper.rs](file:///root/atom/asterinas/kernel/src/device/misc/device_mapper.rs)

实现 `/dev/mapper/control`。

作用：

- 注册 DM control misc device；
- 实现 `ioctl()`；
- 解析 Linux DM ioctl command；
- 读取和校验 `dm_ioctl` buffer；
- 写回 response header；
- 分发 create/remove/rename/status/table_load/table_status/table_deps/list_versions；
- 解析 `dm_target_spec`；
- 解析 linear 参数；
- 查找 backing `BlockDeviceLease`；
- 调用 DM core。

重要 ABI 点：

- `DM_IOCTL_HEADER_SIZE = 312`；
- `DM_IOCTL_FIXED_PREFIX_SIZE = 305`；
- `data_size` 必须在合理范围；
- `data_start` 必须至少为 header size，并且 8 字节对齐；
- `dm_target_spec.next` 在 table load 输入和 table status 输出中的含义不同；
- table load 当前只接受 `linear`；
- target version 可以声明 `striped`，但 table load 不允许 striped；
- 单个 `DM_DEV_REMOVE` 遇到 open mapper 时返回 `EBUSY`，并保留 manager 索引和 runtime 节点；
- `DM_REMOVE_ALL` 遇到 busy mapper 时跳过该设备，继续删除其它 mapper，ioctl 本身保持 best-effort 成功。

### 5.3 block crate 和 BIO 支撑

#### [lib.rs](file:///root/atom/asterinas/kernel/comps/block/src/lib.rs)

block 抽象入口。

与 DM 相关的作用：

- 定义 `BlockDevice` trait；
- 提供 block device registry；
- 提供 `BlockDeviceLease`；
- 提供 `lookup_lease()`；
- 支撑 DM table 持有 backing device 引用，并避免 backing 正在卸载时被错误使用。

#### [bio.rs](file:///root/atom/asterinas/kernel/comps/block/src/bio.rs)

BIO remap/split/completion 的核心支撑。

与 DM 相关的作用：

- 区分原始 BIO metadata range 和当前层 sid range；
- `remap_sid_start()` 支持 mapper 改写 BIO 当前 sector；
- `SubmittedBio::split()` 支持跨 target BIO 拆分；
- `BioSegment::slice()` 支持 child BIO 共享原 buffer 子区间；
- `SplitBioCompletion` 聚合 child completion；
- `chain_complete_fn()` 支持 stacked block device 在底层完成后释放 in-flight 计数。

DM 数据面能支持跨 PV linear LV，关键依赖这里的 split/remap 能力。

### 5.4 block registry、devtmpfs、procfs

相关文件：

- [block.rs](file:///root/atom/asterinas/kernel/src/device/registry/block.rs)
- [mod.rs](file:///root/atom/asterinas/kernel/src/device/mod.rs)
- [devices.rs](file:///root/atom/asterinas/kernel/src/fs/fs_impls/procfs/devices.rs)

整体作用：

- 运行时注册 `/dev/dm-N`；
- 创建 `/dev/mapper/<name>` alias/symlink；
- rename 时同步 runtime node；
- unregister 时清理 runtime node；
- 维护 block open count；
- 提供 legacy block ioctl；
- 让 `/proc/devices` 暴露 `virtblk`、`nvme`、`device-mapper`。

这些不是 DM core，但是真实 LVM2 能跑起来必须依赖它们。

### 5.5 block driver 适配

相关文件：

- [virtio lib.rs](file:///root/atom/asterinas/kernel/comps/virtio/src/lib.rs)
- [virtio block mod.rs](file:///root/atom/asterinas/kernel/comps/virtio/src/device/block/mod.rs)
- [virtio block device.rs](file:///root/atom/asterinas/kernel/comps/virtio/src/device/block/device.rs)
- [nvme lib.rs](file:///root/atom/asterinas/kernel/comps/nvme/src/lib.rs)
- [nvme block_device.rs](file:///root/atom/asterinas/kernel/comps/nvme/src/device/block_device.rs)

整体作用：

- 给 block major 命名，供 `/proc/devices` 和 LVM2 扫描；
- VirtIO block 暴露 host serial/id，供测试盘稳定定位；
- 适配 `BlockDevice::name() -> String`；
- 让 DM backing 可以是真实块设备。

### 5.6 文件系统和 mount 适配

相关文件：

- [registry.rs](file:///root/atom/asterinas/kernel/src/fs/vfs/fs_apis/registry.rs)
- [ext2/fs.rs](file:///root/atom/asterinas/kernel/src/fs/fs_impls/ext2/fs.rs)
- [ext2/fs_type.rs](file:///root/atom/asterinas/kernel/src/fs/fs_impls/ext2/fs_type.rs)
- [exfat/fs.rs](file:///root/atom/asterinas/kernel/src/fs/fs_impls/exfat/fs.rs)

整体作用：

- mount 后持有 `BlockDeviceLease`；
- 避免 mounted filesystem 只保存裸 block device 引用；
- 支撑 DM device 被 ext2 挂载后仍能正确维持生命周期。

### 5.7 NixOS、QEMU 和测试脚本

相关文件：

- [configuration.nix](file:///root/atom/asterinas/distro/etc_nixos/configuration.nix)
- [default.nix](file:///root/atom/asterinas/distro/etc_nixos/overlays/hello-asterinas/default.nix)
- [run.sh](file:///root/atom/asterinas/tools/nixos/run.sh)
- [br.sh](file:///root/atom/asterinas/myshell/br.sh)
- [test1.md](file:///root/atom/asterinas/docs/device-mapper-lvm1/test1.md)

整体作用：

- NixOS guest 内提供 LVM2、e2fsprogs、dmsetup、strace；
- host 侧附加一块或多块 raw 测试盘；
- 通过 `DM_TEST_IMAGES` 控制测试盘列表；
- 为测试盘设置 VirtIO serial；
- guest 内用 locator 稳定找到测试盘；
- 提供 raw BIO、跨 PV、大文件、resize 等系统验证入口。

---

## 6. 配置说明

### 6.1 根 Cargo workspace

[Cargo.toml](file:///root/atom/asterinas/Cargo.toml) 里与 DM 相关的配置包括：

- workspace members 加入 `kernel/comps/device-mapper`；
- workspace dependencies 加入 `aster-device-mapper`；
- `default-members` 包含 `kernel/comps/device-mapper`。

注意：

- 日常开发不要长期修改 `default-members`；
- 为了定向跑 DM ktest，可以临时缩小 `default-members`；
- 测试结束必须恢复。

### 6.2 kernel crate 依赖

[kernel/Cargo.toml](file:///root/atom/asterinas/kernel/Cargo.toml) 中内核 crate 依赖 `aster-device-mapper`，使 `/dev/mapper/control` 和 block mapper 能调用 DM core。

### 6.3 NixOS 配置

[configuration.nix](file:///root/atom/asterinas/distro/etc_nixos/configuration.nix) 的作用是让 guest 环境具备真实 LVM2 测试能力。

关键包：

- lvm2；
- e2fsprogs；
- util-linux；
- dmsetup；
- strace；
- 测试盘 locator。

### 6.4 测试盘配置

[run.sh](file:///root/atom/asterinas/tools/nixos/run.sh) 支持：

```bash
DM_TEST_IMAGES="test.img test2.img" make run_nixos
```

旧变量兼容：

```bash
DM_TEST_IMAGE=test.img
DM_TEST_IMAGE_2=test2.img
```

测试盘 serial 约定：

```text
vdmtest
vdmtest2
vdmtest3
...
```

### 6.5 清理测试盘

[Makefile](file:///root/atom/asterinas/Makefile) 中的 `make rm_dm` 用于删除 DM 测试盘。

注意：

- 它清理的是测试盘 raw image；
- 不应删除 NixOS root image；
- raw BIO 回归会覆盖测试盘开头，不能和需要保留的 LVM2 结果混跑。

---

## 7. 逐文件变更地图

这一节按整体职责说明文件，不只是列出“改了什么”。

### 7.1 DM core 新增/修改文件

- [kernel/comps/device-mapper/Cargo.toml](file:///root/atom/asterinas/kernel/comps/device-mapper/Cargo.toml)：定义独立 DM core crate。
- [lib.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/lib.rs)：暴露 DM core API 和错误类型。
- [manager.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/manager.rs)：管理 DM 设备索引和 minor 生命周期。
- [device.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/device.rs)：运行期 block device、table 状态机、suspend/resume、in-flight drain。
- [table.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/table.rs)：table 校验、BIO remap/split、flush 异步聚合。
- [target/mod.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/target/mod.rs)：target 模块入口。
- [linear.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/target/linear.rs)：linear target range 校验和 sector 映射。

### 7.2 控制面文件

- [device_mapper.rs](file:///root/atom/asterinas/kernel/src/device/misc/device_mapper.rs)：Linux DM ioctl 控制面，连接用户态 ABI 和 DM core。
- [misc/mod.rs](file:///root/atom/asterinas/kernel/src/device/misc/mod.rs)：初始化 DM control misc device。

### 7.3 block/BIO 支撑文件

- [block lib.rs](file:///root/atom/asterinas/kernel/comps/block/src/lib.rs)：BlockDevice、registry、lease、lookup。
- [bio.rs](file:///root/atom/asterinas/kernel/comps/block/src/bio.rs)：remap、split、completion 聚合、chained completion。
- [device_id.rs](file:///root/atom/asterinas/kernel/comps/block/src/device_id.rs)：major 命名和 `/proc/devices` 支撑。
- [request_queue.rs](file:///root/atom/asterinas/kernel/comps/block/src/request_queue.rs)：使用 remap 后的 BIO sector 构建 request。
- [partition.rs](file:///root/atom/asterinas/kernel/comps/block/src/partition.rs)：适配 block device name/lease 相关改动。

### 7.4 设备注册和 VFS 支撑文件

- [registry/block.rs](file:///root/atom/asterinas/kernel/src/device/registry/block.rs)：mapper block device 注册、open count、legacy block ioctl。
- [device/mod.rs](file:///root/atom/asterinas/kernel/src/device/mod.rs)：runtime devtmpfs node/symlink 创建、删除、rename。
- [dentry.rs](file:///root/atom/asterinas/kernel/src/fs/vfs/path/dentry.rs)：按 inode 匹配删除 runtime node。
- [path/mod.rs](file:///root/atom/asterinas/kernel/src/fs/vfs/path/mod.rs)：提供 runtime devtmpfs 清理需要的路径操作。
- [procfs/devices.rs](file:///root/atom/asterinas/kernel/src/fs/fs_impls/procfs/devices.rs)：实现 `/proc/devices`。
- [procfs/mod.rs](file:///root/atom/asterinas/kernel/src/fs/fs_impls/procfs/mod.rs)：注册 `/proc/devices`。

### 7.5 block driver 和文件系统适配文件

- [virtio/src/lib.rs](file:///root/atom/asterinas/kernel/comps/virtio/src/lib.rs)：VirtIO block major 命名。
- [virtio block mod.rs](file:///root/atom/asterinas/kernel/comps/virtio/src/device/block/mod.rs)：VirtIO block GET_ID 支撑。
- [virtio block device.rs](file:///root/atom/asterinas/kernel/comps/virtio/src/device/block/device.rs)：读取 host id/serial。
- [nvme/src/lib.rs](file:///root/atom/asterinas/kernel/comps/nvme/src/lib.rs)：NVMe major 命名。
- [nvme block_device.rs](file:///root/atom/asterinas/kernel/comps/nvme/src/device/block_device.rs)：适配 block device name。
- [mlsdisk/lib.rs](file:///root/atom/asterinas/kernel/comps/mlsdisk/src/lib.rs)：RawDisk 持有 BlockDeviceLease。
- [mlsdisk.rs](file:///root/atom/asterinas/kernel/comps/mlsdisk/src/layers/5-disk/mlsdisk.rs)：适配 block device name。
- [ext2/fs.rs](file:///root/atom/asterinas/kernel/src/fs/fs_impls/ext2/fs.rs)：ext2 持有 BlockDeviceLease。
- [ext2/fs_type.rs](file:///root/atom/asterinas/kernel/src/fs/fs_impls/ext2/fs_type.rs)：ext2 mount cache key 使用 lease device id。
- [exfat/fs.rs](file:///root/atom/asterinas/kernel/src/fs/fs_impls/exfat/fs.rs)：exfat 持有 BlockDeviceLease。

### 7.6 测试和文档文件

- [device_mapper.c](file:///root/atom/asterinas/test/initramfs/src/regression/device/device_mapper.c)：回归 `/dev/mapper/control` 基础 ABI。
- [device run_test.sh](file:///root/atom/asterinas/test/initramfs/src/regression/device/run_test.sh)：加入 DM control regression。
- [procfs devices.c](file:///root/atom/asterinas/test/initramfs/src/regression/fs/procfs/devices.c)：回归 `/proc/devices`。
- [fs run_test.sh](file:///root/atom/asterinas/test/initramfs/src/regression/fs/run_test.sh)：加入 procfs devices regression。
- [block_device.c](file:///root/atom/asterinas/test/initramfs/src/regression/io/file_io/block_device.c)：回归 legacy block ioctl。
- [test1.md](file:///root/atom/asterinas/docs/device-mapper-lvm1/test1.md)：系统实测脚本总入口。
- [nixos-linear-device-mapper-lvm2-test.md](file:///root/atom/asterinas/docs/device-mapper-lvm1/nixos-linear-device-mapper-lvm2-test.md)：手工排查流程。
- [device-mapper-technical-maintenance.md](file:///root/atom/asterinas/docs/device-mapper-technical-maintenance.md)：本文档，当前实际作为技术与设计文档使用。

---

## 8. 当前阶段刚完成的改动

本阶段围绕第一优先级“收紧 linear target 核心语义”完成了以下工作。

### 8.1 修复 suspend 状态机边界

修改 [device.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/device.rs)：

- fresh device 没有 active table 时，`suspend()` 不增加 `event_nr`；
- `Suspending` 阶段 `status().suspended == true`。

对应修复：

- `enforces_suspend_load_resume_state_machine`；
- `suspend_waits_for_submitted_io_and_blocks_new_io`。

### 8.2 修复 flush 转发模型

修改 [table.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/table.rs)：

- 旧逻辑同步 `submit_and_wait()`；
- 新逻辑异步提交到底层 backing；
- `FlushCompletion` 聚合完成状态；
- 避免 suspend/drain 场景中 DM enqueue 路径被底层 deferred BIO 卡死。

### 8.3 补 linear/table 语义测试

修改 [table.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/table.rs)：

- 新增 mapper capacity 和 queue limit 汇总测试；
- 新增 out-of-range BIO 拒绝测试；
- 新增 flush completion 失败传播测试；
- 新增 backing enqueue 失败时原始 flush BIO 完成测试；
- 保留已有 remap、split、flush 去重、table 连续性测试。

### 8.4 补 ioctl/table 行为测试

修改 [device_mapper.rs](file:///root/atom/asterinas/kernel/src/device/misc/device_mapper.rs)：

- table status 多 target `next` offset；
- table deps backing 去重；
- table status/deps buffer-full；
- resume 激活 inactive table；
- running reload + resume 替换 active table；
- multi-target table load；
- linear 参数精确解析。

### 8.5 复跑格式检查和 DM ktest

在容器 `myAsterinas` 内复跑：

```bash
cargo fmt --all --check
cargo osdk test device_mapper
```

结果通过，退出码为 0。ktest 期间临时缩小过根 [Cargo.toml](file:///root/atom/asterinas/Cargo.toml) 的 `default-members`，结束后已恢复，当前没有 Cargo.toml 残留 diff。

### 8.6 对齐 linear status 输出语义

审查 Linux `dm-linear` 后确认：

- `STATUSTYPE_INFO` 下 linear target status 不输出参数；
- `STATUSTYPE_TABLE` 下输出 backing 设备和起始 sector。

当前 Asterinas 对应为：

- `DM_TABLE_STATUS` 不带 `DM_STATUS_TABLE_FLAG` 时参数为空；
- `DM_TABLE_STATUS` 带 `DM_STATUS_TABLE_FLAG` 时输出 `<major>:<minor> <backing_start>`。

已补 ktest 锁住 info/table 两种输出差异，并复跑 `cargo fmt --all --check` 与 `cargo osdk test device_mapper`，结果通过，退出码为 0。

### 8.7 收紧 linear 参数解析格式

修改 [device_mapper.rs](file:///root/atom/asterinas/kernel/src/device/misc/device_mapper.rs)：

- `DM_TABLE_LOAD` 的 linear 参数严格接受 `<major>:<minor> <backing_start>`；
- 不再接受 `sectors` 等额外后缀；
- 补 ktest 覆盖字段缺失、额外字段、畸形 `major:minor`、数值溢出，以及 table load 失败不改变 device state。

### 8.8 明确 discard / write zeroes 当前边界

当前 Asterinas 块层 `BioType` 只有 Read / Write / Flush，还没有 discard / write zeroes 类型。因此 DM 当前不实现这两类语义，也不在 DM 层伪造透传或拒绝路径。

后续如果块层新增 discard / write zeroes BIO 类型，再按 backing device 能力决定 DM 是透传、拆分后透传，还是返回 not supported。

### 8.9 明确 queue limits 当前边界

当前 DM table 的 capacity 来自 linear table 的总 logical sector 数；queue limits 只汇总 Asterinas 当前块层已有的 `max_nr_segments_per_bio`，并取所有 backing device 的最小值。

这能保证跨 backing 的 BIO 不超过任一底层设备的 segment 限制，但暂不对齐 Linux DM 更完整的 queue stacking 规则。alignment、discard、write zeroes、optimal I/O size 等能力需要等块层抽象补齐后再统一建模。

### 8.10 复核 remove/open_count control 语义

单个 `DM_DEV_REMOVE` 当前先调用 block registry 的 `unregister_mapper()`。该路径会在删除 `/dev/dm-N` 和 `/dev/mapper/<name>` 前检查 open count；如果 mapper 仍被打开，返回 `EBUSY`，并保持 manager 索引和 runtime 节点不变。

`DM_REMOVE_ALL` 当前是 best-effort：busy mapper 删除失败时会被跳过，其它 mapper 继续删除，整个 ioctl 不因为单个 busy 设备失败而失败。

### 8.11 钉住 rename 失败回滚语义

修改 [device_mapper.rs](file:///root/atom/asterinas/kernel/src/device/misc/device_mapper.rs)：

- 将 `device_rename()` 中“先更新 manager，再 rename `/dev/mapper/<name>` alias，alias 失败后回滚 manager”的路径抽成 `rename_device_runtime()`；
- runtime 行为保持不变，但失败回滚路径可以直接用 ktest 注入 alias rename 失败；
- 补 ktest 覆盖 rename 成功、同名 rename no-op、alias rename 失败时 manager/name/uuid/status 回滚。

### 8.12 钉住 active/inactive query flags 语义

修改 [device_mapper.rs](file:///root/atom/asterinas/kernel/src/device/misc/device_mapper.rs)：

- 将 `DM_DEV_STATUS` 的 table 选择路径抽成 `device_status_for_device()`，便于直接测试 active/inactive table selector；
- 补 ktest 覆盖 `DM_DEV_STATUS`、`DM_TABLE_STATUS`、`DM_TABLE_DEPS` 都按 `DM_QUERY_INACTIVE_TABLE_FLAG` 一致选择 table；
- 不带 flag 查询 active table，带 flag 查询 inactive table，active/inactive 同时存在时不能串表。

### 8.13 补齐 DM_DEV_WAIT 最小 event 语义

修改 [device.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/device.rs) 和 [device_mapper.rs](file:///root/atom/asterinas/kernel/src/device/misc/device_mapper.rs)：

- `DmDevice` 增加 event wait queue；
- `load_table()`、`clear_inactive_table()`、`suspend()`、`resume()`、remove 成功路径在 `event_nr` 真实变化后唤醒等待者；
- ioctl 解码接受 Linux ABI 里的 `DM_DEV_WAIT_CMD = 8`；
- `DM_DEV_WAIT` 等待期间不持有全局 `DM_CONTROL_LOCK`，避免阻塞后续能改变 `event_nr` 的 control ioctl；
- 返回前重新确认设备仍在 manager 中，避免 remove 后返回陈旧 device header；
- 补 ktest 覆盖 wait 已变化立即返回、未变化时阻塞到 event 变化、控制面 wait header 更新，以及 ioctl 编码接受 8 号命令。

### 8.14 收口 DM ioctl flags 兼容边界

修改 [device_mapper.rs](file:///root/atom/asterinas/kernel/src/device/misc/device_mapper.rs)：

- 补齐 Linux 6.6 `dm-ioctl.h` 中 0..19 位的 DM flag 常量；
- 新增统一 `validate_input_flags()`，所有 ioctl 命令执行前先检查输入 flags；
- 未知 flag 位返回 `EINVAL`；
- `DM_READONLY_FLAG` 在 `DM_DEV_CREATE` 和成功的 `DM_TABLE_LOAD` 中可把 mapper 标记为只读，后续写 BIO 返回拒绝；
- `DM_DEFERRED_REMOVE`、`DM_IMA_MEASUREMENT_FLAG` 因当前语义未实现而返回 `EOPNOTSUPP`；
- `DM_SKIP_BDGET_FLAG`、`DM_SKIP_LOCKFS_FLAG`、`DM_NOFLUSH_FLAG` 显式允许并按当前实现忽略；
- `DM_SECURE_DATA_FLAG` 显式允许，ioctl 结束后清零内核临时 buffer；
- `DM_PERSISTENT_DEV_FLAG` 在 `DM_DEV_CREATE` 中用于 persistent minor；在其它命令上作为 libdevmapper 可能携带的陈旧位兼容忽略；
- `DM_STATUS_TABLE_FLAG`、`DM_QUERY_INACTIVE_TABLE_FLAG`、`DM_UUID_FLAG` 限制在对应命令上使用；
- 补 ktest 覆盖无害兼容 flag、输出-only 旧 flag、未知位、危险未实现位和命令专用 flag。

### 8.15 支持 linear-only readonly mapper

修改 [device.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/device.rs)、[manager.rs](file:///root/atom/asterinas/kernel/comps/device-mapper/src/manager.rs)、[device_mapper.rs](file:///root/atom/asterinas/kernel/src/device/misc/device_mapper.rs) 和 [run_dm_control_abi_test.sh](file:///root/atom/asterinas/myshell/run_dm_control_abi_test.sh)：

- `DmDeviceStatus` 增加 readonly 状态，control header 对 readonly mapper 写回 `DM_READONLY_FLAG`；
- `DmManager::create_with_readonly()` 支持创建只读设备，原 `create()` 保持默认可写；
- `DM_DEV_CREATE + DM_READONLY_FLAG` 创建 readonly mapper；
- `DM_TABLE_LOAD` 成功且输入携带 `DM_READONLY_FLAG` 时，也把设备置为 readonly，用于兼容真实 `dmsetup --readonly create` 的 reload/table load 序列；
- readonly mapper 的 `Read` 和 `Flush` BIO 继续允许，`Write` BIO 返回 `BioEnqueueError::Refused`；
- 查询类 ioctl 上携带的旧 `DM_READONLY_FLAG` 不改变状态；
- 系统测试新增 `dmsetup --readonly create` 后读成功、写失败的真实路径验证。

真实 quick 测试曾暴露只在 create 路径消费 readonly flag 不够：当 `dmsetup --readonly create` 后续 table load 携带该 flag 但设备未置只读时，写 `/dev/mapper/<name>` 会成功。当前已用 ktest 和 quick 系统测试锁住该行为。

---

## 9. 后续优先级

### 第一优先级：linear target 与 Linux DM 行为当前收口

linear target 当前已明确：

1. table 参数严格接受 `<major>:<minor> <backing_start>`；
2. table status 区分 info/table 两种输出；
3. discard/write zeroes 当前不支持；
4. queue limits 当前只建模 capacity 和 `max_nr_segments_per_bio` 保守汇总。

下一步应转入最小 ioctl/control 语义补齐。

### 第二优先级：系统级回归验收

当前标准 Linux DM ioctl 中，`DM_TARGET_MSG`、`DM_DEV_SET_GEOMETRY`、`DM_DEV_ARM_POLL` 暂不补入口。下一阶段重点转为系统级回归验收，确认已有 ioctl/control 收紧没有破坏真实 LVM2 linear 路径。

### 第三优先级：系统实测回归常态化

重点：

1. 把 NixOS + LVM2 三层测试结果变成稳定可复跑流程；
2. 每次修改 ioctl ABI、block registry、devtmpfs、BIO split/remap 后跑系统实测；
3. 明确哪些测试会破坏测试盘数据；
4. 把 raw BIO、跨 PV、大文件、resize 的通过标准写清楚。

---

## 10. 维护注意事项

1. 不要把 udev 当成当前 DM 的必要依赖。
2. LVM2 命令应显式使用 `activation { udev_rules=0 }`。
3. `striped` 目前只是 target version 预检兼容，不能 table load。
4. `DM_TABLE_LOAD` 的 `next` 和 `DM_TABLE_STATUS` 输出里的 `next` 语义不同。
5. `load_table()` 只加载 inactive table，不应直接替换 active table。
6. `resume()` 才能激活 inactive table。
7. suspend 必须阻止新 I/O，并等待旧 I/O drain。
8. DM enqueue 路径不能同步等待底层 I/O 完成。
9. `DM_DEV_WAIT` 等待期间不能持有全局 control lock，否则会阻塞后续改变 `event_nr` 的 ioctl。
10. DM ioctl 输入 flag 必须先归类为已支持、无害忽略或显式拒绝，不能静默吞掉未知位。
11. 当前块层没有 discard / write zeroes BIO 类型，DM 不应提前伪造这两类语义。
12. 当前 queue limits 只做 capacity 和 `max_nr_segments_per_bio` 保守汇总，不应提前复制 Linux 完整 stacking 规则。
13. BIO split 后任何 child remap/enqueue 失败都必须通知 completion。
14. original BIO 只能 complete 一次。
15. `BlockDeviceLease` 不要退回裸 `Arc<dyn BlockDevice>`。
16. 测试时临时缩小 `default-members` 后必须恢复 [Cargo.toml](file:///root/atom/asterinas/Cargo.toml)。
17. raw BIO 回归会覆盖测试盘开头，不能和保留 LVM2 结果的测试混跑。
18. QEMU 测试要串行跑，避免 `test/initramfs/build/ext2.img` write lock 冲突。

---

## 11. 推荐验证顺序

修改 DM core 或 table/linear target 后：

```bash
cargo fmt --all --check
cargo osdk test device_mapper
```

修改 ioctl/control、flags、event/wait 后：

```bash
cargo osdk test device_mapper
myshell/run_dm_system_tests.sh --quick
```

修改 BIO split/remap 后：

```bash
cargo osdk test device_mapper
myshell/run_dm_system_tests.sh --data
```

修改 LVM2 交互、table/status/deps 或 resize 相关路径后：

```bash
cargo osdk test device_mapper
myshell/run_dm_system_tests.sh --lvm2
```

阶段验收或发版前验收：

```bash
cargo osdk test device_mapper
myshell/run_dm_system_tests.sh --full
```

修改 NixOS 测试盘或 guest 启动逻辑后，需要单独确认对应 `make run_nixos` 路径。

---

## 12. 阶段结论

当前 Asterinas DM 已经不是只有孤立 ktest 的原型，而是具备：

- kernel DM core；
- Linux ioctl 控制面子集；
- linear target 数据面；
- BIO remap/split/completion；
- block registry/devtmpfs/procfs 对接；
- ktest 内核语义验证；
- NixOS + LVM2 系统实测路径。

但它仍然是 **linear-only、无 udev 依赖、最小 Linux DM 兼容子集**。

下一阶段重点应转为维护这套分层验证体系：ktest 负责内部语义，system tests 负责真实 dmsetup/LVM2 路径；`DM_TARGET_MSG`、`DM_DEV_SET_GEOMETRY`、`DM_DEV_ARM_POLL` 暂不为覆盖编号而硬补入口。
