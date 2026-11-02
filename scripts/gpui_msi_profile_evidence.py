"""Installer-only byte preservation, separate from legitimate GUI schema writes.

Only synthetic profiles on the disposable MSI runner are passed by the harness.
All evidence files use exclusive creation: later phases cannot replace the
original pre-upgrade baseline and accidentally conceal data loss.
"""
import argparse
from contextlib import closing
import hashlib
import json
from pathlib import Path
import sqlite3


def files(root):
    root = root.resolve(strict=True)
    result = {}
    for path in sorted(root.rglob('*')):
        if path.is_symlink():
            raise ValueError('Profile evidence cannot follow links')
        if path.is_file():
            result[path.relative_to(root).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    if not result:
        raise ValueError('Cannot attest an empty profile')
    return result


def write_new(path, value):
    with path.open('x', encoding='utf-8') as stream:
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write('\n')


def capture(root, output):
    write_new(output, {'root': str(root.resolve(strict=True)), 'files': files(root)})


def verify(root, baseline):
    evidence = json.loads(baseline.read_text(encoding='utf-8'))
    if evidence['root'] != str(root.resolve(strict=True)):
        raise ValueError('Profile root differs from the immutable baseline')
    current = files(root)
    expected = evidence['files']
    if current != expected:
        changed = sorted(path for path in set(current) | set(expected)
                         if current.get(path) != expected.get(path))
        raise ValueError('Installer changed original profile bytes: ' + ', '.join(changed))


def assert_native_profile(root, require_initialized):
    """Inspect only: the genuine application, never this helper, creates schema."""
    database = root.resolve(strict=True) / 'sessions.db'
    if database.is_symlink():
        raise ValueError('Refusing linked fixture database')
    if not database.exists():
        if require_initialized:
            raise ValueError('Official CLI did not initialize the profile database')
        return
    with closing(sqlite3.connect(database.as_uri() + '?mode=ro', uri=True)) as db:
        if db.execute('SELECT count(*) FROM sessions').fetchone()[0] != 0:
            raise ValueError('Refusing a nonempty fixture profile')
        if require_initialized:
            settings = json.loads(db.execute("SELECT value FROM meta WHERE key='settings'").fetchone()[0])
            if settings.get('mcp_access_mode') != 'restricted':
                raise ValueError('Official profile must use restricted permissions')
            # Validate the genuine schema used by the existing seed, without creating it.
            db.execute('SELECT ordinal,id,data FROM sessions LIMIT 0')
            db.execute('SELECT seq,command FROM command_history LIMIT 0')


def seed(root, fixture_id):
    # Populate only the disposable GUI-created database, never an arbitrary
    # existing user profile. The harness first refuses all preexisting paths.
    database = root.resolve(strict=True) / 'sessions.db'
    with closing(sqlite3.connect(database.as_uri() + '?mode=rw', uri=True)) as db:
        if db.execute('SELECT count(*) FROM sessions').fetchone()[0] != 0:
            raise ValueError('Refusing to seed a nonempty profile')
        settings = json.loads(db.execute("SELECT value FROM meta WHERE key='settings'").fetchone()[0])
        session = dict(id=fixture_id, name='Synthetic installer preservation',
                       host='installer-test.invalid', port=22, user='synthetic',
                       auth='password', kind='ssh', group='Installer fixture')
        db.execute('INSERT INTO sessions(ordinal,id,data) VALUES(0,?,?)',
                   (fixture_id, json.dumps(session)))
        db.execute('INSERT INTO command_history(command) VALUES(?)', ('echo synthetic-installer-fixture',))
        for key in ('mcp_enabled', 'mcp_allow_commands', 'mcp_allow_file_transfers'):
            settings[key] = False
        db.execute("UPDATE meta SET value=? WHERE key='settings'", (json.dumps(settings),))
        db.commit()


def semantic(root, fixture_id):
    # Query only after the real GUI has closed. Read-only SQLite must never
    # create a missing database or repair an invalid profile to make QA green.
    database = root.resolve(strict=True) / 'sessions.db'
    with closing(sqlite3.connect(database.as_uri() + '?mode=ro', uri=True)) as db:
        rows = db.execute('SELECT id,data FROM sessions ORDER BY ordinal,id').fetchall()
        if len(rows) != 1 or rows[0][0] != fixture_id:
            raise ValueError('Expected exactly the synthetic installer session')
        value = json.loads(rows[0][1])
        fields = ('id', 'name', 'host', 'port', 'user', 'auth', 'kind', 'group')
        session = {key: value[key] for key in fields}
        history = [row[0] for row in db.execute('SELECT command FROM command_history ORDER BY seq')]
        settings = json.loads(db.execute("SELECT value FROM meta WHERE key='settings'").fetchone()[0])
        policy = {key: settings[key] for key in ('mcp_enabled', 'mcp_allow_commands', 'mcp_allow_file_transfers')}
    return {'session': session, 'history': history, 'policy': policy}


def verify_semantic(root, baseline, fixture_id, output):
    current = semantic(root, fixture_id)
    if current != json.loads(baseline.read_text(encoding='utf-8')):
        raise ValueError('GUI changed synthetic session/history/permission semantics')
    write_new(output, current)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('operation', choices=('assert-empty-or-missing', 'assert-native-initialized', 'seed', 'capture', 'verify', 'semantic', 'verify-semantic'))
    parser.add_argument('--profile', required=True, type=Path)
    parser.add_argument('--baseline', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--fixture-id')
    args = parser.parse_args()
    if args.operation in ('assert-empty-or-missing', 'assert-native-initialized'):
        assert_native_profile(args.profile, args.operation == 'assert-native-initialized')
    elif args.operation == 'seed':
        seed(args.profile, args.fixture_id)
    elif args.operation == 'capture':
        capture(args.profile, args.output)
    elif args.operation == 'verify':
        verify(args.profile, args.baseline)
    elif args.operation == 'semantic':
        write_new(args.output, semantic(args.profile, args.fixture_id))
    else:
        verify_semantic(args.profile, args.baseline, args.fixture_id, args.output)
