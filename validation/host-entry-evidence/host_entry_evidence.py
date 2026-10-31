"""Read-only fixed Host-entry qualification; never authorizes dependency acquisition.

Distinct from the older eight-case transport evidence: this artifact actually
invoked Host thirteen times, but its only children were synthetic cargo fixtures.
Network containment and continuous disk limits remain unqualified.
"""
import re
from pathlib import Path
from compile_adapter import plain, require
from fetch_evidence import _bytes, _json, _hash

QUALIFIED_SHA = '0ac5255e11c889c2095a20deb2e06e56c1eeee58'
QUALIFIED_RUN = '41189403380'
METADATA_SHA = 'ca079826de269d66e494eab7834d94bbb58ad56cec2da7a03b207b36264ccb0d'
SOURCE_MANIFEST_SHA = 'a0fa8cca0b44d80a296480789ee9af9458c5d0183f3be4c1d3b9392ff05dffa0'
HOST_SHA = '0f94cd746e8a04d7bb5e794683a30b7512d406319eccb3e3c4e6fb3d79e29ae3'
TRANSPORT_SHA = 'd6d9d3b5c31a2f697eb435cdbf99f0fc968e2af912b069923bc6f8c7497331b3'
INPUTS = frozenset(('.github/workflows/ci.yml', 'NuGet.Config', 'global.json',
    'run-host-entry.ps1', 'expected-cases.json', 'test_host_entry_preparation.py',
    'Fixture/Program.cs', 'Fixture/Fixture.csproj', 'Harness/Program.cs', 'Harness/Harness.csproj',
    'Host/FetchHost.csproj', 'Host/Program.cs', 'Host/fetch-owned-process.cs', 'Host/NuGet.Config', 'Host/global.json'))
CASES = ('fixed-success', 'consumed-root', 'marker-only-consumed-root', 'compile-argv',
    'wrong-policy', 'wrong-deadline', 'extra-environment', 'case-colliding-environment',
    'wrong-tool-path', 'duplicate-json', 'reparse-executable', 'nonzero-child', 'nonzero-consumed-root')


def _integer(value, low, high):
    return type(value) is int and low <= value <= high


