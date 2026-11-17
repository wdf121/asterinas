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

The `device-mapper-control-plane` suite contains 27 independently selectable cases,
grouped by behavior rather than giving each dmsetup command its own case.
The original 17 linear and general control cases remain unchanged:

- `interface_and_target_probes`
- `create_and_first_load`
- `numeric_backing_load`
- `first_linear_resume_reads_and_writes`
- `query_table_states`
- `suspend_and_resume_linear_mapping`
- `remove_mapping_lifecycle`
- `failed_load_preserves_active_mapping`
- `readonly_and_io_bounds`
- `multi_target_linear_reads_and_writes`
- `multi_disk_linear_reads_and_writes`
- `ext2_multi_target_same_disk`
- `ext2_multi_target_cross_disk`
- `implicit_table_replacement`
- `explicit_table_replacement`
- `raw_mapping_growth_and_shrink`
- `concurrent_hot_table_replacement`

Two additional control cases cover pending-table clearing:

| Case | Coverage |
|---|---|
| `clear_pending_table_preserves_active_mapping` | Clear a pending replacement while preserving the active table, capacity, and read/write path; a later resume must not restore the discarded replacement. |
| `clear_without_pending_table_and_remove` | Clear with no pending table, preserve a published primary node after clearing the first load, reject resume without a table, and reject control operations after removal. |

Eight additional cases cover striped mappings:

| Case | Coverage |
|---|---|
| `striped_cross_disk_reads_and_writes` | Numeric backing identities resolved at runtime, active/inactive queries, dependencies, healthy status, and independent two-disk physical layout checks in both I/O directions. |
| `striped_same_disk_reads_and_writes` | Two non-overlapping offsets on one backing, independent reads/writes, and dependency deduplication. |
| `mixed_linear_and_striped_reads_and_writes` | Linear/striped/linear composition, a non-row-aligned striped start, three-sector chunks, cross-target I/O, and neighboring data/sentinels. |
| `invalid_striped_load_preserves_tables` | Invalid geometry, parameters, and backings preserve both active and inactive tables; the original candidate remains usable. |
| `striped_readonly_and_io_bounds` | Read-only rejection, the last legal sector, short reads, rejected end/beyond-end writes, and nonzero physical sentinels. |
| `striped_table_replacement` | Implicit linear-to-striped and explicit striped-to-linear replacement, activation timing, capacities, and independent data layouts. |
| `striped_raw_mapping_growth_and_shrink` | Complete-row growth/shrink with fixed geometry, retained prefix, new-region I/O, and inaccessible but physically preserved truncated data. |
| `ext2_striped_cross_disk` | A 32 MiB striped mapping with 4 KiB chunks, a non-sparse 24 MiB payload, offline fsck, and size/SHA256 verification after remount. |

Each case prepares and removes its own mapping, so it can run alone through `NIXOS_TEST_CASE`.
The suite requires `raw_disk=2` and a clean guest without unrelated DM mappings.
It destructively overwrites ranges on `/dev/vde` and `/dev/vdf`;
striped cases initialize their tested backing ranges rather than relying on persistent disk contents.
Do not run this suite between LVM preparation and recovery verification,
on disks holding data to retain, or concurrently with another VM sharing the raw images.
After a failure, inspect leftover mappings, mounts, and background I/O before retrying.

Striped raw I/O checks compare fixed physical positions, not just mapper write/read round trips.
They check read exit status and byte length before comparing temporary files.
A failed crossing write may have committed its legal prefix; these cases do not assume rollback.
The tests do not prove a particular BIO split, flush count, health-failure transition, or power-loss safety.
Type conversion and resizing do not migrate data or resize filesystems.
Striped LVM, concurrent striped replacement, and cross-boot striped recovery are outside this suite's coverage.
Final dynamic acceptance should run the entire 27-case suite, not only the new cases:

```bash
# Rebuild the guest image only when needed.
make nixos NIXOS_TEST_SUITE=device-mapper-control-plane

# Destructive: check disk ownership and recovery state first.
make run_nixos NIXOS_TEST_SUITE=device-mapper-control-plane raw_disk=2
```

