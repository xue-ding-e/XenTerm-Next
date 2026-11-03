"""Lightweight fail-closed controls. No downloads, Cargo, product, or SSH."""
import copy, importlib.util, io, pathlib, tarfile, tempfile, tomllib
spec = importlib.util.spec_from_file_location('backport', pathlib.Path(__file__).with_name('prepare-backport.py'))
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
def rejects(f):
    try: f()
    except (ValueError, tarfile.TarError): return
    raise AssertionError('negative control accepted')
raw = '''version = 4
[[package]]
name = "russh"
version = "0.61.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "''' + m.CRATE_SHA + '''"
dependencies = ["other"]
[[package]]
name = "other"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "abc"
'''
before = tomllib.loads(raw); after = tomllib.loads(m.make_lock(raw)); m.verify_lock(before,after)
for field, value in [('version','0.61.3'),('dependencies',[]),('checksum','reintroduced'),('source','fake')]:
    bad = copy.deepcopy(after); bad['package'][0][field] = value
    rejects(lambda: m.verify_lock(before,bad))
for field, value in [('version','2.0.0'),('checksum','def'),('source','fake'),('dependencies',[])]:
    bad = copy.deepcopy(after); bad['package'][1][field] = value
    rejects(lambda: m.verify_lock(before,bad))
for bad in [dict(after, version=3), dict(after, extra=True), dict(after, package=after['package'][:1])]:
    rejects(lambda: m.verify_lock(before,bad))
rejects(lambda: m.checked(b'wrong original crate', m.CRATE_SHA))
rejects(lambda: m.checked(b'wrong patch', m.PATCH_SHA))
rejects(lambda: m.patch_file(b'wrong original source', b'wrong patch'))
m.checked((m.BASE/'backport/russh-0.61.2-pr675.patch').read_bytes(),m.PATCH_SHA)
m.checked((m.BASE/'backport/LICENSE.txt').read_bytes(),m.LICENSE_SHA)
with tempfile.TemporaryDirectory() as temp:
    for name, kind in [('russh-0.61.2/../../escape',tarfile.REGTYPE),('/absolute',tarfile.REGTYPE),('wrong-root/a',tarfile.REGTYPE),('russh-0.61.2/link',tarfile.SYMTYPE),('russh-0.61.2/hard',tarfile.LNKTYPE),('russh-0.61.2/fifo',tarfile.FIFOTYPE),('russh-0.61.2/../escape',tarfile.DIRTYPE)]:
        buffer=io.BytesIO()
        with tarfile.open(fileobj=buffer,mode='w:gz') as archive:
            item=tarfile.TarInfo(name); item.type=kind; item.linkname='/outside'; archive.addfile(item)
        rejects(lambda: m.extract(buffer.getvalue(),pathlib.Path(temp)))
    buffer=io.BytesIO()
    with tarfile.open(fileobj=buffer,mode='w:gz') as archive:
        item=tarfile.TarInfo('russh-0.61.2/a'); item.size=2; archive.addfile(item,io.BytesIO(b'ok'))
    m.extract(buffer.getvalue(), pathlib.Path(temp))
    assert (pathlib.Path(temp)/'russh-0.61.2/a').read_bytes()==b'ok'
print('PASS: hashes, full TOML lock version/checksum/source/dependency/top-level drift, safe archive extraction; no Cargo/product/network')
