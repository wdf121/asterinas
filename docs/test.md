# Asterinas 开发测试手册

本文用于把**改动位置或行为**转换为可执行的测试动作：

```text
改动位置或行为 → 最低测试层 → 命令 → 日志 → 通过判定 → 是否升级到下一层
```

命令默认在仓库根目录执行。修改先通过静态准入；随后从最能定位问题的运行时层开始，不以更高层测试替代更低层的定点断言。

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

## 3. ktest：内核内部逻辑

以下命令均已在项目容器内按所列执行目录实测通过，使用默认 dev 构建和本地依赖缓存。执行目录相对于容器仓库根目录 `/root/asterinas`；未实测的根 workspace 和 core 全量入口不列入此表。无 selector 的命令按当前目录选择测试 crate，执行前必须确认目录。

**默认回归清单**：DM、block 各运行一次 crate 全量，core 运行以下三个模块。

| 测试范围 | 执行目录 | 直接命令 | 覆盖范围 / 文件 |
|---|---|---|---|
| `aster-device-mapper` 全量 | `kernel/core/comps/device-mapper` | `CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test` | `device.rs`、`manager.rs`、`table.rs`、`target/**`。 |
| core：DM ioctl / runtime 模块 | `kernel/core` | `CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test --kcmd-args=earlycon aster_core::device::misc::device_mapper::tests` | `kernel/core/src/device/misc/device_mapper.rs`。 |
| core：runtime block registry 模块 | `kernel/core` | `CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test --kcmd-args=earlycon aster_core::device::registry::block::tests` | `kernel/core/src/device/registry/block.rs`。 |
| `aster-block` 全量 | `kernel/core/comps/block` | `CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test` | BIO、注册/注销与 lease、device ID、partition、request queue。 |
| core：动态设备路径模块 | `kernel/core` | `CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test --kcmd-args=earlycon aster_core::device::tests` | `kernel/core/src/device/mod.rs`：动态 devtmpfs 路径校验。 |

**定向排障入口**：以下模块已包含在对应 crate 全量中；仅在定位失败或局部复测时使用，不需要在全量通过后逐条重复执行。

| 测试范围 | 执行目录 | 直接命令 | 覆盖范围 / 文件 |
|---|---|---|---|
| DM component：table 模块 | `kernel/core/comps/device-mapper` | `CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test aster_device_mapper::table::tests` | `kernel/core/comps/device-mapper/src/table.rs`。 |
| DM component：manager 模块 | `kernel/core/comps/device-mapper` | `CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test aster_device_mapper::manager::tests` | `kernel/core/comps/device-mapper/src/manager.rs`。 |
| block component：BIO 模块 | `kernel/core/comps/block` | `CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test aster_block::bio::tests` | `kernel/core/comps/block/src/bio.rs`。 |
| block component：注册与 lease | `kernel/core/comps/block` | `CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test aster_block::tests` | `kernel/core/comps/block/src/lib.rs`：注册、注销、lease 与事务回滚。 |
| block component：device ID 模块 | `kernel/core/comps/block` | `CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test aster_block::device_id::tests` | `kernel/core/comps/block/src/device_id.rs`：major 快照与持有期。 |
| block component：partition 模块 | `kernel/core/comps/block` | `CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test aster_block::partition::tests` | `kernel/core/comps/block/src/partition.rs`：range BIO 偏移与溢出。 |
| block component：request queue 模块 | `kernel/core/comps/block` | `CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test aster_block::request_queue::tests` | `kernel/core/comps/block/src/request_queue.rs`：range 合并与 segment 上限。 |

以下 console/日志说明适用于当前默认 x86_64、非 TDX 配置。ktest 结果经 early serial 输出，不能与普通内核的 `/dev/console` 选择混为一谈。

