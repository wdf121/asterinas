#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/dm_linear/run_lvm2_linear_reboot_test.sh

Runs a two-guest NixOS regression for single-PV LVM2 linear create, same-PV grow/shrink, ext2 file I/O, and reboot recovery.

Optional environment variables:
  DM_TEST_IMAGE                    Backing test image path, default target/nixos/test.img
  DM_LINEAR_LVM2_REBOOT_LOG        Host-side log path, default /tmp/dm-linear-lvm2-reboot-test.log
  GUEST_READY_TIMEOUT              Seconds to wait for guest root shell, default 240
  RESET_DM_TEST_IMAGES             1 to delete test images before running, default 1
  LINEAR_INITIAL_LV_MIB            Initial LV size in MiB, default 256
  LINEAR_EXTENDED_LV_MIB           Same-PV extended LV size in MiB, default 384
  LINEAR_SHRUNK_LV_MIB             Final shrunk LV size in MiB, default 256
  LINEAR_BASE_FILE_MIB             Base test file size in MiB, default 64
  LINEAR_GROW_FILE_MIB             Transient post-grow test file size in MiB, default 240
  LINEAR_AFTER_SHRINK_FILE_MIB     Post-shrink test file size in MiB, default 64
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/../..")
source "${SCRIPT_DIR}/../lib/dm_nixos_test.sh"

TEST_ID=DM_LINEAR_LVM2_REBOOT
LOG=${DM_LINEAR_LVM2_REBOOT_LOG:-/tmp/dm-linear-lvm2-reboot-test.log}
DM_TEST_IMAGE=${DM_TEST_IMAGE:-target/nixos/test.img}
DM_TEST_IMAGES=${DM_TEST_IMAGES:-${DM_TEST_IMAGE}}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-target/nixos/test2.img}
GUEST_READY_TIMEOUT=${GUEST_READY_TIMEOUT:-240}
RESET_DM_TEST_IMAGES=${RESET_DM_TEST_IMAGES:-1}
LINEAR_INITIAL_LV_MIB=${LINEAR_INITIAL_LV_MIB:-256}
LINEAR_EXTENDED_LV_MIB=${LINEAR_EXTENDED_LV_MIB:-384}
LINEAR_SHRUNK_LV_MIB=${LINEAR_SHRUNK_LV_MIB:-256}
LINEAR_BASE_FILE_MIB=${LINEAR_BASE_FILE_MIB:-64}
LINEAR_GROW_FILE_MIB=${LINEAR_GROW_FILE_MIB:-240}
LINEAR_AFTER_SHRINK_FILE_MIB=${LINEAR_AFTER_SHRINK_FILE_MIB:-64}

test "${LINEAR_INITIAL_LV_MIB}" -lt "${LINEAR_EXTENDED_LV_MIB}"
test "${LINEAR_SHRUNK_LV_MIB}" -lt "${LINEAR_EXTENDED_LV_MIB}"
test "${LINEAR_BASE_FILE_MIB}" -lt "${LINEAR_INITIAL_LV_MIB}"
test $((LINEAR_BASE_FILE_MIB + LINEAR_AFTER_SHRINK_FILE_MIB)) -lt "${LINEAR_SHRUNK_LV_MIB}"
test $((LINEAR_BASE_FILE_MIB + LINEAR_GROW_FILE_MIB)) -lt "${LINEAR_EXTENDED_LV_MIB}"

cd "${ASTERINAS_DIR}"
dm_prepare_nixos_test "${TEST_ID}"
echo "HOST_INFO_${TEST_ID} disk1=${DM_TEST_IMAGE} serial=vdmtest"
echo "HOST_INFO_${TEST_ID} initial_lv_mib=${LINEAR_INITIAL_LV_MIB} extended_lv_mib=${LINEAR_EXTENDED_LV_MIB} shrunk_lv_mib=${LINEAR_SHRUNK_LV_MIB} base_file_mib=${LINEAR_BASE_FILE_MIB} grow_file_mib=${LINEAR_GROW_FILE_MIB} after_shrink_file_mib=${LINEAR_AFTER_SHRINK_FILE_MIB}"

