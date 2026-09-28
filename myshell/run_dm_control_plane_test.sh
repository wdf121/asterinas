#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/run_dm_control_plane_test.sh

Runs one NixOS guest pass for Device Mapper control-plane semantics. It covers
dmsetup discovery, target metadata, table lifecycle, event handling, rename,
read-only devices, busy removal, and cleanup behavior.

Optional environment variables:
  DM_TEST_IMAGE              First backing test image path, default target/nixos/test.img
  DM_TEST_IMAGE_2            Second backing test image path, default target/nixos/test2.img
  DM_CONTROL_PLANE_LOG            Host-side log path, default /tmp/dm-control-plane-test.log
  GUEST_QEMU_TIMEOUT         Full QEMU lifecycle timeout in seconds, default 180
  GUEST_READY_TIMEOUT        Guest shell readiness timeout in seconds, default 40
  RESET_DM_TEST_IMAGES       1 to delete test images before running, default 1

Expected success markers:
  SUMMARY_GAP_DM_CONTROL_PLANE: 0
  TEST_PASS_DM_CONTROL_PLANE
  HOST_PASS_DM_CONTROL_PLANE
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/..")
source "${SCRIPT_DIR}/lib/dm_nixos_test.sh"

TEST_ID=DM_CONTROL_PLANE
LOG=${DM_CONTROL_PLANE_LOG:-/tmp/dm-control-plane-test.log}
DM_TEST_IMAGE=${DM_TEST_IMAGE:-target/nixos/test.img}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-target/nixos/test2.img}
GUEST_QEMU_TIMEOUT=${GUEST_QEMU_TIMEOUT:-180}
GUEST_READY_TIMEOUT=${GUEST_READY_TIMEOUT:-40}
GUEST_INPUT_LINE_DELAY=${GUEST_INPUT_LINE_DELAY:-0.01}
RESET_DM_TEST_IMAGES=${RESET_DM_TEST_IMAGES:-1}

dm_init_log "${TEST_ID}"

cd "${ASTERINAS_DIR}"
dm_prepare_nixos_test "${TEST_ID}"
dm_emit "HOST_INFO_${TEST_ID} disk1=${DM_TEST_IMAGE} serial=vdmtest"
dm_emit "HOST_INFO_${TEST_ID} disk2=${DM_TEST_IMAGE_2} serial=vdmtest2"
dm_emit "HOST_INFO_${TEST_ID} qemu_lifecycle_timeout=${GUEST_QEMU_TIMEOUT}s"

GUEST_SCRIPT_FILE=$(mktemp /tmp/dm-control-plane-guest.XXXXXX)
cat >"${GUEST_SCRIPT_FILE}" <<'GUEST_SCRIPT'
stty -echo 2>/dev/null || true
cat >/tmp/dm_control_plane_guest.sh <<'DMSETUP_GUEST_BODY'
set -u

PREFIX=dm_control
names='stdin notable first_publish first_rename first_renamed first_remove linear_major linear_path discovery striped_major striped_path error zero table_lifecycle state_event rename rename_existing renamed readonly busy busy_other deferred_busy remove_active remove_tableless remove_all_a remove_all_b'
GUEST_TEST_START=$(date +%s)
STEP_START=${GUEST_TEST_START}
STEP_LABEL=START
GAP_COUNT=0
CMD_TIMEOUT=${DMSETUP_CMD_TIMEOUT:-5}

now_s() {
    date +%s
}

elapsed_since() {
    start=$1
    echo "$(( $(now_s) - start ))"
}

name() {
    printf '%s_%s' "${PREFIX}" "$1"
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
    echo "GUEST_DURATION_DM_CONTROL_PLANE: $((now - GUEST_TEST_START))s"
    echo "SUMMARY_GAP_DM_CONTROL_PLANE: ${GAP_COUNT}"
}

observe_gap() {
    GAP_COUNT=$((GAP_COUNT + 1))
    echo "OBSERVE_GAP_DM_CONTROL_PLANE $*"
}

fail_precondition() {
    echo "TEST_FAIL_DM_CONTROL_PLANE $*"
    exit 1
}

cleanup_dm() {
    for suffix in ${names}; do
        timeout 3 dmsetup remove "$(name "${suffix}")" >/dev/null 2>&1 || true
    done
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
    if [ "${1:-}" = "timeout" ]; then
        "$@" >"${out}" 2>"${err}"
    else
        timeout "${CMD_TIMEOUT}" "$@" >"${out}" 2>"${err}"
    fi
    status=$?
    ended=$(now_s)
    echo "STATUS_${label}: ${status}"
    echo "DURATION_${label}: $((ended - started))s"
    print_stream STDOUT "${label}" "${out}"
    print_stream STDERR "${label}" "${err}"
    echo "SCENARIO_END_${label}"
    return "${status}"
}

run_shell_capture() {
    label=$1
    script=$2
    out="/tmp/${label}.out"
    err="/tmp/${label}.err"
    started=$(now_s)
    echo "SCENARIO_BEGIN_${label}"
    echo "CMD_${label}: ${script}"
    timeout "${CMD_TIMEOUT}" sh -c "${script}" >"${out}" 2>"${err}"
    status=$?
    ended=$(now_s)
    echo "STATUS_${label}: ${status}"
    echo "DURATION_${label}: $((ended - started))s"
    print_stream STDOUT "${label}" "${out}"
    print_stream STDERR "${label}" "${err}"
    echo "SCENARIO_END_${label}"
    return "${status}"
}

run_expect_success() {
    label=$1
    shift
    if ! run_capture "${label}" "$@"; then
        observe_gap "${label}_failed"
    fi
    return 0
}

