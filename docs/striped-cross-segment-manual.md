# striped integration 的跨 segment LVM2 手动操作文档

本文记录 [run_lvm2_striped_integration_test.sh](../myshell/dm_striped/run_lvm2_striped_integration_test.sh) 所覆盖的跨 segment striped 场景：手动进入 NixOS guest 后创建初始 striped segment、用第二组 PV 增加第二个 striped segment、验证 table/status/deps、文件数据与 shrink。自动化 `--striped-integration` 使用三次 guest：第一轮创建和 grow，第二轮恢复和 shrink，第三轮重新 scan/activate 并以只读 ext2/MD5 验证 shrink 后持久化。本手册的命令主体展开前两轮，第三轮应按自动化脚本的 readonly recovery 语义补做。

## 1. 默认参数

脚本默认参数如下：

```bash
STRIPED_CS_PV_COUNT=2
STRIPED_CS_TOTAL_PV_COUNT=4
STRIPED_CS_INITIAL_LV_MIB=512
STRIPED_CS_EXTENDED_LV_MIB=1024
STRIPED_CS_SHRUNK_LV_MIB=512
STRIPED_CS_BASE_FILE_MIB=128
STRIPED_CS_GROW_FILE_MIB=480
STRIPED_CS_CHUNK_KIB=4
```

默认需要 4 个测试盘：

```text
target/nixos/test.img   -> vdmtest
target/nixos/test2.img  -> vdmtest2
target/nixos/test3.img  -> vdmtest3
target/nixos/test4.img  -> vdmtest4
```

默认 table 目标：

```text
第一段：PV1 + PV2 组成 2-way striped segment，大小 512MiB
第二段：PV3 + PV4 组成 2-way striped segment，扩容后总大小 1024MiB
缩容后：回到 512MiB，只保留第一段 striped segment
```

## 2. Host 侧准备与启动第一轮 guest

在容器内进入项目目录：

```bash
cd /root/asterinas
```

确认 NixOS 镜像存在：

```bash
test -f target/nixos/asterinas.img
```

确认没有残留 QEMU：

```bash
pgrep -af '[q]emu-system' && exit 1 || true
```

清理本次测试盘镜像：

```bash
rm -f target/nixos/test.img \
      target/nixos/test2.img \
      target/nixos/test3.img \
      target/nixos/test4.img
```

启动第一轮 NixOS guest：

```bash
DM_TEST_IMAGES="target/nixos/test.img target/nixos/test2.img target/nixos/test3.img target/nixos/test4.img" \
make run_nixos
```

等串口进入 `root@asterinas` shell 后，执行下一节命令。

## 3. 第一轮 guest：创建跨 segment striped LV 并写入数据

### 3.1 设置变量和 helper

