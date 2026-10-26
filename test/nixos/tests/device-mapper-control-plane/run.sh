#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

# Keep QEMU supervision inside the official framework's PTY session.
exec python3 "$(dirname "$0")/runner.py" --guest
