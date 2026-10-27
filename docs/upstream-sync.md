# Device Mapper 上游同步记录

> **规则更新（2026-10-26）**：`dm` 允许 merge 最新 `main`；PR 分支更新基线必须 rebase 到最新 `main` 并保持线性。详见第 8 节。
>
> **历史快照**：下文 2026-10-08—09 的同步状态与验证仅代表当时，不代表当前分支或本次同步结果。
>
> **范围**：本文记录本地 `dm` 的上游同步时间线、取舍、验证和后续规则；不属于 upstream PR 文档。

## 2026-10-27 后续状态

完整 dm 的后续修复和记录已提交，当前 HEAD `3cd8af8b7`，本轮未推送 dm。
PR2 `074e7e80d`、PR3 `be3f79c88`、PR4 `6995e5ab3` 已单独授权原子推送并核验；PR4 已验证、待拆分。
九个本地备份分支（含 `dm-backup-20261026-before-sync`）已按用户要求删除；stash 仍保留，磁盘备份未清理。
以下“未提交”“备份保留”等是当时快照，不代表当前状态；详情见 [今日日志](../log/daily/2026-10-27.md)。

## 2026-10-26 同步结果（历史）

- main 从 `77ee3b196` 快进到 `ceb65e3f8`，23 个提交、50 个文件；包含内核、OSDK 与构建入口变化。
- main 的 make format 退出 0 且未改源码；助手后台 make check 在 cargo-osdk 安装访问 crates.io 时遇到 CONNECT 502/超时，并占用 package-cache 锁。最终进程被终止，make 退出 2；锁释放已核对。这不是编译失败。
- 用户随后反馈 Cargo 镜像配置有效、check 无问题，并明确跳过 main 的 make kernel。助手未修改 Cargo 镜像配置，未记录用户重试的原始输出。
- merge 无文本冲突，合并提交 `37ddb9245`，main 祖先检查退出 0。恢复 stash 时 test/nixos/Makefile 冲突，采用上游排版并保留 runner 分支和失败传播；解决后 diff --check、无未合并项及 Makefile dry-run 检查通过。
- 用户反馈合并后 make check 与控制面集成测试通过；未独立归档输出/计数/镜像来源，未运行本次合并后的 kernel/ktest/C 全量验证。低冲突不等于无语义风险。
- 原研发改动和三份规则文件恢复为未暂存；stash `1b60fbd99901a77bbcd91f4773aedcb970433a6b` 与安全引用 `dm-backup-20261026-before-sync` 保留。冲突修复和其他研发内容未提交；未 push，也未更新 PR 分支。

## 1. 2026-10-08 同步结论（历史）

`dm` 已以 126 个提交线性重放到 `main` `10a241870`，`main..dm` 不含 merge commit。重放后以安全引用 `backup/dm-merge-2100c9052` 校准最终树，恢复此前已确认的 DM、block、devtmpfs、QEMU 与构建取舍；校准提交与该引用的源码树一致。

早期验证仅完成格式和 kernel-aware 编译等局部检查，不能当作全仓验证通过；下文早期矩阵保留当时失败事实，后续修复及定向复测以 [当日日志](../log/daily/2026-10-8.md) 为准。
2026-10-08 经用户确认，后续同步改为静态检查、普通构建与按影响范围选择的定向 ktest，不默认执行完整 ktest，详见第 8 节。
由于 rebase 重写了本地历史，当前 `dm` 与 `origin/dm` 分叉；只有在凭证可用且明确确认的前提下，才能用 `git push --force-with-lease` 更新远端。

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

## 5. 2026-10-08 早期验证矩阵

本表记录当时的执行状态，不代表后续修复后的最终状态；后续结果见 [当日日志](../log/daily/2026-10-8.md)。

