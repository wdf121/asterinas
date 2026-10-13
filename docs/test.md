# Asterinas 开发测试手册

各验证阶段的底层执行链见 [生产代码验证链路](production-code-validation-chain.md)。

本文用于把**改动位置或行为**转换为可执行的测试动作：

```text
改动位置或行为 → 最低测试层 → 命令 → 日志 → 通过判定 → 是否升级到下一层
```

命令默认在项目容器 `fork_Asterinas` 的仓库根目录 `/root/asterinas` 执行，只有标明的 crate-local 命令切换目录。修改先通过静态准入；随后从最能定位问题的运行时层开始，不以更高层测试替代更低层的定点断言。

## 1. 三个运行时测试层的分工

| 层 | 如何运行 | 主要回答的问题 | 何时继续向上验证 |
|---|---|---|---|
| ktest | 目标 Cargo crate 被编译进 test kernel，`#[ktest]` 在内核态运行。 | 内部对象、算法、状态机、锁/并发、资源生命周期和局部错误路径是否正确？ | 改动穿过 syscall、ioctl、设备节点、VFS 或其他用户 ABI 时。 |
| initramfs C 回归 | 最小 guest 中，shell runner 执行 C ELF；程序直接调用 libc/syscall。 | 原始用户 ABI 的参数、buffer、返回值、errno 与基础 VFS/设备语义是否正确？ | 改动还影响真实 CLI、用户库、系统服务、发行版配置或安装后工作流时。 |
| NixOS/ISO 系统测试 | 启动 NixOS guest，测试框架向 guest 注入 shell/CLI 命令。 | 真实用户工具、用户库、服务、文件系统、网络、安装和发行版集成是否可用？ | 系统测试失败后，应回到 C 回归或 ktest 补最小可定位用例。 |

三层都在 guest 中触达真实内核，但观察点不同：ktest 直接观察内核内部语义，C 回归直接观察原始用户 ABI，NixOS 测试观察完整用户空间工作流。

## 2. 静态准入

先按改动范围选择预检；这些检查通过后再运行后续三层测试。

| 改动范围 | 命令 | 查看位置 | 通过判定 |
|---|---|---|---|
| 任意已修改文件 | `git diff --check` | 终端输出。 | 无输出且退出码为 0。 |
| Rust 文件 | `cargo fmt --check` | 终端输出。 | 无格式差异且退出码为 0。 |
| 准备进行项目级静态验证 | `make check` | 终端输出。 | workspace lint、Rust 格式/clippy、Nix、initramfs C/Nix、NixOS 测试代码格式和 typos 全部通过。 |

`git diff --check` 覆盖 diff 的通用文本问题，`cargo fmt --check` 仅覆盖 Rust，`make check` 是项目聚合检查。三者都不证明内核已启动、用户 ABI 正确或端到端流程可用。

