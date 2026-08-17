#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
DM_TEST_IMAGE=${DM_TEST_IMAGE:-target/nixos/test.img}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-target/nixos/test2.img}

cd "${SCRIPT_DIR}"

echo "HOST_INFO_NIXOS_DM_TEST_DISKS disk1=${DM_TEST_IMAGE} serial=vdmtest"
echo "HOST_INFO_NIXOS_DM_TEST_DISKS disk2=${DM_TEST_IMAGE_2} serial=vdmtest2"

DM_TEST_IMAGE="${DM_TEST_IMAGE}" \
DM_TEST_IMAGE_2="${DM_TEST_IMAGE_2}" \
make run_nixos
