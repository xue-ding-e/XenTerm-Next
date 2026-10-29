"""Bounded compile-only diagnostic; never export raw compiler messages or snippets."""
import importlib.util,json,re,hashlib,subprocess,shutil,sys,os
from pathlib import Path
HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('native',HERE/'native-key-import.py');n=importlib.util.module_from_spec(spec);spec.loader.exec_module(n)
# Exact known source locations only. Registry/build-script paths are deliberately
# not exported: unknown paths get no location, only error code/category counts.
REPO_FILES={'src/main.rs','src/ssh/impls/key_import.rs','src/ssh/impls/key_import_bounds.rs','src/ssh/impls/ppk.rs','src/ssh/impls/ssh.rs','src/ssh/mod.rs','src/ui/impls/session_editor.rs','src/ui/impls/session_editor_button_tests.rs','src/ui/impls/session_editor_key_import.rs','build.rs'}
CATEGORIES={
 'missing-linker':('linker `link.exe` not found','linker `cc` not found','linker `clang` not found','link.exe: not found'),
 'missing-sdk':('windows sdk not found','could not find windows sdk','cannot find windows sdk','unable to find libclang','could not find a valid visual studio'),
 'custom-build-failed':('failed to run custom build command',),
 'dependency-download':('failed to download','failed to get `','failed to fetch'),
 'tls':('certificate verify failed','ssl certificate problem','tls handshake'),
 'dns':('could not resolve host','failed to lookup address information'),
 'access-denied':('access is denied','permission denied'),
 'gnu-linker-signature':('extra operand',"try 'link --help'",'gnu coreutils'),
 'link-failed':('linking with `link.exe` failed','linking with `cc` failed','linking with `clang` failed'),
}
def sha(value):return hashlib.sha256(value.encode()).hexdigest()
def safe_location(span):
    filename=span.get('file_name')
    if not isinstance(filename,str):return None
    path=filename.replace('\\','/')
    source=n.SOURCE.as_posix()+'/'
    if path.startswith(source):path=path[len(source):]
    if path not in REPO_FILES:return None
    if not all(type(span.get(k)) is int and 0<span[k]<10000000 for k in ('line_start','column_start')):return None
    return {'file':path,'line':span['line_start'],'column':span['column_start']}
def fixed_signals(text):
    # Only fixed enums and syntactically bounded linker codes cross the boundary.
    if not isinstance(text,str):return set(),set()
    text=text[:32768];lower=text.lower()
    categories={name for name,patterns in CATEGORIES.items() if any(p in lower for p in patterns)}
    codes=set(re.findall(r'\bLNK[0-9]{4}\b',text))
    if codes:categories.add('msvc-linker-code')
    return categories,codes

def child_signals(message):
    categories=set();codes=set();queue=[(message,0)];count=0
    while queue and count<32:
        node,depth=queue.pop(0);count+=1
        if not isinstance(node,dict):continue
        found,lnk=fixed_signals(node.get('message'));categories.update(found);codes.update(lnk)
        children=node.get('children',[])
        if depth<2 and isinstance(children,list):queue.extend((child,depth+1) for child in children[:16])
    return categories,codes

def path_family(path):
    # PATH discovery classification does not identify rustc's chosen linker.
    text=str(path).replace('\\','/').lower()
    if '/git/usr/bin/' in text:return 'git-usr-bin'
    if '/vc/tools/msvc/' in text:return 'visual-studio-vc-tools'
    if '/windows kits/' in text:return 'windows-sdk'
    return 'other'

def diagnose(stdout,stderr):
    records=[];categories=set();errors=0;total=0;unknown=0;malformed=0;linker_codes=set()
    # Match signatures internally; never copy any text, span snippet, rendered
    # diagnostic, arbitrary package ID, command line, or absolute path out.
    for line in stdout.splitlines():
        try:item=json.loads(line)
        except ValueError:
            unknown+=1;continue
        if not isinstance(item,dict) or item.get('reason')!='compiler-message':continue
        message=item.get('message',{})
        if not isinstance(message,dict):
            malformed+=1;continue
        level=message.get('level')
        if level not in ('error','warning'):continue
        code=message.get('code');code=code.get('code') if isinstance(code,dict) else None
        if not isinstance(code,str) or not re.fullmatch(r'E[0-9]{4}',code):code=None
        locations=[]
        spans=message.get('spans',[])
        if not isinstance(spans,list):
            malformed+=1;spans=[]
        for span in spans:
            if not isinstance(span,dict) or not isinstance(span.get('file_name'),str):
                malformed+=1;continue
            if span.get('is_primary') is True:
                loc=safe_location(span)
                if loc and loc not in locations:locations.append(loc)
        total+=1
        if len(records)<200:records.append({'level':level,'code':code,'locations':locations[:20]})
        errors+=level=='error'
        found,codes=child_signals(message);categories.update(found);linker_codes.update(codes)
    found,codes=fixed_signals(stderr);categories.update(found);linker_codes.update(codes)
    return {'compiler_messages':records[:200],'compiler_messages_total':total,'non_json_stdout_lines':unknown,'malformed_records':malformed,'error_count':errors,'fixed_categories':sorted(categories),'linker_codes':sorted(linker_codes)[:64],'truncated':total>200,'classification':'diagnostic evidence only; no automatic product/environment root-cause claim'}
