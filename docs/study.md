# Device Mapper 学习记录

## `/dev/mapper/control` 注册主线

这条主线解释 `/dev/mapper/control` 如何从内核启动期注册到用户态可见。重点不是 DM table、target 或 BIO 数据面，而是 DM control 字符设备如何挂到 Asterinas 的 misc/char/devtmpfs 链路下。

### 1. first kthread 阶段

入口在 `misc::init_in_first_kthread()`。

```text
misc::init_in_first_kthread()
  -> acquire_major(MajorId::new(10))
     准备 misc 字符设备 major 10

  -> device_mapper::init_in_first_kthread()
     -> 先初始化 DM_MANAGER
        不是为了注册 control 的硬依赖，
        而是保证 control 暴露给用户态前，后端 manager 已就绪

     -> DmControlDevice::new()
        创建 DM control 字符设备对象
        绑定 DeviceId = misc major 10 + DM_CONTROL_MINOR

     -> char::register(...)
        注册到 char DEVICE_REGISTRY
        建立 DeviceId -> DmControlDevice 的映射
```

### 2. first process 阶段

`char::register(...)` 只完成设备号到设备对象的注册；真正创建 `/dev/mapper/control` 路径节点发生在 first process 阶段。

```text
char::init_in_first_process(path_resolver)
  -> collect_all()
  -> 找到 DmControlDevice
  -> device.devtmpfs_meta()
     返回 "mapper/control"

  -> add_node(DeviceType::Char, dev_id, meta, path_resolver)
     在 /dev 下创建 mapper/control 节点
```

### 3. 阶段结论

最终效果是：

```text
/dev/mapper/control 出现在用户态
用户态 dmsetup 可以 open 它
后续 ioctl 会进入 DmControlFile::ioctl()
再分发到 DM 控制面逻辑
```

理解这条链路时要区分：

- `misc major 10`：给 `/dev/mapper/control` 这个 misc 字符设备使用。
- `DM_MANAGER`：control ioctl 后端依赖的全局管理器，后续管理 mapper block devices。
- `DmControlDevice`：暴露给用户态的 control 字符设备对象。
- `char::register(...)`：建立 `DeviceId -> DmControlDevice` 的内核注册表映射。
- `devtmpfs_meta()`：由 `DmControlDevice` 自己声明用户态路径 `mapper/control`。
- `add_node(...)`：根据 metadata 在 `/dev` 下创建真实路径节点。

## `dmsetup` 控制面命令对齐矩阵

本节用于批量对齐标准 Linux/OpenEuler 与 Asterinas 的 `dmsetup` 用户可见语义。基准入口是 [run_dmsetup_linux_cli_baseline.sh](../myshell/run_dmsetup_linux_cli_baseline.sh)，Asterinas guest 验证入口是 [run_dmsetup_cli_semantics_test.sh](../myshell/run_dmsetup_cli_semantics_test.sh)，统一通过 [run_dm_system_tests.sh](../myshell/run_dm_system_tests.sh) 的 `--dmsetup-cli` suite 运行。

本轮 Linux/OpenEuler 基准已在 2026-08-28 运行，基准结果已固化在下表；当时使用的 `/tmp/dmsetup-linux-baseline.log` 是临时日志，当前不作为现存依据。本机存在非测试 DM 设备 `openeuler-root`、`openeuler-swap`、`wdf`，因此 `EMPTY_LS`、`REMOVE_ALL_TEST_ONLY`、`REMOVE_ALL_EMPTY` 被标记为 `SKIPPED_HOST_UNSAFE`，没有在宿主机执行全局 `remove_all` 语义。Asterinas guest 已在 2026-08-31 重建 NixOS 后通过 `GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --dmsetup-cli` 完成复测：guest ready 19s，guest 内命令审计 7s，`SUMMARY_GAP_DMSETUP_CLI_SEMANTICS: 0`。

动态值归一化规则：`/dev/loopX` 记为 `<LOOP1>/<LOOP2>`，backing 设备号记为 `<DEV1>/<DEV2>`，`/dev/dm-N` 记为 `/dev/dm-<N>`，版本号记为 `<VERSION>`，event number 只记录关键变化。对齐状态按用户可见语义判断，不要求 stdout 字节级完全相同；例如本机支持更多 target，而 Asterinas 只列出当前已实现的 `linear`、`striped`，仍属于符合当前阶段目标。

