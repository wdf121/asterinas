# Device Mapper 当前项目状态

> **更新日期**：2026-10-15。
>
> **事实来源**：实现以当前源码为准，验证以对应源码版本和实际命令输出为准；历史日志不自动证明当前工作区通过。

## 当前停点摘要

- 本地 main 为 `77ee3b196`（v0.19.0）；本轮 dm 改动基于 `b5e91dad1`，按握手修复、终端应答、测试日志和文档分点收口，仅本地提交，不 push。0.19.0 同步已完成（2026-10-13，154 提交线性重放，无 merge commit）。
- 2026-10-15：dm 第一阶段 ABI 修复完成：逐 ioctl 检查 initial user namespace 的 `CAP_SYS_ADMIN`，独立 12 字节版本握手，major/minor 兼容检查与版本 copyout `EFAULT` 优先级；check/kernel exit 0，core ktest 242/0（DM 83），两个 focused C 回归 49/0、195/0。普通命令的快照、结果回写与 secure-data 问题仍待第二阶段。
- 六套系统测试共 11 轮 guest 串行通过；终端查询旁路应答已接入，精简日志及六套测试意图已实测。linear 默认最终容量改为 768 MiB。最后的单轮 `guest_ready_after` 名称修正仅做静态检查，按用户要求不重跑系统测试。
- Linux 主语义基线固定为 `v6.18.56`，首次新版差异对照为 `v7.2.10`，两者 DM ABI 均为 4.50.0；这不代表完整 Linux DM 兼容性已验收。
- PR 1（mapped BIO ranges，分支 mapped-bio-ranges）：本地重放至 `716a1161b`；远端 #3937 分支仍为旧历史，重推待定。
- PR 2（dm-linear-io-path）：`d0088c2c9`（6 文件、421 行）；2026-10-14 清除 6 条中文注释洗史并 force-with-lease 重推，ls-remote 实证远端一致；改史后 gate 全绿（check/kernel exit 0，block 6/0，dm 8/0）；PR 未创建，编号待回填。
- PR 3（dm-control-interface，基于 PR 2）：仍为 `d539b725b`，未推送、未同步本轮 dm 修复。10-14 的 160/0 是该旧树的测试记录，不证明握手契约正确；10-15 对固定 Linux 标签的核对已否定“major-only 与 Linux 一致”等旧结论，独立 12 字节握手、minor 校验、错误优先级及相关注释待单独同步验证。PR 1 在审期间不抢推 PR 3。
- OSDK 0.19.0 ktest selector 为后缀 trie：裸模块路径选择器必匹配空集（EXIT=0 假绿）；定向入口为无 selector 的 crate 全量加串口逐行取证；10-13 日志集成层 81/159 记录已勘误作废。
- 普通 VFS rename、完整 mount source 路径及 procfs 的直接覆盖缺口保留；不能以邻近模块通过替代。
- PR 在新 PR 分支最终验证；同步在 rebase 后 dm 最终验证，候选 main 先做干净基线。两边均采用 make check、make kernel 与按影响范围的定向 ktest，不默认全量。
- 校验取舍已归入 PR 规范：当前保留局部范围检查，全局不变量与统一校验入口留待 dm 讨论，不扩大 PR 范围。

证据见 [10 月 15 日](daily/2026-10-15.md) 日志（更早阶段见 10 月 13、14 日）；接手入口见 [正式交接](../docs/device-mapper-handoff.md)。

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
- `DM_READONLY_FLAG` 是 table mode：tableless create 不持久化 readonly，load 仅安装 inactive table mode，首次/替换 resume 后 active I/O 与 active/inactive 查询分别反映选中 table 的 mode；
- mount source 和 ext2 对象持有 tracked lease；已挂载或仍打开的 mapper 不可直接 remove，释放使用者后才可 remove；
- 当前 LVM 路径是显式创建与显式恢复：`pvscan → vgscan --mknodes → vgchange -ay`；
- runtime nodes 由内核直接发布，不依赖 udev 创建当前 DM primary/alias。

## 3. 当前测试入口

### 3.1 ktest

定向模块测试或单个 crate 全量测试从目标 Cargo crate 目录运行；这里的 crate 全量不等于根 workspace 的完整 `make ktest`：

```bash
cd kernel/core/comps/device-mapper
cargo osdk test
```

按模块运行时使用完整 module `::tests` selector：

