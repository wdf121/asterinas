#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

# 这个脚本一键回归测试 Asterinas NixOS 中的 Device Mapper linear + LVM2 扩缩容路径。
#
# 测试方法：
# 1. host 侧清空两块 DM 测试盘镜像，使用 DM_TEST_IMAGE / DM_TEST_IMAGE_2
#    显式附加两块 512 MiB raw 盘启动 `make run_nixos`；
# 2. host 侧检测到 guest root shell 提示符后再注入测试命令，不再固定 sleep；
# 3. 第一台 guest 中通过 `aster-dm-disk-locator` 按 VirtIO serial 定位两块盘；
# 4. 创建两个 PV、一个 VG、一个 400 MiB linear LV，并在 LV 上格式化 ext2；
# 5. 挂载后写入 hello.txt，再把 LV 扩容到 700 MiB，强制 DM table 跨到第二块 PV；
# 6. 离线扩大 ext2，写入 grow.txt，再离线缩小 ext2 和 LV 到 300 MiB；
# 7. 第二台 guest 复用同一组测试盘，重新扫描并激活 VG/LV，只读挂载后读取文件。
#
# 被验证的功能：
# - LVM2 能通过标准 DM ioctl 创建、加载、切换和查询 linear table；
# - active LV 扩容后 DM table 能从单段变成跨 PV 多段 linear table；
# - ext2 在 DM 设备上格式化、挂载、读写、扩大、缩小后仍可用；
# - PV/VG/LV 元数据和 ext2 文件数据能跨 QEMU 重启恢复。
#
# 注意：脚本不会执行 `make nixos`。首次运行前请先在容器内执行 `make nixos`。
# 默认会删除 DM_TEST_IMAGE / DM_TEST_IMAGE_2 指向的测试盘镜像，以保证测试从空盘开始。
# 全量日志写入 LVM2_RESIZE_LOG；成功时打印 HOST_PASS_LVM2_RESIZE。

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/run_lvm2_resize_test.sh

Runs the full two-PV LVM2 resize regression in an Asterinas NixOS guest.

Optional environment variables:
  DM_TEST_IMAGE          First backing test image path, default target/nixos/test.img
  DM_TEST_IMAGE_2        Second backing test image path, default target/nixos/test2.img
  LVM2_RESIZE_LOG        Host-side log path, default /tmp/lvm2-resize-test.log
  GUEST_READY_TIMEOUT    Seconds to wait for guest root shell, default 120
  RESET_DM_TEST_IMAGES   1 to delete test images before running, default 1
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/..")
LOG=${LVM2_RESIZE_LOG:-/tmp/lvm2-resize-test.log}
DM_TEST_IMAGE=${DM_TEST_IMAGE:-target/nixos/test.img}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-target/nixos/test2.img}
GUEST_READY_TIMEOUT=${GUEST_READY_TIMEOUT:-120}
RESET_DM_TEST_IMAGES=${RESET_DM_TEST_IMAGES:-1}

cd "${ASTERINAS_DIR}"
rm -f "${LOG}"

echo "HOST_INFO_LVM2_RESIZE started"
echo "HOST_INFO_LVM2_RESIZE log=${LOG}"
echo "HOST_INFO_LVM2_RESIZE disk1=${DM_TEST_IMAGE} serial=vdmtest"
echo "HOST_INFO_LVM2_RESIZE disk2=${DM_TEST_IMAGE_2} serial=vdmtest2"

if pgrep -af qemu-system | grep -v 'pgrep -af qemu-system' >/tmp/lvm2-resize-qemu-running.txt 2>/dev/null; then
    echo "HOST_FAIL_LVM2_RESIZE existing_qemu"
    cat /tmp/lvm2-resize-qemu-running.txt
    exit 1
fi

if [ ! -f target/nixos/asterinas.img ]; then
    echo "HOST_FAIL_LVM2_RESIZE missing target/nixos/asterinas.img; run 'make nixos' first"
    exit 1
fi

if [ "${RESET_DM_TEST_IMAGES}" = "1" ]; then
    rm -f "${DM_TEST_IMAGE}" "${DM_TEST_IMAGE_2}"
fi

