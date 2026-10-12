#!/usr/bin/env python3
"""Review-first exact-lease ref migration. Default is read-only preflight."""
import argparse, hashlib, json, pathlib, re, subprocess, sys
P=pathlib.Path(__file__).resolve().parent

def run(args,**kw):return subprocess.check_output(args,**kw).decode().strip()
def remote_refs(url):
    out=run(['git','ls-remote','--heads','--tags',url]);return {ref:sha for sha,ref in (x.split() for x in out.splitlines()) if not ref.endswith('^{}')}
def fail(message):raise SystemExit('STOP: '+message)
def gh(path):return json.loads(run(['gh','api',path]))
def release_fingerprint(release):
    fields=['id','tag_name','target_commitish','name','body','draft','prerelease','created_at','updated_at','published_at']
    result={k:release.get(k) for k in fields}
    asset_fields=['id','name','size','digest','state','content_type','created_at','updated_at']
    result['assets']=sorted([{k:a.get(k) for k in asset_fields} for a in release.get('assets',[])],key=lambda a:(a['id'] or 0,a['name'] or ''))
    return result

def assert_release_snapshot(repo):
    saved=release_fingerprint(json.loads((P/'beta-release-full.json').read_text()))
    observed=release_fingerprint(gh('repos/'+repo+'/releases/tags/beta'))
    if observed!=saved:fail('The beta release or its assets changed without a Git ref change. Rebuild and review the complete backup/plan.')
    return observed

def assert_release_frozen(repo):
    workflow=gh('repos/'+repo+'/actions/workflows/release.yml')
    if workflow.get('state')!='disabled_manually':fail('Release workflow is not explicitly frozen by an administrator.')
    for status in ['queued','in_progress','waiting','pending','requested']:
        release_runs=gh('repos/'+repo+'/actions/workflows/release.yml/runs?status='+status+'&per_page=1').get('workflow_runs',[])
        if release_runs:fail('Release workflow has unfinished runs; freeze/drain before migration.')

