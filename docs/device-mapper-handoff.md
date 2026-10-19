# Device Mapper 对话交接

> **最后更新**：2026-10-19。
> 本文件是交接入口；当前状态见 [项目进度](../log/device-mapper-progress.md)，实际命令、失败及复盘见 [今日日志](../log/daily/2026-10-19.md)。PR 历史档案见 [log/PR](../log/PR/)，其中旧 SHA/测试入口不能覆盖当前源码事实。

## 1. 当前停点

- 今天完成 PR1–PR3 的审阅、复核、修改和单提交整理；随后在 dm 小范围回补已确认收益，没有开展完整 DM 重构。
- dm 本轮基于 `93159d132`：BIO 不变量局部简化、边界与队列顺序测试；DM 公开提交契约测试；控制 fd poll 虚假就绪修复；C ABI 测试按职责拆分；验证规则和文档更新。用户已授权分点本地提交，不 push，提交结果见项目进度及今日日志。
- 控制 fd 当前不再恒报 IN/OUT，但全局事件计数、等待队列、每打开文件事件基准及 `DM_DEV_ARM_POLL` **仍未实现**。已有单设备 `DM_DEV_WAIT` 不等于完整全局 poll。
- 主要 Linux 语义依据为 `/root/Linux-tree-code/kernel` 的 openEuler 基线 `5078a3a23a1e1825ec136485173ec98668cdd640`；每次新核对仍需确认 HEAD/本地修改，不能把学习改动当作上游事实。DM ABI 报告 4.50.0 不代表完整 Linux 兼容。
- 根盘 ESTALE/EUCLEAN 问题继续待定，不重启排查。用户反馈与上游一致的 main 也出现 users activation EUCLEAN；该反馈不证明根因，但不能归咎于 DM，不能写成 `make nixos` 编译失败。

## 2. PR 状态与工作方式

| 分支 | 当前本地提交 | 状态 |
|---|---|---|
| `mapped-bio-ranges` | `2f4fdc9a7` | PR1 #3937；今日曾推送中间提交 `e7e221993`，随后重放的当前历史未再次推送。 |
| `dm-linear-io-path` | `fd97f023f` | 基于新版 PR1，修改已 amend；当前历史未推送。本轮没有最终 PR2 ktest 运行计数。 |
| `dm-control-interface` | `f662bdc2d` | 基于新版 PR2，修复已并入原提交；未推送。完整 check 通过，用户反馈单项 ABI 成功；没有本轮最终 PR3 kernel/core ktest 独立证据。 |

- 后续全部以单 PR 为边界，用户尽可能手写，Claude 基于源码事实分析审查；审查不等于实现授权，不自主扩大 DM 优化。
- 每项判断区分实际缺陷、覆盖缺口、维护性建议和可选设计；不因 Linux/Rust 内部表示不同就要求重构。
- PR 修正回补 dm 前核对完整调用链和能力边界；例如 dm 当前拒绝 DmDevice backing，不能直接移植 PR2 双层映射测试并顺带开放堆叠。
- PR 在自身最终源码树验证；dm 通过不能替代 PR 验证。当前 PR 保持各自单提交，新增修改如何并入历史按用户指示处理，不自动重推。

## 3. 本轮验证与证据

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

全部在 `fork_Asterinas` 容器内串行执行。

```bash
cd /root/asterinas/kernel/core/comps/block
cargo osdk test aster_block::bio::tests
cargo osdk test aster_block::request_queue::tests

cd /root/asterinas/kernel/core/comps/device-mapper
cargo osdk test aster_device_mapper::device::tests
cargo osdk test aster_device_mapper::table::tests

cd /root/asterinas/kernel/core
cargo osdk test --kcmd-args=earlycon aster_core::device::misc::dm_control::tests::ioctl

cd /root/asterinas
make run_kernel AUTO_TEST=regression REGRESSION_TESTS="device/device_mapper_control device/device_mapper_lifecycle device/device_mapper_table"
```

- 当前 OSDK `test_matches_whitelist` 已支持模块路径；旧“模块 selector 必匹配空集”说明不适用于当前树。仍必须核对非零实际匹配数，每次立即保存根目录 `qemu-serial.log`，下一次运行会覆盖。
- ABI 三个程序分别为 control 13、lifecycle 7、table 5 个场景。原 24 个场景保留，新增 poll 后共 25 个；场景数不是断言数量。旧 `device_mapper`/`device_mapper_version` 即使残留二进制也不是当前验收入口。
- 最终验收为 `make check`、`make kernel`、相关定向 ktest；涉及 ABI 加 C 回归，工具流程按影响选择系统套件，不默认完整 ktest。
- 区分编译失败、guest 启动失败、测试失败。已观察到且未阻止测试完成的 ermsb/GRUB/CPU/video/IOMMU 环境提示留作备注，不仅凭 warning/error 字样中断；真实失败、阻塞或影响不明的新问题仍如实报告，不隐瞒。

## 5. 后续边界

- 不自动扩大到全局 poll、DM-on-DM、deferred remove、queue stacking、udev/sysfs 自动化或新 target；分别核对缺失依赖后由用户决定。
- Book/SCML 兼容性说明仍是后续文档项，本轮没有更新。旧 PR 档案和其他手册可能滞后，不能把历史验证当作当前通过。
- 普通 VFS rename、完整 mount source 路径和 procfs 的直接覆盖缺口保留，不以相邻模块通过替代。
- 不修改 KVM、RELEASE、QEMU、NixOS 启动协议或 `myshell/br.sh`，除非用户明确要求。系统 suite 启动已有镜像，不自动重建内核，须确认来源与 profile。
- 上游同步继续遵守 AGENTS.md：main 纯快进、按影响做基线、dm 安全引用与线性 rebase，不创建 merge；已推送未合并 PR 不随 dm 自动重放。
- 未知改动和构建产物不清理。历史根盘/排障保留物（包括 `target/nixos/asterinas.img`、`target/nixos/failed-root-MdBL2F/`、`target/nixos/pre-ext2-fix-OEsCVh/`、`/tmp/dm-nixos-recovery-M7gkiV/`、`/tmp/lvmsrc-dl/`）不因本次交接自动删除；是否仍存在须实查。

## 6. 接手顺序与记录规则

[CLAUDE.md](../CLAUDE.md) / [AGENTS.md](../AGENTS.md)
→ 本交接
→ [项目状态](../log/device-mapper-progress.md)
→ [今日日志](../log/daily/2026-10-19.md)
→ 按任务阅读源码与历史 PR 档案。

接手先核对分支、HEAD 和工作区，不自动切分支、提交或 push。阶段中只汇报进度，验证完成后经授权集中更新文档。每日新增“协作复盘与心得”，记录得到验证的判断、需纠正的认识与下次改进，不重复清单、不虚构经验。
