# Device Mapper 测试执行结构与命令说明

本文整理 `dm` 分支中 Device Mapper 相关生产代码完成后应如何验证：先做静态检查，再用 ktest 验证内核内部语义，最后按改动范围选择 NixOS/LVM2 系统验收 suite。`make kernel`、`make nixos`、`dmsetup`、LVM2、文件系统和数据校验命令放在后半部分，作为脚本背后的命令说明。

## 1. 基本执行环境

| 项目 | 路径 / 约定 |
|---|---|
| 宿主仓库路径 | `/root/atom/asterinas` |
| 容器内仓库路径 | `/root/asterinas` |
| 默认执行位置 | 容器 `myAsterinas` 内的 `/root/asterinas` |
| 命令书写约定 | 除非明确标注为宿主命令，本文命令都默认已在容器内执行 |
| 并发约束 | QEMU、ktest、NixOS 系统测试串行运行，避免共享镜像、测试盘或 `test/initramfs/build/ext2.img` 锁冲突 |

## 2. 推荐验证顺序

| 顺序 | 验证层级 | 默认入口 | 主要目的 | 何时必须跑 |
|---|---|---|---|---|
| 1 | 静态检查 | `cargo fmt --check`、`git diff --check`、`git status --short` | 先排除格式、空白、冲突标记和工作区状态问题 | 每次生产代码改动后 |
| 2 | ktest | `myshell/ktest_crate.sh <crate-dir> <test-path>` | 在测试内核里验证 DM core、target、table、ioctl 等内核内部语义 | 改到 Rust 生产代码时优先跑 |
| 3 | 系统验收 suite | `GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh <suite>` | 在 NixOS guest 里用真实 `dmsetup` / LVM2 / 文件系统 / block device 验证用户可见语义 | 改到用户态链路、ioctl ABI、设备节点、LVM2 交互、脚本，或阶段验收时 |
| 4 | 底层命令说明 | `make kernel`、`make nixos`、`dmsetup`、PV/VG/LV、`dd`、`md5sum` 等 | 帮助理解脚本背后做了什么，便于定位失败 | 不作为日常逐条手敲流程 |

## 3. 静态检查

常用命令：

```bash
cargo fmt --check
git diff --check
git status --short
```

| 命令 | 用途 | 通过标准 | 不能证明什么 |
|---|---|---|---|
| `cargo fmt --check` | 调用当前 Rust toolchain 的 `rustfmt` 检查 Rust 文件格式 | 无格式 diff 输出，退出码为 0 | 不证明代码能编译，也不证明语义正确 |
| `cargo fmt --all --check` | 覆盖 workspace 的 Rust 格式检查 | 整个 workspace 无格式 diff 输出 | 不替代 clippy、ktest 或系统测试 |
| `git diff --check` | 检查当前 diff 中 trailing whitespace、空白错误、冲突标记等提交前问题 | 无 warning/error 输出 | 不检查 Rust 语义，不判断测试是否通过 |
| `git status --short` | 查看当前修改、未跟踪文件和暂存状态 | 输出与本轮预期修改一致 | 不说明 diff 内容是否正确 |

如果只改了 Markdown、日志或测试说明，静态检查通常只需要 `git diff --check` 和 `git status --short`；如果改了 Rust 代码，再加 `cargo fmt --check` 或 `cargo fmt --all --check`。

## 4. ktest：内核内部语义验证

### 4.1 统一入口

当前定向 ktest 默认从仓库根目录调用 [ktest_crate.sh](../myshell/ktest_crate.sh)：

```bash
myshell/ktest_crate.sh <crate-dir> [cargo-osdk-test-filter-or-args...]
```

参数含义：

| 参数 | 含义 | 示例 |
|---|---|---|
| `<crate-dir>` | 相对仓库根目录的目标 crate 路径，脚本会进入该目录运行 `cargo osdk test` | `kernel/core/comps/device-mapper`、`kernel/core` |
| `[cargo-osdk-test-filter-or-args...]` | 原样透传给 `cargo osdk test`；最常用的是测试过滤路径 | `aster_device_mapper::table::tests::<test_name>` |

脚本封装的默认环境变量：