run_expect_failure() {
    label=$1
    shift
    status=0
    run_capture "${label}" "$@" || status=$?
    if [ "${status}" -eq 0 ]; then
        observe_gap "${label}_unexpected_success"
    elif [ "${status}" -eq 124 ]; then
        observe_gap "${label}_timed_out"
    fi
    return 0
}

run_shell_expect_success() {
    label=$1
    script=$2
    if ! run_shell_capture "${label}" "${script}"; then
        observe_gap "${label}_failed"
    fi
    return 0
}

run_shell_expect_failure() {
    label=$1
    script=$2
    status=0
    run_shell_capture "${label}" "${script}" || status=$?
    if [ "${status}" -eq 0 ]; then
        observe_gap "${label}_unexpected_success"
    elif [ "${status}" -eq 124 ]; then
        observe_gap "${label}_timed_out"
    fi
    return 0
}

run_expect_status() {
    expected=$1
    label=$2
    shift 2
    status=0
    run_capture "${label}" "$@" || status=$?
    if [ "${status}" -ne "${expected}" ]; then
        observe_gap "${label}_status_${status}_expected_${expected}"
    fi
    return 0
}

grep_expect() {
    label=$1
    pattern=$2
    file=$3
    if ! timeout 3 grep -q -- "${pattern}" "${file}"; then
        observe_gap "${label}_grep_failed"
    fi
    return 0
}

devno() {
    printf '%d:%d' "0x$(stat -c '%t' "$1")" "0x$(stat -c '%T' "$1")"
}

event_number() {
    timeout 3 dmsetup info "$1" 2>/dev/null | awk -F: '/Event number/ { gsub(/[[:space:]]/, "", $2); print $2; exit }'
}

capture_info_columns() {
    label=$1
    shift
    run_expect_success "${label}" env LC_ALL=C dmsetup info -C --noheadings --separator '|' \
        -o name,uuid,major,minor,open,segments,events,tables_loaded,suspended,readonly "$@"
}

info_column() {
    label=$1
    field_number=$2
    awk -F'|' -v field_number="${field_number}" '
        NR == 1 {
            value = $field_number
            gsub(/^[[:space:]]+/, "", value)
            gsub(/[[:space:]]+$/, "", value)
            print value
            exit
        }
    ' "/tmp/${label}.out"
}

expect_info_column() {
    label=$1
    index=$2
    expected=$3
    actual=$(info_column "${label}" "${index}")
    if [ "${actual}" != "${expected}" ]; then
        observe_gap "${label}_field_${index}_${actual}_expected_${expected}"
    fi
}

expect_info_decimal() {
    label=$1
    index=$2
    actual=$(info_column "${label}" "${index}")
    case "${actual}" in
        ''|*[!0-9]*) observe_gap "${label}_field_${index}_not_decimal" ;;
    esac
}

expect_ls_member() {
    label=$1
    expected_name=$2
    expected_dev=$3
    run_expect_success "${label}" dmsetup ls
    if ! awk -v expected_name="${expected_name}" -v expected_dev="(${expected_dev})" \
        '$1 == expected_name && $2 == expected_dev { found = 1 } END { exit !found }' \
        "/tmp/${label}.out"; then
        observe_gap "${label}_missing_${expected_name}_${expected_dev}"
    fi
}

expect_ls_absent() {
    label=$1
    expected_name=$2
    expected_dev=$3
    run_expect_success "${label}" dmsetup ls
    if awk -v expected_name="${expected_name}" -v expected_dev="(${expected_dev})" \
        '$1 == expected_name && $2 == expected_dev { found = 1 } END { exit !found }' \
        "/tmp/${label}.out"; then
        observe_gap "${label}_still_has_${expected_name}_${expected_dev}"
    fi
}

record_nodes() {
    label=$1
    dev=$2
    mapper_state=absent
    primary_state=unknown
    target=
    if [ -e "/dev/mapper/${dev}" ]; then
        mapper_state=present
        target=$(readlink -f "/dev/mapper/${dev}" 2>/dev/null || true)
    fi
    minor=$(timeout 3 dmsetup info "${dev}" 2>/dev/null | awk -F: '/Major, minor/ { gsub(/[[:space:]]/, "", $2); split($2, a, ","); print a[2]; exit }')
    if [ -n "${minor}" ]; then
        if [ -e "/dev/dm-${minor}" ]; then
            primary_state=present
        else
            primary_state=absent
        fi
    fi
    echo "NODE_${label}: mapper=${mapper_state} primary=${primary_state} target=${target}"
}


expect_table_line() {
    name=$1
    expected=$2
    label=$3
    run_expect_success "${label}" dmsetup table "${name}"
    grep_expect "${label}_expected" "^${expected}$" "/tmp/${label}.out"
}

expect_inactive_table_line() {
    name=$1
    expected=$2
    label=$3
    run_expect_success "${label}" dmsetup table --inactive "${name}"
    grep_expect "${label}_expected" "^${expected}$" "/tmp/${label}.out"
}

expect_deps_count() {
    name=$1
    count=$2
    label=$3
    run_expect_success "${label}" dmsetup deps "${name}"
    grep_expect "${label}_count" "${count} dependencies" "/tmp/${label}.out"
}

