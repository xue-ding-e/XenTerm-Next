"""Installed MSVC bootstrap, process-local allowlist, synthetic link proof only."""
import importlib.util,json,os,re,subprocess,sys,hashlib,shutil
from pathlib import Path
HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('diag',HERE/'diagnose-native.py');d=importlib.util.module_from_spec(spec);spec.loader.exec_module(d);n=d.n
SDK_KEYS={'PATH','INCLUDE','LIB','LIBPATH','VCTOOLSINSTALLDIR','VCINSTALLDIR','VCTOOLSVERSION','VSINSTALLDIR','VISUALSTUDIOVERSION','WINDOWSSDKDIR','WINDOWSSDKVERSION','WINDOWSSDKLIBVERSION','UNIVERSALCRTSDKDIR','UCRTVERSION','VSCMD_ARG_HOST_ARCH','VSCMD_ARG_TGT_ARCH'}

REASONS={'OS tool roots absent': 'os-tool-roots-absent', 'SDK bootstrap failed': 'sdk-bootstrap-failed', 'SDK directories outside VS/SDK': 'sdk-directories-outside-vs-sdk', 'SDK directory absent': 'sdk-directory-absent', 'SDK export too large': 'sdk-export-too-large', 'SDK field absent': 'sdk-field-absent', 'SDK keys invalid': 'sdk-keys-invalid', 'SDK missing': 'sdk-missing', 'SDK value invalid': 'sdk-value-invalid', 'Windows-only proof': 'windows-only-proof', 'ambiguous VS root': 'ambiguous-vs-root', 'control changed': 'control-changed', 'fixture changed': 'fixture-changed', 'invalid tool version': 'invalid-tool-version', 'missing or symlink bootstrap path': 'missing-or-symlink-bootstrap-path', 'not verified Microsoft tools': 'not-verified-microsoft-tools', 'proof binary missing': 'proof-binary-missing', 'proof source changed': 'proof-source-changed', 'rustc absent': 'rustc-absent', 'signature fields': 'signature-fields', 'signature schema': 'signature-schema', 'source changed': 'source-changed', 'status changed': 'status-changed', 'tool missing': 'tool-missing', 'tool signer verification failed': 'tool-signer-verification-failed', 'toolset outside chosen VS': 'toolset-outside-chosen-vs', 'tree changed': 'tree-changed', 'unsafe shell path': 'unsafe-shell-path', 'vswhere failed': 'vswhere-failed', 'wrong tool precedence': 'wrong-tool-precedence'}
CHECKPOINTS=('start','compile-environment','os-roots','bootstrap-paths','vswhere-child','vswhere-result','vs-root','vsdevcmd-path','vc-child','sdk-json','sdk-validation','signer-path','signer-child','signer-json','signer-fields','write-metadata','final-identities','finished')
CURRENT='start'
CHILD_EXITS={}
class ControlGate(RuntimeError):
    def __init__(self,reason):self.reason=reason

def require(ok,message):
    if not ok:raise ControlGate(REASONS.get(message,'unknown-guard'))
def checkpoint(name):
    global CURRENT
    if name not in CHECKPOINTS:raise ControlGate('unknown-checkpoint')
    CURRENT=name

