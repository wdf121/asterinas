#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/run_dm_dataplane_test.sh

Runs one NixOS guest pass for Device Mapper data-plane semantics. It validates
large BIOs across target and stripe boundaries, non-zero backing starts,
non-aligned striped ranges, mixed target tables, and direct-complete target
behavior.

Optional environment variables:
  DM_TEST_IMAGES            Space-separated backing image paths, default four test images
  DM_DATAPLANE_LOG          Host-side log path, default /tmp/dm-dataplane-test.log
  GUEST_QEMU_TIMEOUT        Full QEMU lifecycle timeout in seconds, default 180
  GUEST_READY_TIMEOUT       Guest shell readiness timeout in seconds, default 40
  DM_DATAPLANE_STEP5_IO_TIMEOUT
                            Per-operation Step 5 I/O timeout in seconds, default 20
  RESET_DM_TEST_IMAGES      1 to delete test images before running, default 1

Expected success markers:
  TEST_PASS_DM_DATAPLANE
  HOST_PASS_DM_DATAPLANE
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/..")
source "${SCRIPT_DIR}/lib/dm_nixos_test.sh"

TEST_ID=DM_DATAPLANE
LOG=${DM_DATAPLANE_LOG:-/tmp/dm-dataplane-test.log}
DM_TEST_IMAGE=${DM_TEST_IMAGE:-target/nixos/test.img}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-target/nixos/test2.img}
DM_TEST_IMAGES=${DM_TEST_IMAGES:-target/nixos/test.img target/nixos/test2.img target/nixos/test3.img target/nixos/test4.img}
GUEST_QEMU_TIMEOUT=${GUEST_QEMU_TIMEOUT:-180}
GUEST_READY_TIMEOUT=${GUEST_READY_TIMEOUT:-40}
DM_DATAPLANE_STEP5_IO_TIMEOUT=${DM_DATAPLANE_STEP5_IO_TIMEOUT:-20}
export DM_DATAPLANE_STEP5_IO_TIMEOUT
RESET_DM_TEST_IMAGES=${RESET_DM_TEST_IMAGES:-1}

cd "${ASTERINAS_DIR}"
dm_prepare_nixos_test "${TEST_ID}"
echo "HOST_INFO_${TEST_ID} disks=${DM_TEST_IMAGES} serials=vdmtest,vdmtest2,vdmtest3,vdmtest4"
echo "HOST_INFO_${TEST_ID} qemu_lifecycle_timeout=${GUEST_QEMU_TIMEOUT}s"

GUEST_SCRIPT_FILE=$(mktemp /tmp/dm-dataplane-guest.XXXXXX)
cat >"${GUEST_SCRIPT_FILE}" <<'GUEST_SCRIPT'
stty -echo 2>/dev/null || true
set -eu

cleanup_dm() {
    for name in \
        dm_error_data \
        dm_zero_data \
        dm_mixed_data \
        dm_striped_offset \
        dm_striped_aligned \
        dm_linear_segments \
        dm_linear_cross; do
        timeout 3 dmsetup remove "${name}" >/dev/null 2>&1 || true
    done
}

fail_exit() {
    status=$?
    echo TEST_FAIL_DM_DATAPLANE status=$status
    cleanup_dm
    sync
    poweroff
    exit $status
}
trap fail_exit ERR

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

make_bytes() {
    char=$1
    bytes=$2
    output=$3
    head -c "${bytes}" /dev/zero | tr '\000' "${char}" >"${output}"
}

make_sector() {
    make_bytes "$1" 512 "$2"
}

compare_file() {
    label=$1
    expected=$2
    actual=$3
    md5sum "${expected}" "${actual}" >"/tmp/${label}.md5"
    awk 'NR == 1 { expected = $1 } NR > 1 && $1 != expected { exit 1 }' "/tmp/${label}.md5"
    echo "CHECK_PASS_${label}"
}

