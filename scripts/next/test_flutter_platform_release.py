import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from contextlib import redirect_stdout
from unittest.mock import patch
import zipfile

import flutter_platform_release as publisher


WINDOWS, LINUX, MACOS = publisher.PRODUCERS
SOURCE = 'a' * 40
UPSTREAM = 'b' * 40


class Repository:
    def __init__(self):
        self.producers = {}
        self.releases = {}
        self.tags = {}
        self.files = {}
        self.downloads = []
        self.fail_upload = None

    def build(self, platform, run_id, sha=SOURCE):
        run = {
            'id': run_id, 'head_sha': sha, 'head_branch': publisher.BRANCH,
            'path': publisher.PRODUCERS[platform], 'event': 'push',
            'repository': {'full_name': publisher.REPOSITORY},
            'head_repository': {'full_name': publisher.REPOSITORY},
        }
        jobs = ['Prepare', f'Desktop ({platform})', f'Release requested ({platform})']
        if platform == WINDOWS:
            jobs += ['MSI lifecycle (default)', 'MSI lifecycle (custom)', 'Verify Windows release']
        jobs = [dict(name=name, head_sha=sha, status='completed', conclusion='success') for name in jobs]
        self.producers[run_id] = (run, jobs, platform)
        return run, jobs

    def runs(self):
        return [run for run, _, _ in self.producers.values()]

    def jobs(self, run_id):
        return self.producers[run_id][1]

    def maintained_source(self, sha):
        return any(run['head_sha'] == sha for run in self.runs())

    def tag_source(self, tag):
        return self.tags.get(tag, '')

    def release(self, sha):
        record = self.releases.get(sha)
        return record.copy() if record else None

    def create_release(self, sha):
        tag = f'flutter-beta-{sha[:12]}'
        release = dict(id=len(self.releases) + 1, target_commitish=sha, tag_name=tag,
                       draft=True, html_url=f'https://example.test/releases/{tag}')
        self.releases[sha] = release
        return release.copy()

    def assets(self, rid):
        return [asset for asset, release, _ in self.files.values() if release == rid]

    def artifact(self, run_id, platform, destination):
        self.downloads.append(run_id)
        sha = self.producers[run_id][0]['head_sha']
        stem, packages, _ = publisher.asset_names(sha, platform)
        version = f'xt-beta-{sha[:7]}-T1794491866'
        receipt = dict(schema_version=1, product='XenTerm Next Flutter', frontend='flutter',
                       channel='flutter-beta', repository=publisher.REPOSITORY, source_sha=sha,
                       upstream_snapshot=UPSTREAM, build_run=str(run_id), platform=platform,
                       default_profile='config-flutter-next', runtime_version=version)
        if platform == WINDOWS:
            receipt['msi_product_version'] = '106.245.50650'
        records = []
        with zipfile.ZipFile(destination, 'w') as artifact:
            for name in sorted(packages):
                content = f'{name} from {run_id}'.encode()
                artifact.writestr(name, content)
                records.append(dict(name=name, size=len(content), sha256=hashlib.sha256(content).hexdigest()))
            receipt['files'] = records
            artifact.writestr(stem + '-BUILD.json', json.dumps(receipt))
            artifact.writestr(stem + '.sha256',
                              ''.join(f'{r["sha256"]}  {r["name"]}\n' for r in records))

    def upload(self, path, rid):
        if self.fail_upload and path.name.endswith(self.fail_upload):
            raise RuntimeError('Upload failed')
        aid = len(self.files) + 1
        asset = dict(id=aid, name=path.name)
        self.files[aid] = (asset, rid, path.read_bytes())
        return asset

    def download(self, endpoint, path):
        path.write_bytes(self.files[int(endpoint.rsplit('/', 1)[1])][2])

    def verify_digest(self, asset, checksum, size):
        content = self.files[asset['id']][2]
        if hashlib.sha256(content).hexdigest() != checksum or len(content) != size:
            raise ValueError('Invalid release asset')

    def verify(self, asset, path, checksum):
        self.verify_digest(asset, checksum, path.stat().st_size)

    def api(self, endpoint, method='GET', body=None):
        identity = int(endpoint.rsplit('/', 1)[1])
        if endpoint.startswith('actions/runs/'):
            return self.producers[identity][0]
        release = next(r for r in self.releases.values() if r['id'] == identity)
        if method == 'PATCH':
            release.update(body)
            self.tags[release['tag_name']] = release['target_commitish']
        return release.copy()


