#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/dm_striped/run_lvm2_striped_reboot_test.sh

Runs a two-guest NixOS regression for N-PV LVM2 striped create, same-PV-set grow/shrink, ext2 file I/O, and reboot recovery.

Optional environment variables:
  DM_TEST_IMAGES                    Backing test image list. When unset, generated from STRIPED_PV_COUNT.
  DM_STRIPED_LVM2_REBOOT_LOG        Host-side log path, default /tmp/dm-striped-lvm2-reboot-test.log
  GUEST_READY_TIMEOUT               Seconds to wait for guest root shell, default 240
  RESET_DM_TEST_IMAGES              1 to delete test images before running, default 1
  STRIPED_PV_COUNT                  Number of striped PVs, default 2
  STRIPED_INITIAL_LV_MIB            Initial LV size in MiB, default STRIPED_PV_COUNT * 256
  STRIPED_EXTENDED_LV_MIB           Same-PV-set extended LV size in MiB, default STRIPED_PV_COUNT * 384
  STRIPED_SHRUNK_LV_MIB             Final shrunk LV size in MiB, default STRIPED_INITIAL_LV_MIB
  STRIPED_BASE_FILE_MIB             Base test file size in MiB, default STRIPED_PV_COUNT * 64
  STRIPED_GROW_FILE_MIB             Transient post-grow test file size in MiB, default STRIPED_PV_COUNT * 240
  STRIPED_AFTER_SHRINK_FILE_MIB     Post-shrink test file size in MiB, default STRIPED_PV_COUNT * 64
  STRIPED_CHUNK_KIB                 LVM stripe chunk size in KiB, default 4
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/../..")
source "${SCRIPT_DIR}/../lib/dm_nixos_test.sh"

TEST_ID=DM_STRIPED_LVM2_REBOOT
LOG=${DM_STRIPED_LVM2_REBOOT_LOG:-/tmp/dm-striped-lvm2-reboot-test.log}
STRIPED_PV_COUNT=${STRIPED_PV_COUNT:-2}
STRIPED_INITIAL_LV_MIB=${STRIPED_INITIAL_LV_MIB:-$((STRIPED_PV_COUNT * 256))}
STRIPED_EXTENDED_LV_MIB=${STRIPED_EXTENDED_LV_MIB:-$((STRIPED_PV_COUNT * 384))}
STRIPED_SHRUNK_LV_MIB=${STRIPED_SHRUNK_LV_MIB:-${STRIPED_INITIAL_LV_MIB}}
STRIPED_BASE_FILE_MIB=${STRIPED_BASE_FILE_MIB:-$((STRIPED_PV_COUNT * 64))}
STRIPED_GROW_FILE_MIB=${STRIPED_GROW_FILE_MIB:-$((STRIPED_PV_COUNT * 240))}
STRIPED_AFTER_SHRINK_FILE_MIB=${STRIPED_AFTER_SHRINK_FILE_MIB:-$((STRIPED_PV_COUNT * 64))}
STRIPED_CHUNK_KIB=${STRIPED_CHUNK_KIB:-4}
GUEST_READY_TIMEOUT=${GUEST_READY_TIMEOUT:-240}
RESET_DM_TEST_IMAGES=${RESET_DM_TEST_IMAGES:-1}

test "${STRIPED_PV_COUNT}" -ge 2
test $((STRIPED_INITIAL_LV_MIB % STRIPED_PV_COUNT)) -eq 0
test $((STRIPED_EXTENDED_LV_MIB % STRIPED_PV_COUNT)) -eq 0
test $((STRIPED_SHRUNK_LV_MIB % STRIPED_PV_COUNT)) -eq 0
test "${STRIPED_INITIAL_LV_MIB}" -lt "${STRIPED_EXTENDED_LV_MIB}"
test "${STRIPED_SHRUNK_LV_MIB}" -lt "${STRIPED_EXTENDED_LV_MIB}"
test "${STRIPED_BASE_FILE_MIB}" -lt "${STRIPED_INITIAL_LV_MIB}"
test $((STRIPED_BASE_FILE_MIB + STRIPED_AFTER_SHRINK_FILE_MIB)) -lt "${STRIPED_SHRUNK_LV_MIB}"
test $((STRIPED_BASE_FILE_MIB + STRIPED_GROW_FILE_MIB)) -lt "${STRIPED_EXTENDED_LV_MIB}"

