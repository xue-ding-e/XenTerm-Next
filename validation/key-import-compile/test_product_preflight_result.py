"""Tool-preflight failure still produces bounded outcome; no native calls."""
import importlib.util,json,tempfile,unittest
from pathlib import Path
from unittest.mock import Mock,patch
p=Path(__file__).with_name('run-product-compile.py');spec=importlib.util.spec_from_file_location('product_entry',p)
e=importlib.util.module_from_spec(spec);spec.loader.exec_module(e)
class Preflight(unittest.TestCase):
 def test_vs_selector_failure_has_structured_result_without_controller(self):
  with tempfile.TemporaryDirectory() as temp:
   root=Path(temp)
   with patch.object(e,'plain',side_effect=lambda x:x),patch.object(e,'select_installed',side_effect=ValueError('VS_DISCOVERY_SELECTOR_BOUND')),patch.object(e.controller,'execute') as execute:
    with self.assertRaises(ValueError):e.execute_scoped(root,root,'a'*40,'123')
   execute.assert_not_called()
   result=json.loads((root/'product-compile-result.json').read_text())
   self.assertEqual(result['phase'],'preflight-vs');self.assertFalse(result['controller_started'])
   self.assertEqual(result['preflight_code'],'VS_DISCOVERY_SELECTOR_BOUND')
   self.assertTrue((root/'cargo-diagnostics.json').is_file())
 def test_git_preflight_failure_still_records_no_controller(self):
  with tempfile.TemporaryDirectory() as temp:
   root=Path(temp)
   with patch.object(e,'plain',side_effect=ValueError('missing ancestor')),patch.object(e,'select_installed') as vs:
    with self.assertRaises(ValueError):e.execute_scoped(root,root,'a'*40,'123')
   vs.assert_not_called();result=json.loads((root/'product-compile-result.json').read_text())
   self.assertEqual(result['phase'],'preflight-git');self.assertFalse(result['controller_started'])
 def test_no_raw_discovery_error_export(self):
  with tempfile.TemporaryDirectory() as temp:
   root=Path(temp)
   with patch.object(e,'plain',side_effect=lambda x:x),patch.object(e,'select_installed',side_effect=ValueError('secret raw text')):
    with self.assertRaises(ValueError):e.execute_scoped(root,root,'a'*40,'123')
   self.assertNotIn('secret',(root/'product-compile-result.json').read_text())
if __name__=='__main__':unittest.main()
