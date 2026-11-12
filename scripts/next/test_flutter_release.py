"""Release integrity and failure recovery, without credentials or remote writes."""
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import flutter_release as release

SHA = '1234567' + 'a' * 33
UPSTREAM = 'b' * 40
VERSION = 'xt-beta-1234567-T1794480000'


def fixture(directory, run='42'):
    for platform in release.PLATFORMS:
        stem = f'XenTerm-Next-Flutter-{SHA}-{platform}'
        names = [stem + ('.tar.gz' if platform.startswith('linux') else '.zip')]
        if platform == 'windows-x86_64':
            names.append(stem + '.msi')
        files = []
        for name in names:
            path = directory / name
            path.write_bytes(('distinct synthetic package: ' + name).encode())
            files.append({'name': name, 'size': path.stat().st_size,
                          'sha256': hashlib.sha256(path.read_bytes()).hexdigest()})
        receipt = {'schema_version': 1, 'product': 'XenTerm Next Flutter', 'frontend': 'flutter',
                   'channel': 'flutter-beta', 'repository': release.REPOSITORY, 'source_sha': SHA,
                   'upstream_snapshot': UPSTREAM, 'runtime_version': VERSION, 'build_run': run,
                   'platform': platform, 'default_profile': 'config-flutter-next', 'files': files}
        if platform == 'windows-x86_64':
            receipt['msi_product_version'] = '106.245.38784'
        (directory / (stem + '-BUILD.json')).write_text(json.dumps(receipt), encoding='utf-8')
        (directory / (stem + '.sha256')).write_text(''.join(
            f'{item["sha256"]}  {item["name"]}\n' for item in sorted(files, key=lambda f: f['name'])), encoding='utf-8')


def ci():
    run = {'id': 42, 'repository': {'full_name': release.REPOSITORY},
           'head_repository': {'full_name': release.REPOSITORY}, 'path': release.WORKFLOW,
           'head_branch': 'flutter-next', 'head_sha': SHA, 'event': 'push',
           'status': 'in_progress', 'conclusion': None}
    jobs = [{'name': name, 'head_sha': SHA, 'status': 'completed', 'conclusion': 'success'}
            for name in release.REQUIRED_JOBS]
    return run, jobs


class FakeGitHub:
    def __init__(self):
        self.writes = []
        self.uploaded = {}
        self.current_tag = 'c' * 40
        self.publication = {'id': 77, 'tag_name': 'flutter-beta', 'draft': False,
                            'body': 'previous working Flutter release'}
        self.checks = 0
        self.stale_at = None
        self.fail_download = False

    def check(self, sha, run_id):
        self.checks += 1
        if self.stale_at and self.checks >= self.stale_at:
            raise ValueError('stale current Flutter source')

    def release(self, sha):
        return self.publication.copy()

    def assets(self, rid):
        return list(self.uploaded.values())

    def upload(self, path, rid):
        asset = {'id': len(self.uploaded) + 100, 'name': path.name, 'checksum': release.digest(path),
                 'size': path.stat().st_size, 'data': path.read_bytes()}
        self.uploaded[path.name] = asset
        self.writes.append(('upload', path.name))
        return asset

    def verify(self, asset, path, checksum):
        if self.fail_download or asset['checksum'] != checksum:
            raise ValueError('remote download differs')

    def download(self, asset, destination):
        if self.fail_download or hashlib.sha256(asset['data']).hexdigest() != asset['checksum']:
            raise ValueError('remote download differs')
        with destination.open('xb') as stream:
            stream.write(asset['data'])

    def tag(self, name=None):
        return self.current_tag

    def promote_tag(self, sha, old):
        if old != self.current_tag:
            raise ValueError('tag changed')
        self.current_tag = sha
        self.writes.append(('tag', sha))

    def preserve_tag_object(self, expected):
        pass

    def api(self, endpoint, method='GET', body=None):
        if method == 'PATCH':
            self.publication.update(body)
            self.writes.append(('release', body))
        return self.publication.copy()


