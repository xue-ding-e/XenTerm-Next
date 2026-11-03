"""Cold-start SOCKS5 checks; no GUI, real credentials or external endpoints.

Run with the headless binary: python tests/ssh_proxy_e2e.py --exe target/debug/xenterm
Requires paramiko, as does the shared multi-hop SSH fixture.
"""
import argparse
import json
import os
import pathlib
import socket
import struct
import subprocess

from ssh_jump_chain_e2e import Fixture, Node, background, fixture_directory

PROXY_PASSWORD = "loopback-proxy-only"
WRONG_PROXY_PASSWORD = "wrong-fixture-password"


def isolated_environment(root, inherited):
    """Allow OS launch paths only; never copy credentials or ambient routing.

    An allowlist also excludes future token/auth/session/cloud variables without
    needing to know their names. Do not log the inherited values.
    """
    launch_keys = {"PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT", "SYSTEMDRIVE"}
    environment = {key: value for key, value in inherited.items() if key.upper() in launch_keys}
    for key, child in {
        "HOME": "home", "USERPROFILE": "home",
        "APPDATA": "appdata", "LOCALAPPDATA": "localappdata",
        "XDG_CONFIG_HOME": "xdg/config", "XDG_CACHE_HOME": "xdg/cache",
        "XDG_DATA_HOME": "xdg/data", "XDG_STATE_HOME": "xdg/state",
        "TEMP": "tmp", "TMP": "tmp", "TMPDIR": "tmp",
    }.items():
        environment[key] = str(root / child)
    return environment


def check_environment_isolation(root):
    """Pure negative control: polluted parent variables never reach the child."""
    poison_keys = (
        "ALL_PROXY", "all_proxy", "HTTPS_PROXY", "NO_PROXY", "SSH_AUTH_SOCK",
        "GITHUB_TOKEN", "AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY",
        "AZURE_CLIENT_SECRET", "GOOGLE_APPLICATION_CREDENTIALS", "SESSION_TOKEN",
        "AUTH_TOKEN", "CLOUD_TOKEN", "XENTERM_DATA_DIR", "MEATSHELL_DATA_DIR",
        "LD_PRELOAD", "DYLD_INSERT_LIBRARIES", "PYTHONPATH", "RUST_LOG",
        "HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME",
        "XDG_DATA_HOME", "XDG_CACHE_HOME", "TMP", "TEMP", "TMPDIR",
    )
    poison = {key: "fixture-parent-secret-do-not-inherit" for key in poison_keys}
    poison.update(PATH="fixture-launch-path", SystemRoot="fixture-system-root")
    environment = isolated_environment(root, poison)
    assert environment["PATH"] == "fixture-launch-path"
    assert environment["SystemRoot"] == "fixture-system-root"
    assert all(value != "fixture-parent-secret-do-not-inherit" for value in environment.values())
    assert all(key not in environment for key in poison_keys if key not in {
        "HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME",
        "XDG_DATA_HOME", "XDG_CACHE_HOME", "TMP", "TEMP", "TMPDIR",
    })
    assert pathlib.Path(environment["HOME"]).is_relative_to(root)
    assert pathlib.Path(environment["XDG_CONFIG_HOME"]).is_relative_to(root)



def receive(stream, length):
    value = bytearray()
    while len(value) < length:
        part = stream.recv(length - len(value))
        if not part:
            raise EOFError("incomplete fixture SOCKS frame")
        value.extend(part)
    return bytes(value)


class SocksProxy:
    """Only explicitly mapped fixture names can leave this loopback listener."""
    def __init__(self):
        self.routes = {}
        self.requests = []
        self.clients = []
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen()
        self.port = self.listener.getsockname()[1]
        background(self.accept)

    def accept(self):
        while True:
            try:
                stream, _ = self.listener.accept()
            except OSError:
                return
            self.clients.append(stream)
            background(self.serve, stream)

    def serve(self, stream):
        try:
            stream.settimeout(10)
            version, count = receive(stream, 2)
            assert version == 5
            methods = receive(stream, count)
            if 2 not in methods:
                stream.sendall(b"\x05\xff")
                return
            stream.sendall(b"\x05\x02")
            version, length = receive(stream, 2)
            user = receive(stream, length)
            password = receive(stream, receive(stream, 1)[0])
            if version != 1 or user != b"fixture" or password != PROXY_PASSWORD.encode():
                stream.sendall(b"\x01\x01")
                return
            stream.sendall(b"\x01\x00")
            version, command, reserved, kind = receive(stream, 4)
            assert (version, command, reserved) == (5, 1, 0)
            if kind == 3:
                host = receive(stream, receive(stream, 1)[0]).decode("ascii")
            elif kind == 1:
                host = socket.inet_ntop(socket.AF_INET, receive(stream, 4))
            elif kind == 4:
                host = socket.inet_ntop(socket.AF_INET6, receive(stream, 16))
            else:
                raise AssertionError("unexpected SOCKS address type")
            port = struct.unpack("!H", receive(stream, 2))[0]
            self.requests.append((host, port, kind))
            target = self.routes.get((host, port))
            stream.sendall(b"\x05" + (b"\x00" if target else b"\x05") + b"\x00\x01\x7f\x00\x00\x01\x00\x00")
            if target:
                Node.relay(stream, target)
        except (EOFError, OSError, ValueError):
            # Teardown can close a socket while the relay is selecting it.
            pass
        finally:
            stream.close()

    def close(self):
        self.listener.close()
        for stream in self.clients:
            stream.close()

    def url(self, scheme="socks5h", password=PROXY_PASSWORD):
        return f"{scheme}://fixture:{password}@127.0.0.1:{self.port}"


