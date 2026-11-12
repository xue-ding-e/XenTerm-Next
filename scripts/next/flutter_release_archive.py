"""Move verified old Flutter generations into durable historical Releases.

This module never promotes the rolling tag. The caller publishes the current
generation first and may report cleanup errors separately from publication.
Every canonical byte remains in the rolling release until its historical copy
is published and verified. Manual assets are outside this module's ownership.
"""

import json
from pathlib import Path
import re
import stat
import tempfile


GENERATION = re.compile(r'XenTerm-Next-Flutter-([0-9a-f]{40})-BUILD\.json')


def generation_names(sha, platforms):
    prefix = f'XenTerm-Next-Flutter-{sha}'
    names = {prefix + '-BUILD.json', prefix + '-SHA256SUMS'}
    for platform in platforms:
        stem = f'{prefix}-{platform}'
        names.update((stem + '-BUILD.json', stem + '.sha256',
                      stem + ('.tar.gz' if platform.startswith('linux') else '.zip')))
        if platform == 'windows-x86_64':
            names.add(stem + '.msi')
    return names


def asset_index(assets):
    result = {}
    for asset in assets:
        if isinstance(asset, dict) and isinstance(asset.get('name'), str):
            result.setdefault(asset['name'], []).append(asset)
    return result


def owned_assets(assets, names):
    index = asset_index(assets)
    if any(len(index.get(name, [])) != 1 for name in names):
        raise ValueError('Incomplete or ambiguous historical generation')
    result = {name: dict(index[name][0]) for name in names}
    for asset in result.values():
        if (type(asset.get('id')) is not int or asset['id'] <= 0
                or type(asset.get('size')) is not int or asset['size'] <= 0
                or asset.get('state') != 'uploaded'):
            raise ValueError('Invalid historical asset identity')
    if len({asset['id'] for asset in result.values()}) != len(names):
        raise ValueError('Duplicate historical asset IDs')
    return result


def check_asset(actual, expected):
    # GET by ID immediately before DELETE prevents deleting a renamed/replaced
    # asset. Older GitHub assets may omit digest; download verification remains
    # mandatory and timestamps/digests are additionally bound when available.
    fields = ('id', 'name', 'size', 'state')
    fields += tuple(key for key in ('digest', 'updated_at') if expected.get(key) is not None)
    if not isinstance(actual, dict) or any(actual.get(key) != expected.get(key) for key in fields):
        raise ValueError('Rolling asset changed during archival: ' + expected['name'])


def release_identity(actual, rid, tag, sha):
    if (not isinstance(actual, dict) or type(actual.get('id')) is not int
            or actual['id'] != rid or actual.get('tag_name') != tag
            or actual.get('target_commitish') != sha or type(actual.get('draft')) is not bool):
        raise ValueError('Historical release identity mismatch')


def archive_body(metadata, hashes, tag, repository):
    sha, run = metadata['source_sha'], metadata['build_run']
    links = '\n'.join(
        f'- [{name}](https://github.com/{repository}/releases/download/{tag}/{name})'
        for name in sorted(hashes))
    return (f'Historical XenTerm Next Flutter build.\n\n'
            f'Source SHA: `{sha}`\nUpstream snapshot: `{metadata["upstream_snapshot"]}`\n'
            f'Runtime version: `{metadata["runtime_version"]}`\n'
            f'Product CI: https://github.com/{repository}/actions/runs/{run}\n\n'
            'Full build provenance:\n\n```json\n'
            + json.dumps(metadata, indent=2, ensure_ascii=True) + '\n```\n\n' + links)


def historical_release(api, rolling_id, sha, run, body):
    tag = f'flutter-build-{sha[:12]}-{run}'
    target = api.tag(tag)
    if target not in ('', sha):
        raise ValueError('Historical tag already identifies another source: ' + tag)
    matches = [item for page in api.pages('releases?per_page=100') for item in page
               if item.get('tag_name') == tag]
    if len(matches) > 1:
        raise ValueError('Ambiguous historical release: ' + tag)
    title = f'XenTerm Next Flutter build ({sha[:12]})'
    if matches:
        historical = matches[0]
    else:
        historical = api.api('releases', 'POST', {
            'tag_name': tag, 'target_commitish': sha, 'name': title,
            'draft': True, 'prerelease': True, 'body': body, 'make_latest': 'false',
        })
    rid = historical.get('id')
    if type(rid) is not int or rid <= 0 or rid == rolling_id:
        raise ValueError('Invalid historical release ID')
    release_identity(historical, rid, tag, sha)
    return historical, tag, title


def verify_archive(api, historical, tag, sha, body, title, directory, hashes):
    rid = historical['id']
    actual = api.api(f'releases/{rid}')
    release_identity(actual, rid, tag, sha)
    if (actual['draft'] is not False or actual.get('body') != body
            or actual.get('name') != title or api.tag(tag) != sha):
        raise ValueError('Historical publication readback mismatch')
    assets = owned_assets(api.assets(rid), set(hashes))
    for name in sorted(hashes):
        api.verify(assets[name], directory / name, hashes[name])


