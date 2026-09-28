#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/dm_mixed/run_lvm2_mixed_integration_test.sh

Runs a two-guest NixOS integration test for an LVM2 LV whose Device Mapper table
contains a linear segment followed by a striped segment. It validates table
shape, ext2 I/O across the segment boundary, deactivation, and reboot recovery.

Optional environment variables:
  DM_TEST_IMAGES                         Backing test image list, default "target/nixos/test.img target/nixos/test2.img target/nixos/test3.img"
  DM_MIXED_INTEGRATION_LOG               Host-side log path, default /tmp/dm-mixed-integration-test.log
  GUEST_QEMU_TIMEOUT                     Full QEMU lifecycle timeout in seconds, default 180
  GUEST_READY_TIMEOUT                    Guest shell readiness timeout in seconds, default 40
  RESET_DM_TEST_IMAGES                   1 to delete test images before running, default 1
  MIXED_INITIAL_LV_MIB                   Initial linear LV size in MiB, default 256
  MIXED_EXTENDED_LV_MIB                  Extended mixed LV size in MiB, default 512
  MIXED_BASE_FILE_MIB                    Base test file size in MiB, default 64
  MIXED_GROW_FILE_MIB                    Post-grow test file size in MiB, default 256
  MIXED_STRIPED_CHUNK_KIB                LVM stripe chunk size in KiB, default 4

Expected success markers:
  TEST_PASS_DM_MIXED_INTEGRATION_FIRST
  TEST_PASS_DM_MIXED_INTEGRATION_SECOND
  HOST_PASS_DM_MIXED_INTEGRATION
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/../..")
source "${SCRIPT_DIR}/../lib/dm_nixos_test.sh"

TEST_ID=DM_MIXED_INTEGRATION
LOG=${DM_MIXED_INTEGRATION_LOG:-/tmp/dm-mixed-integration-test.log}
DM_TEST_IMAGE=${DM_TEST_IMAGE:-target/nixos/test.img}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-target/nixos/test2.img}
DM_TEST_IMAGE_3=${DM_TEST_IMAGE_3:-target/nixos/test3.img}
DM_TEST_IMAGES=${DM_TEST_IMAGES:-${DM_TEST_IMAGE} ${DM_TEST_IMAGE_2} ${DM_TEST_IMAGE_3}}
set -- ${DM_TEST_IMAGES}
test "$#" -eq 3
DM_TEST_IMAGE=$1
DM_TEST_IMAGE_2=$2
DM_TEST_IMAGE_3=$3
GUEST_QEMU_TIMEOUT=${GUEST_QEMU_TIMEOUT:-180}
GUEST_READY_TIMEOUT=${GUEST_READY_TIMEOUT:-40}
RESET_DM_TEST_IMAGES=${RESET_DM_TEST_IMAGES:-1}
MIXED_INITIAL_LV_MIB=${MIXED_INITIAL_LV_MIB:-256}
MIXED_EXTENDED_LV_MIB=${MIXED_EXTENDED_LV_MIB:-512}
MIXED_BASE_FILE_MIB=${MIXED_BASE_FILE_MIB:-64}
MIXED_GROW_FILE_MIB=${MIXED_GROW_FILE_MIB:-256}
MIXED_STRIPED_CHUNK_KIB=${MIXED_STRIPED_CHUNK_KIB:-4}

dm_init_log "${TEST_ID}"

test "${MIXED_INITIAL_LV_MIB}" -lt "${MIXED_EXTENDED_LV_MIB}"
test "${MIXED_BASE_FILE_MIB}" -lt "${MIXED_INITIAL_LV_MIB}"
test $((MIXED_BASE_FILE_MIB + MIXED_GROW_FILE_MIB)) -gt "${MIXED_INITIAL_LV_MIB}"
test $((MIXED_BASE_FILE_MIB + MIXED_GROW_FILE_MIB)) -lt "${MIXED_EXTENDED_LV_MIB}"

