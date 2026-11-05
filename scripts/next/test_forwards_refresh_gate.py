import copy
import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import unittest
from unittest.mock import patch

from run_forwards_refresh_gate import (
    ALL_TESTS, BASE, FILES, LATE, NEW_TESTS, PATCHES, SOURCE, FAILURE,
    Cancellation, ControlError, Deadline, checked, digest,
    isolated_env, live_members, verify_hashes, verify_machine,
)


def events(names, red=False, printed=False):
    result = []
    for index, name in enumerate(names):
        result.append({"type": "testStart", "test": {"id": index, "name": name}})
        if red:
            if printed:
                result.append({"type": "print", "testID": index, "messageType": "print",
                               "message": 'EXCEPTION CAUGHT BY FLUTTER TEST FRAMEWORK\n'
                               'Found 0 widgets with text "Latest saved forward"\n'
                               + FAILURE + '\nforwards_test.dart:130'})
            result.append({"type": "error", "testID": index,
                           "error": ('Test failed. See exception logs above.\nThe test description was: '
                                     + LATE) if printed else FAILURE,
                           "stackTrace": '' if printed else 'forwards_test.dart:130'})
        result.append({"type": "testDone", "testID": index, "hidden": False,
                       "skipped": False, "result": "error" if red else "success"})
    result.append({"type": "done", "success": not red})
    return result


def encode(items):
    return "\n".join(json.dumps(item) for item in items)


class Contracts(unittest.TestCase):
    def test_actual_frozen_patches_and_scope(self):
        verify_hashes(Path(__file__).parent, PATCHES)
        self.assertEqual(len(FILES), 2)
        self.assertEqual(set(FILES), set(BASE))
        self.assertTrue(all(name.startswith("apps/xenterm/") for name in FILES))
        self.assertEqual(len(NEW_TESTS), 9)
        self.assertEqual(len(ALL_TESTS), 11)
        self.assertEqual(len(set(ALL_TESTS)), 11)
        red = Path(__file__).with_name("forwards-refresh-red-control.patch").read_text()
        self.assertIn("forwards_screen.dart", red)
        self.assertNotIn("forwards_test.dart", red)

    def test_altered_patch_hash_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "patch").write_bytes(b"tampered")
            with self.assertRaises(ControlError):
                verify_hashes(root, {"patch": "0" * 64})

    def test_green_requires_exact_inventory(self):
        verify_machine(encode(events(ALL_TESTS)), 0, ALL_TESTS)
        for names in (ALL_TESTS[:-1], ALL_TESTS + [ALL_TESTS[0]], [], [LATE]):
            with self.assertRaises(ControlError):
                verify_machine(encode(events(names)), 0, ALL_TESTS)

    def test_green_rejects_skips_errors_and_wrong_exit(self):
        for key, value in (("skipped", True), ("result", "error"), ("result", "failure")):
            items = events([LATE]); items[1][key] = value
            with self.assertRaises(ControlError):
                verify_machine(encode(items), 0, [LATE])
        for code in (1, 2, -9):
            with self.assertRaises(ControlError):
                verify_machine(encode(events([LATE])), code, [LATE])

    def test_red_requires_exact_stale_refresh_failure(self):
        for printed in (False, True):
            verify_machine(encode(events([LATE], red=True, printed=printed)), 1, [LATE], red=True)
            for code in (0, 2, -9):
                with self.assertRaises(ControlError):
                    verify_machine(encode(events([LATE], red=True, printed=printed)), code, [LATE], red=True)
        for field, value in (("error", "Compilation failed"), ("stackTrace", "unrelated.dart"), ("testID", 999)):
            items = events([LATE], red=True); items[1][field] = value
            with self.assertRaises(ControlError):
                verify_machine(encode(items), 1, [LATE], red=True)

    def test_red_rejects_wrong_print_identity_reason_and_multiple_errors(self):
        for field, value in (("message", "unrelated failure"), ("testID", 99), ("messageType", "other")):
            items = events([LATE], red=True, printed=True); items[1][field] = value
            with self.assertRaises(ControlError):
                verify_machine(encode(items), 1, [LATE], red=True)
        items = events([LATE], red=True); items.insert(2, copy.deepcopy(items[1]))
        with self.assertRaises(ControlError):
            verify_machine(encode(items), 1, [LATE], red=True)
        with self.assertRaises(ControlError):
            verify_machine(encode(events(NEW_TESTS, red=True)), 1, [LATE], red=True)

    def test_machine_requires_json_done_and_exact_auxiliary(self):
        for output in ("compiler failed", encode(events([LATE])[:-1]), ""):
            with self.assertRaises(ControlError):
                verify_machine(output, 0, [LATE])
        auxiliary = [{"event": "test.startedProcess", "params": {"vmServiceUri": None}}]
        verify_machine(encode([auxiliary] + events([LATE])), 0, [LATE])
        for wrong in ([], [auxiliary[0], auxiliary[0]], [dict(auxiliary[0], extra=True)], "scalar", 7, None):
            with self.assertRaises(ControlError):
                verify_machine(encode([wrong] + events([LATE])), 0, [LATE])

    def test_hidden_loader_is_not_coverage(self):
        items = [{"type": "testStart", "test": {"id": 99, "name": "loading forwards_test.dart"}},
                 {"type": "testDone", "testID": 99, "hidden": True, "skipped": False, "result": "success"}]
        verify_machine(encode(items + events([LATE])), 0, [LATE])
        with self.assertRaises(ControlError):
            verify_machine(encode(items + [{"type": "done", "success": True}]), 0, [LATE])

    def test_total_budget_is_shared(self):
        with patch("run_forwards_refresh_gate.time.monotonic", side_effect=[10, 12, 19, 21]):
            deadline = Deadline(10)
            self.assertEqual(deadline.limit(9), 8)
            self.assertEqual(deadline.limit(9), 1)
            with self.assertRaises(ControlError): deadline.limit(9)

    def test_environment_excludes_tokens_proxies_and_real_profiles(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {
            "GITHUB_TOKEN": "fake", "HTTP_PROXY": "fake", "SSH_AUTH_SOCK": "fake",
            "XENTERM_DATA_DIR": "/not-the-test-profile",
        }):
            env = isolated_env(Path(directory))
            for key in ("GITHUB_TOKEN", "HTTP_PROXY", "SSH_AUTH_SOCK"): self.assertNotIn(key, env)
            self.assertEqual(env["PUB_HOSTED_URL"], "https://pub.flutter-io.cn")
            self.assertEqual(env["XENTERM_DATA_DIR"], str(Path(directory) / "XENTERM_DATA_DIR"))
            self.assertEqual(Path(env["XENTERM_DATA_DIR"]).stat().st_mode & 0o777, 0o700)

    def test_pinned_dart_only_workflow(self):
        root = Path(__file__).resolve().parents[2]
        workflow = (root / ".github/workflows/next-flutter-forwards-refresh.yml").read_text()
        script = Path(__file__).with_name("run_forwards_refresh_gate.py").read_text()
        for expected in ("ref: " + SOURCE, "flutter-version: '3.47.7'", "group: next-product-heavy",
                         "timeout-minutes: 25", "timeout-minutes: 20", "if-no-files-found: error"):
            self.assertIn(expected, workflow)
        self.assertEqual(workflow.count("persist-credentials: false"), 2)
        for expected in ('"--enforce-lockfile"', 'if digest(lock_path.read_bytes()) != lock_hash:',
                         'red = base_screen', 'if (source / SCREEN).read_bytes() != base_screen:', '"--no-pub"'):
            self.assertIn(expected, script)
        for forbidden in ("cargo ", "flutter build", "gh release", "contents: write", "workflow_dispatch:", "crates/xenterm/src/main.rs"):
            self.assertNotIn(forbidden, workflow); self.assertNotIn(forbidden, script)