if [ -z "${DM_TEST_IMAGES:-}" ]; then
    images=()
    index=1
    while [ "${index}" -le "${STRIPED_PV_COUNT}" ]; do
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
    test "$#" -eq "${STRIPED_PV_COUNT}"
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
echo "HOST_INFO_${TEST_ID} pv_count=${STRIPED_PV_COUNT} initial_lv_mib=${STRIPED_INITIAL_LV_MIB} extended_lv_mib=${STRIPED_EXTENDED_LV_MIB} shrunk_lv_mib=${STRIPED_SHRUNK_LV_MIB} base_file_mib=${STRIPED_BASE_FILE_MIB} grow_file_mib=${STRIPED_GROW_FILE_MIB} after_shrink_file_mib=${STRIPED_AFTER_SHRINK_FILE_MIB} chunk_kib=${STRIPED_CHUNK_KIB}"

FIRST_GUEST_SCRIPT=$(mktemp /tmp/dm-striped-lvm2-basic-first.XXXXXX)
{
    printf 'STRIPED_PV_COUNT=%q\n' "${STRIPED_PV_COUNT}"
    printf 'STRIPED_INITIAL_LV_MIB=%q\n' "${STRIPED_INITIAL_LV_MIB}"
    printf 'STRIPED_EXTENDED_LV_MIB=%q\n' "${STRIPED_EXTENDED_LV_MIB}"
    printf 'STRIPED_SHRUNK_LV_MIB=%q\n' "${STRIPED_SHRUNK_LV_MIB}"
    printf 'STRIPED_BASE_FILE_MIB=%q\n' "${STRIPED_BASE_FILE_MIB}"
    printf 'STRIPED_GROW_FILE_MIB=%q\n' "${STRIPED_GROW_FILE_MIB}"
    printf 'STRIPED_AFTER_SHRINK_FILE_MIB=%q\n' "${STRIPED_AFTER_SHRINK_FILE_MIB}"
    printf 'STRIPED_CHUNK_KIB=%q\n' "${STRIPED_CHUNK_KIB}"
    cat <<'GUEST_FIRST'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_STRIPED_LVM2_REBOOT_FIRST status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
INITIAL_SECTORS=$((STRIPED_INITIAL_LV_MIB * 2048))
EXTENDED_SECTORS=$((STRIPED_EXTENDED_LV_MIB * 2048))
SHRUNK_SECTORS=$((STRIPED_SHRUNK_LV_MIB * 2048))
STRIPED_CHUNK_SECTORS=$((STRIPED_CHUNK_KIB * 2))
MAPPER_NAME=striped_base_vg-striped_base_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmstripedbase
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

check_striped_table() {
    local table_file=$1 deps_file=$2 status_file=$3 expected_sectors=$4 dev
    test "$(wc -l < "${table_file}")" -eq 1
    awk -v len="${expected_sectors}" -v stripes="${STRIPED_PV_COUNT}" -v chunk="${STRIPED_CHUNK_SECTORS}" '
        $1 == 0 && $2 == len && $3 == "striped" && $4 == stripes && $5 == chunk { ok = 1 }
        END { exit !ok }
    ' "${table_file}"
    awk -v len="${expected_sectors}" '$1 == 0 && $2 == len && $3 == "striped" { ok = 1 } END { exit !ok }' "${status_file}"
    grep -q "${STRIPED_PV_COUNT} dependencies" "${deps_file}"
    for dev in "${DEVS[@]}"; do
        grep -F -q "${dev}" "${table_file}"
        grep -F -q "$(dep_token "${dev}")" "${deps_file}"
    done
}

check_striped_table_status_deps() {
    local label=$1 expected_sectors=$2 table_file=$3 status_file=$4 deps_file=$5
    echo "DM_TABLE_LVM2_STRIPED_BASE_${label}_BEGIN"
    dmsetup table "${MAPPER_NAME}" | tee "${table_file}" | sed "s/^/DM_TABLE_LVM2_STRIPED_BASE_${label} /"
    echo "DM_TABLE_LVM2_STRIPED_BASE_${label}_END"
    dmsetup status "${MAPPER_NAME}" | tee "${status_file}"
    dmsetup deps "${MAPPER_NAME}" | tee "${deps_file}"
    check_striped_table "${table_file}" "${deps_file}" "${status_file}" "${expected_sectors}"
}

echo '=== STEP 1: check dm control device, striped target, and base test disks ==='
test -c /dev/mapper/control
dmsetup targets | tee /tmp/striped-lvm2-targets.txt
grep -q '^striped' /tmp/striped-lvm2-targets.txt
locate_striped_disks "${STRIPED_PV_COUNT}"
echo CHECK_PASS_STRIPED_BASE_LVM2_SETUP

echo '=== STEP 2: create N-PV striped LV ==='
pvcreate "${DISKS[@]}"
vgcreate striped_base_vg "${DISKS[@]}"
lvcreate --config "${LVM_CONFIG}" --type striped -i "${STRIPED_PV_COUNT}" -I "${STRIPED_CHUNK_KIB}K" -L "${STRIPED_INITIAL_LV_MIB}M" -n striped_base_lv striped_base_vg "${DISKS[@]}"
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices striped_base_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices striped_base_vg/striped_base_lv
check_striped_table_status_deps INITIAL "${INITIAL_SECTORS}" /tmp/striped-lvm2-base-initial-table.txt /tmp/striped-lvm2-base-initial-status.txt /tmp/striped-lvm2-base-initial-deps.txt
echo CHECK_PASS_STRIPED_BASE_LVM2_INITIAL_TABLE_STATUS_DEPS

echo '=== STEP 3: create ext2 filesystem and write base data ==='
mkfs.ext2 -F -b 4096 "${MAPPER_DEVICE}"
blkid "${MAPPER_DEVICE}"
mkdir -p "${MOUNT_DIR}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
dd if=/dev/urandom of="${MOUNT_DIR}/base${STRIPED_BASE_FILE_MIB}.bin" bs=1M count="${STRIPED_BASE_FILE_MIB}" conv=fsync status=none
printf 'lvm2 striped base\npv_count=%s initial_lv_mib=%s chunk_kib=%s\n' "${STRIPED_PV_COUNT}" "${STRIPED_INITIAL_LV_MIB}" "${STRIPED_CHUNK_KIB}" > "${MOUNT_DIR}/base-marker.txt"
(
    cd "${MOUNT_DIR}"
    md5sum "base${STRIPED_BASE_FILE_MIB}.bin" base-marker.txt > striped.md5
    md5sum -c striped.md5
)
sync
umount "${MOUNT_DIR}"
echo CHECK_PASS_STRIPED_LVM2_BASE_FILE_MD5

echo '=== STEP 4: grow striped LV on the same PV set ==='
lvextend --config "${LVM_CONFIG}" -i "${STRIPED_PV_COUNT}" -I "${STRIPED_CHUNK_KIB}K" -L "${STRIPED_EXTENDED_LV_MIB}M" striped_base_vg/striped_base_lv "${DISKS[@]}"
lvs -a -o lv_name,lv_size,seg_count,devices striped_base_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices striped_base_vg/striped_base_lv
check_striped_table_status_deps EXTENDED "${EXTENDED_SECTORS}" /tmp/striped-lvm2-base-extended-table.txt /tmp/striped-lvm2-base-extended-status.txt /tmp/striped-lvm2-base-extended-deps.txt
echo CHECK_PASS_STRIPED_LVM2_EXTENDED_TABLE_STATUS_DEPS

e2fsck -f -y "${MAPPER_DEVICE}"
resize2fs "${MAPPER_DEVICE}"
e2fsck -f -y "${MAPPER_DEVICE}"
mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c striped.md5
)
dd if=/dev/urandom of="${MOUNT_DIR}/grow${STRIPED_GROW_FILE_MIB}.bin" bs=1M count="${STRIPED_GROW_FILE_MIB}" conv=fsync status=none
printf 'lvm2 striped after grow\nextended_lv_mib=%s\n' "${STRIPED_EXTENDED_LV_MIB}" > "${MOUNT_DIR}/grow-marker.txt"
(
    cd "${MOUNT_DIR}"
    md5sum "grow${STRIPED_GROW_FILE_MIB}.bin" grow-marker.txt > grow.md5
    md5sum -c grow.md5
    rm "grow${STRIPED_GROW_FILE_MIB}.bin" grow-marker.txt grow.md5
    md5sum -c striped.md5
)
sync
umount "${MOUNT_DIR}"
echo CHECK_PASS_STRIPED_LVM2_GROW_FILE_MD5

