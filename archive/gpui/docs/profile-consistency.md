# Concurrent profile changes

Current GUI, background-save, CLI and MCP import writers use the same optimistic
commit boundary. Multiple windows or processes may read a profile, but a cached
writer cannot overwrite a newer committed snapshot. This is conflict detection,
not automatic merging.

## When a save is rejected

- Keep or copy your pending form edits before closing the application.
- Reopen XenTerm to load the latest profile, inspect the changes, then reapply
  your intended edit. Repeatedly pressing Save on the old snapshot cannot force
  an overwrite.
- Background failures remain visible in a persistent warning. A session editor
  stays open with its draft after a conflict.
- CLI/MCP import returns an error rather than committing a stale batch. Reload
  by starting a new CLI command or making a new service request after reviewing
  the latest profile. Import never implicitly saves unrelated pending edits,
  including in a newly initialized profile.

The service profile still needs its own explicit `--data-dir`. Selecting the
same directory does not convert desktop OS-keyring credentials into portable
credentials; see [configuration import](config-import.md).

## Commit and recovery boundary

A load reads the raw database fingerprint and decoded cache from one SQLite
transaction. Every successful writer changes a `meta.write_revision` token.
The next write checks both the token and every raw meta/session/history row
under `BEGIN IMMEDIATE`, before applying its changes. No schema-version bump is
needed. Background snapshots share the latest saved baseline and submission
order; a delayed older job cannot land after a newer foreground or background
save.

A short OS file lock also spans configuration/keyring loading, keyring changes,
SQL commit, and credential compensation. It does not reserve the profile for
the application's lifetime. Directory and database symlink aliases use the
same canonical lock identity. The OS releases the lock when a process exits.

Before changing keyring credentials, the writer durably stages an authenticated,
encrypted recovery journal using the profile's existing master key. On the next
load, the commit token distinguishes a committed write from an unfinished one;
recovery either keeps the committed credentials or restores and verifies their
previous values. Unverifiable or ambiguous recovery is rejected. A failed
credential recovery remains visible and blocks that cached instance's later
writes, including no-op saves. Reopen the original desktop application to try
recovery, and inspect credentials before continuing if the error persists.

Do not remove transaction sidecars while a profile is in use. Treat an encrypted
credential journal as private profile data and keep it with the profile and
matching key when making a manual recovery copy. A headless/explicit service
profile refuses a pending desktop keyring recovery journal before initialization.

## Startup and backups

An initialized profile, even one with no sessions, is never replaced by automatic
backup restoration. A legacy JSON source is retained; a no-clobber `.migrated`
archive is created only after migration commits. An interrupted migration with
an empty SQLite schema can still import its retained JSON source.

Automatic mirrors now use a profile-specific directory under the legacy root's
`xenterm-backups` directory. The old legacy profile is only a read-only recovery
source, never a mirror destination. Mirrors take a fresh WAL-consistent snapshot
under the primary write lock, so a delayed mirror cannot publish an older cache.
Recovery validates the source credentials and matching local key before
publication. Keyring-backed backups are not automatically moved into portable storage. The
original files remain intact: reopen the original application/profile with its
OS keyring, recover there if necessary, then create a portable export. Missing-
or mismatched-key backups likewise never cause an unrelated key to be generated.

## Limits

Upgrade every active writer. Raw-row fingerprints detect ordinary writes from
older versions that do not update the token, but an old application can still
write unconditionally or change only its external keyring entry. No protection
against those older writers is claimed.

Use a filesystem that supports SQLite WAL, advisory file locking, and atomic
no-clobber publication. Network filesystems and externally replacing database,
key, lock or journal files while a profile is open are unsupported. Platform-
native no-replace publication is used on current Linux, macOS and Windows;
there is no unsafe overwrite fallback. Hardware/OS failures and an unavailable
keyring can still prevent recovery, in which case the application preserves the
journal and reports the uncertainty instead of claiming that data was restored.
