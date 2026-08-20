#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/dm_striped/run_lvm2_striped_io_reboot_test.sh

Runs a two-guest NixOS regression for LVM2 striped LV file I/O and reboot recovery.

Optional environment variables:
  DM_TEST_IMAGE                       First backing test image path, default target/nixos/test.img
  DM_TEST_IMAGE_2                     Second backing test image path, default target/nixos/test2.img
  DM_STRIPED_LVM2_IO_REBOOT_LOG       Host-side log path, default /tmp/dm-striped-lvm2-io-reboot-test.log
  GUEST_READY_TIMEOUT                 Seconds to wait for guest root shell, default 300
  RESET_DM_TEST_IMAGES                1 to delete test images before running, default 1
  STRIPED_LV_MIB                      Striped LV size in MiB, default 256
  STRIPED_FILE_MIB                    Test file size in MiB, default 64
  STRIPED_CHUNK_KIB                   LVM stripe chunk size in KiB, default 4
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/../..")
source "${SCRIPT_DIR}/../lib/dm_nixos_test.sh"

TEST_ID=DM_STRIPED_LVM2_IO_REBOOT
LOG=${DM_STRIPED_LVM2_IO_REBOOT_LOG:-/tmp/dm-striped-lvm2-io-reboot-test.log}
DM_TEST_IMAGE=${DM_TEST_IMAGE:-target/nixos/test.img}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-target/nixos/test2.img}
GUEST_READY_TIMEOUT=${GUEST_READY_TIMEOUT:-300}
RESET_DM_TEST_IMAGES=${RESET_DM_TEST_IMAGES:-1}
STRIPED_LV_MIB=${STRIPED_LV_MIB:-256}
STRIPED_FILE_MIB=${STRIPED_FILE_MIB:-64}
STRIPED_CHUNK_KIB=${STRIPED_CHUNK_KIB:-4}

cd "${ASTERINAS_DIR}"
dm_prepare_nixos_test "${TEST_ID}"
echo "HOST_INFO_${TEST_ID} disk1=${DM_TEST_IMAGE} serial=vdmtest"
echo "HOST_INFO_${TEST_ID} disk2=${DM_TEST_IMAGE_2} serial=vdmtest2"
echo "HOST_INFO_${TEST_ID} lv_mib=${STRIPED_LV_MIB} file_mib=${STRIPED_FILE_MIB} chunk_kib=${STRIPED_CHUNK_KIB}"

FIRST_GUEST_SCRIPT=$(mktemp /tmp/dm-striped-lvm2-first.XXXXXX)
cat >"${FIRST_GUEST_SCRIPT}" <<'GUEST_SCRIPT'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_STRIPED_LVM2_IO_REBOOT_FIRST status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
STRIPED_LV_MIB=__STRIPED_LV_MIB__
STRIPED_FILE_MIB=__STRIPED_FILE_MIB__
STRIPED_CHUNK_KIB=__STRIPED_CHUNK_KIB__
STRIPED_LV_SECTORS=$((STRIPED_LV_MIB * 2048))
STRIPED_CHUNK_SECTORS=$((STRIPED_CHUNK_KIB * 2))
MAPPER_NAME=striped_vg-striped_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmstriped

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

dep_token() {
    printf '(%s, %s)' "${1%%:*}" "${1##*:}"
}

check_striped_table() {
    local table_file=$1 deps_file=$2 dev1=$3 dev2=$4
    test "$(wc -l < "$table_file")" -eq 1
    awk -v len="$STRIPED_LV_SECTORS" -v chunk="$STRIPED_CHUNK_SECTORS" '
        $1 != 0 { exit 1 }
        $2 != len { exit 1 }
        $3 != "striped" { exit 1 }
        $4 != 2 { exit 1 }
        $5 != chunk { exit 1 }
    ' "$table_file"
    grep -F -q "$dev1" "$table_file"
    grep -F -q "$dev2" "$table_file"
    grep -q '2 dependencies' "$deps_file"
    grep -F -q "$(dep_token "$dev1")" "$deps_file"
    grep -F -q "$(dep_token "$dev2")" "$deps_file"
}

