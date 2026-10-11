"""Portable CLI export/import, real loopback SSH and credential isolation.

Only generated test credentials and fixture-owned profiles are used. Requires
the same cryptography/paramiko dependencies as ssh_jump_chain_e2e.py.
"""
import argparse
import base64
import contextlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile

from cryptography.exceptions import InvalidTag
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305
from ssh_jump_chain_e2e import Fixture, PASSWORD, PASSPHRASE, session


EXPORT_KEY = b"meatshell.export.portable.key.01"


def decrypt(value, key, prefix):
    if not value:
        return value
    assert value.startswith(prefix)
    encoded = value[len(prefix):]
    blob = base64.urlsafe_b64decode(encoded + "=" * (-len(encoded) % 4))
    return ChaCha20Poly1305(key).decrypt(blob[:12], blob[12:], None).decode()


def decoded(item, key, prefix):
    item = json.loads(json.dumps(item))
    for field in ("password", "private_key_inline"):
        item[field] = decrypt(item[field], key, prefix)
    for trigger in item["triggers"]:
        trigger["response"] = decrypt(trigger["response"], key, prefix)
    return item


def snapshot(profile):
    with contextlib.closing(sqlite3.connect(profile / "sessions.db")) as db, db:
        return tuple(tuple(db.execute(query).fetchall()) for query in (
            "SELECT key,value FROM meta ORDER BY key",
            "SELECT ordinal,id,data FROM sessions ORDER BY ordinal,id",
            "SELECT seq,command FROM command_history ORDER BY seq"))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", required=True, type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="xenterm-export-") as directory:
        root = Path(directory)
        fixture = Fixture(args.exe.resolve(), root)
        source = fixture.config
        try:
            fixture.use_key(1, inline=False, encrypted=True)
            fixture.use_key(2, inline=True, encrypted=True)
            fixture.sessions[2].update(jump_session_ids=["outer", "inner"],
                note="synthetic migration note", group="Synthetic group",
                last_used="2026-01-01T00:00:00Z",
                triggers=[dict(expect="fixture prompt", response="synthetic-trigger", append_enter=False, repeat=True)])
            # Forward references and explicit route order must survive fresh IDs.
            fixture.sessions = list(reversed(fixture.sessions)) + [
                session("serial", "", 22) | dict(kind="serial", serial_port="COM7", baud_rate=57600, data_bits=7, stop_bits=2, parity="even", flow_control="hardware"),
                session("telnet", "telnet.invalid", 2323) | dict(kind="telnet", encoding="GBK", vt100_drawing=True),
                session("local", "", 22) | dict(kind="local", local_distribution="Synthetic WSL", local_working_dir="C:\\synthetic\\work"),
            ]
            fixture.save()

            def cli(profile, *args, ok=True):
                result = subprocess.run([str(fixture.exe), "--data-dir", str(profile), "cli", *args],
                    capture_output=True, text=True, timeout=30, env=fixture.env)
                assert (result.returncode == 0) == ok, (result.returncode, result.stdout, result.stderr)
                for secret in (PASSWORD, PASSPHRASE, "PRIVATE KEY", "synthetic-trigger"):
                    assert secret not in result.stdout + result.stderr
                return json.loads(result.stdout) if ok else result

            cli(source, "sessions", "--json")
            before = snapshot(source)
            export = root / "portable.json"
            cli(source, "export", str(export), "--json", ok=False)
            assert not export.exists()
            cli(source, "export", str(export), "--include-credentials", "--overwrite", ok=False)
            assert not export.exists()
            assert cli(source, "export", str(export), "--include-credentials", "--json") == dict(
                exported=6, scope="sessions", includes_credentials=True)
            assert snapshot(source) == before
            data = export.read_bytes()
            assert PASSWORD.encode() not in data and b"PRIVATE KEY" not in data
            original = json.loads(data)
            assert set(original) == {"meatshell_export", "sessions"}
            assert original["meatshell_export"] == 1 and len(original["sessions"]) == 6
            assert all(item["last_used"] is None for item in original["sessions"])
            if os.name == "posix":
                assert export.stat().st_mode & 0o777 == 0o600
            protected = source / "nested"
            protected.mkdir()
            for forbidden in (source / "new-export.json", protected / "new-export.json",
                              source / ".." / "config" / "new-export.json"):
                cli(source, "export", str(forbidden), "--include-credentials", ok=False)
                assert not forbidden.exists()
            sibling = root / "config-backup"
            sibling.mkdir()
            assert cli(source, "export", str(sibling / "allowed.json"), "--include-credentials", "--json")["exported"] == 6
            cli(source, "export", str(export), "--include-credentials", ok=False)
            assert export.read_bytes() == data
            if os.name == "posix":
                link = root / "export-link.json"
                link.symlink_to(export)
                cli(source, "export", str(link), "--include-credentials", ok=False)
                assert link.is_symlink() and export.read_bytes() == data
                dangling = root / "dangling.json"
                missing = root / "must-not-be-created.json"
                dangling.symlink_to(missing)
                cli(source, "export", str(dangling), "--include-credentials", ok=False)
                assert dangling.is_symlink() and not missing.exists()
                profile_alias = root / "profile-alias"
                profile_alias.symlink_to(source, target_is_directory=True)
                ancestor_alias = root / "ancestor-alias"
                ancestor_alias.symlink_to(root, target_is_directory=True)
                external_alias = root / "external-alias"
                external_alias.symlink_to(sibling, target_is_directory=True)
                assert cli(source, "export", str(external_alias / "allowed-alias.json"), "--include-credentials", "--json")["exported"] == 6
                assert (sibling / "allowed-alias.json").is_file()
                for forbidden in (profile_alias / "new-export.json",
                                  profile_alias / "nested" / "new-export.json",
                                  ancestor_alias / "config" / "new-export.json"):
                    cli(source, "export", str(forbidden), "--include-credentials", ok=False)
                    assert not forbidden.exists()
                for name in ("secret.key", "sessions.db"):
                    hardlink = root / ("existing-" + name)
                    os.link(source / name, hardlink)
                    contents = hardlink.read_bytes()
                    cli(source, "export", str(hardlink), "--include-credentials", ok=False)
                    assert hardlink.read_bytes() == contents
                    assert (source / name).read_bytes() == contents
            assert snapshot(source) == before
            assert not list(root.glob(".xenterm-export-*"))
            print("PASS: CLI requires credential opt-in; export is private, atomic, no-clobber and sessions-only")
            print("PASS: profile/subdirectory/ancestor-symlink exports and existing key/database hardlinks are rejected")

            destination = root / "destination"
            destination.mkdir(mode=0o700)
            (destination / "sessions.json").write_text(json.dumps(dict(sessions=[], defaults_rev=999,
                mcp_enabled=True, mcp_use_saved_credentials=True, mcp_allow_commands=True,
                mcp_allow_file_transfers=True)), encoding="utf-8")
            cli(destination, "sessions", "--json")
            before = snapshot(destination)
            assert cli(destination, "import", str(export), "--dry-run", "--json") == dict(added=6, skipped=0, dry_run=True)
            assert snapshot(destination) == before
            assert cli(destination, "import", str(export), "--json") == dict(added=6, skipped=0, dry_run=False)
            source_key = (source / "secret.key").read_bytes()
            destination_key = (destination / "secret.key").read_bytes()
            assert source_key != destination_key
            with contextlib.closing(sqlite3.connect(destination / "sessions.db")) as db, db:
                rows = [json.loads(row[0]) for row in db.execute("SELECT data FROM sessions ORDER BY ordinal,id")]
            by_name = {item["name"]: item for item in rows}
            assert [item["name"] for item in rows] == [item["name"] for item in original["sessions"]]
            id_map = {old["id"]: new["id"] for old, new in zip(original["sessions"], rows)}
            assert all(old != new for old, new in id_map.items())
            for old, new in zip(original["sessions"], rows):
                expected = decoded(old, EXPORT_KEY, "enc:exp:v1:")
                expected["id"] = id_map[old["id"]]
                expected["jump_session_id"] = id_map.get(old["jump_session_id"], "")
                expected["jump_session_ids"] = [id_map[value] for value in old["jump_session_ids"]]
                assert decoded(new, destination_key, "enc:v1:") == expected
            try:
                decrypt(by_name["target"]["password"], source_key, "enc:v1:")
                raise AssertionError("destination ciphertext used the source key")
            except InvalidTag:
                pass
            assert cli(destination, "import", str(export), "--json") == dict(added=0, skipped=6, dry_run=False)
            reexport = root / "reexport.json"
            cli(destination, "export", str(reexport), "--include-credentials", "--json")
            assert [decoded(item, EXPORT_KEY, "enc:exp:v1:") for item in json.loads(reexport.read_text())["sessions"]] == [
                decoded(item, destination_key, "enc:v1:") for item in rows]
            print("PASS: six profiles/four transports survive export/import/reload/re-export with fresh IDs and a distinct key")

            fixture.config = destination
            target_id = by_name["target"]["id"]
            result = fixture.mcp("run_command", session_id=target_id, command="fixture", timeout_seconds=8)
            assert result.get("isError"), "import unexpectedly trusted a host"
            # Trust only these freshly generated loopback keys explicitly.
            names = ("outer", "inner", "target")
            trusted = [f'{by_name[name]["host"]}:{by_name[name]["port"]} {node.key.get_name()} {node.key.get_base64()}'
                       for name, node in zip(names, fixture.nodes)]
            (destination / "known_hosts").write_text("\n".join(trusted) + "\n")
            result = fixture.mcp("run_command", session_id=target_id, command="fixture", timeout_seconds=8)
            assert not result.get("isError"), result
            assert result["structuredContent"]["stdout"] == "multi-hop-command:target\n"
            assert cli(destination, "exec", target_id, "--json", "--", "fixture")["stdout"] == "multi-hop-command:target\n"
            for tool, path in (("list_remote_files", "."), ("read_remote_text_file", "/fixture.txt")):
                result = fixture.mcp(tool, session_id=target_id, path=path, timeout_seconds=8)
                assert not result.get("isError"), result
            for kind in ("serial", "telnet", "local"):
                result = fixture.mcp("run_command", session_id=by_name[kind]["id"], command="fixture", timeout_seconds=1)
                assert result.get("isError"), "SSH automation must reject non-SSH transports"
            print("PASS: imported two-hop password/file-key/inline-key credentials work in CLI and MCP; trust remains explicit")
            print("PASS: non-SSH profiles roundtrip but CLI/MCP SSH tools explicitly reject their execution")
        finally:
            for node in fixture.nodes:
                node.close()


if __name__ == "__main__":
    main()
