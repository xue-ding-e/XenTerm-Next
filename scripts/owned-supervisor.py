"""Bounded subprocess observer adapted from the reviewed typed-owner gate.
No claim of hostile-process containment; the GitHub job is the outer boundary.
"""
import os, re, selectors, signal, subprocess, time
from pathlib import Path
import json
LOG_LIMIT = 16 * 1024 * 1024
TOTAL_LOG_LIMIT = 48 * 1024 * 1024
TOTAL_LOG_BYTES = 0
RECORDS = []
ACTIVE = None
EVIDENCE = None
PHASE = "validation"
JOB_DEADLINE = None
FINALIZING = False
FINALIZE_RESERVE = 15
FIXTURE = None
check_disk_budgets = lambda: None
check_fixture_budget = lambda: None

def require(ok, message):
    if not ok: raise RuntimeError(message)

def write_json(name, value):
    (EVIDENCE / name).write_text(json.dumps(value, indent=2) + "\n")

def process_table():
    """PID identities include start ticks, preventing PID-reuse cleanup mistakes."""
    result = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            raw = (entry / "stat").read_text()
            fields = raw[raw.rindex(")") + 2:].split()
            result[int(entry.name)] = {
                "state": fields[0], "ppid": int(fields[1]),
                "pgid": int(fields[2]), "sid": int(fields[3]),
                "start": int(fields[19]),
            }
        except (FileNotFoundError, ProcessLookupError):
            pass
    return result


def owned_members(leader, known):
    table = process_table()
    # Same session/group plus previously observed descendants, including those
    # that changed group/session. This does not promise containment of hostile
    # descendants that escape before their first observation; runner job owns
    # that outer boundary. Owner parents spawn bounded actors in the inherited
    # session (no setsid/daemon); the separately bounded shell helper starts its
    # own group and waits/reaps it. These known fixtures are not a guarantee
    # against arbitrary unseen escaping descendants.
    changed = True
    while changed:
        changed = False
        for pid, item in table.items():
            parent = table.get(item["ppid"])
            known_parent = parent and known.get(item["ppid"]) == parent["start"]
            if item["sid"] == leader or item["pgid"] == leader or known_parent:
                if pid not in known:
                    known[pid] = item["start"]
                    changed = True
    return {
        pid: item for pid, item in table.items()
        if known.get(pid) == item["start"] and item["state"] != "Z"
    }


