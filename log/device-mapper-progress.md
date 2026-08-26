# Device Mapper 项目进度

本文记录 `dm` 分支中变化较快的 Device Mapper 项目状态。若本文与当前代码或实际命令结果不一致，以当前代码和命令结果为准，并及时修正本文。

## 当前状态

截至 2026-08-26，`dm` 分支已合入当前 `main`，并完成上游 `kernel/core` 目录迁移适配。当前重点从继续扩慢系统矩阵，转为把 linear / striped LVM2 系统验收脚本分层理清楚：基础 reboot 只负责生成并恢复单 segment 基础产物，resize / 扩容测试应基于基础产物恢复后继续扩容/缩容，避免重复创建基础 LV。

最近相关提交：

```text
41d51bdcb 合入 main 并适配 Device Mapper 目录迁移
0703e3e25 补充 Device Mapper 分支文档和 patch 工具
f53236cd0 新增 Device Mapper mixed LVM2 系统验收
8426e405a 补充 Device Mapper mixed 与 striped 边界 ktest
2e1a3350f 记录 dm 分支协作说明
8f00dec97 补充 Device Mapper mixed table ktest
fd94cd2c7 补充 Device Mapper table-load 非法输入 ktest
49ef85c72 修复 Device Mapper table remap ktest
```

当前 DM 相关路径已迁移到上游新布局：

```text
kernel/core/comps/device-mapper
kernel/core/src/device/misc/device_mapper.rs
kernel/core/src/fs/fs_impls/procfs/devices.rs
```

## 已完成并验证的范围

- Device Mapper linear / striped / mixed 的核心数据面和 ioctl 语义已通过 ktest 与系统脚本分层覆盖。
- 同一 DM device / 同一 LV 内的 BIO 可以跨 target 边界拆分；这不是跨 LV。
- 一个 BIO 不应跨两个不同 LV；BIO 是发给某一个 block device 的。
- `DmTable` 数据面支持：
  - 单段 linear remap。
  - 多段 linear target。
  - striped target 内跨 stripe chunk 拆分。
  - linear + striped mixed target 的跨边界拆分。
  - split child enqueue 失败或 child 完成 `IoError` 时，原 BIO 聚合返回 `IoError`。
- ioctl 控制面支持并已测：
  - `DM_TABLE_LOAD` 多 target linear。
  - `DM_TABLE_LOAD` striped。
  - `DM_TABLE_LOAD` linear + striped mixed table。
  - mixed inactive table 的 `DM_DEV_STATUS.target_count`。
  - mixed table 的 `DM_TABLE_STATUS` type / params / next offset。
  - mixed table 的 `DM_TABLE_DEPS` backing 顺序。
  - running device 中 active linear table 被 ioctl-loaded mixed table 替换后的 active / inactive 查询、resume 切换和 deps 切换。
- table-load 非法输入已补 ktest：
  - linear 参数缺失、额外字段、bad major/minor、bad start、start 溢出。
  - striped 参数字段数、zero stripes、zero chunk、非数字 stripe_count/chunk_size、bad dev/start。
  - unsupported target：`unknown`、`error`、`snapshot`。
  - target type 空字符串、缺少 NUL 终止符。
  - `dm_target_spec.next` 的非最后 0、too-small、unaligned、out-of-bounds，以及 final target 非法 next。

## 当前脚本改动状态

- `myshell/run_dm_system_tests.sh` 已把系统入口分为 canonical suites 与 compatibility / narrow suites：
  - 基础入口：`--linear-lvm2`、`--striped-lvm2`。
  - resize 入口：`--linear-lvm2-resize`、`--striped-lvm2-resize`。
  - striped 补充入口：`--striped-lvm2-extended`，内部包含 3PV / 3-way 与 multi-segment。
  - mixed 入口：`--mixed-lvm2`。
  - 兼容入口保留：`--linear-lvm2-reboot`、`--striped-lvm2-reboot`、`--data`、`--striped`、`--lvm2`、`--striped-lvm2-3pv`、`--striped-lvm2-multi-segment`。
  - `--full` 没有扩大到 striped / mixed，仍是 linear 系统回归集合。
