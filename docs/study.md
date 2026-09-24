# Device Mapper 项目接管训练记录

> 本文记录学习者已经独立完成或正在进行的 Device Mapper 接管训练。目标不是重复保存 AI 整理出的完整项目说明，而是记录学习者自己的分析、错误、纠偏、证据与能力变化。
>
> 当前源码是代码事实的第一权威；CLI、`[dm-debug]`、`strace`、GDB 和测试分别提供不同范围的运行证据。未经独立分析或实际验证的内容，不记录为“已经掌握”。

## 1. 记录原则

### 1.1 什么可以写入

- 学习者在不看现成答案时给出的第一版分析；
- 独立找到的 ioctl、handler、workflow 和核心状态函数；
- 独立识别的状态、资源、锁、并发和失败边界；
- 实际运行过的命令、日志、trace 和测试结果；
- 回答错误或遗漏的内容，以及产生错误的原因；
- 经源码和运行证据确认后的最终结论；
- 下一次遇到同类问题时可复用的检查规则。

### 1.2 什么不能冒充学习成果

```text
AI 已经实现的代码
≠ 学习者已经掌握

AI 整理出的调用链
≠ 学习者能够独立定位

阅读答案时觉得合理
≠ 能够闭卷复述和应用

测试已经通过
≠ 能够解释测试证明了什么
```

### 1.3 事实标记

| 标记 | 含义 |
|---|---|
| **独立分析** | 学习者在查看现成结论前自行得出的判断。 |
| **代码事实** | 已从当前源码直接确认。 |
| **运行事实** | 已由本次 CLI、日志、`strace`、GDB 或测试观察。 |
| **纠偏** | 第一版回答存在错误或遗漏，已经说明原因并修正。 |
| **待验证** | 当前只是合理推断，尚无足够代码或运行证据。 |
| **未掌握** | 仍依赖提示，暂时不能独立解释或迁移。 |

## 2. 当前能力基线

日期：2026-09-16

当前客观起点：

- 已接触 Docker、QEMU、Git、`dmsetup`、LVM2、`strace`、GDB 和内核日志，但独立选择工具、处理异常和完成未知问题定位的能力仍需验证；
- 已跟随学习 `targets`、`create`、`info`、`load` 和首次 `resume` 的部分路径，但大量调用链、设计和实现由 AI 提供；
- 已开始关注业务语义、生命周期、并发、失败和边界，但尚未证明能在陌生代码中主动发现这些问题；
- Rust 代码主要由 AI 编写，所有权、借用、`Arc`、`Mutex`、`Result`、RAII 和 trait 等能力尚需通过实际 review 验证；
- 当前最重要的目标不是继续增加功能，而是取得对现有 DM 项目的理解权、审查权和验证权。

当前阶段：**P0——接管现有 Device Mapper 主链。**

## 3. 固定训练模板

每个命令均由学习者先闭卷回答以下八项，AI 只在第一版完成后拷打和纠偏：

1. 用户要什么？
2. 前置状态是什么？
3. 对应哪个 ioctl 或接口？
4. 核心调用链是什么？
5. 读取或修改哪些状态和资源？
6. 涉及哪些锁、并发访问者和等待关系？
7. 失败后已经提交什么、保留什么、回滚什么？
8. 如何用最小正常、失败和并发证据验证？

每项训练按以下顺序进行：

```text
闭卷回答
→ 记录第一版判断
→ 追问与纠偏
→ 独立定位源码
→ 按需运行 CLI / 日志 / strace / GDB / 测试
→ 形成最终结论
→ 提取可迁移规则
→ 标记掌握等级
```

掌握等级：

| 等级 | 标准 |
|---|---|
| L0 见过 | 只知道概念或命令名称。 |
| L1 跟随完成 | 在明确步骤和提示下可以完成。 |
| L2 独立复现 | 可以重新执行已学流程并解释结果。 |
| L3 独立分析 | 面对相似新问题可以自行定位、提出假设和验证。 |
| L4 独立负责 | 可以设计、约束 AI 编码、review、验证并承担结果。 |

