# Device Mapper 上游 PR 规范与历史

> **适用范围**：本文件定义 Device Mapper 相关 upstream PR 的共同规则、系列路线和已实际制备 PR 的总体历史。
>
> **职责边界**：每个已实际制备的 PR 的技术细节、Git 元数据、验证证据和英文 maintainer 描述记录在 [`log/PR/`](../log/PR/)；日报只记录当天发生的操作与结论。
>
> **最后更新**：2026-09-28。

## 1. 规范

### 1.1 权威实现与 PR 边界

1. `dm` 是唯一权威实现源。生产行为、修复和直接测试必须先在 `dm` 完成并验证；PR 分支只能裁剪其中已验证的最小行为子集。
2. 每个 PR 以当前 source `main` 或已合入的前置 PR 为 base。更新 base 时使用线性重放，不使用 merge 生成 PR 历史。
3. 一个 PR 只包含生产源码、直接 Rust ktest、必要的 initramfs C ABI 回归和面向 upstream 的英文说明。`docs/**`、`log/**`、`patches/**`、本地 NixOS/LVM2/QEMU harness、中文过程记录均不进入 upstream diff。
4. 通用 block、VFS、runtime 能力必须能独立成立，不能仅以“DM 需要”为由扩大通用改动。
5. PR 可以形成 stacked series；前置 PR 合入或 base/HEAD 改变后，后续 PR 必须重新核对冲突和相关验证。

### 1.2 直接分支生命周期

每个新 PR 都直接从已同步的 `main` 创建语义化分支，不使用临时工作树。`main` 是上游合并后的最新公共基线；`dm` 保持为权威实现和验证来源，PR 分支只迁移其中已验证的最小生产 diff。

```text
权威 dm 实现与验证
  → 同步 main 到当前 upstream main
  → 从 main 创建 <component>-<purpose> 分支
  → 迁移最小生产 diff，审核范围、生命周期、并发与失败边界
  → 在该 PR 分支暂存候选内容并验证
  → 将已验证的同一源码树提交为该分支 HEAD
  → push / force-with-lease，并核对远端 HEAD
  → 更新 PR 档案与本文件历史
```

- 新 PR 前先确认工作区干净，并使本地 `main` 快进到当前 `upstream/main`；随后直接创建 PR 分支。只有未合入的前置 PR 是真实依赖时，stacked PR 才以该前置分支为 base。
- 分支名使用 PR 标题的简短 kebab-case，不使用 `pr`、`pr1` 等泛化名称。例如 `block: add mapped BIO ranges` 对应 `mapped-bio-ranges`；后续可使用 `block-device-leases`、`dm-linear-io-path` 等名称。
- 候选内容应先暂存，再在目标 PR 分支完成验证；提交后必须确认该分支的 `HEAD^{tree}` 与验证前的候选 tree 相同。这样测试通过的源码树就是最终 PR 事实，而非需要额外搬运的副本。
- `git push --force-with-lease` 只用于 rebase 后已知的远端分支，拒绝覆盖本地未知的远端更新。每条测试证据必须记录命令、候选 tree、提交后的 PR SHA、环境和结果；旧 tree/SHA 的成功结果只能作为历史背景，不能证明当前 PR。
- `log/PR/` 只为实际制备的 PR 建档；规划中的 PR 保留在本文件，不创建空档案。

### 1.3 事实来源优先级

1. Git 和远端引用：base、HEAD、分支、提交范围、push 事件。
2. 最终 SHA 对应源码和直接测试：设计、接口与文件边界。
3. 命令输出或 CI：验证状态。
4. 日报：操作时间线和失败定位入口。
5. 设计、交接和 patch 快照：背景与决策依据；若与上述事实冲突，以前者为准。

`patches/` 是本地树快照与重放工具，不是 PR 提交范围、提交 SHA 或验证状态的权威来源。

## 2. 已实际制备 PR 总览与历史

| PR | 主题 | 状态 | base | HEAD | 档案 |
|---|---|---|---|---|---|
| PR 1 | `block: add mapped BIO ranges` | 已推送；最终 SHA 验证通过 | `98e717275` | `c14d68c93` | [PR 1 档案](../log/PR/PR-1-mapped-bio-ranges.md) |

| 日期 | PR | 事件 | 事实 |
|---|---|---|---|
| 2026-09-28 | PR 1 | 线性重建与推送 | 以 `98e717275` 为 base 重建单一 `c14d68c93`，仅包含 3 个 block 文件；通过 `--force-with-lease` 更新 fork 分支，移除旧 merge 历史。 |
| 2026-09-28 | PR 1 | 最终 SHA 验证 | 在 `fork_Asterinas:/root/pr-tree` 验证 `c14d68c93`：`make check`、block crate ktest（6/0）、`make ktest`（245/0）与 `make kernel` 均通过；验证后工作树已删除。 |
| 2026-09-28 | PR 1 | 语义化分支迁移 | fork 分支从 `pr` 改为 `mapped-bio-ranges`，保持 `c14d68c93` 不变并删除旧远端引用。 |

PR 1 的当前 Git branch 为 `mapped-bio-ranges`；文档中的稳定编号统一为 **PR 1**。

