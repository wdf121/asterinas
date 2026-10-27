# Asterinas Device Mapper 全局规划

> **2026-10-27 当前卡点：PR4 已验证，待拆分。** 完整 dm 修复已提交至 `7de7025ad`、`74ee6b89e`；当前 dm HEAD 为 `3cd8af8b7`。PR2 `074e7e80d`、枚举 PR3 `be3f79c88`、无表管理 PR4 `6995e5ab3` 均已推送。PR4 助手 format/check 通过，用户确认包括 NixOS 的测试全部通过；下一步讨论拆分，不是继续等待提取或验收。
>
> **路线编号**：插入枚举 PR3 后，原无表管理及后续框架顺延为 PR4–PR12，最小 linear 暂为 PR13。PR4 拆分后再调整。依赖与范围以 [PR 路线](pull_request.md) 为准；本页 P0/P1～P10 是历史能力分类，不是 PR 编号。
>
> PR 分支的枚举、目录整理和新套件尚未自动回补 dm。逐项台账见 [10.4 节](#104-pr3-制备前修复与待办总账2026-10-27)，实际过程及证据边界见 [今日日志](../log/daily/2026-10-27.md)。当前只更新文档，不修改生产代码或推送。

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
| ✅ **`/dev/mapper/control`**<br>用户态进入 DM 控制面 | Linux DM 提供 control character device。<br>`dmsetup`/libdevmapper 通过 `dm_ioctl` 管理 mapper。 | ✅ 注册 control device、解析 `dm_ioctl`、校验 version/flags/buffer，并实现当前命令白名单。<br><br>**生产文件：**<br>/root/github/asterinas/<br>kernel/core/src/device/misc/dm_control.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/mod.rs |
| ✅ **运行期 mapper 节点**<br>`/dev/dm-N`、`/dev/mapper/<name>` | Linux DM 提供 mapped block device 与 mapper name 路径。 | ✅ 首次 load 发布 primary；首次 resume 发布 alias；rename/remove 使用事务、回滚与隔离。<br><br>**生产文件：**<br>/root/github/asterinas/<br>kernel/core/src/device/registry/block.rs<br>/root/github/asterinas/<br>kernel/core/src/device/mod.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/dm_control/runtime.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/dm_control/workflow.rs<br>manager.rs |
| ✅ **LVM2 显式创建、扩缩与恢复** | LVM2 计算 PV/VG/LV metadata，并通过 DM control ABI 创建或调整 table。 | ✅ 承接 linear、striped、mixed LV 的 create/extend/reduce，以及显式 scan/mknodes/activate。<br><br>**生产文件：**<br>/root/github/asterinas/<br>kernel/core/src/device/misc/dm_control.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/dm_control/workflow.rs<br>table.rs<br>target/linear.rs<br>target/striped.rs |
| ✅ **当前 table 与 target**<br>linear / striped / zero / error | Linux DM target table 描述逻辑 sector 到 backing device 的映射，并支持 target 扩展。 | ✅ 实现四个 target、连续 table、capacity、deps、status 和 target I/O action。<br><br>**生产文件：**<br>table.rs<br>target/mod.rs<br>target/linear.rs<br>target/striped.rs<br>target/zero.rs<br>target/error.rs |
| ✅ **table-scoped readonly**<br>`DM_READONLY_FLAG` | Linux 将 readonly 作为 table mode：load 只暂存 inactive table；首次或 replacement resume 激活 table 后，live device 的可写性随其切换。 | ✅ readonly 存在 immutable `DmTable`，tableless create 不持久化 flag；load 不改变 active I/O mode；active/inactive 查询与 BIO 分派均从同一 table state snapshot 获取 mode。readonly active 在 suspend 中拒绝新 write-like BIO；已在 writable mode 接收的 postponed BIO 遇到 readonly replacement 时完成为 I/O error。<br><br>**生产文件：**<br>table.rs<br>device.rs<br>manager.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/dm_control.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/dm_control/workflow.rs |
| ✅ **写入、跨 segment/chunk I/O 与 flush** | Linux block layer 支持 remap、split、completion 和 flush。 | ✅ BIO 保存 original/current range；DM 规划边界后一次 split，聚合子 BIO；flush 向唯一 backing fan-out。<br><br>**生产文件：**<br>/root/github/asterinas/<br>kernel/core/comps/block/src/bio.rs<br>/root/github/asterinas/<br>kernel/core/comps/block/src/request_queue.rs<br>/root/github/asterinas/<br>kernel/core/comps/block/src/partition.rs<br>table.rs<br>/root/github/asterinas/<br>kernel/core/comps/virtio/src/device/block/device.rs<br>/root/github/asterinas/<br>kernel/core/comps/nvme/src/device/block_device.rs |
| ✅ **ext2 挂载与挂载 LV 的 remove 保护** | VFS 从 block device 解析 mount source。<br>已挂载或已打开设备不能直接删除；释放使用者后才允许删除。 | ✅ mount source 与 ext2 对象持有 tracked lease；registry 检查 lease 与 open。<br>**行为：**已挂载 LV 不可直接 remove，必须先 umount。<br><br>**生产文件：**<br>/root/github/asterinas/<br>kernel/core/src/fs/vfs/fs_apis/registry.rs<br>/root/github/asterinas/<br>kernel/core/src/fs/fs_impls/ext2/fs_type.rs<br>/root/github/asterinas/<br>kernel/core/src/fs/fs_impls/ext2/fs.rs<br>/root/github/asterinas/<br>kernel/core/comps/block/src/lib.rs<br>/root/github/asterinas/<br>kernel/core/src/device/registry/block.rs |
| ✅ **suspend、reload、resume、即时 remove** | Linux DM 管理 active/inactive table、suspend/resume、table replacement 和 mapper remove。 | ✅ 实现 drain reload、`--noflush` postponed replay、即时 remove 和失败回滚/隔离。<br><br>**生产文件：**<br>device.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/dm_control/workflow.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/dm_control/runtime.rs<br>/root/github/asterinas/<br>kernel/core/src/device/registry/block.rs |
| ✅ **显式恢复与受控重启恢复** | 恢复可分为 LVM scan/activation、文件系统重放、后端错误恢复，以及多副本/磁盘故障恢复等层次。 | ✅ 当前范围是显式 LVM scan/activation 与受控重启后的 mapper/LV 恢复。<br><br>**生产文件：**<br>device.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/dm_control/workflow.rs<br>/root/github/asterinas/<br>kernel/core/src/device/misc/dm_control/runtime.rs |

**控制事件边界（2026-10-27）：**已实现全局事件计数、poll 等待通知、每次 open 的事件基准及 `DM_DEV_ARM_POLL`，dup 共享同一文件状态；成功命令事件在结果 copyout 前提交。单设备 `DM_DEV_WAIT`、全局 control poll 与设备 uevent 是三套不同机制：前两者已有实现，DM uevent/cookie 与 udev 同步仍未实现，详见 10.4 节。

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
| P0.1 | `targets`、`create`、`info`、`table`、`deps`、`status` 能按 `dm_ioctl` header、selector 和 table slot 返回可消费结果。 | 当前 ioctl 白名单完成 ABI 编解码；active/inactive table、target count、dependency 与状态记录按选择的 slot 编码。 | core ioctl ktest、focused C、control-plane | ⚠ 主链已有，V5 语义修复重新开放 |
| P0.2 | `load` 后发布 primary，首次 `resume` 后发布 alias；节点冲突、发布失败和重试不留下半提交状态。 | primary/alias 发布分别由 workflow/runtime transaction 管理；失败时保留可重试的 table/identity 状态。 | core ioctl ktest、focused C、control-plane | ✅ 已完成 |
| P0.3 | `suspend`、`reload`、`resume`、`clear` 保持 active/inactive generation、drain 与 postponed I/O 的 Linux 可见边界。 | flush/no-flush suspend、running replacement、postponed replay、clear 幂等与旧 table lease 生命周期均已实现。 | DM crate ktest、core ioctl ktest、control-plane | ⚠ 主链已有，V5 语义修复重新开放 |
| P0.4 | `linear`、`striped`、`zero`、`error` table 能正确承接读写、range、跨 segment/chunk BIO 与 flush。 | table 连续性、target 选择、BIO split/聚合、flush fan-out 和 target status/deps 均可用。 | DM crate ktest、focused C、dataplane/linear/striped/mixed suite | ✅ 已完成 |
| P0.5 | `rename`、`remove`、`wait` 的身份、event、EINTR/SA_RESTART 与 current-instance 语义符合已核对的 Linux 实现。 | rename/setuuid/remove 仅在对应 workflow 成功提交后发布 event；wait 不持 control lock；已开始 WAIT 的引用保留和删除唤醒语义按 V5.L05 修复，不沿用 mutation 的 current-instance 检查。 | core ioctl ktest、focused C、control-plane | ⚠ 主链已有，V5 语义修复重新开放 |
| P0.6 | `DM_READONLY_FLAG` 是 table mode：readonly load 不应提前改变 active I/O，writable replacement resume 后应恢复可写。 | readonly 随 immutable table 暂存；active/inactive 查询和 BIO admission 从同一 state snapshot 获取 mode；suspend/replay 保持 mode 边界。 | DM crate ktest、core ioctl ktest、focused C、control-plane | ✅ 已完成 |
| P0.7 | 挂载或打开的 mapper 不能被错误 remove，释放使用者后可移除。 | ext2 mount source 与文件系统对象持有 tracked lease；runtime registry 同时检查 lease/open。 | core/registry ktest、focused C、control-plane | ✅ 已完成 |
| P0.8 | LVM2 能显式创建、扩缩、`pvscan → vgscan --mknodes → vgchange -ay` 激活并在受控重启后恢复当前支持的 LV。 | linear、striped、mixed LV 的显式创建、扩缩、ext2 I/O 与跨启动恢复均已覆盖。 | LVM2 topology、linear/striped/mixed suite | ✅ 已完成 |

**P0 退出条件：**P0.1～P0.8 对应的现有主链语义完成验收。第五版源码复核重新开放了 P0.1、P0.3、P0.5；历史主链测试成功不等于这些细节已对齐 Linux。这是 10 月 16 日的历史优先级；当前规划按顶部 10 月 26 日路线推进，具体缺口实现仍须逐项审定。

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
| **P7** | deferred remove<br>完整 control ABI | **用户价值：**扩大 Linux 命令和生命周期兼容面。<br><br>**依赖：**`DM_TARGET_MSG`、`DM_DEV_SET_GEOMETRY`、IMA 及延迟删除通知；`DM_DEV_ARM_POLL` 已实现，不再计入未实现项。<br><br>**排序原因：**需要延迟 node/manager 生命周期或额外 ABI 语义。 |
| **P8** | DM-on-DM stacking<br>完整 queue stacking | **用户价值：**mapper 作为 backing、异构 queue limit/descriptor 组合。<br><br>**依赖：**依赖图、环检测、lease/flush/remove 级联、通用 block metadata 和 driver capability。<br><br>**排序原因：**`table.rs` 当前拒绝 DmDevice backing；对 DM 与通用 block 框架改动很大。 |
| **P9** | uevent / sysfs / udev<br>systemd activation | **用户价值：**减少显式 scan、mknodes、activate 等运维操作。<br><br>**依赖：**事件分发、sysfs 属性、用户态规则与服务协同。<br><br>**排序原因：**外部生态依赖最强、非 DM 专属。 |
| **P10** | 通用物理 block-device 热插拔<br>运行期 VirtIO/PCI 插盘与真实 USB mass-storage | **用户价值：**系统运行中发现、发布并安全移除物理存储设备。<br><br>**依赖：**总线 add/remove 与 driver probe/unbind、运行期 block registry/devtmpfs 主盘和分区节点事务、I/O worker 生命周期、in-flight I/O 失败收束、open/mount/lease/cache/DM backing 的移除协调；真实 USB 还需要 xHCI 与 mass-storage 驱动。完成上述内核链路后，可选接入 sysfs/NETLINK_KOBJECT_UEVENT/udev。<br><br>**排序原因：**当前启动期 VirtIO/NVMe 枚举和 DM runtime node 已可用，但缺少通用运行期 add/remove 链路；该能力跨总线、driver、VFS 和用户态生态，非 DM 主链所需，置于当前路线最低优先级。 |

### 6.1 第五版暂缓的框架与能力

2026-10-16 决策：先修当前支持功能，以下仅规划，不实施。

| 来源 | 后续能力 | 具体依赖与当前边界 |
|---|---|---|
| V5.F01 | 默认 suspend 的文件系统 freeze/sync/thaw | 需要按 backing 定位文件系统、阻止新写入、同步及失败撤销；当前仅 BIO drain 不等于冻结。NOFLUSH 排空修复仍属当前工作，不随本项暂缓。 |
| V5.B02 | DM 专属 sysfs、uevent/cookie、udev 同步 | 已有 block sysfs 接线与 netlink uevent 基础设施，缺 DM 属性和生命周期发送/用户态同步链；并入 P9，不称为整个框架不存在。 |
| V5.B02 | 新控制命令、deferred remove、IMA、DM-on-DM | 分别沿 P7/P8 规划，不通过本轮 ABI 修复隐式开启。 |
| V5.A06 部分 | 非 UTF-8 设备名称与路径 | 若需要改造通用 String/VFS 表示，单独设计；本轮仅修可独立完成的归一、无关字段校验，不宣称任意字节路径兼容。 |

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
| **Initramfs C 回归** | **验证方向：**raw `open/ioctl/mount/devtmpfs/errno` 用户态 ABI。<br><br>**代表入口：**`REGRESSION_TESTS="device/device_mapper_control device/device_mapper_lifecycle device/device_mapper_table"`。<br><br>**不负责：**Rust 内部并发组合、target 数据面、LVM2 编排。 |
| **NixOS DM suite** | **验证方向：**dmsetup/LVM2、实盘 I/O、PV/VG/LV、文件系统和恢复。<br><br>**代表入口：**`myshell/run_dm_system_tests.sh --<suite>`。<br><br>**不负责：**内核内部失败分支、每个 ioctl buffer 边界、driver fault injection。 |

默认选择规则：最终验收以 `make check`、`make kernel` 与相关定向 ktest 为基础，不默认完整 `make ktest`；涉及用户态 ABI 加 C 回归，涉及工具流程再按影响选择系统套件。影响面难以限定时先讨论扩大范围。记录实际匹配数与退出码，零匹配不算通过。“回归”是测试目的，不是另一种执行位置。

## 9. 维护规则

1. 本文维护功能、依赖、实现方式、未实现原因和优先级；PR3 制备发现的每项修复和待办必须登记在修复总账，记录当前状态、卡点、验证证据类别和提交状态，详细命令与输出保留在日报。
2. 测试命令和结果维护在 `docs/test.md`、`docs/review.md` 和 `log/daily/YYYY-M-D.md`。
3. 新 target 或重新开启框架专项工作前，先说明原来行为、目标行为、示例差异、依赖和验收；未经确认不开始实现。
4. 任何无法由源码或已确认项目决策支撑的内容，先询问项目负责人。
5. 不默认提交或 push；通过小阶段验证后由项目负责人决定是否提交。

## 10. 修复日志

> 历史阶段：2026-10-19 完成 PR 审查收益的局部 dm 回补与定向验证，代码已分点本地提交，未推送；证据见[当日日志](../log/daily/2026-10-19.md)。V5 历史收尾见 [10 月 16 日](../log/daily/2026-10-16.md)。根盘问题继续待定，不重启通用文件系统排障。

### 10.1 第五版：已批准的当前修复

2026-10-16 用户确认：现有功能语义修复、输入容量策略和错误码细化可实施；新增框架/功能暂缓。批次完成后集中测试，不逐项启动整套测试，不自动提交或 push。本表已在阶段收尾时更新；已实现不代表完整 Linux DM 生态兼容。

| 编号 | 修复目标 | 收尾状态 |
|---|---|---|
| V5.00 | 固定头单次快照 | 已实现并纳入本阶段验证，未提交 |
| V5.A01 | 按 type/nr 解码命令 | 已实施，本阶段收尾 |
| V5.A02 | 305 字节固定前缀、NO_PARAMS、命令布局 | 已实施，本阶段收尾 |
| V5.A03 | 有效输出长度及失败回写 | 已实施，本阶段收尾 |
| V5.A04 | flags 规范化与删除私有 EXISTS 位 | 已实施，本阶段收尾 |
| V5.A05 | 互斥 selector 与校验顺序 | 已实施，本阶段收尾 |
| V5.A06 | 固定字符串归一及诊断不干扰语义；字节路径扩展暂缓 | 已实施，本阶段收尾 |
| V5.A07 | 名称列表事件字段、过滤及不足/空输出 | 已实施，本阶段收尾 |
| V5.A08 | WAIT 状态记录与查询 flags | 已实施，本阶段收尾 |
| V5.A09 | 公共状态 target_count | 已实施，本阶段收尾 |
| V5.A10 | target-version 列表容量契约 | 已实施，本阶段收尾 |
| V5.A11 | 末 target next 消费边界 | 已实施，本阶段收尾 |
| V5.A12 | 参数转义及相对 backing 路径 | 已实施，本阶段收尾 |
| V5.S01 | 清理用户安全输入且不回写原参数 | 已实施，本阶段收尾 |
| V5.S02 | 错误路径及临时副本的安全清理 | 已实施，本阶段收尾 |
| V5.L01 | NOFLUSH 仍排空已分配 BIO | 已实施，本阶段收尾 |
| V5.L02 | 可中断 drain 与状态回滚 | 已实施，本阶段收尾 |
| V5.L03 | UUID 只允许首次设置 | 已实施，本阶段收尾 |
| V5.L04 | 名称/UUID 提交前校验 | 已实施，本阶段收尾 |
| V5.L05 | 已开始 WAIT 的删除唤醒语义 | 已实施，本阶段收尾 |
| V5.L06 | 按 live table 条件发布事件 | 已实施，本阶段收尾 |
| V5.L07 | Running 空表 resume 幂等 | 已实施，本阶段收尾 |
| V5.L08 | REMOVE_ALL 有进展时重扫 | 已实施，本阶段收尾 |
| V5.B01 | 输入容量策略、可失败分配及边界 | 已实施，本阶段收尾 |
| V5.B03 | 按命令细化 errno 与检查顺序 | 已实施，本阶段收尾 |
| V5.M01 | 类型表达生命周期持锁/实例约束 | 已实施，本阶段收尾 |
| V5.M02 | 统一 rename 事件责任，与 L06 同修 | 已实施，本阶段收尾 |
| V5.M03 | 移除或落实无效字段标签参数 | 已实施，本阶段收尾 |
| V5.D01 | 按最终实际能力补兼容文档/SCML | docs 能力边界本次更新；book/SCML 未修改，作为后续文档项保留 |
| V5.D02 | 纠正 selector 注释 | 已实施，本阶段收尾 |
| V5.D03 | 纠正列表布局注释 | 已实施，本阶段收尾 |
| V5.D04 | 移除跨层 rename 原子性过度承诺 | 已实施，本阶段收尾 |

### 10.2 已验证修复历史

以下保留当时结果；涉及第五版重新发现的差异时，以新清单的收尾状态与暂缓边界为准，不抹除历史执行事实。

| 日期 | 问题与影响 | 根因 | 修复 | 验证 | 提交状态 |
|---|---|---|---|---|---|
| 2026-09-24 | mapper alias 删除返回非 `ENOENT`/`ESTALE` 错误后恢复 `Live` 时丢失 `DevtmpfsHandle`；后续 rename 失败，重试 remove 可能遗留 `/dev/mapper/<name>`。 | `unregister_mapper` 在删除成功前从 runtime registry 取走 alias handle；按值 `delete` 失败不返还该 handle。 | `devtmpfs::delete` 改为借用 handle；registry 只在 identity 删除成功后清除 primary/alias 记录，并为定向测试提供删除故障注入。 | `cargo fmt --check --all`、`git diff --check` 通过；标准 runtime registry ktest 8/0、devtmpfs ktest 6/0。 | `0423431d5` |
| 2026-09-24 | deferred replay 的 split child 在失败后可能双重递减聚合计数，导致父 BIO 提前完成；后续 child 结束时 release 下可下溢并破坏完成生命周期。 | child 继承 drop-to-`IoError` 后，`DmTable` 仍通过未绑定 child 的 completion handle 手动补完成。 | 移除 completion handle；每个 split child 无条件承担未完成即 `IoError` 的完成责任，table 不再手动递减。 | `cargo fmt --check --all`、`git diff --check` 通过；block crate ktest 25/0、DM crate ktest 87/0，含 deferred replay 跨 target 失败回归。 | 本提交 |
| 2026-09-28 | 上游同步移除私有 VirtIO `GET_ID` ioctl 后，DM NixOS suite 无法按 host serial 定位 backing disk，linear integration 在首台 guest 失败。 | guest `aster-dm-disk-locator` 仍枚举 `/dev/vd*` 并调用已移除的私有 ioctl；现有 VirtIO transport 没有安全恢复旧初始化查询的 timeout/reset 生命周期。 | 固定 NixOS QEMU block PCI 拓扑，以 cmdline `aster.test_disk_first` 声明首块测试盘，并以通用 `aster-test-disk-locator <序号>` 推导且验证 guest block path。 | `make nixos`、静态 shell/Nix 检查通过；DM/block/core ktest、focused C ABI 与六项 DM NixOS suite 在 `fork_Asterinas` 当前 `dm` SHA 通过。 | `c6ab793bf` |
| 2026-09-29 | rootfs 通过裸 block device 名称 lookup 构造 untracked lease，已挂载 rootfs 可能不阻止 backing device 注销；名称 lookup 还会在设备 `name()` 重入 registry 时自锁。 | `lookup_by_name` 在 registry/entry-state 锁域调用公开 trait callback，且 `FsCreationCtx::from_block_device` 将裸 `Arc` 包装为 `new_untracked`。 | 将名称查找改为锁内快照、锁外回调、命中后重新验证；新增 `lookup_lease_by_name`，rootfs 与预解析 filesystem context 仅传递 tracked `BlockDeviceLease`。 | `cargo fmt --check`、`git diff --check`；block registry ktest 8/0，core VFS context、rootfs、ext2 backing-lease ktest 均为 1/0。 | `98eac8346` |
| 2026-10-10 | `make check` 因历史日志类型名拼写错误失败，并在合法跨 target 同名依赖上产生重复包警告。 | 日志拼写未通过 typos；cargo-udeps 0.1.61 对同一包 ID 的重复插入无条件警告，未区分同名声明与不同别名。 | 修正日志类型名；保留源码补丁，仅在同包 ID 对应不同依赖名时警告，并将修补版安装到现有开发容器。 | 四项原版/修补版 used/unused 回归通过，真实未使用依赖仍返回 1；临时 PATH 与替换默认工具后的完整 `make check` 均退出 0，日志无警告或错误。OSDK 安装命令的模拟测试及后续真实 `make install_osdk` 均通过，安装退出 0，日志无工具链覆盖警告。 | 补丁与拼写修正见 `82337349a`；工具安装仅作用于现有容器 |
| 2026-10-10 | `make kernel` 同时引入 dm 与旧 main 基线目录中的 align_ext 0.1.0，发生 lockfile package collision。 | worktree 共享容器的 cargo-osdk 安装位置；基线来源的本地开发工具嵌入旧源码路径，构建包装清单混用两棵源码树，时间戳更新规则未识别来源变化。 | 从 `/root/asterinas` 重新安装 OSDK，绑定回当前 dm；由工具重新生成包装清单，不手工删除锁文件或改动内核源码。 | 安装退出 0；安装元数据、二进制路径及生成依赖清单断言通过；`make kernel` 退出 0 并生成 ISO。仍有 ermsb 未稳定支持及 GRUB locale 目录缺失警告；未做运行时测试。 | 环境修复完成；`install_osdk` 改动见 `acd793859` |
| 2026-10-10 | LVM2 的 `dmsetup ls --uuid` 类查询被拒，`--linear-integration` 首层失败。 | `DM_UUID_FLAG` 仅允许 `DM_DEV_RENAME_CMD` 携带，`DM_LIST_DEVICES_CMD` 返回 `EINVAL`，且不输出扩展记录布局。 | 放宽为 rename 与 list 均可携带该标志；`DM_LIST_DEVICES_CMD` 按标志输出紧凑/扩展两种记录布局。 | 当日套件仍因 sysfs 第二层根因失败；修复正确性由 2026-10-12 `--linear-integration` 全套通过覆盖验证。 | `3615118bb` |
| 2026-10-10 | 新建 release NixOS 根盘有序关机后 `e2fsck -fn` 返回 4：块组 0 空闲块计数与位图统计不一致。 | ext2 `sync_all` 逐组同步后立即持久化计数，其后的 inode 写回仍可能改变其它组的分配状态，落盘计数与位图脱节。 | 最终元数据遍历持有超级块写锁冻结分配，锁内刷新各组位图与描述符快照后统一持久化计数；新增 4 个确定性 ktest 用例先复现失败再转绿。 | ext2 模块 ktest 15/0；release 根盘端到端复验（启动前后 `e2fsck -fn` 均 0）；`--control-plane` 套件复验通过。 | `65c650435` |
| 2026-10-12 | upstream 设备模型创建空 `/sys/dev/block`，LVM2 sysfs filter 拒绝一切块设备，`--linear-integration` 第二层失败。 | 块设备从未注册进设备模型；块类 `/dev` 节点由 registry 自行发布，设备模型无法感知。 | 引入 `CREATES_DEVNODE` 类属性 opt-out，`SelfPublishingBlock` 将块设备接入设备模型：opt-out 设备仍获 `/sys/dev/<kind>/<major>:<minor>` 索引项，模型不创建、不在 teardown 时删除其节点。 | core DM ioctl ktest 81/0；`--linear-integration` 完整套件五次真实运行全部 PASS。 | `535727967` |
| 2026-10-19 | 控制 fd 无事件时恒报 IN/OUT，可能让用户态事件循环持续立即返回。 | poll 直接返回请求 mask 中的 IN/OUT，未关联事件状态。 | 暂无全局事件支持时返回空，补内核及 C ABI 测试；不宣称完整 Linux poll/ARM_POLL 支持。 | 完整 check、control ioctl 定向 ktest 通过，新增 poll 实际执行；用户反馈 kernel/guest ABI 通过，后两项未独立核验日志。详见当日日志。 | `981294ef5`，未推送 |

### 10.4 PR3 制备前修复与待办总账（2026-10-27）

本节标题和 `PR3-Fxx` / `PR3-Txx` 保留为历史稳定索引，避免破坏引用；其中的 PR3 是当时的无表管理，现已顺延为 PR4，不是当前 `/proc/devices` 枚举 PR3。
无表范围仍为 CREATE、STATUS、LIST_DEVICES、单设备 REMOVE、必要参数及身份管理；不包含 table/target/BIO、runtime 发布、rename、REMOVE_ALL。

**当前状态与证据：**完整 dm 修复已在 `7de7025ad`、`74ee6b89e` 提交，当前 HEAD `3cd8af8b7`。下表源码链接仍指完整 dm，不随 PR 分支文件重命名而修改。
无表子集已经提取为 PR4 `6995e5ab3`，助手完整 format/check 通过，用户确认 Rust/C/NixOS 测试全部通过，已提交推送，当前待拆分。
Linux 仍固定参考 `5078a3a23a1e1825ec136485173ec98668cdd640` 的 Git 版本。用户运行结果无完整逐项日志/计数，不虚构；完整 dm 的事件能力没有自动进入 PR4。

#### 已修复项

| 编号 | 原问题与影响 | 修复与当前源码证据 | 验证及 PR3 关系 |
|---|---|---|---|
| PR3-F01 | CREATE 重复 name/UUID 返回 EEXIST，固定 Linux 为 EBUSY。 | [abi.rs](../kernel/core/src/device/misc/dm_control/abi.rs) 的 `map_create_error` 专用映射；保留 minor 分配优先及失败释放。 | 用户反馈通过；Rust/C 冲突与优先级测试；PR3 直接相关。 |
| PR3-F02 | ioctl dev 使用通用扩展 64 位解码，未忽略高 32 位。 | 同文件 `decode_ioctl_device` 在 DM ABI 内实现低 32 位解码；CREATE 只取 minor，未修改通用 DeviceId。 | 用户反馈通过；高位扰动和 persistent minor 回归；PR3 直接相关。 |
| PR3-F03 | C 请求依赖 Nix 新头文件的 4.50.0，与固定 ABI 4.48.0 不一致。 | [device_mapper_common.h](../test/initramfs/src/regression/device/device_mapper_common.h) 固定请求和返回版本断言，三个 C 程序统一使用。 | 用户反馈 C 回归通过；PR3 提取时保留固定基线，不能依宿主头文件漂移。 |
| PR3-F04 | 1 MiB 策略错误拒绝 Linux 可接受的 data_size，包括 NO_PARAMS 声明。 | `DM_IOCTL_MAX_SIZE` 改为 1 GiB；[ioctl.rs](../kernel/core/src/device/misc/dm_control/ioctl.rs) 保持 NO_PARAMS 仅处理 305 字节固定前缀。 | 用户反馈通过；边界测试不等于实际成功分配 1 GiB，资源限制见 T05。 |
| PR3-F05 | UUID 被当作 UTF-8，合法非 UTF-8 身份无法 roundtrip。 | [abi.rs](../kernel/core/src/device/misc/dm_control/abi.rs) `optional_uuid/c_bytes_until`、manager/device 的 `Vec<u8>`、查询/设置 UUID/LIST 输出与前缀过滤按 bytes；name 仍 String。 | 用户反馈通过；opaque UUID Rust/C 测试。PR3 提取身份部分，不引入 rename。 |
| PR3-F06 | 全局 control poll/ARM_POLL 缺失；已有 WAIT 不足以覆盖。 | [dm_control.rs](../kernel/core/src/device/misc/dm_control.rs) 全局 generation、Pollee、per-open 基准、dup 共享；ARM_POLL 确认事件。 | 用户反馈通过；Rust/C 事件与 ARM 测试。完整 dm 已实现，PR3 如何拆分见 T06。 |
| PR3-F07 | 成功命令与 copyout 失败的全局事件边界缺少实现/覆盖。 | 成功 CREATE/REMOVE/RENAME/REMOVE_ALL 在 copyout 前通知；ARM 即使结果回写 EFAULT，确认仍已生效。活表 rename/remove、激活接事件。 | 用户反馈通过；新增 ARM copyout 故障测试。精确 bind 提交点专项证据见 T07；不等同 uevent。 |
| PR3-F08 | REMOVE 返回时未显式等待已准入控制操作，活动表释放边界依赖 Arc。 | [manager.rs](../kernel/core/comps/device-mapper/src/manager.rs) `acquire_control/create_control` 与索引锁配合；[device.rs](../kernel/core/comps/device-mapper/src/device.rs) `DmControlRef/drain_removed`；删除在 lifecycle 锁外等待控制引用和已分配 BIO。 | 用户反馈通过；WAIT/remove 保留旧表与排空测试；PR3 保留控制引用原则，不搬入 BIO。 |
| PR3-F09 | 排空初稿仍可能经旧 raw Arc 接收新 BIO，导致删除边界不闭合。 | `stop_removed_io` 在成功 runtime 注销后永久关闭 admission，失败 postponed BIO，保留已分配 BIO 正常完成；失败注销不执行该提交。 | 用户反馈通过；新增拒绝新 BIO、等待 holder/BIO 测试。属于完整 dm 修复，不进入无表 PR3 数据面。 |
| PR3-F10 | REMOVE 成功后结构性旧 Arc 仍占 minor，立即指定同 minor 创建可能失败。 | `DmDeviceIdOwner` 的编号占用与不可变 ID 分离；drain 后显式一次性 release，Drop 保留兜底。旧对象再次清理/析构不得释放新分配。 | 用户反馈通过；同名同 minor 复用、旧实例拒绝准入、重复清理测试；PR3 直接相关。 |
| PR3-F11 | minor 提前复用后，CREATE/LIST/查询若只保留 raw Arc，可能按旧 ID 读取新注册对象。 | CREATE 发布前获得控制引用；LIST 在查询注册状态及快照期间持引用；selector 的注册状态检查持引用；mutation 仍按实例复核，非仅按 name/id。 | 用户反馈通过；旧实例/编号复用回归。不是允许旧引用继续操作新设备。 |
| PR3-F12 | 最新 make check 因测试辅助函数 dead-code 和泛型实参多余借用失败。 | [workflow.rs](../kernel/core/src/device/misc/dm_control/workflow.rs) `QueryWorkflow::devices` 标为 `cfg(ktest)`；[commands.rs](../kernel/core/src/device/misc/dm_control/commands.rs) 去掉临时 mapper_name 的多余借用；未压制 lint。 | 失败实际输出为 Error 101；修复后用户确认全部验证通过。测试内另三处同类借用仅只读检查，未擅自修改。 |

#### 待修复、暂缓与待核实项

以下逐项保留，不以主链测试通过销项；“本轮不做”不是永久不支持。需要其他子系统改动或新增子系统的项目，按用户决定止步，后续必须重新授权。

| 编号 | 项目与当前状态 | 卡点、影响与当前证据 | 后续条件及验证方向 |
|---|---|---|---|
| PR3-T01 | deferred REMOVE：未实现，本轮因跨子系统边界暂缓。 | REMOVE/REMOVE_ALL 的 DEFERRED flag 仍返回 EOPNOTSUPP。Linux 空闲设备可立即删除，忙设备标记后最后关闭触发工作；当前 [block registry](../kernel/core/src/device/registry/block.rs) 的 OpenBlockFile Drop 和 [block component](../kernel/core/comps/block/src/lib.rs) 的 lease Drop 只递减计数，缺 DM 可用的最后释放通知。 | 另行审定通知、后台执行、重新阻止打开、标记/取消与 REMOVE_ALL 语义；不能只加 flag 或用 DM 轮询伪装支持。 |
| PR3-T02 | DM uevent/cookie、udev 同步：未实现，本轮暂缓。 | 已有 block sysfs 接线和 netlink 组播基础；[UeventMessage](../kernel/core/src/net/socket/netlink/kobject_uevent/message/mod.rs) 构造接口私有，缺 DM 可用的设备生命周期发送接口。控制 poll 不是 uevent，不设置虚假的 UEVENT_GENERATED。 | 另行批准 netlink/设备模型接口后核对 action、环境字段、cookie 和发送失败；用户态 udev 规则/同步另外验收，不能说整个 sysfs 不存在。 |
| PR3-T03 | 非 UTF-8 设备名称/路径：仍不支持；UUID 已独立修复。 | mapper name、runtime alias 与 VFS/devtmpfs 路径使用字符串；任意字节路径需要跨层表示审定。当前仍可能对 Linux 合法字节名称返回 EINVAL。 | 不在 PR3 另造 byte-name 模型；若重启工作，先追踪 manager→节点→VFS 全链与错误/输出语义。 |
| PR3-T04 | 通用 DeviceId major 截断旁支：未修，非 DM 新引入。 | 原审查发现通用解码器在转 u16 后检查 major，可能丢高位；DM 已在 ABI 边界绕开该解码契约，不代表通用问题修复。 | 独立核实其他调用方和影响后另立任务，不混入 PR3。 |
| PR3-T05 | 1 GiB 实际分配/失败承接：待专项核实，不承诺容量可用。 | F04 对齐的是协议校验上限；NO_PARAMS 不分配声明大小。大 payload 仍受 heap/内存资源限制，allocator 大块失败链路及极限压力证据未完整归档。 | 不为此修改全局 allocator；后续先读清失败路径，再单独决定安全的故障注入/压力验收。不能写“已验证能分配 1 GiB”。 |
| PR3-T06 | 无表子集提取与前置：已完成，当前 PR4 待拆分。 | 命名注册及发现前置已独立为枚举 PR3；PR4 `6995e5ab3` 已提取无表身份及控制引用，不带 table/BIO/runtime/global poll；已验证并推送。 | 因 26 文件、+3203/-24 的审阅体量先讨论拆分，保留必要测试；不自动改写远端、不向 dm 回补、不重新扩范围。 |
| PR3-T07 | 事件精确提交点与 blocking poll 专项证据：仍待核实。 | 现有 workflow 在 resume 成功返回后发全局事件；是否需前移到精确 bind 提交点，及阻塞 poll 唤醒的直接专项覆盖，本轮未形成完整结论。已有 readiness/ARM 测试不替代所有调度交错证明。 | 对固定 Linux `__bind/event_callback` 和当前 replay/通知顺序逐项比较；确认可见差异后再决定修改，不能因套件通过宣称逐指令等价。 |
| PR3-T08 | 测试辅助代码三处泛型多余借用：已发现，未修改。 | `tests/lifecycle.rs` 的 duplicate/empty UUID rename 请求、`tests/ioctl.rs` 的 selector 请求仍借用临时 mapper_name；本次仅获准修复生产报错并只读检查其余调用。 | 不视为运行时缺陷；后续获准清理时按 ktest 构建/lint 检查，不把当前用户验收结果否定为失败。 |
| PR3-T09 | 目标分支验证、提交、推送已完成；拆分后的独立证据待补。 | PR4 助手 format/check 通过，用户确认系统及其他测试通过；历史整理核验工作文件不变。PR2/PR3 档案已更新；Book/SCML 未修改。 | 不再标记“未提取/未验证”；用户日志和精确计数未归档不等于测试失败。拆分后逐份验证，docs/log 不进入 upstream 源码 diff。 |

上述为本次 PR3 制备审查发现的全部已登记项；其他历史框架路线（freeze/thaw、target hooks、stacking 等）继续按第 6 节及 PR 规划管理，不能误记为本次已实施或无表 PR3 的新增交付。

### 10.3 历史 PR3 公共入口修正（源码核实，最终运行证据待归档）

| 日期 | 问题与影响 | 根因 | 修复 | 验证边界 | 提交状态 |
|---|---|---|---|---|---|
| 2026-10-20 | PR3 未支持命令在版本握手前返回 ENOTTY，与 Linux 公共入口顺序不同；无效地址/不兼容版本的 errno 和回写行为被提前拒绝遮蔽。 | decode_command 将 type 识别与命令支持检查合在握手前。 | 解码只检查 type 并提取 nr，握手后分派；统一文件名 dm_control.rs，补 C 顺序回归并调整内部测试。完整 dm 已有该顺序，本次不改其生产代码。 | 固定 Linux/PR3 源码核实，C 语法/格式和 diff 检查通过；用户反馈 C 回归已跑完，无最终运行日志/计数，不将新 SHA 全部 gate 标为通过。 | PR3 amend 为 `5b08f9a0a`，未推送；详见 [当日日志](../log/daily/2026-10-20.md) 和 [ABI 说明](control_ABI.md) |
