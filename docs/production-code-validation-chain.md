# 生产代码修改后的验证命令与执行链路

## 执行摘要

本文记录写完生产代码后常用的验证命令，以及这些命令背后实际执行的链路。命令默认在 Docker 容器内、仓库根目录 `/root/asterinas` 下执行，因此所有示例都直接从仓库根目录开始。

验证不是为了“跑一个命令看绿不绿”，而是为了知道每一层在证明什么：

- `cargo fmt --all --check` 证明 Rust 代码格式没有偏离 rustfmt 输出。
- `git diff --check` 证明当前 diff 没有 Git 能识别的空白错误和冲突 marker。
- `make ktest` 证明被选中的 kernel crate 能被 OSDK 构造成测试内核，并在 QEMU 中由 ktest runner 执行。
- DM 系统测试证明真实 NixOS guest 内的 `dmsetup`、LVM2、文件系统和真实 block device I/O 链路可用。

掌握链路后，遇到“慢”或“失败”时就能拆阶段定位：是 Makefile 参数问题、Cargo 构建问题、OSDK bundle 问题、QEMU/KVM 问题、kernel boot 问题，还是 ktest/system test 本身的问题。

## 1. 基础验证命令

### 1.1 Rust 格式检查

```bash
cargo fmt --all --check
```

目的：确认 Rust 源码与 Rust 官方格式化工具 `rustfmt` 的规则一致。

`rustfmt` 是 Rust 官方提供的代码格式化工具，通常作为 Rust toolchain 的一个组件随 `rustup` 管理。它不负责判断业务逻辑是否正确，而是把 Rust 源码解析后按统一规则重新排版，例如缩进、换行、空格、链式调用换行、`use` 分组等。

依赖关系：

```text
cargo fmt
  → 使用当前项目选中的 Rust toolchain
  → 调用该 toolchain 中的 rustfmt 组件
  → rustfmt 解析 Rust 源码并生成规范格式
  → --check 模式下只比较结果，不写回文件
```

这里的“当前项目选中的 Rust toolchain”通常由 `rust-toolchain.toml`、`rust-toolchain` 或当前 shell 的 `rustup default` 决定。因此不是随便安装一个 stable/nightly 就一定够，而是要安装项目实际使用的 toolchain，并且这个 toolchain 里要有 `rustfmt` 组件。

检查当前工具链和 `rustfmt` 是否可用：

```bash
rustc --version
cargo --version
rustfmt --version
cargo fmt --version
```

如果使用 `rustup`，还可以看当前目录会选择哪个 toolchain：

```bash
rustup show
rustup component list --installed | grep rustfmt
```

如果缺少 `rustfmt`，常见安装方式是：

```bash
rustup component add rustfmt
```

如果项目使用指定 toolchain，例如 nightly，则安装到对应 toolchain：

```bash
rustup component add rustfmt --toolchain nightly
```

如果仓库使用 `rust-toolchain.toml` 固定了更具体的 nightly 版本，则应按 `rustup show` 中显示的 active toolchain 安装对应组件。

内部检查链路：

```text
工作区 Rust 源码
  → cargo 根据当前目录选择 Rust toolchain
  → cargo fmt 调用该 toolchain 的 rustfmt
  → rustfmt 解析 crate/module 中的 Rust 语法树
  → rustfmt 在内存中生成规范格式结果
  → --check 将规范格式结果与磁盘文件比较
  → 不一致则输出 diff 并返回失败
```

它能发现：

- Rust 文件缩进、换行、import 分组等格式问题。
- 手工编辑后忘记运行 formatter。
- 缺少 `rustfmt` 组件或当前 toolchain 不完整。
- 某些语法未闭合导致 rustfmt 无法解析。

它不能证明：

- 代码能通过 `cargo check` 或 `cargo build`。
- 类型、生命周期、trait bound、feature/cfg 组合一定正确。
- 代码语义正确或测试覆盖充分。
- QEMU、ktest 或系统测试能启动。

失败时处理：

```bash
cargo fmt --all
cargo fmt --all --check
```

### 1.2 Git diff 空白检查

```bash
git diff --check
```

