# AI 时代的 Asterinas Device Mapper 项目接管与成长计划

> 本文不是功能开发排期，也不是按命令排列的知识清单，而是一份以“将项目成果转化为个人能力”为目标的长期行动计划。
>
> 当前 Device Mapper 项目的大量架构分析、代码实现、测试构造和文档整理曾由 AI 主导。因此，仓库的完成度不能直接等同于学习者的能力。后续工作的首要目标不是继续快速扩展功能，而是逐步取得对现有项目的**理解权、判断权、审查权和最终责任**。
>
> 每周具体任务和验收记录在 `log/weekly/`；每日实际工程改动与验证记录在 `log/daily/`；每日 Device Mapper 学习结论统一记录在 `docs/study2.md`。当前代码和实际运行结果始终高于本文档中的历史描述。

## 1. 执行摘要

### 1.1 当前阶段的客观判断

当前项目已经积累了较多功能、代码、测试和文档，但个人能力仍需要通过独立输出重新验证：

```text
项目中已经存在复杂实现
≠ 能够独立设计这些实现

听懂 AI 整理后的调用链
≠ 能够从陌生源码中找到调用链

认可状态机和并发方案
≠ 能够独立发现遗漏并设计方案

测试已经通过
≠ 能够判断测试是否足以证明实现正确

使用过 strace / GDB
≠ 能够面对未知问题独立选择工具并定位根因
```

因此，后续不再单纯以“新增多少功能、写了多少代码、通过多少测试”衡量成长，而以以下结果为准：

- 有多少调用链由学习者独立定位；
- 有多少业务语义、状态转换和失败边界由学习者先行定义；
- 有多少 AI 生成的代码经过实质性 review；
- 有多少测试由学习者设计，并能够证伪错误实现；
- 有多少未知问题由学习者独立提出假设、收集证据并缩小范围；
- 有多少结论能够脱离现成文档复述、应用和迁移。

### 1.2 总体优先级

| 优先级 | 主题 | 阶段目标 |
|---|---|---|
| **P0** | 接管现有 DM 项目 | 建立项目地图，能够定位主链、解释状态并审查关键代码。 |
| **P1** | 完成一次独立工程闭环 | 独立完成现象、需求、设计、验证和验收，AI 只承担受约束的辅助工作。 |
| **P2** | Rust 审查能力 | 掌握判断 AI 生成代码是否正确所必需的所有权、并发、错误和抽象机制。 |
| **P3** | 测试与调试推理 | 主导测试矩阵、竞争假设和证据收集，不直接向 AI 索取根因。 |
| **P4** | 内核知识体系 | 沿当前 DM 链路补齐 ioctl、设备模型、块 I/O、VFS 和并发基础。 |
| **P5** | 深度工程能力 | 进入性能、故障注入、压力测试、兼容性和长期维护。 |

短期严格按 `P0 → P1 → P2/P3 → P4 → P5` 推进。P2、P3 可以在 P0、P1 过程中穿插，但不能用泛读 Rust 或内核资料替代现有项目接管。

### 1.3 核心原则

```text
AI 可以代替部分编码劳动，
但不能代替定义正确性、审查关键实现和承担最终责任。
```

后续协作采用以下默认顺序：

```text
学习者定义用户问题和正确行为
→ 学习者定位现有入口并写出初步调用链
→ 学习者建立状态、资源、并发与失败模型
→ 学习者设计测试矩阵和验收条件
→ AI challenge 方案、补充遗漏
→ AI 在明确约束下填充代码
→ 学习者 review 关键 diff
→ 学习者运行验证并解释证据
→ AI 做第二轮审查和纠偏
```

如果学习者尚不能解释“正确意味着什么”，则不应直接让 AI 扩展实现。

---

## 2. 最终目标：从项目使用者转变为项目负责人

目标不是要求学习者手写每一行代码，而是逐步具备以下五项能力。

### 2.1 定义问题

能够把模糊需求拆成：

- 用户意图；
- 前置状态；
- 正常结果；
- 失败结果；
- 状态和资源变化；
- 兼容性边界；
- 明确的非目标；
- 可验证的验收标准。

### 2.2 建立系统模型

能够独立画出并解释：

