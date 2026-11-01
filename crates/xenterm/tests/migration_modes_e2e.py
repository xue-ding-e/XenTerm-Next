"""Stable-ID imports and additive native sync using only synthetic local profiles.

Run with --exe target/debug/xenterm. Python's standard library is sufficient;
no SSH server, export/exec CLI extension, GUI, user profile or real key is used.
"""
import argparse
from contextlib import closing
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile


# Public MeatShell v1 fixture, shared with config_import_e2e.py. Portable
# encryption is obfuscation, not source authentication: import trusted files only.
PORTABLE_PASSWORD = "enc:exp:v1:AAAAAAAAAAAAAAAAFeRqZmIDNa2U57LDoinkeMgDVXvorTTv3qrwXh1pSN4M7bdXvSrE"
SECRET = "synthetic-only-password"


def session(name, **overrides):
    value = dict(id=name, name=name, host=f"{name}.invalid", port=22,
                 user="fixture", auth="password", password=SECRET,
                 kind="ssh", group="fixture group")
    value.update(overrides)
    return value


def snapshot(profile):
    with closing(sqlite3.connect(profile / "sessions.db")) as db:
        return {name: db.execute(query).fetchall() for name, query in (
            ("meta", "SELECT key,value FROM meta ORDER BY key"),
            ("sessions", "SELECT ordinal,id,data FROM sessions ORDER BY ordinal,id"),
            ("history", "SELECT seq,command FROM command_history ORDER BY seq"))}


