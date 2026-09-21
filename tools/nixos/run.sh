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
# 调用方显式设置 DM_TEST_IMAGES 时附加多个 Device Mapper 测试盘。
# 兼容旧变量 DM_TEST_IMAGE / DM_TEST_IMAGE_2；如果 DM_TEST_IMAGES 为空，
# 则按旧变量拼出测试盘列表。
DM_TEST_IMAGES=${DM_TEST_IMAGES:-}
DM_TEST_IMAGE=${DM_TEST_IMAGE:-}
DM_TEST_IMAGE_2=${DM_TEST_IMAGE_2:-}
if [ -z "${DM_TEST_IMAGES}" ]; then
    if [ -n "${DM_TEST_IMAGE}" ]; then
        DM_TEST_IMAGES="${DM_TEST_IMAGE}"
    fi
    if [ -n "${DM_TEST_IMAGE_2}" ]; then
        DM_TEST_IMAGES="${DM_TEST_IMAGES:+${DM_TEST_IMAGES} }${DM_TEST_IMAGE_2}"
    fi
fi

append_dm_test_image() {
    image_path=$1
    drive_id=$2
    serial=$3
    pci_addr=$4

    # 每次 NixOS 启动均附加独立的 Device Mapper 测试盘。已有镜像必须原样
    # 复用，以便跨 QEMU 启动验证 LVM 元数据和文件数据的持久性。
    if [ ! -e "${image_path}" ]; then
        echo "Creating Device Mapper test image at ${image_path} (512 MiB)..."
        mkdir -p "$(dirname "${image_path}")"
        fallocate -l 512M "${image_path}"
    fi

    if [ ! -f "${image_path}" ]; then
        echo "Error: ${image_path} 不是普通文件" >&2
        exit 1
    fi

    image_path=$(realpath "${image_path}")
    case "${image_path}" in
        *[[:space:]]*)
            echo "Error: 测试盘路径不能包含空白字符: ${image_path}" >&2
            exit 1
            ;;
    esac

    if [ "${image_path}" = "${ASTERINAS_DIR}/target/nixos/asterinas.img" ]; then
        echo "Error: DM 测试盘不能指向 NixOS 根磁盘" >&2
        exit 1
    fi

    case " ${DM_TEST_IMAGE_REALPATHS:-} " in
        *" ${image_path} "*)
            echo "Error: 不能重复附加同一个 DM 测试盘: ${image_path}" >&2
            exit 1
            ;;
    esac
    DM_TEST_IMAGE_REALPATHS="${DM_TEST_IMAGE_REALPATHS:-} ${image_path}"

    echo "Attaching Device Mapper test image: path=${image_path} serial=${serial} drive_id=${drive_id}"

    QEMU_ARGS="${QEMU_ARGS} \
        -drive if=none,format=raw,id=${drive_id},file=${image_path},cache=none \
        -device virtio-blk-pci,bus=pcie.0,addr=${pci_addr},drive=${drive_id},serial=${serial},disable-legacy=on,disable-modern=off \
    "
}

append_dm_test_images() {
    disk_index=1
    for image_path in ${DM_TEST_IMAGES}; do
        if [ "${disk_index}" -eq 1 ]; then
            drive_id=dmtest
            serial=vdmtest
        else
            drive_id=dmtest${disk_index}
            serial=vdmtest${disk_index}
        fi
        pci_addr=$(printf '0x%x' $((0xb + disk_index)))
        append_dm_test_image "${image_path}" "${drive_id}" "${serial}" "${pci_addr}"
        disk_index=$((disk_index + 1))
    done
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
        NIXOS_DISK_SIZE_IN_MB=${NIXOS_DISK_SIZE_IN_MB:-16384}
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

if [ -n "${DM_TEST_IMAGES}" ]; then
    append_dm_test_images
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
