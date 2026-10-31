"""Local controller candidate. No CLI/dispatch and no product runtime path.

Acquisition uses the qualified synthetic Host through a same-run monitored launcher.
Native product validation is still pending; historical qualification is not a product result. This module cannot substitute synthetic compiler qualification for
an acquisition receipt or current-run Host/toolchain/SDK evidence.
"""
import os,sys,shutil,json
from pathlib import Path
sys.path.insert(0,str(Path(__file__).parent/'adapter'))
from compile_adapter import OwnedCompileAdapter,qualification_gate,digest,plain,require
from product_identity import ProductIdentity,host_build_files,native_artifact,MAIN,BRANCH
from query_rust_host import query
from source_assembly import assemble
from compiler_materials import capture,fixed_environment,unchanged
from dependency_plan import public_locked_fetch_plan
from dependency_executor import validate_fetch_receipt
from owned_fetch import OwnedFetch
import compile_policy
from acquisition_inputs import reject_unreviewed_config,inspect_cache_budget

CHILD_SECONDS=2700
OUTER_HOST_SECONDS=2715
# Future workflow must give the compile stage 46min; 18min preflight is not reused.
COMPILE_STEP_MINUTES=46

def execute(root,control,control_sha,run_id,git_executable,git_environment,vc_version,native_metadata):
    run_id=str(run_id)
    require(os.name=='nt' and os.environ.get('RUNNER_ENVIRONMENT')=='github-hosted' and os.environ.get('RUNNER_ARCH')=='X64','hosted Windows x64 required')
    require(os.environ.get('GITHUB_REPOSITORY')=='xue-ding-e/XenTerm-Next' and os.environ.get('GITHUB_REF')=='refs/heads/'+BRANCH,'fixed control scope')
    require(os.environ.get('GITHUB_SHA')==control_sha and os.environ.get('GITHUB_RUN_ID')==str(run_id),'current run scope')
    require(os.environ.get('SETUP_OUTCOME')=='success' and os.environ.get('HOST_OUTCOME')=='success','current producer outcomes')
    root=plain(root);control=plain(control);source_dir=plain(Path(__file__).parent)
    require(source_dir==control/'validation/key-import-compile','controller source location')
    # The reviewed controller manifest must be committed before this function is
    # deployed; its exact control SHA is externally bound by the hosted event.
    files={str(p.relative_to(control)):digest(plain(p)) for p in source_dir.rglob('*') if p.is_file()}
    assemble(root/'source',git_executable,git_environment)
    identity=ProductIdentity(root,control,control_sha,files,git_executable,git_environment)
    identity.check();qualification_gate(native_metadata)
    host_sources={'Host/'+p.name:digest(plain(p)) for p in (source_dir/'adapter/Host').iterdir() if p.is_file()}
    host=root/'host-build/bin/Release/net8.0'
    hashes=host_build_files(root/'host-build.json',host,control_sha,run_id,host_sources)
    toolchain=query(root,host,control_sha,run_id,host_sources,identity.check)
    materials=capture(vc_version,control_sha,run_id)
    reject_unreviewed_config(root)
    inspect_cache_budget(root,shutil.disk_usage(root).free)
    plan=public_locked_fetch_plan(root,identity.check)
    # Import only committed evidence parsers from this fixed control checkout.
    # They attest different things: transport failure cases versus actual Host entry.
    sys.path[:0]=[str(control/'validation/fetch-evidence'),str(control/'validation/host-entry-evidence'),str(source_dir/'dependency-host')]
    from fetch_evidence import verify_transport_evidence
    from host_entry_evidence import verify_host_entry_evidence
    from fetch_contract import expected_environment
    fetch_env=expected_environment(root,Path('C:/Windows'))
    fetcher=OwnedFetch(root,root/'fetch-host-build/bin/Release/net8.0',root/'fetch-host-build.json',
        control_sha,run_id,identity.check,fetch_env,
        lambda:verify_transport_evidence(control/'validation/fetch-evidence/fixtures/fetch-native-af465570.json',control/'validation/fetch-native'),
        lambda:verify_host_entry_evidence(control/'validation/host-entry-evidence/fixtures/host-entry-0ac5255.json',control/'validation/fetch-host-entry-native'),
        control/'validation/fetch-host-entry-native/Host')
    fetched=fetcher(plan)
    fetch_checked=validate_fetch_receipt(fetched,plan,root,control_sha,run_id)
    fetch_checked.update(control_sha=control_sha,run_id=str(run_id),resource_monitoring=fetched['resource_monitoring'])
    with (root/'fetch-validated.json').open('x') as stream:json.dump(fetch_checked,stream,indent=2)
    reject_unreviewed_config(root)
    inspect_cache_budget(root,shutil.disk_usage(root).free)
    def current_identity():
        identity.check()
        for name,h in toolchain['files'].items():require(digest(plain(root/name))==h,'real toolchain changed')
        unchanged(materials,capture(vc_version,control_sha,run_id))
    current_identity()
    env=compile_policy.compile_environment(fixed_environment(vc_version,root),root)
    request=compile_policy.command(root/'rustup-home/toolchains/1.99.1-x86_64-pc-windows-msvc/bin/cargo.exe',root/'source')
    request['environment']=env
    transport=OwnedCompileAdapter(root,host/'CompileOnlyHost.exe',hashes,native_metadata,current_identity)
    receipt,output=transport(request)
    binary=native_artifact(output,root/'source',root)
    result=identity.finish(binary)
    current_identity()
    return dict(result,compile_receipt=receipt,compiler_materials=materials,fetch_validation=fetch_checked)