run_wait_current_event_unchanged_by_suspend() {
    label=$1
    dev=$2
    event_before=$(event_number "${dev}")
    trigger_out="/tmp/${label}.trigger.out"
    trigger_err="/tmp/${label}.trigger.err"
    echo "SCENARIO_BEGIN_${label}"
    echo "EVENT_BEFORE_${label}: ${event_before}"
    timeout 3 dmsetup suspend --noflush "${dev}" >"${trigger_out}" 2>"${trigger_err}"
    trigger_status=$?
    event_after=$(event_number "${dev}")
    echo "TRIGGER_STATUS_${label}: ${trigger_status}"
    echo "EVENT_AFTER_${label}: ${event_after}"
    print_stream TRIGGER_STDOUT "${label}" "${trigger_out}"
    print_stream TRIGGER_STDERR "${label}" "${trigger_err}"
    if [ "${trigger_status}" -ne 0 ]; then
        observe_gap "${label}_suspend_status_${trigger_status}_expected_0"
    fi
    if [ -z "${event_before}" ] || [ "${event_after}" != "${event_before}" ]; then
        observe_gap "${label}_event_${event_after}_expected_${event_before}"
    fi
    run_expect_status 124 "${label}_WAIT_CURRENT" \
        timeout 3 dmsetup wait --noflush "${dev}" "${event_after}"
    timeout 3 dmsetup resume --noflush "${dev}" >/dev/null 2>&1 || \
        observe_gap "${label}_resume_failed"
    echo "SCENARIO_END_${label}"
    return 0
}

finish_guest() {
    status=$?
    cleanup_dm
    sync
    finish_steps
    if [ "${status}" -eq 0 ] && [ "${GAP_COUNT}" -eq 0 ]; then
        echo TEST_PASS_DM_CONTROL_PLANE
    else
        status=1
        echo "TEST_FAIL_DM_CONTROL_PLANE gaps=${GAP_COUNT}"
    fi
    poweroff
    exit "${status}"
}

trap finish_guest EXIT
cleanup_dm

step '=== STEP 1: static dmsetup control-plane queries ==='
if [ ! -c /dev/mapper/control ]; then
    observe_gap missing_control_device
fi
run_expect_success STATIC_VERSION dmsetup version
run_expect_success STATIC_TARGETS dmsetup targets
grep_expect STATIC_TARGETS_HAS_ERROR '^error' /tmp/STATIC_TARGETS.out
grep_expect STATIC_TARGETS_HAS_LINEAR '^linear' /tmp/STATIC_TARGETS.out
grep_expect STATIC_TARGETS_HAS_STRIPED '^striped' /tmp/STATIC_TARGETS.out
grep_expect STATIC_TARGETS_HAS_ZERO '^zero' /tmp/STATIC_TARGETS.out
run_expect_success TARGET_VERSION_ERROR dmsetup target-version error
run_expect_success TARGET_VERSION_LINEAR dmsetup target-version linear
run_expect_success TARGET_VERSION_STRIPED dmsetup target-version striped
run_expect_success TARGET_VERSION_ZERO dmsetup target-version zero
run_expect_failure TARGET_VERSION_UNKNOWN dmsetup target-version aster_unknown
echo CHECK_PASS_DMSETUP_STATIC_VERSION_TARGETS

step '=== STEP 2: locate backing disks for table-dependent commands ==='
echo 'CMD_LOCATE_DISK1: aster-test-disk-locator 1'
TEST_DISK=$(aster-test-disk-locator 1) || fail_precondition locate_disk1_failed
printf 'TEST_DISK=%s
' "${TEST_DISK}"
echo 'CMD_LOCATE_DISK2: aster-test-disk-locator 2'
TEST_DISK2=$(aster-test-disk-locator 2) || fail_precondition locate_disk2_failed
printf 'TEST_DISK2=%s
' "${TEST_DISK2}"
[ "${TEST_DISK}" != "${TEST_DISK2}" ] || fail_precondition duplicate_backing_disks
[ -b "${TEST_DISK}" ] || fail_precondition missing_test_disk
[ -b "${TEST_DISK2}" ] || fail_precondition missing_test_disk2
DEV=$(devno "${TEST_DISK}")
DEV2=$(devno "${TEST_DISK2}")
printf 'DEV=%s
DEV2=%s
' "${DEV}" "${DEV2}"
echo CHECK_PASS_DMSETUP_BACKING_DISKS

step '=== STEP 3: empty list and tableless create ==='
stdin_name=$(name stdin)
notable_name=$(name notable)
run_expect_success EMPTY_LS dmsetup ls
run_shell_expect_success CREATE_STDIN_EOF "timeout 5 dmsetup create ${stdin_name} < /dev/null"
record_nodes CREATE_STDIN_EOF "${stdin_name}"
run_expect_success TABLELESS_INFO_STDIN dmsetup info "${stdin_name}"
run_expect_success CREATE_NOTABLE dmsetup create "${notable_name}" --notable
record_nodes CREATE_NOTABLE "${notable_name}"
run_expect_success TABLELESS_INFO_NOTABLE dmsetup info "${notable_name}"
run_expect_success TABLELESS_LS dmsetup ls
run_expect_success TABLELESS_REMOVE_STDIN dmsetup remove "${stdin_name}"
run_expect_success TABLELESS_REMOVE_NOTABLE dmsetup remove "${notable_name}"
echo CHECK_PASS_DMSETUP_TABLELESS_CREATE

