# Device Mapper 上游 PR 提交计划

> **状态**：计划，尚未开始制备上游提交。
>
> **更新日期**：2026-09-23。
>
> **目标**：将 `dm` 分支的 Device Mapper 实现拆成可独立审阅、测试和回退的 upstream patch series；不把本地研发、验收和过程资料混入生产代码 PR。

## 1. 当前结论

当前 `dm` 分支相对 `main` 混合了 Device Mapper 生产代码、通用内核基础设施、本地 NixOS/LVM2 验收脚本、patch 快照和大量项目过程文档。它不能直接作为一个 upstream PR，也不应机械按现有 commit 边界 cherry-pick。

上游同步是 PR 制备的前置阶段：必须先在权威 `dm` 工作区完成与最新 `upstream/main` 的同步、处理语义冲突、运行相关验证并形成清晰的同步 commit。同步 worktree 只能用于冲突预演和审核，不能替代权威工作区的正式同步。只有同步后的 `dm` 成为唯一实现来源，才创建独立 PR worktree 重新摘取或重做最小 patch；该 PR worktree 的 base 仍为 `upstream/main` 或已合入的 stacked PR，而不是把整个 `dm` 分支直接提交。

## 2. 提交原则

1. 在创建、应用补丁或测试任何 PR worktree 前，先确认权威 `dm` 工作区已同步至最新 `upstream/main`、冲突已处理、相关验证已通过，并已形成独立同步 commit；有未提交研发内容时，先由用户决定其保留和同步方式，不用 stash/回退掩盖。
2. 每个 PR 必须在其 base 上独立构建、运行最小相关测试，并有明确的用户或内核可观察行为。
3. 通用 block、VFS、runtime 能力先于 Device Mapper 提交；不得仅以“DM 需要”作为扩大通用改动的理由。
4. DM 生产代码、直接 Rust ktest 和直接 initramfs C ABI 回归可随同一语义 PR 提交。
5. NixOS/LVM2/QEMU 系统测试保留为本地端到端验收证据，可写入 PR 的测试说明，但不提交其本地 harness。
6. 后续 PR 可以暂时以此前 PR 为 base，形成 stacked series；前置 PR 合入后依次改 base，不将整套实现压成一个巨型 PR。
7. 只提交英文、面向 upstream 的源码注释、测试名称和提交说明；中文项目过程文档不进入 upstream。

## 3. 建议的 stacked PR 系列

| 顺序 | 建议主题 | 内容边界 | 最小验证 | 主要风险 |
|---|---|---|---|---|
| 1 | `block: add composable BIO remapping` | 仅 `SubmittedBio` 的 remap/split、子段切片与聚合完成，以及 partition/request queue 对当前 sector range 的使用；明确排除 range BIO 和 lease。 | block crate 定向 ktest：组合映射、溢出不变、split、首错锁存、单次完成、partition 偏移与请求合并。 | current/original range 混用、映射溢出、child enqueue 失败悬挂或重复完成。 |
| 2 | `block: add tracked device leases`（审核完成，方案待定） | 已注册 generation 的长期使用权：`BlockDeviceLease`、pending/live/removing 注册状态，以及 ext2/exfat/VFS/MlsDisk 的完整调用链；具体结论见第 8 节。 | registry、VFS mount 与文件系统定向 ktest。 | 全局注册生命周期、锁序、失败回滚、virtio 分区状态与非 DM 调用者兼容性。 |
| 3 | `block: add discard and zeroout operations`（待单独审计） | range BIO、block wrapper、virtio/NVMe 后端适配和必要的用户 ABI；不与 PR 1 混合。 | block/back-end 定向 ktest 与实际用户 ABI 回归。 | 枚举 match 完整性、accepted/completion 错误通道和后端语义一致性。 |
| 4 | `vfs: support identity-safe runtime block nodes` | 按 inode identity 删除、通用 runtime device node/link 操作；必要的 named major 与 `/proc/devices` 支持。 | VFS/registry 定向 ktest，包括替换对象、跨 mount 和失败回滚。 | node 删除绝不能误删替换后的外来对象；DM 专用命名策略留给后续 PR。 |
| 5 | `dm: add mapper table and linear I/O path` | 新 `aster-device-mapper` component、target 抽象、table、mapper block device 和 linear 最小垂直切片。 | DM crate ktest。 | table lease、BIO remap/split、in-flight I/O 与 table 替换的生命周期。 |
| 6 | `dm: add control device and mapper lifecycle` | `/dev/mapper/control` 的基础 ioctl、create/remove/rename/status/list，以及 `/dev/dm-N` 和 `/dev/mapper/<name>` 的专用发布/回滚事务。 | core ioctl ktest 与对应 initramfs C ABI 回归。 | Linux `dm_ioctl` ABI、primary/alias 原子性、remove 与打开/mount 使用者隔离。 |
| 7 | `dm: add table lifecycle and event waiting` | table load/clear/status/deps、suspend/resume、remove protection、`DM_DEV_WAIT` 和信号重启语义。 | core ioctl ktest；C ABI 的 WAIT、lease、rollback 回归。 | wait 时不得持有 control lock；`EINTR` 与 `SA_RESTART` 的真实用户 ABI 必须正确。 |
| 8 | `dm: add additional targets` | `zero` 与 `error` 可作为一项；`striped` 单独提交，覆盖多 backing、跨段 I/O 与错误传播。 | target/table ktest；相关 C ABI 回归。 | striped 的 fan-out、聚合完成、错误锁存与边界 I/O。 |

