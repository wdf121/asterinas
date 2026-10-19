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
docker run -it --name fork_Asterinas --privileged --network=host -v /dev:/dev \
  -v /root/github/asterinas:/root/asterinas \
  asterinas/dev:0.18.1-20260926
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
Daily engineering, learning, analysis/review, validation, failures, and conclusions are recorded in `log/daily/YYYY-M-D.md`; unexecuted plans must not be presented as completed work.

Local environment:

- Host repository path: `/root/github/asterinas`.
- Working branch: `dm`.
- Docker container: `fork_Asterinas`.
- Container project path: `/root/asterinas`.
- Commands in project docs and examples assume they are run inside `fork_Asterinas` from `/root/asterinas`, unless explicitly marked as host commands.

### PR Branch Workflow

- Keep `dm` as the authority branch for implementing and directly validating Device Mapper behavior.
- Before preparing a new upstream PR, ensure the working tree is clean, fast-forward `main` to the current `upstream/main`, then create the PR branch directly from `main`.
- Name the branch after the concise PR title in kebab-case, such as `mapped-bio-ranges`; do not use generic names such as `pr` or `pr1`.
- Move only the verified minimal production diff from `dm` onto that branch. Run the final validation, commit, and push directly on the PR branch; do not create a separate PR worktree.

### Upstream Sync Workflow

- Sync entry point: fetch upstream, fast-forward local `main` to `upstream/main`
  (pure fast-forward, never conflicts), scout the delta, then rebase `dm`
  linearly. Never merge; confirm `git log --merges main..dm` is empty.
- Scout before spending build time: grade the upstream blast radius from the
  commit range. A docs/test-only delta may skip the baseline; changes to kernel
  core, the build system, dependency manifests, or the CI image require it.
- Run the baseline on the checked-out `main` inside the container: `make check`
  then `make kernel` (no full `make ktest`). A baseline failure is reported as
  an upstream/environment problem, never as a DM merge regression. Do not use
  nested worktrees for the baseline: workspace resolution walks into the main
  checkout's workspace root, and fresh checkout timestamps force a cargo-osdk
  reinstall.
- Create a safety ref (`dm-backup-YYYYMMDD`) and keep rerere enabled before the
  rebase; delete the ref after targeted verification passes.
- Conflict adjudication: local workflow facts (`AGENTS.md` blocks, `log/`) keep
  the dm side; lockfiles and dependency versions take the upstream side, then
  normalize with one online cargo run in the container as a separate sync
  commit; a dm fix overlapped by a new upstream implementation is skipped only
  after the fix's intent is confirmed to be covered, otherwise report first.
- Targeted verification after the rebase must cover new code, conflict
  resolutions, and semantically adjacent call chains (same file or caller even
  without a text conflict).
- Upstream container-image bumps are recorded during the sync; align the image
  and rerun targeted verification in it before creating a PR.
- Do not rebase already-pushed unmerged PR branches as part of the `dm` sync;
  handle them separately with their own backup refs and PR archives.

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

### 最终验证标准

- 每个改动的最终验收以 `make check`、`make kernel` 和相关定向 ktest 为基础；
  局部格式检查、静态检查或单个测试通过不能替代最终验收。
- 定向 ktest 应覆盖新增或修改的内核行为及受影响调用链。执行前说明改动与
  测试入口的对应关系；无现成测试或不适用的项目须明确记录覆盖缺口或原因。
- 涉及用户态 ABI 的改动还需运行相应 C 用户态 ABI 回归；涉及 dmsetup/LVM
  等完整用户空间流程时，按影响范围补充用户工具集成测试。
- 完整 `make ktest` 提供更广覆盖，但不默认每次执行；影响面难以限定或需要
  更广验收时，先与用户确认范围，再运行。
- 测试串行执行，记录源码快照、实际命令、匹配数量、结果和退出码；零匹配
  不算通过。区分用户反馈与独立日志核验，未执行或未完成的项目不得标记为通过。
- “内核内部测试（ktest）”“C 用户态 ABI 测试”“用户工具集成测试”按执行位置
  和验证边界区分；“回归”表示测试目的，不是另一种执行位置。

Run targeted ktests from the target Cargo crate directory. Use the module `::tests`
selector to run all tests in that module:

```bash
cd kernel/core/comps/device-mapper
cargo osdk test aster_device_mapper::table::tests

cd ../../
cargo osdk test --kcmd-args=earlycon \
  aster_core::device::misc::device_mapper::tests
```

