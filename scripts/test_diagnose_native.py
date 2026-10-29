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
    def test_children_bounded_and_canary_safe(self):
        message={'level':'error','message':'synthetic','children':[{'message':'SYNTHETIC_SECRET fatal error LNK1104 private-path','children':[{'message':'LNK2001 SYNTHETIC_SECRET','children':[{'message':'LNK9999'}]}]}]}
        record={'reason':'compiler-message','message':message};result=d.diagnose(json.dumps(record),'')
        self.assertEqual(result['linker_codes'],['LNK1104','LNK2001']);self.assertNotIn('SYNTHETIC_SECRET',json.dumps(result));self.assertNotIn('private-path',json.dumps(result))
        message['children']=[{'message':'nothing'}]*16+[{'message':'LNK9999'}]
        self.assertEqual(d.child_signals(message)[1],set())
        message['children']={'message':'LNK9999'};self.assertEqual(d.child_signals(message)[1],set())
    def test_path_family_no_arbitrary_output(self):
        for path,family in [('C:/Program Files/Git/usr/bin/link.exe','git-usr-bin'),('C:/VS/VC/Tools/MSVC/1/bin/link.exe','visual-studio-vc-tools'),('C:/Program Files/Windows Kits/10/bin/link.exe','windows-sdk'),('C:/SYNTHETIC_SECRET/link.exe','other')]:self.assertEqual(d.path_family(path),family)
    def test_link_smoke_fixed_source_not_executed(self):
        with tempfile.TemporaryDirectory() as tmp,patch.object(d.n,'OUT',Path(tmp)),patch.object(d.n,'compile_env',return_value={'PATH':'synthetic'}),patch.object(d.shutil,'which',return_value='rustc'),patch.object(d.n,'run',return_value='') as run:
            run.side_effect=lambda *args:(Path(tmp)/'link-smoke/probe.exe').write_bytes(b'synthetic-not-executed')
            self.assertTrue(d.link_smoke());self.assertEqual(run.call_count,1)
            args=run.call_args.args;self.assertEqual(args[0][0],'rustc');self.assertEqual(args[1],'link-smoke');self.assertEqual(args[2],20)
            self.assertEqual((Path(tmp)/'link-smoke/probe.rs').read_bytes(),b'fn main() {}\n')
            self.assertFalse(json.loads((Path(tmp)/'link-smoke.json').read_text())['executed_binary'])
    def test_smoke_exit_zero_without_binary_is_not_pass(self):
        with tempfile.TemporaryDirectory() as tmp,patch.object(d.n,'OUT',Path(tmp)),patch.object(d.n,'compile_env',return_value={'PATH':'synthetic'}),patch.object(d.shutil,'which',return_value='rustc'),patch.object(d.n,'run',return_value=''):
            self.assertFalse(d.link_smoke());result=json.loads((Path(tmp)/'link-smoke.json').read_text())
            self.assertEqual(result['state'],'failed-or-timeout');self.assertNotIn('binary_sha256',result)
    def test_smoke_failure_prevents_product_compile(self):
        with tempfile.TemporaryDirectory() as tmp,patch.object(d.n,'OUT',Path(tmp)),patch.object(d.n,'platform_target',return_value='x86_64-pc-windows-msvc'),patch.object(d,'tool_metadata',return_value={}),patch.object(d.n,'compile_env',return_value={}),patch.object(d,'link_smoke',return_value=False),patch.object(d.n,'compile_product') as compile_product,patch.object(d.n,'identities'),patch.object(d.n,'patch_hashes'),patch.object(d.n,'I',{'TEST_TREE':'tree','FIXTURES':{}}),patch.object(d.n,'git',side_effect=lambda *a:'tree' if a[0]=='write-tree' else ''),patch.object(d.n,'read',return_value={'control_files':{},'status':''}):
            self.assertEqual(d.main(),1);compile_product.assert_not_called()
            self.assertFalse(json.loads((Path(tmp)/'failure.json').read_text())['product_compile_attempted'])
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
