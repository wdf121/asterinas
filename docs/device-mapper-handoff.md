# Device Mapper 对话交接

> **最后更新**：2026-10-09。
> 本文件是唯一维护的交接入口；PR 技术档案见 [log/PR](../log/PR/)，实际操作见 [daily](../log/daily/)。

## 当前停点

- 最近核对的本地 main 为 `10a241870`，同步收尾的前五个提交已完成，最后一个为 `c02b6ec26`；文档收尾随本次文档提交记录。这里不代表远端实时状态。
- dm 已线性 rebase 到该 main，最近核对 `main..dm` 无 merge commit；安全引用 `backup/dm-merge-2100c9052` 保留。后续禁止用 merge 同步。
- 用户已授权六点拆分提交；源码、C 格式及检查配置已分五次提交，文档为第六点。不创建新 PR，不 push。
- **本次文档收尾的检查由用户手动执行，助手按授权更新并提交文档，不启动检查、构建或测试。**

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

- PR 1 历史记录：`mapped-bio-ranges`，base `98e717275`、HEAD `c14d68c93`；曾完成验证和推送。此处不是本次远端复核，历史完整 ktest 与临时工作树记录保留，不作为当前操作模板。详见 [PR 1 档案](../log/PR/PR-1-mapped-bio-ranges.md)。
- PR 2 尚未制备新分支。既定候选范围为 live registry lease、普通 VFS mount/rootfs 与 ext2/exfat holder；排除 token/runtime node、MlsDisk 和分区事务，详见 [PR 规范](pull_request.md)。
- 本次按用户授权分点提交；用户手动检查文档后再决定是否进入 PR 2。后续创建分支或 push 仍需另行授权。

## 阅读顺序

[CLAUDE.md](../CLAUDE.md) / [AGENTS.md](../AGENTS.md)
→ 本交接
→ [项目状态](../log/device-mapper-progress.md)
→ [PR 规范](pull_request.md) / [上游同步](upstream-sync.md)
→ [当日日志](../log/daily/2026-10-9.md)。

QEMU、ktest、C 回归和 NixOS 测试必须串行；遇失败、环境阻塞或未预期 warning 先报告并讨论。
不覆盖来源不明改动，不自动提交或 push。
