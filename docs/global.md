# Asterinas Device Mapper 全局规划

> **定位**：Device Mapper 项目的唯一全局路线图。
>
> **事实口径**：当前能力依据当前源码和已确认项目决策；无法确认的内容必须先向项目负责人询问。

## 1. 项目目标

在 Asterinas 上提供可由 `dmsetup` 和受控 LVM2 流程使用的 Device Mapper：用户能够创建、激活、挂载、读写、扩缩和恢复当前支持的 LV/table；后续按确认顺序扩展 DM target。

项目边界是实现 **Device Mapper**，不是承担通用设备、VFS、driver、后台任务或发行版用户态服务框架的全面演进。

## 2. 阅读入口

| 用途 | 文档 | 内容 |
|---|---|---|
| 全局路线、优先级与边界 | 本文 | 功能取舍和后续决策 |
| DM 架构与源码路径 | [技术设计与实现说明](device-mapper-technical-design-and-implementation.md) | control/data plane、状态机、lease、target |
| 验收分层与命令 | [测试说明](test.md) | ktest、initramfs C 回归、NixOS suite |
| 框架改动审查 | [评审记录](review.md) | 已知框架问题与暂停边界 |
| 实际执行历史 | [daily log](../log/daily/) | 每日命令、失败和结论 |

## 3. 当前核心功能：标准 Linux 与 Asterinas 对照

> DM crate 内文件使用相对文件名。DM crate 外的内核生产源码使用绝对路径；为缩窄表格，绝对路径按目录边界换行。脚本、Nix、测试、文档和日志不列为生产改动。

| 当前核心功能 | 标准 Linux | Asterinas |
|---|---|---|
| ✅ **`/dev/mapper/control`**<br>用户态进入 DM 控制面 | Linux DM 提供 control character device。<br>`dmsetup`/libdevmapper 通过 `dm_ioctl` 管理 mapper。 | ✅ 注册 control device、解析 `dm_ioctl`、校验 version/flags/buffer，并实现当前命令白名单。<br><br>**生产文件：**<br>/root/github/asterinas/<br>kernel/core/src/device/misc/device_mapper.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/mod.rs |
| ✅ **运行期 mapper 节点**<br>`/dev/dm-N`、`/dev/mapper/<name>` | Linux DM 提供 mapped block device 与 mapper name 路径。 | ✅ 首次 load 发布 primary；首次 resume 发布 alias；rename/remove 使用事务、回滚与隔离。<br><br>**生产文件：**<br>/root/github/asterinas/<br>kernel/core/src/device/registry/block.rs<br>/root/github/asterinas/<br>kernel/core/src/device/mod.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/device_mapper/runtime.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/device_mapper/control.rs<br>manager.rs |
| ✅ **LVM2 显式创建、扩缩与恢复** | LVM2 计算 PV/VG/LV metadata，并通过 DM control ABI 创建或调整 table。 | ✅ 承接 linear、striped、mixed LV 的 create/extend/reduce，以及显式 scan/mknodes/activate。<br><br>**生产文件：**<br>/root/github/asterinas/<br>kernel/core/src/device/misc/device_mapper.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/device_mapper/control.rs<br>table.rs<br>target/linear.rs<br>target/striped.rs |
| ✅ **当前 table 与 target**<br>linear / striped / zero / error | Linux DM target table 描述逻辑 sector 到 backing device 的映射，并支持 target 扩展。 | ✅ 实现四个 target、连续 table、capacity、deps、status 和 target I/O action。<br><br>**生产文件：**<br>table.rs<br>target/mod.rs<br>target/linear.rs<br>target/striped.rs<br>target/zero.rs<br>target/error.rs |
| ✅ **table-scoped readonly**<br>`DM_READONLY_FLAG` | Linux 将 readonly 作为 table mode：load 只暂存 inactive table；首次或 replacement resume 激活 table 后，live device 的可写性随其切换。 | ✅ readonly 存在 immutable `DmTable`，tableless create 不持久化 flag；load 不改变 active I/O mode；active/inactive 查询与 BIO 分派均从同一 table state snapshot 获取 mode。readonly active 在 suspend 中拒绝新 write-like BIO；已在 writable mode 接收的 postponed BIO 遇到 readonly replacement 时完成为 I/O error。<br><br>**生产文件：**<br>table.rs<br>device.rs<br>manager.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/device_mapper.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/device_mapper/control.rs |
| ✅ **写入、跨 segment/chunk I/O 与 flush** | Linux block layer 支持 remap、split、completion 和 flush。 | ✅ BIO 保存 original/current range；DM 规划边界后一次 split，聚合子 BIO；flush 向唯一 backing fan-out。<br><br>**生产文件：**<br>/root/github/asterinas/<br>kernel/core/comps/block/src/bio.rs<br>/root/github/asterinas/<br>kernel/core/comps/block/src/request_queue.rs<br>/root/github/asterinas/<br>kernel/core/comps/block/src/partition.rs<br>table.rs<br>/root/github/asterinas/<br>kernel/core/comps/virtio/src/device/block/device.rs<br>/root/github/asterinas/<br>kernel/core/comps/nvme/src/device/block_device.rs |
| ✅ **ext2 挂载与挂载 LV 的 remove 保护** | VFS 从 block device 解析 mount source。<br>已挂载或已打开设备不能直接删除；释放使用者后才允许删除。 | ✅ mount source 与 ext2 对象持有 tracked lease；registry 检查 lease 与 open。<br>**行为：**已挂载 LV 不可直接 remove，必须先 umount。<br><br>**生产文件：**<br>/root/github/asterinas/<br>kernel/core/src/fs/vfs/fs_apis/registry.rs<br>/root/github/asterinas/<br>kernel/core/src/fs/fs_impls/ext2/fs_type.rs<br>/root/github/asterinas/<br>kernel/core/src/fs/fs_impls/ext2/fs.rs<br>/root/github/asterinas/<br>kernel/core/comps/block/src/lib.rs<br>/root/github/asterinas/<br>kernel/core/src/device/registry/block.rs |
| ✅ **suspend、reload、resume、即时 remove** | Linux DM 管理 active/inactive table、suspend/resume、table replacement 和 mapper remove。 | ✅ 实现 drain reload、`--noflush` postponed replay、即时 remove 和失败回滚/隔离。<br><br>**生产文件：**<br>device.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/device_mapper/control.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/device_mapper/runtime.rs<br>/root/github/asterinas/<br>kernel/core/src/device/registry/block.rs |
| ✅ **显式恢复与受控重启恢复** | 恢复可分为 LVM scan/activation、文件系统重放、后端错误恢复，以及多副本/磁盘故障恢复等层次。 | ✅ 当前范围是显式 LVM scan/activation 与受控重启后的 mapper/LV 恢复。<br><br>**生产文件：**<br>device.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/device_mapper/control.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/device_mapper/runtime.rs |

