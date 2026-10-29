"""Native original-crate qualification, synthetic data only.

Adapted from reviewed native control 45cb4eb4: subprocess.run waits for the
actual child. Hosted step/job deadlines remain the outer lifecycle boundary.
No process-tree cleanup, zero-residue or aggregate RSS guarantee is claimed.
Raw logs stay on the ephemeral runner; only structured evidence is uploaded.
"""
import hashlib, json, os, re, shutil, subprocess, sys, time
from pathlib import Path
HERE=Path(__file__).resolve().parent
CONTROL=HERE.parent
I=json.loads((HERE/'inputs.json').read_text())
SOURCE=Path(os.environ.get('GITHUB_WORKSPACE','.')).resolve()/'source'
OUT=Path(os.environ.get('RUNNER_TEMP','.')).resolve()/'key-import-native-evidence'
BRANCH='validation/key-import-native-20261029'
FEATURES=I['FEATURES']
UNIX_ONLY=tuple('ssh::key_import::tests::'+n for n in ('rejects_symlinks_without_reading_the_target','refuses_replacement_file_symlink_and_fifo_after_metadata_check'))
SUMMARY=re.compile(r'^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; \d+ filtered out; finished in [0-9.]+s$')
RECORDS=[]

def require(ok,message):
    if not ok: raise RuntimeError(message)
def digest(path):
    h=hashlib.sha256()
    with path.open('rb') as f:
        for chunk in iter(lambda:f.read(1024*1024),b''):h.update(chunk)
    return h.hexdigest()
def write(name,data): (OUT/name).write_text(json.dumps(data,indent=2)+'\n',encoding='utf-8')
def read(name): return json.loads((OUT/name).read_text(encoding='utf-8'))
def names(platform):
    require(platform in ('Windows','macOS'),'unsupported platform')
    return [n for n in I['NAMES'] if platform!='Windows' or n not in UNIX_ONLY]
def git(*args,control=False):
    return subprocess.check_output(['git','-C',str(CONTROL if control else SOURCE),*args],text=True,timeout=20).strip()
def identities():
    require(git('rev-parse','HEAD')==I['SHA'],'source changed')
    require(git('rev-parse','HEAD',control=True)==os.environ['EXPECTED_CONTROL_SHA'],'control changed')
    require(not git('status','--porcelain',control=True),'dirty control')
def patch_hashes():
    for n,h in I['PATCHES'].items():require(digest(HERE/n)==h,'patch changed')
def parse_inventory(output,name):
    require([s for s in output.splitlines() if s.endswith(': test') or s.endswith(': benchmark')]==[name+': test'],'inventory mismatch')
def parse_result(output,name):
    lines=output.splitlines();require(lines.count('running 1 test')==1,'count mismatch')
    require([s for s in lines if s.startswith('test ') and ' ... ' in s]==['test '+name+' ... ok'],'wrong or ignored test')
    summaries=[s for s in lines if s.startswith('test result:')]
    require(len(summaries)==1 and SUMMARY.fullmatch(summaries[0]),'summary mismatch')
def artifact_from_log(output,target):
    found=[]
    for line in output.splitlines():
        try:v=json.loads(line)
        except ValueError:continue
        if v.get('reason')=='compiler-artifact' and v.get('profile',{}).get('test') and v.get('executable') and v.get('target',{}).get('name')=='xenterm':
            require(v['target']['kind']==['bin'] and sorted(v['features'])==FEATURES,'product features mismatch')
            require(Path(v['target']['src_path']).resolve()==SOURCE/'src/main.rs','wrong source')
            lexical=Path(v['executable'])
            require(not any(p.is_symlink() for p in (lexical,*lexical.parents)),'symlink executable path')
            binary=lexical.resolve()
            require(binary.is_relative_to((OUT/'target'/target).resolve()) and binary.is_file(),'wrong executable')
            found.append({'path':str(binary),'sha256':digest(binary),'target':target,'features':FEATURES})
    require(len(found)==1,'executable count');return found[0]
def sandbox_env():
    # Runtime allowlist, not a copy of Actions/build environment. Preserve only
    # OS loader/locale essentials; profile/temp all point to synthetic sandbox.
    allowed={'PATH','SystemRoot','SYSTEMROOT','WINDIR','COMSPEC','PATHEXT','OS','NUMBER_OF_PROCESSORS','LANG','LC_ALL','TZ'}
    env={k:v for k,v in os.environ.items() if k in allowed}
    for key,part in [('HOME','home'),('USERPROFILE','home'),('APPDATA','appdata'),('LOCALAPPDATA','localappdata'),('XDG_CONFIG_HOME','config'),('XDG_DATA_HOME','data'),('XDG_CACHE_HOME','cache'),('XDG_RUNTIME_DIR','runtime'),('XENTERM_DATA_DIR','profile'),('TEMP','tmp'),('TMP','tmp'),('TMPDIR','tmp')]:
        p=OUT/part;p.mkdir(mode=0o700,parents=True,exist_ok=True);env[key]=str(p)
    env.update(RUST_BACKTRACE='0');return env

