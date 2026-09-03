# Device Mapper 第一版优化文档

本文只记录第一版优化做了什么、每项怎么验证，以及后续优化优先级。当天命令流水和完整输出见 `log/2026-9-2.md`。

## 1. 第一版优化摘要

第一版优化当前完成了七类工作：

| 类型 | 已优化内容 | 结果 |
|---|---|---|
| 文档事实 | 修正过期接手信息、旧 ktest 路径、patches 待确认口径。 | 文档状态与当前实现一致，避免继续误判跨 target BIO split 和 patches 状态。 |
| 源码注释 | 补强 DM control/table/target 相关文件头、结构体和关键函数/API 英文注释。 | review 时能直接看出 ioctl ABI 边界、table/BIO 拆分职责、target 参数解析与映射语义。 |
| 系统测试脚本 | 统一 DM 系统测试的 QEMU 生命周期超时变量、成功 marker 和失败诊断信息。 | 后续跑系统 suite 时，更容易看出是 guest 启动、QEMU 超时、marker 缺失还是测试逻辑失败。 |
| target 元数据 | 把 target 名称和版本从 ioctl 层本地硬编码表移到 target 模块统一维护。 | `dmsetup targets`、`dmsetup target-version` 和 table/status target type 共享同一份 target 元数据。 |
| target parser | 把四种 target 的类型分派和参数解析从 ioctl 层收敛到 target 模块，并取消 striped 参数字符串重建和二次解析。 | ioctl 层只处理 ABI 与 backing 环境解析；target 模块统一负责参数语义和 target 构造。 |
| target framework | 将 `DmTarget` 从 closed enum 迁移为 `dyn DmTarget` trait object。 | `DmTable` 和 ioctl 控制面不再依赖 enum variant 分派；新增 target 仍需 factory 分支，但不需要扩散修改 table/status/deps/I/O match。 |
| ktest 验证入口 | 新增 crate-local `myshell/ktest_crate.sh` 并改造 ktest runner 结果输出。 | 定向 ktest 不再临时修改 `default-members`；结果日志固定为 `<crate-dir>/ktest.log`，负测 `ERROR:` 不再污染结果行。 |

说明：之前做过一个 `target_at` helper 清理，只是维护性清理，不算第一版核心优化成果。

## 2. 优化细节

本节按改动归属拆分。文档类只保留结论；生产代码和脚本/测试类保留问题、方向、改动和验证，方便后续 review 时判断影响面。

### 2.1 文档类

文档类优化主要用于修正接手入口、测试说明和 patches 验证口径，不作为第一版的核心代码优化成果展开。

| 优化项 | 结论 | 验证 |
|---|---|---|
| P0.1 接手入口更新 | 更新 `log/device-mapper-progress.md` 的当前状态、接手阅读清单和 2026-09-02 阶段索引。 | `git diff --check` 通过；旧日期入口检查无残留。 |
| P0.2 ktest 路径修正 | 先将 `docs/test.md` 的 ioctl ktest 示例从旧 `kernel` 路径修正为 `kernel/core`，后续又收敛到 `myshell/ktest_crate.sh <crate-dir> [filter]`。 | 当前定向 ktest 不再依赖临时修改根 `Cargo.toml`；`aster-core` 和 `aster-device-mapper` ktest 均已通过。 |
| P0.3 BIO split 状态修正 | 修正进度说明和阶段交接 memory，不再把普通跨 target BIO split 列为待开发项。 | 旧限制表述检查无残留；相关 `aster-device-mapper` ktest 通过。 |
| P0.4 patches 验证口径修正 | 区分 patch 生成、verify-tree 和代码语义验证，避免把 numbered patches 树一致性误写成测试覆盖。 | `patches/generate_patches.sh` 通过，`patch_count=96`、`empty_count=0`；`apply_patches.sh --verify-tree` 确认 patched tree 与当前树一致。 |

### 2.2 生产代码：DM 核心组件

该类改动集中在 `kernel/core/comps/device-mapper/`，主要优化 target 元数据、target parser、target 几何校验、table/BIO 映射逻辑和 DM 核心源码注释。

