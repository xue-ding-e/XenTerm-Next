#!/usr/bin/env bash
# Plan only: run after targeted and ui:: regressions, in the same serial CI job.
# Xvfb and the application must share the runner's IPC namespace.
set -euo pipefail
binary="$PWD/target/x86_64-unknown-linux-gnu/debug/xenterm"
root="$(mktemp -d "$RUNNER_TEMP/xenterm-native-XXXXXX")"
export HOME="$root/home" XDG_CONFIG_HOME="$root/config" XDG_DATA_HOME="$root/data" XDG_CACHE_HOME="$root/cache" XDG_RUNTIME_DIR="$root/runtime"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
git rev-parse HEAD > native-source.txt
sha256sum "$binary" > native-binary-sha256.txt
"$binary" --version > native-version.txt
export XENTERM_SMOKE_BINARY="$binary" XENTERM_SMOKE_PROFILE="$root/profile"
timeout 45 xvfb-run -a -s '-screen 0 1280x900x24' bash <<'NATIVE'
set -euo pipefail
"$XENTERM_SMOKE_BINARY" --data-dir "$XENTERM_SMOKE_PROFILE" > native-startup.log 2>&1 &
app=$!
trap 'kill "$app" 2>/dev/null || true; wait "$app" 2>/dev/null || true' EXIT
visible=0
for attempt in $(seq 1 30); do
  kill -0 "$app" || { cat native-startup.log; exit 1; }
  xwininfo -root -tree > native-windows.txt
  if grep -F '"XenTerm"' native-windows.txt; then visible=1; break; fi
  sleep 0.5
done
test "$visible" -eq 1
window_id=$(awk '/"XenTerm"/ {print $1; exit}' native-windows.txt)
xwininfo -id "$window_id" -all > native-window-details.txt
cat native-window-details.txt
sleep 2
kill -0 "$app"
xwd -root -silent -out native-root.xwd
convert native-root.xwd native-root.png
xwd -id "$window_id" -silent -out native-window.xwd
convert native-window.xwd native-window.png
# A title in the X tree proves creation, not mapping or rendered pixels.
identify -format 'root colors=%k\n' native-root.png > native-pixels.txt
identify -format 'window colors=%k\n' native-window.png >> native-pixels.txt
cat native-pixels.txt
colors=$(identify -format %k native-window.png)
test "$colors" -gt 1 || { echo "Native window screenshot is monochrome; rendering remains unverified"; exit 1; }
NATIVE
