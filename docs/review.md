# Device Mapper 第三版优化代码评审

> 评审日期：2026-09-15
>
> 评审对象：`log/device-mapper-optimization-v3.md` 所述 V3.1–V3.5，以及当前工作区中对应的生产代码和测试脚本改动
>
> 评审方式：静态代码复核并对照当前工作区基线；本轮未修改生产代码，未重复运行内核或系统测试

## 1. 评审结论

第三版优化的架构拆分整体合理，文档记录的主要设计边界与当前实现基本一致。

本轮未发现由第三版改动引入的 P0/P1 正确性、并发、生命周期事务或 ABI 阻断问题；确认一项 P2 资源与性能回退。该问题不会否定第三版优化，也没有证据表明已经导致 OOM、超时或用户可见故障，但建议在第三版最终收尾前修正。

| 严重程度 | 数量 | 结论 |
|---|---:|---|
| P0 | 0 | 未发现 |
| P1 | 0 | 未发现 |
| P2 | 1 | 表查询无条件捕获不需要的完整快照 |

## 2. P2：表查询无条件构造完整快照

**位置：**

- `kernel/core/src/device/misc/device_mapper/control.rs:165-196`
- `kernel/core/src/device/misc/device_mapper.rs:480-489`
- `kernel/core/src/device/misc/device_mapper.rs:710-743`
- `kernel/core/comps/device-mapper/src/table.rs:127-138`

### 2.1 问题描述

`QueryWorkflow::table()` 对所有表查询统一执行以下工作：

1. 遍历全部 target；
2. 为每个 target 构造 `TargetSnapshot`；
3. 分配 `Vec<TargetSnapshot>`；
4. 调用 `table.backing_ids()`，再次遍历 target 并去重 backing ID。

但是不同 ioctl 实际需要的数据并不相同，这使部分查询承担了与输出无关的遍历和分配。

### 2.2 `DM_DEV_STATUS` 从 O(1) 退化为 O(T)

`device_status_for_device()` 最终只使用 `snapshot.targets.len()` 写入 target 数量。

改动前直接调用 `table.target_count()`，即读取 `Vec::len()`：

- 时间复杂度为 O(1)；
- 不分配 target snapshot；
- 不遍历 backing device。

当前实现对包含 T 个 target 的 mapper 至少执行 O(T) 的 target 遍历和额外分配，并收集该 ioctl 完全不会使用的 `backing_ids`。

此外，`DmTable::backing_ids()` 使用 `Vec::contains()` 进行去重；当 target 引用较多不同 backing device 时，去重成本最坏可能接近 O(T²)。频繁执行 `dmsetup info` 或由监控程序轮询 mapper 状态时，这部分成本会被重复触发。

### 2.3 `DM_TABLE_STATUS` 丧失短缓冲区提前停止能力

当前顺序为：

1. snapshot 阶段为所有 target 调用 `status_params()` 并生成 `String`；
2. 构造完整 target snapshot；
3. ABI 编码阶段才逐条检查输出缓冲区容量。

改动前会逐条生成参数并立即检查当前 record 是否能写入；首个无法容纳的 record 会设置 `DM_BUFFER_FULL_FLAG` 并返回。

当前实现即使输出缓冲区无法容纳第一条 record，也会先格式化所有 target 的参数。这不会改变 ioctl 输出语义，因此正确性测试仍可通过，但属于确定可触发的资源开销回退。

### 2.4 影响边界

当前证据能够证明：

- 合法的多 target mapper 可以触发额外遍历和分配；
- `DM_DEV_STATUS` 相较基线由 O(1) 退化为至少 O(T)；
- 短缓冲区 `DM_TABLE_STATUS` 不再具备参数格式化的提前停止能力。

当前没有实测延迟、超时、分配失败或 OOM 证据，因此不将其描述为严重可用性故障，也不提升为 P1。

### 2.5 建议

保留 query snapshot 的职责边界，但让请求显式表达捕获需求，例如拆分为：

- count snapshot：只读取 `target_count()`；
- deps snapshot：只收集 `backing_ids()`；
- records snapshot：捕获 target range、名称和状态参数。

对于 `DM_TABLE_STATUS`，建议采用逐条捕获和编码，或者根据剩余 ABI 缓冲区限制需要格式化的 target 数量，以恢复短缓冲区查询的提前终止能力。

## 3. 明确排除的候选

### 3.1 正常 ioctl 使用 `error!` 不列为当前缺陷

**位置：** `kernel/core/src/device/misc/device_mapper.rs:212-284`

第三版文档的 V3.1.1 明确把这些日志定义为默认 `LOG_LEVEL=error` 下可见的低频控制面学习日志，并记录了数据面静默边界和 guest 验证结果。

因此，本轮不能脱离当前阶段的明确设计选择，将正常 ioctl 使用 `error!` 单独认定为日志级别错误。

长期进入面向生产的默认配置前，可以重新评估日志级别、编译期开关或 feature gate；这属于后续产品化决策，不是当前第三版实现缺陷。

### 3.2 `DM_DEV_WAIT` 不归因于第三版改动

生产 dispatch 中的 `DM_DEV_WAIT` 问题早于第三版，V3 文档也明确将其记录为独立阻断。本轮不将其计入第三版评审发现，也不以 non-wait suite 代替 wait 语义验证。

## 4. 未发现新增缺陷的重点边界

本轮重点复核了以下内容，未发现需要报告的第三版新增缺陷：

- runtime primary、alias、rename 和 unregister 的事务顺序；
- lifecycle guard 与 current-device 复核边界；
- initial resume 失败时 inactive table 的保留；
- remove 注销失败时 event、postponed BIO 和 manager index 的保持；
- remove-all 的 snapshot 与 best-effort 语义；
- static target catalogue 的名称、版本、顺序和 parser 绑定；
- normal I/O plan/execute 拆分后的 table/backing 生命周期；
- split child completion 与 enqueue failure 聚合；
- Linux ioctl buffer、flags、errno 和 record 编码边界。

## 5. 验证边界

- 本轮执行了静态代码评审，并将关键查询路径与当前工作区基线实现进行对照。
- 第三版相关改动通过 `git diff --check`，未发现 whitespace error。
- 本轮未重复运行文档中已经记录的 ktest、NixOS guest 或 system suite，因此不新增测试通过声明。
- 文档已有的测试结果是本轮风险判断的重要参考，但不被表述为本轮重新执行的结果。
- 本轮未修改生产代码和测试代码。

## 6. 最终判定

第三版优化在功能正确性、事务边界和生命周期安全性方面通过本轮复审，未发现阻断其成立的严重问题。

建议修复本报告中的一项 P2 查询性能回退后，再完成第三版最终收尾。