| 优化项 | 当前问题 | 优化方向 | 优化了什么 | 验证 |
|---|---|---|---|---|
| P0.5-DM 源码注释补强 | DM 核心组件注释偏少，table、target、字段/variant 和关键映射 API 的语义边界不够直观。 | 按长期规则补齐 DM 核心文件头、结构体、公开 API 和核心私有函数英文注释。 | 为 `table.rs` 补 table/BIO split 文件头、`DmTable` 字段、mapped BIO variant、flush aggregation 字段和核心方法语义；为 `target/mod.rs`、`target/linear.rs`、`target/striped.rs`、`target/error.rs`、`target/zero.rs` 补 target 文件头、metadata/variant/字段、参数解析、几何校验和映射 API 注释。 | `cargo fmt --all --check` 通过；`git diff --check` 通过；DM 相关源码中文注释检查无输出。 |
| P2.1 target 版本元数据集中 | target 名称和版本由 ioctl 层的 `TARGET_VERSIONS` 硬编码维护，新增 target 时容易漏改。 | 由 target 模块统一声明受支持 target 及其版本。 | 新增 `DmTargetMetadata` 和 `SUPPORTED_TARGETS`，删除 ioctl 层的 `TargetVersion` 与 `TARGET_VERSIONS`。 | `cargo fmt --all --check` 通过；`aster-device-mapper` ktest 65 项通过；`aster-core` ktest 通过。 |
| P2.3 table/status target 名称复用 | table/status 输出单独按 `DmTarget` variant 匹配 target 名称，与版本表形成重复硬编码。 | target 实例通过统一接口返回自身名称。 | 新增 `DmTarget::metadata()` 和 `DmTarget::name()`，table/status target type 改用 `target.name()`。 | `aster-device-mapper` ktest 65 项通过；`aster-core` ktest 通过；`--dmsetup-cli` 系统测试通过。 |
| P3.1 target parser 统一入口 | `table_load` 在 ioctl 层按四种 target 名称分支解析和构造，新增 target 时必须修改 ioctl 实现。 | target 模块统一负责类型分派、参数语义和构造入口。 | 新增 `parse_target_with()` 和 `DmTargetParseError`，集中处理 `error`、`zero`、`linear`、`striped`，并返回 `DmTargetBox`。 | `aster-device-mapper` ktest 69 项通过；`aster-core` ktest 186 项通过。 |
| P3.2 backing 两阶段解析 | target 参数需要解析 `/dev/...` 或 `major:minor`，但 target 组件不能反向依赖 ioctl 层 VFS；直接返回 lease 还会在单个 target 的后续 backing 未验证时提前占用设备。 | 分开执行 backing token 解析和 lease 获取，在单个 target 内先验证参数形状、纯几何和全部 backing token，再获取该 target 的 lease。 | `parse_target_with()` 分别注入 `token -> DeviceId` 和 `DeviceId -> BlockDeviceLease`；target parser 保持 VFS/registry 无关，同时保证 striped 全部 token 校验完成后才获取 lease。 | parser ktest 验证 striped 全部 token 先于 lease 获取；设备号边界 ioctl ktest 通过；`--dmsetup-cli` 通过。 |
| P3.3 linear 参数与几何校验下沉 | `linear` 字段解析在 ioctl 层，且 backing 查找可能早于零长度、逻辑范围和 backing range 溢出检查。 | 将 linear 参数和纯几何作为 target 语义，并在环境 lookup 前拒绝确定无效的表。 | 删除 ioctl 层 `parse_linear_params`；target parser 检查两个字段和 offset，调用 `LinearTarget::validate_geometry()` 后才解析设备和获取 lease。 | 参数、几何错误优先级 ktest 通过；现有 ioctl 负向 table-load ktest 通过。 |
| P3.4 striped 单次结构化解析 | ioctl 先把 backing 路径转换为 `major:minor` 并重建字符串，target 层随后再次拆分；无效后续 backing 或确定溢出的 backing range 还可能让前面 lease 被提前持有。 | 一次解析 count、chunk size、backing token 和 offset；完成几何、backing range 纯算术及全部 token 校验后再获取 lease。 | 新增共享 `parse_striped_fields`、`validate_striped_geometry` 和 backing range 算术预检；删除 `normalize_striped_params`；resolved 路径完成全部无环境校验并生成完整 `DeviceId` 列表后，再按顺序获取 lease。 | ktest 验证 backing range 溢出不调用 token parser 或 lease resolver；striped 参数、构造和解析顺序测试通过；`--dmsetup-cli` GAP 为 0。 |
| M1 BIO split 查找条件封装 | `bio_parts` 主循环直接展开 target 范围判断，主流程可读性较差，但没有降低遍历复杂度。 | 只做维护性封装，不把它视为搜索效率优化。 | 新增 `target_at(sector)` 封装单次 target 查找。 | `aster-device-mapper` ktest 65 项通过；`--linear-data` 系统测试通过。 |