FIRST_GUEST_SCRIPT=$(mktemp /tmp/dm-linear-lvm2-first.XXXXXX)
{
    printf 'LINEAR_INITIAL_LV_MIB=%q\n' "${LINEAR_INITIAL_LV_MIB}"
    printf 'LINEAR_EXTENDED_LV_MIB=%q\n' "${LINEAR_EXTENDED_LV_MIB}"
    printf 'LINEAR_SHRUNK_LV_MIB=%q\n' "${LINEAR_SHRUNK_LV_MIB}"
    printf 'LINEAR_BASE_FILE_MIB=%q\n' "${LINEAR_BASE_FILE_MIB}"
    printf 'LINEAR_GROW_FILE_MIB=%q\n' "${LINEAR_GROW_FILE_MIB}"
    printf 'LINEAR_AFTER_SHRINK_FILE_MIB=%q\n' "${LINEAR_AFTER_SHRINK_FILE_MIB}"
    cat <<'GUEST_FIRST'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_LINEAR_LVM2_REBOOT_FIRST status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
INITIAL_SECTORS=$((LINEAR_INITIAL_LV_MIB * 2048))
EXTENDED_SECTORS=$((LINEAR_EXTENDED_LV_MIB * 2048))
SHRUNK_SECTORS=$((LINEAR_SHRUNK_LV_MIB * 2048))
MAPPER_NAME=linear_base_vg-linear_base_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmlinearbase

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

dep_token() {
    printf '(%s, %s)' "${1%%:*}" "${1##*:}"
}

check_linear_table() {
    local table_file=$1 deps_file=$2 status_file=$3 expected_sectors=$4 dev=$5
    test "$(wc -l < "${table_file}")" -eq 1
    awk -v len="${expected_sectors}" -v dev="${dev}" '$1 == 0 && $2 == len && $3 == "linear" && $4 == dev { ok = 1 } END { exit !ok }' "${table_file}"
    awk -v len="${expected_sectors}" '$1 == 0 && $2 == len && $3 == "linear" { ok = 1 } END { exit !ok }' "${status_file}"
    grep -q '1 dependencies' "${deps_file}"
    grep -F -q "$(dep_token "${dev}")" "${deps_file}"
}

check_linear_table_status_deps() {
    local label=$1 expected_sectors=$2 table_file=$3 status_file=$4 deps_file=$5 dev=$6
    echo "DM_TABLE_LVM2_LINEAR_${label}_BEGIN"
    dmsetup table "${MAPPER_NAME}" | tee "${table_file}" | sed "s/^/DM_TABLE_LVM2_LINEAR_${label} /"
    echo "DM_TABLE_LVM2_LINEAR_${label}_END"
    dmsetup status "${MAPPER_NAME}" | tee "${status_file}"
    dmsetup deps "${MAPPER_NAME}" | tee "${deps_file}"
    check_linear_table "${table_file}" "${deps_file}" "${status_file}" "${expected_sectors}" "${dev}"
}

echo '=== STEP 1: check dm control device, linear target, and test disk ==='
test -c /dev/mapper/control
dmsetup targets | tee /tmp/linear-lvm2-targets.txt
grep -q '^linear' /tmp/linear-lvm2-targets.txt
TEST_DISK=$(aster-dm-disk-locator)
printf 'TEST_DISK=%s\n' "${TEST_DISK}"
test -b "${TEST_DISK}"
DEV1=$(devno "${TEST_DISK}")
printf 'DEV1=%s\n' "${DEV1}"
echo CHECK_PASS_LINEAR_LVM2_SETUP

echo '=== STEP 2: create single-PV linear LV ==='
pvcreate "${TEST_DISK}"
vgcreate linear_base_vg "${TEST_DISK}"
lvcreate --config "${LVM_CONFIG}" --type linear -L "${LINEAR_INITIAL_LV_MIB}M" -n linear_base_lv linear_base_vg "${TEST_DISK}"
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices linear_base_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,devices linear_base_vg/linear_base_lv
check_linear_table_status_deps INITIAL "${INITIAL_SECTORS}" /tmp/linear-lvm2-initial-table.txt /tmp/linear-lvm2-initial-status.txt /tmp/linear-lvm2-initial-deps.txt "${DEV1}"
echo CHECK_PASS_LINEAR_LVM2_INITIAL_TABLE_STATUS_DEPS

echo '=== STEP 3: create ext2 filesystem and write base data ==='
mkfs.ext2 -F -b 4096 "${MAPPER_DEVICE}"
blkid "${MAPPER_DEVICE}"
mkdir -p "${MOUNT_DIR}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
dd if=/dev/urandom of="${MOUNT_DIR}/base${LINEAR_BASE_FILE_MIB}.bin" bs=1M count="${LINEAR_BASE_FILE_MIB}" conv=fsync status=none
printf 'lvm2 linear base\ninitial_lv_mib=%s\n' "${LINEAR_INITIAL_LV_MIB}" > "${MOUNT_DIR}/base-marker.txt"
(
    cd "${MOUNT_DIR}"
    md5sum "base${LINEAR_BASE_FILE_MIB}.bin" base-marker.txt > linear.md5
    md5sum -c linear.md5
)
sync
umount "${MOUNT_DIR}"
echo CHECK_PASS_LINEAR_LVM2_BASE_FILE_MD5

echo '=== STEP 4: grow linear LV on the same PV ==='
lvextend --config "${LVM_CONFIG}" -L "${LINEAR_EXTENDED_LV_MIB}M" linear_base_vg/linear_base_lv "${TEST_DISK}"
lvs -a -o lv_name,lv_size,seg_count,devices linear_base_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,devices linear_base_vg/linear_base_lv
check_linear_table_status_deps EXTENDED "${EXTENDED_SECTORS}" /tmp/linear-lvm2-extended-table.txt /tmp/linear-lvm2-extended-status.txt /tmp/linear-lvm2-extended-deps.txt "${DEV1}"
echo CHECK_PASS_LINEAR_LVM2_EXTENDED_TABLE_STATUS_DEPS

e2fsck -f -y "${MAPPER_DEVICE}"
resize2fs "${MAPPER_DEVICE}"
e2fsck -f -y "${MAPPER_DEVICE}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c linear.md5
)
dd if=/dev/urandom of="${MOUNT_DIR}/grow${LINEAR_GROW_FILE_MIB}.bin" bs=1M count="${LINEAR_GROW_FILE_MIB}" conv=fsync status=none
printf 'lvm2 linear after grow\nextended_lv_mib=%s\n' "${LINEAR_EXTENDED_LV_MIB}" > "${MOUNT_DIR}/grow-marker.txt"
(
    cd "${MOUNT_DIR}"
    md5sum "grow${LINEAR_GROW_FILE_MIB}.bin" grow-marker.txt > grow.md5
    md5sum -c grow.md5
    rm "grow${LINEAR_GROW_FILE_MIB}.bin" grow-marker.txt grow.md5
    md5sum -c linear.md5
)
sync
umount "${MOUNT_DIR}"
echo CHECK_PASS_LINEAR_LVM2_GROW_FILE_MD5

