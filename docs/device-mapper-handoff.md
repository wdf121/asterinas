# Device Mapper 对话交接

> **更新日期：2026-10-27。** 当前仍在 `dm`，HEAD `7c46b2ee8` 加未提交生产/测试修复。用户在最新两处 lint 修正后确认验证全部通过；助手未独立取得最终日志、计数及镜像来源。当前阶段已获授权集中更新文档，不继续修改生产代码。
>
> **接手重点**：PR3 制备前已修 CREATE errno、dev 低 32 位、C ABI 4.48.0、1 GiB 校验上限、UUID bytes、全局 poll/ARM_POLL、REMOVE 控制引用与 BIO 排空、minor 一次性释放及旧实例保护。逐项状态与全部卡点见 [global.md §10.4](global.md#104-pr3-制备前修复与待办总账2026-10-27)，实际过程见 [10 月 27 日日志](../log/daily/2026-10-27.md)。
>
> **未完成/禁止默认为完成**：deferred REMOVE 与 DM uevent 因缺跨子系统接口暂缓，非 UTF-8 名称仍不支持；精确事件提交点、极限分配证据、PR3 前置及裁剪待处理。`dm-device-management` 仍与 PR2 同为 `808d36daf`，尚未提取、验证、提交或推送。PR3 只含无表管理，不另造模型、不自动修改 PR1/PR2。
>
> 以下 10 月 26 日同步摘要及第 1–3 节保留历史事实，不代表本轮已提交或当前 poll/ABI 状态。

> **2026-10-26 同步摘要。** dm 已 merge 最新 main `ceb65e3f8`，合并提交 `37ddb9245`；后续五点提交收口至 `7c46b2ee8` 并普通推送，远端 SHA 已核验。stash 恢复的 Makefile 冲突修复已纳入测试套件提交；原研发内容与已确认删除项已提交。当前在 dm 进行经授权的文档更新，不修改 PR 生产实现。
>
> 用户反馈合并后 `make check` 和控制面集成测试通过，原始输出、计数和镜像来源未独立归档；本次同步明确跳过 `make kernel`。无文本冲突不等于无语义回归。
>
> 控制面恢复为薄 `main.rs` 注册/上传 + `cases.sh` 断言 + `run.sh`/`runner.py` 监督，保留 13 例、单份 summary.log 与 CST 时间；新增 extra_config.nix 仅格式/语法检查，未重建镜像。用户在同步前反馈重跑 45 秒；不混为同步后性能证据。
>
> 下一阶段从新 PR2 开始，PR1 不改实现；PR2～PR11 完成框架，随后最小 linear 终验。路线与前置缺口见 [PR 规划](pull_request.md)，今日过程见 [日报](../log/daily/2026-10-26.md)。PR 链经单独授权更新为 `ceb65e3f8 → 2dd1bcac5 → 808d36daf`；PR1 已推送，PR2 未推送。PR2 六文件，不含公共 REGRESSION_TESTS 扩展或代理前缀；原 commit 描述保留，英文页面草稿见 [PR2 档案](../log/PR/PR-2-control-interface.md)。
>
> 安全引用 `dm-backup-20261026-before-sync` 和 stash `1b60fbd99901a77bbcd91f4773aedcb970433a6b` 保留；另有 PR1/PR2 的 `*-backup-20261026` 分支，均未删除；不要擅自清理备份或其他 stash。

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
→ [最新日志（2026-10-27）](../log/daily/2026-10-27.md)
→ 按任务阅读源码与历史 PR 档案。

接手先核对分支、HEAD 和工作区，不自动切分支、提交或 push。阶段中只汇报进度，验证完成后经授权集中更新文档。每日新增“协作复盘与心得”，记录得到验证的判断、需纠正的认识与下次改进，不重复清单、不虚构经验。
