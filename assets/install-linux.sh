#!/usr/bin/env bash
#
# XenTerm Linux installer for the Flutter desktop release bundle.
#
# The release tarball is self-contained: `xenterm` sits next to `lib/` and
# `data/`, so the whole bundle is copied under <prefix>/lib/xenterm/app/ and a
# tiny wrapper in <prefix>/bin makes `xenterm` available on PATH. Optional
# per-user integration adds a Start-menu entry, an XDG desktop shortcut and a
# managed PATH block in the user's shell configuration.
#
# Usage:
#   ./install-linux.sh [--user] [--prefix DIR] [--bundle DIR]
#                      [--[no-]add-to-path] [--[no-]desktop-shortcut]
#                      [--[no-]start-menu] [--uninstall]
#
# Run it from the extracted release folder (the default bundle is the directory
# containing this script), then start a new shell and run `xenterm`.
#
# Security model:
#   * The prefix is canonicalised; control characters and '=' are rejected so a
#     path can never inject desktop-entry keys.
#   * Installer state lives in <prefix>/share/xenterm/installer/ and is never
#     sourced as shell code. It is a numeric options record plus a SHA-256
#     manifest of every installed file.
#   * --uninstall removes only files whose SHA-256 still matches the manifest;
#     a modified or replaced (symlink) file preserves all files and exits
#     non-zero so a half-uninstall cannot happen.
#   * System installs never guess a logged-in desktop user: PATH and XDG desktop
#     integration require --user. User installs never invoke sudo.
#
# Layout (under the selected prefix):
#   bin/xenterm                                        wrapper -> lib/xenterm/app/xenterm
#   lib/xenterm/app/                                   the complete release bundle
#   share/applications/xenterm.desktop                 application menu entry
#   share/icons/hicolor/512x512/apps/xenterm.png       application icon
#   share/xenterm/installer/{options,files.sha256,shell-context,path.sh}

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
  cat <<'EOF'
Usage: install-linux.sh [OPTIONS]

Install the XenTerm desktop bundle for the current user or for all users.

  --user                     Install under $HOME/.local and enable per-user
                             integration (Start menu, desktop shortcut, PATH).
                             This mode never invokes sudo.
  --prefix DIR               Install under DIR (also accepts --prefix=DIR).
                             Default: /usr/local, or $HOME/.local with --user.
  --[no-]add-to-path         Manage the user's shell PATH (default: on with
                             --user, off for system installs).
  --[no-]desktop-shortcut    Copy a launcher to the XDG desktop (default: on
                             with --user, off for system installs).
  --[no-]start-menu          Install the application menu entry (default: on).
  --bundle DIR               Use DIR as the release bundle. It must contain the
                             `xenterm` executable plus `lib/` and `data/`.
                             Default: the directory containing this script.
  --uninstall                Remove the files this installer owns, if they are
                             unchanged since installation.
  -h, --help                 Show this help.

User integration uses the invoking user's HOME. Existing choices are retained
on repeat installs unless an option explicitly overrides them. Shell changes
apply to new bash/zsh/login shells. Paths with spaces and non-ASCII characters
are supported; the prefix may not contain '=' or control characters.

Examples:
  ./install-linux.sh --user
  ./install-linux.sh --user --prefix "$HOME/opt/xenterm"
  sudo ./install-linux.sh --prefix /opt/xenterm --no-add-to-path
  ./install-linux.sh --user --uninstall
EOF
}

die() { echo "error: $*" >&2; exit 1; }

USER_INSTALL=false
UNINSTALL=false
CLI_PREFIX=""
BUNDLE_ARG=""
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
    --bundle)
      [ "$#" -ge 2 ] && [ -n "$2" ] || die "--bundle requires a directory"
      BUNDLE_ARG="$2"
      shift 2
      ;;
    --bundle=*)
      BUNDLE_ARG="${1#*=}"
      [ -n "$BUNDLE_ARG" ] || die "--bundle requires a directory"
      shift
      ;;
    --prefix)
      [ "$#" -ge 2 ] && [ -n "$2" ] || die "--prefix requires a directory"
      CLI_PREFIX="$2"
      shift 2
      ;;
    --prefix=*)
      CLI_PREFIX="${1#*=}"
      [ -n "$CLI_PREFIX" ] || die "--prefix requires a directory"
      shift
      ;;
    -h|--help) usage; exit 0 ;;
    --)
      shift
      [ "$#" -eq 0 ] || die "unexpected argument: $1 (use --bundle DIR for the release folder)"
      ;;
    -*) die "unknown option: $1 (use --help for usage)" ;;
    *) die "unexpected argument: $1 (use --bundle DIR for the release folder)" ;;
  esac
