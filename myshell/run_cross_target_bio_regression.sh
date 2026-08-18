#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

# 这个脚本在 Asterinas NixOS guest 中回归测试 Device Mapper 的跨 target BIO 数据面。
#
# 测试方法：
# 1. host 侧启动 `make run_nixos`，通过 DM_TEST_IMAGE 和 DM_TEST_IMAGE_2
#    附加两块独立 raw 测试盘；
# 2. host 侧持续观察 QEMU 日志，检测到 guest root shell 提示符后再注入测试命令，
#    不再固定 sleep 等待；
# 3. guest 通过 `aster-dm-disk-locator` 找到两块测试盘，并用 `stat` 从
#    `/dev/vdX` 设备节点取得 major:minor；
# 4. guest 创建一个 8-sector 的两段 linear table：0..4 sectors 映射到第一块盘，
#    4..8 sectors 映射到第二块盘，target 边界正好在 2 KiB 处；
# 5. guest 向 `/dev/mapper/cross_bio_test` 写入一次 4 KiB payload，再从同一个
#    mapper 设备读回一次 4 KiB payload；这次 4 KiB BIO 覆盖 0..8 sectors，
#    必然跨过两个 linear target；
# 6. guest 用 md5sum 校验 mapper 读回内容，再分别读取两块 backing 盘的前
#    2048 字节，确认第一块盘收到 payload 前半段，第二块盘收到后半段。
#
# 被验证的功能：
# - 跨 target BIO 会被拆成多个 child BIO；
# - child BIO 会 remap 到正确 backing device；
# - 原始 BIO completion 会在所有 child 完成后聚合完成；
# - raw block 读写路径都能跨 target 边界工作。
#
# 默认会删除 DM_TEST_IMAGE / DM_TEST_IMAGE_2 指向的测试盘镜像，以保证测试从空盘开始。
# 结果判断：全量日志写入 CROSS_TARGET_BIO_LOG；summary 只保留步骤、检查点、
# CHECK_PASS、TEST_PASS/HOST_PASS 和失败信息。

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/run_cross_target_bio_regression.sh

Runs a NixOS guest regression for a single 4 KiB BIO crossing two linear targets.

Optional environment variables:
  DM_TEST_IMAGE              First backing test image path
  DM_TEST_IMAGE_2            Second backing test image path
  CROSS_TARGET_BIO_LOG       Host-side log path inside the container
  GUEST_READY_TIMEOUT        Seconds to wait for guest root shell, default 120
  RESET_DM_TEST_IMAGES       1 to delete test images before running, default 1
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/..")
LOG=${CROSS_TARGET_BIO_LOG:-/tmp/cross-target-bio-regression.log}
DM_TEST_IMAGE=${DM_TEST_IMAGE:-target/nixos/test.img}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-target/nixos/test2.img}
GUEST_READY_TIMEOUT=${GUEST_READY_TIMEOUT:-120}
RESET_DM_TEST_IMAGES=${RESET_DM_TEST_IMAGES:-1}

cd "${ASTERINAS_DIR}"
rm -f "${LOG}"

echo "HOST_INFO_CROSS_TARGET_BIO started"
echo "HOST_INFO_CROSS_TARGET_BIO log=${LOG}"
echo "HOST_INFO_CROSS_TARGET_BIO disk1=${DM_TEST_IMAGE}"
echo "HOST_INFO_CROSS_TARGET_BIO disk2=${DM_TEST_IMAGE_2}"

if pgrep -af qemu-system | grep -v 'pgrep -af qemu-system' >/tmp/cross-target-bio-qemu-running.txt 2>/dev/null; then
    echo "HOST_FAIL_CROSS_TARGET_BIO existing_qemu"
    cat /tmp/cross-target-bio-qemu-running.txt
    exit 1
fi

if [ ! -f target/nixos/asterinas.img ]; then
    echo "HOST_FAIL_CROSS_TARGET_BIO missing target/nixos/asterinas.img; run 'make nixos' first"
    exit 1
