#!/usr/bin/env bash
set -euo pipefail

# Only move the official mirror. Never merge or run files fetched from upstream.
mirror=refs/heads/upstream-main
expected=$(git ls-remote --exit-code origin "$mirror" | cut -f1)
[[ "$expected" =~ ^[0-9a-f]{40}$ ]] || { echo 'Missing or invalid mirror baseline'; exit 1; }
git fetch --no-tags origin "$mirror"
[[ "$(git rev-parse FETCH_HEAD)" == "$expected" ]] || { echo 'Mirror changed during fetch'; exit 1; }
git fetch --no-tags https://github.com/ixbaicn/XenTerm.git refs/heads/main
candidate=$(git rev-parse FETCH_HEAD)
git merge-base --is-ancestor "$expected" "$candidate" || { echo 'Upstream diverged or rewrote history; manual review required'; exit 1; }
[[ "$(git ls-remote --exit-code origin "$mirror" | cut -f1)" == "$expected" ]] || { echo 'Mirror changed concurrently; retry after review'; exit 1; }
if [[ "$candidate" == "$expected" ]]; then
  echo "Official mirror is already current: $expected"
  exit 0
fi
# A regular push rejects non-fast-forward updates; no force, tags, or main writes.
git push --porcelain origin "$candidate:$mirror"
actual=$(git ls-remote --exit-code origin "$mirror" | cut -f1)
[[ "$actual" == "$candidate" ]] || { echo 'Mirror changed after push; inspect concurrent update'; exit 1; }
echo "Official mirror fast-forwarded: $expected -> $actual"
