#!/usr/bin/env python3
"""Package a CI-verified headless binary with a strict Ubuntu 22.04 ABI gate."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tomllib

ALLOWED_NEEDED = {'libc.so.6', 'libm.so.6', 'libgcc_s.so.1', 'ld-linux-x86-64.so.2'}


def validate_elf(dynamic, versions, header):
    """Reject GUI dependencies, unexpected architecture, or newer glibc ABI."""
    if 'ELF64' not in header or 'Advanced Micro Devices X86-64' not in header:
        raise ValueError('Expected x86_64 ELF64')
    needed = set(re.findall(r'\(NEEDED\).*\[([^\]]+)\]', dynamic))
    if not needed or not needed <= ALLOWED_NEEDED:
        raise ValueError(f'Unexpected runtime dependencies: {sorted(needed)}')
    symbols = set(re.findall(r'\bGLIBC_([A-Za-z0-9_.]+)', versions))
    if any(not re.fullmatch(r'\d+(?:\.\d+)+', symbol) for symbol in symbols):
        raise ValueError('Unknown nonnumeric glibc ABI requirement')
    glibc = {tuple(map(int, x.split('.'))) for x in symbols}
    if not glibc or max(glibc) > (2, 35):
        raise ValueError('Missing glibc evidence or requires glibc newer than 2.35')
    return sorted(needed), '.'.join(map(str, max(glibc)))


def output(*args):
    return subprocess.check_output(args, text=True, timeout=30).strip()


def package(root, destination):
    root = root.resolve()
    sha = os.environ['SOURCE_SHA']
    if not re.fullmatch('[0-9a-f]{40}', sha) or output('git', '-C', str(root), 'rev-parse', 'HEAD') != sha:
        raise ValueError('Product source SHA mismatch')
    binary = root / 'target/release/xenterm'
    dynamic = output('readelf', '-d', str(binary))
    versions = output('readelf', '--version-info', str(binary))
    needed, glibc = validate_elf(dynamic, versions, output('readelf', '-h', str(binary)))
    dependencies = output('ldd', str(binary))
    if 'not found' in dependencies:
        raise ValueError('Unresolved runtime dependency')
    version = output(str(binary), '--version')
    expected = tomllib.loads((root / 'Cargo.toml').read_text())['package']['version']
    if version != 'xenterm ' + expected:
        raise ValueError('Binary version mismatch')
    destination.mkdir(parents=True, exist_ok=True)
    stem = f'xenterm-next-{sha[:12]}-headless-linux-x86_64'
    stage = destination / stem
    stage.mkdir()
    shutil.copy2(binary, stage / 'xenterm')
    for name in ('LICENSE', 'THIRD_PARTY_NOTICES.md'):
        shutil.copy2(root / name, stage / name)
    shutil.copytree(root / 'licenses', stage / 'licenses')
    manifest = dict(repository='xue-ding-e/XenTerm-Next', source_sha=sha,
                    tooling_sha=os.environ['GITHUB_SHA'], source_ci_run=os.environ['CI_RUN_ID'],
                    build_run=os.environ['GITHUB_RUN_ID'], platform='linux-x86_64-headless',
                    runner='ubuntu-22.04', features='--no-default-features --features headless',
                    binary_version=version, binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                    needed=needed, maximum_glibc=glibc, rustc=output('rustc', '--version'))
    (stage / 'BUILD.json').write_text(json.dumps(manifest, indent=2) + '\n')
    (stage / 'RUNTIME.txt').write_text(dependencies + '\n\n' + versions + '\n')
    (stage / 'README.txt').write_text('XenTerm Next headless server build; executable/configuration names remain xenterm.\n'
                                    'Based on ixbaicn/XenTerm. Original MIT attribution retained.\n'
                                    'Deployment must preserve a consistent profile backup and verify configuration compatibility.\n')
    archive = destination / (stem + '.tar.gz')
    with tarfile.open(archive, 'w:gz') as bundle:
        bundle.add(stage, arcname=stem)
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    (destination / 'SHA256SUMS').write_text(f'{checksum}  {archive.name}\n')
    (destination / 'BUILD.json').write_text(json.dumps(manifest, indent=2) + '\n')
    shutil.rmtree(stage)
    print(json.dumps(manifest, indent=2))


if __name__ == '__main__':
    package(Path(sys.argv[1]), Path(sys.argv[2]))