def rows(profile):
    return [json.loads(row[2]) for row in snapshot(profile)["sessions"]]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", required=True, type=Path)
    exe = parser.parse_args().exe.resolve()
    env = dict(os.environ)
    env.pop("XENTERM_DATA_DIR", None)
    env.pop("MEATSHELL_DATA_DIR", None)
    with tempfile.TemporaryDirectory(prefix="xenterm-migration-modes-") as directory:
        root = Path(directory)

        def run(profile, *args, ok=True, input=None):
            result = subprocess.run([str(exe), "--data-dir", str(profile), *args],
                                    input=input, capture_output=True, text=True,
                                    timeout=30, env=env)
            assert (result.returncode == 0) == ok, (result.returncode, result.stdout, result.stderr)
            assert SECRET not in result.stdout + result.stderr, "credential leaked"
            return json.loads(result.stdout) if ok else result

        def cli(profile, *args, ok=True):
            return run(profile, "cli", *args, ok=ok)

        external_key = root / "unavailable-external-key"
        source_sessions = [
            session("outer"),
            session("inner", auth="key", private_key_path=str(external_key),
                    jump_session_id="outer"),
            session("target", private_key_inline=SECRET,
                    jump_session_id="inner", jump_session_ids=["outer", "inner"]),
        ]
        source_sessions.append(dict(source_sessions[2], id="equal-alias"))
        source = root / "source"
        source.mkdir()
        (source / "sessions.json").write_text(json.dumps(dict(
            sessions=source_sessions, defaults_rev=999)), encoding="utf-8")
        cli(source, "sessions", "--json")
        export = root / "portable.json"
        portable_sessions = [dict(item, password=PORTABLE_PASSWORD) for item in source_sessions]
        for item in portable_sessions:
            if item.get("private_key_inline"):
                item["private_key_inline"] = PORTABLE_PASSWORD
        export.write_text(json.dumps(dict(meatshell_export=1, sessions=portable_sessions)), encoding="utf-8")

        destination = root / "independent"
        destination.mkdir()
        (destination / "sessions.json").write_text(json.dumps(dict(
            sessions=[], defaults_rev=999, command_history=["synthetic history"],
            mcp_enabled=True, mcp_use_saved_credentials=True, mcp_allow_commands=False,
            mcp_allow_file_transfers=True)), encoding="utf-8")
        cli(destination, "sessions", "--json")
        before = snapshot(destination)
        assert cli(destination, "import", str(export), "--preserve-ids", "--dry-run", "--json") == dict(added=4, skipped=0, dry_run=True)
        assert snapshot(destination) == before
        assert cli(destination, "import", str(export), "--preserve-ids", "--json") == dict(added=4, skipped=0, dry_run=False)
        assert {item["id"] for item in rows(destination)} == {"outer", "inner", "target", "equal-alias"}
        assert cli(destination, "import", str(export), "--preserve-ids", "--json") == dict(added=0, skipped=4, dry_run=False)
        target = next(item for item in rows(destination) if item["id"] == "target")
        assert target["jump_session_id"] == "inner" and target["jump_session_ids"] == ["outer", "inner"]
        assert target["password"].startswith("enc:v1:") and target["private_key_inline"].startswith("enc:v1:")
        assert (source / "secret.key").read_bytes() != (destination / "secret.key").read_bytes()

        conflict = root / "conflicting.json"
        incoming = json.loads(export.read_text())
        incoming["sessions"][2]["host"] = "different.invalid"
        conflict.write_text(json.dumps(incoming), encoding="utf-8")
        before = snapshot(destination)
        for flags in ([], ["--dry-run"]):
            cli(destination, "import", str(conflict), "--preserve-ids", *flags, "--json", ok=False)
            assert snapshot(destination) == before
        print("PASS: stable-ID preview/apply retains aliases/routes, rekeys credentials and rejects conflicts atomically")

        current = rows(destination)
        update = dict(next(item for item in current if item["id"] == "target"),
                      note="synthetic updated note", allow_secret_reveal=True)
        added = dict(next(item for item in current if item["id"] == "outer"),
                     id="native-added", name="Native added", allow_secret_reveal=True)
        native = root / "native.json"
        native.write_text(json.dumps(dict(xenterm_export=1, sessions=[update, added],
                                         mcp_allow_commands=True, known_hosts=["ignored.invalid"])), encoding="utf-8")
        before = snapshot(destination)
        assert cli(destination, "sync-native", str(native), "--dry-run", "--json") == dict(updated=1, added=1, dry_run=True)
        assert snapshot(destination) == before
        # A second-row failure must roll back the first update as well.
        with closing(sqlite3.connect(destination / "sessions.db")) as db, db:
            db.executescript("CREATE TRIGGER fail_new BEFORE INSERT ON sessions WHEN NEW.id='native-added' BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;")
        cli(destination, "sync-native", str(native), "--json", ok=False)
        assert snapshot(destination) == before
        with closing(sqlite3.connect(destination / "sessions.db")) as db, db:
            db.executescript("DROP TRIGGER fail_new;")
        assert cli(destination, "sync-native", str(native), "--json") == dict(updated=1, added=1, dry_run=False)
        after = snapshot(destination)
        # Session transactions advance the internal commit stamp; application
        # settings and command history must still be preserved exactly.
        assert dict(after["meta"])["settings"] == dict(before["meta"])["settings"]
        assert after["history"] == before["history"]
        after_rows = {item["id"]: item for item in rows(destination)}
        assert set(after_rows) == {"outer", "inner", "target", "equal-alias", "native-added"}
        assert after_rows["target"]["note"] == "synthetic updated note"
        assert after_rows["target"]["allow_secret_reveal"] is False
        assert after_rows["native-added"]["allow_secret_reveal"] is False
        assert after_rows["inner"]["private_key_path"] == str(external_key)
        assert not external_key.exists(), "Migration must not create external key files"
        for row in before["sessions"]:
            if row[1] != "target":
                assert row in after["sessions"]
        assert cli(destination, "sync-native", str(native), "--json") == dict(updated=0, added=0, dry_run=False)
        assert snapshot(destination) == after
        print("PASS: native preview, changed-ID-only update, no deletion/settings/trust/key migration, consent reset, rollback and retry")

        # Native ciphertext from another profile must not be copied blindly.
        foreign = root / "foreign.json"
        foreign.write_text(json.dumps(dict(sessions=rows(source))), encoding="utf-8")
        cli(destination, "sync-native", str(foreign), "--json", ok=False)
        assert snapshot(destination) == after
        for version in (2, None, "1"):
            invalid = root / "version.json"
            invalid.write_text(json.dumps(dict(xenterm_export=version, sessions=[])), encoding="utf-8")
            for command in ("import", "sync-native"):
                cli(destination, command, str(invalid), "--json", ok=False)
                assert snapshot(destination) == after
        print("PASS: foreign native keys and invalid portable alias versions fail without partial changes")

        # Existing CLI/MCP read APIs still resolve the preserved external ID;
        # no network connection or command execution is needed for this check.
        assert cli(destination, "session", "target", "--json")["id"] == "target"
        request = dict(jsonrpc="2.0", id=1, method="tools/call", params=dict(
            name="get_session", arguments=dict(session_id="target")))
        result = run(destination, "mcp", "serve", input=json.dumps(request) + "\n")
        assert result["result"]["structuredContent"]["id"] == "target"
        assert snapshot(destination) == after
        print("PASS: preserved IDs remain usable by existing CLI/MCP readers without changing the profile")


if __name__ == "__main__":
    main()