def metadata_only():
    global CURRENT,CHILD_EXITS
    CURRENT='start';CHILD_EXITS={};result={'smoke_attempted':False,'product_compile_attempted':False,'raw_logs_uploaded':False}
    try:
        bootstrap();checkpoint('final-identities')
        n.identities();n.patch_hashes()
        require(n.git('write-tree')==n.I['TEST_TREE'],'tree changed')
        require(not n.git('diff','--exit-code'),'source changed')
        prepared=n.read('prepared.json')
        require(n.git('status','--porcelain','--untracked-files=all','--ignored')==prepared['status'],'status changed')
        for path,h in prepared['control_files'].items():require(n.digest(n.CONTROL/path)==h,'control changed')
        for path,h in n.I['FIXTURES'].items():require(n.digest(n.SOURCE/path)==h,'fixture changed')
        checkpoint('finished');result.update(state='bootstrap-verified',reason=None);code=0
    except ControlGate as exc:result.update(state='guard-failed',reason=exc.reason);code=1
    except BaseException as exc:
        kinds={'TimeoutExpired':'child-timeout','JSONDecodeError':'invalid-json','UnicodeDecodeError':'invalid-encoding','FileNotFoundError':'file-not-found','PermissionError':'permission-denied','OSError':'os-error','TypeError':'invalid-type','ValueError':'invalid-value','KeyError':'missing-field'}
        result.update(state='bootstrap-error',reason=kinds.get(type(exc).__name__,'unknown-error'));code=1
    result.update(checkpoint=CURRENT,child_exits=CHILD_EXITS)
    n.write('bootstrap-status.json',result)
    return code

def capture_env():
    # Explicit key selection only; never use `set` or dump the whole environment.
    print(json.dumps({k.upper():v for k,v in os.environ.items() if k.upper() in SDK_KEYS}))
def safe_cmd_path(path):
    value=str(path)
    require(path.is_absolute() and not any(c in value for c in '%!^&|<>"\r\n'),'unsafe shell path')
    require(path.exists() and not path.is_symlink(),'missing or symlink bootstrap path')
    return value

def parse_sdk_output(raw):
    require(len(raw)<=65536,'SDK export too large');values=json.loads(raw)
    require(isinstance(values,dict) and values and set(values)<=SDK_KEYS,'SDK keys invalid')
    require(all(isinstance(v,str) and len(v)<=32768 and '\n' not in v and '\r' not in v for v in values.values()),'SDK value invalid')
    for k in ('PATH','INCLUDE','LIB','VCTOOLSINSTALLDIR','WINDOWSSDKDIR'):require(bool(values.get(k)),'SDK field absent')
    return values

def validate_sdk(values,root):
    tool=Path(values['VCTOOLSINSTALLDIR']).resolve();sdk=Path(values['WINDOWSSDKDIR']).resolve()
    require(tool.is_relative_to(root/'VC'/'Tools'/'MSVC') and tool.is_dir(),'toolset outside chosen VS')
    require(sdk.is_dir(),'SDK missing')
    expected={name:(tool/'bin'/'Hostx64'/'x64'/name).resolve() for name in ('cl.exe','link.exe')}
    for name,path in expected.items():
        require(path.is_file() and not path.is_symlink(),'tool missing')
        found=shutil.which(name,path=values['PATH']);require(found is not None and Path(found).resolve()==path,'wrong tool precedence')
    for key in ('INCLUDE','LIB'):
        paths=[Path(x).resolve() for x in values[key].split(';') if x]
        require(paths and all(p.is_dir() for p in paths),'SDK directory absent')
        require(all(p.is_relative_to(root) or p.is_relative_to(sdk) for p in paths),'SDK directories outside VS/SDK')
    return expected,sdk

