#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Own the DM suite's disposable disks and its single summary log.

The suite process creates the backing disks, keeps one run directory under
``/tmp/dm-test/dm-control-plane``, and removes both again before it exits, so a
disk is visible only while the suite runs. ``summary.log`` is the only log: it
holds host milestones, guest assertion records, and startup/build diagnostics
without retaining the full console or uploaded command echoes.
"""

import datetime
import os
from pathlib import Path
import re
import selectors
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import traceback


SUITE_DIR = Path(__file__).resolve().parent
ROOT = SUITE_DIR.parents[3]
KVM = Path("/dev/kvm")
# Log wall-clock times use China Standard Time regardless of the container TZ.
CST = datetime.timezone(datetime.timedelta(hours=8), name="CST")

# Framework records that belong in the terminal view.
FRAMEWORK_VIEW = re.compile(r"^(?:=== |✓ |✗ |Passed:|Failed|  - )")

# The framework dumps command output between these two banners on failure.
ERROR_BLOCK = re.compile(r"^={3,}$")

# Guest records kept in the summary; the echoed commands stay out of the log.
GUEST_RECORD = re.compile(
    rb"^(?:STATUS_|DURATION_|STEP_DURATION_|GUEST_DURATION_|SUMMARY_GAP_|"
    rb"CHECK_PASS_|OBSERVE_GAP_|CASE_PASS_|CASE_FAIL_|TEST_FAIL_|EVENT_|NODE_)"
)

# QEMU reports every CPU feature the host cannot emulate; it is noise here.
QEMU_NOISE = re.compile(rb"host doesn't support requested feature")


def stamp():
    return datetime.datetime.now(CST).strftime("%H:%M:%S")


class RunLog:
    """The single append-only log shared by both processes.

    The suite process and the guest supervisor append here, so every write is
    flushed immediately and line-oriented writes start on a line boundary even
    when the other writer left a partial line behind.
    """

    def __init__(self, path):
        self.handle = path.open("a+b")

    def close(self):
        self.handle.close()

    def at_line_start(self):
        size = os.fstat(self.handle.fileno()).st_size
        return size == 0 or os.pread(self.handle.fileno(), 1, size - 1) == b"\n"

    def write_line(self, line):
        if not self.at_line_start():
            line = b"\n" + line
        self.handle.write(line + b"\n")
        self.handle.flush()


def record(log, message):
    """Append a milestone to the summary log and return the formatted line."""
    line = f"[{stamp()}] DM-TEST {message}"
    log.write_line(line.encode())
    return line


def announce(log, message):
    """Milestones raised by the suite process also reach the terminal."""
    print(record(log, message), flush=True)


class Terminal:
    """Reduce the driver's stream to the milestones the shell suite showed.

    The framework announces every command it sends, including each chunk of the
    guest-script upload, and it mixes that with the guest console it reads back.
    The terminal keeps only per-case progress and the framework's own error
    blocks.
    """

    def __init__(self):
        self.in_error_block = False

    def feed(self, line):
        if not line.strip():
            return
        if self.in_error_block:
            print(line, flush=True)
            self.in_error_block = not ERROR_BLOCK.match(line)
            return
        if line.endswith("Error Details ===") or line == "=== Unexpected Output ===":
            self.in_error_block = True
            print(line, flush=True)
            return
        if FRAMEWORK_VIEW.match(line) or line == "--> Waiting for login prompt...":
            print(line, flush=True)

    def flush(self, partial):
        """Keep a final line the framework wrote without a trailing newline."""
        if partial.strip():
            print(partial, flush=True)


class BootProgress:
    """Relay boot milestones from the summary without touching the guest PTY.

    The supervisor cannot print directly to the terminal: its stdout belongs to
    the framework's guest session. Only complete BOOT records are relayed here;
    the suite's own announcements are already printed and must not be repeated.
    """

    def __init__(self, source):
        self.source = source
        self.partial = b""

    def drain(self):
        self.partial += self.source.read()
        while b"\n" in self.partial:
            line, self.partial = self.partial.split(b"\n", 1)
            if re.match(rb"^\[\d{2}:\d{2}:\d{2}\] DM-TEST BOOT ", line):
                print(line.decode(errors="replace"), flush=True)


class GuestStream:
    """Keep the guest's assertion records and drop the console noise around them."""

    def __init__(self, log):
        self.log = log
        self.partial = b""

    def feed(self, data):
        self.partial += data
        while b"\n" in self.partial:
            line, self.partial = self.partial.split(b"\n", 1)
            self.emit(line)
        if len(self.partial) > (1 << 20):
            self.partial = b""

    def close(self):
        if self.partial:
            self.emit(self.partial)
            self.partial = b""

    def emit(self, line):
        line = line.replace(b"\r", b"").strip()
        if GUEST_RECORD.match(line):
            self.log.write_line(line)


