// SPDX-License-Identifier: MPL-2.0

//! dmsetup semantics, linear and striped I/O, ext2, and raw table replacement.
//!
//! Each case prepares and removes its own mapping and can be selected separately.
//! Requires raw_disk=2 and a clean guest with no unrelated DM mappings. The cases
//! overwrite backing ranges on /dev/vde and /dev/vdf; never run between LVM
//! preparation and recovery verification. Successful cleanup allows /dev/dm-0
//! reuse. After failure, inspect mounts and mappings before retrying.

use nixos_test_framework::*;

nixos_test_main!(
    order = [
        "interface_and_target_probes",
        "create_and_first_load",
        "numeric_backing_load",
        "first_linear_resume_reads_and_writes",
        "query_table_states",
        "clear_pending_table_preserves_active_mapping",
        "clear_without_pending_table_and_remove",
        "suspend_and_resume_linear_mapping",
        "remove_mapping_lifecycle",
        "failed_load_preserves_active_mapping",
        "readonly_and_io_bounds",
        "multi_target_linear_reads_and_writes",
        "multi_disk_linear_reads_and_writes",
        "ext2_multi_target_same_disk",
        "ext2_multi_target_cross_disk",
        "implicit_table_replacement",
        "explicit_table_replacement",
        "raw_mapping_growth_and_shrink",
        "concurrent_hot_table_replacement",
        "striped_cross_disk_reads_and_writes",
        "striped_same_disk_reads_and_writes",
        "mixed_linear_and_striped_reads_and_writes",
        "invalid_striped_load_preserves_tables",
        "striped_readonly_and_io_bounds",
        "striped_table_replacement",
        "striped_raw_mapping_growth_and_shrink",
        "ext2_striped_cross_disk",
    ]
);

// Probe the driver and advertised target capabilities.
#[nixos_test]
fn interface_and_target_probes(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup version", &Regex::new("Driver\\ version:").unwrap())?;

    nixos_shell
        .run_cmd_and_expect_regex("dmsetup targets", &Regex::new("(?m)^linear\\s").unwrap())?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup targets", &Regex::new("(?m)^striped\\s").unwrap())?;

    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup target-version striped",
        &Regex::new("1\\.6\\.0").unwrap(),
    )?;
    Ok(())
}

