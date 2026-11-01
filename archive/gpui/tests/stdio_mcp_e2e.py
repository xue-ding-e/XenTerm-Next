"""Stdio framing and detached-worker timeout regression, using loopback only.

Run against a --no-default-features --features headless build.
No real profile, credentials, SSH host trust or external server is used.
"""
import argparse
import contextlib
import json
import os
from pathlib import Path
import selectors
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', required=True, type=Path)
    args = parser.parse_args()
    env = {k: v for k, v in os.environ.items()
           if k.lower() not in ('http_proxy', 'https_proxy', 'all_proxy')
           and k not in ('XENTERM_DATA_DIR', 'MEATSHELL_DATA_DIR')}
    with tempfile.TemporaryDirectory(prefix='xenterm-stdio-fixture-') as temp:
        root = Path(temp)
        untouched = root / 'must-remain-absent'
        isolated_env = dict(env, HOME=str(untouched), XDG_CONFIG_HOME=str(untouched / 'config'),
                            XDG_DATA_HOME=str(untouched / 'data'))
        for command in [['cli', 'sessions'], ['mcp', 'serve'], ['--config-info'], []]:
            result = subprocess.run([str(args.exe.resolve()), *command], env=isolated_env,
                                    input='', capture_output=True, text=True, timeout=5)
            assert result.returncode != 0 and '--data-dir' in result.stderr
            assert not untouched.exists(), 'headless startup touched an implicit desktop profile'
        assert subprocess.run([str(args.exe.resolve()), '--version'], env=isolated_env,
                              capture_output=True, timeout=5).returncode == 0
        print('PASS: all headless profile operations require an explicit profile before touching defaults')
        profile = root / 'profile' 
        profile.mkdir(mode=0o700)
        session = dict(id='fixture', name='Fixture', host='127.0.0.1', port=1,
                       user='fixture', auth='password', password='synthetic-stdio-secret', kind='ssh')
        with contextlib.closing(sqlite3.connect(profile / 'sessions.db')) as db, db:
            db.executescript('CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);'
                             'CREATE TABLE sessions (ordinal INTEGER NOT NULL, id TEXT PRIMARY KEY, data TEXT NOT NULL);'
                             'CREATE TABLE command_history (seq INTEGER PRIMARY KEY AUTOINCREMENT, command TEXT NOT NULL);')
            db.execute("INSERT INTO meta VALUES ('schema_version', '1')")
            db.execute("INSERT INTO meta VALUES ('settings', ?)", (json.dumps(dict(
                defaults_rev=999, mcp_enabled=True, mcp_use_saved_credentials=True,
                mcp_allow_commands=False, mcp_allow_file_transfers=True)),))
            db.execute('INSERT INTO sessions VALUES (0, ?, ?)', ('fixture', json.dumps(session)))
        upload = root / 'upload.txt'
        upload.write_text('synthetic fixture only')
        log = root / 'stderr.log'
        with log.open('w') as stderr:
            process = subprocess.Popen([str(args.exe.resolve()), '--data-dir', str(profile), 'mcp', 'serve'],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr, text=True, env=env)
        selector = selectors.DefaultSelector()
        selector.register(process.stdout, selectors.EVENT_READ)
        def send(request):
            process.stdin.write(json.dumps(request) + '\n')
            process.stdin.flush()
        def exchange(request):
            send(request)
            assert selector.select(timeout=10), 'stdio response deadline exceeded'
            line = process.stdout.readline()
            assert line, 'stdio exited before responding: ' + log.read_text()
            response = json.loads(line)
            assert response.get('jsonrpc') == '2.0' and response.get('id') == request['id']
            assert 'synthetic-stdio-secret' not in line
            return response
        try:
            initialized = exchange(dict(jsonrpc='2.0', id=1, method='initialize', params=dict(
                protocolVersion='2025-11-25', capabilities={}, clientInfo=dict(name='fixture', version='1'))))
            assert initialized['result']['protocolVersion'] == '2025-11-25'
            send(dict(jsonrpc='2.0', method='notifications/initialized'))
            listed = exchange(dict(jsonrpc='2.0', id=2, method='tools/list'))
            assert {t['name'] for t in listed['result']['tools']} >= {'list_sessions', 'import_sessions', 'run_command'}
            exchange(dict(jsonrpc='2.0', id=3, method='ping'))
            print('PASS: stdio initialize, notifications, list and ping retain newline JSON-RPC framing')
            for request_id, (name, extra) in enumerate([
                ('list_remote_files', dict(path='.')),
                ('read_remote_text_file', dict(path='/fixture')),
                ('upload_file', dict(local_path=str(upload), remote_directory='/tmp')),
            ], start=4):
                accepted, closed = threading.Event(), threading.Event()
                with socket.socket() as peer:
                    peer.bind(('127.0.0.1', 0))
                    peer.listen()
                    peer.settimeout(5)
                    session['port'] = peer.getsockname()[1]
                    with contextlib.closing(sqlite3.connect(profile / 'sessions.db')) as db, db:
                        db.execute('UPDATE sessions SET data=? WHERE id=?', (json.dumps(session), 'fixture'))
                    def silent_peer():
                        try:
                            connection, _ = peer.accept()
                            with connection:
                                connection.settimeout(5)
                                accepted.set()
                                try:
                                    while connection.recv(8192):
                                        pass
                                    closed.set()
                                except ConnectionResetError:
                                    closed.set()
                        except OSError:
                            pass
                    thread = threading.Thread(target=silent_peer, daemon=True)
                    thread.start()
                    start = time.monotonic()
                    response = exchange(dict(jsonrpc='2.0', id=request_id, method='tools/call', params=dict(
                        name=name, arguments=dict(session_id='fixture', timeout_seconds=1, **extra))))
                    assert response['result']['isError'] and 'timed out' in json.dumps(response)
                    assert accepted.is_set() and closed.wait(3), f'{name} detached its timed-out SSH/SFTP worker'
                    assert time.monotonic() - start < 5
                    thread.join(timeout=1)
                print(f'PASS: stdio {name} timeout aborts the worker and closes its loopback transport')
            exchange(dict(jsonrpc='2.0', id=10, method='ping'))
        finally:
            process.stdin.close()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
                raise AssertionError('stdio EOF did not shut down')
            selector.close()
        assert process.returncode == 0, log.read_text()
        assert 'synthetic-stdio-secret' not in log.read_text()
        print('PASS: server remains responsive after timeouts, exits on EOF, and never logs credentials')


if __name__ == '__main__':
    main()
