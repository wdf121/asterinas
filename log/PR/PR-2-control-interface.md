# PR2：控制设备与 DM_VERSION

## 1. 状态与 Git 标识

2026-10-27 更新：分支 `dm-control-interface`，HEAD `074e7e80d77fed971cdb111290cfff0e8d9d629f`，直接父提交为 PR1 `2dd1bcac5`，共同 main 基线为 `ceb65e3f8`。
相对 PR1 保持单提交、线性历史，已推送 `origin/dm-control-interface`，并以 `git ls-remote` 核验远端 SHA 一致。
本轮没有创建 GitHub PR 或发布页面描述；文档在 `dm` 分支维护，生产改动未自动回补。

提交标题保持 `Add the Device Mapper control device with DM_VERSION support`；其内容准确，不为调整 PR 页面标题再次改写提交。页面标题建议见第 7 节。

## 2. 目标与非目标

PR1 是通用块层前置，本 PR 首次引入 DM 专用控制代码。目标是逐步建立基于 BIO、兼容 Linux 用户态控制接口的 Device Mapper 框架；本 PR 先建立 `/dev/mapper/control`、权限检查、命令 type、版本握手、错误优先级及未知命令拒绝。报告 ABI 4.48.0，控制事件不支持时不报告虚假 poll 就绪。

不包含 manager、设备创建、表、数据面、生产 target 或完整事件机制。后续先完成通用框架，再由最小 linear 验证真实用户态闭环，见 [系列路线](../../docs/pull_request.md)。该方向不承诺完整 Linux DM 功能、固定 PR 数量或交付时间。

## 3. 当前范围与提取

相对 PR1 共七个文件、587 行新增，无删除行。文件路径以本 PR 提交树为准：

- `kernel/core/src/device/misc/dm_control.rs`
- `kernel/core/src/device/misc/dm_control/abi.rs`
- `kernel/core/src/device/misc/dm_control/tests/mod.rs`
- `kernel/core/src/device/misc/dm_control/tests/control_abi_tests.rs`
- `kernel/core/src/device/misc/mod.rs`
- `test/initramfs/src/regression/device/device_mapper_control.c`
- `test/initramfs/src/regression/device/run_test.sh`

生产实现来源为 dm 的控制入口，支持命令裁剪为 VERSION，不带完整 table/lifecycle 测试。ABI 版本调整与裁剪边界见 [10 月 22 日记录](../daily/2026-10-22.md)。

已从 PR2 移除根 Makefile 的 REGRESSION_TESTS 参数及公共回归脚本的筛选扩展，恢复官方默认行为；保留 device runner 中的 C 测试接线。临时 DCAP 下载代理前缀已撤销，不进入提交。dm 分支的原有筛选能力未随此次 PR2 清理删除。

## 4. 本轮历史

| 动作 | 结果 |
|---|---|
| 更新前置链 | PR1 rebase 到最新 main 为 `2dd1bcac5`，随后已推送 fork。 |
| PR2 rebase | `ea1a9639c` → `fabadd3cb`，使用 `--onto mapped-bio-ranges 2f4fdc9a7`，无冲突，range-diff 为 `=`。 |
| 清理范围 | 撤销临时代理改动，将两个公共构建/runner 文件恢复到 PR1 基线；空白与 Shell 语法检查通过。 |
| amend | `fabadd3cb` → `808d36daf`，提交说明同步移除筛选功能；控制实现和直接测试主体未改变。 |

| 测试目录整理 | 2026-10-27 将 `tests.rs` 移至 `tests/control_abi_tests.rs`，新增仅声明子模块的 `tests/mod.rs`，调整导入层级；`808d36daf` → `074e7e80d`。原有七个测试函数体逐项比对不变，生产代码不变。 |
| 后继重放与推送 | PR3 原补丁重放到新 PR2，补丁逐字节一致；PR2/PR3/当前待拆分 PR4 已原子推送，并核验远端 SHA。 |

此次整理替换原 PR2 提交，不增加单独的重命名提交，也不提前引入管理命令测试或共享缓冲区辅助函数。
用户随后明确要求删除备份分支，九个本地 `*-backup-*` 分支已删除；磁盘备份和 stash 未清理。

## 5. 验证证据与边界

### 用户反馈通过的检查

用户反馈 PR2 的格式、静态检查、普通内核构建及内核专项测试通过。命令按本轮确认的入口记录：

```sh
make format
make check
make kernel
cd kernel/core
cargo osdk test
```

最后一项是 aster-core crate 测试，包含 DM control ktest，不是独立 DM crate 或 C 回归。未归档用户各次运行的原始输出、实际计数及精确源码树，不虚构退出码；make format 是格式化步骤，不是功能测试。源码中七个 DM ktest、七个 C 场景不作为实际通过计数。