done

if [ -n "$CLI_PREFIX" ]; then
  PREFIX="$CLI_PREFIX"
elif [ -n "${PREFIX:-}" ]; then
  PREFIX="$PREFIX"
elif "$USER_INSTALL"; then
  [ -n "${HOME:-}" ] || die "HOME must be set for --user"
  PREFIX="$HOME/.local"
else
  PREFIX="/usr/local"
fi

# Desktop Exec paths cannot contain '='. Reject line/control characters rather
# than allowing a path to inject desktop-entry keys. Validate before any writes.
[[ "$PREFIX" != *'='* && "$PREFIX" != *[[:cntrl:]]* ]] ||
  die "the installation prefix cannot contain '=' or control characters"
PREFIX="$(readlink -m -- "$PREFIX")"
[[ "$PREFIX" != *'='* && "$PREFIX" != *[[:cntrl:]]* ]] ||
  die "the resolved installation prefix cannot contain '=' or control characters"
[ "$PREFIX" != "/" ] || die "refusing to install into /; choose a real prefix"

if "$USER_INSTALL"; then
  if [ -z "${HOME:-}" ] || [[ "$HOME" != /* || "$HOME" == *[[:cntrl:]]* ]]; then
    die "a valid absolute HOME is required for --user"
  fi
fi

# Keep installer state separate from application profiles. Never source state as
# shell code: it is a small numeric options record plus a SHA-256 file manifest.
STATE_DIR="${PREFIX%/}/share/xenterm/installer"
OPTIONS="$STATE_DIR/options"
MANIFEST="$STATE_DIR/files.sha256"
SHELL_CONTEXT="$STATE_DIR/shell-context"
PATH_FRAGMENT="$STATE_DIR/path.sh"

APP_DIR="${PREFIX%/}/lib/xenterm/app"
BIN_DIR="${PREFIX%/}/bin"
WRAPPER="$BIN_DIR/xenterm"
ICON_DIR="${PREFIX%/}/share/icons/hicolor/512x512/apps"
ICON_FILE="$ICON_DIR/xenterm.png"
APP_MENU_DIR="${PREFIX%/}/share/applications"
DESKTOP="$APP_MENU_DIR/xenterm.desktop"

for managed in "$STATE_DIR" "$OPTIONS" "$MANIFEST" "$SHELL_CONTEXT" "$PATH_FRAGMENT"; do
  [ "$(readlink -m -- "$managed")" = "$managed" ] || die "installer state traverses symlinks: $managed"
  [ ! -L "$managed" ] || die "refusing symlink installer state: $managed"
done

PREVIOUS_PATH=0
if [ -f "$OPTIONS" ]; then
  while IFS='=' read -r key value; do
    [[ "$value" == 0 || "$value" == 1 ]] || die "invalid installer options: $OPTIONS"
    case "$key" in
      path) PREVIOUS_PATH="$value"; [ -n "$ADD_PATH" ] || ADD_PATH="$value" ;;
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
  die "PATH and desktop integration require --user; system installs only place files under the prefix"
fi

# The XDG utility resolves the localized desktop folder without evaluating user
# code. A headless user has no desktop directory; that is not an error.
USER_DESKTOP_DIR=""
if "$USER_INSTALL" && command -v xdg-user-dir >/dev/null 2>&1; then
  USER_DESKTOP_DIR="$(xdg-user-dir DESKTOP)"
fi
if [ "$USER_DESKTOP_DIR" = "$HOME" ] || [ "$USER_DESKTOP_DIR" = "$HOME/" ]; then
  USER_DESKTOP_DIR=""
fi
if [ -n "$USER_DESKTOP_DIR" ]; then
  [[ "$(readlink -m -- "$USER_DESKTOP_DIR")" = "$USER_DESKTOP_DIR" ]] ||
    die "the XDG desktop must be canonical and must not traverse symlinks"
  [[ "$USER_DESKTOP_DIR" = "$HOME/"* && "$USER_DESKTOP_DIR" != *[[:cntrl:]]* ]] ||
    die "the XDG desktop must be inside HOME"
fi
if [ "$DESKTOP_LINK" = 1 ] && [ -z "$USER_DESKTOP_DIR" ]; then
  echo "warning: no XDG desktop configured; desktop shortcut skipped" >&2
fi

BLOCK_START="# >>> XenTerm installer PATH: $PREFIX >>>"
BLOCK_END="# <<< XenTerm installer PATH: $PREFIX <<<"

# Shell configuration is only inspected for user installs. The shell context
# record prevents editing another user's rc files after HOME/ZDOTDIR changed.
RC_FILES=()
RC_INSTALL_FILES=()
EXPECTED_SHELL_CONTEXT=""
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
  die "this prefix has user shell integration; use --user with the original HOME/ZDOTDIR"
fi

# Only delete an exact, unchanged installer-owned file. Reject manifest entries
# outside the fixed package layout; a modified manifest is not deletion authority.
owned_path() {
  case "$1" in
    "$WRAPPER"|"$ICON_FILE"|"$DESKTOP"|"$PATH_FRAGMENT") return 0 ;;
  esac
  case "$1" in
    "$APP_DIR"/*) return 0 ;;
  esac
  "$USER_INSTALL" && [ -n "$USER_DESKTOP_DIR" ] &&
    [ "$1" = "$USER_DESKTOP_DIR/xenterm.desktop" ]
}

remove_owned_files() {
  [ -f "$MANIFEST" ] || return 0
  local digest path current failed=0
  local -a verified=()
  while IFS=$'\t' read -r digest path; do
    [[ "$digest" =~ ^[0-9a-f]{64}$ ]] && owned_path "$path" ||
      die "invalid installer manifest (unexpected path; desktop location may have changed)"
    [ "$(readlink -m -- "$path")" = "$path" ] || die "managed path now traverses a symlink: $path"
    if [ -e "$path" ] || [ -L "$path" ]; then
      if [ -L "$path" ] || [ ! -f "$path" ]; then
        echo "preserved replaced file: $path" >&2
        failed=1
        continue
      fi
      current="$(sha256sum < "$path")"
      current="${current%% *}"
      if [ "$current" = "$digest" ]; then
        verified+=("$path")
      else
        echo "preserved modified file: $path" >&2
        failed=1
      fi
    fi
  done < "$MANIFEST"
  [ "$failed" = 0 ] || return 1
  # Validate the entire manifest before deleting anything. A changed file must
  # not cause a half-uninstalled application or a dangling PATH source.
  for path in "${verified[@]}"; do
    "${SUDO[@]}" rm -- "$path"
  done
}

# Reject unsafe shell configurations before the first write. path_blocks()
# repeats these checks when it edits, but a symlinked/non-regular rc file must
# fail an install before any staged or installed file is created.
validate_shell_configuration() {
  "$USER_INSTALL" || return 0
  [ "$ADD_PATH" = 1 ] || [ "$PREVIOUS_PATH" = 1 ] || return 0
  local rc
  for rc in "${RC_FILES[@]}"; do
    [ ! -L "$rc" ] || die "refusing to edit a symlink: $rc"
    [ ! -e "$rc" ] || [ -f "$rc" ] || die "shell configuration is not a regular file: $rc"
    [ ! -f "$rc" ] || [ -w "$rc" ] || die "shell configuration is not writable: $rc"
    [ ! -f "$rc" ] || [ -r "$rc" ] || die "shell configuration is not readable: $rc"
  done
}

# Add or remove the managed PATH block. Only files whose block is well-formed
# are edited; unrelated content (including the final newline) is preserved.
path_blocks() {
  "$USER_INSTALL" || return 0
  [ "$ADD_PATH" = 1 ] || [ "$PREVIOUS_PATH" = 1 ] || return 0
  local mode="$1" rc temporary quoted body
  quoted="${PATH_FRAGMENT//\'/\'\\\'\'}"
  body=". '$quoted'"
  for rc in "${RC_FILES[@]}"; do
    [ ! -L "$rc" ] || die "refusing to edit a symlink: $rc"
    [ ! -e "$rc" ] || [ -f "$rc" ] || die "shell configuration is not a regular file: $rc"
    [ -f "$rc" ] || continue
    [ -w "$rc" ] || die "shell configuration is not writable: $rc"
    [ -r "$rc" ] || die "shell configuration is not readable: $rc"
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
      cat "$temporary" > "$rc"
    fi
    rm -- "$temporary"
  done
}

remove_empty_dirs() {
  local dir="$1" entry
  [ -d "$dir" ] && [ ! -L "$dir" ] || return 0
  for entry in "$dir"/* "$dir"/.[!.]* "$dir"/..?*; do
    [ -e "$entry" ] || [ -L "$entry" ] || continue
    [ -d "$entry" ] && [ ! -L "$entry" ] && remove_empty_dirs "$entry"
  done
  rmdir -- "$dir" 2>/dev/null || true
}

directory_writable() {
  local directory="$1"
  while [ ! -e "$directory" ] && [ ! -L "$directory" ]; do
    directory="$(dirname -- "$directory")"
  done
  [ -d "$directory" ] && [ -w "$directory" ] && [ -x "$directory" ]
}

# Deleting a file needs a writable parent directory, not a writable file; check
# every managed file up front so sudo is chosen once for the whole uninstall.
uninstall_needs_sudo() {
  local digest path parent
  [ -w "$STATE_DIR" ] || return 0
  while IFS=$'\t' read -r digest path; do
    [ -e "$path" ] || [ -L "$path" ] || continue
    parent="${path%/*}"
    [ -w "$parent" ] || return 0
  done < "$MANIFEST"
  return 1
}

SUDO=()
if "$UNINSTALL"; then
  if [ ! -f "$MANIFEST" ]; then
    echo "No managed installation at $PREFIX"
    exit 0
  fi
  if [ "$(id -u)" -ne 0 ] && uninstall_needs_sudo; then
    "$USER_INSTALL" && die "installation is not writable"
    command -v sudo >/dev/null 2>&1 || die "sudo required to uninstall this installation"
    sudo -v
    SUDO=(sudo)
  fi
  # Verify everything (manifest entries and managed PATH blocks) before the
  # first removal; a changed file blocks the uninstall as a whole.
  path_blocks verify
  remove_owned_files || exit 1
  path_blocks remove
  "${SUDO[@]}" rm -f -- "$MANIFEST" "$OPTIONS" "$SHELL_CONTEXT"
  remove_empty_dirs "$APP_DIR"
  rmdir -- "${PREFIX%/}/lib/xenterm" 2>/dev/null || true
  remove_empty_dirs "$STATE_DIR"
  rmdir -- "${PREFIX%/}/share/xenterm" 2>/dev/null || true
  echo "Uninstalled unchanged managed files from $PREFIX"
  exit 0
fi

# Resolve the release bundle: --bundle wins, otherwise the script's directory.
BUNDLE="${BUNDLE_ARG:-$SCRIPT_DIR}"
BUNDLE="$(readlink -f -- "$BUNDLE")" || die "could not resolve the bundle path: ${BUNDLE_ARG:-$SCRIPT_DIR}"
[ -d "$BUNDLE" ] || die "release bundle not found: $BUNDLE (extract the tarball and run ./install-linux.sh from it)"
[ -f "$BUNDLE/xenterm" ] || die "release bundle is incomplete: missing $BUNDLE/xenterm"
[ -d "$BUNDLE/lib" ] || die "release bundle is incomplete: missing $BUNDLE/lib/"
[ -d "$BUNDLE/data" ] || die "release bundle is incomplete: missing $BUNDLE/data/"
ICON_SRC="$BUNDLE/icon@512.png"
[ -f "$ICON_SRC" ] || ICON_SRC="$SCRIPT_DIR/icon@512.png"
[ -f "$ICON_SRC" ] || ICON_SRC=""

validate_shell_configuration

# The prefix itself is resolved above. Each child destination must still stay
# inside that resolved layout; a redirected bin/icons directory is not ours.
for target in "$BIN_DIR" "$ICON_DIR" "$APP_MENU_DIR" "$APP_DIR" "$STATE_DIR"; do
  [ "$(readlink -m -- "$target")" = "$target" ] || die "installation directory traverses a symlink: $target"
  [ ! -e "$target" ] || [ -d "$target" ] || die "installation directory is not a directory: $target"
done
for target in "$WRAPPER" "$ICON_FILE" "$DESKTOP" "$PATH_FRAGMENT"; do
  [ "$(readlink -m -- "$target")" = "$target" ] || die "installation file traverses a symlink: $target"
done

# Do not replace a foreign wrapper, icon or launcher. A manifest from a previous
# install is what makes an existing destination ours; upgrades still verify each
# file against that manifest before overwriting anything.
if [ ! -f "$MANIFEST" ]; then
  if [ -e "$WRAPPER" ] || [ -L "$WRAPPER" ]; then
    die "unmanaged executable already exists: $WRAPPER; move it away before installing"
  fi
  if [ -e "$ICON_FILE" ] || [ -L "$ICON_FILE" ]; then
    die "unmanaged icon already exists: $ICON_FILE; move it away before installing"
  fi
  if [ -e "$DESKTOP" ] || [ -L "$DESKTOP" ]; then
    die "launcher already exists: $DESKTOP; review and move it before retrying"
  fi
fi
if "$USER_INSTALL" && [ "$DESKTOP_LINK" = 1 ] && [ -n "$USER_DESKTOP_DIR" ]; then
  shortcut="$USER_DESKTOP_DIR/xenterm.desktop"
  if [ -e "$shortcut" ] || [ -L "$shortcut" ]; then
    [ ! -L "$shortcut" ] && [ -f "$MANIFEST" ] ||
      die "unmanaged desktop shortcut exists: $shortcut; review and move it before retrying"
  fi
fi

# Escalation is decided once, up front: --user never escalates, and a system
# install only asks for sudo when the selected prefix is not writable.
SUDO=()
if [ "$(id -u)" -ne 0 ]; then
  NEED_SUDO=false
  for directory in "$BIN_DIR" "$ICON_DIR" "$APP_MENU_DIR" "$APP_DIR" "$STATE_DIR"; do
    directory_writable "$directory" || NEED_SUDO=true
  done
  for file in "$WRAPPER" "$ICON_FILE" "$DESKTOP" "$PATH_FRAGMENT"; do
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

APP_EXE="$APP_DIR/xenterm"
EXEC_VALUE="$(desktop_exec "$WRAPPER")"
# GIO checks that Exec's first word exists before expanding %% field escapes.
# Keep literal percent signs in an argument instead; env invokes it without a
# shell and the absolute installed path still does not depend on PATH.
if [[ "$BIN_DIR" == *%* ]]; then
  EXEC_VALUE="/usr/bin/env -- $EXEC_VALUE"
fi
ICON_VALUE="xenterm"
if [ -n "$ICON_SRC" ]; then
  # Custom prefixes need not be on the desktop's icon search path.
  ICON_VALUE="$(desktop_string "$ICON_FILE")"
fi

STAGE="$(mktemp -d)"
WRAPPER_TMP="$(mktemp)"
DESKTOP_TMP="$(mktemp)"
OPTIONS_TMP="$(mktemp)"
MANIFEST_TMP="$(mktemp)"
SHELL_CONTEXT_TMP="$(mktemp)"
trap 'rm -rf -- "$STAGE"; rm -f -- "$WRAPPER_TMP" "$DESKTOP_TMP" "$OPTIONS_TMP" "$MANIFEST_TMP" "$SHELL_CONTEXT_TMP"' EXIT

# Copy the bundle tree with install(1). Every file is staged first so that
# reinstalling from the currently installed bundle would not delete its own
# source. Root-level `config/` and `log/` are runtime profile data created by a
# portable launch; they are never installed.
copy_tree() {
  local src="$1" dst="$2" entry name target
  local -a install_cmd=()
  if [ "$3" = sudo ]; then
    install_cmd=("${SUDO[@]}" install)
  else
    install_cmd=(install)
  fi
  for entry in "$src"/* "$src"/.[!.]* "$src"/..?*; do
    [ -e "$entry" ] || [ -L "$entry" ] || continue
    name="${entry##*/}"
    target="$dst/$name"
    [ ! -L "$entry" ] || die "the release bundle contains a symlink: $entry"
    if [ -d "$entry" ]; then
      if [ "$src" = "$BUNDLE" ] && { [ "$name" = config ] || [ "$name" = log ]; }; then
        continue
      fi
      "${install_cmd[@]}" -d -- "$target"
      copy_tree "$entry" "$target" "$3"
    elif [ -f "$entry" ]; then
      if [ "$name" = xenterm ] && { [ "$src" = "$BUNDLE" ] || [ "$src" = "$STAGE" ]; }; then
        "${install_cmd[@]}" -m755 -- "$entry" "$target"
      else
        "${install_cmd[@]}" -m644 -- "$entry" "$target"
      fi
    else
      die "unsupported file type in the release bundle: $entry"
    fi
  done
}