cd "${ASTERINAS_DIR}"
dm_prepare_nixos_test "${TEST_ID}"
dm_emit "HOST_INFO_${TEST_ID} qemu_lifecycle_timeout=${GUEST_QEMU_TIMEOUT}s"
dm_emit "HOST_INFO_${TEST_ID} disk1=${DM_TEST_IMAGE} serial=vdmtest"
dm_emit "HOST_INFO_${TEST_ID} disk2=${DM_TEST_IMAGE_2} serial=vdmtest2"
dm_emit "HOST_INFO_${TEST_ID} disk3=${DM_TEST_IMAGE_3} serial=vdmtest3"
dm_emit "HOST_INFO_${TEST_ID} initial_lv_mib=${MIXED_INITIAL_LV_MIB} extended_lv_mib=${MIXED_EXTENDED_LV_MIB} base_file_mib=${MIXED_BASE_FILE_MIB} grow_file_mib=${MIXED_GROW_FILE_MIB} chunk_kib=${MIXED_STRIPED_CHUNK_KIB}"

FIRST_GUEST_SCRIPT=$(mktemp /tmp/dm-mixed-lvm2-first.XXXXXX)
cat >"${FIRST_GUEST_SCRIPT}" <<'GUEST_SCRIPT'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_MIXED_INTEGRATION_FIRST status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
MIXED_INITIAL_LV_MIB=__MIXED_INITIAL_LV_MIB__
MIXED_EXTENDED_LV_MIB=__MIXED_EXTENDED_LV_MIB__
MIXED_BASE_FILE_MIB=__MIXED_BASE_FILE_MIB__
MIXED_GROW_FILE_MIB=__MIXED_GROW_FILE_MIB__
MIXED_STRIPED_CHUNK_KIB=__MIXED_STRIPED_CHUNK_KIB__
MIXED_INITIAL_LV_SECTORS=$((MIXED_INITIAL_LV_MIB * 2048))
MIXED_EXTENDED_LV_SECTORS=$((MIXED_EXTENDED_LV_MIB * 2048))
MIXED_GROW_SECTORS=$((MIXED_EXTENDED_LV_SECTORS - MIXED_INITIAL_LV_SECTORS))
MIXED_STRIPED_CHUNK_SECTORS=$((MIXED_STRIPED_CHUNK_KIB * 2))
MAPPER_NAME=mixed_vg-mixed_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmmixed

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

dep_token() {
    printf '(%s, %s)' "${1%%:*}" "${1##*:}"
}

check_initial_linear_table() {
    local table_file=$1 deps_file=$2 status_file=$3 dev1=$4
    test "$(wc -l < "$table_file")" -eq 1
    awk -v len="$MIXED_INITIAL_LV_SECTORS" -v dev1="$dev1" '
        $1 == 0 && $2 == len && $3 == "linear" && $4 == dev1 { ok = 1 }
        END { exit !ok }
    ' "$table_file"
    awk -v len="$MIXED_INITIAL_LV_SECTORS" '
        $1 == 0 && $2 == len && $3 == "linear" { ok = 1 }
        END { exit !ok }
    ' "$status_file"
    grep -q '1 dependencies' "$deps_file"
    grep -F -q "$(dep_token "$dev1")" "$deps_file"
}

