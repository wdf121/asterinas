# Asterinas Device Mapper 技术设计 DOCX 评审

> 评审日期：2026-09-17
>
> 评审对象：`docs/Asterinas_DeviceMapper_技术设计与实现说明.docx`
>
> 评审方式：检查 DOCX 的 OOXML 包结构、正文、样式、编号、目录域、表格、图片、公式、外部关系、页眉页脚和分页设置；当前环境没有 Word、WPS、LibreOffice 或 PDF 渲染工具，本轮未进行逐页视觉检查

## 1. 执行结论

该 DOCX 的技术内容主体与 `docs/device-mapper-technical-design-and-implementation.md` 基本一致，图片、表格、公式、页眉页脚等对象也已实际写入 Word 结构。但是，当前排版不能判定为符合预期，仍存在三项明确的格式问题：

1. 正文大量使用 Word 编号或项目符号，编号范围明显超过真正需要表达顺序的步骤和列表，破坏了连续技术正文的阅读结构；
2. 69 处文件和源码路径被制作成外部超链接，但本项目只要求路径文本本身正确，不需要创建可点击链接；
3. 标题层级不规范：“目录”使用 `Heading1`，正文一级章节从 `Heading2` 开始，可能导致 Word 导航层级偏移，并使自动目录收录“目录”自身。

| 严重程度 | 数量 | 结论 |
|---|---:|---|
| P0 | 0 | 未发现 DOCX 包损坏或正文主体缺失 |
| P1 | 0 | 未发现导致技术内容不可用的事实级问题 |
| P2 | 3 | 正文编号、外部超链接和标题/目录层级需要修正 |

总体判定：**技术内容基本可用，但当前 Word 格式尚未达到定稿要求。** 修正正文编号、移除外部超链接、调整标题层级并在 Word/WPS 中刷新目录后，才适合作为最终正式版本。

## 2. P2：正文编号使用过度

文档包含 115 个真实 Word 列表段落，其中：

- 63 个编号列表段落；
- 52 个项目符号列表段落。

这些段落确实使用了 Word numbering 结构，而不是残留的 Markdown `1.` 或 `-` 文本；但“使用了正式 Word 编号”不等于“编号位置合理”。当前正文大量呈现为带编号或项目符号的段落，使普通解释性内容也呈现为列表，版式不符合连续技术设计文档的阅读习惯。

### 期望格式

- 章节标题保留 `1`、`1.1`、`1.1.1` 等章节编号；
- 有严格先后关系的操作步骤、事务顺序和流程可以使用编号列表；
- 无顺序的少量并列事项可以使用项目符号；
- 背景、设计说明、约束、原因、结论和普通解释应使用无编号正文；
- 图注、表格说明、公式、代码和路径不应继承正文列表编号。

修正时不能简单删除全部列表。应按段落语义逐项判断，仅保留真正的步骤和并列清单。

## 3. P2：文件路径不应制作成外部超链接

DOCX 中检测到 69 个 hyperlink，主要指向项目内 Markdown 文档、源码文件、目录和带行号的相对路径。

本项目对这些引用的要求是：

- 文件路径文本正确；
- 文件名、目录和必要的行号信息正确；
- 读者可以复制或搜索路径定位源码。

不要求路径在 Word 中可点击，也不需要依赖 Word/WPS 对本地相对路径和安全策略的处理。因此，这些路径应保留为普通文本或等宽样式文本，并移除：

- `w:hyperlink` 包装；
- 对应的 external hyperlink relationships；
- 超链接颜色和下划线等交互样式。

移除链接时必须保留原始显示文本，不应删除或改写路径本身。

## 4. P2：标题层级和自动目录结构不规范

当前标题样式结构为：

- 文档名称：`Title`；
- “目录”：`Heading1`；
- “执行摘要”、第 1～15 章和附录：`Heading2`；
- 章节下一级：`Heading3`；
- 更细一级：`Heading4`。

这会造成两个问题：

1. 正文一级章节在 Word 导航窗格和文档语义中被错误地视为二级标题；
2. 自动目录域为 `TOC \\o "1-3" \\h \\z \\u`，“目录”自身使用 `Heading1`，刷新目录后可能被收录进目录。

### 期望格式

- 文档名称继续使用 `Title`；
- “目录”使用独立的目录标题样式或不进入 TOC 的普通标题样式；
- “执行摘要”、正文各章和附录使用 `Heading1`；
- 下一层依次使用 `Heading2`、`Heading3`；
- 调整样式后，在 Word/WPS 中更新整个目录并检查层级与页码。

当前只能确认 TOC 域存在，不能确认目录已经刷新，也不能证明最终页码和层级正确。

## 5. 内容与对象检查

### 5.1 技术内容

DOCX 已包含最终 Markdown 中的主要内容：