run_step5_io() {
    label=$1
    shift
    echo "CHECK_BEGIN_STRIPED_OFFSET_${label}"
    if timeout "${STEP5_IO_TIMEOUT}" "$@"; then
        echo "CHECK_PASS_STRIPED_OFFSET_${label}"
        return 0
    else
        status=$?
    fi

    echo "CHECK_FAIL_STRIPED_OFFSET_${label} status=${status} timeout=${STEP5_IO_TIMEOUT}s"
    timeout 3 dmsetup table dm_striped_offset || true
    timeout 3 dmsetup status dm_striped_offset || true
    return "${status}"
}

echo '=== STEP 1: locate data-plane disks and required targets ==='
test -c /dev/mapper/control
dmsetup targets | tee /tmp/dm-dataplane-targets.txt
for target in linear striped error zero; do
    grep -q "^${target}" /tmp/dm-dataplane-targets.txt
done
DISK1=$(aster-dm-disk-locator)
DISK2=$(aster-dm-disk-locator vdmtest2)
DISK3=$(aster-dm-disk-locator vdmtest3)
printf 'TEST_DISK1=%s\nTEST_DISK2=%s\nTEST_DISK3=%s\n' "$DISK1" "$DISK2" "$DISK3"
test "$DISK1" != "$DISK2"
test "$DISK1" != "$DISK3"
test "$DISK2" != "$DISK3"
test -b "$DISK1"
test -b "$DISK2"
test -b "$DISK3"
DEV1=$(devno "$DISK1")
DEV2=$(devno "$DISK2")
DEV3=$(devno "$DISK3")
printf 'DEV1=%s\nDEV2=%s\nDEV3=%s\n' "$DEV1" "$DEV2" "$DEV3"
cleanup_dm
echo CHECK_PASS_DATAPLANE_SETUP

echo '=== STEP 2: one 4 KiB BIO crosses two linear targets ==='
dd if=/dev/zero of="$DISK1" bs=4096 count=1 conv=fsync status=none
dd if=/dev/zero of="$DISK2" bs=4096 count=1 conv=fsync status=none
printf '0 4 linear %s 0\n4 4 linear %s 0\n' "$DEV1" "$DEV2" | dmsetup create dm_linear_cross
dmsetup table dm_linear_cross | tee /tmp/linear-cross-table.txt
test "$(wc -l </tmp/linear-cross-table.txt)" -eq 2
grep -F -x -q "0 4 linear $DEV1 0" /tmp/linear-cross-table.txt
grep -F -x -q "4 4 linear $DEV2 0" /tmp/linear-cross-table.txt
make_bytes Z 4096 /tmp/linear-cross-payload.bin
make_bytes Z 2048 /tmp/linear-cross-half.bin
dd if=/tmp/linear-cross-payload.bin of=/dev/mapper/dm_linear_cross bs=4096 count=1 conv=fsync status=none
dd if=/dev/mapper/dm_linear_cross of=/tmp/linear-cross-readback.bin bs=4096 count=1 status=none
dd if="$DISK1" of=/tmp/linear-cross-d1.bin bs=2048 count=1 status=none
dd if="$DISK2" of=/tmp/linear-cross-d2.bin bs=2048 count=1 status=none
compare_file LINEAR_CROSS_MAPPER /tmp/linear-cross-payload.bin /tmp/linear-cross-readback.bin
compare_file LINEAR_CROSS_BACKING_D1 /tmp/linear-cross-half.bin /tmp/linear-cross-d1.bin
compare_file LINEAR_CROSS_BACKING_D2 /tmp/linear-cross-half.bin /tmp/linear-cross-d2.bin
dmsetup remove dm_linear_cross

echo '=== STEP 3: three linear segments use non-zero backing starts ==='
dd if=/dev/zero of="$DISK1" bs=4096 count=4 conv=fsync status=none
dd if=/dev/zero of="$DISK2" bs=4096 count=4 conv=fsync status=none
dd if=/dev/zero of="$DISK3" bs=4096 count=4 conv=fsync status=none
printf '0 3 linear %s 5\n3 5 linear %s 7\n8 8 linear %s 11\n' "$DEV1" "$DEV2" "$DEV3" | dmsetup create dm_linear_segments
dmsetup table dm_linear_segments | tee /tmp/linear-segments-table.txt
dmsetup deps dm_linear_segments | tee /tmp/linear-segments-deps.txt
grep -q '3 dependencies' /tmp/linear-segments-deps.txt
: >/tmp/linear-segments-payload.bin
for char in A B C D E F G H I J K L M N O P; do
    make_sector "$char" "/tmp/linear-sector-${char}.bin"
    cat "/tmp/linear-sector-${char}.bin" >>/tmp/linear-segments-payload.bin
