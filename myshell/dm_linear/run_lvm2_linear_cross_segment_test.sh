#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/dm_linear/run_lvm2_linear_cross_segment_test.sh

Runs an independent two-guest NixOS regression for LVM2 linear cross-segment table creation, reboot recovery, shrink, and file I/O.

Optional environment variables:
  DM_TEST_IMAGE                         First backing test image path, default target/nixos/test.img
  DM_TEST_IMAGE_2                       Second backing test image path, default target/nixos/test2.img
  DM_LINEAR_LVM2_CROSS_SEGMENT_LOG      Host-side log path, default /tmp/dm-linear-lvm2-cross-segment-test.log
  GUEST_QEMU_TIMEOUT                    Full QEMU lifecycle timeout in seconds, default 180
  GUEST_READY_TIMEOUT                   Compatibility alias if GUEST_QEMU_TIMEOUT is unset
  RESET_DM_TEST_IMAGES                  1 to delete test images before running, default 1
  LINEAR_CS_INITIAL_LV_MIB              Initial single-PV LV size in MiB, default 256
  LINEAR_CS_EXTENDED_LV_MIB             Cross-segment extended LV size in MiB, default 512
  LINEAR_CS_SHRUNK_LV_MIB               Final shrunk LV size in MiB, default 256
  LINEAR_CS_BASE_FILE_MIB               Base test file size in MiB, default 64
  LINEAR_CS_GROW_FILE_MIB               Post-cross-segment test file size in MiB, default 240

Expected success markers:
  TEST_PASS_DM_LINEAR_LVM2_CROSS_SEGMENT_FIRST
  TEST_PASS_DM_LINEAR_LVM2_CROSS_SEGMENT_SECOND
  HOST_PASS_DM_LINEAR_LVM2_CROSS_SEGMENT
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/../..")
source "${SCRIPT_DIR}/../lib/dm_nixos_test.sh"

TEST_ID=DM_LINEAR_LVM2_CROSS_SEGMENT
LOG=${DM_LINEAR_LVM2_CROSS_SEGMENT_LOG:-/tmp/dm-linear-lvm2-cross-segment-test.log}
DM_TEST_IMAGE=${DM_TEST_IMAGE:-target/nixos/test.img}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-target/nixos/test2.img}
DM_TEST_IMAGES=${DM_TEST_IMAGES:-${DM_TEST_IMAGE} ${DM_TEST_IMAGE_2}}
GUEST_QEMU_TIMEOUT=${GUEST_QEMU_TIMEOUT:-${GUEST_READY_TIMEOUT:-180}}
RESET_DM_TEST_IMAGES=${RESET_DM_TEST_IMAGES:-1}
LINEAR_CS_INITIAL_LV_MIB=${LINEAR_CS_INITIAL_LV_MIB:-256}
LINEAR_CS_EXTENDED_LV_MIB=${LINEAR_CS_EXTENDED_LV_MIB:-512}
LINEAR_CS_SHRUNK_LV_MIB=${LINEAR_CS_SHRUNK_LV_MIB:-256}
LINEAR_CS_BASE_FILE_MIB=${LINEAR_CS_BASE_FILE_MIB:-64}
LINEAR_CS_GROW_FILE_MIB=${LINEAR_CS_GROW_FILE_MIB:-240}

test "${LINEAR_CS_INITIAL_LV_MIB}" -lt "${LINEAR_CS_EXTENDED_LV_MIB}"
test "${LINEAR_CS_SHRUNK_LV_MIB}" -lt "${LINEAR_CS_EXTENDED_LV_MIB}"
test $((LINEAR_CS_EXTENDED_LV_MIB - LINEAR_CS_INITIAL_LV_MIB)) -gt 0
test "${LINEAR_CS_BASE_FILE_MIB}" -lt "${LINEAR_CS_INITIAL_LV_MIB}"
test $((LINEAR_CS_BASE_FILE_MIB + LINEAR_CS_GROW_FILE_MIB)) -gt "${LINEAR_CS_INITIAL_LV_MIB}"
test $((LINEAR_CS_BASE_FILE_MIB + LINEAR_CS_GROW_FILE_MIB)) -lt "${LINEAR_CS_EXTENDED_LV_MIB}"
test "${LINEAR_CS_BASE_FILE_MIB}" -lt "${LINEAR_CS_SHRUNK_LV_MIB}"