## 4. P0 训练进度

| 顺序 | 主题 | 当前状态 | 当前等级 | 完成条件 |
|---:|---|---|---|---|
| 1 | `dmsetup targets` | 进行中：闭卷复盘 | 待评估 | 独立完成八问，并重新定位 ABI 编码主链。 |
| 2 | `dmsetup create --notable` | 待开始 | 待评估 | 解释 identity、minor、manager index 和 tableless 状态。 |
| 3 | `dmsetup info` | 待开始 | 待评估 | 解释枚举与逐设备 status，识别非原子快照边界。 |
| 4 | `dmsetup load` | 待开始 | 待评估 | 解释 spec→target→table→primary→inactive 事务。 |
| 5 | first `resume` | 待开始 | 待评估 | 解释 alias 发布、guard、commit 和失败可重试。 |
| 6 | `suspend` | 待开始 | 待评估 | 解释 phase、in-flight drain 和新 BIO 行为。 |
| 7 | ordinary `resume` / replacement | 待开始 | 待评估 | 按前置状态拆分恢复和换表场景。 |
| 8 | `clear` | 待开始 | 待评估 | 解释 inactive table 回收及非法状态。 |
| 9 | `rename` | 待开始 | 待评估 | 解释 manager/runtime 多资源迁移和回滚。 |
| 10 | `remove` | 待开始 | 待评估 | 解释 revoke、Removing 隔离、manager detach 和最终回收。 |
| 11 | `wait` | 待开始 | 待评估 | 先定义 event 语义，再独立定位当前异常。 |

---

## 5. 2026-09-16：P0-1 `dmsetup targets` 闭卷接管

### 5.1 训练边界

本轮暂时不查看既有学习文档和 AI 整理出的完整调用链。先验证能否从已有理解中独立恢复：

```text
用户语义
→ ioctl 边界
→ 可变长 record
→ 内核编码
→ copy-back
→ libdevmapper 展示
→ 容量与并发边界
```

本轮不修改生产代码，不增加日志，不进入 GDB；只有现有 CLI、日志、`strace` 和源码不足以确认关键事实时，才升级工具。

### 5.2 第一轮：业务与 ABI 主链

状态：**已完成闭卷作答与源码纠偏。**

第一轮围绕以下四项展开：

1. `dmsetup targets` 的用户目的，以及全局能力与 mapper table 的区别；
2. libdevmapper 向 `/dev/mapper/control` 发送的核心 DM ioctl 及其职责；
3. target-version record 的字段，以及 `next`、版本整数、NUL 和 8 字节对齐的作用；
4. 从静态 target 信息到终端文本 `zero v1.1.0` 的数据流。

### 5.3 第一版回答

学习者第一版回答的核心内容：

- `dmsetup targets` 查询全局 target 能力和版本，不是某个 mapper 的 table。
- 识别出 `DM_VERSION` 与 `DM_LIST_VERSIONS` 两个核心 ioctl；最初将 `DM_VERSION` 写成了 `DM_VERSIONS`，并未意识到版本检查与版本写回位于公共 ioctl 流程。
- 正确理解 `next` 是当前 record 起点到下一条 record 起点的相对距离，最后一条为 0。
- 最初将版本数组举例为 `1 1 0`，后纠正为通用的 `[major, minor, patch]` 三个 `u32`，`zero` 当前才是 `[1, 1, 0]`。
- 正确识别 `name + NUL`、8 字节向上对齐，以及 metadata → snapshot → record → userspace 的大致数据流。
- 最初遗漏了 record 容量不足时不写半条、保留已有 record、设置 `DM_BUFFER_FULL_FLAG` 以及此前最后一条 `next` 保持 0 的失败路径。

