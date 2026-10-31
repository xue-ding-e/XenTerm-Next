"""Read-only attestation for transport fixtures, NOT permission to acquire packages.

The pinned artifact proves eight synthetic cases and that FetchHost compiled.
It did not invoke FetchHost or Cargo, and cannot unlock an execution API.
"""
import hashlib
import json
import re
from pathlib import Path
from compile_adapter import plain, require

QUALIFIED_SHA = 'af4655706b2c61a380a18e69a6fecc8372a7d076'
QUALIFIED_RUN = '41185823982'
TRANSPORT_SHA = 'd6d9d3b5c31a2f697eb435cdbf99f0fc968e2af912b069923bc6f8c7497331b3'
METADATA_SHA = '34dc211d4aaf5e3615a81712c49839ad33ea60274aeeda0a04bb0b5f4af36855'
SOURCE_MANIFEST_SHA = '891c87632693c5b68a67752698dc2d4c0ad7d6668b034e941a58c398ad757564'
HOST_SHA = '0f94cd746e8a04d7bb5e794683a30b7512d406319eccb3e3c4e6fb3d79e29ae3'
INPUTS = frozenset(('.github/workflows/ci.yml', 'NuGet.Config', 'global.json', 'run-fetch.ps1',
    'Fixture/Program.cs', 'Fixture/Fixture.csproj', 'Harness/Program.cs', 'Harness/Harness.csproj',
    'Harness/fetch-owned-process.cs', 'Harness/short-clock-owned-process.cs',
    'Host/FetchHost.csproj', 'Host/Program.cs', 'Host/fetch-owned-process.cs', 'Host/NuGet.Config', 'Host/global.json'))
CASE_NAMES = ('fetch-policy-natural-tree-cleanup', 'fetch-policy-output-hard-cap',
    'fetch-policy-timeout-cleanup', 'fetch-policy-argv-rejection', 'fetch-policy-root-mode-rejection',
    'fetch-policy-deadline-rejection', 'fetch-policy-rustup-selector-rejection', 'unknown-policy-rejection')


def _bytes(path, limit):
    path = plain(Path(path))
    require(path.is_file() and 0 < path.stat().st_size <= limit, 'evidence file bound')
    value = path.read_bytes()
    require(0 < len(value) <= limit, 'evidence read bound')
    return value


def _object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'duplicate evidence field')
        result[key] = value
    return result


def _json(data):
    return json.loads(data, object_pairs_hook=_object)


def _hash(data):
    return hashlib.sha256(data).hexdigest()