- 执行摘要与阅读导览；
- 用户动作到内核对象的动作地图；
- DM 专属实现与通用内核框架边界；
- ioctl 命令语义矩阵；
- linear 和 striped 数学模型；
- 两级 BIO 边界规划与完成聚合；
- lease、open count 和 in-flight 生命周期；
- Linux 能力与验证证据矩阵；
- 新 target 接入模板；
- 附录与评审清单。

此前 Markdown 评审指出的 name、UUID、minor 一致性问题，已在 DOCX 中限定为 rename/remove 事务之外的稳定提交态。没有发现 Mermaid 源码或 Markdown 分隔符作为普通正文残留。

### 5.2 图片

文档包含：

- 9 个图片资源；
- 9 个正文 drawing；
- 9 个图注。

图片与架构边界、核心对象、ioctl 工作流、首次 table 发布、mapper phase、primary 注册事务、BIO 分派、split completion 和 mount lease 等主题对应。

静态结构检查不能确认图片在最终页面中的缩放比例、文字清晰度和跨页位置。

### 5.3 表格

文档包含 30 个表格。OOXML 结构显示：

- 表格具有边框；
- 具有 grid 列宽；
- 首行设置为重复表头；
- 表格行设置为禁止拆分。

这些结构设置存在，但尚未通过页面渲染确认五列表格是否过密、是否超出版心或是否产生不自然分页。

### 5.4 公式

文档包含 3 个 `Equation` 样式段落及对应 OMML 数学节点，公式不是 Markdown 源码或普通文本占位符。

### 5.5 页眉、页脚与页面设置

静态检查确认：

- 页面尺寸接近 A4；
- 已设置页边距、页眉距和页脚距；
- 页眉包含文档名称；
- 页脚包含 `PAGE` 域，预期显示为“第 N 页”；
- 没有 `NUMPAGES` 域，因此不是“第 N 页 / 共 M 页”；
- 文档包含 19 个显式分页符。

## 6. 验证边界

当前环境没有可用的 Word、WPS、LibreOffice、PDF 转换或逐页截图工具。本轮结论来自 OOXML 静态检查和正文抽取，因此没有验证：

- 自动目录刷新后的实际条目与页码；
- “目录”是否确实被收录进目录；
- 图片的实际清晰度、比例和分页位置；
- 宽表格是否拥挤或超出版心；
- 是否存在孤立标题、孤行或不自然留白；
- 页眉页脚的最终视觉位置；
- Word 与 WPS 之间的页面渲染差异。

这些项目必须在修正格式后，通过 Word/WPS 打开文档、更新全部域并逐页检查。

## 7. 最终判定

| 检查项 | 判定 |
|---|---|
| 技术内容主体 | 基本符合预期 |
| Markdown 内容转换 | 基本完整，未发现 Mermaid 或分隔符残留 |
| 正文编号 | 不符合预期，编号和项目符号使用过度 |
| 文件路径 | 文本应保留，但不应制作成超链接 |
| 标题层级 | 不符合规范，正文层级整体偏移 |
| 自动目录 | 域存在，但有自引用风险且尚未刷新验证 |
| 图片与图注 | 对象和数量基本符合，页面效果待验证 |
| 表格 | 结构属性基本齐全，页面效果待验证 |
| 公式 | 符合 OMML 结构预期 |
| 页眉页脚和页码 | 结构存在，视觉效果待验证 |

最终结论：**该 DOCX 的内容主体基本正确，但格式不符合最终交付预期。当前版本不应直接定稿；至少应先收敛正文编号、移除所有文件路径超链接、修正标题层级，并在 Word/WPS 中刷新目录和完成逐页视觉检查。**

## 8. 非 Device Mapper 框架改动与测试覆盖审查

> 审查日期：2026-09-18
>
> 审查范围：`docs/non-device-mapper-change-rationale.md` 所列的 block/BIO、`aster-core` registry/VFS/procfs、文件系统、后端驱动与启动支撑改动；不重复评审 DM core crate 内部实现。
>
> 证据口径：以 `e31b265a3 → 当前工作区` 的源码与测试基础设施 diff 为主；daily log 仅作交叉来源。区分生产语义、`#[cfg(ktest)]` 测试增量和构建/回归入口改动。

### 8.1 执行结论

当前 diff 中唯一改变 DM 用户可见生产语义的代码是 `DM_DEV_WAIT`：`device_mapper.rs` 直接进入 wait 处理，不再落入通用命令分派的不可达路径；内核将等待中断转换为 restart 语义，最终用户态结果由 signal action 决定：无 `SA_RESTART` 时得到 `EINTR`，带 `SA_RESTART` 时原 ioctl 自动重启并继续等待。该段对应的后续 focused guest 证据见 9.10。`device.rs` 的 major:minor 日志格式变化不改变主链语义。

