"""Access modes through CLI and real MCP framing, using only a disposable profile."""
import argparse
import asyncio
from contextlib import closing
import json
import sqlite3
import subprocess
import tempfile
from pathlib import Path

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client
from mcp.shared.exceptions import MCPError


def profile_settings(profile):
    with closing(sqlite3.connect(profile / 'sessions.db')) as db:
        return json.loads(db.execute("SELECT value FROM meta WHERE key='settings'").fetchone()[0])


def create_profile(profile, enabled=True):
    profile.mkdir()
    settings = dict(defaults_rev=999, mcp_enabled=enabled, mcp_use_saved_credentials=False,
                    mcp_allow_commands=False, mcp_allow_file_transfers=False,
                    mcp_approval_enabled=True, mcp_approval_timeout_secs=10)
    session = dict(id='fixture', name='Policy fixture', host='127.0.0.1', port=1,
                   user='fixture', auth='password', password='synthetic-policy-secret', kind='ssh')
    with closing(sqlite3.connect(profile / 'sessions.db')) as db:
        db.executescript('CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);'
                         'CREATE TABLE sessions (ordinal INTEGER NOT NULL, id TEXT PRIMARY KEY, data TEXT NOT NULL);'
                         'CREATE TABLE command_history (seq INTEGER PRIMARY KEY AUTOINCREMENT, command TEXT NOT NULL);')
        db.execute("INSERT INTO meta VALUES ('schema_version', '1')")
        db.execute("INSERT INTO meta VALUES ('settings', ?)", (json.dumps(settings),))
        db.execute('INSERT INTO sessions VALUES (0, ?, ?)', ('fixture', json.dumps(session)))
        db.commit()


def cli(executable, profile, *arguments, succeeds=True):
    result = subprocess.run([str(executable), '--data-dir', str(profile), 'cli', 'permissions',
                             *arguments, '--json'], capture_output=True, text=True,
                            encoding='utf-8', timeout=15)
    assert (result.returncode == 0) == succeeds, result.stderr
    return json.loads(result.stdout) if succeeds else result


def payload(result):
    text = '\n'.join(getattr(part, 'text', '') for part in result.content)
    assert 'synthetic-policy-secret' not in text
    return text


async def verify(executable):
    with tempfile.TemporaryDirectory(prefix='xenterm-policy-') as temporary:
        profile = Path(temporary) / 'enabled'
        create_profile(profile)
        initial = cli(executable, profile)
        assert initial['mode'] == 'restricted' and not initial['allow_commands']
        before = profile_settings(profile)
        cli(executable, profile, 'invalid', succeeds=False)
        assert profile_settings(profile) == before
        full = cli(executable, profile, 'full')
        assert full['mode'] == 'full' and not full['execution_log_enabled']
        assert 'approval_enabled' not in full and 'approval_timeout_seconds' not in full
        assert all(full[key] for key in ['use_saved_credentials', 'allow_commands', 'allow_file_transfers'])
        persisted = profile_settings(profile)
        assert persisted['mcp_access_mode'] == 'full'
        assert not persisted['mcp_allow_commands'], 'full overwrote restricted preferences'
        parameters = StdioServerParameters(command=str(executable),
                                          args=['--data-dir', str(profile), 'mcp', 'serve'])
        async with stdio_client(parameters) as (reader, writer):
            async with ClientSession(reader, writer) as client:
                await client.initialize()
                discovered = await client.list_tools()
                assert 'get_access_policy' in {tool.name for tool in discovered.tools}
                policy = await client.call_tool('get_access_policy', {})
                assert not policy.is_error and json.loads(payload(policy))['mode'] == 'full'
                # Corrupt only the disposable profile. A live process must fail
                # closed without claiming the switch was disabled or leaking data.
                with closing(sqlite3.connect(profile / 'sessions.db')) as db:
                    original = db.execute("SELECT value FROM meta WHERE key='settings'").fetchone()[0]
                def set_settings(value):
                    with closing(sqlite3.connect(profile / 'sessions.db')) as db, db:
                        db.execute("UPDATE meta SET value=? WHERE key='settings'", (value,))
                try:
                    set_settings('{invalid-json synthetic-policy-secret')
                    for operation in [client.list_tools, lambda: client.call_tool('get_access_policy', {})]:
                        try:
                            await operation()
                        except MCPError as error:
                            assert error.error.code == -32002, error
                            assert 'configuration is unavailable' in error.error.message, error
                            assert 'synthetic-policy-secret' not in str(error)
                            assert str(profile) not in str(error)
                            assert 'disabled' not in str(error)
                        else:
                            raise AssertionError('Unreadable settings allowed an MCP operation')
                    disabled_settings = json.loads(original)
                    disabled_settings['mcp_enabled'] = False
                    set_settings(json.dumps(disabled_settings))
                    try:
                        await client.list_tools()
                    except MCPError as error:
                        assert error.error.code == -32001, error
                        assert 'disabled' in error.error.message, error
                    else:
                        raise AssertionError('Explicitly disabled MCP allowed discovery')
                finally:
                    set_settings(original)
                recovered = await client.call_tool('get_access_policy', {})
                assert not recovered.is_error and json.loads(payload(recovered))['mcp_enabled'] is True
                assert (await client.list_tools()).tools, 'same-process recovery failed'
                sessions = await client.call_tool('list_sessions', {})
                assert not sessions.is_error and 'Policy fixture' in payload(sessions)
                result = await asyncio.wait_for(client.call_tool('run_command', {
                    'session_id': 'fixture', 'command': 'printf /root', 'timeout_seconds': 1}), timeout=6)
                command_result = payload(result)
                if not result.is_error:
                    outcome = json.loads(command_result)
                    assert outcome.get('exit_code') != 0, outcome
                assert 'risk-approval' not in command_result
                assert not (profile / 'mcp-approvals').exists(), 'approval feature is still active'
                assert not (profile / 'mcp-executions').exists(), 'disabled history created files'
                restricted = cli(executable, profile, 'restricted')
                assert restricted['mode'] == 'restricted' and not restricted['execution_log_enabled']
                assert not restricted['allow_commands'] and not restricted['allow_file_transfers']
                policy = await client.call_tool('get_access_policy', {})
                assert json.loads(payload(policy))['mode'] == 'restricted', 'running MCP did not reload policy'
                blocked = await client.call_tool('list_sessions', {})
                assert blocked.is_error and 'disabled' in payload(blocked)
                mutation = await client.call_tool('get_access_policy', {'mode': 'full'})
                assert mutation.is_error
                assert profile_settings(profile)['mcp_access_mode'] == 'restricted'
        disabled = Path(temporary) / 'disabled'
        create_profile(disabled, enabled=False)
        assert cli(executable, disabled, 'full')['mcp_enabled'] is False
        disabled_parameters = StdioServerParameters(command=str(executable),
            args=['--data-dir', str(disabled), 'mcp', 'serve'])
        async with stdio_client(disabled_parameters) as (reader, writer):
            async with ClientSession(reader, writer) as client:
                try:
                    await client.initialize()
                except Exception as error:
                    assert 'disabled' in str(error), str(error)
                else:
                    raise AssertionError('full access bypassed the MCP server switch')
        print('PASS: CLI persistence, MCP policy discovery, full access without approval or default logging, live restricted restoration, read-only policy and disabled-server gate; no real servers used')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', required=True, type=Path)
    asyncio.run(verify(parser.parse_args().exe.resolve()))
