# Device Mapper 对话交接

## 当前交接（2026-10-28）

**当前停点：无表管理拆分已完成，六份档案已整理；未启动后续框架功能。**
当前 `dm` HEAD 为 `7767dd9bf`，字符注册枚举已回补；新 NixOS 管理套件及整个制备链未自动回补。

| 范围 | 分支 / 提交 | 状态 |
|---|---|---|
| PR1 | `mapped-bio-ranges` / `2dd1bcac5` | 保持不动 |
| PR2 | `dm-control-interface` / `d5347826f` | 本地及 origin 已更新 |
| PR3 | `proc-devices` / `aa11c7ab6` | 本地及 origin 已更新 |
| PR4 | `dm-device-registry` / `0c68ed7da` | 本地及 origin 已更新 |
| PR5–PR9 | `5bd01f020` → `03b2d721a` → `c13e94ffc` → `1e0415869` → `24e7ee2e8` | 本地 `pr-dm` 制备链，未独立建分支或推送 |

- 三个正式分支以精确 lease 原子推送并核验远端；旧 `dm-device-management` 本地和远端分支已删除，历史版本按 `6995e5ab3` 引用。未创建 GitHub PR。
- `pr-dm` 是经用户批准的本地命名例外，不是上游正式分支。当前不自动建分支、改写历史或推送。
- 用户反馈 C 定向检查 control 64、management 180 通过；数字为检查次数，不是 FN_TEST 数。
- 最后套件的一次 `make check` 退出 137，根因未确定；用户随后确认 check 通过，并在 guest 手动跑完三个 dmsetup 场景。不是助手自动 runner 通过记录。
- 套件脚本随镜像安装至 `/etc/dm-management-cases.sh`，构建使用 `make nixos NIXOS_TEST_SUITE=device-mapper-management`；当前 dm 的旧套件仍为 `device-mapper-control-plane`，不得跨分支混用。
- 默认 udev 同步下无表场景通过不等于完整 uevent/sysfs/udev 支持。源码推导和验证边界见 [PR9](../log/PR/PR-9-management-nixos.md)。
- 档案编号为 PR4–PR9，英文维护者描述不含内部编号，保留全英文小标题。原 PR1–PR3 描述不重写，旧 SHA 为历史。
- 后续候选路线顺延至 PR10–PR18，详见 [规划与全部档案索引](pull_request.md)。本次仅文档更新，不提交、不推送、不实施后续功能。

最新过程见 [10 月 28 日日志](../log/daily/2026-10-28.md)。以下保留旧交接事实，旧分支状态及待拆分结论不代表现在。

## 历史交接（2026-10-27 收尾）

**当前卡点：PR4 已验证、已提交并推送，待拆分。** 不再停留在“尚未提取”或“等待 NixOS 验证”。
当前检出 `dm`，HEAD `3cd8af8b7`；本轮只有文档改动未提交，没有把 PR 分支代码自动回补到 dm。

| 阶段 | 分支 | HEAD | 状态 |
|---|---|---|---|
| PR1 | `mapped-bio-ranges` | `2dd1bcac5` | 保持不动 |
| PR2 | `dm-control-interface` | `074e7e80d` | 测试目录整理已归入原提交，已推送 |
| PR3 | `proc-devices` | `be3f79c88` | 字符/块注册枚举，已验证、已推送 |
| PR4 | `dm-device-management` | `6995e5ab3` | 无表管理已验证、已推送，因体量待拆分 |

- PR2/PR3/PR4 三分支原子推送后已核验远端 SHA；未创建 GitHub PR。
- PR3 用户确认 check、kernel、block/core ktest、guest `/test/fs/procfs/devices` 通过，并用 `cat /proc/devices` 检查输出。
- PR4 助手执行 make format、完整 make check 成功；用户先反馈除 NixOS 外通过，修正脚本交付方式后确认测试全部通过。实际运行计数和完整用户日志未归档，不虚构。
- PR4 共 26 文件、+3203/-24；没有删除文件。拆分应保留直接测试，不为减少行数去掉必要覆盖；具体拆分边界未定。
- PR4 NixOS 套件把 cases.sh 安装为 `/etc/dm-management-cases.sh`，不再经串口分块上传；构建/运行使用 `NIXOS_TEST_SUITE=device-mapper-management`，单例用 `NIXOS_TEST_CASE`。脚本变化须重建镜像。该套件尚未回补 dm。
- PR2 原七个测试函数体和生产代码不变；PR3 重放前后补丁一致；历史整理未改变已验证 PR4 工作文件。未独立重跑最终 PR2/PR3 系统测试。
- 九个本地备份分支已按用户要求删除；两个 stash 仍在，磁盘备份未清理，不据旧摘要声称备份分支仍存在。