cd "${ASTERINAS_DIR}"
dm_prepare_nixos_test "${TEST_ID}"
echo "HOST_INFO_${TEST_ID} qemu_lifecycle_timeout=${GUEST_QEMU_TIMEOUT}s"
echo "HOST_INFO_${TEST_ID} disk1=${DM_TEST_IMAGE} serial=vdmtest"
echo "HOST_INFO_${TEST_ID} disk2=${DM_TEST_IMAGE_2} serial=vdmtest2"
echo "HOST_INFO_${TEST_ID} initial_lv_mib=${LINEAR_CS_INITIAL_LV_MIB} extended_lv_mib=${LINEAR_CS_EXTENDED_LV_MIB} shrunk_lv_mib=${LINEAR_CS_SHRUNK_LV_MIB} base_file_mib=${LINEAR_CS_BASE_FILE_MIB} grow_file_mib=${LINEAR_CS_GROW_FILE_MIB}"

FIRST_GUEST_SCRIPT=$(mktemp /tmp/dm-linear-lvm2-cross-segment-first.XXXXXX)
{
    printf 'LINEAR_CS_INITIAL_LV_MIB=%q\n' "${LINEAR_CS_INITIAL_LV_MIB}"
    printf 'LINEAR_CS_EXTENDED_LV_MIB=%q\n' "${LINEAR_CS_EXTENDED_LV_MIB}"
    printf 'LINEAR_CS_BASE_FILE_MIB=%q\n' "${LINEAR_CS_BASE_FILE_MIB}"
    printf 'LINEAR_CS_GROW_FILE_MIB=%q\n' "${LINEAR_CS_GROW_FILE_MIB}"
    cat <<'GUEST_FIRST'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_LINEAR_LVM2_CROSS_SEGMENT_FIRST status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
INITIAL_SECTORS=$((LINEAR_CS_INITIAL_LV_MIB * 2048))
EXTENDED_SECTORS=$((LINEAR_CS_EXTENDED_LV_MIB * 2048))
GROW_SECTORS=$((EXTENDED_SECTORS - INITIAL_SECTORS))
MAPPER_NAME=linear_cs_vg-linear_cs_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmlinearcs

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

dep_token() {
    printf '(%s, %s)' "${1%%:*}" "${1##*:}"
}

check_linear_cross_table() {
    local table_file=$1 deps_file=$2 status_file=$3 dev1=$4 dev2=$5
    test "$(wc -l < "${table_file}")" -eq 2
    awk -v d1="${dev1}" -v d2="${dev2}" -v initial="${INITIAL_SECTORS}" -v grow="${GROW_SECTORS}" '
        $1 == 0 && $2 == initial && $3 == "linear" && $4 == d1 { first = 1 }
        $1 == initial && $2 == grow && $3 == "linear" && $4 == d2 { second = 1 }
        END { exit !(first && second) }
    ' "${table_file}"
    awk -v len="${EXTENDED_SECTORS}" '$3 == "linear" { sum += $2 } END { exit !(sum == len) }' "${status_file}"
    grep -q '2 dependencies' "${deps_file}"
    grep -F -q "$(dep_token "${dev1}")" "${deps_file}"
    grep -F -q "$(dep_token "${dev2}")" "${deps_file}"
}

check_linear_cross_table_status_deps() {
    local label=$1 table_file=$2 status_file=$3 deps_file=$4 dev1=$5 dev2=$6
    echo "DM_TABLE_LVM2_LINEAR_CS_${label}_BEGIN"
    dmsetup table "${MAPPER_NAME}" | tee "${table_file}" | sed "s/^/DM_TABLE_LVM2_LINEAR_CS_${label} /"
    echo "DM_TABLE_LVM2_LINEAR_CS_${label}_END"
    dmsetup status "${MAPPER_NAME}" | tee "${status_file}"
    dmsetup deps "${MAPPER_NAME}" | tee "${deps_file}"
    check_linear_cross_table "${table_file}" "${deps_file}" "${status_file}" "${dev1}" "${dev2}"
}

echo '=== STEP 1: check dm control device, linear target, and two test disks ==='
test -c /dev/mapper/control
dmsetup targets | tee /tmp/linear-cs-targets.txt
grep -q '^linear' /tmp/linear-cs-targets.txt
TEST_DISK=$(aster-dm-disk-locator)
TEST_DISK2=$(aster-dm-disk-locator vdmtest2)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\n' "${TEST_DISK}" "${TEST_DISK2}"
test "${TEST_DISK}" != "${TEST_DISK2}"
test -b "${TEST_DISK}"
test -b "${TEST_DISK2}"
DEV1=$(devno "${TEST_DISK}")
DEV2=$(devno "${TEST_DISK2}")
printf 'DEV1=%s\nDEV2=%s\n' "${DEV1}" "${DEV2}"
echo CHECK_PASS_LINEAR_CS_LVM2_SETUP

echo '=== STEP 2: create single-PV linear LV and base data ==='
pvcreate "${TEST_DISK}"
vgcreate linear_cs_vg "${TEST_DISK}"
lvcreate --config "${LVM_CONFIG}" --type linear -L "${LINEAR_CS_INITIAL_LV_MIB}M" -n linear_cs_lv linear_cs_vg "${TEST_DISK}"
mkfs.ext2 -F -b 4096 "${MAPPER_DEVICE}"
mkdir -p "${MOUNT_DIR}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
dd if=/dev/urandom of="${MOUNT_DIR}/base${LINEAR_CS_BASE_FILE_MIB}.bin" bs=1M count="${LINEAR_CS_BASE_FILE_MIB}" conv=fsync status=none
printf 'lvm2 linear cross-segment base\ninitial_lv_mib=%s\n' "${LINEAR_CS_INITIAL_LV_MIB}" > "${MOUNT_DIR}/base-marker.txt"
(
    cd "${MOUNT_DIR}"
    md5sum "base${LINEAR_CS_BASE_FILE_MIB}.bin" base-marker.txt > linear-cs.md5
    md5sum -c linear-cs.md5
)
sync
umount "${MOUNT_DIR}"
echo CHECK_PASS_LINEAR_CS_LVM2_BASE_FILE_MD5

echo '=== STEP 3: add second PV and extend into a second linear segment ==='
pvcreate "${TEST_DISK2}"
vgextend linear_cs_vg "${TEST_DISK2}"
lvextend --config "${LVM_CONFIG}" -L "${LINEAR_CS_EXTENDED_LV_MIB}M" linear_cs_vg/linear_cs_lv "${TEST_DISK2}"
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices linear_cs_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,devices linear_cs_vg/linear_cs_lv
check_linear_cross_table_status_deps EXTENDED /tmp/linear-cs-extended-table.txt /tmp/linear-cs-extended-status.txt /tmp/linear-cs-extended-deps.txt "${DEV1}" "${DEV2}"
echo CHECK_PASS_LINEAR_CS_LVM2_EXTENDED_TABLE_STATUS_DEPS

e2fsck -f -y "${MAPPER_DEVICE}"
resize2fs "${MAPPER_DEVICE}"
e2fsck -f -y "${MAPPER_DEVICE}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c linear-cs.md5
)
dd if=/dev/urandom of="${MOUNT_DIR}/grow${LINEAR_CS_GROW_FILE_MIB}.bin" bs=1M count="${LINEAR_CS_GROW_FILE_MIB}" conv=fsync status=none
printf 'lvm2 linear cross-segment grow\nextended_lv_mib=%s\n' "${LINEAR_CS_EXTENDED_LV_MIB}" > "${MOUNT_DIR}/grow-marker.txt"
(
    cd "${MOUNT_DIR}"
    md5sum "grow${LINEAR_CS_GROW_FILE_MIB}.bin" grow-marker.txt >> linear-cs.md5
    md5sum -c linear-cs.md5
)
sync
umount "${MOUNT_DIR}"
vgchange --config "${LVM_CONFIG}" -an linear_cs_vg
sync
echo CHECK_PASS_LINEAR_CS_LVM2_GROW_FILE_MD5