class RealProcessControls(unittest.TestCase):
    def invoke(self, script, root, *, failure=False, seconds=3, limit=8192):
        return checked([sys.executable, "-c", script], cwd=root, env={"PATH": os.environ.get("PATH", "")},
                       logs=root, label="test", seconds=seconds, deadline=Deadline(5),
                       cancellation=Cancellation(), allow_failure=failure, max_output=limit)

    def test_nonzero_is_only_returned_when_explicitly_requested(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaises(ControlError):
                self.invoke("print('failure'); raise SystemExit(1)", root)
            code, output = self.invoke("print('failure'); raise SystemExit(1)", root, failure=True)
            self.assertEqual((code, output.strip()), (1, "failure"))

    def test_output_overflow_fails_and_evidence_is_capped(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(ControlError, "output limit"):
                self.invoke("print('x'*100000)", root, limit=1024)
            self.assertLessEqual((root / "test.log").stat().st_size, 1024)

    def test_timeout_terminates_direct_process(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(ControlError, "timeout"):
                self.invoke("import os,time; print(os.getpid(),flush=True); time.sleep(30)", root, seconds=.15)
            pid = int((root / "test.log").read_text())
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)

    def test_parent_exits_with_closed_stdout_descendant_is_cleaned(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            script = ("import os,sys,subprocess; "
                      "p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)'],"
                      "stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL); "
                      "print(os.getpid(),flush=True)")
            _, output = self.invoke(script, root)
            self.assertEqual(live_members(int(output)), [])

    def test_cancel_before_launch_does_not_spawn(self):
        cancel = Cancellation()
        cancel.handle(signal.SIGTERM, None)
        with tempfile.TemporaryDirectory() as directory, patch("run_forwards_refresh_gate.subprocess.Popen") as launch:
            with self.assertRaisesRegex(ControlError, "cancelled"):
                checked(["never"], cwd=Path(directory), env={}, logs=Path(directory), label="cancel",
                        seconds=1, deadline=Deadline(5), cancellation=cancel)
            launch.assert_not_called()


if __name__ == "__main__":
    unittest.main()
