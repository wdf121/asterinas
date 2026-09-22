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

dm_init_log "${TEST_ID}"

cd "${ASTERINAS_DIR}"
dm_prepare_nixos_test "${TEST_ID}"
dm_emit "HOST_INFO_${TEST_ID} disks=${DM_TEST_IMAGES} serials=vdmtest,vdmtest2,vdmtest3,vdmtest4"
dm_emit "HOST_INFO_${TEST_ID} qemu_lifecycle_timeout=${GUEST_QEMU_TIMEOUT}s"

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

run_lvm_report() {
    report_label=$1
    shift
    LC_ALL=C run_lvm_expect_success "${report_label}" "$@" \
        --reportformat basic --noheadings --separator '|' --units b --nosuffix
}

normalize_report() {
    awk -F'|' '
        NF {
            for (i = 1; i <= NF; i++) {
                gsub(/^[[:space:]]+|[[:space:]]+$/, "", $i)
            }
            line = $1
            for (i = 2; i <= NF; i++) {
                line = line "|" $i
            }
            print line
        }
    ' "$1"
}

expect_row() {
    row_label=$1
    file=$2
    expected=$3
    matches=$(normalize_report "${file}" | awk -v expected="${expected}" '$0 == expected { count++ } END { print count + 0 }')
    if [ "${matches}" -ne 1 ]; then
        actual=$(normalize_report "${file}" | tr '\n' ';')
        observe_gap "${row_label}_row_matches_${matches}_expected_1 actual=${actual:-<empty>} expected=${expected}"
    fi
}

expect_column_set() {
    set_label=$1
    file=$2
    column=$3
    shift 3
    actual_file="/tmp/${set_label}.actual"
    expected_file="/tmp/${set_label}.expected"
    normalize_report "${file}" | awk -F'|' -v column="${column}" '$column != "" { print $column }' | sort -u >"${actual_file}"
    printf '%s\n' "$@" | awk 'NF' | sort -u >"${expected_file}"
    if ! diff -u "${expected_file}" "${actual_file}" >/tmp/"${set_label}".diff; then
        actual=$(tr '\n' ',' <"${actual_file}")
        expected=$(tr '\n' ',' <"${expected_file}")
        observe_gap "${set_label}_set_mismatch actual=${actual:-<empty>} expected=${expected:-<empty>}"
    fi
}

expect_empty_report() {
    empty_label=$1
    file=$2
    if normalize_report "${file}" | grep -q .; then
        actual=$(normalize_report "${file}" | tr '\n' ';')
        observe_gap "${empty_label}_expected_empty actual=${actual}"
    fi
}

extract_dm_deps() {
    awk '
        {
            for (i = 1; i < NF; i++) {
                if ($i ~ /^\([0-9]+,$/ && $(i + 1) ~ /^[0-9]+\)$/) {
                    major = $i
                    minor = $(i + 1)
                    gsub(/[^0-9]/, "", major)
                    gsub(/[^0-9]/, "", minor)
                    print major ":" minor
                }
            }
        }
    ' "$1"
}

expect_dm_deps_set() {
    deps_label=$1
    file=$2
    shift 2
    actual_file="/tmp/${deps_label}.actual"
    expected_file="/tmp/${deps_label}.expected"
    extract_dm_deps "${file}" | sort -u >"${actual_file}"
    printf '%s\n' "$@" | awk 'NF' | sort -u >"${expected_file}"
    if ! diff -u "${expected_file}" "${actual_file}" >/tmp/"${deps_label}".diff; then
        actual=$(tr '\n' ',' <"${actual_file}")
        expected=$(tr '\n' ',' <"${expected_file}")
        observe_gap "${deps_label}_deps_mismatch actual=${actual:-<empty>} expected=${expected:-<empty>}"
    fi
}