def validate_plan(plan):
    if plan.get('repository')!='ixbaicn/XenTerm':fail('Unexpected repository; this reviewed plan is upstream-only.')
    actions=plan.get('actions',[])
    if not actions:fail('No actions.')
    refs=set()
    for x in actions:
        ref=x['ref'];old=x['expected_old'];new=x['new']
        if ref in refs:fail('Duplicate ref.')
        refs.add(ref)
        if not ref.startswith(('refs/heads/','refs/tags/')):fail('Only explicitly reviewed heads/tags are permitted.')
        subprocess.check_call(['git','check-ref-format',ref])
        if not re.fullmatch('[0-9a-f]{40}',old):fail('An explicit old SHA is mandatory.')
        if not new:fail('Deletion is deliberately disabled in this upstream cutover script.')
        if not re.fullmatch('[0-9a-f]{40}',new):fail('Invalid destination SHA.')
    if refs!={'refs/heads/main','refs/tags/beta'}:fail('This phase only migrates main and beta; review other phases independently.')

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--plan',default=str(P/'upstream-plan.json'));ap.add_argument('--git-dir',required=True);ap.add_argument('--execute',action='store_true');ap.add_argument('--rollback',action='store_true');ap.add_argument('--confirm-repository');ap.add_argument('--acknowledge-history-rewrite',action='store_true');ap.add_argument('--acknowledge-release-impact',action='store_true');a=ap.parse_args()
    plan=json.loads(pathlib.Path(a.plan).read_text());validate_plan(plan)
    if a.rollback:
        receipt_path=P/'cutover-receipt.json'
        if not receipt_path.is_file():fail('A verified cutover receipt is required for rollback.')
        receipt=json.loads(receipt_path.read_text())
        if receipt['repository']!=plan['repository'] or receipt['before']!=plan['expected_remote_refs'] or receipt['actions']!=plan['actions']:fail('Receipt does not match this original plan.')
        expected_after=dict(plan['expected_remote_refs']);expected_after.update({x['ref']:x['new'] for x in plan['actions']})
        if receipt['after']!=expected_after:fail('Receipt has an unexpected destination state.')
        plan=dict(plan,expected_remote_refs=receipt['after'],actions=[{'ref':x['ref'],'expected_old':x['new'],'new':x['expected_old']} for x in plan['actions']])
        validate_plan(plan)
    repo=plan['repository'];url='https://github.com/'+repo+'.git'
    def g(*args):return run(['git','--git-dir',a.git_dir,*args])
    expected=plan['expected_remote_refs'];observed=remote_refs(url)
    if observed!=expected:fail('Remote heads/tags changed. Rebuild the backup and plan; do not merely replace expected SHAs.')
    for item in plan['actions']:
        if observed.get(item['ref'])!=item['expected_old']:fail('Unexpected ref SHA.')
        if g('cat-file','-t',item['new'])!='commit':fail('Candidate must be a commit; annotated tag replacement is not supported.')
    oldmain=expected['refs/heads/main'];newmain=next(x['new'] for x in plan['actions'] if x['ref']=='refs/heads/main')
    if g('rev-parse',oldmain+'^{tree}')!=g('rev-parse',newmain+'^{tree}'):fail('main tree mismatch.')
    if not a.rollback and g('rev-list','--count','--merges',newmain)!='0':fail('Candidate main is not linear.')
    if any(x['new']!=newmain for x in plan['actions']):fail('The reviewed beta and main snapshots must match.')
    for record in plan['backups']:
        p=P/record['file']
        if not p.is_file() or hashlib.sha256(p.read_bytes()).hexdigest()!=record['sha256']:fail('Backup missing or checksum mismatch.')
        g('bundle','verify',str(p))
    asset_manifest=P/'release-assets-verified.json'
    if not asset_manifest.is_file():fail('Release asset backup has not been verified.')
    asset_records=json.loads(asset_manifest.read_text())
    expected_assets=json.loads((P/'beta-release.json').read_text())['assets']
    if {x['name'] for x in asset_records}!={x['name'] for x in expected_assets}:fail('Incomplete release asset backup.')
    for record in asset_records:
        asset=P/'release-assets'/record['name']
        if not asset.is_file() or hashlib.sha256(asset.read_bytes()).hexdigest()!=record['sha256']:fail('Release asset backup missing or corrupt.')
    metadata=gh('repos/'+repo)
    if not metadata.get('permissions',{}).get('push'):fail('The authenticated operator lacks upstream write permission.')
    rules=gh('repos/'+repo+'/rulesets')
    if rules:fail('Repository rulesets require administrator review; this script will not bypass or change them.')
    branches=gh('repos/'+repo+'/branches/main')
    if branches.get('protected'):fail('main is protected; administrator must review constraints, not bypass them here.')
    assert_release_snapshot(repo)
    assert_release_frozen(repo)
    print('Preflight passed. No refs changed unless --execute is supplied.')
    if not a.execute:return
    if a.confirm_repository!=repo or not a.acknowledge_history_rewrite or not a.acknowledge_release_impact:fail('Explicit execution acknowledgements required.')
    assert_release_snapshot(repo)
    assert_release_frozen(repo)
    if remote_refs(url)!=expected:fail('Remote changed during preflight.')
    cmd=['git','--git-dir',a.git_dir,'push','--atomic','--no-follow-tags']
    cmd += ['--force-with-lease='+x['ref']+':'+x['expected_old'] for x in plan['actions']]
    cmd += [url]+[x['new']+':'+x['ref'] for x in plan['actions']]
    (P/('rollback-intent.json' if a.rollback else 'cutover-intent.json')).write_text(json.dumps({'repository':repo,'expected_before':expected,'actions':plan['actions'],'verified_completed':False},indent=2)+'\n')
    subprocess.check_call(cmd)
    desired=dict(expected);desired.update({x['ref']:x['new'] for x in plan['actions']})
    after=remote_refs(url)
    if after!=desired:fail('Post-push refs differ from the exact plan; inspect before further actions.')
    receipt={'repository':repo,'before':expected,'after':after,'actions':plan['actions'],'release_workflow_left_disabled':True}
    (P/('rollback-receipt.json' if a.rollback else 'cutover-receipt.json')).write_text(json.dumps(receipt,indent=2)+'\n')
    print('Refs match the plan. Release remains frozen; restore or build/verify matching release artifacts before coordinated workflow resume.')
if __name__=='__main__':main()
