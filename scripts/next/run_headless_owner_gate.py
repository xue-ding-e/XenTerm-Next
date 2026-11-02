"""Immutable, headless-only validation; never builds the bridge or installers."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time


SUMMARY = re.compile(
    r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out"
)
CARGO = ["cargo", "test", "--locked", "-p", "xenterm", "--no-default-features", "--features", "headless", "--jobs", "2"]


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
        self.previous = {sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)}
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
                                       stderr=subprocess.STDOUT, text=True, start_new_session=True)
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
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                output, _ = process.communicate(timeout=10)
                (logs / f"{label}.log").write_text(output)
        finally:
            cancellation.cleaning = False
        raise
    (logs / f"{label}.log").write_text(output)
    print(output, end="" if output.endswith("\n") else "\n", flush=True)
    if process.returncode:
        raise RuntimeError(f"{label} failed: exit {process.returncode}")
    return output


def verify_source(source, manifest, logs, deadline=None):
    if len(manifest["files"]) != 8:
        raise RuntimeError("reviewed source scope must contain exactly eight files")
    def git(*args):
        return checked(["git", *args], cwd=source, logs=logs,
                       label="identity-" + args[0] + "-" + str(len(list(logs.iterdir()))), timeout=30, deadline=deadline).strip()
    verify_sha(manifest["source_sha"], git("rev-parse", "HEAD"))
    verify_sha(manifest["source_tree"], git("rev-parse", "HEAD^{tree}"))
    verify_sha(manifest["baseline_tree"], git("rev-parse", "HEAD^1^{tree}"))
    if len(git("show", "-s", "--format=%P", "HEAD").split()) != 1:
        raise RuntimeError("source commit must have one parent; merge commits are forbidden")
    changed = git("diff", "--name-only", "HEAD^1", "HEAD").splitlines()
    if sorted(changed) != sorted(manifest["files"]):
        raise RuntimeError("source commit contains changes outside the eight reviewed core files")
    for name, expected in manifest["files"].items():
        if not name.startswith("crates/xenterm/src/") or Path(name).is_absolute() or ".." in Path(name).parts:
            raise RuntimeError("manifest is not core-only")
        if hashlib.sha256((source / name).read_bytes()).hexdigest() != expected:
            raise RuntimeError(f"reviewed file changed: {name}")
    if git("status", "--porcelain"):
        raise RuntimeError("source checkout is dirty")
    print("FROZEN_SOURCE_VERIFIED " + manifest["source_sha"], flush=True)


def isolated_env(profile, source):
    allowed = ("PATH", "LD_LIBRARY_PATH", "LANG", "LC_ALL", "RUSTUP_TOOLCHAIN")
    env = {name: os.environ[name] for name in allowed if name in os.environ}
    home = Path.home()
    env["CARGO_HOME"] = os.environ.get("CARGO_HOME", str(home / ".cargo"))
    env["RUSTUP_HOME"] = os.environ.get("RUSTUP_HOME", str(home / ".rustup"))
    env["CARGO_TARGET_DIR"] = str(source / "target")
    for name in ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME",
                 "XDG_CACHE_HOME", "XDG_STATE_HOME", "XDG_RUNTIME_DIR", "TMP", "TEMP", "TMPDIR", "XENTERM_DATA_DIR"):
        path = profile / name
        path.mkdir(mode=0o700)
        env[name] = str(path)
    return env


def run():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--logs", type=Path, required=True)
    parser.add_argument("--verify-only", action="store_true")
    args = parser.parse_args()
    # Less than the outer 50-minute step, leaving explicit cleanup/artifact
    # margin. Every command consumes this same budget, not a fresh allowance.
    deadline = Deadline(45 * 60)
    source, logs = args.source.resolve(), args.logs.resolve()
    logs.mkdir(parents=True, exist_ok=True)
    manifest = json.loads(args.manifest.read_text())
    verify_source(source, manifest, logs, deadline)
    if args.verify_only:
        return
    run = lambda cmd, label, timeout, env=None: checked(cmd, cwd=source, logs=logs, label=label, timeout=timeout, env=env, deadline=deadline)
    run(["rustc", "--version", "--verbose"], "rustc", 30)
    run(["cargo", "--version"], "cargo", 30)
    run([*CARGO, "--no-run"], "compile-core", 1500)
    required = manifest["new_tests"] + manifest["maintenance_core_tests"]
    if len(manifest["new_tests"]) != 12 or len(manifest["maintenance_core_tests"]) != 16:
        raise RuntimeError("reviewed exact-test inventory changed")
    with tempfile.TemporaryDirectory(prefix="headless-owner-core-") as directory:
        env = isolated_env(Path(directory), source)
        inventory = run([*CARGO, "--offline", "--lib", "--", "--list"], "inventory", 120, env)
        verify_inventory(inventory, required)
        for index, name in enumerate(required):
            print("EXACT_TEST_START " + name, flush=True)
            output = run([*CARGO, "--offline", "--lib", name, "--", "--exact", "--test-threads=1", "--nocapture"],
                         f"exact-{index:02d}", 90, env)
            verify_exact(output, name in manifest["isolated_tests"])
        output = run([*CARGO, "--offline", "--", "--test-threads=1"], "full-core", 900, env)
        verify_full(output)
    verify_source(source, manifest, logs, deadline)
    CANCELLATION.checkpoint()
    print(f"HEADLESS_OWNER_CORE_PASS source={manifest['source_sha']} new=12 maintenance_core=16", flush=True)


if __name__ == "__main__":
    with CANCELLATION:
        run()
