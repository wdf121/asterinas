# Device Mapper 第二版优化文档

本文件区分**生产代码优化**与**验证补强**。详细过程与完整命令输出见 `log/2026-9-10.md`、`log/2026-9-11.md`。

## 1. 已完成的生产代码优化

| 优化项 | 优化前 | 优化后 | 用户可见结果 | 验证 |
|---|---|---|---|---|
| P1.1 首次 load 生命周期 | primary 与 alias 都延后到 resume。 | load 注册 primary、`Live` registry、open gate 与 `/dev/dm-N`；首次 resume 用 guard 激活 table、发布 alias、失败回滚。 | load 后 primary 为 0 容量/EOF；resume 后 alias 与 active table 可用。 | core guard rollback/commit；ioctl 生命周期；`CHECK_PASS_DMSETUP_FIRST_LOAD_RESUME_LIFECYCLE`。 |
| P1.2 suspend / resume 与 noflush | suspend 拒绝后续 BIO；noflush 错接到 resume。 | FIFO 暂存 BIO；普通 suspend drain，noflush 不等；resume 统一换表/replay；replay 或删除失败完成 `IoError`。 | 新 I/O 不误走旧表、不被直接拒绝；旧 I/O 保持旧表语义；调用方不会永久等待。 | core suspend/replay/failure ktest；控制面 noflush lifecycle。 |
| P1.3 首次 resume 失败原子性 | guard 在 alias 发布前安装 active table 并转为 Running；primary 的并发 BIO 可能在 alias 失败后仍下发到 backing。 | guard 持有 device state lock，alias 成功后才原子安装 active table、转为 Running 并预留 postponed BIO replay。 | alias 发布失败保持 inactive-only 且不产生 backing I/O；成功提交后 BIO 才能按 active table 下发。 | 5 项 DM core guard ktest、1 项 ioctl publisher-failure ktest、`--control-plane`。 |
| P1.4 drain 期间的控制面并发 | 除 `DM_DEV_WAIT` 外的全部 ioctl 持有 `DM_CONTROL_LOCK`；任一 mapper 的 suspend 或 running resume drain 会阻塞其他 mapper 的查询与生命周期命令。 | 生命周期写操作改用 per-device lifecycle guard；取得 guard 后按 manager `Arc` 身份复核，查询和 create 不持全局串行锁。 | mapper-A drain 时 mapper-B 的 status/create 可完成；A 的 load/clear/remove/rename/resume 仍不能越过 drain；stale `Arc` 被拒绝。 | 3 项 lifecycle ktest、wait/P1.3 回归、`--control-plane`。 |
| P2.1 runtime rename 资源事务 | 先更新 manager name/UUID index 与 `DmDevice.name`，再移动 alias；alias 失败时反向 rename 回滚，旧名可能被并发 create 抢占并使回滚失败。 | 保持旧 name 索引并预留新 name；不持 manager lock 移动 alias，成功后一次提交 index/name，失败仅释放预留。 | alias 移动期间旧名和新名均不能复用；失败后旧 alias/index/name 保持一致，新名可再次创建；无关名称 create 不被 VFS 操作阻塞。 | DM core 81 项 ktest、ioctl 注入失败 ktest、`--control-plane`。 |
| P2.2 首次 primary 发布失败原子性 | 先提交 block registry 为 Live、开放 open gate，再创建 `/dev/dm-N`；node 创建失败后撤销可能被 open/lease 阻塞，留下无 node 的 Live registry/wrapper。 | 保持 pending registration/wrapper 拒绝 open，先创建 primary node，成功后才提交 Live、记录 node 并开放 open gate。 | primary 创建失败时不留 registry、wrapper、open 或 inactive table；同一 `DeviceId` 可再次注册；成功的 load/resume 可见语义不变。 | block registry 与 ioctl 两项注入 ktest、`--control-plane`。 |
| P2.3 remove 回滚隔离与重试 | primary 删除失败后 alias 恢复、或 unregister commit 失败后的 node/alias 恢复只写 warning；token drop 会重新暴露 Live registry/open gate，即使 runtime node 已缺失。 | 无法完整恢复时，保留 unregistration token 和 `Removing` 状态；registry lookup/lease 隐藏、wrapper 拒绝 open，后续 remove 取回 token 继续注销。 | 控制面不再把缺失 alias/primary 的 mapper 宣布为可用；操作返回 `EIO`，用户可重试 remove 完成清理。 | 双失败注入 ktest、隔离重试 ktest、`--control-plane`。 |