record_file() {
  local sum
  [ -f "$1" ] || die "cannot record missing installer file: $1"
  sum="$(sha256sum < "$1")"
  sum="${sum%% *}"
  printf '%s\t%s\n' "$sum" "$1" >> "$MANIFEST_TMP"
}

# Manifest entries are tab separated; a file name that breaks the format would
# make uninstall unsafe, so refuse it up front.
walk_record() {
  local dir="$1" entry name
  for entry in "$dir"/* "$dir"/.[!.]* "$dir"/..?*; do
    [ -e "$entry" ] || [ -L "$entry" ] || continue
    name="${entry##*/}"
    [[ "$name" != *[[:cntrl:]]* ]] || die "unsupported control character in installed file name: $entry"
    [ ! -L "$entry" ] || die "installed path became a symlink: $entry"
    if [ -d "$entry" ]; then
      walk_record "$entry"
    elif [ -f "$entry" ]; then
      record_file "$entry"
    else
      die "unsupported file type in the installation: $entry"
    fi
  done
}

# A tiny POSIX wrapper keeps the Flutter bundle directory intact (the engine
# resolves `lib/` and `data/` relative to the real executable).
{
  printf '#!/bin/sh\n'
  printf '# Generated by the XenTerm Linux installer; reinstalling rewrites it.\n'
  printf "exec '%s' \"\$@\"\n" "${APP_EXE//\'/\'\\\'\'}"
} > "$WRAPPER_TMP"

