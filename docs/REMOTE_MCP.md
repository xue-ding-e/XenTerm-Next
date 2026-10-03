# Authenticated remote MCP (Linux or Windows)

`xenterm mcp serve` remains the existing local stdio transport. To run a
remote service, use the **new, opt-in** `--http-config` mode. It serves the
standard MCP Streamable HTTP protocol at the configured resource path, using the official Rust MCP
SDK. Both desktop and `--no-default-features --features headless` builds include this mode. Running
without arguments still starts the GUI in the default desktop build.

This is an **OAuth resource server**, not an identity provider. It verifies
RS256 access tokens from an established external OAuth 2.1 / OIDC provider.
You must configure that provider and an HTTPS reverse proxy before connecting
ChatGPT/dot. No account, token, signing key, tunnel, public listener or production
profile is created by installing the release. Local synthetic integration tests
do not constitute an end-to-end ChatGPT login test.

## 1. Prepare one deliberately selected profile

Run under a dedicated unprivileged OS account. Create a private profile directory
(mode 0700 on Linux) and select it explicitly with `--data-dir` or
`XENTERM_DATA_DIR`. HTTP mode refuses to use an implicit default/sidecar profile. A headless build
requires an explicit profile for CLI, stdio MCP and `--config-info` too (except
`--version`), before it opens logging or configuration files. Headless builds do
not use the desktop OS keyring. Export from the desktop and import into a separate
service profile; do not point headless at an installed desktop profile whose key
may live only in the OS keyring. Current profile writers reject stale snapshots
instead of overwriting newer commits; keep pending edits and reload on a conflict.
Use upgraded writers only: older versions can still overwrite newer saves, and
external replacement of database/key files is unsupported while the profile is
open. The explicit path is a deliberate choice of storage, not a conversion of
desktop keyring credentials. See [configuration consistency](config-import.md)
for recovery instructions.
You can import an export using the existing CLI, or configure that profile in
the desktop application. Never place profile files, credentials or SSH keys in
web roots or release packages.

Existing profile settings remain authoritative:

- `mcp_enabled` must be enabled
- Saved credential use requires `mcp_use_saved_credentials`
- Arbitrary SSH commands require `mcp_allow_commands`
- SFTP transfers/imports require `mcp_allow_file_transfers`
- Applying configuration imports additionally requires startup flag
  `--allow-config-import` (before `--http-config`)
- Unknown/changed SSH host keys fail closed. Seed verified host trust as described
  below before using SSH/SFTP through the headless service; never bypass this check

The command permission permits arbitrary non-interactive commands on saved SSH
sessions, under each session's remote account. Existing risky-command approval
rules still apply; without an operator approving through the authorized workflow,
headless requests time out or are denied. OAuth never approves those prompts.

The file permission is broad: clients may read/write remote paths accessible to
the saved SSH account, upload local files readable by the service OS account, and
download into its writable local directories. There is no per-directory sandbox
or per-subject path allowlist in this adapter. Import previews also read the
specified local export file; applying an import additionally needs the explicit
startup capability. Use a dedicated OS account, private profile and systemd
filesystem restrictions to narrow the accessible files. Do not enable these gates
for untrusted users or profiles containing unrelated servers or credentials.

OAuth authorization adds an outer boundary; it does **not** enable these gates.
Authorized subjects all access the same selected profile and the OS user's
filesystem permissions. This is a **single-owner / trusted-operator** service,
not a multi-tenant SSH hosting service. For separate users/data, run separate
OS accounts, profiles and service instances. A subject allowlist is mandatory.

### Seed verified SSH host trust before the first connection

A session export/import **does not include SSH host trust**. A fresh profile is
not ready to connect merely because its sessions and credentials were imported.
The CLI/MCP service cannot approve the desktop host-key dialog: unknown or changed
keys are rejected. Complete this one-time operator step before starting it.

The trust file is `known_hosts` directly inside the explicitly selected private
`--data-dir` (alongside `sessions.db`). It does not read `~/.ssh/known_hosts`.
Use either of these supported approaches:

1. Copy XenTerm's own `known_hosts` from a trusted profile whose server keys you
   previously verified, using a secure local/admin transfer. Select only the
   verified hosts needed by this service. Session host strings and ports must
   still match exactly. A desktop-capable build can establish this trust first:
   select that profile, compare every displayed SHA256 fingerprint against an
   independently trusted server-console/admin record, then approve it. Do not
   approve an unfamiliar key merely to make the connection work.
2. Without a GUI, obtain the server's **public host key** through its trusted
   console or authenticated administrator channel. For example, on that server,
   inspect `/etc/ssh/ssh_host_ed25519_key.pub` and its fingerprint with
   `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub -E sha256`. Use the actual public
   host key configured by that server's SSH daemon; key paths and types can vary.
   Transfer only the `.pub` key, compare its SHA256 fingerprint against the
   independently verified console/admin value, and add an entry as shown below.
   The server's private host key, your SSH login private key, and an OAuth signing
   key are **never** entries in this file. Unverified `ssh-keyscan` output is not a
   trust source; do not pipe it into the store or blindly accept its fingerprint.

