# OSDK：Asterinas 的构建、启动与 ktest 机制

本文解释 Asterinas 中 OSDK 的职责、`cargo osdk test` 的执行链、参数来源及 ktest 输出位置。文中的路径和行为以当前仓库实现为准。

## 1. 一句话定位

> Cargo 管理 Rust crate；OSDK 复用 Cargo 的构建结果，将 crate 组织成可启动、可在 QEMU 中运行和测试的操作系统。

OSDK 不是 Rust 包管理器，也不替代 Cargo。它解决的是普通用户态 Rust 项目不需要处理的问题：内核入口、链接脚本、启动镜像、bootloader、QEMU、内核测试运行器和 guest 到 host 的测试结果回传。

```text
Cargo workspace / Rust crates
            │
            │  依赖解析、Rust 编译、profile、feature
            ▼
        Cargo build
            │
            │  OSDK 补齐 OS 专属流程
            ▼
OSDK test-base + OSTD + test-kernel
            │
            ▼
      ELF / GRUB ISO
            │
            ▼
          QEMU guest
            │
            ▼
     guest 内执行 ktest
```

## 2. Cargo、crate、module 与 OSDK 的关系

### 2.1 Rust 文件必须属于某个 crate

Rust 文件不会脱离 crate 单独编译。一个 crate 由 `Cargo.toml` 描述，并通过 `mod`、`pub mod` 等模块声明纳入源文件。

例如：

```text
kernel/core/comps/device-mapper/
├── Cargo.toml
└── src/table.rs
```

这是独立的 `aster-device-mapper` crate。因此 `src/table.rs` 中的 ktest 属于该 crate。

而：

```text
kernel/core/
├── Cargo.toml
└── src/device/misc/device_mapper.rs
```

`device_mapper.rs` 没有自己的 `Cargo.toml`，但它是 `aster-core` crate 的一个 module source file。它同样可以定义 ktest；运行 `aster-core` 的 ktest 构建时，该文件会随 `aster-core` 一同参与编译。

### 2.2 OSDK 不要求每个 crate 都由 `cargo osdk new` 创建

`cargo osdk new` 用于创建遵循 OSDK 约定的 OS 项目骨架，或作为新项目的起点。Asterinas 已经是一个 OSDK 项目；其中的 component crate 仍然是普通 Cargo crate，由各自的 `Cargo.toml` 管理。

因此，新增或修改既有组件时，通常不需要重新运行 `cargo osdk new`。关键是该 crate 能被当前 workspace 和 OSDK 测试构建纳入依赖图。

## 3. `cargo osdk test` 实际做了什么

以 Device Mapper component 为例：

```bash
cd /root/asterinas/kernel/core/comps/device-mapper
cargo osdk test
```

执行链可以概括为：

```text
1. 读取当前 crate 与 Cargo metadata
2. 读取并合并 OSDK.toml、环境变量、命令行参数
3. 为被测 crate 生成临时 test-base crate
4. 通过 cargo build 构建测试内核
5. 生成启动产物（当前默认是 GRUB ISO）
6. 用 QEMU 启动测试内核
7. guest 内 test-kernel 发现、过滤并运行 ktest
8. guest 通过 QEMU debug-exit 返回成功或失败状态
9. cargo-osdk 将结果转换为 host 命令退出码
```

重要的是：ktest 的底层构建动作是 `cargo build`，不是普通用户态的 `cargo test`。OSDK 让被测 crate、测试运行器和 OSTD 一起被链接成可启动的测试内核。

主要实现位置：

| 阶段 | 实现位置 |
|---|---|
| `cargo osdk` CLI 入口 | `osdk/src/main.rs`、`osdk/src/cli.rs` |
| `test` 命令调度 | `osdk/src/commands/test.rs` |
| 构建与启动产物 | `osdk/src/commands/build/` |
| QEMU 启动与退出码处理 | `osdk/src/bundle/mod.rs` |
| guest 内 ktest runner | `osdk/deps/test-kernel/src/lib.rs` |

