#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/run_lvm2_topology_test.sh

Runs one NixOS guest pass for LVM2 topology and lifecycle semantics. It covers
PV/VG/LV management, linear and striped growth across backing sets, mixed
segment tables, activation recovery, and removal without filesystem or reboot
checks.

Optional environment variables:
  DM_TEST_IMAGES            Space-separated backing image paths, default four test images
  LVM2_TOPOLOGY_LOG              Host-side log path, default /tmp/lvm2-topology-test.log
  GUEST_QEMU_TIMEOUT        Full QEMU lifecycle timeout in seconds, default 180
  GUEST_READY_TIMEOUT       Guest shell readiness timeout in seconds, default 40
  RESET_DM_TEST_IMAGES      1 to delete test images before running, default 1

Expected success markers:
  SUMMARY_GAP_LVM2_TOPOLOGY: 0
  TEST_PASS_LVM2_TOPOLOGY
  HOST_PASS_LVM2_TOPOLOGY
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/..")
source "${SCRIPT_DIR}/lib/dm_nixos_test.sh"

TEST_ID=LVM2_TOPOLOGY
LOG=${LVM2_TOPOLOGY_LOG:-/tmp/lvm2-topology-test.log}
DM_TEST_IMAGE=${DM_TEST_IMAGE:-target/nixos/test.img}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-target/nixos/test2.img}
DM_TEST_IMAGES=${DM_TEST_IMAGES:-target/nixos/test.img target/nixos/test2.img target/nixos/test3.img target/nixos/test4.img}
GUEST_QEMU_TIMEOUT=${GUEST_QEMU_TIMEOUT:-180}
GUEST_READY_TIMEOUT=${GUEST_READY_TIMEOUT:-40}
GUEST_INPUT_LINE_DELAY=${GUEST_INPUT_LINE_DELAY:-0.01}
RESET_DM_TEST_IMAGES=${RESET_DM_TEST_IMAGES:-1}

cd "${ASTERINAS_DIR}"
dm_prepare_nixos_test "${TEST_ID}"
echo "HOST_INFO_${TEST_ID} disks=${DM_TEST_IMAGES} serials=vdmtest,vdmtest2,vdmtest3,vdmtest4"
echo "HOST_INFO_${TEST_ID} qemu_lifecycle_timeout=${GUEST_QEMU_TIMEOUT}s"

GUEST_SCRIPT_FILE=$(mktemp /tmp/lvm2-topology-guest.XXXXXX)
cat >"${GUEST_SCRIPT_FILE}" <<'GUEST_SCRIPT'
stty -echo 2>/dev/null || true
cat >/tmp/lvm2_topology_guest.sh <<'LVM2_GUEST_BODY'
set -u

PREFIX=lvm2_topology
TEST_VG=${PREFIX}_vg
LINEAR_LV=${PREFIX}_linear_lv
STRIPED_LV=${PREFIX}_striped_lv
MIXED_LV=${PREFIX}_mixed_lv
LINEAR_INITIAL_MIB=32
LINEAR_SAME_PV_EXTENDED_MIB=64
LINEAR_CROSS_PV_EXTENDED_MIB=96
LINEAR_SHRUNK_MIB=32
STRIPED_INITIAL_MIB=32
STRIPED_SAME_SET_EXTENDED_MIB=64
STRIPED_CROSS_SET_EXTENDED_MIB=96
STRIPED_SHRUNK_MIB=32
MIXED_INITIAL_MIB=32
MIXED_EXTENDED_MIB=64
STRIPES=2
STRIPE_SIZE_KIB=64
GUEST_TEST_START=$(date +%s)
STEP_START=${GUEST_TEST_START}
STEP_LABEL=START
GAP_COUNT=0
CMD_TIMEOUT=${LVM2_CMD_TIMEOUT:-8}
LVM_DEVICES_SUPPORTED=0

now_s() {
    date +%s
}

label_from_step() {
    printf '%s' "$1" | sed 's/^=== //; s/:.*$//; s/[^A-Za-z0-9_]/_/g'
}

step() {
    next_label=$1
    now=$(now_s)
    echo "STEP_DURATION_${STEP_LABEL}: $((now - STEP_START))s total=$((now - GUEST_TEST_START))s"
    echo "${next_label}"
    STEP_LABEL=$(label_from_step "${next_label}")
    STEP_START=${now}
}

