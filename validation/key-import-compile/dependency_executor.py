"""Fail-closed dependency stage placeholder, NOT a process launcher.

Neither smoke-v1 nor cargo-compile-only-v2 qualifies network acquisition.
No caller-supplied boolean, receipt or callback can activate this placeholder.
"""
import re
import tomllib
from adapter.compile_adapter import require, plain, digest, read_json
from dependency_plan import FETCH_POLICY, FETCH_SECONDS, FETCH_OUTPUT_BUDGET, REGISTRY, TARGET

REQUIRED_QUALIFICATION = (
    'dedicated-owned-fetch-policy-and-host-fixed-command-validation',
    'native-natural-tree-timeout-output-cap-cleanup-and-rejection-evidence',
    'new-transport-host-source-run-artifact-hash-pins',
    'source-lock-manifest-and-real-toolchain-identity-before-and-after',
    'isolated-home-cache-no-user-credentials-or-inherited-environment',
    'reviewed-cargo-config-ancestors-source-replacements-and-public-network-scope',
    'bounded-download-disk-budget-and-public-endpoint-redirect-policy',
    'single-use-owner-unknown-lifecycle-poisoning-and-no-automatic-retry',
    'post-fetch-lock-unchanged-cache-integrity-and-no-product-execution-receipt',
)


def fetch_readiness(plan):
    # Deliberately do not treat plan fields as authority or executable evidence.
    return dict(ready=False, state='blocked-unqualified-fetch-policy',
                missing=list(REQUIRED_QUALIFICATION), network_attempted=False,
                product_compile_attempted=False, product_runtime_attempted=False)


def execute_fetch(plan, owned_transport=None, qualification=None):
    # Must remain unconditional until a separately reviewed implementation and
    # native qualification exist. In particular never fall back to smoke policy.
    require(False, 'fetch execution unavailable: dedicated owned policy not qualified')