## 4. `#[cfg(ktest)]` 与 `#[ktest]`

常见测试写法：

```rust
#[cfg(ktest)]
mod tests {
    use ostd::prelude::ktest;

    #[ktest]
    fn example() {
        // assertions
    }
}
```

两者分工不同：

| 写法 | 含义 |
|---|---|
| `#[cfg(ktest)]` | 仅在 OSDK 的 ktest 构建中编译这段代码；生产内核构建不会编译它。 |
| `#[ktest]` | 注册具体测试函数，使 guest 内的 ktest runner 能发现并执行它。 |

`cargo osdk test` 会给测试构建增加 `--cfg ktest` 和 `panic=unwind`。因此 `#[cfg(ktest)]` module 会被编译；`#[ktest]` macro 再生成测试登记项。

宏实现位于 `ostd/libs/ostd-macros/src/lib.rs`。每个 `#[ktest]` 会生成一个 `KtestItem`，记录测试函数指针、crate 名、module path、函数名、源代码位置和 `should_panic` 预期等信息。

这些 `KtestItem` 被放入 ELF 的 `.ktest_array` section。链接脚本保留该 section，测试运行器在 guest 中遍历它，从而发现所有已注册测试。

## 5. 测试运行与过滤

guest 启动后，`osdk-test-kernel` 创建测试 task，读取 `.ktest_array`，逐项运行测试，并捕获可恢复的 panic。

```text
读取所有 KtestItem
        │
        ├─ 按 crate 白名单过滤
        ├─ 按 TESTNAME 过滤
        ├─ 运行测试
        ├─ 捕获 panic / 判断 should_panic
        └─ 输出 ok、FAILED 与汇总
```

每个普通 panic 通常只使该测试标记为 `FAILED`，不会立刻终止同一 crate 中的后续测试；但不可恢复的内核错误、double panic 或启动异常仍可能直接终止 guest。

### 5.1 TESTNAME 过滤

命令形式：

```bash
cargo osdk test [TESTNAME]
```

例如，在 `kernel/core` 下运行 Device Mapper core module 的全部 ktest：

```bash
cd /root/asterinas/kernel/core
cargo osdk test --kcmd-args=earlycon device_mapper::tests
```

该参数不是内核命令行参数，也不是普通 libtest 参数。OSDK 将它编译进测试内核的白名单。

过滤按 `::` 分段的路径后缀匹配。它既可以匹配具体测试的完整路径，也可以匹配 module path，从而选中该 module 下的全部测试。假设测试全名是：

```text
aster_device_mapper::table::tests::maps_bio
```

下列 filter 可匹配：

```text
maps_bio
tests::maps_bio
table::tests::maps_bio
aster_device_mapper::table::tests::maps_bio
tests
table::tests
```

其中前四项选择具体测试，后两项选择对应 module 下的全部测试。`maps` 或单独的 `table` 不能视为可靠匹配。

> 注意：filter 没匹配任何测试时，命令可能仍成功退出。因此使用 filter 时，必须检查日志中的实际执行数和 `filtered out` 数，不能只看 host 退出码。

## 6. 参数分层

OSDK 相关参数至少分为四层，避免把它们混为一谈。

| 层次 | 示例 | 控制对象 | 主要定义位置 |
|---|---|---|---|
| 项目 OSDK 配置 | boot method、默认 QEMU args | OSDK 的默认构建/运行方式 | `OSDK.toml` |
| OSDK CLI 参数 | `--release`、`--qemu-args`、`TESTNAME` | 当前这次构建或测试 | `osdk/src/cli.rs` |
| host 环境变量 | `CONSOLE`、`ENABLE_KVM`、`MEM`、`SMP`、`OVMF` | `qemu_args.sh` 生成的 QEMU argv | `tools/qemu_args.sh` |
| guest 内核命令行 | `earlycon`、`loglevel=error` | guest 内核启动和日志行为 | `--kcmd-args` / boot 配置 |

