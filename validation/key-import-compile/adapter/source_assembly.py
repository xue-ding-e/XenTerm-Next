"""Apply only the two reviewed patches to an already checked-out fixed main.
No network/clone, arbitrary patch, reset, clean, hooks, compiler, or product execution.
"""
import os,subprocess
from pathlib import Path
from compile_adapter import plain,digest,read_json,require
from product_identity import MAIN,PRODUCT_TREE,TEST_TREE,INPUTS,INPUTS_SHA
PATCH_ORDER=('key-import.patch','busy-save-tests-only.patch')
def assemble(source,git_executable,environment,run_git=None):
    source=plain(source);git_executable=plain(git_executable)
    require(git_executable.is_file(),'Git executable absent')
    require(set(environment)<= {'SYSTEMROOT','WINDIR','PATH','HOME','USERPROFILE','TEMP','TMP','GIT_CONFIG_NOSYSTEM','GIT_CONFIG_GLOBAL','GIT_TERMINAL_PROMPT'},'Git environment inventory')
    require(environment.get('GIT_CONFIG_NOSYSTEM')=='1' and environment.get('GIT_CONFIG_GLOBAL')==os.devnull and environment.get('GIT_TERMINAL_PROMPT')=='0','isolated Git configuration')
    require(digest(INPUTS)==INPUTS_SHA,'fixed inputs changed')
    inputs=read_json(INPUTS,16384);require(set(inputs['PATCHES'])==set(PATCH_ORDER),'patch inventory')
    patches=[plain(Path(__file__).parent/name) for name in PATCH_ORDER]
    for p in patches:require(digest(p)==inputs['PATCHES'][p.name],'patch changed')
    def git(*args):
        argv=[str(git_executable),'-c','core.hooksPath='+os.devnull,'-c','core.fsmonitor=false','-c','core.autocrlf=false','-C',str(source),*args]
        if run_git is not None:return run_git(argv,environment).strip()
        return subprocess.check_output(argv,env=environment,stdin=subprocess.DEVNULL,stderr=subprocess.DEVNULL,text=True,timeout=30).strip()
    git_dir=plain(source/'.git');require(git_dir.is_dir(),'fresh checkout Git directory required')
    require(plain(git_dir/'config').is_file(),'plain local Git config required')
    # Only names are inspected: never expose URL/helper/credential values.
    # Includes, filters, alternate worktree config and arbitrary core commands
    # are forbidden even when HEAD/tree look clean.
    keys=git('config','--local','--no-includes','--name-only','--list').splitlines()
    allowed={'core.repositoryformatversion','core.filemode','core.bare','core.logallrefupdates','core.ignorecase','core.symlinks','remote.origin.url','remote.origin.fetch'}
    for key in keys:
        require(key in allowed or (key.startswith('branch.') and key.endswith(('.remote','.merge'))),'unapproved local Git configuration')
    require(git('rev-parse','HEAD')==MAIN,'main baseline')
    require(git('status','--porcelain','--untracked-files=all','--ignored')=='','source not pristine')
    # PRODUCT_TREE is after the product patch, not main's original tree.
    # Preserve the original two-stage verification; never reset on failure.
    for p,tree in zip(patches,(PRODUCT_TREE,TEST_TREE)):
        git('apply','--check','--whitespace=error',str(p))
        git('apply','--index',str(p))
        require(git('write-tree')==tree,'candidate indexed tree')
    require(git('diff','--no-ext-diff','--no-textconv','--exit-code')=='','source worktree mutation')
    for name,h in inputs['FIXTURES'].items():require(digest(plain(source/name))==h,'synthetic fixture changed')
    return dict(source_sha=MAIN,test_tree=TEST_TREE,patches=inputs['PATCHES'],product_compile_attempted=False,product_runtime_attempted=False)
