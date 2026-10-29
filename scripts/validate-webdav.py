"""Linux-only adaptation of reviewed bounded runner; not a release or advisory proof.

Reuse the process-group owner, bounded cleanup, output/disk/RSS limits and strict
libtest parsers unchanged. Windows/macOS actual TLS execution remains unverified.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import sys

HERE = Path(__file__).resolve().parent
RUNTIME_HASH = 'f6ce086e8578fb4066c5ee73d776f4d3db77282a0928a22acd3ba34e223b40df'
assert hashlib.sha256((HERE/'bounded-runtime.py').read_bytes()).hexdigest() == RUNTIME_HASH
spec = importlib.util.spec_from_file_location('bounded', HERE/'bounded-runtime.py')
b = importlib.util.module_from_spec(spec)
spec.loader.exec_module(b)
SHA = '79ef3363f0f600f5674ee2c245b5eb357f855961'
PATCHES = {
    'webdav-tls-policy-tests.patch': '7281721d8ad6bc293d7225cb6e9a97f899867e720f77984d018ec81e02151e34',
    'rustls-02346.patch': 'e36557a7e31b935199afd1f268a1dfbb5c87175d474f1b3c9e3abed461db4f23',
}
NAMES = (
    'app::webdav::tests::webdav_default_rejects_untrusted_loopback_certificate',
    'app::webdav::tests::webdav_explicit_opt_in_allows_untrusted_loopback_certificate',
)
TARGET = 'x86_64-unknown-linux-gnu'


def apply_patch(name):
    assert b.digest(HERE/name) == PATCHES[name]
    b.run(['git','apply','--check','--whitespace=error',str(HERE/name)], name+'-check', 10)
    # Preserve the exact reviewed candidate bytes; do not rewrite patches.
    b.run(['git','apply','--index',str(HERE/name)], name+'-apply', 10)


def desktop(stage):
    # Default features are desktop + remote-mcp, matching the product CI graph.
    output = b.run([b.CARGO,'+1.99.1','test','--locked','--bin','xenterm',
                    '--target',TARGET,'--jobs','1','--no-run','--message-format=json'],
                   stage+'-compile', 3600, 64*1024*1024)
    artifacts = []
    for line in output.splitlines():
        try: item = json.loads(line)
        except json.JSONDecodeError: continue
        if (item.get('reason') == 'compiler-artifact' and item.get('profile',{}).get('test')
            and item.get('executable') and Path(item['target']['src_path']).resolve() == b.SOURCE/'src/main.rs'):
            artifacts.append(Path(item['executable']).resolve())
    assert len(artifacts) == 1, artifacts
    binary = artifacts[0]
    assert binary.is_relative_to((b.OUT/'target').resolve()) and binary.is_file()
    digest = b.digest(binary)
    for i,name in enumerate(NAMES):
        b.verify_hash(b.digest(binary),digest)
        b.parse_inventory(b.run([str(binary),name,'--exact','--list','--format','terse'],stage+f'-list-{i}',30),name)
        b.verify_hash(b.digest(binary),digest)  # Recheck after listing, before execution.
        b.parse_result(b.run([str(binary),name,'--exact','--test-threads=1','--format','pretty','--color','never'],stage+f'-test-{i}',60),name)
    b.verify_hash(b.digest(binary),digest)
    return {'binary_sha256':digest,'tests':list(NAMES),'features':'default: desktop,remote-mcp','target':TARGET,'toolchain':'1.99.1'}


def validate():
    assert sys.platform == 'linux', 'bounded runtime uses Linux /proc, not a cross-platform runner'
    assert not sys.flags.optimize, 'assertion guards must be enabled'
    assert b.run(['git','rev-parse','HEAD'],'source-head',10).strip() == SHA
    assert not b.run(['git','status','--porcelain','--untracked-files=all','--ignored'],'clean-before',10).strip()
    control = HERE.parent
    expected = os.environ['EXPECTED_CONTROL_SHA']
    assert len(expected) == 40 and all(c in '0123456789abcdef' for c in expected)
    assert os.environ['GITHUB_SHA'] == expected
    assert b.run(['git','-C',str(control),'rev-parse','HEAD'],'control-head',10).strip() == expected
    assert not b.run(['git','-C',str(control),'status','--porcelain'],'control-clean',10).strip()
    for name,digest in PATCHES.items(): assert b.digest(HERE/name) == digest
    assert b.run(['git','rev-parse','HEAD:Cargo.lock'],'base-lock',10).strip() == 'b475ab5af8e42cf33406970838fc42c748dc9383'
    assert b.run(['git','rev-parse','HEAD:src/app/webdav.rs'],'base-webdav',10).strip() == 'c4100d3e4bbe94638c5dc9f8c5998e851ae0a836'
    b.SHA = SHA
    version = b.version_preflight(b.OUT)
    # Preserve runner-owned dependency caches, isolate all application locations.
    for key,sub in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_DATA_HOME','data'),('XDG_CACHE_HOME','cache'),('XDG_RUNTIME_DIR','runtime'),('XENTERM_DATA_DIR','profile'),('CARGO_TARGET_DIR','target')]:
        path = b.OUT/sub
        path.mkdir(mode=0o700)
        b.ENV[key] = str(path)
    for key in ['HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','http_proxy','https_proxy','all_proxy','XT_VERSION','XT_CHANNEL','RUSTUP_TOOLCHAIN']:
        b.ENV.pop(key,None)
    b.ENV.update(XT_VERSION_JSON=str(version),GITHUB_SHA=SHA,GITHUB_REF_TYPE='branch',GITHUB_REF_NAME='validation/webdav-tls-20261029')
    apply_patch('webdav-tls-policy-tests.patch')
    baseline_tree = b.run(['git','write-tree'],'baseline-tree',10).strip()
    baseline_status = b.run(['git','status','--porcelain','--untracked-files=all','--ignored'],'baseline-status',10).strip()
    b.run([b.CARGO,'+1.99.1','tree','--locked','-e','features','-i','rustls'],'original-rustls-features',120)
    baseline = desktop('original-lock')
    assert b.run(['git','status','--porcelain','--untracked-files=all','--ignored'],'baseline-status-after',10).strip() == baseline_status
    assert b.run(['git','write-tree'],'baseline-tree-after',10).strip() == baseline_tree
    assert not b.run(['git','diff','--exit-code'],'baseline-source-unchanged',10).strip()
    apply_patch('rustls-02346.patch')
    candidate_tree = b.run(['git','write-tree'],'upgraded-tree',10).strip()
    b.run([b.CARGO,'+1.99.1','tree','--locked','-e','features','-i','rustls'],'rustls-features',120)
    upgraded_status = b.run(['git','status','--porcelain','--untracked-files=all','--ignored'],'upgraded-status',10).strip()
    upgraded = desktop('upgraded-lock')
    # Explicit retained support gate: Linux headless ONLY, not GUI minimum Rust.
    b.run([b.CARGO,'+1.88.0','test','--locked','--no-default-features','--features','headless','--jobs','1'],'msrv-headless',1500,64*1024*1024)
    assert not b.run(['git','diff','--exit-code'],'source-unchanged',10).strip()
    assert b.run(['git','write-tree'],'tree-after',10).strip() == candidate_tree
    assert b.run(['git','status','--porcelain','--untracked-files=all','--ignored'],'upgraded-status-after',10).strip() == upgraded_status
    assert b.run(['git','rev-parse','HEAD'],'final-source-head',10).strip() == SHA
    assert b.run(['git','-C',str(control),'rev-parse','HEAD'],'final-control-head',10).strip() == expected
    assert not b.run(['git','-C',str(control),'status','--porcelain'],'final-control-clean',10).strip()
    for name,digest in PATCHES.items(): assert b.digest(HERE/name) == digest
    assert b.digest(HERE/'bounded-runtime.py') == RUNTIME_HASH
    assert b.digest(HERE/'owned-supervisor.py') == 'bc48ae5497f44cbdd97d73bce1a67c36e970c3dd13995336ce992de6c750b62c'
    (b.OUT/'receipt.json').write_text(json.dumps({'source_sha':SHA,'control_sha':expected,'patches':PATCHES,'runtime_sha256':RUNTIME_HASH,'baseline_tree':baseline_tree,'candidate_tree':candidate_tree,'original_lock':baseline,'upgraded_lock':upgraded,'msrv':'1.88.0 Linux headless passed','version_metadata':b.version_receipt(version),'limits':'Linux only; Windows/macOS native TLS pending; GET only; not advisory reproduction or full desktop CI'},indent=2))


if __name__ == '__main__':
    b.SOURCE = Path(sys.argv[1]).resolve()
    b.OUT = Path(sys.argv[2]).resolve()
    b.OUT.mkdir(parents=True,exist_ok=False)
    b.CARGO = os.path.abspath(shutil.which('cargo'))
    home = Path.home()
    b.ENV = dict(os.environ,CARGO_HOME=os.environ.get('CARGO_HOME',str(home/'.cargo')),RUSTUP_HOME=os.environ.get('RUSTUP_HOME',str(home/'.rustup')),CARGO_BUILD_JOBS='1',CARGO_PROFILE_DEV_DEBUG='0',CARGO_PROFILE_TEST_DEBUG='0',CARGO_INCREMENTAL='0')
    signal.signal(signal.SIGTERM,b.interrupted)
    signal.signal(signal.SIGINT,b.interrupted)
    try: validate()
    except BaseException as exc:
        (b.OUT/'failure.json').write_text(json.dumps({'error':str(exc),'source_sha':SHA,'classification':'unclassified; NOT product RED','regressions_passed':False}))
        raise