直接查看当前安装版本支持的参数：

```bash
cargo osdk --help
cargo osdk test --help
```

源码中 CLI 参数主要定义在 `osdk/src/cli.rs`。

## 7. 常用参数

### 7.1 `--release`

```bash
cargo osdk test --release
```

使用 release profile 构建测试内核；不写则使用 dev profile。该参数影响优化和编译 profile，不改变测试选择范围。

### 7.2 `ENABLE_KVM` 与 `--qemu-args`

根 `OSDK.toml` 使用 `tools/qemu_args.sh` 生成 x86 OSDK 的 QEMU 参数。该脚本默认：

```bash
ENABLE_KVM=1
```

因此直接运行：

```bash
cargo osdk test
cargo osdk run
```

在使用 `normal`、`test`、`microvm`、`iommu` 或 `tdx` scheme 时，默认会包含：

```text
-accel kvm
```

需要显式切换到 TCG 时使用：

```bash
ENABLE_KVM=0 cargo osdk test
```

`ENABLE_KVM=0` 保留给没有 `/dev/kvm` 的环境，以及需要验证非 KVM 路径的场景。TDX 依赖 KVM，不支持这个关闭方式；RISC-V scheme 也不会加入 x86 的 KVM 参数。

`--qemu-args` 用于临时追加或覆盖 QEMU 参数，例如：

```bash
cargo osdk test --qemu-args="-m 4G"
```

日常运行不需要再手动追加 `--qemu-args="-accel kvm"`。手写 `qemu-system-* ...` 的命令不经过 `tools/qemu_args.sh`，仍必须自行决定是否传入 `-accel kvm`。

### 7.3 `TESTNAME`

```bash
cargo osdk test device_mapper::tests::some_test
```

选择路径后缀匹配的 ktest，仅适合定向排障。完整 crate 回归应不带 filter 执行。

### 7.4 `--kcmd-args`

```bash
cargo osdk test --kcmd-args="earlycon"
```

追加 guest 内核命令行参数。常见示例：

```bash
--kcmd-args="earlycon"
--kcmd-args="loglevel=error"
--kcmd-args="console=ttyS0"
```

其中 `earlycon` 控制 early console，`loglevel=error` 降低常规内核日志噪声，`console=ttyS0` 控制 guest 内核的常规 console。

### 7.5 `--initramfs`

```bash
cargo osdk test \
  --initramfs=/root/asterinas/test/initramfs/build/initramfs.cpio.gz
```

指定测试内核使用的 initramfs。纯组件级 ktest 是否需要它，取决于测试的依赖和仓库当前约定。

### 7.6 boot 相关参数

例如：

```bash
--boot-method=grub-rescue-iso
--grub-boot-protocol=multiboot2
```

当前仓库根 `OSDK.toml` 已提供默认 boot method 和 protocol。日常手动测试不应重复填写，更不应为了临时排障擅自改变启动协议或启动方式。

## 8. `CONSOLE`、`--kcmd-args` 与日志位置

这三者最容易混淆。

### 8.1 `CONSOLE=hvc0` 与 `CONSOLE=ttyS0`

`CONSOLE` 是 host 环境变量，由 `tools/qemu_args.sh` 读取，用于决定 QEMU 怎样连接 guest 的 UART 和 virtio console。ktest runner 通过 UART 输出测试结果。

| host 设置 | QEMU 的 UART 后端 | ktest 原始输出位置 | 当前终端能否直接看到 ktest | 实时查看方式 | 适用情况 |
|---|---|---|---|---|---|
| `CONSOLE=hvc0`（默认） | `-serial file:qemu-serial.log` | `/root/asterinas/qemu-serial.log` | 通常不能；终端主要连接 virtio console/mux | `tail -f /root/asterinas/qemu-serial.log` | 默认运行；原始 UART 日志单独保存，避免终端混杂大量启动输出。 |
| `CONSOLE=ttyS0` | `-serial chardev:mux` | 当前终端和 `/root/asterinas/qemu.log` | 可以 | 直接观察终端，或 `tail -f /root/asterinas/qemu.log` | 手动观察、交互式排障或希望实时阅读 ktest 结果。 |

