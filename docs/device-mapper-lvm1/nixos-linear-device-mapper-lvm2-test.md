# 在 Asterinas NixOS 中测试 Linear Device Mapper 与 LVM2

本文档说明如何在正常启动的 Asterinas NixOS 中，
使用标准、未经修改的 LVM2 验证第一版 Linear Device Mapper。

测试覆盖以下完整链路：

```text
pvcreate
→ vgcreate
→ lvcreate
→ DM_CREATE / DM_TABLE_LOAD / DM_DEV_SUSPEND
→ /dev/dm-N
→ /dev/mapper/<VG>-<LV>
→ ext2 格式化、挂载和读写
→ 第二台独立 QEMU 重新扫描并激活 VG/LV
→ 校验第一台 QEMU 写入的数据
```

本文档只描述测试步骤、预期结果和故障排查。
Device Mapper 的内部设计和 Linux ABI 分析不在本文档范围内。

## 1. 测试约束

- 所有构建和运行命令都应在 Asterinas 项目开发容器中执行。
- 必须通过正常的 `make nixos` 和 `make run_nixos` 启动系统。
- 不要使用 `NIXOS_DISABLE_SYSTEMD=true` 或自定义 Stage 2 shell 绕过正常启动流程。
- 测试盘固定为 `target/nixos/test.img`。
- 每轮全新测试开始前删除旧的 `test.img`，让运行脚本自动创建空盘。
- 第一次 QEMU 关机后必须保留 `test.img`，供第二次 QEMU 做持久化验证。
- 不要在 `configuration.nix` 中全局覆盖 `/etc/lvm/lvm.conf`。
- 只在需要 LVM2 自行创建 no-udev 设备链接的命令中显式设置 `udev_rules=0`。
- ext2 必须使用 4 KiB block size。
  当前 Asterinas 挂载路径不能可靠挂载默认的 1 KiB block-size ext2。
- 不要使用整个 `test.img` 的 SHA-256 判断文件数据是否保持不变。
  LVM 激活或文件系统挂载可能更新磁盘元数据。
  应校验 LV 文件系统内的 payload。
- 每次结束前都要卸载文件系统、停用 VG，并正常执行 `poweroff`。

## 2. 进入开发容器

本地已经准备好名为 `myAsterinas` 的容器时，执行：

```bash
docker exec -it myAsterinas bash
cd /root/asterinas
```

后续所有 host 侧命令均在容器内的 `/root/asterinas` 执行。

## 3. 准备全新的默认测试盘

先确认没有 QEMU 正在运行：

```bash
pgrep -af qemu-system || true
```

如果输出中存在本项目的 QEMU，
应先进入 guest 执行 `poweroff`，
不要直接删除正在使用的磁盘镜像。

确认 QEMU 已退出后，删除上一轮测试盘：

```bash
rm -f target/nixos/test.img
```

`test.img` 是普通文件，
使用 `rm -f` 即可。
不要使用 `rm -rf`，
以免路径填写错误时递归删除非预期目录。

[run.sh](../../tools/nixos/run.sh) 会在 `test.img` 不存在时：

1. 创建一个 512 MiB raw 镜像；
2. 将它作为独立的 VirtIO block 设备附加到 NixOS guest；
3. 为该设备设置 VirtIO serial `vdmtest`；
4. 在后续启动中原样复用同一个镜像。

## 4. 构建正常 NixOS 根镜像

执行：

```bash
make nixos
```

通过标准如下：

- 命令退出码为 0；
- 安装日志最终出现 NixOS 安装成功信息；
- 不出现 `Activation script snippet 'users' failed`；
- 不出现 `Structure needs cleaning`。

## 5. 第一台 QEMU：创建并写入 LV

启动 NixOS：

```bash
make run_nixos
```

等待系统通过正常 Stage 2 和 systemd 路径启动。
串口中应出现 root 自动登录和 shell 提示符：

```text
asterinas login: root (automatic login)
[root@asterinas:~]#
```

以下命令均在 guest root shell 中执行。

### 5.1 定位测试盘

执行：

```bash
TEST_DISK=$(aster-dm-disk-locator)
printf 'TEST_DISK=%s\n' "$TEST_DISK"
ls -l "$TEST_DISK"
```

