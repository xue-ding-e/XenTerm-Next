"""Read-only acquisition configuration/disk gates; no downloads or cleanup.
Not OS egress enforcement or a claim of complete compiler-input provenance.
"""
import os
from pathlib import Path
from adapter.compile_adapter import plain,require
MAX_CACHE_BYTES=4*1024**3
MAX_CACHE_FILES=100000
MIN_FREE_BYTES=8*1024**3

def reject_unreviewed_config(root):
    root=plain(root);source=plain(root/'source')
    candidates=set()
    for parent in (source,*source.parents):
        for name in ('config','config.toml'):candidates.add(parent/'.cargo'/name)
    for name in ('config','config.toml','credentials','credentials.toml'):
        candidates.add(root/'cargo-home'/name)
    for name in ('.gitconfig','.netrc','_netrc'):
        candidates.add(root/'home'/name)
    for p in candidates:
        # lstat notices dangling links, never follows or prints file values.
        try:p.lstat()
        except FileNotFoundError:continue
        require(False,'unreviewed acquisition configuration present')
    return {'reviewed_absent_configuration_paths':len(candidates)}

def inspect_cache_budget(root,free_bytes,checkpoint=None):
    """Incremental local scan; checkpoint can abort between entries/directories.

    No os.walk list of an unbounded directory and no reparse traversal. A kernel
    filesystem call is not interruptible here; this is not an OS hard-time quota.
    """
    root=plain(root);require(type(free_bytes) is int and free_bytes>=MIN_FREE_BYTES,'acquisition free disk reserve')
    cache=plain(root/'cargo-home');require(cache.is_dir(),'cache directory required');count=0;total=0;directories=0;entries=0
    pending=[cache]
    while pending:
        if checkpoint is not None:checkpoint()
        directory=plain(pending.pop());directories+=1
        require(directories<=MAX_CACHE_FILES,'acquisition directory budget')
        with os.scandir(directory) as iterator:
            for entry in iterator:
                entries+=1
                if entries % 128 == 0 and checkpoint is not None:checkpoint()
                p=plain(Path(entry.path))
                # rustup's installed cargo/rustc shims are known reparse entries
                # (native run 41127962926). Never traverse or execute them here.
                # The bin directory itself must be ordinary; real toolchain files
                # are independently hashed/queried. Volume free covers bin too.
                if directory==cache and entry.name=='bin':
                    require(entry.is_dir(follow_symlinks=False),'installed tool directory required')
                    continue
                if entry.is_dir(follow_symlinks=False):
                    require(directories+len(pending)<MAX_CACHE_FILES,'acquisition directory budget')
                    pending.append(p)
                else:
                    require(p.is_file(),'cache nonregular file')
                    count+=1;total+=p.stat().st_size
                    require(count<=MAX_CACHE_FILES and total<=MAX_CACHE_BYTES,'acquisition cache budget')
    if checkpoint is not None:checkpoint()
    return dict(files=count,bytes=total,max_bytes=MAX_CACHE_BYTES,max_files=MAX_CACHE_FILES,continuous_disk_enforcement=False,cache_excludes_installed_bin=True)
