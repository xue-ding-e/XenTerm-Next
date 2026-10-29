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

SHA = None  # Set by the reviewed product driver before version receipt checks.
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


# Delegate lifecycle ownership to the independently reviewed WNOWAIT/pidfd owner.
# No Popen.poll/wait/killpg path remains in this adapter.
import importlib.util
_OWNER_PATH = Path(__file__).with_name('owned-supervisor.py')
assert digest(_OWNER_PATH) == 'bc48ae5497f44cbdd97d73bce1a67c36e970c3dd13995336ce992de6c750b62c'
_owner_spec = importlib.util.spec_from_file_location('webdav_owner', _OWNER_PATH)
owner = importlib.util.module_from_spec(_owner_spec)
_owner_spec.loader.exec_module(owner)

def _budgets():
    if shutil.disk_usage(OUT).free < 2 * 1024**3:
        raise RuntimeError('disk budget (<2 GiB free)')
    # Retain the old sampled process-group RSS ceiling, not a hard memory limit.
    if owner.ACTIVE is not None and group_rss_kib(owner.ACTIVE) > 6 * 1024**2:
        raise RuntimeError('sampled process-group RSS budget >6 GiB')

def run(argv, stem, seconds, cap=8 * 1024 * 1024):
    owner.EVIDENCE = OUT
    owner.PHASE = 'webdav'
    owner.LOG_LIMIT = cap
    owner.TOTAL_LOG_LIMIT = 192 * 1024 * 1024
    owner.check_disk_budgets = _budgets
    # A single owner-wide deadline reserves cleanup time across all stages.
    if owner.JOB_DEADLINE is None:
        owner.JOB_DEADLINE = time.monotonic() + 143 * 60
    return owner.run(stem.replace('.', '-'), argv, seconds, SOURCE, ENV).decode('utf-8')


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
    raw = path.read_bytes()
    content = json.loads(raw)
    assert isinstance(content, dict) and set(content) == {'version','channel','semver','hash','timestamp','source','branch','tag','generated_at'}, 'version schema'
    assert content['hash'] == SHA[:7], 'version hash not product base'
    assert isinstance(content['version'], str) and re.fullmatch(r'xt-dev-' + SHA[:7] + r'-T[0-9]{10}', content['version']), 'version identity'
    assert content['channel'] == 'dev' and content['source'] == 'branch', 'version derivation'
    # Reading can update atime; compare only identity and content-change metadata.
    stable = lambda st: (st.st_dev, st.st_ino, st.st_mode, st.st_size, st.st_mtime_ns, st.st_ctime_ns)
    assert stable(path.lstat()) == stable(info), 'version changed during validation'
    return {'sha256': hashlib.sha256(raw).hexdigest(), 'bytes': len(raw), 'content': content}

