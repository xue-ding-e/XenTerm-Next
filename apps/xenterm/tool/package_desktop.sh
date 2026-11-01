#!/usr/bin/env bash
# Packages the Flutter desktop release bundle, with the FFI bridge already
# staged by bundle_native.sh, into the archive shipped by the release workflow.
# Usage: bash apps/xenterm/tool/package_desktop.sh <version>
# Prints the archive path on the last line.
set -euo pipefail

version="${1:?usage: package_desktop.sh <version>}"
repo="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$repo"

copy_extras() {
  local stage="$1"
  cp README.md README.zh.md CHANGELOG.md THIRD_PARTY_NOTICES.md "$stage/" 2>/dev/null || true
  # Build provenance (版本号规范 v1 §6); version.sh writes this file.
  cp version.json "$stage/" 2>/dev/null || true
}

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    bundle="apps/xenterm/build/windows/x64/runner/Release"
    [ -f "$bundle/xenterm.exe" ] || { echo "missing Flutter Windows release build" >&2; exit 1; }
    [ -f "$bundle/xenterm_bridge.dll" ] || { echo "missing xenterm_bridge.dll in the bundle" >&2; exit 1; }
    stage="xenterm-${version}-windows-x86_64-desktop"
    rm -rf "$stage" "${stage}.zip"
    mkdir -p "$stage"
    cp -a "$bundle/." "$stage/"
    # A local launch creates its profile next to the exe; never ship it.
    rm -rf "$stage/config"
    # Never ship stale artifacts left in the build tree: symbols and a
    # pre-CMake-rename executable (windows/CMakeLists.txt sets BINARY_NAME to
    # xenterm, so xenterm_app.exe cannot be a current output).
    rm -f "$stage"/*.pdb "$stage"/xenterm_app.exe
    copy_extras "$stage"
    if [ -x "/c/Windows/System32/tar.exe" ]; then
      # bsdtar ships with Windows and can write zip from the extension.
      "/c/Windows/System32/tar.exe" -a -c -f "${stage}.zip" "$stage"
    elif command -v 7z >/dev/null 2>&1; then
      7z a "${stage}.zip" "$stage" >/dev/null
    else
      # Git Bash paths (/d/...) are not PowerShell paths.
      root_win="$(cygpath -w "$(pwd)")"
      powershell -NoProfile -Command \
        "Compress-Archive -Path '$root_win\\$stage' -DestinationPath '$root_win\\${stage}.zip' -Force"
    fi
    echo "${stage}.zip"
    ;;

  Darwin)
    app="apps/xenterm/build/macos/Build/Products/Release/xenterm_app.app"
    [ -d "$app" ] || { echo "missing Flutter macOS release build" >&2; exit 1; }
    case "$(uname -m)" in
      arm64) arch="aarch64" ;;
      x86_64) arch="x86_64" ;;
      *) echo "unsupported macOS architecture: $(uname -m)" >&2; exit 1 ;;
    esac
    [ -f "$app/Contents/Frameworks/libxenterm_bridge.dylib" ] \
      || { echo "missing bridge dylib in the app bundle" >&2; exit 1; }
    # Rename the bundle directory only; Info.plist and the inner executable
    # keep their generated names.
    rm -rf XenTerm.app
    ditto "$app" "XenTerm.app"
    # Re-sign the whole bundle after adding the bridge (the release build is
    # stripped, so the linker's automatic ad-hoc signature is already invalid).
    codesign --force --deep --sign - "XenTerm.app"
    codesign --verify --deep --strict --verbose=2 "XenTerm.app"
    stage="xenterm-${version}-macos-${arch}-desktop"
    rm -f "${stage}.zip"
    ditto -c -k --keepParent "XenTerm.app" "${stage}.zip"
    # Regression guard: the release zip must carry the bridge inside the
    # bundle, otherwise users get "xenterm_bridge library not found".
    unzip -l "${stage}.zip" | grep -q "Contents/Frameworks/libxenterm_bridge.dylib" \
      || { echo "bridge dylib missing from ${stage}.zip" >&2; exit 1; }
    echo "${stage}.zip"
    ;;

  Linux)
    case "$(uname -m)" in
      x86_64|amd64) arch="x64"; deb_arch="x86_64" ;;
      aarch64|arm64) arch="arm64"; deb_arch="aarch64" ;;
      *) echo "unsupported Linux architecture: $(uname -m)" >&2; exit 1 ;;
    esac
    bundle="apps/xenterm/build/linux/$arch/release/bundle"
    [ -x "$bundle/xenterm" ] || { echo "missing Flutter Linux release build" >&2; exit 1; }
    [ -f "$bundle/lib/libxenterm_bridge.so" ] \
      || { echo "missing libxenterm_bridge.so in the bundle" >&2; exit 1; }
    stage="xenterm-${version}-linux-${deb_arch}-desktop"
    rm -rf "$stage" "${stage}.tar.gz"
    mkdir -p "$stage"
    cp -a "$bundle/." "$stage/"
    copy_extras "$stage"
    # Desktop entry + icon so distributions (and users) can register the app.
    cp assets/xenterm.desktop "assets/icon@512.png" "$stage/" 2>/dev/null || true
    # Linux installer: run it from the extracted folder for a per-user install.
    # Git on Windows records the file as 0644, so set the mode here instead of
    # relying on the checkout to make it executable.
    cp assets/install-linux.sh "$stage/"
    chmod 755 "$stage/install-linux.sh"
    tar -czf "${stage}.tar.gz" "$stage"
    echo "${stage}.tar.gz"
    ;;

  *)
    echo "unsupported platform: $(uname -s)" >&2
    exit 1
    ;;
esac