step '=== STEP 4: Linux-style first load resume publication ==='
first_publish=$(name first_publish)
run_expect_success FIRST_PUBLISH_CREATE dmsetup create "${first_publish}" --notable
first_publish_minor=$(timeout 3 dmsetup info "${first_publish}" 2>/dev/null | awk -F: '/Major, minor/ { gsub(/[[:space:]]/, "", $2); split($2, a, ","); print a[2]; exit }')
[ -n "${first_publish_minor}" ] || observe_gap FIRST_PUBLISH_MISSING_MINOR
first_publish_primary="/dev/dm-${first_publish_minor}"
first_publish_alias="/dev/mapper/${first_publish}"
run_shell_expect_success FIRST_PUBLISH_CREATED_PATHS_ABSENT "[ ! -e '${first_publish_primary}' ] && [ ! -L '${first_publish_primary}' ] && [ ! -e '${first_publish_alias}' ] && [ ! -L '${first_publish_alias}' ]"
run_expect_success FIRST_PUBLISH_LOAD dmsetup load "${first_publish}" --table "0 8 linear ${DEV} 0"
record_nodes FIRST_PUBLISH_LOAD "${first_publish}"
run_shell_expect_success FIRST_PUBLISH_LOAD_STATE "[ -z \"\$(dmsetup table '${first_publish}')\" ] && [ \"\$(dmsetup table --inactive '${first_publish}')\" = '0 8 linear ${DEV} 0' ]"
run_shell_expect_success FIRST_PUBLISH_LOAD_PRIMARY_EOF "[ -b '${first_publish_primary}' ] && [ ! -e '${first_publish_alias}' ] && [ ! -L '${first_publish_alias}' ] && [ \"\$(blockdev --getsize64 '${first_publish_primary}')\" = 0 ] && [ \"\$(timeout 3 dd if='${first_publish_primary}' bs=512 count=1 status=none | wc -c)\" = 0 ]"
run_expect_success FIRST_PUBLISH_RESUME dmsetup resume "${first_publish}"
record_nodes FIRST_PUBLISH_RESUME "${first_publish}"
run_shell_expect_success FIRST_PUBLISH_RESUME_READY "[ -b '${first_publish_primary}' ] && [ -b '${first_publish_alias}' ] && [ \"\$(blockdev --getsize64 '${first_publish_primary}')\" = 4096 ] && [ \"\$(readlink -f '${first_publish_alias}')\" = '${first_publish_primary}' ]"
run_shell_expect_success FIRST_PUBLISH_PRIMARY_TO_ALIAS "printf 'dm-first-resume-primary' | dd of='${first_publish_primary}' bs=512 count=1 conv=sync status=none && [ \"\$(dd if='${first_publish_alias}' bs=512 count=1 status=none | head -c 23)\" = 'dm-first-resume-primary' ]"
run_shell_expect_success FIRST_PUBLISH_ALIAS_TO_PRIMARY "printf 'dm-first-resume-alias' | dd of='${first_publish_alias}' bs=512 count=1 conv=sync status=none && [ \"\$(dd if='${first_publish_primary}' bs=512 count=1 status=none | head -c 21)\" = 'dm-first-resume-alias' ]"
run_expect_success FIRST_PUBLISH_REMOVE dmsetup remove "${first_publish}"
run_shell_expect_success FIRST_PUBLISH_REMOVE_PATHS_ABSENT "[ ! -e '${first_publish_primary}' ] && [ ! -L '${first_publish_primary}' ] && [ ! -e '${first_publish_alias}' ] && [ ! -L '${first_publish_alias}' ]"

first_rename=$(name first_rename)
first_renamed=$(name first_renamed)
run_expect_success FIRST_RENAME_CREATE dmsetup create "${first_rename}" --notable
first_rename_minor=$(timeout 3 dmsetup info "${first_rename}" 2>/dev/null | awk -F: '/Major, minor/ { gsub(/[[:space:]]/, "", $2); split($2, a, ","); print a[2]; exit }')
[ -n "${first_rename_minor}" ] || observe_gap FIRST_RENAME_MISSING_MINOR
first_rename_primary="/dev/dm-${first_rename_minor}"
run_expect_success FIRST_RENAME_LOAD dmsetup load "${first_rename}" --table "0 8 linear ${DEV} 0"
run_expect_success FIRST_RENAME_NAME dmsetup rename "${first_rename}" "${first_renamed}"
run_shell_expect_success FIRST_RENAME_PRIMARY_ONLY "[ -b '${first_rename_primary}' ] && [ ! -e '/dev/mapper/${first_rename}' ] && [ ! -L '/dev/mapper/${first_rename}' ] && [ ! -e '/dev/mapper/${first_renamed}' ] && [ ! -L '/dev/mapper/${first_renamed}' ]"
run_expect_success FIRST_RENAME_RESUME dmsetup resume "${first_renamed}"
run_shell_expect_success FIRST_RENAME_ALIAS_AFTER_RESUME "[ -b '/dev/mapper/${first_renamed}' ] && [ \"\$(readlink -f '/dev/mapper/${first_renamed}')\" = '${first_rename_primary}' ]"
run_expect_success FIRST_RENAME_REMOVE dmsetup remove "${first_renamed}"

first_remove=$(name first_remove)
run_expect_success FIRST_REMOVE_CREATE dmsetup create "${first_remove}" --notable
first_remove_minor=$(timeout 3 dmsetup info "${first_remove}" 2>/dev/null | awk -F: '/Major, minor/ { gsub(/[[:space:]]/, "", $2); split($2, a, ","); print a[2]; exit }')
[ -n "${first_remove_minor}" ] || observe_gap FIRST_REMOVE_MISSING_MINOR
first_remove_primary="/dev/dm-${first_remove_minor}"
run_expect_success FIRST_REMOVE_LOAD dmsetup load "${first_remove}" --table "0 8 linear ${DEV} 0"
run_shell_expect_success FIRST_REMOVE_PRIMARY_ONLY "[ -b '${first_remove_primary}' ] && [ ! -e '/dev/mapper/${first_remove}' ] && [ ! -L '/dev/mapper/${first_remove}' ]"
run_expect_success FIRST_REMOVE dmsetup remove "${first_remove}"
run_expect_failure FIRST_REMOVE_INFO dmsetup info "${first_remove}"
run_shell_expect_success FIRST_REMOVE_PATHS_ABSENT "[ ! -e '${first_remove_primary}' ] && [ ! -L '${first_remove_primary}' ] && [ ! -e '/dev/mapper/${first_remove}' ] && [ ! -L '/dev/mapper/${first_remove}' ]"
echo CHECK_PASS_DMSETUP_FIRST_LOAD_RESUME_LIFECYCLE

