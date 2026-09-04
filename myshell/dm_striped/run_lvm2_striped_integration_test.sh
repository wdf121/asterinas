#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/dm_striped/run_lvm2_striped_integration_test.sh

Runs a three-guest NixOS integration test for LVM2 striped storage. It validates
N-way creation, same-backing-set growth, growth onto a second backing set, ext2
I/O across the segment boundary, recovery after growth, shrink to one segment,
and recovery after shrink.

Optional environment variables:
  DM_TEST_IMAGES                    Backing test image list, default 2 * STRIPED_PV_COUNT images
  DM_STRIPED_INTEGRATION_LOG        Host-side log path, default /tmp/dm-striped-integration-test.log
  GUEST_QEMU_TIMEOUT                Full QEMU lifecycle timeout in seconds, default 180
  GUEST_READY_TIMEOUT               Compatibility alias if GUEST_QEMU_TIMEOUT is unset
  RESET_DM_TEST_IMAGES              1 to delete test images before running, default 1
  STRIPED_PV_COUNT                  Number of PVs per striped segment, default 2
  STRIPED_INITIAL_LV_MIB            Initial LV size in MiB, default STRIPED_PV_COUNT * 256
  STRIPED_SAME_SET_LV_MIB           Same-set extended size, default initial + STRIPED_PV_COUNT * 32
  STRIPED_EXTENDED_LV_MIB           Cross-set extended size, default initial * 2
  STRIPED_SHRUNK_LV_MIB             Final shrunk size, default initial
  STRIPED_BASE_FILE_MIB             Base test file size, default STRIPED_PV_COUNT * 64
  STRIPED_GROW_FILE_MIB             Post-cross-set test file size, default STRIPED_PV_COUNT * 240
  STRIPED_CHUNK_KIB                 LVM stripe chunk size in KiB, default 4

Expected success markers:
  TEST_PASS_DM_STRIPED_INTEGRATION_FIRST
  TEST_PASS_DM_STRIPED_INTEGRATION_SECOND
  TEST_PASS_DM_STRIPED_INTEGRATION_THIRD
  HOST_PASS_DM_STRIPED_INTEGRATION
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/../..")
source "${SCRIPT_DIR}/../lib/dm_nixos_test.sh"

TEST_ID=DM_STRIPED_INTEGRATION
LOG=${DM_STRIPED_INTEGRATION_LOG:-/tmp/dm-striped-integration-test.log}
STRIPED_PV_COUNT=${STRIPED_PV_COUNT:-2}
STRIPED_TOTAL_PV_COUNT=$((STRIPED_PV_COUNT * 2))
STRIPED_INITIAL_LV_MIB=${STRIPED_INITIAL_LV_MIB:-$((STRIPED_PV_COUNT * 256))}
STRIPED_SAME_SET_LV_MIB=${STRIPED_SAME_SET_LV_MIB:-$((STRIPED_INITIAL_LV_MIB + STRIPED_PV_COUNT * 32))}
STRIPED_EXTENDED_LV_MIB=${STRIPED_EXTENDED_LV_MIB:-$((STRIPED_INITIAL_LV_MIB * 2))}
STRIPED_SHRUNK_LV_MIB=${STRIPED_SHRUNK_LV_MIB:-${STRIPED_INITIAL_LV_MIB}}
STRIPED_BASE_FILE_MIB=${STRIPED_BASE_FILE_MIB:-$((STRIPED_PV_COUNT * 64))}
STRIPED_GROW_FILE_MIB=${STRIPED_GROW_FILE_MIB:-$((STRIPED_PV_COUNT * 240))}
STRIPED_CHUNK_KIB=${STRIPED_CHUNK_KIB:-4}
GUEST_QEMU_TIMEOUT=${GUEST_QEMU_TIMEOUT:-180}
GUEST_READY_TIMEOUT=${GUEST_READY_TIMEOUT:-40}
RESET_DM_TEST_IMAGES=${RESET_DM_TEST_IMAGES:-1}

