"""Read-only installed VS discovery. No installer, SDK mutation or credentials.

Uses Microsoft's installed vswhere, fixed VC-component filter, two supported
Enterprise roots, and bounded output/time. Hashes identify this run's files;
this is not publisher-signature attestation or proof a product compiles.
"""
import hashlib
import json
import os
from pathlib import Path, PureWindowsPath
import re
import subprocess
import threading
from compile_adapter import plain, digest, require

VSWHERE = PureWindowsPath('C:/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe')
ROOTS = {
 'vs2022-enterprise': (PureWindowsPath('C:/Program Files/Microsoft Visual Studio/2022/Enterprise'), '17'),
 'vs2026-enterprise': (PureWindowsPath('C:/Program Files/Microsoft Visual Studio/18/Enterprise'), '18'),
}
ARGS = ['-latest','-products','Microsoft.VisualStudio.Product.Enterprise','-requires',
        'Microsoft.VisualStudio.Component.VC.Tools.x86.x64','-version','[17.0,19.0)','-format','json','-utf8']
CAP = 16384
PREFLIGHT_CODES = frozenset(('VS_DISCOVERY_DRAIN', 'VS_DISCOVERY_DUPLICATE', 'VS_DISCOVERY_EXIT', 'VS_DISCOVERY_INSTANCE_COUNT', 'VS_DISCOVERY_OUTPUT_BOUND', 'VS_DISCOVERY_ROOT', 'VS_DISCOVERY_SELECTOR_BOUND', 'VS_DISCOVERY_SELECTOR_CHANGED', 'VS_DISCOVERY_STATE', 'VS_DISCOVERY_TIMEOUT', 'VS_DISCOVERY_TOOL', 'VS_DISCOVERY_TOOL_CHANGED', 'VS_DISCOVERY_VC_VERSION', 'VS_DISCOVERY_VERSION', 'VS_DISCOVERY_WINDOWS_ONLY'))


def _pairs(pairs):
 result={}
 for key,value in pairs:
  require(key not in result,'VS_DISCOVERY_DUPLICATE');result[key]=value
 return result


def parse_discovery(raw):
 require(isinstance(raw,bytes) and 0<len(raw)<=CAP,'VS_DISCOVERY_OUTPUT_BOUND')
 rows=json.loads(raw.decode('utf-8-sig'),object_pairs_hook=_pairs)
 require(isinstance(rows,list) and len(rows)==1 and isinstance(rows[0],dict),'VS_DISCOVERY_INSTANCE_COUNT')
 row=rows[0];value=row.get('installationPath');version=row.get('installationVersion')
 require(isinstance(value,str) and len(value)<=256 and '\x00' not in value,'VS_DISCOVERY_ROOT')
 path=PureWindowsPath(value);require(path.is_absolute() and '..' not in path.parts,'VS_DISCOVERY_ROOT')
 matches=[(label,root,major) for label,(root,major) in ROOTS.items() if str(path).casefold()==str(root).casefold()]
 require(len(matches)==1,'VS_DISCOVERY_ROOT');label,root,major=matches[0]
 require(isinstance(version,str) and re.fullmatch(major+r'\.[0-9]{1,5}\.[0-9]{1,6}\.[0-9]{1,6}',version),'VS_DISCOVERY_VERSION')
 require(row.get('isComplete') is True and row.get('isLaunchable') is True
         and row.get('isPrerelease') is False,'VS_DISCOVERY_STATE')
 return dict(root_id=label,installation_root=str(root),installation_version=version)