done
dd if=/tmp/linear-segments-payload.bin of=/dev/mapper/dm_linear_segments bs=8192 count=1 conv=fsync status=none
dd if=/dev/mapper/dm_linear_segments of=/tmp/linear-segments-readback.bin bs=8192 count=1 status=none
compare_file LINEAR_SEGMENTS_MAPPER /tmp/linear-segments-payload.bin /tmp/linear-segments-readback.bin
dd if=/tmp/linear-segments-payload.bin of=/tmp/linear-segments-expected-d1.bin bs=512 count=3 status=none
dd if=/tmp/linear-segments-payload.bin of=/tmp/linear-segments-expected-d2.bin bs=512 skip=3 count=5 status=none
dd if=/tmp/linear-segments-payload.bin of=/tmp/linear-segments-expected-d3.bin bs=512 skip=8 count=8 status=none
dd if="$DISK1" of=/tmp/linear-segments-actual-d1.bin bs=512 skip=5 count=3 status=none
dd if="$DISK2" of=/tmp/linear-segments-actual-d2.bin bs=512 skip=7 count=5 status=none
dd if="$DISK3" of=/tmp/linear-segments-actual-d3.bin bs=512 skip=11 count=8 status=none
compare_file LINEAR_SEGMENTS_BACKING_D1 /tmp/linear-segments-expected-d1.bin /tmp/linear-segments-actual-d1.bin
compare_file LINEAR_SEGMENTS_BACKING_D2 /tmp/linear-segments-expected-d2.bin /tmp/linear-segments-actual-d2.bin
compare_file LINEAR_SEGMENTS_BACKING_D3 /tmp/linear-segments-expected-d3.bin /tmp/linear-segments-actual-d3.bin
dmsetup remove dm_linear_segments

echo '=== STEP 4: one 8 KiB BIO crosses four striped chunks ==='
dd if=/dev/zero of="$DISK1" bs=8192 count=1 conv=fsync status=none
dd if=/dev/zero of="$DISK2" bs=8192 count=1 conv=fsync status=none
printf '0 16 striped 2 4 %s 0 %s 0\n' "$DEV1" "$DEV2" | dmsetup create dm_striped_aligned
dmsetup table dm_striped_aligned | tee /tmp/striped-aligned-table.txt
dmsetup deps dm_striped_aligned | tee /tmp/striped-aligned-deps.txt
grep -q '2 dependencies' /tmp/striped-aligned-deps.txt
make_bytes a 2048 /tmp/striped-aligned-a.bin
make_bytes b 2048 /tmp/striped-aligned-b.bin
make_bytes c 2048 /tmp/striped-aligned-c.bin
make_bytes d 2048 /tmp/striped-aligned-d.bin
cat /tmp/striped-aligned-a.bin /tmp/striped-aligned-b.bin /tmp/striped-aligned-c.bin /tmp/striped-aligned-d.bin >/tmp/striped-aligned-payload.bin
cat /tmp/striped-aligned-a.bin /tmp/striped-aligned-c.bin >/tmp/striped-aligned-expected-d1.bin
cat /tmp/striped-aligned-b.bin /tmp/striped-aligned-d.bin >/tmp/striped-aligned-expected-d2.bin
dd if=/tmp/striped-aligned-payload.bin of=/dev/mapper/dm_striped_aligned bs=8192 count=1 conv=fsync status=none
dd if=/dev/mapper/dm_striped_aligned of=/tmp/striped-aligned-readback.bin bs=8192 count=1 status=none
dd if="$DISK1" of=/tmp/striped-aligned-actual-d1.bin bs=4096 count=1 status=none
dd if="$DISK2" of=/tmp/striped-aligned-actual-d2.bin bs=4096 count=1 status=none
compare_file STRIPED_ALIGNED_MAPPER /tmp/striped-aligned-payload.bin /tmp/striped-aligned-readback.bin
compare_file STRIPED_ALIGNED_BACKING_D1 /tmp/striped-aligned-expected-d1.bin /tmp/striped-aligned-actual-d1.bin
compare_file STRIPED_ALIGNED_BACKING_D2 /tmp/striped-aligned-expected-d2.bin /tmp/striped-aligned-actual-d2.bin
dmsetup remove dm_striped_aligned