示例：

```bash
# 默认：ktest 结果写入 qemu-serial.log
cargo osdk test

# UART 接到终端，并同时记录到 qemu.log
CONSOLE=ttyS0 cargo osdk test
```

> `CONSOLE=ttyS0` 是 host 侧 QEMU 连接配置；`--kcmd-args="console=ttyS0"` 是 guest 内核命令行参数。两者不能互相替代。

### 8.3 三类日志

| 文件 | 生成者 | 用途 |
|---|---|---|
| `/root/asterinas/qemu-serial.log` | QEMU 的 serial file backend | 默认 `hvc0` 模式下的原始 UART 输出，通常包含 ktest 结果。 |
| `/root/asterinas/qemu.log` | QEMU stdio mux 的 logfile | 终端/mux 通道日志；`ttyS0` 配置下通常包含 UART/ktest 输出。 |

## 9. 启动产物、QEMU 与退出状态

当前项目默认使用 GRUB rescue ISO 与 multiboot2。测试时看到：

```text
ISO image produced
Writing to ... completed successfully
```

只表示 ISO 制作成功，不能表示 ktest 已经开始、执行完成或通过。必须继续检查 guest 测试摘要。

测试结束后，guest 的 test-kernel 通过 x86 QEMU 的 `isa-debug-exit` 机制关机并传回状态。cargo-osdk 将 QEMU 的状态转换为 host 侧状态：

| `cargo osdk test` 返回值 | 含义 |
|---:|---|
| `0` | 测试运行并通过 |
| `1` | ktest 断言失败 |
| `2` | QEMU、启动链或其他未知失败 |

因此必须同时核对：

1. guest 日志中实际运行了预期测试；
2. `test result` 显示 `0 failed`；
3. 出现 `All crates tested.`；
4. host 命令退出状态为 `0`。

## 10. Device Mapper 的两个 ktest 入口

### 10.1 component crate 全量 ktest

```bash
cd /root/asterinas/kernel/core/comps/device-mapper
cargo osdk test
```

被测 crate：`aster-device-mapper`。

主要覆盖可复用 Device Mapper 组件内部语义：target 参数、table 映射、BIO split、completion aggregation、manager 索引、device 状态机等。

### 10.2 `aster-core` 中的 Device Mapper 定向 ktest

```bash
cd /root/asterinas/kernel/core
cargo osdk test \
  aster_core::device::misc::device_mapper::tests
```

被测 crate：`aster-core`；filter 选择其中 `device_mapper.rs` module 的测试。

主要覆盖 core 整合层的内部 façade：table active/inactive 状态、header/flags、runtime node 与 mapper alias、rename/setuuid、event/wait、publication rollback 等。

它们都是内核内部测试，不等同于 raw `/dev/mapper/control` ioctl 用户态 E2E；后者应由 initramfs C regression 或 NixOS dmsetup/LVM2 system suite 证明。

## 11. 日常使用建议

完整 component crate 回归：

```bash
cd /root/asterinas/kernel/core/comps/device-mapper
cargo osdk test
```

定向排障：

```bash
cd /root/asterinas/kernel/core/comps/device-mapper
cargo osdk test --qemu-args="-accel kvm" \
  table::tests::splits_twelve_sector_write_across_four_striped_children
```

默认 `hvc0` 配置下，在另一个终端看原始结果：

```bash
tail -f /root/asterinas/qemu-serial.log
```

不要把以下证据混为一谈：

- 测试源码存在；
- 测试成功编译；
- QEMU 成功启动；
- guest 实际执行了预期测试；
- guest 测试全部通过；
- host 命令返回 0；
- 历史 QEMU 日志中的旧结果。

只有当前命令产生的日志和退出状态，才能证明当前一轮测试结果。