生产代码提交：`83b6187b3`（load 生命周期）、`1815c7578`（running resume barrier）、`37f7bba2b`（suspend/no-flush 语义）、`c1848d8a9`（P1.3/P1.4 生命周期并发边界）。P2.1、P2.2 与 P2.3 已验证，待提交。

## 2. 已完成的验证补强（不占后续生产优化优先级）

| 项目 | 补强内容 | 结果 |
|---|---|---|
| V1 `wait` 事件 | rename / `--setuuid` 后验证旧 event 返回、当前 event timeout、header 回写。 | core 与 control-plane 均通过。 |
| V2 未支持命令边界 | `message` / `setgeometry` / arm-poll 的 `ENOTTY`，deferred remove 的 `EOPNOTSUPP` 与 mapper 保留。 | core、Linux baseline、control-plane 均通过。 |
| V3 `info` / `ls` discovery 字段 | columns 验证 UUID、major:minor、open、segments、event、table state、selector 与列表成员。 | Linux baseline 与 `CHECK_PASS_DMSETUP_DISCOVERY_FIELDS` 通过。 |

## 3. 后续优先级：生产代码优化优先

具体优化编号按已完成的主题连续演进：P1.1–P1.4 属于 mapper 生命周期与控制面并发阶段，均已完成；P2.1 已收敛 runtime rename 的名称与 alias 事务，P2.2 已收敛首次 primary 发布，P2.3 已收敛 remove 回滚的隔离边界，后续同一资源事务主题从 P2.4 开始；数据面可扩展性收敛时从 P3.1 开始。P0–P4 仅表示候选类别/优先级，不替代具体实现编号。

| 优先级 | 类型 | 优化方向 | 预期收益 / 边界 |
|---|---|---|---|
| P0 | 生产结构审计（已完成） | 已盘点 DM 生产代码中的硬编码、重复生命周期分支、线性遍历、锁范围和状态所有权；首个落地项为 P1.3 首次 resume 失败原子性。 | 后续只将能改善生产结构、扩展性、健壮性或实际并发行为的候选进入实现；测试缺口不单列为优化。 |
| P1 | mapper 生命周期与控制面并发（已完成） | P1.1–P1.4 已依次收敛首次 load、suspend/resume、失败原子性和 drain 并发边界。 | 后续不再为此类别新增 P1.x；跨越到 runtime 资源事务后使用 P2.1。 |
| P2 | runtime 生命周期收敛 | P2.1 已收敛运行中 rename 的 name index、UUID index、`DmDevice.name` 与 `/dev/mapper/<name>` alias 的失败边界；P2.2 已收敛首次 `/dev/dm-N` primary 的 pending/Live/open 发布顺序；P2.3 已让 remove 补偿失败进入可重试的隔离状态；P2.4 再审计 manager index 与 runtime 注销的最后提交边界。 | 避免 alias rename、primary node 创建或 remove 补偿失败后遗留可用但半发布的资源；后续继续减少跨 registry 的模糊状态。 |
| P3 | data-driven 扩展框架 | 审计 ioctl command、target metadata 与用户可见能力的硬编码表。 | 在不扩大当前支持集合的前提下，减少新增 target 或 control 能力时需要同步修改的固定分支。 |
| P4 | 验证补强 | `mangle`、persistent minor、inactive deps、`wipe_table`、`mknodes` / `ls --tree`。 | 仅在对应生产代码优化完成后，按命令风险补最小验证；不抢占生产优化。 |

`git diff --check` 通过；`cargo fmt --check` 仍仅报告 `kernel/core/comps/device-mapper/src/table.rs` 的三处既有差异。
