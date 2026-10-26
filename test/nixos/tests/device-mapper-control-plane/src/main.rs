// SPDX-License-Identifier: MPL-2.0

//! Official framework entry points for the shell-based DM control-plane suite.
//!
//! The guest script owns commands, assertions, and per-case cleanup. Rust only
//! uploads it once per guest session and registers independently selectable cases.
//! The external runner retains disposable disks, QEMU supervision, and logging.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use nixos_test_framework::*;

/// Uploads the embedded script in PTY-safe chunks and runs one isolated case.
/// A content-derived path avoids reusing a stale script in an existing guest.
/// Both the exit status and the case marker must indicate success.
fn run_case(shell: &mut Session, name: &str) -> Result<(), Error> {
    let script = include_bytes!("../cases.sh");
    let mut hash = DefaultHasher::new();
    script.hash(&mut hash);
    let path = format!("/tmp/dm-control-plane-{:016x}.sh", hash.finish());
    let any_output = Regex::new("(?s).*").unwrap();
    match shell.run_cmd_and_expect_regex(&format!("test -f {path}"), &any_output) {
        Ok(()) => {}
        Err(Error::NonZeroExit { .. }) => {
            // Publish only a complete upload; interrupted writes remain unused.
            shell.run_cmd_and_expect_regex(&format!(": > {path}.tmp"), &any_output)?;
            // Octal escapes preserve all bytes without literal newlines. Allow
            // room for Session's quoting and status marker below the PTY limit.
            for chunk in script.chunks(700) {
                let encoded: String = chunk.iter().map(|byte| format!("\\{byte:03o}")).collect();
                shell.run_cmd_and_expect_regex(
                    &format!("printf '%b' '{encoded}' >> {path}.tmp"),
                    &any_output,
                )?;
            }
            shell.run_cmd_and_expect_regex(&format!("mv {path}.tmp {path}"), &any_output)?;
        }
        Err(error) => return Err(error),
    }
    shell.run_cmd_and_expect_regex(
        &format!("sh {path} {name}"),
        &Regex::new(&format!("(?m)^CASE_PASS_{name}\\r?$")).unwrap(),
    )
}

#[nixos_test]
fn dmsetup_static_queries(shell: &mut Session) -> Result<(), Error> {
    run_case(shell, "dmsetup_static_queries")
}

#[nixos_test]
fn backing_disks(shell: &mut Session) -> Result<(), Error> {
    run_case(shell, "backing_disks")
}

#[nixos_test]
fn tableless_create(shell: &mut Session) -> Result<(), Error> {
    run_case(shell, "tableless_create")
}

#[nixos_test]
fn first_load_resume_publication(shell: &mut Session) -> Result<(), Error> {
    run_case(shell, "first_load_resume_publication")
}

#[nixos_test]
fn linear_create_query(shell: &mut Session) -> Result<(), Error> {
    run_case(shell, "linear_create_query")
}

#[nixos_test]
fn structured_discovery(shell: &mut Session) -> Result<(), Error> {
    run_case(shell, "structured_discovery")
}

#[nixos_test]
fn striped_create_query(shell: &mut Session) -> Result<(), Error> {
    run_case(shell, "striped_create_query")
}

#[nixos_test]
fn error_zero_targets(shell: &mut Session) -> Result<(), Error> {
    run_case(shell, "error_zero_targets")
}

#[nixos_test]
fn table_lifecycle(shell: &mut Session) -> Result<(), Error> {
    run_case(shell, "table_lifecycle")
}

#[nixos_test]
fn suspend_resume_events(shell: &mut Session) -> Result<(), Error> {
    run_case(shell, "suspend_resume_events")
}

#[nixos_test]
fn rename_uuid(shell: &mut Session) -> Result<(), Error> {
    run_case(shell, "rename_uuid")
}

#[nixos_test]
fn readonly_busy_lifecycle(shell: &mut Session) -> Result<(), Error> {
    run_case(shell, "readonly_busy_lifecycle")
}

#[nixos_test]
fn remove_devices(shell: &mut Session) -> Result<(), Error> {
    run_case(shell, "remove_devices")
}

nixos_test_framework::nixos_test_main!();
