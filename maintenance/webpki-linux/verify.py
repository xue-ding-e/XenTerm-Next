import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import time
import threading
import sys

BASE = '6617bdde54e29007aafa1108105c2d4481a4ea6e'
ROOT = Path('source')
OUT = Path('evidence')
OUT.mkdir(exist_ok=True)
ASSETS = Path('harness/maintenance/webpki-linux')
MANIFEST = '343eaa943299aa4da0bf48b0f656f40904ff8f802f5bdcdc41f2b907d6388a6b'
ORIGINAL = '57093f2f0fa4c96203cf892c744303bc947873f4e015417c7fd27bac195d91af'
CANDIDATE = '0db78919f4b78b2d7c4d7c2f4ba149f06a7c0b4763759bd230a4e32130aa0393'
PATCH = '95538f44256733696b360f2850cc00bf8f52738aef70bf63184c7615a3cdcc0c'
TARGET = 'x86_64-unknown-linux-gnu'
RESULT = {'source_sha': BASE, 'harness_sha': os.environ.get('GITHUB_SHA'),
          'run_id': os.environ.get('GITHUB_RUN_ID'), 'attempt': os.environ.get('GITHUB_RUN_ATTEMPT'),
          'conclusion': 'running', 'stages': [], 'scope': 'Linux 1.99.1 desktop and 1.88.0 headless regression; no GUI or real endpoint',
          'lock_origin': {'run_id': 39787108965, 'artifact_id': 12430950399,
                         'artifact_zip_sha256': 'c5d0a10d2d585d1cc0ee35f61d0753698f72a47a223f8bc1774014b3f262044b',
                         'harness_sha': '158d63adebaf73f9000d740f90a821e65b5bcee4',
                         'candidate_lock_sha256': CANDIDATE, 'patch_sha256': PATCH}}

def save():
    (OUT / 'result.json').write_text(json.dumps(RESULT, indent=2), encoding='utf-8')

def sha(p):
    return hashlib.sha256(Path(p).read_bytes()).hexdigest()

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

def git(*args):
    return subprocess.check_output(['git', *args], cwd=ROOT, text=True, encoding='utf-8', timeout=15).strip()

def source_gate():
    assert git('rev-parse', 'HEAD') == BASE
    assert sha(ROOT / 'Cargo.toml') == MANIFEST
    assert sha(ROOT / 'Cargo.lock') == CANDIDATE
    assert git('diff', 'HEAD', '--name-only').splitlines() == ['Cargo.lock']
    assert not git('ls-files', '--others', '--exclude-standard')

def metadata(name, version, flags):
    run(name, ['cargo', '+' + version, 'metadata', '--locked', '--offline', '--format-version', '1',
               '--filter-platform', TARGET, *flags], 30)
    data = json.loads((OUT / (name + '.stdout')).read_text(encoding='utf-8'))
    package = [p for p in data['packages'] if p['name'] == 'rustls-webpki']
    assert len(package) == 1 and package[0]['version'] == '0.103.16' and package[0]['rust_version'] == '1.71'
    nodes = {n['id']: n for n in data['resolve']['nodes']}
    seen, pending, edges = set(), [data['resolve']['root']], []
    while pending:
        key = pending.pop()
        if key in seen:
            continue
        seen.add(key)
        for dep in nodes[key]['deps']:
            kinds = [k for k in dep['dep_kinds'] if k['kind'] in (None, 'build')]
            if kinds:
                edges.append({'from': key, 'to': dep['pkg'], 'kinds': kinds})
                pending.append(dep['pkg'])
    (OUT / (name + '-reachability.json')).write_text(json.dumps({'edges': edges,
        'scope': 'Target-filtered normal/build graph; may overapproximate compilation units.'}, indent=2), encoding='utf-8')
    source_gate()