| 参数 / 日志 | 默认或写法 | 作用 | 手动查看 |
|---|---|---|---|
| `CONSOLE=hvc0` | 未显式设置 `CONSOLE` 时默认。 | virtconsole 接终端；ktest 结果走独立 UART，不在终端显示。 | `less qemu-serial.log` |
| `CONSOLE=ttyS0` | `make ktest` 默认；手动 ktest 表中显式指定。 | UART 接终端；ktest 结果同时显示在终端并写入 `qemu.log`。 | `less qemu.log` |
| 模块 selector | `crate::...::tests` | 一次选择该模块中的全部 `#[ktest]`；不需要逐个填写函数名。 | 在仓库根目录按上述 console 设置查看对应日志。 |
| 无 selector | 上表 DM、block crate 全量命令。 | 执行当前目录所选 crate 在当前构建配置下的全部 ktest；不代表 core 或 workspace 全量已验证。 | 同上。 |
| 离线依赖 | `CARGO_NET_OFFLINE=true` | 仅使用本地缓存；缓存缺失时报错，不自动联网拉取。 | 命令终端输出。 |
| core early console | `--kcmd-args=earlycon` | 启用 core 的早期串口；只设置 `CONSOLE=ttyS0` 不会启用它。 | 结果输出到终端和根目录 `qemu.log`。 |

core 链入的早期参数解析器默认关闭 early console；未传 `earlycon` 时，ktest 的 `early_print!` 输出会被丢弃。上表 core 模块均使用带 `--kcmd-args=earlycon` 的命令实测通过。DM/block 使用 OSTD 默认开启 early console 的解析器，保留其已实测通过的原命令，不额外添加参数。

上表所列测试入口均使用仓库根目录的 OSDK manifest，QEMU 日志因此位于**仓库根目录**，不是调用命令的 crate 目录；查看日志的命令应在仓库根目录执行。`hvc0` 下的 `qemu.log` 记录 virtconsole/终端 mux 输出，不是 ktest UART 结果日志；`ttyS0` 下不会为本轮创建 `qemu-serial.log`，已有同名文件可能是旧日志。

## 4. initramfs C 回归：原始用户 ABI

C 回归用于验证真实用户进程能否通过 syscall、ioctl、设备节点和 VFS 正确使用内核接口。测试源码先被编译为用户态 ELF，再与 shell runner 一起打包进 initramfs；QEMU 启动 Asterinas 后，由 guest 中的 regression runner 执行 ELF。它与直接运行内核内部 Rust 断言的 ktest 分工不同，不能互相替代。

```text
C 测试源码
  → 编译为用户态 ELF
  → 打包进 initramfs
  → QEMU 启动 Asterinas guest
  → /init 启动 regression runner
  → runner 执行指定 ELF
  → ELF 通过 libc/syscall 进入内核
```

### 4.1 执行环境与准备

以下命令已在项目容器 `myAsterinas` 的 `/root/asterinas` 中实测。除非命令明确包含 `docker exec`，本节命令均从该容器内的仓库根目录执行。

运行前确认没有其他 QEMU、ktest 或 NixOS 测试占用测试镜像；这些测试必须串行运行。可在宿主机检查：

```bash
pgrep -af 'qemu-system|cargo osdk test|make run_kernel'
```

若当前位于宿主机，可进入项目容器：

```bash
docker exec -it -w /root/asterinas myAsterinas bash
```

### 4.2 Device Mapper focused C 回归

在容器内的 `/root/asterinas` 执行最小必要命令：

```bash
AUTO_TEST=regression \
  REGRESSION_TESTS=device/device_mapper make run_kernel
```

其中，`AUTO_TEST=regression` 启用 C regression 的构建和 guest runner；`REGRESSION_TESTS=device/device_mapper` 让 runner 直接执行 initramfs 中的 `/test/device/device_mapper` 用户态 ELF。这两个参数是 focused Device Mapper C 回归的必要参数。

若需要固定为本次已验证的离线、release、非 TDX 配置，使用完整复现命令：

```bash
CARGO_NET_OFFLINE=true RELEASE=1 AUTO_TEST=regression \
  INTEL_TDX=0 REGRESSION_TESTS=device/device_mapper make run_kernel
```