表中顺序表达依赖与评审顺序，不要求一次性准备所有 PR。每个 PR 合入后应重新以最新 upstream base 验证后续 PR。

## 4. 文件归属

### 4.1 可进入上游系列

| 类别 | 主要路径 | 归属 |
|---|---|---|
| BIO 映射基础 | `kernel/core/comps/block/src/bio.rs`、`partition.rs`、`request_queue.rs` 的 remap/split 和 current-range 相关部分 | PR 1；不包含 range BIO。 |
| tracked lease | `kernel/core/comps/block/src/lib.rs` 的 lease/registry 状态，以及 VFS、ext2、exfat、MlsDisk 对它的真实使用 | PR 2；审核结论待确认，见第 8 节。 |
| range BIO 与后端适配 | `BioType::Discard` / `WriteZeroes`、`impl_block_device.rs`、virtio/NVMe、必要 registry ioctl | PR 3，待完成后端与 ABI 审计。 |
| runtime/VFS 基础 | `kernel/core/src/device/registry/**`、`kernel/core/src/device/mod.rs`、`kernel/core/src/fs/vfs/path/**`、必要的 procfs 支持 | PR 4；mapper 专用部分延后。 |
| DM component | `kernel/core/comps/device-mapper/**` | PR 5 与 PR 8。 |
| DM ioctl 与生命周期 | `kernel/core/src/device/misc/device_mapper/**`、对应 mapper 专用 registry 代码 | PR 6 与 PR 7。 |
| 用户 ABI 回归 | `test/initramfs/src/regression/device/device_mapper.c` 及必要注册入口 | 随相应 DM 用户可见语义提交。 |

### 4.2 必须排除

| 类别 | 路径 | 原因 |
|---|---|---|
| 协作、学习和过程文档 | `AGENTS.md`、`CLAUDE.md`、`docs/**`、`log/**` | 中文计划、交接、学习和本地验收记录不是 upstream 用户文档。若确有长期价值，应另行提炼简洁英文文档。 |
| 二进制文档 | `docs/*.docx` | 来源、权威性和替换关系未确认，也不适合作为源码 PR 证据。 |
| patch 快照与本地待办 | `patches/**`、`todo/**` | 是本地分支快照、日志或调试笔记，与上游 patch series 重复。 |
| 本地系统验收 harness | `myshell/**` | 依赖临时镜像、QEMU、LVM2、特定日志 marker 与超时策略；保留为开发验收。 |
| 本地 NixOS 发行环境 | `distro/**`、`tools/nixos/**` | 含本地 DM 工具打包、overlay 与运行环境假设，需由对应维护者另行评审。 |
| 本地构建便利项 | 与本地 TDX、selector、默认 release 或 QEMU 启动偏好相关的 `Makefile`、脚本改动 | 不属于 DM 生产功能。 |

## 5. 当前未提交工作区边界

当前未提交的 DM 源码、C 回归与本地系统脚本改动均不自动属于本提交计划：

```text
kernel/core/comps/device-mapper/src/device.rs
kernel/core/comps/device-mapper/src/manager.rs
kernel/core/comps/device-mapper/src/table.rs
kernel/core/src/device/misc/device_mapper.rs
kernel/core/src/device/misc/device_mapper/control.rs
test/initramfs/src/regression/device/device_mapper.c
myshell/run_dm_control_plane_test.sh
```

它们必须先独立完成语义审阅和最小测试闭环；之后再按实际行为放入对应 PR，或保留为本地验收改动。来源待确认的协作规则、学习资料和 DOCX 也始终隔离，不暂存、不回退、不混入任何上游提交。

## 6. 提交前阻塞项

1. `kernel/core/src/device/misc/device_mapper.rs` 中存在无条件 `[dm-debug]` 输出。上游版本应移除，或改为项目认可且不污染正常路径的诊断机制。
2. `kernel/core/comps/virtio/src/device/block/device.rs` 的 GET_ID 失败路径存在无界 `spin_loop()`。它不应随 DM PR 混入；应单独设计可恢复的通用驱动错误策略，或从本系列排除。
3. `BlockDeviceLease`、registry 状态机和 VFS identity 删除是高影响通用改动。提交前必须核对锁序、失败回滚和非 DM 调用者兼容性。
4. DM ioctl façade 规模较大，至少拆分为“基础控制/节点生命周期”和“table/WAIT 生命周期”两项，避免单个不可审阅的大 PR。
5. C ABI 回归按用户可见语义分配；不得以 NixOS harness、旧日志、编译成功或零用例替代真实测试结果。