## 4. 依赖绕过与当前运行模式

| 依赖或运行模式 | 标准 Linux / 发行版职责 | Asterinas 当前做法与定位 |
|---|---|---|
| **发行版自动化**<br>uevent / sysfs / udev / systemd activation | 自动发现设备。<br>应用规则、维护节点。<br>同步 LVM、自动激活卷组。 | 不实现自动化链。<br>内核直接创建 DM primary/alias。<br>用户态采用显式 scan、mknodes、activation。<br><br>**定位：**非当前目标；需要事件分发、sysfs 属性、用户态规则和服务协同。 |
| **LVM udev rules / sync** | LVM 常与 udev rule 和同步机制协同。 | topology 流程关闭 `udev_rules` 和 `udev_sync`。<br>部分 integration 流程只明确关闭 `udev_rules`。<br><br>**定位：**只按各流程的实际配置理解，不泛化为所有 LVM 路径均关闭 sync。 |
| **DM runtime node 发布** | 常见发行版可由用户态设备管理协助暴露路径。 | 内核 devtmpfs 直接发布 `/dev/dm-N` 和 `/dev/mapper/<name>`。<br><br>**定位：**当前主链必须能力；不依赖 udev 创建当前 DM 节点。 |
| **TDX attestation regression 构建** | TDX 环境需要 attestation package 与运行时设备。 | `INTEL_TDX=0` 映射为 `enableTdxAttest=false`。<br>非 TDX regression 不打包 TDX attestation。<br><br>**定位：**构建图隔离，不属于 DM 运行时功能。 |

## 5. 工程 P0：DM 主链语义收敛路线

