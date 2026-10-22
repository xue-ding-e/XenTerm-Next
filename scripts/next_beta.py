#!/usr/bin/env python3
"""Publish the single Next beta; no release mutation precedes verified inputs."""
import hashlib
import json
import os
from pathlib import Path
import re
import stat
from pathlib import PurePosixPath
import subprocess
import tempfile
import time
import tarfile
import zipfile
from packaging_version import prepared_metadata, validate_manifest_version

REPOSITORY = 'xue-ding-e/XenTerm-Next'
WORKFLOW = '.github/workflows/ci.yml'
PLATFORMS = {'windows-x86_64': '.zip', 'linux-x86_64': '.tar.gz', 'macos-aarch64': '.zip'}
MANAGED = re.compile(r'^XenTerm-Next-beta-[0-9a-f]{40}-(?:windows-x86_64\.zip|linux-x86_64\.tar\.gz|macos-aarch64\.zip|SHA256SUMS|BUILD\.json)$')


def gate(repository, sha, main_sha, run):
    checks = [
        repository == REPOSITORY,
        bool(re.fullmatch('[0-9a-f]{40}', sha)), sha == main_sha,
        run.get('repository', {}).get('full_name') == REPOSITORY,
        run.get('head_repository', {}).get('full_name') == REPOSITORY,
        run.get('path') == WORKFLOW,
        run.get('head_branch') == 'main', run.get('head_sha') == sha,
        run.get('event') in ('push', 'workflow_dispatch'),
        run.get('status') == 'completed', run.get('conclusion') == 'success',
    ]
    if not all(checks):
        raise RuntimeError('Release gate rejected repository, branch, SHA, workflow or CI status')


def prefix(sha):
    if not re.fullmatch('[0-9a-f]{40}', sha):
        raise ValueError('Expected a full lowercase Git SHA')
    return f'XenTerm-Next-beta-{sha}-'


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def safe_members(names, package_root):
    if len(names) != len(set(names)):
        raise RuntimeError('Duplicate archive paths')
    for name in names:
        parts = PurePosixPath(name).parts
        if name.rstrip('/') != package_root and not name.startswith(package_root + '/'):
            raise RuntimeError('Archive member outside expected package root')
        if not name or name.startswith('/') or '\\' in name or ':' in name or '..' in parts:
            raise RuntimeError('Unsafe archive member path')


def verify_archive(path, platform, sha, ci_id, build_id):
    # Inspect, never extract, archives in the write-enabled publisher. Accept
    # only the expected package root and the manifest produced by this run.
    if platform.startswith('macos'):
        root = 'XenTerm Next.app/Contents/Resources/'
        binary_name = 'XenTerm Next.app/Contents/MacOS/xenterm'
    else:
        root = prefix(sha) + platform + '/'
        binary_name = root + ('xenterm.exe' if platform.startswith('windows') else 'xenterm')
    package_root = 'XenTerm Next.app' if platform.startswith('macos') else prefix(sha) + platform
    required = {root + 'LICENSE', root + 'THIRD_PARTY_NOTICES.md', root + 'BUILD.json', binary_name}
    if path.suffix == '.zip':
        with zipfile.ZipFile(path) as archive:
            safe_members(archive.namelist(), package_root)
            if any(stat.S_ISLNK(item.external_attr >> 16) for item in archive.infolist()):
                raise RuntimeError('Archive symlinks are unsupported')
            names = set(archive.namelist())
            if not required <= names:
                raise RuntimeError('Archive missing executable, license or provenance')
            if archive.getinfo(root + 'BUILD.json').file_size > 16384:
                raise RuntimeError('Oversized package provenance')
            manifest = json.loads(archive.read(root + 'BUILD.json'))
    else:
        with tarfile.open(path, 'r:gz') as archive:
            safe_members(archive.getnames(), package_root)
            if any(not (member.isfile() or member.isdir()) for member in archive.getmembers()):
                raise RuntimeError('Archive links and special files are unsupported')
            names = set(archive.getnames())
            if not required <= names or any(not archive.getmember(n).isfile() for n in required):
                raise RuntimeError('Archive missing regular executable, license or provenance')
            if archive.getmember(root + 'BUILD.json').size > 16384:
                raise RuntimeError('Oversized package provenance')
            manifest = json.load(archive.extractfile(root + 'BUILD.json'))
    if any(manifest.get(k) != v for k, v in {
        'repository': REPOSITORY, 'sha': sha, 'platform': platform,
        'ci_run': str(ci_id), 'build_run': str(build_id),
    }.items()):
        raise RuntimeError('Archive provenance does not match tested source and build')
    try:
        return validate_manifest_version(manifest, sha)
    except ValueError as error:
        raise RuntimeError(str(error)) from error


