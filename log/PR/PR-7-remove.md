# PR7：REMOVE

## 1. 状态与 Git 标识

- 提交：`c13e94ffc`；直接父提交：`03b2d721a`。
- 位于本地 `pr-dm` 制备链，尚未建立独立正式分支或推送此提交。
- 范围：8 文件，+204/-12；移除命令、生命周期检查和直接 Rust 测试。

## 2. 目标与非目标

完成无表设备单项 REMOVE，保证控制操作排空与 minor 复用；不交付 deferred remove、映射表/I/O 排空或 uevent。

## 3. 设计与改动范围

REMOVE 以普通 Arc 定位对象，不取得自身需要排空的控制引用。
生命周期锁内校验当前实例、编码身份响应并摘除索引；锁外排空并释放 minor。
响应保留输入 dev、event_nr、target_count、open_count，不重新编码为 STATUS。
拒绝 deferred remove；不实现映射表/I/O 排空或 uevent。


## 4. 历史与远端事件

提交已在本地 pr-dm 制备链保存，尚未建立或推送对应正式分支。

## 5. 验证证据与边界

助手完成 make format、make check，用户随后授权提交。
本阶段未保存独立 ktest 的原始输出与计数，不推断其执行数量。
随后用户态 C 与 NixOS 场景覆盖了无表设备移除；这些结果不替代此提交点的全部单元测试证据。

## 6. 后续动作

发布前核对上游基线及前置依赖；实际运行证据与复现命令分开记录。
文档更新不代表创建、提交或推送上游页面。

## 7. Maintainer description draft

以下英文标题和正文为维护者阅读的草稿，尚未发布。

### Title

Add Device Mapper REMOVE ioctl support

### Summary

This change implements `DM_DEV_REMOVE` for tableless Device Mapper devices selected by name, UUID, or device number.

### Design

The handler validates that the selected object is still the manager's current instance while holding its lifecycle lock.
It detaches the device from the identity indexes, releases that lock, and then drains admitted control operations before releasing the minor number.
New control operations cannot be admitted after detachment, while existing operations can finish without being blocked by a lifecycle lock held by the remover.
REMOVE does not acquire a control reference of its own that it would then have to wait for.

The response returns the device name and UUID while preserving the caller's device number, event number, target count, and open count, and clearing the applicable flags.
It is not rebuilt as a STATUS response.
Completed removal allows minor reuse even when ordinary references to the old object remain alive.

### Scope

Deferred removal is rejected with `EOPNOTSUPP` before changing the identity.
Mapping-table and I/O draining, deferred removal, and removal uevents are outside this change.
Userspace management regression and dmsetup tests are delivered separately.

### Testing

Run from the repository root in the development container, on the corresponding source revision:

```bash
make format
make check
make kernel
(
    cd kernel/core
    cargo osdk test
)
```

Tests cover stale-object rejection, response fields and flags, identity removal, minor reuse with surviving object references, deferred-removal rejection, command decoding, and fixed-header processing.
Control-reference draining and allocation ownership are also covered by the identity component's tests:

```bash
(
    cd kernel/core/comps/device-mapper
    cargo osdk test
)
```

### Review focus

Lifecycle lock ordering, control-reference draining, and REMOVE response semantics.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
