# PR 1：mapped BIO ranges / `block: add mapped BIO ranges`

## 1. 状态与标识

| 项目 | 事实 |
|---|---|
| 状态 | 已推送；最终 SHA 验证待完成。 |
| 实际 Git branch | `dm-pr1a-mapped-range` |
| base | `98e717275` (`origin/main`) |
| 当前远端 HEAD | `c14d68c93` |
| 提交主题 | `block: add mapped BIO ranges` |
| 远端事件 | 已通过 `git push --force-with-lease` 更新 fork 分支，移除旧 merge 历史。 |
| 依赖 PR | 无。 |

本文档使用稳定编号 **PR 1**；`1a` 仅保留在历史 Git branch 名中，不作为 PR 编号。

## 2. Git 与远端历史

| 日期 | 事件 | base | HEAD | 结论 |
|---|---|---|---|---|
| 2026-09-28 | 将原 block 补丁线性重放到 source `main` | `98e717275` | `c14d68c93` | 无冲突；只保留单一 block 提交。 |
| 2026-09-28 | 安全强制更新 fork 分支 | `98e717275` | `c14d68c93` | `--force-with-lease` 成功移除旧 `Merge branch 'asterinas:main' into dm-pr1a-mapped-range` 历史。 |

最终 diff 仅修改：

```text
kernel/core/comps/block/src/bio.rs
kernel/core/comps/block/src/partition.rs
kernel/core/comps/block/src/request_queue.rs
```

## 3. 问题、目标与非目标

### 目标

- 保持 BIO 的原始 logical sector range 不变，同时允许每层 block device 持有自身可组合的 mapped sector range。
- 让 partition 将 data BIO 映射到 backing device 的分区起点；让 request queue 以当前设备可见的 mapped range 判断连续性和合并。
- 对映射溢出返回 `BioEnqueueError::Refused`，且不改变原 mapping。

### 非目标

- 不引入 discard、zeroout 或其他 range BIO 用户 ABI。
- 不引入 block-device lease、runtime/VFS node 生命周期或任何 Device Mapper 专用代码。
- 不包含本地 NixOS/LVM2/QEMU harness、文档、日志或 patch 快照。

## 4. 技术设计与范围

`BioMetadata::sid_range` 是 immutable logical range；`SubmittedBio::mapped_sid_range` 是当前 block 层可重映射的 range。`remap_sid_start` 和 `offset_mapped_sid_range` 在保留长度的前提下更新后者，溢出时拒绝且保持原值。

- `bio.rs`：建立 logical/mapped range 分层，提供组合映射和溢出保护。
- `partition.rs`：对提交给 backing device 的 BIO 叠加 partition 起始 sector。
- `request_queue.rs`：使用 mapped range 构造和合并 request，而不再另行叠加逻辑 offset。

审阅重点：logical 与 mapped range 不得混用；连续性判断必须以 queue 所见 range 为准；失败映射不得部分修改 state。

## 5. 验证证据

### 最终 HEAD `c14d68c93`

| 层次 | 命令 | 结果 | 状态 |
|---|---|---|---|
| 静态准入 | `make check` | 未在最终 SHA 执行。 | 待执行 |
| 内核测试 | `make ktest` | 未在最终 SHA 执行。 | 待执行 |
| 内核构建 | `make kernel` | 未在最终 SHA 执行。 | 待执行 |

### 历史背景（不能替代最终验证）

权威 `dm` 上的 block crate、DM crate 及早期 PR worktree 曾运行相关 ktest/构建，证明该补丁演进过程中的局部行为；这些结果对应旧 base 或旧 HEAD，不能作为 `c14d68c93` 的最终验证结论。

## 6. 已知边界与后续动作

1. 在最终 SHA `c14d68c93` 上串行运行 `make check`、`make ktest`、`make kernel`，并将每项命令、日期、环境和结果回填本节。
2. 若 final HEAD 或 base 再次改变，重新验证并追加 Git 事件；不得复用当前待执行状态外的旧结果。
3. PR 2 的 tracked device lease 保持独立审计和决策，不随 PR 1 扩大范围。

## 7. Upstream maintainer description

### Title

```text
block: add mapped BIO ranges
```

### Summary

- Keep the logical sector range of a BIO immutable while tracking a separate mapped range for the block-device layer currently handling the request.
- Make partition offsets compose with prior mappings and make request-queue merging use the range visible to the queue.
- Reject overflowing remaps without mutating the submitted BIO.

### Design

`BioMetadata` continues to own the original logical range. `SubmittedBio` owns a mutable mapped range, so each block layer can remap the request without losing the caller-visible logical coordinates. Partition devices offset that mapped range before forwarding the BIO, and request queues construct and merge requests directly from the mapped range.

This keeps remapping composable and removes the split representation where queues had to combine a logical range with a separate offset.

### Scope

Included:

- `kernel/core/comps/block/src/bio.rs`
- `kernel/core/comps/block/src/partition.rs`
- `kernel/core/comps/block/src/request_queue.rs`

Excluded: range BIO operations, device leases, runtime/VFS node lifecycle, Device Mapper code, and local system-test harnesses.

### Testing

Final-HEAD validation is pending for `c14d68c93`:

```text
make check
make ktest
make kernel
```

Historical tests on earlier bases are intentionally not presented as final-HEAD results.

### Review notes

Please focus review on the logical-versus-mapped range invariant, overflow rejection without partial mutation, and request-queue contiguity after layered remapping.