# Device Mapper 当前项目状态

> **更新日期**：2026-10-26。当前 dm 为 `37ddb9245` 加恢复的未提交改动，最新 main 为 `ceb65e3f8`，未 push。
>
> 实现以源码为准，验证区分助手实际执行、用户反馈及历史证据。今日明细见 [日报](daily/2026-10-26.md)。

## 当前停点摘要

- 上游新增 23 个提交已 merge；原研发改动恢复，唯一 stash 冲突 test/nixos/Makefile 已解决，保留上游缩进和套件 runner。安全引用和 stash 保留。
- 用户反馈合并后 make check 与控制面集成测试通过；无独立输出/计数/镜像来源记录。本次同步按用户决定跳过 make kernel，未运行合并后 ktest/C 全量回归。
- 控制面 13 例恢复薄 main.rs + cases.sh，删除实验性四个 Rust 模块及辅助层 7 项单测；run.sh/runner.py 保留，日志为 CST。同步前恢复运行 13/13、62 秒，用户随后反馈 45 秒；不是同步后性能数据。
- extra_config.nix 显式声明工具并保留基础包，复用 dm overlay 的 aster-test-disk-locator；格式/语法检查通过，未重建镜像，上游需另行解决 locator 依赖。
- ABI 4.48.0、测试与文档改动仍未提交；旧 PR 档案及 Word 删除保持用户原状态。
- PR1 不改实现；新 PR2 为 ea1a9639c，后续 PR2～PR11 框架路线见 [规划](../docs/pull_request.md)，框架后接最小 linear 终验。当前只批准记录规划，不自动授权实现缺口。

## 历史停点（2026-10-19 至 2026-10-20）

以下内容保留当时的分支、授权和测试事实；不作为当前分支清单或当前工作区的验收依据。

- 2026-10-19：完成 PR1–PR3 审阅、复核和单提交整理；dm 回补 poll 虚假就绪修复、BIO 边界与队列顺序测试、公开提交契约测试。验证基线为 `93159d132` 加本轮修改；代码已分点提交为 `7740a5281`、`64adad1c0`、`981294ef5`，文档随本次收口提交更新，均未推送。
- Claude 独立运行完整 `make check` 通过；五组定向 ktest 为 BIO 13/0、queue 5/0、device 20/0、table 36/0、control ioctl 24/0，共 98/0，各命令退出 0。不是全仓验证。用户反馈 `make check`、`make kernel`、guest ABI 回归通过，后两项本轮未独立核验日志。
- C ABI 测试按 control/lifecycle/table 职责拆为三个程序；原 24 个场景保留，新增 poll 后为 13/7/5，共 25 个场景，不是断言数量。
- 控制 fd 不再恒报 IN/OUT；全局事件计数、等待队列、每打开文件事件基准及 ARM_POLL 尚未实现。单设备 `DM_DEV_WAIT` 不等于完整全局 poll。
- PR1（`mapped-bio-ranges`，#3937）当前本地 `2f4fdc9a7`；今日曾推送中间版本 `e7e221993`，随后重放形成的当前历史未再次推送。
- PR2（`dm-linear-io-path`）当前本地 `fd97f023f`，依赖新版 PR1，已 amend，未推送；本轮没有最终 PR2 ktest 运行计数，不以 dm 测试替代。
- PR3（`dm-control-interface`）当前本地 `5b08f9a0a`，依赖新版 PR2；10 月 20 日已 amend 公共入口顺序、命名与测试，未推送。10 月 19 日的旧提交检查结果不能替代新 SHA 验收，详见顶部最新记录。
- 当前 OSDK `test_matches_whitelist` 已支持模块路径与函数路径，本日实际非零计数验证了模块筛选；此前“裸模块路径必匹配空集”的说明不再适用于当前源码，历史零匹配仍不算通过。
- 后续以单 PR 为边界，用户主导手写，Claude 基于源码事实分析审查，仅在明确授权后实现，不自主大幅优化 DM。回补前核对完整 dm 的调用链和能力边界。
- 2026-10-16 V5 收尾及系统套件结果属于历史验证，今日未重跑 dmsetup/LVM/NixOS 系统套件。主要 Linux 语义依据仍为指定 openEuler 源码基线，报告 ABI 4.50.0 不代表完整 Linux DM 兼容。
- 根盘问题继续待定，不重启排查。用户反馈与上游一致的 main 也出现 users activation EUCLEAN；不能据此确定根因，也不能归咎于 DM 或写成 NixOS 编译失败。
- 普通 VFS rename、完整 mount source 路径及 procfs 的直接覆盖缺口保留。最终验证采用 check/kernel/相关定向 ktest，涉及 ABI 加 C 回归，不默认完整 ktest。

最新证据见 [10 月 19 日](daily/2026-10-19.md)，历史 V5 收尾见 [10 月 16 日](daily/2026-10-16.md)。旧交接文档中的提交号与入口可能滞后，以当前源码、实际结果和本页为准。

## 1. 当前方向

当前从 PR2 起规划通用 DM 框架，按单 PR 审查、复核并另行授权实现；最后以最小 linear 终验。下列主链用于依赖分析，不代表框架缺口已补齐：

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

本轮五组定向入口（容器内串行执行）：

```bash
cd /root/asterinas/kernel/core/comps/block
cargo osdk test aster_block::bio::tests
cargo osdk test aster_block::request_queue::tests
cd /root/asterinas/kernel/core/comps/device-mapper
cargo osdk test aster_device_mapper::device::tests
cargo osdk test aster_device_mapper::table::tests
cd /root/asterinas/kernel/core
cargo osdk test --kcmd-args=earlycon aster_core::device::misc::dm_control::tests::ioctl
```

每次立即核对并保存根目录 `qemu-serial.log`，记录实际 passed/failed/filtered 与退出码；下一次运行会覆盖，零匹配不算通过。

### 3.2 Initramfs C 用户态 ABI 回归

```bash
cd /root/asterinas
make run_kernel AUTO_TEST=regression REGRESSION_TESTS="device/device_mapper_control device/device_mapper_lifecycle device/device_mapper_table"
```

| 程序 | 职责 | 场景数 |
|---|---|---:|
| `device_mapper_control` | 版本、权限、参数/内存边界、选择器、回写、无事件 poll | 13 |
| `device_mapper_lifecycle` | 身份、节点回滚、挂载占用、WAIT 信号与事件 | 7 |
| `device_mapper_table` | 表查询、backing 路径、只读转换、范围 ioctl | 5 |

原 `device_mapper`、`device_mapper_version` 不再是当前源码构建/runner 入口；旧构建目录即使残留二进制也不能用于本轮验收。按职责而非命令分文件，原 24 个场景保留，新增后共 25 个，不是断言数量。这些是真实系统调用的用户态 ABI 测试，不替代 dmsetup/LVM 集成。

### 3.3 NixOS DM system suite

控制面优先使用官方 `make run_nixos NIXOS_TEST_SUITE=device-mapper-control-plane`；以下为原六套本地脚本的历史/辅助入口，不是后续 PR 的交付模板：

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
- deferred remove、完整 control ABI 扩展及全局控制事件 poll/ARM_POLL；
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
→ log/daily/2026-10-19.md（更早日志按需回溯）
→ 按任务阅读源码证据、设计或测试手册
→ 核对当前工作区与实际任务的比较基线
```

接手时不假定工作区干净；先按当前 diff 判断生产、测试基础设施与文档的改动归属。QEMU、ktest 和 NixOS system test 必须串行执行。
