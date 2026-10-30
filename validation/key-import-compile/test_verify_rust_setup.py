"""Pure fixed-toolchain contracts: all subprocess results are synthetic."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock

import verify_rust_setup as verifier


class FixedToolchain(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.bin = self.root / 'rustup-home/toolchains' / verifier.TOOLCHAIN / 'bin'
        self.proxy_bin = self.root / 'cargo-home/bin'
        self.bin.mkdir(parents=True)
        self.proxy_bin.mkdir(parents=True)
        for path in (self.bin / 'rustc.exe', self.bin / 'cargo.exe', self.proxy_bin / 'rustup.exe'):
            path.write_bytes(b'synthetic; never executed')
        self.receipt = dict(completionMode='natural-tree', treeEmpty=True, safeToStart=True,
                            exitCode=0, timedOut=False, outputExceeded=False,
                            captureFailed=False, forcedOwnedCleanup=False)

    def runner(self):
        return Mock(side_effect=[
            (self.receipt, 'rustc 1.99.1 (synthetic)\nrelease: 1.99.1\nhost: ' + verifier.TARGET),
            (self.receipt, 'cargo 1.99.1 (synthetic)'),
            (self.receipt, str(self.bin / 'rustc.exe')),
        ])

    def test_queries_only_plain_fixed_binaries_despite_reparse_proxies(self):
        for name in ('cargo.exe', 'rustc.exe'):
            (self.proxy_bin / name).symlink_to(self.proxy_bin / 'rustup.exe')
        runner = self.runner()
        result = verifier.verify(self.root, runner)
        self.assertEqual([call.args[0] for call in runner.call_args_list], [
            [str(self.bin / 'rustc.exe'), '-vV'],
            [str(self.bin / 'cargo.exe'), '-V'],
            [str(self.proxy_bin / 'rustup.exe'), 'which', '--toolchain', '1.99.1', 'rustc'],
        ])
        self.assertTrue(all(call.args[2] == 20 for call in runner.call_args_list))
        self.assertEqual(len(result['files']), 3)
        self.assertNotIn('cargo-home/bin/cargo.exe', result['files'])
        self.assertNotIn('cargo-home/bin/rustc.exe', result['files'])
        self.assertFalse(result['product_compile_attempted'])
        self.assertFalse(result['product_runtime_attempted'])

    def test_fixed_toolchain_reparse_substitution_is_rejected_before_launch(self):
        binary = self.bin / 'rustc.exe'
        binary.unlink()
        binary.symlink_to(self.proxy_bin / 'rustup.exe')
        runner = self.runner()
        with self.assertRaisesRegex(ValueError, 'reparse point'):
            verifier.verify(self.root, runner)
        runner.assert_not_called()

    def test_rustup_reparse_substitution_is_rejected_before_launch(self):
        rustup = self.proxy_bin / 'rustup.exe'
        rustup.unlink()
        rustup.symlink_to(self.bin / 'cargo.exe')
        runner = self.runner()
        with self.assertRaisesRegex(ValueError, 'reparse point'):
            verifier.verify(self.root, runner)
        runner.assert_not_called()

    def test_rustup_resolution_cannot_point_back_to_proxy(self):
        (self.proxy_bin / 'rustc.exe').write_bytes(b'plain but wrong target')
        runner = self.runner()
        responses = list(runner.side_effect)
        responses[2] = (self.receipt, str(self.proxy_bin / 'rustc.exe'))
        runner.side_effect = responses
        with self.assertRaisesRegex(ValueError, 'toolchain resolved outside synthetic root'):
            verifier.verify(self.root, runner)

    def test_changed_binary_fails_post_query_hash(self):
        runner = self.runner()
        responses = list(runner.side_effect)
        def mutate(command, label, seconds):
            if label == 'resolved-rustc':
                (self.bin / 'cargo.exe').write_bytes(b'changed')
            return responses.pop(0)
        runner.side_effect = mutate
        with self.assertRaisesRegex(ValueError, 'tool changed during verification'):
            verifier.verify(self.root, runner)

    def test_wrong_version_and_cleanup_failure_are_rejected(self):
        for replacement in [(self.receipt, 'rustc 1.99.2 (synthetic)'),
                            (dict(self.receipt, forcedOwnedCleanup=True), 'version')]:
            with self.subTest(replacement=replacement):
                runner = self.runner()
                responses = list(runner.side_effect)
                responses[0] = replacement
                runner.side_effect = responses
                with self.assertRaises(ValueError):
                    verifier.verify(self.root, runner)


if __name__ == '__main__':
    unittest.main()
