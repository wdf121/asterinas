#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/run_dm_control_abi_test.sh

Runs a lightweight NixOS guest smoke test for Device Mapper control ioctl paths.

Optional environment variables:
  DM_TEST_IMAGE              First backing test image path, default target/nixos/test.img
  DM_TEST_IMAGE_2            Second backing test image path, default target/nixos/test2.img
  DM_CONTROL_ABI_LOG         Host-side log path, default /tmp/dm-control-abi-test.log
  GUEST_READY_TIMEOUT        Seconds to wait for guest root shell, default 120
  RESET_DM_TEST_IMAGES       1 to delete test images before running, default 1
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/..")
source "${SCRIPT_DIR}/lib/dm_nixos_test.sh"

TEST_ID=DM_CONTROL_ABI
LOG=${DM_CONTROL_ABI_LOG:-/tmp/dm-control-abi-test.log}
DM_TEST_IMAGE=${DM_TEST_IMAGE:-target/nixos/test.img}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-target/nixos/test2.img}
GUEST_READY_TIMEOUT=${GUEST_READY_TIMEOUT:-120}
RESET_DM_TEST_IMAGES=${RESET_DM_TEST_IMAGES:-1}

cd "${ASTERINAS_DIR}"
dm_prepare_nixos_test "${TEST_ID}"
echo "HOST_INFO_${TEST_ID} disk1=${DM_TEST_IMAGE} serial=vdmtest"
echo "HOST_INFO_${TEST_ID} disk2=${DM_TEST_IMAGE_2} serial=vdmtest2"

GUEST_SCRIPT_FILE=$(mktemp /tmp/dm-control-abi-guest.XXXXXX)
cat >"${GUEST_SCRIPT_FILE}" <<'GUEST_SCRIPT'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_CONTROL_ABI status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'

echo '=== STEP 1: check dm control device and target versions ==='
test -c /dev/mapper/control
dmsetup version
dmsetup targets | tee /tmp/dm-targets.txt
grep -q '^linear' /tmp/dm-targets.txt
echo CHECK_PASS_DM_VERSION_AND_TARGETS

echo '=== STEP 2: locate test disks and create a simple linear mapper ==='
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "$TEST_DISK" "$TEST_DISK2"
test "$TEST_DISK" != "$TEST_DISK2"
test -b "$TEST_DISK"
test -b "$TEST_DISK2"
DEV=$(printf '%d:%d' "0x$(stat -c '%t' "$TEST_DISK")" "0x$(stat -c '%T' "$TEST_DISK")")
DEV2=$(printf '%d:%d' "0x$(stat -c '%t' "$TEST_DISK2")" "0x$(stat -c '%T' "$TEST_DISK2")")
printf 'DEV=%s\nDEV2=%s\n' "$DEV" "$DEV2"
dmsetup remove dm_control_abi >/dev/null 2>&1 || true
dmsetup remove dm_control_renamed >/dev/null 2>&1 || true
dmsetup remove dm_control_multi >/dev/null 2>&1 || true
dmsetup remove dm_control_busy >/dev/null 2>&1 || true
dmsetup remove dm_control_remove_all >/dev/null 2>&1 || true
dmsetup remove dm_control_readonly >/dev/null 2>&1 || true
printf '0 8 linear %s 0\n' "$DEV" | dmsetup create dm_control_abi

echo 'DM_TABLE_CONTROL_ABI_BEGIN'
dmsetup table dm_control_abi | tee /tmp/control-table.txt | sed 's/^/DM_TABLE_CONTROL_ABI /'
echo 'DM_TABLE_CONTROL_ABI_END'
grep -F -x -q "0 8 linear $DEV 0" /tmp/control-table.txt

echo '=== STEP 3: query status/deps/info through control ioctl ==='
dmsetup status dm_control_abi | tee /tmp/control-status.txt
dmsetup deps dm_control_abi | tee /tmp/control-deps.txt
dmsetup info dm_control_abi | tee /tmp/control-info.txt
grep -q 'linear' /tmp/control-status.txt
grep -q '1 dependencies' /tmp/control-deps.txt
grep -q 'Name:.*dm_control_abi' /tmp/control-info.txt
echo CHECK_PASS_DM_STATUS_DEPS_INFO

echo '=== STEP 4: rename mapper by name and keep table/status valid ==='
dmsetup rename dm_control_abi dm_control_renamed
dmsetup info dm_control_renamed | tee /tmp/control-info-renamed.txt
dmsetup table dm_control_renamed | tee /tmp/control-table-renamed.txt
grep -q 'Name:.*dm_control_renamed' /tmp/control-info-renamed.txt
grep -F -x -q "0 8 linear $DEV 0" /tmp/control-table-renamed.txt
if dmsetup info dm_control_abi >/dev/null 2>&1; then
    echo TEST_FAIL_DM_CONTROL_ABI old_name_still_exists
    exit 1
fi
echo CHECK_PASS_DM_RENAME_NAME

