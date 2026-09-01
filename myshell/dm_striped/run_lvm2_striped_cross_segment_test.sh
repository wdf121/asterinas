#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/dm_striped/run_lvm2_striped_cross_segment_test.sh

Runs an independent two-guest NixOS regression for LVM2 striped N-to-2N cross-segment creation, reboot recovery, shrink, and file I/O.

Optional environment variables:
  DM_TEST_IMAGES                          Backing test image list. When unset, generated from STRIPED_CS_PV_COUNT * 2.
  DM_STRIPED_LVM2_CROSS_SEGMENT_LOG       Host-side log path, default /tmp/dm-striped-lvm2-cross-segment-test.log
  GUEST_READY_TIMEOUT                     Seconds to allow one full QEMU guest lifecycle, default 180
  RESET_DM_TEST_IMAGES                    1 to delete test images before running, default 1
  STRIPED_CS_PV_COUNT                     Number of PVs per striped segment, default 2
  STRIPED_CS_INITIAL_LV_MIB               Initial LV size in MiB, default STRIPED_CS_PV_COUNT * 256
  STRIPED_CS_EXTENDED_LV_MIB              Cross-segment extended LV size in MiB, default STRIPED_CS_INITIAL_LV_MIB * 2
  STRIPED_CS_SHRUNK_LV_MIB                Final shrunk LV size in MiB, default STRIPED_CS_INITIAL_LV_MIB
  STRIPED_CS_BASE_FILE_MIB                Base test file size in MiB, default STRIPED_CS_PV_COUNT * 64
  STRIPED_CS_GROW_FILE_MIB                Post-cross-segment test file size in MiB, default STRIPED_CS_PV_COUNT * 240
  STRIPED_CS_CHUNK_KIB                    LVM stripe chunk size in KiB, default 4
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/../..")
source "${SCRIPT_DIR}/../lib/dm_nixos_test.sh"

TEST_ID=DM_STRIPED_LVM2_CROSS_SEGMENT
LOG=${DM_STRIPED_LVM2_CROSS_SEGMENT_LOG:-/tmp/dm-striped-lvm2-cross-segment-test.log}
STRIPED_CS_PV_COUNT=${STRIPED_CS_PV_COUNT:-2}
STRIPED_CS_TOTAL_PV_COUNT=$((STRIPED_CS_PV_COUNT * 2))
STRIPED_CS_INITIAL_LV_MIB=${STRIPED_CS_INITIAL_LV_MIB:-$((STRIPED_CS_PV_COUNT * 256))}
STRIPED_CS_EXTENDED_LV_MIB=${STRIPED_CS_EXTENDED_LV_MIB:-$((STRIPED_CS_INITIAL_LV_MIB * 2))}
STRIPED_CS_SHRUNK_LV_MIB=${STRIPED_CS_SHRUNK_LV_MIB:-${STRIPED_CS_INITIAL_LV_MIB}}
STRIPED_CS_BASE_FILE_MIB=${STRIPED_CS_BASE_FILE_MIB:-$((STRIPED_CS_PV_COUNT * 64))}
STRIPED_CS_GROW_FILE_MIB=${STRIPED_CS_GROW_FILE_MIB:-$((STRIPED_CS_PV_COUNT * 240))}
STRIPED_CS_CHUNK_KIB=${STRIPED_CS_CHUNK_KIB:-4}
GUEST_READY_TIMEOUT=${GUEST_READY_TIMEOUT:-180}
RESET_DM_TEST_IMAGES=${RESET_DM_TEST_IMAGES:-1}

