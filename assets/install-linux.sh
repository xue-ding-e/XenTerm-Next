#!/usr/bin/env bash
#
# Install xenterm on Linux so the dock and app launcher use the installed binary.
#
# Why this is needed: the Windows build embeds the icon in the .exe, but on Linux
# the icon comes from a freedesktop ".desktop" entry plus an icon installed into
# the hicolor icon theme. On Wayland (Ubuntu's default) the shell matches a
# running window to its .desktop file via the window's app_id — xenterm sets
# that to "xenterm" (set_xdg_app_id), and this script's StartupWMClass
# matches it.
#
# Usage:
#   ./install-linux.sh [--user] [--prefix DIR] [/path/to/xenterm-binary]
# You normally don't need an argument: when run from inside a release package
# (the `xenterm` binary sits next to this script) it is picked up automatically.
# In the source tree it falls back to ./target/release/xenterm.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
    cat <<'EOF'
Usage: install-linux.sh [--user] [--prefix DIR] [BINARY]

  --user         Default to $HOME/.local and never invoke sudo
  --prefix DIR   Install under DIR (also accepts --prefix=DIR)
  -h, --help     Show this help

Prefix precedence: --prefix > PREFIX environment variable > default.
The default is /usr/local, or $HOME/.local with --user.
BINARY defaults to the sibling xenterm, then ../target/release/xenterm.
Use -- before a binary path starting with a dash.
EOF
}

die() { echo "error: $*" >&2; exit 1; }

USER_INSTALL=false
CLI_PREFIX=""
BIN=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --user) USER_INSTALL=true; shift ;;
        --prefix)
            [ "$#" -ge 2 ] && [ -n "$2" ] || die "--prefix requires a directory"
            CLI_PREFIX="$2"; shift 2 ;;
        --prefix=*)
            CLI_PREFIX="${1#*=}"
            [ -n "$CLI_PREFIX" ] || die "--prefix requires a directory"
            shift ;;
        -h|--help) usage; exit 0 ;;
        --) shift; break ;;
        -*) die "unknown option: $1 (use --help for usage)" ;;
        *)
            [ -z "$BIN" ] || die "only one binary path may be supplied"
            BIN="$1"; shift ;;
    esac
done
if [ "$#" -gt 0 ]; then
    [ "$#" -eq 1 ] && [ -z "$BIN" ] || die "only one binary path may be supplied"
    BIN="$1"
fi

if [ -n "$CLI_PREFIX" ]; then
    PREFIX="$CLI_PREFIX"
elif [ -z "${PREFIX:-}" ]; then
    if "$USER_INSTALL"; then
        [ -n "${HOME:-}" ] || die "HOME must be set for --user"
        PREFIX="$HOME/.local"
    else
        PREFIX="/usr/local"
    fi
fi

# Desktop Exec paths cannot contain '='. Reject line/control characters rather
# than allowing a path to inject desktop-entry keys. Validate before any writes.
[[ "$PREFIX" != *'='* && "$PREFIX" != *[[:cntrl:]]* ]] ||
    die "the installation prefix cannot contain '=' or control characters"
PREFIX="$(readlink -m -- "$PREFIX")"
[[ "$PREFIX" != *'='* && "$PREFIX" != *[[:cntrl:]]* ]] ||
    die "the resolved installation prefix cannot contain '=' or control characters"

# Resolve the binary: explicit arg > sibling (release package) > source-tree build.
if [ -z "$BIN" ]; then
    if [ -f "$SCRIPT_DIR/xenterm" ]; then
        BIN="$SCRIPT_DIR/xenterm"
    else
        BIN="$SCRIPT_DIR/../target/release/xenterm"
    fi
fi
BIN="$(readlink -f -- "$BIN")" || die "could not resolve the binary path"

if [ ! -f "$BIN" ] || [ ! -r "$BIN" ]; then
    echo "error: xenterm binary not found or unreadable: $BIN" >&2
    echo "Run this script from the extracted release folder (it sits next to the" >&2
    echo "'xenterm' binary), or pass the binary path as an argument." >&2
    exit 1
fi

ICON_SRC="$SCRIPT_DIR/icon@512.png"
BIN_DIR="${PREFIX%/}/bin"
ICON_DIR="${PREFIX%/}/share/icons/hicolor/512x512/apps"
APP_DIR="${PREFIX%/}/share/applications"
DESKTOP="$APP_DIR/xenterm.desktop"

# Exact output of the original installer, before managed-launcher markers.
# Keep this frozen: matching only Name/Exec would overwrite custom launchers.
legacy_desktop() {
    cat <<'EOF'
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
}

# Don't overwrite a hand-written or package-managed launcher at the destination.
# Launchers at other locations are also left alone, even if they mention xenterm.
if [ -e "$DESKTOP" ] || [ -L "$DESKTOP" ]; then
    grep -qx 'X-XenTerm-Installer=true' "$DESKTOP" ||
        cmp -s -- "$DESKTOP" <(legacy_desktop) ||
        die "launcher already exists: $DESKTOP; review and move it before retrying"