def prepare_assets(directory, sha, ci_id, build_id):
    p = prefix(sha)
    packages = {p + platform + suffix for platform, suffix in PLATFORMS.items()}
    sidecars = {p + 'BUILD.json', p + 'SHA256SUMS'}
    actual = {f.name for f in directory.iterdir()}
    if actual not in (packages, packages | sidecars) or any(not f.is_file() or f.is_symlink() or f.stat().st_size == 0 for f in directory.iterdir()):
        raise RuntimeError('Expected exactly three nonempty, regular platform archives')
    versions = [verify_archive(directory / (p + platform + suffix), platform, sha, ci_id, build_id)
                for platform, suffix in PLATFORMS.items()]
    if any(version != versions[0] for version in versions[1:]):
        raise RuntimeError('Platform archives disagree on runtime/package version')
    metadata = {**versions[0], 'repository': REPOSITORY, 'commit': sha, 'ci_run': str(ci_id),
                'build_run': str(build_id), 'platforms': sorted(PLATFORMS),
                'windows_signing': 'unsigned', 'macos_signing': 'ad-hoc, not notarized',
                'upstream': 'https://github.com/ixbaicn/XenTerm', 'license': 'MIT'}
    if actual == packages | sidecars:
        # An idempotent retry must verify existing provenance/checksums, never
        # overwrite metadata to bless files from another source or build run.
        if json.loads((directory / (p + 'BUILD.json')).read_text(encoding='utf-8')) != metadata:
            raise RuntimeError('Existing release provenance mismatch')
        sums = ''.join(f'{digest(f)}  {f.name}\n' for f in sorted(directory.iterdir())
                       if f.name != p + 'SHA256SUMS')
        if (directory / (p + 'SHA256SUMS')).read_text(encoding='utf-8') != sums:
            raise RuntimeError('Existing checksum manifest mismatch')
        return {f.name: digest(f) for f in sorted(directory.iterdir())}
    (directory / (p + 'BUILD.json')).write_text(json.dumps(metadata, indent=2) + '\n', encoding='utf-8')
    files = sorted(directory.iterdir())
    (directory / (p + 'SHA256SUMS')).write_text(''.join(f'{digest(f)}  {f.name}\n' for f in files), encoding='utf-8')
    return {f.name: digest(f) for f in sorted(directory.iterdir())}


def obsolete_assets(assets, current):
    return [a for a in assets if MANAGED.fullmatch(a['name']) and a['name'] not in current]


class GitHub:
    def command(self, *args, **kwargs):
        return subprocess.check_output(args, text=True, encoding='utf-8', **kwargs).strip()

    def api(self, endpoint, method='GET', body=None):
        args = ['gh', 'api', f'repos/{REPOSITORY}/{endpoint}', '--method', method]
        if body is not None:
            args += ['--input', '-']
        raw = self.command(*args, input=json.dumps(body) if body is not None else None)
        return json.loads(raw) if raw else None

    def check(self, sha, ci_id):
        run = self.api(f'actions/runs/{ci_id}')
        main = self.api('git/ref/heads/main')['object']['sha']
        gate(os.environ['GITHUB_REPOSITORY'], sha, main, run)

    def release(self):
        # Listing distinguishes a missing beta from auth/server errors. Never turn
        # an arbitrary failed GET into permission to create another release.
        pages = json.loads(self.command('gh', 'api', '--paginate', '--slurp',
                                       f'repos/{REPOSITORY}/releases?per_page=100'))
        return next((r for page in pages for r in page if r['tag_name'] == 'beta'), None)

    def assets(self, release_id):
        pages = json.loads(self.command('gh', 'api', '--paginate', '--slurp',
                         f'repos/{REPOSITORY}/releases/{release_id}/assets?per_page=100', timeout=15))
        return [asset for page in pages for asset in page]

    def upload(self, path):
        self.command('gh', 'release', 'upload', 'beta', str(path), '--repo', REPOSITORY)

    def verify(self, asset, expected, size):
        if asset.get('state') != 'uploaded' or asset.get('size') != size:
            raise RuntimeError('Incomplete release asset: ' + asset['name'])
        with tempfile.TemporaryFile() as dest:
            subprocess.run(['gh', 'api', f'repos/{REPOSITORY}/releases/assets/{asset["id"]}',
                            '-H', 'Accept: application/octet-stream'], stdout=dest, check=True)
            dest.seek(0)
            h = hashlib.sha256()
            for block in iter(lambda: dest.read(1024 * 1024), b''):
                h.update(block)
            if h.hexdigest() != expected:
                raise RuntimeError('Remote checksum mismatch: ' + asset['name'])

    def tag(self):
        # ls-remote returns no line only when the ref is genuinely absent.
        result = self.command('git', 'ls-remote', 'origin', 'refs/tags/beta')
        return result.split()[0] if result else ''

    def move_tag(self, sha, old):
        self.command('git', '-c', 'credential.helper=', '-c',
                     'credential.helper=!gh auth git-credential', 'push',
                     f'--force-with-lease=refs/tags/beta:{old}', 'origin',
                     f'{sha}:refs/tags/beta')


