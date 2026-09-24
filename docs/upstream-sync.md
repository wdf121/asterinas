# Device Mapper 上游同步记录

> **状态**：2026-09-24 的正式同步已提交；本文继续记录后续同步相关修复与验证，不属于 upstream PR 内容。
>
> **最后更新**：2026-09-24。

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
4. 每次同步按固定依赖顺序推进：先配置与构建入口（Cargo/OSDK/Nix/initramfs/QEMU/CI），再通用基础设施（block/VFS/devtmpfs/registry/驱动），再 DM 生产语义，最后测试与验收。前一组没有可用构建契约或未关闭的取舍时，不进入后一组。
5. 任何文本冲突、API 语义冲突或自动合并后的行为冲突，都必须先汇报：涉及文件、upstream 与 `dm` 行为、可选方案、是否需要回同步 `dm`。
6. 每个冲突或设计取舍必须先写入本次同步的决策记录，再实现选择的方案；未经用户决定，不强行选择一侧、提交 merge result，或只修预演/PR worktree 而不处理 `dm` 分支的对应分叉。
7. 每个冲突组完成代码修改后立即停下汇报；仅在用户确认后运行该组最小测试。
8. 每次实际修改和验证完成后更新本文与当天 daily log；PR 计划变化同步更新 `pull_request.md`。

## 2.1 本次同步决策记录

每个冲突组或需要设计取舍的自动合并差异，都在本节新增一条记录；记录先于代码修改，并在向用户汇报时引用。不要用“选 ours/theirs”替代语义说明。

| 字段 | 必填内容 |
|---|---|
| 分组与范围 | 所属同步阶段、文件和关键符号。 |
| 两侧行为 | upstream 与 `dm` 的原行为、接口或生命周期差异。 |
| 决策与理由 | 选择保留、合并或重构什么，以及为何满足上游与 DM 约束。 |
| 影响与回同步 | 对 `dm`、后续 PR、用户可见行为、锁/资源/失败路径的影响。 |
| 验证与状态 | 最小测试入口、实际结果、未关闭风险或待用户确认项。 |

## 2.2 2026-09-23 权威 `dm` 正式同步：决策与合并结果

在 WIP `604942896` 之后，权威 `dm` 执行了 `merge --no-commit --no-ff upstream/main`。下表记录各冲突组在实施时作出的决策及其文本合并结果；表中“尚未编译”或“留待编译”仅描述该组完成合并时的阶段状态。所有列出的 Git 冲突已在权威 `dm` 解决，并于 `a4603e369` 创建同步 merge commit；最终 DM 专项验证结果以第 2.3 节为准。

