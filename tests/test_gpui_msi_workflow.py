"""Static gate contracts, not proof that a native MSI or GUI ran."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class WorkflowContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        cls.job = cls.workflow.split('  installer-windows:', 1)[1].split('  installer-macos:', 1)[0]

    def test_read_only_exact_source_and_existing_serial_chain(self):
        self.assertIn('permissions:\n  contents: read', self.workflow)
        self.assertNotIn('contents: write', self.job)
        self.assertIn('needs: installer', self.job)
        self.assertIn('needs: installer-windows', self.workflow.split('  installer-macos:', 1)[1])
        self.assertIn('SOURCE_SHA: ${{ github.sha }}', self.job)
        self.assertIn('ref: ${{ github.sha }}', self.job)
        self.assertIn('persist-credentials: false', self.job)
        for prohibited in ('scripts/next_beta.py', 'gh release', 'next-beta-build.yml', 'next-beta.yml', 'GH_TOKEN:'):
            self.assertNotIn(prohibited, self.job)

    def test_real_payload_and_version_bound_build(self):
        self.assertIn('cargo build --locked --release --target x86_64-pc-windows-msvc --jobs 2', self.job)
        self.assertIn('RUSTFLAGS: -C target-feature=+crt-static', self.job)
        self.assertIn('XT_VERSION: ${{ steps.gpui-version.outputs.xt_version }}', self.job)
        self.assertIn('python scripts/next_beta_package.py', self.job)
        self.assertIn('GPUI_UPGRADE_BASELINE', self.job)
        self.assertIn('(Get-FileHash $oldZip -Algorithm SHA256).Hash -ine $pin.sha256', self.job)
        self.assertIn('-SameVersionMsi $same', self.job)
        self.assertIn('-SourceSha $oldSha', self.job)
        self.assertIn('-SourceSha $env:SOURCE_SHA', self.job)
        self.assertNotIn('-UpgradeTestFixture', self.job)
        self.assertIn('not product acceptance', self.job)

    def test_actual_pe_dependency_gate_precedes_packaging(self):
        probe = '$deps = & dumpbin /DEPENDENTS "target/$env:TARGET/release/xenterm.exe"'
        self.assertIn(probe, self.job)
        self.assertIn("if ($LASTEXITCODE -ne 0) { throw 'dumpbin failed' }", self.job)
        self.assertIn('(VCRUNTIME|MSVCP|CONCRT)', self.job)
        self.assertIn("throw 'Portable EXE still needs unpackaged Microsoft C++ runtime DLLs'", self.job)
        self.assertLess(self.job.index('cargo build --locked --release'), self.job.index(probe))
        self.assertLess(self.job.index(probe), self.job.index('python scripts/next_beta_package.py'))

    def test_time_limits_and_evidence(self):
        self.assertIn('timeout-minutes: 120', self.job)
        self.assertIn('timeout-minutes: 70', self.job)
        self.assertIn('timeout-minutes: 25', self.job)
        self.assertIn('if: always()', self.job)
        self.assertIn('gpui-msi-validation-${{ github.sha }}', self.job)
        self.assertIn('gpui-installer-evidence', self.job)
        self.assertIn('gpui-real-prior.msi*', self.job)
        self.assertNotIn('choco install', self.job)

    def test_changes_retrigger_ci(self):
        for path in ('scripts/*next_gpui_msi*', 'scripts/install-wix.ps1',
                     'tests/test_next_gpui_msi.py', 'tests/test_gpui_msi_workflow.py'):
            self.assertEqual(self.workflow.count('"' + path + '"'), 2)
        self.assertIn('python3 tests/test_gpui_msi_workflow.py', self.workflow)


if __name__ == '__main__':
    unittest.main()