finish_steps() {
    now=$(now_s)
    echo "STEP_DURATION_${STEP_LABEL}: $((now - STEP_START))s total=$((now - GUEST_TEST_START))s"
    echo "GUEST_DURATION_LVM2_TOPOLOGY: $((now - GUEST_TEST_START))s"
    echo "SUMMARY_GAP_LVM2_TOPOLOGY: ${GAP_COUNT}"
}

observe_gap() {
    GAP_COUNT=$((GAP_COUNT + 1))
    echo "OBSERVE_GAP_LVM2_TOPOLOGY $*"
}

fail_precondition() {
    echo "TEST_FAIL_LVM2_TOPOLOGY $*"
    exit 1
}

print_stream() {
    prefix=$1
    label=$2
    file=$3
    if [ ! -s "${file}" ]; then
        echo "${prefix}_${label}: <empty>"
        return
    fi
    sed "s/^/${prefix}_${label}: /" "${file}"
}

run_capture() {
    label=$1
    shift
    out="/tmp/${label}.out"
    err="/tmp/${label}.err"
    started=$(now_s)
    echo "SCENARIO_BEGIN_${label}"
    echo "CMD_${label}: $*"
    timeout "${CMD_TIMEOUT}" "$@" >"${out}" 2>"${err}"
    status=$?
    ended=$(now_s)
    echo "STATUS_${label}: ${status}"
    echo "DURATION_${label}: $((ended - started))s"
    print_stream STDOUT "${label}" "${out}"
    print_stream STDERR "${label}" "${err}"
    echo "SCENARIO_END_${label}"
    return "${status}"
}

run_lvm_capture() {
    label=$1
    shift
    if [ "${LVM_DEVICES_SUPPORTED}" = "1" ]; then
        run_capture "${label}" "$@" --config "${LVM_CONFIG}" --devices "${DEVICES_CSV}"
    else
        run_capture "${label}" "$@" --config "${LVM_CONFIG}"
    fi
}

run_expect_success() {
    expect_label=$1
    shift
    if ! run_capture "${expect_label}" "$@"; then
        observe_gap "${expect_label}_failed"
    fi
    return 0
}

run_lvm_expect_success() {
    expect_label=$1
    shift
    if ! run_lvm_capture "${expect_label}" "$@"; then
        observe_gap "${expect_label}_failed"
    fi
    return 0
}

grep_expect() {
    grep_label=$1
    pattern=$2
    file=$3
    if ! timeout 3 grep -E -q -- "${pattern}" "${file}"; then
        observe_gap "${grep_label}_grep_failed"
    fi
}

line_count_expect() {
    count_label=$1
    expected=$2
    file=$3
    count=$(awk 'NF && $1 != "LV" { count++ } END { print count + 0 }' "${file}")
    if [ "${count}" -ne "${expected}" ]; then
        observe_gap "${count_label}_line_count_${count}_expected_${expected}"
    fi
}

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

mapper_name() {
    printf '%s-%s' "${TEST_VG}" "$1"
}

locate_disk() {
    serial=$1
    if [ "${serial}" = "vdmtest" ]; then
        aster-dm-disk-locator
    else
        aster-dm-disk-locator "${serial}"
    fi
}

record_dm_state() {
    dm_label=$1
    dm_mapper=$2
    run_expect_success "${dm_label}_DM_TABLE" dmsetup table "${dm_mapper}"
    run_expect_success "${dm_label}_DM_STATUS" dmsetup status "${dm_mapper}"
    run_expect_success "${dm_label}_DM_DEPS" dmsetup deps "${dm_mapper}"
}

cleanup_lvm() {
    lvremove --config "${LVM_CONFIG:-activation { udev_rules=0 }}" -y "${TEST_VG}/${MIXED_LV}" >/dev/null 2>&1 || true
    lvremove --config "${LVM_CONFIG:-activation { udev_rules=0 }}" -y "${TEST_VG}/${STRIPED_LV}" >/dev/null 2>&1 || true
    lvremove --config "${LVM_CONFIG:-activation { udev_rules=0 }}" -y "${TEST_VG}/${LINEAR_LV}" >/dev/null 2>&1 || true
    vgchange --config "${LVM_CONFIG:-activation { udev_rules=0 }}" -an "${TEST_VG}" >/dev/null 2>&1 || true
    vgremove --config "${LVM_CONFIG:-activation { udev_rules=0 }}" -y "${TEST_VG}" >/dev/null 2>&1 || true
    pvremove --config "${LVM_CONFIG:-activation { udev_rules=0 }}" -ff -y ${DISK1:-} ${DISK2:-} ${DISK3:-} ${DISK4:-} >/dev/null 2>&1 || true
}