参数的必要性与作用如下：

| 参数 | 必要性 | 作用 |
|---|---|---|
| `AUTO_TEST=regression` | 必须显式设置；默认值为 `none`。 | 构建 regression C ELF，并让 guest 启动后进入 initramfs regression runner。 |
| `REGRESSION_TESTS=device/device_mapper` | focused 回归必须设置；省略时运行全量 regression。 | 让 runner 直接执行 `/test/device/device_mapper`。 |
| `CARGO_NET_OFFLINE=true` | 可选。 | Cargo 仅使用本地依赖缓存，缓存缺失时直接报错。 |
| `RELEASE=1` | 可选；当前默认值就是 `1`。 | 明确使用 release 配置，避免外部环境覆盖默认值。 |
| `INTEL_TDX=0` | 可选；当前默认值就是 `0`。 | 明确使用非 TDX 配置，避免外部环境覆盖默认值。 |
| `make run_kernel` | 必须执行的 Make target。 | 构建内核和 initramfs，启动 QEMU，并等待 guest 回归结束。 |

对应源码为 `test/initramfs/src/regression/device/device_mapper.c`。该回归覆盖：

| 覆盖范围 | 主要验证内容 |
|---|---|
| tableless DM ioctl | `DM_VERSION`、create、status、table status、remove 的结构体布局、flag、返回值和 errno。 |
| runtime 节点事务 | `/dev/dm-N` 与 `/dev/mapper/<name>` 的创建，以及路径冲突后的回滚和重试。 |
| mount lease | ext2 挂载期间删除 mapper 返回 `EBUSY`，卸载后可以删除。 |
| block range ioctl | `BLKDISCARD`、`BLKZEROOUT` 的合法范围、对齐、溢出、越界和零长度边界。 |
| WAIT 中断 | 未设置 `SA_RESTART` 时，信号中断最终向用户态返回 `EINTR`；设备不存在时返回对应 errno。 |
| WAIT 自动重启 | 设置 `SA_RESTART` 后，signal handler 实际执行，原 ioctl 不提前返回，并在 rename 事件后成功完成。 |
| WAIT 事件唤醒 | rename 和 setuuid 推进 event，唤醒 waiter，并正确回填 `dm_ioctl` header。 |

该入口验证原始用户 ABI，不替代 DM/block crate ktest，也不替代使用 `dmsetup`、LVM2 和真实发行版用户空间的 NixOS 系统测试。

### 4.3 日志查看与通过判定

QEMU 的主日志写入仓库根目录 `qemu.log`。命令运行时可以直接观察终端；运行结束后，在容器内的 `/root/asterinas` 查看：

```bash
less qemu.log
```

只提取 Device Mapper 回归的执行结果：

```bash
grep -E 'Running test /test/device/device_mapper|summary:|test result:|Test /test/device/device_mapper passed\.|All regression tests passed\.|failed' qemu.log
```

需要排查早期启动、串口或主日志缺失问题时，再查看 UART 副本：

```bash
less qemu-serial.log
```

一次成功的 focused Device Mapper C 回归必须同时满足：

1. `make run_kernel` 退出码为 0；
2. 各测试函数的局部 summary 均为 `0 tests failed`；
3. ELF 的累计汇总为 `test result: ok. 159 passed; 0 failed`；
4. 日志包含：

```text
Test /test/device/device_mapper passed.
All regression tests passed.
```

若 `make run_kernel` 失败，以本次命令生成的终端输出和 `qemu.log` 为准。不要用旧日志中的成功标记判断本轮结果。

### 4.4 其他 C 回归入口