| 变量 | 默认值 | 作用 |
|---|---|---|
| `KTEST_LOGLEVEL` | `error` | 设置内核日志级别，减少无关输出 |
| `KTEST_TIMEOUT` | `180s` | 单次 ktest 完整生命周期超时 |
| `KTEST_TIMEOUT_KILL` | `10s` | `timeout` 到期后的强制终止等待时间 |
| `RELEASE` | `1` | 默认用 release profile 编译测试内核 |
| `BOOT_METHOD` | `grub-rescue-iso` | OSDK 启动方式 |
| `BOOT_PROTOCOL` | `multiboot2` | GRUB boot protocol |
| `ENABLE_KVM` | `1` | 默认给 QEMU 加 `-accel kvm`，避免退化到 TCG 慢启动 |
| `KTEST_CONSOLE` | `hvc0` | ktest serial 输出使用的 console |
| `INITRAMFS` | `/root/asterinas/test/initramfs/build/initramfs.cpio.gz` | 测试内核使用的 initramfs |

脚本执行链路：

| 阶段 | `ktest_crate.sh` 做什么 | 为什么需要 |
|---|---|---|
| 1 | 校验 `<crate-dir>/Cargo.toml` 存在 | 防止在错误目录启动 OSDK 测试 |
| 2 | 组装 release、boot、console、KVM、initramfs、`timeout --foreground` 参数 | 避免每次手写公共参数，也避免漏掉 KVM/initramfs；`--foreground` 避免交互终端里 QEMU 被 job-control stop |
| 3 | 运行期间保留终端输出，并用 OSDK 原始日志作为提取来源 | 终端仍能看到 QEMU/ktest 进度，最终结果日志不混入启动噪声 |
| 4 | `cd` 到目标 crate 目录 | 让 `cargo osdk test` 选择正确 crate |
| 5 | 执行 `cargo osdk test ... <filter>` | 编译测试内核、启动 QEMU、在 guest 内运行 ktest runner |
| 6 | 从原始输出提取当前 crate 的 ktest 结果到 `<crate-dir>/ktest.log` | 日志只保留每个 ktest 的结果，不保留 QEMU 启动和 kernel 杂项日志 |
| 7 | 删除 QEMU 原始日志 | 最终只留下一个结果日志 |

### 4.2 按改动范围选择 ktest

| 改动范围 | 推荐 ktest crate | 推荐过滤路径 | 主要验证内容 |
|---|---|---|---|
| DM target 参数解析、target metadata、target status、target map 逻辑 | `kernel/core/comps/device-mapper` | `aster_device_mapper::target::<target>::tests::<test_name>` | concrete target 的 parse/status/map 行为 |
| DM table 连续性、跨 target BIO split、mapped BIO 聚合、flush fan-out、deps 去重 | `kernel/core/comps/device-mapper` | `aster_device_mapper::table::tests::<test_name>` | DM core 数据面和 table 语义 |
| `DmDevice`、BIO enqueue、child completion、target action 组合 | `kernel/core/comps/device-mapper` | `aster_device_mapper::<module>::tests::<test_name>` | mapper 内部 I/O 路径 |
| `/dev/mapper/control` ioctl、table load、active/inactive table、status/deps、suspend/resume | `kernel/core` | `aster_core::device::misc::device_mapper::tests::<test_name>` | DM ioctl 控制面和 core 的连接 |
| block device 注册、devtmpfs 节点、misc device、用户可见设备路径 | `kernel/core`，必要时再跑系统 suite | `aster_core::<related_module>::tests::<test_name>` | 内核框架与 DM 控制面的集成 |
| 只改文档、日志或注释，未改变 Rust 语义 | 通常不需要 ktest | 无 | 用静态检查和文档 diff 检查即可 |

常用示例：

```bash
myshell/ktest_crate.sh kernel/core/comps/device-mapper aster_device_mapper::table::tests::<test_name>
```

```bash
myshell/ktest_crate.sh kernel/core aster_core::device::misc::device_mapper::tests::<test_name>
```

不传测试过滤路径时，脚本会跑目标 crate 的全部 ktest；日常定位优先传完整测试路径，阶段验收或大范围重构后再考虑扩大范围。

### 4.3 ktest 日志和通过标记