echo '=== STEP 1: check dm control device, striped target, and test disks ==='
test -c /dev/mapper/control
dmsetup targets | tee /tmp/striped-lvm2-targets.txt
grep -q '^striped' /tmp/striped-lvm2-targets.txt
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "$TEST_DISK" "$TEST_DISK2"
test "$TEST_DISK" != "$TEST_DISK2"
test -b "$TEST_DISK"
test -b "$TEST_DISK2"
DEV1=$(devno "$TEST_DISK")
DEV2=$(devno "$TEST_DISK2")
printf 'DEV1=%s\nDEV2=%s\n' "$DEV1" "$DEV2"
echo CHECK_PASS_STRIPED_LVM2_SETUP

echo '=== STEP 2: create a 2-way LVM2 striped LV ==='
pvcreate "$TEST_DISK" "$TEST_DISK2"
vgcreate striped_vg "$TEST_DISK" "$TEST_DISK2"
lvcreate --config "$LVM_CONFIG" --type striped -i 2 -I "${STRIPED_CHUNK_KIB}K" -L "${STRIPED_LV_MIB}M" -n striped_lv striped_vg "$TEST_DISK" "$TEST_DISK2"
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices striped_vg
lvs --segments -o lv_name,seg_start,seg_size,stripes,stripesize,devices striped_vg/striped_lv

echo 'DM_TABLE_LVM2_STRIPED_BEGIN'
dmsetup table "$MAPPER_NAME" | tee /tmp/striped-lvm2-table.txt | sed 's/^/DM_TABLE_LVM2_STRIPED /'
echo 'DM_TABLE_LVM2_STRIPED_END'
dmsetup status "$MAPPER_NAME" | tee /tmp/striped-lvm2-status.txt
dmsetup deps "$MAPPER_NAME" | tee /tmp/striped-lvm2-deps.txt
check_striped_table /tmp/striped-lvm2-table.txt /tmp/striped-lvm2-deps.txt "$DEV1" "$DEV2"
grep -q 'striped' /tmp/striped-lvm2-status.txt
echo CHECK_PASS_STRIPED_LVM2_TABLE_STATUS_DEPS

echo '=== STEP 3: create ext2 filesystem and write test data ==='
mkfs.ext2 -F -b 4096 "$MAPPER_DEVICE"
blkid "$MAPPER_DEVICE"
mkdir -p "$MOUNT_DIR"
mount -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"
dd if=/dev/urandom of="$MOUNT_DIR/file${STRIPED_FILE_MIB}.bin" bs=1M count="$STRIPED_FILE_MIB" conv=fsync status=none
printf 'lvm2 striped io reboot\nlv_mib=%s file_mib=%s chunk_kib=%s\n' "$STRIPED_LV_MIB" "$STRIPED_FILE_MIB" "$STRIPED_CHUNK_KIB" > "$MOUNT_DIR/marker.txt"
(
    cd "$MOUNT_DIR"
    md5sum "file${STRIPED_FILE_MIB}.bin" marker.txt > striped.md5
    md5sum -c striped.md5
)
sync
umount "$MOUNT_DIR"
vgchange --config "$LVM_CONFIG" -an striped_vg
sync
echo CHECK_PASS_STRIPED_LVM2_FILE_MD5

echo TEST_PASS_DM_STRIPED_LVM2_IO_REBOOT_FIRST
poweroff
GUEST_SCRIPT

SECOND_GUEST_SCRIPT=$(mktemp /tmp/dm-striped-lvm2-second.XXXXXX)
cat >"${SECOND_GUEST_SCRIPT}" <<'GUEST_SCRIPT'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_STRIPED_LVM2_IO_REBOOT_SECOND status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
STRIPED_LV_MIB=__STRIPED_LV_MIB__
STRIPED_CHUNK_KIB=__STRIPED_CHUNK_KIB__
STRIPED_FILE_MIB=__STRIPED_FILE_MIB__
STRIPED_LV_SECTORS=$((STRIPED_LV_MIB * 2048))
STRIPED_CHUNK_SECTORS=$((STRIPED_CHUNK_KIB * 2))
MAPPER_NAME=striped_vg-striped_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmstriped

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

dep_token() {
    printf '(%s, %s)' "${1%%:*}" "${1##*:}"
}

