# Asterinas Device Mapper 技术维护文档

本文档面向没有参与本阶段开发的人，目标是让维护者能快速理解 Asterinas Device Mapper 当前做到哪里、为什么要这么做、代码从哪里入手、测试该怎么跑，以及后续扩展时哪些地方必须同步修改。

本文档基线：`c6c61ed828a58b80e28e0659dc6a598d13c9fb11` 之后到当前工作树。这里的“当前工作树”包括已经提交的改动和当前尚未提交的改动。

---

## 1. 阅读路线

如果你只是想确认功能能不能跑，先看：

```text
docs/device-mapper-lvm1/test1.md
```

如果你要维护或继续开发 DM，推荐按这个顺序读本文：

```text
1. 当前能力和边界
2. 测试体系
3. 一次 LVM2 创建 LV 的完整控制面流程
4. 一次文件读写的完整数据面流程
5. 代码模块职责
6. 逐文件变更地图
7. 常见维护任务和易错点
```

这样先建立整体路径，再进入具体文件，不会在 ioctl、block registry、BIO split、NixOS 测试脚本之间来回跳。

---

## 2. 当前能力和边界

当前实现目标是让标准、未经修改的 LVM2 能在 Asterinas NixOS guest 中使用 linear Device Mapper。

已经支持：

1. `/dev/mapper/control` 字符设备；
2. Linux DM ioctl envelope 的基本解析和写回；
3. DM device create/remove/rename/status；
4. active table / inactive table；
5. suspend/resume 和 table reload；
6. 一段或多段 `linear` target；
7. `/dev/dm-N` 和 `/dev/mapper/<name>` 运行时节点；
8. `/proc/devices` 中暴露 `virtblk`、`nvme`、`device-mapper`；
9. LVM2 依赖的 legacy block ioctl；
10. VirtIO block serial 查询，用于稳定定位测试盘；
11. BIO remap；
12. 单个 BIO 跨多个 linear target 时 split 成多个 child BIO；
13. child BIO completion 聚合成原始 BIO completion；
14. NixOS + LVM2 双 PV、跨 PV、大文件写入、扩缩容和重启恢复测试。

当前边界：

- 实际 table load 只支持 `linear` target；
- `striped` 只用于回应 LVM2 target version 查询，不支持实际数据面；
- 当前 NixOS 测试主线是 x86_64 + VirtIO block；
- 三盘以上测试盘可以通过 `DM_TEST_IMAGES` 启动，但自动化脚本当前仍固定验证前两块盘；
- raw BIO 回归会覆盖测试盘开头，不能和需要保留的 LVM 结果混跑。

---

## 3. 测试体系

测试分三层，从小到大：

```text
第一层：raw BIO 跨 target 边界
第二层：跨 PV 大文件数据面
第三层：完整 LVM2 扩缩容流程
```

日常入口见：

```text
docs/device-mapper-lvm1/test1.md
```

### 3.1 第一层：raw BIO 跨 target 边界

脚本：

```bash
myshell/run_cross_target_bio_regression.sh
```

验证：

```text
一个 4 KiB BIO 覆盖两个 4-sector linear target
→ DmTable::bio_parts 找出两个 part
→ SubmittedBio::split 拆成两个 child BIO
→ child BIO 分别 remap 到两块 backing device
→ mapper 读回和 backing 半段数据通过 md5sum 校验
→ 两个 child 完成后原 BIO completion 聚合完成
```

这是定位 DM 数据面 split/remap 问题最快的测试。

### 3.2 第二层：跨 PV 大文件数据面

脚本：

```bash
myshell/run_cross_pv_large_write_test.sh
```

验证：

```text
两块 512 MiB 测试盘
→ 900 MiB linear LV
→ ext2
→ 写一个 700 MiB 随机文件
→ 保存 file700.md5
→ 首次 md5sum -c
→ 第二台 QEMU 重启恢复后再次 md5sum -c
```

这是验证真实 ext2 + LVM2 + DM 多段 linear table 数据路径的测试。