def tool_metadata(env):
    tools={}
    for name in ('rustc','cargo','link.exe','cl.exe'):
        path=shutil.which(name,path=env.get('PATH',''));row={'available_on_path':bool(path)}
        if path:
            p=Path(path);row.update(path_sha256=sha(str(p)),file_sha256=n.digest(p),path_family=path_family(p))
            if name in ('rustc','cargo'):
                # Only numeric version components are exported; wrappers/raw
                # banners are neither printed nor uploaded.
                try:
                    result=subprocess.run([path,'+1.99.1','--version'],cwd=n.SOURCE,env=env,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=10,check=False)
                    match=re.fullmatch(name+r' ([0-9]+\.[0-9]+\.[0-9]+)(?:[-+][A-Za-z0-9.-]+)?(?: \([^\r\n]*\))?\r?\n?',result.stdout.decode('utf-8',errors='replace'))
                    row.update(version_exit=result.returncode,version=match.group(1) if match else None)
                except (OSError,subprocess.TimeoutExpired) as exc:row['probe_error_type']=type(exc).__name__
        tools[name]=row
    sdk={}
    for k,v in env.items():
        if k.casefold() in ('windowssdkdir','vctoolsinstalldir','vcinstalldir','sdkroot','developer_dir'):
            sdk[k.casefold()]={'path_sha256':sha(v),'directory_exists':Path(v).is_dir()}
    return {'tools':tools,'sdk':sdk,'scope':'PATH discovery only; not proof MSVC registry discovery is unavailable'}
SMOKE_SOURCE=b'fn main() {}\n'
def link_smoke():
    # Fixed empty main, compiler/linker only; the produced binary is NEVER run.
    root=n.OUT/'link-smoke';root.mkdir(mode=0o700,exist_ok=False)
    source=root/'probe.rs';source.write_bytes(SMOKE_SOURCE);binary=root/'probe.exe'
    env=n.compile_env();rustc=shutil.which('rustc',path=env.get('PATH',''));n.require(rustc is not None,'rustc absent')
    result={'source_sha256':hashlib.sha256(SMOKE_SOURCE).hexdigest(),'executed_binary':False,'timeout_seconds':20,'target':'x86_64-pc-windows-msvc'}
    try:
        n.run([rustc,'+1.99.1','--target','x86_64-pc-windows-msvc','--crate-name','synthetic_key_import_link_probe','--crate-type','bin',str(source),'-o',str(binary)],'link-smoke',20,env)
        n.require(binary.is_file() and not binary.is_symlink(),'smoke executable missing or nonregular')
        result['binary_sha256']=n.digest(binary)
        result['state']='linked-not-executed'
        return True
    except BaseException as exc:
        result.update(state='failed-or-timeout',error_type=type(exc).__name__)
        return False
    finally:
        n.require(source.read_bytes()==SMOKE_SOURCE,'smoke source changed')
        stdout=(n.OUT/'link-smoke.stdout.log');stderr=(n.OUT/'link-smoke.stderr.log')
        result['diagnostic']=diagnose(stdout.read_text(errors='replace') if stdout.is_file() and stdout.stat().st_size<=64*1024*1024 else '',stderr.read_text(errors='replace') if stderr.is_file() and stderr.stat().st_size<=64*1024*1024 else '')
        n.write('link-smoke.json',result)

def main():
    n.require(n.platform_target()=='x86_64-pc-windows-msvc','Windows-only diagnostic')
    n.write('tool-metadata.json',tool_metadata(n.compile_env()))
    smoke_ok=link_smoke()
    code=0
    try:
        n.require(smoke_ok,'smoke failed; no product compilation')
        n.compile_product('x86_64-pc-windows-msvc',compile_seconds=120)
    except BaseException as exc:
        code=1;n.write('failure.json',{'error_type':type(exc).__name__,'stage':'compile-diagnostic' if smoke_ok else 'link-smoke','product_compile_attempted':smoke_ok,'classification':'unclassified; inspect bounded diagnostic','raw_logs_uploaded':False})
    finally:
        def read_log(name):
            p=n.OUT/name
            return p.read_text(encoding='utf-8',errors='replace') if p.is_file() and p.stat().st_size<=64*1024*1024 else ''
        n.write('diagnostic.json',diagnose(read_log('compile.stdout.log'),read_log('compile.stderr.log')))
        # Preserve input identity even when compilation fails. No runtime follows.
        n.identities();n.patch_hashes();n.require(n.git('write-tree')==n.I['TEST_TREE'],'diagnostic tree changed')
        n.require(not n.git('diff','--exit-code'),'diagnostic source mutation')
        for name,h in n.I['FIXTURES'].items():n.require(n.digest(n.SOURCE/name)==h,'diagnostic fixture changed')
        p=n.read('prepared.json')
        for name,h in p['control_files'].items():n.require(n.digest(n.CONTROL/name)==h,'diagnostic control changed')
        n.require(n.git('status','--porcelain','--untracked-files=all','--ignored')==p['status'],'diagnostic status changed')
    return code
if __name__=='__main__':
    try:result=main()
    except BaseException as exc:
        # No traceback/raw exception: even malformed tool or compiler data must
        # not disclose paths, banners, source snippets or arbitrary payloads.
        error_type=type(exc).__name__ if type(exc).__name__ in ('RuntimeError','OSError','FileNotFoundError','PermissionError','TimeoutExpired','UnicodeError','ValueError','TypeError','KeyError','AssertionError') else 'unknown-error'
        if n.OUT.is_dir():n.write('failure.json',{'stage':'diagnostic-control','error_type':error_type,'classification':'diagnostic unavailable; not product RED','raw_logs_uploaded':False})
        result=1
    sys.exit(result)
