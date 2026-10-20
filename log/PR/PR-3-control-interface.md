# PR 3：公共 DM 控制入口与首个 VERSION 命令

> 更新：2026-10-20。当前事实以 `5b08f9a0a` 源码为准；旧提交的测试结果仅作为历史记录。
> 协议及结构体解释见 [控制面 ABI](../../docs/control_ABI.md)，本次操作见 [10 月 20 日日志](../daily/2026-10-20.md)。

## 1. 状态与 Git 标识

| 项目 | 当前事实 |
|---|---|
| 分支 | `dm-control-interface` |
| base | PR2 `fd97f023f`，分支 `dm-linear-io-path` |
| HEAD | `5b08f9a0a801871c8b6b2670f72d0310611a88bc` |
| 被替代的 PR3 提交 | `f662bdc2d68d6bd94335f4172be2c8f0a9e6a4b0` |
| 提交主题 | Add the Device Mapper control device with `DM_VERSION` support |
| 提交组织 | PR3 仍为 base 上的单提交，本次通过 amend 纳入修正 |
| 推送 | 本次未推送；没有重新查询远端状态，不据本地分支推断远端最新 SHA |
| PR4 分支 | `dm-linear-configuration` 已从新 PR3 重建，仍指向 `5b08f9a0a`，无独有提交；用户要求暂缓推进 |
| 文档归属 | 本档案及 docs/log 在 dm 保存，不加入 upstream 源码 PR |

## 2. 目标与边界

PR3 的目标是建立**通用 DM 控制入口**，VERSION 是第一个接入的命令，不是公共入口的全部验收边界。没有实现 CREATE 等命令，不代表可以忽略公共权限、版本协商和未支持命令的失败顺序。

已实现：

- 注册 `/dev/mapper/control`，字符设备 major 10、minor 236。
- 每次 ioctl 检查当前调用者在 initial user namespace 的 CAP_SYS_ADMIN；能力不足返回 EACCES。
- 检查 DM type，提取命令号；不按 direction/size 额外拒绝。
- 所有 DM type 请求先进行 12 字节版本握手，再分派命令；VERSION 成功，其余暂时 ENOTTY。
- major 相同、客户端 minor 不高于服务端 minor，忽略 patch；无论兼容与否回写服务端版本，写故障优先于不兼容错误。
- 普通 read/write 不支持，seek 不支持；无控制事件时 poll 不虚报就绪。
- Rust 内部测试、C 用户态回归，以及指定单项回归入口。

未实现：普通 dm_ioctl 参数复制、CREATE/LOAD/STATUS/REMOVE、active/inactive 管理、块设备发布、动态生命周期、全局控制事件和 ARM_POLL、完整 dmsetup/LVM2 兼容。

报告接口版本 4.50.0 不是完整功能声明。指定 openEuler 基线 UAPI 报告 4.48.0；本次没有修改版本号或宣称两者具备相同命令集合。

## 3. 本次修正（2026-10-20）

### 3.1 入口顺序

旧提交把 type 检查与命令支持检查放在同一个 `decode_command()` 中，导致未知 DM 命令在版本握手前返回 ENOTTY。

现在分为：

```text
权限检查 → type 检查/提取 nr → 版本读取 → 版本回写
→ 传播版本不兼容错误 → 命令分派
```

具体影响：有权限且 DM type 正确时，未知命令搭配 NULL 返回 EFAULT，搭配不兼容的可读写版本返回 EINVAL 并回写版本，搭配兼容版本才在握手后返回 ENOTTY。错误 type 仍在用户内存访问前返回 ENOTTY。

该问题在源码中曾被标注为限制，但“限制已记录”不能代替对 PR3 公共入口目标的验收。完整 dm 已有先握手后查命令的顺序，本次修正没有把完整 dm 降级或引入新命令。

### 3.2 文件命名与测试

- `misc/device_mapper.rs` → `misc/dm_control.rs`，同步模块声明及初始化调用。
- 不提前复制完整 dm 的 abi/commands/runtime/workflow 子模块。
- 将旧解码拒绝测试替换为 `decodes_command_number_only_for_dm_type`；文件共 5 个 ktest，数量未增加。
- C 新增 `negotiates_before_command_lookup`；使用未知 nr 0xff，避免在 Linux 上误执行真实状态变更命令。
- 覆盖未知命令的版本回写、版本不兼容、NULL、页末版本、只读页回写故障及错误 type；继承 fd 测试追加权限先于未知命令及地址访问的断言。
- C 文件现在有 7 个 FN_TEST，不含 setup/cleanup；这不是断言数或实测通过计数。

### 3.3 提交说明与分支

amend 同时修正旧提交说明中“先固定头再完整缓冲复制”的过时描述，改为公共入口和仅 12 字节 VERSION 的真实行为。

旧 PR4 `dm-linear-configuration` 与旧 PR3 完全相同，无独有提交。amend 后普通 `git branch -d` 因旧提交不再是新提交祖先而拒绝；再次确认旧 SHA 后按用户授权删除并重建。新 PR4 与新 PR3 相同，工作区当时干净，未推送。

## 4. 最终 PR 独有 diff

`fd97f023f..5b08f9a0a` 共 **6 文件，514 insertions、10 deletions**：

| 文件 | 职责 |
|---|---|
| Makefile | REGRESSION_TESTS 选择器透传 |
| kernel/core/src/device/misc/dm_control.rs | 控制设备、公共入口、VERSION、5 个 ktest |
| kernel/core/src/device/misc/mod.rs | 控制设备模块接入及初始化 |
| test/initramfs/src/regression/device/device_mapper_control.c | 真实用户态 ABI 回归 |
| test/initramfs/src/regression/device/run_test.sh | 加入 control 测试程序 |
| test/initramfs/src/regression/scripts/run_regression_test.sh | 校验并执行目录/可执行文件选择器；无选择器仍走原全量路径 |

