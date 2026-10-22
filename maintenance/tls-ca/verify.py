import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import time
import threading
import sys

BASE = '05b77b961bf0e314db4eec2a34c45723d4e11aa5'
ROOT = Path('source')
OUT = Path('evidence')
OUT.mkdir(exist_ok=True)
EXPECTED = {
    'Cargo.toml': '343eaa943299aa4da0bf48b0f656f40904ff8f802f5bdcdc41f2b907d6388a6b',
    'Cargo.lock': 'b9726bfc1a44a40084cf83feec3f4a330ac9dc9fe55d9a13efe7e64c53292bb4',
}
RESULT = {'harness_sha': os.environ.get('GITHUB_SHA'), 'run_id': os.environ.get('GITHUB_RUN_ID'), 'run_attempt': os.environ.get('GITHUB_RUN_ATTEMPT'), 'source_sha': BASE, 'target': 'x86_64-unknown-linux-gnu', 'features': ['headless without defaults', 'default desktop'], 'stages': []}

RESULT['security_scope'] = {'RUSTSEC-2026-0381': 'roots 1.0.10 candidate; not applied to main', 'RUSTSEC-2026-0382': 'not_fixed', 'rustls_webpki_retained': '0.103.15'}

def save():
    (OUT / 'result.json').write_text(json.dumps(RESULT, indent=2))


def hashes():
    return {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in EXPECTED}


def run(name, command, seconds):
    started = time.monotonic()
    remaining = float(os.environ['XENTERM_WORK_DEADLINE']) - time.time()
    if remaining <= 0:
        raise TimeoutError('overall 55-minute work deadline exhausted before ' + name)
    seconds = min(seconds, remaining)
    record = {'name': name, 'command': command, 'timeout_seconds': seconds}
    RESULT['stages'].append(record)
    save()
    print('STAGE START', name, 'timeout_seconds=', round(seconds, 2), flush=True)
    proc = subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, start_new_session=True)
    errors = []
    old_handlers = {sig: signal.getsignal(sig) for sig in (signal.SIGTERM, signal.SIGINT)}

    def kill_owned(sig):
        try:
            os.killpg(proc.pid, sig)
        except ProcessLookupError:
            pass

    def interrupted(signum, frame):
        record['interrupted_signal'] = signum
        raise RuntimeError('interrupted by signal ' + str(signum))

    def drain(stream, suffix):
        written = 0
        exceeded = False
        try:
            with (OUT / (name + suffix)).open('wb') as dest:
                while True:
                    chunk = stream.read(8192)
                    if not chunk:
                        break
                    retained = chunk[:max(0, 64 * 1024 * 1024 - written)]
                    dest.write(retained)
                    dest.flush()
                    written += len(retained)
                    if len(retained) != len(chunk) and not exceeded:
                        exceeded = True
                        errors.append('output limit exceeded: ' + suffix)
                        kill_owned(signal.SIGKILL)
        except Exception as exc:
            errors.append('drain error: ' + repr(exc))
            kill_owned(signal.SIGKILL)
        finally:
            stream.close()

    threads = [threading.Thread(target=drain, args=(proc.stdout, '.stdout'), daemon=True),
               threading.Thread(target=drain, args=(proc.stderr, '.stderr'), daemon=True)]
    for sig in old_handlers:
        signal.signal(sig, interrupted)
    for thread in threads:
        thread.start()
    pending_error = None
    try:
        record['exit_code'] = proc.wait(timeout=seconds)
    except subprocess.TimeoutExpired as exc:
        record['timed_out'] = True
        pending_error = exc
    except Exception as exc:
        pending_error = exc
    finally:
        try:
            try:
                os.killpg(proc.pid, 0)
            except ProcessLookupError:
                record['owned_group_gone'] = True
            else:
                record['owned_group_cleanup_required'] = True
                kill_owned(signal.SIGTERM)
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    pass
                kill_owned(signal.SIGKILL)
            record['exit_code'] = proc.wait(timeout=10)
            record['leader_reaped'] = True
        except Exception as exc:
            record['cleanup_error'] = repr(exc)
            pending_error = pending_error or exc
        finally:
            for sig, handler in old_handlers.items():
                signal.signal(sig, handler)
            for thread in threads:
                thread.join(timeout=5)
            record['drain_errors'] = errors
            record['drain_threads_finished'] = all(not t.is_alive() for t in threads)
            record['seconds'] = round(time.monotonic() - started, 3)
            if pending_error:
                record['stage_error'] = repr(pending_error)
            save()
            print('STAGE END', name, json.dumps(record), flush=True)
    if (pending_error or record.get('exit_code') != 0 or errors
            or record.get('owned_group_cleanup_required')
            or not record['drain_threads_finished']):
        raise RuntimeError(name + ' failed; see exact bounded stdout/stderr and result') from pending_error


