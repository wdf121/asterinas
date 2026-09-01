#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -uo pipefail
export LC_ALL=C

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/run_lvm2_linux_cli_baseline.sh [--preflight]

Runs a host Linux/OpenEuler LVM2 CLI semantic baseline for the LVM2 command
subset currently exercised by Asterinas Device Mapper tests. The script only
uses temporary loop backing files and test VG/LV names with a unique prefix.

Safety rules:
  - never uses real /dev/sd*, /dev/nvme*, /dev/vd*, or /dev/mapper/* as PVs;
  - never removes objects outside the current run manifest;
  - fails if LVM device filtering cannot hide non-test host PVs;
  - fails if objects with the chosen prefix already exist.

Environment variables:
  LVM2_BASELINE_PREFIX       Test object prefix, default aster_lvm2_sem_<pid>
  LVM2_BASELINE_LOOP_SIZE    Backing file size per loop, default 256M
  LVM2_BASELINE_LOOP_COUNT   Number of loop devices, default 4
EOF
    exit 0
fi

PREFLIGHT=0
if [ "${1:-}" = "--preflight" ]; then
    PREFLIGHT=1
elif [ "$#" -ne 0 ]; then
    echo "Usage: $0 [--preflight]" >&2
    exit 2
fi

PREFIX=${LVM2_BASELINE_PREFIX:-aster_lvm2_sem_$$}
LOOP_SIZE=${LVM2_BASELINE_LOOP_SIZE:-256M}
LOOP_COUNT=${LVM2_BASELINE_LOOP_COUNT:-4}
TMPDIR_PATH=
LVM_CONFIG=
LOOPS_CSV=
DEV_LIST=
LVM_ARGS=()
BACKING_FILES=()
LOOPS=()
DEVS=()
TEST_VG="${PREFIX}_vg"
LINEAR_LV="${PREFIX}_linear_lv"
STRIPED_LV="${PREFIX}_striped_lv"
MIXED_LV="${PREFIX}_mixed_lv"

fail() {
    echo "BASELINE_FAIL_LVM2_LINUX_CLI_BASELINE $*" >&2
    exit 1
}

require_tool() {
    command -v "$1" >/dev/null 2>&1 || fail "missing_tool=$1"
}

normalize_line() {
    local line=$1 loop dev backing
    line=${line//${PREFIX}/NAME}
    if [ -n "${TMPDIR_PATH:-}" ]; then
        line=${line//${TMPDIR_PATH}/<TMPDIR>}
    fi
    if [ -n "${LVM_CONFIG:-}" ]; then
        line=${line//${LVM_CONFIG}/<LVM_CONFIG>}
    fi
    for loop in "${LOOPS[@]:-}"; do
        [ -n "${loop}" ] && line=${line//${loop}/<LOOP>}
    done
    for backing in "${BACKING_FILES[@]:-}"; do
        [ -n "${backing}" ] && line=${line//${backing}/<BACKING>}
    done
    for dev in "${DEVS[@]:-}"; do
        [ -n "${dev}" ] && line=${line//${dev}/<DEV>}
    done
    printf '%s\n' "${line}" | sed -E 's#/dev/dm-[0-9]+#/dev/dm-<N>#g; s/[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}/<UUID>/g; s/[[:space:]]+$//'
}

print_stream() {
    local prefix=$1
    local label=$2
    local file=$3
    local line
    if [ ! -s "${file}" ]; then
        echo "${prefix}_${label}: <empty>"
        return
    fi
    while IFS= read -r line || [ -n "${line}" ]; do
        printf '%s_%s: %s\n' "${prefix}" "${label}" "$(normalize_line "${line}")"
    done <"${file}"
}

run_capture() {
    local label=$1
    shift
    local out err status start_ts end_ts
    out=$(mktemp "${TMPDIR_PATH}/stdout.${label}.XXXXXX")
    err=$(mktemp "${TMPDIR_PATH}/stderr.${label}.XXXXXX")
    start_ts=$(date +%s)
    echo "SCENARIO_BEGIN_${label}"
    echo "CMD_${label}: $(normalize_line "$*")"
    "$@" >"${out}" 2>"${err}"
    status=$?
    end_ts=$(date +%s)
    echo "STATUS_${label}: ${status}"
    echo "DURATION_${label}: $((end_ts - start_ts))s"
    print_stream STDOUT "${label}" "${out}"
    print_stream STDERR "${label}" "${err}"
    echo "SCENARIO_END_${label}"
    return "${status}"
}

run_lvm() {
    local label=$1
    shift
    run_capture "${label}" "$@" "${LVM_ARGS[@]}"
}

snapshot_lvm() {
    local file=$1
    {
        echo "PVS_BEGIN"
        pvs --noheadings --separator '|' -o pv_name,vg_name,pv_uuid 2>/dev/null | sed 's/[[:space:]]\+/ /g' | sort || true
        echo "PVS_END"
        echo "VGS_BEGIN"
        vgs --noheadings --separator '|' -o vg_name,vg_uuid 2>/dev/null | sed 's/[[:space:]]\+/ /g' | sort || true
        echo "VGS_END"
        echo "LVS_BEGIN"
        lvs --noheadings --separator '|' -o vg_name,lv_name,lv_uuid 2>/dev/null | sed 's/[[:space:]]\+/ /g' | sort || true
        echo "LVS_END"
        echo "DM_BEGIN"
        dmsetup ls 2>/dev/null | sort || true
        echo "DM_END"
    } >"${file}"
}

existing_prefixed_objects() {
    {
        vgs --noheadings -o vg_name 2>/dev/null | awk '{print "vg:" $1}' || true
        lvs --noheadings --separator '/' -o vg_name,lv_name 2>/dev/null | awk 'NF {gsub(/[[:space:]]/, ""); print "lv:" $0}' || true
        dmsetup ls 2>/dev/null | awk 'NF >= 1 && $1 != "No" {print "dm:" $1}' || true
    } | awk -v prefix="${PREFIX}" 'index($0, prefix) > 0 {print}'
}

safe_name() {
    case "$1" in
        ${PREFIX}_*) return 0 ;;
        *) return 1 ;;
    esac
}

quiet_lvremove() {
    local lv=$1
    safe_name "${lv}" || return 0
    lvremove "${LVM_ARGS[@]}" -y "${TEST_VG}/${lv}" >/dev/null 2>&1 || true
}

quiet_vgremove() {
    safe_name "${TEST_VG}" || return 0
    vgchange "${LVM_ARGS[@]}" -an "${TEST_VG}" >/dev/null 2>&1 || true
    vgremove "${LVM_ARGS[@]}" -y "${TEST_VG}" >/dev/null 2>&1 || true
}

quiet_pvremove_loops() {
    local loop
    for loop in "${LOOPS[@]:-}"; do
        case "${loop}" in
            /dev/loop*) pvremove "${LVM_ARGS[@]}" -ff -y "${loop}" >/dev/null 2>&1 || true ;;
        esac
    done
}

cleanup() {
    quiet_lvremove "${MIXED_LV}"
    quiet_lvremove "${STRIPED_LV}"
    quiet_lvremove "${LINEAR_LV}"
    quiet_vgremove
    quiet_pvremove_loops
    local loop
    for loop in "${LOOPS[@]:-}"; do
        case "${loop}" in
            /dev/loop*) losetup -d "${loop}" >/dev/null 2>&1 || true ;;
        esac
    done
    if [ -n "${TMPDIR_PATH:-}" ]; then
        rm -rf "${TMPDIR_PATH}"
    fi
}

trap cleanup EXIT

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

mapper_name() {
    printf '%s-%s' "${TEST_VG}" "$1"
}

build_lvm_config() {
    local filters=() loop regex joined
    for loop in "${LOOPS[@]}"; do
        regex=$(printf '%s' "${loop}" | sed 's/[].[^$*+?{}|()\\]/\\&/g')
        filters+=("\"a|^${regex}$|\"")
    done
    filters+=("\"r|.*|\"")
    joined=$(IFS=,; printf '%s' "${filters[*]}")
    LVM_CONFIG="devices { use_devicesfile = 0 filter = [ ${joined} ] global_filter = [ ${joined} ] } activation { udev_rules = 0 udev_sync = 0 }"
    LOOPS_CSV=$(IFS=,; printf '%s' "${LOOPS[*]}")
    LVM_ARGS=(--config "${LVM_CONFIG}")
    if pvs --config "${LVM_CONFIG}" --devices "${LOOPS_CSV}" --noheadings -o pv_name >/dev/null 2>&1; then
        LVM_ARGS+=(--devices "${LOOPS_CSV}")
    fi
}

verify_lvm_filter() {
    local visible
    visible=$(pvs "${LVM_ARGS[@]}" --noheadings -o pv_name 2>/dev/null | awk 'NF {print $1}')
    if [ -n "${visible}" ]; then
        echo "BASELINE_FAIL filter_visible_existing_pvs_begin" >&2
        printf '%s\n' "${visible}" >&2
        echo "BASELINE_FAIL filter_visible_existing_pvs_end" >&2
        return 1
    fi
    return 0
}

record_dm_state() {
    local label=$1
    local mapper=$2
    run_capture "${label}_DM_TABLE" dmsetup table "${mapper}" || true
    run_capture "${label}_DM_STATUS" dmsetup status "${mapper}" || true
    run_capture "${label}_DM_DEPS" dmsetup deps "${mapper}" || true
}

[ "$(id -u)" -eq 0 ] || fail "must_run_as_root"
[ "${LOOP_COUNT}" -ge 4 ] || fail "loop_count_must_be_at_least_4"
require_tool awk
require_tool date
require_tool diff
require_tool dmsetup
require_tool grep
require_tool losetup
require_tool lvcreate
require_tool lvextend
require_tool lvreduce
require_tool lvremove
require_tool lvs
require_tool mktemp
require_tool pvcreate
require_tool pvremove
require_tool pvs
require_tool pvscan
require_tool sed
require_tool sort
require_tool stat
require_tool truncate
require_tool vgchange
require_tool vgcreate
require_tool vgextend
require_tool vgremove
require_tool vgs
require_tool vgscan

prefixed=$(existing_prefixed_objects)
if [ -n "${prefixed}" ]; then
    echo "BASELINE_FAIL existing_prefixed_lvm_or_dm_objects_begin" >&2
    printf '%s\n' "${prefixed}" >&2
    echo "BASELINE_FAIL existing_prefixed_lvm_or_dm_objects_end" >&2
    fail "existing_prefixed_objects prefix=${PREFIX}"
fi

TMPDIR_PATH=$(mktemp -d /tmp/lvm2-linux-baseline.XXXXXX)
snapshot_lvm "${TMPDIR_PATH}/snapshot.before"

for index in $(seq 1 "${LOOP_COUNT}"); do
    backing="${TMPDIR_PATH}/backing${index}.img"
    truncate -s "${LOOP_SIZE}" "${backing}" || fail "create_backing${index}"
    loop=$(losetup --find --show "${backing}") || fail "setup_loop${index}"
    BACKING_FILES+=("${backing}")
    LOOPS+=("${loop}")
    DEVS+=("$(devno "${loop}")")
done

build_lvm_config
verify_lvm_filter || fail "lvm_filter_not_isolated_to_test_loops"

DEV_LIST=$(IFS=,; printf '%s' "${DEVS[*]}")
echo "BASELINE_INFO_LVM2 prefix=${PREFIX}"
echo "BASELINE_INFO_LVM2 tmpdir=<TMPDIR>"
echo "BASELINE_INFO_LVM2 loop_count=${LOOP_COUNT} loop_size=${LOOP_SIZE}"
echo "BASELINE_INFO_LVM2 loops=$(normalize_line "${LOOPS[*]}")"
echo "BASELINE_INFO_LVM2 devs=$(normalize_line "${DEV_LIST}")"
echo "BASELINE_INFO_LVM2 devices_arg=$([ "${#LVM_ARGS[@]}" -gt 2 ] && echo enabled || echo disabled)"

if [ "${PREFLIGHT}" -eq 1 ]; then
    echo "PREFLIGHT_PASS_LVM2_LINUX_CLI_BASELINE"
    exit 0
fi

LINEAR_INITIAL_MIB=${LVM2_LINEAR_INITIAL_MIB:-32}
LINEAR_SAME_PV_EXTENDED_MIB=${LVM2_LINEAR_SAME_PV_EXTENDED_MIB:-64}
LINEAR_CROSS_PV_EXTENDED_MIB=${LVM2_LINEAR_CROSS_PV_EXTENDED_MIB:-96}
LINEAR_SHRUNK_MIB=${LVM2_LINEAR_SHRUNK_MIB:-32}
STRIPED_INITIAL_MIB=${LVM2_STRIPED_INITIAL_MIB:-32}
STRIPED_SAME_SET_EXTENDED_MIB=${LVM2_STRIPED_SAME_SET_EXTENDED_MIB:-64}
STRIPED_CROSS_SET_EXTENDED_MIB=${LVM2_STRIPED_CROSS_SET_EXTENDED_MIB:-96}
STRIPED_SHRUNK_MIB=${LVM2_STRIPED_SHRUNK_MIB:-32}
MIXED_INITIAL_MIB=${LVM2_MIXED_INITIAL_MIB:-32}
MIXED_EXTENDED_MIB=${LVM2_MIXED_EXTENDED_MIB:-64}
STRIPES=${LVM2_STRIPES:-2}
STRIPE_SIZE_KIB=${LVM2_STRIPE_SIZE_KIB:-64}

run_lvm STATIC_PVS pvs -o pv_name,vg_name,pv_size || true
run_lvm STATIC_VGS vgs -o vg_name,pv_count,lv_count,vg_size,vg_free || true
run_lvm STATIC_LVS lvs -a -o vg_name,lv_name,lv_size,seg_count,devices || true

run_lvm PV_CREATE pvcreate -ff -y "${LOOPS[@]}" || true
run_lvm PVS_AFTER_PVCREATE pvs -o pv_name,pv_size,vg_name || true
run_lvm PVSCAN_AFTER_PVCREATE pvscan || true

run_lvm VG_CREATE vgcreate "${TEST_VG}" "${LOOPS[0]}" "${LOOPS[1]}" || true
run_lvm VGS_AFTER_VGCREATE vgs -o vg_name,vg_size,vg_free,pv_count,lv_count || true
run_lvm VG_EXTEND vgextend "${TEST_VG}" "${LOOPS[2]}" "${LOOPS[3]}" || true
run_lvm VGS_AFTER_VGEXTEND vgs -o vg_name,vg_size,vg_free,pv_count,lv_count || true

run_lvm LV_CREATE_LINEAR lvcreate --type linear -L "${LINEAR_INITIAL_MIB}M" -n "${LINEAR_LV}" "${TEST_VG}" "${LOOPS[0]}" || true
run_lvm LVS_LINEAR_INITIAL lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}" || true
run_lvm LVS_SEGMENTS_LINEAR_INITIAL lvs --segments -o lv_name,seg_start,seg_size,segtype,devices "${TEST_VG}/${LINEAR_LV}" || true
record_dm_state LINEAR_INITIAL "$(mapper_name "${LINEAR_LV}")"

run_lvm LV_EXTEND_LINEAR_SAME_PV lvextend -L "${LINEAR_SAME_PV_EXTENDED_MIB}M" "${TEST_VG}/${LINEAR_LV}" "${LOOPS[0]}" || true
run_lvm LVS_LINEAR_SAME_PV_EXTENDED lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}" || true
run_lvm LVS_SEGMENTS_LINEAR_SAME_PV_EXTENDED lvs --segments -o lv_name,seg_start,seg_size,segtype,devices "${TEST_VG}/${LINEAR_LV}" || true
record_dm_state LINEAR_SAME_PV_EXTENDED "$(mapper_name "${LINEAR_LV}")"

run_lvm LV_EXTEND_LINEAR_CROSS_PV lvextend -L "${LINEAR_CROSS_PV_EXTENDED_MIB}M" "${TEST_VG}/${LINEAR_LV}" "${LOOPS[1]}" || true
run_lvm LVS_LINEAR_CROSS_PV_EXTENDED lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}" || true
run_lvm LVS_SEGMENTS_LINEAR_CROSS_PV_EXTENDED lvs --segments -o lv_name,seg_start,seg_size,segtype,devices "${TEST_VG}/${LINEAR_LV}" || true
record_dm_state LINEAR_CROSS_PV_EXTENDED "$(mapper_name "${LINEAR_LV}")"

run_lvm LV_REDUCE_LINEAR lvreduce -y -L "${LINEAR_SHRUNK_MIB}M" "${TEST_VG}/${LINEAR_LV}" || true
run_lvm LVS_LINEAR_SHRUNK lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}" || true
run_lvm LVS_SEGMENTS_LINEAR_SHRUNK lvs --segments -o lv_name,seg_start,seg_size,segtype,devices "${TEST_VG}/${LINEAR_LV}" || true
record_dm_state LINEAR_SHRUNK "$(mapper_name "${LINEAR_LV}")"

run_lvm LV_CREATE_STRIPED lvcreate --type striped -i "${STRIPES}" -I "${STRIPE_SIZE_KIB}K" -L "${STRIPED_INITIAL_MIB}M" -n "${STRIPED_LV}" "${TEST_VG}" "${LOOPS[0]}" "${LOOPS[1]}" || true
run_lvm LVS_STRIPED_INITIAL lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}" || true
run_lvm LVS_SEGMENTS_STRIPED_INITIAL lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices "${TEST_VG}/${STRIPED_LV}" || true
record_dm_state STRIPED_INITIAL "$(mapper_name "${STRIPED_LV}")"

run_lvm LV_EXTEND_STRIPED_SAME_SET lvextend -i "${STRIPES}" -I "${STRIPE_SIZE_KIB}K" -L "${STRIPED_SAME_SET_EXTENDED_MIB}M" "${TEST_VG}/${STRIPED_LV}" "${LOOPS[0]}" "${LOOPS[1]}" || true
run_lvm LVS_STRIPED_SAME_SET_EXTENDED lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}" || true
run_lvm LVS_SEGMENTS_STRIPED_SAME_SET_EXTENDED lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices "${TEST_VG}/${STRIPED_LV}" || true
record_dm_state STRIPED_SAME_SET_EXTENDED "$(mapper_name "${STRIPED_LV}")"

run_lvm LV_EXTEND_STRIPED_CROSS_SET lvextend -i "${STRIPES}" -I "${STRIPE_SIZE_KIB}K" -L "${STRIPED_CROSS_SET_EXTENDED_MIB}M" "${TEST_VG}/${STRIPED_LV}" "${LOOPS[2]}" "${LOOPS[3]}" || true
run_lvm LVS_STRIPED_CROSS_SET_EXTENDED lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}" || true
run_lvm LVS_SEGMENTS_STRIPED_CROSS_SET_EXTENDED lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices "${TEST_VG}/${STRIPED_LV}" || true
record_dm_state STRIPED_CROSS_SET_EXTENDED "$(mapper_name "${STRIPED_LV}")"

run_lvm LV_REDUCE_STRIPED lvreduce -y -L "${STRIPED_SHRUNK_MIB}M" "${TEST_VG}/${STRIPED_LV}" || true
run_lvm LVS_STRIPED_SHRUNK lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}" || true
run_lvm LVS_SEGMENTS_STRIPED_SHRUNK lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices "${TEST_VG}/${STRIPED_LV}" || true
record_dm_state STRIPED_SHRUNK "$(mapper_name "${STRIPED_LV}")"

run_lvm LV_CREATE_MIXED_LINEAR lvcreate --type linear -L "${MIXED_INITIAL_MIB}M" -n "${MIXED_LV}" "${TEST_VG}" "${LOOPS[3]}" || true
run_lvm LV_EXTEND_MIXED_STRIPED lvextend --type striped -i "${STRIPES}" -I "${STRIPE_SIZE_KIB}K" -L "${MIXED_EXTENDED_MIB}M" "${TEST_VG}/${MIXED_LV}" "${LOOPS[1]}" "${LOOPS[2]}" || true
run_lvm LVS_MIXED lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}" || true
run_lvm LVS_SEGMENTS_MIXED lvs --segments -o lv_name,seg_start,seg_size,segtype,stripes,stripesize,devices "${TEST_VG}/${MIXED_LV}" || true
record_dm_state MIXED "$(mapper_name "${MIXED_LV}")"

run_lvm VGCHANGE_INACTIVE vgchange -an "${TEST_VG}" || true
run_capture DM_LS_AFTER_VGCHANGE_INACTIVE dmsetup ls || true
run_lvm PVSCAN_RECOVERY pvscan || true
run_lvm VGSCAN_MKNODES vgscan --mknodes || true
run_lvm VGCHANGE_ACTIVE vgchange -ay "${TEST_VG}" || true
run_lvm LVS_AFTER_REACTIVATE lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}" || true
record_dm_state LINEAR_AFTER_REACTIVATE "$(mapper_name "${LINEAR_LV}")"

run_lvm LVREMOVE_MIXED lvremove -y "${TEST_VG}/${MIXED_LV}" || true
run_lvm LVREMOVE_STRIPED lvremove -y "${TEST_VG}/${STRIPED_LV}" || true
run_lvm LVREMOVE_LINEAR lvremove -y "${TEST_VG}/${LINEAR_LV}" || true
run_lvm LVS_AFTER_LVREMOVE lvs -a -o vg_name,lv_name,lv_size,seg_count,devices "${TEST_VG}" || true
run_lvm VGREMOVE_TEST vgremove -y "${TEST_VG}" || true
run_lvm VGS_AFTER_VGREMOVE vgs -o vg_name,pv_count,lv_count || true
run_lvm PVREMOVE_TEST pvremove -ff -y "${LOOPS[@]}" || true
run_lvm PVS_AFTER_PVREMOVE pvs -o pv_name,pv_size,vg_name || true

snapshot_lvm "${TMPDIR_PATH}/snapshot.after"
if ! diff -u "${TMPDIR_PATH}/snapshot.before" "${TMPDIR_PATH}/snapshot.after" >/dev/null; then
    echo "BASELINE_FAIL_LVM2_LINUX_CLI_BASELINE non_test_lvm_snapshot_changed" >&2
    diff -u "${TMPDIR_PATH}/snapshot.before" "${TMPDIR_PATH}/snapshot.after" | sed 's/^/BASELINE_SNAPSHOT_DIFF /' >&2 || true
    exit 1
fi

echo "SUMMARY_GAP_LVM2_LINUX_BASELINE: 0"
echo "BASELINE_PASS_LVM2_LINUX_CLI_BASELINE"
