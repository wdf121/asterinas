#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: myshell/ktest_crate.sh <crate-dir> [cargo-osdk-test-filter-or-args...]

Examples:
  myshell/ktest_crate.sh kernel/core/comps/device-mapper aster_device_mapper::table::tests::<test_name>
  myshell/ktest_crate.sh kernel/core aster_core::device::misc::device_mapper::tests::<test_name>

Environment overrides:
  KTEST_LOGLEVEL=error
  KTEST_TIMEOUT=180s
  KTEST_TIMEOUT_KILL=10s
  RELEASE=1
  BOOT_METHOD=grub-rescue-iso
  BOOT_PROTOCOL=multiboot2
  ENABLE_KVM=1
  KTEST_CONSOLE=hvc0
  INITRAMFS=/root/asterinas/test/initramfs/build/initramfs.cpio.gz
EOF
}

strip_ansi() {
    if command -v perl >/dev/null 2>&1; then
        perl -pe 's/\e\[[0-9;]*[A-Za-z]//g'
    else
        cat
    fi
}

generate_result_log() {
    local raw_log=$1
    local result_log=$2

    if [ ! -f "${raw_log}" ]; then
        echo "ktest result log unavailable: ${raw_log} was not created" >"${result_log}"
        return
    fi

    strip_ansi <"${raw_log}" | awk '
        { sub(/\r$/, "") }
        /^running [0-9]+ tests in crate "/ { started = 1; print; next }
        !started { next }
        /^test .* \.\.\.$/ { next }
        /^test .* \.\.\. (ok|FAILED)$/ { print; next }
        /^test result: / { print; next }
        /^failures:/ { print; next }
        /^---- / { print; next }
        /^\[caught panic\]/ { print; next }
        /^test did not panic as expected/ { print; next }
        /^expected: / { print; next }
        /^caught: / { print; next }
        /^\[ktest runner\] All crates tested\./ { print "All crates tested."; next }
    ' >"${result_log}"

    if [ ! -s "${result_log}" ]; then
        echo "ktest result log unavailable: no ktest result marker found in ${raw_log}" >"${result_log}"
    fi
}

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    usage
    exit 0
fi

if [ "$#" -lt 1 ]; then
    usage >&2
    exit 2
fi

CRATE_DIR=${1%/}
while [[ "${CRATE_DIR}" == */ ]]; do
    CRATE_DIR=${CRATE_DIR%/}
done
shift

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "${SCRIPT_DIR}/.." && pwd)
CRATE_PATH="${REPO_ROOT}/${CRATE_DIR}"

if [ ! -f "${CRATE_PATH}/Cargo.toml" ]; then
    echo "ktest_crate: crate directory does not contain Cargo.toml: ${CRATE_DIR}" >&2
    exit 2
fi

KTEST_LOGLEVEL=${KTEST_LOGLEVEL:-error}
KTEST_TIMEOUT=${KTEST_TIMEOUT:-180s}
KTEST_TIMEOUT_KILL=${KTEST_TIMEOUT_KILL:-10s}
RELEASE=${RELEASE:-1}
BOOT_METHOD=${BOOT_METHOD:-grub-rescue-iso}
BOOT_PROTOCOL=${BOOT_PROTOCOL:-multiboot2}
ENABLE_KVM=${ENABLE_KVM:-1}
KTEST_CONSOLE=${KTEST_CONSOLE:-hvc0}
INITRAMFS=${INITRAMFS:-${REPO_ROOT}/test/initramfs/build/initramfs.cpio.gz}

RESULT_LOG="${CRATE_PATH}/ktest.log"
QEMU_RAW_LOG="${REPO_ROOT}/qemu.log"
QEMU_SERIAL_RAW_LOG="${REPO_ROOT}/qemu-serial.log"

cargo_args=()

if [ "${RELEASE}" = "1" ]; then
    cargo_args+=(--release)
fi

cargo_args+=(
    --kcmd-args="loglevel=${KTEST_LOGLEVEL}"
    --kcmd-args=earlycon
    --kcmd-args="console=${KTEST_CONSOLE}"
    --boot-method="${BOOT_METHOD}"
    --grub-boot-protocol="${BOOT_PROTOCOL}"
)

if [ "${ENABLE_KVM}" = "1" ]; then
    cargo_args+=(--qemu-args="-accel kvm")
fi

cargo_args+=(--initramfs="${INITRAMFS}")

rm -f "${RESULT_LOG}" "${QEMU_RAW_LOG}" "${QEMU_SERIAL_RAW_LOG}"

echo "ktest_crate: result log: ${RESULT_LOG}" >&2

export CONSOLE="${KTEST_CONSOLE}"

cd "${CRATE_PATH}"
set +e
timeout --foreground -k "${KTEST_TIMEOUT_KILL}" "${KTEST_TIMEOUT}" cargo osdk test "${cargo_args[@]}" "$@"
status=$?
set -e

raw_result_log="${QEMU_SERIAL_RAW_LOG}"
if [ ! -s "${raw_result_log}" ]; then
    raw_result_log="${QEMU_RAW_LOG}"
fi

generate_result_log "${raw_result_log}" "${RESULT_LOG}"
rm -f "${QEMU_RAW_LOG}" "${QEMU_SERIAL_RAW_LOG}"

exit "${status}"