其余 Rust diff 主要位于 `#[cfg(ktest)]`：block/BIO、table child completion、minor 生命周期和 registry rollback 的覆盖补强，不应写成新的框架生产能力。回归基础设施则改为目标 crate 目录直接运行 `cargo osdk test`、支持 `REGRESSION_TESTS` selector，并让非 TDX regression 不构建 TDX attestation。当前不应扩大为无目标的全量框架测试；exfat lease、MlsDisk 生命周期和后端 fault injection 保持触发式边界。

| 范围 | 当前 diff 结论 | 文档定位 |
|---|---|---|
| DM wait ABI | 直接分派、event wait、restart 中断处理属于生产语义改动 | global/progress/review |
| block/BIO、table、manager、registry | 当前增量主要为 `#[cfg(ktest)]` 覆盖 | review/test，不扩写 global 生产能力 |
| initramfs C 回归 | 覆盖 raw control ABI、node/alias rollback、ext2 mount lease、range ioctl、wait errno、`SA_RESTART` 与 rename/setuuid 唤醒 | test/review |
| 测试入口 | crate-local `cargo osdk test`、`REGRESSION_TESTS`、非 TDX gate | AGENTS/test/progress |
| exfat / MlsDisk / driver fault | 当前未作为主线生产修复 | 保持触发式边界 |

### 8.2 已有直接测试证据

#### block/BIO

`aster-block` 的当前增量补充 split BIO 多错误乱序完成、partition remap/overflow、request queue range merge/segment 上限等 `#[cfg(ktest)]` 覆盖。它们用于保护内部不变量，不应表述为新增 block 框架生产语义。

该层测试选择正确：上述行为属于 `BlockDevice`/BIO/queue 内部不变量，使用 DM 系统 suite 间接覆盖会降低失败定位能力，也不能完整覆盖 overflow 原子性。

#### registry、VFS 与 DM 控制 ABI

新增的 initramfs `device_mapper.c` 已覆盖：

- primary 或 alias 路径碰撞时，table load/suspend 失败后 mapper 状态与节点回滚；
- 删除碰撞文件后能够重试；
- alias 为指向 primary block node 的 symlink；
- ext2 mount 存活时 `DM_DEV_REMOVE` 返回 `EBUSY`，umount 后可删除；
- `BLKDISCARD`/`BLKZEROOUT` 覆盖合法、未对齐、溢出、越界以及合法/非法零长度 byte range；
- `DM_DEV_WAIT` 被不可重启信号打断时返回 `EINTR`，等待不存在 mapper 返回 `ENXIO`；
- 带 `SA_RESTART` 的 signal handler 确实执行后，原 ioctl 在事件前不提前返回，并在 rename 后自动重启、成功返回和回填 header；
- fork waiter 由 rename 或 setuuid 成功唤醒，并回填新的 header/event。

这类验证必须从用户态经由 `open`/`ioctl`/`mount` 进入内核；ktest 无法单独证明 ioctl buffer ABI、VFS 节点状态与 errno 的最终组合语义。

#### procfs

新增的 `/proc/devices` C 回归以短读方式验证 offset 续读，并检查 `virtblk` 与 `device-mapper` 条目唯一、major 非零且格式可解析。该测试直接保护用户态设备发现接口，不能由 DM NixOS suite 替代。

### 8.3 C 回归实现与实际执行证据

本轮对 `test/initramfs/src/regression/device/device_mapper.c` 的实际改动为：

- 用 raw `dm_ioctl` 构造 linear table；预建 `/dev/dm-N` 或 `/dev/mapper/<name>` 路径，以 `EEXIST` 验证 primary/alias 发布失败后 inactive table、alias 和重试语义；成功 resume 后以 `lstat/stat` 验证 alias 与 primary 的设备 identity；
- 映射完整 512MiB ext2 backing，验证 mount 存活时 `DM_DEV_REMOVE` 为 `EBUSY`，umount 后 remove 成功；
- 对 zero target 执行 `BLKDISCARD`/`BLKZEROOUT`，验证合法、未对齐、溢出、越界和零长度 byte range 的 ioctl/errno 边界；
- 用 `SIGALRM` 中断当前 event 的 `DM_DEV_WAIT`，验证无 `SA_RESTART` 时用户态得到 `EINTR`，并验证未知 mapper 的 `ENXIO`；
- 通过 ready、signal 与 result pipe 验证带 `SA_RESTART` 的 handler 已执行、事件前 ioctl 未返回，并在 rename 后由同一 ioctl 成功返回；
- fork waiter 由 rename 或 setuuid 成功唤醒，并回填新的 header/event；
- 修正既有 tableless 回归对 `DM_EXISTS_FLAG` 的位掩码比较：应判断非零而不是等于 1。