Do not use root `make ktest CARGO_OSDK_TEST_ARGS="..."` as the default targeted
ktest entry. Run `cargo osdk test [module::tests]` from the target crate
directory. Core tests additionally require `--kcmd-args=earlycon` for observable
ktest output. Inspect QEMU logs from the repository root: the default x86_64
non-TDX `hvc0` path writes ktest UART output to `qemu-serial.log`.
`CONSOLE=ttyS0` is diagnostic-only and may be added when interactive serial
output is needed; it writes the UART stream to `qemu.log`.

Run QEMU, ktest, and NixOS system tests serially to avoid image lock conflicts,
especially around `test/initramfs/build/ext2.img`. For new NixOS system suite
runs, set `GUEST_READY_TIMEOUT=40` and `GUEST_QEMU_TIMEOUT=180`: the former
limits boot to the guest shell, while the latter limits the complete lifecycle
of each QEMU guest, including the shell-ready wait. The root Makefile defaults
to `RELEASE=1` (release).
DM system suites launch an existing NixOS root image via `make run_nixos`;
they do not rebuild or install the current kernel source. Release acceptance
requires an image built and installed from the intended source with an explicit
release configuration. Record the image's source and profile; setting
`RELEASE=1` only when launching a suite does not convert an existing dev image.
Dev-mode results do not replace release acceptance. If a
ktest/QEMU/NixOS run makes no relevant progress for about three minutes,
suspect command filtering, wrong crate working directory, root `make ktest`
argument override, missing KVM or initramfs arguments, leftover processes, or
image-lock issues; inspect output and processes, stop only processes started
for the current run, then retry from the target crate directory with
`cargo osdk test`. Add `CONSOLE=ttyS0` only while diagnosing missing or delayed
serial output.

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

## Upstream PR Preparation and Review

- Treat the authoritative `dm` worktree as the sole implementation source. Fix and
  validate every production-code issue there before preparing an upstream patch.
  A PR worktree may reorganize history and exclude unrelated features, but its
  production behavior must be a tested subset of the latest authoritative local
  implementation; it must never introduce a fix or semantic change that is absent
  from `dm`.
- Evaluate candidate designs and reviews for system-wide architectural fitness:
  API ownership, dependency direction, resource lifetime, concurrency, failure
  recovery, non-DM consumers, and expected future extensions. Do not choose a
  patch-local optimum that weakens correctness, cohesion, or long-term evolution
  merely to shrink or simplify one PR.
- If review evidence indicates an upstream-code bug, unsafe behavior, or semantic
  gap, report it to the user before changing local code. State the upstream
  evidence, concrete impact, whether it blocks the current work, and the available
  local responses. Do not silently repair, work around, weaken tests for, or
  classify an upstream issue as a local `dm` defect without user direction.

## Linux Device Mapper Semantic Alignment

For existing target coverage, prioritize closing semantic and lifecycle gaps with
standard upstream Linux Device Mapper over adding new targets. Do not treat
"the final I/O works" as sufficient compatibility evidence for `dmsetup`,
LVM2, or other user-visible Linux command behavior.

### Linux 参考版本与更新规则

- **主要设计与语义基线：本机 Linux 源码树。** 用户指定目录为
  `/root/Linux-tree-code`，实际 Git/源码根为 `/root/Linux-tree-code/kernel`。
  DM 迁移先依据该树的真实调用链、数据结构、状态转换及错误行为，不凭经验另行设计。
  该约定不扩展为整个 Asterinas 的兼容目标，也不承诺实现全部 Linux DM 功能。
- **每轮先固定证据快照：** 核实实际版本、commit、相关文件本地修改及仓库来源；
  区分提交中的实现和工作树注释/代码，不将本地修改当成上游事实。
  2026-10-15 初次核实：Makefile 为 6.6.0，标签 `6.6.0-132.0.0`，
  commit `5078a3a23a1e1825ec136485173ec98668cdd640`，origin 为
  `https://gitee.com/openeuler/kernel.git`。这是 openEuler Linux 内核树，
  不应称为未经修改的 kernel.org v6.6；当时 `drivers/md/dm-linear.c` 有本地
  中文学习注释与空白改动，另有未跟踪配置/构建文件。后续必须重新核实，不能沿用此快照假定。
