# Device Mapper V5 审查的事实复核

## 1. 结论摘要

本文复核 [`log/device-mapper-optimization-v5.md`](log/device-mapper-optimization-v5.md) 中的评审意见，目标是区分：

- 当前工作区仍然存在的缺陷；
- 已在 dirty 工作区修复、但尚未完成动态验证的旧问题；
- 需要收窄事实或调整严重性的意见；
- 描述同一根因、应合并实施的意见；
- 属于能力范围或资源策略、不能直接当作实现缺陷的事项；
- 原评审遗漏的 Linux Device Mapper 语义差异。

复核基线：

- **Linux 语义基线：**`/root/Linux-tree-code/kernel`，commit `5078a3a23a1e1825ec136485173ec98668cdd640`，openEuler `6.6.0-132.0.0`，DM ioctl ABI `4.48.0`。该源码树不是未经修改的 kernel.org v6.6；本地 `dm-linear.c` 学习注释和空白改动不作为上游证据。
- **Asterinas：**`dm` 分支当前 dirty 工作区。原V5文档记录的HEAD和行号只代表其审查时点；本轮以当前文件内容为准。
- **证据性质：**本轮仅做静态源码核验，没有修改生产代码，没有重新执行构建、ktest或C回归。原文记录的历史测试不能直接证明之后继续变化的dirty工作区。

总体判断：V5并非无依据，但存在四类需要纠正的问题：

1. **状态过期：**A01、A11以及L03、L04、D04等条目所针对的代码已经发生变化，不能继续按原状态计数。
2. **重复或根因重叠：**两条A08应合并；D02并入A05；D03并入A07；A02、A03、S01、S02应统一设计但分别验收。
3. **结论过宽或严重性偏高：**A06、A07、A09、A10、A12需要收窄事实；维护性意见不应作为已证实运行时缺陷。
4. **存在遗漏：**control fd的`poll`/`DM_DEV_ARM_POLL`/global-event协议、首次`TABLE_LOAD`发布窗口、首次发布的queue metadata来源，以及ABI 4.50能力声明均应补充审查。

## 2. 最终裁决总表