// CREATE reserves the identity; the first LOAD publishes the primary data node.
#[nixos_test]
fn create_and_first_load(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("test ! -e /dev/dm-0", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --uuid phase1_test_uuid --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup info phase1_test",
        &Regex::new("phase1_test_uuid").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("test ! -e /dev/dm-0", &Regex::new("(?s).*").unwrap())?;

    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("test -b /dev/dm-0", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("test \"$(stat -c %t /dev/dm-0)\" = \"$(awk '/device-mapper/ { printf \"%x\", $1 }' /proc/devices)\"", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("stat -c %T /dev/dm-0", &Regex::new("0").unwrap())?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

// Resolve the backing device number at runtime and verify mapped I/O.
#[nixos_test]
fn numeric_backing_load(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("read -r major minor < <(stat -c \"%t %T\" /dev/vde); dmsetup load phase1_test --table \"0 8 linear $((16#$major)):$((16#$minor)) 4096\"", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("test -b /dev/dm-0", &Regex::new("(?s).*").unwrap())?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_expected\" bs=4096 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/vde bs=512 seek=4096 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/dm-0 bs=4096 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

#[nixos_test]
fn first_linear_resume_reads_and_writes(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_expected\" bs=4096 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/vde bs=512 seek=4096 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/dm-0 bs=4096 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_expected\" bs=4096 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=\"${dm_work}/dm_expected\" of=/dev/dm-0 bs=4096 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/vde bs=512 skip=4096 count=8 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

// Query one lifecycle across tableless, inactive, active, and pending replacement states.
#[nixos_test]
fn query_table_states(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dmsetup ls", &Regex::new("phase1_test").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("test ! -e /dev/dm-0", &Regex::new("(?s).*").unwrap())?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup table phase1_test", &Regex::new("^\\s*$").unwrap())?;

    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup table phase1_test", &Regex::new("^\\s*$").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup table --inactive phase1_test",
        &Regex::new("0\\ 8\\ linear").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup status --inactive phase1_test",
        &Regex::new("0\\ 8\\ linear").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup deps --inactive -o devname phase1_test",
        &Regex::new("vde").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup info phase1_test",
        &Regex::new("Open\\ count:").unwrap(),
    )?;

    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup table phase1_test",
        &Regex::new("0\\ 8\\ linear").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup status phase1_test",
        &Regex::new("0\\ 8\\ linear").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup deps -o devname phase1_test",
        &Regex::new("vde").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup info phase1_test", &Regex::new("ACTIVE").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("blockdev --getra /dev/vde", &Regex::new("0").unwrap())?;
    nixos_shell
        .run_cmd_and_expect_regex("blockdev --getra /dev/dm-0", &Regex::new("0").unwrap())?;

    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 4 linear /dev/vdf 8192\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup table phase1_test",
        &Regex::new("0\\ 8\\ linear").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup table --inactive phase1_test",
        &Regex::new("0\\ 4\\ linear").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup status phase1_test",
        &Regex::new("0\\ 8\\ linear").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup status --inactive phase1_test",
        &Regex::new("0\\ 4\\ linear").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup deps -o devname phase1_test",
        &Regex::new("vde").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup deps --inactive -o devname phase1_test",
        &Regex::new("vdf").unwrap(),
    )?;

    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;

    Ok(())
}

// Discard a replacement without changing the live mapping or its data path.
#[nixos_test]
fn clear_pending_table_preserves_active_mapping(nixos_shell: &mut Session) -> Result<(), Error> {
    // Refuse an existing mapping or mount before clearing only this suite's scratch files.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="$dm_work/expected" bs=4096 count=1 iflag=fullblock status=none"#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="$dm_work/expected" of=/dev/vde bs=512 seek=4096 conv=fsync,notrunc status=none"#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/zero of=/dev/vdf bs=512 seek=8192 count=8 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vdf 8192\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup table phase1_test",
        &Regex::new("0\\ 8\\ linear.*4096").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup table --inactive phase1_test",
        &Regex::new("0\\ 8\\ linear.*8192").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup clear phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup table phase1_test",
        &Regex::new("0\\ 8\\ linear.*4096").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test -z "$(dmsetup table --inactive phase1_test)""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "test -b /dev/dm-0 && test \"$(blockdev --getsize64 /dev/dm-0)\" -eq 4096",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="$dm_work/actual" bs=4096 count=1 iflag=fullblock status=none && test "$(stat -c %s "$dm_work/actual")" -eq 4096 && cmp "$dm_work/expected" "$dm_work/actual""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="$dm_work/updated" bs=4096 count=1 iflag=fullblock status=none && test "$(stat -c %s "$dm_work/updated")" -eq 4096"#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="$dm_work/updated" of=/dev/dm-0 bs=4096 count=1 iflag=fullblock conv=fsync,notrunc status=none"#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde of="$dm_work/physical" bs=4096 skip=512 count=1 iflag=fullblock status=none && test "$(stat -c %s "$dm_work/physical")" -eq 4096 && cmp "$dm_work/updated" "$dm_work/physical""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    // A subsequent RESUME must not resurrect the discarded replacement.
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup table phase1_test",
        &Regex::new("0\\ 8\\ linear.*4096").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="$dm_work/actual" bs=4096 count=1 iflag=fullblock status=none && test "$(stat -c %s "$dm_work/actual")" -eq 4096 && cmp "$dm_work/updated" "$dm_work/actual""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("test ! -e /dev/dm-0", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

// Clearing without a candidate is a no-op; removing the mapper ends its control lifetime.
#[nixos_test]
fn clear_without_pending_table_and_remove(nixos_shell: &mut Session) -> Result<(), Error> {
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup clear phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("test ! -e /dev/dm-0", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup clear phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "test -b /dev/dm-0 && test \"$(blockdev --getsize64 /dev/dm-0)\" -eq 0",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test -z "$(dmsetup table --inactive phase1_test)""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "if dmsetup resume phase1_test; then false; else true; fi",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup clear phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("test ! -e /dev/dm-0", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "if dmsetup clear phase1_test; then false; else true; fi",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "if dmsetup info phase1_test; then false; else true; fi",
        &Regex::new("(?s).*").unwrap(),
    )?;
    Ok(())
}

// Repeated suspend preserves state; resume restores access to the original data.
#[nixos_test]
fn suspend_and_resume_linear_mapping(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_expected\" bs=4096 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/vde bs=512 seek=4096 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup suspend phase1_test",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup info phase1_test",
        &Regex::new("SUSPENDED").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup suspend phase1_test",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup info phase1_test",
        &Regex::new("SUSPENDED").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup info phase1_test", &Regex::new("ACTIVE").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/dm-0 bs=4096 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

// Remove tableless, inactive and active devices, and reject removal while open.
#[nixos_test]
fn remove_mapping_lifecycle(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "if dmsetup info phase1_test; then false; else true; fi",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("test ! -e /dev/dm-0", &Regex::new("(?s).*").unwrap())?;

    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "if dmsetup info phase1_test; then false; else true; fi",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("test ! -e /dev/dm-0", &Regex::new("(?s).*").unwrap())?;

    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "if dmsetup info phase1_test; then false; else true; fi",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("test ! -e /dev/dm-0", &Regex::new("(?s).*").unwrap())?;

    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("exec 9</dev/dm-0", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "if dmsetup remove phase1_test; then false; else true; fi",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("test -b /dev/dm-0", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("exec 9<&-", &Regex::new("(?s).*").unwrap())?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "if dmsetup info phase1_test; then false; else true; fi",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("test ! -e /dev/dm-0", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

#[nixos_test]
fn failed_load_preserves_active_mapping(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_expected\" bs=4096 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/vde bs=512 seek=4096 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("if dmsetup load phase1_test --table \"0 8 linear /dev/vde 999999999\"; then false; else true; fi", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "blockdev --getsize64 /dev/dm-0",
        &Regex::new("4096").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/dm-0 bs=4096 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

// Check read-only protection and short or rejected I/O at the mapping boundary.
#[nixos_test]
fn readonly_and_io_bounds(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_expected\" bs=4096 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/vde bs=512 seek=4096 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load --readonly phase1_test --table \"0 8 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/dm-0 bs=4096 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("if dd if=/dev/zero of=/dev/dm-0 bs=512 count=1 conv=fsync,notrunc status=none; then false; else true; fi", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/vde bs=512 skip=4096 count=8 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;

    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/zero of=/dev/vde bs=512 seek=4096 count=10 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/dm-0 of=\"${dm_work}/dm_tail\" bs=512 skip=7 count=2 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "stat -c %s \"${dm_work}/dm_tail\"",
        &Regex::new("512").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_patch\" bs=1024 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("if dd if=\"${dm_work}/dm_patch\" of=/dev/dm-0 bs=1024 oflag=seek_bytes seek=3584 conv=fsync,notrunc status=none; then false; else true; fi", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("cmp <(head -c 512 \"${dm_work}/dm_patch\") <(dd if=/dev/vde bs=512 skip=4103 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/dm-0 of=\"${dm_work}/dm_eof\" bs=512 skip=8 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "test ! -s \"${dm_work}/dm_eof\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("if dd if=/dev/zero of=/dev/dm-0 bs=512 seek=8 count=1 conv=fsync,notrunc status=none; then false; else true; fi", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/dm-0 of=\"${dm_work}/dm_eof\" bs=512 skip=9 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "test ! -s \"${dm_work}/dm_eof\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("if dd if=/dev/zero of=/dev/dm-0 bs=512 seek=9 count=1 conv=fsync,notrunc status=none; then false; else true; fi", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp <(head -c 1024 /dev/zero) <(dd if=/dev/vde bs=512 skip=4104 count=2 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

#[nixos_test]
fn multi_target_linear_reads_and_writes(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/zero of=/dev/vde bs=512 seek=4095 count=6 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/zero of=/dev/vde bs=512 seek=8191 count=6 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("printf '%s\\n' '0 4 linear /dev/vde 4096' '4 4 linear /dev/vde 8192' > \"${dm_work}/dm_table\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test \"${dm_work}/dm_table\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_expected\" bs=4096 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/dm-0 bs=4096 count=1 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/vde of=\"${dm_work}/dm_backing\" bs=512 skip=4096 count=4 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/vde bs=512 skip=8192 count=4 status=none >> \"${dm_work}/dm_backing\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" \"${dm_work}/dm_backing\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/dm-0 bs=4096 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_expected\" bs=4096 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/vde bs=512 seek=4096 count=4 conv=fsync,notrunc status=none", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/vde bs=512 skip=4 seek=8192 count=4 conv=fsync,notrunc status=none", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/dm-0 bs=4096 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("cmp <(dd if=\"${dm_work}/dm_expected\" bs=512 skip=3 count=2 status=none) <(dd if=/dev/dm-0 bs=512 skip=3 count=2 status=none)", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_patch\" bs=1024 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_patch\" of=/dev/dm-0 bs=1024 oflag=seek_bytes seek=1536 conv=fsync,notrunc status=none", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_patch\" of=\"${dm_work}/dm_expected\" bs=512 seek=3 conv=notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/dm-0 bs=4096 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/vde of=\"${dm_work}/dm_backing\" bs=512 skip=4096 count=4 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/vde bs=512 skip=8192 count=4 status=none >> \"${dm_work}/dm_backing\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" \"${dm_work}/dm_backing\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp <(head -c 512 /dev/zero) <(dd if=/dev/vde bs=512 skip=4095 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp <(head -c 512 /dev/zero) <(dd if=/dev/vde bs=512 skip=4100 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp <(head -c 512 /dev/zero) <(dd if=/dev/vde bs=512 skip=8191 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp <(head -c 512 /dev/zero) <(dd if=/dev/vde bs=512 skip=8196 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

#[nixos_test]
fn multi_disk_linear_reads_and_writes(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/zero of=/dev/vde bs=512 seek=4095 count=6 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/zero of=/dev/vdf bs=512 seek=8191 count=6 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("printf '%s\\n' '0 4 linear /dev/vde 4096' '4 4 linear /dev/vdf 8192' > \"${dm_work}/dm_table\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test \"${dm_work}/dm_table\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_expected\" bs=4096 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/dm-0 bs=4096 count=1 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/vde of=\"${dm_work}/dm_backing\" bs=512 skip=4096 count=4 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/vdf bs=512 skip=8192 count=4 status=none >> \"${dm_work}/dm_backing\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" \"${dm_work}/dm_backing\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/dm-0 bs=4096 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_expected\" bs=4096 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/vde bs=512 seek=4096 count=4 conv=fsync,notrunc status=none", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/vdf bs=512 skip=4 seek=8192 count=4 conv=fsync,notrunc status=none", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/dm-0 bs=4096 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("cmp <(dd if=\"${dm_work}/dm_expected\" bs=512 skip=3 count=2 status=none) <(dd if=/dev/dm-0 bs=512 skip=3 count=2 status=none)", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_patch\" bs=1024 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_patch\" of=/dev/dm-0 bs=1024 oflag=seek_bytes seek=1536 conv=fsync,notrunc status=none", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_patch\" of=\"${dm_work}/dm_expected\" bs=512 seek=3 conv=notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/dm-0 bs=4096 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/vde of=\"${dm_work}/dm_backing\" bs=512 skip=4096 count=4 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/vdf bs=512 skip=8192 count=4 status=none >> \"${dm_work}/dm_backing\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" \"${dm_work}/dm_backing\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp <(head -c 512 /dev/zero) <(dd if=/dev/vde bs=512 skip=4095 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp <(head -c 512 /dev/zero) <(dd if=/dev/vde bs=512 skip=4100 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp <(head -c 512 /dev/zero) <(dd if=/dev/vdf bs=512 skip=8191 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp <(head -c 512 /dev/zero) <(dd if=/dev/vdf bs=512 skip=8196 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

#[nixos_test]
fn ext2_multi_target_same_disk(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("printf '%s\\n' '0 32768 linear /dev/vde 32768' '32768 32768 linear /dev/vde 98304' > \"${dm_work}/dm_table\"", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test \"${dm_work}/dm_table\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "blockdev --getsize64 /dev/dm-0",
        &Regex::new("33554432").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mkfs.ext2 -q -F -b 4096 -m 0 /dev/dm-0",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mkdir -p \"${dm_work}/phase1_dm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mount -t ext2 /dev/dm-0 \"${dm_work}/phase1_dm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=/dev/urandom of=\"${dm_work}/phase1_dm_mnt\"/payload bs=1M count=24 conv=fsync status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "stat -c %s \"${dm_work}/phase1_dm_mnt\"/payload",
        &Regex::new("25165824").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "sha256sum \"${dm_work}/phase1_dm_mnt\"/payload > \"${dm_work}/dm_payload.sha256\"",
        &Regex::new("(?s).*").unwrap(),
    )?;

    // No raw device or regular file is open: the mount alone must prevent removal.
    nixos_shell.run_cmd_and_expect_regex(
        "if dmsetup remove phase1_test; then false; else true; fi",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("test -b /dev/dm-0", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "sha256sum -c \"${dm_work}/dm_payload.sha256\"",
        &Regex::new("payload: OK").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/lease_expected" bs=4096 count=1 status=none"#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/lease_expected" of="${dm_work}/phase1_dm_mnt/lease_payload" bs=4096 conv=fsync status=none"#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/lease_expected" "${dm_work}/phase1_dm_mnt/lease_payload""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("sync", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("cmp <(dd if=/dev/dm-0 bs=1M count=16 status=none) <(dd if=/dev/vde bs=512 skip=32768 count=32768 status=none)", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("cmp <(dd if=/dev/dm-0 bs=1M skip=16 count=16 status=none) <(dd if=/dev/vde bs=512 skip=98304 count=32768 status=none)", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "umount \"${dm_work}/phase1_dm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "e2fsck -f -n /dev/dm-0",
        &Regex::new("(?s)Pass 5:.*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mount -t ext2 /dev/dm-0 \"${dm_work}/phase1_dm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "sha256sum -c \"${dm_work}/dm_payload.sha256\"",
        &Regex::new("payload: OK").unwrap(),
    )?;

    // Lazy unmount lets a regular file outlive the mount on both Linux and Asterinas.
    // Keep no raw block-device descriptor, so only the filesystem retains its lease.
    nixos_shell.run_cmd_and_expect_regex(
        r#"exec 9<"${dm_work}/phase1_dm_mnt/lease_payload""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "umount -l \"${dm_work}/phase1_dm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "if dmsetup remove phase1_test; then false; else true; fi",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("test -b /dev/dm-0", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/lease_expected" - <&9"#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("exec 9<&-", &Regex::new("(?s).*").unwrap())?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("test ! -e /dev/dm-0", &Regex::new("(?s).*").unwrap())?;

    // Reuse the released minor without formatting and verify the original contents.
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test \"${dm_work}/dm_table\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("test -b /dev/dm-0", &Regex::new("(?s).*").unwrap())?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "mount -t ext2 /dev/dm-0 \"${dm_work}/phase1_dm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "sha256sum -c \"${dm_work}/dm_payload.sha256\"",
        &Regex::new("payload: OK").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/lease_expected" "${dm_work}/phase1_dm_mnt/lease_payload""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "umount \"${dm_work}/phase1_dm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("test ! -e /dev/dm-0", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

#[nixos_test]
fn ext2_multi_target_cross_disk(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("printf '%s\\n' '0 32768 linear /dev/vde 32768' '32768 32768 linear /dev/vdf 98304' > \"${dm_work}/dm_table\"", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test \"${dm_work}/dm_table\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "blockdev --getsize64 /dev/dm-0",
        &Regex::new("33554432").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mkfs.ext2 -q -F -b 4096 -m 0 /dev/dm-0",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mkdir -p \"${dm_work}/phase1_dm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mount -t ext2 /dev/dm-0 \"${dm_work}/phase1_dm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=/dev/urandom of=\"${dm_work}/phase1_dm_mnt\"/payload bs=1M count=24 conv=fsync status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "stat -c %s \"${dm_work}/phase1_dm_mnt\"/payload",
        &Regex::new("25165824").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "sha256sum \"${dm_work}/phase1_dm_mnt\"/payload > \"${dm_work}/dm_payload.sha256\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("sync", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("cmp <(dd if=/dev/dm-0 bs=1M count=16 status=none) <(dd if=/dev/vde bs=512 skip=32768 count=32768 status=none)", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("cmp <(dd if=/dev/dm-0 bs=1M skip=16 count=16 status=none) <(dd if=/dev/vdf bs=512 skip=98304 count=32768 status=none)", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "umount \"${dm_work}/phase1_dm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "e2fsck -f -n /dev/dm-0",
        &Regex::new("(?s)Pass 5:.*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "mount -t ext2 /dev/dm-0 \"${dm_work}/phase1_dm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "sha256sum -c \"${dm_work}/dm_payload.sha256\"",
        &Regex::new("payload: OK").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "umount \"${dm_work}/phase1_dm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

#[nixos_test]
fn implicit_table_replacement(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_expected\" bs=4096 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/vde bs=512 seek=4096 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/zero of=/dev/vde bs=512 seek=8192 count=8 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 8192\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp <(head -c 4096 /dev/zero) <(dd if=/dev/dm-0 bs=4096 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/dm-0 bs=4096 count=1 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/vde bs=512 skip=8192 count=8 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/vde bs=512 skip=4096 count=8 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

#[nixos_test]
fn explicit_table_replacement(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_expected\" bs=4096 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/vde bs=512 seek=4096 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/zero of=/dev/vde bs=512 seek=8192 count=8 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup suspend phase1_test",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 8192\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp <(head -c 4096 /dev/zero) <(dd if=/dev/dm-0 bs=4096 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/dm-0 bs=4096 count=1 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/vde bs=512 skip=8192 count=8 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/vde bs=512 skip=4096 count=8 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

// Verify capacity publication, preserved data, newly usable space, and the reduced boundary.
#[nixos_test]
fn raw_mapping_growth_and_shrink(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_expected\" bs=4096 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/vde bs=512 seek=4096 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 16 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "blockdev --getsize64 /dev/dm-0",
        &Regex::new("4096").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "blockdev --getsize64 /dev/dm-0",
        &Regex::new("8192").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/dm-0 bs=4096 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/dm-0 bs=512 seek=8 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/vde bs=512 skip=4104 count=8 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;

    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_expected\" bs=4096 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_expected\" of=/dev/vde bs=512 seek=4096 conv=fsync,notrunc status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 16 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup suspend phase1_test",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 8 linear /dev/vde 4096\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "blockdev --getsize64 /dev/dm-0",
        &Regex::new("8192").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "blockdev --getsize64 /dev/dm-0",
        &Regex::new("4096").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_expected\" <(dd if=/dev/dm-0 bs=4096 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/dm-0 of=\"${dm_work}/dm_eof\" bs=512 skip=8 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "test ! -s \"${dm_work}/dm_eof\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("if dd if=/dev/zero of=/dev/dm-0 bs=512 seek=8 count=1 conv=fsync,notrunc status=none; then false; else true; fi", &Regex::new("(?s).*").unwrap())?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

// Keep I/O in the unchanged prefix while replacing the table and resizing its tail.
// Progress does not prove a particular kernel critical-section interleaving.
#[nixos_test]
fn concurrent_hot_table_replacement(nixos_shell: &mut Session) -> Result<(), Error> {
    // Clear only this suite's temporary files before starting the case.
    // Refuse leftover mappings or mounts instead of deleting live test data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup create phase1_test --notable",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "dmsetup load phase1_test --table \"0 131072 linear /dev/vde 131072\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup resume phase1_test", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        "dd if=/dev/urandom of=\"${dm_work}/dm_hot_expected\" bs=4096 count=1 status=none",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "sha256sum < \"${dm_work}/dm_hot_expected\" > \"${dm_work}/dm_hot.sha256\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "rm -f \"${dm_work}/dm_hot_stop\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        ": > \"${dm_work}/dm_hot_progress\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("bash -euo pipefail -c 'while test ! -e \"${dm_work}/dm_hot_stop\"; do dd if=\"${dm_work}/dm_hot_expected\" of=/dev/dm-0 bs=4096 count=1 conv=fsync,notrunc status=none; dd if=/dev/dm-0 bs=4096 count=1 status=none | sha256sum -c \"${dm_work}/dm_hot.sha256\"; printf . >> \"${dm_work}/dm_hot_progress\"; done' > \"${dm_work}/dm_hot.log\" 2>&1 & dm_hot_pid=$!", &Regex::new("(?s).*").unwrap())?;
    nixos_shell.run_cmd_and_expect_regex("for attempt in {1..1000}; do test -s \"${dm_work}/dm_hot_progress\" && break; sleep 0.01; done; test -s \"${dm_work}/dm_hot_progress\"", &Regex::new("(?s).*").unwrap())?;
    let success = Regex::new("(?s).*").unwrap();
    let original_capacity = Regex::new("67108864").unwrap();
    let expanded_capacity = Regex::new("100663296").unwrap();
    for _ in 0..8 {
        nixos_shell.run_cmd_and_expect_regex("printf '%s\\n' '0 131072 linear /dev/vde 131072' '131072 65536 linear /dev/vdf 131072' > \"${dm_work}/dm_table\"", &success)?;
        nixos_shell.run_cmd_and_expect_regex(
            "dmsetup load phase1_test \"${dm_work}/dm_table\"",
            &success,
        )?;
        nixos_shell
            .run_cmd_and_expect_regex("blockdev --getsize64 /dev/dm-0", &original_capacity)?;
        nixos_shell.run_cmd_and_expect_regex("dmsetup resume phase1_test", &success)?;
        nixos_shell
            .run_cmd_and_expect_regex("blockdev --getsize64 /dev/dm-0", &expanded_capacity)?;
        nixos_shell.run_cmd_and_expect_regex(
            "stat -c %s \"${dm_work}/dm_hot_progress\" > \"${dm_work}/dm_hot_before\"",
            &success,
        )?;
        nixos_shell.run_cmd_and_expect_regex("for attempt in {1..1000}; do test \"$(stat -c %s \"${dm_work}/dm_hot_progress\")\" -gt \"$(cat \"${dm_work}/dm_hot_before\")\" && break; sleep 0.01; done; test \"$(stat -c %s \"${dm_work}/dm_hot_progress\")\" -gt \"$(cat \"${dm_work}/dm_hot_before\")\"", &success)?;
        nixos_shell.run_cmd_and_expect_regex("dd if=\"${dm_work}/dm_hot_expected\" of=/dev/dm-0 bs=4096 seek=16384 conv=fsync,notrunc status=none", &success)?;
        nixos_shell.run_cmd_and_expect_regex("cmp \"${dm_work}/dm_hot_expected\" <(dd if=/dev/vdf bs=4096 skip=16384 count=1 status=none)",
            &success,
        )?;
        nixos_shell.run_cmd_and_expect_regex("dmsetup suspend phase1_test", &success)?;
        nixos_shell.run_cmd_and_expect_regex(
            "dmsetup load phase1_test --table \"0 131072 linear /dev/vde 131072\"",
            &success,
        )?;
        nixos_shell.run_cmd_and_expect_regex("dmsetup resume phase1_test", &success)?;
        nixos_shell
            .run_cmd_and_expect_regex("blockdev --getsize64 /dev/dm-0", &original_capacity)?;
        nixos_shell.run_cmd_and_expect_regex(
            "stat -c %s \"${dm_work}/dm_hot_progress\" > \"${dm_work}/dm_hot_before\"",
            &success,
        )?;
        nixos_shell.run_cmd_and_expect_regex("for attempt in {1..1000}; do test \"$(stat -c %s \"${dm_work}/dm_hot_progress\")\" -gt \"$(cat \"${dm_work}/dm_hot_before\")\" && break; sleep 0.01; done; test \"$(stat -c %s \"${dm_work}/dm_hot_progress\")\" -gt \"$(cat \"${dm_work}/dm_hot_before\")\"", &success)?;
    }
    nixos_shell.run_cmd_and_expect_regex(
        "touch \"${dm_work}/dm_hot_stop\"; wait \"$dm_hot_pid\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        "cmp \"${dm_work}/dm_hot_expected\" <(dd if=/dev/dm-0 bs=4096 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex("cmp \"${dm_work}/dm_hot_expected\" <(dd if=/dev/vde bs=4096 skip=16384 count=1 status=none)",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
    Ok(())
}

// Check numeric backings against four independently enumerated physical chunks.
#[nixos_test]
fn striped_cross_disk_reads_and_writes(nixos_shell: &mut Session) -> Result<(), Error> {
    // Refuse stale mappings and mounts before replacing this case's artifacts.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_e_hex=$(stat -c "%t %T" /dev/vde) && read -r dm_e_major dm_e_minor <<< "$dm_e_hex" && dm_e="$((16#$dm_e_major)):$((16#$dm_e_minor))" && dm_f_hex=$(stat -c "%t %T" /dev/vdf) && read -r dm_f_major dm_f_minor <<< "$dm_f_hex" && dm_f="$((16#$dm_f_major)):$((16#$dm_f_minor))""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Logical chunks [0,2), [2,4), [4,6), [6,8) alternate between the two disks.
    // Random chunks distinguish both sectors within each chunk and successive rows.
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/expected" bs=512 count=8 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/expected")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vde bs=512 skip=0 seek=4096 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vdf bs=512 skip=2 seek=8192 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vde bs=512 skip=4 seek=4098 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vdf bs=512 skip=6 seek=8194 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup create phase1_test --notable"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup load phase1_test --table "0 8 striped 2 2 $dm_e 4096 $dm_f 8192""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table phase1_test"#,
        &Regex::new(r#"^\s*$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table --inactive phase1_test) && test "$dm_table" = "0 8 striped 2 2 $dm_e 4096 $dm_f 8192""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_status=$(dmsetup status --inactive phase1_test) && test "$dm_status" = "0 8 striped 2 $dm_e $dm_f 1 AA""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup deps --inactive phase1_test"#,
        &Regex::new(r#"^2 dependencies\s*:\s*\([0-9]+, [0-9]+\)\s*\([0-9]+, [0-9]+\)$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup resume phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"blockdev --getsize64 /dev/dm-0"#,
        &Regex::new(r#"^4096$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table phase1_test) && test "$dm_table" = "0 8 striped 2 2 $dm_e 4096 $dm_f 8192""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table --inactive phase1_test"#,
        &Regex::new(r#"^\s*$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_deps=$(dmsetup deps phase1_test) && read -r -a dm_dep_fields <<< "$dm_deps" && test "${dm_dep_fields[*]}" = "2 dependencies : ($((16#$dm_e_major)), $((16#$dm_e_minor))) ($((16#$dm_f_major)), $((16#$dm_f_minor)))""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/expected" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Exercise a chunk interior, a chunk boundary, a row boundary, and the final sector.
    // These user-space requests do not assert a particular BIO split.
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of="${dm_work}/slice" bs=512 skip=1 count=1 status=none && test "$(stat -c %s "${dm_work}/slice")" -eq 512"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=1 count=1 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 512"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/slice" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of="${dm_work}/slice" bs=512 skip=1 count=2 status=none && test "$(stat -c %s "${dm_work}/slice")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=1 count=2 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/slice" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of="${dm_work}/slice" bs=512 skip=3 count=2 status=none && test "$(stat -c %s "${dm_work}/slice")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=3 count=2 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/slice" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of="${dm_work}/slice" bs=512 skip=7 count=1 status=none && test "$(stat -c %s "${dm_work}/slice")" -eq 512"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=7 count=1 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 512"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/slice" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/written" bs=512 count=8 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/written")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/written" of=/dev/dm-0 bs=512 skip=0 seek=0 count=8 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4096 count=2 status=none > "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8192 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 2048"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4098 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 3072"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8194 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/written" "${dm_work}/physical""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/patch" bs=512 count=4 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/patch")" -eq 2048"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/patch" of=/dev/dm-0 bs=512 skip=0 seek=1 count=4 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/patch" of="${dm_work}/written" bs=512 skip=0 seek=1 count=4 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4096 count=2 status=none > "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8192 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 2048"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4098 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 3072"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8194 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/written" "${dm_work}/physical""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/last" bs=512 count=1 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/last")" -eq 512"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/last" of=/dev/dm-0 bs=512 skip=0 seek=7 count=1 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/last" of="${dm_work}/written" bs=512 skip=0 seek=7 count=1 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4096 count=2 status=none > "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8192 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 2048"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4098 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 3072"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8194 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/written" "${dm_work}/physical""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_status=$(dmsetup status phase1_test) && test "$dm_status" = "0 8 striped 2 $dm_e $dm_f 1 AA""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup remove phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"test ! -e /dev/dm-0"#, &Regex::new(r#"(?s).*"#).unwrap())?;
    Ok(())
}

// Keep two stripe parameters but report one unique backing dependency.
#[nixos_test]
fn striped_same_disk_reads_and_writes(nixos_shell: &mut Session) -> Result<(), Error> {
    // Refuse stale mappings and mounts before replacing this case's artifacts.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_e_hex=$(stat -c "%t %T" /dev/vde) && read -r dm_e_major dm_e_minor <<< "$dm_e_hex" && dm_e="$((16#$dm_e_major)):$((16#$dm_e_minor))" && dm_f_hex=$(stat -c "%t %T" /dev/vdf) && read -r dm_f_major dm_f_minor <<< "$dm_f_hex" && dm_f="$((16#$dm_f_major)):$((16#$dm_f_minor))""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/expected" bs=512 count=8 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/expected")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vde bs=512 skip=0 seek=4096 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vde bs=512 skip=2 seek=8192 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vde bs=512 skip=4 seek=4098 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vde bs=512 skip=6 seek=8194 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup create phase1_test --notable"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup load phase1_test --table "0 8 striped 2 2 /dev/vde 4096 /dev/vde 8192""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup resume phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table phase1_test) && test "$dm_table" = "0 8 striped 2 2 $dm_e 4096 $dm_e 8192""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_deps=$(dmsetup deps phase1_test) && read -r -a dm_dep_fields <<< "$dm_deps" && test "${dm_dep_fields[*]}" = "1 dependencies : ($((16#$dm_e_major)), $((16#$dm_e_minor)))""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/expected" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/written" bs=512 count=8 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/written")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/written" of=/dev/dm-0 bs=512 skip=0 seek=0 count=8 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4096 count=2 status=none > "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=8192 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 2048"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4098 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 3072"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=8194 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/written" "${dm_work}/physical""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Dependency deduplication does not prove a particular number of backing flushes.
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup remove phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"test ! -e /dev/dm-0"#, &Regex::new(r#"(?s).*"#).unwrap())?;
    Ok(())
}

// Use a non-row-aligned target start and a non-power-of-two chunk.
#[nixos_test]
fn mixed_linear_and_striped_reads_and_writes(nixos_shell: &mut Session) -> Result<(), Error> {
    // Refuse stale mappings and mounts before replacing this case's artifacts.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // The striped target starts at sector 2, with six-sector rows and three-sector chunks.
    // The fixed physical list also includes both neighboring linear targets.
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/expected" bs=512 count=16 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/expected")" -eq 8192"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vde bs=512 skip=0 seek=12288 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vde bs=512 skip=2 seek=4096 count=3 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vdf bs=512 skip=5 seek=8192 count=3 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vde bs=512 skip=8 seek=4099 count=3 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vdf bs=512 skip=11 seek=8195 count=3 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vdf bs=512 skip=14 seek=16384 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/sentinel" bs=512 count=1 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/sentinel")" -eq 512"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/sentinel" of=/dev/vde bs=512 skip=0 seek=12287 count=1 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/sentinel" of=/dev/vde bs=512 skip=0 seek=12290 count=1 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/sentinel" of=/dev/vdf bs=512 skip=0 seek=16383 count=1 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/sentinel" of=/dev/vdf bs=512 skip=0 seek=16386 count=1 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup create phase1_test --notable"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"printf '%s\n' '0 2 linear /dev/vde 12288' '2 12 striped 2 3 /dev/vde 4096 /dev/vdf 8192' '14 2 linear /dev/vdf 16384' > "${dm_work}/table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup load phase1_test "${dm_work}/table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup resume phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"blockdev --getsize64 /dev/dm-0"#,
        &Regex::new(r#"^8192$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=16 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 8192"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/expected" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of="${dm_work}/slice" bs=512 skip=1 count=2 status=none && test "$(stat -c %s "${dm_work}/slice")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=1 count=2 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/slice" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of="${dm_work}/slice" bs=512 skip=13 count=2 status=none && test "$(stat -c %s "${dm_work}/slice")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=13 count=2 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/slice" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // First write only the striped region; both linear regions must retain their contents.
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/patch" bs=512 count=12 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/patch")" -eq 6144"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/patch" of=/dev/dm-0 bs=512 skip=0 seek=2 count=12 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/patch" of="${dm_work}/expected" bs=512 skip=0 seek=2 count=12 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=12288 count=2 status=none > "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4096 count=3 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 2560"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8192 count=3 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4099 count=3 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 5632"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8195 count=3 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 7168"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=16384 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 8192"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/expected" "${dm_work}/physical""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Then cross both target boundaries and compare the entire independent layout.
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/crossing" bs=512 count=14 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/crossing")" -eq 7168"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/crossing" of=/dev/dm-0 bs=512 skip=0 seek=1 count=14 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/crossing" of="${dm_work}/expected" bs=512 skip=0 seek=1 count=14 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=12288 count=2 status=none > "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4096 count=3 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 2560"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8192 count=3 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4099 count=3 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 5632"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8195 count=3 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 7168"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=16384 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 8192"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/expected" "${dm_work}/physical""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=16 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 8192"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/expected" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde of="${dm_work}/actual" bs=512 skip=12287 count=1 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 512"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/sentinel" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde of="${dm_work}/actual" bs=512 skip=12290 count=1 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 512"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/sentinel" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf of="${dm_work}/actual" bs=512 skip=16383 count=1 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 512"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/sentinel" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf of="${dm_work}/actual" bs=512 skip=16386 count=1 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 512"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/sentinel" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup remove phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"test ! -e /dev/dm-0"#, &Regex::new(r#"(?s).*"#).unwrap())?;
    Ok(())
}

// Reject invalid loads without losing either the active or pending generation.
#[nixos_test]
fn invalid_striped_load_preserves_tables(nixos_shell: &mut Session) -> Result<(), Error> {
    // Refuse stale mappings and mounts before replacing this case's artifacts.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_e_hex=$(stat -c "%t %T" /dev/vde) && read -r dm_e_major dm_e_minor <<< "$dm_e_hex" && dm_e="$((16#$dm_e_major)):$((16#$dm_e_minor))" && dm_f_hex=$(stat -c "%t %T" /dev/vdf) && read -r dm_f_major dm_f_minor <<< "$dm_f_hex" && dm_f="$((16#$dm_f_major)):$((16#$dm_f_minor))""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/active" bs=512 count=8 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/active")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/active" of=/dev/vde bs=512 skip=0 seek=4096 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/active" of=/dev/vdf bs=512 skip=2 seek=8192 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/active" of=/dev/vde bs=512 skip=4 seek=4098 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/active" of=/dev/vdf bs=512 skip=6 seek=8194 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/candidate" bs=512 count=8 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/candidate")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/candidate" of=/dev/vdf bs=512 skip=0 seek=12288 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/candidate" of=/dev/vde bs=512 skip=2 seek=16384 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/candidate" of=/dev/vdf bs=512 skip=4 seek=12290 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/candidate" of=/dev/vde bs=512 skip=6 seek=16386 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup create phase1_test --notable"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup load phase1_test --table "0 8 striped 2 2 /dev/vde 4096 /dev/vdf 8192""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup resume phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup load phase1_test --table "0 8 striped 2 2 /dev/vdf 12288 /dev/vde 16384""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table phase1_test) && test "$dm_table" = "0 8 striped 2 2 $dm_e 4096 $dm_f 8192""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table --inactive phase1_test) && test "$dm_table" = "0 8 striped 2 2 $dm_f 12288 $dm_e 16384""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table phase1_test > "${dm_work}/active.table" && dmsetup table --inactive phase1_test > "${dm_work}/candidate.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_e_sectors=$(blockdev --getsz /dev/vde) && test "$dm_e_sectors" -gt 0"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Each rejected LOAD is followed by separate table and data checks.
    // The capacity-derived offset is outside the backing regardless of persistent disk contents.
    nixos_shell.run_cmd_and_expect_regex(
        r#"test ! -e /dev/phase1_missing_backing"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Zero stripe count.
    nixos_shell.run_cmd_and_expect_regex(
        r#"if dmsetup load phase1_test --table "0 8 striped 0 2"; then false; else true; fi"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table phase1_test > "${dm_work}/actual.table" && cmp "${dm_work}/active.table" "${dm_work}/actual.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table --inactive phase1_test > "${dm_work}/actual.table" && cmp "${dm_work}/candidate.table" "${dm_work}/actual.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/active" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Zero chunk size.
    nixos_shell.run_cmd_and_expect_regex(
        r#"if dmsetup load phase1_test --table "0 8 striped 2 0 /dev/vde 4096 /dev/vdf 8192"; then false; else true; fi"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table phase1_test > "${dm_work}/actual.table" && cmp "${dm_work}/active.table" "${dm_work}/actual.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table --inactive phase1_test > "${dm_work}/actual.table" && cmp "${dm_work}/candidate.table" "${dm_work}/actual.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/active" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Missing offset.
    nixos_shell.run_cmd_and_expect_regex(
        r#"if dmsetup load phase1_test --table "0 8 striped 2 2 /dev/vde 4096 /dev/vdf"; then false; else true; fi"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table phase1_test > "${dm_work}/actual.table" && cmp "${dm_work}/active.table" "${dm_work}/actual.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table --inactive phase1_test > "${dm_work}/actual.table" && cmp "${dm_work}/candidate.table" "${dm_work}/actual.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/active" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Extra parameter.
    nixos_shell.run_cmd_and_expect_regex(
        r#"if dmsetup load phase1_test --table "0 8 striped 2 2 /dev/vde 4096 /dev/vdf 8192 extra"; then false; else true; fi"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table phase1_test > "${dm_work}/actual.table" && cmp "${dm_work}/active.table" "${dm_work}/actual.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table --inactive phase1_test > "${dm_work}/actual.table" && cmp "${dm_work}/candidate.table" "${dm_work}/actual.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/active" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Invalid numeric field.
    nixos_shell.run_cmd_and_expect_regex(
        r#"if dmsetup load phase1_test --table "0 8 striped 2 invalid /dev/vde 4096 /dev/vdf 8192"; then false; else true; fi"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table phase1_test > "${dm_work}/actual.table" && cmp "${dm_work}/active.table" "${dm_work}/actual.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table --inactive phase1_test > "${dm_work}/actual.table" && cmp "${dm_work}/candidate.table" "${dm_work}/actual.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/active" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Incomplete stripe row.
    nixos_shell.run_cmd_and_expect_regex(
        r#"if dmsetup load phase1_test --table "0 6 striped 2 2 /dev/vde 4096 /dev/vdf 8192"; then false; else true; fi"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table phase1_test > "${dm_work}/actual.table" && cmp "${dm_work}/active.table" "${dm_work}/actual.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table --inactive phase1_test > "${dm_work}/actual.table" && cmp "${dm_work}/candidate.table" "${dm_work}/actual.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/active" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Backing capacity exceeded.
    nixos_shell.run_cmd_and_expect_regex(
        r#"if dmsetup load phase1_test --table "0 8 striped 2 2 /dev/vde $dm_e_sectors /dev/vdf 8192"; then false; else true; fi"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table phase1_test > "${dm_work}/actual.table" && cmp "${dm_work}/active.table" "${dm_work}/actual.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table --inactive phase1_test > "${dm_work}/actual.table" && cmp "${dm_work}/candidate.table" "${dm_work}/actual.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/active" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Missing backing.
    nixos_shell.run_cmd_and_expect_regex(
        r#"if dmsetup load phase1_test --table "0 8 striped 2 2 /dev/phase1_missing_backing 0 /dev/vdf 8192"; then false; else true; fi"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table phase1_test > "${dm_work}/actual.table" && cmp "${dm_work}/active.table" "${dm_work}/actual.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table --inactive phase1_test > "${dm_work}/actual.table" && cmp "${dm_work}/candidate.table" "${dm_work}/actual.table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/active" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup resume phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"blockdev --getsize64 /dev/dm-0"#,
        &Regex::new(r#"^4096$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table phase1_test) && test "$dm_table" = "0 8 striped 2 2 $dm_f 12288 $dm_e 16384""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table --inactive phase1_test"#,
        &Regex::new(r#"^\s*$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/candidate" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup remove phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"test ! -e /dev/dm-0"#, &Regex::new(r#"(?s).*"#).unwrap())?;
    Ok(())
}

// Protect both backings and nonzero sentinels at the logical end.
#[nixos_test]
fn striped_readonly_and_io_bounds(nixos_shell: &mut Session) -> Result<(), Error> {
    // Refuse stale mappings and mounts before replacing this case's artifacts.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/expected" bs=512 count=8 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/expected")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vde bs=512 skip=0 seek=4096 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vdf bs=512 skip=2 seek=8192 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vde bs=512 skip=4 seek=4098 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/expected" of=/dev/vdf bs=512 skip=6 seek=8194 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/sentinel" bs=512 count=2 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/sentinel")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/sentinel" of=/dev/vde bs=512 skip=0 seek=4100 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/sentinel" of=/dev/vdf bs=512 skip=0 seek=8196 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup create phase1_test --notable"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup load --readonly phase1_test --table "0 8 striped 2 2 /dev/vde 4096 /dev/vdf 8192""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup resume phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/expected" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"if dd if=/dev/zero of=/dev/dm-0 bs=4096 count=1 conv=fsync,notrunc status=none; then false; else true; fi"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4096 count=2 status=none > "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8192 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 2048"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4098 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 3072"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8194 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/expected" "${dm_work}/physical""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup remove phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup create phase1_test --notable"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup load phase1_test --table "0 8 striped 2 2 /dev/vde 4096 /dev/vdf 8192""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup resume phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/last" bs=512 count=1 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/last")" -eq 512"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/last" of=/dev/dm-0 bs=512 skip=0 seek=7 count=1 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/last" of="${dm_work}/expected" bs=512 skip=0 seek=7 count=1 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=7 count=1 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 512"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/last" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4096 count=2 status=none > "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8192 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 2048"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4098 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 3072"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8194 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/expected" "${dm_work}/physical""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // A crossing write may commit its legal prefix before reporting failure.
    // Use two sector-sized writes: sector 7 succeeds, sector 8 fails; no rollback is assumed.
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/crossing" bs=512 count=2 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/crossing")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"if dd if="${dm_work}/crossing" of=/dev/dm-0 bs=512 seek=7 count=2 conv=fsync,notrunc status=none; then false; else true; fi"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/crossing" of="${dm_work}/expected" bs=512 skip=0 seek=7 count=1 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4096 count=2 status=none > "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8192 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 2048"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4098 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 3072"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8194 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/expected" "${dm_work}/physical""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"if dd if=/dev/zero of=/dev/dm-0 bs=512 seek=8 count=1 conv=fsync,notrunc status=none; then false; else true; fi"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/eof" bs=512 skip=8 count=1 status=none && test "$(stat -c %s "${dm_work}/eof")" -eq 0"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"if dd if=/dev/zero of=/dev/dm-0 bs=512 seek=9 count=1 conv=fsync,notrunc status=none; then false; else true; fi"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/eof" bs=512 skip=9 count=1 status=none && test "$(stat -c %s "${dm_work}/eof")" -eq 0"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/tail" bs=512 skip=7 count=2 status=none && test "$(stat -c %s "${dm_work}/tail")" -eq 512"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/crossing" of="${dm_work}/slice" bs=512 skip=0 count=1 status=none && test "$(stat -c %s "${dm_work}/slice")" -eq 512"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/slice" "${dm_work}/tail""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/expected" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4096 count=2 status=none > "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8192 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 2048"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4098 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 3072"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8194 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/expected" "${dm_work}/physical""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde of="${dm_work}/actual" bs=512 skip=4100 count=2 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/sentinel" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf of="${dm_work}/actual" bs=512 skip=8196 count=2 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/sentinel" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup remove phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"test ! -e /dev/dm-0"#, &Regex::new(r#"(?s).*"#).unwrap())?;
    Ok(())
}

// Switch linear to striped implicitly, then back to linear explicitly.
#[nixos_test]
fn striped_table_replacement(nixos_shell: &mut Session) -> Result<(), Error> {
    // Refuse stale mappings and mounts before replacing this case's artifacts.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_e_hex=$(stat -c "%t %T" /dev/vde) && read -r dm_e_major dm_e_minor <<< "$dm_e_hex" && dm_e="$((16#$dm_e_major)):$((16#$dm_e_minor))" && dm_f_hex=$(stat -c "%t %T" /dev/vdf) && read -r dm_f_major dm_f_minor <<< "$dm_f_hex" && dm_f="$((16#$dm_f_major)):$((16#$dm_f_minor))""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/linear" bs=512 count=8 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/linear")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/linear" of=/dev/vde bs=512 skip=0 seek=12288 count=8 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/striped" bs=512 count=8 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/striped")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/striped" of=/dev/vde bs=512 skip=0 seek=4096 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/striped" of=/dev/vdf bs=512 skip=2 seek=8192 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/striped" of=/dev/vde bs=512 skip=4 seek=4098 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/striped" of=/dev/vdf bs=512 skip=6 seek=8194 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/replacement" bs=512 count=4 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/replacement")" -eq 2048"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/replacement" of=/dev/vdf bs=512 skip=0 seek=16384 count=4 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup create phase1_test --notable"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup load phase1_test --table "0 8 linear /dev/vde 12288""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup resume phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/linear" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup load phase1_test --table "0 8 striped 2 2 /dev/vde 4096 /dev/vdf 8192""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table phase1_test) && test "$dm_table" = "0 8 linear $dm_e 12288""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table --inactive phase1_test) && test "$dm_table" = "0 8 striped 2 2 $dm_e 4096 $dm_f 8192""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"blockdev --getsize64 /dev/dm-0"#,
        &Regex::new(r#"^4096$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/linear" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup resume phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table phase1_test) && test "$dm_table" = "0 8 striped 2 2 $dm_e 4096 $dm_f 8192""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table --inactive phase1_test"#,
        &Regex::new(r#"^\s*$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"blockdev --getsize64 /dev/dm-0"#,
        &Regex::new(r#"^4096$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/striped" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Suspended I/O is queued, so inspect tables and capacity without reading until resume.
    // The new linear layout has independent data; replacement does not migrate the old data.
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup suspend phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup info phase1_test"#,
        &Regex::new(r#"(?m)^State:\s+SUSPENDED$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup load phase1_test --table "0 4 linear /dev/vdf 16384""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table phase1_test) && test "$dm_table" = "0 8 striped 2 2 $dm_e 4096 $dm_f 8192""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table --inactive phase1_test) && test "$dm_table" = "0 4 linear $dm_f 16384""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"blockdev --getsize64 /dev/dm-0"#,
        &Regex::new(r#"^4096$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup resume phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table phase1_test) && test "$dm_table" = "0 4 linear $dm_f 16384""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup table --inactive phase1_test"#,
        &Regex::new(r#"^\s*$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"blockdev --getsize64 /dev/dm-0"#,
        &Regex::new(r#"^2048$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=4 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 2048"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/replacement" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4096 count=2 status=none > "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8192 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 2048"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4098 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 3072"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8194 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/striped" "${dm_work}/physical""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde of="${dm_work}/actual" bs=512 skip=12288 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/linear" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup remove phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"test ! -e /dev/dm-0"#, &Regex::new(r#"(?s).*"#).unwrap())?;
    Ok(())
}

// Resize complete stripe rows without changing their physical geometry.
#[nixos_test]
fn striped_raw_mapping_growth_and_shrink(nixos_shell: &mut Session) -> Result<(), Error> {
    // Refuse stale mappings and mounts before replacing this case's artifacts.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/prefix" bs=512 count=8 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/prefix")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/added" bs=512 count=8 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/added")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/sentinel" bs=512 count=2 iflag=fullblock status=none && test "$(stat -c %s "${dm_work}/sentinel")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Initialize only the maximum mapped ranges and their immediate outside sentinels.
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/zero of=/dev/vde bs=512 seek=4096 count=8 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/zero of=/dev/vdf bs=512 seek=8192 count=8 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/sentinel" of=/dev/vde bs=512 skip=0 seek=4104 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/sentinel" of=/dev/vdf bs=512 skip=0 seek=8200 count=2 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup create phase1_test --notable"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup load phase1_test --table "0 8 striped 2 2 /dev/vde 4096 /dev/vdf 8192""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup resume phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"blockdev --getsize64 /dev/dm-0"#,
        &Regex::new(r#"^4096$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/prefix" of=/dev/dm-0 bs=512 skip=0 seek=0 count=8 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/prefix" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4096 count=2 status=none > "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8192 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 2048"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4098 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 3072"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8194 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/prefix" "${dm_work}/physical""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup load phase1_test --table "0 16 striped 2 2 /dev/vde 4096 /dev/vdf 8192""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"blockdev --getsize64 /dev/dm-0"#,
        &Regex::new(r#"^4096$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup resume phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"blockdev --getsize64 /dev/dm-0"#,
        &Regex::new(r#"^8192$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/prefix" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="${dm_work}/added" of=/dev/dm-0 bs=512 skip=0 seek=8 count=8 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=8 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/added" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4100 count=2 status=none > "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8196 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 2048"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4102 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 3072"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8198 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/added" "${dm_work}/physical""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup suspend phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup load phase1_test --table "0 8 striped 2 2 /dev/vde 4096 /dev/vdf 8192""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"blockdev --getsize64 /dev/dm-0"#,
        &Regex::new(r#"^8192$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup resume phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"blockdev --getsize64 /dev/dm-0"#,
        &Regex::new(r#"^4096$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/actual" bs=512 skip=0 count=8 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/prefix" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/dm-0 of="${dm_work}/eof" bs=512 skip=8 count=1 status=none && test "$(stat -c %s "${dm_work}/eof")" -eq 0"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"if dd if=/dev/zero of=/dev/dm-0 bs=512 seek=8 count=1 conv=fsync,notrunc status=none; then false; else true; fi"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"if dd if=/dev/zero of=/dev/dm-0 bs=512 seek=10 count=1 conv=fsync,notrunc status=none; then false; else true; fi"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4096 count=2 status=none > "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8192 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 2048"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4098 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 3072"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8194 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/prefix" "${dm_work}/physical""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // The old tail now acts as nonzero sentinels on both disks; no data relocation is expected.
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4100 count=2 status=none > "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8196 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 2048"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde bs=512 skip=4102 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 3072"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf bs=512 skip=8198 count=2 status=none >> "${dm_work}/physical" && test "$(stat -c %s "${dm_work}/physical")" -eq 4096"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/added" "${dm_work}/physical""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vde of="${dm_work}/actual" bs=512 skip=4104 count=2 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/sentinel" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/vdf of="${dm_work}/actual" bs=512 skip=8200 count=2 status=none && test "$(stat -c %s "${dm_work}/actual")" -eq 1024"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "${dm_work}/sentinel" "${dm_work}/actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup remove phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"test ! -e /dev/dm-0"#, &Regex::new(r#"(?s).*"#).unwrap())?;
    Ok(())
}

// Verify a non-sparse ext2 payload across unmount, fsck, and remount.
#[nixos_test]
fn ext2_striped_cross_disk(nixos_shell: &mut Session) -> Result<(), Error> {
    // Refuse stale mappings and mounts before replacing this case's artifacts.
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-control-plane; test ! -L "$dm_work" && test ! -e /dev/dm-0 && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // A 32 MiB mapping uses 4 KiB chunks; initialize its 16 MiB range on each disk.
    // Raw mapping cases, not filesystem allocation assumptions, prove the physical layout.
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/zero of=/dev/vde bs=1M seek=16 count=16 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/zero of=/dev/vdf bs=1M seek=48 count=16 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup create phase1_test --notable"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup load phase1_test --table "0 65536 striped 2 8 /dev/vde 32768 /dev/vdf 98304""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup resume phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"blockdev --getsize64 /dev/dm-0"#,
        &Regex::new(r#"^33554432$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mkfs.ext2 -q -F -b 4096 -m 0 /dev/dm-0"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mkdir "${dm_work}/mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mount -t ext2 /dev/dm-0 "${dm_work}/mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="${dm_work}/mnt/payload" bs=1M count=24 iflag=fullblock conv=fsync status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"stat -c %s "${dm_work}/mnt/payload""#,
        &Regex::new(r#"^25165824$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"sha256sum "${dm_work}/mnt/payload" > "${dm_work}/payload.sha256""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(r#"sync"#, &Regex::new(r#"(?s).*"#).unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"umount "${dm_work}/mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"e2fsck -f -n /dev/dm-0"#,
        &Regex::new(r#"(?s)Pass 5:.*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mount -t ext2 /dev/dm-0 "${dm_work}/mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"stat -c %s "${dm_work}/mnt/payload""#,
        &Regex::new(r#"^25165824$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"sha256sum -c "${dm_work}/payload.sha256""#,
        &Regex::new(r#"(?m)^/tmp/aster-dm-control-plane/mnt/payload: OK$"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"umount "${dm_work}/mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dmsetup remove phase1_test"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"test ! -e /dev/dm-0"#, &Regex::new(r#"(?s).*"#).unwrap())?;
    Ok(())
}