def validate_fetch_receipt(receipt, plan, root, control_sha, run_id):
    """Validate same-run local evidence, not authorization or cache completeness.

    This consumes a controller summary: unchanged raw Host lifecycle fields plus
    control_sha/run_id/input_hashes/tool_hashes/cache_scope/cache_files and the
    explicit false all_dependencies_available claim. The raw Host receipt alone
    is intentionally insufficient; summarize_fetch_receipt reads back local proof.
    This must follow a separately qualified executor; arbitrary JSON cannot prove
    a process ran. Only listed, checksum-matched sparse crates are attested here.
    Target-specific fetch may produce a subset; offline compilation remains the
    availability judge. No cache directory scan, network, extraction or execution.
    """
    root = plain(root)
    require(isinstance(receipt, dict) and isinstance(plan, dict), 'fetch evidence shape')
    require(isinstance(control_sha, str) and re.fullmatch('[0-9a-f]{40}', control_sha) is not None
            and re.fullmatch('[0-9]+', str(run_id)) is not None, 'fetch current run identity')
    require(type(receipt.get('schema')) is int and receipt['schema'] == 1
            and receipt.get('control_sha') == control_sha and receipt.get('run_id') == str(run_id), 'fetch receipt provenance')
    require(receipt.get('state') == 'returned' and receipt.get('succeeded') is True, 'fetch Host return state')
    require(receipt.get('commandPolicy') == FETCH_POLICY and receipt.get('completionMode') == 'natural-tree'
            and type(receipt.get('seconds')) is int and receipt['seconds'] == FETCH_SECONDS
            and type(receipt.get('outputBudget')) is int and receipt['outputBudget'] == FETCH_OUTPUT_BUDGET, 'fetch receipt policy')
    for key in ('treeEmpty', 'safeToStart', 'boundaryRootSignaled', 'boundaryRootExitCodeKnown'):
        require(receipt.get(key) is True, 'fetch lifecycle incomplete')
    for key in ('timedOut', 'outputExceeded', 'captureFailed', 'forcedOwnedCleanup', 'reusable',
                'productCompileAttempted', 'productRuntimeAttempted', 'all_dependencies_available'):
        require(receipt.get(key) is False, 'fetch lifecycle or scope failure')
    for key in ('exitCode', 'boundaryRootExitCode', 'boundaryActiveProcessCount'):
        require(type(receipt.get(key)) is int and receipt[key] == 0, 'fetch exit boundary failure')
    require(type(receipt.get('cleanupMilliseconds')) is int and 0 <= receipt['cleanupMilliseconds'] <= 5000,
            'fetch cleanup bound')
    require(type(receipt.get('outputBytes')) is int and 0 <= receipt['outputBytes'] <= FETCH_OUTPUT_BUDGET,
            'fetch output bound')
    logs = [plain(root / ('fetch.' + stream + '.log')) for stream in ('stdout', 'stderr')]
    require(all(path.is_file() for path in logs) and sum(path.stat().st_size for path in logs) == receipt['outputBytes'],
            'fetch retained output bytes')
    tool_bin = root / 'rustup-home/toolchains' / ('1.99.1-' + TARGET) / 'bin'
    require(plan.get('executable') == str(tool_bin / 'cargo.exe') and plan.get('required_rustc') == str(tool_bin / 'rustc.exe')
            and plan.get('cwd') == str(root / 'source') and plan.get('args') == ['fetch', '--locked', '--target', TARGET]
            and plan.get('commandPolicy') == FETCH_POLICY and plan.get('completionMode') == 'natural-tree'
            and type(plan.get('seconds')) is int and plan['seconds'] == FETCH_SECONDS
            and type(plan.get('outputBudget')) is int and plan['outputBudget'] == FETCH_OUTPUT_BUDGET, 'fetch plan binding')
    for field, paths in (
            ('input_hashes', [root / 'source/Cargo.toml', root / 'source/Cargo.lock']),
            ('tool_hashes', [tool_bin / 'cargo.exe', tool_bin / 'rustc.exe'])):
        actual = {}
        for path in paths:
            path = plain(path)
            bound = 2 * 1024 * 1024 if field == 'input_hashes' else 128 * 1024 * 1024
            require(path.is_file() and path.stat().st_size <= bound, 'fetch input size')
            actual[path.name] = digest(path)
        require(receipt.get(field) == plan.get(field) == actual, 'fetch inputs or tools changed')
    lock = tomllib.loads((root / 'source/Cargo.lock').read_text(encoding='utf-8'))
    packages = lock.get('package', [])
    require(isinstance(packages, list) and packages, 'fetch lock inventory')
    expected = {}
    local = 0
    for package in packages:
        require(isinstance(package, dict), 'fetch locked package')
        if package.get('source') is None:
            require(package.get('name') == 'xenterm', 'fetch local package'); local += 1
            continue
        require(package.get('source') == REGISTRY, 'fetch dependency source')
        name, version, checksum = (package.get(k) for k in ('name', 'version', 'checksum'))
        require(isinstance(name, str) and re.fullmatch('[A-Za-z0-9_-]+', name) is not None
                and isinstance(version, str) and re.fullmatch('[0-9A-Za-z.+_-]+', version) is not None
                and isinstance(checksum, str) and re.fullmatch('[0-9a-f]{64}', checksum) is not None, 'fetch locked checksum')
        relative = 'cargo-home/registry/cache/index.crates.io-1949cf8c6b5b557f/' + name + '-' + version + '.crate'
        require(relative not in expected, 'duplicate locked crate')
        expected[relative] = checksum
    require(local == 1, 'fetch workspace inventory')
    rows = receipt.get('cache_files')
    require(receipt.get('cache_scope') == 'verified-subset' and isinstance(rows, list)
            and len(rows) <= len(expected) and (bool(rows) or not expected), 'fetch cache evidence absent')
    seen = set(); total = 0
    for row in rows:
        require(isinstance(row, dict) and set(row) == {'relative', 'sha256', 'bytes'}, 'fetch cache row')
        relative = row['relative']
        require(isinstance(relative, str) and relative in expected and relative not in seen, 'unexpected or duplicate crate')
        seen.add(relative)
        path = plain(root / relative)
        require(path.is_file() and type(row['bytes']) is int and 0 < row['bytes'] <= 64 * 1024 * 1024
                and path.stat().st_size == row['bytes'], 'fetch crate size')
        total += row['bytes']; require(total <= 512 * 1024 * 1024, 'fetch cache evidence budget')
        require(row['sha256'] == expected[relative] == digest(path), 'fetch crate checksum')
    return dict(validated=True, cache_scope='verified-subset', verified_crate_count=len(seen),
                locked_registry_count=len(expected), all_dependencies_available=False,
                offline_compile_required=True, product_compile_attempted=False, product_runtime_attempted=False)


