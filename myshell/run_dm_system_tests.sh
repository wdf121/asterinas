#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/run_dm_system_tests.sh [--quick|--data|--striped|--striped-lvm2|--striped-lvm2-3pv|--lvm2|--linear-flow|--full]

Suites:
  --quick         Run linear control ABI smoke and raw cross-target BIO regression.
  --data          Run raw cross-target BIO regression only.
  --striped       Run raw dm_striped BIO split/remap regression.
  --striped-lvm2      Run LVM2 striped create, resize, file I/O, and reboot recovery regression.
  --striped-lvm2-3pv  Run 3PV / 3-way LVM2 striped file I/O and reboot recovery regression.
  --lvm2          Run cross-PV large-file and LVM2 resize regressions.
  --linear-flow   Run the single-guest linear end-to-end flow.
  --full          Run all linear DM system regressions. This is the default.
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
SUITE=${1:---full}

run_suite() {
    case "$1" in
        --quick)
            "${SCRIPT_DIR}/dm_linear/run_control_abi_test.sh"
            "${SCRIPT_DIR}/dm_linear/run_cross_target_bio_regression.sh"
            ;;
        --data)
            "${SCRIPT_DIR}/dm_linear/run_cross_target_bio_regression.sh"
            ;;
        --striped)
            "${SCRIPT_DIR}/dm_striped/run_raw_striped_bio_test.sh"
            ;;
        --striped-lvm2)
            "${SCRIPT_DIR}/dm_striped/run_lvm2_striped_io_reboot_test.sh"
            ;;
        --striped-lvm2-3pv)
            "${SCRIPT_DIR}/dm_striped/run_lvm2_striped_3pv_reboot_test.sh"
            ;;
        --lvm2)
            "${SCRIPT_DIR}/dm_linear/run_cross_pv_large_write_test.sh"
            "${SCRIPT_DIR}/dm_linear/run_lvm2_resize_test.sh"
            ;;
        --linear-flow)
            "${SCRIPT_DIR}/dm_linear/run_linear_full_flow_test.sh"
            ;;
        --full)
            "${SCRIPT_DIR}/dm_linear/run_control_abi_test.sh"
            "${SCRIPT_DIR}/dm_linear/run_cross_target_bio_regression.sh"
            "${SCRIPT_DIR}/dm_linear/run_cross_pv_large_write_test.sh"
            "${SCRIPT_DIR}/dm_linear/run_lvm2_resize_test.sh"
            "${SCRIPT_DIR}/dm_linear/run_linear_full_flow_test.sh"
            ;;
        *)
            echo "Usage: $0 [--quick|--data|--striped|--striped-lvm2|--striped-lvm2-3pv|--lvm2|--linear-flow|--full]" >&2
            exit 2
            ;;
    esac
}

run_suite "${SUITE}"
echo HOST_PASS_DM_SYSTEM_TESTS "${SUITE}"
