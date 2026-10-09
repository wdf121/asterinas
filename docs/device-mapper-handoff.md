# Device Mapper 对话交接

> **最后更新**：2026-10-09。
> 本文件是唯一维护的交接入口；PR 技术档案见 [log/PR](../log/PR/)，实际操作见 [daily](../log/daily/)。

## 当前停点

- 本地 dm HEAD 为 `2699dfed0`，本地 main 为 `10a241870`。用户已提供 dm 推送成功输出；本轮未查询远端。
- dm 已线性 rebase；后续禁止用 merge 同步。
- PR 1 已无冲突 rebase 到本地 main，HEAD 为 `67ed6c70f`；range-diff 一致、三文件范围不变、无 merge。用户反馈验证通过，未提供原始输出；新 SHA 尚无本次推送记录。
- PR 2 分支 `dm-linear-io-path` 已建立，用户已手敲最小 linear 和 DmDevice，开始首个构造测试；不是只读规划阶段，也尚未完成制备。
- 当前在 dm；PR 2 代码保存在 stash `43291518c58869e5dd59efb1bc253f1af378c985`（pr2-code），不在分支 HEAD。文档已恢复，后续切分支前先保存文档。
- 文档收尾已授权，不新建文档、不提交或 push。用户手敲与执行测试，助手按完整的小主题讲解、核对，不代写功能；曾单独获授权执行 PR 1 rebase 和备份分支删除。

## 已有验证与边界

| 已执行事项 | 结果与覆盖边界 |
|---|---|
| 10 月 8 日完整 ktest | 退出码 0，但早于随后 lint 源码清理，不能当作当前快照全量验证。 |
| 后续静态检查、普通内核构建 | 当时 make check 和 dev 内核/启动 ISO 构建退出码 0；不代表本轮文档再次检查，也不是 release 或 NixOS 验收。 |
| 10 月 8—9 日定向复测 | DM component 87/0、core ioctl 81/0、devtmpfs 6/0、runtime registry 8/0、VMCS 1/0，合计 183 项通过，各命令退出码 0。 |
| 未覆盖 | 本轮未重跑 C guest、NixOS suite；普通 VFS rename、完整 mount source 路径与 procfs 不以邻近模块通过代替直接验证。 |

VMCS 首次 selector 零匹配不算通过，修正后实际执行 1 项通过。
详细命令、源码差异指纹和原始输出索引见 [10 月 8 日日志](../log/daily/2026-10-8.md) 与 [10 月 9 日日志](../log/daily/2026-10-9.md)。

## 验证位置与规则

- **PR：在新 PR 分支最终验证。** 顺序为 make check、make kernel、相关定向 ktest；dm 成功结果不能代替 PR 分支。最终提交源码树必须与实际验证树一致。
- **上游同步：在 rebase 后的权威 dm 最终验证。** 同步前仍在容器干净独立 worktree 对候选 main 做基线验证；main 与 dm 均默认定向，不运行完整 ktest。
- 定向覆盖新增、冲突解决与接口/语义影响链；用户 ABI 或真实工具流程按影响补 C/系统测试。影响难以限定时先讨论，不自动扩大范围。
- make check 已包含格式检查，不重复列必跑格式命令；make kernel 默认 dev。系统 suite 启动已有镜像，必须确认镜像来源和 profile。
- 具体规则以 [PR 规范](pull_request.md#13-每个-pr-的最终验证)、[上游同步](upstream-sync.md#8-后续同步规则) 为准；测试入口见 [测试手册](test.md)。

## 工作区边界

- 前五个提交已收录的源码修改包括 VMCS 测试 guard 生命周期、DM lint 清理、VFS/type 无调用残留删除、devtmpfs/procfs 调整及三个 C 回归文件格式化。
- Makefile、typos 配置与历史日志空白清理已独立提交；协作规范及验证文档归入文档提交。不得将这些改动全部纳入 upstream PR。
- 未跟踪的构建产物、其他文档和来源不明改动不删除、不格式化、不暂存。实际文件清单以接手时的工作区为准。
- `patches/` 等 PR 提取备份不是最终提交或验收证据，不进入 upstream PR。

## PR 状态与下一步

- PR 1 的旧 `c14d68c93` 有历史验证和推送记录；当前本地 `67ed6c70f` 的验证只有用户反馈，不混用旧快照证据。详见 [PR 1 档案](../log/PR/PR-1-mapped-bio-ranges.md)。
- PR 2 完成 crate 接入、LinearTarget 构造/元数据/转发、DmDevice 接口，首个几何测试已写入 stash，未报告测试执行结果。组件 cargo osdk check 通过是用户在添加测试前的反馈。
- 保留局部校验，不扩展 block 框架；不变量、空请求及统一准入校验后续在 dm 讨论。完整取舍已归档到 [PR 规范第 5 节](pull_request.md#5-pr-2-制备状态与设计取舍)，不再依赖 Git 本地临时笔记。
- 旧 lease 分支和两条 backup 分支已删除；后续是否建立备份先问用户。
- 下一步从构造测试组继续，先补边界用例，再验证转发与完成路径。测试 cfg 注释是用户有意调整，不擅自修复。恢复代码前核对 stash 身份，不将其应用到 dm 的完整组件上。
- PR 是完整 dm 的缩小版本，适配在 PR 分支验证；共有缺陷仍先修 dm。每份后续 PR 基于上一份 PR + 最新已核对 main。

## 阅读顺序

[CLAUDE.md](../CLAUDE.md) / [AGENTS.md](../AGENTS.md)
→ 本交接
→ [项目状态](../log/device-mapper-progress.md)
→ [PR 规范](pull_request.md) / [上游同步](upstream-sync.md)
→ [当日日志](../log/daily/2026-10-9.md)。

QEMU、ktest、C 回归和 NixOS 测试必须串行；遇失败、环境阻塞或未预期 warning 先报告并讨论。
不覆盖来源不明改动，不自动提交或 push。
