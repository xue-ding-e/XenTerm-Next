"""Isolated Rust 1.88 compatibility feasibility experiment; not a product change."""
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess

BASE = '16ccb477efe01445c784d4c2ace198c4830f0b9a'
REF = 'refs/heads/test/msrv-compat-188-20261021'
HERE = Path(__file__).resolve().parent
HARNESS = HERE.parents[1]
SOURCE = HARNESS.parent / 'source'
OUT = Path(os.environ['RUNNER_TEMP']) / 'msrv-compat-evidence'
OUT.mkdir(exist_ok=True)
HASHES = {'Cargo.toml': 'daf3ed6e0d829f8a4ecc99d9d7abfe68853f95cd0f9886c43125ee1d363647de', 'Cargo.lock': '6bde5b10c9e9e55b43083cd777e74123a9f1d6247393abb03984dfa058f3ef0d', 'src/ssh/impls/ssh.rs': '1d47b11f33874fa9fd39916ff0075c9c95030f87bec1569e76835a5d0c226ecc'}
RESULT = {'base': BASE, 'harness': os.environ.get('GITHUB_SHA'), 'status': 'started'}


def run(label, args, seconds=30, expected=0, cwd=SOURCE):
    with (OUT / (label + '.stdout')).open('w') as out, (OUT / (label + '.stderr')).open('w') as err:
        p = subprocess.Popen(args, cwd=cwd, stdout=out, stderr=err, start_new_session=True)
        try:
            code = p.wait(timeout=seconds)
        except subprocess.TimeoutExpired:
            errors = []
            try:
                os.killpg(p.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except BaseException as e:
                errors.append(repr(e))
            try:
                p.wait(timeout=10)
            except BaseException as e:
                errors.append(repr(e))
            (OUT / (label + '.exit.json')).write_text(json.dumps({'exit': p.poll(), 'timed_out': True, 'cleanup_errors': errors}))
            raise RuntimeError(label + ' timeout')
    (OUT / (label + '.exit.json')).write_text(json.dumps({'exit': code, 'args': args}))
    text = (OUT / (label + '.stdout')).read_text()
    stderr = (OUT / (label + '.stderr')).read_text()
    if expected is not None and code != expected:
        raise RuntimeError(label + ': unexpected exit ' + str(code))
    return code, text, stderr


def main():
    assert (os.environ['GITHUB_REPOSITORY'], os.environ['GITHUB_REF'], os.environ['GITHUB_EVENT_NAME']) == ('xue-ding-e/XenTerm-Next', REF, 'workflow_dispatch')
    assert run('harness-parents', ['git', 'rev-list', '--parents', '-n', '1', 'HEAD'], cwd=HARNESS)[1].split() == [os.environ['GITHUB_SHA'], BASE]
    assert set(run('harness-scope', ['git', 'diff', '--name-only', BASE, 'HEAD'], cwd=HARNESS)[1].splitlines()) == {'.github/workflows/ci.yml', 'scripts/msrv-compat/verify.py', 'scripts/msrv-compat/helper-only.patch'}
    assert run('source-head', ['git', 'rev-parse', 'HEAD'])[1].strip() == BASE
    assert not run('source-clean', ['git', 'status', '--porcelain'])[1]
    for name, digest in HASHES.items():
        assert hashlib.sha256((SOURCE / name).read_bytes()).hexdigest() == digest
    assert hashlib.sha256((HERE / 'helper-only.patch').read_bytes()).hexdigest() == '81269ee024711a9073bf779268ad24dc56ca5b0aeb9bb7123150db6918d7f951'
    assert run('rustc-188-version', ['rustc', '+1.88.0', '-Vv'])[1].startswith('rustc 1.88.0 ')
    run('cargo-stable-version', ['cargo', '+stable', '-V'])
    before_lock = (SOURCE / 'Cargo.lock').read_bytes()
    assert before_lock.count(b'aes 0.9.3') == 4
    old_package = b'name = "aes"\nversion = "0.9.3"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "35f0f96ce78e38c3dc6d8948aa8163d06385be74000f3c7a95bf1eef35d3ea32"'
    new_package = b'name = "aes"\nversion = "0.9.2"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "f8eb277bec05f56a0e0591f155a484cbd0f4f07ff2905051a48c72f004f7ed58"'
    assert before_lock.count(old_package) == 1
    expected_lock = before_lock.replace(old_package, new_package).replace(b'aes 0.9.3', b'aes 0.9.2')
    run('apply-helper', ['git', 'apply', str(HERE / 'helper-only.patch')])
    assert hashlib.sha256((SOURCE / 'src/ssh/impls/ssh.rs').read_bytes()).hexdigest() == 'f401f9c426c50cd0d9b7378d689084c7d59e6c599a07e1a8736fe092e4549756'
    # Cargo performs the resolution. Never hand-edit or relax this exact lock gate.
    run('precise-aes-update', ['cargo', '+stable', 'update', '-p', 'aes@0.9.3', '--precise', '0.9.2'], seconds=180)
    run('resolved-diff', ['git', 'diff', '--', 'Cargo.lock', 'src/ssh/impls/ssh.rs'])
    actual_lock = (SOURCE / 'Cargo.lock').read_bytes()
    (OUT / 'resolved-Cargo.lock').write_bytes(actual_lock)
    assert actual_lock == expected_lock, 'Cargo changed more than the one aes package and its four references; stop'
    assert hashlib.sha256((SOURCE / 'Cargo.toml').read_bytes()).hexdigest() == HASHES['Cargo.toml']
    RESULT['candidate_lock_sha256'] = hashlib.sha256(actual_lock).hexdigest()
    RESULT['candidate_source_sha256'] = hashlib.sha256((SOURCE / 'src/ssh/impls/ssh.rs').read_bytes()).hexdigest()
    # A metadata parse failure is a real stop, not permission to force a build.
    _, metadata, _ = run('metadata-188', ['cargo', '+1.88.0', 'metadata', '--locked', '--format-version=1', '--filter-platform', 'x86_64-unknown-linux-gnu', '--no-default-features', '--features', 'headless'], seconds=180)
    data = json.loads(metadata)
    nodes = {n['id']: n for n in data['resolve']['nodes']}
    packages = {p['id']: p for p in data['packages']}
    reached, edges, todo = set(), [], [data['resolve']['root']]
    while todo:
        ident = todo.pop()
        if ident in reached:
            continue
        reached.add(ident)
        for dep in nodes[ident]['deps']:
            edges.append({'from': ident, 'to': dep['pkg'], 'dep_kinds': dep['dep_kinds']})
            todo.append(dep['pkg'])
    records = [{'id': ident, 'name': packages[ident]['name'], 'version': packages[ident]['version'], 'rust_version': packages[ident].get('rust_version')} for ident in sorted(reached)]
    (OUT / 'dependency-audit.json').write_text(json.dumps({'reachable': records, 'edges': edges, 'scope': 'Includes dev/build/target-labeled edges. Unknown rust_version is not compatibility. Linux headless only.'}, indent=2))
    args = ['cargo', '+1.88.0', 'test', '--locked', '--no-default-features', '--features', 'headless', '--jobs', '2', '--bin', 'xenterm']
    _, compiled, _ = run('compile-188', args + ['--no-run', '--message-format=json'], seconds=900)
    bins = [m['executable'] for line in compiled.splitlines() if line.startswith('{') for m in [json.loads(line)] if m.get('reason') == 'compiler-artifact' and m.get('profile', {}).get('test') and m.get('executable') and m.get('target', {}).get('name') == 'xenterm']
    assert len(set(bins)) == 1, bins
    binary = bins[0]
    prefix = 'ssh::ssh::utf8_boundary_tests::'
    names = [prefix + n for n in ['ascii_boundaries_are_unchanged', 'multibyte_boundaries_round_up_and_trigger_tail_stays_valid', 'out_of_range_and_empty_indices_clamp_to_length']]
    _, listing, _ = run('list-helper-tests', [binary, prefix, '--list'])
    assert {s.removesuffix(': test') for s in listing.splitlines() if s.endswith(': test')} == set(names)
    for index, name in enumerate(names):
        _, output, _ = run('helper-case-' + str(index), [binary, name, '--exact', '--nocapture', '--test-threads=1'], seconds=45)
        assert 'test result: ok. 1 passed; 0 failed;' in output
    assert set(run('final-scope', ['git', 'diff', '--name-only'])[1].splitlines()) == {'Cargo.lock', 'src/ssh/impls/ssh.rs'}
    assert (SOURCE / 'Cargo.lock').read_bytes() == expected_lock
    assert hashlib.sha256((SOURCE / 'Cargo.toml').read_bytes()).hexdigest() == HASHES['Cargo.toml']
    assert hashlib.sha256((SOURCE / 'src/ssh/impls/ssh.rs').read_bytes()).hexdigest() == RESULT['candidate_source_sha256']
    RESULT['status'] = 'linux_headless_test_binary_188_and_three_helper_cases_passed_not_all_targets'


try:
    main()
except BaseException as error:
    RESULT['status'] = 'failed'
    RESULT['error'] = repr(error)
    raise
finally:
    RESULT['script_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    RESULT['patch_sha256'] = hashlib.sha256((HERE / 'helper-only.patch').read_bytes()).hexdigest()
    (OUT / 'result.json').write_text(json.dumps(RESULT, indent=2))
