"""Windows headless approval regression with an isolated, loopback-only profile."""
import argparse
import asyncio
import ctypes
from contextlib import closing
import json
import sqlite3
import tempfile
import threading
import time
from pathlib import Path

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client


def observe_windows(stop, titles):
    callback_type = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
    def observe(hwnd, _):
        if ctypes.windll.user32.IsWindowVisible(ctypes.c_void_p(hwnd)):
            title = ctypes.create_unicode_buffer(512)
            ctypes.windll.user32.GetWindowTextW(ctypes.c_void_p(hwnd), title, 512)
            if title.value in {'XenTerm 风险操作审批', 'XenTerm Risk approval',
                               'XenTerm · 操作确认', 'XenTerm · Confirmation'}:
                titles.add(title.value)
        return True
    callback = callback_type(observe)
    while not stop.wait(0.1):
        ctypes.windll.user32.EnumWindows(callback, 0)


def create_profile(profile):
    settings = dict(defaults_rev=999, mcp_enabled=True, mcp_use_saved_credentials=True,
                    mcp_allow_commands=True, mcp_approval_enabled=True,
                    mcp_approval_timeout_secs=10)
    session = dict(id='fixture', name='审批窗口测试（本地假会话）', host='127.0.0.1',
                   port=1, user='fixture', auth='password',
                   password='synthetic-fixture-only', kind='ssh')
    with closing(sqlite3.connect(profile / 'sessions.db')) as db:
        db.executescript('CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);'
                         'CREATE TABLE sessions (ordinal INTEGER NOT NULL, id TEXT PRIMARY KEY, data TEXT NOT NULL);'
                         'CREATE TABLE command_history (seq INTEGER PRIMARY KEY AUTOINCREMENT, command TEXT NOT NULL);')
        db.execute("INSERT INTO meta VALUES ('schema_version', '1')")
        db.execute("INSERT INTO meta VALUES ('settings', ?)", (json.dumps(settings),))
        db.execute('INSERT INTO sessions VALUES (0, ?, ?)', ('fixture', json.dumps(session)))
        db.commit()


async def verify(executable, expected_ui):
    with tempfile.TemporaryDirectory(prefix='xenterm-native-approval-') as temporary:
        profile = Path(temporary)
        create_profile(profile)
        titles, stop = set(), threading.Event()
        watcher = threading.Thread(target=observe_windows, args=(stop, titles), daemon=True)
        watcher.start()
        parameters = StdioServerParameters(command=str(executable.resolve()),
            args=['--data-dir', str(profile), 'mcp', 'serve'])
        try:
            async with stdio_client(parameters) as (reader, writer):
                async with ClientSession(reader, writer) as client:
                    await client.initialize()
                    started = time.monotonic()
                    result = await asyncio.wait_for(client.call_tool('run_command', {
                        'session_id': 'fixture', 'command': 'printf /root',
                        'timeout_seconds': 30}), timeout=35)
                    message = '\n'.join(getattr(part, 'text', '') for part in result.content)
                    assert result.isError, message
                    records = [json.loads(line) for day in (profile / 'mcp-approvals' / 'audit').glob('*.jsonl')
                               for line in day.read_text(encoding='utf-8').splitlines()]
                    assert titles, 'MCP did not display a confirmation window'
                    if expected_ui == 'gpui':
                        assert titles & {'XenTerm · 操作确认', 'XenTerm · Confirmation'}, titles
                    elif expected_ui == 'native':
                        assert titles & {'XenTerm 风险操作审批', 'XenTerm Risk approval'}, titles
                    assert len(records) == 1, records
                    record = records[0]
                    assert record['outcome'] == 'denied' and record['by'] == 'auto-timeout', record
                    assert 'denied-by-timeout' in message, message
                    assert 9 <= time.monotonic() - started < 35
                    print(json.dumps({'visible_window': sorted(titles), 'outcome': record['outcome'],
                                      'decided_by': record['by'], 'elapsed_seconds': round(time.monotonic()-started, 2),
                                      'real_servers_used': False}, ensure_ascii=False))
        finally:
            stop.set()
            watcher.join(timeout=2)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', required=True, type=Path)
    parser.add_argument('--expected-ui', choices=['gpui', 'native'], required=True)
    arguments = parser.parse_args()
    asyncio.run(verify(arguments.exe, arguments.expected_ui))