- 控制面调用链；
- 领域对象和所有权；
- mapper/table 生命周期；
- runtime primary/alias 可见性；
- BIO 数据路径；
- 锁、并发访问者和串行边界；
- 跨资源事务和失败提交点。

### 2.3 约束 AI 实现

给 AI 的任务不应只是“实现某功能”，而应至少包含：

```text
原有行为
目标行为
示例差异
业务不变量
允许修改的层次
禁止扩大范围的部分
状态转换
资源提交顺序
并发约束
失败策略
测试矩阵
验收条件
```

### 2.4 审查与证伪

能够判断 AI 输出是否存在：

- 语义偏差；
- 错误的层次归属；
- stale object；
- 锁范围或锁顺序问题；
- 资源泄漏；
- 半提交状态；
- 错误回滚；
- errno 映射错误；
- 弱测试或遗漏场景；
- 只在单线程 golden path 下成立的实现。

### 2.5 对结果负责

面对失败时，不依赖“AI 说实现正确”，而能够回答：

1. 当前已经证明了什么？
2. 使用的证据是什么？
3. 还有哪些假设没有被排除？
4. 哪些场景尚未覆盖？
5. 出现生产问题时应从哪一层开始定位？

---

## 3. P0：接管现有 Device Mapper 项目

P0 是当前最高优先级。在 P0 达标之前，原则上不以扩展 snapshot、mirror 等复杂 target 作为学习主线。

### 3.1 建立六张项目地图

这些地图应由学习者先自行整理。AI 可以在完成后 review，但不应直接给出第一版完整答案。

#### 地图一：用户能力地图

覆盖当前主要用户操作：

```text
dmsetup version
→ targets
→ create
→ info
→ load
→ first resume
→ suspend
→ ordinary resume / table replacement
→ table / deps / status
→ clear
→ rename
→ remove / remove_all
→ wait
→ LVM2 驱动的组合路径
```

每项至少记录：

| 字段 | 要回答的问题 |
|---|---|
| 用户意图 | 用户希望系统完成什么？ |
| 前置状态 | mapper、table、runtime node 应处于什么状态？ |
| 成功结果 | CLI、节点、状态和 I/O 有什么可见变化？ |
| 失败结果 | 典型 errno 和残留状态是什么？ |
| 当前实现 | 已实现、部分实现、当前偏差还是待验证？ |
| 证据级别 | 源码推断、CLI、日志、strace、GDB 或测试？ |

#### 地图二：控制面调用地图

对每个代表性命令，独立定位：

```text
dmsetup / libdevmapper
→ ioctl command
→ dm_ioctl decode 与 header 校验
→ command handler
→ typed workflow
→ DmManager / DmDevice / DmTable / target
→ response 编码和 copy-back
→ userspace 可见结果
```

不要求背诵所有辅助函数，但必须能够在当前代码中重新找到核心入口，并知道每一层负责什么、不负责什么。

#### 地图三：对象和所有权地图

至少覆盖：

| 对象/资源 | 需要说明的内容 |
|---|---|
| `DmManager` | 索引、minor 分配、当前实例身份和删除关系。 |
| `DmDevice` | 创建者、`Arc` 持有者、内部状态、生命周期锁和旧引用。 |
| `DmTable` | 构造、active/inactive slot、BIO generation 和释放时机。 |
| target object | spec 如何解析、谁持有、如何参与 map/status/deps。 |
| backing lease | 谁申请、何时保持、table 被释放后如何回收。 |
| primary | `/dev/dm-N` 何时发布、谁注册、何时撤销。 |
| alias | `/dev/mapper/<name>` 何时发布、rename/remove 如何处理。 |
| postponed BIO | 为什么进入队列、谁持有、resume/remove 如何结束。 |
| in-flight BIO | 由谁计数、suspend/replacement 为什么等待 drain。 |
| event/waiter | event 如何变化、waiter 何时唤醒、删除如何处理。 |

对每个资源固定回答：

```text
谁创建？
谁拥有？
谁能并发访问？
何时发布给外部？
何时撤销外部入口？
最后由谁回收？
失败时保留、回滚还是隔离？
```

#### 地图四：状态机地图

必须区分以下概念，禁止用一个“设备是否存在”概括全部状态：

