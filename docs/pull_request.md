# Device Mapper 上游 PR 规范与历史

> **适用范围**：本文件定义 Device Mapper 相关 upstream PR 的共同规则、系列路线和已实际制备 PR 的总体历史。
>
> **职责边界**：每个已实际制备的 PR 的技术细节、Git 元数据、验证证据和英文 maintainer 描述记录在 [`log/PR/`](../log/PR/)；日报只记录当天发生的操作与结论。
>
> **最后更新**：2026-10-09。

## 1. 规范

### 1.1 权威实现与 PR 边界

1. `dm` 是完整实现的权威来源，PR 是逐步交付的缩小版本。允许在 PR 分支进行最小子集必需的接口适配、结构简化和直接测试；这些简化形态可能无法在完整 `dm` 中验证，必须在 PR 分支独立验证。不得将 `dm` 降级成简化版本；发现两者共有的生产缺陷时，仍先修复并验证 `dm`，再迁移到 PR 分支。
2. 本系列每个后续 PR 基于“上一份 PR + 最新已核对的 main”：先将尚未合入的前置 PR 链线性更新到 main，再从更新后的上一份 PR 创建后续分支。前置已合入 main 时，不重复重放。禁止 merge；记录 main SHA、前置 SHA 和本 PR 独有 diff。
3. 一个 PR 只包含生产源码、直接 Rust ktest、必要的 initramfs C ABI 回归和面向 upstream 的英文说明。`docs/**`、`log/**`、`patches/**`、本地 NixOS/LVM2/QEMU harness、中文过程记录均不进入 upstream diff。
4. 通用 block、VFS、runtime 能力必须能独立成立，不能仅以“DM 需要”为由扩大通用改动。
5. PR 可以形成 stacked series；前置 PR 合入或 base/HEAD 改变后，后续 PR 必须重新核对冲突和相关验证。

### 1.2 直接分支生命周期

本系列直接创建语义化分支，不使用临时工作树制备 PR。先核对最新 main，再线性更新前置 PR 链；后续分支从更新后的上一份 PR 创建。`dm` 提供完整实现素材，简化版本的验证事实来自 PR 分支自身。上游同步所需的干净基线 worktree 不受此制备规则影响。

```text
权威 dm 作为完整实现素材
  → 核对并按既有同步规则更新 main
  → 将未合入的前置 PR 链线性更新到 main
  → 从更新后的上一份 PR 创建 <component>-<purpose> 分支
  → 提取并适配最小生产 diff，审核生命周期、并发与失败边界
  → 在该 PR 分支暂存候选内容并验证
  → 将已验证的同一源码树提交为该分支 HEAD
  → push / force-with-lease，并核对远端 HEAD
  → 更新 PR 档案与本文件历史
```

- 新 PR 前先确认工作区干净并核对上游 main；本地 main 不自动视为远端最新。同步和验证沿用现有规则，不为制备 PR 擅自启动同步。更新前置 PR 前询问用户是否保留备份引用，按当次决定执行；更新后重新判断验证范围；不得把旧 SHA 的通过结果沿用到新基线。
- 分支名使用 PR 标题的简短 kebab-case，不使用 `pr`、`pr1` 等泛化名称。例如 `block: add mapped BIO ranges` 对应 `mapped-bio-ranges`；后续可使用 `dm-linear-io-path`、`dm-control-interface` 等名称。
- 候选内容应先暂存，再在目标 PR 分支完成验证；提交后必须确认该分支的 `HEAD^{tree}` 与验证前的候选 tree 相同。这样测试通过的源码树就是最终 PR 事实，而非需要额外搬运的副本。
- `git push --force-with-lease` 只用于 rebase 后已知的远端分支，拒绝覆盖本地未知的远端更新。每条测试证据必须记录命令、候选 tree、提交后的 PR SHA、环境和结果；旧 tree/SHA 的成功结果只能作为历史背景，不能证明当前 PR。
- `log/PR/` 只为实际制备的 PR 建档；规划中的 PR 保留在本文件，不创建空档案。

