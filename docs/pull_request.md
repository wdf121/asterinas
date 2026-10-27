# Device Mapper 上游 PR 规范与历史

> **适用范围**：本文件定义 Device Mapper 相关 upstream PR 的共同规则、系列路线和已实际制备 PR 的总体历史。
>
> **职责边界**：每个已实际制备的 PR 的技术细节、Git 元数据、验证证据和英文 maintainer 描述记录在 [`log/PR/`](../log/PR/)；日报只记录当天发生的操作与结论。
>
> **最后更新**：2026-10-27。
>
> **PR3 制备进展**：完整 dm 的已批准语义修复已写入工作区，用户反馈验证通过；尚未提交和提取。`dm-device-management` 仍为 `808d36daf`，与 PR2 相同。所有已修/待修项、状态和卡点统一维护在 [global.md §10.4](global.md#104-pr3-制备前修复与待办总账2026-10-27)；[今日日志](../log/daily/2026-10-27.md) 区分用户反馈和独立证据。PR3 仍限定无表管理，不自动前移 table/BIO、runtime、rename、REMOVE_ALL；命名 major 前置及全局事件拆分待审定。dm 验证不能替代 PR3 验证。
>
> **当前说明**：PR1 不改实现；从现有 PR2 开始规划至通用 DM 框架闭环，再以一个最简单的 linear target 完成真实用户态终验。PR2～PR11 为候选拆分，不代表已实现。dm 已 merge 最新 main；随后经单独授权，PR1/PR2 已线性更新至 main `ceb65e3f8`。PR1 已推送，PR2 清理后为 `808d36daf`、未推送；网页审阅状态未复核。

## 1. 规范

### 1.1 权威实现与 PR 边界

1. **每个 PR 必须是 `dm` 的功能子集，`dm` 是唯一生产实现来源。** 允许必要的提取和接线，不允许为缩小 PR 单独设计架构、改变语义或引入 `dm` 中不存在的生产实现。无法独立提取时调整 PR 边界或前移依赖；核验发现 `dm` 缺口时，先汇报影响、是否阻塞及可选方案。未经确认不修复、不在 PR 分支绕过；批准后先在 `dm` 实现并验证，再提取。
2. 本系列每个后续 PR 基于“上一份 PR + 最新已核对的 main”：先将尚未合入的前置 PR 链线性更新到 main，再从更新后的上一份 PR 创建后续分支。前置已合入 main 时，不重复重放。禁止 merge；记录 main SHA、前置 SHA 和本 PR 独有 diff。
3. 一个 PR 只包含生产源码、直接 Rust ktest、必要的官方 `test/initramfs` C ABI 回归、`test/nixos` 系统测试及面向 upstream 的英文说明。`docs/**`、`log/**`、`patches/**`、本地 NixOS/LVM2/QEMU harness、中文过程记录均不进入 upstream diff。
4. 通用 block、VFS、runtime 能力必须能独立成立，不能仅以“DM 需要”为由扩大通用改动。
5. PR 可以形成 stacked series；前置 PR 合入或 base/HEAD 改变后，后续 PR 必须重新核对冲突和相关验证。

### 1.2 直接分支生命周期

本系列直接创建语义化分支，不使用临时工作树制备 PR。main 同步、前置链更新和分支操作须按授权及既有同步规则进行，不因本大纲自动重放已推送未合并 PR。`dm` 是实现来源，各 PR 子集必须在自身分支独立验证；上游基线不使用嵌套 worktree。

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

- 新 PR 前先确认工作区干净并核对上游 main；本地 main 不自动视为远端最新。同步和验证沿用现有规则，不为制备 PR 擅自启动同步。更新前置 PR 前创建安全引用，按当次授权执行；更新后重新判断验证范围；不得把旧 SHA 的通过结果沿用到新基线。
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
- 提取或接线问题按 dm 的已有实现修正；若需要新增设计或行为，先报告，不在 PR 分支另造实现。dm 本身的缺口或缺陷经用户确认后，先在 dm 修复并验证，再提取。dm 的通过结果不能替代 PR 最终源码树的验证。
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

| PR | 主题 | 本地状态（2026-10-26 核对） | 直接 base | HEAD | 档案 |
|---|---|---|---|---|---|
| PR1 | mapped BIO ranges | 用户反馈检查及 block 测试成功；已推送并核对远端 SHA，历史关联 #3937 | `ceb65e3f8` | `2dd1bcac5` | [PR1](../log/PR/PR-1-mapped-bio-ranges.md) |
| 新 PR2 | 控制设备与 DM_VERSION | 六文件单提交，已移除公共筛选扩展；本轮未推送 | `2dd1bcac5` | `808d36daf` | [PR2](../log/PR/PR-2-control-interface.md) |

两者共同 main 基线现为 `ceb65e3f8`，保持 `main → PR1 → PR2` 线性链。PR2 的格式/静态/构建/core 测试成功为用户反馈，最终清理后未重跑；官方全量回归在 cgroup 断言处失败，main 同环境复现，不能写为全量通过。具体命令与边界见 PR2 档案；dm 测试不替代 PR 验收。

| 日期 | PR | 事件 | 事实 |
|---|---|---|---|
| 2026-09-28 | PR 1 | 线性重建与推送 | 以 `98e717275` 为 base 重建单一 `c14d68c93`，仅包含 3 个 block 文件；通过 `--force-with-lease` 更新 fork 分支，移除旧 merge 历史。 |
| 2026-09-28 | PR 1 | 最终 SHA 验证 | 在 `fork_Asterinas:/root/pr-tree` 验证 `c14d68c93`：`make check`、block crate ktest（6/0）、`make ktest`（245/0）与 `make kernel` 均通过；验证后工作树已删除。 |
| 2026-09-28 | PR 1 | 语义化分支迁移 | fork 分支从 `pr` 改为 `mapped-bio-ranges`，保持 `c14d68c93` 不变并删除旧远端引用。 |
| 2026-10-09 | PR 1 | 线性更新本地基线 | 在 mapped-bio-ranges 执行 git rebase main，无冲突；c14d68c93 → 67ed6c70f，base 为 10a241870。range-diff 为 =，三文件范围不变，无 merge commit，diff --check 通过。未推送新 SHA；用户随后反馈验证通过，未提供命令输出与计数。 |
| 2026-10-26 | PR1 | rebase 与推送 | `2f4fdc9a7` → `2dd1bcac5`，range-diff 为 `=`；用户反馈验证成功，精确 lease 推送后远端 SHA 核验一致。 |
| 2026-10-26 | PR2 | rebase 与范围收口 | `ea1a9639c` → `fabadd3cb` → `808d36daf`；移除回归参数扩展，代理前缀撤销，保留六文件；未推送。 |

PR 1 的当前 Git branch 为 `mapped-bio-ranges`；文档中的稳定编号统一为 **PR 1**。

## 3. 从 PR2 到框架完成的路线

### 3.1 目标与非目标

PR1 保持原范围，不在本阶段重做。从 PR2 开始交付不依赖具体 target 算法的 DM 框架；完成后接入一个最简单的 linear target，以真实用户态路径终验。不再按“先交付固定 linear，再补基础管理”的旧路线推进。

终态要求：新增符合已交付 BIO 契约的 target，主要实现构造、注册、映射、状态/依赖与必要生命周期回调，而不应重新设计公共 ioctl、manager、表事务、完成聚合、暂停换表或节点安全删除。该目标不承诺所有 Linux target 可直接移植；复杂 target 的元数据、后台任务、密码学等依赖需单独评估。

每份必须以 dm 为生产实现来源。当前缺口先在 dm 审定、实现和验证，再提取；不在 PR 分支另造实现。下表为候选顺序，文件边界和数量允许依源码审查调整；安全依赖不能为了维持编号而后移。

### 3.2 候选 PR 与验收边界

| PR | 主题 | 交付与依赖 | 主要验收 |
|---|---|---|---|
| PR2 | 控制入口 | 现有 control device、权限、编码校验、版本握手、未知命令处理；依赖现有 PR 链。 | 原始 ioctl 的权限、版本、指针及错误优先级。 |
| PR3 | 身份与基础管理 | component/manager、编号、名称/UUID 索引、无表状态；CREATE、STATUS、LIST_DEVICES、无表 REMOVE；普通参数处理随命令引入。 | 无 target 的创建、查询、删除；重复标识、查找并发和失败回滚。 |
| PR4 | backing 持有与安全注销基础 | 必要的 block registry、lease、open/mount 持有与注销状态机，作为通用前置。 | 长期持有阻止错误注销；失败恢复，不把 Arc 内存保活当作设备使用权。 |
| PR5 | 节点身份与发布基础 | 必要的 devtmpfs identity handle、节点创建/删除/回滚；服务后续 runtime 接线。 | 同路径外来替换不能被旧 handle 误删；失败不留下半发布状态。 |
| PR6 | target 类型与实例契约 | 类型目录、构造、参数/backing 解析边界、能力表达、状态/依赖遍历与类型发现；依赖 PR4。 | 增加测试专用实现无需改控制命令；无生产 target 的空目录行为明确。 |
| PR7 | 通用表与 inactive 装载 | 范围校验、实例/lease 所有权、容量及能力汇总；LOAD、CLEAR、STATUS、DEPS；依赖 PR3/4/6。 | 构造失败保留旧状态；引用释放、active/inactive 查询与缓冲区边界。 |
| PR8 | 通用 BIO 通路 | 定位/拆分、下层提交、完成聚合、错误传播、异步表保活；明确普通和特殊 I/O 契约；依赖 PR7。 | 延迟/乱序完成、部分提交失败时恰好完成一次，旧表和 backing 不提前释放。 |
| PR9 | 首次激活与运行时生命周期 | primary/alias、容量/只读、激活事务、open gate、安全移除；整合 PR4/5/7/8。 | 无表→可用块设备→删除闭环；各失败点可见状态及恢复正确。 |
| PR10 | 暂停、恢复与换表 | in-flight/drain、排队重放、表切换、target hooks 及失败撤销；审定普通 suspend/NOFLUSH 与文件系统协调。 | 并发 I/O 换表；中断和回调失败不丢请求、不丢表、不提前发布新状态。 |
| PR11 | 管理与事件闭环 | rename/UUID、单设备事件与 WAIT、信号中断/重启、REMOVE_ALL 忙设备及部分成功语义。 | 索引与节点一致，事件与状态一致，批量操作保留忙设备。 |
| 框架后首个 target PR | 最小 linear 与终验 | 从 dm 提取符合新框架接口的最简单 linear，不加入 striped/LVM 等扩展；依赖完整框架。 | 真实 dmsetup 创建、读写、暂停/换表/恢复、忙设备删除检查与清理。 |

PR4/5 是公共安全前置，不强塞进 target 实现。PR9 首次开放设备 I/O 时必须同时具备相应删除保护。涉及公共基础设施过大时可独立拆分，不提前承诺具体文件数。

### 3.3 当前已发现的前置问题与决策点

以下来自 2026-10-26 源码初步核对，尚非逐项 Linux 语义审定或修改授权：

| 项目 | 当前事实与证据 | 路线处理 |
|---|---|---|
| 固定 target 目录 | `target/mod.rs` 的 TargetKind/TARGET_CATALOG 显式引用四种具体 target。 | PR6 前在 dm 审定构造/注册边界；静态注册即可作为候选，不要求动态模块加载。 |
| 映射动作受限 | DmTarget::map_io_range 返回 Remap/Error/Zero 动作，不能据此推定支持 target 自持 BIO、重排队或完成回调。 | PR6/8 审定异步所有权、特殊 I/O 与支持范围；Zero 动作是否属于通用协议也须说明。 |
| target hooks | 当前 DmTarget trait 无 suspend/resume hooks；Linux 基线的 target_type 有 presuspend/undo/postsuspend/preresume/resume，dm.c 经 dm-table.c 调用。 | PR10 前追清调用顺序、失败/撤销语义，再在 dm 补齐；不是仅添加空方法。 |
| 文件系统协调 | device.rs 明确仅提供 BIO drain、不提供 freeze；Linux dm.c 存在 lock_fs/freeze_bdev 与 unlock_fs/thaw_bdev。 | 核对 VFS 前置；补齐依赖或明确收窄暂停语义，不宣称完整 Linux suspend。 |
| DM 堆叠 | table.rs 当前拒绝 DmDevice backing。 | PR4/7/8 审定是否纳入；需要依赖图、持有及删除协调，不能默认自然支持。 |
| 设备管理生态 | 已有 devtmpfs 节点不等于 sysfs/uevent/udev 自动发现，单设备 WAIT 不等于全局 poll。 | 分别列依赖与验收；不自动纳入完整 LVM 自动激活或 request-based DM。 |

主要 Linux 依据仍为本机 `/root/Linux-tree-code/kernel` 的 `5078a3a23a1e1825ec136485173ec98668cdd640`。实施前逐项提供双方源码调用链、差异、影响与验证方案，不能把本表推断当已确认缺陷或已完成能力。

### 3.4 分层验证与最小 linear 终验

- 每份 PR 在其最终源码树执行第 1.3 节 gate，dm 验证不替代 PR 验证。
- 内核框架阶段允许仅测试构建启用的 target/backing 替身，覆盖构造失败、延迟完成、拆分聚合、暂停和回调失败；不增加测试专用用户 ABI，不把替身作为生产 target 交付。
- 用户态测试参考官方 `test/initramfs` 与 `test/nixos` 的构建、组织与运行入口；无生产 target 时只验收实际可用的静态查询、无表管理及错误路径，明确成功装表/数据面覆盖缺口。
- 框架完成后接入最小 linear，补上真实端到端终验：定位 disposable backing → 创建单段映射 → 原始块读写并核对映射位置 → 暂停与恢复 → 换表并核对新映射 → 打开期间 remove 返回 busy → 关闭并删除 → 确认节点和资源清理。挂载/文件系统场景仅在相关持有与 freeze 语义已支持时补充，不将单次原始 I/O 验收夸大为完整文件系统验证。
- 最小 linear 只是首个生产 target 的验收载体，不作为“为了跑通而绕过框架缺口”的理由。若需要修改公共机制，回到对应框架阶段处理。
- 运行已有 NixOS 镜像不能证明新 PR 内核已安装；必须核对镜像来源。实际测试记录源码树、配置、命令、非零计数、退出码及日志。

### 3.5 旧系列与新系列关系

旧 linear-first 路线不再作为当前规划。旧 PR2/PR3 档案为工作区待提交删除，旧分支的历史验证不能移用于新 PR2。当前本地 dm-control-interface 已复用于新 PR2；本次未核查远端旧分支删除状态。

## 4. 提取边界

| 内容 | 处理原则 |
|---|---|
| dm 已有框架实现 | 按依赖提取；必要接线不得改变设计、语义和生命周期保证。 |
| dm 中的具体 target | 不进入框架 PR；框架完成后单独提取最小 linear 做终验。 |
| 通用 block/VFS/runtime 前置能力 | 核实必要性后提取 dm 中已有实现，过大则独立拆分；不借机扩展无关能力。 |
| dm 缺失或存在缺陷的能力 | 先汇报，由用户决定是否在 dm 修复并验证；不能只在 PR 分支实现。 |
| 文档、日志、本地环境及工具便利项 | 不进入 upstream 源码 diff；本文件仅记录本地规划与历史。 |

## 5. PR 档案更新要求

每个 `log/PR/PR-<n>-<topic>.md` 必须包含：状态与 Git 标识、远端事件、目标与非目标、设计/范围、diff、验证证据、风险/后续动作，以及可直接给 maintainer 的英文标题和描述。

英文维护者描述只说明行为、设计取舍、范围和真实测试；不得包含中文过程、工作区路径、未验证推测或虚构通过结果。