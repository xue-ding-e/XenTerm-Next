#!/usr/bin/env python3
"""Bounded real Cargo/libtest validation; no account or GUI traffic."""
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import signal
import stat
import subprocess
import sys
import time

SHA = 'd46a32e2e2c80d31fea3c03c7355a3f53b01b800'
TREE = '6d363a82c410a5b2c937e25827fafd50cbbd9e7e'
PATCH_HASH = 'ee8dedf02e638011fb96c03aea89113f785c21bd43503f9fd13857980546056b'
EXPECTED = {
    'ssh::ssh::unmapped_forwarding_tests::mapped_connect_failure_releases_connection_count',
    'ssh::ssh::unmapped_forwarding_tests::mapped_channel_at_cap_does_not_dial_target',
}
SUMMARY = re.compile(r'^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; \d+ filtered out; finished in [0-9.]+s$')

def digest(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b''):
            h.update(chunk)
    return h.hexdigest()


def verify_hash(actual, expected):
    assert actual == expected, 'test binary changed'


def parse_inventory(output, name):
    lines = output.splitlines()
    assert lines.count(name + ': test') == 1, 'missing/duplicate exact test'
    assert [x for x in lines if x.endswith(': test') or x.endswith(': benchmark')] == [name + ': test']


def parse_result(output, name):
    lines = output.splitlines()
    assert lines.count('running 1 test') == 1, 'not exactly one running test'
    assert [x for x in lines if x.startswith('test ') and ' ... ' in x] == ['test ' + name + ' ... ok'], 'wrong/ignored/duplicate case'
    summaries = [x for x in lines if x.startswith('test result:')]
    assert len(summaries) == 1 and SUMMARY.fullmatch(summaries[0]), 'not unique successful one-test summary'


def group_rss_kib(group):
    # Sample the entire dedicated process group, not just Cargo/rustc parent RSS.
    total = 0
    for entry in Path('/proc').iterdir():
        if not entry.name.isdigit():
            continue
        try:
            fields = (entry / 'stat').read_text().rsplit(')', 1)[1].split()
            if int(fields[2]) != group:
                continue
            for line in (entry / 'status').read_text().splitlines():
                if line.startswith('VmRSS:'):
                    total += int(line.split()[1])
        except (FileNotFoundError, ProcessLookupError):
            continue
    return total


