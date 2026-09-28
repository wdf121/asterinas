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
| 4 | [PR 1 档案](../log/PR/PR-1-mapped-bio-ranges.md) | 获取当前已推送 PR 的范围、Git 状态、英文描述和验证待办。 |
| 5 | `git status --short` | 区分分支提交、文档恢复改动和来源待确认文件。 |

## 当前停点

### 权威 `dm` 已同步最新 main

- `main` 已快进至 `upstream/main` 的 `98e717275`。
- `dm` 已无冲突合并该 main，当前合并提交为 `5f29d2270`。
- `dm` 仍是生产实现、修复和直接测试的唯一权威来源；PR 分支不得领先其行为。

### PR 1 已推送，最终验证待完成

- PR 编号为 **PR 1**；实际 branch 为 `dm-pr1a-mapped-range`。
- 远端 HEAD 为 `c14d68c93`，base 为 `98e717275`；diff 严格限于 block 的 `bio.rs`、`partition.rs` 和 `request_queue.rs`。
- 旧 merge 历史已通过 `--force-with-lease` 从 fork 分支移除。
- `make check`、`make ktest`、`make kernel` 尚未在最终 SHA 上重跑。历史验证仅作背景，不能将 PR 标记为最终验证通过。

详见 [PR 1 档案](../log/PR/PR-1-mapped-bio-ranges.md)。

## 当前工作区边界

- `AGENTS.md` 的 upstream 评审规则已恢复为本地协作规则。
- `.claude/dm-pr1-bio-migration.patch` 与 `patches/pr1-mapped-bio-ranges/` 是本地 PR 提取备份；不进入 upstream PR。
- `docs/plan.md`、`docs/study.md`、DOCX 来源与替换关系仍待人工审阅；不得与生产代码或 PR 文档同步混合提交。

## 下一步

1. 在 PR 1 最终 SHA 上完成 `make check`、`make ktest`、`make kernel`，逐项回填 PR 1 档案。
2. 若最终 SHA/base 变化，重新核对 diff、验证和远端状态。
3. PR 2 保持审核完成、尚未决定制备的状态；除非用户授权，不主动推进。

## 安全规则

- 写入、测试、提交前先核对 `git status --short`。
- 不覆盖、回退、格式化或提交来源不明的改动。
- 构建和测试默认在 `myAsterinas` 容器的 `/root/asterinas` 中进行。
- QEMU、ktest、C regression 与 NixOS system test 必须串行。
- 默认不 push。