fi

# A fresh prefix may not exist yet: check its nearest existing ancestor too.
directory_writable() {
    local directory="$1"
    while [ ! -e "$directory" ] && [ ! -L "$directory" ]; do
        directory="$(dirname -- "$directory")"
    done
    [ -d "$directory" ] && [ -w "$directory" ] && [ -x "$directory" ]
}

SUDO=()
if [ "$(id -u)" -ne 0 ]; then
    NEED_SUDO=false
    for directory in "$BIN_DIR" "$ICON_DIR" "$APP_DIR"; do
        directory_writable "$directory" || NEED_SUDO=true
    done
    for file in "$BIN_DIR/xenterm" "$ICON_DIR/xenterm.png" "$DESKTOP"; do
        if { [ -e "$file" ] || [ -L "$file" ]; } && [ ! -w "$file" ]; then
            NEED_SUDO=true
        fi
    done
    if "$NEED_SUDO"; then
        "$USER_INSTALL" && die "prefix is not writable: $PREFIX (--user never uses sudo)"
        command -v sudo >/dev/null 2>&1 ||
            die "prefix is not writable: $PREFIX; use --user, a writable --prefix, or run as root"
        sudo -v
        SUDO=(sudo)
    fi
fi

# Exec has two escaping layers: argument quoting, then desktop string escaping.
# Escape literal '%' as '%%' so a path cannot be interpreted as a field code.
# https://specifications.freedesktop.org/desktop-entry/latest/exec-variables.html
desktop_string() {
    local value="$1"
    value="${value//\\/\\\\}"
    printf '%s' "$value"
}
desktop_exec() {
    local value="$1"
    value="${value//\\/\\\\}"
    value="${value//\"/\\\"}"
    value="${value//\$/\\\$}"
    value="${value//\`/\\\`}"
    value="${value//%/%%}"
    printf '"%s"' "$(desktop_string "$value")"
}
EXEC_VALUE="$(desktop_exec "$BIN_DIR/xenterm")"
# GIO checks that Exec's first word exists before expanding %% field escapes.
# Keep literal percent signs in an argument instead; env invokes it without a
# shell and the absolute installed path still does not depend on PATH.
if [[ "$BIN_DIR" == *%* ]]; then
    EXEC_VALUE="/usr/bin/env -- $EXEC_VALUE"
fi
ICON_VALUE=xenterm
if [ -f "$ICON_SRC" ]; then
    # Custom prefixes need not be on the desktop's icon search path.
    ICON_VALUE="$(desktop_string "$ICON_DIR/xenterm.png")"
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
Exec=$EXEC_VALUE
Icon=$ICON_VALUE
Terminal=false
Categories=Network;TerminalEmulator;
Keywords=ssh;sftp;terminal;shell;
StartupNotify=true
StartupWMClass=xenterm
Actions=new-window;
X-XenTerm-Installer=true

[Desktop Action new-window]
Name=New Window
Name[zh_CN]=新建窗口
Exec=$EXEC_VALUE --new-window
EOF
"${SUDO[@]}" install -d -- "$BIN_DIR" "$ICON_DIR" "$APP_DIR"
"${SUDO[@]}" install -m755 -- "$BIN" "$BIN_DIR/xenterm"
if [ -f "$ICON_SRC" ]; then
    "${SUDO[@]}" install -m644 -- "$ICON_SRC" "$ICON_DIR/xenterm.png"
else
    echo "warning: icon not found ($ICON_SRC); the desktop entry will use a generic icon" >&2
fi
"${SUDO[@]}" install -m644 -- "$DESKTOP_TMP" "$DESKTOP"

if [ -n "${HOME:-}" ]; then
    USER_DESKTOP="${XDG_DATA_HOME:-$HOME/.local/share}/applications/xenterm.desktop"
    if [ -f "$USER_DESKTOP" ] && ! [ "$USER_DESKTOP" -ef "$DESKTOP" ]; then
        echo "warning: preserved user launcher may override this installation: $USER_DESKTOP" >&2
    fi
fi

# Refresh the desktop + icon caches (best-effort; harmless if the tools are absent).
if command -v update-desktop-database >/dev/null 2>&1; then
    "${SUDO[@]}" update-desktop-database "$APP_DIR" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    "${SUDO[@]}" gtk-update-icon-cache -f -t "${PREFIX%/}/share/icons/hicolor" 2>/dev/null || true
fi

echo "Installed:"
if [ -f "$ICON_SRC" ]; then
    echo "  icon    -> $ICON_DIR/xenterm.png"
fi
echo "  desktop -> $DESKTOP"
echo "  exec    -> $BIN_DIR/xenterm"
echo
echo "If the dock still shows the generic icon, log out/in (Wayland) or run"
echo "'killall -3 gnome-shell' (X11) to refresh the shell."