def check(binary, directory):
    root = pathlib.Path(directory)
    check_environment_isolation(root)
    fixture = Fixture(binary, directory)
    # Fixture construction only writes its synthetic profile/starts loopback
    # listeners. Replace its shared helper's inherited environment before any
    # executable is launched, preserving only essential OS launch paths.
    fixture.env = isolated_environment(root, os.environ)
    for key in ("HOME", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME",
                "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "TMPDIR"):
        pathlib.Path(fixture.env[key]).mkdir(parents=True, exist_ok=True)
    proxy = SocksProxy()
    try:
        outer = fixture.sessions[0]
        outer.update(host="outer.invalid", port=22, proxy=proxy.url())
        proxy.routes[("outer.invalid", 22)] = ("127.0.0.1", fixture.nodes[0].port)
        fixture.save()
        fixture.trust()

        def command():
            fixture.command("outer")
            assert proxy.requests[-1] == ("outer.invalid", 22, 3), proxy.requests

        # Every call launches a new process. No GUI SSH window or previous live
        # transport can make these calls pass; .invalid requires proxy-side DNS.
        for scheme in ("socks5", "socks5h"):
            outer["proxy"] = proxy.url(scheme)
            fixture.save()
            command()
        print("PASS: saved authenticated SOCKS5/SOCKS5h works on cold MCP startup with remote DNS")
        run = subprocess.run([str(fixture.exe), "--data-dir", str(fixture.config),
                              "cli", "exec", "outer", "--json", "--", "fixture"],
                             env=fixture.env, text=True, capture_output=True, timeout=30)
        assert run.returncode == 0, (run.stdout, run.stderr)
        assert json.loads(run.stdout)["stdout"] == "multi-hop-command:outer\n", run.stdout
        assert proxy.requests[-1] == ("outer.invalid", 22, 3)
        print("PASS: cold CLI command uses the same saved SOCKS5 route")
        result = fixture.mcp("list_remote_files", session_id="outer", path=".", timeout_seconds=8)
        assert not result.get("isError"), result
        assert "fixture.txt" in json.dumps(result), result
        fixture.command("target")
        result = fixture.mcp("read_remote_text_file", session_id="target", path="fixture.txt", timeout_seconds=8)
        assert not result.get("isError"), result
        assert "multi-hop-sftp" in json.dumps(result), result
        assert fixture.nodes[0].forwarded[-1] == ("inner.invalid", 22)
        assert fixture.nodes[1].forwarded[-1] == ("target.invalid", 22)
        print("PASS: cold SFTP and two SSH bastions retain the outer SOCKS5 route")

        def rejected():
            result = fixture.mcp("run_command", session_id="outer", command="fixture", timeout_seconds=8)
            assert result.get("isError"), result
            encoded = json.dumps(result)
            assert all(password not in encoded for password in
                       (PROXY_PASSWORD, WRONG_PROXY_PASSWORD)), "fixture proxy credential leaked"

        for omit in ("outer",):
            fixture.trust(omit=omit)
            attempts = len(fixture.nodes[0].auth_attempts)
            rejected()
            assert len(fixture.nodes[0].auth_attempts) == attempts, "unknown host reached authentication"
        fixture.trust()
        known_hosts = fixture.config / "known_hosts"
        trusted = known_hosts.read_text()
        known_hosts.write_text(trusted.replace(fixture.nodes[0].key.get_base64(), fixture.nodes[1].key.get_base64()))
        attempts = len(fixture.nodes[0].auth_attempts)
        rejected()
        assert len(fixture.nodes[0].auth_attempts) == attempts, "changed host reached authentication"
        fixture.trust()
        print("PASS: proxy does not bypass unknown or changed SSH host-key rejection")

        # Make the real SSH destination directly reachable: proxy failures must
        # still fail, rather than quietly using a direct connection.
        outer.update(host="127.0.0.1", port=fixture.nodes[0].port,
                     proxy=proxy.url(password=WRONG_PROXY_PASSWORD))
        fixture.save()
        fixture.trust()
        attempts = len(fixture.nodes[0].auth_attempts)
        rejected()
        outer["proxy"] = proxy.url()
        fixture.save()  # This numeric target is intentionally absent from routes.
        rejected()
        assert len(fixture.nodes[0].auth_attempts) == attempts, "proxy rejection fell back to direct SSH"
        print("PASS: rejected proxy authentication and CONNECT never fall back to direct SSH")

        outer.update(host="outer.invalid", port=22, proxy="")
        fixture.save()
        fixture.trust()
        fixture.env["ALL_PROXY"] = proxy.url()
        command()
        outer["proxy"] = proxy.url()
        fixture.save()
        fixture.env["ALL_PROXY"] = "unsupported://fixture.invalid:1"
        command()
        print("PASS: environment fallback and saved-session precedence work without a GUI")
    finally:
        proxy.close()
        for node in fixture.nodes:
            node.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", required=True)
    args = parser.parse_args()
    with fixture_directory() as directory:
        check(args.exe, directory)


if __name__ == "__main__":
    main()
