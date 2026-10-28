# PR8：用户态 C 管理回归

## 1. 状态与 Git 标识

- 提交：`1e0415869`；直接父提交：`c13e94ffc`。
- 位于本地 `pr-dm` 制备链，未建立独立正式分支或推送此提交。
- 范围：2 文件，+521；新增 520 行 `device_mapper_management.c`，device runner 增加一行。
- 现有 Makefile 自动发现 C 文件，无需额外构建接线。

## 2. 目标与非目标

通过真实用户态 ioctl 验证无表管理 ABI；不以原始 ioctl 回归替代 dmsetup 工具集成或块 I/O 验证。

## 3. 设计与改动范围

新增 `device_mapper_management.c`，并接入 device 目录 runner；沿用 Makefile 自动发现与公共检查汇总。
覆盖 CREATE、STATUS、LIST、REMOVE、身份冲突及复用、opaque UUID、guard page、安全清理与失败回写边界。

## 4. 历史与远端事件

提交已在本地 pr-dm 制备链保存，尚未建立或推送对应正式分支。

## 5. 验证证据与边界

助手完成格式、静态检查；用户在 guest 重测 control 为 64 passed、management 为 180 passed。
这些数字是检查次数，不是 FN_TEST 场景数量；management 源码包含九个场景。
程序级总汇总已折入控制设备提交 `d5347826f` 的公共 test.h，本提交不重复修改公共框架。
测试不涉及映射表或真实块 I/O；原始 ioctl 测试不代表 dmsetup 集成验证。

## 6. 后续动作

发布前核对上游基线及前置依赖；实际运行证据与复现命令分开记录。
文档更新不代表创建、提交或推送上游页面。

## 7. Maintainer description draft

以下英文标题和正文为维护者阅读的草稿，尚未发布。

### Title

Add userspace regression tests for Device Mapper management

### Summary

This change adds raw-ioctl userspace regression coverage for tableless Device Mapper management and registers the executable in the device regression runner.
The existing regression Makefile discovers the new C source automatically.

### Design

The tests exercise CREATE, STATUS, LIST, and REMOVE through `/dev/mapper/control`.
Coverage includes identity lookup and reuse, opaque UUIDs, selector and query flags, version negotiation before parameter errors, fixed-header guard-page boundaries, secure-data faults, committed creation followed by failed copyout, and raw list record layout and empty-list copyout.
They also check that a requested-minor conflict takes precedence over an identity collision.

### Scope

This is ABI regression coverage, not dmsetup integration or mapping-table and block-I/O validation.
It uses the existing common test framework, including per-function results and a program-wide check summary.

### Testing

Run from the repository root in the development container, on the corresponding source revision:

```bash
make format
make check
make kernel
make run_kernel ENABLE_REGRESSION_TEST=true
```

Inside the guest:

```sh
/test/device/device_mapper_control
/test/device/device_mapper_management
```

The guest runs reported 64 successful checks for control and 180 for management, with no failed checks.
These are assertion/check totals, not test-function counts.

### Review focus

Guard-page assertions, failure ordering, and raw list-layout coverage.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