`aster-dm-disk-locator` 会读取 VirtIO Host ID，
查找 serial 为 `vdmtest` 的整盘设备，
并要求恰好匹配一个设备。

设备名由当前 VirtIO 枚举顺序决定。
不要在测试中硬编码 `/dev/vdb`、`/dev/vdc` 或其他固定名称。

### 5.2 定义最小 no-udev 配置

Asterinas 当前不提供 udev。
LVM2 在创建 LV 时默认需要通过 `/dev/<VG>/<LV>` 清理新 LV 的起始区域，
而该路径通常由 udev 创建。

本测试使用以下最小命令级配置，
让标准 LVM2 自行创建所需链接：

```bash
LVM_CONFIG='activation { udev_rules=0 }'
```

不需要额外设置 `udev_sync=0`。
当前 LVM2 会检测到 Asterinas 不支持 System V semaphore，
并自动停止等待 udev 同步。

该变量只用于显式传给 LVM2 命令，
不会修改 guest 的全局 `/etc/lvm/lvm.conf`。

### 5.3 创建 PV 和 VG

以下命令会改写 `TEST_DISK`，
只能对本轮刚创建的空 `test.img` 执行：

```bash
pvcreate "$TEST_DISK"
vgcreate test_vg "$TEST_DISK"
```

预期输出包含：

```text
Physical volume "..." successfully created.
Volume group "test_vg" successfully created
```

### 5.4 创建 200 MiB Linear LV

执行：

```bash
lvcreate --config "$LVM_CONFIG" -L 200M -n test_lv test_vg
```

预期输出包含：

```text
Logical volume "test_lv" created.
```

如果不传 `udev_rules=0`，
当前环境中的标准 LVM2 会因 `/dev/test_vg/test_lv` 不存在而中止新 LV 的起始区域清理：

```text
/dev/test_vg/test_lv: not found: device not cleared
Aborting. Failed to wipe start of new LV.
```

因此 `udev_rules=0` 是第一版标准 `lvcreate` 流程的必要配置。

### 5.5 检查 PV、VG 和 LV

执行：

```bash
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,vg_name,lv_size,lv_attr,devices
```

预期结果包括：

- 一个属于 `test_vg` 的 PV；
- `test_vg` 中只有一个 LV；
- `test_lv` 大小为 200 MiB；
- `test_lv` 的属性包含 active 状态；
- backing device 是 `TEST_DISK` 的起始 extent。

典型结果如下：

```text
LV      VG      LSize   Attr       Devices
 test_lv test_vg 200.00m -wi-a----- /dev/vdX(0)
```

其中 `/dev/vdX` 应等于 `TEST_DISK` 的实际值。

### 5.6 检查 Device Mapper 设备节点

执行：

```bash
ls -l /dev/dm-* /dev/mapper /dev/test_vg
```

在没有其他 DM 设备的全新 guest 中，
预期存在：

```text
/dev/dm-0
/dev/mapper/control
/dev/mapper/test_vg-test_lv -> ../dm-0
/dev/test_vg/test_lv -> /dev/mapper/test_vg-test_lv
```

各路径职责如下：

- `/dev/dm-0` 是真正的块设备节点；
- `/dev/mapper/test_vg-test_lv` 是 Device Mapper 的稳定名称；
- `/dev/test_vg/test_lv` 是标准 LVM2 在 `udev_rules=0` 模式下创建的用户友好链接。

由于 Asterinas 当前不实现 udev，
LVM2 创建的 `/dev/test_vg/test_lv` 会指向 mapper 路径，
形成两级软链接解析：

```text
/dev/test_vg/test_lv
→ /dev/mapper/test_vg-test_lv
→ /dev/dm-0
```

第一版接受该布局。
所有实际格式化、挂载和数据校验统一直接使用：

```text
/dev/mapper/test_vg-test_lv
```

不要在内核中解析 LVM mapper 名称，
也不要在内核中特判创建 `/dev/<VG>/<LV>`。

### 5.7 检查 dmsetup 视图

执行：

```bash
dmsetup version
dmsetup targets
dmsetup info test_vg-test_lv
dmsetup table test_vg-test_lv
dmsetup deps test_vg-test_lv
```

