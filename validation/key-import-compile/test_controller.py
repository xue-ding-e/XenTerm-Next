"""Fail-closed orchestration and budget contract; no subprocess product run."""
import unittest
from unittest.mock import patch
import controller
class Controller(unittest.TestCase):
 def test_no_local_execution_without_exact_hosted_run(self):
  with patch.dict(controller.os.environ,{},clear=True):
   with self.assertRaisesRegex(ValueError,'hosted Windows x64 required'):
    controller.execute(None,None,None,None,None,None,None,None)
 def test_compile_budget_leaves_cleanup_margin(self):
  self.assertEqual(controller.CHILD_SECONDS,2700)
  self.assertEqual(controller.OUTER_HOST_SECONDS,2715)
  self.assertGreater(controller.COMPILE_STEP_MINUTES*60,controller.OUTER_HOST_SECONDS)
 def test_fetch_uses_fixed_evidence_not_ready_boolean(self):
  from pathlib import Path
  source=Path(controller.__file__).read_text()
  self.assertIn('fetched=fetcher(plan)',source)
  self.assertIn('verify_transport_evidence(control/',source)
  self.assertIn('verify_host_entry_evidence(control/',source)
  self.assertNotIn('fetched=execute_fetch(plan)',source)
if __name__=='__main__':unittest.main()
