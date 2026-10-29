"""Small synthetic gates: no Rust compile, native TLS or process lifecycle claim."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec=importlib.util.spec_from_file_location('native',Path(__file__).with_name('native-webdav.py'))
d=importlib.util.module_from_spec(spec);spec.loader.exec_module(d)

class Gates(unittest.TestCase):
    def test_original_patch_bytes(self): d.patch_hashes()
    def test_inventory_rejects_missing_wrong_duplicate_benchmark(self):
        for name in d.NAMES:
            d.parse_inventory(name+': test\n',name)
            for bad in ['',name+': test\n'+name+': test\n',name+': benchmark\n','other: test\n']:
                with self.assertRaises(RuntimeError): d.parse_inventory(bad,name)
    def test_results_reject_zero_ignored_wrong_duplicate(self):
        for name in d.NAMES:
            good='running 1 test\ntest '+name+' ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 0.01s\n'
            d.parse_result(good,name)
            for bad in ['',good+good,good.replace('1 passed','0 passed'),good.replace(' ... ok',' ... ignored'),good.replace(name,'other')]:
                with self.assertRaises(RuntimeError): d.parse_result(bad,name)
    def test_compiler_product_and_features(self):
        with tempfile.TemporaryDirectory() as tmp,patch.object(d,'SOURCE',Path(tmp)):
            binary=Path(tmp)/'target'/'test-target'/'debug'/'deps'/'xenterm';binary.parent.mkdir(parents=True);binary.write_bytes(b'synthetic')
            item={'reason':'compiler-artifact','target':{'name':'xenterm','kind':['bin'],'src_path':str(Path(tmp)/'src/main.rs')},'features':d.FEATURES,'executable':str(binary),'profile':{'test':True}}
            receipt=d.artifact_from_log(json.dumps(item),'test-target');self.assertEqual(receipt['sha256'],d.digest(binary))
            with self.assertRaises(RuntimeError):d.artifact_from_log(json.dumps(item)+'\n'+json.dumps(item),'test-target')
            for field,value in [('features',['headless']),('executable',str(Path(tmp)/'elsewhere'))]:
                changed=dict(item);changed[field]=value
                with self.assertRaises(RuntimeError):d.artifact_from_log(json.dumps(changed),'test-target')
    def test_binary_hash_change_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp,patch.object(d,'OUT',Path(tmp)):
            binary=Path(tmp)/'binary';binary.write_bytes(b'synthetic');d.write('binary.json',{'path':str(binary),'sha256':d.digest(binary)})
            self.assertEqual(d.verify_binary(),binary)
            binary.write_bytes(b'changed')
            with self.assertRaises(RuntimeError):d.verify_binary()
    def test_final_identity_rejects_source_control_and_dirty(self):
        with patch.dict(os.environ,{'EXPECTED_CONTROL_SHA':'a'*40}):
            with patch.object(d,'git',side_effect=[d.BASE,'a'*40,'']):d.identities()
            for values in [['b'*40], [d.BASE,'b'*40], [d.BASE,'a'*40,'dirty']]:
                with patch.object(d,'git',side_effect=values),self.assertRaises(RuntimeError):d.identities()
    def test_no_custom_supervisor_and_hash_before_exec(self):
        source=(d.HERE/'native-webdav.py').read_text()
        for forbidden in ['killpg(', 'os.kill(', 'taskkill', 'Stop-Process', 'Popen(']:self.assertNotIn(forbidden,source)
        start=source.index("elif action=='exec':")
        self.assertLess(source.index('binary=verify_binary()',start),source.index('os.execv(',start))
        self.assertIn("require(git('write-tree')==TREE",source)
        self.assertIn('control file changed',source)
    def test_workflow_serial_native_boundaries(self):
        text=(d.CONTROL/'.github/workflows/ci.yml').read_text()
        self.assertEqual(text.count('timeout-minutes: 60'),2)
        self.assertEqual(text.count('timeout-minutes: 45'),2)
        self.assertIn('needs: windows',text)
        self.assertNotIn('continue-on-error',text)
        self.assertNotIn('push:',text);self.assertNotIn('pull_request:',text)
        self.assertEqual(text.count('expected_sha == github.sha'),2)
        for title in ['Execute real native default-rejection TLS test','Execute real native explicit-opt-in TLS test']:
            self.assertEqual(text.count(title+'\n        timeout-minutes: 1'),2)
        self.assertNotIn('owned-supervisor.py',text)
        self.assertEqual(text.count('GIT_CONFIG_KEY_0: core.autocrlf'),2)
        self.assertEqual(text.count("GIT_CONFIG_VALUE_0: 'false'"),2)
        self.assertEqual(text.count('GIT_CONFIG_KEY_1: core.eol'),2)
        self.assertEqual(text.count('GIT_CONFIG_VALUE_1: lf'),2)

if __name__=='__main__':unittest.main()
