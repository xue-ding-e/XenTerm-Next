"""Package verified Flutter desktop bundles without running or publishing them.

The caller supplies the version from scripts/version.sh and performs native
CLI/bridge smoke tests before packaging. Static version checks here never load
the payload. macOS staging, signing and ZIP creation require Apple's native tools;
only the Python API's command_runner hook substitutes them in synthetic tests.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import plistlib
import posixpath
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import zipfile


PLATFORMS = ('windows-x86_64', 'linux-x86_64', 'macos-aarch64')
REPOSITORY = 'xue-ding-e/XenTerm-Next'
CHUNK_SIZE = 1024 * 1024
VERSION_PATTERN = r'xt-(?:v[0-9]+\.[0-9]+\.[0-9]+|(?:beta|alpha|dev)-([0-9a-f]{7})-T[0-9]{10})'
BINARY_SUFFIXES = {'.exe', '.dll', '.so'}
REQUIRED = {
    'windows-x86_64': (
        'xenterm.exe', 'xenterm-cli.exe', 'xenterm_bridge.dll', 'flutter_windows.dll',
        'data/icudtl.dat', 'data/app.so', 'data/flutter_assets/NOTICES.Z',
        'WINDOWS-RUNTIME.json', 'msvcp140.dll', 'vcruntime140.dll',
    ),
    'linux-x86_64': (
        'xenterm', 'xenterm-cli', 'lib/libxenterm_bridge.so',
        'lib/libflutter_linux_gtk.so', 'lib/libapp.so', 'data/icudtl.dat',
        'data/flutter_assets/NOTICES.Z',
    ),
    'macos-aarch64': (
        'Contents/Info.plist', 'Contents/MacOS/xenterm_app', 'Contents/MacOS/xenterm-cli',
        'Contents/Frameworks/libxenterm_bridge.dylib',
        'Contents/Frameworks/FlutterMacOS.framework/FlutterMacOS',
        'Contents/Frameworks/FlutterMacOS.framework/Resources/icudtl.dat',
        'Contents/Frameworks/App.framework/App',
        'Contents/Frameworks/App.framework/Resources/flutter_assets/NOTICES.Z',
    ),
}


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(CHUNK_SIZE), b''):
            digest.update(chunk)
    return digest.hexdigest()


def excluded(relative):
    parts = [part.casefold() for part in relative.parts]
    return (any(part in ('config', 'config-flutter-next', 'log') for part in parts)
            or relative.suffix.casefold() == '.pdb'
            or relative.name.casefold() == 'xenterm_app.exe')


def is_link(path):
    info = path.lstat()
    return (stat.S_ISLNK(info.st_mode)
            or bool(getattr(info, 'st_file_attributes', 0)
                    & getattr(stat, 'FILE_ATTRIBUTE_REPARSE_POINT', 0)))


def plain_path(path):
    """Check ancestors before resolve() can hide a symlink or Windows junction."""
    path = Path(os.path.abspath(path))
    for ancestor in (path, *path.parents):
        if os.path.lexists(ancestor) and is_link(ancestor):
            raise ValueError(f'Path links are not permitted: {ancestor}')
    return path


def payload_paths(bundle, platform):
    """Walk without following directories linked to profiles or outside the app."""
    def walk(directory):
        for path in sorted(directory.iterdir()):
            relative = path.relative_to(bundle)
            if excluded(relative):
                continue
            if is_link(path):
                # Frameworks use Versions/Current, Resources and executable
                # symlinks. Preserve only relative links within that framework.
                framework = next((bundle.joinpath(*relative.parts[:i + 1])
                                  for i, part in enumerate(relative.parts)
                                  if part.endswith('.framework')), None)
                target = os.readlink(path) if path.is_symlink() else ''
                try:
                    resolved = path.resolve(strict=True)
                except (OSError, RuntimeError) as error:
                    raise ValueError(f'Invalid framework symlink: {relative}') from error
                if (platform != 'macos-aarch64' or framework is None
                        or relative.parts[:2] != ('Contents', 'Frameworks')
                        or not target or os.path.isabs(target) or ('\\' in target and os.name != 'nt')
                        or not resolved.is_relative_to(framework)
                        or excluded(resolved.relative_to(bundle))):
                    raise ValueError(f'Unsafe bundle link: {relative}')
                yield path
            elif path.is_dir():
                yield path
                yield from walk(path)
            elif path.is_file():
                yield path
            else:
                raise ValueError(f'Unsupported bundle entry: {relative}')
    return list(walk(bundle))


def require_file(path):
    if not path.is_file() or path.stat().st_size == 0:
        raise ValueError(f'Missing or empty runtime file: {path}')


def contains_version(path, version):
    needles = (version.encode('ascii'), version.encode('utf-16-le'))
    overlap = max(map(len, needles)) - 1
    tail = b''
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(CHUNK_SIZE), b''):
            data = tail + chunk
            if any(needle in data for needle in needles):
                return True
            tail = data[-overlap:]
    return False


def validate_identity(sha, version, run_id, upstream_sha):
    for label, value in (('source commit', sha), ('upstream snapshot', upstream_sha)):
        if not isinstance(value, str) or not re.fullmatch(r'[0-9a-f]{40}', value):
            raise ValueError(f'A full lowercase {label} SHA is required')
    match = re.fullmatch(VERSION_PATTERN, version)
    if not match:
        raise ValueError('Runtime version must be an XT_VERSION from scripts/version.sh')
    if match.group(1) and match.group(1) != sha[:7]:
        raise ValueError('Runtime source/version mismatch')
    if not re.fullmatch(r'[0-9]+', str(run_id)):
        raise ValueError('Build run ID must be numeric')


def validate_windows_runtime(bundle, paths, sha, version):
    info = json.loads((bundle / 'WINDOWS-RUNTIME.json').read_text(encoding='utf-8-sig'))
    if not isinstance(info, dict):
        raise ValueError('Windows runtime receipt must be an object')
    if info.get('source_sha') != sha or info.get('core_version') != version:
        raise ValueError('Runtime source/version mismatch')
    if (info.get('runtime_mode') != 'app-local' or info.get('clean_windows_tested') is not False
            or not isinstance(info.get('source'), str) or not info['source']):
        raise ValueError('App-local CRT source and testing status must be explicit')
    if not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+', str(info.get('minimum_vc_runtime', ''))):
        raise ValueError('Missing MSVC runtime version')
    crt = info.get('crt_files')
    if (not isinstance(crt, list) or not all(isinstance(name, str) for name in crt)
            or not {'msvcp140.dll', 'vcruntime140.dll'}.issubset(crt)
            or len(set(crt)) != len(crt)):
        raise ValueError('Missing app-local CRT inventory')
    for name in crt:
        if PurePosixPath(name).name != name or '\\' in name or not name.endswith('.dll'):
            raise ValueError(f'Invalid CRT filename: {name}')
        require_file(bundle / name)
    expected = {path.relative_to(bundle).as_posix(): sha256(path) for path in paths
                if path.is_file() and path.suffix.casefold() in BINARY_SUFFIXES}
    if info.get('binary_sha256') != expected:
        raise ValueError('Binary provenance mismatch (missing, changed or unlisted binary)')
    return info


def validate_bundle(bundle, platform, sha, version):
    if not bundle.is_dir():
        raise ValueError(f'Missing runtime bundle: {bundle}')
    if platform == 'macos-aarch64' and bundle.name != 'xenterm_app.app':
        raise ValueError('macOS --bundle must be xenterm_app.app')
    paths = payload_paths(bundle, platform)
    for name in REQUIRED[platform]:
        require_file(bundle / name)
    if platform == 'windows-x86_64':
        binaries = ('xenterm.exe', 'xenterm-cli.exe', 'xenterm_bridge.dll')
    elif platform == 'linux-x86_64':
        binaries = ('xenterm-cli', 'lib/libxenterm_bridge.so')
    else:
        binaries = ('Contents/MacOS/xenterm-cli', 'Contents/Frameworks/libxenterm_bridge.dylib')
        with (bundle / 'Contents/Info.plist').open('rb') as stream:
            if plistlib.load(stream).get('CFBundleExecutable') != 'xenterm_app':
                raise ValueError('Unexpected macOS GUI executable in Info.plist')
    for name in binaries:
        if not contains_version(bundle / name, version):
            raise ValueError(f'Runtime binary version mismatch: {name}')
    if platform != 'windows-x86_64' and os.name != 'nt':
        executables = (('xenterm', 'xenterm-cli') if platform == 'linux-x86_64'
                       else ('Contents/MacOS/xenterm_app', 'Contents/MacOS/xenterm-cli'))
        for name in executables:
            if not (bundle / name).stat().st_mode & 0o111:
                raise ValueError(f'Runtime is not executable: {name}')
    runtime = validate_windows_runtime(bundle, paths, sha, version) if platform == 'windows-x86_64' else None
    return paths, runtime


def license_sources(root):
    fonts = root / 'assets/fonts'
    plain_path(fonts)
    sources = [root / 'LICENSE', root / 'THIRD_PARTY_NOTICES.md', *sorted(fonts.glob('LICENSE*'))]
    if len(sources) < 3:
        raise ValueError('Missing font licenses under assets/fonts/LICENSE*')
    for source in sources:
        plain_path(source)
        require_file(source)
    return sources


def write_json(path, data):
    with path.open('x', encoding='utf-8', newline='\n') as stream:
        json.dump(data, stream, indent=2, ensure_ascii=False)
        stream.write('\n')


def copy_exclusive(source, destination):
    with source.open('rb') as reader, destination.open('xb') as writer:
        shutil.copyfileobj(reader, writer, CHUNK_SIZE)


def prune_excluded(directory, bundle):
    # Only called for the private ditto copy, never the caller's build tree.
    for path in directory.iterdir():
        if excluded(path.relative_to(bundle)):
            if not is_link(path) and path.is_dir():
                shutil.rmtree(path)
            else:
                path.unlink()
        elif not is_link(path) and path.is_dir():
            prune_excluded(path, bundle)


def native_command(argv):
    subprocess.run(argv, check=True)


def make_zip(stage, archive):
    with zipfile.ZipFile(archive, 'x', zipfile.ZIP_DEFLATED) as output:
        for path in sorted(stage.rglob('*')):
            if path.is_file():
                output.write(path, path.relative_to(stage.parent).as_posix())


def make_tar(stage, archive):
    def modes(member):
        # A Windows synthetic test host cannot express POSIX executable bits.
        # The two companion entry points must be executable on extraction too.
        if member.name in (f'{stage.name}/xenterm', f'{stage.name}/xenterm-cli'):
            member.mode = 0o755
        member.mode &= 0o777  # Do not ship setuid/setgid/sticky flags.
        return member
    with archive.open('xb') as stream, tarfile.open(fileobj=stream, mode='w:gz', dereference=False) as output:
        output.add(stage, arcname=stage.name, filter=modes)


def verify_archive(archive, prefix, platform):
    required = list(REQUIRED[platform])
    base = f'{prefix}/'
    extras = ('BUILD.json', 'licenses/LICENSE', 'licenses/THIRD_PARTY_NOTICES.md')
    if platform == 'macos-aarch64':
        base += 'xenterm_app.app/'
        required.extend(f'Contents/Resources/{name}' for name in extras)
    else:
        required.extend(extras)
    if platform == 'linux-x86_64':
        with tarfile.open(archive, 'r:gz') as packaged:
            members = {member.name: member for member in packaged.getmembers()}
            for name in required:
                member = members.get(base + name)
                if member is None or not member.isfile() or member.size == 0:
                    raise ValueError(f'Missing archived runtime file: {name}')
    else:
        with zipfile.ZipFile(archive) as packaged:
            if packaged.testzip() is not None:
                raise ValueError('ZIP integrity check failed')
            for name in required:
                if zip_member(packaged, base + name).file_size == 0:
                    raise ValueError(f'Empty archived runtime file: {name}')
    return archive


def zip_member(packaged, name):
    """Resolve framework directory links without extracting or following them."""
    root = name.split('/')[0] + '/'
    for _ in range(40):
        parts = name.split('/')
        for i in range(1, len(parts) + 1):
            candidate = '/'.join(parts[:i])
            try:
                member = packaged.getinfo(candidate)
            except KeyError:
                continue
            if stat.S_ISLNK(member.external_attr >> 16):
                if member.file_size > 4096:
                    raise ValueError('Oversized archived symlink')
                target = packaged.read(member).decode('utf-8')
                name = posixpath.normpath(posixpath.join(posixpath.dirname(candidate), target, *parts[i:]))
                if target.startswith('/') or not name.startswith(root):
                    raise ValueError('Unsafe archived symlink')
                break
        else:
            try:
                return packaged.getinfo(name)
            except KeyError as error:
                raise ValueError(f'Missing archived runtime file: {name}') from error
    raise ValueError('Archived symlink cycle')


def package(root, bundle, output, platform, version, source_sha, run_id, upstream_sha,
            msi=None, msi_product_version=None, *, command_runner=None):
    """Return the external receipt path; all output names are exclusive.

    The receipt's files inventory covers the final archive and, on Windows, MSI.
    The .sha256 manifest covers those same artifacts, sorted by filename. The
    publisher adds the receipts to the combined release checksum manifest.
    """
    if platform not in PLATFORMS:
        raise ValueError(f'Unsupported platform: {platform}')
    validate_identity(source_sha, version, run_id, upstream_sha)
    root, bundle, output = (plain_path(path) for path in (root, bundle, output))
    if output.is_relative_to(bundle):
        raise ValueError('Output must be outside the input bundle')
    if platform == 'macos-aarch64' and sys.platform != 'darwin' and command_runner is None:
        raise ValueError('macOS packaging requires native ditto and codesign on macOS')
    if platform == 'windows-x86_64':
        if msi is None or msi_product_version is None:
            raise ValueError('Windows requires --msi and --msi-product-version')
        if not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+', str(msi_product_version)):
            raise ValueError('MSI product version must be numeric major.minor.build')
        if any(int(part) > limit for part, limit in zip(msi_product_version.split('.'), (255, 255, 65535))):
            raise ValueError('MSI product version exceeds Windows Installer limits')
        msi = plain_path(msi)
        require_file(msi)
    elif msi is not None or msi_product_version is not None:
        raise ValueError('MSI arguments are Windows-only')
    prefix = f'XenTerm-Next-Flutter-{source_sha}-{platform}'
    archive_name = prefix + ('.tar.gz' if platform == 'linux-x86_64' else '.zip')
    receipt_name, manifest_name = f'{prefix}-BUILD.json', f'{prefix}.sha256'
    names = [archive_name, receipt_name, manifest_name]
    if msi is not None:
        names.append(f'{prefix}.msi')
    for name in names:
        if os.path.lexists(output / name):
            raise FileExistsError(f'Artifact already exists: {output / name}')
    paths, runtime = validate_bundle(bundle, platform, source_sha, version)
    licenses = license_sources(root)
    launcher = root / 'scripts/next/Start-Flutter-Preview.ps1'
    if platform == 'windows-x86_64':
        plain_path(launcher)
        require_file(launcher)
    metadata = {
        'schema_version': 1, 'product': 'XenTerm Next Flutter', 'frontend': 'flutter',
        'channel': 'flutter-beta', 'repository': REPOSITORY, 'source_sha': source_sha,
        'upstream_snapshot': upstream_sha, 'runtime_version': version,
        'build_run': str(run_id), 'platform': platform,
        'signing': ({'status': 'ad-hoc', 'notarized': False} if platform == 'macos-aarch64'
                    else {'status': 'unsigned'}),
        'default_profile': 'config-flutter-next',
    }
    if runtime is not None:
        metadata.update(windows_runtime=runtime, msi_product_version=msi_product_version)
    output.mkdir(parents=True, exist_ok=True)
    runner = command_runner or native_command
    with tempfile.TemporaryDirectory(prefix='.flutter-package-', dir=output) as temporary:
        working = Path(temporary)
        stage = working / prefix
        stage.mkdir()
        if platform == 'macos-aarch64':
            staged_bundle = stage / bundle.name
            runner(['/usr/bin/ditto', '--rsrc', '--extattr', '--acl', str(bundle), str(staged_bundle)])
            prune_excluded(staged_bundle, staged_bundle)
            validate_bundle(staged_bundle, platform, source_sha, version)
            resources = staged_bundle / 'Contents/Resources'
            resources.mkdir(exist_ok=True)
        else:
            staged_bundle = stage
            for path in paths:
                destination = stage / path.relative_to(bundle)
                if path.is_dir():
                    destination.mkdir(exist_ok=True)
                else:
                    shutil.copy2(path, destination)
            validate_bundle(staged_bundle, platform, source_sha, version)
            resources = stage
        write_json(resources / 'BUILD.json', metadata)
        license_directory = resources / 'licenses'
        license_directory.mkdir(exist_ok=True)
        for source in licenses:
            copy_exclusive(source, license_directory / source.name)
        if platform == 'windows-x86_64':
            copy_exclusive(launcher, stage / launcher.name)
        archive = working / archive_name
        if platform == 'macos-aarch64':
            for binary in ('Contents/Frameworks/libxenterm_bridge.dylib', 'Contents/MacOS/xenterm-cli'):
                runner(['/usr/bin/codesign', '--force', '--sign', '-', str(staged_bundle / binary)])
            # Insert all resources first so the outer app seals the final tree.
            # Retain the release runner's sandbox/library-validation entitlements.
            runner(['/usr/bin/codesign', '--force', '--deep', '--sign', '-',
                    '--preserve-metadata=entitlements', str(staged_bundle)])
            runner(['/usr/bin/codesign', '--verify', '--deep', '--strict', '--verbose=2', str(staged_bundle)])
            runner(['/usr/bin/ditto', '-c', '-k', '--rsrc', '--extattr', '--keepParent', str(stage), str(archive)])
        elif platform == 'linux-x86_64':
            make_tar(stage, archive)
        else:
            make_zip(stage, archive)
        verify_archive(archive, prefix, platform)
        artifacts = [archive]
        if msi is not None:
            staged_msi = working / f'{prefix}.msi'
            copy_exclusive(msi, staged_msi)
            if sha256(staged_msi) != sha256(msi):
                raise ValueError('MSI changed while being copied')
            artifacts.append(staged_msi)
        receipt = working / receipt_name
        write_json(receipt, dict(metadata, files=[{
            'name': path.name, 'size': path.stat().st_size, 'sha256': sha256(path),
        } for path in artifacts]))
        manifest = working / manifest_name
        with manifest.open('x', encoding='utf-8', newline='\n') as stream:
            for path in sorted(artifacts, key=lambda path: path.name):
                stream.write(f'{sha256(path)}  {path.name}\n')
        # Build and verify everything before exposing final names. Even a race
        # with another packager cannot replace an existing canonical artifact.
        for path in [*artifacts, receipt, manifest]:
            copy_exclusive(path, output / path.name)
    return output / receipt_name


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bundle', required=True, type=Path)
    parser.add_argument('--platform', required=True, choices=PLATFORMS)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--version', required=True)
    parser.add_argument('--source-sha', required=True)
    parser.add_argument('--run-id', required=True)
    parser.add_argument('--upstream-sha', required=True)
    parser.add_argument('--msi', type=Path)
    parser.add_argument('--msi-product-version')
    args = parser.parse_args(argv)
    try:
        result = package(Path(__file__).resolve().parents[2], **vars(args))
    except (OSError, ValueError, subprocess.CalledProcessError, zipfile.BadZipFile, tarfile.TarError) as error:
        parser.exit(1, f'Packaging failed: {error}\n')
    print(result)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
