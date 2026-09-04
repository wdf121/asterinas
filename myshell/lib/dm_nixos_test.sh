#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

DM_SUMMARY_EXCLUDE='root@asterinas|^\+ |^echo |echo (TEST_FAIL|HOST_FAIL)_|^[[:space:]]*[A-Z0-9_]+=\$\(|^trap |^set |^test |^if |^timeout |^exec |^dd |^grep |^awk |^dmsetup |^tee |^sed |^cat |^printf |^mount |^umount |^vgchange |^vgscan |^pvscan |^pvs |^vgs |^lvs |^df |^du |^md5sum |^mkdir |^sync |^poweroff |^LVM_CONFIG=|^MAPPER_DEVICE='

_dm_test_tmp_slug() {
    printf '%s' "$1" | tr '[:upper:]_' '[:lower:]-'
}

dm_check_no_qemu() {
    local test_id=$1
    local slug
    slug=$(_dm_test_tmp_slug "${test_id}")
    if pgrep -af '[q]emu-system' >"/tmp/${slug}-qemu-running.txt" 2>/dev/null; then
        echo "HOST_FAIL_${test_id} existing_qemu"
        cat "/tmp/${slug}-qemu-running.txt"
        exit 1
    fi
}

dm_check_nixos_image() {
    local test_id=$1
    if [ ! -f target/nixos/asterinas.img ]; then
        echo "HOST_FAIL_${test_id} missing target/nixos/asterinas.img; run 'make nixos' first"
        exit 1
    fi
}

dm_test_images() {
    if [ -n "${DM_TEST_IMAGES:-}" ]; then
        printf '%s\n' ${DM_TEST_IMAGES}
    else
        printf '%s\n' "${DM_TEST_IMAGE}" "${DM_TEST_IMAGE_2}"
    fi
}

dm_test_images_env() {
    dm_test_images | paste -sd ' ' -
}

dm_reset_test_images() {
    local image_path

    if [ "${RESET_DM_TEST_IMAGES}" = "1" ]; then
        dm_test_images | while IFS= read -r image_path; do
            rm -f "${image_path}"
        done
    fi
}

dm_prepare_nixos_test() {
    local test_id=$1
    rm -f "${LOG}"
    echo "HOST_INFO_${test_id} started"
    echo "HOST_INFO_${test_id} log=${LOG}"
    dm_check_no_qemu "${test_id}"
    dm_check_nixos_image "${test_id}"
    dm_reset_test_images
}

