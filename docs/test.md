# Device Mapper 编译与测试命令整理

本文整理 `dm` 分支中 Asterinas 编译、静态检查、ktest、NixOS/LVM2 系统验收，以及脚本中实际使用的 `dmsetup`、PV/VG/LV、文件系统和数据校验命令。

目标是说明：

- 如何编译 Asterinas。
- 如何执行 DM 相关 ktest 和系统验收。
- 每类命令验证了什么功能。
- linear、striped、mixed 等 target 分别怎么测。

## 1. 基本执行环境

仓库路径：

```bash
/root/atom/asterinas
```

容器内路径：

```bash
/root/asterinas
```

构建、ktest、NixOS/LVM2 系统验收优先在容器 `myAsterinas` 内执行：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && <command>'
```

QEMU、ktest、NixOS 系统测试必须串行运行，避免共享镜像、测试盘或 `test/initramfs/build/ext2.img` 锁冲突。

## 2. 编译 Asterinas

### 2.1 静态检查

常用静态检查命令：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && cargo fmt --check'
git diff --check
git diff -- Cargo.toml
git status --short
```

用途：

- `cargo fmt --check`：检查 Rust 格式。
- `git diff --check`：检查 trailing whitespace、空白错误等。
- `git diff -- Cargo.toml`：确认定向 ktest 临时修改的 `default-members` 已恢复。
- `git status --short`：确认工作区状态。

如果需要覆盖 workspace 格式检查，可使用：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && cargo fmt --all --check'
```

### 2.2 编译 kernel

直接编译 kernel：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && make kernel'
```

该入口会构建 initramfs，并通过 `cargo osdk build` 构建内核。

### 2.3 构建 NixOS image

DM / LVM2 系统验收依赖 NixOS guest image。若内核或 NixOS image 相关内容变更，先执行：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && make nixos'
```

系统测试脚本会检查：

```bash
target/nixos/asterinas.img
```

如果该镜像不存在，测试 harness 会提示先运行 `make nixos`。

### 2.4 清理 DM 测试盘

LVM2 会把 PV/VG/LV 元数据写入测试盘。重复跑系统验收前，如需回到干净状态，可执行：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && make rm_dm'
```

默认清理：

```text
target/nixos/test.img
target/nixos/test[0-9]*.img
```

也可以显式指定：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && DM_TEST_IMAGES="target/nixos/test.img target/nixos/test2.img target/nixos/test3.img" make rm_dm'
```

## 3. ktest 命令

### 3.1 通用 ktest 入口

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && make ktest'
```

该入口会构建 initramfs，并通过 `cargo osdk test` 执行 kernel-mode tests。

### 3.2 定向跑 `aster-device-mapper` crate ktest

临时把根 [Cargo.toml](../Cargo.toml) 的 `default-members` 缩小为：

```toml
default-members = [
    "kernel/comps/device-mapper",
]
```

然后执行：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && timeout -k 10s 180s make ktest CARGO_OSDK_TEST_ARGS="--kcmd-args=loglevel=error --kcmd-args=earlycon --kcmd-args=console=ttyS0 --boot-method=grub-rescue-iso --grub-boot-protocol=multiboot2 aster_device_mapper::table::tests::<test_name>"'
```

用途：

- 验证 `aster-device-mapper` crate 内部 table 解析。
- 验证 linear / striped target 参数。
- 验证 BIO remap、split、completion 聚合。
- 验证 flush fan-out、deps 去重等 DM core 数据面逻辑。

示例测试方向：

```text
aster_device_mapper::table::tests::<test_name>
```

常用于测试：

- 单段 linear remap。
- 多段 linear target。
- striped chunk split。
- BIO 恰好结束在 target 末尾。
- linear + striped mixed table 跨 target split。
- child enqueue 失败或 child 完成 IoError 时的聚合返回。

测试完成后必须恢复 [Cargo.toml](../Cargo.toml)，并确认：

```bash
git diff -- Cargo.toml
```

无输出。

### 3.3 定向跑 `aster-kernel` ioctl 层 ktest

临时把根 [Cargo.toml](../Cargo.toml) 的 `default-members` 缩小为：

```toml
default-members = [
    "kernel",
]
```

然后执行：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && timeout -k 10s 180s make ktest CARGO_OSDK_TEST_ARGS="--kcmd-args=loglevel=error --kcmd-args=earlycon --kcmd-args=console=ttyS0 --boot-method=grub-rescue-iso --grub-boot-protocol=multiboot2 aster_kernel::device::misc::device_mapper::tests::<test_name>"'
```

用途：

- 验证 `/dev/mapper/control` ioctl 层。
- 验证 `DM_TABLE_LOAD`。
- 验证 active / inactive table。
- 验证 `DM_TABLE_STATUS`、`DM_TABLE_DEPS`。
- 验证 suspend / resume 切换。
- 验证非法 table-load 输入。

测试完成后同样必须恢复 [Cargo.toml](../Cargo.toml)，并确认：

```bash
git diff -- Cargo.toml
```

无输出。

## 4. 系统验收入口

