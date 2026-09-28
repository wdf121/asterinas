# PR 1：mapped BIO ranges / `block: add mapped BIO ranges`

## 1. 状态与标识

| 项目 | 事实 |
|---|---|
| 状态 | 已推送；最终 SHA 验证通过。 |
| 实际 Git branch | `pr` |
| base | `98e717275` (`main`) |
| 当前远端 HEAD | `c14d68c93` |
| 提交主题 | `block: add mapped BIO ranges` |
| 远端事件 | 已通过 `git push --force-with-lease` 更新 fork 分支，移除旧 merge 历史。 |
| 依赖 PR | 无。 |

本文档使用稳定编号 **PR 1**；实际分支名为 `pr`。

## 2. Git 与远端历史

| 日期 | 事件 | base | HEAD | 结论 |
|---|---|---|---|---|
| 2026-09-28 | 将原 block 补丁线性重放到 source `main` | `98e717275` | `c14d68c93` | 无冲突；只保留单一 block 提交。 |
| 2026-09-28 | 安全强制更新 fork 分支 | `98e717275` | `c14d68c93` | `--force-with-lease` 成功移除旧 merge 历史。 |
| 2026-09-28 | 容器内最终 SHA 验证 | `98e717275` | `c14d68c93` | `HEAD` 与 `origin/pr` 一致；完整 PR gate 通过。 |

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

执行环境为 `fork_Asterinas:/root/pr-tree`。工作树直接检出当前 `pr`，验证完成后确认其 clean，且 `HEAD` 与 `origin/pr` 均为 `c14d68c93`；随后按流程删除临时工作树。

| 层次 | 命令 | 结果 | 状态 |
|---|---|---|---|
| 范围与空白 | `git diff --check main...HEAD` | diff 仅为 `bio.rs`、`partition.rs`、`request_queue.rs`，无空白错误。 | 通过 |
| 格式 | `cargo fmt --check --all` | 格式检查通过。 | 通过 |
| 静态准入 | `make check` | Rust、C、Nix、拼写与聚合静态检查通过。 | 通过 |
| ktest 前提 | `make initramfs` | initramfs、`ext2.img` 与其他测试镜像构建完成。 | 通过 |
| block 专项 ktest | `cd kernel/core/comps/block && cargo osdk test` | 以标准命令运行，无离线或 console 覆盖；guest 中 6 passed，0 failed，覆盖 remap、offset、overflow、partition 与 queue merge。 | 通过 |
| 项目级 ktest | `make ktest` | 5 个 crate、245 项测试全部完成，无失败。 | 通过 |
| 内核构建 | `make kernel` | 非测试内核及 ISO 构建完成。 | 通过 |

### 历史背景

权威 `dm` 上的 block crate、DM crate 及早期 PR worktree 的测试仍只作为补丁演进背景；上述 final-HEAD 结果才是 `c14d68c93` 的 PR 验证事实。

## 6. 已知边界与后续动作

1. 若 `pr` 的 base、HEAD 或三文件范围变化，必须从更新后的 `pr` 创建新的 `/root/pr-tree`，重新执行完整 PR gate；不得复用本次结果。
2. PR 2 的 tracked device lease 保持独立审计和决策，不随 PR 1 扩大范围。

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

Validated on final PR HEAD `c14d68c93`:

```text
cargo fmt --check --all
make check
make initramfs
cd kernel/core/comps/block && cargo osdk test
make ktest
make kernel
```

The block ktest completed with 6 passed and 0 failed using the standard command without offline or console overrides. The project ktest completed 245 tests across 5 crates with no failures.

### Review notes

Please focus review on the logical-versus-mapped range invariant, overflow rejection without partial mutation, and request-queue contiguity after layered remapping. Please let me know if any part of the design or scope needs clarification. I’m happy to discuss it and will respond promptly.