"""Static preparation checks, not PowerShell execution or native product success."""
from pathlib import Path
import ast
import unittest
ROOT=Path(__file__).parent

class Preparation(unittest.TestCase):
 def test_manual_fixed_source_and_budgets(self):
  source=(ROOT/'product-workflow.yml').read_text()
  for value in ('workflow_dispatch:', 'inputs.expected_sha == github.sha', 'timeout-minutes: 80',
                'timeout-minutes: 60', 'timeout-minutes: 10', 'timeout-minutes: 4',
                '459b593c830b8be643df29f9d9c0c46915d54696', 'cancel-in-progress: false'):
   self.assertIn(value,source)
  for value in ('  push:', '  pull_request:', 'secrets.', 'cargo run', 'cargo test', '*.log', '*.exe'):
   self.assertNotIn(value,source)
  self.assertEqual(source.count('persist-credentials: false'),2)
  self.assertGreater(60*60,90+615+2715)
 def test_fixed_sources_move_outside_control(self):
  s=(ROOT/'prepare-product-scope.ps1').read_text()
  for value in ('fresh output required', '459b593c830b8be643df29f9d9c0c46915d54696',
                "-Destination (Join-Path $root 'source')", 'control not clean after source move', 'ReparsePoint'):
   self.assertIn(value,s)
  for value in ('Remove-Item','Reset','git clean','git fetch'):
   self.assertNotIn(value,s)
 def test_two_control_builds_keep_bounded_isolation(self):
  s=(ROOT/'build-control-hosts.ps1').read_text()
  for value in ('65536','WaitForExit(60000)','WaitAll($copies,5000)',"Fail-Diagnostic 'BUILD_OUTPUT_LIMIT'",
                "$Info.Environment.Clear()", "$values['PROGRAMFILES(X86)']", "'NuGet.Config','global.json'",
                'Check-NuGetMigration;Check-CopiedInputs', 'copy inventory changed','source input changed',
                'control-host-build.json', "assembly='CompileOnlyHost'", "assembly='FetchHost'",
                "-p:BaseIntermediateOutputPath=$obj", '--disable-build-servers'):
   self.assertIn(value,s)
  self.assertNotIn('Fixture/',s);self.assertNotIn('Harness/',s)
 def test_entry_never_runs_produced_binary_or_uploads_path(self):
  s=(ROOT/'run-product-compile.py').read_text();ast.parse(s)
  self.assertIn('controller.execute(',s)
  self.assertIn("if k != 'path'",s)
  self.assertIn("product_runtime_attempted=False",s)
  self.assertNotIn('subprocess',s)
  self.assertNotIn('os.environ.copy',s)
  self.assertIn('cargo-diagnostics.json',s)
  self.assertIn('summarize(root/',s)
 def test_actual_version_script_gets_reviewed_git_bash_path(self):
  s=(ROOT/'adapter/compiler_materials.py').read_text()
  for value in ('C:/Program Files/Git/bin/bash.exe','C:/Program Files/Git/cmd/git.exe',
                "Path('C:/Program Files/Git/bin')","Path('C:/Program Files/Git/usr/bin')"):
   self.assertIn(value,s)
 def test_fetch_and_compile_both_monitor_real_resources(self):
  self.assertIn('monitor_host(process, self.root, OUTER_SECONDS)',(ROOT/'owned_fetch.py').read_text())
  self.assertIn('monitor_host(process,self.root,2715)',(ROOT/'adapter/compile_adapter.py').read_text())
  self.assertIn('process.kill()',(ROOT/'resource_monitor.py').read_text())
  self.assertIn('continuous_disk_enforcement=False',(ROOT/'resource_monitor.py').read_text())

if __name__=='__main__':unittest.main()
