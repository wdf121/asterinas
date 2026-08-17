# 在 Asterinas NixOS 中测试跨 PV 大文件写入

本文档记录一个更贴近真实使用路径的 Device Mapper + LVM2 回归测试：两块 512 MiB 测试盘组成一个 VG，创建一个 900 MiB linear LV，格式化为 ext2 后直接写入一个 700 MiB 大文件，再重启 guest 后重新激活 VG/LV 并校验文件内容。

这个测试覆盖的是：

1. LVM2 能把一个 linear LV 分配到两个 PV 上；
2. Device Mapper 能加载跨 PV 的多段 linear table；
3. ext2 在跨 PV 的 DM 设备上能完成大文件写入；
4. 一个 700 MiB 文件能成功写完，不出现 `No space left on device` 或 `Input/output error`；
5. 文件内容在首次写入后和重启恢复后都能通过 `md5sum -c`；
6. PV/VG/LV 元数据和 ext2 文件数据能跨 QEMU 重启持久化。

文件内容直接从 `/dev/urandom` 写入。写完后把 `md5sum` 结果保存到同一个 ext2 文件系统中，重启恢复后再用 `md5sum -c` 校验。

这样不需要额外生成 expected 文件，也不需要再跑一遍 `cmp`；测试目标是跨 PV 数据路径是否正确。

---

## 1. 测试布局

使用两块 512 MiB VirtIO raw 测试盘：

```text
/dev/vdX  ── PV ┐
                ├── large_vg ── large_lv 900 MiB ── ext2 ── file700.bin
/dev/vdY  ── PV ┘
```

900 MiB 大于单块 512 MiB 测试盘，因此 `large_lv` 必须跨到第二块 PV。

典型分配结果：

```text
508 MiB 来自第一块 PV
392 MiB 来自第二块 PV
```

对应 DM table 总长度为：

```text
900 MiB / 512 B = 1843200 sectors
```

700 MiB 文件大于第一块 PV 的可用空间，因此文件数据会跨过第一个 PV，进入第二个 PV。

通过条件：

- `lvs --segments` 至少显示两段；
- `dmsetup table large_vg-large_lv` 至少显示两行 `linear`；
- table sector 总长度等于 `1843200`；
- backing device 至少有两个不同的 major:minor；
- 700 MiB 文件写入成功；
- 首次挂载状态下 `file700.bin` 通过 `md5sum -c`；
- 重启后重新激活 LV，只读挂载，`file700.bin` 仍通过 `md5sum -c`。

---

## 2. host 侧准备

进入开发容器：

```bash
docker exec -it myAsterinas bash
cd /root/asterinas
```

确认没有正在运行的 QEMU：

```bash
pgrep -af qemu-system || true
```

如果存在本项目的 QEMU，优先在 guest 内执行：

```bash
poweroff
```

准备两块新的测试盘。本测试统一使用 `br.sh` 默认的两块测试盘：

```bash
rm -f target/nixos/test.img target/nixos/test2.img
```

如果根镜像不存在，先执行：

```bash
make nixos
```

启动第一台 NixOS guest：

```bash
./br.sh
```

等待正常自动登录到 root shell：

```text
asterinas login: root (automatic login)
[root@asterinas:~]#
```

---

## 3. 第一台 guest：创建跨 PV LV 并写入大文件

以下命令均在 guest root shell 中执行。

### 3.1 定位测试盘

```bash
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "$TEST_DISK" "$TEST_DISK2"
test "$TEST_DISK" != "$TEST_DISK2"
test -b "$TEST_DISK"
test -b "$TEST_DISK2"
```

预期能看到两块不同的整盘设备，例如：

```text
TEST_DISK=/dev/vde
TEST_DISK2=/dev/vdf
```

不要硬编码设备名，实际名称由 VirtIO 枚举顺序决定。

### 3.2 创建 PV/VG/LV

Asterinas 当前不提供 udev，因此 LVM 命令显式使用 no-udev 配置：

```bash
LVM_CONFIG='activation { udev_rules=0 }'
```

创建两个 PV 和一个 VG：

```bash
pvcreate "$TEST_DISK" "$TEST_DISK2"
vgcreate large_vg "$TEST_DISK" "$TEST_DISK2"
```

创建 900 MiB linear LV：

