# NixOS-Based Test Suites

This directory contains NixOS-based tests and a framework for writing and running them. The framework executes tests by interacting with a live instance of the operating system in a virtual environment. Thanks to this interactive design, the framework can test virtually any behavior that a real user could trigger through a terminal. It also offers a simple, imperative API, making it easy to write and maintain these interactive test scenarios.

## Directory Structure

```
test/nixos/
├── common/
│   ├── template/               # Template for creating new tests
│   └── ...                     # Core implementation of the framework
├── tests/
│   ├── system-core/            # A real test crate
│   │   ├── Cargo.toml
│   │   ├── extra_config.nix    # (Optional) Additional NixOS configuration
│   │   └── src/
│   │       └── main.rs
│   └── ...                     # Other tests
└── Makefile
```

## Creating a New Test

### Step 1: Copy the Template

```bash
cd test/nixos
cp -r common/template tests/my-test
```

### Step 2: Update `Cargo.toml`

Replace `<test_name>` with your test name:

### Step 3: Implement Your Tests

Edit `src/main.rs`:

```rust
// SPDX-License-Identifier: MPL-2.0

use nixos_test_framework::*;
use nixos_test_macro::nixos_test;

// This macro generates the main function that runs all registered tests
nixos_test_main!();

// Register a test case using the #[nixos_test] attribute.
// Name test cases with a `<tool>_<action>`-style identifier so the output
// groups naturally by tool, for example `echo_print_message` or
// `touch_create_file`.
#[nixos_test]
fn echo_print_message(nixos_shell: &mut Session) -> Result<(), Error> {
    nixos_shell.run_cmd("echo 'Hello, World!'")?;
    nixos_shell.run_cmd_and_expect("cat /etc/os-release", "NixOS")?;
    Ok(())
}

// You can define multiple test cases in the same file
#[nixos_test]
fn touch_create_file(nixos_shell: &mut Session) -> Result<(), Error> {
    nixos_shell.run_cmd("touch /tmp/test.txt")?;
    nixos_shell.run_cmd_and_expect("ls /tmp", "test.txt")?;
    Ok(())
}
```

The `Session` type provides APIs for interacting with the VM. See the [Session API documentation](common/framework/src/session.rs) for details.

When one test covers multiple closely related commands from the same package,
keep the same left-hand grouping and make the right-hand side describe the
behavior being verified, such as `coreutils_cat` or `findutils_xargs`.

### Documentation Maintenance

If a test suite verifies applications that are documented in Asterinas Book, keep the test suite and the corresponding "Verified Usage" section in sync. Whenever you add, remove, or change covered behavior in `test/nixos/tests/<suite>/`, review the matching documentation under `book/src/distro/popular-applications` and update it if needed.

**Note**: `book/src/distro/popular-applications/desktop-environments-and-display/` intentionally has no counterpart under `test/nixos/tests/` and does not appear in the NixOS test matrix. GUI and display validation are out of scope for the current headless NixOS test runner.

### Step 4: (Optional) Configure NixOS

If your test requires additional packages or system configuration, edit `extra_config.nix`:

```nix
{ config, lib, pkgs, ... }:

{
    environment.systemPackages = with pkgs; [
        # Add required packages here
        vim
        git
    ];

    # Configure system services
    virtualisation.podman.enable = true;
}
```
This content of this file will be merged with the [default configuration file](../../distro/etc_nixos/configuration.nix) to generate the final configuration file for the testing Asterinas NixOS system.

### Persistent raw test disks

Set `raw_disk=2` when running tests that need `/dev/vde` and `/dev/vdf`:

```bash
make run_nixos NIXOS_TEST_SUITE=device-mapper-control-plane raw_disk=2
```

Both the initramfs and NixOS runners use the **same** numbered images, `test/initramfs/build/raw_test_1.img` through `raw_test_18.img`. `raw_disk=N` attaches the first N images. Missing images are created as 512 MiB unformatted disks; existing images are reused without clearing, resizing, or formatting them. An existing image with a different size causes an error rather than being silently changed. The `/dev/vd*` names differ between the two guests because their other devices differ; the underlying numbered host files are identical.

The images live in the initramfs build directory, so `make clean` deletes them along with other initramfs build artifacts. Ordinary test runs and VM shutdowns do not clear them. Existing device-mapper tests assume blank backing ranges; run tests that consume a prior boot's data separately from tests that assume blank disks. A recovery test must run setup and verification in separate VM boots.