统一入口：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh <suite>'
```

可用 suite：

```bash
myshell/run_dm_system_tests.sh --quick
myshell/run_dm_system_tests.sh --data
myshell/run_dm_system_tests.sh --striped
myshell/run_dm_system_tests.sh --striped-lvm2
myshell/run_dm_system_tests.sh --striped-lvm2-3pv
myshell/run_dm_system_tests.sh --striped-lvm2-multi-segment
myshell/run_dm_system_tests.sh --mixed-lvm2
myshell/run_dm_system_tests.sh --lvm2
myshell/run_dm_system_tests.sh --linear-flow
myshell/run_dm_system_tests.sh --full
```

### 4.1 suite 与功能对应关系

| suite | 子脚本 | 主要验证功能 |
|---|---|---|
| `--quick` | [run_control_abi_test.sh](../myshell/dm_linear/run_control_abi_test.sh)、[run_cross_target_bio_regression.sh](../myshell/dm_linear/run_cross_target_bio_regression.sh) | linear 控制面 smoke、raw cross-target BIO 回归。 |
| `--data` | [run_cross_target_bio_regression.sh](../myshell/dm_linear/run_cross_target_bio_regression.sh) | 只验证 raw cross-target BIO split/remap。 |
| `--striped` | [run_raw_striped_bio_test.sh](../myshell/dm_striped/run_raw_striped_bio_test.sh) | raw `dmsetup striped` BIO split/remap 和 backing 分布。 |
| `--striped-lvm2` | [run_lvm2_striped_io_reboot_test.sh](../myshell/dm_striped/run_lvm2_striped_io_reboot_test.sh) | 2PV / 2-way LVM2 striped create、I/O、grow、shrink、reboot recovery。 |
| `--striped-lvm2-3pv` | [run_lvm2_striped_3pv_reboot_test.sh](../myshell/dm_striped/run_lvm2_striped_3pv_reboot_test.sh) | 3PV / 3-way striped create、I/O、reboot recovery。 |
| `--striped-lvm2-multi-segment` | [run_lvm2_striped_multi_segment_reboot_test.sh](../myshell/dm_striped/run_lvm2_striped_multi_segment_reboot_test.sh) | 多段 striped table、ext2 I/O、reboot recovery。 |
| `--mixed-lvm2` | [run_lvm2_linear_striped_mixed_reboot_test.sh](../myshell/dm_mixed/run_lvm2_linear_striped_mixed_reboot_test.sh) | 同一 LV 内 linear + striped mixed table、ext2 I/O、reboot recovery。 |
| `--lvm2` | [run_cross_pv_large_write_test.sh](../myshell/dm_linear/run_cross_pv_large_write_test.sh)、[run_lvm2_resize_test.sh](../myshell/dm_linear/run_lvm2_resize_test.sh) | linear LVM2 cross-PV 大文件写入和 resize。 |
| `--linear-flow` | [run_linear_full_flow_test.sh](../myshell/dm_linear/run_linear_full_flow_test.sh) | 单 guest linear 端到端流程。 |
| `--full` | 多个 linear 子脚本 | 当前 linear 全量系统回归；不默认包含 striped / mixed 慢测试。 |

### 4.2 测试盘和 guest 启动

系统验收脚本通过 NixOS guest 运行真实用户态工具。测试盘由 [tools/nixos/run.sh](../tools/nixos/run.sh) 挂入 QEMU。

单盘或双盘旧变量：

```bash
DM_TEST_IMAGE=target/nixos/test.img
DM_TEST_IMAGE_2=target/nixos/test2.img
```

多盘变量：

```bash
DM_TEST_IMAGES="target/nixos/test.img target/nixos/test2.img target/nixos/test3.img"
```

如果测试盘不存在，会创建 512 MiB raw image：

```bash
fallocate -l 512M "${image_path}"
```

每块测试盘设置稳定 VirtIO serial：

```text
vdmtest
vdmtest2
vdmtest3
```

guest 内通过 locator 定位：

```bash
aster-dm-disk-locator
aster-dm-disk-locator vdmtest2
aster-dm-disk-locator vdmtest3
```

这样避免硬编码 `/dev/vda`、`/dev/vdb`、`/dev/vdc`，防止多盘枚举顺序变化导致误测。

## 5. `dmsetup` 命令整理

### 5.1 `dmsetup version`

```bash
dmsetup version
```

验证功能：

- `/dev/mapper/control` 可打开。
- DM version ioctl 可用。
- libdevmapper 能和内核 DM 控制面通信。

### 5.2 `dmsetup targets`

```bash
dmsetup targets | tee /tmp/dm-targets.txt
grep -q '^linear' /tmp/dm-targets.txt
grep -q '^striped' /tmp/dm-targets.txt
```

验证功能：

- target registry 正确暴露 `linear`。
- striped 测试中确认暴露 `striped`。
- mixed 测试中同时确认 `linear` 和 `striped` 都可见。

### 5.3 `dmsetup create` 创建 linear mapper

单 target linear：

```bash
printf '0 8 linear %s 0\n' "$DEV" | dmsetup create dm_control_abi
```

多 target linear：

```bash
printf '0 4 linear %s 0\n4 4 linear %s 0\n' "$DEV1" "$DEV2" | dmsetup create dm_linear_full_raw
```

验证功能：

- `DM_DEV_CREATE`。
- table load。
- resume 激活。
- linear target remap。
- 多 target table 连续性。
- BIO 跨 target 边界拆分。

### 5.4 `dmsetup create` 创建 striped mapper

```bash
printf '0 16 striped 2 4 %s 0 %s 0\n' "$DEV1" "$DEV2" | dmsetup create dm_striped_raw
```

参数含义：

```text
0       logical start sector
16      mapper length in sectors
striped target type
2       stripe count
4       chunk size in sectors
DEV1 0  第一个 backing device 和起始 sector
DEV2 0  第二个 backing device 和起始 sector
```

验证功能：

- striped target 参数解析。
- stripe count。
- chunk size。
- 多 backing deps。
- BIO 按 stripe chunk 拆分到不同 backing。

### 5.5 readonly mapper

```bash
printf '0 8 linear %s 0\n' "$DEV" | dmsetup --readonly create dm_control_readonly
```

读测试：

```bash
dd if=/dev/mapper/dm_control_readonly of=/tmp/control-readonly-read.bin bs=512 count=1 status=none
```

写失败测试：

```bash
if dd if=/tmp/control-readonly-write.bin of=/dev/mapper/dm_control_readonly bs=512 count=1 conv=fsync status=none; then
    echo TEST_FAIL_DM_CONTROL_ABI readonly_write_succeeded
    exit 1
