# Device Mapper 上游同步记录

> **状态**：2026-09-24 已完成 `upstream/main` 同步、DM 专项验证、alias 删除失败修复及 split child 完成责任修复。
>
> **范围**：本文是本地 `dm` 分支的同步快照与后续同步规则；不属于 upstream PR 文档。

## 1. 当前结论

权威 `dm` 已同步到 `upstream/main` `ac790aa89`。同步后的 DM 专项验证已完成；当前实现可作为后续 PR 制备的唯一代码来源。

本次同步不等同于全仓 CI、完整非 DM initramfs regression、AArch64 或非 QEMU 机器验证。完整 C regression 的已知 network `MSG_PEEK` 失败不计入 DM 同步结论。

## 2. 正式历史

| 提交 | 定位 |
|---|---|
| `b38955a21` | 将 Device Mapper 只读模式限定为表级语义。 |
| `1b0cb772d` | 补充表级只读模式 C 与系统测试。 |
| `cd7186074` | 记录表级只读模式验证。 |
| `119f06fe8` | 新增交接与同步规划。 |
| `0d789a000` | 更新本地协作流程、学习资料与 DOCX。 |
| `a4603e369` | 合并 `upstream/main`。 |
| `cdefc74dc` | 记录已验证的上游同步。 |
| `0423431d5` | 修复 alias 删除失败后句柄丢失与重试失败。 |

同步预演与容器验证 worktree 仅保留为审计证据，不是后续实现来源。

## 3. 本次同步的关键取舍

| 主题 | 冲突或风险 | 选择 | 结果与边界 |
|---|---|---|---|
| 构建入口 | upstream 更新 OSDK、Nix/initramfs、QEMU boot method 和根依赖图；`dm` 同时依赖 TDX 门控、KVM/FORCE_OVMF 与 hvc0 guest-ready。只取一侧会丢失另一侧的构建或启动契约。 | 以 upstream OSDK、Nix/initramfs/QEMU 契约为底，保留 TDX 显式门控、KVM/FORCE_OVMF、hvc0 guest-ready 与 TSC 保护。 | 根 `Cargo.lock` 在容器在线重生成。`osdk/Cargo.lock` 没有 manifest/upstream 驱动变化，恢复为索引版本，不纳入同步。 |
| BIO 映射 | upstream 的单个无符号 `sid_offset` 在 partition 中可覆盖已有 offset，不能表示所有 DM logical-to-backing 映射；`backing_start < logical_start` 时无符号差值下溢。 | 保留不可变逻辑 `sid_range` 与可变 current range。 | 保留 absolute remap、可组合 offset、split 和完成聚合；block 25/0、DM crate 87/0、core ioctl 81/0、C 182/0 与六项 NixOS suite 验证。 |
| split child 完成 | `SplitBioCompletionHandle` 不绑定 child。deferred child 的 drop guard 已聚合 IoError 后，DM table 仍可手工补偿同一 child，导致计数双减、父 BIO 提前完成和 table-generation 保活提前释放。 | 移除脱离 child 的 completion handle；每个 split child 的 `complete` 或 drop 成为唯一完成来源。 | 本提交收口该修复：block 25/0、DM crate 87/0；PR 1 必须以此修复后的权威 `dm` 为来源。 |
| block 生命周期 | upstream registry/partition 演进与 `dm` 的 `BlockDeviceLease`、Pending/Live/Removing 生命周期交错；只保留普通 `Arc` 会允许仍被 table、mount 或 open 使用的设备被注销。 | 保留 `BlockDeviceLease` 状态机和 dm 所需 named-major API。 | mount、open、table backing 与 remove 保持长期使用权；完整 lease 扩展仍需在独立 PR 中审计。 |
| stable primary / mutable alias | upstream `BlockDevice::name() -> &str` 假设名称生命周期稳定；`dmsetup rename` 的 mapper alias 是可变 `String`，不能安全借用。直接把 alias 作为 trait name 会迫使通用块层拥有字符串。 | `BlockDevice::name()` 返回不可变 `dm-N` primary；`DmDevice::mapper_name()` 返回受锁保护的 alias 快照。 | manager/control/runtime 只使用 alias 快照，primary/alias 回滚语义不变；不得取消 rename 或暴露可变 alias 的借用。 |
| devtmpfs identity | upstream 按路径、节点类型和 rdev 删除，无法区分“同路径、同 rdev、但已被外部替换”的节点；旧 DM 的 `Path` 预查再删除/rename 又有 TOCTOU。 | 使用 `DevtmpfsHandle` 绑定创建时 inode identity 与当前路径。 | delete/rename 在目录锁内校验 identity；外来替换返回 `ESTALE` 且不删除/覆盖外物；devtmpfs ktest 6/0。 |
| remove/recovery | alias 删除返回非 `ENOENT`/`ESTALE` 时，旧实现过早取走 handle；设备恢复 `Live` 后丢失 alias 身份，rename/retry remove 可能失败或遗留 symlink。 | `devtmpfs::delete` 借用 handle，registry 仅在删除成功后清除 primary/alias 记录。 | alias → primary → commit；恢复失败维持 `Removing`。runtime registry ktest 8/0 覆盖 handle 保留与 retry。 |
| driver | upstream VirtIO/NVMe 采用 partition manager、稳定名称和新的 DMA 描述符路径；`dm` 需保留 discard/zeroout。旧 VirtIO GET_ID/host_id 路径在失败时无界循环。 | 采用 upstream partition manager/初始化/稳定名称，保留 discard/zeroout，添加 `DmaBuf for Slice<Arc<DmaBuffer<_>>>`，移除 GET_ID/host_id 路径；NixOS 测试盘改由固定 PCI block 拓扑和 cmdline 声明的首盘路径定位。 | DMA child slice 可进入 VirtIO descriptor；测试定位不再依赖私有 VirtIO ioctl。 |
| DM UAPI 版本 | focused C ABI 使用 Linux dm-ioctl 头文件期望 `4.50.0`，而 `dm` kernel 仍返回 `4.48.0`；这不是 upstream 改动，而是验证暴露的既有契约缺口。 | `DM_VERSION` 返回 `4.50.0`。 | focused C ABI 由 181/1 收敛为 182/0；版本号表示 ioctl ABI 契约，不表示完整 Linux DM target 集。 |

