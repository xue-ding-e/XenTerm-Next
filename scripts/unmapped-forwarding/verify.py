"""One-shot exact-source russh 0.61 accounting RED/GREEN validation; no network fixtures."""
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess

BASE = '77ce7313b71d176c26f62ec199b34aaad91c2251'
REF = 'refs/heads/test/unmapped-forwarding-061-20261020'
ROOT = Path(__file__).resolve().parents[2]
SOURCE = Path('src/ssh/impls/ssh.rs')
DIR = Path(__file__).resolve().parent
EVIDENCE = Path(os.environ['RUNNER_TEMP']) / 'unmapped-forwarding-evidence'
EVIDENCE.mkdir(exist_ok=True)
PREFIX = 'ssh::ssh::unmapped_forwarding_tests::'
CASES = {
    'unmapped_channel_does_not_create_counter': '(0, None), observed (1, Some(1))',
    'unmapped_channel_preserves_zero_counter': '(1, Some(0)), observed (1, Some(1))',
    'unmapped_channel_preserves_existing_counter': '(1, Some(7)), observed (1, Some(8))',
}
IDENTITIES = {
    'Cargo.toml': 'daf3ed6e0d829f8a4ecc99d9d7abfe68853f95cd0f9886c43125ee1d363647de',
    'Cargo.lock': '6bde5b10c9e9e55b43083cd777e74123a9f1d6247393abb03984dfa058f3ef0d',
    str(SOURCE): '5ba6d78ec7f29e6a1eaad50901280b9ac8a9f0625fc2324c2dcaa32403c424c5',
    'scripts/unmapped-forwarding/test-only.patch': 'b634e8adf6ba1b0725e2ba360e29530b19406eb5009879db34e98915aa2fb07e',
    'scripts/unmapped-forwarding/fix-only.patch': '5db2f993dc8f37fafd1b38f535f4fc249ba9730f7878c312c76f0c086c7338c3',
}
ENV = os.environ.copy()
for name in ('ALL_PROXY', 'all_proxy', 'HTTP_PROXY', 'http_proxy', 'HTTPS_PROXY', 'https_proxy'):
    ENV.pop(name, None)
ENV['CARGO_TERM_COLOR'] = 'never'
RESULT = {'base': BASE, 'harness': os.environ.get('GITHUB_SHA'), 'status': 'started', 'cases': []}


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(label, command, seconds=60, expected=0):
    out = EVIDENCE / (label + '.stdout.log')
    err = EVIDENCE / (label + '.stderr.log')
    with out.open('w') as stdout, err.open('w') as stderr:
        process = subprocess.Popen(command, cwd=ROOT, env=ENV, stdout=stdout,
                                   stderr=stderr, start_new_session=True)
        try:
            code = process.wait(timeout=seconds)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=10)
            raise RuntimeError(label + ' timed out; process group killed')
    (EVIDENCE / (label + '.exit.json')).write_text(json.dumps({'exit': code, 'command': command}))
    if expected is not None and code != expected:
        raise RuntimeError(f'{label}: expected exit {expected}, got {code}; see logs')
    return code, out.read_text(), err.read_text()


def compile_tests(phase):
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
    if names != {PREFIX + name for name in CASES} or not re.search(r'^3 tests, 0 benchmarks$', listed, re.M):
        raise RuntimeError(f'{phase}: exact three-test inventory not verified')
    (EVIDENCE / (phase + '-identity.json')).write_text(json.dumps({
        'source_sha256': sha(ROOT / SOURCE), 'binary_sha256': sha(binary),
        'cargo_toml_sha256': sha(ROOT / 'Cargo.toml'), 'cargo_lock_sha256': sha(ROOT / 'Cargo.lock'),
        'tests': sorted(names),
    }, indent=2))
    return binary


def cases(phase, binary):
    for name, observed in CASES.items():
        code, out, err = run(phase + '-' + name,
                            [str(binary), PREFIX + name, '--exact', '--nocapture', '--test-threads=1'],
                            seconds=60, expected=None)
        combined = out + err
        if phase == 'red':
            if code != 101 or 'test result: FAILED. 0 passed; 1 failed;' not in out:
                raise RuntimeError(name + ': not an exact one-case RED assertion failure')
            if 'unmapped forwarding changed accounting: expected ' + observed not in combined:
                raise RuntimeError(name + ': RED did not contain the expected accounting observation')
            if combined.count('Error: ') != 1 or 'cleanup failed' in combined or 'timed out' in combined:
                raise RuntimeError(name + ': RED had unexpected error or cleanup failure')
        elif code != 0 or 'test result: ok. 1 passed; 0 failed;' not in out or 'Error: ' in combined:
            raise RuntimeError(name + ': GREEN did not pass cleanly')
        RESULT['cases'].append({'phase': phase, 'name': PREFIX + name, 'exit': code})


def main():
    if (os.environ.get('GITHUB_REPOSITORY'), os.environ.get('GITHUB_REF'), os.environ.get('GITHUB_EVENT_NAME')) != (
            'xue-ding-e/XenTerm-Next', REF, 'workflow_dispatch'):
        raise RuntimeError('repository/ref/manual-event guard failed')
    _, head, _ = run('head', ['git', 'rev-parse', 'HEAD'])
    if head.strip() != os.environ['GITHUB_SHA']:
        raise RuntimeError('checkout does not match dispatch SHA')
    _, parent, _ = run('parent', ['git', 'rev-parse', 'HEAD^'])
    if parent.strip() != BASE:
        raise RuntimeError('validation must be exactly one harness commit on fixed main')
    _, status, _ = run('initial-status', ['git', 'status', '--porcelain'])
    if status:
        raise RuntimeError('initial worktree not clean')
    _, changed, _ = run('scope', ['git', 'diff', '--name-only', BASE, 'HEAD'])
    if set(changed.splitlines()) != {'.github/workflows/ci.yml',
            'scripts/unmapped-forwarding/verify.py', 'scripts/unmapped-forwarding/test-only.patch',
            'scripts/unmapped-forwarding/fix-only.patch'}:
        raise RuntimeError('harness scope differs from reviewed four files')
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
    RESULT['status'] = 'three_expected_red_then_three_green'


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