```bash
lvcreate \
  --config "$LVM_CONFIG" \
  --type linear \
  -L 900M \
  -n large_lv \
  large_vg "$TEST_DISK" "$TEST_DISK2"
```

检查 LVM 状态：

```bash
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,vg_name,lv_size,lv_attr,devices
lvs --segments -o lv_name,seg_start,seg_size,devices large_vg/large_lv
```

预期 `large_lv` 使用两块 PV，例如：

```text
large_lv      0  508.00m /dev/vde(0)
large_lv 508.00m 392.00m /dev/vdf(0)
```

### 3.3 检查 DM table

```bash
dmsetup table large_vg-large_lv | tee /tmp/large-table.txt
test "$(grep -c ' linear ' /tmp/large-table.txt)" -ge 2
awk '{ sum += $2 } END { exit !(sum == 1843200) }' /tmp/large-table.txt
awk '{ dev[$4] = 1 } END { count = 0; for (d in dev) count++; exit !(count >= 2) }' /tmp/large-table.txt
```

预期 table 至少两行，且总长度为 `1843200` sectors。

### 3.4 格式化并挂载 ext2

```bash
MAPPER_DEVICE=/dev/mapper/large_vg-large_lv
mkfs.ext2 -F -b 4096 "$MAPPER_DEVICE"
blkid "$MAPPER_DEVICE"
mkdir -p /mnt/dmlarge
mount -t ext2 "$MAPPER_DEVICE" /mnt/dmlarge
```

`blkid` 应包含：

```text
BLOCK_SIZE="4096" TYPE="ext2"
```

### 3.5 直接写入一个 700 MiB 文件

```bash
dd if=/dev/urandom of=/mnt/dmlarge/file700.bin bs=1M count=700 conv=fsync
(
  cd /mnt/dmlarge
  md5sum file700.bin > file700.md5
  cat file700.md5
  md5sum -c file700.md5
)
```

预期：

```text
700+0 records in
700+0 records out
file700.bin: OK
```

检查空间和占用：

```bash
df -h /mnt/dmlarge
du -sh /mnt/dmlarge /mnt/dmlarge/file700.bin
sync
```

### 3.6 关机前清理

```bash
umount /mnt/dmlarge
vgchange --config "$LVM_CONFIG" -an large_vg
sync
echo TEST_PASS_CROSS_PV_LARGE_WRITE_FIRST
poweroff
```

第一轮通过标记：

```text
TEST_PASS_CROSS_PV_LARGE_WRITE_FIRST
```

---

## 4. 第二台 guest：重启后恢复并校验

host 侧重新启动 NixOS，必须继续附加同一组测试盘：

```bash
./br.sh
```

进入 guest root shell 后执行以下命令。

### 4.1 重新扫描并激活 VG/LV

```bash
LVM_CONFIG='activation { udev_rules=0 }'
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "$TEST_DISK" "$TEST_DISK2"

vgscan --mknodes
vgchange --config "$LVM_CONFIG" -ay large_vg
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs --segments -o lv_name,seg_start,seg_size,devices large_vg/large_lv
```

预期：

```text
1 logical volume(s) in volume group "large_vg" now active
```

并且 `large_lv` 仍显示两段，分别位于两块 PV 上。

### 4.2 重启后再次检查 DM table

```bash
dmsetup table large_vg-large_lv | tee /tmp/large-table-recovered.txt
test "$(grep -c ' linear ' /tmp/large-table-recovered.txt)" -ge 2
awk '{ sum += $2 } END { exit !(sum == 1843200) }' /tmp/large-table-recovered.txt
awk '{ dev[$4] = 1 } END { count = 0; for (d in dev) count++; exit !(count >= 2) }' /tmp/large-table-recovered.txt
```

通过条件与第一轮相同：至少两行 linear，总长度 `1843200` sectors，至少两个 backing device。

### 4.3 只读挂载并校验大文件

```bash
MAPPER_DEVICE=/dev/mapper/large_vg-large_lv
mkdir -p /mnt/dmlarge
mount -o ro -t ext2 "$MAPPER_DEVICE" /mnt/dmlarge

df -h /mnt/dmlarge
du -sh /mnt/dmlarge /mnt/dmlarge/file700.bin
(
  cd /mnt/dmlarge
  cat file700.md5
  md5sum -c file700.md5
)
```