| 编号 | 最终裁决 | 建议严重性 | 当前状态/处理方式 |
|---|---|---:|---|
| V5.A01 | 从现存缺陷删除 | — | dirty工作区已按`_IOC_TYPE/_IOC_NR`解码；保留为“已实现、待raw ioctl验证”的历史项即可 |
| V5.A02 | 保留 | High | 仍统一要求312字节、统一校验`data_start`并复制整段输入 |
| V5.A03 | 保留 | High | 仍整包copyout，且未正确区分输入容量和实际输出长度 |
| V5.A04 | 修改后保留 | High | 应拆成输出位归一、自定义bit、逐命令合法flags；不能说Linux接受任意未知位 |
| V5.A05 | 保留 | High | selector仍按UUID→name→dev优先级选择，可能让mutation作用于非预期设备 |
| V5.A06 | 修改后保留 | Low/Medium | 满长固定字段与UTF-8差异仍在；删除已过时的REMOVE_ALL日志提前失败例子 |
| V5.A07 | 修改后保留 | Medium | ABI布局、筛选、容量和空列表行为有差异；不能说所有无UUID客户端必然错位 |
| V5.A08 | 两条合并后保留 | High | WAIT缺target status records，且拒绝合法查询flags；删除交错问题另放L05 |
| V5.A09 | 收窄后保留 | Low/Medium | 当前直接可证的是TABLE_DEPS的`target_count`；其他命令必须逐项证明 |
| V5.A10 | 修改后保留 | Low/Medium | Linux使用保守needed上界并在首轮不足时不输出部分列表 |
| V5.A11 | 从现存缺陷删除 | — | dirty工作区已忽略末项`next`；保留为已实现待边界验证的历史项即可 |
| V5.A12 | 修改后保留 | Low/Medium | tokenizer缺转义；VFS已支持cwd-relative，限制位于DM façade |
| V5.S01 | 保留 | High | 尚未在dispatch前清除带secure flag的用户输入区 |
| V5.S02 | 保留，标记部分修复 | High | RAII覆盖外层buffer，但可靠清零、临时/持久副本和故障路径尚未完整验收 |
| V5.L01 | 保留 | High | noflush suspend仍缺Linux要求的在途I/O静止点和pushback/requeue协议 |
| V5.L02 | 保留 | High | suspend及隐式suspend等待不可被信号取消，失败回滚不完整 |
| V5.L03 | 改为已修复待验证 | — | 当前`rename_uuid`已禁止覆盖既有UUID并保持错误顺序 |
| V5.L04 | 改为已修复待验证 | — | 当前已增加mapper名称校验；仍需用失败后状态不变回归验收 |
| V5.L05 | 保留 | High | 已开始的WAIT在remove后被current-instance检查转成`ENXIO` |
| V5.L06 | 保留 | Low/Medium | rename/remove仍可能在无live table时无条件推进device event |
| V5.L07 | 保留 | Low/Medium | Running且无表的设备resume不符合Linux幂等行为 |
| V5.L08 | 保留 | Low/Medium | REMOVE_ALL只遍历单次快照，没有成功删除后重扫 |
| V5.F01 | 保留为框架能力缺口 | High（能力影响） | 缺freeze/sync/thaw协议属事实；“未补齐前应拒绝”是策略建议，不是Linux源码事实 |
| V5.M01 | 保留 | Low | 生命周期锁约束只存在于注释和调用纪律中 |
| V5.M02 | 保留 | Low | rename mutation和事件发布职责分散；应与正确事件条件一起处理 |
| V5.M03 | 保留 | Low | `_field`参数未用于诊断 |
| V5.D01 | 保留 | Medium | ABI 4.50及兼容文档容易暗示尚未实现的能力 |
| V5.D02 | 并入A05 | — | 属于selector事实和注释错误，不单独重复计数 |
| V5.D03 | 并入A07 | — | 属于name-list ABI布局说明错误，不单独重复计数 |
| V5.D04 | 改为已修正文档待审计 | Low | dirty注释已不再承诺跨VFS/manager原子可见 |
| V5.B01 | 保留为兼容边界 | Medium | 1 MiB是本地资源策略，确实拒绝Linux基线可接受的更大请求 |
| V5.B02 | 拆分 | 见分项 | 普通未实现命令是范围决定；poll/global event是独立缺陷；sysfs/uevent/udev是生态缺口 |
| V5.B03 | 保留 | Medium | errno及校验/副作用顺序尚未系统对齐；失败后状态改变的具体项应升High |
| 新增P01 | 新增 | High | control poll、ARM_POLL和global-event协议错误 |
| 新增P02 | 新增 | Medium | 首次TABLE_LOAD先发布primary、后提交inactive table的可见窗口 |
| 新增P03 | 新增 | Medium | 首次发布时queue metadata没有绑定首张待加载表 |
| 新增P04 | 新增 | Medium | 报告ABI 4.50，但仍缺部分4.48既有行为 |

## 3. ABI与输入输出语义

### 3.1 V5.A01：删除现存缺陷，保留历史验证项

**Linux源码事实：**`ctl_ioctl()`按`_IOC_TYPE`判断DM namespace，`lookup_ioctl()`按`_IOC_NR`查找命令，不要求direction和encoded size严格匹配某个固定前缀。

**当前dm事实：**dirty工作区中的命令解码已只提取type和nr，不再要求完整`0xc138_fd00`前缀。

**裁决：**原问题已不再是当前缺陷。若保留审计历史，应标记“已实现、待验证”，并用不同direction/size的raw ioctl C回归确认权限、版本交换及已知/未知nr的错误顺序。

### 3.2 V5.A02：保留——命令输入边界仍不符合Linux

**Linux源码事实：**

- 固定输入最小长度是`offsetof(struct dm_ioctl, data) == 305`；`sizeof(struct dm_ioctl) == 312`来自C结构体尾部对齐。
- `check_version()`已经复制前12字节，`copy_params()`从offset 12补读到305，不重读version。
- 标记为`IOCTL_FLAGS_NO_PARAMS`的命令不读取variable payload。
- 输出命令可以由内核设置输出起点，不要求用户提供统一有效的输入`data_start`。

