// SPDX-License-Identifier: MPL-2.0

//! Single-boot LVM linear LV and ext2 workflow on a persistent raw disk.

use nixos_test_framework::*;

nixos_test_main!();

// This opt-in suite overwrites raw_test_1.img when invoked with raw_disk=1.
// Do not run it on a disk containing data to retain, or alongside another
// suite that expects the first raw disk's contents to remain intact.
const DISK: &str = "/dev/vde";
const VG: &str = "phase1_lvm_vg";
const LV: &str = "phase1_linear";

fn stage(shell: &mut Session, description: &str, body: &str, marker: &str) -> Result<(), Error> {
    // Keep LVM's device scan restricted to this suite's disk; the guest has no
    // udev, so do not wait for udev synchronization or mapper symlinks.
    let command = format!(
        "bash -euo pipefail -c 'lvm() {{ local action=$1; shift; command lvm \"$action\" --config \"activation {{ udev_sync=0 udev_rules=0 }} devices {{ use_devicesfile=0 }}\" \"$@\"; }}; {body}; echo {marker}'"
    );
    shell.run_cmd_and_expect_display(description, &command, marker)
}

#[nixos_test]
fn linear_lv_single_boot(nixos_shell: &mut Session) -> Result<(), Error> {
    stage(
        nixos_shell,
        "verify the first raw disk before writing",
        &format!("test -b {DISK}; test \"$(blockdev --getsize64 {DISK})\" -eq 536870912"),
        "LVM_DISK_READY",
    )?;
    stage(
        nixos_shell,
        "clear the first raw disk",
        &format!("dd if=/dev/zero of={DISK} bs=1M count=512 conv=notrunc status=none; sync"),
        "LVM_DISK_CLEARED",
    )?;

    stage(
        nixos_shell,
        "initialize a physical volume",
        &format!("lvm pvcreate --yes --devices {DISK} {DISK}"),
        "LVM_PVCREATE_OK",
    )?;
    stage(
        nixos_shell,
        "create a volume group on that physical volume",
        &format!("lvm vgcreate --devices {DISK} {VG} {DISK}"),
        "LVM_VGCREATE_OK",
    )?;
    stage(
        nixos_shell,
        "create an inactive ordinary linear logical volume",
        &format!("lvm lvcreate --devices {DISK} --type linear -an -L 256M -n {LV} {VG}"),
        "LVM_LVCREATE_OK",
    )?;
    stage(
        nixos_shell,
        "activate the logical volume without udev",
        &format!("lvm lvchange --devices {DISK} -ay {VG}/{LV}"),
        "LVM_ACTIVATE_OK",
    )?;
    stage(
        nixos_shell,
        "inspect the single-PV linear mapping",
        &format!(
            "lvm pvs --devices {DISK} -o pv_name,vg_name; \
             lvm vgs --devices {DISK} -o vg_name,pv_count,lv_count; \
             lvm lvs --devices {DISK} --segments -o lv_name,seg_start,seg_size,segtype,devices {VG}/{LV}; \
             table=$(dmsetup table {VG}-{LV}); printf \"%s\\n\" \"$table\"; \
             test \"$(printf \"%s\\n\" \"$table\" | wc -l)\" -eq 1; \
             printf \"%s\\n\" \"$table\" | grep -qE \"^0 524288 linear [^[:space:]]+ [0-9]+$\"; \
             dmsetup deps -o devname {VG}-{LV} | grep -q vde"
        ),
        "LVM_LINEAR_TABLE_OK",
    )?;
    stage(
        nixos_shell,
        "read and write through the actual device-mapper primary node",
        &format!(
            "minor=$(dmsetup info -c --noheadings -o minor {VG}-{LV} | tr -d \" \" ); \
             test -n \"$minor\"; node=/dev/dm-$minor; test -b \"$node\"; \
             test \"$(blockdev --getsize64 \"$node\")\" -eq 268435456; \
             printf phase1-lvm-linear | dd of=\"$node\" bs=512 conv=sync,notrunc status=none; \
             printf phase1-lvm-linear | dd bs=512 conv=sync status=none | cmp - <(dd if=\"$node\" bs=512 count=1 status=none); \
             sync"
        ),
        "LVM_LINEAR_IO_OK",
    )?;
    stage(
        nixos_shell,
        "format and mount ext2 on the LVM logical volume",
        &format!(
            "minor=$(dmsetup info -c --noheadings -o minor {VG}-{LV} | tr -d \" \" ); \
             node=/dev/dm-$minor; test -b \"$node\"; \
             mkfs.ext2 -q -F -b 4096 -m 0 \"$node\"; \
             mkdir -p /tmp/phase1_lvm_mnt; \
             mount -t ext2 \"$node\" /tmp/phase1_lvm_mnt; \
             dd if=/dev/urandom of=/tmp/phase1_lvm_mnt/payload bs=1M count=64 conv=fsync status=none; \
             test \"$(stat -c %s /tmp/phase1_lvm_mnt/payload)\" -eq 67108864; \
             sha256sum /tmp/phase1_lvm_mnt/payload | cut -d \" \" -f1 > /tmp/phase1_lvm_payload.sha256; \
             sync; umount /tmp/phase1_lvm_mnt; \
             mount -t ext2 \"$node\" /tmp/phase1_lvm_mnt; \
             test \"$(stat -c %s /tmp/phase1_lvm_mnt/payload)\" -eq 67108864; \
             sha256sum /tmp/phase1_lvm_mnt/payload | cut -d \" \" -f1 | cmp - /tmp/phase1_lvm_payload.sha256; \
             umount /tmp/phase1_lvm_mnt"
        ),
        "LVM_EXT2_IO_OK",
    )?;
    Ok(())
}