**XenTerm uses its own exact-string format**, not the OpenSSH `known_hosts`
format. Each entry is exactly:

```text
<literal session.host>:<decimal port> <key-type> <base64-public-host-key>
```

Always include the port, including `:22`. Use the precise saved session host
string: a DNS name and its IP address are different entries, and spelling/case is
not normalized. Do not add OpenSSH-style `[host]:port` brackets, hashed hostnames,
comma-separated host lists, wildcard patterns, markers, or a trailing key comment.
For an unbracketed IPv6 session host `2001:db8::10` on port 22, the identifier is
`2001:db8::10:22`. Blank lines and whole-line `#` comments are allowed. The stored
key type and base64 must match the key that the server actually presents.

For example, **after** verifying a single-line public key saved locally as
`verified-bastion-host-key.pub`, run the following as the dedicated service OS
account. Replace the example directory and exact session host/port. Stop the
service/desktop while editing, and preserve a private backup of an existing trust
file before modifying it. These commands append a verified new entry and strip
any `.pub` comment; they do not establish trust or verify identity for you.

```sh
PROFILE=/var/lib/xenterm/profile
umask 077
mkdir -p "$PROFILE"
chmod 700 "$PROFILE"
# Compare the displayed SHA256 fingerprint with the trusted console/admin value.
ssh-keygen -lf verified-bastion-host-key.pub -E sha256
# Continue only after that comparison succeeds and you approve the identity.
touch "$PROFILE/known_hosts"
chmod 600 "$PROFILE/known_hosts"
awk -v id='bastion.example.com:22' \
  'NR == 1 { printf "%s %s %s\n", id, $1, $2 }' \
  verified-bastion-host-key.pub >> "$PROFILE/known_hosts"
```

Keep the profile and file owned by the service account, with directory mode 0700
and file mode 0600 on Linux (equivalent private ACLs on Windows). Restrict backup
copies too. Seed **every jump host and the final target**, each under its own saved
host/port, even when the target is reachable only through a jump. Keep the final
target's saved hostname; do not substitute localhost or the bastion address. Repeat for any
verified host-key type that can be negotiated; a different key is not implicitly
trusted. Start the service only after completing this setup. Unknown/changed keys
must continue to fail closed: investigate a change out of band, then deliberately
replace the old host entry after re-verification. Never erase the store, enable
accept-all behavior, or silently append an unverified replacement to bypass a
failure. No real profile or trust file is supplied in this release.

## 2. Configure the external authorization server

Use an established provider with authorization-code + PKCE S256, issuer
metadata discovery and a ChatGPT-compatible client registration mechanism
(CIMD, DCR, or a predefined client). Configure the exact redirect URI displayed
by the ChatGPT connection setup. Configure it to issue **RS256 access tokens**
with:

- `iss`: exact configured HTTPS issuer, including any trailing slash
- `aud`: exact canonical resource URL, e.g. `https://shell.example.com/xenterm/mcp`
- `sub`: the explicitly allowed account identifier
- `exp`: a mandatory expiry timestamp; `nbf` is checked when present
- `scope`: a space-separated string containing `xenterm:mcp`

The provider must honor the OAuth `resource` parameter. Do not use ID tokens,
shared static bearer tokens, a client secret, or SSH credentials as access tokens.
Static bearer authentication is not implemented by this HTTP adapter.

Download the provider's **public** JWKS through a trusted, TLS-verified admin
workflow into a local `public-jwks.json` file. The service deliberately does not
fetch URLs supplied by clients. Only RSA/RS256 signature-verification keys with
unique nonempty `kid` values are usable; private/symmetric key material is rejected.
Protect the file against untrusted modification. On rotation, replace it
atomically with the provider's current public keys and restart the service.
For overlap, retain both old and new public keys until old access tokens expire.
Key removal, subject/scope changes and revocation require restarting this service;
JWT revocation/introspection is not supported. Use short token lifetimes.