echo TEST_PASS_DM_LINEAR_LVM2_CROSS_SEGMENT_FIRST
poweroff
GUEST_FIRST
} >"${FIRST_GUEST_SCRIPT}"

SECOND_GUEST_SCRIPT=$(mktemp /tmp/dm-linear-lvm2-cross-segment-second.XXXXXX)
{
    printf 'LINEAR_CS_INITIAL_LV_MIB=%q\n' "${LINEAR_CS_INITIAL_LV_MIB}"
    printf 'LINEAR_CS_EXTENDED_LV_MIB=%q\n' "${LINEAR_CS_EXTENDED_LV_MIB}"
    printf 'LINEAR_CS_SHRUNK_LV_MIB=%q\n' "${LINEAR_CS_SHRUNK_LV_MIB}"
    printf 'LINEAR_CS_BASE_FILE_MIB=%q\n' "${LINEAR_CS_BASE_FILE_MIB}"
    printf 'LINEAR_CS_GROW_FILE_MIB=%q\n' "${LINEAR_CS_GROW_FILE_MIB}"
    cat <<'GUEST_SECOND'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_LINEAR_LVM2_CROSS_SEGMENT_SECOND status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
INITIAL_SECTORS=$((LINEAR_CS_INITIAL_LV_MIB * 2048))
EXTENDED_SECTORS=$((LINEAR_CS_EXTENDED_LV_MIB * 2048))
SHRUNK_SECTORS=$((LINEAR_CS_SHRUNK_LV_MIB * 2048))
GROW_SECTORS=$((EXTENDED_SECTORS - INITIAL_SECTORS))
MAPPER_NAME=linear_cs_vg-linear_cs_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmlinearcs

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

dep_token() {
    printf '(%s, %s)' "${1%%:*}" "${1##*:}"
}

check_linear_cross_table() {
    local table_file=$1 deps_file=$2 status_file=$3 dev1=$4 dev2=$5
    test "$(wc -l < "${table_file}")" -eq 2
    awk -v d1="${dev1}" -v d2="${dev2}" -v initial="${INITIAL_SECTORS}" -v grow="${GROW_SECTORS}" '
        $1 == 0 && $2 == initial && $3 == "linear" && $4 == d1 { first = 1 }
        $1 == initial && $2 == grow && $3 == "linear" && $4 == d2 { second = 1 }
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

echo '=== STEP 1: recover cross-segment linear LV after reboot ==='
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
vgchange --config "${LVM_CONFIG}" -ay linear_cs_vg
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices linear_cs_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,devices linear_cs_vg/linear_cs_lv
echo 'DM_TABLE_LVM2_LINEAR_CS_RECOVERED_BEGIN'
dmsetup table "${MAPPER_NAME}" | tee /tmp/linear-cs-recovered-table.txt | sed 's/^/DM_TABLE_LVM2_LINEAR_CS_RECOVERED /'
echo 'DM_TABLE_LVM2_LINEAR_CS_RECOVERED_END'
dmsetup status "${MAPPER_NAME}" | tee /tmp/linear-cs-recovered-status.txt
dmsetup deps "${MAPPER_NAME}" | tee /tmp/linear-cs-recovered-deps.txt
check_linear_cross_table /tmp/linear-cs-recovered-table.txt /tmp/linear-cs-recovered-deps.txt /tmp/linear-cs-recovered-status.txt "${DEV1}" "${DEV2}"
echo CHECK_PASS_LINEAR_CS_LVM2_RECOVERED_TABLE_STATUS_DEPS

echo '=== STEP 2: verify files and shrink back to one segment ==='
mkdir -p "${MOUNT_DIR}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c linear-cs.md5
    cat base-marker.txt
    cat grow-marker.txt
    rm "grow${LINEAR_CS_GROW_FILE_MIB}.bin" grow-marker.txt
    md5sum "base${LINEAR_CS_BASE_FILE_MIB}.bin" base-marker.txt > linear-cs.md5
    md5sum -c linear-cs.md5
)
umount "${MOUNT_DIR}"
e2fsck -f -y "${MAPPER_DEVICE}"
resize2fs "${MAPPER_DEVICE}" "${LINEAR_CS_SHRUNK_LV_MIB}M"
e2fsck -f -y "${MAPPER_DEVICE}"
lvreduce --config "${LVM_CONFIG}" -y -L "${LINEAR_CS_SHRUNK_LV_MIB}M" linear_cs_vg/linear_cs_lv
lvs -a -o lv_name,lv_size,seg_count,devices linear_cs_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,devices linear_cs_vg/linear_cs_lv
echo 'DM_TABLE_LVM2_LINEAR_CS_SHRUNK_BEGIN'
dmsetup table "${MAPPER_NAME}" | tee /tmp/linear-cs-shrunk-table.txt | sed 's/^/DM_TABLE_LVM2_LINEAR_CS_SHRUNK /'
echo 'DM_TABLE_LVM2_LINEAR_CS_SHRUNK_END'
dmsetup status "${MAPPER_NAME}" | tee /tmp/linear-cs-shrunk-status.txt
dmsetup deps "${MAPPER_NAME}" | tee /tmp/linear-cs-shrunk-deps.txt
check_linear_shrunk_table /tmp/linear-cs-shrunk-table.txt /tmp/linear-cs-shrunk-deps.txt /tmp/linear-cs-shrunk-status.txt "${DEV1}"
echo CHECK_PASS_LINEAR_CS_LVM2_SHRUNK_TABLE_STATUS_DEPS

mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c linear-cs.md5
)
df -h "${MOUNT_DIR}"
du -sh "${MOUNT_DIR}" "${MOUNT_DIR}/base${LINEAR_CS_BASE_FILE_MIB}.bin"
umount "${MOUNT_DIR}"
vgchange --config "${LVM_CONFIG}" -an linear_cs_vg
sync
echo CHECK_PASS_LINEAR_CS_LVM2_SHRINK_FILE_MD5