The registered case count and successful static checks are not evidence of a dynamic pass.

`concurrent_hot_table_replacement` runs a background write/read/checksum loop in an unchanged mapping prefix.
The foreground repeatedly adds a second-disk tail through implicit replacement and removes it through explicit suspend/load/resume.
It checks capacity, tail writes, progress after each resume, and the worker's exit status.
This is a bounded concurrency correctness test, not a throughput benchmark or proof of a particular kernel interleaving.
Each suite uses its own fixed guest directory: `/tmp/aster-dm-control-plane`,
`/tmp/aster-dm-lvm`, or `/tmp/aster-dm-lvm-recovery`.
Every case clears and recreates only its suite's directory before doing test work,
including when selected individually with `NIXOS_TEST_CASE`.
Cleanup refuses symbolic-link directories, remaining mounts at or below the directory,
or a failed mount inspection. The control-plane suite also refuses an existing `/dev/dm-0`;
the LVM suites refuse any published `/dev/dm-*` node. Inspect and resolve such
leftovers before retrying; the tests do not automatically unmount or remove stale mappings.
Mountpoints, table files, payloads, checksums, and worker output stay beneath the directory.
Cases still unmount filesystems, stop and wait for workers, and remove or deactivate mappings
as required, but do not delete the temporary directory at the end.
Files remain available for inspection until the next case starts; that case also deletes
any earlier failure artifacts in the same suite directory, including `dm_hot.log`.
Cleanup does not touch another suite's directory, persistent raw images, or the LVM state
retained for recovery. Directories left by older random-directory versions are not removed automatically.

The `device-mapper-lvm` suite is an **opt-in, destructive integration test**. The full suite requires `raw_disk=2` and overwrites the first two numbered images (`raw_test_1.img` and `raw_test_2.img`, `/dev/vde` and `/dev/vdf` in NixOS). Do not run it on images containing data to retain, or between the preparation and verification boots of a recovery test. The images are persistent and shared with the ordinary device-mapper suite, not reserved for LVM.

The suite contains three explicitly ordered cases in one preparation boot:

1. `prepare_shared_lvm_storage`: the only case that erases the disks and creates both PVs. It creates `phase1_offline_vg` with 4 MiB extents on `/dev/vde` only.
2. `linear_lv_offline_ext2_resize`: create `phase1_offline_vg/ext2_lv` at 192 MiB on `/dev/vde`, add `/dev/vdf` to the VG, extend the LV to 256 MiB using that second PV, then shrink back to 192 MiB. This incorporates the former ordinary create/use case's table, dependencies, raw I/O, and initial remount checks.
3. `striped_lv_offline_ext2_resize`: require the retained inactive linear LV and both PVs, then create `phase1_offline_vg/striped_ext2_lv`. Exercise the same 192 → 256 → 192 MiB lifecycle with two stripes and 64 KiB chunks, explicitly retaining that geometry when extending.

Both target workflows check raw I/O before formatting and ext2 data after the initial remount, growth, and shrink. Each leaves its own unmounted filesystem and inactive LV. Each filesystem contains a 48 MiB `retained` file, a 32 MiB `after-shrink` file, and their original SHA256 manifests with relative filenames. During growth, the additional non-sparse 160 MiB `added` file makes the total payload exceed the original LV size; that file and its manifest are deleted before shrinking ext2 and then the LV. Neither workflow deletes the shared VG, PVs, or the other LV.

The two 512 MiB PVs are budgeted individually, not just by total VG free space. Linear retains 48 extents on `/dev/vde`; its temporary extension uses 16 on `/dev/vdf`. Striped uses 24 extents per PV initially/finally and 32 at its peak. The combined peak allocation is therefore 80 extents on `/dev/vde` and 32 on `/dev/vdf`. Tests verify actual extent counts and allocations, including the second PV's capacity after it joins the VG.

These are **ordered integration cases**, not independently self-preparing tests. `NIXOS_TEST_CASE` is useful only with the correct existing predecessor state. A missing or incorrect prerequisite fails instead of being silently rebuilt. A partial prepare failure is not a valid recovery baseline. Inspect leftovers before explicitly restarting destructive shared preparation.