### 1.3 每个 PR 的最终验证

**最终验证必须在新建的 PR 分支中执行，不能用 dm 上的通过结果替代。**
在项目容器中迁移完最小改动后，按以下顺序逐项执行；验证对象是准备提交的 PR 源码树。

| 顺序 | 验证方法 | 要求 |
|---|---|---|
| 1 | `make check` | 包含格式、lint、依赖和拼写等静态检查，不另列重复的格式检查步骤。 |
| 2 | `make kernel` | 确认裁剪后的 PR 可独立构建普通内核；默认 dev，不称为 release 验收。 |
| 3 | 目标 crate 中的 `cargo osdk test …` | 定向覆盖 PR 新增、修改及受接口/语义变化影响的路径；selector 按该分支实际测试模块选择。 |
| 按需 | C ABI 或系统测试 | 涉及 syscall/ioctl、用户工具工作流时补对应层，不要求每个 PR 全部运行。 |

例如，仅含 block BIO 映射的 PR 验证相关 block 测试，不要求运行尚未进入该 PR 的 DM 测试。
默认不执行完整 `make ktest`；影响难以限定时先讨论扩大范围。

完成要求：

- 构建与测试串行；选定检查全部成功，定向测试实际匹配并执行，零匹配或内部跳过不能证明目标路径通过。没有直接测试的改动明确记录覆盖缺口。
- 格式失败先按授权范围修复，再运行 `make check`；失败、环境阻塞或未预期 warning 出现时先停止并讨论，不扩大验证范围。
- 测试实际读取的源码必须与候选 tree 一致，不得依靠未暂存或未跟踪的辅助实现；提交后确认 `HEAD^{tree}` 与已验证候选 tree 相同。
- PR 简化或适配特有的问题在 PR 分支修复并验证；完整 dm 也存在的共有生产问题先在 dm 修复并验证，再迁移。不得要求将简化版本反向移入 dm，也不得把未验证的适配表述为已由 dm 验证。
- 记录分支、base、候选 tree、最终 SHA、配置、实际命令、匹配/通过/失败数量、退出码与日志；后续源码或 base 改变时重新判断影响并补跑相应验证。
- 历史结果不是当前 PR 的验收证据；验证通过不自动授权提交或 push。

### 1.4 事实来源优先级

1. Git 和远端引用：base、HEAD、分支、提交范围、push 事件。
2. 最终 SHA 对应源码和直接测试：设计、接口与文件边界。
3. 命令输出或 CI：验证状态。
4. 日报：操作时间线和失败定位入口。
5. 设计、交接和 patch 快照：背景与决策依据；若与上述事实冲突，以前者为准。

`patches/` 是本地树快照与重放工具，不是 PR 提交范围、提交 SHA 或验证状态的权威来源。

## 2. 已实际制备 PR 总览与历史

| PR | 主题 | 状态 | base | HEAD | 档案 |
|---|---|---|---|---|---|
| PR 1 | `block: add mapped BIO ranges` | 本地已 rebase；用户反馈新基线验证通过，原始输出未归档；旧 SHA 有推送及验证记录 | `10a241870` | `67ed6c70f` | [PR 1 档案](../log/PR/PR-1-mapped-bio-ranges.md) |

| 日期 | PR | 事件 | 事实 |
|---|---|---|---|
| 2026-09-28 | PR 1 | 线性重建与推送 | 以 `98e717275` 为 base 重建单一 `c14d68c93`，仅包含 3 个 block 文件；通过 `--force-with-lease` 更新 fork 分支，移除旧 merge 历史。 |
| 2026-09-28 | PR 1 | 最终 SHA 验证 | 在 `fork_Asterinas:/root/pr-tree` 验证 `c14d68c93`：`make check`、block crate ktest（6/0）、`make ktest`（245/0）与 `make kernel` 均通过；验证后工作树已删除。 |
| 2026-09-28 | PR 1 | 语义化分支迁移 | fork 分支从 `pr` 改为 `mapped-bio-ranges`，保持 `c14d68c93` 不变并删除旧远端引用。 |
| 2026-10-09 | PR 1 | 线性更新本地基线 | 在 mapped-bio-ranges 执行 git rebase main，无冲突；c14d68c93 → 67ed6c70f，base 为 10a241870。range-diff 为 =，三文件范围不变，无 merge commit，diff --check 通过。未推送新 SHA；用户随后反馈验证通过，未提供命令输出与计数。 |

