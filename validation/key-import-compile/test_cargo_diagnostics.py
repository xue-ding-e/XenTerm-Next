"""Untrusted JSON negative tests. No Cargo execution or network."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import cargo_diagnostics as d

class Diagnostics(unittest.TestCase):
 def setUp(self):
  self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
  self.root=Path(self.temp.name);self.source=self.root/'source';self.source.mkdir();(self.source/'src').mkdir()
  (self.source/'src/main.rs').write_text('synthetic source never exported')
  (self.source/'Cargo.lock').write_text('[[package]]\nname="xenterm"\nversion="0.1.0"\n')
  self.path=self.root/'stdout'
 def row(self):
  return dict(reason='compiler-message',package_id='path+'+self.source.as_uri()+'#xenterm@0.1.0',
     message=dict(level='error',code={'code':'E0308'},message='SECRET fake sensitive error text',rendered='SECRET snippet',
       spans=[dict(file_name='src/main.rs',line_start=1,line_end=1,column_start=2,column_end=3,text=[{'text':'SECRET'}])]))
 def run_rows(self,rows):
  self.path.write_text('\n'.join(json.dumps(x) for x in rows))
  return d.summarize(self.source,self.path)
 def test_whitelist_has_type_category_but_no_raw_text(self):
  result=self.run_rows([self.row()]);text=json.dumps(result)
  self.assertTrue(result['structured_diagnostics_available']);self.assertNotIn('SECRET',text)
  self.assertNotIn(str(self.root),text);self.assertIn('types differ',text)
  self.assertEqual(result['diagnostics'][0]['locations'][0]['file'],'src/main.rs')
 def test_external_or_traversal_paths_are_not_probed(self):
  for name in ['/synthetic/outside/secret','../outside','src/../../outside']:
   row=self.row();row['message']['spans'][0]['file_name']=name
   with patch.object(d,'plain',wraps=d.plain) as check:
    result=self.run_rows([row])
    self.assertEqual(result['diagnostics'][0]['locations'],[])
    self.assertFalse(any(str(x.args[0]).startswith('/synthetic/outside') for x in check.call_args_list))
 def test_reparse_location_is_omitted(self):
  (self.source/'src/link.rs').symlink_to(self.source/'src/main.rs')
  row=self.row();row['message']['spans'][0]['file_name']='src/link.rs'
  self.assertEqual(self.run_rows([row])['diagnostics'][0]['locations'],[])
 def test_unknown_package_and_build_script_omitted(self):
  row=self.row();row['package_id']='unreviewed#SECRET'
  result=self.run_rows([row,{'reason':'build-script-executed','linked_paths':['SECRET']}])
  self.assertFalse(result['structured_diagnostics_available']);self.assertNotIn('SECRET',json.dumps(result))
 def test_unknown_code_does_not_expose_message(self):
  row=self.row();row['message']['code']={'code':'SECRET_ENV=value'}
  result=self.run_rows([row]);self.assertIsNone(result['diagnostics'][0]['code'])
  self.assertNotIn('SECRET',json.dumps(result));self.assertIn('free-form details omitted',json.dumps(result))
 def test_row_cap_marks_truncated(self):
  result=self.run_rows([self.row() for _ in range(70)])
  self.assertTrue(result['truncated']);self.assertEqual(len(result['diagnostics']),64)
  self.assertLessEqual(len(json.dumps(result).encode()),d.OUTPUT_CAP)
 def test_input_cap_fails_not_unbounded_read(self):
  self.path.write_bytes(b'12345')
  with patch.object(d,'INPUT_CAP',4),self.assertRaisesRegex(ValueError,'input bound'):d.summarize(self.source,self.path)
 def test_duplicate_and_large_lines_are_omitted(self):
  self.path.write_bytes(b'{"reason":"compiler-message","reason":"compiler-message"}\n'+b'x'*(d.LINE_CAP+1))
  result=d.summarize(self.source,self.path)
  self.assertFalse(result['structured_diagnostics_available']);self.assertTrue(result['truncated'])
  self.assertEqual(result['omitted_lines'],2)
 def test_invalid_position_types_are_omitted(self):
  for value in (True,0,-1,10000001,'secret'):
   row=self.row();row['message']['spans'][0]['line_start']=value
   self.assertEqual(self.run_rows([row])['diagnostics'][0]['locations'],[])
 def test_absence_is_not_environment_classification(self):
  self.path.write_text('build script failed: SECRET')
  result=d.summarize(self.source,self.path)
  self.assertFalse(result['structured_diagnostics_available'])
  self.assertIn('not an environment diagnosis',result['limitation'])
  self.assertNotIn('SECRET',json.dumps(result))

if __name__=='__main__':unittest.main()
