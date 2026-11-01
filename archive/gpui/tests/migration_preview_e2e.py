"""Full CLI migration previews must leave synthetic profiles byte-for-byte intact.

Run with --exe target/debug/xenterm. Only Python's standard library is needed;
all keys, sessions and databases belong to a temporary fixture directory.
"""
import argparse
from contextlib import closing
import json
import os
from pathlib import Path
import shutil
import sqlite3
import stat
import subprocess
import tempfile


SECRET = "synthetic-preview-credential"


def session(name, **overrides):
    return dict(id=name, name=name, host=f"{name}.invalid", port=22,
                user="fixture", auth="password", password=SECRET,
                kind="ssh", group="", **overrides)


def tree(path):
    """Include directory mtimes: create-then-remove is still a preview mutation."""
    result = {}
    for item in [path, *sorted(path.rglob("*"))]:
        info = item.lstat()
        value = item.read_bytes() if stat.S_ISREG(info.st_mode) else None
        result[str(item.relative_to(path))] = (info.st_mode, info.st_mtime_ns, value)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", required=True, type=Path)
    exe = parser.parse_args().exe.resolve()
    env = dict(os.environ)
    env.pop("XENTERM_DATA_DIR", None)
    env.pop("MEATSHELL_DATA_DIR", None)
    failures = []
    checks = 0
    with tempfile.TemporaryDirectory(prefix="xenterm-migration-preview-") as directory:
        root = Path(directory)

        def invoke(profile, *args, environment=False, cwd=None):
            command = [str(exe)]
            local_env = dict(env)
            if environment:
                local_env["XENTERM_DATA_DIR"] = str(profile)
            else:
                command += ["--data-dir", str(profile)]
            result = subprocess.run(command + ["cli", *args], env=local_env,
                                    cwd=cwd, capture_output=True, text=True, timeout=30)
            assert SECRET not in result.stdout + result.stderr, "credential leaked"
            return result

        source = root / "source"
        source.mkdir()
        (source / "sessions.json").write_text(json.dumps(dict(
            sessions=[session("existing")], defaults_rev=0,
            command_history=["fixture", "fixture"])), encoding="utf-8")
        initialized = invoke(source, "sessions", "--json")
        assert initialized.returncode == 0, initialized.stderr
        # The setup connection is closed before any filesystem measurement;
        # otherwise SQLite's final checkpoint can mutate the fixture itself.
        with closing(sqlite3.connect(source / "sessions.db")) as db:
            existing = json.loads(db.execute("SELECT data FROM sessions WHERE id='existing'").fetchone()[0])
        assert existing["password"].startswith("enc:v1:")

        inputs = root / "inputs"
        inputs.mkdir()
        payloads = {
            "add": dict(sessions=[session("added")]),
            "equal": dict(sessions=[session("existing")]),
            "conflict": dict(sessions=[session("existing") | dict(host="changed.invalid")]),
            "cipher": dict(sessions=[existing]),
            "invalid": dict(sessions=[session("bad") | dict(password="enc:v1:invalid")]),
            "version": dict(xenterm_export=2, sessions=[]),
        }
        paths = {}
        for name, payload in payloads.items():
            paths[name] = inputs / (name + ".json")
            paths[name].write_text(json.dumps(payload), encoding="utf-8")
        paths["broken"] = inputs / "broken.json"
        paths["broken"].write_text("{broken", encoding="utf-8")
        paths["missing"] = inputs / "missing.json"

        def make_profile(kind, base):
            profile = base / "nested" / "profile" if kind == "absent" else base / "profile"
            if kind == "absent":
                return profile
            if kind in ("db", "mismatch", "pending", "wal"):
                shutil.copytree(source, profile)
                # No pre-existing log or lock should conceal initialization.
                for name in ("error.log", "sessions.db.profile-lock"):
                    (profile / name).unlink(missing_ok=True)
            else:
                profile.mkdir()
            if kind == "key":
                (profile / "secret.key").write_bytes(bytes(range(32)))
            if kind in ("legacy", "empty-schema"):
                (profile / "sessions.json").write_text(json.dumps(dict(
                    sessions=[session("existing") | dict(group="System")],
                    defaults_rev=0, command_history=["fixture", "fixture"])), encoding="utf-8")
            if kind == "empty-schema":
                with closing(sqlite3.connect(profile / "sessions.db")) as db, db:
                    db.executescript("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);"
                                     "CREATE TABLE sessions(id TEXT PRIMARY KEY,ordinal INTEGER NOT NULL,data TEXT NOT NULL);"
                                     "CREATE TABLE command_history(seq INTEGER PRIMARY KEY AUTOINCREMENT,command TEXT NOT NULL);")
            if kind == "mismatch":
                (profile / "secret.key").write_bytes(bytes(range(32)))
            if kind == "bad-key":
                (profile / "secret.key").write_bytes(b"invalid synthetic key")
            if kind == "pending":
                (profile / "sessions.db.credential-journal").write_bytes(b"synthetic pending recovery")
            return profile

        def check(kind, mode, payload="add", *, ok=True, expected=None, extra=(), environment=False):
            nonlocal checks
            checks += 1
            label = f"{kind}/{mode}/{payload}/{checks}"
            base = root / f"case-{checks}"
            base.mkdir()
            profile = make_profile(kind, base)
            active_db = None
            if kind == "wal":
                # Keep a WAL reader/writer open so uncheckpointed committed
                # rows must be read without touching the real WAL or SHM.
                active_db = sqlite3.connect(profile / "sessions.db")
                active_db.execute("PRAGMA journal_mode=WAL")
                active_db.execute("PRAGMA wal_autocheckpoint=0")
                row = dict(existing, id="wal-added", name="wal-added", host="wal-added.invalid")
                active_db.execute("INSERT INTO sessions(id,ordinal,data) VALUES(?,?,?)", (row["id"], 1, json.dumps(row)))
                active_db.commit()
            before = tree(base)
            try:
                options = ["--preserve-ids"] if mode == "import" else []
                source_args = [] if payload is None else [str(paths[payload])]
                result = invoke(profile, mode, *source_args, *options,
                                "--dry-run", "--json", *extra, environment=environment)
                after = tree(base)
                changed = [name for name in sorted(before.keys() | after.keys())
                           if before.get(name) != after.get(name)]
                if changed:
                    failures.append(f"{label}: profile changed: {', '.join(changed)}")
                if (result.returncode == 0) != ok:
                    failures.append(f"{label}: unexpected exit {result.returncode}")
                if ok and result.returncode == 0:
                    value = json.loads(result.stdout)
                    if expected is None:
                        expected = dict(added=1, skipped=0) if mode == "import" else dict(updated=0, added=1)
                    if value != dict(expected, dry_run=True):
                        failures.append(f"{label}: unexpected preview counts")
            finally:
                if active_db is not None:
                    active_db.close()

        for mode in ("import", "sync-native"):
            for kind in ("absent", "empty", "key", "legacy", "empty-schema", "db", "wal"):
                check(kind, mode)
                for payload in ("broken", "missing", "invalid", "version"):
                    check(kind, mode, payload, ok=False)
                check(kind, mode, extra=("--unknown-option",), ok=False)
            for kind in ("legacy", "empty-schema", "db", "wal"):
                equal = dict(added=0, skipped=1) if mode == "import" else dict(updated=0, added=0)
                check(kind, mode, "equal", expected=equal)
                check(kind, mode, "conflict", ok=mode == "sync-native", expected=dict(updated=1, added=0))
            for kind in ("pending", "mismatch", "bad-key"):
                check(kind, mode, ok=False)
            check("db", mode, "cipher", expected=dict(added=0, skipped=1) if mode == "import" else dict(updated=0, added=0))
            check("key", mode, "cipher", ok=False)
            check("absent", mode, environment=True)
        # Missing source paths are invalid CLI syntax, but explicit local
        # previews must fail without initializing or logging into a profile.
        for mode in ("import", "sync-native"):
            for kind in ("absent", "legacy", "db"):
                check(kind, mode, None, ok=False)
        assert not failures, "\n".join(failures)
        print(f"PASS: {checks} full CLI migration previews preserve file bytes, mtimes and directories; legacy/WAL, conflicts, recovery and key failures covered")


if __name__ == "__main__":
    main()