trap 'status=$?; if [ "${status}" -ne 0 ]; then echo TEST_FAIL_LVM2_TOPOLOGY status=${status}; fi; cleanup_lvm; sync; poweroff; exit ${status}' EXIT

step '=== STEP 1: locate LVM2 test disks ==='
DISK1=$(locate_disk vdmtest) || fail_precondition locate_disk1_failed
DISK2=$(locate_disk vdmtest2) || fail_precondition locate_disk2_failed
DISK3=$(locate_disk vdmtest3) || fail_precondition locate_disk3_failed
DISK4=$(locate_disk vdmtest4) || fail_precondition locate_disk4_failed
printf 'TEST_DISK1=%s\nTEST_DISK2=%s\nTEST_DISK3=%s\nTEST_DISK4=%s\n' "${DISK1}" "${DISK2}" "${DISK3}" "${DISK4}"
[ -b "${DISK1}" ] || fail_precondition missing_test_disk1
[ -b "${DISK2}" ] || fail_precondition missing_test_disk2
[ -b "${DISK3}" ] || fail_precondition missing_test_disk3
[ -b "${DISK4}" ] || fail_precondition missing_test_disk4
[ "${DISK1}" != "${DISK2}" ] || fail_precondition duplicate_disk12
[ "${DISK1}" != "${DISK3}" ] || fail_precondition duplicate_disk13
[ "${DISK1}" != "${DISK4}" ] || fail_precondition duplicate_disk14
[ "${DISK2}" != "${DISK3}" ] || fail_precondition duplicate_disk23
[ "${DISK2}" != "${DISK4}" ] || fail_precondition duplicate_disk24
[ "${DISK3}" != "${DISK4}" ] || fail_precondition duplicate_disk34
DEV1=$(devno "${DISK1}")
DEV2=$(devno "${DISK2}")
DEV3=$(devno "${DISK3}")
DEV4=$(devno "${DISK4}")
printf 'DEV1=%s\nDEV2=%s\nDEV3=%s\nDEV4=%s\n' "${DEV1}" "${DEV2}" "${DEV3}" "${DEV4}"
DEVICES_CSV=${DISK1},${DISK2},${DISK3},${DISK4}
LVM_CONFIG="devices { use_devicesfile = 0 filter = [ \"a|^${DISK1}$|\", \"a|^${DISK2}$|\", \"a|^${DISK3}$|\", \"a|^${DISK4}$|\", \"r|.*|\" ] global_filter = [ \"a|^${DISK1}$|\", \"a|^${DISK2}$|\", \"a|^${DISK3}$|\", \"a|^${DISK4}$|\", \"r|.*|\" ] } activation { udev_rules = 0 udev_sync = 0 }"
if pvs --config "${LVM_CONFIG}" --devices "${DEVICES_CSV}" --noheadings -o pv_name >/dev/null 2>&1; then
    LVM_DEVICES_SUPPORTED=1
fi
echo "LVM_DEVICES_SUPPORTED=${LVM_DEVICES_SUPPORTED}"

step '=== STEP 2: static LVM2 queries under test filter ==='
run_lvm_expect_success STATIC_PVS pvs -o pv_name,vg_name,pv_size
run_lvm_expect_success STATIC_VGS vgs -o vg_name,pv_count,lv_count,vg_size,vg_free
run_lvm_expect_success STATIC_LVS lvs -a -o vg_name,lv_name,lv_size,seg_count,devices

