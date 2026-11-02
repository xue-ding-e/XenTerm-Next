"""Run reviewed Flutter maintenance regressions; zero/ignored tests fail closed."""

import argparse
import json
from pathlib import Path
import re
import subprocess


TUNNEL_TESTS = (
    "core::tunnel::runtime_key_tests::identity_ignores_labels_but_preserves_destinations_and_bind_scope",
    "ssh::ssh::runtime_tunnel_tests::repeated_start_stop_reuses_one_row_and_one_owner",
    "ssh::ssh::runtime_tunnel_tests::failed_bind_is_one_retryable_row_not_running",
    "ssh::ssh::runtime_tunnel_tests::different_destination_conflicts_without_replacing_owner",
    "ssh::ssh::runtime_tunnel_tests::readiness_and_stop_wait_for_acknowledgement",
    "ssh::ssh::runtime_tunnel_tests::a_reconnected_session_does_not_reset_runtime_revisions",
    "ssh::ssh::runtime_tunnel_tests::a_listener_failure_after_readiness_updates_the_existing_row",
    "tunnel::forward::listener_lifecycle_tests::first_start_and_restart_execute_the_same_normalized_destination",
    "tunnel::forward::listener_lifecycle_tests::completed_clients_are_reaped_before_an_already_ready_accept_backlog",
    "tunnel::forward::listener_lifecycle_tests::an_external_listener_is_reported_busy_and_never_adopted_or_closed",
    "tunnel::forward::listener_lifecycle_tests::stop_drains_accepted_clients_before_same_port_restart",
    "tunnel::forward::listener_lifecycle_tests::dropping_owner_requests_listener_and_client_cleanup",
    "frontend::runtime_tunnel_bridge_tests::closed_settles_live_rows_and_keeps_failure_details_in_the_existing_json_contract",
)
SETTINGS_TESTS = (
    "config::config::settings_update_tests::conflict_restores_raw_settings_and_prior_dirty_state_without_changing_baseline",
    "config::config::settings_update_tests::storage_failure_does_not_leak_failed_values_into_a_later_save",
    "config::config::settings_update_tests::successful_partial_update_keeps_omitted_raw_fields_and_advances_baseline",
)
BRIDGE_TESTS = (
    "failed_settings_save_keeps_bridge_projection_and_external_disk_unchanged",
)
DART_TESTS = (
    "runtime loader reports an unavailable native source instead of empty rows",
    "runtime loader accepts a legitimate empty native snapshot",
    "runtime loader preserves native failure details and owner identity",
    "runtime JSON preserves failed status and owning session identity",
    "same runtime row follows its lifecycle without appending cards",
    "only running rows count as active and dynamic target is SOCKS5",
    "long errors remain complete at narrow and wide window widths",
    "slow reads serialize and coalesce ticks into a latest-state read",
    "a new session source cannot be overwritten by an old response",
    "an old source failure cannot mark a replacement source stale",
    "a pending read completing while minimized waits for wake to refresh",
    "a late response after dispose neither repaints nor polls again",
    "failed reads disclose stale state and retry on a later tick",
    "minimizing pauses reads and waking refreshes the final state",
)

CORE = ("cargo", "test", "--locked", "-p", "xenterm", "--lib")
BRIDGE = ("cargo", "test", "--locked", "-p", "xenterm-bridge", "--test", "settings")
RUST_GROUPS = ((CORE, TUNNEL_TESTS + SETTINGS_TESTS), (BRIDGE, BRIDGE_TESTS))


def checked(command, *, timeout, runner=subprocess.run, cwd=None):
    result = runner(command, capture_output=True, text=True, timeout=timeout, cwd=cwd)
    output = (result.stdout or "") + (result.stderr or "")
    print(output, end="" if output.endswith("\n") else "\n", flush=True)
    if result.returncode != 0:
        raise RuntimeError(f"command failed ({result.returncode}): {' '.join(command)}")
    return output