STEP5_IO_TIMEOUT=${DM_DATAPLANE_STEP5_IO_TIMEOUT:-20}

echo '=== STEP 5: striped I/O starts inside a chunk and spans boundaries ==='
dd if=/dev/zero of="$DISK1" bs=4096 count=4 conv=fsync status=none
dd if=/dev/zero of="$DISK2" bs=4096 count=4 conv=fsync status=none
printf '0 24 striped 2 4 %s 0 %s 0\n' "$DEV1" "$DEV2" | dmsetup create dm_striped_offset
: >/tmp/striped-offset-payload.bin
for char in e f g h i j k l m n o p; do
    make_sector "$char" "/tmp/striped-offset-${char}.bin"
    cat "/tmp/striped-offset-${char}.bin" >>/tmp/striped-offset-payload.bin
done
run_step5_io WRITE \
    dd if=/tmp/striped-offset-payload.bin of=/dev/mapper/dm_striped_offset bs=512 seek=2 count=12 conv=notrunc status=none
run_step5_io FLUSH sync -f /dev/mapper/dm_striped_offset
run_step5_io MAPPER_READ \
    dd if=/dev/mapper/dm_striped_offset of=/tmp/striped-offset-readback.bin bs=512 skip=2 count=12 status=none
dd if=/dev/zero of=/tmp/striped-offset-expected-d1.bin bs=512 count=2 status=none
dd if=/tmp/striped-offset-payload.bin of=/tmp/striped-offset-expected-d1.bin bs=512 seek=2 count=2 conv=notrunc status=none
dd if=/tmp/striped-offset-payload.bin of=/tmp/striped-offset-expected-d1.bin bs=512 skip=6 seek=4 count=4 conv=notrunc status=none
dd if=/tmp/striped-offset-payload.bin of=/tmp/striped-offset-expected-d2.bin bs=512 skip=2 count=4 status=none
dd if=/tmp/striped-offset-payload.bin of=/tmp/striped-offset-expected-d2.bin bs=512 skip=10 seek=4 count=2 conv=notrunc status=none
dd if=/dev/zero of=/tmp/striped-offset-expected-d2.bin bs=512 seek=6 count=2 conv=notrunc status=none
run_step5_io BACKING_READ_D1 \
    dd if="$DISK1" of=/tmp/striped-offset-actual-d1.bin bs=512 count=8 status=none
run_step5_io BACKING_READ_D2 \
    dd if="$DISK2" of=/tmp/striped-offset-actual-d2.bin bs=512 count=8 status=none
compare_file STRIPED_OFFSET_MAPPER /tmp/striped-offset-payload.bin /tmp/striped-offset-readback.bin
compare_file STRIPED_OFFSET_BACKING_D1 /tmp/striped-offset-expected-d1.bin /tmp/striped-offset-actual-d1.bin
compare_file STRIPED_OFFSET_BACKING_D2 /tmp/striped-offset-expected-d2.bin /tmp/striped-offset-actual-d2.bin
dmsetup remove dm_striped_offset

