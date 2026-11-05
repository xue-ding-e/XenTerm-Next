import argparse
import difflib
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time

MAX_OUTPUT = 8 * 1024 * 1024
TOTAL_SECONDS = 45 * 60


class ControlError(RuntimeError):
    pass


class Deadline:
    def __init__(self, seconds=TOTAL_SECONDS):
        self.end = time.monotonic() + seconds

    def limit(self, seconds):
        remaining = self.end - time.monotonic()
        if remaining <= 0:
            raise ControlError("global validation budget exhausted")
        return min(seconds, remaining)


class Cancellation:
    def __init__(self):
        self.signal = None

    def handle(self, signum, _frame):
        # Defer interruption until Popen has returned its owned PID.
        self.signal = signum

    def check(self):
        if self.signal is not None:
            raise ControlError("validation cancelled")


def live_members(pgid):
    if not Path("/proc/self/stat").is_file():
        raise ControlError("Linux process inventory unavailable")
    result = []
    for path in Path("/proc").glob("[0-9]*/stat"):
        try:
            fields = path.read_text().rsplit(")", 1)[1].split()
            if int(fields[2]) == pgid and fields[0] not in ("Z", "X"):
                result.append(path.parent.name)
        except (FileNotFoundError, ProcessLookupError):
            pass
    return result


def cleanup(process):
    # Also terminate descendants which closed stdout before their parent exited.
    # Zombie grandchildren belong to the runner reaper, not executable work.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait(timeout=10)
    end = time.monotonic() + 2
    while live_members(process.pid):
        if time.monotonic() >= end:
            raise ControlError("cleanup_unconfirmed: live owned process group")
        time.sleep(0.01)


def checked(command, *, cwd, env, logs, label, seconds, deadline,
            cancellation, allow_failure=False, max_output=MAX_OUTPUT):
    cancellation.check()
    end = time.monotonic() + deadline.limit(seconds)
    path = logs / (label + ".log")
    process = None
    with path.open("wb") as output:
        try:
            process = subprocess.Popen(command, cwd=cwd, env=env, stdout=output,
                                       stderr=subprocess.STDOUT, start_new_session=True)
            while True:
                cancellation.check()
                if path.stat().st_size > max_output:
                    raise ControlError(label + ": output limit exceeded")
                if time.monotonic() >= end:
                    raise ControlError(label + ": stage timeout")
                if process.poll() is not None:
                    break
                time.sleep(0.05)
        finally:
            if process is not None:
                try:
                    cleanup(process)
                except BaseException as error:
                    raise ControlError("cleanup_unconfirmed: " + label) from error
            # Keep evidence bounded even when a command emits a burst between
            # polling intervals. An overflow is still a failed control stage.
            if path.stat().st_size > max_output:
                output.truncate(max_output)
                raise ControlError(label + ": output limit exceeded")
    cancellation.check()
    if path.stat().st_size > max_output:
        raise ControlError(label + ": output limit exceeded")
    result = path.read_text(encoding="utf-8", errors="strict")
    print(label + ": exit " + str(process.returncode), flush=True)
    if process.returncode and not allow_failure:
        raise ControlError(label + ": nonzero exit; see bounded log")
    return process.returncode, result


def digest(data):
    return hashlib.sha256(data).hexdigest()


def verify_hashes(root, expected):
    for name, sha in expected.items():
        if digest((root / name).read_bytes()) != sha:
            raise ControlError("frozen bytes mismatch: " + name)



PRODUCT_PATHS = frozenset(['apps/xenterm/lib/bridge/native_bridge.dart', 'apps/xenterm/lib/l10n/zh_hosts.dart', 'apps/xenterm/lib/screens/hosts/hosts_screen.dart', 'apps/xenterm/native/src/lib.rs', 'apps/xenterm/native/tests/groups.rs', 'apps/xenterm/test/group_order_actions_test.dart', 'apps/xenterm/test/group_order_display_test.dart', 'crates/xenterm/src/config/impls/config.rs', 'crates/xenterm/src/config/impls/session_group_order_tests.rs', 'crates/xenterm/src/config/struct/config_file.rs'])
KNOWN_IGNORED_RUST = frozenset(['bench_terminal_pipeline_profiles', 'bench_terminal_pipeline_resize', 'bench_terminal_pipeline_history_memory', 'bench_terminal_find_over_full_scrollback', 'bench_terminal_render_frame'])
