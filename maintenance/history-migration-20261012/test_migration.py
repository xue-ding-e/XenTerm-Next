import contextlib, importlib.util, io, json, os, pathlib, subprocess, sys, tempfile, unittest, hashlib
from unittest import mock
P=pathlib.Path(__file__).resolve().parent

def load(name):
    spec=importlib.util.spec_from_file_location(name,P/(name+'.py'));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m
class PlanTests(unittest.TestCase):
    def setUp(self):
        self.m=load('admin_apply');self.plan={'repository':'ixbaicn/XenTerm','actions':[{'ref':'refs/heads/main','expected_old':'a'*40,'new':'b'*40},{'ref':'refs/tags/beta','expected_old':'a'*40,'new':'b'*40}]}
    def test_valid_plan(self): self.m.validate_plan(self.plan)
    def test_wrong_repository(self):
        self.plan['repository']='xue-ding-e/XenTerm'
        with self.assertRaises(SystemExit):self.m.validate_plan(self.plan)
    def test_delete_rejected(self):
        self.plan['actions'][0]['new']=''
        with self.assertRaises(SystemExit):self.m.validate_plan(self.plan)
    def test_unknown_ref_rejected(self):
        self.plan['actions'][0]['ref']='refs/pull/93/head'
        with self.assertRaises(SystemExit):self.m.validate_plan(self.plan)
    def test_duplicate_ref_rejected(self):
        self.plan['actions'][1]=self.plan['actions'][0].copy()
        with self.assertRaises(SystemExit):self.m.validate_plan(self.plan)
    def test_missing_exact_old_sha_rejected(self):
        self.plan['actions'][0]['expected_old']='main'
        with self.assertRaises(SystemExit):self.m.validate_plan(self.plan)
    def test_unreviewed_branch_rejected(self):
        self.plan['actions'][1]['ref']='refs/heads/other'
        with self.assertRaises(SystemExit):self.m.validate_plan(self.plan)
