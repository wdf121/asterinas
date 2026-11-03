// SPDX-License-Identifier: MPL-2.0

//! Device Mapper control-plane integration tests on Asterinas NixOS.

use nixos_test_framework::*;

nixos_test_main!();

#[nixos_test]
fn version(nixos_shell: &mut Session) -> Result<(), Error> {
    nixos_shell.run_cmd_and_expect("dmsetup version", "Driver version:")?;
    Ok(())
}

#[nixos_test]
fn create_tableless(nixos_shell: &mut Session) -> Result<(), Error> {
    nixos_shell.run_cmd_and_expect(
        "dmsetup create phase1_tableless --uuid phase1_tableless_uuid --notable; printf 'CREATE_EXIT=%s\\n' $?",
        "CREATE_EXIT=0",
    )?;
    Ok(())
}