cat > "$DESKTOP_TMP" <<EOF
[Desktop Entry]
Type=Application
Name=XenTerm
GenericName=SSH Client
Comment=SSH, SFTP and terminal client with a Rust core
Comment[zh_CN]=Rust 核心的 SSH / SFTP / 终端客户端
Exec=$EXEC_VALUE
Icon=$ICON_VALUE
Terminal=false
Categories=Network;TerminalEmulator;
Keywords=ssh;sftp;terminal;shell;
StartupNotify=true
StartupWMClass=xenterm
X-XenTerm-Installer=true
EOF

# Stage the bundle before touching the installation so an upgrade cannot read
# files it is about to replace.
copy_tree "$BUNDLE" "$STAGE" no-sudo

"${SUDO[@]}" install -d -- "$BIN_DIR" "$ICON_DIR" "$APP_MENU_DIR" "$STATE_DIR"
"${SUDO[@]}" install -d -- "$APP_DIR"

# Remove only verified prior artifacts before recreating this installation.
# A modified file blocks the upgrade rather than being silently overwritten.
remove_owned_files || die "modified managed files prevent upgrade; review them first"
path_blocks remove

copy_tree "$STAGE" "$APP_DIR" sudo
"${SUDO[@]}" install -m755 -- "$WRAPPER_TMP" "$WRAPPER"
if [ -n "$ICON_SRC" ]; then
  "${SUDO[@]}" install -m644 -- "$ICON_SRC" "$ICON_FILE"
