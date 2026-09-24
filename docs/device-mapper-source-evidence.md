# Device Mapper 源码证据交接

> **证据快照日期**：2026-09-23
>
> **记录基线**：`05ab2beed9d0c0ccdc081b0db1d2c271e7f51593`
>
> **适用范围**：Device Mapper V4 已完成的控制面、table/BIO 生命周期、C ABI、NixOS 系统验收与 P0 table-scoped readonly 语义。
>
> **目的**：让新窗口先以文件内容指纹确认既有源码结论是否仍适用；hash 一致时复用本表结论并定点阅读符号，不重新进行全局源码检索。

## 1. 使用规则

本文件是“已核对源码 + 实际验证”的证据索引，不替代当前源码。

1. 新窗口先读取 [device-mapper-progress.md](../log/device-mapper-progress.md) 与本文件。
2. 任务涉及某个主题时，只核对该主题列出的源码文件 blob hash。
3. hash 一致时，默认复用“已确认事实”和“运行证据”；仅按任务阅读表内列出的符号。
4. hash 不一致时，只重新审查变更文件、表中依赖文件和相关符号；不要因单个文件变化而全局重搜。
5. 新任务超出本表主题、依赖边变化、或需要新的设计/安全结论时，必须重新核对源码并新增一行证据。
6. 本表不覆盖来源不明的工作区改动。接手前始终先执行 `git status --short`，并隔离非本阶段改动。

文件内容指纹可通过以下形式核对：

```bash
git hash-object <表中列出的文件路径>
```

`git hash-object` 匹配表示当前工作区该文件内容与本快照完全一致；它比行号更可靠。行号只用于首次定位，后续以符号和 hash 为准。

## 2. 当前工作区边界

本快照建立时，以下工作区项不属于 DM V4 测试阶段，不能与本表的工程结论混用：

| 类别 | 当前项 | 处理原则 |
|---|---|---|
| 协作规则 | `AGENTS.md`、`CLAUDE.md` | 等待单独审阅，不据此推导 DM 行为。 |
| 学习资料 | `docs/plan.md`、`docs/study.md` | 等待单独审阅，不据此推导工程事实。 |
| DOCX 替换关系 | 两个删除的历史 DOCX、两个未跟踪 DOCX | 来源、权威性和替换关系未确认。 |

## 3. 源码事实证据表

