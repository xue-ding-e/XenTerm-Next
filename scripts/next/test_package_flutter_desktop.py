"""Synthetic packaging contracts: no fixture executable is ever launched."""

import base64
from contextlib import redirect_stderr, redirect_stdout
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import plistlib
import shutil
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('desktop_pack', HERE / 'package_flutter_desktop.py')
pack = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(pack)
SHA, UPSTREAM = 'a' * 40, 'b' * 40
VERSION = 'xt-beta-aaaaaaa-T1794440000'
CRT_FILES = ('msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll', 'concrt140.dll')
POWERSHELLS = list(dict.fromkeys(filter(None, (shutil.which('powershell'), shutil.which('pwsh')))))


class FakeAppleTools:
    """Emulate copies/signatures, while asserting the native command ordering."""

    def __init__(self):
        self.commands = []

    def copy_app(self, source, target):
        # shutil.copytree on Python 3.11/Windows creates dangling directory
        # links as file links. ditto preserves the directory-link type.
        target.mkdir()
        for path in source.iterdir():
            destination = target / path.name
            if path.is_symlink():
                destination.symlink_to(os.readlink(path), target_is_directory=path.is_dir())
            elif path.is_dir():
                self.copy_app(path, destination)
            else:
                shutil.copy2(path, destination)

    def __call__(self, command):
        self.commands.append(command)
        source, target = Path(command[-2]), Path(command[-1])
        if command[0] == '/usr/bin/ditto' and '-c' not in command:
            assert '--rsrc' in command and '--extattr' in command and '--acl' in command
            self.copy_app(source, target)
        elif command[0] == '/usr/bin/codesign':
            app = target if target.is_dir() else next(p for p in target.parents if p.suffix == '.app')
            metadata = json.loads((app / 'Contents/Resources/BUILD.json').read_text(encoding='utf-8'))
            assert 'files' not in metadata
            assert (app / 'Contents/Resources/licenses/LICENSE').is_file()
            if '--verify' in command:
                assert '--deep' in command and '--strict' in command
                assert (app / 'Contents/_CodeSignature/CodeResources').is_file()
            elif target.is_file():
                with target.open('ab') as stream:
                    stream.write(b'\0synthetic signature')
            else:
                assert '--preserve-metadata=entitlements' in command
                signature = app / 'Contents/_CodeSignature/CodeResources'
                signature.parent.mkdir(exist_ok=True)
                signature.write_bytes(b'synthetic seal, not an Apple signature')
        else:
            assert command[0] == '/usr/bin/ditto' and '--keepParent' in command
            assert '--rsrc' in command and '--extattr' in command
            with zipfile.ZipFile(target, 'x', zipfile.ZIP_DEFLATED) as archive:
                for path in sorted(source.rglob('*')):
                    name = path.relative_to(source.parent).as_posix()
                    if path.is_symlink():
                        info = zipfile.ZipInfo(name)
                        info.create_system = 3
                        info.external_attr = (stat.S_IFLNK | 0o777) << 16
                        target_text = os.readlink(path)
                        if os.name == 'nt':
                            target_text = target_text.replace('\\', '/')
                        archive.writestr(info, target_text.encode('utf-8'))
                    elif path.is_file():
                        archive.write(path, name)


class DesktopPackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='flutter-desktop-test-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        for name in ('LICENSE', 'THIRD_PARTY_NOTICES.md', 'assets/fonts/LICENSE-One.txt',
                     'assets/fonts/LICENSE-Two.txt'):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(f'fixture license {name}\n'.encode())
        launcher = self.root / 'scripts/next/Start-Flutter-Preview.ps1'
        launcher.parent.mkdir(parents=True)
        shutil.copyfile(HERE / launcher.name, launcher)
        self.msi = self.root / 'input.msi'
        self.msi.write_bytes(b'synthetic MSI bytes: never execute or install')
        self.count = 0

    def bundle(self, platform):
        self.count += 1
        directory = self.root / f'input-{self.count}'
        if platform == 'macos-aarch64':
            directory /= 'xenterm_app.app'
        for name in pack.REQUIRED[platform]:
            path = directory / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b'synthetic non-executable fixture: ' + name.encode())
        if platform == 'windows-x86_64':
            versions = ('xenterm-cli.exe', 'xenterm_bridge.dll')
            (directory / 'xenterm.exe').write_bytes(b'synthetic resource\0' + VERSION.encode('utf-16-le'))
            extras = (*CRT_FILES, 'plugins/plugin.dll', 'plugins/helper.exe')
        elif platform == 'linux-x86_64':
            versions = ('xenterm-cli', 'lib/libxenterm_bridge.so')
            extras = ('lib/plugin.so', 'data/flutter_assets/AssetManifest.bin')
        else:
            versions = ('Contents/MacOS/xenterm-cli', 'Contents/Frameworks/libxenterm_bridge.dylib')
            (directory / 'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable': 'xenterm_app'}))
            extras = ('Contents/Frameworks/plugin.dylib', 'Contents/Resources/icon.icns')
        for name in versions:
            (directory / name).write_bytes(b'synthetic version\0' + VERSION.encode() + b'\0payload')
        for name in extras:
            path = directory / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b'fixture ' + name.encode())
        # Include nested profiles: filtering only the first component leaks them.
        for name in ('config/sessions.db', 'config-flutter-next/secret.key', 'log/debug.log',
                     'nested/CONFIG/sessions.db', 'nested/config-flutter-next/secret.key',
                     'nested/log/private', 'nested/debug.PDB', 'nested/xenterm_app.exe'):
            path = directory / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b'private fixture must not be archived')
        if platform != 'windows-x86_64':
            executables = (('xenterm', 'xenterm-cli') if platform == 'linux-x86_64'
                           else ('Contents/MacOS/xenterm_app', 'Contents/MacOS/xenterm-cli'))
            for name in executables:
                (directory / name).chmod(0o755)
        else:
            self.runtime(directory)
        return directory

    def runtime(self, bundle):
        info = {
            'schema_version': 1, 'source': 'installed Visual Studio/x64/Microsoft.VC143.CRT',
            'source_sha': SHA, 'core_version': VERSION, 'minimum_vc_runtime': '14.44.35211.0',
            'runtime_mode': 'app-local', 'clean_windows_tested': False,
            'crt_files': list(CRT_FILES),
            'binary_sha256': {path.relative_to(bundle).as_posix(): pack.sha256(path)
                              for path in pack.payload_paths(bundle, 'windows-x86_64')
                              if path.is_file() and path.suffix.lower() in pack.BINARY_SUFFIXES},
        }
        (bundle / 'WINDOWS-RUNTIME.json').write_text(json.dumps(info), encoding='utf-8')
        return info

    def package(self, bundle, platform, output=None, **overrides):
        arguments = dict(root=self.root, bundle=bundle, output=output or self.root / 'dist',
                         platform=platform, version=VERSION, source_sha=SHA,
                         run_id='123456789', upstream_sha=UPSTREAM)
        if platform == 'windows-x86_64':
            arguments.update(msi=self.msi, msi_product_version='1.2.345')
        if platform == 'macos-aarch64':
            arguments['command_runner'] = FakeAppleTools()
        arguments.update(overrides)
        return pack.package(**arguments)

    def receipt(self, path, platform):
        info = json.loads(path.read_text(encoding='utf-8'))
        expected = dict(schema_version=1, product='XenTerm Next Flutter', frontend='flutter',
                        channel='flutter-beta', repository=pack.REPOSITORY, source_sha=SHA,
                        upstream_snapshot=UPSTREAM, runtime_version=VERSION,
                        build_run='123456789', platform=platform, default_profile='config-flutter-next')
        for key, value in expected.items():
            self.assertEqual(info[key], value, key)
        for item in info['files']:
            artifact = path.parent / item['name']
            self.assertEqual(item['size'], artifact.stat().st_size)
            self.assertEqual(item['sha256'], hashlib.sha256(artifact.read_bytes()).hexdigest())
        prefix = path.name.removesuffix('-BUILD.json')
        checksums = (path.parent / f'{prefix}.sha256').read_text(encoding='utf-8').splitlines()
        entries = {line.split('  ', 1)[1]: line.split('  ', 1)[0] for line in checksums}
        self.assertEqual(set(entries), {item['name'] for item in info['files']})
        self.assertEqual(list(entries), sorted(entries))
        for name, digest in entries.items():
            self.assertEqual(digest, hashlib.sha256((path.parent / name).read_bytes()).hexdigest())
        return info, prefix

    def assert_no_private_entries(self, names):
        for name in names:
            self.assertFalse(pack.excluded(Path(name)), name)

    def test_windows_zip_msi_receipt_checksums_and_all_licenses(self):
        bundle = self.bundle('windows-x86_64')
        receipt = self.package(bundle, 'windows-x86_64')
        info, prefix = self.receipt(receipt, 'windows-x86_64')
        self.assertEqual(info['signing'], {'status': 'unsigned'})
        self.assertEqual(info['msi_product_version'], '1.2.345')
        self.assertEqual(info['windows_runtime']['runtime_mode'], 'app-local')
        self.assertFalse(info['windows_runtime']['clean_windows_tested'])
        self.assertEqual([item['name'] for item in info['files']], [prefix + '.zip', prefix + '.msi'])
        self.assertEqual((receipt.parent / f'{prefix}.msi').read_bytes(), self.msi.read_bytes())
        with zipfile.ZipFile(receipt.parent / f'{prefix}.zip') as archive:
            self.assert_no_private_entries(archive.namelist())
            for name in (*pack.REQUIRED['windows-x86_64'], *CRT_FILES, 'plugins/plugin.dll',
                         'plugins/helper.exe', 'Start-Flutter-Preview.ps1', 'licenses/LICENSE',
                         'licenses/THIRD_PARTY_NOTICES.md', 'licenses/LICENSE-One.txt', 'licenses/LICENSE-Two.txt'):
                self.assertGreater(archive.getinfo(f'{prefix}/{name}').file_size, 0, name)
            internal = json.loads(archive.read(f'{prefix}/BUILD.json'))
            self.assertEqual(internal, {key: value for key, value in info.items() if key != 'files'})
            self.assertNotIn('files', internal)

    def test_linux_tar_keeps_gui_cli_libs_assets_and_executable_bits(self):
        bundle = self.bundle('linux-x86_64')
        receipt = self.package(bundle, 'linux-x86_64')
        info, prefix = self.receipt(receipt, 'linux-x86_64')
        self.assertEqual(len(info['files']), 1)
        with tarfile.open(receipt.parent / f'{prefix}.tar.gz', 'r:gz') as archive:
            self.assert_no_private_entries(archive.getnames())
            for name in pack.REQUIRED['linux-x86_64']:
                self.assertGreater(archive.getmember(f'{prefix}/{name}').size, 0, name)
            for name in ('xenterm', 'xenterm-cli'):
                self.assertEqual(archive.getmember(f'{prefix}/{name}').mode & 0o777, 0o755)
            for source in pack.license_sources(self.root):
                self.assertEqual(archive.extractfile(f'{prefix}/licenses/{source.name}').read(), source.read_bytes())
            self.assertIn(f'{prefix}/lib/plugin.so', archive.getnames())
            internal = json.load(archive.extractfile(f'{prefix}/BUILD.json'))
            self.assertEqual(internal, {key: value for key, value in info.items() if key != 'files'})

    @unittest.skipIf(os.name == 'nt', 'Windows filesystems do not expose POSIX mode bits')
    def test_linux_keeps_additional_executables_and_rejects_nonexecutable_gui(self):
        bundle = self.bundle('linux-x86_64')
        extra = bundle / 'helper'
        extra.write_bytes(b'non executable synthetic contents')
        extra.chmod(0o751)
        receipt = self.package(bundle, 'linux-x86_64')
        prefix = receipt.name.removesuffix('-BUILD.json')
        with tarfile.open(receipt.parent / f'{prefix}.tar.gz', 'r:gz') as archive:
            self.assertEqual(archive.getmember(f'{prefix}/helper').mode, 0o751)
        (bundle / 'xenterm').chmod(0o644)
        with self.assertRaisesRegex(ValueError, 'not executable'):
            self.package(bundle, 'linux-x86_64', self.root / 'bad')

    def test_macos_native_commands_seal_metadata_licenses_cli_and_bridge(self):
        bundle = self.bundle('macos-aarch64')
        runner = FakeAppleTools()
        original_bridge = (bundle / 'Contents/Frameworks/libxenterm_bridge.dylib').read_bytes()
        receipt = self.package(bundle, 'macos-aarch64', command_runner=runner)
        info, prefix = self.receipt(receipt, 'macos-aarch64')
        self.assertEqual(info['signing'], {'status': 'ad-hoc', 'notarized': False})
        self.assertEqual([command[0] for command in runner.commands],
                         ['/usr/bin/ditto', *['/usr/bin/codesign'] * 4, '/usr/bin/ditto'])
        with zipfile.ZipFile(receipt.parent / f'{prefix}.zip') as archive:
            self.assert_no_private_entries(archive.namelist())
            base = f'{prefix}/xenterm_app.app/'
            for name in pack.REQUIRED['macos-aarch64']:
                self.assertGreater(archive.getinfo(base + name).file_size, 0)
            self.assertIn(base + 'Contents/_CodeSignature/CodeResources', archive.namelist())
            internal = json.loads(archive.read(base + 'Contents/Resources/BUILD.json'))
            self.assertEqual(internal, {key: value for key, value in info.items() if key != 'files'})
            for source in pack.license_sources(self.root):
                self.assertEqual(archive.read(base + f'Contents/Resources/licenses/{source.name}'), source.read_bytes())
        self.assertEqual((bundle / 'Contents/Frameworks/libxenterm_bridge.dylib').read_bytes(), original_bridge)
        self.assertFalse((bundle / 'Contents/Resources/BUILD.json').exists())

    def symlink(self, target, link, directory=False):
        try:
            link.symlink_to(str(Path(target)), target_is_directory=directory)
        except OSError as error:
            self.skipTest(f'Host cannot create symbolic links: {error}')

    def test_macos_framework_links_are_preserved_and_required_files_resolve(self):
        bundle = self.bundle('macos-aarch64')
        for framework_name, binary in (('App', 'App'), ('FlutterMacOS', 'FlutterMacOS')):
            framework = bundle / f'Contents/Frameworks/{framework_name}.framework'
            version = framework / 'Versions/A'
            version.mkdir(parents=True)
            for name in (binary, 'Resources'):
                (framework / name).rename(version / name)
            self.symlink('A', framework / 'Versions/Current', directory=True)
            self.symlink(f'Versions/Current/{binary}', framework / binary)
            self.symlink('Versions/Current/Resources', framework / 'Resources', directory=True)
        receipt = self.package(bundle, 'macos-aarch64')
        _, prefix = self.receipt(receipt, 'macos-aarch64')
        with zipfile.ZipFile(receipt.parent / f'{prefix}.zip') as archive:
            name = f'{prefix}/xenterm_app.app/Contents/Frameworks/App.framework/Resources'
            self.assertTrue(stat.S_ISLNK(archive.getinfo(name).external_attr >> 16))
            self.assertEqual(archive.read(name), b'Versions/Current/Resources')

    def test_macos_unsafe_framework_link_and_payload_root_links_are_rejected(self):
        bundle = self.bundle('macos-aarch64')
        link = bundle / 'Contents/Frameworks/App.framework/leak'
        self.symlink(os.path.relpath(self.msi, link.parent), link)
        with self.assertRaisesRegex(ValueError, 'Unsafe bundle link'):
            self.package(bundle, 'macos-aarch64')
        link.unlink()
        alias = self.root / 'alias'
        self.symlink(bundle, alias, directory=True)
        with self.assertRaisesRegex(ValueError, 'Path links'):
            self.package(alias, 'macos-aarch64')

    def test_all_platforms_reject_each_missing_or_empty_required_file(self):
        for platform in pack.PLATFORMS:
            bundle = self.bundle(platform)
            for name in pack.REQUIRED[platform]:
                with self.subTest(platform=platform, name=name):
                    path = bundle / name
                    original = path.read_bytes()
                    path.unlink()
                    with self.assertRaisesRegex(ValueError, 'Missing or empty runtime file'):
                        self.package(bundle, platform)
                    path.write_bytes(b'')
                    with self.assertRaisesRegex(ValueError, 'Missing or empty runtime file'):
                        self.package(bundle, platform)
                    path.write_bytes(original)
                    if '/MacOS/' in f'/{name}' or name in ('xenterm', 'xenterm-cli'):
                        path.chmod(0o755)
        self.assertFalse((self.root / 'dist').exists())

    def test_rejects_mismatched_sources_versions_and_invalid_identities(self):
        bundle = self.bundle('windows-x86_64')
        for override in (dict(source_sha='main'), dict(source_sha='A' * 40),
                         dict(upstream_sha='short'), dict(run_id='12x'),
                         dict(version='1.2.3'), dict(version='xt-beta-ccccccc-T1794440000'),
                         dict(source_sha='a' * 7 + 'c' * 33),
                         dict(version='xt-beta-aaaaaaa-T1794440001')):
            with self.subTest(override=override), self.assertRaises(ValueError):
                self.package(bundle, 'windows-x86_64', **override)
        self.assertFalse((self.root / 'dist').exists())

    def test_all_platforms_reject_a_version_not_embedded_in_the_cli_or_bridge(self):
        for platform in pack.PLATFORMS:
            bundle = self.bundle(platform)
            names = (('xenterm-cli.exe', 'xenterm_bridge.dll') if platform == 'windows-x86_64' else
                     ('xenterm-cli', 'lib/libxenterm_bridge.so') if platform == 'linux-x86_64' else
                     ('Contents/MacOS/xenterm-cli', 'Contents/Frameworks/libxenterm_bridge.dylib'))
            for name in names:
                with self.subTest(platform=platform, name=name):
                    path = bundle / name
                    original = path.read_bytes()
                    path.write_bytes(b'wrong runtime version')
                    with self.assertRaisesRegex(ValueError, 'Runtime binary version mismatch'):
                        self.package(bundle, platform)
                    path.write_bytes(original)

    def test_windows_rejects_tampered_core_crt_engine_aot_and_plugin(self):
        bundle = self.bundle('windows-x86_64')
        for name in ('xenterm.exe', 'xenterm-cli.exe', 'xenterm_bridge.dll', *CRT_FILES,
                     'flutter_windows.dll', 'data/app.so', 'plugins/plugin.dll', 'plugins/helper.exe'):
            with self.subTest(name=name):
                path = bundle / name
                original = path.read_bytes()
                path.write_bytes(original + b'tamper')
                with self.assertRaisesRegex(ValueError, 'Binary provenance mismatch'):
                    self.package(bundle, 'windows-x86_64')
                path.write_bytes(original)

    def test_windows_rejects_unlisted_binary_or_missing_hash_and_external_mode(self):
        bundle = self.bundle('windows-x86_64')
        extra = bundle / 'unlisted.dll'
        extra.write_bytes(b'unlisted executable fixture')
        with self.assertRaisesRegex(ValueError, 'Binary provenance mismatch'):
            self.package(bundle, 'windows-x86_64')
        extra.unlink()
        for mutation in ('hash', 'mode', 'tested', 'source', 'crt'):
            with self.subTest(mutation=mutation):
                info = self.runtime(bundle)
                if mutation == 'hash':
                    del info['binary_sha256']['flutter_windows.dll']
                elif mutation == 'mode':
                    info['runtime_mode'] = 'external-prerequisite'
                elif mutation == 'tested':
                    info['clean_windows_tested'] = True
                elif mutation == 'source':
                    info['source_sha'] = 'c' * 40
                else:
                    info['crt_files'].append('../outside.dll')
                (bundle / 'WINDOWS-RUNTIME.json').write_text(json.dumps(info), encoding='utf-8')
                with self.assertRaises(ValueError):
                    self.package(bundle, 'windows-x86_64')

    def test_windows_requires_msi_and_numeric_product_version(self):
        bundle = self.bundle('windows-x86_64')
        for overrides in (dict(msi=None), dict(msi_product_version=None), dict(msi_product_version=VERSION),
                          dict(msi_product_version='1.2.3.4'), dict(msi_product_version='256.1.1'),
                          dict(msi_product_version='1.256.1'), dict(msi_product_version='1.2.65536'),
                          dict(msi=self.root / 'missing.msi')):
            with self.subTest(overrides=overrides), self.assertRaises(ValueError):
                self.package(bundle, 'windows-x86_64', **overrides)
        for platform in ('linux-x86_64', 'macos-aarch64'):
            with self.assertRaisesRegex(ValueError, 'Windows-only'):
                self.package(self.bundle(platform), platform, msi=self.msi)

    def test_refuses_every_existing_canonical_output_without_touching_it(self):
        for platform in pack.PLATFORMS:
            bundle = self.bundle(platform)
            prefix = f'XenTerm-Next-Flutter-{SHA}-{platform}'
            names = [prefix + ('.tar.gz' if platform == 'linux-x86_64' else '.zip'),
                     prefix + '-BUILD.json', prefix + '.sha256']
            if platform == 'windows-x86_64':
                names.append(prefix + '.msi')
            for i, name in enumerate(names):
                with self.subTest(platform=platform, name=name):
                    output = self.root / f'dist-{platform}-{i}'
                    output.mkdir()
                    (output / name).write_bytes(b'existing verified output')
                    with self.assertRaises(FileExistsError):
                        self.package(bundle, platform, output)
                    self.assertEqual(list(output.iterdir()), [output / name])
                    self.assertEqual((output / name).read_bytes(), b'existing verified output')

    def test_missing_license_aborts_before_creating_artifacts(self):
        bundle = self.bundle('linux-x86_64')
        for name in ('LICENSE', 'THIRD_PARTY_NOTICES.md'):
            path = self.root / name
            original = path.read_bytes()
            path.unlink()
            with self.assertRaisesRegex(ValueError, 'runtime file'):
                self.package(bundle, 'linux-x86_64')
            path.write_bytes(original)
        for path in (self.root / 'assets/fonts').iterdir():
            path.unlink()
        with self.assertRaisesRegex(ValueError, 'font licenses'):
            self.package(bundle, 'linux-x86_64')
        self.assertFalse((self.root / 'dist').exists())

    def test_no_native_mac_commands_run_on_other_hosts(self):
        bundle = self.bundle('macos-aarch64')
        with mock.patch.object(pack.sys, 'platform', 'win32'), mock.patch.object(pack.subprocess, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'requires native'):
                self.package(bundle, 'macos-aarch64', command_runner=None)
            run.assert_not_called()

    def test_output_cannot_be_inside_the_payload(self):
        bundle = self.bundle('linux-x86_64')
        with self.assertRaisesRegex(ValueError, 'outside'):
            self.package(bundle, 'linux-x86_64', bundle / 'dist')

    def test_hashes_and_version_search_stream_across_chunk_boundaries(self):
        file = self.root / 'large-binary'
        contents = b'x' * 13 + VERSION.encode('utf-16-le') + b'\0' * 1000
        file.write_bytes(contents)
        with mock.patch.object(pack, 'CHUNK_SIZE', 17), mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('unbounded read')):
            self.assertTrue(pack.contains_version(file, VERSION))
            self.assertEqual(pack.sha256(file), hashlib.sha256(contents).hexdigest())
            self.assertFalse(pack.contains_version(file, 'xt-v999.999.999'))

    def test_cli_accepts_explicit_identity_without_git_or_payload_execution(self):
        bundle = self.bundle('windows-x86_64')
        output = self.root / 'cli-dist'
        arguments = ['--bundle', str(bundle), '--platform', 'windows-x86_64', '--output', str(output),
                     '--version', VERSION, '--source-sha', SHA, '--upstream-sha', UPSTREAM,
                     '--run-id', '123456789', '--msi', str(self.msi), '--msi-product-version', '1.2.345']
        with mock.patch.object(pack.subprocess, 'run', side_effect=AssertionError('must not execute payload')):
            with redirect_stdout(io.StringIO()) as stdout:
                self.assertEqual(pack.main(arguments), 0)
        self.assertTrue(Path(stdout.getvalue().strip()).is_file())
        for flag in ('--platform', '--source-sha', '--upstream-sha', '--run-id'):
            missing = list(arguments)
            position = missing.index(flag)
            del missing[position:position + 2]
            with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                pack.main(missing)
            self.assertEqual(error.exception.code, 2)