check_mixed_table() {
    local table_file=$1 deps_file=$2 status_file=$3 dev1=$4 dev2=$5 dev3=$6
    test "$(wc -l < "$table_file")" -eq 2
    awk -v initial_len="$MIXED_INITIAL_LV_SECTORS" -v grow_len="$MIXED_GROW_SECTORS" -v chunk="$MIXED_STRIPED_CHUNK_SECTORS" -v dev1="$dev1" -v dev2="$dev2" -v dev3="$dev3" '
        $1 == 0 && $2 == initial_len && $3 == "linear" && $4 == dev1 { linear_ok = 1 }
        $1 == initial_len && $2 == grow_len && $3 == "striped" && $4 == 2 && $5 == chunk {
            found_dev2 = 0; found_dev3 = 0
            for (i = 6; i <= NF; i += 2) {
                if ($i == dev2) found_dev2 = 1
                if ($i == dev3) found_dev3 = 1
            }
            if (found_dev2 && found_dev3) striped_ok = 1
        }
        END { exit !(linear_ok && striped_ok) }
    ' "$table_file"
    awk -v initial_len="$MIXED_INITIAL_LV_SECTORS" -v grow_len="$MIXED_GROW_SECTORS" '
        $1 == 0 && $2 == initial_len && $3 == "linear" { linear_ok = 1 }
        $1 == initial_len && $2 == grow_len && $3 == "striped" { striped_ok = 1 }
        END { exit !(linear_ok && striped_ok) }
    ' "$status_file"
    grep -q '3 dependencies' "$deps_file"
    grep -F -q "$(dep_token "$dev1")" "$deps_file"
    grep -F -q "$(dep_token "$dev2")" "$deps_file"
    grep -F -q "$(dep_token "$dev3")" "$deps_file"
}

check_mixed_table_status_deps() {
    local label=$1 mode=$2 table_file=$3 status_file=$4 deps_file=$5 dev1=$6 dev2=${7:-} dev3=${8:-}
    echo "DM_TABLE_LVM2_MIXED_${label}_BEGIN"
    dmsetup table "$MAPPER_NAME" | tee "$table_file" | sed "s/^/DM_TABLE_LVM2_MIXED_${label} /"
    echo "DM_TABLE_LVM2_MIXED_${label}_END"
    dmsetup status "$MAPPER_NAME" | tee "$status_file"
    dmsetup deps "$MAPPER_NAME" | tee "$deps_file"
    if [ "$mode" = "linear" ]; then
        check_initial_linear_table "$table_file" "$deps_file" "$status_file" "$dev1"
    else
        check_mixed_table "$table_file" "$deps_file" "$status_file" "$dev1" "$dev2" "$dev3"
    fi
}

echo '=== STEP 1: check dm control device, targets, and three test disks ==='
test -c /dev/mapper/control
dmsetup targets | tee /tmp/mixed-lvm2-targets.txt
grep -q '^linear' /tmp/mixed-lvm2-targets.txt
grep -q '^striped' /tmp/mixed-lvm2-targets.txt
TEST_DISK=$(aster-test-disk-locator 1)
TEST_DISK2=$(aster-test-disk-locator 2)
TEST_DISK3=$(aster-test-disk-locator 3)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\nTEST_DISK3=%s\n' "$TEST_DISK" "$TEST_DISK2" "$TEST_DISK3"
test "$TEST_DISK" != "$TEST_DISK2"
test "$TEST_DISK" != "$TEST_DISK3"
test "$TEST_DISK2" != "$TEST_DISK3"
test -b "$TEST_DISK"
test -b "$TEST_DISK2"
test -b "$TEST_DISK3"
DEV1=$(devno "$TEST_DISK")
DEV2=$(devno "$TEST_DISK2")
DEV3=$(devno "$TEST_DISK3")
printf 'DEV1=%s\nDEV2=%s\nDEV3=%s\n' "$DEV1" "$DEV2" "$DEV3"
echo CHECK_PASS_MIXED_LVM2_SETUP

echo '=== STEP 2: create initial linear LV on PV1 ==='
pvcreate "$TEST_DISK"
vgcreate mixed_vg "$TEST_DISK"
lvcreate --config "$LVM_CONFIG" --type linear -L "${MIXED_INITIAL_LV_MIB}M" -n mixed_lv mixed_vg "$TEST_DISK"
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices mixed_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices mixed_vg/mixed_lv
check_mixed_table_status_deps INITIAL linear /tmp/mixed-lvm2-initial-table.txt /tmp/mixed-lvm2-initial-status.txt /tmp/mixed-lvm2-initial-deps.txt "$DEV1"
echo CHECK_PASS_MIXED_LVM2_INITIAL_LINEAR_TABLE_STATUS_DEPS

