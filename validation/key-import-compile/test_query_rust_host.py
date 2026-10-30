import unittest
from pathlib import Path
class QueryContract(unittest.TestCase):
 def test_narrow_source_policy(self):
  root=Path(__file__).parent;s=(root/'Host/Program.cs').read_text();q=s[s.index('static int QueryRust'):s.index('static int Main')]
  self.assertIn('"natural-tree","smoke-v1"',q);self.assertIn('root,20,stdout,stderr',q)
  self.assertIn('Require(v.Succeeded && XenOwnedProcess.SafeToStart',q)
  self.assertNotIn('CompileArgs',q);self.assertNotIn('INCLUDE',q);self.assertNotIn('LIB',q)
  self.assertIn('new[]{"-vV"}',q);self.assertNotIn('"+1.99.1"',q);self.assertIn('string exe=tools[i];PlainPath(exe,true);',q);self.assertIn('"which","--toolchain","1.99.1","rustc"',q)
  adapter=(root/'query_rust_host.py').read_text();self.assertIn('timeout=85',adapter);self.assertEqual(adapter.split('def main():')[0].count('subprocess.run('),1)
  self.assertIn('finally:',adapter);self.assertIn('host_build_files',adapter)

class PersistentOwner(unittest.TestCase):
 def test_timeout_root_cannot_restart(self):
  import tempfile,subprocess
  from unittest.mock import patch
  import query_rust_host as q
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);host=root/'host';host.mkdir()
   def fake_verify(root,owned):return owned([], 'rustc-version',20)
   with patch.object(q,'host_build_files',return_value={}),patch.object(q,'verify',side_effect=fake_verify),patch.object(q.subprocess,'run',side_effect=subprocess.TimeoutExpired('host',85)) as run,patch.dict('os.environ',{'SYSTEMROOT':'synthetic'}):
    with self.assertRaises(subprocess.TimeoutExpired):q.query(root,host,'sha','run',{},lambda:None)
    with self.assertRaises(FileExistsError):q.query(root,host,'sha','run',{},lambda:None)
    self.assertEqual(run.call_count,1)
    self.assertIn('unknown',(root/'rust-query-state.json').read_text())

class InstallDiagnostics(unittest.TestCase):
 def test_fixed_scope_no_paths(self):
  import tempfile
  import query_rust_host as q
  with tempfile.TemporaryDirectory() as d:
   rows=q.fixed_install_diagnostic(Path(d))
   self.assertEqual(len(rows),8)
   self.assertTrue(all(r['state']=='missing' for r in rows))
   self.assertTrue(all(set(r)=={'relative','state','reparse','ancestors_readable'} for r in rows))

 def test_missing_leaf_under_link_is_still_reported(self):
  import tempfile
  import query_rust_host as q
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);outside=root/'actual';outside.mkdir();(root/'cargo-home').symlink_to(outside,target_is_directory=True)
   row=next(r for r in q.fixed_install_diagnostic(root) if r['relative']=='cargo-home/bin/rustc.exe')
   self.assertEqual(row['state'],'missing');self.assertTrue(row['reparse'])

 def test_unreadable_leaf_and_ancestor_are_distinct(self):
  import tempfile
  from unittest.mock import patch
  import query_rust_host as q
  original=Path.lstat
  with tempfile.TemporaryDirectory() as d:
   root=Path(d)
   def denied(path,*args,**kwargs):
    if path==root/'cargo-home':raise PermissionError('synthetic private detail')
    return original(path,*args,**kwargs)
   with patch.object(Path,'lstat',denied):rows=q.fixed_install_diagnostic(root)
   home=next(r for r in rows if r['relative']=='cargo-home')
   child=next(r for r in rows if r['relative']=='cargo-home/bin/rustc.exe')
   self.assertEqual(home['state'],'unreadable');self.assertFalse(home['ancestors_readable'])
   self.assertEqual(child['state'],'missing');self.assertFalse(child['ancestors_readable'])
   self.assertNotIn('synthetic private detail',str(rows))

if __name__=='__main__':unittest.main()