fi

if [ "${RESET_DM_TEST_IMAGES}" = "1" ]; then
    rm -f "${DM_TEST_IMAGE}" "${DM_TEST_IMAGE_2}"
fi

GUEST_SCRIPT_FILE=$(mktemp /tmp/cross-target-bio-guest.XXXXXX)
cat >"${GUEST_SCRIPT_FILE}" <<'GUEST_SCRIPT'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_CROSS_TARGET_BIO status=$status; sync; poweroff; exit $status' ERR

echo '=== STEP 1: locate test disks ==='
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "$TEST_DISK" "$TEST_DISK2"
test "$TEST_DISK" != "$TEST_DISK2"
test -b "$TEST_DISK"
test -b "$TEST_DISK2"

DEV1=$(printf '%d:%d' "0x$(stat -c '%t' "$TEST_DISK")" "0x$(stat -c '%T' "$TEST_DISK")")
DEV2=$(printf '%d:%d' "0x$(stat -c '%t' "$TEST_DISK2")" "0x$(stat -c '%T' "$TEST_DISK2")")
printf 'DEV1=%s\nDEV2=%s\n' "$DEV1" "$DEV2"

echo '=== STEP 2: create two-target linear table ==='
dmsetup remove cross_bio_test >/dev/null 2>&1 || true

dd if=/dev/zero of="$TEST_DISK" bs=4096 count=1 conv=fsync status=none
dd if=/dev/zero of="$TEST_DISK2" bs=4096 count=1 conv=fsync status=none
printf '0 4 linear %s 0\n4 4 linear %s 0\n' "$DEV1" "$DEV2" | dmsetup create cross_bio_test

echo 'DM_TABLE_CROSS_BIO_BEGIN'
dmsetup table cross_bio_test | tee /tmp/cross-table.txt | sed 's/^/DM_TABLE_CROSS_BIO /'
echo 'DM_TABLE_CROSS_BIO_END'
test "$(wc -l < /tmp/cross-table.txt)" -eq 2
grep -F -x -q "0 4 linear $DEV1 0" /tmp/cross-table.txt
grep -F -x -q "4 4 linear $DEV2 0" /tmp/cross-table.txt
awk '{ sum += $2 } END { exit !(sum == 8) }' /tmp/cross-table.txt

echo '=== STEP 3: issue one 4 KiB BIO across the 2 KiB target boundary ==='
head -c 4096 /dev/zero | tr '\000' '\132' > /tmp/cross-pattern.bin
head -c 2048 /tmp/cross-pattern.bin > /tmp/cross-expect-half.bin

dd if=/tmp/cross-pattern.bin of=/dev/mapper/cross_bio_test bs=4096 count=1 conv=fsync status=none
dd if=/dev/mapper/cross_bio_test of=/tmp/cross-readback.bin bs=4096 count=1 status=none

echo '=== CHECK 1: mapper readback md5 equals original payload ==='
md5sum /tmp/cross-pattern.bin /tmp/cross-readback.bin > /tmp/cross-readback.md5
awk 'NR == 1 { expected = $1 } NR > 1 && $1 != expected { exit 1 }' /tmp/cross-readback.md5
echo CHECK_PASS_MAPPER_READBACK_MD5

echo '=== CHECK 2: each backing disk received exactly one half ==='
dd if="$TEST_DISK" of=/tmp/cross-first.bin bs=2048 count=1 status=none
dd if="$TEST_DISK2" of=/tmp/cross-second.bin bs=2048 count=1 status=none
md5sum /tmp/cross-expect-half.bin /tmp/cross-first.bin /tmp/cross-second.bin > /tmp/cross-backing.md5
awk 'NR == 1 { expected = $1 } NR > 1 && $1 != expected { exit 1 }' /tmp/cross-backing.md5
echo CHECK_PASS_BACKING_SPLIT_MD5

echo '=== STEP 4: cleanup ==='
dmsetup remove cross_bio_test
sync