### 3.3 第三层：完整 LVM2 扩缩容流程

脚本：

```bash
myshell/run_lvm2_resize_test.sh
```

验证：

```text
400 MiB LV
→ ext2 挂载写 hello.txt
→ lvextend 到 700 MiB，强制跨 PV
→ resize2fs 扩大文件系统
→ 写 grow.txt
→ resize2fs 缩小文件系统
→ lvreduce 到 300 MiB
→ 第二台 QEMU 重启恢复后只读挂载读取两个文件
```

这是完整功能验收，最慢，但覆盖面最全。

### 3.4 手工排查文档

只在脚本失败、需要逐步看状态时使用：

```text
docs/device-mapper-lvm1/nixos-linear-device-mapper-lvm2-test.md
```

---

## 4. 一次 LVM2 创建 LV 的控制面流程

用户态命令：

```bash
lvcreate --config 'activation { udev_rules=0 }' --type linear -L 400M -n test_lv test_vg <pv>
```

大致进入内核路径：

```text
LVM2 / dmsetup
        ↓ ioctl
/dev/mapper/control
        ↓
kernel/src/device/misc/device_mapper.rs
        ↓
DmManager 创建 DmDevice
        ↓
block registry 创建 /dev/dm-N
        ↓
devtmpfs 创建 /dev/mapper/<name>
        ↓
DM_TABLE_LOAD 解析 linear target
        ↓
lookup_lease 找 backing PV
        ↓
LinearTarget + DmTable
        ↓
load inactive table
        ↓
DM_DEV_SUSPEND / resume
        ↓
inactive table 切换为 active table
```

这条路径依赖以下模块一起正确工作：

- DM ioctl envelope 解析；
- DM manager name/uuid/minor 管理；
- block mapper 注册；
- devtmpfs runtime node/symlink；
- backing block device lease；
- table load/status/deps；
- suspend/resume 状态机。

---

## 5. 一次文件读写的数据面流程

用户态读写：

```text
ext2 file I/O
        ↓
page cache / filesystem block I/O
        ↓
/dev/mapper/<vg>-<lv>
        ↓
DmDevice::enqueue
        ↓
DmTable::enqueue
        ↓
LinearTarget::map_sector
        ↓
backing BlockDevice::enqueue
        ↓
VirtIO block / raw image
```

如果一个 BIO 完整落在一个 target 内：

```text
BIO logical range
→ target.map_sector(start)
→ bio.remap_sid_start(backing_start)
→ backing.enqueue(bio)
```

如果一个 BIO 跨 target 边界：

```text
BIO logical range
→ DmTable::bio_parts 拆出多个 logical part
→ SubmittedBio::split 生成多个 child BIO
→ 每个 child remap 到对应 backing sector
→ 分别 enqueue
→ SplitBioCompletion 聚合 child completion
→ 原 BIO complete
```

这就是本阶段最后补齐的关键数据面能力。

---

## 6. 核心实现模块

### 6.1 `kernel/src/device/misc/device_mapper.rs`

这是 `/dev/mapper/control` 控制面。

职责：

- 注册 DM control misc 设备；
- 校验 Linux DM ioctl buffer layout；
- 解析 `dm_ioctl`；
- 分发 DM command；
- 创建、删除、重命名、查询 DM device；
- 解析 `DM_TABLE_LOAD` 的 `dm_target_spec`；
- 写回 `DM_TABLE_STATUS`、`DM_TABLE_DEPS`、`DM_LIST_DEVICES`、target version 等结果。

维护重点：

- `DM_IOCTL_HEADER_SIZE = 312` 不能随意改；
- `DM_TABLE_LOAD` 输入里的 `next` 和 `DM_TABLE_STATUS` 输出里的 `next` 语义不同；
- 当前 table load 只接受 `linear`；
- `striped` 只是 target version 兼容项，不是实际支持；
- 多 target status/deps 必须按 Linux ABI 写回。