class PlatformReleaseTest(unittest.TestCase):
    def test_public_index_requires_the_release_and_each_download_link(self):
        tag = f'flutter-beta-{SOURCE[:12]}'
        _, packages, _ = publisher.asset_names(SOURCE, WINDOWS)
        release_link = f'<a href="/{publisher.REPOSITORY}/releases/tag/{tag}">Flutter beta</a>'
        package_links = [f'<a href="{publisher.RELEASES_URL}/download/{tag}/{name}">Download</a>'
                         for name in packages]
        result = dict(source_sha=SOURCE, platforms=[WINDOWS])
        with patch.object(publisher, 'urlopen') as open_page, patch.object(publisher.time, 'sleep'):
            response = open_page.return_value.__enter__.return_value
            response.read.return_value = (release_link + package_links[0]).encode()
            with redirect_stdout(io.StringIO()), self.assertRaisesRegex(RuntimeError, 'package links'):
                publisher.verify_release_index(result)
            response.read.return_value = (release_link + ''.join(package_links)).encode()
            with redirect_stdout(io.StringIO()):
                publisher.verify_release_index(result)
            self.assertEqual(open_page.call_args.args[0].full_url, publisher.RELEASES_URL)

    def test_actions_download_uses_json_negotiation_and_checks_the_zip_digest(self):
        api = publisher.GitHub()
        archive = b'synthetic artifact bytes'
        item = dict(id=58, name=f'flutter-package-{LINUX}', expired=False,
                    digest='sha256:' + hashlib.sha256(archive).hexdigest())

        def download(command, *, stdout, **kwargs):
            self.assertIn('Accept: application/vnd.github+json', command)
            self.assertTrue(command[2].endswith('/actions/artifacts/58/zip'))
            stdout.write(archive)

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'artifact.zip'
            with patch.object(api, 'api', return_value={'artifacts': [item]}), \
                    patch.object(publisher.subprocess, 'run', side_effect=download):
                api.artifact(50, LINUX, path)
            self.assertEqual(path.read_bytes(), archive)

    def test_first_platform_releases_and_later_successes_append(self):
        repo = Repository()
        repo.build(WINDOWS, 10)
        publisher.reconcile(repo)
        release = repo.release(SOURCE)
        self.assertFalse(release['draft'])
        self.assertIn('**Windows x64** · [MSI 安装包]', release['body'].split('\n')[2])
        self.assertEqual(len(repo.assets(release['id'])), 4)
        windows = repo.assets(release['id'])
        # The original Windows Actions artifact may expire after publishing.
        repo.producers.clear()
        repo.build(WINDOWS, 11)
        repo.build(LINUX, 12)
        repo.build(MACOS, 13)
        publisher.reconcile(repo)
        updated = repo.release(SOURCE)
        self.assertEqual(release['id'], updated['id'])
        self.assertEqual(len(repo.assets(updated['id'])), 10)
        self.assertEqual(repo.assets(updated['id'])[:4], windows)
        self.assertEqual(repo.downloads, [10, 12, 13])
        for run_id in (10, 12, 13):
            self.assertIn(f'/actions/runs/{run_id}', updated['body'])

    def test_failed_platform_upload_preserves_other_platform_and_resumes(self):
        repo = Repository()
        repo.build(LINUX, 20)
        repo.build(WINDOWS, 21)
        repo.fail_upload = '.tar.gz'
        with redirect_stdout(io.StringIO()), self.assertRaisesRegex(RuntimeError, '1 platform'):
            publisher.reconcile(repo)
        release = repo.release(SOURCE)
        self.assertFalse(release['draft'])
        self.assertIn('**windows-x86_64**', release['body'])
        self.assertNotIn('**linux-x86_64**', release['body'])
        repo.build(LINUX, 22)
        repo.fail_upload = None
        publisher.reconcile(repo)
        self.assertEqual(repo.downloads, [20, 21, 20])
        self.assertIn('/actions/runs/20', repo.release(SOURCE)['body'])

    def test_other_source_has_its_own_release(self):
        repo = Repository()
        other = 'c' * 40
        repo.build(WINDOWS, 30)
        repo.build(LINUX, 31, other)
        publisher.reconcile(repo)
        for sha, count in ((SOURCE, 4), (other, 3)):
            release = repo.release(sha)
            self.assertEqual(repo.tag_source(release['tag_name']), sha)
            assets = repo.assets(release['id'])
            self.assertEqual(len(assets), count)
            self.assertTrue(all(sha in asset['name'] for asset in assets))

    def test_publication_requires_own_branch_intent_and_both_msi_jobs(self):
        repo = Repository()
        run, jobs = repo.build(WINDOWS, 40)
        self.assertTrue(publisher.producer_ready(run, jobs, WINDOWS))
        for key, value in (('event', 'pull_request'), ('head_repository', {'full_name': 'fork/repo'}),
                           ('head_branch', 'other')):
            self.assertFalse(publisher.producer_ready(dict(run, **{key: value}), jobs, WINDOWS))
        for name in ('MSI lifecycle (default)', 'MSI lifecycle (custom)', 'Verify Windows release',
                     'Release requested (windows-x86_64)'):
            with self.subTest(job=name):
                skipped = [dict(job, conclusion='skipped') if job['name'] == name else job for job in jobs]
                self.assertFalse(publisher.producer_ready(run, skipped, WINDOWS))

    def test_rejects_an_artifact_with_an_unexpected_archive_path(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with zipfile.ZipFile(root / 'artifact.zip', 'w') as artifact:
                artifact.writestr('../unexpected', 'bad')
            with self.assertRaisesRegex(ValueError, 'unexpected'):
                publisher.extract_artifact(root / 'artifact.zip', root / 'payload', ['expected'])
            self.assertFalse((root / 'unexpected').exists())


if __name__ == '__main__':
    unittest.main()