fi
```

验证功能：

- readonly flag 生效。
- 只读 mapper 允许读。
- 只读 mapper 拒绝写。

### 5.6 `dmsetup table`

```bash
dmsetup table <mapper> | tee /tmp/table.txt
```

验证功能：

- table 输出格式。
- target type。
- logical start。
- target length。
- backing major:minor。
- striped 参数。
- LVM2 resize / reboot recovery 后 table 是否保持预期。

mixed table 示例：

```text
0 524288 linear 253:64 2048
524288 524288 striped 2 8 253:80 2048 253:96 2048
```

含义：

- `0 524288 linear 253:64 2048`：第一段 linear，映射到 PV1。
- `524288 524288 striped 2 8 ...`：第二段 striped，2-way，chunk size 为 8 sectors，即 4 KiB。

### 5.7 `dmsetup status`

```bash
dmsetup status <mapper> | tee /tmp/status.txt
```

验证功能：

- mapper 状态可查询。
- status 中 target type 与 table 一致。
- mixed table 中同时能看到 linear 和 striped segment。

### 5.8 `dmsetup deps`

```bash
dmsetup deps <mapper> | tee /tmp/deps.txt
```

验证功能：

- backing dependencies 数量正确。
- linear 单 backing：`1 dependencies`。
- 2-way striped 或双 PV linear：`2 dependencies`。
- 3PV striped / multi-segment / mixed：`3 dependencies`。
- deps 中包含预期 backing major:minor。

### 5.9 `dmsetup info`

```bash
dmsetup info <mapper> | tee /tmp/info.txt
```

验证功能：

- mapper name。
- active 状态。
- suspended 状态。
- open count / event 信息。
- busy remove 后 mapper 是否仍存在。

### 5.10 `dmsetup rename`

```bash
dmsetup rename dm_control_abi dm_control_renamed
```

后续检查：

```bash
dmsetup info dm_control_renamed
dmsetup table dm_control_renamed
if dmsetup info dm_control_abi >/dev/null 2>&1; then
    echo TEST_FAIL_DM_CONTROL_ABI old_name_still_exists
    exit 1
fi
```

验证功能：

- rename 后新名字可用。
- 旧名字不可用。
- rename 不破坏 table/status。
- `/dev/mapper/<name>` runtime node 维护正确。

### 5.11 `dmsetup suspend` / `dmsetup resume`

```bash
dmsetup suspend --noflush dm_control_renamed
dmsetup info dm_control_renamed | tee /tmp/control-info-suspended.txt

dmsetup resume --noflush dm_control_renamed
dmsetup info dm_control_renamed | tee /tmp/control-info-live.txt
```

验证功能：

- suspend 状态切换。
- resume 状态切换。
- `--noflush` flag 兼容。
- active table 继续可用。

### 5.12 `dmsetup wait`

```bash
timeout 10 dmsetup wait --noflush dm_control_renamed 0
```

验证功能：

- `DM_DEV_WAIT` 最小语义。
- event number 等待路径。
- `--noflush` flag 兼容。

### 5.13 `dmsetup ls`

```bash
dmsetup ls | tee /tmp/control-ls.txt
```

验证功能：

- 多个 mapper 可以同时枚举。
- renamed mapper 和 multi mapper 均可见。

### 5.14 busy remove 和 `dmsetup remove_all`

构造 busy mapper：

```bash
printf '0 8 linear %s 0\n' "$DEV" | dmsetup create dm_control_busy
exec 9< /dev/mapper/dm_control_busy
```

busy remove 应失败：

```bash
if dmsetup remove dm_control_busy >/tmp/control-busy-remove.txt 2>&1; then
    echo TEST_FAIL_DM_CONTROL_ABI busy_remove_succeeded
    exit 1