def bootstrap():
    checkpoint('compile-environment')
    env=n.compile_env();env['PYTHONUTF8']='1';fold={k.upper():v for k,v in env.items()}
    checkpoint('os-roots')
    require('PROGRAMFILES(X86)' in fold and 'SYSTEMROOT' in fold,'OS tool roots absent')
    vswhere=Path(fold['PROGRAMFILES(X86)'])/'Microsoft Visual Studio'/'Installer'/'vswhere.exe'
    cmd=Path(fold['SYSTEMROOT'])/'System32'/'cmd.exe'
    checkpoint('bootstrap-paths')
    for path in (vswhere,cmd,Path(sys.executable),HERE/'msvc-proof.py'):safe_cmd_path(path)
    checkpoint('vswhere-child')
    result=subprocess.run([str(vswhere),'-latest','-products','*','-requires','Microsoft.VisualStudio.Component.VC.Tools.x86.x64','-property','installationPath'],env=env,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=10,check=False)
    CHILD_EXITS['vswhere']=result.returncode
    checkpoint('vswhere-result')
    require(result.returncode==0 and len(result.stdout)<=4096,'vswhere failed')
    checkpoint('vs-root')
    rows=result.stdout.decode('utf-8-sig').splitlines();require(len(rows)==1,'ambiguous VS root');root=Path(rows[0]).resolve();safe_cmd_path(root)
    checkpoint('vsdevcmd-path')
    script=root/'Common7'/'Tools'/'VsDevCmd.bat';safe_cmd_path(script)
    command=f'call "{script}" -arch=x64 -host_arch=x64 >nul 2>nul && "{sys.executable}" -B "{HERE / "msvc-proof.py"}" --capture-env'
    checkpoint('vc-child')
    result=subprocess.run([str(cmd),'/d','/s','/c',command],env=env,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=30,check=False)
    CHILD_EXITS['vc-bootstrap']=result.returncode
    require(result.returncode==0,'SDK bootstrap failed')
    checkpoint('sdk-json');values=parse_sdk_output(result.stdout.decode('utf-8-sig'))
    checkpoint('sdk-validation');expected,sdk=validate_sdk(values,root)
    # Only this isolated child-environment dictionary is amended. No GITHUB_ENV,
    # global PATH, machine registry, install, permissions or runtime env mutation.
    env.update(values)
    metadata={'vswhere_sha256':n.digest(vswhere),'vs_root_sha256':d.sha(str(root)),'bootstrap_sha256':n.digest(script),'sdk_root_sha256':d.sha(str(sdk)),'tools':{k:{'path_sha256':d.sha(str(v)),'file_sha256':n.digest(v),'family':'visual-studio-vc-tools'} for k,v in expected.items()},'sdk_fields':{k:{'value_sha256':d.sha(v),'present':bool(v)} for k,v in values.items()},'scope':'ephemeral SDK overlay; no credentials exported; discovered cl/link same installed x64 toolset'}
    checkpoint('signer-path')
    powershell=Path(fold['SYSTEMROOT'])/'System32'/'WindowsPowerShell'/'v1.0'/'powershell.exe';safe_cmd_path(powershell)
    checkpoint('signer-child')
    result=subprocess.run([str(powershell),'-NoProfile','-NonInteractive','-File',str(HERE/'tool-signatures.ps1'),'-ClPath',str(expected['cl.exe']),'-LinkPath',str(expected['link.exe'])],env=env,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=20,check=False)
    CHILD_EXITS['signatures']=result.returncode
    require(result.returncode==0 and len(result.stdout)<4096,'tool signer verification failed')
    checkpoint('signer-json')
    signatures=json.loads(result.stdout.decode('utf-8-sig'));require(set(signatures)=={'cl','link'},'signature schema')
    checkpoint('signer-fields')
    for row in signatures.values():
        require(isinstance(row,dict) and set(row)=={'valid_signature','microsoft_signer','microsoft_company','version'},'signature fields')
        require(row.get('valid_signature') is True and row.get('microsoft_signer') is True and row.get('microsoft_company') is True,'not verified Microsoft tools')
        require(isinstance(row['version'],list) and len(row['version'])==4 and all(type(v) is int and 0<=v<=65535 for v in row['version']),'invalid tool version')
    checkpoint('write-metadata')
    metadata['signatures']=signatures;n.write('msvc-environment.json',metadata)
    return env,expected['link.exe']

