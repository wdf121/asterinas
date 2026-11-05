// SPDX-License-Identifier: MPL-2.0

//! Device Mapper control-plane integration tests on Asterinas NixOS.

use nixos_test_framework::*;

nixos_test_main!(
    order = [
        "version",
        "create_tableless",
        "first_linear_load_publishes_primary",
        "numeric_backing_load",
        "first_linear_resume_reads_and_writes",
        "multi_target_linear_reads_and_writes",
        "multi_disk_linear_reads_and_writes",
        "active_failure_readonly_and_bounds"
    ]
);

#[nixos_test]
fn version(nixos_shell: &mut Session) -> Result<(), Error> {
    nixos_shell.run_cmd_and_expect("dmsetup version", "Driver version:")?;
    Ok(())
}

#[nixos_test]
fn create_tableless(nixos_shell: &mut Session) -> Result<(), Error> {
    nixos_shell.run_cmd_and_expect_display(
        "dmsetup create phase1_tableless --uuid phase1_tableless_uuid --notable",
        "dmsetup create phase1_tableless --uuid phase1_tableless_uuid --notable; printf 'CREATE_EXIT=%s\\n' $?",
        "CREATE_EXIT=0",
    )?;
    Ok(())
}

/// Observe the automatically allocated minor by comparing primary nodes
/// before and after LOAD. The official NixOS test entry supplies a fresh raw
/// backing disk at /dev/vde; without REMOVE, the mapper lasts until VM restart.
#[nixos_test]
fn first_linear_load_publishes_primary(nixos_shell: &mut Session) -> Result<(), Error> {
    nixos_shell.run_cmd_and_expect_display(
        "check fresh raw backing disk /dev/vde and record existing /dev/dm-* nodes",
        r#"bash -e -c 'test -b /dev/vde; dd if=/dev/vde bs=512 count=1 status=none | cmp - <(head -c 512 /dev/zero); printf "%s " /dev/dm-* > /tmp/phase1_load_before_nodes; echo LOAD_PRECONDITIONS_OK'"#,
        "LOAD_PRECONDITIONS_OK",
    )?;
    nixos_shell.run_cmd_and_expect_display(
        "dmsetup create phase1_load --notable",
        r#"bash -e -c 'dmsetup create phase1_load --notable; echo CREATE_OK'"#,
        "CREATE_OK",
    )?;
    nixos_shell.run_cmd_and_expect_display(
        "check CREATE has not published a /dev/dm-* node",
        r#"bash -e -c 'printf "%s " /dev/dm-* | cmp - /tmp/phase1_load_before_nodes; echo CREATE_WITHOUT_NODE_OK'"#,
        "CREATE_WITHOUT_NODE_OK",
    )?;
    nixos_shell.run_cmd_and_expect_display(
        "dmsetup load phase1_load --table \"0 8 linear /dev/vde 0\"",
        r#"bash -e -c 'dmsetup load phase1_load --table "0 8 linear /dev/vde 0"; echo LOAD_OK'"#,
        "LOAD_OK",
    )?;
    nixos_shell.run_cmd_and_expect_display(
        "check newly published /dev/dm-N block node and device number",
        r#"bash -e -c 'before=" $(cat /tmp/phase1_load_before_nodes)"; major=$(awk "/device-mapper/ { print \$1; exit }" /proc/devices); test -n "$major"; count=0; for node in /dev/dm-*; do test -b "$node" || continue; case "$before" in *" $node "*) continue;; esac; count=$((count + 1)); minor=${node#/dev/dm-}; test "$(stat -c %t "$node")" = "$(printf %x "$major")"; test "$((16#$(stat -c %T "$node")))" -eq "$minor"; echo "LOAD_PRIMARY_NODE=$node"; done; test "$count" -eq 1; echo LOAD_PRIMARY_OK'"#,
        "LOAD_PRIMARY_OK",
    )?;
    Ok(())
}

