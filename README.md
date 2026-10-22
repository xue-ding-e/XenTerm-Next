> **XenTerm Next (XenTerm-Next)** is an independent SSH, SFTP and terminal project created and maintained by [xue-ding-e](https://github.com/xue-ding-e), building on [ixbaicn/XenTerm](https://github.com/ixbaicn/XenTerm). We develop our own next-generation terminal experience, including Next-specific enhancements and reviewed community fixes that may not yet be in upstream. It is not an official XenTerm release or a promise of upstream’s next version; upstream authorship, copyright and the MIT license are preserved.
>
> Our development line is `main`. `upstream-main` preserves official history; a daily read-only workflow checks drift, and verified mirror fast-forwards are handled separately. Upstream updates never automatically replace Next features. See the [branch policy](CONTRIBUTING.md#independent-repository-branch-policy-xue-ding-exenterm-next).

<p align="center">
  <b>English</b> · <a href="README.zh.md">简体中文</a>
</p>

<h1 align="center">XenTerm Next</h1>

<p align="center">
  <em>A lightweight SSH / SFTP / terminal client for Windows, macOS and Linux — written in Rust, no runtime, no JVM.</em>
</p>

<p align="center">
  <a href="https://github.com/xue-ding-e/XenTerm-Next/stargazers"><img src="https://img.shields.io/github/stars/xue-ding-e/XenTerm-Next?style=flat-square&label=Stars&color=2ea043" alt="Stars"></a>
  <a href="https://github.com/xue-ding-e/XenTerm-Next/releases"><img src="https://img.shields.io/github/v/release/xue-ding-e/XenTerm-Next?style=flat-square&label=Release&color=00b8d4" alt="Release"></a>
  <a href="https://github.com/xue-ding-e/XenTerm-Next/commits/main"><img src="https://img.shields.io/github/last-commit/xue-ding-e/XenTerm-Next?style=flat-square&label=Last%20commit" alt="Last commit"></a>
  <a href="#license"><img src="https://img.shields.io/badge/License-MIT-blue?style=flat-square" alt="License: MIT"></a>
</p>

<p align="center">
  <img src="assets/xt.jpg" alt="XenTerm — SSH Terminal · SFTP · System Monitor" width="820">
</p>

---

## What is XenTerm Next

**XenTerm Next** is the independent terminal project by **xue-ding-e**, for people who want a native Rust desktop client and scriptable CLI / MCP workflows in one codebase. “Next” describes our own development direction: a next-generation experience built on XenTerm, with independently developed enhancements and carefully integrated community contributions. It does not designate the official upstream’s next release.

### Why choose Next?

Next ships its own development and release line. Examples already implemented here, beyond [upstream `f25f6af`](https://github.com/ixbaicn/XenTerm/tree/f25f6afcd82429d2286526c33c9ea257cb34809f) as checked on **2026-10-16**:

- **Private, no-overwrite GUI session exports**: the GUI keeps the hardened export policy and reports an existing destination instead of falling back to a plain overwrite. See the [implementation](src/ui/impls/shell.rs) and [regression tests](src/ui/impls/file_dialog_tests.rs).
- **Traceable Next desktop previews**: our [beta release pipeline](.github/workflows/next-beta.yml) publishes cross-platform packages tied to a successful main CI run, with source SHA, build metadata and SHA256 checksums. [Download the current preview](https://github.com/xue-ding-e/XenTerm-Next/releases/tag/beta).
- **Dedicated headless server packaging**: our [headless build and release entry point](docs/headless-server.md) reuses the existing shared core, checks the Ubuntu 22.04 ABI/runtime dependencies and can produce fixed-source releases. The shared CLI/MCP core builds on upstream work; the Next packaging is an additional workflow, not a separate implementation.
- **Installer build tooling and lifecycle tests**: [Windows MSI](wix/main.wxs) and [macOS packaging](packaging/macos) include optional integrations with [Windows](tests/windows_installer_test.py) and [macOS](tests/macos_installer_test.py) regression coverage. These are installer build capabilities; the current rolling beta downloads are portable archives.

Upstream may adopt or independently add similar changes later. Other improvements here include reviewed contributions from the XenTerm community; their authors retain credit. “Available in Next” does not mean every feature was originally authored by us, nor that every Next change is awaiting an upstream PR.

If these workflows help you, **[star XenTerm Next](https://github.com/xue-ding-e/XenTerm-Next)** to support the project and help other developers find it. Try the preview, report reproducible issues and follow our independent development here.

XenTerm is a native desktop terminal client that keeps the workflow people like about
FinalShell — a resource-monitor sidebar, session management, tabbed terminals, an SFTP
panel that follows your shell — while dropping the part they do not: several hundred
megabytes of JVM. It is a single Rust binary with the interface built directly on
[GPUI](https://gpui.rs), so there is no interpreter, no web runtime and no garbage
collector in the process.

It is a client, not a re-implementation of a shell: SSH, Telnet and serial sessions talk
to whatever runs on the other end, and local shells run in a real PTY / ConPTY.

| | |
| --- | --- |
| **Sessions** | SSH, local shell (PowerShell / cmd / WSL / `$SHELL`), Telnet, serial |
| **Transfers** | SFTP file panel, batch download, ZMODEM in the terminal, WebDAV sync |
| **Terminals** | Full VT/ANSI, mouse tracking, DEC line drawing, colour emoji, split panes |
| **Monitoring** | Local + remote CPU / memory / swap / disk / network, remote process list |
| **Automation** | A `cli` for scripts and CI, and an MCP server for AI clients |
| **Interface** | GPUI-rendered, English / Simplified Chinese, collapsible docked panels |

## The interface

The window is a navigation rail and one page at a time — three pages, each owning its own
state and reporting only what crosses its border to the shell:

- **Terminal** — the working surface. A tab strip, nestable horizontal split panes, a
  floating command line with the quick-command and command-history popovers, the resource
  sidebar, and the file panel docked to the bottom or the right.
- **Sessions** — the saved connections and the built-in local shells, with groups,
  import/export and the session editor.
- **Settings** — everything in the table further down.

Two monitors are separate windows rather than panels, because they are tables you keep
open beside the terminal while you work in it: the **process monitor** and the
**system information** window. The transfer list, the tunnel list and the file panel are
panels, so they stay attached to the window whose sessions they belong to.

## Features

### Sessions and connection

- **Four session types** — SSH, local shell, Telnet and serial, in the same tabbed UI.
- **Local shells out of the box** — PowerShell, `cmd.exe` and every configured WSL
  distribution on Windows; `$SHELL` elsewhere. Windows shells start in UTF-8.
- **Ordered SSH bastions** — add saved SSH sessions on the session editor’s SSH bastions page, move any hop earlier/later, or remove it. The displayed order is outermost first; each hop uses its own credentials. Existing `jump_session_id` configurations still work, including nested legacy routes.
- **SSH authentication** — password, private key, passphrase-protected key, and
  keyboard-interactive for 2FA / OTP prompts.
- **PuTTY `.ppk` keys** — PPK v2/v3 is decrypted and verified in memory; no `puttygen`
  round-trip.
- **Outbound proxies** — SOCKS5 (`socks5://`, `socks5h://`) and HTTP/HTTPS CONNECT, set
  per session or picked up from `ALL_PROXY`. Telnet uses the same plumbing.
- **Serial** — configurable baud rate, data/stop bits, parity and flow control.
- **Telnet** with an RFC 854/855 option state machine (SGA + NAWS window resizing).
- **Triggers** — expect/send rules for interactive logins, each with its own response.
- **Session groups** — create, rename and delete folders, plus per-session notes.
- **Host key verification** — trust on first use with a SHA-256 fingerprint prompt, and a
  hard warning if a key later changes.

> Session passwords and keys can also be entered without the GUI, and the config supports
> per-session encodings. Ordered jump routes can be edited in the GUI. Imported
> `jump_session_ids` are ordered from the outermost bastion toward the target;
> when present, they override legacy `jump_session_id` links. Invalid, repeated,
> missing, non-SSH or more-than-16-hop routes fail before network activity.

### Terminal

- **Full VT/ANSI emulation** — `htop`, `btop`, `vim`, `tmux` and friends render correctly
  full-screen, over a 100,000-line scrollback.
- **Mouse tracking** — SGR and X10 mouse reports are forwarded to TUI applications that
  ask for them, so `htop` and `mc` respond to clicks.
- **DEC line drawing** — the VT100 special-graphics charset, so `dialog` and `mc` frames
  draw properly even in UTF-8 mode.
- **Colour emoji** — skin tones, flags and ZWJ sequences render in colour, from Twemoji
  images embedded in the binary.
- **Character encodings** — the terminal decodes per-session charsets (UTF-8 by default,
  GBK and others) with a streaming decoder, so a multi-byte character split across packets
  still decodes.
- **Output highlighting** — colour log levels (with a DevOps preset) and your own rules,
  without disturbing output that already carries its own colours.
- **Split panes** — nestable horizontal splits, from the tab's context menu; drag the
  splitter to resize. Tabs reorder by dragging within the strip or from the same menu.
- **Find in the scrollback**, paging keys that scroll the scrollback on the normal screen
  and reach the program on the alternate screen as they should.
- **Paste protection** — multi-line pastes can require confirmation, and the extra paste
  shortcuts (`Ctrl+Alt+V`, `Shift+Insert`) can be turned off.
- **Font and cursor** — any installed monospace family plus a bundled one, size, line
  spacing, bold, and block / bar / underline cursors with a custom colour.

### Static terminal wallpaper (opt-in)

Open **Settings → Wallpaper**, import a PNG, JPEG, WebP or BMP, and enable the
preview. **Apply** saves the complete draft; **Cancel changes** restores the saved
settings, and **Reset draft to defaults** does not save. Profiles upgrading from
before this feature start disabled, even with choices from the retired interface.

- Cover, Contain, Stretch, Original and Tile; nine anchor presets, offsets and zoom.
- Non-destructive crop, arbitrary rotation, quarter-turn shortcuts and both flips;
  EXIF orientation is applied before editing.
- Separate image opacity and dark readability overlay, brightness, saturation and
  blur. These effects do not change text, explicit cell backgrounds, selections or
  cursor rendering.
- **Each terminal pane** repeats the layout independently. **Continuous terminal
  canvas** aligns terminal images in window coordinates across split panes; it does
  not make toolbars, sidebars, settings or dialogs transparent. The editor previews
  one canvas. There is no automatic theme recolouring.

Imports are local, immutable content-hash copies in the profile's `wallpapers/`
folder. Original bytes, including metadata, are preserved; cancelling a draft may
leave its imported original there. When moving a complete profile, carry this
folder alongside `sessions.db`; session exports and WebDAV do not bundle images.
Missing or invalid assets fall back to the terminal colour and report an error in
the editor. No remote image URLs, animated backgrounds, SVG or video are supported.

Limits: 32 MiB per imported file, 33,554,432 decoded pixels and 16,384 pixels per
source edge. Processed textures are limited to 4,194,304 pixels and a 4,096-pixel
edge; Original layout retains the logical source dimensions after downsampling.
Decode/transform work is debounced, serialized off the UI thread and cached;
these per-image limits are not a total application-memory cap.

The control vocabulary draws on the official documentation for
[Windows Terminal](https://learn.microsoft.com/en-us/windows/terminal/customize-settings/profile-appearance#background-images-and-icons),
[IntelliJ IDEA](https://www.jetbrains.com/help/idea/setting-background-image.html)
and [WezTerm](https://wezterm.org/config/lua/config/background.html).
No wallpaper artwork or implementation code is copied from these projects.

Verification commands: `cargo check --locked`, `cargo test --locked wallpaper`,
`cargo test --locked`, and
`cargo test --locked --no-default-features --features headless`.
The implementation has Linux native GUI coverage; Windows and macOS native GUI
behaviour still require platform testing.

### Files

- **SFTP panel** that follows the terminal's `cd`, with a lazy-loading directory tree.
- **Upload and download** single files or whole directory trees, from the toolbar or the
  file panel's context menu.
- **Batch download** — the selection is tarred on the remote side, pulled down as one
  file, and the remote temp file is cleaned up.
- **Built-in viewer / editor** — a plain text editor for small files, read-only when the
  file is; opening a file in your own editor re-uploads it when you save.
- **File operations** — rename, delete, create file or directory, and `chmod` that keeps
  the setuid / setgid / sticky bits.
- **Copy between sessions** — move remote files from one SFTP session into another.
- **Conflict handling** — replace or keep both, per download.
- **ZMODEM** in the terminal: `sz` on the remote downloads to your Downloads folder; `rz`
  opens a local file picker and uploads over the existing PTY.
- **WebDAV sync** — upload and download the connection list by hand, with an option to
  accept self-signed certificates.

### Monitoring

- **Local system panel** — CPU, memory, swap, network throughput and per-filesystem usage,
  with sparklines.
- **Remote monitoring over SSH** — the same metrics read from `/proc` and `df` on the
  server, refreshed every two seconds.
- **Remote process list** — CPU-sorted, with copyable PIDs and the ability to terminate a
  process you own (signalling another user's process asks first).
- **System information window** — OS, kernel, architecture, hostname, CPU, memory,
  filesystems and GPU.

### Automation

- **CLI** — one-shot SSH commands, file listing / reading / transfer, and session
  inspection, with human-readable output or `--json`.
- **MCP server** — exposes the same saved sessions to an MCP-capable AI client over local
  stdio: session lookup, remote commands, directory listing, bounded text reads, uploads
  and downloads. Each capability is behind its own permission switch, and credentials are
  never returned to the client.

## Screenshots

<p align="center">
  <img src="docs/screenshots/01-welcome.png" alt="XenTerm on the Terminal page with nothing connected" width="820"><br>
  <em>The landing view: the navigation rail, the status panel on the left, and Connect / New session / Import</em>
</p>

<p align="center">
  <img src="docs/screenshots/02-terminal-htop.png" alt="A local PowerShell session with the file panel docked below" width="820"><br>
  <em>A local PowerShell session in a tab, with the file panel docked underneath</em>
</p>

## Install

This independent repository has not published installation packages yet. Build the enhanced `main` from source using [Quick start](#quick-start); future packages will appear in [this repository’s Releases](https://github.com/xue-ding-e/XenTerm-Next/releases). The instructions below describe inherited Windows, macOS and Linux package formats, not packages already published here. [Official upstream releases](https://github.com/ixbaicn/XenTerm/releases) do not include all enhancements from this repository.

### Windows

- **Installer** — run `xenterm-<version>-windows-x86_64.msi` and choose an install
  location.
- **Portable** — unzip `xenterm-<version>-windows-x86_64.zip` and run `xenterm.exe`.

The MSI feature page initially selects PATH, desktop and Start menu integration;
each can be disabled. Upgrades between different numeric versions preserve feature
selection. Rebuilt previews with the same version number are not seamless upgrades;
uninstall the old package through Windows Installed apps before installing another
build with that version. PATH is maintained by
Windows Installer without replacing unrelated entries; reopen terminals afterward.

### macOS

The packaging workflow also builds an Apple `.pkg`: required `/Applications`
app plus an optional, initially selected CLI entry. It does not modify the Dock
or guess a desktop user. Each installer run starts with CLI selected; deselection
skips that component without removing an existing link, and does not yet remember
an earlier opt-out. See the [installer notes](packaging/macos/README.zh-CN.md).
Unsigned packages are not Developer ID signed or notarized; check the actual
assets provided by the version you download.


The download is a `.zip` containing `xenterm.app`:

```bash
# aarch64 = Apple Silicon, x86_64 = Intel
unzip xenterm-*-macos-*.zip

# Optional: move it into Applications (running it in place also works)
mv xenterm.app /Applications/

# Clear the quarantine flag, or macOS reports "xenterm is damaged and can't be opened"
xattr -dr com.apple.quarantine /Applications/xenterm.app

open /Applications/xenterm.app
```

If you did not move it, point both paths above at wherever the `.app` actually is.
Requires macOS 11 Big Sur or later.

### Linux

User script installs offer PATH, desktop and application-menu integration;
`--no-add-to-path`, `--no-desktop-shortcut` and `--no-start-menu` opt out, with
choices preserved on repeat installs. `--uninstall` with the original prefix
removes only unchanged managed files. A headless user without an XDG desktop gets
no desktop shortcut. System installs do not edit user shell profiles. Debian
packages keep package-manager semantics; Flatpak does not promise a host CLI.

| Package | Command |
| --- | --- |
| Debian / Ubuntu | `sudo apt install ./xenterm-*-linux-amd64.deb` |
| Fedora / others | `tar -xzf xenterm-*-linux-x86_64.tar.gz` |
| Flatpak | `flatpak install --user xenterm-*.flatpak` |
| AppImage | `chmod +x xenterm-*.AppImage && ./xenterm-*.AppImage` |
| Arch (AUR) | `yay -S xenterm-bin` |

The tarball runs directly:

```bash
tar -xzf xenterm-*-linux-x86_64.tar.gz
cd xenterm-*-linux-x86_64
./xenterm

# Optional: install the binary, icon and launcher under /usr/local
# sudo is requested only when the destination needs it; root does not need sudo
chmod +x install-linux.sh && ./install-linux.sh

# Or install for this user under ~/.local, without sudo
./install-linux.sh --user

# Or choose an installation directory (quote paths containing spaces)
./install-linux.sh --prefix "$HOME/Apps/XenTerm"
# Equivalent environment setting; an optional binary path still works
PREFIX="$HOME/Apps/XenTerm" ./install-linux.sh /path/to/xenterm
```

The default prefix is `/usr/local`. The installer creates `<prefix>/bin/xenterm`,
`<prefix>/share/applications/xenterm.desktop` and
`<prefix>/share/icons/hicolor/512x512/apps/xenterm.png`. Prefix precedence is
`--prefix` > `PREFIX` > the default (`$HOME/.local` with `--user`). `--user` never
invokes sudo and fails with a clear error if the selected destination is not writable.
Relative prefixes are resolved to absolute paths. Prefixes containing `=` or control
characters are rejected because they cannot form a safe desktop launcher path.

The launcher and its **New Window** action use the installed binary's absolute path,
so they do not depend on `PATH`. For command-line use, add `<prefix>/bin` to `PATH`.
For a custom prefix outside the desktop's search path, also add `<prefix>/share` to
`XDG_DATA_DIRS` in your desktop session environment, then log out and back in.

Other user launchers are preserved; the installer warns if one may override the new
installation. Rerunning the installer updates launchers marked as managed by it, and
also upgrades the original installer's unchanged launcher (an exact content match).
If an existing destination launcher is unmarked and differs from that original
template, installation stops before changing files. Review and move that customized
launcher before retrying.

> Requires glibc ≥ 2.35 (Ubuntu 22.04+ / Debian 12+). If you need an older baseline, the
> `-glibc228` tarballs are built against glibc 2.28 (Debian 10).

> On Wayland you may need to log out and back in once after installing the icon, so the
> shell picks it up.

## Quick start

### Rust toolchain support
Evidence: [Rust 1.99.1 Linux desktop CI](https://github.com/xue-ding-e/XenTerm-Next/actions/runs/39570649230/job/119123663957), [1.88 failure](https://github.com/xue-ding-e/XenTerm-Next/actions/runs/39698435848), [1.92 failure](https://github.com/xue-ding-e/XenTerm-Next/actions/runs/39709879675), [1.93 failure](https://github.com/xue-ding-e/XenTerm-Next/actions/runs/39711092025).


The package's Rust 1.88 declaration is a feature-specific baseline, not a promise
that every feature and platform supports Rust 1.88:

- Linux headless: Rust 1.88 is verified with `--locked --no-default-features --features headless`.
- Default Linux desktop: the current locked GUI dependencies declare a minimum of
  at least Rust 1.92. Actual checks reject Rust 1.88 and fail on Rust 1.92 because
  `gpui-pre-util` uses `slice::as_array`, stabilized in Rust 1.93. Rust 1.93 is
  not sufficient either: its check fails on `std::hint::cold_path` in `gpui-pre`,
  an API stabilized in Rust 1.95. Existing same-source Linux desktop CI passed
  on Rust 1.99.1; this is a verified toolchain, not proof of the minimum version.
- Windows and macOS desktop: existing CI verifies stable Rust, not a fixed minimum
  Rust version. Compilation does not replace native interaction testing.

Keep `Cargo.lock` and use `--locked`. Do not ignore Rust-version checks or downgrade
GPUI dependencies merely to force the headless minimum onto desktop builds.
Tracking and exact evidence: [issue #11](https://github.com/xue-ding-e/XenTerm-Next/issues/11).

```bash
git clone --branch main https://github.com/xue-ding-e/XenTerm-Next.git
cd XenTerm-Next
cargo run --release
```

On first launch XenTerm creates an empty session store. Add your first server with **New
Session**, or import one — `~/.ssh/config`, a FinalShell connection export, a previous
XenTerm export, or a pasted `host|port|user|password|name` list are all supported.

### Headless CLI / authenticated remote MCP

Build without the GPUI desktop libraries:

```sh
cargo build --locked --release --no-default-features --features headless
```

The default build still includes the complete GUI, CLI and stdio MCP. The opt-in
HTTP service uses external OAuth access tokens, a deliberately selected private
profile, and an existing HTTPS reverse proxy. See [remote MCP deployment and
verification](docs/REMOTE_MCP.md), including OpenResty routing for multiple MCP
services on one domain. No public listener, account or deployment is created by
the build.

### Building on Linux

`cargo run` needs the system development packages the GUI stack links against:

```bash
sudo apt update
sudo apt install -y --no-install-recommends \
  build-essential pkg-config cmake \
  libfontconfig1-dev libfreetype6-dev \
  libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libgl1-mesa-dev libegl1-mesa-dev libgtk-3-dev \
  libudev-dev
```

## Keyboard shortcuts

`⌘` denotes Command on macOS, where other platforms use `Ctrl`. The clipboard set is
deliberate: plain `Ctrl+C` stays `SIGINT`, which is the one shortcut a terminal must not
take away.

| Keys | Action |
| --- | --- |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab |
| `Ctrl+K` | Quick-connect palette |
| `Enter` on a dead session | Reconnect it |
| `Ctrl+Shift+C` | Copy selection |
| `Ctrl+V` / `Ctrl+Shift+V` | Paste |
| `Ctrl+Alt+V` / `Shift+Insert` | Extra paste shortcuts (switchable) |
| `Ctrl+F` | Find in the terminal; `Esc` closes the bar |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+0` | Zoom this session's font |
| `PageUp` / `Home` / `PageDown` / `End` | Scroll the scrollback (normal screen only) |

A second window is opened from the operating system's own entry point — the Windows
taskbar jump list, the macOS dock menu, or the desktop action on Linux.

## Settings

**Interface** — theme (system / dark / light), panel font size, and the interface
language.
**Terminal**

| Page | Settings |
| --- | --- |
| Font | Family (any installed monospace family, plus the bundled one), size, bold, line spacing |
| Cursor | Shape (block / bar / underline) and colour |
| Input | Confirm multi-line paste; the extra paste shortcuts; panel strip height |
| Highlight | Enable output highlighting, the preset (log levels / DevOps), and your own rules with add / enable / delete |

**Connections** — import `~/.ssh/config`, export every saved connection to a file.
**Pasting** (under Connections) — paste a `host|port|user|password|name` list and import it
in bulk.
**Files** — a default download directory and whether to always ask where to save.
**Sync** — WebDAV: enable, address, username, password, remote path, accept invalid
certificates, and the upload / download buttons.
**Automation** — the four switches, split by what they actually govern: **Unattended
access** (use saved credentials, allow arbitrary SSH commands, allow file transfers) and
the MCP server's own **MCP server** switch.

## CLI and MCP

The CLI and the MCP server share the sessions saved in the GUI and the same SSH / SFTP
implementation, so there is only one server list to maintain. The CLI suits scripts, CI
and explicit commands; MCP lets an AI client do inspection, log analysis and file
transfers from a natural-language request.

> Connect to the target session in the GUI at least once first, so the host key is
> confirmed. Normal CLI/MCP responses do not expose passwords or private keys. Explicit
> CLI exports write recoverable credentials to a private file; do not put
> plaintext passwords into prompts or MCP configuration files.

### CLI

```bash
xenterm cli help
```

```bash
# List saved sessions; the first column is the session-id used below
xenterm cli sessions
xenterm cli sessions --json

# Non-secret metadata for one session
xenterm cli session <session-id>

# Run a non-interactive command; the remote command follows --
xenterm cli exec <session-id> -- free -h
xenterm cli exec <session-id> --timeout 60 --json -- journalctl -n 100 --no-pager

# Browse, read and transfer remote files
xenterm cli files <session-id> /var/log
xenterm cli read <session-id> /var/log/example.log
xenterm cli upload <session-id> ./local.txt /tmp
xenterm cli download <session-id> /tmp/result.txt ./downloads

# Inspect the effective access policy without changing it
xenterm cli permissions --json

# Explicit private export: choose a new file outside the profile directory
xenterm cli export ./private-sessions.json --include-credentials

# Preview an append-only import before applying it
xenterm cli import ./private-sessions.json --dry-run --json

# Preview stable-ID migration in an independent profile with local credential storage
xenterm --data-dir ./migration-profile cli import ./private-sessions.json --preserve-ids --dry-run --json
xenterm --data-dir ./migration-profile cli sync-native ./sessions.json --dry-run --json
```

Downloads require an existing local destination directory and will not overwrite a file of
the same name.

Portable exports contain recoverable credentials and must be kept private. They include
saved sessions, not global settings, trusted host keys or external key files. Export refuses
an existing destination. Import and `sync-native` examples above are previews; removing
`--dry-run` writes configuration. For migration from another profile, start with a portable
import. Native encrypted snapshots still require the matching local encryption key;
importing portable sessions does not make another profile's ciphertext readable, and OS
keyring markers cannot be imported. Headless CLI/MCP also requires an
explicit independent `--data-dir` (before the subcommand) or `XENTERM_DATA_DIR`; do not
point it at the desktop profile. See [profile migration](docs/REMOTE_MCP.md).

### MCP

Open **Settings → Automation → MCP server** in XenTerm and turn on what you need: enable
MCP, then allow saved credentials, arbitrary SSH commands and file transfers as your use
case requires. The three unattended-access switches default to on while MCP is in preview.

Then register the stdio server in your MCP client:

```json
{
  "mcpServers": {
    "xenterm": {
      "command": "/absolute/path/to/xenterm",
      "args": ["mcp", "serve"]
    }
  }
}
```

On Windows, `command` can be `C:\\path\\to\\xenterm.exe`. Restart or refresh the client and
you should see an `xenterm` server exposing the tools registered in
[`src/mcp/impls/tools.rs`](src/mcp/impls/tools.rs): `get_access_policy`, `import_sessions`,
`list_sessions`, `get_session`,
`run_command`, `list_remote_files`, `read_remote_text_file`, `upload_file`,
`download_file`. MCP configuration files live in different places per client, so check that
client's documentation.

`get_access_policy` is read-only. `import_sessions` previews by default; applying an import
requires starting MCP with `--allow-config-import`, and file-transfer permission is required
for both preview and apply. Credential export is deliberately available only through the
local CLI, not as an MCP tool.

#### Debugging the stdio connection

An AI client generates these requests for you. When debugging by hand, send each one as a
single line of JSON, in this order:

```jsonl
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"example-client","version":"1.0.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
```

List sessions, then run a read-only OOM diagnosis and browse the heap-dump directory:

```jsonl
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_sessions","arguments":{}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"run_command","arguments":{"session_id":"<session-id>","command":"free -h; dmesg 2>/dev/null | grep -iE 'oom|killed process' | tail -50 || true; ps -ef | grep '[j]ava'","timeout_seconds":30,"max_output_bytes":1048576}}}
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"list_remote_files","arguments":{"session_id":"<session-id>","path":"/home/jeff/test/heapdumps"}}}
```

`read_remote_text_file` accepts only bounded UTF-8 text; for binaries such as HPROF use
`download_file`, which requires an existing local directory and will not overwrite a file
of the same name.

A prompt you could give an MCP-capable client:

> Use the `xenterm` MCP to investigate an OOM on my `192.168.100.41` server. Heap dumps are
> in `/home/jeff/test/heapdumps`. Check system memory, kernel OOM records, Java processes,
> application logs and the HPROF files, then identify the root cause. Read-only diagnostics
> first — do not restart services or delete files.

The server finds the session with `list_sessions` and then acts within the permissions you
granted. If several sessions share a host, name the GUI session in the prompt.

## Configuration and data

XenTerm is **portable-first**: if a writable `config/` folder sits next to the executable
(a USB stick, an extracted tarball), everything lives there. Otherwise it falls back to the
per-user OS config directory:

| Platform | Location |
| --- | --- |
| Windows | `%APPDATA%\xenterm\XenTerm\config` |
| Linux | `~/.config/xenterm` |
| macOS | `~/Library/Application Support/dev.xenterm.XenTerm` |

Upgrading from the pre-rename build (`meatshell`) migrates `sessions.json`, `secret.key`
and `known_hosts` automatically — copies are left in place, and existing files are never
overwritten. Diagnostics go to a separate `log/` directory beside the config, with
`error.log` capped at 50 MiB.

## Security notes

- **Passwords at rest** go to the OS keychain (Windows Credential Manager, macOS Keychain,
  Linux Secret Service) whenever it is available. When it is not, they are encrypted with
  **ChaCha20-Poly1305** under a per-install key in `secret.key`, so `sessions.json` never
  contains a plaintext password.
- **Exported connection lists are portable, not secret.** The export format uses a fixed
  key compiled into the binary so a file opens on any machine — that is obfuscation, the
  same level as a FinalShell export, not encryption to rely on.
- **Host keys** are remembered by XenTerm itself in its own `known_hosts` file (a
  `host:port <key-type> <base64>` format), which is not OpenSSH's file.
- **Secrets are zeroed on drop**, so plaintext credentials do not linger in freed memory.

## Tech stack

| Layer | Choice |
| --- | --- |
| UI | [GPUI](https://gpui.rs) via `gpui-kit` — GPU-rendered Rust UI, compiled in |
| Async runtime | [`tokio`](https://tokio.rs) |
| SSH / SFTP | [`russh`](https://crates.io/crates/russh) + [`russh-sftp`](https://crates.io/crates/russh-sftp) — pure Rust, no libssh |
| Terminal emulation | [`rio-vt`](https://crates.io/crates/rio-vt) native grid, scrollback and reflow |
| Local PTY | [`portable-pty`](https://crates.io/crates/portable-pty) |
| Serial ports | [`serialport`](https://crates.io/crates/serialport) |
| System metrics | [`sysinfo`](https://crates.io/crates/sysinfo) |
| Encryption | [`chacha20poly1305`](https://crates.io/crates/chacha20poly1305); PPK via [`aes`](https://crates.io/crates/aes) + [`argon2`](https://crates.io/crates/argon2) |
| Serialization | `serde` + `serde_json` |
| Logging | `tracing` + `tracing-subscriber` |

## Project layout

```
xenterm/
├── Cargo.toml
├── build.rs                    # embeds assets/xenterm.ico on Windows
├── ui/fonts/                   # bundled terminal font (regular + bold) and the icon font
├── assets/                     # icons, banner, desktop entry, Linux installer
├── packaging/                  # AUR PKGBUILD and the Flatpak manifest
└── src/
    ├── main.rs                 # entry point: UI, `cli`, `mcp serve`
    ├── ui/                     # the shell — pages, panels, dialogs, detached windows
    │   ├── impls/pages/        # the three pages: terminal, sessions, settings
    │   └── impls/              # panels, dialogs, the terminal view and the shell
    ├── app/                    # session models, the PTY pump, window/jump-list plumbing
    ├── core/                   # toolkit-free state: tabs, panes, maps, histories
    ├── cli/  mcp/  automation/ # script and AI entry points over one dispatcher
    ├── config/                 # sessions.json, encryption, keychain, import/export
    ├── ssh/  sftp/  tunnel/    # protocols and plumbing
    ├── terminal/               # VT parsing, rendering, ZMODEM, serial, Telnet
    ├── resource/               # local sampling and remote metric parsing
    ├── webdav/  i18n/  layout/  logging/
    └── allocator/              # jemalloc on Unix, mimalloc on Windows
```

## Development

Before contributing, read the [contribution guide](CONTRIBUTING.md) (also available in 简体中文) for PR scope, conflict handling, review readiness, and API compatibility checks.

- **The interface is GPUI only.** There is no feature flag to build against and no second
  frontend to keep in step. A bare `xenterm` launch and the migration-era `xenterm gpui`
  argument both open the same shell.
- **A page is a view with its own state**, built on first entry and kept after that, so a
  half-typed filter or a scroll position survives switching away. The shell renders exactly
  one page per frame; anything that cannot be done inside the page is queued and drained by
  the shell at the top of the next frame, which is also why a click is never carried out
  inside the render that received it.
- **The interface uses icon-font glyphs and words, never emoji** — enforced by a test that
  scans `src/ui`, because a check that ran once is a fact about that day rather than a
  property of the code.
- **Translations are pairs of literals**, not a catalogue: `crate::i18n::t("中文", "English")`
  picks by the current language flag, and both languages ship in the binary.
- **`cargo check` is the fastest feedback loop** while editing the UI.
- Run the test suite with `cargo test`; the integration tests live under `tests/app/`.
  Only the three suites wired in through `#[path]` are actually compiled today, so add a
  new module to `mod.rs` rather than only creating the file.

## Release

Do not bump `Cargo.toml` by hand and then tag it. Use the release script, so the tag points
at a commit that already carries the matching version:

```powershell
.\scripts\release.ps1 v0.7.4 -Push
```

It requires no uncommitted tracked-file changes, updates `Cargo.toml` and the `xenterm`
entry in `Cargo.lock`, runs `cargo check --locked`, verifies `xenterm --version`, commits
`Release v0.7.4` and creates an annotated tag, then pushes the branch and tag. See
[docs/release.md](docs/release.md).

> **Before the first release from this tree**, the workflow's Linux runners may need extra
> apt packages for GPUI's platform libraries, and GPUI's Windows crate wants `fxc.exe` from
> the Windows SDK for release builds (debug builds skip that step). Both are recorded in
> `Cargo.toml` rather than assumed away.

## Community

<p align="center">
  <img src="docs/QR/QQ_Group_QR_Code.jpg" alt="QQ group QR code" width="280"><br>
  <em>Scan to join the QQ group for usage questions, bug reports and release news</em>
</p>

## Acknowledgements

- [ixbaicn/XenTerm](https://github.com/ixbaicn/XenTerm) — the upstream project on which XenTerm Next is based.
- [meatshell](https://github.com/yituorou/meatshell)

## License

Released under the [MIT license](LICENSE). Contributions are accepted under the same terms.

Third-party attributions — including the Twemoji artwork used for colour emoji — are in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
