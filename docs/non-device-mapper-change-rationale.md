# Device Mapper 项目文件改动说明

## 执行摘要

本文从 review 和维护视角整理 `dm` 分支为了支持当前 Device Mapper 能力所涉及的主要改动。

组织方式改为“改动文件 → 对应功能/语义变化”，而不是“功能 → 涉及文件”。原因是很多文件同时承载多项功能：例如 `kernel/core/comps/block/src/bio.rs` 同时支撑 BIO remap、split、range I/O 和 completion 聚合；如果按功能组织，同一个文件会反复出现，不利于 review。

本文中的文件路径均为相对于仓库根目录的相对路径。

范围仍分三类：

1. Device Mapper core crate 内部改动；
2. Asterinas 通用内核框架改动；
3. 配置、构建、启动和回归支撑改动。

本文不展开系统测试脚本、patch 目录、每日日志和测试日志。通用测试分层和 selector 说明见 `docs/test.md`；当前 canonical DM suite、超时、release 与串行边界以 `AGENTS.md`、`log/device-mapper-progress.md` 和实际脚本为准。

## 1. Device Mapper core crate 内部改动

本节覆盖 `kernel/core/comps/device-mapper/` 内的 DM core crate。它负责 mapper 生命周期、table/target 语义和 DM 数据面映射。

| 文件 | 所在层 | 主要承载功能 | 为什么必须改 |
|---|---|---|---|
| `kernel/core/comps/device-mapper/Cargo.toml` | DM crate 构建 | 声明 `aster-device-mapper` crate 及其依赖。 | DM core 需要从 `aster-core` 中独立出来，供 control ioctl 层以 crate API 调用。 |
| `kernel/core/comps/device-mapper/src/lib.rs` | DM crate 对外接口 | 对外暴露 `DmManager`、`DmDevice`、`DmTable`、target 模块和错误类型。 | `aster-core` 只应依赖稳定入口，不应直接知道 DM crate 内部文件组织。 |
| `kernel/core/comps/device-mapper/src/manager.rs` | mapper 全局管理 | 管理 name、uuid、minor/id 索引；支持 create、remove、remove_all、rename、lookup；处理重复名、busy remove 和 rename 回滚。 | Linux DM control 命令都需要按用户可见身份查找 mapper，生命周期必须集中维护。 |
| `kernel/core/comps/device-mapper/src/device.rs` | mapper 设备对象 | 定义 `DmDevice` 并实现 `BlockDevice`；维护 active/inactive table、readonly、suspend/resume、event number、in-flight BIO 和 wait 语义。 | mapper 既是 control 面管理对象，也是数据面 block device；状态机和 I/O 入口必须在同一个设备对象上汇合。 |
| `kernel/core/comps/device-mapper/src/table.rs` | table 与数据面核心 | 表达连续 target table；校验 logical sector range；按 BIO range 查找 target；执行 table-level split、target dispatch、remap/zero/error action、flush fan-out 和 completion 聚合；提供 table/status/deps 输出基础。 | table 是 DM 数据面的核心边界：它把上层 logical BIO 拆成一个或多个 target-local action，再映射到底层 block device。 |
| `kernel/core/comps/device-mapper/src/target/mod.rs` | target 统一接口 | 定义 `DmTarget` trait object、`DmTargetBox`、`TargetRange`、`TargetIoAction`；集中维护 supported target 名称/版本；提供 `parse_target_with`。 | 新 target 不应让 table/control 面继续扩散 concrete type 分支；Linux-visible target metadata 也需要统一来源。 |
| `kernel/core/comps/device-mapper/src/target/linear.rs` | linear target | 解析 `<dev> <offset>`；持有 backing `BlockDeviceLease`；校验容量；执行固定 sector offset remap；输出 status/deps。 | linear 是 LVM2 最基础 target，也是多 segment 和 mixed table 的基础能力。 |
| `kernel/core/comps/device-mapper/src/target/striped.rs` | striped target | 解析 `<stripe_count> <chunk_size> <dev offset>...`；校验 N-way 几何和 backing 容量；实现 stripe/chunk 映射、跨 chunk split、status/deps 输出。 | striped 需要多 backing、chunk 轮转和 target-local split，不能由 linear 语义覆盖。 |
| `kernel/core/comps/device-mapper/src/target/zero.rs` | zero target | 支持无 backing；read 返回零；write/discard/write-zeroes/flush 直接成功；deps 为空。 | Linux DM 常用零设备 target 需要覆盖无 backing 的成功完成路径。 |
| `kernel/core/comps/device-mapper/src/target/error.rs` | error target | 支持无 backing；普通 I/O 返回 I/O error；flush 在无 backing table 上成功；支持 table/status/deps 查询。 | error target 用于稳定表达错误路径，也用于验证 control 面和无 backing 行为。 |