test "${STRIPED_CS_PV_COUNT}" -ge 2
test $((STRIPED_CS_INITIAL_LV_MIB % STRIPED_CS_PV_COUNT)) -eq 0
test $((STRIPED_CS_EXTENDED_LV_MIB % STRIPED_CS_PV_COUNT)) -eq 0
test $((STRIPED_CS_SHRUNK_LV_MIB % STRIPED_CS_PV_COUNT)) -eq 0
test "${STRIPED_CS_INITIAL_LV_MIB}" -lt "${STRIPED_CS_EXTENDED_LV_MIB}"
test "${STRIPED_CS_SHRUNK_LV_MIB}" -lt "${STRIPED_CS_EXTENDED_LV_MIB}"
test $((STRIPED_CS_EXTENDED_LV_MIB - STRIPED_CS_INITIAL_LV_MIB)) -eq "${STRIPED_CS_INITIAL_LV_MIB}"
test "${STRIPED_CS_BASE_FILE_MIB}" -lt "${STRIPED_CS_INITIAL_LV_MIB}"
test $((STRIPED_CS_BASE_FILE_MIB + STRIPED_CS_GROW_FILE_MIB)) -gt "${STRIPED_CS_INITIAL_LV_MIB}"
test $((STRIPED_CS_BASE_FILE_MIB + STRIPED_CS_GROW_FILE_MIB)) -lt "${STRIPED_CS_EXTENDED_LV_MIB}"
test "${STRIPED_CS_BASE_FILE_MIB}" -lt "${STRIPED_CS_SHRUNK_LV_MIB}"

if [ -z "${DM_TEST_IMAGES:-}" ]; then
    images=()
    index=1
    while [ "${index}" -le "${STRIPED_CS_TOTAL_PV_COUNT}" ]; do
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
    test "$#" -eq "${STRIPED_CS_TOTAL_PV_COUNT}"
fi
DM_TEST_IMAGE=${DM_TEST_IMAGE:-target/nixos/test.img}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-target/nixos/test2.img}

cd "${ASTERINAS_DIR}"
dm_prepare_nixos_test "${TEST_ID}"
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
echo "HOST_INFO_${TEST_ID} pv_count=${STRIPED_CS_PV_COUNT} total_pv_count=${STRIPED_CS_TOTAL_PV_COUNT} initial_lv_mib=${STRIPED_CS_INITIAL_LV_MIB} extended_lv_mib=${STRIPED_CS_EXTENDED_LV_MIB} shrunk_lv_mib=${STRIPED_CS_SHRUNK_LV_MIB} base_file_mib=${STRIPED_CS_BASE_FILE_MIB} grow_file_mib=${STRIPED_CS_GROW_FILE_MIB} chunk_kib=${STRIPED_CS_CHUNK_KIB}"

FIRST_GUEST_SCRIPT=$(mktemp /tmp/dm-striped-lvm2-cross-segment-first.XXXXXX)
{
    printf 'STRIPED_CS_PV_COUNT=%q\n' "${STRIPED_CS_PV_COUNT}"
    printf 'STRIPED_CS_TOTAL_PV_COUNT=%q\n' "${STRIPED_CS_TOTAL_PV_COUNT}"
    printf 'STRIPED_CS_INITIAL_LV_MIB=%q\n' "${STRIPED_CS_INITIAL_LV_MIB}"
    printf 'STRIPED_CS_EXTENDED_LV_MIB=%q\n' "${STRIPED_CS_EXTENDED_LV_MIB}"
    printf 'STRIPED_CS_BASE_FILE_MIB=%q\n' "${STRIPED_CS_BASE_FILE_MIB}"
    printf 'STRIPED_CS_GROW_FILE_MIB=%q\n' "${STRIPED_CS_GROW_FILE_MIB}"
    printf 'STRIPED_CS_CHUNK_KIB=%q\n' "${STRIPED_CS_CHUNK_KIB}"
    cat <<'GUEST_FIRST'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_STRIPED_LVM2_CROSS_SEGMENT_FIRST status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
INITIAL_SECTORS=$((STRIPED_CS_INITIAL_LV_MIB * 2048))
EXTENDED_SECTORS=$((STRIPED_CS_EXTENDED_LV_MIB * 2048))
GROW_SECTORS=$((EXTENDED_SECTORS - INITIAL_SECTORS))
STRIPED_CHUNK_SECTORS=$((STRIPED_CS_CHUNK_KIB * 2))
MAPPER_NAME=striped_cs_vg-striped_cs_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmstripedcs
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
        if [ "${index}" -le "${STRIPED_CS_PV_COUNT}" ]; then
            BASE_DISKS+=("${disk}")
        else
            GROW_DISKS+=("${disk}")
        fi
        index=$((index + 1))
    done
}

