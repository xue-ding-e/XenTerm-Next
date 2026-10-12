#!/usr/bin/env python3
"""Review-first fork branch restacking/archival. Default is read-only preflight."""
import argparse, hashlib, json, pathlib, re, subprocess
P=pathlib.Path(__file__).resolve().parent
REPOSITORY='xue-ding-e/XenTerm'
URL='https://github.com/'+REPOSITORY+'.git'

def run(*args):return subprocess.check_output(list(args),text=True).strip()
def gh(path):return json.loads(run('gh','api',path))
def fail(message):raise SystemExit('STOP: '+message)
def refs():return {r:s for s,r in (line.split() for line in run('git','ls-remote','--heads','--tags',URL).splitlines()) if not r.endswith('^{}')}
def digest(path):
    h=hashlib.sha256()
    with path.open('rb') as f:
        for b in iter(lambda:f.read(1024*1024),b''):h.update(b)
    return h.hexdigest()
def open_pr_refs():
    heads=set()
    for repo in [REPOSITORY,'ixbaicn/XenTerm']:
        page=1
        while True:
            prs=gh('repos/'+repo+'/pulls?state=open&per_page=100&page='+str(page))
            for pr in prs:
                h=pr.get('head',{})
                if (h.get('repo') or {}).get('full_name')==REPOSITORY:heads.add('refs/heads/'+h['ref'])
                b=pr.get('base',{})
                if (b.get('repo') or {}).get('full_name')==REPOSITORY:heads.add('refs/heads/'+b['ref'])
            if len(prs)<100:break
            page+=1
    return heads

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--plan',default=str(P/'fork-plan.json'));ap.add_argument('--git-dir',required=True);ap.add_argument('--execute',action='store_true');ap.add_argument('--rollback',action='store_true');ap.add_argument('--confirm-repository');ap.add_argument('--acknowledge-history-rewrite',action='store_true');ap.add_argument('--acknowledge-branch-cleanup',action='store_true');ap.add_argument('--acknowledge-collaboration-freeze',action='store_true');a=ap.parse_args()
    plan=json.loads(pathlib.Path(a.plan).read_text())
    if plan.get('repository')!=REPOSITORY or plan.get('review_status')!='ready':fail('This must be the reviewed, complete fork plan.')
    expected=plan['expected_remote_refs'];actions=plan['actions'];original_actions=actions
    if not actions or not any(x.get('new')!=x.get('expected_old') for x in actions):fail('A non-empty, effective explicit ref action list is mandatory.')
    if plan.get('fork_bundle') not in {b.get('file') for b in plan.get('backups',[])}:fail('The restoration bundle must be checksum-verified in the backup list.')
    if a.rollback:
        rp=P/'fork-cutover-receipt.json'
        if not rp.is_file():fail('Verified fork cutover receipt required.')
        receipt=json.loads(rp.read_text())
        if receipt.get('before')!=expected or receipt.get('actions')!=actions:fail('Receipt does not match this plan.')
        desired=dict(expected)
        for x in actions:
            if x['new'] is None:desired.pop(x['ref'])
            else:desired[x['ref']]=x['new']
        if receipt.get('after')!=desired:fail('Receipt has unexpected destinations.')
        expected=desired;actions=[{'ref':x['ref'],'expected_old':x['new'],'new':x['expected_old']} for x in actions]
    seen=set()
    for x in actions:
        ref=x['ref']
        if ref in seen or not ref.startswith('refs/heads/'):fail('Duplicate or non-branch ref.')
        seen.add(ref);subprocess.check_call(['git','check-ref-format',ref])
        for value in [x['expected_old'],x['new']]:
            if value is not None and not re.fullmatch('[0-9a-f]{40}',value):fail('Unresolved SHA placeholder.')
        if x['expected_old'] is None and not a.rollback:fail('Forward plan may only migrate explicitly snapshotted existing refs.')
        if ref=='refs/heads/main' and x['new'] is None:fail('main cannot be deleted.')
        if expected.get(ref)!=x['expected_old']:fail('Action does not match the expected snapshot.')
    def g(*args):return run('git','--git-dir',a.git_dir,*args)
    for b in plan['backups']:
        if pathlib.Path(b['file']).name!=b['file'] or not b['file'].endswith('.bundle'):fail('Backup filenames must be local bundle basenames.')
        f=P/b['file']
        if not f.is_file() or digest(f)!=b['sha256']:fail('Backup missing or corrupt.')
        g('bundle','verify',str(f))
    backup_refs={r:s for s,r in (line.split() for line in g('bundle','list-heads',str(P/plan['fork_bundle'])).splitlines())}
    for x in original_actions:
        if backup_refs.get(x['ref'])!=x['expected_old']:fail('Original ref is not recoverable from the verified fork bundle.')
    before=refs()
    if before!=expected:fail('Fork heads/tags changed. Refresh and review the entire plan, not just expected SHAs.')
    desired=dict(expected)
    for x in actions:
        if x['new'] is None:desired.pop(x['ref'])
        else:
            if g('cat-file','-t',x['new'])!='commit':fail('A branch destination is not a commit.')
            desired[x['ref']]=x['new']
    if not a.rollback:
        commits=sorted({s for r,s in desired.items() if r.startswith('refs/heads/')})
        for s in commits:
            if g('rev-list','--count','--merges',s)!='0':fail('A retained branch still contains merge commits.')
        for i,s in enumerate(commits):
            for t in commits[i+1:]:
                common=g('merge-base',s,t)
                if common not in [s,t]:fail('Retained branches do not lie on one comparable ancestor chain.')
        for ref,tree in plan['expected_target_trees'].items():
            if ref not in desired or g('rev-parse',desired[ref]+'^{tree}')!=tree:fail('A target tree does not match the tested integration result.')
    meta=gh('repos/'+REPOSITORY)
    if not meta.get('permissions',{}).get('push'):fail('Operator lacks fork write permission.')
    if gh('repos/'+REPOSITORY+'/rulesets'):fail('Rulesets require administrator review; no bypass is attempted.')
    protected=[];page=1
    while True:
        bs=gh('repos/'+REPOSITORY+'/branches?per_page=100&page='+str(page))
        protected += ['refs/heads/'+b['name'] for b in bs if b.get('protected')]
        if len(bs)<100:break
        page+=1
    if set(protected)&seen:fail('A target branch is protected; review the constraints separately.')
    deleting={x['ref'] for x in actions if x['new'] is None}
    if deleting&open_pr_refs():fail('A branch proposed for archival is referenced by an open PR.')
    print('Fork preflight passed. Default is read-only; no branches changed.')
    if not a.execute:return
    if a.confirm_repository!=REPOSITORY or not a.acknowledge_history_rewrite or not a.acknowledge_branch_cleanup or not a.acknowledge_collaboration_freeze:fail('Explicit scope and collaboration-freeze acknowledgements required.')
    if deleting&open_pr_refs():fail('An open PR now references a branch proposed for deletion.')
    if refs()!=expected:fail('Remote changed during preflight.')
    command=['git','--git-dir',a.git_dir,'push','--atomic','--no-follow-tags']
    command += ['--force-with-lease='+x['ref']+':'+(x['expected_old'] or '') for x in actions]
    command += [URL]+[(x['new'] or '')+':'+x['ref'] for x in actions]
    (P/('fork-rollback-intent.json' if a.rollback else 'fork-cutover-intent.json')).write_text(json.dumps({'before':expected,'actions':actions,'verified_completed':False},indent=2)+'\n')
    subprocess.check_call(command)
    after=refs()
    if after!=desired:fail('Post-push refs differ; inspect before further actions.')
    (P/('fork-rollback-receipt.json' if a.rollback else 'fork-cutover-receipt.json')).write_text(json.dumps({'repository':REPOSITORY,'before':expected,'after':after,'actions':actions},indent=2)+'\n')
    print('Verified fork refs match the exact plan. PR base/head coordination and upstream approvals remain separate.')
if __name__=='__main__':main()
