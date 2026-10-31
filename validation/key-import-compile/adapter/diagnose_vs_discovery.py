"""Bounded read-only differential; never selects or starts a product compiler."""
import hashlib,json,os,re,subprocess,threading
from pathlib import Path, PureWindowsPath
import vs_discovery as v
from compile_adapter import plain,digest,require
ARGS=v.ARGS
CAP=v.CAP


def environments(root):
 base={'SYSTEMROOT':r'C:\Windows','WINDIR':r'C:\Windows','PATH':r'C:\Windows\System32',
       'HOME':str(root/'home'),'USERPROFILE':str(root/'home'),'TEMP':str(root/'tmp'),'TMP':str(root/'tmp')}
 machine=dict(base,PROGRAMDATA=r'C:\ProgramData',ALLUSERSPROFILE=r'C:\ProgramData',
              PROGRAMFILES=r'C:\Program Files',PROGRAMW6432=r'C:\Program Files',SYSTEMDRIVE='C:')
 machine['ProgramFiles(x86)']=r'C:\Program Files (x86)'
 machine['CommonProgramFiles']=r'C:\Program Files\Common Files'
 machine['CommonProgramFiles(x86)']=r'C:\Program Files (x86)\Common Files'
 return {'minimal':base,'machine-directories':machine}


def summarize(code,raw):
 result={'exit_code':code,'json_bytes':len(raw)}
 try:rows=json.loads(raw.decode('utf-8-sig'),object_pairs_hook=v._pairs)
 except Exception:return dict(result,parse='invalid-json')
 if not isinstance(rows,list):return dict(result,parse='not-array')
 result.update(parse='array',row_count=len(rows),rows=[])
 for row in rows[:8]:
  if not isinstance(row,dict):result['rows'].append({'shape':'not-object'});continue
  item={}
  path=row.get('installationPath')
  item['root_id']=next((label for label,(root,_) in v.ROOTS.items() if isinstance(path,str) and len(path)<257 and str(PureWindowsPath(path)).casefold()==str(root).casefold()),'unrecognized-redacted')
  version=row.get('installationVersion')
  item['version']=version if isinstance(version,str) and re.fullmatch(r'[0-9]{1,3}(?:\.[0-9]{1,6}){3}',version) else 'invalid-redacted'
  product=row.get('productId')
  item['product']=product if product in ('Microsoft.VisualStudio.Product.Enterprise','Microsoft.VisualStudio.Product.Community','Microsoft.VisualStudio.Product.Professional','Microsoft.VisualStudio.Product.BuildTools') else 'unrecognized-redacted'
  for key in ('isComplete','isLaunchable','isPrerelease'):item[key]=row.get(key) if type(row.get(key)) is bool else None
  result['rows'].append(item)
 result['rows_truncated']=len(rows)>8
 return result


def query(executable, environment):
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
  require(not failed.is_set(),'VS_DISCOVERY_EXIT')
  return code, bytes(data)
 finally:
  if process.poll() is None:
   process.kill();process.wait(timeout=5)
  if done.is_set():process.stdout.close()


def main():
 require(os.name=='nt' and os.environ.get('GITHUB_ACTIONS')=='true','HOSTED_WINDOWS_ONLY')
 require(os.environ.get('GITHUB_REPOSITORY')=='xue-ding-e/XenTerm-Next','REPOSITORY')
 require(os.environ.get('GITHUB_SHA')==os.environ.get('EXPECTED_CONTROL_SHA'),'CONTROL_SHA')
 require(os.environ.get('GITHUB_REF')=='refs/heads/validation/xenterm-vs-discovery-diagnostic-20261031','CONTROL_REF')
 root=plain(Path(os.environ['RUNNER_TEMP']))/'vs-discovery-diagnostic'
 root.mkdir(exist_ok=False);(root/'home').mkdir();(root/'tmp').mkdir()
 state={'control_sha':os.environ['GITHUB_SHA'],'run_id':os.environ['GITHUB_RUN_ID'],'product_started':False,'queries':{}}
 success=False
 try:
  executable=plain(Path(str(v.VSWHERE)));before=digest(executable)
  state['vswhere_sha256']=before
  state['roots']={label:Path(str(path)).is_dir() for label,(path,_) in v.ROOTS.items()}
  for label,env in environments(root).items():
   try:
    code,raw=query(executable,env);state['queries'][label]=summarize(code,raw)
   except Exception as error:
    state['queries'][label]={'error':str(error) if str(error) in v.PREFLIGHT_CODES else 'QUERY_FAILED'}
  require(digest(plain(executable))==before,'VS_DISCOVERY_TOOL_CHANGED')
  success=all(q.get('exit_code')==0 and q.get('parse')=='array' for q in state['queries'].values())
 except Exception as error:state['error']=str(error) if str(error) in v.PREFLIGHT_CODES else 'DIAGNOSTIC_FAILED'
 finally:
  state['diagnostic_complete']=success
  encoded=json.dumps(state,sort_keys=True).encode();require(len(encoded)<=16384,'SUMMARY_BOUND')
  (root/'summary.json').write_bytes(encoded)
 return 0 if success else 1

if __name__=='__main__':raise SystemExit(main())
