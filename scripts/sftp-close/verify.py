"""Exact-source SFTP CLOSE status validation using bounded in-memory protocol peers."""
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess

BASE = '7393417b28e6eb8c2e3e9bc9a8608805225f8b38'
REF = 'refs/heads/test/sftp-close-status-20261020'
HARNESS = Path(__file__).resolve().parents[2]
ROOT = HARNESS.parent / 'source'
SOURCE = Path('src/sftp/impls/sftp.rs')
DIR = Path(__file__).resolve().parent
EVIDENCE = Path(os.environ['RUNNER_TEMP']) / 'sftp-close-evidence'
EVIDENCE.mkdir(exist_ok=True)
PREFIX = 'sftp::sftp::write_text_close_tests::'
CASES = ['close_failure_is_not_save_success', 'acknowledged_close_succeeds', 'write_failure_is_preserved', 'fsync_failure_is_preserved']
IDENTITIES = {'Cargo.toml': 'daf3ed6e0d829f8a4ecc99d9d7abfe68853f95cd0f9886c43125ee1d363647de', 'Cargo.lock': '6bde5b10c9e9e55b43083cd777e74123a9f1d6247393abb03984dfa058f3ef0d', 'src/sftp/impls/sftp.rs': '6aa993d11d0bb733d1b07f065419db6ccdf95c7faa990673875f15e73dc7387d'}
PATCH_IDENTITIES = {'test-only.patch': '5865829b62af0530ab545c9275fe5afb79820d52b85db64ad809d8e7b41bb063', 'fix-only.patch': 'e8ab33de9bb4f3414eff65efa3fd3c1c4699a5ec20761c63f62ae466d1333bd4'}
PHASE_SOURCE = {'red': 'b442ad514c339458710200b0dcdf21e1526b4146e466ebf92ca1e0e44552ce33', 'green': '8e6decffd32bbec46c902ed13c472164635bc3d14bf811e7d5278ce37bfd0617'}
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
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=10)
            (EVIDENCE / (label + '.exit.json')).write_text(json.dumps({'exit': process.returncode, 'timed_out': True, 'command': command}))
            raise RuntimeError(label + ' timed out; process group killed')
    (EVIDENCE / (label + '.exit.json')).write_text(json.dumps({'exit': code, 'command': command}))
    if expected is not None and code != expected:
        raise RuntimeError(f'{label}: expected exit {expected}, got {code}; see logs')
    return code, out.read_text(), err.read_text()


def compile_tests(phase):
    if sha(ROOT / SOURCE) != PHASE_SOURCE[phase]:
        raise RuntimeError(phase + ': source identity mismatch')
    # Compilation must succeed before an assertion failure can count as RED.
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
    if names != {PREFIX + name for name in CASES} or not re.search(r'^4 tests, 0 benchmarks$', listed, re.M):
        raise RuntimeError(f'{phase}: exact four-test inventory not verified')
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
                            seconds=45, expected=None)
        combined = out + err
        if phase == 'red' and name == 'close_failure_is_not_save_success':
            if code != 101 or 'test result: FAILED. 0 passed; 1 failed;' not in out:
                raise RuntimeError(name + ': not an exact one-case RED failure')
            if err.strip() != 'Error: CLOSE failure was reported as save success':
                raise RuntimeError(name + ': RED error differs from the expected save-success contract violation')
        elif code != 0 or 'test result: ok. 1 passed; 0 failed;' not in out or err.strip():
            raise RuntimeError(name + ': control/GREEN did not pass cleanly')
        RESULT['cases'].append({'phase': phase, 'name': PREFIX + name, 'exit': code})


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
            'scripts/sftp-close/verify.py', 'scripts/sftp-close/test-only.patch',
            'scripts/sftp-close/fix-only.patch'}:
        raise RuntimeError('harness scope differs from reviewed four files')
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
    cases('red', compile_tests('red'))
    run('apply-fix-check', ['git', 'apply', '--check', str(DIR / 'fix-only.patch')])
    run('apply-fix', ['git', 'apply', str(DIR / 'fix-only.patch')])
    cases('green', compile_tests('green'))
    for name in ('Cargo.toml', 'Cargo.lock'):
        if sha(ROOT / name) != IDENTITIES[name]:
            raise RuntimeError('manifest/lock changed during validation')
    run('final-diff', ['git', 'diff', '--', str(SOURCE)])
    RESULT['status'] = 'one_expected_red_three_controls_then_four_green'


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
