# PR3：字符与块设备注册枚举

## 1. 状态与 Git 标识

2026-10-27 更新：分支 `proc-devices`，HEAD `be3f79c88f3161f1a0f62b210760913bf7422392`。
直接 base 为 PR2 `074e7e80d77fed971cdb111290cfff0e8d9d629f`，共同 main 基线为 `ceb65e3f8`。
相对 PR2 为单提交、线性历史；提交标题为 `Add character and block device enumeration through /proc/devices`。
已推送 `origin/proc-devices`，以 `git ls-remote` 核验远端与本地一致；本轮没有创建 GitHub PR。

本档案描述 PR 分支的提交树，不表示改动已回补完整 `dm` 分支。
原来规划为 PR3 的无表管理已顺延为 PR4；当前 PR4 暂停，后续拆分，不作为既定最终交付边界。

## 2. 目标与非目标

提供 Linux 风格的 `/proc/devices`，枚举已注册的字符设备和块设备驱动名称及 major。
这既是通用 procfs 能力，也是后续 libdevmapper/dmsetup 发现 Device Mapper block major 的前置。

输出反映设备号注册，不是磁盘清单、设备节点清单或正在使用的映射清单。
字符 major 可以由不同 minor 区间共享，因此不能强行去重成每 major 一行。

不包含 DM manager、`device-mapper` block major 注册、CREATE/STATUS/LIST/REMOVE、映射表、BIO、runtime 节点或 uevent。
PR2 的 `/dev/mapper/control` 属于字符 major 10 的 `misc`，不会因此单独产生一条 `device-mapper` 字符 major 记录。
本 PR 也不固定未来 DM major 为 253，不移除 virtio-blk 等独立底层驱动的注册。

用户明确授权本项通用前置先在 PR 分支实现和验证，再按后续授权考虑回补 `dm`；这是该项的例外，不放宽整个系列的实现来源规则。

## 3. 设计与改动范围

相对 PR2 共 18 个文件，507 行新增、46 行删除；没有删除整个文件。

### 注册信息与所有权

- block 注册表保存 major 与名称，新增带名称的固定申请和动态分配接口。
- 保留原有无名称接口，使用 `unknown` 名称；扩展设备号使用 `blkext`。
- 动态 block major 继续按现有从 254 向下的顺序分配，不引入 Linux 环境中特定数字的硬编码。
- char 注册表按 `(major, first_minor)` 排序，记录区间终点和名称；区间重叠拒绝，互不重叠的区间可共享 major。
- 注册所有者释放时移除对应记录；整 major 申请与区间申请遵循同一冲突约束。

### procfs 与驱动接线

- `/proc/devices` 分别输出 `Character devices:` 和 `Block devices:` 两节。
- 获取拥有自身字符串的注册快照后再写用户缓冲区，不在 copyout 时持有注册表锁。
- 支持按 offset 读取、短读、seek 和 EOF；每次读取重新获取快照，不承诺跨多次读取的事务性一致视图。
- 为现有 virtio、NVMe、mem、misc、input、framebuffer、TTY/console、PTY 等注册路径补充名称或区间信息。

关键实现路径（均指 PR3 提交树）：

- `kernel/core/comps/block/src/device_id.rs` 及 `lib.rs`
- `kernel/core/src/device/registry/char.rs`
- `kernel/core/src/fs/fs_impls/procfs/devices.rs` 及 `mod.rs`
- 相关现有驱动的设备号注册调用点
- `test/initramfs/src/regression/fs/procfs/devices.c` 及目录 runner

Linux 语义参考为本机 openEuler Linux 6.6 源码固定提交 `5078a3a23a1e1825ec136485173ec98668cdd640`，主要涉及 `fs/proc/devices.c`、`fs/char_dev.c` 和 block major 注册。
不以本地学习改动作为参考基线，也不宣称该树为未经修改的 kernel.org Linux。

## 4. 历史与远端事件

| 动作 | 结果 |
|---|---|
| 独立前置 | 从 PR2 制备 `proc-devices`，原无表管理改称 PR4。 |
| 初次提交与推送 | `be23d3b32b9e496abc2752dc4ed4937b60147984`，用户反馈测试通过后提交并推送，远端 SHA 核验一致。 |
| PR2 目录整理 | PR2 由 `808d36daf` 替换为 `074e7e80d`；PR3 重放为 `be3f79c88`。 |
| 差异核验 | 新旧 PR3 相对各自 parent 的二进制补丁逐字节一致；PR2 原七个测试函数体及生产代码不变。 |
| 最终推送 | 精确 lease 更新 `origin/proc-devices`，与 PR2 和当时 PR4 分支原子推送；远端 SHA 已核验。 |

推送分支不等于创建或合并上游 PR。文档更新不包含重新推送或回补 `dm` 的授权。