def proof():
    require(n.platform_target()=='x86_64-pc-windows-msvc','Windows-only proof')
    env,linker=bootstrap();root=n.OUT/'msvc-link-proof';root.mkdir(mode=0o700);source=root/'probe.rs';source.write_bytes(d.SMOKE_SOURCE);binary=root/'probe.exe'
    rustc=shutil.which('rustc',path=env['PATH']);require(rustc is not None,'rustc absent')
    result={'source_sha256':hashlib.sha256(d.SMOKE_SOURCE).hexdigest(),'executed_binary':False,'product_compile_attempted':False,'selected_linker_path_sha256':d.sha(str(linker)),'timeout_seconds':20}
    code=0
    try:
        n.run([rustc,'+1.99.1','--target','x86_64-pc-windows-msvc','--crate-name','synthetic_msvc_proof','--crate-type','bin','-C','linker='+str(linker),str(source),'-o',str(binary)],'msvc-link-proof',20,env)
        require(binary.is_file() and not binary.is_symlink(),'proof binary missing');result.update(state='linked-not-executed',binary_sha256=n.digest(binary))
    except BaseException as exc:code=1;result.update(state='failed-or-timeout',error_type=type(exc).__name__)
    finally:
        require(source.read_bytes()==d.SMOKE_SOURCE,'proof source changed')
        stderr=n.OUT/'msvc-link-proof.stderr.log';result['diagnostic']=d.diagnose('',stderr.read_text(errors='replace') if stderr.is_file() and stderr.stat().st_size<=64*1024*1024 else '')
        n.write('msvc-proof.json',result)
        n.identities();n.patch_hashes();require(n.git('write-tree')==n.I['TEST_TREE'],'tree changed');require(not n.git('diff','--exit-code'),'source changed')
        p=n.read('prepared.json');require(n.git('status','--porcelain','--untracked-files=all','--ignored')==p['status'],'status changed')
        for path,h in p['control_files'].items():require(n.digest(n.CONTROL/path)==h,'control changed')
        for path,h in n.I['FIXTURES'].items():require(n.digest(n.SOURCE/path)==h,'fixture changed')
    return code

# Read-only discovery probes. They never run the SDK batch or any compiler.
DISCOVERY_QUERIES={
    'original-property':['-latest','-products','*','-requires','Microsoft.VisualStudio.Component.VC.Tools.x86.x64','-property','installationPath'],
    'standard-component':['-latest','-products','*','-requires','Microsoft.VisualStudio.Component.VC.Tools.x86.x64','-format','json','-utf8'],
    'all-instances':['-all','-prerelease','-products','*','-format','json','-utf8'],
    'all-component':['-all','-prerelease','-products','*','-requires','Microsoft.VisualStudio.Component.VC.Tools.x86.x64','-format','json','-utf8'],
}
def discovery_inventory(raw):
    if len(raw)>65536:raise ControlGate('discovery-output-too-large')
    data=json.loads(raw.decode('utf-8-sig'))
    if not isinstance(data,list) or len(data)>32:raise ControlGate('discovery-schema-invalid')
    items=[]
    for item in data:
        if not isinstance(item,dict):raise ControlGate('discovery-schema-invalid')
        # Unknown fields are intentionally ignored, never echoed or forwarded.
        version=item.get('installationVersion')
        if not isinstance(version,str) or not re.fullmatch(r'[0-9]{1,6}(?:\.[0-9]{1,6}){1,3}',version):raise ControlGate('discovery-version-invalid')
        record={'version':[int(v) for v in version.split('.')]}
        for key in ('isComplete','isLaunchable','isPrerelease'):
            value=item.get(key)
            if value is not None and type(value) is not bool:raise ControlGate('discovery-state-invalid')
            record[key]=value
        state=item.get('state')
        if state is not None and (type(state) is not int or not 0<=state<=4294967295):raise ControlGate('discovery-state-invalid')
        record['state']=state;items.append(record)
    return {'count':len(items),'instances':items}

