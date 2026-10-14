#!/usr/bin/env python3
"""Validate downloaded headless packages without extracting or executing them."""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import sys
import tarfile


def verify(directory, source, tooling, ci_run, build_run):
    """Bind archive bytes and embedded metadata to this trusted workflow run."""
    if not re.fullmatch('[0-9a-f]{40}', source):
        raise ValueError('Invalid source SHA')
    stem = f'xenterm-next-{source[:12]}-headless-linux-x86_64'
    archive = stem + '.tar.gz'
    if sorted(p.name for p in directory.iterdir()) != sorted([archive, 'BUILD.json', 'SHA256SUMS']):
        raise ValueError('Unexpected package files')
    checksum, name = (directory / 'SHA256SUMS').read_text().strip().split()
    if name != archive or hashlib.sha256((directory / archive).read_bytes()).hexdigest() != checksum:
        raise ValueError('Archive checksum mismatch')
    manifest = json.loads((directory / 'BUILD.json').read_text())
    expected = dict(repository='xue-ding-e/XenTerm-Next', source_sha=source, tooling_sha=tooling,
                    source_ci_run=str(ci_run), build_run=str(build_run), platform='linux-x86_64-headless',
                    runner='ubuntu-22.04', features='--no-default-features --features headless')
    if any(manifest.get(k) != v for k, v in expected.items()):
        raise ValueError('Manifest provenance mismatch')
    if not re.fullmatch(r'\d+(?:\.\d+)+', manifest.get('maximum_glibc', '')) or tuple(map(int, manifest['maximum_glibc'].split('.'))) > (2, 35):
        raise ValueError('Incompatible glibc requirement')
    allowed = {'libc.so.6', 'libm.so.6', 'libgcc_s.so.1', 'ld-linux-x86-64.so.2'}
    if not manifest.get('needed') or not set(manifest['needed']) <= allowed:
        raise ValueError('Unexpected runtime dependencies')
    with tarfile.open(directory / archive) as bundle:
        members = bundle.getmembers()
        if len({m.name for m in members}) != len(members):
            raise ValueError('Duplicate archive member')
        for member in members:
            path = PurePosixPath(member.name)
            if member.name != path.as_posix():
                raise ValueError('Noncanonical archive member')
            if path.is_absolute() or '..' in path.parts or path.parts[0] != stem or not (member.isfile() or member.isdir()):
                raise ValueError('Unsafe archive member')
        embedded = json.load(bundle.extractfile(stem + '/BUILD.json'))
        if embedded != manifest:
            raise ValueError('Embedded manifest mismatch')
        binary = bundle.getmember(stem + '/xenterm')
        if not binary.isfile() or not binary.mode & 0o111:
            raise ValueError('Missing executable permissions')
        if hashlib.sha256(bundle.extractfile(binary).read()).hexdigest() != manifest.get('binary_sha256'):
            raise ValueError('Binary checksum mismatch')
        for required in ('LICENSE', 'THIRD_PARTY_NOTICES.md', 'RUNTIME.txt'):
            if not bundle.getmember(stem + '/' + required).isfile():
                raise ValueError('Missing required package notice')
    return manifest


if __name__ == '__main__':
    verify(Path(sys.argv[1]), os.environ['SOURCE_SHA'], os.environ['GITHUB_SHA'],
           os.environ['CI_RUN_ID'], os.environ['GITHUB_RUN_ID'])
