"""Reconcile independent platform producers into one append-only source release.

Each source gets its own tag. Platform receipts and packages are immutable:
later successes append files and update the release's available-platform list.
Every wakeup scans all ready runs, so coalesced dispatches cannot lose a platform.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
from urllib.parse import urlencode
from urllib.request import Request, urlopen
import zipfile

from flutter_release import BRANCH, REPOSITORY, digest as sha256, full_sha as source_sha, runtime_details

PRODUCERS = {
    'windows-x86_64': '.github/workflows/next-flutter-windows.yml',
    'linux-x86_64': '.github/workflows/next-flutter-linux.yml',
    'macos-aarch64': '.github/workflows/next-flutter-macos.yml',
}
RELEASES_URL = f'https://github.com/{REPOSITORY}/releases'
PLATFORM_NAMES = {'windows-x86_64': 'Windows x64', 'linux-x86_64': 'Linux x64',
                  'macos-aarch64': 'macOS Apple Silicon'}
PACKAGE_LABELS = {'windows-x86_64': ('MSI 安装包', '便携 ZIP'),
                  'linux-x86_64': ('tar.gz',), 'macos-aarch64': ('ZIP',)}


def asset_names(sha, platform):
    source_sha(sha)
    if platform not in PRODUCERS:
        raise ValueError('Unsupported release platform')
    stem = f'XenTerm-Next-Flutter-{sha}-{platform}'
    packages = [stem + ('.tar.gz' if platform.startswith('linux') else '.zip')]
    if platform == 'windows-x86_64':
        packages.append(stem + '.msi')
    return stem, packages, [stem + '-BUILD.json', stem + '.sha256', *sorted(packages)]


def latest_jobs(jobs):
    selected = {}
    for job in jobs:
        name = job.get('name')
        order = (job.get('run_attempt') or 0, job.get('id') or 0)
        old = selected.get(name)
        if old is None or order > (old.get('run_attempt') or 0, old.get('id') or 0):
            selected[name] = job
    return selected


def producer_ready(run, jobs, platform):
    sha = source_sha(run.get('head_sha'))
    if (run.get('repository', {}).get('full_name') != REPOSITORY or
            run.get('head_repository', {}).get('full_name') != REPOSITORY or
            run.get('head_branch') != BRANCH or run.get('path') != PRODUCERS[platform] or
            run.get('event') not in ('push', 'workflow_dispatch')):
        return False
    selected = latest_jobs(jobs)
    required = ['Prepare', f'Desktop ({platform})']
    if platform == 'windows-x86_64':
        required += ['MSI lifecycle (default)', 'MSI lifecycle (custom)', 'Verify Windows release']
    for name in required:
        job = selected.get(name, {})
        if (job.get('head_sha') != sha or job.get('status') != 'completed' or
                job.get('conclusion') != 'success'):
            return False
    # This conditional job is the producer's explicit publication intent.
    # It can still be dispatching when the publisher starts. Dispatch failure
    # does not invalidate product tests; a later wakeup can reconcile that run.
    request = selected.get(f'Release requested ({platform})', {})
    return (request.get('head_sha') == sha and request.get('status') in ('in_progress', 'completed')
            and request.get('conclusion') not in ('skipped', 'cancelled'))


def validate_receipt(receipt, sha, run_id, platform):
    _, packages, _ = asset_names(sha, platform)
    if not re.fullmatch('[1-9][0-9]*', str(run_id)):
        raise ValueError('Expected a positive product build run ID')
    identity = {'schema_version': 1, 'product': 'XenTerm Next Flutter', 'frontend': 'flutter',
                'repository': REPOSITORY, 'source_sha': sha, 'platform': platform,
                'build_run': str(run_id), 'channel': 'flutter-beta',
                'default_profile': 'config-flutter-next'}
    if any(type(receipt.get(k)) is not type(v) or receipt.get(k) != v for k, v in identity.items()):
        raise ValueError('Platform source/build identity mismatch')
    source_sha(receipt.get('upstream_snapshot'))
    msi_version = runtime_details(receipt.get('runtime_version', ''), sha)
    if platform == 'windows-x86_64':
        if receipt.get('msi_product_version') != msi_version:
            raise ValueError('MSI version does not match the source runtime')
    records = receipt.get('files', [])
    if (not isinstance(records, list) or len(records) != len(packages) or
            {record.get('name') for record in records if isinstance(record, dict)} != set(packages)):
        raise ValueError('Missing or duplicated platform packages')
    for record in records:
        if (type(record.get('size')) is not int or record['size'] <= 0 or
                not isinstance(record.get('sha256'), str) or not re.fullmatch('[0-9a-f]{64}', record['sha256'])):
            raise ValueError('Invalid package checksum/size record')
    return records


def validate_platform(directory, sha, run_id, platform):
    stem, packages, names = asset_names(sha, platform)
    if {p.name for p in directory.iterdir()} != set(names):
        raise ValueError('Incomplete or unexpected platform artifact files')
    if any(not p.is_file() or p.is_symlink() for p in directory.iterdir()):
        raise ValueError('Platform artifacts must contain regular files')
    receipt = json.loads((directory / (stem + '-BUILD.json')).read_text(encoding='utf-8-sig'))
    records = validate_receipt(receipt, sha, run_id, platform)
    for record in records:
        path = directory / record['name']
        if path.stat().st_size != record['size'] or sha256(path) != record['sha256']:
            raise ValueError('Platform package checksum/size mismatch')
    checksums = ''.join(f'{sha256(directory / name)}  {name}\n' for name in sorted(packages))
    if (directory / (stem + '.sha256')).read_text(encoding='utf-8') != checksums:
        raise ValueError('Platform checksum manifest mismatch')
    return receipt, {name: sha256(directory / name) for name in names}


def extract_artifact(archive, destination, names):
    destination.mkdir()
    with zipfile.ZipFile(archive) as source:
        entries = source.infolist()
        if len(entries) != len(names) or {entry.filename for entry in entries} != set(names):
            raise ValueError('Artifact contains unexpected or duplicate entries')
        for entry in entries:
            if ((entry.external_attr >> 16) & 0xf000) == 0xa000 or entry.is_dir():
                raise ValueError('Artifact links/directories are not permitted')
            with source.open(entry) as incoming, (destination / entry.filename).open('xb') as outgoing:
                for block in iter(lambda: incoming.read(1024 * 1024), b''):
                    outgoing.write(block)


class GitHub:
    def command(self, *args, **kwargs):
        return subprocess.check_output(args, text=True, encoding='utf-8', **kwargs).strip()

    def api(self, endpoint, method='GET', body=None):
        args = ['gh', 'api', f'repos/{REPOSITORY}/{endpoint}', '--method', method]
        if body is not None:
            args += ['--input', '-']
        value = self.command(*args, input=json.dumps(body) if body is not None else None, timeout=60)
        return json.loads(value) if value else None

    def pages(self, endpoint):
        return json.loads(self.command('gh', 'api', '--paginate', '--slurp',
                                      f'repos/{REPOSITORY}/{endpoint}', timeout=60))

    def jobs(self, run_id):
        return [job for page in self.pages(f'actions/runs/{run_id}/jobs?filter=all&per_page=100') for job in page['jobs']]

    def runs(self):
        # A notification is a wakeup, never the sole release input. Scan recent
        # runs for every producer, including older sources finishing later.
        result = []
        for page in range(1, 4):
            data = self.api(f'actions/runs?branch={BRANCH}&per_page=100&page={page}')
            result += data['workflow_runs']
            if len(data['workflow_runs']) < 100:
                break
        return result

    def maintained_source(self, sha):
        head = self.api(f'git/ref/heads/{BRANCH}')['object']['sha']
        if sha == head:
            return True
        comparison = self.api(f'compare/{sha}...{head}')
        return comparison.get('status') == 'ahead' and comparison.get('merge_base_commit', {}).get('sha') == sha

    def release(self, sha):
        tag = f'flutter-beta-{sha[:12]}'
        if self.tag_source(tag) not in ('', sha):
            raise ValueError('Source release tag identifies another commit')
        records = [record for page in self.pages('releases?per_page=100') for record in page if record.get('tag_name') == tag]
        if len(records) > 1:
            raise ValueError('Ambiguous source release')
        if records:
            result = records[0]
            if result.get('target_commitish') != sha or result.get('immutable'):
                raise ValueError('Source release is immutable or identifies another source')
            return result
        return None

    def create_release(self, sha):
        return self.api('releases', 'POST', {'tag_name': f'flutter-beta-{sha[:12]}', 'target_commitish': sha,
            'name': f'XenTerm Next Flutter beta ({sha[:12]})', 'draft': True,
            'prerelease': True, 'make_latest': 'false'})

    def assets(self, rid):
        return [asset for page in self.pages(f'releases/{rid}/assets?per_page=100') for asset in page]

    def download(self, endpoint, destination, *, accept='application/octet-stream'):
        with destination.open('xb') as stream:
            subprocess.run(['gh', 'api', f'repos/{REPOSITORY}/{endpoint}', '-H', f'Accept: {accept}'],
                           stdout=stream, check=True, timeout=180)

    def artifact(self, run_id, platform, destination):
        for attempt in range(5):
            data = self.api(f'actions/runs/{run_id}/artifacts?per_page=100')
            matches = [item for item in data['artifacts'] if item['name'] == f'flutter-package-{platform}' and not item['expired']]
            if matches:
                item = max(matches, key=lambda value: value['id'])
                # Actions negotiates JSON before redirecting to the artifact ZIP.
                # Release assets instead require octet-stream to return bytes.
                self.download(f'actions/artifacts/{item["id"]}/zip', destination,
                              accept='application/vnd.github+json')
                if item.get('digest') and item['digest'] != 'sha256:' + sha256(destination):
                    raise ValueError('Downloaded artifact digest mismatch')
                return
            if attempt < 4:
                time.sleep(2 ** attempt)
        raise ValueError('Successful producer artifact is unavailable')

    def upload(self, path, rid):
        return json.loads(self.command('gh', 'api',
            f'https://uploads.github.com/repos/{REPOSITORY}/releases/{rid}/assets?' + urlencode({'name': path.name}),
            '--method', 'POST', '--input', str(path), '-H', 'Content-Type: application/octet-stream', timeout=180))

    def verify(self, asset, path, checksum):
        current = self.api(f'releases/assets/{asset["id"]}')
        if (current.get('id') != asset['id'] or current.get('name') != path.name or
                current.get('state') != 'uploaded' or current.get('size') != path.stat().st_size):
            raise ValueError('Release asset identity mismatch')
        with tempfile.TemporaryDirectory() as temporary:
            downloaded = Path(temporary) / path.name
            self.download(f'releases/assets/{asset["id"]}', downloaded)
            if downloaded.stat().st_size != path.stat().st_size or sha256(downloaded) != checksum:
                raise ValueError('Release download checksum mismatch')

    def verify_digest(self, asset, checksum, size):
        current = self.api(f'releases/assets/{asset["id"]}')
        if (current.get('id') != asset['id'] or current.get('name') != asset['name'] or
                current.get('state') != 'uploaded' or current.get('size') != size):
            raise ValueError('Published platform asset identity mismatch')
        if current.get('digest'):
            if current['digest'] != 'sha256:' + checksum:
                raise ValueError('Published platform asset digest mismatch')
        else:
            with tempfile.TemporaryDirectory() as temporary:
                downloaded = Path(temporary) / asset['name']
                self.download(f'releases/assets/{asset["id"]}', downloaded)
                if downloaded.stat().st_size != size or sha256(downloaded) != checksum:
                    raise ValueError('Published platform download mismatch')

    def tag_source(self, tag):
        raw = self.command('git', 'ls-remote', 'origin', f'refs/tags/{tag}', f'refs/tags/{tag}^{{}}', timeout=30)
        values = dict(line.split()[::-1] for line in raw.splitlines())
        return values.get(f'refs/tags/{tag}^{{}}', values.get(f'refs/tags/{tag}', ''))


def release_body(sha, receipts, tag):
    lines = ['**下载**', '']
    for platform in PRODUCERS:
        if platform not in receipts:
            continue
        _, packages, _ = asset_names(sha, platform)
        links = [f'[{label}]({RELEASES_URL}/download/{tag}/{name})'
                 for name, label in zip(sorted(packages), PACKAGE_LABELS[platform], strict=True)]
        lines.append(f'- **{PLATFORM_NAMES[platform]}** · ' + ' · '.join(links))
    lines += ['', f'源码 `{sha}`。各平台独立验收，通过后追加到本版本。',
              '', '基于 [XenTerm](https://github.com/ixbaicn/XenTerm)，保留 MIT 许可证与原作者归属。', '']
    details = {
        'windows-x86_64': 'Windows ZIP/MSI 未签名，包含应用目录内的微软运行库。',
        'linux-x86_64': 'Linux 构建基于 Ubuntu 22.04。',
        'macos-aarch64': 'macOS 使用 ad-hoc 签名，未公证。',
    }
    for platform, receipt in sorted(receipts.items()):
        lines += [f'**{platform}** — `{receipt["runtime_version"]}`',
                  f'构建与验收：[CI {receipt["build_run"]}](https://github.com/{REPOSITORY}/actions/runs/{receipt["build_run"]})', '']
        stem, _, _ = asset_names(sha, platform)
        lines += [f'- [BUILD.json]({RELEASES_URL}/download/{tag}/{stem}-BUILD.json)',
                  f'- [SHA-256]({RELEASES_URL}/download/{tag}/{stem}.sha256)']
        lines += ['', details[platform], '']
    lines += ['每个平台的 BUILD.json 与 .sha256 记录该平台的来源和校验信息。']
    return '\n'.join(lines) + '\n'


def published_platform(api, known, sha, platform, directory):
    stem, _, names = asset_names(sha, platform)
    if not set(names).issubset(known):
        return None
    directory.mkdir()
    for name in (stem + '-BUILD.json', stem + '.sha256'):
        path = directory / name
        api.download(f'releases/assets/{known[name]["id"]}', path)
        api.verify_digest(known[name], sha256(path), path.stat().st_size)
    receipt = json.loads((directory / (stem + '-BUILD.json')).read_text(encoding='utf-8-sig'))
    records = validate_receipt(receipt, sha, receipt.get('build_run'), platform)
    expected = ''.join(f'{record["sha256"]}  {record["name"]}\n'
                       for record in sorted(records, key=lambda item: item['name']))
    if (directory / (stem + '.sha256')).read_text(encoding='utf-8') != expected:
        raise ValueError('Published platform checksum manifest mismatch')
    for record in records:
        api.verify_digest(known[record['name']], record['sha256'], record['size'])
    # Published packages no longer depend on expiring Actions artifacts.
    return receipt


def publish_platform(api, sha, platform, run):
    if not api.maintained_source(sha):
        return None
    release = api.release(sha)
    listed = api.assets(release['id']) if release else []
    known = {item['name']: item for item in listed}
    if len(known) != len(listed):
        raise ValueError('Duplicate release asset names')
    with tempfile.TemporaryDirectory(prefix='flutter-platform-release-') as temporary:
        directory = Path(temporary)
        stem, _, names = asset_names(sha, platform)
        receipt = published_platform(api, known, sha, platform, directory / 'existing')
        if receipt is None:
            receipt_name = stem + '-BUILD.json'
            if receipt_name in known:
                # A failed upload can leave a receipt first. Resume the producer
                # it names rather than overwriting packages with a later rebuild.
                saved = directory / 'partial.json'
                api.download(f'releases/assets/{known[receipt_name]["id"]}', saved)
                old = json.loads(saved.read_text(encoding='utf-8-sig'))
                validate_receipt(old, sha, old.get('build_run'), platform)
                original = api.api(f'actions/runs/{old["build_run"]}')
                if original.get('head_sha') != sha or not producer_ready(original, api.jobs(original['id']), platform):
                    raise ValueError('Partial platform producer provenance mismatch')
                run = original
            archive = directory / 'artifact.zip'
            api.artifact(run['id'], platform, archive)
            payload = directory / 'payload'
            extract_artifact(archive, payload, names)
            receipt, hashes = validate_platform(payload, sha, run['id'], platform)
            if not release:
                release = api.create_release(sha)
            for name in names:
                if not api.maintained_source(sha):
                    raise ValueError('Product source left the maintained history')
                if name not in known:
                    known[name] = api.upload(payload / name, release['id'])
                api.verify(known[name], payload / name, hashes[name])
        receipts = {platform: receipt}
        for other in PRODUCERS:
            if other == platform:
                continue
            saved = published_platform(api, known, sha, other, directory / other)
            if saved is not None:
                if any(saved[key] != receipt[key] for key in ('runtime_version', 'upstream_snapshot')):
                    raise ValueError('Source platforms disagree about runtime version or upstream')
                receipts[other] = saved
        body = release_body(sha, receipts, release['tag_name'])
        if not api.maintained_source(sha) or api.tag_source(release['tag_name']) not in ('', sha):
            raise ValueError('Source release changed before publication')
        if release.get('body') != body or release.get('draft') is not False:
            api.api(f'releases/{release["id"]}', 'PATCH', {'body': body, 'draft': False,
                'prerelease': True, 'make_latest': 'false'})
        actual = api.api(f'releases/{release["id"]}')
        if (actual.get('draft') is not False or actual.get('body') != body or
                actual.get('target_commitish') != sha or api.tag_source(release['tag_name']) != sha):
            raise ValueError('Published platform release readback mismatch')
        return {'source_sha': sha, 'platforms': sorted(receipts), 'url': actual.get('html_url')}


def reconcile(api):
    candidates = {}
    for run in api.runs():
        platform = next((p for p, path in PRODUCERS.items() if path == run.get('path')), None)
        if platform is None or run.get('head_branch') != BRANCH or run.get('event') not in ('push', 'workflow_dispatch'):
            continue
        sha = source_sha(run.get('head_sha'))
        if not producer_ready(run, api.jobs(run['id']), platform):
            continue
        key = (sha, platform)
        if key not in candidates or run['id'] > candidates[key]['id']:
            candidates[key] = run
    results, errors = [], []
    for (sha, platform), run in sorted(candidates.items()):
        try:
            result = publish_platform(api, sha, platform, run)
            if result is not None:
                results.append(result)
        except Exception as error:
            # A bad platform must not prevent another successful platform from
            # becoming public. Keep the run red and preserve all uploaded files.
            errors.append((sha, platform, str(error)))
            print(json.dumps({'source_sha': sha, 'platform': platform, 'error': str(error)}))
    if errors:
        raise RuntimeError(f'{len(errors)} platform publication(s) failed; successful platforms remain published')
    return results


def verify_release_index(result):
    sha = result['source_sha']
    tag = f'flutter-beta-{sha[:12]}'
    index_path = f'/{REPOSITORY}/releases'
    expected = [f'{index_path}/tag/{tag}"']
    for platform in result['platforms']:
        _, packages, _ = asset_names(sha, platform)
        expected += [f'{index_path}/download/{tag}/{name}"' for name in packages]
    # The web index can lag behind the Releases API. Check the public page
    # after publication and retry briefly while GitHub updates its cache.
    request = Request(RELEASES_URL, headers={'Cache-Control': 'no-cache',
                                            'User-Agent': 'XenTerm-Next-release-verification'})
    for delay in (0, 2, 5, 10, 20):
        if delay:
            time.sleep(delay)
        try:
            with urlopen(request, timeout=15) as response:
                page = response.read().decode('utf-8')
        except OSError as error:
            print(f'Cannot read the public Releases page yet: {error}', flush=True)
            continue
        if all(link in page for link in expected):
            print(f'Verified {tag} and its package links on {RELEASES_URL}', flush=True)
            return
    raise RuntimeError(f'{tag} or its package links are missing from {RELEASES_URL}')


if __name__ == '__main__':
    if os.environ.get('GITHUB_REPOSITORY') != REPOSITORY or os.environ.get('GITHUB_REF') != f'refs/heads/{BRANCH}':
        raise ValueError('Platform publishing is restricted to the maintained Flutter branch')
    releases = reconcile(GitHub())
    print(json.dumps({'releases': releases}, ensure_ascii=False), flush=True)
    # A delayed build for an older SHA may publish on a later index page.
    # Only require the current source to appear on the first page.
    head = os.environ.get('GITHUB_SHA')
    current = next((item for item in reversed(releases) if item['source_sha'] == head), None)
    if current:
        verify_release_index(current)
