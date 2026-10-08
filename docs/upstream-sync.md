# Device Mapper 上游同步记录

> **状态**：2026-10-08 已将权威 `dm` 线性同步至本地 `main` `10a241870`；尚未 push。
>
> **范围**：本文记录本地 `dm` 的上游同步时间线、取舍、验证和后续规则；不属于 upstream PR 文档。

## 1. 当前结论

`dm` 已以 126 个提交线性重放到 `main` `10a241870`，`main..dm` 不含 merge commit。重放后以安全引用 `backup/dm-merge-2100c9052` 校准最终树，恢复此前已确认的 DM、block、devtmpfs、QEMU 与构建取舍；校准提交与该引用的源码树一致。

本轮只完成格式和 kernel-aware 编译层验证，不等同于全仓 CI、ktest、QEMU 或 NixOS 系统回归。由于 rebase 重写了本地历史，当前 `dm` 与 `origin/dm` 分叉；恢复凭证后只有在明确确认的前提下，才能用 `git push --force-with-lease` 更新远端。

## 2. 2026-10-08 同步时间线

| 时间 | 动作 | 结果与证据 |
|---|---|---|
| 2026-10-08 | 获取官方主线 | `origin/main` 从 `98e717275` 更新至 `10a241870`，本地 `main` 随后快进。 |
| 2026-10-08 | 首次整合与安全引用 | 首次未提交 merge 用于识别 8 个文本冲突及后续 API 语义差异；结果未 push，并保存在 `backup/dm-merge-2100c9052` 作为最终树审计基线。 |
| 2026-10-08 | 线性重放 | 从合并前 `dm` 逐条 rebase 126 个 DM 提交到 `main`；目录迁移导致的 Device Mapper 文件位置冲突映射到对应当前路径。 |
| 2026-10-08 | 最终树校准 | 初步 rebase 结果与安全引用有差异，故以该引用的已确认最终树校准为 `9b9cf5025`；校准后两树一致。 |
| 2026-10-08 | 规则与记录 | 在 `CLAUDE.md` 写入 rebase-only 规则，并记录本轮实际验证和未覆盖项。 |

## 3. 2026-10-08 关键取舍

| 主题 | 冲突或风险 | 选择 | 结果与边界 |
|---|---|---|---|
| 同步历史 | 普通 merge 保留两个父提交，后续 PR 基线和补丁提取难以线性审计。 | 以 `git rebase main` 逐条重放 `dm`；不得创建 merge commit。 | `main..dm` 无 merge commit；重放前必须建立安全引用，重放后必须与已确认最终树比较。 |
| 最终语义 | 自动重放的路径迁移和 `-X theirs` 不能完整再现此前 merge 的手工 API/生命周期取舍。 | 以安全引用的最终树为权威，形成单独线性校准提交。 | 保留 DM、block、devtmpfs、QEMU 和构建语义；不能将“rebase 无文本冲突”误认为语义等价。 |
| 新 device model | 上游 `KernelHooks::create_devnode` 只返回 unit，而 DM 的 `devtmpfs::create_node` 返回 inode-identity `DevtmpfsHandle`。 | 上游通用 device model 创建节点后暂丢弃 handle；DM runtime 的 primary/alias 节点继续持有和使用 handle。 | 通用 device model 仍是 request-based 删除；完整通用热插拔 identity 生命周期留给 P10 独立设计，DM 已有 remove/recovery 语义不回退。 |
| workspace 与 NixOS 工具 | 上游新增 `aster-device`、`gitMinimal`，DM 依赖 `aster-device-mapper` 及 LVM/磁盘验收工具。 | workspace dependency 和 NixOS 包列表均取并集。 | 新 device model 与 DM 同时编译；DM/LVM 验收环境不丢失。 |
| KVM、TSC 与 QEMU | 上游引入 KVM pvclock 与 AArch64 KVM CPU 选择；DM 有 QEMU timing、fallback 与 `ENABLE_KVM`/TDX 契约。 | TSC 优先 KVM pvclock，再使用 QEMU/CPUID/PIT 路径；保留 DM 开关并采用上游 AArch64 host CPU 选择。 | 保留 KVM 频率精度与 DM 的异常/QEMU 回退；未完成非 QEMU 或 AArch64 实机验证。 |
| lockfile | 合并后的 workspace 同时引入 upstream device model 和 DM 组件。 | 基于合并后的 manifest 在线重生成根 `Cargo.lock`。 | 锁文件反映二者共存的实际依赖图。 |