fi
```

remove_all：

```bash
dmsetup remove_all || true
```

检查 busy mapper 仍存在：

```bash
dmsetup info dm_control_busy >/dev/null
```

释放 fd 后删除：

```bash
exec 9<&-
dmsetup remove dm_control_busy
```

验证功能：

- open 的 mapper 不能被 remove。
- `remove_all` 只删除 non-busy mapper。
- busy mapper 在引用释放后可删除。

### 5.15 `dmsetup load`

系统 shell 脚本中没有直接写显式：

```bash
dmsetup load
```

但 `dmsetup create`、LVM2 `lvcreate`、`lvextend`、`lvreduce` 底层都会通过 libdevmapper 走 table load / resume 相关 ioctl。

显式 `DM_TABLE_LOAD` 语义主要由 kernel ioctl ktest 覆盖，重点包括：

- load 只更新 inactive table。
- resume 后 inactive table 才成为 active table。
- failed table load 不污染 active/inactive table。
- `DM_TABLE_STATUS`、`DM_TABLE_DEPS` 对 active/inactive table 输出正确。
- 非法 `dm_target_spec.next`、缺少 NUL、unsupported target 等输入被拒绝。

## 6. PV / VG / LV 命令整理

所有 LVM2 系统测试都使用：

```bash
LVM_CONFIG='activation { udev_rules=0 }'
```

调用形式：

```bash
lvcreate --config "$LVM_CONFIG" ...
lvextend --config "$LVM_CONFIG" ...
lvreduce --config "$LVM_CONFIG" ...
vgchange --config "$LVM_CONFIG" ...
```

目的：

- 避免依赖完整 udev / systemd 自动联动。
- 直接测试 libdevmapper 和 Asterinas DM core。

### 6.1 PV 命令

创建 PV：

```bash
pvcreate "$TEST_DISK" "$TEST_DISK2"
pvcreate "$TEST_DISK" "$TEST_DISK2" "$TEST_DISK3"
pvcreate "$TEST_DISK3"
```

用途：

- 初始化测试盘为 LVM PV。
- 2PV linear / striped 使用两块盘。
- 3PV striped / mixed 使用三块盘。
- multi-segment 测试中后续单独加入第三块 PV。

扫描 PV：

```bash
pvscan
```

用途：

- reboot recovery 后重新发现 PV。

查看 PV：

```bash
pvs -o pv_name,pv_size,vg_name
```

用途：

- 输出 PV 名称、大小、所属 VG。
- 辅助确认测试盘被正确纳入 VG。

清理 PV：

```bash
pvremove -y "$TEST_DISK" "$TEST_DISK2"
```

用途：

- linear full flow cleanup 中清理 PV 元数据。

### 6.2 VG 命令

创建 VG：

```bash
vgcreate test_vg "$TEST_DISK" "$TEST_DISK2"
vgcreate large_vg "$TEST_DISK" "$TEST_DISK2"
vgcreate striped_vg "$TEST_DISK" "$TEST_DISK2"
vgcreate striped3_vg "$TEST_DISK" "$TEST_DISK2" "$TEST_DISK3"
vgcreate mixed_vg "$TEST_DISK"
```

用途：

- `test_vg`：linear resize。
- `large_vg`：linear cross-PV large write。
- `striped_vg`：2-way striped。
- `striped3_vg`：3-way striped。
- `mixed_vg`：先在 PV1 上创建 linear LV，后续扩展 PV2+PV3。

扩展 VG：

```bash
vgextend striped_ms_vg "$TEST_DISK3"
vgextend mixed_vg "$TEST_DISK2" "$TEST_DISK3"
```

用途：

- multi-segment striped：初始 PV1+PV2，扩容时加入 PV3。
- mixed：初始 PV1 linear，扩容时加入 PV2+PV3 生成 striped segment。

扫描 VG：

```bash
vgscan
vgscan --mknodes
```

用途：

- reboot recovery 后扫描 VG。
- `--mknodes` 让 LVM 补齐 mapper 节点，替代依赖完整 udev 自动创建。

激活 VG：

```bash
vgchange --config "$LVM_CONFIG" -ay <vg>
```

停用 VG：

```bash
vgchange --config "$LVM_CONFIG" -an <vg>
```

用途：

- 第一轮 guest 结束前停用 VG，确保 DM device 关闭、状态落盘。
- 第二轮 guest 启动后激活 VG，触发 LVM2 重新加载 DM table。

查看 VG：

```bash
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
```

用途：

- 输出 VG 容量、空闲空间、PV 数、LV 数。

删除 VG：

```bash
vgremove --config "$LVM_CONFIG" -y full_vg
```

用途：

- linear full flow cleanup。

### 6.3 LV 命令

创建 linear LV：

```bash
lvcreate --config "$LVM_CONFIG" --type linear -L 400M -n test_lv test_vg "$TEST_DISK"
lvcreate --config "$LVM_CONFIG" --type linear -L 900M -n large_lv large_vg "$TEST_DISK" "$TEST_DISK2"
lvcreate --config "$LVM_CONFIG" --type linear -L "${MIXED_INITIAL_LV_MIB}M" -n mixed_lv mixed_vg "$TEST_DISK"
```

用途：

- 400M linear LV：resize 测试。
- 900M linear LV：跨 PV large write。
- mixed 初始 LV：先创建 PV1 上的 linear segment。

创建 striped LV：

```bash
lvcreate --config "$LVM_CONFIG" --type striped -i 2 -I "${STRIPED_CHUNK_KIB}K" -L "${STRIPED_INITIAL_LV_MIB}M" -n striped_lv striped_vg "$TEST_DISK" "$TEST_DISK2"
```

创建 3-way striped LV：

```bash
lvcreate --config "$LVM_CONFIG" --type striped -i 3 -I "${STRIPED3_CHUNK_KIB}K" -L "${STRIPED3_LV_MIB}M" -n striped3_lv striped3_vg "$TEST_DISK" "$TEST_DISK2" "$TEST_DISK3"
```

参数含义：

- `--type striped`：让 LVM2 生成 striped DM target。
- `-i 2` / `-i 3`：stripe count。
- `-I <size>K`：stripe chunk size。

linear 扩容：

```bash
lvextend --config "$LVM_CONFIG" -L 700M test_vg/test_lv "$TEST_DISK2"
```

用途：

- 把 linear LV 扩到第二块 PV。
- 触发 active table reload/resume。
- 验证多 target linear table。

striped 扩容：

```bash
lvextend --config "$LVM_CONFIG" -i 2 -I "${STRIPED_CHUNK_KIB}K" -L "${STRIPED_EXTENDED_LV_MIB}M" striped_vg/striped_lv "$TEST_DISK" "$TEST_DISK2"
```

multi-segment striped 扩容：

```bash
lvextend --config "$LVM_CONFIG" -i 2 -I "${STRIPED_MS_CHUNK_KIB}K" -L "${STRIPED_MS_EXTENDED_LV_MIB}M" striped_ms_vg/striped_ms_lv "$TEST_DISK2" "$TEST_DISK3"
```

用途：

- 初始 segment 使用 PV1+PV2。
- 扩容 segment 使用 PV2+PV3。
- 验证 multi-segment striped table 和 deps 去重。

mixed 扩容：

```bash
lvextend --config "$LVM_CONFIG" --type striped -i 2 -I "${MIXED_STRIPED_CHUNK_KIB}K" -L "${MIXED_EXTENDED_LV_MIB}M" mixed_vg/mixed_lv "$TEST_DISK2" "$TEST_DISK3"
```

用途：

- 在已有 linear LV 后追加 striped segment。
- 生成同一个 LV 内的 linear + striped mixed table。

缩容 linear LV：

```bash
resize2fs "$MAPPER_DEVICE" 300M
lvreduce --config "$LVM_CONFIG" -y -L 300M test_vg/test_lv
```

缩容 striped LV：

```bash
resize2fs "$MAPPER_DEVICE" "${STRIPED_SHRUNK_LV_MIB}M"
lvreduce --config "$LVM_CONFIG" -y -L "${STRIPED_SHRUNK_LV_MIB}M" striped_vg/striped_lv
```

注意：shrink 顺序是先 shrink 文件系统，再 shrink LV。

查看 LV：

```bash
lvs -a -o lv_name,vg_name,lv_size,lv_attr,devices
lvs -a -o lv_name,lv_size,seg_count,devices <vg>
lvs --segments -o lv_name,seg_start,seg_size,devices <vg>/<lv>
lvs --segments -o lv_name,seg_start,seg_size,stripes,stripesize,devices <vg>/<lv>
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices <vg>/<lv>
```

用途：

- 查看 LV 大小和属性。
- 查看 segment 布局。
- 查看 segment type。
- 查看 stripe count / stripe size。
- 查看每段使用哪些 PV。

删除 LV：

```bash
lvremove --config "$LVM_CONFIG" -y full_vg/full_lv
```

用途：

- linear full flow cleanup。

## 7. 文件系统和数据校验命令

### 7.1 创建 ext2

```bash
mkfs.ext2 -F -b 4096 "$MAPPER_DEVICE"
```

用途：

- 在 `/dev/mapper/<vg-lv>` 上创建 ext2。
- `-F` 强制格式化 block device。
- `-b 4096` 使用 4 KiB block size。

### 7.2 检查 block device 文件系统标识

```bash
blkid "$MAPPER_DEVICE"
```

用途：

- 确认 mapper 上存在 ext2 文件系统。
- 输出测试日志，便于定位失败。

### 7.3 挂载和卸载

读写挂载：

```bash
mount -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"
```

只读恢复挂载：

```bash
mount -o ro -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"
```

卸载：

```bash
umount "$MOUNT_DIR"
```

用途：

- 首轮 guest 中读写挂载并写入文件。
- reboot 后只读挂载并校验数据。
- offline resize 前必须卸载。

### 7.4 ext2 检查和 resize

扩容文件系统：

```bash
e2fsck -f -y "$MAPPER_DEVICE"
resize2fs "$MAPPER_DEVICE"
e2fsck -f -y "$MAPPER_DEVICE"
```

缩容文件系统：

```bash
e2fsck -f -y "$MAPPER_DEVICE"
resize2fs "$MAPPER_DEVICE" 300M
e2fsck -f -y "$MAPPER_DEVICE"
```

用途：

- LV grow 后扩容 ext2。
- LV shrink 前先缩小 ext2。
- 前后 `e2fsck` 确认文件系统一致性。

### 7.5 `dd` 写入和读回

清零测试盘：

```bash
dd if=/dev/zero of="$TEST_DISK" bs=4096 count=1 conv=fsync status=none
dd if=/dev/zero of="$TEST_DISK2" bs=4096 count=1 conv=fsync status=none
```

raw mapper 写入：

```bash
dd if=/tmp/cross-pattern.bin of=/dev/mapper/cross_bio_test bs=4096 count=1 conv=fsync status=none
dd if=/tmp/striped-payload.bin of=/dev/mapper/dm_striped_raw bs=8192 count=1 conv=fsync status=none
```

raw mapper 读回：

```bash
dd if=/dev/mapper/cross_bio_test of=/tmp/cross-readback.bin bs=4096 count=1 status=none
dd if=/dev/mapper/dm_striped_raw of=/tmp/striped-readback.bin bs=8192 count=1 status=none
```

读取 backing：

```bash
dd if="$TEST_DISK" of=/tmp/cross-first.bin bs=2048 count=1 status=none
dd if="$TEST_DISK2" of=/tmp/cross-second.bin bs=2048 count=1 status=none
```

LVM2 文件写入：

```bash
dd if=/dev/urandom of="$MOUNT_DIR/base64.bin" bs=1M count=64 conv=fsync status=none
dd if=/dev/urandom of="$MOUNT_DIR/grow256.bin" bs=1M count=256 conv=fsync status=none
```

用途：

- raw BIO 测试中精确制造跨 target / 跨 stripe chunk I/O。
- LVM2 测试中制造真实文件 I/O。
- `conv=fsync` 确保写入同步落盘。

### 7.6 `md5sum` 数据校验

生成校验文件：

```bash
md5sum file700.bin > file700.md5
md5sum "base${SIZE}.bin" base-marker.txt > mixed.md5
md5sum "grow${SIZE}.bin" grow-marker.txt >> mixed.md5
```

验证：

```bash
md5sum -c file700.md5
md5sum -c mixed.md5
```

raw readback 对比：

```bash
md5sum /tmp/cross-pattern.bin /tmp/cross-readback.bin > /tmp/cross-readback.md5
md5sum /tmp/striped-payload.bin /tmp/striped-readback.bin > /tmp/striped-readback.md5
```

用途：

- 验证 mapper 读回数据等于原始 payload。
- 验证 backing 分布符合预期。
- 验证 reboot 后文件数据未损坏。

### 7.7 `sync`、`df`、`du`

```bash
sync
df -h "$MOUNT_DIR"
du -sh "$MOUNT_DIR" "$MOUNT_DIR/base64.bin" "$MOUNT_DIR/grow256.bin"
```

用途：

- `sync`：guest 关机前同步文件数据和 LVM 元数据。
- `df`：确认文件系统容量。
- `du`：确认测试文件占用。

## 8. 按 target / 功能分类的测试方法

### 8.1 linear 控制面

入口：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --quick'
```

