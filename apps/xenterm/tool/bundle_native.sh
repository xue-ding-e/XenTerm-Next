#!/usr/bin/env bash
# Copies the release FFI bridge next to the Flutter desktop release binary.
# Run after `flutter build <platform> --release` and
# `cargo build --release -p xenterm-bridge`.
set -euo pipefail

repo="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$repo"

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    lib="xenterm_bridge.dll"
    dest="apps/xenterm/build/windows/x64/runner/Release"
    ;;
  Darwin)
    lib="libxenterm_bridge.dylib"
    dest="apps/xenterm/build/macos/Build/Products/Release/xenterm_app.app/Contents/Frameworks"
    ;;
  Linux)
    lib="libxenterm_bridge.so"
    case "$(uname -m)" in
      x86_64|amd64) arch="x64" ;;
      aarch64|arm64) arch="arm64" ;;
      *) echo "unsupported Linux architecture: $(uname -m)" >&2; exit 1 ;;
    esac
    dest="apps/xenterm/build/linux/$arch/release/bundle/lib"
    ;;
  *)
    echo "unsupported platform: $(uname -s)" >&2
    exit 1
    ;;
esac

src="target/release/$lib"
[ -f "$src" ] || { echo "missing $src (build xenterm-bridge first)" >&2; exit 1; }
[ -d "$dest" ] || { echo "missing Flutter release bundle directory: $dest" >&2; exit 1; }

cp -f "$src" "$dest/"
[ -f "$dest/$lib" ] || { echo "bridge copy failed: $dest/$lib" >&2; exit 1; }

if [ "$(uname -s)" = "Darwin" ]; then
  # An unsigned dylib is rejected by dyld on Apple Silicon even inside an
  # ad-hoc-signed bundle.
  codesign --force --sign - "$dest/$lib"
fi

echo "bundled $lib -> $dest"