**当前dm事实：**仍统一读取和要求312字节，统一约束`data_start >= 312`且8字节对齐，并按声明的`data_size`读取完整输入。

**影响：**不需要payload的命令可能因为不可访问的无关尾页返回`EFAULT`，或者因无意义的`data_start`返回`EINVAL`。

**裁决：**保留High。A02应与A03、S01、S02统一设计，但保护页输入边界必须独立验收。

### 3.3 V5.A03：保留——输出长度和失败copyout仍错误

**Linux源码事实：**普通handler只有成功时才copyout；输入容量、内核工作buffer长度和实际输出`data_size`是不同概念。version写回和secure-data清理有自己的独立时序。

**当前dm事实：**普通命令结束后仍尝试把整个分配buffer写回；没有按命令生成的实际结果长度限制copyout，也没有稳定更新输出`data_size`。

**影响：**

- 未使用的只读尾页可让本应成功的命令返回`EFAULT`；
- 普通handler失败时会修改version之外的用户内存；
- 查询响应把输入容量伪装成有效输出长度。

**裁决：**保留High。修复时不能把version交换、普通handler输出和secure-data用户区清理混为一种copyout规则。

### 3.4 V5.A04：拆解后保留

准确结论应为：

1. Linux在dispatch前统一清除若干输出位，例如BUFFER_FULL、UEVENT_GENERATED、SECURE_DATA和DATA_OUT；
2. Linux没有当前dm这种façade级统一全局白名单，剩余位由具体命令解释或忽略；
3. 当前dm定义了Linux UAPI中不存在的`DM_EXISTS_FLAG` bit 2；
4. WAIT合法使用的status/query-inactive flags被当前入口拒绝。

不能将其表述为“Linux接受任意未知位”。正确修复方向是命令级flags表和统一输出位归一，不是无条件接受所有输入bit。

### 3.5 V5.A05：保留——含糊selector可作用于错误设备

**Linux源码事实：**非create命令中，name+UUID由参数校验返回`EINVAL`；name+dev或UUID+dev无法找到设备，外层通常返回`ENXIO`。

**当前dm事实：**仍按UUID、name、dev的优先级静默选择一个设备。

**影响：**删除、改名、换表等mutation请求可以携带彼此矛盾的标识，当前实现仍作用于其中一台设备，而Linux拒绝请求。

**裁决：**保留High。D02属于该问题的错误注释，应合并修复而不重复计数。

### 3.6 V5.A06：删除过期例子后保留

仍成立的事实：

- Linux会强制固定name/UUID字段的最后一个字节为NUL；当前dm要求输入原本就含NUL。
- Linux UAPI按字节数组处理名称；当前dm要求UTF-8，属于表达能力边界。

应删除的事实：当前诊断日志解析已使用容错路径，不能继续用“REMOVE_ALL被日志预解析提前拒绝”作为现存例子。哪些命令仍解析本应忽略的字段，必须按命令逐项列证，不能泛化。

**裁决：**Low/Medium。非UTF-8支持涉及manager、VFS和alias表示，不是修改一个解析函数即可完成。

### 3.7 V5.A07：收窄并降级

**确定差异：**

- Linux 4.48的name-list记录无条件包含`event_nr`和flags；`DM_UUID_FLAG`只控制UUID内容及其标志。
- 当前dm缺name/UUID prefix filter。
- 容量不足时当前dm可能输出部分记录；Linux通常先计算所需容量。
- 空列表没有按基线写首记录`dev=0`及对应有效长度。
- 当前dm没有正确更新实际输出`data_size`。

**需要删除的过强表述：**不能说“所有不请求UUID的客户端都会错位”。只按基础`next`链遍历的旧客户端仍可能工作；依赖4.48扩展字段的客户端才会读取到错误位置、遗漏event信息或观察陈旧内容。

**裁决：**保留Medium，并与D03合并实现和文档修正。

### 3.8 V5.A08：合并两条重复finding

**Linux源码事实：**`DM_DEV_WAIT`在事件发生后返回设备头，并按active/inactive选择返回target status records；status/table模式由flags决定。