关键命令：

```bash
dmsetup version
dmsetup targets
printf '0 8 linear %s 0\n' "$DEV" | dmsetup create dm_control_abi
dmsetup table dm_control_abi
dmsetup status dm_control_abi
dmsetup deps dm_control_abi
dmsetup info dm_control_abi
dmsetup rename dm_control_abi dm_control_renamed
dmsetup suspend --noflush dm_control_renamed
dmsetup resume --noflush dm_control_renamed
dmsetup remove dm_control_renamed
```

验证功能：

- control 设备可用。
- linear target 暴露。
- create / table / status / deps / info。
- rename。
- suspend / resume。
- remove。
- readonly。
- busy remove。
- remove_all。

### 8.2 linear raw cross-target BIO

入口：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --data'
```

核心 table：

```bash
printf '0 4 linear %s 0\n4 4 linear %s 0\n' "$DEV1" "$DEV2" | dmsetup create cross_bio_test
```

测试方法：

- mapper 总长 8 sectors。
- 第一个 target 覆盖 `0..4` sectors。
- 第二个 target 覆盖 `4..8` sectors。
- 写入一个 4 KiB BIO：

```bash
dd if=/tmp/cross-pattern.bin of=/dev/mapper/cross_bio_test bs=4096 count=1 conv=fsync status=none
```

该 BIO 跨过 2 KiB target 边界。

校验：

```bash
dd if=/dev/mapper/cross_bio_test of=/tmp/cross-readback.bin bs=4096 count=1 status=none
dd if="$TEST_DISK" of=/tmp/cross-first.bin bs=2048 count=1 status=none
dd if="$TEST_DISK2" of=/tmp/cross-second.bin bs=2048 count=1 status=none
md5sum ...
```

验证功能：

- 一个 BIO 跨 table target 边界时，会拆成两个 child BIO。
- child BIO 分别 remap 到不同 backing。
- 原始 BIO 最终完成状态聚合正确。
- mapper 读回与 backing 分布都正确。

### 8.3 linear LVM2 resize

入口：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --lvm2'
```