[ktest_crate.sh](../myshell/ktest_crate.sh) 启动前会打印结果日志路径：

```text
ktest_crate: result log: /root/asterinas/<crate-dir>/ktest.log
```

最终只保留一个日志文件：目标 crate 目录下的 `ktest.log`。它从当前 crate 的 `running ... tests in crate "..."` 开始，只记录 ktest runner 的测试结果、summary 和必要 failure 信息；QEMU 启动日志、OVMF/GRUB 输出、kernel 普通日志不会保留在该结果日志中。

常用查看命令：

```bash
cat <crate-dir>/ktest.log
```

通过时应能看到类似：

```text
running ... tests in crate "..."
test <module_path>::<test_name> ... ok
test result: ok. ... passed; 0 failed; ... filtered out.
All crates tested.
```

负测中被测代码可能会在终端打印预期内的 `ERROR:` 日志；`ktest.log` 只保留最终测试结果行，所以不会把错误路径日志混进 `test ... ok` 同一行。

### 4.4 ktest 失败排查

| 现象 | 优先怀疑 | 排查方向 |
|---|---|---|
| 超过约 3 分钟没有关键进展 | 过滤路径不正确、选错 crate、QEMU 卡住、镜像锁冲突、资源压力 | 看终端输出、`<crate-dir>/ktest.log`、`pgrep -af '[q]emu-system'`、`free -h`、`uptime` |
| QEMU 启动很慢 | 没有 KVM、退化到 TCG、宿主资源压力 | 确认脚本默认 `ENABLE_KVM=1`，看终端输出里的 QEMU 命令和资源状态 |
| 命令退出 0 但 `ktest.log` 没有结果 | serial/mux 原始输出没有出现 ktest marker，或 console 参数不匹配 | 确认 `KTEST_CONSOLE=hvc0`，并检查终端输出是否到达 ktest runner |
| 只跑了 0 个目标测试 | 测试过滤路径写错或 crate 不匹配 | 使用完整模块路径，确认 crate 前缀是 `aster_device_mapper` 或 `aster_core` |
| `ktest.log` 中有 `FAILED` | Rust 生产代码语义错误或测试断言失败 | 查看 `ktest.log` 中对应 test name、failure 段和终端上下文 |

必要时只停止自己本轮启动的异常 QEMU/ktest 进程；不要用重启容器、清系统 cache 或修改启动协议来掩盖问题。

## 5. 系统验收 suite：用户可见语义验证

统一入口：

```bash
GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh <suite>
```

当前入口要求显式传入 suite；不保留无参数默认运行、`--full` 或旧兼容别名。

### 5.1 suite 与功能对应关系