/// Verify the first RESUME maps writes and reads through the new primary node.
#[nixos_test]
fn first_linear_resume_reads_and_writes(nixos_shell: &mut Session) -> Result<(), Error> {
    nixos_shell.run_cmd_and_expect_display(
        "create and load a linear mapper on the fresh raw test disk",
        r#"bash -e -c 'test -b /dev/vde; printf "%s " /dev/dm-* > /tmp/phase1_resume_before; dmsetup create phase1_resume --notable; dmsetup load phase1_resume --table "0 8 linear /dev/vde 8"; before=" $(cat /tmp/phase1_resume_before)"; count=0; for node in /dev/dm-*; do test -b "$node" || continue; case "$before" in *" $node "*) continue;; esac; count=$((count + 1)); printf "%s" "$node" > /tmp/phase1_resume_node; done; test "$count" -eq 1; echo RESUME_LOAD_OK'"#,
        "RESUME_LOAD_OK",
    )?;
    nixos_shell.run_cmd_and_expect_display(
        "resume and verify mapped writes and reads against backing",
        r#"bash -e -c 'dmsetup resume phase1_resume; node=$(cat /tmp/phase1_resume_node); test -b "$node"; printf "phase1-linear-resume" | dd of="$node" bs=512 seek=1 conv=sync,notrunc status=none; printf "phase1-linear-resume" | dd bs=512 conv=sync status=none | cmp - <(dd if=/dev/vde bs=512 skip=9 count=1 status=none); dd if=/dev/vde bs=512 skip=9 count=1 status=none | cmp - <(dd if="$node" bs=512 skip=1 count=1 status=none); echo RESUME_LINEAR_IO_OK'"#,
        "RESUME_LINEAR_IO_OK",
    )?;
    Ok(())
}

/// Cross a target boundary within one sector-aligned block-device request.
#[nixos_test]
fn multi_target_linear_reads_and_writes(nixos_shell: &mut Session) -> Result<(), Error> {
    check_multi_target_linear_io(nixos_shell, "/dev/vde", 32, 64, "phase1_multi")
}

/// Exercise cross-target forwarding to distinct fresh backing devices.
#[nixos_test]
fn multi_disk_linear_reads_and_writes(nixos_shell: &mut Session) -> Result<(), Error> {
    check_multi_target_linear_io(nixos_shell, "/dev/vdf", 96, 128, "phase1_two_disk")
}

