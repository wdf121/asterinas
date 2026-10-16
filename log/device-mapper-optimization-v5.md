---
date: 2026-10-16
mode: files
files: kernel/core/src/device/misc/device_mapper.rs,kernel/core/src/device/misc/device_mapper/control.rs,kernel/core/src/device/misc/device_mapper/runtime.rs
head: 6ae38b409-dirty
branch: dm
title: "Device Mapper \u7b2c\u4e94\u7248\u4f18\u5316\uff1a\u63a7\u5236\u9762\u6e90\u7801\u5bf9\u7167"
---

# Summary

## 第五版结论与执行边界

**2026-10-16 收尾：已批准的现有功能优化、控制面拆分及约定测试完成，用户确认本阶段告一段落。** Rust 控制面迁为 `dm_control.rs` / `dm_control/`，工作流为 `workflow.rs`；C 回归保持单文件。根盘问题待定，不作为本阶段阻塞；不追加专项排查。

验证：`make check`、release 构建通过；DM component 既有结果 91/91，core 265/265；C 回归 457/0 与 55/0；用户执行的 control-plane 和完整 linear-integration 均 PASS，Claude 已核对日志。完整日期、证据和执行者边界见[今日日志](daily/2026-10-16.md)。未提交、未 push。

范围：V5.F01、V5.B02 和 A06 非 UTF-8 通用表示仍暂缓；V5.D01 的 docs 边界说明本次更新，book/SCML 尚未修改，保留后续文档项。临时 ext2/sync 诊断不属于 DM 正式修复。

**下文为实施前审查快照**：31 条意见对应 30 个候选编号（A08 从两个视角记录），其旧路径、行号、问题描述及“待决定/未执行”反映当时状态，不覆盖以上收尾结论。保留历史证据，不将旧问题描述误当当前代码。

### 审查时基线与覆盖

- Asterinas：`dm`，HEAD `6ae38b4095e3294e83454e3bbb26593a5280f74d` 加现有快照修复；主文件 SHA256 `60efb1d3a0dab1f99f76d1677b07da441fa615bae279feef77cda74a18242e4a`。审查结束时六个核心文件哈希与开始一致。
- Linux：本机 `/root/Linux-tree-code/kernel`，openEuler origin `https://gitee.com/openeuler/kernel.git`，HEAD `5078a3a23a1e1825ec136485173ec98668cdd640`、tag `6.6.0-132.0.0`、Makefile 6.6.0；不是未经修改的 kernel.org v6.6。已跟踪本地修改仅 dm-linear.c 学习注释/空白；不将其当作上游证据。
- 本机 UAPI 为 **4.48.0**（include/uapi/linux/dm-ioctl.h:288-290）；当前 dm 报告 **4.50.0**。内核版本与 ioctl ABI 版本不同；本轮不自动降版本，也不以版本号推定完整能力。
- 入口：[device_mapper.rs](../kernel/core/src/device/misc/device_mapper.rs)；工作流：[control.rs](../kernel/core/src/device/misc/device_mapper/control.rs)；运行期协调：[runtime.rs](../kernel/core/src/device/misc/device_mapper/runtime.rs)。沿调用链检查 component device/manager/table、block registry、VFS、设备模型与事件基础设施。
- 覆盖权限/版本/输入输出、create/remove/remove-all/rename、load/clear/suspend/resume、status/deps/list/version/wait、引用锁与回滚、节点和通知责任。数据面 target 映射/拆分/完成算法、通用时钟、根盘修复、PR 分支不在本轮全面审查范围；I/O 排空作为控制面的前置条件纳入。
- 证据性质：静态源码事实与可达输入/交错推导；下文“例子”与“验证”是后续测试设计，不是本轮实测。

### V5.00：已实现的固定头快照修复

**状态：已实现、部分验证、未提交，普通 DM C 回归未完成。** Linux copy_params（dm-ioctl.c:1960-2010）沿用内核固定头，只补读 payload；旧 dm 第一次头负责分配、第二次头负责执行。当前工作区 read_ioctl_header/copy_ioctl_payload 保留协商版本与一次头快照，不再从用户空间重读头；地址加法溢出返回 EFAULT。仍保留 312 字节门槛、整包输出等旧行为，不能据此宣称全部 ABI 对齐。

| 已实际执行 | 结果 | 边界 |
|---|---|---|
| 容器根目录 `make check` | exit 0，无 warning:/error: | 静态检查，不代表运行时 |
| kernel/core 下 `cargo osdk test --kcmd-args=earlycon` | 245 passed / 0 failed / 0 filtered，exit 0 | 新增三项逐条串口确认通过；没有用 selector 零匹配充当通过 |
| `make run_kernel AUTO_TEST=regression REGRESSION_TESTS=device/device_mapper_version` | 49 passed / 0 failed，exit 0 | selector、程序通过与总通过标记齐全；不是普通命令竞态测试 |
| 普通 `device/device_mapper` C 回归 | **未运行** | 用户改为整块审查后集中决定实施和测试 |

三项新增 ktest：头 A 捕获后用户头全部变成 B，结果仍使用 A；头/载荷复制错误传播；无 payload 不再读取。复制错误后的不分派由 helper 返回错误及生产调用点的 `?` 共同保证，测试不等同真实 ioctl 线程竞态实验。未单独运行 make kernel；版本 C 命令包含其构建链，但不伪造额外命令。

证据在容器：`/tmp/dm-snapshot-check.log`、`/tmp/dm-snapshot-ktest.iAamcdQv/`、`/tmp/dm-snapshot-version.tfny3ZJH/`。已出现 ermsb、GRUB locale、QEMU 特性告警；版本 C 另出现 Nix hostPlatform 更名提示，报告后暂停扩大验证。不能写成无告警验收或本清单其他项已验证。

### 候选清单与建议批次