**当前dm事实：**WAIT只调用通用header encoder，`target_count`为0，没有target records；入口还拒绝合法status/query-inactive组合。

**裁决：**合并成一条High问题。WAIT/remove引用语义属于L05，不能混入A08的输出编码根因。

### 3.9 V5.A09：只保留当前可直接证明的TABLE_DEPS

公共header helper固定写`target_count=0`，但DEV_STATUS和TABLE_STATUS已有自己的覆盖逻辑。因此不能仅凭共享helper断言load、clear、rename、suspend等所有响应都错误。

当前高置信实例是：有表的`TABLE_DEPS`仍可能返回`target_count=0`。其他命令应结合命令结束时所选active/inactive表逐项核验。

**裁决：**保留为Low/Medium，不再作为广泛High缺陷。

### 3.10 V5.A10：保留，但准确描述Linux needed规则

Linux为稳定target目录计算每项`sizeof(record) + name + NUL + 7`的保守上界；首轮发现容量不足时置BUFFER_FULL，不开始输出部分列表。该needed不是最终按8字节对齐后的精确输出长度。

当前dm边编码边发现不足，可能暴露前缀记录，同时没有正确更新实际`data_size`。LIST_VERSIONS与GET_TARGET_VERSION都应验收，但修复不能简单改成“精确align长度预检”。

### 3.11 V5.A11：删除现存缺陷

当前dirty解析器已只对非末项消费和验证`next`，最后一个target在剩余输入范围内查找参数NUL，与Linux`populate_table()`的核心循环语义一致。

**裁决：**从当前缺陷清单删除，保留一项一表、多项重叠、末项任意`next`和参数越界的边界回归即可。

### 3.12 V5.A12：确认限制位于DM façade

**Linux源码事实：**`dm_split_args()`支持反斜杠转义；backing token若不是严格设备号格式，会进入路径解析，不要求绝对路径。

**当前dm事实：**linear/striped使用ASCII空白切分，不支持反斜杠转义；`parse_backing_device_id()`只有以`/`开头才进入VFS，否则只接受`major:minor`。

**框架事实：**`FsPath`已经支持基于当前工作目录解析相对路径。因此“VFS是否支持相对路径尚待核实”应删除；限制明确位于DM façade。

**裁决：**Low/Medium。公共tokenizer和“非严格`major:minor` token交给`FsPath`”可以一起设计，但应分开验收。非UTF-8路径是另一架构边界。

## 4. 安全语义

### 4.1 V5.S01：保留High——用户输入区没有按secure-data协议清除

Linux在成功捕获带`DM_SECURE_DATA_FLAG`的输入后、命令dispatch前清除用户输入区。当前`IoctlBuffer`的RAII清零只覆盖内核副本，不能替代用户缓冲区这一可观察UAPI安全语义。

修复必须保证：

- 用户区清除失败时不执行命令；
- 普通响应不会把原始敏感payload重新写回；
- 成功、命令校验失败、未知target和copyout故障分别验收。

### 4.2 V5.S02：保留High，标记部分修复

当前dirty工作区已经有两项实质改进：

- `IoctlBuffer`在`Drop`中清理外层内核buffer，覆盖更多早退路径；
- target variable参数改为借用buffer，减少普通`String`副本。

但仍不能标为解决，因为尚未完整证明：

- 所用清零方式具备可靠、不会被优化移除的安全性质；
- 部分copyin失败、handler失败、copyout失败均实际触发清理；
- target、rename和其他路径产生的所有临时或持久副本均受保护；
- 故障注入测试能够观察清理行为。

S01是用户缓冲区语义，S02是内核副本生命周期；两者不能互相替代。

## 5. 生命周期与并发语义

### 5.1 V5.L01：保留High——noflush不等于不等待在途BIO

Linux noflush主要改变freeze/flush和target pushback策略；在存在active map时，suspend仍需达到I/O静止点。当前`suspend_no_flush()`直接将phase改为Suspended，不等待已经分配的BIO。`Arc<DmTable>`只能保证旧表对象存活，不能保证noflush成功返回时旧I/O已经停止，也不能替代Linux的`BLK_STS_DM_REQUEUE`协议。