echo TEST_PASS_DM_LINEAR_LVM2_CROSS_SEGMENT_SECOND
poweroff
GUEST_SECOND
} >"${SECOND_GUEST_SCRIPT}"

SUMMARY_INCLUDE='^TEST_|^CHECK_PASS_|^=== STEP|^=== CHECK|^TEST_DISK=|^TEST_DISK2=|^DEV1=|^DEV2=|^DM_TABLE_LVM2_LINEAR_CS|^linear[[:space:]]|^0 [0-9]+ linear |^[0-9]+ dependencies|^Resizing the filesystem|^The filesystem on|base[0-9]+\.bin: OK|grow[0-9]+\.bin: OK|base-marker\.txt: OK|grow-marker\.txt: OK|lvm2 linear cross-segment|initial_lv_mib=|extended_lv_mib=|No space left|Input/output error|Command failed|Kernel panic|panicked|records in|records out|bytes .* copied'
dm_run_two_guest_test \
    "${TEST_ID}" \
    "${FIRST_GUEST_SCRIPT}" \
    "${SECOND_GUEST_SCRIPT}" \
    TEST_PASS_DM_LINEAR_LVM2_CROSS_SEGMENT_FIRST \
    TEST_PASS_DM_LINEAR_LVM2_CROSS_SEGMENT_SECOND \
    "${SUMMARY_INCLUDE}"