```bash
stty -echo 2>/dev/null || true
set -eu

LVM_CONFIG='activation { udev_rules=0 }'
STRIPED_CS_PV_COUNT=2
STRIPED_CS_TOTAL_PV_COUNT=4
STRIPED_CS_INITIAL_LV_MIB=512
STRIPED_CS_EXTENDED_LV_MIB=1024
STRIPED_CS_BASE_FILE_MIB=128
STRIPED_CS_GROW_FILE_MIB=480
STRIPED_CS_CHUNK_KIB=4
INITIAL_SECTORS=$((STRIPED_CS_INITIAL_LV_MIB * 2048))
EXTENDED_SECTORS=$((STRIPED_CS_EXTENDED_LV_MIB * 2048))
GROW_SECTORS=$((EXTENDED_SECTORS - INITIAL_SECTORS))
STRIPED_CHUNK_SECTORS=$((STRIPED_CS_CHUNK_KIB * 2))
MAPPER_NAME=striped_cs_vg-striped_cs_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmstripedcs
DISKS=()
DEVS=()
BASE_DISKS=()
GROW_DISKS=()

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

dep_token() {
    printf '(%s, %s)' "${1%%:*}" "${1##*:}"
}

locate_striped_disks() {
    local count=$1 index serial disk label existing dev
    index=1
    while [ "${index}" -le "${count}" ]; do
        if [ "${index}" -eq 1 ]; then
            disk=$(aster-dm-disk-locator)
            label=TEST_DISK
        else
            serial=vdmtest${index}
            disk=$(aster-dm-disk-locator "${serial}")
            label=TEST_DISK${index}
        fi
        printf '%s=%s\n' "${label}" "${disk}"
        test -b "${disk}"
        for existing in "${DISKS[@]}"; do
            test "${existing}" != "${disk}"
        done
        DISKS+=("${disk}")
        dev=$(devno "${disk}")
        DEVS+=("${dev}")
        printf 'DEV%s=%s\n' "${index}" "${dev}"
        if [ "${index}" -le "${STRIPED_CS_PV_COUNT}" ]; then
            BASE_DISKS+=("${disk}")
        else
            GROW_DISKS+=("${disk}")
        fi
        index=$((index + 1))
    done
}

check_row_devices() {
    local table_file=$1 start=$2 len=$3 devs=$4
    awk -v start="${start}" -v len="${len}" -v stripes="${STRIPED_CS_PV_COUNT}" -v chunk="${STRIPED_CHUNK_SECTORS}" -v devs="${devs}" '
        BEGIN { n = split(devs, expected, " ") }
        $1 == start && $2 == len && $3 == "striped" && $4 == stripes && $5 == chunk {
            delete present
            for (i = 6; i <= NF; i += 2) present[$i] = 1
            ok = 1
            for (i = 1; i <= n; i++) if (!(expected[i] in present)) ok = 0
        }
        END { exit !ok }
    ' "${table_file}"
}

check_striped_cross_table() {
    local table_file=$1 deps_file=$2 status_file=$3 base_devs grow_devs dev
    base_devs="${DEVS[*]:0:${STRIPED_CS_PV_COUNT}}"
    grow_devs="${DEVS[*]:${STRIPED_CS_PV_COUNT}:${STRIPED_CS_PV_COUNT}}"
    test "$(wc -l < "${table_file}")" -eq 2
    check_row_devices "${table_file}" 0 "${INITIAL_SECTORS}" "${base_devs}"
    check_row_devices "${table_file}" "${INITIAL_SECTORS}" "${GROW_SECTORS}" "${grow_devs}"
    awk -v len="${EXTENDED_SECTORS}" '$3 == "striped" { sum += $2 } END { exit !(sum == len) }' "${status_file}"
    grep -q "${STRIPED_CS_TOTAL_PV_COUNT} dependencies" "${deps_file}"
    for dev in "${DEVS[@]}"; do
        grep -F -q "${dev}" "${table_file}"
        grep -F -q "$(dep_token "${dev}")" "${deps_file}"
    done
}

check_striped_cross_table_status_deps() {
    local label=$1 table_file=$2 status_file=$3 deps_file=$4
    echo "DM_TABLE_LVM2_STRIPED_CS_${label}_BEGIN"
    dmsetup table "${MAPPER_NAME}" | tee "${table_file}" | sed "s/^/DM_TABLE_LVM2_STRIPED_CS_${label} /"
    echo "DM_TABLE_LVM2_STRIPED_CS_${label}_END"
    dmsetup status "${MAPPER_NAME}" | tee "${status_file}"
    dmsetup deps "${MAPPER_NAME}" | tee "${deps_file}"
    check_striped_cross_table "${table_file}" "${deps_file}" "${status_file}"
}
```

### 3.2 检查 DM 控制设备、striped target 和 4 个测试盘

```bash
test -c /dev/mapper/control
dmsetup targets | tee /tmp/striped-cs-targets.txt
grep -q '^striped' /tmp/striped-cs-targets.txt
locate_striped_disks "${STRIPED_CS_TOTAL_PV_COUNT}"
```

应能看到类似输出：

```text
TEST_DISK=/dev/vde
DEV1=253:64
TEST_DISK2=/dev/vdf
DEV2=253:80
TEST_DISK3=/dev/vdg
DEV3=253:96
TEST_DISK4=/dev/vdh
DEV4=253:112
```

### 3.3 创建第一段 2-way striped LV 并写入基础数据

```bash
pvcreate "${BASE_DISKS[@]}"
vgcreate striped_cs_vg "${BASE_DISKS[@]}"
lvcreate --config "${LVM_CONFIG}" \
    --type striped \
    -i "${STRIPED_CS_PV_COUNT}" \
    -I "${STRIPED_CS_CHUNK_KIB}K" \
    -L "${STRIPED_CS_INITIAL_LV_MIB}M" \
    -n striped_cs_lv \
    striped_cs_vg \
    "${BASE_DISKS[@]}"

mkfs.ext2 -F -b 4096 "${MAPPER_DEVICE}"
mkdir -p "${MOUNT_DIR}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
dd if=/dev/urandom of="${MOUNT_DIR}/base${STRIPED_CS_BASE_FILE_MIB}.bin" bs=1M count="${STRIPED_CS_BASE_FILE_MIB}" conv=fsync status=none
printf 'lvm2 striped cross-segment base\npv_count=%s initial_lv_mib=%s\n' "${STRIPED_CS_PV_COUNT}" "${STRIPED_CS_INITIAL_LV_MIB}" > "${MOUNT_DIR}/base-marker.txt"
(
    cd "${MOUNT_DIR}"
    md5sum "base${STRIPED_CS_BASE_FILE_MIB}.bin" base-marker.txt > striped-cs.md5
    md5sum -c striped-cs.md5
)
sync
umount "${MOUNT_DIR}"
```

