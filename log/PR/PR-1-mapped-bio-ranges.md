# PR 1：mapped BIO ranges / `block: add mapped BIO ranges`

## 1. 状态与标识

| 项目 | 事实 |
|---|---|
| 状态 | 本地已重放到新 main；用户反馈新基线验证通过，原始输出未归档。旧 c14d68c93 曾完成验证与推送。 |
| 实际 Git branch | `mapped-bio-ranges` |
| 当前本地 base / HEAD | `10a241870` / `67ed6c70f` |
| 历史 base / HEAD | `98e717275` / `c14d68c93` |
| 最近有推送证据的远端 HEAD | `c14d68c93`；本轮未复核远端，也未推送新 SHA。 |
| 提交主题 | `block: add mapped BIO ranges` |
| 远端事件 | 已通过 `git push --force-with-lease` 更新 fork 分支，移除旧 merge 历史。 |
| 依赖 PR | 无。 |

本文档使用稳定编号 **PR 1**；当前分支名为 `mapped-bio-ranges`。

## 2. Git 与远端历史

| 日期 | 事件 | base | HEAD | 结论 |
|---|---|---|---|---|
| 2026-09-28 | 将原 block 补丁线性重放到 source `main` | `98e717275` | `c14d68c93` | 无冲突；只保留单一 block 提交。 |
| 2026-09-28 | 安全强制更新 fork 分支 | `98e717275` | `c14d68c93` | `--force-with-lease` 成功移除旧 merge 历史。 |
| 2026-09-28 | 容器内最终 SHA 验证 | `98e717275` | `c14d68c93` | `HEAD` 与 `origin/pr` 一致；完整 PR gate 通过。 |
| 2026-09-28 | 语义化分支迁移 | `98e717275` | `c14d68c93` | fork 从 `pr` 创建并切换为 `mapped-bio-ranges`，随后删除旧 `pr`；提交内容不变。 |

2026-10-09 执行 `git rebase main` 无冲突，`c14d68c93` 重放为 `67ed6c70f`。祖先检查退出码 0，`main..mapped-bio-ranges` 无 merge commit，`git range-diff 98e717275..c14d68c93 main..mapped-bio-ranges` 显示 `=`，`git diff --check` 通过。此次安全备份随后按用户要求删除，PR 2 从新 HEAD 建立。

重放后 diff 仍仅修改：

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

### 4.1 BIO 对象模型与学习要点（补充）

本节是对本 PR 数据模型的学习/审阅补充，不改变第 3 节的正式范围和第 5 节的历史验证结论。

#### 生命周期与共享边界

`Bio` 是尚未交给 block device 的逻辑 I/O 请求：它持有请求类型、调用者可见的 sector range、数据 `segments` 和可选 completion callback。提交时，`Bio` 被消费并转换为 `SubmittedBio`；后者持有执行 I/O 所需的 `segments` 与 callback。完成时释放 segments、运行 callback、发布最终状态。

`BioMetadata` 被 `Arc` 共享给 `SubmittedBio` 与等待方：它保存请求身份（`type_`、logical range）以及跨异步完成边界的状态（`status`、`wait_queue`）。因此它不是 request data buffer 的所有者，而是请求身份与完成状态的共享锚点。

```text
Bio ──submit──> SubmittedBio ──queue/driver──> complete
  └────────────── Arc<BioMetadata> ──────────────┘
                   ↑
                IoBatch/waiter
```

#### 地址层级

| 坐标 | 所在对象 | 可变性 | 含义 |
|---|---|---|---|
| logical range | `BioMetadata::sid_range` | 不可变 | 调用者最初向当前设备请求的范围 |
| mapped range | `SubmittedBio::mapped_sid_range` | 可变 | 当前 block layer 转发给下一层的范围 |
| queue-visible range | `BioRequest::sid_range` | 可因合并扩展 | 当前 queue 所服务设备实际处理的范围 |

提交时 mapped range 从 logical range 复制初始化。partition 或后续 target 只变换 mapped range；新模型中的 request queue 直接使用该范围。因而“物理地址”只应理解为**当前设备层可处理的地址**，而不是分层栈中的全局绝对坐标。旧模型则保存 logical range 加单一 `sid_offset`，由 queue 临时计算其可处理范围。

#### `status` 与 `wait_queue`

`status` 是完成状态而不是调度位置：正常路径为 `Init → Submit → Complete/NotSupported/NoSpace/IoError`。`submit` 的 CAS 防止重复提交；enqueue 失败时状态回退为 `Init`。完成方在 callback 后以 release 语义发布最终状态，等待方以 acquire 语义读取它，从而将完成结果和此前 callback 的效果交给等待方。

`wait_queue` 是等待 I/O 完成的任务队列：当状态仍为 `Submit`，等待方睡眠；完成方写入最终状态后 `wake_all`，等待方重新检查状态并映射为 I/O 结果。它不同于 `BioRequestSingleQueue`：后者保存等待 driver 消费的 I/O 请求，用于排队与合并；前者只负责通知提交者完成。

#### `BioSegment` 与长度汇总

一个 `BioSegment` 是带方向信息的 DMA 可访问有效内存 slice。DMA backing memory 按 block/page 粒度分配和池化（当前 pool 是每方向 `4096 × 4 KiB = 16 MiB`），而 block I/O 按 512 B sector 寻址和计量；segment 的有效 offset 与长度必须 sector 对齐。BIO/SubmittedBio 持有 segment 直到完成或丢弃，此后 backing memory 才能被回收或复用。`Vec<BioSegment>` 表示一次 I/O 可使用多段有效内存，不意味着每个 page 必然是一个 segment。

`Bio::new` 通过每个 segment 的**有效**长度得到 `nsectors`，不以 backing allocation capacity 代替实际传输长度。PR 1 将原本的直接 `.sum()` 改为 `try_fold(0_u64, u64::checked_add)`，并对 logical range 终点使用 checked arithmetic。对于已分析的 pool 上界，单 BIO 的有效长度至多为 `32768 = 2^15` 个 sector，远低于 `u64` 的表示上界；因此这两处构造期 hardening 的可达性尚无支撑，且不直接服务于 mapped-range 主线。仍存在非 pool 映射入口，尚未完成全部调用路径审计；本记录只保留该范围判断，未据此修改源码或改变 PR 结论。

## 5. 验证证据

### 当前本地 HEAD `67ed6c70f`

用户在 rebase 后明确反馈验证已通过，但对话未提供实际命令输出、测试数量及退出码。记录为用户反馈，不将旧 HEAD 的 6/0、245/0 等结果移用到新 SHA，也不编造当前验证日志。本轮助手仅执行上节列出的 Git 范围和历史检查。

### 历史已验证 HEAD `c14d68c93`

该验证是历史上在 `fork_Asterinas:/root/pr-tree` 完成的：当时工作树检出分支 `pr`，验证后确认其 clean，且 `HEAD` 与 `origin/pr` 均为 `c14d68c93`；随后按当时流程删除临时工作树。该历史流程不作为新制备模板；当前采用上一份 PR + 最新已核对 main 的线性分支规则。

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

1. 若 `mapped-bio-ranges` 的 base、HEAD 或三文件范围变化，先同步 `main` 到当前 upstream，再直接在该分支完成 rebase 与完整 PR gate；不得复用本次结果。
2. PR 2 现为最小单段 linear 数据面，基于更新后的 PR 1；通用 tracked lease 路线已暂缓。
3. 下方英文维护者描述保留旧 SHA 的历史验证事实；实际再次发布前，须按新 SHA 的证据更新，不能当作 67ed6c70f 的完整验收描述直接发布。

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