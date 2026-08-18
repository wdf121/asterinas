#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

# 这个脚本一键回归测试 Asterinas NixOS 中的跨 PV 大文件数据路径。
#
# 测试方法：
# 1. host 侧清空两块 DM 测试盘镜像，使用 DM_TEST_IMAGE / DM_TEST_IMAGE_2
#    显式附加两块 512 MiB raw 盘启动 `make run_nixos`；
# 2. host 侧检测到 guest root shell 提示符后再注入测试命令，不再固定 sleep；
# 3. 第一台 guest 中创建两个 PV、一个 VG、一个 900 MiB linear LV；
# 4. 由于单块 PV 可用空间约 508 MiB，900 MiB LV 必然跨到第二块 PV；
# 5. 在该 LV 上创建 4 KiB block size 的 ext2，直接写入一个 700 MiB 随机文件；
# 6. 把文件 md5 保存到同一个 ext2 文件系统中，并立即执行 `md5sum -c`；
# 7. 第二台 guest 复用同一组测试盘，重新激活 VG/LV，只读挂载后再次 `md5sum -c`。
#
# 被验证的功能：
# - LVM2 能创建跨 PV 的多段 linear LV；
# - Device Mapper 能加载并使用跨 PV 的多段 linear table；
# - ext2 大文件写入会真实经过跨 PV 数据路径；
# - 文件内容和 LVM 元数据能跨 QEMU 重启恢复。
#
# 注意：脚本不会执行 `make nixos`。首次运行前请先在容器内执行 `make nixos`。
# 默认会删除 DM_TEST_IMAGE / DM_TEST_IMAGE_2 指向的测试盘镜像，以保证测试从空盘开始。
# 全量日志写入 CROSS_PV_LARGE_WRITE_LOG；成功时打印 HOST_PASS_CROSS_PV_LARGE_WRITE。

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/run_cross_pv_large_write_test.sh

Runs the two-PV large-file regression in an Asterinas NixOS guest.

Optional environment variables:
  DM_TEST_IMAGE              First backing test image path, default target/nixos/test.img
  DM_TEST_IMAGE_2            Second backing test image path, default target/nixos/test2.img
  CROSS_PV_LARGE_WRITE_LOG   Host-side log path, default /tmp/cross-pv-large-write-test.log
  GUEST_READY_TIMEOUT        Seconds to wait for guest root shell, default 120
  RESET_DM_TEST_IMAGES       1 to delete test images before running, default 1
  LARGE_WRITE_MIB            File size in MiB, default 700
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/..")
LOG=${CROSS_PV_LARGE_WRITE_LOG:-/tmp/cross-pv-large-write-test.log}
DM_TEST_IMAGE=${DM_TEST_IMAGE:-target/nixos/test.img}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-target/nixos/test2.img}
GUEST_READY_TIMEOUT=${GUEST_READY_TIMEOUT:-120}
RESET_DM_TEST_IMAGES=${RESET_DM_TEST_IMAGES:-1}
LARGE_WRITE_MIB=${LARGE_WRITE_MIB:-700}

cd "${ASTERINAS_DIR}"
rm -f "${LOG}"

echo "HOST_INFO_CROSS_PV_LARGE_WRITE started"
echo "HOST_INFO_CROSS_PV_LARGE_WRITE log=${LOG}"
echo "HOST_INFO_CROSS_PV_LARGE_WRITE disk1=${DM_TEST_IMAGE} serial=vdmtest"
echo "HOST_INFO_CROSS_PV_LARGE_WRITE disk2=${DM_TEST_IMAGE_2} serial=vdmtest2"
echo "HOST_INFO_CROSS_PV_LARGE_WRITE file_mib=${LARGE_WRITE_MIB}"

if pgrep -af qemu-system | grep -v 'pgrep -af qemu-system' >/tmp/cross-pv-large-qemu-running.txt 2>/dev/null; then
    echo "HOST_FAIL_CROSS_PV_LARGE_WRITE existing_qemu"
    cat /tmp/cross-pv-large-qemu-running.txt
    exit 1
fi

if [ ! -f target/nixos/asterinas.img ]; then
    echo "HOST_FAIL_CROSS_PV_LARGE_WRITE missing target/nixos/asterinas.img; run 'make nixos' first"
    exit 1
fi