完整 initramfs regression 仍可能被无关 network/VT 子回归提前阻断，因此新增 `REGRESSION_TESTS`：空值保持全量；`device` 运行目录脚本；`device/device_mapper` 只运行该已打包二进制，且 runner 拒绝绝对路径和路径穿越。非 TDX 构建通过 `enableTdxAttest` 跳过运行时本会跳过的 TDX attestation package，避免 DCAP GitHub 下载。

实际命令与结果：

```bash
NIX_CONFIG="substitute = false" RELEASE=1 AUTO_TEST=regression \
  INTEL_TDX=0 REGRESSION_TESTS=device/device_mapper make run_kernel
```

focused 命令使用 `REGRESSION_TESTS=device/device_mapper` 选择单一已打包 C ELF；该回归覆盖 raw control ABI、linear table load、node/alias rollback、ext2 lease、range ioctl、无 `SA_RESTART` 的 `EINTR` 对照、真实 `SA_RESTART` 自动重启与 rename/setuuid 唤醒。对应 core wait/registry 模块测试从 `kernel/core` 使用 `cargo osdk test --kcmd-args=earlycon <module>::tests`；DM component 测试则从目标 crate 目录运行，不应混用入口。

### 8.4 必要补强与不应伪造的覆盖

#### exfat lease 生命周期

ext2 与 exfat 均修改为持有 `BlockDeviceLease`，但当前用户态回归只验证 ext2。exfat 等价场景作为框架边界暂停：只有 DM 文件系统路径出现可见失败、数据风险或后续 target 明确依赖时，才恢复以下回归：

```text
exfat mount → DM_DEV_REMOVE 返回 EBUSY → umount → DM_DEV_REMOVE 成功
```

此项验证文件系统对象、VFS mount 生命周期、lease 与 registry remove 的端到端拼接，不宜仅以 core ktest 代替。

#### MlsDisk tracked lease

探索性 ktest 已表明 MlsDisk facade drop 后 backing unregister 仍返回 Busy。该结果说明当前问题不在测试缺失，而在 production 释放语义：MlsDisk drop 仅改变状态标志，未释放 RawDisk 持有的 tracked backing lease。

按当前范围，记录该缺口及其所有权关系即可；不修改 production 释放语义，也不保留会失败的探索性 ktest。若未来重新进入该主题，应先明确 backing lease 的预期所有权与 release 时机，再设计修复与回归。

#### 后端故障路径

VirtIO 的正常数据路径已有 DM NixOS suite 证据。NVMe 对未支持 range I/O 的错误 completion 当前没有可信的 test-only transport/CQ 注入点；在没有可重复故障注入基础时，不应为了覆盖率编写脱离真实驱动路径的伪测试。

### 8.5 文档一致性状态

本轮已同步以下入口：

- 已删除的 maintenance Markdown、旧 DOCX、`kernel/comps/README.md` 和旧 wrapper 不再作为当前资料入口；
- 构建/NixOS 用户工具职责以当前配置、overlay、`tools/nixos/run.sh` 与 `tools/qemu_args.sh` 的实际分工描述；
- 当前通用分层与 selector 说明以 `docs/test.md` 为准；core ktest 从 `kernel/core` 运行并带 `--kcmd-args=earlycon`，DM component ktest 从目标 crate 目录运行。六个 canonical NixOS suite、超时、release 与串行边界以 `AGENTS.md`、`log/device-mapper-progress.md` 和实际脚本为准。
- 2026-09-22 的系统验收已串行通过 control-plane、dataplane、LVM2 topology、linear integration、striped integration 和 mixed integration。LVM2 topology 中 `lvs` 请求 `segtype` 对跨 PV 的双 linear segment 产生重复汇总行，已将该条断言改为 LV 汇总的 `seg_count=2`；DM table 与 dependency 的两 backing 验证保留，复跑通过。

后续文档同步必须先比较 `e31b265a3 → 当前工作区` 的实际 diff，不从固定日期 daily log 推导当前状态。

### 8.6 最终判定

| 项目 | 判定 |
|---|---|
| `DM_DEV_WAIT` direct dispatch | 当前生产语义改动；应作为 DM control ABI 主链维护。 |
| block / table / manager / registry ktest 增量 | `#[cfg(ktest)]` 覆盖补强，不表述为新增框架生产能力。 |
| initramfs C 回归 | 覆盖 raw control ABI、linear table load、node/alias rollback、ext2 lease、range ioctl、WAIT errno、`SA_RESTART` 和 rename/setuuid waiter 唤醒。 |
| 非 TDX regression 构建图门控 | `INTEL_TDX=0` 不构建无关 TDX attestation。 |
| crate-local/core ktest 与 regression selector | 当前测试入口；分层和 selector 见 test，canonical DM suite 边界见 AGENTS/progress 与实际脚本。 |
| exfat lease / MlsDisk / driver fault | 触发式框架边界；当前不主动扩展。 |

