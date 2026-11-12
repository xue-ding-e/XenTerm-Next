import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("maintenance_gate", Path(__file__).with_name("run_flutter_maintenance_regressions.py"))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


# Keep target expectations independent of the runner constants and routing table.
EXPECTED_CORE_TARGET = ("cargo", "test", "--locked", "-p", "xenterm", "--lib")
EXPECTED_SETTINGS_TARGET = ("cargo", "test", "--locked", "-p", "xenterm-bridge", "--test", "settings")
EXPECTED_BRIDGE_LIB_TARGET = ("cargo", "test", "--locked", "-p", "xenterm-bridge", "--lib")


EXPECTED_INPUT_DART_TESTS = (
    "existing tab 1 takes input from tab 0",
    "existing tab 0 takes input from tab 1",
    "covered workspace rejects even residual terminal focus",
    "initially inactive panes neither autofocus nor accept input",
    "inactive lifecycle blocks hardware input until resumed",
    "mounting while inactive restores input on first resume",
    "resuming leaves an existing search field focused",
    "search typing never reaches either PTY",
    "explicit background commands preserve visible tab ownership",
    "same-frame field request beats pending terminal restoration",
    "delayed copy does not steal focus after switching tabs",
    "delayed copy does not steal focus after clicking a field",
    "modal route blocks queued terminal restoration (requestFocus=true)",
    "modal route blocks queued terminal restoration (requestFocus=false)",
    "inline credential field keeps its identity across A B A",
    "pending prompt blocks residual focus and ordinary send paths",
    "new hidden prompt blocks commands without disabling background input",
    "new unpolled prompt blocks ordinary clear input",
    "new unpolled prompt blocks ordinary snippet input",
    "new unpolled prompt blocks ordinary hardware input",
    "prompt replaced while hidden never restores the old field",
    "unfocusable saved prompt field does not fall back to PTY",
    "competing field wins over queued inline prompt restoration",
    "new hidden prompt without a saved field never focuses PTY",
    "resolved hidden prompt releases its field for terminal input",
    "activation read failure (version) blocks input until refreshed",
    "activation read failure (snapshot) blocks input until refreshed",
    "activation read failure (empty) blocks input until refreshed",
    "rapid A B A routes input and disposal cancels pending focus",
    "reconnect credential same-id prompt clears fields before first frame",
    "reconnect credential same-id prompt clears fields after blank frame",
    "reconnect mfa same-id prompt clears fields before first frame",
    "reconnect mfa same-id prompt clears fields after blank frame",
    "reconnect rejects old Continue pointer-up before rebuild",
)
EXPECTED_INPUT_CORE_TESTS = (
    "session::protocol::input_admission_tests::send_raw_reports_actual_admission_and_closed_receiver",
    "session::protocol::input_admission_tests::prompt_answers_do_not_open_the_command_gate",
    "session::protocol::input_admission_tests::non_ssh_type_ahead_stays_available_before_the_worker_reads",
    "session::protocol::input_admission_tests::ready_shell_keeps_text_sudo_and_terminal_query_replies",
    "session::protocol::input_admission_tests::controls_remain_usable_during_startup",
    "session::protocol::input_admission_tests::close_before_ready_cannot_be_reopened_by_the_worker",
    "session::protocol::input_admission_tests::close_after_ready_closes_all_sender_clones",
    "session::protocol::input_admission_tests::dropped_readiness_token_never_opens_input",
    "session::protocol::input_admission_tests::readiness_token_does_not_keep_the_command_queue_connected",
    "session::protocol::input_admission_tests::poisoned_admission_lock_fails_closed",
    "session::protocol::input_admission_tests::concurrent_ready_and_close_cannot_reopen_input",
    "session::protocol::input_admission_tests::concurrent_input_is_never_enqueued_behind_close",
    "session::protocol::input_admission_tests::regressions::starting_ssh_rejects_handle_input_before_any_prompt_event",
    "session::protocol::input_admission_tests::regressions::starting_ssh_rejects_cloned_command_sender_input",
    "session::protocol::input_admission_tests::regressions::rejected_startup_input_is_never_replayed_after_admission_opens",
    "frontend::input_admission_tests::answering_a_prompt_does_not_make_terminal_input_ready",
    "frontend::input_admission_tests::responders_report_real_delivery_once_and_reject_dropped_receivers",
    "frontend::input_admission_tests::close_cancels_all_prompt_kinds_before_or_after_install_despite_clones",
    "frontend::input_admission_tests::prompt_answers_reject_stale_duplicate_missing_and_disconnected_receivers",
    "frontend::input_admission_tests::replacing_a_prompt_explicitly_cancels_the_previous_responder",
    "frontend::input_admission_tests::concurrent_close_and_answer_settle_once_without_reopening_input",
    "frontend::input_admission_tests::concurrent_close_and_prompt_install_always_cancel_retained_clones",
    "frontend::input_admission_tests::oneshot_wake_can_reenter_responder_and_close_without_held_locks",
    "frontend::input_admission_tests::poisoned_prompt_state_cancels_and_never_accepts_late_answers",
)
EXPECTED_ENCODING_TESTS = (
    "terminal::encoding::tests::gbk_round_trip_and_split_decode",
    "terminal::encoding::tests::unknown_label_falls_back_to_utf8",
    "terminal::local::tests::conpty_read_boundary_keeps_utf8_codepoints",
)
EXPECTED_WINDOWS_CORE_TESTS = (
    "terminal::terminal_windows_tests::conpty_powershell_preserves_long_unicode_output",
)
EXPECTED_INPUT_BRIDGE_TESTS = (
    "input_admission_tests::ffi_rejects_ssh_input_before_worker_or_prompt_pump_runs",
    "input_admission_tests::ffi_prompt_answer_rejects_live_handle_without_prompt_and_removed_handle",
)


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
            "test result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 99 filtered out",
            "test result: ok. 1 passed; 0 failed; 0 ignored; 1 measured; 99 filtered out",
            "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 99 filtered out",
            "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 99 filtered out",
        ):
            with self.subTest(summary=summary), self.assertRaises(RuntimeError):
                gate.verify_single_pass("test synthetic ... ok\n" + summary, "synthetic")
        gate.verify_single_pass(
            "test synthetic ... ok\n"
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
                if "xenterm-bridge" not in command:
                    tests = gate.TUNNEL_TESTS + gate.SETTINGS_TESTS + EXPECTED_INPUT_CORE_TESTS + EXPECTED_ENCODING_TESTS + EXPECTED_WINDOWS_CORE_TESTS
                elif "--lib" in command:
                    tests = EXPECTED_INPUT_BRIDGE_TESTS
                else:
                    tests = gate.BRIDGE_TESTS
                output = "\n".join(f"{name}: test" for name in tests)
            else:
                name = command[command.index("--") - 1]
                output = f"test {name} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out\n"
            return subprocess.CompletedProcess(command, 0, output, "")

        with patch.object(sys, "platform", "win32"), contextlib.redirect_stdout(io.StringIO()) as captured:
            gate.run_regressions("a" * 40, runner=fake)
        self.assertEqual(len(commands), 51)  # git + three inventories + 47 exact tests
        self.assertIn("suite=rust count=47", captured.getvalue())
        self.assertNotIn("PLATFORM_EXCLUDED", captured.getvalue())
        exact = [command for command in commands if "--exact" in command]
        expected = [*gate.TUNNEL_TESTS, *gate.SETTINGS_TESTS, *EXPECTED_INPUT_CORE_TESTS, *EXPECTED_ENCODING_TESTS, *EXPECTED_WINDOWS_CORE_TESTS,
                    *gate.BRIDGE_TESTS, *EXPECTED_INPUT_BRIDGE_TESTS]
        self.assertEqual(len(exact), 47)
        for command, name in zip(exact, expected):
            if name in EXPECTED_INPUT_BRIDGE_TESTS:
                target = EXPECTED_BRIDGE_LIB_TARGET
            else:
                target = EXPECTED_SETTINGS_TARGET if name in gate.BRIDGE_TESTS else EXPECTED_CORE_TARGET
            self.assertEqual(command, [*target, name, "--", "--exact", "--test-threads=1", "--nocapture"])
        self.assertEqual(
            [command for command in commands if command[-1] == "--list"],
            [[*target, "--", "--list"] for target in
             (EXPECTED_CORE_TARGET, EXPECTED_SETTINGS_TARGET, EXPECTED_BRIDGE_LIB_TARGET)],
        )

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
            "Test exact Flutter maintenance regressions (48 required)",
            "Test exact Rust maintenance regressions (47 required)",
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
    def flutter_output(*, names=None, omit=None, skipped=None, failed=None, done=True):
        events = []
        for index, name in enumerate(gate.DART_TESTS if names is None else names):
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
            if command[0] == "git":
                output = "a" * 40
            elif "test/terminal_input_ownership_test.dart" in command:
                output = self.flutter_output(names=EXPECTED_INPUT_DART_TESTS)
            else:
                output = self.flutter_output()
            return subprocess.CompletedProcess(command, 0, output, "")
        with contextlib.redirect_stdout(io.StringIO()) as captured:
            gate.run_regressions("a" * 40, suite="flutter", runner=fake)
        self.assertEqual(len(commands), 3)
        self.assertIn("suite=flutter count=48", captured.getvalue())
        self.assertIn("count=14", captured.getvalue())
        self.assertIn("count=34", captured.getvalue())
        self.assertIn("test/runtime_tunnels_test.dart", commands[1][0])
        self.assertIn("test/runtime_tunnel_loader_test.dart", commands[1][0])
        self.assertIn("json", commands[1][0])
        self.assertEqual(commands[1][1], Path("apps/xenterm"))
        self.assertNotIn("test/terminal_input_ownership_test.dart", commands[1][0])
        self.assertEqual(commands[2][0][-1:], ["test/terminal_input_ownership_test.dart"])
        self.assertNotIn("test/runtime_tunnels_test.dart", commands[2][0])
        self.assertIn("json", commands[2][0])
        self.assertEqual(commands[2][1], Path("apps/xenterm"))


    def test_windows_flutter_launcher_uses_argv_for_both_independent_groups(self):
        commands = []
        def fake(command, **kwargs):
            commands.append((command, kwargs))
            if command[0] == "git":
                output = "a" * 40
            elif "test/terminal_input_ownership_test.dart" in command:
                output = self.flutter_output(names=EXPECTED_INPUT_DART_TESTS)
            else:
                output = self.flutter_output()
            return subprocess.CompletedProcess(command, 0, output, "")
        with patch.object(sys, "platform", "win32"), contextlib.redirect_stdout(io.StringIO()):
            gate.run_regressions("a" * 40, suite="flutter", runner=fake)
        self.assertEqual(len(commands), 3)
        for command, kwargs in commands[1:]:
            self.assertEqual(command[:7], ["cmd.exe", "/d", "/c", "flutter", "test", "--reporter", "json"])
            self.assertNotIn("a" * 40, command)
            self.assertIsNot(kwargs.get("shell"), True)
            self.assertEqual(kwargs["cwd"], Path("apps/xenterm"))

    def test_input_manifests_match_reviewed_names_and_targets(self):
        self.assertEqual(gate.INPUT_OWNERSHIP_DART_TESTS, EXPECTED_INPUT_DART_TESTS)
        self.assertEqual(len(set(EXPECTED_INPUT_DART_TESTS)), 34)
        self.assertEqual(gate.INPUT_ADMISSION_TESTS, EXPECTED_INPUT_CORE_TESTS)
        self.assertEqual(gate.WINDOWS_INPUT_TESTS, EXPECTED_WINDOWS_CORE_TESTS)
        self.assertEqual(gate.BRIDGE_INPUT_TESTS, EXPECTED_INPUT_BRIDGE_TESTS)
        core, settings, bridge_input = gate.RUST_GROUPS
        self.assertEqual(core, (EXPECTED_CORE_TARGET, gate.TUNNEL_TESTS + gate.SETTINGS_TESTS + EXPECTED_INPUT_CORE_TESTS + EXPECTED_ENCODING_TESTS))
        self.assertEqual(settings, (EXPECTED_SETTINGS_TARGET, gate.BRIDGE_TESTS))
        self.assertEqual(bridge_input, (EXPECTED_BRIDGE_LIB_TARGET, EXPECTED_INPUT_BRIDGE_TESTS))
        self.assertEqual([len(tests) for _, tests in gate.RUST_GROUPS], [43, 1, 2])
        self.assertEqual(len({name for _, tests in gate.RUST_GROUPS for name in tests}), 46)

    def test_target_assertions_reject_consistent_constant_and_group_drift(self):
        # Mutate both the constant and routing table so a self-referential
        # expectation cannot pass merely by following the same wrong value.
        for target_name, before, after in (("CORE", "--lib", "--bins"), ("BRIDGE", "--test", "--bin")):
            original = getattr(gate, target_name)
            changed = tuple(after if part == before else part for part in original)
            groups = tuple((changed if target == original else target, tests)
                           for target, tests in gate.RUST_GROUPS)
            for method in (
                "test_runner_uses_library_exact_selection_and_checks_every_result",
                "test_input_manifests_match_reviewed_names_and_targets",
            ):
                with self.subTest(target=target_name, assertion=method):
                    with patch.object(gate, target_name, changed), patch.object(gate, "RUST_GROUPS", groups):
                        with self.assertRaises(AssertionError):
                            getattr(self, method)()

    def test_duplicate_rust_inventory_or_manifest_cannot_inflate_counts(self):
        with self.assertRaisesRegex(RuntimeError, "duplicate"):
            gate.verify_inventory("required: test\nrequired: test", ("required",))
        with self.assertRaisesRegex(RuntimeError, "duplicate"):
            gate.verify_inventory("required: test", ("required", "required"))

    def test_exact_rust_pass_requires_the_named_test_even_with_nocapture(self):
        summary = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out"
        for output in (summary, "test other ... ok\n" + summary,
                       "test required ... ok\ntest required ... ok\n" + summary,
                       "test required ... ok\n" + summary + "\n" + summary):
            with self.subTest(output=output), self.assertRaises(RuntimeError):
                gate.verify_single_pass(output, "required")
        gate.verify_single_pass("test required ... synthetic captured output\nmore output\nok\n" + summary, "required")

    def test_each_new_rust_inventory_name_is_required_before_execution(self):
        for missing in (*EXPECTED_INPUT_CORE_TESTS, *EXPECTED_ENCODING_TESTS, *EXPECTED_WINDOWS_CORE_TESTS, *EXPECTED_INPUT_BRIDGE_TESTS):
            exact = []
            def fake(command, **kwargs):
                if command[0] == "git":
                    output = "a" * 40
                elif command[-1] == "--list":
                    if "xenterm-bridge" not in command:
                        tests = gate.TUNNEL_TESTS + gate.SETTINGS_TESTS + EXPECTED_INPUT_CORE_TESTS + EXPECTED_ENCODING_TESTS + EXPECTED_WINDOWS_CORE_TESTS
                    elif "--lib" in command:
                        tests = EXPECTED_INPUT_BRIDGE_TESTS
                    else:
                        tests = gate.BRIDGE_TESTS
                    output = "\n".join(f"{name}: test" for name in tests if name != missing)
                else:
                    name = command[command.index("--") - 1]
                    exact.append(name)
                    output = f"test {name} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
                return subprocess.CompletedProcess(command, 0, output, "")
            with self.subTest(missing=missing):
                with patch.object(sys, "platform", "win32"), contextlib.redirect_stdout(io.StringIO()), self.assertRaisesRegex(RuntimeError, "not discovered"):
                    gate.run_regressions("a" * 40, runner=fake)
                if missing in (*EXPECTED_INPUT_CORE_TESTS, *EXPECTED_ENCODING_TESTS, *EXPECTED_WINDOWS_CORE_TESTS):
                    self.assertEqual(exact, [])
                else:
                    self.assertEqual(len(exact), 45)

    def test_each_auth_lifecycle_case_rejects_zero_ignored_or_wrong_name(self):
        # These are the eight new frontend cases and the new bridge-library case.
        names = (*EXPECTED_INPUT_CORE_TESTS[-8:], EXPECTED_INPUT_BRIDGE_TESTS[-1])
        self.assertEqual(len(set(names)), 9)
        for rejected in names:
            for fault in ("zero", "ignored", "wrong_name"):
                exact = []
                def fake(command, **kwargs):
                    if command[0] == "git":
                        output = "a" * 40
                    elif command[-1] == "--list":
                        if "xenterm-bridge" not in command:
                            tests = gate.TUNNEL_TESTS + gate.SETTINGS_TESTS + EXPECTED_INPUT_CORE_TESTS + EXPECTED_ENCODING_TESTS + EXPECTED_WINDOWS_CORE_TESTS
                        elif "--lib" in command:
                            tests = EXPECTED_INPUT_BRIDGE_TESTS
                        else:
                            tests = gate.BRIDGE_TESTS
                        output = "\n".join(f"{name}: test" for name in tests)
                    else:
                        name = command[command.index("--") - 1]
                        exact.append(name)
                        reported = "synthetic_wrong_test" if name == rejected and fault == "wrong_name" else name
                        passed = 0 if name == rejected and fault in ("zero", "ignored") else 1
                        ignored = 1 if name == rejected and fault == "ignored" else 0
                        output = (f"test {reported} ... {'ignored' if ignored else 'ok'}\n"
                                  f"test result: ok. {passed} passed; 0 failed; {ignored} ignored; 0 measured; 0 filtered out")
                    return subprocess.CompletedProcess(command, 0, output, "")
                with self.subTest(name=rejected, fault=fault):
                    with patch.object(sys, "platform", "win32"), contextlib.redirect_stdout(io.StringIO()) as captured, self.assertRaises(RuntimeError):
                        gate.run_regressions("a" * 40, runner=fake)
                    self.assertEqual(exact[-1], rejected)
                    self.assertNotIn("MAINTENANCE_REGRESSIONS_PASS", captured.getvalue())

    def test_non_windows_core_does_not_require_or_execute_conpty(self):
        for platform in ("linux", "darwin"):
            exact = []
            def fake(command, **kwargs):
                if command[0] == "git":
                    output = "a" * 40
                elif command[-1] == "--list":
                    if "xenterm-bridge" not in command:
                        tests = gate.TUNNEL_TESTS + gate.SETTINGS_TESTS + EXPECTED_INPUT_CORE_TESTS + EXPECTED_ENCODING_TESTS
                    elif "--lib" in command:
                        tests = EXPECTED_INPUT_BRIDGE_TESTS
                    else:
                        tests = gate.BRIDGE_TESTS
                    output = "\n".join(f"{name}: test" for name in tests)
                else:
                    name = command[command.index("--") - 1]
                    exact.append(name)
                    output = f"test {name} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
                return subprocess.CompletedProcess(command, 0, output, "")
            with self.subTest(platform=platform), patch.object(sys, "platform", platform), contextlib.redirect_stdout(io.StringIO()) as captured:
                gate.run_regressions("a" * 40, runner=fake)
                self.assertEqual(exact, [*gate.TUNNEL_TESTS, *gate.SETTINGS_TESTS, *EXPECTED_INPUT_CORE_TESTS, *EXPECTED_ENCODING_TESTS,
                                         *gate.BRIDGE_TESTS, *EXPECTED_INPUT_BRIDGE_TESTS])
                self.assertNotIn(EXPECTED_WINDOWS_CORE_TESTS[0], exact)
                self.assertIn("suite=rust count=46", captured.getvalue())
                self.assertIn(f"PLATFORM_EXCLUDED platform={platform} requires=win32 name={EXPECTED_WINDOWS_CORE_TESTS[0]}", captured.getvalue())

    def test_each_input_dart_case_requires_a_visible_non_skipped_first_pass(self):
        valid = self.flutter_output(names=EXPECTED_INPUT_DART_TESTS)
        gate.verify_flutter_results(valid, EXPECTED_INPUT_DART_TESTS)
        for name in EXPECTED_INPUT_DART_TESTS:
            for field in ("omit", "skipped", "failed"):
                with self.subTest(name=name, field=field), self.assertRaises(RuntimeError):
                    gate.verify_flutter_results(self.flutter_output(names=EXPECTED_INPUT_DART_TESTS, **{field: name}), EXPECTED_INPUT_DART_TESTS)
            for field in ("hidden", "retried"):
                events = [json.loads(line) for line in valid.splitlines()]
                events[2 * EXPECTED_INPUT_DART_TESTS.index(name) + 1][field] = True
                with self.subTest(name=name, field=field), self.assertRaises(RuntimeError):
                    gate.verify_flutter_results("\n".join(json.dumps(event) for event in events), EXPECTED_INPUT_DART_TESTS)

    def test_input_dart_duplicates_and_fake_success_cannot_pass(self):
        valid = self.flutter_output(names=EXPECTED_INPUT_DART_TESTS)
        events = [json.loads(line) for line in valid.splitlines()]
        duplicates = (
            {"type": "testStart", "test": {"id": 99, "name": EXPECTED_INPUT_DART_TESTS[0]}},
            {"type": "testStart", "test": {"id": 0, "name": EXPECTED_INPUT_DART_TESTS[1]}},
            events[1],
            {"type": "done", "success": True},
            {"type": "error", "testID": 0, "error": "synthetic late error"},
        )
        for duplicate in duplicates:
            with self.subTest(duplicate=duplicate), self.assertRaises(RuntimeError):
                gate.verify_flutter_results(valid + "\n" + json.dumps(duplicate), EXPECTED_INPUT_DART_TESTS)
        for output in ("All tests passed!", self.flutter_output(names=()),
                       self.flutter_output(names=EXPECTED_INPUT_DART_TESTS, done=False)):
            with self.subTest(output=output), self.assertRaises(RuntimeError):
                gate.verify_flutter_results(output, EXPECTED_INPUT_DART_TESTS)
        with self.assertRaisesRegex(RuntimeError, "duplicate"):
            gate.verify_flutter_results(valid, EXPECTED_INPUT_DART_TESTS + EXPECTED_INPUT_DART_TESTS[:1])

    def test_flutter_groups_cannot_make_up_for_each_others_missing_cases(self):
        for names, other in ((gate.DART_TESTS, EXPECTED_INPUT_DART_TESTS),
                             (EXPECTED_INPUT_DART_TESTS, gate.DART_TESTS)):
            # Even an equal total of passing cases from the wrong file cannot
            # replace one required exact name in this independently run group.
            replacement = names[1:] + other[:1]
            with self.subTest(names=names), self.assertRaisesRegex(RuntimeError, "not discovered"):
                gate.verify_flutter_results(self.flutter_output(names=replacement), names)

    def test_reconnect_dart_names_cannot_be_replaced_by_other_successes(self):
        # A passing result with another name cannot substitute for any new case.
        for name in EXPECTED_INPUT_DART_TESTS[-5:]:
            names = tuple("synthetic_wrong_reconnect_case" if item == name else item
                          for item in EXPECTED_INPUT_DART_TESTS)
            with self.subTest(name=name), self.assertRaisesRegex(RuntimeError, "not discovered"):
                gate.verify_flutter_results(self.flutter_output(names=names), EXPECTED_INPUT_DART_TESTS)

    def test_flutter_runner_rejects_a_missing_or_skipped_input_case(self):
        for name in EXPECTED_INPUT_DART_TESTS[:1] + EXPECTED_INPUT_DART_TESTS[-5:]:
            for fault in ("omit", "skipped", "failed"):
                def fake(command, **kwargs):
                    if command[0] == "git":
                        output = "a" * 40
                    elif "test/terminal_input_ownership_test.dart" in command:
                        output = self.flutter_output(names=EXPECTED_INPUT_DART_TESTS, **{fault: name})
                    else:
                        output = self.flutter_output()
                    return subprocess.CompletedProcess(command, 0, output, "")
                with self.subTest(name=name, fault=fault), contextlib.redirect_stdout(io.StringIO()), self.assertRaises(RuntimeError):
                    gate.run_regressions("a" * 40, suite="flutter", runner=fake)


class EncodingGateTests(unittest.TestCase):
    # Independent product-test names above are the oracle, not gate.ENCODING_TESTS.
    def simulate(self, platform="win32", rejected=None, fault=None):
        commands = []
        core_names = (*gate.TUNNEL_TESTS, *gate.SETTINGS_TESTS,
                      *EXPECTED_INPUT_CORE_TESTS, *EXPECTED_ENCODING_TESTS)
        if platform == "win32":
            core_names += EXPECTED_WINDOWS_CORE_TESTS
        groups = ((EXPECTED_CORE_TARGET, core_names),
                  (EXPECTED_SETTINGS_TARGET, gate.BRIDGE_TESTS),
                  (EXPECTED_BRIDGE_LIB_TARGET, EXPECTED_INPUT_BRIDGE_TESTS))

        def fake(command, **kwargs):
            commands.append(command)
            returncode = 0
            if command == ["git", "rev-parse", "HEAD"]:
                output = "a" * 40
            elif command[-2:] == ["--", "--list"]:
                target = tuple(command[:-2])
                names = dict(groups)[target]
                output = "\n".join(f"{name}: test" for name in names
                                   if not (fault == "missing_inventory" and name == rejected))
            else:
                name = command[command.index("--") - 1]
                output = (f"test {name} ... ok\n"
                          "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out")
                if name == rejected:
                    if fault == "zero":
                        output = "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
                    elif fault == "ignored":
                        output = (f"test {name} ... ignored\n"
                                  "test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out")
                    elif fault == "wrong_name":
                        output = output.replace(name, "synthetic_wrong_encoding_test")
                    elif fault == "failure":
                        output = (f"test {name} ... FAILED\n"
                                  "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out")
                        returncode = 101
                    elif fault == "failure_with_success_text":
                        returncode = 101
                    elif fault == "not_executed":
                        output = ""
            return subprocess.CompletedProcess(command, returncode, output, "")

        # No external command may escape the injected fake, even after refactors.
        with patch.object(subprocess, "Popen", side_effect=AssertionError("external process forbidden")):
            with patch.object(sys, "platform", platform), contextlib.redirect_stdout(io.StringIO()) as captured:
                if fault:
                    with self.assertRaises(RuntimeError):
                        gate.run_regressions("a" * 40, runner=fake)
                    self.assertNotIn("MAINTENANCE_REGRESSIONS_PASS", captured.getvalue())
                else:
                    gate.run_regressions("a" * 40, runner=fake)
        if fault == "missing_inventory":
            self.assertFalse(any("--exact" in command for command in commands))
        elif fault:
            self.assertEqual(commands[-1], [*EXPECTED_CORE_TARGET, rejected, "--", "--exact", "--test-threads=1", "--nocapture"])
        else:
            expected = [["git", "rev-parse", "HEAD"]]
            for target, names in groups:
                expected.append([*target, "--", "--list"])
                expected.extend([*target, name, "--", "--exact", "--test-threads=1", "--nocapture"]
                                for name in names)
            self.assertEqual(commands, expected)
            self.assertIn(f"suite=rust count={47 if platform == 'win32' else 46}", captured.getvalue())

    def test_encoding_commands_and_existing_scope_are_exact(self):
        self.assertEqual(gate.ENCODING_TESTS, EXPECTED_ENCODING_TESTS)
        for platform in ("win32", "linux", "darwin"):
            with self.subTest(platform=platform):
                self.simulate(platform)

    def test_each_encoding_result_fails_closed(self):
        for name in EXPECTED_ENCODING_TESTS:
            for fault in ("zero", "ignored", "wrong_name", "failure",
                          "failure_with_success_text", "not_executed"):
                with self.subTest(name=name, fault=fault):
                    self.simulate(rejected=name, fault=fault)

    def test_each_encoding_inventory_entry_is_required(self):
        for name in EXPECTED_ENCODING_TESTS:
            with self.subTest(name=name):
                self.simulate(rejected=name, fault="missing_inventory")

    def test_oracle_rejects_omission_or_misspelled_selection(self):
        # Prove the command oracle detects silent non-scheduling and filter drift.
        for name in EXPECTED_ENCODING_TESTS:
            for replacement in (None, "synthetic_misspelled_encoding_filter"):
                groups = tuple((target, tuple(replacement if item == name else item
                                              for item in names if item != name or replacement is not None))
                               for target, names in gate.RUST_GROUPS)
                with self.subTest(name=name, replacement=replacement):
                    with patch.object(gate, "RUST_GROUPS", groups):
                        with self.assertRaises((AssertionError, RuntimeError)):
                            self.simulate()


if __name__ == "__main__":
    unittest.main()