## 2. Asterinas control 入口与设备注册框架改动

本节覆盖 DM 为接入 Linux 用户态路径而对 `aster-core` device/misc/registry 层做的改动。

| 文件 | 所在层 | 主要承载功能 | 为什么必须改 |
|---|---|---|---|
| `kernel/core/Cargo.toml` | kernel dependency graph | 让 `aster-core` 依赖 `aster-device-mapper`。 | `/dev/mapper/control` 的 ioctl 入口在 `aster-core`，但实际 DM 语义由独立 DM crate 提供。 |
| `kernel/core/src/device/misc/mod.rs` | misc device 初始化 | 在 first kthread 阶段初始化 DM control 设备。 | Linux 用户态通过 misc major 10 下的 `/dev/mapper/control` 进入 DM control 面。 |
| `kernel/core/src/device/misc/device_mapper.rs` | Linux DM ioctl ABI | 注册 `DmControlDevice`；提供 `DmControlFile::ioctl()`；解析/校验 `dm_ioctl` buffer；实现 version、create/remove/rename/status/wait、table load/status/deps、target version 等 control 命令；把 table-load 参数适配到 DM crate target parser。 | 这是 Linux `dmsetup`/LVM2 和 Asterinas DM core 的 ABI 桥梁：raw ioctl number 决定命令，`dm_ioctl` buffer 承载参数和写回结果。 |
| `kernel/core/src/device/mod.rs` | device 子系统 | 接入 runtime block device 节点创建/删除所需的设备层能力。 | mapper 不是启动期固定设备，而是 control 命令运行期创建出的 block device。 |
| `kernel/core/src/device/registry/block.rs` | block device registry 与 devtmpfs | 支持运行期注册/注销 mapper block device；创建 `/dev/dm-N` 和 `/dev/mapper/<name>` alias；支持 rename 回滚、open count/busy remove、legacy block ioctl。 | LVM2 和文件系统最终打开的是 mapper block node；仅有 `/dev/mapper/control` 不足以承载数据面。 |
| `kernel/core/src/device/registry/char.rs` | char registry | 配合 misc/char device 注册 control 设备。 | `/dev/mapper/control` 是字符设备入口，需要进入 char registry 才能被 VFS open。 |
| `kernel/core/src/device/registry/mod.rs` | device registry 汇总 | 汇总 char/block registry 的初始化和枚举入口。 | DM 同时引入 control char device 和 runtime block device，需要 registry 层统一暴露。 |

## 3. 通用 block/BIO 框架改动

本节覆盖 DM 数据面依赖的通用 block 框架能力。这些不是 DM target 本身，但 stacked block device、partition 和真实驱动都会受影响。

