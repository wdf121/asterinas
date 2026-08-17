#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

# 这个脚本在 Asterinas NixOS guest 中回归测试 Device Mapper 的跨 target BIO 数据面。
#
# 测试方法：
# 1. host 侧启动 `make run_nixos`，通过 DM_TEST_IMAGE 和 DM_TEST_IMAGE_2
#    附加两块独立 raw 测试盘；
# 2. 等待 guest 自动登录 root shell 后，把下面的 GUEST_SCRIPT 注入 guest 执行；
# 3. guest 通过 `aster-dm-disk-locator` 找到两块测试盘，并用 `stat` 从
#    `/dev/vdX` 设备节点取得 major:minor；
# 4. guest 创建一个 8-sector 的两段 linear table：0..4 sectors 映射到第一块盘，
#    4..8 sectors 映射到第二块盘，target 边界正好在 2 KiB 处；
# 5. guest 向 `/dev/mapper/cross_bio_test` 写入一次 4 KiB payload，再从同一个
#    mapper 设备读回一次 4 KiB payload；这次 4 KiB BIO 覆盖 0..8 sectors，
#    必然跨过两个 linear target；
# 6. guest 先比较 mapper 读回内容与原始 payload，再分别读取两块 backing 盘
#    的前 2048 字节，确认第一块盘收到 payload 前半段，第二块盘收到后半段。
#
# 被验证的功能：
# - 跨 target BIO 会被拆成多个 child BIO；
# - child BIO 会 remap 到正确 backing device；
# - 原始 BIO completion 会在所有 child 完成后聚合完成；
# - raw block 读写路径都能跨 target 边界工作。
#
# 结果判断：全量日志写入 CROSS_TARGET_BIO_LOG；guest 成功时打印
# TEST_PASS_CROSS_TARGET_BIO，host 确认后打印 HOST_PASS_CROSS_TARGET_BIO。

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: tools/nixos/run_cross_target_bio_regression.sh

Runs a NixOS guest regression for a single 4 KiB BIO crossing two linear targets.

Optional environment variables:
  DM_TEST_IMAGE              First backing test image path
  DM_TEST_IMAGE_2            Second backing test image path
  CROSS_TARGET_BIO_LOG       Host-side log path inside the container
  GUEST_INPUT_DELAY          Seconds to wait before feeding commands to guest
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/../..")
LOG=${CROSS_TARGET_BIO_LOG:-/tmp/cross-target-bio-regression.log}
DM_TEST_IMAGE=${DM_TEST_IMAGE:-target/nixos/test.img}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-target/nixos/test2.img}
GUEST_INPUT_DELAY=${GUEST_INPUT_DELAY:-180}

cd "${ASTERINAS_DIR}"
rm -f "${LOG}"

echo "HOST_INFO_CROSS_TARGET_BIO started"
echo "HOST_INFO_CROSS_TARGET_BIO log=${LOG}"
echo "HOST_INFO_CROSS_TARGET_BIO disk1=${DM_TEST_IMAGE}"
echo "HOST_INFO_CROSS_TARGET_BIO disk2=${DM_TEST_IMAGE_2}"

RUN_STATUS=0
((sleep "${GUEST_INPUT_DELAY}"; cat) | \
    DM_TEST_IMAGE="${DM_TEST_IMAGE}" \
    DM_TEST_IMAGE_2="${DM_TEST_IMAGE_2}" \
    make run_nixos) >"${LOG}" 2>&1 <<'GUEST_SCRIPT' || RUN_STATUS=$?
set -eux
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
dmsetup remove cross_bio_test || true

dd if=/dev/zero of="$TEST_DISK" bs=4096 count=1 conv=fsync
dd if=/dev/zero of="$TEST_DISK2" bs=4096 count=1 conv=fsync
printf '0 4 linear %s 0\n4 4 linear %s 0\n' "$DEV1" "$DEV2" | dmsetup create cross_bio_test

dmsetup table cross_bio_test | tee /tmp/cross-table.txt
grep -F -x "0 4 linear $DEV1 0" /tmp/cross-table.txt
grep -F -x "4 4 linear $DEV2 0" /tmp/cross-table.txt

echo '=== STEP 3: issue one 4 KiB BIO across the 2 KiB target boundary ==='
head -c 4096 /dev/zero | tr '\000' '\132' > /tmp/cross-pattern.bin
head -c 2048 /tmp/cross-pattern.bin > /tmp/cross-expect-half.bin

dd if=/tmp/cross-pattern.bin of=/dev/mapper/cross_bio_test bs=4096 count=1 conv=fsync
dd if=/dev/mapper/cross_bio_test of=/tmp/cross-readback.bin bs=4096 count=1

echo '=== CHECK 1: mapper readback equals original 4 KiB payload ==='
sha256sum /tmp/cross-pattern.bin /tmp/cross-readback.bin
cmp /tmp/cross-pattern.bin /tmp/cross-readback.bin

echo '=== CHECK 2: each backing disk received exactly one half ==='
dd if="$TEST_DISK" of=/tmp/cross-first.bin bs=2048 count=1
dd if="$TEST_DISK2" of=/tmp/cross-second.bin bs=2048 count=1
sha256sum /tmp/cross-expect-half.bin /tmp/cross-first.bin /tmp/cross-second.bin
cmp /tmp/cross-expect-half.bin /tmp/cross-first.bin
cmp /tmp/cross-expect-half.bin /tmp/cross-second.bin

echo '=== STEP 4: cleanup ==='
dmsetup remove cross_bio_test
sync

echo TEST_PASS_CROSS_TARGET_BIO
poweroff
GUEST_SCRIPT

echo "HOST_INFO_CROSS_TARGET_BIO summary:"
grep -aE 'TEST_|=== STEP|=== CHECK|TEST_DISK=|TEST_DISK2=|DEV1=|DEV2=|^0 4 linear |^4 4 linear |sha256sum|records in|records out|bytes .* copied|Command failed|Kernel panic|panicked' "${LOG}" || true

if [ "${RUN_STATUS}" -ne 0 ]; then
    echo "HOST_FAIL_CROSS_TARGET_BIO qemu_status=${RUN_STATUS}"
    exit "${RUN_STATUS}"
fi

grep -q TEST_PASS_CROSS_TARGET_BIO "${LOG}"
echo HOST_PASS_CROSS_TARGET_BIO
