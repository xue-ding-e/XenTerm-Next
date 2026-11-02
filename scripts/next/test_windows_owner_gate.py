import re
import signal
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from run_windows_owner_gate import (
    CARGO, LIB_CARGO, Cancellation, Cancelled, Deadline, checked, kill_tree,
    verify_exact, verify_full, verify_inventory, verify_sha,
)


PASS = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 694 filtered out\n"


class WindowsGateTests(unittest.TestCase):
    def test_job_env_cannot_reference_runner_context(self):
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/next-flutter-headless-windows.yml").read_text(encoding="utf-8")

        def verify_job_env(text):
            # A targeted regression for the service-side validation failure;
            # this does not claim to be a complete Actions schema validator.
            job_env = text.split("    env:\n", 1)[1].split("    steps:\n", 1)[0]
            allowed = {"github", "needs", "strategy", "matrix", "vars", "secrets", "inputs"}
            contexts = set(re.findall(r"\$\{\{\s*([A-Za-z_]\w*)\.", job_env))
            if not contexts.issubset(allowed):
                raise ValueError("context unavailable in job env")

        verify_job_env(workflow)
        invalid = workflow.replace("    env:\n", "    env:\n      XENTERM_DATA_DIR: ${{ runner.temp }}/profile\n", 1)
        with self.assertRaisesRegex(ValueError, "unavailable"):
            verify_job_env(invalid)
        self.assertIn("$ownerDataDir = Join-Path $env:RUNNER_TEMP", workflow)
        self.assertIn('"XENTERM_DATA_DIR=$ownerDataDir" | Out-File -FilePath $env:GITHUB_ENV', workflow)

    def test_immutable_source_sha(self):
        verify_sha("a" * 40, "a" * 40)
        for expected, actual in [("UNBOUND", "a" * 40), ("a" * 40, "b" * 40)]:
            with self.assertRaises(RuntimeError):
                verify_sha(expected, actual)

    def test_inventory_requires_every_unique_test(self):
        verify_inventory("one::test: test\n", ["one::test"])
        for required in [["missing::test"], ["one::test", "one::test"]]:
            with self.assertRaises(RuntimeError):
                verify_inventory("one::test: test\n", required)

    def test_parent_and_child_summaries_are_required(self):
        verify_exact(PASS, False)
        verify_exact(PASS + PASS, True)
        for output, isolated in [(PASS, True), (PASS.replace("1 passed", "0 passed"), False),
                                 (PASS.replace("0 ignored", "1 ignored"), False)]:
            with self.assertRaises(RuntimeError):
                verify_exact(output, isolated)

    def test_full_core_cannot_be_zero(self):
        verify_full(PASS)
        with self.assertRaises(RuntimeError):
            verify_full(PASS.replace("1 passed", "0 passed"))

    def test_full_core_has_no_target_filter_but_exact_inventory_is_library_only(self):
        self.assertEqual(CARGO, ["cargo", "test", "--locked", "-p", "xenterm", "--jobs", "2"])
        self.assertEqual(LIB_CARGO, [*CARGO, "--lib"])

    def test_budget_is_shared_across_stages(self):
        clock = [0]
        deadline = Deadline(30, clock=lambda: clock[0])
        self.assertEqual(deadline.limit(60), 30)
        clock[0] = 29
        self.assertEqual(deadline.limit(60), 1)
        clock[0] = 31
        with self.assertRaises(RuntimeError):
            deadline.limit(1)

    def test_windows_cleanup_targets_owned_pid_tree_not_image_names(self):
        process = Mock(pid=43210)
        with patch("run_windows_owner_gate.subprocess.run", return_value=Mock(returncode=0)) as run:
            kill_tree(process, platform="nt")
        self.assertEqual(run.call_args.args[0][1:], ["/PID", "43210", "/T", "/F"])
        self.assertNotIn("/IM", run.call_args.args[0])

    def test_windows_cleanup_failure_is_not_confirmed(self):
        for parent_status in (None, 0):
            with self.subTest(parent_status=parent_status):
                process = Mock(pid=43210)
                process.poll.return_value = parent_status
                with patch("run_windows_owner_gate.subprocess.run", return_value=Mock(returncode=1)):
                    with self.assertRaisesRegex(RuntimeError, "cleanup_unconfirmed"):
                        kill_tree(process, platform="nt")

    def exercise_cancel(self, during_spawn):
        cancellation = Cancellation()
        process = Mock(pid=43210, returncode=1)
        process.communicate.return_value = ("partial output\n", None)

        def launch(*_args, **_kwargs):
            if during_spawn:
                cancellation.handle(signal.SIGTERM, None)
            return process

        if not during_spawn:
            def interrupted(**_kwargs):
                process.communicate.side_effect = None
                cancellation.handle(signal.SIGINT, None)
                raise subprocess.TimeoutExpired(["fake"], 0.5)
            process.communicate.side_effect = interrupted

        def cleanup(owned):
            self.assertIs(owned, process)
            self.assertTrue(cancellation.cleaning)
            cancellation.handle(signal.SIGTERM, None)

        with tempfile.TemporaryDirectory() as directory:
            with patch("run_windows_owner_gate.subprocess.Popen", side_effect=launch), \
                 patch("run_windows_owner_gate.kill_tree", side_effect=cleanup) as kill:
                with self.assertRaises(Cancelled):
                    checked(["fake"], cwd=Path(directory), logs=Path(directory), label="cancel",
                            timeout=10, cancellation=cancellation)
                kill.assert_called_once_with(process)
                self.assertEqual(process.communicate.call_args.kwargs["timeout"], 10)
                self.assertEqual((Path(directory) / "cancel.log").read_text(), "partial output\n")

    def test_spawn_window_keeps_owned_handle(self):
        self.exercise_cancel(True)

    def test_signal_cleanup_survives_repeated_cancellation(self):
        self.exercise_cancel(False)

    def test_failed_tree_cleanup_still_reaps_direct_child_and_reports_unconfirmed(self):
        cancellation = Cancellation()
        process = Mock(pid=43210, returncode=1)
        process.communicate.return_value = ("partial output\n", None)

        def launch(*_args, **_kwargs):
            cancellation.handle(signal.SIGTERM, None)
            return process

        with tempfile.TemporaryDirectory() as directory:
            with patch("run_windows_owner_gate.subprocess.Popen", side_effect=launch), \
                 patch("run_windows_owner_gate.kill_tree", side_effect=RuntimeError("denied")):
                with self.assertRaisesRegex(RuntimeError, "cleanup_unconfirmed"):
                    checked(["fake"], cwd=Path(directory), logs=Path(directory), label="uncertain",
                            timeout=10, cancellation=cancellation)
                process.kill.assert_called_once()
                process.communicate.assert_called_once_with(timeout=10)


if __name__ == "__main__":
    unittest.main()