目的：检查当前未提交 diff 里是否有不适合进入提交的空白问题，例如行尾空格、文件末尾空白错误，或残留 merge conflict marker。

`git diff` 是 Git 用来查看“当前内容相对另一个版本改了什么”的命令。日常验证里不需要理解 Git 的底层存储，只要熟悉几个常用看法：

```bash
git diff
```

查看 working tree 中还没 staged 的改动。

```bash
git diff --staged
```

查看已经 staged、准备进入下一次 commit 的改动。

```bash
git diff -- <path>
```

只看某个文件或目录的改动，例如：

```bash
git diff -- "kernel/core/comps/device-mapper/src"
```

`--check` 是 `git diff` 的一个验证模式。它不关心业务逻辑，只扫描 diff 中 Git 能识别的空白错误：

```bash
git diff --check
```

也可以限制范围：

```bash
git diff --check -- "kernel/core/comps/device-mapper/src" "kernel/core/src/device/misc/device_mapper.rs"
```

使用时重点看两点：

1. 命令没有输出且 exit code 为 0：说明当前检查范围没有 Git 空白错误。
2. 命令输出 `path:line: message`：说明对应文件对应行附近有问题，需要打开文件修掉。

常见输出含义：

```text
path/to/file.rs:42: trailing whitespace.
```

表示第 42 行行尾有多余空格。

```text
path/to/file.md:10: leftover conflict marker
```

表示文件里可能还残留 `<<<<<<<`、`=======`、`>>>>>>>` 这类 merge conflict marker。

它和 `cargo fmt` 的关系：

- `cargo fmt` 只处理 Rust 格式，并且会按 rustfmt 规则重排 Rust 代码。
- `git diff --check` 不重排代码，只检查当前 diff 的空白错误。
- `git diff --check` 能覆盖 Markdown、Shell、TOML、patch 等非 Rust 文件。
- 所以两者都要跑：`cargo fmt` 解决 Rust 格式，`git diff --check` 兜住跨文件类型的 diff 空白问题。

### 1.3 工作区状态检查

```bash
git status --short
```

目的：确认验证后工作区状态与本轮预期一致，特别是区分代码/文档改动、测试产物日志和新脚本文件。

定向 ktest 在目标 crate 目录执行，避免为了筛选 crate 临时修改根 [Cargo.toml](../Cargo.toml) 的 `default-members`。例如：

```bash
cd kernel/core/comps/device-mapper
cargo osdk test aster_device_mapper::table::tests
```

或：

```bash
cd kernel/core
cargo osdk test --kcmd-args=earlycon \
  aster_core::device::misc::device_mapper::tests
```

`git status --short` 的作用是确认本轮留下的文件是否都在预期范围内。手动 ktest 虽从目标 crate 目录启动，但当前使用仓库根 OSDK manifest，因此日志应在仓库根目录查看：默认 `hvc0` 的 ktest UART 输出在 `qemu-serial.log`。仅在交互式排障时显式设置 `CONSOLE=ttyS0`，并查看 `qemu.log`；不要提交运行日志。

## 2. `make ktest` 背后到底在做什么

通用入口：

```bash
make ktest
```

定向入口在目标 crate 目录手动执行：

```bash
cd kernel/core/comps/device-mapper
cargo osdk test aster_device_mapper::table::tests
```

ioctl 层测试：

```bash
cd kernel/core
cargo osdk test --kcmd-args=earlycon \
  aster_core::device::misc::device_mapper::tests
```

先看总链路：

