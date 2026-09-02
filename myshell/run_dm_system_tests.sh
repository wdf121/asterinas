#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    cat <<'EOF'
Usage: myshell/run_dm_system_tests.sh <suite>

Canonical suites:
  --quick                         Run control ABI smoke plus raw linear and striped data regressions.
  --dmsetup-cli                   Run dmsetup CLI control-plane semantics audit with backing disks.
  --lvm2-cli                      Run LVM2 CLI control-plane semantics audit with test disks.
  --dataplane-edge                Run raw DM data-plane edge remap, stripe-boundary, and zero target range audit.
  --linear-data                   Run raw linear cross-target BIO split/remap regression.
  --striped-data                  Run raw striped BIO split/remap and backing distribution regression.
  --linear-lvm2                   Run single-PV LVM2 linear create, same-PV grow/shrink, file I/O, and reboot recovery.
  --striped-lvm2                  Run N-PV LVM2 striped create, same-PV-set grow/shrink, file I/O, and reboot recovery.
  --linear-lvm2-cross-segment     Run independent linear cross-segment table, reboot recovery, and shrink regression.
  --striped-lvm2-cross-segment    Run independent striped N-to-2N cross-segment, reboot recovery, and shrink regression.
  --mixed-lvm2                    Run LVM2 linear + striped mixed table file I/O and reboot recovery regression.

Expected success markers:
  HOST_PASS_DM_SYSTEM_TESTS <suite>
EOF
    exit 0
fi

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

if [ "$#" -ne 1 ]; then
    echo "Usage: $0 [--quick|--dmsetup-cli|--lvm2-cli|--dataplane-edge|--linear-data|--striped-data|--linear-lvm2|--striped-lvm2|--linear-lvm2-cross-segment|--striped-lvm2-cross-segment|--mixed-lvm2]" >&2
    exit 2
fi

SUITE=$1

run_suite() {
    case "$1" in
        --quick)
            "${SCRIPT_DIR}/dm_linear/run_control_abi_test.sh"
            "${SCRIPT_DIR}/dm_linear/run_cross_target_bio_regression.sh"
            "${SCRIPT_DIR}/dm_striped/run_raw_striped_bio_test.sh"
            ;;
        --dmsetup-cli)
            "${SCRIPT_DIR}/run_dmsetup_cli_semantics_test.sh"
            ;;
        --lvm2-cli)
            "${SCRIPT_DIR}/run_lvm2_cli_semantics_test.sh"
            ;;
        --dataplane-edge)
            "${SCRIPT_DIR}/run_dm_dataplane_edge_test.sh"
            ;;
        --linear-data)
            "${SCRIPT_DIR}/dm_linear/run_cross_target_bio_regression.sh"
            ;;
        --striped-data)
            "${SCRIPT_DIR}/dm_striped/run_raw_striped_bio_test.sh"
            ;;
        --linear-lvm2)
            "${SCRIPT_DIR}/dm_linear/run_lvm2_linear_reboot_test.sh"
            ;;
        --striped-lvm2)
            "${SCRIPT_DIR}/dm_striped/run_lvm2_striped_reboot_test.sh"
            ;;
        --linear-lvm2-cross-segment)
            "${SCRIPT_DIR}/dm_linear/run_lvm2_linear_cross_segment_test.sh"
            ;;
        --striped-lvm2-cross-segment)
            "${SCRIPT_DIR}/dm_striped/run_lvm2_striped_cross_segment_test.sh"
            ;;
        --mixed-lvm2)
            "${SCRIPT_DIR}/dm_mixed/run_lvm2_linear_striped_mixed_reboot_test.sh"
            ;;
        *)
            echo "Usage: $0 [--quick|--dmsetup-cli|--lvm2-cli|--dataplane-edge|--linear-data|--striped-data|--linear-lvm2|--striped-lvm2|--linear-lvm2-cross-segment|--striped-lvm2-cross-segment|--mixed-lvm2]" >&2
            exit 2
            ;;
    esac
}

run_suite "${SUITE}"
echo HOST_PASS_DM_SYSTEM_TESTS "${SUITE}"
