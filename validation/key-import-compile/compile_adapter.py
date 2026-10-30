"""Candidate only: no CLI, no dispatch, no dependency download or product run.

Host must be separately compiled/reviewed. Outer Host timeout is unknown lifecycle,
not proof of tree cleanup; caller poisons the job and cannot start another proof.
"""
import hashlib,json,subprocess,stat
from pathlib import Path
QUALIFIED_SHA='6850c63da8518d8095853e5ff1dbcfd73bb7f851'
QUALIFIED_RUN=41064036830
TRANSPORT_SHA='62f2b8a074e57524840f5f56134fdf63eccb6079f9d3c199e5a6e5c372728dbb'
METADATA_SHA='7c5e5417b6086e66a2967dc088cbae7941a8f74b58b9378d7ffbeb26d264b87a'
SOURCE_MANIFEST_SHA='d2f019f9121984119b890327c13f942494e0e476d16fd6944e0494da8c231db8'
CAP=16*1024*1024
HOST_FILES={'CompileOnlyHost.exe','CompileOnlyHost.dll','CompileOnlyHost.deps.json','CompileOnlyHost.runtimeconfig.json'}
ARGS=['+1.99.1','test','--frozen','--offline','--bin','xenterm','--target','x86_64-pc-windows-msvc','--jobs','2','--no-run','--message-format=json']

def require(value,code):
    if not value:raise ValueError(code)
def digest(p):return hashlib.sha256(Path(p).read_bytes()).hexdigest()
def canonical_parts(path):
    require(path.is_absolute() and '..' not in path.parts,'canonical absolute path required')
    return path
def plain(path,exists=True):
    path=canonical_parts(Path(path))
    for item in (path,*path.parents):
        try:info=item.lstat()
        except FileNotFoundError:
            require(item==path and not exists,'missing ancestor');continue
        require(not stat.S_ISLNK(info.st_mode) and not(getattr(info,'st_file_attributes',0)&0x400),'reparse point')
    return path

def read_json(path,limit):
    p=plain(path);require(p.stat().st_size<=limit,'metadata size')
    def pairs(items):
        out={}
        for key,value in items:require(key not in out,'duplicate key');out[key]=value
        return out
    return json.loads(p.read_text(encoding='utf-8-sig'),object_pairs_hook=pairs)

ENV_KEYS=set('PATH SYSTEMROOT WINDIR COMSPEC PATHEXT OS NUMBER_OF_PROCESSORS PROCESSOR_ARCHITECTURE PROCESSOR_IDENTIFIER PROGRAMFILES PROGRAMFILES(X86) PROGRAMW6432 COMMONPROGRAMFILES COMMONPROGRAMFILES(X86) INCLUDE LIB LIBPATH VCINSTALLDIR VCTOOLSINSTALLDIR VCTOOLSVERSION VSINSTALLDIR VISUALSTUDIOVERSION WINDOWSSDKDIR WINDOWSSDKVERSION WINDOWSSDKLIBVERSION UNIVERSALCRTSDKDIR UCRTVERSION VSCMD_ARG_HOST_ARCH VSCMD_ARG_TGT_ARCH LANG LC_ALL TZ HOME USERPROFILE APPDATA LOCALAPPDATA XDG_CONFIG_HOME XDG_DATA_HOME XDG_CACHE_HOME XDG_RUNTIME_DIR XENTERM_DATA_DIR TEMP TMP TMPDIR CARGO_HOME RUSTUP_HOME CARGO_TARGET_DIR CARGO_NET_OFFLINE CARGO_BUILD_JOBS CARGO_INCREMENTAL CARGO_PROFILE_DEV_DEBUG CARGO_PROFILE_TEST_DEBUG XT_VERSION_JSON GITHUB_SHA GITHUB_REF_TYPE GITHUB_REF_NAME'.split())
DIRECTORIES={'HOME':'home','USERPROFILE':'home','APPDATA':'appdata','LOCALAPPDATA':'localappdata','XDG_CONFIG_HOME':'config','XDG_DATA_HOME':'data','XDG_CACHE_HOME':'cache','XDG_RUNTIME_DIR':'runtime','XENTERM_DATA_DIR':'profile','TEMP':'tmp','TMP':'tmp','TMPDIR':'tmp','CARGO_HOME':'cargo-home','RUSTUP_HOME':'rustup-home','CARGO_TARGET_DIR':'target'}

