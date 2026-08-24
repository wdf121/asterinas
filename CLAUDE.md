See [AGENTS.md](AGENTS.md).

# Asterinas dm 分支实时工作说明

本文是可持续修正的项目交接文件。新对话开始时，先阅读本文件，再按需阅读 `AGENTS.md` 和相关源码；如果本文件与当前代码不一致，以当前代码和实际命令结果为准，并及时修正本文件。

## 交流与协作偏好

- 始终使用简体中文交流，技术解释和必要代码注释也使用简体中文。
- 汇报要精简，只保留卡点、失败原因、关键验证结果和最终状态。
- 新功能或新小阶段开始前，先说明“原来行为 / 目标行为 / 示例差异”，等用户确认后再实现。
- 用户说“继续”时，在当前优先级和边界内自主推进；不要擅自扩大到慢速系统验收或框架集成。
- 用户说“分点提交”表示按逻辑点拆成多个 commit，不是一个 commit 里写分点说明。
- 默认不 push；只有用户明确要求 push 才推送。
- 小阶段通常由 Claude 实现并验证，通过后由用户决定是否提交；用户明确要求提交时再提交。
- 不要创建大段临时规划文档；阶段性工程日志写入 `log/YYYY-M-D.md`，写入前先用 `date +%F` 核对当天日期。

## 执行环境与安全边界

- 仓库：`/root/atom/asterinas`。
- 当前工作分支：`dm`。
- 容器：`myAsterinas`。
- 容器内项目路径：`/root/asterinas`。
- 构建、ktest、NixOS/LVM2 系统测试优先在容器内执行：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && <command>'
```

- QEMU、ktest、NixOS 系统测试必须串行运行，避免 `test/initramfs/build/ext2.img` 等镜像锁冲突。
- ktest / QEMU 超过约 3 分钟未命中目标测试或没有关键进展，默认怀疑命令过滤、default-members、残留进程或镜像锁异常；应主动检查输出和进程状态，必要时终止自己启动的异常任务并换窄跑方式。
- 不要擅自修改 KVM、RELEASE、QEMU、NixOS 启动协议、`myshell/br.sh`。
- 不要把 `--full` 扩大为 striped 全量；新增系统入口需保持现有 `--full` 语义。
- 有框架依赖的内容，例如 udev、devtmpfs、systemd 自动联动，放在后续低优先级，不要混入 DM core 收敛任务。

## 常用验证命令

快速静态检查：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && cargo fmt --check'
git diff --check
git diff -- Cargo.toml
git status --short
```

窄跑 `aster-device-mapper` crate ktest 时，临时把根 `Cargo.toml` 的 `default-members` 缩减为：

```toml
default-members = [
    "kernel/comps/device-mapper",
]
```

然后运行，例如：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && timeout -k 10s 180s make ktest CARGO_OSDK_TEST_ARGS="--kcmd-args=loglevel=error --kcmd-args=earlycon --kcmd-args=console=ttyS0 --boot-method=grub-rescue-iso --grub-boot-protocol=multiboot2 aster_device_mapper::table::tests::<test_name>"'
```

窄跑 `aster-kernel` ioctl 层 ktest 时，临时把根 `Cargo.toml` 的 `default-members` 缩减为：

```toml
default-members = [
    "kernel",
]
```

然后运行，例如：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && timeout -k 10s 180s make ktest CARGO_OSDK_TEST_ARGS="--kcmd-args=loglevel=error --kcmd-args=earlycon --kcmd-args=console=ttyS0 --boot-method=grub-rescue-iso --grub-boot-protocol=multiboot2 aster_kernel::device::misc::device_mapper::tests::<test_name>"'
```

验证完成后必须还原 `Cargo.toml`，并用 `git diff -- Cargo.toml` 确认无输出。

