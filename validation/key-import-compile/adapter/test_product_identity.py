import json,tempfile,unittest
from pathlib import Path,PureWindowsPath,PurePosixPath
from unittest.mock import Mock
import product_identity as p
class ProductIdentity(unittest.TestCase):
    def test_fixed_inputs(self):
        self.assertEqual(p.digest(p.INPUTS),p.INPUTS_SHA)
        x=p.read_json(p.INPUTS,16384)
        self.assertEqual((x['SHA'],x['PRODUCT_TREE'],x['TEST_TREE']),(p.MAIN,p.PRODUCT_TREE,p.TEST_TREE))
        self.assertEqual(x['PATCHES']['key-import.patch'],'1740f70f99d2c715a2f652d526d81896dd5f17db59e513cc50b2916f0e435305')
    def test_single_exact_artifact_and_negative_cases(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);source=root/'source';(source/'src').mkdir(parents=True);(source/'src/main.rs').write_text('// synthetic')
            binary=root/'target'/p.TARGET/'debug/xenterm.exe';binary.parent.mkdir(parents=True);binary.write_bytes(b'synthetic not executable')
            v=dict(reason='compiler-artifact',profile={'test':True},executable=str(binary),target={'name':'xenterm','kind':['bin'],'src_path':str(source/'src/main.rs')},features=p.FEATURES)
            result=p.native_artifact(json.dumps(v),source,root);self.assertEqual(result['sha256'],p.digest(binary))
            for bad in [json.dumps(v)+'\n'+json.dumps(v),json.dumps(dict(v,features=['headless'])),json.dumps(dict(v,profile={'test':'false'})),json.dumps(dict(v,profile={'test':1})),json.dumps(v).replace('"test": true','"test": false, "test": true'),json.dumps(dict(v,executable=str(root/'outside.exe'))),json.dumps(dict(v,target=dict(v['target'],src_path=str(root/'wrong.rs'))))]:
                with self.assertRaises(ValueError):p.native_artifact(bad,source,root)
            with self.assertRaises(ValueError):p.native_artifact('x'*(p.CAP+1),source,root)
    def test_windows_git_root_normalization(self):
        git=Mock(return_value='C:/work/source')
        self.assertTrue(p.git_path_matches(git(),PureWindowsPath('C:\\work\\source')))
        self.assertFalse(p.git_path_matches('C:/work/source/../outside',PureWindowsPath('C:/work/source')))
        self.assertFalse(p.git_path_matches('D:/work/source',PureWindowsPath('C:/work/source')))
        self.assertTrue(p.git_path_matches('/tmp/source',PurePosixPath('/tmp/source')))
    def test_host_build_current_run_and_four_files(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);host=root/'host';host.mkdir();hashes={}
            for name in p.HOST_FILES:(host/name).write_bytes(b'synthetic');hashes[name]=p.digest(host/name)
            expected={'Host/Program.cs':'fixed source hash'}
            r=dict(state='compiled',compile_exit=0,control_sha='a'*40,run_id=123,initial_identity=True,final_identity=True,compiled_copy_identity=True,host_sources=expected,files=hashes)
            path=root/'build.json';path.write_text(json.dumps(r));self.assertEqual(p.host_build_files(path,host,'a'*40,123,expected),hashes)
            with self.assertRaises(ValueError):p.host_build_files(path,host,'a'*40,124,expected)
            invalid=dict(r,compile_exit=False);path.write_text(json.dumps(invalid))
            with self.assertRaises(ValueError):p.host_build_files(path,host,'a'*40,123,expected)
            path.write_text(json.dumps(r))
            (host/'CompileOnlyHost.dll').write_bytes(b'changed')
            with self.assertRaises(ValueError):p.host_build_files(path,host,'a'*40,123,expected)
if __name__=='__main__':unittest.main()