### 5.2 V5.L02：保留High——等待不可中断且回滚不完整

normal suspend和运行中换表会等待BIO drain，但当前等待不能因信号取消。Linux使用可中断等待，失败时恢复admission、撤销suspend状态，并处理已经取出的新表。

正确修复不只是把等待函数换成可中断版本，还必须定义：

- signal到达时phase恢复；
- postponed BIO恢复或重放；
- 隐式suspend期间暂取inactive table的回滚；
- filesystem freeze已发生时的thaw。

### 5.3 V5.L03：改为已修复待验证

当前`rename_uuid()`已在同一manager临界区检查目标UUID冲突和源设备已有UUID，并禁止覆盖。原finding不应继续作为现存缺陷计数。

仍需验证Linux错误顺序：目标UUID已被其他设备占用时优先返回占用错误；源设备已经具有UUID时拒绝再次设置；失败后索引、事件和设备UUID均不变。

### 5.4 V5.L04：改为已修复待验证

当前已引入统一mapper名称校验，拒绝空名、超长名、含`/`、`.`、`..`和`control`。因此原“先提交rename再发现名字非法”的主要路径已经改变。

仍应通过C/ktest确认create和rename的长度、保留名及失败状态不变，而不是继续将旧源码结论列为现存High。

### 5.5 V5.L05：保留High——已开始WAIT不应被删除后的hash状态否定

Linux WAIT在lookup后持有mapped-device引用；remove唤醒waiter后，waiter继续返回最终状态和target records。当前路径在唤醒后重新套用只适用于新mutation的current-instance校验，设备已从manager移除时会返回`ENXIO`。

Rust `Arc`足以表达对象保活，但当前控制流没有利用这份引用完成最后一次状态读取。修复应区分：

- remove前已经成功开始的WAIT；
- remove后才新发起的WAIT。

### 5.6 V5.L06：保留Low/Medium——device event发布条件不一致

当前rename/remove路径可能在完全无表或只有inactive table时无条件推进device-local `event_nr`。Linux只在存在live table时调用相应table event。

本条只讨论device-local WAIT事件；不能与kobject uevent、control global event或udev cookie混为一条通知链。

### 5.7 V5.L07：保留Low/Medium——Running空设备resume不幂等

Linux ioctl层在没有新表且设备已经不是suspended时直接成功返回状态。当前刚create、从未suspend且没有表的设备可能进入`resume()`并因active/inactive均为空返回`EINVAL`。

应区分：

- Running、无表：resume幂等成功；
- 真正Suspended、无可恢复表：可以失败。

### 5.8 V5.L08：保留Low/Medium——REMOVE_ALL缺少成功后的重扫

Linux每成功删除一个设备后重新扫描，直到一轮没有进展。当前实现只遍历一次预先捕获的设备快照。

即使不支持DM-on-DM，仍可通过busy设备fd关闭及并发create构造可观察差异。修复时不应跨I/O drain长期持有全局manager锁。

### 5.9 V5.F01：保留为框架能力缺口

Linux默认普通suspend涉及文件系统freeze、同步和resume后的thaw。当前Asterinas VFS有`sync()`能力，但缺少完整freeze/thaw及阻止新文件系统写入的协议。BIO drain不能覆盖仍在page cache中的脏数据。

**事实结论：**当前默认suspend不具备Linux lockfs语义。

**策略结论：**在框架补齐前，是拒绝默认操作、仅支持显式skip-lockfs子集，还是文档化限制后继续接受，需要单独产品/兼容性决策；不能伪装成Linux源码直接规定的唯一处理办法。

## 6. 维护性与文档

### 6.1 V5.M01：保留Low

`ControlWorkflow`依赖调用者已经持有生命周期guard并完成current-instance复核，但方法签名只接收普通设备引用。当前生产调用链可能正确，这仍是未来新增调用点可绕过的维护风险，不是已经证明的竞态漏洞。

### 6.2 V5.M02：保留Low

