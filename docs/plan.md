# Asterinas Device Mapper 学习计划

> 本文描述整体学习路线与协作方法，不记录某一周的具体任务。每周范围、完成条件和未完成项统一记录在 `log/weekly/`；每天实际工程改动与验证统一记录在 `log/daily/`。

## 1. 目标与边界

目标不是泛泛地“学习内核”，也不是原样迁移 Linux Device Mapper；而是在当前 Asterinas 实现基础上，逐步建立以下能力：

```text
理解 dmsetup/LVM2 的用户可见语义
→ 能先打通命令的业务逻辑与主代码路径
→ 能用结构化内核日志、strace 和必要时的 GDB 建立运行证据
→ 能补全资源、并发、失败与边界条件
→ 能区分预期、事实、异常与根因
→ 能在明确语义、并发和验证边界后维护或扩展 DM
```

当前 Asterinas DM 是 Linux DM 核心 ABI 与数据面语义的兼容子集。学习应以当前代码和实际 guest 命令结果为事实来源；上游 Linux 是用户可见语义的基线，但不要求逐行模仿其内部实现。

本计划不要求先完整学完操作系统、Rust 或 Linux DM 才开始读项目代码。抽象知识应由当前命令和实际问题驱动补齐。

## 2. 总方法：先主逻辑，后补全正确性边界

每个代表性 `dmsetup` 命令或用户可见操作都按三层推进。第一层没有打通前，不提前把学习重心转为锁、回滚或 GDB 单步；第三层也不能因主链“看起来能跑”而省略。

```text
第一层：业务逻辑
用户要什么、前置状态是什么
→ 成功、失败时用户应看到什么
→ 哪些状态或资源应发生变化

第二层：代码主逻辑
命令/操作
→ 核心 ioctl 或接口
→ handler / workflow / 核心状态转换
→ 数据来源、ABI 编解码、结果回写
→ userspace 如何消费结果

第三层：正确性边界
资源由谁拥有、何时创建/发布/回收
→ 哪些访问者可并发发生
→ 锁粒度、锁顺序与数据竞争/死锁风险
→ 提交、失败回滚、容量不足、非法输入和状态边界
→ 正常、失败与并发场景的验证证据
```

先建立正确预期，再判断异常。不能在不了解命令契约时直接做 bug 定位题；否则只是猜测代码，而非调试。

运行证据按成本从低到高升级：先观察当前命令的 `[dm-debug]` `ostd::error!` 结构化日志；日志不足以解释用户态边界时再采集 `strace`；只有仍无法确定 ABI 原始内容、实际内核分支、局部状态或并发关系时才使用远程 GDB。

每次记录只写已经证实的事实，并明确证据来源：

| 证据类型 | 能证明什么 | 不能替代什么 |
|---|---|---|
| CLI 输出/后续命令 | 用户可见结果与状态变化 | 内核实际执行的函数。 |
| `[dm-debug]` `ostd::error!` | 已埋点的控制命令、ABI 摘要与真实状态提交 | 未埋点字段、原始 userspace buffer 或完整调用栈。 |
| `strace` | 进程访问的对象、syscall、ioctl、errno、用户态输出顺序 | 用户态库或内核内部为何这样实现。 |
| GDB | 实际内核调用栈、分支、局部字段和用户 buffer 读写 | 高频性能结论或全局统计。 |
| 源码 | 可能路径、设计约束、资源/锁/错误边界 | 当前运行是否实际走到该路径。 |
| 定向 ktest/系统测试 | 已编码的内核语义或端到端路径 | 未覆盖场景的正确性。 |

## 3. 核心能力地图

### 3.1 控制面与用户 ABI

优先掌握 `dmsetup` 命令如何经 `/dev/mapper/control` 进入内核：

```text
dmsetup / libdevmapper
→ dm_ioctl buffer
→ syscall ioctl
→ VFS / 文件对象分派
→ DmControlFile::ioctl
→ 命令解码、header 校验、handler、用户 buffer 回写
```

代表主题包括版本与 target 查询、设备选择器、create/load/resume/suspend/remove、table/status/deps、event/wait、errno 和可变长 ABI records。

### 3.2 Asterinas Rust 与内核框架

以当前代码实际出现的概念为入口，而非脱离项目背诵语言特性：

```text
Result 与 errno 映射
Arc、trait object 与所有权
锁、guard 与并发生命周期
用户地址空间读写
VFS 文件操作与 block registry
```

`kernel/` 保持 safe Rust，`unsafe` 被限制在 `ostd/`。需要理解 unsafe 契约和边界，但当前 DM 主线不以练习手写裸指针或 `unsafe impl` 为目标。

### 3.3 数据面与块 I/O

控制面建立后，沿真实 I/O 请求理解：

```text
/dev/dm-N 或 /dev/mapper/<name>
→ BlockDevice
→ SubmittedBio
→ DmTable
→ target 选择与 sector 映射
→ backing BIO 或 direct completion
→ split / completion 聚合
```

