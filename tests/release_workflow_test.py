#!/usr/bin/env python3
"""Exercise the workflow's real resolver without building or publishing artifacts."""
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = (ROOT / '.github/workflows/release.yml').read_text()
RESOLVE = WORKFLOW.split('        run: |\n', 1)[1].split('\n  # Fork', 1)[0]
RESOLVE = '\n'.join(line[10:] for line in RESOLVE.splitlines())
SOURCE_SHA = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
CARGO_VERSION = re.search(r'^version = "([^"]+)"', (ROOT / 'Cargo.toml').read_text(), re.M)[1]


class ReleaseWorkflowTest(unittest.TestCase):
    def resolve(self, mode='beta', event='workflow_dispatch', ref_type='branch',
                ref_name='main', repository='ixbaicn/XenTerm', cargo_version=None,
                lock_version=None, source_sha=SOURCE_SHA):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'scripts').mkdir()
            shutil.copyfile(ROOT / 'scripts/version.sh', root / 'scripts/version.sh')
            (root / 'Cargo.toml').write_text(
                '[package]\nname = "xenterm"\nversion = "' + (cargo_version or CARGO_VERSION) + '"\n')
            (root / 'Cargo.lock').write_text(
                '[[package]]\nname = "xenterm"\nversion = "' + (lock_version or CARGO_VERSION) + '"\n')
            # The resolver is real. Only git metadata is stubbed so the fixture
            # never writes repository refs or depends on tags attached to test HEAD.
            (root / 'bin').mkdir()
            git = root / 'bin/git'
            git.write_text('#!/bin/sh\nif [ "$*" = "rev-parse HEAD" ]; then\n'
                           '  printf "%s\\n" "' + source_sha + '"\nelse\n  exit 1\nfi\n')
            git.chmod(0o755)
            output = root / 'outputs'
            inherited_json = root / 'inherited-version.json'
            env = os.environ | {
                'INPUT_MODE': mode, 'GITHUB_EVENT_NAME': event,
                'GITHUB_REF_TYPE': ref_type, 'GITHUB_REF_NAME': ref_name,
                'GITHUB_REPOSITORY': repository,
                # An event/stale shell must not override the checked-out source
                # or leak package labels into the runtime resolver.
                'GITHUB_SHA': 'a' * 40, 'GITHUB_OUTPUT': str(output),
                'XT_VERSION': 'xt-v9.9.9', 'XT_CHANNEL': 'alpha',
                'VERSION': 'inherited-package-label',
                'XT_VERSION_JSON': str(inherited_json),
                'RUNNER_TEMP': str(root), 'TMPDIR': str(root),
                'PATH': str(root / 'bin') + os.pathsep + os.environ['PATH'],
            }
            result = subprocess.run(['bash', '-eu', '-c', RESOLVE], cwd=root,
                                    env=env, text=True, capture_output=True)
            values = (dict(line.split('=', 1) for line in output.read_text().splitlines())
                      if output.exists() else {})
            self.assertFalse((root / 'version.json').exists())
            self.assertFalse(inherited_json.exists())
            return result, values

    def test_dry_run_feature_branch_is_artifact_safe_and_never_publishes(self):
        result, values = self.resolve(mode='dry-run', ref_name='feat/installer-cli-integration')
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertEqual(values['mode'], 'dry-run')
        self.assertEqual(values['version'], 'dry-run-' + SOURCE_SHA[:12])
        self.assertNotIn('/', values['version'])
        self.assertEqual(values['deb_version'], CARGO_VERSION)
        self.assertEqual(values['app_version'], CARGO_VERSION)
        self.assertRegex(values['runtime_version'], r'^xt-dev-' + SOURCE_SHA[:7] + r'-T[0-9]{10}$')
        publish = WORKFLOW.split('\n  publish:', 1)[1]
        self.assertIn("if: needs.prepare.outputs.mode != 'dry-run'", publish)

    def test_beta_uses_one_runtime_version_without_changing_package_versions(self):
        for event, ref_type, ref_name in [('push', 'branch', 'main'),
                                           ('workflow_dispatch', 'branch', 'feat/package'),
                                           ('workflow_dispatch', 'tag', 'v' + CARGO_VERSION)]:
            with self.subTest(event=event, ref_type=ref_type):
                result, values = self.resolve(event=event, ref_type=ref_type, ref_name=ref_name)
                self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
                self.assertEqual(values['mode'], 'beta')
                self.assertEqual(values['version'], 'beta-' + SOURCE_SHA[:7])
                self.assertEqual(values['deb_version'], CARGO_VERSION + '~beta.' + SOURCE_SHA[:7])
                self.assertEqual(values['app_version'], CARGO_VERSION + '-beta')
                self.assertRegex(values['runtime_version'], r'^xt-beta-' + SOURCE_SHA[:7] + r'-T[0-9]{10}$')
                self.assertEqual(result.stderr.count('[version.sh] resolved '), 1)

    def test_stable_push_and_retry_use_the_tag_runtime_and_semver_packages(self):
        for event in ('push', 'workflow_dispatch'):
            with self.subTest(event=event):
                result, values = self.resolve(mode='stable', event=event, ref_type='tag',
                                              ref_name='v' + CARGO_VERSION)
                self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
                self.assertEqual(values['mode'], 'stable')
                self.assertEqual(values['version'], 'v' + CARGO_VERSION)
                self.assertEqual(values['runtime_version'], 'xt-v' + CARGO_VERSION)
                self.assertEqual(values['deb_version'], CARGO_VERSION)
                self.assertEqual(values['app_version'], CARGO_VERSION)

    def test_stable_push_and_retry_reject_manifest_and_lock_mismatches(self):
        for event in ('push', 'workflow_dispatch'):
            for manifest in ('cargo_version', 'lock_version'):
                with self.subTest(event=event, manifest=manifest):
                    result, values = self.resolve(mode='stable', event=event, ref_type='tag',
                                                  ref_name='v' + CARGO_VERSION,
                                                  **{manifest: '9.9.9'})
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn('does not match Cargo.', result.stdout)
                    self.assertEqual(values, {})

    def test_stable_requires_a_plain_version_tag(self):
        for ref_type, ref_name in [('branch', 'main'), ('tag', 'beta'),
                                   ('tag', CARGO_VERSION), ('tag', 'v' + CARGO_VERSION + '-rc.1')]:
            with self.subTest(ref_name=ref_name):
                result, values = self.resolve(mode='stable', ref_type=ref_type, ref_name=ref_name)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(values, {})

    def test_next_cannot_bypass_its_single_beta_publisher(self):
        result, values = self.resolve(repository='xue-ding-e/XenTerm-Next')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('only by next-beta.yml', result.stdout)
        self.assertEqual(values, {})

    def test_resolver_failure_stops_prepare_without_partial_outputs(self):
        result, values = self.resolve(source_sha='not-a-commit')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('assembled version', result.stderr)
        self.assertEqual(values, {})

    def test_all_builds_receive_the_same_resolved_runtime(self):
        self.assertEqual(WORKFLOW.count('bash scripts/version.sh'), 1)
        self.assertIn('runtime_version: ${{ steps.resolve.outputs.runtime_version }}', WORKFLOW)
        for job in ('build', 'build-flatpak', 'build-glibc228'):
            # Job sections have nested indentation, so split only at the next job.
            section = re.split(r'\n  [a-z][a-z0-9-]*:', WORKFLOW.split('\n  ' + job + ':', 1)[1], 1)[0]
            self.assertIn('XT_VERSION: ${{ needs.prepare.outputs.runtime_version }}', section)
            if job != 'build-flatpak':  # Flatpak repackages the already verified binary.
                self.assertIn('test "$ACTUAL" = "xenterm ${XT_VERSION}"', section)
        verify = WORKFLOW.split('      - name: Verify binary version', 1)[1].split('\n      - ', 1)[0]
        self.assertNotIn("runner.os != 'Windows'", verify)
        self.assertNotIn('EXPECTED=', verify)

    def test_runtime_check_rejects_semver_and_stale_runtime(self):
        section = WORKFLOW.split('      - name: Verify binary version', 1)[1].split('\n      #', 1)[0]
        check = section.split('        run: |\n', 1)[1]
        check = '\n'.join(line[10:] for line in check.splitlines())
        check = check.replace('${{ matrix.target }}', 'test-target').replace('${{ matrix.bin }}', 'xenterm')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / 'target/test-target/release/xenterm'
            binary.parent.mkdir(parents=True)
            for actual, accepted in [('xt-v' + CARGO_VERSION, True),
                                     (CARGO_VERSION, False), ('xt-v9.9.9', False)]:
                with self.subTest(actual=actual):
                    binary.write_text('#!/bin/sh\nprintf "%s\\n" "xenterm ' + actual + '"\n')
                    binary.chmod(0o755)
                    result = subprocess.run(['bash', '-eu', '-c', check], cwd=root,
                                            env=os.environ | {'XT_VERSION': 'xt-v' + CARGO_VERSION},
                                            capture_output=True, text=True)
                    self.assertEqual(result.returncode == 0, accepted, result.stderr)

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
