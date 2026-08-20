# Device Mapper 测试脚本使用汇总

本文档汇总当前 Device Mapper / LVM2 相关一键测试脚本的用途、运行方法、日志位置和通过标准。

所有自建 shell 脚本统一放在仓库根目录下的 `myshell/`，linear 自动化脚本位于 `myshell/dm_linear/`，striped 自动化脚本位于 `myshell/dm_striped/`：

```text
myshell/br.sh
myshell/run_dm_system_tests.sh
myshell/dm_linear/run_cross_target_bio_regression.sh
myshell/dm_linear/run_cross_pv_large_write_test.sh
myshell/dm_linear/run_lvm2_resize_test.sh
myshell/dm_linear/run_linear_full_flow_test.sh
myshell/dm_striped/run_raw_striped_bio_test.sh
myshell/dm_striped/run_lvm2_striped_io_reboot_test.sh
```

这些脚本都应该在 Asterinas 开发容器内的 `/root/asterinas` 目录运行。

---

## 1. 测试分层逻辑

当前 linear 测试按从小到大分三类分脚本，另有一个单 guest 完整流程脚本；striped 当前提供 raw BIO 验收脚本和 LVM2 create + 文件 I/O + reboot 恢复脚本：

```text
第一层：raw BIO 边界回归
  linear：验证单个 4 KiB BIO 跨两个 DM target 时能 split/remap/complete。
  striped：验证 2-way striped mapper 按 chunk split/remap，并检查 backing 数据分布。

第二层：LVM2 文件 I/O 与重启恢复
  linear：验证 ext2 + LVM2 + DM 多段 linear table 上的大文件读写和重启恢复。
  striped：验证 LVM2 生成 2-way striped table、ext2 文件读写和重启恢复。

第三层：完整 LVM2 扩缩容流程
  验证 LV 创建、active 扩容、resize2fs、缩容和重启恢复。

单 guest full flow：linear 端到端流程衔接
  在一个 guest 内串起 control、raw BIO、LVM2 create/write/extend/shrink/cleanup。
```

推荐顺序：

```bash
myshell/dm_linear/run_cross_target_bio_regression.sh
myshell/dm_striped/run_raw_striped_bio_test.sh
myshell/dm_striped/run_lvm2_striped_io_reboot_test.sh
myshell/dm_linear/run_cross_pv_large_write_test.sh
myshell/dm_linear/run_lvm2_resize_test.sh
myshell/dm_linear/run_linear_full_flow_test.sh
```

这样排的原因：

- BIO 回归最小，失败时能最快定位 DM split/remap/completion；
- raw striped 脚本直接验证 striped chunk split/remap 和 backing 数据分布；
- LVM2 striped 脚本验证真实 LVM2 table、ext2 文件 I/O 和重启恢复；
- 大文件测试验证真实跨 PV linear 数据面；
- LVM2 扩缩容测试覆盖重启恢复，适合作分层验收；
- full flow 验证一个 guest 内的 linear 用户态流程能连续接好。

---

## 2. 通用准备

进入开发容器：

```bash
docker exec -it myAsterinas bash
cd /root/asterinas
```

先构建 NixOS 根镜像：

```bash
make nixos
```

注意：测试脚本不会执行 `make nixos`。如果 `target/nixos/asterinas.img` 不存在，测试脚本会直接失败并提示先构建根镜像。

运行测试前确认没有正在运行的 QEMU：

```bash
pgrep -af qemu-system || true
```

如果存在本项目的 QEMU，优先进入 guest 执行：

```bash
poweroff
```

不要直接删除正在使用的测试盘镜像。

---

## 3. 测试盘约定

默认两块 Device Mapper 测试盘为：

```text
target/nixos/test.img
target/nixos/test2.img
```

对应 VirtIO serial：

```text
vdmtest
vdmtest2
```

删除当前已有的默认测试盘：

```bash
make rm_dm
```

它会自动删除 `target/nixos/` 下已有的：

```text
test.img
test2.img
test3.img
...
```

也就是有几块测试盘就删几块。

如果要删除指定测试盘，可以传入空格分隔的 `DM_TEST_IMAGES`：