- manager identity 是否存在；
- primary 是否注册；
- alias 是否发布；
- active table 是否存在；
- inactive table 是否存在；
- phase 是 `Running`、`Suspending` 还是 `Suspended`；
- readonly 状态；
- open count；
- event number；
- in-flight BIO；
- postponed BIO；
- runtime 是否处于 `Pending`、`Live` 或 `Removing` 类状态。

代表性生命周期：

```text
tableless identity
→ inactive table loaded + primary visible
→ first activation + alias visible
→ running
→ suspending
→ suspended
→ original-table resume 或 replacement resume
→ clear / rename
→ runtime revoke
→ manager detach
→ final resource reclamation
```

每个箭头都要说明触发命令、锁、状态提交点和失败结果。

#### 地图五：锁与并发地图

至少回答：

- per-device lifecycle lock 串行哪些操作？
- device state lock 保护哪些字段和不变量？
- manager 索引如何同步？
- runtime registry 和 block-file 生命周期如何同步？
- lookup 后为什么还要执行 current-instance 重验？
- `Arc` 保证了什么，又没有保证什么？
- 哪些操作会等待 in-flight I/O？
- 哪些外部操作不能在持有某把锁时执行？
- 是否存在固定锁顺序？
- remove、rename、load、resume 和 status 并发时分别如何表现？

应特别牢记：

```text
Arc 保证对象仍然存活
≠ 对象仍在 manager 中
≠ 对象仍是同名 mapper 的当前实例
≠ 业务状态仍允许本次操作
```

#### 地图六：验证与证据地图

| 证据 | 可以证明 | 不能单独证明 |
|---|---|---|
| CLI 输出和后续命令 | 用户可见结果与状态变化 | 内核具体执行了哪个分支。 |
| `[dm-debug]` 日志 | 已埋点的控制流和真实状态提交 | 未埋点字段、完整调用栈和原始 buffer。 |
| `strace` | syscall、FD、ioctl、errno 和用户态/内核边界 | 内核内部为何这样实现。 |
| 源码 | 可能路径、设计约束、锁和失败边界 | 本次运行实际经过该路径。 |
| GDB | 实际调用栈、分支、局部状态和少量原始内存 | 长期并发正确性和性能结论。 |
| 定向测试 | 特定输入和断言覆盖的语义 | 未编码到测试中的全部边界。 |
| 系统验收 | 真实 dmsetup/LVM2 组合路径 | 精确根因和所有局部不变量。 |

### 3.2 每个命令固定回答八个问题

今后复盘任何 `dmsetup` 命令时，统一回答：

1. 用户要什么？
2. 前置状态是什么？
3. 对应哪个 ioctl 或接口？
4. 核心调用链是什么？
5. 修改或读取哪些状态和资源？
6. 涉及哪些锁、并发访问者和等待关系？
7. 失败后已经提交什么、保留什么、回滚什么？
8. 如何用最小正常、失败和并发证据验证？

AI 只能在学习者先作答后进行追问、纠偏和补充，避免再次由 AI 完成全部认知劳动。

### 3.3 P0 验收标准

P0 不以文档页数验收，而以随机闭卷讲解验收。随机抽取一个已学命令，学习者在不看现成答案的情况下应能：

1. 准确说明用户语义和前置状态；
2. 从代码中重新找到 ioctl、handler 和核心状态函数；
3. 画出主要状态和资源变化；
4. 说明相关锁和至少一个并发窗口；
5. 说明两个失败或非法状态边界；
6. 设计正常、失败和必要的并发验证；
7. 指出哪些是代码事实、哪些是运行事实、哪些仍待验证。

如果只能复述最终结论，不能独立重新找到路径，则该主题仍未完成接管。

---

## 4. P1：完成一次由学习者主导的完整工程闭环

### 4.1 任务选择原则

首个独立闭环应满足：

- 范围有限；
- 用户可见；
- 可以稳定复现；
- 包含真实状态或失败路径；
- 不需要一次理解整个内核；
- 可以设计明确的回归验证。

合适类型包括：

- 独立分析一个尚未系统学习的简单命令；
- 定位并修复一个可复现的小 bug；
- 补足一个 ioctl 的非法输入或非法状态行为；
- 增加一条资源失败路径的故障注入验证；
- 补一项当前弱测试无法区分的语义。

