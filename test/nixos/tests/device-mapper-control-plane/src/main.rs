// SPDX-License-Identifier: MPL-2.0

//! Device Mapper control-plane integration tests on Asterinas NixOS.

use nixos_test_framework::*;

nixos_test_main!(
    order = [
        "version",
        "create_tableless",
        "first_linear_load_publishes_primary",
        "numeric_backing_load"
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
