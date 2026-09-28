# Device Mapper 源码证据交接

> **证据快照日期**：2026-09-24
>
> **记录基线**：`a4603e369`（正式 upstream 同步 merge）、`cdefc74dc`（同步验证记录）与 `0423431d5`（alias 删除失败修复）；本提交收口已验证的 split child 完成责任修复。
>
> **适用范围**：Device Mapper V4 已完成的控制面、table/BIO 生命周期、C ABI、NixOS 系统验收与 P0 table-scoped readonly 语义，以及正式同步后的直接适配与修复。
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
| `DM_DEV_WAIT` 用户 ABI | `kernel/core/src/device/misc/device_mapper.rs`<br>`4b224229ca4a1d0b13f79c14229ecf94d8a0567a` | `DM_DEV_WAIT` 不持有 control lock 等待；`EINTR` 映射为 `ERESTARTSYS`，由用户信号重启路径决定是否重启原 ioctl；rename/setuuid 更新 event/header 并唤醒 waiter。 | core DM ioctl：81/0；focused C：182/0。 | `DM_DEV_WAIT_CMD` dispatch、`device_wait`、`restart_interrupted_wait`、`wait_for_device_event`；C 的两个 WAIT 用例。 |
| table tracked lease 与旧 BIO | `kernel/core/comps/device-mapper/src/device.rs`<br>`685f3a36e7bb148e23c27cd4af09afa94554b289`<br>`kernel/core/comps/device-mapper/src/table.rs`<br>`a10001060379307758b2b698a51406106099e45f`<br>`kernel/core/comps/device-mapper/src/manager.rs`<br>`eb0598fb665896dfc70cf4e6919bf284a144e968` | active/inactive table 保持 backing lease；clear、替换和旧 in-flight BIO completion 后按 holder 生命周期释放。failed table-load 不覆盖既有非空状态。 | DM crate：87/0；core DM ioctl：81/0。 | table 构造/lease、DmDevice table swap/completion、manager 生命周期；仅在这些文件变化时复核。 |
| table-scoped `DM_READONLY_FLAG` | `kernel/core/comps/device-mapper/src/device.rs`<br>`685f3a36e7bb148e23c27cd4af09afa94554b289`<br>`kernel/core/comps/device-mapper/src/table.rs`<br>`a10001060379307758b2b698a51406106099e45f`<br>`kernel/core/comps/device-mapper/src/manager.rs`<br>`eb0598fb665896dfc70cf4e6919bf284a144e968`<br>`kernel/core/src/device/misc/device_mapper.rs`<br>`4b224229ca4a1d0b13f79c14229ecf94d8a0567a`<br>`kernel/core/src/device/misc/device_mapper/control.rs`<br>`b189c7f56ad271a05fae5e90fd34af6f0960806d`<br>`test/initramfs/src/regression/device/device_mapper.c`<br>`dce15653bdecad0c74ee1bb5d2c8be444f7eeb5e`<br>`myshell/run_dm_control_plane_test.sh`<br>`21d2d60e6a3b08ee5db5b394e5da01ef63140cfd` | readonly 随 immutable table 暂存；tableless create 不持久化 flag；load 不改变 active I/O mode，首次/替换 resume 才切换；selected table 与 mode 在同一 state snapshot 中查询和分派。readonly active 在 suspend 时仍立即拒绝新 write-like BIO，先前 writable 的 postponed BIO 在 readonly replacement 上以 I/O error 完成。 | DM crate：87/0；core DM ioctl：81/0；focused C：182/0（新增 transition 23/0）；重建 NixOS 镜像后 control-plane：`SUMMARY_GAP=0`。 | `DmTable::new_targets_with_readonly`、`DmDevice::table_snapshot`/`enqueue`/`resume`、table-load parse、`QueryWorkflow`、C transition 用例与 control-plane Step 11。 |
| 禁止 DM-on-DM backing | `kernel/core/comps/device-mapper/src/table.rs`<br>`a10001060379307758b2b698a51406106099e45f` | linear/striped 遇到真实 `DmDevice` backing 返回 `UnsupportedBackingDevice`；本阶段只保护拒绝边界，不实现 stacking。 | DM crate：87/0；core DM ioctl：81/0。 | `TableBuilder` backing 检查与对应 tests。 |
| block range wrapper | `kernel/core/comps/block/src/bio.rs`<br>`690f735097f1c91d212f6eb7028db58bc5f0d7a2`<br>`kernel/core/comps/block/src/impl_block_device.rs`<br>`e7ab799e3b4a2840709f51c740e908c19815fad7` | discard/zeroout 的同步、异步、batch/callback 与 enqueue/完成错误通道由 block wrapper 负责；C ABI 的 byte range 另行验证。 | block crate：25/0；focused C：182/0。 | `discard_sectors*`、`zeroout_sectors*`、BIO fixture/tests。 |
| split child 完成责任 | `kernel/core/comps/block/src/bio.rs`<br>`690f735097f1c91d212f6eb7028db58bc5f0d7a2`<br>`kernel/core/comps/device-mapper/src/device.rs`<br>`685f3a36e7bb148e23c27cd4af09afa94554b289`<br>`kernel/core/comps/device-mapper/src/table.rs`<br>`a10001060379307758b2b698a51406106099e45f` | split child 是线性完成责任：正常完成或未完成析构均恰好向父 BIO 报告一次；不暴露可重复调用的聚合 handle。deferred replay 的失败 child 必须等待其余 child 终结后才完成父 BIO 并释放 table/in-flight 保活。 | block crate：25/0；DM crate：87/0。 | `SubmittedBio::split`/`Drop`、`SplitBioCompletion`、`DmTable::execute_normal_io`、`DmDevice::dispatch_assigned_bio` 与 deferred replay 跨 target 失败用例。 |
| runtime block 节点与删除失败恢复 | `kernel/core/src/device/registry/block.rs`<br>`1f01c8b6531fcb2cb3128335db47c087b3805214`<br>`kernel/core/src/fs/fs_impls/devtmpfs/mod.rs`<br>`b56b82c05e9c89eed60dda8b9ca530f74678811e`<br>`kernel/core/src/fs/fs_impls/devtmpfs/worker.rs`<br>`bf583d7d3f1a2e7e3caa3587366757200974048d` | runtime remove 仅在 identity 删除成功后清除 primary/alias handle；非 `ENOENT`/`ESTALE` 删除错误保留 handle，恢复 `Live` 后可 retry。外来替换仍由 `ESTALE` 保留外物。真实 alias/VFS 深层恢复仍未实施。 | devtmpfs 6/0；core registry 8/0；focused C 182/0。 | `unregister_mapper`、`delete_mapper_alias`、`delete_node`、`devtmpfs::delete` 与删除失败重试用例。 |
| 同步后的 stable primary / mutable alias | `kernel/core/comps/device-mapper/src/device.rs`<br>`685f3a36e7bb148e23c27cd4af09afa94554b289`<br>`kernel/core/comps/device-mapper/src/manager.rs`<br>`eb0598fb665896dfc70cf4e6919bf284a144e968`<br>`kernel/core/src/device/misc/device_mapper.rs`<br>`4b224229ca4a1d0b13f79c14229ecf94d8a0567a` | `BlockDevice::name()` 只返回 immutable `dm-N` primary；`DmDevice::mapper_name()` 在锁内快照可变 mapper alias，control/manager/runtime 不借用 alias 为 `&str`。`DM_VERSION` 与 Linux UAPI 收敛为 4.50.0。 | core check、DM crate 86/0、core DM ioctl 81/0、focused C 182/0 与六项 NixOS suite 通过。 | `DmDevice` 的 primary/alias 字段、`mapper_name`/`rename`、`BlockDevice::name`、control/manager 的 alias 使用与 `DM_VERSION`。 |
| focused DM C ABI | `test/initramfs/src/regression/device/device_mapper.c`<br>`dce15653bdecad0c74ee1bb5d2c8be444f7eeb5e`<br>`test/initramfs/src/regression/common/test.h`<br>`e8d43c9a406545b63c2966c7a4f2f81ec4358ecf` | C ELF 覆盖 tableless ioctl、runtime node rollback、ext2 mount lease、range ioctl、table-scoped readonly transition、WAIT `EINTR` / `SA_RESTART` / rename-setuuid 唤醒；公共框架在 ELF 末尾输出累计断言数。 | 在线与离线 focused `device/device_mapper`：182 passed，0 failed。 | `device_mapper_*` 测试函数、`__TEST_SUMMARY`、`main` 总计输出。 |
| DM NixOS harness 日志语义 | `myshell/lib/dm_nixos_test.sh`<br>`255f237290459c85dc7edaf553492d482675fb77`<br>`myshell/run_dm_system_tests.sh`<br>`ef6567cee83e77bae1a925bb852d2ed07db3db76` | 每个 suite 的 `/tmp/*-test.log` 是唯一权威日志；host/QEMU/guest 通过临时 FIFO 单 writer 写入，正常路径不留下 `*-qemu-running.txt`，FIFO 退出即删。 | control-plane 对单 writer 动态验证通过；六项 suite 均无持久 FIFO、QEMU 或状态文件残留。 | `dm_init_log`、`dm_emit`、`dm_close_log`、`dm_check_no_qemu`、统一 selector 的最终 marker。 |
| LVM2 topology 报表边界 | `myshell/run_lvm2_topology_test.sh`<br>`a6ad7fd0214d58842c369c726a57fd3c1a83d51c` | cross-PV linear LV 的汇总报表验证 `seg_count=2`；不要在未使用 `--segments` 时请求 `segtype` 并要求单行。DM table/dependencies 仍验证两个 backing。 | topology 首次发现重复汇总行误报；修正断言后复跑通过，`SUMMARY_GAP_LVM2_TOPOLOGY: 0`。 | `LVS_LINEAR_CROSS_PV_EXTENDED`、`record_dm_state`、`expect_dm_deps_set`。 |

