# Device Mapper 上游同步记录

> **状态**：进行中；本文仅记录本地 `dm` 分支跟进 upstream 的同步决策和实际状态，不属于 upstream PR 内容。
>
> **最后更新**：2026-09-23。

## 1. 目的与分工

`dm` 分支长期开发 Device Mapper 时，upstream `main` 会持续演进。同步不能只依赖一次 merge 的冲突标记：自动合并也可能掩盖 API 或生命周期语义不兼容。

本文记录每次同步的基线、冲突、决策、验证和待定项，使后续工作能区分：

```text
pull_request.md
= 哪些代码应如何拆成 upstream PR

upstream-sync.md
= dm 如何吸收 upstream 演进，以及每个冲突的同步决策

log/daily/YYYY-M-D.md
= 当天实际执行的修改、测试、失败与结果
```

本文不复制完整终端输出、临时 patch 或未经验证的推测。

## 2. 同步规则

1. 同步前记录 `dm` HEAD、`upstream/main` 和 merge-base；原研发工作区未提交改动必须隔离。
2. 可以使用独立 worktree 预演 merge/rebase、收集冲突和验证方案，但它只用于审核，不能成为后续 PR 的权威实现来源。
3. 正式同步必须回到权威 `dm` 工作区完成：在最新 `upstream/main` 基础上处理已确认方案、运行相关验证、由用户决定后创建同步 commit。未完成这一步时，禁止在 PR worktree 中应用或测试 PR patch。
4. 任何文本冲突、API 语义冲突或自动合并后的行为冲突，都必须先汇报：涉及文件、upstream 与 `dm` 行为、可选方案、是否需要回同步 `dm`。
5. 未经用户决定，不强行选择一侧、提交 merge result，或只修预演/PR worktree 而不处理 `dm` 分支的对应分叉。
6. 每个冲突组完成代码修改后立即停下汇报；仅在用户确认后运行该组最小测试。
7. 每次实际修改和验证完成后更新本文与当天 daily log；PR 计划变化同步更新 `pull_request.md`。

## 3. 当前同步基线

| 项目 | 当前值 | 说明 |
|---|---|---|
| `dm` 已提交 HEAD | `05ab2beed9d0c0ccdc081b0db1d2c271e7f51593` | 原研发分支；工作区另有未提交改动，未纳入同步。 |
| `upstream/main` | `ac790aa89` | 已通过 `git fetch upstream` 更新。 |
| merge-base | `604948581512d83734377974d4c34adb4530f2d7` | `dm` 与当前 upstream 的共同基线。 |
| PR 1 worktree | `.claude/worktrees/dm-pr1-bio` | 基于 `upstream/main` 的干净 `dm-pr1-bio` 分支，尚未应用补丁。 |
| 同步 worktree | `.claude/worktrees/dm-sync-upstream` | `dm-sync-upstream` 从 `dm` HEAD 创建；执行 `merge --no-commit --no-ff upstream/main` 后停在冲突状态。 |

## 4. 当前冲突分组

| 组别 | 文件或范围 | 当前状态 | 处理原则 |
|---|---|---|---|
| block BIO 与 partition | `bio.rs`、`partition.rs`、`request_queue.rs` | 已发现 `sid_offset` 与 current-range 模型冲突。 | 采用“不可变逻辑 metadata + 可变 current range”统一模型，保留任意 DM logical-to-backing 映射和 split 聚合。 |
| block 生命周期与公开 API | `block/src/lib.rs`、`device_id.rs` | lease 状态机本身未冲突；re-export、可见性与名称 API 有冲突。 | 保留 lease 的 Pending/Live/Removing 语义；采纳 upstream 的 allocator 可见性收敛，DM 所需命名-major API 在统一导出处处理。 |
| 稳定 primary 与可变 alias | `BlockDevice::name`、`DmDevice`、DM runtime registry/manager | 发现 Git 未标记的语义冲突。 | 见第 5 节；不得机械以 `&str` 替换动态 alias。 |
| 驱动与 runtime registry | virtio、NVMe、`device/mod.rs`、`registry/block.rs`、`registry/char.rs` | 待审计。 | 在 block API 和 primary/alias 分层稳定后处理。 |
| 构建与环境 | `Cargo.lock`、NixOS systemd、initramfs Nix/Makefile、QEMU args、TSC | 待审计。 | 与生产 DM 语义分离；不得为消除冲突而带入无关本地环境偏好。 |

## 5. 已确认的设计决策

### 5.1 BIO 映射：逻辑 metadata 与 current range 分离

upstream 当前 `SubmittedBio` 保存不可变逻辑 `sid_range` 和单个无符号 `sid_offset`，由 request queue 在末端物理化；partition 设置 offset。`dm` 已使用可变 current range，并提供绝对 remap、累加 offset、跨 target split 和 child completion 聚合。

单个、可覆盖的无符号 offset 无法完整表示 DM 的任意 logical-to-backing 映射：当 `backing_start < logical_start` 时，不能安全地以无符号差值表示；多层映射也不能只依赖覆盖式 partition offset。

**决策**：保留不可变逻辑 metadata，同时将 current range 作为每层实际处理的可变坐标；request queue 消费 current range。该模型必须保留 DM 的 length/overflow、linear/striped、split、首错锁存和单次完成语义。该方案仅在 `dm-sync-upstream` 预演工作树中实现，后续编译暴露了关联 VFS 接口冲突，验证尚未完成；不得作为权威 `dm` 或 PR 基线。

### 5.2 稳定 BlockDevice primary 与可 rename mapper alias 分离

upstream `BlockDevice::name() -> &str` 适合生命周期稳定的内核设备名。DM 的 `dmsetup rename` 操作的是 `/dev/mapper/<name>` alias；当前 `DmDevice` 将它存为 `Mutex<String>`，通过 clone 支持并发 rename/read，不能安全借用为 `&str`。

**决策**：

```text
BlockDevice::name() -> &str
= 稳定 primary 内核名，例如 dm-N

DmDevice::mapper_name() -> String
= 可变 dmsetup 名称；供 control、manager、runtime alias 发布/rename 使用
```

runtime registry 已独立创建 `/dev/dm-N` primary，并独立发布/记录/移动 `/dev/mapper/<name>` alias，因此此分层不改变 `dmsetup rename`、primary/alias 回滚或 I/O。

该分层方案仅在 `dm-sync-upstream` 预演工作树中实现，尚未完成验证或回写权威 `dm` 工作区。重构必须将 immutable primary-name 与 locked mapper-alias 分开存储，并同步所有 `BlockDevice` 实现与测试 fixture 的 `String -> &str` API；不得取消 rename 或暴露借用的可变 alias。

## 6. 当前停点

`dm-sync-upstream` 仅保留为同步预演与冲突审核现场，不是权威 `dm` 基线，也不能作为 `dm-pr1-bio` 或其他 PR worktree 的补丁来源。原 `dm` 工作区仍未被本次预演修改。

下一正式阶段不是继续在 PR worktree 提取代码，而是先在权威 `dm` 工作区完成经审核的最新 upstream 同步、串行验证并形成同步 commit；随后才以该同步后的实现制备 PR 1 或其他最小上游补丁。
