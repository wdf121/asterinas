#!/bin/sh
# SPDX-License-Identifier: MPL-2.0

# Prepare the same numbered, persistent raw disks for both official VM runners.
set -eu

case "${1-}" in
    0|[1-9]|1[0-8]) ;;
    *) echo "Error: raw disk count must be between 0 and 18" >&2; exit 1 ;;
esac
count=$1
[ "$count" -gt 0 ] || exit 0

SCRIPT_DIR=$(dirname "$0")
ROOT=$(realpath "${SCRIPT_DIR}/..")
DISK_DIR="${ROOT}/test/initramfs/build"
LOCK="${ROOT}/test/initramfs/build/.raw-test-disks.lock"
if [ -L "${DISK_DIR}" ]; then
    echo "Error: raw test disk directory is a symlink" >&2
    exit 1
fi
mkdir -p "${DISK_DIR}"
exec 9>"${LOCK}"
if ! flock -n 9; then
    echo "Error: raw test disks are in use by another VM or preparer" >&2
    exit 1
fi

index=1
while [ "$index" -le "$count" ]; do
    image="${DISK_DIR}/raw_test_${index}.img"
    if [ -L "$image" ]; then
        echo "Error: raw test disk is a symlink: ${image}" >&2
        exit 1
    elif [ -e "$image" ]; then
        if [ ! -f "$image" ] || [ "$(stat -c %s "$image")" -ne 536870912 ]; then
            echo "Error: raw test disk must be a 512 MiB regular file: ${image}" >&2
            exit 1
        fi
    else
        truncate -s 512M "$image"
    fi
    index=$((index + 1))
done