def summarize_fetch_receipt(plan, root, control_sha, run_id):
    """Read back fixed local Host/cache evidence without launching anything.

    Only a future qualified executor may call this after its same-run Host has
    returned. This reader is NOT a replacement for executor/run authentication.
    It cannot activate execute_fetch, accepts no user-supplied passed flag, does
    not scan directories and never reads an unrelated/user cache.
    """
    root = plain(root)
    marker = plain(root / 'fetch-started.marker')
    require(marker.is_file() and marker.stat().st_size == 1 and marker.read_bytes() == b'\x01', 'fetch owner marker absent')
    host = read_json(root / 'fetch.receipt.json', 32768)
    require(isinstance(host, dict) and set(host) == {
        'schema', 'state', 'seconds', 'outputBytes', 'succeeded', 'exitCode', 'timedOut',
        'outputExceeded', 'captureFailed', 'treeEmpty', 'forcedOwnedCleanup',
        'completionMode', 'commandPolicy', 'outputBudget', 'cleanupMilliseconds',
        'boundaryRootSignaled', 'boundaryActiveProcessCount', 'boundaryRootExitCode',
        'boundaryRootExitCodeKnown', 'safeToStart', 'reusable',
        'productCompileAttempted', 'productRuntimeAttempted'}, 'fetch Host receipt inventory')
    require(isinstance(plan, dict), 'fetch plan absent')
    tool_bin = root / 'rustup-home/toolchains' / ('1.99.1-' + TARGET) / 'bin'
    input_hashes = {}
    tool_hashes = {}
    for destination, paths, bound in (
            (input_hashes, [root / 'source/Cargo.lock', root / 'source/Cargo.toml'], 2 * 1024 * 1024),
            (tool_hashes, [tool_bin / 'cargo.exe', tool_bin / 'rustc.exe'], 128 * 1024 * 1024)):
        for path in paths:
            path = plain(path)
            require(path.is_file() and path.stat().st_size <= bound, 'fetch summary input size')
            destination[path.name] = digest(path)
    require(input_hashes == plan.get('input_hashes') and tool_hashes == plan.get('tool_hashes'), 'fetch summary inputs changed')
    packages = tomllib.loads((root / 'source/Cargo.lock').read_text(encoding='utf-8')).get('package', [])
    require(isinstance(packages, list) and packages, 'fetch summary lock inventory')
    rows = []; total = 0; seen = set()
    for package in packages:
        require(isinstance(package, dict), 'fetch summary package')
        if package.get('source') is None:
            require(package.get('name') == 'xenterm', 'fetch summary local package')
            continue
        require(package.get('source') == REGISTRY, 'fetch summary source')
        name, version = package.get('name'), package.get('version')
        require(isinstance(name, str) and re.fullmatch('[A-Za-z0-9_-]+', name) is not None
                and isinstance(version, str) and re.fullmatch('[0-9A-Za-z.+_-]+', version) is not None,
                'fetch summary crate identity')
        relative = 'cargo-home/registry/cache/index.crates.io-1949cf8c6b5b557f/' + name + '-' + version + '.crate'
        require(relative not in seen, 'duplicate summary crate'); seen.add(relative)
        path = root / relative
        try:
            path.lstat()
        except FileNotFoundError:
            continue
        path = plain(path)
        require(path.is_file(), 'fetch summary crate type')
        size = path.stat().st_size
        require(0 < size <= 64 * 1024 * 1024, 'fetch summary crate bound')
        total += size; require(total <= 512 * 1024 * 1024, 'fetch summary byte budget')
        rows.append(dict(relative=relative, bytes=size, sha256=digest(path)))
    summary = dict(host, control_sha=control_sha, run_id=str(run_id), input_hashes=input_hashes,
                   tool_hashes=tool_hashes, cache_scope='verified-subset', cache_files=rows,
                   all_dependencies_available=False)
    validate_fetch_receipt(summary, plan, root, control_sha, run_id)
    return summary