最终结论：**提交基线之后的主链生产语义改动集中于 DM wait dispatch 与中断处理。其余 Rust 增量主要是测试覆盖，构建/脚本增量主要服务于定向回归与非 TDX 构建图。文档应按当前入口同步，不将历史 wrapper、已删除 suite 或旧 maintenance 文档继续作为现行资料。**

## 9. Device Mapper 第四版优化文档审核

> 审核日期：2026-09-21
>
> 审核对象：[log/device-mapper-optimization-v4.md](../log/device-mapper-optimization-v4.md)
>
> 审核方式：本节 9.1～9.9 保留 2026-09-21 首轮只读审核的历史结论；当时未修改生产代码、未构建或运行测试。后续修订、实施与动态验证结果见 9.10，不倒改首轮审核事实。

### 9.1 执行结论

第四版文档“从分支行为差异反查直接断言，并区分 ktest、用户 ABI、系统测试和间接触达”的总体方向正确，git 基线、193 项 ktest 统计以及多数生产行为描述可以由当前代码和既有运行记录支持。但文档目前还不能直接作为完整实施与验收清单，存在 1 项 P1 和 5 项 P2：

| 严重程度 | 数量 | 结论 |
|---|---:|---|
| P0 | 0 | 未发现会立即导致文档整体不可用的问题。 |
| P1 | 1 | VirtIO GET_ID 将不同层次的异常错误合并成并不存在的统一容错契约，可能导向错误验收或生产语义改动。 |
| P2 | 5 | 已有 tracked lease 断言分类不准，并遗漏 WAIT、真实节点恢复和 MlsDisk 边界；部分计划还与当前触发式实施范围冲突。 |

总体判定：**文档框架可保留，但应先修正 GET_ID 契约，重新分类已有 lease 断言，并补齐 WAIT、真实 runtime 恢复及 MlsDisk/范围决策，之后再开始 V4 补测。**

### 9.2 P1：GET_ID 异常被写成并不存在的统一“继续初始化”契约