test "${STRIPED_PV_COUNT}" -ge 2
test $((STRIPED_INITIAL_LV_MIB % STRIPED_PV_COUNT)) -eq 0
test $((STRIPED_SAME_SET_LV_MIB % STRIPED_PV_COUNT)) -eq 0
test $((STRIPED_EXTENDED_LV_MIB % STRIPED_PV_COUNT)) -eq 0
test $((STRIPED_SHRUNK_LV_MIB % STRIPED_PV_COUNT)) -eq 0
test "${STRIPED_INITIAL_LV_MIB}" -lt "${STRIPED_SAME_SET_LV_MIB}"
test "${STRIPED_SAME_SET_LV_MIB}" -lt "${STRIPED_EXTENDED_LV_MIB}"
test "${STRIPED_SHRUNK_LV_MIB}" -lt "${STRIPED_EXTENDED_LV_MIB}"
test "${STRIPED_BASE_FILE_MIB}" -lt "${STRIPED_INITIAL_LV_MIB}"
test $((STRIPED_BASE_FILE_MIB + STRIPED_GROW_FILE_MIB)) -gt "${STRIPED_SAME_SET_LV_MIB}"
test $((STRIPED_BASE_FILE_MIB + STRIPED_GROW_FILE_MIB)) -lt "${STRIPED_EXTENDED_LV_MIB}"
test "${STRIPED_BASE_FILE_MIB}" -lt "${STRIPED_SHRUNK_LV_MIB}"

if [ -z "${DM_TEST_IMAGES:-}" ]; then
    images=()
    index=1
    while [ "${index}" -le "${STRIPED_TOTAL_PV_COUNT}" ]; do
        if [ "${index}" -eq 1 ]; then
            image_path=${DM_TEST_IMAGE:-target/nixos/test.img}
        else
            var_name="DM_TEST_IMAGE_${index}"
            image_path=${!var_name:-target/nixos/test${index}.img}
        fi
        images+=("${image_path}")
        index=$((index + 1))
    done
    DM_TEST_IMAGES=${images[*]}
else
    set -- ${DM_TEST_IMAGES}
    test "$#" -eq "${STRIPED_TOTAL_PV_COUNT}"
fi
DM_TEST_IMAGE=${DM_TEST_IMAGE:-target/nixos/test.img}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-target/nixos/test2.img}

cd "${ASTERINAS_DIR}"
dm_prepare_nixos_test "${TEST_ID}"
echo "HOST_INFO_${TEST_ID} qemu_lifecycle_timeout=${GUEST_QEMU_TIMEOUT}s"
index=1
for image_path in ${DM_TEST_IMAGES}; do
    if [ "${index}" -eq 1 ]; then
        serial=vdmtest
    else
        serial=vdmtest${index}
    fi
    echo "HOST_INFO_${TEST_ID} disk${index}=${image_path} serial=${serial}"
    index=$((index + 1))
done
echo "HOST_INFO_${TEST_ID} pv_count=${STRIPED_PV_COUNT} total_pv_count=${STRIPED_TOTAL_PV_COUNT} initial_lv_mib=${STRIPED_INITIAL_LV_MIB} same_set_lv_mib=${STRIPED_SAME_SET_LV_MIB} extended_lv_mib=${STRIPED_EXTENDED_LV_MIB} shrunk_lv_mib=${STRIPED_SHRUNK_LV_MIB} base_file_mib=${STRIPED_BASE_FILE_MIB} grow_file_mib=${STRIPED_GROW_FILE_MIB} chunk_kib=${STRIPED_CHUNK_KIB}"