def run(argv,label,seconds,env):
    require(seconds>0,'deadline elapsed');start=time.monotonic();record={'label':label,'timeout_seconds':seconds,'state':'unknown'}
    # File-backed output avoids buffering arbitrarily large compile output in
    # memory. No raw text/exception is printed or uploaded.
    try:
        with (OUT/(label+'.stdout.log')).open('wb') as stdout,(OUT/(label+'.stderr.log')).open('wb') as stderr:
            result=subprocess.run(argv,cwd=SOURCE,env=env,stdin=subprocess.DEVNULL,stdout=stdout,stderr=stderr,timeout=seconds,check=False)
        record.update(exit_code=result.returncode,state='completed' if result.returncode==0 else 'nonzero-needs-review')
    except subprocess.TimeoutExpired:
        record.update(exit_code=None,state='timeout-hosted-lifecycle');raise
    finally:
        record['duration_seconds']=round(time.monotonic()-start,3)
        record['logs']={stream:{'bytes':(OUT/(label+'.'+stream+'.log')).stat().st_size,'sha256':digest(OUT/(label+'.'+stream+'.log'))} for stream in ('stdout','stderr')}
        RECORDS.append(record);write('commands.json',RECORDS)
    require(record['exit_code']==0,'child nonzero; unclassified')
    require((OUT/(label+'.stdout.log')).stat().st_size<64*1024*1024,'output exceeds evidence parse bound')
    return (OUT/(label+'.stdout.log')).read_text(encoding='utf-8',errors='strict')
def remaining(cap,deadline):
    value=min(cap,deadline-time.monotonic());require(value>0,'deadline elapsed');return value

def platform_target():
    platform=os.environ['RUNNER_OS']
    require((platform,sys.platform) in [('Windows','win32'),('macOS','darwin')],'not a native runner')
    return 'x86_64-pc-windows-msvc' if platform=='Windows' else 'aarch64-apple-darwin'

def prepare():
    platform_target()
    require(re.fullmatch('[0-9a-f]{40}',os.environ['EXPECTED_CONTROL_SHA']),'invalid control SHA')
    require(not OUT.exists(),'output exists');OUT.mkdir(mode=0o700)
    require(os.environ['GITHUB_SHA']==os.environ['EXPECTED_CONTROL_SHA'],'head mismatch')
    require(os.environ['GITHUB_REF']=='refs/heads/'+BRANCH,'branch mismatch')
    identities();patch_hashes();require(not git('status','--porcelain','--untracked-files=all','--ignored'),'dirty source')
    for n,tree in [('key-import.patch',I['PRODUCT_TREE']),('busy-save-tests-only.patch',I['TEST_TREE'])]:
        git('apply','--check','--whitespace=error',str(HERE/n));git('apply','--index',str(HERE/n));require(git('write-tree')==tree,'tree mismatch')
    git('diff','--cached','--check')
    for n,h in I['FIXTURES'].items():require(digest(SOURCE/n)==h,'fixture mismatch')
    files=[HERE/'native-key-import.py',HERE/'test_native_key_import.py',HERE/'inputs.json',CONTROL/'.github/workflows/ci.yml',*[HERE/n for n in I['PATCHES']]]
    write('prepared.json',{'source_sha':I['SHA'],'product_tree':I['PRODUCT_TREE'],'test_tree':I['TEST_TREE'],'control_sha':os.environ['EXPECTED_CONTROL_SHA'],'control_files':{str(p.relative_to(CONTROL)):digest(p) for p in files},'status':git('status','--porcelain','--untracked-files=all','--ignored'),'started_monotonic':time.monotonic()})