### 6.2 `kernel/comps/device-mapper/src/manager.rs`

这是 DM device 索引和设备号管理层。

职责：

- 动态申请 `device-mapper` block major；
- 分配 minor；
- 维护 name → device；
- 维护 uuid → name；
- 支持指定 minor；
- 支持 lookup/remove/rename/remove_all。

维护重点：

- `DmDeviceIdOwner` 决定 minor 生命周期；
- remove 只从 manager 索引移除，不等于 block registry 和 devtmpfs 已清理；
- rename 必须同步 manager、device name、mapper symlink。

### 6.3 `kernel/comps/device-mapper/src/device.rs`

这是运行期 DM block device。

职责：

- 保存 active table；
- 保存 inactive table；
- 管理 Running / Suspending / Suspended；
- suspend 时阻止新 I/O 并等待 in-flight I/O drain；
- resume 时把 inactive table 切换成 active table；
- 实现 `BlockDevice`。

维护重点：

- `load_table()` 只加载 inactive table；
- `resume()` 才切换 active table；
- `suspend()` 必须等所有旧 I/O 完成；
- `enqueue()` 必须 chain completion，确保 in-flight 计数能释放。

### 6.4 `kernel/comps/device-mapper/src/table.rs`

这是 DM table 和数据面转发层。

职责：

- 保存一组连续 linear target；
- 验证 table 从 logical sector 0 开始且无空洞；
- 计算 mapper capacity；
- 返回 backing device 依赖；
- remap 并转发普通 BIO；
- 去重转发 flush；
- 跨 target 时 split BIO。

维护重点：

- `DmTable::new_linear()` 要保持 table 连续性约束；
- `DmTable::bio_parts()` 是跨 target BIO 的边界划分逻辑；
- child BIO remap/enqueue 失败时必须调用 `completion.complete_child(IoError)`；
- `enqueue_flush()` 应按 backing device 去重。

### 6.5 `kernel/comps/device-mapper/src/target/linear.rs`

这是 linear target。

职责：

- 保存 logical range；
- 保存 backing start；
- 持有 backing `BlockDeviceLease`；
- 把 logical sector 映射到 backing sector。

维护重点：

- length 不能为 0；
- logical/backing range 不能溢出；
- backing range 不能超过 backing capacity；
- range 是 end-exclusive。

### 6.6 `kernel/comps/block/src/bio.rs`

这是 BIO remap/split 的核心支撑。

职责：

- 区分原始 BIO metadata range 和当前层 sector range；
- `remap_sid_start()` 支持 mapper 改写当前 sector；
- `SubmittedBio::split()` 支持 child BIO；
- `BioSegment::slice()` 让 child BIO 共享原 DMA buffer slice；
- `SplitBioCompletion` 聚合 completion；
- `chain_complete_fn()` 支持 stacked block device 在底层完成后做清理。

维护重点：

- remap 失败不能破坏原 range；
- split ranges 必须连续覆盖原 BIO；
- DMA slice 必须 sector aligned；
- 原 BIO 只能 complete 一次；
- 任意 child 失败时最终原 BIO 应失败。

### 6.7 block registry、devtmpfs、procfs

相关文件：

```text
kernel/src/device/registry/block.rs
kernel/src/device/mod.rs
kernel/src/fs/fs_impls/procfs/devices.rs
```

它们解决的问题：标准 LVM2 不只需要 `/dev/mapper/control`，还需要完整的 Linux 设备生态。

职责：

- 运行时创建 `/dev/dm-N`；
- 运行时创建 `/dev/mapper/<name>`；
- 维护 block open count；
- 支持 mapper register/rename/unregister；
- 支持 legacy block ioctl；
- 输出 `/proc/devices`。

维护重点：

- remove/rename 失败路径必须避免误删节点；
- open_count 会影响 DM remove 行为；
- `/proc/devices` 缺失 `device-mapper` 或 `virtblk` 会影响 LVM2 扫描。

### 6.8 NixOS 和测试盘定位