FIRST_GUEST_SCRIPT=$(mktemp /tmp/dm-striped-integration-first.XXXXXX)
{
    printf 'STRIPED_PV_COUNT=%q\n' "${STRIPED_PV_COUNT}"
    printf 'STRIPED_TOTAL_PV_COUNT=%q\n' "${STRIPED_TOTAL_PV_COUNT}"
    printf 'STRIPED_INITIAL_LV_MIB=%q\n' "${STRIPED_INITIAL_LV_MIB}"
    printf 'STRIPED_SAME_SET_LV_MIB=%q\n' "${STRIPED_SAME_SET_LV_MIB}"
    printf 'STRIPED_EXTENDED_LV_MIB=%q\n' "${STRIPED_EXTENDED_LV_MIB}"
    printf 'STRIPED_BASE_FILE_MIB=%q\n' "${STRIPED_BASE_FILE_MIB}"
    printf 'STRIPED_GROW_FILE_MIB=%q\n' "${STRIPED_GROW_FILE_MIB}"
    printf 'STRIPED_CHUNK_KIB=%q\n' "${STRIPED_CHUNK_KIB}"
    cat <<'GUEST_FIRST'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_STRIPED_INTEGRATION_FIRST status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
INITIAL_SECTORS=$((STRIPED_INITIAL_LV_MIB * 2048))
SAME_SET_SECTORS=$((STRIPED_SAME_SET_LV_MIB * 2048))
EXTENDED_SECTORS=$((STRIPED_EXTENDED_LV_MIB * 2048))
CROSS_SET_SECTORS=$((EXTENDED_SECTORS - SAME_SET_SECTORS))
STRIPED_CHUNK_SECTORS=$((STRIPED_CHUNK_KIB * 2))
MAPPER_NAME=striped_integration_vg-striped_integration_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmstriped
DISKS=()
DEVS=()
BASE_DISKS=()
GROW_DISKS=()

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

dep_token() {
    printf '(%s, %s)' "${1%%:*}" "${1##*:}"
}

locate_striped_disks() {
    local count=$1 index serial disk label existing dev
    index=1
    while [ "${index}" -le "${count}" ]; do
        if [ "${index}" -eq 1 ]; then
            disk=$(aster-dm-disk-locator)
            label=TEST_DISK
        else
            serial=vdmtest${index}
            disk=$(aster-dm-disk-locator "${serial}")
            label=TEST_DISK${index}
        fi
        printf '%s=%s\n' "${label}" "${disk}"
        test -b "${disk}"
        for existing in "${DISKS[@]}"; do
            test "${existing}" != "${disk}"
        done
        DISKS+=("${disk}")
        dev=$(devno "${disk}")
        DEVS+=("${dev}")
        printf 'DEV%s=%s\n' "${index}" "${dev}"
        if [ "${index}" -le "${STRIPED_PV_COUNT}" ]; then
            BASE_DISKS+=("${disk}")
        else
            GROW_DISKS+=("${disk}")
        fi
        index=$((index + 1))
    done
}

check_row_devices() {
    local table_file=$1 start=$2 len=$3 devs=$4
    awk -v start="${start}" -v len="${len}" -v stripes="${STRIPED_PV_COUNT}" -v chunk="${STRIPED_CHUNK_SECTORS}" -v devs="${devs}" '
        BEGIN { n = split(devs, expected, " ") }
        $1 == start && $2 == len && $3 == "striped" && $4 == stripes && $5 == chunk {
            delete present
            for (i = 6; i <= NF; i += 2) present[$i] = 1
            ok = 1
            for (i = 1; i <= n; i++) if (!(expected[i] in present)) ok = 0
        }
        END { exit !ok }
    ' "${table_file}"
}

check_striped_cross_table() {
    local table_file=$1 deps_file=$2 status_file=$3 base_devs grow_devs dev
    base_devs="${DEVS[*]:0:${STRIPED_PV_COUNT}}"
    grow_devs="${DEVS[*]:${STRIPED_PV_COUNT}:${STRIPED_PV_COUNT}}"
    test "$(wc -l < "${table_file}")" -eq 2
    check_row_devices "${table_file}" 0 "${SAME_SET_SECTORS}" "${base_devs}"
    check_row_devices "${table_file}" "${SAME_SET_SECTORS}" "${CROSS_SET_SECTORS}" "${grow_devs}"
    awk -v len="${EXTENDED_SECTORS}" '$3 == "striped" { sum += $2 } END { exit !(sum == len) }' "${status_file}"
    grep -q "${STRIPED_TOTAL_PV_COUNT} dependencies" "${deps_file}"
    for dev in "${DEVS[@]}"; do
        grep -F -q "${dev}" "${table_file}"
        grep -F -q "$(dep_token "${dev}")" "${deps_file}"
    done
}

check_striped_single_table_status_deps() {
    local label=$1 expected_sectors=$2 table_file=$3 status_file=$4 deps_file=$5 base_devs dev index
    base_devs="${DEVS[*]:0:${STRIPED_PV_COUNT}}"
    echo "DM_TABLE_LVM2_STRIPED_${label}_BEGIN"
    dmsetup table "${MAPPER_NAME}" | tee "${table_file}" | sed "s/^/DM_TABLE_LVM2_STRIPED_${label} /"
    echo "DM_TABLE_LVM2_STRIPED_${label}_END"
    dmsetup status "${MAPPER_NAME}" | tee "${status_file}"
    dmsetup deps "${MAPPER_NAME}" | tee "${deps_file}"
    test "$(wc -l < "${table_file}")" -eq 1
    check_row_devices "${table_file}" 0 "${expected_sectors}" "${base_devs}"
    awk -v len="${expected_sectors}" '$3 == "striped" { sum += $2 } END { exit !(sum == len) }' "${status_file}"
    grep -q "${STRIPED_PV_COUNT} dependencies" "${deps_file}"
    index=0
    while [ "${index}" -lt "${STRIPED_PV_COUNT}" ]; do
        dev=${DEVS[${index}]}
        grep -F -q "${dev}" "${table_file}"
        grep -F -q "$(dep_token "${dev}")" "${deps_file}"
        index=$((index + 1))
    done
}

check_striped_cross_table_status_deps() {
    local label=$1 table_file=$2 status_file=$3 deps_file=$4
    echo "DM_TABLE_LVM2_STRIPED_${label}_BEGIN"
    dmsetup table "${MAPPER_NAME}" | tee "${table_file}" | sed "s/^/DM_TABLE_LVM2_STRIPED_${label} /"
    echo "DM_TABLE_LVM2_STRIPED_${label}_END"
    dmsetup status "${MAPPER_NAME}" | tee "${status_file}"
    dmsetup deps "${MAPPER_NAME}" | tee "${deps_file}"
    check_striped_cross_table "${table_file}" "${deps_file}" "${status_file}"
}

echo '=== STEP 1: check dm control device, striped target, and 2N test disks ==='
test -c /dev/mapper/control
dmsetup targets | tee /tmp/striped-integration-targets.txt
grep -q '^striped' /tmp/striped-integration-targets.txt
locate_striped_disks "${STRIPED_TOTAL_PV_COUNT}"
echo CHECK_PASS_STRIPED_LVM2_SETUP

echo '=== STEP 2: create N-PV striped LV and base data ==='
pvcreate "${BASE_DISKS[@]}"
vgcreate striped_integration_vg "${BASE_DISKS[@]}"
lvcreate --config "${LVM_CONFIG}" --type striped -i "${STRIPED_PV_COUNT}" -I "${STRIPED_CHUNK_KIB}K" -L "${STRIPED_INITIAL_LV_MIB}M" -n striped_integration_lv striped_integration_vg "${BASE_DISKS[@]}"
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices striped_integration_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices striped_integration_vg/striped_integration_lv
check_striped_single_table_status_deps INITIAL "${INITIAL_SECTORS}" /tmp/striped-integration-initial-table.txt /tmp/striped-integration-initial-status.txt /tmp/striped-integration-initial-deps.txt
echo CHECK_PASS_STRIPED_LVM2_INITIAL_TABLE_STATUS_DEPS
mkfs.ext2 -F -b 4096 "${MAPPER_DEVICE}"
mkdir -p "${MOUNT_DIR}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
dd if=/dev/urandom of="${MOUNT_DIR}/base${STRIPED_BASE_FILE_MIB}.bin" bs=1M count="${STRIPED_BASE_FILE_MIB}" conv=fsync status=none
printf 'lvm2 striped integration base\npv_count=%s initial_lv_mib=%s\n' "${STRIPED_PV_COUNT}" "${STRIPED_INITIAL_LV_MIB}" > "${MOUNT_DIR}/base-marker.txt"
(
    cd "${MOUNT_DIR}"
    md5sum "base${STRIPED_BASE_FILE_MIB}.bin" base-marker.txt > striped-integration.md5
    md5sum -c striped-integration.md5
)
sync
umount "${MOUNT_DIR}"
echo CHECK_PASS_STRIPED_LVM2_BASE_FILE_MD5

echo '=== STEP 3: grow the striped LV on the original PV set ==='
lvextend --config "${LVM_CONFIG}" -i "${STRIPED_PV_COUNT}" -I "${STRIPED_CHUNK_KIB}K" -L "${STRIPED_SAME_SET_LV_MIB}M" striped_integration_vg/striped_integration_lv "${BASE_DISKS[@]}"
lvs -a -o lv_name,lv_size,seg_count,devices striped_integration_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices striped_integration_vg/striped_integration_lv
check_striped_single_table_status_deps SAME_SET "${SAME_SET_SECTORS}" /tmp/striped-integration-same-set-table.txt /tmp/striped-integration-same-set-status.txt /tmp/striped-integration-same-set-deps.txt
echo CHECK_PASS_STRIPED_LVM2_SAME_SET_TABLE_STATUS_DEPS

echo '=== STEP 4: add a second PV set and grow into another striped segment ==='
pvcreate "${GROW_DISKS[@]}"
vgextend striped_integration_vg "${GROW_DISKS[@]}"
lvextend --config "${LVM_CONFIG}" -i "${STRIPED_PV_COUNT}" -I "${STRIPED_CHUNK_KIB}K" -L "${STRIPED_EXTENDED_LV_MIB}M" striped_integration_vg/striped_integration_lv "${GROW_DISKS[@]}"
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices striped_integration_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices striped_integration_vg/striped_integration_lv
check_striped_cross_table_status_deps EXTENDED /tmp/striped-integration-extended-table.txt /tmp/striped-integration-extended-status.txt /tmp/striped-integration-extended-deps.txt
echo CHECK_PASS_STRIPED_LVM2_EXTENDED_TABLE_STATUS_DEPS

e2fsck -f -y "${MAPPER_DEVICE}"
resize2fs "${MAPPER_DEVICE}"
e2fsck -f -y "${MAPPER_DEVICE}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c striped-integration.md5
)
dd if=/dev/urandom of="${MOUNT_DIR}/grow${STRIPED_GROW_FILE_MIB}.bin" bs=1M count="${STRIPED_GROW_FILE_MIB}" conv=fsync status=none
printf 'lvm2 striped integration cross-set grow\nsame_set_lv_mib=%s\nextended_lv_mib=%s\n' "${STRIPED_SAME_SET_LV_MIB}" "${STRIPED_EXTENDED_LV_MIB}" > "${MOUNT_DIR}/grow-marker.txt"
(
    cd "${MOUNT_DIR}"
    md5sum "grow${STRIPED_GROW_FILE_MIB}.bin" grow-marker.txt >> striped-integration.md5
    md5sum -c striped-integration.md5
)
sync
umount "${MOUNT_DIR}"
vgchange --config "${LVM_CONFIG}" -an striped_integration_vg
sync
echo CHECK_PASS_STRIPED_LVM2_GROW_FILE_MD5

