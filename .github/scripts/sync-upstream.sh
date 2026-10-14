#!/usr/bin/env bash
set -euo pipefail

# Historical filename retained for dispatch links. This is a read-only check:
# never push refs, merge, or run files fetched from upstream.
mirror_url=https://github.com/xue-ding-e/XenTerm-Next.git
upstream_url=https://github.com/ixbaicn/XenTerm.git
mirror=refs/heads/upstream-main
report() {
  printf '%s\n' "$1"
  if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    printf '%s\n\n' "$1" >> "$GITHUB_STEP_SUMMARY"
  fi
}
report 'Official mirror drift check (read-only; no synchronization is performed).'
expected=$(git ls-remote --exit-code "$mirror_url" "$mirror" | cut -f1)
[[ "$expected" =~ ^[0-9a-f]{40}$ ]] || { report 'Missing or invalid mirror baseline'; exit 1; }
# Fetch into a disposable bare repository, not the maintained checkout.
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
git init --bare --quiet "$work"
git -C "$work" fetch --quiet --no-tags "$mirror_url" "$mirror"
[[ "$(git -C "$work" rev-parse FETCH_HEAD)" == "$expected" ]] || { report 'Mirror changed during fetch; retry after review'; exit 1; }
git -C "$work" fetch --quiet --no-tags "$upstream_url" refs/heads/main
candidate=$(git -C "$work" rev-parse FETCH_HEAD)
[[ "$(git ls-remote --exit-code "$mirror_url" "$mirror" | cut -f1)" == "$expected" ]] || { report 'Mirror changed concurrently; retry after review'; exit 1; }
[[ "$(git ls-remote --exit-code "$upstream_url" refs/heads/main | cut -f1)" == "$candidate" ]] || { report 'Official main changed concurrently; retry after review'; exit 1; }
report "Mirror SHA: $expected"
report "Official SHA: $candidate"
if [[ "$candidate" == "$expected" ]]; then
  report 'Status: current. No refs were changed.'
elif git -C "$work" merge-base --is-ancestor "$expected" "$candidate"; then
  count=$(git -C "$work" rev-list --count "$expected..$candidate")
  report "Status: behind by $count commit(s). Synchronization is pending; use the existing authorized connector after ancestry and expected-SHA checks. No refs were changed."
  echo '::warning::Official mirror is behind; this read-only check does not synchronize it.'
else
  report 'Status: diverged or upstream history rewritten. Manual review required; no refs were changed.'
  exit 1
fi
