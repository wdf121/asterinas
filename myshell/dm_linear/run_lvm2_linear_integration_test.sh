#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/dm_linear/run_lvm2_linear_integration_test.sh

Runs a three-guest NixOS integration test for LVM2 linear storage. It validates
single-PV creation, same-PV growth, cross-PV segment growth, ext2 I/O across the
segment boundary, recovery after growth, shrink back to one segment, and recovery
after shrink.

Optional environment variables:
  DM_TEST_IMAGE                    First backing test image path, default target/nixos/test.img
  DM_TEST_IMAGE_2                  Second backing test image path, default target/nixos/test2.img
  DM_LINEAR_INTEGRATION_LOG        Host-side log path, default /tmp/dm-linear-integration-test.log
  GUEST_QEMU_TIMEOUT               Full QEMU lifecycle timeout in seconds, default 180
  GUEST_READY_TIMEOUT              Guest shell readiness timeout in seconds, default 40
  RESET_DM_TEST_IMAGES             1 to delete test images before running, default 1
  LINEAR_INITIAL_LV_MIB            Initial single-PV LV size in MiB, default 256
  LINEAR_SAME_PV_LV_MIB            Same-PV extended LV size in MiB, default 288
  LINEAR_EXTENDED_LV_MIB           Cross-PV extended LV size in MiB, default 512
  LINEAR_SHRUNK_LV_MIB             Final shrunk LV size in MiB, default 256
  LINEAR_BASE_FILE_MIB             Base test file size in MiB, default 64
  LINEAR_GROW_FILE_MIB             Post-cross-PV test file size in MiB, default 240

Expected success markers:
  TEST_PASS_DM_LINEAR_INTEGRATION_FIRST
  TEST_PASS_DM_LINEAR_INTEGRATION_SECOND
  TEST_PASS_DM_LINEAR_INTEGRATION_THIRD
  HOST_PASS_DM_LINEAR_INTEGRATION
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/../..")
source "${SCRIPT_DIR}/../lib/dm_nixos_test.sh"

TEST_ID=DM_LINEAR_INTEGRATION
LOG=${DM_LINEAR_INTEGRATION_LOG:-/tmp/dm-linear-integration-test.log}
DM_TEST_IMAGE=${DM_TEST_IMAGE:-target/nixos/test.img}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-target/nixos/test2.img}
DM_TEST_IMAGES=${DM_TEST_IMAGES:-${DM_TEST_IMAGE} ${DM_TEST_IMAGE_2}}
GUEST_QEMU_TIMEOUT=${GUEST_QEMU_TIMEOUT:-180}
GUEST_READY_TIMEOUT=${GUEST_READY_TIMEOUT:-40}
RESET_DM_TEST_IMAGES=${RESET_DM_TEST_IMAGES:-1}
LINEAR_INITIAL_LV_MIB=${LINEAR_INITIAL_LV_MIB:-256}
LINEAR_SAME_PV_LV_MIB=${LINEAR_SAME_PV_LV_MIB:-288}
LINEAR_EXTENDED_LV_MIB=${LINEAR_EXTENDED_LV_MIB:-512}
LINEAR_SHRUNK_LV_MIB=${LINEAR_SHRUNK_LV_MIB:-256}
LINEAR_BASE_FILE_MIB=${LINEAR_BASE_FILE_MIB:-64}
LINEAR_GROW_FILE_MIB=${LINEAR_GROW_FILE_MIB:-240}

dm_init_log "${TEST_ID}"

test "${LINEAR_INITIAL_LV_MIB}" -lt "${LINEAR_SAME_PV_LV_MIB}"
test "${LINEAR_SAME_PV_LV_MIB}" -lt "${LINEAR_EXTENDED_LV_MIB}"
test "${LINEAR_SHRUNK_LV_MIB}" -lt "${LINEAR_EXTENDED_LV_MIB}"
test "${LINEAR_BASE_FILE_MIB}" -lt "${LINEAR_INITIAL_LV_MIB}"
test $((LINEAR_BASE_FILE_MIB + LINEAR_GROW_FILE_MIB)) -gt "${LINEAR_SAME_PV_LV_MIB}"
test $((LINEAR_BASE_FILE_MIB + LINEAR_GROW_FILE_MIB)) -lt "${LINEAR_EXTENDED_LV_MIB}"
test "${LINEAR_BASE_FILE_MIB}" -lt "${LINEAR_SHRUNK_LV_MIB}"

