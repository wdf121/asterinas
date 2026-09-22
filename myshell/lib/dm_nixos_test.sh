#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

DM_SUMMARY_EXCLUDE='root@asterinas|^\+ |^echo |echo (TEST_FAIL|HOST_FAIL)_|^[[:space:]]*[A-Z0-9_]+=\$\(|^trap |^set |^test |^if |^timeout |^exec |^dd |^grep |^awk |^dmsetup |^tee |^sed |^cat |^printf |^mount |^umount |^vgchange |^vgscan |^pvscan |^pvs |^vgs |^lvs |^df |^du |^md5sum |^mkdir |^sync |^poweroff |^LVM_CONFIG=|^MAPPER_DEVICE='

_dm_test_tmp_slug() {
    printf '%s' "$1" | tr '[:upper:]_' '[:lower:]-'
}

DM_LOG_TEST_ID=
DM_LOG_FIFO=
DM_LOG_FD=
DM_LOG_WRITER_PID=

dm_emit() {
    printf '\n%s\n' "$*" >&"${DM_LOG_FD}"
}

dm_close_log() {
    local status=$?

    trap - EXIT
    if [ -n "${DM_LOG_FD}" ]; then
        exec {DM_LOG_FD}>&-
        DM_LOG_FD=
    fi
    if [ -n "${DM_LOG_WRITER_PID}" ]; then
        wait "${DM_LOG_WRITER_PID}" || true
        DM_LOG_WRITER_PID=
    fi
    if [ -n "${DM_LOG_FIFO}" ]; then
        rm -f "${DM_LOG_FIFO}"
        DM_LOG_FIFO=
    fi
    return "${status}"
}

dm_report_host_error() {
    local status=$1
    local line=$2

    trap - ERR
    dm_emit "HOST_FAIL_${DM_LOG_TEST_ID} host_command_failure status=${status} line=${line}"
    exit "${status}"
}

dm_init_log() {
    local test_id=$1
    local slug

    DM_LOG_TEST_ID=${test_id}
    slug=$(_dm_test_tmp_slug "${test_id}")
    DM_LOG_FIFO=$(mktemp -u "/tmp/${slug}-log.XXXXXX")
    : >"${LOG}"
    mkfifo "${DM_LOG_FIFO}"
    tee -a "${LOG}" <"${DM_LOG_FIFO}" &
    DM_LOG_WRITER_PID=$!
    exec {DM_LOG_FD}>"${DM_LOG_FIFO}"
    trap 'dm_close_log' EXIT
    trap 'dm_report_host_error "$?" "$LINENO"' ERR
}

dm_check_no_qemu() {
    local test_id=$1
    local qemu_processes

    if qemu_processes=$(pgrep -af '[q]emu-system'); then
        dm_emit "HOST_FAIL_${test_id} existing_qemu"
        dm_emit "${qemu_processes}"
        exit 1
    fi
}

dm_check_nixos_image() {
    local test_id=$1
    if [ ! -f target/nixos/asterinas.img ]; then
        dm_emit "HOST_FAIL_${test_id} missing target/nixos/asterinas.img; run 'make nixos' first"
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

    dm_emit "HOST_INFO_${test_id} started"
    dm_emit "HOST_INFO_${test_id} log=${LOG}"
    dm_check_no_qemu "${test_id}"
    dm_check_nixos_image "${test_id}"
    dm_reset_test_images
}

_dm_stop_process_group() {
    local leader_pid=${1:-}
    local grace_seconds=${2:-5}
    local grace_deadline

    if [ -z "${leader_pid}" ]; then
        return
    fi
    if kill -0 -- "-${leader_pid}" 2>/dev/null; then
        kill -TERM -- "-${leader_pid}" 2>/dev/null || true
        grace_deadline=$(($(date +%s) + grace_seconds))
        while kill -0 -- "-${leader_pid}" 2>/dev/null &&
              [ "$(date +%s)" -lt "${grace_deadline}" ]; do
            sleep 1
        done
        if kill -0 -- "-${leader_pid}" 2>/dev/null; then
            kill -KILL -- "-${leader_pid}" 2>/dev/null || true
        fi
    fi
    wait "${leader_pid}" 2>/dev/null || true
}

_dm_stop_guest_process_group() {
    _dm_stop_process_group "$@"
}

