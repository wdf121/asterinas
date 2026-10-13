# PR 2:linear I/O path / `Add the aster-device-mapper crate with a linear target`

## 1. 状态与标识

| 项目 | 事实 |
|---|---|
| 状态 | 本地已提交并推送;PR 尚未创建,由用户在网页自行创建。 |
| 实际 Git branch | `dm-linear-io-path` |
| 当前本地 base / HEAD | `67ed6c70f` / `7a497f9d5` |
| 远端事件 | 已推送 `origin/dm-linear-io-path`(新建远端分支,非强制更新);GitHub 返回 PR 创建入口 `https://github.com/wdf121/asterinas/pull/new/dm-linear-io-path`。 |
| 提交主题 | `Add the \`aster-device-mapper\` crate with a linear target` |
| 提交内容 | 6 文件、422 行:根 `Cargo.toml`/`Cargo.lock` workspace 接入 + 新 crate `kernel/core/comps/device-mapper/`(Cargo.toml、lib.rs、device.rs、linear.rs)。 |
| 依赖 PR | #3937(PR 1,mapped BIO ranges,open)。 |

本文档使用稳定编号 **PR 2**;当前分支名为 `dm-linear-io-path`。

## 2. Git 与远端历史

| 日期 | 事件 | base | HEAD | 结论 |
|---|---|---|---|---|
| 2026-10-13 | 基于 PR 1 重放后的 `67ed6c70f` 创建分支;用户手敲生产代码(LinearTarget 校验/元数据/enqueue 与 DmDevice 适配层),本轮补齐全部 ktest 并完成 review 修正 | `67ed6c70f` | `7a497f9d5` | 单提交,无 merge 历史 |
| 2026-10-13 | 推送 `origin/dm-linear-io-path` | 同上 | 同上 | 新建远端分支成功,未遇代理超时 |

review 修正(均在提交前完成):中文学习注释英文化并补公开 API doc comment;`LinearTargetError` 补 `#[derive(Clone, Copy, Debug, Eq, PartialEq)]` 与 doc(对齐 `BioEnqueueError` 先例);容量比较改为无损 `nr_sectors as u64`,`BackingRangeOverflow` 语义收窄为仅表示 start+length 溢出(原 `u64::try_from(usize)` 分支不可达,已移除);`metadata()` 补截断不可达的不变式注释;新增 `accepts_write_bio_ending_at_mapping_end`(同时覆盖 Write 正向重映射与 end==length 边界)。

## 3. 问题、目标与非目标

### 目标

- 引入 `aster-device-mapper` 内核 crate,提供对单个 backing 块设备的固定线性映射。
- 构造期校验映射几何:拒绝零长度、start+length 溢出、越过 backing 容量的映射。
- enqueue 期做逻辑→backing 的 sector 翻译:Flush 直通;数据 BIO 空范围或越过映射长度时拒绝,否则按映射起点偏移后转发。

### 非目标

- 不引入多 target 表、运行时设备注册、VFS 节点生命周期或用户态控制接口(dm-ioctls)。
- 不引入 range BIO 操作与 device lease(仍是 PR 1 的非目标)。

## 4. 技术设计与范围

`LinearTarget` 持有映射几何与 backing 设备。构造时一次性完成校验,enqueue 只做廉价范围检查:`BioType::Flush` 不经数据范围校验直接转发(对齐 Linux DM 语义);Read/Write 要求 `start < end <= length`,通过后经 `offset_mapped_sid_range`(PR 1 引入的 mapped-range 机制)叠加 `backing_start` 再转发。

`DmDevice` 将 target 与设备身份、名字配对,实现 `BlockDevice`,使映射设备可出现在任何接受块设备的位置。target 元数据上报映射长度并透传 backing 的 `max_nr_segments_per_bio`;linear 为 1:1 sector 映射,BIO segment 数不变,无需切分。

- `linear.rs`:映射几何校验、enqueue 翻译、8 个 ktest(GeometryBacking 钉构造边界;RecordingBacking 捕获转发 range,钉 Flush 直通、Read 偏移 10..14→910..914、Write 边界 96..100→996..1000、空范围与越界双拒绝且不触达 backing)。
- `device.rs`:薄适配层(4 个单行转发),无独立 ktest,如实记录为覆盖缺口。
- `lib.rs`:crate 门面,re-export `DmDevice`/`LinearTarget`/`LinearTargetError`。