按复杂度学习 `error`、`zero`、`linear`、`striped` 与 mixed table，重点是用户可见 I/O 结果、边界拆分、flush、readonly 与错误传播，而不是一次吞下所有 Linux target。

### 3.4 生命周期、资源与并发

资源与并发不是脱离命令主链的附加话题。每个控制面变更在完成业务逻辑和代码主逻辑后，都要补全其第三层正确性边界；跨命令的复杂竞态再集中深入学习：

```text
资源所有权：谁创建、持有、发布与回收 mapper、table、runtime node、alias、lease
状态提交：active / inactive table、phase、manager index 与用户可见节点何时一起生效
并发访问：哪些 ioctl、I/O、waiter 或 registry 操作可能同时发生
同步设计：lifecycle guard、其他锁的粒度、获取顺序与持锁等待边界
异常处理：失败回滚、隔离、重试、容量不足、非法输入和用户可见一致性
验证：正常路径、失败注入/errno、并发或等待场景的最小证据
```

分析时必须明确共享资源、串行边界、锁顺序、潜在数据竞争/死锁、失败路径及用户可见影响；不能只因单线程 golden path 成功就宣布命令闭环。

### 3.5 Linux 语义对齐与扩展

当当前命令与实现已能稳定解释后，再使用上游 Linux 源码、最小 Linux 基线实验和 Asterinas 当前代码做对照。新增 target 或框架能力前，先定义：

```text
原有行为 → 目标行为 → 用户可见差异 → 最小验证
```

复杂 target、完整 udev/systemd 自动联动、完整 sysfs、DM-on-DM 和完整 queue stacking 不应混入日常学习或小阶段优化。

## 4. 调试工具的分工与升级顺序

| 问题 | 优先工具 | 使用边界 |
|---|---|---|
| 当前 DM 控制命令进入了什么 handler、header/专属 ABI 摘要与状态提交如何关联 | `[dm-debug]` `ostd::error!` | 默认首选；仅记录低频控制面与生命周期事实，不向 BIO 等热路径扩展。 |
| 用户态发出了什么请求、control FD 与 stdout/errno 的时序 | `strace` | 日志无法说明 userspace/内核边界，或需要确认 libdevmapper 的多次 ioctl 时使用。 |
| syscall 在内核中实际命中何处、具体分支、原始 ABI buffer 或并发状态为何如此 | QEMU + GDB | 结构化日志与 `strace` 仍不足以证实关键事实时使用。 |
| 内核代码可能如何运行，以及资源、锁、错误设计约束 | 源码走读 | 与运行证据配合，不能代替运行证据。 |
| 单测锁定局部语义 | 定向 ktest | 用仓库 wrapper，在对应 crate 目录运行。 |
| 真实 dmsetup/LVM2 路径 | 显式 NixOS system suite | 只在当前改动或阶段要求相关覆盖时运行。 |

GDB 不是每条命令的必经步骤。路径短且结构化日志已经能与 CLI 结果闭环时，不为“使用工具”而进入 GDB；详细操作记录见 [GDB 学习笔记](../todo/gdb.md)。

## 5. 节奏与验收

一次只处理一个具体命令或一个清晰的用户可见问题，并按三层完成：

1. 学习者先说明用户目标、前置状态、成功/失败的用户可见预期；
2. 用 CLI 与 `[dm-debug]` 日志打通业务逻辑，必要时再用 `strace`、GDB 升级证据；
3. 从命令反推核心 handler、数据流、ABI、状态转换、回写与 userspace 消费；
4. 补全资源生命周期、并发访问者、锁粒度/顺序、竞争/死锁、回滚和边界条件；
5. 以正常、失败及相关并发场景的最小证据验证结论；
6. 只在关键分叉、易误判处提问，不提前给出完整答案；
7. 确认闭环后再更新学习文档；生产代码改动、测试、日志和提交均另行确认，不借学习任务扩大范围。

整体路线不是固定课表。满足下列条件后，才自然进入下一个主题：

| 当前主题 | 可离开的完成条件 |
|---|---|
| 控制面命令 | 能从用户语义解释核心 ioctl、内核入口、ABI/状态变化和用户可见结果；已检查相关资源、失败及并发边界。 |
| 数据面命令 | 能解释 I/O 如何到达 target、如何映射/完成，以及资源持有、边界失败和并发可见性。 |
| 生命周期问题 | 能写出状态变化、资源所有权、锁与并发边界、回滚证据及用户可见影响。 |
| 发现异常 | 已建立正确基线，能复现并独立缩小至一段实际路径；未把日志、源码推断或历史结果混作本次事实。 |

## 6. 文档组织

```text
docs/plan.md
→ 整体学习目标、方法和能力路线。

log/weekly/YYYY-Www.md
→ 某一周的范围、练习顺序、验收与边界。

log/daily/YYYY-M-D.md
→ 当天实际工程改动和验证；不记录纯讨论或未来计划。

todo/strace.md、todo/gdb.md
→ 可复用的工具知识、命令观察模板与已确认的学习结论。
```