| 层次 | 实际结果 | 覆盖与边界 |
|---|---|---|
| 最终树校准 | 通过。 | `9b9cf5025` 的源码树与 `backup/dm-merge-2100c9052` 一致。 |
| 历史形状 | 通过。 | `main` 是 `dm` 祖先，`git log --merges main..dm` 无输出。 |
| 格式与局部空白 | 通过。 | `cargo fmt --check --all` 通过；校准 diff 的 `git diff --check` 通过。 |
| kernel-aware 编译 | 通过，保留 8 条既有 warning。 | `VDSO_LIBRARY_DIR=/root/linux_vdso`、隔离 target 下的 `cargo osdk check --manifest-path kernel/core/Cargo.toml` 通过；覆盖合并后的 `aster-device`、`aster-device-mapper` 和 `aster-core` 编译图。 |
| 全分支空白检查 | 未通过，均为既有历史内容。 | `git diff --check main..dm` 命中历史 patch 归档及旧日志的尾随空白；本轮未修改这些文件。 |
| `make check` | 未通过。 | 在 clippy 前被既有 DOCX 文件与 `log/daily/2026-8-31.md` 的尾随空白阻断；宿主环境另缺少 `jq`，均未修改。 |
| `make initramfs` | 通过。 | 容器中重建 initramfs 与镜像；这是 `make ktest` 的自动前置步骤，本轮仅作为单独排障/输入验证执行。 |
| `make kernel` | 通过，保留既有 warning。 | 当时记录为容器中完成普通非测试内核与启动 ISO 构建；不能据此称为 release 验收，根 Makefile 当前默认 `RELEASE=0`。 |
| block / DM 补充 ktest | 通过。 | block crate 28 passed、DM crate 87 passed；覆盖同步后的 BIO/lease 与 DM table/target/lifecycle 回归，但不是全量 ktest 的替代。 |
| `make ktest` | 未通过。 | OSTD 的 `ostd::arch::vm::vmx::vmcs::test::switch_and_reuse_vmcs` 发生 atomic mode 检查 panic（210 passed，1 failed）；后续 Asterinas 构建又因代理 502 无法获取 `smoltcp`。完整 gate 未完成。 |
| C ABI / NixOS 系统回归 | 未运行。 | 当前完整同步验证已被 `make check` 和 `make ktest` 阻断，不得宣布本轮同步验证完成。 |

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

## 8. 后续同步规则（2026-10-26 更新）

### 分支职责与同步顺序

1. `dm` 是研发集成分支，允许 merge 最新 `main`，不再要求 `main..dm` 无 merge commit。PR 分支是最小补丁交付分支，必须保持线性；新 PR 从最新 `main` 创建，已有 PR 需要更新基线时 rebase 到最新 `main`，不引入 `dm` 的 merge 历史。
2. 同步前记录 `dm` HEAD、本地 `main` SHA、merge-base 与工作区状态；保全并隔离未提交内容（含未跟踪文件），创建可恢复的安全引用并保持 rerere 开启。不得清理未知改动或构建产物。
3. 先 fetch 上游，将本地 `main` 纯快进到 `upstream/main`，再侦察增量与影响面。仅文档或测试的增量可跳过 main 基线；触及内核核心、构建系统、依赖清单或 CI 镜像时必须执行基线。
4. 基线在检出 `main` 的主工作区容器内执行，不使用嵌套 worktree。基线失败先报告为上游/环境问题并讨论，不进入 dm 整合后再归因。
5. 基线通过或确认可跳过后，回到权威 `dm`，将最新 `main` 合并进来。固定审查顺序：配置/构建入口 → 通用基础设施 → DM 语义 → 分层测试与验收。
6. 每个文本冲突、API 差异或自动合并语义问题，说明两侧行为、取舍、影响和验证入口。保留本地工作约定事实；锁文件与依赖版本取上游侧，再在容器内在线归一实际依赖图。旧 DM 修复只有确认已被上游覆盖才可舍弃；不确定时先报告。与安全引用比较 DM 改动是否保全，而非要求整棵树相同。
7. 已推送未合并的 PR 不随 `dm` 自动更新；待维护者反馈或确需新基线时单独授权处理，先建安全引用，再 rebase 到最新 `main`，确认 `git log --merges main..<PR分支>` 无输出并在该分支验证。
8. 同步完成后确认最新 `main` 是 `dm` 的祖先（`git merge-base --is-ancestor main dm` 返回 0），核对未提交内容恢复情况。验证通过后再处理安全引用。验证不自动授权提交或 push；日志、交接与 PR 档案须在结果汇总并获确认后更新。

### 验证矩阵

main 基线用于区分上游/环境问题；**最终集成验证必须在权威 dm 上执行，PR 验证必须在对应 PR 分支执行**，不能互相替代。

