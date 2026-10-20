# Device Mapper 上游 PR 规范与历史

> **适用范围**：本文件定义 Device Mapper 相关 upstream PR 的共同规则、系列路线和已实际制备 PR 的总体历史。
>
> **职责边界**：每个已实际制备的 PR 的技术细节、Git 元数据、验证证据和英文 maintainer 描述记录在 [`log/PR/`](../log/PR/)；日报只记录当天发生的操作与结论。
>
> **最后更新**：2026-10-20（PR3 更新为 `5b08f9a0a`；PR4 仅重建分支，暂不推进；历史章节保留当时记录）。

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
| 1 | `make format` | 固定前置步骤；检查格式化 diff，不混入范围外改动。它会修改文件，不是测试通过证据。 |
| 2 | `make check` | 格式、lint、依赖和拼写等静态检查。 |
| 3 | `make kernel` | 确认裁剪后的 PR 可独立构建普通内核；记录实际 profile，不凭命令名断言 dev/release。 |
| 4 | 目标 crate 中的 `cargo osdk test` | 进入含目标 Cargo.toml 的目录，默认无函数筛选跑该 crate 全部 ktest，至少覆盖相关文件；core 加 `--kcmd-args=earlycon`。 |
| 按需 | C ABI 或系统测试 | 涉及 syscall/ioctl、用户工具工作流时补对应层，不要求每个 PR 全部运行。 |

例如，仅含 block BIO 映射的 PR 验证相关 block 测试，不要求运行尚未进入该 PR 的 DM 测试。
默认不执行完整 `make ktest`；影响难以限定时先讨论扩大范围。

完成要求：

- 构建与测试串行；选定检查全部成功，定向测试实际匹配并执行，零匹配或内部跳过不能证明目标路径通过。没有直接测试的改动明确记录覆盖缺口。
- 格式失败先按授权范围修复，再运行 `make check`；真实失败、阻塞或影响不明的问题须报告。已确认不影响构建/测试的环境提示留作备注，不仅凭 warning/error 字样中断。
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

## 3. PR 2–8：渐进交付路线

> 2026-10-19 初步规划。PR1–PR3 为已实际制备的本地提交；PR4–PR8 是待逐份源码核对和复审的方向，不是已经实现、验证或固定不变的承诺。本节只规划交付边界，不是 PR4 执行计划。第 5 节及历史档案保留当时事实，不能替代当前状态；最新验证见 [今日日志](../log/daily/2026-10-19.md)。

旧路线把创建与表加载放在 PR3；实际 PR3 交付公共控制入口，以 `DM_VERSION` 为首个命令，普通配置命令仍未实现。2026-10-20 已补齐“先版本握手、后拒绝未知命令”的公共入口约束，不能再把 VERSION 成功作为入口的唯一验收标准。因此由 PR4 补齐最小控制配置，首次激活目标移至 PR5；不沿用不存在的前置能力。最新 PR3 证据见 [10 月 20 日日志](../log/daily/2026-10-20.md)。

```text
PR1 mapped BIO ranges
  → PR2 固定单段 linear 数据面
  → PR3 控制设备与版本握手
  → PR4 创建、单段 linear 配置与未激活清理
  → PR5 首次激活与用户态块设备读写
  → PR6 已激活设备的安全删除
  → PR7 挂起、排空与在线换表
  → PR8 查询、身份与单设备事件管理
```

**路线原则：**

- 每份有独立可观察结果与失败路径测试；按职责拆，不按完整 dm 的文件布局拆包。
- 编号表示推荐交付顺序，不证明依赖已满足。若某项是前一份正确性的必要条件，必须前移或调整拆分，不能为守编号而延迟保护。
- 本轮不启动 main 同步、前置 PR 重放或推送。实际制备时核对 main/前置 SHA，是否更新按授权及既有同步规则处理，不自动重放已推送未合并 PR。
- 计划中的命令/flags 均需制备时对照指定 Linux 源码确定；不预先承诺所有 errno、顺序或 dmsetup/LVM 兼容。
- 当前本地前置链为 `77ee3b196` → `2f4fdc9a7` → `fd97f023f` → `5b08f9a0a`。本次未推送；PR1 在 10 月 19 日曾推送中间版本，不等于当前整条链已推送。

### 3.1 PR2：固定单段 linear 数据面（已制备）

分支 `dm-linear-io-path`，当前本地 `fd97f023f`。

- 单 backing、固定偏移和扇区数量、不可变单段 linear；`DmDevice` 提供 BlockDevice 接口。
- 复用 PR1 mapped BIO range，直接转发原 SubmittedBio；支持 read/write/flush，校验容量、请求范围和溢出。
- 没有控制对象管理、节点发布、动态表、BIO 拆分或通用 lease。
- 已补公开提交契约测试；测试源码存在不等于已运行，PR 分支最终验证证据单独记录。

### 3.2 PR3：控制设备与版本握手（已制备）