def archive_old_generations(api, rolling_release, current_sha, current_run, keep_previous_sha):
    """Return summaries of archived generations; errors leave durable copies.

    Incomplete/invalid source generations and unsuccessful native product runs
    are ignored. Remote failures propagate so the publisher can log cleanup
    separately. An interrupted deletion can leave partial rolling assets; their
    complete historical release remains available for download and recovery.
    """
    # Lazy import also supports the publisher importing this module at startup.
    import flutter_release as release

    release.full_sha(current_sha)
    if keep_previous_sha:
        release.full_sha(keep_previous_sha)
    if not str(current_run).isdigit() or int(current_run) <= 0:
        raise ValueError('Expected a positive current build run ID')
    rid = rolling_release.get('id')
    if (type(rid) is not int or rid <= 0 or rolling_release.get('draft') is not False
            or rolling_release.get('tag_name') != release.TAG or rolling_release.get('immutable')):
        raise ValueError('Archive cleanup requires a published mutable Flutter release')
    api.check(current_sha, current_run)
    if api.tag() != current_sha:
        raise ValueError('Current Flutter tag changed before archival')
    source_index = asset_index(api.assets(rid))
    protected = {current_sha, keep_previous_sha}
    candidates = sorted({match[1] for name in source_index
                         if (match := GENERATION.fullmatch(name)) and match[1] not in protected})
    results = []
    for sha in candidates:
        names = generation_names(sha, release.PLATFORMS)
        try:
            sources = owned_assets([item for values in source_index.values() for item in values], names)
        except ValueError:
            continue
        with tempfile.TemporaryDirectory(prefix='flutter-release-archive-') as temporary:
            directory = Path(temporary)
            for name in sorted(names):
                path = directory / name
                api.download(sources[name], path)
                info = path.lstat()
                if not stat.S_ISREG(info.st_mode) or info.st_size != sources[name]['size']:
                    raise ValueError('Historical download is not an exact regular file: ' + name)
            try:
                metadata_path = directory / f'XenTerm-Next-Flutter-{sha}-BUILD.json'
                metadata = json.loads(metadata_path.read_text(encoding='utf-8-sig'))
                if not isinstance(metadata, dict) or metadata.get('source_sha') != sha:
                    continue
                run = metadata.get('build_run')
                if not isinstance(run, str) or not run.isdigit() or int(run) <= 0:
                    continue
                metadata, hashes = release.verify_packages(directory, sha, run)
            except (ValueError, KeyError, TypeError):
                continue
            run_object = api.api(f'actions/runs/{run}')
            if run_object.get('status') != 'completed' or run_object.get('conclusion') != 'success':
                continue
            jobs = [job for page in api.pages(f'actions/runs/{run}/jobs?filter=all&per_page=100') for job in page['jobs']]
            try:
                # An old successful run is evaluated against its own source,
                # never against today's flutter-next branch head.
                release.source_gate(sha, run, run_object, sha, jobs)
            except ValueError:
                continue
            if set(hashes) != names:
                raise ValueError('Unexpected historical generation contract')
            api.check(current_sha, current_run)
            tag = f'flutter-build-{sha[:12]}-{run}'
            body = archive_body(metadata, hashes, tag, release.REPOSITORY)
            historical, tag, title = historical_release(api, rid, sha, run, body)
            archive_id = historical['id']
            known = asset_index(api.assets(archive_id))
            for name in sorted(names):
                matches = known.get(name, [])
                if len(matches) > 1:
                    raise ValueError('Duplicate historical asset names: ' + name)
                if matches:
                    asset = matches[0]
                else:
                    if historical.get('immutable'):
                        raise ValueError('Cannot complete an immutable historical release')
                    asset = api.upload(directory / name, archive_id)
                api.verify(asset, directory / name, hashes[name])
            api.check(current_sha, current_run)
            if api.tag(tag) not in ('', sha):
                raise ValueError('Historical tag changed before publication')
            if (historical['draft'] or historical.get('body') != body or historical.get('name') != title):
                if historical.get('immutable'):
                    raise ValueError('Cannot publish an immutable historical release')
                api.api(f'releases/{archive_id}', 'PATCH', {
                    'name': title, 'target_commitish': sha, 'body': body, 'draft': False,
                    'prerelease': True, 'make_latest': 'false',
                })
            verify_archive(api, historical, tag, sha, body, title, directory, hashes)
            # Check every original before deleting any. This also catches
            # same-size corruption if GitHub has no digest for an old asset.
            for name in sorted(names):
                check_asset(api.api(f'releases/assets/{sources[name]["id"]}'), sources[name])
                api.verify(sources[name], directory / name, hashes[name])
            # Re-read the complete, public archive after source preflight; an
            # interrupted/uncertain archive must never consume rolling copies.
            verify_archive(api, historical, tag, sha, body, title, directory, hashes)
            api.check(current_sha, current_run)
            actual = api.api(f'releases/{rid}')
            if (actual.get('id') != rid or actual.get('tag_name') != release.TAG
                    or actual.get('draft') is not False or api.tag() != current_sha):
                raise ValueError('Rolling publication changed before cleanup')
            # Keep provenance sidecars until last if cleanup is interrupted.
            metadata_name = f'XenTerm-Next-Flutter-{sha}-BUILD.json'
            sums_name = f'XenTerm-Next-Flutter-{sha}-SHA256SUMS'
            order = sorted(names - {metadata_name, sums_name}) + [sums_name, metadata_name]
            for name in order:
                source = sources[name]
                check_asset(api.api(f'releases/assets/{source["id"]}'), source)
                api.api(f'releases/assets/{source["id"]}', 'DELETE')
            results.append({'source_sha': sha, 'build_run': run, 'tag_name': tag,
                            'release_id': archive_id, 'deleted_assets': len(names)})
    return results
