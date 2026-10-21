#!/usr/bin/env python3
"""Offline release policy and failure-injection checks; no GitHub mutations."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('next_beta', ROOT / 'scripts/next_beta.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
SHA = 'a' * 40
OLD = 'b' * 40
RUN = {'repository': {'full_name': release.REPOSITORY}, 'head_repository': {'full_name': release.REPOSITORY},
       'path': release.WORKFLOW, 'head_branch': 'main', 'head_sha': SHA,
       'event': 'push', 'status': 'completed', 'conclusion': 'success'}


class FakeGitHub:
    def __init__(self, fail=None, existing=True):
        self.fail = fail
        self.events = []
        self.ref = OLD
        self.current = {'id': 1, 'tag_name': 'beta', 'draft': False, 'body': 'old'} if existing else None
        self.files = {release.prefix(OLD) + 'windows-x86_64.zip': b'old', 'manual.txt': b'keep'}
        self.ids = {name: i + 1 for i, name in enumerate(self.files)}

    def check(self, sha, ci):
        self.events.append('check')
        if self.fail == 'main_changed' and 'upload' in self.events:
            raise RuntimeError('main moved')

    def release(self):
        return self.current

    def api(self, endpoint, method='GET', body=None):
        self.events.append(method + ':' + endpoint)
        if method == 'POST':
            self.current = body | {'id': 1}
            return self.current
        if method == 'PATCH':
            if self.fail == 'metadata':
                raise RuntimeError('metadata failure')
            self.current.update(body)
        if method == 'DELETE':
            asset_id = int(endpoint.rsplit('/', 1)[1])
            name = next(name for name in self.files if self.ids[name] == asset_id)
            del self.files[name]
        return self.current

    def assets(self, rid):
        return [{'id': self.ids[name], 'name': name, 'size': len(data), 'state': 'uploaded'} for name, data in self.files.items()]

    def upload(self, path):
        self.events.append('upload')
        if self.fail == 'upload' and self.events.count('upload') == 2:
            raise RuntimeError('upload failed')
        self.ids[path.name] = max(self.ids.values(), default=0) + 1
        self.files[path.name] = path.read_bytes()

    def verify(self, asset, expected, size):
        self.events.append('verify')
        data = self.files[asset['name']]
        if self.fail == 'checksum' or release.hashlib.sha256(data).hexdigest() != expected or len(data) != size:
            raise RuntimeError('checksum mismatch')

    def tag(self):
        return self.ref

    def move_tag(self, sha, old):
        self.events.append('move_tag')
        if self.fail == 'lease':
            raise RuntimeError('lease rejected')
        assert old == self.ref
        self.ref = sha


def archives(path, manifest_sha=SHA, versions=None):
    import io
    import json
    import tarfile
    import zipfile
    for platform, suffix in release.PLATFORMS.items():
        root = ('XenTerm Next.app/Contents/Resources/' if platform.startswith('macos') else release.prefix(SHA) + platform + '/')
        binary = ('XenTerm Next.app/Contents/MacOS/xenterm' if platform.startswith('macos') else root + ('xenterm.exe' if platform.startswith('windows') else 'xenterm'))
        files = {root + 'LICENSE': b'MIT', root + 'THIRD_PARTY_NOTICES.md': b'notices', binary: b'synthetic binary',
                 root + 'BUILD.json': json.dumps({'repository': release.REPOSITORY, 'sha': manifest_sha, 'platform': platform,
                                                 'ci_run': '12', 'build_run': '34',
                                                 **((versions or {}).get(platform) or {
                                                     'version_scheme': 'cargo-legacy', 'runtime_version': '1.2.3',
                                                     'package_version': '1.2.3', 'version_timestamp': None,
                                                     'binary_version': 'xenterm 1.2.3'})}).encode()}
        output = path / (release.prefix(SHA) + platform + suffix)
        if suffix == '.zip':
            with zipfile.ZipFile(output, 'w') as archive:
                for name, data in files.items(): archive.writestr(name, data)
        else:
            with tarfile.open(output, 'w:gz') as archive:
                for name, data in files.items():
                    item = tarfile.TarInfo(name); item.size = len(data)
                    archive.addfile(item, io.BytesIO(data))


class GateTests(unittest.TestCase):
    def test_current_main_success_is_allowed(self):
        release.gate(release.REPOSITORY, SHA, SHA, RUN)

    def test_dispatch_ci_on_main_allowed(self):
        release.gate(release.REPOSITORY, SHA, SHA, RUN | {'event': 'workflow_dispatch'})

    def test_untrusted_or_failed_ci_rejected(self):
        cases = [('head_branch', 'upstream-main'), ('head_branch', 'feature'), ('head_sha', OLD),
                 ('event', 'pull_request'), ('event', 'pull_request_target'), ('status', 'in_progress'),
                 ('conclusion', 'failure'), ('conclusion', 'cancelled'), ('conclusion', 'skipped'),
                 ('path', '.github/workflows/impostor.yml'),
                 ('head_repository', {'full_name': 'attacker/fork'}),
                 ('repository', {'full_name': 'ixbaicn/XenTerm'})]
        for key, value in cases:
            with self.subTest(key=key, value=value), self.assertRaises(RuntimeError):
                release.gate(release.REPOSITORY, SHA, SHA, RUN | {key: value})

    def test_old_sha_or_wrong_repository_rejected(self):
        for repo, sha, main in [(release.REPOSITORY, SHA, OLD), ('ixbaicn/XenTerm', SHA, SHA), (release.REPOSITORY, 'a'*7, 'a'*7)]:
            with self.subTest(repo=repo, sha=sha), self.assertRaises(RuntimeError):
                release.gate(repo, sha, main, RUN)


class AssetTests(unittest.TestCase):
    def test_required_platform_set_and_checksums(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp); archives(path)
            hashes = release.prepare_assets(path, SHA, '12', '34')
            self.assertEqual(len(hashes), 5)
            sums = (path / (release.prefix(SHA) + 'SHA256SUMS')).read_text(encoding='utf-8')
            for file in path.iterdir():
                self.assertEqual(hashes[file.name], release.digest(file))
                if not file.name.endswith('SHA256SUMS'):
                    self.assertIn(hashes[file.name] + '  ' + file.name, sums)

    def test_modern_versions_are_shared_and_recorded(self):
        import json
        details = dict(version_scheme='xt-v1', runtime_version='xt-beta-aaaaaaa-T1792060000',
                       package_version='1.2.3', version_timestamp=1792060000,
                       binary_version='xenterm xt-beta-aaaaaaa-T1792060000')
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp); archives(path, versions={p: details for p in release.PLATFORMS})
            release.prepare_assets(path, SHA, '12', '34')
            manifest = json.loads((path / (release.prefix(SHA) + 'BUILD.json')).read_text())
            self.assertEqual(manifest['runtime_version'], details['runtime_version'])
            self.assertEqual(manifest['package_version'], '1.2.3')

    def test_cross_platform_runtime_mismatch_rejected(self):
        first = dict(version_scheme='xt-v1', runtime_version='xt-beta-aaaaaaa-T1792060000',
                     package_version='1.2.3', version_timestamp=1792060000,
                     binary_version='xenterm xt-beta-aaaaaaa-T1792060000')
        second = first | dict(runtime_version='xt-beta-aaaaaaa-T1792060001', version_timestamp=1792060001,
                              binary_version='xenterm xt-beta-aaaaaaa-T1792060001')
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            archives(path, versions={p: second if p.startswith('macos') else first for p in release.PLATFORMS})
            with self.assertRaisesRegex(RuntimeError, 'Platform archives disagree'):
                release.prepare_assets(path, SHA, '12', '34')

    def test_missing_empty_or_extra_artifact_rejected(self):
        for kind in ('missing', 'empty', 'extra'):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp); archives(path)
                one = next(path.iterdir())
                if kind == 'missing': one.unlink()
                if kind == 'empty': one.write_bytes(b'')
                if kind == 'extra': (path / 'unexpected.zip').write_bytes(b'x')
                with self.assertRaises(RuntimeError): release.prepare_assets(path, SHA, '12', '34')

    def test_repeated_prepare_is_idempotent(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp); archives(path)
            first = release.prepare_assets(path, SHA, '12', '34')
            self.assertEqual(first, release.prepare_assets(path, SHA, '12', '34'))

    def test_retry_rejects_modified_provenance_or_checksums(self):
        for suffix in ('BUILD.json', 'SHA256SUMS'):
            with self.subTest(suffix=suffix), tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp); archives(path)
                release.prepare_assets(path, SHA, '12', '34')
                file = path / (release.prefix(SHA) + suffix)
                file.write_text('{}' if suffix == 'BUILD.json' else 'wrong', encoding='utf-8')
                with self.assertRaises(RuntimeError): release.prepare_assets(path, SHA, '12', '34')

    def test_retry_does_not_overwrite_foreign_build_run(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp); archives(path)
            release.prepare_assets(path, SHA, '12', '34')
            before = {p.name: p.read_bytes() for p in path.iterdir()}
            with self.assertRaises(RuntimeError): release.prepare_assets(path, SHA, '12', '35')
            self.assertEqual(before, {p.name: p.read_bytes() for p in path.iterdir()})

    def test_unsafe_zip_paths_and_symlinks_rejected(self):
        import zipfile
        for kind in ('traversal', 'absolute', 'windows', 'symlink'):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp); archives(path)
                archive_path = next(path.glob('*windows*.zip'))
                with zipfile.ZipFile(archive_path, 'a') as archive:
                    if kind == 'symlink':
                        item = zipfile.ZipInfo('escape'); item.create_system = 3
                        item.external_attr = (0o120777 << 16)
                        archive.writestr(item, '../../outside')
                    else:
                        archive.writestr({'traversal': '../outside', 'absolute': '/outside', 'windows': 'C:\\outside'}[kind], b'x')
                with self.assertRaises(RuntimeError): release.prepare_assets(path, SHA, '12', '34')

    def test_tar_hardlinks_and_path_traversal_rejected(self):
        import io
        import tarfile
        for kind in ('symlink', 'hardlink', 'traversal'):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp); archives(path)
                archive_path = next(path.glob('*.tar.gz'))
                with tarfile.open(archive_path, 'w:gz') as archive:
                    item = tarfile.TarInfo('../escape' if kind == 'traversal' else 'escape')
                    if kind != 'traversal':
                        item.type = tarfile.SYMTYPE if kind == 'symlink' else tarfile.LNKTYPE
                        item.linkname = '/outside'
                    archive.addfile(item, io.BytesIO(b''))
                with self.assertRaises(RuntimeError): release.prepare_assets(path, SHA, '12', '34')

    def test_safe_but_outside_root_file_rejected(self):
        import zipfile
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp); archives(path)
            with zipfile.ZipFile(next(path.glob('*windows*.zip')), 'a') as archive:
                archive.writestr('unrelated/safe.txt', b'extra')
            with self.assertRaises(RuntimeError): release.prepare_assets(path, SHA, '12', '34')

    def test_wrong_source_inside_archive_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp); archives(path, manifest_sha=OLD)
            with self.assertRaises(RuntimeError): release.prepare_assets(path, SHA, '12', '34')

    def test_corrupt_archive_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp); archives(path)
            next(path.glob('*.zip')).write_bytes(b'not a zip')
            with self.assertRaises(Exception): release.prepare_assets(path, SHA, '12', '34')

    def test_cleanup_recognizes_only_owned_names(self):
        own = release.prefix(OLD) + 'windows-x86_64.zip'
        current = release.prefix(SHA) + 'linux-x86_64.tar.gz'
        names = [own, current, 'manual.txt', 'XenTerm-Next-beta-no-sha-linux-x86_64.tar.gz',
                 release.prefix(OLD) + 'personal-backup.zip']
        stale = release.obsolete_assets([{'name': name} for name in names], {current})
        self.assertEqual(stale, [{'name': own}])


class PublisherTests(unittest.TestCase):
    def test_success_keeps_manual_attachment_and_replaces_only_managed_old_assets(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp); archives(path); api = FakeGitHub()
            release.publish(api, path, SHA, '12', '34')
            self.assertEqual(api.ref, SHA)
            self.assertIn('manual.txt', api.files)
            self.assertNotIn(release.prefix(OLD) + 'windows-x86_64.zip', api.files)
            self.assertEqual(len(api.files), 6)
            self.assertGreater(api.events.index('move_tag'), max(i for i, event in enumerate(api.events) if event == 'verify'))
            self.assertGreater(next(i for i, event in enumerate(api.events) if event.startswith('DELETE:')), api.events.index('PATCH:releases/1'))

    def test_any_pre_cleanup_failure_preserves_old_and_manual_downloads(self):
        for failure in ('upload', 'checksum', 'main_changed', 'lease', 'metadata'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp); archives(path); api = FakeGitHub(fail=failure)
                with self.assertRaises(RuntimeError): release.publish(api, path, SHA, '12', '34')
                self.assertIn(release.prefix(OLD) + 'windows-x86_64.zip', api.files)
                self.assertIn('manual.txt', api.files)
                self.assertFalse(any(event.startswith('DELETE:') for event in api.events))
                if failure != 'metadata': self.assertEqual(api.ref, OLD)

    def test_initial_release_is_draft_until_assets_verified(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp); archives(path); api = FakeGitHub(existing=False, fail='upload')
            with self.assertRaises(RuntimeError): release.publish(api, path, SHA, '12', '34')
            self.assertTrue(api.current['draft'])
            self.assertEqual(api.current['target_commitish'], SHA)

    def test_immutable_release_is_not_modified(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp); archives(path); api = FakeGitHub(); api.current['immutable'] = True
            with self.assertRaises(RuntimeError): release.publish(api, path, SHA, '12', '34')
            self.assertNotIn('upload', api.events)
            self.assertNotIn('move_tag', api.events)


class PackagingTests(unittest.TestCase):
    def test_windows_toolchain_platform_does_not_override_package_identity(self):
        import os
        import sys
        from unittest.mock import patch
        sys.modules['next_beta'] = release
        package_spec = importlib.util.spec_from_file_location('next_beta_package', ROOT / 'scripts/next_beta_package.py')
        package = importlib.util.module_from_spec(package_spec)
        package_spec.loader.exec_module(package)
        with patch.dict(os.environ, {'PLATFORM': 'x64', 'XENTERM_PACKAGE_PLATFORM': 'windows-x86_64',
                                     'TARGET': 'x86_64-pc-windows-msvc', 'SOURCE_SHA': SHA}), \
             patch.object(package, 'package') as build:
            package.package_from_environment()
        self.assertEqual(build.call_args.args[1], 'windows-x86_64')

    def test_real_archive_layout_for_three_platforms_with_synthetic_binaries(self):
        # Exercise the actual packaging code and archive structures. Native
        # codesign/ditto/sips are simulated here; genuine OS builds remain CI's
        # job, and this is not a desktop-startup or signing acceptance test.
        import json
        import os
        import sys
        import zipfile
        from unittest.mock import patch
        sys.modules['next_beta'] = release
        package_spec = importlib.util.spec_from_file_location('next_beta_package', ROOT / 'scripts/next_beta_package.py')
        package = importlib.util.module_from_spec(package_spec)
        package_spec.loader.exec_module(package)
        from itertools import product
        for modern, platform in product((False, True), release.PLATFORMS):
            with self.subTest(platform=platform, modern=modern), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                (root / 'Cargo.toml').write_text('# 中文注释\n[package]\nversion = "1.2.3"\n', encoding='utf-8')
                runtime = 'xt-beta-aaaaaaa-T1792060000' if modern else '1.2.3'
                if modern:
                    (root / 'src').mkdir(); (root / 'src/version.rs').write_text('// Runtime fixture')
                    (root / 'scripts').mkdir(); (root / 'scripts/version.sh').write_text('# Resolver fixture')
                (root / 'LICENSE').write_text('MIT attribution', encoding='utf-8')
                (root / 'THIRD_PARTY_NOTICES.md').write_text('notices', encoding='utf-8')
                (root / 'licenses').mkdir(); (root / 'licenses/dependency.txt').write_text('license', encoding='utf-8')
                (root / 'assets').mkdir(); (root / 'assets/icon@512.png').write_bytes(b'synthetic image')
                (root / 'assets/Info.plist').write_bytes(package.plistlib.dumps({'CFBundleExecutable': 'xenterm'}))
                target = root / 'target/synthetic/release'; target.mkdir(parents=True)
                (target / ('xenterm.exe' if platform.startswith('windows') else 'xenterm')).write_bytes(b'synthetic binary')
                def output(args, **kwargs):
                    self.assertEqual(kwargs['timeout'], 30)
                    if args[0] == 'git': return SHA
                    if args[0] == 'rustc': return 'rustc synthetic'
                    if args[0] == 'ldd': return 'libc.so.6 => /lib/libc.so.6\n'
                    return 'xenterm ' + runtime + '\n'
                def run(args, **kwargs):
                    if args[0] == 'ditto':
                        app = Path(args[-2]); destination = Path(args[-1])
                        with zipfile.ZipFile(destination, 'w') as archive:
                            for file in app.rglob('*'):
                                if file.is_file(): archive.write(file, file.relative_to(app.parent))
                original_read = Path.read_text
                def simulated_windows_read(path, *args, **kwargs):
                    if path.name == 'Cargo.toml':
                        self.assertEqual(kwargs.get('encoding'), 'utf-8')
                    return original_read(path, *args, **kwargs)
                with patch.object(Path, 'read_text', simulated_windows_read), \
                     patch.dict(os.environ, {'CI_RUN_ID': '12', 'GITHUB_RUN_ID': '34',
                                             'XT_VERSION': runtime if modern else ''}, clear=True), \
                     patch.object(package.subprocess, 'check_output', side_effect=output), \
                     patch.object(package.subprocess, 'run', side_effect=run):
                    package.package(root, platform, 'synthetic', SHA)
                artifact = next((root / 'dist').iterdir())
                release.verify_archive(artifact, platform, SHA, '12', '34')
                if platform.startswith('macos'):
                    with zipfile.ZipFile(artifact) as archive:
                        info = package.plistlib.loads(archive.read('XenTerm Next.app/Contents/Info.plist'))
                        self.assertEqual(info['CFBundleIdentifier'], 'io.github.xue-ding-e.xenterm-next')
                        self.assertEqual(info['CFBundleDisplayName'], 'XenTerm Next')
                        self.assertEqual(info['CFBundleShortVersionString'], '1.2.3')
                        self.assertEqual(info['CFBundleVersion'], '1.2.3')


class WorkflowTests(unittest.TestCase):
    def test_single_publisher_and_pinned_actions(self):
        import re
        workflow = (ROOT / '.github/workflows/next-beta.yml').read_text(encoding='utf-8')
        self.assertIn('cancel-in-progress: false', workflow)
        self.assertIn("run.head_sha !== main.object.sha", workflow)
        self.assertNotIn('pull_request:', workflow)
        self.assertEqual(workflow.count('contents: write'), 1)
        for action in re.findall(r'uses: ([^\s]+)', workflow):
            if not action.startswith('./'):
                self.assertRegex(action, r'@[0-9a-f]{40}$')
        original = (ROOT / '.github/workflows/release.yml').read_text(encoding='utf-8')
        self.assertIn('[ "${INPUT_MODE:-beta}" = "beta" ]', original)
        self.assertIn('Next beta is published only by next-beta.yml', original)

    def test_candidate_packaging_cannot_publish(self):
        ci = (ROOT / '.github/workflows/ci.yml').read_text(encoding='utf-8').split('  next-beta-dry-run:', 1)[1]
        self.assertIn("github.ref == 'refs/heads/ci/next-beta-preview-20261014'", ci)
        self.assertIn('needs: [desktop, migration-e2e, headless-e2e, msrv-headless, windows-ssh, installer, installer-windows, installer-macos]', ci)
        self.assertNotIn('contents: write', ci)
        build = (ROOT / '.github/workflows/next-beta-build.yml').read_text(encoding='utf-8')
        self.assertNotIn('contents: write', build)
        self.assertNotIn('scripts/next_beta.py', build)
        self.assertIn('prepare_assets(', build)
        import re
        for action in re.findall(r'uses: ([^\s]+)', build):
            self.assertRegex(action, r'@[0-9a-f]{40}$')

    def test_ci_wires_policy_tests_and_preserves_security_regression(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text(encoding='utf-8')
        self.assertIn('python3 tests/next_beta_test.py', workflow)
        self.assertIn('ui::shell::file_dialog_tests::gui_export_', workflow)
        self.assertEqual(workflow.count('"scripts/next_beta*.py"'), 2)


if __name__ == '__main__':
    unittest.main()
