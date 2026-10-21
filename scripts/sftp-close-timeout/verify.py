"""Exact-source SFTP CLOSE timeout coverage; no production fix or external SSH."""
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess

BASE = 'd9b0605b378031288de99975f06354cfa2e5312d'
REF = 'refs/heads/test/sftp-close-timeout-20261021'
HARNESS = Path(__file__).resolve().parents[2]
ROOT = HARNESS.parent / 'source'
SOURCE = Path('src/sftp/impls/sftp.rs')
DIR = Path(__file__).resolve().parent
EVIDENCE = Path(os.environ['RUNNER_TEMP']) / 'sftp-close-timeout-evidence'
EVIDENCE.mkdir(exist_ok=True)
PREFIX = 'sftp::sftp::write_text_close_tests::'
CASES = ['close_failure_is_not_save_success', 'acknowledged_close_succeeds', 'write_failure_is_preserved', 'fsync_failure_is_preserved', 'unacknowledged_close_times_out']
IDENTITIES = {'Cargo.toml': 'daf3ed6e0d829f8a4ecc99d9d7abfe68853f95cd0f9886c43125ee1d363647de', 'Cargo.lock': '6bde5b10c9e9e55b43083cd777e74123a9f1d6247393abb03984dfa058f3ef0d', 'src/sftp/impls/sftp.rs': '838ea029fdc86fb11095a5bc8487c1badd1fe37c5a2a7795e60762a68b57fc3d'}
PATCH_IDENTITIES = {'test-only.patch': '478c31bc522cfe0c75e30d9d592e52da47e9db3e946397e3fe12cda307f21de8'}
PHASE_SOURCE = {'coverage': 'f901698507e937034955952900a7096dfb7cb69d39b2d09bf64bd2b909dbd90f'}
PRODUCTION_PREFIX = '1b7b6c31b4785692c2eb5c079c45c233c13612065e719db32081604a1942006b'
ENV = os.environ.copy()
for name in ('ALL_PROXY', 'all_proxy', 'HTTP_PROXY', 'http_proxy', 'HTTPS_PROXY', 'https_proxy'):
    ENV.pop(name, None)
ENV['CARGO_TERM_COLOR'] = 'never'
RESULT = {'base': BASE, 'harness': os.environ.get('GITHUB_SHA'), 'status': 'started', 'cases': []}


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(label, command, seconds=60, expected=0, cwd=None):
    out = EVIDENCE / (label + '.stdout.log')
    err = EVIDENCE / (label + '.stderr.log')
    with out.open('w') as stdout, err.open('w') as stderr:
        process = subprocess.Popen(command, cwd=cwd or ROOT, env=ENV, stdout=stdout,
                                   stderr=stderr, start_new_session=True)
        try:
            code = process.wait(timeout=seconds)
        except subprocess.TimeoutExpired:
            cleanup_errors = []
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except BaseException as error:
                cleanup_errors.append('kill: ' + repr(error))
            try:
                process.wait(timeout=10)
            except BaseException as error:
                cleanup_errors.append('wait: ' + repr(error))
            finally:
                (EVIDENCE / (label + '.exit.json')).write_text(json.dumps({
                    'exit': process.poll(), 'timed_out': True, 'command': command,
                    'cleanup_errors': cleanup_errors, 'termination_confirmed': process.poll() is not None,
                }))
            raise RuntimeError(label + ' timed out; cleanup errors: ' + repr(cleanup_errors))
    (EVIDENCE / (label + '.exit.json')).write_text(json.dumps({'exit': code, 'command': command}))
    if expected is not None and code != expected:
        raise RuntimeError(f'{label}: expected exit {expected}, got {code}; see logs')
    return code, out.read_text(), err.read_text()


