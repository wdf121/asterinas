// SPDX-License-Identifier: MPL-2.0

//! LVM workflows: create and use a linear LV, then resize ext2 offline.
//!
//! The create/use case cleans up its VG and PV. The offline resize case leaves
//! an inactive LV with files and checksums for verification in a later boot.
//! Concurrent I/O and cross-boot verification are outside this suite's scope.

use nixos_test_framework::*;

nixos_test_main!(order = ["linear_lv_create_and_use", "linear_lv_offline_ext2_resize",]);

// The full suite requires raw_disk=2 and erases raw_test_1.img and raw_test_2.img.
// Do not rerun it between preparation and cross-boot verification.
// A clean guest and successful preceding cleanup make /dev/dm-0 deterministic.
// After a failure, inspect leftover mounts and mappings before retrying.

// ============================================================================
// Create and use a fixed-size linear LV
// ============================================================================

#[nixos_test]
fn linear_lv_create_and_use(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-lvm; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "blockdev --getsize64 /dev/vde",
        &Regex::new("536870912").unwrap(),
    )?;

    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/zero of=/dev/vde bs=1M count=512 conv=notrunc status=none && sync",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("pvcreate --yes /dev/vde", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "vgcreate phase1_lvm_vg /dev/vde",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "lvcreate --type linear -an -L 256M -n phase1_linear phase1_lvm_vg",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "lvchange -ay phase1_lvm_vg/phase1_linear",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup table phase1_lvm_vg-phase1_linear",
        &Regex::new("0\\ 524288\\ linear").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup deps -o devname phase1_lvm_vg-phase1_linear",
        &Regex::new("vde").unwrap(),
    )?;

    nixos_shell.run_cmd_and_expect_regex(
        "blockdev --getsize64 /dev/dm-0",
        &Regex::new("268435456").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "printf phase1-lvm-linear | dd of=/dev/dm-0 bs=512 conv=sync,notrunc status=none && sync",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/dm-0 bs=512 count=1 status=none | grep -ao phase1-lvm-linear",
        &Regex::new("phase1\\-lvm\\-linear").unwrap(),
    )?;

    // Verify the file survives unmount and remount on a fixed-size LV.
    nixos_shell.run_cmd_and_expect_regex(
        "mkfs.ext2 -q -F -b 4096 -m 0 /dev/dm-0",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mkdir -p \"${dm_work}/phase1_lvm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mount -t ext2 /dev/dm-0 \"${dm_work}/phase1_lvm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/phase1_lvm_mnt\"/payload bs=1M count=64 conv=fsync",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "stat -c %s \"${dm_work}/phase1_lvm_mnt\"/payload",
        &Regex::new("67108864").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("sha256sum \"${dm_work}/phase1_lvm_mnt\"/payload > \"${dm_work}/phase1_lvm_payload.sha256\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("sync", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "umount \"${dm_work}/phase1_lvm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mount -t ext2 /dev/dm-0 \"${dm_work}/phase1_lvm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "stat -c %s \"${dm_work}/phase1_lvm_mnt\"/payload",
        &Regex::new("67108864").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "sha256sum -c \"${dm_work}/phase1_lvm_payload.sha256\"",
        &Regex::new("phase1_lvm_mnt/payload:\\ OK").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "umount \"${dm_work}/phase1_lvm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "lvchange -an phase1_lvm_vg/phase1_linear",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "vgremove --yes phase1_lvm_vg",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("pvremove --yes /dev/vde", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

// ============================================================================
// Offline ext2 growth across PVs and shrink back to the original size
// ============================================================================

#[nixos_test]
fn linear_lv_offline_ext2_resize(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-lvm; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "blockdev --getsize64 /dev/vde",
        &Regex::new("536870912").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "blockdev --getsize64 /dev/vdf",
        &Regex::new("536870912").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/zero of=/dev/vde bs=1M count=512 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/zero of=/dev/vdf bs=1M count=512 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "pvcreate --yes /dev/vde /dev/vdf",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "vgcreate phase1_offline_vg /dev/vde",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "lvcreate --type linear -an -L 192M -n ext2_lv phase1_offline_vg /dev/vde",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "lvchange -ay phase1_offline_vg/ext2_lv",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "blockdev --getsize64 /dev/dm-0",
        &Regex::new("201326592").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mkfs.ext2 -q -F -b 4096 -m 0 /dev/dm-0",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mkdir -p \"${dm_work}/phase1_offline_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mount -t ext2 /dev/dm-0 \"${dm_work}/phase1_offline_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=/dev/urandom of=\"${dm_work}/phase1_offline_mnt\"/retained bs=1M count=48 conv=fsync",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "stat -c %s \"${dm_work}/phase1_offline_mnt\"/retained",
        &Regex::new("50331648").unwrap(),
    )?;
    // Relative paths let a later boot verify the original baseline at any mount point.
    nixos_shell.run_cmd_and_expect_regex(
        "(cd \"${dm_work}/phase1_offline_mnt\" && sha256sum retained > retained.sha256)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "umount \"${dm_work}/phase1_offline_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;

    // The trailing PV argument forces the extension onto the second disk.
    // Grow the unmounted LV before growing ext2.
    nixos_shell.run_cmd_and_expect_regex(
        "vgextend phase1_offline_vg /dev/vdf",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "lvextend -L 256M phase1_offline_vg/ext2_lv /dev/vdf",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "blockdev --getsize64 /dev/dm-0",
        &Regex::new("268435456").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "lvs --segments -o seg_start,seg_size,segtype,devices phase1_offline_vg/ext2_lv",
        &Regex::new("/dev/vdf\\(").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "e2fsck -f -n /dev/dm-0",
        &Regex::new("(?s)Pass 5: Checking group summary information.*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("resize2fs /dev/dm-0", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "e2fsck -f -n /dev/dm-0",
        &Regex::new("(?s)Pass 5: Checking group summary information.*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dumpe2fs -h /dev/dm-0 2>/dev/null | grep 'Block count:'",
        &Regex::new("65536").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mount -t ext2 /dev/dm-0 \"${dm_work}/phase1_offline_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "(cd \"${dm_work}/phase1_offline_mnt\" && sha256sum -c retained.sha256)",
        &Regex::new("retained:\\ OK").unwrap(),
    )?;
    // Non-sparse payload totals 208 MiB, exceeding the first PV's 192 MiB segment.
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/phase1_offline_mnt\"/added bs=1M count=160 conv=fsync",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "stat -c %s \"${dm_work}/phase1_offline_mnt\"/added",
        &Regex::new("167772160").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "(cd \"${dm_work}/phase1_offline_mnt\" && sha256sum added > added.sha256)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "umount \"${dm_work}/phase1_offline_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mount -t ext2 /dev/dm-0 \"${dm_work}/phase1_offline_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "(cd \"${dm_work}/phase1_offline_mnt\" && sha256sum -c retained.sha256 added.sha256)",
        &Regex::new("(?s)retained: OK.*added: OK").unwrap(),
    )?;

    // Remove the growth payload, then shrink ext2 before reducing the LV.
    nixos_shell.run_cmd_and_expect_regex(
        "rm \"${dm_work}/phase1_offline_mnt\"/added \"${dm_work}/phase1_offline_mnt\"/added.sha256",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "umount \"${dm_work}/phase1_offline_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "e2fsck -f -n /dev/dm-0",
        &Regex::new("(?s)Pass 5: Checking group summary information.*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("resize2fs /dev/dm-0 192M", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dumpe2fs -h /dev/dm-0 2>/dev/null | grep 'Block count:'",
        &Regex::new("49152").unwrap(),
    )?;
    // ext2 was already shrunk above; avoid LVM's filesystem probe via VG/LV aliases.
    nixos_shell.run_cmd_and_expect_regex(
        "lvreduce --fs ignore --yes -L 192M phase1_offline_vg/ext2_lv",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "blockdev --getsize64 /dev/dm-0",
        &Regex::new("201326592").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "e2fsck -f -n /dev/dm-0",
        &Regex::new("(?s)Pass 5: Checking group summary information.*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mount -t ext2 /dev/dm-0 \"${dm_work}/phase1_offline_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "(cd \"${dm_work}/phase1_offline_mnt\" && sha256sum -c retained.sha256)",
        &Regex::new("retained:\\ OK").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "test ! -e \"${dm_work}/phase1_offline_mnt\"/added",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=/dev/urandom of=\"${dm_work}/phase1_offline_mnt\"/after-shrink bs=1M count=32 conv=fsync",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "stat -c %s \"${dm_work}/phase1_offline_mnt\"/after-shrink",
        &Regex::new("33554432").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "(cd \"${dm_work}/phase1_offline_mnt\" && sha256sum after-shrink > after-shrink.sha256)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "umount \"${dm_work}/phase1_offline_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mount -t ext2 /dev/dm-0 \"${dm_work}/phase1_offline_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("(cd \"${dm_work}/phase1_offline_mnt\" && sha256sum -c retained.sha256 after-shrink.sha256)",
        &Regex::new("(?s)retained: OK.*after-shrink: OK").unwrap(),
    )?;

    // Preserve both PVs, the VG, the LV, and on-disk baselines for a later boot.
    nixos_shell.run_cmd_and_expect_regex(
        "umount \"${dm_work}/phase1_offline_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "lvchange -an phase1_offline_vg/ext2_lv",
        &Regex::new("(?s).*").unwrap(),
    )?;
    Ok(())
}