echo TEST_PASS_DM_STRIPED_INTEGRATION_FIRST
poweroff
GUEST_FIRST
} >"${FIRST_GUEST_SCRIPT}"

SECOND_GUEST_SCRIPT=$(mktemp /tmp/dm-striped-integration-second.XXXXXX)
{
    printf 'STRIPED_PV_COUNT=%q\n' "${STRIPED_PV_COUNT}"
    printf 'STRIPED_TOTAL_PV_COUNT=%q\n' "${STRIPED_TOTAL_PV_COUNT}"
    printf 'STRIPED_INITIAL_LV_MIB=%q\n' "${STRIPED_INITIAL_LV_MIB}"
    printf 'STRIPED_SAME_SET_LV_MIB=%q\n' "${STRIPED_SAME_SET_LV_MIB}"
    printf 'STRIPED_EXTENDED_LV_MIB=%q\n' "${STRIPED_EXTENDED_LV_MIB}"
    printf 'STRIPED_SHRUNK_LV_MIB=%q\n' "${STRIPED_SHRUNK_LV_MIB}"
    printf 'STRIPED_BASE_FILE_MIB=%q\n' "${STRIPED_BASE_FILE_MIB}"
    printf 'STRIPED_GROW_FILE_MIB=%q\n' "${STRIPED_GROW_FILE_MIB}"
    printf 'STRIPED_CHUNK_KIB=%q\n' "${STRIPED_CHUNK_KIB}"
    cat <<'GUEST_SECOND'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_STRIPED_INTEGRATION_SECOND status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
INITIAL_SECTORS=$((STRIPED_INITIAL_LV_MIB * 2048))
SAME_SET_SECTORS=$((STRIPED_SAME_SET_LV_MIB * 2048))
EXTENDED_SECTORS=$((STRIPED_EXTENDED_LV_MIB * 2048))
SHRUNK_SECTORS=$((STRIPED_SHRUNK_LV_MIB * 2048))
CROSS_SET_SECTORS=$((EXTENDED_SECTORS - SAME_SET_SECTORS))
STRIPED_CHUNK_SECTORS=$((STRIPED_CHUNK_KIB * 2))
MAPPER_NAME=striped_integration_vg-striped_integration_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmstriped
DISKS=()
DEVS=()

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

dep_token() {
    printf '(%s, %s)' "${1%%:*}" "${1##*:}"
}

locate_striped_disks() {
    local count=$1 index serial disk label existing dev
    index=1
    while [ "${index}" -le "${count}" ]; do
        if [ "${index}" -eq 1 ]; then
            disk=$(aster-dm-disk-locator)
            label=TEST_DISK
        else
            serial=vdmtest${index}
            disk=$(aster-dm-disk-locator "${serial}")
            label=TEST_DISK${index}
        fi
        printf '%s=%s\n' "${label}" "${disk}"
        test -b "${disk}"
        for existing in "${DISKS[@]}"; do
            test "${existing}" != "${disk}"
        done
        DISKS+=("${disk}")
        dev=$(devno "${disk}")
        DEVS+=("${dev}")
        printf 'DEV%s=%s\n' "${index}" "${dev}"
        index=$((index + 1))
    done
}

check_row_devices() {
    local table_file=$1 start=$2 len=$3 devs=$4
    awk -v start="${start}" -v len="${len}" -v stripes="${STRIPED_PV_COUNT}" -v chunk="${STRIPED_CHUNK_SECTORS}" -v devs="${devs}" '
        BEGIN { n = split(devs, expected, " ") }
        $1 == start && $2 == len && $3 == "striped" && $4 == stripes && $5 == chunk {
            delete present
            for (i = 6; i <= NF; i += 2) present[$i] = 1
            ok = 1
            for (i = 1; i <= n; i++) if (!(expected[i] in present)) ok = 0
        }
        END { exit !ok }
    ' "${table_file}"
}

check_striped_cross_table() {
    local table_file=$1 deps_file=$2 status_file=$3 base_devs grow_devs dev
    base_devs="${DEVS[*]:0:${STRIPED_PV_COUNT}}"
    grow_devs="${DEVS[*]:${STRIPED_PV_COUNT}:${STRIPED_PV_COUNT}}"
    test "$(wc -l < "${table_file}")" -eq 2
    check_row_devices "${table_file}" 0 "${SAME_SET_SECTORS}" "${base_devs}"
    check_row_devices "${table_file}" "${SAME_SET_SECTORS}" "${CROSS_SET_SECTORS}" "${grow_devs}"
    awk -v len="${EXTENDED_SECTORS}" '$3 == "striped" { sum += $2 } END { exit !(sum == len) }' "${status_file}"
    grep -q "${STRIPED_TOTAL_PV_COUNT} dependencies" "${deps_file}"
    for dev in "${DEVS[@]}"; do
        grep -F -q "${dev}" "${table_file}"
        grep -F -q "$(dep_token "${dev}")" "${deps_file}"
    done
}

check_striped_shrunk_table() {
    local table_file=$1 deps_file=$2 status_file=$3 base_devs dev index
    base_devs="${DEVS[*]:0:${STRIPED_PV_COUNT}}"
    test "$(wc -l < "${table_file}")" -eq 1
    check_row_devices "${table_file}" 0 "${SHRUNK_SECTORS}" "${base_devs}"
    awk -v len="${SHRUNK_SECTORS}" '$3 == "striped" { sum += $2 } END { exit !(sum == len) }' "${status_file}"
    grep -q "${STRIPED_PV_COUNT} dependencies" "${deps_file}"
    index=0
    while [ "${index}" -lt "${STRIPED_PV_COUNT}" ]; do
        dev=${DEVS[${index}]}
        grep -F -q "${dev}" "${table_file}"
        grep -F -q "$(dep_token "${dev}")" "${deps_file}"
        index=$((index + 1))
    done
}

echo '=== STEP 1: recover the cross-set striped LV after reboot ==='
locate_striped_disks "${STRIPED_TOTAL_PV_COUNT}"
pvscan
vgscan --mknodes
vgchange --config "${LVM_CONFIG}" -ay striped_integration_vg
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices striped_integration_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices striped_integration_vg/striped_integration_lv
echo 'DM_TABLE_LVM2_STRIPED_RECOVERED_BEGIN'
dmsetup table "${MAPPER_NAME}" | tee /tmp/striped-integration-recovered-table.txt | sed 's/^/DM_TABLE_LVM2_STRIPED_RECOVERED /'
echo 'DM_TABLE_LVM2_STRIPED_RECOVERED_END'
dmsetup status "${MAPPER_NAME}" | tee /tmp/striped-integration-recovered-status.txt
dmsetup deps "${MAPPER_NAME}" | tee /tmp/striped-integration-recovered-deps.txt
check_striped_cross_table /tmp/striped-integration-recovered-table.txt /tmp/striped-integration-recovered-deps.txt /tmp/striped-integration-recovered-status.txt
echo CHECK_PASS_STRIPED_LVM2_RECOVERED_TABLE_STATUS_DEPS

echo '=== STEP 2: verify files and shrink back to one striped segment ==='
mkdir -p "${MOUNT_DIR}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c striped-integration.md5
    cat base-marker.txt
    cat grow-marker.txt
    rm "grow${STRIPED_GROW_FILE_MIB}.bin" grow-marker.txt
    md5sum "base${STRIPED_BASE_FILE_MIB}.bin" base-marker.txt > striped-integration.md5
    md5sum -c striped-integration.md5
)
umount "${MOUNT_DIR}"
e2fsck -f -y "${MAPPER_DEVICE}"
resize2fs "${MAPPER_DEVICE}" "${STRIPED_SHRUNK_LV_MIB}M"
e2fsck -f -y "${MAPPER_DEVICE}"
lvreduce --config "${LVM_CONFIG}" -y -L "${STRIPED_SHRUNK_LV_MIB}M" striped_integration_vg/striped_integration_lv
lvs -a -o lv_name,lv_size,seg_count,devices striped_integration_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices striped_integration_vg/striped_integration_lv
echo 'DM_TABLE_LVM2_STRIPED_SHRUNK_BEGIN'
dmsetup table "${MAPPER_NAME}" | tee /tmp/striped-integration-shrunk-table.txt | sed 's/^/DM_TABLE_LVM2_STRIPED_SHRUNK /'
echo 'DM_TABLE_LVM2_STRIPED_SHRUNK_END'
dmsetup status "${MAPPER_NAME}" | tee /tmp/striped-integration-shrunk-status.txt
dmsetup deps "${MAPPER_NAME}" | tee /tmp/striped-integration-shrunk-deps.txt
check_striped_shrunk_table /tmp/striped-integration-shrunk-table.txt /tmp/striped-integration-shrunk-deps.txt /tmp/striped-integration-shrunk-status.txt
echo CHECK_PASS_STRIPED_LVM2_SHRUNK_TABLE_STATUS_DEPS

