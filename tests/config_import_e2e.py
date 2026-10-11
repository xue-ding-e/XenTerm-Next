"""CLI/stdio MCP regression checks against isolated, synthetic SQLite profiles.

Run: python tests/config_import_e2e.py --exe target/debug/xenterm
No external server, user profile or real credential is used.
"""
import argparse
from contextlib import closing
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile

# Public MeatShell v1 fixture; generated using its portable export format.
PORTABLE_PASSWORD = "enc:exp:v1:AAAAAAAAAAAAAAAAFeRqZmIDNa2U57LDoinkeMgDVXvorTTv3qrwXh1pSN4M7bdXvSrE"
SENTINEL = "synthetic-only-password"
PROXY_SENTINEL = "synthetic-proxy-secret%40%25:p@ss"


def session(name, **overrides):
    value = dict(id=name, name=name, host=f"{name}.invalid", port=22,
                 user="fixture", auth="password", password=SENTINEL,
                 kind="ssh", group="fixture group")
    value.update(overrides)
    return value


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", required=True, type=Path)
    exe = parser.parse_args().exe.resolve()
    with tempfile.TemporaryDirectory(prefix="xenterm-import-") as directory:
        root = Path(directory)
        profile = root / "profile"
        profile.mkdir()
        legacy = profile / "sessions.json"
        legacy.write_text(json.dumps(dict(sessions=[session("existing")], defaults_rev=999,
            mcp_enabled=True, mcp_use_saved_credentials=True, mcp_allow_commands=False,
            mcp_allow_file_transfers=True)), encoding="utf-8")
        export = root / "export.json"
        export.write_text(json.dumps(dict(meatshell_export=1, sessions=[
            session("target", password=PORTABLE_PASSWORD, jump_session_id="inner", jump_session_ids=["outer", "inner"],
                    proxy=f"fixture-user:{PROXY_SENTINEL}@127.0.0.1:1080"),
            session("inner", jump_session_id="outer"), session("outer")
        ])), encoding="utf-8")

        def run(args, *, input=None, ok=True):
            env = dict(os.environ)
            env.pop("MEATSHELL_DATA_DIR", None)
            env.pop("XENTERM_DATA_DIR", None)
            result = subprocess.run([str(exe), "--data-dir", str(profile), *args],
                                    input=input, capture_output=True, text=True, timeout=30, env=env)
            assert (result.returncode == 0) == ok, (result.returncode, result.stdout, result.stderr)
            assert SENTINEL not in result.stdout + result.stderr
            assert PROXY_SENTINEL not in result.stdout + result.stderr
            return result

        def cli(*args, ok=True):
            result = run(["cli", *args], ok=ok)
            return json.loads(result.stdout) if ok else result

        def mcp(arguments, allow=False, tool="import_sessions"):
            request = dict(jsonrpc="2.0", id=1, method="tools/call", params=dict(name=tool, arguments=arguments))
            result = run(["mcp", "serve", *(["--allow-config-import"] if allow else [])], input=json.dumps(request)+"\n")
            response = json.loads(result.stdout)
            return response.get("result", response)

        def snapshot():
            # SQLite's context manager ends the transaction but does not close the
            # connection. Close before spawning XenTerm so delayed GC cannot
            # checkpoint/remove its WAL during the read-only profile inspection.
            with closing(sqlite3.connect(profile / "sessions.db")) as connection, connection:
                return tuple(tuple(connection.execute(query).fetchall()) for query in (
                    "SELECT key,value FROM meta ORDER BY key", "SELECT ordinal,id,data FROM sessions ORDER BY ordinal,id",
                    "SELECT seq,command FROM command_history ORDER BY seq"))

        def config():
            with closing(sqlite3.connect(profile / "sessions.db")) as connection, connection:
                settings = json.loads(connection.execute("SELECT value FROM meta WHERE key='settings'").fetchone()[0])
                settings["sessions"] = [json.loads(row[0]) for row in connection.execute("SELECT data FROM sessions ORDER BY ordinal,id")]
                return settings

        cli("sessions", "--json")  # Complete legacy migration before measuring.
        before = snapshot()
        assert cli("import", str(export), "--dry-run", "--json") == dict(added=3, skipped=0, dry_run=True)
        assert snapshot() == before
        assert mcp(dict(local_path=str(export)))["structuredContent"] == dict(added=3, skipped=0, dry_run=True)
        assert snapshot() == before
        denied = mcp(dict(local_path=str(export), dry_run=False))
        assert denied["isError"] and "--allow-config-import" in denied["content"][0]["text"]
        assert snapshot() == before
        print("PASS: CLI/MCP previews and explicit MCP write permission")

        assert cli("import", str(export), "--json") == dict(added=3, skipped=0, dry_run=False)
        saved = config()
        assert saved["mcp_allow_commands"] is False
        by_name = {item["name"]: item for item in saved["sessions"]}
        assert by_name["existing"]["id"] == "existing"
        assert by_name["target"]["jump_session_ids"] == [by_name[name]["id"] for name in ("outer", "inner")]
        assert by_name["target"]["jump_session_id"] == by_name["inner"]["id"]
        assert by_name["inner"]["jump_session_id"] == by_name["outer"]["id"]
        assert by_name["target"]["password"].startswith("enc:v1:")
        assert SENTINEL not in json.dumps(saved)
        assert PROXY_SENTINEL not in json.dumps(saved)
        assert by_name["target"]["proxy"].startswith("fixture-user:enc:v1:")
        metadata = cli("sessions", "--json")
        assert all(item["has_saved_password"] for item in metadata["sessions"])
        assert all(item["group"] == "fixture group" for item in metadata["sessions"])
        detail = cli("session", by_name["target"]["id"], "--json")
        assert "password" not in detail and "private_key_inline" not in detail and "proxy" not in detail
        assert cli("import", str(export), "--json") == dict(added=0, skipped=3, dry_run=False)
        assert mcp(dict(local_path=str(export), dry_run=False), allow=True)["structuredContent"] == dict(added=0, skipped=3, dry_run=False)
        print("PASS: MeatShell export compatibility, secrets, lists, references and idempotence")

        distinct = root / "distinct.json"
        incoming = json.loads(export.read_text())
        incoming["sessions"].append(dict(incoming["sessions"][0], id="target-alias", name="Intentional alias"))
        distinct.write_text(json.dumps(incoming))
        assert cli("import", str(distinct), "--json") == dict(added=1, skipped=3, dry_run=False)
        assert cli("import", str(distinct), "--json") == dict(added=0, skipped=4, dry_run=False)
        assert len([item for item in config()["sessions"] if item["host"] == "target.invalid"]) == 2
        print("PASS: distinct aliases survive without overwriting existing IDs")

        before = snapshot()
        broken = root / "broken.json"
        for value in ["{broken", json.dumps(dict(meatshell_export=99, sessions=[])),
                      json.dumps(dict(sessions=[session("bad", auth=SENTINEL)])),
                      json.dumps(dict(sessions=[session("bad", password="enc:exp:v1:broken")])),
                      json.dumps(dict(sessions=[session("bad", jump_session_id="missing")])),
                      json.dumps(dict(sessions=[session("repeated"), session("repeated")]))]:
            broken.write_text(value)
            cli("import", str(broken), "--json", ok=False)
            assert snapshot() == before
        cli("import", str(export), "--overwrite", ok=False)
        assert mcp(dict(local_path=str(export), dry_run="false"))["isError"]
        assert mcp(dict(local_path=str(export), overwrite=True))["isError"]
        print("PASS: invalid JSON/version/fields/IDs/secrets/routes do not partially import")

        native = root / "native.json"
        native.write_text(json.dumps(dict(sessions=[session("mcp")], mcp_allow_commands=True)))
        assert mcp(dict(local_path=str(native), dry_run=False), allow=True)["structuredContent"] == dict(added=1, skipped=0, dry_run=False)
        assert config()["mcp_allow_commands"] is False
        print("PASS: MCP apply and native sessions-only import preserve destination settings")

        legacy_options = root / "legacy-options.json"
        ignored = "SYNTHETIC_IGNORED_PRIVATE_VALUE"
        options = session("legacy-options", session_log="on", allow_secret_reveal=True,
                          rdp_domain=ignored)
        options[ignored] = ignored
        legacy_options.write_text(json.dumps(dict(meatshell_export=1, sessions=[options])))
        before = snapshot()
        preview = cli("import", str(legacy_options), "--dry-run", "--json")
        assert preview["added"] == 1 and len(preview["warnings"]) == 4
        assert ignored not in json.dumps(preview)
        assert {warning["field"] for warning in preview["warnings"]} == {"session_log", "allow_secret_reveal", "rdp_domain", "unknown"}
        assert snapshot() == before
        protocol_preview = mcp(dict(local_path=str(legacy_options)))["structuredContent"]
        assert protocol_preview == preview
        human_preview = run(["cli", "import", str(legacy_options), "--dry-run"])
        assert "Warning" in human_preview.stderr and ignored not in human_preview.stdout + human_preview.stderr
        applied = mcp(dict(local_path=str(legacy_options), dry_run=False), allow=True)["structuredContent"]
        assert applied["warnings"] == preview["warnings"] and applied["added"] == 1
        imported = next(item for item in config()["sessions"] if item["name"] == "legacy-options")
        assert "session_log" not in imported and imported["allow_secret_reveal"] is False
        assert any(w["code"] == "local_permission_reset" for w in preview["warnings"])
        print("PASS: unsupported legacy preferences are warned in preview/apply without exposing values")

        for permission in ("mcp_allow_file_transfers", "mcp_enabled"):
            with closing(sqlite3.connect(profile / "sessions.db")) as connection, connection:
                settings = config()
                settings["sessions"] = []
                settings[permission] = False
                connection.execute("UPDATE meta SET value=? WHERE key='settings'", [json.dumps(settings)])
            before = snapshot()
            denied = mcp(dict(local_path=str(export)), allow=True)
            assert denied.get("isError") or "error" in denied
            assert snapshot() == before
            assert cli("import", str(export), "--dry-run", "--json")["dry_run"] is True
        print("PASS: MCP file-transfer/server gates remain effective; explicit CLI is independent")

        assert (profile / "secret.key").is_file()
        if os.name == "posix":
            assert (profile / "secret.key").stat().st_mode & 0o777 == 0o600
        print("PASS: explicit profile owns its key and uses synthetic isolated storage")

        for name, payload in (("sessions.json", b'{"sessions": [invalid-json]}'),
                              ("sessions.db", b"synthetic-corrupt-database")):
            broken_profile = root / ("corrupt-" + name.replace(".", "-"))
            broken_profile.mkdir()
            source = broken_profile / name
            source.write_bytes(payload)
            failed = subprocess.run([str(exe), "--data-dir", str(broken_profile), "cli", "sessions", "--json"],
                                    capture_output=True, text=True, timeout=30)
            assert failed.returncode != 0
            assert source.read_bytes() == payload
            assert not list(broken_profile.glob("*.broken"))
            assert not (broken_profile / "secret.key").exists()
            assert not (broken_profile / "log").exists()
        print("PASS: corrupt explicit profiles fail without replacing the original files")

        def profile_files(directory):
            return {str(path.relative_to(directory)): path.read_bytes()
                    for path in directory.rglob("*") if path.is_file()}

        # Simulate an installed GUI profile. Startup rejection occurs before
        # tracing or key creation, including for DBs that use WAL journaling.
        for name, password, key in (
                ("native-keyring", "keyring:v1", None),
                ("native-encrypted", "enc:v1:synthetic-ciphertext", None),
                ("native-wrong-key", "enc:v1:synthetic-ciphertext", bytes([99]) * 32),
                ("db-keyring", "keyring:v1", None)):
            candidate = root / name
            candidate.mkdir()
            settings = dict(defaults_rev=999, mcp_enabled=True, sessions=[])
            imported = session("preflight", password=password)
            if name.startswith("db-"):
                with closing(sqlite3.connect(candidate / "sessions.db")) as connection, connection:
                    connection.execute("PRAGMA journal_mode=WAL")
                    connection.executescript("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT); CREATE TABLE sessions(ordinal INTEGER,id TEXT PRIMARY KEY,data TEXT); CREATE TABLE command_history(seq INTEGER PRIMARY KEY,command TEXT);")
                    connection.execute("INSERT INTO meta VALUES('settings',?)", [json.dumps(settings)])
                    connection.execute("INSERT INTO sessions VALUES(0,'preflight',?)", [json.dumps(imported)])
            else:
                settings["sessions"] = [imported]
                (candidate / "sessions.json").write_text(json.dumps(settings))
            if key is not None:
                (candidate / "secret.key").write_bytes(key)
            before = profile_files(candidate)
            failed = subprocess.run([str(exe), "--data-dir", str(candidate), "cli", "sessions", "--json"],
                                    capture_output=True, text=True, timeout=30)
            assert failed.returncode != 0 and "portable export" in failed.stderr
            assert password not in failed.stdout + failed.stderr
            assert profile_files(candidate) == before
            assert not (candidate / "log").exists()
        print("PASS: incompatible installed profiles fail before key, log, migration or SQLite writes")


if __name__ == "__main__":
    main()