审阅重点:构造期几何不变式;enqueue 校验读的是 mapped range(单层 DM 下等于 logical,未来多层堆叠需重审);Flush 直通与 Linux DM 语义的一致性。

## 5. 验证证据

| 层次 | 命令 | 结果 | 状态 |
|---|---|---|---|
| 格式 | `cargo fmt --check`(容器内,crate 级) | 通过。 | 通过 |
| crate 专项 ktest | `cd kernel/core/comps/device-mapper && cargo osdk test` | 助手在容器内验证 8 passed、0 failed(doc comment 之前的工作树;doc comment 不改测试);用户在最终 HEAD 复核通过。 | 通过 |
| 静态准入 | `make check` | 用户执行,通过。 | 通过(用户反馈,输出未归档) |
| 内核构建 | `make kernel` | 用户执行,通过。 | 通过(用户反馈,输出未归档) |
| 项目级 ktest | `make ktest` | **未执行**,如实记录;若维护者按 PR 1 标准要求,再补跑。 | 未覆盖 |

环境注记:本轮排障确认 ktest 必须在 `fork_Asterinas` 容器内执行;宿主机(openEuler)缺 qemu、grub 命名不同且 cargo-osdk 曾指向已删除的 `/root/pr-tree`。曾重装宿主机 cargo-osdk 并误判"旧版带 openEuler 适配",已修正结论:容器环境始终正常,宿主机侧失败均为跑错环境所致。

## 6. 已知边界与后续动作

1. PR 由用户在网页创建(base 选上游 `asterinas/asterinas` 的 `main`,compare 选 `wdf121:dm-linear-io-path`);创建后回填 PR 编号。
2. 全套 `make ktest`/`make initramfs` 未跑,属已知验证缺口;是否补跑由维护者反馈或用户决定。
3. `device.rs` 薄适配层无独立测试;若后续 PR 3 的运行时链路覆盖它,可在此回填。

## 7. Upstream maintainer description

### Title

```text
Add the `aster-device-mapper` crate with a linear target
```

### Summary

- Introduce the `aster-device-mapper` kernel crate with fixed linear mappings over a single backing block device.
- Validate mapping geometry at construction: reject zero-length mappings, start-plus-length overflow, and ranges extending past the backing device's capacity.
- Translate logical sector ranges on enqueue: flushes forward unchanged; data BIOs are refused when empty or past the mapping length, and otherwise offset by the mapping start before forwarding.

### Design

`LinearTarget` owns the mapping geometry and the backing device. Construction validates the mapping up front, so enqueue only performs cheap range checks: flushes bypass data-range validation and forward directly to the backing device, while reads and writes are refused when empty or past the mapping length and otherwise remapped through the mapped-range mechanism introduced in #3937 before being forwarded.

`DmDevice` pairs a target with a device identity and name, implementing `BlockDevice` so a mapped device can be used wherever a block device is expected. Target metadata reports the mapping length and passes through the backing device's `max_nr_segments_per_bio`; linear targets map sectors one-to-one, so BIO segment counts never change and no splitting is needed.

### Scope

Included:

- `kernel/core/comps/device-mapper/` (new crate: linear target, block-device adapter, ktests)
- workspace registration in the root `Cargo.toml` and `Cargo.lock`

Excluded: multi-target tables, device registration and node lifecycle, the userspace control interface, and range BIO operations.

### Testing

Validated on final PR HEAD `7a497f9d5`:

```text
make check
make kernel
cd kernel/core/comps/device-mapper && cargo osdk test
```

`make check` and `make kernel` pass. The crate ktest completed with 8 passed and 0 failed. The full project ktest suite was not re-run for this PR; block-layer coverage comes from #3937, and this PR adds the device-mapper crate's own kernel-mode unit tests.

### Review notes

Please focus review on the construction-time geometry invariants and the enqueue translation (flush passthrough, empty/past-end refusal, offset by the mapping start). Please let me know if any part of the design or scope needs clarification. I'm happy to discuss it and will respond promptly.
