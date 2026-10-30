"""Pure installed-tool bootstrap guards; no bootstrap/native compiler locally."""
import importlib.util,json,unittest,tempfile,sys,os,io,subprocess
from pathlib import Path
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('proof',Path(__file__).with_name('msvc-proof.py'));p=importlib.util.module_from_spec(spec);spec.loader.exec_module(p)
spec=importlib.util.spec_from_file_location('previous',Path(__file__).with_name('test_diagnose_native.py'));previous=importlib.util.module_from_spec(spec);spec.loader.exec_module(previous)
class BootstrapGuards(unittest.TestCase):
    def test_programdata_overlay_only_two_directories(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);data=root/'ProgramData';data.mkdir();env={'SYSTEMROOT':str(root/'Windows'),'PATH':'fixed','TEMP':tmp}
            with patch.dict(os.environ,{'ProgramData':str(data),'ALLUSERSPROFILE':str(data),'GITHUB_TOKEN':'SECRET'},clear=True):
                out=p.programdata_overlay(env);self.assertEqual(set(out)-set(env),{'PROGRAMDATA','ALLUSERSPROFILE'});self.assertEqual(out['PATH'],'fixed');self.assertNotIn('SECRET',str(out));self.assertNotIn('PROGRAMDATA',env)
    def test_programdata_overlay_rejects_missing_outside_relative_nul(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);data=root/'ProgramData';data.mkdir();other=root/'Other';other.mkdir();env={'SYSTEMROOT':str(root/'Windows')}
            for values in [{},{'PROGRAMDATA':str(data)},{'PROGRAMDATA':str(other),'ALLUSERSPROFILE':str(other)},{'PROGRAMDATA':'relative','ALLUSERSPROFILE':'relative'},{'PROGRAMDATA':'nul\x00','ALLUSERSPROFILE':str(data)}]:
                with patch.object(os,'environ',previous.prior.WindowsEnvironment(values)),self.assertRaises(p.ControlGate):p.programdata_overlay(env)
    def test_programdata_overlay_reparse_ancestor_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);data=root/'ProgramData';data.mkdir();env={'SYSTEMROOT':str(root/'Windows')};original=Path.lstat
            class Reparse:st_file_attributes=0x400;st_mode=0o040755
            def stat(path):return Reparse() if path==root else original(path)
            with patch.dict(os.environ,{'PROGRAMDATA':str(data),'ALLUSERSPROFILE':str(data)},clear=True),patch.object(Path,'lstat',stat),self.assertRaises(p.ControlGate):p.programdata_overlay(env)
    def test_paired_query_exact_delta_and_no_compiler(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);data=root/'ProgramData';data.mkdir();env={'SYSTEMROOT':str(root/'Windows'),'PROGRAMFILES(X86)':tmp,'PATH':'fixed'};calls=[]
            def run(argv,**kw):
                calls.append((argv,kw));return subprocess.CompletedProcess(argv,0,b'[]' if len(calls)==1 else b'[{"installationVersion":"17.1","isComplete":true}]')
            with patch.dict(os.environ,{'PROGRAMDATA':str(data),'ALLUSERSPROFILE':str(data),'TOKEN':'SECRET'},clear=True),patch.object(p.n,'compile_env',return_value=env),patch.object(p.n,'OUT',root),patch.object(p,'safe_cmd_path'),patch.object(p.n,'digest',return_value='0'*64),patch.object(p,'discovery_final_identities'),patch.object(p.subprocess,'run',side_effect=run),patch.object(p,'bootstrap') as boot,patch.object(p,'proof') as smoke:
                self.assertEqual(p.paired_discovery_only(),0);r=json.loads((root/'vs-paired-discovery.json').read_text());self.assertEqual(r['queries']['baseline']['count'],0);self.assertEqual(r['queries']['directory-overlay']['count'],1);self.assertNotIn('SECRET',json.dumps(r));self.assertEqual(calls[0][0],calls[1][0]);self.assertEqual(calls[0][1]['env'],env);self.assertEqual(set(calls[1][1]['env'])-set(env),{'PROGRAMDATA','ALLUSERSPROFILE'});self.assertEqual(calls[1][1]['timeout'],10);boot.assert_not_called();smoke.assert_not_called()
    def test_paired_query_error_continues_non_green(self):
        with tempfile.TemporaryDirectory() as tmp,patch.object(p.n,'OUT',Path(tmp)),patch.object(p.n,'compile_env',return_value={'PROGRAMFILES(X86)':tmp}),patch.object(p,'programdata_overlay',return_value={'PROGRAMFILES(X86)':tmp,'PROGRAMDATA':tmp,'ALLUSERSPROFILE':tmp}),patch.object(p,'safe_cmd_path'),patch.object(p.n,'digest',return_value='0'*64),patch.object(p,'discovery_final_identities'),patch.object(p.subprocess,'run',side_effect=[subprocess.TimeoutExpired(['SECRET'],10),subprocess.CompletedProcess([],0,b'[]')]) as run:
            self.assertEqual(p.paired_discovery_only(),1);r=json.loads((Path(tmp)/'vs-paired-discovery.json').read_text());self.assertEqual(run.call_count,2);self.assertEqual(r['queries']['baseline']['reason'],'child-timeout');self.assertNotIn('SECRET',json.dumps(r))
    def test_discovery_ambient_presence_reads_keys_only(self):
        code=(p.HERE/'msvc-proof.py').read_text().split('def discovery_only():',1)[1].split("if __name__==",1)[0]
        self.assertIn('parent={k.upper() for k in os.environ}',code);self.assertNotIn('os.environ.items()',code);self.assertNotIn('os.environ.values()',code)
    def test_discovery_original_counts_no_decoding_or_paths(self):
        for raw,count,nonempty in [(b'',0,0),(b'PRIVATE\r\n',1,1),(b'PRIVATE\r\n\r\n',2,1),(b'\xffPRIVATE',1,1)]:
            r=p.discovery_parse('original-property',raw);self.assertEqual(r['lines'],count);self.assertEqual(r['nonempty_lines'],nonempty);self.assertEqual(r['bytes'],len(raw));self.assertNotIn('PRIVATE',json.dumps(r))
    def test_discovery_inventory_secret_fields_omitted(self):
        raw=json.dumps([{'installationVersion':'17.14.123.4','isComplete':True,'isLaunchable':False,'isPrerelease':True,'state':4294967295,'installationPath':'SYNTHETIC_SECRET','catalog':{'displayName':'SYNTHETIC_SECRET'}}]).encode()
        r=p.discovery_parse('all-instances',raw);self.assertEqual(r['count'],1);self.assertEqual(r['instances'][0]['version'],[17,14,123,4]);self.assertNotIn('SECRET',json.dumps(r))
    def test_discovery_inventory_malformed_and_size_fail_closed(self):
        bad=[{},[None],[{'installationVersion':'SECRET'}],[{'installationVersion':'17.0','isComplete':1}],[{'installationVersion':'17.0','state':True}],[{'installationVersion':'17.0','state':-1}],[{'installationVersion':'17.0'}]*33]
        for value in bad:
            with self.assertRaises(p.ControlGate):p.discovery_inventory(json.dumps(value).encode())
        with self.assertRaises(p.ControlGate):p.discovery_parse('all-instances',b'x'*65537)
        with self.assertRaises(p.ControlGate):p.discovery_parse('UNKNOWN',b'[]')
    def test_discovery_only_queries_no_batch_compiler_secrets(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);vs=root/'Microsoft Visual Studio'/'Installer'/'vswhere.exe';vs.parent.mkdir(parents=True);vs.write_bytes(b'fixture')
            env={'PROGRAMFILES(X86)':str(root),'PATH':'fixed'}
            calls=[]
            def run(argv,**kw):
                calls.append((argv,kw));self.assertEqual(kw['env'],env);self.assertEqual(kw['timeout'],10);self.assertEqual(argv[0],str(vs));self.assertNotIn('SECRET',str(kw))
                return subprocess.CompletedProcess(argv,0,b'PRIVATE\r\n' if '-property' in argv else b'[]')
            with patch.object(p.n,'OUT',root),patch.object(p.n,'compile_env',return_value=env),patch.object(p.subprocess,'run',side_effect=run),patch.object(p,'discovery_final_identities') as identities,patch.object(p,'bootstrap') as boot,patch.object(p,'proof') as smoke:
                self.assertEqual(p.discovery_only(),0);r=json.loads((root/'vs-discovery.json').read_text());self.assertEqual(len(calls),4);self.assertEqual(r['state'],'discovery-recorded');self.assertFalse(r['sdk_batch_attempted']);self.assertNotIn('PRIVATE',json.dumps(r));identities.assert_called_once();boot.assert_not_called();smoke.assert_not_called()
                for argv,_ in calls[1:]:self.assertIn('-utf8',argv)
    def test_discovery_query_failure_continues_and_non_green(self):
        for failure in [subprocess.TimeoutExpired(['SECRET'],10),subprocess.CompletedProcess([],7,b'SECRET'),subprocess.CompletedProcess([],0,b'bad SECRET')]:
            with tempfile.TemporaryDirectory() as tmp,patch.object(p.n,'OUT',Path(tmp)),patch.object(p.n,'compile_env',return_value={'PROGRAMFILES(X86)':tmp}),patch.object(p,'safe_cmd_path'),patch.object(p.n,'digest',return_value='0'*64),patch.object(p,'discovery_final_identities'):
                replies=[subprocess.CompletedProcess([],0,b''),failure,subprocess.CompletedProcess([],0,b'[]'),subprocess.CompletedProcess([],0,b'[]')]
                with patch.object(p.subprocess,'run',side_effect=replies) as run:
                    self.assertEqual(p.discovery_only(),1);r=json.loads((Path(tmp)/'vs-discovery.json').read_text());self.assertEqual(run.call_count,4);self.assertEqual(r['queries']['all-component']['count'],0);self.assertNotIn('SECRET',json.dumps(r))
    def test_discovery_identity_failure_not_green(self):
        with tempfile.TemporaryDirectory() as tmp,patch.object(p.n,'OUT',Path(tmp)),patch.object(p.n,'compile_env',return_value={'PROGRAMFILES(X86)':tmp}),patch.object(p,'safe_cmd_path'),patch.object(p.n,'digest',return_value='0'*64),patch.object(p,'discovery_final_identities',side_effect=p.ControlGate('tree-changed')),patch.object(p.subprocess,'run',return_value=subprocess.CompletedProcess([],0,b'[]')):
            self.assertEqual(p.discovery_only(),1);r=json.loads((Path(tmp)/'vs-discovery.json').read_text());self.assertEqual(r['reason'],'tree-changed');self.assertFalse(r['product_compile_attempted'])
    def test_bootstrap_overlay_rejection_prevents_children(self):
        with patch.object(p.n,'compile_env',return_value={'SYSTEMROOT':'unused'}),patch.object(p,'programdata_overlay',side_effect=p.ControlGate('ab-directory-variable-missing')) as overlay,patch.object(p,'owned_child') as child:
            with self.assertRaises(p.ControlGate) as error:p.bootstrap()
            self.assertEqual(error.exception.reason,'ab-directory-variable-missing');overlay.assert_called_once();child.assert_not_called()
    def test_proof_links_only_and_checks_final_identity_on_success_and_failure(self):
        for fail in (False,True):
            with self.subTest(fail=fail),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);env={'PATH':'isolated'};linker=root/'link.exe';calls=[]
                def link(argv,label,seconds,actual_env):
                    calls.append((argv,label,seconds,actual_env))
                    if fail:raise RuntimeError('SYNTHETIC_PRIVATE_ERROR')
                    Path(argv[-1]).write_bytes(b'synthetic-not-executed')
                with patch.object(p.n,'OUT',root),patch.object(p.n,'platform_target',return_value='x86_64-pc-windows-msvc'),patch.object(p,'bootstrap',return_value=(env,linker)),patch.object(p.shutil,'which',return_value='rustc'),patch.object(p,'owned_link',side_effect=link),patch.object(p.owned,'smoke_receipts',return_value={'command_count':4,'all_child_trees_verified':True}),patch.object(p.n,'identities') as identities,patch.object(p.n,'patch_hashes') as hashes,patch.object(p.n,'git',side_effect=lambda *args:p.n.I['TEST_TREE'] if args==('write-tree',) else ''),patch.object(p.n,'read',return_value={'status':'','control_files':{}}),patch.dict(p.n.I,{'FIXTURES':{}}),patch.object(p.subprocess,'run') as child:
                    self.assertEqual(p.proof(),int(fail));identities.assert_called_once();hashes.assert_called_once();child.assert_not_called()
                    self.assertEqual(len(calls),1);argv,label,seconds,actual_env=calls[0]
                    self.assertEqual((label,seconds,actual_env),('msvc-link-proof',20,env));self.assertIn('linker='+str(linker),argv)
                    self.assertEqual((root/'msvc-link-proof/probe.rs').read_bytes(),p.d.SMOKE_SOURCE)
                    text=(root/'msvc-proof.json').read_text();result=json.loads(text)
                    self.assertFalse(result['executed_binary']);self.assertFalse(result['product_compile_attempted']);self.assertNotIn('PRIVATE',text)
                    self.assertEqual(result['state'],'failed-or-timeout' if fail else 'linked-not-executed')
    def test_final_identity_failure_cannot_leave_success_receipt(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)
            def link(argv,*args):Path(argv[-1]).write_bytes(b'synthetic-not-executed')
            with patch.object(p.n,'OUT',root),patch.object(p.n,'platform_target',return_value='x86_64-pc-windows-msvc'),patch.object(p,'bootstrap',return_value=({'PATH':'isolated'},root/'link.exe')),patch.object(p.shutil,'which',return_value='rustc'),patch.object(p,'owned_link',side_effect=link),patch.object(p.owned,'smoke_receipts',return_value={'command_count':4,'all_child_trees_verified':True}),patch.object(p.n,'identities',side_effect=RuntimeError('SYNTHETIC_PRIVATE_IDENTITY')):
                with self.assertRaises(RuntimeError):p.proof()
            self.assertFalse((root/'msvc-proof.json').exists(), 'success-shaped receipt must not precede identity verification')
    def test_bootstrap_failure_checks_identity_and_is_not_success(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)
            with patch.object(p.n,'OUT',root),patch.object(p.n,'platform_target',return_value='x86_64-pc-windows-msvc'),patch.object(p,'bootstrap',side_effect=RuntimeError('PRIVATE_BOOTSTRAP')),patch.object(p.n,'identities') as identities,patch.object(p.n,'patch_hashes'),patch.object(p.n,'git',side_effect=lambda *args:p.n.I['TEST_TREE'] if args==('write-tree',) else ''),patch.object(p.n,'read',return_value={'status':'','control_files':{}}),patch.dict(p.n.I,{'FIXTURES':{}}),patch.object(p.n,'run') as compiler:
                self.assertEqual(p.proof(),1);identities.assert_called_once();compiler.assert_not_called()
            text=(root/'msvc-proof.json').read_text();result=json.loads(text)
            self.assertEqual(result['state'],'failed-or-timeout');self.assertTrue(result['final_identity_verified']);self.assertNotIn('PRIVATE',text)
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
    def test_sdk_diagnostics_are_hashes_categories_only_and_do_not_accept_outside(self):
        with tempfile.TemporaryDirectory() as tmp:
            base=Path(tmp);pf=base/'Program Files x86';vs=base/'VS';sdk=pf/'Windows Kits/10';netfx=pf/'Microsoft SDKs/NETFXSDK/4.8';sibling=pf/'Windows Kits/NETFXSDK/4.8';unknown=base/'SECRET_UNKNOWN'
            for path in (vs,sdk,netfx,sibling,unknown):path.mkdir(parents=True,exist_ok=True)
            values={'INCLUDE':';'.join(map(str,[vs,sdk,netfx,sibling,unknown])),'LIB':str(unknown)}
            result=p.sdk_path_diagnostics(values,vs,sdk,pf);text=json.dumps(result)
            self.assertEqual(result['fields']['INCLUDE']['categories'],{'vs-root':1,'current-sdk-root':1,'microsoft-netfxsdk':1,'windows-kits-sibling':1,'unknown':1})
            self.assertFalse(result['changes_acceptance']);self.assertNotIn(tmp,text);self.assertNotIn('SECRET_UNKNOWN',text)
            for row in result['fields']['INCLUDE']['paths']:self.assertEqual(len(row['path_sha256']),64)
            values['LIB']=';'.join([str(unknown)]*129)
            with self.assertRaises(p.ControlGate):p.sdk_path_diagnostics(values,vs,sdk,pf)
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
            data=root/'ProgramData';data.mkdir()
            clean={'ProgramFiles(x86)':str(pf),'SystemRoot':str(win),'PATH':'synthetic-parent'}
            signatures={name:{'valid_signature':True,'microsoft_signer':True,'microsoft_company':True,'version':[1,2,3,4]} for name in ('cl','link')}
            responses=[subprocess.CompletedProcess([],0,(str(vs)+'\n').encode()),subprocess.CompletedProcess([],0,json.dumps(sdkvalues).encode()),subprocess.CompletedProcess([],0,json.dumps(signatures).encode())]
            with patch.object(p.sys,'executable',str(root/'python.exe')),patch.object(p.n,'OUT',out),patch.object(p.n,'compile_env',return_value=clean),patch.object(p,'owned_child',side_effect=responses) as run,patch.object(p.shutil,'which',side_effect=lambda name,**kw:str(bin/name)),patch.dict(os.environ,{'GITHUB_TOKEN':'SYNTHETIC_SECRET','PROGRAMDATA':str(data),'ALLUSERSPROFILE':str(data)},clear=True):
                env,linker=p.bootstrap();self.assertEqual(linker,bin/'link.exe');self.assertEqual(env['PATH'],str(bin));self.assertEqual(run.call_count,3)
                child=run.call_args_list[1];self.assertEqual(child.args[0][1:4],['/d','/s','/c']);self.assertEqual(child.args[0][4],str(out/'owned-vc-bootstrap.cmd'));self.assertNotIn('"',child.args[0][4]);batch=(out/'owned-vc-bootstrap.cmd').read_text();self.assertIn('" -B "',batch);self.assertIn('--capture-env',batch);self.assertNotIn('&& set',batch)
                for call in run.call_args_list:
                    self.assertNotIn('GITHUB_TOKEN',call.args[3])
                    self.assertEqual(call.args[3]['PROGRAMDATA'],str(data))
                    self.assertEqual(call.args[3]['ALLUSERSPROFILE'],str(data))
                self.assertNotIn('PROGRAMDATA',clean)
                self.assertEqual(os.environ['GITHUB_TOKEN'],'SYNTHETIC_SECRET')
                self.assertFalse((p.HERE/'__pycache__').exists())
                self.assertNotIn('SYNTHETIC_SECRET',(out/'msvc-environment.json').read_text())
    def test_bootstrap_batch_rejects_drift_and_existing_file(self):
        with tempfile.TemporaryDirectory(prefix='owned path ') as tmp:
            root=Path(tmp);cmd=root/'cmd.exe';script=root/'vc.cmd';python=root/'python.exe'
            for f in (cmd,script,python):f.write_bytes(b'fixture')
            with patch.object(p.n,'OUT',root),patch.object(p,'owned_child') as child:
                def mutate(*args):
                    (root/'owned-vc-bootstrap.cmd').write_bytes(b'changed')
                    return subprocess.CompletedProcess([],0,b'')
                child.side_effect=mutate
                with self.assertRaises(p.ControlGate):p.bootstrap_batch(cmd,script,python,{})
                child.reset_mock()
                with self.assertRaises(FileExistsError):p.bootstrap_batch(cmd,script,python,{})
                child.assert_not_called()
    def test_crt_quote_command_counterexample(self):
        command='call "C:\\Program Files\\VS\\vc.cmd" && "C:\\Python\\python.exe" -B proof.py'
        encoded=subprocess.list2cmdline(['cmd.exe','/d','/s','/c',command])
        self.assertIn('\\"',encoded)  # cmd does not use CRT backslash quote escapes.
        batch=subprocess.list2cmdline(['cmd.exe','/d','/s','/c','C:\\owned path\\bootstrap.cmd'])
        self.assertNotIn('\\"',batch)
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
    def test_entrypoint_smoke_only(self):
        code=(p.HERE/'msvc-proof.py').read_text();self.assertIn('else:code=proof()',code);self.assertNotIn('else:code=paired_discovery_only()',code)
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