| 阶段 | 谁在做 | 主要动作 | 关键产物/日志 | 慢或失败时优先看 |
|---|---|---|---|---|
| 1. Makefile 入口 | `make ktest` | 准备前置依赖，然后调用 `cargo osdk test` | 默认日志与 shell 退出码 | 当前目录或命令行参数是否改变测试范围 |
| 2. initramfs 前置依赖 | `make initramfs` | 构建测试用 initramfs | `test/initramfs/build/initramfs.cpio.gz` | initramfs 是否在重建、VDSO 环境是否缺失 |
| 3. cargo-osdk 前置依赖 | `$(CARGO_OSDK)` | 确认 `~/.cargo/bin/cargo-osdk` 可用；必要时重新安装 OSDK | `~/.cargo/bin/cargo-osdk` | 是否触发 `cargo install cargo-osdk --path osdk` |
| 4. 选择测试 crate | `cargo osdk test` | 根据当前目录识别 current crate；在根目录时才受 workspace/default-members 影响 | 当前 crate 或 root `Cargo.toml` | 是否站在正确 crate 目录、是否误从根目录扩大测试范围 |
| 5. 生成 test base crate | OSDK test command | 为每个被测 crate 生成临时 test base crate，并写入测试白名单 | `target/osdk/<crate>/src/main.rs` | TESTNAME 是否正确进入 whitelist |
| 6. 编译测试内核 | OSDK build | 用 `--cfg ktest` 编译 kernel ELF，让 `#[cfg(ktest)]` 测试代码进入内核 | `target/<arch>/<profile>/<crate>` | Rust 编译错误、profile 改变、增量缓存失效 |
| 7. 生成启动 bundle | OSDK bundle | 复制 kernel/initramfs，按 boot method 生成 GRUB ISO 等启动产物 | `target/osdk/<crate>/bundle.toml`、ISO | bundle/cache 是否重建、GRUB ISO 是否耗时 |
| 8. 启动 QEMU | OSDK bundle runner | 拼 QEMU 参数并启动 guest | 终端 stdout、`qemu.log`、`qemu-serial.log` | 是否带 `-accel kvm`，是否退回 TCG，是否进入 ktest runner |
| 9. guest 内运行 ktest runner | `osdk-test-kernel` | 枚举所有 `#[ktest]`，按 crate/test whitelist 过滤并执行 | `[ktest runner]`、`test result` | 测试是否卡住、panic、过滤条件是否过宽 |
| 10. 返回结果 | OSDK/QEMU | guest 通过 `isa-debug-exit` 退出，OSDK 解码成功/失败 | shell exit code、`test result` | QEMU 自身失败、kernel panic、triple fault |

几个最容易误解的点：

| 现象 | 实际含义 |
|---|---|
| `make ktest` 不是普通 `cargo test` | 它会构建一个可启动测试内核，并在 QEMU guest 里跑 kernel-mode tests。 |
| 只跑一个测试仍显示很多 tests/crates | guest 内 runner 会先枚举完整 ktest tree，再用 whitelist 过滤。 |
| `... filtered out` | 不是失败，只是当前 crate 中未匹配测试被跳过；如果目标是覆盖某个路径而结果为 `0 passed`，应改跑正确 crate、修正过滤路径或扩大到 crate 全量 ktest。 |
| `cargo osdk test` | 在目标 crate 目录运行定向测试；使用默认 console，并从对应 QEMU 日志读取原始输出。 |
| ktest 很慢但 QEMU 已出现 | 多半要看 guest boot 到 `[ktest runner]` 之间，例如 KVM/TCG、boot protocol、kernel init。 |

定向 ktest 的过滤链路可以记成一行：

```text
命令行 TESTNAME → OSDK 写入 KTEST_TEST_WHITELIST → guest 内 ktest runner 按后缀匹配测试路径 → 不匹配的计入 filtered out
```

`#[cfg(ktest)]` 的编译链路可以记成一行：

```text
cargo osdk test → RUSTFLAGS 加 `--cfg ktest` → `#[cfg(ktest)]` 测试模块/helper 编进 kernel ELF → 正常生产 build 不包含这些测试代码
```

慢启动定位也先看一张表：

| 观察阶段 | 如果这里慢，优先怀疑 |
|---|---|
| 命令开始 → cargo build 完成 | 编译增量失效、profile 改变、当前目录选错导致测试 crate 过大、依赖更新 |
| cargo build 完成 → QEMU 进程出现 | OSDK bundle、GRUB ISO、initramfs 打包 |
| QEMU 进程出现 → 终端出现 boot / runner 输出 | QEMU 参数、固件启动、日志重定向 |
| boot 输出 → `[ktest runner]` | KVM/TCG、boot protocol、kernel 初始化、panic |
| `[ktest runner]` → `test result` | 测试本身慢、死锁、过滤条件过宽 |

常用定位命令：

```bash
pgrep -af '[q]emu-system' || true
tail -n 100 qemu.log
tail -n 100 qemu-serial.log
```

手动定向测试时，在目标 crate 目录运行 `cargo osdk test <crate>::<module>::tests`；core 测试还要传 `--kcmd-args=earlycon`。默认 `hvc0` 的 UART 输出查看根目录 `qemu-serial.log`；仅在排障时设置 `CONSOLE=ttyS0` 并查看 `qemu.log`，已有日志可能是旧文件。

## 3. DM system test 背后在做什么

### 3.1 表层命令

```bash
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 \
  myshell/run_dm_system_tests.sh <suite>