def verify_transport_evidence(metadata_path, native_root=None):
    """Authenticate pinned downloaded bytes and exact local source, without launch.

    No caller boolean, mutable receipt or arbitrary native source can substitute
    for these hashes. Result intentionally excludes Host/runtime readiness.
    """
    root = plain(Path(native_root) if native_root is not None else Path(__file__).parent.parent / 'fetch-native')
    data = _bytes(metadata_path, 65536)
    require(_hash(data) == METADATA_SHA, 'fetch metadata pin mismatch')
    manifest_data = _bytes(root / 'source-hashes.json', 32768)
    require(_hash(manifest_data) == SOURCE_MANIFEST_SHA, 'fetch manifest pin mismatch')
    manifest = _json(manifest_data)
    require(isinstance(manifest, dict) and set(manifest) == INPUTS, 'fetch source inventory')
    for name in sorted(INPUTS):
        require(_hash(_bytes(root / name, 1024 * 1024)) == manifest[name], 'fetch input changed')
    here = root / 'Host'
    for path in (root / 'Harness/fetch-owned-process.cs', root / 'Host/fetch-owned-process.cs', here / 'fetch-owned-process.cs'):
        require(_hash(_bytes(path, 1024 * 1024)) == TRANSPORT_SHA, 'fetch transport pin mismatch')
    for path in (root / 'Host/Program.cs', here / 'Program.cs'):
        require(_hash(_bytes(path, 1024 * 1024)) == HOST_SHA, 'fetch Host pin mismatch')
    m = _json(data)
    require(isinstance(m, dict) and m.get('controlSha') == QUALIFIED_SHA and m.get('runId') == QUALIFIED_RUN,
            'fetch evidence provenance')
    require(m.get('sourceManifestSha256', '').lower() == SOURCE_MANIFEST_SHA, 'fetch evidence manifest')
    require(all(m.get(key) is True for key in ('passed', 'initialIdentity', 'finalIdentity')),
            'fetch evidence identity')
    require(m.get('stage') == 'native-fetch8' and m.get('failureType') is None and m.get('compilerCodes') == [],
            'fetch evidence stage')
    for key in ('productCompileAttempted', 'productRuntimeAttempted', 'fullTenMinuteElapsedValidation',
                'fixtureNetworkAttempted', 'dependencyFetchAttempted', 'hostExecuted'):
        require(m.get(key) is False, 'fetch evidence scope')
    suites = m.get('suites')
    require(isinstance(suites, dict) and set(suites) == {'fetch8'}, 'fetch suite inventory')
    suite = suites['fetch8']
    require(suite.get('controlSha') == QUALIFIED_SHA and suite.get('runId') == QUALIFIED_RUN
            and type(suite.get('exit')) is int and suite['exit'] == 0, 'fetch suite identity')
    for values, name in ((m.get('compiledHostFiles'), 'FetchHost'),
                         (suite.get('fixtureFiles'), 'cargo'),
                         (suite.get('harnessFiles'), 'FetchPolicyHarness')):
        require(isinstance(values, dict) and set(values) == {
            name + suffix for suffix in ('.exe', '.dll', '.deps.json', '.runtimeconfig.json')}
            and all(isinstance(value, str) and re.fullmatch('[0-9a-f]{64}', value) is not None
                    for value in values.values()), 'fetch runtime identity inventory')
    r = suite['receipt']; cases = r.get('cases')
    require(r.get('passed') is True and type(r.get('case_count')) is int and r['case_count'] == 8
            and isinstance(cases, list) and len(cases) == 8 and r.get('failureCase') is None,
            'fetch cases missing')
    require(all(r.get(k) is False for k in ('productCompileAttempted', 'productRuntimeAttempted',
                'fullTenMinuteElapsedValidation', 'networkAttempted')), 'fetch receipt scope')
    for index, case in enumerate(cases):
        require(case.get('name') == CASE_NAMES[index] and case.get('passed') is True, 'fetch case identity')
        count = case.get('actualExecutions')
        require(type(count) is int and count == (3 if index < 3 else 0), 'fetch execution count')
        if index >= 3:
            require(case.get('expectedRejection') is True, 'fetch rejection absent')
        else:
            require(case.get('treeEmpty') is True and case.get('safeToStart') is True
                    and type(case.get('cleanupMilliseconds')) is int and 0 <= case['cleanupMilliseconds'] <= 5000
                    and type(case.get('productionRequestedSeconds')) is int and case['productionRequestedSeconds'] == 600,
                    'fetch cleanup evidence')
    normal, overflow, clock = cases[:3]
    require(normal.get('injectedClock') is False and normal.get('retainedBytes') == 0
            and normal.get('outputExceeded') is False and normal.get('forcedOwnedCleanup') is False, 'fetch natural case')
    require(overflow.get('injectedClock') is False and overflow.get('outputExceeded') is True
            and overflow.get('forcedOwnedCleanup') is False and overflow.get('retainedBytes') == 1048576
            and overflow.get('stdoutBytes', 0) > 0 and overflow.get('stderrBytes', 0) > 0
            and overflow['stdoutBytes'] + overflow['stderrBytes'] == 1048576, 'fetch output cap evidence')
    require(clock.get('injectedClock') is True and clock.get('injectedMilliseconds') == 8000
            and clock.get('timedOut') is True, 'fetch short-clock evidence')
    return dict(transport_qualified=True, control_sha=QUALIFIED_SHA, run_id=QUALIFIED_RUN,
                metadata_sha256=METADATA_SHA, source_manifest_sha256=SOURCE_MANIFEST_SHA,
                case_count=8, host_compiled=True, host_executed=False, acquisition_ready=False,
                product_compile_attempted=False, full_ten_minute_elapsed_validation=False)
