# Device Mapper 对话交接

> **最后更新**：2026-09-23
>
> **定位**：本文是一页式启动说明，只记录当前停点、阅读顺序、工作区边界和下一步决策。源码事实、测试命令和运行证据不在本文重复。

## 维护规则

本文与其他项目状态文档一样，在阶段结束、当前停点、阅读入口、工作区边界或可复用源码证据发生实际变化后自动更新；不需要用户逐次提出。更新只记录已发生的事实、下一步和必要入口，不复制源码细节、完整测试输出或未经验证的推测。实现细节以当前源码为准；已核对的源码事实以 [源码证据交接](device-mapper-source-evidence.md) 为准；当天实际操作以 [daily log](../log/daily/) 为准。

## 新窗口启动顺序

| 顺序 | 文档或动作 | 用途 |
|---|---|---|
| 1 | [CLAUDE.md](../CLAUDE.md) 与 [AGENTS.md](../AGENTS.md) | 获取协作规则、写入确认、容器与串行测试约束。 |
| 2 | [当前项目状态](../log/device-mapper-progress.md) | 获取路线、测试入口和主动边界。 |
| 3 | [源码证据交接](device-mapper-source-evidence.md) | 获取已读源码、blob hash、可复用事实、运行证据和未关闭边界。 |
| 4 | `git status --short` | 区分本阶段提交、当前未提交改动和来源待确认项。 |
| 5 | 对当前任务相关的证据表文件运行 `git hash-object <path>` | hash 一致时只阅读表中列出的符号；hash 不一致时才定点审查变更和直接依赖。 |

需要具体设计、测试命令或审核结论时，再按任务读取 [test.md](test.md)、[review.md](review.md)、[global.md](global.md) 或对应源码；不要把“交接”变回全局检索。

## 当前停点

### 已完成并提交的 V4 测试阶段

截至 `05ab2beed`，以下阶段提交已完成：

```text
e6f00b9fa test(regression): report aggregate C assertion totals
8868631f0 test(nixos): consolidate DM suite logs and LVM reporting
6c4bf7ad6 docs(test): document C and DM system regression workflows
05ab2beed docs(dm): record system test acceptance
```

已验证结论摘要：

- DM crate ktest：在线 86/0；
- core crate 全量 ktest：在线 205/0，runner 为 16 crates、621 tests；
- focused `device/device_mapper` C 回归：在线 159/0；
- control-plane、dataplane、LVM2 topology、linear、striped、mixed 六个 canonical NixOS suite 均通过；
- harness 每个 suite 使用一个权威日志，正常路径不残留 QEMU 状态文件或日志 FIFO；
- LVM2 topology 的跨 PV linear 验证以汇总 `seg_count=2` 加 DM table/dependencies 两 backing 断言为准。

完整非 TDX initramfs regression 已证明在线构建和 guest 启动链路可用，但 `/test/network` 的 `test_tcp_append_after_peek_full_read` 为 137/1 失败；它不是 DM 失败，不能把完整 regression 标记为通过。

### 已完成、待提交的工程 P0 第一项：table-scoped readonly

`DM_READONLY_FLAG` 已从 mapper identity 的永久状态收敛为 immutable table mode：load 只暂存 inactive mode，首次/替换 resume 后才切换 active I/O 与查询状态；tableless create 不再持久化该 flag。active/inactive 查询与 BIO 分派均从同一 table state snapshot 取得 mode；readonly active 在 suspend 中仍拒绝新 write-like BIO。

实际验证：DM crate 86/0、core DM ioctl 81/0、focused C 182/0（新增 transition 23/0）。control-plane 最初两次在默认 40 秒 ready timeout 前未进入 guest；放宽至 50 秒后发现镜像过期，`make nixos` 重建镜像后复跑通过，guest 63 秒完成、全程 71 秒、`SUMMARY_GAP_DM_CONTROL_PLANE: 0`，正常退出且无 QEMU 残留。

### 当前未关闭边界

- mount source 类型；
- 真实 alias/VFS 深层恢复；
- DM-on-DM stacking；
- exfat、RawDisk/subset/clone lease；
- VirtIO/NVMe 故障注入。

除非用户单独授权，或出现用户可见失败、数据风险或新 target 依赖，不主动扩展这些范围。

## 当前工作区边界

接手时不要假定工作区干净。当前仍有待单独审阅的协作规则、学习资料和 DOCX 替换关系；不得把它们与已验证 DM 测试阶段混合提交或回退。

| 组别 | 当前项 | 处理 |
|---|---|---|
| 协作规则 | `AGENTS.md`、`CLAUDE.md` | 等待确认是否为项目长期规则。 |
| 学习资料 | `docs/plan.md`、`docs/study.md` | 等待人工审阅，避免不可逆压缩旧内容。 |
| DOCX | 两个删除历史 DOCX、两个未跟踪 DOCX | 来源、权威性和替换关系未确认。 |
| P0 readonly 阶段 | DM 生产源码、focused C、control-plane 脚本、`docs/device-mapper-source-evidence.md`、`log/device-mapper-progress.md`、`log/daily/2026-9-23.md` | 已验证，等待用户决定是否提交；不得混入其他来源待确认项。 |

## 下一步

当前 V4 与工程 P0 第一项均已收口。下一步必须由用户选择：

1. 进入 P0 的下一项具体 DM 用户可见行为；
2. 进入 verity 的设计确认；
3. 单独处理完整 C regression 的非 DM 网络失败；
4. 单独审阅并提交或清理上述来源待确认文档。

新功能或新阶段开始前，先说明原行为、目标行为、示例差异、涉及文件和最小验证；获得确认后再修改。

## 安全规则

- 写入、测试、提交前先核对 `git status --short`；
- 不覆盖、回退、格式化或提交来源不明的改动；
- 构建和测试默认在 `myAsterinas` 容器的 `/root/asterinas` 中进行；
- QEMU、ktest、C regression 与 NixOS system test 必须串行；
- 默认不 push。
