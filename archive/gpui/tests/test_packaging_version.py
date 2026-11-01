"""Offline runtime/package version gates, using the actual upstream resolver."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
import packaging_version as version

SHA = 'a' * 40
RUNTIME = 'xt-beta-aaaaaaa-T1792060000'


class PackagingVersion(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        (self.root / 'Cargo.toml').write_text('[package]\nversion = "0.7.3"\n', encoding='utf-8')

    def modern(self):
        (self.root / 'src').mkdir()
        (self.root / 'src/version.rs').write_text('// Runtime version module fixture\n')
        (self.root / 'scripts').mkdir()
        shutil.copyfile(ROOT / 'scripts/version.sh', self.root / 'scripts/version.sh')

    def test_product_sha_checked_instead_of_tooling(self):
        with patch.object(version.subprocess, 'check_output', return_value=SHA) as command:
            version.verify_source(self.root, SHA)
            self.assertEqual(command.call_args.args[0], ['git', '-C', str(self.root), 'rev-parse', 'HEAD'])
            with self.assertRaisesRegex(ValueError, 'source SHA mismatch'):
                version.verify_source(self.root, 'b' * 40)
        for bad in ('a' * 7, 'A' * 40, ''):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                version.verify_source(self.root, bad)

    def test_new_source_requires_explicit_prepared_version(self):
        self.modern()
        with patch.object(version, 'verify_source'):
            with self.assertRaisesRegex(ValueError, 'requires prepared XT_VERSION'):
                version.expected_version(self.root, SHA, {})
            details = version.verify_binary_version(self.root, SHA, 'xenterm ' + RUNTIME, {'XT_VERSION': RUNTIME})
            self.assertEqual(details['package_version'], '0.7.3')
            self.assertEqual(details['version_timestamp'], 1792060000)
            for actual in ('xenterm 0.7.3', 'xenterm ' + RUNTIME + '-dirty', 'xenterm xt-beta-aaaaaaa-T1792060001'):
                with self.subTest(actual=actual), self.assertRaises(ValueError):
                    version.verify_binary_version(self.root, SHA, actual, {'XT_VERSION': RUNTIME})

    def test_wrong_source_or_illegal_runtime_never_falls_back(self):
        self.modern()
        with patch.object(version, 'verify_source'):
            for runtime in ('xt-beta-bbbbbbb-T1792060000', 'xt-beta-aaaaaaa-T123',
                            '0.7.3', 'xt-dev-aaaaaaa-T1792060000', 'xt-v0.7.4'):
                with self.subTest(runtime=runtime), self.assertRaises(ValueError):
                    version.expected_version(self.root, SHA, {'XT_VERSION': runtime})

    def test_legacy_source_keeps_strict_cargo_output(self):
        with patch.object(version, 'verify_source'):
            details = version.verify_binary_version(self.root, SHA, 'xenterm 0.7.3', {})
            self.assertEqual(details['version_scheme'], 'cargo-legacy')
            with self.assertRaises(ValueError):
                version.verify_binary_version(self.root, SHA, 'xenterm ' + RUNTIME, {})
            with self.assertRaisesRegex(ValueError, 'cannot consume XT_VERSION'):
                version.expected_version(self.root, SHA, {'XT_VERSION': RUNTIME})
            self.assertEqual(version.resolve_version(self.root, SHA), details)

    def test_incomplete_version_implementation_is_not_legacy(self):
        (self.root / 'src').mkdir()
        (self.root / 'src/version.rs').write_text('// Missing resolver\n')
        with patch.object(version, 'verify_source'), self.assertRaisesRegex(ValueError, 'Incomplete'):
            version.expected_version(self.root, SHA, {})

    def test_resolver_uses_product_hash_and_discards_inherited_context(self):
        self.modern()
        contaminated = os.environ | dict(XT_VERSION='xt-v9.9.9', XT_CHANNEL='dev', VERSION='bad',
                                        GITHUB_SHA='b' * 40, GITHUB_REF_TYPE='tag',
                                        GITHUB_REF_NAME='headless-' + SHA, GITHUB_REF='refs/tags/headless-' + SHA)
        with patch.object(version, 'verify_source'):
            details = version.resolve_version(self.root, SHA, environ=contaminated)
        self.assertRegex(details['runtime_version'], r'^xt-beta-aaaaaaa-T[0-9]{10}$')
        self.assertFalse((self.root / 'version.json').exists())

    def test_stable_tag_uses_resolver_and_numeric_package_version(self):
        self.modern()
        with patch.object(version, 'verify_source'):
            details = version.resolve_version(self.root, SHA, stable_tag='v0.7.3')
            self.assertEqual(details['runtime_version'], 'xt-v0.7.3')
            self.assertEqual(details['package_version'], '0.7.3')
            self.assertIsNone(details['version_timestamp'])
            for tag in ('v0.7.4', 'headless-' + SHA, 'beta'):
                with self.subTest(tag=tag), self.assertRaises(ValueError):
                    version.resolve_version(self.root, SHA, stable_tag=tag)

    def test_prepared_metadata_rejects_another_build_timestamp(self):
        self.modern()
        wrong = version.version_details('xt-beta-aaaaaaa-T1792060001', '0.7.3', 'xt-v1', SHA)
        with patch.object(version, 'verify_source'), self.assertRaisesRegex(ValueError, 'Prepared version'):
            version.expected_version(self.root, SHA, {'XT_VERSION': RUNTIME, 'XENTERM_VERSION_METADATA': json.dumps(wrong)})

    def test_empty_or_malformed_prepared_metadata_fails_closed(self):
        with self.assertRaisesRegex(ValueError, 'Missing prepared'):
            version.prepared_metadata({}, required=True)
        details = version.version_details(RUNTIME, '0.7.3', 'xt-v1', SHA)
        manifest = details | {'binary_version': 'xenterm ' + RUNTIME}
        for raw in ('', 'null', '[]', '{}', 'not-json'):
            with self.subTest(raw=raw), patch.dict(os.environ, {'XENTERM_VERSION_METADATA': raw}, clear=True):
                with self.assertRaisesRegex(ValueError, 'prepared version'):
                    version.validate_manifest_version(manifest, SHA)

    def test_admission_entrypoints_require_prepared_metadata(self):
        for script in ('next_beta.py', 'headless_server_verify.py'):
            with self.subTest(script=script):
                result = subprocess.run([sys.executable, str(ROOT / 'scripts' / script)],
                                        env={}, text=True, capture_output=True, timeout=30)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('Missing prepared version metadata', result.stderr)

    def test_manifest_is_bound_to_the_prepared_version(self):
        details = version.version_details(RUNTIME, '0.7.3', 'xt-v1', SHA)
        manifest = details | {'binary_version': 'xenterm ' + RUNTIME}
        self.assertEqual(version.validate_manifest_version(manifest, SHA, details), details)
        with self.assertRaises(ValueError):
            version.validate_manifest_version(manifest, SHA, details | {'package_version': '0.7.4'})
        with self.assertRaises(ValueError):
            version.validate_manifest_version(manifest | {'version_timestamp': None}, SHA, details)

    def test_workflows_propagate_prepared_values(self):
        workflow = (ROOT / '.github/workflows/next-beta-build.yml').read_text()
        self.assertEqual(workflow.count('run: python3 scripts/packaging_version.py'), 1)
        self.assertIn('needs: [prepare, build]', workflow)
        self.assertIn('XT_VERSION: ${{ needs.prepare.outputs.xt_version }}', workflow)
        self.assertIn('XENTERM_VERSION_METADATA: ${{ needs.prepare.outputs.version_metadata }}', workflow)
        publish = (ROOT / '.github/workflows/next-beta.yml').read_text()
        self.assertIn('XENTERM_VERSION_METADATA: ${{ needs.build.outputs.version_metadata }}', publish)
        headless = (ROOT / '.github/workflows/headless-server.yml').read_text()
        self.assertIn('tooling/scripts/packaging_version.py source --source-sha "$SOURCE_SHA"', headless)
        self.assertLess(headless.index('id: version'), headless.index('cargo build'))
        self.assertIn('--github-env "$GITHUB_ENV"', headless)
        self.assertIn('XENTERM_VERSION_METADATA: ${{ needs.build.outputs.version_metadata }}', headless)


if __name__ == '__main__':
    unittest.main()
