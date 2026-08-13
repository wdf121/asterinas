#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

# Run a NixOS installation or NixOS ISO installer image built by the root Makefile inside a VM
#
# Usage: ./run.sh [nixos | iso]

set -e

usage() {
    echo "Usage: $0 [nixos | iso]"
    exit 1
}

if [ "$#" -ne 1 ]; then
    usage
fi

MODE=$1
TARGET_ARCH=${TARGET_ARCH:-x86_64}
SCRIPT_DIR=$(dirname "$0")
ASTERINAS_DIR=$(realpath "${SCRIPT_DIR}/../..")
# 调用方可覆盖路径；默认测试盘与 NixOS 根盘并列但独立保存。
DM_TEST_IMAGE=${DM_TEST_IMAGE:-"${ASTERINAS_DIR}/target/nixos/test.img"}

append_dm_test_image() {
    # 每次 NixOS 启动均附加独立的 Device Mapper 测试盘。已有镜像必须原样
    # 复用，以便跨 QEMU 启动验证 LVM 元数据和文件数据的持久性。
    if [ ! -e "${DM_TEST_IMAGE}" ]; then
        echo "Creating Device Mapper test image at ${DM_TEST_IMAGE} (512 MiB)..."
        mkdir -p "$(dirname "${DM_TEST_IMAGE}")"
        fallocate -l 512M "${DM_TEST_IMAGE}"
    fi

    if [ ! -f "${DM_TEST_IMAGE}" ]; then
        echo "Error: DM_TEST_IMAGE 不是普通文件: ${DM_TEST_IMAGE}" >&2
        exit 1
    fi

    DM_TEST_IMAGE=$(realpath "${DM_TEST_IMAGE}")
    case "${DM_TEST_IMAGE}" in
        *[[:space:]]*)
            echo "Error: DM_TEST_IMAGE 路径不能包含空白字符: ${DM_TEST_IMAGE}" >&2
            exit 1
            ;;
    esac

    if [ "${DM_TEST_IMAGE}" = "${ASTERINAS_DIR}/target/nixos/asterinas.img" ]; then
        echo "Error: DM_TEST_IMAGE 不能指向 NixOS 根磁盘" >&2
        exit 1
    fi

    # 这里只附加调用方预先创建的 raw 镜像；脚本绝不创建、截断或调整它。
    QEMU_ARGS="${QEMU_ARGS} \
        -drive if=none,format=raw,id=dmtest,file=${DM_TEST_IMAGE},cache=none \
        -device virtio-blk-pci,bus=pcie.0,addr=0xc,drive=dmtest,serial=vdmtest,disable-legacy=on,disable-modern=off \
    "
}

# tools/qemu_args.sh currently emits x86_64-specific arguments.
# Reject other architectures to avoid invoking non-x86 QEMU with incompatible args.
if [ "${TARGET_ARCH}" != "x86_64" ]; then
    echo "Error: tools/nixos/run.sh currently supports only TARGET_ARCH=x86_64; got ${TARGET_ARCH}" >&2
    exit 1
fi

# Change to Asterinas root directory to ensure all scripts run from the correct location.
cd "${ASTERINAS_DIR}"

# Get base QEMU arguments from qemu_args.sh script
# NixOS 根镜像是带 ESP 的 UEFI 安装；不依赖调用方的通用启动方式选择。
QEMU_ARGS=$(FORCE_OVMF=on ${ASTERINAS_DIR}/tools/qemu_args.sh common 2>/dev/null)

# Add mode-specific disk and device arguments
case "$MODE" in
    nixos)
        NIXOS_DIR="${ASTERINAS_DIR}/target/nixos"
        QEMU_ARGS="${QEMU_ARGS} \
            -drive if=none,format=raw,id=u0,file=${NIXOS_DIR}/asterinas.img \
            -device virtio-blk-pci,drive=u0,bootindex=1,disable-legacy=on,disable-modern=off \
        "
        ;;
    iso)
        ASTER_IMAGE_PATH=${ASTERINAS_DIR}/target/nixos/asterinas.img
        NIXOS_DISK_SIZE_IN_MB=${NIXOS_DISK_SIZE_IN_MB:-8192}
        ISO_IMAGE_PATH=$(find "${ASTERINAS_DIR}/target/nixos/iso_image/iso" -name "*.iso" | head -n 1)

        if [ ! -f "$ISO_IMAGE_PATH" ]; then
            echo "Error: ISO_IMAGE not found!"
            exit 1
        fi

        rm -f "${ASTER_IMAGE_PATH}"
        echo "Creating image at ${ASTER_IMAGE_PATH} of size ${NIXOS_DISK_SIZE_IN_MB}MB......"
        dd if=/dev/zero of="${ASTER_IMAGE_PATH}" bs=1M count=${NIXOS_DISK_SIZE_IN_MB} status=none
        echo "Image created successfully!"

        QEMU_ARGS="${QEMU_ARGS} \
            -cdrom ${ISO_IMAGE_PATH} -boot d \
            -drive if=none,format=raw,id=u0,file=${ASTER_IMAGE_PATH} \
            -device virtio-blk-pci,drive=u0,disable-legacy=on,disable-modern=off \
        "
        ;;
    *)
        usage
        ;;
esac

append_dm_test_image

if [ "${ENABLE_KVM}" = "1" ]; then
    QEMU_ARGS="${QEMU_ARGS} -accel kvm"
fi

QEMU_BIN=${QEMU_BIN:-qemu-system-${TARGET_ARCH}}

# The kernel uses a specific value to signal a successful shutdown via the
# isa-debug-exit device.
KERNEL_SUCCESS_EXIT_CODE=16 # 0x10 in hexadecimal
# QEMU translates the value written to the isa-debug-exit device into a final
# process exit code using following formula.
QEMU_SUCCESS_EXIT_CODE=$(((KERNEL_SUCCESS_EXIT_CODE << 1) | 1))

# Execute QEMU
# shellcheck disable=SC2086
${QEMU_BIN} ${QEMU_ARGS} || exit_code=$?
exit_code=${exit_code:-0}

# Check if the execution was successful:
# - Exit code 0: Normal successful exit (e.g., ACPI shutdown or clean termination)
# - Exit code $QEMU_SUCCESS_EXIT_CODE: Kernel signaled success via isa-debug-exit device
if [ ${exit_code} -eq 0 ] || [ ${exit_code} -eq ${QEMU_SUCCESS_EXIT_CODE} ]; then
    exit 0
fi

exit ${exit_code}