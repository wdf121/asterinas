# 在 Asterinas NixOS 中测试 Linear Device Mapper 与 LVM2

本文档说明如何在正常启动的 Asterinas NixOS 中，使用标准、未经修改的 LVM2 验证 Linear Device Mapper。

本轮验收重点是：**两个 PV 组成一个 VG，一个 LV 从这个 VG 中创建；初始 LV 小于单块 512 MiB 测试盘，扩容后 LV 大于 512 MiB，从而强制 LV 跨 PV；随后再缩容并验证文件系统数据和大小统计。**

完整链路：

```text
两块 512 MiB VirtIO 测试盘
→ pvcreate 两块盘
→ vgcreate 一个 VG
→ lvcreate 一个 400 MiB LV，小于单块 PV 容量
→ DM_CREATE / DM_TABLE_LOAD / DM_DEV_SUSPEND
→ ext2 4 KiB block size 格式化、挂载、写入 hello world
→ 挂载状态下 lvextend 到 700 MiB，大于单块 PV 容量，强制跨 PV
→ 检查 dmsetup table 出现跨 PV 的多段 linear table
→ 离线 resize2fs 扩大文件系统
→ 写入 grow.txt，并用 du -sh 检查挂载点和文件占用
→ 离线 resize2fs 缩小文件系统到 300 MiB
→ lvreduce 缩小 LV 到 300 MiB
→ 第二台独立 QEMU 重新扫描并激活 VG/LV
→ 只读挂载并读取第一台 QEMU 写入的文件
→ du -sh 再次验证恢复后的文件系统统计
```

这个测试验证的是：

1. LVM2 能通过标准 Device Mapper ioctl 创建 linear LV；
2. 一个 VG 可以同时包含两块 PV；
3. 初始 LV 小于单块 PV 容量时可以正常创建、格式化、挂载和读写；
4. LV 扩容到大于单块 PV 容量后，LVM2 能把同一个 LV 跨到第二块 PV；
5. Device Mapper 能加载并使用跨 PV 的多段 linear table；
6. 扩容和缩容前后 ext2 数据保持可读；
7. `du -sh` 能在 DM 设备上的 ext2 中统计挂载点和文件占用；
8. 第二台 QEMU 能从持久化 PV/VG/LV 元数据恢复跨 PV 操作后的 LV。

---

## 1. 当前测试布局

### 1.1 支持的正向场景

本验收使用以下布局作为通过条件：

```text
/dev/vdX  ── PV ┐
                ├── test_vg ── test_lv
/dev/vdY  ── PV ┘
```

初始创建：

```text
test_lv = 400 MiB
```

400 MiB 小于单块 512 MiB 测试盘容量，因此初始 LV 应只落在一块 PV 上。

扩容后：

```text
test_lv = 700 MiB
```

700 MiB 大于单块 512 MiB 测试盘容量，因此 LV 必须使用第二块 PV 的空间。

缩容后：

```text
test_lv = 300 MiB
```

300 MiB 小于单块 PV 容量，用于验证跨 PV 扩容后再缩小 LV 和文件系统是否仍保持一致。

### 1.2 预期的 dmsetup table 变化

初始 400 MiB LV 对应 819200 个 512 字节 sector，典型 table 只有一行：

```text
0 819200 linear <pv1-major>:<pv1-minor> <offset1>
```

扩容到 700 MiB 后，对应 1433600 个 512 字节 sector。由于 700 MiB 超过单块测试盘容量，典型 table 应出现至少两行 linear target：

```text
0      <len1> linear <pv1-major>:<pv1-minor> <offset1>
<len1> <len2> linear <pv2-major>:<pv2-minor> <offset2>
```

只要求：

- table 总长度等于 1433600 sectors；
- 至少出现两行 `linear`；
- backing device 至少包含两块不同 PV；
- `cat` 和 `du -sh` 在扩容前后都能正常工作。

缩容到 300 MiB 后，对应 614400 个 512 字节 sector。LVM2 可能把 table 缩回一行，也可能保留符合其分配策略的 linear 段；通过条件是：