echo '=== STEP 5: shrink ext2 and striped LV on the same PV set ==='
e2fsck -f -y "${MAPPER_DEVICE}"
resize2fs "${MAPPER_DEVICE}" "${STRIPED_SHRUNK_LV_MIB}M"
e2fsck -f -y "${MAPPER_DEVICE}"
lvreduce --config "${LVM_CONFIG}" -y -L "${STRIPED_SHRUNK_LV_MIB}M" striped_base_vg/striped_base_lv
lvs -a -o lv_name,lv_size,seg_count,devices striped_base_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices striped_base_vg/striped_base_lv
check_striped_table_status_deps SHRUNK "${SHRUNK_SECTORS}" /tmp/striped-lvm2-base-shrunk-table.txt /tmp/striped-lvm2-base-shrunk-status.txt /tmp/striped-lvm2-base-shrunk-deps.txt
echo CHECK_PASS_STRIPED_LVM2_SHRUNK_TABLE_STATUS_DEPS

mount -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c striped.md5
)
dd if=/dev/urandom of="${MOUNT_DIR}/after-shrink${STRIPED_AFTER_SHRINK_FILE_MIB}.bin" bs=1M count="${STRIPED_AFTER_SHRINK_FILE_MIB}" conv=fsync status=none
printf 'lvm2 striped after shrink\nshrunk_lv_mib=%s\n' "${STRIPED_SHRUNK_LV_MIB}" > "${MOUNT_DIR}/shrink-marker.txt"
(
    cd "${MOUNT_DIR}"
    md5sum "after-shrink${STRIPED_AFTER_SHRINK_FILE_MIB}.bin" shrink-marker.txt >> striped.md5
    md5sum -c striped.md5
)
sync
umount "${MOUNT_DIR}"
vgchange --config "${LVM_CONFIG}" -an striped_base_vg
sync
echo CHECK_PASS_STRIPED_LVM2_SHRINK_FILE_MD5

