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

- Repository path: `/root/atom/asterinas`.
- Working branch: `dm`.
- Docker container: `myAsterinas`.
- Container project path: `/root/asterinas`.
- Prefer running builds, ktests, and NixOS/LVM2 system tests inside the container:

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && <command>'
```

Resource checks before commands that may consume noticeable CPU or memory,
especially builds, ktests, QEMU/NixOS runs, patch generation, and large document
rewrites:

```bash
free -h
uptime
ps -eo pid,ppid,comm,%mem,%cpu,rss --sort=-rss | head -15
docker exec myAsterinas bash -lc 'free -h && uptime'
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
docker exec myAsterinas bash -lc 'cd /root/asterinas && cargo fmt --check'
git diff --check
git diff -- Cargo.toml
git status --short
```

To narrowly run `aster-device-mapper` crate ktests, temporarily reduce the root
`Cargo.toml` `default-members` to:

```toml
default-members = [
    "kernel/core/comps/device-mapper",
]
```

Then run, for example:

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && timeout -k 10s 180s make ktest CARGO_OSDK_TEST_ARGS="--kcmd-args=loglevel=error --kcmd-args=earlycon --kcmd-args=console=ttyS0 --boot-method=grub-rescue-iso --grub-boot-protocol=multiboot2 aster_device_mapper::table::tests::<test_name>"'
```

To narrowly run `aster-core` ioctl-layer ktests, temporarily reduce the root
`Cargo.toml` `default-members` to:

```toml
default-members = [
    "kernel/core",
]
```

Then run, for example:

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && timeout -k 10s 180s make ktest CARGO_OSDK_TEST_ARGS="--kcmd-args=loglevel=error --kcmd-args=earlycon --kcmd-args=console=ttyS0 --boot-method=grub-rescue-iso --grub-boot-protocol=multiboot2 aster_core::device::misc::device_mapper::tests::<test_name>"'
```

After any temporary `Cargo.toml` default-member change, restore `Cargo.toml` and
confirm `git diff -- Cargo.toml` has no output.

Run QEMU, ktest, and NixOS system tests serially to avoid image lock conflicts,
especially around `test/initramfs/build/ext2.img`. For NixOS system tests, set `GUEST_READY_TIMEOUT=180` so each single QEMU guest run has a three-minute full-lifecycle timeout. If a ktest/QEMU/NixOS run makes no relevant progress for about three
minutes, suspect command filtering, default-members, leftover processes, or
image-lock issues; inspect output and processes, stop only processes started for
the current run if needed, and retry with a narrower command.

Do not modify KVM, RELEASE, QEMU, NixOS boot protocol, or `myshell/br.sh` unless
explicitly requested. Run DM system tests through explicit suite entries; do not
reintroduce a default all-in-one suite, and keep linear/striped LVM2 execution
aligned as base plus cross-segment entries.

Run slower system tests only when the corresponding path changes or during stage
acceptance, for example:

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=180 myshell/run_dm_system_tests.sh --linear-lvm2'
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=180 myshell/run_dm_system_tests.sh --striped-lvm2'
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=180 myshell/run_dm_system_tests.sh --linear-lvm2-cross-segment'
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=180 myshell/run_dm_system_tests.sh --striped-lvm2-cross-segment'
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_READY_TIMEOUT=180 myshell/run_dm_system_tests.sh --mixed-lvm2'
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
