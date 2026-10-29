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
def diagnose(stdout,stderr):
    records=[];categories=set();errors=0;total=0;unknown=0;malformed=0
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
        text=str(message.get('message','')).lower()
        categories.update(name for name,patterns in CATEGORIES.items() if any(p in text for p in patterns))
    text=stderr.lower()
    categories.update(name for name,patterns in CATEGORIES.items() if any(p in text for p in patterns))
    return {'compiler_messages':records[:200],'compiler_messages_total':total,'non_json_stdout_lines':unknown,'malformed_records':malformed,'error_count':errors,'fixed_categories':sorted(categories),'truncated':total>200,'classification':'diagnostic evidence only; no automatic product/environment root-cause claim'}
def tool_metadata(env):
    tools={}
    for name in ('rustc','cargo','link.exe','cl.exe'):
        path=shutil.which(name,path=env.get('PATH',''));row={'available_on_path':bool(path)}
        if path:
            p=Path(path);row.update(path_sha256=sha(str(p)),file_sha256=n.digest(p))
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
def main():
    n.require(n.platform_target()=='x86_64-pc-windows-msvc','Windows-only diagnostic')
    n.write('tool-metadata.json',tool_metadata(n.compile_env()))
    code=0
    try:n.compile_product('x86_64-pc-windows-msvc',compile_seconds=120)
    except BaseException as exc:
        code=1;n.write('failure.json',{'error_type':type(exc).__name__,'stage':'compile-diagnostic','classification':'unclassified; inspect bounded diagnostic','raw_logs_uploaded':False})
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