check_row_devices() {
    local table_file=$1 start=$2 len=$3 devs=$4
    awk -v start="${start}" -v len="${len}" -v stripes="${STRIPED_CS_PV_COUNT}" -v chunk="${STRIPED_CHUNK_SECTORS}" -v devs="${devs}" '
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
    base_devs="${DEVS[*]:0:${STRIPED_CS_PV_COUNT}}"
    grow_devs="${DEVS[*]:${STRIPED_CS_PV_COUNT}:${STRIPED_CS_PV_COUNT}}"
    test "$(wc -l < "${table_file}")" -eq 2
    check_row_devices "${table_file}" 0 "${INITIAL_SECTORS}" "${base_devs}"
    check_row_devices "${table_file}" "${INITIAL_SECTORS}" "${GROW_SECTORS}" "${grow_devs}"
    awk -v len="${EXTENDED_SECTORS}" '$3 == "striped" { sum += $2 } END { exit !(sum == len) }' "${status_file}"
    grep -q "${STRIPED_CS_TOTAL_PV_COUNT} dependencies" "${deps_file}"
    for dev in "${DEVS[@]}"; do
        grep -F -q "${dev}" "${table_file}"
        grep -F -q "$(dep_token "${dev}")" "${deps_file}"
    done
}

check_striped_cross_table_status_deps() {
    local label=$1 table_file=$2 status_file=$3 deps_file=$4
    echo "DM_TABLE_LVM2_STRIPED_CS_${label}_BEGIN"
    dmsetup table "${MAPPER_NAME}" | tee "${table_file}" | sed "s/^/DM_TABLE_LVM2_STRIPED_CS_${label} /"
    echo "DM_TABLE_LVM2_STRIPED_CS_${label}_END"
    dmsetup status "${MAPPER_NAME}" | tee "${status_file}"
    dmsetup deps "${MAPPER_NAME}" | tee "${deps_file}"
    check_striped_cross_table "${table_file}" "${deps_file}" "${status_file}"
}

echo '=== STEP 1: check dm control device, striped target, and 2N test disks ==='
test -c /dev/mapper/control
dmsetup targets | tee /tmp/striped-cs-targets.txt
grep -q '^striped' /tmp/striped-cs-targets.txt
locate_striped_disks "${STRIPED_CS_TOTAL_PV_COUNT}"
echo CHECK_PASS_STRIPED_CS_LVM2_SETUP

echo '=== STEP 2: create N-PV striped LV and base data ==='
pvcreate "${BASE_DISKS[@]}"
vgcreate striped_cs_vg "${BASE_DISKS[@]}"
lvcreate --config "${LVM_CONFIG}" --type striped -i "${STRIPED_CS_PV_COUNT}" -I "${STRIPED_CS_CHUNK_KIB}K" -L "${STRIPED_CS_INITIAL_LV_MIB}M" -n striped_cs_lv striped_cs_vg "${BASE_DISKS[@]}"
mkfs.ext2 -F -b 4096 "${MAPPER_DEVICE}"
mkdir -p "${MOUNT_DIR}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
dd if=/dev/urandom of="${MOUNT_DIR}/base${STRIPED_CS_BASE_FILE_MIB}.bin" bs=1M count="${STRIPED_CS_BASE_FILE_MIB}" conv=fsync status=none
printf 'lvm2 striped cross-segment base\npv_count=%s initial_lv_mib=%s\n' "${STRIPED_CS_PV_COUNT}" "${STRIPED_CS_INITIAL_LV_MIB}" > "${MOUNT_DIR}/base-marker.txt"
(
    cd "${MOUNT_DIR}"
    md5sum "base${STRIPED_CS_BASE_FILE_MIB}.bin" base-marker.txt > striped-cs.md5
    md5sum -c striped-cs.md5
)
sync
umount "${MOUNT_DIR}"
echo CHECK_PASS_STRIPED_CS_LVM2_BASE_FILE_MD5

