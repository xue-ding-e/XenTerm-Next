"""Source-only contracts; this test never builds or invokes Windows programs."""
import hashlib
import unittest
from pathlib import Path
import xml.etree.ElementTree as ET

HERE = Path(__file__).parent
HOST = HERE.parent / 'fetch-native/Host'


class HostEntryPreparation(unittest.TestCase):
    def test_original_host_production_hashes_still_pinned(self):
        expected = {
            'Program.cs': '0f94cd746e8a04d7bb5e794683a30b7512d406319eccb3e3c4e6fb3d79e29ae3',
            'fetch-owned-process.cs': 'd6d9d3b5c31a2f697eb435cdbf99f0fc968e2af912b069923bc6f8c7497331b3',
            'FetchHost.csproj': 'fdcf2d82f51d9986e855c388eeafb0a67b186ffbfc31a0bf47fab47ba52defa9',
        }
        script = (HERE / 'run-host-entry.ps1').read_text()
        for name, digest in expected.items():
            self.assertEqual(hashlib.sha256((HOST / name).read_bytes()).hexdigest(), digest)
            self.assertIn(digest, script)
        self.assertIn('Check-Production;Check-Copies', script)

    def test_fixture_is_only_fixed_fetch_no_network_or_children(self):
        fixture = (HERE / 'Fixture/Program.cs').read_text()
        self.assertIn('new[]{"fetch","--locked","--target","x86_64-pc-windows-msvc"}', fixture)
        for forbidden in ('Process.Start', 'HttpClient', 'WebRequest', 'Socket', 'Assembly.Load'):
            self.assertNotIn(forbidden, fixture)
        self.assertIn('return mode=="normal" ? 0 : 7;', fixture)

    def test_real_host_entry_receipts_and_negative_case_inventory(self):
        source = (HERE / 'Harness/Program.cs').read_text()
        self.assertIn('new ProcessStartInfo(host)', source)
        for name in ('fixed-success', 'consumed-root', 'marker-only-consumed-root', 'compile-argv', 'wrong-policy',
                     'wrong-deadline', 'extra-environment', 'case-colliding-environment',
                     'wrong-tool-path', 'duplicate-json', 'reparse-executable',
                     'nonzero-child', 'nonzero-consumed-root'):
            self.assertIn('"' + name + '"', source)
        self.assertIn('cases.Count==13', source)
        for key in ('boundaryRootExitCode', 'boundaryActiveProcessCount', 'outputBytes',
                    'cleanupMilliseconds', 'succeeded', 'forcedOwnedCleanup'):
            self.assertIn('"' + key + '"', source)

    def test_timeout_is_failure_and_link_failure_is_not_skipped(self):
        source = (HERE / 'Harness/Program.cs').read_text()
        self.assertIn('WaitForExit(20000)', source)
        self.assertIn('process.Kill(true)', source)
        self.assertIn('throw new InvalidOperationException("synthetic Host outer timeout")', source)
        self.assertIn('File.CreateSymbolicLink(cargo,fixture)', source)
        self.assertNotIn('skipped=true', source)
        self.assertIn('outerJobObjectQualification=false', source)
        self.assertIn('hostEntryExecuted=hostStarts>0', source)

    def test_projects_and_no_workflow_or_qualification_unlock(self):
        for path, name in (('Fixture/Fixture.csproj', 'cargo'), ('Harness/Harness.csproj', 'FetchHostEntryHarness')):
            project = ET.fromstring((HERE / path).read_text())
            self.assertEqual(project.findtext('PropertyGroup/AssemblyName'), name)
            self.assertEqual(project.findtext('PropertyGroup/TargetFramework'), 'net8.0')
        self.assertFalse((HERE / '.github').exists())
        script = (HERE / 'run-host-entry.ps1').read_text()
        self.assertIn('WaitForExit(90000)', script)
        self.assertIn('WaitForExit(60000)', script)
        self.assertIn('$info.WorkingDirectory=$work', script)
        self.assertIn('foreach($exe in @($fetchHost,$fixture,$harness))', script)
        self.assertIn('foreach($p in $runtime.Keys)', script)
        self.assertNotIn('workflow_dispatch', script)


if __name__ == '__main__':
    unittest.main()
