# PR 2：`block: add tracked device leases` 最小 hunk 审阅

> 审阅日期：2026-09-29
> 基线：`main`（`98e717275`）→ 权威 `dm`
> 范围：通用 block-device lease 与非 DM 消费者；不包含 DM crate、DM ioctl、runtime node、range BIO 或本地系统测试 harness。

## 结论

当前 `dm` 中的 lease 相关改动**不能直接作为 PR 2 导出**。可复用的核心是“注册表持有计数 + `BlockDeviceLease` + VFS/filesystem 持有者迁移”；但当前实现还存在按名称查找重入、rootfs 绕过 tracked lease、MlsDisk facade 生命周期和分区刷新事务四项问题。直接按文件导出会把已知缺陷、PR 4 的 runtime 事务或未验证语义混入 PR 2。

## 可作为 PR 2 核心的最小 hunks

| 文件 | 需要保留的最小语义 | 不应带入的内容 |
|---|---|---|
| `kernel/core/comps/block/src/lib.rs` | 注册条目持有 `lease_count`；`BlockDeviceLease` 的 clone/drop 计数；`lookup_lease`；持有 lease 时 `unregister` 返回 `Busy`；按名称取得 lease 的无重入实现；直接 ktest。 | Pending/Removing token、commit/abort、`is_removing`、`list` 与 runtime 删除回滚，均属于 PR 4。 |
| `kernel/core/src/fs/vfs/fs_apis/registry.rs` | `FsCreationCtx` 保存 `BlockDeviceLease`；路径挂载使用 `lookup_lease`；预解析设备路径也接收 tracked lease。 | “任意 device file 都可作为 mount source”的类型语义变更；应单列为后续 mount-source 行为项。 |
| `kernel/core/src/fs/rootfs.rs` | `/dev/<name>` root lookup 改为按名称取得 tracked lease，并将其移交 `FsCreationCtx`。 | 与 root 参数解析、候选文件系统选择无关的改动。 |
| `kernel/core/src/fs/fs_impls/ext2/fs.rs`、`fs_type.rs`、`test_utils.rs` | `Ext2` 保存 lease 至卸载；`open` 参数和 fixture 的 untracked wrapper 适配。 | 无关 ext2 功能或 mount option 改动。 |
| `kernel/core/src/fs/fs_impls/exfat/fs.rs` | `ExfatFs` 保存 lease 至卸载，并适配 cache key 与底层设备访问。 | exfat 独立功能改动。 |

### 最小 API 约束

1. `register` 必须在取得 registry 锁之前调用 `device.id()`；不能在 registry 锁域执行 `BlockDevice` trait 回调。
2. `lookup_lease` 必须在 registry 锁与同一注册项的计数锁保护下增加计数；`unregister` 在同一锁序下检查计数。最小 PR 不需要 Pending/Removing 状态机。
3. 按名称查找应先在 registry 锁内快照 `Arc<RegisteredBlockDevice>`，释放所有 registry/entry-state 锁后调用 `device.name()`，命中后再取得该项的 tracked lease。否则 `name()` 重入 `lookup` 时会自锁。
4. `FsCreationCtx` 在创建失败、cache 命中或候选 FS 失败后析构时应归还 lease；成功创建的 ext2/exfat 实例持有 lease 至实例销毁。

## 阻断直接导出的发现

### 1. rootfs 绕过 tracked lease