首个闭环不选择 snapshot、mirror、大规模性能改造或多子系统重构。

### 4.2 角色反转

学习者必须先完成：

```text
复现现象
→ 定义原有行为和目标行为
→ 写出前置状态与非目标
→ 定位源码入口
→ 画初步调用链
→ 写状态/资源/并发模型
→ 列出竞争假设
→ 设计测试矩阵
→ 提出实现方案
```

随后 AI 才可以：

- challenge 设计；
- 搜索遗漏位置；
- 提醒可能的边界；
- 在批准的设计内生成代码；
- 提供第二视角 code review；
- 协助分析编译器或测试输出。

### 4.3 编码前的最小设计包

在把编码交给 AI 前，学习者至少应提供：

```text
原有行为
目标行为
用户可见示例差异
范围与非目标
前置状态
状态转换
资源创建/发布/撤销/回收
锁与并发访问者
失败提交点和回滚策略
接口变化
测试矩阵
验收标准
```

缺少上述核心信息时，AI 应先提出问题或指出空缺，而不是直接扩展生产代码。

### 4.4 Patch 接管要求

AI 生成 patch 后，学习者必须自行解释：

- 为什么修改这些文件和层次？
- 哪几行承担核心状态提交？
- 关键值由谁拥有？
- 哪些 clone 只是增加引用计数？
- 哪些锁在何处取得和释放？
- 每个关键 `?` 提前返回时已经修改了什么？
- guard 未 commit 时会发生什么？
- 哪一步可能失败？
- 测试为什么能够发现错误实现？
- 哪些风险仍未覆盖？

不能解释关键 diff，就不能把该功能视为已经转化成个人能力。

### 4.5 P1 验收标准

完整闭环应由学习者独立完成并保留证据：

```text
现象/需求
→ 正确语义
→ 调用链
→ 根因或设计缺口
→ 状态与资源方案
→ 测试矩阵
→ AI 辅助编码
→ 人工 review
→ 定向验证
→ 用户态验收
→ 未覆盖项
```

AI 可以指出错误，但不能替代学习者产出第一版分析。

---

## 5. P2：面向 AI 代码审查的 Rust 学习路线

目标不是优先掌握全部 Rust 语法，而是先获得审查当前内核代码正确性所必需的能力。

### 5.1 第一优先组

#### 所有权、借用与移动

需要能够判断：

- 值何时发生 move；
- `&T`、`&mut T` 分别提供什么保证；
- borrow 的作用域；
- clone 的是完整数据还是共享引用；
- Rust lifetime 与业务生命周期为何不是同一概念；
- 编译器保证了哪些内存安全，哪些业务不变量仍要人工维护。

#### `Arc`、`Mutex`、`Send` 与 `Sync`

重点理解：

- `Arc<T>` 只管理共享所有权和对象存活；
- `T` 的线程安全仍取决于其内部类型和同步；
- `Mutex` 应保护明确的不变量，而不是仅仅“让编译通过”；
- 锁外 snapshot 何时失效；
- current-instance 检查为何不能被引用计数替代；
- 锁粒度、持锁等待和锁顺序如何影响正确性。

#### `Option`、`Result`、`?` 与 errno

需要能够 review：

- `None` 是正常空状态还是错误；
- domain error 在哪一层映射成 Linux errno；
- `?` 提前返回是否遗漏资源撤销或状态恢复；
- 错误发生时哪些步骤已经提交；
- 错误转换是否丢失用户可见语义。

#### enum 与状态机

通过 `Running / Suspending / Suspended` 等实际状态学习：

- enum 如何排除部分非法组合；
- 为什么多个独立 bool 容易制造不可解释状态；
- phase 与 active/inactive 等正交字段如何共同构成系统状态；
- 状态转换的前置条件应在哪里检查。

#### RAII guard 与 `Drop`

需要能够解释：

```text
构造 guard
→ 暂时持有状态、锁或资源
→ 成功后 commit
→ 未 commit 时 Drop 自动恢复或释放
```

重点检查：

- 所有提前返回路径；
- commit 后是否还会错误回滚；
- guard 是否跨越不应持锁的外部操作；
- panic 或错误传播时是否保持不变量。

### 5.2 第二优先组