### 静态查询

| 命令 / 前置条件 | 标准 Linux 用户可见行为 | 当前 Asterinas 情况 | 依据 | 当前结论 |
| --- | --- | --- | --- | --- |
| `dmsetup version`<br>无 mapper 依赖 | status 0<br>输出 Library / Driver version | status 0<br>输出 `1.02.205` / `4.48.0` | 本机基准<br>guest `STATIC_VERSION` | 已对齐 |
| `dmsetup targets`<br>查询可用 target | status 0<br>列出内核支持的 targets | status 0<br>列出已实现的 `error`、`linear`、`striped` | 本机 target 更多是支持范围差异<br>guest `STATIC_TARGETS` | 已对齐 |
| `dmsetup target-version error`<br>target 存在 | status 0<br>`error v<VERSION>` | status 0<br>`error v1.6.0` | 本机基准<br>guest `TARGET_VERSION_ERROR` | 已对齐 |
| `dmsetup target-version linear`<br>target 存在 | status 0<br>`linear v<VERSION>` | status 0<br>`linear v1.4.0` | 本机基准<br>guest `TARGET_VERSION_LINEAR` | 已对齐 |
| `dmsetup target-version striped`<br>target 存在 | status 0<br>`striped v<VERSION>` | status 0<br>`striped v1.6.0` | 本机基准<br>guest `TARGET_VERSION_STRIPED` | 已对齐 |
| `dmsetup target-version aster_unknown`<br>target 不存在 | status 1<br>`Invalid argument` | status 1<br>`Invalid argument` | 本机基准<br>guest `TARGET_VERSION_UNKNOWN` | 已对齐 |

### list 与 tableless device

| 命令 / 前置条件 | 标准 Linux 用户可见行为 | 当前 Asterinas 情况 | 依据 | 当前结论 |
| --- | --- | --- | --- | --- |
| `dmsetup ls`<br>无 mapper | status 0<br>`No devices found` | guest status 0<br>`No devices found` | 宿主非空，不能安全测空列表<br>guest `EMPTY_LS` | 限定场景已对齐 |
| `dmsetup ls`<br>存在 tableless mapper | status 0<br>输出包含 tableless mapper 的名称和设备号 | status 0<br>输出包含 tableless mapper 的名称和设备号 | 本机基准<br>guest `TABLELESS_LS` 使用 stdin EOF 和 `--notable` 两种 tableless 创建方式 | 已对齐 |
| `timeout 5 dmsetup create NAME < /dev/null`<br>stdin EOF，无 table | status 0<br>`info` 可查<br>不创建 mapper block node | status 0<br>`info` 可查<br>不创建 mapper block node | 本机基准<br>guest `CREATE_STDIN_EOF` | 已对齐 |
| `dmsetup create NAME --notable`<br>显式 tableless | status 0<br>`info` 可查<br>不创建 mapper block node | status 0<br>`info` 可查<br>不创建 mapper block node | 本机基准<br>guest `CREATE_NOTABLE` | 已对齐 |
| `dmsetup info NAME`<br>device 存在但无 table | ACTIVE<br>Tables None<br>targets 0 | ACTIVE<br>Tables None<br>targets 0 | 本机基准<br>guest `TABLELESS_INFO_*` | 已对齐 |
| `dmsetup remove NAME`<br>tableless device 存在 | status 0<br>device 删除 | status 0<br>device 删除 | 本机基准<br>guest `TABLELESS_REMOVE_*` | 已对齐 |

### linear target

