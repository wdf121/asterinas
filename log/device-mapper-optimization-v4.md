# Device Mapper 第四版优化：分支新增行为的断言覆盖

> 状态：本轮授权范围内的补测与验证已完成；非 DM 基础设施项已移出，VFS/runtime 深层补偿与其他条件性专项未实施。
>
> 最后核对：2026-09-21。
>
> 审核基线：`main`（`604948581`）→ 当前工作区，包括 `dm` 分支已提交变更（当前 HEAD 为 `e31b265a3`）及未提交变更。不是只比较 HEAD 与工作树。
>
> 本文承接[第三版优化](device-mapper-optimization-v3.md)。当前保留 `V4.1`～`V4.4` 与 `V4.7`；原 `V4.5`（CPUID/TSC）和 `V4.6`（test-kernel selector）不属于 DM 功能验收，已从本方案移除。源码与实际命令结果优先于历史记录。

## 1. 执行摘要

第四版的目标是：**相对 main，为 DM 用户可见语义及其直接依赖新增或修改的代码，尽可能建立能够发现行为错误的明确断言。** 当前范围包括 DM 本体、直接依赖的 block/VFS 行为和用户 ABI；平台启动与通用测试基础设施不作为 DM 验收项。

这不是再把已有 ktest 跑一遍，也不是要求所有行为都使用 ktest。优化对象是新增行为的成功路径、拒绝路径、错误传播、边界条件、并发与资源生命周期。

| 维度 | 原来的判断方式 | 第四版目标 |
|---|---|---|
| 覆盖边界 | 选定的现有直接 ktest 全部通过，就把清单视为阶段完成条件。 | 从 `main → 当前工作区` 的行为差异出发，检查是否存在对应断言。 |
| 覆盖证据 | 容易把依赖编译、启动经过、正常工具执行当成相关分支已覆盖。 | 区分明确断言、间接触达、跳过、未执行及未找到断言。 |
| 生命周期 | 底层 lease/token 通过，容易推及全部消费者。 | 分别验证 table、未完成 BIO、文件系统、RawDisk 等消费者持有和释放真实 tracked lease。 |
| 测试层次 | 容易用 core 全量或系统 suite 替代缺失的定点测试。 | 内部契约用 ktest，raw 用户 ABI 用 C 回归，真实工具组合用系统测试。 |
| 阶段完成 | 以测试数量和退出码为主要依据。 | 每个优化项都有行为、断言、执行证据及明确的未覆盖边界。 |

**前期 193 项 ktest 基线仍保留为历史证据。** 本轮重新执行的 DM crate、block crate 与 core DM ioctl 共 191 项，均通过；另有 focused Device Mapper C regression 159 项断言通过。授权子项完成不代表整份覆盖地图已完成，真实 runtime 恢复、mount source 类型与条件性专项仍保持未实施。

### 1.1 分类与本轮证据边界

| 分类 | 含义 | 如何处理 |
|---|---|---|
| 已有断言且已验证 | 能指出实际断言，且有对应运行记录。 | 保留证据；相关实现变化后回归。 |
| 已有断言，待本阶段验证 | 用例存在，但不能用当前 ktest 结果证明其执行。 | 运行已有用例，不重复补写；历史通过记录与本阶段重验分开。 |
| 仅间接触达 | 编译、启动或正常流程经过代码，没有观察关键约束。 | 不能记作该行为已有直接覆盖。 |
| 未找到直接断言 | 在本轮核查范围内没有找到能够检验该行为的用例。 | 新增测试，或给出适合其他测试层的验证方式。 |
| 待确认 | 尚未证明分支可达性、故障构造方式或验证环境。 | 先补证据，不机械制造生产不可达状态。 |

本轮覆盖审核最初以 `main → 当前工作区` 为宽口径，因此识别到 DM 本体、block/VFS、驱动和测试基础设施中的相关差异。实施阶段按项目目标收敛为 **DM 用户可见语义及其直接依赖**：CPUID/TSC 和 test-kernel selector 虽是分支改动，但与 DM 功能没有直接调用关系，不纳入本方案验收。

## 2. 已验证基线与不能重复算作缺口的行为

### 2.1 当前验证结果

