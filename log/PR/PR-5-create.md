# PR5：CREATE

## 1. 状态与 Git 标识

- 提交：`5bd01f020`；直接父提交：`0c68ed7da`。
- 位于本地 `pr-dm` 制备链，尚未建立独立正式分支或推送此提交。
- 范围：11 文件，+1167/-24；包含组件接入、通用参数处理、CREATE 和直接 Rust 测试。

## 2. 目标与非目标

接通无表 CREATE 及其公共参数处理路径；不提前加入查询、移除、映射表或数据面。

## 3. 设计与改动范围

控制设备初始化 DmManager；参数层负责固定头部、用户复制、安全清理、分派和回写。
CREATE 使用获准控制引用保护响应生成期间的设备号。
创建已提交后发生用户回写错误，不撤销设备。
该提交不支持 STATUS、LIST、REMOVE，不包含映射表、运行时块设备发布或数据面。


## 4. 历史与远端事件

提交已在本地 pr-dm 制备链保存，尚未建立或推送对应正式分支。

## 5. 验证证据与边界

助手完成 make format、make check；用户确认验证无问题后授权提交。
core 单测入口为进入 `kernel/core` 后执行 `cargo osdk test`，不使用不受支持的 `-p` 参数或模块 selector。
未保存运行计数，不将源码测试数量当作执行结果。
公共 C 汇总后来折入祖先提交，CREATE 源码未因此改变；不声称重写后逐点重跑。

## 6. 后续动作

发布前核对上游基线及前置依赖；实际运行证据与复现命令分开记录。
文档更新不代表创建、提交或推送上游页面。

## 7. Maintainer description draft

以下英文标题和正文为维护者阅读的草稿，尚未发布。

### Title

Add Device Mapper CREATE ioctl support

### Summary

This change connects the Device Mapper identity component to the control device and implements `DM_DEV_CREATE` for tableless devices.
Userspace can create a device with a name, an optional UUID, and an automatically allocated or explicitly requested minor number.

### Design

It also introduces the shared parameter-processing path for non-version ioctls: copying and validating the fixed header, handling secure data, dispatching commands, and copying results back to userspace.
CREATE uses only the fixed header rather than reading the extra payload declared by the caller.
The implementation preserves opaque UUID bytes, validates mapper names, translates component errors into Linux errno values, and returns the new identity and tableless state.

Creation holds an admitted control reference while preparing the response so concurrent removal cannot prematurely release the device number.
If creation succeeds but the subsequent userspace copyout fails, the created device remains registered.
Secure requests clear the relevant userspace parameter region, and the owned kernel parameter buffer is wiped on release.

### Scope

STATUS, LIST, REMOVE, mapping tables, runtime block-device publication, and I/O forwarding are outside this change.
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

The tests cover creation results, identity validation, requested device numbers, fixed-header boundaries, parameter sizes, secure clearing, memory-fault ordering, and committed creation followed by a copyout failure.

### Review focus

Fixed-header boundaries, secure-data handling, and creation committed before a copyout failure.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
