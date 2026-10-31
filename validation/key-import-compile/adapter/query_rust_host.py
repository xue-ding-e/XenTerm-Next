"""Bounded query adapter; trusted controller first checks setup/build job outcomes.
Outer Host timeout leaves lifecycle unknown: fails whole stage, never retries.
No Cargo fetch or compile. Raw version outputs remain local, never artifact uploads.
"""
import subprocess
import json
import stat
from pathlib import Path
from compile_adapter import plain,digest,require
from product_identity import host_build_files,read_json
from verify_rust_setup import verify

def fixed_install_diagnostic(root):
    # Fixed scope only: no directory scan, raw path, environment or tool output.
    names=['cargo-home','rustup-home','rustup-home/toolchains/1.99.1-x86_64-pc-windows-msvc']
    names += ['cargo-home/bin/'+n for n in ('rustup.exe','cargo.exe','rustc.exe')]
    names += ['rustup-home/toolchains/1.99.1-x86_64-pc-windows-msvc/bin/'+n for n in ('cargo.exe','rustc.exe')]
    rows=[]
    for index,name in enumerate(names):
        p=root/name;state='unknown';reparse=False
        try:
            info=p.lstat();state='file' if stat.S_ISREG(info.st_mode) else 'directory' if stat.S_ISDIR(info.st_mode) else 'other'
        except FileNotFoundError:state='missing'
        except OSError:state='unreadable'
        ancestors_known=True
        for parent in (p,*p.parents):
            try:
                value=parent.lstat();reparse |= stat.S_ISLNK(value.st_mode) or bool(getattr(value,'st_file_attributes',0)&0x400)
            except FileNotFoundError:continue
            except OSError:ancestors_known=False
        rows.append(dict(relative=name,state=state,reparse=reparse,ancestors_readable=ancestors_known))
    return rows

def query(root,host_directory,control,run,host_sources,identity_gate):
    root=plain(root);host_directory=plain(host_directory);identity_gate()
    before=host_build_files(root/'host-build.json',host_directory,control,run,host_sources)
    # CreateNew consumes this root permanently, even if launch or cleanup fails.
    with (root/"rust-query-started.marker").open("x") as f:f.write("single-use\n")
    result=None;called=False;rows=None;outer="not-started";final_identity=False;error_type=None;gate=None
    install_diagnostic=fixed_install_diagnostic(root)
    def owned(command,label,seconds):
        nonlocal called,rows,outer
        require(label in ('rustc-version','cargo-version','resolved-rustc') and seconds==20,'fixed query')
        if not called:
            called=True
            # Exactly 3 * (20s + 5s cleanup), plus outer Host startup margin.
            outer="unknown"
            completed=subprocess.run([str(host_directory/'CompileOnlyHost.exe'),'--verify-rust-setup',str(root)],cwd=root,env={'SYSTEMROOT':__import__('os').environ['SYSTEMROOT']},stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=85,check=False)
            require(completed.returncode==0,'Host query failure; do not continue')
            record=read_json(root/'rust-query-receipts.json',32768)
            require(record.get('schema')==1 and record.get('productCompileAttempted') is False and record.get('productRuntimeAttempted') is False,'query scope receipt')
            require(record.get('completed') is True and record.get('query_count')==3,'query receipts incomplete')
            rows=record.get('rows',[])
            require(len(rows)==3 and [r.get('label') for r in rows]==['rustc-version','cargo-version','resolved-rustc'],'exact query ordering')
        row=next(r for r in rows if r['label']==label)
        require(row.get('commandPolicy')=='smoke-v1' and row.get('outputBudget')==1048576 and type(row.get('cleanupMilliseconds')) is int and 0<=row['cleanupMilliseconds']<=5000,'query policy receipt')
        p=plain(root/(label+'.stdout.log'));require(p.stat().st_size<=16384,'query output bounds')
        return row,p.read_text(encoding='utf-8')
    try:
        result=verify(root,owned)
        outer="verified-return"
        return result
    except Exception as e:
        error_type=type(e).__name__
        allowed={"missing ancestor","reparse point","canonical absolute path required","rustc version target","cargo version","toolchain resolved outside synthetic root","tool changed during verification"}
        gate=str(e) if str(e) in allowed else "other"
        raise
    finally:
        try:
            identity_gate()
            require(before==host_build_files(root/'host-build.json',host_directory,control,run,host_sources),'Host runtime identity changed')
            final_identity=True
        finally:
            with (root/'rust-query-state.json').open('x') as f:json.dump(dict(outer_lifecycle=outer,final_identity=final_identity,verified=result is not None and final_identity,reusable=False,install_diagnostic=install_diagnostic,error_type=error_type,gate=gate),f)

def main():
    import os,re
    root=plain(Path(os.environ['RUNNER_TEMP'])/'key-import-compile-evidence')
    source=plain(Path(__file__).parent);workspace=plain(os.environ['GITHUB_WORKSPACE'])
    control=os.environ['EXPECTED_CONTROL_SHA'];run=os.environ['GITHUB_RUN_ID']
    require(os.environ.get('RUNNER_ENVIRONMENT')=='github-hosted' and os.environ.get('RUNNER_ARCH')=='X64' and os.environ.get('GITHUB_REPOSITORY')=='xue-ding-e/XenTerm-Next' and os.environ.get('GITHUB_REF')=='refs/heads/validation/xenterm-compile-preflight-20261030','query scope')
    require(re.fullmatch('[0-9a-f]{40}',control) and os.environ.get('GITHUB_SHA')==control and run.isdigit(),'query run identity')
    require(os.environ.get('SETUP_OUTCOME')=='success' and os.environ.get('HOST_OUTCOME')=='success','producer step outcomes')
    require(source==workspace/'validation/key-import-compile/adapter','query source location')
    def identity():
        for args,expected in [(['rev-parse','HEAD'],control),(['status','--porcelain','--untracked-files=all'],'')]:
            p=subprocess.run(['git','-C',str(workspace),*args],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=20,check=True)
            require(p.stdout.decode().strip()==expected,'control source changed')
    sources={'Host/'+n:digest(plain(source/'Host'/n)) for n in ('Host.csproj','Program.cs','windows-owned-process.cs','NuGet.Config','global.json')}
    report=None
    try:
        report=query(root,root/'host-build/bin/Release/net8.0',control,run,sources,identity)
        with (root/'rust-verified.json').open('x') as f:json.dump(dict(report,control=control,run=run,rust_toolchain_verified=True),f)
        return 0
    except Exception:
        # No raw command output, paths or environment are sent to Actions log.
        return 1
if __name__=='__main__':raise SystemExit(main())
