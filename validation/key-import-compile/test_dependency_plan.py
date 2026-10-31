import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock
import dependency_plan as p


class DependencyPlan(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.source = self.root / 'source'
        self.source.mkdir()
        (self.source / 'Cargo.toml').write_text('[package]\nname="xenterm"\nversion="0.1.0"\n')
        self.lock = ('[[package]]\nname="xenterm"\nversion="0.1.0"\n\n'
                     '[[package]]\nname="synthetic"\nversion="1.0.0"\nsource="' +
                     p.REGISTRY + '"\nchecksum="' + 'a' * 64 + '"\n')
        (self.source / 'Cargo.lock').write_text(self.lock)
        self.bin = self.root / 'rustup-home/toolchains/1.99.1-x86_64-pc-windows-msvc/bin'
        self.bin.mkdir(parents=True)
        for name in ('cargo.exe', 'rustc.exe'):
            (self.bin / name).write_bytes(b'synthetic never executed')

    def test_public_lock_fixed_real_tools_and_explicit_unready_bounds(self):
        gate = Mock()
        plan = p.public_locked_fetch_plan(self.root, gate)
        self.assertEqual(gate.call_count, 2)
        self.assertEqual(plan['args'], ['fetch', '--locked', '--target', p.TARGET])
        self.assertEqual(plan['executable'], str(self.bin / 'cargo.exe'))
        self.assertEqual(plan['required_rustc'], str(self.bin / 'rustc.exe'))
        self.assertEqual(plan['seconds'], 600)
        self.assertEqual(plan['outputBudget'], 1024 * 1024)
        self.assertEqual(plan['cleanupMilliseconds'], 5000)
        self.assertFalse(plan['executable_ready'])
        self.assertFalse(plan['compiler_policy_qualification_applicable'])
        self.assertFalse(plan['executes_product_tests'])
        self.assertEqual(set(plan['input_hashes']), {'Cargo.toml', 'Cargo.lock'})

    def test_unapproved_sources_or_missing_checksum_rejected(self):
        for lock in (self.lock.replace(p.REGISTRY, 'git+https://unapproved.example/repo'),
                     self.lock.replace('a' * 64, 'invalid'),
                     self.lock.replace('name="xenterm"', 'name="unexpected"')):
            with self.subTest(lock=lock):
                (self.source / 'Cargo.lock').write_text(lock)
                with self.assertRaises(ValueError):
                    p.public_locked_fetch_plan(self.root, Mock())

    def test_reparse_real_tools_rejected(self):
        for name in ('cargo.exe', 'rustc.exe'):
            with self.subTest(name=name):
                binary = self.bin / name
                binary.unlink()
                binary.symlink_to(self.source / 'Cargo.toml')
                with self.assertRaisesRegex(ValueError, 'reparse point'):
                    p.public_locked_fetch_plan(self.root, Mock())
                binary.unlink()
                binary.write_bytes(b'synthetic never executed')

    def test_changed_lock_or_manifest_rejected(self):
        for name in ('Cargo.lock', 'Cargo.toml'):
            with self.subTest(name=name):
                target = self.source / name
                original = target.read_text()
                calls = []
                def mutate():
                    calls.append(True)
                    if len(calls) == 2:
                        target.write_text(original + '\n')
                with self.assertRaisesRegex(ValueError, 'dependency inputs changed'):
                    p.public_locked_fetch_plan(self.root, mutate)
                target.write_text(original)

    def test_changed_tool_rejected(self):
        calls = []
        def mutate():
            calls.append(True)
            if len(calls) == 2:
                (self.bin / 'cargo.exe').write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError, 'dependency tools changed'):
            p.public_locked_fetch_plan(self.root, mutate)

    def test_oversized_manifest_rejected(self):
        (self.source / 'Cargo.toml').write_bytes(b' ' * 262145)
        with self.assertRaisesRegex(ValueError, 'manifest bound'):
            p.public_locked_fetch_plan(self.root, Mock())

    def test_source_identity_failure_propagates(self):
        with self.assertRaisesRegex(ValueError, 'source denied'):
            p.public_locked_fetch_plan(self.root, Mock(side_effect=ValueError('source denied')))


if __name__ == '__main__':
    unittest.main()