[rootfs.rs:91-108](../kernel/core/src/fs/rootfs.rs#L91-L108) 使用 `lookup_by_name()` 取得裸 `Arc<dyn BlockDevice>`，随后 [registry.rs:133-146](../kernel/core/src/fs/vfs/fs_apis/registry.rs#L133-L146) 将其包装为 `new_untracked`。这使 rootfs 挂载不增加 `lease_count`，与普通 VFS mount 的保护语义不一致。

**处理：**PR 2 必须新增安全的按名称 lease lookup，并把 `FsCreationCtx::from_block_device` 改为接收 lease；不能仅迁移普通路径 mount。

### 2. 当前 `lookup_by_name` 仍可能重入死锁

[block/lib.rs:461-468](../kernel/core/comps/block/src/lib.rs#L461-L468) 在 registry 和 entry-state 锁域内调用 `registered.device.name()`。`BlockDevice::name()` 是公开 trait 方法，不能假定其不会重入 registry。

**处理：**此修复是 PR 2 lease API 的组成部分，而不是可延后的局部优化；必须用可重入 `name()` 的 block test double 建立直接 ktest。

### 3. MlsDisk 不能按当前 hunk 纳入

[MlsDisk `Drop`](../kernel/core/comps/mlsdisk/src/layers/5-disk/mlsdisk.rs#L628-L632) 仅设置 `is_dropped`；`RawDisk` 对 backing lease 的所有权仍可能被 `DiskInner`、缓存或异步对象保留。已有探索性证据表明 facade drop 后 backing unregister 仍为 `Busy`。

**处理：**推荐将 `kernel/core/comps/mlsdisk/src/lib.rs` 从 PR 2 最小 patch 排除，待明确“最后一个实际 I/O 使用者”的 release 边界后另行设计、测试和提交。若坚持将它纳入 PR 2，必须同时实现并验证 facade/inner 的释放语义；不能只迁移字段类型。

### 4. 分区刷新不属于 PR 2 最小 hunk

[partition.rs:442-480](../kernel/core/comps/block/src/partition.rs#L442-L480) 在持有 partition 锁时忽略旧分区 `unregister` 与新分区 `register` 的失败。旧分区被 lease 持有时，刷新会失去旧列表并可能静默注册失败。

**处理：**该路径需要可回滚的分区替换事务，依赖 Pending/Removing 或等价 registry 事务，应归入 PR 4；PR 2 不应仅把 `let _ =` 改成错误返回而留下半替换状态。

## 推荐的 PR 2 边界

**建议标题不变：**`block: add tracked device leases`。

包含：block lease 计数与无重入查找、普通 VFS mount 和 rootfs、ext2/exfat 持有者、直接 block/FS lifecycle 测试。

排除：MlsDisk、partition refresh、runtime node/register token、DM table/ioctl、range BIO、NixOS/DM harness。

这与 PR 1 一样形成单一可审阅不变量：**任何由 registry 解析并仍可能发起 I/O 的 block consumer 都持有 tracked lease；最后一个 holder 释放前设备不可注销。**

## 最小验证要求

1. block ktest：lease clone 阻止注销、最后 holder drop 后可注销；reentrant `name()` 通过 `lookup_by_name`/按名称 lease lookup 不死锁。
2. VFS 或 core ktest：路径 mount 获取 lease，创建失败与 cache 命中不泄漏 lease。
3. rootfs 定向测试：`/dev/<name>` root 路径通过 tracked lease 构造 `FsCreationCtx`。
4. ext2/exfat：现有实例/fixture 编译并持有 lease；至少一条真实 registry backing 的 mount→`Busy`→释放→unregister 断言。

## 本地产物状态

- 已完成：基于 `main...dm` 的文件级和所有权审阅；现有候选 hunks 的 `git diff --check` 通过。
- 后续实现：已在权威 `dm` 补齐无重入的 `lookup_lease_by_name`、rootfs tracked lease 传递和预解析 `FsCreationCtx` 的 tracked-only 接口，并完成 block、VFS、rootfs 与 ext2 backing-lease 定向 ktest。MlsDisk、partition refresh、runtime token 与 DM 仍未纳入。
- 未生成 patch：自动安全策略拒绝向临时路径写入 patch；更重要的是当前 `dm` 仍含 PR 4 的 Pending/Removing token API，不能按文件范围直接导出。必须从 `main` 单独提取不含事务层的核心 lease diff。
- 已创建权威 `dm` core lease checkpoint `98eac8346`；尚未创建 PR 分支、提交 PR 或 push。
