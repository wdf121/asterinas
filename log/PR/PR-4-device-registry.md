# PR4：设备身份管理

## 1. 状态与 Git 标识

- 提交：`0c68ed7da`；直接父提交：`aa11c7ab6`。
- 分支：`dm-device-registry`；本地与 origin 已更新并核验一致。
- 范围：9 文件，+656；新增 `kernel/core/comps/device-mapper` 组件及直接单元测试。
- 本档案对应制备提交，不以完整 `dm` 的实现替代该提交的事实。

## 2. 目标与非目标

提供独立的无表设备身份组件，供控制层使用；本项不接入 ioctl，不发布运行时块设备，也不处理映射表或 I/O。

## 3. 设计与改动范围

组件管理名称、opaque UUID、设备号及控制引用生命周期，不依赖 core 的进程、VFS 或用户地址空间设施。
Linux ioctl 解析、权限检查及 errno 转换留在 core 的控制设备层。
创建失败归还临时 minor；移除先摘除索引并拒绝新控制操作，再在生命周期锁外排空已有操作、释放 minor。
普通 Arc 快照可保留身份信息，但不阻止完成移除后的 minor 复用；旧对象析构不得释放新设备占用的号码。
没有映射表、运行时块设备发布或 I/O，也没有新增 ioctl 接线。


## 4. 历史与远端事件

已更新本地和 origin 身份管理分支，精确 lease 推送后核验 SHA 一致。

## 5. 验证证据与边界

制备阶段格式与静态检查完成，用户确认运行验证通过后授权提交。
未归档完整运行输出，不填写推测的通过计数。
后续公共 C 汇总的历史改写只改变此提交树中的 `common/test.h`，组件源码未变；不声称在每个重写后的提交点重新运行过全部测试。
下列英文部分给出复现入口，不将所有命令列为本轮重新执行的记录。

## 6. 后续动作

发布前核对上游基线及前置依赖；实际运行证据与复现命令分开记录。
文档更新不代表创建、提交或推送上游页面。

## 7. Maintainer description draft

以下英文标题和正文为维护者阅读的草稿，尚未发布。

### Title

Add tableless Device Mapper identity management

### Summary

This change introduces the `aster-device-mapper` component to manage tableless device identities and device-number ownership.
The Linux ioctl ABI remains in the kernel device layer.

### Design

The component dynamically allocates a block major named `device-mapper` and supports automatic or requested minor allocation.
It manages names and optional opaque UUIDs, supports lookup by name, UUID, or device number, and provides snapshots of registered devices.
Creation checks identity conflicts and releases temporary minor allocations on failure.

Admitted control references are distinct from ordinary object references.
Detaching a device prevents new control admission; draining existing control operations then releases its minor.
The drain runs outside the device lifecycle lock so admitted operations can finish.
Structural snapshots may outlive removal without preventing minor reuse, and cleanup of an old object cannot release a replacement device's allocation.
Creation can acquire a control reference before publication to keep the device number reserved while a future CREATE handler prepares its response.

### Scope

This change includes component unit tests, but no ioctl command integration, mapping tables, runtime block-device publication, or I/O forwarding.

### Testing

Run from the repository root in the development container, on the corresponding source revision:

```bash
make format
make check
make kernel
(
    cd kernel/core/comps/device-mapper
    cargo osdk test
)
```

Tests cover identity lookup and conflicts, requested minors, creation rollback, control-reference draining, minor reuse with surviving snapshots, and device-number ownership on cleanup.

### Review focus

Identity admission, allocation ownership, and minor reuse with surviving snapshots.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
