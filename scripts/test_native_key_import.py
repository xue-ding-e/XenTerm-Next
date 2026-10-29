"""Pure controls only: no Rust build, network, real keys or native GUI claims."""
import importlib.util,json,os,sys,tempfile,unittest,subprocess
from pathlib import Path
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('native',Path(__file__).with_name('native-key-import.py'));d=importlib.util.module_from_spec(spec);spec.loader.exec_module(d)
class Gates(unittest.TestCase):
    def test_patches(self):d.patch_hashes()
    def test_platform_counts(self):
        self.assertEqual(len(d.names('Windows')),27);self.assertEqual(len(d.names('macOS')),29)
        self.assertEqual(set(d.names('macOS'))-set(d.names('Windows')),set(d.UNIX_ONLY))
        self.assertEqual(len(set(d.I['NAMES'])),29)
        with self.assertRaises(RuntimeError):d.names('Linux')
    def test_inventory_fail_closed(self):
        for n in d.I['NAMES']:
            d.parse_inventory(n+': test\n',n)
            for bad in ['',n+': benchmark\n',n+': test\n'+n+': test\n','other: test\n']:
                with self.assertRaises(RuntimeError):d.parse_inventory(bad,n)
    def test_result_fail_closed(self):
        n=d.I['NAMES'][0];good=f'running 1 test\ntest {n} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 20 filtered out; finished in 0.01s\n'
        d.parse_result(good,n)
        for bad in ['',good+good,good.replace('1 passed','0 passed'),good.replace('0 ignored','1 ignored'),good.replace(' ... ok',' ... ignored'),good.replace(n,'other')]:
            with self.assertRaises(RuntimeError):d.parse_result(bad,n)
    def test_binary_original_crate(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp).resolve();source=root/'source';out=root/'out';binary=out/'target'/'native'/'debug'/'xenterm';binary.parent.mkdir(parents=True);binary.write_bytes(b'synthetic')
            v={'reason':'compiler-artifact','profile':{'test':True},'target':{'name':'xenterm','kind':['bin'],'src_path':str(source/'src/main.rs')},'features':d.FEATURES,'executable':str(binary)}
            with patch.object(d,'SOURCE',source),patch.object(d,'OUT',out):
                self.assertEqual(d.artifact_from_log(json.dumps(v),'native')['sha256'],d.digest(binary))
                for bad in [json.dumps(v)+'\n'+json.dumps(v),json.dumps(dict(v,features=['headless'])),json.dumps(dict(v,executable=str(root/'wrong')))]:
                    with self.assertRaises(RuntimeError):d.artifact_from_log(bad,'native')
    def test_runtime_env_excludes_build_and_secrets(self):
        with tempfile.TemporaryDirectory() as tmp,patch.object(d,'OUT',Path(tmp)),patch.dict(os.environ,{'GITHUB_TOKEN':'synthetic-secret','ACTIONS_RUNTIME_TOKEN':'synthetic-secret','CARGO_HOME':'real','HOME':'real','HTTPS_PROXY':'synthetic-proxy','RUSTFLAGS':'bad'}):
            env=d.sandbox_env()
            for k in ('GITHUB_TOKEN','ACTIONS_RUNTIME_TOKEN','CARGO_HOME','HTTPS_PROXY','RUSTFLAGS'):self.assertNotIn(k,env)
            for k in ('HOME','USERPROFILE','TMP','APPDATA'):self.assertTrue(Path(env[k]).is_relative_to(Path(tmp)))
    def test_compile_env_preserves_sdk_not_credentials(self):
        inherited={'GITHUB_TOKEN':'synthetic','GH_TOKEN':'synthetic','ACTIONS_RUNTIME_TOKEN':'synthetic','ACTIONS_ID_TOKEN_REQUEST_TOKEN':'synthetic','CARGO_REGISTRIES_CRATES_IO_TOKEN':'synthetic','AWS_SECRET_ACCESS_KEY':'synthetic','HTTPS_PROXY':'synthetic','RUSTFLAGS':'synthetic','ARBITRARY_AUTH':'synthetic','PATH':'sdk-path','INCLUDE':'sdk-include','LIB':'sdk-lib','WindowsSdkDir':'sdk-dir','SDKROOT':'sdk-root','DEVELOPER_DIR':'developer-dir','CARGO_HOME':'cargo-cache','RUSTUP_HOME':'rustup-cache'}
        with tempfile.TemporaryDirectory() as tmp,patch.object(d,'OUT',Path(tmp)),patch.dict(os.environ,inherited,clear=True):
            env=d.compile_env()
            for k in ('GITHUB_TOKEN','GH_TOKEN','ACTIONS_RUNTIME_TOKEN','ACTIONS_ID_TOKEN_REQUEST_TOKEN','CARGO_REGISTRIES_CRATES_IO_TOKEN','AWS_SECRET_ACCESS_KEY','HTTPS_PROXY','RUSTFLAGS','ARBITRARY_AUTH'):self.assertNotIn(k,env)
            for k in ('PATH','INCLUDE','LIB','WindowsSdkDir','SDKROOT','DEVELOPER_DIR','CARGO_HOME','RUSTUP_HOME'):self.assertEqual(env[k],inherited[k])
            self.assertTrue(Path(env['HOME']).is_relative_to(Path(tmp)))
    def test_platform_target_mismatch(self):
        for platform,system,target in [('Windows','win32','x86_64-pc-windows-msvc'),('macOS','darwin','aarch64-apple-darwin')]:
            with patch.dict(os.environ,{'RUNNER_OS':platform}),patch.object(d.sys,'platform',system):self.assertEqual(d.platform_target(),target)
        with patch.dict(os.environ,{'RUNNER_OS':'Windows'}),patch.object(d.sys,'platform','darwin'),self.assertRaises(RuntimeError):d.platform_target()
        with patch.object(d,'platform_target',return_value='native'),self.assertRaises(RuntimeError):d.compile_product('wrong')
    def test_spawn_and_timeout_fail_closed(self):
        for exc,expected in [(FileNotFoundError(),'unknown'),(subprocess.TimeoutExpired(['synthetic'],1),'timeout-hosted-lifecycle')]:
            with tempfile.TemporaryDirectory() as tmp,patch.object(d,'OUT',Path(tmp)),patch.object(d,'SOURCE',Path(tmp)),patch.object(d,'RECORDS',[]),patch.object(d.subprocess,'run',side_effect=exc):
                with self.assertRaises(type(exc)):d.run(['synthetic'],'probe',1,{})
                self.assertEqual(d.RECORDS[-1]['state'],expected)
                self.assertNotEqual(d.RECORDS[-1].get('exit_code'),0)
    def test_lexical_symlink_rejected_before_resolve(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp).resolve();source=root/'source';out=root/'out';binary=out/'target'/'native'/'debug'/'xenterm';binary.parent.mkdir(parents=True);binary.write_bytes(b'synthetic')
            v={'reason':'compiler-artifact','profile':{'test':True},'target':{'name':'xenterm','kind':['bin'],'src_path':str(source/'src/main.rs')},'features':d.FEATURES,'executable':str(binary)}
            original=Path.is_symlink
            # Model a symlink without requiring Windows symlink privileges.
            with patch.object(d,'SOURCE',source),patch.object(d,'OUT',out),patch.object(Path,'is_symlink',lambda p:p==binary or original(p)),self.assertRaisesRegex(RuntimeError,'symlink executable path'):
                d.artifact_from_log(json.dumps(v),'native')
    def test_deadline(self):
        with patch.object(d.time,'monotonic',return_value=100):
            self.assertEqual(d.remaining(60,120),20)
            with self.assertRaises(RuntimeError):d.remaining(60,99)
    def test_wait_actual_exit_and_no_raw_output(self):
        with tempfile.TemporaryDirectory() as tmp,patch.object(d,'OUT',Path(tmp)),patch.object(d,'SOURCE',Path(tmp)),patch.object(d,'RECORDS',[]):
            result=d.run([sys.executable,'-c','print("synthetic")'],'ok',5,dict(os.environ));self.assertEqual(result.strip(),'synthetic')
            with self.assertRaises(RuntimeError):d.run([sys.executable,'-c','import sys; sys.exit(7)'],'bad',5,dict(os.environ))
            self.assertEqual(d.RECORDS[-1]['exit_code'],7);self.assertNotIn('synthetic',json.dumps(d.RECORDS))
    def test_nonzero_not_parsed_as_pass(self):
        with tempfile.TemporaryDirectory() as tmp,patch.object(d,'OUT',Path(tmp)),patch.object(d,'SOURCE',Path(tmp)),patch.object(d,'RECORDS',[]):
            with patch.object(d.subprocess,'run',return_value=subprocess.CompletedProcess([],101)),self.assertRaises(RuntimeError):d.run(['synthetic'],'test',1,{})
            self.assertEqual(d.RECORDS[-1]['state'],'nonzero-needs-review')
    def test_workflow(self):
        text=(d.CONTROL/'.github/workflows/ci.yml').read_text()
        for limit in ('60','45','10'):self.assertEqual(text.count('timeout-minutes: '+limit+'\n'),2)
        self.assertIn('needs: windows',text);self.assertNotIn('continue-on-error',text)
        self.assertNotIn('push:',text);self.assertNotIn('*.log',text);self.assertNotIn('*.json',text)
        self.assertEqual(text.count('expected_sha == github.sha'),2)
        code=(d.HERE/'native-key-import.py').read_text()
        for bad in ('execv(', 'killpg(', 'taskkill', 'Popen('):self.assertNotIn(bad,code)
        self.assertIn('subprocess.run(',code);self.assertIn('p[\'started_monotonic\']+57*60',code)
if __name__=='__main__':unittest.main()