| suite | 子脚本 | 主要验证功能 |
|---|---|---|
| `--quick` | [run_control_abi_test.sh](../myshell/dm_linear/run_control_abi_test.sh)、[run_cross_target_bio_regression.sh](../myshell/dm_linear/run_cross_target_bio_regression.sh)、[run_raw_striped_bio_test.sh](../myshell/dm_striped/run_raw_striped_bio_test.sh) | control ABI smoke、raw linear cross-target BIO、raw striped BIO split/remap 与 backing 分布 |
| `--dmsetup-cli` | [run_dmsetup_cli_semantics_test.sh](../myshell/run_dmsetup_cli_semantics_test.sh) | 对照标准 Linux/OpenEuler 的 dmsetup CLI 控制面语义，覆盖 tableless create、linear/striped/error/zero table、table/status/deps/info、rename、suspend/resume、wait、remove_all |
| `--lvm2-cli` | [run_lvm2_cli_semantics_test.sh](../myshell/run_lvm2_cli_semantics_test.sh) | 对照标准 Linux/OpenEuler 的 LVM2 CLI 控制面语义，覆盖 PV/VG/LV 查询、linear/striped/mixed LV、扩缩容、scan/activation 和 remove 闭环 |
| `--dataplane-edge` | [run_dm_dataplane_edge_test.sh](../myshell/run_dm_dataplane_edge_test.sh) | raw DM 数据面边界审计，覆盖三段 linear 非零 backing start、striped 非 chunk 起点写入、跨 stripe 边界分布、zero target direct-complete 路径 |
| `--linear-data` | [run_cross_target_bio_regression.sh](../myshell/dm_linear/run_cross_target_bio_regression.sh) | raw linear cross-target BIO split/remap |
| `--striped-data` | [run_raw_striped_bio_test.sh](../myshell/dm_striped/run_raw_striped_bio_test.sh) | raw striped BIO split/remap 和 backing 分布 |
| `--linear-lvm2` | [run_lvm2_linear_reboot_test.sh](../myshell/dm_linear/run_lvm2_linear_reboot_test.sh) | 单 PV、单 linear segment、同盘 grow/shrink、ext2 I/O、reboot recovery |
| `--striped-lvm2` | [run_lvm2_striped_reboot_test.sh](../myshell/dm_striped/run_lvm2_striped_reboot_test.sh) | N PV / N-way 单 striped segment、同组盘 grow/shrink、ext2 I/O、reboot recovery |
| `--linear-lvm2-cross-segment` | [run_lvm2_linear_cross_segment_test.sh](../myshell/dm_linear/run_lvm2_linear_cross_segment_test.sh) | 独立 linear cross-segment table、reboot recovery、shrink 回单段 |
| `--striped-lvm2-cross-segment` | [run_lvm2_striped_cross_segment_test.sh](../myshell/dm_striped/run_lvm2_striped_cross_segment_test.sh) | 独立 striped N-to-2N cross-segment table、reboot recovery、shrink 回单段 |
| `--mixed-lvm2` | [run_lvm2_linear_striped_mixed_reboot_test.sh](../myshell/dm_mixed/run_lvm2_linear_striped_mixed_reboot_test.sh) | 同一 LV 内 linear + striped mixed table、ext2 I/O、reboot recovery |

### 5.2 按改动范围选择系统验收

| 改动范围 | 推荐 suite | 说明 |
|---|---|---|
| 只改 DM core 内部 table/target/BIO 逻辑 | 先 ktest；必要时 `--dataplane-edge` 或相关 raw data suite | ktest 更快，系统 suite 用来确认真实 block device 行为 |
| 改 `dmsetup` 可见 ioctl 语义、status、deps、info、rename、suspend/resume | `--dmsetup-cli`，必要时加 `--quick` | 验证 libdevmapper 与 `/dev/mapper/control` 交互 |
| 改 linear 数据面或跨 target split | `--linear-data`、`--linear-lvm2-cross-segment` | raw BIO 覆盖边界，LVM2 覆盖用户态生成 table |
| 改 striped map/chunk/stripe/deps | `--striped-data`、`--striped-lvm2`、`--striped-lvm2-cross-segment` | 覆盖 raw striped 和真实 LVM2 striped LV |
| 改 mixed linear + striped table 或跨 segment I/O | `--mixed-lvm2` | 验证同一 LV 内 mixed table 与 reboot recovery |
| 改 LVM2 交互、scan/activation、PV/VG/LV layout | `--lvm2-cli` 加对应 LVM2 suite | 需要真实 LVM2 CLI 和 NixOS guest |
| 改测试脚本或 guest harness | 直接跑被改脚本对应 suite | 验证脚本自己的日志、marker、cleanup 和超时逻辑 |

### 5.3 系统测试日志和通过标记

系统验收脚本通过公共 harness [dm_nixos_test.sh](../myshell/lib/dm_nixos_test.sh) 运行。每个脚本启动时都会打印 host 侧日志路径：

```text
HOST_INFO_<TEST_ID> log=/tmp/<test-log>.log
```

常见默认日志：

| suite | 默认日志 |
|---|---|
| `--quick` 中 control ABI | `/tmp/dm-control-abi-test.log` |
| `--dmsetup-cli` | `/tmp/dmsetup-cli-semantics-test.log` |
| `--linear-data` | `/tmp/cross-target-bio-regression.log` |
| `--striped-data` | `/tmp/dm-striped-raw-bio-test.log` |
| `--linear-lvm2` | `/tmp/dm-linear-lvm2-reboot-test.log` |
| `--striped-lvm2` | `/tmp/dm-striped-lvm2-reboot-test.log` |
| `--linear-lvm2-cross-segment` | `/tmp/dm-linear-lvm2-cross-segment-test.log` |
| `--striped-lvm2-cross-segment` | `/tmp/dm-striped-lvm2-cross-segment-test.log` |
| `--mixed-lvm2` | `/tmp/dm-mixed-lvm2-reboot-test.log` |

