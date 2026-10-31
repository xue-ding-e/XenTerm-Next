"""Bounded untrusted Cargo JSON -> whitelist-only diagnostic metadata.

Never exports message/rendered/source text or build-script output. Descriptions
are static categories, NOT reconstructed exact expected/actual Rust types. Missing
structured messages are a diagnostic limitation, never proof of an environment bug.
"""
import json
from pathlib import Path
import re
import tomllib
from adapter.compile_adapter import plain, require
from dependency_plan import REGISTRY
INPUT_CAP = 16 * 1024 * 1024
LINE_CAP = 256 * 1024
OUTPUT_CAP = 64 * 1024
MAX_ROWS = 64
DESCRIPTIONS = {
 'E0308': 'Expected and actual Rust types differ; exact type text omitted.',
 'E0277': 'A required trait bound is not satisfied; exact type/trait text omitted.',
 'E0599': 'Method or associated item is unavailable for the type; names omitted.',
 'E0432': 'An import could not be resolved; imported path omitted.',
 'E0433': 'A referenced module or type could not be resolved; path omitted.',
 'E0425': 'A value is not found in the current scope; identifier omitted.',
 'E0061': 'Function argument count differs from its signature; details omitted.',
 'E0382': 'A moved value was used again; value/source text omitted.',
 'E0502': 'Conflicting mutable and immutable borrows; source text omitted.',
 'E0499': 'Multiple mutable borrows conflict; source text omitted.',
 'E0282': 'Rust could not infer a required type; source text omitted.',
}


def _object(pairs):
 out={}
 for key,value in pairs:
  require(key not in out,'duplicate diagnostic key');out[key]=value
 return out


def _location(span, source):
 if not isinstance(span,dict):return None
 name=span.get('file_name')
 if not isinstance(name,str) or len(name)>4096:return None
 candidate=Path(name)
 if not candidate.is_absolute():candidate=source/candidate
 if '..' in candidate.parts or not candidate.is_relative_to(source):return None
 try:
  candidate=plain(candidate)
  if not candidate.is_file():return None
  relative=candidate.relative_to(source).as_posix()
 except (OSError,ValueError):return None
 if len(relative)>256 or not re.fullmatch(r'[A-Za-z0-9_./-]+',relative):return None
 keys=('line_start','line_end','column_start','column_end')
 if not all(type(span.get(k)) is int and 1<=span[k]<=10000000 for k in keys):return None
 if span['line_end']<span['line_start']:return None
 return dict(file=relative,**{k:span[k] for k in keys})


def summarize(source, stdout):
 source=plain(Path(source));stdout=plain(Path(stdout))
 lock=plain(source/'Cargo.lock');require(lock.is_file() and lock.stat().st_size<=2*1024*1024,'lock bound')
 packages=tomllib.loads(lock.read_text()).get('package',[])
 allowed={}
 for package in packages:
  name=package.get('name');version=package.get('version')
  require(isinstance(name,str) and re.fullmatch(r'[A-Za-z0-9_-]{1,64}',name) and
          isinstance(version,str) and re.fullmatch(r'[A-Za-z0-9_.+-]{1,64}',version),'package identity')
  if package.get('source') is None:
   require(name=='xenterm','local package identity')
   key='path+'+source.as_uri()+'#'+name+'@'+version
   allowed[key]=dict(package='xenterm',scope='product')
  else:
   require(package.get('source')==REGISTRY,'registry scope')
   allowed[REGISTRY+'#'+name+'@'+version]=dict(package=name,scope='locked-dependency')
 require(stdout.is_file() and stdout.stat().st_size<=INPUT_CAP,'diagnostic input bound')
 with stdout.open('rb') as stream:raw=stream.read(INPUT_CAP+1)
 require(len(raw)<=INPUT_CAP,'diagnostic input grew')
 result=dict(schema=1,structured_diagnostics_available=False,description_policy='static-code-category-only',
             exact_type_or_freeform_text_included=False,source_snippets_included=False,
             build_script_text_included=False,truncated=False,omitted_lines=0,diagnostics=[])
 for line in raw.splitlines():
  if len(line)>LINE_CAP:
   result['truncated']=True;result['omitted_lines']+=1;continue
  try:record=json.loads(line,object_pairs_hook=_object)
  except (ValueError,UnicodeError,RecursionError):result['omitted_lines']+=1;continue
  if not isinstance(record,dict) or record.get('reason')!='compiler-message':continue
  identity=allowed.get(record.get('package_id')) if isinstance(record.get('package_id'),str) else None
  message=record.get('message')
  if identity is None or not isinstance(message,dict) or message.get('level') not in ('error','warning'):
   result['omitted_lines']+=1;continue
  code=message.get('code');code=code.get('code') if isinstance(code,dict) else None
  # Rust error codes only. Lint strings and unrecognized free-form code values are omitted.
  code=code if isinstance(code,str) and re.fullmatch(r'E[0-9]{4}',code) else None
  row=dict(identity,level=message['level'],code=code,
           explanation=DESCRIPTIONS.get(code,'Compiler diagnostic reported; free-form details omitted.'),locations=[])
  spans=message.get('spans',[])
  if isinstance(spans,list):
   for span in spans[:64]:
    location=_location(span,source)
    if location is not None:
     row['locations'].append(location)
     if len(row['locations'])==4:break
  result['diagnostics'].append(row);result['structured_diagnostics_available']=True
  if len(result['diagnostics'])>MAX_ROWS or len(json.dumps(result).encode())>OUTPUT_CAP-512:
   result['diagnostics'].pop();result['truncated']=True;break
 result['limitation']='No rendered messages, exact type strings, source snippets or build-script text; absence is not an environment diagnosis.'
 require(len(json.dumps(result).encode())<=OUTPUT_CAP,'diagnostic output bound')
 return result
