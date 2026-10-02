# CLI/MCP session imports

XenTerm accepts the original MeatShell portable export without conversion. The
export marker stays `"meatshell_export": 1`, and `enc:exp:v1:` credentials retain
the original portable format. This fixed-key format is reversible obfuscation,
not protection against someone who has the file. Keep exports private; do not
paste them into prompts or version control.

## Isolated profiles

```sh
xenterm --data-dir /absolute/private/profile cli sessions --json
xenterm --data-dir /absolute/private/profile cli import ./export.json --dry-run --json
xenterm --data-dir /absolute/private/profile cli import ./export.json --json
xenterm --data-dir /absolute/private/profile cli session <session-id> --json
```

Put the global `--data-dir` before the command. `XENTERM_DATA_DIR` is an
alternative; the legacy `MEATSHELL_DATA_DIR` variable remains a fallback.
An explicit directory is pinned for the process, uses its own `secret.key` and
logs, and never restores or mirrors another profile or uses the shared OS
keyring. Make this directory private to the service account. The normal GUI
profile selection is unchanged when neither option nor variable is supplied.

Do not run the GUI and a headless service against the same profile at the same
time. Use a separate `--data-dir` for the service, and close any GUI using a
profile before importing into it. The importer detects stale state during its
own transaction, but existing GUI/background writers do not all perform that
check: a later save from an already-open GUI can replace newer session state.
Directory selection does not enforce exclusive use or provide a cross-process
profile lock. Pointing `--data-dir` at an installed profile backed by the OS
keyring is unsupported. Startup checks reject keyring markers, encrypted values
without a local key, and values that the local key cannot decrypt before creating
keys, logs or database sidecars. Export from the original application and import
that portable export into the service's separate directory instead. Plaintext
legacy JSON can still initialize a new isolated profile. The check inspects a
private temporary snapshot of SQLite and its WAL so that even a rejected WAL
profile gets no new files. Repeated profile loads incur this read/copy overhead;
the check is not a replacement for exclusive operational use of the directory.

Session listing and inspection return connection metadata and boolean credential
presence only. They omit passwords, private-key contents/paths, proxy URLs,
trigger responses and notes. Import results contain counts and, when necessary,
non-sensitive compatibility warnings.

## Supported inputs and conflict policy

- Original MeatShell/XenTerm v1 portable JSON exports
- Native legacy `sessions.json`, reading only the required `sessions` array
- FinalShell connection JSON accepted by XenTerm's existing FinalShell decoder

Global source settings are never imported. Native `enc:v1:` values require the
matching local profile key; foreign, malformed or unknown encrypted values and
OS-keyring placeholders are rejected. Use a portable export to move credentials
between profiles or computers. Unsupported transport kinds are rejected instead
of silently changing their meaning. A batch containing RDP sessions is rejected
in full because XenTerm does not implement that transport.

The public MeatShell branch also exports `session_log`, `allow_secret_reveal`
and `rdp_domain`/`rdp_width`/`rdp_height`/`rdp_fullscreen` metadata. This XenTerm
version does not implement those settings. Preview and apply explicitly return
`warnings` naming these known unsupported fields and the number of affected
entries; no field values or session identifiers are included. Unknown optional
fields produce a generic warning without echoing their names or values. These
settings are not applied, and importing never enables secret reveal. Review the
preview and retain your original export if these preferences matter to you.

Imports are **append-only**. An equivalent complete profile, including its
credentials and resolved jump route, is skipped. Profiles sharing an endpoint
but having different names, groups, credentials, proxies or routes remain
separate. Every new session receives a fresh ID. Both legacy `jump_session_id`
and ordered `jump_session_ids` references are remapped, including forward
references and references to skipped duplicates. There is no overwrite switch.

The whole batch is validated first. Unknown export versions, duplicate/empty
source IDs, invalid hosts/ports, malformed paths and missing/cyclic/oversized
jump routes fail without adding any sessions. File input must be UTF-8 JSON, a
regular file, and at most 16 MiB. Key/device/working-directory paths remain
portable references: control characters are rejected, but importing never opens
those paths or requires files from a different computer to exist locally. Check
key availability on the destination before connecting.

`--dry-run` reports the same added/skipped counts without applying the batch.
Applying inserts the new rows in one SQLite transaction. Existing session rows,
settings and credentials are not rewritten. A failed transaction leaves the
in-memory store unchanged; a concurrent change or unsaved editor change requires
reloading or saving/discarding those changes before retrying.

## MCP

`import_sessions` takes `local_path` (a JSON file on the server computer) and an
optional boolean `dry_run`, which defaults to `true`. Both preview and apply
require MCP and its file-transfer permission to be enabled. Applying also
requires starting the process with `--allow-config-import`:

```sh
xenterm --data-dir /absolute/private/profile mcp serve --allow-config-import
```

```json
{"name":"import_sessions","arguments":{"local_path":"/private/export.json","dry_run":true}}
```

The tool returns `added`, `skipped` and `dry_run`, plus `warnings` when unsupported
metadata is present, with no source contents or credentials. It neither connects to an imported host nor trusts its host key.
Use the normal host-trust workflow before executing commands or transferring
files. List/get retain the existing MCP saved-credentials permission gate.

## Verification

```sh
cargo test --no-default-features --features headless config::config::import
cargo test --no-default-features --features headless config::config::profile
python tests/config_import_e2e.py --exe target/debug/xenterm
```

All fixtures are synthetic. Regression coverage includes an original public
MeatShell ciphertext fixture, export/import round trips across different profile
keys, both jump-reference formats, aliases, preview, malformed inputs, explicit
MCP permissions, rollback after a second-row SQLite failure, stale caches, and
credential redaction. CRUD editing is still performed through the session editor;
the CLI/MCP management surface provides import, list and inspect.