cd "${ASTERINAS_DIR}"
dm_prepare_nixos_test "${TEST_ID}"
dm_emit "HOST_INFO_${TEST_ID} qemu_lifecycle_timeout=${GUEST_QEMU_TIMEOUT}s"
dm_emit "HOST_INFO_${TEST_ID} disk1=${DM_TEST_IMAGE} serial=vdmtest"
dm_emit "HOST_INFO_${TEST_ID} disk2=${DM_TEST_IMAGE_2} serial=vdmtest2"
dm_emit "HOST_INFO_${TEST_ID} initial_lv_mib=${LINEAR_INITIAL_LV_MIB} same_pv_lv_mib=${LINEAR_SAME_PV_LV_MIB} extended_lv_mib=${LINEAR_EXTENDED_LV_MIB} shrunk_lv_mib=${LINEAR_SHRUNK_LV_MIB} base_file_mib=${LINEAR_BASE_FILE_MIB} grow_file_mib=${LINEAR_GROW_FILE_MIB}"

FIRST_GUEST_SCRIPT=$(mktemp /tmp/dm-linear-integration-first.XXXXXX)
{
    printf 'LINEAR_INITIAL_LV_MIB=%q\n' "${LINEAR_INITIAL_LV_MIB}"
    printf 'LINEAR_SAME_PV_LV_MIB=%q\n' "${LINEAR_SAME_PV_LV_MIB}"
    printf 'LINEAR_EXTENDED_LV_MIB=%q\n' "${LINEAR_EXTENDED_LV_MIB}"
    printf 'LINEAR_BASE_FILE_MIB=%q\n' "${LINEAR_BASE_FILE_MIB}"
    printf 'LINEAR_GROW_FILE_MIB=%q\n' "${LINEAR_GROW_FILE_MIB}"
    cat <<'GUEST_FIRST'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_LINEAR_INTEGRATION_FIRST status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
INITIAL_SECTORS=$((LINEAR_INITIAL_LV_MIB * 2048))
SAME_PV_SECTORS=$((LINEAR_SAME_PV_LV_MIB * 2048))
EXTENDED_SECTORS=$((LINEAR_EXTENDED_LV_MIB * 2048))
CROSS_PV_SECTORS=$((EXTENDED_SECTORS - SAME_PV_SECTORS))
MAPPER_NAME=linear_integration_vg-linear_integration_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmlinear

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

dep_token() {
    printf '(%s, %s)' "${1%%:*}" "${1##*:}"
}

check_linear_single_table() {
    local table_file=$1 deps_file=$2 status_file=$3 expected_sectors=$4 dev=$5
    test "$(wc -l < "${table_file}")" -eq 1
    awk -v len="${expected_sectors}" -v dev="${dev}" '$1 == 0 && $2 == len && $3 == "linear" && $4 == dev { ok = 1 } END { exit !ok }' "${table_file}"
    awk -v len="${expected_sectors}" '$1 == 0 && $2 == len && $3 == "linear" { ok = 1 } END { exit !ok }' "${status_file}"
    grep -q '1 dependencies' "${deps_file}"
    grep -F -q "$(dep_token "${dev}")" "${deps_file}"
}

check_linear_cross_table() {
    local table_file=$1 deps_file=$2 status_file=$3 dev1=$4 dev2=$5
    test "$(wc -l < "${table_file}")" -eq 2
    awk -v d1="${dev1}" -v d2="${dev2}" -v same="${SAME_PV_SECTORS}" -v cross="${CROSS_PV_SECTORS}" '
        $1 == 0 && $2 == same && $3 == "linear" && $4 == d1 { first = 1 }
        $1 == same && $2 == cross && $3 == "linear" && $4 == d2 { second = 1 }
        END { exit !(first && second) }
    ' "${table_file}"
    awk -v len="${EXTENDED_SECTORS}" '$3 == "linear" { sum += $2 } END { exit !(sum == len) }' "${status_file}"
    grep -q '2 dependencies' "${deps_file}"
    grep -F -q "$(dep_token "${dev1}")" "${deps_file}"
    grep -F -q "$(dep_token "${dev2}")" "${deps_file}"
}

check_linear_single_table_status_deps() {
    local label=$1 expected_sectors=$2 table_file=$3 status_file=$4 deps_file=$5 dev=$6
    echo "DM_TABLE_LVM2_LINEAR_${label}_BEGIN"
    dmsetup table "${MAPPER_NAME}" | tee "${table_file}" | sed "s/^/DM_TABLE_LVM2_LINEAR_${label} /"
    echo "DM_TABLE_LVM2_LINEAR_${label}_END"
    dmsetup status "${MAPPER_NAME}" | tee "${status_file}"
    dmsetup deps "${MAPPER_NAME}" | tee "${deps_file}"
    check_linear_single_table "${table_file}" "${deps_file}" "${status_file}" "${expected_sectors}" "${dev}"
}

check_linear_cross_table_status_deps() {
    local label=$1 table_file=$2 status_file=$3 deps_file=$4 dev1=$5 dev2=$6
    echo "DM_TABLE_LVM2_LINEAR_${label}_BEGIN"
    dmsetup table "${MAPPER_NAME}" | tee "${table_file}" | sed "s/^/DM_TABLE_LVM2_LINEAR_${label} /"
    echo "DM_TABLE_LVM2_LINEAR_${label}_END"
    dmsetup status "${MAPPER_NAME}" | tee "${status_file}"
    dmsetup deps "${MAPPER_NAME}" | tee "${deps_file}"
    check_linear_cross_table "${table_file}" "${deps_file}" "${status_file}" "${dev1}" "${dev2}"
}

echo '=== STEP 1: check dm control device, linear target, and two test disks ==='
test -c /dev/mapper/control
dmsetup targets | tee /tmp/linear-integration-targets.txt
grep -q '^linear' /tmp/linear-integration-targets.txt
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "${TEST_DISK}" "${TEST_DISK2}"
test "${TEST_DISK}" != "${TEST_DISK2}"
test -b "${TEST_DISK}"
test -b "${TEST_DISK2}"
DEV1=$(devno "${TEST_DISK}")
DEV2=$(devno "${TEST_DISK2}")
printf 'DEV1=%s\nDEV2=%s\n' "${DEV1}" "${DEV2}"
echo CHECK_PASS_LINEAR_LVM2_SETUP

echo '=== STEP 2: create single-PV linear LV and base data ==='
pvcreate "${TEST_DISK}"
vgcreate linear_integration_vg "${TEST_DISK}"
lvcreate --config "${LVM_CONFIG}" --type linear -L "${LINEAR_INITIAL_LV_MIB}M" -n linear_integration_lv linear_integration_vg "${TEST_DISK}"
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices linear_integration_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,devices linear_integration_vg/linear_integration_lv
check_linear_single_table_status_deps INITIAL "${INITIAL_SECTORS}" /tmp/linear-integration-initial-table.txt /tmp/linear-integration-initial-status.txt /tmp/linear-integration-initial-deps.txt "${DEV1}"
echo CHECK_PASS_LINEAR_LVM2_INITIAL_TABLE_STATUS_DEPS
mkfs.ext2 -F -b 4096 "${MAPPER_DEVICE}"
mkdir -p "${MOUNT_DIR}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
dd if=/dev/urandom of="${MOUNT_DIR}/base${LINEAR_BASE_FILE_MIB}.bin" bs=1M count="${LINEAR_BASE_FILE_MIB}" conv=fsync status=none
printf 'lvm2 linear integration base\ninitial_lv_mib=%s\n' "${LINEAR_INITIAL_LV_MIB}" > "${MOUNT_DIR}/base-marker.txt"
(
    cd "${MOUNT_DIR}"
    md5sum "base${LINEAR_BASE_FILE_MIB}.bin" base-marker.txt > linear-integration.md5
    md5sum -c linear-integration.md5
)
sync
umount "${MOUNT_DIR}"
echo CHECK_PASS_LINEAR_LVM2_BASE_FILE_MD5

echo '=== STEP 3: grow the linear LV on the original PV ==='
lvextend --config "${LVM_CONFIG}" -L "${LINEAR_SAME_PV_LV_MIB}M" linear_integration_vg/linear_integration_lv "${TEST_DISK}"
lvs -a -o lv_name,lv_size,seg_count,devices linear_integration_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,devices linear_integration_vg/linear_integration_lv
check_linear_single_table_status_deps SAME_PV "${SAME_PV_SECTORS}" /tmp/linear-integration-same-pv-table.txt /tmp/linear-integration-same-pv-status.txt /tmp/linear-integration-same-pv-deps.txt "${DEV1}"
echo CHECK_PASS_LINEAR_LVM2_SAME_PV_TABLE_STATUS_DEPS

echo '=== STEP 4: add a second PV and grow into a second linear segment ==='
pvcreate "${TEST_DISK2}"
vgextend linear_integration_vg "${TEST_DISK2}"
lvextend --config "${LVM_CONFIG}" -L "${LINEAR_EXTENDED_LV_MIB}M" linear_integration_vg/linear_integration_lv "${TEST_DISK2}"
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices linear_integration_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,devices linear_integration_vg/linear_integration_lv
check_linear_cross_table_status_deps EXTENDED /tmp/linear-integration-extended-table.txt /tmp/linear-integration-extended-status.txt /tmp/linear-integration-extended-deps.txt "${DEV1}" "${DEV2}"
echo CHECK_PASS_LINEAR_LVM2_EXTENDED_TABLE_STATUS_DEPS

e2fsck -f -y "${MAPPER_DEVICE}"
resize2fs "${MAPPER_DEVICE}"
e2fsck -f -y "${MAPPER_DEVICE}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c linear-integration.md5
)
dd if=/dev/urandom of="${MOUNT_DIR}/grow${LINEAR_GROW_FILE_MIB}.bin" bs=1M count="${LINEAR_GROW_FILE_MIB}" conv=fsync status=none
printf 'lvm2 linear integration cross-PV grow\nsame_pv_lv_mib=%s\nextended_lv_mib=%s\n' "${LINEAR_SAME_PV_LV_MIB}" "${LINEAR_EXTENDED_LV_MIB}" > "${MOUNT_DIR}/grow-marker.txt"
(
    cd "${MOUNT_DIR}"
    md5sum "grow${LINEAR_GROW_FILE_MIB}.bin" grow-marker.txt >> linear-integration.md5
    md5sum -c linear-integration.md5
)
sync
umount "${MOUNT_DIR}"
vgchange --config "${LVM_CONFIG}" -an linear_integration_vg
sync
echo CHECK_PASS_LINEAR_LVM2_GROW_FILE_MD5