分支 `dm-control-interface`，当前本地 `5b08f9a0a`，替代 `f662bdc2d`，本次未推送。

- 注册 `/dev/mapper/control`，提供公共权限/type/版本/分派入口，以 `DM_VERSION` 为首个命令；逐次检查 initial user namespace 的 CAP_SYS_ADMIN。
- 版本只访问 12 字节，major/minor 兼容规则及 copyout 优先级保持；正确 DM type 的未知命令也先握手，再返回 ENOTTY。
- 控制文件统一命名 `dm_control.rs`，与完整 dm 命名一致，但不提前复制其子模块；poll 不伪报就绪。
- 不具备创建、加载、查询、激活或删除，尚不实现普通参数复制及全局控制事件。入口通用不等于已实现完整 DM。
- Rust 共 5 个 ktest，C 共 7 个 FN_TEST；用户反馈 C 回归已跑完，本次最终 check/kernel/ktest 的日志、计数未归档，不能沿用旧树通过结论。
- 详见 [PR3 档案](../log/PR/PR-3-control-interface.md) 与 [控制面 ABI](control_ABI.md)。

### 3.3 PR4：最小创建与 linear 配置（拟制备）

建议标题：`Add Device Mapper creation and linear table configuration`。
建议分支：`dm-linear-configuration`。2026-10-20 已删除无独有提交的旧同名分支，并从 `5b08f9a0a` 重建；目前与 PR3 无差异，用户要求先记录 PR3，暂不推进实现。

**目标闭环：**打开控制 fd → 创建无表对象 → 加载单段 linear 待激活配置 → 查询状态 → 清理未激活对象。

- 候选命令：`DM_DEV_CREATE`、`DM_TABLE_LOAD`、`DM_DEV_STATUS`、未激活对象的 `DM_DEV_REMOVE`；沿用版本握手。
- 区分控制对象与 PR2 已具备固定 target 的数据面设备；不能强迫无表对象提前拥有有效 target。选择满足本阶段且可承接首次激活的最小结构，不复制完整状态机。
- 只加入实际命令所需的用户内存、固定头/载荷、字符串、flags、权限、身份冲突与表边界处理。加载失败不得破坏已有有效配置；待激活表替换和清理的语义制备时明确。
- backing 查找方式、身份分配、引用持有与释放必须追清现有框架；Arc 不等于注册/移除保护。尚未支持的 backing/flags 必须明确处理，不静默接受无法实现的行为。
- **不包含：**可读写 mapper 发布、激活、在线换表、挂起、已激活删除、rename、WAIT、多 target、完整 dmsetup/LVM。
- **验收：**内部对象/表测试，C create/load/status/remove 及非法参数、权限、加载失败保留状态测试；不宣称用户态能打开 mapper 做 I/O。

### 3.4 PR5：首次激活与用户态读写

建议标题：`Activate linear mappings and expose block devices`。
建议分支：`dm-linear-activation`。

**目标闭环：**PR4 已加载配置 → 首次激活 → 获得块设备访问路径 → 用户态读写与 flush。

- 接入 `DM_DEV_SUSPEND` 的首次 resume/激活语义，不因命令名而同时承诺 suspend 和 replacement。
- 先核对 block registry、设备号、节点发布及 VFS open 的实际机制；判断既有节点机制或用户态 mknod 是否足够。不为自动 `/dev/mapper/<name>` 别名预先引入通用事务。
- 激活失败必须回滚或可重试；不得出现无有效表却可接收 I/O 的已发布设备。名称、设备号、backing 引用和在途 BIO 的所有权应明确。
- 成功激活的对象在此阶段按长期存在管理，明确拒绝未支持的删除、挂起和在线换表；禁止删除能力并不替代 backing 安全性核对。
- **验收：**真实 backing 上偏移读写、映射外数据不变、容量/边界、flush、重复激活、发布失败和后端错误；先 C ABI，再探测 dmsetup 最小命令额外需求，不为工具便利盲目扩展。

### 3.5 PR6：已激活设备的安全删除

建议标题：`Support safe removal of active Device Mapper devices`。
建议分支：`dm-safe-removal`。

**目标闭环：**已激活设备 → 拒绝仍被使用的删除 → 释放使用者 → 删除成功 → 不再能通过旧身份打开或提交新 I/O。

- 核对 manager、block registry、VFS 打开文件、挂载引用和在途 BIO 的实际责任，设计删除 admission 与注销顺序。
- 已打开/已挂载对象的保护不能只看 Arc 强引用数。若既有框架没有可靠的使用者保护，应报告最小前置依赖；不能照搬完整 dm 的通用 lease/VFS 改动，也不能为缩小 PR 提供不安全删除。
- 并发 open/remove、失败恢复、重复删除、设备号复用和陈旧引用必须纳入审查。支持何种“忙”拒绝及 drain 规则以 Linux 与框架证据确定。
- **不包含：**deferred remove、remove-all、自动 udev 同步、通用物理热拔插。
- **验收：**打开占用、关闭后删除、挂载占用（若当前可挂载路径成立）、并发边界、注销故障和资源释放；无覆盖条件的路径明确记为缺口。

