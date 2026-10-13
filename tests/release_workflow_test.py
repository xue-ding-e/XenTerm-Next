#!/usr/bin/env python3
"""Exercise the actual version resolver without invoking packaging or publishing."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = (ROOT / '.github/workflows/release.yml').read_text()


class ReleaseWorkflowTest(unittest.TestCase):
    def test_dry_run_feature_branch_is_artifact_safe_and_never_publishes(self):
        resolver = WORKFLOW.split('        run: |\n', 1)[1].split('\n  # Fork', 1)[0]
        resolver = '\n'.join(line[10:] for line in resolver.splitlines())
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'outputs'
            env = os.environ | {
                'INPUT_MODE': 'dry-run', 'GITHUB_EVENT_NAME': 'workflow_dispatch',
                'GITHUB_REF_TYPE': 'branch', 'GITHUB_REF_NAME': 'feat/installer-cli-integration',
                'GITHUB_SHA': 'a' * 40, 'GITHUB_OUTPUT': str(output),
            }
            subprocess.run(['bash', '-eu', '-c', resolver], cwd=ROOT, env=env, check=True)
            values = dict(line.split('=', 1) for line in output.read_text().splitlines())
            self.assertEqual(values['mode'], 'dry-run')
            self.assertEqual(values['version'], 'dry-run-' + 'a' * 12)
            self.assertNotIn('/', values['version'])
        publish = WORKFLOW.split('\n  publish:', 1)[1]
        self.assertIn("if: needs.prepare.outputs.mode != 'dry-run'", publish)

    def test_native_package_failures_cannot_be_hidden(self):
        msi = WORKFLOW.split('      - name: Build MSI installer (Windows)', 1)[1].split('\n      - name:', 1)[0]
        self.assertNotIn('continue-on-error: true', msi)
        for name in ('Upload MSI artifact', 'Upload macOS Installer artifact'):
            step = WORKFLOW.split('      - name: ' + name, 1)[1].split('\n      - name:', 1)[0]
            self.assertIn('if-no-files-found: error', step)

    def test_beta_installer_version_is_numeric(self):
        result = subprocess.check_output(['bash', '-c', 'APP_VERSION=0.4.2-beta; printf %s "${APP_VERSION%%[-+]*}"'], text=True)
        self.assertEqual(result, '0.4.2')
        self.assertIn('PKG_VERSION="${APP_VERSION%%[-+]*}"', WORKFLOW)


if __name__ == '__main__':
    unittest.main()