## 4. 当前运行证据

2026-09-28 在 `fork_Asterinas` 容器的 `/root/asterinas`、`dm` SHA `78f4eb24a` 上重新运行下表 DM/block/core ktest、focused C ABI 和六项 DM NixOS suite。表中的参数是当日实际执行证据，不是当前默认命令；默认验证不设置 `CARGO_NET_OFFLINE=true` 或 `CONSOLE=ttyS0`。更早的细节仅作为历史背景，不替代该 SHA 的当前证据。

| 层次 | 命令或 selector | 实际结果 | 结论边界 |
|---|---|---|---|
| DM crate ktest | `kernel/core/comps/device-mapper`：`CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test` | `fork_Asterinas`：87/0。 | DM component 内部语义；不替代 core/C/system 层。 |
| block crate ktest | `kernel/core/comps/block`：`CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test` | `fork_Asterinas`：27/0。 | BIO/block wrapper。 |
| core DM ioctl | `kernel/core`：`CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test --kcmd-args=earlycon aster_core::device::misc::device_mapper::tests` | `fork_Asterinas`：81/0。 | DM ioctl/runtime 内部断言。 |
| core block registry | `kernel/core`：`CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test --kcmd-args=earlycon aster_core::device::registry::block::tests` | `fork_Asterinas`：8/0。 | runtime block registry 生命周期。 |
| core devtmpfs | `kernel/core`：`CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test --kcmd-args=earlycon aster_core::fs::fs_impls::devtmpfs::tests` | `fork_Asterinas`：6/0。 | 运行期节点与 identity 路径。 |
| focused C ABI | `CARGO_NET_OFFLINE=true RELEASE=1 AUTO_TEST=regression INTEL_TDX=0 REGRESSION_TESTS=device/device_mapper make run_kernel` | `fork_Asterinas`：182/0。 | 原始用户 ABI；不替代真实 CLI/LVM2。 |
| core crate 全量 ktest | `kernel/core`：`CARGO_NET_OFFLINE=true CONSOLE=ttyS0 cargo osdk test --kcmd-args=earlycon` | `fork_Asterinas`：232/0。 | 当前 core crate 全量证据。 |
| 基础 NixOS suite | `--control-plane`、`--dataplane`、`--lvm2-topology` | 2026-09-28 在 `fork_Asterinas` 重建镜像后全部重跑通过。 | 分别证明控制面、裸块数据面与 LVM2 拓扑；三项合并仍不等于完整文件系统闭环。 |
| 完整存储 suite | `--linear-integration`、`--striped-integration`、`--mixed-integration` | 2026-09-28 在 `fork_Asterinas` 重建镜像后全部重跑通过。 | LVM2、ext2、文件 MD5、扩缩容与有序关机后的跨启动恢复；不是断电/崩溃一致性。 |

完整非 TDX initramfs regression 已在 `fork_Asterinas` 的当前 `dm` SHA 上完成构建与 guest 启动，但 `test/initramfs/src/regression/process/cgroup.sh` 的 `cpu.stat` busy-loop 记账只得到 467ms，低于 1.9s 阈值，因此以失败退出。该失败在 Cgroup CPU accounting 路径，不是 DM、代理或构建失败；完整 regression 不能记为通过。

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
