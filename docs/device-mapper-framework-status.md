# DM 基础框架现状与 Linux 差距

日期：2026-10-22。

## 1. 总体结论

**当前 dm 已具备基本控制与 I/O 框架；主要缺口是 I/O 扩展接口、完整暂停语义、设备堆叠、块能力传播和设备事件链路。**

对照基线：

- 当前 dm：`6b8948e1ad026a61181f4c5b7fc3273500ec34aa`。
- Linux：本机 openEuler 内核树，`5078a3a23a1e1825ec136485173ec98668cdd640`；使用提交版源码，不采用工作树修改。

范围：BIO-based DM 的控制、表管理、I/O、生命周期、设备管理及相关依赖。
供 target 使用的公共接口统一计入这些能力，不单列，不评价具体 target 实现。
request-based DM、DAX、zoned、IMA 属于本轮范围外的专项能力，不计入下列基础框架排序。

## 2. 能力现状与差距

“已有”表示该项基本能力存在，不代表所有 flags、errno 和并发细节均已完成兼容性审计。
缺口编号与第 4 节排序一一对应；“无”表示该行列明的能力没有已确认的功能缺口。

### 2.1 控制与表管理

| 能力 | 当前 dm | 相比 Linux 的缺口 |
|---|---|---|
| 控制入口 | **已有**：权限检查、12 字节版本握手、用户参数处理、结果写回 | 无；ABI 版本号不同不代表能力超集 |
| 设备身份 | **已有**：name/UUID/dev_t、创建、查找、重命名、编号回收 | 无 |
| 表管理 | **已有**：多段表、范围校验、active/inactive、加载、切换、清除、只读模式 | 无 |
| 信息查询 | **已有**：设备列表、设备状态、表内容/状态、依赖、类型版本查询 | 无 |
| 类型注册 | **部分具备**：静态目录与构造分派 | **G13**：缺动态注册/注销、模块引用与按需装载 |
| 控制消息 | **缺失** | **G6**：缺 TARGET_MSG 与实例 message 分派接口 |
| 几何信息 | **缺失** | **G14**：缺 SET_GEOMETRY 及对应查询支持 |

### 2.2 I/O 与生命周期

| 能力 | 当前 dm | 相比 Linux 的缺口 |
|---|---|---|
| 基本 I/O | **已有**：映射、重映射、拆分、子请求派发、父请求完成聚合 | 无 |
| 请求与表寿命 | **已有**：接受/拒绝责任、in-flight 计数、旧表保活 | 无 |
| 操作感知 | **部分具备**：块层有操作类型，公共映射接口仅接收逻辑范围 | **G3**：缺传入操作上下文的映射接口 |
| Flush 分派 | **部分具备**：表层按 backing 去重后派发 | **G3**：缺可扩展的 Flush 声明与映射契约 |
| 接管与完成反馈 | **部分具备**：框架统一派发和完成 | **G7**：缺 I/O 接管、end_io、延后完成及重排接口 |
| 暂停与恢复 | **部分具备**：drain、暂停排队、恢复重放、换表 | **G4**：缺 presuspend/undo/postsuspend/preresume/resume hooks；NOFLUSH 语义不完整 |
| 文件系统冻结 | **缺失**：suspend 仅 drain | **G11**：缺 freeze/thaw；SKIP_LOCKFS 无对应冻结策略。依赖 VFS/文件系统 |
| 设备堆叠 | **缺失**：明确拒绝 DM backing | **G10**：缺 DM stacking 及相应依赖、持有和生命周期规则 |

### 2.3 块设备与资源管理

