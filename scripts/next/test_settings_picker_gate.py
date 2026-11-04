import copy
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from run_settings_picker_gate import (
    ALL_TESTS, BASE, FILES, GREEN_GUARD, LATE, NEW_TESTS, PATCHES, RED_GUARD,
    SOURCE, Cancellation, ControlError, Deadline, checked, digest,
    isolated_env, live_members, red_version, verify_hashes, verify_machine,
)


def events(names, red=False):
    result = []
    for index, name in enumerate(names):
        result.append({"type": "testStart", "test": {"id": index, "name": name}})
        if red:
            result.append({"type": "error", "testID": index,
                           "error": "setState() called after dispose(): _SettingsScreenState",
                           "stackTrace": "_SettingsScreenState._save (package:xenterm_app/screens/settings/settings_screen.dart:166)"})
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
        self.assertEqual(len(NEW_TESTS), 4)
        self.assertEqual(len(ALL_TESTS), 8)
        self.assertEqual(len(set(ALL_TESTS)), 8)

    def test_altered_patch_hash_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "patch").write_bytes(b"tampered")
            with self.assertRaises(ControlError):
                verify_hashes(root, {"patch": "0" * 64})

    def test_red_guard_removal_is_exact_and_unique(self):
        original = "before\n" + GREEN_GUARD + "\nafter\n"
        self.assertEqual(red_version(original), "before\n" + RED_GUARD + "\nafter\n")
        for value in ("missing", GREEN_GUARD + GREEN_GUARD):
            with self.assertRaises(ControlError):
                red_version(value)

    def test_green_requires_exact_full_inventory(self):
        verify_machine(encode(events(ALL_TESTS)), 0, ALL_TESTS)
        for names in (ALL_TESTS[:-1], ALL_TESTS + [ALL_TESTS[0]], [], [LATE]):
            with self.assertRaises(ControlError):
                verify_machine(encode(events(names)), 0, ALL_TESTS)

    def test_green_requires_actual_success_not_skip_or_error(self):
        for key, value in (("skipped", True), ("result", "error"), ("result", "failure")):
            items = events([LATE])
            items[1][key] = value
            with self.assertRaises(ControlError):
                verify_machine(encode(items), 0, [LATE])
        for code in (1, 2, -9):
            with self.assertRaises(ControlError):
                verify_machine(encode(events([LATE])), code, [LATE])

    def test_red_accepts_only_expected_disposed_failure(self):
        verify_machine(encode(events([LATE], red=True)), 1, [LATE], red=True)
        for code in (0, 2, -9):
            with self.assertRaises(ControlError):
                verify_machine(encode(events([LATE], red=True)), code, [LATE], red=True)
        for field, value in (("error", "Compilation failed"),
                             ("error", "setState() called after dispose(): DifferentState"),
                             ("stackTrace", "unrelated.dart"),
                             ("testID", 999)):
            items = events([LATE], red=True)
            items[1][field] = value
            with self.assertRaises(ControlError):
                verify_machine(encode(items), 1, [LATE], red=True)

    def test_red_rejects_multiple_errors_and_unexpected_tests(self):
        items = events([LATE], red=True)
        items.insert(2, copy.deepcopy(items[1]))
        with self.assertRaises(ControlError):
            verify_machine(encode(items), 1, [LATE], red=True)
        with self.assertRaises(ControlError):
            verify_machine(encode(events(NEW_TESTS, red=True)), 1, [LATE], red=True)

    def test_machine_output_requires_valid_json_and_final_done(self):
        for output in ("compiler failed", encode(events([LATE])[:-1]), ""):
            with self.assertRaises(ControlError):
                verify_machine(output, 0, [LATE])

    def test_hidden_loader_is_not_claimed_as_coverage(self):
        items = [{"type": "testStart", "test": {"id": 99, "name": "loading settings_test.dart"}},
                 {"type": "testDone", "testID": 99, "hidden": True, "skipped": False, "result": "success"}]
        verify_machine(encode(items + events([LATE])), 0, [LATE])
        with self.assertRaises(ControlError):
            verify_machine(encode(items + [{"type": "done", "success": True}]), 0, [LATE])

    def test_total_budget_is_shared(self):
        with patch("run_settings_picker_gate.time.monotonic", side_effect=[10, 12, 19, 21]):
            deadline = Deadline(10)
            self.assertEqual(deadline.limit(9), 8)
            self.assertEqual(deadline.limit(9), 1)
            with self.assertRaises(ControlError):
                deadline.limit(9)

    def test_isolation_does_not_inherit_tokens_or_proxies(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {
            "GITHUB_TOKEN": "fake", "HTTP_PROXY": "fake", "SSH_AUTH_SOCK": "fake",
            "XENTERM_DATA_DIR": "/not-the-test-profile",
        }):
            env = isolated_env(Path(directory))
            for key in ("GITHUB_TOKEN", "HTTP_PROXY", "SSH_AUTH_SOCK"):
                self.assertNotIn(key, env)
            self.assertEqual(env["PUB_HOSTED_URL"], "https://pub.flutter-io.cn")
            self.assertEqual(env["XENTERM_DATA_DIR"], str(Path(directory) / "XENTERM_DATA_DIR"))
            self.assertEqual(Path(env["XENTERM_DATA_DIR"]).stat().st_mode & 0o777, 0o700)

    def test_fixed_dart_only_workflow_and_command_contracts(self):
        root = Path(__file__).resolve().parents[2]
        workflow = (root / ".github/workflows/next-flutter-settings-picker.yml").read_text()
        script = Path(__file__).with_name("run_settings_picker_gate.py").read_text()
        self.assertIn("ref: " + SOURCE, workflow)
        self.assertIn("flutter-version: '3.47.7'", workflow)
        self.assertIn("group: next-product-heavy", workflow)
        self.assertNotIn("queue: max", workflow)
        self.assertIn("if-no-files-found: error", workflow)
        self.assertIn("timeout-minutes: 25", workflow)
        self.assertIn("timeout-minutes: 20", workflow)
        self.assertEqual(workflow.count("persist-credentials: false"), 2)
        self.assertIn('"--enforce-lockfile"', script)
        self.assertIn('if digest(lock_path.read_bytes()) != lock_hash:', script)
        self.assertIn('"--no-pub"', script)
        for forbidden in ("cargo ", "rust-toolchain", "flutter build", "gh release", "contents: write",
                          "workflow_dispatch:", "crates/xenterm/src/main.rs"):
            self.assertNotIn(forbidden, workflow)
            self.assertNotIn(forbidden, script)

    def actual_red_events(self):
        path = Path(__file__).with_name("fixtures") / "picker-red-late-dispose.jsonl"
        return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]

    def test_actual_red_fixture_preserves_expected_failure(self):
        verify_machine(encode(self.actual_red_events()), 1, [LATE], red=True)

    def test_only_exact_auxiliary_envelope_is_accepted(self):
        auxiliary = [{"event": "test.startedProcess", "params": {"vmServiceUri": None}}]
        verify_machine(encode([auxiliary] + events([LATE])), 0, [LATE])
        for wrong in ([], [auxiliary[0], auxiliary[0]], [dict(auxiliary[0], extra=True)],
                      [{"event": "other", "params": {"vmServiceUri": None}}],
                      [{"event": "test.startedProcess", "params": {"vmServiceUri": "unexpected"}}],
                      "scalar", 7, None):
            with self.subTest(wrong=wrong), self.assertRaises(ControlError):
                verify_machine(encode([wrong] + events([LATE])), 0, [LATE])

    def test_actual_red_rejects_wrong_print_id_cause_and_stack(self):
        for change in ("wrong-id", "different-exception", "wrong-state", "missing-save",
                       "missing-picker", "missing-print", "duplicate-exception", "extra-error",
                       "generic-error-changed", "wrong-error-id"):
            items = self.actual_red_events()
            block = next(item for item in items if isinstance(item, dict) and item.get("type") == "print")
            error = next(item for item in items if isinstance(item, dict) and item.get("type") == "error")
            if change == "wrong-id":
                block["testID"] = 999
            elif change == "different-exception":
                block["message"] = block["message"].replace("setState() called after dispose()", "unrelated assertion")
            elif change == "wrong-state":
                block["message"] = block["message"].replace("_SettingsScreenState#", "_OtherState#")
            elif change == "missing-save":
                block["message"] = block["message"].replace("_SettingsScreenState._save", "_OtherState._save")
            elif change == "missing-picker":
                block["message"] = block["message"].replace("_SettingsScreenState._chooseDownloadFolder", "_OtherState._choose")
            elif change == "missing-print":
                items.remove(block)
            elif change == "duplicate-exception":
                items.insert(-1, copy.deepcopy(block))
            elif change == "extra-error":
                items.insert(-1, copy.deepcopy(error))
            elif change == "generic-error-changed":
                error["error"] = "Compilation failure"
            elif change == "wrong-error-id":
                error["testID"] = 999
            with self.subTest(change=change), self.assertRaises(ControlError):
                verify_machine(encode(items), 1, [LATE], red=True)


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
        with tempfile.TemporaryDirectory() as directory, patch("run_settings_picker_gate.subprocess.Popen") as launch:
            with self.assertRaisesRegex(ControlError, "cancelled"):
                checked(["never"], cwd=Path(directory), env={}, logs=Path(directory), label="cancel",
                        seconds=1, deadline=Deadline(5), cancellation=cancel)
            launch.assert_not_called()


if __name__ == "__main__":
    unittest.main()