关键命令：

```bash
pvcreate "$TEST_DISK" "$TEST_DISK2"
vgcreate test_vg "$TEST_DISK" "$TEST_DISK2"
lvcreate --config "$LVM_CONFIG" --type linear -L 400M -n test_lv test_vg "$TEST_DISK"
dmsetup table test_vg-test_lv
mkfs.ext2 -F -b 4096 "$MAPPER_DEVICE"
mount -t ext2 "$MAPPER_DEVICE" /mnt/dmtest
lvextend --config "$LVM_CONFIG" -L 700M test_vg/test_lv "$TEST_DISK2"
dmsetup table test_vg-test_lv
umount /mnt/dmtest
e2fsck -f -y "$MAPPER_DEVICE"
resize2fs "$MAPPER_DEVICE"
e2fsck -f -y "$MAPPER_DEVICE"
resize2fs "$MAPPER_DEVICE" 300M
lvreduce --config "$LVM_CONFIG" -y -L 300M test_vg/test_lv
dmsetup table test_vg-test_lv
vgchange --config "$LVM_CONFIG" -an test_vg
```

reboot recovery：

```bash
pvscan
vgscan
vgchange --config "$LVM_CONFIG" -ay test_vg
dmsetup table test_vg-test_lv
mount -t ext2 -o ro /dev/mapper/test_vg-test_lv /mnt/dmtest
```

验证功能：

- LVM2 创建 linear LV。
- linear LV 扩容后生成多 target table。
- ext2 grow 后数据仍正确。
- shrink 顺序正确：先 shrink ext2，再 shrink LV。
- reboot 后 PV/VG/LV 可恢复。

### 8.4 linear cross-PV large write

入口：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --lvm2'
```

关键命令：

```bash
pvcreate "$TEST_DISK" "$TEST_DISK2"
vgcreate large_vg "$TEST_DISK" "$TEST_DISK2"
lvcreate --config "$LVM_CONFIG" --type linear -L 900M -n large_lv large_vg "$TEST_DISK" "$TEST_DISK2"
dmsetup table large_vg-large_lv
mkfs.ext2 -F -b 4096 "$MAPPER_DEVICE"
mount -t ext2 "$MAPPER_DEVICE" /mnt/dmlarge
dd if=/dev/urandom of=/mnt/dmlarge/file700.bin bs=1M count="$LARGE_WRITE_MIB" conv=fsync
md5sum file700.bin > file700.md5
vgchange --config "$LVM_CONFIG" -an large_vg
```

reboot recovery：

```bash
vgscan --mknodes
vgchange --config "$LVM_CONFIG" -ay large_vg
dmsetup table large_vg-large_lv
mount -o ro -t ext2 "$MAPPER_DEVICE" /mnt/dmlarge
md5sum -c file700.md5
```

验证功能：

- linear LV 跨 PV。
- 大文件写入跨越多个 backing 区域。
- reboot 后 table 和文件数据恢复正确。

### 8.5 striped raw BIO

入口：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --striped'
```

核心 table：

```bash
printf '0 16 striped 2 4 %s 0 %s 0\n' "$DEV1" "$DEV2" | dmsetup create dm_striped_raw
```

测试方法：

- 构造 8 KiB payload。
- striped 参数为 2-way，chunk size 4 sectors，即 2 KiB。
- payload 分成 4 个 2 KiB chunk：A、B、C、D。
- 写入 mapper：