### 5.4 纠偏记录

- **纠偏**：命令名称是 `DM_VERSION`，不是 `DM_VERSIONS`。索引：[device_mapper.rs:75-88](../kernel/core/src/device/misc/device_mapper.rs#L75-L88)。
- **纠偏**：版本兼容性检查和内核版本写回位于 `DmControlFile::ioctl` 的公共流程，而不是只属于 `DM_VERSION` 分支。索引：[device_mapper.rs:237-285](../kernel/core/src/device/misc/device_mapper.rs#L237-L285)、[device_mapper.rs:367-372](../kernel/core/src/device/misc/device_mapper.rs#L367-L372)、[device_mapper.rs:1192-1196](../kernel/core/src/device/misc/device_mapper.rs#L1192-L1196)。
- **纠偏**：三个版本字段是二进制 `u32` 数组，分别表示 major、minor、patch；各 target 的静态值不同。索引：[control.rs:166-170](../kernel/core/src/device/misc/device_mapper/control.rs#L166-L170)、[target/mod.rs:167-196](../kernel/core/comps/device-mapper/src/target/mod.rs#L167-L196)、[device_mapper.rs:775-780](../kernel/core/src/device/misc/device_mapper.rs#L775-L780)。
- **确认**：8 字节对齐的回答方向正确；当前实现将 `16 + name.len() + 1` 向上对齐为 `record_len`，padding 属于 record 占用范围，`next` 跨过完整对齐后的 record。索引：[device_mapper.rs:763-766](../kernel/core/src/device/misc/device_mapper.rs#L763-L766)、[device_mapper.rs:1179-1183](../kernel/core/src/device/misc/device_mapper.rs#L1179-L1183)。
- **确认**：snapshot 是对静态 target 声明中 `name` 与 `version` 的查询快照。索引：[target/mod.rs:234-240](../kernel/core/comps/device-mapper/src/target/mod.rs#L234-L240)、[control.rs:235-244](../kernel/core/src/device/misc/device_mapper/control.rs#L235-L244)。
- **补充**：`DM_LIST_VERSIONS` 专属 handler 负责把 target snapshot 编码为 records；公共 ioctl 外层负责用户 buffer 的读取和 copy-back。索引：[device_mapper.rs:407-416](../kernel/core/src/device/misc/device_mapper.rs#L407-L416)、[device_mapper.rs:226-285](../kernel/core/src/device/misc/device_mapper.rs#L226-L285)。
- **补充**：容量不足时不写半条 record，已有完整 record 保留，设置 `DM_BUFFER_FULL_FLAG`，并停止编码；此前最后一条仍保持 `next=0`。索引：[device_mapper.rs:812-835](../kernel/core/src/device/misc/device_mapper.rs#L812-L835)、[device_mapper.rs:1186-1190](../kernel/core/src/device/misc/device_mapper.rs#L1186-L1190)。

### 5.5 独立源码定位

本轮根据纠偏索引重新确认了命令常量、公共 ioctl envelope、target catalog/snapshot、record 长度与字段写入、`next` 回填和 buffer-full 分支。尚未进行本轮 CLI、`strace` 或 GDB 验证；相关运行事实沿用此前已记录的独立证据，不在本轮重复冒充新运行结果。

### 5.6 运行证据

本轮未新增 CLI、日志、`strace` 或 GDB 证据。

### 5.7 最终结论与掌握等级

`dmsetup targets` 已完成本轮闭卷接管：能够说明全局能力语义、两个 ioctl 的职责、target-version record 的主要 ABI 字段、metadata 到 userspace 展示的数据流，以及容量不足边界。对版本公共流程和失败分支是在纠偏后建立的理解，尚不能据此宣称已经达到完全独立分析等级。

当前主题暂评：**L2——独立复现已学流程；局部机制达到 L2/L3 之间，仍需在后续陌生路径中验证独立定位能力。**
