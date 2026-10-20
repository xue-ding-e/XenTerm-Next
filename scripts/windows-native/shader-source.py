"""Exact source-only hypothesis experiment. No general archive extraction."""
import hashlib, json, os, pathlib, stat, sys, tarfile, time, urllib.request
ROOT = pathlib.Path(r'C:\Users\runneradmin\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\gpui-pre-windows-0.3.8')
NAMES = ('shaders.hlsl', 'color_text_raster.hlsl', 'alpha_correction.hlsl')
CHECKSUM = '371f9bb13f97e3bea2f8b8e5e81232bb74d735153c244ae67f7400e743925b1c'
URL = 'https://static.crates.io/crates/gpui-pre-windows/gpui-pre-windows-0.3.8.crate'

def gate():
    if os.name != 'nt' or os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted':
        raise RuntimeError('Disposable hosted Windows only')

def save(path, value):
    path.write_text(json.dumps(value, indent=2), encoding='utf-8')

def inspect():
    ancestors = list(reversed(ROOT.parents)) + [ROOT, ROOT / 'src']
    observations = []
    for p in ancestors:
        exists = os.path.lexists(p)
        reparse = exists and bool(os.lstat(p).st_file_attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT)
        observations.append({'path': str(p), 'exists': exists, 'reparse': reparse})
        if reparse or (exists and not p.is_dir()):
            raise RuntimeError('Unexpected ancestor type; no restoration')
    files = [{'path': str(ROOT / 'src' / n), 'exists': os.path.lexists(ROOT / 'src' / n)} for n in NAMES]
    eligible = not os.path.lexists(ROOT) and not any(f['exists'] for f in files)
    return {'status': 'absent_eligible' if eligible else 'stop_existing_path', 'ancestors': observations, 'files': files}

def cleanup(state):
    errors = []
    for entry in reversed(state.get('files', [])):
        p = pathlib.Path(entry['path'])
        try:
            if p not in [ROOT / 'src' / n for n in NAMES]: raise RuntimeError('Unexpected cleanup target')
            if os.path.lexists(p):
                if os.lstat(p).st_file_attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT: raise RuntimeError('Target became reparse point')
                if entry.get('sha256') and hashlib.sha256(p.read_bytes()).hexdigest() != entry['sha256']: raise RuntimeError('Created file changed; do not delete')
                p.unlink()
        except Exception as e: errors.append(type(e).__name__ + ': ' + str(e))
    for name in reversed(state.get('directories', [])):
        p = pathlib.Path(name)
        try:
            if p not in list(ROOT.parents) + [ROOT, ROOT / 'src']: raise RuntimeError('Unexpected directory cleanup target')
            if os.path.lexists(p):
                if os.lstat(p).st_file_attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT: raise RuntimeError('Directory became reparse point')
                p.rmdir()  # Empty only, never recursive.
        except Exception as e: errors.append(type(e).__name__ + ': ' + str(e))
    state['cleanup_errors'] = errors
    state['cleanup_confirmed'] = not errors and not os.path.lexists(ROOT)
    return state

def restore(evidence, staging):
    state = {'status': 'starting', 'directories': [], 'files': [], 'cleanup_confirmed': False}
    journal = evidence / 'shader-created.json'
    save(journal, state)
    archive = staging / 'gpui-pre-windows-0.3.8.crate'
    try:
        if inspect()['status'] != 'absent_eligible': raise RuntimeError('Absent gate changed')
        # Confirm current checkout lock still contains this exact package identity.
        lock = (pathlib.Path(os.environ['GITHUB_WORKSPACE']) / 'Cargo.lock').read_text(encoding='utf-8')
        block = 'name = "gpui-pre-windows"\nversion = "0.3.8"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "' + CHECKSUM + '"'
        if block not in lock: raise RuntimeError('Cargo.lock package identity mismatch')
        start = time.monotonic()
        with urllib.request.urlopen(URL, timeout=20) as response, archive.open('xb') as out:
            if response.url != URL: raise RuntimeError('Unexpected source redirect')
            total = 0
            while True:
                chunk = response.read(65536)
                if not chunk: break
                total += len(chunk)
                if total > 16 * 1024 * 1024 or time.monotonic() - start > 60: raise RuntimeError('Download bound exceeded')
                out.write(chunk)
        if hashlib.sha256(archive.read_bytes()).hexdigest() != CHECKSUM: raise RuntimeError('Official source checksum mismatch')
        wanted = {'gpui-pre-windows-0.3.8/src/' + n: n for n in NAMES}
        payloads = {}; expanded = 0; start = time.monotonic()
        with tarfile.open(archive, 'r|gz') as tar:
            for number, member in enumerate(tar, 1):
                expanded += member.size
                if number > 2048 or expanded > 64 * 1024 * 1024 or time.monotonic() - start > 30: raise RuntimeError('Archive scan bound exceeded')
                if member.name not in wanted: continue
                name = wanted[member.name]
                if not member.isfile() or member.size > 1024 * 1024 or name in payloads: raise RuntimeError('Unexpected requested archive member')
                payload = tar.extractfile(member).read(1024 * 1024 + 1)
                if len(payload) != member.size: raise RuntimeError('Unexpected source length')
                payloads[name] = payload
        if set(payloads) != set(NAMES): raise RuntimeError('Required source missing')
        if inspect()['status'] != 'absent_eligible': raise RuntimeError('Absent gate changed during download')
        for p in list(reversed(ROOT.parents)) + [ROOT, ROOT / 'src']:
            if not os.path.lexists(p):
                p.mkdir(); state['directories'].append(str(p)); save(journal, state)
        for name in NAMES:
            p = ROOT / 'src' / name
            with p.open('xb') as out:
                entry = {'path': str(p), 'sha256': None}; state['files'].append(entry); save(journal, state)
                out.write(payloads[name])
            entry['sha256'] = hashlib.sha256(payloads[name]).hexdigest(); save(journal, state)
            if hashlib.sha256(p.read_bytes()).hexdigest() != entry['sha256']: raise RuntimeError('Written source mismatch')
        state['status'] = 'restored_three_locked_source_files'; state['archive_sha256'] = CHECKSUM
        save(journal, state)
    except BaseException:
        cleanup(state); save(journal, state); raise
    finally:
        if archive.exists(): archive.unlink()

if __name__ == '__main__':
    gate()
    operation, directory = sys.argv[1:3]
    evidence = pathlib.Path(directory)
    if operation == 'probe':
        report = inspect(); save(evidence / 'shader-before.json', report)
        if report['status'] != 'absent_eligible': sys.exit(2)
    elif operation == 'restore': restore(evidence, pathlib.Path(sys.argv[3]))
    elif operation == 'cleanup':
        journal = evidence / 'shader-created.json'
        if journal.exists():
            state = cleanup(json.loads(journal.read_text(encoding='utf-8'))); save(journal, state)
            if not state['cleanup_confirmed']: sys.exit(3)
        else: save(evidence / 'shader-cleanup.json', {'status': 'nothing_created'})
    else: raise RuntimeError('Unknown operation')