echo TEST_PASS_DM_LINEAR_INTEGRATION_FIRST
poweroff
GUEST_FIRST
} >"${FIRST_GUEST_SCRIPT}"

SECOND_GUEST_SCRIPT=$(mktemp /tmp/dm-linear-integration-second.XXXXXX)
{
    printf 'LINEAR_INITIAL_LV_MIB=%q\n' "${LINEAR_INITIAL_LV_MIB}"
    printf 'LINEAR_SAME_PV_LV_MIB=%q\n' "${LINEAR_SAME_PV_LV_MIB}"
    printf 'LINEAR_EXTENDED_LV_MIB=%q\n' "${LINEAR_EXTENDED_LV_MIB}"
    printf 'LINEAR_SHRUNK_LV_MIB=%q\n' "${LINEAR_SHRUNK_LV_MIB}"
    printf 'LINEAR_BASE_FILE_MIB=%q\n' "${LINEAR_BASE_FILE_MIB}"
    printf 'LINEAR_GROW_FILE_MIB=%q\n' "${LINEAR_GROW_FILE_MIB}"
    cat <<'GUEST_SECOND'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_LINEAR_INTEGRATION_SECOND status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
INITIAL_SECTORS=$((LINEAR_INITIAL_LV_MIB * 2048))
SAME_PV_SECTORS=$((LINEAR_SAME_PV_LV_MIB * 2048))
EXTENDED_SECTORS=$((LINEAR_EXTENDED_LV_MIB * 2048))
SHRUNK_SECTORS=$((LINEAR_SHRUNK_LV_MIB * 2048))
CROSS_PV_SECTORS=$((EXTENDED_SECTORS - SAME_PV_SECTORS))
MAPPER_NAME=linear_integration_vg-linear_integration_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmlinear

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

dep_token() {
    printf '(%s, %s)' "${1%%:*}" "${1##*:}"
}

check_linear_cross_table() {
    local table_file=$1 deps_file=$2 status_file=$3 dev1=$4 dev2=$5
    test "$(wc -l < "${table_file}")" -eq 2
    awk -v d1="${dev1}" -v d2="${dev2}" -v same="${SAME_PV_SECTORS}" -v cross="${CROSS_PV_SECTORS}" '
        $1 == 0 && $2 == same && $3 == "linear" && $4 == d1 { first = 1 }
        $1 == same && $2 == cross && $3 == "linear" && $4 == d2 { second = 1 }
        END { exit !(first && second) }
    ' "${table_file}"
    awk -v len="${EXTENDED_SECTORS}" '$3 == "linear" { sum += $2 } END { exit !(sum == len) }' "${status_file}"
    grep -q '2 dependencies' "${deps_file}"
    grep -F -q "$(dep_token "${dev1}")" "${deps_file}"
    grep -F -q "$(dep_token "${dev2}")" "${deps_file}"
}

check_linear_shrunk_table() {
    local table_file=$1 deps_file=$2 status_file=$3 dev1=$4
    test "$(wc -l < "${table_file}")" -eq 1
    awk -v len="${SHRUNK_SECTORS}" -v dev="${dev1}" '$1 == 0 && $2 == len && $3 == "linear" && $4 == dev { ok = 1 } END { exit !ok }' "${table_file}"
    awk -v len="${SHRUNK_SECTORS}" '$1 == 0 && $2 == len && $3 == "linear" { ok = 1 } END { exit !ok }' "${status_file}"
    grep -q '1 dependencies' "${deps_file}"
    grep -F -q "$(dep_token "${dev1}")" "${deps_file}"
}

echo '=== STEP 1: recover the cross-PV linear LV after reboot ==='
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "${TEST_DISK}" "${TEST_DISK2}"
test "${TEST_DISK}" != "${TEST_DISK2}"
test -b "${TEST_DISK}"
test -b "${TEST_DISK2}"
DEV1=$(devno "${TEST_DISK}")
DEV2=$(devno "${TEST_DISK2}")
printf 'DEV1=%s\nDEV2=%s\n' "${DEV1}" "${DEV2}"
pvscan
vgscan --mknodes
vgchange --config "${LVM_CONFIG}" -ay linear_integration_vg
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices linear_integration_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,devices linear_integration_vg/linear_integration_lv
echo 'DM_TABLE_LVM2_LINEAR_RECOVERED_BEGIN'
dmsetup table "${MAPPER_NAME}" | tee /tmp/linear-integration-recovered-table.txt | sed 's/^/DM_TABLE_LVM2_LINEAR_RECOVERED /'
echo 'DM_TABLE_LVM2_LINEAR_RECOVERED_END'
dmsetup status "${MAPPER_NAME}" | tee /tmp/linear-integration-recovered-status.txt
dmsetup deps "${MAPPER_NAME}" | tee /tmp/linear-integration-recovered-deps.txt
check_linear_cross_table /tmp/linear-integration-recovered-table.txt /tmp/linear-integration-recovered-deps.txt /tmp/linear-integration-recovered-status.txt "${DEV1}" "${DEV2}"
echo CHECK_PASS_LINEAR_LVM2_RECOVERED_TABLE_STATUS_DEPS

echo '=== STEP 2: verify files and shrink back to one segment ==='
mkdir -p "${MOUNT_DIR}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c linear-integration.md5
    cat base-marker.txt
    cat grow-marker.txt
    rm "grow${LINEAR_GROW_FILE_MIB}.bin" grow-marker.txt
    md5sum "base${LINEAR_BASE_FILE_MIB}.bin" base-marker.txt > linear-integration.md5
    md5sum -c linear-integration.md5
)
umount "${MOUNT_DIR}"
e2fsck -f -y "${MAPPER_DEVICE}"
resize2fs "${MAPPER_DEVICE}" "${LINEAR_SHRUNK_LV_MIB}M"
e2fsck -f -y "${MAPPER_DEVICE}"
lvreduce --config "${LVM_CONFIG}" -y -L "${LINEAR_SHRUNK_LV_MIB}M" linear_integration_vg/linear_integration_lv
lvs -a -o lv_name,lv_size,seg_count,devices linear_integration_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,devices linear_integration_vg/linear_integration_lv
echo 'DM_TABLE_LVM2_LINEAR_SHRUNK_BEGIN'
dmsetup table "${MAPPER_NAME}" | tee /tmp/linear-integration-shrunk-table.txt | sed 's/^/DM_TABLE_LVM2_LINEAR_SHRUNK /'
echo 'DM_TABLE_LVM2_LINEAR_SHRUNK_END'
dmsetup status "${MAPPER_NAME}" | tee /tmp/linear-integration-shrunk-status.txt
dmsetup deps "${MAPPER_NAME}" | tee /tmp/linear-integration-shrunk-deps.txt
check_linear_shrunk_table /tmp/linear-integration-shrunk-table.txt /tmp/linear-integration-shrunk-deps.txt /tmp/linear-integration-shrunk-status.txt "${DEV1}"
echo CHECK_PASS_LINEAR_LVM2_SHRUNK_TABLE_STATUS_DEPS