echo TEST_PASS_DM_STRIPED_LVM2_REBOOT_FIRST
poweroff
GUEST_FIRST
} >"${FIRST_GUEST_SCRIPT}"

SECOND_GUEST_SCRIPT=$(mktemp /tmp/dm-striped-lvm2-basic-second.XXXXXX)
{
    printf 'STRIPED_PV_COUNT=%q\n' "${STRIPED_PV_COUNT}"
    printf 'STRIPED_SHRUNK_LV_MIB=%q\n' "${STRIPED_SHRUNK_LV_MIB}"
    printf 'STRIPED_BASE_FILE_MIB=%q\n' "${STRIPED_BASE_FILE_MIB}"
    printf 'STRIPED_AFTER_SHRINK_FILE_MIB=%q\n' "${STRIPED_AFTER_SHRINK_FILE_MIB}"
    printf 'STRIPED_CHUNK_KIB=%q\n' "${STRIPED_CHUNK_KIB}"
    cat <<'GUEST_SECOND'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_STRIPED_LVM2_REBOOT_SECOND status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
SHRUNK_SECTORS=$((STRIPED_SHRUNK_LV_MIB * 2048))
STRIPED_CHUNK_SECTORS=$((STRIPED_CHUNK_KIB * 2))
MAPPER_NAME=striped_base_vg-striped_base_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmstripedbase
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

check_striped_table() {
    local table_file=$1 deps_file=$2 status_file=$3 dev
    test "$(wc -l < "${table_file}")" -eq 1
    awk -v len="${SHRUNK_SECTORS}" -v stripes="${STRIPED_PV_COUNT}" -v chunk="${STRIPED_CHUNK_SECTORS}" '
        $1 == 0 && $2 == len && $3 == "striped" && $4 == stripes && $5 == chunk { ok = 1 }
        END { exit !ok }
    ' "${table_file}"
    awk -v len="${SHRUNK_SECTORS}" '$1 == 0 && $2 == len && $3 == "striped" { ok = 1 } END { exit !ok }' "${status_file}"
    grep -q "${STRIPED_PV_COUNT} dependencies" "${deps_file}"
    for dev in "${DEVS[@]}"; do
        grep -F -q "${dev}" "${table_file}"
        grep -F -q "$(dep_token "${dev}")" "${deps_file}"
    done
}

check_striped_table_status_deps() {
    local label=$1 table_file=$2 status_file=$3 deps_file=$4
    echo "DM_TABLE_LVM2_STRIPED_BASE_${label}_BEGIN"
    dmsetup table "${MAPPER_NAME}" | tee "${table_file}" | sed "s/^/DM_TABLE_LVM2_STRIPED_BASE_${label} /"
    echo "DM_TABLE_LVM2_STRIPED_BASE_${label}_END"
    dmsetup status "${MAPPER_NAME}" | tee "${status_file}"
    dmsetup deps "${MAPPER_NAME}" | tee "${deps_file}"
    check_striped_table "${table_file}" "${deps_file}" "${status_file}"
}

echo '=== STEP 1: recover shrunk N-PV striped LV after reboot ==='
locate_striped_disks "${STRIPED_PV_COUNT}"
pvscan
vgscan --mknodes
vgchange --config "${LVM_CONFIG}" -ay striped_base_vg
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices striped_base_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices striped_base_vg/striped_base_lv
check_striped_table_status_deps RECOVERED /tmp/striped-lvm2-base-recovered-table.txt /tmp/striped-lvm2-base-recovered-status.txt /tmp/striped-lvm2-base-recovered-deps.txt
echo CHECK_PASS_STRIPED_BASE_LVM2_RECOVERED_TABLE_STATUS_DEPS

echo '=== STEP 2: readonly mount and verify md5 after reboot ==='
mkdir -p "${MOUNT_DIR}"
mount -o ro -t ext2 "${MAPPER_DEVICE}" "${MOUNT_DIR}"
(
    cd "${MOUNT_DIR}"
    md5sum -c striped.md5
    cat base-marker.txt
    cat shrink-marker.txt
)
df -h "${MOUNT_DIR}"
du -sh "${MOUNT_DIR}" "${MOUNT_DIR}/base${STRIPED_BASE_FILE_MIB}.bin" "${MOUNT_DIR}/after-shrink${STRIPED_AFTER_SHRINK_FILE_MIB}.bin"
umount "${MOUNT_DIR}"
vgchange --config "${LVM_CONFIG}" -an striped_base_vg
sync
echo CHECK_PASS_STRIPED_BASE_LVM2_RECOVERED_FILE_MD5

echo TEST_PASS_DM_STRIPED_LVM2_REBOOT_SECOND
poweroff
GUEST_SECOND
} >"${SECOND_GUEST_SCRIPT}"

SUMMARY_INCLUDE='^TEST_|^CHECK_PASS_|^=== STEP|^=== CHECK|^TEST_DISK[0-9]*=|^DEV[0-9]+=|^DM_TABLE_LVM2_STRIPED_BASE|^striped[[:space:]]|^0 [0-9]+ striped |^[0-9]+ dependencies|^Resizing the filesystem|^The filesystem on|base[0-9]+\.bin: OK|grow[0-9]+\.bin: OK|after-shrink[0-9]+\.bin: OK|base-marker\.txt: OK|shrink-marker\.txt: OK|lvm2 striped|pv_count=|initial_lv_mib=|extended_lv_mib=|shrunk_lv_mib=|No space left|Input/output error|Command failed|Kernel panic|panicked|records in|records out|bytes .* copied'
dm_run_two_guest_test \
    "${TEST_ID}" \
    "${FIRST_GUEST_SCRIPT}" \
    "${SECOND_GUEST_SCRIPT}" \
    TEST_PASS_DM_STRIPED_LVM2_REBOOT_FIRST \
    TEST_PASS_DM_STRIPED_LVM2_REBOOT_SECOND \
    "${SUMMARY_INCLUDE}"