相关文件：

```text
distro/etc_nixos/configuration.nix
distro/etc_nixos/overlays/hello-asterinas/default.nix
tools/nixos/run.sh
myshell/br.sh
```

职责：

- 在 guest 中提供 LVM2、e2fsprogs、dmsetup、strace；
- 通过 VirtIO serial 稳定定位测试盘；
- host 侧按 `DM_TEST_IMAGES` 附加多块测试盘；
- 提供 `make rm_dm` 清理已有测试盘。

---

## 7. 逐文件变更地图

这一节按职责分组列出从基线提交到当前工作树的增改文件。它不是阅读顺序，而是排查时查文件用的地图。

### 7.1 构建和入口

#### `.gitignore` — 修改

- 将 `target/` 调整为 `/target/`。
- 只忽略仓库根目录 target，避免误忽略其他目录同名文件夹。

#### `Cargo.toml` — 修改

- workspace 成员加入 `kernel/comps/device-mapper`。
- workspace dependency 加入 `aster-device-mapper`。

#### `Cargo.lock` — 修改

- 增加 `aster-device-mapper` package。
- `aster-kernel` 依赖链包含 `aster-device-mapper`。

#### `kernel/Cargo.toml` — 修改

- 内核 crate 新增 `aster-device-mapper.workspace = true`。

#### `Makefile` — 修改

- `ENABLE_KVM` 默认值为 `1`。
- 新增 `make rm_dm`，用于删除当前已有的 DM 测试盘：`test.img`、`test2.img`、`test3.img` 等。
- `DM_TEST_IMAGES=... make rm_dm` 可以删除指定测试盘列表。

#### `MAKE_TARGETS_CHAIN.md` — 新增

- 记录 Makefile / OSDK / NixOS 构建运行链路。
- 属于构建链路说明，不是 DM 核心实现。

#### `rustc-ice-2026-08-12T04_09_40-8081.txt` — 新增

- Rust 编译器 ICE 日志。
- 与 DM 功能无直接关系；是否保留应由维护者决定。

### 7.2 Device Mapper 核心 crate

#### `kernel/comps/device-mapper/Cargo.toml` — 新增

- 定义 `aster-device-mapper` crate。
- 依赖 block、device-id、id-alloc、io-util、ostd。

#### `kernel/comps/device-mapper/src/lib.rs` — 新增

- 声明 `device`、`manager`、`table`、`target` 模块。
- 导出 `DmDevice`、`DmDeviceStatus`、`DmManager`、`DmTable`。
- 定义 `DmError` 和 `TableError`。

#### `kernel/comps/device-mapper/src/manager.rs` — 新增

- 实现 DM manager。
- 管理 major/minor、name、uuid、device id。
- 支持 create、lookup、rename、remove、remove_all。

#### `kernel/comps/device-mapper/src/device.rs` — 新增

- 实现运行期 DM block device。
- 管理 active/inactive table、suspend/resume、event number、in-flight I/O。

#### `kernel/comps/device-mapper/src/table.rs` — 新增

- 实现 `DmTable`。
- 支持多段连续 linear target。
- 实现 BIO remap、flush 去重、跨 target BIO split。

#### `kernel/comps/device-mapper/src/target/mod.rs` — 新增

- 声明 target 模块。

#### `kernel/comps/device-mapper/src/target/linear.rs` — 新增

- 实现 `LinearTarget`。
- 验证 range，执行 logical sector 到 backing sector 的映射。

### 7.3 DM ioctl 控制面

#### `kernel/src/device/misc/device_mapper.rs` — 新增

- 实现 `/dev/mapper/control`。
- 支持 LVM2 所需主要 ioctl。
- 解析多条 linear target。
- 写回 table status、deps、device list、target version。

#### `kernel/src/device/misc/mod.rs` — 修改

- 初始化 DM control misc 设备。

### 7.4 block crate 和 BIO

#### `kernel/comps/block/src/lib.rs` — 修改