| 能力 | 当前 dm | 相比 Linux 的缺口 |
|---|---|---|
| 注册与普通删除 | **已有**：主设备注册、节点管理、open_count、busy 检查、注销及失败补偿 | 无 |
| backing/挂载持有 | **部分具备**：lease 防注销，文件系统持有 lease | **G9**：缺 Linux 式打开模式、claim 与 holder 关系契约。依赖块层 |
| 延迟删除 | **缺失**：DEFERRED_REMOVE 返回 EOPNOTSUPP | **G15**：缺最后关闭后触发的延迟删除机制 |
| 容量与 limits | **部分具备**：容量、最大 segment 数及表级合成 | **G8**：缺完整块大小、对齐、操作限制与 io_hints 合成。依赖块层 |
| 请求 flags | **部分具备**：有 Read/Write/Flush/Discard/WriteZeroes 操作类型 | **G8**：缺 FUA/PREFLUSH/NOWAIT 等请求语义的完整承接。依赖块层 |

### 2.4 事件与用户态设备管理

| 能力 | 当前 dm | 相比 Linux 的缺口 |
|---|---|---|
| 单设备事件 | **部分具备**：event_nr、WAIT、等待队列 | **G1**：缺供映射实例异步通知的公共事件接口 |
| 全局控制事件 | **缺失**：控制 fd poll 恒空 | **G5**：缺全局事件计数、每 fd 基准、poll 唤醒与 ARM_POLL |
| sysfs | **部分具备**：已有块设备 sysfs 基础视图 | **G2**：缺 DM name/uuid/suspended 等专用属性 |
| uevent/cookie | **部分具备**：已有 netlink uevent 传输基础 | **G12**：缺设备生命周期发布、DM_COOKIE 与事件结果反馈链路 |
| 节点与用户态同步 | **部分具备**：内核直接维护 mapper alias；udev 禁用，工具流程采用简化配置 | **G12**：缺标准 udev 规则与 cookie 完成同步闭环。依赖设备模型和用户态 |

其中 WAIT、全局 poll、uevent 是三种不同机制，不能互相替代。
udev 属于 Linux 用户态生态；本次确认了当前禁用及接线缺口，具体规则、IPC 和服务依赖仍需固定用户态版本核查。

## 3. 已有成果的验证佐证

以下是 **2026-10-19 的历史验证记录**，不是本次重新执行的结果。

| 验证 | 已有结果 | 证明范围 |
|---|---|---|
| `make check` | 独立执行通过 | 仓库静态检查与工程规范 |
| 定向内核测试 | BIO 13、队列 5、DM device 20、DM table 36、控制 ioctl 24；共 **98 passed、0 failed** | 已覆盖的 BIO、表、生命周期和控制行为；不是完整 `make ktest` |
| `make kernel` | 用户反馈通过；未独立核验日志 | 当时配置下的内核构建 |
| guest C ABI 回归 | 用户反馈通过；未独立核验日志 | 当时的控制、生命周期及表操作用户态接口 |

`make format` 是格式化操作，不是功能验证；不能把它与测试通过混为一谈。
本次未运行完整 `make ktest`，也不补写没有核验的工具集成结果。

历史测试对应 `93159d132` 加当时工作区改动，后续代码提交为 `7740a5281`、`64adad1c0`、`981294ef5`；从这些代码提交到当前 HEAD 的后续提交为文档变更。
证据见 [2026-10-19 日志](../log/daily/2026-10-19.md)；原始定向日志目录为 `/tmp/dm-ktest-20261019.u8QPBZ/`，临时文件不保证长期保存。
这些结果支持已有实现，不证明上述缺失能力已完成，也不等于完整 Linux 兼容性认证。

## 4. 缺口优先顺序

**先按完整改动量从小到大；同档再按框架重要性从高到低。**
改动量包括接线、错误处理和测试，属于估算；不是仅计算新增接口的代码量。
小＝局部接线，中＝DM 内多模块协同，大＝I/O/拓扑或公共块层改动，很大＝跨子系统闭环。
重要性：核心正确性 ＞ 通用扩展 ＞ 观察/生态 ＞ 辅助兼容；同级顺序是初步建议。

