"""Isolated qualification only. Never edits the original checkout or Cargo cache."""
import copy, hashlib, io, json, pathlib, re, shutil, subprocess, sys, tarfile, tempfile, tomllib, urllib.request
BASE = pathlib.Path(__file__).resolve().parent
CRATE_SHA = 'bbf893f64684e58da8a68d56a5e84d1cf0440226274c515770fe267707a7d0b0'
PATCH_SHA = '23d6181682b93aa7f49a8f38ce9443f3e495b2c8da556dc7e2b0bbad46fa88ef'
ORIGINAL_SHA = 'd8f4340413e22d654a522f93661fb211acf94fb6b250596034c172839ec1b4a7'
PATCHED_SHA = 'ddf947c3dc679bbcfe01fa723c1c1f1f7956c3753510c26f24e89f15638d94f8'
LICENSE_SHA = 'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30'
SOURCE = '495f000b69856384732f7281c3b544bef783a755'
TREE = '28d03b69c667d3dbda4ad241d6e856e193be3790'
NOTICE = b'// Modified for isolated qualification: backport of Corey Leavitt\'s russh PR #675,\n// upstream commit 6fc20b2803da7c77e5e89bd3afbf86c80d3cf3e5.\n// Original copyright and Apache-2.0 license remain unchanged.\n'
def need(ok, message):
    if not ok: raise ValueError(message)
def sha(data): return hashlib.sha256(data).hexdigest()
def checked(data, expected):
    need(sha(data) == expected, 'SHA-256 mismatch'); return data

def extract(data, destination):
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive:
        members = archive.getmembers()
        names = set()
        for item in members:
            parts = pathlib.PurePosixPath(item.name).parts
            need(parts and parts[0] == 'russh-0.61.2' and not item.name.startswith('/')
                 and '..' not in parts and '\\' not in item.name, 'archive path escape')
            need(item.isdir() or item.isfile(), 'archive links/special files forbidden')
            need(item.name not in names, 'duplicate archive member'); names.add(item.name)
        need(sum(m.size for m in members) < 32*1024*1024, 'archive too large')
        archive.extractall(destination, members=members, filter='data')

def patch_file(original, patch):
    checked(original, ORIGINAL_SHA); checked(patch, PATCH_SHA)
    suffix = b'\n                }\n                // Forward the close to the channel before removing it, so that'
    old = b'                    enc.channels.remove(&channel_num);' + suffix
    new = b'                    // Reply with CHANNEL_CLOSE per RFC 4254 Section 5.3.\n                    enc.close(channel_num)?;'
    new += suffix
    need(original.count(old) == 1, 'patch context ambiguous')
    result = original.replace(old, new, 1)
    checked(result, PATCHED_SHA)
    return NOTICE + result

def verify_lock(before, after):
    expected = copy.deepcopy(before)
    matches = [p for p in expected['package'] if p['name'] == 'russh']
    need(len(matches) == 1 and matches[0]['version'] == '0.61.2', 'unexpected russh package')
    need(matches[0].get('source') == 'registry+https://github.com/rust-lang/crates.io-index'
         and matches[0].get('checksum') == CRATE_SHA, 'unexpected locked russh provenance')
    del matches[0]['source']; del matches[0]['checksum']
    need(expected == after, 'lock drift outside exactly russh source/checksum removal')

def make_lock(raw):
    pieces = re.split(r'(?=^\[\[package\]\]$)', raw, flags=re.M)
    count = 0
    for index, piece in enumerate(pieces):
        if re.search(r'^name = "russh"$', piece, re.M):
            pieces[index] = re.sub(r'^(source|checksum) = .*\n', '', piece, flags=re.M); count += 1
    need(count == 1, 'expected one russh lock block')
    result = ''.join(pieces)
    verify_lock(tomllib.loads(raw), tomllib.loads(result))
    return result

def snapshot(root):
    result = {}
    for p in sorted(root.rglob('*')):
        need(not p.is_symlink(), 'prepared source symlink forbidden')
        if p.is_file(): result[str(p.relative_to(root))] = sha(p.read_bytes())
    return result

