"""Recoverable generation rotation with an in-memory GitHub, no network writes."""

import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import flutter_release as release
import flutter_release_archive as archive


CURRENT, PREVIOUS, OLD, OTHER = ('c' * 40, 'b' * 40, 'a' * 40, 'd' * 40)
UPSTREAM = 'e' * 40


def generation(directory, sha, run):
    directory.mkdir()
    version = f'xt-beta-{sha[:7]}-T{1794480000 + int(run)}'
    for platform in release.PLATFORMS:
        stem = f'XenTerm-Next-Flutter-{sha}-{platform}'
        names = [stem + ('.tar.gz' if platform.startswith('linux') else '.zip')]
        if platform == 'windows-x86_64':
            names.append(stem + '.msi')
        files = []
        for name in names:
            path = directory / name
            path.write_bytes(('synthetic package, never executed: ' + name).encode())
            files.append({'name': name, 'size': path.stat().st_size, 'sha256': release.digest(path)})
        receipt = {
            'schema_version': 1, 'product': 'XenTerm Next Flutter', 'frontend': 'flutter',
            'channel': release.TAG, 'repository': release.REPOSITORY, 'source_sha': sha,
            'upstream_snapshot': UPSTREAM, 'runtime_version': version, 'build_run': str(run),
            'platform': platform, 'default_profile': 'config-flutter-next', 'files': files,
        }
        if platform == 'windows-x86_64':
            receipt['msi_product_version'] = release.runtime_details(version, sha)
        (directory / (stem + '-BUILD.json')).write_text(json.dumps(receipt), encoding='utf-8')
        (directory / (stem + '.sha256')).write_text(''.join(
            f'{item["sha256"]}  {item["name"]}\n' for item in sorted(files, key=lambda item: item['name'])),
            encoding='utf-8')
    metadata, hashes = release.verify_packages(directory, sha, str(run))
    return metadata, {name: (directory / name).read_bytes() for name in hashes}


