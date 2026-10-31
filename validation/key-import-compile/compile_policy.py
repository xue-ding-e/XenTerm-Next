"""Compile-only policy preparation. No launcher/CLI; not native-qualified yet.

Cargo offline forbids Cargo resolution network, NOT arbitrary build-script network.
No OS network isolation is supplied or claimed. Review known build scripts;
dependency materialization is a separate stage, not a network-permission change.
16 MiB is a provisional bounded ceiling, not an empirically sufficient allowance.
"""
from pathlib import Path
import hashlib,json,re,stat
MAIN='459b593c830b8be643df29f9d9c0c46915d54696'
PRODUCT_TREE='fb7fdd0881444bf790635876bc4088f095b75e43'
TEST_TREE='3b660a20b67381b3522b6a43f37aaaa433201958'
PATCH='1740f70f99d2c715a2f652d526d81896dd5f17db59e513cc50b2916f0e435305'
POLICY='cargo-compile-only-v2'
SECONDS=2700
CAP=16*1024*1024
ARGS=['test','--frozen','--offline','--bin','xenterm','--target','x86_64-pc-windows-msvc','--jobs','2','--no-run','--message-format=json']

def require(ok,code):
    if not ok: raise ValueError(code)

def command(executable,source):
    p=Path(executable); s=Path(source)
    require(p.is_absolute() and p.name.lower()=='cargo.exe' and p.is_file(),'cargo identity')
    require(s.is_absolute() and s.is_dir() and s.name=='source','source directory')
    require(p==s.parent/'rustup-home/toolchains/1.99.1-x86_64-pc-windows-msvc/bin/cargo.exe','fixed real cargo path')
    for q in (p,*p.parents,s,*s.parents):
        info=q.lstat()
        require(not stat.S_ISLNK(info.st_mode) and not(getattr(info,'st_file_attributes',0)&0x400),'reparse path')
    return dict(executable=str(p),args=list(ARGS),cwd=str(s),seconds=SECONDS,commandPolicy=POLICY,completionMode='natural-tree')

def receipt_ok(receipt):
    require(receipt.get('commandPolicy')==POLICY and receipt.get('completionMode')=='natural-tree','policy mismatch')
    require(receipt.get('outputBudget')==CAP and type(receipt.get('outputBytes')) is int and 0<=receipt['outputBytes']<=CAP,'output bound')
    for key in ('treeEmpty','safeToStart','boundaryRootSignaled','boundaryRootExitCodeKnown'):
        require(receipt.get(key) is True,key)
    for key in ('timedOut','outputExceeded','captureFailed','forcedOwnedCleanup'):
        require(receipt.get(key) is False,key)
    for key in ('exitCode','boundaryRootExitCode','boundaryActiveProcessCount'):
        require(type(receipt.get(key)) is int and receipt[key]==0,key)

DEFAULT_CASES={'normal':3,'timeout':3,'output-budget':3,'parent-exit':3,'root-completed-cleanup':3,'root-nonzero':3,'root-mode-timeout':3,'root-mode-output-budget':3,'invalid-completion-mode':0,'invalid-env':0,'null-args':0,'nul-arg':0,'same-logs':0,'concurrent':3,'injected-cleanup-uncertainty':3}
COMPILE_CASES={'compile-policy-natural-tree-cleanup':3,'compile-policy-output-hard-cap':3,'compile-policy-timeout-cleanup':3,'compile-policy-argv-rejection':0,'compile-policy-root-mode-rejection':0,'compile-policy-deadline-rejection':0,'unknown-policy-rejection':0,'compile-policy-rustup-selector-rejection':0}

NATIVE_SOURCE_FILES=('native/.github/workflows/ci.yml', 'native/Default15/Fixture/Fixture.csproj', 'native/Default15/Fixture/Program.cs', 'native/Default15/Harness/Harness.csproj', 'native/Default15/Harness/Program.cs', 'native/Default15/Harness/windows-owned-process-fault-fixture.cs', 'native/Default15/Harness/windows-owned-process.cs', 'native/Fixture/Fixture.csproj', 'native/Fixture/Program.cs', 'native/Harness/Harness.csproj', 'native/Harness/Program.cs', 'native/Harness/short-clock-owned-process.cs', 'native/Harness/windows-owned-process.cs', 'native/NuGet.Config', 'native/global.json', 'native/run-combined.ps1', 'native/source-hashes.json')

def relative_key(path,root):
    return path.relative_to(root).as_posix()

def native_source_hashes(root):
    # Fixed source inventory, never transient build outputs (bin/obj).
    return {relative_key(root/name,root):hashlib.sha256((root/name).read_bytes()).hexdigest() for name in NATIVE_SOURCE_FILES}