> **目标：**以 Linux 用户可见语义收敛当前已支持的 DM 主链，而不是新增 target 或扩展通用框架。只有在源码、用户态 ABI 与对应验收层均已形成证据后，条目才可勾选完成；测试次数和具体运行记录见 [源码证据交接](device-mapper-source-evidence.md) 与 [daily log](../log/daily/)。

| 编号 | Linux 用户可见语义 | Asterinas 完成条件 | 最小验收层 | 状态 |
|---|---|---|---|---|
| P0.1 | `targets`、`create`、`info`、`table`、`deps`、`status` 能按 `dm_ioctl` header、selector 和 table slot 返回可消费结果。 | 当前 ioctl 白名单完成 ABI 编解码；active/inactive table、target count、dependency 与状态记录按选择的 slot 编码。 | core ioctl ktest、focused C、control-plane | ✅ 已完成 |
| P0.2 | `load` 后发布 primary，首次 `resume` 后发布 alias；节点冲突、发布失败和重试不留下半提交状态。 | primary/alias 发布分别由 workflow/runtime transaction 管理；失败时保留可重试的 table/identity 状态。 | core ioctl ktest、focused C、control-plane | ✅ 已完成 |
| P0.3 | `suspend`、`reload`、`resume`、`clear` 保持 active/inactive generation、drain 与 postponed I/O 的 Linux 可见边界。 | flush/no-flush suspend、running replacement、postponed replay、clear 幂等与旧 table lease 生命周期均已实现。 | DM crate ktest、core ioctl ktest、control-plane | ✅ 已完成 |
| P0.4 | `linear`、`striped`、`zero`、`error` table 能正确承接读写、range、跨 segment/chunk BIO 与 flush。 | table 连续性、target 选择、BIO split/聚合、flush fan-out 和 target status/deps 均可用。 | DM crate ktest、focused C、dataplane/linear/striped/mixed suite | ✅ 已完成 |
| P0.5 | `rename`、`remove`、`wait` 的身份、event、EINTR/SA_RESTART 与 current-instance 语义符合已核对的 Linux 实现。 | rename/setuuid/remove 仅在对应 workflow 成功提交后发布 event；wait 不持 control lock，醒来后重验 current instance。 | core ioctl ktest、focused C、control-plane | ✅ 已完成 |
| P0.6 | `DM_READONLY_FLAG` 是 table mode：readonly load 不应提前改变 active I/O，writable replacement resume 后应恢复可写。 | readonly 随 immutable table 暂存；active/inactive 查询和 BIO admission 从同一 state snapshot 获取 mode；suspend/replay 保持 mode 边界。 | DM crate ktest、core ioctl ktest、focused C、control-plane | ✅ 已完成 |
| P0.7 | 挂载或打开的 mapper 不能被错误 remove，释放使用者后可移除。 | ext2 mount source 与文件系统对象持有 tracked lease；runtime registry 同时检查 lease/open。 | core/registry ktest、focused C、control-plane | ✅ 已完成 |
| P0.8 | LVM2 能显式创建、扩缩、`pvscan → vgscan --mknodes → vgchange -ay` 激活并在受控重启后恢复当前支持的 LV。 | linear、striped、mixed LV 的显式创建、扩缩、ext2 I/O 与跨启动恢复均已覆盖。 | LVM2 topology、linear/striped/mixed suite | ✅ 已完成 |

**P0 退出条件：**P0.1～P0.8 全部勾选，且未将明确的非目标伪装为“主链收敛”缺口。下一工程优先级进入 P1 `dm-verity` 的设计确认。

**不属于 P0：**DM-on-DM/queue stacking、uevent/sysfs/udev/systemd、mount source 类型、真实 alias/VFS 深层恢复、ext4/exfat、deferred remove/完整 control ABI、VirtIO/NVMe 故障注入，以及新 target。这些项目保持各自的后续优先级或触发条件。

## 6. 后续未实现能力与优先级 ❌