echo '=== STEP 3: create ext2 filesystem and write base data ==='
mkfs.ext2 -F -b 4096 "$MAPPER_DEVICE"
blkid "$MAPPER_DEVICE"
mkdir -p "$MOUNT_DIR"
mount -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"
dd if=/dev/urandom of="$MOUNT_DIR/base${MIXED_BASE_FILE_MIB}.bin" bs=1M count="$MIXED_BASE_FILE_MIB" conv=fsync status=none
printf 'lvm2 mixed base\ninitial_lv_mib=%s chunk_kib=%s\n' "$MIXED_INITIAL_LV_MIB" "$MIXED_STRIPED_CHUNK_KIB" > "$MOUNT_DIR/base-marker.txt"
(
    cd "$MOUNT_DIR"
    md5sum "base${MIXED_BASE_FILE_MIB}.bin" base-marker.txt > mixed.md5
    md5sum -c mixed.md5
)
sync
umount "$MOUNT_DIR"
echo CHECK_PASS_MIXED_LVM2_BASE_FILE_MD5

echo '=== STEP 4: extend LV with a 2-way striped segment on PV2+PV3 ==='
pvcreate "$TEST_DISK2" "$TEST_DISK3"
vgextend mixed_vg "$TEST_DISK2" "$TEST_DISK3"
lvextend --config "$LVM_CONFIG" --type striped -i 2 -I "${MIXED_STRIPED_CHUNK_KIB}K" -L "${MIXED_EXTENDED_LV_MIB}M" mixed_vg/mixed_lv "$TEST_DISK2" "$TEST_DISK3"
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices mixed_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices mixed_vg/mixed_lv
check_mixed_table_status_deps EXTENDED mixed /tmp/mixed-lvm2-extended-table.txt /tmp/mixed-lvm2-extended-status.txt /tmp/mixed-lvm2-extended-deps.txt "$DEV1" "$DEV2" "$DEV3"
echo CHECK_PASS_MIXED_LVM2_EXTENDED_MIXED_TABLE_STATUS_DEPS

echo '=== STEP 5: grow ext2 offline and write data across the mixed LV ==='
e2fsck -f -y "$MAPPER_DEVICE"
resize2fs "$MAPPER_DEVICE"
e2fsck -f -y "$MAPPER_DEVICE"
mount -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"
(
    cd "$MOUNT_DIR"
    md5sum -c mixed.md5
)
dd if=/dev/urandom of="$MOUNT_DIR/grow${MIXED_GROW_FILE_MIB}.bin" bs=1M count="$MIXED_GROW_FILE_MIB" conv=fsync status=none
printf 'lvm2 mixed grow\nextended_lv_mib=%s grow_file_mib=%s\n' "$MIXED_EXTENDED_LV_MIB" "$MIXED_GROW_FILE_MIB" > "$MOUNT_DIR/grow-marker.txt"
(
    cd "$MOUNT_DIR"
    md5sum "grow${MIXED_GROW_FILE_MIB}.bin" grow-marker.txt >> mixed.md5
    md5sum -c mixed.md5
)
sync
umount "$MOUNT_DIR"
vgchange --config "$LVM_CONFIG" -an mixed_vg
sync
echo CHECK_PASS_MIXED_LVM2_GROW_FILE_MD5

echo TEST_PASS_DM_MIXED_INTEGRATION_FIRST
poweroff
GUEST_SCRIPT

SECOND_GUEST_SCRIPT=$(mktemp /tmp/dm-mixed-lvm2-second.XXXXXX)
cat >"${SECOND_GUEST_SCRIPT}" <<'GUEST_SCRIPT'
stty -echo 2>/dev/null || true
set -eu
trap 'status=$?; echo TEST_FAIL_DM_MIXED_INTEGRATION_SECOND status=$status; sync; poweroff; exit $status' ERR