def visible_assets(api, release_id, required):
    # Upload success need not be immediately visible in the asset list. Retry
    # only reads, never uploads or checksum failures; missing assets fail closed.
    for delay in (0, 1, 2, 4):
        if delay:
            time.sleep(delay)
        listed = api.assets(release_id)
        assets = {asset['name']: asset for asset in listed}
        if len(assets) != len(listed):
            raise RuntimeError('Duplicate release asset names')
        if set(required) <= assets.keys():
            return assets
    raise RuntimeError('Release assets not visible after bounded readback: ' +
                       ', '.join(sorted(set(required) - assets.keys())))


def publish(api, directory, sha, ci_id, build_id):
    expected = prepare_assets(directory, sha, ci_id, build_id)
    api.check(sha, ci_id)
    release = api.release()
    if release is None:
        api.check(sha, ci_id)
        release = api.api('releases', 'POST', {'tag_name': 'beta', 'target_commitish': sha,
                          'name': 'XenTerm Next beta (preparing)', 'draft': True,
                          'prerelease': True, 'make_latest': 'false'})
    if release.get('immutable'):
        raise RuntimeError('Immutable beta cannot be rolled; no settings will be changed')
    rid = release['id']
    assets = visible_assets(api, rid, ())
    for name, checksum in expected.items():
        api.check(sha, ci_id)
        path = directory / name
        if name not in assets:
            api.upload(path)
            assets = visible_assets(api, rid, (name,))
        api.verify(assets[name], checksum, path.stat().st_size)
    # Re-read every new asset before touching either ref or release metadata.
    assets = visible_assets(api, rid, expected)
    for name, checksum in expected.items():
        api.verify(assets[name], checksum, (directory / name).stat().st_size)
    old = api.tag()
    api.check(sha, ci_id)
    api.move_tag(sha, old)
    api.check(sha, ci_id)
    if api.tag() != sha:
        raise RuntimeError('Beta tag changed concurrently')
    links = '\n'.join(f'- [{name}](https://github.com/{REPOSITORY}/releases/download/beta/{name})'
                      for name in expected)
    body = (f'XenTerm Next 独立增强预览版，基于 [XenTerm](https://github.com/ixbaicn/XenTerm)，保留 MIT 归属。\n\n'
            f'源码 SHA: `{sha}`\nCI: https://github.com/{REPOSITORY}/actions/runs/{ci_id}\n'
            f'Build: https://github.com/{REPOSITORY}/actions/runs/{build_id}\n\n'
            'Windows portable 未签名（unsigned）；macOS 仅 ad-hoc 签名、未公证。请核对 SHA256SUMS；不要关闭系统安全保护。\n'
            'Linux x86_64 构建于 Ubuntu 22.04，不宣称 glibc 2.28 兼容。可执行文件内部名称仍为 xenterm。\n'
            '滚动预览更新时不同 SHA 资产可能短暂共存；以下链接明确指定本次构建。\n\n' + links)
    api.api(f'releases/{rid}', 'PATCH', {'name': f'XenTerm Next beta ({sha[:12]})',
            'body': body, 'draft': False, 'prerelease': True, 'make_latest': 'false'})
    verified = api.api(f'releases/{rid}')
    if verified['body'] != body or verified['draft'] or api.tag() != sha:
        raise RuntimeError('Release readback mismatch; preserving all old assets')
    # Never delete unknown/manual attachments. Abort cleanup if main moved; a
    # subsequent successful generation can remove recognized stale assets.
    for asset in obsolete_assets(api.assets(rid), expected):
        api.check(sha, ci_id)
        if api.tag() != sha:
            raise RuntimeError('Tag changed before cleanup')
        api.api(f'releases/assets/{asset["id"]}', 'DELETE')


if __name__ == '__main__':
    prepared_metadata(required=True)
    publish(GitHub(), Path('dist'), os.environ['SOURCE_SHA'],
            os.environ['CI_RUN_ID'], os.environ['GITHUB_RUN_ID'])