```bash
DM_TEST_IMAGES="target/nixos/test.img target/nixos/test2.img target/nixos/test3.img" make rm_dm
```

`myshell/br.sh` 和 `tools/nixos/run.sh` 支持同一个 `DM_TEST_IMAGES` 列表。比如附加三块盘：

```bash
DM_TEST_IMAGES="target/nixos/test.img target/nixos/test2.img target/nixos/test3.img" myshell/br.sh
```

serial 会按顺序自动生成并显示：

```text
第 1 块：vdmtest
第 2 块：vdmtest2
第 3 块：vdmtest3
第 N 块：vdmtestN
```

guest 内不要硬编码 `/dev/vdb`、`/dev/vdc` 等设备名，而是通过 serial 定位：

```bash
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
TEST_DISK3=$(aster-dm-disk-locator vdmtest3)
```

三个自动化测试脚本当前固定使用前两块盘，默认都会删除这两块测试盘并重新创建空盘：

```text
RESET_DM_TEST_IMAGES=1
```

如果只想复用已有测试盘，可以显式设置：

```bash
RESET_DM_TEST_IMAGES=0 myshell/<script>.sh
```

通常不建议复用旧盘，因为旧的 LVM metadata 或 DM table 测试数据可能影响结果。

---

## 4. 第一层：`myshell/dm_linear/run_cross_target_bio_regression.sh`

用途：一键测试单个 4 KiB BIO 跨两个 DM linear target 边界时的拆分、重映射和 completion 聚合。

运行：

```bash
myshell/dm_linear/run_cross_target_bio_regression.sh
```

默认日志：

```text
/tmp/cross-target-bio-regression.log
```

查看关键日志：

```bash
grep -aE 'HOST_PASS|HOST_FAIL|TEST_PASS|TEST_FAIL|CHECK_PASS|=== STEP|=== CHECK|TEST_DISK=|TEST_DISK2=|DEV1=|DEV2=|^0 4 linear |^4 4 linear |Kernel panic|panicked' /tmp/cross-target-bio-regression.log
```

脚本会动态检测 guest root shell，检测到 `root@asterinas` 后立刻注入测试命令；`GUEST_READY_TIMEOUT` 只是等待 root shell 的超时上限。

测试内容：

```text
两块 512 MiB 测试盘
→ dmsetup create cross_bio_test
→ table 为两段 4-sector linear target
→ target 边界在 2 KiB
→ 向 mapper 写入一次 4 KiB payload
→ 从 mapper 读回一次 4 KiB payload
→ 校验 mapper 读回 md5 一致
→ 校验第一块 backing 盘前 2048 字节 md5
→ 校验第二块 backing 盘前 2048 字节 md5
```

通过标记：

```text
TEST_PASS_CROSS_TARGET_BIO
HOST_PASS_CROSS_TARGET_BIO
```

这个脚本只验证最小内核边界：

```text
一个 BIO 覆盖两个 linear target
→ SubmittedBio::split
→ child BIO 分别 remap
→ backing device 分别完成
→ 原 BIO completion 聚合完成
```

它不验证 LVM2 扩缩容，也不验证 ext2 大文件路径。

---

## 5. 第二层：`myshell/dm_linear/run_cross_pv_large_write_test.sh`

用途：一键测试跨 PV 大文件数据路径。

运行：

```bash
myshell/dm_linear/run_cross_pv_large_write_test.sh
```

默认日志：

```text
/tmp/cross-pv-large-write-test.log
```

查看关键日志：

```bash
grep -aE 'HOST_PASS|HOST_FAIL|TEST_PASS|TEST_FAIL|=== STEP|=== CHECK|large_vg|large_lv|linear|file700.bin|OK|No space left|Input/output error|Kernel panic|panicked' /tmp/cross-pv-large-write-test.log
```

测试内容：

```text
两块 512 MiB 测试盘
→ pvcreate 两块盘
→ vgcreate large_vg
→ lvcreate 900 MiB large_lv
→ DM table 至少两段 linear
→ mkfs.ext2 -b 4096
→ 写一个 700 MiB file700.bin
→ 保存 file700.md5
→ 首次 md5sum -c
→ 第二台 QEMU 重新激活 VG/LV
→ 只读挂载后再次 md5sum -c
```