### 3.6 PR7：挂起、排空与在线换表

建议标题：`Support Device Mapper suspension and linear table replacement`。
建议分支：`dm-linear-table-reload`。

**目标闭环：**运行中的单段 linear → 暂存新配置 → suspend/drain → resume 切换 → 新请求访问新映射，旧请求按旧表完成。

- 引入本阶段必要的 active/inactive 表与运行状态，处理 `DM_TABLE_LOAD`、`DM_TABLE_CLEAR`、suspend/resume 的相互作用。
- 明确新 I/O admission、在途计数、旧表/backing 生命周期、postponed I/O 接收/重放及完成责任，避免丢请求、重复完成或换表期间访问错误地址。
- 中断、失败回滚、只读表模式及 noflush 等 flags 逐项核对；不把 BIO drain 等同于文件系统 freeze/sync/thaw，也不静默把不同语义合并成一个路径。若所需 freeze 支持缺失，先明确可交付边界再实施。
- **不包含：**多 target、striped、DM-on-DM；不为了换表先引入 BIO 拆分框架。
- **验收：**延迟完成、旧请求排空、替换前后数据归属、失败/中断恢复、只读边界，以及对应 C ABI 状态转换。仅 inline completion 不足以证明并发生命周期。

### 3.7 PR8：查询、身份与单设备事件管理

建议主题：补齐已有单段 linear 设备的管理可观察性，支撑有边界的用户工具操作。
建议分支：`dm-management-queries-events`。

**候选交付：**设备/target 枚举、table/dependency/status 查询，rename/UUID 身份管理，以及 `DM_DEV_WAIT` 单设备事件。

- 查询编码区分 active/inactive 表、长度、对齐、buffer-full 和失败回写；不能用命令存在代替输出布局正确。
- 身份变更需与已有节点、索引及并发删除保持一致；UUID 支持范围与首次设置规则逐项核对。
- WAIT 的事件发布点、信号中断/重启、等待期间删除与持有引用应独立审查，不能把单设备 WAIT 写成全局控制 fd poll。
- **体量检查点：**此项是五份初步路线中最可能再拆的部分。查询与身份/事件没有必须同 PR 的耦合；制备时若无法保持单一可审查主题，优先交付查询，身份/事件另行拆分，不为编号硬塞。
- **依赖前移规则：**若 PR5–PR7 的正确 ABI 或必要验收要求某条查询/身份能力，应把最小所需部分前移，不能用“规划在 PR8”阻断前置验收。
- **验收：**原始 C ABI 的布局、容量和状态矩阵；身份/事件并发 ktest；再核对并实测选定 dmsetup 命令。不据此承诺完整 LVM2/udev 生态。

### 3.8 PR4–PR8 的共同验收与非目标

每份按第 1.3 节在自身最终源码树执行 `make check`、`make kernel`、相关定向 ktest；涉及 ABI 必须有 C 测试，工具流程按实际范围补集成。不复用 dm 结果冒充 PR 验证。

测试按职责组织并复用已有框架：控制 ABI 在 control 测试中扩展，随着生命周期或数据路径出现再按职责拆分，不为每个 ioctl 新建文件。每次新增能力同时测试拒绝/失败后的可见状态。

PR8 之后不预分编号：多段 linear、BIO 拆分、striped、zero/error、discard/zeroout、DM-on-DM、全局 poll/ARM_POLL、udev/uevent 自动化、完整 LVM2 及新 target 均不由本路线隐式承诺。

## 4. 文件归属与范围控制

| 范围 | 归属与边界 |
|---|---|
| block BIO 映射 | PR1；后续复用，不重复算作独有修改。 |
| 固定单段 linear 数据面 | PR2；后续演进只引入本阶段必需结构，不整目录复制完整 dm。 |
| control 节点与版本握手 | PR3 已制备；普通命令配置从 PR4 开始。 |
| 控制对象与待激活表 | PR4；实际参数/身份语义在执行计划中逐项确认。 |
| 首次激活、registry 与设备访问 | PR5；只补现有框架真实缺口。 |
| 活跃设备删除与使用者保护 | PR6；通用前置能力如确有必要，先报告再决定拆分。 |
| suspend/drain/reload | PR7；严格区分块层排空与文件系统冻结。 |
| 查询、身份与单设备事件 | PR8 候选；必要依赖前移，体量过大再拆。 |
| 通用 lease、rootfs/文件系统迁移、物理热拔插 | 不预设整套搬入；仅在证明具体阻塞后另行决定。 |
| docs/log/patches、本地镜像与构建便利项 | 不进入 upstream 源码 diff。 |

能用现有机制完成的，不新增通用框架。必须修改框架时，说明具体阻塞、最小变更和为何不能放在 DM 内部，由用户确认。功能受限不等于可以忽略生命周期和并发正确性。

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