| 测试范围 | 最新实测结果 | 证据边界 |
|---|---:|---|
| DM crate 全量 | 86 passed，0 failed，0 filtered out | 包含旧 in-flight BIO lease 与 DmDevice backing 拒绝新增断言。 |
| block crate 全量 | 23 passed，0 failed，0 filtered out | 包含四个 range wrapper 的 3 个新增 ktest。 |
| core DM ioctl 模块 | 82 passed，0 failed，123 filtered out | 包含 table lease 替换与 failed table-load 状态保持新增断言。 |
| **本轮 Rust 合计** | **191 项，0 failed** | 三组均在本轮重新执行。 |
| core block registry 模块 | 7 passed，0 failed | 2026-09-21 前期已验证，本轮未重跑。 |
| core 动态设备路径模块 | 1 passed，0 failed | 2026-09-21 前期已验证，本轮未重跑。 |
| **最新已验证 ktest 清单** | **199 项，0 failed** | 跨轮次统计；不得表述为本轮 199 项全部重跑。 |

focused `device/device_mapper` C regression 另有 7 个测试函数、159 项断言通过，0 failed。该结果覆盖本轮新增的 WAIT/`SA_RESTART`、raw range ioctl，并复验原有 ext2 lease 等 Device Mapper 用户 ABI；它不是完整 initramfs regression 全量，也不是 159 个独立 C 测试函数。