PATCHES = [
    ('webpki-roots', '1.0.7', '1.0.10', '52f5ee44c96cf55f1b349600768e3ece3a8f26010c05265ab73f945bb1a2eb9d', '1c1c8e20c8a38fc7d65abfe74eb7280e4aa667e8eec8bc84996d2874810748d9'),
]


def expected_lock(original, patches):
    # This is an allowlist comparison only. Cargo alone writes Cargo.lock.
    expected = original
    for name, old, new, old_checksum, checksum in patches:
        before = f'name = "{name}"\nversion = "{old}"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "{old_checksum}"'
        after = f'name = "{name}"\nversion = "{new}"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "{checksum}"'
        assert expected.count(before) == 1
        expected = expected.replace(before, after, 1)
        if name == 'webpki-roots':
            assert expected.count('"webpki-roots 1.0.7"') == 1
            expected = expected.replace('"webpki-roots 1.0.7"', '"webpki-roots 1.0.10"', 1)
    return expected


def verify_source(expected):
    assert (ROOT / 'Cargo.toml').read_bytes() == manifest_bytes
    assert (ROOT / 'Cargo.lock').read_text() == expected, 'Cargo changed an unapproved lock entry'
    changed = subprocess.check_output(['git', 'diff', 'HEAD', '--name-only'], cwd=ROOT, text=True, timeout=15).splitlines()
    assert changed == ['Cargo.lock'], changed
    assert not subprocess.check_output(['git', 'ls-files', '--others', '--exclude-standard'], cwd=ROOT, text=True, timeout=15).strip()


def metadata(name, toolchain, headless):
    command = ['cargo', '+' + toolchain, 'metadata', '--locked', '--format-version', '1', '--filter-platform', RESULT['target']]
    if headless:
        command += ['--no-default-features', '--features', 'headless']
    run(name, command, 180)
    data = json.loads((OUT / (name + '.stdout')).read_text())
    packages = {p['name']: p for p in data['packages'] if p['name'] in ['rustls-webpki'] or (p['name'] == 'webpki-roots' and p['version'].startswith('1.'))}
    for package, _, version, _, _ in PATCHES:
        assert packages[package]['version'] == version
        assert packages[package]['rust_version'] == '1.71'
    assert [p['version'] for p in data['packages'] if p['name'] == 'rustls-webpki'] == ['0.103.15'], 'webpki must remain unchanged and not_fixed'
    nodes = {n['id']: n for n in data['resolve']['nodes']}
    report, todo, seen = [], [data['resolve']['root']], set()
    while todo:
        ident = todo.pop()
        if ident in seen:
            continue
        seen.add(ident)
        for dep in nodes[ident]['deps']:
            kinds = [k for k in dep['dep_kinds'] if k['kind'] in (None, 'build')]
            if kinds:
                report.append({'from': ident, 'to': dep['pkg'], 'kinds': kinds})
                todo.append(dep['pkg'])
    (OUT / (name + '-reachability.json')).write_text(json.dumps({'edges': report, 'scope': 'Target-filtered normal/build graph, may overapproximate compilation units; not all resolve nodes.'}, indent=2))


