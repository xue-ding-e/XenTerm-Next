"""Pure contracts for manual synthetic Host control. Never builds or launches."""
import hashlib
import json
import re
import unittest
from pathlib import Path
import xml.etree.ElementTree as ET

ROOT = Path(__file__).parent


class HostEntryControl(unittest.TestCase):
    def test_exact_fifteen_inputs_and_hashes(self):
        manifest = json.loads((ROOT / 'source-hashes.json').read_text())
        actual = {p.relative_to(ROOT).as_posix() for p in ROOT.rglob('*') if p.is_file()}
        self.assertEqual(len(manifest), 15)
        self.assertEqual(actual, set(manifest) | {'source-hashes.json'})
        for name, sha in manifest.items():
            self.assertEqual(hashlib.sha256((ROOT / name).read_bytes()).hexdigest(), sha)

    def test_original_host_unchanged_and_compile_inventory(self):
        pinned = {'Program.cs': '0f94cd746e8a04d7bb5e794683a30b7512d406319eccb3e3c4e6fb3d79e29ae3',
                  'fetch-owned-process.cs': 'd6d9d3b5c31a2f697eb435cdbf99f0fc968e2af912b069923bc6f8c7497331b3',
                  'FetchHost.csproj': 'fdcf2d82f51d9986e855c388eeafb0a67b186ffbfc31a0bf47fab47ba52defa9'}
        for name, sha in pinned.items():
            self.assertEqual(hashlib.sha256((ROOT / 'Host' / name).read_bytes()).hexdigest(), sha)
        project = ET.fromstring((ROOT / 'Host/FetchHost.csproj').read_text())
        self.assertEqual(project.findtext('PropertyGroup/EnableDefaultCompileItems'), 'false')
        self.assertEqual([node.attrib['Include'] for node in project.findall('ItemGroup/Compile')], ['Program.cs', 'fetch-owned-process.cs'])

    def test_thirteen_cases_match_harness(self):
        expected = json.loads((ROOT / 'expected-cases.json').read_text())
        self.assertEqual(len(expected), 13)
        self.assertEqual(len({row['name'] for row in expected}), 13)
        source = (ROOT / 'Harness/Program.cs').read_text()
        for row in expected:
            self.assertIn('"' + row['name'] + '"', source)
        self.assertEqual(sum(row['syntheticCargoExecutions'] for row in expected), 2)
        for text in ('cases.Count==13', 'marker-only-consumed-root', 'outerJobObjectQualification=false',
                     'hostEntryExecuted=hostStarts>0', 'info.Environment.Clear()', 'WaitForExit(20000)',
                     'File.CreateSymbolicLink(cargo,fixture)', 'realCargoExecuted=false'):
            self.assertIn(text, source)
        self.assertNotIn('skipped=true', source)

    def test_fixture_never_launches_children_or_network(self):
        source = (ROOT / 'Fixture/Program.cs').read_text()
        self.assertIn('new[]{"fetch","--locked","--target","x86_64-pc-windows-msvc"}', source)
        for name in ('Process.Start', 'HttpClient', 'WebRequest', 'Socket', 'Assembly.Load'):
            self.assertNotIn(name, source)

    def test_executing_cases_retain_only_actual_lifecycle_whitelist(self):
        harness = (ROOT / 'Harness/Program.cs').read_text()
        control = (ROOT / 'run-host-entry.ps1').read_text()
        self.assertIn('hostReceipt=successReceipt', harness)
        self.assertIn('hostReceipt=nonzeroReceipt', harness)
        self.assertEqual(harness.count('hostReceipt='), 2)
        self.assertIn('r.GetProperty(key).GetInt64()', harness)
        self.assertIn('r.GetProperty(key).GetBoolean()', harness)
        for key in ('exitCode', 'boundaryRootExitCode', 'treeEmpty', 'safeToStart',
                    'boundaryActiveProcessCount', 'cleanupMilliseconds', 'outputBytes',
                    'outputBudget', 'succeeded', 'forcedOwnedCleanup', 'timedOut',
                    'outputExceeded', 'captureFailed'):
            self.assertIn("'" + key + "'", control)
        for text in ("$retained['hostReceipt']=$safeProof", 'Host lifecycle integer required',
                     'Host lifecycle boolean required', 'Host cleanup/output bounds',
                     'rejected case cannot claim a Host receipt'):
            self.assertIn(text, control)
        self.assertNotIn("$retained['hostReceipt']=$proof", control)

    def test_manual_fixed_sha_readonly_limits_and_metadata_only(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        for text in ('workflow_dispatch:', 'inputs.expected_sha == github.sha', 'contents: read',
                     'persist-credentials: false', 'timeout-minutes: 8', 'timeout-minutes: 6',
                     'validation/xenterm-fetch-host-entry-20261031', './validation/fetch-host-entry-native/run-host-entry.ps1'):
            self.assertIn(text, workflow)
        for text in ('push:', 'pull_request:', 'schedule:', '*.log', '*.exe'):
            self.assertNotIn(text, workflow)
        upload = workflow.split('name: fetch-host-entry-native-metadata')[1]
        self.assertEqual(re.findall(r'^\s+path: (.*)$', upload, re.M), ['${{ runner.temp }}/xenterm-host-entry-${{ github.sha }}/metadata.json'])

    def test_subprocess_environment_cleared_and_fixed_dotnet(self):
        source = (ROOT / 'run-host-entry.ps1').read_text()
        env = source.split('function Set-ChildEnvironment')[1].split('function Build')[0]
        self.assertIn('$Info.Environment.Clear()', env)
        self.assertNotIn('$env:', env)
        self.assertNotIn('GetEnvironmentVariables', env)
        self.assertIn("$dotnet='C:\\Program Files\\dotnet\\dotnet.exe'", source)
        self.assertNotIn('Get-Command dotnet', source)
        self.assertEqual(source.count('Set-ChildEnvironment $info'), 2)
        for text in ('DOTNET_CLI_TELEMETRY_OPTOUT', 'DOTNET_SKIP_FIRST_TIME_EXPERIENCE', 'isolated/nuget', 'isolated/home'):
            self.assertIn(text, env)

    def test_timeouts_fail_and_streams_have_zero_retention(self):
        source = (ROOT / 'run-host-entry.ps1').read_text()
        for text in ('WaitForExit(60000)', 'WaitForExit(90000)', '$info.WorkingDirectory=$work',
                     'lifecycle unknown', '[IO.Stream]::Null', '--disable-build-servers'):
            self.assertIn(text, source)
        self.assertNotIn('build.stdout.log', source)
        self.assertNotIn('[IO.File]::Open', source)
        self.assertEqual(source.count('CopyToAsync([IO.Stream]::Null)'), 4)

    def test_snapshot_runtime_and_sanitized_failure_contract(self):
        source = (ROOT / 'run-host-entry.ps1').read_text()
        for text in ('Check-Identity', 'Check-Copies', '$runtime.Count -ne 12',
                     "$runtimeEvidence[$label+'/'+$base+$suffix]", 'foreach($path in $runtime.Keys)',
                     '$Record.Exception.GetType().Name', '$entry=$null', '$allowedFailureCases',
                     'outerJobObjectQualification=$false', 'fullTenMinuteElapsedValidation=$false'):
            self.assertIn(text, source)
        for forbidden in ('$_.Exception.Message', '$_.ScriptStackTrace', 'runtimeFiles=$runtime;'):
            self.assertNotIn(forbidden, source)

    def test_primary_and_final_failures_cannot_overwrite_each_other(self):
        source = (ROOT / 'run-host-entry.ps1').read_text()
        self.assertIn('catch {$primaryFailure=Failure-Snapshot $_}', source)
        self.assertIn('catch {$passed=$false;$finalFailure=Failure-Snapshot $_}', source)
        final_block = source.split('$inFinal=$true')[1]
        self.assertNotIn('$primaryFailure=', final_block)
        self.assertIn('primaryFailure=$primaryFailure;finalFailure=$finalFailure', final_block)
        self.assertNotIn('$failureType=', source)
        snapshot = source.split('function Failure-Snapshot')[1].split('function Extra-Class')[0]
        for unsafe in ('.Message', '.StackTrace', '.ToString()', '.InvocationInfo'):
            self.assertNotIn(unsafe, snapshot)
        self.assertIn("else{'OTHER'}", snapshot)
        self.assertIn("$kind='OtherException'", snapshot)

    def test_build_outcomes_keep_unknown_values_and_fixed_checkpoints(self):
        source = (ROOT / 'run-host-entry.ps1').read_text()
        for text in ('started=$false;exited=$null;exitCode=$null;streamsComplete=$null;timedOut=$null',
                     '$outcome.started=$true', '$outcome.exited=$false;$outcome.timedOut=$true',
                     '$outcome.exitCode=$process.ExitCode', '$outcome.streamsComplete=[Threading.Tasks.Task]::WaitAll',
                     "Fail-Diagnostic 'BUILD_EXIT'", "Fail-Diagnostic 'BUILD_DRAIN'"):
            self.assertIn(text, source)
        self.assertIn('buildOutcomes=@($buildOutcomes.ToArray())', source)

    def test_copy_diagnostics_export_indices_counts_and_only_fixed_classes(self):
        source = (ROOT / 'run-host-entry.ps1').read_text()
        body = source.split('function Extra-Class')[1].split('function Plain')[0]
        classes = set(re.findall(r"return '([^']+)'", body))
        self.assertEqual(classes, {'json', 'props', 'targets', 'cs', 'dll', 'exe', 'pdb', 'cache', 'txt', 'other'})
        self.assertNotIn('return $Relative', body)
        check = source.split('function Check-Copies')[1].split('function Set-ChildEnvironment')[0]
        self.assertIn('$check.missingIndices+= $i', check)
        self.assertIn('$check.changedIndices+= $i', check)
        self.assertIn('$class=Extra-Class $relative', check)
        self.assertNotIn('$check.extraClasses[$relative]', check)
        for field in ('missingIndices', 'changedIndices', 'extraCount', 'extraClasses', 'scanComplete'):
            self.assertIn(field, check)
        self.assertIn("Fail-Diagnostic 'COPY_EXTRA'", check)
        self.assertIn('Compare-Object ($compileInputs|Sort-Object)', check)
        # Reference classification negatives: secret-bearing filenames or unknown
        # extensions have only a fixed class output, never their original text.
        known = {'.' + item: item for item in classes - {'other'}}
        for name, want in [('private-token.bin', 'other'), ('secret.json', 'json'),
                           ('private.USER_VALUE', 'other'), ('build.PROPS', 'props')]:
            value = known.get(Path(name).suffix.lower(), 'other')
            self.assertEqual(value, want)
            self.assertNotIn(name, value)


if __name__ == '__main__':
    unittest.main()