本次相对旧 PR3 的修订为 **3 个逻辑文件，67 insertions、15 deletions**，其中控制文件重命名被 Git 识别。不能把这组增量统计与完整 PR diff 混为一谈。

## 5. 验证记录与证据边界

| 项目 | 本次可确认事实 |
|---|---|
| C 语法 | Claude 在容器执行 `gcc -Wall -Wextra -Werror -D__asterinas__ -fsyntax-only .../device_mapper_control.c`，通过 |
| C 格式 | 容器内 `clang-format --dry-run --Werror` 通过 |
| diff 空白 | 修订暂存内容 `git diff --cached --check` 通过 |
| C 用户态运行 | 用户明确反馈“C回归我跑完了”；没有归档该次输出、计数、退出码或精确源码 tree，不补写独立核验通过 |
| 最终 make format/check/kernel | 本次未独立运行或收集最终输出；先前提交结果不替代当前 SHA |
| aster-core ktest | 已提供所属 crate 内无 selector 的运行命令；当前对话没有最终运行计数/退出码，不填入旧树的 160/0 或 dm 的 98/0 |
| amend/重命名后重跑 | 未由 Claude 执行，不声称新 SHA 的所有最终 gate 已独立验证 |

用户随后表示 PR3 可以收口并明确授权 amend；这构成提交授权，不等于产生缺失的测试日志。

后续复验在容器内、PR3 自身源码树串行执行：

```bash
cd /root/asterinas
make format
# 检查格式化是否带来范围外修改。
make check
make kernel

cd /root/asterinas/kernel/core
cargo osdk test --kcmd-args=earlycon

cd /root/asterinas
make run_kernel AUTO_TEST=regression REGRESSION_TESTS=device/device_mapper_control
```

定向最小覆盖单位为相关文件；默认进入其所属 Cargo crate 跑全部 ktest，不逐函数循环。这里是 aster-core 范围，不是全仓 make ktest。实际子命令是 `test`，不是 `cargo osdk ktest`。记录测试名、非零计数、失败数、退出码及日志；当前 PR3 OSDK 不支持直接用模块路径代替函数路径筛选，不套用完整 dm 的实现。

## 6. 历史及已撤回的结论

| 日期 | 提交/事件 | 历史事实与限制 |
|---|---|---|
| 2026-10-14 | `3977e321e` | 从 PR2 `d0088c2c9` 制备；当时 check/kernel exit 0，aster-core 158/0 |
| 2026-10-14 | `d539b725b` | 当时 review 后 amend；格式/clippy/check/kernel 通过，aster-core 160/0（19 个旧 DM 测试） |
| 2026-10-15 | 握手复核 | 撤回 major-only、VERSION 必须复制完整头/信封等结论；旧测试成功没有证明 Linux ABI 语义正确 |
| 2026-10-19 | `f662bdc2d` | 基于新版 PR2，已缩为 12 字节握手、能力检查、poll 与直接测试；未知命令提前拒绝仍作为限制存在 |
| 2026-10-20 | `5b08f9a0a` | 公共入口顺序修正、控制文件改名、测试更新并 amend；PR4 空分支重建 |

不再沿用的旧设计结论：

- “Linux 只检查 major”：错误；也要求 client minor ≤ server minor。
- “VERSION 校验 data_size/data_start/flags 并回写整个 envelope”：错误；VERSION 只访问 version 三个字。
- “两遍读整头/整块就是 Linux copy_params”：错误；固定头快照应复用，不能重读用户头替代已验证内容。
- “未知 flags 一律拒绝即可称 Linux 兼容”：错误；取决于命令及位语义。
- “poll 恒就绪”：不符合无事件状态；PR3 当前返回空，但不代表全局事件功能已经实现。
- “首个命令成功即可验收公共入口”：不足；未支持命令也受公共检查顺序约束。

旧原始文档内容可从 dm 文档历史追溯；上表保留对应提交与实测历史，不将旧错误解释继续列为当前维护者说明。

## 7. Upstream maintainer description（当前草稿）

### Title

Add the Device Mapper control device with `DM_VERSION` support

### Summary

Introduce `/dev/mapper/control` as a misc character device (major 10, minor 236) and establish the common Device Mapper ioctl entry path. Check the current caller's capability and ioctl type before exchanging the interface version, then dispatch the command. `DM_VERSION` is the first supported command; unsupported DM commands are rejected after a successful version handshake.

The handshake accesses only three native-endian version words. It requires an equal major and a client minor no newer than the server minor, ignores the patch, and writes back the server version even on incompatibility. A copyout fault takes precedence over the compatibility error. The file is nonseekable, does not support ordinary reads or writes, and reports no poll readiness while control events are unsupported.

### Scope and tests

The change includes kernel tests for decoding, version rules, and poll behavior, plus userspace regression tests for memory boundaries, failure ordering, unsupported commands, and per-call capability checks. Regression selectors allow running the control test without the full device suite.

Ordinary command envelopes, device creation, table loading, activation, removal, and global control events are outside this change. Reporting interface version 4.50.0 does not imply support for the complete DM command set.

Userspace execution was reported by the author. Final build and aster-core ktest output for this amended revision have not been archived in this record; no final pass count is claimed here. Fill in verified results before publishing a validation claim.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