manifest_bytes = (ROOT / 'Cargo.toml').read_bytes()
original_lock = (ROOT / 'Cargo.lock').read_text()
final_expected = expected_lock(original_lock, PATCHES)
try:
    assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True, timeout=15).strip() == BASE
    RESULT['harness_hashes'] = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in [Path(__file__), Path('harness/.github/workflows/ci.yml')]}
    RESULT['overall_deadline_unix'] = float(os.environ['XENTERM_WORK_DEADLINE'])
    (OUT / 'original-Cargo.lock').write_text(original_lock)
    RESULT['before_hashes'] = hashes()
    assert RESULT['before_hashes'] == EXPECTED
    assert not subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT, text=True, timeout=15).strip()
    for version in ['1.88.0', '1.99.1']:
        run('rustc-' + version, ['rustc', '+' + version, '-vV'], 30)
        assert 'rustc ' + version + ' ' in (OUT / ('rustc-' + version + '.stdout')).read_text()
    for i, (name, old, new, _, checksum) in enumerate(PATCHES):
        run('resolve-' + name, ['cargo', '+1.88.0', 'update', '-p', name + '@' + old, '--precise', new], 180)
        verify_source(expected_lock(original_lock, PATCHES[:i + 1]))
    (OUT / 'Cargo.lock').write_bytes((ROOT / 'Cargo.lock').read_bytes())
    (OUT / 'lock.patch').write_bytes(subprocess.check_output(['git', 'diff', '--', 'Cargo.lock'], cwd=ROOT, timeout=15))
    metadata('metadata-headless', '1.88.0', True)
    verify_source(final_expected)
    metadata('metadata-desktop', '1.99.1', False)
    verify_source(final_expected)
    # Fetch verifies official Cargo registry checksums; no alternate registry or mirror.
    run('fetch', ['cargo', '+1.88.0', 'fetch', '--locked', '--target', RESULT['target']], 300)
    import tarfile
    cache = Path.home() / '.cargo' / 'registry' / 'cache'
    for name, _, new, _, checksum in PATCHES:
        archives = list(cache.glob('index.crates.io-*' + '/' + name + '-' + new + '.crate'))
        assert len(archives) == 1, str(archives)
        blob = archives[0].read_bytes()
        assert hashlib.sha256(blob).hexdigest() == checksum
        (OUT / (name + '-' + new + '.crate')).write_bytes(blob)
        with tarfile.open(archives[0], 'r:gz') as archive:
            cargo_toml = archive.extractfile(name + '-' + new + '/Cargo.toml').read()
        assert b'rust-version = "1.71"' in cargo_toml
    run('test-headless', ['cargo', '+1.88.0', 'test', '--locked', '--no-default-features', '--features', 'headless', '--jobs', '2', '--', '--test-threads=1'], 900)
    verify_source(final_expected)
    run('test-desktop', ['cargo', '+1.99.1', 'test', '--locked', '--target', RESULT['target'], '--jobs', '2', '--', '--test-threads=1'], 900)
    verify_source(final_expected)
    RESULT['conclusion'] = 'passed: exact roots-only resolution and Linux headless/desktop regression; RUSTSEC-2026-0382 not_fixed; not native GUI or real endpoint verification'
except Exception as exc:
    RESULT['conclusion'] = 'failed'
    RESULT['error'] = repr(exc)
    raise
finally:
    final_error = None
    try:
        RESULT['after_hashes'] = hashes()
        (OUT / 'Cargo.lock').write_bytes((ROOT / 'Cargo.lock').read_bytes())
        RESULT['changed_paths'] = subprocess.check_output(['git', 'diff', 'HEAD', '--name-only'], cwd=ROOT, text=True, timeout=15).splitlines()
        assert RESULT['after_hashes']['Cargo.toml'] == EXPECTED['Cargo.toml'], 'manifest changed'
        RESULT['lock_diff'] = subprocess.check_output(['git', 'diff', '--', 'Cargo.lock'], cwd=ROOT, text=True, timeout=15)
        if RESULT.get('conclusion', '').startswith('passed'):
            verify_source(final_expected)
    except Exception as exc:
        RESULT['conclusion'] = 'failed'
        RESULT['final_verification_error'] = repr(exc)
        RESULT.setdefault('error', repr(exc))
        final_error = exc
    save()
    inventory = {str(p.relative_to(OUT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in OUT.iterdir() if p.is_file() and p.name != 'SHA256SUMS.json'}
    (OUT / 'SHA256SUMS.json').write_text(json.dumps(inventory, indent=2))
    if final_error is not None:
        raise RuntimeError('final source verification failed') from final_error
