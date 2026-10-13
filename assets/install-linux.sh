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
  --[no-]add-to-path       Manage user shell PATH (default: on for --user)
  --[no-]desktop-shortcut  Copy a launcher to the XDG desktop (default: --user)
  --[no-]start-menu        Register the application menu (default: on)
  --uninstall             Remove unchanged files owned by this installer
  -h, --help               Show this help

User integration uses the invoking user's HOME; system installs never guess a
logged-in desktop user. Explicit custom prefixes remain isolated unless --user
is also given. Shell changes apply to new bash/zsh/login shells. Existing choices
are retained on repeat installs unless an option explicitly overrides them.

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
UNINSTALL=false
ADD_PATH=""
DESKTOP_LINK=""
MENU_LINK=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --user) USER_INSTALL=true; shift ;;
        --uninstall) UNINSTALL=true; shift ;;
        --add-to-path) ADD_PATH=1; shift ;;
        --no-add-to-path) ADD_PATH=0; shift ;;
        --desktop-shortcut) DESKTOP_LINK=1; shift ;;
        --no-desktop-shortcut) DESKTOP_LINK=0; shift ;;
        --start-menu) MENU_LINK=1; shift ;;
        --no-start-menu) MENU_LINK=0; shift ;;
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

# Keep installer state separate from application profiles. Never source state as
# shell code: it is a small numeric options record plus a SHA-256 file manifest.
STATE_DIR="${PREFIX%/}/share/xenterm/installer"
OPTIONS="$STATE_DIR/options"
MANIFEST="$STATE_DIR/files.sha256"
SHELL_CONTEXT="$STATE_DIR/shell-context"
for managed in "$STATE_DIR" "$OPTIONS" "$MANIFEST" "$SHELL_CONTEXT"; do
    [ "$(readlink -m -- "$managed")" = "$managed" ] || die "installer state traverses symlinks: $managed"
    [ ! -L "$managed" ] || die "refusing symlink installer state: $managed"
done
if [ -f "$OPTIONS" ]; then
    while IFS='=' read -r key value; do
        [[ "$value" == 0 || "$value" == 1 ]] || die "invalid installer options"
        case "$key" in
            path) [ -n "$ADD_PATH" ] || ADD_PATH="$value" ;;
            desktop) [ -n "$DESKTOP_LINK" ] || DESKTOP_LINK="$value" ;;
            menu) [ -n "$MENU_LINK" ] || MENU_LINK="$value" ;;
            *) die "unknown installer option: $key" ;;
        esac
    done < "$OPTIONS"
fi
DEFAULT_USER=0
"$USER_INSTALL" && DEFAULT_USER=1
ADD_PATH="${ADD_PATH:-$DEFAULT_USER}"
DESKTOP_LINK="${DESKTOP_LINK:-$DEFAULT_USER}"
MENU_LINK="${MENU_LINK:-1}"
if [ "$ADD_PATH" = 1 ] && [[ "$PREFIX" = *:* ]]; then
    die "a colon in the prefix cannot be represented as one PATH entry; use --no-add-to-path"
fi
if ! "$USER_INSTALL" && { [ "$ADD_PATH" = 1 ] || [ "$DESKTOP_LINK" = 1 ]; }; then
    die "PATH and desktop integration require --user; system /usr/local/bin already uses the system PATH"