- trait、associated type 与 `dyn Trait`；
- 泛型静态分派和 trait object 动态分派；
- closure、回调和依赖注入；
- interior mutability；
- 原子操作和内存序；
- unsafe、裸指针、layout、alignment 和 ABI；
- 宏与高级 lifetime 技巧；
- async/pin 仅在实际项目需要时学习，不作为当前首要内容。

### 5.3 项目驱动的学习方式

每个 Rust 主题都必须绑定当前代码案例：

| Rust 主题 | DM 中的观察点 |
|---|---|
| `Arc` | manager、device、table 的共享所有权和 stale instance。 |
| `Mutex` | lifecycle 串行、device state 和 registry 生命周期。 |
| `Result` / `?` | domain error、errno 映射和跨资源失败路径。 |
| enum | device phase、runtime 状态和 request 类型。 |
| RAII | initial resume、rename/remove 等事务 guard。 |
| trait object | `BlockDevice`、`DmTarget` 和 target catalog。 |
| closure | 可注入的 runtime 操作与失败测试。 |
| ABI/layout | `dm_ioctl`、可变长 records、copy-in/copy-back。 |

学习步骤统一为：

```text
先从真实代码提出问题
→ 补对应 Rust 概念
→ 回到代码解释编译器保证和业务保证
→ 做一个小修改或 review 练习
→ 用测试或编译错误验证理解
```

### 5.4 P2 验收标准

面对一段 AI 生成的 DM patch，学习者应能够逐项回答：

- 每个关键值由谁拥有？
- clone 的实际成本和语义是什么？
- 是否存在超出业务有效期的旧 `Arc`？
- 哪个锁保护哪个不变量？
- 哪个 `?` 可能产生半提交？
- guard 的 drop 和 commit 路径是什么？
- trait object 的具体实现如何选择？
- ABI 是否依赖了不稳定的 Rust 内存布局？

---

## 6. P3：由学习者主导测试设计与证据驱动调试

如果编码大量交给 AI，测试和验收反而必须更多由学习者主导。测试不是实现后的装饰，而是编码前对“正确”的可执行定义。

### 6.1 最小测试矩阵

每项功能至少检查：

| 类型 | 核心问题 |
|---|---|
| 正常路径 | 合法前置状态下是否得到预期状态和用户结果？ |
| 非法输入 | 名称、长度、flags、selector、buffer、record 边界错误如何处理？ |
| 非法状态 | tableless、suspending、removing、无 active/inactive table 时会怎样？ |
| 失败注入 | primary/alias 发布失败、registry 操作失败、backing 错误时是否保持原子性？ |
| 并发场景 | load/remove、rename/status、suspend/BIO、wait/event 是否正确串行或隔离？ |
| 资源回收 | node、alias、minor、lease、table、BIO、waiter 是否最终释放或结束？ |
| 用户态兼容 | dmsetup/LVM2 是否按预期消费 header、flags、records 和 errno？ |

不是每个小命令都必须运行高成本并发测试，但必须明确判断哪些类别相关、哪些暂不适用及其理由。

### 6.2 未知问题的调试纪律

面对未知故障时，不把“直接询问 AI 根因”作为第一步。学习者先提交：

1. 可重复的用户可见现象；
2. 正确行为基线；
3. 至少两个竞争假设；
4. 每个假设对应的可观察证据；
5. 能够区分假设的最低成本实验；
6. 实验结果和被排除的路径。

示例：

```text
现象：load 返回成功，但 status 未显示 inactive table

假设 A：table 没有真正提交
假设 B：table 已提交，但 response header 编码错误
假设 C：status 查询命中了旧实例或错误 selector

区分方式：
先看状态提交日志
→ 再对比内部 snapshot 与 ioctl copy-back
→ 最后检查 selector 和 current-instance identity
```

### 6.3 工具升级顺序

```text
CLI 与后续状态观察
→ [dm-debug] 低频结构化日志
→ strace 确认 userspace/kernel 边界
→ 源码提出路径和假设
→ 必要时 GDB 验证真实分支、字段或并发状态
→ 定向测试固化结论
```

工具使用原则：

- 日志足以证明时，不为展示技巧进入 GDB；
- `strace` 看不到可变长 payload 时，不从 `...` 推断内部字段；
- 源码只能证明可能路径和设计，不自动证明本次运行实际经过；
- 一次成功不能证明并发安全；
- 测试通过只能证明测试断言覆盖的范围；
- 根因确定后必须增加能够区分错误实现的回归验证。