相关系统验收只在对应链路变更或阶段验收时运行，例如：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --striped'
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --striped-lvm2'
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --striped-lvm2-3pv'
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --striped-lvm2-multi-segment'
```

## 当前 Device Mapper 状态

截至 2026-08-24，`dm` 分支最近相关提交：

```text
2e1a3350f 记录 dm 分支协作说明
8f00dec97 补充 Device Mapper mixed table ktest
fd94cd2c7 补充 Device Mapper table-load 非法输入 ktest
49ef85c72 修复 Device Mapper table remap ktest
487f375c3 新增 Device Mapper striped multi-segment 验收
```

当前已完成并验证的范围：

- Device Mapper linear 多 target 已支持 LVM2 跨 PV 扩容 / 缩容相关路径。
- Device Mapper striped 已覆盖 raw BIO、2-way LVM2、3PV / 3-way、multi-segment reboot recovery；ktest 已补 3-way partial final row range 拆分和 BIO 恰好结束在 striped target 末尾的边界。
- 同一 DM device / 同一 LV 内的 BIO 可以跨 target 边界拆分；这不是跨 LV。
- 一个 BIO 不应跨两个不同 LV；BIO 是发给某一个 block device 的。
- `DmTable` 数据面支持：
  - 单段 linear remap。
  - 多段 linear target。
  - striped target 内跨 stripe chunk 拆分。
  - linear + striped mixed target 的跨边界拆分。
  - split child enqueue 失败或 child 完成 `IoError` 时，原 BIO 聚合返回 `IoError`。
- ioctl 控制面支持并已测：
  - `DM_TABLE_LOAD` 多 target linear。
  - `DM_TABLE_LOAD` striped。
  - `DM_TABLE_LOAD` linear + striped mixed table。
  - mixed inactive table 的 `DM_DEV_STATUS.target_count`。
  - mixed table 的 `DM_TABLE_STATUS` type / params / next offset。
  - mixed table 的 `DM_TABLE_DEPS` backing 顺序。
  - running device 中 active linear table 被 ioctl-loaded mixed table 替换后的 active / inactive 查询、resume 切换和 deps 切换。
- table-load 非法输入已补 ktest：
  - linear 参数缺失、额外字段、bad major/minor、bad start、start 溢出。
  - striped 参数字段数、zero stripes、zero chunk、非数字 stripe_count/chunk_size、bad dev/start。
  - unsupported target：`unknown`、`error`、`snapshot`。
  - target type 空字符串、缺少 NUL 终止符。
  - `dm_target_spec.next` 的非最后 0、too-small、unaligned、out-of-bounds，以及 final target 非法 next。

## 当前功能边界

- “跨 target”指同一个 DM table 内逻辑地址跨过相邻 target，例如：

```text
0..100    linear  -> vda
100..500  striped -> vdb/vdc
```

一个 `start=96, len=8` 的 BIO 会被拆成 linear 段和 striped 段后分别下发。

- “跨 LV”不是当前应支持场景；不同 LV 是不同 block device，不应出现单个 BIO 同时覆盖两个 LV 逻辑地址空间。
- 当前 DM core 收敛优先通过 ktest 覆盖，不优先新增慢速 NixOS/LVM2 系统矩阵。
- udev、devtmpfs、systemd、LVM 自动扫描/自动激活这类用户态或框架集成，后续最后考虑。

## 后续可做优先级

1. 复查当前 mixed / split / striped boundary ktest 是否需要拆分或补充说明；如果用户要求，可按逻辑点继续拆 commit 或补日志。
2. 如能找到稳定 LVM2 命令自然生成 linear + striped mixed table，再考虑系统级验收。
3. udev / devtmpfs / systemd 自动联动放最后。

## 工程日志规则

- 写日志前先用 `date +%F` 核对当前日期，按当天日期写入 `log/YYYY-M-D.md`。
- 每天的日志阶段号都从 1 开始，不沿用前一天的阶段编号。
- `log/2026-8-21.md` 已记录当天 DM 小阶段 1 到 6；`log/2026-8-24.md` 从第 1 阶段开始记录当天改动，当前已到第 2 阶段。
- 日志按小阶段顺序写：背景、改动、测试。
- 不按“生产代码 / ktest”分类。
- 只记录实际工程改动和验证；单纯讨论、复核、规划不写入日志。