def discovery_parse(name,raw):
    if name not in DISCOVERY_QUERIES:raise ControlGate('discovery-query-invalid')
    if not isinstance(raw,bytes) or len(raw)>65536:raise ControlGate('discovery-output-too-large')
    result={'bytes':len(raw),'sha256':hashlib.sha256(raw).hexdigest()}
    if name=='original-property':
        # Byte split works without assuming the old default Windows encoding.
        lines=raw.splitlines();result.update(lines=len(lines),nonempty_lines=sum(bool(x.strip()) for x in lines))
    else:result.update(discovery_inventory(raw))
    return result

def discovery_final_identities():
    n.identities();n.patch_hashes()
    require(n.git('write-tree')==n.I['TEST_TREE'],'tree changed')
    require(not n.git('diff','--exit-code'),'source changed')
    prepared=n.read('prepared.json')
    require(n.git('status','--porcelain','--untracked-files=all','--ignored')==prepared['status'],'status changed')
    for path,h in prepared['control_files'].items():require(n.digest(n.CONTROL/path)==h,'control changed')
    for path,h in n.I['FIXTURES'].items():require(n.digest(n.SOURCE/path)==h,'fixture changed')

def discovery_only():
    result={'smoke_attempted':False,'product_compile_attempted':False,'sdk_batch_attempted':False,'raw_logs_uploaded':False,'queries':{},'state':'incomplete'}
    stage='environment'
    try:
        env=n.compile_env();fold={k.upper():v for k,v in env.items()}
        parent={k.upper() for k in os.environ}
        result['environment_presence']={key:{'parent':key in parent,'isolated':key in fold} for key in ('PROGRAMDATA','ALLUSERSPROFILE')}
        stage='tool-path'
        require('PROGRAMFILES(X86)' in fold,'OS tool roots absent')
        vswhere=Path(fold['PROGRAMFILES(X86)'])/'Microsoft Visual Studio'/'Installer'/'vswhere.exe';safe_cmd_path(vswhere)
        result['vswhere_sha256']=n.digest(vswhere)
        failed=False
        for name,args in DISCOVERY_QUERIES.items():
            stage=name
            try:
                child=subprocess.run([str(vswhere),*args],env=env,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=10,check=False)
                result['queries'][name]={'exit':child.returncode}
                if child.returncode!=0:raise ControlGate('discovery-child-nonzero')
                result['queries'][name].update(discovery_parse(name,child.stdout))
            except ControlGate as exc:result['queries'].setdefault(name,{})['reason']=exc.reason;failed=True
            except BaseException as exc:
                kinds={'TimeoutExpired':'child-timeout','JSONDecodeError':'invalid-json','UnicodeDecodeError':'invalid-encoding','FileNotFoundError':'file-not-found','PermissionError':'permission-denied','OSError':'os-error'}
                result['queries'].setdefault(name,{})['reason']=kinds.get(type(exc).__name__,'unknown-error');failed=True
        stage='final-identities';discovery_final_identities()
        result.update(state='discovery-failed' if failed else 'discovery-recorded',reason='query-failed' if failed else None);code=1 if failed else 0
    except ControlGate as exc:result.update(state='discovery-failed',reason=exc.reason);code=1
    except BaseException as exc:
        kinds={'TimeoutExpired':'child-timeout','JSONDecodeError':'invalid-json','UnicodeDecodeError':'invalid-encoding','FileNotFoundError':'file-not-found','PermissionError':'permission-denied','OSError':'os-error','TypeError':'invalid-type','ValueError':'invalid-value','KeyError':'missing-field'}
        result.update(state='discovery-error',reason=kinds.get(type(exc).__name__,'unknown-error'));code=1
    result['stage']=stage;n.write('vs-discovery.json',result)
    return code