| 优先级 | 未实现能力 | 用户价值、依赖与排序原因 |
|---|---|---|
| **P1** | `dm-verity` | **用户价值：**只读完整性保护 target。<br><br>**依赖：**进入前核验当前哈希能力。<br><br>**排序原因：**首个新 target；若需要大范围通用 crypto 改动，先停在设计确认。 |
| **P2** | `dm-crypt` | **用户价值：**块级加密 target。<br><br>**依赖：**cipher、密钥输入、内存清理和生命周期边界。<br><br>**排序原因：**位于 verity 后；若需要密钥服务或大范围通用安全框架，先停在设计确认。 |
| **P3** | `dm-snapshot` | **用户价值：**COW snapshot。<br><br>**依赖：**持久 metadata、恢复与空间耗尽语义。<br><br>**排序原因：**位于 crypt 后；需要显著的状态和恢复设计。 |
| **P4** | `dm-mirror` | **用户价值：**多副本、故障策略和重同步。<br><br>**依赖：**副本状态、重同步和后台工作。<br><br>**排序原因：**位于 snapshot 后；通用框架和并发/生命周期侵入最大。 |
| **P5** | ext4 / exfat 等<br>块设备文件系统兼容范围 | **用户价值：**扩大用户把 LV 用作常见块设备文件系统的场景。<br><br>**依赖：**ext4 需要 ext4 兼容能力；exfat 是否列为 DM 核心验收尚未确认；tmpfs 不以 LV/mapper 为数据 backing。<br><br>**排序原因：**不扩展 DM table/控制 ABI；ext4 需要较大文件系统能力扩展。 |
| **P6** | 后端 I/O 错误<br>断电/crash 与磁盘故障恢复 | **用户价值：**真实故障时的错误传播、可恢复性和数据可靠性。<br><br>**依赖：**真实设备错误、文件系统 crash recovery、磁盘故障与恢复策略。<br><br>**排序原因：**预计需要 driver fault-injection seam、错误处理和文件系统/持久状态改动；若确认仅需 DM 内部逻辑，应按实际改动量前移。 |
| **P7** | deferred remove<br>完整 control ABI | **用户价值：**扩大 Linux 命令和生命周期兼容面。<br><br>**依赖：**`DM_TARGET_MSG`、`DM_DEV_SET_GEOMETRY`、`DM_DEV_ARM_POLL`、IMA。<br><br>**排序原因：**需要延迟 node/manager 生命周期或额外 ABI 语义。 |
| **P8** | DM-on-DM stacking<br>完整 queue stacking | **用户价值：**mapper 作为 backing、异构 queue limit/descriptor 组合。<br><br>**依赖：**依赖图、环检测、lease/flush/remove 级联、通用 block metadata 和 driver capability。<br><br>**排序原因：**`table.rs` 当前拒绝 DmDevice backing；对 DM 与通用 block 框架改动很大。 |
| **P9** | uevent / sysfs / udev<br>systemd activation | **用户价值：**减少显式 scan、mknodes、activate 等运维操作。<br><br>**依赖：**事件分发、sysfs 属性、用户态规则与服务协同。<br><br>**排序原因：**外部生态依赖最强、非 DM 专属。 |

## 7. 用户、运维、产品与架构决策视角

> 本节是决策分析框架，不是未经调研的市场事实。

| 视角 | 当前问题与路线回应 |
|---|---|
| **用户** | **问题：**能否通过 dmsetup/LVM 完成 LV 创建、格式化、挂载、读写、扩缩、显式恢复，并在挂载期间避免误 remove。<br><br>**回应：**当前优先保持 DM 主链与 LVM 显式流程；已挂载 LV 必须先 umount 才能 remove；新 target 按 verity、crypt、snapshot、mirror 扩展。 |
| **运维** | **问题：**节点何时出现、如何显式恢复、真实后端错误或磁盘故障时能恢复到什么程度。<br><br>**回应：**当前提供内核直接节点发布与显式 scan/mknodes/activate；后端错误/crash 为 P6，磁盘故障/多副本语义由 P4 mirror 承接，自动化发行版服务为 P9。 |
| **产品** | **问题：**哪些能力能扩大 DM 可用场景，而不会把项目变成通用存储框架重写。<br><br>**回应：**先实现 target；框架专项能力只在产生明确 DM 价值时投入。 |
| **架构** | **问题：**哪些需求会迫使项目承担通用框架、后台任务、持久 metadata 或用户态生态。<br><br>**回应：**snapshot/mirror、stacking、完整 queue stacking、后端故障恢复和发行版自动化分别按 P3–P9 的改动范围排序；进入每项前单独确认。 |

## 8. 三层验证模型