```bash
cargo osdk test aster_device_mapper::table::tests
```

跨到 core 的 aster_core ktest 时(OSDK 0.19.0 的 selector 为后缀 trie,自函数名段反向匹配,裸模块路径选择器必匹配空集,故不传 selector,以串口聚合计数与逐测试行取证):

```bash
cd kernel/core
cargo osdk test --kcmd-args=earlycon
```

### 3.2 Initramfs C ABI 回归

`REGRESSION_TESTS` 可选择目录或单个 C ELF。DM focused C 回归入口：

```bash
RELEASE=1 AUTO_TEST=regression INTEL_TDX=0 \
  REGRESSION_TESTS=device/device_mapper make run_kernel
```

它覆盖 raw ioctl、runtime node/alias rollback、ext2 mount lease、zero target range ioctl、table-scoped readonly mode，以及无 `SA_RESTART` 的 `EINTR` 对照和带 `SA_RESTART` 的 WAIT 重启；完整 regression 仍可能被无关目录中的失败提前中止，因此 focused selector 是当前 DM ABI 的最小入口。

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

`--control-plane-non-wait` 已删除，不再作为当前入口。
根 Makefile 默认 `RELEASE=1`；系统 suite 启动已有 NixOS 镜像，不自动重建当前源码。
release 验收须确认镜像来自目标源码的显式 release 构建，不能仅在 suite 启动时设置 `RELEASE=1`，也不能把旧镜像结果当作当前源码验证。

`--dataplane` 当前使用三块测试盘；其长 guest 脚本在未显式覆盖时使用 `GUEST_INPUT_LINE_DELAY=0.05`。公共 harness 负责 FIFO 注入、ready/lifecycle 超时和 QEMU process-group 清理。

### 3.4 历史系统验证（不代表当前源码复测）

2026-09-22 已按默认 `GUEST_READY_TIMEOUT=40`、`GUEST_QEMU_TIMEOUT=180` 串行完成六个 canonical suite：control-plane、dataplane、LVM2 topology、linear integration、striped integration 与 mixed integration 均出现 guest、host 和聚合通过标记。LVM2 topology 首次运行发现 `lvs -o ... segtype` 对双 linear segment 返回重复汇总行；测试改为单独验证 LV 汇总的 `seg_count=2`，DM table/dependencies 仍验证两个 backing，复跑通过。每个 suite 的 `/tmp/*-test.log` 是唯一验收日志；host 事件和 QEMU/guest 输出经运行期 FIFO 单写入，FIFO 退出即删，正常路径不保留 `*-qemu-running.txt`。2026-09-23 的 readonly P0 control-plane 验收因默认 40 秒 ready timeout 两次未进入 guest；重建 `target/nixos/asterinas.img` 后，以用户授权的 `GUEST_READY_TIMEOUT=50`、`GUEST_QEMU_TIMEOUT=180` 复跑通过，guest 63 秒完成、全程 71 秒，`SUMMARY_GAP_DM_CONTROL_PLANE: 0`。

2026-09-24：`a4603e369` 已创建 latest `upstream/main` 的同步 merge，`cdefc74dc` 记录专项验证，`0423431d5` 已修复 alias 删除失败时的 `DevtmpfsHandle` 丢失。源码同版本 OSDK core check 通过，devtmpfs 6/0、core registry 8/0、core DM ioctl 81/0、focused C ABI 182/0，以及六项 canonical NixOS suite 全部通过。后续 PR 1 审计发现 deferred replay 的 split child 在 drop guard 自动报错后仍由 table 手动补偿，可能双重递减聚合计数；现移除 completion handle 并由 child 自治完成，block/DM 完整 ktest 为 25/0、87/0。该 split 修复随本提交收口，完整非 DM C regression、全仓 CI、AArch64 和非 QEMU TSC 验证仍不属于本轮覆盖；详见 [upstream sync](../docs/upstream-sync.md)。

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
CLAUDE.md / AGENTS.md
→ docs/device-mapper-handoff.md
→ 本项目状态
→ docs/pull_request.md / docs/upstream-sync.md
→ log/daily/2026-10-15.md（更早日志按需回溯）
→ 按任务阅读源码证据、设计或测试手册
→ 核对当前工作区与实际任务的比较基线
```

接手时不假定工作区干净；先按当前 diff 判断生产、测试基础设施与文档的改动归属。QEMU、ktest 和 NixOS system test 必须串行执行。