class QemuStderr:
    """Keep QEMU's own errors; the per-feature CPU warnings are noise."""

    def __init__(self, log):
        self.log = log
        self.partial = b""

    def feed(self, data):
        self.partial += data
        while b"\n" in self.partial:
            line, self.partial = self.partial.split(b"\n", 1)
            self.emit(line)
        if len(self.partial) > (1 << 16):
            self.emit(self.partial)
            self.partial = b""

    def close(self):
        if self.partial:
            self.emit(self.partial)
            self.partial = b""

    def emit(self, line):
        line = line.rstrip(b"\r")
        if line.strip() and not QEMU_NOISE.search(line):
            self.log.write_line(b"[qemu] " + line)


def stop_group(pid):
    """Stop only the process group created for the current invocation."""
    try:
        os.killpg(pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        try:
            os.killpg(pid, 0)
        except ProcessLookupError:
            return
        time.sleep(0.05)
    try:
        os.killpg(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


def interrupted(signum, _frame):
    raise SystemExit(128 + signum)


def supervise_guest():
    """Run one QEMU boot as the official framework's PTY child."""
    directory = Path(os.environ["DM_LOG_RUN_DIR"])
    runtime = Path(os.environ["DM_CONTROL_RUNTIME"])
    ready_timeout = int(os.environ.get("GUEST_READY_TIMEOUT", "40"))
    lifecycle_timeout = int(os.environ.get("GUEST_QEMU_TIMEOUT", "180"))
    if ready_timeout <= 0 or lifecycle_timeout <= 0:
        raise ValueError("guest timeouts must be positive seconds")

    # The original suite labels a single boot `guest`; extra boots are numbered.
    count_path = runtime / "boots"
    boot = int(count_path.read_text()) + 1 if count_path.exists() else 1
    count_path.write_text(str(boot))
    label = "guest" if boot == 1 else f"guest{boot}"

    started = time.monotonic()
    child = None
    status = 1
    log = RunLog(directory / "summary.log")
    try:
        record(log, f"BOOT {label} starting")
        guest = GuestStream(log)
        stderr = QemuStderr(log)
        child = subprocess.Popen(
            ["bash", str(ROOT / "tools/nixos/run.sh"), "nixos"],
            cwd=ROOT,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        (runtime / "qemu.pid").write_text(str(child.pid))

        ready = False
        pending = b""
        timed_out = False
        # Only QEMU's own streams end the boot. The framework closes this
        # process's PTY after it has reaped us, so waiting on stdin EOF too
        # would deadlock until the lifecycle deadline.
        open_outputs = 2
        with selectors.DefaultSelector() as selector:
            selector.register(sys.stdin.fileno(), selectors.EVENT_READ)
            selector.register(child.stdout, selectors.EVENT_READ)
            selector.register(child.stderr, selectors.EVENT_READ)
            while open_outputs:
                elapsed = time.monotonic() - started
                if elapsed >= lifecycle_timeout or (
                    not ready and elapsed >= ready_timeout
                ):
                    phase = "execution_or_shutdown" if ready else "ready"
                    record(
                        log,
                        f"HOST_FAIL_DM_CONTROL_PLANE {label}_timeout phase={phase}",
                    )
                    timed_out = True
                    break
                for key, _ in selector.select(0.1):
                    data = os.read(key.fd, 65536)
                    if not data:
                        selector.unregister(key.fileobj)
                        if key.fileobj is child.stdout or key.fileobj is child.stderr:
                            open_outputs -= 1
                        continue
                    if key.fileobj is child.stdout:
                        guest.feed(data)
                        view = memoryview(data)
                        while view:
                            view = view[os.write(sys.stdout.fileno(), view) :]
                        if not ready:
                            pending = (pending + data)[-4096:]
                            if b"root@asterinas:" in pending:
                                ready = True
                                record(
                                    log,
                                    f"BOOT {label} shell ready after={elapsed:.1f}s",
                                )
                    elif key.fileobj is child.stderr:
                        stderr.feed(data)
                    else:
                        child.stdin.write(data)
                        child.stdin.flush()

        if timed_out:
            # The deadline already fired; do not wait on a wedged guest.
            stop_group(child.pid)
            child.wait()
            status = 124
        else:
            status = child.wait()
        guest.close()
        stderr.close()
        return status
    except Exception:
        # The PTY error may be discarded by the driver during startup. Preserve
        # the original exception before cleanup changes the child's exit status.
        record(log, f"HOST_FAIL_DM_CONTROL_PLANE {label}_supervisor_exception")
        for line in traceback.format_exc().splitlines():
            log.write_line(line.encode())
        raise
    finally:
        if child is not None:
            if child.poll() is None:
                stop_group(child.pid)
                child.wait()
            (runtime / "qemu.pid").unlink(missing_ok=True)
            record(
                log,
                f"BOOT {label} finished status={child.returncode} "
                f"elapsed={time.monotonic() - started:.0f}s",
            )
        log.close()


def run_suite(command):
    """Create the run directory and disks, drive the suite, then clean both up."""
    if not command:
        raise ValueError("missing official test driver command")

    parent = Path(os.environ.get("DM_TEST_LOG_ROOT", "/tmp/dm-test")) / "dm-control-plane"
    parent.mkdir(parents=True, exist_ok=True)
    base = datetime.datetime.now(CST).strftime("%Y-%m-%d-%H-%M")
    directory = parent / base
    suffix = 0
    while True:
        try:
            directory.mkdir()
            break
        except FileExistsError:
            suffix += 1
            directory = parent / f"{base}-{suffix:02d}"
    latest = parent / "latest"
    if latest.is_symlink():
        latest.unlink()
    elif latest.exists():
        shutil.rmtree(latest)
    latest.symlink_to(directory.resolve(), target_is_directory=True)

    runtime = None
    child = None
    status = 1
    log = RunLog(directory / "summary.log")
    try:
        if subprocess.run(
            ["pgrep", "-f", "[q]emu-system"], stdout=subprocess.DEVNULL
        ).returncode == 0:
            raise RuntimeError("another QEMU is running; run system tests serially")
        if not (ROOT / "target/nixos/asterinas.img").is_file():
            raise RuntimeError(
                "missing target/nixos/asterinas.img; build the image before testing"
            )

        # Disks live under target/nixos only for the duration of this run.
        runtime = Path(
            tempfile.mkdtemp(prefix="dm-control-plane.", dir=ROOT / "target/nixos")
        )
        disks = [runtime / "disk1.img", runtime / "disk2.img"]
        for index, disk in enumerate(disks, 1):
            subprocess.run(["fallocate", "-l", "512M", str(disk)], check=True)
            announce(log, f"测试盘{index}：{disk}，容量512MiB")
        announce(log, f"suite started log_dir={directory}")
        announce(
            log,
            f"TIMEOUT shell_ready={os.environ.get('GUEST_READY_TIMEOUT', '40')}s "
            f"lifecycle={os.environ.get('GUEST_QEMU_TIMEOUT', '180')}s",
        )
        if not KVM.exists():
            announce(log, "HOST_INFO_DM_CONTROL_PLANE kvm=absent tcg_fallback")

        env = os.environ.copy()
        env.update(
            DM_CONTROL_RUNTIME=str(runtime),
            DM_LOG_RUN_DIR=str(directory.resolve()),
            DM_TEST_IMAGES=" ".join(map(str, disks)),
        )
        child = subprocess.Popen(
            command,
            cwd=SUITE_DIR,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        with selectors.DefaultSelector() as selector, (
            directory / "summary.log"
        ).open("rb") as summary:
            progress = BootProgress(summary)
            selector.register(child.stdout, selectors.EVENT_READ)
            partial = b""
            terminal = Terminal()
            while selector.get_map():
                events = selector.select(0.1)
                progress.drain()
                for key, _ in events:
                    data = os.read(key.fd, 65536)
                    if not data:
                        selector.unregister(key.fileobj)
                        continue
                    partial += data
                    while b"\n" in partial:
                        line, partial = partial.split(b"\n", 1)
                        forward(line, log, terminal)
            if partial:
                log_framework_diagnostic(partial, log, terminal)
                terminal.flush(partial.decode(errors="replace").rstrip("\r"))
            status = child.wait()
            progress.drain()
        return status
    finally:
        if child is not None and child.poll() is None:
            stop_group(child.pid)
            child.wait()
        if runtime is not None:
            pid_path = runtime / "qemu.pid"
            if pid_path.exists():
                stop_group(int(pid_path.read_text()))
            # Only this run's freshly created directory is removed.
            for filename in ("disk1.img", "disk2.img", "qemu.pid", "boots"):
                (runtime / filename).unlink(missing_ok=True)
            runtime.rmdir()
        outcome = "HOST_PASS" if status == 0 else "HOST_FAIL"
        announce(log, f"{outcome}_DM_CONTROL_PLANE status={status} disks_cleaned")
        if status != 0:
            print(f"诊断信息：{directory / 'summary.log'}", flush=True)
        log.close()


def log_framework_diagnostic(line, log, terminal):
    """Keep driver/build diagnostics without duplicating the normal outline.

    Command echoes include the uploaded script and are not diagnostics. Error
    block contents must be retained even when they resemble progress records.
    """
    text = line.decode(errors="replace").rstrip("\r")
    if text.strip() and (
        terminal.in_error_block
        or (not text.startswith("--> Running:") and not FRAMEWORK_VIEW.match(text))
        or text.endswith("Error Details ===")
        or text == "=== Unexpected Output ==="
    ):
        log.write_line(line.rstrip(b"\r"))


def forward(line, log, terminal):
    """Retain diagnostics in the summary while keeping the terminal outline."""
    log_framework_diagnostic(line, log, terminal)
    terminal.feed(line.decode(errors="replace").rstrip("\r"))


def main():
    for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(sig, interrupted)
    if sys.argv[1:] == ["--guest"]:
        return supervise_guest()
    return run_suite(sys.argv[1:])


if __name__ == "__main__":
    sys.exit(main())