step '=== STEP 3: PV and VG lifecycle ==='
run_lvm_expect_success PV_CREATE pvcreate -ff -y "${DISK1}" "${DISK2}" "${DISK3}" "${DISK4}"
run_lvm_expect_success PVS_AFTER_PVCREATE pvs -o pv_name,pv_size,vg_name
grep_expect PVS_AFTER_PVCREATE_DISK1 "${DISK1}" /tmp/PVS_AFTER_PVCREATE.out
grep_expect PVS_AFTER_PVCREATE_DISK4 "${DISK4}" /tmp/PVS_AFTER_PVCREATE.out
run_lvm_expect_success PVSCAN_AFTER_PVCREATE pvscan
run_lvm_expect_success VG_CREATE vgcreate "${TEST_VG}" "${DISK1}" "${DISK2}"
run_lvm_expect_success VGS_AFTER_VGCREATE vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
grep_expect VGS_AFTER_VGCREATE_NAME "${TEST_VG}" /tmp/VGS_AFTER_VGCREATE.out
run_lvm_expect_success VG_EXTEND vgextend "${TEST_VG}" "${DISK3}" "${DISK4}"
run_lvm_expect_success VGS_AFTER_VGEXTEND vgs -o vg_name,vg_size,vg_free,pv_count,lv_count
grep_expect VGS_AFTER_VGEXTEND_NAME "${TEST_VG}" /tmp/VGS_AFTER_VGEXTEND.out

step '=== STEP 4: linear LV create grow shrink ==='
run_lvm_expect_success LV_CREATE_LINEAR lvcreate --type linear -L "${LINEAR_INITIAL_MIB}M" -n "${LINEAR_LV}" "${TEST_VG}" "${DISK1}"
run_lvm_expect_success LVS_LINEAR_INITIAL lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}"
run_lvm_expect_success LVS_SEGMENTS_LINEAR_INITIAL lvs --segments -o lv_name,seg_start,seg_size,segtype,devices "${TEST_VG}/${LINEAR_LV}"
grep_expect LVS_SEGMENTS_LINEAR_INITIAL_TYPE 'linear' /tmp/LVS_SEGMENTS_LINEAR_INITIAL.out
line_count_expect LVS_SEGMENTS_LINEAR_INITIAL 1 /tmp/LVS_SEGMENTS_LINEAR_INITIAL.out
record_dm_state LINEAR_INITIAL "$(mapper_name "${LINEAR_LV}")"
grep_expect LINEAR_INITIAL_TABLE_TYPE ' linear ' /tmp/LINEAR_INITIAL_DM_TABLE.out
grep_expect LINEAR_INITIAL_DEPS '1 dependencies' /tmp/LINEAR_INITIAL_DM_DEPS.out

run_lvm_expect_success LV_EXTEND_LINEAR_SAME_PV lvextend -L "${LINEAR_SAME_PV_EXTENDED_MIB}M" "${TEST_VG}/${LINEAR_LV}" "${DISK1}"
run_lvm_expect_success LVS_LINEAR_SAME_PV_EXTENDED lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}"
run_lvm_expect_success LVS_SEGMENTS_LINEAR_SAME_PV_EXTENDED lvs --segments -o lv_name,seg_start,seg_size,segtype,devices "${TEST_VG}/${LINEAR_LV}"
grep_expect LVS_SEGMENTS_LINEAR_SAME_PV_EXTENDED_TYPE 'linear' /tmp/LVS_SEGMENTS_LINEAR_SAME_PV_EXTENDED.out
record_dm_state LINEAR_SAME_PV_EXTENDED "$(mapper_name "${LINEAR_LV}")"

run_lvm_expect_success LV_EXTEND_LINEAR_CROSS_PV lvextend -L "${LINEAR_CROSS_PV_EXTENDED_MIB}M" "${TEST_VG}/${LINEAR_LV}" "${DISK2}"
run_lvm_expect_success LVS_LINEAR_CROSS_PV_EXTENDED lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}"
run_lvm_expect_success LVS_SEGMENTS_LINEAR_CROSS_PV_EXTENDED lvs --segments -o lv_name,seg_start,seg_size,segtype,devices "${TEST_VG}/${LINEAR_LV}"
grep_expect LVS_SEGMENTS_LINEAR_CROSS_PV_EXTENDED_TYPE 'linear' /tmp/LVS_SEGMENTS_LINEAR_CROSS_PV_EXTENDED.out
record_dm_state LINEAR_CROSS_PV_EXTENDED "$(mapper_name "${LINEAR_LV}")"
grep_expect LINEAR_CROSS_PV_EXTENDED_DEPS '2 dependencies' /tmp/LINEAR_CROSS_PV_EXTENDED_DM_DEPS.out

