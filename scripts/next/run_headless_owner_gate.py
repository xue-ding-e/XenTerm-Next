"""Immutable, headless-only validation; never builds the bridge or installers."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import shutil
import subprocess
import sys
import tempfile
import time


SUMMARY = re.compile(
    r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out"
)
CARGO = ["cargo", "test", "--locked", "-p", "xenterm", "--no-default-features", "--features", "headless", "--jobs", "2"]


class Cancelled(RuntimeError):
    pass


class Cancellation:
    def __init__(self):
        self.launching = False
        self.cleaning = False
        self.pending = None

    def handle(self, signum, _frame):
        self.pending = signum
        # Never raise from an asynchronous signal handler. Popen can have
        # created the child before returning its handle, and a repeated signal
        # could otherwise interrupt the cleanup exception handler itself.
        # communicate polls explicit safe checkpoints at most 0.5s apart.

    def checkpoint(self):
        if self.pending is not None:
            raise Cancelled(f"validation cancelled by signal {self.pending}")

    def __enter__(self):
        self.pending = None
        self.previous = {sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)}
        for sig in self.previous:
            signal.signal(sig, self.handle)
        return self

    def __exit__(self, exc_type, *_):
        for sig, handler in self.previous.items():
            signal.signal(sig, handler)
        if exc_type is None:
            self.checkpoint()


class Deadline:
    def __init__(self, seconds, clock=time.monotonic):
        self.clock = clock
        self.end = clock() + seconds

    def limit(self, stage_seconds):
        remaining = self.end - self.clock()
        if remaining <= 0:
            raise RuntimeError("global validation budget exhausted")
        return min(stage_seconds, remaining)


CANCELLATION = Cancellation()

# This controller is intentionally bound to one reviewed permission-fix candidate.
# Updating the candidate requires a new review, not a permissive file count.
REVIEWED_CONTRACT = {
  "parent_sha": "c48a493fdce3e46ae02a3d70a6502e10f2ec02f2",
  "parent_tree": "45c73465ab84742e20dbcf9f8345b00fce54791d",
  "baseline_sha": "fd5c0f8ceafb3975710a4c784bba820ddcefdb8b",
  "baseline_tree": "a337f6719e7e75ce49ab50dc72f99751fc85ef1b",
  "baseline_files": {
    "crates/xenterm/src/ssh/impls/headless_policy.rs": "18cbd8337a6eab41ff5a64ed8f801dcb34405805497f9a636828f437c5007727",
    "crates/xenterm/src/ssh/impls/known_hosts.rs": "fec380c2f3f1fc9444e4237353538bc3b9cd9179342fb839ae557b1495174ac8",
    "crates/xenterm/src/ssh/impls/ssh.rs": "189aeee13da0c92404b3a8ad5a5cf2aba60d5c4a8961ac388e59dbbc36b7040b",
    "crates/xenterm/src/ssh/mod.rs": "a739184f13924c881dedbeed20e721ee0f8392a3f4f26060ee85ad2203ccfcbd",
    "crates/xenterm/src/tunnel/impls/forward.rs": "f314b91dcba489baec5609bbd83514f511d77ef607f507bb5f2ee1754058542a",
    "crates/xenterm/src/tunnel/impls/owner.rs": "29fb107a97f4b299375f4670837df3bc392c9eb0d0bc2356616c0feed89311a5",
    "crates/xenterm/src/tunnel/impls/owner_tests.rs": "0b2bfa07d0a54eafcb52be9101cfe19f877a216b81cdc93523fe319ca69f899f",
    "crates/xenterm/src/tunnel/mod.rs": "a18fb31a33adc0cdd944b6a29e471e7562b27400d6194ae3219d0f7ca6d51e5c"
  },
  "parent_blobs": {
    "crates/xenterm/src/tunnel/impls/owner.rs": "8ad706bde337b8173917f344ae09ab672e8ac3ff",
    "crates/xenterm/src/tunnel/impls/owner_tests.rs": "ad687aab6ce74e42acd74c54881f521682f9f74f"
  },
  "files": {
    "crates/xenterm/src/tunnel/impls/owner.rs": "62ee32d615efc043652be3c43c0d22c466cde003c9eebb0f24f1ae12ab207b57",
    "crates/xenterm/src/tunnel/impls/owner_tests.rs": "6385da21af73536e2fc550418b9308c941672e7223540380f8aa88950261602a"
  },
  "new_tests": [
    "ssh::headless_policy::tests::captured_direct_never_falls_back_to_later_proxy_input",
    "ssh::headless_policy::tests::changed_trust_changes_private_reuse_identity",
    "ssh::headless_policy::tests::file_key_paths_never_count_as_reusable_credential_snapshots",
    "tunnel::owner::tests::unattended_bind_is_loopback_only",
    "tunnel::owner::tests::reuse_identity_ignores_ui_metadata_but_tracks_selected_route_and_auth",
    "tunnel::owner::tests::reuse_fingerprint_tracks_only_the_actual_outer_tcp_proxy",
    "tunnel::owner::tests::cancelled_start_rolls_back_listener_and_pending_proxy_transport",
    "tunnel::owner::tests::expired_start_rolls_back_without_ever_becoming_ready",
    "tunnel::owner::tests::owner_shutdown_cancels_a_registered_pending_start",
    "tunnel::owner::lifecycle_tests::headless_owner_three_node_lifecycle",
    "tunnel::owner::lifecycle_tests::stalled_start_does_not_block_another_handles_release",
    "tunnel::owner::lifecycle_tests::cancelled_or_timed_out_key_load_is_joined_before_release_ack",
    "tunnel::owner::lifecycle_tests::begin_registers_before_preparation_and_cancel_joins_late_result_without_binding",
    "tunnel::owner::lifecycle_tests::blocked_preparation_does_not_block_active_owner_release",
    "tunnel::owner::lifecycle_tests::terminal_retention_is_bounded_without_evicting_pending_or_unconfirmed_work",
    "tunnel::owner::lifecycle_tests::pending_entries_enforce_the_live_limit_and_shutdown_joins_them",
    "tunnel::owner::lifecycle_tests::begin_rejects_mismatched_preparation_before_binding_and_keeps_unattended_gate",
    "tunnel::owner::lifecycle_tests::panicked_operation_still_joins_its_owned_key_loader",
    "tunnel::owner::lifecycle_tests::expiring_preparation_retains_its_slot_until_the_job_is_joined",
    "tunnel::owner::lifecycle_tests::permission_profile_lock_must_not_stall_cancellation_timer",
    "tunnel::owner::lifecycle_tests::revoked_permission_during_preparation_must_not_bind_late_route",
    "tunnel::owner::lifecycle_tests::permission_read_observes_other_process_revocation_not_cached_store",
    "tunnel::owner::lifecycle_tests::owner_release_waits_for_blocked_permission_read",
    "tunnel::owner::lifecycle_tests::owner_shutdown_waits_for_blocked_permission_read",
    "tunnel::owner::lifecycle_tests::operation_panic_drains_in_flight_permission_reader_without_clean_ack"
  ],
  "isolated_tests": [
    "tunnel::owner::tests::cancelled_start_rolls_back_listener_and_pending_proxy_transport",
    "tunnel::owner::tests::expired_start_rolls_back_without_ever_becoming_ready",
    "tunnel::owner::tests::owner_shutdown_cancels_a_registered_pending_start",
    "tunnel::owner::lifecycle_tests::headless_owner_three_node_lifecycle",
    "tunnel::owner::lifecycle_tests::stalled_start_does_not_block_another_handles_release",
    "tunnel::owner::lifecycle_tests::cancelled_or_timed_out_key_load_is_joined_before_release_ack",
    "tunnel::owner::lifecycle_tests::begin_registers_before_preparation_and_cancel_joins_late_result_without_binding",
    "tunnel::owner::lifecycle_tests::blocked_preparation_does_not_block_active_owner_release",
    "tunnel::owner::lifecycle_tests::terminal_retention_is_bounded_without_evicting_pending_or_unconfirmed_work",
    "tunnel::owner::lifecycle_tests::pending_entries_enforce_the_live_limit_and_shutdown_joins_them",
    "tunnel::owner::lifecycle_tests::begin_rejects_mismatched_preparation_before_binding_and_keeps_unattended_gate",
    "tunnel::owner::lifecycle_tests::panicked_operation_still_joins_its_owned_key_loader",
    "tunnel::owner::lifecycle_tests::expiring_preparation_retains_its_slot_until_the_job_is_joined",
    "tunnel::owner::lifecycle_tests::permission_profile_lock_must_not_stall_cancellation_timer",
    "tunnel::owner::lifecycle_tests::revoked_permission_during_preparation_must_not_bind_late_route",
    "tunnel::owner::lifecycle_tests::permission_read_observes_other_process_revocation_not_cached_store",
    "tunnel::owner::lifecycle_tests::owner_release_waits_for_blocked_permission_read",
    "tunnel::owner::lifecycle_tests::owner_shutdown_waits_for_blocked_permission_read",
    "tunnel::owner::lifecycle_tests::operation_panic_drains_in_flight_permission_reader_without_clean_ack"
  ],
  "maintenance_core_tests": [
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
    "config::config::settings_update_tests::conflict_restores_raw_settings_and_prior_dirty_state_without_changing_baseline",
    "config::config::settings_update_tests::storage_failure_does_not_leak_failed_values_into_a_later_save",
    "config::config::settings_update_tests::successful_partial_update_keeps_omitted_raw_fields_and_advances_baseline"
  ],
  "owner_sha": "291b4f8548970fbcd4147c567ed3805321f5be14",
  "owner_tree": "66b6620dda42fd091d2e218cd6cbd7ea3f52cf21",
  "regression_blobs": {
    "crates/xenterm/src/tunnel/impls/owner_tests.rs": "436c27e96cddd51aed21ff1f14c5e5b8985207ed"
  }
}
PERMISSION_PREFIX = "tunnel::owner::lifecycle_tests::"
EXTERNAL_REVOCATION = PERMISSION_PREFIX + "permission_read_observes_other_process_revocation_not_cached_store"


def validate_manifest(manifest):
    if set(manifest) != set(REVIEWED_CONTRACT) | {"source_sha", "source_tree"}:
        raise RuntimeError("unexpected candidate manifest fields")
    for name, expected in REVIEWED_CONTRACT.items():
        if manifest[name] != expected:
            raise RuntimeError(f"reviewed candidate contract changed: {name}")
    for name in ("source_sha", "source_tree"):
        if not re.fullmatch(r"[0-9a-f]{40}", manifest[name]):
            raise RuntimeError("candidate must be bound before any source command")


def verify_required(output, name, manifest):
    required = manifest["new_tests"] + manifest["maintenance_core_tests"]
    if name not in required:
        raise RuntimeError("unreviewed exact test cannot establish coverage")
    # The external writer is a third real test process, not a stand-alone
    # permission regression: without its private marker that helper is inert.
    layers = 3 if name == EXTERNAL_REVOCATION else (2 if name in manifest["isolated_tests"] else 1)
    summaries = SUMMARY.findall(output)
    if len(summaries) != layers or any(row[:4] != ("1", "0", "0", "0") for row in summaries):
        raise RuntimeError("expected all exact non-ignored fixture layers to pass")




def verify_sha(expected, actual):
    if not re.fullmatch(r"[0-9a-f]{40}", expected) or expected != actual.strip():
        raise RuntimeError(f"unbound or mismatched source: {expected} != {actual.strip()}")


def verify_inventory(output, names):
    listed = re.findall(r"^([A-Za-z0-9_:]+): test$", output, re.MULTILINE)
    if len(names) != len(set(names)) or not set(names).issubset(listed):
        raise RuntimeError("duplicate or undiscovered required tests")


def verify_exact(output, isolated):
    summaries = SUMMARY.findall(output)
    # Isolated owner fixtures execute one child test plus one parent test. The
    # child completion marker is independently asserted by the parent fixture.
    expected = 2 if isolated else 1
    if len(summaries) != expected or any(row[:4] != ("1", "0", "0", "0") for row in summaries):
        raise RuntimeError("expected exact, non-ignored parent/child test passes")


def verify_full(output):
    summaries = SUMMARY.findall(output)
    if not summaries or not any(int(row[0]) for row in summaries) or any(row[1] != "0" for row in summaries):
        raise RuntimeError("full core suite produced no passing test evidence")
    print("FULL_CORE_SUMMARIES " + json.dumps(summaries), flush=True)


class ProcessFailed(RuntimeError):
    """An ordinary nonzero exit after owned-process cleanup was confirmed."""


def live_group_members(pgid):
    # Linux-only gate: zombies have no executable work or open descriptors.
    # Direct children are reaped below; orphan zombies belong to the runner's
    # reaper and must not be mistaken for a surviving background worker.
    if not Path("/proc/self/stat").is_file():
        raise RuntimeError("cleanup_unconfirmed: Linux process inventory unavailable")
    result = []
    for stat in Path("/proc").glob("[0-9]*/stat"):
        try:
            fields = stat.read_text().rsplit(")", 1)[1].split()
            if int(fields[2]) == pgid and fields[0] not in ("Z", "X"):
                result.append(int(stat.parent.name))
        except (FileNotFoundError, ProcessLookupError):
            pass
    return result


def cleanup_owned_group(process):
    # Even a successful parent can leave a descendant after closing stdout.
    # Never acknowledge completion until the entire owned group has stopped.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    output, _ = process.communicate(timeout=10)
    deadline = time.monotonic() + 2
    while live_group_members(process.pid):
        if time.monotonic() >= deadline:
            raise RuntimeError("cleanup_unconfirmed: owned process group remains live")
        time.sleep(0.01)
    return output


def checked(command, *, cwd, logs, label, timeout, env=None, deadline=None, cancellation=None):
    cancellation = cancellation or CANCELLATION
    cancellation.checkpoint()
    allowance = deadline.limit(timeout) if deadline is not None else timeout
    stage_end = time.monotonic() + allowance
    print("COMMAND " + json.dumps(command), flush=True)
    process = None
    try:
        cancellation.launching = True
        try:
            process = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE,
                                       stderr=subprocess.STDOUT, text=True, start_new_session=True)
        finally:
            cancellation.launching = False
        cancellation.checkpoint()
        while True:
            cancellation.checkpoint()
            remaining = stage_end - time.monotonic()
            if remaining <= 0:
                raise subprocess.TimeoutExpired(command, allowance)
            try:
                output, _ = process.communicate(timeout=min(0.5, remaining))
                break
            except subprocess.TimeoutExpired:
                # communicate is retry-safe: it retains the partial output.
                # Recheck both cancellation and the shared deadline next poll.
                pass
        cancellation.checkpoint()
    finally:
        cancellation.cleaning = True
        try:
            if process is not None:
                # Apply identical bounded cleanup on success, nonzero exit,
                # timeout and cancellation; repeated signals cannot interrupt it.
                try:
                    output = cleanup_owned_group(process)
                except BaseException as error:
                    raise RuntimeError("cleanup_unconfirmed: owned process group cleanup failed") from error
                (logs / f"{label}.log").write_text(output)
        finally:
            cancellation.cleaning = False
    cancellation.checkpoint()
    (logs / f"{label}.log").write_text(output)
    print(output, end="" if output.endswith("\n") else "\n", flush=True)
    if process.returncode:
        raise ProcessFailed(f"{label} failed: exit {process.returncode}")
    return output


def verify_source(source, manifest, logs, deadline=None):
    validate_manifest(manifest)
    def git(*args):
        return checked(["git", *args], cwd=source, logs=logs,
                       label="identity-" + args[0] + "-" + str(len(list(logs.iterdir()))), timeout=30, deadline=deadline).strip()
    verify_sha(manifest["source_sha"], git("rev-parse", "HEAD"))
    verify_sha(manifest["source_tree"], git("rev-parse", "HEAD^{tree}"))
    verify_sha(manifest["parent_sha"], git("rev-parse", "HEAD^1"))
    verify_sha(manifest["parent_tree"], git("rev-parse", "HEAD^1^{tree}"))
    verify_sha(manifest["owner_sha"], git("rev-parse", "HEAD^1^1"))
    verify_sha(manifest["owner_tree"], git("rev-parse", "HEAD^1^1^{tree}"))
    verify_sha(manifest["baseline_sha"], git("rev-parse", "HEAD^1^1^1"))
    verify_sha(manifest["baseline_tree"], git("rev-parse", "HEAD^1^1^1^{tree}"))
    for ref, expected_parent in (("HEAD", manifest["parent_sha"]), ("HEAD^1", manifest["owner_sha"]), ("HEAD^1^1", manifest["baseline_sha"])):
        if git("show", "-s", "--format=%P", ref).split() != [expected_parent]:
            raise RuntimeError("candidate lineage must retain exactly the reviewed parent")
    if sorted(git("diff", "--name-only", "HEAD^1", "HEAD").splitlines()) != sorted(manifest["files"]):
        raise RuntimeError("permission fix changed files outside the two reviewed owner files")
    if sorted(git("diff", "--name-only", "HEAD^1^1", "HEAD^1").splitlines()) != sorted(manifest["regression_blobs"]):
        raise RuntimeError("red regression commit changed files outside owner_tests.rs")
    for name, expected in manifest["regression_blobs"].items():
        verify_sha(expected, git("rev-parse", "HEAD^1:" + name))
    if sorted(git("diff", "--name-only", "HEAD^1^1^1", "HEAD^1^1").splitlines()) != sorted(manifest["parent_blobs"]):
        raise RuntimeError("parent changed files outside the two reviewed owner blobs")
    for name, expected in manifest["parent_blobs"].items():
        verify_sha(expected, git("rev-parse", "HEAD^1^1:" + name))
    # Compare raw object bytes, never decoded/stripped text. This retains the
    # original eight-file formatted source identity before the owner changes.
    for index, (name, expected) in enumerate(manifest["baseline_files"].items()):
        digest = checked(
            [sys.executable, "-c",
             "import hashlib,subprocess,sys; "
             "print(hashlib.sha256(subprocess.check_output(['git','show',sys.argv[1]])).hexdigest())",
             "HEAD^1^1^1:" + name],
            cwd=source, logs=logs, label=f"baseline-hash-{index}", timeout=30, deadline=deadline).strip()
        if digest != expected:
            raise RuntimeError(f"original eight-file identity changed: {name}")
    for name, expected in manifest["files"].items():
        if hashlib.sha256((source / name).read_bytes()).hexdigest() != expected:
            raise RuntimeError(f"reviewed permission-fix file changed: {name}")
    if git("status", "--porcelain"):
        raise RuntimeError("source checkout is dirty")
    print("FROZEN_SOURCE_VERIFIED " + manifest["source_sha"], flush=True)


def isolated_env(profile, source):
    allowed = ("PATH", "LD_LIBRARY_PATH", "LANG", "LC_ALL")
    env = {name: os.environ[name] for name in allowed if name in os.environ}
    home = Path.home()
    env["CARGO_HOME"] = os.environ.get("CARGO_HOME", str(home / ".cargo"))
    env["RUSTUP_HOME"] = os.environ.get("RUSTUP_HOME", str(home / ".rustup"))
    env["CARGO_TARGET_DIR"] = str(source / "target")
    env["RUSTUP_TOOLCHAIN"] = "1.99.1"
    for name in ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME",
                 "XDG_CACHE_HOME", "XDG_STATE_HOME", "XDG_RUNTIME_DIR", "TMP", "TEMP", "TMPDIR", "XENTERM_DATA_DIR"):
        path = profile / name
        path.mkdir(mode=0o700)
        env[name] = str(path)
    return env


def run_regressions(run, manifest, deadline, env):
    failures = []
    required = manifest["new_tests"] + manifest["maintenance_core_tests"]
    for index, name in enumerate(required):
        print("EXACT_TEST_START " + name, flush=True)
        try:
            output = run([*CARGO, "--offline", "--lib", name, "--", "--exact", "--test-threads=1", "--nocapture"],
                         f"exact-{index:02d}", 90, env)
        except ProcessFailed as error:
            # Only an ordinary test failure may continue. Cancellation, timeout,
            # exhausted budget or unconfirmed cleanup still stop immediately.
            failures.append(name)
            print("EXACT_TEST_FAILED " + json.dumps({"name": name, "reason": str(error)}), flush=True)
            continue
        verify_required(output, name, manifest)
    if deadline.limit(900) < 900:
        print("FULL_CORE_SKIPPED insufficient remaining shared budget for 900-second stage", flush=True)
        failures.append("full-core: insufficient remaining budget")
    else:
        try:
            output = run([*CARGO, "--offline", "--", "--test-threads=1"], "full-core", 900, env)
        except ProcessFailed as error:
            failures.append("full-core")
            print("FULL_CORE_FAILED " + str(error), flush=True)
        else:
            verify_full(output)
    if failures:
        raise RuntimeError("REGRESSION_FAILURES " + json.dumps(failures))


def run():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--logs", type=Path, required=True)
    parser.add_argument("--verify-only", action="store_true")
    args = parser.parse_args()
    # Less than the outer 50-minute step, leaving explicit cleanup/artifact
    # margin. Every command consumes this same budget, not a fresh allowance.
    deadline = Deadline(45 * 60)
    source, logs = args.source.resolve(), args.logs.resolve()
    logs.mkdir(parents=True, exist_ok=True)
    manifest = json.loads(args.manifest.read_text())
    verify_source(source, manifest, logs, deadline)
    if args.verify_only:
        return
    run = lambda cmd, label, timeout, env=None: checked(cmd, cwd=source, logs=logs, label=label, timeout=timeout, env=env, deadline=deadline)
    required = manifest["new_tests"] + manifest["maintenance_core_tests"]
    if len(manifest["new_tests"]) != 25 or len(manifest["maintenance_core_tests"]) != 16:
        raise RuntimeError("reviewed exact-test inventory changed")
    with tempfile.TemporaryDirectory(prefix="headless-owner-core-") as directory:
        env = isolated_env(Path(directory), source)
        # Resolve the official toolchain with the same sanitized environment
        # used for compilation and execution; no ambient auth/proxy variables.
        rustup = shutil.which("rustup", path=env.get("PATH"))
        if rustup is None:
            raise RuntimeError("official rustup is not available in the tool PATH")
        tool_root = (Path(env["RUSTUP_HOME"]) / "toolchains").resolve()
        tool_paths = {}
        for name in ("rustc", "cargo"):
            actual = Path(run([rustup, "which", "--toolchain", "1.99.1", name],
                              "resolve-" + name, 30, env).strip()).resolve()
            if not actual.is_relative_to(tool_root) or not actual.is_file():
                raise RuntimeError("resolved tool is outside the official Rust toolchain cache")
            tool_paths[name] = actual
            version = run([str(actual), "--version"], name, 30, env)
            if not version.startswith("rustc 1.99.1 " if name == "rustc" else "cargo 1.99."):
                raise RuntimeError("unexpected pinned Rust toolchain version")
        if tool_paths["rustc"].parent != tool_paths["cargo"].parent:
            raise RuntimeError("cargo and rustc must resolve from the same pinned toolchain")
        env["PATH"] = str(tool_paths["cargo"].parent) + os.pathsep + env.get("PATH", "")
        run([*CARGO, "--no-run"], "compile-core", 1500, env)
        inventory = run([*CARGO, "--offline", "--lib", "--", "--list"], "inventory", 120, env)
        verify_inventory(inventory, required)
        run_regressions(run, manifest, deadline, env)
    verify_source(source, manifest, logs, deadline)
    CANCELLATION.checkpoint()
    print(f"HEADLESS_OWNER_CORE_PASS source={manifest['source_sha']} new=25 maintenance_core=16", flush=True)


if __name__ == "__main__":
    with CANCELLATION:
        run()

