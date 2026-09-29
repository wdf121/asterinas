# Device Mapper 对话交接

> **最后更新**：2026-09-29
>
> **定位**：本文件只记录当前停点、阅读入口、工作区边界和下一步决策；详细 PR 技术事实以 [`log/PR/`](../log/PR/) 为准，操作时间线以 [daily log](../log/daily/) 为准。

## 新窗口启动顺序

| 顺序 | 文档或动作 | 用途 |
|---|---|---|
| 1 | [CLAUDE.md](../CLAUDE.md) 与 [AGENTS.md](../AGENTS.md) | 获取协作、容器、测试和 upstream PR 规则。 |
| 2 | [当前项目状态](../log/device-mapper-progress.md) | 获取 DM 路线和主动边界。 |
| 3 | [PR 规范与历史](pull_request.md) | 获取共同 PR 规则、系列路线和总体历史。 |
| 4 | [PR 1 档案](../log/PR/PR-1-mapped-bio-ranges.md) | 获取当前已推送 PR 的范围、Git 状态、英文描述和最终验证证据。 |
| 5 | `git status --short` | 区分分支提交、文档恢复改动和来源待确认文件。 |

## 当前停点

### 权威 `dm` 已同步最新 main

- `main` 已快进至 `upstream/main` 的 `98e717275`。
- `dm` 已无冲突合并该 main，当前合并提交为 `5f29d2270`。
- `dm` 仍是生产实现、修复和直接测试的唯一权威来源；PR 分支不得领先其行为。

### PR 1 已推送且完成最终验证

- PR 编号为 **PR 1**；当前 branch 为 `mapped-bio-ranges`。
- 远端 HEAD 为 `c14d68c93`，base 为 `98e717275`；diff 严格限于 block 的 `bio.rs`、`partition.rs` 和 `request_queue.rs`。
- 历史最终验证曾在 `fork_Asterinas:/root/pr-tree` 完成：`make check`、block crate ktest（6/0）、`make ktest`（245/0）与 `make kernel` 均通过；验证时的 `pr` HEAD 与 `origin/pr` 均为 `c14d68c93`。
- 当前分支采用语义化名称；fork 的旧 `pr` 引用已删除。旧 merge 历史已通过 `--force-with-lease` 从 fork 分支移除。

详见 [PR 1 档案](../log/PR/PR-1-mapped-bio-ranges.md)。

## 当前工作区边界

- `AGENTS.md` 的 upstream 评审规则已恢复为本地协作规则。
- `.claude/dm-pr1-bio-migration.patch` 与 `patches/pr1-mapped-bio-ranges/` 是本地 PR 提取备份；不进入 upstream PR。
- `docs/plan.md`、`docs/study.md`、DOCX 来源与替换关系仍待人工审阅；不得与生产代码或 PR 文档同步混合提交。

## 下一步

1. PR 1 已完成；只有其 base、HEAD 或三文件范围变化时，才同步 `main` 并在 `mapped-bio-ranges` 分支重新验证。
2. 后续上游系列已按 PR 1 的独立、最小闭环标准复审：PR 2 不依赖 PR 1；PR 3 依赖 PR 1；PR 4 依赖 PR 2；PR 5 依赖 PR 1 与 PR 2；PR 6 仅依赖 PR 5 并承载 control-device primitives；PR 7 依赖 PR 4 与 PR 6，承载 table lifecycle、首次 primary/alias 节点事务和 WAIT；zero/error 与 striped 分拆为 PR 8、PR 9。完整边界与依赖图以 [PR 规范](pull_request.md#3-规划中的上游-pr-系列与依赖) 为准。
3. **PR 2 核心 lease 范围已获授权推进**：权威 `dm` 已实现并定向验证 registry tracked lease 的无重入按名称查找、rootfs 的 tracked lease 传递，以及预解析 VFS context 的生命周期保护。PR 2 只提取 live registry lease、普通 VFS mount/rootfs 与 ext2/exfat holder；Pending/Removing token、runtime node、MlsDisk facade 生命周期和 VirtIO 分区刷新事务均排除，分别留给 PR 4 或后续独立阶段。完成所有核心直接验证后，同步 `main` 到当前 upstream，从 `main` 直接创建 `block-device-leases`，迁移最小生产 diff 并在该分支完成最终验证、提交和 push；此时才为 PR 2 创建独立档案。

## 安全规则

- 写入、测试、提交前先核对 `git status --short`。
- 不覆盖、回退、格式化或提交来源不明的改动。
- `dm` 的构建和测试默认在 `fork_Asterinas:/root/asterinas` 中进行；新 PR 在已同步的 `main` 上直接创建语义化分支，并在该分支完成验证、提交与 push。
- QEMU、ktest、C regression 与 NixOS system test 必须串行。
- 默认不 push。