常见通过标记：

| suite / script | 关键 pass marker |
|---|---|
| `run_dm_system_tests.sh <suite>` | `HOST_PASS_DM_SYSTEM_TESTS <suite>` |
| `--quick` | `HOST_PASS_DM_CONTROL_ABI`、`HOST_PASS_CROSS_TARGET_BIO`、`HOST_PASS_DM_STRIPED_RAW_BIO`、`HOST_PASS_DM_SYSTEM_TESTS --quick` |
| `--dmsetup-cli` | `SUMMARY_GAP_DMSETUP_CLI_SEMANTICS: 0`、`TEST_PASS_DMSETUP_CLI_SEMANTICS`、`HOST_PASS_DMSETUP_CLI_SEMANTICS` |
| `--lvm2-cli` | `SUMMARY_GAP_LVM2_CLI_SEMANTICS: 0`、`TEST_PASS_LVM2_CLI_SEMANTICS`、`HOST_PASS_LVM2_CLI_SEMANTICS` |
| `--dataplane-edge` | `TEST_PASS_DM_DATAPLANE_EDGE`、`HOST_PASS_DM_DATAPLANE_EDGE` |
| `--linear-data` | `TEST_PASS_CROSS_TARGET_BIO`、`HOST_PASS_CROSS_TARGET_BIO` |
| `--striped-data` | `TEST_PASS_DM_STRIPED_RAW_BIO`、`HOST_PASS_DM_STRIPED_RAW_BIO` |
| `--linear-lvm2` | `TEST_PASS_DM_LINEAR_LVM2_REBOOT_FIRST`、`TEST_PASS_DM_LINEAR_LVM2_REBOOT_SECOND`、`HOST_PASS_DM_LINEAR_LVM2_REBOOT` |
| `--striped-lvm2` | `TEST_PASS_DM_STRIPED_LVM2_REBOOT_FIRST`、`TEST_PASS_DM_STRIPED_LVM2_REBOOT_SECOND`、`HOST_PASS_DM_STRIPED_LVM2_REBOOT` |
| `--linear-lvm2-cross-segment` | `TEST_PASS_DM_LINEAR_LVM2_CROSS_SEGMENT_FIRST`、`TEST_PASS_DM_LINEAR_LVM2_CROSS_SEGMENT_SECOND`、`HOST_PASS_DM_LINEAR_LVM2_CROSS_SEGMENT` |
| `--striped-lvm2-cross-segment` | `TEST_PASS_DM_STRIPED_LVM2_CROSS_SEGMENT_FIRST`、`TEST_PASS_DM_STRIPED_LVM2_CROSS_SEGMENT_SECOND`、`HOST_PASS_DM_STRIPED_LVM2_CROSS_SEGMENT` |
| `--mixed-lvm2` | `TEST_PASS_DM_MIXED_LVM2_LINEAR_STRIPED_REBOOT_FIRST`、`TEST_PASS_DM_MIXED_LVM2_LINEAR_STRIPED_REBOOT_SECOND`、`HOST_PASS_DM_MIXED_LVM2_LINEAR_STRIPED_REBOOT` |

失败时优先看终端里的 `HOST_FAIL_...` 或 `TEST_FAIL_...`，再打开 `HOST_INFO_<TEST_ID> log=...` 指向的完整日志，搜索最后一个 `=== STEP` 或 `=== CHECK`。

## 6. 系统测试背后的 guest、测试盘和超时

| 项目 | 说明 |
|---|---|
| guest | 系统 suite 在 NixOS guest 中运行真实用户态工具，不是只跑内核单测 |
| 完整生命周期超时 | `GUEST_QEMU_TIMEOUT=180` 控制单个 QEMU guest 从启动到退出的完整生命周期 |
| 旧兼容变量 | `GUEST_READY_TIMEOUT` 只作为旧脚本兼容 alias |
| 测试盘 | 由 [tools/nixos/run.sh](../tools/nixos/run.sh) 挂入 QEMU |
| 默认测试盘 | `target/nixos/test.img`、`target/nixos/test2.img`、`target/nixos/test3.img` 等 |
| 多盘变量 | `DM_TEST_IMAGES="target/nixos/test.img target/nixos/test2.img target/nixos/test3.img"` |
| guest 内定位 | `aster-dm-disk-locator`、`aster-dm-disk-locator vdmtest2`、`aster-dm-disk-locator vdmtest3` |

