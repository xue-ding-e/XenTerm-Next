#!/usr/bin/env bash
#
# Install xenterm system-wide on Linux so the GNOME/Ubuntu dock and app
# launcher use one canonical executable and desktop entry.
#
# Why this is needed: the Windows build embeds the icon in the .exe, but on Linux
# the icon comes from a freedesktop ".desktop" entry plus an icon installed into
# the hicolor icon theme. On Wayland (Ubuntu's default) the shell matches a
# running window to its .desktop file via the window's app_id — xenterm sets
# that to "xenterm" (set_xdg_app_id), and this script's StartupWMClass
# matches it.
#
# Usage (requires sudo):
#   ./install-linux.sh [/path/to/xenterm-binary]
# You normally don't need an argument: when run from inside a release package
# (the `xenterm` binary sits next to this script) it is picked up automatically.
# In the source tree it falls back to ./target/release/xenterm.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Resolve the binary: explicit arg > sibling (release package) > source-tree build.
if [ -n "${1:-}" ]; then
    BIN="$1"
elif [ -x "$SCRIPT_DIR/xenterm" ]; then
    BIN="$SCRIPT_DIR/xenterm"
else
    BIN="$SCRIPT_DIR/../target/release/xenterm"
fi
BIN="$(readlink -f "$BIN" 2>/dev/null || echo "$BIN")"

# Make sure the binary is executable (a downloaded tarball may have lost +x).
[ -f "$BIN" ] && chmod +x "$BIN" 2>/dev/null || true

if [ ! -x "$BIN" ]; then
    echo "error: xenterm binary not found: $BIN" >&2
    echo "Run this script from the extracted release folder (it sits next to the" >&2
    echo "'xenterm' binary), or pass the binary path as an argument." >&2
    exit 1
fi

ICON_SRC="$SCRIPT_DIR/icon@512.png"
PREFIX="/usr/local"
ICON_DIR="$PREFIX/share/icons/hicolor/512x512/apps"
APP_DIR="$PREFIX/share/applications"

if ! command -v sudo >/dev/null 2>&1; then
    echo "error: sudo is required for a system-wide installation" >&2
    exit 1
fi
sudo -v

sudo install -d "$ICON_DIR" "$APP_DIR"
sudo install -m755 "$BIN" "$PREFIX/bin/xenterm"
if [ -f "$ICON_SRC" ]; then
    sudo install -m644 "$ICON_SRC" "$ICON_DIR/xenterm.png"
else
    echo "warning: icon not found ($ICON_SRC); the desktop entry will use a generic icon" >&2
fi

DESKTOP_TMP="$(mktemp)"
trap 'rm -f "$DESKTOP_TMP"' EXIT
cat > "$DESKTOP_TMP" <<EOF
[Desktop Entry]
Type=Application
Name=xenterm
GenericName=SSH Client
Comment=Lightweight Rust + GPUI SSH/SFTP client
Comment[zh_CN]=轻量级 Rust + GPUI SSH/SFTP 客户端
Exec=xenterm
Icon=xenterm
Terminal=false
Categories=Network;TerminalEmulator;
Keywords=ssh;sftp;terminal;shell;
StartupNotify=true
StartupWMClass=xenterm
Actions=new-window;

[Desktop Action new-window]
Name=New Window
Name[zh_CN]=新建窗口
Exec=xenterm --new-window
EOF
sudo install -m644 "$DESKTOP_TMP" "$APP_DIR/xenterm.desktop"

OLD_USER_DESKTOP="$HOME/.local/share/applications/xenterm.desktop"
if [ -f "$OLD_USER_DESKTOP" ] && grep -q '^Exec=.*xenterm' "$OLD_USER_DESKTOP"; then
    rm -f "$OLD_USER_DESKTOP"
    echo "Removed stale user launcher: $OLD_USER_DESKTOP"
fi

# Refresh the desktop + icon caches (best-effort; harmless if the tools are absent).
sudo update-desktop-database "$APP_DIR" 2>/dev/null || true
sudo gtk-update-icon-cache -f -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true

echo "Installed:"
echo "  icon    -> $ICON_DIR/xenterm.png"
echo "  desktop -> $APP_DIR/xenterm.desktop"
echo "  exec    -> $PREFIX/bin/xenterm"
echo
echo "If the dock still shows the generic icon, log out/in (Wayland) or run"
echo "'killall -3 gnome-shell' (X11) to refresh the shell."