文档在 [V4 第 158 行](../log/device-mapper-optimization-v4.md#L158)要求 status、token、used_len 异常时“不保存错误 ID，初始化按现有容错契约继续”。该结论只适用于已经被队列接受并返回给调用者、且 `query_host_id` 能够解析的一部分异常，不适用于队列层过滤的非法完成项或设备不完成。

证据如下：

- [`query_host_id`](../kernel/core/comps/virtio/src/device/block/device.rs#L348-L375)循环调用 `pop_used_with_min_bytes()`，任何错误都只执行 `spin_loop()`，没有退出或超时分支；
- [virtqueue used-ring 处理](../kernel/core/comps/virtio/src/queue.rs#L415-L449)会跳过非法 token 或 used length；没有可返回的合法完成项时返回 `NotReady`；
- [GET_ID 查询](../kernel/core/comps/virtio/src/device/block/device.rs#L298-L305)发生在正常 queue IRQ callback 注册之前，并同步阻塞设备初始化。

具体影响：若按文档把所有异常都制作成“返回 `None` 并继续初始化”的验收用例，局部解析 mock 可以通过，但真实 queue 路径可能永远不结束；若为满足该验收而修改生产代码，则已经超出“补契约测试”的范围。

修订建议：把行为拆为三类：

1. query 层实际可返回的错误响应：按当前实现验证不保存错误 ID；
2. queue 层过滤的非法 token/used length：明确当前会继续等待，不能写成已实现的容错返回；
3. 设备不完成：标为尚未界定的初始化超时/故障策略，若要改变行为应另立生产设计事项。

### 9.3 P2：V4.3 漏认了已存在且纳入 80 项基线的 tracked lease 清理断言

[V4 第 129～135 行](../log/device-mapper-optimization-v4.md#L129-L135)把 inactive clear 和 table 解析/构造部分失败后的 tracked lease 释放整体列为缺口，与文档自己的“已有断言/未找到断言”分类不一致。

当前已有直接证据：

- [`rejects_missing_striped_backing_without_setting_readonly_or_table`](../kernel/core/src/device/misc/device_mapper.rs#L4242-L4269)真实注册第一个 backing，在第二个 backing 缺失时断言请求失败、状态与 readonly 不变，随后成功注销第一个 backing；
- 该用例经过[生产 table parser 与 `lookup_lease`](../kernel/core/src/device/misc/device_mapper.rs#L605-L647)，不是 untracked 解析替身；striped target 会按顺序[取得 backing lease](../kernel/core/comps/device-mapper/src/target/striped.rs#L245-L251)；
- [inactive table clear 用例](../kernel/core/src/device/misc/device_mapper.rs#L3740-L3780)真实注册两个 backing、加载 inactive striped table、清除 inactive table，随后成功注销两个 backing；
- [容量失败](../kernel/core/src/device/misc/device_mapper.rs#L4306-L4338)和[表连续性失败](../kernel/core/src/device/misc/device_mapper.rs#L4370-L4409)后也检查 backing 可注销；
- 这些 core DM ioctl 用例已包含在 [80 项通过记录](../log/daily/2026-9-21.md#L19-L24)中。

具体影响：文档会把已能发现 lease 泄漏的清理用例重新算作“无断言”，安排重复建设，并模糊真正缺失的窄边界。

修订建议：将 V4.3 拆为“已有 tracked 清理断言且已纳入基线”和“仍需新增的保护期/状态断言”。后者应集中于 holder 存活时 `Busy`、换表后的旧 in-flight BIO、已有非空 active/inactive table 的失败保留，以及新旧 backing 保护关系。现有用例不能证明消费者生命周期全部覆盖，但也不能继续整体归类为缺口。

### 9.4 P2：承接 V3 的计划遗漏 DM_DEV_WAIT / SA_RESTART 用户 ABI 缺口

V4 在[范围说明](../log/device-mapper-optimization-v4.md#L9)中明确承接 V3，并把新增成功、错误和并发路径作为目标，但 [V4.1～V4.7](../log/device-mapper-optimization-v4.md#L72-L80)及三批实施安排没有保留 V3 已明确登记的真实用户态 `SA_RESTART` 验收缺口。

证据如下：

- [V3 记录](../log/device-mapper-optimization-v3.md#L430-L436)已说明 `EINTR → ERESTARTSYS` 修复完成，但真实用户态 `SA_RESTART` 回归没有 guest 执行证据，对应用例已删除；
- [生产 WAIT 路径](../kernel/core/src/device/misc/device_mapper.rs#L508-L518)执行 restart 错误转换；
- [现有 ktest](../kernel/core/src/device/misc/device_mapper.rs#L3255-L3264)只验证错误枚举转换，没有经过用户信号现场、寄存器恢复和 syscall 重启；
- [现有 C 用例](../test/initramfs/src/regression/device/device_mapper.c#L319-L326)没有设置 `SA_RESTART`，只断言不可重启信号得到 `EINTR`；
- [`SA_RESTART` 处理](../kernel/core/src/process/signal/mod.rs#L145-L157)还会经过用户寄存器和指令指针恢复分支。

具体影响：即使 V4.1～V4.7 全部完成，现有矩阵仍不能证明 WAIT 遇到可重启信号后会继续等待并最终返回事件，而不是提前返回 `EINTR`。

修订建议：至少把该项显式保留为既有未覆盖边界；若纳入 V4，应分别定义可重启和不可重启信号的用户 ABI 结果，并以真实用户态 syscall 重启链路验收，不能用错误映射 helper 通过代替。

### 9.5 P2：“真实 runtime 节点补偿”目标没有覆盖实际恢复分支

[V4.2 场景表](../log/device-mapper-optimization-v4.md#L113-L119)正确指出 closure 注入不能证明真实 Path/inode 恢复，但所列场景主要覆盖身份删除、目录清理、外来节点以及发布/rename 所有权，没有明确安排“alias 已删除、primary 删除失败后，真实 alias 恢复成功或失败”这两条核心补偿路径。

生产顺序和状态差异如下：

- [runtime remove](../kernel/core/src/device/registry/block.rs#L604-L623)先删除 alias，随后 primary 删除失败时才进入 `recover_primary_removal_failure`；
- [删除辅助逻辑](../kernel/core/src/device/registry/block.rs#L708-L715)把 `ENOENT`/`ESTALE` 当作清理成功，因此单纯替换 alias/primary 通常不会触发补偿；
- [真实恢复成功](../kernel/core/src/device/registry/block.rs#L750-L771)会恢复 live/open 状态，[恢复失败](../kernel/core/src/device/registry/block.rs#L774-L804)则保留 `Removing` 隔离，两者验收结果不同；
- [现有恢复成功/失败用例](../kernel/core/src/device/registry/block.rs#L985-L1061)只注入 closure 结果，没有实际创建并验证恢复后的 alias。

具体影响：执行 V4.2 当前表格内全部场景后，仍可能完全没有调用真实 `restore_mapper_alias()`；恢复链接目标、设备 identity、open gate、隔离和重试语义仍无直接证据，却可能被错误记为 V4.2 完成。

修订建议：单列两项：

1. primary 删除失败后，真实 alias 恢复成功，验证链接目标、identity、registry 状态和 open 行为；
2. 恢复时路径冲突或恢复失败，验证保持 `Removing` 隔离、外来对象不被覆盖且后续可重试。

测试设计应先证明 primary 删除失败分支可达，或采用仍保留真实 VFS 恢复操作的局部故障注入；不能只分析更晚的最终 commit 失败。

### 9.6 P2：RawDisk 补测不能替代已记录的 MlsDisk facade 释放问题

[V4 第 129、139～143 行](../log/device-mapper-optimization-v4.md#L129-L143)把该区域主要表述为 fixture/断言缺失，并把计划限定为 RawDisk/subset/clone，但没有承接本评审已经登记的 MlsDisk facade 释放阻断。

- [现有评审](#84-必要补强与不应伪造的覆盖)记录：探索性 ktest 观察到 facade drop 后 backing 仍为 `Busy`，应先明确 ownership/release 时机，不能只归类为测试缺失；
- [`RawDisk`](../kernel/core/comps/mlsdisk/src/lib.rs#L67-L79)直接持有 lease，并会为 subset/clone [克隆 lease](../kernel/core/comps/mlsdisk/src/lib.rs#L118-L129)，适合独立验证末个 holder 释放；
- [`MlsDisk` facade](../kernel/core/comps/mlsdisk/src/layers/5-disk/mlsdisk.rs#L47-L68)持有 `Arc<DiskInner<D>>`，其 [`Drop`](../kernel/core/comps/mlsdisk/src/layers/5-disk/mlsdisk.rs#L628-L631)只设置 `is_dropped`，不能据此证明内层所有权和 tracked backing 已释放。

具体影响：RawDisk 单元测试全部通过后，真实 MlsDisk 消费者的已知 `Busy` 问题仍可能存在；若把 facade drop 直接纳入“末个 holder 释放后放行”验收，会重新遇到既有失败并误判为新发现。

修订建议：保留两条独立结论：

- RawDisk/subset/clone 的 tracked lease 适配可以补直接测试；
- MlsDisk facade 的所有权释放问题仍未解决，本阶段不能承诺 facade drop 后 backing 可立即注销。

本轮没有重新运行探索性测试，因此这里只引用既有审核证据，不把具体泄漏根因表述为本轮动态确认结果。

### 9.7 P2：exfat、RawDisk 和驱动专项计划与当前触发式边界冲突

V4 把 exfat 生命周期、RawDisk 以及 VirtIO/NVMe mock/受控集成列入[待实施总览](../log/device-mapper-optimization-v4.md#L70-L80)和[第二、三批计划](../log/device-mapper-optimization-v4.md#L223-L225)，但没有说明这是否要替代当前评审和交接文档规定的触发式边界。

现行边界为：

- [本评审第 8.4～8.6 节](#84-必要补强与不应伪造的覆盖)将 exfat、MlsDisk 与后端故障路径保留为触发式范围；
- [交接文档](device-mapper-handoff.md#L39-L56)规定，只有出现 DM 用户可见失败、数据风险或阻塞新 target 时才重新开启这些专项；
- [V4 第 167 行](../log/device-mapper-optimization-v4.md#L167)只要求调整 QEMU feature/设备配置前另行授权，没有解决测试范围决策冲突。

具体影响：后续执行者会同时面对“进入第二/三批实施”和“当前不主动扩展”两个相反入口，可能未经范围确认直接扩围，也可能据旧边界拒绝执行 V4 清单。

修订建议：明确 V4 是否提出范围变更。若要重新开启，应记录替代旧边界所需的确认、动因和退出条件；否则把这些项目改为条件性候选，而不是无条件批次内容。该冲突与 diff 基线选择无关，不能因 V4 使用 `main → 工作区` 就推定旧实施决策自动失效。

### 9.8 已核实且不应误报的内容

以下内容与当前源码或既有执行记录一致，不列为问题：

- `main` 为 `604948581512d83734377974d4c34adb4530f2d7`，HEAD 为 `e31b265a3`；V4 对 `main → 当前工作区` 的基线描述正确；
- 85＋20＋80＋7＋1＝193，且各组通过数可由 [2026-09-20](../log/daily/2026-9-20.md)和[2026-09-21](../log/daily/2026-9-21.md)日志支持；文档没有把历史静默运行或依赖编译误算成测试通过；
- range ABI 对零长度仍先执行对齐、溢出和容量校验，再跳过下发，V4 对该边界的描述正确；
- 四个 range wrapper、NVMe range 拒绝、host-ID ioctl errno、TSC 来源优先级以及 test-kernel 的构建时 `OSDK_LOCAL_DEV` 门槛，整体描述与当前源码一致；
- 禁止 DmDevice backing 的生产分支存在，本轮未找到直接拒绝用例。实施时需要让 inner DmDevice 先具有合法容量，避免在 target 容量校验阶段提前失败；这是 fixture 约束，不单列为文档事实错误；
- 引用路径和绝大多数行号仍有效，没有发现确定的已删除文件链接或指向无关符号的引用。

另有一项需要在后续覆盖矩阵中谨慎表述：现有 WAIT 唤醒测试使用 ready 标记加单次 `sched_yield`，尚不能严格证明 waiter 已经进入阻塞阶段。V4 要求使用可观察阶段屏障的方向正确；在补齐该屏障前，不应把现有用例强化表述为已经证明“阻塞后唤醒”。

### 9.9 最终判定

| 审核项 | 判定 |
|---|---|
| 文档目标与证据分层 | 方向正确，可保留。 |
| git 基线与 193 项统计 | 与当前仓库及日志一致。 |
| GET_ID 异常契约 | 不准确，必须在实施前修正。 |
| tracked lease 缺口分类 | 不准确，应先扣除已有直接断言。 |
| WAIT 用户 ABI | 遗漏 V3 已登记的 `SA_RESTART` 缺口。 |
| runtime 节点补偿 | 未覆盖真实 alias 恢复成功/失败分支。 |
| RawDisk/MlsDisk | 必须区分可补测的 RawDisk 与未解决的 facade 所有权问题。 |
| exfat/驱动等实施范围 | 与现行触发式边界冲突，需明确是否重新授权扩围。 |

最终结论（首轮审核时）：**第四版文档不是无效方案，但当时版本仍存在会误导测试设计和完成判定的关键问题，应先修订上述 6 项再实施。后续状态不得由本段历史结论推断，见 9.10。**

### 9.10 后续修订、实施与验证结果

首轮提出的契约与范围问题已按代码事实修订，并在用户授权范围内完成直接断言与动态验证。原 V4.5（CPUID/TSC）和 V4.6（test-kernel selector）因属于通用平台/测试基础设施、与 DM 功能无直接调用关系，从 V4 方案移除；这是范围收敛，不是测试通过。

| 首轮问题 / 实施项 | 当前状态 | 证据与边界 |
|---|---|---|
| GET_ID 异常契约混写 | **文档已修订** | 已区分 query 可返回异常、queue 过滤异常和设备不完成；未改变 timeout、轮询或降级生产语义。 |
| tracked lease 误分类及缺口 | **核心子项完成** | 新增 table 存活/替换、旧 in-flight BIO、failed table-load 状态保持断言；DM 86/0，core DM ioctl 82/0。mount source 类型仍未实施。 |
| WAIT / `SA_RESTART` | **完成** | C 用例以 signal handler pipe 证明信号送达，事件前结果 pipe 未就绪，rename 后原 ioctl 成功返回并回填；无 `SA_RESTART` 的 `EINTR` 对照保留。 |
| 真实 runtime alias 恢复 | **计划已准确列出，测试未实施** | closure 注入和既有路径碰撞回滚仍不能替代真实 `restore_mapper_alias()` 成功/失败路径。 |
| RawDisk 与 MlsDisk facade 混淆 | **文档已拆分，生产边界未解决** | RawDisk 为条件性候选；不承诺 facade drop 后 backing 立即可注销。 |
| 条件性专项与范围冲突 | **文档冲突已关闭** | exfat、RawDisk、VirtIO/NVMe 保持触发式候选，本轮未执行。 |
| block range wrapper | **完成** | 3 个 ktest 覆盖四个 wrapper 的构造、同步/异步完成、enqueue/completion 错误通道、batch/callback；block 23/0。 |
| raw range ioctl | **完成** | `BLKDISCARD`/`BLKZEROOUT` 覆盖合法、未对齐、溢出、越界和零长度 byte range；不外推真实硬件内容语义。 |
| DmDevice backing 拒绝 | **直接分支完成** | 有合法容量的实际 DmDevice 作为 linear/striped backing 均返回 `UnsupportedBackingDevice`；未实现 stacking。 |

动态结果：

- DM crate 全量：86 passed，0 failed，0 filtered out；
- block crate 全量：23 passed，0 failed，0 filtered out；
- core DM ioctl 模块：82 passed，0 failed，123 filtered out；
- focused `device/device_mapper` C regression：7 个测试函数累计 159 passed，0 failed，最终 `All regression tests passed.`。

其中前三组是本轮重新执行的 191 项 Rust 测试。此前 core registry 7/0 与动态设备路径 1/0 仍是有效历史证据，但本轮未重跑；因此最新跨轮次清单可计为 199 项，不能写成“本轮 199 项全部通过”。C 的 159 是 focused ELF 内断言累计数，不是完整 initramfs regression，也不是 159 个独立测试函数。

**当前判定：**本轮授权子集已完成并通过验证；真实 alias/VFS 深层补偿、mount source 类型和条件性专项仍未完成，不据此宣称整份 V4 覆盖地图全部关闭。
