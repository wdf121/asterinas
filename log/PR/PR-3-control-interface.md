# PR 3:control device skeleton / `Add the Device Mapper control device with \`DM_VERSION\` support`

## 1. 状态与标识

| 项目 | 事实 |
|---|---|
| 状态 | 本地已提交,gate 全绿;未推送,PR 尚未创建,由用户在网页自行创建。 |
| 实际 Git branch | `dm-control-interface` |
| 当前本地 base / HEAD | `d0088c2c9`(PR 2)/ `d539b725b` |
| 远端事件 | 未推送。推送时建议直接使用分支名 `dm-control-interface`;PR 的 base 取向(stacked 指向 PR 2 分支,或待 PR 2 合并后重放)待用户创建时确定。 |
| 提交主题 | `Add the Device Mapper control device with \`DM_VERSION\` support` |
| 提交内容 | 2 文件、517 行:新增 `kernel/core/src/device/misc/device_mapper.rs`(控制设备 + `DM_VERSION` 握手 + flags 校验 + 19 个 ktest)+ `misc/mod.rs` 接入(3 行)。 |
| 依赖 PR | PR 2(`dm-linear-io-path`,已推送,PR 尚未创建)。 |

本文档使用稳定编号 **PR 3**;当前分支名为 `dm-control-interface`。

## 2. Git 与本地历史

| 日期 | 事件 | base | HEAD | 结论 |
|---|---|---|---|---|
| 2026-10-14 | 基于 PR 2 的 `d0088c2c9` 创建分支;学习式手敲 Stage 1(点 11-12 用户手敲,点 13 与 17 个 ktest 由助手代写),clippy `needless_range_loop` 修正后 amend | `d0088c2c9` | `3977e321e` | 单提交,未推送 |
| 2026-10-14 | 六条外部 review 评判与修复(ioctl 重构 capture→stamp→write-back、注释三连、测试字面量锚定、新增 2 个 ktest),amend 收口 | 同上 | `d539b725b` | 单提交,未推送;gate 复验全绿 |

review 评判结论:#1 失败路径回写 version(**接受**,UAPI 契约 + façade 实证);#2 minor 校验(**拒绝**,Linux 与 façade 均 major-only,更严会弹掉新版 libdm);#3 data_start 收紧(**接受**,仅注释);#4 ktest 自证恒等式(**接受**,硬编码上游字面量);#5 两段拷贝注释(**接受**,措辞修正为"后续只消费快照 B 字节,界以快照 A 为准");#6 1MiB 上限(**部分接受**,保留并注明理由,拒绝 16KiB——数字未证实且与 TABLE_LOAD 路线冲突)。修复前后 gate 证据见第 5 节。

## 3. 问题、目标与非目标

### 目标

- 以 Linux 兼容 misc 字符设备(major 10, minor 236)提供 `/dev/mapper/control`。
- 实现 `dm-ioctl` ABI 子集的首个命令 `DM_VERSION`:校验客户端主版本、内核版本原位盖章、回写(失败路径亦回写)。
- 信封进入门槛:`data_size`/`data_start` 几何校验;flags 校验(未知位 EINVAL,`DM_DEFERRED_REMOVE`/IMA 等已知未实现位 EOPNOTSUPP)。

### 非目标

- 不实现 `DM_VERSION` 之外的任何命令(DEV_CREATE/TABLE_LOAD/...),运行时注册表与 target 生命周期属后续阶段。
- 不实现 per-command flag 规则(随命令加入时补)。
- uevent/sysfs/udev 仍属项目级边界(见进度总账第 4 节)。

## 4. 技术设计与范围

ioctl 四步流水线:`decode_command`(非本子集命令 ENOTTY)→ 读 312 字节固定头定量(快照 A)→ `validate_ioctl_buffer_layout` → 按快照 A 的 `data_size` 全量再拷(快照 B)。两段拷贝对齐 Linux `copy_params`:界/校验/分配全出自快照 A,处理内容全出自快照 B,用户竞态只伤语义不破内存安全。

`DM_VERSION` 处理为 capture→stamp→write-back:先捕获主版本校验结果,无条件盖章 `[4, 50, 0]`,无条件回写,捕获结果单独决定返回值——对齐 dm-ioctl UAPI 契约("recognised 命令即使失败也填 version")且防自证(校验读的是盖章前的客户端原文)。major-only 校验与 Linux 一致,新版 libdm 依赖回写的 minor/patch 探测特性。

flags:19 个已知位(永久空洞 bit 7)与 `DM_KNOWN_FLAGS` 总掩码;未知位 EINVAL,已知未实现位 EOPNOTSUPP。

