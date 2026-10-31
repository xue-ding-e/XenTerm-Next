"""Pure discovery/parser and fake-process tests; never executes installed VS tools."""
import io,json,subprocess,unittest
from unittest.mock import Mock,patch
from pathlib import PureWindowsPath, Path
import vs_discovery as v

class Discovery(unittest.TestCase):
 def row(self):return dict(installationPath=str(v.ROOTS['vs2026-enterprise'][0]),installationVersion='18.11.12315.225',isComplete=True,isLaunchable=True,isPrerelease=False)
 def parse(self,row):return v.parse_discovery(json.dumps([row]).encode())
 def test_discovery_env_matches_qualified_machine_directories_without_ambient(self):
  with patch.dict(v.os.environ,{'SECRET':'do-not-copy','PROGRAMDATA':'private'},clear=True):
   env=v.discovery_environment(Path('/owned'))
  self.assertNotIn('SECRET',env)
  expected={'PROGRAMDATA':r'C:\ProgramData','ALLUSERSPROFILE':r'C:\ProgramData',
   'PROGRAMFILES':r'C:\Program Files','PROGRAMW6432':r'C:\Program Files','SYSTEMDRIVE':'C:',
   'ProgramFiles(x86)':r'C:\Program Files (x86)','CommonProgramFiles':r'C:\Program Files\Common Files',
   'CommonProgramFiles(x86)':r'C:\Program Files (x86)\Common Files'}
  self.assertEqual({key:env[key] for key in expected},expected)
  self.assertEqual(set(env),set(expected)|{'SYSTEMROOT','WINDIR','PATH','HOME','USERPROFILE','TEMP','TMP'})
  self.assertEqual(env['HOME'],'/owned/home');self.assertEqual(env['TEMP'],'/owned/tmp')
  self.assertEqual(env['PATH'],r'C:\Windows\System32')
 def test_actual_image_2026_root_is_selected(self):
  got=self.parse(self.row());self.assertEqual(got['root_id'],'vs2026-enterprise')
  self.assertEqual(got['installation_root'],str(v.ROOTS['vs2026-enterprise'][0]))
 def test_supported_2022_requires_actual_discovery_not_default(self):
  row=self.row();row.update(installationPath=str(v.ROOTS['vs2022-enterprise'][0]),installationVersion='17.14.1.2')
  self.assertEqual(self.parse(row)['root_id'],'vs2022-enterprise')
 def test_unapproved_root_versions_and_states_reject(self):
  for changes in ({'installationPath':'C:/Users/private/VS'}, {'installationPath':'C:/Program Files/Microsoft Visual Studio/18/Enterprise/../Enterprise'},
                  {'installationVersion':'17.1.2.3'}, {'installationVersion':'18.11.12315.225;cmd'},
                  {'isPrerelease':True},{'isComplete':False},{'isLaunchable':1}):
   row=self.row();row.update(changes)
   with self.subTest(changes=changes),self.assertRaises(ValueError):self.parse(row)
 def test_empty_duplicate_and_oversize_results_reject(self):
  for raw in (b'[]',json.dumps([self.row(),self.row()]).encode(),b'x'*(v.CAP+1),
              b'[{"installationPath":"one","installationPath":"two"}]'):
   with self.assertRaises(ValueError):v.parse_discovery(raw)
 def test_selector_bom_supported_but_not_arbitrary_text(self):
  self.assertEqual(v.parse_selector(b'\xef\xbb\xbf14.51.36247\r\n'),'14.51.36247')
  for raw in (b'',b'../14.51.36247',b'14.51.36247;cmd',b'x'*129):
   with self.assertRaises(ValueError):v.parse_selector(raw)
 def fake(self,data,code=0):
  process=Mock();process.stdout=io.BytesIO(data);process.wait.return_value=code;process.poll.return_value=code
  return process
 def test_fixed_query_cleared_environment(self):
  process=self.fake(json.dumps([self.row()]).encode());env={'SYSTEMROOT':'synthetic'}
  with patch.object(v.subprocess,'Popen',return_value=process) as spawn:
   result=v.run_installed_query('fixed-vswhere',env)
  self.assertEqual(v.parse_discovery(result)['root_id'],'vs2026-enterprise')
  self.assertEqual(spawn.call_args.args[0],['fixed-vswhere',*v.ARGS]);self.assertEqual(spawn.call_args.kwargs['env'],env)
  self.assertNotIn('shell',spawn.call_args.kwargs)
 def test_output_overflow_kills_and_rejects(self):
  process=self.fake(b'x'*(v.CAP+1))
  with patch.object(v.subprocess,'Popen',return_value=process),self.assertRaisesRegex(ValueError,'OUTPUT_BOUND'):
   v.run_installed_query('fixed-vswhere',{})
  process.kill.assert_called_once()
 def test_timeout_kills_and_rejects(self):
  process=self.fake(b'[]');process.wait.side_effect=[subprocess.TimeoutExpired('fixed',15),0]
  with patch.object(v.subprocess,'Popen',return_value=process),self.assertRaisesRegex(ValueError,'TIMEOUT'):
   v.run_installed_query('fixed-vswhere',{})
  process.kill.assert_called_once()
 def test_nonzero_does_not_export_output(self):
  process=self.fake(b'private untrusted text',3)
  with patch.object(v.subprocess,'Popen',return_value=process),self.assertRaisesRegex(ValueError,'^VS_DISCOVERY_EXIT$'):
   v.run_installed_query('fixed-vswhere',{})

if __name__=='__main__':unittest.main()
