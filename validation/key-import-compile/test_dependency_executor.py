import unittest
from unittest.mock import Mock
import copy
import hashlib
import json
import tempfile
from pathlib import Path
import dependency_executor as executor
from dependency_plan import public_locked_fetch_plan, REGISTRY, FETCH_POLICY


class DependencyExecutor(unittest.TestCase):
    def test_readiness_never_accepts_claims(self):
        for plan in (None, {}, {'executable_ready': True, 'qualified': True}):
            status = executor.fetch_readiness(plan)
            self.assertFalse(status['ready'])
            self.assertFalse(status['network_attempted'])
            self.assertFalse(status['product_compile_attempted'])
            self.assertFalse(status['product_runtime_attempted'])
            self.assertEqual(status['missing'], list(executor.REQUIRED_QUALIFICATION))

    def test_compile_smoke_and_forged_fetch_evidence_cannot_launch(self):
        for policy in ('smoke-v1', 'cargo-compile-only-v2', 'cargo-fetch-only-v1'):
            with self.subTest(policy=policy):
                transport = Mock()
                with self.assertRaisesRegex(ValueError, 'dedicated owned policy not qualified'):
                    executor.execute_fetch({'commandPolicy': policy, 'executable_ready': True},
                                           transport, {'passed': True, 'policy': policy})
                transport.assert_not_called()

    def test_missing_request_or_evidence_cannot_launch(self):
        with self.assertRaisesRegex(ValueError, 'fetch execution unavailable'):
            executor.execute_fetch(None)