vgchange --config "${LVM_CONFIG}" -an striped_integration_vg
sync
echo CHECK_PASS_STRIPED_LVM2_SHRINK_DEACTIVATED

echo TEST_PASS_DM_STRIPED_INTEGRATION_SECOND
poweroff
GUEST_SECOND
} >"${SECOND_GUEST_SCRIPT}"

THIRD_GUEST_SCRIPT=$(mktemp /tmp/dm-striped-integration-third.XXXXXX)
{
    printf 'STRIPED_PV_COUNT=%q\n' "${STRIPED_PV_COUNT}"
    printf 'STRIPED_TOTAL_PV_COUNT=%q\n' "${STRIPED_TOTAL_PV_COUNT}"
    printf 'STRIPED_INITIAL_LV_MIB=%q\n' "${STRIPED_INITIAL_LV_MIB}"
    printf 'STRIPED_SHRUNK_LV_MIB=%q\n' "${STRIPED_SHRUNK_LV_MIB}"
    printf 'STRIPED_BASE_FILE_MIB=%q\n' "${STRIPED_BASE_FILE_MIB}"
    printf 'STRIPED_GROW_FILE_MIB=%q\n' "${STRIPED_GROW_FILE_MIB}"
    printf 'STRIPED_CHUNK_KIB=%q\n' "${STRIPED_CHUNK_KIB}"
    cat <<'GUEST_THIRD'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_STRIPED_INTEGRATION_THIRD status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
SHRUNK_SECTORS=$((STRIPED_SHRUNK_LV_MIB * 2048))
STRIPED_CHUNK_SECTORS=$((STRIPED_CHUNK_KIB * 2))
MAPPER_NAME=striped_integration_vg-striped_integration_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmstriped
DISKS=()
DEVS=()

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

dep_token() {
    printf '(%s, %s)' "${1%%:*}" "${1##*:}"
}

locate_striped_disks() {
    local count=$1 index serial disk label existing dev
    index=1
    while [ "${index}" -le "${count}" ]; do
        if [ "${index}" -eq 1 ]; then
            disk=$(aster-dm-disk-locator)
            label=TEST_DISK
        else
            serial=vdmtest${index}
            disk=$(aster-dm-disk-locator "${serial}")
            label=TEST_DISK${index}
        fi
        printf '%s=%s\n' "${label}" "${disk}"
        test -b "${disk}"
        for existing in "${DISKS[@]}"; do
            test "${existing}" != "${disk}"
        done
        DISKS+=("${disk}")
        dev=$(devno "${disk}")
        DEVS+=("${dev}")
        printf 'DEV%s=%s\n' "${index}" "${dev}"
        index=$((index + 1))
    done
}

check_row_devices() {
    local table_file=$1 start=$2 len=$3 devs=$4
    awk -v start="${start}" -v len="${len}" -v stripes="${STRIPED_PV_COUNT}" -v chunk="${STRIPED_CHUNK_SECTORS}" -v devs="${devs}" '
        BEGIN { n = split(devs, expected, " ") }
        $1 == start && $2 == len && $3 == "striped" && $4 == stripes && $5 == chunk {
            delete present
            for (i = 6; i <= NF; i += 2) present[$i] = 1
            ok = 1
            for (i = 1; i <= n; i++) if (!(expected[i] in present)) ok = 0
        }
        END { exit !ok }
    ' "${table_file}"
}

check_striped_shrunk_table() {
    local table_file=$1 deps_file=$2 status_file=$3 base_devs dev index
    base_devs="${DEVS[*]:0:${STRIPED_PV_COUNT}}"
    test "$(wc -l < "${table_file}")" -eq 1
    check_row_devices "${table_file}" 0 "${SHRUNK_SECTORS}" "${base_devs}"
    awk -v len="${SHRUNK_SECTORS}" '$3 == "striped" { sum += $2 } END { exit !(sum == len) }' "${status_file}"
    grep -q "${STRIPED_PV_COUNT} dependencies" "${deps_file}"
    index=0
    while [ "${index}" -lt "${STRIPED_PV_COUNT}" ]; do
        dev=${DEVS[${index}]}
        grep -F -q "${dev}" "${table_file}"
        grep -F -q "$(dep_token "${dev}")" "${deps_file}"
        index=$((index + 1))
    done
}

echo '=== STEP 1: recover the shrunken striped LV after reboot ==='
locate_striped_disks "${STRIPED_TOTAL_PV_COUNT}"
pvscan
vgscan --mknodes
vgchange --config "${LVM_CONFIG}" -ay striped_integration_vg
lvs -a -o lv_name,lv_size,seg_count,devices striped_integration_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices striped_integration_vg/striped_integration_lv
echo 'DM_TABLE_LVM2_STRIPED_SHRUNK_RECOVERED_BEGIN'
dmsetup table "${MAPPER_NAME}" | tee /tmp/striped-integration-shrunk-recovered-table.txt | sed 's/^/DM_TABLE_LVM2_STRIPED_SHRUNK_RECOVERED /'
echo 'DM_TABLE_LVM2_STRIPED_SHRUNK_RECOVERED_END'
dmsetup status "${MAPPER_NAME}" | tee /tmp/striped-integration-shrunk-recovered-status.txt
dmsetup deps "${MAPPER_NAME}" | tee /tmp/striped-integration-shrunk-recovered-deps.txt
check_striped_shrunk_table /tmp/striped-integration-shrunk-recovered-table.txt /tmp/striped-integration-shrunk-recovered-deps.txt /tmp/striped-integration-shrunk-recovered-status.txt
echo CHECK_PASS_STRIPED_LVM2_SHRUNK_RECOVERED_TABLE_STATUS_DEPS

echo '=== STEP 2: verify shrunken ext2 data from a read-only mount ==='
mkdir -p "${MOUNT_DIR}"
mount -o ro -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c striped-integration.md5
    grep -Fx 'lvm2 striped integration base' base-marker.txt
    grep -Fx "pv_count=${STRIPED_PV_COUNT} initial_lv_mib=${STRIPED_INITIAL_LV_MIB}" base-marker.txt
    test ! -e "grow${STRIPED_GROW_FILE_MIB}.bin"
    test ! -e grow-marker.txt
)
df -h "${MOUNT_DIR}"
du -sh "${MOUNT_DIR}" "${MOUNT_DIR}/base${STRIPED_BASE_FILE_MIB}.bin"
umount "${MOUNT_DIR}"
vgchange --config "${LVM_CONFIG}" -an striped_integration_vg
sync
echo CHECK_PASS_STRIPED_LVM2_SHRINK_RECOVERY_FILE_MD5