run_guest_script() {
    script_file=$1
    log_mode=$2
    label=$3
    fifo=$(mktemp -u /tmp/lvm2-resize-stdin.XXXXXX)
    mkfifo "${fifo}"

    if [ -f "${LOG}" ]; then
        start_line=$(wc -l <"${LOG}")
    else
        start_line=0
    fi
    if [ "${log_mode}" = "append" ]; then
        DM_TEST_IMAGE="${DM_TEST_IMAGE}" \
        DM_TEST_IMAGE_2="${DM_TEST_IMAGE_2}" \
        setsid make run_nixos <"${fifo}" >>"${LOG}" 2>&1 &
    else
        DM_TEST_IMAGE="${DM_TEST_IMAGE}" \
        DM_TEST_IMAGE_2="${DM_TEST_IMAGE_2}" \
        setsid make run_nixos <"${fifo}" >"${LOG}" 2>&1 &
    fi
    qemu_pid=$!

    exec 3>"${fifo}"
    rm -f "${fifo}"

    waited=0
    while ! tail -n "+$((start_line + 1))" "${LOG}" | grep -aq 'root@asterinas'; do
        if ! kill -0 "${qemu_pid}" 2>/dev/null; then
            exec 3>&-
            status=0
            wait "${qemu_pid}" || status=$?
            if [ "${status}" -eq 0 ]; then
                status=1
            fi
            echo "HOST_FAIL_LVM2_RESIZE ${label}_guest_exited_before_shell status=${status}"
            return "${status}"
        fi
        if [ "${waited}" -ge "${GUEST_READY_TIMEOUT}" ]; then
            echo "HOST_FAIL_LVM2_RESIZE ${label}_guest_shell_timeout=${GUEST_READY_TIMEOUT}s"
            exec 3>&-
            kill -- "-${qemu_pid}" 2>/dev/null || kill "${qemu_pid}" 2>/dev/null || true
            wait "${qemu_pid}" || true
            return 124
        fi
        sleep 1
        waited=$((waited + 1))
    done

    echo "HOST_INFO_LVM2_RESIZE ${label}_guest_ready_after=${waited}s"
    cat "${script_file}" >&3
    exec 3>&-

    if wait "${qemu_pid}"; then
        return 0
    else
        return $?
    fi
}

FIRST_GUEST_SCRIPT=$(mktemp /tmp/lvm2-resize-first.XXXXXX)
cat >"${FIRST_GUEST_SCRIPT}" <<'GUEST_FIRST'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_LVM2_RESIZE_FIRST status=$status; sync; poweroff; exit $status' ERR

echo '=== STEP 1: locate test disks ==='
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "$TEST_DISK" "$TEST_DISK2"
ls -l "$TEST_DISK" "$TEST_DISK2"
test "$TEST_DISK" != "$TEST_DISK2"
test -b "$TEST_DISK"
test -b "$TEST_DISK2"

LVM_CONFIG='activation { udev_rules=0 }'

echo '=== STEP 2: create PV/VG and 400M LV ==='
pvcreate "$TEST_DISK" "$TEST_DISK2"
vgcreate test_vg "$TEST_DISK" "$TEST_DISK2"
lvcreate --config "$LVM_CONFIG" --type linear -L 400M -n test_lv test_vg "$TEST_DISK"
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,vg_name,lv_size,lv_attr,devices
lvs --segments -o lv_name,seg_start,seg_size,devices test_vg/test_lv

echo '=== CHECK 1: initial dm table is 400M ==='
echo 'DM_TABLE_LVM2_INITIAL_BEGIN'
dmsetup table test_vg-test_lv | tee /tmp/initial-table.txt | sed 's/^/DM_TABLE_LVM2_INITIAL /'
echo 'DM_TABLE_LVM2_INITIAL_END'
test "$(wc -l < /tmp/initial-table.txt)" -eq 1
awk '{ sum += $2 } END { exit !(sum == 819200) }' /tmp/initial-table.txt
awk '{ if ($3 != "linear") exit 1 }' /tmp/initial-table.txt

echo '=== STEP 3: mkfs, mount, write hello ==='
MAPPER_DEVICE=/dev/mapper/test_vg-test_lv
mkfs.ext2 -F -b 4096 "$MAPPER_DEVICE"
blkid "$MAPPER_DEVICE"
mkdir -p /mnt/dmtest
mount -t ext2 "$MAPPER_DEVICE" /mnt/dmtest
printf 'hello world\n' > /mnt/dmtest/hello.txt
cat /mnt/dmtest/hello.txt
du -sh /mnt/dmtest /mnt/dmtest/hello.txt
sync

