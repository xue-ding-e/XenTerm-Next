"""Preparation of source/artifact gates, not a launcher or build authorization.

Reuses existing native-key-import artifact acceptance (one exact xenterm test
binary/features/native target/source), adding strict JSON shape/reparse handling.
Toolchain/SDK provenance and actual Host build evidence are separate required gates.
"""
import hashlib,json,re,subprocess
from pathlib import Path,PureWindowsPath,PurePosixPath
from compile_adapter import plain,read_json,digest,require,CAP,HOST_FILES
MAIN='459b593c830b8be643df29f9d9c0c46915d54696'
PRODUCT_TREE='fb7fdd0881444bf790635876bc4088f095b75e43'
TEST_TREE='3b660a20b67381b3522b6a43f37aaaa433201958'
TARGET='x86_64-pc-windows-msvc'
FEATURES=['default','desktop','remote-mcp']
BRANCH='validation/key-import-compile-only-20261030'
INPUTS=Path(__file__).with_name('source-inputs.json')
# Populated from the reviewed fixed input file; not derived from incoming JSON.
INPUTS_SHA='220d4bca082c2053a0d3e67541870066348bd7dd40f8f66d68eba185b45437b3'

def git_path_matches(value,expected):
    cls=PureWindowsPath if isinstance(expected,PureWindowsPath) else PurePosixPath
    candidate=cls(value)
    return candidate.is_absolute() and '..' not in candidate.parts and candidate==cls(expected)

def unique_json_pairs(items):
    result={}
    for key,value in items:
        require(key not in result,'duplicate compiler JSON key');result[key]=value
    return result

def native_artifact(output,source,root):
    require(isinstance(output,str) and len(output.encode('utf-8'))<=CAP,'compiler metadata budget')
    found=[]
    for line in output.splitlines():
        try:v=json.loads(line,object_pairs_hook=unique_json_pairs)
        except json.JSONDecodeError:continue
        if not isinstance(v,dict):continue
        if v.get('reason')=='compiler-artifact' and isinstance(v.get('profile'),dict) and v['profile'].get('test') is True and v.get('executable') and v.get('target',{}).get('name')=='xenterm':
            require(v['target'].get('kind')==['bin'] and sorted(v.get('features',[]))==FEATURES,'product features mismatch')
            require(plain(v['target']['src_path'])==source/'src/main.rs','wrong source')
            binary=plain(v['executable']);require(binary.is_relative_to(root/'target'/TARGET) and binary.is_file(),'wrong executable')
            found.append({'path':str(binary),'sha256':digest(binary),'target':TARGET,'features':FEATURES})
    require(len(found)==1,'executable count')
    return found[0]

class ProductIdentity:
    def __init__(self,root,control,expected_control_sha,control_files,git_executable,git_environment):
        self.root=plain(root);self.source=plain(self.root/'source');self.control=plain(control)
        require(re.fullmatch('[0-9a-f]{40}',expected_control_sha) is not None,'control SHA')
        self.sha=expected_control_sha;self.control_files=control_files
        self.git_executable=plain(git_executable);self.git_environment=git_environment
        require(digest(INPUTS)==INPUTS_SHA,'fixed inputs changed');self.inputs=read_json(INPUTS,16384)
        require(self.inputs['SHA']==MAIN and self.inputs['PRODUCT_TREE']==PRODUCT_TREE and self.inputs['TEST_TREE']==TEST_TREE and self.inputs['FEATURES']==FEATURES,'fixed source binding')
        self.source_status=None
    def git(self,path,*args):
        # Local identity commands only; caller supplies isolated noncredential env.
        return subprocess.check_output([str(self.git_executable),'-c','core.fsmonitor=false','-C',str(path),*args],env=self.git_environment,text=True,stderr=subprocess.DEVNULL,timeout=20).strip()
    def check(self):
        require(self.git(self.source,'rev-parse','HEAD')==MAIN,'main baseline')
        require(git_path_matches(self.git(self.source,'rev-parse','--show-toplevel'),self.source),'source checkout root')
        require(self.git(self.control,'rev-parse','HEAD')==self.sha,'control baseline')
        require(self.git(self.control,'status','--porcelain')=='','dirty control')
        require(self.git(self.source,'write-tree')==TEST_TREE,'candidate indexed tree')
        require(self.git(self.source,'diff','--no-ext-diff','--no-textconv','--exit-code')=='','source worktree mutation')
        status=self.git(self.source,'status','--porcelain','--untracked-files=all','--ignored')
        if self.source_status is None:
            require(status and all(line.startswith(('A  ','M  ')) for line in status.splitlines()),'uncontrolled initial source files')
            self.source_status=status
        require(status==self.source_status,'source status changed')
        for name,h in self.control_files.items():
            path=plain(self.control/name);require(path.is_relative_to(self.control) and digest(path)==h,'control input changed')
        for name,h in self.inputs['FIXTURES'].items():require(digest(plain(self.source/name))==h,'synthetic fixture changed')
        for name,h in self.inputs['PATCHES'].items():require(digest(plain(self.control/'validation/key-import-compile/adapter'/name))==h,'patch changed')
    def finish(self,artifact):
        self.check();binary=plain(artifact['path'])
        require(binary.is_relative_to(self.root/'target'/TARGET) and digest(binary)==artifact['sha256'],'binary changed')
        version=read_json(self.root/'version.json',16384)
        require(version.get('hash')==MAIN[:7] and version.get('source')=='branch' and version.get('channel')=='dev' and version.get('branch')==BRANCH,'version identity')
        return {'state':'compiled-not-executed','source_sha':MAIN,'test_tree':TEST_TREE,'control_sha':self.sha,'artifact':artifact,'version_sha256':digest(self.root/'version.json'),'final_identity_verified':True,'product_runtime_attempted':False}

def host_build_files(build_path,host_directory,expected_control_sha,current_run_id,expected_host_sources):
    """Validate future locally-produced build evidence; not a hand-authored approval.

    Trusted controller must first verify the actual build stage outcome and source
    snapshot. This validator alone cannot prove a compiler ran.
    """
    record=read_json(build_path,32768);host_directory=plain(host_directory)
    require(record.get('state')=='compiled' and type(record.get('compile_exit')) is int and record['compile_exit']==0 and record.get('control_sha')==expected_control_sha and record.get('run_id')==current_run_id,'Host build identity')
    require(record.get('initial_identity') is True and record.get('final_identity') is True and record.get('compiled_copy_identity') is True and record.get('host_sources')==expected_host_sources,'Host build source snapshot')
    hashes=record.get('files',{});require(set(hashes)==HOST_FILES,'Host runtime file set')
    for name,h in hashes.items():require(digest(plain(host_directory/name))==h,'Host runtime file changed')
    return hashes