PR 1 的当前 Git branch 为 `mapped-bio-ranges`；文档中的稳定编号统一为 **PR 1**。

## 3. PR 2—4：最小 Device Mapper 交付路线

本路线取代此前通用 lease、runtime/VFS、完整 DM 生命周期的九份 PR 规划。目标是尽量沿用 Asterinas 既有框架，先交付简单且正确的 DM；不按完整 dm 的文件布局直接拆包。不为 PR 4 之后的能力预分编号。

```text
最新已核对 main + PR 1 mapped BIO ranges
  → PR 2 最小单段 linear 数据面
  → PR 3 最小控制设备与配置接口
  → PR 4 首次激活与用户态块设备读写闭环
```

各阶段先线性更新前置链，再制备下一份 PR。PR 1 历史 SHA 与验证不因路线更新而改变；是否合入、更新或重新验证以实际操作为准。

### 3.1 PR 2：最小 linear 数据面

建议标题：`dm: add a minimal linear mapping component`；建议分支：`dm-linear-io-path`。

- 新增独立 DM component，实现单个 backing、固定偏移和长度、创建后不可变的单段 linear 映射及 BlockDevice 接口。
- 支持 read/write/flush，检查零长度、算术溢出、backing 容量和请求越界；正确传播提交失败与完成状态。
- 复用 PR 1 的 mapped BIO range；单段路径优先直接转发原 SubmittedBio，不引入拆分、子 BIO 聚合或通用 target 插件框架。
- 使用既有块设备引用机制，不引入通用 BlockDeviceLease。必须核对 mapper/backing/在途请求的所有权；Arc 仅保证对象存活，不是注册保护、热拔插保护或 I/O 成功保证。
- 不接 ioctl、设备节点、动态注册管理、在线换表、删除、多段映射、striped、zero/error、discard/zeroout。
- 验收：偏移读写、容量与请求边界、flush、后端拒绝/错误、延迟完成及必要生命周期测试；make check、make kernel 和相关 block/DM 定向 ktest。在内核测试中可构造 mapper，不宣称用户态可创建。

### 3.2 PR 3：最小控制与配置

建议标题：`dm: add the initial device-mapper control interface`。

- 复用既有字符设备机制接入 /dev/mapper/control，采用 Linux DM ABI 的明确子集，不另造临时协议。
- 提供必要的版本查询、无表对象创建、单段 linear 表加载、状态查询及未激活对象清理；具体 ioctl/flags 清单在制备时确认。
- 校验用户内存、头部长度、偏移、字符串、权限与名称冲突；加载失败不得破坏既有有效配置。
- 不发布可读写 mapper 节点，不提供已激活设备删除、rename、WAIT 或完整 dmsetup/LVM2 兼容声明。协议版本与 flags 不照搬完整 dm 的承诺。
- 验收：控制路径定向 ktest 和 C ABI 创建/加载/查询/清理及非法输入测试；仍执行 check/kernel。

### 3.3 PR 4：首次激活与用户态访问

建议标题：`dm: activate linear mappings and expose block devices`。

- 将已加载表首次激活，通过既有 block registry 注册 mapper，完成用户态打开、读写和 flush。
- 优先核实已有设备节点机制或用户态 mknod 是否足够；此接入方式尚未确认，不宣称已有实现可直接满足。不为自动维护 /dev/mapper/<name> 别名预先引入通用节点事务。
- 激活失败必须可清理或重试，不能留下错误注册。成功激活设备暂按长期存在对象管理，明确拒绝尚不支持的在线换表、挂起和删除。
- 不要求完整 udev/uevent、LVM2、多 PV 或多 target。先以 C 程序验证 ABI，再实测 dmsetup 最小命令的额外要求，不为工具兼容盲目扩展范围。
- 验收：真实 backing 上创建/加载/激活/读写/flush，核对对应偏移数据和映射外数据不变；覆盖重复激活、失败回滚和后端错误。执行 check/kernel、定向 ktest 和用户态回归。

