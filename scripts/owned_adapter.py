"""Windows smoke-only adapter; no product execution is authorized by this module.

The exact C# transport passed native fixture run 41056633320 at 8d81aca5.
A killed/stalled Host has UNKNOWN descendant lifecycle, never verified cleanup.
That poisons this Python owner and ends the proof; the disposable CI step/job
is the final boundary. No outer-Host kill is claimed to prove tree cleanup.
"""
import hashlib,json,os,re,subprocess,time
from pathlib import Path
from types import SimpleNamespace

TRANSPORT_SHA256='3d5a75d1baa1c90a843f26cb47a5a5a7c9328fbc73eb992326495cf23d6aaaef'
HERE=Path(__file__).resolve().parent
ARTIFACTS={'OwnedAdapter.exe','OwnedAdapter.dll','OwnedAdapter.deps.json','OwnedAdapter.runtimeconfig.json'}
ENV_KEYS=set('PATH SYSTEMROOT WINDIR COMSPEC PATHEXT OS NUMBER_OF_PROCESSORS PROCESSOR_ARCHITECTURE PROCESSOR_IDENTIFIER PROGRAMFILES PROGRAMFILES(X86) PROGRAMW6432 COMMONPROGRAMFILES COMMONPROGRAMFILES(X86) INCLUDE LIB LIBPATH VCINSTALLDIR VCTOOLSINSTALLDIR VCTOOLSVERSION VSINSTALLDIR VISUALSTUDIOVERSION WINDOWSSDKDIR WINDOWSSDKVERSION WINDOWSSDKLIBVERSION UNIVERSALCRTSDKDIR UCRTVERSION VSCMD_ARG_HOST_ARCH VSCMD_ARG_TGT_ARCH CARGO_HOME RUSTUP_HOME LANG LC_ALL TZ HOME USERPROFILE APPDATA LOCALAPPDATA XDG_CONFIG_HOME XDG_DATA_HOME XDG_CACHE_HOME XDG_RUNTIME_DIR XENTERM_DATA_DIR TEMP TMP TMPDIR RUST_BACKTRACE PROGRAMDATA ALLUSERSPROFILE PYTHONUTF8 CARGO_TARGET_DIR'.split())
POISONED=False
RECORDS=[]
class OwnedFailure(RuntimeError):
    pass

def check(value,code):
    if not value:raise OwnedFailure(code)
def digest(path):return hashlib.sha256(Path(path).read_bytes()).hexdigest()
def checked_json(path,limit):
    path=Path(path);check(path.is_file() and not path.is_symlink() and path.stat().st_size<=limit,'bounded-json-missing-or-invalid')
    def unique(pairs):
        data={}
        for key,value in pairs:
            check(key not in data,'duplicate-json-key');data[key]=value
        return data
    return json.loads(path.read_text(encoding='utf-8-sig'),object_pairs_hook=unique)
def validate_request(argv,seconds,env,cwd):
    check(type(seconds) is int and 1<=seconds<=30,'invalid-child-deadline')
    check(isinstance(argv,list) and argv and all(isinstance(s,str) and '\0' not in s for s in argv),'invalid-argv')
    check(Path(argv[0]).is_absolute() and Path(argv[0]).is_file(),'invalid-executable')
    check(Path(cwd).is_absolute() and Path(cwd).is_dir(),'invalid-cwd')
    check(isinstance(env,dict),'invalid-env');fold={}
    for key,value in env.items():
        check(isinstance(key,str) and key.upper() in ENV_KEYS and key.upper() not in fold and isinstance(value,str) and '\0' not in value,'invalid-explicit-env')
        fold[key.upper()]=value
    return fold

def identity(out):
    check(digest(HERE/'owned_adapter/windows-owned-process.cs')==TRANSPORT_SHA256,'transport-source-mismatch')
    expected=os.environ.get('EXPECTED_CONTROL_SHA','');check(re.fullmatch('[0-9a-f]{40}',expected) is not None,'invalid-control-sha')
    build=checked_json(Path(out)/'owned-adapter-build.json',16384)
    check(build.get('passed') is True and build.get('state')=='compiled' and build.get('transport_sha256')==TRANSPORT_SHA256 and build.get('control_sha')==expected,'adapter-build-not-qualified')
    info=checked_json(Path(out)/'owned-adapter-local.json',16384)
    check(info.get('transport_sha256')==TRANSPORT_SHA256 and info.get('control_sha')==expected,'adapter-provenance-mismatch')
    base=Path(info['directory']);check(base.is_absolute() and base==base.resolve() and base.is_relative_to(Path(out).resolve()),'adapter-directory-outside-control')
    check(set(info['artifacts'])==ARTIFACTS,'adapter-artifact-set')
    for name,expected in info['artifacts'].items():
        path=base/name
        check(path.is_file() and not any(p.is_symlink() for p in (path,*path.parents)) and digest(path)==expected,'adapter-binary-mismatch')
    return base/'OwnedAdapter.exe',info