vgchange --config "${LVM_CONFIG}" -an linear_integration_vg
sync
echo CHECK_PASS_LINEAR_LVM2_SHRINK_DEACTIVATED

echo TEST_PASS_DM_LINEAR_INTEGRATION_SECOND
poweroff
GUEST_SECOND
} >"${SECOND_GUEST_SCRIPT}"

THIRD_GUEST_SCRIPT=$(mktemp /tmp/dm-linear-integration-third.XXXXXX)
{
    printf 'LINEAR_INITIAL_LV_MIB=%q\n' "${LINEAR_INITIAL_LV_MIB}"
    printf 'LINEAR_SHRUNK_LV_MIB=%q\n' "${LINEAR_SHRUNK_LV_MIB}"
    printf 'LINEAR_BASE_FILE_MIB=%q\n' "${LINEAR_BASE_FILE_MIB}"
    printf 'LINEAR_GROW_FILE_MIB=%q\n' "${LINEAR_GROW_FILE_MIB}"
    cat <<'GUEST_THIRD'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_LINEAR_INTEGRATION_THIRD status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
SHRUNK_SECTORS=$((LINEAR_SHRUNK_LV_MIB * 2048))
MAPPER_NAME=linear_integration_vg-linear_integration_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmlinear

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

dep_token() {
    printf '(%s, %s)' "${1%%:*}" "${1##*:}"
}

echo '=== STEP 1: recover the shrunken linear LV after reboot ==='
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "${TEST_DISK}" "${TEST_DISK2}"
test "${TEST_DISK}" != "${TEST_DISK2}"
test -b "${TEST_DISK}"
test -b "${TEST_DISK2}"
DEV1=$(devno "${TEST_DISK}")
DEV2=$(devno "${TEST_DISK2}")
printf 'DEV1=%s\nDEV2=%s\n' "${DEV1}" "${DEV2}"
pvscan
vgscan --mknodes
vgchange --config "${LVM_CONFIG}" -ay linear_integration_vg
lvs -a -o lv_name,lv_size,seg_count,devices linear_integration_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,devices linear_integration_vg/linear_integration_lv
echo 'DM_TABLE_LVM2_LINEAR_SHRUNK_RECOVERED_BEGIN'
dmsetup table "${MAPPER_NAME}" | tee /tmp/linear-integration-shrunk-recovered-table.txt | sed 's/^/DM_TABLE_LVM2_LINEAR_SHRUNK_RECOVERED /'
echo 'DM_TABLE_LVM2_LINEAR_SHRUNK_RECOVERED_END'
dmsetup status "${MAPPER_NAME}" | tee /tmp/linear-integration-shrunk-recovered-status.txt
dmsetup deps "${MAPPER_NAME}" | tee /tmp/linear-integration-shrunk-recovered-deps.txt
test "$(wc -l < /tmp/linear-integration-shrunk-recovered-table.txt)" -eq 1
awk -v len="${SHRUNK_SECTORS}" -v dev="${DEV1}" '$1 == 0 && $2 == len && $3 == "linear" && $4 == dev { ok = 1 } END { exit !ok }' /tmp/linear-integration-shrunk-recovered-table.txt
awk -v len="${SHRUNK_SECTORS}" '$1 == 0 && $2 == len && $3 == "linear" { ok = 1 } END { exit !ok }' /tmp/linear-integration-shrunk-recovered-status.txt
grep -q '1 dependencies' /tmp/linear-integration-shrunk-recovered-deps.txt
grep -F -q "$(dep_token "${DEV1}")" /tmp/linear-integration-shrunk-recovered-deps.txt
echo CHECK_PASS_LINEAR_LVM2_SHRUNK_RECOVERED_TABLE_STATUS_DEPS

echo '=== STEP 2: verify shrunken ext2 data from a read-only mount ==='
mkdir -p "${MOUNT_DIR}"
mount -o ro -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c linear-integration.md5
    grep -Fx 'lvm2 linear integration base' base-marker.txt
    grep -Fx "initial_lv_mib=${LINEAR_INITIAL_LV_MIB}" base-marker.txt
    test ! -e "grow${LINEAR_GROW_FILE_MIB}.bin"
    test ! -e grow-marker.txt
)
df -h "${MOUNT_DIR}"
du -sh "${MOUNT_DIR}" "${MOUNT_DIR}/base${LINEAR_BASE_FILE_MIB}.bin"
umount "${MOUNT_DIR}"
vgchange --config "${LVM_CONFIG}" -an linear_integration_vg
sync
echo CHECK_PASS_LINEAR_LVM2_SHRINK_RECOVERY_FILE_MD5