| 命令 / 前置条件 | 标准 Linux 用户可见行为 | 当前 Asterinas 情况 | 依据 | 当前结论 |
| --- | --- | --- | --- | --- |
| `dmsetup create NAME`<br>stdin table：`0 8 linear DEV 0` | status 0<br>创建 mapper node | status 0<br>创建 mapper node | 本机基准<br>guest `LINEAR_CREATE_MAJOR` | 已对齐 |
| `dmsetup create NAME --table ...`<br>table 使用 backing path | status 0<br>后续 table 输出 backing 设备号 | status 0<br>`/dev/vde` 转为 `253:64` | 本机基准<br>guest `LINEAR_CREATE_PATH` | 已对齐 |
| `dmsetup info NAME`<br>active linear mapper 存在 | status 0<br>mapper 可查 | status 0<br>mapper 可查 | 本机基准<br>guest `LINEAR_INFO` | 已对齐 |
| `dmsetup table NAME`<br>active linear table 存在 | 输出 active table<br>`0 8 linear DEV 0` | 输出 active table<br>`0 8 linear 253:64 0` | 本机基准<br>guest `LINEAR_TABLE` | 已对齐 |
| `dmsetup status NAME`<br>active linear table 存在 | status 0<br>`0 8 linear ` | status 0<br>`0 8 linear ` | 本机基准<br>guest `LINEAR_STATUS` | 已对齐 |
| `dmsetup deps NAME`<br>active linear table 存在 | status 0<br>1 dependency | status 0<br>1 dependency | 本机基准<br>guest `LINEAR_DEPS` | 已对齐 |
| `dmsetup remove NAME`<br>active linear mapper 存在 | status 0<br>node 删除 | status 0<br>node 删除 | 本机基准<br>guest `LINEAR_REMOVE` | 已对齐 |

### striped target

| 命令 / 前置条件 | 标准 Linux 用户可见行为 | 当前 Asterinas 情况 | 依据 | 当前结论 |
| --- | --- | --- | --- | --- |
| `dmsetup create NAME`<br>stdin table：`striped 2 4 DEV1 0 DEV2 0` | status 0<br>创建 mapper node | status 0<br>创建 mapper node | 本机基准<br>guest `STRIPED_CREATE_MAJOR` | 已对齐 |
| `dmsetup create NAME --table ...`<br>table 使用两个 backing path | status 0<br>后续 table 输出 backing 设备号 | status 0<br>`/dev/vde/vdf` 转为 `253:64/253:80` | 本机基准<br>guest `STRIPED_CREATE_PATH` | 已对齐 |
| `dmsetup info NAME`<br>active striped mapper 存在 | status 0<br>mapper 可查 | status 0<br>mapper 可查 | 本机基准<br>guest `STRIPED_INFO` | 已对齐 |
| `dmsetup table NAME`<br>active striped table 存在 | 输出 table 参数<br>含 chunk size 和 backing start | 输出 table 参数<br>格式匹配 | 本机基准<br>guest `STRIPED_TABLE` | 已对齐 |
| `dmsetup status NAME`<br>active striped table 存在 | 输出 striped status 参数<br>`2 DEV1 DEV2 1 AA` | 输出 striped status 参数<br>`2 DEV1 DEV2 1 AA` | 本机基准<br>guest `STRIPED_STATUS` | 已对齐 |
| `dmsetup deps NAME`<br>active striped table 存在 | status 0<br>2 dependencies | status 0<br>2 dependencies | 本机基准<br>guest `STRIPED_DEPS` | 已对齐 |
| `dmsetup remove NAME`<br>active striped mapper 存在 | status 0<br>可删除 | status 0<br>可删除 | 本机基准<br>guest `STRIPED_REMOVE` | 已对齐 |

### error target

| 命令 / 前置条件 | 标准 Linux 用户可见行为 | 当前 Asterinas 情况 | 依据 | 当前结论 |
| --- | --- | --- | --- | --- |
| `dmsetup create NAME --table "0 8 error"`<br>无 backing 参数 | status 0<br>创建 mapper node | status 0<br>创建 mapper node | 本机基准<br>guest `ERROR_CREATE` | 已对齐 |
| `dmsetup table NAME`<br>active error mapper 存在 | 输出 `0 8 error` | 输出 `0 8 error` | 本机基准<br>guest `ERROR_TABLE` | 已对齐 |
| `dmsetup status NAME`<br>active error mapper 存在 | status 0<br>target type 为 `error`，参数为空 | status 0<br>target type 为 `error`，参数为空 | 本机基准<br>guest `ERROR_STATUS` | 已对齐 |
| `dmsetup deps NAME`<br>active error mapper 存在 | status 0<br>`0 dependencies` | status 0<br>`0 dependencies` | 本机基准<br>guest `ERROR_DEPS` | 已对齐 |
| 对 `/dev/mapper/NAME` 读写<br>active error mapper 存在 | 读写返回 I/O error | 读写返回 I/O error | 本机基准<br>guest `ERROR_READ` / `ERROR_WRITE` | 已对齐 |
| `dmsetup remove NAME`<br>active error mapper 存在 | status 0<br>可删除 | status 0<br>可删除 | 本机基准<br>guest `ERROR_REMOVE` | 已对齐 |

