"""Read-only dependency metadata and isolated standard-library MSRV diagnostic."""
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess

BASE = '16ccb477efe01445c784d4c2ace198c4830f0b9a'
REF = 'refs/heads/test/msrv-probe-20261021'
HERE = Path(__file__).resolve().parent
HARNESS = HERE.parents[1]
SOURCE = HARNESS.parent / 'source'
OUT = Path(os.environ['RUNNER_TEMP']) / 'msrv-probe-evidence'
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
    assert run('harness-head', ['git', 'rev-parse', 'HEAD'], cwd=HARNESS)[1].strip() == os.environ['GITHUB_SHA']
    assert run('harness-parents', ['git', 'rev-list', '--parents', '-n', '1', 'HEAD'], cwd=HARNESS)[1].split() == [os.environ['GITHUB_SHA'], BASE]
    expected_files = {'.github/workflows/ci.yml', 'scripts/msrv-probe/verify.py', 'scripts/msrv-probe/original.rs', 'scripts/msrv-probe/helper.rs'}
    assert set(run('harness-scope', ['git', 'diff', '--name-only', BASE, 'HEAD'], cwd=HARNESS)[1].splitlines()) == expected_files
    assert run('source-head', ['git', 'rev-parse', 'HEAD'])[1].strip() == BASE
    assert not run('source-clean', ['git', 'status', '--porcelain'])[1]
    for name, digest in HASHES.items():
        assert hashlib.sha256((SOURCE / name).read_bytes()).hexdigest() == digest
    src = (SOURCE / 'src/ssh/impls/ssh.rs').read_text()
    excerpt = '                let split = trigger.buffer.len() - keep;\n                let split = trigger.buffer.ceil_char_boundary(split);'
    assert src.count(excerpt) == 1
    (OUT / 'source-excerpt.json').write_text(json.dumps({'source': 'src/ssh/impls/ssh.rs', 'line': src[:src.index(excerpt)].count('\n') + 1, 'excerpt': excerpt, 'hashes': HASHES}))
    assert run('rustc-188-version', ['rustc', '+1.88.0', '-Vv'])[1].startswith('rustc 1.88.0 ')
    run('cargo-stable-version', ['cargo', '+stable', '-V'])
    # Metadata first: no builds, no update, and do not classify every lock package
    # as a runtime dependency. Preserve reachable edges with target/kind labels.
    _, metadata, _ = run('metadata', ['cargo', '+stable', 'metadata', '--locked', '--format-version=1', '--filter-platform', 'x86_64-unknown-linux-gnu', '--no-default-features', '--features', 'headless'], seconds=300)
    data = json.loads(metadata)
    nodes = {n['id']: n for n in data['resolve']['nodes']}
    packages = {p['id']: p for p in data['packages']}
    root = data['resolve']['root']
    reached, edges, todo = set(), [], [root]
    while todo:
        ident = todo.pop()
        if ident in reached:
            continue
        reached.add(ident)
        for dep in nodes[ident]['deps']:
            edges.append({'from': ident, 'to': dep['pkg'], 'name': dep['name'], 'dep_kinds': dep['dep_kinds']})
            todo.append(dep['pkg'])
    records = [{'id': ident, 'name': packages[ident]['name'], 'version': packages[ident]['version'], 'rust_version': packages[ident].get('rust_version'), 'features': nodes[ident]['features']} for ident in sorted(reached)]
    higher = [r for r in records if r['rust_version'] and tuple(map(int, r['rust_version'].split('.')[:2])) > (1, 88)]
    report = {'root': root, 'target': 'x86_64-unknown-linux-gnu', 'features': 'headless/no-default-features', 'reachable_packages': records, 'edges': edges, 'declared_above_188': higher, 'undeclared': [r['id'] for r in records if not r['rust_version']], 'scope_limit': 'Metadata reachability includes dev/build and target-labeled edges; inspect those labels before calling a package mandatory for a particular build. Undeclared MSRV is unknown, not compatibility. Desktop and other targets are not audited. No whole-project 1.88 compile is performed.'}
    (OUT / 'dependency-audit.json').write_text(json.dumps(report, indent=2))
    RESULT['metadata_declared_above_188'] = higher
    # This failure proves only the API/MSRV mismatch, not a whole-project build failure.
    code, _, _ = run('original-api-188', ['rustc', '+1.88.0', '--edition=2021', '--crate-type=lib', '--error-format=json', str(HERE / 'original.rs'), '-o', str(OUT / 'original.rlib')], expected=None)
    diagnostics = [json.loads(line) for line in (OUT / 'original-api-188.stderr').read_text().splitlines()]
    errors = [d for d in diagnostics if d.get('level') == 'error' and d.get('code')]
    assert code == 1 and len(errors) == 1
    assert errors[0]['code']['code'] == 'E0658' and 'round_char_boundary' in errors[0]['message']
    assert any(s['is_primary'] and 'ceil_char_boundary' in ''.join(t['text'] for t in s['text']) for s in errors[0]['spans'])
    run('helper-compile-188', ['rustc', '+1.88.0', '--edition=2021', '--test', str(HERE / 'helper.rs'), '-o', str(OUT / 'helper-tests')])
    _, listing, _ = run('helper-list', [str(OUT / 'helper-tests'), '--list'])
    assert {s.removesuffix(': test') for s in listing.splitlines() if s.endswith(': test')} == {'ascii_boundaries', 'multibyte_boundaries_and_trigger_offset', 'out_of_range_and_empty'}
    _, results, stderr = run('helper-tests', [str(OUT / 'helper-tests'), '--test-threads=1'])
    assert 'test result: ok. 3 passed; 0 failed;' in results and not stderr.strip()
    assert not run('source-still-clean', ['git', 'status', '--porcelain'])[1]
    for name, digest in HASHES.items():
        assert hashlib.sha256((SOURCE / name).read_bytes()).hexdigest() == digest
    RESULT['status'] = 'api_failure_and_helper_cases_verified_not_whole_project'


try:
    main()
except BaseException as error:
    RESULT['status'] = 'failed'
    RESULT['error'] = repr(error)
    raise
finally:
    RESULT['script_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    RESULT['original_sha256'] = hashlib.sha256((HERE / 'original.rs').read_bytes()).hexdigest()
    RESULT['helper_sha256'] = hashlib.sha256((HERE / 'helper.rs').read_bytes()).hexdigest()
    (OUT / 'result.json').write_text(json.dumps(RESULT, indent=2))