echo TEST_PASS_DM_STRIPED_INTEGRATION_THIRD
poweroff
GUEST_THIRD
} >"${THIRD_GUEST_SCRIPT}"

SUMMARY_INCLUDE='^TEST_|^CHECK_PASS_|^=== STEP|^=== CHECK|^TEST_DISK[0-9]*=|^DEV[0-9]+=|^DM_TABLE_LVM2_STRIPED|^striped[[:space:]]|^0 [0-9]+ striped |^[0-9]+ dependencies|^Resizing the filesystem|^The filesystem on|base[0-9]+\.bin: OK|grow[0-9]+\.bin: OK|base-marker\.txt: OK|grow-marker\.txt: OK|lvm2 striped integration|pv_count=|initial_lv_mib=|same_set_lv_mib=|extended_lv_mib=|No space left|Input/output error|Command failed|Kernel panic|panicked|records in|records out|bytes .* copied'
dm_run_three_guest_test \
    "${TEST_ID}" \
    "${FIRST_GUEST_SCRIPT}" \
    "${SECOND_GUEST_SCRIPT}" \
    "${THIRD_GUEST_SCRIPT}" \
    TEST_PASS_DM_STRIPED_INTEGRATION_FIRST \
    TEST_PASS_DM_STRIPED_INTEGRATION_SECOND \
    TEST_PASS_DM_STRIPED_INTEGRATION_THIRD \
    "${SUMMARY_INCLUDE}"