### 2.3 生产代码：Asterinas 集成层

该类改动集中在 `kernel/core/src/device/misc/device_mapper.rs` 这类内核集成位置，职责是 Linux DM ioctl ABI、`/dev/mapper/control`、VFS/backing token 解析、block registry lease 获取和 DM table 安装。

| 优化项 | 当前问题 | 优化方向 | 优化了什么 | 验证 |
|---|---|---|---|---|
| P0.5-集成层源码注释补强 | DM control 文件承担 ioctl ABI、设备生命周期和 backing 环境解析，但结构体、入口函数和 ABI helper 注释不足。 | 让 review 能直接区分 Linux ABI 处理、Asterinas VFS/block 集成和 DM target 语义委托边界。 | 为 `device_mapper.rs` 扩充 ioctl control plane 文件头，并补 control device 字段、ioctl 入口、命令分派、table-load、target-version、backing 解析和 ABI helper 注释。 | `cargo fmt --all --check` 通过；`git diff --check` 通过；DM 相关源码中文注释检查无输出。 |
| P2.2 target-version 查询复用元数据 | `DM_LIST_VERSIONS` 和 `DM_GET_TARGET_VERSION` 依赖 ioctl 层本地版本表，不能复用 target 自身定义。 | ioctl ABI 输出从 DM 核心的统一 target 元数据集合派生。 | `list_versions`、`get_target_version`、记录长度计算和记录写入改为消费 `SUPPORTED_TARGETS`。 | `aster-core` ioctl ktest 通过；`--dmsetup-cli` 系统测试通过，GAP 为 0。 |
| P3.5 table_load 职责收敛 | ioctl 主循环混合 ABI 布局解析、target 参数解析、backing 查找和具体 target 构造。 | ioctl 只保留 Linux ABI、VFS/registry 环境解析和 table 安装。 | 四种 target 构造大分支改为一次 `parse_target_with()` 调用，成功后再统一安装 inactive table。 | `aster-core` ktest 186 项通过；`--dmsetup-cli` 系统测试 guest/host marker 均通过。 |
| P3.6 target trait object 化 | `DmTarget` enum 形成 closed target set，table/status/deps/I/O 路径需要按 variant 做薄分派，新增 target 时改动容易扩散。 | 让 table/control-plane 面向统一 target trait；具体 target 只在各自文件实现语义。 | 删除 enum variant 分派，新增 `pub trait DmTarget` 与 `DmTargetBox`；`DmTable` 持有 `Vec<DmTargetBox>`；`error`、`zero`、`linear`、`striped` 分别实现 trait；ktest-only concrete 断言通过 `downcast_ref` 保留。 | `aster-device-mapper` ktest 69 项通过；`aster-core` ktest 186 项通过；`--quick` 系统验收通过。 |

### 2.4 脚本与测试类

该类改动集中在 DM 系统测试脚本、测试入口 help、host 诊断输出和测试文档中直接对应 suite/marker 的部分。

