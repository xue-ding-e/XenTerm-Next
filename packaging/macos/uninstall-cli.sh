#!/bin/sh
# Run with sudo on macOS. App data, saved sessions and the .app are untouched.
set -eu
[ "$(uname -s)" = Darwin ] || { echo 'This command is for macOS' >&2; exit 1; }
[ "$(id -u)" = 0 ] || { echo 'Run with sudo to remove the system CLI entry' >&2; exit 1; }
for path in /usr /usr/local /usr/local/bin; do
    [ ! -L "$path" ] || { echo "Refusing redirected path: $path" >&2; exit 1; }
done
link=/usr/local/bin/xenterm
target=/Applications/xenterm.app/Contents/MacOS/xenterm
if [ -L "$link" ] && [ "$(readlink "$link")" = "$target" ]; then
    rm "$link"
elif [ -e "$link" ] || [ -L "$link" ]; then
    echo 'Foreign xenterm command; not removed' >&2; exit 1
fi
pkgutil --forget dev.xenterm.installer.cli >/dev/null 2>&1 || true
printf '%s\n' 'CLI entry removed. To uninstall the app, move /Applications/xenterm.app to Trash in Finder.'