## 4. 验证矩阵

| 层次 | 实际结果 | 覆盖 |
|---|---|---|
| kernel-aware 编译 | 源码同版本 OSDK：`kernel/core` `osdk check --ktests` 通过。 | 同步后的 core/ktest 编译图。普通 host `cargo check` 不具备 kernel target 配置，不作为结论。 |
| devtmpfs ktest | 6 passed，0 failed。 | node/symlink、同 rdev 替换、handle delete/rename。 |
| runtime registry ktest | 8 passed，0 failed。 | primary 创建失败、alias recovery、alias 删除失败 handle 保留、open gate、`Removing` 隔离与 retry。 |
| block crate ktest | 25 passed，0 failed。 | BIO、partition、request queue、lease。 |
| DM crate ktest | 87 passed，0 failed。 | table、target、manager、BIO 生命周期。 |
| core DM ioctl ktest | 81 passed，0 failed，134 filtered。 | control ioctl、WAIT、rename、table lifecycle、readonly。 |
| focused C ABI | 182 passed，0 failed。 | raw ioctl、node/alias、mount lease、range、WAIT/SA_RESTART、readonly、DM_VERSION。 |
| NixOS canonical suites | control-plane、dataplane、LVM2 topology、linear、striped、mixed 全部通过。 | dmsetup、LVM2、ext2、文件 I/O、扩缩容和有序关机后的跨启动恢复；不覆盖断电一致性。 |

构建与运行期间仍出现 upstream/现有 warning：devtmpfs 的冗余限定、未使用 identity helper、procfs visibility 与宿主 CPU feature。它们未阻断上述专项矩阵，本轮未顺带重构。

## 5. 未覆盖边界

- 全仓 CI、完整非 DM initramfs regression；
- AArch64、非 QEMU TSC 和真实宿主硬件验证；
- uevent/sysfs/udev/systemd 自动发现与自动激活；
- DM-on-DM stacking、deferred remove、完整 control ABI 扩展；
- MlsDisk 生命周期与 VirtIO/NVMe fault injection。

只有这些边界造成用户可见失败、数据风险或阻塞新的 target/PR 时才重新开启。

## 6. 后续同步规则

1. 同步前记录权威 `dm` HEAD、目标 `upstream/main` 与 merge-base；先隔离未提交研发内容。
2. 可以用 worktree 预演冲突，但正式实现、验证和同步 commit 必须回到权威 `dm`。
3. 固定顺序：配置/构建入口 → 通用基础设施 → DM 语义 → 分层测试与验收。
4. 每个文本冲突、API 差异或自动合并语义问题，先记录两侧行为、取舍、影响与验证入口，再向用户汇报；不得静默选择 `ours/theirs`。
5. 测试串行执行、逐项即时汇报；完整验证矩阵结束后，再一次性更新同步记录、daily、交接和 PR 状态。

## 7. 当前停点

同步已完成并提交。当前 split child 完成责任修复已通过 block 25/0 与 DM crate 87/0，并随本提交收口；后续 PR 只能从包含该修复的权威 `dm` 提取最小补丁，不能直接提交整条同步历史。