- `BlockDevice::name()` 改为返回 `String`。
- 新增 `BlockDeviceLease`。
- 新增 pending registration / unregistration 生命周期。
- 新增 `lookup_lease()`。

#### `kernel/comps/block/src/bio.rs` — 修改

- 支持 current sector range。
- 新增 remap、split、segment slice、completion 聚合和 chained completion。

#### `kernel/comps/block/src/device_id.rs` — 修改

- 支持带名称的 major 分配。
- 新增 `major_devices()`，供 `/proc/devices` 使用。

#### `kernel/comps/block/src/partition.rs` — 修改

- 适配 `BlockDevice::name() -> String`。

#### `kernel/comps/block/src/request_queue.rs` — 修改

- 使用 remap 后的 current `sid_range()` 建 request。

### 7.5 block driver 适配

#### `kernel/comps/virtio/src/lib.rs` — 修改

- VirtIO block major 命名为 `virtblk`。

#### `kernel/comps/virtio/src/device/block/mod.rs` — 修改

- 新增 VirtIO block GET_ID 类型和 ID 长度。

#### `kernel/comps/virtio/src/device/block/device.rs` — 修改

- 初始化时读取 host id。
- 暴露 `host_id()`，用于测试盘定位。
- 适配 `name() -> String`。

#### `kernel/comps/nvme/src/lib.rs` — 修改

- NVMe major 命名为 `nvme`。

#### `kernel/comps/nvme/src/device/block_device.rs` — 修改

- 适配 `BlockDevice::name() -> String`。

#### `kernel/comps/mlsdisk/src/lib.rs` — 修改

- RawDisk 持有 `BlockDeviceLease`。

#### `kernel/comps/mlsdisk/src/layers/5-disk/mlsdisk.rs` — 修改

- 适配 `BlockDevice::name() -> String`。

### 7.6 device registry、VFS、procfs、文件系统

#### `kernel/src/device/mod.rs` — 修改

- 支持运行时 devtmpfs node/symlink 创建、删除、rename。

#### `kernel/src/device/registry/mod.rs` — 修改

- 导出 mapper register/rename/unregister/open_count 接口。

#### `kernel/src/device/registry/block.rs` — 修改

- 支持 mapper block file 生命周期。
- 支持 `/dev/dm-N` 和 mapper alias。
- 支持 legacy block ioctl 和 VirtIO ID ioctl。

#### `kernel/src/fs/fs_impls/procfs/devices.rs` — 新增

- 实现 `/proc/devices`。
- 输出 block major 名称，供 LVM2 扫描。

#### `kernel/src/fs/fs_impls/procfs/mod.rs` — 修改

- 注册 `/proc/devices`。

#### `kernel/src/fs/vfs/fs_apis/registry.rs` — 修改

- mount source 解析后持有 `BlockDeviceLease`。

#### `kernel/src/fs/fs_impls/ext2/fs.rs` — 修改

- ext2 持有 `BlockDeviceLease`。

#### `kernel/src/fs/fs_impls/ext2/fs_type.rs` — 修改

- ext2 mount cache key 通过 lease 的 device id 获取。

#### `kernel/src/fs/fs_impls/ext2/test_utils.rs` — 修改

- 测试工具适配 block lease 和 `name() -> String`。

#### `kernel/src/fs/fs_impls/exfat/fs.rs` — 修改

- exfat 持有 `BlockDeviceLease`。

#### `kernel/src/fs/vfs/path/dentry.rs` — 修改

- 新增只在 inode 匹配时删除目录项的接口。

#### `kernel/src/fs/vfs/path/mod.rs` — 修改

- 新增 `unlink_if_matches()` / `rmdir_if_matches()`，供 runtime devtmpfs 清理使用。

#### `kernel/src/vm/page_cache/tests/utils.rs` — 修改

- 测试 mock 适配 `BlockDevice::name() -> String`。

### 7.7 NixOS、脚本和测试文档

#### `distro/etc_nixos/configuration.nix` — 修改