step '=== STEP 5: linear create and query ==='
linear_major=$(name linear_major)
run_shell_expect_success LINEAR_CREATE_MAJOR "printf '0 8 linear ${DEV} 0\\n' | dmsetup create ${linear_major}"
record_nodes LINEAR_CREATE_MAJOR "${linear_major}"
run_expect_success LINEAR_INFO dmsetup info "${linear_major}"
expect_table_line "${linear_major}" "0 8 linear ${DEV} 0" LINEAR_TABLE
run_expect_success LINEAR_STATUS dmsetup status "${linear_major}"
expect_deps_count "${linear_major}" 1 LINEAR_DEPS
run_expect_success LINEAR_LS dmsetup ls
run_expect_success LINEAR_REMOVE dmsetup remove "${linear_major}"
record_nodes LINEAR_REMOVE "${linear_major}"
linear_path=$(name linear_path)
run_expect_success LINEAR_CREATE_PATH dmsetup create "${linear_path}" --table "0 8 linear ${TEST_DISK} 0"
record_nodes LINEAR_CREATE_PATH "${linear_path}"
expect_table_line "${linear_path}" "0 8 linear ${DEV} 0" LINEAR_PATH_TABLE
expect_deps_count "${linear_path}" 1 LINEAR_PATH_DEPS
run_expect_success LINEAR_PATH_REMOVE dmsetup remove "${linear_path}"
echo CHECK_PASS_DMSETUP_LINEAR_CREATE

step '=== STEP 5a: structured discovery fields ==='
discovery_name=$(name discovery)
discovery_uuid=asterinas-dmsetup-discovery-uuid-guest
run_expect_success DISCOVERY_CREATE dmsetup create "${discovery_name}" --table "0 8 linear ${DEV} 0"
capture_info_columns DISCOVERY_INFO "${discovery_name}"
expect_info_column DISCOVERY_INFO 1 "${discovery_name}"
expect_info_column DISCOVERY_INFO 2 ''
expect_info_decimal DISCOVERY_INFO 3
expect_info_decimal DISCOVERY_INFO 4
expect_info_column DISCOVERY_INFO 5 0
expect_info_column DISCOVERY_INFO 6 1
expect_info_column DISCOVERY_INFO 7 0
expect_info_column DISCOVERY_INFO 8 Live
expect_info_column DISCOVERY_INFO 9 Active
expect_info_column DISCOVERY_INFO 10 Writeable
discovery_major=$(info_column DISCOVERY_INFO 3)
discovery_minor=$(info_column DISCOVERY_INFO 4)
discovery_dev="${discovery_major}:${discovery_minor}"
expect_ls_member DISCOVERY_LS_PRESENT "${discovery_name}" "${discovery_dev}"
capture_info_columns DISCOVERY_BY_DEV -j "${discovery_major}" -m "${discovery_minor}"
expect_info_column DISCOVERY_BY_DEV 1 "${discovery_name}"
expect_info_column DISCOVERY_BY_DEV 3 "${discovery_major}"
expect_info_column DISCOVERY_BY_DEV 4 "${discovery_minor}"
exec 9<"/dev/mapper/${discovery_name}"
capture_info_columns DISCOVERY_OPEN "${discovery_name}"
exec 9<&-
expect_info_column DISCOVERY_OPEN 5 1
run_shell_expect_success DISCOVERY_LOAD_INACTIVE "printf '0 4 linear ${DEV} 0\\n4 4 linear ${DEV} 4\\n' | dmsetup load ${discovery_name}"
capture_info_columns DISCOVERY_ACTIVE_WITH_INACTIVE "${discovery_name}"
capture_info_columns DISCOVERY_INACTIVE --inactive "${discovery_name}"
expect_info_column DISCOVERY_ACTIVE_WITH_INACTIVE 6 1
expect_info_column DISCOVERY_INACTIVE 6 2
expect_info_column DISCOVERY_ACTIVE_WITH_INACTIVE 8 Both
expect_info_column DISCOVERY_INACTIVE 8 Both
discovery_event_before_uuid=$(info_column DISCOVERY_ACTIVE_WITH_INACTIVE 7)
run_expect_success DISCOVERY_SET_UUID dmsetup rename "${discovery_name}" --setuuid "${discovery_uuid}"
capture_info_columns DISCOVERY_BY_UUID -u "${discovery_uuid}"
expect_info_column DISCOVERY_BY_UUID 1 "${discovery_name}"
expect_info_column DISCOVERY_BY_UUID 2 "${discovery_uuid}"
expect_info_column DISCOVERY_BY_UUID 3 "${discovery_major}"
expect_info_column DISCOVERY_BY_UUID 4 "${discovery_minor}"
expect_info_column DISCOVERY_BY_UUID 7 "$((discovery_event_before_uuid + 1))"
run_expect_success DISCOVERY_REMOVE dmsetup remove "${discovery_name}"
run_expect_failure DISCOVERY_INFO_AFTER_REMOVE dmsetup info "${discovery_name}"
expect_ls_absent DISCOVERY_LS_ABSENT "${discovery_name}" "${discovery_dev}"
echo CHECK_PASS_DMSETUP_DISCOVERY_FIELDS

