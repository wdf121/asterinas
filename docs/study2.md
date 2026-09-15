# Device Mapper 学习记录

> 本文记录每日已完成、已验证的 Device Mapper 学习结论。每次学习按“业务逻辑 → 代码主逻辑 → 资源、并发与边界”推进；运行证据默认按 `[dm-debug]` 日志 → `strace` → 按需远程 GDB 升级。

## 2026-09-15：`dmsetup targets`、tableless mapper 创建与 zero table 加载

### 1. `dmsetup targets`：静态 target 能力的可变长 ABI 输出

#### 业务逻辑

`dmsetup targets` 查询当前内核支持的 target 类型和版本，而不是某个 mapper 的 table。实际 CLI 输出为：

```text
error   v1.6.0
linear  v1.4.0
striped v1.6.0
zero    v1.1.0
```

#### 运行证据

- `[dm-debug]` 与 `strace` 均显示主请求顺序为 `DM_VERSION → DM_LIST_VERSIONS`；前者是 libdevmapper 的前置版本检查，后者才是 target 列表查询。
- `/dev/mapper/control` 使用 FD 3；`TCGETS` 发生在 stdout 的 FD 1 上，是终端输出环境查询，不是 Device Mapper ioctl。
- `DM_LIST_VERSIONS` 返回后，libdevmapper 将内核回写的 record 格式化，再逐行 `write(1)` 输出 target 名称和 `v<major>.<minor>.<patch>`。

#### 代码主逻辑

1. 静态 target metadata 被复制为 target-version snapshot；控制面只输出 `name + version[3]`，不执行 target 数据面逻辑。
2. 每条 record 的 ABI 布局为：`next: u32`、`version: [u32; 3]`、NUL 结尾的 target name；record 总长度按 8 字节对齐。
3. 内核写入的是三个二进制版本整数，`v` 与 `.` 是 libdevmapper 的 userspace 展示格式，不存在于内核 record 中。
4. 当前 record 初写 `next=0`；下一条起点确定后才回填上一条的 `next`。最后一条保持 `next=0`。
5. 内核将完成后的本地 buffer copy-back 给 userspace，由 libdevmapper 遍历 record 链。

关键实现：

