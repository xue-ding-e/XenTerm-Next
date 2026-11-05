"""Pinned, synthetic Flutter SFTP action state validation. No native product build."""
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

SOURCE = "c0d371e4730b83a81ece806cbb42d216eb9d874d"
SCREEN = "apps/xenterm/lib/screens/terminal/sftp_view.dart"
TEST = "apps/xenterm/test/sftp_test.dart"
FILES = {'apps/xenterm/lib/screens/terminal/sftp_view.dart': '3bdd5b84c79b6396f7fb5a21e151f46a766894143f1a91b1d6db09ed8eb79a6a', 'apps/xenterm/test/sftp_test.dart': 'c070e4cc344a612a0ec6cd35a64243c1d82c1a5e4c2a898959b46f2857da5aff'}
BASE = {'apps/xenterm/lib/screens/terminal/sftp_view.dart': '589fc6b96e3130eeb50237df654c1b2e517bf168267ee64c6647b676a5515f79', 'apps/xenterm/test/sftp_test.dart': '1cfbd7caa7b480351bf8405f624d3760cde7e622def4b93dadd427264f9ba9a4'}
PATCHES = {'sftp-action-state.patch': 'f365145ce571e9c7de8274f7b47b09070ad866a1392c01fa8eff34490624ef0b', 'sftp-action-state-red-control.patch': '15b83695204cf2b4981bf376fabbdfedce7e711e69c02ab393955b6a6b6027e8'}
LATE = 'directory change clears the previous file selection'
NEW_TESTS = ['directory change clears the previous file selection', 'directory change cancels a pending delete confirmation', 'directory change cancels a new folder draft', 'same directory refresh keeps selection with fresh metadata', 'same directory removed clears an outdated delete confirmation', 'same directory type clears an outdated delete confirmation', 'same directory name clears an outdated delete confirmation', 'same directory size clears an outdated delete confirmation', 'same directory modified clears an outdated delete confirmation', 'same directory mode clears an outdated delete confirmation', 'unchanged entry refresh preserves its delete confirmation', 'entering a folder clears its parent listing selection', 'same directory refresh preserves a new folder draft']
ALL_TESTS = ['directory change clears the previous file selection', 'directory change cancels a pending delete confirmation', 'directory change cancels a new folder draft', 'same directory refresh keeps selection with fresh metadata', 'same directory removed clears an outdated delete confirmation', 'same directory type clears an outdated delete confirmation', 'same directory name clears an outdated delete confirmation', 'same directory size clears an outdated delete confirmation', 'same directory modified clears an outdated delete confirmation', 'same directory mode clears an outdated delete confirmation', 'unchanged entry refresh preserves its delete confirmation', 'entering a folder clears its parent listing selection', 'same directory refresh preserves a new folder draft', 'the browser lists a real snapshot and navigates on click', 'selection drives download and delete commands', 'new folder creates through the callback', 'the folder tree expands and opens directories', 'errors and empty folders are explicit']
FAILURE = "navigation kept a download action for the previous directory"
MAX_OUTPUT = 8 * 1024 * 1024
TOTAL_SECONDS = 18 * 60


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


