"""Read-only same-run selected compiler/SDK byte inventory, not full provenance.
Never activates vcvars, changes system SDKs, probes credentials, or runs a binary.
The actual compile receipt, source identity and this before/after snapshot are
independent gates. Selected file presence/hashes alone never qualify a product.
"""
import os,re
from pathlib import Path,PureWindowsPath
from compile_adapter import plain,digest,require
SDK='10.0.26100.0'
def fixed_paths(vc_version,vs_root):
    require(isinstance(vc_version,str) and re.fullmatch(r'14\.[0-9]{2}\.[0-9]{5}',vc_version),'unsupported VC version')
    from vs_discovery import ROOTS
    vs=PureWindowsPath(vs_root)
    require(vs in {item[0] for item in ROOTS.values()},'unsupported VS root')
    vc=vs/'VC/Tools/MSVC'/vc_version
    sdk=PureWindowsPath('C:/Program Files (x86)/Windows Kits/10')
    return {
      'cl':vc/'bin/Hostx64/x64/cl.exe','link':vc/'bin/Hostx64/x64/link.exe',
      'vcruntime-header':vc/'include/vcruntime.h','vcruntime-lib':vc/'lib/x64/vcruntime.lib',
      'windows-header':sdk/f'Include/{SDK}/um/Windows.h',
      'ucrt-header':sdk/f'Include/{SDK}/ucrt/stdio.h',
      'kernel32-lib':sdk/f'Lib/{SDK}/um/x64/kernel32.lib',
      'ucrt-lib':sdk/f'Lib/{SDK}/ucrt/x64/ucrt.lib',
      'git-bash':PureWindowsPath('C:/Program Files/Git/bin/bash.exe'),
      'git':PureWindowsPath('C:/Program Files/Git/cmd/git.exe')}
def capture(vc_version,control_sha,run_id,vs_root,read_file=None):
    require(re.fullmatch('[0-9a-f]{40}',control_sha) is not None and re.fullmatch('[0-9]+',str(run_id)) is not None,'run identity')
    paths=fixed_paths(vc_version,vs_root)
    if read_file is None:
        require(os.name=='nt','Windows capture required')
        def read_file(path):
            p=plain(Path(str(path)));require(p.is_file() and 0<p.stat().st_size<=128*1024*1024,'compiler material bound')
            return {'bytes':p.stat().st_size,'sha256':digest(p)}
    rows={}
    for name,path in paths.items():
        row=read_file(path)
        require(type(row.get('bytes')) is int and 0<row['bytes']<=128*1024*1024 and re.fullmatch('[0-9a-f]{64}',row.get('sha256','')) is not None,'material evidence')
        rows[name]={'relative_to_fixed_installation':str(path),**row}
    return dict(control_sha=control_sha,run_id=str(run_id),vc_version=vc_version,sdk_version=SDK,selected_files=rows,all_compiler_inputs_verified=False,product_environment_qualified=False,product_compile_attempted=False)
def unchanged(before,after):
    require(before==after,'compiler material changed')
    require(before.get('all_compiler_inputs_verified') is False and before.get('product_environment_qualified') is False,'inventory overclaim')

def fixed_environment(vc_version,root,vs_root):
    """Build a non-inherited x64 tool search path; fail on missing installations.
    Does not assert publisher signatures or full build-script network isolation.
    """
    require(os.name=='nt','Windows environment required')
    files=fixed_paths(vc_version,vs_root);root=plain(root)
    vc=Path(str(files['cl'])).parents[3]
    kits=Path('C:/Program Files (x86)/Windows Kits/10')
    tool=plain(root/'rustup-home/toolchains/1.99.1-x86_64-pc-windows-msvc/bin')
    path=[tool,vc/'bin/Hostx64/x64',kits/f'bin/{SDK}/x64',Path('C:/Program Files/Git/bin'),Path('C:/Program Files/Git/cmd'),Path('C:/Program Files/Git/usr/bin'),Path('C:/Windows/System32')]
    include=[vc/'include',kits/f'Include/{SDK}/ucrt',kits/f'Include/{SDK}/shared',kits/f'Include/{SDK}/um',kits/f'Include/{SDK}/winrt']
    libs=[vc/'lib/x64',kits/f'Lib/{SDK}/ucrt/x64',kits/f'Lib/{SDK}/um/x64']
    for p in path+include+libs:require(plain(p).is_dir(),'fixed SDK directory absent')
    return {'PATH':';'.join(map(str,path)),'INCLUDE':';'.join(map(str,include)),'LIB':';'.join(map(str,libs)),
      'SYSTEMROOT':'C:\\Windows','WINDIR':'C:\\Windows','COMSPEC':'C:\\Windows\\System32\\cmd.exe',
      'VCTOOLSINSTALLDIR':str(vc)+'\\','VCTOOLSVERSION':vc_version,'WINDOWSSDKDIR':str(kits)+'\\',
      'WINDOWSSDKVERSION':SDK+'\\','VSCMD_ARG_HOST_ARCH':'x64','VSCMD_ARG_TGT_ARCH':'x64'}