echo '=== STEP 6: one BIO crosses a linear-to-striped target boundary ==='
dd if=/dev/zero of="$DISK1" bs=8192 count=1 conv=fsync status=none
dd if=/dev/zero of="$DISK2" bs=8192 count=1 conv=fsync status=none
dd if=/dev/zero of="$DISK3" bs=8192 count=1 conv=fsync status=none
printf '0 8 linear %s 0\n8 8 striped 2 4 %s 0 %s 0\n' "$DEV1" "$DEV2" "$DEV3" | dmsetup create dm_mixed_data
make_bytes L 4096 /tmp/mixed-linear.bin
make_bytes M 2048 /tmp/mixed-stripe-a.bin
make_bytes N 2048 /tmp/mixed-stripe-b.bin
cat /tmp/mixed-linear.bin /tmp/mixed-stripe-a.bin /tmp/mixed-stripe-b.bin >/tmp/mixed-payload.bin
dd if=/tmp/mixed-payload.bin of=/dev/mapper/dm_mixed_data bs=8192 count=1 conv=fsync status=none
dd if=/dev/mapper/dm_mixed_data of=/tmp/mixed-readback.bin bs=8192 count=1 status=none
dd if="$DISK1" of=/tmp/mixed-actual-d1.bin bs=4096 count=1 status=none
dd if="$DISK2" of=/tmp/mixed-actual-d2.bin bs=2048 count=1 status=none
dd if="$DISK3" of=/tmp/mixed-actual-d3.bin bs=2048 count=1 status=none
compare_file MIXED_MAPPER /tmp/mixed-payload.bin /tmp/mixed-readback.bin
compare_file MIXED_BACKING_LINEAR /tmp/mixed-linear.bin /tmp/mixed-actual-d1.bin
compare_file MIXED_BACKING_STRIPED_D2 /tmp/mixed-stripe-a.bin /tmp/mixed-actual-d2.bin
compare_file MIXED_BACKING_STRIPED_D3 /tmp/mixed-stripe-b.bin /tmp/mixed-actual-d3.bin
dmsetup remove dm_mixed_data

echo '=== STEP 7: zero and error targets complete without backing devices ==='
printf '0 8 zero\n' | dmsetup create dm_zero_data
dmsetup deps dm_zero_data | tee /tmp/zero-data-deps.txt
grep -q '0 dependencies' /tmp/zero-data-deps.txt
dd if=/dev/mapper/dm_zero_data of=/tmp/zero-data-read.bin bs=512 count=1 status=none
cmp -n 512 /tmp/zero-data-read.bin /dev/zero
dd if=/dev/urandom of=/dev/mapper/dm_zero_data bs=512 count=1 conv=fsync status=none
dd if=/dev/mapper/dm_zero_data of=/tmp/zero-data-after-write.bin bs=512 count=1 status=none
cmp -n 512 /tmp/zero-data-after-write.bin /dev/zero
blkdiscard /dev/mapper/dm_zero_data
blkdiscard -z /dev/mapper/dm_zero_data
dmsetup remove dm_zero_data
echo CHECK_PASS_ZERO_DATAPLANE

printf '0 8 error\n' | dmsetup create dm_error_data
dmsetup deps dm_error_data | tee /tmp/error-data-deps.txt
grep -q '0 dependencies' /tmp/error-data-deps.txt
if dd if=/dev/mapper/dm_error_data of=/dev/null bs=512 count=1 status=none 2>/tmp/error-read.err; then
    echo TEST_FAIL_DM_DATAPLANE error_read_succeeded
    exit 1
fi
if dd if=/dev/zero of=/dev/mapper/dm_error_data bs=512 count=1 status=none 2>/tmp/error-write.err; then
    echo TEST_FAIL_DM_DATAPLANE error_write_succeeded
    exit 1
fi
dmsetup remove dm_error_data
echo CHECK_PASS_ERROR_DATAPLANE

echo '=== STEP 8: cleanup ==='
cleanup_dm
sync
echo TEST_PASS_DM_DATAPLANE
poweroff
GUEST_SCRIPT

SUMMARY_INCLUDE='^(TEST_|CHECK_(BEGIN|PASS|FAIL)_|=== STEP|=== CHECK|TEST_DISK[0-9]=|DEV[0-9]=|[0-9]+ dependencies|Command failed|Kernel panic|panicked)'
dm_run_single_guest_test "${TEST_ID}" "${GUEST_SCRIPT_FILE}" TEST_PASS_DM_DATAPLANE "${SUMMARY_INCLUDE}"