| 主题 | 源码与内容指纹 | 已确认事实 | 直接运行证据 | 新窗口最小阅读范围 |
|---|---|---|---|---|
| `DM_DEV_WAIT` 用户 ABI | `kernel/core/src/device/misc/device_mapper.rs`<br>`951ec8eacc0009e073ee7d0f2390f629ca637c7e` | `DM_DEV_WAIT` 不持有 control lock 等待；`EINTR` 映射为 `ERESTARTSYS`，由用户信号重启路径决定是否重启原 ioctl；rename/setuuid 更新 event/header 并唤醒 waiter。 | core DM ioctl：81/0；focused C：182/0。 | `DM_DEV_WAIT_CMD` dispatch、`device_wait`、`restart_interrupted_wait`、`wait_for_device_event`；C 的两个 WAIT 用例。 |
| table tracked lease 与旧 BIO | `kernel/core/comps/device-mapper/src/device.rs`<br>`581e203251a460f1c79d90233bbd985b66423b4d`<br>`kernel/core/comps/device-mapper/src/table.rs`<br>`de0126bc431caebe9b4a2287051096038450b4ac`<br>`kernel/core/comps/device-mapper/src/manager.rs`<br>`259b70b7e6fda7b97c5c1235fd7e91572cd54a58` | active/inactive table 保持 backing lease；clear、替换和旧 in-flight BIO completion 后按 holder 生命周期释放。failed table-load 不覆盖既有非空状态。 | DM crate：86/0；core DM ioctl：81/0。 | table 构造/lease、DmDevice table swap/completion、manager 生命周期；仅在这些文件变化时复核。 |
| table-scoped `DM_READONLY_FLAG` | `kernel/core/comps/device-mapper/src/device.rs`<br>`581e203251a460f1c79d90233bbd985b66423b4d`<br>`kernel/core/comps/device-mapper/src/table.rs`<br>`de0126bc431caebe9b4a2287051096038450b4ac`<br>`kernel/core/comps/device-mapper/src/manager.rs`<br>`259b70b7e6fda7b97c5c1235fd7e91572cd54a58`<br>`kernel/core/src/device/misc/device_mapper.rs`<br>`951ec8eacc0009e073ee7d0f2390f629ca637c7e`<br>`kernel/core/src/device/misc/device_mapper/control.rs`<br>`b9e1c4e4b792d7e5115d10f8a8a839331c240ff1`<br>`test/initramfs/src/regression/device/device_mapper.c`<br>`dce15653bdecad0c74ee1bb5d2c8be444f7eeb5e`<br>`myshell/run_dm_control_plane_test.sh`<br>`21d2d60e6a3b08ee5db5b394e5da01ef63140cfd` | readonly 随 immutable table 暂存；tableless create 不持久化 flag；load 不改变 active I/O mode，首次/替换 resume 才切换；selected table 与 mode 在同一 state snapshot 中查询和分派。readonly active 在 suspend 时仍立即拒绝新 write-like BIO，先前 writable 的 postponed BIO 在 readonly replacement 上以 I/O error 完成。 | DM crate：86/0；core DM ioctl：81/0；focused C：181/0（新增 transition 23/0）；重建 NixOS 镜像后 control-plane：`SUMMARY_GAP=0`。 | `DmTable::new_targets_with_readonly`、`DmDevice::table_snapshot`/`enqueue`/`resume`、table-load parse、`QueryWorkflow`、C transition 用例与 control-plane Step 11。 |
| 禁止 DM-on-DM backing | `kernel/core/comps/device-mapper/src/table.rs`<br>`de0126bc431caebe9b4a2287051096038450b4ac` | linear/striped 遇到真实 `DmDevice` backing 返回 `UnsupportedBackingDevice`；本阶段只保护拒绝边界，不实现 stacking。 | DM crate：86/0；core DM ioctl：81/0。 | `TableBuilder` backing 检查与对应 tests。 |
| block range wrapper | `kernel/core/comps/block/src/bio.rs`<br>`138acd7a7b9b283cad56a671a575da068c926176`<br>`kernel/core/comps/block/src/impl_block_device.rs`<br>`9f324fd2e35be87af6a5053bae502066a87f324b` | discard/zeroout 的同步、异步、batch/callback 与 enqueue/完成错误通道由 block wrapper 负责；C ABI 的 byte range 另行验证。 | block crate：23/0；focused C：182/0。 | `discard_sectors*`、`zeroout_sectors*`、BIO fixture/tests。 |
| runtime block 节点与恢复边界 | `kernel/core/src/device/registry/block.rs`<br>`f6f8e2426d57a99a84bcbf05882da94f2b236387` | runtime remove 有 primary 删除失败后的恢复辅助路径；当前已验证路径冲突回滚与部分 lifecycle 边界。真实 alias 深层恢复仍是未实施项，不能因现有测试关闭。 | core registry：7/0；focused C：182/0。 | runtime remove、`recover_primary_removal_failure`、相关 tests；任务涉及 alias 真实补偿时必须重审。 |
| focused DM C ABI | `test/initramfs/src/regression/device/device_mapper.c`<br>`dce15653bdecad0c74ee1bb5d2c8be444f7eeb5e`<br>`test/initramfs/src/regression/common/test.h`<br>`e8d43c9a406545b63c2966c7a4f2f81ec4358ecf` | C ELF 覆盖 tableless ioctl、runtime node rollback、ext2 mount lease、range ioctl、table-scoped readonly transition、WAIT `EINTR` / `SA_RESTART` / rename-setuuid 唤醒；公共框架在 ELF 末尾输出累计断言数。 | 在线与离线 focused `device/device_mapper`：182 passed，0 failed。 | `device_mapper_*` 测试函数、`__TEST_SUMMARY`、`main` 总计输出。 |
| DM NixOS harness 日志语义 | `myshell/lib/dm_nixos_test.sh`<br>`255f237290459c85dc7edaf553492d482675fb77`<br>`myshell/run_dm_system_tests.sh`<br>`ef6567cee83e77bae1a925bb852d2ed07db3db76` | 每个 suite 的 `/tmp/*-test.log` 是唯一权威日志；host/QEMU/guest 通过临时 FIFO 单 writer 写入，正常路径不留下 `*-qemu-running.txt`，FIFO 退出即删。 | control-plane 对单 writer 动态验证通过；六项 suite 均无持久 FIFO、QEMU 或状态文件残留。 | `dm_init_log`、`dm_emit`、`dm_close_log`、`dm_check_no_qemu`、统一 selector 的最终 marker。 |
| LVM2 topology 报表边界 | `myshell/run_lvm2_topology_test.sh`<br>`a6ad7fd0214d58842c369c726a57fd3c1a83d51c` | cross-PV linear LV 的汇总报表验证 `seg_count=2`；不要在未使用 `--segments` 时请求 `segtype` 并要求单行。DM table/dependencies 仍验证两个 backing。 | topology 首次发现重复汇总行误报；修正断言后复跑通过，`SUMMARY_GAP_LVM2_TOPOLOGY: 0`。 | `LVS_LINEAR_CROSS_PV_EXTENDED`、`record_dm_state`、`expect_dm_deps_set`。 |