def ps_quote(value):
    return "'" + str(value).replace("'", "''") + "'"


@unittest.skipUnless(POWERSHELLS, 'No PowerShell available for mocked launcher/CRT tests')
class WindowsScriptTests(unittest.TestCase):
    setUp = DesktopPackageTests.setUp
    bundle = DesktopPackageTests.bundle
    runtime = DesktopPackageTests.runtime

    def run_ps(self, executable, code):
        code = "[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)\n$ProgressPreference = 'SilentlyContinue'\n" + code
        command = base64.b64encode(code.encode('utf-16-le')).decode('ascii')
        environment = os.environ.copy()
        if os.name == 'nt' and Path(executable).name.lower() == 'powershell.exe':
            # The Codex host inherits PS7's module path. Give PS5 its own modules.
            environment['PSModulePath'] = str(Path(os.environ['SystemRoot']) / 'System32/WindowsPowerShell/v1.0/Modules')
        return subprocess.run([executable, '-NoProfile', '-NonInteractive', '-EncodedCommand', command],
                              capture_output=True, text=True, encoding='utf-8', errors='replace', timeout=45,
                              env=environment,
                              creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)

    def test_launcher_checks_app_local_hashes_without_registry_or_gui(self):
        bundle = self.bundle('windows-x86_64')
        shutil.copyfile(HERE / 'Start-Flutter-Preview.ps1', bundle / 'Start-Flutter-Preview.ps1')
        wrapper = f"""$ErrorActionPreference = 'Stop'
function Get-ItemProperty {{ throw 'App-local mode must not consult the registry' }}
function Start-Process {{ param($FilePath, $WorkingDirectory) Write-Output 'MOCK-GUI-LAUNCH' }}
& {ps_quote(bundle / 'Start-Flutter-Preview.ps1')}
"""
        for executable in POWERSHELLS:
            with self.subTest(shell=executable):
                result = self.run_ps(executable, wrapper)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn('MOCK-GUI-LAUNCH', result.stdout)
                crt = bundle / 'msvcp140.dll'
                original = crt.read_bytes()
                crt.write_bytes(b'tampered CRT')
                result = self.run_ps(executable, wrapper)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn('MOCK-GUI-LAUNCH', result.stdout)
                self.assertIn('provenance mismatch', result.stderr)
                crt.write_bytes(original)

    def test_launcher_keeps_external_runtime_registry_gate(self):
        bundle = self.bundle('windows-x86_64')
        shutil.copyfile(HERE / 'Start-Flutter-Preview.ps1', bundle / 'Start-Flutter-Preview.ps1')
        info = dict(runtime_mode='external-prerequisite', minimum_vc_runtime='14.44.35211.0')
        (bundle / 'WINDOWS-RUNTIME.json').write_text(json.dumps(info), encoding='utf-8')
        for executable in POWERSHELLS:
            for installed, success in (('14.44.35211.0', True), ('14.1.0.0', False)):
                with self.subTest(shell=executable, installed=installed):
                    result = self.run_ps(executable, f"""
$ErrorActionPreference = 'Stop'
function Get-ItemProperty {{ [pscustomobject]@{{ Installed = 1; Version = 'v{installed}' }} }}
function Start-Process {{ param($FilePath, $WorkingDirectory) Write-Output 'MOCK-GUI-LAUNCH' }}
& {ps_quote(bundle / 'Start-Flutter-Preview.ps1')}
""")
                    self.assertEqual(result.returncode == 0, success, result.stderr)
                    self.assertEqual('MOCK-GUI-LAUNCH' in result.stdout, success)

    @unittest.skipUnless(os.name == 'nt', 'VS CRT bundler is Windows-only')
    def test_crt_helper_discovers_installed_redist_and_records_all_binaries(self):
        for executable in POWERSHELLS:
            with self.subTest(shell=executable):
                bundle = self.bundle('windows-x86_64')
                (bundle / 'WINDOWS-RUNTIME.json').unlink()
                for name in CRT_FILES:
                    (bundle / name).unlink()
                redist = self.root / f'redist-{self.count}'
                crt = redist / 'x64/Microsoft.VC143.CRT'
                crt.mkdir(parents=True)
                for name in CRT_FILES:
                    # A minimal x64 DLL header; signatures/version resources are
                    # supplied by mocks, never a claim about genuine fixture DLLs.
                    header = bytearray(256)
                    struct.pack_into('<H', header, 0, 0x5A4D)
                    struct.pack_into('<I', header, 0x3C, 128)
                    struct.pack_into('<IH', header, 128, 0x4550, 0x8664)
                    struct.pack_into('<H', header, 150, 0x2000)
                    (crt / name).write_bytes(header + name.encode())
                wrapper = f"""
$ErrorActionPreference = 'Stop'
$env:VCToolsRedistDir = {ps_quote(redist)}
function Get-AuthenticodeSignature {{
  param($LiteralPath)
  [pscustomobject]@{{ Status = 'Valid'; SignerCertificate = [pscustomobject]@{{ Subject = 'CN=Microsoft Corporation, O=Microsoft Corporation, C=US' }} }}
}}
function Get-Item {{
  param($LiteralPath, [switch]$Force)
  $item = Microsoft.PowerShell.Management\\Get-Item -LiteralPath $LiteralPath -Force:$Force
  if ($item.Name -eq 'xenterm.exe') {{
    return [pscustomobject]@{{ Attributes = $item.Attributes; FullName = $item.FullName; PSIsContainer = $item.PSIsContainer; Length = $item.Length; VersionInfo = [pscustomobject]@{{ FileVersion = '{VERSION}'; ProductVersion = '{VERSION}' }} }}
  }} elseif ($item.Extension -eq '.dll') {{
    return [pscustomobject]@{{ Attributes = $item.Attributes; FullName = $item.FullName; PSIsContainer = $item.PSIsContainer; Length = $item.Length; VersionInfo = [pscustomobject]@{{ FileMajorPart = 14; FileMinorPart = 44; FileBuildPart = 35211; FilePrivatePart = 0 }} }}
  }}
  $item
}}
& {ps_quote(HERE / 'Bundle-Flutter-VCRuntime.ps1')} -BundleDir {ps_quote(bundle)} -Version '{VERSION}' -SourceSha '{SHA}'
"""
                result = self.run_ps(executable, wrapper)
                self.assertEqual(result.returncode, 0, result.stderr)
                info = json.loads((bundle / 'WINDOWS-RUNTIME.json').read_text(encoding='utf-8-sig'))
                self.assertEqual(info['core_version'], VERSION)
                self.assertEqual(info['source_sha'], SHA)
                self.assertEqual(info['runtime_mode'], 'app-local')
                self.assertEqual(info['minimum_vc_runtime'], '14.44.35211.0')
                self.assertFalse(info['clean_windows_tested'])
                self.assertEqual(set(info['crt_files']), set(CRT_FILES))
                paths = pack.payload_paths(bundle, 'windows-x86_64')
                expected = {p.relative_to(bundle).as_posix(): pack.sha256(p) for p in paths
                            if p.is_file() and p.suffix.lower() in pack.BINARY_SUFFIXES}
                self.assertEqual(info['binary_sha256'], expected)
                before = (bundle / 'WINDOWS-RUNTIME.json').read_bytes()
                result = self.run_ps(executable, wrapper)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('already exists', result.stderr)
                self.assertEqual((bundle / 'WINDOWS-RUNTIME.json').read_bytes(), before)
                (bundle / 'WINDOWS-RUNTIME.json').unlink()
                result = self.run_ps(executable, wrapper)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('overwrite CRT', result.stderr)
                for name in CRT_FILES:
                    (bundle / name).unlink()
                result = self.run_ps(executable, wrapper.replace("Status = 'Valid'", "Status = 'NotSigned'"))
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('valid Microsoft Authenticode', result.stderr)
                self.assertFalse((bundle / 'WINDOWS-RUNTIME.json').exists())
                self.assertFalse((bundle / 'msvcp140.dll').exists())


if __name__ == '__main__':
    unittest.main()