### table 生命周期

| 命令 / 前置条件 | 标准 Linux 用户可见行为 | 当前 Asterinas 情况 | 依据 | 当前结论 |
| --- | --- | --- | --- | --- |
| `dmsetup load NAME --table ...`<br>device 有 active table | status 0<br>只装载 inactive table | status 0<br>只装载 inactive table | 本机基准<br>guest `LOAD_INACTIVE` | 已对齐 |
| `dmsetup table NAME`<br>load 后未 resume | 仍返回旧 active table | 仍返回旧 active table | 本机基准<br>guest `TABLE_ACTIVE_AFTER_LOAD` | 已对齐 |
| `dmsetup table --inactive NAME`<br>存在 inactive table | 返回 inactive table | 返回 inactive table | 本机基准<br>guest `TABLE_INACTIVE_AFTER_LOAD` | 已对齐 |
| `dmsetup status --inactive NAME`<br>存在 inactive table | status 0<br>返回 inactive status | status 0<br>返回 inactive status | 本机基准<br>guest `STATUS_INACTIVE_AFTER_LOAD` | 已对齐 |
| `dmsetup info NAME`<br>同时有 active/inactive table | `Tables present: LIVE & INACTIVE` | `Tables present: LIVE & INACTIVE` | event 数值不作为单独语义差异<br>guest `INFO_AFTER_LOAD` | 已对齐 |
| `dmsetup clear NAME`<br>存在 inactive table | status 0<br>清除 inactive<br>active 保留 | status 0<br>清除 inactive<br>active 保留 | 本机基准<br>guest `CLEAR_INACTIVE` | 已对齐 |
| `dmsetup table --inactive NAME`<br>clear 后无 inactive table | status 0<br>stdout 空 | status 0<br>stdout 空 | 本机基准<br>guest `TABLE_INACTIVE_AFTER_CLEAR` | 已对齐 |
| `dmsetup reload NAME --table ...`<br>active mapper 存在 | status 0<br>装载 inactive table | status 0<br>装载 inactive table | 本机基准<br>guest `RELOAD_INACTIVE` | 已对齐 |
| `dmsetup resume NAME`<br>存在 inactive table | status 0<br>inactive 切为 active | status 0<br>inactive 切为 active | 本机基准<br>guest `RESUME_AFTER_RELOAD` | 已对齐 |

### suspend / resume / wait

| 命令 / 前置条件 | 标准 Linux 用户可见行为 | 当前 Asterinas 情况 | 依据 | 当前结论 |
| --- | --- | --- | --- | --- |
| `dmsetup suspend NAME`<br>active mapper 存在 | status 0<br>State 变 SUSPENDED | status 0<br>State 变 SUSPENDED | 本机基准<br>guest `SUSPEND` | 已对齐 |
| `dmsetup resume NAME`<br>suspended mapper 存在 | status 0<br>State 变 ACTIVE | status 0<br>State 变 ACTIVE | 本机基准<br>guest `RESUME` | 已对齐 |
| `dmsetup suspend --noflush NAME`<br>active mapper 存在 | status 0<br>可被 resume | status 0<br>可被 resume | 本机基准<br>guest `SUSPEND_NOFLUSH` | 已对齐 |
| `dmsetup resume --noflush NAME`<br>suspended mapper 存在 | status 0<br>回到 ACTIVE | status 0<br>回到 ACTIVE | 本机基准<br>guest `RESUME_NOFLUSH` | 已对齐 |
| `timeout 3 dmsetup wait --noflush NAME 0`<br>当前 event 为 0 | Linux 等待到 timeout<br>status 124 | 等待到 timeout<br>status 124 | 本机基准<br>guest `WAIT_ZERO` | 已对齐 |
| `dmsetup wait --noflush NAME EVENT`<br>后台 wait 当前 event，再触发 suspend | Linux wait 未被 suspend 唤醒<br>status 124 | wait 未被 suspend 唤醒<br>status 124 | 本机基准<br>guest `WAIT_OLD_EVENT` | 已对齐 |