稳定 VirtIO serial：

```text
vdmtest
vdmtest2
vdmtest3
```

使用 locator 是为了避免硬编码 `/dev/vda`、`/dev/vdb`、`/dev/vdc`，防止多盘枚举顺序变化导致误测。

## 7. 支撑性构建和清理命令

这些命令通常由脚本或 Makefile 间接使用；日常执行测试时不需要逐条手敲，除非镜像缺失、构建产物过期或测试盘被旧状态污染。

| 命令 | 作用 | 常见使用时机 |
|---|---|---|
| `make kernel` | 构建 initramfs，并通过 `cargo osdk build` 构建内核 | 需要单独确认 kernel 可构建时 |
| `make ktest` | 构建 initramfs，并通过 `cargo osdk test` 跑默认 kernel-mode tests | 全量默认 ktest；定向 DM ktest 优先用 `myshell/ktest_crate.sh` |
| `make nixos` | 构建 NixOS guest image，产物通常是 `target/nixos/asterinas.img` | 系统 suite 提示 NixOS image 不存在，或 NixOS 配置变更后 |
| `make rm_dm` | 清理 DM/LVM2 测试盘 image | 重复系统验收前需要回到干净测试盘状态时 |

显式清理多块测试盘：

```bash
DM_TEST_IMAGES="target/nixos/test.img target/nixos/test2.img target/nixos/test3.img" make rm_dm
```

## 8. `dmsetup` 命令说明

`dmsetup` 命令主要由系统 suite 在 guest 内执行，用来验证 libdevmapper 与 Asterinas DM 控制面的用户可见语义。

| 命令 | 主要验证点 |
|---|---|
| `dmsetup version` | `/dev/mapper/control` 可打开，version ioctl 可用，libdevmapper 能和内核 DM 控制面通信 |
| `dmsetup targets` | `linear`、`striped`、`error`、`zero` target metadata 暴露正确 |
| `dmsetup create` | 创建设备、加载 table、resume 激活；覆盖 linear/striped/error/zero table |
| `dmsetup table <mapper>` | table 输出格式、target type、logical start、length、backing major:minor、striped 参数 |
| `dmsetup status <mapper>` | mapper 状态和 target status 输出 |
| `dmsetup deps <mapper>` | backing dependency 数量和 major:minor 是否符合 table |
| `dmsetup info <mapper>` | mapper name、active/suspended 状态、open count、event 信息 |
| `dmsetup rename` | rename 后新名字可用、旧名字不可用，table/status 不被破坏 |
| `dmsetup suspend --noflush` / `dmsetup resume --noflush` | suspend/resume 状态切换，`--noflush` flag 兼容 |
| `dmsetup wait --noflush <mapper> 0` | `DM_DEV_WAIT` 最小语义和 event number 等待路径 |
| `dmsetup remove` / `dmsetup remove_all` | 普通 remove、busy remove 失败、remove_all 只删除 non-busy mapper |

典型 table 示例：

```text
0 524288 linear 253:64 2048
524288 524288 striped 2 8 253:80 2048 253:96 2048
```

含义：第一段是 linear segment，第二段是 2-way striped segment，chunk size 为 8 sectors。

## 9. LVM2 PV / VG / LV 命令说明

LVM2 suite 用真实 LVM2 命令生成 DM table，重点验证 Asterinas 能否承接 Linux 用户态工具生成的控制面和数据面行为。

所有 LVM2 系统测试都会尽量禁用 udev 自动联动，直接测试 libdevmapper 与 Asterinas DM core：

```bash
LVM_CONFIG='activation { udev_rules=0 }'
```

