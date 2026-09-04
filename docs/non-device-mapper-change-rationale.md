# Device Mapper 项目改动清单

## 执行摘要

本文从 review 和维护视角整理 `dm` 分支为了支持当前 Device Mapper 能力所涉及的主要改动。

表格按“功能/职责 → 改动原因 → 对应改动 → 主要文件”组织，便于快速判断每个改动属于哪一层、为什么需要、应从哪里 review。

范围分三类：

1. 仅属于 DM crate 的改动；
2. 对 Asterinas 内核框架的改动；
3. 配置、构建和启动支撑改动。

本文不列系统测试脚本、不列补丁目录，也不记录测试日志和阶段进度。系统验收命令与结果应看 [test.md](test.md) 和 [device-mapper-progress.md](../log/device-mapper-progress.md)。

## 1. 仅属于 DM crate 的改动清单

本节只覆盖 [kernel/core/comps/device-mapper/](../kernel/core/comps/device-mapper/) 内的 Device Mapper core crate。它负责 Linux DM core 的内核侧抽象、mapper 生命周期、table/target 语义和 BIO 数据面。

| 功能/职责 | 改动原因 | 对应改动 | 主要文件 |
|---|---|---|---|
| DM crate 对外接口 | 需要把 DM core 从 `aster-core` 中独立出来。<br>让控制面只依赖清晰的 crate API。 | 新增并维护 `aster-device-mapper` crate。<br>对外暴露 `DmManager`、`DmDevice`、`DmTable`、target 类型和错误类型。 | [lib.rs](../kernel/core/comps/device-mapper/src/lib.rs) |
| mapper 全局管理 | Linux DM 需要按 name、uuid、minor 查找 mapper。<br>create/remove/rename 必须有一致的生命周期语义。 | 维护 name、uuid、minor/id 索引。<br>支持 create、remove、remove_all、rename、按 name/uuid 查找。<br>处理重复名、busy remove 和 rename 回滚。 | [manager.rs](../kernel/core/comps/device-mapper/src/manager.rs) |
| mapper 设备对象 | mapper 本身要作为 block device 接收 BIO。<br>同时还要维护 active/inactive table 和运行状态。 | 表达一个已创建的 DM block device。<br>维护 active/inactive table、readonly、suspend/resume、event number、open/lifecycle 状态。 | [device.rs](../kernel/core/comps/device-mapper/src/device.rs) |
| active/inactive table 生命周期 | Linux DM 的 table-load 不是立即替换 active table。<br>failed load 也不能破坏现有可用 table。 | table load 写入 inactive table。<br>resume 后 inactive 切换为 active。<br>failed load 不污染现有 table 和 event 状态。 | [device.rs](../kernel/core/comps/device-mapper/src/device.rs)<br>[table.rs](../kernel/core/comps/device-mapper/src/table.rs) |
| table 结构与连续性校验 | DM table 必须表达连续的逻辑 sector 空间。<br>否则 BIO 查找和 split 会产生不确定行为。 | 用有序 target 列表表达 table。<br>校验 table 非空、从 sector 0 开始、相邻 range 连续。<br>导出 table capacity。 | [table.rs](../kernel/core/comps/device-mapper/src/table.rs) |
| backing 生命周期持有 | mapper table 会长期引用底层 block device。<br>不能让 backing 在 I/O 期间被注销。 | target/table 通过 `BlockDeviceLease` 持有 backing。<br>table 层拒绝当前不支持的 DM-on-DM backing。 | [table.rs](../kernel/core/comps/device-mapper/src/table.rs)<br>[target/linear.rs](../kernel/core/comps/device-mapper/src/target/linear.rs)<br>[target/striped.rs](../kernel/core/comps/device-mapper/src/target/striped.rs) |
| target 统一接口 | 新增 target 时不应在 table/control-plane 中继续扩散 concrete type 分支。<br>需要统一 target 行为边界。 | 抽象 `DmTarget` trait object。<br>统一 metadata、logical range、backing 枚举、table/status 参数和 I/O mapping。<br>`DmTable` 保存 `Vec<DmTargetBox>`。 | [target/mod.rs](../kernel/core/comps/device-mapper/src/target/mod.rs)<br>[table.rs](../kernel/core/comps/device-mapper/src/table.rs) |
| target metadata 与解析入口 | `DM_TABLE_LOAD`、`DM_LIST_VERSIONS`、target-version 查询需要同一批 target 名称和版本。 | 集中维护当前支持 target 的 Linux-visible 名称和版本。<br>通过 `parse_target_with` 将 table-load 字符串解析为 typed target。<br>当前支持 `error`、`zero`、`linear`、`striped`。 | [target/mod.rs](../kernel/core/comps/device-mapper/src/target/mod.rs) |
| table/status/deps 输出基础 | `dmsetup table/status/deps` 是用户可见 ABI。<br>输出必须由 target 语义和 table 顺序共同决定。 | target 输出自身 `DM_TABLE_STATUS` 参数。<br>table 汇总 target 顺序、logical range 和 backing deps。 | [target/mod.rs](../kernel/core/comps/device-mapper/src/target/mod.rs)<br>[table.rs](../kernel/core/comps/device-mapper/src/table.rs) |
| BIO table-level split | 一个 BIO 可能跨多个 target。<br>上层调用者仍只能看到一次最终完成。 | 按 table range 拆分跨 target BIO。<br>再委托 target-local mapping。<br>聚合 child completion 回原始 BIO。 | [table.rs](../kernel/core/comps/device-mapper/src/table.rs) |
| BIO remap action 模型 | table 需要统一执行 target 的 I/O 决策。<br>target 又不应直接依赖外层 BIO 分发细节。 | target 返回 `TargetIoAction`。<br>支持 remap 到 backing、返回 I/O error、zero/direct completion。<br>table 统一执行 action。 | [target/mod.rs](../kernel/core/comps/device-mapper/src/target/mod.rs)<br>[table.rs](../kernel/core/comps/device-mapper/src/table.rs) |
| Flush fan-out | Flush 没有数据 range，但需要覆盖所有 backing。<br>重复 backing 不能重复 flush。 | 从所有 target 收集 backing。<br>按 backing id 去重下发 flush。<br>聚合 child flush completion。<br>无 backing table 直接完成。 | [table.rs](../kernel/core/comps/device-mapper/src/table.rs) |
| `error` target | 需要 Linux DM 的稳定错误 target。<br>用于控制面、错误路径和无 backing 行为验证。 | 支持无 backing 参数。<br>table/status/deps 可查询。<br>Read/Write 返回 I/O error。<br>Flush 对无 backing table 成功。 | [target/error.rs](../kernel/core/comps/device-mapper/src/target/error.rs) |
| `zero` target | 需要 Linux DM 的零设备 target。<br>用于无 backing 成功完成路径和零读语义。 | 支持无 backing 参数。<br>Read 返回全 0。<br>Write 丢弃成功。<br>Flush/Discard/WriteZeroes direct-complete。<br>deps 为空。 | [target/zero.rs](../kernel/core/comps/device-mapper/src/target/zero.rs) |
| `linear` target | linear 是 LVM2 最基础 target。<br>也是多 segment 和 mixed table 的基础能力。 | 支持 `<dev> <offset>` 参数。<br>解析 backing、校验容量。<br>执行 sector offset remap。<br>输出 status 和 deps。 | [target/linear.rs](../kernel/core/comps/device-mapper/src/target/linear.rs) |
| `striped` target | striped 需要多 backing、chunk 轮转和跨 chunk split。<br>单纯 linear 语义无法覆盖。 | 支持 `<stripe_count> <chunk_size> <dev offset>...` 参数。<br>校验 N-way 几何和容量。<br>实现 chunk/stripe 映射、跨 chunk split、status/deps 输出。 | [target/striped.rs](../kernel/core/comps/device-mapper/src/target/striped.rs) |
| mixed table | LVM2 可在同一 LV 内生成不同 target 类型的连续 segment。<br>需要验证 table-level split 与 target-level split 可组合。 | 支持同一 table 中相邻 target 类型不同。<br>典型形态是前段 linear、后段 striped。 | [table.rs](../kernel/core/comps/device-mapper/src/table.rs)<br>[target/linear.rs](../kernel/core/comps/device-mapper/src/target/linear.rs)<br>[target/striped.rs](../kernel/core/comps/device-mapper/src/target/striped.rs) |
| readonly 与 suspend 数据面策略 | Linux DM 状态机会影响 I/O 是否可进入 table。<br>只读和挂起不能只在控制面标记。 | readonly mapper 拒绝 write-like BIO。<br>suspend 状态拒绝新 I/O。<br>等待既有 I/O 完成。<br>Read/Flush 按当前状态机处理。 | [device.rs](../kernel/core/comps/device-mapper/src/device.rs)<br>[table.rs](../kernel/core/comps/device-mapper/src/table.rs) |
| range BIO 支持 | Discard / WriteZeroes 没有普通 data segment。<br>但仍需要按 DM logical range remap 或 direct-complete。 | 将 Discard / WriteZeroes 纳入 write-like range BIO 语义。<br>linear/striped remap。<br>error 返回 I/O error。<br>zero direct-complete。 | [table.rs](../kernel/core/comps/device-mapper/src/table.rs)<br>[target/error.rs](../kernel/core/comps/device-mapper/src/target/error.rs)<br>[target/zero.rs](../kernel/core/comps/device-mapper/src/target/zero.rs)<br>[target/linear.rs](../kernel/core/comps/device-mapper/src/target/linear.rs)<br>[target/striped.rs](../kernel/core/comps/device-mapper/src/target/striped.rs) |
| DM core ktest 支撑 | target/table/BIO 语义需要不依赖用户态脚本的内核级证明。<br>复杂 split/completion 也需要可控 mock。 | 提供 crate-local ktest。<br>覆盖 table、target、BIO split、flush、completion 和 parser。<br>包含 recording backing 和 deferred completion mock。 | [table.rs](../kernel/core/comps/device-mapper/src/table.rs)<br>[target/mod.rs](../kernel/core/comps/device-mapper/src/target/mod.rs)<br>各 target 文件 |

