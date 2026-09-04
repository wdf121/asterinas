# Agents Guidelines for Asterinas

Asterinas is a Linux-compatible, general-purpose OS kernel
written in Rust using the framekernel architecture.
`unsafe` Rust is confined to OSTD (`ostd/`);
the kernel (`kernel/`) is entirely safe Rust.

## Repository Layout

| Directory    | Purpose                                                  |
|--------------|----------------------------------------------------------|
| `kernel/`    | Safe-Rust OS kernel (syscalls, VFS, networking, etc.)    |
| `ostd/`      | OS framework — the only crate permitted to use `unsafe`  |
| `osdk/`      | `cargo-osdk` CLI tool for building/running/testing       |
| `test/`      | Regression and syscall tests (C user-space programs)     |
| `distro/`    | Asterinas NixOS distribution configuration               |
| `tools/`     | Utility scripts (formatting, Docker, benchmarking, etc.) |
| `book/`      | The Asterinas Book (mdBook documentation)                |

## Building and Running

All development is done inside the project Docker container:

```bash
docker run -it --privileged --network=host -v /dev:/dev \
  -v $(pwd)/asterinas:/root/asterinas \
  asterinas/dev:0.18.1-20260805
```

Key Makefile targets:

| Command              | What it does                                         |
|----------------------|------------------------------------------------------|
| `make kernel`        | Build initramfs and the kernel                       |
| `make run_kernel`    | Build and run in QEMU                                |
| `make test`          | Unit tests for non-OSDK crates (`cargo test`)        |
| `make ktest`         | Kernel-mode unit tests via `cargo osdk test` in QEMU |
| `make check`         | Full lint: rustfmt, clippy, typos, license checks    |
| `make format`        | Auto-format Rust, Nix, and C code                    |
| `make docs`          | Build rustdocs for all crates                        |

Set `TARGET_ARCH` to `x86_64` (default), `riscv64`, or `loongarch64`.

## dm Branch Local Workflow

This branch keeps Device Mapper project progress in `log/device-mapper-progress.md`.
Stage-by-stage engineering changes are recorded in `log/YYYY-M-D.md`.

Local environment:

- Host repository path: `/root/atom/asterinas`.
- Working branch: `dm`.
- Docker container: `myAsterinas`.
- Container project path: `/root/asterinas`.
- Commands in project docs and examples assume they are run inside the container from `/root/asterinas`, unless explicitly marked as host commands.

Resource checks before commands that may consume noticeable CPU or memory,
especially builds, ktests, QEMU/NixOS runs, patch generation, and large document
rewrites:

```bash
free -h
uptime
ps -eo pid,ppid,comm,%mem,%cpu,rss --sort=-rss | head -15
```

Interpretation rules:

- Low `free` memory alone is not a blocker on Linux; check `available` because
  page cache under `buff/cache` is reclaimable.
- If `available` is below about 2 GiB, swap usage is growing, or load average is
  higher than the available CPU cores, avoid starting new parallel build/test
  jobs and report the resource pressure.
- If a long-running command stalls, inspect memory, CPU, and the top processes
  before retrying; only stop processes that were started for the current run.

Quick checks:

```bash
cargo fmt --check
git diff --check
git status --short
```

To narrowly run `aster-device-mapper` crate ktests, call the repository wrapper
from the repository root. The wrapper enters the target crate and supplies the
release, boot, KVM, initramfs, console, and timeout arguments:

```bash
myshell/ktest_crate.sh kernel/core/comps/device-mapper aster_device_mapper::table::tests::<test_name>
```

To narrowly run `aster-core` ioctl-layer ktests, use the same wrapper with the
`kernel/core` crate directory:

```bash
myshell/ktest_crate.sh kernel/core aster_core::device::misc::device_mapper::tests::<test_name>
```

Do not use root `make ktest CARGO_OSDK_TEST_ARGS="..."` as the default targeted
ktest entry because it can override Makefile-provided release, KVM, and
initramfs arguments. Use `myshell/ktest_crate.sh <crate-dir> <test-path>` for
targeted ktests. The wrapper writes one result log at `<crate-dir>/ktest.log`;
it contains the current crate's ktest results and omits QEMU startup and kernel
noise. It uses `timeout --foreground` so QEMU stays in the invoking terminal's
foreground process group during interactive runs.

