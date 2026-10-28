# PR6：STATUS 与 LIST

## 1. 状态与 Git 标识

- 提交：`03b2d721a`；直接父提交：`5bd01f020`。
- 位于本地 `pr-dm` 制备链，尚未建立独立正式分支或推送此提交。
- 范围：7 文件，+616/-20；合并交付两个查询命令及直接单元测试。

## 2. 目标与非目标

提供无表 STATUS 与 LIST 查询，共享身份选择及参数处理；不改变生命周期，不交付表装载、移除或事件等待。

## 3. 设计与改动范围

STATUS 复用设备选择逻辑并取得控制引用，返回身份和无表状态。
LIST 编码名称、设备号、事件字段和可选 UUID，处理字节前缀过滤、对齐、next 偏移、空列表和 BUFFER_FULL。
不实现移除、表装载、I/O 或事件等待。


## 4. 历史与远端事件

提交已在本地 pr-dm 制备链保存，尚未建立或推送对应正式分支。

## 5. 验证证据与边界

助手完成格式化与差异检查；当时资源压力较大，未启动 make check。
用户明确确认 make check 和 `kernel/core` 下的 `cargo osdk test` 通过后授权提交。
未归档完整测试日志或计数，不虚构数字；下列 make kernel 为完整复现流程，不冒充独立保存的执行证据。

## 6. 后续动作

发布前核对上游基线及前置依赖；实际运行证据与复现命令分开记录。
文档更新不代表创建、提交或推送上游页面。

## 7. Maintainer description draft

以下英文标题和正文为维护者阅读的草稿，尚未发布。

### Title

Add Device Mapper STATUS and LIST ioctl support

### Summary

This change implements `DM_DEV_STATUS` and `DM_LIST_DEVICES` for tableless Device Mapper devices.
Both commands reuse the existing ioctl parameter-processing path without changing device identities or lifecycle state.

### Design

STATUS selects a device by name, UUID, or device number and rejects invalid selector combinations.
It acquires an admitted control reference while preparing the identity and state response.
Querying inactive state does not invent mapping information for a tableless device.

LIST returns device names and numbers, with optional UUID extensions and name or UUID prefix filtering.
Records follow the Linux ABI layout, including eight-byte alignment, next-record offsets, event fields, and UUID flags.
The implementation handles empty lists, insufficient output space, `DM_BUFFER_FULL_FLAG`, and copyout boundaries.
UUID lookup, filtering, and output preserve opaque bytes rather than requiring UTF-8.

### Scope

Device removal, mapping-table loading, I/O forwarding, and event waiting are outside this change.
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

Tests cover device selectors and number decoding, opaque UUIDs, list layout and filtering, empty and undersized output buffers, fixed-header processing, secure clearing, and memory-fault ordering.

### Review focus

Selector validation, opaque UUID handling, and list layout and copyout boundaries.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
