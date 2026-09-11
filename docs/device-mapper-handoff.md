# Device Mapper 对话交接

> 最后更新：2026-09-11
> 本文只记录新对话继续工作所需的阅读入口、当前停点和协作节奏；代码、测试和项目事实以当前工作区与链接文档为准。

## 维护规则

只有用户明确说“更新交接文档”时，才允许修改本文。更新时只记录新的当前停点、下一步和必要阅读入口，不复制权威文档已有的实现细节或测试输出。

## 新窗口首先阅读

| 顺序 | 文档 | 用途 |
|---|---|---|
| 1 | [CLAUDE.md](../CLAUDE.md) | 获取简体中文、写入确认、阶段边界和提交规则。 |
| 2 | [AGENTS.md](../AGENTS.md) | 获取容器路径、定向 ktest、系统测试串行和资源检查要求。 |
| 3 | [log/device-mapper-progress.md](../log/device-mapper-progress.md) | 获取当前实现范围、功能边界与 canonical suite。 |
| 4 | [log/device-mapper-optimization-v2.md](../log/device-mapper-optimization-v2.md) | 获取生产代码优化与验证补强的优先级区分。 |
| 5 | [log/2026-9-10.md](../log/2026-9-10.md) | 获取本阶段实际改动、验证和 Linux 基线记录。 |
| 6 | [device-mapper-technical-maintenance.md](device-mapper-technical-maintenance.md) | 审查 DM 架构、生命周期与控制面事实。 |
| 7 | [test.md](test.md) 和 [production-code-validation-chain.md](production-code-validation-chain.md) | 选择定向 ktest 与系统验收。 |

如果文档与当前代码、实际命令结果或 Git 工作区不一致，以当前代码和命令为准，并修正过期记录。

## 当前工作模式

当前进入 **生产代码优化** 阶段。优先级必须是：

1. 生产代码的结构、扩展性、健壮性、状态/资源所有权、锁范围和真实数据面成本；
2. 跟随生产改动的定向 ktest 与系统验收；
3. 命令语义、脚本和文档的验证补强。

已完成的 `wait`、未支持命令、`info` / `ls` 工作属于验证补强，保留但不再抢占生产优化优先级。不要将只改测试、脚本或文档的工作报告为主要“优化”。

## 当前停点：P2.1–P2.3 已验证，等待分点提交

本阶段已完成三项 runtime 资源事务优化；生产代码、定向 ktest 与控制面系统验收均已通过，但尚未创建提交：

- **P2.1 runtime rename 资源事务**：旧 name 保持有效、新 name reservation 防止并发抢占；alias 成功后才提交 manager name/UUID index 与 `DmDevice.name`，失败只释放 reservation。
- **P2.2 首次 primary 发布失败原子性**：pending registry/wrapper 拒绝 open，先创建 `/dev/dm-N`，成功后才转 Live、记录 node 并开放 open gate；失败不留下 registry、wrapper 或 inactive table。
- **P2.3 remove 回滚隔离与重试**：补偿失败时保留 `Removing` token，registry lookup/lease 隐藏、wrapper 拒绝 open；控制面返回 `EIO`，后续 remove 可继续完成注销。

验证详情见 [log/2026-9-11.md](../log/2026-9-11.md)：P2.1 的 DM crate 全量与 ioctl failure-injection ktest、P2.2/P2.3 的 block registry/ioctl failure-injection ktest，以及三轮 `--control-plane` 都通过。

后续候选（尚未选择或实现）：

- `DmManager::lookup_id` 的线性扫描与稳定 ID 索引；
- table target 查找、flush/deps 去重与 target catalog 的数据结构优化；
- P2.4 中 manager index 与 runtime 注销最后提交边界的进一步审计。

不要将候选合并成一个大重构；每次只选择一个生产问题，先说明优化前/后和最小验收，再经用户确认实施。

## 本阶段已完成的可追溯工作

- 已提交：`83b6187b3`（首次 load 的 primary/alias 生命周期）、`1815c7578`（running resume 换表屏障）、`37f7bba2b`（suspend/no-flush 语义对齐）、`c1848d8a9`（P1.3/P1.4 生命周期并发边界）。
- 未提交：P2.1、P2.2、P2.3 的生产代码、ktest 日志和同步文档；提交时按逻辑点精确暂存，禁止 broad stage。
- 最新控制面系统测试已通过；详见 [log/2026-9-11.md](../log/2026-9-11.md)。

## 工作区接手检查

新窗口在执行写入、构建、测试或提交前，先执行：

```bash
git status --short
git diff --check
```

不要假定工作区干净，不要覆盖、删除或提交来源不明的现有改动。构建与测试默认在 `myAsterinas` 容器内进行；QEMU/ktest/NixOS system test 必须串行。