| 层级 | 验证方向与边界 |
|---|---|
| **Rust ktest** | **验证方向：**target/table/state/BIO 等内核内部不变量。<br><br>**代表入口：**在 `kernel/core/comps/device-mapper` 执行 `cargo osdk test`。<br><br>**不负责：**raw ioctl ABI、devtmpfs path、LVM2 编排、文件系统用户态行为。 |
| **Initramfs C 回归** | **验证方向：**raw `open/ioctl/mount/devtmpfs/errno` 用户态 ABI。<br><br>**代表入口：**`REGRESSION_TESTS=device/device_mapper`。<br><br>**不负责：**Rust 内部并发组合、target 数据面、LVM2 编排。 |
| **NixOS DM suite** | **验证方向：**dmsetup/LVM2、实盘 I/O、PV/VG/LV、文件系统和恢复。<br><br>**代表入口：**`myshell/run_dm_system_tests.sh --<suite>`。<br><br>**不负责：**内核内部失败分支、每个 ioctl buffer 边界、driver fault injection。 |

默认选择规则：DM crate 改动先运行完整 DM ktest，再按观察面追加 C 回归或 NixOS suite。为承接现有 DM 功能而改动框架时，不默认新增框架专项测试；只有框架问题造成 DM 可见失败、数据风险或阻塞新 target 时，才确认最小所有者范围并补定向测试或用户态回归。

## 9. 维护规则

1. 本文只维护功能、依赖、实现方式、未实现原因和优先级；不记录测试次数、通过结果或每日命令输出。
2. 测试命令和结果维护在 `docs/test.md`、`docs/review.md` 和 `log/daily/YYYY-M-D.md`。
3. 新 target 或重新开启框架专项工作前，先说明原来行为、目标行为、示例差异、依赖和验收；未经确认不开始实现。
4. 任何无法由源码或已确认项目决策支撑的内容，先询问项目负责人。
5. 不默认提交或 push；通过小阶段验证后由项目负责人决定是否提交。

## 10. 修复日志

> 仅记录已完成并验证的修复；完整命令、失败输出和排障过程见 [daily log](../log/daily/)。

| 日期 | 问题与影响 | 根因 | 修复 | 验证 | 提交状态 |
|---|---|---|---|---|---|
| 2026-09-24 | mapper alias 删除返回非 `ENOENT`/`ESTALE` 错误后恢复 `Live` 时丢失 `DevtmpfsHandle`；后续 rename 失败，重试 remove 可能遗留 `/dev/mapper/<name>`。 | `unregister_mapper` 在删除成功前从 runtime registry 取走 alias handle；按值 `delete` 失败不返还该 handle。 | `devtmpfs::delete` 改为借用 handle；registry 只在 identity 删除成功后清除 primary/alias 记录，并为定向测试提供删除故障注入。 | `cargo fmt --check --all`、`git diff --check` 通过；标准 runtime registry ktest 8/0、devtmpfs ktest 6/0。 | `0423431d5` |
| 2026-09-24 | deferred replay 的 split child 在失败后可能双重递减聚合计数，导致父 BIO 提前完成；后续 child 结束时 release 下可下溢并破坏完成生命周期。 | child 继承 drop-to-`IoError` 后，`DmTable` 仍通过未绑定 child 的 completion handle 手动补完成。 | 移除 completion handle；每个 split child 无条件承担未完成即 `IoError` 的完成责任，table 不再手动递减。 | `cargo fmt --check --all`、`git diff --check` 通过；block crate ktest 25/0、DM crate ktest 87/0，含 deferred replay 跨 target 失败回归。 | 本提交 |
| 2026-09-28 | 上游同步移除私有 VirtIO `GET_ID` ioctl 后，DM NixOS suite 无法按 host serial 定位 backing disk，linear integration 在首台 guest 失败。 | guest `aster-dm-disk-locator` 仍枚举 `/dev/vd*` 并调用已移除的私有 ioctl；现有 VirtIO transport 没有安全恢复旧初始化查询的 timeout/reset 生命周期。 | 固定 NixOS QEMU block PCI 拓扑，以 cmdline `aster.test_disk_first` 声明首块测试盘，并以通用 `aster-test-disk-locator <序号>` 推导且验证 guest block path。 | `make nixos`、静态 shell/Nix 检查通过；DM/block/core ktest、focused C ABI 与六项 DM NixOS suite 在 `fork_Asterinas` 当前 `dm` SHA 通过。 | 未提交 |
