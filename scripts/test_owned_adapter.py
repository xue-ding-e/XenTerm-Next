"""Pure adapter tests only: mocked Host, no Windows or compiler execution."""
import hashlib,json,os,subprocess,tempfile,unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
import owned_adapter as a

class Adapter(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.root=Path(self.tmp.name).resolve()
        self.target=self.root/'synthetic-tool.exe';self.target.write_bytes(b'fixture')
        self.host=self.root/'OwnedAdapter.exe';self.host.write_bytes(b'host-fixture')
        a.POISONED=False;a.RECORDS=[]
        self.good=dict(schema=1,state='returned',exitCode=0,timedOut=False,outputExceeded=False,treeEmpty=True,captureFailed=False,succeeded=True,safeToStart=True,productExecuted=False)
    def tearDown(self):self.tmp.cleanup()
    def invoke(self,label='test',env=None):
        return a.run([str(self.target),'synthetic'],label,20,{'PATH':'synthetic'} if env is None else env,self.root,self.root)
    def fake(self,receipt=None,size=0):
        def run(argv,**kw):
            self.assertEqual(argv[0],str(self.host));self.assertEqual(kw['timeout'],35)
            self.assertEqual(kw['stdout'],subprocess.DEVNULL);self.assertEqual(kw['stderr'],subprocess.DEVNULL)
            payload=json.loads(Path(argv[1]).read_text());self.assertEqual(payload['seconds'],20)
            Path(payload['stdout']).write_bytes(b'x'*size);Path(payload['stderr']).write_bytes(b'')
            Path(argv[2]).write_text(json.dumps(self.good if receipt is None else receipt))
            return SimpleNamespace(returncode=0)
        return run
    def test_success_requires_checked_host_and_cleanup_receipt(self):
        with patch.object(a,'identity',return_value=(self.host,{})),patch.object(a.subprocess,'run',side_effect=self.fake()) as host:
            self.assertEqual(self.invoke().returncode,0);self.assertEqual(host.call_count,1)
        self.assertFalse(a.POISONED);self.assertTrue(a.RECORDS[0]['tree_empty'])
    def test_outer_host_timeout_poison_never_claims_cleanup_or_continues(self):
        with patch.object(a,'identity',return_value=(self.host,{})),patch.object(a.subprocess,'run',side_effect=subprocess.TimeoutExpired(['PRIVATE_ARG'],35)) as host:
            with self.assertRaisesRegex(a.OwnedFailure,'unknown-host-lifecycle'):self.invoke()
            with self.assertRaisesRegex(a.OwnedFailure,'owner-poisoned'):self.invoke('next')
            self.assertEqual(host.call_count,1)
        record=(self.root/'owned-commands.json').read_text();self.assertNotIn('PRIVATE_ARG',record)
        self.assertFalse(a.RECORDS[0]['tree_empty']);self.assertTrue(a.POISONED)
    def test_unknown_exit_and_cleanup_failures_poison(self):
        for delta in [dict(exitCode=None),dict(treeEmpty=False),dict(captureFailed=True),dict(safeToStart=False),dict(succeeded=False)]:
            with self.subTest(delta=delta):
                a.POISONED=False
                name='bad-'+str(len(a.RECORDS))
                with patch.object(a,'identity',return_value=(self.host,{})),patch.object(a.subprocess,'run',side_effect=self.fake(dict(self.good,**delta))):
                    with self.assertRaises(a.OwnedFailure):self.invoke(name)
                self.assertTrue(a.POISONED)
    def test_budget_failure_is_not_success_despite_confirmed_cleanup(self):
        data=dict(self.good,exitCode=124,timedOut=True,succeeded=False)
        with patch.object(a,'identity',return_value=(self.host,{})),patch.object(a.subprocess,'run',side_effect=self.fake(data)):
            with self.assertRaisesRegex(a.OwnedFailure,'child-budget-failed'):self.invoke()
        self.assertTrue(a.RECORDS[0]['tree_empty']);self.assertFalse(a.POISONED)
    def test_environment_rejects_unknown_and_duplicate_case_before_host(self):
        for env in [{'GITHUB_TOKEN':'PRIVATE'}, {'PATH':'one','path':'two'}, {'PATH':'bad\0value'}]:
            with patch.object(a.subprocess,'run') as host:
                with self.assertRaises(a.OwnedFailure):self.invoke(env=env)
                host.assert_not_called()
    def test_output_overshoot_fails_closed(self):
        with patch.object(a,'identity',return_value=(self.host,{})),patch.object(a.subprocess,'run',side_effect=self.fake(size=1048577)):
            with self.assertRaisesRegex(a.OwnedFailure,'retained-output-budget-exceeded'):self.invoke()
        self.assertTrue(a.POISONED)
    def test_artifact_change_cannot_leave_success(self):
        with patch.object(a,'identity',side_effect=[(self.host,{}),(self.host,{'changed':True})]),patch.object(a.subprocess,'run',side_effect=self.fake()):
            with self.assertRaisesRegex(a.OwnedFailure,'adapter-identity-changed'):self.invoke()
        self.assertTrue(a.POISONED)
    def test_qualified_csharp_exact_bytes_and_host_has_no_other_execution(self):
        source=(a.HERE/'owned_adapter/windows-owned-process.cs').read_bytes()
        self.assertEqual(hashlib.sha256(source).hexdigest(),a.TRANSPORT_SHA256)
        host=(a.HERE/'owned_adapter/Program.cs').read_text()
        self.assertIn('XenOwnedProcess.Run(',host);self.assertNotIn('Process.Start(',host)
        proof=(a.HERE/'msvc-proof.py').read_text();bootstrap=proof[proof.index('def bootstrap():'):proof.index('def proof():')]
        self.assertNotIn('subprocess.run',bootstrap);self.assertEqual(bootstrap.count('result=owned_child('),3)
        self.assertIn("owned_link([rustc,'+1.99.1'",proof)
    def test_smoke_receipt_cannot_accept_missing_duplicate_or_incomplete_children(self):
        good=lambda label:dict(label=label,state='returned',tree_empty=True,host_lifecycle='returned-verified',exit_code=0,timed_out=False,output_exceeded=False,capture_failed=False)
        labels=['vswhere','vc-bootstrap','signatures','msvc-link-proof']
        a.RECORDS=[good(label) for label in labels]
        self.assertEqual(a.smoke_receipts()['command_count'],4)
        for records in [a.RECORDS[:3],[good(labels[0])]*4,[dict(good(label),tree_empty=False) for label in labels]]:
            a.RECORDS=records
            with self.assertRaises(a.OwnedFailure):a.smoke_receipts()
    def test_prepare_covers_adapter_and_workflow_never_exports_requests(self):
        prepare=(a.HERE/'native-key-import.py').read_text()
        for name in ['owned_adapter.py','test_owned_adapter.py','build-owned-adapter.ps1']:
            self.assertIn("HERE/'"+name+"'",prepare)
        self.assertIn("sorted((HERE/'owned_adapter').glob('*'))",prepare)
        workflow=(a.HERE.parent/'.github/workflows/ci.yml').read_text()
        self.assertIn('build-owned-adapter.ps1',workflow)
        self.assertNotIn('*.log',workflow);self.assertNotIn('request.json',workflow);self.assertNotIn('owned-adapter-local.json',workflow)
    def test_no_product_cache_and_compiler_diagnostics_are_codes_only(self):
        workflow=(a.HERE.parent/'.github/workflows/ci.yml').read_text()
        self.assertNotIn('rust-cache',workflow)
        build=(a.HERE/'build-owned-adapter.ps1').read_text()
        for required in ['CS[0-9]{4}|MSB[0-9]{4}|NU[0-9]{4}', '16777216', '$counts.Count -ge 32', '[Math]::Min(1000', 'compilerCodes=$compilerCodes', 'ConvertTo-Json -Depth 8']:
            self.assertIn(required,build)
        self.assertNotIn('Write-Host',build)
    def test_receipt_duplicate_fields_rejected(self):
        path=self.root/'duplicate.json';path.write_text('{"treeEmpty":false,"treeEmpty":true}')
        with self.assertRaisesRegex(a.OwnedFailure,'duplicate-json-key'):a.checked_json(path,1000)
if __name__=='__main__':unittest.main()