| 顺序 | 缺口 | 改动量 | 重要性 | 主要依赖/边界 |
|---|---|---|---|---|
| 1 | **G1** 公共异步事件接口 | 小 | 通用扩展 | 复用 event_nr/WAIT；处理表寿命 |
| 2 | **G2** DM sysfs 属性 | 小 | 观察/生态 | 复用设备模型；验证属性和对象寿命 |
| 3 | **G3** 操作感知与 Flush 契约 | 中 | 核心正确性 | 调整映射接口和表级分派；不包含完整请求 flags |
| 4 | **G4** 生命周期 hooks 与暂停 flags | 中 | 核心正确性 | 接入阶段顺序和失败撤销；完整 NOFLUSH 还依赖 G7 |
| 5 | **G5** 全局 poll/ARM_POLL | 中 | 通用扩展 | 全局等待队列、每 fd 状态和事件接线 |
| 6 | **G6** TARGET_MSG/message 路由 | 中 | 通用扩展 | 控制 ABI 与实例分派；不实现具体业务消息 |
| 7 | **G15** 延迟删除 | 中 | 通用扩展 | 最后关闭通知、删除调度、并发和 busy 检查 |
| 8 | **G14** 几何信息设置/查询 | 中 | 辅助兼容 | 控制命令、块设备查询路径；须联动实现 |
| 9 | **G7** I/O 接管、end_io 与重排 | 大 | 核心正确性 | BIO 所有权、完成聚合、in-flight 和暂停队列 |
| 10 | **G8** 块能力、limits、请求 flags | 大 | 核心正确性 | 公共块接口、调用者适配及表级合成 |
| 11 | **G9** 打开模式、claim/holder | 大 | 核心正确性 | 公共块层；保留已有 lease 防注销能力 |
| 12 | **G10** DM stacking | 大 | 通用扩展 | 合法拓扑、持有、移除与暂停顺序；所用能力须满足 G8/G9 |
| 13 | **G11** freeze/thaw | 很大 | 核心正确性 | VFS、文件系统写入路径、挂载关联及 G4 |
| 14 | **G13** 动态注册与模块装载 | 很大 | 通用扩展 | 模块装载/卸载及引用保护；不是换一个动态容器即可完成 |
| 15 | **G12** uevent/cookie/udev 闭环 | 很大 | 观察/生态 | 设备模型发布、DM 事件、用户态规则与同步；具体依赖待核实 |

G13/G14/G15 同样是对照差距，不能因暂不实施而从清单消失；是否纳入近期开发由需求决定。
大改动排后不代表不重要，例如 freeze/thaw 缺失仍是当前暂停能力的明确边界。
所有接口改动可用测试替身验证，不以新增具体 target 为验收前提。

## 附：源码核对入口

仅用于复核，正文不依赖读者阅读源码。
Linux 路径相对 `/root/Linux-tree-code/kernel`，应读取上述固定提交。

| 对照内容 | 当前 dm | Linux |
|---|---|---|
| 控制与身份 | [dm_control](../kernel/core/src/device/misc/dm_control/)、[manager.rs](../kernel/core/comps/device-mapper/src/manager.rs) | `drivers/md/dm-ioctl.c`、`dm-target.c` |
| I/O、表与生命周期 | [device.rs](../kernel/core/comps/device-mapper/src/device.rs)、[table.rs](../kernel/core/comps/device-mapper/src/table.rs)、[公共接口](../kernel/core/comps/device-mapper/src/target/mod.rs) | `include/linux/device-mapper.h`、`drivers/md/dm.c`、`dm-table.c` |
| 块管理与文件系统 | [block](../kernel/core/comps/block/src/)、[块注册](../kernel/core/src/device/registry/block.rs)、[FileSystem](../kernel/core/src/fs/vfs/fs_apis/file_system.rs) | `drivers/md/dm.c` 的设备持有和 freeze/thaw、`dm-table.c` 的 limits 合成 |
| 事件和设备模型 | [控制 fd](../kernel/core/src/device/misc/dm_control.rs)、[uevent](../kernel/core/src/net/socket/netlink/kobject_uevent/)、[udev 配置](../distro/etc_nixos/modules/systemd.nix) | `drivers/md/dm-ioctl.c` 的 WAIT/poll、`dm.c` 的 cookie 事件、`dm-sysfs.c` |
