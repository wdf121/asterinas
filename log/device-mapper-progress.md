# Device Mapper 项目进度

本文记录 `dm` 分支中变化较快的 Device Mapper 项目状态。若本文与当前代码或实际命令结果不一致，以当前代码和命令结果为准，并及时修正本文。

## 当前状态

截至 2026-09-04，当前工作重点是稳固已实现的 Device Mapper 功能与验证链路，不新增 target 或扩大 Linux DM 兼容声明。已确认的实现范围包括 `error`、`zero`、`linear`、`striped`，以及同一 mapper/LV 内的 linear + striped mixed table。DM table/control-plane 已从 closed `DmTarget` enum 迁移为 `dyn DmTarget` trait object；定向 ktest 默认使用 crate-local wrapper。

当前六个 canonical NixOS system suite 均已通过，且每个实际 guest 的 shell-ready 时间均不超过 40 秒：

```text
myshell/run_dm_system_tests.sh --control-plane
myshell/run_dm_system_tests.sh --dataplane
myshell/run_dm_system_tests.sh --lvm2-topology
myshell/run_dm_system_tests.sh --linear-integration
myshell/run_dm_system_tests.sh --striped-integration
myshell/run_dm_system_tests.sh --mixed-integration
```

运行时统一使用：

```bash
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 \
  myshell/run_dm_system_tests.sh <suite>
```

`GUEST_READY_TIMEOUT=40` 限制 QEMU 启动到 guest shell-ready；`GUEST_QEMU_TIMEOUT=180` 限制单个 guest 的完整生命周期。系统测试必须串行执行。入口不提供无参数默认执行或历史兼容别名。

## 新对话接手阅读清单

新开对话或上下文压缩后，优先阅读：

```text
CLAUDE.md
AGENTS.md
log/device-mapper-progress.md
log/2026-9-4.md
docs/test.md
docs/production-code-validation-chain.md
docs/device-mapper-technical-maintenance.md
docs/non-device-mapper-change-rationale.md
```

阅读重点：

- `CLAUDE.md`：协作规则，包括简体中文、精简汇报、新阶段先说明差异、默认不 push。
- `AGENTS.md`：容器路径、测试入口、系统测试串行和定向 ktest 约束。
- `log/2026-9-4.md`：six-suite 收敛、公共 harness 变更、非对齐 striped 诊断与本轮真实验证记录。
- `docs/test.md`：当前系统验收命令、suite 职责、marker 和超时含义。
- `docs/production-code-validation-chain.md`：ktest 与 system test 的实际执行链路。
- `docs/device-mapper-technical-maintenance.md`：DM 架构、target 边界、系统验证矩阵和 command alignment 附录。
- `docs/non-device-mapper-change-rationale.md`：DM 依赖的通用内核与测试基础设施改动边界。

## 当前验证入口与职责

| suite | 所有者与覆盖范围 |
|---|---|
| `--control-plane` | `dmsetup` discovery、tableless/table 生命周期、linear/striped/error/zero table/status/deps/info、events、rename/UUID、readonly、busy remove/remove_all；当前包含 error/zero I/O。 |
| `--dataplane` | raw linear、striped、mixed、error、zero；nonzero backing start、跨 target/chunk split、flush、discard/write-zeroes、mapper readback 和 backing 布局。 |
| `--lvm2-topology` | static LVM2 查询、PV/VG/LV lifecycle、linear/striped/mixed create/grow/shrink、table/status/deps、activation/scan/remove；不做 ext2 或 reboot persistence。 |
| `--linear-integration` | linear same-PV/cross-PV second segment、ext2、grow/shrink 和三次启动恢复。 |
| `--striped-integration` | parameterized N-way striped、same-set/cross-set second segment、ext2、grow/shrink 和三次启动恢复。 |
| `--mixed-integration` | linear + striped mixed LV、跨段 ext2 I/O 和两次启动恢复。 |

公共 harness [dm_nixos_test.sh](../myshell/lib/dm_nixos_test.sh) 提供 single、two、three guest 流程，并输出 started、shell-ready、completed 的 ISO 时间与 elapsed marker。默认 `GUEST_INPUT_LINE_DELAY=0.01`，按行节流注入 guest 脚本；设置为 `0` 才显式关闭节流。