## 7. 每个 PR 的准备流程

1. 核对权威 `dm` 工作区已经完成最新 `upstream/main` 同步、相关测试和同步 commit；若尚未满足，停止 PR 制备并先进入 [upstream-sync.md](upstream-sync.md) 的同步流程。
2. 从已同步 `dm` 的实现中识别该 PR 所需的生产文件和直接测试，同时以 `upstream/main` 或前序 stacked PR 作为干净 PR branch base；不复制文档、日志、patch 或本地 harness。
3. 审查 API、锁、资源所有权、失败回滚和并发边界；尤其核对该 PR 对非 DM 调用者的影响。
4. 在项目容器中串行运行该 PR 的最小定向测试；必要时再运行所属 crate 全量测试。
5. 对最终 diff 运行格式检查与 `git diff --check`，确认不存在调试日志、本地路径、中文面向 upstream 注释或无关格式化。
6. PR 描述说明行为变更、设计取舍、已知边界和实际测试命令；NixOS/LVM2 完整流程仅作为额外端到端验证证据。
7. 前置 PR 合入后，更新后续 PR base，重新检查冲突与定向测试结果。

## 8. PR 2 审核记录（待定）

### 8.1 已确认的通用语义

PR 2 可以独立表达为“对一个已注册 block-device generation 的长期使用权”，而不是 DM 专用引用计数：

```text
Pending：不可见、不可 lookup
Live：可取得 BlockDeviceLease
Removing：拒绝新的 lookup/lease
lease_count = 0：才允许 begin/commit unregister
未提交 token Drop：恢复为 Live
```

普通 `Arc<BlockDevice>` 不能表达“设备仍被已发布使用者持有，因此不能注销”的 registry 生命周期约束，不能替代 lease。

### 8.2 候选范围

若决定推进，PR 2 的候选生产范围为：

- `kernel/core/comps/block/src/lib.rs`：`RegisteredBlockDevice` 状态、`BlockDeviceLease`、registration/unregistration pending token、`begin_unregister`、`commit_unregister`、`lookup_lease` 和仅枚举 Live 的接口；
- `kernel/core/src/fs/vfs/fs_apis/registry.rs`：mount source 的 block-device lease 解析；
- ext2、exfat 的 open/mount 路径；
- `kernel/core/comps/mlsdisk/src/lib.rs` 中 `RawDisk` 的 backing lease 持有；
- virtio 分区刷新对注销结果的正确处理。

下列内容不属于 PR 2：`retain_removing`、`is_removing`、mapper primary/alias 事务、`kernel/core/src/device/registry/block.rs` 的 runtime node 逻辑、VFS identity-safe 删除、named major、`/proc/devices` 和所有 `device-mapper/**`。PR 2 应位于 runtime/VFS node PR 之前，因为 mount source 的 lease 解析不依赖动态节点。

### 8.3 已发现的阻塞问题

1. `lookup_by_name` 在持有全局 block registry lock 时调用 `device.name()`，存在 trait 回调重入 registry 的死锁面。若推进 PR 2，应先改为快照候选设备、释放 registry lock 后再调用 `name()`。
2. virtio 的分区刷新忽略旧分区 `unregister` 结果。若旧分区因 mount/lease 返回 `Busy`，它仍注册但驱动本地列表已丢弃，后续重扫或 ID 复用不安全；若推进 PR 2，必须修复并测试。
3. MlsDisk facade 的 `Drop` 当前只设置 `is_dropped`，不保证其内部 `RawDisk` 已释放 backing lease。PR 2 只能保证内部使用者存活期间的安全性，不能承诺 facade drop 后设备立即可注销；若需要后者，应另列 MlsDisk 生命周期 PR。

### 8.4 候选最小测试矩阵

- block：Pending 不可见、lease 最后释放后才可注销、begin unregister 后拒绝新 lease、两类 pending token 的 Drop 回滚、commit 失败 token 可回滚、同 ID 重入；
- ext2/exfat：mount 持 lease 时注销返回 `Busy`，unmount/drop 后可注销；
- virtio：刷新分区时 `Busy` 不丢失旧分区追踪；
- MlsDisk：RawDisk clone/subset 继续持有 backing lease。

**状态：审核完成，是否以该范围制备 PR 2 仍由用户决定。**在决定前，不创建提交分支、不暂存、不提交，也不将本节的候选结论表述为已实现或已验证。

## 9. 当前停点

PR 1 的只读拆分已经完成：它只保留与 Device Mapper 无关仍可独立成立的 BIO 映射底座，且确认 `aster-block` 没有对 `device-mapper` 的直接依赖。

PR 2 审核结论已记录但处于待定状态。当前不继续制备 PR 2 或自动进入后续 PR 审计，等待用户决定下一步。