- 静态 target snapshot：[QueryWorkflow::target_versions()](kernel/core/src/device/misc/device_mapper/control.rs#L207-L215)
- record 长度与 8 字节对齐：[target_version_record_len()](kernel/core/src/device/misc/device_mapper.rs#L748-L750)
- record 字段写入：[write_target_version()](kernel/core/src/device/misc/device_mapper.rs#L752-L766)
- `next` 回填、最终 record 日志与容量处理：[list_versions()](kernel/core/src/device/misc/device_mapper.rs#L789-L827)
- ioctl 本地 buffer 复制与 userspace 回写：[DmControlFile::ioctl()](kernel/core/src/device/misc/device_mapper.rs#L215-L286)

#### 资源、并发与边界

- target catalog 是静态只读 metadata；每次 ioctl 在独立本地 buffer 中编码，多个进程的 `targets` 请求不会互相写入同一输出 buffer。
- 此命令不创建 mapper、table、primary node 或 alias，也不需要 mapper lifecycle lock。
- `data_size` 是 userspace 在 `dm_ioctl` header 中提供的本次 buffer 总容量；普通 `dmsetup` 本次使用 16384 字节，`data_start=312`。
- 可变长区装不下下一条完整 record 时，内核不写半条：保留已编码 record、设置 `DM_BUFFER_FULL`，然后停止编码。当前专属日志输出已编码条数与 `buffer_full`。
- GDB 仅在需要独立核验小端原始字节、NUL、padding 或日志与实际 buffer 是否一致时使用，不是本命令闭环的前置条件。

### 2. tableless mapper：`DM_DEV_CREATE --notable`

#### 业务逻辑与观察

执行：

```bash
dmsetup create wdf --notable
```

实际结果：

```text
DM_VERSION → DM_DEV_CREATE
name=wdf
id=DeviceId(264241152)
readonly=false
requested_minor=None
```

`dmsetup info` 显示：

```text
State: ACTIVE
Tables present: None
Open count: 0
Event number: 0
Major, minor: 252, 0
Number of targets: 0
/dev/mapper/wdf: open failed: No such file or directory
```

这里 `State: ACTIVE` 只表示没有设置 suspend flag；它不表示 active table 已存在。`Tables present: None` 才说明 active 与 inactive table slot 都为空。

#### 代码主逻辑

```text
DM_DEV_CREATE
→ DmManager 分配 minor 与 DeviceId
→ 创建 Arc<DmDevice>
→ 写入 manager 的 name → Arc<DmDevice> 索引
→ 回写 dm_ioctl header
```

- 创建与 name/UUID 索引写入：[DmManager::create_with_readonly()](kernel/core/comps/device-mapper/src/manager.rs#L135-L178)
- name 查询：[DmManager::lookup_name()](kernel/core/comps/device-mapper/src/manager.rs#L181-L184)
- `dmsetup info` 的设备枚举：[DmManager::devices()](kernel/core/comps/device-mapper/src/manager.rs#L247-L250) 与 [list_devices()](kernel/core/src/device/misc/device_mapper.rs#L846-L851)
- `DM_DEV_STATUS` header 编码：[fill_device_header_snapshot()](kernel/core/src/device/misc/device_mapper.rs#L1019-L1050)

`DeviceId(264241152)` 是 Asterinas 内部 raw 值：

```text
252 << 20 | 0 = 264241152
```

内部布局是高 12 bit 为 major、低 20 bit 为 minor；写回 Linux `dev_t` ABI 时会再转换为 encoded `u64`。

- 内部 `DeviceId` 构造与 major/minor 提取：[DeviceId](kernel/libs/device-id/src/lib.rs#L21-L45)
- Linux `dev_t` ABI 编码转换：[DeviceId::as_encoded_u64()](kernel/libs/device-id/src/lib.rs#L62-L76)

最小结论：

```text
create --notable 成功
= manager 可发现 mapper identity
≠ 已发布 primary block node
≠ 已安装 table
≠ 已发布 mapper alias
≠ 已具备 mapping I/O 能力
```

### 3. 加载 zero table：`DM_TABLE_LOAD`

#### 业务逻辑与观察

执行：

```bash
dmsetup load wdf --table '0 2048 zero'
```

本次 header 显示 `target_count=1`，意思是本次 table 输入含一条 target spec；它不是系统支持 target 类型的数量。

```text
0 2048 zero
→ logical start = 0
→ length = 2048 sectors
→ target type = zero
→ 无额外参数
```

实际日志确认：

```text
state table-loaded ... slot=inactive targets=1 sectors=2048
control table-load committed ... targets=1 readonly=false primary_registered=true
```

`dmsetup info` 的 table 状态由 `None` 变为 `INACTIVE`，符合 load 只准备待切换 table 的预期。primary `/dev/dm-N` 在 load 时注册；`/dev/mapper/<name>` alias 仍要等首次 `resume` 才发布。

`DM_TABLE_LOAD` 的 `strace` 只清晰展示固定 header：输入含 `name` 和 `target_count=1`，成功回写包含 `DM_INACTIVE_PRESENT_FLAG`。target spec 所在的可变长输入区被 strace 的 DM decoder 折叠为 `...`，不能据此看到 `0 2048 zero` 的字段。

#### 代码主逻辑

```text
按 name 找到当前 Arc<DmDevice>
→ 获取该 mapper 的 lifecycle guard，并重验它仍在 manager 中
→ 解析 DM_TABLE_LOAD 的可变长 target spec
→ target catalog 分派并构造 ZeroTarget
→ 验证整张 DmTable 的连续性与 backing 约束
→ 得到 Arc<DmTable>
→ 先注册 /dev/dm-N primary
→ primary 成功后把 table 安装进 inactive slot
→ 回写更新后的 ioctl header
```

- lifecycle 入口与 control 摘要：[table_load()](kernel/core/src/device/misc/device_mapper.rs#L540-L556)
- ABI spec 解析、target 构造、table 构造：[parse_table_load_request()](kernel/core/src/device/misc/device_mapper.rs#L580-L624)
- target catalog 分派：[parse_target_with()](kernel/core/comps/device-mapper/src/target/mod.rs#L290-L310)
- table 必须从 sector 0 开始且 target 连续：[DmTable::new_targets()](kernel/core/comps/device-mapper/src/table.rs#L49-L77)
- primary 先行、inactive slot 后提交：[ControlWorkflow::load_table_with_primary()](kernel/core/src/device/misc/device_mapper/control.rs#L267-L285)
- inactive slot 安装：[DmDevice::load_table()](kernel/core/comps/device-mapper/src/device.rs#L276-L288)

术语：

```text
spec
→ specification；用户给出的映射规则声明，如 `0 2048 zero`。

target object
→ 内核解析 spec 后构造的具体对象，如 ZeroTarget。

table slot
→ active/inactive 两个固定 table 容器位置。

primary
→ 按 major:minor 识别的底层正式 block-device node，如 /dev/dm-0。

alias
→ 按 mapper 名称访问的友好入口，如 /dev/mapper/wdf。
```

#### 资源、并发与失败边界

- 同一 mapper 的 load/resume/suspend/clear/rename/remove 通过每台 `DmDevice` 的 lifecycle mutex 串行；不同 mapper 不会被该锁互相阻塞。
- 查到 `Arc<DmDevice>` 后、等待 lifecycle lock 时，另一个线程可能已经 remove 并重建同名 mapper。取得锁后通过 `manager.is_current(device)` 重验，拒绝修改 stale Arc。
- lifecycle guard 与重验：[with_current_device_lifecycle_in_with_policy()](kernel/core/src/device/misc/device_mapper.rs#L998-L1012)
- `DmDevice` 的 lifecycle mutex：[DmDevice::lock_lifecycle()](kernel/core/comps/device-mapper/src/device.rs#L150-L156)
- 当前实例的 `Arc` 身份检查：[DmManager::is_current()](kernel/core/comps/device-mapper/src/manager.rs#L206-L217)

资源提交顺序是：先完成可能失败的 external runtime primary 发布，再提交不可失败的内部 readonly/inactive 状态。

```text
spec / target / table 验证失败
→ 未发布 primary，未改 DmDevice state。

primary 注册失败
→ inactive slot 保持原样，可安全重试 load。

primary 注册成功
→ 才写入 inactive table。
```

primary 注册会从 `DmDevice` 的 `BlockDevice::id()` 取得 `DeviceId`，以其 minor 生成 `/dev/dm-N`：

- `DmDevice` 的 `BlockDevice` 实现：[BlockDevice for DmDevice](kernel/core/comps/device-mapper/src/device.rs#L490-L529)
- `/dev/dm-N` primary 注册：[register_mapper_primary()](kernel/core/src/device/registry/block.rs#L510-L518)

primary 先出现、inactive table 尚未安装的短暂窗口是允许且安全的：新节点可以 open，但无 active table 时 metadata 为零容量，I/O 不能派发到 mapping table。

- 无 active table 时的 metadata：[DmDevice::metadata()](kernel/core/comps/device-mapper/src/device.rs#L516-L520)
- I/O 仅在 running 且 active table 存在时下发：[DmDevice::enqueue()](kernel/core/comps/device-mapper/src/device.rs#L498-L513)
- runtime primary 的明确约束：[register_mapper_primary() 注释](kernel/core/src/device/registry/block.rs#L510-L514)

### 4. 本日结论

- `targets` 已完成业务、ABI 主链、私有 buffer 并发隔离和 `DM_BUFFER_FULL` 边界的学习闭环；GDB 保留为原始字节独立验证工具。
- zero mapper 已完成 `create --notable` 与 `load` 的业务和代码主链；下一步从首次 `resume` 开始，观察 inactive → active、alias 发布与 event 变化。
- `remove` 必须在资源对象设计阶段同步考虑：它涉及 manager 索引、runtime registry/node、table、BIO、waiter 与旧 Arc 的撤销顺序、失败回滚或隔离，不能简化为删除一条 name 索引。