## 5. 验证证据与边界

### 直接测试覆盖

Rust ktest 覆盖 block 名称快照排序与所有者生命周期、char 区间冲突与非法参数、共享 major、procfs 两节隔离以及部分读取与 EOF。
C 回归包含两个源码场景：

- `devices_sections_and_registered_names`：章节、排序、已注册名称，以及 `/dev/mapper/control` 的字符设备号 10:236。
- `devices_partial_reads_seek_and_eof`：完整读取与 7 字节分块读取一致、seek、EOF 与零长度读取。

C 文件为 `test/initramfs/src/regression/fs/procfs/devices.c`，已接入官方目录 runner，guest 中对应 `/test/fs/procfs/devices`。
源码测试数量不等于实际执行计数。
本 PR 未增加独立 NixOS 套件；真实 dmsetup 用例属于后续无表管理，不冒充本 PR 已有功能。

### 已有结果

用户在初次提交前反馈枚举测试通过，并授权提交和推送；本次审阅文档时进一步确认，PR3 制备期间以下检查均通过：

- `make check`。
- `make kernel`。
- block 组件和 aster-core crate 中的 `cargo osdk test`。
- guest 中的 `/test/fs/procfs/devices` 回归，覆盖章节、注册名称、排序、控制节点设备号 10:236，以及分块读取、seek、零长度读取与 EOF。

用户还在 guest 中执行 `cat /proc/devices`，人工检查了输出。
以上为用户确认的运行结果，不是助手独立重跑记录。
本档案没有保存该轮全部命令输出、精确执行计数及逐项退出码，因此不补写不存在的独立运行日志，也不据此宣称全量用户态回归通过。
PR2 历史上的官方全量回归 cgroup 阻塞见 [PR2 档案](PR-2-control-interface.md#5-验证证据与边界)，不将其改记为 PR3 新失败。

目录整理后的 PR3 补丁保持不变，助手核验了历史差异和空白检查，但没有在最终独立 `be3f79c88` 树重新执行构建或系统测试。
后继 PR4 的静态检查及用户反馈不能替代这里的独立验证。

## 6. 后续动作

发布上游 PR 前核对最终源码树的验证证据，保持字符/块注册枚举的通用范围。
PR4 当前实现已在 `dm-device-management` 的 `6995e5ab3` 保存并推送，但用户决定暂缓、后续拆分；本次不拟定新的拆分边界，不改写该分支，也不继续后续功能。
具体技术债和后续路线不得用旧编号自动推进，见 [系列规划](../../docs/pull_request.md)。

## 7. Maintainer description draft

以下英文标题和正文为维护者阅读的草稿，尚未发布。

### Title

Add character and block device enumeration through /proc/devices

### Summary

This PR adds `/proc/devices` with separate character and block sections containing registered driver names and major numbers.
It provides general device-registration visibility and a prerequisite for discovering the Device Mapper block major in later userspace workflows.
It does not add Device Mapper management commands or register a Device Mapper block major itself.

### Design

Block major reservations retain their driver names for the lifetime of their owners.
Existing unnamed allocation APIs remain available, while named variants allow drivers to publish meaningful entries.
The existing dynamic allocation order is preserved.

Character registrations retain their minor intervals and names.
Disjoint intervals may share a major and remain separate rows; overlapping reservations are rejected.
The existing character drivers are connected to these named reservations, including drivers sharing the TTY major.

Procfs takes owned snapshots of both registries before copying output to userspace.
Rows are sorted within each section, and the file supports partial reads, seeks, and EOF.
Each read takes fresh snapshots rather than maintaining a stable per-open view across registration changes.

### Scope

The change includes named registration APIs, existing driver call-site updates, the procfs file, kernel tests, and an initramfs C regression.
The output describes number reservations rather than individual device nodes or active disks.
In particular, `/dev/mapper/control` remains part of character major 10 (`misc`).
No fixed Device Mapper block major, mapping devices, tables, data path, or uevent support is introduced.

### Testing

The following checks passed during PR preparation:

- `make check`
- `make kernel`
- `cargo osdk test` in both the block component and aster-core crates.
- The initramfs regression at `/test/fs/procfs/devices`, covering:
  - Character and block sections, registered driver names, and ordering.
  - `/dev/mapper/control` identity as character device 10:236.
  - Chunked reads, seeking, zero-length reads, and EOF.

The output was also manually inspected with `cat /proc/devices` in the guest.

After validation, the preceding PR reorganized its tests without changing their function bodies or production code.
This PR was replayed with a byte-identical patch relative to its new parent; build and runtime tests were not rerun after that history-only update.

### Review focus

Feedback is welcome on character minor-region ownership, registration lifetime, and the distinction between registration enumeration and device-node discovery.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