step '=== STEP 6: striped create and query ==='
striped_major=$(name striped_major)
run_shell_expect_success STRIPED_CREATE_MAJOR "printf '0 16 striped 2 4 ${DEV} 0 ${DEV2} 0\\n' | dmsetup create ${striped_major}"
record_nodes STRIPED_CREATE_MAJOR "${striped_major}"
run_expect_success STRIPED_INFO dmsetup info "${striped_major}"
expect_table_line "${striped_major}" "0 16 striped 2 4 ${DEV} 0 ${DEV2} 0" STRIPED_TABLE
run_expect_success STRIPED_STATUS dmsetup status "${striped_major}"
expect_deps_count "${striped_major}" 2 STRIPED_DEPS
run_expect_success STRIPED_REMOVE dmsetup remove "${striped_major}"
striped_path=$(name striped_path)
run_expect_success STRIPED_CREATE_PATH dmsetup create "${striped_path}" --table "0 16 striped 2 4 ${TEST_DISK} 0 ${TEST_DISK2} 0"
record_nodes STRIPED_CREATE_PATH "${striped_path}"
expect_table_line "${striped_path}" "0 16 striped 2 4 ${DEV} 0 ${DEV2} 0" STRIPED_PATH_TABLE
expect_deps_count "${striped_path}" 2 STRIPED_PATH_DEPS
run_expect_success STRIPED_PATH_REMOVE dmsetup remove "${striped_path}"
echo CHECK_PASS_DMSETUP_STRIPED_CREATE

step '=== STEP 7: error and zero create query and io ==='
error_name=$(name error)
run_expect_success ERROR_CREATE dmsetup create "${error_name}" --table "0 8 error"
record_nodes ERROR_CREATE "${error_name}"
run_expect_success ERROR_INFO dmsetup info "${error_name}"
expect_table_line "${error_name}" "0 8 error " ERROR_TABLE
run_expect_success ERROR_STATUS dmsetup status "${error_name}"
expect_deps_count "${error_name}" 0 ERROR_DEPS
run_shell_expect_failure ERROR_READ "dd if=/dev/mapper/${error_name} of=/dev/null bs=512 count=1 status=none"
run_shell_expect_failure ERROR_WRITE "dd if=/dev/zero of=/dev/mapper/${error_name} bs=512 count=1 status=none"
run_expect_success ERROR_REMOVE dmsetup remove "${error_name}"
zero_name=$(name zero)
run_expect_success ZERO_CREATE dmsetup create "${zero_name}" --table "0 8 zero"
record_nodes ZERO_CREATE "${zero_name}"
run_expect_success ZERO_INFO dmsetup info "${zero_name}"
expect_table_line "${zero_name}" "0 8 zero " ZERO_TABLE
run_expect_success ZERO_STATUS dmsetup status "${zero_name}"
expect_deps_count "${zero_name}" 0 ZERO_DEPS
run_shell_expect_success ZERO_READ_ALL_ZERO "timeout 5 sh -c 'dd if=/dev/mapper/${zero_name} of=/tmp/zero-read.bin bs=512 count=1 status=none && cmp -n 512 /tmp/zero-read.bin /dev/zero'"
run_shell_expect_success ZERO_WRITE "timeout 5 dd if=/dev/urandom of=/dev/mapper/${zero_name} bs=512 count=1 status=none"
run_shell_expect_success ZERO_READ_AFTER_WRITE_ALL_ZERO "timeout 5 sh -c 'dd if=/dev/mapper/${zero_name} of=/tmp/zero-read-after-write.bin bs=512 count=1 status=none && cmp -n 512 /tmp/zero-read-after-write.bin /dev/zero'"
run_shell_expect_success ZERO_BLKDISCARD "timeout 5 blkdiscard /dev/mapper/${zero_name}"
run_shell_expect_success ZERO_BLKZEROOUT "timeout 5 blkdiscard -z /dev/mapper/${zero_name}"
run_expect_success ZERO_REMOVE dmsetup remove "${zero_name}"
echo CHECK_PASS_DMSETUP_ERROR_ZERO_CREATE_IO

step '=== STEP 8: table lifecycle ==='
table_name=$(name table_lifecycle)
run_expect_success TABLE_LIFECYCLE_CREATE dmsetup create "${table_name}" --table "0 8 linear ${DEV} 0"
run_expect_success LOAD_INACTIVE dmsetup load "${table_name}" --table "0 16 linear ${DEV} 8"
expect_table_line "${table_name}" "0 8 linear ${DEV} 0" TABLE_ACTIVE_AFTER_LOAD
expect_inactive_table_line "${table_name}" "0 16 linear ${DEV} 8" TABLE_INACTIVE_AFTER_LOAD
run_expect_success STATUS_INACTIVE_AFTER_LOAD dmsetup status --inactive "${table_name}"
run_expect_success INFO_AFTER_LOAD dmsetup info "${table_name}"
run_expect_success CLEAR_INACTIVE dmsetup clear "${table_name}"
run_expect_success TABLE_INACTIVE_AFTER_CLEAR dmsetup table --inactive "${table_name}"
run_expect_success RELOAD_INACTIVE dmsetup reload "${table_name}" --table "0 16 linear ${DEV} 8"
run_expect_success RESUME_AFTER_RELOAD dmsetup resume "${table_name}"
expect_table_line "${table_name}" "0 16 linear ${DEV} 8" TABLE_AFTER_RESUME
run_expect_success SUSPEND_NOFLUSH_BEFORE_RELOAD dmsetup suspend --noflush "${table_name}"
run_expect_success RELOAD_NOFLUSH_INACTIVE dmsetup reload "${table_name}" --table "0 24 linear ${DEV} 16"
expect_table_line "${table_name}" "0 16 linear ${DEV} 8" TABLE_ACTIVE_BEFORE_NOFLUSH_RESUME
expect_inactive_table_line "${table_name}" "0 24 linear ${DEV} 16" TABLE_INACTIVE_BEFORE_NOFLUSH_RESUME
run_expect_success RESUME_AFTER_NOFLUSH_SUSPEND dmsetup resume "${table_name}"
expect_table_line "${table_name}" "0 24 linear ${DEV} 16" TABLE_AFTER_NOFLUSH_SUSPEND_RESUME
run_expect_success RELOAD_RESUME_NOFLUSH_COMPAT dmsetup reload "${table_name}" --table "0 32 linear ${DEV} 24"
run_expect_success RESUME_NOFLUSH_COMPAT dmsetup resume --noflush "${table_name}"
expect_table_line "${table_name}" "0 32 linear ${DEV} 24" TABLE_AFTER_RESUME_NOFLUSH_COMPAT
run_expect_success TABLE_LIFECYCLE_REMOVE dmsetup remove "${table_name}"
echo CHECK_PASS_DMSETUP_TABLE_LIFECYCLE