Run QEMU, ktest, and NixOS system tests serially to avoid image lock conflicts,
especially around `test/initramfs/build/ext2.img`. For new NixOS system suite
runs, set `GUEST_READY_TIMEOUT=40` and `GUEST_QEMU_TIMEOUT=180`: the former
limits boot to the guest shell, while the latter limits the complete lifecycle
of each QEMU guest. If a ktest/QEMU/NixOS run makes no relevant progress for
about three minutes, suspect command filtering, wrong crate working directory,
root `make ktest` argument override, missing KVM or initramfs arguments,
leftover processes, or image-lock issues; inspect output and processes, stop
only processes started for the current run if needed, and retry with
`myshell/ktest_crate.sh` from the repository root.

Do not modify KVM, RELEASE, QEMU, NixOS boot protocol, or `myshell/br.sh` unless
explicitly requested. Run DM system tests through explicit canonical suite
entries; do not reintroduce a default all-in-one suite or compatibility aliases.

Run slower system tests only when the corresponding path changes or during stage
acceptance. The six canonical entries are:

```bash
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --control-plane
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --dataplane
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --lvm2-topology
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --linear-integration
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --striped-integration
GUEST_READY_TIMEOUT=40 GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --mixed-integration
```

Project logging rules:

- Before writing a dated log, run `date +%F` and write to `log/YYYY-M-D.md`.
- Restart stage numbering from 1 each day.
- Log by small stage in this order: background, changes, tests.
- Do not split logs by “production code / ktest”.
- Record only actual engineering changes and validation; do not log discussion,
  review-only work, or planning-only work.

## Toolchain

- **Rust nightly** pinned in `rust-toolchain.toml`.
- **Edition:** 2024.
- `rustfmt.toml`: imports grouped as Std / External / Crate
  (`imports_granularity = "Crate"`, `group_imports = "StdExternalCrate"`).
- Clippy lints are configured in the workspace `Cargo.toml`
  under `[workspace.lints.clippy]`.
  Every member crate must have `[lints] workspace = true`.

## Coding Guidelines

The coding guidelines are the authoritative standard
for both **writing** and **reviewing** code.
The guidelines are organized by **persona**:
five durable engineering roles,
each a page whose Index doubles as that persona's review checklist.
Consult the persona whose concern matches your change.
Each Index lists every guideline as a stable `short-name` paired with a one-line gist,
so you can grasp a rule from the table and open its full text only when needed.

| Persona | Focus | Index |
|---|---|---|
| Project maintainer | Is the code well-shaped and understandable? | [For Maintainability](book/src/to-contribute/coding-guidelines/for-maintainability/README.md) |
| Kernel developer | Is it correct and efficient? | [For Development](book/src/to-contribute/coding-guidelines/for-development/README.md) |
| Security expert | Is it safe and secure? | [For Security](book/src/to-contribute/coding-guidelines/for-security/README.md) |
| Hardware expert | Is it correct against the hardware contract? | [For Hardware](book/src/to-contribute/coding-guidelines/for-hardware/README.md) |
| Documentation writer | Are the user-facing docs well-written? | [For Documentation](book/src/to-contribute/coding-guidelines/for-documentation/README.md) |

## Architecture Notes

Asterinas is a framekernel
comprising a safe upper half (`kernel/`)
and an unsafe lower half (`ostd/`).

The `ostd` crate is a minimal Rust OS development framework
that encapsulates `unsafe` Rust code within safe APIs.
`unsafe` Rust code is confined to OSTD (`ostd/`);
kernel code under `kernel/` must remain safe Rust.

The kernel code under `kernel/` is written using `ostd` APIs.
In the current repository layout, the top-level `asterinas` crate
under `kernel/src/` is a thin assembler crate that enters
`aster_core::boot()`. Most Linux-compatible kernel semantics live in
`kernel/core/src/`, while concrete component crates live under
`kernel/core/comps/`, and reusable kernel libraries live under
`kernel/libs/`.

A practical dependency model is:

1. The assembler crate (`kernel/src/`) depends on `aster-core` and `ostd`.
2. The `aster-core` crate (`kernel/core/`) depends on component crates and libraries.
3. Component crates (`kernel/core/comps/`) provide concrete kernel subsystems.
4. Kernel libraries (`kernel/libs/`) provide reusable support crates.
5. OSTD (`ostd/`) provides the lower-half OS framework and safe APIs over unsafe internals.

Higher-level crates may depend on lower-level crates. Lower-level crates
should not depend on higher-level Linux semantics such as syscalls, VFS,
or Device Mapper ioctl behavior.

## CI

CI runs in the project Docker container with KVM.
Key test matrices:
- x86-64: lint, compile, usermode tests, kernel tests, integration tests
  (boot, syscall, general), multiple boot protocols, SMP configurations.
- RISC-V 64, LoongArch 64, and Intel TDX have dedicated workflows.
- License headers and SCML validation are also checked.