if [ "${RESET_DM_TEST_IMAGES}" = "1" ]; then
    rm -f "${DM_TEST_IMAGE}" "${DM_TEST_IMAGE_2}"
fi

run_guest_script() {
    script_file=$1
    log_mode=$2
    label=$3
    fifo=$(mktemp -u /tmp/cross-pv-large-stdin.XXXXXX)
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
            echo "HOST_FAIL_CROSS_PV_LARGE_WRITE ${label}_guest_exited_before_shell status=${status}"
            return "${status}"
        fi
        if [ "${waited}" -ge "${GUEST_READY_TIMEOUT}" ]; then
            echo "HOST_FAIL_CROSS_PV_LARGE_WRITE ${label}_guest_shell_timeout=${GUEST_READY_TIMEOUT}s"
            exec 3>&-
            kill -- "-${qemu_pid}" 2>/dev/null || kill "${qemu_pid}" 2>/dev/null || true
            wait "${qemu_pid}" || true
            return 124
        fi
        sleep 1
        waited=$((waited + 1))
    done

    echo "HOST_INFO_CROSS_PV_LARGE_WRITE ${label}_guest_ready_after=${waited}s"
    cat "${script_file}" >&3
    exec 3>&-

    if wait "${qemu_pid}"; then
        return 0
    else
        return $?
    fi
}

FIRST_GUEST_SCRIPT=$(mktemp /tmp/cross-pv-large-first.XXXXXX)
{
    printf 'LARGE_WRITE_MIB=%q\n' "${LARGE_WRITE_MIB}"
    cat <<'GUEST_FIRST'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_CROSS_PV_LARGE_WRITE_FIRST status=$status; sync; poweroff; exit $status' ERR

echo '=== STEP 1: locate test disks ==='
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "$TEST_DISK" "$TEST_DISK2"
test "$TEST_DISK" != "$TEST_DISK2"
test -b "$TEST_DISK"
test -b "$TEST_DISK2"

LVM_CONFIG='activation { udev_rules=0 }'

echo '=== STEP 2: create PV/VG and a 900M cross-PV LV ==='
pvcreate "$TEST_DISK" "$TEST_DISK2"
vgcreate large_vg "$TEST_DISK" "$TEST_DISK2"
lvcreate --config "$LVM_CONFIG" --type linear -L 900M -n large_lv large_vg "$TEST_DISK" "$TEST_DISK2"
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,vg_name,lv_size,lv_attr,devices
lvs --segments -o lv_name,seg_start,seg_size,devices large_vg/large_lv

echo '=== CHECK 1: dm table crosses at least two linear targets ==='
echo 'DM_TABLE_CROSS_PV_LARGE_BEGIN'
dmsetup table large_vg-large_lv | tee /tmp/large-table.txt | sed 's/^/DM_TABLE_CROSS_PV_LARGE /'
echo 'DM_TABLE_CROSS_PV_LARGE_END'
test "$(wc -l < /tmp/large-table.txt)" -ge 2
awk '{ sum += $2 } END { exit !(sum == 1843200) }' /tmp/large-table.txt
awk '{ dev[$4] = 1 } END { count = 0; for (d in dev) count++; exit !(count >= 2) }' /tmp/large-table.txt

echo '=== STEP 3: mkfs, mount, write one large file ==='
MAPPER_DEVICE=/dev/mapper/large_vg-large_lv
mkfs.ext2 -F -b 4096 "$MAPPER_DEVICE"
blkid "$MAPPER_DEVICE"
mkdir -p /mnt/dmlarge
mount -t ext2 "$MAPPER_DEVICE" /mnt/dmlarge
dd if=/dev/urandom of=/mnt/dmlarge/file700.bin bs=1M count="$LARGE_WRITE_MIB" conv=fsync
(
  cd /mnt/dmlarge
  md5sum file700.bin > file700.md5
  cat file700.md5
  md5sum -c file700.md5
)
df -h /mnt/dmlarge
du -sh /mnt/dmlarge /mnt/dmlarge/file700.bin
sync

echo '=== STEP 4: cleanup first guest ==='
umount /mnt/dmlarge
vgchange --config "$LVM_CONFIG" -an large_vg
sync
echo TEST_PASS_CROSS_PV_LARGE_WRITE_FIRST
poweroff
GUEST_FIRST
} >"${FIRST_GUEST_SCRIPT}"