## 4. 当前运行证据

以下命令均在 `myAsterinas` 容器的 `/root/asterinas` 中串行执行；结果记录的是实际命令，不是预期命令。

| 层次 | 命令或 selector | 实际结果 | 结论边界 |
|---|---|---|---|
| DM crate ktest | `kernel/core/comps/device-mapper`：`CONSOLE=ttyS0 cargo osdk test` | 在线 86/0。 | DM component 内部语义；不替代 core/C/system 层。 |
| block crate ktest | `kernel/core/comps/block`：`CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test` | 23/0。 | BIO/block wrapper。 |
| core DM ioctl | `kernel/core`：`CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test --kcmd-args=earlycon aster_core::device::misc::device_mapper::tests` | 81/0。 | DM ioctl/runtime 内部断言。 |
| focused C ABI | `RELEASE=1 AUTO_TEST=regression INTEL_TDX=0 REGRESSION_TESTS=device/device_mapper make run_kernel` | 在线 159/0。 | 原始用户 ABI；不替代真实 CLI/LVM2。 |
| core crate 全量 ktest | `kernel/core`：`CONSOLE=ttyS0 cargo osdk test --kcmd-args=earlycon` | 在线 205/0；runner 为 16 crates、621 tests。 | 当前 core crate 全量证据；默认在线 Cargo/libgit2 已可拉取 `smoltcp`。 |
| 基础 NixOS suite | `--control-plane`、`--dataplane`、`--lvm2-topology` | 全部通过。 | 分别证明控制面、裸块数据面与 LVM2 拓扑；三项合并仍不等于完整文件系统闭环。 |
| 完整存储 suite | `--linear-integration`、`--striped-integration`、`--mixed-integration` | 全部通过；共 8 次 guest 启动。 | LVM2、ext2、文件 MD5、扩缩容与有序关机后的跨启动恢复；不是断电/崩溃一致性。 |

完整非 TDX initramfs regression 已在线完成构建与 guest 启动，证明此前 DCAP/502 不再阻断 `INTEL_TDX=0` 构建图；但 `/test/network` 的 `test_tcp_append_after_peek_full_read` 为 137/1 失败。该失败不是 DM、代理或构建失败，不能把完整 regression 记为通过。

## 5. 未关闭的边界

| 项目 | 当前状态 | 重新开启条件 |
|---|---|---|
| mount source 类型 | 未实施。 | 用户单独授权或出现用户可见 mount source 失败。 |
| 真实 alias/VFS 深层恢复 | 未实施。 | 需要证明 primary 删除失败后 alias 真实恢复或隔离/重试。 |
| DM-on-DM stacking | 未实现。 | 目标明确转为 stacking 设计，而不是补测试。 |
| exfat、RawDisk/subset/clone lease | 条件性候选。 | 出现 DM 可见失败、数据风险或新 target 依赖。 |
| VirtIO/NVMe 故障注入 | 条件性候选。 | 有真实、可重复的 transport/CQ 故障注入点。 |
| TCP `MSG_PEEK` 完整 C regression 失败 | 非 DM 回归失败。 | 网络子系统单独定位；不得阻断或篡改 DM focused 验收结论。 |

## 6. 新窗口接手模板

新窗口应按以下顺序行动：

```text
CLAUDE.md
→ log/device-mapper-progress.md
→ docs/device-mapper-source-evidence.md
→ git status --short
→ 对当前任务相关主题核对本表 blob hash
→ hash 不同才对相关源码做定点检索
```

阶段结束时，只在下列情况更新本文件：

- 已确认的生产源码、测试源码或测试 harness 内容变化；
- 新增或关闭一个可验证的行为边界；
- 实际运行证据改变；
- 已确认新的未覆盖边界。

不要把完整终端输出、临时日志内容或未经验证的推测复制到本文件。
