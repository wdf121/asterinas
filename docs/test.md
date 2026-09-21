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

## 4. initramfs：启动与原始用户 ABI 回归

### 4.1 按改动位置或行为执行

| 改动位置或行为 | 命令 | 结果日志与通过判定 | 升级条件 |
|---|---|---|---|
| 启动协议、早期初始化、rootfs、initramfs 可用性 | `make run_kernel AUTO_TEST=boot` | 根目录 `qemu.log` 最后 100 行；命令成功且含 `Successfully booted.`。 | 启动后用户 ABI 也变化时，执行相关 regression selector。 |
| 一个 regression 目录，例如设备、文件系统或进程类别 | `make run_kernel AUTO_TEST=regression REGRESSION_TESTS=<directory>` | 根目录 `qemu.log` 最后 100 行；每个目录完成且最终含 `All regression tests passed.`。 | 该 ABI 被真实工具、服务或发行版配置使用时，执行 NixOS suite。 |
| 单个 C ELF | `make run_kernel AUTO_TEST=regression REGRESSION_TESTS=<directory>/<binary>` | 同上；输出应包含该 ELF 的运行与成功信息，以及最终回归成功标记。 | 同上。 |
| 修改多个类别或准备全量基础回归 | `make run_kernel AUTO_TEST=regression` | 同上；省略 `REGRESSION_TESTS` 会遍历 `/test` 下的全部一级测试目录。 | 对真实发行版行为继续 NixOS。 |

`REGRESSION_TESTS` 可以给出一个或多个相对 selector。目录 selector 执行该目录的 `run_test.sh`；ELF selector 直接执行对应测试程序。不要传绝对路径、`.`、路径穿越或包含空格的 selector。

### 4.2 C 程序、shell runner 与日志

C ELF 负责构造 syscall/ioctl 输入、检查返回值和 errno，并以非零退出码报告失败；目录的 `run_test.sh` 负责按顺序调度多个 ELF，并用 `set -e` 传播失败。二者不是互相替代关系。

根 `Makefile` 以根目录 `qemu.log` 的最后 100 行判断启动和 regression 结果；`qemu-serial.log` 是 UART 输出副本，适合辅助排查，但不是该判定的来源。

## 5. NixOS 与 ISO：完整用户空间流程

### 5.1 NixOS suite

| 改动位置或行为 | 构建与运行命令 | 产物、日志与通过判定 |
|---|---|---|
| `distro/**`、NixOS 配置、服务、真实 CLI/用户库流程、网络或文件系统工作流 | `make nixos NIXOS_TEST_SUITE=<suite>`<br>`make run_nixos NIXOS_TEST_SUITE=<suite>` | 镜像：`target/nixos/asterinas.img`。日志：根目录 `qemu.log`、`qemu-serial.log`。所选 suite 的进程退出码为 0，汇总 `Failed: 0`。 |
| 只重跑 suite 内一个用例 | `make run_nixos NIXOS_TEST_SUITE=<suite> NIXOS_TEST_CASE=<case>` | 同上；日志中应只出现所选 case 的执行结果。 |
| 调整 suite 执行时限 | `make run_nixos NIXOS_TEST_SUITE=<suite> NIXOS_TEST_TIMEOUT=<duration>` | 同上；`<duration>` 使用测试框架接受的时间格式。 |

`<suite>` 对应 `test/nixos/tests/<suite>/`。系统测试应在 C 回归已能证明原始 ABI 正确后，用于确认真实用户空间仍能把该 ABI 组合成可用工作流。

### 5.2 ISO 与安装后验证

| 改动位置或行为 | 构建与运行命令 | 产物、日志与通过判定 |
|---|---|---|
| ISO、安装器、发行版引导或安装后配置 | `make iso NIXOS_TEST_SUITE=<suite>`<br>`make run_iso`<br>`make run_nixos NIXOS_TEST_SUITE=<suite>` | ISO 链接：`target/nixos/iso_image/`。安装日志：根目录 `qemu.log`、`qemu-serial.log`。ISO 测试输出 `Congratulations!`，随后安装后的 suite 以 `Failed: 0` 验证。 |

`make run_iso` 完成安装流程；安装完成不等于安装后的系统功能已经验证，因此需要再运行与改动相关的 NixOS suite。