## 已完成并验证的范围

### DM core 与用户可见 ABI

- `/dev/mapper/control` 和 Linux DM 核心 ioctl 子集已支持：create/remove/remove_all/rename/status/list/wait、table load/clear/status/deps、active/inactive lifecycle、readonly 与主要 flags。
- target 支持 `error`、`zero`、`linear`、`striped`，以及 linear + striped mixed table。
- `error` 无 backing，Read/Write 返回 I/O error；`zero` Read 返回全零、Write 丢弃；二者无 backing Flush 均 direct-complete，deps 为空。
- linear/striped 支持 Read/Write/Discard/WriteZeroes remap；table-level 与 target-level split 通过 completion 聚合保证原始 BIO 只完成一次。
- Flush 对 backing 去重后 fan-out；无 backing table direct-complete。

### 2026-09-04 数据面与系统验收收敛

- 删除历史阶段性 system-test 入口，只保留六个 canonical suite；保留公共 harness、`myshell/ktest_crate.sh` 和 Linux baseline 对照脚本。
- 公共 harness 增加独立的 40 秒 shell-ready timeout、180 秒 lifecycle timeout、guest timing marker、QEMU group cleanup 和 three-guest helper。
- 修复长 guest shell script 一次性通过 serial 输入时可能丢失后续命令的问题：默认按行以 10ms 节流注入；这不是 DM I/O hang。
- `--dataplane` 的非对齐 striped 场景从 mapper sector 2 单次写入 12 sectors，验证四个 child 的真实 backing 布局；Step 5 I/O 另有 20 秒诊断 timeout。
- `aster-device-mapper` 新增精确 12-sector ktest，验证 `striped 2 4` 下 `[2,14)` Write 分成 2/4/4/2 sectors 的四个 child；前三个乱序完成后原 BIO 仍 pending，最后一个完成后原 BIO 只完成一次。
- 六个 canonical suite 已逐个串行通过；所有实际 guest shell-ready 均在 40 秒上限内，未发现残留 QEMU。

### 当前功能边界

当前不声明完整 Linux Device Mapper、完整 LVM2 用户体验或完整 udev/systemd 自动激活生态。尚不纳入：snapshot、thin、cache、crypt、mirror、raid 等 target 族，完整 sysfs DM 层级，DM-on-DM backing，queue limit/alignment/topology，真实 guest backing I/O error/partial completion 注入，以及 NVMe discard/write-zeroes 后端命令。

## 后续优先级

1. 继续以当前六个 suite 和相关 ktest 稳固已实现功能；改动按 owner 选择窄验证，避免无关 system suite。
2. 若审计数据面，优先评估 queue limit/alignment/topology、真实 backing error 和 partial completion；不要将这些解释为当前已支持的完整 queue stacking。
3. 新 target 或 target registry 等扩展应另起小阶段，先说明原有行为、目标行为和最小验收路径。
4. `patches/` 当前暂不处理；后续若更新 patch，再单独核对其与执行树和验证记录的一致性。

## 相关阶段日志

- `log/2026-8-24.md`：mixed active/inactive ktest、striped 几何边界 ktest、mixed LVM2 系统验收、合入当前 main 并适配 `kernel/core` 目录迁移。
- `log/2026-8-31.md`：dmsetup 控制面语义对齐、LVM2 控制面 baseline/guest 同构、raw DM 数据面边界 guest 审计。
- `log/2026-9-1.md`：guest 启动慢排查修复、`zero` target 核心/控制面/数据面覆盖、discard / write zeroes 通用 range BIO 与 DM 映射接入。
- `log/2026-9-2.md`：源码注释、DM 文档事实、系统测试脚本口径、成功 marker、超时变量和 patches 同步记录。
- `log/2026-9-3.md`：`DmTarget` trait object 重构、crate-local ktest wrapper、runner 结果行拆分、`timeout --foreground` 修复和验证结果。
- `log/2026-9-4.md`：six-suite 收敛、公共 harness 输入节流、非对齐 striped 四 child ktest 和六套系统验收。