| 文件 | 所在层 | 主要承载功能 | 为什么必须改 |
|---|---|---|---|
| `kernel/core/comps/block/src/lib.rs` | block crate 公共接口 | 扩展 `BlockDevice` 相关生命周期；引入/暴露 `BlockDeviceLease`、registry pending/commit/unregister、major/name 枚举等能力。 | DM table 和 filesystem mount 都会长期持有 block device，不能只靠裸 `Arc` 或启动期固定注册模型。 |
| `kernel/core/comps/block/src/device_id.rs` | block major/minor 管理 | 支撑 block major 分配、名称记录和 device id 生命周期。 | DM runtime minor 分配、`/proc/devices` 和 mapper node 都依赖稳定的 major/minor 语义。 |
| `kernel/core/comps/block/src/impl_block_device.rs` | block trait 辅助实现 | 适配 `BlockDevice` trait 和测试/包装设备实现。 | block trait 变化后，非 DM 的 block device 实现也必须保持接口一致。 |
| `kernel/core/comps/block/src/bio.rs` | BIO 表达与 stacked I/O | 支持 `Bio`/`SubmittedBio` 当前层 sector range、remap、split、child completion 聚合、range BIO 类型、测试专用 exact segment。 | DM 和 partition 都是 stacked block device；上层一次 BIO 可能被 remap 或拆成多个 backing BIO，但原始提交者只能收到一次最终完成。 |
| `kernel/core/comps/block/src/request_queue.rs` | driver request queue | 让 queue 使用 BIO 当前层 range 进行 merge/dispatch；真实类型是 `BioRequestSingleQueue`/`BioRequest`。 | BIO 经过 DM/partition remap 后，driver 端不能继续按原始 logical sector 访问 backing。 |
| `kernel/core/comps/block/src/partition.rs` | partition stacked 语义 | partition 改用统一 BIO remap 模型。 | partition 和 DM 都会改变 logical sector 到 backing sector 的映射，二者叠加时必须共享同一套 BIO 语义。 |

## 4. 文件系统、procfs 与后端驱动适配

本节覆盖 DM 能被真实用户态工具、文件系统和后端驱动使用所需的通用内核适配。

| 文件 | 所在层 | 主要承载功能 | 为什么必须改 |
|---|---|---|---|
| `kernel/core/src/fs/vfs/fs_apis/registry.rs` | VFS mount source 解析 | block mount source 返回可持有的 `BlockDeviceLease`。 | ext2/exfat 挂载 mapper 后，应阻止 mapper 或 backing 在 mount 生命周期内被提前删除。 |
| `kernel/core/src/fs/fs_impls/ext2/fs_type.rs` | ext2 mount 入口 | 适配 block lease 形式的 mount source。 | ext2 是 DM 系统验收中主要使用的文件系统，需要持有 mapper 生命周期。 |
| `kernel/core/src/fs/fs_impls/ext2/fs.rs` | ext2 filesystem | 持有 block lease 并适配新的 block 访问接口。 | 已挂载文件系统不能因 mapper remove 或 backing unregister 失去底层设备。 |
| `kernel/core/src/fs/fs_impls/exfat/fs.rs` | exfat filesystem | 适配 block lease 生命周期。 | 通用文件系统 block mount 语义不能只让 ext2 特判成立。 |
| `kernel/core/src/fs/fs_impls/ext2/test_utils.rs` | ext2 测试辅助 | 测试辅助代码改用 lease 形式的 block device。 | 测试路径需要和真实 mount 生命周期一致，避免绕过 lease。 |
| `kernel/core/src/vm/page_cache/tests/utils.rs` | page cache 测试辅助 | 适配新的 block device 持有方式。 | page cache 测试也会构造 block-backed 场景，需要跟随 block trait/lease 变化。 |
| `kernel/core/src/fs/vfs/path/dentry.rs` | VFS path 删除 | 支持条件 unlink/rmdir 能力。 | mapper alias 创建失败、rename 回滚和删除时，必须避免误删非目标节点。 |
| `kernel/core/src/fs/vfs/path/mod.rs` | VFS path API | 暴露条件删除路径操作。 | device registry 需要通过 VFS 安全维护 `/dev/dm-N` 和 `/dev/mapper/<name>`。 |
| `kernel/core/src/fs/fs_impls/procfs/devices.rs` | `/proc/devices` | 输出 block major/name，例如 `device-mapper`、`virtblk`、`nvme`。 | LVM2 和 util-linux 会通过 Linux 常用发现路径识别 block major。 |
| `kernel/core/src/fs/fs_impls/procfs/mod.rs` | procfs 汇总 | 接入 `/proc/devices` 输出。 | 用户态工具依赖 procfs 观察内核设备能力。 |
| `kernel/core/comps/virtio/src/device/block/device.rs` | virtio-blk 后端 | 适配新的 BIO/request queue；承接 DM remap 后的 read/write/flush/discard/write-zeroes。 | NixOS 系统验收的测试盘主要是 virtio-blk，DM 数据面最终会落到这里。 |
| `kernel/core/comps/virtio/src/device/block/mod.rs` | virtio-blk 模块 | 暴露 range I/O 能力和 block backend 支撑。 | DM range BIO 需要真实 backing driver 能识别或拒绝对应请求。 |
| `kernel/core/comps/virtio/src/lib.rs` | virtio crate 汇总 | 适配 virtio block 模块接口变化。 | block 后端接口变化需要沿 crate 边界导出。 |
| `kernel/core/comps/nvme/src/device/block_device.rs` | NVMe 后端 | 适配新的 BIO/request queue；对尚未支持的 range I/O 返回 `NotSupported`。 | 不能让用户误以为 NVMe 已实现真实 discard/write-zeroes 后端命令。 |
| `kernel/core/comps/nvme/src/lib.rs` | NVMe crate 汇总 | 适配 NVMe block 模块接口变化。 | block 后端接口变化需要沿 crate 边界导出。 |
| `kernel/core/comps/mlsdisk/src/lib.rs` | mlsdisk block 适配 | 适配 block trait/BIO 变化。 | 通用 block 框架变化不能只让 DM 和 virtio 编译通过。 |
| `kernel/core/comps/mlsdisk/src/layers/5-disk/mlsdisk.rs` | mlsdisk 具体实现 | 跟随新的 block/BIO 接口。 | mlsdisk 作为已有 block device 实现，需要保持接口兼容。 |