run_lvm_expect_success LV_REDUCE_LINEAR lvreduce -y -L "${LINEAR_SHRUNK_MIB}M" "${TEST_VG}/${LINEAR_LV}"
run_lvm_expect_success LVS_LINEAR_SHRUNK lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}"
run_lvm_expect_success LVS_SEGMENTS_LINEAR_SHRUNK lvs --segments -o lv_name,seg_start,seg_size,segtype,devices "${TEST_VG}/${LINEAR_LV}"
record_dm_state LINEAR_SHRUNK "$(mapper_name "${LINEAR_LV}")"

step '=== STEP 5: striped LV create grow shrink ==='
run_lvm_expect_success LV_CREATE_STRIPED lvcreate --type striped -i "${STRIPES}" -I "${STRIPE_SIZE_KIB}K" -L "${STRIPED_INITIAL_MIB}M" -n "${STRIPED_LV}" "${TEST_VG}" "${DISK1}" "${DISK2}"
run_lvm_expect_success LVS_STRIPED_INITIAL lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}"
run_lvm_expect_success LVS_SEGMENTS_STRIPED_INITIAL lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices "${TEST_VG}/${STRIPED_LV}"
grep_expect LVS_SEGMENTS_STRIPED_INITIAL_TYPE 'striped' /tmp/LVS_SEGMENTS_STRIPED_INITIAL.out
grep_expect LVS_SEGMENTS_STRIPED_INITIAL_STRIPES '2' /tmp/LVS_SEGMENTS_STRIPED_INITIAL.out
record_dm_state STRIPED_INITIAL "$(mapper_name "${STRIPED_LV}")"
grep_expect STRIPED_INITIAL_TABLE_TYPE ' striped ' /tmp/STRIPED_INITIAL_DM_TABLE.out
grep_expect STRIPED_INITIAL_DEPS '2 dependencies' /tmp/STRIPED_INITIAL_DM_DEPS.out

run_lvm_expect_success LV_EXTEND_STRIPED_SAME_SET lvextend -i "${STRIPES}" -I "${STRIPE_SIZE_KIB}K" -L "${STRIPED_SAME_SET_EXTENDED_MIB}M" "${TEST_VG}/${STRIPED_LV}" "${DISK1}" "${DISK2}"
run_lvm_expect_success LVS_STRIPED_SAME_SET_EXTENDED lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}"
run_lvm_expect_success LVS_SEGMENTS_STRIPED_SAME_SET_EXTENDED lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices "${TEST_VG}/${STRIPED_LV}"
grep_expect LVS_SEGMENTS_STRIPED_SAME_SET_EXTENDED_TYPE 'striped' /tmp/LVS_SEGMENTS_STRIPED_SAME_SET_EXTENDED.out
record_dm_state STRIPED_SAME_SET_EXTENDED "$(mapper_name "${STRIPED_LV}")"

run_lvm_expect_success LV_EXTEND_STRIPED_CROSS_SET lvextend -i "${STRIPES}" -I "${STRIPE_SIZE_KIB}K" -L "${STRIPED_CROSS_SET_EXTENDED_MIB}M" "${TEST_VG}/${STRIPED_LV}" "${DISK3}" "${DISK4}"
run_lvm_expect_success LVS_STRIPED_CROSS_SET_EXTENDED lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}"
run_lvm_expect_success LVS_SEGMENTS_STRIPED_CROSS_SET_EXTENDED lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices "${TEST_VG}/${STRIPED_LV}"
grep_expect LVS_SEGMENTS_STRIPED_CROSS_SET_EXTENDED_TYPE 'striped' /tmp/LVS_SEGMENTS_STRIPED_CROSS_SET_EXTENDED.out
record_dm_state STRIPED_CROSS_SET_EXTENDED "$(mapper_name "${STRIPED_LV}")"
grep_expect STRIPED_CROSS_SET_EXTENDED_DEPS '4 dependencies' /tmp/STRIPED_CROSS_SET_EXTENDED_DM_DEPS.out

