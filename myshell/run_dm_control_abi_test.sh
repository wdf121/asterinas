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

echo '=== STEP 2: create a simple linear mapper ==='
TEST_DISK=$(aster-dm-disk-locator)
printf 'TEST_DISK=%s\n' "$TEST_DISK"
test -b "$TEST_DISK"
DEV=$(printf '%d:%d' "0x$(stat -c '%t' "$TEST_DISK")" "0x$(stat -c '%T' "$TEST_DISK")")
printf 'DEV=%s\n' "$DEV"
dmsetup remove dm_control_abi >/dev/null 2>&1 || true
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

echo '=== STEP 4: exercise wait and noflush suspend/resume ==='
timeout 10 dmsetup wait --noflush dm_control_abi 0
dmsetup suspend --noflush dm_control_abi
dmsetup info dm_control_abi | tee /tmp/control-info-suspended.txt
grep -q 'State:.*SUSPENDED' /tmp/control-info-suspended.txt
dmsetup resume --noflush dm_control_abi
dmsetup info dm_control_abi | tee /tmp/control-info-live.txt
grep -q 'State:.*ACTIVE' /tmp/control-info-live.txt
timeout 10 dmsetup wait --noflush dm_control_abi 0
echo CHECK_PASS_DM_WAIT_AND_NOFLUSH

echo '=== STEP 5: cleanup ==='
dmsetup remove dm_control_abi
sync
echo TEST_PASS_DM_CONTROL_ABI
poweroff
GUEST_SCRIPT

SUMMARY_INCLUDE='TEST_|CHECK_PASS_|=== STEP|=== CHECK|TEST_DISK=|DEV=|DM_TABLE_CONTROL_ABI|linear|Name:|State:|dependencies|Command failed|Kernel panic|panicked'
dm_run_single_guest_test "${TEST_ID}" "${GUEST_SCRIPT_FILE}" TEST_PASS_DM_CONTROL_ABI "${SUMMARY_INCLUDE}"