### 6.4 P3 验收标准

面对一个未知问题，学习者能够在 AI 不预先给出根因的情况下完成：

```text
复现
→ 建立正确基线
→ 判断问题层次
→ 提出竞争假设
→ 设计区分实验
→ 收集运行证据
→ 定位关键函数
→ 得出根因
→ 设计回归测试
→ 说明尚未覆盖的风险
```

第一次假设可以错误，但推理和证据链必须由学习者主导。

---

## 7. P4：沿 Device Mapper 补齐内核知识体系

当前不以泛读整个 Linux 内核为目标，而是先沿一条真实纵向链路建立可迁移知识。

### 7.1 第一阶段：当前 DM 必需基础

#### syscall 与 ioctl ABI

- syscall dispatch；
- 文件描述符与 VFS file operation；
- userspace buffer；
- copy-in/copy-back；
- 固定 header 和可变长 payload；
- endianness、alignment、NUL、record offset；
- errno 和用户态库消费。

#### 设备模型

- `DeviceId`；
- major/minor；
- block device；
- device node；
- registry；
- open/close；
- open count；
- primary 与 alias；
- runtime publication 和 revoke。

#### 块 I/O

- sector 与容量；
- BIO；
- read/write/flush；
- target selection；
- split；
- remap；
- backing I/O；
- completion 聚合；
- in-flight drain；
- postponed queue 与 replay。

#### 并发基础

- mutex 和 guard；
- condition/event wait；
- lost wake-up；
- lock ordering；
- snapshot 和 generation；
- graceful drain；
- stale object；
- remove 与外部旧引用隔离。

### 7.2 第二阶段：向外围扩展

完成当前纵向链路后，再逐步学习：

- VFS 到 block device 的完整路径；
- 页缓存与 direct I/O；
- 线程调度、阻塞和唤醒；
- 内存管理基础；
- 设备初始化；
- initramfs、启动流程和 QEMU 虚拟设备；
- Linux upstream DM 架构；
- Asterinas 当前实现与 Linux 语义、内部结构的差异。

### 7.3 P4 验收标准

能够解释一条真实 I/O：

```text
用户进程发出读写
→ VFS / block file
→ mapper BlockDevice
→ phase 与 active table 选择
→ target 定位和 BIO split
→ backing device 或本地完成
→ completion 聚合
→ 原始请求完成
```

同时能够说明 suspend、resume、table replacement 和 remove 分别会在哪些位置影响这条路径。

---

## 8. P5：性能、故障注入与长期维护

P5 在 P0～P3 形成稳定能力后重点开展，包括：

- 并发压力测试；
- 失败注入；
- race 复现；
- 长时间运行和资源泄漏；
- 性能 profiling；
- 锁竞争分析；
- 大表、mixed table 和边界容量；
- 与 Linux 用户可见语义对照；
- dmsetup/LVM2 兼容性；
- 文档与代码漂移检查；
- review 和提交组织；
- 上游化所需的接口、注释和测试质量。

这一阶段不以“优化得更快”为唯一目标，而是同时确认：

```text
语义正确
→ 并发安全
→ 失败可恢复或可隔离
→ 资源最终回收
→ 性能瓶颈有证据
→ 优化没有破坏兼容性和可维护性
```

---

## 9. 学习者与 AI 的职责边界

### 9.1 学习者必须主导

- 需求澄清；
- 用户语义；
- 原有行为和目标行为；
- 范围与非目标；
- 架构分层；
- 状态机；
- 资源所有权与生命周期；
- 并发模型和锁边界；
- 失败策略；
- 测试矩阵；
- 验收标准；
- 关键 patch review；
- 运行结果解释；
- 最终结论和责任。

### 9.2 可以大量交给 AI

- 明确接口下的机械实现；
- 重复性 ABI 编解码；
- 样板代码；
- 测试框架样板；
- 文档排版和候选结构；
- 搜索可能的入口；
- 生成初版 patch；
- 提取重复逻辑；
- 提供候选风险清单；
- 做第二视角 review。

### 9.3 必须共同完成

以下内容即使由 AI 先写，学习者也必须逐段审查：