run_lvm_expect_success LV_REDUCE_STRIPED lvreduce -y -L "${STRIPED_SHRUNK_MIB}M" "${TEST_VG}/${STRIPED_LV}"
run_lvm_expect_success LVS_STRIPED_SHRUNK lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}"
run_lvm_expect_success LVS_SEGMENTS_STRIPED_SHRUNK lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices "${TEST_VG}/${STRIPED_LV}"
record_dm_state STRIPED_SHRUNK "$(mapper_name "${STRIPED_LV}")"

step '=== STEP 6: mixed linear plus striped LV ==='
run_lvm_expect_success LV_CREATE_MIXED_LINEAR lvcreate --type linear -L "${MIXED_INITIAL_MIB}M" -n "${MIXED_LV}" "${TEST_VG}" "${DISK4}"
run_lvm_expect_success LV_EXTEND_MIXED_STRIPED lvextend --type striped -i "${STRIPES}" -I "${STRIPE_SIZE_KIB}K" -L "${MIXED_EXTENDED_MIB}M" "${TEST_VG}/${MIXED_LV}" "${DISK1}" "${DISK2}"
run_lvm_expect_success LVS_MIXED lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}"
run_lvm_expect_success LVS_SEGMENTS_MIXED lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices "${TEST_VG}/${MIXED_LV}"
grep_expect LVS_SEGMENTS_MIXED_LINEAR 'linear' /tmp/LVS_SEGMENTS_MIXED.out
grep_expect LVS_SEGMENTS_MIXED_STRIPED 'striped' /tmp/LVS_SEGMENTS_MIXED.out
record_dm_state MIXED "$(mapper_name "${MIXED_LV}")"
grep_expect MIXED_DEPS '3 dependencies' /tmp/MIXED_DM_DEPS.out

step '=== STEP 7: activation scan mknodes ==='
run_lvm_expect_success VGCHANGE_INACTIVE vgchange -an "${TEST_VG}"
run_expect_success DM_LS_AFTER_VGCHANGE_INACTIVE dmsetup ls
run_lvm_expect_success PVSCAN_RECOVERY pvscan
run_lvm_expect_success VGSCAN_MKNODES vgscan --mknodes
run_lvm_expect_success VGCHANGE_ACTIVE vgchange -ay "${TEST_VG}"
run_lvm_expect_success LVS_AFTER_REACTIVATE lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}"
record_dm_state LINEAR_AFTER_REACTIVATE "$(mapper_name "${LINEAR_LV}")"

step '=== STEP 8: test-object remove lifecycle ==='
run_lvm_expect_success LVREMOVE_MIXED lvremove -y "${TEST_VG}/${MIXED_LV}"
run_lvm_expect_success LVREMOVE_STRIPED lvremove -y "${TEST_VG}/${STRIPED_LV}"
run_lvm_expect_success LVREMOVE_LINEAR lvremove -y "${TEST_VG}/${LINEAR_LV}"
run_lvm_expect_success LVS_AFTER_LVREMOVE lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}"
run_lvm_expect_success VGREMOVE_TEST vgremove -y "${TEST_VG}"
run_lvm_expect_success VGS_AFTER_VGREMOVE vgs -o vg_name,pv_count,lv_count
run_lvm_expect_success PVREMOVE_TEST pvremove -ff -y "${DISK1}" "${DISK2}" "${DISK3}" "${DISK4}"
run_lvm_expect_success PVS_AFTER_PVREMOVE pvs -o pv_name,pv_size,vg_name

cleanup_lvm
sync
finish_steps
if [ "${GAP_COUNT}" -ne 0 ]; then
    echo "TEST_FAIL_LVM2_TOPOLOGY gap_count=${GAP_COUNT}"
    exit 1
fi
echo TEST_PASS_LVM2_TOPOLOGY
poweroff
LVM2_GUEST_BODY
sh /tmp/lvm2_topology_guest.sh

GUEST_SCRIPT

SUMMARY_INCLUDE='^(TEST_PASS|TEST_FAIL|OBSERVE_|=== STEP|STATUS_|STEP_DURATION_|GUEST_DURATION_|SUMMARY_GAP_|TEST_DISK|DEV[0-9]=|LVM_DEVICES_SUPPORTED|HOST_FAIL|Kernel panic|panicked)'
dm_run_single_guest_test "${TEST_ID}" "${GUEST_SCRIPT_FILE}" TEST_PASS_LVM2_TOPOLOGY "${SUMMARY_INCLUDE}"