- table 总长度等于 614400 sectors；
- 每一行都是 `linear`；
- 文件系统缩容、挂载、读取和 `du -sh` 都成功。

### 1.3 关于在线 resize 和 umount

LVM 层和文件系统层分开验证。

在正常 Linux/openEuler 上，常见顺序是：

```text
扩容：扩底层块设备 / pvresize / VG 变大 / lvextend / 扩文件系统
缩容：先缩文件系统 / 再 lvreduce / 必要时再缩 PV 或底层设备
```

这个顺序是对的。

本文档采用：

1. **LV 扩容可以在文件系统仍挂载时执行**，用于验证 active LV 的 DM table reload；
2. 扩容后立即读取 `hello.txt` 并执行 `du -sh`，确认旧文件系统仍然可用；
3. 文件系统 resize 使用离线路径，即 `umount` 后执行 `resize2fs`；
4. 缩容严格使用安全顺序：`umount` → `resize2fs` 缩文件系统 → `lvreduce` 缩 LV。

本文档不把 ext2 在线 resize 作为通过条件。

---

## 2. 测试约束

- 所有构建和运行命令都应在 Asterinas 项目开发容器中执行。
- 必须通过正常的 `make nixos` 和 `make run_nixos` 启动系统。
- 不要使用 `NIXOS_DISABLE_SYSTEMD=true` 或自定义 Stage 2 shell 绕过正常启动流程。
- 第一块测试盘为 `target/nixos/test.img`，VirtIO serial 为 `vdmtest`。
- 第二块测试盘通过 `DM_TEST_IMAGE_2=target/nixos/test2.img` 显式启用，VirtIO serial 为 `vdmtest2`。
- 每轮全新测试开始前删除旧的 `target/nixos/test.img` 和 `target/nixos/test2.img`，让运行脚本自动创建空盘。
- 第一台 QEMU 关机后必须保留两块测试盘，供第二台 QEMU 做持久化验证。
- 不要在 `configuration.nix` 中全局覆盖 `/etc/lvm/lvm.conf`。
- 只在需要 LVM2 自行创建 no-udev 设备链接的命令中显式设置 `udev_rules=0`。
- ext2 必须使用 4 KiB block size。
- 不要使用整个磁盘镜像的 SHA-256 判断文件数据是否保持不变。
- 每次结束前都要卸载文件系统、停用 VG，并正常执行 `poweroff`。

---

## 3. 进入开发容器

本地已经准备好名为 `myAsterinas` 的容器时，执行：

```bash
docker exec -it myAsterinas bash
cd /root/asterinas
```

后续所有 host 侧命令均在容器内的 `/root/asterinas` 执行。

---

## 4. 准备全新的双测试盘

先确认没有 QEMU 正在运行：

```bash
pgrep -af qemu-system || true
```

如果输出中存在本项目的 QEMU，应先进入 guest 执行：

```bash
poweroff
```

不要直接删除正在使用的磁盘镜像。

确认 QEMU 已退出后，删除上一轮测试盘：

```bash
rm -f target/nixos/test.img target/nixos/test2.img
```

这两个文件都是普通 raw 镜像，使用 `rm -f` 即可。不要使用 `rm -rf`。

`tools/nixos/run.sh` 会在测试盘不存在时：

1. 创建 512 MiB raw 镜像；
2. 将 `target/nixos/test.img` 作为 VirtIO block 设备附加，serial 为 `vdmtest`；
3. 当 `DM_TEST_IMAGE_2` 非空时，将第二块镜像附加为 VirtIO block 设备，serial 为 `vdmtest2`；
4. 后续启动中原样复用同一组镜像。

---

## 5. 构建正常 NixOS 根镜像

执行：

```bash
make nixos
```

通过标准：

- 命令退出码为 0；
- 安装日志最终出现 NixOS 安装成功信息；
- 不出现 `Activation script snippet 'users' failed`；
- 不出现 `Structure needs cleaning`。

---

## 6. 第一台 QEMU：创建跨 PV LV、读写、扩容和缩容

启动 NixOS，并显式附加第二块测试盘：

```bash
DM_TEST_IMAGE_2=target/nixos/test2.img make run_nixos
```