SECOND_GUEST_SCRIPT=$(mktemp /tmp/cross-pv-large-second.XXXXXX)
cat >"${SECOND_GUEST_SCRIPT}" <<'GUEST_SECOND'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_CROSS_PV_LARGE_WRITE_SECOND status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'

echo '=== STEP 5: reactivate VG/LV after reboot ==='
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "$TEST_DISK" "$TEST_DISK2"
vgscan --mknodes
vgchange --config "$LVM_CONFIG" -ay large_vg
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs --segments -o lv_name,seg_start,seg_size,devices large_vg/large_lv

echo '=== CHECK 2: recovered dm table still crosses PVs ==='
echo 'DM_TABLE_CROSS_PV_LARGE_RECOVERED_BEGIN'
dmsetup table large_vg-large_lv | tee /tmp/large-table-recovered.txt | sed 's/^/DM_TABLE_CROSS_PV_LARGE_RECOVERED /'
echo 'DM_TABLE_CROSS_PV_LARGE_RECOVERED_END'
test "$(wc -l < /tmp/large-table-recovered.txt)" -ge 2
awk '{ sum += $2 } END { exit !(sum == 1843200) }' /tmp/large-table-recovered.txt
awk '{ dev[$4] = 1 } END { count = 0; for (d in dev) count++; exit !(count >= 2) }' /tmp/large-table-recovered.txt

echo '=== CHECK 3: recovered file md5 is still valid ==='
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
umount /mnt/dmlarge
vgchange --config "$LVM_CONFIG" -an large_vg
sync
echo TEST_PASS_CROSS_PV_LARGE_WRITE_SECOND
poweroff
GUEST_SECOND

print_summary() {
    echo "HOST_INFO_CROSS_PV_LARGE_WRITE summary:"
    grep -aE 'TEST_|=== STEP|=== CHECK|TEST_DISK=|TEST_DISK2=|DM_TABLE_CROSS_PV_LARGE|large_vg|large_lv|linear|file700\.bin|OK|No space left|Input/output error|Command failed|Kernel panic|panicked|records in|records out|bytes .* copied' "${LOG}" \
        | grep -avE 'root@asterinas|^\+ |^echo |^trap |^set |^test |^grep |^awk |^dmsetup |^tee |^sed |^cat |^printf |^mount |^umount |^vgchange |^vgscan |^pvs |^vgs |^lvs |^df |^du |^md5sum |^mkdir |^sync |^poweroff |^LVM_CONFIG=|^MAPPER_DEVICE=' \
        | awk 'BEGIN { seen = 0 } /^=== STEP|^=== CHECK|^DM_TABLE_.*_BEGIN/ { if (seen) print ""; seen = 1 } { print } /^DM_TABLE_.*_END/ { print "" }' \
        || true
}

print_failure_context() {
    echo "HOST_INFO_CROSS_PV_LARGE_WRITE log=${LOG}"
    if [ ! -f "${LOG}" ]; then
        echo "HOST_INFO_CROSS_PV_LARGE_WRITE missing_log=${LOG}"
        return
    fi
    if grep -aq 'Error 65' "${LOG}"; then
        echo "HOST_INFO_CROSS_PV_LARGE_WRITE qemu_error_65=asterinas_exit_failure_before_guest_shell"
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
    echo "HOST_FAIL_CROSS_PV_LARGE_WRITE first_qemu_status=${FIRST_STATUS}"
    rm -f "${SECOND_GUEST_SCRIPT}"
    exit "${FIRST_STATUS}"
fi

grep -q TEST_PASS_CROSS_PV_LARGE_WRITE_FIRST "${LOG}"

SECOND_STATUS=0
run_guest_script "${SECOND_GUEST_SCRIPT}" append second || SECOND_STATUS=$?
rm -f "${SECOND_GUEST_SCRIPT}"

print_summary

if [ "${SECOND_STATUS}" -ne 0 ]; then
    print_failure_context
    echo "HOST_FAIL_CROSS_PV_LARGE_WRITE second_qemu_status=${SECOND_STATUS}"
    exit "${SECOND_STATUS}"
fi

grep -q TEST_PASS_CROSS_PV_LARGE_WRITE_SECOND "${LOG}"
echo HOST_PASS_CROSS_PV_LARGE_WRITE