def compile_env():
    # Explicit SDK/toolchain allowlist: build scripts do not inherit Actions,
    # GitHub, registry credentials, proxy auth, or arbitrary parent variables.
    # Windows names are matched case-insensitively to preserve hosted SDK setup.
    allowed={n.casefold() for n in (
        'PATH','SystemRoot','WINDIR','COMSPEC','PATHEXT','OS','NUMBER_OF_PROCESSORS',
        'PROCESSOR_ARCHITECTURE','PROCESSOR_IDENTIFIER','ProgramFiles',
        'ProgramFiles(x86)','ProgramW6432','CommonProgramFiles','CommonProgramFiles(x86)',
        'INCLUDE','LIB','LIBPATH','VCINSTALLDIR','VCToolsInstallDir','VCToolsVersion',
        'VSINSTALLDIR','VisualStudioVersion','WindowsSdkDir','WindowsSDKVersion',
        'WindowsSDKLibVersion','UniversalCRTSdkDir','UCRTVersion',
        'VSCMD_ARG_HOST_ARCH','VSCMD_ARG_TGT_ARCH','SDKROOT','DEVELOPER_DIR',
        'MACOSX_DEPLOYMENT_TARGET','CC','CXX','AR','CMAKE_GENERATOR',
        'CMAKE_MAKE_PROGRAM','CMAKE_PREFIX_PATH','CARGO_HOME','RUSTUP_HOME',
        'LANG','LC_ALL','TZ')}
    env={k:v for k,v in os.environ.items() if k.casefold() in allowed}
    # Capture the already configured toolchain locations before isolated HOME.
    for k in ('CARGO_HOME','RUSTUP_HOME'):
        # Do not resolve HOME when a configured cache already supplies the path.
        # Windows has no passwd fallback if USERPROFILE is absent in a guard.
        if k not in env:env[k]=str(Path.home()/('.cargo' if k=='CARGO_HOME' else '.rustup'))
    sandbox=sandbox_env()
    for k in ('HOME','USERPROFILE','APPDATA','LOCALAPPDATA','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_CACHE_HOME','XDG_RUNTIME_DIR','XENTERM_DATA_DIR','TEMP','TMP','TMPDIR'):env[k]=sandbox[k]
    return env

def compile_product(target):
    require(target==platform_target(),'wrong native target')
    env=compile_env()
    env.update(CARGO_TARGET_DIR=str(OUT/'target'),XT_VERSION_JSON=str(OUT/'version.json'),GITHUB_SHA=I['SHA'],GITHUB_REF_TYPE='branch',GITHUB_REF_NAME=BRANCH,CARGO_BUILD_JOBS='2',CARGO_INCREMENTAL='0',CARGO_PROFILE_DEV_DEBUG='0',CARGO_PROFILE_TEST_DEBUG='0')
    cargo=shutil.which('cargo');require(cargo is not None,'cargo missing')
    deadline=read('prepared.json')['started_monotonic']+57*60
    output=run([cargo,'+1.99.1','test','--locked','--bin','xenterm','--target',target,'--jobs','2','--no-run','--message-format=json'],'compile',remaining(45*60,deadline),env)
    write('binary.json',artifact_from_log(output,target))

def runtime():
    global RECORDS
    require(read('binary.json')['target']==platform_target(),'wrong native binary target')
    RECORDS=read('commands.json');p=read('prepared.json');b=read('binary.json');binary=Path(b['path']);platform=os.environ['RUNNER_OS'];selected=names(platform)
    deadline=min(time.monotonic()+600,p['started_monotonic']+57*60);env=sandbox_env();results=[]
    for i,name in enumerate(selected):
        require(digest(binary)==b['sha256'],'binary changed')
        parse_inventory(run([str(binary),name,'--exact','--list','--format','terse'],f'list-{i}',remaining(30,deadline),env),name)
        require(digest(binary)==b['sha256'],'binary changed')
        parse_result(run([str(binary),name,'--exact','--test-threads=1','--format','pretty','--color','never'],f'test-{i}',remaining(60,deadline),env),name)
        results.append({'name':name,'state':'passed','executed':1,'ignored':0,'exit_code':0});write('tests.json',results)
    identities();patch_hashes();require(git('write-tree')==I['TEST_TREE'],'final tree changed');require(not git('diff','--exit-code'),'source mutated')
    require(git('status','--porcelain','--untracked-files=all','--ignored')==p['status'],'status changed')
    for n,h in p['control_files'].items():require(digest(CONTROL/n)==h,'control mutated')
    for n,h in I['FIXTURES'].items():require(digest(SOURCE/n)==h,'fixture mutated')
    require(digest(binary)==b['sha256'],'binary changed');v=read('version.json')
    require(v['hash']==I['SHA'][:7] and v['source']=='branch' and v['channel']=='dev' and v['branch']==BRANCH,'version mismatch')
    write('receipt.json',dict(p,patches=I['PATCHES'],fixtures=I['FIXTURES'],binary=b,version={'sha256':digest(OUT/'version.json'),'content':v},platform=platform,tests=results,not_applicable=list(UNIX_ONLY) if platform=='Windows' else [],scope='Native target GPUI test-platform synthetic fixtures, not real desktop UI or complete Windows security edge coverage',lifecycle='Hosted step/job outer boundary; subprocess.run waits; no process-tree cleanup/aggregate RSS/zero-residue guarantee'))

if __name__=='__main__':
    try:
        require(not sys.flags.optimize,'optimized Python forbidden')
        action=sys.argv[1]
        if action=='prepare':prepare()
        elif action=='compile':compile_product(sys.argv[2])
        elif action=='runtime':runtime()
        else:raise RuntimeError('unknown action')
    except BaseException as exc:
        if OUT.is_dir():write('failure.json',{'error_type':type(exc).__name__,'stage':sys.argv[1],'classification':'unclassified; not automatic product failure','raw_logs_uploaded':False})
        raise SystemExit(1)
