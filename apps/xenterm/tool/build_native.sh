#!/usr/bin/env bash
# Builds the Rust FFI bridge and drops it next to the Flutter desktop binary.
# Usage: apps/xenterm/tool/build_native.sh
set -euo pipefail

repo="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$repo"

cargo build --release -p xenterm-bridge

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) lib="xenterm_bridge.dll" ;;
  Darwin) lib="libxenterm_bridge.dylib" ;;
  *) lib="libxenterm_bridge.so" ;;
esac

src="$repo/target/release/$lib"
[ -f "$src" ] || { echo "missing $src" >&2; exit 1; }

for cfg in Debug Release; do
  dst="$repo/apps/xenterm/build/windows/x64/runner/$cfg"
  # Windows runner layout.
  if [ -d "$dst" ]; then
    cp -f "$src" "$dst/"
    echo "copied -> $cfg"
  fi
done

# macOS bundles embed shared libraries in Contents/Frameworks (the release
# packaging and the app's loader both look there).
if [ "$(uname -s)" = "Darwin" ]; then
  for cfg in Debug Release; do
    dst="$repo/apps/xenterm/build/macos/Build/Products/$cfg/xenterm_app.app/Contents/Frameworks"
    if [ -d "$dst" ]; then
      cp -f "$src" "$dst/"
      codesign --force --sign - "$dst/$lib"
      echo "copied -> macos/$cfg/Frameworks"
    fi
  done
fi

# Linux desktop builds put the executable in a bundle directory; the bridge has
# to sit next to it (the loader does not search the working directory).
if [ "$(uname -s)" = "Linux" ]; then
  for cfg in debug profile release; do
    dst="$repo/apps/xenterm/build/linux/x64/$cfg/bundle"
    if [ -d "$dst" ]; then
      cp -f "$src" "$dst/"
      echo "copied -> linux/$cfg/bundle"
    fi
  done
fi