class FetchReceipt(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        source = self.root / 'source'; source.mkdir()
        (source / 'Cargo.toml').write_text('[package]\nname="xenterm"\nversion="0.1.0"\n')
        content = b'synthetic archive bytes never extracted'
        checksum = hashlib.sha256(content).hexdigest()
        (source / 'Cargo.lock').write_text('[[package]]\nname="xenterm"\nversion="0.1.0"\n'
            '[[package]]\nname="one"\nversion="1.0.0"\nsource="' + REGISTRY + '"\nchecksum="' + checksum + '"\n'
            '[[package]]\nname="two"\nversion="2.0.0"\nsource="' + REGISTRY + '"\nchecksum="' + 'b' * 64 + '"\n')
        self.bin = self.root / 'rustup-home/toolchains/1.99.1-x86_64-pc-windows-msvc/bin'
        self.bin.mkdir(parents=True)
        for name in ('cargo.exe', 'rustc.exe'):
            (self.bin / name).write_bytes(b'synthetic tool never executed')
        self.relative = 'cargo-home/registry/cache/index.crates.io-1949cf8c6b5b557f/one-1.0.0.crate'
        self.crate = self.root / self.relative
        self.crate.parent.mkdir(parents=True); self.crate.write_bytes(content)
        for stream in ('stdout', 'stderr'):
            (self.root / ('fetch.' + stream + '.log')).write_bytes(b'')
        self.plan = public_locked_fetch_plan(self.root, Mock())
        self.receipt = dict(schema=1, state='returned', succeeded=True, control_sha='c' * 40, run_id='123', commandPolicy=FETCH_POLICY,
            completionMode='natural-tree', seconds=600, outputBudget=1024 * 1024, outputBytes=0,
            treeEmpty=True, safeToStart=True, boundaryRootSignaled=True, boundaryRootExitCodeKnown=True,
            timedOut=False, outputExceeded=False, captureFailed=False, forcedOwnedCleanup=False,
            reusable=False, productCompileAttempted=False, productRuntimeAttempted=False,
            exitCode=0, boundaryRootExitCode=0, boundaryActiveProcessCount=0, cleanupMilliseconds=10,
            input_hashes=self.plan['input_hashes'], tool_hashes=self.plan['tool_hashes'],
            cache_scope='verified-subset', all_dependencies_available=False,
            cache_files=[dict(relative=self.relative, sha256=checksum, bytes=len(content))])

    def validate(self, receipt=None):
        return executor.validate_fetch_receipt(self.receipt if receipt is None else receipt,
                                              self.plan, self.root, 'c' * 40, '123')

    def test_actual_subset_is_attested_without_completeness_claim(self):
        result = self.validate()
        self.assertTrue(result['validated'])
        self.assertEqual(result['verified_crate_count'], 1)
        self.assertEqual(result['locked_registry_count'], 2)
        self.assertFalse(result['all_dependencies_available'])
        self.assertTrue(result['offline_compile_required'])

    def test_passed_boolean_and_missing_cache_are_not_evidence(self):
        for receipt in ({'passed': True, 'cache_verified': True},
                        dict(self.receipt, cache_files=[]), dict(self.receipt, all_dependencies_available=True)):
            with self.assertRaises(ValueError):
                self.validate(receipt)

    def test_wrong_run_source_policy_and_unknown_lifecycle_rejected(self):
        for key, value in (('state', 'unknown'), ('succeeded', False), ('run_id', '124'), ('control_sha', 'd' * 40), ('commandPolicy', 'smoke-v1'),
                           ('treeEmpty', False), ('safeToStart', None), ('forcedOwnedCleanup', True),
                           ('exitCode', True), ('boundaryActiveProcessCount', 1), ('cleanupMilliseconds', 5001)):
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.validate(dict(self.receipt, **{key: value}))

    def test_changed_lock_manifest_or_tool_rejected(self):
        for path in (self.root / 'source/Cargo.lock', self.root / 'source/Cargo.toml', self.bin / 'rustc.exe'):
            original = path.read_bytes(); path.write_bytes(original + b'\n')
            with self.subTest(path=path.name), self.assertRaisesRegex(ValueError, 'inputs or tools changed'):
                self.validate()
            path.write_bytes(original)

    def test_wrong_crate_checksum_path_size_duplicate_and_reparse_rejected(self):
        for key, value in (('sha256', 'e' * 64), ('relative', '../outside.crate'), ('bytes', 1)):
            receipt = copy.deepcopy(self.receipt); receipt['cache_files'][0][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.validate(receipt)
        receipt = copy.deepcopy(self.receipt); receipt['cache_files'] *= 2
        with self.assertRaisesRegex(ValueError, 'duplicate crate'):
            self.validate(receipt)
        self.crate.unlink(); self.crate.symlink_to(self.bin / 'rustc.exe')
        with self.assertRaisesRegex(ValueError, 'reparse point'):
            self.validate()

    def test_changed_actual_crate_bytes_rejected(self):
        content = self.crate.read_bytes()
        self.crate.write_bytes(b'x' * len(content))
        with self.assertRaisesRegex(ValueError, 'crate checksum'):
            self.validate()

    def test_output_claim_must_match_actual_bounded_logs(self):
        (self.root / 'fetch.stdout.log').write_bytes(b'x')
        with self.assertRaisesRegex(ValueError, 'retained output bytes'):
            self.validate()

    def write_host(self, overrides=None):
        added = {'control_sha', 'run_id', 'input_hashes', 'tool_hashes', 'cache_scope', 'cache_files', 'all_dependencies_available'}
        host = {key: value for key, value in self.receipt.items() if key not in added}
        host.update(overrides or {})
        (self.root / 'fetch.receipt.json').write_text(json.dumps(host))
        (self.root / 'fetch-started.marker').write_bytes(b'\x01')

    def test_summary_reads_fixed_host_and_actual_cache(self):
        self.write_host()
        summary = executor.summarize_fetch_receipt(self.plan, self.root, 'c' * 40, 123)
        self.assertEqual(summary, self.receipt)
        self.assertFalse(summary['all_dependencies_available'])
        with self.assertRaisesRegex(ValueError, 'fetch execution unavailable'):
            executor.execute_fetch(self.plan, qualification=summary)

    def test_summary_rejects_passed_flag_and_unknown_host_state(self):
        for overrides in ({'passed': True}, {'state': 'unknown'}, {'safeToStart': False}, {'outputBytes': 1}):
            self.write_host(overrides)
            with self.subTest(overrides=overrides), self.assertRaises(ValueError):
                executor.summarize_fetch_receipt(self.plan, self.root, 'c' * 40, 123)

    def test_summary_rejects_changed_tools_and_wrong_crate(self):
        self.write_host()
        original = (self.bin / 'cargo.exe').read_bytes()
        (self.bin / 'cargo.exe').write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError, 'summary inputs changed'):
            executor.summarize_fetch_receipt(self.plan, self.root, 'c' * 40, 123)
        (self.bin / 'cargo.exe').write_bytes(original)
        self.crate.write_bytes(b'wrong actual crate')
        with self.assertRaisesRegex(ValueError, 'crate checksum'):
            executor.summarize_fetch_receipt(self.plan, self.root, 'c' * 40, 123)

    def test_summary_rejects_missing_owner_or_missing_all_cache(self):
        self.write_host()
        (self.root / 'fetch-started.marker').write_bytes(b'')
        with self.assertRaisesRegex(ValueError, 'owner marker absent'):
            executor.summarize_fetch_receipt(self.plan, self.root, 'c' * 40, 123)
        (self.root / 'fetch-started.marker').write_bytes(b'\x01')
        self.crate.unlink()
        with self.assertRaisesRegex(ValueError, 'cache evidence absent'):
            executor.summarize_fetch_receipt(self.plan, self.root, 'c' * 40, 123)


if __name__ == '__main__':
    unittest.main()