### 3.4 增加第二组 PV，扩容出第二段 striped segment

```bash
pvcreate "${GROW_DISKS[@]}"
vgextend striped_cs_vg "${GROW_DISKS[@]}"
lvextend --config "${LVM_CONFIG}" \
    -i "${STRIPED_CS_PV_COUNT}" \
    -I "${STRIPED_CS_CHUNK_KIB}K" \
    -L "${STRIPED_CS_EXTENDED_LV_MIB}M" \
    striped_cs_vg/striped_cs_lv \
    "${GROW_DISKS[@]}"

pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices striped_cs_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices striped_cs_vg/striped_cs_lv

check_striped_cross_table_status_deps \
    EXTENDED \
    /tmp/striped-cs-extended-table.txt \
    /tmp/striped-cs-extended-status.txt \
    /tmp/striped-cs-extended-deps.txt
```

默认预期 DM table 是两行 `striped`：

```text
0       1048576  striped 2 8 <DEV1> <offset> <DEV2> <offset>
1048576 1048576  striped 2 8 <DEV3> <offset> <DEV4> <offset>
```

默认预期 deps 是 4 个 backing device：

```text
4 dependencies : (<DEV1>) (<DEV2>) (<DEV3>) (<DEV4>)
```

### 3.5 扩展 ext2 并写入跨 segment 数据

```bash
e2fsck -f -y "${MAPPER_DEVICE}"
resize2fs "${MAPPER_DEVICE}"
e2fsck -f -y "${MAPPER_DEVICE}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c striped-cs.md5
)
dd if=/dev/urandom of="${MOUNT_DIR}/grow${STRIPED_CS_GROW_FILE_MIB}.bin" bs=1M count="${STRIPED_CS_GROW_FILE_MIB}" conv=fsync status=none
printf 'lvm2 striped cross-segment grow\nextended_lv_mib=%s\n' "${STRIPED_CS_EXTENDED_LV_MIB}" > "${MOUNT_DIR}/grow-marker.txt"
(
    cd "${MOUNT_DIR}"
    md5sum "grow${STRIPED_CS_GROW_FILE_MIB}.bin" grow-marker.txt >> striped-cs.md5
    md5sum -c striped-cs.md5
)
sync
umount "${MOUNT_DIR}"
vgchange --config "${LVM_CONFIG}" -an striped_cs_vg
sync
poweroff
```

第一轮结束后，应已在测试镜像中留下：

```text
striped_cs_vg/striped_cs_lv
base128.bin
grow480.bin
base-marker.txt
grow-marker.txt
striped-cs.md5
```

## 4. Host 侧启动第二轮 guest

第一轮 guest 关机后，在 host 侧重新启动 NixOS guest，继续使用同一组测试镜像：

```bash
DM_TEST_IMAGES="target/nixos/test.img target/nixos/test2.img target/nixos/test3.img target/nixos/test4.img" \
make run_nixos
```

等串口进入 `root@asterinas` shell 后，执行下一节命令。

## 5. 第二轮 guest：恢复、校验、缩容

### 5.1 设置变量和 helper