rename的manager/runtime mutation和`notify_event()`分散在workflow与ioctl façade。若整理职责，必须同时采用正确的live-table事件条件，不能只把当前无条件通知机械地下沉。

### 6.3 V5.M03：保留Low

字符串解析器接收字段标签却以`_field`忽略。要么删除无效参数，要么用枚举或静态消息提供稳定字段诊断。

### 6.4 V5.D01：保留Medium

当前报告DM ioctl ABI 4.50，但兼容文档没有完整说明：

- 接受和拒绝的命令；
- `CAP_SYS_ADMIN`要求；
- target子集；
- DM-on-DM限制；
- sysfs、uevent、udev/cookie和mapper alias的责任边界；
- buffer大小及其他本地ABI限制。

文档不能把“存在block设备模型/sysfs基础设施”写成“DM专属sysfs和udev链已经完成”，也不能把内核直接创建alias等同于用户态udev同步。

### 6.5 V5.D02、D03：分别并入A05、A07

二者是对应实现差异的错误源码注释。应随行为修复或差异说明一起更新，避免重复计算同一根因。

### 6.6 V5.D04：改为已修正待审计

当前dirty注释已经说明：alias先在VFS侧移动，manager reservation随后提交；它不再承诺跨VFS和manager查询者的原子可见性。若无新的错误承诺，本条应从现存缺陷移出。

## 7. 资源与能力边界

### 7.1 V5.B01：保留Medium兼容边界

当前1 MiB上限是明确的本地资源策略，会拒绝Linux基线可以接收的更大请求。它不等同于内存安全漏洞，也不应机械放大到Linux约1 GiB的理论上限。

需要明确：

- 支持上限；
- 超限错误；
- 分配失败错误；
- TABLE_LOAD和大型查询的合理容量需求；
- 对兼容性文档和测试的影响。

### 7.2 V5.B02：必须拆分

- TARGET_MSG、SET_GEOMETRY及其他普通未实现命令：能力范围决定。
- DM专属sysfs attrs、kobject uevent、cookie及用户态udev规则：Medium生态能力缺口。
- control `poll()`、`DM_DEV_ARM_POLL`和global-event：内核控制协议错误，应独立列为P01，而不是埋在范围清单中。

### 7.3 V5.B03：保留Medium

错误码和校验顺序仍未对全部命令及输入组合完成系统核验。不能把若干路径已对齐表述为全错误空间兼容。

若具体命令出现“返回失败但状态已经改变”，该实例应提升为High并单独修复；不能只当作errno数字差异。

## 8. 原V5遗漏的问题

### 8.1 P01：control poll、ARM_POLL和global-event协议错误——High

**Linux源码事实：**

- 每个control fd在open时记录全局事件号基线；
- 只有全局事件号推进后，control fd的poll才返回可读；
- `DM_DEV_ARM_POLL`重新武装当前fd的事件基线；
- create、remove、rename及table event等操作推进global event。

**当前dm事实：**

- `DmControlFile::poll()`对请求中的IN/OUT恒返回ready，可能造成poll/epoll busy loop；
- `DM_DEV_ARM_POLL`未实现；
- control file没有per-open baseline；
- 没有统一global event counter和wait queue。

**责任边界：**per-device `event_nr`与`DM_DEV_WAIT`不能替代control global-event；kobject uevent和udev cookie又是另外的通知链。

**验证建议：**

- 新open的control fd在无事件时不应可读；
- create/rename/remove/table event后变为可读；
- ARM_POLL后重新阻塞；
- 两个control fd维护独立baseline；
- epoll循环不产生无事件忙轮询。

### 8.2 P02：首次TABLE_LOAD的primary发布/表提交顺序窗口——Medium

**当前dm事实：**`load_table_with_primary()`先调用primary发布，再把已验证表写入inactive slot。因此并发观察者可能看到primary已注册、`/dev/dm-N`可以open，但inactive table尚未提交。

**Linux源码事实：**首表路径会先根据新表执行queue setup/restrictions，再发布disk，之后完成inactive map staging。create本身建立tableless identity，不等于应立即发布primary。

**差异与影响：**这是首次TABLE_LOAD过程中的可见中间状态，不是“create后应该有节点”的问题。需要把primary发布所需状态和首表提交组织为一致的事务边界，并明确失败回滚。

