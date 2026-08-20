#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/run_dm_system_tests.sh [--quick|--data|--lvm2|--linear-flow|--full]

Suites:
  --quick        Run linear control ABI smoke and raw cross-target BIO regression.
  --data         Run raw cross-target BIO regression only.
  --lvm2         Run cross-PV large-file and LVM2 resize regressions.
  --linear-flow  Run the single-guest linear end-to-end flow.
  --full         Run all linear DM system regressions. This is the default.
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
            echo "Usage: $0 [--quick|--data|--lvm2|--linear-flow|--full]" >&2
            exit 2
            ;;
    esac
}

run_suite "${SUITE}"
echo HOST_PASS_DM_SYSTEM_TESTS "${SUITE}"
