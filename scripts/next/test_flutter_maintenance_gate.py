import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import unittest


spec = importlib.util.spec_from_file_location("maintenance_gate", Path(__file__).with_name("run_flutter_maintenance_regressions.py"))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class MaintenanceGateTests(unittest.TestCase):
    def test_inventory_requires_all_thirteen_exact_library_names(self):
        self.assertEqual(len(gate.TUNNEL_TESTS), 13)
        self.assertEqual(len(set(gate.TUNNEL_TESTS)), 13)
        inventory = "\n".join(f"{name}: test" for name in gate.TUNNEL_TESTS)
        gate.verify_inventory(inventory, gate.TUNNEL_TESTS)
        with self.assertRaisesRegex(RuntimeError, "not discovered"):
            gate.verify_inventory("\n".join(inventory.splitlines()[1:]), gate.TUNNEL_TESTS)

    def test_zero_or_ignored_tests_do_not_count_as_a_pass(self):
        for summary in (
            "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 99 filtered out",
            "test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 99 filtered out",
            "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 99 filtered out",
            "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 99 filtered out",
        ):
            with self.subTest(summary=summary), self.assertRaises(RuntimeError):
                gate.verify_single_pass(summary, "synthetic")
        gate.verify_single_pass(
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 99 filtered out", "synthetic"
        )

    def test_source_is_an_exact_commit_not_a_branch_or_another_sha(self):
        gate.verify_source("a" * 40, "a" * 40 + "\n")
        for expected, actual in (("main", "a" * 40), ("a" * 40, "b" * 40)):
            with self.assertRaises(RuntimeError):
                gate.verify_source(expected, actual)

    def test_runner_uses_library_exact_selection_and_checks_every_result(self):
        commands = []

        def fake(command, **kwargs):
            commands.append(command)
            if command[0] == "git":
                output = "a" * 40 + "\n"
            elif command[-1] == "--list":
                tests = gate.BRIDGE_TESTS if "xenterm-bridge" in command else gate.TUNNEL_TESTS + gate.SETTINGS_TESTS
                output = "\n".join(f"{name}: test" for name in tests)
            else:
                output = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out\n"
            return subprocess.CompletedProcess(command, 0, output, "")

        with contextlib.redirect_stdout(io.StringIO()):
            gate.run_regressions("a" * 40, runner=fake)
        self.assertEqual(len(commands), 20)  # git + two inventories + 17 exact tests
        exact = [command for command in commands if "--exact" in command]
        expected = [*gate.TUNNEL_TESTS, *gate.SETTINGS_TESTS, *gate.BRIDGE_TESTS]
        for command, name in zip(exact, expected):
            target = gate.BRIDGE if name in gate.BRIDGE_TESTS else gate.CORE
            self.assertEqual(command[:len(target)], list(target))
            self.assertIn(name, command)
            self.assertIn("--exact", command)
            self.assertIn("--test-threads=1", command)

    def test_subprocess_failure_cannot_be_hidden_by_success_text(self):
        def fake(command, **kwargs):
            return subprocess.CompletedProcess(command, 1, "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out", "")

        with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(RuntimeError):
            gate.checked(["synthetic"], timeout=1, runner=fake)

    def test_all_settings_targets_have_an_exact_inventory_gate(self):
        self.assertEqual(len(gate.SETTINGS_TESTS), 3)
        self.assertEqual(len(gate.BRIDGE_TESTS), 1)
        for _, tests in gate.RUST_GROUPS:
            inventory = "\n".join(f"{name}: test" for name in tests)
            gate.verify_inventory(inventory, tests)
            with self.assertRaises(RuntimeError):
                gate.verify_inventory("\n".join(inventory.splitlines()[1:]), tests)

    def test_active_count_translation_reuses_the_existing_terminal_entry(self):
        root = Path(__file__).resolve().parents[2] / "apps/xenterm/lib/l10n"
        self.assertNotIn("'{count} active':", (root / "zh_screens.dart").read_text(encoding="utf-8"))
        self.assertIn("'{count} active': '{count} 个进行中'", (root / "zh_terminal.dart").read_text(encoding="utf-8"))
        self.assertIn("...zhTerminal,", (root / "app_language.dart").read_text(encoding="utf-8"))

    def test_authoritative_dart_gates_precede_rust_but_native_consumers_do_not(self):
        root = Path(__file__).resolve().parents[2]
        workflow = (root / ".github/workflows/next-flutter-windows.yml").read_text()
        steps = [
            "Resolve and verify Flutter dependencies",
            "Verify dependency resolution did not rewrite the lockfile",
            "Analyze Flutter",
            "Test exact Flutter runtime regressions (14 required)",
            "Test exact Rust maintenance regressions (17 required)",
            "Build Rust bridge and CLI/MCP companion",
            "Test Flutter",
            "Build Flutter Windows",
            "Bundle native bridge",
            "Smoke test packaged native bridge with isolated data",
        ]
        positions = [workflow.index(f"- name: {name}\n") for name in steps]
        self.assertEqual(positions, sorted(positions))

    def test_validation_ref_has_one_readonly_artifact_workflow(self):
        root = Path(__file__).resolve().parents[2]
        workflows = root / ".github/workflows"
        files = list(workflows.glob("*.yml")) + list(workflows.glob("*.yaml"))
        self.assertEqual([file.name for file in files], ["next-flutter-windows.yml"])
        workflow = files[0].read_text()
        self.assertIn("branches: [flutter-next, validation/flutter-maintenance-20261102]", workflow)
        self.assertIn("github.ref == 'refs/heads/validation/flutter-maintenance-20261102'", workflow)
        self.assertIn("permissions:\n  contents: read", workflow)
        self.assertIn("persist-credentials: false", workflow)
        self.assertIn('cancel-in-progress: false', workflow)
        self.assertNotIn('contents: write', workflow)
        self.assertNotIn('gh release', workflow)
        self.assertNotIn('validation/flutter-*', workflow)
        for suite in ("rust", "flutter"):
            self.assertIn(f'--suite {suite} --expected-source "$env:GITHUB_SHA"', workflow)

    @staticmethod
    def flutter_output(*, omit=None, skipped=None, failed=None, done=True):
        events = []
        for index, name in enumerate(gate.DART_TESTS):
            if name == omit:
                continue
            events.append({"type": "testStart", "test": {"id": index, "name": name}})
            events.append({"type": "testDone", "testID": index, "result": "failure" if name == failed else "success", "skipped": name == skipped})
        events.append({"type": "done", "success": done})
        return "\n".join(json.dumps(event) for event in events)

    def test_flutter_machine_results_require_every_real_non_skipped_test(self):
        self.assertEqual(len(gate.DART_TESTS), 14)
        gate.verify_flutter_results(self.flutter_output())
        for kwargs in ({"omit": gate.DART_TESTS[0]}, {"skipped": gate.DART_TESTS[0]}, {"failed": gate.DART_TESTS[0]}, {"done": False}):
            with self.subTest(kwargs=kwargs), self.assertRaises(RuntimeError):
                gate.verify_flutter_results(self.flutter_output(**kwargs))
        with self.assertRaises(RuntimeError):
            gate.verify_flutter_results("All tests passed!")

    def test_late_errors_hidden_or_retried_results_cannot_be_false_green(self):
        valid = self.flutter_output()
        late_error = json.dumps({"type": "error", "testID": 0, "error": "synthetic late error"})
        with self.assertRaises(RuntimeError):
            gate.verify_flutter_results(valid + "\n" + late_error)
        for field in ("hidden", "retried"):
            events = [json.loads(line) for line in valid.splitlines()]
            events[1][field] = True
            with self.subTest(field=field), self.assertRaises(RuntimeError):
                gate.verify_flutter_results("\n".join(json.dumps(event) for event in events))

    def test_flutter_runner_uses_the_focused_runtime_suite(self):
        commands = []
        def fake(command, **kwargs):
            commands.append((command, kwargs.get("cwd")))
            output = "a" * 40 if command[0] == "git" else self.flutter_output()
            return subprocess.CompletedProcess(command, 0, output, "")
        with contextlib.redirect_stdout(io.StringIO()):
            gate.run_regressions("a" * 40, suite="flutter", runner=fake)
        self.assertEqual(len(commands), 2)
        self.assertIn("test/runtime_tunnels_test.dart", commands[1][0])
        self.assertIn("test/runtime_tunnel_loader_test.dart", commands[1][0])
        self.assertIn("json", commands[1][0])
        self.assertEqual(commands[1][1], Path("apps/xenterm"))


if __name__ == "__main__":
    unittest.main()