def observe_exit(child):
    """Observe without reaping, retaining the leader PID/session generation.

    A zombie leader cannot have its numeric PID reused. Keep it until the final
    ownership scan has finished, so matching sid/pgid cannot admit an unrelated
    new session merely because the old leader PID was reallocated.
    """
    result = os.waitid(os.P_PID, child.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
    if result is None:
        return None
    if result.si_code == os.CLD_EXITED:
        return result.si_status
    require(result.si_code in {os.CLD_KILLED, os.CLD_DUMPED}, "unexpected child wait status")
    return -result.si_status


def cleanup(child, known):
    """Bounded TERM then KILL, only verified owned identities; always reap leader."""
    for sig, seconds in [(signal.SIGTERM, 2), (signal.SIGKILL, 3)]:
        members = owned_members(child.pid, known)
        for pid, item in members.items():
            # Recheck identity immediately before signaling this specific PID.
            current = process_table().get(pid)
            if current and current["start"] == item["start"]:
                try:
                    fd = os.pidfd_open(pid)
                    try:
                        verified = process_table().get(pid)
                        if verified and verified["start"] == item["start"]:
                            signal.pidfd_send_signal(fd, sig)
                    finally:
                        os.close(fd)
                except ProcessLookupError:
                    pass
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            observe_exit(child)
            if not owned_members(child.pid, known):
                break
            time.sleep(0.05)
    # This final ownership scan occurs while leader PID is still reserved.
    # No sid/pgid scan is permitted after the unique final reap below.
    remaining = owned_members(child.pid, known)
    try:
        child.wait(timeout=1)
    except subprocess.TimeoutExpired:
        remaining["leader_not_reaped"] = {"pid": child.pid}
    return remaining


def interrupted(signum, _frame):
    raise RuntimeError(f"interrupted by signal {signum}")


def run(label, command, timeout, cwd, env, allowed_codes=(0,)):
    """Nonblocking pipe drain plus owned process observation with hard deadlines."""
    global ACTIVE, TOTAL_LOG_BYTES
    require(re.fullmatch(r"[a-z0-9-]+", label), "unsafe log label")
    output_paths = {key: EVIDENCE / f"{PHASE}-{label}.{key}.log" for key in ("stdout", "stderr")}
    require(not any(path.exists() for path in output_paths.values()), "duplicate command label")
    started = time.monotonic()
    requested_timeout = timeout
    if JOB_DEADLINE is not None:
        timeout = min(timeout, JOB_DEADLINE - time.monotonic() - (10 if FINALIZING else FINALIZE_RESERVE))
    require(timeout > 0, "job evidence/cleanup reserve reached")
    record = {"label": label, "argv": command, "timeout_seconds": timeout,
              "requested_timeout_seconds": requested_timeout,
              "exit_code": None, "reason": "not_started", "survivors": {},
              "log_bytes_seen": 0, "logs": {key: path.name for key, path in output_paths.items()}}
    child = None
    known = {}
    selector = selectors.DefaultSelector()
    files = {}
    reason = "exception"
    try:
        files = {key: path.open("wb") for key, path in output_paths.items()}
        child = subprocess.Popen(command, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 start_new_session=True)
        ACTIVE = child.pid
        record["pid"] = child.pid
        initial = process_table().get(child.pid)
        if initial:
            known[child.pid] = initial["start"]
        for stream, key in [(child.stdout, "stdout"), (child.stderr, "stderr")]:
            os.set_blocking(stream.fileno(), False)
            selector.register(stream, selectors.EVENT_READ, key)
        deadline = started + timeout
        while True:
            check_disk_budgets()
            if FIXTURE is not None:
                check_fixture_budget()
            exit_code = observe_exit(child)
            members = owned_members(child.pid, known)
            if exit_code is not None and members:
                reason = "surviving_descendants_after_parent_exit"
                break
            if time.monotonic() >= deadline:
                reason = "timeout"
                break
            for key, _mask in selector.select(timeout=0.05):
                try:
                    data = os.read(key.fileobj.fileno(), 65536)
                except BlockingIOError:
                    continue
                if not data:
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
                    continue
                remaining = min(LOG_LIMIT - record["log_bytes_seen"], TOTAL_LOG_LIMIT - TOTAL_LOG_BYTES)
                files[key.data].write(data[:max(0, remaining)])
                record["log_bytes_seen"] += len(data)
                TOTAL_LOG_BYTES += len(data)
            if record["log_bytes_seen"] > LOG_LIMIT or TOTAL_LOG_BYTES > TOTAL_LOG_LIMIT:
                reason = "log_soft_limit_exceeded"
                break
            if exit_code is not None and not selector.get_map():
                reason = "completed" if exit_code in allowed_codes else "nonzero_exit"
                break
    finally:
        try:
            if child is not None:
                try:
                    record["survivors"] = cleanup(child, known)
                    record["exit_code"] = child.returncode
                except BaseException as cleanup_error:
                    reason = "cleanup_failed"
                    record["survivors"] = {"unknown": "cleanup could not be confirmed"}
                    record["cleanup_error"] = f"{type(cleanup_error).__name__}: {cleanup_error}"
        finally:
            ACTIVE = None
            record["reason"] = reason
            record["duration_seconds"] = round(time.monotonic() - started, 3)
            for key in list(selector.get_map().values()):
                key.fileobj.close()
            selector.close()
            for file in files.values():
                file.close()
            RECORDS.append(record)
            write_json(f"{PHASE}-commands.json", RECORDS)
    require(reason == "completed" and not record["survivors"], f"{label}: {reason}; see command evidence")
    return output_paths["stdout"].read_bytes()