class SnapshotTests(unittest.TestCase):
    def test_conflict_resolution_and_attribution_survive(self):
        with tempfile.TemporaryDirectory() as t:
            p=pathlib.Path(t);r=p/'upstream.git';r.mkdir()
            env=os.environ.copy();env.update(GIT_AUTHOR_NAME='Main Author',GIT_AUTHOR_EMAIL='main@example.invalid',GIT_COMMITTER_NAME='Test Committer',GIT_COMMITTER_EMAIL='test@example.invalid')
            def g(*args,check=True):
                x=subprocess.run(['git','-C',str(r),*args],env=env,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
                if check and x.returncode:raise AssertionError(x.stderr)
                return x
            g('init','-b','main');(r/'file').write_text('root\n');g('add','file');g('commit','-m','root')
            g('checkout','-b','side');(r/'file').write_text('side\n');g('add','file');g('commit','--author','Side Author <side@example.invalid>','-m','side feature')
            side=g('rev-parse','HEAD').stdout.strip();g('checkout','main');(r/'file').write_text('main\n');g('add','file');g('commit','-m','main change')
            self.assertNotEqual(g('merge','side',check=False).returncode,0)
            (r/'file').write_text('manual merged resolution, neither original side\n');g('add','file');g('commit','-m','merge reviewed feature')
            old=g('rev-parse','HEAD').stdout.strip();oldtree=g('rev-parse','HEAD^{tree}').stdout.strip()
            m=load('build_candidate');m.P=p;argv=sys.argv;sys.argv=['build_candidate.py','--committer-name','Migration Operator','--committer-email','migration@example.invalid','--committer-date','2026-10-12T16:40:00+00:00']
            try:
                with contextlib.redirect_stdout(io.StringIO()):m.main()
            finally:sys.argv=argv
            summary=json.loads((p/'verification.json').read_text());new=summary['candidate_main']
            self.assertEqual(g('rev-parse',new+'^{tree}').stdout.strip(),oldtree)
            self.assertEqual(g('rev-list','--merges',new).stdout.strip(),'')
            self.assertEqual(g('rev-parse','main').stdout.strip(),old)
            body=g('show','-s','--format=%B',new).stdout
            self.assertIn('Co-authored-by: Side Author <side@example.invalid>',body)
            self.assertIn(side,body)
            self.assertIn('manual merged resolution',g('show',new+':file').stdout)
            mapping=json.loads((p/'all-commit-map.json').read_text())
            self.assertEqual(mapping[side]['kind'],'absorbed-into-integration')
            self.assertEqual(mapping[side]['new'],new)
            self.assertEqual(summary['original_commits'],4);self.assertEqual(summary['linear_commits'],3)
class PreflightTests(unittest.TestCase):
    def exercise(self, case):
        m=load('admin_apply')
        with tempfile.TemporaryDirectory() as t:
            p=pathlib.Path(t);m.P=p;(p/'release-assets').mkdir();(p/'release-assets'/'asset').write_bytes(b'asset')
            digest=hashlib.sha256(b'asset').hexdigest()
            (p/'release-assets-verified.json').write_text(json.dumps([{'name':'asset','sha256':digest}]))
            (p/'beta-release.json').write_text(json.dumps({'assets':[{'name':'asset'}]}))
            release={'id':1,'tag_name':'beta','assets':[{'id':2,'name':'asset','size':5,'digest':'sha256:'+digest,'updated_at':'2026-10-12T00:00:00Z'}]}
            (p/'beta-release-full.json').write_text(json.dumps(release))
            (p/'backup.bundle').write_bytes(b'backup')
            expected={'refs/heads/main':'a'*40,'refs/tags/beta':'a'*40}
            plan={'repository':'ixbaicn/XenTerm','expected_remote_refs':expected,'backups':[{'file':'backup.bundle','sha256':hashlib.sha256(b'backup').hexdigest()}],'actions':[{'ref':r,'expected_old':v,'new':'b'*40} for r,v in expected.items()]}
            (p/'plan.json').write_text(json.dumps(plan))
            calls=[]
            def run(args,**kw):
                calls.append(args)
                if 'cat-file' in args:return 'commit'
                if 'rev-parse' in args:return ('wrong' if case=='tree' and 'a'*40+'^{tree}' in args else 'tree')
                if 'rev-list' in args:return '1' if case=='merge' else '0'
                return ''
            freeze_calls=[0]
            def gh(path):
                if path.endswith('/releases/tags/beta'):
                    if case=='changed-release':return dict(release,id=99)
                    result=json.loads(json.dumps(release))
                    if case=='changed-asset-digest':result['assets'][0]['digest']='sha256:'+'f'*64
                    if case=='changed-asset-size':result['assets'][0]['size']=999
                    return result
                if path.endswith('/rulesets'):return [{'id':1}] if case=='rules' else []
                if path.endswith('/branches/main'):return {'protected':case=='protected'}
                if 'runs?' in path:return {'workflow_runs':[{'status':'in_progress'}] if case=='running' else []}
                if path.endswith('release.yml'):
                    freeze_calls[0]+=1
                    return {'state':'active' if case=='active' or (case=='active-after-preflight' and freeze_calls[0]>1) else 'disabled_manually'}
                return {'permissions':{'push':case!='permission'}}
            if case=='backup':(p/'backup.bundle').write_bytes(b'corrupt')
            argv=['admin_apply.py','--plan',str(p/'plan.json'),'--git-dir','unused']
            remote_expected=expected
            if case in ('execute-pass','execute-missing-acks','active-after-preflight'):
                argv.append('--execute')
                if case in ('execute-pass','active-after-preflight'):argv += ['--confirm-repository','ixbaicn/XenTerm','--acknowledge-history-rewrite','--acknowledge-release-impact']
            if case.startswith('rollback'):
                argv.append('--rollback')
                after={r:'b'*40 for r in expected}
                if case!='rollback-missing':
                    receipt={'repository':'ixbaicn/XenTerm','before':expected,'after':after,'actions':plan['actions']}
                    if case=='rollback-corrupt':receipt['before']={}
                    (p/'cutover-receipt.json').write_text(json.dumps(receipt))
                remote_expected=after
            remote_values=[{},{}] if case=='stale' else ([expected,expected,{r:'b'*40 for r in expected}] if case=='execute-pass' else [remote_expected]*3)
            external_calls=[]
            with mock.patch.object(sys,'argv',argv),mock.patch.object(m,'run',side_effect=run),mock.patch.object(m,'gh',side_effect=gh),mock.patch.object(m,'remote_refs',side_effect=remote_values),mock.patch.object(m.subprocess,'check_call',side_effect=lambda args:external_calls.append(args)),contextlib.redirect_stdout(io.StringIO()):
                if case in ('pass','rollback-pass','execute-pass'):m.main()
                else:
                    with self.assertRaises(SystemExit):m.main()
            self.assertFalse(any('push' in c for c in calls))
            pushes=[c for c in external_calls if 'push' in c]
            if case=='execute-pass':
                self.assertEqual(len(pushes),1)
                self.assertIn('--no-follow-tags',pushes[0]);self.assertIn('--atomic',pushes[0]);self.assertNotIn('--force',pushes[0])
                for ref in expected:self.assertIn('--force-with-lease='+ref+':'+expected[ref],pushes[0])
                self.assertTrue((p/'cutover-receipt.json').is_file())
            else:self.assertEqual(pushes,[])
    def test_explicit_apply_uses_exact_atomic_leases(self):self.exercise('execute-pass')
    def test_apply_requires_explicit_acknowledgements(self):self.exercise('execute-missing-acks')
    def test_dry_run_cannot_push(self):self.exercise('pass')
    def test_rollback_preflight(self):self.exercise('rollback-pass')
    def test_rollback_requires_receipt(self):self.exercise('rollback-missing')
    def test_rollback_rejects_corrupt_receipt(self):self.exercise('rollback-corrupt')
    def test_only_asset_digest_change_rejected(self):self.exercise('changed-asset-digest')
    def test_only_asset_size_change_rejected(self):self.exercise('changed-asset-size')
    def test_release_reenabled_after_preflight_rejected(self):self.exercise('active-after-preflight')
    def test_changed_release_without_ref_change(self):self.exercise('changed-release')
    def test_stale_ref(self):self.exercise('stale')
    def test_corrupt_backup(self):self.exercise('backup')
    def test_tree_mismatch(self):self.exercise('tree')
    def test_non_linear_candidate(self):self.exercise('merge')
    def test_missing_write_permission(self):self.exercise('permission')
    def test_ruleset_rejected(self):self.exercise('rules')
    def test_protected_main_rejected(self):self.exercise('protected')
    def test_active_release_workflow_rejected(self):self.exercise('active')
    def test_unfinished_release_run_rejected(self):self.exercise('running')

class FollowTagsTests(unittest.TestCase):
    def test_real_local_push_positive_and_negative_control(self):
        with tempfile.TemporaryDirectory() as t:
            p=pathlib.Path(t);local=p/'local';local.mkdir()
            env=os.environ.copy();env.update(GIT_AUTHOR_NAME='Test Author',GIT_AUTHOR_EMAIL='test@example.invalid',GIT_COMMITTER_NAME='Test Author',GIT_COMMITTER_EMAIL='test@example.invalid')
            def g(repo,*a):return subprocess.check_output(['git','-C',str(repo),*a],env=env,stderr=subprocess.DEVNULL,text=True).strip()
            g(local,'init','-b','main');(local/'file').write_text('one');g(local,'add','file');g(local,'commit','-m','old');old=g(local,'rev-parse','HEAD')
            g(local,'tag','beta');remotes=[]
            for name in ['red.git','green.git']:
                r=p/name;r.mkdir();g(r,'init','--bare');g(local,'push',str(r),'main','beta');remotes.append(r)
            (local/'file').write_text('two');g(local,'commit','-am','new');new=g(local,'rev-parse','HEAD');g(local,'tag','-a','unplanned-tag','-m','must not escape planned refs');g(local,'config','push.followTags','true')
            for r,no_follow in zip(remotes,[False,True]):
                args=['push','--atomic','--force-with-lease=refs/heads/main:'+old,'--force-with-lease=refs/tags/beta:'+old]
                if no_follow:args.append('--no-follow-tags')
                g(local,*args,str(r),new+':refs/heads/main',new+':refs/tags/beta')
            red=g(remotes[0],'for-each-ref','--format=%(refname)').splitlines();green=g(remotes[1],'for-each-ref','--format=%(refname)').splitlines()
            self.assertIn('refs/tags/unplanned-tag',red)
            self.assertEqual(set(green),{'refs/heads/main','refs/tags/beta'})

if __name__=='__main__':unittest.main()
