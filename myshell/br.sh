#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
DM_TEST_IMAGES=${DM_TEST_IMAGES:-target/nixos/test.img target/nixos/test2.img}

cd "${SCRIPT_DIR}"

disk_index=1
for image_path in ${DM_TEST_IMAGES}; do
    if [ "${disk_index}" -eq 1 ]; then
        serial=vdmtest
    else
        serial=vdmtest${disk_index}
    fi
    echo "HOST_INFO_NIXOS_DM_TEST_DISKS disk${disk_index}=${image_path} serial=${serial}"
    disk_index=$((disk_index + 1))
done

DM_TEST_IMAGES="${DM_TEST_IMAGES}" make run_nixos
