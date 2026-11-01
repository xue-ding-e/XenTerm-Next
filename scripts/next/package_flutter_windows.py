"""Package a complete Flutter preview; never publish or alter a release channel."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import zipfile

UPSTREAM_SHA = 'a520c70daaa965c86080409b33e96ebf5c8c2906'
REQUIRED = ('xenterm.exe', 'xenterm-cli.exe', 'xenterm_bridge.dll', 'flutter_windows.dll',
            'data/icudtl.dat', 'data/app.so', 'data/flutter_assets/NOTICES.Z', 'WINDOWS-RUNTIME.json')


def validate_bundle(bundle):
    for name in REQUIRED:
        path = bundle / name
        if not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f'Missing or empty runtime file: {name}')
    if not (bundle / 'data/flutter_assets').is_dir():
        raise ValueError('Missing Flutter assets')


def validate_provenance(bundle, sha, version):
    info = json.loads((bundle / 'WINDOWS-RUNTIME.json').read_text(encoding='utf-8'))
    if not version or info.get('core_version') != version or info.get('source_sha') != sha:
        raise ValueError('Runtime source/version mismatch')
    if not re.fullmatch(r'\d+\.\d+\.\d+\.\d+', info.get('minimum_vc_runtime', '')):
        raise ValueError('Missing VC++ runtime prerequisite')
    if info.get('runtime_mode') != 'external-prerequisite' or info.get('clean_windows_tested') is not False:
        raise ValueError('Runtime prerequisite/testing status must be explicit')
    for name in ('xenterm.exe', 'xenterm-cli.exe', 'xenterm_bridge.dll'):
        digest = hashlib.sha256((bundle / name).read_bytes()).hexdigest()
        if info.get('binary_sha256', {}).get(name) != digest:
            raise ValueError(f'Binary provenance mismatch: {name}')
    return info


def package(root, bundle, output, sha, version):
    if not re.fullmatch(r'[0-9a-f]{40}', sha):
        raise ValueError('A full source commit SHA is required')
    validate_bundle(bundle)
    runtime = validate_provenance(bundle, sha, version)
    name = f'XenTerm-Next-Flutter-{sha}-windows-x86_64'
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f'{name}.zip'
    # Exclusive creation prevents accidentally replacing an already verified artifact.
    with zipfile.ZipFile(archive, 'x', zipfile.ZIP_DEFLATED) as z:
        for path in sorted(bundle.rglob('*')):
            rel = path.relative_to(bundle)
            if path.is_symlink():
                raise ValueError(f'Symlink not permitted in portable bundle: {rel}')
            if rel.parts[0].lower() in ('config', 'config-flutter-next', 'log'):
                continue
            if not path.is_file() or path.suffix.lower() == '.pdb':
                continue
            if path.name.lower() == 'xenterm_app.exe':
                continue
            z.write(path, f'{name}/{rel.as_posix()}')
        for source in [root / 'LICENSE', root / 'THIRD_PARTY_NOTICES.md',
                       *sorted((root / 'assets/fonts').glob('LICENSE*'))]:
            if not source.is_file():
                raise ValueError(f'Missing license: {source}')
            z.write(source, f'{name}/licenses/{source.name}')
        z.write(root / 'scripts/next/Start-Flutter-Preview.ps1', f'{name}/Start-Flutter-Preview.ps1')
        z.writestr(f'{name}/WINDOWS-PREREQUISITE.txt',
            'EXPERIMENTAL: Windows 10/11 x64. Not a fully self-contained package.\n'
            f'Microsoft Visual C++ x64 runtime {runtime["minimum_vc_runtime"]} or newer is required.\n'
            'Use Start-Flutter-Preview.ps1 to check the prerequisite before launching.\n'
            'Install prerequisites manually from Microsoft, never third-party DLL sites:\n'
            'https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist\n'
            'Clean Windows execution has NOT been tested. No automatic installation.\n')
        z.writestr(f'{name}/BUILD.json', json.dumps({
            'product': 'XenTerm Next Flutter Preview', 'frontend': 'flutter',
            'source_sha': sha, 'upstream_snapshot': UPSTREAM_SHA,
            'channel': 'flutter-preview', 'default_profile': 'config-flutter-next',
            'signed': False, 'core_version': version, 'windows_runtime': runtime,
        }, indent=2) + '\n')
    # Check the archive, not merely its input directory.
    with zipfile.ZipFile(archive) as z:
        if z.testzip() is not None:
            raise ValueError('ZIP integrity check failed')
        for relative in REQUIRED:
            if z.getinfo(f'{name}/{relative}').file_size == 0:
                raise ValueError(f'Empty packaged runtime file: {relative}')
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (output / f'{name}.sha256').write_text(f'{digest}  {archive.name}\n', encoding='utf-8')
    return archive


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--bundle', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--version', required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    sha = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()
    print(package(root, args.bundle, args.output, sha, args.version))