## 2. 对 Asterinas 内核框架的改动清单

本节覆盖 DM 项目为了接入真实 Linux 用户态路径而对 Asterinas 通用内核框架做出的支撑性改动。这些改动不属于 DM target 算法本身，但会被 `dmsetup`、LVM2、filesystem mount 或 block I/O 间接依赖。

| 功能/职责 | 改动原因 | 对应改动 | 主要文件 |
|---|---|---|---|
| `aster-core` 接入 DM crate | `/dev/mapper/control` 在 `aster-core` 中注册。<br>实际 DM 语义应委托给独立 crate。 | 在 core crate 中引入 `aster-device-mapper`。<br>ioctl 层调用 DM manager、table 和 target parser。 | [kernel/core/Cargo.toml](../kernel/core/Cargo.toml)<br>[device_mapper.rs](../kernel/core/src/device/misc/device_mapper.rs) |
| DM misc 控制设备 | Linux 用户态通过 `/dev/mapper/control` 进入 DM。<br>需要一个 misc/char device 入口承接 ioctl。 | 在 misc device 初始化中注册 DM control device。<br>将 ioctl 分派到 DM 控制面实现。 | [misc/mod.rs](../kernel/core/src/device/misc/mod.rs)<br>[device_mapper.rs](../kernel/core/src/device/misc/device_mapper.rs) |
| Linux DM ioctl ABI | `dmsetup` 和 LVM2 依赖 Linux DM ioctl buffer layout。<br>不能只暴露 Rust 内部 API。 | 实现核心命令的 buffer 解析、flags 处理和结果写回。<br>覆盖 version、create/remove/rename/status/wait、table load/status/deps、target version 等。 | [device_mapper.rs](../kernel/core/src/device/misc/device_mapper.rs) |
| target table-load 适配层 | target parser 需要 backing lookup。<br>但路径解析和 registry lookup 属于 `aster-core` 环境。 | 从 `dm_target_spec` 读取 type、start、length、params。<br>调用 DM crate `parse_target_with`。<br>将 VFS/block lookup 留在 `aster-core`。 | [device_mapper.rs](../kernel/core/src/device/misc/device_mapper.rs) |
| active/inactive table ioctl 可见性 | `dmsetup table --inactive`、reload、clear、resume 都依赖 table selector。<br>状态不可只存在于 DM crate 内部。 | 支持 table/status/deps 对 active 与 inactive table 的选择。<br>保持用户可见生命周期语义。 | [device_mapper.rs](../kernel/core/src/device/misc/device_mapper.rs) |
| mapper runtime block node | 创建 mapper 后，用户态必须能打开 `/dev/dm-N` 和 `/dev/mapper/<name>`。<br>LVM2 也依赖这些 runtime node。 | 支持运行期创建/删除 `/dev/dm-N`。<br>创建、rename、删除 `/dev/mapper/<name>` alias。 | [device/mod.rs](../kernel/core/src/device/mod.rs)<br>[registry/block.rs](../kernel/core/src/device/registry/block.rs) |
| block device 动态注册生命周期 | DM device 是运行期创建和删除的 block device。<br>原有启动期固定设备模型不足。 | 增加 pending registration、commit/abort、begin unregister、commit unregister。<br>避免半注册或已注销设备继续被 lookup。 | [block lib.rs](../kernel/core/comps/block/src/lib.rs)<br>[registry/block.rs](../kernel/core/src/device/registry/block.rs) |
| block open count / busy remove | 已打开、已挂载或被 table 持有的 block device 不应被删除。<br>否则会破坏后续 I/O 生命周期。 | 维护 block device open count 和 registry 生命周期。<br>让 busy remove 返回错误而不是拆掉正在使用的设备。 | [registry/block.rs](../kernel/core/src/device/registry/block.rs)<br>[block lib.rs](../kernel/core/comps/block/src/lib.rs) |
| `BlockDeviceLease` | filesystem mount 和 DM table 都是长期持有者。<br>需要统一生命周期保护，而不是裸 `Arc`。 | 引入并使用 block device lease。<br>VFS mount、DM backing 和测试辅助代码统一通过 lease 持有 block device。 | [block lib.rs](../kernel/core/comps/block/src/lib.rs)<br>[registry.rs](../kernel/core/src/fs/vfs/fs_apis/registry.rs)<br>[ext2/fs.rs](../kernel/core/src/fs/fs_impls/ext2/fs.rs)<br>[exfat/fs.rs](../kernel/core/src/fs/fs_impls/exfat/fs.rs) |
| block major/name 枚举 | LVM2 会通过 Linux 常用发现路径识别 block major。<br>`/proc/devices` 需要输出兼容名称。 | block registry 记录 major name。<br>procfs 暴露 `device-mapper`、`virtblk`、`nvme` 等 block major 名称。 | [block lib.rs](../kernel/core/comps/block/src/lib.rs)<br>[procfs/devices.rs](../kernel/core/src/fs/fs_impls/procfs/devices.rs) |
| BIO remap 语义 | stacked block device 需要把上层 logical sector 转换为下层 sector。<br>request queue 不能一直看原始 range。 | BIO 增加当前层 sector range/remap 能力。<br>DM、partition 和 request queue 使用 remap 后 range。 | [bio.rs](../kernel/core/comps/block/src/bio.rs)<br>[request_queue.rs](../kernel/core/comps/block/src/request_queue.rs)<br>[partition.rs](../kernel/core/comps/block/src/partition.rs) |
| BIO split 与 completion 聚合 | 一个上层 BIO 可能对应多个 backing BIO。<br>原始提交者仍应只收到一次最终完成。 | 支持 submitted BIO 切成多个 child BIO。<br>将多个 child completion 聚合回 original BIO。 | [bio.rs](../kernel/core/comps/block/src/bio.rs) |
| range BIO 类型 | Discard / WriteZeroes 有 sector range 但没有普通数据段。<br>DM 和 driver 需要统一表达。 | 支持 range BIO 在 block ioctl、driver 与 DM 映射间传递。<br>让 range I/O 进入统一 BIO 路径。 | [bio.rs](../kernel/core/comps/block/src/bio.rs)<br>[device/block/device.rs](../kernel/core/comps/virtio/src/device/block/device.rs)<br>[nvme block_device.rs](../kernel/core/comps/nvme/src/device/block_device.rs) |
| request queue 使用 remap 后 range | BIO 被 partition/DM remap 后，queue dispatch 必须使用当前层范围。<br>否则会写错 backing sector。 | request queue 按 BIO 当前 range 做 merge/dispatch。<br>不固定使用最初提交的 logical range。 | [request_queue.rs](../kernel/core/comps/block/src/request_queue.rs) |
| partition 与 stacked 语义对齐 | partition 也是 stacked block device。<br>它与 DM 必须共享 sector remap 语义。 | partition 改用同一套 BIO remap/split 模型。<br>避免 partition 与 DM 叠加时 range 解释不一致。 | [partition.rs](../kernel/core/comps/block/src/partition.rs) |
| VFS mount source 到 block lease | ext2/exfat 挂载 mapper 后，应阻止 mapper/backing 被提前删除。<br>mount 解析需要返回可持有的生命周期对象。 | VFS block mount 解析返回 `BlockDeviceLease`。<br>filesystem mount 持有 block device 生命周期。 | [registry.rs](../kernel/core/src/fs/vfs/fs_apis/registry.rs)<br>[ext2/fs_type.rs](../kernel/core/src/fs/fs_impls/ext2/fs_type.rs)<br>[ext2/fs.rs](../kernel/core/src/fs/fs_impls/ext2/fs.rs)<br>[exfat/fs.rs](../kernel/core/src/fs/fs_impls/exfat/fs.rs) |
| page cache / fs 测试工具适配 lease | 测试辅助代码如果仍绕过 lease，会与真实 mount 生命周期不一致。 | 文件系统和 page cache 测试辅助代码改用 lease 形式的 block device。 | [ext2/test_utils.rs](../kernel/core/src/fs/fs_impls/ext2/test_utils.rs)<br>[page_cache/tests/utils.rs](../kernel/core/src/vm/page_cache/tests/utils.rs) |
| devtmpfs 条件删除支撑 | mapper node 创建失败、rename 回滚、删除都要避免误删非目标节点。 | 增加条件 unlink/rmdir 能力。<br>支撑 runtime mapper node 和 alias 的安全回滚。 | [dentry.rs](../kernel/core/src/fs/vfs/path/dentry.rs)<br>[path/mod.rs](../kernel/core/src/fs/vfs/path/mod.rs) |
| `/proc/devices` Linux 兼容输出 | 用户态工具通过 `/proc/devices` 判断 block major。<br>缺少输出会影响 LVM2 探测。 | procfs 暴露 block major/name。<br>让 Linux 常用发现路径能识别 Asterinas block device。 | [procfs/devices.rs](../kernel/core/src/fs/fs_impls/procfs/devices.rs)<br>[procfs/mod.rs](../kernel/core/src/fs/fs_impls/procfs/mod.rs) |
| legacy block ioctl | LVM2、mkfs、blkid、util-linux 会查询 size、sector size、read-ahead 和 range I/O 能力。 | 支持相关 legacy block ioctl。<br>为 DM mapper 和普通 block device 提供用户态兼容入口。 | [device_mapper.rs](../kernel/core/src/device/misc/device_mapper.rs)<br>[registry/block.rs](../kernel/core/src/device/registry/block.rs)<br>[bio.rs](../kernel/core/comps/block/src/bio.rs) |
| virtio-blk range I/O 后端 | DM remap 后的 Discard / WriteZeroes 需要真实 backing driver 承接。<br>VirtIO 是当前 NixOS 测试盘后端。 | virtio block 后端按协商能力处理 discard/write-zeroes 请求。 | [virtio block device.rs](../kernel/core/comps/virtio/src/device/block/device.rs)<br>[virtio block mod.rs](../kernel/core/comps/virtio/src/device/block/mod.rs) |
| NVMe range I/O 边界 | NVMe 尚未接入真实 discard/write-zeroes 后端命令。<br>需要避免用户误以为已支持。 | NVMe block device 对未支持的 range I/O 显式返回 NotSupported。 | [nvme block_device.rs](../kernel/core/comps/nvme/src/device/block_device.rs) |
| mlsdisk block 接口适配 | block trait/BIO 变化是通用框架能力。<br>不能只让 DM 和 VirtIO 编译通过。 | mlsdisk 相关 block device 实现适配新的 block/BIO 接口。 | [mlsdisk lib.rs](../kernel/core/comps/mlsdisk/src/lib.rs)<br>[mlsdisk.rs](../kernel/core/comps/mlsdisk/src/layers/5-disk/mlsdisk.rs) |
| regression 入口支撑 | 基础 Linux 接口需要有轻量回归保护。<br>避免后续框架改动破坏 DM 依赖的最小 ABI。 | initramfs 用户态回归覆盖最小 DM control ABI、`/proc/devices` 和 block device 文件 I/O。 | [device_mapper.c](../test/initramfs/src/regression/device/device_mapper.c)<br>[devices.c](../test/initramfs/src/regression/fs/procfs/devices.c)<br>[block_device.c](../test/initramfs/src/regression/io/file_io/block_device.c) |