等待系统通过正常 Stage 2 和 systemd 路径启动。串口中应出现 root 自动登录和 shell 提示符：

```text
asterinas login: root (automatic login)
[root@asterinas:~]#
```

以下命令均在 guest root shell 中执行。

---

### 6.1 定位两块测试盘

执行：

```bash
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "$TEST_DISK" "$TEST_DISK2"
ls -l "$TEST_DISK" "$TEST_DISK2"
test "$TEST_DISK" != "$TEST_DISK2"
```

`aster-dm-disk-locator` 默认查找 serial 为 `vdmtest` 的整盘设备。传入 `vdmtest2` 时查找第二块测试盘。

设备名由当前 VirtIO 枚举顺序决定。不要硬编码 `/dev/vdb`、`/dev/vdc` 或其他固定名称。

---

### 6.2 定义最小 no-udev 配置

Asterinas 当前不提供 udev。LVM2 在创建 LV 时默认需要通过 `/dev/<VG>/<LV>` 清理新 LV 的起始区域，而该路径通常由 udev 创建。

本测试使用以下最小命令级配置，让标准 LVM2 自行创建所需链接：

```bash
LVM_CONFIG='activation { udev_rules=0 }'
```

该变量只用于显式传给 LVM2 命令，不会修改 guest 的全局 `/etc/lvm/lvm.conf`。

---

### 6.3 创建两个 PV 和一个 VG

以下命令会改写 `TEST_DISK` 和 `TEST_DISK2`，只能对本轮刚创建的空测试盘执行：

```bash
pvcreate "$TEST_DISK" "$TEST_DISK2"
vgcreate test_vg "$TEST_DISK" "$TEST_DISK2"
```

预期输出包含：

```text
Physical volume "..." successfully created.
Volume group "test_vg" successfully created
```

检查：

```bash
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
```

预期：

- `test_vg` 中有两个 PV；
- 每块 PV 大小约为 512 MiB；
- VG 总大小约为 1 GiB；
- VG 中还没有 LV。

---

### 6.4 创建初始 400 MiB LV

初始 LV 必须小于单块 512 MiB 测试盘。执行：

```bash
lvcreate \
    --config "$LVM_CONFIG" \
    --type linear \
    -L 400M \
    -n test_lv \
    test_vg "$TEST_DISK"
```

预期输出包含：

```text
Logical volume "test_lv" created.
```

检查：

```bash
lvs -a -o lv_name,vg_name,lv_size,lv_attr,devices
lvs --segments -o lv_name,seg_start,seg_size,devices test_vg/test_lv
dmsetup info test_vg-test_lv
dmsetup table test_vg-test_lv
dmsetup deps test_vg-test_lv
```

预期：

- `test_lv` 大小为 400 MiB；
- `test_lv` active；
- 初始 `test_lv` 应只使用 `TEST_DISK`；
- 初始 `dmsetup table test_vg-test_lv` 只有一行 `linear`；
- 400 MiB 对应 819200 个 512 字节 sector。

典型 table：

```text
0 819200 linear <pv1-major>:<pv1-minor> <offset1>
```

---

### 6.5 检查 Device Mapper 设备节点

执行：

```bash
ls -l /dev/dm-* /dev/mapper /dev/test_vg
```

在没有其他 DM 设备的全新 guest 中，预期存在：

```text
/dev/dm-0
/dev/mapper/control
/dev/mapper/test_vg-test_lv -> ../dm-0
/dev/test_vg/test_lv -> /dev/mapper/test_vg-test_lv
```

实际 `/dev/dm-N` 编号只要求稳定指向对应 mapper 设备，不要求业务逻辑依赖具体数字。

所有实际格式化、挂载和数据校验统一直接使用：

```text
/dev/mapper/test_vg-test_lv
```

---

### 6.6 创建 4 KiB ext2 并写入数据

执行：

```bash
MAPPER_DEVICE=/dev/mapper/test_vg-test_lv

mkfs.ext2 -F -b 4096 "$MAPPER_DEVICE"
blkid "$MAPPER_DEVICE"
```

