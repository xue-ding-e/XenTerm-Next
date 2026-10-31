"""Public dependency acquisition plan only; no subprocess/network implementation.

Not covered by compile-only native qualification. The eventual controller must
verify installed rustup/Cargo provenance and source/config identities before fetch.
Cargo --offline constrains dependency resolution, not arbitrary build.rs networking.
The bounds below are a proposal, not an implemented or qualified execution policy.
"""
import re
import tomllib
from adapter.compile_adapter import plain, require, digest

TARGET = 'x86_64-pc-windows-msvc'
REGISTRY = 'registry+https://github.com/rust-lang/crates.io-index'
FETCH_POLICY = 'cargo-fetch-only-v1'
FETCH_SECONDS = 600
FETCH_OUTPUT_BUDGET = 1024 * 1024


def public_locked_fetch_plan(root, source_identity):
    root = plain(root)
    source = plain(root / 'source')
    source_identity()
    lock = plain(source / 'Cargo.lock')
    manifest = plain(source / 'Cargo.toml')
    require(lock.stat().st_size <= 2 * 1024 * 1024 and manifest.stat().st_size <= 262144, 'manifest bound')
    before = {p.name: digest(p) for p in (lock, manifest)}
    data = tomllib.loads(lock.read_text(encoding='utf-8'))
    packages = data.get('package', [])
    require(isinstance(packages, list) and packages, 'locked packages absent')
    registry_count = 0
    workspace_count = 0
    for package in packages:
        require(isinstance(package, dict), 'invalid locked package')
        registry = package.get('source')
        if registry is None:
            require(package.get('name') == 'xenterm', 'unexpected local package')
            workspace_count += 1
        else:
            require(registry == REGISTRY, 'unapproved dependency source')
            require(isinstance(package.get('checksum'), str) and re.fullmatch('[0-9a-f]{64}', package['checksum']) is not None, 'missing locked checksum')
            registry_count += 1
    require(workspace_count == 1, 'workspace lock inventory')
    tool_bin = root / 'rustup-home/toolchains' / ('1.99.1-' + TARGET) / 'bin'
    cargo = plain(tool_bin / 'cargo.exe')
    rustc = plain(tool_bin / 'rustc.exe')
    tool_hashes = {p.name: digest(p) for p in (cargo, rustc)}
    source_identity()
    require(before == {p.name: digest(plain(p)) for p in (lock, manifest)}, 'dependency inputs changed')
    require(tool_hashes == {p.name: digest(plain(p)) for p in (cargo, rustc)}, 'dependency tools changed')
    # No inherited user cache/credential/config can be silently substituted.
    # The acquisition controller must materialize a new isolated CARGO_HOME,
    # RUSTUP_HOME and inspect repository/ancestor Cargo config before running.
    # A public lock source is NOT proof that Cargo config cannot redirect downloads.
    return dict(stage='public-locked-fetch-only', executable=str(cargo),
                args=['fetch', '--locked', '--target', TARGET], cwd=str(source),
                required_rustc=str(rustc), input_hashes=before, tool_hashes=tool_hashes,
                commandPolicy=FETCH_POLICY, completionMode='natural-tree',
                seconds=FETCH_SECONDS, outputBudget=FETCH_OUTPUT_BUDGET,
                cleanupMilliseconds=5000, registry=REGISTRY,
                registry_package_count=registry_count, executable_ready=False,
                requires_separate_reviewed_execution=True, executes_product_tests=False,
                compiler_policy_qualification_applicable=False)