## 5. 构建、启动和用户态环境支撑

本节覆盖为了运行真实 NixOS/LVM2/DM 场景而修改的配置和启动链路。

| 文件 | 所在层 | 主要承载功能 | 为什么必须改 |
|---|---|---|---|
| `Cargo.toml` | workspace | 将 `aster-device-mapper` 纳入 workspace。 | DM crate 需要进入统一构建和测试图。 |
| `Cargo.lock` | dependency lock | 锁定新增 crate/dependency graph。 | workspace 依赖变化需要可复现。 |
| `Makefile` | 构建与清理入口 | 提供适合 DM 系统验收的构建/清理入口，包括测试盘清理。 | LVM2 会写入 PV/VG/LV 元数据，旧测试盘状态会污染下次验证。 |
| `distro/etc_nixos/configuration.nix` | NixOS guest 环境 | 内置 `lvm2`、`e2fsprogs`、`util-linux`、`strace` 和测试盘 locator。 | DM/LVM2 验收依赖真实用户态工具，启动后临时补装不可重复。 |
| `distro/etc_nixos/modules/systemd.nix` | NixOS/systemd 启动 | 调整系统服务和启动行为以减少 guest 启动阻塞。 | 系统验收需要在限定时间内进入 root shell，慢启动会掩盖真实测试结果。 |
| `distro/etc_nixos/overlays/hello-asterinas/default.nix` | Nix overlay | 构建并安装通用 `aster-test-disk-locator`。 | 固定 QEMU block PCI 拓扑，并通过 `/proc/cmdline` 声明首块测试盘；guest 按序号推导并验证设备，不依赖已移除的 VirtIO private ioctl 或隐式枚举。 |
| `tools/nixos/build_nixos.sh` | NixOS image 构建入口 | 调用 NixOS 镜像构建流程。 | guest 用户工具与 locator 的声明主要位于 NixOS 配置和 overlay；本脚本不应被描述为其主要实现位置。 |
| `tools/nixos/run.sh` | NixOS/QEMU 启动 | 支持多块 `DM_TEST_IMAGES`；设置 root disk boot order；校验测试盘安全；隔离 OVMF/boot protocol 影响。 | striped、mixed、跨 PV LVM2 和 reboot recovery 都依赖多块持久测试盘；同时不能误伤 root image。 |
| `tools/qemu_args.sh` | QEMU 参数生成 | 统一组织 acceleration、firmware 与通用 QEMU 参数。 | KVM/OVMF 等启动参数应在该层理解；多盘、root disk 保护与测试盘安全检查的主要逻辑位于 `tools/nixos/run.sh`。 |
| `myshell/br.sh` | 本地启动辅助 | 适配当前 boot/run 参数约定。 | 手工验证路径需要跟随 NixOS/QEMU 启动链路变化。 |
| `docs/test.md` | 开发测试手册 | 说明在目标 crate 手动执行 `cargo osdk test`。 | ktest 需要可重复的 crate-local 手动入口。 |
| `.gitignore` | 仓库忽略规则 | 忽略 DM/NixOS 验收产生的本地临时产物。 | 多盘系统测试会生成 raw image、日志或中间文件，不能污染提交。 |
| `osdk/deps/test-kernel/src/lib.rs` | test kernel 支撑 | 适配 ktest 或 test kernel dependency 变化。 | DM crate ktest 需要能在当前 test kernel 环境下链接运行。 |
| `ostd/src/arch/x86/cpu/cpuid.rs` | x86 CPU 支撑 | 启动/虚拟化环境相关适配。 | DM 系统验收依赖稳定 guest 启动，底层 CPU feature 处理不能成为干扰项。 |
| `ostd/src/arch/x86/kernel/tsc.rs` | x86 时间源支撑 | TSC/时间源相关适配。 | QEMU/NixOS guest 启动和超时判断依赖稳定时间源。 |