- guest 包加入 LVM2、e2fsprogs、util-linux、strace、测试盘定位工具。

#### `distro/etc_nixos/overlays/hello-asterinas/default.nix` — 修改

- 新增 `aster-dm-disk-locator`。

#### `tools/nixos/build_nixos.sh` — 修改

- 包含构建调试相关改动；与 DM 功能无直接关系。

#### `tools/nixos/run.sh` — 修改

- 支持 `DM_TEST_IMAGES` 动态多盘列表。
- 兼容旧的 `DM_TEST_IMAGE` / `DM_TEST_IMAGE_2`。
- 自动生成 serial：`vdmtest`、`vdmtest2`、`vdmtest3` 等。
- 创建 512 MiB raw 测试盘并打印附加信息。

#### `tools/qemu_args.sh` — 修改

- 支持 `FORCE_OVMF=on`，稳定 NixOS UEFI 启动。

#### `myshell/br.sh` — 新增

- 手工启动入口。
- 默认附加两块测试盘，也支持 `DM_TEST_IMAGES` 多盘列表。

#### `myshell/run_cross_target_bio_regression.sh` — 新增

- 一键 raw BIO 跨 target 回归，动态检测 guest root shell 后注入测试命令。

#### `myshell/run_cross_pv_large_write_test.sh` — 新增

- 一键跨 PV 大文件数据面测试。

#### `myshell/run_lvm2_resize_test.sh` — 新增

- 一键完整 LVM2 扩缩容测试。

#### `docs/device-mapper-lvm1/test1.md` — 新增

- 测试脚本使用汇总。
- 按三层测试逻辑组织。

#### `docs/device-mapper-lvm1/nixos-linear-device-mapper-lvm2-test.md` — 修改

- 精简为 LVM2 扩缩容手工排查文档。

#### `docs/device-mapper-lvm1/nixos-cross-pv-large-write-test.md` — 删除

- 大文件跨 PV 详细文档已删除。
- 对应内容由 `myshell/run_cross_pv_large_write_test.sh` 和 `test1.md` 承接。

#### `docs/device-mapper-technical-maintenance.md` — 新增

- 即本文档。

### 7.8 initramfs 回归

#### `test/initramfs/src/regression/device/device_mapper.c` — 新增

- 回归 `/dev/mapper/control` 基础 ABI。
- 验证 `dm_ioctl` 大小和 tableless device status。

#### `test/initramfs/src/regression/device/run_test.sh` — 修改

- 加入 `./device_mapper`。

#### `test/initramfs/src/regression/fs/procfs/devices.c` — 新增

- 回归 `/proc/devices`。
- 检查 `virtblk` 和 `device-mapper`。

#### `test/initramfs/src/regression/fs/run_test.sh` — 修改

- 加入 `./procfs/devices`。

#### `test/initramfs/src/regression/io/file_io/block_device.c` — 修改

- 回归 `BLKGETSIZE`、`BLKGETSIZE64`、`BLKRAGET`。

---

## 8. 典型维护任务

### 8.1 新增 target 类型

需要同步修改：

1. `kernel/comps/device-mapper/src/target/`；
2. `kernel/comps/device-mapper/src/table.rs` table 表示和 enqueue；
3. `kernel/src/device/misc/device_mapper.rs` table load params 解析；
4. `DM_TABLE_STATUS` 输出；
5. `DM_LIST_VERSIONS` / `DM_GET_TARGET_VERSION`；
6. ktest 和 NixOS guest 测试。

不要只在 target version 中声明支持，除非明确只是兼容用户态预检。

### 8.2 修改 table reload / suspend / resume

重点检查：

- inactive table 是否只在 resume 时切换；
- suspend 是否阻止新 I/O；
- in-flight I/O 是否 drain；
- event number 是否合理更新；
- active LV 挂载状态下 `lvextend` 是否仍可用。

### 8.3 修改 BIO split/remap

重点检查：