echo '=== STEP 4: extend LV to 700M while mounted ==='
lvextend --config "$LVM_CONFIG" -L 700M test_vg/test_lv "$TEST_DISK2"
lvs --segments -o lv_name,seg_start,seg_size,devices test_vg/test_lv
echo 'DM_TABLE_LVM2_EXTENDED_BEGIN'
dmsetup table test_vg-test_lv | tee /tmp/extended-table.txt | sed 's/^/DM_TABLE_LVM2_EXTENDED /'
echo 'DM_TABLE_LVM2_EXTENDED_END'
cat /mnt/dmtest/hello.txt
du -sh /mnt/dmtest /mnt/dmtest/hello.txt
sync

echo '=== CHECK 2: extended dm table crosses PVs ==='
test "$(wc -l < /tmp/extended-table.txt)" -ge 2
awk '{ sum += $2 } END { exit !(sum == 1433600) }' /tmp/extended-table.txt
awk '{ if ($3 != "linear") exit 1 }' /tmp/extended-table.txt
awk '{ dev[$4] = 1 } END { count = 0; for (d in dev) count++; exit !(count >= 2) }' /tmp/extended-table.txt

echo '=== STEP 5: offline grow ext2 and write grow.txt ==='
umount /mnt/dmtest
e2fsck -f -y "$MAPPER_DEVICE"
resize2fs "$MAPPER_DEVICE"
e2fsck -f -y "$MAPPER_DEVICE"
mount -t ext2 "$MAPPER_DEVICE" /mnt/dmtest
cat /mnt/dmtest/hello.txt
printf 'after grow\n' > /mnt/dmtest/grow.txt
cat /mnt/dmtest/grow.txt
du -sh /mnt/dmtest /mnt/dmtest/hello.txt /mnt/dmtest/grow.txt
sync

echo '=== STEP 6: shrink ext2 and LV to 300M ==='
umount /mnt/dmtest
e2fsck -f -y "$MAPPER_DEVICE"
resize2fs "$MAPPER_DEVICE" 300M
e2fsck -f -y "$MAPPER_DEVICE"
lvreduce --config "$LVM_CONFIG" -y -L 300M test_vg/test_lv
lvs --segments -o lv_name,seg_start,seg_size,devices test_vg/test_lv
echo 'DM_TABLE_LVM2_SHRUNK_BEGIN'
dmsetup table test_vg-test_lv | tee /tmp/shrunk-table.txt | sed 's/^/DM_TABLE_LVM2_SHRUNK /'
echo 'DM_TABLE_LVM2_SHRUNK_END'

echo '=== CHECK 3: shrunk dm table is 300M linear ==='
test "$(wc -l < /tmp/shrunk-table.txt)" -eq 1
awk '{ sum += $2 } END { exit !(sum == 614400) }' /tmp/shrunk-table.txt
awk '{ if ($3 != "linear") exit 1 }' /tmp/shrunk-table.txt
mount -t ext2 "$MAPPER_DEVICE" /mnt/dmtest
cat /mnt/dmtest/hello.txt
cat /mnt/dmtest/grow.txt
du -sh /mnt/dmtest /mnt/dmtest/hello.txt /mnt/dmtest/grow.txt
sync

echo '=== STEP 7: cleanup first guest ==='
umount /mnt/dmtest
vgchange --config "$LVM_CONFIG" -an test_vg
sync
echo TEST_PASS_LVM2_RESIZE_FIRST
poweroff
GUEST_FIRST

SECOND_GUEST_SCRIPT=$(mktemp /tmp/lvm2-resize-second.XXXXXX)
cat >"${SECOND_GUEST_SCRIPT}" <<'GUEST_SECOND'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_LVM2_RESIZE_SECOND status=$status; sync; poweroff; exit $status' ERR

echo '=== STEP 8: recover VG/LV after reboot ==='
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "$TEST_DISK" "$TEST_DISK2"
test "$TEST_DISK" != "$TEST_DISK2"
LVM_CONFIG='activation { udev_rules=0 }'
pvscan
vgscan
vgchange --config "$LVM_CONFIG" -ay test_vg
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,vg_name,lv_size,lv_attr,devices
lvs --segments -o lv_name,seg_start,seg_size,devices test_vg/test_lv
echo 'DM_TABLE_LVM2_RECOVERED_BEGIN'
dmsetup table test_vg-test_lv | tee /tmp/recovered-table.txt | sed 's/^/DM_TABLE_LVM2_RECOVERED /'
echo 'DM_TABLE_LVM2_RECOVERED_END'
ls -l /dev/dm-* /dev/mapper /dev/test_vg