echo TEST_PASS_CROSS_TARGET_BIO
poweroff
GUEST_SCRIPT

run_guest_script() {
    fifo=$(mktemp -u /tmp/cross-target-bio-stdin.XXXXXX)
    mkfifo "${fifo}"

    DM_TEST_IMAGE="${DM_TEST_IMAGE}" \
    DM_TEST_IMAGE_2="${DM_TEST_IMAGE_2}" \
    setsid make run_nixos <"${fifo}" >>"${LOG}" 2>&1 &
    qemu_pid=$!

    exec 3>"${fifo}"
    rm -f "${fifo}"

    waited=0
    while ! grep -aq 'root@asterinas' "${LOG}"; do
        if ! kill -0 "${qemu_pid}" 2>/dev/null; then
            status=0
            wait "${qemu_pid}" || status=$?
            exec 3>&-
            if [ "${status}" -eq 0 ]; then
                status=1
            fi
            echo "HOST_FAIL_CROSS_TARGET_BIO guest_exited_before_shell status=${status}"
            return "${status}"
        fi
        if [ "${waited}" -ge "${GUEST_READY_TIMEOUT}" ]; then
            echo "HOST_FAIL_CROSS_TARGET_BIO guest_shell_timeout=${GUEST_READY_TIMEOUT}s"
            exec 3>&-
            kill -- "-${qemu_pid}" 2>/dev/null || kill "${qemu_pid}" 2>/dev/null || true
            wait "${qemu_pid}" || true
            return 124
        fi
        sleep 1
        waited=$((waited + 1))
    done

    echo "HOST_INFO_CROSS_TARGET_BIO guest_ready_after=${waited}s"
    cat "${GUEST_SCRIPT_FILE}" >&3
    exec 3>&-

    wait "${qemu_pid}"
}

RUN_STATUS=0
run_guest_script || RUN_STATUS=$?
rm -f "${GUEST_SCRIPT_FILE}"

echo "HOST_INFO_CROSS_TARGET_BIO summary:"
grep -aE 'TEST_|CHECK_PASS_|=== STEP|=== CHECK|TEST_DISK=|TEST_DISK2=|DEV1=|DEV2=|DM_TABLE_CROSS_BIO|Command failed|Kernel panic|panicked' "${LOG}" \
    | grep -avE 'root@asterinas|^\+ |^echo |^trap |^set |^cmp |^dd |^grep |^dmsetup |^head |^poweroff |^md5sum |^awk ' \
    | awk 'BEGIN { seen = 0 } /^=== STEP|^=== CHECK|^DM_TABLE_.*_BEGIN/ { if (seen) print ""; seen = 1 } { print } /^DM_TABLE_.*_END/ { print "" }' \
    || true

if [ "${RUN_STATUS}" -ne 0 ]; then
    echo "HOST_INFO_CROSS_TARGET_BIO log=${LOG}"
    if [ -f "${LOG}" ]; then
        if grep -aq 'Error 65' "${LOG}"; then
            echo "HOST_INFO_CROSS_TARGET_BIO qemu_error_65=asterinas_exit_failure_before_guest_shell"
        fi
        grep -aE 'TEST_FAIL|HOST_FAIL|ERROR:|assertion failed|panic|panicked|Error [0-9]+|No space left|Input/output error|Command failed' "${LOG}" | tail -n 40 || true
        if [ -f qemu-serial.log ]; then
            grep -aE 'ERROR:|assertion failed|panic|panicked' qemu-serial.log | tail -n 20 || true
        fi
    else
        echo "HOST_INFO_CROSS_TARGET_BIO missing_log=${LOG}"
    fi
    echo "HOST_FAIL_CROSS_TARGET_BIO qemu_status=${RUN_STATUS}"
    exit "${RUN_STATUS}"
fi

grep -q TEST_PASS_CROSS_TARGET_BIO "${LOG}"
echo HOST_PASS_CROSS_TARGET_BIO
