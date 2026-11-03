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
                 patch("run_headless_owner_gate.time.monotonic", side_effect=[0, 0, 2, 2] if scenario == "timeout" else [0, 0, 0]):
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



class PermissionContractTests(unittest.TestCase):
    def setUp(self):
        import copy
        from run_headless_owner_gate import REVIEWED_CONTRACT
        self.manifest = copy.deepcopy(REVIEWED_CONTRACT)
        self.manifest.update(source_sha="1" * 40, source_tree="2" * 40)

    def test_unbound_manifest_fails_before_launch(self):
        from run_headless_owner_gate import verify_source
        self.manifest["source_sha"] = "BIND_BEFORE_PUBLICATION"
        with patch("run_headless_owner_gate.checked") as checked:
            with self.assertRaises(RuntimeError):
                verify_source(Path("."), self.manifest, Path("."))
            checked.assert_not_called()

    def test_exact_manifest_contract_not_just_counts(self):
        import copy
        from run_headless_owner_gate import validate_manifest
        validate_manifest(self.manifest)
        mutations = []
        for key in ("source_sha", "source_tree"):
            mutated = copy.deepcopy(self.manifest)
            mutated[key] = "BIND_BEFORE_PUBLICATION"
            mutations.append(mutated)
        for key in ("parent_sha", "parent_tree", "baseline_sha", "baseline_tree"):
            mutated = copy.deepcopy(self.manifest)
            mutated[key] = "f" * 40
            mutations.append(mutated)
        for key in ("files", "parent_blobs", "baseline_files"):
            mutated = copy.deepcopy(self.manifest)
            first = next(iter(mutated[key]))
            value = mutated[key].pop(first)
            mutated[key]["crates/xenterm/src/main.rs"] = value
            mutations.append(mutated)
        for key in ("new_tests", "isolated_tests", "maintenance_core_tests"):
            mutated = copy.deepcopy(self.manifest)
            mutated[key][0] = "tunnel::owner::lifecycle_tests::external_permission_writer_fixture"
            mutations.append(mutated)
        mutated = copy.deepcopy(self.manifest)
        mutated["allow_extra_files"] = True
        mutations.append(mutated)
        for mutated in mutations:
            with self.subTest(mutated=mutated):
                with self.assertRaises(RuntimeError):
                    validate_manifest(mutated)

    def test_reviewed_inventory_has_22_owner_and_16_maintenance(self):
        names = self.manifest["new_tests"] + self.manifest["maintenance_core_tests"]
        self.assertEqual(len(self.manifest["new_tests"]), 22)
        self.assertEqual(len(self.manifest["maintenance_core_tests"]), 16)
        self.assertEqual(len(names), len(set(names)))
        self.assertEqual(len(self.manifest["isolated_tests"]), 16)
        self.assertNotIn("tunnel::owner::lifecycle_tests::external_permission_writer_fixture", names)
        verify_inventory("".join(name + ": test\n" for name in names), names)
        with self.assertRaises(RuntimeError):
            verify_inventory("".join(name + ": test\n" for name in names[:-1]), names)

    def test_only_external_revocation_requires_three_real_layers(self):
        from run_headless_owner_gate import verify_required, EXTERNAL_REVOCATION
        for name in self.manifest["new_tests"] + self.manifest["maintenance_core_tests"]:
            expected = 3 if name == EXTERNAL_REVOCATION else (2 if name in self.manifest["isolated_tests"] else 1)
            verify_required(PASS * expected, name, self.manifest)
            for wrong in (0, 1, 2, 3, 4):
                if wrong != expected:
                    with self.subTest(name=name, wrong=wrong):
                        with self.assertRaises(RuntimeError):
                            verify_required(PASS * wrong, name, self.manifest)
            for bad in (PASS.replace("1 passed", "0 passed"), PASS.replace("0 ignored", "1 ignored"), PASS.replace("0 failed", "1 failed")):
                with self.assertRaises(RuntimeError):
                    verify_required(PASS * (expected - 1) + bad, name, self.manifest)
        with self.assertRaises(RuntimeError):
            verify_required(PASS, "tunnel::owner::lifecycle_tests::external_permission_writer_fixture", self.manifest)

    def exercise_source(self, replace_command=None, replacement=None, changed_digest=None):
        from run_headless_owner_gate import verify_source
        m = self.manifest
        answers = {
            ("rev-parse", "HEAD"): m["source_sha"],
            ("rev-parse", "HEAD^{tree}"): m["source_tree"],
            ("rev-parse", "HEAD^1"): m["parent_sha"],
            ("rev-parse", "HEAD^1^{tree}"): m["parent_tree"],
            ("rev-parse", "HEAD^1^1"): m["baseline_sha"],
            ("rev-parse", "HEAD^1^1^{tree}"): m["baseline_tree"],
            ("show", "-s", "--format=%P", "HEAD"): m["parent_sha"],
            ("show", "-s", "--format=%P", "HEAD^1"): m["baseline_sha"],
            ("diff", "--name-only", "HEAD^1", "HEAD"): "\n".join(m["files"]),
            ("diff", "--name-only", "HEAD^1^1", "HEAD^1"): "\n".join(m["parent_blobs"]),
            ("status", "--porcelain"): "",
        }
        for name, blob in m["parent_blobs"].items():
            answers[("rev-parse", "HEAD^1:" + name)] = blob
        def run(command, **_kwargs):
            if command[0] == "git":
                key = tuple(command[1:])
                return replacement if key == replace_command else answers[key]
            self.assertEqual(command[0], sys.executable)
            self.assertEqual(command[1], "-c")
            self.assertIn("hashlib.sha256", command[2])
            key = command[3]
            if key == replace_command:
                return replacement
            return m["baseline_files"][key.removeprefix("HEAD^1^1:")]
        digest = Mock()
        digest.hexdigest.return_value = changed_digest or next(iter(m["files"].values()))
        with tempfile.TemporaryDirectory() as directory, \
             patch("run_headless_owner_gate.checked", side_effect=run), \
             patch("run_headless_owner_gate.hashlib.sha256", return_value=digest), \
             patch.object(Path, "read_bytes", return_value=b"synthetic candidate"):
            verify_source(Path(directory), m, Path(directory))

    def test_exact_lineage_and_all_original_blobs_pass(self):
        self.exercise_source()

    def test_lineage_merge_scope_blob_and_dirty_fail_closed(self):
        owner = "crates/xenterm/src/tunnel/impls/owner.rs"
        cases = [
            (("rev-parse", "HEAD"), "0" * 40),
            (("rev-parse", "HEAD^{tree}"), "0" * 40),
            (("rev-parse", "HEAD^1"), "0" * 40),
            (("rev-parse", "HEAD^1^{tree}"), "0" * 40),
            (("rev-parse", "HEAD^1^1"), "0" * 40),
            (("rev-parse", "HEAD^1^1^{tree}"), "0" * 40),
            (("show", "-s", "--format=%P", "HEAD"), self.manifest["parent_sha"] + " " + "f" * 40),
            (("show", "-s", "--format=%P", "HEAD^1"), self.manifest["baseline_sha"] + " " + "f" * 40),
            (("diff", "--name-only", "HEAD^1", "HEAD"), owner),
            (("diff", "--name-only", "HEAD^1^1", "HEAD^1"), owner),
            (("rev-parse", "HEAD^1:" + owner), "0" * 40),
            ("HEAD^1^1:" + owner, "0" * 64),
            (("status", "--porcelain"), " M crates/xenterm/src/main.rs"),
        ]
        for command, answer in cases:
            with self.subTest(command=command):
                with self.assertRaises(RuntimeError):
                    self.exercise_source(command, answer)

    def test_workflow_is_core_only_precisely_scoped_and_budgeted(self):
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/next-flutter-headless-owner.yml").read_text()
        branch = "validation/flutter-permission-regression-20261103"
        self.assertIn("branches: [" + branch + "]", workflow)
        self.assertIn("github.ref == 'refs/heads/" + branch + "'", workflow)
        self.assertIn("github.repository == 'xue-ding-e/XenTerm-Next'", workflow)
        self.assertIn("fetch-depth: 3", workflow)
        self.assertIn("group: next-product-heavy", workflow)
        self.assertIn("cancel-in-progress: false", workflow)
        self.assertIn("queue: max", workflow)
        self.assertIn("timeout-minutes: 60", workflow)
        self.assertIn("timeout-minutes: 50", workflow)
        self.assertEqual(workflow.count("persist-credentials: false"), 2)
        for forbidden in ("workflow_dispatch:", "flutter build", "cargo build", "xenterm-bridge", "gh release", "contents: write"):
            self.assertNotIn(forbidden, workflow)

    def test_isolated_env_excludes_credentials_and_uses_private_profiles(self):
        from run_headless_owner_gate import isolated_env
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {
            "GITHUB_TOKEN": "synthetic", "SSH_AUTH_SOCK": "synthetic",
            "HTTP_PROXY": "synthetic", "XENTERM_DATA_DIR": "/must-not-reuse",
        }):
            root = Path(directory)
            env = isolated_env(root, root / "source")
            for forbidden in ("GITHUB_TOKEN", "SSH_AUTH_SOCK", "HTTP_PROXY"):
                self.assertNotIn(forbidden, env)
            self.assertEqual(env["XENTERM_DATA_DIR"], str(root / "XENTERM_DATA_DIR"))
            self.assertEqual((root / "XENTERM_DATA_DIR").stat().st_mode & 0o777, 0o700)

    def test_modified_worktree_file_hash_fails_even_with_clean_git_mock(self):
        with self.assertRaises(RuntimeError):
            self.exercise_source(changed_digest="0" * 64)