| 优先级与范围 | upstream 与 `dm` 差异 | 决策与验证 |
|---|---|---|
| 配置：`Cargo.lock` | upstream 锁图引入 DRM、AArch64 和新的 Git tag；`dm` 需保留 `aster-device-mapper`，旧冲突块含过期压缩依赖。 | **决策：**不手工拼锁图；以 upstream 锁图为临时基线，待本组 manifest/配置合并后在容器在线重生成，并验证 `--locked`。 |
| 配置：NixOS systemd | upstream 使用 util-linux `loginProgram` 与 `systemd.settings.Manager`；`dm` 禁用 `serial-getty@hvc0`、只保留 `autovt@hvc0` 以稳定 guest ready。 | **决策：**同时保留两侧行为；验证 Nix 求值与 guest ready。 |
| 配置：initramfs Makefile/Nix | upstream 增加 benchmark、conformance selector、共享 nixpkgs、AArch64/system；`dm` 增加 `INTEL_TDX -> enableTdxAttest` 显式门控。 | **决策：**采用 upstream 参数全集合并保留显式 TDX 门控；验证 non-TDX focused regression 不拉取 TDX。 |
| 配置：QEMU 参数 | upstream 将 `qemu-direct` 更名为 `vmm-direct`，增加 OVMF_DIR、PVH、AArch64、VMX；`dm` 保留 ENABLE_KVM 与 FORCE_OVMF。 | **决策：**采用 upstream `vmm-direct`/OVMF_DIR/PVH/AArch64/VMX，同时保留 KVM 与 FORCE_OVMF；验证源码同版本 OSDK、QEMU 参数和启动。 |
| 启动基础：TSC | upstream 仅收窄可见性；`dm` 添加 QEMU/KVM 频率来源链、10 GHz 上限和 2.5 GHz fallback。 | **决策：**保留 dm 防异常频率策略，并采用 upstream 可见性收敛；验证 QEMU 与非 QEMU 时间/启动行为。 |
| 通用基础设施：BIO/lease/partition | `kernel/core/comps/block/{bio.rs,lib.rs,partition.rs}` | 已在权威 `dm` 完成文本合并、固定 nightly 格式和限定 diff 检查；尚未编译。 | 保留 DM 不可变逻辑 metadata + 可变 current range、absolute remap、可组合 offset、跨 target split 和完成聚合；吸收 upstream `DmaBuffer`、`PartitionManager`、stable `BlockDevice::name() -> &str` 和可见性收敛；lease Pending/Live/Removing 不变。 |
| 通用基础设施：driver | `kernel/core/comps/{nvme,virtio}` 的 block device 与 VirtIO `DmaBuf`。 | 已在权威 `dm` 完成文本合并、固定 nightly 格式和限定 diff 检查；尚未编译。 | 采用 upstream partition manager、设备初始化和 stable `name() -> &str`；保留 discard/zeroout，移除 VirtIO GET_ID 无界循环，并增加 `DmaBuf for Slice<Arc<DmaBuffer<_>>>`。 |
| 其余 block consumers | MlsDisk、DM crate、block/registry 测试 fixture 的 `name() -> &str` 适配。 | **待决策：**需要逐类适配稳定 primary 与可变 alias，不能把 DM alias 借用为 `&str`。 |
| 通用基础设施：VFS/devtmpfs/registry | `device/mod.rs`、`registry/{block,char}.rs`、devtmpfs、RamFS、`vfs/path`。 | 已在权威 `dm` 完成文本合并、固定 nightly 格式和限定 diff 检查；devtmpfs ktest 为 6/0，runtime registry ktest 初始为 7/0。 | 使用 `DevtmpfsHandle` 绑定 inode identity 与路径；delete/rename 在目录锁内校验，`ESTALE` 保留外来替代对象。同步后审阅补充 alias 删除非 `ENOENT`/`ESTALE` 错误的 handle 保留与重试回归，当前 registry ktest 为 8/0。DM 保持 alias → primary → commit、失败恢复与 `Removing` 隔离；Dentry 条件操作按 dentry identity 并保留 `ESTALE`/`EXDEV`。 |
| 其余 block/runtime consumers | MlsDisk、DM crate、block/registry 测试 fixture、DM control-device 的 `name() -> &str` / `DevtmpfsNodeMeta` 适配。 | **决策：**普通固定名称返回 `&str`；`DmDevice` 分离 immutable `dm-N` primary 和 locked mapper alias，manager/control/runtime 使用 `mapper_name()` 快照；control-device 改用 `DevtmpfsNodeMeta`。 | 完成后首次运行同步后的 core 编译检查；QEMU/ktest 留待编译通过。 |

## 2.3 2026-09-24 正式同步验证结果

以下结果只覆盖本次同步受影响的 Device Mapper 专项矩阵，不等同于全仓 CI、全量非 DM regression、AArch64 或非 QEMU 机器验证。

| 层次 | 实际结果 | 覆盖边界 |
|---|---|---|
| kernel-aware 编译 | 源码同版本 OSDK：`kernel/core` 的 `osdk check --ktests` 通过。 | 同步后的 core/ktest 编译图。普通 host `cargo check` 不具备内核 target 配置，不作为结论。 |
| devtmpfs ktest | `aster_core::fs::fs_impls::devtmpfs::tests`：6 passed，0 failed。 | node/symlink、同 rdev 外来替换、identity handle delete/rename。 |
| runtime registry ktest | `aster_core::device::registry::block::tests`：8 passed，0 failed。 | primary 创建失败、alias recovery、alias 删除失败后的 handle 保留与重试、open gate、`Removing` 隔离。 |
| block ktest | `aster-block`：23 passed，0 failed。 | BIO、partition、request queue、lease。 |
| DM crate ktest | `aster-device-mapper`：86 passed，0 failed。 | table、target、manager、BIO 生命周期。 |
| core DM ioctl ktest | `aster_core::device::misc::device_mapper::tests`：81 passed，0 failed，134 filtered。 | control ioctl、WAIT、rename、table lifecycle、readonly。 |
| focused C ABI | `REGRESSION_TESTS=device/device_mapper`：182 passed，0 failed。 | raw ioctl、node/alias、mount lease、range、WAIT/SA_RESTART、readonly。同步中发现 kernel `DM_VERSION` 4.48 与 Linux UAPI 4.50 不一致，收敛 kernel 返回值为 4.50 后通过。 |
| NixOS canonical suites | control-plane、dataplane、LVM2 topology、linear、striped、mixed 全部通过。 | 真实 dmsetup/LVM2/ext2/文件 I/O、扩缩容和有序关机后的跨启动恢复；不是断电一致性测试。 |