try:
    assert git('rev-parse', 'HEAD') == BASE
    assert not git('status', '--porcelain')
    assert sha(ROOT / 'Cargo.toml') == MANIFEST and sha(ROOT / 'Cargo.lock') == ORIGINAL
    RESULT['harness_hashes'] = {str(p): sha(p) for p in [Path(__file__), ASSETS / 'candidate.lock.patch', Path('harness/.github/workflows/ci.yml')]}
    RESULT['overall_deadline_unix'] = float(os.environ['XENTERM_WORK_DEADLINE'])
    (OUT / 'original.Cargo.lock').write_bytes((ROOT / 'Cargo.lock').read_bytes())
    assert sha(ASSETS / 'candidate.lock.patch') == PATCH
    patch = (ASSETS / 'candidate.lock.patch').resolve()
    run('apply-check', ['git', 'apply', '--check', str(patch)], 15)
    run('apply', ['git', 'apply', str(patch)], 15)
    source_gate()
    (OUT / 'candidate.lock.patch').write_bytes(patch.read_bytes())
    for version in ('1.99.1', '1.88.0'):
        run('rustc-' + version, ['rustc', '+' + version, '-vV'], 15)
        assert 'rustc ' + version + ' ' in (OUT / ('rustc-' + version + '.stdout')).read_text(encoding='utf-8')
        run('cargo-' + version, ['cargo', '+' + version, '-V'], 15)
        assert 'cargo ' + version + ' ' in (OUT / ('cargo-' + version + '.stdout')).read_text(encoding='utf-8')
    run('fetch', ['cargo', '+1.99.1', 'fetch', '--locked', '--target', TARGET], 180)
    source_gate()
    metadata('desktop-metadata', '1.99.1', [])
    metadata('headless-metadata', '1.88.0', ['--no-default-features', '--features', 'headless'])
    common = ['--locked', '--offline', '--target', TARGET, '--jobs', '2']
    for name, version, flags, compile_timeout, passed in [
        ('desktop', '1.99.1', [], 900, 1017),
        ('headless', '1.88.0', ['--no-default-features', '--features', 'headless'], 600, 666),
    ]:
        run(name + '-compile', ['cargo', '+' + version, 'test', '--no-run', *common, *flags], compile_timeout)
        source_gate()
        if name == 'desktop':
            run('desktop-check', ['cargo', '+' + version, 'check', *common], 360)
            source_gate()
        run(name + '-tests', ['cargo', '+' + version, 'test', *common, *flags, '--', '--test-threads=1'], 180)
        source_gate()
        output = (OUT / (name + '-tests.stdout')).read_text(encoding='utf-8')
        expected = f'test result: ok. {passed} passed; 0 failed; 5 ignored; 0 measured; 0 filtered out;'
        assert output.count(expected) == 1, 'complete suite count changed or missing: ' + name
        import re
        assert sorted(re.findall(r'^test (\S+) \.\.\. ignored', output, re.M)) == ['terminal::core_tests::bench_terminal_pipeline_history_memory', 'terminal::core_tests::bench_terminal_pipeline_profiles', 'terminal::core_tests::bench_terminal_pipeline_resize', 'terminal::terminal_bench::bench_terminal_find_over_full_scrollback', 'terminal::terminal_bench::bench_terminal_render_frame']
        RESULT.setdefault('suite_counts', {})[name] = {'passed': passed, 'failed': 0, 'ignored': 5}
    RESULT['conclusion'] = 'passed'
except BaseException as exc:
    RESULT['conclusion'] = 'failed'
    RESULT['error'] = repr(exc)
    raise
finally:
    final_error = None
    try:
        RESULT['final_manifest_sha256'] = sha(ROOT / 'Cargo.toml')
        RESULT['final_lock_sha256'] = sha(ROOT / 'Cargo.lock')
        (OUT / 'candidate.Cargo.lock').write_bytes((ROOT / 'Cargo.lock').read_bytes())
        RESULT['changed_paths'] = git('diff', 'HEAD', '--name-only').splitlines()
        if RESULT.get('conclusion') == 'passed':
            source_gate()
    except BaseException as exc:
        RESULT['conclusion'] = 'failed'
        RESULT['final_evidence_error'] = repr(exc)
        RESULT.setdefault('error', repr(exc))
        final_error = exc
    save()
    inventory = {str(p.relative_to(OUT)): sha(p) for p in OUT.iterdir() if p.is_file() and p.name != 'SHA256SUMS.json'}
    (OUT / 'SHA256SUMS.json').write_text(json.dumps(inventory, indent=2), encoding='utf-8')
    if final_error is not None:
        raise RuntimeError('final source evidence failed') from final_error