LVM_CONFIG='activation { udev_rules=0 }'
MIXED_INITIAL_LV_MIB=__MIXED_INITIAL_LV_MIB__
MIXED_EXTENDED_LV_MIB=__MIXED_EXTENDED_LV_MIB__
MIXED_BASE_FILE_MIB=__MIXED_BASE_FILE_MIB__
MIXED_GROW_FILE_MIB=__MIXED_GROW_FILE_MIB__
MIXED_STRIPED_CHUNK_KIB=__MIXED_STRIPED_CHUNK_KIB__
MIXED_INITIAL_LV_SECTORS=$((MIXED_INITIAL_LV_MIB * 2048))
MIXED_EXTENDED_LV_SECTORS=$((MIXED_EXTENDED_LV_MIB * 2048))
MIXED_GROW_SECTORS=$((MIXED_EXTENDED_LV_SECTORS - MIXED_INITIAL_LV_SECTORS))
MIXED_STRIPED_CHUNK_SECTORS=$((MIXED_STRIPED_CHUNK_KIB * 2))
MAPPER_NAME=mixed_vg-mixed_lv
MAPPER_DEVICE=/dev/mapper/$MAPPER_NAME
MOUNT_DIR=/mnt/dmmixed

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

dep_token() {
    printf '(%s, %s)' "${1%%:*}" "${1##*:}"
}

check_mixed_table() {
    local table_file=$1 deps_file=$2 status_file=$3 dev1=$4 dev2=$5 dev3=$6
    test "$(wc -l < "$table_file")" -eq 2
    awk -v initial_len="$MIXED_INITIAL_LV_SECTORS" -v grow_len="$MIXED_GROW_SECTORS" -v chunk="$MIXED_STRIPED_CHUNK_SECTORS" -v dev1="$dev1" -v dev2="$dev2" -v dev3="$dev3" '
        $1 == 0 && $2 == initial_len && $3 == "linear" && $4 == dev1 { linear_ok = 1 }
        $1 == initial_len && $2 == grow_len && $3 == "striped" && $4 == 2 && $5 == chunk {
            found_dev2 = 0; found_dev3 = 0
            for (i = 6; i <= NF; i += 2) {
                if ($i == dev2) found_dev2 = 1
                if ($i == dev3) found_dev3 = 1
            }
            if (found_dev2 && found_dev3) striped_ok = 1
        }
        END { exit !(linear_ok && striped_ok) }
    ' "$table_file"
    awk -v initial_len="$MIXED_INITIAL_LV_SECTORS" -v grow_len="$MIXED_GROW_SECTORS" '
        $1 == 0 && $2 == initial_len && $3 == "linear" { linear_ok = 1 }
        $1 == initial_len && $2 == grow_len && $3 == "striped" { striped_ok = 1 }
        END { exit !(linear_ok && striped_ok) }
    ' "$status_file"
    grep -q '3 dependencies' "$deps_file"
    grep -F -q "$(dep_token "$dev1")" "$deps_file"
    grep -F -q "$(dep_token "$dev2")" "$deps_file"
    grep -F -q "$(dep_token "$dev3")" "$deps_file"
}

check_mixed_table_status_deps() {
    local label=$1 table_file=$2 status_file=$3 deps_file=$4 dev1=$5 dev2=$6 dev3=$7
    echo "DM_TABLE_LVM2_MIXED_${label}_BEGIN"
    dmsetup table "$MAPPER_NAME" | tee "$table_file" | sed "s/^/DM_TABLE_LVM2_MIXED_${label} /"
    echo "DM_TABLE_LVM2_MIXED_${label}_END"
    dmsetup status "$MAPPER_NAME" | tee "$status_file"
    dmsetup deps "$MAPPER_NAME" | tee "$deps_file"
    check_mixed_table "$table_file" "$deps_file" "$status_file" "$dev1" "$dev2" "$dev3"
}