# Paired metadata experiment only; no permanent build-environment change.
def programdata_overlay(env):
    fold={k.upper():v for k,v in env.items()}
    if 'SYSTEMROOT' not in fold:raise ControlGate('ab-systemroot-missing')
    system=Path(fold['SYSTEMROOT'])
    if not system.is_absolute() or system.name.casefold()!='windows':raise ControlGate('ab-systemroot-invalid')
    known=system.parent/'ProgramData'
    # Read exactly two named directory variables, never copy ambient values.
    selected={k.upper():os.environ[k] for k in os.environ if k.upper() in ('PROGRAMDATA','ALLUSERSPROFILE')}
    if set(selected)!= {'PROGRAMDATA','ALLUSERSPROFILE'}:raise ControlGate('ab-directory-variable-missing')
    for value in selected.values():
        if not isinstance(value,str):raise ControlGate('ab-directory-invalid')
        if '\x00' in value:raise ControlGate('ab-directory-invalid')
        path=Path(value);safe_cmd_path(path)
        for part in (path,*path.parents):
            if not part.is_dir() or part.is_symlink() or getattr(part.lstat(),'st_file_attributes',0)&0x400:raise ControlGate('ab-directory-reparse-or-invalid')
        if path.resolve()!=known.resolve():raise ControlGate('ab-directory-not-known-root')
    return {**env,**selected}

def paired_discovery_only():
    result={'smoke_attempted':False,'product_compile_attempted':False,'sdk_batch_attempted':False,'raw_logs_uploaded':False,'state':'incomplete','queries':{}}
    stage='environment'
    try:
        env=n.compile_env();other=programdata_overlay(env)
        fold={k.upper():v for k,v in env.items()}
        if 'PROGRAMFILES(X86)' not in fold:raise ControlGate('ab-tool-root-missing')
        stage='tool-path';vswhere=Path(fold['PROGRAMFILES(X86)'])/'Microsoft Visual Studio'/'Installer'/'vswhere.exe';safe_cmd_path(vswhere)
        result['vswhere_sha256']=n.digest(vswhere);failed=False
        for name,childenv in (('baseline',env),('directory-overlay',other)):
            stage=name
            try:
                child=subprocess.run([str(vswhere),*DISCOVERY_QUERIES['standard-component']],env=childenv,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=10,check=False)
                result['queries'][name]={'exit':child.returncode}
                if child.returncode!=0:raise ControlGate('discovery-child-nonzero')
                result['queries'][name].update(discovery_parse('standard-component',child.stdout))
            except ControlGate as exc:result['queries'].setdefault(name,{})['reason']=exc.reason;failed=True
            except BaseException as exc:
                kinds={'TimeoutExpired':'child-timeout','JSONDecodeError':'invalid-json','UnicodeDecodeError':'invalid-encoding','FileNotFoundError':'file-not-found','PermissionError':'permission-denied','OSError':'os-error'}
                result['queries'].setdefault(name,{})['reason']=kinds.get(type(exc).__name__,'unknown-error');failed=True
        stage='final-identities';discovery_final_identities()
        result.update(state='paired-discovery-failed' if failed else 'paired-discovery-recorded',reason='query-failed' if failed else None);code=1 if failed else 0
    except ControlGate as exc:result.update(state='paired-discovery-failed',reason=exc.reason);code=1
    except BaseException as exc:
        kinds={'TimeoutExpired':'child-timeout','JSONDecodeError':'invalid-json','UnicodeDecodeError':'invalid-encoding','FileNotFoundError':'file-not-found','PermissionError':'permission-denied','OSError':'os-error','TypeError':'invalid-type','ValueError':'invalid-value','KeyError':'missing-field'}
        result.update(state='paired-discovery-error',reason=kinds.get(type(exc).__name__,'unknown-error'));code=1
    result['stage']=stage;n.write('vs-paired-discovery.json',result);return code

if __name__=='__main__':
    try:
        if sys.argv[1:]==['--capture-env']:capture_env();code=0
        else:code=paired_discovery_only()
    except BaseException as exc:
        if n.OUT.is_dir():n.write('failure.json',{'stage':'msvc-environment-proof','error_type':type(exc).__name__,'classification':'control/environment failure; no product test','product_compile_attempted':False,'raw_logs_uploaded':False})
        code=1
    sys.exit(code)