### rename / UUID

| 命令 / 前置条件 | 标准 Linux 用户可见行为 | 当前 Asterinas 情况 | 依据 | 当前结论 |
| --- | --- | --- | --- | --- |
| `dmsetup rename OLD NEW`<br>OLD 存在，NEW 不存在 | status 0<br>旧名失效，新名可查 | status 0<br>旧名失效，新名可查 | 本机基准<br>guest `RENAME_NAME` | 已对齐 |
| `dmsetup rename NAME NAME`<br>源名和目标名相同 | status 1<br>`Device or resource busy` | status 1<br>`Device or resource busy` | 本机基准<br>guest `RENAME_SAME_NAME` | 已对齐 |
| `dmsetup rename NAME EXISTING`<br>目标名已存在 | status 1<br>状态不破坏 | status 1<br>状态不破坏 | stderr 文本不同不影响本阶段语义<br>guest `RENAME_DUPLICATE` | 已对齐 |
| `dmsetup rename NAME --setuuid UUID`<br>device 存在 | status 0<br>name 不变，uuid 改变 | status 0<br>name 不变，uuid 改变 | 本机基准<br>guest `SET_UUID` | 已对齐 |
| `dmsetup info -u UUID`<br>UUID 存在 | status 0<br>按 UUID 查到 device | status 0<br>按 UUID 查到 device | 本机基准<br>guest `INFO_BY_UUID` | 已对齐 |

### remove / remove_all

| 命令 / 前置条件 | 标准 Linux 用户可见行为 | 当前 Asterinas 情况 | 依据 | 当前结论 |
| --- | --- | --- | --- | --- |
| `dmsetup remove NAME`<br>active mapper 存在 | status 0<br>device 删除 | status 0<br>device 删除 | 本机基准<br>guest `REMOVE_ACTIVE` | 已对齐 |
| `dmsetup info NAME`<br>刚 remove 后再查 | status 1<br>`Device does not exist` | status 1<br>`Device does not exist` | 本机基准<br>guest `INFO_AFTER_REMOVE_ACTIVE` | 已对齐 |
| `dmsetup remove NAME`<br>tableless device 存在 | status 0<br>device 删除 | status 0<br>device 删除 | 本机基准<br>guest `REMOVE_TABLELESS` | 已对齐 |
| `dmsetup remove NAME`<br>device 不存在 | status 1<br>`No such device or address` | status 1<br>同样错误 | 本机基准<br>guest `REMOVE_NONEXISTENT` | 已对齐 |
| `dmsetup remove_all`<br>存在脚本创建的 mapper | 宿主非空<br>未执行全局 remove_all | guest status 0<br>测试 mapper 均删除 | 宿主安全跳过<br>guest `REMOVE_ALL_TEST_ONLY` | 限定场景已对齐 |
| `dmsetup remove_all`<br>空 DM 环境 | 宿主非空<br>未测 | guest status 0<br>stdout/stderr 空 | 宿主安全跳过<br>guest `REMOVE_ALL_EMPTY` | 待测 |

当前阶段明确不纳入：`dmsetup message`、`geometry`、`stats`、`udev`、`mknodes`、`tree`、`columns`、`wipe_table`、deferred remove、IMA，以及 `snapshot`、`thin`、`cache`、`crypt`、`mirror`、`zero` 等 target 族。后续如果要支持，需要另起小阶段先做本机基准。

## LVM2 控制面命令对齐矩阵

