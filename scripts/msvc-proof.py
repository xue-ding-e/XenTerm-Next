"""Installed MSVC bootstrap, process-local allowlist, synthetic link proof only."""
import importlib.util,json,os,re,subprocess,sys,hashlib,shutil
from pathlib import Path
HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('diag',HERE/'diagnose-native.py');d=importlib.util.module_from_spec(spec);spec.loader.exec_module(d);n=d.n
SDK_KEYS={'PATH','INCLUDE','LIB','LIBPATH','VCTOOLSINSTALLDIR','VCINSTALLDIR','VCTOOLSVERSION','VSINSTALLDIR','VISUALSTUDIOVERSION','WINDOWSSDKDIR','WINDOWSSDKVERSION','WINDOWSSDKLIBVERSION','UNIVERSALCRTSDKDIR','UCRTVERSION','VSCMD_ARG_HOST_ARCH','VSCMD_ARG_TGT_ARCH'}
def capture_env():
    # Explicit key selection only; never use `set` or dump the whole environment.
    print(json.dumps({k.upper():v for k,v in os.environ.items() if k.upper() in SDK_KEYS}))
def safe_cmd_path(path):
    value=str(path)
    n.require(path.is_absolute() and not any(c in value for c in '%!^&|<>"\r\n'),'unsafe shell path')
    n.require(path.exists() and not path.is_symlink(),'missing or symlink bootstrap path')
    return value

def parse_sdk_output(raw):
    n.require(len(raw)<=65536,'SDK export too large');values=json.loads(raw)
    n.require(isinstance(values,dict) and values and set(values)<=SDK_KEYS,'SDK keys invalid')
    n.require(all(isinstance(v,str) and len(v)<=32768 and '\n' not in v and '\r' not in v for v in values.values()),'SDK value invalid')
    for k in ('PATH','INCLUDE','LIB','VCTOOLSINSTALLDIR','WINDOWSSDKDIR'):n.require(bool(values.get(k)),'SDK field absent')
    return values

def validate_sdk(values,root):
    tool=Path(values['VCTOOLSINSTALLDIR']).resolve();sdk=Path(values['WINDOWSSDKDIR']).resolve()
    n.require(tool.is_relative_to(root/'VC'/'Tools'/'MSVC') and tool.is_dir(),'toolset outside chosen VS')
    n.require(sdk.is_dir(),'SDK missing')
    expected={name:(tool/'bin'/'Hostx64'/'x64'/name).resolve() for name in ('cl.exe','link.exe')}
    for name,path in expected.items():
        n.require(path.is_file() and not path.is_symlink(),'tool missing')
        found=shutil.which(name,path=values['PATH']);n.require(found is not None and Path(found).resolve()==path,'wrong tool precedence')
    for key in ('INCLUDE','LIB'):
        paths=[Path(x).resolve() for x in values[key].split(';') if x]
        n.require(paths and all(p.is_dir() for p in paths),'SDK directory absent')
        n.require(all(p.is_relative_to(root) or p.is_relative_to(sdk) for p in paths),'SDK directories outside VS/SDK')
    return expected,sdk