`blkid` 输出应包含：

```text
BLOCK_SIZE="4096" TYPE="ext2"
```

挂载并写入：

```bash
mkdir -p /mnt/dmtest
mount -t ext2 "$MAPPER_DEVICE" /mnt/dmtest

printf 'hello world\n' > /mnt/dmtest/hello.txt
cat /mnt/dmtest/hello.txt
du -sh /mnt/dmtest /mnt/dmtest/hello.txt
sync
```

预期输出：

```text
hello world
```

`du -sh` 应能正常统计挂载点和 `hello.txt` 的占用，证明文件系统目录遍历和 inode/block 统计路径在 DM 设备上可用。

此时文件系统保持挂载，用于下一步验证 active LV 跨 PV 扩容。

---

### 6.7 在挂载状态下扩容 LV 到 700 MiB

这一步只扩 LVM/DM 设备，不立即要求 ext2 在线 resize。

700 MiB 大于单块 512 MiB 测试盘容量，因此必须使用第二块 PV。执行：

```bash
lvextend \
    --config "$LVM_CONFIG" \
    -L 700M \
    test_vg/test_lv "$TEST_DISK2"
```

预期输出包含：

```text
Size of logical volume test_vg/test_lv changed
Logical volume test_vg/test_lv successfully resized.
```

立即检查 DM table 和文件系统旧数据：

```bash
lvs --segments -o lv_name,seg_start,seg_size,devices test_vg/test_lv
dmsetup table test_vg-test_lv
cat /mnt/dmtest/hello.txt
du -sh /mnt/dmtest /mnt/dmtest/hello.txt
sync
```

通过条件：

- `lvextend` 成功；
- `test_lv` 大小为 700 MiB；
- `lvs --segments` 显示 `test_lv` 至少使用两个 PV；
- `dmsetup table test_vg-test_lv` 至少有两行 `linear`；
- table 总长度为 1433600 sectors；
- 文件系统仍挂载时，旧文件 `hello.txt` 仍可读；
- 文件系统仍挂载时，`du -sh` 仍能统计挂载点和 `hello.txt`。

这一步覆盖的是：LVM2 对 active DM 设备重新加载跨 PV 的 linear table。

---

### 6.8 离线扩大 ext2 文件系统

当前不把 ext2 在线 resize 作为通过条件，因此先卸载：

```bash
umount /mnt/dmtest
```

然后扩大文件系统：

```bash
e2fsck -f -y "$MAPPER_DEVICE"
resize2fs "$MAPPER_DEVICE"
e2fsck -f -y "$MAPPER_DEVICE"
```

重新挂载并确认数据：

```bash
mount -t ext2 "$MAPPER_DEVICE" /mnt/dmtest
cat /mnt/dmtest/hello.txt
printf 'after grow\n' > /mnt/dmtest/grow.txt
cat /mnt/dmtest/grow.txt
du -sh /mnt/dmtest /mnt/dmtest/hello.txt /mnt/dmtest/grow.txt
sync
```

预期输出包含：

```text
hello world
after grow
```

`du -sh` 应能正常统计扩容后的挂载点、旧文件和新文件。

---

### 6.9 离线缩小 ext2，再缩小 LV 到 300 MiB

缩容必须先缩文件系统，再缩 LV。先卸载：

```bash
umount /mnt/dmtest
```

先把 ext2 缩小到 300 MiB：

```bash
e2fsck -f -y "$MAPPER_DEVICE"
resize2fs "$MAPPER_DEVICE" 300M
e2fsck -f -y "$MAPPER_DEVICE"
```

再缩小 LV：

```bash
lvreduce \
    --config "$LVM_CONFIG" \
    -y \
    -L 300M \
    test_vg/test_lv
```

检查：

```bash
lvs --segments -o lv_name,seg_start,seg_size,devices test_vg/test_lv
dmsetup table test_vg-test_lv
```

通过条件：

- `resize2fs "$MAPPER_DEVICE" 300M` 成功；
- `lvreduce` 成功；
- `test_lv` 大小为 300 MiB；
- table 总长度为 614400 sectors；
- table 中每一行都是 `linear`。