构建和运行中仍有 upstream/现有 warning：devtmpfs 的冗余限定、未使用 identity helper、procfs visibility，以及宿主 CPU feature warning；它们未阻断本次 DM 专项矩阵，不在本同步阶段顺带重构。

## 2.4 同步后审阅修复：alias 删除失败的 handle 保留

同步 merge commit 后的审阅发现：`unregister_mapper` 在 identity 删除 alias 成功前取走 `DevtmpfsHandle`。当删除返回非 `ENOENT`/`ESTALE` 错误时，设备会恢复 `Live`，但丢失 alias 的身份记录；后续 rename 会返回 `ESTALE`，重试 remove 可能遗留 `/dev/mapper/<name>`。

**修复**：`devtmpfs::delete` 改为借用 handle，registry 仅在删除成功后清除 primary/alias 记录；测试通过内部删除回调注入 `EIO`，验证恢复 `Live` 后 alias 仍可由下一次 remove 清理。

**实际验证**：重装源码同版本 OSDK 后，使用标准命令运行 `aster_core::device::registry::block::tests`（8 passed、0 failed）与 `aster_core::fs::fs_impls::devtmpfs::tests`（6 passed、0 failed）。首次 host 尝试因缺少 `cargo-osdk` 未启动；首次容器构建暴露 devtmpfs 既有测试的按值调用，修正后新用例因 selector 未初始化 `devtmpfsd` 挂起，显式初始化测试 worker 并清理遗留 QEMU 后复跑通过。


## 3. 当前同步基线

| 项目 | 当前值 | 说明 |
|---|---|---|
| 权威 `dm` HEAD | `cdefc74dc` | `a4603e369` 为正式 upstream 同步 merge commit；当前 HEAD 记录该同步的专项验证。 |
| `upstream/main` | `ac790aa89` | 已通过 `git fetch upstream` 更新。 |
| merge-base | `604948581512d83734377974d4c34adb4530f2d7` | 预演开始时 `dm` 与当前 upstream 的共同基线。 |
| PR 1 worktree | `.claude/worktrees/dm-pr1-bio` | 基于 `upstream/main` 的干净 `dm-pr1-bio` 分支，未应用 PR patch、未作为同步来源。 |
| 同步预演 worktree | `.claude/worktrees/dm-sync-upstream` | 从预演前 `05ab2beed` 创建并合入 `upstream/main`；仅保存未提交的冲突解决和编译排障现场。 |
| 容器验证 worktree | `/root/asterinas/.claude/worktrees/dm-sync-upstream-verify` | 从容器内 `dm` HEAD 创建并应用预演 delta；含仅用于验证的临时 OSDK workspace/lockfile，不回写权威 `dm`。 |

## 3.1 已执行的预演事件与验证

1. 在 `dm-sync-upstream` 中对预演前 `dm` HEAD 执行 `merge --no-commit --no-ff upstream/main`；文本冲突覆盖 block/BIO、设备 registry、驱动、TSC、Nix/initramfs/QEMU 和 lockfile。预演不产生 merge commit，也没有回写当时的权威 `dm` 工作区。
2. 预演中实现了 stable `dm-N` primary 与可 rename mapper alias 分层、current-range BIO 映射、devtmpfs inode-identity handle、runtime registry handle 生命周期和相应配置合并。它们都是待验证的预演实现，不是已完成同步结论。
3. 为验证预演 delta，创建容器验证 worktree。验证过程先发现容器全局 `cargo-osdk` 只识别旧 `qemu-direct`；改用验证 worktree 构建的 upstream 本地 OSDK 后，离线缓存缺少新的 `smoltcp` tag，在线获取成功。
4. 实际 `aster_core::fs::fs_impls::devtmpfs::tests` 构建推进到 block、virtio 与 Device Mapper crate。期间发现并修正预演 BIO 的缺失 `SpinLock` 导入、DMA slice 共享所有权和 virtio `DmaBuf` 兼容；随后在 VFS `dentry.rs` 发现自动合并将已适配的新 Inode/Dentry API 回退为旧调用形式，导致编译失败。该 VFS 冲突尚未解决，因此没有任何 ktest 结果可标记为通过。
5. 上述预演发生在“配置先行、先记录取舍”的规则固化之前；后续正式同步必须按第 2 节顺序重新执行，不能直接采纳预演 worktree 的未提交结果。

