# Device Mapper 对话交接

> **最后更新**：2026-10-13。
> 本文件是唯一维护的交接入口；PR 技术档案见 [log/PR](../log/PR/)，实际操作见 [daily](../log/daily/)。

## 当前停点

- 2026-10-13 已把此前积累的工作区改动按逻辑点提交：9 个代码 commit + 1 个文档 commit（清单见"工作区边界"）。未 push。
- `--linear-integration` 失败的两层根因均已闭环：`DM_LIST_DEVICES` 支持 `DM_UUID_FLAG`（10-10 日志第 8 节）；块设备经 `CREATES_DEVNODE` opt-out 接入设备模型，LVM2 sysfs filter 不再拒绝（10-12 日志第 1 节）。完整套件五次运行全部 PASS。
- 系统测试日志改造为结构化运行目录形态并推广到全部五个套件：串行 5/5 全绿（10-12 日志第 4 节）；console 转义根因定性为 guest systemd 的 OSC 3008 规范广播，host 以 clean.log 字符级剥离兜底（10-12 日志第 3 节）。
- C 回归 `REGRESSION_TESTS` 的 Makefile 接线已恢复：focused 182/0；全量回归在本机稳定失败于 cgroup 2.2.4，定性为环境限制，日常使用 focused（10-12 日志第 5 节）。
- 文档更新已按授权执行（10-13）：本交接、source-evidence 日志语义行、test.md 5.2 运行目录形态与 RELEASE 默认值、global.md 修复日志提交状态、rationale 补 ext2 修复归属；"未提交"标注已回填实际 SHA。`pull_request.md` 未改。

## PR 状态

- PR 逐项状态、分支 SHA 与下一步以 [PR 规范](pull_request.md) 和 [log/PR](../log/PR/) 档案为准，本文件不再复述具体 SHA、stash 或分支。
- 用户已把最新 PR 档案更新暂存在另一分支；以恢复后的档案为准。
- 通用规则不变：每份 PR 在其 PR 分支最终验证（make check、make kernel、定向 ktest），dm 成功结果不能代替；后续 PR 基于上一份 PR + 最新已核对 main；共有缺陷仍先修 dm。

## 已有验证与边界

| 已执行事项 | 结果与覆盖边界 |
|---|---|
| 2026-10-10 静态与构建 gate | 容器 `make check` 退出 0、无警告（cargo-udeps 修补版）；`make kernel` 退出 0 并生成 ISO；仅构建验证，未做运行时测试。 |
| 2026-10-10 ext2 落盘计数修复 | 新增 ktest 用例先红后绿；ext2 模块 15/0；release 根盘端到端复验，启动前后 `e2fsck -fn` 均 0；`--control-plane` 套件复验通过。 |
| 2026-10-12 两层根因修复验证 | core DM ioctl ktest 81/0；`--linear-integration` 五次真实运行全部 PASS（87-91s，含 legacy 日志对照）。 |
| 2026-10-12 五套件结构化日志推广 | control-plane/dataplane/lvm2-topology/striped/mixed 串行全绿（47-117s）、TEST_FAIL=0、退出码全 0；运行目录产物齐全，clean.log 字符级 CLEAN。 |
| 2026-10-12 C 回归接线恢复 | `REGRESSION_TESTS=device/device_mapper` focused 182/0、退出码 0，qemu.log 仅含单一 selector。 |
| 未覆盖 | `test/registration.rs` 新增用例未单独运行 ktest（由同 crate ioctl ktest 间接覆盖编译与调用链）；cgroup 环境限制未做 upstream 对照；ESTALE 删除失败未定性；五套件与 C 回归验证时的内核/脚本内容与提交后 HEAD 一致（仅文档标注有差异），提交序列本身未重跑运行时测试。 |

各日命令、源码快照与原始输出索引见 [daily log](../log/daily/)。

## 验证位置与规则

- **PR：在新 PR 分支最终验证。** 顺序为 make check、make kernel、相关定向 ktest；dm 成功结果不能代替 PR 分支。最终提交源码树必须与实际验证树一致。
- **上游同步：在 rebase 后的权威 dm 最终验证。** 同步前仍在容器干净独立 worktree 对候选 main 做基线验证；main 与 dm 均默认定向，不运行完整 ktest。
- 定向覆盖新增、冲突解决与接口/语义影响链；用户 ABI 或真实工具流程按影响补 C/系统测试。影响难以限定时先讨论，不自动扩大范围。
- make check 已包含格式检查，不重复列必跑格式命令；make kernel 默认 dev。系统 suite 启动已有镜像，必须确认镜像来源和 profile。
- **DM 系统 suite 日志**：每次运行写入 `/tmp/dm-test/<slug>/<日期-时间>/` 运行目录（`latest` 指向最新），验收以该目录的 `summary.log`/`console.clean.log` 为准；详见[测试手册 5.2.3](test.md#523-通过判定与日志)。
- 具体规则以 [PR 规范](pull_request.md#13-每个-pr-的最终验证)、[上游同步](upstream-sync.md#8-后续同步规则) 为准；测试入口见 [测试手册](test.md)。

## 工作区边界

2026-10-13 提交序列（自 `6b1b24026` 起，9 个代码 commit + 本文件所在的文档 commit）：

| commit | 内容 |
|---|---|
| `65c650435` | ext2 落盘空闲块计数一致性修复。 |
| `3615118bb` | `DM_LIST_DEVICES` 支持 `DM_UUID_FLAG`。 |
| `535727967` | 块设备经 `CREATES_DEVNODE` opt-out 接入设备模型。 |
| `acd793859` | `install_osdk` 显式选取活动工具链。 |
| `1a513d3e6` | `REGRESSION_TESTS` 定向回归接线恢复。 |
| `bffd9e9b0` | 内核默认构建 release（`RELEASE ?= 1`）。 |
| `848fcdae2` | DM 系统 suite 结构化运行目录日志。 |
| `82337349a` | cargo-udeps 补丁归档与历史日志拼写修正。 |
| `51e3c781b` | CLAUDE.md 文档更新时机规则。 |

- 未跟踪的构建产物、其他文档和来源不明改动不删除、不格式化、不暂存。
- 保留物不删除：`target/nixos/asterinas.img`、`target/nixos/failed-root-MdBL2F/`、`target/nixos/pre-ext2-fix-OEsCVh/`、`/tmp/dm-nixos-recovery-M7gkiV/`、`/tmp/lvmsrc-dl/`。

## 阅读顺序

[CLAUDE.md](../CLAUDE.md) / [AGENTS.md](../AGENTS.md)
→ 本交接
→ [项目状态](../log/device-mapper-progress.md)
→ [PR 规范](pull_request.md) / [上游同步](upstream-sync.md)
→ [当日日志](../log/daily/2026-10-12.md)。

QEMU、ktest、C 回归和 NixOS 测试必须串行；遇失败、环境阻塞或未预期 warning 先报告并讨论。
不覆盖来源不明改动，不自动提交或 push。