def environment_gate(env,root):
    require(isinstance(env,dict),'explicit environment');fold={}
    for key,value in env.items():
        require(isinstance(key,str) and key.upper() in ENV_KEYS and key.upper() not in fold and isinstance(value,str) and '\0' not in value,'environment key/value')
        fold[key.upper()]=value
    for key,part in DIRECTORIES.items():require(fold.get(key)==str(root/part),'synthetic directory');plain(root/part)
    constants={'CARGO_NET_OFFLINE':'true','CARGO_BUILD_JOBS':'2','CARGO_INCREMENTAL':'0','CARGO_PROFILE_DEV_DEBUG':'0','CARGO_PROFILE_TEST_DEBUG':'0','GITHUB_SHA':'459b593c830b8be643df29f9d9c0c46915d54696','GITHUB_REF_TYPE':'branch','GITHUB_REF_NAME':'validation/key-import-compile-only-20261030','XT_VERSION_JSON':str(root/'version.json')}
    for key,value in constants.items():require(fold.get(key)==value,'compile environment constant')
    return fold

DEFAULT_CASES={'normal':3,'timeout':3,'output-budget':3,'parent-exit':3,'root-completed-cleanup':3,'root-nonzero':3,'root-mode-timeout':3,'root-mode-output-budget':3,'invalid-completion-mode':0,'invalid-env':0,'null-args':0,'nul-arg':0,'same-logs':0,'concurrent':3,'injected-cleanup-uncertainty':3}
COMPILE_CASES={'compile-policy-natural-tree-cleanup':3,'compile-policy-output-hard-cap':3,'compile-policy-timeout-cleanup':3,'compile-policy-argv-rejection':0,'compile-policy-root-mode-rejection':0,'compile-policy-deadline-rejection':0,'unknown-policy-rejection':0}

def qualification_gate(metadata_path):
    # Pinned bytes were downloaded only after official run AND job success were
    # verified. No caller-provided verified boolean or editable summary qualifies.
    path=plain(metadata_path);require(path.is_file() and path.stat().st_size<=65536,'native metadata bound')
    require(digest(path)==METADATA_SHA,'native artifact bytes differ')
    m=read_json(path,65536)
    require(m.get('controlSha')==QUALIFIED_SHA and m.get('runId')==str(QUALIFIED_RUN) and m.get('sourceManifestSha256','').lower()==SOURCE_MANIFEST_SHA,'native provenance')
    for flag in ('passed','initialIdentity','finalIdentity'):require(m.get(flag) is True,'native identity failure')
    for flag in ('productCompileAttempted','productRuntimeAttempted','full45MinuteElapsedValidation'):require(m.get(flag) is False,'native scope')
    require(set(m.get('suites',{}))=={'default15','compile7'},'native suite inventory')
    for name,required in [('default15',DEFAULT_CASES),('compile7',COMPILE_CASES)]:
        suite=m['suites'][name]
        require(suite.get('exit')==0 and suite.get('controlSha')==QUALIFIED_SHA and suite.get('runId')==str(QUALIFIED_RUN),'native suite provenance')
        r=suite.get('receipt',{});items=r.get('cases',[])
        require(r.get('passed') is True and len(items)==len(required),'native cases count')
        cases={item['name']:item for item in items};require(set(cases)==set(required),'native cases exact')
        for case,count in required.items():
            item=cases[case];require(item.get('passed') is True and type(item.get('actualExecutions')) is int and item['actualExecutions']==count,'native execution count')
            require(item.get('treeEmpty') is True if count else item.get('expectedRejection') is True,'native cleanup/rejection')
        if name=='compile7':
            timeout=cases['compile-policy-timeout-cleanup'];overflow=cases['compile-policy-output-hard-cap']
            require(timeout.get('injectedClock') is True and timeout.get('injectedMilliseconds')==8000 and timeout.get('productionRequestedSeconds')==2700,'short clock scope')
            require(overflow['stdoutBytes']>0 and overflow['stderrBytes']>0 and overflow['stdoutBytes']+overflow['stderrBytes']==CAP,'shared cap evidence')
            for item in (timeout,overflow):require(item.get('safeToStart') is True and 0<=item['cleanupMilliseconds']<=5000,'native cleanup deadline')
    return m