echo '=== STEP 3: add N PVs and extend into a second striped segment ==='
pvcreate "${GROW_DISKS[@]}"
vgextend striped_cs_vg "${GROW_DISKS[@]}"
lvextend --config "${LVM_CONFIG}" -i "${STRIPED_CS_PV_COUNT}" -I "${STRIPED_CS_CHUNK_KIB}K" -L "${STRIPED_CS_EXTENDED_LV_MIB}M" striped_cs_vg/striped_cs_lv "${GROW_DISKS[@]}"
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices striped_cs_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices striped_cs_vg/striped_cs_lv
check_striped_cross_table_status_deps EXTENDED /tmp/striped-cs-extended-table.txt /tmp/striped-cs-extended-status.txt /tmp/striped-cs-extended-deps.txt
echo CHECK_PASS_STRIPED_CS_LVM2_EXTENDED_TABLE_STATUS_DEPS

e2fsck -f -y "${MAPPER_DEVICE}"
resize2fs "${MAPPER_DEVICE}"
e2fsck -f -y "${MAPPER_DEVICE}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c striped-cs.md5
)
dd if=/dev/urandom of="${MOUNT_DIR}/grow${STRIPED_CS_GROW_FILE_MIB}.bin" bs=1M count="${STRIPED_CS_GROW_FILE_MIB}" conv=fsync status=none
printf 'lvm2 striped cross-segment grow\nextended_lv_mib=%s\n' "${STRIPED_CS_EXTENDED_LV_MIB}" > "${MOUNT_DIR}/grow-marker.txt"
(
    cd "${MOUNT_DIR}"
    md5sum "grow${STRIPED_CS_GROW_FILE_MIB}.bin" grow-marker.txt >> striped-cs.md5
    md5sum -c striped-cs.md5
)
sync
umount "${MOUNT_DIR}"
vgchange --config "${LVM_CONFIG}" -an striped_cs_vg
sync
echo CHECK_PASS_STRIPED_CS_LVM2_GROW_FILE_MD5

echo TEST_PASS_DM_STRIPED_LVM2_CROSS_SEGMENT_FIRST
poweroff
GUEST_FIRST
} >"${FIRST_GUEST_SCRIPT}"

