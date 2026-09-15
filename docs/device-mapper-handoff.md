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
| 4 | [todo/strace.md](../todo/strace.md) | 进入用户主导的 `strace` 学习主线，查看已完成观察与后续练习矩阵。 |
| 5 | [log/device-mapper-optimization-v2.md](../log/device-mapper-optimization-v2.md) | 获取生产代码优化与验证补强的优先级区分。 |
| 6 | [2026-9-11.md](../log/daily/2026-9-11.md) 和 [2026-9-10.md](../log/daily/2026-9-10.md) | 获取 P2 资源事务和前序阶段的实际改动、验证记录。 |
| 7 | [device-mapper-technical-maintenance.md](device-mapper-technical-maintenance.md) | 审查 DM 架构、生命周期与控制面事实。 |
| 8 | [test.md](test.md) 和 [production-code-validation-chain.md](production-code-validation-chain.md) | 选择定向 ktest 与系统验收。 |

如果文档与当前代码、实际命令结果或 Git 工作区不一致，以当前代码和命令为准，并修正过期记录。

## 当前工作模式

当前以**用户主导的调试能力训练与控制面源码走读**为主，生产优化暂停。协作方式是：用户亲自运行命令、观察输出并先给出判断；Claude 先解释命令与预期观察点，再纠正推理链，不代替用户执行练习或提前给出答案。

当前两条主线：

1. **strace 与相关调试工具**：以 [todo/strace.md](../todo/strace.md) 为学习记录。已完成 `dmsetup version` 与 `dmsetup targets` 的 guest `strace` 观察，包括 control FD、`DM_VERSION`、`DM_LIST_VERSIONS`、用户态调用栈和终端 ioctl 分类；后续按文档的查询、生命周期、失败、等待与数据面矩阵继续。
2. **控制面按命令走读**：公共路径已走完：`dmsetup` / libdevmapper → `/dev/mapper/control` → `DmControlFile::ioctl` → ioctl 命令解码与分派。后续以一个具体用户命令为单位，先从 CLI 可见语义和 `strace` 中的 ioctl 出发，再进入对应 handler、状态机和资源路径；不要跳回只按源码文件顺序阅读。

生产代码优化仅在用户明确说“恢复优化”后继续。P2.4 保持候选状态，未实现、未验证、不得写入完成记录。

## 当前停点：P2.1–P2.3 已提交归档；调试学习继续

P2 runtime 资源事务已完成并已提交归档：

- `3bf690ec1`：P2.1 runtime rename name reservation。
- `fe7503aea`：P2.2 primary pending-to-Live 发布原子性。
- `4f528a8c7`：P2.3 remove 失败隔离与重试。
- `20c7927a1`：归档 P2 资源事务验证。

P2.4 的“隔离 mapper 控制面可见性”只停留在候选/设计层面；不要把此前未完成的实验性实现或计划当作当前代码事实。

若恢复生产优化，先重新阅读当前代码和 `git status`，然后按“原行为 → 目标行为 → 最小验证”的节奏取得用户确认。

## 工作区接手检查

新窗口在执行写入、构建、测试或提交前，先执行：

```bash
git status --short
git diff --check
```

不要假定工作区干净，不要覆盖、删除或提交来源不明的现有改动。构建与测试默认在 `myAsterinas` 容器内进行；QEMU/ktest/NixOS system test 必须串行。