预期 `dmsetup info` 显示设备处于 active 状态，
并且只有一个 Linear target。

200 MiB LV 包含 409600 个 512 字节 sector，
典型 table 如下：

```text
0 409600 linear <major>:<minor> <offset>
```

backing device 的 `<major>:<minor>` 应对应 `TEST_DISK`。

### 5.8 创建 4 KiB ext2

统一使用 mapper 路径格式化：

```bash
MAPPER_DEVICE=/dev/mapper/test_vg-test_lv
mkfs.ext2 -F -b 4096 "$MAPPER_DEVICE"
```

检查文件系统：

```bash
blkid "$MAPPER_DEVICE"
```

输出应包含：

```text
BLOCK_SIZE="4096" TYPE="ext2"
```

不要省略 `-b 4096`。
默认的 1 KiB block-size ext2 不能作为当前 Asterinas 挂载验收方式。

### 5.9 挂载并写入 payload

执行：

```bash
mkdir -p /mnt/dmtest
mount -t ext2 "$MAPPER_DEVICE" /mnt/dmtest

printf '%s\n' \
    'Asterinas Device Mapper LVM2 persistence test' \
    > /mnt/dmtest/payload

(
    cd /mnt/dmtest
    sha256sum payload > payload.sha256
    sha256sum -c payload.sha256
)

sync
```

`sha256sum -c` 应输出：

```text
payload: OK
```

### 5.10 卸载、停用并正常关机

执行：

```bash
umount /mnt/dmtest
vgchange --config "$LVM_CONFIG" -an test_vg
sync
poweroff
```

预期 LVM2 报告 `test_vg` 中没有 active LV，
随后 QEMU 正常退出。

不要直接终止 QEMU，
否则可能损坏 NixOS 根镜像或测试盘文件系统。

## 6. 第二台 QEMU：恢复并校验数据

第一台 QEMU 完全退出后，
必须保留刚才写入的 `target/nixos/test.img`。
不要再次执行 `rm -f target/nixos/test.img`。

在开发容器中的项目根目录重新启动：

```bash
make run_nixos
```

等待第二台 guest 正常进入 root shell。
以下命令均在第二台 guest 中执行。

### 6.1 重新定位测试盘

执行：

```bash
TEST_DISK=$(aster-dm-disk-locator)
printf 'TEST_DISK=%s\n' "$TEST_DISK"
```

第二次启动中的 `/dev/vdX` 名称不应被硬编码。
仍然必须通过 serial `vdmtest` 唯一定位。

### 6.2 扫描并重新激活 VG/LV

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
ls -l /dev/dm-* /dev/mapper /dev/test_vg
```

第二台 QEMU 中必须重新出现：

```text
/dev/dm-0
/dev/mapper/test_vg-test_lv -> ../dm-0
/dev/test_vg/test_lv -> /dev/mapper/test_vg-test_lv
```

这证明 DM 设备不是从第一台 QEMU 的内存状态继承而来，
而是标准 LVM2 根据 `test.img` 中持久化的 PV/VG/LV 元数据重新建立的。

### 6.3 只读挂载并校验 payload

仍然直接使用 mapper 路径：

```bash
MAPPER_DEVICE=/dev/mapper/test_vg-test_lv
mkdir -p /mnt/dmtest
mount -t ext2 -o ro "$MAPPER_DEVICE" /mnt/dmtest