def main():
    mode, source_arg, evidence_arg = sys.argv[1:]
    source, evidence = pathlib.Path(source_arg).resolve(), pathlib.Path(evidence_arg).resolve()
    receipt_path = evidence/'backport-receipt.json'
    if mode == 'verify':
        receipt = json.loads(receipt_path.read_text())
        work = pathlib.Path(receipt['work_root'])
        need(sha((source/'Cargo.lock').read_bytes()) == receipt['original_lock_sha256'], 'original lock changed')
        need(snapshot(BASE) == receipt['controller_files'], 'controller changed')
        need(snapshot(work/'source') == receipt['prepared_files'], 'prepared source changed')
        need(snapshot(work/'russh-0.61.2') == receipt['crate_files'], 'prepared crate changed')
        verify_lock(tomllib.loads((source/'Cargo.lock').read_text()), tomllib.loads((work/'source/Cargo.lock').read_text()))
        return
    need(mode == 'prepare', 'unknown mode')
    git = lambda *args: subprocess.check_output(['git','-C',str(source),*args], timeout=10).decode().strip()
    need(git('rev-parse','HEAD') == SOURCE and git('rev-parse','HEAD^{tree}') == TREE, 'source identity mismatch')
    need(not git('status','--porcelain'), 'original source not clean')
    work = pathlib.Path(tempfile.mkdtemp(prefix='broker-russh675-'))
    # Receipt is created only after preparation succeeds; temp is always runner-owned.
    try:
        with urllib.request.urlopen('https://static.crates.io/crates/russh/russh-0.61.2.crate', timeout=60) as response:
            data = response.read(32*1024*1024 + 1)
        checked(data, CRATE_SHA); extract(data, work)
        crate = work/'russh-0.61.2'
        patch = (BASE/'backport/russh-0.61.2-pr675.patch').read_bytes()
        target = crate/'src/server/encrypted.rs'
        target.write_bytes(patch_file(target.read_bytes(), patch))
        license_bytes = checked((BASE/'backport/LICENSE.txt').read_bytes(), LICENSE_SHA)
        (crate/'LICENSE.txt').write_bytes(license_bytes)
        copied = work/'source'; copied.mkdir()
        tracked = subprocess.check_output(['git','-C',str(source),'ls-files','-z'], timeout=10).decode().split('\0')
        for name in filter(None, tracked):
            src = source/name
            need(src.is_file() and not src.is_symlink(), 'tracked input must be a regular file')
            dest = copied/name; dest.parent.mkdir(parents=True, exist_ok=True); shutil.copy2(src, dest)
        cargo = copied/'Cargo.toml'; original_manifest = cargo.read_text(); before = tomllib.loads(original_manifest)
        need('russh' not in before.get('patch',{}).get('crates-io',{}), 'existing russh override')
        need('crates-io' not in before.get('patch',{}), 'existing patch table requires explicit review')
        cargo.write_text(cargo.read_text() + '\n[patch.crates-io]\nrussh = { path = "../russh-0.61.2" }\n')
        expected = copy.deepcopy(before); expected.setdefault('patch',{})['crates-io']={'russh':{'path':'../russh-0.61.2'}}
        need(tomllib.loads(cargo.read_text()) == expected, 'manifest drift')
        lock = copied/'Cargo.lock'; original_lock = lock.read_text(); lock.write_text(make_lock(original_lock))
        shutil.copyfile(source/'Cargo.lock', evidence/'original-Cargo.lock')
        shutil.copyfile(lock, evidence/'temporary-Cargo.lock')
        import difflib
        (evidence/'Cargo.toml.diff').write_text(''.join(difflib.unified_diff(original_manifest.splitlines(True), cargo.read_text().splitlines(True), fromfile='original/Cargo.toml', tofile='temporary/Cargo.toml')))
        (evidence/'Cargo.lock.diff').write_text(''.join(difflib.unified_diff(original_lock.splitlines(True), lock.read_text().splitlines(True), fromfile='original/Cargo.lock', tofile='temporary/Cargo.lock')))
        receipt = dict(source_commit=SOURCE, source_tree=TREE, work_root=str(work), crate_sha256=sha(data), patch_sha256=sha(patch), original_file_sha256=ORIGINAL_SHA, patched_before_notice_sha256=PATCHED_SHA, modified_file_sha256=sha(target.read_bytes()), license_sha256=sha(license_bytes), original_lock_sha256=sha((source/'Cargo.lock').read_bytes()), temporary_lock_sha256=sha(lock.read_bytes()), lock_scope='only russh source/checksum removed; original lock NOT qualified', prepared_files=snapshot(copied), crate_files=snapshot(crate), controller_files=snapshot(BASE))
        receipt_path.write_text(json.dumps(receipt,indent=2)+'\n')
    except BaseException:
        shutil.rmtree(work); raise
if __name__ == '__main__': main()