```

可用 suite 是：

```text
--control-plane
--dataplane
--lvm2-topology
--linear-integration
--striped-integration
--mixed-integration
```

入口要求显式选择一个 suite；不提供默认聚合或历史别名。

### 3.2 system test 的执行链路

系统测试不是 ktest。它启动 NixOS guest，在 guest 内运行真实 `dmsetup`、LVM2、ext2 和 block I/O 命令。

```text
myshell/run_dm_system_tests.sh <suite>
  → 选择对应子脚本
  → 子脚本 source myshell/lib/dm_nixos_test.sh
  → dm_prepare_nixos_test
      → 检查无残留 QEMU
      → 检查 target/nixos/asterinas.img
      → 准备/重置 DM_TEST_IMAGES
  → dm_run_single_guest_test / dm_run_two_guest_test / dm_run_three_guest_test
      → dm_run_guest_script：setsid make run_nixos
      → tools/nixos/run.sh 拼 QEMU 命令并挂入测试盘
      → 在 40 秒内等待 `root@asterinas` shell-ready
      → 默认以 10ms 逐行节流注入 guest script
      → 在 180 秒 guest 生命周期内等待退出
  → guest 输出 CHECK_PASS_*、TEST_PASS_*
  → host 扫描日志，输出 HOST_PASS_* 或 HOST_FAIL_*
```

`dm_run_three_guest_test` 用于 linear、striped integration：第一轮建卷和 grow，第二轮恢复与 shrink，第三轮再次恢复并只读校验。mixed integration 使用两轮 guest。

### 3.3 system test 验证层次

| suite | 验证层次 | 实际证明什么 |
|---|---|---|
| `--control-plane` | 真实 libdevmapper 控制面 | `/dev/mapper/control`、table/status/deps/info、active/inactive 生命周期、events、rename/UUID、readonly、remove；并验证 error/zero I/O 语义。 |
| `--dataplane` | raw DM 数据面 | linear、striped、mixed、error、zero 的 mapper I/O、跨 target/chunk split、direct completion 与 backing 布局。 |
| `--lvm2-topology` | 真实 LVM2 同 boot 生命周期 | PV/VG/LV 查询与 create/grow/shrink、linear/striped/mixed segment、scan/activation/remove。 |
| `--linear-integration` | LVM2 linear + ext2 + reboot | 同 PV 和跨 PV second segment、grow/shrink、三次启动恢复。 |
| `--striped-integration` | LVM2 striped + ext2 + reboot | same-set 和 cross-set striped segment、grow/shrink、三次启动恢复。 |
| `--mixed-integration` | LVM2 mixed + ext2 + reboot | linear + striped mixed table、跨段文件 I/O、两次启动恢复。 |

### 3.4 时间 marker 与超时含义

每个 guest 输出：

```text
HOST_INFO_<TEST_ID> <label>_guest_started_at=<ISO8601>
HOST_INFO_<TEST_ID> <label>_guest_ready_after=<seconds>s ready_timeout=40s lifecycle_timeout=180s
HOST_INFO_<TEST_ID> <label>_guest_completed_at=<ISO8601> lifecycle_after=<seconds>s status=<status>
```

shell-ready 时间只证明 guest shell 已可接收命令；180 秒 lifecycle 上限覆盖 guest 内测试、同步和关机。ready 超过 40 秒即失败，不能用更长 lifecycle timeout 掩盖启动问题。

### 3.5 raw 数据面为何能证明映射正确

`--dataplane` 的 linear 三段和非对齐 striped 场景均从 mapper 写入可区分 payload，再分别从 mapper 与 backing disk 指定 sector 读回并对比。striped 场景使用 `striped 2 4`，从 mapper sector 2 写入 12 sectors，覆盖 partial chunk、完整 stripe row 和最终 partial chunk；它同时验证 table-level/target-level split、remap、completion 与真实 backing 落点，而不仅是 mapper 读回。

## 4. 推荐验证组合

### 4.1 任何生产 Rust 代码修改后

```bash
cargo fmt --all --check
git diff --check
git status --short
```

测试表现不像预期时，优先确认 `git status --short` 输出是否只包含本轮预期文件；如果出现意外的 workspace 配置改动，应按普通工作区异常处理，而不是把某个单文件 diff 作为固定验证步骤。

### 4.2 改 DM core target/table 数据面

```bash
cargo fmt --all --check
git diff --check -- "kernel/core/comps/device-mapper/src"
```

定向 ktest：

```bash
cd kernel/core/comps/device-mapper
cargo osdk test aster_device_mapper::table::tests
```

如果改动影响真实数据面边界：

```bash
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 \
  myshell/run_dm_system_tests.sh --dataplane