fn check_multi_target_linear_io(
    nixos_shell: &mut Session,
    second_path: &str,
    first_sector: u64,
    second_sector: u64,
    name: &str,
) -> Result<(), Error> {
    let command = |body: &str| {
        format!(
            "bash -euo pipefail -c 'second={second_path}; first_sector={first_sector}; second_sector={second_sector}; name={name}; {body}'"
        )
    };
    nixos_shell.run_cmd_and_expect_display(
        "load and activate two linear targets with discontiguous backing ranges",
        &command(r#"test -b /dev/vde; test -b "$second"; if [ "$second" != /dev/vde ]; then test "$(stat -c %t:%T /dev/vde)" != "$(stat -c %t:%T "$second")"; fi; printf "%s " /dev/dm-* > /tmp/phase1_multi_before; dmsetup create "$name" --notable; dmsetup load "$name" --table "$(printf "0 4 linear /dev/vde %s\n4 4 linear %s %s\n" "$first_sector" "$second" "$second_sector")"; before=" $(cat /tmp/phase1_multi_before)"; count=0; for node in /dev/dm-*; do test -b "$node" || continue; case "$before" in *" $node "*) continue;; esac; count=$((count + 1)); printf "%s" "$node" > /tmp/phase1_multi_node; done; test "$count" -eq 1; dmsetup resume "$name"; echo MULTI_RESUME_OK"#),
        "MULTI_RESUME_OK",
    )?;
    nixos_shell.run_cmd_and_expect_display(
        "verify fresh ranges and cross-target writes against both backing offsets",
        &command(r#"node=$(cat /tmp/phase1_multi_node); dd if=/dev/vde bs=512 skip="$first_sector" count=4 status=none | cmp - <(head -c 2048 /dev/zero); dd if="$second" bs=512 skip="$second_sector" count=4 status=none | cmp - <(head -c 2048 /dev/zero); for value in A B C D E F G H; do head -c 512 /dev/zero | tr "\000" "$value"; done > /tmp/phase1_multi_expected; dd if=/tmp/phase1_multi_expected of="$node" bs=4096 count=1 conv=notrunc status=none; dd if=/dev/vde bs=512 skip="$first_sector" count=4 status=none > /tmp/phase1_multi_backing; dd if="$second" bs=512 skip="$second_sector" count=4 status=none >> /tmp/phase1_multi_backing; cmp /tmp/phase1_multi_expected /tmp/phase1_multi_backing; dd if="$node" bs=4096 count=1 status=none | cmp - /tmp/phase1_multi_expected; echo MULTI_WRITE_OK"#),
        "MULTI_WRITE_OK",
    )?;
    nixos_shell.run_cmd_and_expect_display(
        "seed backing independently and verify cross-target reads and untouched guards",
        &command(r#"node=$(cat /tmp/phase1_multi_node); for value in a b c d e f g h; do head -c 512 /dev/zero | tr "\000" "$value"; done > /tmp/phase1_multi_expected; dd if=/tmp/phase1_multi_expected of=/dev/vde bs=512 seek="$first_sector" count=4 conv=notrunc status=none; dd if=/tmp/phase1_multi_expected of="$second" bs=512 skip=4 seek="$second_sector" count=4 conv=notrunc status=none; dd if="$node" bs=4096 count=1 status=none | cmp - /tmp/phase1_multi_expected; for offset in "$((first_sector - 1))" "$((first_sector + 4))"; do dd if=/dev/vde bs=512 skip="$offset" count=1 status=none | cmp - <(head -c 512 /dev/zero); done; for offset in "$((second_sector - 1))" "$((second_sector + 4))"; do dd if="$second" bs=512 skip="$offset" count=1 status=none | cmp - <(head -c 512 /dev/zero); done; echo MULTI_READ_GUARDS_OK"#),
        "MULTI_READ_GUARDS_OK",
    )?;
    nixos_shell.run_cmd_and_expect_display(
        "read and overwrite only the two sectors straddling the target boundary",
        &command(r#"node=$(cat /tmp/phase1_multi_node); dd if="$node" bs=512 skip=3 count=2 status=none | cmp - <(dd if=/tmp/phase1_multi_expected bs=512 skip=3 count=2 status=none); head -c 512 /dev/zero | tr "\000" X > /tmp/phase1_partial; head -c 512 /dev/zero | tr "\000" Y >> /tmp/phase1_partial; dd if=/tmp/phase1_partial of="$node" bs=1024 oflag=seek_bytes seek=1536 conv=notrunc status=none; dd if=/tmp/phase1_partial of=/tmp/phase1_multi_expected bs=512 seek=3 conv=notrunc status=none; dd if=/dev/vde bs=512 skip="$first_sector" count=4 status=none > /tmp/phase1_multi_backing; dd if="$second" bs=512 skip="$second_sector" count=4 status=none >> /tmp/phase1_multi_backing; cmp /tmp/phase1_multi_expected /tmp/phase1_multi_backing; dd if="$node" bs=4096 count=1 status=none | cmp - /tmp/phase1_multi_expected; echo PARTIAL_CROSS_OK"#),
        "PARTIAL_CROSS_OK",
    )?;
    Ok(())
}

// LOAD-only coverage, independent of the later activation milestone.
#[nixos_test]
fn numeric_backing_load(nixos_shell: &mut Session) -> Result<(), Error> {
    nixos_shell.run_cmd_and_expect_display(
        "load a table using the real backing major:minor",
        r#"bash -euo pipefail -c 'id=$(stat -c "%t %T" /dev/vde); read -r major minor <<< "$id"; backing="$((16#$major)):$((16#$minor))"; dmsetup create phase1_numeric_load --notable; dmsetup load phase1_numeric_load --table "0 8 linear $backing 160"; echo NUMERIC_LOAD_OK'"#,
        "NUMERIC_LOAD_OK",
    )?;
    Ok(())
}

#[nixos_test]
fn active_failure_readonly_and_bounds(nixos_shell: &mut Session) -> Result<(), Error> {
    nixos_shell.run_cmd_and_expect_display(
        "activate a numeric-backing mapper and retain its node",
        r#"bash -euo pipefail -c 'printf "%s " /dev/dm-* > /tmp/phase1_checks_before; read -r major minor < <(stat -c "%t %T" /dev/vde); backing="$((16#$major)):$((16#$minor))"; head -c 4096 /dev/zero | tr "\000" S > /tmp/phase1_checks_expected; dd if=/tmp/phase1_checks_expected of=/dev/vde bs=512 seek=192 conv=notrunc status=none; dmsetup create phase1_checks --notable; dmsetup load phase1_checks --table "0 8 linear $backing 192"; before=" $(cat /tmp/phase1_checks_before)"; count=0; for node in /dev/dm-*; do test -b "$node" || continue; case "$before" in *" $node "*) continue;; esac; count=$((count+1)); printf "%s" "$node" > /tmp/phase1_checks_node; done; test "$count" -eq 1; dmsetup resume phase1_checks; dd if="$(cat /tmp/phase1_checks_node)" bs=4096 count=1 status=none | cmp - /tmp/phase1_checks_expected; echo NUMERIC_IO_OK'"#,
        "NUMERIC_IO_OK",
    )?;
    nixos_shell.run_cmd_and_expect_display(
        "failed LOAD and refused hot replacement keep the original active mapping usable",
        r#"bash -euo pipefail -c 'node=$(cat /tmp/phase1_checks_node); if dmsetup load phase1_checks --table "0 8 linear /dev/vde 999999999"; then exit 1; fi; dd if="$node" bs=4096 count=1 status=none | cmp - /tmp/phase1_checks_expected; dmsetup load phase1_checks --table "0 8 linear /dev/vde 224"; if dmsetup resume phase1_checks; then exit 1; fi; dd if="$node" bs=4096 count=1 status=none | cmp - /tmp/phase1_checks_expected; head -c 512 /dev/zero | tr "\000" W > /tmp/phase1_checks_patch; dd if=/tmp/phase1_checks_patch of="$node" bs=512 conv=notrunc status=none; dd if=/tmp/phase1_checks_patch of=/tmp/phase1_checks_expected bs=512 conv=notrunc status=none; dd if=/dev/vde bs=512 skip=192 count=8 status=none | cmp - /tmp/phase1_checks_expected; dd if=/dev/vde bs=512 skip=224 count=8 status=none | cmp - <(head -c 4096 /dev/zero); echo ACTIVE_PRESERVED_OK'"#,
        "ACTIVE_PRESERVED_OK",
    )?;
    nixos_shell.run_cmd_and_expect_display(
        "verify short I/O at the mapper end and rejected writes beyond it",
        r#"bash -euo pipefail -c 'node=$(cat /tmp/phase1_checks_node); dd if="$node" bs=512 skip=7 count=2 status=none > /tmp/phase1_checks_tail; test "$(stat -c %s /tmp/phase1_checks_tail)" -eq 512; dd if=/tmp/phase1_checks_expected bs=512 skip=7 count=1 status=none | cmp - /tmp/phase1_checks_tail; head -c 1024 /dev/zero | tr "\000" T > /tmp/phase1_checks_tail_input; if dd if=/tmp/phase1_checks_tail_input of="$node" bs=1024 oflag=seek_bytes seek=3584 conv=notrunc status=none; then exit 1; fi; dd if=/tmp/phase1_checks_tail_input of=/tmp/phase1_checks_expected bs=512 seek=7 count=1 conv=notrunc status=none; dd if="$node" bs=4096 count=1 status=none | cmp - /tmp/phase1_checks_expected; for sector in 8 9; do dd if="$node" bs=512 skip="$sector" count=1 status=none > /tmp/phase1_checks_eof; test ! -s /tmp/phase1_checks_eof; if dd if=/tmp/phase1_checks_patch of="$node" bs=512 seek="$sector" conv=notrunc status=none; then exit 1; fi; done; dd if=/dev/vde bs=512 skip=200 count=1 status=none | cmp - <(head -c 512 /dev/zero); echo BOUNDS_OK'"#,
        "BOUNDS_OK",
    )?;
    nixos_shell.run_cmd_and_expect_display(
        "activate a readonly mapping and check reads succeed while writes leave backing unchanged",
        r#"bash -euo pipefail -c 'printf "%s " /dev/dm-* > /tmp/phase1_ro_before; dd if=/tmp/phase1_checks_patch of=/dev/vde bs=512 seek=256 conv=notrunc status=none; dmsetup create phase1_readonly --notable; dmsetup load --readonly phase1_readonly --table "0 8 linear /dev/vde 256"; before=" $(cat /tmp/phase1_ro_before)"; count=0; for node in /dev/dm-*; do test -b "$node" || continue; case "$before" in *" $node "*) continue;; esac; count=$((count+1)); printf "%s" "$node" > /tmp/phase1_ro_node; done; test "$count" -eq 1; dmsetup resume phase1_readonly; node=$(cat /tmp/phase1_ro_node); dd if="$node" bs=512 count=1 status=none | cmp - /tmp/phase1_checks_patch; if dd if=/dev/zero of="$node" bs=512 count=1 conv=notrunc status=none; then exit 1; fi; dd if=/dev/vde bs=512 skip=256 count=1 status=none | cmp - /tmp/phase1_checks_patch; dd if="$node" bs=512 count=1 status=none | cmp - /tmp/phase1_checks_patch; echo READONLY_OK'"#,
        "READONLY_OK",
    )?;
    Ok(())
}