通过标记：

```text
TEST_PASS_CROSS_PV_LARGE_WRITE_FIRST
TEST_PASS_CROSS_PV_LARGE_WRITE_SECOND
HOST_PASS_CROSS_PV_LARGE_WRITE
```

这个脚本验证的是实际数据面：

- LV 跨两块 PV；
- ext2 大文件写入跨过第一块 PV；
- DM 多段 linear table 读写正确；
- 文件内容跨重启后仍正确。

如果想临时调小文件大小，可以设置：

```bash
LARGE_WRITE_MIB=600 myshell/dm_linear/run_cross_pv_large_write_test.sh
```

默认 700 MiB 更稳，因为它大于第一块 PV 的可用空间，能确保数据进入第二块 PV。

---

## 6. 第三层：`myshell/dm_linear/run_lvm2_resize_test.sh`

用途：一键测试完整 LVM2 扩容、跨 PV、ext2 resize、缩容和重启恢复路径。

运行：

```bash
myshell/dm_linear/run_lvm2_resize_test.sh
```

默认日志：

```text
/tmp/lvm2-resize-test.log
```

查看关键日志：

```bash
grep -aE 'HOST_PASS|HOST_FAIL|TEST_PASS|TEST_FAIL|=== STEP|=== CHECK|test_vg|test_lv|linear|hello world|after grow|No space left|Input/output error|Kernel panic|panicked' /tmp/lvm2-resize-test.log
```

测试内容：

```text
两块 512 MiB 测试盘
→ pvcreate 两块盘
→ vgcreate test_vg
→ lvcreate 400 MiB test_lv
→ mkfs.ext2 -b 4096
→ mount 后写 hello.txt
→ lvextend 到 700 MiB，强制跨 PV
→ 检查 dmsetup table 至少两段 linear
→ 离线 resize2fs 扩大文件系统
→ 写 grow.txt
→ 离线 resize2fs 缩小文件系统
→ lvreduce 到 300 MiB
→ 第二台 QEMU 重新激活 VG/LV
→ 只读挂载并读取 hello.txt / grow.txt
```

通过标记：

```text
TEST_PASS_LVM2_RESIZE_FIRST
TEST_PASS_LVM2_RESIZE_SECOND
HOST_PASS_LVM2_RESIZE
```

这个脚本验证的是完整 LVM2 使用路径，重点是：

- DM ioctl 控制面；
- active table reload；
- 多段 linear table；
- ext2 扩缩容；
- LVM metadata 和文件数据跨重启恢复。

如果失败，先看日志里的：

```text
HOST_FAIL_LVM2_RESIZE
TEST_FAIL_LVM2_RESIZE_FIRST
TEST_FAIL_LVM2_RESIZE_SECOND
```

---

## 7. 单 guest 完整流程：`myshell/dm_linear/run_linear_full_flow_test.sh`

用途：在一个 NixOS guest session 内验证 linear 用户态流程可以连续接好。

运行：

```bash
myshell/dm_linear/run_linear_full_flow_test.sh
```

也可以通过组合入口运行：

```bash
myshell/run_dm_system_tests.sh --linear-flow
```

默认日志：

```text
/tmp/dm-linear-full-flow-test.log
```

测试内容：

```text
check /dev/mapper/control 和 linear target version
→ simple linear mapper table/status/deps/rename/suspend/resume
→ raw two-target 4 KiB cross-boundary BIO
→ LVM2 linear LV create + ext2 mount/write
→ LV extend 到第二块 PV + ext2 grow
→ ext2 shrink + LV shrink
→ LV/VG/PV cleanup
```

通过标记：

```text
TEST_PASS_DM_LINEAR_FULL_FLOW
HOST_PASS_DM_LINEAR_FULL_FLOW
```

这个脚本不做 reboot 恢复验证；重启恢复仍由跨 PV 大文件和 LVM2 resize 分脚本覆盖。它也不覆盖 striped target。

---

## 8. raw striped BIO 验收：`myshell/dm_striped/run_raw_striped_bio_test.sh`