本节用于对齐标准 Linux/OpenEuler 与 Asterinas 当前 Device Mapper 能支撑的 LVM2 控制面子集。主机基准入口是 [run_lvm2_linux_cli_baseline.sh](../myshell/run_lvm2_linux_cli_baseline.sh)，已在 2026-08-31 用临时 loop 设备完成实测：`SUMMARY_GAP_LVM2_LINUX_BASELINE: 0`，且 postflight 未发现非测试 LVM/DM 快照变化。Asterinas guest 同构入口是 [run_lvm2_cli_semantics_test.sh](../myshell/run_lvm2_cli_semantics_test.sh)，已通过 `GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --lvm2-cli` 完成复测：`SUMMARY_GAP_LVM2_CLI_SEMANTICS: 0`。该结论只表示当前脚本覆盖的 linear/striped/mixed LVM2 控制面子集在限定场景下对齐，不代表完整 LVM2 兼容。

主机安全口径：所有 destructive 命令只允许作用于本次 manifest 中的测试 VG/LV/PV 和临时 loop device；不删除、不 deactivate、不清理用户已有 PV/VG/LV。

LVM2 common 参数口径：表格里的 `pvcreate`、`vgcreate`、`lvcreate`、`lvextend` 等是命令模板。host/guest 实际执行时都会追加 `COMMON_LVM_ARGS`：`--config 'devices { use_devicesfile=0 filter/global_filter=[测试盘, reject all] } activation { udev_rules=0 udev_sync=0 }'`，并在支持时追加 `--devices <测试盘列表>`。因此当前结论只证明“命令模板 + COMMON_LVM_ARGS”在测试盘限定场景下对齐；裸 LVM2 命令、默认 devices file、默认 udev/systemd 联动仍未判定。

### 静态查询与空测试环境

| 命令 / 前置条件 | 标准 Linux 用户可见行为 | 当前 Asterinas 情况 | 依据 | 当前结论 |
| --- | --- | --- | --- | --- |
| `pvs -o pv_name,vg_name,pv_size`<br>测试 filter 下无测试 PV | status 0<br>stdout 空 | status 0<br>stdout 空 | host `STATIC_PVS`<br>guest `STATIC_PVS` | 已对齐 |
| `vgs -o vg_name,pv_count,lv_count,`<br>`vg_size,vg_free`<br>测试 filter 下无测试 VG | status 0<br>stdout 空 | status 0<br>stdout 空 | host `STATIC_VGS`<br>guest `STATIC_VGS` | 已对齐 |
| `lvs -a -o vg_name,lv_name,lv_size,`<br>`seg_count,devices`<br>测试 filter 下无测试 LV | status 0<br>stdout 空 | status 0<br>stdout 空 | host `STATIC_LVS`<br>guest `STATIC_LVS` | 已对齐 |

### PV / VG 生命周期

| 命令 / 前置条件 | 标准 Linux 用户可见行为 | 当前 Asterinas 情况 | 依据 | 当前结论 |
| --- | --- | --- | --- | --- |
| `pvcreate -ff -y`<br>`<DISK1> <DISK2> <DISK3> <DISK4>` | status 0<br>创建测试 PV | status 0<br>创建测试 PV | host `PV_CREATE`<br>guest `PV_CREATE` | 已对齐 |
| `pvs -o pv_name,pv_size,vg_name`<br>PV 已创建 | status 0<br>列出测试 PV | status 0<br>列出测试 PV | host `PVS_AFTER_PVCREATE`<br>guest `PVS_AFTER_PVCREATE` | 已对齐 |
| `pvscan`<br>PV 已创建 | status 0<br>可扫描测试 PV | status 0<br>可扫描测试 PV | host `PVSCAN_AFTER_PVCREATE`<br>guest `PVSCAN_AFTER_PVCREATE` | 已对齐 |
| `vgcreate <VG>`<br>`<DISK1> <DISK2>`<br>测试 PV 存在 | status 0<br>创建测试 VG | status 0<br>创建测试 VG | host `VG_CREATE`<br>guest `VG_CREATE` | 已对齐 |
| `vgextend <VG>`<br>`<DISK3> <DISK4>`<br>VG 已存在 | status 0<br>PV count 增加 | status 0<br>PV count 增加 | host `VG_EXTEND`<br>guest `VG_EXTEND` | 已对齐 |
| `vgs -o vg_name,vg_size,vg_free,`<br>`pv_count,lv_count`<br>VG 已创建或扩展 | status 0<br>显示 PV/LV count | status 0<br>显示 PV/LV count | host `VGS_AFTER_*`<br>guest `VGS_AFTER_*` | 已对齐 |

