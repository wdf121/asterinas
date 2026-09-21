#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

set -e

SCRIPT_DIR=/test

if [ "$#" -eq 0 ]; then
    test_targets=$(find -L "${SCRIPT_DIR}" -mindepth 1 -maxdepth 1 -type d)
else
    test_targets=
    for selector in "$@"; do
        case "${selector}" in
            ""|/*|.*|*" "*|*"//"*|*"/."*)
                echo "Invalid regression test selector: ${selector}" >&2
                exit 2
                ;;
        esac
        target="${SCRIPT_DIR}/${selector}"
        if [ -d "${target}" ]; then
            if [ ! -x "${target}/run_test.sh" ]; then
                echo "Regression test directory has no executable run_test.sh: ${selector}" >&2
                exit 2
            fi
        elif [ ! -x "${target}" ]; then
            echo "Unknown or non-executable regression test: ${selector}" >&2
            exit 2
        fi
        test_targets="${test_targets} ${target}"
    done
fi

for target in ${test_targets}; do
    if [ -d "${target}" ]; then
        echo "Running test in ${target}"
        (cd "${target}" && ./run_test.sh)
        echo "All test in ${target} passed."
    else
        echo "Running test ${target}"
        "${target}"
        echo "Test ${target} passed."
    fi
done

echo "All regression tests passed."