def validate_receipt(data):
    check(isinstance(data,dict) and type(data.get('schema')) is int and data.get('schema')==1 and data.get('state')=='returned','host-receipt-invalid')
    check(data.get('productExecuted') is False,'invalid-scope')
    for key in ('treeEmpty','captureFailed','safeToStart','timedOut','outputExceeded','succeeded'):
        check(type(data.get(key)) is bool,'invalid-host-flag')
    code=data.get('exitCode');check(type(code) is int and 0<=code<=4294967295,'unknown-final-exit')
    check(data['treeEmpty'] and data['safeToStart'] and not data['captureFailed'],'cleanup-not-verified')
    check(data['succeeded']==(code==0 and not data['timedOut'] and not data['outputExceeded']),'inconsistent-success-flag')
    check(type(data.get('boundaryRootSignaled')) is bool and type(data.get('boundaryRootExitCodeKnown')) is bool,'invalid-boundary-flags')
    active=data.get('boundaryActiveProcessCount');check(type(active) is int and 0<=active<=4294967295,'invalid-boundary-count')
    known=data['boundaryRootExitCodeKnown'];boundary=data.get('boundaryRootExitCode')
    check(known==data['boundaryRootSignaled'] and ((type(boundary) is int and 0<=boundary<=4294967295) if known else boundary is None),'invalid-boundary-exit')
    return data

def run(argv,label,seconds,env,cwd,out):
    global POISONED
    check(not POISONED,'owner-poisoned')
    check(re.fullmatch('[a-z0-9-]{1,48}',label) is not None,'invalid-label')
    out=Path(out).resolve();fold=validate_request(argv,seconds,env,cwd)
    host,before=identity(out);start=time.monotonic()
    request=out/(label+'.request.json');receipt=out/(label+'.host.json')
    stdout=out/(label+'.stdout.log');stderr=out/(label+'.stderr.log')
    check(not any(p.exists() for p in (request,receipt,stdout,stderr)),'command-evidence-exists')
    payload=json.dumps(dict(executable=argv[0],args=argv[1:],cwd=str(cwd),seconds=seconds,stdout=str(stdout),stderr=str(stderr),environment=fold)).encode('utf-8')
    check(len(payload)<=131072,'request-too-large')
    with request.open('xb') as file:file.write(payload)
    record={'label':label,'child_deadline_seconds':seconds,'host_deadline_seconds':seconds+15,'transport_sha256':TRANSPORT_SHA256,'state':'unknown','tree_empty':False,'host_lifecycle':'unknown','product_compile_attempted':False}
    POISONED=True  # Restored only by a complete, checked Host/child cleanup receipt.
    try:
        completed=subprocess.run([str(host),str(request),str(receipt)],cwd=str(host.parent),env=fold,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=seconds+15,check=False)
        check(completed.returncode==0,'host-did-not-complete')
        data=validate_receipt(checked_json(receipt,16384))
        check(identity(out)==(host,before),'adapter-identity-changed')
        sizes={p.name:p.stat().st_size for p in (stdout,stderr)}
        check(sum(sizes.values())<=1048576,'retained-output-budget-exceeded')
        record.update(state='returned',tree_empty=True,host_lifecycle='returned-verified',exit_code=data['exitCode'],timed_out=data['timedOut'],output_exceeded=data['outputExceeded'],capture_failed=data['captureFailed'],logs={stream:{'bytes':path.stat().st_size,'sha256':digest(path)} for stream,path in [('stdout',stdout),('stderr',stderr)]})
        record['before_cleanup']={key:data[key] for key in ('boundaryRootSignaled','boundaryActiveProcessCount','boundaryRootExitCodeKnown','boundaryRootExitCode')}
        POISONED=False
        check(not data['timedOut'] and not data['outputExceeded'],'child-budget-failed')
        return SimpleNamespace(returncode=data['exitCode'],stdout=stdout.read_bytes())
    except subprocess.TimeoutExpired:
        record.update(state='unknown-host-lifecycle',reason='outer-host-timeout');raise OwnedFailure('unknown-host-lifecycle') from None
    except OwnedFailure as error:
        record['reason']=str(error);raise
    except BaseException as error:
        record['reason']=type(error).__name__;raise OwnedFailure('adapter-error') from None
    finally:
        record['duration_seconds']=round(time.monotonic()-start,3);record['owner_poisoned']=POISONED
        RECORDS.append(record)
        (out/'owned-commands.json').write_text(json.dumps(RECORDS,indent=2)+'\n',encoding='utf-8')

def smoke_receipts():
    check(not POISONED,'owner-poisoned')
    check([r['label'] for r in RECORDS]==['vswhere','vc-bootstrap','signatures','msvc-link-proof'],'smoke-command-inventory')
    check(all(r.get('state')=='returned' and r.get('tree_empty') is True and r.get('host_lifecycle')=='returned-verified' and r.get('exit_code')==0 and r.get('timed_out') is False and r.get('output_exceeded') is False and r.get('capture_failed') is False for r in RECORDS),'smoke-command-incomplete')
    return {'command_count':4,'all_child_trees_verified':True,'transport_sha256':TRANSPORT_SHA256,'qualification_run':41056633320,'outer_host_failure_cleanup':'unknown; disposable CI final boundary'}
