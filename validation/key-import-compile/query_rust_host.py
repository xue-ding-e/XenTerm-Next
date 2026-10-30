"""Bounded query adapter; trusted controller first checks setup/build job outcomes.
Outer Host timeout leaves lifecycle unknown: fails whole stage, never retries.
No Cargo fetch or compile. Raw version outputs remain local, never artifact uploads.
"""
import subprocess
import json
from pathlib import Path
from compile_adapter import plain,digest,require
from product_identity import host_build_files,read_json
from verify_rust_setup import verify

def query(root,host_directory,control,run,host_sources,identity_gate):
    root=plain(root);host_directory=plain(host_directory);identity_gate()
    before=host_build_files(root/'host-build.json',host_directory,control,run,host_sources)
    # CreateNew consumes this root permanently, even if launch or cleanup fails.
    with (root/"rust-query-started.marker").open("x") as f:f.write("single-use\n")
    result=None;called=False;rows=None;outer="not-started";final_identity=False
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
    finally:
        try:
            identity_gate()
            require(before==host_build_files(root/'host-build.json',host_directory,control,run,host_sources),'Host runtime identity changed')
            final_identity=True
        finally:
            with (root/'rust-query-state.json').open('x') as f:json.dump(dict(outer_lifecycle=outer,final_identity=final_identity,verified=result is not None and final_identity,reusable=False),f)

def main():
    import os,re
    root=plain(Path(os.environ['RUNNER_TEMP'])/'key-import-compile-evidence')
    source=plain(Path(__file__).parent);workspace=plain(os.environ['GITHUB_WORKSPACE'])
    control=os.environ['EXPECTED_CONTROL_SHA'];run=os.environ['GITHUB_RUN_ID']
    require(os.environ.get('RUNNER_ENVIRONMENT')=='github-hosted' and os.environ.get('RUNNER_ARCH')=='X64' and os.environ.get('GITHUB_REPOSITORY')=='xue-ding-e/XenTerm-Next' and os.environ.get('GITHUB_REF')=='refs/heads/validation/xenterm-compile-preflight-20261030','query scope')
    require(re.fullmatch('[0-9a-f]{40}',control) and os.environ.get('GITHUB_SHA')==control and run.isdigit(),'query run identity')
    require(os.environ.get('SETUP_OUTCOME')=='success' and os.environ.get('HOST_OUTCOME')=='success','producer step outcomes')
    require(source==workspace/'validation/key-import-compile','query source location')
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