- **其他固定版本仅作补充参考：** 原 `v6.18.56` 主基线调整为后续修复和 ABI
  差异参考；此前使用的 `v7.2.10` 同样不作为永久“最新稳定版”。引用需核实来源，
  只核对当前任务涉及的调用链。与本机基线冲突时列明证据、影响与建议，经用户
  审定再取舍，不静默换基线，也不照搬旧版已知缺陷。
- **主线仅作前瞻参考：** mainline/RC 不默认作为实现或验收要求；引用时固定
  commit，不以浮动 `master` 作为唯一证据。
- Rust 所有权、RAII 等是表达职责与约束的手段，不是改变 Linux 语义的默认理由；
  无需逐行翻译 C，但设计偏离须说明原因、影响和验证依据。框架限制须指明具体
  缺失依赖及责任边界，不笼统以“不支持 udev”等理由跳过。
- 按项展示 Linux 源码事实与当前 dm 源码事实，追踪相关调用链，并列出差异、
  影响、分类、原来/目标行为及示例、验证方式。细小的校验顺序、失败状态、
  引用/锁边界、事件和 I/O 完成语义也纳入审查；用户审定后再实现。
- Linux 内核版本与 DM ioctl ABI 版本分别记录，不混为一谈。选定参考基线
  不代表已通过兼容性验证；未核对或未测试事项必须明确标注，不将其他版本上的
  验证直接视为本基线通过。基线更新与扩大实现范围须另行讨论。

### Observable behavior and evidence

Before designing, implementing, or reviewing a Linux DM ABI or command-semantics
change, build a per-command observable-state matrix. Cover, when applicable:

- DM identity and table state;
- primary device node and mapper alias publication;
- VFS open behavior and block I/O behavior;
- block/VFS registry state, lifecycle events, and rollback boundaries;
- command output, status/event values, and failure behavior.

Use evidence in this order:

1. Read upstream Linux source first to establish the standard semantic baseline.
2. Compare the corresponding current Asterinas implementation against that baseline.
3. If upstream source cannot locate the relevant behavior or cannot explain a
   required user-space orchestration detail, inspect the external openEuler VM's
   installed source, libdevmapper, rules, or diagnostic information as auxiliary
   evidence only; it is not the semantic standard.
4. If both source investigations remain insufficient, run the smallest possible
   command sequence on the external openEuler VM and monitor the unresolved
   observable state, such as nodes, table/status, events, open, and I/O results.

Mark conclusions by evidence type: upstream source, Asterinas source, external
openEuler auxiliary evidence, or minimal external-VM experiment. Never infer a
user-visible node, alias, or command lifecycle solely from an internal kernel
object state.

Prioritize findings in a future user-visible command matrix as follows:

1. **High — observable failure or data-safety risk.** The semantic difference
   already causes command, open, mount, I/O, lifecycle, or data-integrity
   failures; fix it before treating the relevant path as aligned.
2. **Medium — observable Linux lifecycle mismatch.** Ordinary serial I/O may
   still succeed, but userspace can observe a different node, alias, table,
   status, event, or command-state transition than upstream Linux; schedule it
   as semantic-alignment work.
3. **Low — internal-only implementation difference.** The implementation order
   differs but creates no user-visible behavior difference and no concurrent
   safety risk; do not force source-level imitation.

A short execution window does not remove an observable semantic difference. For
example, publishing a mapper node and accepting open before its active table is
installed is high priority if a concurrent I/O can be refused; publishing the
same nodes at a different, but otherwise safe, command stage is medium priority.

Project logging rules:

- Before writing a dated log, run `date +%F` and write to `log/daily/YYYY-M-D.md`.
- Restart stage numbering from 1 each day.
- Log by small stage in this order: background, changes, tests.
- Do not split logs by “production code / ktest”.
- 每日日志增加“协作复盘与心得”：由 Claude 基于当天实际工作总结得到验证的
  判断、需要纠正的认识，以及下次可执行的改进；区分事实与反思，不重复工作
  清单、不虚构经验，没有新认识时简写，不为固定篇幅凑内容。
- Record actual engineering, learning, analysis/review, validation, failures, and
  conclusions. Do not present discussion or unexecuted plans as completed work.
- After every verified bug fix, append one factual row to the `修复日志` table
  at the end of `docs/global.md`. Record the issue and impact, root cause, fix,
  validation, and commit status; do not record unverified hypotheses.

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