## 6. 用户态回归测试代码

本节只列 initramfs 内的轻量回归代码，不展开 `myshell/` 下的系统验收脚本。

| 文件 | 所在层 | 主要承载功能 | 为什么必须改 |
|---|---|---|---|
| `test/initramfs/src/regression/device/device_mapper.c` | initramfs regression | 覆盖 raw DM control ABI、linear table load、runtime node/alias rollback、ext2 mount lease、range ioctl、wait errno、`SA_RESTART` 与 rename/setuuid waiter 唤醒。 | 在不启动完整 NixOS/LVM2 的情况下保护原始 DM 用户 ABI；不替代完整 target 数据面或 LVM2 编排。 |
| `test/initramfs/src/regression/device/run_test.sh` | initramfs regression runner | 调度 device 目录下的回归 ELF。 | 目录级 regression 仍通过该脚本按顺序运行。 |
| `test/initramfs/src/regression/scripts/run_regression_test.sh` | initramfs regression selector | 接受目录或单个 ELF selector，并拒绝路径穿越。 | `REGRESSION_TESTS=device/device_mapper` 使 focused DM C ABI 回归不受无关目录失败阻断。 |
| `test/initramfs/src/regression/fs/procfs/devices.c` | procfs regression | 覆盖 `/proc/devices` block major/name 输出。 | LVM2 依赖该发现路径，不能只靠系统测试发现回归。 |
| `test/initramfs/src/regression/fs/run_test.sh` | fs regression runner | 接入 procfs devices 回归。 | 新增 procfs 兼容输出需要进入现有 regression 流程。 |
| `test/initramfs/src/regression/io/file_io/block_device.c` | block file I/O regression | 覆盖 block device 文件打开、size/ioctl 或基础 I/O 行为。 | DM mapper 和普通 block device 都依赖 block file 用户态入口。 |

## 7. 边界说明

| 不在本文展开的内容 | 原因 | 应查看的位置 |
|---|---|---|
| `myshell/` 下的 DM 系统测试脚本 | 系统测试入口变化快，且脚本本身不是生产代码改动原因说明。 | `AGENTS.md`、`log/device-mapper-progress.md` 与实际脚本；通用分层见 `docs/test.md` |
| patch 生成、同步和验证 | 不是本文主题，且 patch 内容应由当前 git diff/patch 文件本身作为事实来源。 | patch 目录或后续 patch 阶段记录 |
| 每日工程日志、测试日志、docx 交付物 | 本文是文件改动说明，不是阶段日志或附件索引。 | `log/`、`docs/` 下对应文档 |
| 完整 Linux DM 生态兼容 | 当前实现不声明完整 udev/systemd/sysfs/queue stacking 或复杂 target 族兼容。 | `docs/global.md` |