通过条件：

- 只读挂载成功；
- `md5sum -c file700.md5` 输出 `file700.bin: OK`。

### 4.4 第二轮清理

```bash
umount /mnt/dmlarge
vgchange --config "$LVM_CONFIG" -an large_vg
sync
echo TEST_PASS_CROSS_PV_LARGE_WRITE_SECOND
poweroff
```

第二轮通过标记：

```text
TEST_PASS_CROSS_PV_LARGE_WRITE_SECOND
```

---

## 5. 总通过标准

本测试通过时必须同时满足：

1. 第一台 guest 成功创建两块 PV 和一个 VG；
2. `lvcreate -L 900M --type linear` 成功；
3. `lvs --segments` 显示 `large_lv` 跨两块 PV；
4. `dmsetup table large_vg-large_lv` 至少有两行 `linear`；
5. table sector 总长度为 `1843200`；
6. 700 MiB 文件写入不报错；
7. 首次写入后 `file700.bin` 通过 `md5sum -c`；
8. 第一台 guest 输出 `TEST_PASS_CROSS_PV_LARGE_WRITE_FIRST`；
9. 第二台 guest 能重新扫描并激活 `large_vg/large_lv`；
10. 重启后 DM table 仍跨两块 PV；
11. 重启后只读挂载 ext2 成功；
12. 重启后 `file700.bin` 再次通过 `md5sum -c`；
13. 第二台 guest 输出 `TEST_PASS_CROSS_PV_LARGE_WRITE_SECOND`。

如果 `dd` 出现 `No space left on device` 或 `Input/output error`，本测试失败。

---

## 6. 三块盘扩展思路

如果后续要测三块 512 MiB 测试盘，核心思路不需要写多个文件，直接写一个足够大的文件即可。

三块盘典型可用空间约为：

```text
508 MiB * 3 = 1524 MiB
```

要强制跨到第三块 PV，LV 大小需要大于两块 PV 的合计可用空间，文件大小也应超过两块 PV 能容纳的数据范围。

推荐布局：

```text
3 块 512 MiB 测试盘
→ large_vg
→ 1200 MiB large_lv
→ ext2
→ file1100.bin
```

关键检查值：

```text
1200 MiB / 512 B = 2457600 sectors
```

三块盘场景下，DM table 检查应改为：

```bash
dmsetup table large_vg-large_lv | tee /tmp/large-table.txt
test "$(grep -c ' linear ' /tmp/large-table.txt)" -ge 3
awk '{ sum += $2 } END { exit !(sum == 2457600) }' /tmp/large-table.txt
awk '{ dev[$4] = 1 } END { count = 0; for (d in dev) count++; exit !(count >= 3) }' /tmp/large-table.txt
```

大文件写入和校验可以简化为：

```bash
dd if=/dev/urandom of=/mnt/dmlarge/file1100.bin bs=1M count=1100 conv=fsync
(
  cd /mnt/dmlarge
  md5sum file1100.bin > file1100.md5
  cat file1100.md5
  md5sum -c file1100.md5
)
```

重启后只需要：

```bash
(
  cd /mnt/dmlarge
  cat file1100.md5
  md5sum -c file1100.md5
)
```

通过条件就是：

- LV 至少跨 3 个 linear target；
- table 总长度为 `2457600` sectors；
- backing device 至少有 3 个不同 major:minor；
- `file1100.bin` 首次写入后通过 `md5sum -c`；
- 重启恢复后再次通过 `md5sum -c`。

---

## 7. 与 4 KiB raw BIO 回归的区别

这个大文件测试验证真实用户路径：

```text
ext2 文件写入
→ LVM2 管理的 LV
→ Device Mapper 多段 linear table
→ 两块或三块 VirtIO backing PV
→ 重启恢复后再次读取
```

4 KiB raw BIO 回归验证更小的内核边界场景：

```text
单个 4 KiB BIO 正好跨过两个 linear target 边界
→ BIO 被拆成两个 child BIO
→ 分别 remap 到两个 backing device
→ completion 聚合完成
```

两者互补：

- 大文件测试证明格式化、挂载、大文件写入和跨重启恢复可用；
- raw BIO 回归证明单个 BIO 跨 target 边界时不会被 DM 拒绝或错误完成。
