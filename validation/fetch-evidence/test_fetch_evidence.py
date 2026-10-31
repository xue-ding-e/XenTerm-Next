"""Pinned historical evidence tests. No native process, network or acquisition."""
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
from unittest.mock import patch
HERE = Path(__file__).parent
sys.path.insert(0, str(HERE.parent / 'key-import-compile'))
import fetch_evidence as e

FIXTURE = HERE / 'fixtures/fetch-native-af465570.json'


class FetchEvidence(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def test_real_pinned_evidence_only_attests_synthetic_transport(self):
        result = e.verify_transport_evidence(FIXTURE)
        self.assertEqual(result['case_count'], 8)
        self.assertTrue(result['transport_qualified'])
        self.assertTrue(result['host_compiled'])
        for key in ('host_executed', 'acquisition_ready', 'product_compile_attempted', 'full_ten_minute_elapsed_validation'):
            self.assertIs(result[key], False)

    def test_each_pin_tamper_rejects(self):
        for name in ('QUALIFIED_SHA', 'QUALIFIED_RUN', 'TRANSPORT_SHA', 'METADATA_SHA', 'SOURCE_MANIFEST_SHA', 'HOST_SHA'):
            with self.subTest(name=name), patch.object(e, name, '0' * 64), self.assertRaises(ValueError):
                e.verify_transport_evidence(FIXTURE)

    def test_changed_artifact_bytes_reject_before_json(self):
        path = self.root / 'metadata.json'
        path.write_bytes(FIXTURE.read_bytes() + b' ')
        with self.assertRaisesRegex(ValueError, 'metadata pin mismatch'):
            e.verify_transport_evidence(path)

    def test_manifest_and_every_input_tamper_rejects(self):
        root = self.root / 'native'; shutil.copytree(HERE.parent / 'fetch-native', root)
        for name in ['source-hashes.json', *sorted(e.INPUTS)]:
            path = root / name; original = path.read_bytes(); path.write_bytes(original + b' ')
            with self.subTest(name=name), self.assertRaises(ValueError):
                e.verify_transport_evidence(FIXTURE, root)
            path.write_bytes(original)

    def test_symlink_evidence_is_rejected(self):
        path = self.root / 'linked.json'; path.symlink_to(FIXTURE)
        with self.assertRaisesRegex(ValueError, 'reparse point'):
            e.verify_transport_evidence(path)

    def _mutated(self, mutate):
        data = json.loads(FIXTURE.read_bytes()); mutate(data)
        raw = json.dumps(data).encode(); path = self.root / 'mutated.json'; path.write_bytes(raw)
        # Bypass ONLY raw digest in this negative test to exercise semantic gates.
        with patch.object(e, 'METADATA_SHA', hashlib.sha256(raw).hexdigest()), self.assertRaises(ValueError):
            e.verify_transport_evidence(path)

    def test_provenance_scope_and_identity_negatives(self):
        mutations = [lambda m: m.update(controlSha='0' * 40), lambda m: m.update(runId='other'),
            lambda m: m.update(initialIdentity=False), lambda m: m.update(finalIdentity=False),
            lambda m: m.update(passed=False), lambda m: m.update(hostExecuted=True),
            lambda m: m.update(dependencyFetchAttempted=True), lambda m: m.update(productCompileAttempted=True),
            lambda m: m.update(fullTenMinuteElapsedValidation=True), lambda m: m.update(sourceManifestSha256='wrong'),
            lambda m: m.update(compiledHostFiles={})]
        for i, mutate in enumerate(mutations):
            with self.subTest(i=i): self._mutated(mutate)

    def test_case_cleanup_output_and_negative_execution(self):
        def change(index, **values):
            return lambda m: m['suites']['fetch8']['receipt']['cases'][index].update(values)
        for i, mutate in enumerate((change(0, treeEmpty=False), change(0, cleanupMilliseconds=5001),
                change(0, actualExecutions=True), change(1, retainedBytes=1048577),
                change(1, stdoutBytes=0), change(2, injectedMilliseconds=600000),
                change(2, timedOut=False), change(3, actualExecutions=1), change(7, expectedRejection=False))):
            with self.subTest(i=i): self._mutated(mutate)

    def test_duplicate_json_key_rejected_even_if_digest_bypassed(self):
        raw = FIXTURE.read_bytes().replace(b'"finalIdentity": true', b'"finalIdentity": true, "finalIdentity": true', 1)
        path = self.root / 'duplicate.json'; path.write_bytes(raw)
        with patch.object(e, 'METADATA_SHA', hashlib.sha256(raw).hexdigest()), self.assertRaisesRegex(ValueError, 'duplicate'):
            e.verify_transport_evidence(path)

    def test_verified_evidence_exposes_no_execution_api(self):
        qualified = e.verify_transport_evidence(FIXTURE)
        self.assertIs(qualified['acquisition_ready'], False)
        self.assertIs(qualified['host_executed'], False)
        self.assertFalse(hasattr(e, 'execute'))
        self.assertFalse(hasattr(e, 'execute_fetch'))


if __name__ == '__main__': unittest.main()