else
  echo "warning: icon not found ($BUNDLE/icon@512.png); the desktop entry will use a generic icon" >&2
fi
if [ "$MENU_LINK" = 1 ]; then
  "${SUDO[@]}" install -m644 -- "$DESKTOP_TMP" "$DESKTOP"
fi

# Record every installed file, then persist the options record. The manifest is
# written before shell integration so a failure there leaves a retryable state.
: > "$MANIFEST_TMP"
walk_record "$APP_DIR"
record_file "$WRAPPER"
[ -z "$ICON_SRC" ] || record_file "$ICON_FILE"
[ "$MENU_LINK" != 1 ] || record_file "$DESKTOP"

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
  record_file "$PATH_FRAGMENT"
fi

if [ "$DESKTOP_LINK" = 1 ] && [ -n "$USER_DESKTOP_DIR" ]; then
  install -d -- "$USER_DESKTOP_DIR"
  install -m644 -- "$DESKTOP_TMP" "$USER_DESKTOP_DIR/xenterm.desktop"
  record_file "$USER_DESKTOP_DIR/xenterm.desktop"
fi

printf 'path=%s\ndesktop=%s\nmenu=%s\n' "$ADD_PATH" "$DESKTOP_LINK" "$MENU_LINK" > "$OPTIONS_TMP"
"${SUDO[@]}" install -m644 -- "$OPTIONS_TMP" "$OPTIONS"
"${SUDO[@]}" install -m644 -- "$MANIFEST_TMP" "$MANIFEST"
if "$USER_INSTALL"; then
  printf '%s\n' "$EXPECTED_SHELL_CONTEXT" > "$SHELL_CONTEXT_TMP"
  "${SUDO[@]}" install -m644 -- "$SHELL_CONTEXT_TMP" "$SHELL_CONTEXT"