(
    cd /mnt/dmtest
    cat payload
    sha256sum -c payload.sha256
)
```

预期输出包含：

```text
Asterinas Device Mapper LVM2 persistence test
payload: OK
```

只有 checksum 通过，
才能证明第一台 QEMU 写入的文件数据在第二台独立 QEMU 中保持完整。

### 6.4 卸载、停用并正常关机

执行：

```bash
umount /mnt/dmtest
vgchange --config "$LVM_CONFIG" -an test_vg
sync
poweroff
```

等待 QEMU 正常退出。

## 7. 通过标准

必须同时满足以下条件，
才能认为第一版 Linear Device Mapper 的 NixOS/LVM2 验收通过：

1. `make nixos` 成功构建正常 NixOS 根镜像。
2. 测试开始前删除旧 `target/nixos/test.img`。
3. `make run_nixos` 自动创建并附加新的 512 MiB 测试盘。
4. 两台 QEMU 都通过正常 Stage 2 和 systemd 路径启动。
5. 两台 QEMU 都自动登录到真实 root shell。
6. `aster-dm-disk-locator` 每次都唯一定位到 serial 为 `vdmtest` 的测试盘。
7. 标准 LVM2 成功完成 PV、VG 和 200 MiB LV 创建。
8. `lvcreate` 使用的最小配置只有 `activation { udev_rules=0 }`。
9. `/dev/dm-N`、`/dev/mapper/test_vg-test_lv` 和 `/dev/test_vg/test_lv` 均存在。
10. `/dev/mapper/test_vg-test_lv` 直接指向对应的 `/dev/dm-N`。
11. 4 KiB block-size ext2 可以格式化、挂载和读写。
12. 第一台 QEMU 正常关机后，第二台能够重新扫描并激活 VG/LV。
13. 第二台只读挂载后，`sha256sum -c payload.sha256` 输出 `payload: OK`。
14. 两次测试结束前都完成卸载、VG 停用和正常关机。

## 8. 常见非阻塞警告

当前环境中，LVM2 可能输出：

```text
Failed to set up async io, using sync io.
WARNING: setpriority -18 failed: Permission denied.
Kernel not configured for semaphores (System V IPC). Not using udev synchronization code.
WARNING: Unknown logical_block_size for device /dev/vdX.
```

如果后续明确报告操作成功，
并且设备节点、挂载和数据校验均符合预期，
则这些 warning 不应单独判定测试失败。

正常启动期间还可能出现：

```text
Failed to find module 'autofs4'
Failed to find module 'unix'
```

这里的 `unix` 是内核模块名，
不是 PAM 的 `pam_unix.so`。
如果 root 自动登录成功并出现 shell 提示符，
则该 warning 不阻塞本测试。

## 9. 故障排查

### 9.1 找不到测试盘

如果 `aster-dm-disk-locator` 报告匹配数量不是 1：

1. 确认 [run.sh](../../tools/nixos/run.sh) 已附加默认的 `target/nixos/test.img`；
2. 确认测试盘使用 serial `vdmtest`；
3. 确认没有额外附加另一块同 serial 的磁盘；
4. 不要改用硬编码 `/dev/vdX` 绕过定位失败。

### 9.2 `lvcreate` 报告设备不存在

如果出现：

```text
/dev/test_vg/test_lv: not found: device not cleared
Aborting. Failed to wipe start of new LV.
```

确认 `lvcreate` 使用了：

```bash
--config 'activation { udev_rules=0 }'
```

完整命令应为：

```bash
lvcreate \
    --config 'activation { udev_rules=0 }' \
    -L 200M \
    -n test_lv \
    test_vg
```

不需要加入 `udev_sync=0`。

### 9.3 mapper 设备不存在

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

### 9.4 ext2 格式化成功但挂载失败

确认格式化命令使用 mapper 路径并显式指定 4 KiB block size：

```bash
mkfs.ext2 -F -b 4096 /dev/mapper/test_vg-test_lv
```

不要使用当前环境下默认生成 1 KiB block-size 文件系统的命令作为验收方式。

### 9.5 users activation 或 PAM 登录失败

如果启动日志出现：

```text
malformed JSON string
Activation script snippet 'users' failed
login: PAM Failure
Structure needs cleaning
```

应优先怀疑复用的 NixOS 根镜像文件系统损坏，
而不是 Device Mapper ioctl 或独立测试盘参数。

先确保 QEMU 已完全退出，
再在开发容器中只读检查根分区：

```bash
disk=$(losetup -fP --show -r target/nixos/asterinas.img)
e2fsck -fn "${disk}p2"
losetup -d "$disk"
```

如果确认根镜像损坏，
先将它重命名备份，再重新构建：

```bash
mv target/nixos/asterinas.img \
    "target/nixos/asterinas.img.corrupt-$(date +%Y%m%d-%H%M%S)"
make nixos
```

只处理 `asterinas.img`。
不要删除、移动或重新格式化仍需用于第二次 QEMU 验证的 `target/nixos/test.img`。