def bootstrap():
    env=n.compile_env();env['PYTHONUTF8']='1';fold={k.upper():v for k,v in env.items()}
    n.require('PROGRAMFILES(X86)' in fold and 'SYSTEMROOT' in fold,'OS tool roots absent')
    vswhere=Path(fold['PROGRAMFILES(X86)'])/'Microsoft Visual Studio'/'Installer'/'vswhere.exe'
    cmd=Path(fold['SYSTEMROOT'])/'System32'/'cmd.exe'
    for path in (vswhere,cmd,Path(sys.executable),HERE/'msvc-proof.py'):safe_cmd_path(path)
    result=subprocess.run([str(vswhere),'-latest','-products','*','-requires','Microsoft.VisualStudio.Component.VC.Tools.x86.x64','-property','installationPath'],env=env,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=10,check=False)
    n.require(result.returncode==0 and len(result.stdout)<=4096,'vswhere failed')
    rows=result.stdout.decode('utf-8-sig').splitlines();n.require(len(rows)==1,'ambiguous VS root');root=Path(rows[0]).resolve();safe_cmd_path(root)
    script=root/'Common7'/'Tools'/'VsDevCmd.bat';safe_cmd_path(script)
    command=f'call "{script}" -arch=x64 -host_arch=x64 >nul 2>nul && "{sys.executable}" -B "{HERE / "msvc-proof.py"}" --capture-env'
    result=subprocess.run([str(cmd),'/d','/s','/c',command],env=env,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=30,check=False)
    n.require(result.returncode==0,'SDK bootstrap failed');values=parse_sdk_output(result.stdout.decode('utf-8-sig'));expected,sdk=validate_sdk(values,root)
    # Only this isolated child-environment dictionary is amended. No GITHUB_ENV,
    # global PATH, machine registry, install, permissions or runtime env mutation.
    env.update(values)
    metadata={'vswhere_sha256':n.digest(vswhere),'vs_root_sha256':d.sha(str(root)),'bootstrap_sha256':n.digest(script),'sdk_root_sha256':d.sha(str(sdk)),'tools':{k:{'path_sha256':d.sha(str(v)),'file_sha256':n.digest(v),'family':'visual-studio-vc-tools'} for k,v in expected.items()},'sdk_fields':{k:{'value_sha256':d.sha(v),'present':bool(v)} for k,v in values.items()},'scope':'ephemeral SDK overlay; no credentials exported; discovered cl/link same installed x64 toolset'}
    powershell=Path(fold['SYSTEMROOT'])/'System32'/'WindowsPowerShell'/'v1.0'/'powershell.exe';safe_cmd_path(powershell)
    result=subprocess.run([str(powershell),'-NoProfile','-NonInteractive','-File',str(HERE/'tool-signatures.ps1'),'-ClPath',str(expected['cl.exe']),'-LinkPath',str(expected['link.exe'])],env=env,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=20,check=False)
    n.require(result.returncode==0 and len(result.stdout)<4096,'tool signer verification failed')
    signatures=json.loads(result.stdout.decode('utf-8-sig'));n.require(set(signatures)=={'cl','link'},'signature schema')
    for row in signatures.values():
        n.require(isinstance(row,dict) and set(row)=={'valid_signature','microsoft_signer','microsoft_company','version'},'signature fields')
        n.require(row.get('valid_signature') is True and row.get('microsoft_signer') is True and row.get('microsoft_company') is True,'not verified Microsoft tools')
        n.require(isinstance(row['version'],list) and len(row['version'])==4 and all(type(v) is int and 0<=v<=65535 for v in row['version']),'invalid tool version')
    metadata['signatures']=signatures;n.write('msvc-environment.json',metadata)
    return env,expected['link.exe']

def proof():
    n.require(n.platform_target()=='x86_64-pc-windows-msvc','Windows-only proof')
    env,linker=bootstrap();root=n.OUT/'msvc-link-proof';root.mkdir(mode=0o700);source=root/'probe.rs';source.write_bytes(d.SMOKE_SOURCE);binary=root/'probe.exe'
    rustc=shutil.which('rustc',path=env['PATH']);n.require(rustc is not None,'rustc absent')
    result={'source_sha256':hashlib.sha256(d.SMOKE_SOURCE).hexdigest(),'executed_binary':False,'product_compile_attempted':False,'selected_linker_path_sha256':d.sha(str(linker)),'timeout_seconds':20}
    code=0
    try:
        n.run([rustc,'+1.99.1','--target','x86_64-pc-windows-msvc','--crate-name','synthetic_msvc_proof','--crate-type','bin','-C','linker='+str(linker),str(source),'-o',str(binary)],'msvc-link-proof',20,env)
        n.require(binary.is_file() and not binary.is_symlink(),'proof binary missing');result.update(state='linked-not-executed',binary_sha256=n.digest(binary))
    except BaseException as exc:code=1;result.update(state='failed-or-timeout',error_type=type(exc).__name__)
    finally:
        n.require(source.read_bytes()==d.SMOKE_SOURCE,'proof source changed')
        stderr=n.OUT/'msvc-link-proof.stderr.log';result['diagnostic']=d.diagnose('',stderr.read_text(errors='replace') if stderr.is_file() and stderr.stat().st_size<=64*1024*1024 else '')
        n.write('msvc-proof.json',result)
        n.identities();n.patch_hashes();n.require(n.git('write-tree')==n.I['TEST_TREE'],'tree changed');n.require(not n.git('diff','--exit-code'),'source changed')
        p=n.read('prepared.json');n.require(n.git('status','--porcelain','--untracked-files=all','--ignored')==p['status'],'status changed')
        for path,h in p['control_files'].items():n.require(n.digest(n.CONTROL/path)==h,'control changed')
        for path,h in n.I['FIXTURES'].items():n.require(n.digest(n.SOURCE/path)==h,'fixture changed')
    return code
if __name__=='__main__':
    try:
        if sys.argv[1:]==['--capture-env']:capture_env();code=0
        else:code=proof()
    except BaseException as exc:
        if n.OUT.is_dir():n.write('failure.json',{'stage':'msvc-environment-proof','error_type':type(exc).__name__,'classification':'control/environment failure; no product test','product_compile_attempted':False,'raw_logs_uploaded':False})
        code=1
    sys.exit(code)
