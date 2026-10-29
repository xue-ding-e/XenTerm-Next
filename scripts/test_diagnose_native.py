"""Secret-canary and bounded diagnostic guard tests; no Rust/native test run."""
import importlib.util,json,unittest,sys,tempfile
from pathlib import Path
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('diag',Path(__file__).with_name('diagnose-native.py'));d=importlib.util.module_from_spec(spec);spec.loader.exec_module(d)
spec=importlib.util.spec_from_file_location('prior',Path(__file__).with_name('test_native_key_import.py'));prior=importlib.util.module_from_spec(spec);spec.loader.exec_module(prior)
class Diagnostic(unittest.TestCase):
    def test_canaries_and_primary_tracked_locations(self):
        secret='SYNTHETIC_SECRET_CANARY';message={'reason':'compiler-message','package_id':secret,'message':{'level':'error','code':{'code':'E0308'},'message':secret,'rendered':secret,'spans':[{'is_primary':True,'file_name':str(d.n.SOURCE/'src/main.rs'),'line_start':23,'column_start':4,'text':[{'text':secret}]},{'is_primary':False,'file_name':'src/main.rs','line_start':1,'column_start':1},{'is_primary':True,'file_name':secret+'/src/main.rs','line_start':1,'column_start':1}]}}
        result=d.diagnose(json.dumps(message),secret);self.assertNotIn(secret,json.dumps(result));self.assertEqual(result['compiler_messages'],[{'level':'error','code':'E0308','locations':[{'file':'src/main.rs','line':23,'column':4}]}])
        message['message']['code']['code']=secret;message['message']['spans'][0]['file_name']='untrusted.rs';result=d.diagnose(json.dumps(message),secret);self.assertEqual(result['compiler_messages'][0]['code'],None);self.assertEqual(result['compiler_messages'][0]['locations'],[])
        message['message']['level']=secret;self.assertEqual(d.diagnose(json.dumps(message),secret)['compiler_messages'],[])
    def test_malformed_payload_types_are_counted_not_exposed(self):
        for spans in (None,42,'SECRET_CANARY'):
            item={'reason':'compiler-message','message':{'level':'error','spans':spans}}
            r=d.diagnose(json.dumps(item),'');self.assertGreater(r['malformed_records'],0);self.assertNotIn('SECRET_CANARY',json.dumps(r))
        for filename in (None,42,{'SECRET_CANARY':1}):
            item={'reason':'compiler-message','message':{'level':'error','spans':[{'file_name':filename,'is_primary':True,'line_start':1,'column_start':1}]}}
            r=d.diagnose(json.dumps(item),'');self.assertGreater(r['malformed_records'],0);self.assertEqual(r['compiler_messages'][0]['locations'],[]);self.assertNotIn('SECRET_CANARY',json.dumps(r))
    def test_fixed_signatures_not_text(self):
        result=d.diagnose('', 'SYNTHETIC_SECRET failed to run custom build command /private-secret linker `link.exe` not found')
        self.assertEqual(result['fixed_categories'],['custom-build-failed','missing-linker']);self.assertNotIn('SECRET',json.dumps(result));self.assertNotIn('private-secret',json.dumps(result))
    def test_limits_unknown_and_invalid_locations(self):
        v={'reason':'compiler-message','message':{'level':'warning','code':None,'spans':[]}};r=d.diagnose('\n'.join([json.dumps(v)]*201),'');self.assertTrue(r['truncated']);self.assertEqual(len(r['compiler_messages']),200)
        self.assertEqual(d.diagnose('not-json\n[]\nnull','unknown')['compiler_messages'],[])
        for line in [-1,0,True,'12',10000000]:self.assertIsNone(d.safe_location({'file_name':'src/main.rs','line_start':line,'column_start':1}))
    def test_tool_metadata_no_raw_paths_or_banners(self):
        with tempfile.TemporaryDirectory() as tmp:
            sdk=Path(tmp)/'SYNTHETIC_PRIVATE_PATH';sdk.mkdir()
            with patch.object(d.shutil,'which',return_value=None):r=d.tool_metadata({'SDKROOT':str(sdk)})
            self.assertNotIn('SYNTHETIC_PRIVATE_PATH',json.dumps(r));self.assertTrue(r['sdk']['sdkroot']['directory_exists']);self.assertFalse(r['tools']['cargo']['available_on_path'])
    def test_diagnostic_workflow_only(self):
        text=(d.n.CONTROL/'.github/workflows/ci.yml').read_text()
        self.assertIn('timeout-minutes: 15',text);self.assertIn('timeout-minutes: 3',text);self.assertNotIn('  macos:',text);self.assertNotIn('native-key-import.py runtime',text);self.assertNotIn('*.log',text);self.assertNotIn('continue-on-error',text)
        source=(d.HERE/'diagnose-native.py').read_text();self.assertIn('compile_seconds=120',source);self.assertNotIn('n.runtime()',source)
if __name__=='__main__':
    # Preserve all prior environment/identity/parser guards; only their workflow
    # assertion belongs to the previous two-platform workflow, replaced above.
    def suite():
        prior_cases=[prior.Gates(name) for name in unittest.defaultTestLoader.getTestCaseNames(prior.Gates) if name!='test_workflow']
        return unittest.TestSuite(prior_cases+[unittest.defaultTestLoader.loadTestsFromTestCase(Diagnostic)])
    runner=unittest.TextTestRunner();normal=runner.run(suite())
    with patch.object(prior.os,'environ',prior.WindowsEnvironment(dict(prior.os.environ))):normalized=runner.run(suite())
    sys.exit(0 if normal.wasSuccessful() and normalized.wasSuccessful() else 1)
