"""Verify the fixed formatted source and its Windows owner/core regressions."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time


SUMMARY = re.compile(
    r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out"
)
CARGO = ["cargo", "test", "--locked", "-p", "xenterm", "--jobs", "2"]
LIB_CARGO = [*CARGO, "--lib"]


class Cancelled(RuntimeError):
    pass


class Cancellation:
    def __init__(self):
        self.launching = False
        self.cleaning = False
        self.pending = None

    def handle(self, signum, _frame):
        self.pending = signum
        # Never raise from an asynchronous signal handler. Popen can have
        # created the child before returning its handle, and a repeated signal
        # could otherwise interrupt the cleanup exception handler itself.
        # communicate polls explicit safe checkpoints at most 0.5s apart.

    def checkpoint(self):
        if self.pending is not None:
            raise Cancelled(f"validation cancelled by signal {self.pending}")

    def __enter__(self):
        self.pending = None
        self.previous = {sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM, *([signal.SIGBREAK] if hasattr(signal, "SIGBREAK") else []))}
        for sig in self.previous:
            signal.signal(sig, self.handle)
        return self

    def __exit__(self, exc_type, *_):
        for sig, handler in self.previous.items():
            signal.signal(sig, handler)
        if exc_type is None:
            self.checkpoint()


class Deadline:
    def __init__(self, seconds, clock=time.monotonic):
        self.clock = clock
        self.end = clock() + seconds

    def limit(self, stage_seconds):
        remaining = self.end - self.clock()
        if remaining <= 0:
            raise RuntimeError("global validation budget exhausted")
        return min(stage_seconds, remaining)


CANCELLATION = Cancellation()


def verify_sha(expected, actual):
    if not re.fullmatch(r"[0-9a-f]{40}", expected) or expected != actual.strip():
        raise RuntimeError(f"unbound or mismatched source: {expected} != {actual.strip()}")


def verify_inventory(output, names):
    listed = re.findall(r"^([A-Za-z0-9_:]+): test$", output, re.MULTILINE)
    if len(names) != len(set(names)) or not set(names).issubset(listed):
        raise RuntimeError("duplicate or undiscovered required tests")


def verify_exact(output, isolated):
    summaries = SUMMARY.findall(output)
    # Isolated owner fixtures execute one child test plus one parent test. The
    # child completion marker is independently asserted by the parent fixture.
    expected = 2 if isolated else 1
    if len(summaries) != expected or any(row[:4] != ("1", "0", "0", "0") for row in summaries):
        raise RuntimeError("expected exact, non-ignored parent/child test passes")


def verify_full(output):
    summaries = SUMMARY.findall(output)
    if not summaries or not any(int(row[0]) for row in summaries) or any(row[1] != "0" for row in summaries):
        raise RuntimeError("full core suite produced no passing test evidence")
    print("FULL_CORE_SUMMARIES " + json.dumps(summaries), flush=True)


def kill_tree(process, platform=None):
    platform = platform or os.name
    if platform == "nt":
        # Only this owned cargo/git/rustfmt PID and its descendants. Do not use
        # image-name kills, and do not create a detached Windows session.
        tool = str(Path(os.environ.get("SYSTEMROOT", r"C:\Windows")) / "System32" / "taskkill.exe")
        result = subprocess.run([tool, "/PID", str(process.pid), "/T", "/F"],
                                capture_output=True, text=True, timeout=10)
        # An exited parent cannot prove that its descendants also exited.
        # Any failed tree termination remains unconfirmed, even after poll().
        if result.returncode:
            raise RuntimeError("cleanup_unconfirmed: taskkill could not stop the owned process tree")
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass


def checked(command, *, cwd, logs, label, timeout, env=None, deadline=None, cancellation=None):
    cancellation = cancellation or CANCELLATION
    cancellation.checkpoint()
    allowance = deadline.limit(timeout) if deadline is not None else timeout
    stage_end = time.monotonic() + allowance
    print("COMMAND " + json.dumps(command), flush=True)
    process = None
    try:
        cancellation.launching = True
        try:
            process = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE,
                                       stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace",
                                       start_new_session=os.name != "nt")
        finally:
            cancellation.launching = False
        cancellation.checkpoint()
        while True:
            cancellation.checkpoint()
            remaining = stage_end - time.monotonic()
            if remaining <= 0:
                raise subprocess.TimeoutExpired(command, allowance)
            try:
                output, _ = process.communicate(timeout=min(0.5, remaining))
                break
            except subprocess.TimeoutExpired:
                # communicate is retry-safe: it retains the partial output.
                # Recheck both cancellation and the shared deadline next poll.
                pass
        cancellation.checkpoint()
    except BaseException:
        cancellation.cleaning = True
        try:
            if process is not None:
                # Include cargo's test subprocesses and each fixture child,
                # even on runner SIGINT/SIGTERM, not only TimeoutExpired.
                cleanup_error = None
                try:
                    kill_tree(process)
                except Exception as error:
                    cleanup_error = error
                    # Always reap the directly owned process even if Windows
                    # refuses tree termination; never turn that into a clean
                    # ACK for its possibly surviving descendants.
                    try:
                        process.kill()
                    except ProcessLookupError:
                        pass
                output, _ = process.communicate(timeout=10)
                (logs / f"{label}.log").write_text(output, encoding="utf-8")
                if cleanup_error is not None:
                    raise RuntimeError("cleanup_unconfirmed: process-tree termination failed") from cleanup_error
        finally:
            cancellation.cleaning = False
        raise
    (logs / f"{label}.log").write_text(output, encoding="utf-8")
    print(output, end="" if output.endswith("\n") else "\n", flush=True)
    if process.returncode:
        raise RuntimeError(f"{label} failed: exit {process.returncode}")
    return output


def verify_source(source, manifest, logs, deadline):
    if len(manifest["files"]) != 8 or len(manifest["changed_files"]) != 7:
        raise RuntimeError("formatted source scope must be eight reviewed files, seven changed")

    def git(*args):
        return checked(["git", *args], cwd=source, logs=logs,
                       label="identity-" + str(len(list(logs.iterdir()))), timeout=30, deadline=deadline).strip()

    verify_sha(manifest["source_sha"], git("rev-parse", "HEAD"))
    verify_sha(manifest["source_tree"], git("rev-parse", "HEAD^{tree}"))
    verify_sha(manifest["parent_sha"], git("rev-parse", "HEAD^1"))
    verify_sha(manifest["parent_tree"], git("rev-parse", "HEAD^1^{tree}"))
    verify_sha(manifest["grandparent_sha"], git("rev-parse", "HEAD^1^1"))
    if git("show", "-s", "--format=%P", "HEAD").split() != [manifest["parent_sha"]]:
        raise RuntimeError("format fixup must retain exactly its reviewed source parent")
    if sorted(git("diff", "--name-only", "HEAD^1", "HEAD").splitlines()) != sorted(manifest["changed_files"]):
        raise RuntimeError("format fixup changed unreviewed files")
    for name, expected in manifest["files"].items():
        if not name.startswith("crates/xenterm/src/") or Path(name).is_absolute() or ".." in Path(name).parts:
            raise RuntimeError("source manifest is not core-only")
        if hashlib.sha256((source / name).read_bytes()).hexdigest() != expected:
            raise RuntimeError(f"reviewed formatted file changed: {name}")
    if git("status", "--porcelain"):
        raise RuntimeError("source checkout is dirty")
    print("WINDOWS_OWNER_SOURCE_VERIFIED " + manifest["source_sha"], flush=True)


def run():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--logs", type=Path, required=True)
    parser.add_argument("--mode", choices=("verify", "format", "core"), required=True)
    args = parser.parse_args()
    # Core is a 25-minute outer step; leave five minutes for cancellation and
    # evidence. Format and identity phases have much smaller total budgets.
    deadline = Deadline(20 * 60 if args.mode == "core" else 120)
    source, logs = args.source.resolve(), args.logs.resolve()
    logs.mkdir(parents=True, exist_ok=True)
    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    verify_source(source, manifest, logs, deadline)

    def invoke(command, label, timeout):
        return checked(command, cwd=source, logs=logs, label=label, timeout=timeout, deadline=deadline)

    if args.mode == "format":
        version = invoke(["rustfmt", "--version"], "rustfmt-version", 20)
        if "1.10.0-stable" not in version or "58cf8f9c35" not in version:
            raise RuntimeError("unexpected formatter; require the reviewed Rust 1.99.1 toolchain")
        invoke(["rustfmt", "--check", "--edition", "2021", "--config", "skip_children=true", *manifest["files"]],
               "format-eight-files", 60)
        print("WINDOWS_OWNER_FORMAT_PASS files=8 source=" + manifest["source_sha"], flush=True)
    elif args.mode == "core":
        version = invoke(["rustc", "--version"], "rustc-version", 20)
        if not version.startswith("rustc 1.99.1 "):
            raise RuntimeError("unexpected Rust toolchain")
        invoke([*CARGO, "--no-run"], "compile-core", 900)
        required = manifest["new_tests"]
        if len(required) != 12 or len(manifest["isolated_tests"]) != 6:
            raise RuntimeError("reviewed owner inventory changed")
        verify_inventory(invoke([*LIB_CARGO, "--offline", "--", "--list"], "owner-inventory", 60), required)
        for index, name in enumerate(required):
            print("WINDOWS_OWNER_EXACT_START " + name, flush=True)
            output = invoke([*LIB_CARGO, "--offline", name, "--", "--exact", "--test-threads=1", "--nocapture"],
                            f"owner-exact-{index:02d}", 90)
            verify_exact(output, name in manifest["isolated_tests"])
        verify_full(invoke([*CARGO, "--offline", "--", "--test-threads=1"], "full-default-core", 600))
        print("WINDOWS_OWNER_CORE_PASS new=12 source=" + manifest["source_sha"], flush=True)
    verify_source(source, manifest, logs, deadline)
    CANCELLATION.checkpoint()


if __name__ == "__main__":
    with CANCELLATION:
        run()