def verify_source(expected, actual):
    if not re.fullmatch(r"[0-9a-f]{40}", expected) or actual.strip() != expected:
        raise RuntimeError(f"source mismatch: expected {expected}, got {actual.strip()}")


def verify_inventory(output, expected):
    listed = set(re.findall(r"^([A-Za-z0-9_:]+): test$", output, re.MULTILINE))
    missing = set(expected) - listed
    if missing:
        raise RuntimeError("required regressions were not discovered: " + ", ".join(sorted(missing)))


def verify_single_pass(output, name):
    summaries = re.findall(
        r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out",
        output,
    )
    if len(summaries) != 1 or summaries[0][:4] != ("1", "0", "0", "0"):
        raise RuntimeError(f"{name}: expected exactly one non-ignored passing test")


def verify_flutter_results(output):
    """Check the machine reporter, not a substring such as 'All tests passed'."""
    starts = {}
    finished = {}
    done = []
    for line in output.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue  # Flutter may print dependency/VM startup lines first.
        if not isinstance(event, dict):
            continue
        if event.get("type") == "testStart":
            test = event.get("test", {})
            if test.get("name") in DART_TESTS:
                name = test["name"]
                if name in starts.values() or test["id"] in starts:
                    raise RuntimeError(f"duplicate Flutter regression: {name}")
                starts[test["id"]] = name
        elif event.get("type") == "testDone":
            test_id = event.get("testID")
            if test_id in finished:
                raise RuntimeError(f"duplicate Flutter result: {test_id}")
            finished[test_id] = event
        elif event.get("type") == "error":
            # The reporter can emit an async error after testDone(success).
            raise RuntimeError(f"Flutter reported an uncaught test error: {event.get('testID')}")
        elif event.get("type") == "done":
            done.append(event.get("success"))
    missing = set(DART_TESTS) - set(starts.values())
    if missing:
        raise RuntimeError("required Flutter regressions were not discovered: " + ", ".join(sorted(missing)))
    for test_id, name in starts.items():
        result = finished.get(test_id, {})
        if (result.get("result") != "success" or result.get("skipped") is not False
                or result.get("hidden") is True or result.get("retried") is True):
            raise RuntimeError(f"{name}: expected a visible, non-skipped first-pass Flutter test")
    if done != [True]:
        raise RuntimeError("Flutter machine reporter did not finish successfully")


def run_regressions(expected_source, *, suite="rust", runner=subprocess.run):
    actual = checked(["git", "rev-parse", "HEAD"], timeout=30, runner=runner)
    verify_source(expected_source, actual)
    if suite == "flutter":
        # Windows Flutter is a .bat launcher and needs cmd.exe; never use shell
        # interpolation of user input. The source id is verified above, not run.
        import os
        command = ["flutter", "test", "--reporter", "json", "test/runtime_tunnels_test.dart", "test/runtime_tunnel_loader_test.dart"]
        if os.name == "nt":
            command = ["cmd.exe", "/d", "/c", *command]
        output = checked(command, timeout=600, runner=runner, cwd=Path("apps/xenterm"))
        verify_flutter_results(output)
        count = len(DART_TESTS)
    else:
        count = 0
        for cargo, tests in RUST_GROUPS:
            inventory = checked([*cargo, "--", "--list"], timeout=1200, runner=runner)
            verify_inventory(inventory, tests)
            for name in tests:
                print(f"MAINTENANCE_REGRESSION_START {name}", flush=True)
                output = checked(
                    [*cargo, name, "--", "--exact", "--test-threads=1", "--nocapture"],
                    timeout=180,
                    runner=runner,
                )
                verify_single_pass(output, name)
                count += 1
    print(f"MAINTENANCE_REGRESSIONS_PASS source={expected_source} suite={suite} count={count}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--expected-source", required=True)
    parser.add_argument("--suite", choices=("rust", "flutter"), required=True)
    args = parser.parse_args()
    run_regressions(args.expected_source, suite=args.suite)


if __name__ == "__main__":
    main()