class FakeGitHub:
    def __init__(self):
        self.releases = {1: {'id': 1, 'tag_name': release.TAG, 'target_commitish': CURRENT,
                             'draft': False, 'body': 'current published build', 'name': 'Flutter beta'}}
        self.storage = {1: {}}
        self.contents = {}
        self.tags = {release.TAG: CURRENT}
        self.runs = {}
        self.jobs = {}
        self.events = []
        self.next_id = 100
        self.upload_attempts = 0
        self.delete_attempts = 0
        self.fail_upload_at = None
        self.fail_delete_at = None
        self.fail_publish = False
        self.fail_archive_verify = False
        self.stale_after_publish = False
        self.after_publish = None
        self.on_check = None
        self.after_source_preflight = None

    def add_asset(self, rid, name, data):
        self.next_id += 1
        asset = {'id': self.next_id, 'name': name, 'size': len(data), 'state': 'uploaded',
                 'digest': 'sha256:' + hashlib.sha256(data).hexdigest(), 'updated_at': f'seed-{self.next_id}'}
        self.storage[rid][asset['id']] = asset
        self.contents[asset['id']] = data
        return copy.deepcopy(asset)

    def add_generation(self, sha, run, files):
        for name, contents in files.items():
            self.add_asset(1, name, contents)
        self.runs[str(run)] = {
            'id': int(run), 'repository': {'full_name': release.REPOSITORY},
            'head_repository': {'full_name': release.REPOSITORY}, 'path': release.WORKFLOW,
            'head_branch': release.BRANCH, 'head_sha': sha, 'event': 'push',
            'status': 'completed', 'conclusion': 'success',
        }
        self.jobs[str(run)] = [
            {'id': index + 1, 'name': name, 'head_sha': sha, 'run_attempt': 1,
             'started_at': '2026-11-12T00:00:00Z', 'status': 'completed', 'conclusion': 'success'}
            for index, name in enumerate(release.REQUIRED_JOBS)]

    def lookup(self, aid):
        for rid, assets in self.storage.items():
            if aid in assets:
                return rid, assets[aid]
        raise ValueError('Asset no longer exists')

    def check(self, sha, run):
        self.events.append(('check', sha, str(run)))
        if sha != CURRENT or str(run) != '900':
            raise AssertionError('Historical runs must not use current-branch freshness checks')
        if self.stale_after_publish and any(not item['draft'] for rid, item in self.releases.items() if rid != 1):
            raise ValueError('Current source became stale')
        if self.on_check:
            self.on_check(self)

    def tag(self, name=None):
        return self.tags.get(name or release.TAG, '')

    def assets(self, rid):
        self.events.append(('assets', rid))
        return copy.deepcopy(list(self.storage[rid].values()))

    def pages(self, endpoint):
        self.events.append(('pages', endpoint))
        if endpoint.startswith('releases?'):
            # Multiple pages exercise draft discovery beyond the first page.
            return [[copy.deepcopy(self.releases[1])],
                    [copy.deepcopy(item) for rid, item in self.releases.items() if rid != 1]]
        if endpoint.startswith('actions/runs/') and '/jobs?' in endpoint:
            run = endpoint.split('/')[2]
            jobs = self.jobs[run]
            return [{'jobs': copy.deepcopy(jobs[:2])}, {'jobs': copy.deepcopy(jobs[2:])}]
        raise AssertionError(endpoint)

    def api(self, endpoint, method='GET', body=None):
        if endpoint == 'releases' and method == 'POST':
            rid = max(self.releases) + 1
            self.releases[rid] = dict(body, id=rid)
            self.storage[rid] = {}
            self.events.append(('create-release', rid, copy.deepcopy(body)))
            return copy.deepcopy(self.releases[rid])
        if endpoint.startswith('actions/runs/') and method == 'GET':
            return copy.deepcopy(self.runs[endpoint.split('/')[2]])
        if endpoint.startswith('releases/assets/'):
            aid = int(endpoint.split('/')[-1])
            rid, asset = self.lookup(aid)
            if method == 'GET':
                self.events.append(('get-asset', rid, aid, asset['name']))
                return copy.deepcopy(asset)
            if method == 'DELETE':
                self.delete_attempts += 1
                if self.fail_delete_at == self.delete_attempts:
                    raise ValueError('Simulated interrupted cleanup')
                self.events.append(('delete', rid, aid, asset['name']))
                del self.storage[rid][aid]
                return None
        if endpoint.startswith('releases/'):
            rid = int(endpoint.split('/')[1])
            if method == 'PATCH':
                if self.fail_publish:
                    raise ValueError('Simulated uncertain publication')
                self.releases[rid].update(body)
                self.tags[self.releases[rid]['tag_name']] = self.releases[rid]['target_commitish']
                self.events.append(('publish', rid))
                if self.after_publish:
                    self.after_publish(self, rid)
            elif method == 'GET':
                self.events.append(('get-release', rid))
            else:
                raise AssertionError((endpoint, method))
            return copy.deepcopy(self.releases[rid])
        raise AssertionError((endpoint, method))

    def download(self, asset, destination):
        rid, actual = self.lookup(asset['id'])
        archive.check_asset(actual, asset)
        with destination.open('xb') as stream:
            stream.write(self.contents[asset['id']])
        self.events.append(('download', rid, asset['name']))
        self.verify(asset, destination, release.digest(destination))

    def upload(self, path, rid):
        self.upload_attempts += 1
        if self.fail_upload_at == self.upload_attempts:
            raise ValueError('Simulated interrupted upload')
        asset = self.add_asset(rid, path.name, path.read_bytes())
        self.events.append(('upload', rid, path.name))
        return asset

    def verify(self, asset, path, digest):
        rid, actual = self.lookup(asset['id'])
        if (actual['name'] != path.name or actual['size'] != path.stat().st_size
                or actual['state'] != 'uploaded'
                or hashlib.sha256(self.contents[asset['id']]).hexdigest() != digest
                or release.digest(path) != digest or rid != 1 and self.fail_archive_verify):
            raise ValueError('Remote asset does not match verified historical bytes')
        self.events.append(('verify', rid, path.name))
        if rid == 1 and self.after_source_preflight and any(event[0] == 'publish' for event in self.events):
            self.after_source_preflight(self)


class ArchiveTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='archive-release-test-')
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.api = FakeGitHub()
        self.generations = {}
        for sha, run in ((CURRENT, '900'), (PREVIOUS, '800'), (OLD, '700')):
            metadata, files = generation(self.directory / sha, sha, run)
            self.generations[sha] = (metadata, files)
            self.api.add_generation(sha, run, files)
        self.rolling = copy.deepcopy(self.api.releases[1])

    def rotate(self):
        return archive.archive_old_generations(self.api, self.rolling, CURRENT, '900', PREVIOUS)

    def names(self, rid):
        return {item['name'] for item in self.api.storage[rid].values()}

    def assert_no_deletes(self):
        self.assertFalse(any(event[0] == 'delete' for event in self.api.events))

    def assert_protected(self):
        for sha in (CURRENT, PREVIOUS):
            self.assertTrue(set(self.generations[sha][1]).issubset(self.names(1)))

    def test_complete_archive_is_public_and_fully_verified_before_first_delete(self):
        metadata, files = self.generations[OLD]
        manual = self.api.add_asset(1, 'manual-kept.zip', b'manual content')
        result = self.rotate()
        tag = f'flutter-build-{OLD[:12]}-700'
        self.assertEqual(result, [{'source_sha': OLD, 'build_run': '700', 'tag_name': tag,
                                   'release_id': 2, 'deleted_assets': 12}])
        self.assertEqual(self.names(2), set(files))
        self.assertFalse(set(files) & self.names(1))
        self.assertIn(manual['id'], self.api.storage[1])
        self.assert_protected()
        published = self.api.releases[2]
        self.assertFalse(published['draft'])
        self.assertEqual(published['name'], f'XenTerm Next Flutter build ({OLD[:12]})')
        self.assertEqual(self.api.tag(tag), OLD)
        self.assertIn(json.dumps(metadata, indent=2, ensure_ascii=True), published['body'])
        for name in files:
            self.assertIn(f'/releases/download/{tag}/{name}', published['body'])
        events = self.api.events
        first_delete = next(i for i, event in enumerate(events) if event[0] == 'delete')
        publication = next(i for i, event in enumerate(events) if event[0] == 'publish')
        before = events[:first_delete]
        after_publish = events[publication:first_delete]
        self.assertEqual({event[2] for event in after_publish if event[:2] == ('verify', 2)}, set(files))
        self.assertGreaterEqual(sum(event[:2] == ('assets', 2) for event in after_publish), 2)
        self.assertTrue(any(event[0] == 'check' for event in after_publish))
        self.assertTrue(any(event[:2] == ('get-release', 2) for event in after_publish))
        for index, event in enumerate(events):
            if event[0] == 'delete':
                self.assertEqual(events[index - 1][:3], ('get-asset', 1, event[2]))
                self.assertIn(('verify', 1, event[3]), before)
        self.assertIn(('pages', 'actions/runs/700/jobs?filter=all&per_page=100'), events)

    def test_failed_upload_is_recoverable_and_retry_reuses_verified_assets(self):
        self.api.fail_upload_at = 5
        with self.assertRaisesRegex(ValueError, 'interrupted upload'):
            self.rotate()
        self.assert_no_deletes()
        self.assertTrue(self.api.releases[2]['draft'])
        self.assertEqual(len(self.api.storage[2]), 4)
        self.assertTrue(set(self.generations[OLD][1]).issubset(self.names(1)))
        old_ids = set(self.api.storage[2])
        self.api.add_asset(2, 'archive-manual-kept.zip', b'manual archive asset')
        self.api.fail_upload_at = None
        self.rotate()
        self.assertTrue(old_ids.issubset(self.api.storage[2]))
        self.assertEqual(sum(event[0] == 'upload' for event in self.api.events), 12)
        self.assertEqual(sum(event[0] == 'create-release' for event in self.api.events), 1)
        self.assertIn('archive-manual-kept.zip', self.names(2))
        self.assert_protected()

    def test_existing_published_archive_is_reverified_without_duplicate_upload(self):
        self.api.stale_after_publish = True
        with self.assertRaisesRegex(ValueError, 'stale'):
            self.rotate()
        self.assert_no_deletes()
        self.assertFalse(self.api.releases[2]['draft'])
        uploads = self.api.upload_attempts
        self.api.events.clear()
        self.api.stale_after_publish = False
        self.rotate()
        self.assertEqual(self.api.upload_attempts, uploads)
        self.assertFalse(any(event[0] in ('create-release', 'publish') for event in self.api.events))
        first_delete = next(i for i, event in enumerate(self.api.events) if event[0] == 'delete')
        self.assertEqual({event[2] for event in self.api.events[:first_delete] if event[:2] == ('verify', 2)},
                         set(self.generations[OLD][1]))

    def test_partial_unrecognized_and_failed_generations_are_ignored(self):
        files = self.generations[OLD][1]
        missing = next(name for name in files if name.endswith('.msi'))
        aid = next(aid for aid, item in self.api.storage[1].items() if item['name'] == missing)
        del self.api.storage[1][aid]
        self.api.add_asset(1, f'XenTerm-Next-Flutter-{OTHER[:12]}-BUILD.json', b'unrecognized short SHA')
        self.api.add_asset(1, f'XenTerm-Next-Flutter-{OLD}-notes.txt', b'manual same prefix')
        before = copy.deepcopy(self.api.storage[1])
        self.assertEqual(self.rotate(), [])
        self.assertEqual(self.api.storage[1], before)
        self.assertEqual(len(self.api.releases), 1)
        self.api.add_asset(1, missing, files[missing])
        for status, conclusion in (('in_progress', None), ('completed', 'failure'), ('completed', 'cancelled')):
            with self.subTest(status=status, conclusion=conclusion):
                self.api.runs['700'].update(status=status, conclusion=conclusion)
                self.assertEqual(self.rotate(), [])
                self.assertEqual(len(self.api.releases), 1)
        self.assert_no_deletes()

    def test_missing_successful_native_job_prevents_archival(self):
        for job in self.api.jobs['700']:
            original = job['conclusion']
            job['conclusion'] = 'failure'
            with self.subTest(job=job['name']):
                self.assertEqual(self.rotate(), [])
                self.assert_no_deletes()
                self.assertEqual(len(self.api.releases), 1)
            job['conclusion'] = original

    def test_corrupt_generation_cannot_create_an_archive(self):
        asset = next(item for item in self.api.storage[1].values()
                     if OLD in item['name'] and item['name'].endswith('.msi'))
        self.api.contents[asset['id']] = b'x' * asset['size']
        self.assertEqual(self.rotate(), [])
        self.assert_no_deletes()
        self.assertEqual(len(self.api.releases), 1)

    def test_conflicting_archive_tag_is_never_moved(self):
        tag = f'flutter-build-{OLD[:12]}-700'
        self.api.tags[tag] = OTHER
        with self.assertRaisesRegex(ValueError, 'another source'):
            self.rotate()
        self.assertEqual(self.api.tag(tag), OTHER)
        self.assertEqual(len(self.api.releases), 1)
        self.assert_no_deletes()

    def test_failed_archive_verification_and_uncertain_publication_never_delete(self):
        self.api.fail_archive_verify = True
        with self.assertRaisesRegex(ValueError, 'Remote asset'):
            self.rotate()
        self.assert_no_deletes()
        self.api.fail_archive_verify = False
        self.api.fail_publish = True
        with self.assertRaisesRegex(ValueError, 'uncertain publication'):
            self.rotate()
        self.assert_no_deletes()
        self.assertTrue(self.api.releases[2]['draft'])

    def test_archive_is_reread_after_publish_and_missing_asset_blocks_deletion(self):
        def remove_asset(api, rid):
            del api.storage[rid][next(iter(api.storage[rid]))]
        self.api.after_publish = remove_asset
        with self.assertRaisesRegex(ValueError, 'Incomplete'):
            self.rotate()
        self.assert_no_deletes()
        self.assertTrue(set(self.generations[OLD][1]).issubset(self.names(1)))

    def test_source_identity_changed_after_preflight_is_not_deleted(self):
        def mutate_on_last_check(api):
            public = any(event[0] == 'publish' for event in api.events)
            verified = sum(event[:2] == ('verify', 2) for event in api.events)
            if public and verified >= 36:
                source = next(item for item in api.storage[1].values()
                              if OLD in item['name'] and item['name'].endswith('linux-x86_64-BUILD.json'))
                source['updated_at'] = 'changed after preflight'
        self.api.on_check = mutate_on_last_check
        with self.assertRaisesRegex(ValueError, 'changed during archival'):
            self.rotate()
        self.assert_no_deletes()

    def test_same_size_source_corruption_before_preflight_keeps_all_rolling_assets(self):
        def corrupt_source(api, rid):
            source = next(item for item in api.storage[1].values()
                          if OLD in item['name'] and item['name'].endswith('windows-x86_64.zip'))
            api.contents[source['id']] = b'!' * source['size']
        self.api.after_publish = corrupt_source
        with self.assertRaisesRegex(ValueError, 'Remote asset'):
            self.rotate()
        self.assert_no_deletes()

    def test_cleanup_failure_leaves_complete_durable_archive_and_last_two_generations(self):
        self.api.fail_delete_at = 3
        with self.assertRaisesRegex(ValueError, 'interrupted cleanup'):
            self.rotate()
        self.assertEqual(sum(event[0] == 'delete' for event in self.api.events), 2)
        self.assert_protected()
        self.assertEqual(self.names(2), set(self.generations[OLD][1]))
        self.assertFalse(self.api.releases[2]['draft'])
        self.assertEqual(self.api.tag(f'flutter-build-{OLD[:12]}-700'), OLD)
        restored = self.directory / 'restored'
        restored.mkdir()
        for item in self.api.storage[2].values():
            (restored / item['name']).write_bytes(self.api.contents[item['id']])
        self.assertEqual(release.verify_packages(restored, OLD, '700')[0], self.generations[OLD][0])
        # Partial rolling generations stay untouched on a later cleanup pass.
        before = copy.deepcopy(self.api.storage[1])
        self.assertEqual(self.rotate(), [])
        self.assertEqual(self.api.storage[1], before)

    def test_multiple_complete_old_generations_rotate_but_current_and_previous_stay(self):
        metadata, files = generation(self.directory / OTHER, OTHER, '600')
        self.api.add_generation(OTHER, '600', files)
        result = self.rotate()
        self.assertEqual({item['source_sha'] for item in result}, {OLD, OTHER})
        self.assert_protected()
        self.assertEqual(len(self.names(1)), 24)
        self.assertEqual(len(self.api.releases), 3)


if __name__ == '__main__':
    unittest.main()