SECOND_GUEST_SCRIPT=$(mktemp /tmp/dm-striped-lvm2-cross-segment-second.XXXXXX)
{
    printf 'STRIPED_CS_PV_COUNT=%q\n' "${STRIPED_CS_PV_COUNT}"
    printf 'STRIPED_CS_TOTAL_PV_COUNT=%q\n' "${STRIPED_CS_TOTAL_PV_COUNT}"
    printf 'STRIPED_CS_INITIAL_LV_MIB=%q\n' "${STRIPED_CS_INITIAL_LV_MIB}"
    printf 'STRIPED_CS_EXTENDED_LV_MIB=%q\n' "${STRIPED_CS_EXTENDED_LV_MIB}"
    printf 'STRIPED_CS_SHRUNK_LV_MIB=%q\n' "${STRIPED_CS_SHRUNK_LV_MIB}"
    printf 'STRIPED_CS_BASE_FILE_MIB=%q\n' "${STRIPED_CS_BASE_FILE_MIB}"
    printf 'STRIPED_CS_GROW_FILE_MIB=%q\n' "${STRIPED_CS_GROW_FILE_MIB}"
    printf 'STRIPED_CS_CHUNK_KIB=%q\n' "${STRIPED_CS_CHUNK_KIB}"
    cat <<'GUEST_SECOND'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_STRIPED_LVM2_CROSS_SEGMENT_SECOND status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
INITIAL_SECTORS=$((STRIPED_CS_INITIAL_LV_MIB * 2048))
EXTENDED_SECTORS=$((STRIPED_CS_EXTENDED_LV_MIB * 2048))
SHRUNK_SECTORS=$((STRIPED_CS_SHRUNK_LV_MIB * 2048))
GROW_SECTORS=$((EXTENDED_SECTORS - INITIAL_SECTORS))
STRIPED_CHUNK_SECTORS=$((STRIPED_CS_CHUNK_KIB * 2))
MAPPER_NAME=striped_cs_vg-striped_cs_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmstripedcs
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
    awk -v start="${start}" -v len="${len}" -v stripes="${STRIPED_CS_PV_COUNT}" -v chunk="${STRIPED_CHUNK_SECTORS}" -v devs="${devs}" '
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
    base_devs="${DEVS[*]:0:${STRIPED_CS_PV_COUNT}}"
    grow_devs="${DEVS[*]:${STRIPED_CS_PV_COUNT}:${STRIPED_CS_PV_COUNT}}"
    test "$(wc -l < "${table_file}")" -eq 2
    check_row_devices "${table_file}" 0 "${INITIAL_SECTORS}" "${base_devs}"
    check_row_devices "${table_file}" "${INITIAL_SECTORS}" "${GROW_SECTORS}" "${grow_devs}"
    awk -v len="${EXTENDED_SECTORS}" '$3 == "striped" { sum += $2 } END { exit !(sum == len) }' "${status_file}"
    grep -q "${STRIPED_CS_TOTAL_PV_COUNT} dependencies" "${deps_file}"
    for dev in "${DEVS[@]}"; do
        grep -F -q "${dev}" "${table_file}"
        grep -F -q "$(dep_token "${dev}")" "${deps_file}"
    done
}

check_striped_shrunk_table() {
    local table_file=$1 deps_file=$2 status_file=$3 base_devs dev index
    base_devs="${DEVS[*]:0:${STRIPED_CS_PV_COUNT}}"
    test "$(wc -l < "${table_file}")" -eq 1
    check_row_devices "${table_file}" 0 "${SHRUNK_SECTORS}" "${base_devs}"
    awk -v len="${SHRUNK_SECTORS}" '$3 == "striped" { sum += $2 } END { exit !(sum == len) }' "${status_file}"
    grep -q "${STRIPED_CS_PV_COUNT} dependencies" "${deps_file}"
    index=0
    while [ "${index}" -lt "${STRIPED_CS_PV_COUNT}" ]; do
        dev=${DEVS[${index}]}
        grep -F -q "${dev}" "${table_file}"
        grep -F -q "$(dep_token "${dev}")" "${deps_file}"
        index=$((index + 1))
    done
}

echo '=== STEP 1: recover cross-segment striped LV after reboot ==='
locate_striped_disks "${STRIPED_CS_TOTAL_PV_COUNT}"
pvscan
vgscan --mknodes
vgchange --config "${LVM_CONFIG}" -ay striped_cs_vg
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices striped_cs_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices striped_cs_vg/striped_cs_lv
echo 'DM_TABLE_LVM2_STRIPED_CS_RECOVERED_BEGIN'
dmsetup table "${MAPPER_NAME}" | tee /tmp/striped-cs-recovered-table.txt | sed 's/^/DM_TABLE_LVM2_STRIPED_CS_RECOVERED /'
echo 'DM_TABLE_LVM2_STRIPED_CS_RECOVERED_END'
dmsetup status "${MAPPER_NAME}" | tee /tmp/striped-cs-recovered-status.txt
dmsetup deps "${MAPPER_NAME}" | tee /tmp/striped-cs-recovered-deps.txt
check_striped_cross_table /tmp/striped-cs-recovered-table.txt /tmp/striped-cs-recovered-deps.txt /tmp/striped-cs-recovered-status.txt
echo CHECK_PASS_STRIPED_CS_LVM2_RECOVERED_TABLE_STATUS_DEPS