```

### 4.3 改 DM ioctl/control-plane

```bash
cargo fmt --all --check
git diff --check -- "kernel/core/comps/device-mapper/src" "kernel/core/src/device/misc/device_mapper.rs"
```

定向 ioctl ktest：

```bash
cd kernel/core
cargo osdk test --kcmd-args=earlycon \
  aster_core::device::misc::device_mapper::tests
```

如果改变用户可见 dmsetup 语义：

```bash
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 \
  myshell/run_dm_system_tests.sh --control-plane
```

如果改变 LVM2 可见行为或 segment 布局：

```bash
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 \
  myshell/run_dm_system_tests.sh --lvm2-topology
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 \
  myshell/run_dm_system_tests.sh --linear-integration
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 \
  myshell/run_dm_system_tests.sh --striped-integration
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 \
  myshell/run_dm_system_tests.sh --mixed-integration
```

### 4.4 测试前后检查 QEMU 残留

```bash
pgrep -af '[q]emu-system' || true
```

目的：避免多个 QEMU 同时争用同一批测试 image、host port 或日志文件。

如果发现残留，不要直接杀未知 QEMU；先确认它是否是当前 run 启动的进程。只停止当前测试自己启动的进程。

## 5. 通过标准

一次生产代码修改通常至少需要满足：

- `cargo fmt --all --check` 通过。
- `git diff --check` 通过。
- 在目标 crate 目录执行的相关 `cargo osdk test` 或必要的 `make ktest` 通过。
- 如果影响真实 `dmsetup`/LVM2/数据面语义，相关 system suite 输出 `HOST_PASS_*`。
- 测试后无 QEMU 残留。
- 定向 ktest 使用模块 selector `crate::...::tests`；core 测试附加 `--kcmd-args=earlycon`，默认从仓库根目录的 `qemu-serial.log` 查看 UART 输出。`CONSOLE=ttyS0` 仅用于交互式排障。

判断 system test 完整通过时，要看最终 marker：

```text
TEST_PASS_...
HOST_PASS_...
```

只有中间的 `CHECK_PASS_*` 不代表完整 suite 通过。

## 6. 核心方法论

1. 先知道命令会进入哪条链路，再解释结果。
2. 先看阶段 marker，再判断是构建慢、QEMU 慢、boot 慢还是测试慢。
3. 定向 ktest 的过滤发生在 guest 内 ktest runner，不是宿主 shell 层过滤。
4. 当前工作目录决定 OSDK 为哪个 crate 生成测试内核；模块 selector 只决定 runner 最后运行哪些测试。
5. QEMU 参数是性能和启动路径判断的关键证据，尤其要确认是否带 `-accel kvm`。
6. ktest 证明 kernel 内部逻辑；system test 证明真实用户态工具和真实 block device 链路。
