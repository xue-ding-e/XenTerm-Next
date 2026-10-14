#!/usr/bin/env python3
"""Stage portable Next packages without changing source compatibility names."""
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tarfile
import zipfile
from next_beta import PLATFORMS, REPOSITORY, prefix


def package(root, platform, target, sha):
    stem = prefix(sha) + platform
    stage = root / 'next-stage' / stem
    stage.mkdir(parents=True)
    binary = root / 'target' / target / 'release' / ('xenterm.exe' if platform.startswith('windows') else 'xenterm')
    version = subprocess.check_output([str(binary), '--version'], text=True, encoding='utf-8', timeout=30).strip()
    cargo_version = __import__('tomllib').loads((root / 'Cargo.toml').read_text(encoding='utf-8'))['package']['version']
    if version != 'xenterm ' + cargo_version:
        raise RuntimeError('Binary version differs from Cargo.toml')
    notices = stage
    if platform.startswith('macos'):
        app = stage / 'XenTerm Next.app'
        contents = app / 'Contents'
        executable = contents / 'MacOS'
        executable.mkdir(parents=True)
        notices = contents / 'Resources'
        notices.mkdir()
        shutil.copy2(binary, executable / 'xenterm')
        with (root / 'assets/Info.plist').open('rb') as source:
            info = plistlib.load(source)
        info.update(CFBundleIdentifier='io.github.xue-ding-e.xenterm-next',
                    CFBundleName='XenTerm Next', CFBundleDisplayName='XenTerm Next',
                    CFBundleShortVersionString=cargo_version, CFBundleVersion=cargo_version,
                    NSHumanReadableCopyright='Copyright (c) 2026 xenterm contributors. MIT.')
        # Retain upstream artwork with a distinct bundle identity.
        iconset = stage / 'next.iconset'
        iconset.mkdir()
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                pixels = size * scale
                subprocess.run(['sips', '-z', str(pixels), str(pixels), str(root / 'assets/icon@512.png'),
                                '--out', str(iconset / f'icon_{size}x{size}{"@2x" if scale == 2 else ""}.png')], check=True)
        subprocess.run(['iconutil', '-c', 'icns', str(iconset), '-o', str(notices / 'xenterm.icns')], check=True)
        shutil.rmtree(iconset)
        with (contents / 'Info.plist').open('wb') as dest:
            plistlib.dump(info, dest)
    else:
        shutil.copy2(binary, stage / binary.name)
    for name in ('LICENSE', 'THIRD_PARTY_NOTICES.md'):
        shutil.copy2(root / name, notices / name)
    shutil.copytree(root / 'licenses', notices / 'licenses')
    (notices / 'XenTerm-Next.txt').write_text(
        'XenTerm Next: independent enhanced distribution.\n'
        f'Repository: https://github.com/{REPOSITORY}\nCommit: {sha}\n'
        'Based on https://github.com/ixbaicn/XenTerm; original MIT attribution retained.\n'
        'Windows: unsigned. macOS: ad-hoc signed, not notarized. Do not disable OS security protections.\n'
        'Linux: built on Ubuntu 22.04; system GUI libraries are required.\n'
        'Executable and existing configuration paths remain xenterm for compatibility.\n', encoding='utf-8')
    if platform.startswith('linux'):
        dependencies = subprocess.check_output(['ldd', str(binary)], text=True, encoding='utf-8', timeout=30)
        if 'not found' in dependencies:
            raise RuntimeError('Linux executable has unresolved runtime libraries')
        (notices / 'RUNTIME.txt').write_text(
            'Built and version-checked on Ubuntu 22.04. Requires the following system libraries.\n'
            'This is a dependency check, not a GUI startup acceptance test.\n\n' + dependencies, encoding='utf-8')
    (notices / 'BUILD.json').write_text(json.dumps({
        'repository': REPOSITORY, 'sha': sha, 'platform': platform,
        'binary_version': version, 'rustc': subprocess.check_output(['rustc', '--version'], text=True, encoding='utf-8', timeout=30).strip(),
        'ci_run': os.environ['CI_RUN_ID'], 'build_run': os.environ['GITHUB_RUN_ID'],
    }, indent=2) + '\n', encoding='utf-8')
    dist = root / 'dist'
    dist.mkdir(exist_ok=True)
    output = dist / (stem + PLATFORMS[platform])
    if platform.startswith('macos'):
        subprocess.run(['codesign', '--force', '--deep', '--sign', '-', str(app)], check=True)
        subprocess.run(['codesign', '--verify', '--deep', '--strict', '--verbose=2', str(app)], check=True)
        subprocess.run(['ditto', '-c', '-k', '--norsrc', '--keepParent', str(app), str(output)], check=True)
    elif platform.startswith('windows'):
        with zipfile.ZipFile(output, 'w', zipfile.ZIP_DEFLATED) as archive:
            for path in stage.rglob('*'):
                if path.is_file():
                    archive.write(path, path.relative_to(stage.parent))
    else:
        with tarfile.open(output, 'w:gz') as archive:
            archive.add(stage, arcname=stem)


def package_from_environment():
    # MSVC dev-cmd sets the generic PLATFORM variable to x64. Keep our package
    # identity in a dedicated variable that the toolchain does not mutate.
    package(Path.cwd(), os.environ['XENTERM_PACKAGE_PLATFORM'], os.environ['TARGET'], os.environ['SOURCE_SHA'])


if __name__ == '__main__':
    package_from_environment()