def verify_host_entry_evidence(metadata_path, native_root=None):
    """Verify downloaded bytes plus all fifteen pinned inputs without launching tools."""
    root = plain(Path(native_root) if native_root is not None else Path(__file__).parent.parent / 'fetch-host-entry-native')
    raw = _bytes(metadata_path, 65536)
    require(_hash(raw) == METADATA_SHA, 'Host metadata pin mismatch')
    manifest_raw = _bytes(root / 'source-hashes.json', 32768)
    require(_hash(manifest_raw) == SOURCE_MANIFEST_SHA, 'Host manifest pin mismatch')
    manifest = _json(manifest_raw)
    require(isinstance(manifest, dict) and set(manifest) == INPUTS, 'Host source inventory')
    for name in INPUTS:
        require(_hash(_bytes(root / name, 1024 * 1024)) == manifest[name], 'Host input changed')
    require(manifest['Host/Program.cs'] == HOST_SHA and manifest['Host/fetch-owned-process.cs'] == TRANSPORT_SHA,
            'Host production input pins')
    m = _json(raw)
    require(isinstance(m, dict) and m.get('controlSha') == QUALIFIED_SHA and m.get('runId') == QUALIFIED_RUN,
            'Host evidence provenance')
    require(m.get('sourceManifestSha256', '').lower() == SOURCE_MANIFEST_SHA, 'Host evidence manifest')
    require(all(m.get(k) is True for k in ('passed', 'initialIdentity', 'finalIdentity', 'hostEntryExecuted')),
            'Host execution identity')
    require(m.get('stage') == 'synthetic-host-entry13' and _integer(m.get('hostOsProcessInvocations'), 13, 13)
            and all(m.get(k) is None for k in ('primaryFailure', 'finalFailure', 'failureCase')), 'Host execution stage')
    for key in ('realCargoExecuted', 'dependencyFetchAttempted', 'fixtureNetworkAttempted', 'productCompileAttempted',
                'productRuntimeAttempted', 'outerJobObjectQualification', 'fullTenMinuteElapsedValidation', 'outerTimeoutObserved'):
        require(m.get(key) is False, 'Host qualification scope')
    isolation = m.get('nugetIsolation', {})
    require(isolation.get('migrationMarkerAtExpectedRoot') is True and _integer(isolation.get('markerBytes'), 0, 0)
            and _integer(m.get('directoryProbeChecks'), 28, 28), 'NuGet isolation proof')
    copies = m.get('copyChecks')
    require(isinstance(copies, list) and len(copies) == 3, 'Host copy proof count')
    for i, check in enumerate(copies):
        require(check.get('phase') == ('final' if i == 2 else 'primary') and _integer(check.get('ordinal'), i, i)
                and check.get('scanComplete') is True and check.get('extraEntriesTruncated') is False
                and _integer(check.get('extraCount'), 0, 0) and check.get('extraEntries') == []
                and check.get('missingIndices') == [] and check.get('changedIndices') == [], 'Host source changed')
    builds = m.get('buildOutcomes')
    require(isinstance(builds, list) and len(builds) == 3, 'Host build proof count')
    for outcome, stage in zip(builds, ('build-host', 'build-fixture', 'build-harness')):
        require(outcome.get('stage') == stage and _integer(outcome.get('exitCode'), 0, 0)
                and all(outcome.get(k) is True for k in ('started', 'exited', 'streamsComplete'))
                and all(outcome.get(k) is False for k in ('timedOut', 'outputExceeded'))
                and outcome.get('diagnosticFailure') is None, 'Host build failed')
    runtime = m.get('runtimeFiles')
    expected = {folder + '/' + base + suffix for folder, base in
                (('Host', 'FetchHost'), ('Fixture', 'cargo'), ('Harness', 'FetchHostEntryHarness'))
                for suffix in ('.exe', '.dll', '.deps.json', '.runtimeconfig.json')}
    require(isinstance(runtime, dict) and set(runtime) == expected and
            all(isinstance(v, str) and re.fullmatch('[0-9a-f]{64}', v) for v in runtime.values()), 'Host runtime inventory')
    cases = m.get('cases')
    require(_integer(m.get('case_count'), 13, 13) and isinstance(cases, list) and len(cases) == 13, 'Host case count')
    for i, (case, name) in enumerate(zip(cases, CASES)):
        executing = i in (0, 11)
        require(case.get('name') == name and case.get('passed') is True and
                _integer(case.get('syntheticCargoExecutions'), int(executing), int(executing)) and
                case.get('expectedRejection') is (not executing), 'Host case evidence')
        if not executing:
            require('hostReceipt' not in case, 'rejected case cannot claim execution receipt')
            continue
        proof = case.get('hostReceipt', {})
        exit_code = 0 if i == 0 else 7
        require(_integer(proof.get('exitCode'), exit_code, exit_code) and
                _integer(proof.get('boundaryRootExitCode'), exit_code, exit_code) and
                _integer(proof.get('boundaryActiveProcessCount'), 0, 0) and
                all(proof.get(k) is True for k in ('treeEmpty', 'safeToStart')) and
                proof.get('succeeded') is (exit_code == 0), 'Host child lifecycle')
        require(all(proof.get(k) is False for k in ('forcedOwnedCleanup', 'timedOut', 'outputExceeded', 'captureFailed'))
                and _integer(proof.get('cleanupMilliseconds'), 0, 5000)
                and _integer(proof.get('outputBytes'), 1, 1048576)
                and _integer(proof.get('outputBudget'), 1048576, 1048576), 'Host child bounds')
    return dict(host_entry_qualified=True, host_executed=True, case_count=13,
                control_sha=QUALIFIED_SHA, run_id=QUALIFIED_RUN, metadata_sha256=METADATA_SHA,
                source_manifest_sha256=SOURCE_MANIFEST_SHA, acquisition_ready=False,
                real_cargo_executed=False, product_compile_attempted=False,
                network_containment_qualified=False, continuous_disk_limit_qualified=False)