```bash
stty -echo 2>/dev/null || true
set -eu

LVM_CONFIG='activation { udev_rules=0 }'
STRIPED_CS_PV_COUNT=2
STRIPED_CS_TOTAL_PV_COUNT=4
STRIPED_CS_INITIAL_LV_MIB=512
STRIPED_CS_EXTENDED_LV_MIB=1024
STRIPED_CS_SHRUNK_LV_MIB=512
STRIPED_CS_BASE_FILE_MIB=128
STRIPED_CS_GROW_FILE_MIB=480
STRIPED_CS_CHUNK_KIB=4
INITIAL_SECTORS=$((STRIPED_CS_INITIAL_LV_MIB * 2048))
EXTENDED_SECTORS=$((STRIPED_CS_EXTENDED_LV_MIB * 2048))
SHRUNK_SECTORS=$((STRIPED_CS_SHRUNK_LV_MIB * 2048))
GROW_SECTORS=$((EXTENDED_SECTORS - INITIAL_SECTORS))
STRIPED_CHUNK_SECTORS=$((STRIPED_CS_CHUNK_KIB * 2))
MAPPER_NAME=striped_cs_vg-striped_cs_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmstripedcs
DISKS=()
DEVS=()

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

dep_token() {
    printf '(%s, %s)' "${1%%:*}" "${1##*:}"
}

locate_striped_disks() {
    local count=$1 index serial disk label existing dev
    index=1
    while [ "${index}" -le "${count}" ]; do
        if [ "${index}" -eq 1 ]; then
            disk=$(aster-dm-disk-locator)
            label=TEST_DISK
        else
            serial=vdmtest${index}
            disk=$(aster-dm-disk-locator "${serial}")
            label=TEST_DISK${index}
        fi
        printf '%s=%s\n' "${label}" "${disk}"
        test -b "${disk}"
        for existing in "${DISKS[@]}"; do
            test "${existing}" != "${disk}"
        done
        DISKS+=("${disk}")
        dev=$(devno "${disk}")
        DEVS+=("${dev}")
        printf 'DEV%s=%s\n' "${index}" "${dev}"
        index=$((index + 1))
    done
}

check_row_devices() {
    local table_file=$1 start=$2 len=$3 devs=$4
    awk -v start="${start}" -v len="${len}" -v stripes="${STRIPED_CS_PV_COUNT}" -v chunk="${STRIPED_CHUNK_SECTORS}" -v devs="${devs}" '
        BEGIN { n = split(devs, expected, " ") }
        $1 == start && $2 == len && $3 == "striped" && $4 == stripes && $5 == chunk {
            delete present
            for (i = 6; i <= NF; i += 2) present[$i] = 1
            ok = 1
            for (i = 1; i <= n; i++) if (!(expected[i] in present)) ok = 0
        }
        END { exit !ok }
    ' "${table_file}"
}

check_striped_cross_table() {
    local table_file=$1 deps_file=$2 status_file=$3 base_devs grow_devs dev
    base_devs="${DEVS[*]:0:${STRIPED_CS_PV_COUNT}}"
    grow_devs="${DEVS[*]:${STRIPED_CS_PV_COUNT}:${STRIPED_CS_PV_COUNT}}"
    test "$(wc -l < "${table_file}")" -eq 2
    check_row_devices "${table_file}" 0 "${INITIAL_SECTORS}" "${base_devs}"
    check_row_devices "${table_file}" "${INITIAL_SECTORS}" "${GROW_SECTORS}" "${grow_devs}"
    awk -v len="${EXTENDED_SECTORS}" '$3 == "striped" { sum += $2 } END { exit !(sum == len) }' "${status_file}"
    grep -q "${STRIPED_CS_TOTAL_PV_COUNT} dependencies" "${deps_file}"
    for dev in "${DEVS[@]}"; do
        grep -F -q "${dev}" "${table_file}"
        grep -F -q "$(dep_token "${dev}")" "${deps_file}"
    done
}

check_striped_shrunk_table() {
    local table_file=$1 deps_file=$2 status_file=$3 base_devs dev index
    base_devs="${DEVS[*]:0:${STRIPED_CS_PV_COUNT}}"
    test "$(wc -l < "${table_file}")" -eq 1
    check_row_devices "${table_file}" 0 "${SHRUNK_SECTORS}" "${base_devs}"
    awk -v len="${SHRUNK_SECTORS}" '$3 == "striped" { sum += $2 } END { exit !(sum == len) }' "${status_file}"
    grep -q "${STRIPED_CS_PV_COUNT} dependencies" "${deps_file}"
    index=0
    while [ "${index}" -lt "${STRIPED_CS_PV_COUNT}" ]; do
        dev=${DEVS[${index}]}
        grep -F -q "${dev}" "${table_file}"
        grep -F -q "$(dep_token "${dev}")" "${deps_file}"
        index=$((index + 1))
    done
}
```

### 5.2 恢复跨 segment striped LV