重新挂载并读取文件：

```bash
mount -t ext2 "$MAPPER_DEVICE" /mnt/dmtest
cat /mnt/dmtest/hello.txt
cat /mnt/dmtest/grow.txt
du -sh /mnt/dmtest /mnt/dmtest/hello.txt /mnt/dmtest/grow.txt
sync
```

预期输出包含：

```text
hello world
after grow
```

`du -sh` 应能正常统计缩容后的挂载点、旧文件和扩容后写入的新文件。

---

### 6.10 卸载、停用并正常关机

执行：

```bash
umount /mnt/dmtest
vgchange --config "$LVM_CONFIG" -an test_vg
sync
poweroff
```

预期 LVM2 报告 `test_vg` 中没有 active LV，随后 QEMU 正常退出。

不要直接终止 QEMU，否则可能损坏 NixOS 根镜像或测试盘文件系统。

---

## 7. 第二台 QEMU：恢复并校验数据

第一台 QEMU 完全退出后，必须保留刚才写入的：

```text
target/nixos/test.img
target/nixos/test2.img
```

不要再次执行：

```bash
rm -f target/nixos/test.img target/nixos/test2.img
```

在开发容器中的项目根目录重新启动，并继续附加同一块第二测试盘：

```bash
DM_TEST_IMAGE_2=target/nixos/test2.img make run_nixos
```

等待第二台 guest 正常进入 root shell。以下命令均在第二台 guest 中执行。

---

### 7.1 重新定位两块测试盘

执行：

```bash
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "$TEST_DISK" "$TEST_DISK2"
test "$TEST_DISK" != "$TEST_DISK2"
```

第二次启动中的 `/dev/vdX` 名称不应被硬编码，仍然必须通过 serial `vdmtest` 和 `vdmtest2` 唯一定位。

---

### 7.2 扫描并重新激活 VG/LV

执行：

```bash
LVM_CONFIG='activation { udev_rules=0 }'

pvscan
vgscan
vgchange --config "$LVM_CONFIG" -ay test_vg
```

预期输出包含：

```text
Found volume group "test_vg" using metadata type lvm2
1 logical volume(s) in volume group "test_vg" now active
```

检查恢复后的状态：

```bash
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,vg_name,lv_size,lv_attr,devices
lvs --noheadings -o lv_size --units m test_vg/test_lv
lvs --segments -o lv_name,seg_start,seg_size,devices test_vg/test_lv
dmsetup table test_vg-test_lv
ls -l /dev/dm-* /dev/mapper /dev/test_vg
```

预期结果包括：

- `test_vg` 中有两个 PV；
- `test_vg` 中有一个 LV；
- `test_lv` 大小为 300 MiB；
- `test_lv` 重新 active；
- mapper 设备和 `/dev/test_vg/test_lv` 链接重新出现；
- `dmsetup table test_vg-test_lv` 的总长度为 614400 sectors；
- table 中每一行都是 `linear`。

这证明 DM 设备不是从第一台 QEMU 的内存状态继承而来，而是标准 LVM2 根据测试盘中持久化的 PV/VG/LV 元数据重新建立的。

---

### 7.3 只读挂载并读取数据

执行：

```bash
mkdir -p /mnt/dmtest
mount -t ext2 -o ro /dev/mapper/test_vg-test_lv /mnt/dmtest

cat /mnt/dmtest/hello.txt
cat /mnt/dmtest/grow.txt
du -sh /mnt/dmtest /mnt/dmtest/hello.txt /mnt/dmtest/grow.txt
```

预期输出包含：

```text
hello world
after grow
```

`du -sh` 应能在只读挂载状态下正常统计恢复后的挂载点和文件。

只要第二台独立 QEMU 能读到这些内容，即可证明第一台 QEMU 写入的文件数据能够跨启动恢复，并且跨 PV 扩容、缩容后的 LV 元数据和文件系统状态也能恢复。

---

### 7.4 卸载、停用并正常关机

执行：

```bash
umount /mnt/dmtest
vgchange --config "$LVM_CONFIG" -an test_vg
sync
poweroff
```

