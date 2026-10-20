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
timeout --kill-after=5s 75s xvfb-run -a -s '-screen 0 1280x900x24' bash <<'NATIVE'
set -euo pipefail
app= control= wm=
cleanup() {
  for pid in "$app" "$control" "$wm"; do
    if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  done
}
trap cleanup EXIT
echo "stage=window-manager-start"
openbox --sm-disable > native-wm.log 2>&1 &
wm=$!
for attempt in $(seq 1 30); do
  kill -0 "$wm"
  xprop -root _NET_SUPPORTING_WM_CHECK > native-wm-ready.txt
  if grep -Eq 'window id # 0x[1-9a-fA-F]' native-wm-ready.txt; then break; fi
  sleep 0.1
done
grep -Eq 'window id # 0x[1-9a-fA-F]' native-wm-ready.txt
echo "stage=control-window-map-and-render"
xmessage -name xenterm-control -title "XenTerm Smoke Control" -buttons OK:0 -default OK -geometry 400x120+50+50 "Synthetic native window control" > native-control.log 2>&1 &
control=$!
control_id=
for attempt in $(seq 1 30); do
  kill -0 "$control"
  xwininfo -root -tree > native-control-tree.txt
  control_id=$(awk '/"XenTerm Smoke Control"/ {print $1; exit}' native-control-tree.txt)
  if [ -n "$control_id" ]; then
    xwininfo -id "$control_id" -all > native-control-details.txt
    if grep -q 'Map State: IsViewable' native-control-details.txt; then break; fi
  fi
  sleep 0.1
done
test -n "$control_id"
grep -q 'Map State: IsViewable' native-control-details.txt
xwd -id "$control_id" -silent -out native-control.xwd
convert native-control.xwd native-control.png
test "$(identify -format %k native-control.png)" -gt 1
cat native-control-details.txt
kill "$control" 2>/dev/null || true
wait "$control" 2>/dev/null || true
control=
echo "stage=xenterm-start-map-and-render"
"$XENTERM_SMOKE_BINARY" --data-dir "$XENTERM_SMOKE_PROFILE" > native-startup.log 2>&1 &
app=$!
visible=0
for attempt in $(seq 1 30); do
  kill -0 "$app" || { cat native-startup.log; exit 1; }
  xwininfo -root -tree > native-windows.txt
  window_id=$(awk '/"XenTerm"/ {print $1; exit}' native-windows.txt)
  if [ -n "$window_id" ]; then
    xwininfo -id "$window_id" -all > native-window-details.txt
    if grep -q 'Map State: IsViewable' native-window-details.txt; then visible=1; break; fi
  fi
  sleep 0.5
done
cat native-window-details.txt 2>/dev/null || true
if [ -z "${window_id:-}" ]; then
  cat native-startup.log
  echo "No XenTerm window was created"
  exit 1
fi
xwininfo -id "$window_id" -all > native-window-details.txt
cat native-window-details.txt
# Keep the same WM/driver/product and observe the first client frame for at most
# 30 seconds. A software GPU can take longer than two seconds to paint its UI.
started=$SECONDS
colors=1
: > native-pixels.txt
while true; do
  kill -0 "$app"
  elapsed=$((SECONDS - started))
  printf -v sample '%02d' "$elapsed"
  xwd -id "$window_id" -silent -out native-window.xwd
  convert native-window.xwd "native-frame-$sample.png"
  cp "native-frame-$sample.png" native-window.png
  colors=$(identify -format %k native-window.png)
  printf 'elapsed_seconds=%s client_colors=%s\n' "$elapsed" "$colors" | tee -a native-pixels.txt
  if [ "$colors" -gt 1 ] || [ "$elapsed" -ge 30 ]; then break; fi
  sleep 2
done
xwd -root -silent -out native-root.xwd
convert native-root.xwd native-root.png
identify -format 'root colors=%k\n' native-root.png >> native-pixels.txt
cat native-pixels.txt
test "$visible" -eq 1 || { echo "Native window was created but remained unmapped"; exit 1; }
test "$colors" -gt 1 || { echo "Native client remained monochrome after the bounded first-frame observation"; exit 1; }

NATIVE
