import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import time
import threading
import sys

BASE = 'ec516928b618e0cbd8c70bd79fa0bb7378690274'
ROOT = Path('source')
OUT = Path('evidence')
OUT.mkdir(exist_ok=True)
EXPECTED = {
    'Cargo.toml': 'daf3ed6e0d829f8a4ecc99d9d7abfe68853f95cd0f9886c43125ee1d363647de',
    'Cargo.lock': 'b9726bfc1a44a40084cf83feec3f4a330ac9dc9fe55d9a13efe7e64c53292bb4',
}
RESULT = {'source_sha': BASE, 'target': 'x86_64-unknown-linux-gnu', 'features': 'default', 'stages': []}


def save():
    (OUT / 'result.json').write_text(json.dumps(RESULT, indent=2))


def hashes():
    return {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in EXPECTED}


def run(name, command, seconds):
    started = time.monotonic()
    record = {'name': name, 'command': command, 'timeout_seconds': seconds}
    RESULT['stages'].append(record)
    save()
    with (OUT / (name + '.stdout')).open('wb') as stdout, (OUT / (name + '.stderr')).open('wb') as stderr:
        proc = subprocess.Popen(command, cwd=ROOT, stdout=stdout, stderr=stderr, start_new_session=True)
        stop = threading.Event()
        mirror_errors = []

        def mirror(path):
            # Read regular log files, not child pipes: logging cannot prevent
            # the child from draining stdout/stderr. Metadata JSON stays in file.
            try:
                with path.open('rb') as stream:
                    while True:
                        chunk = stream.read(8192)
                        if chunk:
                            console = sys.stderr if path.suffix == '.stderr' else sys.stdout
                            console.buffer.write(chunk)
                            console.buffer.flush()
                        elif stop.is_set():
                            break
                        else:
                            stop.wait(0.2)
            except Exception as exc:
                mirror_errors.append(repr(exc))

        paths = [OUT / (name + '.stderr')]
        if name != 'metadata':
            paths.append(OUT / (name + '.stdout'))
        mirrors = [threading.Thread(target=mirror, args=(path,), daemon=True) for path in paths]
        for thread in mirrors:
            thread.start()
        try:
            code = proc.wait(timeout=seconds)
        except subprocess.TimeoutExpired:
            record['timed_out'] = True
            try:
                os.killpg(proc.pid, signal.SIGKILL)
                code = proc.wait(timeout=10)
                record['timeout_child_reaped'] = True
            except Exception as exc:
                record['cleanup_error'] = repr(exc)
                save()
                raise
        finally:
            stop.set()
            for thread in mirrors:
                thread.join(timeout=2)
            record['console_mirror_errors'] = mirror_errors
            record['console_mirror_threads_finished'] = all(not t.is_alive() for t in mirrors)
            save()
        record.update(exit_code=code, seconds=round(time.monotonic() - started, 3))
        save()
        if code != 0 or record.get('timed_out') or mirror_errors or not record['console_mirror_threads_finished']:
            raise RuntimeError(name + ' failed; see exact stdout/stderr')


try:
    actual = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    assert actual == BASE, actual
    RESULT['before_hashes'] = hashes()
    assert RESULT['before_hashes'] == EXPECTED
    run('rustc', ['rustc', '+1.88.0', '-vV'], 30)
    run('cargo', ['cargo', '+1.88.0', '-V'], 30)
    assert 'rustc 1.88.0 ' in (OUT / 'rustc.stdout').read_text()
    run('metadata', ['cargo', '+1.88.0', 'metadata', '--locked', '--format-version', '1', '--filter-platform', RESULT['target']], 300)
    data = json.loads((OUT / 'metadata.stdout').read_text())
    nodes = {n['id']: n for n in data['resolve']['nodes']}
    packages = {p['id']: p for p in data['packages']}
    root = data['resolve']['root']
    assert root in nodes
    seen, todo, edges = set(), [root], []
    while todo:
        package = todo.pop()
        if package in seen:
            continue
        seen.add(package)
        for dep in nodes[package]['deps']:
            kinds = [k for k in dep['dep_kinds'] if k['kind'] in (None, 'build')]
            if kinds:
                edges.append({'from': package, 'to': dep['pkg'], 'dep_kinds': kinds})
                todo.append(dep['pkg'])
    report = {'target': RESULT['target'], 'scope': 'Default-feature filtered metadata normal/build graph; not all resolve nodes. Feature unification may overapproximate cargo check units; compile is authoritative.', 'packages': [{'id': p, 'rust_version': packages[p]['rust_version'], 'features': nodes[p]['features']} for p in sorted(seen)], 'edges': edges}
    (OUT / 'reachability.json').write_text(json.dumps(report, indent=2))
    assert hashes() == EXPECTED, 'metadata changed manifests'
    run('check', ['cargo', '+1.88.0', 'check', '--locked', '--target', RESULT['target'], '--jobs', '2'], 1200)
    RESULT['conclusion'] = 'passed: Linux default-feature cargo check only; no linking or runtime validation'
except Exception as exc:
    RESULT['conclusion'] = 'failed'
    RESULT['error'] = repr(exc)
    raise
finally:
    final_error = None
    try:
        RESULT['after_hashes'] = hashes()
        RESULT['manifests_unchanged'] = RESULT['after_hashes'] == EXPECTED
        if not RESULT['manifests_unchanged']:
            raise RuntimeError('manifest identity changed')
    except Exception as exc:
        final_error = exc
        RESULT['conclusion'] = 'failed'
        RESULT['final_verification_error'] = repr(exc)
        RESULT.setdefault('error', repr(exc))
    save()
    if final_error is not None:
        raise RuntimeError('final manifest verification failed') from final_error