step '=== STEP 9: suspend resume wait event ==='
state_name=$(name state_event)
run_expect_success STATE_CREATE dmsetup create "${state_name}" --table "0 8 linear ${DEV} 0"
run_expect_failure UNSUPPORTED_MESSAGE env LC_ALL=C dmsetup message "${state_name}" 0 status
grep_expect UNSUPPORTED_MESSAGE_ENOTTY 'Inappropriate ioctl for device' /tmp/UNSUPPORTED_MESSAGE.err
run_expect_failure UNSUPPORTED_SETGEOMETRY env LC_ALL=C dmsetup setgeometry "${state_name}" 1 1 8 0
grep_expect UNSUPPORTED_SETGEOMETRY_ENOTTY 'Inappropriate ioctl for device' /tmp/UNSUPPORTED_SETGEOMETRY.err
echo "EVENT_STATE_AFTER_CREATE: $(event_number "${state_name}")"
run_expect_success SUSPEND dmsetup suspend "${state_name}"
run_expect_success INFO_AFTER_SUSPEND dmsetup info "${state_name}"
run_expect_success RESUME dmsetup resume "${state_name}"
run_expect_success INFO_AFTER_RESUME dmsetup info "${state_name}"
run_expect_success SUSPEND_NOFLUSH dmsetup suspend --noflush "${state_name}"
run_expect_success RESUME_NOFLUSH dmsetup resume --noflush "${state_name}"
run_wait_current_event_unchanged_by_suspend WAIT_CURRENT_EVENT "${state_name}"
run_expect_success STATE_REMOVE dmsetup remove "${state_name}"
echo CHECK_PASS_DMSETUP_SUSPEND_RESUME_WAIT

step '=== STEP 10: rename and uuid selector ==='
rename_name=$(name rename)
existing_name=$(name rename_existing)
rename_new=$(name renamed)
uuid_value=asterinas-dmsetup-baseline-uuid-guest
run_expect_success RENAME_CREATE dmsetup create "${rename_name}" --table "0 8 linear ${DEV} 0"
run_expect_failure RENAME_SAME_NAME dmsetup rename "${rename_name}" "${rename_name}"
run_expect_success RENAME_EXISTING_CREATE dmsetup create "${existing_name}" --table "0 8 linear ${DEV2} 0"
run_expect_failure RENAME_DUPLICATE dmsetup rename "${rename_name}" "${existing_name}"
event_before_rename=$(event_number "${rename_name}")
run_expect_success RENAME_NAME dmsetup rename "${rename_name}" "${rename_new}"
record_nodes RENAME_NAME "${rename_new}"
run_expect_failure INFO_OLD_NAME_AFTER_RENAME dmsetup info "${rename_name}"
run_expect_success INFO_NEW_NAME_AFTER_RENAME dmsetup info "${rename_new}"
event_after_rename=$(event_number "${rename_new}")
if [ -z "${event_before_rename}" ] || [ -z "${event_after_rename}" ] || [ "${event_after_rename}" -ne "$((event_before_rename + 1))" ]; then
    observe_gap WAIT_RENAME_EVENT_INCREMENT
fi
run_expect_status 0 WAIT_STALE_AFTER_RENAME timeout 3 dmsetup wait --noflush "${rename_new}" "${event_before_rename}"
run_expect_status 124 WAIT_CURRENT_AFTER_RENAME timeout 3 dmsetup wait --noflush "${rename_new}" "${event_after_rename}"
run_expect_success SET_UUID dmsetup rename "${rename_new}" --setuuid "${uuid_value}"
run_expect_success INFO_BY_UUID dmsetup info -u "${uuid_value}"
event_after_set_uuid=$(event_number "${rename_new}")
if [ -z "${event_after_set_uuid}" ] || [ "${event_after_set_uuid}" -ne "$((event_after_rename + 1))" ]; then
    observe_gap WAIT_SET_UUID_EVENT_INCREMENT
fi
run_expect_status 0 WAIT_STALE_AFTER_SET_UUID timeout 3 dmsetup wait --noflush "${rename_new}" "${event_after_rename}"
run_expect_status 124 WAIT_CURRENT_AFTER_SET_UUID timeout 3 dmsetup wait --noflush "${rename_new}" "${event_after_set_uuid}"
run_expect_success RENAME_REMOVE_NEW dmsetup remove "${rename_new}"
run_expect_success RENAME_REMOVE_EXISTING dmsetup remove "${existing_name}"
echo CHECK_PASS_DMSETUP_RENAME_UUID

