"""Post-setup verifier candidate. Called only after fixed setup action succeeds.
No install/download/copy or raw output publication. Version calls use the reviewed
short owned natural-tree transport; this module has no subprocess launcher.
"""
from pathlib import Path
from compile_adapter import plain,digest,require
ACTION='dtolnay/rust-toolchain@b725eda525362e7cf34af9a7243cac1e3f01a7ec'
TARGET='x86_64-pc-windows-msvc'
TOOLCHAIN='1.99.1-'+TARGET

def check_child(receipt,text):
    require(receipt.get('completionMode')=='natural-tree' and receipt.get('treeEmpty') is True and receipt.get('safeToStart') is True and type(receipt.get('exitCode')) is int and receipt['exitCode']==0,'setup query lifecycle')
    for key in ('timedOut','outputExceeded','captureFailed','forcedOwnedCleanup'):require(receipt.get(key) is False,'setup query failed')
    require(isinstance(text,str) and len(text.encode())<=16384,'bounded version output')

def verify(root,run_owned):
    root=plain(root);cargo_home=plain(root/'cargo-home');rustup_home=plain(root/'rustup-home')
    sysroot=plain(rustup_home/'toolchains'/TOOLCHAIN)
    paths=[cargo_home/'bin'/n for n in ('rustup.exe','cargo.exe','rustc.exe')]+[sysroot/'bin'/n for n in ('cargo.exe','rustc.exe')]
    before={p.relative_to(root).as_posix():digest(plain(p)) for p in paths}
    commands=[('rustc-version',cargo_home/'bin/rustc.exe',['+1.99.1','-vV']),('cargo-version',cargo_home/'bin/cargo.exe',['+1.99.1','-V']),('resolved-rustc',cargo_home/'bin/rustup.exe',['which','--toolchain','1.99.1','rustc'])]
    outputs={}
    for label,exe,args in commands:
        receipt,text=run_owned([str(exe),*args],label,20)
        check_child(receipt,text);outputs[label]=text.strip()
    rustc=outputs['rustc-version'].splitlines()
    require(rustc and rustc[0].startswith('rustc 1.99.1 ') and 'release: 1.99.1' in rustc and 'host: '+TARGET in rustc,'rustc version target')
    require(outputs['cargo-version'].startswith('cargo 1.99.1 '),'cargo version')
    resolved=plain(outputs['resolved-rustc']);require(resolved==sysroot/'bin/rustc.exe','toolchain resolved outside synthetic root')
    after={p.relative_to(root).as_posix():digest(plain(p)) for p in paths};require(before==after,'tool changed during verification')
    return dict(action=ACTION,action_owner='third-party',bootstrap_endpoint='https://win.rustup.rs/x86_64',action_modifies_job_path_and_environment=True,distribution_server='https://static.rust-lang.org',update_root='https://static.rust-lang.org/rustup',version='1.99.1',target=TARGET,files=after,owned_query_count=3,registry_cache_copied=False,product_compile_attempted=False,product_runtime_attempted=False)