def run(argv, stem, seconds, cap=8 * 1024 * 1024):
    start = time.monotonic()
    size = 0
    peak_group_rss_kib = 0
    p = poller = None
    blocked = {signal.SIGTERM, signal.SIGINT}
    oldmask = signal.pthread_sigmask(signal.SIG_BLOCK, blocked)
    try:
        p = subprocess.Popen(argv, cwd=SOURCE, env=ENV, stdout=subprocess.PIPE,
                             stderr=subprocess.STDOUT, start_new_session=True)
        signal.pthread_sigmask(signal.SIG_SETMASK, oldmask)
        poller = selectors.DefaultSelector()
        poller.register(p.stdout, selectors.EVENT_READ)
        with open(OUT / (stem + '.log'), 'wb') as log:
            while poller.get_map() or p.poll() is None:
                if time.monotonic() - start > seconds:
                    raise RuntimeError(stem + ': timeout')
                if shutil.disk_usage(OUT).free < 2 * 1024**3:
                    raise RuntimeError(stem + ': disk budget (<2 GiB free)')
                peak_group_rss_kib = max(peak_group_rss_kib, group_rss_kib(p.pid))
                if peak_group_rss_kib > 6 * 1024**2:
                    raise RuntimeError(stem + ': sampled process-group RSS budget >6 GiB')
                if not poller.get_map():
                    time.sleep(0.2)
                for key, _ in poller.select(0.2) if poller.get_map() else []:
                    chunk = os.read(key.fd, 65536)
                    if not chunk:
                        poller.unregister(key.fileobj)
                        continue
                    size += len(chunk)
                    if size > cap:
                        raise RuntimeError(stem + ': output budget exceeded')
                    log.write(chunk)
            code = p.wait(timeout=max(1, seconds - (time.monotonic() - start)))
        evidence = {'argv': argv, 'exit': code, 'seconds': time.monotonic() - start,
                    'bytes': size, 'peak_group_rss_kib': peak_group_rss_kib, 'disk_free': shutil.disk_usage(OUT).free}
        (OUT / (stem + '.json')).write_text(json.dumps(evidence, indent=2))
        if code != 0:
            raise RuntimeError(stem + ': exit ' + str(code))
        return (OUT / (stem + '.log')).read_text()
    finally:
        # Prevent repeated cancellation from interrupting bounded group cleanup.
        previous_handlers = {sig: signal.signal(sig, signal.SIG_IGN) for sig in blocked}
        signal.pthread_sigmask(signal.SIG_SETMASK, oldmask)
        try:
            if p is not None:
                try:
                    os.killpg(p.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                try:
                    p.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    (OUT / (stem + '-cleanup.json')).write_text(json.dumps({
                        'cleanup': 'uncertain: child did not reap within 10s', 'pid': p.pid}))
                    raise RuntimeError(stem + ': cleanup uncertain')
                p.stdout.close()
            if poller is not None:
                poller.close()
        finally:
            for sig, handler in previous_handlers.items():
                signal.signal(sig, handler)


def interrupted(signum, _frame):
    raise RuntimeError('driver interrupted by signal ' + str(signum))


def version_preflight(out):
    path = out / 'version.json'
    assert not any(p.is_symlink() for p in [out, *out.parents]), 'symlink output parent'
    assert not os.path.lexists(path), 'pre-existing version output'
    return path


def version_receipt(path):
    assert not any(p.is_symlink() for p in path.parents), 'symlink output parent'
    info = path.lstat()
    assert stat.S_ISREG(info.st_mode) and info.st_size <= 16384, 'version type/size'
    content = json.loads(path.read_text())
    assert isinstance(content, dict) and set(content) == {'version','channel','semver','hash','timestamp','source','branch','tag','generated_at'}, 'version schema'
    assert content['hash'] == SHA[:7], 'version hash not product base'
    assert isinstance(content['version'], str) and re.fullmatch(r'xt-dev-' + SHA[:7] + r'-T[0-9]{10}', content['version']), 'version identity'
    assert content['channel'] == 'dev' and content['source'] == 'branch', 'version derivation'
    assert path.lstat() == info, 'version changed during validation'
    return {'sha256': digest(path), 'bytes': info.st_size, 'content': content}


def validate():
    control = Path(__file__).resolve().parent.parent
    control_sha = os.environ['GITHUB_SHA']
    assert re.fullmatch(r'[0-9a-f]{40}', control_sha)
    assert run(['git', '-C', str(control), 'rev-parse', 'HEAD'], 'control-head', 10).strip() == control_sha
    assert run(['git', 'rev-parse', 'HEAD'], 'source-head', 10).strip() == SHA
    assert not run(['git', 'status', '--porcelain', '--untracked-files=all', '--ignored'], 'clean-before', 10).strip()
    version_output = version_preflight(OUT)
    patch = control / 'scripts/forwarding.patch'
    assert digest(patch) == PATCH_HASH
    run(['git', 'apply', '--check', '--whitespace=error', str(patch)], 'patch-check', 10)
    run(['git', 'apply', '--index', '--whitespace=error', str(patch)], 'patch-apply', 10)
    assert run(['git', 'write-tree'], 'candidate-tree', 10).strip() == TREE
    for key, name in [('HOME','home'), ('CARGO_HOME','cargo-home'), ('CARGO_TARGET_DIR','target'),
                      ('XDG_CACHE_HOME','cache'), ('XDG_CONFIG_HOME','config'), ('XENTERM_DATA_DIR','profile')]:
        ENV[key] = str(OUT / name)
        (OUT / name).mkdir()
    for key in ['ALL_PROXY','all_proxy','HTTP_PROXY','http_proxy','HTTPS_PROXY','https_proxy']:
        ENV.pop(key, None)
    for key in ['XT_VERSION', 'XT_CHANNEL']:
        ENV.pop(key, None)
    ENV.update(XT_VERSION_JSON=str(version_output), GITHUB_SHA=SHA,
               GITHUB_REF_TYPE='branch', GITHUB_REF_NAME='validation/forwarding-slots-20261028')
    output = run([CARGO, 'test', '--locked', '--no-default-features', '--features', 'headless',
                  '--jobs', '1', '--no-run', '--message-format=json'], 'compile', 2400, 64*1024*1024)
    artifacts = []
    for line in output.splitlines():
        try:
            item = json.loads(line)
        except json.JSONDecodeError:
            continue
        if (item.get('reason') == 'compiler-artifact' and item.get('profile', {}).get('test')
                and item.get('executable') and
                Path(item['target']['src_path']).resolve() == SOURCE / 'src/main.rs'):
            artifacts.append(Path(item['executable']).resolve())
    assert len(artifacts) == 1, artifacts
    binary = artifacts[0]
    assert binary.is_relative_to((OUT / 'target').resolve()) and binary.is_file()
    binary_hash = digest(binary)
    for i, name in enumerate(sorted(EXPECTED)):
        verify_hash(digest(binary), binary_hash)
        inventory = run([str(binary), name, '--exact', '--list', '--format', 'terse'], 'inventory-'+str(i), 30)
        parse_inventory(inventory, name)
        result = run([str(binary), name, '--exact', '--test-threads=1', '--format', 'pretty', '--color', 'never'], 'test-'+str(i), 60)
        parse_result(result, name)
    verify_hash(digest(binary), binary_hash)
    assert not run(['git', 'diff', '--exit-code'], 'source-unchanged', 10).strip()
    assert run(['git', 'write-tree'], 'tree-after', 10).strip() == TREE
    assert run(['git', 'status', '--porcelain', '--untracked-files=all', '--ignored'], 'status-after', 10).strip() == 'M  src/ssh/impls/ssh.rs'
    (OUT / 'receipt.json').write_text(json.dumps({'source_sha': SHA, 'candidate_tree': TREE,
        'patch_sha256': PATCH_HASH, 'control_sha': control_sha, 'binary_sha256': binary_hash,
        'version_metadata': version_receipt(version_output), 'source_identity': 'base plus patch/tree, not a candidate commit SHA',
        'tests': sorted(EXPECTED), 'scope': 'two synthetic loopback SSH headless tests; not native GUI or full suite'}, indent=2))


if __name__ == '__main__':
    SOURCE = Path(sys.argv[1]).resolve()
    OUT = Path(sys.argv[2]).resolve()
    OUT.mkdir(parents=True, exist_ok=False)
    CARGO = os.path.abspath(shutil.which('cargo'))
    ENV = dict(os.environ, RUSTUP_HOME=os.environ.get('RUSTUP_HOME', str(Path.home()/'.rustup')),
               CARGO_BUILD_JOBS='1', CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0', CARGO_INCREMENTAL='0')
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    try:
        validate()
    except BaseException as exc:
        (OUT/'failure.json').write_text(json.dumps({'error': str(exc), 'classification': 'unclassified; NOT product RED',
            'source_sha': SHA, 'regressions_passed': False}))
        raise
