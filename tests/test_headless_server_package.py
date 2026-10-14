"""ABI validation rejects unsuitable server artifacts before publication."""
import importlib.util
from pathlib import Path
import unittest

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


if __name__ == '__main__':
    unittest.main()
