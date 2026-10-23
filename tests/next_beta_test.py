#!/usr/bin/env python3
"""Offline release policy and failure-injection checks; no GitHub mutations."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
import subprocess
from unittest.mock import patch
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
        self.current = {'id': 1, 'tag_name': 'beta', 'draft': False, 'body': 'old', 'upload_url': f'https://uploads.github.com/repos/{release.REPOSITORY}/releases/1/assets{{?name,label}}'} if existing else None
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
            self.current = body | {'id': 1, 'upload_url': f'https://uploads.github.com/repos/{release.REPOSITORY}/releases/1/assets{{?name,label}}'}
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
        return [self.record(name) for name in self.files]

    def record(self, name):
        asset_id = self.ids[name]
        return {'id': asset_id, 'name': name, 'size': len(self.files[name]), 'state': 'uploaded',
                'url': f'https://api.github.com/repos/{release.REPOSITORY}/releases/assets/{asset_id}',
                'browser_download_url': f'https://github.com/{release.REPOSITORY}/releases/download/beta/{name}'}

    def asset(self, asset_id):
        self.events.append('asset')
        name = next(n for n in self.files if self.ids[n] == asset_id)
        return self.record(name)

    def upload(self, path, release_info):
        self.events.append('upload')
        if self.fail == 'upload' and self.events.count('upload') == 2:
            raise RuntimeError('upload failed')
        self.ids[path.name] = max(self.ids.values(), default=0) + 1
        self.files[path.name] = path.read_bytes()
        return self.record(path.name)

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


class ReleaseSelectionTests(unittest.TestCase):
    def record(self, rid=1, draft=False):
        return {'id': rid, 'tag_name': 'beta', 'draft': draft,
                'url': f'https://api.github.com/repos/{release.REPOSITORY}/releases/{rid}',
                'html_url': f'https://github.com/{release.REPOSITORY}/releases/tag/beta',
                'upload_url': f'https://uploads.github.com/repos/{release.REPOSITORY}/releases/{rid}/assets{{?name,label}}'}

    def select(self, pages, exact):
        import json
        api = release.GitHub()
        with patch.object(api, 'command', return_value=json.dumps(pages)) as listing, \
                patch.object(api, 'api', return_value=exact) as get:
            return api.release()

    def test_release_reads_exact_tag_and_all_pages(self):
        import json
        api = release.GitHub()
        with patch.object(api, 'command', return_value=json.dumps([[self.record()]])) as listing, \
                patch.object(api, 'api', return_value=self.record()) as get:
            self.assertEqual(api.release(), self.record())
            listing.assert_called_once_with('gh', 'api', '--paginate', '--slurp',
                f'repos/{release.REPOSITORY}/releases?per_page=100', timeout=15)
            get.assert_called_once_with('releases/tags/beta')

    def test_draft_first_and_pagination_select_exact_published(self):
        expected = self.record()
        for pages in ([[self.record(2, True), expected]],
                      [[self.record(2, True)], [self.record(3, True), expected]]):
            with self.subTest(pages=pages):
                self.assertEqual(self.select(pages, expected), expected)

    def test_missing_or_only_drafts_rejected(self):
        for pages in ([[]], [[self.record(2, True)]]):
            with self.subTest(pages=pages), self.assertRaises(RuntimeError):
                self.select(pages, self.record())

    def test_duplicate_published_rejected(self):
        with self.assertRaises(RuntimeError):
            self.select([[self.record()], [self.record(4)]], self.record())

    def test_exact_tag_identity_must_match_unique_listed_release(self):
        for change in ({'id': 2}, {'id': True}, {'tag_name': 'other'}, {'draft': True},
                       {'url': 'https://api.github.com/repos/other/repo/releases/1'},
                       {'html_url': 'https://github.com/other/repo/releases/tag/beta'},
                       {'upload_url': 'https://uploads.github.com/repos/other/repo/releases/1/assets{?name,label}'}):
            with self.subTest(change=change), self.assertRaises(RuntimeError):
                self.select([[self.record()]], self.record() | change)

    def test_malformed_listing_identity_rejected(self):
        for change in ({'draft': None}, {'id': True}, {'url': 'wrong'}):
            with self.subTest(change=change), self.assertRaises(RuntimeError):
                self.select([[self.record() | change]], self.record())

    def test_api_error_is_not_permission_to_create(self):
        import json
        api = release.GitHub()
        with patch.object(api, 'command', return_value=json.dumps([[self.record()]])), \
                patch.object(api, 'api', side_effect=RuntimeError('API failure')) as get:
            with self.assertRaisesRegex(RuntimeError, 'API failure'):
                api.release()
            get.assert_called_once_with('releases/tags/beta')


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
    def test_upload_ids_work_when_lists_never_show_new_assets(self):
        class StaleList(FakeGitHub):
            def assets(self, rid):
                return [a for a in super().assets(rid)
                        if not a['name'].startswith(release.prefix(SHA))]
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp); archives(path); api = StaleList()
            release.publish(api, path, SHA, '12', '34')
            self.assertEqual(api.ref, SHA)
            self.assertEqual(api.events.count('upload'), 5)
            self.assertEqual(api.events.count('asset'), 10)
            self.assertEqual(api.events.count('verify'), 10)

    def test_asset_response_and_readback_identity_must_match(self):
        bad_fields = {'id': [True, 0, 999], 'name': ['wrong'], 'size': [-1],
                      'state': ['starter'], 'url': ['https://other.invalid/asset'],
                      'browser_download_url': ['https://github.com/other/repo/releases/download/beta/asset']}
        for where in ('upload', 'asset'):
            for field, values in bad_fields.items():
                for value in values:
                    class WrongIdentity(FakeGitHub):
                        def upload(self, path, release_info):
                            result = super().upload(path, release_info)
                            return result | {field: value} if where == 'upload' else result

                        def asset(self, asset_id):
                            result = super().asset(asset_id)
                            return result | {field: value} if where == 'asset' else result
                    with self.subTest(where=where, field=field, value=value), tempfile.TemporaryDirectory() as tmp:
                        path = Path(tmp); archives(path); api = WrongIdentity()
                        with self.assertRaises(RuntimeError):
                            release.publish(api, path, SHA, '12', '34')
                        self.assertEqual(api.ref, OLD)
                        self.assertFalse(any(e.startswith(('PATCH:', 'DELETE:')) for e in api.events))
                        self.assertEqual(api.events.count('upload'), 1)

    def test_unknown_upload_outcome_is_not_retransmitted(self):
        class UnknownUpload(FakeGitHub):
            def upload(self, path, release_info):
                super().upload(path, release_info)
                raise subprocess.TimeoutExpired('gh', 120)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp); archives(path); api = UnknownUpload()
            with self.assertRaises(subprocess.TimeoutExpired):
                release.publish(api, path, SHA, '12', '34')
            self.assertEqual(api.events.count('upload'), 1)
            self.assertEqual(api.ref, OLD)
            self.assertFalse(any(e.startswith(('PATCH:', 'DELETE:')) for e in api.events))

    def test_final_id_readback_failure_preserves_old_assets(self):
        class ChangedAsset(FakeGitHub):
            def asset(self, asset_id):
                result = super().asset(asset_id)
                return result | {'size': 0} if self.events.count('verify') == 5 else result
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp); archives(path); api = ChangedAsset()
            with self.assertRaisesRegex(RuntimeError, 'identity mismatch'):
                release.publish(api, path, SHA, '12', '34')
            self.assertEqual(api.events.count('upload'), 5)
            self.assertEqual(api.ref, OLD)
            self.assertIn(release.prefix(OLD) + 'windows-x86_64.zip', api.files)

    def test_official_upload_url_and_json_response_contract(self):
        import json
        api = release.GitHub()
        endpoint = f'https://uploads.github.com/repos/{release.REPOSITORY}/releases/1/assets'
        response = FakeGitHub().record('manual.txt')
        with patch.object(api, 'command', return_value='HTTP/2.0 201 Created\r\nContent-Type: application/json\r\n\r\n' + json.dumps(response)) as command:
            self.assertEqual(api.upload(Path('a b.zip'), {'id': 1, 'upload_url': endpoint + '{?name,label}'}), response)
            command.assert_called_once_with('gh', 'api', endpoint + '?name=a+b.zip', '--method', 'POST',
                                           '--include', '--input', 'a b.zip', '-H', 'Content-Type: application/octet-stream', timeout=120)
        for rid, url in ((True, endpoint + '{?name,label}'), (1, 'https://other.invalid/assets'),
                         (1, endpoint + '?name=x'), (1, endpoint.replace('/1/', '/2/') + '{?name,label}')):
            with self.subTest(rid=rid, url=url), patch.object(api, 'command') as command:
                with self.assertRaises(RuntimeError):
                    api.upload(Path('a.zip'), {'id': rid, 'upload_url': url})
                command.assert_not_called()

    def test_id_read_uses_fixed_endpoint_and_timeout(self):
        import json
        api = release.GitHub()
        with patch.object(api, 'command', return_value=json.dumps({'id': 3})) as command:
            self.assertEqual(api.asset(3), {'id': 3})
            command.assert_called_once_with('gh', 'api', f'repos/{release.REPOSITORY}/releases/assets/3', timeout=15)

    def test_upload_bad_or_unknown_response_is_never_retried(self):
        api = release.GitHub()
        info = {'id': 1, 'upload_url': f'https://uploads.github.com/repos/{release.REPOSITORY}/releases/1/assets{{?name,label}}'}
        for raw in ('', '{}', 'HTTP/2.0 200 OK\n\n{}', 'HTTP/2.0 204 No Content\n\n',
                    'HTTP/2.0 201 Created\n\nbroken'):
            with self.subTest(raw=raw), patch.object(api, 'command', return_value=raw) as command:
                with self.assertRaises((RuntimeError, ValueError)):
                    api.upload(Path('asset.zip'), info)
                self.assertEqual(command.call_count, 1)
        for error in (subprocess.TimeoutExpired('gh', 120), subprocess.CalledProcessError(1, 'gh')):
            with self.subTest(error=type(error).__name__), patch.object(api, 'command', side_effect=error) as command:
                with self.assertRaises(type(error)):
                    api.upload(Path('asset.zip'), info)
                self.assertEqual(command.call_count, 1)

    def test_existing_conflict_and_missing_list_conflict_stop_without_overwrite(self):
        for hidden in (False, True):
            class Conflict(FakeGitHub):
                def assets(self, rid):
                    return [a for a in super().assets(rid) if not hidden or not a['name'].startswith(release.prefix(SHA))]

                def upload(self, path, release_info):
                    if path.name in self.files:
                        self.events.append('upload')
                        raise RuntimeError('HTTP 422 already_exists')
                    return super().upload(path, release_info)
            with self.subTest(hidden=hidden), tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp); archives(path); expected = release.prepare_assets(path, SHA, '12', '34')
                name = next(iter(expected)); api = Conflict(); api.files[name] = b'conflict'; api.ids[name] = 3
                with self.assertRaises(RuntimeError): release.publish(api, path, SHA, '12', '34')
                self.assertEqual(api.files[name], b'conflict')
                self.assertEqual(api.events.count('upload'), int(hidden))
                self.assertEqual(api.ref, OLD)
                self.assertFalse(any(e.startswith(('PATCH:', 'DELETE:')) for e in api.events))

    def test_final_id_missing_or_content_changed_prevents_promotion(self):
        for failure in ('404', 'content'):
            class Changed(FakeGitHub):
                def asset(self, asset_id):
                    result = super().asset(asset_id)
                    if self.events.count('verify') == 5:
                        if failure == '404': raise RuntimeError('HTTP 404')
                        data = self.files[result['name']]
                        self.files[result['name']] = bytes([data[0] ^ 1]) + data[1:]
                    return result
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp); archives(path); api = Changed()
                with self.assertRaises(RuntimeError): release.publish(api, path, SHA, '12', '34')
                self.assertEqual(api.ref, OLD)
                self.assertFalse(any(e.startswith(('PATCH:', 'DELETE:')) for e in api.events))

    def test_real_download_hash_is_checked_by_id(self):
        api = release.GitHub(); record = FakeGitHub().record('manual.txt')
        def download(args, **kwargs):
            self.assertEqual(args, ['gh', 'api', f'repos/{release.REPOSITORY}/releases/assets/2',
                                    '-H', 'Accept: application/octet-stream'])
            self.assertTrue(kwargs['check'])
            kwargs['stdout'].write(b'keep')
        with patch.object(release.subprocess, 'run', side_effect=download):
            api.verify(record, release.hashlib.sha256(b'keep').hexdigest(), 4)
            with self.assertRaisesRegex(RuntimeError, 'checksum mismatch'):
                api.verify(record, release.hashlib.sha256(b'nope').hexdigest(), 4)
            with self.assertRaisesRegex(RuntimeError, 'size mismatch'):
                api.verify(record | {'size': 5}, release.hashlib.sha256(b'keep').hexdigest(), 5)

    def test_initial_list_duplicate_or_error_prevents_upload(self):
        for failure in ('duplicate', 'error'):
            class BrokenList(FakeGitHub):
                def assets(self, rid):
                    self.events.append('list')
                    if failure == 'error': raise RuntimeError('list unavailable')
                    result = super().assets(rid)
                    return result + [result[0]]
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp); archives(path); api = BrokenList()
                with self.assertRaises(RuntimeError): release.publish(api, path, SHA, '12', '34')
                self.assertEqual(api.events.count('list'), 1)
                self.assertNotIn('upload', api.events)
                self.assertNotIn('move_tag', api.events)
                self.assertFalse(any(e.startswith(('PATCH:', 'DELETE:')) for e in api.events))

    def test_asset_list_timeout_is_bounded_and_not_retried(self):
        api = release.GitHub()
        with patch.object(api, 'command', return_value='[[]]') as command:
            self.assertEqual(api.assets(1), [])
            command.assert_called_once_with('gh', 'api', '--paginate', '--slurp',
                f'repos/{release.REPOSITORY}/releases/1/assets?per_page=100', timeout=15)
        with patch.object(api, 'command', side_effect=subprocess.TimeoutExpired('gh', 15)) as command:
            with self.assertRaises(subprocess.TimeoutExpired): api.assets(1)
            self.assertEqual(command.call_count, 1)

    def test_different_expected_names_cannot_reuse_one_id(self):
        class ReusedID(FakeGitHub):
            def upload(self, path, release_info):
                super().upload(path, release_info)
                self.ids[path.name] = 3
                self.latest = path.name
                return self.record(path.name)

            def asset(self, asset_id):
                self.events.append('asset')
                return self.record(self.latest)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp); archives(path); api = ReusedID()
            with self.assertRaisesRegex(RuntimeError, 'reused the same ID'):
                release.publish(api, path, SHA, '12', '34')
            self.assertEqual(api.events.count('upload'), 2)
            self.assertEqual(api.ref, OLD)
            self.assertFalse(any(e.startswith(('PATCH:', 'DELETE:')) for e in api.events))

    def test_identity_diagnostics_are_bounded_and_allowlisted(self):
        import json
        api = FakeGitHub()
        api.upload_diagnostic = {'asset_id': 3, 'name': 'asset', 'http_status': 201, 'request_id': 'ABCD:1234:AB:CD:1234', 'Authorization': 'private-header'}
        expected = {'id': 3, 'name': 'asset', 'size': 4, 'state': 'uploaded',
                    'url': 'https://api.github.com/repos/a/b/releases/assets/3',
                    'browser_download_url': 'https://github.com/a/b/releases/download/beta/asset'}
        value = {'id': 3, 'name': 'x' * 2000, 'state': 'starter', 'size': 0,
                 'url': 'https://api.github.com/repos/a/b/releases/assets/3?secret=query#private-fragment',
                 'browser_download_url': 'https://user:password@other.invalid/path?secret=value',
                 'token': 'private-token', 'uploader': {'secret': 'private-uploader'}}
        error = release.asset_failure(api, value, expected, 'upload_response', 'selected_asset', 'first', 1)
        text = str(error)
        evidence = json.loads(text.split(': ', 1)[1])
        self.assertEqual(evidence['origin'], 'upload_response')
        self.assertEqual(evidence['upload'], {'http_status': 201, 'request_id': 'ABCD:1234:AB:CD:1234'})
        existing = str(release.asset_failure(api, value, expected, 'existing_list', 'selected_asset', 'first', 1))
        self.assertNotIn('http_status', existing)
        other_name = str(release.asset_failure(api, value, expected | {'name': 'other'}, 'upload_response', 'selected_asset', 'first', 1))
        self.assertNotIn('http_status', other_name)
        self.assertNotIn('value', evidence['actual']['name'])
        self.assertIn('name', evidence['mismatch_fields'])
        self.assertLess(len(text), 2500)
        for secret in ('private-header', 'private-token', 'private-uploader', 'password', 'secret=', 'private-fragment'):
            self.assertNotIn(secret, text)
        api.upload_diagnostic['request_id'] = 'private-request-secret'
        unsafe = value | {'name': 'private-name-secret', 'state': 'private-state-secret',
                          'url': 'https://api.github.com/private-path-secret', 'size': 10 ** 1000}
        text = str(release.asset_failure(api, unsafe, expected, 'upload_response', 'selected_asset', 'first', 1))
        for secret in ('private-request-secret', 'private-name-secret', 'private-state-secret', 'private-path-secret', str(10 ** 1000)):
            self.assertNotIn(secret, text)
        self.assertLess(len(text), 2500)
        giant = 10 ** 1000
        with self.assertRaises(RuntimeError) as raised:
            release.verify_asset(api, value | {'id': giant}, 'asset', 'unused', 4, 'upload_response', 'first', 1)
        text = str(raised.exception)
        self.assertNotIn(str(giant), text)
        self.assertLess(len(text), 2500)
        self.assertEqual(json.loads(text.split(': ', 1)[1])['expected']['id'], '<out of range>')

    def test_identity_failure_records_source_without_promotion(self):
        import json
        for origin in ('existing_list', 'upload_response'):
            class WrongState(FakeGitHub):
                def upload(self, path, info):
                    return super().upload(path, info) | {'state': 'starter'}

                def assets(self, rid):
                    return [a | {'state': 'starter'} if a['name'].startswith(release.prefix(SHA)) else a
                            for a in super().assets(rid)]
            with self.subTest(origin=origin), tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp); archives(path); expected = release.prepare_assets(path, SHA, '12', '34')
                api = WrongState()
                if origin == 'existing_list':
                    name = next(iter(expected)); api.ids[name] = 3; api.files[name] = (path / name).read_bytes()
                with self.assertRaises(RuntimeError) as raised:
                    release.publish(api, path, SHA, '12', '34')
                evidence = json.loads(str(raised.exception).split(': ', 1)[1])
                self.assertEqual(evidence['origin'], origin)
                self.assertEqual(evidence['stage'], 'selected_asset')
                self.assertEqual(api.events.count('upload'), int(origin == 'upload_response'))
                self.assertNotIn('move_tag', api.events)
                self.assertFalse(any(e.startswith(('PATCH:', 'DELETE:')) for e in api.events))

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

    def test_missing_published_release_never_creates_or_uploads(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp); archives(path); api = FakeGitHub(existing=False, fail='upload')
            with self.assertRaises(RuntimeError): release.publish(api, path, SHA, '12', '34')
            self.assertIsNone(api.current)
            self.assertEqual(api.events, ['check'])

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