fi

# Shell integration last: the manifest above already describes the files, so a
# failure here leaves a state a rerun can repair instead of a half-uninstall.
# The block is appended without an extra blank line so a repeat install leaves
# the rc file byte-identical to its pre-install content.
if [ "$ADD_PATH" = 1 ]; then
  quoted="${PATH_FRAGMENT//\'/\'\\\'\'}"
  for rc in "${RC_INSTALL_FILES[@]}"; do
    # Keep trailing newlines: command substitution strips them, so append a
    # sentinel character and remove it again. A missing rc file starts empty.
    existing=""
    if [ -f "$rc" ]; then
      existing="$( { cat -- "$rc" 2>/dev/null || true; }; printf x)"
      existing="${existing%x}"
    fi
    separator=""
    if [ -n "$existing" ] && [[ "$existing" != *$'\n' ]]; then
      separator=$'\n'
    fi
    printf '%s%s\n. %s\n%s\n' "$separator" "$BLOCK_START" "'$quoted'" "$BLOCK_END" >> "$rc"
  done
fi

if "$USER_INSTALL" && [ -n "${HOME:-}" ]; then
  USER_DESKTOP="${XDG_DATA_HOME:-$HOME/.local/share}/applications/xenterm.desktop"
  if [ -f "$USER_DESKTOP" ] && [ ! "$USER_DESKTOP" -ef "$DESKTOP" ]; then
    echo "warning: a user launcher may override this installation: $USER_DESKTOP" >&2
  fi
