"""Offline wiring contracts; native build and installation require actual CI."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class WorkflowContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.ci = (ROOT / '.github/workflows/ci.yml').read_text()
        cls.windows = cls.ci.split('  installer-windows:', 1)[1].split('  installer-macos:', 1)[0]
        cls.builder = (ROOT / '.github/workflows/next-beta-build.yml').read_text()
        cls.publisher = (ROOT / '.github/workflows/next-beta.yml').read_text()

    def test_readonly_exact_source_and_serial_order(self):
        for text in (self.ci, self.builder):
            self.assertIn('permissions:\n  contents: read', text)
            self.assertNotIn('contents: write', text)
        self.assertIn('needs: installer', self.windows)
        self.assertIn('needs: installer-windows', self.ci.split('  installer-macos:', 1)[1])
        self.assertIn('SOURCE_SHA: ${{ github.sha }}', self.windows)
        self.assertIn('ref: ${{ github.sha }}', self.windows)
        self.assertIn('SOURCE_SHA: ${{ inputs.source_sha }}', self.builder)
        self.assertIn('ref: ${{ inputs.source_sha }}', self.builder)
        self.assertIn('max-parallel: 1', self.builder)
        self.assertIn('fail-fast: true', self.builder)
        self.assertIn('timeout-minutes: 120', self.windows)
        self.assertIn('timeout-minutes: 60', self.windows)
        self.assertIn("timeout-minutes: ${{ matrix.os == 'windows-latest' && 120 || 90 }}", self.builder)
        self.assertIn("timeout-minutes: ${{ matrix.os == 'windows-latest' && 70 || 90 }}", self.builder)
        for text in (self.windows, self.builder):
            for prohibited in ('gh release', 'GH_TOKEN:', 'run: python3 scripts/next_beta.py'):
                self.assertNotIn(prohibited, text)
            self.assertIn('persist-credentials: false', text)

    def test_preflight_and_real_source_bound_build(self):
        for text in (self.windows, self.builder):
            for name in ('profile_environment', 'gui_observation', 'initialization', 'registration', 'msi_property'):
                self.assertIn('tests/test_gpui_' + name + '.ps1', text)
            self.assertLess(text.index('Verify synthetic profile preservation contracts'), text.index('cargo build --locked --release'))
            self.assertIn('target-feature=+crt-static', text)
            self.assertIn('dumpbin /DEPENDENTS', text)
            self.assertIn('Portable EXE still needs unpackaged Microsoft C++ runtime DLLs', text)
            self.assertIn('scripts/next_beta_package.py', text)
            self.assertIn('XENTERM_VERSION_METADATA:', text)
        self.assertIn('not product acceptance', self.windows)
        self.assertIn('windows_installer_e2e.ps1 -WixBin $env:GPUI_WIX_BIN', self.windows)

    def test_same_real_lifecycle_and_old_baseline_gates(self):
        for text in (self.windows, self.builder):
            self.assertIn('from next_beta import GPUI_UPGRADE_BASELINE', text)
            self.assertIn('(Get-Item -LiteralPath $oldZip).Length -ne $pin.size', text)
            self.assertIn('(Get-FileHash $oldZip -Algorithm SHA256).Hash -ine $pin.sha256', text)
            self.assertLess(text.index('Prior real GPUI archive hash mismatch'), text.index('Expand-Archive -LiteralPath $oldZip'))
            self.assertLess(text.index('tests/test_gpui_native_environment.ps1'), text.index('-Output $previous'))
            self.assertIn('-SourceSha $oldSha', text)
            self.assertIn('-SourceSha $env:SOURCE_SHA', text)
            self.assertIn('test_gpui_msi_property.ps1 -MsiPaths @($msi,$previous,$same)', text)
            self.assertIn('test_next_gpui_msi_e2e.ps1 -Msi $msi -PreviousMsi $previous -SameVersionMsi $same', text)
            self.assertNotIn('-UpgradeTestFixture', text)
            self.assertIn('timeout-minutes: 25', text)
            self.assertIn('gpui-packaging-evidence', text)
            self.assertIn('gpui-real-prior.msi*', text)
            self.assertIn('gpui-equal-build-fixture.msi*', text)

    def test_windows_release_artifacts_wait_for_actual_lifecycle(self):
        text = self.builder
        lifecycle = text.index('name: Build and verify real GPUI MSI from this exact release bundle')
        upload = text.index('name: next-${{ matrix.platform }}')
        self.assertLess(lifecycle, upload)
        segment = text[lifecycle:upload]
        self.assertIn("if: runner.os == 'Windows'", segment)
        self.assertIn("if: always() && runner.os == 'Windows'", segment)
        self.assertNotIn('continue-on-error', text)
        self.assertIn('needs: [prepare, build]', text)
        self.assertIn("prepare_assets(Path('dist')", text)
        self.assertIn('pattern: next-*', text)
        self.assertIn('name: gpui-installer-evidence', segment)
        self.assertNotIn('name: next-gpui-installer-evidence', text)
        self.assertLess(segment.index("if: always() && runner.os == 'Windows'"), segment.index('name: gpui-installer-evidence'))
        formal = text[upload:].split('  verify-artifacts:', 1)[0]
        self.assertNotIn('path: dist/*', formal)
        for extension in ('.zip', '.tar.gz', '.msi', '.msi.json'):
            self.assertIn('dist/XenTerm-Next-beta-${{ inputs.source_sha }}-${{ matrix.platform }}' + extension, formal)
        self.assertNotIn('wixpdb', formal)
        self.assertIn('dist/*.wixpdb', text[lifecycle:upload])

    def test_paths_and_main_gate_remain_explicit(self):
        for path in ('scripts/*next_gpui_msi*', 'scripts/install-wix.ps1', 'scripts/gpui_msi_*',
                     'scripts/initialize_gpui_test_profile.ps1', 'tests/test_next_gpui_msi.py',
                     'tests/test_gpui*', 'tests/fixtures/gpui-heat-component-group.wxs'):
            self.assertEqual(self.ci.count('"' + path + '"'), 2)
        self.assertIn('python3 tests/test_gpui_msi_workflow.py', self.ci)
        self.assertIn("run.head_sha !== main.object.sha", self.publisher)
        self.assertIn("run.path !== '.github/workflows/ci.yml'", self.publisher)
        self.assertIn('needs: [gate, build]', self.publisher)
        self.assertNotIn('if: always()', self.publisher.split('  publish:', 1)[1])
        self.assertIn("run.conclusion !== 'success'", self.publisher)
        self.assertNotIn('if: always()', self.builder.split('  verify-artifacts:', 1)[1])
        self.assertIn('run: python3 scripts/next_beta.py', self.publisher)


if __name__ == '__main__':
    unittest.main()
