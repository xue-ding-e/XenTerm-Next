#!/usr/bin/env python3
"""Bind package/runtime versions to the checked-out product, never the tooling SHA.

Only scripts/version.sh assembles xt-* strings. A read-only prepare job resolves
once; builds and archive admission use that exact value, without another clock.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib

VERSION_KEYS = {'version_scheme', 'runtime_version', 'package_version', 'version_timestamp'}
SEMVER = re.compile(r'[0-9]+\.[0-9]+\.[0-9]+')
BETA = re.compile(r'xt-beta-([0-9a-f]{7})-T([0-9]{10})')
STABLE = re.compile(r'xt-v([0-9]+\.[0-9]+\.[0-9]+)')


def verify_source(root, source_sha):
    if not re.fullmatch(r'[0-9a-f]{40}', source_sha):
        raise ValueError('Expected a full lowercase product source SHA')
    actual = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'],
                                     text=True, encoding='utf-8', timeout=30).strip()
    if actual != source_sha:
        raise ValueError('Product source SHA mismatch')


def package_version(root):
    version = tomllib.loads((root / 'Cargo.toml').read_text(encoding='utf-8'))['package']['version']
    if not isinstance(version, str) or not SEMVER.fullmatch(version):
        raise ValueError('Expected numeric Cargo package version')
    return version


def modern_source(root):
    module = (root / 'src/version.rs').is_file()
    resolver = (root / 'scripts/version.sh').is_file()
    if module != resolver:
        raise ValueError('Incomplete product runtime version implementation')
    return module


def version_details(runtime, package, scheme, source_sha):
    """Validate a resolved value; deliberately never construct a runtime version."""
    if not isinstance(package, str) or not SEMVER.fullmatch(package):
        raise ValueError('Invalid package version')
    if not isinstance(runtime, str):
        raise ValueError('Missing runtime version')
    timestamp = None
    if scheme == 'cargo-legacy':
        if runtime != package:
            raise ValueError('Legacy runtime must exactly match Cargo package version')
    elif scheme == 'xt-v1':
        beta, stable = BETA.fullmatch(runtime), STABLE.fullmatch(runtime)
        if beta:
            if beta[1] != source_sha[:7]:
                raise ValueError('Runtime version hash differs from product source SHA')
            timestamp = int(beta[2])
        elif not stable or stable[1] != package:
            raise ValueError('Expected source-bound beta or matching stable runtime version')
    else:
        raise ValueError('Unknown runtime version scheme')
    return dict(version_scheme=scheme, runtime_version=runtime,
                package_version=package, version_timestamp=timestamp)


def prepared_metadata(environ=None, *, required=False):
    environ = os.environ if environ is None else environ
    if 'XENTERM_VERSION_METADATA' not in environ:
        if required:
            raise ValueError('Missing prepared version metadata')
        return None
    try:
        details = json.loads(environ['XENTERM_VERSION_METADATA'])
    except (TypeError, ValueError) as error:
        raise ValueError('Invalid prepared version metadata') from error
    if not isinstance(details, dict) or set(details) != VERSION_KEYS:
        raise ValueError('Invalid prepared version metadata fields')
    return details


def expected_version(root, source_sha, environ=None):
    """Packaging is strict: new sources need the prepared XT_VERSION, old do not."""
    environ = os.environ if environ is None else environ
    verify_source(root, source_sha)
    package = package_version(root)
    if modern_source(root):
        runtime = environ.get('XT_VERSION', '')
        if not runtime:
            raise ValueError('New runtime source requires prepared XT_VERSION')
        details = version_details(runtime, package, 'xt-v1', source_sha)
    else:
        if environ.get('XT_VERSION'):
            raise ValueError('Legacy source cannot consume XT_VERSION')
        details = version_details(package, package, 'cargo-legacy', source_sha)
    expected = prepared_metadata(environ)
    if expected is not None and expected != details:
        raise ValueError('Prepared version metadata mismatch')
    return details


def verify_binary_version(root, source_sha, actual, environ=None):
    details = expected_version(root, source_sha, environ)
    if actual != 'xenterm ' + details['runtime_version']:
        raise ValueError('Binary version differs from prepared product runtime version')
    return details


def validate_manifest_version(manifest, source_sha, expected=None):
    details = version_details(manifest.get('runtime_version'), manifest.get('package_version'),
                              manifest.get('version_scheme'), source_sha)
    if any(key not in manifest or manifest[key] != value for key, value in details.items()):
        raise ValueError('Manifest runtime version metadata mismatch')
    if manifest.get('binary_version') != 'xenterm ' + details['runtime_version']:
        raise ValueError('Manifest binary version mismatch')
    if expected is None:
        expected = prepared_metadata()
    if expected is not None and details != expected:
        raise ValueError('Manifest differs from prepared runtime version')
    return details


def resolve_version(root, source_sha, *, stable_tag=None, environ=None):
    """Resolve once with trusted product identity and an explicit release context.

    Fixed headless-* tags are package identities, not legal upstream version
    tags. Next beta/headless therefore explicitly select the resolver's rolling
    beta context; formal vX.Y.Z callers may opt into their verified stable tag.
    """
    root = root.resolve()
    verify_source(root, source_sha)
    package = package_version(root)
    if not modern_source(root):
        return version_details(package, package, 'cargo-legacy', source_sha)
    env = dict(os.environ if environ is None else environ)
    for key in tuple(env):
        if key in ('XT_VERSION', 'XT_CHANNEL', 'XT_VERSION_JSON', 'VERSION') or key.startswith('GITHUB_REF'):
            env.pop(key)
    if stable_tag is not None and (not re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+', stable_tag)
                                   or stable_tag[1:] != package):
        raise ValueError('Stable tag must match Cargo package version')
    env.update(GITHUB_SHA=source_sha, GITHUB_REF_TYPE='tag',
               GITHUB_REF_NAME=stable_tag or 'beta')
    # Resolver JSON belongs to this temporary invocation, not the product tree.
    with tempfile.TemporaryDirectory() as directory:
        env['XT_VERSION_JSON'] = str(Path(directory) / 'version.json')
        runtime = subprocess.check_output(['bash', str(root / 'scripts/version.sh')],
                                          cwd=root, env=env, text=True, encoding='utf-8', timeout=30).strip()
        resolved = json.loads(Path(env['XT_VERSION_JSON']).read_text(encoding='utf-8'))
    if resolved.get('version') != runtime:
        raise ValueError('Resolver stdout and version metadata differ')
    details = version_details(runtime, package, 'xt-v1', source_sha)
    if stable_tag is None and not BETA.fullmatch(runtime):
        raise ValueError('Next beta/headless requires beta runtime version')
    return details


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('--source-sha', required=True)
    parser.add_argument('--stable-tag')
    parser.add_argument('--github-output', type=Path)
    parser.add_argument('--github-env', type=Path)
    args = parser.parse_args()
    details = resolve_version(args.source, args.source_sha, stable_tag=args.stable_tag)
    encoded = json.dumps(details, separators=(',', ':'))
    runtime = details['runtime_version'] if details['version_scheme'] == 'xt-v1' else ''
    if args.github_output:
        with args.github_output.open('a', encoding='utf-8') as output:
            output.write(f'xt_version={runtime}\nversion_metadata={encoded}\n')
    if args.github_env:
        with args.github_env.open('a', encoding='utf-8') as output:
            output.write(f'XT_VERSION={runtime}\nXENTERM_VERSION_METADATA={encoded}\n')
    print(encoded)


if __name__ == '__main__':
    main()