```bash
dd if=/tmp/striped-payload.bin of=/dev/mapper/dm_striped_raw bs=8192 count=1 conv=fsync status=none
```

校验：

```bash
dd if=/dev/mapper/dm_striped_raw of=/tmp/striped-readback.bin bs=8192 count=1 status=none
dd if="$TEST_DISK" of=/tmp/striped-first.bin bs=4096 count=1 status=none
dd if="$TEST_DISK2" of=/tmp/striped-second.bin bs=4096 count=1 status=none
```

预期：

```text
backing1 = A + C
backing2 = B + D
```

验证功能：

- striped target 参数解析。
- BIO 按 chunk 拆分。
- chunk 按 stripe 顺序落到不同 backing。
- mapper 读回数据和 backing 分布均正确。

### 8.6 2-way striped LVM2

入口：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --striped-lvm2'
```

创建：

```bash
pvcreate "$TEST_DISK" "$TEST_DISK2"
vgcreate striped_vg "$TEST_DISK" "$TEST_DISK2"
lvcreate --config "$LVM_CONFIG" --type striped -i 2 -I "${STRIPED_CHUNK_KIB}K" -L "${STRIPED_INITIAL_LV_MIB}M" -n striped_lv striped_vg "$TEST_DISK" "$TEST_DISK2"
dmsetup table striped_vg-striped_lv
dmsetup status striped_vg-striped_lv
dmsetup deps striped_vg-striped_lv
```

文件系统和写入：

```bash
mkfs.ext2 -F -b 4096 "$MAPPER_DEVICE"
mount -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"
dd if=/dev/urandom of="$MOUNT_DIR/base${STRIPED_BASE_FILE_MIB}.bin" bs=1M count="$STRIPED_BASE_FILE_MIB" conv=fsync status=none
md5sum -c striped.md5
```

扩容：

```bash
lvextend --config "$LVM_CONFIG" -i 2 -I "${STRIPED_CHUNK_KIB}K" -L "${STRIPED_EXTENDED_LV_MIB}M" striped_vg/striped_lv "$TEST_DISK" "$TEST_DISK2"
e2fsck -f -y "$MAPPER_DEVICE"
resize2fs "$MAPPER_DEVICE"
e2fsck -f -y "$MAPPER_DEVICE"
```

缩容：

```bash
resize2fs "$MAPPER_DEVICE" "${STRIPED_SHRUNK_LV_MIB}M"
lvreduce --config "$LVM_CONFIG" -y -L "${STRIPED_SHRUNK_LV_MIB}M" striped_vg/striped_lv
```

reboot recovery：

```bash
vgchange --config "$LVM_CONFIG" -an striped_vg
pvscan
vgscan --mknodes
vgchange --config "$LVM_CONFIG" -ay striped_vg
mount -o ro -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"
md5sum -c striped.md5
```

验证功能：

- LVM2 真实生成 striped table。
- 2-way stripe count 正确。
- chunk size 正确。
- deps 为两个 backing。
- grow / shrink 后 table 正确。
- reboot 后 table/status/deps 和数据均恢复正确。

### 8.7 3PV / 3-way striped LVM2

入口：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --striped-lvm2-3pv'
```

关键命令：

```bash
pvcreate "$TEST_DISK" "$TEST_DISK2" "$TEST_DISK3"
vgcreate striped3_vg "$TEST_DISK" "$TEST_DISK2" "$TEST_DISK3"
lvcreate --config "$LVM_CONFIG" --type striped -i 3 -I "${STRIPED3_CHUNK_KIB}K" -L "${STRIPED3_LV_MIB}M" -n striped3_lv striped3_vg "$TEST_DISK" "$TEST_DISK2" "$TEST_DISK3"
dmsetup table striped3_vg-striped3_lv
dmsetup status striped3_vg-striped3_lv
dmsetup deps striped3_vg-striped3_lv
mkfs.ext2 -F -b 4096 "$MAPPER_DEVICE"
mount -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"
md5sum -c striped3.md5
```

reboot recovery：

```bash
vgchange --config "$LVM_CONFIG" -an striped3_vg
pvscan
vgscan --mknodes
vgchange --config "$LVM_CONFIG" -ay striped3_vg
dmsetup table striped3_vg-striped3_lv
mount -o ro -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"
md5sum -c striped3.md5
```

验证功能：

- stripe count > 2。
- 三个 backing deps。
- 3PV table/status/deps 正确。
- reboot 后 3-way striped LV 可恢复。

### 8.8 multi-segment striped LVM2

入口：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --striped-lvm2-multi-segment'
```

初始创建：

```bash
pvcreate "$TEST_DISK" "$TEST_DISK2"
vgcreate striped_ms_vg "$TEST_DISK" "$TEST_DISK2"
lvcreate --config "$LVM_CONFIG" --type striped -i 2 -I "${STRIPED_MS_CHUNK_KIB}K" -L "${STRIPED_MS_INITIAL_LV_MIB}M" -n striped_ms_lv striped_ms_vg "$TEST_DISK" "$TEST_DISK2"
```

扩成第二个 segment：

```bash
pvcreate "$TEST_DISK3"
vgextend striped_ms_vg "$TEST_DISK3"
lvextend --config "$LVM_CONFIG" -i 2 -I "${STRIPED_MS_CHUNK_KIB}K" -L "${STRIPED_MS_EXTENDED_LV_MIB}M" striped_ms_vg/striped_ms_lv "$TEST_DISK2" "$TEST_DISK3"
```

验证 table：

```bash
dmsetup table striped_ms_vg-striped_ms_lv
dmsetup status striped_ms_vg-striped_ms_lv
dmsetup deps striped_ms_vg-striped_ms_lv
```

预期：

- 初始 table 使用 PV1+PV2。
- 扩容后 table 至少两段。
- 第二段使用 PV2+PV3。
- deps 为 3 dependencies，且 backing 去重。
- 每段 logical start 连续。

文件系统和恢复：

```bash
mkfs.ext2 -F -b 4096 "$MAPPER_DEVICE"
mount -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"
md5sum -c striped-ms.md5
vgchange --config "$LVM_CONFIG" -an striped_ms_vg
pvscan
vgscan --mknodes
vgchange --config "$LVM_CONFIG" -ay striped_ms_vg
mount -o ro -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"
md5sum -c striped-ms.md5
```

验证功能：

- 多段 striped table。
- 不同 segment 使用不同 PV 组合。
- deps 去重。
- reboot recovery 后多段 table 保持正确。

### 8.9 mixed linear + striped LVM2

入口：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --mixed-lvm2'
```