fi

# Refresh the desktop + icon caches (best-effort; harmless if the tools are absent).
if [ "$MENU_LINK" = 1 ] && command -v update-desktop-database >/dev/null 2>&1; then
  "${SUDO[@]}" update-desktop-database "$APP_MENU_DIR" 2>/dev/null || true
fi
if [ -n "$ICON_SRC" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then
  "${SUDO[@]}" gtk-update-icon-cache -f -t "${PREFIX%/}/share/icons/hicolor" 2>/dev/null || true
fi

echo "Installed XenTerm:"
echo "  app     -> $APP_DIR"
echo "  exec    -> $WRAPPER"
if [ "$MENU_LINK" = 1 ]; then
  echo "  menu    -> $DESKTOP"
fi
if [ "$DESKTOP_LINK" = 1 ] && [ -n "$USER_DESKTOP_DIR" ]; then
  echo "  desktop -> $USER_DESKTOP_DIR/xenterm.desktop"
fi
if [ "$ADD_PATH" = 1 ]; then
  echo "  PATH    -> $PATH_FRAGMENT (open a new shell to pick it up)"
else
  echo "Add $BIN_DIR to PATH or run $WRAPPER directly."
fi
echo
echo "If the dock still shows the generic icon, log out/in (Wayland) or run"
echo "'killall -3 gnome-shell' (X11) to refresh the shell."