check_striped_table() {
    local table_file=$1 deps_file=$2 dev1=$3 dev2=$4
    test "$(wc -l < "$table_file")" -eq 1
    awk -v len="$STRIPED_LV_SECTORS" -v chunk="$STRIPED_CHUNK_SECTORS" '
        $1 != 0 { exit 1 }
        $2 != len { exit 1 }
        $3 != "striped" { exit 1 }
        $4 != 2 { exit 1 }
        $5 != chunk { exit 1 }
    ' "$table_file"
    grep -F -q "$dev1" "$table_file"
    grep -F -q "$dev2" "$table_file"
    grep -q '2 dependencies' "$deps_file"
    grep -F -q "$(dep_token "$dev1")" "$deps_file"
    grep -F -q "$(dep_token "$dev2")" "$deps_file"
}

echo '=== STEP 1: recover LVM2 striped LV after reboot ==='
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "$TEST_DISK" "$TEST_DISK2"
test "$TEST_DISK" != "$TEST_DISK2"
test -b "$TEST_DISK"
test -b "$TEST_DISK2"
DEV1=$(devno "$TEST_DISK")
DEV2=$(devno "$TEST_DISK2")
printf 'DEV1=%s\nDEV2=%s\n' "$DEV1" "$DEV2"
pvscan
vgscan --mknodes
vgchange --config "$LVM_CONFIG" -ay striped_vg
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices striped_vg
lvs --segments -o lv_name,seg_start,seg_size,stripes,stripesize,devices striped_vg/striped_lv

echo 'DM_TABLE_LVM2_STRIPED_RECOVERED_BEGIN'
dmsetup table "$MAPPER_NAME" | tee /tmp/striped-lvm2-recovered-table.txt | sed 's/^/DM_TABLE_LVM2_STRIPED_RECOVERED /'
echo 'DM_TABLE_LVM2_STRIPED_RECOVERED_END'
dmsetup status "$MAPPER_NAME" | tee /tmp/striped-lvm2-recovered-status.txt
dmsetup deps "$MAPPER_NAME" | tee /tmp/striped-lvm2-recovered-deps.txt
check_striped_table /tmp/striped-lvm2-recovered-table.txt /tmp/striped-lvm2-recovered-deps.txt "$DEV1" "$DEV2"
grep -q 'striped' /tmp/striped-lvm2-recovered-status.txt
echo CHECK_PASS_STRIPED_LVM2_RECOVERED_TABLE_STATUS_DEPS

echo '=== STEP 2: readonly mount and verify md5 after reboot ==='
mkdir -p "$MOUNT_DIR"
mount -o ro -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"
(
    cd "$MOUNT_DIR"
    md5sum -c striped.md5
    cat marker.txt
)
df -h "$MOUNT_DIR"
du -sh "$MOUNT_DIR"
umount "$MOUNT_DIR"
vgchange --config "$LVM_CONFIG" -an striped_vg
sync
echo CHECK_PASS_STRIPED_LVM2_RECOVERED_FILE_MD5

echo TEST_PASS_DM_STRIPED_LVM2_IO_REBOOT_SECOND
poweroff
GUEST_SCRIPT

for script in "${FIRST_GUEST_SCRIPT}" "${SECOND_GUEST_SCRIPT}"; do
    sed -i \
        -e "s/__STRIPED_LV_MIB__/${STRIPED_LV_MIB}/g" \
        -e "s/__STRIPED_FILE_MIB__/${STRIPED_FILE_MIB}/g" \
        -e "s/__STRIPED_CHUNK_KIB__/${STRIPED_CHUNK_KIB}/g" \
        "${script}"
done

SUMMARY_INCLUDE='TEST_|CHECK_PASS_|=== STEP|=== CHECK|TEST_DISK=|TEST_DISK2=|DEV1=|DEV2=|DM_TABLE_LVM2_STRIPED|striped_vg|striped_lv|striped|file[0-9]+\.bin|marker|OK|No space left|Input/output error|Command failed|Kernel panic|panicked|records in|records out|bytes .* copied'
dm_run_two_guest_test \
    "${TEST_ID}" \
    "${FIRST_GUEST_SCRIPT}" \
    "${SECOND_GUEST_SCRIPT}" \
    TEST_PASS_DM_STRIPED_LVM2_IO_REBOOT_FIRST \
    TEST_PASS_DM_STRIPED_LVM2_IO_REBOOT_SECOND \
    "${SUMMARY_INCLUDE}"