echo '=== STEP 5: shrink ext2 and linear LV on the same PV ==='
e2fsck -f -y "${MAPPER_DEVICE}"
resize2fs "${MAPPER_DEVICE}" "${LINEAR_SHRUNK_LV_MIB}M"
e2fsck -f -y "${MAPPER_DEVICE}"
lvreduce --config "${LVM_CONFIG}" -y -L "${LINEAR_SHRUNK_LV_MIB}M" linear_base_vg/linear_base_lv
lvs -a -o lv_name,lv_size,seg_count,devices linear_base_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,devices linear_base_vg/linear_base_lv
check_linear_table_status_deps SHRUNK "${SHRUNK_SECTORS}" /tmp/linear-lvm2-shrunk-table.txt /tmp/linear-lvm2-shrunk-status.txt /tmp/linear-lvm2-shrunk-deps.txt "${DEV1}"
echo CHECK_PASS_LINEAR_LVM2_SHRUNK_TABLE_STATUS_DEPS

mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c linear.md5
)
dd if=/dev/urandom of="${MOUNT_DIR}/after-shrink${LINEAR_AFTER_SHRINK_FILE_MIB}.bin" bs=1M count="${LINEAR_AFTER_SHRINK_FILE_MIB}" conv=fsync status=none
printf 'lvm2 linear after shrink\nshrunk_lv_mib=%s\n' "${LINEAR_SHRUNK_LV_MIB}" > "${MOUNT_DIR}/shrink-marker.txt"
(
    cd "${MOUNT_DIR}"
    md5sum "after-shrink${LINEAR_AFTER_SHRINK_FILE_MIB}.bin" shrink-marker.txt >> linear.md5
    md5sum -c linear.md5
)
sync
umount "${MOUNT_DIR}"
vgchange --config "${LVM_CONFIG}" -an linear_base_vg
sync
echo CHECK_PASS_LINEAR_LVM2_SHRINK_FILE_MD5

echo TEST_PASS_DM_LINEAR_LVM2_REBOOT_FIRST
poweroff
GUEST_FIRST
} >"${FIRST_GUEST_SCRIPT}"