## 4. 预演发现的冲突分组（历史）

下表记录预演阶段如何识别同步冲突；当前权威 `dm` 已按第 2.2 节的决策完成这些组的文本合并，最终验证以第 2.3 节为准。

| 组别 | 文件或范围 | 当前状态 | 处理原则 |
|---|---|---|---|
| 配置与构建入口 | `Cargo.lock`、NixOS systemd、initramfs Nix/Makefile、QEMU args、TSC | 已在权威 `dm` 完成文本合并；固定 nightly 的 TSC 格式、QEMU shell 和限定 diff 检查通过；`Cargo.lock` 已在容器在线重生成。功能启动验证待同步后分层测试。 | 采用 upstream 构建契约，保留经确认的 TDX 门控、KVM/OVMF、guest-ready 和 TSC 保护行为。 |
| block BIO 与 partition | `bio.rs`、`partition.rs`、`request_queue.rs` | 预演实现并在编译中修正 DMA 兼容；仍未完成完整验证。 | 采用“不可变逻辑 metadata + 可变 current range”统一模型，保留任意 DM logical-to-backing 映射和 split 聚合。 |
| block 生命周期与公开 API | `block/src/lib.rs`、`device_id.rs` | 预演保留 lease 状态机并合并导出/可见性；未完成验证。 | 保留 Pending/Live/Removing；DM 所需命名-major API 在统一导出处处理。 |
| 稳定 primary 与可变 alias | `BlockDevice::name`、`DmDevice`、DM runtime registry/manager | 预演实现；未回写权威 `dm`。 | 见第 5 节；不得机械以 `&str` 替换动态 alias。 |
| VFS/devtmpfs/runtime registry | `device/mod.rs`、`registry/block.rs`、`registry/char.rs`、devtmpfs、`vfs/path` | devtmpfs handle/registry 已预演；VFS `dentry.rs` 自动合并回退了新 Inode/Dentry API，当前编译阻断。 | 必须按 Dentry identity 合并 VFS 新接口、notify 和 `ESTALE`/`EXDEV` 边界后再验证。 |

## 5. 已确认的设计决策

### 5.1 BIO 映射：逻辑 metadata 与 current range 分离

upstream 当前 `SubmittedBio` 保存不可变逻辑 `sid_range` 和单个无符号 `sid_offset`，由 request queue 在末端物理化；partition 设置 offset。`dm` 已使用可变 current range，并提供绝对 remap、累加 offset、跨 target split 和 child completion 聚合。

单个、可覆盖的无符号 offset 无法完整表示 DM 的任意 logical-to-backing 映射：当 `backing_start < logical_start` 时，不能安全地以无符号差值表示；多层映射也不能只依赖覆盖式 partition offset。

**决策**：保留不可变逻辑 metadata，同时将 current range 作为每层实际处理的可变坐标；request queue 消费 current range。该模型保留 DM 的 length/overflow、linear/striped、split、首错锁存和单次完成语义。它已在权威 `dm` 中完成同步，并由 block 23/0、DM crate 86/0、core ioctl 81/0、focused C 182/0 与六项 NixOS suite 共同验证。

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

该分层方案已在权威 `dm` 完成实现。除源码同版本 OSDK 的 `osdk check --ktests` 外，已通过 block 23/0、DM crate 86/0、core DM ioctl 81/0、focused C ABI 182/0 与六项 canonical NixOS suite；完整结果与覆盖边界见第 2.3 节。实现保留 immutable primary-name 与 locked mapper-alias，并同步 `BlockDevice` 实现与测试 fixture 的 `String -> &str` API；不得取消 rename 或暴露借用的可变 alias。

## 6. 当前停点

正式 upstream 同步已由 `a4603e369` 提交，`cdefc74dc` 记录专项验证；同步预演/容器验证 worktree 保留为审计现场，不再作为实现来源。同步后 alias 删除失败修复已通过定向 ktest，但尚未创建后续修复 commit。

下一步由用户决定：审阅当前修复 diff 并创建修复 commit，或在提交前要求补充更广泛验证。该修复收口后，权威 `dm` 可作为 [pull_request.md](pull_request.md) 的 PR 制备实现来源。