- 锁和生命周期；
- 状态机修改；
- 跨资源事务；
- remove 和 rollback；
- runtime publication/revoke；
- unsafe 和 ABI；
- 并发测试；
- 根因判定；
- 与 Linux 用户可见语义的兼容判断。

### 9.4 禁止退化的协作模式

应避免长期重复：

```text
学习者提出模糊目标
→ AI 搜索全部源码
→ AI 完成设计
→ AI 编写代码
→ AI 设计测试
→ AI 解释结果
→ 学习者只确认完成
```

如果某个紧急任务确实采用该模式交付，必须明确标记为“AI 主导交付，尚未完成个人能力接管”，并在之后安排独立复盘和 review，而不能把交付结果直接计为学习完成。

---

## 10. 固定工作流程

### 10.1 学习一个已有命令

```text
学习者先写八问答案
→ 运行 CLI 建立正确预期
→ 用日志/strace 建立最小证据
→ 独立定位入口和调用链
→ 补资源、锁、失败边界
→ AI 拷打与纠偏
→ 学习者修订结论
→ 更新 docs/study2.md
```

### 10.2 实现一个新功能或修复

```text
学习者复现或定义需求
→ 写原有/目标行为和示例差异
→ 建立状态、资源、并发模型
→ 设计测试矩阵
→ AI review 设计
→ 用户确认实施
→ AI 或学习者编码
→ 学习者 review diff
→ 定向测试
→ 必要的系统验收
→ 记录未覆盖项
```

### 10.3 Review AI 生成代码

固定检查：

1. 是否符合用户语义？
2. 修改是否位于正确层次？
3. 状态转换前置条件是否完整？
4. 资源由谁拥有和回收？
5. 是否存在 stale `Arc` 或旧 generation？
6. 锁保护什么不变量？是否持锁等待外部操作？
7. 所有 `?` 和提前返回是否保持原子性？
8. commit/rollback/隔离策略是否明确？
9. remove 是否与 create/load/resume 同步设计？
10. 测试能否在实现错误时失败？
11. 是否只验证了 golden path？
12. 文档和注释是否准确描述当前实现？

### 10.4 调试未知故障

```text
记录现象和复现条件
→ 明确正确基线
→ 判断控制面/ABI/runtime/core/data path 层次
→ 写至少两个竞争假设
→ 选择最低成本的区分证据
→ 执行实验
→ 根据证据收缩路径
→ 必要时升级 strace/GDB
→ 根因确认
→ 回归测试
```

---

## 11. 推荐时间分配

### 当前接管阶段

```text
35%：阅读和 review 当前代码
25%：独立分析、画状态机、解释调用链
20%：测试设计和调试
15%：Rust 定向补课
 5%：AI 填充机械代码
```

### 形成接管能力后

```text
25%：需求与设计
20%：关键代码 review
20%：测试与验证
15%：调试和故障分析
10%：文档与知识沉淀
10%：AI 编码迭代管理
```

这里的百分比是投入方向，不是严格工时统计。核心约束是：AI 编码可以很快，但学习者对关键输出的理解和验证时间不能同步缩减。

---

## 12. 当前执行顺序

### 第一阶段：停止盲目扩功能，接管现有主链

按以下顺序复盘：

```text
targets
→ create
→ info
→ load
→ first resume
→ suspend
→ ordinary resume / table replacement
→ clear
→ rename
→ remove
→ wait
```

每项使用八问模板。已经学习过的内容也要通过闭卷重新验证，不因现有文档已写完而视为掌握。

### 第二阶段：独立完成一次 code review

选择约 200～400 行、边界清晰的 DM 代码，学习者先独立检查：

- 业务语义；
- ownership；
- 状态不变量；
- 锁范围；
- stale object；
- 错误传播；
- 资源提交顺序；
- rollback；
- 非法输入；
- 测试缺口。

完成后再与 AI review 对照，记录：

```text
遗漏了什么
为什么当时没有看到
下次增加哪项检查规则
```

### 第三阶段：独立定位一个已知现象

选择一个用户可复现、范围有限的问题。学习者只获取用户可见现象，不提前获取根因，独立完成：