SECOND_GUEST_SCRIPT=$(mktemp /tmp/dm-linear-lvm2-second.XXXXXX)
{
    printf 'LINEAR_SHRUNK_LV_MIB=%q\n' "${LINEAR_SHRUNK_LV_MIB}"
    printf 'LINEAR_BASE_FILE_MIB=%q\n' "${LINEAR_BASE_FILE_MIB}"
    printf 'LINEAR_AFTER_SHRINK_FILE_MIB=%q\n' "${LINEAR_AFTER_SHRINK_FILE_MIB}"
    cat <<'GUEST_SECOND'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_LINEAR_LVM2_REBOOT_SECOND status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
SHRUNK_SECTORS=$((LINEAR_SHRUNK_LV_MIB * 2048))
MAPPER_NAME=linear_base_vg-linear_base_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmlinearbase

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

dep_token() {
    printf '(%s, %s)' "${1%%:*}" "${1##*:}"
}

check_linear_table() {
    local table_file=$1 deps_file=$2 status_file=$3 dev=$4
    test "$(wc -l < "${table_file}")" -eq 1
    awk -v len="${SHRUNK_SECTORS}" -v dev="${dev}" '$1 == 0 && $2 == len && $3 == "linear" && $4 == dev { ok = 1 } END { exit !ok }' "${table_file}"
    awk -v len="${SHRUNK_SECTORS}" '$1 == 0 && $2 == len && $3 == "linear" { ok = 1 } END { exit !ok }' "${status_file}"
    grep -q '1 dependencies' "${deps_file}"
    grep -F -q "$(dep_token "${dev}")" "${deps_file}"
}

check_linear_table_status_deps() {
    local label=$1 table_file=$2 status_file=$3 deps_file=$4 dev=$5
    echo "DM_TABLE_LVM2_LINEAR_${label}_BEGIN"
    dmsetup table "${MAPPER_NAME}" | tee "${table_file}" | sed "s/^/DM_TABLE_LVM2_LINEAR_${label} /"
    echo "DM_TABLE_LVM2_LINEAR_${label}_END"
    dmsetup status "${MAPPER_NAME}" | tee "${status_file}"
    dmsetup deps "${MAPPER_NAME}" | tee "${deps_file}"
    check_linear_table "${table_file}" "${deps_file}" "${status_file}" "${dev}"
}

echo '=== STEP 1: recover shrunk single-PV linear LV after reboot ==='
TEST_DISK=$(aster-dm-disk-locator)
printf 'TEST_DISK=%s\n' "${TEST_DISK}"
test -b "${TEST_DISK}"
DEV1=$(devno "${TEST_DISK}")
printf 'DEV1=%s\n' "${DEV1}"
pvscan
vgscan --mknodes
vgchange --config "${LVM_CONFIG}" -ay linear_base_vg
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices linear_base_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,devices linear_base_vg/linear_base_lv
check_linear_table_status_deps RECOVERED /tmp/linear-lvm2-recovered-table.txt /tmp/linear-lvm2-recovered-status.txt /tmp/linear-lvm2-recovered-deps.txt "${DEV1}"
echo CHECK_PASS_LINEAR_LVM2_RECOVERED_TABLE_STATUS_DEPS

echo '=== STEP 2: readonly mount and verify md5 after reboot ==='
mkdir -p "${MOUNT_DIR}"
mount -o ro -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c linear.md5
    cat base-marker.txt
    cat shrink-marker.txt
)
df -h "${MOUNT_DIR}"
du -sh "${MOUNT_DIR}" "${MOUNT_DIR}/base${LINEAR_BASE_FILE_MIB}.bin" "${MOUNT_DIR}/after-shrink${LINEAR_AFTER_SHRINK_FILE_MIB}.bin"
umount "${MOUNT_DIR}"
vgchange --config "${LVM_CONFIG}" -an linear_base_vg
sync
echo CHECK_PASS_LINEAR_LVM2_RECOVERED_FILE_MD5

echo TEST_PASS_DM_LINEAR_LVM2_REBOOT_SECOND
poweroff
GUEST_SECOND
} >"${SECOND_GUEST_SCRIPT}"

SUMMARY_INCLUDE='^TEST_|^CHECK_PASS_|^=== STEP|^=== CHECK|^TEST_DISK=|^DEV1=|^DM_TABLE_LVM2_LINEAR|^linear[[:space:]]|^0 [0-9]+ linear |^[0-9]+ dependencies|^Resizing the filesystem|^The filesystem on|base[0-9]+\.bin: OK|grow[0-9]+\.bin: OK|after-shrink[0-9]+\.bin: OK|base-marker\.txt: OK|shrink-marker\.txt: OK|lvm2 linear|initial_lv_mib=|extended_lv_mib=|shrunk_lv_mib=|No space left|Input/output error|Command failed|Kernel panic|panicked|records in|records out|bytes .* copied'
dm_run_two_guest_test \
    "${TEST_ID}" \
    "${FIRST_GUEST_SCRIPT}" \
    "${SECOND_GUEST_SCRIPT}" \
    TEST_PASS_DM_LINEAR_LVM2_REBOOT_FIRST \
    TEST_PASS_DM_LINEAR_LVM2_REBOOT_SECOND \
    "${SUMMARY_INCLUDE}"