def compile_tests(phase):
    if sha(ROOT / SOURCE) != PHASE_SOURCE[phase]:
        raise RuntimeError(phase + ': source identity mismatch')
    # Compilation must succeed before evaluating the five protocol coverage cases.
    _, output, _ = run(phase + '-compile', [
        'cargo', 'test', '--locked', '--no-default-features', '--features', 'headless',
        '--jobs', '2', '--bin', 'xenterm', '--no-run', '--message-format=json',
    ], seconds=900)
    binaries = set()
    for line in output.splitlines():
        item = json.loads(line)
        if item.get('reason') == 'compiler-artifact' and item.get('profile', {}).get('test'):
            if item.get('target', {}).get('name') == 'xenterm' and item.get('executable'):
                binaries.add(item['executable'])
    if len(binaries) != 1:
        raise RuntimeError(f'{phase}: expected one compiled test binary, got {len(binaries)}')
    binary = Path(binaries.pop()).resolve()
    if not binary.is_relative_to(ROOT / 'target') or not binary.is_file():
        raise RuntimeError('test executable outside owned target directory')
    _, listed, _ = run(phase + '-list', [str(binary), PREFIX, '--list'])
    names = {line.removesuffix(': test') for line in listed.splitlines() if line.endswith(': test')}
    if names != {PREFIX + name for name in CASES} or not re.search(r'^5 tests, 0 benchmarks$', listed, re.M):
        raise RuntimeError(f'{phase}: exact five-test inventory not verified')
    (EVIDENCE / (phase + '-identity.json')).write_text(json.dumps({
        'source_sha256': sha(ROOT / SOURCE), 'binary_sha256': sha(binary),
        'cargo_toml_sha256': sha(ROOT / 'Cargo.toml'), 'cargo_lock_sha256': sha(ROOT / 'Cargo.lock'),
        'tests': sorted(names),
    }, indent=2))
    return binary


def cases(phase, binary):
    for name in CASES:
        code, out, err = run(phase + '-' + name,
                            [str(binary), PREFIX + name, '--exact', '--nocapture', '--test-threads=1'],
                            seconds=60, expected=None)
        if code != 0 or 'test result: ok. 1 passed; 0 failed;' not in out or err.strip():
            raise RuntimeError(name + ': did not pass cleanly; fixture/cleanup failure is failure')
        RESULT['cases'].append({'name': PREFIX + name, 'exit': code})


def main():
    if (os.environ.get('GITHUB_REPOSITORY'), os.environ.get('GITHUB_REF'), os.environ.get('GITHUB_EVENT_NAME')) != (
            'xue-ding-e/XenTerm-Next', REF, 'workflow_dispatch'):
        raise RuntimeError('repository/ref/manual-event guard failed')
    _, head, _ = run('head', ['git', 'rev-parse', 'HEAD'], cwd=HARNESS)
    if head.strip() != os.environ['GITHUB_SHA']:
        raise RuntimeError('checkout does not match dispatch SHA')
    _, parent, _ = run('parent', ['git', 'rev-parse', 'HEAD^'], cwd=HARNESS)
    if parent.strip() != BASE:
        raise RuntimeError('validation must be exactly one harness commit on fixed main')
    _, status, _ = run('initial-status', ['git', 'status', '--porcelain'])
    if status:
        raise RuntimeError('initial worktree not clean')
    _, changed, _ = run('scope', ['git', 'diff', '--name-only', BASE, 'HEAD'], cwd=HARNESS)
    if set(changed.splitlines()) != {'.github/workflows/ci.yml',
            'scripts/sftp-close-timeout/verify.py', 'scripts/sftp-close-timeout/test-only.patch'}:
        raise RuntimeError('harness scope differs from reviewed three files')
    _, source_head, _ = run('source-head', ['git', 'rev-parse', 'HEAD'])
    if source_head.strip() != BASE:
        raise RuntimeError('source checkout is not exact baseline')
    for name, expected in PATCH_IDENTITIES.items():
        if sha(DIR / name) != expected:
            raise RuntimeError('patch identity mismatch: ' + name)
    for name, expected in IDENTITIES.items():
        if sha(ROOT / name) != expected:
            raise RuntimeError('identity mismatch: ' + name)
    run('rustc-version', ['rustc', '-Vv'])
    run('cargo-version', ['cargo', '-V'])
    run('apply-test-check', ['git', 'apply', '--check', str(DIR / 'test-only.patch')])
    run('apply-test', ['git', 'apply', str(DIR / 'test-only.patch')])
    prefix = (ROOT / SOURCE).read_bytes().split(b'mod write_text_close_tests {')[0]
    if hashlib.sha256(prefix).hexdigest() != PRODUCTION_PREFIX:
        raise RuntimeError('production prefix changed by test patch')
    cases('coverage', compile_tests('coverage'))
    for name in ('Cargo.toml', 'Cargo.lock'):
        if sha(ROOT / name) != IDENTITIES[name]:
            raise RuntimeError('manifest/lock changed during validation')
    run('final-diff', ['git', 'diff', '--', str(SOURCE)])
    RESULT['status'] = 'five_protocol_cases_passed'


try:
    main()
except BaseException as error:
    RESULT['status'] = 'failed'
    RESULT['error'] = str(error)
    raise
finally:
    RESULT['final_source_sha256'] = sha(ROOT / SOURCE)
    RESULT['script_sha256'] = sha(Path(__file__))
    (EVIDENCE / 'result.json').write_text(json.dumps(RESULT, indent=2))
