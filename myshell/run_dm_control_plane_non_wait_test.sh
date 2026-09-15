#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
DM_CONTROL_PLANE_SKIP_WAIT=1 exec "${SCRIPT_DIR}/run_dm_control_plane_test.sh" "$@"