上述反馈不能表述为助手在最终 `074e7e80d` 上完整重跑通过。
10 月 26 日清理参数扩展后的助手检查仅包括 diff --check、Shell 语法及当时六文件范围；未重跑构建/运行测试。
10 月 27 日目录整理时另行核验七个测试函数体不变、生产代码不变，以及提交的空白检查通过。
后继 PR4 源码树上 make format、make check 已由助手执行成功，用户也反馈测试全部通过，但这些不能替代 PR2 独立源码树的最终运行证据。
不列“PR2 定向 C 回归通过”，最终 PR 已无该筛选参数扩展。

### 官方全量用户态回归：基线可复现的阻塞

实际入口：

```sh
make run_kernel AUTO_TEST=regression
```

PR2 `fabadd3cb` 的全量回归在 cgroup 脚本 2.2.4 失败：两秒窗口要求 usage/user 各增长至少 1.9 秒，用户首次输出为 0.462/0.461 秒，复跑保存日志为 0.404/0.404 秒。经授权在 main `ceb65e3f8` 用同命令、同环境对照，增长为 0.476/0.476 秒并在同处失败，make 退出 2。对照期间仅保留已校验原哈希的 DCAP 下载 URL 前缀修改，未改测试阈值、KVM、CPU 数量或构建模式。

因此该失败不依赖 PR1/PR2 改动。用户随后表示已查证为上游 bug，并决定不继续排查；未提供 issue/修复链接，助手未独立核验根因。全量未完成，失败之后的覆盖不能推定通过。证据目录为 `/tmp/pr2-main-cgroup-20261026-7l3yamjc/`，临时文件不保证永久存在。

## 6. 后续动作

保持当前七文件边界及提交说明；上游页面描述使用下节草稿，发布前核对最终源码树的验证证据。
全量基线阻塞单独披露，不扩大本 PR 修复 cgroup。
下一份已制备 PR 是 [PR3：/proc/devices 枚举](PR-3-proc-devices.md)，无表管理顺延为 PR4，当前待拆分。
PR2 已推送；PR 创建、后续历史改写和文档提交仍需另行授权。

## 7. Maintainer description draft

以下为英文 PR 页面草稿，尚未发布；提交标题无需与页面标题逐字一致。

### Title

device_mapper: introduce the control device and DM_VERSION

### Summary

This PR introduces the Device Mapper control device at `/dev/mapper/control` and implements `DM_VERSION` negotiation.

It is the first DM-specific step following the mapped BIO range changes. The goal is to incrementally build a BIO-based Device Mapper framework in Asterinas, with Linux-compatible userspace control interfaces for managing mapped block devices. This change establishes the control entry point before introducing device management, mapping tables, or concrete targets.

### Design

The control device uses the Linux-compatible misc-device identity and establishes the common ioctl processing order:

1. Check the caller's `CAP_SYS_ADMIN` capability on each ioctl.
2. Validate the ioctl type.
3. Read the three version words, check compatibility, and write back the supported version.
4. Reject unsupported commands after version negotiation.

The implementation advertises DM ioctl ABI version 4.48.0, matching the Linux 6.6 reference baseline used for this work. This identifies the interface version; it does not imply support for all Linux DM commands or targets.

`DM_VERSION` accesses only the three version words rather than copying the entire `dm_ioctl` structure. A version-copyout fault takes precedence over an incompatible client version. The control file reports no poll readiness while control events are unsupported.

### Scope

Included are control-device registration, privilege and ioctl-type checks, version negotiation, unsupported-command rejection, and direct kernel and userspace regression tests.

Device creation, target registration, mapping tables, BIO mapping, suspend/resume, and global control-event notification are not included. No changes to the common regression runner or download mirrors are included.

### Follow-up direction

The next prepared change adds registered character and block driver enumeration through `/proc/devices`. Device identity and management follow separately; that change is being split into smaller review units. Later changes are intended to introduce target and table interfaces, BIO dispatch and completion, and safe device lifecycle operations. The framework will be developed before adding concrete targets. A minimal linear target will then exercise mapping creation, I/O, suspend/table replacement/resume, and safe removal through the userspace interface.

These are follow-up goals, not capabilities provided by this PR. The intent is to keep each change reviewable while introducing the necessary lifetime and error-handling guarantees alongside the functionality that depends on them.

### Testing

The following checks were reported successful during preparation:

```sh
make format
make check
make kernel
cd kernel/core
cargo osdk test
```

The kernel test command runs the aster-core tests, including the DM control tests. Subsequent cleanup removed the regression-selection extension, and the seven existing kernel tests were relocated into a dedicated test module without changing their function bodies or production behavior. Build and runtime checks have not been repeated on this final standalone PR tree.

The userspace C tests cover version compatibility, negotiation ordering, non-version field preservation, memory boundaries and faults, per-call privilege checks, and control-file readiness. Their presence is not a claim that all seven scenarios completed in the full regression run.

The official full userspace regression command, `make run_kernel AUTO_TEST=regression`, stopped at the cgroup `cpu.stat` CPU-time accounting assertion. The same failure was reproduced on upstream main (`ceb65e3f8`) with the same command and environment. The full run is not reported as passing.

### Review focus

Feedback is welcome on the initial scope, Linux-compatible negotiation and error ordering, and the incremental approach to introducing the remaining framework.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