用途：一键测试 raw `dm_striped` mapper 的 chunk split/remap 和 backing 数据分布。

运行：

```bash
myshell/dm_striped/run_raw_striped_bio_test.sh
```

也可以通过组合入口运行：

```bash
myshell/run_dm_system_tests.sh --striped
```

默认日志：

```text
/tmp/dm-striped-raw-bio-test.log
```

测试内容：

```text
两块 512 MiB 测试盘
→ dmsetup targets 确认 striped target 可见
→ dmsetup create dm_striped_raw
→ table 为 2-way striped、chunk size 4 sectors、总长 16 sectors
→ 写入 8 KiB payload，覆盖四个 2 KiB stripe chunk
→ 从 mapper 读回 8 KiB 并校验 md5
→ 校验第一块 backing 包含 chunk A + C
→ 校验第二块 backing 包含 chunk B + D
→ 查询 table/status/deps
```

通过标记：

```text
TEST_PASS_DM_STRIPED_RAW_BIO
HOST_PASS_DM_STRIPED_RAW_BIO
```

这个脚本只验证 raw `dmsetup create ... striped ...` 路径，不做 mkfs/mount，也不覆盖 LVM2 `lvcreate --type striped`。

---

## 9. LVM2 striped I/O 与重启恢复：`myshell/dm_striped/run_lvm2_striped_io_reboot_test.sh`

用途：一键测试 LVM2 创建 2-way striped LV 后，ext2 文件 I/O 和重启恢复路径是否可用。

运行：

```bash
myshell/dm_striped/run_lvm2_striped_io_reboot_test.sh
```

也可以通过组合入口运行：

```bash
myshell/run_dm_system_tests.sh --striped-lvm2
```

默认日志：

```text
/tmp/dm-striped-lvm2-io-reboot-test.log
```

测试内容：

```text
first guest:
  两块 512 MiB 测试盘
  → dmsetup targets 确认 striped target 可见
  → pvcreate / vgcreate
  → lvcreate --type striped -i 2 -I 4K -L 256M
  → dmsetup table/status/deps 确认为 2-way striped
  → mkfs.ext2 + mount
  → 写入 64 MiB 文件和 marker 文件
  → md5sum -c
  → umount + vgchange -an

second guest:
  → vgscan --mknodes + vgchange -ay
  → dmsetup table/status/deps 再次确认为 2-way striped
  → readonly mount
  → md5sum -c
  → 读取 marker 文件
  → umount + vgchange -an
```

通过标记：

```text
TEST_PASS_DM_STRIPED_LVM2_IO_REBOOT_FIRST
TEST_PASS_DM_STRIPED_LVM2_IO_REBOOT_SECOND
HOST_PASS_DM_STRIPED_LVM2_IO_REBOOT
```

这个脚本验证 LVM2 生成 striped table 后的文件 I/O 和 reboot recovery；不做 striped resize/shrink，也不复刻 raw 脚本中的 backing A+C/B+D 物理分布校验。

---

## 10. 手工入口：`myshell/br.sh`

用途：手工启动 NixOS，并显式附加 DM 测试盘。默认附加两块，也可以通过 `DM_TEST_IMAGES` 附加更多块。

运行：

```bash
myshell/br.sh
```

它等价于默认执行：

```bash
DM_TEST_IMAGES="target/nixos/test.img target/nixos/test2.img" make run_nixos
```

附加第三块盘时不用改脚本，直接传列表：

```bash
DM_TEST_IMAGES="target/nixos/test.img target/nixos/test2.img target/nixos/test3.img" myshell/br.sh
```

它只负责启动，不负责构建，也不会自动执行 guest 内测试命令。

进入 guest root shell 后，如果想复用原来的测试盘和 LVM metadata，不要 `pvremove` / `vgremove`，直接扫描并激活：

```bash
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "$TEST_DISK" "$TEST_DISK2"
test "$TEST_DISK" != "$TEST_DISK2"

pvscan
vgscan
vgchange --config 'activation { udev_rules=0 }' -ay test_vg

pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs --segments -o lv_name,seg_start,seg_size,devices test_vg/test_lv
dmsetup table test_vg-test_lv
```

