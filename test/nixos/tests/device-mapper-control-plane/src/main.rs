// SPDX-License-Identifier: MPL-2.0

//! dmsetup command semantics, linear I/O, and concurrent raw table replacement.
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
    nixos_shell.run_cmd_and_expect_regex(
        "umount \"${dm_work}/phase1_dm_mnt\"",
        &Regex::new("(?s).*").unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex("dmsetup remove phase1_test", &Regex::new("(?s).*").unwrap())?;
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