| 优化项 | 当前问题 | 优化方向 | 优化了什么 | 验证 |
|---|---|---|---|---|
| P1.1 QEMU 超时变量统一 | 部分脚本使用 `GUEST_READY_TIMEOUT` 表示整个 QEMU 生命周期超时，变量名称与实际语义不一致。 | 用变量名明确区分完整 QEMU 生命周期和 guest ready 阶段。 | 统一以 `GUEST_QEMU_TIMEOUT` 作为主变量，保留 `GUEST_READY_TIMEOUT` 兼容 alias。 | 相关 shell 脚本 `bash -n` 通过；`--quick` 系统测试通过。 |
| P1.2 Host 超时诊断统一 | 测试启动后无法仅从 host 输出确认当前 suite 使用的 QEMU 生命周期超时值。 | 在每个底层测试入口输出统一的运行参数诊断。 | linear、striped、mixed、dmsetup、LVM2 和 dataplane 脚本统一输出 `HOST_INFO_* qemu_lifecycle_timeout=...s`。 | `HOST_INFO_*` 覆盖检查通过；`--quick` 系统测试通过。 |
| P1.3 成功 marker 自说明 | 多个脚本只有运行后才能知道成功 marker，查看 `--help` 无法确认验收标准。 | 让每个测试入口直接声明其成功条件。 | 在相关脚本 help 中增加 `Expected success markers`，列出 guest、host 和 GAP marker。 | marker grep 覆盖 wrapper、dmsetup、LVM2、dataplane、linear、striped 和 mixed 脚本。 |
| P1.4 suite 与 marker 文档对齐 | `docs/test.md` 未完整说明 `--quick` 等 suite 应出现的 host marker，文档与脚本验收口径分散。 | 在测试文档中给出 suite 到成功 marker 的对应关系。 | 补充 `--quick` 的 control ABI、跨 target BIO、striped raw BIO 和 wrapper 成功 marker。 | 文档 marker 与脚本实际输出逐项对照通过；`--quick` 系统测试通过。 |
| P1.5 crate-local ktest wrapper | 定向 ktest 依赖根目录 `make ktest` 或手写 OSDK 参数，容易漏掉 KVM/initramfs/release；根目录 QEMU 日志也混入启动噪声。 | 用统一 wrapper 进入目标 crate，并只保留 crate-local ktest 结果日志。 | 新增 `myshell/ktest_crate.sh <crate-dir> [filter]`，统一补齐 release、boot、KVM、initramfs、console 和 timeout 参数；结果写入 `<crate-dir>/ktest.log`，QEMU 原始日志提取后删除。 | `bash -n` 通过；`aster-device-mapper` ktest 69 项通过；`aster-core` ktest 186 项通过。 |
| P1.6 ktest runner 结果行拆分 | 负测会打印预期 `ERROR:`，旧 runner 把测试开始行和最终 `ok/FAILED` 分两次拼在同一行，导致错误日志夹在 `test ... ok` 中间。 | 测试开始行和最终结果行分别完整打印。 | `osdk-test-kernel` runner 先打印 `test ...` 开始行换行，测试结束后再打印完整 `test ... ok/FAILED` 结果行；`ktest.log` 只保留最终结果行。 | `propagates_flush_completion_failure` 等负测在 `ktest.log` 中显示为独立 `... ok` 行；DM crate 全量 ktest 通过。 |

## 3. 后续优化优先级

| 优先级 | 优化方向 | 当前问题 | 下一步 |
|---|---|---|---|
| 1 | data path target 查找效率 | table 已保证 target 连续有序，但 BIO split 每个 part 仍可能从头扫描 target。 | 起点定位一次，后续按 target index 顺序推进，避免 `O(parts * targets)`。 |
| 2 | ioctl command / flag spec 表驱动 | command number、decode、handler、flag validation、wait lock 特例分散在多个 match/if。 | 建立 command spec，集中描述 handler、允许/拒绝 flags、锁策略。 |
| 3 | 生命周期语义集中 | active/inactive、suspend/resume、wait/event、rename/remove 的 event 规则分散在 device 层和 ioctl 层。 | 建立 lifecycle API，明确哪些操作 bump event、wake waiters、切换 table。 |
| 4 | target I/O 行为内聚 | remap、direct-complete、IoError、zero-fill 分散在 table 层和 target 层。 | 让 target 返回统一 I/O action，table 只负责 split、submit 和 completion aggregation。 |
| 5 | range validation / 错误分类集中 | 各 target 重复做 length、overflow、range 校验，parse/load 错误映射仍较粗。 | 抽公共 range 校验和更细的 parse/load error，再统一映射 errno/message。 |
| 6 | 系统测试 suite manifest | wrapper help、usage、case dispatch、marker 和 docs 表格重复维护。 | 用 suite manifest 驱动 dispatch/help/marker，减少漏改。 |