### 下一步边界

1. 先审定 PR4 拆分方案，不自行改写已经推送的提交，也不继续 table/BIO 等后续功能。
2. [PR 规划](pull_request.md) 已按插入枚举 PR3 重编号：PR4 无表管理、PR5–PR12 后续框架、PR13 最小 linear；PR4 拆分后再调整。
3. 完整 dm 的语义修复已在 `7de7025ad`、`74ee6b89e` 提交；PR 分支的枚举、新文件组织与 NixOS 套件尚未自动回补。
4. deferred REMOVE、DM uevent/cookie、非 UTF-8 名称、极限分配和精确事件点仍为独立后续事项。无表 dmsetup 通过不代表完整 uevent/udev 支持，见 [总账](global.md#104-pr3-制备前修复与待办总账2026-10-27)。
5. 本轮仅集中更新文档；提交、push、拆分和回补各需对应授权。详情见 [今日日志](../log/daily/2026-10-27.md)、[PR2](../log/PR/PR-2-control-interface.md)、[PR3](../log/PR/PR-3-proc-devices.md)。

## 历史同步摘要（以下仅代表当时）

> **2026-10-26 同步摘要。** dm 已 merge 最新 main `ceb65e3f8`，合并提交 `37ddb9245`；后续五点提交收口至 `7c46b2ee8` 并普通推送，远端 SHA 已核验。stash 恢复的 Makefile 冲突修复已纳入测试套件提交；原研发内容与已确认删除项已提交。当前在 dm 进行经授权的文档更新，不修改 PR 生产实现。
>
> 用户反馈合并后 `make check` 和控制面集成测试通过，原始输出、计数和镜像来源未独立归档；本次同步明确跳过 `make kernel`。无文本冲突不等于无语义回归。
>
> 控制面恢复为薄 `main.rs` 注册/上传 + `cases.sh` 断言 + `run.sh`/`runner.py` 监督，保留 13 例、单份 summary.log 与 CST 时间；新增 extra_config.nix 仅格式/语法检查，未重建镜像。用户在同步前反馈重跑 45 秒；不混为同步后性能证据。
>
> 下一阶段从新 PR2 开始，PR1 不改实现；PR2～PR11 完成框架，随后最小 linear 终验。路线与前置缺口见 [PR 规划](pull_request.md)，今日过程见 [日报](../log/daily/2026-10-26.md)。PR 链经单独授权更新为 `ceb65e3f8 → 2dd1bcac5 → 808d36daf`；PR1 已推送，PR2 未推送。PR2 六文件，不含公共 REGRESSION_TESTS 扩展或代理前缀；原 commit 描述保留，英文页面草稿见 [PR2 档案](../log/PR/PR-2-control-interface.md)。
>
> 当时安全引用 `dm-backup-20261026-before-sync` 和 stash `1b60fbd99901a77bbcd91f4773aedcb970433a6b` 保留；另有 PR1/PR2 的 `*-backup-20261026` 分支，当时均未删除；备份分支已于 10 月 27 日按授权删除，stash 未清理。

> **PR2 验证边界**：用户反馈 format/check/kernel/core 测试成功；core 入口为 `cd kernel/core && cargo osdk test`，不记未提供的运行计数。官方全量回归在 cgroup CPU 记账阈值处失败，main 同命令复现；用户表示已确认上游 bug 并停止排查，未提供链接。最终清理后未重跑，不列定向 C 回归通过。对照日志保存在 `/tmp/pr2-main-cgroup-20261026-7l3yamjc/`。临时 DCAP 代理修改已撤销，main 未改写。

## 1. 历史停点（2026-10-19 至 2026-10-20）

以下第 1–3 节保留当时事实；旧 PR 编号、分支、SHA 和 98 项测试计数不代表当前状态，历史授权也不延续到新的操作。

- 今天完成 PR1–PR3 的审阅、复核、修改和单提交整理；随后在 dm 小范围回补已确认收益，没有开展完整 DM 重构。
- dm 本轮基于 `93159d132`：BIO 不变量局部简化、边界与队列顺序测试；DM 公开提交契约测试；控制 fd poll 虚假就绪修复；C ABI 测试按职责拆分；验证规则和文档更新。用户已授权分点本地提交，不 push，提交结果见项目进度及今日日志。
- 控制 fd 当前不再恒报 IN/OUT，但全局事件计数、等待队列、每打开文件事件基准及 `DM_DEV_ARM_POLL` **仍未实现**。已有单设备 `DM_DEV_WAIT` 不等于完整全局 poll。
- 主要 Linux 语义依据为 `/root/Linux-tree-code/kernel` 的 openEuler 基线 `5078a3a23a1e1825ec136485173ec98668cdd640`；每次新核对仍需确认 HEAD/本地修改，不能把学习改动当作上游事实。DM ABI 报告 4.50.0 不代表完整 Linux 兼容。
- 根盘 ESTALE/EUCLEAN 问题继续待定，不重启排查。用户反馈与上游一致的 main 也出现 users activation EUCLEAN；该反馈不证明根因，但不能归咎于 DM，不能写成 `make nixos` 编译失败。

## 2. 历史 PR 状态与工作方式

| 分支 | 当前本地提交 | 状态 |
|---|---|---|
| `mapped-bio-ranges` | `2f4fdc9a7` | PR1 #3937；今日曾推送中间提交 `e7e221993`，随后重放的当前历史未再次推送。 |
| `dm-linear-io-path` | `fd97f023f` | 基于新版 PR1，修改已 amend；当前历史未推送。本轮没有最终 PR2 ktest 运行计数。 |
| `dm-control-interface` | `5b08f9a0a` | 已 amend 公共入口顺序、命名和测试，未推送；用户反馈 C 回归已跑完，最终 check/kernel/ktest 输出和计数未归档。 |
| `dm-linear-configuration` | `5b08f9a0a` | 从新 PR3 重建，无独有提交，暂缓实现。 |

- 后续全部以单 PR 为边界，用户尽可能手写，Claude 基于源码事实分析审查；审查不等于实现授权，不自主扩大 DM 优化。
- 每项判断区分实际缺陷、覆盖缺口、维护性建议和可选设计；不因 Linux/Rust 内部表示不同就要求重构。
- PR 修正回补 dm 前核对完整调用链和能力边界；例如 dm 当前拒绝 DmDevice backing，不能直接移植 PR2 双层映射测试并顺带开放堆叠。
- PR 在自身最终源码树验证；dm 通过不能替代 PR 验证。当前 PR 保持各自单提交，新增修改如何并入历史按用户指示处理，不自动重推。

## 3. 历史验证与证据（2026-10-19）

测试源码为 dm `93159d132` 加本日代码/测试修改；提交组织和后续文档不改变已验证的生产/测试源码。

| 范围 | passed | failed | filtered | 命令退出码 |
|---|---:|---:|---:|---:|
| block BIO | 13 | 0 | 20 | 0 |
| block queue | 5 | 0 | 28 | 0 |
| DM device | 20 | 0 | 74 | 0 |
| DM table | 36 | 0 | 58 | 0 |
| core control ioctl | 24 | 0 | 242 | 0 |

- Claude 独立核验：完整 `make check`、以上 98 个定向 ktest、C 程序编译/格式和 shell 语法。不是全仓 ktest。
- 用户反馈：`make check`、`make kernel`、guest ABI 回归已运行通过；后两项本轮未独立核验日志，不虚构断言数量或退出码。
- 证据目录：`/tmp/dm-ktest-20261019.u8QPBZ/`，各组有独立 command/serial 日志；临时目录不保证永久保存。`tracked.patch` 不包含当时未跟踪的新 C 文件。
- 本轮导入遗漏（Vec、DmControlFile、IoEvents、Pollable）已修复后通过 check。文档更新后未重跑完整 check，不把早先结果说成覆盖最后文档。
- 2026-10-16 V5 core 265/265、C 回归及系统套件结果仅为历史证据，见 [10 月 16 日](../log/daily/2026-10-16.md)。今天没有重跑 dmsetup/LVM/NixOS 全套集成，也没有运行完整 `make ktest`。

## 4. 当前测试入口

全部在 `fork_Asterinas` 容器内串行执行。先在目标分支根目录运行 `make format`，核对 diff，再执行 `make check`、`make kernel`。定向测试进入相关 Cargo crate，默认覆盖该 crate 全部 ktest，不逐函数循环：

```bash
cd /root/asterinas/kernel/core/comps/block
cargo osdk test

cd /root/asterinas/kernel/core/comps/device-mapper
cargo osdk test

cd /root/asterinas/kernel/core
cargo osdk test --kcmd-args=earlycon

# 完整 dm 的 C 测试入口：
cd /root/asterinas
make run_kernel AUTO_TEST=regression REGRESSION_TESTS="device/device_mapper_control device/device_mapper_lifecycle device/device_mapper_table"

# 新 PR2 已移除 REGRESSION_TESTS 扩展，使用官方全量入口：
# make run_kernel AUTO_TEST=regression
```

- 当前完整 dm 的 OSDK 支持模块筛选，但新 PR2 基线不同；不跨分支照搬 selector。每次核对非零实际计数并及时保存串口日志，下一次运行可能覆盖。上述全 crate 命令避免模块筛选差异，并不是全仓 make ktest。
- 当前 C 测试源码为 control 15、lifecycle 7、table 5 个 `FN_TEST` 场景，共 27 个；新增事件/ARM 与 opaque UUID 场景。源码场景数不是实际执行数或断言数，最终运行计数未独立取得。旧 `device_mapper`/`device_mapper_version` 即使残留二进制也不是当前验收入口。
- 最终验收为 `make check`、`make kernel`、相关定向 ktest；涉及 ABI 加 C 回归，工具流程按影响选择系统套件，不默认完整 ktest。
- 区分编译失败、guest 启动失败、测试失败。已观察到且未阻止测试完成的 ermsb/GRUB/CPU/video/IOMMU 环境提示留作备注，不仅凭 warning/error 字样中断；真实失败、阻塞或影响不明的新问题仍如实报告，不隐瞒。

## 5. 后续边界

- 全局 control poll/ARM_POLL 已在完整 dm 实现；不自动扩大到 DM-on-DM、deferred remove、queue stacking、udev/sysfs 自动化或新 target。需要修改其他子系统或新增子系统的工作本轮不做，卡点逐项保存在 global.md。
- Book/SCML 兼容性说明仍是后续文档项，本轮没有更新。旧 PR 档案和其他手册可能滞后，不能把历史验证当作当前通过。
- 普通 VFS rename、完整 mount source 路径和 procfs 的直接覆盖缺口保留，不以相邻模块通过替代。
- 不修改 KVM、RELEASE、QEMU、NixOS 启动协议或 `myshell/br.sh`，除非用户明确要求。系统 suite 启动已有镜像，不自动重建内核，须确认来源与 profile。
- 上游同步继续遵守 AGENTS.md：main 纯快进、按影响做基线、dm 同步前建立安全引用，允许 merge；PR 链更新必须基于最新 main 线性 rebase，已推送未合并 PR 不随 dm 自动重放。
- 未知改动和构建产物不清理。历史根盘/排障保留物（包括 `target/nixos/asterinas.img`、`target/nixos/failed-root-MdBL2F/`、`target/nixos/pre-ext2-fix-OEsCVh/`、`/tmp/dm-nixos-recovery-M7gkiV/`、`/tmp/lvmsrc-dl/`）不因本次交接自动删除；是否仍存在须实查。

## 6. 接手顺序与记录规则

[CLAUDE.md](../CLAUDE.md) / [AGENTS.md](../AGENTS.md)
→ 本交接
→ [项目状态](../log/device-mapper-progress.md)
→ [最新日志（2026-10-28）](../log/daily/2026-10-28.md)
→ 按任务阅读源码与历史 PR 档案。

接手先核对分支、HEAD 和工作区，不自动切分支、提交或 push。阶段中只汇报进度，验证完成后经授权集中更新文档。每日新增“协作复盘与心得”，记录得到验证的判断、需纠正的认识与下次改进，不重复清单、不虚构经验。