## 3. 规划中的 stacked PR 系列

| 顺序 | 建议主题 | 内容边界 | 最小验证 | 主要风险 |
|---|---|---|---|---|
| 1 | `block: add mapped BIO ranges` | `SubmittedBio` mapped range、partition 偏移与 request queue 合并；排除 range BIO 和 lease。 | block crate 相关 ktest、静态检查与内核构建。 | logical/mapped range 混用、映射溢出、完成生命周期。 |
| 2 | `block: add tracked device leases` | `BlockDeviceLease`、pending/live/removing 状态与真实非 DM 使用者。 | registry、VFS mount、文件系统定向 ktest。 | 生命周期、锁序、失败回滚、VirtIO 分区跟踪。 |
| 3 | `block: add discard and zeroout operations` | range BIO、block wrapper、virtio/NVMe 适配及必要 ABI。 | block/back-end ktest 与用户 ABI 回归。 | 后端语义、错误通道、枚举完整性。 |
| 4 | `vfs: support identity-safe runtime block nodes` | identity-safe 删除、runtime node/link、必要 registry/VFS 基础。 | VFS/registry ktest。 | 替换对象保护、失败回滚。 |
| 5 | `dm: add mapper table and linear I/O path` | DM component、target 抽象、table、mapper block device、linear 最小路径。 | DM crate ktest。 | table lease、BIO 映射/拆分、in-flight 生命周期。 |
| 6 | `dm: add control device and mapper lifecycle` | 基础 ioctl、create/remove/rename/status/list、节点发布回滚。 | core ioctl ktest 与 C ABI 回归。 | `dm_ioctl` ABI、primary/alias 原子性。 |
| 7 | `dm: add table lifecycle and event waiting` | load/clear/status/deps、suspend/resume、WAIT 信号语义。 | core ioctl ktest 与 C ABI 回归。 | 锁持有、`EINTR`、`SA_RESTART`。 |
| 8 | `dm: add additional targets` | zero/error；striped 另行覆盖多 backing 与跨段 I/O。 | target/table ktest 与 C ABI 回归。 | fan-out、聚合完成、错误锁存。 |

该表表达依赖与评审顺序，不表示每个 PR 已开始制备。

## 4. 文件归属与排除规则

### 4.1 可进入上游系列

| 类别 | 主要路径 | 归属 |
|---|---|---|
| BIO 映射基础 | `kernel/core/comps/block/src/bio.rs`、`partition.rs`、`request_queue.rs` | PR 1。 |
| tracked lease | block registry、VFS、ext2/exfat、MlsDisk 的真实 lease 使用 | PR 2。 |
| range BIO 与后端适配 | `BioType::Discard` / `WriteZeroes`、virtio/NVMe、必要 ioctl | PR 3。 |
| runtime/VFS 基础 | registry、VFS path、必要 procfs 支持 | PR 4。 |
| DM component | `kernel/core/comps/device-mapper/**` | PR 5 与 PR 8。 |
| DM ioctl 与生命周期 | `kernel/core/src/device/misc/device_mapper/**` | PR 6 与 PR 7。 |

### 4.2 必须排除

| 类别 | 原因 |
|---|---|
| 协作、学习和过程文档 | 本地中文计划、交接和验证记录不属于 upstream 源码 PR。 |
| `patches/**` 与本地待办 | 本地快照或调试资料，不代表 Git 提交边界。 |
| 本地 NixOS/LVM2/QEMU harness | 可作为测试说明证据，但依赖本地镜像、磁盘和超时约定。 |
| 本地发行与构建便利项 | 需由对应维护者独立评审，不能作为 DM 功能附带改动。 |

## 5. PR 2 审核状态（尚未制备）

PR 2 的候选语义是“对已注册 block-device generation 的长期使用权”，而不是 DM 专用引用计数：

```text
Pending：不可见、不可 lookup
Live：可取得 BlockDeviceLease
Removing：拒绝新的 lookup/lease
lease_count = 0：才允许 unregister
未提交 token Drop：恢复为 Live
```

候选范围包括 block registry lease、VFS mount source 解析、ext2/exfat 使用路径、MlsDisk backing lease 与 VirtIO 分区刷新。PR 2 已获授权推进，但以下问题必须先在权威 `dm` 关闭，才可从最新 `main` 制备 `block-device-leases` 分支：

1. `lookup_by_name` 持 registry lock 调用 `device.name()` 的潜在重入死锁；
2. VirtIO 分区刷新忽略旧分区 unregister 结果；
3. MlsDisk facade drop 与内部 RawDisk backing lease 的生命周期边界。

状态：**已授权在 `dm` 推进 PR 2 前置工作；尚未制备 PR 分支**。三项阻塞关闭并完成直接验证前，不创建 PR 档案、不将候选结论表述为已实现或已验证。

## 6. PR 档案更新要求

每个 `log/PR/PR-<n>-<topic>.md` 必须包含：状态与 Git 标识、远端事件、目标与非目标、设计/范围、diff、验证证据、风险/后续动作，以及可直接给 maintainer 的英文标题和描述。

英文维护者描述只说明行为、设计取舍、范围和真实测试；不得包含中文过程、工作区路径、未验证推测或虚构通过结果。