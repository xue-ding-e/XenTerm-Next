import io,json,subprocess,unittest
from pathlib import Path
from unittest.mock import Mock,patch
import diagnose_vs_discovery as d
class Diagnostic(unittest.TestCase):
 def test_profiles_are_explicit_no_ambient(self):
  with patch.dict(d.os.environ,{'SECRET':'never-copy'},clear=True):profiles=d.environments(Path('/owned'))
  self.assertNotIn('SECRET',str(profiles));self.assertNotIn('PROGRAMDATA',profiles['minimal'])
  self.assertEqual(profiles['machine-directories']['PROGRAMDATA'],r'C:\ProgramData')
 def test_zero_and_multiple_counts(self):
  for rows in ([],[{},{}]):self.assertEqual(d.summarize(0,json.dumps(rows).encode())['row_count'],len(rows))
 def test_private_unknown_strings_redacted(self):
  raw=json.dumps([dict(installationPath='C:/Users/secret/VS',installationVersion='secret',productId='secret',isComplete='secret',displayName='secret',properties={'secret':'token'})]).encode()
  self.assertNotIn('secret',json.dumps(d.summarize(0,raw)))
 def test_known_public_metadata(self):
  row=dict(installationPath=str(d.v.ROOTS['vs2026-enterprise'][0]),installationVersion='18.11.12315.225',productId='Microsoft.VisualStudio.Product.Enterprise',isComplete=True)
  got=d.summarize(0,json.dumps([row]).encode())['rows'][0]
  self.assertEqual(got['root_id'],'vs2026-enterprise');self.assertEqual(got['version'],'18.11.12315.225')
 def test_shapes_duplicates_and_count_bound(self):
  for raw in (b'not json',b'[{"a":1,"a":2}]'):self.assertEqual(d.summarize(0,raw)['parse'],'invalid-json')
  self.assertEqual(d.summarize(0,b'{}')['parse'],'not-array')
  got=d.summarize(0,json.dumps([{}]*9).encode());self.assertTrue(got['rows_truncated']);self.assertEqual(len(got['rows']),8)
 def fake(self,data,code=0):
  proc=Mock();proc.stdout=io.BytesIO(data);proc.wait.return_value=code;proc.poll.return_value=code;return proc
 def test_nonzero_retained_without_raw(self):
  proc=self.fake(b'private-error',3)
  with patch.object(d.subprocess,'Popen',return_value=proc) as spawn:code,raw=d.query('fixed',{'ONLY':'fixed'})
  self.assertEqual(d.summarize(code,raw),{'exit_code':3,'json_bytes':13,'parse':'invalid-json'})
  self.assertEqual(spawn.call_args.args[0],['fixed',*d.v.ARGS]);self.assertNotIn('shell',spawn.call_args.kwargs)
 def test_cap_and_timeout_fail_closed(self):
  proc=self.fake(b'x'*(d.CAP+1))
  with patch.object(d.subprocess,'Popen',return_value=proc),self.assertRaises(ValueError):d.query('fixed',{})
  proc.kill.assert_called_once()
  proc=self.fake(b'[]');proc.wait.side_effect=[subprocess.TimeoutExpired('fixed',15),0]
  with patch.object(d.subprocess,'Popen',return_value=proc),self.assertRaisesRegex(ValueError,'TIMEOUT'):d.query('fixed',{})
if __name__=='__main__':unittest.main()
