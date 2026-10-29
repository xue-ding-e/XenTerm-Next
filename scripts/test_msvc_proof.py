"""Pure installed-tool bootstrap guards; no bootstrap/native compiler locally."""
import importlib.util,json,unittest,tempfile,sys,os,io,subprocess
from pathlib import Path
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('proof',Path(__file__).with_name('msvc-proof.py'));p=importlib.util.module_from_spec(spec);spec.loader.exec_module(p)
spec=importlib.util.spec_from_file_location('previous',Path(__file__).with_name('test_diagnose_native.py'));previous=importlib.util.module_from_spec(spec);spec.loader.exec_module(previous)
class BootstrapGuards(unittest.TestCase):
    def test_export_whitelist_never_all_environment(self):
        with patch.dict(os.environ,{'GITHUB_TOKEN':'SYNTHETIC_SECRET','ACTIONS_RUNTIME_TOKEN':'SYNTHETIC_SECRET','PATH':'sdk-path','INCLUDE':'sdk-include'},clear=True),patch.object(sys,'stdout',io.StringIO()) as out:p.capture_env()
        self.assertEqual(json.loads(out.getvalue()),{'PATH':'sdk-path','INCLUDE':'sdk-include'});self.assertNotIn('SECRET',out.getvalue())
    def test_sdk_parse_rejects_unknown_or_missing(self):
        valid={k:'synthetic' for k in ('PATH','INCLUDE','LIB','VCTOOLSINSTALLDIR','WINDOWSSDKDIR')};self.assertEqual(p.parse_sdk_output(json.dumps(valid)),valid)
        for bad in [dict(valid,GITHUB_TOKEN='SECRET'),{},[],dict(valid,LIB=None),dict(valid,LIB='bad\nvalue')]:
            with self.assertRaises(RuntimeError):p.parse_sdk_output(json.dumps(bad))
    def test_shell_metacharacters_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            good=Path(tmp).resolve();self.assertEqual(p.safe_cmd_path(good),str(good))
            for character in '%!^&|<>"\r\n':
                with self.assertRaises(RuntimeError):p.safe_cmd_path(good/('bad'+character))
    def test_selected_tools_and_sdk_containment(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp).resolve()/'VS';tool=root/'VC/Tools/MSVC/14.4';sdk=Path(tmp).resolve()/'SDK';include=sdk/'Include';lib=sdk/'Lib';include.mkdir(parents=True);lib.mkdir();bin=tool/'bin/Hostx64/x64';bin.mkdir(parents=True)
            for name in ('cl.exe','link.exe'):(bin/name).write_bytes(b'synthetic')
            values={'PATH':str(bin),'VCTOOLSINSTALLDIR':str(tool),'WINDOWSSDKDIR':str(sdk),'INCLUDE':str(include),'LIB':str(lib)}
            with patch.object(p.shutil,'which',side_effect=lambda name,**kw:str(bin/name)):
                tools,_=p.validate_sdk(values,root);self.assertEqual(tools['link.exe'],bin/'link.exe')
            with patch.object(p.shutil,'which',return_value=str(Path(tmp)/'Git/usr/bin/link.exe')),self.assertRaises(RuntimeError):p.validate_sdk(values,root)
            outside=Path(tmp)/'outside';outside.mkdir();values['LIB']=str(outside)
            with patch.object(p.shutil,'which',side_effect=lambda name,**kw:str(bin/name)),self.assertRaises(RuntimeError):p.validate_sdk(values,root)
    def test_full_bootstrap_sanitized_child_no_bytecode(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp).resolve();pf=root/'ProgramFiles';win=root/'Windows';vs=root/'VS';tool=vs/'VC/Tools/MSVC/14.4';sdk=root/'SDK';bin=tool/'bin/Hostx64/x64';out=root/'out';out.mkdir()
            files=[root/'python.exe',pf/'Microsoft Visual Studio/Installer/vswhere.exe',win/'System32/cmd.exe',win/'System32/WindowsPowerShell/v1.0/powershell.exe',vs/'Common7/Tools/VsDevCmd.bat',bin/'cl.exe',bin/'link.exe']
            for f in files:f.parent.mkdir(parents=True,exist_ok=True);f.write_bytes(b'synthetic')
            for f in (sdk/'Include',sdk/'Lib'):f.mkdir(parents=True)
            sdkvalues={'PATH':str(bin),'VCTOOLSINSTALLDIR':str(tool),'WINDOWSSDKDIR':str(sdk),'INCLUDE':str(sdk/'Include'),'LIB':str(sdk/'Lib')}
            clean={'ProgramFiles(x86)':str(pf),'SystemRoot':str(win),'PATH':'synthetic-parent'}
            signatures={name:{'valid_signature':True,'microsoft_signer':True,'microsoft_company':True,'version':[1,2,3,4]} for name in ('cl','link')}
            responses=[subprocess.CompletedProcess([],0,(str(vs)+'\n').encode()),subprocess.CompletedProcess([],0,json.dumps(sdkvalues).encode()),subprocess.CompletedProcess([],0,json.dumps(signatures).encode())]
            with patch.object(p.sys,'executable',str(root/'python.exe')),patch.object(p.n,'OUT',out),patch.object(p.n,'compile_env',return_value=clean),patch.object(p.subprocess,'run',side_effect=responses) as run,patch.object(p.shutil,'which',side_effect=lambda name,**kw:str(bin/name)),patch.dict(os.environ,{'GITHUB_TOKEN':'SYNTHETIC_SECRET'}):
                env,linker=p.bootstrap();self.assertEqual(linker,bin/'link.exe');self.assertEqual(env['PATH'],str(bin));self.assertEqual(run.call_count,3)
                child=run.call_args_list[1];self.assertEqual(child.args[0][1:4],['/d','/s','/c']);self.assertIn('" -B "',child.args[0][4]);self.assertIn('--capture-env',child.args[0][4]);self.assertNotIn('&& set',child.args[0][4])
                for call in run.call_args_list:self.assertNotIn('GITHUB_TOKEN',call.kwargs['env'])
                self.assertEqual(os.environ['GITHUB_TOKEN'],'SYNTHETIC_SECRET')
                self.assertFalse((p.HERE/'__pycache__').exists())
                self.assertNotIn('SYNTHETIC_SECRET',(out/'msvc-environment.json').read_text())
    def test_workflow_smoke_only(self):
        text=(p.n.CONTROL/'.github/workflows/ci.yml').read_text();self.assertIn('python control/scripts/msvc-proof.py',text);self.assertNotIn('python control/scripts/diagnose-native.py',text);self.assertNotIn('native-key-import.py runtime',text);self.assertNotIn('  macos:',text);self.assertNotIn('*.log',text)
        code=(p.HERE/'msvc-proof.py').read_text();self.assertNotIn('compile_product(',code);self.assertNotIn('n.runtime(',code);self.assertIn("'msvc-link-proof',20,env",code);self.assertIn("'linker='+str(linker)",code);self.assertNotIn('&& set',code);self.assertNotIn('os.environ.update(',code)
    def test_every_bootstrap_guard_emits_fixed_reason(self):
        for message,reason in p.REASONS.items():
            with tempfile.TemporaryDirectory() as tmp,patch.object(p.n,'OUT',Path(tmp)),patch.object(p,'bootstrap',side_effect=lambda m=message:p.require(False,m)),patch.object(p,'proof') as smoke:
                self.assertEqual(p.metadata_only(),1);r=json.loads((Path(tmp)/'bootstrap-status.json').read_text());self.assertEqual(r['reason'],reason);self.assertFalse(r['smoke_attempted']);smoke.assert_not_called()
    def test_each_checkpoint_exception_is_payload_free(self):
        for stage in p.CHECKPOINTS:
            def fail(stage=stage):p.checkpoint(stage);raise OSError('SYNTHETIC_SECRET path=private')
            with tempfile.TemporaryDirectory() as tmp,patch.object(p.n,'OUT',Path(tmp)),patch.object(p,'bootstrap',side_effect=fail),patch.object(p,'proof') as smoke:
                self.assertEqual(p.metadata_only(),1);text=(Path(tmp)/'bootstrap-status.json').read_text();r=json.loads(text);self.assertEqual(r['checkpoint'],stage);self.assertEqual(r['reason'],'os-error');self.assertNotIn('SECRET',text);self.assertNotIn('private',text);smoke.assert_not_called()
    def test_child_timeout_and_parse_errors_fixed(self):
        errors=[(subprocess.TimeoutExpired(['SECRET'],1),'child-timeout'),(json.JSONDecodeError('SECRET','SECRET',0),'invalid-json'),(UnicodeDecodeError('utf-8',b'x',0,1,'SECRET'),'invalid-encoding'),(FileNotFoundError('SECRET'),'file-not-found')]
        for error,reason in errors:
            with tempfile.TemporaryDirectory() as tmp,patch.object(p.n,'OUT',Path(tmp)),patch.object(p,'bootstrap',side_effect=error):
                self.assertEqual(p.metadata_only(),1);text=(Path(tmp)/'bootstrap-status.json').read_text();self.assertEqual(json.loads(text)['reason'],reason);self.assertNotIn('SECRET',text)
    def test_entrypoint_metadata_only(self):
        code=(p.HERE/'msvc-proof.py').read_text();self.assertIn('else:code=metadata_only()',code);self.assertNotIn('else:code=proof()',code)
    def test_signer_script_no_raw_export(self):
        code=(p.HERE/'tool-signatures.ps1').read_text();self.assertIn('Get-AuthenticodeSignature',code);self.assertIn("$signature.Status -eq 'Valid'",code);self.assertIn("$version.CompanyName -eq 'Microsoft Corporation'",code);self.assertNotIn('Write-Host',code);self.assertNotIn('ExecutionPolicy',code)
if __name__=='__main__':
    def suite():
        old=[previous.prior.Gates(name) for name in unittest.defaultTestLoader.getTestCaseNames(previous.prior.Gates) if name!='test_workflow']
        diag=[previous.Diagnostic(name) for name in unittest.defaultTestLoader.getTestCaseNames(previous.Diagnostic) if name!='test_diagnostic_workflow_only']
        return unittest.TestSuite(old+diag+[unittest.defaultTestLoader.loadTestsFromTestCase(BootstrapGuards)])
    runner=unittest.TextTestRunner();normal=runner.run(suite())
    with patch.object(os,'environ',previous.prior.WindowsEnvironment(dict(os.environ))):normalized=runner.run(suite())
    sys.exit(0 if normal.wasSuccessful() and normalized.wasSuccessful() else 1)