def run_installed_query(executable, environment):
 # Only one fixed read-only discovery process; no shell or ambient environment.
 process=subprocess.Popen([str(executable),*ARGS],env=environment,stdin=subprocess.DEVNULL,
                          stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
 data=bytearray();done=threading.Event();failed=threading.Event();overflow=threading.Event()
 def read():
  try:
   while True:
    chunk=process.stdout.read(1024)
    if not chunk:break
    remaining=CAP-len(data);data.extend(chunk[:remaining])
    if len(chunk)>remaining:
     overflow.set();process.kill();break
  except Exception:failed.set()
  finally:done.set()
 reader=threading.Thread(target=read,daemon=True);reader.start()
 try:
  try:code=process.wait(timeout=15)
  except subprocess.TimeoutExpired:
   process.kill();process.wait(timeout=5);raise ValueError('VS_DISCOVERY_TIMEOUT')
  require(done.wait(5),'VS_DISCOVERY_DRAIN')
  require(not overflow.is_set(),'VS_DISCOVERY_OUTPUT_BOUND')
  require(not failed.is_set() and code==0,'VS_DISCOVERY_EXIT')
  return bytes(data)
 finally:
  if process.poll() is None:
   process.kill();process.wait(timeout=5)
  if done.is_set():process.stdout.close()


def parse_selector(raw):
 require(isinstance(raw,bytes) and 0<len(raw)<=128,'VS_DISCOVERY_SELECTOR_BOUND')
 version=raw.decode('utf-8-sig').strip()
 require(re.fullmatch(r'14\.[0-9]{2}\.[0-9]{5}',version),'VS_DISCOVERY_VC_VERSION')
 return version


def discovery_environment(root):
 # Same-run differential 41203671031: minimal env returns 0 instances; these
 # fixed machine directories restore the installed SetupConfiguration query.
 # No ambient credentials/profile are copied. Only vswhere receives this env;
 # compiler/fetch child environments retain their independent isolation.
 base={'SYSTEMROOT':'C:\\Windows','WINDIR':'C:\\Windows','PATH':'C:\\Windows\\System32',
      'HOME':str(root/'home'),'USERPROFILE':str(root/'home'),'TEMP':str(root/'tmp'),'TMP':str(root/'tmp')}
 base.update(PROGRAMDATA=r'C:\ProgramData',ALLUSERSPROFILE=r'C:\ProgramData',
             PROGRAMFILES=r'C:\Program Files',PROGRAMW6432=r'C:\Program Files',SYSTEMDRIVE='C:')
 base['ProgramFiles(x86)']=r'C:\Program Files (x86)'
 base['CommonProgramFiles']=r'C:\Program Files\Common Files'
 base['CommonProgramFiles(x86)']=r'C:\Program Files (x86)\Common Files'
 return base


def select_installed(root):
 require(os.name=='nt','VS_DISCOVERY_WINDOWS_ONLY');root=plain(root)
 executable=plain(Path(str(VSWHERE)));require(executable.is_file(),'VS_DISCOVERY_TOOL')
 before=digest(executable)
 env=discovery_environment(root)
 selected=parse_discovery(run_installed_query(executable,env))
 require(digest(plain(executable))==before,'VS_DISCOVERY_TOOL_CHANGED')
 installation=plain(Path(selected['installation_root']));require(installation.is_dir(),'VS_DISCOVERY_ROOT')
 selector=plain(installation/'VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt')
 require(selector.is_file() and 0<selector.stat().st_size<=128,'VS_DISCOVERY_SELECTOR_BOUND')
 with selector.open('rb') as stream:raw=stream.read(129)
 version=parse_selector(raw);selector_hash=hashlib.sha256(raw).hexdigest()
 require(digest(plain(selector))==selector_hash,'VS_DISCOVERY_SELECTOR_CHANGED')
 selected.update(vc_version=version,vswhere_sha256=before,selector_sha256=selector_hash)
 return selected


def unchanged(selection):
 label=selection.get('root_id');require(label in ROOTS,'VS_DISCOVERY_ROOT')
 root,major=ROOTS[label]
 require(selection.get('installation_root')==str(root),'VS_DISCOVERY_ROOT')
 require(digest(plain(Path(str(VSWHERE))))==selection['vswhere_sha256'],'VS_DISCOVERY_TOOL_CHANGED')
 selector=plain(Path(str(root))/'VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt')
 require(digest(selector)==selection['selector_sha256'],'VS_DISCOVERY_SELECTOR_CHANGED')