dm_run_guest_script() {
    local test_id=$1
    local script_file=$2
    local log_mode=$3
    local label=$4
    local slug fifo start_line qemu_pid status dm_test_images start_ts elapsed ready_timeout_seconds lifecycle_timeout_seconds start_at completed_at input_line_delay

    dm_test_images=$(dm_test_images_env)
    ready_timeout_seconds=${GUEST_READY_TIMEOUT:-40}
    lifecycle_timeout_seconds=${GUEST_QEMU_TIMEOUT:-180}
    input_line_delay=${GUEST_INPUT_LINE_DELAY:-0.01}
    slug=$(_dm_test_tmp_slug "${test_id}")
    fifo=$(mktemp -u "/tmp/${slug}-stdin.XXXXXX")
    mkfifo "${fifo}"

    if [ -f "${LOG}" ]; then
        start_line=$(wc -l <"${LOG}")
    else
        start_line=0
    fi

    start_ts=$(date +%s)
    start_at=$(date -Is)
    echo "HOST_INFO_${test_id} ${label}_guest_started_at=${start_at}"
    if [ "${log_mode}" = "append" ]; then
        DM_TEST_IMAGES="${dm_test_images}" \
        DM_TEST_IMAGE="${DM_TEST_IMAGE}" \
        DM_TEST_IMAGE_2="${DM_TEST_IMAGE_2}" \
        setsid make run_nixos <"${fifo}" >>"${LOG}" 2>&1 &
    else
        DM_TEST_IMAGES="${dm_test_images}" \
        DM_TEST_IMAGE="${DM_TEST_IMAGE}" \
        DM_TEST_IMAGE_2="${DM_TEST_IMAGE_2}" \
        setsid make run_nixos <"${fifo}" >"${LOG}" 2>&1 &
    fi
    qemu_pid=$!

    exec 3>"${fifo}"
    rm -f "${fifo}"

    while ! tail -n "+$((start_line + 1))" "${LOG}" | grep -aq 'root@asterinas'; do
        if ! kill -0 "${qemu_pid}" 2>/dev/null; then
            exec 3>&-
            status=0
            wait "${qemu_pid}" || status=$?
            if [ "${status}" -eq 0 ]; then
                status=1
            fi
            completed_at=$(date -Is)
            echo "HOST_FAIL_${test_id} ${label}_guest_exited_before_shell status=${status}"
            echo "HOST_INFO_${test_id} ${label}_guest_completed_at=${completed_at} lifecycle_after=$(($(date +%s) - start_ts))s"
            return "${status}"
        fi
        elapsed=$(($(date +%s) - start_ts))
        if [ "${elapsed}" -ge "${ready_timeout_seconds}" ]; then
            echo "HOST_FAIL_${test_id} ${label}_guest_ready_timeout=${ready_timeout_seconds}s"
            exec 3>&-
            kill -- "-${qemu_pid}" 2>/dev/null || kill "${qemu_pid}" 2>/dev/null || true
            wait "${qemu_pid}" || true
            completed_at=$(date -Is)
            echo "HOST_INFO_${test_id} ${label}_guest_completed_at=${completed_at} lifecycle_after=$(($(date +%s) - start_ts))s"
            return 124
        fi
        sleep 1
    done

    elapsed=$(($(date +%s) - start_ts))
    echo "HOST_INFO_${test_id} ${label}_guest_ready_after=${elapsed}s ready_timeout=${ready_timeout_seconds}s lifecycle_timeout=${lifecycle_timeout_seconds}s"
    if [ "${input_line_delay}" != "0" ]; then
        while IFS= read -r line || [ -n "${line}" ]; do
            printf '%s\n' "${line}" >&3
            sleep "${input_line_delay}"
        done <"${script_file}"
    else
        cat "${script_file}" >&3
    fi
    exec 3>&-

    while kill -0 "${qemu_pid}" 2>/dev/null; do
        elapsed=$(($(date +%s) - start_ts))
        if [ "${elapsed}" -ge "${lifecycle_timeout_seconds}" ]; then
            echo "HOST_FAIL_${test_id} ${label}_guest_lifecycle_timeout=${lifecycle_timeout_seconds}s"
            kill -- "-${qemu_pid}" 2>/dev/null || kill "${qemu_pid}" 2>/dev/null || true
            wait "${qemu_pid}" || true
            completed_at=$(date -Is)
            echo "HOST_INFO_${test_id} ${label}_guest_completed_at=${completed_at} lifecycle_after=$(($(date +%s) - start_ts))s"
            return 124
        fi
        sleep 1
    done

    status=0
    wait "${qemu_pid}" || status=$?
    completed_at=$(date -Is)
    echo "HOST_INFO_${test_id} ${label}_guest_completed_at=${completed_at} lifecycle_after=$(($(date +%s) - start_ts))s status=${status}"
    return "${status}"
}

dm_print_summary() {
    local test_id=$1
    local include=$2
    local exclude=${3:-${DM_SUMMARY_EXCLUDE}}

    echo "HOST_INFO_${test_id} summary:"
    if [ ! -f "${LOG}" ]; then
        echo "HOST_INFO_${test_id} missing_log=${LOG}"
        return
    fi

    grep -aE "${include}" "${LOG}" \
        | grep -avE "${exclude}" \
        | awk 'BEGIN { seen = 0 } /^=== STEP|^=== CHECK|^DM_TABLE_.*_BEGIN/ { if (seen) print ""; seen = 1 } { print } /^DM_TABLE_.*_END/ { print "" }' \
        || true
}

dm_log_has_marker() {
    local marker=$1
    grep -aqE "^${marker}([[:space:]]|$)" "${LOG}"
}

dm_log_has_failure_marker() {
    grep -aqE '^(TEST_FAIL|HOST_FAIL)_' "${LOG}"
}