如果要继续检查文件系统数据，可以只读挂载已有 LV，并在检查完成后停用 VG、关机：

```bash
mkdir -p /mnt/dmtest
mount -t ext2 -o ro /dev/mapper/test_vg-test_lv /mnt/dmtest
cat /mnt/dmtest/hello.txt
cat /mnt/dmtest/grow.txt
umount /mnt/dmtest
vgchange --config 'activation { udev_rules=0 }' -an test_vg
sync
poweroff
```

适合场景：

- 手工调试 LVM2 / dmsetup；
- 按手工排查文档逐步观察 `pvs`、`vgs`、`lvs`、`dmsetup table`；
- 需要进入 guest root shell 做临时排查。

---

## 11. 常用环境变量

linear 自动化测试脚本通用：

```bash
DM_TEST_IMAGE=target/nixos/test.img
DM_TEST_IMAGE_2=target/nixos/test2.img
RESET_DM_TEST_IMAGES=1
```

linear 自动化脚本都会动态检测 guest root shell，检测到 `root@asterinas` 后立刻注入测试命令。

linear 自动化脚本默认等待上限都是：

```bash
GUEST_READY_TIMEOUT=120
```

它只是等待 root shell 的超时上限，不是固定注入延迟。

手工启动多块测试盘时使用：

```bash
DM_TEST_IMAGES="target/nixos/test.img target/nixos/test2.img target/nixos/test3.img"
```

日志变量分别是：

```bash
LVM2_RESIZE_LOG=/tmp/lvm2-resize-test.log
CROSS_PV_LARGE_WRITE_LOG=/tmp/cross-pv-large-write-test.log
CROSS_TARGET_BIO_LOG=/tmp/cross-target-bio-regression.log
DM_LINEAR_FULL_FLOW_LOG=/tmp/dm-linear-full-flow-test.log
DM_STRIPED_RAW_BIO_LOG=/tmp/dm-striped-raw-bio-test.log
DM_STRIPED_LVM2_IO_REBOOT_LOG=/tmp/dm-striped-lvm2-io-reboot-test.log
```

LVM2 striped 脚本还支持调整 LV、文件和 stripe chunk 大小：

```bash
STRIPED_LV_MIB=256
STRIPED_FILE_MIB=64
STRIPED_CHUNK_KIB=4
```

如果 guest 自动登录较慢，可以加大 root shell 等待上限：

```bash
GUEST_READY_TIMEOUT=300 myshell/dm_striped/run_lvm2_striped_io_reboot_test.sh
```

如果要保留测试盘复查：

```bash
RESET_DM_TEST_IMAGES=0 myshell/dm_linear/run_cross_pv_large_write_test.sh
```

---

## 12. 失败时先看什么

先看 host 最终标记：

```text
HOST_PASS_...
HOST_FAIL_...
```

再看 guest 失败标记：

```text
TEST_FAIL_...
```

常见失败点：

1. `missing target/nixos/asterinas.img`
   - 先执行 `make nixos`。
2. `existing_qemu`
   - 还有 QEMU 在跑，先在 guest 中 `poweroff`。
3. `No space left on device`
   - LV 或文件大小设置不合理。
4. `Input/output error`
   - 优先怀疑 DM table、BIO split/remap 或 backing I/O。
5. 没有 `HOST_PASS_...`
   - 看对应 `/tmp/*.log` 中最后一个 `=== STEP` 或 `=== CHECK`。

---

## 13. 和手工排查文档的关系

LVM2 扩缩容的手工排查步骤保留在：

```text
docs/device-mapper-lvm1/nixos-linear-device-mapper-lvm2-test.md
```

linear 和 striped 系统测试已经脚本化，日常入口是：

```text
myshell/run_dm_system_tests.sh --quick
myshell/run_dm_system_tests.sh --striped
myshell/run_dm_system_tests.sh --striped-lvm2
myshell/run_dm_system_tests.sh --lvm2
myshell/run_dm_system_tests.sh --linear-flow
```

需要定位单个层次时，也可以直接运行 `myshell/dm_linear/` 或 `myshell/dm_striped/` 下的分脚本。