dm_run_guest_script() (
    local test_id=$1
    local script_file=$2
    local label=$3
    local slug fifo start_line status feeder_status
    local dm_test_images start_ts now_ts elapsed ready_timeout_seconds
    local lifecycle_timeout_seconds lifecycle_deadline ready_deadline start_at completed_at
    local input_line_delay ready_timeout_input lifecycle_timeout_input
    local qemu_pid= feeder_pid= fifo_guard_open=0 feeder_fd_open=0
    local qemu_reaped=0 qemu_status=0

    dm_test_images=$(dm_test_images_env)
    ready_timeout_input=${GUEST_READY_TIMEOUT:-40}
    lifecycle_timeout_input=${GUEST_QEMU_TIMEOUT:-180}
    for timeout_value in "${ready_timeout_input}" "${lifecycle_timeout_input}"; do
        case "${timeout_value}" in
            ''|*[!0-9]*)
                dm_emit "HOST_FAIL_${test_id} ${label}_invalid_timeout=${timeout_value:-<empty>}"
                return 2
                ;;
        esac
        if ((10#${timeout_value} <= 0)); then
            dm_emit "HOST_FAIL_${test_id} ${label}_invalid_timeout=${timeout_value}"
            return 2
        fi
    done
    ready_timeout_seconds=$((10#${ready_timeout_input}))
    lifecycle_timeout_seconds=$((10#${lifecycle_timeout_input}))
    input_line_delay=${GUEST_INPUT_LINE_DELAY:-0.01}
    slug=$(_dm_test_tmp_slug "${test_id}")
    fifo=$(mktemp -u "/tmp/${slug}-stdin.XXXXXX")

    trap '
        if [ "${feeder_fd_open}" -eq 1 ]; then exec 3>&-; fi
        if [ "${fifo_guard_open}" -eq 1 ]; then exec 4>&-; fi
        if [ -n "${feeder_pid}" ]; then _dm_stop_process_group "${feeder_pid}" 1; fi
        if [ -n "${qemu_pid}" ]; then _dm_stop_guest_process_group "${qemu_pid}"; fi
        rm -f "${fifo}"
    ' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM

    mkfifo "${fifo}"
    exec 4<>"${fifo}"
    fifo_guard_open=1

    if [ -f "${LOG}" ]; then
        start_line=$(wc -l <"${LOG}")
    else
        start_line=0
    fi

    start_ts=$(date +%s)
    lifecycle_deadline=$((start_ts + lifecycle_timeout_seconds))
    ready_deadline=$((start_ts + ready_timeout_seconds))
    if [ "${ready_deadline}" -gt "${lifecycle_deadline}" ]; then
        ready_deadline=${lifecycle_deadline}
    fi
    start_at=$(date -Is)
    dm_emit "HOST_INFO_${test_id} ${label}_guest_started_at=${start_at}"
    DM_TEST_IMAGES="${dm_test_images}" \
    DM_TEST_IMAGE="${DM_TEST_IMAGE}" \
    DM_TEST_IMAGE_2="${DM_TEST_IMAGE_2}" \
    setsid make run_nixos 3>&- 4>&- <"${fifo}" >"${DM_LOG_FIFO}" 2>&1 &
    qemu_pid=$!
    exec 3>"${fifo}"
    feeder_fd_open=1
    exec 4>&-
    fifo_guard_open=0

    while ! tail -n "+$((start_line + 1))" "${LOG}" | grep -aq 'root@asterinas'; do
        if ! kill -0 "${qemu_pid}" 2>/dev/null; then
            exec 3>&-
            feeder_fd_open=0
            status=0
            wait "${qemu_pid}" || status=$?
            qemu_pid=
            if [ "${status}" -eq 0 ]; then
                status=1
            fi
            completed_at=$(date -Is)
            dm_emit "HOST_FAIL_${test_id} ${label}_guest_exited_before_shell status=${status}"
            dm_emit "HOST_INFO_${test_id} ${label}_guest_completed_at=${completed_at} lifecycle_after=$(($(date +%s) - start_ts))s"
            return "${status}"
        fi
        now_ts=$(date +%s)
        if [ "${now_ts}" -ge "${ready_deadline}" ]; then
            exec 3>&-
            feeder_fd_open=0
            if [ "${now_ts}" -ge "${lifecycle_deadline}" ]; then
                dm_emit "HOST_FAIL_${test_id} ${label}_guest_lifecycle_timeout=${lifecycle_timeout_seconds}s phase=ready"
            else
                dm_emit "HOST_FAIL_${test_id} ${label}_guest_ready_timeout=${ready_timeout_seconds}s"
            fi
            _dm_stop_guest_process_group "${qemu_pid}"
            qemu_pid=
            completed_at=$(date -Is)
            dm_emit "HOST_INFO_${test_id} ${label}_guest_completed_at=${completed_at} lifecycle_after=$(($(date +%s) - start_ts))s"
            return 124
        fi
        sleep 1
    done

    elapsed=$(($(date +%s) - start_ts))
    dm_emit "HOST_INFO_${test_id} ${label}_guest_ready_after=${elapsed}s ready_timeout=${ready_timeout_seconds}s lifecycle_timeout=${lifecycle_timeout_seconds}s"
    setsid bash -c '
        script_file=$1
        input_line_delay=$2
        status=0
        if [ "${input_line_delay}" != "0" ]; then
            mapfile -t input_lines <"${script_file}" || status=$?
            if [ "${status}" -eq 0 ]; then
                for ((index = 0; index < ${#input_lines[@]}; index++)); do
                    if [ "${index}" -gt 0 ]; then
                        sleep "${input_line_delay}" || {
                            status=$?
                            break
                        }
                    fi
                    printf "%s\n" "${input_lines[index]}" >&3 || {
                        status=$?
                        break
                    }
                done
            fi
        else
            cat "${script_file}" >&3 || status=$?
        fi
        exec 3>&-
        exit "${status}"
    ' dm-guest-feeder "${script_file}" "${input_line_delay}" 4>&- &
    feeder_pid=$!
    exec 3>&-
    feeder_fd_open=0
    rm -f "${fifo}"

    while kill -0 "${feeder_pid}" 2>/dev/null; do
        if ! kill -0 "${qemu_pid}" 2>/dev/null; then
            qemu_status=0
            wait "${qemu_pid}" || qemu_status=$?
            qemu_pid=
            qemu_reaped=1
            for _ in 1 2; do
                if ! kill -0 "${feeder_pid}" 2>/dev/null; then
                    break
                fi
                sleep 1
            done
            if kill -0 "${feeder_pid}" 2>/dev/null; then
                _dm_stop_process_group "${feeder_pid}" 1
                feeder_pid=
                if [ "${qemu_status}" -eq 0 ]; then
                    qemu_status=1
                fi
                completed_at=$(date -Is)
                dm_emit "HOST_FAIL_${test_id} ${label}_guest_exited_during_input status=${qemu_status}"
                dm_emit "HOST_INFO_${test_id} ${label}_guest_completed_at=${completed_at} lifecycle_after=$(($(date +%s) - start_ts))s"
                return "${qemu_status}"
            fi
            break
        fi
        if [ "$(date +%s)" -ge "${lifecycle_deadline}" ]; then
            dm_emit "HOST_FAIL_${test_id} ${label}_guest_lifecycle_timeout=${lifecycle_timeout_seconds}s phase=input"
            _dm_stop_process_group "${feeder_pid}" 1
            feeder_pid=
            _dm_stop_guest_process_group "${qemu_pid}"
            qemu_pid=
            completed_at=$(date -Is)
            dm_emit "HOST_INFO_${test_id} ${label}_guest_completed_at=${completed_at} lifecycle_after=$(($(date +%s) - start_ts))s"
            return 124
        fi
        sleep 1
    done
    feeder_status=0
    wait "${feeder_pid}" || feeder_status=$?
    feeder_pid=
    if [ "${feeder_status}" -ne 0 ]; then
        dm_emit "HOST_FAIL_${test_id} ${label}_guest_input_failure status=${feeder_status}"
        _dm_stop_guest_process_group "${qemu_pid}"
        qemu_pid=
        completed_at=$(date -Is)
        dm_emit "HOST_INFO_${test_id} ${label}_guest_completed_at=${completed_at} lifecycle_after=$(($(date +%s) - start_ts))s"
        return "${feeder_status}"
    fi

    if [ "${qemu_reaped}" -eq 0 ]; then
        while kill -0 "${qemu_pid}" 2>/dev/null; do
            if [ "$(date +%s)" -ge "${lifecycle_deadline}" ]; then
                dm_emit "HOST_FAIL_${test_id} ${label}_guest_lifecycle_timeout=${lifecycle_timeout_seconds}s phase=execution_or_shutdown"
                _dm_stop_guest_process_group "${qemu_pid}"
                qemu_pid=
                completed_at=$(date -Is)
                dm_emit "HOST_INFO_${test_id} ${label}_guest_completed_at=${completed_at} lifecycle_after=$(($(date +%s) - start_ts))s"
                return 124
            fi
            sleep 1
        done

        qemu_status=0
        wait "${qemu_pid}" || qemu_status=$?
        qemu_pid=
    fi
    completed_at=$(date -Is)
    dm_emit "HOST_INFO_${test_id} ${label}_guest_completed_at=${completed_at} lifecycle_after=$(($(date +%s) - start_ts))s status=${qemu_status}"
    return "${qemu_status}"
)
dm_print_summary() {
    local test_id=$1
    local include=$2
    local exclude=${3:-${DM_SUMMARY_EXCLUDE}}

    dm_emit "HOST_INFO_${test_id} summary:"
    if [ ! -f "${LOG}" ]; then
        dm_emit "HOST_INFO_${test_id} missing_log=${LOG}"
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

    dm_emit "HOST_INFO_${test_id} log=${LOG}"
    if [ ! -f "${LOG}" ]; then
        dm_emit "HOST_INFO_${test_id} missing_log=${LOG}"
        return
    fi
    if grep -aq 'Error 65' "${LOG}"; then
        dm_emit "HOST_INFO_${test_id} qemu_error_65=asterinas_exit_failure_before_guest_shell"
    fi
    grep -aE 'TEST_FAIL|HOST_FAIL|ERROR:|assertion failed|panic|panicked|Error [0-9]+|No space left|Input/output error|Command failed' "${LOG}" | tail -n 40 || true
    if [ -f qemu-serial.log ]; then
        while IFS= read -r line; do
            dm_emit "QEMU_SERIAL: ${line}"
        done < <(grep -aE 'ERROR:|assertion failed|panic|panicked' qemu-serial.log || true)
    fi
}

dm_run_single_guest_test() {
    local test_id=$1
    local script_file=$2
    local pass_marker=$3
    local summary_include=$4
    local status=0

    dm_run_guest_script "${test_id}" "${script_file}" guest || status=$?
    rm -f "${script_file}"

    dm_print_summary "${test_id}" "${summary_include}"
    if [ "${status}" -ne 0 ] || dm_log_has_failure_marker || ! dm_log_has_marker "${pass_marker}"; then
        dm_print_failure_context "${test_id}"
        dm_emit "HOST_FAIL_${test_id} qemu_status=${status}"
        exit 1
    fi

    dm_emit "HOST_PASS_${test_id}"
}

dm_run_two_guest_test() {
    local test_id=$1
    local first_script=$2
    local second_script=$3
    local first_pass=$4
    local second_pass=$5
    local summary_include=$6
    local first_status=0 second_status=0

    dm_run_guest_script "${test_id}" "${first_script}" first || first_status=$?
    rm -f "${first_script}"
    if [ "${first_status}" -ne 0 ] || dm_log_has_failure_marker || ! dm_log_has_marker "${first_pass}"; then
        dm_print_summary "${test_id}" "${summary_include}"
        dm_print_failure_context "${test_id}"
        dm_emit "HOST_FAIL_${test_id} first_qemu_status=${first_status}"
        rm -f "${second_script}"
        exit 1
    fi

    dm_run_guest_script "${test_id}" "${second_script}" second || second_status=$?
    rm -f "${second_script}"

    dm_print_summary "${test_id}" "${summary_include}"
    if [ "${second_status}" -ne 0 ] || dm_log_has_failure_marker || ! dm_log_has_marker "${second_pass}"; then
        dm_print_failure_context "${test_id}"
        dm_emit "HOST_FAIL_${test_id} second_qemu_status=${second_status}"
        exit 1
    fi

    dm_emit "HOST_PASS_${test_id}"
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

    dm_run_guest_script "${test_id}" "${first_script}" first || first_status=$?
    rm -f "${first_script}"
    if [ "${first_status}" -ne 0 ] || dm_log_has_failure_marker || ! dm_log_has_marker "${first_pass}"; then
        dm_print_summary "${test_id}" "${summary_include}"
        dm_print_failure_context "${test_id}"
        dm_emit "HOST_FAIL_${test_id} first_qemu_status=${first_status}"
        rm -f "${second_script}" "${third_script}"
        exit 1
    fi

    dm_run_guest_script "${test_id}" "${second_script}" second || second_status=$?
    rm -f "${second_script}"
    if [ "${second_status}" -ne 0 ] || dm_log_has_failure_marker || ! dm_log_has_marker "${second_pass}"; then
        dm_print_summary "${test_id}" "${summary_include}"
        dm_print_failure_context "${test_id}"
        dm_emit "HOST_FAIL_${test_id} second_qemu_status=${second_status}"
        rm -f "${third_script}"
        exit 1
    fi

    dm_run_guest_script "${test_id}" "${third_script}" third || third_status=$?
    rm -f "${third_script}"

    dm_print_summary "${test_id}" "${summary_include}"
    if [ "${third_status}" -ne 0 ] || dm_log_has_failure_marker || ! dm_log_has_marker "${third_pass}"; then
        dm_print_failure_context "${test_id}"
        dm_emit "HOST_FAIL_${test_id} third_qemu_status=${third_status}"
        exit 1
    fi

    dm_emit "HOST_PASS_${test_id}"
}
