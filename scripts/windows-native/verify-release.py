"""Verify only the fixed published Windows portable; no tokens or builds."""
import hashlib, json, os, pathlib, stat, urllib.parse, urllib.request, urllib.error, zipfile, datetime
SHA = 'b59003f674b7fc4a17c7b389ea0af252e08d2ecf'
REPO = 'xue-ding-e/XenTerm-Next'
PREFIX = 'XenTerm-Next-beta-' + SHA + '-'
RUNTIME = 'xt-beta-b59003f-T1792541318'
ASSETS = {
    'BUILD.json': (660893044, 538, '6f773594941abe9366fd5540a8d620d7acf241a0c4b53d4c0c01cff526463140'),
    'SHA256SUMS': (660893103, 568, '9c4842b72685166397b521237983ec0f9ab605e52745c5ac56f84afb152c424d'),
    'windows-x86_64.zip': (660893536, 43709284, '0c2a7af0d8e21f09c6bdbd542ce08683fb87ab18930ef96a8205ef991d9973ec'),
    'linux-x86_64.tar.gz': (660893160, 50198695, '63eb35ce713fc7ce715a0d0b6711ccba3828edd843a1f2e6f4eb72910a1ffa7f'),
    'macos-aarch64.zip': (660893354, 43294098, '7684fc1385e6ea44fe20727d9c6df8b91efe643d607c9211974c7a9aa3d76841'),
}
class PublicRedirects(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        url = urllib.parse.urlparse(newurl)
        if url.scheme != 'https' or url.hostname not in ('github.com', 'release-assets.githubusercontent.com'):
            raise RuntimeError('Unexpected public download redirect')
        return super().redirect_request(req, fp, code, msg, headers, newurl)
OPENER = urllib.request.build_opener(PublicRedirects())
def read(url, limit):
    request = urllib.request.Request(url, headers={'User-Agent': 'XenTerm-Next-bounded-release-QA'})
    try:
        with OPENER.open(request, timeout=30) as response:
            data = response.read(limit + 1)
            if len(data) > limit: raise RuntimeError('Response size bound exceeded')
            return data
    except urllib.error.HTTPError as error:
        # Public request only. Never record redirects, cookies, credentials or response body.
        parsed = urllib.parse.urlparse(url)
        evidence = pathlib.Path(os.environ['RUNNER_TEMP']) / 'native-evidence'
        allowed = ('x-ratelimit-limit', 'x-ratelimit-remaining', 'x-ratelimit-reset', 'retry-after', 'date')
        record = {'stage': 'public-release-verification-before-input', 'status': 'http-error-no-retry',
                  'host': parsed.hostname, 'path': parsed.path, 'http_status': error.code,
                  'observed_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                  'headers': {k: error.headers[k][:128] for k in allowed if error.headers.get(k)},
                  'product_started': False, 'accepted_input_events': 0}
        (evidence / 'public-http-error.json').write_text(json.dumps(record, indent=2), encoding='utf-8')
        error.close()
        raise

def api(path):
    return json.loads(read('https://api.github.com/repos/' + REPO + path, 2 * 1024 * 1024))
def digest(data): return hashlib.sha256(data).hexdigest()

def main():
    if os.name != 'nt' or os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted':
        raise RuntimeError('Disposable hosted Windows only')
    release = api('/releases/tags/beta')
    if release['id'] != 412871971 or release['draft'] or not release['prerelease'] or release['tag_name'] != 'beta':
        raise RuntimeError('Release identity changed')
    tag = api('/git/ref/tags/beta')['object']
    if tag != {'sha': SHA, 'type': 'commit', 'url': 'https://api.github.com/repos/' + REPO + '/git/commits/' + SHA}:
        raise RuntimeError('Beta moved; do not substitute a newer binary')
    assets = {a['name']: a for a in release['assets']}
    if set(assets) != {PREFIX + suffix for suffix in ASSETS}: raise RuntimeError('Published asset set changed')
    for suffix, (asset_id, size, sha256) in ASSETS.items():
        asset = assets[PREFIX + suffix]
        if asset['browser_download_url'] != 'https://github.com/' + REPO + '/releases/download/beta/' + PREFIX + suffix:
            raise RuntimeError('Unexpected public asset URL')
        if (asset['id'], asset['size'], asset['digest'], asset['state']) != (asset_id, size, 'sha256:' + sha256, 'uploaded'):
            raise RuntimeError('Published asset identity changed')
    for run_id, name, event in [(39518269978, 'CI', 'push'), (39519470032, 'XenTerm Next beta', 'workflow_run')]:
        run = api('/actions/runs/' + str(run_id))
        if (run['head_sha'], run['head_branch'], run['status'], run['conclusion'], run['name'], run['event'], run['repository']['full_name']) != (SHA, 'main', 'completed', 'success', name, event, REPO):
            raise RuntimeError('Release or CI provenance mismatch')
    root = pathlib.Path(os.environ['RUNNER_TEMP']) / 'xenterm-published-b59003f'
    root.mkdir(exist_ok=False)
    (root / '.owned-native-release').write_text('/'.join(os.environ[k] for k in ('GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT', 'GITHUB_SHA')), encoding='utf-8')
    downloaded = {}
    for suffix in ('BUILD.json', 'SHA256SUMS', 'windows-x86_64.zip'):
        asset_id, size, sha256 = ASSETS[suffix]
        data = read(assets[PREFIX + suffix]['browser_download_url'], size)
        if len(data) != size or digest(data) != sha256: raise RuntimeError('Downloaded asset byte mismatch')
        (root / suffix).write_bytes(data); downloaded[suffix] = data
    build = json.loads(downloaded['BUILD.json'])
    if (build['repository'], build['commit'], build['ci_run'], build['build_run'], build['runtime_version'], build['package_version']) != (REPO, SHA, '39518269978', '39519470032', RUNTIME, '0.7.3'):
        raise RuntimeError('BUILD manifest mismatch')
    sums = dict(line.split('  ', 1)[::-1] for line in downloaded['SHA256SUMS'].decode('utf-8').splitlines())
    if sums != {PREFIX + suffix: info[2] for suffix, info in ASSETS.items() if suffix != 'SHA256SUMS'}:
        raise RuntimeError('Checksum manifest mismatch')
    stem = PREFIX + 'windows-x86_64'
    allowed = {'xenterm.exe', 'BUILD.json', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'XenTerm-Next.txt', 'licenses/HeroGPUI-NOTICE', 'licenses/HeroGPUI-APACHE-2.0'}
    package = root / stem
    with zipfile.ZipFile(root / 'windows-x86_64.zip') as z:
        members = z.infolist()
        if len(members) != len(allowed) or {m.filename for m in members} != {stem + '/' + n for n in allowed}:
            raise RuntimeError('Archive member set mismatch')
        if sum(m.file_size for m in members) > 200 * 1024 * 1024: raise RuntimeError('Expanded archive bound exceeded')
        for member in members:
            if member.is_dir() or stat.S_IFMT(member.external_attr >> 16) not in (0, stat.S_IFREG) or member.flag_bits & 1:
                raise RuntimeError('Archive member is not an ordinary unencrypted file')
            relative = member.filename[len(stem) + 1:]
            if relative != 'xenterm.exe' and member.file_size > 65536: raise RuntimeError('Notice/manifest size bound exceeded')
            target = package / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            with target.open('xb') as out: out.write(z.read(member))
    inner = json.loads((package / 'BUILD.json').read_text(encoding='utf-8'))
    if (inner['repository'], inner['sha'], inner['platform'], inner['ci_run'], inner['build_run'], inner['runtime_version'], inner['package_version']) != (REPO, SHA, 'windows-x86_64', '39518269978', '39519470032', RUNTIME, '0.7.3'):
        raise RuntimeError('Inner package provenance mismatch')
    exe = package / 'xenterm.exe'
    identity = {'status': 'published_release_identity_verified_not_execution_pass', 'repository': REPO, 'build_sha': SHA,
                'harness_sha': os.environ['GITHUB_SHA'], 'harness_source_is_not_application_source': True,
                'ci_run': 39518269978, 'build_run': 39519470032, 'runtime_version': RUNTIME,
                'zip_sha256': ASSETS['windows-x86_64.zip'][2], 'binary_sha256': digest(exe.read_bytes()),
                'binary_bytes': exe.stat().st_size, 'package_dir': str(package), 'windows_signing': 'unsigned'}
    evidence = pathlib.Path(os.environ['RUNNER_TEMP']) / 'native-evidence'
    (evidence / 'release-identity.json').write_text(json.dumps(identity, indent=2), encoding='utf-8')
    with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as output:
        output.write('identity_gate=verified\npackage_dir=' + str(package) + '\n')
if __name__ == '__main__': main()