初始 linear：

```bash
pvcreate "$TEST_DISK"
vgcreate mixed_vg "$TEST_DISK"
lvcreate --config "$LVM_CONFIG" --type linear -L "${MIXED_INITIAL_LV_MIB}M" -n mixed_lv mixed_vg "$TEST_DISK"
dmsetup table mixed_vg-mixed_lv
dmsetup status mixed_vg-mixed_lv
dmsetup deps mixed_vg-mixed_lv
```

扩展为 mixed table：

```bash
pvcreate "$TEST_DISK2" "$TEST_DISK3"
vgextend mixed_vg "$TEST_DISK2" "$TEST_DISK3"
lvextend --config "$LVM_CONFIG" --type striped -i 2 -I "${MIXED_STRIPED_CHUNK_KIB}K" -L "${MIXED_EXTENDED_LV_MIB}M" mixed_vg/mixed_lv "$TEST_DISK2" "$TEST_DISK3"
```

预期 table：

```text
0 524288 linear 253:64 2048
524288 524288 striped 2 8 253:80 2048 253:96 2048
```

文件系统和数据：

```bash
mkfs.ext2 -F -b 4096 "$MAPPER_DEVICE"
mount -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"
dd if=/dev/urandom of="$MOUNT_DIR/base${MIXED_BASE_FILE_MIB}.bin" bs=1M count="$MIXED_BASE_FILE_MIB" conv=fsync status=none
md5sum -c mixed.md5
umount "$MOUNT_DIR"

e2fsck -f -y "$MAPPER_DEVICE"
resize2fs "$MAPPER_DEVICE"
e2fsck -f -y "$MAPPER_DEVICE"
mount -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"
dd if=/dev/urandom of="$MOUNT_DIR/grow${MIXED_GROW_FILE_MIB}.bin" bs=1M count="$MIXED_GROW_FILE_MIB" conv=fsync status=none
md5sum -c mixed.md5
```

reboot recovery：

```bash
vgchange --config "$LVM_CONFIG" -an mixed_vg
pvscan
vgscan --mknodes
vgchange --config "$LVM_CONFIG" -ay mixed_vg
dmsetup table mixed_vg-mixed_lv
dmsetup status mixed_vg-mixed_lv
dmsetup deps mixed_vg-mixed_lv
mount -o ro -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"
md5sum -c mixed.md5
```

验证功能：

- 真实 LVM2 生成同一 LV 内 linear + striped mixed table。
- 初始 table 只有 linear。
- 扩容后 table 恰好包含 linear 和 striped 两段。
- BIO 可以跨 mixed target 边界拆分。
- deps 包含 PV1 / PV2 / PV3。
- reboot 后 table/status/deps 和文件 md5 正确。

## 9. 快速选择测试命令

只改 DM table / target 数据面：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && timeout -k 10s 180s make ktest CARGO_OSDK_TEST_ARGS="--kcmd-args=loglevel=error --kcmd-args=earlycon --kcmd-args=console=ttyS0 --boot-method=grub-rescue-iso --grub-boot-protocol=multiboot2 aster_device_mapper::table::tests::<test_name>"'
```

只改 ioctl 控制面：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && timeout -k 10s 180s make ktest CARGO_OSDK_TEST_ARGS="--kcmd-args=loglevel=error --kcmd-args=earlycon --kcmd-args=console=ttyS0 --boot-method=grub-rescue-iso --grub-boot-protocol=multiboot2 aster_kernel::device::misc::device_mapper::tests::<test_name>"'
```

只验证 linear control + cross-target BIO：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --quick'
```

只验证 striped raw BIO：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --striped'
```

验证 LVM2 linear resize 和 cross-PV large write：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --lvm2'
```

验证 2-way striped LVM2：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --striped-lvm2'
```

验证 3PV / 3-way striped：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --striped-lvm2-3pv'
```

验证 multi-segment striped：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --striped-lvm2-multi-segment'
```

验证 mixed linear + striped：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --mixed-lvm2'
```

阶段验收 linear 全量：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=300 myshell/run_dm_system_tests.sh --full'
```

## 10. 输出标记

系统脚本会用固定输出标记判断通过，例如：

```text
CHECK_PASS_...
TEST_PASS_...
HOST_PASS_...
```

失败常见标记：

```text
TEST_FAIL_...
HOST_FAIL_...
Kernel panic
panicked
Input/output error
No space left
```

ktest / QEMU 超过约 3 分钟没有命中目标测试或没有关键进展时，应优先怀疑：

- 命令过滤不正确。
- `Cargo.toml default-members` 没缩到目标 crate。
- 残留 QEMU 进程。
- 测试镜像锁冲突。
- NixOS image 或测试盘旧状态污染。

处理方向：

```bash
git diff -- Cargo.toml
pgrep -af qemu-system
make rm_dm
```

必要时终止自己启动的异常任务，并换更窄的测试入口。
