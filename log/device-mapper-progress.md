# Device Mapper 当前项目状态

> **更新日期**：2026-09-22。
>
> **事实来源**：当前工作区相对 `e31b265a3` 的实际 diff 与当前源码。daily log 只用于交叉核对已执行动作，不替代源码事实。

## 1. 当前方向

当前方向是稳固已实现的 Device Mapper 主链，不主动扩大通用框架：

```text
DM control ABI
→ runtime primary / alias
→ linear / striped / zero / error table
→ mapper I/O / flush
→ LVM2 显式创建、扩缩、恢复
→ ext2 mapper mount / remove 保护
```

后续 target 路线以 [global.md](../docs/global.md) 为准：

```text
P0 主链语义收敛
→ verity
→ crypt
→ snapshot
→ mirror
```

## 2. 当前实现状态

- `/dev/mapper/control` 提供当前 DM ioctl 子集；
- `DM_DEV_WAIT` 直接进入 wait 处理，不再落入通用命令分派的不可达路径；无 `SA_RESTART` 时用户态收到 `EINTR`，带 `SA_RESTART` 时原 ioctl 自动重启并继续等待事件；
- 首次 table load 发布 `/dev/dm-N`，首次 resume 发布 `/dev/mapper/<name>`；node/alias 创建、rename、remove 均有事务和回滚/Removing 隔离；
- target 为 `linear`、`striped`、`zero`、`error`；支持连续 mixed table、跨 target/chunk I/O、flush fan-out；
- mount source 和 ext2 对象持有 tracked lease；已挂载或仍打开的 mapper 不可直接 remove，释放使用者后才可 remove；
- 当前 LVM 路径是显式创建与显式恢复：`pvscan → vgscan --mknodes → vgchange -ay`；
- runtime nodes 由内核直接发布，不依赖 udev 创建当前 DM primary/alias。

## 3. 当前测试入口

### 3.1 ktest

当前定向/完整 ktest 从目标 Cargo crate 目录运行，不再使用已删除的 `myshell/ktest_crate.sh`：

```bash
cd kernel/core/comps/device-mapper
CONSOLE=ttyS0 cargo osdk test
```

按模块运行时使用完整 module `::tests` selector：

```bash
CONSOLE=ttyS0 cargo osdk test aster_device_mapper::table::tests
```

跨到 core ioctl/runtime 时：

```bash
cd kernel/core
CONSOLE=ttyS0 cargo osdk test --kcmd-args=earlycon \
  aster_core::device::misc::device_mapper::tests
```

### 3.2 Initramfs C ABI 回归

`REGRESSION_TESTS` 可选择目录或单个 C ELF。DM focused C 回归入口：

```bash
RELEASE=1 AUTO_TEST=regression INTEL_TDX=0 \
  REGRESSION_TESTS=device/device_mapper make run_kernel
```

它覆盖 raw ioctl、runtime node/alias rollback、ext2 mount lease、zero target range ioctl，以及无 `SA_RESTART` 的 `EINTR` 对照和带 `SA_RESTART` 的 WAIT 重启；完整 regression 仍可能被无关目录中的失败提前中止，因此 focused selector 是当前 DM ABI 的最小入口。

### 3.3 NixOS DM system suite

当前只保留六个 canonical suite：

```bash
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 \
  myshell/run_dm_system_tests.sh --control-plane

GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 \
  myshell/run_dm_system_tests.sh --dataplane

GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 \
  myshell/run_dm_system_tests.sh --lvm2-topology

GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 \
  myshell/run_dm_system_tests.sh --linear-integration

GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 \
  myshell/run_dm_system_tests.sh --striped-integration

GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 \
  myshell/run_dm_system_tests.sh --mixed-integration
```

`--control-plane-non-wait` 已删除，不再作为当前入口。系统 suite 默认 release；只有明确调试时才指定 `RELEASE=0`。

`--dataplane` 当前使用三块测试盘；其长 guest 脚本在未显式覆盖时使用 `GUEST_INPUT_LINE_DELAY=0.05`。公共 harness 负责 FIFO 注入、ready/lifecycle 超时和 QEMU process-group 清理。

2026-09-22 已按默认 `GUEST_READY_TIMEOUT=40`、`GUEST_QEMU_TIMEOUT=180` 串行完成六个 canonical suite：control-plane、dataplane、LVM2 topology、linear integration、striped integration 与 mixed integration 均出现 guest、host 和聚合通过标记。LVM2 topology 首次运行发现 `lvs -o ... segtype` 对双 linear segment 返回重复汇总行；测试改为单独验证 LV 汇总的 `seg_count=2`，DM table/dependencies 仍验证两个 backing，复跑通过。每个 suite 的 `/tmp/*-test.log` 是唯一验收日志；host 事件和 QEMU/guest 输出经运行期 FIFO 单写入，FIFO 退出即删，正常路径不保留 `*-qemu-running.txt`。

## 4. 当前边界

以下不属于当前主动工作：

- uevent / sysfs / udev / systemd 自动发现和自动激活；
- DM-on-DM stacking；
- deferred remove、完整 control ABI 扩展；
- 完整 queue stacking、后端限制自动再拆；
- ext4 / exfat 作为当前核心验收；
- MlsDisk 生命周期、virtio/NVMe fault injection 等框架专项工作。

只有它们造成 DM 用户可见失败、数据风险或阻塞新 target 时才重新开启。

## 5. 新对话接手顺序

```text
CLAUDE.md
→ AGENTS.md
→ docs/global.md
→ docs/device-mapper-technical-design-and-implementation.md
→ docs/test.md
→ docs/review.md
→ docs/non-device-mapper-change-rationale.md
→ log/daily/2026-9-18.md
→ log/daily/2026-9-20.md
→ git status --short
→ git diff e31b265a3
```

接手时不假定工作区干净；先按当前 diff 判断生产、测试基础设施与文档的改动归属。QEMU、ktest 和 NixOS system test 必须串行执行。