| 测试范围 | 容器内仓库根目录执行的命令 | 通过判定 |
|---|---|---|
| 启动协议、早期初始化、rootfs、initramfs 可用性 | `CARGO_NET_OFFLINE=true RELEASE=1 AUTO_TEST=boot INTEL_TDX=0 make run_kernel` | 命令退出码为 0，根目录 `qemu.log` 包含 `Successfully booted.`。 |
| 一个 regression 目录 | `CARGO_NET_OFFLINE=true RELEASE=1 AUTO_TEST=regression INTEL_TDX=0 REGRESSION_TESTS=<directory> make run_kernel` | 所选目录完成，最终包含 `All regression tests passed.`。 |
| 单个 C ELF | `CARGO_NET_OFFLINE=true RELEASE=1 AUTO_TEST=regression INTEL_TDX=0 REGRESSION_TESTS=<directory>/<binary> make run_kernel` | 对应 ELF 成功结束，最终包含 `All regression tests passed.`。 |
| 全量 initramfs regression | `CARGO_NET_OFFLINE=true RELEASE=1 AUTO_TEST=regression INTEL_TDX=0 make run_kernel` | 遍历 `/test` 下全部一级测试目录，最终包含 `All regression tests passed.`。 |

`REGRESSION_TESTS` 接受相对 selector。目录 selector 执行该目录的 `run_test.sh`，ELF selector 直接执行对应程序；不要传绝对路径、`.`、路径穿越或包含空格的 selector。目录 `run_test.sh` 使用 `set -e` 传播失败，C ELF 通过非零退出码报告失败。

### 4.5 常见失败定位

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

Device Mapper 系统测试启动完整 NixOS guest，通过真实 `dmsetup`、LVM2、ext2 和 virtio-blk 测试盘验证用户空间工作流。以下命令均从项目容器 `myAsterinas` 内的 `/root/asterinas` 执行。

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
| 构建 NixOS 根盘 | `make nixos` | 首次运行或根盘不存在时执行。 |
| 检查根盘 | `test -f target/nixos/asterinas.img` | 退出码必须为 0；该镜像是 guest 根盘，不能作为测试盘。 |
| 创建测试盘目录 | `mkdir -p target/nixos/dm-test` | 建议把一次性 DM 测试盘集中放在该目录。 |
| 检查并发任务 | `pgrep -af 'qemu-system|cargo osdk test|make run_kernel|make run_nixos'` | 六项测试不能与 QEMU、ktest、C 回归或其他 NixOS 测试并发。已有 QEMU 时脚本会报告 `existing_qemu`。 |
| 默认 guest 就绪时限 | `GUEST_READY_TIMEOUT=40` | 每次 guest shell 未在时限内就绪时失败。 |
| 默认 QEMU 生命周期时限 | `GUEST_QEMU_TIMEOUT=180` | 适用于每次 guest 的启动、命令注入、测试执行和关机。 |

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
| virtio 序列号 | `vdmtest`、`vdmtest2`、`vdmtest3`…… | 测试盘按传入顺序编号。 |
| guest 设备定位 | `aster-dm-disk-locator [serial]` | 测试不依赖固定 `/dev/vdX`；以日志中的 `TEST_DISK*=` 为实际设备名。 |

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
| DM 控制面 | `SUMMARY_GAP_DM_CONTROL_PLANE: 0`<br>`TEST_PASS_DM_CONTROL_PLANE`<br>`HOST_PASS_DM_CONTROL_PLANE`<br>`HOST_PASS_DM_SYSTEM_TESTS --control-plane` | `/tmp/dm-control-plane-test.log` |
| DM 裸块数据面 | `TEST_PASS_DM_DATAPLANE`<br>`HOST_PASS_DM_DATAPLANE`<br>`HOST_PASS_DM_SYSTEM_TESTS --dataplane` | `/tmp/dm-dataplane-test.log` |
| LVM2 拓扑 | `SUMMARY_GAP_LVM2_TOPOLOGY: 0`<br>`TEST_PASS_LVM2_TOPOLOGY`<br>`HOST_PASS_LVM2_TOPOLOGY`<br>`HOST_PASS_DM_SYSTEM_TESTS --lvm2-topology` | `/tmp/lvm2-topology-test.log` |
| Linear 完整流程 | `TEST_PASS_DM_LINEAR_INTEGRATION_FIRST`<br>`TEST_PASS_DM_LINEAR_INTEGRATION_SECOND`<br>`TEST_PASS_DM_LINEAR_INTEGRATION_THIRD`<br>`HOST_PASS_DM_LINEAR_INTEGRATION`<br>`HOST_PASS_DM_SYSTEM_TESTS --linear-integration` | `/tmp/dm-linear-integration-test.log` |
| Striped 完整流程 | `TEST_PASS_DM_STRIPED_INTEGRATION_FIRST`<br>`TEST_PASS_DM_STRIPED_INTEGRATION_SECOND`<br>`TEST_PASS_DM_STRIPED_INTEGRATION_THIRD`<br>`HOST_PASS_DM_STRIPED_INTEGRATION`<br>`HOST_PASS_DM_SYSTEM_TESTS --striped-integration` | `/tmp/dm-striped-integration-test.log` |
| Mixed 完整流程 | `TEST_PASS_DM_MIXED_INTEGRATION_FIRST`<br>`TEST_PASS_DM_MIXED_INTEGRATION_SECOND`<br>`HOST_PASS_DM_MIXED_INTEGRATION`<br>`HOST_PASS_DM_SYSTEM_TESTS --mixed-integration` | `/tmp/dm-mixed-integration-test.log` |