## 4. 文件归属与范围控制

| 范围 | 归属与边界 |
|---|---|
| block BIO 映射 | 已有 PR 1；PR 2 依赖它，不重复算作 PR 2 独有修改。 |
| DM component 与必要 Cargo 接入 | PR 2 起按最小功能提取；不能整目录复制完整 dm。 |
| control 字符设备、管理与 ABI | PR 3；仅配置阶段必要能力。 |
| 首次激活、registry 与设备访问适配 | PR 4；仅补当前框架实际缺口。 |
| 通用 lease、ext2/exfat/rootfs 迁移、分区事务 | 暂缓，不作为本路线前置。 |
| runtime rename/remove、复杂 target、range BIO 后端扩展 | 暂缓，不预排后续编号。 |
| docs/log/patches、本地镜像与构建便利项 | 不进入 upstream 源码 diff。 |

能用现有机制完成的，不新增通用框架。必须修改框架时，先说明具体阻塞、最小变更和为何不能放在 DM 内部，反馈用户后决定。限制功能不等于忽略正确性：尚未保护的设备生命周期必须明确，不允许静默提供不安全操作。

## 5. PR 2 制备状态与设计取舍

### 5.1 当前停点（2026-10-09）

- 完整实现来源 dm 为 `2699dfed0`；本地 main 为 `10a241870`。本次没有获取新的远端 main。
- PR 1 已线性 rebase 为 `67ed6c70f`；PR 2 分支 `dm-linear-io-path` 从该提交创建，当前没有 PR 2 独有提交。
- 旧 `block-device-leases` 已由用户删除。两条 backup 分支已按用户确认删除，后续是否备份先询问，不自动创建。
- 用户手敲，助手分组讲解与只读核对；允许在 PR 分支将完整 dm 缩小适配，不要求将简化实现反向写入 dm。
- 已编写：新 crate 与 workspace members/default-members 接入、LinearTarget/LinearTargetError、构造范围校验、元数据、单 backing read/write/flush 转发；DmDevice 保存 id/name/target，提供构造及 BlockDevice 接口；lib.rs 对外导出类型。
- 当前最小组件为 Cargo.toml 与 lib.rs、linear.rs、device.rs，不引入 table/manager、lease、BIO 拆分聚合或控制接口；根 Cargo.lock 有相应更新。
- 已开始构造测试：GeometryBacking 模拟设备及 accepts_mapping_ending_at_backing_capacity，检查映射恰好到 backing 尾端、mapper 容量及 segment 上限。测试尚无执行结果。
- 用户反馈 PR 1 新基线验证通过；在指导执行组件 cargo osdk check 后反馈“完全没毛病”。均未提供原始输出、完整环境与退出码；组件反馈早于新增首个测试，不能当作当前测试快照或最终 PR gate 通过。
- 收工已回到 dm，规划文档已恢复；PR 2 代码保存在 stash `43291518c58869e5dd59efb1bc253f1af378c985`（当前 stash@{0}，说明 pr2-code）。它包含两个根 Cargo 文件及四个新组件文件，不在 PR 2 分支 HEAD 中。stash 编号可能变化，恢复前按 SHA 和说明核对。
- 用户明确测试模块的 cfg(ktest) 注释是有意调整；不擅自修改，不将此作为已确认缺陷。

### 5.2 已采用的最小设计

