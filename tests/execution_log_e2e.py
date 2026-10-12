"""Real MCP + loopback SSH proof of approval removal and opt-in execution history."""
import argparse
import asyncio
from contextlib import closing
import json
from pathlib import Path
import socket
import sqlite3
import tempfile
import threading

import paramiko
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client
from access_policy_e2e import create_profile, payload, profile_settings


def settings(profile, **values):
    with closing(sqlite3.connect(profile / 'sessions.db')) as db:
        current = profile_settings(profile)
        current.update(values)
        db.execute("UPDATE meta SET value=? WHERE key='settings'", (json.dumps(current),))
        db.commit()


def records(profile):
    return [json.loads(line) for day in (profile / 'mcp-executions').glob('executions-*.jsonl')
            for line in day.read_text(encoding='utf-8').splitlines()]


class Server(paramiko.ServerInterface):
    def __init__(self):
        self.command = ''
        self.requested = threading.Event()

    def check_auth_password(self, username, password):
        if username == 'fixture' and password == 'synthetic-policy-secret':
            return paramiko.AUTH_SUCCESSFUL
        return paramiko.AUTH_FAILED

    def get_allowed_auths(self, username):
        return 'password'

    def check_channel_request(self, kind, channel_id):
        return paramiko.OPEN_SUCCEEDED if kind == 'session' else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

    def check_channel_exec_request(self, channel, command):
        self.command = command.decode('utf-8')
        self.requested.set()
        return True


class Loopback:
    def __init__(self):
        self.key = paramiko.RSAKey.generate(2048)
        self.socket = socket.socket()
        self.socket.bind(('127.0.0.1', 0))
        self.socket.listen(4)
        self.socket.settimeout(0.2)
        self.port = self.socket.getsockname()[1]
        self.stop = threading.Event()
        self.started = threading.Event()
        self.resume = threading.Event()
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def run(self):
        while not self.stop.is_set():
            try:
                connection, _ = self.socket.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            transport = paramiko.Transport(connection)
            try:
                transport.add_server_key(self.key)
                server = Server()
                transport.start_server(server=server)
                channel = transport.accept(5)
                if channel is None or not server.requested.wait(5):
                    continue
                if server.command == 'fixture-delay':
                    self.started.set()
                    self.resume.wait(3)
                channel.sendall(b'synthetic-output-never-recorded\n')
                channel.send_stderr(b'synthetic-stderr-never-recorded\n')
                channel.send_exit_status(17 if server.command == 'fixture-fail' else 0)
                channel.shutdown_write()
                channel.close()
            except (OSError, EOFError, paramiko.SSHException) as error:
                print(f'Loopback SSH fixture: {type(error).__name__}: {error}')
            finally:
                transport.close()

    def close(self):
        self.stop.set()
        self.resume.set()
        self.socket.close()
        self.thread.join(timeout=6)


async def verify(executable):
    with tempfile.TemporaryDirectory(prefix='xenterm-execution-history-') as temporary:
        profile = Path(temporary) / 'profile'
        create_profile(profile)
        server = Loopback()
        try:
            with closing(sqlite3.connect(profile / 'sessions.db')) as db:
                session = json.loads(db.execute("SELECT data FROM sessions WHERE id='fixture'").fetchone()[0])
                session['port'] = server.port
                db.execute("UPDATE sessions SET data=? WHERE id='fixture'", (json.dumps(session),))
                db.commit()
            # Trust only this fixture's newly generated key, without bypassing verification.
            (profile / 'known_hosts').write_text(
                f'127.0.0.1:{server.port} {server.key.get_name()} {server.key.get_base64()}\n',
                encoding='utf-8')
            settings(profile, mcp_access_mode='restricted', mcp_use_saved_credentials=True,
                     mcp_allow_commands=True, mcp_allow_file_transfers=True,
                     mcp_approval_enabled=True, mcp_risky_dirs=['/root'],
                     mcp_approval_timeout_secs=600)
            parameters = StdioServerParameters(command=str(executable),
                args=['--data-dir', str(profile), 'mcp', 'serve'])
            async with stdio_client(parameters) as (reader, writer):
                async with ClientSession(reader, writer) as client:
                    await client.initialize()

                    async def execute(command):
                        result = await asyncio.wait_for(client.call_tool('run_command', {
                            'session_id': 'fixture', 'command': command,
                            'timeout_seconds': 5}), timeout=8)
                        assert not result.isError, payload(result)
                        return json.loads(payload(result))

                    policy = json.loads(payload(await client.call_tool('get_access_policy', {})))
                    assert not policy['execution_log_enabled'] and 'approval_enabled' not in policy
                    outcome = await execute('printf /root')
                    assert outcome['exit_code'] == 0
                    assert outcome['stdout'] == 'synthetic-output-never-recorded\n'
                    assert outcome['stderr'] == 'synthetic-stderr-never-recorded\n'
                    assert not (profile / 'mcp-approvals').exists(), 'legacy risk settings still gate execution'
                    assert not (profile / 'mcp-executions').exists(), 'default-off history wrote files'

                    settings(profile, mcp_execution_log_enabled=True)
                    assert (await execute('fixture-fail'))['exit_code'] == 17
                    saved = records(profile)
                    assert len(saved) == 1 and saved[0]['outcome'] == 'failed'
                    assert saved[0]['exit_code'] == 17 and saved[0]['command'] == 'fixture-fail'
                    raw = json.dumps(saved)
                    assert 'synthetic-output-never-recorded' not in raw
                    assert 'synthetic-stderr-never-recorded' not in raw
                    assert 'synthetic-policy-secret' not in raw

                    # Opting out while an operation is running also stops its final write.
                    running = asyncio.create_task(execute('fixture-delay'))
                    assert await asyncio.to_thread(server.started.wait, 5), 'fixture did not start'
                    settings(profile, mcp_execution_log_enabled=False)
                    server.resume.set()
                    assert (await running)['exit_code'] == 0
                    assert records(profile) == saved
                    assert (await execute('printf /root'))['exit_code'] == 0
                    assert records(profile) == saved
                    assert not (profile / 'mcp-approvals').exists()
            print('PASS: real loopback SSH command without approval, default-off history with no directory, explicit opt-in and failed exit, no output/credential logging, live and in-flight opt-out; no external servers used')
        finally:
            server.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', required=True, type=Path)
    asyncio.run(verify(parser.parse_args().exe.resolve()))