- `device_mapper.rs`:设备注册、文件语义(read/write EINVAL、ESPIPE、poll 恒就绪)、上述流水线与校验、19 个 ktest(decode 四态、layout 六态含上边界、flags 四态、握手两态加失败回写契约、u32 LE 往返、ioctl 编号硬编码上游字面量 312/0xc138_fd00 锚定)。

审阅重点:失败路径回写与 capture 顺序(UAPI 契约一致性);两段拷贝的内存安全论证(界出自快照 A);major-only 版本语义;data_start ≥ 312 的有意收紧。

## 5. 验证证据

容器内执行,真实退出码;ktest 均为无 selector 的 aster-core 全量(OSDK 0.19.0 selector 为后缀 trie,模块路径选择器必匹配空集,勘误详见 [2026-10-14](../daily/2026-10-14.md) 第 4 节):

| 时点 | 门 | 结果 |
|---|---|---|
| review 修复前(`3977e321e`) | make check / make kernel | exit 0 / exit 0 |
| 同上 | ktest(aster-core 全量) | 158 passed / 0 failed / 0 filtered(含 17 个 DM ktest) |
| review 修复后(`d539b725b`) | cargo fmt(-p aster-core) | 通过 |
| 同上 | clippy 双变体(-Dwarnings;普通 + `--ktests`) | 均通过 |
| 同上 | ktest(aster-core 全量) | 160 passed / 0 failed / 0 filtered(= 158 + 2 新;新用例串口逐行确认 ok) |
| 同上 | make check / make kernel | exit 0 / exit 0 |

未覆盖:`make ktest` 全量、C regression、NixOS 系统套件——Stage 1 无用户态客户端,系统级验证随控制面命令补齐后的阶段进行。

## 6. 已知边界与后续动作

1. 推送与 PR 创建由用户执行;创建后回填 PR 编号与 base 取向。
2. Stage 2(`DM_DEV_CREATE` 与运行时注册表)按学习节奏推进;flags 的 per-command 规则随命令加入补上。
3. 若维护者要求全量 `make ktest`,再补跑并回填本档案。

## 7. Upstream maintainer description

### Title

```text
Add the Device Mapper control device with `DM_VERSION` support
```

### Summary

- Introduce `/dev/mapper/control` as a Linux-compatible misc character device (major 10, minor 236) that speaks a subset of the `dm-ioctl` ABI from the Linux UAPI header `include/uapi/linux/dm-ioctl.h`.
- Implement the `DM_VERSION` handshake: validate the client's interface major version, stamp the kernel's version `[4, 50, 0]` into the envelope, and write the envelope back even when validation fails, as the dm-ioctl contract requires.
- Gate every ioctl behind envelope validation (`data_size`/`data_start` geometry) and flag validation (unknown bits are rejected with EINVAL; known but not-yet-supported bits with EOPNOTSUPP).

### Design

The control device is registered from the first kernel thread at init. Its ioctl follows the same shape as Linux `copy_params`: peek the fixed 312-byte header to size the envelope, then copy it whole. All bounds and validation decisions come from the first snapshot only, so a user racing the buffer between the two copies cannot break memory safety.

`DM_VERSION` validation results are captured instead of propagated: the envelope is always stamped and written back, and only the captured result decides the ioctl return value. Stamping after the version check keeps the check reading the client's bytes rather than the kernel's own stamp. Like Linux, only the major version is checked; the write-back reports the exact kernel version, which is how newer `libdm` releases probe for optional features.

### Scope

Included:

- `kernel/core/src/device/misc/device_mapper.rs` (new: control device, `DM_VERSION`, envelope and flag validation, ktests)
- registration in `kernel/core/src/device/misc/mod.rs`

Excluded: every command beyond `DM_VERSION`, the runtime device registry, per-command flag rules, and uevent/sysfs integration.

### Testing

Validated on final PR HEAD `d539b725b`:

```text
make check
make kernel
cd kernel/core && cargo osdk test --kcmd-args=earlycon
```

`make check` and `make kernel` pass. The aster-core ktest suite completes with 160 passed and 0 failed, including the 19 new control-device tests (command decoding, envelope geometry including the maximum-size boundary, flag handling, the version handshake including write-back on a rejected handshake, and the ioctl number anchored to upstream `dm-ioctl.h` literals). The full project ktest suite was not re-run for this PR; block and device-mapper crate coverage comes from the preceding PRs.

### Review notes

Please focus review on the failure-path write-back ordering (the captured-result-then-stamp sequence and its contract implications), the two-phase copy memory-safety argument, and the major-only version semantics. Please let me know if any part of the design or scope needs clarification. I'm happy to discuss it and will respond promptly.