step '=== STEP 11: read-only and busy device lifecycle ==='
readonly_name=$(name readonly)
run_shell_expect_success READONLY_CREATE "printf '0 8 linear ${DEV} 0\\n' | dmsetup --readonly create ${readonly_name}"
record_nodes READONLY_CREATE "${readonly_name}"
capture_info_columns READONLY_ACTIVE "${readonly_name}"
expect_info_column READONLY_ACTIVE 10 Read-only
run_shell_expect_success READONLY_READ "dd if=/dev/mapper/${readonly_name} of=/tmp/readonly-read.bin bs=512 count=1 status=none"
run_shell_expect_failure READONLY_WRITE "dd if=/dev/zero of=/dev/mapper/${readonly_name} bs=512 count=1 conv=fsync status=none"
run_shell_expect_success READONLY_RELOAD_WRITABLE "printf '0 8 linear ${DEV} 0\\n' | dmsetup reload ${readonly_name}"
capture_info_columns READONLY_ACTIVE_WITH_INACTIVE "${readonly_name}"
expect_info_column READONLY_ACTIVE_WITH_INACTIVE 10 Read-only
capture_info_columns READONLY_INACTIVE --inactive "${readonly_name}"
expect_info_column READONLY_INACTIVE 10 Writeable
run_expect_success READONLY_RESUME_WRITABLE dmsetup resume "${readonly_name}"
capture_info_columns READONLY_ACTIVE_WRITABLE "${readonly_name}"
expect_info_column READONLY_ACTIVE_WRITABLE 10 Writeable
run_shell_expect_success READONLY_WRITE_AFTER_RELOAD "dd if=/dev/zero of=/dev/mapper/${readonly_name} bs=512 count=1 conv=fsync status=none"
run_expect_success READONLY_REMOVE dmsetup remove "${readonly_name}"

deferred_name=$(name deferred_busy)
run_expect_success DEFERRED_CREATE dmsetup create "${deferred_name}" --table "0 8 linear ${DEV} 0"
run_expect_failure DEFERRED_REMOVE env LC_ALL=C dmsetup remove --deferred "${deferred_name}"
grep_expect DEFERRED_REMOVE_EOPNOTSUPP 'Operation not supported' /tmp/DEFERRED_REMOVE.err
run_expect_success DEFERRED_REMAINS dmsetup info "${deferred_name}"
run_expect_success DEFERRED_REMOVE_PLAIN dmsetup remove "${deferred_name}"

busy_name=$(name busy)
busy_other=$(name busy_other)
run_shell_expect_success BUSY_CREATE "printf '0 8 linear ${DEV} 0\\n' | dmsetup create ${busy_name}"
run_shell_expect_success BUSY_OTHER_CREATE "printf '0 8 linear ${DEV2} 0\\n' | dmsetup create ${busy_other}"
run_shell_expect_success BUSY_REMOVE_AND_REMOVE_ALL "exec 9</dev/mapper/${busy_name}; if dmsetup remove ${busy_name}; then exit 1; fi; dmsetup info ${busy_name} >/dev/null; dmsetup remove_all || true; dmsetup info ${busy_name} >/dev/null; if dmsetup info ${busy_other} >/dev/null 2>&1; then exit 1; fi; exec 9<&-; dmsetup remove ${busy_name}"
echo CHECK_PASS_DMSETUP_READONLY_BUSY_LIFECYCLE

step '=== STEP 12: remove and remove_all ==='
remove_active=$(name remove_active)
remove_tableless=$(name remove_tableless)
run_expect_success REMOVE_ACTIVE_CREATE dmsetup create "${remove_active}" --table "0 8 linear ${DEV} 0"
run_expect_success REMOVE_ACTIVE dmsetup remove "${remove_active}"
run_expect_failure INFO_AFTER_REMOVE_ACTIVE dmsetup info "${remove_active}"
run_expect_success REMOVE_TABLELESS_CREATE dmsetup create "${remove_tableless}" --notable
run_expect_success REMOVE_TABLELESS dmsetup remove "${remove_tableless}"
run_expect_failure REMOVE_NONEXISTENT dmsetup remove "$(name nonexistent)"
remove_all_a=$(name remove_all_a)
remove_all_b=$(name remove_all_b)
run_expect_success REMOVE_ALL_CREATE_A dmsetup create "${remove_all_a}" --table "0 8 linear ${DEV} 0"
run_expect_success REMOVE_ALL_CREATE_B dmsetup create "${remove_all_b}" --table "0 8 linear ${DEV2} 0"
run_expect_success REMOVE_ALL_TEST_ONLY dmsetup remove_all
run_expect_failure REMOVE_ALL_INFO_A dmsetup info "${remove_all_a}"
run_expect_failure REMOVE_ALL_INFO_B dmsetup info "${remove_all_b}"
run_expect_success REMOVE_ALL_EMPTY dmsetup remove_all
echo CHECK_PASS_DMSETUP_REMOVE_COMMANDS

exit 0
DMSETUP_GUEST_BODY
sh /tmp/dm_control_plane_guest.sh

GUEST_SCRIPT

SUMMARY_INCLUDE='TEST_|CHECK_PASS_|CHECK_SKIP_|OBSERVE_|=== STEP|SCENARIO_|CMD_|STATUS_|DURATION_|STEP_DURATION_|GUEST_DURATION_|SUMMARY_GAP_|STDOUT_|STDERR_|TRIGGER_|EVENT_|NODE_|TEST_DISK=|TEST_DISK2=|DEV=|DEV2=|Name:|State:|UUID:|Tables present:|linear|striped|error|zero|dependencies|Command failed|Invalid argument|Input/output error|No such device|No devices found|Kernel panic|panicked'
dm_run_single_guest_test "${TEST_ID}" "${GUEST_SCRIPT_FILE}" TEST_PASS_DM_CONTROL_PLANE "${SUMMARY_INCLUDE}"