```text
正确语义
→ 复现
→ strace / 日志
→ ioctl 定位
→ dispatch 和状态路径
→ 竞争假设
→ 证据验证
→ 根因报告
```

第一轮可以只定位而不修复，以避免“编码进度”掩盖调试训练。

### 第四阶段：补齐 Rust 审查基础

结合当前源码依次学习：

```text
Arc + Mutex + current identity
→ Result + ? + rollback
→ enum + 状态机
→ RAII guard + commit
→ trait object + target catalog
→ unsafe/layout + ioctl ABI
```

### 第五阶段：学习者设计、AI 编码一个小改动

学习者提供完整设计包和测试矩阵；AI challenge 后填充代码；学习者逐段 review、运行验证并解释关键 diff。只有学习者能够说明核心状态、资源、并发和失败路径，才算完成。

---

## 13. 成长评价标准

### 13.1 不再作为主要指标

- 仓库总行数；
- AI 完成了多少功能；
- 文档页数；
- 单次测试通过；
- 能否跟随现成讲解；
- 是否记住某个函数名或命令参数。

### 13.2 核心指标

| 能力 | 可观察结果 |
|---|---|
| 需求能力 | 能独立写出原有/目标行为、非目标和验收条件。 |
| 源码阅读 | 能从陌生入口恢复主调用链，并排除无关代码。 |
| 系统建模 | 能画出状态、资源、锁和失败提交点。 |
| Rust review | 能解释 ownership、guard、`?` 和 trait 边界。 |
| 测试设计 | 测试在错误实现下确实能够失败。 |
| 调试推理 | 能提出竞争假设并用最低成本证据区分。 |
| AI 协作 | 能给出充分约束，识别 AI 输出的错误或遗漏。 |
| 知识迁移 | 能将 stale object、drain、rollback 等方法用于新问题。 |

### 13.3 能力分级

```text
Level 0：见过
知道概念或工具名称。

Level 1：跟随完成
在明确步骤和提示下可以复现。

Level 2：独立复现
不依赖逐步指导，可以重复已有流程并解释结果。

Level 3：独立分析
面对相似的新问题，可以找到路径、提出假设和设计验证。

Level 4：独立负责
可以设计、约束 AI 实现、review、验证并承担结果。

Level 5：迁移与指导
能够把方法迁移到新子系统，并帮助他人建立正确模型。
```

当前每个主题应分别评级，不能因为项目整体复杂就统一高估个人能力。

---

## 14. 文档与记录分工

```text
docs/plan.md
→ AI 时代的个人成长、项目接管、协作方式和长期优先级。

docs/global.md
→ Device Mapper 当前功能、依赖绕过、未实现优先级和全局路线。

docs/device-mapper-technical-design-and-implementation.md
→ 当前 Device Mapper 源码的结构化业务导航、状态机、资源与并发参考。

docs/study.md
→ 已整理的 Device Mapper 学习记录。

log/weekly/YYYY-Www.md
→ 当周训练范围、顺序、验收条件和明确不做事项。

log/daily/YYYY-M-D.md
→ 当天实际工程改动和验证；不记录纯讨论和未执行计划。

todo/strace.md、todo/gdb.md
→ 可复用工具方法和已确认的运行观察。
```

记录时必须区分：

- **代码事实**：当前源码能够直接确认；
- **运行事实**：本次 CLI、日志、strace、GDB 或测试已经观察；
- **推断**：根据代码或现象推导但尚未直接验证；
- **目标设计**：未来希望达到的行为；
- **当前偏差**：实现与预期语义不一致；
- **待确认**：当前证据不足。

---

## 15. 最终原则

后续不再追求“让 AI 尽快把项目做大”，而追求：

```text
学习者知道正确意味着什么
→ 能为 AI 提供充分约束
→ 能判断 AI 是否写对
→ 能设计测试证伪错误实现
→ 能在系统失败时独立开始调查
→ 能对最终结果负责
```

将机械编码交给 AI 是效率提升；将需求判断、状态机、并发模型、失败策略、测试设计和最终审查也交给 AI，则只是把未知隐藏在更多代码下面。

本计划的最终目标不是让学习者成为“完全不使用 AI 的程序员”，而是成为：

> **能够掌握整个项目，利用 AI 提高实现效率，同时保留独立判断、审查、验证和负责能力的系统软件工程师。**