### linear LV

| 命令 / 前置条件 | 标准 Linux 用户可见行为 | 当前 Asterinas 情况 | 依据 | 当前结论 |
| --- | --- | --- | --- | --- |
| `lvcreate --type linear -L 32M`<br>`-n <LINEAR_LV> <VG> <DISK1>` | status 0<br>创建 linear LV 和 mapper | status 0<br>创建 linear LV 和 mapper | host `LV_CREATE_LINEAR`<br>guest `LV_CREATE_LINEAR` | 已对齐 |
| `lvs -a -o vg_name,lv_name,lv_size,`<br>`seg_count,devices <VG>`<br>`lvs --segments -o lv_name,seg_start,`<br>`seg_size,segtype,devices <VG>/<LINEAR_LV>` | status 0<br>segtype 为 linear | status 0<br>segtype 为 linear | host `LVS_LINEAR_INITIAL`<br>guest `LVS_LINEAR_INITIAL` | 已对齐 |
| `dmsetup table <LINEAR_MAPPER>`<br>`dmsetup status <LINEAR_MAPPER>`<br>`dmsetup deps <LINEAR_MAPPER>`<br>linear LV active | status 0<br>table 为 linear<br>deps 匹配 PV | status 0<br>table 为 linear<br>deps 匹配 PV | host `LINEAR_INITIAL_DM_*`<br>guest `LINEAR_INITIAL_DM_*` | 已对齐 |
| `lvextend -L 64M`<br>`<VG>/<LINEAR_LV> <DISK1>`<br>linear LV 存在 | status 0<br>容量增长 | status 0<br>容量增长 | host `LV_EXTEND_LINEAR_SAME_PV`<br>guest `LV_EXTEND_LINEAR_SAME_PV` | 已对齐 |
| `lvextend -L 96M`<br>`<VG>/<LINEAR_LV> <DISK2>`<br>linear LV 存在 | status 0<br>形成跨 PV linear segments | status 0<br>形成跨 PV linear segments | host `LV_EXTEND_LINEAR_CROSS_PV`<br>guest `LV_EXTEND_LINEAR_CROSS_PV` | 已对齐 |
| `lvreduce -y -L 32M`<br>`<VG>/<LINEAR_LV>`<br>linear LV 已扩容 | status 0<br>容量收缩<br>LV 保留 | status 0<br>容量收缩<br>LV 保留 | host `LV_REDUCE_LINEAR`<br>guest `LV_REDUCE_LINEAR` | 已对齐 |

### striped LV

