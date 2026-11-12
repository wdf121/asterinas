// SPDX-License-Identifier: MPL-2.0

//! Verifies the LVM workflow's retained volume after a separate VM boot.
//!
//! Requires a successful device-mapper-lvm run on the same persistent raw disks.
//! This suite explicitly discovers and activates existing metadata. It does not
//! prepare disks, recreate volumes, or test boot-time automatic activation.

use nixos_test_framework::*;

nixos_test_main!();

#[nixos_test]
fn linear_lv_recover_after_reboot(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-lvm-recovery; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    // A fresh guest without unrelated mappings makes /dev/dm-0 deterministic.
    // Fail rather than hide automatic activation or reuse an existing mapping.
    nixos_shell.run_cmd_and_expect_regex("test ! -e /dev/dm-0", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect("blockdev --getsize64 /dev/vde", "536870912")?;
    nixos_shell.run_cmd_and_expect("blockdev --getsize64 /dev/vdf", "536870912")?;

    // Rebuild userspace discovery state from the PV metadata, not a prior cache.
    nixos_shell.run_cmd_and_expect_regex("pvscan", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("vgscan", &Regex::new("phase1_offline_vg").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "pvs --noheadings -o pv_name,vg_name /dev/vde /dev/vdf",
        &Regex::new("(?s)/dev/vde\\s+phase1_offline_vg.*?/dev/vdf\\s+phase1_offline_vg").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "lvchange -ay phase1_offline_vg/ext2_lv",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect("blockdev --getsize64 /dev/dm-0", "201326592")?;
    nixos_shell.run_cmd_and_expect("dmsetup table phase1_offline_vg-ext2_lv", "0 393216 linear")?;
    nixos_shell.run_cmd_and_expect_regex(
        "e2fsck -f -n /dev/dm-0",
        &Regex::new("(?s)Pass 5: Checking group summary information.*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect(
        "dumpe2fs -h /dev/dm-0 2>/dev/null | grep 'Block count:'",
        "49152",
    )?;

    // Read-only verification consumes the original on-disk checksum baselines.
    nixos_shell.run_cmd("mkdir -p \"${dm_work}/phase1_recovery_mnt\"")?;
    nixos_shell.run_cmd_and_expect_regex(
        "mount -t ext2 -o ro /dev/dm-0 \"${dm_work}/phase1_recovery_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect(
        "stat -c %s \"${dm_work}/phase1_recovery_mnt\"/retained",
        "50331648",
    )?;
    nixos_shell.run_cmd_and_expect(
        "stat -c %s \"${dm_work}/phase1_recovery_mnt\"/after-shrink",
        "33554432",
    )?;
    nixos_shell.run_cmd_and_expect_regex("(cd \"${dm_work}/phase1_recovery_mnt\" && sha256sum -c retained.sha256 after-shrink.sha256)",
        &Regex::new("(?s)retained: OK.*after-shrink: OK").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("test ! -e \"${dm_work}/phase1_recovery_mnt\"/added && test ! -e \"${dm_work}/phase1_recovery_mnt\"/added.sha256",
        &Regex::new("(?s).*").unwrap(),
    )?;

    // Keep the prepared state intact so verification can be repeated next boot.
    nixos_shell.run_cmd_and_expect_regex(
        "umount \"${dm_work}/phase1_recovery_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "lvchange -an phase1_offline_vg/ext2_lv",
        &Regex::new("(?s).*").unwrap(),
    )?;
    Ok(())
}
