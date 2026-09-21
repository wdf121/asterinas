#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

SUITE_ORDER=(
    --control-plane
    --dataplane
    --lvm2-topology
    --linear-integration
    --striped-integration
    --mixed-integration
)

declare -A SUITE_SCRIPTS=(
    [--control-plane]="run_dm_control_plane_test.sh"
    [--dataplane]="run_dm_dataplane_test.sh"
    [--lvm2-topology]="run_lvm2_topology_test.sh"
    [--linear-integration]="dm_linear/run_lvm2_linear_integration_test.sh"
    [--striped-integration]="dm_striped/run_lvm2_striped_integration_test.sh"
    [--mixed-integration]="dm_mixed/run_lvm2_mixed_integration_test.sh"
)

declare -A SUITE_DESCRIPTIONS=(
    [--control-plane]="dmsetup discovery, tables, lifecycle, events, rename, read-only, and removal semantics"
    [--dataplane]="raw linear, striped, mixed, zero, and error target I/O semantics"
    [--lvm2-topology]="PV/VG/LV topology, segment growth, activation, and removal semantics"
    [--linear-integration]="linear LVM2, ext2, same-PV and cross-PV growth, reboot, and shrink"
    [--striped-integration]="striped LVM2, same-set and cross-set growth, ext2, reboot, and shrink"
    [--mixed-integration]="mixed linear-plus-striped LVM2 table, ext2 I/O, and reboot recovery"
)

print_usage() {
    local suite

    echo "Usage: $0 <suite>"
    echo
    echo "Canonical suites:"
    for suite in "${SUITE_ORDER[@]}"; do
        printf '  %-24s %s\n' "${suite}" "${SUITE_DESCRIPTIONS[${suite}]}"
    done
    echo
    echo "Expected success marker:"
    echo "  HOST_PASS_DM_SYSTEM_TESTS <suite>"
}

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    print_usage
    exit 0
fi

if [ "$#" -ne 1 ]; then
    print_usage >&2
    exit 2
fi

SUITE=$1
if [ -z "${SUITE_SCRIPTS[${SUITE}]+x}" ]; then
    print_usage >&2
    exit 2
fi

"${SCRIPT_DIR}/${SUITE_SCRIPTS[${SUITE}]}"
echo HOST_PASS_DM_SYSTEM_TESTS "${SUITE}"