### 8.3 P03：首次发布时queue metadata来源不足——Medium

当前`DmDevice::metadata()`只读取active table；只有首张inactive table时会返回默认metadata。Linux在首次发布前使用待加载表建立queue limits和restrictions。

Asterinas当前`BlockDeviceMeta`主要表达容量和segment数量，不等价于Linux完整queue能力，例如discard、write-zeroes、poll、rotational、cache/FUA等尚未全部表达。

**裁决：**先列Medium。若证明默认segment/queue限制会让上层提交backing device不能接受的BIO，则该具体数据安全问题应升High。

### 8.4 P04：ABI 4.50能力声明过度——Medium

指定openEuler基线报告DM ioctl ABI 4.48.0；当前Asterinas报告4.50.0，但仍缺少4.48已经存在的行为，例如ARM_POLL以及部分list/status契约。

版本号本身不是唯一兼容证据。可选方向：

1. 暂时报告与实际能力及选定语义基线相符的版本；
2. 逐项核实4.49/4.50新增UAPI并实现后继续报告4.50。

无论选择哪种，降低版本号都不能替代修复4.48基线内已经存在的协议缺口。

## 9. 合并实施关系

为避免重复修改和重复计数，建议按以下根因组织设计，但每条可观察行为仍需独立验收：

1. **输入—执行—输出—释放生命周期：**A02、A03、S01、S02。
2. **flags和查询编码：**A04、A07、A08、A09、A10。
3. **selector行为与注释：**A05、D02。
4. **name-list布局与注释：**A07、D03。
5. **suspend事务：**L01、L02、F01；其中I/O drain、信号回滚和文件系统freeze是不同保证，不能修复一项后宣称三项全部完成。
6. **事件责任：**L06、M02、P01；必须区分device-local event、control global event和kobject/udev通知。
7. **首次表发布：**P02、P03；首表提交、primary publication和queue metadata应共享事务边界。

## 10. 验证要求

本复核没有执行动态验证。后续实施后至少应按影响范围覆盖：

### 10.1 ABI C回归

- 305/311/312字节保护页；
- NO_PARAMS命令不读取无关尾部；
- 输入容量与输出`data_size`分离；
- 普通handler失败不整包copyout；
- selector组合及精确errno；
- flags输入/输出矩阵；
- name-list二进制布局、prefix filter、空列表和短buffer；
- WAIT active/inactive、table/status模式、短buffer和remove交错；
- secure-data用户区清理与copy fault。

### 10.2 ktest与故障注入

- 部分copyin、handler失败及copyout失败后的内核buffer清理；
- noflush仍达到旧BIO静止点；
- suspend被信号中断后的phase、admission、postponed BIO和inactive table回滚；
- rename/UUID/name校验失败后所有索引和alias不变；
- REMOVE_ALL成功删除后的重扫；
- 首表发布前后inactive table和metadata可见性。

### 10.3 control poll与系统链路

- control fd的per-open global-event baseline；
- ARM_POLL重新武装；
- poll/epoll无事件时不忙轮询；
- 分清device WAIT、control poll、kobject uevent和udev/cookie各自的测试入口。

测试应串行执行。core ktest应从目标crate无selector运行并当场保存串口计数；零匹配不算通过。若测试出现环境阻塞、未预期warning或失败，应停止扩大验证范围并先报告。

## 11. 最终问题清单

按当前静态证据，建议将V5整理为：

- **删除现存缺陷：**A01、A11。
- **改为已修复待验证：**L03、L04、D04。
- **合并：**两条A08；D02→A05；D03→A07。
- **明确保留：**A02、A03、A04、A05、A08、S01、S02、L01、L02、L05、L06、L07、L08、F01。
- **收窄或降级后保留：**A06、A07、A09、A10、A12、M01、M02、M03、D01、B01、B03。
- **拆分：**B02。
- **新增：**P01、P02、P03、P04。

以上裁决是当前源码的静态事实复核，不代表对应缺陷已经修复，也不代表当前dirty工作区已经通过新的完整验证。