echo '=== STEP 5: create multi-target mapper and query table/status/deps ==='
printf '0 4 linear %s 0\n4 4 linear %s 0\n' "$DEV" "$DEV2" | dmsetup create dm_control_multi
echo 'DM_TABLE_CONTROL_MULTI_BEGIN'
dmsetup table dm_control_multi | tee /tmp/control-multi-table.txt | sed 's/^/DM_TABLE_CONTROL_MULTI /'
echo 'DM_TABLE_CONTROL_MULTI_END'
test "$(wc -l < /tmp/control-multi-table.txt)" -eq 2
grep -F -x -q "0 4 linear $DEV 0" /tmp/control-multi-table.txt
grep -F -x -q "4 4 linear $DEV2 0" /tmp/control-multi-table.txt
dmsetup status dm_control_multi | tee /tmp/control-multi-status.txt
dmsetup deps dm_control_multi | tee /tmp/control-multi-deps.txt
grep -q 'linear' /tmp/control-multi-status.txt
grep -q '2 dependencies' /tmp/control-multi-deps.txt
echo CHECK_PASS_DM_MULTI_TARGET_STATUS_DEPS

echo '=== STEP 6: list multiple mapper devices ==='
dmsetup ls | tee /tmp/control-ls.txt
grep -q '^dm_control_renamed' /tmp/control-ls.txt
grep -q '^dm_control_multi' /tmp/control-ls.txt
echo CHECK_PASS_DM_LIST_MULTIPLE_DEVICES

echo '=== STEP 7: exercise wait and noflush suspend/resume ==='
timeout 10 dmsetup wait --noflush dm_control_renamed 0
dmsetup suspend --noflush dm_control_renamed
dmsetup info dm_control_renamed | tee /tmp/control-info-suspended.txt
grep -q 'State:.*SUSPENDED' /tmp/control-info-suspended.txt
dmsetup resume --noflush dm_control_renamed
dmsetup info dm_control_renamed | tee /tmp/control-info-live.txt
grep -q 'State:.*ACTIVE' /tmp/control-info-live.txt
timeout 10 dmsetup wait --noflush dm_control_renamed 0
echo CHECK_PASS_DM_WAIT_AND_NOFLUSH

echo '=== STEP 8: readonly mapper allows read and rejects write ==='
printf '0 8 linear %s 0\n' "$DEV" | dmsetup --readonly create dm_control_readonly
dd if=/dev/mapper/dm_control_readonly of=/tmp/control-readonly-read.bin bs=512 count=1 status=none
head -c 512 /dev/zero | tr '\000' '\123' > /tmp/control-readonly-write.bin
if dd if=/tmp/control-readonly-write.bin of=/dev/mapper/dm_control_readonly bs=512 count=1 conv=fsync status=none 2>/tmp/control-readonly-write.err; then
    echo TEST_FAIL_DM_CONTROL_ABI readonly_write_succeeded
    exit 1
fi
dmsetup remove dm_control_readonly
echo CHECK_PASS_DM_READONLY

echo '=== STEP 9: busy remove fails but remove_all removes non-busy mappers ==='
printf '0 8 linear %s 0\n' "$DEV" | dmsetup create dm_control_busy
printf '0 8 linear %s 0\n' "$DEV2" | dmsetup create dm_control_remove_all
exec 9< /dev/mapper/dm_control_busy
if dmsetup remove dm_control_busy >/tmp/control-busy-remove.txt 2>&1; then
    echo TEST_FAIL_DM_CONTROL_ABI busy_remove_succeeded
    exit 1
fi
dmsetup info dm_control_busy | tee /tmp/control-info-busy.txt
dmsetup remove_all || true
dmsetup info dm_control_busy >/dev/null
if dmsetup info dm_control_renamed >/dev/null 2>&1; then
    echo TEST_FAIL_DM_CONTROL_ABI remove_all_left_renamed
    exit 1
fi
if dmsetup info dm_control_multi >/dev/null 2>&1; then
    echo TEST_FAIL_DM_CONTROL_ABI remove_all_left_multi
    exit 1
fi
if dmsetup info dm_control_remove_all >/dev/null 2>&1; then
    echo TEST_FAIL_DM_CONTROL_ABI remove_all_left_non_busy
    exit 1
fi
exec 9<&-
dmsetup remove dm_control_busy
echo CHECK_PASS_DM_BUSY_REMOVE_AND_REMOVE_ALL

echo '=== STEP 10: cleanup ==='
dmsetup remove dm_control_renamed >/dev/null 2>&1 || true
dmsetup remove dm_control_multi >/dev/null 2>&1 || true
dmsetup remove dm_control_remove_all >/dev/null 2>&1 || true
dmsetup remove dm_control_readonly >/dev/null 2>&1 || true
sync
echo TEST_PASS_DM_CONTROL_ABI
poweroff
GUEST_SCRIPT

SUMMARY_INCLUDE='TEST_|CHECK_PASS_|=== STEP|=== CHECK|TEST_DISK=|TEST_DISK2=|DEV=|DEV2=|DM_TABLE_CONTROL|dm_control_|linear|Name:|State:|dependencies|Command failed|Kernel panic|panicked'
dm_run_single_guest_test "${TEST_ID}" "${GUEST_SCRIPT_FILE}" TEST_PASS_DM_CONTROL_ABI "${SUMMARY_INCLUDE}"
