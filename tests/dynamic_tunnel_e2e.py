"""No-GUI dynamic-forward lifecycle, using only generated loopback fixtures.

--expect-missing checks the old binary's missing API (controlled red baseline).
Normal mode verifies the implementation, not merely advertised tool names.
"""
import argparse
import json
import os
import pathlib
import queue
import socket
import sqlite3
from contextlib import closing
import struct
import subprocess
import tempfile
import threading
import time
from urllib.parse import urlsplit

from ssh_jump_chain_e2e import Fixture, background, fixture_directory
from ssh_proxy_e2e import isolated_environment, check_environment_isolation


def configure_fixture_environment(fixture):
    """Share the cold-proxy fixture factory; every writable profile path is temporary."""
    check_environment_isolation(fixture.root)
    fixture.env = isolated_environment(fixture.root, os.environ)
    for key in ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME",
                "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "TEMP", "TMP", "TMPDIR"):
        pathlib.Path(fixture.env[key]).mkdir(parents=True, exist_ok=True)


class Peer:
    def __init__(self):
        self.listener = socket.socket()
        self.listener.bind(('127.0.0.1', 0))
        self.listener.listen()
        self.address = self.listener.getsockname()
        self.clients = []
        background(self.accept)

    def accept(self):
        while True:
            try:
                client, _ = self.listener.accept()
            except OSError:
                return
            self.clients.append(client)
            background(self.echo, client)

    def echo(self, client):
        try:
            while data := client.recv(1024):
                client.sendall(data)
        except OSError:
            pass
        finally:
            client.close()

    def close(self):
        self.listener.close()
        for client in self.clients:
            client.close()


def receive(stream, count):
    value = b''
    while len(value) < count:
        chunk = stream.recv(count - len(value))
        if not chunk:
            raise EOFError('fixture connection closed')
        value += chunk
    return value


def socks(url):
    addr = urlsplit(url)
    stream = socket.create_connection((addr.hostname, addr.port), timeout=3)
    stream.sendall(b'\x05\x01\x00')
    assert receive(stream, 2) == b'\x05\x00'
    host = b'echo.invalid'
    stream.sendall(b'\x05\x01\x00\x03' + bytes([len(host)]) + host + struct.pack('!H', 23))
    assert receive(stream, 10)[1] == 0
    stream.sendall(b'fixture-through-dynamic')
    assert receive(stream, 23) == b'fixture-through-dynamic'
    return stream


class Mcp:
    def __init__(self, fixture):
        self.errors = tempfile.TemporaryFile(mode='w+', encoding='utf-8')
        self.process = subprocess.Popen([str(fixture.exe), '--data-dir', str(fixture.config), 'mcp', 'serve'],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.errors,
                                        env=fixture.env, text=True, encoding='utf-8')
        self.replies = queue.Queue()
        self.counter = 0
        def reader():
            for line in self.process.stdout:
                if line.strip():
                    self.replies.put(json.loads(line))
        background(reader)
        self.request('initialize', dict(protocolVersion='2025-06-18', capabilities={},
                                      clientInfo=dict(name='dynamic-fixture', version='1')))

    def request(self, method, params):
        self.counter += 1
        self.process.stdin.write(json.dumps(dict(jsonrpc='2.0', id=self.counter, method=method, params=params)) + '\n')
        self.process.stdin.flush()
        result = self.replies.get(timeout=70)
        assert result['id'] == self.counter, result
        assert 'result' in result, result
        return result['result']

    def call(self, name, error=False, **arguments):
        result = self.request('tools/call', dict(name=name, arguments=arguments))
        assert bool(result.get('isError')) == error, result
        return result if error else result['structuredContent']

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            self.process.wait(timeout=10)
        assert self.process.returncode == 0
        self.errors.seek(0)
        errors = self.errors.read()
        assert 'loopback-fixture-only' not in errors, 'saved SSH password leaked to stderr'
        self.errors.close()


def free_port():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


def assert_closed(url):
    address = urlsplit(url)
    deadline = time.monotonic() + 5
    while True:
        # A closed listener can leave accepted TCP streams in TIME_WAIT. A
        # non-reuse bind would confuse that with a live listener on Windows/Linux.
        try:
            with socket.create_connection((address.hostname, address.port), timeout=.2):
                pass
        except ConnectionRefusedError:
            return
        assert time.monotonic() < deadline, 'listener still accepts connections after release/exit'
        time.sleep(.05)