class ReleaseTests(unittest.TestCase):
    def test_lossless_ordered_installer_version_and_source_binding(self):
        epoch = 1794480000
        expected = f'{epoch >> 24}.{(epoch >> 16) & 255}.{epoch & 65535}'
        self.assertEqual(release.runtime_details(VERSION, SHA), expected)
        major, minor, build = map(int, expected.split('.'))
        self.assertEqual((major << 24) | (minor << 16) | build, epoch)
        for version in ('xt-dev-1234567-T1794480000', 'xt-beta-fffffff-T1794480000',
                        'xt-beta-1234567-T9999999999', '0.7.3'):
            with self.subTest(version=version), self.assertRaises(ValueError):
                release.runtime_details(version, SHA)

    def test_every_product_gate_and_source_identity_is_required(self):
        run, jobs = ci()
        release.source_gate(SHA, '42', run, SHA, jobs)
        changes = {'event': 'pull_request', 'head_sha': 'd' * 40, 'head_branch': 'main',
                   'path': '.github/workflows/other.yml', 'conclusion': 'cancelled',
                   'head_repository': {'full_name': 'other/fork'}, 'id': 43}
        for key, value in changes.items():
            wrong = {**run, key: value}
            with self.subTest(key=key), self.assertRaises(ValueError):
                release.source_gate(SHA, '42', wrong, SHA, jobs)
        with self.assertRaises(ValueError):
            release.source_gate(SHA, '42', run, 'd' * 40, jobs)
        for index in range(len(jobs)):
            for field, value in (('conclusion', 'failure'), ('status', 'in_progress'), ('head_sha', 'd' * 40)):
                changed = copy.deepcopy(jobs)
                changed[index][field] = value
                with self.subTest(index=index, field=field), self.assertRaises(ValueError):
                    release.source_gate(SHA, '42', run, SHA, changed)
            with self.assertRaises(ValueError):
                release.source_gate(SHA, '42', run, SHA, jobs[:index] + jobs[index + 1:])

    def test_complete_package_checksums_and_idempotent_sidecars(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            fixture(directory)
            metadata, hashes = release.verify_packages(directory, SHA, '42')
            self.assertEqual(metadata['runtime_version'], VERSION)
            self.assertEqual(len(hashes), 12)
            self.assertEqual({r['platform'] for r in metadata['platforms']}, set(release.PLATFORMS))
            self.assertEqual(hashes, release.verify_packages(directory, SHA, '42')[1])
            sums = (directory / f'XenTerm-Next-Flutter-{SHA}-SHA256SUMS').read_text()
            for name, checksum in hashes.items():
                self.assertEqual(checksum, release.digest(directory / name))
                if not name.endswith('-SHA256SUMS'):
                    self.assertIn(f'{checksum}  {name}\n', sums)

    def test_failed_job_retry_uses_each_latest_execution_without_losing_prior_successes(self):
        run, jobs = ci()
        name = release.REQUIRED_JOBS[1]
        old = {**jobs[1], 'id': 100, 'run_attempt': 1, 'conclusion': 'failure'}
        new = {**jobs[1], 'id': 200, 'run_attempt': 2}
        rest = jobs[:1] + jobs[2:]
        release.source_gate(SHA, '42', run, SHA, [old, *rest, new])
        with self.assertRaisesRegex(ValueError, name.replace('(', r'\(').replace(')', r'\)')):
            release.source_gate(SHA, '42', run, SHA, [{**old, 'conclusion': 'success'}, *rest,
                                                    {**new, 'conclusion': 'failure'}])

    def test_missing_or_tampered_or_mixed_source_never_gets_promoted(self):
        faults = ('missing-msi', 'tampered-archive', 'wrong-run', 'wrong-source', 'wrong-version',
                  'wrong-upstream', 'wrong-msi-version', 'unexpected-file', 'wrong-checksum')
        for fault in faults:
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                directory = Path(temp)
                fixture(directory)
                stem = f'XenTerm-Next-Flutter-{SHA}-windows-x86_64'
                receipt_path = directory / (stem + '-BUILD.json')
                receipt = json.loads(receipt_path.read_text())
                if fault == 'missing-msi':
                    (directory / (stem + '.msi')).unlink()
                elif fault == 'tampered-archive':
                    (directory / (stem + '.zip')).write_bytes(b'changed')
                elif fault == 'unexpected-file':
                    (directory / 'old-package.zip').write_bytes(b'old source')
                elif fault == 'wrong-checksum':
                    (directory / (stem + '.sha256')).write_text('wrong')
                else:
                    field, value = {'wrong-run': ('build_run', '41'), 'wrong-source': ('source_sha', 'd' * 40),
                                    'wrong-version': ('runtime_version', 'xt-beta-1234567-T1794480001'),
                                    'wrong-upstream': ('upstream_snapshot', 'd' * 40),
                                    'wrong-msi-version': ('msi_product_version', '0.7.3')}[fault]
                    receipt[field] = value
                    receipt_path.write_text(json.dumps(receipt))
                api = FakeGitHub()
                with self.assertRaises((ValueError, FileNotFoundError)):
                    release.publish(api, directory, SHA, '42')
                self.assertEqual(api.writes, [])

    def test_failed_download_or_newer_source_preserves_previous_release(self):
        for fault in ('download', 'stale-before-upload', 'stale-during-upload', 'stale-after-tag'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                directory = Path(temp)
                fixture(directory)
                api = FakeGitHub()
                if fault == 'download':
                    api.fail_download = True
                else:
                    api.stale_at = {'stale-before-upload': 1, 'stale-during-upload': 3,
                                    'stale-after-tag': 15}[fault]
                before = api.publication.copy()
                with self.assertRaises(ValueError):
                    release.publish(api, directory, SHA, '42')
                self.assertEqual(api.publication, before)
                self.assertEqual(api.current_tag, 'c' * 40)
                if fault == 'stale-after-tag':
                    self.assertEqual([value for kind, value in api.writes if kind == 'tag'], [SHA, 'c' * 40])
                else:
                    self.assertFalse(any(kind in ('tag', 'release', 'delete') for kind, _ in api.writes))

    def test_success_and_retry_never_delete_or_duplicate_assets(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            fixture(directory)
            api = FakeGitHub()
            api.uploaded['manual-kept.zip'] = {'id': 9, 'name': 'manual-kept.zip', 'checksum': 'unused'}
            release.publish(api, directory, SHA, '42')
            self.assertEqual(api.current_tag, SHA)
            self.assertFalse(api.publication['draft'])
            self.assertIn('/releases/download/flutter-beta/', api.publication['body'])
            count = sum(kind == 'upload' for kind, _ in api.writes)
            self.assertEqual(count, 12)
            release.publish(api, directory, SHA, '42')
            self.assertEqual(sum(kind == 'upload' for kind, _ in api.writes), count)
            self.assertIn('manual-kept.zip', api.uploaded)

    def test_same_source_new_run_reuses_original_verified_generation(self):
        with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
            original, rebuilt = Path(first), Path(second)
            fixture(original, run='42')
            fixture(rebuilt, run='43')
            api = FakeGitHub()
            release.publish(api, original, SHA, '42')
            before, writes = api.publication.copy(), list(api.writes)
            result = release.publish(api, rebuilt, SHA, '43')
            self.assertEqual(result, before)
            self.assertEqual(api.publication, before)
            self.assertEqual(api.writes, writes)
            self.assertIn('/actions/runs/42', result['body'])
            self.assertEqual(json.loads(api.uploaded[f'XenTerm-Next-Flutter-{SHA}-BUILD.json']['data'])['build_run'], '42')

    def test_tag_update_uses_an_exact_flutter_lease(self):
        with patch.object(release.GitHub, 'command') as command:
            release.GitHub().promote_tag(SHA, 'c' * 40)
        args = command.call_args.args
        self.assertIn('--force-with-lease=refs/tags/flutter-beta:' + 'c' * 40, args)
        self.assertIn(SHA + ':refs/tags/flutter-beta', args)
        self.assertFalse(any('refs/heads/' in value for value in args))

    def test_previous_tag_object_is_fetched_before_any_promotion_can_need_rollback(self):
        with patch.object(release.GitHub, 'command') as command:
            release.GitHub().preserve_tag_object('c' * 40)
        self.assertEqual(command.call_args_list[0].args, ('git', 'fetch', '--no-tags', 'origin', 'c' * 40))
        self.assertEqual(command.call_args_list[1].args, ('git', 'cat-file', '-e', 'c' * 40))
        with patch.object(release.GitHub, 'command') as command:
            release.GitHub().preserve_tag_object('')
        command.assert_not_called()


if __name__ == '__main__':
    unittest.main()