## 4. 2026-09-24 同步的关键取舍（历史）

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

## 5. 2026-10-08 验证矩阵

| 层次 | 实际结果 | 覆盖与边界 |
|---|---|---|
| 最终树校准 | 通过。 | `9b9cf5025` 的源码树与 `backup/dm-merge-2100c9052` 一致。 |
| 历史形状 | 通过。 | `main` 是 `dm` 祖先，`git log --merges main..dm` 无输出。 |
| 格式与局部空白 | 通过。 | `cargo fmt --check --all` 通过；校准 diff 的 `git diff --check` 通过。 |
| kernel-aware 编译 | 通过，保留 8 条既有 warning。 | `VDSO_LIBRARY_DIR=/root/linux_vdso`、隔离 target 下的 `cargo osdk check --manifest-path kernel/core/Cargo.toml` 通过；覆盖合并后的 `aster-device`、`aster-device-mapper` 和 `aster-core` 编译图。 |
| 全分支空白检查 | 未通过，均为既有历史内容。 | `git diff --check main..dm` 命中历史 patch 归档及旧日志的尾随空白；本轮未修改这些文件。 |
| `make check` | 未完成。 | 环境缺少 `jq`，随后命中既有 DOCX 文件与 `log/daily/2026-8-31.md` 的尾随空白；未进入完整 clippy/格式矩阵。 |
| QEMU/ktest/系统回归 | 未运行。 | 未重新验证 block、DM、core ioctl、C ABI 与 NixOS suites；不得沿用为本轮已验证事实。 |

## 6. 2026-09-24 验证矩阵（历史）

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

## 7. 2026-09-24 未覆盖边界（历史）

- 全仓 CI、完整非 DM initramfs regression；
- AArch64、非 QEMU TSC 和真实宿主硬件验证；
- uevent/sysfs/udev/systemd 自动发现与自动激活；
- DM-on-DM stacking、deferred remove、完整 control ABI 扩展；
- MlsDisk 生命周期与 VirtIO/NVMe fault injection。

只有这些边界造成用户可见失败、数据风险或阻塞新的 target/PR 时才重新开启。

## 8. 后续同步规则

1. 同步前记录权威 `dm` HEAD、目标 `main`、merge-base 与工作区状态；先隔离未提交研发内容，并在任何历史重写前创建可恢复的安全引用。
2. 可以用 worktree 预演冲突，但正式同步必须回到权威 `dm`，使用 `git rebase main` 线性重放；不得创建 merge commit。
3. 固定顺序：配置/构建入口 → 通用基础设施 → DM 语义 → 分层测试与验收。
4. 每个文本冲突、API 差异或自动合并语义问题，记录两侧行为、取舍、影响和验证入口；若依据既有取舍自动处理，rebase 后必须与安全引用的最终树比较，不能仅以“无冲突”作为正确性结论。
5. 测试必须串行执行、逐项即时汇报；完成后确认 `git log --merges main..dm` 无输出，并更新本文档、daily、交接和 PR 状态。

## 9. 当前停点

本地 `dm` 已线性同步至 `main` `10a241870`，并已提交最终树校准与同步规则；`backup/dm-merge-2100c9052` 保留为本轮审计和恢复引用。当前仅完成格式和 kernel-aware 编译验证，未重新运行 QEMU、ktest、C ABI 或 NixOS 回归。

`dm` 因 rebase 与 `origin/dm` 分叉，且当前凭证问题尚未解决；在用户恢复凭证并明确确认前不得 push。后续 PR 仍只能从权威 `dm` 提取最小补丁，不能直接交付整条同步历史。
