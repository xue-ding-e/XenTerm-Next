"""Pure evidence checks only: no native child, network, Cargo or build."""
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
from unittest.mock import patch
HERE = Path(__file__).parent
sys.path[:0] = [str(HERE.parent / 'key-import-compile'), str(HERE.parent / 'fetch-evidence')]
import host_entry_evidence as e
FIXTURE = HERE / 'fixtures/host-entry-0ac5255.json'


class HostEvidence(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def test_actual_host_qualification_never_unlocks_fetch(self):
        result = e.verify_host_entry_evidence(FIXTURE)
        self.assertTrue(result['host_entry_qualified']); self.assertTrue(result['host_executed'])
        self.assertEqual(result['case_count'], 13)
        for key in ('acquisition_ready', 'real_cargo_executed', 'product_compile_attempted',
                    'network_containment_qualified', 'continuous_disk_limit_qualified'):
            self.assertIs(result[key], False)
        self.assertFalse(hasattr(e, 'execute_fetch')); self.assertFalse(hasattr(e, 'execute'))

    def test_each_identity_pin_rejects(self):
        for name in ('QUALIFIED_SHA', 'QUALIFIED_RUN', 'METADATA_SHA', 'SOURCE_MANIFEST_SHA', 'HOST_SHA', 'TRANSPORT_SHA'):
            with self.subTest(pin=name), patch.object(e, name, '0' * 64), self.assertRaises(ValueError):
                e.verify_host_entry_evidence(FIXTURE)

    def test_old_transport_cannot_substitute_for_executed_host(self):
        old = HERE.parent / 'fetch-evidence/fixtures/fetch-native-af465570.json'
        with self.assertRaisesRegex(ValueError, 'metadata pin'):
            e.verify_host_entry_evidence(old)
        with patch.object(e, 'METADATA_SHA', hashlib.sha256(old.read_bytes()).hexdigest()), self.assertRaises(ValueError):
            e.verify_host_entry_evidence(old)

    def test_every_source_input_and_manifest_bound(self):
        root = self.root / 'native'; shutil.copytree(HERE.parent / 'fetch-host-entry-native', root)
        for name in ['source-hashes.json', *sorted(e.INPUTS)]:
            path = root / name; original = path.read_bytes(); path.write_bytes(original + b' ')
            with self.subTest(input=name), self.assertRaises(ValueError): e.verify_host_entry_evidence(FIXTURE, root)
            path.write_bytes(original)

    def _mutated(self, mutate):
        data = json.loads(FIXTURE.read_bytes()); mutate(data)
        raw = json.dumps(data).encode(); path = self.root / 'mutated.json'; path.write_bytes(raw)
        # Test-only raw-pin bypass exercises semantic gates; production has no bypass parameter.
        with patch.object(e, 'METADATA_SHA', hashlib.sha256(raw).hexdigest()), self.assertRaises(ValueError):
            e.verify_host_entry_evidence(path)

    def test_wrong_run_or_unexecuted_host_and_scope_reject(self):
        changes = dict(runId='41185823982', controlSha='wrong', hostEntryExecuted=False,
                       hostOsProcessInvocations=0, initialIdentity=False, finalIdentity=False,
                       passed=False, sourceManifestSha256='wrong', stage='native-fetch8',
                       dependencyFetchAttempted=True, realCargoExecuted=True, productCompileAttempted=True,
                       fixtureNetworkAttempted=True, outerJobObjectQualification=True)
        for key, value in changes.items():
            with self.subTest(field=key): self._mutated(lambda m: m.update({key: value}))

    def test_directory_and_copy_proof_rejects(self):
        for mutate in (lambda m: m['nugetIsolation'].update(migrationMarkerAtExpectedRoot=False),
                       lambda m: m['nugetIsolation'].update(markerBytes=1),
                       lambda m: m.update(directoryProbeChecks=27),
                       lambda m: m['copyChecks'][2].update(extraCount=1),
                       lambda m: m['copyChecks'][1].update(changedIndices=[0]),
                       lambda m: m['copyChecks'][0].update(scanComplete=False)):
            self._mutated(mutate)

    def test_build_runtime_and_lifecycle_rejects(self):
        for mutate in (lambda m: m['buildOutcomes'][0].update(exitCode=1),
                       lambda m: m['buildOutcomes'][1].update(timedOut=True),
                       lambda m: m.update(runtimeFiles={}),
                       lambda m: m['cases'][0]['hostReceipt'].update(treeEmpty=False),
                       lambda m: m['cases'][11]['hostReceipt'].update(boundaryRootExitCode=0),
                       lambda m: m['cases'][0]['hostReceipt'].update(cleanupMilliseconds=5001),
                       lambda m: m['cases'][0]['hostReceipt'].update(outputBytes=1048577),
                       lambda m: m['cases'][0]['hostReceipt'].update(boundaryActiveProcessCount=True),
                       lambda m: m['cases'][2].update(syntheticCargoExecutions=1)):
            self._mutated(mutate)

    def test_symlink_or_raw_mutation_rejected(self):
        linked = self.root / 'linked.json'; linked.symlink_to(FIXTURE)
        with self.assertRaises(ValueError): e.verify_host_entry_evidence(linked)
        changed = self.root / 'changed.json'; changed.write_bytes(FIXTURE.read_bytes() + b' ')
        with self.assertRaises(ValueError): e.verify_host_entry_evidence(changed)

    def test_duplicate_json_rejected_even_when_digest_bypassed(self):
        raw = FIXTURE.read_bytes().replace(b'"passed": true', b'"passed": true, "passed": true', 1)
        self.assertNotEqual(raw, FIXTURE.read_bytes())
        path = self.root / 'duplicate.json'; path.write_bytes(raw)
        with patch.object(e, 'METADATA_SHA', hashlib.sha256(raw).hexdigest()), self.assertRaisesRegex(ValueError, 'duplicate'):
            e.verify_host_entry_evidence(path)


if __name__ == '__main__': unittest.main()