See the official [MCP authorization specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization)
and [OpenAI authentication requirements](https://developers.openai.com/plugins/build/auth).

## 3. Create the service configuration

Example `http.json` (all hostnames and subjects below are placeholders):

```json
{
  "bind": "127.0.0.1:8765",
  "allowed_origins": [],
  "oauth": {
    "issuer": "https://identity.example.com/",
    "resource": "https://shell.example.com/xenterm/mcp",
    "jwks_file": "public-jwks.json",
    "required_scope": "xenterm:mcp",
    "allowed_subjects": ["REPLACE_WITH_YOUR_PROVIDER_SUBJECT"]
  }
}
```

`jwks_file` is relative to the configuration file (or absolute). The default bind
is loopback `127.0.0.1:8765`. Authentication is mandatory for every MCP request,
even loopback requests; there is no unauthenticated/public-bind flag. Non-loopback binds are rejected. The application speaks HTTP internally; keep
the reverse proxy on the same host.

Every request with an `Origin` header must match an explicitly listed HTTPS
origin. With the empty default, all browser-origin requests are rejected; normal
server-to-server MCP requests do not need an Origin. Allowed browser origins receive explicit CORS/preflight responses, including
access to authentication challenges and the session header. MCP CORS never uses
a wildcard; public OAuth discovery metadata permits cross-origin reads.
Host must match the configured resource authority or bind address. The reverse
proxy must preserve the public Host header. Forwarded headers never grant trust.

Start (example paths are operator-selected, not shipped profiles):

```sh
xenterm --data-dir /var/lib/xenterm/profile mcp serve --http-config /etc/xenterm/http.json
```

No access tokens/passwords belong in command-line arguments or logs. Clients
send access tokens solely using the HTTP `Authorization: Bearer ...` header.

## 4. Terminate HTTPS in a reverse proxy

Reuse the existing OpenResty/Nginx HTTPS virtual host. Do not install a second
proxy or change other applications' routes. The exact `location` blocks below
allow XenTerm and another service to share one domain with distinct paths.
Keep your current certificate configuration and add only these locations:

```nginx
# Inside the existing HTTPS server {} for shell.example.com
location = /xenterm/mcp {
    proxy_pass http://127.0.0.1:8765;
    proxy_http_version 1.1;
    proxy_set_header Host $http_host;
    proxy_set_header Connection "";
    proxy_set_header Authorization $http_authorization;
    proxy_buffering off;
    proxy_request_buffering off;
    proxy_read_timeout 310s;
    proxy_send_timeout 10s;
    client_body_timeout 10s;
    client_max_body_size 1m;
    access_log off;
}
location = /.well-known/oauth-protected-resource/xenterm/mcp {
    proxy_pass http://127.0.0.1:8765;
    proxy_set_header Host $http_host;
    access_log off;
}
```

Do not add a URI suffix to `proxy_pass`: preserve the complete public path.
For another service at `/meatshell/mcp`, configure its own loopback port and
`/.well-known/oauth-protected-resource/meatshell/mcp` location. Keep OAuth audiences
and selected profiles distinct. The public discovery challenge names the full
path-specific metadata URL. Do not route all `/.well-known/*` to one application,
and avoid the optional root discovery alias on a shared domain.

Preserve Authorization and Host; forwarded headers never authorize requests.
Do not log bearer tokens or request bodies. Disable response and request
buffering so SSE/cancellation and early 401/408/413 responses are not delayed.
Set server-level TLS, header and idle deadlines/rate limits according to your
existing site's policy. Test the combined configuration with your installed
`openresty -t`/`nginx -t`, then have the operator reload using its existing service
manager. This repository does not edit proxy configuration or reload services.

The bundled [OpenResty example](../packaging/remote-mcp/openresty.conf.example)
and [systemd unit](../packaging/remote-mcp/xenterm-mcp.service) are templates only.
Nginx and OpenResty share these standard proxy directives; run the local TLS
fixture with your actual OpenResty executable for deployment-specific validation.

TLS ends at the proxy in this deployment. The application does not itself accept
HTTPS or gRPC. Keep the cleartext hop on loopback. HTTPS already encrypts standard MCP traffic and does not require a
custom gRPC transport.

Optional systemd unit (adjust all paths/user names; this does not install itself):

```ini
[Unit]
Description=XenTerm authenticated MCP
After=network-online.target
[Service]
User=xenterm
Group=xenterm
ExecStart=/opt/xenterm/xenterm --data-dir /var/lib/xenterm/profile mcp serve --http-config /etc/xenterm/http.json
WorkingDirectory=/var/lib/xenterm
UMask=0077
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ReadWritePaths=/var/lib/xenterm
Restart=on-failure
TimeoutStopSec=15
[Install]
WantedBy=multi-user.target
```

Filesystem hardening may intentionally prevent upload/download/import outside the
profile. Grant only the paths this service actually needs. Never run as root.
On Windows, use equivalent private directory ACLs and a service manager. Native
Windows packaging/build results are stated separately in each release; a Linux
runtime test is not Windows runtime verification.

## 5. Connect and verify

The public URL is `https://shell.example.com/xenterm/mcp`. Unauthenticated endpoint
requests return 401 with a `WWW-Authenticate` discovery challenge. Public metadata
at `/.well-known/oauth-protected-resource/xenterm/mcp` (also the root well-known path)
contains only resource/issuer/scope information, never sessions or credentials.

Use MCP Inspector, then [ChatGPT's connection setup](https://developers.openai.com/plugins/deploy/connect-chatgpt).
The external provider, redirect URI, scopes, audience, HTTPS certificate and
public reachability must work together. This release has no preconfigured dot
plugin or production deployment. Do not claim it is connected until that end-to-end
flow succeeds in the target environment.

### Reproduce local HTTP and HTTPS verification

Install Python `cryptography` and `paramiko`, and obtain Nginx/OpenResty from its official
distribution. Point `--exe` at a built binary or an independently downloaded,
checksum-verified release executable:

```sh
python tests/remote_mcp_e2e.py --exe /path/to/xenterm
python tests/remote_mcp_e2e.py --exe /path/to/xenterm --nginx /path/to/nginx
python tests/remote_mcp_e2e.py --exe /path/to/xenterm --nginx /path/to/nginx --protocol-version 2025-11-25
```

The default protocol revision is `2025-06-18`; the optional revision is asserted
against the negotiated initialization result. These tests do not claim support
for the changed `2026-07-28` lifecycle.

HTTPS mode repeats the complete HTTP authentication/session/cancellation suite
through a real Nginx/OpenResty reverse proxy. It generates a one-hour test CA and localhost
certificate at runtime, trusts the CA only in that Python client's SSL context,
and verifies TLS 1.2/1.3 plus rejection of an untrusted CA and wrong hostname.
The external HTTPS resource audience and discovery URL must survive proxying;
wrong issuer/audience/signature/expiry/scope/subject, disallowed Host/Origin,
oversized and stalled bodies, permission gates, explicit cancellation, token
expiry, session deletion and active SFTP-upload cancellation are exercised.
Both listeners are loopback-only. The fixture has no certificate issuer or public listener. No system CA store,
real profile, public tunnel or production account is touched. Certificates,
private keys and fixture state are generated under temporary storage and are
never committed or packaged.

A successful run proves the tested executable works behind locally verified HTTPS. It
does **not** prove public DNS/ingress, a publicly trusted certificate, external
OAuth authorization-code/PKCE login, or a ChatGPT/dot connection. Verify those
separately in the intended deployment before calling the service connected.

## Runtime limits and cancellation

- RS256 signature, issuer, audience, expiry/not-before, scope and subject checked
  on every request; invalid tokens return 401, insufficient permission 403
- MCP session IDs bind to the authenticated subject of the configured issuer;
  IDs belonging to another subject/unknown/expired IDs all return 404
- At most 64 protocol sessions, 15-minute absolute lifetime, SDK cleanup;
  clients must reinitialize after expiry. Refreshing an OAuth token for the same
  subject can continue a live session; subject changes cannot
- 1 MiB HTTP body limit, 5-second body-read deadline, 10-second protocol-header
  response deadline, 16 tool calls/response streams; control notifications retain
  separate admission capacity
- Tool calls and response streams end after at most 300 seconds or token expiry,
  whichever occurs first. Existing per-operation limits still apply
- Use `notifications/cancelled` to cancel an in-flight request, and HTTP DELETE to
  close its MCP session. Cancellation stops local SSH/SFTP work; it cannot undo
  commands already executed, bytes already written, or remote jobs detached by a
  command. A dropped network connection is not proof an action was undone
- Results use POST SSE response streams. Standalone GET streams/resumption are
  deliberately unsupported (405); completed-response replay caching is disabled
- SIGTERM/Ctrl-C requests graceful shutdown. HTTP configuration/JWKS are loaded
  once per process; restart after changes. Persisted MCP capability settings are
  checked on every request and tool call

## Reproducible checks

```sh
cargo test --no-default-features --features headless --bin xenterm
cargo build --no-default-features --features headless
python3 tests/stdio_mcp_e2e.py --exe target/debug/xenterm
python3 tests/remote_mcp_e2e.py --exe target/debug/xenterm
python3 tests/config_import_e2e.py --exe target/debug/xenterm
# Synthetic HTTP and SSH/SFTP tests require Python cryptography + paramiko:
python3 tests/ssh_jump_chain_e2e.py --exe target/debug/xenterm
python3 tests/ssh_jump_chain_e2e.py --exe target/debug/xenterm --stage-timeouts
cargo check                         # default desktop source compatibility
```

Tests generate synthetic RSA signing keys in memory, write only their public
JWKS, create disposable profiles and connect solely to loopback fixtures.
Headless builds contain CLI/MCP functionality and no GUI; the default desktop
build keeps the GUI, CLI, stdio MCP and HTTP MCP in the same executable.
Remote HTTP is implemented with the official rmcp SDK. The public MeatShell
authenticated-remote-mcp branch (890792d) was the adapter/test baseline; this
port uses XenTerm's GPUI feature split and SQLite profile store.