**2026-10-16 后续决策：**用户已批准现有功能修复及 V5.B01/B03；V5.F01、V5.B02 的新增能力，以及需要通用框架改造的非 UTF-8 表示暂缓。逐项实施台账与暂缓规划见 [global.md](../docs/global.md#101-第五版已批准的当前修复)。下方评审正文的“待决定”保留为审查时点记录，审查时授权以本段为准；最终实施与验证状态以文首收尾说明为准。按关联批次实现后集中测试，不提交、不 push。

| 编号 | 差异/优化项 | 分类与建议 |
|---|---|---|
| V5.A01 | ioctl type/nr 之外额外校验 direction/size | ABI；批次一 |
| V5.A02 | 305/312 固定头、NO_PARAMS、命令输入/输出起点 | ABI；批次一 |
| V5.A03 | 有效输出长度与普通失败整包回写 | ABI；批次一，与 A02 统一设计 |
| V5.A04 | 输出 flags 预清、命令组合、自定义 EXISTS bit | ABI；批次一 |
| V5.A05 | selector 组合误按优先级接受 | ABI；批次一 |
| V5.A06 | 固定字符串归一、诊断提前拒绝、UTF-8 边界 | ABI＋表达限制；批次一，字节路径支持另决策 |
| V5.A07 | 名称列表事件扩展、前缀过滤、空/不足输出 | 查询编码；批次二 |
| V5.A08 | WAIT 缺表状态 payload 与查询 flags | 查询＋生命周期；批次二/三共用修复 |
| V5.A09 | 公共状态头 target_count 丢失 | 查询编码；批次二 |
| V5.A10 | target-version 列表不足空间行为 | 查询编码；批次二 |
| V5.A11 | 额外消费末项 target next | 表输入；批次二 |
| V5.A12 | 参数转义与相对 backing 路径 | 表输入；批次二 |
| V5.S01 | secure-data 未清用户输入又回写原参数 | 安全契约；批次一，与 A02/A03 联动 |
| V5.S02 | 部分复制/copyout 错误及 String 副本逃过清理 | 安全契约；批次一，需可靠清零原语 |
| V5.L01 | NOFLUSH 错误跳过在途 BIO drain | **优先：换表静止点**；批次三 |
| V5.L02 | suspend/隐式 suspend 等待不可被信号中断 | 生命周期及回滚；批次三 |
| V5.L03 | UUID 可覆盖既有值 | 身份生命周期；批次三 |
| V5.L04 | rename 提交后才发现过长；create/rename 漏名字校验 | **优先：失败后状态已变**；批次三 |
| V5.L05 | 已开始 WAIT 遇 remove 被 current 校验拒绝 | 引用/删除语义；批次三 |
| V5.L06 | 无 active table 时仍发布设备事件 | 事件语义；批次三 |
| V5.L07 | Running 空表设备 resume 不幂等 | 状态机；批次三 |
| V5.L08 | REMOVE_ALL 不在成功删除后重扫 | 生命周期；批次三 |
| V5.F01 | 默认 lockfs 的 freeze/sync/thaw 协议缺失 | **框架前置，单独决定**；不能以 BIO drain 替代 |
| V5.M01 | 工作流类型未表达持锁及当前实例约束 | 维护性建议，不是已证实并发漏洞 |
| V5.M02 | rename 事件发布责任散落在 façade | 与 L06 同批处理，不能只移动错误通知 |
| V5.M03 | 字符串字段标签形参被忽略 | 维护性建议 |
| V5.D01 | Linux 兼容文档/SCML 缺 DM 能力边界 | 实施后按实际能力更新，不提前宣称支持 |
| V5.D02 | selector 注释误称 Linux 接受冗余输入 | 与 A05 同批处理 |
| V5.D03 | compact name-list 注释误称 Linux 格式 | 与 A07 同批处理 |
| V5.D04 | alias/manager rename 跨层原子性承诺过强 | 文档纠正，不自动重构跨层锁 |

**建议优先级：**安全清理与错误回写、rename 先改状态后失败、NOFLUSH 静止点优先于纯格式/注释整理。A02/A03/S01/S02 共用输入—执行—输出—释放生命周期；A04/A07/A08/A09/A10 共用输出规范化但分别验收；L01/L02/F01 共用 suspend 状态机分析，不可把缺少 freeze 的问题随 drain 修复一并标成解决。

### 需要用户取舍的范围项

- **V5.B01｜输入资源上限：**本机 dm-core.h:25-26 与 dm-ioctl.c:1973-1977 允许最高 `1048576*1024` 字节；当前限制 1 MiB。属于明确的资源策略偏离，不能为“逐行相同”直接改成 1 GiB。需决定支持上限及分配失败策略，后续测试边界与 ENOMEM；本轮不称为内存破坏缺陷。
- **V5.B02｜设备生态与未实现命令：**当前支持命令 0..13、17，target 为 error/linear/striped/zero；TARGET_MSG、SET_GEOMETRY、ARM_POLL 返回 ENOTTY，deferred remove/IMA/DM-on-DM 有明确限制。是否新增能力需单独选择。已有 [block sysfs 接线](../kernel/core/src/device/registry/block.rs#L850) 和 [uevent socket 基础设施](../kernel/core/src/net/socket/netlink/kobject_uevent/mod.rs)，但 DM 专属 sysfs 属性、DM uevent/cookie 与用户态 udev 同步链未接通；内核直接创建 mapper alias 不代表 udev 已实现。设备 event_nr、全局 poll 事件、kobject uevent 是不同通知链。
- **V5.B03｜错误码与顺序的第二层细化：**已确认的查找、UUID、rename 长度等错误顺序见对应条目；例如 manager NameExists/UuidExists 统一映射 EEXIST，而 Linux rename 目标占用是 EBUSY，应在 L03/L04 同批区分。未穷举所有数字解析、ENOMEM/ENOSPC、权限/设备错误组合；不能把本块审查完成表述为全输入空间的兼容证明。

### 已核对的正确行为与未成立候选

- 每 ioctl 的 initial-user-namespace CAP_SYS_ADMIN 检查、独立 12 字节握手、major/minor 条件及版本 copyout 错误优先级已存在；此次不重复作为新修复。
- **create 后没有立即发布主节点不是本基线缺陷。** Linux table_load→dm_setup_md_queue→add_disk 才首次发布；不能凭旧经验要求 create 即可 open。
- DEV_STATUS 已按 active/inactive 快照取 target_count 和 readonly；不把公共 encoder 其他调用的缺口误报为 DEV_STATUS 全部错误。
- mutation 在取得生命周期锁后复核 Arc-current 能防止操作已删除实例；问题是把该规则用于已开始的 WAIT，而非删除该检查。
- remove 的 open_count/admission、tracked lease/Removing 检查各在对应锁下；本轮未确证这里有 TOCTOU 权限突破。
- rename reservation 在 alias 失败时释放新名字预留；runtime 删除补偿失败保留 Removing 供重试。未找到可证明的生产死锁或 reservation.commit panic，不凭假设添加缺陷。
- table load 先解析、临时 lease RAII 释放、clear 空 inactive 幂等均已有实现；这些结论是静态追踪，不替代故障注入测试。
- 既有测试把某些差异固定为预期（例如 noflush 不 drain、可覆盖 UUID、tableless 事件）；后续需要从 Linux 行为重写断言，不能以旧测试通过否定源码事实。

### 后续集中验证建议（未执行）

1. 用户选定批次后，先实现并补对应断言，再一次性运行 make check、make kernel 和影响 crate 的 ktest；core 使用无 selector 全量加串口核对，不用旧模块 selector 假绿。
2. ABI/错误内存边界用 raw C 回归：保护页、哨兵、输出长度、flags/selector、secure-data、列表二进制布局。
3. 生命周期用可控延迟 BIO、阶段屏障及故障注入：noflush drain、信号撤销、WAIT/remove、rename 失败状态不变；不依赖 sleep 猜测竞态进入。
4. 仅在节点/控制生命周期或真实 LVM 路径受影响时选 control-plane/topology/相关集成套件，串行执行并记录镜像来源；不自动全跑六套。
5. F01 若获批，另设挂载文件系统脏页/并发写入及 thaw 失败回滚验收；未补框架时只说明局限，不能宣称默认 suspend 已提供一致磁盘视图。

以下按评审视角保留具体症状、源码证据及修复建议。共同修复不合并掉不同症状；确认结论为源码审查结论，所有运行时建议仍未验证。

## Maintainability

### `kernel/core/src/device/misc/device_mapper.rs` line 635

> ```diff
> with_current_device_lifecycle(&device, |device| {
>     workflow.rename(device, &request)?;
>     device.notify_event();
> ```

`single-responsibility` (minor): **V5.M02｜已确认，待决定。** 重命名事务的事件发布仍由 ABI façade 直接执行：`workflow.rename(...)` 成功后，调用者必须另外记得 `device.notify_event()`；而删除事务已在 `ControlWorkflow::remove_with_unregistration` 内负责发布事件。同属控制工作流的生命周期副作用因此有两种归属，新调用者即便正确持有生命周期锁，直接使用 `ControlWorkflow::rename` 也只会得到身份变更，不会得到当前 ioctl 路径的完整事务。Linux 基线 `5078a3a23` 的 `drivers/md/dm-ioctl.c:429-514` 将重命名及相应事件处理放在 `dm_hash_rename` 中，外层 `dev_rename:1075-1080` 再填充状态；这里引用的是责任边界，不将两边事件条件的语义差异归入本条。

**Fix.** 与 V5.L06 共用事件发布条件和职责修复；最终采用 Linux live-table 条件，不保留无条件通知。将当前重命名成功后的 `notify_event()` 收入 `ControlWorkflow::rename` 的统一成功路径，ABI façade 只负责解码、调用、诊断和编码。保留当前成功后通知、失败不通知及持锁范围，不借此重构改变 Linux 事件条件；事件语义对齐应单独审定。已有重命名及等待事件测试可作为后续修改的定向验证入口。

### `kernel/core/src/device/misc/device_mapper.rs` line 1315

> ```diff
> fn optional_c_string(
>     buffer: &[u8],
>     offset: usize,
>     len: usize,
>     _field: &str,
> ) -> Result<Option<String>> {
> ```

`least-surprise` (minor): **V5.M03｜已确认，待决定。** 字符串解析接口要求每个调用者提供字段标签，但 `optional_c_string` 和 `c_string_until` 都将其命名为 `_field` 并完全忽略，`required_c_string` 也只是继续传递。调用点分别提供名称、UUID、target 类型或参数等标签，看起来会形成字段特定诊断，实际所有这些输入都会返回同一组泛化错误。具体例子是名称和 UUID 各自缺少 NUL 时，传入不同标签不会改变诊断；大量调用点因此维护着没有任何作用的参数。

**Fix.** 要么删除这组解析函数的字段参数及所有无效实参，要么让它实际选择字段特定的错误消息。当前 `Error::with_message` 接收 `&'static str`，若保留字段诊断，可使用字段枚举映射静态消息，不必为此改变错误类型或分配动态字符串。

### `kernel/core/src/device/misc/device_mapper/control.rs` line 262

> ```diff
> /// Callers must hold the mapper lifecycle guard before invoking per-device
> /// methods. The ioctl façade retains that guard because it also owns response
> /// header encoding, which must observe the same current mapper instance.
> pub(super) struct ControlWorkflow<'a> {
>     manager: &'a DmManager,
> }
> ```

`rust-type-invariants` (minor): **V5.M01｜已确认，待决定。** 生命周期工作流的核心前置条件只存在于类型注释中：`load_table`、`remove`、`rename`、`transition` 都接受普通的 `&Arc<DmDevice>`，但必须已经持有该设备的生命周期锁，并完成当前实例及隔离状态复核。实际保证位于父模块的 `with_current_device_lifecycle_in_with_policy`，工作流接口本身无法区分经过复核的设备和普通查找结果。具体维护场景是增加一个调用 `ControlWorkflow::rename` 的控制入口时，可以完全绕过这些步骤而正常编译；判断该调用是否满足接口契约必须追溯调用链。当前生产调用点已有包装，因此这里报告的是接口未表达关键约束的维护缺陷，而非已证实的并发故障，也不要求照搬 Linux 的锁实现。

**Fix.** 在控制层引入只能由生命周期包装器构造的受保护设备上下文，保存对应设备引用及生命周期 guard，并在构造时完成当前实例和隔离策略复核；让依赖这些前置条件的工作流方法接收该上下文。保持现有锁获取、复核、状态变更和响应编码顺序，让上下文继续存活至响应编码结束；不要简单将加锁移入方法后提前释放，也不要改变允许清理隔离设备的策略。

## Correctness

### `kernel/core/src/device/misc/device_mapper.rs` line 318

> ```diff
> let write_result = current_userspace!().write_bytes(raw_ioctl.arg(), &buffer);
> let result = match command_result {
>     Ok(()) => { write_result?; Ok(0) }
>     Err(error) => { let _ = write_result; Err(error) }
> };
> ```

Output size and failure writeback mismatch (major): **V5.A03｜已确认，待决定。Linux：**`ctl_ioctl:2105-2124` 保存输入容量，先将输出 `data_size` 设为 305，仅在 handler 成功时复制 `param->data_size` 字节；各输出命令填写实际大小，remove-all 设置 0（533-537）。**当前 dm：**生产路径没有写 `OFF_DATA_SIZE`，成功或失败都尝试回写整个分配 buffer。**差异/影响：**成功命令可能因未使用的尾部只读页而 `EFAULT`；失败请求也会改动版本以外的内存；列表和状态响应把输入容量伪装成输出长度。权限/版本失败仍在此流程之前，不受这条影响。

**Fix.** 与 V5.A02 共用命令边界机制，区分输入容量与输出有效长度。**目标：**只有 handler 成功才回写有效输出；普通失败仅保留之前版本协商的写入（secure-data 擦除另列）。**例子：**status 的 header 可写而未使用的尾页只读，当前可返回 EFAULT，Linux 不写尾页。**验证：**C 哨兵/保护页覆盖成功、参数错误、handler 错误、copyout 错误及 remove-all；检查返回 errno、data_size、实际变更字节和设备状态。

### `kernel/core/src/device/misc/device_mapper.rs` line 359

> ```diff
> let mut header = [0u8; DM_IOCTL_HEADER_SIZE];
> read(version.len(), &mut header[version.len()..])?;
> // ...
> if !(DM_IOCTL_HEADER_SIZE..=DM_IOCTL_MAX_SIZE).contains(&data_size) {
> ```

Command input boundary mismatch (major): **V5.A02｜已确认，待决定。Linux：**`dm-ioctl.c:1965-2009` 的最小固定输入是 `offsetof(dm_ioctl, data)=305`，`lookup_ioctl:1877-1898` 将 create/remove/suspend/status/clear/remove-all 标为 `NO_PARAMS`，这些命令不读额外 payload；`get_result_buffer:557-567` 自行设置输出起点 312。**当前 dm：**统一读取 312 字节、统一要求 `data_size>=312`、`data_start>=312` 且 8 字节对齐，并对所有普通命令读取声明的整段 payload。**差异/影响：**无 payload 命令会因本不需要访问的页面 `EFAULT`，或因无意义的 `data_start` 返回 `EINVAL`。

**Fix.** 与 V5.A03 共同建立逐命令输入/输出描述，但保留各自验收项。**目标：**305 字节固定字段；输入 payload 命令单独校验自身起点；纯输出命令由内核确定起点；无参数命令不读取尾部。**例子：**create 仅前 305 字节可访问，声明较大 size 且尾页不可读，Linux 无需读尾页，当前 dm 失败。**验证：**保护页 C 用例覆盖 305/311/312、不同 data_start、NO_PARAMS 与真实输入命令；未知和 VERSION 仍按先前契约处理。

### `kernel/core/src/device/misc/device_mapper.rs` line 398

> ```diff
> if raw & !0xff != DM_IOCTL_COMMAND_PREFIX || (raw >> 8) & 0xff != DM_IOCTL_MAGIC {
>     return_errno_with_message!(Errno::ENOTTY, "未知的 Device Mapper ioctl 命令");
> }
> ```

Command decoding mismatch (minor): **V5.A01｜已确认，待决定。Linux 源码事实：**本机 `dm-ioctl.c:2068-2095` 只以 `_IOC_TYPE` 和 `_IOC_NR` 识别 DM 命令，版本命令在查表前直接返回。**当前 dm：**`decode_command` 还要求方向与编码大小严格等于 `0xc138_fd00`。**差异/影响：**相同 DM type/nr 但不同 size/direction 的请求，Linux 进入版本或命令处理，dm 在版本回写后返回 `ENOTTY`；这是用户 ABI 差异，不是内存安全防护必需条件。

**Fix.** **建议目标：**按 type/nr 解码，访问长度由命令参数契约决定，不依赖 ioctl 编码的 size。**例子：**`_IO(0xfd, 0)` 配合法的 12 字节版本地址，Linux 成功，当前 dm 拒绝。**验证：**C 回归覆盖合法/非 DM type、已知/未知 nr 和不同 direction/size；不得改变权限及版本错误优先级。

### `kernel/core/src/device/misc/device_mapper.rs` line 462

> ```diff
> let flags = read_u32(buffer, OFF_FLAGS)?;
> if flags & !DM_KNOWN_FLAGS != 0 { /* EINVAL */ }
> // ...
> let mut flags = DM_EXISTS_FLAG;
> ```

Flag normalization mismatch (major): **V5.A04｜已确认，待决定。Linux：**`validate_params:2026-2038` 预先清除 BUFFER_FULL、UEVENT_GENERATED、SECURE_DATA、DATA_OUT；其他位由具体命令解释，不采用当前 dm 的统一白名单与组合拒绝。`dev_wait:1391-1395` 支持状态类型及 inactive 查询。Linux UAPI `dm-ioctl.h:293-309` 没有 `DM_EXISTS_FLAG` bit 2。**当前 dm：**允许用户带入输出位但没有统一清理，list 等路径可能原样返回虚假的 BUFFER_FULL/UEVENT_GENERATED；一部分 Linux 忽略的位被拒；WAIT 的 STATUS_TABLE/QUERY_INACTIVE 被拒；状态填充还自行设置 bit 2 并整体覆盖 flags。**影响：**调用者可能错误重试、等待未发生的 uevent，或不能使用合法 WAIT 查询模式。

**Fix.** **目标：**建立命令级输入/输出 flags 表，统一清理输出位、逐命令更新状态位，移除自定义 ABI bit；未实现的有意义能力单独保留明确拒绝，不机械接受。WAIT 组合与 V5.A08 一起处理。**例子：**足够大的 LIST_VERSIONS 输入置 BUFFER_FULL，当前成功响应仍带 BUFFER_FULL。**验证：**各命令输入输出位矩阵，尤其伪造输出位、忽略位和实际不支持位；更新把 bit 2 当正确结果的旧断言。

### `kernel/core/src/device/misc/device_mapper.rs` line 594

> ```diff
> wait_for_device_event(buffer, &device).map_err(restart_interrupted_wait)?;
> with_current_device_lifecycle(&device, |device| fill_device_header(buffer, device))
> ```

Incorrect removal wakeup result (major): **V5.L05｜已确认，待决定。** 已进入 WAIT 的线程在设备被删除时返回 `ENXIO`，而不是已持有设备引用的最终状态。具体交错：WAIT 完成 lookup 并睡眠；remove 在 lifecycle 锁内 unregister、`notify_event()`、manager detach；WAIT 被唤醒后必须等待同一 lifecycle 锁，之后 `is_current()` 必定失败。Linux `/root/Linux-tree-code/kernel/drivers/md/dm-ioctl.c:308-336` 在删除 active device 时通过 `dm_table_event()` 唤醒 waiter；`1367-1401` 的 `dev_wait()` 持有 lookup 获得的 md 引用，醒来不重新检查 hash，继续返回状态与 target records。`/root/Linux-tree-code/kernel/drivers/md/dm.c:2459-2503` 的 destroy 等待这些 holders 退出。仅把 Rust 对象通过 `Arc` 保活并不能弥补当前 ioctl 的错误返回语义。

**Fix.** 为已经成功开始的 WAIT 保留可供最后一次状态读取的设备/表生命周期，不在唤醒后套用只适用于新 mutation 的 manager-current 校验。明确删除期间表与事件快照的保留边界，并回归 active mapper 的 WAIT/remove 交错，区别于删除后才发起的新 WAIT 应返回 `ENXIO`。

### `kernel/core/src/device/misc/device_mapper.rs` line 594

> ```diff
> fn device_wait(buffer: &mut [u8]) -> Result<()> {
>     let device = lookup_device(buffer)?;
>     wait_for_device_event(buffer, &device).map_err(restart_interrupted_wait)?;
>     with_current_device_lifecycle(&device, |device| fill_device_header(buffer, device))
> }
> ```

Missing event status payload (major): **V5.A08｜已确认，待决定。** WAIT 醒来只调用通用 header encoder，永远把 `target_count` 写成 `0`，没有返回触发事件时所选表的 target status records；即使调用方提供足够 payload 空间也不会获得状态。`validate_input_flags()` 还拒绝 WAIT 上的 `DM_STATUS_TABLE_FLAG` 与 `DM_QUERY_INACTIVE_TABLE_FLAG`。Linux `/root/Linux-tree-code/kernel/drivers/md/dm-ioctl.c:1391-1396` 明确在等待后调用 `__dev_status()`、`dm_get_live_or_inactive_table()` 和 `retrieve_status()`；`1291-1306,1361` 按 flag 决定 status/table 输出并填 target count。依赖一次 WAIT 获取事件与表状态的客户端看到空表，不能只靠额外补一个事件号解决。

**Fix.** 与另一条 V5.A08 共用修复：WAIT 复用所选表状态编码、flags 和输出长度机制；保留本条生命周期视角证据。让 WAIT 成功路径复用所选表状态快照及 target-record 编码，保留查询 flag 直到完成选择，并正确处理输出空间不足。增加 active/inactive、多 target、table/status 模式和短缓冲区的 WAIT 回归；不要将它仅实现为 header-only 的 DEV_STATUS。

### `kernel/core/src/device/misc/device_mapper.rs` line 594

> ```diff
> wait_for_device_event(buffer, &device).map_err(restart_interrupted_wait)?;
> with_current_device_lifecycle(&device, |device| fill_device_header(buffer, device))
> ```

Wait response omits target status (major): **V5.A08｜已确认，待决定。Linux：**`dev_wait:1381-1396` 等待事件后填头并按 active/inactive 选择返回 target 状态记录，支持 STATUS_TABLE/NOFLUSH。**当前 dm：**等待后仅 `fill_device_header`，target_count 置零，无 target 记录；flags 入口还拒绝相关查询组合。**影响：**调用者得到唤醒但没有 Linux 承诺的变更后表信息。已有 EINTR→ERESTARTSYS 映射正确，应保留。

**Fix.** **目标：**共享 TABLE_STATUS 的选表、记录生成和有效长度规则，保留信号重启语义；已开始 WAIT 的引用保留与删除后状态读取按 V5.L05 单独处理，不能继续套用 mutation 的 current 实例复核。不只补 target_count。与 V5.A04 联动。**例子：**有一个 linear target 的设备事件变化后 WAIT 应返回一条状态记录，当前只返回头。**验证：**事件已变化/阻塞后变化、active/inactive、状态/表模式、输出不足、信号、并发 remove；检查事件号和实际记录内容。

### `kernel/core/src/device/misc/device_mapper.rs` line 625

> ```diff
> let value = c_string_until(buffer, data_start, buffer.len(), "新设备名称或 UUID")?;
> let request = if flags & DM_UUID_FLAG != 0 {
>     RenameRequest::Uuid(value)
> } else {
>     RenameRequest::Name(value)
> };
> ```

Validation after state mutation (major): **V5.L04｜已确认，待决定。** rename 的变长字符串只检查 NUL/UTF-8，没有在提交前限制 `DM_NAME_LEN`/`DM_UUID_LEN` 或校验名字。对未发布 alias 的设备传入 `128` 字节名字，`control.rs:380-383` 先经 manager 改名，随后这里 `fill_device_header()` 才因名字装不下返回 `EINVAL`；旧名字已经丢失，后续查询也不能正常编码。`129` 字节新 UUID 同理；已发布 alias 的名字只受 runtime 的 `NAME_MAX` 限制，也可先把 alias 和 manager 改为超过 DM ABI 长度的名字再报错。另外 create 和未发布设备 rename 接受 `/`、`.`、`..`、`control`；load 后 resume/remove 才遇到 runtime 校验或 alias 冲突，可能留下无法正常使用的设备。Linux `/root/Linux-tree-code/kernel/drivers/md/dm-ioctl.c:1055-1075` 在 `dm_hash_rename()` 前检查非空、长度及 `check_name()`；`770-784,888-890` 对 create 同样禁止这些名字。

**Fix.** 在 create/rename 的任何 manager、registry、devtmpfs 更新前统一校验 Linux DM 名字及 UUID 的字节长度、非空条件与保留名字。不要让 runtime 路径的较宽文件名限制替代 DM ABI 校验。验证超长名字/UUID及非法名字返回错误后，原名字、UUID、alias、事件号和索引完全不变。

### `kernel/core/src/device/misc/device_mapper.rs` line 636

> ```diff
> workflow.rename(device, &request)?;
> device.notify_event();
> ```

Spurious device event (minor): **V5.L06｜已确认，待决定。** rename 无条件增加 `event_nr` 并唤醒 WAIT，包括 create 后尚无 active table、以及仅 load inactive table 的设备；remove 的 `control.rs:331` 也无条件通知。Linux `/root/Linux-tree-code/kernel/drivers/md/dm-ioctl.c:499-502,322-325` 只在存在 live table 时调用 `dm_table_event()`，因此同样的 tableless rename 不推进设备事件号。当前 `device_wait_returns_current_header_after_rename_events` 等测试直接对 tableless 设备断言事件递增，固定了与基线相反的可见语义。此处讨论的是设备 `event_nr`，不应和 rename 的 `KOBJ_CHANGE` uevent/cookie 混为一谈。

**Fix.** 在受生命周期保护的状态下按 Linux live-table 条件发布设备事件，独立处理 kobject/udev 通知。把 rename/remove 的事件测试拆为有 active table、仅 inactive table、完全无表三种情形。

### `kernel/core/src/device/misc/device_mapper.rs` line 712

> ```diff
> let next_spec = validate_target_spec_next(cursor, next, buffer.len())?;
> let params = c_string_until(buffer, spec_end, next_spec, "target 参数")?;
> ```

Unused final target offset rejected (minor): **V5.A11｜已确认，待决定。Linux：**`populate_table:1461-1511` 用上一个 spec 的 next 定位当前项，最后一项解析完成后不再消费其 next；参数 NUL 在剩余输入范围寻找。**当前 dm：**连最后一项的 next 也交给 validate_target_spec_next，并作为其参数结束边界。**影响：**Linux 不使用的末项 next 非零且越界/不对齐时，dm 额外拒绝；这不是防止访问下一项所必需。

**Fix.** **目标：**按 target_count 决定是否需要消费 next；非末项必须验证不重叠及对齐，末项仅检查自身 spec/参数有效范围，不沿用任意 next。**例子：**一个合法 zero spec、空 NUL 参数、next=1，Linux 不消费该 next，当前 EINVAL。**验证：**一项末 next 0/1/越界、多项重叠/参数未终止/越界；保留已有范围安全检查。

### `kernel/core/src/device/misc/device_mapper.rs` line 738

> ```diff
> let request = if flags & DM_SUSPEND_FLAG != 0 {
>     LifecycleRequest::Suspend {
>         noflush: flags & DM_NOFLUSH_FLAG != 0,
>     }
> } else {
>     LifecycleRequest::Resume
> };
> ```

Missing filesystem freeze (major): **V5.F01｜已确认，待决定。** 普通 suspend 和运行中换表没有执行文件系统冻结/同步，而且 `DM_SKIP_LOCKFS_FLAG` 在解码后被丢弃。调用链最终只到 `/root/github/asterinas/kernel/core/comps/device-mapper/src/device.rs:343-365,416-470` 的 BIO admission/drain；尚在文件系统 page cache 的脏数据不属于 in-flight BIO，因而普通 suspend 可以在它们未写回、文件系统仍可改变时返回成功。Linux `/root/Linux-tree-code/kernel/drivers/md/dm-ioctl.c:1131-1147,1186-1196` 默认设置 `DM_SUSPEND_LOCKFS_FLAG`，`/root/Linux-tree-code/kernel/drivers/md/dm.c:2656-2673,2710-2722,2844-2854` 在停止 BIO admission 前 `freeze_bdev()`，resume 后 thaw。这会影响依赖默认 suspend 获得一致磁盘视图的挂载设备维护。当前具体框架缺口是 `/root/github/asterinas/kernel/core/src/fs/vfs/fs_apis/file_system.rs:11-51` 只有 `sync()`，没有 freeze/thaw 或阻止新文件系统写入的协议；不能以单纯 drain 代替冻结。

**Fix.** 将 lockfs 策略贯穿 typed request 与 resume 隐式 suspend，接入按块设备定位挂载文件系统、阻止新写入、同步及 thaw/失败撤销协议。该前置能力补齐前，应明确拒绝无法兑现的默认 lockfs 操作，保留显式 skip-lockfs/noflush 子集，而不是无声宣告冻结成功。后续验证挂载文件系统存在脏页与并发写入时的 suspend 行为。

### `kernel/core/src/device/misc/device_mapper.rs` line 897

> ```diff
> for target in QueryWorkflow::target_versions() {
>     let record_len = target_version_record_len(&target)?;
>     if available_from(buffer, cursor) < record_len {
>         set_buffer_full(buffer)?;
>         break;
>     }
> ```

Version list truncation contract mismatch (minor): **V5.A10｜已确认，待决定。Linux：**`__list_versions:707-752` 先为全部 target（或指定 target）计算 needed，不足则置 BUFFER_FULL 而不输出部分记录，并设置输出 data_size；所需量含 `sizeof(dm_target_versions)+name+NUL+7` 的保守余量。**当前 dm：**LIST_VERSIONS 边写边发现不足，返回前半列表；GET_TARGET_VERSION 使用紧凑记录长度判断，且二者都不更新 data_size。**影响：**不足空间时可见记录与边界行为不同。

**Fix.** **目标：**与 V5.A03/A07 共用输出容量/长度设计，但分别保持 Linux 各列表的 needed 规则。**例子：**空间仅容纳第一个 target，Linux 返回 BUFFER_FULL 而不提交列表，当前写出第一条。**验证：**0/一条/全部/刚好保守 needed 容量的哨兵缓冲，以及不存在的 target；检验输出位、有效长度与未改尾部。

### `kernel/core/src/device/misc/device_mapper.rs` line 983

> ```diff
> let record_len = if include_uuid {
>     // extended record
> } else {
>     align_up(12usize.checked_add(name_len).ok_or_else(invalid_buffer)?, 8)?
> };
> if include_uuid { /* write event_nr and flags */ }
> ```

Device list record and filtering mismatch (major): **V5.A07｜已确认，待决定。Linux：**`list_devices:594-672` 始终为每条名称记录附 event_nr 和 flags 两个 u32，UUID_FLAG 只控制 UUID 内容/标志；按 name/uuid 前缀筛选（570-590）；先计算全部所需大小，不够则只置 BUFFER_FULL；空列表准备 dev=0 标记并配合输出长度。**当前 dm：**未置 UUID_FLAG 时省略 event/flags 扩展；无前缀筛选；空间不足返回部分列表；空列表不清理 payload，且 V5.A03 导致旧输入长度仍返回。**影响：**非 UUID 客户端读取错位或缺失事件、筛选结果包含无关设备，复用非零缓冲区可能看见旧记录。

**Fix.** **目标：**按本机 4.48 ABI 的真实布局统一列表预计算、筛选、全量/不足/空输出规则；保留显式 UUID 分支但不省略公共 event 字段。与 V5.A03 共用有效长度机制。**例子：**不置 UUID_FLAG 查询一个设备，name 后仍应有 event_nr/flags。**验证：**raw C 字节级对照有无 UUID、空列表、name/uuid 前缀、刚好容纳/少一字节、旧缓冲复用；不能仅靠本地编码器与本地解析器互证。

### `kernel/core/src/device/misc/device_mapper.rs` line 1054

> ```diff
> // The Linux ABI gives the UUID selector priority over name ...
> let device = if let Some(uuid) = uuid {
>     manager.lookup_uuid(&uuid)
> } else if let Some(name) = name {
>     manager.lookup_name(&name)
> ```

Ambiguous device selector accepted (major): **V5.A05｜已确认，待决定。Linux：**`validate_params:2040-2047` 对非 create 的 name+uuid 拒绝 EINVAL；`__find_device_hash_cell:925-954` 对 uuid+dev 或 name+dev 拒绝，外层返回 ENXIO。**当前 dm：**注释声称冗余标识是 Linux 合法输入，实际直接按 uuid>name>dev 选择。**差异/影响：**相互矛盾的设备标识仍可能导致删除/改名/换表作用于其中一个设备，而 Linux 拒绝这份请求。

**Fix.** **目标：**分别实现通用 validate_params 和设备查找约束，不把 create 的 name+uuid 与查询/变更命令混淆。**例子：**请求 name=A、dev=B 时当前选择 A，Linux 返回 ENXIO；不是提高权限，而是避免接受含糊操作。**验证：**以不同设备构造 name/uuid/dev 七类组合，检查 errno、未修改两台设备及 version-only 失败回写。修正误导注释与旧宽松 selector 测试。

### `kernel/core/src/device/misc/device_mapper.rs` line 1154

> ```diff
> write_u32(buffer, OFF_TARGET_COUNT, 0)?;
> write_u32(buffer, OFF_FLAGS, flags)?;
> ```

Common status loses selected target count (minor): **V5.A09｜已确认，待决定。Linux：**`__dev_status:855-878` 按所选表设置 target_count，供 load/clear/rename/suspend/deps 等共同使用。**当前 dm：**公共 `fill_device_header_snapshot` 永远设为零；只有 DEV_STATUS 与 TABLE_STATUS 独立补写，deps 和其他公共状态响应丢失计数。DEV_STATUS 自身已使用 table_metadata 和 table_snapshot(inactive)，其 target_count/readonly 不应误报。

**Fix.** **目标：**让公共响应 snapshot 携带所选表 target_count，依命令语义填充；不要对 tableless 人为增加数量。**例子：**已有两个 active target 时 TABLE_DEPS 返回 target_count 应为 2，当前为 0。**验证：**双 active target、另一个 inactive 数量、readonly 差异及 tableless，逐命令检查共同头与状态 flags。

### `kernel/core/src/device/misc/device_mapper.rs` line 1195

> ```diff
> if dev.starts_with('/') { /* resolve path */ }
> if let Some((major, minor)) = dev.split_once(':') { /* resolve id */ }
> return_errno_with_message!(Errno::EINVAL, "Device Mapper backing 必须采用 major:minor 或绝对设备路径");
> ```

Backing token resolution mismatch (minor): **V5.A12｜已确认，待决定。Linux：**`dm-table.c:339-364` 尝试 major:minor，否则交给 lookup_bdev 路径解析，不要求绝对路径；`dm_split_args:516-573` 支持反斜杠转义。**当前 dm：**parse_backing_device_id 只接受绝对路径或 major:minor；linear/striped 的参数解析使用 split_ascii_whitespace，无转义。**影响：**相对设备路径或带转义空格的合法块设备路径不能加载。VFS 已有路径解析入口，尚需核实所有非 UTF-8 字节路径支持，不能把全部差异归为框架不支持。

**Fix.** **目标：**迁移公共参数 tokenizer 的转义语义，并将非设备号 token 交给当前进程路径解析，保留块设备类型和 tracked lease 校验。**例子：**cwd=/dev 时使用 vda；或设备路径含空格并在参数中转义。**验证：**相对/绝对/major:minor、转义空格/反斜杠、失效路径、非块节点及非法数字；这是控制面表输入解析，不扩展到 target I/O 算法。

### `kernel/core/src/device/misc/device_mapper.rs` line 1318

> ```diff
> let name_in = optional_c_string(&header, OFF_NAME, DM_NAME_LEN, "name")?;
> // ...
> let Some(end) = bytes.iter().position(|byte| *byte == 0) else { /* EINVAL */ };
> let value = core::str::from_utf8(&bytes[..end])
> ```

Fixed string normalization mismatch (minor): **V5.A06｜已确认，待决定。Linux：**`validate_params:2050-2052` 将固定 name/uuid 的最后一个字节置 NUL，字符数组不要求 UTF-8；list-devices/list-versions/remove-all 在此之前返回，按命令忽略或按定长字段使用输入。**当前 dm：**入口为日志而解析 name，所有固定字段要求原始输入已有 NUL，非 UTF-8 也 EINVAL。**影响：**日志解析提前拒绝原本忽略 name 的命令；满长字段不按 Linux 截断。非 UTF-8 是当前 String/路径表达边界，不能声称仅改一行就全面兼容。

**Fix.** **目标：**首先把无关诊断从校验路径分离，并按命令归一固定字段；非 UTF-8 名称的存储/VFS 表达列为需决定的架构边界，不能无声替换或损坏字节。**例子：**REMOVE_ALL 的 name 填满无 NUL，Linux 忽略，当前入口即 EINVAL。**验证：**固定字段无 NUL、末字节截断、非 UTF-8、被忽略字段分别测试，区分已支持与明确拒绝边界。

### `kernel/core/src/device/misc/device_mapper/control.rs` line 351

> ```diff
> let removed = request
>     .devices
>     .into_iter()
>     .filter(|device| remove_current(device).is_ok())
>     .count();
> ```

Incomplete best-effort removal (minor): **V5.L08｜已确认，待决定。** REMOVE_ALL 只遍历一次事先捕获的设备快照，不能复现 Linux 每次成功删除后重新扫描的行为。即使当前禁止 DM-on-DM，也存在已支持的交错：第一次遇到 A 时仍有打开 fd，跳过 A；随后关闭 A 的 fd，并成功删除 B。当前 ioctl 仍保留已不忙的 A，或者漏掉在删除 B 期间新创建的 C。Linux `/root/Linux-tree-code/kernel/drivers/md/dm-ioctl.c:339-385` 每次成功销毁后 `goto retry`，明确说明新创建的设备也会被继续删除，直到不再取得进展。不能将 snapshot 单次 best-effort 宣称为与该基线等价。

**Fix.** 在保持每设备 lifecycle 锁、不跨 drain 持有全局 manager 锁的前提下，每轮有成功删除后重新获取 manager 快照，直到一轮没有进展。回归 busy A 在删除 B 期间关闭以及期间创建 C 的可控交错；不要依靠尚未支持的 DM-on-DM 才覆盖该行为。

### `kernel/core/src/device/misc/device_mapper/control.rs` line 370

> ```diff
> self.manager
>     .rename_uuid(&device.mapper_name(), uuid.clone())
>     .map_err(map_dm_error)
> ```

Mutable persistent identity (major): **V5.L03｜已确认，待决定。** `DM_UUID_FLAG` 分支允许覆盖已有 UUID，甚至把重复设置同一 UUID 当成功。实际 `/root/github/asterinas/kernel/core/comps/device-mapper/src/manager.rs:272-292` 会删除旧 UUID 索引并写入新值。对已建立 UUID 的 mapper 执行 setuuid 后，按旧 UUID 定位的客户端立即失去设备，与 Linux 的只允许首次赋值约束不同。Linux `/root/Linux-tree-code/kernel/drivers/md/dm-ioctl.c:451-488` 先以 `EBUSY` 拒绝已占用目标 UUID，再以 `EINVAL` 拒绝为已有 UUID 的设备设置任何新值；只有未设置 UUID 的设备到达 `491-492`。现有测试反而固定了覆盖行为。

**Fix.** 在同一 manager 临界区内按 Linux 顺序检查目标 UUID 是否已存在、源设备是否已经有 UUID，只有原 UUID 为空才提交新 UUID；失败不更新索引、不发布事件。修订覆盖既有 UUID 的测试，分别覆盖首次设置、重复同值、不同新值和重复他人 UUID。

### `kernel/core/src/device/misc/device_mapper/control.rs` line 398

> ```diff
> LifecycleRequest::Suspend { noflush } => {
>     (if noflush {
>         device.suspend_no_flush()
>     } else {
>         device.suspend()
>     })
> ```

Incomplete I/O quiescence (major): **V5.L01｜已确认，待决定。** `DM_NOFLUSH_FLAG` 被解释为不等待已分配的 BIO：本路径调用 `/root/github/asterinas/kernel/core/comps/device-mapper/src/device.rs:368-389` 的 `suspend_no_flush()`，仅改变 phase 就成功返回；随后 `resume()` 的 suspended 分支也直接换表。具体场景是旧表上仍有写 BIO 未完成，用户执行 noflush suspend 并收到成功后迁移数据、加载并恢复新表，旧写仍可能继续落到原 backing。保留旧表的 `Arc` 只能保证对象存活，不能提供 Linux 的静止点。Linux 基线 `/root/Linux-tree-code/kernel/drivers/md/dm.c:2685-2756` 只让 noflush 跳过文件系统冻结；`2753-2754` 对有 active map 的情况仍无条件调用 `dm_wait_for_completion()`，因此 noflush 成功也必须等待已下发 I/O 完成或回退。

**Fix.** 将 noflush 与是否 drain 分离：noflush 只控制 Linux 所规定的冻结及 target pushback 行为；在提交 Suspended 状态之前仍等待所有已分配 BIO 完成。同步修订把旧 BIO 跨 noflush resume 存活视为预期行为的测试，增加延迟 backing BIO 的静止点回归。

### `kernel/core/src/device/misc/device_mapper/control.rs` line 400

> ```diff
> } else {
>     device.suspend()
> })
> .map_err(map_dm_error)?;
> ```

Uninterruptible control operation (major): **V5.L02｜已确认，待决定。** suspend 以及运行中带新表的 resume 会在持有 mapper lifecycle 锁期间无限等待 BIO，而且信号无法取消：`/root/github/asterinas/kernel/core/comps/device-mapper/src/device.rs:176-180,354,446` 调用 OSTD `WaitQueue::wait_until()`，`/root/github/asterinas/ostd/src/sync/wait.rs:69-84` 的 cancel closure 永远返回成功。若 backing I/O 长时间不完成，发送信号不能让控制 ioctl 返回，且同设备 remove/clear 等也被 lifecycle 锁挡住。Linux `/root/Linux-tree-code/kernel/drivers/md/dm.c:2533-2555,2822` 使用 `TASK_INTERRUPTIBLE` 并返回 `-ERESTARTSYS`；`2763-2772` 撤销 suspend，`/root/Linux-tree-code/kernel/drivers/md/dm-ioctl.c:1195-1212` 在隐式 suspend 失败时恢复或释放取出的新表。当前 WAIT 已有 signal-aware 等待，不意味着 suspend 也具备该语义。

**Fix.** 给 drain 提供可中断等待接口，由 kernel/core 注入信号感知的等待策略，避免 DM component 直接依赖进程层。中断时恢复 Running/admission、重放延期 BIO，并对 resume 暂时取出的 inactive table 执行对应回滚；向 ioctl 返回 `ERESTARTSYS`。增加未完成 backing BIO 下发送信号的回归。

### `kernel/core/src/device/misc/device_mapper/control.rs` line 408

> ```diff
> if !runtime.is_registered(device) || runtime.is_alias_published(device)? {
>     device.resume().map_err(map_dm_error)?;
> ```

Non-idempotent resume (minor): **V5.L07｜已确认，待决定。** 对刚 create、从未 suspend、也没有任何 table 的 mapper 发起 resume 会返回 `EINVAL`：未注册设备走此处 `device.resume()`，其 `/root/github/asterinas/kernel/core/comps/device-mapper/src/device.rs:422-424` 无条件拒绝两个空表槽。Linux `/root/Linux-tree-code/kernel/drivers/md/dm-ioctl.c:1180-1187,1234-1252,1258-1261` 对没有新表且已非 suspended 的设备不调用 `dm_resume()`，而是成功返回现有状态。应区分“已经 Running 的空设备”与“真正 Suspended 且无表、无法恢复的设备”，后者失败不能作为前者失败的依据。

**Fix.** 保留 Linux ioctl 层的幂等条件：无 inactive table 且当前未 suspended 时直接返回状态；只有需要真正恢复时才验证 active table。增加 create→resume 成功与 create→suspend→resume 失败的对照回归。

## Security

### `kernel/core/src/device/misc/device_mapper.rs` line 318

> ```diff
> let secure_data = flags & DM_SECURE_DATA_FLAG != 0;
> let command_result = validate_input_flags(command, &buffer).and_then(|_| {
>     if command == DM_DEV_WAIT_CMD {
>         device_wait(&mut buffer)
>     } else {
>         handle_command(command, &mut buffer)
>     }
> });
> let write_result = current_userspace!().write_bytes(raw_ioctl.arg(), &buffer);
> ```

Sensitive input retained in userspace (major): **V5.S01｜已确认，待决定。** `DM_SECURE_DATA_FLAG` 没有清除用户态输入，而且这里无论命令成功与否都将整个输入大小的 `buffer` 写回。具体例子：特权客户端提交带该标志的 `DM_TABLE_LOAD`，参数区含敏感字符串；即使目标类型不受支持而返回 `EINVAL`，输入仍完整留在用户缓冲区。对成功的加载，`fill_device_header` 也只改写头部，因此原参数同样被写回。客户端依赖该标志而释放缓冲区后，敏感内容仍可留在用户堆或转储中。这是敏感数据清理承诺失效，不是权限绕过，也不以当前已支持加密 target 为前提。本机 Linux `5078a3a23` 的 `drivers/md/dm-ioctl.c:2012-2015` 在分派前执行 `clear_user`；`2110` 将输出长度重置为固定前缀，`2123` 只在成功时写回实际输出范围；`include/uapi/linux/dm-ioctl.h:352-356` 明确承诺清除敏感缓冲区。

**Fix.** 与 V5.A02/A03 共同设计输入、输出和清理生命周期，但保留本条安全验收。在成功捕获安全输入之后、命令分派之前清除对应用户输入范围，清除失败则不执行命令；按 Linux 区分输入长度与实际输出长度，只回写有效结果，不能重新写回未使用的原始参数。补充带敏感哨兵的成功加载、拒绝未知 target、校验失败及用户清除失败场景，核对返回后输入区不残留哨兵。

### `kernel/core/src/device/misc/device_mapper.rs` line 320

> ```diff
> let result = match command_result {
>     Ok(()) => {
>         write_result?;
>         Ok(0)
>     }
>     Err(error) => {
>         let _ = write_result;
>         Err(error)
>     }
> };
> if secure_data {
>     buffer.fill(0);
> }
> result
> ```

Sensitive kernel copies escape cleanup (major): **V5.S02｜已确认，待决定。** 安全输入的内核清理只在正常控制流尾部执行，至少有两条确定路径绕过它：命令成功后用户输出页不可写时，`write_result?` 直接退出闭包；跨不可读页的载荷在 `copy_ioctl_payload` 内部分复制后失败时，`read(...)?` 直接释放含已复制字节的 `Vec`，此时甚至还没建立 `secure_data` 清理状态。此外，`parse_table_load_request` 经 `c_string_until` 将参数复制成普通 `String`，该副本正常退出也不会擦除；例如提交带敏感参数但未知目标类型的安全加载请求，就会留下这份已释放的堆副本。单独清零外层 `buffer` 不能消除这些残留。Linux `5078a3a23` 的 `drivers/md/dm-ioctl.c:1980-1982` 在载荷复制前保存清理标记，`2020-2021` 与 `2126-2127` 分别在复制失败及所有分派后退出路径调用 `free_params`，`1951-1957` 在释放前清零。

**Fix.** 与 V5.A02/A03 共同设计输入、输出和清理生命周期，但保留本条安全验收。在开始复制载荷前建立按不可变头部标志配置的敏感缓冲区清理守卫，使部分复制失败、命令失败和复制输出失败均执行擦除；使用不会被死存储优化移除的安全清零原语。让 target 参数解析借用受保护缓冲区，或使所有临时参数副本也受同类清理保护，避免普通 `String` 提前释放敏感内容。用注入部分复制错误和输出错误的定向测试检查清理实际发生，并覆盖参数解析失败释放副本的路径。

## Documentation

### `kernel/core/src/device/misc/device_mapper.rs` line 396

> ```diff
> /// Decodes the Linux ioctl number and rejects command IDs outside this DM subset.
> fn decode_command(raw: u32) -> Result<u8> {
> ```

`linux-compat-docs` (major): **V5.D01｜已确认，待决定。** 公开的 DM ioctl 子集没有进入 Linux Compatibility 文档。`decode_command` 已接受命令 `0..=13` 和 `17`，但 `book/src/kernel/linux-compatibility/syscall-flag-coverage/file-descriptor-and-io-control/ioctl.scml` 完全没有 DM 命令，对应 `README.md` 的 `ioctl` 节也没有能力边界说明。用户无法据此区分已实现的控制命令与返回 `ENOTTY` 的 `DM_TARGET_MSG`、`DM_DEV_SET_GEOMETRY`、`DM_DEV_ARM_POLL`，也无法知道 target 仅有 `error`、`linear`、`striped`、`zero`，不能把报告的接口版本 `4.50.0` 当作完整 Linux DM/LVM 生态支持声明。

**Fix.** 在同一变更中补齐上述 SCML 与 `README.md`：列出实际接受的 DM 命令、`CAP_SYS_ADMIN` 要求、target 子集、`DM_DEFERRED_REMOVE`/`DM_IMA_MEASUREMENT_FLAG` 的拒绝行为和 DM-on-DM 限制，并明确包大小等 ABI 限制。设备管理边界应准确写为：已有 block 设备模型的 sysfs 目录和设备号链接；DM 专属属性及 DM uevent/cookie 发布未接通；`/dev/mapper/<name>` 当前由内核直接创建，不等价于用户态 udev 规则及同步链已实现。不要将这些差异统称为“没有 sysfs/udev 框架”。

### `kernel/core/src/device/misc/device_mapper.rs` line 966

> ```diff
> /// Encodes one immutable mapper snapshot sequence as Linux name-list records.
> ///
> /// When `include_uuid` is false (the caller did not set `DM_UUID_FLAG`), each
> /// record is the compact `dev | next | name\0` form aligned to 8 bytes. When
> /// `include_uuid` is true, the extended form with event number, name-list flags,
> /// and optional UUID is written instead.
> ```

Incorrect Linux record format description (minor): **V5.D03｜已确认，待决定。** 这里把无 `DM_UUID_FLAG` 时省略事件字段的 compact 记录称为 Linux name-list 格式，容易让读者误以为 UUID flag 控制整个扩展区。指定 Linux 基线的 `drivers/md/dm-ioctl.c:613-616,649-661` 无条件预留并写入 `event_nr` 和 flags 两个 `u32`；`DM_UUID_FLAG` 只决定 UUID 标志及字符串。具体输入是 `DM_LIST_DEVICES` 且 flags 为 `0`：当前编码确实只写 `dev | next | name`，但基线仍返回事件号和零 flags。注释准确描述了本地分支，却没有说明它与所宣称 Linux 格式的差异。

**Fix.** 与 V5.A07 共用行为与注释修复；明确把 compact 分支标为当前实现与指定 Linux 基线的 ABI 差异，并引用基线的无条件事件字段布局；不要描述为 Linux 的 UUID flag 语义。行为是否对齐及相应 ABI 回归应另行审定，最终文档应与实际选定的编码一致。

### `kernel/core/src/device/misc/device_mapper.rs` line 1054

> ```diff
> // The Linux ABI gives the UUID selector priority over name, and uses `dev`
> // only when neither is specified. Treating multiple selectors as malformed
> // would reject standard clients that populate redundant fields, so each
> // lookup uses only the highest-priority selector.
> ```

Incorrect Linux ABI explanation (major): **V5.D02｜已确认，待决定。** 这段注释把本实现的 selector 优先级误称为 Linux ABI，并宣称拒绝多 selector 会破坏标准客户端。指定基线 `/root/Linux-tree-code/kernel` 的 `HEAD 5078a3a23` 恰好相反：`drivers/md/dm-ioctl.c:2026-2047` 的 `validate_params` 对非 create 的 `name + uuid` 返回 `EINVAL`；`:925-954` 的 `__find_device_hash_cell` 明确拒绝 `uuid + dev` 或 `name + dev`。例如同时设置有效 `name` 和 `dev` 的 `DM_DEV_STATUS`，本代码选择名称，基线查找失败并返回 `ENXIO`。这不是框架依赖缺失，而是被注释描述成标准行为的语义偏离，会误导后续迁移和回归测试。

**Fix.** 与 V5.A05 共用行为与注释修复；将注释改为明确描述当前 Asterinas 的扩展选择规则及其相对指定 Linux 基线的差异，删除未经证实的标准客户端必要性声明，并标出基线调用链；是否改成 Linux 的互斥 selector 校验应作为独立语义修复交由用户审定，不用修改注释掩盖行为差异。

### `kernel/core/src/device/misc/device_mapper/runtime.rs` line 80

> ```diff
> /// Moves a published mapper alias and commits manager indexes atomically.
> ///
> /// The manager reserves `new_name` while the alias moves without holding its
> /// global lock. A failed move drops the reservation and preserves the old
> /// manager indexes, device name, and alias.
> ```

Overstated atomicity guarantee (minor): **V5.D04｜已确认，待决定。** `rename` 的文档把 alias 移动和 manager 索引提交整体称为原子操作，但 `rename_with` 在 `rename_alias(...)` 返回后才调用 `reservation.commit()`。`kernel/core/src/device/registry/block.rs:574-604` 已在前一步完成 devtmpfs rename，而 `kernel/core/comps/device-mapper/src/manager.rs:71-84` 随后才持有 manager 锁更新名称。若线程在两次调用之间被抢占，另一个线程能通过 VFS 打开新 alias，同时 `DM_LIST_DEVICES` 或名称查询仍看见旧 manager 名称；per-device lifecycle guard 并不覆盖这两类读者。当前实现提供名称预留及失败回滚，并不提供跨 VFS 与 manager 的统一原子可见性。

**Fix.** 将首句改为描述“先移动已发布 alias，再提交预留的 manager 名称”，并说明只保证冲突生命周期操作被串行化、失败时保留旧状态；不要承诺 VFS 与查询端跨层原子可见。除非另行获批实现相应同步，不应以文档承诺尚不存在的保证。