def verify_machine(output, code, expected, *, red=False):
    try:
        events = []
        for line in output.splitlines():
            if not line.strip():
                continue
            event = json.loads(line)
            # Flutter 3.47.7 emits this one auxiliary process-start envelope
            # alongside package:test JSON. Other list/scalar shapes fail closed.
            if event == [{"event": "test.startedProcess", "params": {"vmServiceUri": None}}]:
                continue
            if not isinstance(event, dict):
                raise ControlError("unreviewed machine event shape")
            events.append(event)
    except (ValueError, TypeError) as error:
        raise ControlError("test output is not valid machine JSON") from error
    starts = {event["test"]["id"]: event["test"]["name"]
              for event in events if event.get("type") == "testStart"}
    finishes = [event for event in events
                if event.get("type") == "testDone" and not event.get("hidden", False)]
    names = [starts.get(event.get("testID")) for event in finishes]
    done = [event for event in events if event.get("type") == "done"]
    if sorted(names) != sorted(expected) or len(names) != len(set(names)):
        raise ControlError("missing, duplicate or unexpected executed tests")
    if len(done) != 1 or any(event.get("skipped") is not False for event in finishes):
        raise ControlError("missing final result or skipped test")
    errors = [event for event in events if event.get("type") == "error"]
    if red:
        if expected != [LATE] or code != 1 or done[0].get("success") is not False:
            raise ControlError("red control must fail exactly the stale-selection test")
        target = finishes[0]["testID"]
        if finishes[0].get("result") not in ("error", "failure"):
            raise ControlError("red control did not report a test failure")
        if len(errors) != 1 or errors[0].get("testID") != target:
            raise ControlError("red must report one same-test failure")
        error = errors[0]
        direct = (FAILURE in error.get("error", "")
                  and "Found 1 widget with icon" in error.get("error", "")
                  and "sftp_test.dart" in error.get("stackTrace", ""))
        blocks = [event for event in events
                  if event.get("type") == "print"
                  and "EXCEPTION CAUGHT BY FLUTTER TEST FRAMEWORK" in event.get("message", "")]
        generic = "Test failed. See exception logs above.\nThe test description was: " + LATE
        printed = (len(blocks) == 1 and blocks[0].get("testID") == target
                   and blocks[0].get("messageType") == "print"
                   and FAILURE in blocks[0].get("message", "")
                   and "sftp_test.dart" in blocks[0].get("message", "")
                   and 'Found 1 widget with icon' in blocks[0].get("message", "")
                   and error.get("error") == generic
                   and error.get("stackTrace") == "")
        if not direct and not printed:
            raise ControlError("red failed for an unreviewed reason")
    elif code != 0 or done[0].get("success") is not True or errors or any(
            event.get("result") != "success" for event in finishes):
        raise ControlError("green test suite did not pass completely")


def isolated_env(root):
    env = {key: os.environ[key] for key in ("PATH", "LANG", "LC_ALL") if key in os.environ}
    for key in ("HOME", "TMPDIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XENTERM_DATA_DIR", "PUB_CACHE"):
        path = root / key
        path.mkdir(mode=0o700)
        env[key] = str(path)
    env.update(PUB_HOSTED_URL="https://pub.flutter-io.cn", CI="true",
               FLUTTER_SUPPRESS_ANALYTICS="true", DART_SUPPRESS_ANALYTICS="true")
    return env