2026-10-08 已在当日未提交修复上完成容器 `make check`（退出码 0）。当前 gate 对生产与 `--ktests` 配置分别运行 clippy，并运行 udeps；C 和 NixOS 部分在这里只检查测试源码，不运行 guest。尾随空白检查忽略二进制，typos 排除 `patches/**` 历史归档，其余源码与正式文档仍受检查。逐阶段执行链及 `udeps 0.1.61` 跨 target 重复包警告的解释见 [验证链路 §1.4](production-code-validation-chain.md#14-make-check-的完整静态检查链)，失败与修复文件清单见 [当日日志](../log/daily/2026-10-8.md)。

## 3. ktest：内核内部逻辑

以下 crate-local 命令保留原有实测入口，执行目录相对于容器仓库根目录 `/root/asterinas`。
无 selector 的命令按当前目录选择测试 crate，执行前必须确认目录；历史通过结果不自动覆盖最新源码。
定向结果必须同时确认目标匹配数量大于 0、目标测试零失败、整次命令退出码为 0；runner 枚举的测试总数和 `filtered out` 不计入实际通过数量。
记录 HEAD、相关未提交源码差异、实际命令与独立保存的本轮日志；没有直接测试覆盖的改动应标明缺口，不能以邻近模块通过代替。

上游同步默认保留 `make check`、`make kernel`，并按影响范围选取定向 ktest；范围包括新增、冲突解决及受接口、依赖或语义变化影响的代码，不能只看文本冲突。
候选 `main` 在干净独立 worktree 中先验证上游适用的范围，集成后的 `dm` 追加 DM 专属回归；详见 [同步规则](upstream-sync.md#8-后续同步规则)。

只有经讨论需要扩大范围时，才在仓库根目录执行完整 `make ktest`，由当前 `default-members` 决定范围。
它自动依赖 initramfs，并在根 Makefile 中指定 `CONSOLE=ttyS0`，与下表手动命令的默认 `hvc0` 日志路径不同；完整结果须看整次命令输出与退出码，不能仅统计最后一次覆盖的 `qemu.log`。
2026-10-08 的 VMX 测试修复和网络恢复证据见 [当日日志](../log/daily/2026-10-8.md)。
该日完整 ktest 早于后续 lint 源码修复；10 月 8—9 日又对修复后工作区完成 DM component 87/0、core DM ioctl 81/0、devtmpfs 6/0、runtime registry 8/0、VMCS 1/0 的定向回归，各命令 exit 0。
本轮结果与覆盖缺口见 [回归收尾日志](../log/daily/2026-10-9.md)，不代表 C/NixOS 回归或全仓验证通过。

**默认回归清单**：DM、block 各运行一次 crate 全量，core 运行以下三个模块。

| 测试范围 | 执行目录 | 直接命令 | 覆盖范围 / 文件 |
|---|---|---|---|
| `aster-device-mapper` 全量 | `kernel/core/comps/device-mapper` | `cargo osdk test` | `device.rs`、`manager.rs`、`table.rs`、`target/**`。 |
| core：DM ioctl / runtime 模块 | `kernel/core` | `cargo osdk test --kcmd-args=earlycon aster_core::device::misc::device_mapper::tests` | `kernel/core/src/device/misc/device_mapper.rs`。 |
| core：runtime block registry 模块 | `kernel/core` | `cargo osdk test --kcmd-args=earlycon aster_core::device::registry::block::tests` | `kernel/core/src/device/registry/block.rs`。 |
| `aster-block` 全量 | `kernel/core/comps/block` | `cargo osdk test` | BIO、注册/注销与 lease、device ID、partition、request queue。 |
| core：devtmpfs 模块 | `kernel/core` | `cargo osdk test --kcmd-args=earlycon aster_core::fs::fs_impls::devtmpfs::tests` | `kernel/core/src/fs/fs_impls/devtmpfs/**`：运行期节点与 identity 路径。 |

**定向排障入口**：以下模块已包含在对应 crate 全量中；仅在定位失败或局部复测时使用，不需要在全量通过后逐条重复执行。

| 测试范围 | 执行目录 | 直接命令 | 覆盖范围 / 文件 |
|---|---|---|---|
| DM component：table 模块 | `kernel/core/comps/device-mapper` | `cargo osdk test aster_device_mapper::table::tests` | `kernel/core/comps/device-mapper/src/table.rs`。 |
| DM component：manager 模块 | `kernel/core/comps/device-mapper` | `cargo osdk test aster_device_mapper::manager::tests` | `kernel/core/comps/device-mapper/src/manager.rs`。 |
| block component：BIO 模块 | `kernel/core/comps/block` | `cargo osdk test aster_block::bio::tests` | `kernel/core/comps/block/src/bio.rs`。 |
| block component：注册与 lease | `kernel/core/comps/block` | `cargo osdk test aster_block::tests` | `kernel/core/comps/block/src/lib.rs`：注册、注销、lease 与事务回滚。 |
| block component：device ID 模块 | `kernel/core/comps/block` | `cargo osdk test aster_block::device_id::tests` | `kernel/core/comps/block/src/device_id.rs`：major 快照与持有期。 |
| block component：partition 模块 | `kernel/core/comps/block` | `cargo osdk test aster_block::partition::tests` | `kernel/core/comps/block/src/partition.rs`：range BIO 偏移与溢出。 |
| block component：request queue 模块 | `kernel/core/comps/block` | `cargo osdk test aster_block::request_queue::tests` | `kernel/core/comps/block/src/request_queue.rs`：range 合并与 segment 上限。 |

下表只说明上述手动 crate-local 命令在默认 x86_64、非 TDX 配置下的 console/日志行为；根 `make ktest` 默认指定 `CONSOLE=ttyS0`，不适用表中的默认 `hvc0` 路径。
ktest 结果经 early serial 输出，不能与普通内核的 `/dev/console` 选择混为一谈。

| 参数 / 日志 | 默认或写法 | 作用 | 手动查看 |
|---|---|---|---|
| `CONSOLE=hvc0` | 未显式设置 `CONSOLE` 时默认。 | virtconsole 接终端；ktest UART 结果写入独立日志，不在终端显示。 | `less qemu-serial.log` |
| `CONSOLE=ttyS0` | 仅交互式排障时显式设置，不属于默认命令。 | UART 接终端；ktest 结果同时显示在终端并写入 `qemu.log`。 | `less qemu.log` |
| 模块 selector | `crate::...::tests` | 一次选择该模块中的全部 `#[ktest]`；不需要逐个填写函数名。 | 默认查看 `qemu-serial.log`；排障时按显式 console 设置查看。 |
| 无 selector | 上表 DM、block crate 全量命令。 | 执行当前目录所选 crate 在当前构建配置下的全部 ktest；不代表 core 或 workspace 全量已验证。 | 同上。 |
| 离线依赖 | `CARGO_NET_OFFLINE=true` 仅用于网络或依赖问题的排障/恢复。 | 仅使用本地缓存；缓存缺失时报错。标准验证不设置该变量。 | 命令终端输出。 |
| core early console | `--kcmd-args=earlycon` | 启用 core 的早期串口；它是 core ktest 输出的必要内核参数，不等同于 host 的 `CONSOLE` 覆盖。 | 默认写入根目录 `qemu-serial.log`。 |

core 链入的早期参数解析器默认关闭 early console；未传 `earlycon` 时，ktest 的 `early_print!` 输出会被丢弃。上表 core 模块均使用带 `--kcmd-args=earlycon` 的命令实测通过。DM/block 使用 OSTD 默认开启 early console 的解析器，保留其已实测通过的原命令，不额外添加参数。

上表所列测试入口均使用仓库根目录的 OSDK manifest，QEMU 日志因此位于**仓库根目录**，不是调用命令的 crate 目录；默认 `hvc0` 下的 ktest UART 结果写入 `qemu-serial.log`。仅在排障时显式设置 `CONSOLE=ttyS0`，此时输出写入 `qemu.log`；已有同名文件可能是旧日志。

## 4. initramfs C 回归：原始用户 ABI

C 回归把测试源码编译为用户态 ELF，与 shell runner 一起打包进 initramfs；QEMU 启动 Asterinas 后，guest 内的 regression runner 执行 ELF，验证真实用户进程能否通过 syscall、ioctl、设备节点和 VFS 正确使用内核接口。它与直接运行内核内部 Rust 断言的 ktest 分工不同，不能互相替代。

### 4.1 运行命令

命令均在容器内仓库根目录执行；测试必须串行，运行前确认没有其他 QEMU、ktest 或 NixOS 测试占用测试镜像：

```bash
pgrep -af 'qemu-system|cargo osdk test|make run_kernel'
```

Device Mapper focused 回归的最小命令：

```bash
AUTO_TEST=regression \
  REGRESSION_TESTS=device/device_mapper make run_kernel
```

`AUTO_TEST=regression` 构建回归 ELF 并让 guest 启动后进入 regression runner；`REGRESSION_TESTS=device/device_mapper` 让 runner 只执行 `/test/device/device_mapper`，省略时运行全量。可选参数：`RELEASE=1`（release 内核，默认 0）、`CARGO_NET_OFFLINE=true`（仅用本地 Cargo 缓存）、`INTEL_TDX=0`（默认值即 0，显式写出只为防环境覆盖）。

对应源码为 `test/initramfs/src/regression/device/device_mapper.c`，覆盖：tableless DM ioctl（`DM_VERSION`、create、status、table status、remove 的结构体布局、flag、返回值和 errno）、`/dev/dm-N` 与 `/dev/mapper/<name>` 运行时节点事务与冲突回滚、ext2 mount lease（挂载期间删除返回 `EBUSY`）、`BLKDISCARD`/`BLKZEROOUT` 范围边界、WAIT ioctl 的 `EINTR`/`SA_RESTART`/事件唤醒。该入口验证原始用户 ABI，不替代 DM/block crate ktest，也不替代 NixOS 系统测试。

### 4.2 通过判定与日志

命令退出码为 0，且仓库根目录 `qemu.log` 末尾包含：

```text
Test /test/device/device_mapper passed.
All regression tests passed.
```

只提取执行结果：

```bash
grep -E 'Running test /test/device/device_mapper|summary:|test result:|Test /test/device/device_mapper passed\.|All regression tests passed\.|failed' qemu.log
```

失败时以本次命令生成的终端输出和 `qemu.log` 为准，不要用旧日志中的成功标记判断本轮结果。排查早期启动或串口问题时再看 `qemu-serial.log`。

### 4.3 其他 C 回归入口

| 测试范围 | 容器内仓库根目录执行的命令 | 通过判定 |
|---|---|---|
| 启动协议、早期初始化、rootfs、initramfs 可用性 | `AUTO_TEST=boot make run_kernel` | 命令退出码为 0，根目录 `qemu.log` 包含 `Successfully booted.`。 |
| 一个 regression 目录 | `AUTO_TEST=regression REGRESSION_TESTS=<directory> make run_kernel` | 所选目录完成，最终包含 `All regression tests passed.`。 |
| 单个 C ELF | `AUTO_TEST=regression REGRESSION_TESTS=<directory>/<binary> make run_kernel` | 对应 ELF 成功结束，最终包含 `All regression tests passed.`。 |
| 全量 initramfs regression | `AUTO_TEST=regression make run_kernel` | 遍历 `/test` 下全部一级测试目录，最终包含 `All regression tests passed.`。 |

`REGRESSION_TESTS` 接受相对 selector：目录 selector 执行该目录的 `run_test.sh`，ELF selector 直接执行对应程序；不要传绝对路径、`.`、路径穿越或包含空格的 selector。

已知环境限制：在本机容器上，全量回归会稳定失败于 `process/cgroup` 的 CPU 计时用例（busy 任务 2 秒仅记账约 0.6 秒，判据要求 ≥1.9 秒），与被测改动无关；日常验证请使用 focused selector。

### 4.4 常见失败定位

| 现象 | 优先检查 |
|---|---|
| C 编译因 warning 失败 | 终端中编译器的首个 `error:`；initramfs C 构建启用了 `-Werror`。 |
| QEMU 报测试镜像无法取得 write lock | 是否已有 QEMU、ktest 或 NixOS 测试进程；不要并发复用同一镜像。 |
| guest 启动后测试断言失败 | `qemu.log` 中首个 `failed` 及其所在测试函数，随后再检查最终汇总。 |
| 没有最终成功标记 | 检查 `qemu.log` 最后 100 行，确认是 guest panic、runner 退出、QEMU 异常退出还是测试超时。 |
| 完整生命周期超过约三分钟且无有效进展 | 将其视为异常，检查构建进程、QEMU 进程、镜像锁和 `qemu.log` 是否仍在增长；不要用重启容器代替定位。 |

## 5. NixOS 与 ISO：完整用户空间流程

### 5.1 通用 NixOS suite

| 改动位置或行为 | 构建与运行命令 | 产物、日志与通过判定 |
|---|---|---|
| `distro/**`、NixOS 配置、服务、真实 CLI/用户库流程、网络或文件系统工作流 | `make nixos NIXOS_TEST_SUITE=<suite>`<br>`make run_nixos NIXOS_TEST_SUITE=<suite>` | 镜像：`target/nixos/asterinas.img`。日志：根目录 `qemu.log`、`qemu-serial.log`。所选 suite 的进程退出码为 0，汇总 `Failed: 0`。 |
| 只重跑 suite 内一个用例 | `make run_nixos NIXOS_TEST_SUITE=<suite> NIXOS_TEST_CASE=<case>` | 同上；日志中应只出现所选 case 的执行结果。 |
| 调整 suite 执行时限 | `make run_nixos NIXOS_TEST_SUITE=<suite> NIXOS_TEST_TIMEOUT=<duration>` | 同上；`<duration>` 使用测试框架接受的时间格式。 |

`<suite>` 对应 `test/nixos/tests/<suite>/`。系统测试应在 C 回归已能证明原始 ABI 正确后，用于确认真实用户空间仍能把该 ABI 组合成可用工作流。

### 5.2 Device Mapper 系统测试

Device Mapper 系统测试启动完整 NixOS guest，通过真实 `dmsetup`、LVM2、ext2 和 virtio-blk 测试盘验证用户空间工作流。以下命令的历史实测记录来自项目容器 `myAsterinas` 的 `/root/asterinas`；后续执行应使用 `fork_Asterinas` 容器内的同一路径。

“端到端”必须注明链路边界：control-plane、dataplane 和 LVM2 topology 只分别贯通控制面、裸块数据面和 LVM2 拓扑；验证“LVM2 → DM → ext2 → 文件读写 → 扩缩容 → 跨启动恢复”的完整存储闭环，必须运行 integration selector。

#### 5.2.1 测试分层与验收清单

**基础系统回归**：三项均为真实用户工具到内核的系统测试，但不能单独或合并视为完整存储闭环。

| 测试范围 | Selector | guest 启动 | 测试盘 | 直接命令 | 实际验证边界 | 未覆盖 |
|---|---|---:|---:|---|---|---|
| DM 控制面 | `--control-plane` | 1 次 | 2 块 | `DM_TEST_IMAGE=target/nixos/dm-test/control1.img DM_TEST_IMAGE_2=target/nixos/dm-test/control2.img ./myshell/run_dm_system_tests.sh --control-plane` | 真实 `dmsetup` 的查询、建表、load/resume、suspend/WAIT、rename/setuuid、只读、删除和 table 生命周期。 | LVM2、文件系统、文件持久化、扩缩容和跨启动恢复。 |
| DM 裸块数据面 | `--dataplane` | 1 次 | 3 块 | `DM_TEST_IMAGES='target/nixos/dm-test/data1.img target/nixos/dm-test/data2.img target/nixos/dm-test/data3.img' ./myshell/run_dm_system_tests.sh --dataplane` | linear、striped、mixed、zero/error target 的真实 raw I/O、backing 内容比较、discard 和 zeroout。 | LVM2、mkfs、mount、文件持久化、扩缩容和跨启动恢复。 |
| LVM2 拓扑 | `--lvm2-topology` | 1 次 | 4 块 | `DM_TEST_IMAGES='target/nixos/dm-test/lvm1.img target/nixos/dm-test/lvm2.img target/nixos/dm-test/lvm3.img target/nixos/dm-test/lvm4.img' ./myshell/run_dm_system_tests.sh --lvm2-topology` | 真实 PV/VG/LV、linear/striped/mixed 拓扑、扩缩容、停用、扫描、重新激活、dependencies 和清理。 | ext2、文件内容持久化和跨 guest 启动恢复。 |

**完整存储集成**：复用同一组测试盘启动多个 guest，验证文件系统和数据在有序关机后的恢复。这里的“恢复”不是断电或崩溃一致性测试。

| 测试范围 | Selector | guest 启动 | 测试盘 | 直接命令 | 完整流程与边界 |
|---|---|---:|---:|---|---|
| Linear 完整流程 | `--linear-integration` | 3 次 | 2 块 | `DM_TEST_IMAGE=target/nixos/dm-test/linear1.img DM_TEST_IMAGE_2=target/nixos/dm-test/linear2.img ./myshell/run_dm_system_tests.sh --linear-integration` | 创建 linear LV 和 ext2、文件 MD5、同 PV 扩容、跨 PV 扩容、文件系统扩容；第二次启动恢复并先缩 ext2 再缩 LV；第三次启动验证缩容后的文件与单段拓扑。 |
| Striped 完整流程 | `--striped-integration` | 3 次 | 默认 4 块 | `DM_TEST_IMAGES='target/nixos/dm-test/striped1.img target/nixos/dm-test/striped2.img target/nixos/dm-test/striped3.img target/nixos/dm-test/striped4.img' ./myshell/run_dm_system_tests.sh --striped-integration` | 创建 striped LV 和 ext2、文件 MD5、同 backing set 扩容、跨 backing set 扩容；第二次启动恢复并缩回单段；第三次启动验证缩容后的文件与 striped 拓扑。默认每段 2 路，可通过 `STRIPED_PV_COUNT` 扩展。 |
| Mixed 完整流程 | `--mixed-integration` | 2 次 | 3 块 | `DM_TEST_IMAGES='target/nixos/dm-test/mixed1.img target/nixos/dm-test/mixed2.img target/nixos/dm-test/mixed3.img' ./myshell/run_dm_system_tests.sh --mixed-integration` | 从 linear LV 创建 ext2 和文件，追加 striped segment，验证跨 mixed table 的文件 MD5；第二次启动重新扫描、激活并验证拓扑和文件。当前不覆盖 mixed LV 缩容。 |

验收范围按改动选择：

| 验收目标 | 应运行的 selector | 可以得出的结论 |
|---|---|---|
| 基础系统回归 | `--control-plane`、`--dataplane`、`--lvm2-topology` | 三条真实用户空间链路分别通过；不能表述为完整文件系统存储闭环通过。 |
| 最小完整存储闭环 | 三项基础回归 + `--linear-integration` | 至少一种 linear 的 LVM2、ext2、文件数据、扩缩容和跨启动恢复闭环通过；不能外推到 striped 或 mixed。 |
| 当前 DM V4 完整系统验收 | 六个 selector 全部运行 | 仓库现有基础链路和 linear/striped/mixed 集成流程均通过。共启动 11 次 guest。 |
| stripe 参数扩展验收 | 完整清单 + 额外 `STRIPED_PV_COUNT` / `STRIPED_CHUNK_KIB` 组合 | 只证明实际运行的 N-way 和 chunk 组合；默认 2 路、4 KiB 通过不能外推到所有参数。 |

统一入口一次只接受一个 selector，没有 `--all`。六项必须逐项串行运行，每项结束并确认 QEMU 已退出后再运行下一项。

#### 5.2.2 运行环境与测试盘

| 准备项 | 命令或默认值 | 判定与说明 |
|---|---|---|
| 构建 NixOS 根盘 | `RELEASE=1 make nixos` | release 验收前从待验源码构建并安装内核到根盘；普通调试构建与 release 验收分开记录。已有镜像也必须核对源码版本与构建配置，不能只检查文件存在。 |
| 检查根盘 | `test -f target/nixos/asterinas.img` | 退出码必须为 0；该镜像是 guest 根盘，不能作为测试盘。 |
| 创建测试盘目录 | `mkdir -p target/nixos/dm-test` | 建议把一次性 DM 测试盘集中放在该目录。 |
| 检查并发任务 | `pgrep -af 'qemu-system|cargo osdk test|make run_kernel|make run_nixos'` | 六项测试不能与 QEMU、ktest、C 回归或其他 NixOS 测试并发。已有 QEMU 时脚本会报告 `existing_qemu`。 |
| 默认 guest 就绪时限 | `GUEST_READY_TIMEOUT=40` | 每次 guest shell 未在时限内就绪时失败。 |
| 默认 QEMU 生命周期时限 | `GUEST_QEMU_TIMEOUT=180` | 适用于每次 guest 的启动、命令注入、测试执行和关机。 |

当前六项 DM suite 经 `myshell/lib/dm_nixos_test.sh` 调用 `make run_nixos`，由 `tools/nixos/run.sh` 启动已有根盘；这条路径不自动重新构建或安装当前源码。
因此，在 suite 命令前设置 `RELEASE=1` 不能把已有 dev 镜像变成 release 镜像；必须记录根盘中内核的构建来源、profile，以及 suite 的本轮结果。
根 Makefile 当前默认 `RELEASE=1`；但 suite 不重建根盘，验收依据始终是根盘中内核的实际构建来源与 profile，而不是 Makefile 默认值。

测试盘是 host 上的普通 raw 镜像，由 `tools/nixos/run.sh` 作为独立 virtio-blk 设备附加给 guest。测试会覆盖扇区并创建或清除 LVM 元数据，**绝不能使用 NixOS 根盘、业务盘或需要保留内容的镜像。**

| 配置项 | 写法或默认行为 | 说明 |
|---|---|---|
| 第 1、2 块盘 | `DM_TEST_IMAGE=<path>`、`DM_TEST_IMAGE_2=<path>` | control-plane 和 linear integration 可使用。 |
| 多块测试盘 | `DM_TEST_IMAGES='<path1> <path2> ...'` | 其他 selector 使用；路径以空格分隔，路径本身不能含空白字符。 |
| 自动创建 | 指定尚不存在的路径 | runner 使用 `fallocate` 创建 512 MiB raw 镜像，通常无需提前创建。 |
| 默认重置 | `RESET_DM_TEST_IMAGES=1` | suite 开始前删除指定镜像并创建空白镜像；多 guest integration 在后续启动中复用第一阶段写入的镜像。 |
| 保留文件 | `RESET_DM_TEST_IMAGES=0` | 只避免 suite 开始前删除文件；测试仍会改写镜像内容。 |
| 手动创建 | `fallocate -l 512M <path>` | 需要预先准备镜像时使用。 |
| 安全限制 | 普通文件、路径唯一、不能是 `target/nixos/asterinas.img` | runner 会拒绝根盘、重复路径、非普通文件和含空白字符的路径。 |
| virtio 序列号 | `vdmtest`、`vdmtest2`、`vdmtest3`…… | runner 记录的 host 侧诊断标签，不作为 guest ABI。 |
| guest 设备定位 | `aster-test-disk-locator <正整数序号>` | QEMU 固定 root、通用 fixture 与测试盘的 PCI block 拓扑；NixOS cmdline 以 `aster.test_disk_first=/dev/vde` 声明第 1 块测试盘，locator 按序号推导并验证块设备。以日志中的 `TEST_DISK*=` 为实际设备名。 |

需要手动创建默认最多使用的四块盘时执行：

```bash
mkdir -p target/nixos/dm-test
fallocate -l 512M target/nixos/dm-test/disk1.img
fallocate -l 512M target/nixos/dm-test/disk2.img
fallocate -l 512M target/nixos/dm-test/disk3.img
fallocate -l 512M target/nixos/dm-test/disk4.img
```

正式命令只需传入专用路径；默认重置会重新创建镜像。设置 `RESET_DM_TEST_IMAGES=0` 并不把测试变为只读，也不保护已有数据。

#### 5.2.3 通过判定与日志

| 测试范围 | 必须同时出现的成功标记 | 完整日志 |
|---|---|---|
| DM 控制面 | `SUMMARY_GAP_DM_CONTROL_PLANE: 0`<br>`TEST_PASS_DM_CONTROL_PLANE`<br>`HOST_PASS_DM_CONTROL_PLANE`<br>`HOST_PASS_DM_SYSTEM_TESTS --control-plane` | `/tmp/dm-test/dm-control-plane/latest/` |
| DM 裸块数据面 | `TEST_PASS_DM_DATAPLANE`<br>`HOST_PASS_DM_DATAPLANE`<br>`HOST_PASS_DM_SYSTEM_TESTS --dataplane` | `/tmp/dm-test/dm-dataplane/latest/` |
| LVM2 拓扑 | `SUMMARY_GAP_LVM2_TOPOLOGY: 0`<br>`TEST_PASS_LVM2_TOPOLOGY`<br>`HOST_PASS_LVM2_TOPOLOGY`<br>`HOST_PASS_DM_SYSTEM_TESTS --lvm2-topology` | `/tmp/dm-test/lvm2-topology/latest/` |
| Linear 完整流程 | `TEST_PASS_DM_LINEAR_INTEGRATION_FIRST`<br>`TEST_PASS_DM_LINEAR_INTEGRATION_SECOND`<br>`TEST_PASS_DM_LINEAR_INTEGRATION_THIRD`<br>`HOST_PASS_DM_LINEAR_INTEGRATION`<br>`HOST_PASS_DM_SYSTEM_TESTS --linear-integration` | `/tmp/dm-test/dm-linear-integration/latest/` |
| Striped 完整流程 | `TEST_PASS_DM_STRIPED_INTEGRATION_FIRST`<br>`TEST_PASS_DM_STRIPED_INTEGRATION_SECOND`<br>`TEST_PASS_DM_STRIPED_INTEGRATION_THIRD`<br>`HOST_PASS_DM_STRIPED_INTEGRATION`<br>`HOST_PASS_DM_SYSTEM_TESTS --striped-integration` | `/tmp/dm-test/dm-striped-integration/latest/` |
| Mixed 完整流程 | `TEST_PASS_DM_MIXED_INTEGRATION_FIRST`<br>`TEST_PASS_DM_MIXED_INTEGRATION_SECOND`<br>`HOST_PASS_DM_MIXED_INTEGRATION`<br>`HOST_PASS_DM_SYSTEM_TESTS --mixed-integration` | `/tmp/dm-test/dm-mixed-integration/latest/` |

每条命令退出码必须为 0，本轮日志中不能出现 `TEST_FAIL_*`、`HOST_FAIL_*` 或 panic。结构化日志（各 suite 默认开启，`DM_STRUCT_LOG=0` 回退 legacy 单流 `/tmp/*-test.log`）为每次运行生成独立目录 `/tmp/dm-test/<slug>/<日期-时间>/`，并把 `<slug>/latest` 指向它。目录内四类产物：`console.log`（QEMU/guest 完整原始输出）、`console.clean.log`（剥离转义序列与控制字节的派生视图，可直接 `cat`）、`summary.log`（host 里程碑、`HOST_*`/`TEST_*` 判定与最终聚合标记）、按 guest 分文件的 `qemu-stderr-*`。验收以本轮运行目录为准，不得引用旧目录的成功标记；`latest` 会被下一次运行更新，判定前先核对目录名的时间戳。根目录 `qemu.log`、`qemu-serial.log` 仅用于通用 runner 的补充诊断，不能替代运行目录作为本轮验收依据。快速提取最新一次运行的结果、gap 和实际测试盘：

```bash
run=/tmp/dm-test/dm-control-plane/latest
grep -aE '^(TEST_|HOST_|SUMMARY_GAP_|OBSERVE_|TEST_DISK|DEV[0-9]*=|Kernel panic|panicked)' \
  "$run/summary.log" "$run/console.clean.log"
```

`run_dm_system_tests.sh` 只提供统一 selector 和最终聚合标记；实际测试逻辑由六个子脚本执行。使用 `./myshell/run_dm_system_tests.sh --help` 查看当前 selector，使用具体子脚本的 `--help` 查看参数。

#### 5.2.4 超时与失败定位

仅在当前机器确实需要时临时放宽每次 guest 的超时，例如：

```bash
GUEST_READY_TIMEOUT=60 GUEST_QEMU_TIMEOUT=240 \
  ./myshell/run_dm_system_tests.sh --linear-integration
```

| 现象 | 优先检查 |
|---|---|
| `missing target/nixos/asterinas.img` | 执行 `make nixos`，确认根盘构建成功。 |
| `existing_qemu` 或镜像 write lock | 检查其他 QEMU、ktest、C 回归或 NixOS 测试；保持串行，不终止不属于本轮的进程。 |
| 测试盘被拒绝 | 检查是否误用根盘、重复路径、非普通文件，或路径包含空白字符。 |
| `locate_disk*` / `missing_test_disk*` | 检查 `Attaching Device Mapper test image` 和 `TEST_DISK*=`；确认盘数满足所选 selector。 |
| `guest_ready_timeout` | 查看本轮运行目录的 `summary.log` 与 `console.log` 末尾，确认 guest 是否启动到 shell；不要通过重启容器掩盖启动问题。 |
| `guest_lifecycle_timeout` | 根据 `phase=ready/input/execution_or_shutdown` 判断卡在启动、命令注入、执行还是关机。 |
| integration 第一阶段通过、后续阶段失败 | 不要重建或替换测试盘；检查上一阶段是否有序卸载、停用、关机，以及下一阶段的扫描、激活、table 和文件 MD5 断言。 |
| `OBSERVE_GAP_*` 或 `SUMMARY_GAP_*` 非 0 | 从首个 gap 对应的 `SCENARIO_`、`STATUS_` 和 stdout/stderr 开始定位。 |
| `TEST_FAIL_*`、`HOST_FAIL_*` 或 panic | 以本轮运行目录 `summary.log`、`console.clean.log` 的首个失败为准；必要时再检查仓库根目录 `qemu-serial.log`。 |

### 5.3 ISO 与安装后验证

| 改动位置或行为 | 构建与运行命令 | 产物、日志与通过判定 |
|---|---|---|
| ISO、安装器、发行版引导或安装后配置 | `make iso NIXOS_TEST_SUITE=<suite>`<br>`make run_iso`<br>`make run_nixos NIXOS_TEST_SUITE=<suite>` | ISO 链接：`target/nixos/iso_image/`。安装日志：根目录 `qemu.log`、`qemu-serial.log`。ISO 测试输出 `Congratulations!`，随后安装后的 suite 以 `Failed: 0` 验证。 |

`make run_iso` 完成安装流程；安装完成不等于安装后的系统功能已经验证，因此需要再运行与改动相关的 NixOS suite。