| 阶段 | 执行位置 | 顺序与范围 |
|---|---|---|
| 按影响执行的同步前基线 | 主工作区容器内检出的最新 main。 | `make format` → 核对格式化是否产生改动 → `make check` → `make kernel`；不默认完整 ktest。格式化若改变上游源码，先报告，不将修改后的树当作原始 main 基线。 |
| 整合后最终验证 | `fork_Asterinas:/root/asterinas` 的权威 dm。 | `make format` → 核对范围外改动 → `make check` → `make kernel` → 新增、冲突解决及接口/语义影响链的定向 ktest。 |
| 用户 ABI 回归 | 当前待验收分支的 `test/initramfs`。 | 参考官方 C regression 的组织、构建及运行入口，覆盖 ioctl/syscall、返回值、错误路径和用户可见状态。 |
| 用户工具与系统工作流 | 当前待验收分支的 `test/nixos`。 | 参考官方 suite、框架注册、配置及 `make run_nixos NIXOS_TEST_SUITE=<suite>` 入口，按需验证 dmsetup、文件系统或其他实际工具工作流。 |
| PR 最终验收 | 基于最新 main 的对应 PR 分支。 | 按最小补丁及依赖链选择上述层次；创建 PR 前拉齐上游容器镜像并重跑定向验证。 |

后续 PR 的测试设计必须先参考 `test/initramfs`、`test/nixos` 现有实现及当前 Makefile，不以私有 runner 作为默认上游交付入口。每个 PR 只携带其所需测试与依赖；研发分支整套测试通过不代表最小 PR 自身可构建或可运行。官方目录中依赖 DM 分支扩展的测试也必须逐项核对，不因路径正确就认定可独立上游运行。

ktest 覆盖内核内部行为，C 回归覆盖用户 ABI，NixOS 测试覆盖真实用户工具调用链，三者不能互相替代。定向 ktest 从目标 crate 目录运行，先核对当前 OSDK 的 selector 行为，确保实际匹配；覆盖相关文件内全部测试及受影响调用链。上游 block 接口变化，即使 DM 无文本冲突，也要验证 block 与 DM 的关联路径。

### 验证范围与完成判定

- 执行前列出“改动 → 测试入口”；不默认运行全部 DM 模块、C 回归或系统测试。调度、内存、锁、OSDK 等变化导致影响难以限定时，先讨论扩大范围，不自动启动完整 `make ktest`。
- 测试串行执行，记录分支、源码 SHA 与相关未提交差异、配置、实际命令、匹配数量、退出码及日志。零匹配或内部跳过不算通过；无直接测试的改动明确记录覆盖缺口。
- `make kernel` 默认 dev 构建，不表述为 release 验收；完整 ktest 已依赖 initramfs，不重复增加独立 initramfs gate。NixOS 运行已有镜像不代表新内核或新配置已安装，必须核对镜像来源；配置变化需走对应构建流程。
- 失败、环境阻塞或未预期 warning 出现时先停止扩大验证范围并讨论；不得把历史通过写成当前源码验证通过。
- 选定检查全部成功、实际测试零失败且命令 exit 0，才可宣布相应范围验证通过。后续源码修改需补跑受影响项目；定向通过不是全仓验证通过。

## 9. 2026-10-09 停点（历史）

本地 `dm` 已线性同步至 `main` `10a241870`；`backup/dm-merge-2100c9052` 保留为本轮审计和恢复引用。
10 月 8 日后续完整 ktest 已 exit 0，之后 lint 修复后的完整静态检查及普通 dev 内核/启动 ISO 构建也已 exit 0，原始证据见 [当日日志](../log/daily/2026-10-8.md)。
完整 ktest 早于后续源码清理，不能称为当前同一快照的全量验证。
10 月 8—9 日针对未提交修复串行完成 DM component 87/0、core DM ioctl 81/0、devtmpfs 6/0、runtime registry 8/0、VMCS 1/0，合计 183 项通过，各命令 exit 0，相关源码差异指纹保持一致。
VMCS 首次零匹配不计入，修正路径后实际执行通过；结果及覆盖缺口见 [10 月 9 日日志](../log/daily/2026-10-9.md)。
本轮没有 C guest 或 NixOS 系统复测，不宣称全仓或完整系统验收通过；源码修复、C 格式及检查配置已按五点提交至 `c02b6ec26`，后续定向规则与文档更新归入第六个文档提交。

`dm` 因 rebase 与 `origin/dm` 分叉，且当前凭证问题尚未解决；在用户恢复凭证并明确确认前不得 push。后续 PR 仍只能从权威 `dm` 提取最小补丁，不能直接交付整条同步历史。