echo TEST_PASS_DM_LINEAR_INTEGRATION_THIRD
poweroff
GUEST_THIRD
} >"${THIRD_GUEST_SCRIPT}"

SUMMARY_INCLUDE='^TEST_|^CHECK_PASS_|^=== STEP|^=== CHECK|^TEST_DISK=|^TEST_DISK2=|^DEV1=|^DEV2=|^DM_TABLE_LVM2_LINEAR|^linear[[:space:]]|^0 [0-9]+ linear |^[0-9]+ dependencies|^Resizing the filesystem|^The filesystem on|base[0-9]+\.bin: OK|grow[0-9]+\.bin: OK|base-marker\.txt: OK|grow-marker\.txt: OK|lvm2 linear integration|initial_lv_mib=|same_pv_lv_mib=|extended_lv_mib=|No space left|Input/output error|Command failed|Kernel panic|panicked|records in|records out|bytes .* copied'
dm_run_three_guest_test \
    "${TEST_ID}" \
    "${FIRST_GUEST_SCRIPT}" \
    "${SECOND_GUEST_SCRIPT}" \
    "${THIRD_GUEST_SCRIPT}" \
    TEST_PASS_DM_LINEAR_INTEGRATION_FIRST \
    TEST_PASS_DM_LINEAR_INTEGRATION_SECOND \
    TEST_PASS_DM_LINEAR_INTEGRATION_THIRD \
    "${SUMMARY_INCLUDE}"
