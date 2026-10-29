"""Control guard checks only: never product-test acceptance."""
import importlib.util, unittest
from pathlib import Path
spec=importlib.util.spec_from_file_location('driver',Path(__file__).with_name('validate-key-import.py'));d=importlib.util.module_from_spec(spec);spec.loader.exec_module(d)
class Guards(unittest.TestCase):
    def test_patch_and_owner_identity(self):
        for name,h in d.PATCHES.items():self.assertEqual(d.b.digest(d.HERE/name),h)
        self.assertEqual(d.b.digest(d.HERE/'owned-supervisor.py'),d.OWNER_HASH)
        self.assertEqual(d.b.digest(d.HERE/'bounded-runtime.py'),d.RUNTIME_HASH)
    def test_unique_exact_cases(self):
        self.assertEqual(len(set(d.NAMES)),29)
        for name in d.NAMES:
            d.b.parse_inventory(name+': test\n',name)
            with self.assertRaises(AssertionError):d.b.parse_inventory('',name)
    def test_pass_and_failure_not_ignored(self):
        for name in d.NAMES:
            ok='running 1 test\ntest '+name+' ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out; finished in 0.1s\n'
            self.assertEqual(d.classify(ok,name,0),'passed')
            failed=ok.replace(' ... ok',' ... FAILED').replace('result: ok. 1 passed; 0 failed','result: FAILED. 0 passed; 1 failed')
            self.assertEqual(d.classify(failed,name,101),'failed-test-needs-review')
            for bad in [ok.replace(' ... ok',' ... ignored'),ok.replace(name,'wrong'),ok+ok]:
                with self.assertRaises(AssertionError):d.classify(bad,name,0)
    def test_failure_evidence_never_exports_arbitrary_text(self):
        raw="thread 'case' panicked at src/ui/impls/session_editor_button_tests.rs:162:9:\nassertion failed: window.has_active_dialog(cx)\nSECRET-PRIVATE-KEY-DATA"
        result=d.safe_failure_evidence(raw)
        self.assertEqual(result['locations'],[{'file':'src/ui/impls/session_editor_button_tests.rs','line':162,'column':9}])
        self.assertEqual(result['markers'],[{'marker':'dialog-closed','type':'assertion'}])
        self.assertNotIn('SECRET',str(result))
        self.assertEqual(d.safe_failure_evidence('panicked at /home/user/private:1:1: SECRET')['locations'],[])
        self.assertEqual(d.safe_failure_evidence('unknown failure SECRET')['markers'],[])
    def test_frozen_source_and_budgets(self):
        self.assertEqual(d.SHA,'8819e2ec132760047209e9dcff7341c5f8a15f94')
        self.assertEqual((len(d.CORE),len(d.UI),len(d.CONTROLS)),(19,7,3))
        self.assertEqual(d.FEATURES,['default','desktop','remote-mcp'])
        s=(d.HERE/'validate-key-import.py').read_text()
        for marker in ["'compile',2700",'time.monotonic()+600','remaining(60)',"'final-control-clean'","'final-tree'",'source-unchanged']:
            self.assertIn(marker,s)
if __name__=='__main__':unittest.main()