等待 QEMU 正常退出。

---

## 8. 通过标准

必须同时满足以下条件，才能认为 Linear Device Mapper 的 NixOS/LVM2 跨 PV 验收通过：

1. `make nixos` 成功构建正常 NixOS 根镜像。
2. 测试开始前删除旧 `target/nixos/test.img` 和 `target/nixos/test2.img`。
3. `DM_TEST_IMAGE_2=target/nixos/test2.img make run_nixos` 自动创建并附加两块 512 MiB 测试盘。
4. 两台 QEMU 都通过正常 Stage 2 和 systemd 路径启动。
5. 两台 QEMU 都自动登录到真实 root shell。
6. `aster-dm-disk-locator` 每次都唯一定位到 serial 为 `vdmtest` 的测试盘。
7. `aster-dm-disk-locator vdmtest2` 每次都唯一定位到 serial 为 `vdmtest2` 的测试盘。
8. 标准 LVM2 成功完成两个 PV、一个 VG、一个 LV 创建。
9. 初始 `test_lv` 为 400 MiB，小于单块测试盘容量，并能格式化、挂载和读写。
10. 初始 `dmsetup table` 总长度为 819200 sectors。
11. 初次写入后，`hello.txt` 输出 `hello world`，`du -sh` 能统计挂载点和 `hello.txt`。
12. `test_lv` 能从 400 MiB 扩容到 700 MiB，大于单块测试盘容量。
13. 扩容后 `lvs --segments` 显示 `test_lv` 至少使用两个 PV。
14. 扩容后的 `dmsetup table` 至少有两行 `linear`，总长度为 1433600 sectors。
15. 扩容后旧数据 `hello.txt` 仍可读，`du -sh` 仍能统计挂载点和 `hello.txt`。
16. 离线扩大 ext2 后，`grow.txt` 可写入，`du -sh` 能统计挂载点、旧文件和新文件。
17. `test_lv` 能按 `resize2fs` 先行、`lvreduce` 后行的顺序从 700 MiB 缩容到 300 MiB。
18. 缩容后的 `dmsetup table` 总长度为 614400 sectors，且每一行都是 `linear`。
19. 缩容后 `hello.txt` 和 `grow.txt` 仍可读，`du -sh` 能统计缩容后的挂载点和文件占用。
20. 第二台 QEMU 重新扫描并激活后，`test_vg` 仍包含两个 PV，`test_lv` 为 300 MiB。
21. 第二台只读挂载后，`hello.txt` 输出 `hello world`，`grow.txt` 输出 `after grow`，`du -sh` 能统计恢复后的挂载点和文件占用。
22. 两次测试结束前都完成卸载、VG 停用和正常关机。

---

## 9. Expected unsupported 检查

下面这些布局可以作为 expected unsupported 检查，但不能作为本轮跨 PV linear 验收的通过条件：

- `striped` LV；
- mirror/raid/thin/cache 等非 linear LV；
- DM-on-DM backing device；
- 已挂载 ext2 上的在线 resize；
- 依赖 udev 自动创建设备节点的流程。

本轮验收只要求 linear target。跨 PV 后可以有多行 `linear` target，但不要求支持其他 target 类型。

---

## 10. 常见非阻塞警告

当前环境中，LVM2 可能输出：

```text
Failed to set up async io, using sync io.
WARNING: setpriority -18 failed: Permission denied.
Kernel not configured for semaphores (System V IPC). Not using udev synchronization code.
WARNING: Unknown logical_block_size for device /dev/vdX.
```

如果后续明确报告操作成功，并且设备节点、挂载和数据校验均符合预期，则这些 warning 不应单独判定测试失败。

正常启动期间还可能出现：

```text
Failed to find module 'autofs4'
Failed to find module 'unix'
```

这里的 `unix` 是内核模块名，不是 PAM 的 `pam_unix.so`。如果 root 自动登录成功并出现 shell 提示符，则该 warning 不阻塞本测试。

---

## 11. 故障排查

### 11.1 找不到测试盘

如果 `aster-dm-disk-locator` 报告匹配数量不是 1：