echo '=== STEP 2: verify files and shrink back to one striped segment ==='
mkdir -p "${MOUNT_DIR}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c striped-cs.md5
    cat base-marker.txt
    cat grow-marker.txt
    rm "grow${STRIPED_CS_GROW_FILE_MIB}.bin" grow-marker.txt
    md5sum "base${STRIPED_CS_BASE_FILE_MIB}.bin" base-marker.txt > striped-cs.md5
    md5sum -c striped-cs.md5
)
umount "${MOUNT_DIR}"
e2fsck -f -y "${MAPPER_DEVICE}"
resize2fs "${MAPPER_DEVICE}" "${STRIPED_CS_SHRUNK_LV_MIB}M"
e2fsck -f -y "${MAPPER_DEVICE}"
lvreduce --config "${LVM_CONFIG}" -y -L "${STRIPED_CS_SHRUNK_LV_MIB}M" striped_cs_vg/striped_cs_lv
lvs -a -o lv_name,lv_size,seg_count,devices striped_cs_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices striped_cs_vg/striped_cs_lv
echo 'DM_TABLE_LVM2_STRIPED_CS_SHRUNK_BEGIN'
dmsetup table "${MAPPER_NAME}" | tee /tmp/striped-cs-shrunk-table.txt | sed 's/^/DM_TABLE_LVM2_STRIPED_CS_SHRUNK /'
echo 'DM_TABLE_LVM2_STRIPED_CS_SHRUNK_END'
dmsetup status "${MAPPER_NAME}" | tee /tmp/striped-cs-shrunk-status.txt
dmsetup deps "${MAPPER_NAME}" | tee /tmp/striped-cs-shrunk-deps.txt
check_striped_shrunk_table /tmp/striped-cs-shrunk-table.txt /tmp/striped-cs-shrunk-deps.txt /tmp/striped-cs-shrunk-status.txt
echo CHECK_PASS_STRIPED_CS_LVM2_SHRUNK_TABLE_STATUS_DEPS

mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c striped-cs.md5
)
df -h "${MOUNT_DIR}"
du -sh "${MOUNT_DIR}" "${MOUNT_DIR}/base${STRIPED_CS_BASE_FILE_MIB}.bin"
umount "${MOUNT_DIR}"
vgchange --config "${LVM_CONFIG}" -an striped_cs_vg
sync
echo CHECK_PASS_STRIPED_CS_LVM2_SHRINK_FILE_MD5

echo TEST_PASS_DM_STRIPED_LVM2_CROSS_SEGMENT_SECOND
poweroff
GUEST_SECOND
} >"${SECOND_GUEST_SCRIPT}"

SUMMARY_INCLUDE='^TEST_|^CHECK_PASS_|^=== STEP|^=== CHECK|^TEST_DISK[0-9]*=|^DEV[0-9]+=|^DM_TABLE_LVM2_STRIPED_CS|^striped[[:space:]]|^0 [0-9]+ striped |^[0-9]+ dependencies|^Resizing the filesystem|^The filesystem on|base[0-9]+\.bin: OK|grow[0-9]+\.bin: OK|base-marker\.txt: OK|grow-marker\.txt: OK|lvm2 striped cross-segment|pv_count=|initial_lv_mib=|extended_lv_mib=|No space left|Input/output error|Command failed|Kernel panic|panicked|records in|records out|bytes .* copied'
dm_run_two_guest_test \
    "${TEST_ID}" \
    "${FIRST_GUEST_SCRIPT}" \
    "${SECOND_GUEST_SCRIPT}" \
    TEST_PASS_DM_STRIPED_LVM2_CROSS_SEGMENT_FIRST \
    TEST_PASS_DM_STRIPED_LVM2_CROSS_SEGMENT_SECOND \
    "${SUMMARY_INCLUDE}"
