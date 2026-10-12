import contextlib,hashlib,importlib.util,io,json,os,pathlib,subprocess,sys,tempfile,unittest
from unittest import mock
P=pathlib.Path(__file__).resolve().parent

def load():
 s=importlib.util.spec_from_file_location('fork_apply',P/'fork_apply.py');m=importlib.util.module_from_spec(s);s.loader.exec_module(m);return m
class ForkMigrationTests(unittest.TestCase):
 def setup_case(self,t):
  p=pathlib.Path(t);local=p/'local';remote=p/'remote.git';local.mkdir();remote.mkdir()
  env=os.environ.copy();env.update(GIT_AUTHOR_NAME='Test',GIT_AUTHOR_EMAIL='test@example.invalid',GIT_COMMITTER_NAME='Test',GIT_COMMITTER_EMAIL='test@example.invalid')
  def g(repo,*args,input=None):return subprocess.check_output(['git','-C',str(repo),*args],input=input,env=env,stderr=subprocess.DEVNULL).decode().strip()
  g(local,'init','-b','main');g(remote,'init','--bare');(local/'file').write_text('root');g(local,'add','file');g(local,'commit','-m','root');oldmain=g(local,'rev-parse','HEAD')
  g(local,'checkout','-b','feature');(local/'file').write_text('feature');g(local,'commit','-am','feature');oldfeature=g(local,'rev-parse','HEAD')
  g(local,'checkout','main');g(local,'checkout','-b','evidence');(local/'evidence').write_text('evidence');g(local,'add','evidence');g(local,'commit','-m','evidence');oldevidence=g(local,'rev-parse','HEAD')
  g(local,'push',str(remote),'main','feature','evidence')
  newmain=g(local,'commit-tree',g(local,'rev-parse',oldmain+'^{tree}'),input=b'linear root\n');newfeature=g(local,'commit-tree',g(local,'rev-parse',oldfeature+'^{tree}'),'-p',newmain,input=b'linear feature\n')
  g(local,'update-ref','refs/heads/candidate-main',newmain);g(local,'update-ref','refs/heads/candidate-feature',newfeature);g(local,'tag','-a','unplanned',newfeature,'-m','must stay local');g(local,'config','push.followTags','true')
  bundle=p/'fork.bundle';g(local,'bundle','create',str(bundle),'--all')
  expected={'refs/heads/main':oldmain,'refs/heads/feature':oldfeature,'refs/heads/evidence':oldevidence}
  plan={'repository':'xue-ding-e/XenTerm','review_status':'ready','expected_remote_refs':expected,'fork_bundle':'fork.bundle','backups':[{'file':'fork.bundle','sha256':hashlib.sha256(bundle.read_bytes()).hexdigest()}],'actions':[{'ref':'refs/heads/main','expected_old':oldmain,'new':newmain},{'ref':'refs/heads/feature','expected_old':oldfeature,'new':newfeature},{'ref':'refs/heads/evidence','expected_old':oldevidence,'new':None}],'expected_target_trees':{'refs/heads/main':g(local,'rev-parse',newmain+'^{tree}'),'refs/heads/feature':g(local,'rev-parse',newfeature+'^{tree}')}}
  (p/'fork-plan.json').write_text(json.dumps(plan));m=load();m.P=p;m.URL=str(remote)
  return p,local,remote,g,m,expected,plan
 def invoke(self,m,p,local,extra,open_pr=False):
  def gh(path):
   if '/rulesets' in path:return []
   if '/branches?' in path:return []
   if '/pulls?' in path:
    if open_pr=='base':return [{'base':{'repo':{'full_name':'xue-ding-e/XenTerm'},'ref':'evidence'}}]
    return [{'head':{'repo':{'full_name':'xue-ding-e/XenTerm'},'ref':'evidence'}}] if open_pr else []
   return {'permissions':{'push':True}}
  args=['fork_apply.py','--plan',str(p/'fork-plan.json'),'--git-dir',str(local/'.git')]+extra
  with mock.patch.object(m,'gh',side_effect=gh),mock.patch.object(sys,'argv',args),contextlib.redirect_stdout(io.StringIO()):m.main()
 def test_actual_atomic_apply_and_deleted_ref_rollback(self):
  with tempfile.TemporaryDirectory() as t:
   p,local,remote,g,m,expected,plan=self.setup_case(t)
   self.invoke(m,p,local,[]);self.assertEqual(m.refs(),expected)
   flags=['--execute','--confirm-repository','xue-ding-e/XenTerm','--acknowledge-history-rewrite','--acknowledge-branch-cleanup','--acknowledge-collaboration-freeze']
   self.invoke(m,p,local,flags)
   self.assertEqual(m.refs(),{x['ref']:x['new'] for x in plan['actions'] if x['new']})
   self.assertNotIn('refs/tags/unplanned',m.refs())
   self.invoke(m,p,local,['--rollback']+flags)
   self.assertEqual(m.refs(),expected)
 def test_open_pr_branch_cannot_be_deleted(self):
  with tempfile.TemporaryDirectory() as t:
   p,local,remote,g,m,expected,plan=self.setup_case(t)
   with self.assertRaises(SystemExit):self.invoke(m,p,local,[],open_pr=True)
   self.assertEqual(m.refs(),expected)
 def test_apply_without_acknowledgements_is_read_only(self):
  with tempfile.TemporaryDirectory() as t:
   p,local,remote,g,m,expected,plan=self.setup_case(t)
   with self.assertRaises(SystemExit):self.invoke(m,p,local,['--execute'])
   self.assertEqual(m.refs(),expected)
 def test_changed_remote_is_rejected(self):
  with tempfile.TemporaryDirectory() as t:
   p,local,remote,g,m,expected,plan=self.setup_case(t)
   g(local,'push',str(remote),'main:refs/heads/new-concurrent-branch');before=m.refs()
   with self.assertRaises(SystemExit):self.invoke(m,p,local,[])
   self.assertEqual(m.refs(),before)
 def test_open_pr_base_branch_cannot_be_deleted(self):
  with tempfile.TemporaryDirectory() as t:
   p,local,remote,g,m,expected,plan=self.setup_case(t)
   with self.assertRaises(SystemExit):self.invoke(m,p,local,[],open_pr='base')
   self.assertEqual(m.refs(),expected)
 def test_empty_action_plan_cannot_use_push_defaults(self):
  with tempfile.TemporaryDirectory() as t:
   p,local,remote,g,m,expected,plan=self.setup_case(t);plan['actions']=[];(p/'fork-plan.json').write_text(json.dumps(plan))
   with self.assertRaises(SystemExit):self.invoke(m,p,local,['--execute'])
   self.assertEqual(m.refs(),expected)
 def test_noop_action_plan_rejected(self):
  with tempfile.TemporaryDirectory() as t:
   p,local,remote,g,m,expected,plan=self.setup_case(t);plan['actions']=[{'ref':'refs/heads/main','expected_old':expected['refs/heads/main'],'new':expected['refs/heads/main']}];(p/'fork-plan.json').write_text(json.dumps(plan))
   with self.assertRaises(SystemExit):self.invoke(m,p,local,['--execute'])
   self.assertEqual(m.refs(),expected)
 def test_restoration_bundle_must_be_checksummed(self):
  with tempfile.TemporaryDirectory() as t:
   p,local,remote,g,m,expected,plan=self.setup_case(t);plan['backups']=[];(p/'fork-plan.json').write_text(json.dumps(plan))
   with self.assertRaises(SystemExit):self.invoke(m,p,local,[])
   self.assertEqual(m.refs(),expected)

if __name__=='__main__':unittest.main()