| 命令 / 前置条件 | 标准 Linux 用户可见行为 | 当前 Asterinas 情况 | 依据 | 当前结论 |
| --- | --- | --- | --- | --- |
| `lvcreate --type striped -i 2 -I 64K -L 32M`<br>`-n <STRIPED_LV> <VG> <DISK1> <DISK2>` | status 0<br>创建 striped LV | status 0<br>创建 striped LV | host `LV_CREATE_STRIPED`<br>guest `LV_CREATE_STRIPED` | 已对齐 |
| `lvs --segments -o lv_name,seg_start,`<br>`seg_size,segtype,stripes,stripesize,devices`<br>`<VG>/<STRIPED_LV>` | status 0<br>segtype striped<br>stripes=2 | status 0<br>segtype striped<br>stripes=2 | host `LVS_SEGMENTS_STRIPED_INITIAL`<br>guest `LVS_SEGMENTS_STRIPED_INITIAL` | 已对齐 |
| `dmsetup table <STRIPED_MAPPER>`<br>`dmsetup status <STRIPED_MAPPER>`<br>`dmsetup deps <STRIPED_MAPPER>`<br>striped LV active | status 0<br>table/status 为 striped<br>deps=2 | status 0<br>table/status 为 striped<br>deps=2 | host `STRIPED_INITIAL_DM_*`<br>guest `STRIPED_INITIAL_DM_*` | 已对齐 |
| `lvextend -i 2 -I 64K -L 64M`<br>`<VG>/<STRIPED_LV> <DISK1> <DISK2>`<br>striped LV 存在 | status 0<br>容量增长<br>仍为 striped | status 0<br>容量增长<br>仍为 striped | host `LV_EXTEND_STRIPED_SAME_SET`<br>guest `LV_EXTEND_STRIPED_SAME_SET` | 已对齐 |
| `lvextend -i 2 -I 64K -L 96M`<br>`<VG>/<STRIPED_LV> <DISK3> <DISK4>`<br>striped LV 存在 | status 0<br>形成跨 segment striped LV | status 0<br>形成跨 segment striped LV | host `LV_EXTEND_STRIPED_CROSS_SET`<br>guest `LV_EXTEND_STRIPED_CROSS_SET` | 已对齐 |
| `lvreduce -y -L 32M`<br>`<VG>/<STRIPED_LV>`<br>striped LV 已扩容 | status 0<br>容量收缩<br>LV 保留 | status 0<br>容量收缩<br>LV 保留 | host `LV_REDUCE_STRIPED`<br>guest `LV_REDUCE_STRIPED` | 已对齐 |

### mixed / scan / activation / remove

| 命令 / 前置条件 | 标准 Linux 用户可见行为 | 当前 Asterinas 情况 | 依据 | 当前结论 |
| --- | --- | --- | --- | --- |
| `lvcreate --type linear -L 32M`<br>`-n <MIXED_LV> <VG> <DISK4>`<br>`lvextend --type striped -i 2 -I 64K -L 64M`<br>`<VG>/<MIXED_LV> <DISK1> <DISK2>` | status 0<br>LV 同时含 linear/striped segments | status 0<br>LV 同时含 linear/striped segments | host `LV_CREATE_MIXED_LINEAR` / `LV_EXTEND_MIXED_STRIPED`<br>guest `LV_CREATE_MIXED_LINEAR` / `LV_EXTEND_MIXED_STRIPED` | 已对齐 |
| `vgchange -an <VG>`<br>测试 VG active | status 0<br>deactivate 测试 VG | status 0<br>deactivate 测试 VG | host `VGCHANGE_INACTIVE`<br>guest `VGCHANGE_INACTIVE` | 已对齐 |
| `vgscan --mknodes`<br>测试 VG inactive 后扫描 | status 0<br>扫描并准备恢复节点 | status 0<br>扫描并准备恢复节点 | host `VGSCAN_MKNODES`<br>guest `VGSCAN_MKNODES` | 已对齐 |
| `vgchange -ay <VG>`<br>测试 VG 可扫描 | status 0<br>reactivate 测试 VG | status 0<br>reactivate 测试 VG | host `VGCHANGE_ACTIVE`<br>guest `VGCHANGE_ACTIVE` | 已对齐 |
| `lvremove -y <VG>/<MIXED_LV>`<br>`lvremove -y <VG>/<STRIPED_LV>`<br>`lvremove -y <VG>/<LINEAR_LV>` | status 0<br>测试 LV 消失 | status 0<br>测试 LV 消失 | host `LVREMOVE_*`<br>guest `LVREMOVE_*` | 已对齐 |
| `vgremove -y <VG>`<br>VG 为本次测试对象且无 LV | status 0<br>测试 VG 消失 | status 0<br>测试 VG 消失 | host `VGREMOVE_TEST`<br>guest `VGREMOVE_TEST` | 已对齐 |
| `pvremove -ff -y`<br>`<DISK1> <DISK2> <DISK3> <DISK4>` | status 0<br>测试 PV label 清除 | status 0<br>测试 PV label 清除 | host `PVREMOVE_TEST`<br>guest `PVREMOVE_TEST` | 已对齐 |

当前阶段明确不纳入：thin、snapshot、cache、mirror、raid、crypt 等 target 族相关 LVM2 命令，`lvconvert`、`lvrename`、`vgrename`、`pvmove`，以及完整 udev/systemd 自动激活语义。