echo '=== STEP 1: recover mixed linear + striped LV after reboot ==='
TEST_DISK=$(aster-test-disk-locator 1)
TEST_DISK2=$(aster-test-disk-locator 2)
TEST_DISK3=$(aster-test-disk-locator 3)
printf 'TEST_DISK=%s\nTEST_DISK2=%s\nTEST_DISK3=%s\n' "$TEST_DISK" "$TEST_DISK2" "$TEST_DISK3"
test "$TEST_DISK" != "$TEST_DISK2"
test "$TEST_DISK" != "$TEST_DISK3"
test "$TEST_DISK2" != "$TEST_DISK3"
test -b "$TEST_DISK"
test -b "$TEST_DISK2"
test -b "$TEST_DISK3"
DEV1=$(devno "$TEST_DISK")
DEV2=$(devno "$TEST_DISK2")
DEV3=$(devno "$TEST_DISK3")
printf 'DEV1=%s\nDEV2=%s\nDEV3=%s\n' "$DEV1" "$DEV2" "$DEV3"
pvscan
vgscan --mknodes
vgchange --config "$LVM_CONFIG" -ay mixed_vg
pvs -o pv_name,pv_size,vg_name
vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
lvs -a -o lv_name,lv_size,seg_count,devices mixed_vg
lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices mixed_vg/mixed_lv
check_mixed_table_status_deps RECOVERED /tmp/mixed-lvm2-recovered-table.txt /tmp/mixed-lvm2-recovered-status.txt /tmp/mixed-lvm2-recovered-deps.txt "$DEV1" "$DEV2" "$DEV3"
echo CHECK_PASS_MIXED_LVM2_RECOVERED_MIXED_TABLE_STATUS_DEPS

echo '=== STEP 2: readonly mount and verify md5 after reboot ==='
mkdir -p "$MOUNT_DIR"
mount -o ro -t ext2 "$MAPPER_DEVICE" "$MOUNT_DIR"
(
    cd "$MOUNT_DIR"
    md5sum -c mixed.md5
    cat base-marker.txt
    cat grow-marker.txt
)
df -h "$MOUNT_DIR"
du -sh "$MOUNT_DIR" "$MOUNT_DIR/base${MIXED_BASE_FILE_MIB}.bin" "$MOUNT_DIR/grow${MIXED_GROW_FILE_MIB}.bin"
umount "$MOUNT_DIR"
vgchange --config "$LVM_CONFIG" -an mixed_vg
sync
echo CHECK_PASS_MIXED_LVM2_RECOVERED_FILE_MD5

echo TEST_PASS_DM_MIXED_INTEGRATION_SECOND
poweroff
GUEST_SCRIPT

for script in "${FIRST_GUEST_SCRIPT}" "${SECOND_GUEST_SCRIPT}"; do
    sed -i \
        -e "s/__MIXED_INITIAL_LV_MIB__/${MIXED_INITIAL_LV_MIB}/g" \
        -e "s/__MIXED_EXTENDED_LV_MIB__/${MIXED_EXTENDED_LV_MIB}/g" \
        -e "s/__MIXED_BASE_FILE_MIB__/${MIXED_BASE_FILE_MIB}/g" \
        -e "s/__MIXED_GROW_FILE_MIB__/${MIXED_GROW_FILE_MIB}/g" \
        -e "s/__MIXED_STRIPED_CHUNK_KIB__/${MIXED_STRIPED_CHUNK_KIB}/g" \
        "${script}"
done

SUMMARY_INCLUDE='^TEST_|^CHECK_PASS_|^=== STEP|^=== CHECK|^TEST_DISK=|^TEST_DISK2=|^TEST_DISK3=|^DEV1=|^DEV2=|^DEV3=|^DM_TABLE_LVM2_MIXED|^linear[[:space:]]|^striped[[:space:]]|^0 [0-9]+ linear |^[0-9]+ [0-9]+ striped |^[0-9]+ dependencies|base[0-9]+\.bin: OK|grow[0-9]+\.bin: OK|No space left|Input/output error|Command failed|Kernel panic|panicked|records in|records out|bytes .* copied'
dm_run_two_guest_test \
    "${TEST_ID}" \
    "${FIRST_GUEST_SCRIPT}" \
    "${SECOND_GUEST_SCRIPT}" \
    TEST_PASS_DM_MIXED_INTEGRATION_FIRST \
    TEST_PASS_DM_MIXED_INTEGRATION_SECOND \
    "${SUMMARY_INCLUDE}"
