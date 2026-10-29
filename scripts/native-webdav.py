"""Evidence gates only, not a process supervisor.

Actual Cargo/libtest commands inherit the hosted runner step/job boundary. No PID
or process-name cleanup is attempted; failure leaves uncertain actors to the
hosted runner. Windows/macOS must execute real native TLS, not mocked coverage.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

BASE = '79ef3363f0f600f5674ee2c245b5eb357f855961'
TREE = 'fc25d67215b44310ef9ba2fa857dad94f3993f16'
BRANCH = 'validation/webdav-native-20261029'
PATCHES = {'rustls-02346.patch':'e36557a7e31b935199afd1f268a1dfbb5c87175d474f1b3c9e3abed461db4f23',
           'webdav-tls-policy-tests.patch':'095a847ea749a001f42c292bd9c5d03e22fe24b9b4946f27c45c9fd28bd5f129'}
NAMES = ('app::webdav::tests::webdav_default_rejects_untrusted_loopback_certificate',
         'app::webdav::tests::webdav_explicit_opt_in_allows_untrusted_loopback_certificate')
FEATURES = ['default','desktop','remote-mcp']
HERE = Path(__file__).resolve().parent
CONTROL = HERE.parent
SOURCE = Path(os.environ.get('GITHUB_WORKSPACE','.')).resolve()/'source'
OUT = Path(os.environ.get('RUNNER_TEMP','.')).resolve()/'webdav-native-evidence'
SUMMARY = re.compile(r'^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; \d+ filtered out; finished in [0-9.]+s$')


def require(ok,msg):
    if not ok: raise RuntimeError(msg)


def digest(p):
    h=hashlib.sha256()
    with p.open('rb') as f:
        for chunk in iter(lambda:f.read(1024*1024),b''):h.update(chunk)
    return h.hexdigest()


def git(*args,control=False):
    # Small Git guards only; the hosted step provides the outer timeout.
    return subprocess.check_output(['git','-C',str(CONTROL if control else SOURCE),*args],text=True).strip()


def identities():
    require(git('rev-parse','HEAD')==BASE,'source HEAD changed')
    require(git('rev-parse','HEAD',control=True)==os.environ['EXPECTED_CONTROL_SHA'],'control HEAD changed')
    require(not git('status','--porcelain',control=True),'dirty control')


def patch_hashes():
    for name,expected in PATCHES.items():require(digest(HERE/name)==expected,'patch changed: '+name)


def write(name,data): (OUT/name).write_text(json.dumps(data,indent=2)+'\n',encoding='utf-8')
def read(name): return json.loads((OUT/name).read_text(encoding='utf-8'))


def artifact_from_log(text,target):
    found=[]
    for line in text.splitlines():
        try:item=json.loads(line)
        except ValueError:continue
        if item.get('reason')=='compiler-artifact' and item.get('target',{}).get('name')=='xenterm' and item.get('executable') and item.get('profile',{}).get('test'):
            require(item['target']['kind']==['bin'],'not product bin')
            require(sorted(item['features'])==FEATURES,'wrong compiled features')
            require(Path(item['target']['src_path']).resolve()==SOURCE/'src/main.rs','wrong product source')
            binary=Path(item['executable']).resolve()
            require(binary.is_relative_to((SOURCE/'target'/target).resolve()),'binary outside target')
            found.append((binary,item))
    require(len(found)==1,'missing/duplicate product test executable')
    binary,item=found[0]
    require(binary.is_file() and not binary.is_symlink(),'missing/nonregular binary')
    return {'path':str(binary),'sha256':digest(binary),'features':item['features'],'target':target}


def verify_binary():
    receipt=read('binary.json');binary=Path(receipt['path'])
    require(digest(binary)==receipt['sha256'],'test executable changed')
    return binary


def parse_inventory(text,name):
    names=[s for s in text.splitlines() if s.endswith(': test') or s.endswith(': benchmark')]
    require(names==[name+': test'],'missing/duplicate/wrong inventory')


def parse_result(text,name):
    lines=text.splitlines()
    require(lines.count('running 1 test')==1,'not exactly one test')
    require([s for s in lines if s.startswith('test ') and ' ... ' in s]==['test '+name+' ... ok'],'wrong/ignored test')
    summaries=[s for s in lines if s.startswith('test result:')]
    require(len(summaries)==1 and SUMMARY.fullmatch(summaries[0]),'not 1 passed / 0 ignored')


def prepare():
    require(not OUT.exists(),'evidence output exists')
    OUT.mkdir(mode=0o700)
    require(os.environ['GITHUB_SHA']==os.environ['EXPECTED_CONTROL_SHA'],'dispatch control mismatch')
    require(os.environ['GITHUB_REF']=='refs/heads/'+BRANCH,'wrong control branch')
    identities();patch_hashes()
    require(not git('status','--porcelain','--untracked-files=all'),'source not clean')
    control_hashes={str(p.relative_to(CONTROL)):digest(p) for p in [HERE/'native-webdav.py',HERE/'test_native_webdav.py',CONTROL/'.github/workflows/ci.yml',*[HERE/n for n in PATCHES]]}
    for name in ['webdav-tls-policy-tests.patch','rustls-02346.patch']:
        git('apply','--check','--whitespace=error',str(HERE/name))
        git('apply','--index',str(HERE/name))
    require(git('write-tree')==TREE,'candidate tree mismatch')
    status=git('status','--porcelain','--untracked-files=all')
    write('prepared.json',{'source_sha':BASE,'control_sha':os.environ['EXPECTED_CONTROL_SHA'],'candidate_tree':TREE,'patches':PATCHES,'status':status,'control_files':control_hashes})
    # Preserve runner toolchain/dependency caches before isolating application HOME.
    env={'CARGO_HOME':os.environ.get('CARGO_HOME',str(Path.home()/'.cargo')),'RUSTUP_HOME':os.environ.get('RUSTUP_HOME',str(Path.home()/'.rustup')),
         'XT_VERSION_JSON':str(OUT/'version.json'),'WEBDAV_EVIDENCE':OUT.as_posix()}
    for key,part in [('HOME','home'),('USERPROFILE','home'),('APPDATA','appdata'),('LOCALAPPDATA','localappdata'),('XDG_CONFIG_HOME','config'),('XDG_DATA_HOME','data'),('XDG_CACHE_HOME','cache'),('XDG_RUNTIME_DIR','runtime'),('XENTERM_DATA_DIR','profile')]:
        p=OUT/part;p.mkdir(mode=0o700,exist_ok=True);env[key]=str(p)
    for key in ['HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','http_proxy','https_proxy','all_proxy','XT_VERSION','XT_CHANNEL']:env[key]=''
    with open(os.environ['GITHUB_ENV'],'a',encoding='utf-8') as f:
        for key,value in env.items():require('\n' not in value and '\r' not in value,'unsafe env value');f.write(key+'='+value+'\n')


def finish():
    identities();patch_hashes();p=read('prepared.json')
    require(git('write-tree')==TREE,'candidate tree changed')
    require(not git('diff','--exit-code'),'unstaged product mutation')
    require(git('status','--porcelain','--untracked-files=all')==p['status'],'source status changed')
    for path,expected in p['control_files'].items():require(digest(CONTROL/path)==expected,'control file changed')
    verify_binary()
    for i,name in enumerate(NAMES):
        parse_inventory((OUT/f'list-{i}.log').read_text(encoding='utf-8'),name)
        parse_result((OUT/f'test-{i}.log').read_text(encoding='utf-8'),name)
    version=read('version.json')
    require(version['hash']==BASE[:7] and version['channel']=='dev' and version['source']=='branch' and version['branch']==BRANCH,'wrong version provenance')
    write('receipt.json',dict(p,binary=read('binary.json'),tests=list(NAMES),version=version,
        toolchain=(OUT/'toolchain.log').read_text(encoding='utf-8'),platform=os.environ['RUNNER_OS'],
        scope='Upgraded lock native desktop two exact TLS tests only; not full desktop CI or advisory reproduction',
        lifecycle='Hosted runner step/job timeouts only; no custom process-tree cleanup or aggregate RSS guarantee'))


if __name__=='__main__':
    action=sys.argv[1]
    if action=='prepare':prepare()
    elif action=='compile-receipt':write('binary.json',artifact_from_log((OUT/'compile.log').read_text(encoding='utf-8'),sys.argv[2]))
    elif action=='exec':
        binary=verify_binary();i=int(sys.argv[2]);require(0<=i<len(NAMES),'bad test index')
        mode=sys.argv[3];require(mode in ['list','test'],'bad mode')
        if mode=='test':parse_inventory((OUT/f'list-{i}.log').read_text(encoding='utf-8'),NAMES[i])
        args=['--exact','--list','--format','terse'] if mode=='list' else ['--exact','--test-threads=1','--format','pretty','--color','never']
        # No subprocess supervisor or PID signaling: direct execution inherits
        # the hosted Actions step boundary. On failure do not attempt cleanup.
        os.execv(str(binary),[str(binary),NAMES[i],*args])
    elif action=='verify-test':
        i=int(sys.argv[2]);verify_binary()
        parse_inventory((OUT/f'list-{i}.log').read_text(encoding='utf-8'),NAMES[i])
        parse_result((OUT/f'test-{i}.log').read_text(encoding='utf-8'),NAMES[i])
    elif action=='finish':finish()
    else:raise RuntimeError('unknown action')