The `device-mapper-control-plane` suite groups checks by behavior rather than giving each dmsetup command its own case.
Related probes, creation/loading, state queries, suspend/resume, removal states, and capacity changes share focused lifecycle cases.
Same-disk and cross-disk I/O, ext2 workflows, explicit and implicit replacement, and concurrent replacement remain independently selectable.
Each case prepares and removes its own mapping, so it can run alone through `NIXOS_TEST_CASE`.
The suite requires a clean guest without unrelated DM mappings and destructively overwrites ranges on both raw disks.
After a failure, inspect leftover mappings, mounts, and background I/O before retrying.

`concurrent_hot_table_replacement` runs a background write/read/checksum loop in an unchanged mapping prefix.
The foreground repeatedly adds a second-disk tail through implicit replacement and removes it through explicit suspend/load/resume.
It checks capacity, tail writes, progress after each resume, and the worker's exit status.
This is a bounded concurrency correctness test, not a throughput benchmark or proof of a particular kernel interleaving.
Each suite uses its own fixed guest directory: `/tmp/aster-dm-control-plane`,
`/tmp/aster-dm-lvm`, or `/tmp/aster-dm-lvm-recovery`.
Every case clears and recreates only its suite's directory before doing test work,
including when selected individually with `NIXOS_TEST_CASE`.
Cleanup refuses symbolic-link directories, an existing `/dev/dm-0`, remaining mounts
at or below the directory, or a failed mount inspection. Inspect and resolve such
leftovers before retrying; the tests do not automatically unmount or remove stale mappings.
Mountpoints, table files, payloads, checksums, and worker output stay beneath the directory.
Cases still unmount filesystems, stop and wait for workers, and remove or deactivate mappings
as required, but do not delete the temporary directory at the end.
Files remain available for inspection until the next case starts; that case also deletes
any earlier failure artifacts in the same suite directory, including `dm_hot.log`.
Cleanup does not touch another suite's directory, persistent raw images, or the LVM state
retained for recovery. Directories left by older random-directory versions are not removed automatically.

The `device-mapper-lvm` suite is an **opt-in, destructive integration test**. The full suite requires `raw_disk=2` and overwrites the first two numbered images (`raw_test_1.img` and `raw_test_2.img`, `/dev/vde` and `/dev/vdf` in NixOS). Do not run it on images containing data to retain, or between the preparation and verification boots of a recovery test. The images are persistent and shared with the ordinary device-mapper suite, not reserved for LVM.

The suite contains two single-boot functional workflows:

- `linear_lv_create_and_use`: create and activate a single-PV linear LV, verify raw I/O and an ext2 file after remount, then remove the VG and PV.
- `linear_lv_offline_ext2_resize`: grow a 192 MiB LV to 256 MiB using a second PV, grow unmounted ext2, and verify non-sparse file data exceeding the original segment. Remove the growth payload, shrink ext2 before reducing the LV back to 192 MiB, and verify retained data and new writes after remount.

The final workflow deliberately preserves `phase1_offline_vg/ext2_lv` and both PVs for a later recovery-verification boot. It unmounts ext2 and deactivates the LV. The filesystem contains `retained`, `after-shrink`, and their original SHA256 manifests with relative filenames. The `added` payload and its manifest have been removed. Do not rerun this suite, run another destructive disk test, or run `make clean` before consuming that state. Cross-boot verification is performed by the separate `device-mapper-lvm-recovery` suite.

Concurrent-I/O and hot raw-mapping resize tests belong to the dmsetup suite, not these LVM workflows. Cases use `/dev/dm-0` and require a clean guest with no unrelated mappers. After a failure, inspect leftover mounts and mappings before retrying. The suite is not part of the ordinary device-mapper suite or the NixOS CI matrix.

Both LVM suites use the guest's default LVM configuration and require `e2fsprogs` and `lvm2`.
All three suite configurations supply these packages, but package availability alone does not establish an old image's LVM configuration.
The suite Rust sources and `extra_config.nix` files do not set `LVM_SYSTEM_DIR`, `udev_sync`, `udev_rules`, or `use_devicesfile`.
The shared distribution configuration still disables the udev service (`services.udev.enable = false`); this is separate from LVM's own synchronization settings.
`make run_nixos` recompiles the Rust runner but does not rebuild the guest image.
Before reusing an old image, inspect `printenv LVM_SYSTEM_DIR` and `lvmconfig --type full` inside the guest, as well as `/etc/lvm/lvm.conf` and any selected alternate LVM configuration directory.
If image configuration needs updating, rebuild with `make nixos NIXOS_TEST_SUITE=device-mapper-lvm` before preparation, not between preparation and recovery.
Removing temporary files is not a substitute for checking or rebuilding the image configuration.

