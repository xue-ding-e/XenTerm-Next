#!/usr/bin/env python3
"""Offline, snapshot-preserving history linearization. Never pushes Git refs."""
import argparse, datetime, hashlib, json, os, pathlib, subprocess
P=pathlib.Path(__file__).resolve().parent

def git(repo,*args,input=None,env=None):
    return subprocess.check_output(['git','-C',str(repo),*args],input=input,env=env).decode().strip()

def refs(repo):
    return dict(line.split(' ',1) for line in git(repo,'for-each-ref','--format=%(refname) %(objectname)').splitlines())

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--committer-name',required=True);ap.add_argument('--committer-email',required=True);ap.add_argument('--committer-date',required=True);a=ap.parse_args()
    repo=P/'upstream.git'; old=git(repo,'rev-parse','refs/heads/main'); before=refs(repo)
    sequence=git(repo,'rev-list','--first-parent','--reverse',old).splitlines(); snapshots=[];mapping={};new_parent=None
    for s in sequence:
        tree=git(repo,'rev-parse',s+'^{tree}'); parents=git(repo,'show','-s','--format=%P',s).split()
        name,email,date=git(repo,'show','-s','--format=%an%x00%ae%x00%aI',s).split('\0')
        message=git(repo,'show','-s','--format=%B',s)
        newly_reachable=git(repo,'rev-list',s,*(['--not',parents[0]] if parents else [])).splitlines()
        coauthors=set()
        message+='\n\nOriginal-Commit: '+s+'\nHistory-Migration: first-parent snapshot; original signatures remain in the backup\n'
        if len(parents)>1:
            message+='\nIntegrated original commits (preserved in verified backup):\n'
            for c in newly_reachable:
                if c==s:continue
                message+='- '+c+' '+git(repo,'show','-s','--format=%s',c)+'\n'
                identity=git(repo,'show','-s','--format=%an <%ae>',c)
                if identity!=f'{name} <{email}>':coauthors.add(identity)
            message+='\n'
        for identity in sorted(coauthors):
            if 'Co-authored-by: '+identity not in message:message+='Co-authored-by: '+identity+'\n'
        env=os.environ.copy();env.update(GIT_AUTHOR_NAME=name,GIT_AUTHOR_EMAIL=email,GIT_AUTHOR_DATE=date,GIT_COMMITTER_NAME=a.committer_name,GIT_COMMITTER_EMAIL=a.committer_email,GIT_COMMITTER_DATE=a.committer_date)
        new=git(repo,'commit-tree',tree,*(['-p',new_parent] if new_parent else []),input=message.encode(),env=env)
        assert git(repo,'rev-parse',new+'^{tree}')==tree
        for c in newly_reachable:
            assert c not in mapping
            mapping[c]={'new':new,'integration_snapshot':s,'kind':'snapshot' if c==s else 'absorbed-into-integration'}
        snapshots.append({'old':s,'new':new,'tree':tree,'old_parents':parents,'author':f'{name} <{email}>','coauthors':sorted(coauthors),'absorbed_commits':len(newly_reachable)-1})
        new_parent=new
    assert set(mapping)==set(git(repo,'rev-list',old).splitlines())
    assert git(repo,'diff','--stat',old,new_parent)==''
    assert git(repo,'rev-list','--count','--merges',new_parent)=='0'
    candidate_ref='refs/heads/history/linear-main-20261012'
    existing=before.get(candidate_ref,'0'*40)
    git(repo,'update-ref',candidate_ref,new_parent,existing)
    after=refs(repo);assert {k:v for k,v in after.items() if k!=candidate_ref}=={k:v for k,v in before.items() if k!=candidate_ref}
    (P/'snapshot-map.json').write_text(json.dumps(snapshots,ensure_ascii=False,indent=2)+'\n')
    (P/'all-commit-map.json').write_text(json.dumps(mapping,ensure_ascii=False,indent=2)+'\n')
    summary={'old_main':old,'candidate_main':new_parent,'candidate_ref':candidate_ref,'tree':git(repo,'rev-parse',old+'^{tree}'),'original_commits':len(mapping),'linear_commits':len(snapshots),'original_first_parent_merges':sum(len(s['old_parents'])>1 for s in snapshots),'candidate_merges':0,'every_snapshot_tree_equal':True,'original_refs_unchanged':True,'committer':f'{a.committer_name} <{a.committer_email}>','committer_date':a.committer_date,'remote_modified':False}
    (P/'verification.json').write_text(json.dumps(summary,ensure_ascii=False,indent=2)+'\n')
    git(repo,'bundle','create',str(P/'linear-candidate.bundle'),candidate_ref)
    git(repo,'bundle','verify',str(P/'linear-candidate.bundle'))
    print(json.dumps(summary,ensure_ascii=False,indent=2))
if __name__=='__main__':main()