fi
if "$USER_INSTALL"; then
    [ -n "${HOME:-}" ] && [[ "$HOME" = /* && "$HOME" != *[[:cntrl:]]* ]] || die "valid absolute HOME required"
fi

# The XDG utility resolves localized desktop names without evaluating user code.
USER_DESKTOP_DIR=""
if "$USER_INSTALL" && command -v xdg-user-dir >/dev/null 2>&1; then
    USER_DESKTOP_DIR="$(xdg-user-dir DESKTOP)"
fi
if [ "$USER_DESKTOP_DIR" = "$HOME" ] || [ "$USER_DESKTOP_DIR" = "$HOME/" ]; then
    USER_DESKTOP_DIR=""
fi
if [ -n "$USER_DESKTOP_DIR" ]; then
    [[ "$(readlink -m -- "$USER_DESKTOP_DIR")" = "$USER_DESKTOP_DIR" ]] ||
        die "XDG desktop must be canonical and must not traverse symlinks"
    [[ "$USER_DESKTOP_DIR" = "$HOME/"* && "$USER_DESKTOP_DIR" != *[[:cntrl:]]* ]] ||
        die "XDG desktop must be inside HOME"
fi
# No desktop is a valid headless setup. Do not create an English Desktop folder.
if [ "$DESKTOP_LINK" = 1 ] && [ -z "$USER_DESKTOP_DIR" ]; then
    echo "warning: no XDG desktop configured; desktop shortcut skipped" >&2
fi
PATH_FRAGMENT="${PREFIX%/}/share/xenterm/installer/path.sh"
BLOCK_START="# >>> XenTerm installer PATH: $PREFIX >>>"
BLOCK_END="# <<< XenTerm installer PATH: $PREFIX <<<"

# Only delete an exact, unchanged installer-owned file. Reject manifest entries
# outside the fixed package layout; a modified manifest is not deletion authority.
owned_path() {
    case "$1" in
        "$PREFIX/bin/xenterm"|"$PREFIX/share/icons/hicolor/512x512/apps/xenterm.png"|"$PREFIX/share/applications/xenterm.desktop"|"$PATH_FRAGMENT") return 0 ;;
    esac
    "$USER_INSTALL" && [ -n "$USER_DESKTOP_DIR" ] &&
        [ "$1" = "$USER_DESKTOP_DIR/xenterm.desktop" ]
}
remove_owned_files() {
    [ -f "$MANIFEST" ] || return 0
    local digest path current failed=0
    local -a verified=()
    while IFS=$'\t' read -r digest path; do
        [[ "$digest" =~ ^[0-9a-f]{64}$ ]] && owned_path "$path" || die "invalid installer manifest (desktop location may have changed)"
        [ "$(readlink -m -- "$path")" = "$path" ] || die "managed path now traverses a symlink: $path"
        if [ -e "$path" ] || [ -L "$path" ]; then
            [ ! -L "$path" ] || { echo "preserved replaced symlink: $path" >&2; failed=1; continue; }
            current="$(sha256sum < "$path")"; current="${current%% *}"
            if [ "$current" = "$digest" ]; then
                verified+=("$path")
            else
                echo "preserved modified file: $path" >&2; failed=1
            fi
        fi
    done < "$MANIFEST"
    [ "$failed" = 0 ] || return 1
    # Validate the entire manifest before deleting anything. A changed binary
    # must not cause a half-uninstalled application or dangling PATH source.
    for path in "${verified[@]}"; do
        "${SUDO[@]}" rm -- "$path"
    done
}
# Scan all conventional login files so a later shell change cannot strand a
# previous managed block. Respect zsh's explicit configuration directory.
RC_FILES=()
RC_INSTALL_FILES=()
if "$USER_INSTALL"; then
    ZSH_HOME="${ZDOTDIR:-$HOME}"
    [[ "$ZSH_HOME" = /* && "$ZSH_HOME" != *[[:cntrl:]]* ]] || die "ZDOTDIR must be an absolute path"
    [ -d "$ZSH_HOME" ] || die "ZDOTDIR does not exist"
    RC_FILES=("$HOME/.profile" "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.bashrc" "$ZSH_HOME/.zshrc")
    LOGIN_RC="$HOME/.profile"
    [ ! -f "$HOME/.bash_login" ] || LOGIN_RC="$HOME/.bash_login"
    [ ! -f "$HOME/.bash_profile" ] || LOGIN_RC="$HOME/.bash_profile"
    RC_INSTALL_FILES=("$LOGIN_RC" "$HOME/.bashrc" "$ZSH_HOME/.zshrc")
    EXPECTED_SHELL_CONTEXT="$(printf '%s\n%s' "$HOME" "$ZSH_HOME")"
    if [ -f "$SHELL_CONTEXT" ]; then
        [ "$(cat "$SHELL_CONTEXT")" = "$EXPECTED_SHELL_CONTEXT" ] ||
            die "HOME or ZDOTDIR changed; rerun with the original shell context to safely remove old integration"
    fi
elif [ -f "$SHELL_CONTEXT" ]; then
    die "this prefix has user shell integration; use --user and the original HOME/ZDOTDIR"
fi
path_blocks() {
    "$USER_INSTALL" || return 0
    local mode="$1" rc temporary quoted body
    quoted="${PATH_FRAGMENT//\'/\'\\\'\'}"
    body=". '$quoted'"
    for rc in "${RC_FILES[@]}"; do
        [ ! -L "$rc" ] || die "refusing to edit a symlink: $rc"
        [ ! -e "$rc" ] || [ -f "$rc" ] || die "shell configuration is not a regular file: $rc"
        [ -f "$rc" ] || continue
        [ -w "$rc" ] || die "shell configuration is not writable: $rc"
        # Do not rewrite unrelated files (including their original final newline).
        grep -Fxq -- "$BLOCK_START" "$rc" || continue
        temporary="$(mktemp)"
        XENTERM_BLOCK_START="$BLOCK_START" XENTERM_BLOCK_END="$BLOCK_END" XENTERM_BLOCK_BODY="$body" awk '
            BEGIN { start=ENVIRON["XENTERM_BLOCK_START"]; end=ENVIRON["XENTERM_BLOCK_END"]; body=ENVIRON["XENTERM_BLOCK_BODY"] }
            $0 == start { if (seen++ || state) exit 1; state=1; next }
            state == 1 { if ($0 != body) exit 1; state=2; next }
            state == 2 { if ($0 != end) exit 1; state=0; next }
            { print }
            END { if (state) exit 1 }
        ' "$rc" > "$temporary" || { rm -- "$temporary"; die "modified or malformed managed PATH block in $rc"; }
        if [ "$mode" = remove ]; then
            # Preserve permissions and unrelated shell content.
            cat "$temporary" > "$rc"
        fi
        rm -- "$temporary"
    done
}

SUDO=()
if "$UNINSTALL"; then
    [ -f "$MANIFEST" ] || { echo "No managed installation at $PREFIX"; exit 0; }
    if [ ! -w "$STATE_DIR" ]; then
        "$USER_INSTALL" && die "installation is not writable"
        command -v sudo >/dev/null 2>&1 || die "sudo required to uninstall this installation"
        sudo -v; SUDO=(sudo)
    fi
    # Keep the manifest when a file changed so a later retry can still verify it.
    path_blocks verify
    remove_owned_files || exit 1
    path_blocks remove
    "${SUDO[@]}" rm -f -- "$MANIFEST" "$OPTIONS" "$SHELL_CONTEXT"
    echo "Uninstalled unchanged managed files from $PREFIX"
    exit 0
fi

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

# Avoid replacing a foreign executable. The legacy launcher is the migration
# marker for installations made before the manifest was introduced.
if { [ -e "$BIN_DIR/xenterm" ] || [ -L "$BIN_DIR/xenterm" ]; } && [ ! -f "$MANIFEST" ] && [ ! -f "$DESKTOP" ]; then
    die "unmanaged executable already exists: $BIN_DIR/xenterm"
fi
if "$USER_INSTALL"; then
    path_blocks verify
    if [ "$DESKTOP_LINK" = 1 ] && [ -n "$USER_DESKTOP_DIR" ]; then
        shortcut="$USER_DESKTOP_DIR/xenterm.desktop"
        if [ -e "$shortcut" ] || [ -L "$shortcut" ]; then
            [ ! -L "$shortcut" ] && grep -qx 'X-XenTerm-Installer=true' "$shortcut" ||
                die "unmanaged desktop shortcut exists: $shortcut"
        fi
    fi
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
BINARY_TMP="$(mktemp)"
# Stage the source before removing owned old files, including an invocation
# that explicitly passes the currently installed executable as its source.
install -m755 -- "$BIN" "$BINARY_TMP"
trap 'rm -f "$DESKTOP_TMP" "$BINARY_TMP" "${MANIFEST_TMP:-}"' EXIT
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
"${SUDO[@]}" install -d -- "$BIN_DIR" "$ICON_DIR" "$APP_DIR" "$STATE_DIR"
# Remove only verified prior artifacts before recreating this installation.
# A modified file blocks the upgrade rather than being silently overwritten.
remove_owned_files || die "modified managed files prevent upgrade; review them first"
path_blocks remove
"${SUDO[@]}" install -m755 -- "$BINARY_TMP" "$BIN_DIR/xenterm"
if [ -f "$ICON_SRC" ]; then
    "${SUDO[@]}" install -m644 -- "$ICON_SRC" "$ICON_DIR/xenterm.png"
else
    echo "warning: icon not found ($ICON_SRC); the desktop entry will use a generic icon" >&2
fi
if [ "$MENU_LINK" = 1 ]; then
    "${SUDO[@]}" install -m644 -- "$DESKTOP_TMP" "$DESKTOP"
fi
MANIFEST_TMP="$(mktemp)"
record_owned() {
    local sum
    sum="$(sha256sum < "$1")"; sum="${sum%% *}"
    printf '%s\t%s\n' "$sum" "$1" >> "$MANIFEST_TMP"
}
record_owned "$BIN_DIR/xenterm"
[ ! -f "$ICON_SRC" ] || record_owned "$ICON_DIR/xenterm.png"
[ "$MENU_LINK" != 1 ] || record_owned "$DESKTOP"
if [ "$DESKTOP_LINK" = 1 ] && [ -n "$USER_DESKTOP_DIR" ]; then
    install -d -- "$USER_DESKTOP_DIR"
    install -m755 -- "$DESKTOP_TMP" "$USER_DESKTOP_DIR/xenterm.desktop"
    record_owned "$USER_DESKTOP_DIR/xenterm.desktop"
fi
if [ "$ADD_PATH" = 1 ]; then
    # POSIX quoting also works in bash and zsh; no eval of a user-supplied path.
    quoted="${BIN_DIR//\'/\'\\\'\'}"
    printf "xenterm_bin='%s'\n" "$quoted" > "$PATH_FRAGMENT"
    cat >> "$PATH_FRAGMENT" <<'PATH_EOF'
case ":$PATH:" in
    *:"$xenterm_bin":*) ;;
    *) PATH="$xenterm_bin:$PATH"; export PATH ;;
esac
unset xenterm_bin
PATH_EOF
    record_owned "$PATH_FRAGMENT"
    quoted="${PATH_FRAGMENT//\'/\'\\\'\'}"
    for rc in "${RC_INSTALL_FILES[@]}"; do
        printf '\n%s\n. %s\n%s\n' "$BLOCK_START" "'$quoted'" "$BLOCK_END" >> "$rc"
    done
fi
printf 'path=%s\ndesktop=%s\nmenu=%s\n' "$ADD_PATH" "$DESKTOP_LINK" "$MENU_LINK" > "$DESKTOP_TMP"
"${SUDO[@]}" install -m644 -- "$DESKTOP_TMP" "$OPTIONS"
"${SUDO[@]}" install -m644 -- "$MANIFEST_TMP" "$MANIFEST"
if "$USER_INSTALL"; then
    printf '%s\n' "$EXPECTED_SHELL_CONTEXT" > "$SHELL_CONTEXT"
fi
rm -- "$MANIFEST_TMP"

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
