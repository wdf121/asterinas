# Device Mapper 对话交接

> **最后更新**：2026-09-28
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

- PR 编号为 **PR 1**；实际 branch 为 `pr`。
- 远端 HEAD 为 `c14d68c93`，base 为 `98e717275`；diff 严格限于 block 的 `bio.rs`、`partition.rs` 和 `request_queue.rs`。
- 在 `fork_Asterinas:/root/pr-tree` 上完成最终 SHA 验证：`make check`、block crate ktest（6/0）、`make ktest`（245/0）与 `make kernel` 均通过；完成后已确认 `HEAD == origin/pr == c14d68c93` 且工作树 clean。
- 临时 `/root/pr-tree` 已删除；本地与远端 `pr` 分支保留。旧 merge 历史已通过 `--force-with-lease` 从 fork 分支移除。

详见 [PR 1 档案](../log/PR/PR-1-mapped-bio-ranges.md)。

## 当前工作区边界

- `AGENTS.md` 的 upstream 评审规则已恢复为本地协作规则。
- `.claude/dm-pr1-bio-migration.patch` 与 `patches/pr1-mapped-bio-ranges/` 是本地 PR 提取备份；不进入 upstream PR。
- `docs/plan.md`、`docs/study.md`、DOCX 来源与替换关系仍待人工审阅；不得与生产代码或 PR 文档同步混合提交。

## 下一步

1. PR 1 若出现 base、HEAD 或范围变更，必须从更新后的 `pr` 创建 `/root/pr-tree`，重新运行完整 PR gate；当前结果只适用于 `c14d68c93`。
2. 当前 `dm` 工作区保留通用 NixOS 测试盘定位改动及其本地文档记录；后续在 `dm` 收敛并验证新增行为后，才裁剪新的 PR。
3. PR 2 保持审核完成、尚未决定制备的状态；除非用户授权，不主动推进。

## 安全规则

- 写入、测试、提交前先核对 `git status --short`。
- 不覆盖、回退、格式化或提交来源不明的改动。
- `dm` 的构建和测试默认在 `fork_Asterinas:/root/asterinas` 中进行；PR 验证固定从当前 `pr` 创建容器内 `/root/pr-tree`，完成 push/远端核对后删除该临时工作树。
- QEMU、ktest、C regression 与 NixOS system test 必须串行。
- 默认不 push。