1. 确认 `tools/nixos/run.sh` 已附加默认的 `target/nixos/test.img`；
2. 确认第一块测试盘使用 serial `vdmtest`；
3. 确认没有额外附加另一块同 serial 的磁盘；
4. 不要改用硬编码 `/dev/vdX` 绕过定位失败。

如果 `aster-dm-disk-locator vdmtest2` 找不到设备：

1. 确认 host 侧启动命令包含 `DM_TEST_IMAGE_2=target/nixos/test2.img`；
2. 确认第二块测试盘使用 serial `vdmtest2`；
3. 确认第二次 QEMU 恢复验证时仍然传入同一个 `DM_TEST_IMAGE_2`。

### 11.2 `lvcreate` 报告设备不存在

如果出现：

```text
/dev/test_vg/test_lv: not found: device not cleared
Aborting. Failed to wipe start of new LV.
```

确认 `lvcreate` 使用了：

```bash
--config 'activation { udev_rules=0 }'
```

完整命令应包含：

```bash
lvcreate \
    --config 'activation { udev_rules=0 }' \
    --type linear \
    -L 400M \
    -n test_lv \
    test_vg "$TEST_DISK"
```

不需要加入 `udev_sync=0`。

### 11.3 mapper 设备不存在

执行：

```bash
ls -l /dev/dm-* /dev/mapper
lvs -a
```

如果 VG 已存在但 LV 未激活，执行：

```bash
vgchange \
    --config 'activation { udev_rules=0 }' \
    -ay test_vg
```

### 11.4 扩容后没有跨 PV

如果扩容到 700 MiB 后 `lvs --segments` 仍只显示一个 PV，说明测试盘容量、PV 元数据或命令参数与预期不符。

检查：

```bash
pvs -o pv_name,pv_size,pv_free,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs --segments -o lv_name,seg_start,seg_size,devices test_vg/test_lv
```

确认：

- 每块测试盘约 512 MiB；
- `test_vg` 有两个 PV；
- `test_lv` 目标大小确实是 700 MiB；
- `lvextend` 命令中传入了 `$TEST_DISK2`。

### 11.5 ext2 格式化成功但挂载失败

确认格式化命令使用 mapper 路径并显式指定 4 KiB block size：

```bash
mkfs.ext2 -F -b 4096 /dev/mapper/test_vg-test_lv
```

不要使用当前环境下默认生成 1 KiB block-size 文件系统的命令作为验收方式。

### 11.6 缩容顺序错误

不要先执行 `lvreduce` 再缩小 ext2。

正确顺序是：

```text
umount
→ e2fsck -f
→ resize2fs 缩小文件系统
→ e2fsck -f
→ lvreduce
```

先缩小 LV 会截断文件系统后部，可能直接破坏数据。

### 11.7 多 target table 被拒绝

如果扩容到 700 MiB 时失败，并且内核日志或 LVM2 报错指向 `DM_TABLE_LOAD`、`target_count` 或 `UnsupportedTargetCount`，说明当前运行的内核仍然拒绝多段 linear table。

本测试要求跨 PV LV 成为正向验收，因此需要确认当前内核已经支持一个 DM 设备加载多行 `linear` target。

### 11.8 users activation 或 PAM 登录失败

如果启动日志出现：

```text
malformed JSON string
Activation script snippet 'users' failed
login: PAM Failure
Structure needs cleaning
```

应优先怀疑复用的 NixOS 根镜像文件系统损坏，而不是 Device Mapper ioctl 或独立测试盘参数。

先确保 QEMU 已完全退出，再在开发容器中只读检查根分区：

```bash
disk=$(losetup -fP --show -r target/nixos/asterinas.img)
e2fsck -fn "${disk}p2"
losetup -d "$disk"
```

如果确认根镜像损坏，先将它重命名备份，再重新构建：

```bash
mv target/nixos/asterinas.img \
    "target/nixos/asterinas.img.corrupt-$(date +%Y%m%d-%H%M%S)"
make nixos
```

只处理 `asterinas.img`。不要删除、移动或重新格式化仍需用于第二次 QEMU 验证的测试盘。