| 类别 | 典型命令 | 主要验证点 |
|---|---|---|
| PV | `pvcreate`、`pvscan`、`pvs` | 初始化测试盘、reboot 后重新发现 PV、确认 PV 所属 VG |
| VG | `vgcreate`、`vgextend`、`vgscan --mknodes`、`vgchange -ay/-an`、`vgs` | 创建/扩展 VG、reboot 后扫描并补 mapper 节点、激活/停用 VG |
| linear LV | `lvcreate --type linear`、`lvextend`、`lvreduce`、`lvs --segments` | 单 PV linear、追加 linear segment、shrink 回单段、查看 segment 布局 |
| striped LV | `lvcreate --type striped -i <count> -I <chunk>K`、`lvextend -i ... -I ...` | N-way striped、stripe count、chunk size、deps 与 PV 数一致 |
| mixed LV | 先 `lvcreate --type linear`，再 `lvextend --type striped` | 同一 LV 内生成 linear + striped mixed table |

shrink 顺序必须是先缩文件系统，再缩 LV：

```bash
e2fsck -f -y "$MAPPER_DEVICE"
resize2fs "$MAPPER_DEVICE" <smaller-size>
lvreduce --config "$LVM_CONFIG" -y -L <smaller-size> <vg>/<lv>
```

## 10. 文件系统和数据校验命令说明

| 命令 | 用途 |
|---|---|
| `mkfs.ext2 -F -b 4096 "$MAPPER_DEVICE"` | 在 mapper 上创建 ext2，强制格式化并使用 4 KiB block size |
| `blkid "$MAPPER_DEVICE"` | 确认 mapper 上存在文件系统标识 |
| `mount -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"` | 首轮 guest 中读写挂载 |
| `mount -o ro -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"` | reboot 后只读挂载并校验数据 |
| `umount "$MOUNT_DIR"` | 卸载，通常在 offline resize 或 guest 结束前执行 |
| `e2fsck -f -y "$MAPPER_DEVICE"` | resize 前后检查文件系统一致性 |
| `resize2fs "$MAPPER_DEVICE"` | LV grow 后扩容 ext2 |
| `resize2fs "$MAPPER_DEVICE" <smaller-size>` | LV shrink 前缩小 ext2 |
| `dd ... conv=fsync status=none` | raw BIO 测试或 LVM2 文件写入，`conv=fsync` 确保同步落盘 |
| `md5sum -c <file>.md5` | 验证 mapper 读回数据、backing 分布或 reboot 后文件数据未损坏 |
| `sync` | guest 关机前同步文件数据和 LVM 元数据 |
| `df -h` / `du -sh` | 辅助确认文件系统容量和测试文件占用 |

raw BIO 测试通常用 `dd` 精确制造跨 target 或跨 stripe chunk I/O；LVM2 reboot 测试通常用 `md5sum` 验证首轮 guest 写入的数据在第二轮 guest 中仍可读且内容一致。

## 11. 常用测试选择速查

| 目标 | 命令 |
|---|---|
| DM table / target 数据面 ktest | `myshell/ktest_crate.sh kernel/core/comps/device-mapper aster_device_mapper::table::tests::<test_name>` |
| DM ioctl 控制面 ktest | `myshell/ktest_crate.sh kernel/core aster_core::device::misc::device_mapper::tests::<test_name>` |
| quick smoke | `GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --quick` |
| dmsetup CLI 语义 | `GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --dmsetup-cli` |
| LVM2 CLI 语义 | `GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --lvm2-cli` |
| raw linear cross-target BIO | `GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --linear-data` |
| raw striped BIO | `GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --striped-data` |
| raw 数据面边界审计 | `GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --dataplane-edge` |
| linear LVM2 基础 | `GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --linear-lvm2` |
| linear cross-segment | `GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --linear-lvm2-cross-segment` |
| striped LVM2 基础 | `GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --striped-lvm2` |
| 3-way striped 基础 | `STRIPED_PV_COUNT=3 GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --striped-lvm2` |
| striped N-to-2N cross-segment | `GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --striped-lvm2-cross-segment` |
| 3-way 到 6 盘 striped cross-segment | `STRIPED_CS_PV_COUNT=3 GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --striped-lvm2-cross-segment` |
| mixed linear + striped | `GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --mixed-lvm2` |

阶段验收时按相关路径显式组合上述 suite；当前不提供无参数默认运行或 `--full` 聚合入口。
