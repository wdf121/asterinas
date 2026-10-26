# PR2：控制设备与 DM_VERSION

## 1. 状态与 Git 标识

2026-10-26 只读核对：分支 `dm-control-interface`，HEAD `ea1a9639cd53560f15b4f9ff3d6a5cf26f0e5c38`，直接父提交为 PR1 `2f4fdc9a7`，共同 main 基线 `77ee3b196`。尚未 rebase 到最新 main `ceb65e3f8`。本地已有独立提交，远端状态本次未核验；不把旧同名 PR3 的推送或测试沿用到此 PR。

## 2. 目标与非目标

建立 `/dev/mapper/control` 和公共 ioctl 入口：权限、命令 type、版本握手、错误优先级及未知命令拒绝。报告 ABI 4.48.0；控制事件不支持时不报告虚假 poll 就绪。

不包含 manager、设备创建、表、数据面、生产 target 或完整事件机制。后续先完成通用框架，再由最小 linear 终验，见 [系列路线](../../docs/pull_request.md)。

## 3. 范围与提取

相对 PR1 共八个文件：根 Makefile；dm_control.rs、dm_control/abi.rs、dm_control/tests.rs、misc/mod.rs；initramfs 的 device_mapper_control.c、device/run_test.sh、scripts/run_regression_test.sh。

生产实现来源为 dm 的控制入口，支持命令裁剪为 VERSION；不带完整 table/lifecycle 测试。回归选择器接线的必要性需在更新最新 main 时重新核对，不能机械保留旧构建差异。ABI 版本调整来源和裁剪边界见 [10 月 22 日记录](../daily/2026-10-22.md)。

## 4. 验证证据与缺口

- 2026-10-26 核对分支、base、diff 范围及无 merge commit。
- 源码计数：7 个 Rust ktest、7 个 C 测试场景；这不是运行结果。
- 10 月 22 日记录版本修改后 diff 检查通过，但当时尚未执行完整验证。当前未取得 ea1a9639c 最终源码树的完整 check/kernel/ktest/C 运行证据。
- 10 月 26 日用户反馈完整 dm check 与控制面集成通过，不能替代本 PR 分支验收。

## 5. 后续动作

按授权更新前置 PR 链至最新 main，核对控制公共检查顺序、上游构建变化及 dm 子集关系，在最终 PR 树运行格式/静态/构建、相关文件全部 ktest 与官方 initramfs C 回归。涉及真实工具路径时使用官方 NixOS 入口，不要求本 PR 运行尚未支持的映射数据面。未获授权不 rebase、提交或 push。

## 6. Maintainer description draft

以下为待补验证后的英文草稿，不是已发布描述。

### Title

Add the Device Mapper control device with DM_VERSION support

### Summary

- Register `/dev/mapper/control` and establish the common ioctl entry path.
- Check caller capabilities and ioctl type, then negotiate the version before rejecting unsupported commands.
- Advertise ABI version 4.48.0 and preserve version-copyout error precedence.
- Add focused kernel and userspace regression tests for compatibility, permissions, memory boundaries, and error ordering.

### Scope

This change supports DM_VERSION only. Device creation, mapping tables, data I/O, and concrete targets belong to subsequent changes. The control file reports no poll readiness while control events are unsupported.

### Testing

Final-commit runtime validation is pending. Test definitions are present, but their presence is not a passing test result. Populate this section with commands and results from the final PR source tree before publication.