Do not rerun preparation, run another destructive disk test, run `make clean`, or share the images with another VM from the start of preparation until both recovery cases complete. Both final LVs must survive together for the separate `device-mapper-lvm-recovery` suite.

Concurrent-I/O and hot raw-mapping resize tests belong to the dmsetup suite, not these LVM workflows. Both preparation and recovery reuse `/dev/dm-0` for each LV: cases run sequentially, activate only their own LV, and deactivate it before the next case. They require no published `/dev/dm-*` nodes at each case boundary and check that `/dev/dm-0` is a block device after activation. They do not depend on `/dev/mapper` aliases or query LVM's kernel device numbers. The suite is not part of the ordinary device-mapper suite or the NixOS CI matrix.

Both LVM suites require `e2fsprogs` and `lvm2`.
The `device-mapper-lvm` suite's `extra_config.nix` sets `global/use_aio = 0` because Asterinas does not yet support Linux native AIO.
This selects LVM's synchronous I/O engine directly instead of warning and falling back on each invocation; it does not add kernel AIO support or change application I/O through activated LVs.
That suite also sets `activation/udev_rules = 0` and `activation/udev_sync = 0` so LVM manages its own device nodes and symlinks without waiting for udev notifications.
These overrides apply to images built for that suite, not the shared distribution defaults; the recovery suite does not independently specify them.
All three suite configurations supply these packages, but package availability alone does not establish an old image's LVM configuration.
The suite Rust sources and `extra_config.nix` files do not set `LVM_SYSTEM_DIR` or `use_devicesfile`.
The shared distribution configuration still disables the udev service (`services.udev.enable = false`); disabling that service alone does not explicitly select LVM's node-management policy.
`make run_nixos` recompiles the Rust runner but does not rebuild the guest image.
Before reusing an old image, inspect `printenv LVM_SYSTEM_DIR` and `lvmconfig --type full` inside the guest, as well as `/etc/lvm/lvm.conf` and any selected alternate LVM configuration directory.
If image configuration needs updating, rebuild with `make nixos NIXOS_TEST_SUITE=device-mapper-lvm` before preparation, not between preparation and recovery.
Removing temporary files is not a substitute for checking or rebuilding the image configuration.

### LVM recovery across VM boots

Run preparation to completion and wait for its VM to shut down before starting recovery:

```bash
# First boot: all three preparation cases; both LVs end unmounted and inactive.
make run_nixos NIXOS_TEST_SUITE=device-mapper-lvm raw_disk=2

# Second boot: reuse the same image and raw disks; no preparation or formatting.
make run_nixos NIXOS_TEST_SUITE=device-mapper-lvm-recovery raw_disk=2
```

If the preparation suite has already passed and its disks have not been changed, run only the second command.
Do not run another destructive disk suite or `make clean` between boots.
Each invocation starts a separate QEMU process; merely selecting another case within one invocation is not a reboot.
The framework sends `poweroff` and waits for QEMU to exit after the suite finishes.

The recovery suite has two explicitly registered cases in the same second boot:
`linear_lv_recovers_after_reboot` and `striped_lv_recovers_after_reboot`.
Each independently scans existing PV/VG metadata, locates and explicitly activates its own LV,
checks every actual DM table segment, backing dependencies, LV capacity, and ext2 size,
and mounts its filesystem read-only in a target-specific directory.
Each verifies both retained file sizes and the original on-disk SHA256 manifests,
and confirms the growth file and manifest are absent. Recovery never formats, reconstructs
missing volumes, repairs missing data, or generates replacement checksum baselines.
Each unmounts and deactivates only its own LV without deleting either LV, the shared VG/PVs,
or baselines, so verification can be repeated. Neither case depends on the other activating
its volume, and both expect the two prepared LVs to be inactive at their boundaries.
A clean guest with no published `/dev/dm-*` nodes is required before explicit activation.
The full acceptance run covers all three prepare cases followed by both recovery cases;
source registration counts and static checks alone are not dynamic pass results.

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
