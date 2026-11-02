import unittest
from pathlib import Path
import os
import signal
import subprocess
import sys
import tempfile
import threading
import time
from unittest.mock import Mock, patch

from run_headless_owner_gate import (
    Cancellation, Cancelled, Deadline, checked,
    verify_exact, verify_full, verify_inventory, verify_sha,
)


PASS = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out\n"


class GateTests(unittest.TestCase):
    def test_unbound_source_fails_closed(self):
        with self.assertRaises(RuntimeError):
            verify_sha("BIND_BEFORE_PUBLICATION", "a" * 40)

    def test_exact_sha_required(self):
        verify_sha("a" * 40, "a" * 40 + "\n")
        with self.assertRaises(RuntimeError):
            verify_sha("a" * 40, "b" * 40)

    def test_inventory_is_required(self):
        verify_inventory("fixture::one: test\n", ["fixture::one"])
        with self.assertRaises(RuntimeError):
            verify_inventory("fixture::one: test\n", ["fixture::two"])

    def test_duplicate_requirements_fail(self):
        with self.assertRaises(RuntimeError):
            verify_inventory("fixture::one: test\n", ["fixture::one", "fixture::one"])

    def test_regular_exact_pass(self):
        verify_exact(PASS, False)

    def test_child_and_parent_both_required(self):
        verify_exact(PASS + PASS, True)
        with self.assertRaises(RuntimeError):
            verify_exact(PASS, True)

    def test_zero_or_ignored_cannot_pass(self):
        for output in [PASS.replace("1 passed", "0 passed"), PASS.replace("0 ignored", "1 ignored")]:
            with self.assertRaises(RuntimeError):
                verify_exact(output, False)

    def test_full_suite_requires_real_passes(self):
        verify_full(PASS)
        with self.assertRaises(RuntimeError):
            verify_full(PASS.replace("1 passed", "0 passed"))

    def test_every_stage_uses_the_same_remaining_budget(self):
        now = [100.0]
        deadline = Deadline(30, clock=lambda: now[0])
        self.assertEqual(deadline.limit(20), 20)
        now[0] = 125
        self.assertEqual(deadline.limit(20), 5)
        now[0] = 130
        with self.assertRaises(RuntimeError):
            deadline.limit(20)

    def exercise_cancellation(self, scenario):
        cancellation = Cancellation()
        process = Mock(pid=43210, returncode=-signal.SIGKILL)
        process.communicate.return_value = ("partial output\n", None)

        def launch(*_args, **_kwargs):
            if scenario == "during-popen":
                cancellation.handle(signal.SIGTERM, None)
                # No exception until the newly created handle is assigned.
                self.assertTrue(cancellation.launching)
            return process

        if scenario == "timeout":
            process.communicate.side_effect = [subprocess.TimeoutExpired(["fake"], 1), ("partial output\n", None)]
        elif scenario == "during-communicate":
            def interrupted(**_kwargs):
                process.communicate.side_effect = None
                cancellation.handle(signal.SIGINT, None)
                raise subprocess.TimeoutExpired(["fake"], 0.5)
            process.communicate.side_effect = interrupted

        def kill_group(pid, sig):
            self.assertEqual((pid, sig), (43210, signal.SIGKILL))
            self.assertTrue(cancellation.cleaning)
            # A repeated cancellation must not interrupt cleanup/reaping.
            cancellation.handle(signal.SIGTERM, None)

        with tempfile.TemporaryDirectory() as directory:
            with patch("run_headless_owner_gate.subprocess.Popen", side_effect=launch), \
                 patch("run_headless_owner_gate.os.killpg", side_effect=kill_group) as kill, \
                 patch("run_headless_owner_gate.time.monotonic", side_effect=[0, 0, 2] if scenario == "timeout" else [0, 0]):
                exception = subprocess.TimeoutExpired if scenario == "timeout" else Cancelled
                with self.assertRaises(exception):
                    checked(["fake"], cwd=Path(directory), logs=Path(directory), label="cancel",
                            timeout=1, cancellation=cancellation)
                kill.assert_called_once()
                self.assertEqual(process.communicate.call_args.kwargs["timeout"], 10)
                self.assertEqual((Path(directory) / "cancel.log").read_text(), "partial output\n")
                self.assertFalse(cancellation.cleaning)

    def test_signal_during_popen_cannot_lose_child_handle(self):
        self.exercise_cancellation("during-popen")

    def test_signal_during_communicate_kills_and_reaps(self):
        self.exercise_cancellation("during-communicate")

    def test_timeout_kills_and_reaps_despite_repeated_signal(self):
        self.exercise_cancellation("timeout")

    def test_real_sigterm_reaps_the_spawned_process(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            marker = root / "ready.pid"
            errors = []

            def cancel_when_ready():
                end = time.monotonic() + 5
                while time.monotonic() < end:
                    if marker.exists() and marker.read_text().strip():
                        os.kill(os.getpid(), signal.SIGTERM)
                        return
                    time.sleep(0.01)
                errors.append("child did not reach readiness barrier")

            trigger = threading.Thread(target=cancel_when_ready)
            with self.assertRaises(Cancelled):
                with Cancellation() as cancellation:
                    trigger.start()
                    checked([sys.executable, "-c",
                             "import os,sys,time; from pathlib import Path; "
                             "Path(sys.argv[1]).write_text(str(os.getpid())); time.sleep(60)", str(marker)],
                            cwd=root, logs=root, label="real-cancel", timeout=10, cancellation=cancellation)
            trigger.join(timeout=6)
            self.assertFalse(trigger.is_alive())
            self.assertFalse(errors)
            pid = int(marker.read_text())
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)


if __name__ == "__main__":
    unittest.main()