class OwnedCompileAdapter:
    # host_hashes and identity_gate must come from the separately reviewed build
    # controller. They are not proof that ambient PATH/SDK/cache inputs are trusted.
    def __init__(self,root,host,host_hashes,qualification,identity_gate):
        self.root=plain(root);self.host=plain(host);self.host_hashes=host_hashes
        self.qualification=qualification;self.identity_gate=identity_gate;self.poisoned=False
    def identity(self):
        qualification_gate(self.qualification)
        require(digest(Path(__file__).parent/'Host/windows-owned-process.cs')==TRANSPORT_SHA,'transport changed')
        require(self.host.name=='CompileOnlyHost.exe' and self.host.parent.is_relative_to(self.root),'host outside output')
        require(set(self.host_hashes)==HOST_FILES,'host file inventory')
        for name,h in self.host_hashes.items():require(digest(plain(self.host.parent/name))==h,'host binary changed')
        self.identity_gate()
    def __call__(self,request):
        require(not self.poisoned,'previous lifecycle uncertain')
        self.identity();self.poisoned=True
        record={'state':'unknown','treeEmpty':False,'outerHostLifecycle':'unknown','productRuntimeAttempted':False}
        out=self.root
        try:
            require(request.get('args')==ARGS and request.get('seconds')==2700 and request.get('completionMode')=='natural-tree' and request.get('commandPolicy')=='cargo-compile-only-v1','fixed compile command')
            require(request.get('executable')==str(out/'cargo-home/bin/cargo.exe') and request.get('cwd')==str(out/'source'),'compile paths')
            payload=dict(request,label='key-import-compile-only',stdout=str(out/'compile.stdout.log'),stderr=str(out/'compile.stderr.log'))
            payload['environment']=environment_gate(payload.get('environment'),out)
            raw=json.dumps(payload).encode();require(len(raw)<=131072,'request bound')
            request_path=plain(out/'compile.request.json',False);receipt_path=plain(out/'compile.receipt.json',False)
            require(not request_path.exists() and not receipt_path.exists(),'stale request/receipt')
            with request_path.open('xb') as f:f.write(raw)
            # 2700s child + 5s cleanup + 10s transport margin; not an OS kill guarantee.
            result=subprocess.run([str(self.host),str(request_path),str(receipt_path)],cwd=str(self.host.parent),env=payload['environment'],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=2715,check=False)
            require(result.returncode==0,'host failure')
            receipt=read_json(receipt_path,16384)
            require(receipt.get('state')=='returned' and receipt.get('productRuntimeAttempted') is False and receipt.get('productCompileAttempted') is True,'host receipt')
            for name in ('treeEmpty','safeToStart','boundaryRootSignaled','boundaryRootExitCodeKnown'):require(receipt.get(name) is True,name)
            for name in ('timedOut','outputExceeded','captureFailed','forcedOwnedCleanup'):require(receipt.get(name) is False,name)
            for name in ('exitCode','boundaryRootExitCode','boundaryActiveProcessCount'):require(type(receipt.get(name)) is int and receipt[name]==0,name)
            require(receipt.get('completionMode')=='natural-tree' and receipt.get('commandPolicy')=='cargo-compile-only-v1' and receipt.get('outputBudget')==CAP,'receipt policy')
            logs=[plain(out/'compile.stdout.log'),plain(out/'compile.stderr.log')]
            total=sum(f.stat().st_size for f in logs);require(total<=CAP and type(receipt.get('outputBytes')) is int and receipt['outputBytes']==total,'retained log budget')
            require(type(receipt.get('cleanupMilliseconds')) is int and 0<=receipt['cleanupMilliseconds']<=5000,'cleanup deadline')
            output=logs[0].read_text(encoding='utf-8',errors='strict')
            self.identity();self.poisoned=False;record.update(state='returned',treeEmpty=True,outerHostLifecycle='returned')
            # JSON parser receives bounded data locally; raw content is never uploaded.
            return receipt,output
        finally:
            try:self.identity();record['finalIdentityVerified']=True
            except BaseException:self.poisoned=True;record['finalIdentityVerified']=False;raise
            finally:
                record['ownerPoisoned']=self.poisoned
                with (out/'compile-adapter-state.json').open('x',encoding='utf-8') as f:json.dump(record,f)