## 3. 配置、构建和启动支撑改动清单

本节只列 DM 项目运行真实 NixOS/LVM2 场景所需的配置、构建和启动支撑。这里不列系统测试 suite，不列 patch 生成或 patch 验证内容。

| 功能/职责 | 改动原因 | 对应改动 | 主要文件 |
|---|---|---|---|
| workspace / crate 接入 | DM crate 需要进入 workspace 和 kernel dependency graph。<br>否则内核无法编译并链接 DM core。 | 将 `aster-device-mapper` 纳入 workspace。<br>让 `aster-core` 依赖该 crate。 | [Cargo.toml](../Cargo.toml)<br>[kernel/core/Cargo.toml](../kernel/core/Cargo.toml)<br>[device-mapper Cargo.toml](../kernel/core/comps/device-mapper/Cargo.toml) |
| NixOS guest 工具集 | 真实 LVM2/DM 验收依赖 guest 内用户态工具。<br>启动后不能临时补装依赖。 | NixOS 镜像内置 `lvm2`、`e2fsprogs`、`util-linux`、`strace` 和测试盘 locator。 | [configuration.nix](../distro/etc_nixos/configuration.nix) |
| 测试盘 locator 包 | 多测试盘不能依赖 `/dev/vdX` 枚举顺序。<br>需要按 VirtIO serial 稳定定位。 | 在 Nix overlay 中构建并安装 `aster-dm-disk-locator`。 | [default.nix](../distro/etc_nixos/overlays/hello-asterinas/default.nix) |
| NixOS/systemd 启动优化 | 系统验收需要在限定时间内进入 root shell。<br>慢启动会掩盖真实测试结果。 | 调整 NixOS/systemd 相关配置。<br>减少 guest 启动阻塞。 | [systemd.nix](../distro/etc_nixos/modules/systemd.nix) |
| 多块持久测试盘启动 | striped、mixed、跨 PV LVM2 场景需要多块 PV。<br>reboot recovery 需要测试盘持久化。 | NixOS/QEMU 启动支持 `DM_TEST_IMAGES`。<br>一次附加多块持久 raw image。 | [run.sh](../tools/nixos/run.sh) |
| 稳定 VirtIO serial | guest 内盘符枚举顺序可能变化。<br>脚本和手工验证需要稳定识别测试盘。 | 为测试盘设置稳定 serial：`vdmtest`、`vdmtest2`、`vdmtest3` 等。 | [run.sh](../tools/nixos/run.sh)<br>[qemu_args.sh](../tools/qemu_args.sh) |
| 测试盘安全校验 | 错把 root image 当测试盘会破坏系统镜像。<br>重复附加同一盘会污染测试结论。 | 启动前拒绝 root image 作为 DM 测试盘。<br>拒绝重复测试盘。<br>限制测试盘路径形态。 | [run.sh](../tools/nixos/run.sh) |
| NixOS root 盘启动顺序 | 多测试盘附加后，QEMU boot order 不能漂移。<br>否则可能影响 NixOS root image boot。 | 为 NixOS root disk 设置明确启动优先级。 | [run.sh](../tools/nixos/run.sh) |
| NixOS OVMF 启动固定 | NixOS root image 依赖 UEFI/OVMF 路径。<br>不应被 ktest 常用 boot 环境变量干扰。 | NixOS run 显式保持 OVMF 启动。<br>隔离其他 boot method / boot protocol 变量影响。 | [run.sh](../tools/nixos/run.sh)<br>[qemu_args.sh](../tools/qemu_args.sh) |
| QEMU host forwarding 随机端口 | 并行或残留 QEMU 可能占用固定 host port。<br>端口冲突会表现为启动失败。 | QEMU host forwarding 使用不同随机端口。 | [qemu_args.sh](../tools/qemu_args.sh) |
| 默认 release 构建 | DM/LVM2 系统场景启动和运行较慢。<br>debug 构建会放大验收耗时。 | 将默认构建配置调整为更适合系统验收的 release 路径。 | [Makefile](../Makefile) |
| 测试盘清理入口 | LVM2 会在测试盘写入 PV/VG/LV 元数据。<br>旧元数据会污染下次运行。 | Makefile 提供 DM 测试盘清理入口。<br>可按 `DM_TEST_IMAGES` 或默认命名清理持久测试盘。 | [Makefile](../Makefile) |
| NixOS image 构建支撑 | guest 工具和 locator 必须在 image 构建阶段进入系统。<br>否则系统启动后环境不可重复。 | NixOS image 构建链路包含 DM/LVM2 所需工具包和 locator。 | [build_nixos.sh](../tools/nixos/build_nixos.sh)<br>[configuration.nix](../distro/etc_nixos/configuration.nix)<br>[default.nix](../distro/etc_nixos/overlays/hello-asterinas/default.nix) |

## 边界说明

| 不在本文展开的内容 | 原因 | 应查看的位置 |
|---|---|---|
| 系统测试 suite 与 harness | 用户要求本文件不写这些脚本。<br>系统测试入口变化快，应集中放在测试文档和日志。 | [test.md](test.md)<br>[device-mapper-progress.md](../log/device-mapper-progress.md) |
| 补丁生成、同步和验证 | 不是本文件主题。 | 后续 patch 阶段记录 |
| 历史验证结果和每日工程日志 | 本文件是改动清单。<br>不是阶段日志。 | [log/](../log/) |
| 完整 Linux DM 生态 | 当前实现不声明完整 udev/systemd/sysfs/queue stacking 或复杂 target 族兼容。 | [device-mapper-technical-maintenance.md](device-mapper-technical-maintenance.md) |
