"""Pinned, synthetic Flutter settings picker validation. No native product build."""
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

SOURCE = "de9c366867c4ff8c8c1b2b4cda6e9356c0ca1109"
SCREEN = "apps/xenterm/lib/screens/settings/settings_screen.dart"
TEST = "apps/xenterm/test/settings_test.dart"
FILES = {
    SCREEN: "674e8123cf217c9a365d1897b68ed3672d11e71d71302d8b0530cb6745cce142",
    TEST: "a9812b15e964fa1e2dec08ca4986ea71dab3c370b04fa6d1a856b1bde9c55a32",
}
BASE = {
    SCREEN: "68cadf557a4e4604d9ed9bead4ad228bb3172ed77f90324c7d9048c3ef5a4b7a",
    TEST: "df7ef71f716b28b34b4cefd39fc846cee4e26fa62deed5ef0cb4a90ef7e681cf",
}
PATCHES = {
    "settings-picker.patch": "16107067361a4a6479e63e16d087333f8a4aba6e25b2efe97d256dca588570a7",
    "settings-picker-red-control.patch": "551bf9da9259c6d107620f5f44c56bbbc587818f8f62cecd685df155c77a37ab",
}
LATE = "closed settings ignore a late download folder selection"
NEW_TESTS = [LATE, "mounted folder picker handles selection",
             "mounted folder picker handles cancel", "mounted folder picker handles empty"]
ALL_TESTS = NEW_TESTS + [
    "language menu switches settings labels without changing values",
    "renderer settings save through the core callback",
    "every tab fits the default 1280×720 window",
    "controls stay visible at the 940×600 minimum window",
]
GREEN_GUARD = "if (!mounted || directory == null || directory.isEmpty) return;"
RED_GUARD = "if (directory == null || directory.isEmpty) return;"
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


def red_version(green):
    if green.count(GREEN_GUARD) != 1:
        raise ControlError("formatted source must contain exactly one reviewed guard")
    return green.replace(GREEN_GUARD, RED_GUARD, 1)


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
            raise ControlError("red control must fail exactly the late-dispose test")
        if finishes[0].get("result") not in ("error", "failure"):
            raise ControlError("red control did not report a test error")
        matching = [event for event in errors
                    if event.get("testID") == finishes[0]["testID"]
                    and "setState() called after dispose()" in event.get("error", "")
                    and "_SettingsScreenState" in event.get("error", "")
                    and "settings_screen.dart" in event.get("stackTrace", "")
                    and "_save" in event.get("stackTrace", "")]
        # Flutter framework exceptions can be rendered as a same-test print
        # block, followed by a generic error event with an empty stack. Bind
        # both records to the single failed test; never accept generic failure.
        if len(errors) == 1 and not matching:
            error = errors[0]
            generic = ("Test failed. See exception logs above.\n"
                       "The test description was: " + LATE)
            blocks = [event for event in events
                      if event.get("type") == "print"
                      and "EXCEPTION CAUGHT BY FLUTTER TEST FRAMEWORK" in event.get("message", "")]
            if (error.get("testID") == finishes[0]["testID"]
                    and error.get("error") == generic
                    and error.get("stackTrace") == ""
                    and error.get("isFailure") is False
                    and len(blocks) == 1):
                block = blocks[0]
                message = block.get("message", "")
                if (block.get("testID") == error["testID"]
                        and block.get("messageType") == "print"
                        and "setState() called after dispose(): _SettingsScreenState#" in message
                        and "_SettingsScreenState._save (package:xenterm_app/screens/settings/settings_screen.dart:" in message
                        and "_SettingsScreenState._chooseDownloadFolder (package:xenterm_app/screens/settings/settings_screen.dart:" in message):
                    matching = [error]
        if len(errors) != 1 or len(matching) != 1:
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
        with tempfile.TemporaryDirectory(prefix="settings-picker-") as directory:
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
            git("apply", "--check", str(controls / "settings-picker.patch"), label="check-main-patch")
            git("apply", str(controls / "settings-picker.patch"), label="apply-main-patch")
            verify_hashes(source, FILES)
            raw_green = {name: (source / name).read_bytes() for name in FILES}
            # Prove the frozen red patch is exactly the guard removal before
            # any formatter can change its line context.
            git("apply", "--check", str(controls / "settings-picker-red-control.patch"), label="check-red-patch")
            git("apply", str(controls / "settings-picker-red-control.patch"), label="bind-red-patch")
            if (source / SCREEN).read_text() != red_version(raw_green[SCREEN].decode()):
                raise ControlError("red patch changes more than the reviewed guard")
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
            call([str(flutter), "analyze", "--no-pub", *paths], "analyze-reviewed-files", 180, app)
            red = red_version(green[SCREEN].decode()).encode()
            receipt = {"base": SOURCE, "patches": PATCHES, "raw_green": FILES,
                       "formatted_green": {name: digest(data) for name, data in green.items()},
                       "red_screen": digest(red), "lock_sha256": lock_hash, "sdk": version}
            (logs / "source-identity.json").write_text(json.dumps(receipt, indent=2) + "\n")
            for name, data in green.items():
                (logs / Path(name).name).write_bytes(data)
            try:
                (source / SCREEN).write_bytes(red)
                command = [str(flutter), "test", "--no-pub", "--machine",
                           "test/settings_test.dart", "--plain-name", LATE]
                code, output = call(command, "red-late-dispose", 120, app, True)
                verify_machine(output, code, [LATE], red=True)
            finally:
                for name, data in green.items():
                    (source / name).write_bytes(data)
            for index, name in enumerate(NEW_TESTS):
                code, output = call([str(flutter), "test", "--no-pub", "--machine",
                                     "test/settings_test.dart", "--plain-name", name],
                                    f"green-exact-{index}", 90, app, True)
                verify_machine(output, code, [name])
            code, output = call([str(flutter), "test", "--no-pub", "--machine", "test/settings_test.dart"],
                                "green-full-settings", 180, app, True)
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
            print("SETTINGS_PICKER_RED_GREEN_PASS synthetic_only=true", flush=True)
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--logs", type=Path, required=True)
    run(parser.parse_args())