dm_print_failure_context() {
    local test_id=$1

    echo "HOST_INFO_${test_id} log=${LOG}"
    if [ ! -f "${LOG}" ]; then
        echo "HOST_INFO_${test_id} missing_log=${LOG}"
        return
    fi
    if grep -aq 'Error 65' "${LOG}"; then
        echo "HOST_INFO_${test_id} qemu_error_65=asterinas_exit_failure_before_guest_shell"
    fi
    grep -aE 'TEST_FAIL|HOST_FAIL|ERROR:|assertion failed|panic|panicked|Error [0-9]+|No space left|Input/output error|Command failed' "${LOG}" | tail -n 40 || true
    if [ -f qemu-serial.log ]; then
        grep -aE 'ERROR:|assertion failed|panic|panicked' qemu-serial.log | tail -n 20 || true
    fi
}

dm_run_single_guest_test() {
    local test_id=$1
    local script_file=$2
    local pass_marker=$3
    local summary_include=$4
    local status=0

    dm_run_guest_script "${test_id}" "${script_file}" append guest || status=$?
    rm -f "${script_file}"

    dm_print_summary "${test_id}" "${summary_include}"
    if [ "${status}" -ne 0 ] || dm_log_has_failure_marker || ! dm_log_has_marker "${pass_marker}"; then
        dm_print_failure_context "${test_id}"
        echo "HOST_FAIL_${test_id} qemu_status=${status}"
        exit 1
    fi

    echo "HOST_PASS_${test_id}"
}

dm_run_two_guest_test() {
    local test_id=$1
    local first_script=$2
    local second_script=$3
    local first_pass=$4
    local second_pass=$5
    local summary_include=$6
    local first_status=0 second_status=0

    dm_run_guest_script "${test_id}" "${first_script}" replace first || first_status=$?
    rm -f "${first_script}"
    if [ "${first_status}" -ne 0 ] || dm_log_has_failure_marker || ! dm_log_has_marker "${first_pass}"; then
        dm_print_summary "${test_id}" "${summary_include}"
        dm_print_failure_context "${test_id}"
        echo "HOST_FAIL_${test_id} first_qemu_status=${first_status}"
        rm -f "${second_script}"
        exit 1
    fi

    dm_run_guest_script "${test_id}" "${second_script}" append second || second_status=$?
    rm -f "${second_script}"

    dm_print_summary "${test_id}" "${summary_include}"
    if [ "${second_status}" -ne 0 ] || dm_log_has_failure_marker || ! dm_log_has_marker "${second_pass}"; then
        dm_print_failure_context "${test_id}"
        echo "HOST_FAIL_${test_id} second_qemu_status=${second_status}"
        exit 1
    fi

    echo "HOST_PASS_${test_id}"
}

# Runs a three-boot flow while preserving one host log and checking each guest's marker.
# The third guest is used by integration suites that must verify persisted state after shrink.
dm_run_three_guest_test() {
    local test_id=$1
    local first_script=$2
    local second_script=$3
    local third_script=$4
    local first_pass=$5
    local second_pass=$6
    local third_pass=$7
    local summary_include=$8
    local first_status=0 second_status=0 third_status=0

    dm_run_guest_script "${test_id}" "${first_script}" replace first || first_status=$?
    rm -f "${first_script}"
    if [ "${first_status}" -ne 0 ] || dm_log_has_failure_marker || ! dm_log_has_marker "${first_pass}"; then
        dm_print_summary "${test_id}" "${summary_include}"
        dm_print_failure_context "${test_id}"
        echo "HOST_FAIL_${test_id} first_qemu_status=${first_status}"
        rm -f "${second_script}" "${third_script}"
        exit 1
    fi

    dm_run_guest_script "${test_id}" "${second_script}" append second || second_status=$?
    rm -f "${second_script}"
    if [ "${second_status}" -ne 0 ] || dm_log_has_failure_marker || ! dm_log_has_marker "${second_pass}"; then
        dm_print_summary "${test_id}" "${summary_include}"
        dm_print_failure_context "${test_id}"
        echo "HOST_FAIL_${test_id} second_qemu_status=${second_status}"
        rm -f "${third_script}"
        exit 1
    fi

    dm_run_guest_script "${test_id}" "${third_script}" append third || third_status=$?
    rm -f "${third_script}"

    dm_print_summary "${test_id}" "${summary_include}"
    if [ "${third_status}" -ne 0 ] || dm_log_has_failure_marker || ! dm_log_has_marker "${third_pass}"; then
        dm_print_failure_context "${test_id}"
        echo "HOST_FAIL_${test_id} third_qemu_status=${third_status}"
        exit 1
    fi

    echo "HOST_PASS_${test_id}"
}