class CleanupAndContinuationTests(unittest.TestCase):
    def test_real_background_descendants_stop_after_success_and_failure(self):
        from run_headless_owner_gate import ProcessFailed, live_group_members
        for exit_code in (0, 7):
            with self.subTest(exit_code=exit_code), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                marker = root / "descendant.pid"
                script = (
                    "import os,subprocess,sys,time; from pathlib import Path; "
                    "child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'],"
                    "stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL); "
                    "Path(sys.argv[1]).write_text(str(child.pid)); "
                    "print(os.getpid(),flush=True); sys.exit(int(sys.argv[2]))"
                )
                try:
                    if exit_code:
                        with self.assertRaises(ProcessFailed):
                            checked([sys.executable, "-c", script, str(marker), str(exit_code)],
                                    cwd=root, logs=root, label="background", timeout=5)
                    else:
                        checked([sys.executable, "-c", script, str(marker), str(exit_code)],
                                cwd=root, logs=root, label="background", timeout=5)
                    pgid = int((root / "background.log").read_text())
                    self.assertEqual(live_group_members(pgid), [])
                    pid = int(marker.read_text())
                    stat = Path(f"/proc/{pid}/stat")
                    if stat.exists():
                        self.assertIn(stat.read_text().rsplit(")", 1)[1].split()[0], ("Z", "X"))
                finally:
                    if marker.exists():
                        try:
                            os.kill(int(marker.read_text()), signal.SIGKILL)
                        except ProcessLookupError:
                            pass

    def test_cleanup_failure_never_reports_success(self):
        from run_headless_owner_gate import cleanup_owned_group
        process = Mock(pid=43210)
        with patch("run_headless_owner_gate.os.killpg", side_effect=PermissionError("synthetic")):
            with self.assertRaises(PermissionError):
                cleanup_owned_group(process)
        process.communicate.assert_not_called()

    def test_first_exact_nonzero_continues_but_final_result_fails(self):
        from run_headless_owner_gate import REVIEWED_CONTRACT, ProcessFailed, run_regressions, EXTERNAL_REVOCATION
        names = REVIEWED_CONTRACT["new_tests"] + REVIEWED_CONTRACT["maintenance_core_tests"]
        calls = []
        def invoke(command, label, timeout, env):
            calls.append(label)
            if label == "exact-00":
                raise ProcessFailed("synthetic real test failure: exit 101")
            if label == "full-core":
                return PASS
            name = names[int(label.split("-")[1])]
            layers = 3 if name == EXTERNAL_REVOCATION else (2 if name in REVIEWED_CONTRACT["isolated_tests"] else 1)
            return PASS * layers
        with self.assertRaisesRegex(RuntimeError, "REGRESSION_FAILURES"):
            run_regressions(invoke, REVIEWED_CONTRACT, Deadline(2000), {})
        self.assertEqual(calls, [f"exact-{index:02d}" for index in range(38)] + ["full-core"])

    def test_control_errors_abort_without_later_tests(self):
        from run_headless_owner_gate import REVIEWED_CONTRACT, run_regressions
        for error in (Cancelled("cancel"), subprocess.TimeoutExpired("synthetic", 1),
                      RuntimeError("cleanup_unconfirmed"), RuntimeError("budget exhausted")):
            invoke = Mock(side_effect=error)
            with self.subTest(error=error), self.assertRaises(type(error)):
                run_regressions(invoke, REVIEWED_CONTRACT, Deadline(2000), {})
            self.assertEqual(invoke.call_count, 1)

    def test_full_suite_nonzero_cannot_be_swallowed(self):
        from run_headless_owner_gate import ProcessFailed, run_regressions
        manifest = {"new_tests": [], "maintenance_core_tests": [], "isolated_tests": []}
        with self.assertRaisesRegex(RuntimeError, "REGRESSION_FAILURES"):
            run_regressions(Mock(side_effect=ProcessFailed("full-core exit 101")), manifest, Deadline(2000), {})

    def test_missing_full_budget_is_explicit_failure(self):
        from run_headless_owner_gate import run_regressions
        manifest = {"new_tests": [], "maintenance_core_tests": [], "isolated_tests": []}
        invoke = Mock()
        with self.assertRaisesRegex(RuntimeError, "insufficient remaining budget"):
            run_regressions(invoke, manifest, Deadline(100), {})
        invoke.assert_not_called()

    def test_compile_and_tests_share_sanitized_official_toolchain_environment(self):
        import json
        from run_headless_owner_gate import run, EXTERNAL_REVOCATION
        manifest_path = Path(__file__).with_name("headless_owner_candidate.json")
        manifest = json.loads(manifest_path.read_text())
        observed = []
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            toolchain = root / "rustup/toolchains/1.99.1-test/bin"
            toolchain.mkdir(parents=True)
            for name in ("rustc", "cargo"):
                (toolchain / name).touch()
            def invoke(command, **kwargs):
                env = kwargs["env"]
                observed.append((command, env))
                self.assertIsNotNone(env)
                self.assertEqual(env["RUSTUP_TOOLCHAIN"], "1.99.1")
                for forbidden in ("GITHUB_TOKEN", "HTTP_PROXY", "SSH_AUTH_SOCK"):
                    self.assertNotIn(forbidden, env)
                label = kwargs["label"]
                if label.startswith("resolve-"):
                    return str(toolchain / label.removeprefix("resolve-")) + "\n"
                if label == "rustc":
                    return "rustc 1.99.1 (official synthetic)\n"
                if label == "cargo":
                    return "cargo 1.99.0 (official synthetic)\n"
                if label == "inventory":
                    return "".join(name + ": test\n" for name in manifest["new_tests"] + manifest["maintenance_core_tests"])
                if label.startswith("exact-"):
                    name = command[command.index("--lib") + 1]
                    layers = 3 if name == EXTERNAL_REVOCATION else (2 if name in manifest["isolated_tests"] else 1)
                    return PASS * layers
                return PASS
            with patch.dict(os.environ, {"RUSTUP_HOME": str(root / "rustup"), "CARGO_HOME": str(root / "cargo"),
                                         "GITHUB_TOKEN": "synthetic", "HTTP_PROXY": "synthetic", "SSH_AUTH_SOCK": "synthetic",
                                         "RUSTUP_TOOLCHAIN": "unreviewed"}), \
                 patch("run_headless_owner_gate.shutil.which", return_value=str(root / "rustup-bin")), \
                 patch("run_headless_owner_gate.verify_source"), \
                 patch("run_headless_owner_gate.checked", side_effect=invoke), \
                 patch.object(sys, "argv", ["gate", "--source", str(root), "--manifest", str(manifest_path), "--logs", str(root / "logs")]):
                run()
            compile_env = next(env for command, env in observed if "--no-run" in command)
            self.assertEqual(compile_env["PATH"].split(os.pathsep)[0], str(toolchain))
            self.assertEqual(compile_env["CARGO_HOME"], str(root / "cargo"))
            self.assertEqual(compile_env["RUSTUP_HOME"], str(root / "rustup"))
            for command, env in observed:
                self.assertEqual(env, compile_env)


if __name__ == "__main__":
    unittest.main()