```bash
locate_striped_disks "${STRIPED_CS_TOTAL_PV_COUNT}"
pvscan
vgscan --mknodes
vgchange --config "${LVM_CONFIG}" -ay striped_cs_vg

pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices striped_cs_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices striped_cs_vg/striped_cs_lv

echo 'DM_TABLE_LVM2_STRIPED_CS_RECOVERED_BEGIN'
dmsetup table "${MAPPER_NAME}" | tee /tmp/striped-cs-recovered-table.txt | sed 's/^/DM_TABLE_LVM2_STRIPED_CS_RECOVERED /'
echo 'DM_TABLE_LVM2_STRIPED_CS_RECOVERED_END'
dmsetup status "${MAPPER_NAME}" | tee /tmp/striped-cs-recovered-status.txt
dmsetup deps "${MAPPER_NAME}" | tee /tmp/striped-cs-recovered-deps.txt
check_striped_cross_table \
    /tmp/striped-cs-recovered-table.txt \
    /tmp/striped-cs-recovered-deps.txt \
    /tmp/striped-cs-recovered-status.txt
```

默认恢复后仍应是两段 striped table，deps 仍包含 4 个 backing device。

### 5.3 校验文件并准备 shrink

```bash
mkdir -p "${MOUNT_DIR}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c striped-cs.md5
    cat base-marker.txt
    cat grow-marker.txt
    rm "grow${STRIPED_CS_GROW_FILE_MIB}.bin" grow-marker.txt
    md5sum "base${STRIPED_CS_BASE_FILE_MIB}.bin" base-marker.txt > striped-cs.md5
    md5sum -c striped-cs.md5
)
umount "${MOUNT_DIR}"
```

这里先删除 grow 文件和 grow marker，再把 md5 清单收敛回基础文件，确保后续能安全缩回第一段大小。

### 5.4 缩容 ext2 和 LV 回单 segment

```bash
e2fsck -f -y "${MAPPER_DEVICE}"
resize2fs "${MAPPER_DEVICE}" "${STRIPED_CS_SHRUNK_LV_MIB}M"
e2fsck -f -y "${MAPPER_DEVICE}"
lvreduce --config "${LVM_CONFIG}" -y -L "${STRIPED_CS_SHRUNK_LV_MIB}M" striped_cs_vg/striped_cs_lv

lvs -a -o lv_name,lv_size,seg_count,devices striped_cs_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices striped_cs_vg/striped_cs_lv

echo 'DM_TABLE_LVM2_STRIPED_CS_SHRUNK_BEGIN'
dmsetup table "${MAPPER_NAME}" | tee /tmp/striped-cs-shrunk-table.txt | sed 's/^/DM_TABLE_LVM2_STRIPED_CS_SHRUNK /'
echo 'DM_TABLE_LVM2_STRIPED_CS_SHRUNK_END'
dmsetup status "${MAPPER_NAME}" | tee /tmp/striped-cs-shrunk-status.txt
dmsetup deps "${MAPPER_NAME}" | tee /tmp/striped-cs-shrunk-deps.txt
check_striped_shrunk_table \
    /tmp/striped-cs-shrunk-table.txt \
    /tmp/striped-cs-shrunk-deps.txt \
    /tmp/striped-cs-shrunk-status.txt
```

默认 shrink 后预期 DM table 只剩一行 striped：

```text
0 1048576 striped 2 8 <DEV1> <offset> <DEV2> <offset>
```

默认 deps 应只剩第一组 PV：

```text
2 dependencies : (<DEV1>) (<DEV2>)
```

### 5.5 最终挂载校验并关闭 guest

```bash
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c striped-cs.md5
)
df -h "${MOUNT_DIR}"
du -sh "${MOUNT_DIR}" "${MOUNT_DIR}/base${STRIPED_CS_BASE_FILE_MIB}.bin"
umount "${MOUNT_DIR}"
vgchange --config "${LVM_CONFIG}" -an striped_cs_vg
sync
poweroff
```

## 6. 关键检查点

手动执行时重点看这些结果：

1. `dmsetup targets` 包含 `striped`。
2. 第一轮扩容后 `dmsetup table striped_cs_vg-striped_cs_lv` 有两行 `striped`。
3. 第一轮扩容后 `dmsetup deps striped_cs_vg-striped_cs_lv` 有 4 个依赖。
4. 第二轮 reboot recovery 后 table 仍是两行 `striped`，deps 仍是 4 个依赖。
5. shrink 后 table 回到一行 `striped`，deps 回到 2 个依赖。
6. `md5sum -c striped-cs.md5` 在扩容前、扩容后、reboot 后、shrink 后都通过。

## 7. 对应脚本入口

自动化脚本入口是：

```bash
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 \
  myshell/run_dm_system_tests.sh --striped-integration
```

容器内常用执行方式：

```bash
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 \
  myshell/run_dm_system_tests.sh --striped-integration
```