def run(args):
    source, logs = args.source.resolve(), args.logs.resolve()
    logs.mkdir(parents=True, exist_ok=True)
    controls = Path(__file__).resolve().parent
    verify_hashes(controls, PATCHES)
    deadline, cancel = Deadline(), Cancellation()
    previous = {sig: signal.signal(sig, cancel.handle) for sig in (signal.SIGINT, signal.SIGTERM)}
    try:
        with tempfile.TemporaryDirectory(prefix="sftp-action-state-") as directory:
            env = isolated_env(Path(directory))
            def call(command, label, seconds=60, cwd=source, allow_failure=False):
                return checked(command, cwd=cwd, env=env, logs=logs, label=label,
                               seconds=seconds, deadline=deadline, cancellation=cancel,
                               allow_failure=allow_failure)
            def git(*arguments, label):
                return call(["git", *arguments], label)[1].strip()
            if git("rev-parse", "HEAD", label="source-sha") != SOURCE:
                raise ControlError("wrong source commit")
            if git("status", "--porcelain", label="clean-source"):
                raise ControlError("source must start clean")
            verify_hashes(source, BASE)
            lock_path = source / "apps/xenterm/pubspec.lock"
            lock_hash = digest(lock_path.read_bytes())
            if lock_hash != "2bf7221d4be4a58e6b41df5f28a00b7e9f3821a514c671a2dda278f420bc1a23":
                raise ControlError("unreviewed pubspec.lock")
            base_screen = (source / SCREEN).read_bytes()
            git("apply", "--check", str(controls / "sftp-action-state.patch"), label="check-main-patch")
            git("apply", str(controls / "sftp-action-state.patch"), label="apply-main-patch")
            verify_hashes(source, FILES)
            raw_green = {name: (source / name).read_bytes() for name in FILES}
            # RED restores the exact official screen bytes while retaining
            # the new tests. No assertion or loader is changed for the control.
            git("apply", "--check", str(controls / "sftp-action-state-red-control.patch"), label="check-red-patch")
            git("apply", str(controls / "sftp-action-state-red-control.patch"), label="bind-red-patch")
            if (source / SCREEN).read_bytes() != base_screen:
                raise ControlError("red patch does not restore the exact official screen")
            for name, data in raw_green.items():
                (source / name).write_bytes(data)

            flutter = shutil.which("flutter", path=env.get("PATH"))
            if flutter is None:
                raise ControlError("fixed Flutter SDK unavailable")
            flutter = Path(flutter).resolve()
            dart = flutter.parent / "dart"
            if not dart.is_file():
                raise ControlError("bundled Dart executable missing")
            app = source / "apps/xenterm"
            version = json.loads(call([str(flutter), "--version", "--machine"], "sdk-version", 90, app)[1])
            if version.get("frameworkVersion") != "3.47.7":
                raise ControlError("unreviewed Flutter SDK version")
            call([str(flutter), "pub", "get", "--enforce-lockfile"], "locked-dependencies", 300, app)
            if digest(lock_path.read_bytes()) != lock_hash:
                raise ControlError("dependency resolution rewrote the lock")
            paths = [str(source / name) for name in FILES]
            call([str(dart), "format", *paths], "format-reviewed-files", 60, app)
            green = {name: (source / name).read_bytes() for name in FILES}
            format_diff = "".join(
                "".join(difflib.unified_diff(raw_green[name].decode().splitlines(True),
                                            green[name].decode().splitlines(True),
                                            fromfile="raw/" + name, tofile="formatted/" + name))
                for name in FILES)
            if len(format_diff.encode()) > MAX_OUTPUT:
                raise ControlError("formatter diff exceeds evidence limit")
            (logs / "formatter-only.patch").write_text(format_diff)
            # This bounded diff is retained for independent review before any
            # formatted bytes can be promoted into the product branch.
            git("diff", "--no-ext-diff", "--", *FILES, label="formatted-source-diff")
            call([str(flutter), "analyze", "--no-pub"], "analyze-reviewed-files", 180, app)
            red = base_screen
            receipt = {"base": SOURCE, "patches": PATCHES, "raw_green": FILES,
                       "formatted_green": {name: digest(data) for name, data in green.items()},
                       "red_screen": digest(red), "lock_sha256": lock_hash, "sdk": version}
            (logs / "source-identity.json").write_text(json.dumps(receipt, indent=2) + "\n")
            for name, data in green.items():
                (logs / Path(name).name).write_bytes(data)
            try:
                (source / SCREEN).write_bytes(red)
                command = [str(flutter), "test", "--no-pub", "--machine",
                           "test/sftp_test.dart", "--plain-name", LATE]
                code, output = call(command, "red-stale-selection", 120, app, True)
                verify_machine(output, code, [LATE], red=True)
            finally:
                for name, data in green.items():
                    (source / name).write_bytes(data)
            for index, name in enumerate(NEW_TESTS):
                code, output = call([str(flutter), "test", "--no-pub", "--machine",
                                     "test/sftp_test.dart", "--plain-name", name],
                                    f"green-exact-{index}", 90, app, True)
                verify_machine(output, code, [name])
            code, output = call([str(flutter), "test", "--no-pub", "--machine", "test/sftp_test.dart"],
                                "green-full-sftp", 180, app, True)
            verify_machine(output, code, ALL_TESTS)
            if any((source / name).read_bytes() != data for name, data in green.items()):
                raise ControlError("green source drifted during tests")
            if digest(lock_path.read_bytes()) != lock_hash:
                raise ControlError("lock changed during validation")
            if sorted(git("diff", "--name-only", label="final-scope").splitlines()) != sorted(FILES):
                raise ControlError("validation changed files outside the reviewed pair")
            if git("ls-files", "--others", "--exclude-standard", label="untracked-source"):
                raise ControlError("unexpected untracked source files")
            cancel.check()
            print("SFTP_ACTION_STATE_RED_GREEN_PASS synthetic_only=true", flush=True)
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--logs", type=Path, required=True)
    run(parser.parse_args())