- 新增 `myshell/dm_linear/run_lvm2_linear_reboot_test.sh`：单 PV、单 segment linear LV，两 guest 验证 create、ext2 I/O、reboot recovery、table/status/deps。
- 新增 `myshell/dm_striped/run_lvm2_striped_reboot_test.sh`：2PV / 2-way、单 segment striped LV，两 guest 验证 create、ext2 I/O、reboot recovery、table/status/deps。
- `myshell/dm_linear/run_lvm2_resize_test.sh` 和 `myshell/dm_striped/run_lvm2_striped_io_reboot_test.sh` 已收窄 summary 过滤，避免把 guest 脚本源码误摘进 host summary；但这两个脚本当前仍会从零创建基础 LV，和最新目标不一致，后续需要改为“基于基础 reboot 产物恢复后继续扩容/缩容”。
- `myshell/dm_striped/run_lvm2_striped_3pv_reboot_test.sh` 已收窄 summary 过滤；它是 striped 几何补充，仍可作为独立 3PV / 3-way 初始创建测试。
- `myshell/dm_striped/run_lvm2_striped_multi_segment_reboot_test.sh` 已删除导致 serial shell 卡在 heredoc 续行提示的二次包装逻辑，并降低默认数据规模；但修复后尚未拿到完整通过结果，不能算最终验收通过。

## 最近验证结果

- 已通过：`myshell/run_dm_system_tests.sh --linear-lvm2`。
- 已通过：`myshell/run_dm_system_tests.sh --striped-lvm2`。
- 曾通过但需重构语义后重跑：`--linear-lvm2-resize`、`--striped-lvm2-resize`。原因是它们当前会重复创建基础 LV，不符合“复用基础产物再扩容/缩容”的分层目标。
- 部分通过：`--striped-lvm2-extended` 中的 3PV / 3-way 子项已通过；multi-segment 子项修复后还没有完整通过记录。
- 待复跑：`--mixed-lvm2`，用于确认当前脚本整理后 mixed 基础组合仍然稳定。
- 待补齐：每个系统脚本的 wall-clock 执行时间基准；此前只记录了部分 guest ready 时间，不能替代完整脚本耗时。

## 功能边界

“跨 target”指同一个 DM table 内逻辑地址跨过相邻 target，例如：

```text
0..100    linear  -> vda
100..500  striped -> vdb/vdc
```

一个 `start=96, len=8` 的 BIO 会被拆成 linear 段和 striped 段后分别下发。

“跨 LV”不是当前应支持场景；不同 LV 是不同 block device，不应出现单个 BIO 同时覆盖两个 LV 逻辑地址空间。

当前系统验收不再把重点写成“跨 PV”，而是写成“跨 segment/table”。PV 数量只是 LVM2 生成不同 table 形态的手段，DM review 重点是 table 形态、target 边界、BIO split/remap、status/deps、flush 和 reboot recovery。

udev、systemd、LVM 自动扫描/自动激活这类完整生态集成，后续最后考虑；当前只把最小 `/dev/dm-*` 和 `/dev/mapper/*` runtime node 作为真实用户态链路所需支撑。

## 后续可做优先级

1. 先重构 resize / 扩容脚本：`--linear-lvm2-resize` 基于 `--linear-lvm2` 产物恢复后扩容/缩容；`--striped-lvm2-resize` 基于 `--striped-lvm2` 产物恢复后扩容/缩容。单独运行 resize 且缺少基础产物时应快速失败并提示先跑基础入口，不应静默重建基础 LV。
2. 再串行复跑并记录耗时：基础 linear、linear resize、基础 striped、striped resize、striped 3PV、striped multi-segment、mixed。
3. 复跑通过后，把实测耗时写入 `docs/test.md` 和 `docs/device-mapper-technical-maintenance.md`；耗时需标注环境与日期，只作为本地容器基准。
4. 如果需要让 `patches/` 适配合入当前 `main` 后的新基线，另起小阶段处理；旧 patch 已知至少 `003-Cargo.lock.patch` 不能直接套到更新后的 `main`。
5. udev / systemd / LVM 自动联动放最后。

## 相关阶段日志

- `log/2026-8-24.md`：当天第 1 到第 4 阶段，包括 mixed active/inactive ktest、striped 几何边界 ktest、mixed LVM2 系统验收、合入当前 main 并适配 `kernel/core` 目录迁移。
- `log/device-mapper-progress.md`：滚动记录当前 `dm` 分支 Device Mapper 项目状态、脚本整理进度和下一步优先级。
