#!/usr/bin/env python3
"""Create verified local backups from public repositories. Never changes remote state."""
import argparse, hashlib, json, pathlib, re, shutil, subprocess, sys
from admin_apply import release_fingerprint
SOURCE=pathlib.Path(__file__).resolve().parent
UPSTREAM='https://github.com/ixbaicn/XenTerm.git'
FORK='https://github.com/xue-ding-e/XenTerm.git'

def run(*args):return subprocess.check_output(list(args),text=True).strip()
def refs(url):
    return {r:s for s,r in (line.split() for line in run('git','ls-remote','--heads','--tags',url).splitlines()) if not r.endswith('^{}')}
def sha(path):
    h=hashlib.sha256()
    with path.open('rb') as f:
        for chunk in iter(lambda:f.read(1024*1024),b''):h.update(chunk)
    return h.hexdigest()
def stop(message):raise SystemExit('STOP: '+message)
def main():
    ap=argparse.ArgumentParser();ap.add_argument('--directory',default='xenterm-migration-prepared');ap.add_argument('--handoff-commit',required=True);a=ap.parse_args()
    if not re.fullmatch('[0-9a-f]{40}',a.handoff_commit):stop('A pinned full handoff SHA is required.')
    for line in (SOURCE/'SOURCE_SHA256SUMS').read_text().splitlines():
        expected_hash,name=line.split('  ',1)
        if pathlib.Path(name).is_absolute() or '..' in pathlib.Path(name).parts or sha(SOURCE/name)!=expected_hash:stop('Source package checksum mismatch.')
    if run('git','-C',str(SOURCE),'rev-parse','HEAD')!=a.handoff_commit:stop('Check out the pinned handoff commit before preparation.')
    subprocess.check_call(['git','-C',str(SOURCE),'diff','--exit-code','HEAD','--','.'])
    target=pathlib.Path(a.directory).resolve()
    if target.exists():stop('Output directory already exists. Use a new directory; nothing will be overwritten.')
    template=json.loads((SOURCE/'upstream-plan.json').read_text());expected=template['expected_remote_refs']
    verification=json.loads((SOURCE/'verification.json').read_text());candidate=verification['candidate_main'];candidate_ref=verification['candidate_ref']
    def current_release():return json.loads(run('curl','--fail','--silent','--show-error','--location','https://api.github.com/repos/ixbaicn/XenTerm/releases/tags/beta'))
    saved_release=release_fingerprint(json.loads((SOURCE/'beta-release-full.json').read_text()))
    if release_fingerprint(current_release())!=saved_release:stop('Beta release/asset metadata changed; request a newly verified package.')
    current=refs(UPSTREAM);fork_before=refs(FORK)
    fork_template=json.loads((SOURCE/'fork-plan.template.json').read_text())
    if fork_before.get(fork_template['handoff_ref'])!=a.handoff_commit:stop('Pinned handoff ref changed.')
    handoff_tree=run('git','-C',str(SOURCE),'rev-parse',a.handoff_commit+'^{tree}')
    def resolve(value):
        if isinstance(value,str):return {'$HANDOFF_HEAD':a.handoff_commit,'$HANDOFF_TREE':handoff_tree}.get(value,value)
        if isinstance(value,list):return [resolve(v) for v in value]
        if isinstance(value,dict):return {k:resolve(v) for k,v in value.items()}
        return value
    fork_template=resolve(fork_template)
    if fork_before!=fork_template['expected_remote_refs']:stop('Fork refs changed from the reviewed complete migration plan.')
    if current!=expected:stop('Upstream heads/tags changed since the reviewed candidate; request a newly verified migration package.')
    if fork_before.get(candidate_ref)!=candidate:stop('The public candidate ref no longer matches the reviewed SHA.')
    target.mkdir()
    files=['admin_apply.py','backup_release_assets.py','test_migration.py','build_candidate.py','upstream-plan.json','verification.json','snapshot-map.json','all-commit-map.json','fork-ref-audit.json','fork-cleanup-proposal.json','beta-release.json','beta-release-full.json','管理员迁移说明.md','fork_apply.py','test_fork_migration.py','fork-plan.template.json','stack-pr93-map.json','stack-pr95-map.json','stack-validation-worker.json']
    for name in files:shutil.copy2(SOURCE/name,target/name)
    up=target/'migration.git';fork=target/'fork-backup.git'
    subprocess.check_call(['git','clone','--mirror',UPSTREAM,str(up)])
    subprocess.check_call(['git','clone','--mirror',FORK,str(fork)])
    def g(repo,*args):return run('git','-C',str(repo),*args)
    if g(up,'rev-parse','refs/heads/main')!=expected['refs/heads/main'] or g(up,'rev-parse','refs/tags/beta')!=expected['refs/tags/beta']:stop('Cloned upstream does not match the frozen snapshot.')
    for repo in [up,fork]:
        if g(repo,'rev-parse','--is-shallow-repository')!='false':stop('Shallow repository is not a complete backup.')
        g(repo,'fsck','--full')
    # The original upstream backup is created before adding candidate refs.
    g(up,'bundle','create',str(target/'upstream-before.bundle'),'--all')
    g(fork,'bundle','create',str(target/'fork-before.bundle'),'--all')
    g(up,'fetch',FORK,candidate_ref+':'+candidate_ref)
    if g(up,'rev-parse',candidate_ref)!=candidate:stop('Fetched candidate changed.')
    if g(up,'rev-parse',candidate+'^{tree}')!=verification['tree'] or g(up,'rev-parse',expected['refs/heads/main']+'^{tree}')!=verification['tree']:stop('Final tree differs from the reviewed snapshot.')
    if g(up,'rev-list','--count','--merges',candidate)!='0':stop('Candidate is not linear.')
    snapshots=json.loads((target/'snapshot-map.json').read_text());mapping=json.loads((target/'all-commit-map.json').read_text())
    for item in snapshots:
        if g(up,'rev-parse',item['old']+'^{tree}')!=item['tree'] or g(up,'rev-parse',item['new']+'^{tree}')!=item['tree']:stop('Per-snapshot tree mismatch.')
    if set(g(up,'rev-list',expected['refs/heads/main']).splitlines())!=set(mapping):stop('Original commit mapping is incomplete.')
    g(up,'bundle','create',str(target/'linear-candidate.bundle'),candidate_ref)
    for name in ['upstream-before.bundle','fork-before.bundle','linear-candidate.bundle']:g(up,'bundle','verify',str(target/name))
    if refs(UPSTREAM)!=current or refs(FORK)!=fork_before:stop('A remote changed during backup; keep these files for inspection, but do not proceed.')
    template['backups']=[{'file':name,'sha256':sha(target/name)} for name in ['upstream-before.bundle','fork-before.bundle']]
    (target/'upstream-plan.json').write_text(json.dumps(template,indent=2)+'\n')
    fork_template['backups']=[{'file':'fork-before.bundle','sha256':sha(target/'fork-before.bundle')}]
    (target/'fork-plan.json').write_text(json.dumps(fork_template,indent=2)+'\n')
    (target/'fork-refs-at-preparation.json').write_text(json.dumps(fork_before,indent=2)+'\n')
    subprocess.check_call([sys.executable,str(target/'backup_release_assets.py')])
    if release_fingerprint(current_release())!=saved_release:stop('Beta release changed during backup; do not proceed.')
    if refs(UPSTREAM)!=current or refs(FORK)!=fork_before:stop('A remote changed during asset download; do not proceed.')
    subprocess.check_call([sys.executable,str(target/'test_migration.py')])
    subprocess.check_call([sys.executable,str(target/'test_fork_migration.py')])
    (target/'preparation-receipt.json').write_text(json.dumps({'upstream_refs':current,'fork_refs':fork_before,'candidate':candidate,'every_snapshot_tree_verified':True,'assets_verified':12,'remote_changes':False},indent=2)+'\n')
    print('Prepared: '+str(target))
    print('Next, an authorized administrator coordinates release freeze, then runs:')
    print('  cd '+str(target))
    print('  python admin_apply.py --git-dir migration.git')
    print('After coordinated upstream cutover, the fork owner separately preflights:')
    print('  python fork_apply.py --git-dir fork-backup.git')
if __name__=='__main__':main()