echo '=== CHECK 4: recovered dm table is 300M linear ==='
test "$(wc -l < /tmp/recovered-table.txt)" -eq 1
awk '{ sum += $2 } END { exit !(sum == 614400) }' /tmp/recovered-table.txt
awk '{ if ($3 != "linear") exit 1 }' /tmp/recovered-table.txt

echo '=== STEP 9: readonly mount and read files ==='
mkdir -p /mnt/dmtest
mount -t ext2 -o ro /dev/mapper/test_vg-test_lv /mnt/dmtest
cat /mnt/dmtest/hello.txt
cat /mnt/dmtest/grow.txt
du -sh /mnt/dmtest /mnt/dmtest/hello.txt /mnt/dmtest/grow.txt
umount /mnt/dmtest
vgchange --config "$LVM_CONFIG" -an test_vg
sync
echo TEST_PASS_LVM2_RESIZE_SECOND
poweroff
GUEST_SECOND

print_summary() {
    echo "HOST_INFO_LVM2_RESIZE summary:"
    grep -aE 'TEST_|=== STEP|=== CHECK|TEST_DISK=|TEST_DISK2=|DM_TABLE_LVM2|test_vg|test_lv|linear|hello world|after grow|No space left|Input/output error|Command failed|Kernel panic|panicked|records in|records out|bytes .* copied' "${LOG}" \
        | grep -avE 'root@asterinas|^\+ |^echo |^trap |^set |^test |^grep |^awk |^dmsetup |^tee |^sed |^cat |^printf |^mount |^umount |^vgchange |^vgscan |^pvscan |^pvs |^vgs |^lvs |^df |^du |^md5sum |^mkdir |^sync |^poweroff |^LVM_CONFIG=|^MAPPER_DEVICE=' \
        | awk 'BEGIN { seen = 0 } /^=== STEP|^=== CHECK|^DM_TABLE_.*_BEGIN/ { if (seen) print ""; seen = 1 } { print } /^DM_TABLE_.*_END/ { print "" }' \
        || true
}

print_failure_context() {
    echo "HOST_INFO_LVM2_RESIZE log=${LOG}"
    if [ ! -f "${LOG}" ]; then
        echo "HOST_INFO_LVM2_RESIZE missing_log=${LOG}"
        return
    fi
    if grep -aq 'Error 65' "${LOG}"; then
        echo "HOST_INFO_LVM2_RESIZE qemu_error_65=asterinas_exit_failure_before_guest_shell"
    fi
    grep -aE 'TEST_FAIL|HOST_FAIL|ERROR:|assertion failed|panic|panicked|Error [0-9]+|No space left|Input/output error|Command failed' "${LOG}" | tail -n 40 || true
    if [ -f qemu-serial.log ]; then
        grep -aE 'ERROR:|assertion failed|panic|panicked' qemu-serial.log | tail -n 20 || true
    fi
}

FIRST_STATUS=0
run_guest_script "${FIRST_GUEST_SCRIPT}" replace first || FIRST_STATUS=$?
rm -f "${FIRST_GUEST_SCRIPT}"
if [ "${FIRST_STATUS}" -ne 0 ]; then
    print_summary
    print_failure_context
    echo "HOST_FAIL_LVM2_RESIZE first_qemu_status=${FIRST_STATUS}"
    rm -f "${SECOND_GUEST_SCRIPT}"
    exit "${FIRST_STATUS}"
fi

grep -q TEST_PASS_LVM2_RESIZE_FIRST "${LOG}"

SECOND_STATUS=0
run_guest_script "${SECOND_GUEST_SCRIPT}" append second || SECOND_STATUS=$?
rm -f "${SECOND_GUEST_SCRIPT}"

print_summary

if [ "${SECOND_STATUS}" -ne 0 ]; then
    print_failure_context
    echo "HOST_FAIL_LVM2_RESIZE second_qemu_status=${SECOND_STATUS}"
    exit "${SECOND_STATUS}"
fi

grep -q TEST_PASS_LVM2_RESIZE_SECOND "${LOG}"
echo HOST_PASS_LVM2_RESIZE