- child ranges 是否完整覆盖原 BIO；
- ranges 是否连续无空洞；
- segment slice 是否 sector aligned；
- child enqueue 失败是否通知 completion；
- 原 BIO 是否只 complete 一次；
- request queue 是否看到 remap 后 sector。

### 8.4 修改 remove/rename

重点检查：

- manager name/uuid 索引；
- block registry accepting opens；
- open_count；
- `/dev/dm-N`；
- `/dev/mapper/<name>` symlink；
- 失败路径回滚。

### 8.5 修改测试盘逻辑

重点检查：

- `DM_TEST_IMAGES` 顺序和 serial 是否一致；
- `aster-dm-disk-locator` 是否能找到对应 serial；
- 测试盘不能指向 NixOS root image；
- 不允许重复附加同一个 image；
- `make rm_dm` 是否不会误删 root image；
- raw BIO 回归是否提醒会覆盖测试盘开头。

---

## 9. 推荐验证顺序

代码修改后建议先跑静态/构建：

```bash
cargo fmt --all --check
make kernel
```

再按改动范围选择：

```bash
make ktest CARGO_OSDK_TEST_ARGS="splits_bio_across_linear_target_boundary"
```

```bash
myshell/run_cross_target_bio_regression.sh
```

```bash
myshell/run_cross_pv_large_write_test.sh
```

```bash
myshell/run_lvm2_resize_test.sh
```

如果改动涉及 ioctl ABI、block registry、procfs 或 legacy block ioctl，还应跑 initramfs regression。

---

## 10. 已知易错点

1. `/sys/class/block/<dev>/dev` 当前不可靠，测试脚本用 `stat -c '%t'/'%T' /dev/vdX` 获取 major/minor。
2. LVM2 在无 udev 环境下需要命令级配置：`activation { udev_rules=0 }`。
3. `DM_TABLE_STATUS` 的 `next` 偏移和 `DM_TABLE_LOAD` 输入中的 `next` 语义不同。
4. 大文件测试不要只看 `dd` 成功，还要保存 hash 并重启后校验。
5. raw BIO 回归会写测试盘开头，不能和需要保留的 LVM 结果混跑。
6. BIO split 后任何 child remap/enqueue 失败都必须通知 completion handle，否则原始 BIO 会永久等待。
7. DM table 依赖 backing `BlockDeviceLease`，不要退回裸 `Arc<dyn BlockDevice>`。
8. `/proc/devices` 缺少正确 major 名称会导致 LVM2 不扫描对应设备。
9. `striped` 当前不是实际数据面支持，只是 target version 兼容项。
10. `myshell/br.sh` 不构建根镜像，只启动 NixOS；根镜像缺失时先跑 `make nixos`。
11. `make rm_dm` 删除的是测试盘，不删除 NixOS root image。

---

## 11. 提交时间线

从基线提交之后，相关提交大致如下：

```text
f16adb151 修改 Makefile ENABLE_KVM = 1
c3c60c31b PV/VG 成功，但 lvcreate 阶段仍有问题
802115f87 最小 device-mapper-linear 验证成功
79ab2c4b2 LV 可以跨 PV，但 BIO 还不能跨 target 边界
e0fec499d 跨多块盘 PV 测试成功
```

建议按这个顺序理解历史：

1. 先补 block registry、DM 控制面、linear target；
2. 让 LVM2 能创建 PV/VG 并开始创建 LV；
3. 让最小 DM linear 路径跑通；
4. 让 LVM 跨 PV table load/status/deps 跑通；
5. 补跨 target BIO split；
6. 把 NixOS 测试逐步脚本化。

---

## 12. 后续更新要求

后续每次补 DM 功能时，应同步更新本文档：

1. 新增或修改的 ioctl；
2. 新增 target 类型；
3. table status/deps/list version 行为变化；
4. BIO 数据面行为变化；
5. block registry 生命周期变化；
6. NixOS 测试步骤或通过标准；
7. 已知限制和未覆盖场景；
8. 每个新增/修改文件的维护说明。
