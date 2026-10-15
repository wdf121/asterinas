# Device Mapper 对话交接

> **最后更新**：2026-10-14。
> 本文件是唯一维护的交接入口；PR 技术档案见 [log/PR](../log/PR/)，实际操作见 [daily](../log/daily/)。

## 当前停点

- 2026-10-14:PR 3（`dm-control-interface`，基于 PR 2）Stage 1 学习式手敲完成，`d539b725b` = 控制设备骨架 + `DM_VERSION` 握手 + flags 校验 + 19 ktest；六条外部 review 评判修复完毕（详见表后说明与档案），gate 全绿（check/kernel exit 0，aster-core 全量 ktest 160/0）；未推送。档案：[PR-3-control-interface](../log/PR/PR-3-control-interface.md)。
- 2026-10-14：PR 2（`dm-linear-io-path`）清除 6 条中文注释洗史至 `d0088c2c9` 并 force-with-lease 重推，ls-remote 实证远端一致；改史后 gate 全绿（check/kernel exit 0，block 6/0，dm 8/0）。PR 未创建，编号待回填。PR 1 本地重放至 `716a1161b`，远端 #3937 分支仍为旧历史，重推待定。
- 2026-10-13：完成上游 0.19.0 同步，main 快进至 `77ee3b196`，dm 154 提交线性重放（无 merge commit）；dm 的 force push 挂起，待用户统一执行；同步策略已固化进 CLAUDE.md/AGENTS.md。
- OSDK 0.19.0 ktest selector 为后缀 trie：裸模块路径选择器必匹配空集（EXIT=0 假绿）；定向入口为无 selector 的 crate 全量加串口逐行取证；10-13 日志集成层 81/159 记录已勘误作废。
- 此前结论仍有效（10-12/10-13 早段）：`--linear-integration` 两层根因闭环、五套件结构化日志串行全绿、C 回归 focused 182/0（cgroup 全量失败定性为环境限制）、文档按授权更新；细节见各日日志。

## PR 状态

- PR 逐项状态、分支 SHA 与下一步以 [PR 规范](pull_request.md) 和 [log/PR](../log/PR/) 档案为准，本文件不再复述具体 SHA、stash 或分支。
- PR 1/2/3 档案均在 [log/PR](../log/PR/)（PR-1、PR-2、PR-3-control-interface）；PR 2 与 PR 3 尚未创建，编号与 PR 3 的 base 取向待用户创建时回填。
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
- **上游同步：在 rebase 后的权威 dm 最终验证。** 同步入口固定：fetch 上游 → main 快进 → 侦察增量 → 按影响面做基线。基线（make check、make kernel）在检出 main 的主工作区容器内直接执行，不用嵌套 worktree（向上解析撞主 workspace 根，checkout 时间戳触发 cargo-osdk 重装）；main 与 dm 均默认定向，不运行完整 ktest。
- 定向覆盖新增、冲突解决与接口/语义影响链；用户 ABI 或真实工具流程按影响补 C/系统测试。影响难以限定时先讨论，不自动扩大范围。
- make check 已包含格式检查，不重复列必跑格式命令；make kernel 默认 dev。系统 suite 启动已有镜像，必须确认镜像来源和 profile。
- **DM 系统 suite 日志**：每次运行写入 `/tmp/dm-test/<slug>/<日期-时间>/` 运行目录（`latest` 指向最新），验收以该目录的 `summary.log`/`console.clean.log` 为准；详见[测试手册 5.2.3](test.md#523-通过判定与日志)。
- 具体规则以 [PR 规范](pull_request.md#13-每个-pr-的最终验证)、[上游同步](upstream-sync.md#8-后续同步规则) 为准；测试入口见 [测试手册](test.md)。

## 工作区边界

dm 在 main（`77ee3b196`）之上的提交序列（SHA 为 0.19.0 线性重放后的现值；原 `6b1b24026` 起的序列已随重放改写）：

| commit | 内容 |
|---|---|
| `d20886cf0` | ext2 落盘空闲块计数一致性修复。 |
| `f1e4a3fb1` | `DM_LIST_DEVICES` 支持 `DM_UUID_FLAG`。 |
| `427bc8784` | 块设备经 `CREATES_DEVNODE` opt-out 接入设备模型。 |
| `aea2bc12e` | `install_osdk` 显式选取活动工具链。 |
| `74cd7e56c` | `REGRESSION_TESTS` 定向回归接线恢复。 |
| `e33417f58` | 内核默认构建 release（`RELEASE ?= 1`）。 |
| `71c00a91d` | DM 系统 suite 结构化运行目录日志。 |
| `358a5d6d5` | cargo-udeps 补丁归档与历史日志拼写修正。 |
| `e741a6042` | CLAUDE.md 文档更新时机规则。 |
| `8e95d775d` / `be418c5dc` / `f787680ed` / `726398086` | 文档与锁文件收尾：指南与日志同步、PR 2 描述归档、Cargo.lock 归一、10-13 日志与同步策略固化。 |

- 未跟踪的构建产物、其他文档和来源不明改动不删除、不格式化、不暂存。
- 保留物不删除：`target/nixos/asterinas.img`、`target/nixos/failed-root-MdBL2F/`、`target/nixos/pre-ext2-fix-OEsCVh/`、`/tmp/dm-nixos-recovery-M7gkiV/`、`/tmp/lvmsrc-dl/`。

## 阅读顺序

[CLAUDE.md](../CLAUDE.md) / [AGENTS.md](../AGENTS.md)
→ 本交接
→ [项目状态](../log/device-mapper-progress.md)
→ [PR 规范](pull_request.md) / [上游同步](upstream-sync.md)
→ [当日日志](../log/daily/2026-10-14.md)。

QEMU、ktest、C 回归和 NixOS 测试必须串行；遇失败、环境阻塞或未预期 warning 先报告并讨论。
不覆盖来源不明改动，不自动提交或 push。