### LVM recovery across VM boots

Run preparation to completion and wait for its VM to shut down before starting recovery:

```bash
# First boot: destructive preparation, ending with an unmounted, inactive LV.
make run_nixos NIXOS_TEST_SUITE=device-mapper-lvm raw_disk=2

# Second boot: reuse the same image and raw disks; no preparation or formatting.
make run_nixos NIXOS_TEST_SUITE=device-mapper-lvm-recovery raw_disk=2
```

If the preparation suite has already passed and its disks have not been changed, run only the second command.
Do not run another destructive disk suite or `make clean` between boots.
Each invocation starts a separate QEMU process; merely selecting another case within one invocation is not a reboot.
The framework sends `poweroff` and waits for QEMU to exit after the suite finishes.

The recovery case scans existing PV/VG metadata, explicitly activates `phase1_offline_vg/ext2_lv`, checks the LV and ext2 sizes, and mounts it read-only.
It checks the retained file sizes, validates the original on-disk SHA256 manifests, and confirms the growth payload is absent.
It then unmounts and deactivates the LV without deleting any PV, VG, LV, or baseline, so verification can be repeated in another boot.
A clean guest with no active `/dev/dm-0` mapping is required before explicit activation.

Explicit deactivation in preparation defines a controlled end state; it is not required to make LVM metadata persistent.
The filesystem is unmounted first to flush its data and metadata before the mapping is removed.
Across shutdown, the in-memory DM table disappears while the PV/VG/LV metadata remains on the raw disks.
The next boot must reconstruct the table from that metadata before mounting the filesystem.
This tests clean-shutdown persistence and explicit reactivation, not sudden power-loss recovery or automatic boot activation.

Automatic activation is a separate userspace boot policy, not something persistent DM tables provide.
It can be implemented with a boot service or a working LVM/udev autoactivation path, followed by an explicitly configured mount.
That requires validating the guest's startup ordering, device discovery, and device paths; these suites do not install such a policy.
Automatically launching the next QEMU after `poweroff` is instead host-side orchestration and does not prove guest autoactivation.

## Running Tests

The following commands should be run under the project root.

### Build Test Image

```bash
# Build NixOS image for a test suite
make nixos NIXOS_TEST_SUITE=my-test

# Or build using ISO installer workflow
make iso NIXOS_TEST_SUITE=my-test
make run_iso
```

### Run Tests

```bash
# Run all tests in the suite
make run_nixos NIXOS_TEST_SUITE=my-test

# Run a specific test case
make run_nixos NIXOS_TEST_SUITE=my-test NIXOS_TEST_CASE=echo_print_message

# Customize timeout with units (default: 10min)
make run_nixos NIXOS_TEST_SUITE=my-test NIXOS_TEST_TIMEOUT=10min    # 10 minutes
make run_nixos NIXOS_TEST_SUITE=my-test NIXOS_TEST_TIMEOUT=600s   # 600 seconds
make run_nixos NIXOS_TEST_SUITE=my-test NIXOS_TEST_TIMEOUT=600000ms  # 600000 milliseconds
```

### Complete Workflow Examples

```bash
# Quick test
make nixos NIXOS_TEST_SUITE=my-test && make run_nixos NIXOS_TEST_SUITE=my-test

# Test with ISO installer
make iso NIXOS_TEST_SUITE=my-test && make run_iso && make run_nixos NIXOS_TEST_SUITE=my-test

# Run specific test with custom timeout (10 minutes)
make nixos NIXOS_TEST_SUITE=containerization-and-virtualization
make run_nixos NIXOS_TEST_SUITE=containerization-and-virtualization NIXOS_TEST_CASE=podman_run_alpine_container NIXOS_TEST_TIMEOUT=10min
```

## Variables

Make variables:
- **`NIXOS_TEST_SUITE`**: Name of the test suite to run (required for test mode)
- **`NIXOS_TEST_CASE`**: Specific test case to run (optional, runs all if not specified)

Framework environment variables:
- **`NIXOS_TEST_TIMEOUT`**: Timeout for command execution with unit suffix (optional, default: 10min)
  - Supported formats: `<number>ms` (milliseconds), `<number>s` (seconds), `<number>min` (minutes)
  - Examples: `300000ms`, `300s`, `5min`
