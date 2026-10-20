"""Verify only the fixed published Windows portable; no tokens or builds."""
import hashlib, json, os, pathlib, stat, urllib.parse, urllib.request, urllib.error, zipfile, datetime
SHA = '77ce7313b71d176c26f62ec199b34aaad91c2251'
REPO = 'xue-ding-e/XenTerm-Next'
PREFIX = 'XenTerm-Next-beta-' + SHA + '-'
RUNTIME = 'xt-beta-77ce731-T1792516261'
ASSETS = {
    'BUILD.json': (660003322, 538, 'a7912da6cf62167bbdb829f77285a31673e7687d6c1629b472fb991eb0a18af8'),
    'SHA256SUMS': (660003397, 568, '197fc1c1e26e5c05a4735997b392b8d4b3c6397cfb5617394769bc8c307c159e'),
    'windows-x86_64.zip': (660003884, 43710067, '2739215d5555a7819ac390ce08372fcaff6fe44ca2e6830e8df2b1cbf87f86c6'),
    'linux-x86_64.tar.gz': (660003488, 50198413, 'c227f498a8c13796b2eedfa344517411dbaa469d9c3d6fc8122133382c6d18ef'),
    'macos-aarch64.zip': (660003722, 43294194, '8c7806426d013a6bbb98f15d961b3b3bb0feb3bd7c0074da3ebb0eed74d5f77f'),
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
    for run_id, name, event in [(39461925613, 'CI', 'push'), (39463787968, 'XenTerm Next beta', 'workflow_run')]:
        run = api('/actions/runs/' + str(run_id))
        if (run['head_sha'], run['head_branch'], run['status'], run['conclusion'], run['name'], run['event'], run['repository']['full_name']) != (SHA, 'main', 'completed', 'success', name, event, REPO):
            raise RuntimeError('Release or CI provenance mismatch')
    root = pathlib.Path(os.environ['RUNNER_TEMP']) / 'xenterm-published-77ce731'
    root.mkdir(exist_ok=False)
    (root / '.owned-native-release').write_text('/'.join(os.environ[k] for k in ('GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT', 'GITHUB_SHA')), encoding='utf-8')
    downloaded = {}
    for suffix in ('BUILD.json', 'SHA256SUMS', 'windows-x86_64.zip'):
        asset_id, size, sha256 = ASSETS[suffix]
        data = read(assets[PREFIX + suffix]['browser_download_url'], size)
        if len(data) != size or digest(data) != sha256: raise RuntimeError('Downloaded asset byte mismatch')
        (root / suffix).write_bytes(data); downloaded[suffix] = data
    build = json.loads(downloaded['BUILD.json'])
    if (build['repository'], build['commit'], build['ci_run'], build['build_run'], build['runtime_version'], build['package_version']) != (REPO, SHA, '39461925613', '39463787968', RUNTIME, '0.7.3'):
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
    if (inner['repository'], inner['sha'], inner['platform'], inner['ci_run'], inner['build_run'], inner['runtime_version'], inner['package_version']) != (REPO, SHA, 'windows-x86_64', '39461925613', '39463787968', RUNTIME, '0.7.3'):
        raise RuntimeError('Inner package provenance mismatch')
    exe = package / 'xenterm.exe'
    identity = {'status': 'published_release_identity_verified_not_execution_pass', 'repository': REPO, 'build_sha': SHA,
                'harness_sha': os.environ['GITHUB_SHA'], 'harness_source_is_not_application_source': True,
                'ci_run': 39461925613, 'build_run': 39463787968, 'runtime_version': RUNTIME,
                'zip_sha256': ASSETS['windows-x86_64.zip'][2], 'binary_sha256': digest(exe.read_bytes()),
                'binary_bytes': exe.stat().st_size, 'package_dir': str(package), 'windows_signing': 'unsigned'}
    evidence = pathlib.Path(os.environ['RUNNER_TEMP']) / 'native-evidence'
    (evidence / 'release-identity.json').write_text(json.dumps(identity, indent=2), encoding='utf-8')
    with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as output:
        output.write('identity_gate=verified\npackage_dir=' + str(package) + '\n')
if __name__ == '__main__': main()