- 单段逻辑范围固定从 0 开始；LinearTarget 持有 Arc<dyn BlockDevice>、backing_start 与 length，配置不可变。
- DmDevice 负责设备身份与 BlockDevice 接口，实际元数据和 I/O 委托给 target。id/name 由调用方提供，不自动分配编号、注册设备或发布节点。
- metadata 报告 mapper 长度并沿用 backing 的 segment 上限，不修改 backing 的元数据。
- 直接偏移原 SubmittedBio 的当前 mapped range，再将同一请求移交 backing；不复制 payload、不创建子 BIO、不替 backing 完成请求。flush 不按普通数据范围偏移，直接转发。
- 入队拒绝返回 BioEnqueueError；已接受请求的最终状态由后端完成机制传播。enqueue 的 Ok(()) 不等于 I/O 已完成。
- Arc 保证对象引用的存活，不保证设备注册、硬件可用性或任意销毁期间的异步 I/O 安全。生命周期与错误传播仍需直接测试。

### 5.3 BIO 校验：保留局部方案，后续在 dm 讨论全局职责

当前决定不为 PR 2 改造通用 block 框架：

1. 构造时拒绝零长度，检查 backing_start + length 溢出，以及映射终点超过构造时 backing 容量。
2. enqueue 对 read/write 保留 start >= end || end > self.length 的拒绝条件；flush 单独处理。
3. 继续使用 offset_mapped_sid_range，保留通用 remap API 自身的算术检查。

理由与边界：

- Bio::new 当前验证长度汇总和终点运算，不接收目标设备，不能替代 mapper 窗口检查。u64 的输入值合法不保证加法结果不溢出；小请求也可能因很大的映射起点而溢出。
- 合法映射配置不保证每个请求都在窗口内。超出 mapper 的请求映射后仍可能在整盘容量内，底层整盘检查不能替代 mapper 边界。
- 构造合法、请求在窗口内且 backing 容量不变时，可以推导转换后的范围仍在 backing 内；不需要逐次重新查询 backing 容量。
- 现有 BIO 构造与重映射接口已排除反向范围，因此局部判断存在防御性重叠；拒绝空 read/write 是当前局部策略，不宣称已统一全栈语义。
- 该方案是最小 PR 范围下的局部取舍，不宣称全局最优，也不因此认定 block 框架存在已证实缺陷。尚无完整测试证据。

后续回到权威 dm 再讨论：BIO 构造/地址修改建立的不变量、设备准入约束的责任边界、空读写契约，以及是否需要统一校验入口。不能仅在 Bio::submit 加检查就认为所有入口被覆盖，因为层间转发直接调用 enqueue。涉及统一接口时须评估分区、mapper、驱动、可变容量、错误传播与回归范围，再决定独立改动，不在本 PR 静默扩展。

### 5.4 下次恢复与测试顺序

- 先保存 dm 文档改动，再切回 dm-linear-io-path；核对代码 stash 后恢复，不把过程文档带入源码 PR，也不覆盖 dm 的完整组件。
- 从已写的首个构造测试继续：检查当前测试布局，补零长度、终点溢出、超过容量等构造边界；然后补映射、请求越界、flush、入队拒绝、完成一次性与延迟完成、数据内容及引用生命周期测试。
- 测试中的预期条件必须用断言，不能仅靠编译通过。依据实际测试模块确定 selector，记录匹配数量和退出码；目前不能写任何通过数量。
- 最终在 PR 2 分支执行 make check、make kernel 和 block/DM 定向 ktest。当前组件尚未被 core 使用，make kernel 不单独证明该组件被编入生产内核，组件检查与自身 ktest 不可省略。
- 教学注释及格式在提交前按仓库风格整理；正式源码注释英文，按实际需要添加 trait/derive，不为未来功能预先扩展接口。
- 本次用户要求只更新已有文档，不新建 PR 2 档案。未提交、未推送新 PR；后续另行授权。

## 6. PR 档案更新要求

每个 `log/PR/PR-<n>-<topic>.md` 必须包含：状态与 Git 标识、远端事件、目标与非目标、设计/范围、diff、验证证据、风险/后续动作，以及可直接给 maintainer 的英文标题和描述。

英文维护者描述只说明行为、设计取舍、范围和真实测试；不得包含中文过程、工作区路径、未验证推测或虚构通过结果。