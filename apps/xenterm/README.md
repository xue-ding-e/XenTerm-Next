# XenTerm desktop UI (`apps/xenterm`)

The Flutter desktop front end of XenTerm. It renders the pages, the terminal
grid and the dialogs, and talks to the Rust service core through the
`xenterm-bridge` FFI library — it does not open sessions, read the profile or
speak SSH itself.

- Product overview and installation: [top-level README](../../README.md)
- Bridge architecture and ABI conventions: [docs/flutter-bridge.md](../../docs/flutter-bridge.md)
- Contribution flow: [CONTRIBUTING.md](../../CONTRIBUTING.md) /
  [CONTRIBUTING.zh-CN.md](../../CONTRIBUTING.zh-CN.md)

## Requirements

- Flutter 3.47 or newer, stable channel (CI builds with 3.47.7)
- Rust 1.88 or newer, for the bridge library
- Platform toolchains: Visual Studio C++ build tools (Windows), Xcode (macOS),
  GTK 3 development packages (Linux)

## Build and run

From the repository root, build the bridge, stage it next to the Flutter output,
then build or run the app:

```bash
cargo build --release -p xenterm-bridge
bash apps/xenterm/tool/bundle_native.sh

cd apps/xenterm
flutter pub get
flutter run -d windows        # or linux / macos
flutter build windows --release
```

`tool/build_native.ps1` (Windows) and `tool/build_native.sh` combine the first
two steps. `tool/package_desktop.sh <version>` produces the release archive that
the release workflow publishes.

The Dart side loads the bridge from the application directory; set
`XENTERM_BRIDGE_PATH` to override the library path during development. No bridge
found means the app cannot start — check the error dialog and the staging step
first.

## Layout

```
lib/
├── main.dart               # app bootstrap, theme and font loading
├── bridge/                 # dart:ffi bindings and the JSON DTOs
├── data/                   # host models and the remote-OS registry
├── screens/
│   ├── hosts/              # Home: host cards, groups, editors, import
│   ├── terminal/           # terminal view, SFTP view, connection prompts
│   ├── keychain/           # key metadata
│   ├── forwards/           # tunnels and jump chains
│   ├── snippets/           # quick commands
│   ├── known_hosts/        # trusted host keys
│   ├── logs/               # session transcripts
│   └── settings/           # terminal, SFTP, logs and shortcut settings
├── shell/                  # navigation rail, title bar, window chrome
├── terminal/               # core settings cache and command bus
├── theme/                  # palette, metrics, motion, field primitives
└── widgets/                # shared primitives and extra Lucide icons
native/                     # xenterm-bridge crate: C ABI over the Rust core
tool/                       # build, staging, packaging and test-server scripts
windows/ linux/ macos/ ios/ android/   # Flutter platform runners
```

## Working rules

- **Behaviour belongs in the core.** If it needs a protocol, the profile, the
  keychain or a shell, add it to `crates/xenterm` and expose it through
  `native/src/lib.rs`; keep the Dart side limited to presentation and input.
- **Poll snapshots, do not push state.** Live terminal and SFTP state is read
  through cheap version counters; only fetch a snapshot when the counter moved.
- **One theme, one component set.** Colours, radii and spacing come from
  `theme/app_theme.dart` (neutral monochrome palette); inputs, selects and
  dialogs come from `theme/app_field.dart` and `widgets/primitives.dart`.
  Do not add a second styling vocabulary for a single screen.
- **No UI-toolkit dependency in the core.** The core is also built as a
  standalone CLI/MCP binary; the bridge is its only UI-facing surface.

## Checks

```bash
flutter analyze
flutter test
```

Widget tests inject sample hosts and loaders as fixtures instead of pretending
production data. For the bridge's own tests, run
`cargo test -p xenterm-bridge` from the repository root.
