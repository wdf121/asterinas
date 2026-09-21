# Device Mapper 对话交接

> **最后更新**：2026-09-20。
>
> 本文只记录新对话继续工作所需的当前入口、工作区边界和下一步决策点。实现细节、测试说明和每日执行事实分别以链接文档为准。

## 维护规则

只有用户明确说“更新交接文档”时，才允许修改本文。更新时只记录新的当前停点、下一步和必要阅读入口，不复制权威文档已有的实现细节或测试输出。

## 新窗口首先阅读

| 顺序 | 文档或动作 | 用途 |
|---|---|---|
| 1 | [CLAUDE.md](../CLAUDE.md) | 获取协作规则、写入确认、阶段边界与提交规则。 |
| 2 | [AGENTS.md](../AGENTS.md) | 获取容器路径、crate-local ktest、release 规则和系统测试串行约束。 |
| 3 | [global.md](global.md) | 获取当前核心功能、依赖绕过、未实现优先级和用户/运维/架构取舍。 |
| 4 | [技术设计与实现说明](device-mapper-technical-design-and-implementation.md) | 查询 control/data plane、状态机、lease、target 和源码路径。 |
| 5 | [test.md](test.md) | 按改动位置选择 ktest、initramfs C 回归或 NixOS suite。 |
| 6 | [review.md](review.md) | 查询提交基线之后的生产语义、测试增量与框架边界。 |
| 7 | [non-device-mapper-change-rationale.md](non-device-mapper-change-rationale.md) | 查询 DM 所需的非 core 框架改动归属。 |
| 8 | [log/device-mapper-progress.md](../log/device-mapper-progress.md) | 获取当前工作区接手索引和当前测试入口。 |
| 9 | [2026-9-18.md](../log/daily/2026-9-18.md) 与 [2026-9-20.md](../log/daily/2026-9-20.md) | 只交叉核对已执行动作；不以日志替代当前 diff。 |
| 10 | `git status --short`、`git diff e31b265a3` | 判断当前未提交生产、测试基础设施与文档改动。 |

如果文档、日志与当前工作区不一致，以当前源码和 `e31b265a3 → 当前工作区` 的 diff 为准。

## 当前工作区边界

`e31b265a3` 是当前已提交的文档/优化收口基线。该提交之后当前没有新的 commit，所有后续变化都在工作区中。

当前 diff 的主要归属：

- **DM 生产语义**：`DM_DEV_WAIT` 直接进入 wait 处理，不再落入通用命令分派的不可达路径；无 `SA_RESTART` 时用户态收到 `EINTR`，带 `SA_RESTART` 时原 ioctl 自动重启并继续等待事件。
- **测试覆盖**：block/BIO、table child completion、minor 生命周期、registry rollback 等 Rust 增量位于 `#[cfg(ktest)]`，不应描述为新框架生产能力。
- **用户 ABI 回归**：initramfs `device_mapper.c` 覆盖 control ABI、primary/alias rollback、ext2 mount lease、range ioctl、无 `SA_RESTART` 的 `EINTR` 对照、带 `SA_RESTART` 的 WAIT 重启，以及 rename/setuuid waiter 唤醒。
- **测试基础设施**：目标 crate 目录直接运行 `CONSOLE=ttyS0 cargo osdk test`；`REGRESSION_TESTS` 支持目录或单个 C ELF；非 TDX regression 不打包 TDX attestation；当前只保留六个 canonical NixOS DM suite。

## 当前项目路线

当前优先级以 [global.md](global.md) 为准：

```text
P0 现有 DM 主链语义收敛
→ P1 verity
→ P2 crypt
→ P3 snapshot
→ P4 mirror
→ P5 文件系统兼容范围
→ P6 真实故障恢复
→ P7 ABI/deferred remove
→ P8 stacking/queue
→ P9 uevent/sysfs/udev/systemd 自动化
```

当前不主动扩展 exfat 专项验收、MlsDisk 生命周期、virtio/NVMe fault injection、完整 queue stacking 或发行版自动化。只有它们造成 DM 用户可见失败、数据风险或阻塞新 target 时才重新开启。

## 当前验证入口

### ktest

从目标 crate 目录执行：

```bash
cd kernel/core/comps/device-mapper
CONSOLE=ttyS0 cargo osdk test
```

模块选择使用完整 `::tests` selector：

```bash
CONSOLE=ttyS0 cargo osdk test aster_device_mapper::table::tests
```

跨到 core ioctl/runtime 时：

```bash
cd kernel/core
CONSOLE=ttyS0 cargo osdk test --kcmd-args=earlycon \
  aster_core::device::misc::device_mapper::tests
```

### Initramfs C ABI 回归

```bash
RELEASE=1 AUTO_TEST=regression INTEL_TDX=0 \
  REGRESSION_TESTS=device/device_mapper make run_kernel
```

### NixOS DM suite

当前六个入口：

```text
--control-plane
--dataplane
--lvm2-topology
--linear-integration
--striped-integration
--mixed-integration
```

系统 suite 默认 release；QEMU、ktest 和 NixOS system test 必须串行。`--control-plane-non-wait` 已删除。

## 下一步

不要从固定日期日志或旧计划继续推断。先检查当前 diff，再由用户选择：

1. P0 中需要收敛的具体 DM 用户可见行为；或
2. 进入 verity 的设计确认；或
3. 明确要求重新开启某个框架边界。

任何新功能或新阶段先说明原行为、目标行为、示例差异、依赖和最小验证，获得确认后再修改。

## 工作区安全

新窗口在执行写入、构建、测试或提交前先执行：

```bash
git status --short
git diff --check
```

不要假定工作区干净，不要覆盖、删除或提交来源不明的现有改动。构建与测试默认在 `myAsterinas` 容器内进行。
