"""ABI validation rejects unsuitable server artifacts before publication."""
import importlib.util
import contextlib
import io
import json
import os
import tempfile
from unittest.mock import patch
from pathlib import Path
import unittest
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))

spec = importlib.util.spec_from_file_location('package', Path(__file__).resolve().parents[1] / 'scripts/headless_server_package.py')
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class AbiValidation(unittest.TestCase):
    def check(self, dynamic='(NEEDED) Shared library: [libc.so.6]', versions='GLIBC_2.17 GLIBC_2.35', header='ELF64 Advanced Micro Devices X86-64'):
        return package.validate_elf(dynamic, versions, header)

    def test_workflow_uses_real_cli_and_fixed_source(self):
        root = Path(__file__).resolve().parents[1]
        workflow = (root / '.github/workflows/headless-server.yml').read_text()
        cli = (root / 'tests/access_policy_e2e.py').read_text()
        self.assertIn("parser.add_argument('--exe', required=True", cli)
        self.assertIn("access_policy_e2e.py --exe target/release/xenterm", workflow)
        self.assertIn('cargo build --locked --release --no-default-features --features headless', workflow)
        self.assertIn('runs-on: ubuntu-22.04', workflow)
        self.assertIn('ref: ${{ env.SOURCE_SHA }}', workflow)
        self.assertIn('contents: read', workflow)
        self.assertEqual(workflow.count('contents: write'), 1)
        self.assertIn('if: inputs.publish', workflow)
        self.assertNotIn('secrets.', workflow)

    def test_allowed(self):
        self.assertEqual(self.check(), (['libc.so.6'], '2.35'))

    def test_gui_rejected(self):
        with self.assertRaises(ValueError):
            self.check(dynamic='(NEEDED) Shared library: [libgtk-3.so.0]')

    def test_new_glibc_rejected(self):
        with self.assertRaises(ValueError):
            self.check(versions='GLIBC_2.39')

    def test_nonnumeric_glibc_abi_rejected(self):
        with self.assertRaises(ValueError):
            self.check(versions='GLIBC_2.34 GLIBC_ABI_DT_RELR')

    def test_missing_versions_rejected(self):
        with self.assertRaises(ValueError):
            self.check(versions='')

    def test_missing_dependencies_rejected(self):
        with self.assertRaises(ValueError):
            self.check(dynamic='')

    def test_wrong_arch_rejected(self):
        with self.assertRaises(ValueError):
            self.check(header='ELF64 AArch64')


class PackageRuntime(unittest.TestCase):
    def test_old_and_new_product_sources_round_trip(self):
        from headless_server_verify import verify
        source, tooling = 'a' * 40, 'b' * 40
        for modern in (False, True):
            with self.subTest(modern=modern), tempfile.TemporaryDirectory() as directory:
                root = Path(directory) / 'source'; root.mkdir()
                (root / 'Cargo.toml').write_text('[package]\nversion = "0.7.3"\n')
                (root / 'target/release').mkdir(parents=True)
                binary = root / 'target/release/xenterm'; binary.write_bytes(b'synthetic binary')
                binary.chmod(0o755)
                for name in ('LICENSE', 'THIRD_PARTY_NOTICES.md'):
                    (root / name).write_text('synthetic notice')
                (root / 'licenses').mkdir(); (root / 'licenses/dependency').write_text('MIT')
                runtime = 'xt-beta-aaaaaaa-T1792060000' if modern else '0.7.3'
                env = dict(SOURCE_SHA=source, GITHUB_SHA=tooling, CI_RUN_ID='12', GITHUB_RUN_ID='34', XT_VERSION='')
                if modern:
                    (root / 'src').mkdir(); (root / 'src/version.rs').write_text('// Runtime fixture')
                    (root / 'scripts').mkdir(); (root / 'scripts/version.sh').write_text('# Resolver fixture')
                    env['XT_VERSION'] = runtime
                def output(args, **kwargs):
                    if args[0] == 'git': return source
                    if args[0] == 'readelf':
                        return {'-d': '(NEEDED) Shared library: [libc.so.6]',
                                '--version-info': 'GLIBC_2.17 GLIBC_2.35',
                                '-h': 'ELF64 Advanced Micro Devices X86-64'}[args[1]]
                    if args[0] == 'ldd': return 'libc.so.6 => /lib/libc.so.6'
                    if args[0] == 'rustc': return 'rustc synthetic'
                    if args[0] == str(binary): return 'xenterm ' + runtime
                    raise AssertionError(args)
                destination = Path(directory) / 'dist'
                with patch.dict(os.environ, env, clear=True), \
                     patch.object(package.subprocess, 'check_output', side_effect=output), \
                     contextlib.redirect_stdout(io.StringIO()):
                    package.package(root, destination)
                    manifest = verify(destination, source, tooling, '12', '34')
                self.assertEqual(manifest['source_sha'], source)
                self.assertEqual(manifest['tooling_sha'], tooling)
                self.assertEqual(manifest['runtime_version'], runtime)
                self.assertEqual(manifest['package_version'], '0.7.3')
                self.assertEqual(manifest['version_scheme'], 'xt-v1' if modern else 'cargo-legacy')


if __name__ == '__main__':
    unittest.main()
