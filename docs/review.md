# 无表设备管理提取边界分析（历史 PR3，现 PR4）

> **2026-10-27 收尾更新：PR4 已验证，待拆分。** 完整 dm 语义修复已在 `7de7025ad`、`74ee6b89e` 提交。命名 major 与工具发现前置独立为枚举 PR3 `be3f79c88`；无表管理已提取为 PR4 `6995e5ab3`，助手 format/check 通过，用户最终确认包含 NixOS 的测试全部通过，已推送。
>
> **当前权威台账**：[global.md §10.4](global.md#104-pr3-制备前修复与待办总账2026-10-27)，保留稳定 F/T 编号并更新提取、验证状态；过程见 [今日日志](../log/daily/2026-10-27.md)。当前卡点是审阅体量与拆分边界，不是尚未提取或测试失败。
>
> **语义与范围**：UUID 已按 bytes，minor 排空后一次性释放，1 GiB 是 ABI 校验上限不是容量保证。全局 poll/ARM_POLL 属于完整 dm，PR4 仍不支持全局事件；不得把完整 dm 的能力套到无表子集。deferred REMOVE、uevent/cookie、非 UTF-8 名称及极限/精确事件证据仍独立暂缓。
>
> 下方旧 PR3 指现在的无表 PR4。保留原始问题与调查事实，不批量改写为“当时已经完成”。新的 PR 编号见 [路线](pull_request.md)，本次不决定 PR4 拆分方案或回补代码。
>
> **以下为 2026-10-26 原始只读分析。** “尚未实施”“待审定”“本轮未执行”等表述仅指当时；旧源码行号可能随后续修改移动，以当前符号和上方台账为准。

### 1. 阶段目标与固定快照

| 项目 | 本轮核实结果 |
|---|---|
| 完整 dm | `7c46b2ee8cfb1c3dc3db7cc32ecd799bff15adcb`；实际检出分支为 `dm` |
| PR2 `dm-control-interface` | `808d36daf07344c27291adccb74e8e59671871d3` |
| PR3 `dm-device-management` | 分支已存在，同为 `808d36daf07344c27291adccb74e8e59671871d3`，尚无独有提交 |
| Linux 主基线 | 本机 `/root/Linux-tree-code/kernel`，openEuler `6.6.0-132.0.0`，commit `5078a3a23a1e1825ec136485173ec98668cdd640` |
| Linux 来源 | `https://gitee.com/openeuler/kernel.git`；Makefile 版本为 6.6.0，不称为未经修改的 kernel.org v6.6 |
| Linux 工作树 | `drivers/md/dm.c`、`dm-core.h`、`dm-linear.c` 有本地修改，另有未跟踪构建文件；生命周期与宏定义证据读取 Git HEAD 中的版本，不把本地修改当上游事实 |

| 行为 | 本阶段边界 |
|---|---|
| 原来 | PR2 仅提供控制节点和 VERSION；其他命令在版本握手后返回 `ENOTTY` |
| 目标 | 无表 DM 设备的创建、身份查找、基本状态查询、列表及单设备删除 |
| 示例 | 创建名称为 `test` 的设备 → 分别按 name/UUID/dev 查询 → 列表可见 → 删除；不能装表或读写映射设备 |
| 包含 | CREATE、STATUS、LIST_DEVICES、单个无表设备 REMOVE，以及必要的参数处理与 manager 接线 |
| 不包含 | target、映射表、BIO、运行时块设备发布、rename、批量删除；不自动扩展 deferred remove 或全局事件机制 |

`dm` 是唯一生产实现来源。若现有实现无法安全裁剪，先报告依赖；不为缩小 PR 单独创造设备模型，不只在 PR3 修复完整 dm 的语义问题。不修改 PR1/PR2，不自动提交或推送。

### 2. 无表阶段与可观察状态矩阵

**Linux 源码事实：CREATE 不执行 `add_disk()`。**

```text
CREATE
  → check_name
  → dm_create / alloc_dev：分配 minor 和内部设备对象
  → dm_hash_insert：发布 name / UUID 身份索引
  → 返回无表状态

后续首次装表
  → dm_setup_md_queue
  → add_disk
```

固定基线证据：`drivers/md/dm-ioctl.c:882–919`、`drivers/md/dm.c:2038–2152`、`drivers/md/dm.c:2330–2369`，以及 `dm-ioctl.c` 中 TABLE_LOAD 对 `dm_setup_md_queue` 的调用。

因此，不提前发布运行时块设备的阶段可以独立成立，不是为缩小补丁虚构的模型。但“内核未发布磁盘”不等于“用户工具绝不创建节点路径”；用户空间节点编排本轮未调查，不从内部对象状态推断。

| 操作 | 身份与状态 | 节点、open、I/O | 锁、失败和回滚边界 |
|---|---|---|---|
| CREATE | 分配设备号，登记 name/可选 UUID；无 active/inactive table，初始 event/target/open count 为 0；READONLY 不成为无表身份属性 | PR3 不发布 primary/alias，不提供映射 I/O | minor 分配先于重复身份检查；身份插入失败释放临时 minor |
| STATUS | 按 name、UUID 或 dev 查找，返回身份与无表状态 | 不改变注册表或节点 | name+UUID 为 `EINVAL`；文本标识+dev、缺失/不存在标识为 `ENXIO`；成功 lookup 的引用保护对象生命周期 |
| LIST_DEVICES | 名称有序列表、name/UUID 前缀过滤、event 字段及可选 UUID 扩展 | 不发布节点 | 容量按整个过滤后列表判断，不足设置 `DM_BUFFER_FULL_FLAG`，不输出部分列表；空列表仍要求 sentinel 空间 |
| REMOVE | name/UUID 索引删除后，新 lookup 不可见 | 无表且未发布的设备没有 runtime 注销工作；不能据此宣称已支持打开设备的删除保护 | lifecycle 串行化并复核实例，防止旧 lookup 删除同名新对象；旧引用存活期间不得复用 minor |

#### 参数处理与结果写回顺序

Linux `ctl_ioctl/copy_params/validate_params` 顺序为：权限 → ioctl type → 版本读写及兼容检查 → 命令查找 → 固定参数复制和大小检查 → 必要 payload 复制/secure 清理 → 参数规范化 → 命令执行 → 成功时结果写回。

- CREATE/STATUS/REMOVE 为 NO_PARAMS：输入固定前缀为 305 字节，不应机械要求整个 312 字节对齐结构都可读，也不应把 TABLE_LOAD 的 data_start 校验引入这些命令。
- 未支持命令应继续在版本握手之后、参数处理之前返回 `ENOTTY`。
- **命令执行成功后的 copyout `EFAULT` 不回滚已完成的 CREATE/REMOVE。** 创建已提交但结果页不可写时，设备仍存在；删除同理。不得将这种情况与身份插入失败混为同一种事务回滚。
- Linux 成功 CREATE/REMOVE 还触发全局控制事件；当前 dm/PR2 没有完整全局 poll/event 机制。本阶段不将此能力伪装为已实现，控制文件继续不能报告虚假就绪。

对应 dm 参数入口见 [ioctl.rs](../kernel/core/src/device/misc/dm_control/ioctl.rs)，Linux 证据为 `dm-ioctl.c:1960–2128`。

### 3. 已确认差异与待审定决策

以下结论均来自双方源码；本轮未运行外部 VM 实验或内核测试。差异成立不等于已获准修复。

#### 3.1 CREATE 重复名称/UUID：EBUSY 与 EEXIST

- **Linux**：`dm_hash_insert` 在名称或 UUID 冲突时清理 cell 并返回 `-EBUSY`，见 `dm-ioctl.c:262–305`。
- **dm**：`map_create_error` 将身份冲突交给 `map_dm_error`，返回 `EEXIST`，见 [abi.rs:412–435](../kernel/core/src/device/misc/dm_control/abi.rs#L412-L435)。现有 Rust/C 测试也固定了 `EEXIST` 预期。
- **示例**：已有名称 `test`，不指定 persistent minor 再次 CREATE `test`；Linux 为 `EBUSY`，dm 为 `EEXIST`。
- **分类/影响**：实际用户可见 ABI 错误码差异，阻止将当前代码称为无差异提取；不是内部表示差异。
- **建议**：先在 dm 修正 CREATE 专用错误映射和直接测试，再提取。不要为了 CREATE 顺手改变所有 `map_dm_error` 调用方。
- **验证要求**：重复 name、重复 UUID；与已占用 persistent minor 同时出现时检查错误优先级；失败后验证索引不变、临时 minor 可重新分配。

minor 分配优先于身份冲突这一点两侧基本一致。Linux `specific_minor` 的内部错误经过 `alloc_dev → dm_create` 对外成为 `ENXIO`，不能只看内部 `EBUSY` 就修改对外 errno；dm 已采用该顺序与映射。

#### 3.2 参数大小：本地 1 MiB 策略与 Linux 1 GiB 上限

- **Linux**：固定基线 `DM_MAX_TARGETS=1048576`、`DM_MAX_TARGET_PARAMS=1024`，`copy_params` 上限为 1 GiB。
- **dm**：[validate_ioctl_size](../kernel/core/src/device/misc/dm_control/abi.rs#L80-L86) 限制为 1 MiB，注释明确这是本地资源政策。
- **示例**：有效 CREATE 固定头声明 `data_size=2 MiB`；Linux NO_PARAMS 路径可继续只使用固定前缀，dm 在此之前返回 `EINVAL`。
- **分类/影响**：显式兼容范围与资源策略差异；不只影响 LIST 的实际大缓冲区分配。
- **待审定**：保留并披露当前上限，还是调整包括 NO_PARAMS 在内的校验策略。不能未讨论就把实际可分配缓冲区提高到 1 GiB。
- **验证要求**：304/305/311/312 字节、上限及上限加一；分别覆盖 header-only 和 LIST 路径，以及 payload/secure 清理失败优先级。

#### 3.3 身份字符串：字节串与 UTF-8

- **Linux**：CREATE/STATUS/REMOVE 的固定字符串末尾被强制补 NUL，然后按 C 字节串处理，不做 UTF-8 校验。名称禁止 `/`、`control`、`.`、`..`，CREATE 还要求非空。
- **dm**：同样先补末尾 NUL，但 [optional_c_string](../kernel/core/src/device/misc/dm_control/abi.rs#L301-L320) 额外要求 UTF-8；manager 索引使用 `String`。
- **示例**：名称包含非 UTF-8 的 `0xff`、且无其他非法字符，Linux 可以接受，dm 返回 `EINVAL`。
- **分类/影响**：身份输入域差异，不是仅缺一个校验用例。改为字节串会涉及 manager 存储以及后续节点路径接口。
- **待审定**：先在完整 dm 调整表示，或明确保留 UTF-8 支持边界；不能只在 PR3 换一种身份模型。
- **验证要求**：空名称、保留名称、斜杠、127/128 字节边界、UUID 128/129 字节边界、字段无 NUL、非 UTF-8；LIST 的过滤仍应按有界字节前缀而不是 UTF-8 字符串处理。

#### 3.4 dev 编码：高 32 位行为不同

- **Linux**：`include/linux/kdev_t.h:39–60` 的 `huge_decode_dev(u64)` 调用 `new_decode_dev(u32)`，高 32 位被截断。CREATE persistent 模式只取解码后的 minor，不用输入 major 决定 DM major。
- **dm**：CREATE 和 dev lookup 使用通用 [DeviceId::from_encoded_u64](../kernel/libs/device-id/src/lib.rs#L62-L85)，按扩展 64 位设备号格式解码。
- **示例**：有效设备编码 OR `(1ULL << 32)`，Linux 按低 32 位查找；dm 解出超范围 minor 后返回 `EINVAL`。persistent CREATE 也受影响。
- **分类/影响**：ABI 解码和错误顺序差异；正常低 32 位编码的设备号输出没有因此被判为错误。
- **建议**：若批准对齐，在完整 dm 的 ioctl ABI 边界实现 Linux 解码语义；不为 DM 改变全局设备号编码契约，再提取到 PR3。
- **验证要求**：高 32 位扰动、不同输入 major、minor 边界、零 dev、与 name/UUID 组合的歧义优先级。

**基线旁支问题，未修改：** 通用解码器把 major 转为 `u16` 后才做范围检查，可能截掉超范围 major 高位；该实现已存在于 PR2 基线，不是 DM 新引入。全局修复的必要性和影响须单独审查，不将其混入 PR3 或静默当作 dm 专属缺陷修复。

#### 3.5 REMOVE 完成时机与引用释放

- **Linux**：`dev_remove → dm_destroy → __dm_destroy` 等待内部 holder 归零，再释放设备及 minor，见已提交 `dm.c:2459–2518`、`dm.c:2157–2170`。
- **dm**：从索引中移除后可以返回；minor 由 `DmDeviceIdOwner` 在最后一个设备引用销毁时归还，见 [manager.rs:27–50](../kernel/core/comps/device-mapper/src/manager.rs#L27-L50) 和 [workflow.rs:346–365](../kernel/core/src/device/misc/dm_control/workflow.rs#L346-L365)。
- **示例**：并发 STATUS/LIST 保有旧对象引用时，dm 的 REMOVE 已返回，紧接着指定同一 minor 的 CREATE 仍可能得到 `ENXIO`；Linux 的删除完成会等待旧 holder。
- **分类/影响**：潜在用户可观察的并发生命周期差异；双方都防止旧引用期间复用 minor，但等待边界不同，不能仅归为 Rust RAII 的内部差异。
- **证据边界**：已确认源码控制流，尚未运行可控并发复现，不把该情景写为实测失败。
- **待审定**：保留并披露返回时机差异，或先在完整 dm 处理同步语义。不能通过提前释放 minor 消除报错，那会破坏旧对象的设备号所有权。
- **验证要求**：控制旧查询引用存活，覆盖 remove、同名重建、指定 minor 重建和引用释放后的复用；证明旧 lookup 不会修改新对象。

#### 3.6 DEFERRED_REMOVE 与事件支持边界

- dm 的 [validate_input_flags](../kernel/core/src/device/misc/dm_control/abi.rs#L197-L208) 对 REMOVE 的 `DM_DEFERRED_REMOVE` 返回 `EOPNOTSUPP`。
- Linux 的 `dm_lock_for_deletion` 对未打开的无表设备即使带该 flag 也可立即完成删除；忙设备才涉及延迟删除状态。
- 本阶段已明确不实现完整 deferred remove，需如实说明该 flag 的限制，不称为所有 REMOVE 参数都兼容。全局事件和 uevent 输出同样不能凭普通身份删除成功推定已经支持。

### 4. 提取依赖与安全裁剪边界

#### 4.1 可沿用的身份管理机制

[manager.rs](../kernel/core/comps/device-mapper/src/manager.rs) 已有以下机制，适合构成本阶段基础：

- 动态 major、minor 分配及 `DmDeviceIdOwner` 所有权；
- name/UUID 索引与 dev lookup；
- 分配 minor 后检查身份冲突，失败由 owner 释放；
- 返回独立 `Arc` 的查询，manager 列表快照；
- `is_current()` 按对象实例而非名称复核；
- 删除身份索引，但旧对象存活期间继续保留设备号。

**不能裁掉删除路径中的 `lookup → lifecycle lock → is_current → remove`。** 若退化为 `lookup(name)` 后无实例复核地 `remove(name)`，旧引用可能删除并发创建的同名新对象。现有包装见 [commands.rs:735–809](../kernel/core/src/device/misc/dm_control/commands.rs#L735-L809)。

锁关系：manager 查询在持有内部锁时克隆 `Arc`，随后释放 manager 锁；生命周期操作再取得设备锁并复核当前实例。不能持有全局 manager 锁等待设备生命周期锁，不能仅凭 `Arc` 保活就认定对象仍在当前索引中。

#### 4.2 不能直接整文件搬入的三处耦合

| 耦合点 | 当前源码事实 | PR3 提取约束 |
|---|---|---|
| DmDevice 与 table/BIO | [device.rs:144–170](../kernel/core/comps/device-mapper/src/device.rs#L144-L170) 包含表状态、BIO 队列、in-flight、事件队列，并实现 `BlockDevice` | 不能整文件搬入，也不能伪造空 table/BIO 实现；需明确身份和无表状态如何从现有对象裁剪，不另造平行模型 |
| STATUS/REMOVE 与 workflow | STATUS 经过 table snapshot；REMOVE 经过 runtime 注销、排队 BIO 清理及 inactive table 清理 | 无表不变量下这些工作不参与，但必须保留身份、状态、lookup 引用、生命周期串行化与实例复核语义 |
| manager 与公共 major API | 完整 dm 使用 `allocate_major_with_name("device-mapper")`，PR3 基线只有 `allocate_major()` | 真正的前置缺口；需审定前移 dm 已有命名 major 支持，或明确批准更窄接线；不得假定 API 已存在或静默删掉名称语义 |

完整 dm 的无表 REMOVE 中，runtime 未注册时 `unregister_if_registered` 返回成功；无排队 BIO、无 inactive/active table 时相关清理没有实际工作。但这种运行时事实不等于编译依赖自然消失，仍须设计明确的裁剪清单。

“不发布映射块设备”不等于“不需要设备号分配基础设施”。反过来，也不能因 major 分配依赖就提前搬入完整 block registry、lease、节点发布和注销状态机。命名 major 的 API 前置与 `/proc/devices` 用户态可见性须分清；本轮尚未完成 procfs 接线调查，不声称引入 API 就已完成工具发现支持。

### 5. 审定顺序与后续验证计划

**结论：无表管理阶段成立，但尚不能宣称当前代码可无差异提取。** 写代码前建议按以下顺序审定：

1. CREATE 冲突 errno：是否先在 dm 对齐 `EBUSY`，同步修正相关 Rust/C 断言。
2. dev 解码：是否先在 dm ABI 边界匹配 Linux 低 32 位语义。
3. 1 MiB 上限、UTF-8、deferred remove、全局事件：明确保留的兼容限制与先行补齐项目。
4. REMOVE 的完成与引用释放：决定是否先处理等待语义，并补可控并发证据。
5. 命名 major 前置及 DmDevice 身份部分的裁剪：先列逐文件依赖，不在 PR 分支另造架构。
6. 审定后先在 dm 修正并验证必要生产行为，再在已创建的 PR3 分支提取；保持 PR1/PR2 不变。

建议测试覆盖如下，**均为计划，不是通过记录**：

| 层次 | 覆盖 |
|---|---|
| manager / device ktest | name/UUID/dev 查询、重复身份、minor 优先级与失败释放、索引删除、旧引用保号、同名新对象保护、无表状态 |
| core 参数与命令 ktest | 固定前缀复制、NO_PARAMS、secure 清理、错误优先级、字符串和编码边界、LIST 过滤/对齐/UUID 扩展/容量、copyout 失败后的真实状态 |
| 官方 initramfs C ABI | CREATE → 三种身份 STATUS → LIST → REMOVE；重复与非法参数；不引入 target/table 或依赖映射 I/O 的场景 |
| 用户态可见边界 | 核对没有提前发布运行时块设备；若使用真实工具，另查工具的 major 发现、节点编排及前置命令，不把原始 ioctl 成功夸大为 dmsetup 全流程通过 |

最终验证在目标 PR 源码树执行：先 `make format` 并检查范围外修改，再 `make check`、`make kernel`、相关 crate/文件测试与官方 C ABI 回归。测试串行，记录源码树、命令、非零计数、退出码和日志；dm 的结果不替代 PR3 验收。现有全量回归 cgroup 基线阻塞单独披露，不借本轮重新扩大排查。

### 6. 本轮记录边界

- 已执行：核实分支/提交与 Linux 来源；读取上述双方源码及必要的基线差异；`git diff --check` 通过。
- 未执行：生产代码修改、缺口修复、外部 VM 实验、构建、ktest、C ABI 或系统测试。
- 未执行：切分支、stash/恢复六份文档、提交、推送或修改远端 PR。
- 本节是在用户授权下记录分析；结论中的建议和验证矩阵不构成后续实施授权。