def check_cli(fixture):
    with tempfile.TemporaryFile(mode='w+', encoding='utf-8') as errors:
        process = subprocess.Popen([str(fixture.exe), '--data-dir', str(fixture.config),
                                    'cli', 'tunnel', 'target', '--forward-index', '0', '--json'],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors,
                                   env=fixture.env, text=True, encoding='utf-8')
        lines = queue.Queue()
        def reader():
            for line in process.stdout:
                if line.strip():
                    lines.put(json.loads(line))
        background(reader)
        try:
            ready = lines.get(timeout=70)
            assert ready['state'] == 'running', ready
            stream = socks(ready['proxy_url'])
            assert process.poll() is None, 'CLI returned while leaving a detached tunnel'
            process.stdin.close()
            process.wait(timeout=10)
            assert process.returncode == 0
            assert lines.get(timeout=3)['state'] == 'released'
            try:
                assert stream.recv(1) == b''
            except ConnectionResetError:
                pass
            stream.close()
            assert_closed(ready['proxy_url'])
            errors.seek(0)
            assert 'loopback-fixture-only' not in errors.read()
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
    print('PASS: CLI remains in foreground and EOF closes the listener, active clients and SSH route')


def check(binary, directory, expect_missing):
    fixture = Fixture(binary, directory)
    # Configure before launching any child; no inherited profile or routing.
    configure_fixture_environment(fixture)
    peer = Peer()
    fixture.nodes[2].routes[('echo.invalid', 23)] = peer.address
    fixture.sessions[2]['forwards'] = [
        dict(kind='dynamic', bind_addr='127.0.0.1', bind_port=free_port()),
        dict(kind='dynamic', bind_addr='', bind_port=0),
    ]
    fixture.save()
    clients = []
    try:
        mcp = Mcp(fixture)
        clients.append(mcp)
        names = {tool['name'] for tool in mcp.request('tools/list', {})['tools']}
        if expect_missing:
            assert 'start_dynamic_tunnel' not in names, 'baseline unexpectedly implements dynamic lifecycle'
            mcp.call('start_dynamic_tunnel', error=True, session_id='target', forward_index=0)
            print('REPRODUCED: old binary has no no-GUI dynamic-tunnel lifecycle entry point')
            return
        assert {'start_dynamic_tunnel', 'ensure_dynamic_tunnel', 'dynamic_tunnel_status', 'release_dynamic_tunnel'} <= names
        meta = mcp.call('get_session', session_id='target')
        assert len(meta['dynamic_forwards']) == 2, meta
        mcp.call('start_dynamic_tunnel', error=True, session_id='target')  # Ambiguous saved rules.
        first = mcp.call('start_dynamic_tunnel', session_id='target', forward_index=0)
        assert first['state'] == 'running' and first['proxy_url'].startswith('socks5h://127.0.0.1:'), first
        stream = socks(first['proxy_url'])
        assert fixture.nodes[2].forwarded[-1] == ('echo.invalid', 23)
        same = mcp.call('ensure_dynamic_tunnel', session_id='target', forward_index=0)
        assert same['handle'] == first['handle'] and same['proxy_url'] == first['proxy_url']
        assert mcp.call('dynamic_tunnel_status', handle=first['handle'])['state'] == 'running'
        mcp.call('start_dynamic_tunnel', error=True, session_id='target', forward_index=0)
        with closing(sqlite3.connect(fixture.config / 'sessions.db')) as db:
            original = db.execute("SELECT data FROM sessions WHERE id='target'").fetchone()[0]
            changed = json.loads(original)
            changed['forwards'][0]['name'] = 'edited fixture rule'
            changed.update(name='renamed fixture', note='edited note', last_used='2026-01-01T00:00:00Z')
            db.execute("UPDATE sessions SET data=? WHERE id='target'", (json.dumps(changed),))
            db.commit()
        assert mcp.call('ensure_dynamic_tunnel', session_id='target', forward_index=0)['handle'] == first['handle']
        changed['forwards'][0]['bind_port'] = free_port()
        with closing(sqlite3.connect(fixture.config / 'sessions.db')) as db:
            db.execute("UPDATE sessions SET data=? WHERE id='target'", (json.dumps(changed),))
            db.commit()
        mcp.call('ensure_dynamic_tunnel', error=True, session_id='target', forward_index=0)
        with closing(sqlite3.connect(fixture.config / 'sessions.db')) as db:
            db.execute("UPDATE sessions SET data=? WHERE id='target'", (original,))
            db.commit()
        assert mcp.call('ensure_dynamic_tunnel', session_id='target', forward_index=0)['handle'] == first['handle']
        print('PASS: cold MCP starts saved SOCKS through two SSH bastions; remote DNS, reuse and edited-rule rejection')
        second = mcp.call('start_dynamic_tunnel', session_id='target', forward_index=1)
        assert second['handle'] != first['handle'] and second['proxy_url'] != first['proxy_url']
        socks(second['proxy_url']).close()
        other = Mcp(fixture)
        clients.append(other)
        other.call('dynamic_tunnel_status', error=True, handle=first['handle'])
        conflict = other.call('start_dynamic_tunnel', error=True, session_id='target', forward_index=0)
        assert 'port is in use' in json.dumps(conflict), conflict
        stream.sendall(b'x')
        assert receive(stream, 1) == b'x', 'conflicting owner altered the existing listener'
        print('PASS: multiple rules, foreign-handle isolation and non-destructive port conflict')
        released = mcp.call('release_dynamic_tunnel', handle=first['handle'])
        assert released['state'] == 'released'
        try:
            assert stream.recv(1) == b'', 'active SOCKS client survived release'
        except ConnectionResetError:
            pass
        stream.close()
        assert_closed(first['proxy_url'])
        mcp.call('dynamic_tunnel_status', error=True, handle=first['handle'])
        mcp.close()
        clients.remove(mcp)
        assert_closed(second['proxy_url'])
        print('PASS: explicit release and stdio EOF close listeners and active SOCKS clients')

        fixture.trust(omit='target')
        attempts = len(fixture.nodes[2].auth_attempts)
        other.call('start_dynamic_tunnel', error=True, session_id='target', forward_index=0)
        assert len(fixture.nodes[2].auth_attempts) == attempts, 'unknown host reached authentication'
        fixture.trust()
        print('PASS: headless dynamic startup rejects an unknown target host key')
        check_cli(fixture)
        revoked = other.call('start_dynamic_tunnel', session_id='target', forward_index=0)
        active = socks(revoked['proxy_url'])
        with closing(sqlite3.connect(fixture.config / 'sessions.db')) as db:
            original_settings = db.execute("SELECT value FROM meta WHERE key='settings'").fetchone()[0]
            settings = json.loads(original_settings)
            settings.update(mcp_access_mode='restricted', mcp_allow_commands=False)
            db.execute("UPDATE meta SET value=? WHERE key='settings'", (json.dumps(settings),))
            db.commit()
        deadline = time.monotonic() + 5
        while other.call('dynamic_tunnel_status', handle=revoked['handle'])['state'] == 'running':
            assert time.monotonic() < deadline, 'permission revocation left proxy running'
            time.sleep(.1)
        try:
            assert active.recv(1) == b''
        except ConnectionResetError:
            pass
        active.close()
        other.call('release_dynamic_tunnel', handle=revoked['handle'])
        assert_closed(revoked['proxy_url'])
        with closing(sqlite3.connect(fixture.config / 'sessions.db')) as db:
            db.execute("UPDATE meta SET value=? WHERE key='settings'", (original_settings,))
            db.commit()
        print('PASS: permission revocation closes active SOCKS traffic; release remains available')
        third = other.call('start_dynamic_tunnel' , session_id='target', forward_index=0)
        fixture.nodes[2].close()
        deadline = time.monotonic() + 5
        while other.call('dynamic_tunnel_status', handle=third['handle'])['state'] == 'running':
            assert time.monotonic() < deadline, 'SSH disconnect was not reported'
            time.sleep(.1)
        other.call('ensure_dynamic_tunnel', error=True, session_id='target', forward_index=0)
        other.call('release_dynamic_tunnel', handle=third['handle'])
        assert_closed(third['proxy_url'])
        print('PASS: SSH disconnect reports stale state; ensure does not claim a dead handle is reusable')
    finally:
        for client in clients:
            client.close()
        peer.close()
        for node in fixture.nodes:
            node.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', required=True)
    parser.add_argument('--expect-missing', action='store_true')
    args = parser.parse_args()
    with fixture_directory() as directory:
        check(args.exe, directory, args.expect_missing)


if __name__ == '__main__':
    main()