前期 85＋20＋80＋7＋1＝193 项是实施前基线，应作为历史记录保留，不能继续当作当前新增断言后的统计。可执行命令以[开发测试手册](../docs/test.md#3-ktest内核内部逻辑)为准，具体阶段证据见[2026-9-20 日志](daily/2026-9-20.md)与[2026-9-21 日志](daily/2026-9-21.md)。

### 2.2 已有断言的准确边界

| 已有覆盖 | 代码依据 | 不应外推的结论 |
|---|---|---|
| DM discard 跨 linear、write-zeroes 跨 stripe 的类型/区间；child enqueue 失败后继续等待；readonly 拒绝写类 BIO。 | [table.rs](../kernel/core/comps/device-mapper/src/table.rs#L653-L794)、[striped range 用例](../kernel/core/comps/device-mapper/src/table.rs#L1461-L1491)、[readonly 用例](../kernel/core/comps/device-mapper/src/device.rs#L1227-L1280)。 | 不等于 block 便捷 API 或真实驱动请求编码已验证。 |
| manager reservation commit/drop，以及最后引用释放前 minor/major 不复用。 | [manager.rs](../kernel/core/comps/device-mapper/src/manager.rs#L394-L449)、[编号寿命用例](../kernel/core/comps/device-mapper/src/manager.rs#L524-L569)。 | 不等于所有消费者的 backing lease 释放时机已验证。 |
| lease clone 阻止注销、末个 holder 释放后放行，pending 状态与 token Drop 回滚。 | [block/lib.rs](../kernel/core/comps/block/src/lib.rs#L573-L671)。 | 不等于 DM table、RawDisk 或文件系统真实持有 tracked lease。 |
| core registry 的 pending/open gate、busy、隔离后重试，以及注入恢复成功/失败结果、primary 创建失败无注册残留。 | [registry/block.rs](../kernel/core/src/device/registry/block.rs#L922-L1085)。 | closure 注入的恢复结果不等于真实 `restore_mapper_alias()` 的 Path/inode 操作已验证。 |
| tracked table/BIO 生命周期与失败清理 | 既有部分失败、inactive clear 清理断言；本轮新增 inactive/active table 存活保护、替换关系、旧 in-flight BIO 持有旧 backing，以及非空状态下 failed table-load 保留与临时 lease 释放。 | 证明 DM table/BIO 对真实 tracked backing 的相应保护与清理；不等于 mount、RawDisk、MlsDisk facade 或真实 VFS 恢复均已覆盖。 |
| ext2 mount 后 remove 返回 EBUSY，umount 后 remove 成功。 | [device_mapper.c](../test/initramfs/src/regression/device/device_mapper.c#L265-L298)。 | 已有 C 用例应纳入下一层验证，不应误报为“ext2 lease 完全没测”；也不能代替 exfat 和失败挂载释放。 |
| `/proc/devices` 短读、标题顺序、设备名称唯一性及 major 范围。 | [devices.c](../test/initramfs/src/regression/fs/procfs/devices.c#L47-L88)。 | 这是现有 C 断言，不能拿 ktest 基线算作它已执行。 |
| mapper/backing 内容一致、LVM 扩容前后内容保持。 | [dataplane](../myshell/run_dm_dataplane_test.sh#L157-L223)、[linear integration](../myshell/dm_linear/run_lvm2_linear_integration_test.sh#L170-L219)。 | 正常数据流程不能代替 inode 替换、驱动拒绝或异常资源释放断言。 |

## 3. 优化项状态总览

| 编号 | 类别 | 优化项 | 当前状态 |
|---|---|---|---|
| V4.1 | 非 DM block/ABI 框架 | range 便捷 API 与用户态 range ioctl | **授权子项完成**：wrapper ktest 与 raw C ABI 已通过；不外推为真实硬件 discard 内容语义。 |
| V4.2 | 非 DM VFS 框架＋DM runtime 集成 | 身份匹配删除与真实节点补偿 | **未实施**：真实 alias 恢复成功/失败和深层 VFS 补偿仍待单独授权。 |
| V4.3 | DM 本体＋非 DM 消费者 | tracked lease 的真实消费者生命周期 | **授权核心子项完成**：table/BIO/failed-load 已验证；mount source 类型未实施，其他消费者为条件性候选。 |
| V4.4 | 非 DM 驱动适配 | VirtIO GET_ID/range 与 NVMe 拒绝分支 | **条件性候选**：本轮未实施，且不改变 GET_ID 故障策略。 |
| V4.7 | DM 本体 | 禁止 DM backing 的拒绝边界 | **直接拒绝完成**：实际 DmDevice backing 的 linear/striped 构造均得到 `UnsupportedBackingDevice`。 |
| 承接 V3 | DM 用户 ABI | `DM_DEV_WAIT / SA_RESTART` | **完成**：不可重启信号保持 `EINTR`；可重启信号 handler 执行后 ioctl 继续等待并在事件后成功返回。 |

原 V4.5（CPUID/TSC）与 V4.6（test-kernel selector）属于通用平台/测试基础设施，与 DM 功能没有直接调用关系，已从本方案移除；这表示范围决策，不表示两项专项测试已经通过。

## 4. V4.1：range 便捷 API 与用户态 range ioctl

### 4.1 实现与验证结果

[impl_block_device.rs](../kernel/core/comps/block/src/impl_block_device.rs) 的 3 个直接 ktest 已覆盖四个 wrapper：构造的 `BioType`、sector range 与空 data segments，同步/异步提交、共享 batch、callback 恰一次，以及 enqueue 拒绝与接受后 completion 错误的通道区分。block crate 全量结果为 23 passed、0 failed。

[device_mapper.c](../test/initramfs/src/regression/device/device_mapper.c) 使用无 backing 的 8-sector zero target，对 `BLKDISCARD` 和 `BLKZEROOUT` 检验合法范围、未对齐、`start + len` 溢出、容量越界，以及合法/非法零长度；focused C regression 已通过。该用例证明 raw 用户内存、ioctl command、byte range 与 errno 边界，不证明真实 backing 的硬件 discard、磁盘内容清零或所有 `BioStatus → errno` 组合。

### 4.2 已关闭边界与保留限制

| 子项 | 本轮证据 | 保留限制 |
|---|---|---|
| 四个 wrapper | 直接记录 BIO 类型、区间和 segment 数。 | 不是每个 API×所有状态的完整笛卡尔矩阵。 |
| 同步/异步完成 | 覆盖成功、enqueue 拒绝、completion `IoError`、batch 与 callback。 | 不修改公共 BIO/`IoBatch` 契约。 |
| 用户 ABI | 两个 ioctl 共用 14 组 byte range 矩阵。 | discard 不承诺读回为零。 |
| 零长度 | 合法 start 成功，未对齐或越界 start 返回 `EINVAL`。 | 本轮 C 用例不直接计数内部 BIO 下发次数；零长度不下发来自现有分派实现。 |

**选择与权衡：**mock 检验内部转发和完成，C 回归检验真实用户 ABI；二者分层组合，不用 zero target 的成功外推真实驱动请求编码。

## 5. V4.2：VFS 身份删除与真实 runtime 节点补偿

### 5.1 当前行为与缺口

[Path 包装](../kernel/core/src/fs/vfs/path/mod.rs#L693-L709)与[dentry 身份匹配删除](../kernel/core/src/fs/vfs/path/dentry.rs#L688-L759)服务于动态节点生命周期。[core/device](../kernel/core/src/device/mod.rs#L193-L306)和[block registry](../kernel/core/src/device/registry/block.rs#L536-L655)负责发布、删除与补偿。

目前路径 ktest 主要检查字符串是否合法；registry 的部分恢复用例用 closure 注入结果；已有 C 回归验证名称碰撞 EEXIST、状态及可重试。这些不能证明“旧对象已经被替换时，不会误删新对象”。

| 场景 | 目标断言 | 适合层次 |
|---|---|---|
| 正常删除与身份不符 | 匹配对象可删除；旧 inode 被替换后返回 ESTALE，并保留新对象及内容。 | VFS ktest。 |
| 跨挂载边界 | 在实际实现禁止的跨 mount 情形返回 EXDEV，目录项保持不变。 | 可控 mount fixture 的 ktest。 |
| 目录回滚 | 只清理本次创建且身份仍匹配的空目录；预存、替换、已变非空目录不被误删。 | VFS/runtime ktest。 |
| mapper remove 的外来节点 | alias/primary 被替换后，保留外来对象；自身 registry 按既有注销/隔离契约处理，不错误回到可用状态。 | C lifecycle 用例＋内部状态 ktest。 |
| primary 删除失败后真实恢复成功 | alias 已删除、primary 删除失败后，实际执行 `restore_mapper_alias()`；恢复链接目标和设备 identity 正确，registry 恢复 Live/open，本次 remove 保留原删除错误且后续可重试。 | 保留真实 VFS 恢复操作的 runtime ktest；先证明 primary 删除失败分支可达。 |
| 真实恢复失败 | 恢复时发生路径冲突或其他 VFS 失败时不覆盖外来对象，registry 保持 Removing 隔离，open/lookup 受限；清障后按既有契约重试。 | runtime ktest；如需注入，只注入 primary 删除结果，不能用 closure 直接替代真实恢复。 |
| 发布/rename 所有权丢失 | 拒绝覆盖不再属于当前设备的节点，manager、alias 与 primary 不形成不一致状态。 | runtime ktest＋C。 |

**实施约束：**先使用已有内存文件系统/路径 fixture；不足时仅补局部测试接口。并发场景以可观察的阶段屏障构造，不用一次 `yield_now` 或固定 sleep 宣称必然进入竞态窗口。必须验证失败后的目录项、registry、持有者和可重试性，不仅验证 errno。

**待确认：**[最终 commit 失败补偿](../kernel/core/src/device/registry/block.rs#L626-L655)的部分分支尚未证明在 token/锁约束下可达。先说明可达条件，再决定是否需要注入，不能制造生产不可能出现的状态后把它当真实验收。

## 6. V4.3：tracked lease 在实际消费者中的生命周期

### 6.1 已完成核心生命周期与剩余边界

底层 lease clone/drop 与注销阻塞已有直接断言。本轮进一步使用真实 registry backing 覆盖 table/BIO 生命周期，而不是用 `new_untracked` 外推真实注销保护。

| 消费者 / 场景 | 本轮状态 | 证据边界 |
|---|---|---|
| DM inactive/active table 保护与替换 | **完成**：table 存在时 backing 注销为 `Busy`；clear/替换后旧 backing 在无额外 holder 时可注销，新 backing 继续受保护。 | core DM ioctl 新增断言。 |
| DM 换表与旧 in-flight BIO | **完成**：旧 BIO 完成前旧 backing 仍为 `Busy`；最后 completion 后释放；新 backing 的保护独立保持，completion 恰一次。 | DM crate noflush/延迟完成 fixture。 |
| table 解析/构造失败 | **完成**：missing backing、容量、连续性三类错误均保持原 active/inactive table identity、readonly、phase 与 lease；临时 lease 释放，后续 clear/resume/reload 可继续。 | 复用生产 parser、`lookup_lease` 和状态 helper。 |
| mount source 类型 | **未实施**：非 BlockDevice 节点即使 devno 对应已有块设备也应拒绝挂载。 | 仍需单独 C/runtime 验证。 |
| ext2 | **复验通过**：原 mount→`EBUSY`→umount→remove 用例包含在 focused C regression。 | 不代替 exfat 或失败挂载释放。 |
| exfat、RawDisk/subset/clone | **条件性候选**。 | 仅在出现 DM 用户可见失败、数据风险、阻塞后续 target 或单独授权时重开。 |
| MlsDisk facade | **已知边界未解决**：facade drop 后立即释放 backing 并无保证。 | 本轮不改变生产所有权语义。 |

**选择与权衡：**table/BIO 核心路径使用真实 tracked backing 获得确定性证据；RawDisk 与 MlsDisk facade 继续分开，前者是可补 holder 测试，后者是尚未解决的生产所有权问题。

**验收约束：**测试自身不额外持有 tracked lease；并发/延迟完成使用显式状态，结束后清理 registry 与 pending BIO。

## 7. V4.4：VirtIO 协议分支与 NVMe 拒绝行为

### 7.1 当前行为与证据

- VirtIO [GET_ID](../kernel/core/comps/virtio/src/device/block/device.rs#L312-L385)及 [range 请求](../kernel/core/comps/virtio/src/device/block/device.rs#L584-L691)增加设备识别、能力判断、限额检查与请求编码；[配置读取](../kernel/core/comps/virtio/src/device/block/mod.rs#L219-L233)参与此链路。
- [locator](../distro/etc_nixos/overlays/hello-asterinas/default.nix#L97-L123)与[dataplane](../myshell/run_dm_dataplane_test.sh#L140-L149)已有正常 ID 内容、长度及不同设备识别断言。对 ENOTTY/ENODATA 的容错跳过不是负向行为已被断言。
- zero target 上的 discard 不进入 VirtIO range 驱动，不能算真实硬件 range 分支覆盖。
- NVMe [新增拒绝分支](../kernel/core/comps/nvme/src/device/block_device.rs#L137-L142)逐 BIO 完成 NotSupported；旧读写测试不检查此分支，无设备时返回也不能算运行成功覆盖。

### 7.2 已实现契约边界与条件性测试候选

GET_ID 必须按完成项所在层次区分，不能写成统一的“异常后继续初始化”契约：

| 异常层次 | 当前行为 | 本阶段处理 |
|---|---|---|
| query 层实际取得的 completion | 合法 token 且达到 queue 最小长度后，status 非成功或返回长度不等于 21 时不保存 host ID，查询返回 `None`。 | 可在重新开启驱动专项后补局部解析断言。 |
| queue 层过滤的异常 | 非法 token 或低于 queue 最小长度的 used entry 被过滤；没有后续合法完成项时，`query_host_id()` 继续轮询。 | 不得写成“返回 `None` 并继续初始化”；本轮不改变轮询语义。 |
| 设备不完成请求 | 同步 GET_ID 查询没有超时或取消路径，且发生在正常 queue IRQ callback 注册之前。 | 作为未界定的故障策略保留；若要增加超时或降级，另立生产设计事项。 |

| 子项 | 需要明确检验的内容 | 建议方案 |
|---|---|---|
| GET_ID 可返回响应 | 对实际返回给 query 层的 status/长度异常，不保存错误 ID；不把解析 mock 外推为 queue/DMA 已验证。 | 条件性局部响应解析测试。 |
| VirtIO range 拒绝 | 无 feature、未对齐、超过限额时不入队；合并请求中的每个 BIO 完成一次且状态正确。 | 条件性能力/范围检查与记录型队列测试。 |
| VirtIO range 编码 | header sector 为 0，range sector、num_sectors、flags 编码符合操作类型；边界长度不截断。 | 条件性编码结构/字节及限额边界断言。 |
| VirtIO completion | 成功和错误 status 传播正确；不丢 child、不重复完成。 | 条件性 mock completion＋必要的受控 QEMU fixture。 |
| NVMe 不支持操作 | discard/write-zeroes 均逐 BIO 完成 NotSupported，且没有硬件下发。 | 条件性最小请求分派测试接口。 |
| host-ID ioctl | 非 VirtIO 返回 ENOTTY，无 ID 返回 ENODATA，合法 ID 返回正确内容。 | 可控内部测试和适当 C 用例；无 ID 场景需真实可达 fixture。 |

**权衡与范围：**上述驱动项技术上仍属于覆盖地图，但当前按既有评审与交接边界保持为条件性候选，不进入无条件第三批。只有出现 DM 用户可见失败、数据风险、阻塞后续 target，或用户明确重新授权专项时才实施。纯编码/判定测试不能证明 DMA、真实 queue 和设备交互；本轮也不修改 GET_ID timeout、轮询退出或初始化降级策略。

若必须调整 QEMU feature/设备配置，先给出单独方案并获得授权；不修改全局 KVM、release、启动协议来绕过问题。

## 8. V4.7：DM backing 拒绝边界

[DmTable 构造](../kernel/core/comps/device-mapper/src/table.rs)继续明确拒绝 `DmDevice` 作为 backing，保持不支持 DM-on-DM stacking 的现有边界。

本轮新增直接 ktest：先为真实 inner `DmDevice` 加载合法 128-sector table，排除容量校验提前失败，再分别将其作为 linear 与 striped backing；两条路径均直接得到 `TableError::UnsupportedBackingDevice`。该用例已包含在 DM crate 86 passed、0 failed 的全量结果中。

该证据只关闭 `DmTable` 层的实际 backing 拒绝分支。它使用 untracked lease 构造目标，并非 ioctl façade 专用状态/资源回归；通用 failed table-load 的非空状态保持由 V4.3 的 core 测试独立证明。本轮不移除拒绝、不实现 stacking，也不建立嵌套 DM 框架。

## 9. 实施状态与阶段验收

### 9.1 已完成与剩余项

| 项目 | 当前状态 | 验证结果 / 退出条件 |
|---|---|---|
| V4.3 table 存活保护与替换 | **完成** | 真实 tracked backing 在 table 存活时 `Busy`，clear/替换后按 holder 生命周期释放。 |
| V4.3 旧 in-flight BIO | **完成** | 旧 completion 前保护旧 backing，最后 completion 后释放；新 backing 独立受保护。 |
| V4.3 failed table-load | **完成** | 原非空状态与 lease 不变，临时 lease 释放，后续操作可继续。 |
| V4.7 禁止 DmDevice backing | **完成** | linear/striped 均直接命中 `UnsupportedBackingDevice`。 |
| V4.1 block wrapper 与 raw range ABI | **完成** | block 23/0；focused C 中 range 矩阵通过。 |
| `DM_DEV_WAIT / SA_RESTART` | **完成** | handler 实际执行后 ioctl 未提前返回，rename 事件后成功完成；原 `EINTR` 对照保留。 |
| V4.3 ext2 lease 复验 | **完成** | focused C regression 复验通过。 |
| V4.3 mount source 类型 | **未实施** | 非 BlockDevice 节点边界仍待单独授权。 |
| V4.2 VFS identity/真实 alias 恢复 | **未实施** | 不把既有 closure 注入或路径碰撞回滚当作真实恢复证据。 |

以下项目是**条件性候选**，不因出现在覆盖地图中自动进入实施：

- exfat mount/失败/卸载 lease；
- RawDisk/subset/clone tracked lease；
- VirtIO GET_ID/range、NVMe range 的 mock 或受控集成；
- GET_ID timeout/轮询退出策略和 MlsDisk facade 释放语义。

重新开启条件为：出现 DM 用户可见失败、数据风险、阻塞新 target，或取得单独范围确认。GET_ID 故障策略和 MlsDisk facade 所有权属于生产设计事项，不能作为“只补测试”夹带修改。

### 9.2 统一完成条件

每个 V4 子项完成时，必须具备：

1. **行为依据**：指明相对 main 新增/修改了什么；期望来自源码契约还是 Linux ABI，规格不确定处先标注。
2. **直接断言**：给出输入、可观察状态、预期结果，以及测试能发现哪种错误；不能只断言函数返回成功。
3. **成功与失败边界**：按实际行为覆盖拒绝、完成错误、部分成功补偿、末个持有者释放及可重试性。
4. **同步与清理**：全局 registry、目录项、句柄、task、BIO 均有清理；并发用例使用阶段同步，不以调度概率替代证明。
5. **运行证据**：记录测试类型、目录、真实命令、选中用例、通过/失败数及日志；跳过、零用例、仅编译、未进入 guest 都单独标明。
6. **覆盖复核**：把新增断言回填到对应生产行为，说明仍未覆盖的部分；不因测试总数增加就默认缺口消失。
7. **文档更新**：只有代码与测试完成后才把命令加入手册；阶段日志只记录实际发生的变更和验证。

### 9.3 运行策略

- DM、block 改动后使用各自已验证的 crate 全量入口；模块入口用于定位，不在全量通过后机械重复。
- core 先运行受影响模块；只有新增行为或 fixture 影响范围扩大时再评估 core 全量。core 全量不是整个项目回归，也不执行依赖 crate 的测试。
- C 回归按 ABI/目录编排需求选择；procfs 等已有用例纳入覆盖矩阵，不因主要关注 DM ELF 而漏掉。
- 系统 suite 按真实工具工作流选择，不能用其成功替代驱动拒绝分支或内部生命周期断言。
- 默认 Cargo 离线；`CARGO_NET_OFFLINE=true` 不保证 Nix 等工具离线。缺缓存时报告，不自动联网。
- 构建与 QEMU/ktest/NixOS 串行，运行前检查资源；所有结果按当轮独立日志核对，避免根目录共享日志被后续运行覆盖。
- 本文不提供尚未实测的新 selector 或故障配置命令；新增入口先实测再写入[测试手册](../docs/test.md)。

## 10. 风险、假设与待确认事项

| 风险 / 未确认项 | 处理原则 |
|---|---|
| 用 mock 替代了真正需要验证的路径 | 明确 mock 边界；协议编码与 queue/DMA、内部 buffer 与真实用户 ABI 分层验证。 |
| 为测试抽接口时改变生产语义 | 优先复用现有接口；最小抽取单独说明前后行为，避免顺便重构。 |
| 全局状态污染或测试自己持有引用 | 为设备/目录分配独立身份，明确所有 holder；通过失败后可重试与最终清理断言。 |
| 并发用例存在假阳性 | 同步到实际生命周期阶段；检查阻塞前后状态，不只看到最终完成就宣称等待成立。 |
| 驱动 feature/硬件不可用 | 缺设备或 feature 的 skip 单独记录；不能记为目标分支通过。 |
| commit 失败补偿可达性未证明 | 先从 token、锁和状态约束证明可达输入，再决定故障注入。 |
| exfat、RawDisk、驱动 fixture 未具备 | 作为条件性候选保留；只有出现 DM 用户可见失败、数据风险、阻塞后续 target 或取得单独授权时重新开启。 |
| GET_ID 故障策略与 MlsDisk facade 所有权 | 当前实现边界与测试缺口分开记录；本轮不改变 GET_ID timeout/轮询语义，也不承诺 facade drop 后立即释放 backing。 |
| 旧记录与源码变化 | 本文行号为核对时定位；后续以符号和当前 diff 复核。main 或工作区变化后更新基线与断言映射。 |
| 首轮未穷尽所有行为 | 实施前逐项复核，实施后回查剩余 diff；不能以本清单项数声称绝对完整。 |

**当前最终状态：**文档契约与范围问题已修订；本轮授权的 tracked table/BIO 生命周期、failed table-load 状态保持、实际 DmDevice backing 拒绝、四个 block range wrapper、WAIT/`SA_RESTART` 用户 ABI 和 raw range ioctl 均已有直接断言并通过对应验证。最新本轮 Rust 结果为 DM 86/0、block 23/0、core DM ioctl 82/0；focused `device/device_mapper` C regression 为 159/0。真实 alias/VFS 深层补偿、mount source 类型仍未实施；exfat、RawDisk、VirtIO/NVMe 等保持条件性候选。原 V4.5/V4.6 已按范围决策移出，而非测试通过。
