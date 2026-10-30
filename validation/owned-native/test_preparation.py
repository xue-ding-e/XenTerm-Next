"""Pure preparation checks. Passing these is not a native execution receipt."""
from pathlib import Path
import hashlib,json,unittest,re
P=Path(__file__).resolve().parent
class Preparation(unittest.TestCase):
    def test_exact_source_inventory(self):
        values=json.loads((P/'source-hashes.json').read_text())
        for name,digest in values.items():
            self.assertEqual(hashlib.sha256((P/name).read_bytes()).hexdigest(),digest,name)
    def test_inventory_covers_all_sources_and_bootstrap_itself(self):
        inventory=json.loads((P/'source-hashes.json').read_text())
        actual={str(f.relative_to(P)) for f in P.rglob('*') if f.is_file() and f.name!='source-hashes.json'}
        self.assertEqual(set(inventory),actual)
        script=(P/'run-native.ps1').read_text()
        for name in inventory:self.assertIn("'"+name+"'",script)
        self.assertIn("Stop-Identity 'manifest-changed'",script)
        self.assertIn("Stop-Identity 'source-inventory'",script)
    def test_failure_metadata_preserves_original_failure(self):
        script=(P/'run-native.ps1').read_text();workflow=(P/'.github/workflows/ci.yml').read_text()
        self.assertIn('} finally {',script)
        self.assertIn('$initialIdentity -and $finalIdentity -and $null -ne $receipt',script)
        self.assertIn('if (-not $passed) { exit 1 }',script)
        self.assertIn('if: always() &&',workflow);self.assertIn('if-no-files-found: ignore',workflow)
        self.assertNotIn('*.log',workflow);self.assertNotIn('*.exe',workflow)
    def test_windows_checkout_preserves_byte_identity(self):
        workflow=(P/'.github/workflows/ci.yml').read_text()
        for expected in ["GIT_CONFIG_COUNT: '2'",'GIT_CONFIG_KEY_0: core.autocrlf',"GIT_CONFIG_VALUE_0: 'false'",'GIT_CONFIG_KEY_1: core.eol',"GIT_CONFIG_VALUE_1: lf"]:
            self.assertIn(expected,workflow)
        source=(P/'Fixture/Program.cs').read_bytes()
        self.assertNotEqual(hashlib.sha256(source).hexdigest(),hashlib.sha256(source.replace(b'\n',b'\r\n')).hexdigest())
    def test_fault_variant_only_injects_after_real_cleanup(self):
        source=(P/'Harness/windows-owned-process.cs').read_text()
        expected='// Test-only fault injection, not a product transport. Native cleanup and capture join run first.\n'+source.replace('public static class XenOwnedProcess {','public static class XenOwnedProcessFaultFixture {\n    public static bool CleanupObservedEmpty { get; private set; }\n    public static bool InjectCleanupUncertainty { get; set; }').replace('result.treeEmpty=true;safeToStart=true;','result.treeEmpty=true;CleanupObservedEmpty=true;if(InjectCleanupUncertainty) throw new InvalidOperationException("injected after verified empty and capture join");safeToStart=true;')
        self.assertEqual(expected,(P/'Harness/windows-owned-process-fault-fixture.cs').read_text())
    def test_boundary_diagnostics_do_not_change_success_or_cleanup(self):
        source=(P/'Harness/windows-owned-process.cs').read_text()
        snapshot=source.index('uint boundaryWait=');cleanup=source.index('var cleanupTimer=')
        self.assertLess(snapshot,cleanup)
        self.assertIn('result.boundaryActiveProcessCount=Active(job);',source)
        self.assertIn('if(boundaryWait==0)',source)
        self.assertIn('exitCode.HasValue && exitCode.Value==0 && treeEmpty && !timedOut && !outputExceeded && !captureFailed',source)
        self.assertNotIn('Process.GetProcesses',source)
        harness=(P/'Harness/Program.cs').read_text()
        for assertion in ['normal boundary empty','timeout root live','parent exit descendants remain rejected']:
            self.assertIn(assertion,harness)
    def test_root_confirmation_uses_remaining_cleanup_budget_and_checked_exit(self):
        source=(P/'Harness/windows-owned-process.cs').read_text()
        self.assertIn('var cleanupTimer=Stopwatch.StartNew();',source)
        self.assertIn('5000-deadline.ElapsedMilliseconds',source)
        self.assertIn('WaitForSingleObject(p.process,remaining)',source)
        self.assertIn('if(wait!=0) throw new Exception("root exit confirmation uncertain")',source)
        self.assertIn('Check(GetExitCodeProcess(p.process,out code),"final root exit code")',source)
        self.assertNotIn('if(GetExitCodeProcess(p.process,out code))',source)
        self.assertIn('first.Join(Remaining(deadline)) && second.Join(Remaining(deadline))',source)
        self.assertIn('capture.Join(cleanupTimer)',source)
        self.assertNotIn('Join(2500)',source)
        self.assertLess(source.index('Empty(job,cleanupTimer);'),source.index('WaitForSingleObject(p.process,remaining)'))
        self.assertLess(source.index('WaitForSingleObject(p.process,remaining)'),source.index('result.treeEmpty=true;safeToStart=true;'))
    def test_shared_budget_model_delayed_root_and_both_readers(self):
        # Pure timing model; native correctness is established only by the Windows cases.
        def cleanup(durations):
            elapsed=0; budgets=[]
            for duration in durations:
                remaining=max(0,5000-elapsed);budgets.append(remaining)
                if duration>remaining:return False,budgets,elapsed+remaining
                elapsed+=duration
            return True,budgets,elapsed
        self.assertEqual(cleanup([4200,500,200,100]),(True,[5000,800,300,100],5000))
        self.assertEqual(cleanup([4200,501,200,100]),(False,[5000,800,299,99],5000))
        self.assertEqual(cleanup([0,5001,0,0]),(False,[5000,5000],5000))
    def test_failed_native_case_preserves_bounded_observation(self):
        s=(P/'Harness/Program.cs').read_text()
        self.assertIn('lastObservation=new {name,elapsedMilliseconds=activeTimer.ElapsedMilliseconds,actualExecutions=Count(p)',s)
        self.assertLess(s.index('lastObservation=new {'),s.index('        CheckCounts(p);'))
        self.assertIn('failureCode=(e as AssertionFailure)?.Code',s)
        self.assertNotIn('failureCode=e.Message',s)
        self.assertIn('nativeErrorCode,activeCase,elapsedMilliseconds=activeTimer?.ElapsedMilliseconds,lastObservation,cases',s)
    def test_compiler_metadata_excludes_raw_diagnostics(self):
        script=(P/'run-native.ps1').read_text()
        pattern=re.search(r"Matches\(\$line,'([^']+)'\)",script).group(1)
        synthetic='C:/private/path.cs(189,9): error CS0162: PRIVATE_VALUE\nerror MSB1009: PRIVATE_VALUE\nwarning CS9999: PRIVATE_VALUE\nerror UNKNOWN123: PRIVATE_VALUE'
        self.assertEqual(re.findall(pattern,synthetic),['CS0162','MSB1009'])
        self.assertIn('@{code=$code;count=$counts[$code]}',script)
        self.assertIn('16777216',script);self.assertIn('$counts.Count -ge 32',script)
    def test_fault_switch_is_runtime_default_off_and_explicitly_enabled(self):
        source=(P/'Harness/windows-owned-process-fault-fixture.cs').read_text()
        self.assertIn('public static bool InjectCleanupUncertainty { get; set; }',source)
        self.assertNotIn('InjectCleanupUncertainty { get; set; } = true',source)
        self.assertIn('if(InjectCleanupUncertainty) throw',source)
        harness=(P/'Harness/Program.cs').read_text()
        self.assertLess(harness.index('InjectCleanupUncertainty=true'),harness.index('try {XenOwnedProcessFaultFixture.Run('))
    def test_native_cases_demand_executed_three_process_tree(self):
        s=(P/'Harness/Program.cs').read_text()
        for mode in ['normal','timeout','overflow','parent-exit','invalid-env','null-args','nul-arg','same-logs','concurrent','injected-cleanup-uncertainty']:
            self.assertIn('"'+mode+'"',s)
        for required in ['CheckCounts(p)','pids.Distinct().Count()==3','r.treeEmpty && !r.captureFailed && r.exitCode.HasValue','retained<=1024*1024','Count(denied)==0','Count(after)==0','cases.Count==10','return passed?0:1']:
            self.assertIn(required,s)
    def test_manual_exact_ref_metadata_only_no_product(self):
        s=(P/'.github/workflows/ci.yml').read_text()
        for required in ['workflow_dispatch','inputs.expected_sha == github.sha','validation/xenterm-owned-process-20261030','persist-credentials: false','timeout-minutes: 6','native-metadata/native-receipt.json','native-metadata/provenance.json']:
            self.assertIn(required,s)
        for absent in ['pull_request_target','cargo ', 'msvc-proof','native-key-import','push:', 'release:']:
            self.assertNotIn(absent,s)
        self.assertIn('<clear />',(P/'NuGet.Config').read_text())
    def test_no_fixture_count_from_source_assertions(self):
        s=(P/'run-native.ps1').read_text()
        self.assertIn('& $harness $fixture $output',s)
        self.assertIn('$nativeExit -ne 0',s)
        self.assertIn('$case.actualExecutions -ne 3',s)
        self.assertIn('Assert-SourceIdentity',s)
        self.assertIn('finalSourceIdentityVerified=$finalIdentity',s)
if __name__=='__main__':unittest.main()
