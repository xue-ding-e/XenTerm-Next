"""Verify and publish the independent Flutter channel after all product gates.

Build jobs have read-only tokens. Only the final publisher has contents:write;
neither a pull request nor a stale source can promote a release. Existing assets
are retained so a failed upload never removes a user's last working download.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
from urllib.parse import urlencode

REPOSITORY = 'xue-ding-e/XenTerm-Next'
BRANCH = 'flutter-next'
TAG = 'flutter-beta'
WORKFLOW = '.github/workflows/next-flutter-windows.yml'
PLATFORMS = ('windows-x86_64', 'linux-x86_64', 'macos-aarch64')
REQUIRED_JOBS = ('Prepare', *(f'Desktop ({p})' for p in PLATFORMS),
                 'MSI lifecycle (default)', 'MSI lifecycle (custom)', 'Verify complete release')


def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(block)
    return value.hexdigest()


def full_sha(value):
    if not isinstance(value, str) or not re.fullmatch('[0-9a-f]{40}', value):
        raise ValueError('Expected a full source SHA')
    return value


def runtime_details(version, sha):
    match = re.fullmatch(r'xt-beta-([0-9a-f]{7})-T([0-9]{10})', version)
    if not match or match[1] != sha[:7]:
        raise ValueError('Flutter beta runtime does not identify its source')
    epoch = int(match[2])
    if not 0 < epoch <= 0xffffffff:
        raise ValueError('Runtime timestamp exceeds the MSI version range')
    # Same lossless 8/8/16-bit timestamp encoding as Next GPUI. Product identity
    # stays independent; the Cargo version does not order rolling beta builds.
    return f'{epoch >> 24}.{(epoch >> 16) & 255}.{epoch & 65535}'


def source_gate(sha, run_id, run, current, jobs):
    full_sha(sha)
    if (not str(run_id).isdigit() or int(run_id) <= 0 or
            str(run.get('id')) != str(run_id) or
            run.get('repository', {}).get('full_name') != REPOSITORY or
            run.get('head_repository', {}).get('full_name') != REPOSITORY or
            run.get('path') != WORKFLOW or run.get('head_branch') != BRANCH or
            run.get('event') not in ('push', 'workflow_dispatch') or
            run.get('head_sha') != sha or current != sha or
            run.get('status') not in ('in_progress', 'completed') or
            run.get('conclusion') not in (None, 'success')):
        raise ValueError('Only the current Flutter source and its own product CI may publish')
    for name in REQUIRED_JOBS:
        matches = [job for job in jobs if job.get('name') == name]
        if (len(matches) != 1 or matches[0].get('head_sha') != sha or
                matches[0].get('status') != 'completed' or matches[0].get('conclusion') != 'success'):
            raise ValueError('Missing successful product gate: ' + name)


def verify_packages(directory, sha, run_id):
    full_sha(sha)
    if not str(run_id).isdigit() or int(run_id) <= 0:
        raise ValueError('Expected a positive product build run ID')
    prefix = f'XenTerm-Next-Flutter-{sha}'
    expected = set()
    receipts = []
    version = upstream = None
    for platform in PLATFORMS:
        stem = f'{prefix}-{platform}'
        receipt_path = directory / (stem + '-BUILD.json')
        receipt = json.loads(receipt_path.read_text(encoding='utf-8-sig'))
        identity = {'schema_version': 1, 'product': 'XenTerm Next Flutter',
                    'frontend': 'flutter', 'channel': TAG, 'repository': REPOSITORY,
                    'source_sha': sha, 'build_run': str(run_id), 'platform': platform,
                    'default_profile': 'config-flutter-next'}
        if any(type(receipt.get(k)) is not type(v) or receipt.get(k) != v for k, v in identity.items()):
            raise ValueError('Artifact provenance mismatch: ' + platform)
        current_version = receipt.get('runtime_version', '')
        msi_version = runtime_details(current_version, sha)
        current_upstream = full_sha(receipt.get('upstream_snapshot'))
        if version is None:
            version, upstream = current_version, current_upstream
        if (version, upstream) != (current_version, current_upstream):
            raise ValueError('Platform builds disagree about version or upstream source')
        if platform == 'windows-x86_64' and receipt.get('msi_product_version') != msi_version:
            raise ValueError('MSI product version does not match the runtime timestamp')
        package_names = {stem + ('.tar.gz' if platform.startswith('linux') else '.zip')}
        if platform == 'windows-x86_64':
            package_names.add(stem + '.msi')
        files = receipt.get('files')
        if not isinstance(files, list) or len(files) != len(package_names):
            raise ValueError('Incomplete platform package manifest: ' + platform)
        if {f.get('name') for f in files if isinstance(f, dict)} != package_names:
            raise ValueError('Unexpected platform package names: ' + platform)
        for item in files:
            path = directory / item['name']
            if (not path.is_file() or path.is_symlink() or type(item.get('size')) is not int or
                    item['size'] <= 0 or path.stat().st_size != item['size'] or
                    item.get('sha256') != digest(path)):
                raise ValueError('Incomplete or changed package: ' + item['name'])
        expected.update(package_names)
        expected.update((receipt_path.name, stem + '.sha256'))
        sums = ''.join(f'{digest(directory / name)}  {name}\n' for name in sorted(package_names))
        if (directory / (stem + '.sha256')).read_text(encoding='utf-8') != sums:
            raise ValueError('Platform checksum manifest mismatch: ' + platform)
        receipts.append(receipt)
    sidecars = {prefix + '-BUILD.json', prefix + '-SHA256SUMS'}
    actual = {p.name for p in directory.iterdir()}
    if actual not in (expected, expected | sidecars):
        raise ValueError('Incomplete or unexpected release files')
    if any(not p.is_file() or p.is_symlink() for p in directory.iterdir()):
        raise ValueError('Release assets must be regular files')
    metadata = {'schema_version': 1, 'product': 'XenTerm Next Flutter', 'frontend': 'flutter',
                'channel': TAG, 'repository': REPOSITORY, 'source_sha': sha,
                'upstream_snapshot': upstream, 'runtime_version': version,
                'build_run': str(run_id), 'platforms': receipts,
                'validation': {'required_jobs': list(REQUIRED_JOBS),
                               'windows_msi': 'install, upgrade, uninstall; default and custom paths',
                               'limits': 'Hosted runners; no clean-Windows, standard-user, real SSH/SFTP or GPUI replacement acceptance.'},
                'upstream': 'https://github.com/ixbaicn/XenTerm', 'license': 'MIT'}
    manifest = directory / (prefix + '-BUILD.json')
    if manifest.exists():
        if json.loads(manifest.read_text(encoding='utf-8')) != metadata:
            raise ValueError('Existing release provenance differs')
    else:
        manifest.write_text(json.dumps(metadata, indent=2) + '\n', encoding='utf-8')
    hashes = {name: digest(directory / name) for name in sorted(expected | {manifest.name})}
    sums = ''.join(f'{value}  {name}\n' for name, value in hashes.items())
    checksum = directory / (prefix + '-SHA256SUMS')
    if checksum.exists() and checksum.read_text(encoding='utf-8') != sums:
        raise ValueError('Existing release checksums differ')
    if not checksum.exists():
        checksum.write_text(sums, encoding='utf-8')
    hashes[checksum.name] = digest(checksum)
    return metadata, hashes


class GitHub:
    def command(self, *args, **kwargs):
        return subprocess.check_output(args, text=True, encoding='utf-8', **kwargs).strip()

    def api(self, endpoint, method='GET', body=None):
        args = ['gh', 'api', f'repos/{REPOSITORY}/{endpoint}', '--method', method]
        if body is not None:
            args += ['--input', '-']
        raw = self.command(*args, input=json.dumps(body) if body is not None else None, timeout=60)
        return json.loads(raw) if raw else None

    def pages(self, endpoint):
        pages = json.loads(self.command('gh', 'api', '--paginate', '--slurp',
                                      f'repos/{REPOSITORY}/{endpoint}', timeout=60))
        return pages

    def check(self, sha, run_id):
        run = self.api(f'actions/runs/{run_id}')
        current = self.api(f'git/ref/heads/{BRANCH}')['object']['sha']
        jobs = [job for page in self.pages(f'actions/runs/{run_id}/jobs?per_page=100') for job in page['jobs']]
        source_gate(sha, run_id, run, current, jobs)

    def release(self, sha):
        releases = [r for page in self.pages('releases?per_page=100') for r in page if r.get('tag_name') == TAG]
        if len(releases) > 1:
            raise ValueError('Ambiguous Flutter release; no release will be changed')
        if not releases:
            return self.api('releases', 'POST', {'tag_name': TAG, 'target_commitish': sha,
                'name': f'XenTerm Next Flutter beta ({sha[:12]})', 'draft': True,
                'prerelease': True, 'body': 'Complete artifacts verified locally; uploading and checking downloads.'})
        release = releases[0]
        if release.get('immutable'):
            raise ValueError('Flutter channel is immutable; refusing to change repository settings')
        if type(release.get('id')) is not int or type(release.get('draft')) is not bool:
            raise ValueError('Invalid Flutter release identity')
        return release

    def assets(self, rid):
        return [asset for page in self.pages(f'releases/{rid}/assets?per_page=100') for asset in page]

    def upload(self, path, rid):
        raw = self.command('gh', 'api',
            f'https://uploads.github.com/repos/{REPOSITORY}/releases/{rid}/assets?' + urlencode({'name': path.name}),
            '--method', 'POST', '--input', str(path), '-H', 'Content-Type: application/octet-stream', timeout=180)
        return json.loads(raw)

    def verify(self, asset, path, checksum):
        current = self.api(f'releases/assets/{asset["id"]}')
        if (current.get('id') != asset['id'] or current.get('name') != path.name or
                current.get('state') != 'uploaded' or current.get('size') != path.stat().st_size):
            raise ValueError('Remote asset identity mismatch: ' + path.name)
        # Verify downloaded bytes even for an idempotent retry. A draft's
        # browser_download_url may use an untagged URL until it is published.
        with tempfile.TemporaryFile() as target:
            subprocess.run(['gh', 'api', f'repos/{REPOSITORY}/releases/assets/{asset["id"]}',
                            '-H', 'Accept: application/octet-stream'], stdout=target, check=True, timeout=180)
            if target.tell() != path.stat().st_size:
                raise ValueError('Remote asset byte count mismatch: ' + path.name)
            target.seek(0)
            value = hashlib.sha256()
            for block in iter(lambda: target.read(1024 * 1024), b''):
                value.update(block)
            if value.hexdigest() != checksum:
                raise ValueError('Remote asset checksum mismatch: ' + path.name)

    def tag(self):
        result = self.command('git', 'ls-remote', 'origin', f'refs/tags/{TAG}', timeout=30)
        return result.split()[0] if result else ''

    def promote_tag(self, sha, old):
        # Exact expected ref lease preserves concurrent manual updates. The
        # command targets only Flutter's rolling tag, never either product branch.
        self.command('git', '-c', 'credential.helper=', '-c', 'credential.helper=!gh auth git-credential',
                     'push', f'--force-with-lease=refs/tags/{TAG}:{old}', 'origin', f'{sha}:refs/tags/{TAG}', timeout=60)


def publish(api, directory, sha, run_id):
    metadata, hashes = verify_packages(directory, sha, run_id)
    api.check(sha, run_id)
    release = api.release(sha)
    rid = release['id']
    assets = api.assets(rid)
    known = {asset['name']: asset for asset in assets}
    if len(known) != len(assets):
        raise ValueError('Duplicate release asset names')
    for name, checksum in hashes.items():
        api.check(sha, run_id)
        path = directory / name
        if name not in known:
            known[name] = api.upload(path, rid)
        api.verify(known[name], path, checksum)
    old = api.tag()
    api.check(sha, run_id)
    api.promote_tag(sha, old)
    api.check(sha, run_id)
    if api.tag() != sha:
        raise ValueError('Flutter tag changed concurrently')
    links = '\n'.join(f'- [{name}](https://github.com/{REPOSITORY}/releases/download/{TAG}/{name})' for name in hashes)
    body = (f'XenTerm Next Flutter 独立预览版；与 GPUI beta 分开维护，基于 [XenTerm](https://github.com/ixbaicn/XenTerm)，保留 MIT 归属。\n\n'
            f'源码 SHA: `{sha}`\n上游快照: `{metadata["upstream_snapshot"]}`\n'
            f'应用版本: `{metadata["runtime_version"]}`\n'
            f'构建与安装验收: https://github.com/{REPOSITORY}/actions/runs/{run_id}\n\n'
            'Windows x64 同时提供 portable ZIP 和 MSI，附带应用目录内的微软 VC++ 运行库。MSI 数字版本由应用版本时间戳无损编码；安装、升级、卸载已分别在默认和自定义目录通过。\n'
            'Windows 产物未签名；macOS Apple Silicon 仅 ad-hoc 签名、未公证；Linux x64 基于 Ubuntu 22.04。请核对 SHA256SUMS。\n'
            '验证使用 GitHub 托管 runner；干净 Windows、标准用户、真实 SSH/SFTP 和替代 GPUI 的功能对等验收仍需独立完成。\n'
            '以下链接指向同一源码和同一次构建；旧资产保留用于回退。\n\n' + links)
    api.api(f'releases/{rid}', 'PATCH', {'name': f'XenTerm Next Flutter beta ({sha[:12]})',
        'target_commitish': sha, 'body': body, 'draft': False, 'prerelease': True, 'make_latest': 'false'})
    actual = api.api(f'releases/{rid}')
    if actual.get('draft') is not False or actual.get('body') != body or api.tag() != sha:
        raise ValueError('Flutter publication readback mismatch; retaining all assets')
    return actual


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--source-sha', required=True)
    parser.add_argument('--run-id', required=True)
    parser.add_argument('--verify-only', action='store_true')
    args = parser.parse_args()
    if args.verify_only:
        metadata, hashes = verify_packages(args.directory, args.source_sha, args.run_id)
        print(json.dumps({'source_sha': metadata['source_sha'], 'assets': list(hashes)}))
    else:
        if os.environ.get('GITHUB_REPOSITORY') != REPOSITORY:
            raise ValueError('Flutter publication is restricted to the maintained repository')
        result = publish(GitHub(), args.directory, args.source_sha, args.run_id)
        print(result['html_url'])