def qualification_ok(q):
    # Controller verifies official run/job/artifact identity before supplying this
    # record; a caller-written local JSON is not independent native evidence.
    root=Path(__file__).parent
    current=hashlib.sha256((root/'windows-owned-process.cs').read_bytes()).hexdigest()
    require(q.get('policy')==POLICY and q.get('native_passed') is True,'new native qualification required')
    require(q.get('transport_sha256')==current,'qualification transport mismatch')
    require(isinstance(q.get('control_sha'),str) and re.fullmatch('[0-9a-f]{40}',q['control_sha']) is not None,'qualification control identity')
    require(type(q.get('run_id')) is int and q['run_id']>0,'qualification run absent')
    require(q.get('verified_by_controller') is True,'qualification unverified')
    source_hashes=native_source_hashes(root)
    require(q.get('source_hashes')==source_hashes,'fixture source identity mismatch')
    require(q.get('short_clock_sha256')==source_hashes['native/Harness/short-clock-owned-process.cs'],'injection source mismatch')
    require(q.get('full45MinuteElapsedValidation') is False,'elapsed claim mismatch')
    for name,expected in [('default15',DEFAULT_CASES),('compile8',COMPILE_CASES)]:
        suite=q.get('suites',{}).get(name,{})
        require(suite.get('run_id')==q['run_id'] and suite.get('control_sha')==q['control_sha'] and suite.get('transport_sha256')==current,'suite provenance mismatch')
        cases=suite.get('cases',{})
        require(set(cases)==set(expected),'qualification coverage absent')
        for case,count in expected.items():
            item=cases[case]
            require(item.get('passed') is True and type(item.get('actualExecutions')) is int and item['actualExecutions']==count,'native execution evidence absent')
            require(item.get('treeEmpty') is True if count else item.get('expectedRejection') is True,'native cleanup/rejection evidence absent')
    timeout=q['suites']['compile8']['cases']['compile-policy-timeout-cleanup']
    require(timeout.get('injectedClock') is True and timeout.get('injectedMilliseconds')==8000 and timeout.get('productionRequestedSeconds')==2700,'injected timeout provenance')

def compile_only(request,transport,qualification,check_identity,artifact_parser,write_receipt):
    """Inject a separately qualified adapter; always verify final identity.

    No automatic qualification creation: current 15-case smoke receipt is rejected.
    Parser reuses native-key-import.artifact_from_log with SOURCE/OUT fixed by host.
    No produced binary is invoked by this orchestration.
    """
    state={'state':'not-started','product_runtime_attempted':False,'compile_attempted':False,'final_identity_verified':False}
    try:
        qualification_ok(qualification)
        require(request['commandPolicy']==POLICY and request['completionMode']=='natural-tree' and request['seconds']==SECONDS and request['args']==ARGS,'request mismatch')
        check_identity();state['compile_attempted']=True
        receipt,output=transport(request)
        receipt_ok(receipt)
        require(isinstance(output,str) and len(output.encode('utf-8'))<=CAP,'bounded compiler metadata')
        state['binary']=artifact_parser(output)
        state['state']='compiled-not-executed'
    except BaseException:
        state['state']='failed'
        raise
    finally:
        try: check_identity();state['final_identity_verified']=True
        except BaseException:
            state['state']='identity-failed'
            raise
        finally: write_receipt(state)
    return state

def compile_environment(validated_toolchain_env,output):
    """Input is a path/hash-selected toolchain, never ambient os.environ; not signer attestation.

    Python symlink checks are not complete Windows junction/reparse protection;
    the future Host must inspect each ancestor ReparsePoint before use.
    Copy only the existing smoke SDK allowlist. Do not borrow real user HOME/cache.
    Dependency fetch must populate this isolated Cargo home in a separate stage.
    """
    output=Path(output)
    require(output.is_absolute() and output.is_dir() and not any(p.is_symlink() for p in (output,*output.parents)),'output directory')
    allowed=set('PATH SYSTEMROOT WINDIR COMSPEC PATHEXT OS NUMBER_OF_PROCESSORS PROCESSOR_ARCHITECTURE PROCESSOR_IDENTIFIER PROGRAMFILES PROGRAMFILES(X86) PROGRAMW6432 COMMONPROGRAMFILES COMMONPROGRAMFILES(X86) INCLUDE LIB LIBPATH VCINSTALLDIR VCTOOLSINSTALLDIR VCTOOLSVERSION VSINSTALLDIR VISUALSTUDIOVERSION WINDOWSSDKDIR WINDOWSSDKVERSION WINDOWSSDKLIBVERSION UNIVERSALCRTSDKDIR UCRTVERSION VSCMD_ARG_HOST_ARCH VSCMD_ARG_TGT_ARCH LANG LC_ALL TZ'.split())
    env={}
    for key,value in validated_toolchain_env.items():
        upper=key.upper()
        if upper not in allowed:continue
        require(upper not in env and isinstance(value,str) and '\0' not in value,'ambiguous toolchain environment')
        env[upper]=value
    for key,part in [('HOME','home'),('USERPROFILE','home'),('APPDATA','appdata'),('LOCALAPPDATA','localappdata'),('XDG_CONFIG_HOME','config'),('XDG_DATA_HOME','data'),('XDG_CACHE_HOME','cache'),('XDG_RUNTIME_DIR','runtime'),('XENTERM_DATA_DIR','profile'),('TEMP','tmp'),('TMP','tmp'),('TMPDIR','tmp'),('CARGO_HOME','cargo-home'),('RUSTUP_HOME','rustup-home'),('CARGO_TARGET_DIR','target')]:
        directory=output/part
        require(not directory.is_symlink(),'sandbox symlink')
        directory.mkdir(exist_ok=True)
        env[key]=str(directory)
    env['RUSTC']=str(output/'rustup-home/toolchains/1.99.1-x86_64-pc-windows-msvc/bin/rustc.exe')
    env.update(CARGO_NET_OFFLINE='true',CARGO_BUILD_JOBS='2',CARGO_INCREMENTAL='0',CARGO_PROFILE_DEV_DEBUG='0',CARGO_PROFILE_TEST_DEBUG='0',XT_VERSION_JSON=str(output/'version.json'),GITHUB_SHA=MAIN,GITHUB_REF_TYPE='branch',GITHUB_REF_NAME='validation/key-import-compile-only-20261030')
    return env