expect_mapper_absent() {
    absent_label=$1
    lv_name=$2
    dm_name=$(mapper_name "${lv_name}")
    if run_capture "${absent_label}_DM_INFO" dmsetup info "${dm_name}"; then
        observe_gap "${absent_label}_dm_info_present"
    fi
    mapper_path="/dev/mapper/${dm_name}"
    if [ -e "${mapper_path}" ] || [ -L "${mapper_path}" ]; then
        observe_gap "${absent_label}_mapper_path_present_${mapper_path}"
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
run_lvm_report STATIC_PVS pvs -o pv_name,vg_name
run_lvm_report STATIC_VGS vgs -o vg_name,pv_count,lv_count
run_lvm_report STATIC_LVS lvs -a -o lv_name,lv_size,seg_count

step '=== STEP 3: PV and VG lifecycle ==='
run_lvm_expect_success PV_CREATE pvcreate -ff -y "${DISK1}" "${DISK2}" "${DISK3}" "${DISK4}"
run_lvm_report PVS_AFTER_PVCREATE pvs -o pv_name,vg_name
expect_column_set PVS_AFTER_PVCREATE_NAMES /tmp/PVS_AFTER_PVCREATE.out 1 "${DISK1}" "${DISK2}" "${DISK3}" "${DISK4}"
expect_row PVS_AFTER_PVCREATE_DISK1 /tmp/PVS_AFTER_PVCREATE.out "${DISK1}|"
expect_row PVS_AFTER_PVCREATE_DISK2 /tmp/PVS_AFTER_PVCREATE.out "${DISK2}|"
expect_row PVS_AFTER_PVCREATE_DISK3 /tmp/PVS_AFTER_PVCREATE.out "${DISK3}|"
expect_row PVS_AFTER_PVCREATE_DISK4 /tmp/PVS_AFTER_PVCREATE.out "${DISK4}|"
run_lvm_expect_success PVSCAN_AFTER_PVCREATE pvscan
run_lvm_expect_success VG_CREATE vgcreate "${TEST_VG}" "${DISK1}" "${DISK2}"
run_lvm_report VGS_AFTER_VGCREATE vgs -o vg_name,pv_count,lv_count
expect_row VGS_AFTER_VGCREATE_COUNTS /tmp/VGS_AFTER_VGCREATE.out "${TEST_VG}|2|0"
run_lvm_expect_success VG_EXTEND vgextend "${TEST_VG}" "${DISK3}" "${DISK4}"
run_lvm_report VGS_AFTER_VGEXTEND vgs -o vg_name,pv_count,lv_count
expect_row VGS_AFTER_VGEXTEND_COUNTS /tmp/VGS_AFTER_VGEXTEND.out "${TEST_VG}|4|0"
run_lvm_report PVS_AFTER_VGEXTEND pvs -o pv_name,vg_name
expect_column_set PVS_AFTER_VGEXTEND_NAMES /tmp/PVS_AFTER_VGEXTEND.out 1 "${DISK1}" "${DISK2}" "${DISK3}" "${DISK4}"
expect_row PVS_AFTER_VGEXTEND_DISK1 /tmp/PVS_AFTER_VGEXTEND.out "${DISK1}|${TEST_VG}"
expect_row PVS_AFTER_VGEXTEND_DISK2 /tmp/PVS_AFTER_VGEXTEND.out "${DISK2}|${TEST_VG}"
expect_row PVS_AFTER_VGEXTEND_DISK3 /tmp/PVS_AFTER_VGEXTEND.out "${DISK3}|${TEST_VG}"
expect_row PVS_AFTER_VGEXTEND_DISK4 /tmp/PVS_AFTER_VGEXTEND.out "${DISK4}|${TEST_VG}"

step '=== STEP 4: linear LV create grow shrink ==='
run_lvm_expect_success LV_CREATE_LINEAR lvcreate --type linear -L "${LINEAR_INITIAL_MIB}M" -n "${LINEAR_LV}" "${TEST_VG}" "${DISK1}"
run_lvm_report LVS_LINEAR_INITIAL lvs -o lv_name,lv_size,seg_count,segtype "${TEST_VG}/${LINEAR_LV}"
expect_row LVS_LINEAR_INITIAL_FIELDS /tmp/LVS_LINEAR_INITIAL.out "${LINEAR_LV}|$((LINEAR_INITIAL_MIB * 1024 * 1024))|1|linear"
record_dm_state LINEAR_INITIAL "$(mapper_name "${LINEAR_LV}")"
expect_dm_deps_set LINEAR_INITIAL_DEPS /tmp/LINEAR_INITIAL_DM_DEPS.out "${DEV1}"

run_lvm_expect_success LV_EXTEND_LINEAR_SAME_PV lvextend -L "${LINEAR_SAME_PV_EXTENDED_MIB}M" "${TEST_VG}/${LINEAR_LV}" "${DISK1}"
run_lvm_report LVS_LINEAR_SAME_PV_EXTENDED lvs -o lv_name,lv_size,seg_count,segtype "${TEST_VG}/${LINEAR_LV}"
expect_row LVS_LINEAR_SAME_PV_EXTENDED_FIELDS /tmp/LVS_LINEAR_SAME_PV_EXTENDED.out "${LINEAR_LV}|$((LINEAR_SAME_PV_EXTENDED_MIB * 1024 * 1024))|1|linear"
record_dm_state LINEAR_SAME_PV_EXTENDED "$(mapper_name "${LINEAR_LV}")"
expect_dm_deps_set LINEAR_SAME_PV_EXTENDED_DEPS /tmp/LINEAR_SAME_PV_EXTENDED_DM_DEPS.out "${DEV1}"

run_lvm_expect_success LV_EXTEND_LINEAR_CROSS_PV lvextend -L "${LINEAR_CROSS_PV_EXTENDED_MIB}M" "${TEST_VG}/${LINEAR_LV}" "${DISK2}"
run_lvm_report LVS_LINEAR_CROSS_PV_EXTENDED lvs -o lv_name,lv_size,seg_count "${TEST_VG}/${LINEAR_LV}"
expect_row LVS_LINEAR_CROSS_PV_EXTENDED_FIELDS /tmp/LVS_LINEAR_CROSS_PV_EXTENDED.out "${LINEAR_LV}|$((LINEAR_CROSS_PV_EXTENDED_MIB * 1024 * 1024))|2"
record_dm_state LINEAR_CROSS_PV_EXTENDED "$(mapper_name "${LINEAR_LV}")"
expect_dm_deps_set LINEAR_CROSS_PV_EXTENDED_DEPS /tmp/LINEAR_CROSS_PV_EXTENDED_DM_DEPS.out "${DEV1}" "${DEV2}"

run_lvm_expect_success LV_REDUCE_LINEAR lvreduce -y -L "${LINEAR_SHRUNK_MIB}M" "${TEST_VG}/${LINEAR_LV}"
run_lvm_report LVS_LINEAR_SHRUNK lvs -o lv_name,lv_size,seg_count,segtype "${TEST_VG}/${LINEAR_LV}"
expect_row LVS_LINEAR_SHRUNK_FIELDS /tmp/LVS_LINEAR_SHRUNK.out "${LINEAR_LV}|$((LINEAR_SHRUNK_MIB * 1024 * 1024))|1|linear"
record_dm_state LINEAR_SHRUNK "$(mapper_name "${LINEAR_LV}")"
expect_dm_deps_set LINEAR_SHRUNK_DEPS /tmp/LINEAR_SHRUNK_DM_DEPS.out "${DEV1}"

step '=== STEP 5: striped LV create grow shrink ==='
run_lvm_expect_success LV_CREATE_STRIPED lvcreate --type striped -i "${STRIPES}" -I "${STRIPE_SIZE_KIB}K" -L "${STRIPED_INITIAL_MIB}M" -n "${STRIPED_LV}" "${TEST_VG}" "${DISK1}" "${DISK2}"
run_lvm_report LVS_STRIPED_INITIAL lvs -o lv_name,lv_size,seg_count "${TEST_VG}/${STRIPED_LV}"
expect_row LVS_STRIPED_INITIAL_FIELDS /tmp/LVS_STRIPED_INITIAL.out "${STRIPED_LV}|$((STRIPED_INITIAL_MIB * 1024 * 1024))|1"
run_lvm_report LVS_SEGMENTS_STRIPED_INITIAL lvs --segments -o lv_name,segtype,stripes,stripesize "${TEST_VG}/${STRIPED_LV}"
expect_row LVS_SEGMENTS_STRIPED_INITIAL_FIELDS /tmp/LVS_SEGMENTS_STRIPED_INITIAL.out "${STRIPED_LV}|striped|${STRIPES}|$((STRIPE_SIZE_KIB * 1024))"
record_dm_state STRIPED_INITIAL "$(mapper_name "${STRIPED_LV}")"
expect_dm_deps_set STRIPED_INITIAL_DEPS /tmp/STRIPED_INITIAL_DM_DEPS.out "${DEV1}" "${DEV2}"

run_lvm_expect_success LV_EXTEND_STRIPED_SAME_SET lvextend -i "${STRIPES}" -I "${STRIPE_SIZE_KIB}K" -L "${STRIPED_SAME_SET_EXTENDED_MIB}M" "${TEST_VG}/${STRIPED_LV}" "${DISK1}" "${DISK2}"
run_lvm_report LVS_STRIPED_SAME_SET_EXTENDED lvs -o lv_name,lv_size,seg_count "${TEST_VG}/${STRIPED_LV}"
expect_row LVS_STRIPED_SAME_SET_EXTENDED_FIELDS /tmp/LVS_STRIPED_SAME_SET_EXTENDED.out "${STRIPED_LV}|$((STRIPED_SAME_SET_EXTENDED_MIB * 1024 * 1024))|1"
run_lvm_report LVS_SEGMENTS_STRIPED_SAME_SET_EXTENDED lvs --segments -o lv_name,segtype,stripes,stripesize "${TEST_VG}/${STRIPED_LV}"
expect_row LVS_SEGMENTS_STRIPED_SAME_SET_EXTENDED_FIELDS /tmp/LVS_SEGMENTS_STRIPED_SAME_SET_EXTENDED.out "${STRIPED_LV}|striped|${STRIPES}|$((STRIPE_SIZE_KIB * 1024))"
record_dm_state STRIPED_SAME_SET_EXTENDED "$(mapper_name "${STRIPED_LV}")"
expect_dm_deps_set STRIPED_SAME_SET_EXTENDED_DEPS /tmp/STRIPED_SAME_SET_EXTENDED_DM_DEPS.out "${DEV1}" "${DEV2}"

run_lvm_expect_success LV_EXTEND_STRIPED_CROSS_SET lvextend -i "${STRIPES}" -I "${STRIPE_SIZE_KIB}K" -L "${STRIPED_CROSS_SET_EXTENDED_MIB}M" "${TEST_VG}/${STRIPED_LV}" "${DISK3}" "${DISK4}"
run_lvm_report LVS_STRIPED_CROSS_SET_EXTENDED lvs -o lv_name,lv_size,seg_count "${TEST_VG}/${STRIPED_LV}"
expect_row LVS_STRIPED_CROSS_SET_EXTENDED_FIELDS /tmp/LVS_STRIPED_CROSS_SET_EXTENDED.out "${STRIPED_LV}|$((STRIPED_CROSS_SET_EXTENDED_MIB * 1024 * 1024))|2"
run_lvm_report LVS_SEGMENTS_STRIPED_CROSS_SET_EXTENDED lvs --segments -o lv_name,segtype,stripes,stripesize "${TEST_VG}/${STRIPED_LV}"
expect_column_set LVS_SEGMENTS_STRIPED_CROSS_SET_TYPES /tmp/LVS_SEGMENTS_STRIPED_CROSS_SET_EXTENDED.out 2 striped
expect_column_set LVS_SEGMENTS_STRIPED_CROSS_SET_STRIPES /tmp/LVS_SEGMENTS_STRIPED_CROSS_SET_EXTENDED.out 3 "${STRIPES}"
expect_column_set LVS_SEGMENTS_STRIPED_CROSS_SET_STRIPE_SIZE /tmp/LVS_SEGMENTS_STRIPED_CROSS_SET_EXTENDED.out 4 "$((STRIPE_SIZE_KIB * 1024))"
record_dm_state STRIPED_CROSS_SET_EXTENDED "$(mapper_name "${STRIPED_LV}")"
expect_dm_deps_set STRIPED_CROSS_SET_EXTENDED_DEPS /tmp/STRIPED_CROSS_SET_EXTENDED_DM_DEPS.out "${DEV1}" "${DEV2}" "${DEV3}" "${DEV4}"

run_lvm_expect_success LV_REDUCE_STRIPED lvreduce -y -L "${STRIPED_SHRUNK_MIB}M" "${TEST_VG}/${STRIPED_LV}"
run_lvm_report LVS_STRIPED_SHRUNK lvs -o lv_name,lv_size,seg_count "${TEST_VG}/${STRIPED_LV}"
expect_row LVS_STRIPED_SHRUNK_FIELDS /tmp/LVS_STRIPED_SHRUNK.out "${STRIPED_LV}|$((STRIPED_SHRUNK_MIB * 1024 * 1024))|1"
run_lvm_report LVS_SEGMENTS_STRIPED_SHRUNK lvs --segments -o lv_name,segtype,stripes,stripesize "${TEST_VG}/${STRIPED_LV}"
expect_row LVS_SEGMENTS_STRIPED_SHRUNK_FIELDS /tmp/LVS_SEGMENTS_STRIPED_SHRUNK.out "${STRIPED_LV}|striped|${STRIPES}|$((STRIPE_SIZE_KIB * 1024))"
record_dm_state STRIPED_SHRUNK "$(mapper_name "${STRIPED_LV}")"
expect_dm_deps_set STRIPED_SHRUNK_DEPS /tmp/STRIPED_SHRUNK_DM_DEPS.out "${DEV1}" "${DEV2}"

step '=== STEP 6: mixed linear plus striped LV ==='
run_lvm_expect_success LV_CREATE_MIXED_LINEAR lvcreate --type linear -L "${MIXED_INITIAL_MIB}M" -n "${MIXED_LV}" "${TEST_VG}" "${DISK4}"
run_lvm_expect_success LV_EXTEND_MIXED_STRIPED lvextend --type striped -i "${STRIPES}" -I "${STRIPE_SIZE_KIB}K" -L "${MIXED_EXTENDED_MIB}M" "${TEST_VG}/${MIXED_LV}" "${DISK1}" "${DISK2}"
run_lvm_report LVS_MIXED lvs -o lv_name,lv_size,seg_count "${TEST_VG}/${MIXED_LV}"
expect_row LVS_MIXED_FIELDS /tmp/LVS_MIXED.out "${MIXED_LV}|$((MIXED_EXTENDED_MIB * 1024 * 1024))|2"
run_lvm_report LVS_SEGMENTS_MIXED lvs --segments -o lv_name,segtype,stripes,stripesize "${TEST_VG}/${MIXED_LV}"
expect_column_set LVS_SEGMENTS_MIXED_TYPES /tmp/LVS_SEGMENTS_MIXED.out 2 linear striped
expect_row LVS_SEGMENTS_MIXED_STRIPED_FIELDS /tmp/LVS_SEGMENTS_MIXED.out "${MIXED_LV}|striped|${STRIPES}|$((STRIPE_SIZE_KIB * 1024))"
record_dm_state MIXED "$(mapper_name "${MIXED_LV}")"
expect_dm_deps_set MIXED_DEPS /tmp/MIXED_DM_DEPS.out "${DEV1}" "${DEV2}" "${DEV4}"
run_lvm_report VGS_WITH_LVS vgs -o vg_name,pv_count,lv_count
expect_row VGS_WITH_LVS_COUNTS /tmp/VGS_WITH_LVS.out "${TEST_VG}|4|3"

step '=== STEP 7: activation scan mknodes ==='
run_lvm_expect_success VGCHANGE_INACTIVE vgchange -an "${TEST_VG}"
run_lvm_expect_success PVSCAN_RECOVERY pvscan
run_lvm_expect_success VGSCAN_MKNODES vgscan --mknodes
run_lvm_expect_success VGCHANGE_ACTIVE vgchange -ay "${TEST_VG}"
run_lvm_report LVS_AFTER_REACTIVATE lvs -o lv_name,lv_size,seg_count,lv_active "${TEST_VG}"
expect_row LVS_AFTER_REACTIVATE_LINEAR /tmp/LVS_AFTER_REACTIVATE.out "${LINEAR_LV}|$((LINEAR_SHRUNK_MIB * 1024 * 1024))|1|active"
expect_row LVS_AFTER_REACTIVATE_STRIPED /tmp/LVS_AFTER_REACTIVATE.out "${STRIPED_LV}|$((STRIPED_SHRUNK_MIB * 1024 * 1024))|1|active"
expect_row LVS_AFTER_REACTIVATE_MIXED /tmp/LVS_AFTER_REACTIVATE.out "${MIXED_LV}|$((MIXED_EXTENDED_MIB * 1024 * 1024))|2|active"
record_dm_state LINEAR_AFTER_REACTIVATE "$(mapper_name "${LINEAR_LV}")"
expect_dm_deps_set LINEAR_AFTER_REACTIVATE_DEPS /tmp/LINEAR_AFTER_REACTIVATE_DM_DEPS.out "${DEV1}"
record_dm_state STRIPED_AFTER_REACTIVATE "$(mapper_name "${STRIPED_LV}")"
expect_dm_deps_set STRIPED_AFTER_REACTIVATE_DEPS /tmp/STRIPED_AFTER_REACTIVATE_DM_DEPS.out "${DEV1}" "${DEV2}"
record_dm_state MIXED_AFTER_REACTIVATE "$(mapper_name "${MIXED_LV}")"
expect_dm_deps_set MIXED_AFTER_REACTIVATE_DEPS /tmp/MIXED_AFTER_REACTIVATE_DM_DEPS.out "${DEV1}" "${DEV2}" "${DEV4}"

step '=== STEP 8: test-object remove lifecycle ==='
run_lvm_expect_success LVREMOVE_MIXED lvremove -y "${TEST_VG}/${MIXED_LV}"
expect_mapper_absent AFTER_LVREMOVE_MIXED "${MIXED_LV}"
run_lvm_expect_success LVREMOVE_STRIPED lvremove -y "${TEST_VG}/${STRIPED_LV}"
expect_mapper_absent AFTER_LVREMOVE_STRIPED "${STRIPED_LV}"
run_lvm_expect_success LVREMOVE_LINEAR lvremove -y "${TEST_VG}/${LINEAR_LV}"
expect_mapper_absent AFTER_LVREMOVE_LINEAR "${LINEAR_LV}"
run_lvm_report LVS_AFTER_LVREMOVE lvs -o lv_name,vg_name "${TEST_VG}"
expect_empty_report LVS_AFTER_LVREMOVE_EMPTY /tmp/LVS_AFTER_LVREMOVE.out
run_lvm_report VGS_AFTER_LVREMOVE vgs -o vg_name,pv_count,lv_count
expect_row VGS_AFTER_LVREMOVE_COUNTS /tmp/VGS_AFTER_LVREMOVE.out "${TEST_VG}|4|0"
run_lvm_report PVS_AFTER_LVREMOVE pvs -o pv_name,vg_name
expect_row PVS_AFTER_LVREMOVE_DISK1 /tmp/PVS_AFTER_LVREMOVE.out "${DISK1}|${TEST_VG}"
expect_row PVS_AFTER_LVREMOVE_DISK2 /tmp/PVS_AFTER_LVREMOVE.out "${DISK2}|${TEST_VG}"
expect_row PVS_AFTER_LVREMOVE_DISK3 /tmp/PVS_AFTER_LVREMOVE.out "${DISK3}|${TEST_VG}"
expect_row PVS_AFTER_LVREMOVE_DISK4 /tmp/PVS_AFTER_LVREMOVE.out "${DISK4}|${TEST_VG}"
run_lvm_expect_success VGREMOVE_TEST vgremove -y "${TEST_VG}"
run_lvm_report VGS_AFTER_VGREMOVE vgs -o vg_name,pv_count,lv_count
expect_column_set VGS_AFTER_VGREMOVE_NAMES /tmp/VGS_AFTER_VGREMOVE.out 1
run_lvm_report PVS_AFTER_VGREMOVE pvs -o pv_name,vg_name
expect_column_set PVS_AFTER_VGREMOVE_NAMES /tmp/PVS_AFTER_VGREMOVE.out 1 "${DISK1}" "${DISK2}" "${DISK3}" "${DISK4}"
expect_row PVS_AFTER_VGREMOVE_DISK1 /tmp/PVS_AFTER_VGREMOVE.out "${DISK1}|"
expect_row PVS_AFTER_VGREMOVE_DISK2 /tmp/PVS_AFTER_VGREMOVE.out "${DISK2}|"
expect_row PVS_AFTER_VGREMOVE_DISK3 /tmp/PVS_AFTER_VGREMOVE.out "${DISK3}|"
expect_row PVS_AFTER_VGREMOVE_DISK4 /tmp/PVS_AFTER_VGREMOVE.out "${DISK4}|"
run_lvm_expect_success PVREMOVE_TEST pvremove -ff -y "${DISK1}" "${DISK2}" "${DISK3}" "${DISK4}"
run_lvm_report PVS_AFTER_PVREMOVE pvs -o pv_name,vg_name
expect_empty_report PVS_AFTER_PVREMOVE_EMPTY /tmp/PVS_AFTER_PVREMOVE.out
expect_mapper_absent FINAL_MIXED "${MIXED_LV}"
expect_mapper_absent FINAL_STRIPED "${STRIPED_LV}"
expect_mapper_absent FINAL_LINEAR "${LINEAR_LV}"

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
