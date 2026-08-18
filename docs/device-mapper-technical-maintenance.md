# Asterinas Device Mapper 技术与设计文档

本文档面向后续继续开发、审查和维护 Asterinas Device Mapper 的开发者。它不是简单的维护清单，而是说明当前 Asterinas DM 做到了什么程度、为什么这样设计、如何验证、各个文件在整体中的作用，以及下一阶段应该优先补什么。

当前目标不是完整复刻 Linux Device Mapper 生态，而是先实现一个 **kernel-only、linear-only、尽量对齐 Linux DM 核心语义、但避免 udev/sysfs 等成熟用户态框架依赖** 的最小可用版本。

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
- 不做完整 event polling/wait 语义；
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
7. active table / inactive table；
8. table load；
9. table clear；
10. suspend / resume；
11. `event_nr` 状态变化计数；
12. table status；
13. table deps；
14. list devices；
15. list target versions；
16. get target version；
17. linear target 参数解析；
18. 一段或多段连续 linear target；
19. logical sector 到 backing sector 的映射；
20. BIO remap；
21. 跨 linear target 边界的 BIO split；
22. child BIO completion 聚合；
23. flush 按 backing device 去重后异步 fan-out；
24. `/dev/dm-N` 和 `/dev/mapper/<name>` runtime 节点；
25. `/proc/devices` 暴露 block major；
26. LVM2 依赖的 legacy block ioctl；
27. VirtIO block serial 查询，用于稳定定位测试盘；
28. NixOS guest 中 LVM2 创建、扩容、缩容、重启恢复测试脚本。

### 2.2 当前 linear target 语义

当前只支持 Linux DM 的 linear target 子集，table 行为按以下格式理解：

```text
<logical_start> <length> linear <major>:<minor> <backing_start>
```

语义：

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
- 当前拒绝 DM-on-DM backing，避免递归 mapper 语义尚未成熟时引入复杂生命周期问题。

### 2.3 当前 suspend/resume 语义

当前状态机核心语义：

- `load_table()` 只更新 inactive table；
- `resume()` 才把 inactive table 切换成 active table；
- `suspend()` 阻止新 I/O；
- `suspend()` 等待已进入 DM 的 in-flight I/O drain；
- `Suspending` 阶段对 control/status 语义也表现为 suspended；
- fresh device 上没有 active table 时，`suspend()` 不应错误增加 `event_nr`；
- running device reload 后再次 `resume()` 会用 inactive table 替换 active table。

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
- tableless device status；
- zero target table load 拒绝；
- multi-target `dm_target_spec.next` 解析；
- table status 的 Linux-style `next` offset；
- linear info status 输出空参数；
- linear table status 输出 `<major>:<minor> <backing_start>`；
- table deps backing 去重；
- table status/deps buffer-full 语义；
- resume 激活 inactive table；
- running 状态下 reload + resume 替换 active table；
- linear 参数精确解析；
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

### 3.2 NixOS + LVM2 实测

NixOS 实测用于验证真实用户态路径：LVM2、dmsetup、ext2、block registry、devtmpfs、procfs、VirtIO block、QEMU raw image 一起工作。

当前已有脚本入口集中在：

- [test1.md](file:///root/atom/asterinas/docs/device-mapper-lvm1/test1.md)
- [run_cross_target_bio_regression.sh](file:///root/atom/asterinas/myshell/run_cross_target_bio_regression.sh)
- [run_cross_pv_large_write_test.sh](file:///root/atom/asterinas/myshell/run_cross_pv_large_write_test.sh)
- [run_lvm2_resize_test.sh](file:///root/atom/asterinas/myshell/run_lvm2_resize_test.sh)

三层系统验证：

1. raw BIO 跨 target 回归；
2. 跨 PV 大文件写入和重启 hash 校验；
3. LVM2 创建 LV、扩容、缩容、ext2 resize、重启恢复。

这些测试说明 DM 不只是 ktest 能过，也能在 NixOS guest 里承载标准 LVM2 的 linear LV 使用场景。

本轮代码修复后主要重跑的是 ktest；NixOS 脚本属于已有系统级验证路径，后续若修改 ioctl ABI、block registry、devtmpfs 或 LVM2 交互，应重新跑 NixOS 实测。

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
- 计算 mapper capacity；
- 聚合 backing queue limit；
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
- target version 可以声明 `striped`，但 table load 不允许 striped。

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

---

## 9. 后续优先级

### 第一优先级：继续收紧 linear target 与 Linux DM 行为

下一步最需要做：

1. 明确是否需要支持 `<dev> <offset> [sectors]` 之外的兼容格式；
2. 明确 discard/write zeroes 当前是拒绝还是透传；
3. 检查 queue limits 是否需要更接近 Linux stacking 规则。

### 第二优先级：补最小 ioctl/control 语义

重点：

1. remove 时 open_count 行为；
2. rename 的失败回滚；
3. inactive/active table query flags；
4. event_nr 和 wait/event 相关最小语义；
5. DM flags 的兼容处理。

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
9. BIO split 后任何 child remap/enqueue 失败都必须通知 completion。
10. original BIO 只能 complete 一次。
11. `BlockDeviceLease` 不要退回裸 `Arc<dyn BlockDevice>`。
12. 测试时临时缩小 `default-members` 后必须恢复 [Cargo.toml](file:///root/atom/asterinas/Cargo.toml)。
13. raw BIO 回归会覆盖测试盘开头，不能和保留 LVM2 结果的测试混跑。
14. QEMU 测试要串行跑，避免 `test/initramfs/build/ext2.img` write lock 冲突。

---

## 11. 推荐验证顺序

修改 DM core 或 table/linear target 后：

```bash
cargo fmt --all --check
cargo osdk test device_mapper
```

修改 BIO split/remap 后：

```bash
cargo osdk test device_mapper
myshell/run_cross_target_bio_regression.sh
```

修改 ioctl、block registry、devtmpfs、procfs 后：

```bash
cargo osdk test device_mapper
make test
```

并按需跑：

```bash
myshell/run_cross_pv_large_write_test.sh
myshell/run_lvm2_resize_test.sh
```

修改 NixOS 测试盘逻辑后：

```bash
DM_TEST_IMAGES="test.img test2.img" make run_nixos
```

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

下一阶段最重要的是继续围绕 linear target 和最小 ioctl/control 语义补齐 Linux 行为，而不是扩展新 target 或引入 udev/sysfs 等外部框架。