每条命令退出码必须为 0，本轮日志中不能出现 `TEST_FAIL_*`、`HOST_FAIL_*` 或 panic。每个 suite 只有一个权威的持久 `LOG`：其中按实际顺序同时记录 host 的 `HOST_*` 事件、QEMU/guest 完整输出、`TEST_*` 结果和最终聚合标记。运行期间的 FIFO 会在 suite 退出后自动删除；正常路径不创建 `*-qemu-running.txt`。根目录 `qemu.log`、`qemu-serial.log` 仅用于通用 runner 的补充诊断，不能替代 suite `LOG` 作为本轮验收依据。快速提取六类日志中的结果、gap 和实际测试盘：

```bash
grep -aE '^(TEST_|HOST_|SUMMARY_GAP_|OBSERVE_|TEST_DISK|DEV[0-9]*=|Kernel panic|panicked)' \
  /tmp/dm-*-test.log /tmp/lvm2-topology-test.log
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
| `guest_ready_timeout` | 查看对应 `/tmp/*.log`，确认 guest 是否启动到 shell；不要通过重启容器掩盖启动问题。 |
| `guest_lifecycle_timeout` | 根据 `phase=ready/input/execution_or_shutdown` 判断卡在启动、命令注入、执行还是关机。 |
| integration 第一阶段通过、后续阶段失败 | 不要重建或替换测试盘；检查上一阶段是否有序卸载、停用、关机，以及下一阶段的扫描、激活、table 和文件 MD5 断言。 |
| `OBSERVE_GAP_*` 或 `SUMMARY_GAP_*` 非 0 | 从首个 gap 对应的 `SCENARIO_`、`STATUS_` 和 stdout/stderr 开始定位。 |
| `TEST_FAIL_*`、`HOST_FAIL_*` 或 panic | 以本轮 `/tmp/*.log` 的首个失败为准；必要时再检查仓库根目录 `qemu-serial.log`。 |

### 5.3 ISO 与安装后验证

| 改动位置或行为 | 构建与运行命令 | 产物、日志与通过判定 |
|---|---|---|
| ISO、安装器、发行版引导或安装后配置 | `make iso NIXOS_TEST_SUITE=<suite>`<br>`make run_iso`<br>`make run_nixos NIXOS_TEST_SUITE=<suite>` | ISO 链接：`target/nixos/iso_image/`。安装日志：根目录 `qemu.log`、`qemu-serial.log`。ISO 测试输出 `Congratulations!`，随后安装后的 suite 以 `Failed: 0` 验证。 |

`make run_iso` 完成安装流程；安装完成不等于安装后的系统功能已经验证，因此需要再运行与改动相关的 NixOS suite。
