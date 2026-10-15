#!/usr/bin/env bash
# Version resolution — the single legal assembly point for version strings
# (版本号规范 v1 §3/§4). Every caller (build.rs, CI, release tooling) gets its
# version from here; nothing else may concatenate "xt-…" strings.
#
# Inputs (environment):
#   XT_VERSION   explicit override; must already satisfy the format check or
#                the script fails — never silently downgraded (§2/§7)
#   XT_CHANNEL   force the channel during branch derivation (beta|alpha|dev);
#                ignored when XT_VERSION or a tag already decided the version
#   GITHUB_REF_TYPE / GITHUB_REF_NAME / GITHUB_SHA   CI checkout context
#
# Authority order (§2): XT_VERSION > git tag > branch derivation > dev fallback.
#
# Output: the version string as the ONLY stdout line; logs go to stderr.
# Exit 0 on success, non-zero on any failure. Side effect: writes version.json
# next to the repository root (§6; override path with XT_VERSION_JSON).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION_JSON="${XT_VERSION_JSON:-$REPO_ROOT/version.json}"

# Legal format (§1): stable xt-v<M>.<m>.<p>, prerelease xt-<channel>-<hash7>-T<unix_seconds>
VERSION_REGEX='^xt-(v[0-9]+\.[0-9]+\.[0-9]+|(beta|alpha|dev)-[0-9a-f]{7}-T[0-9]{10})$'

log()  { printf '[version.sh] %s\n' "$*" >&2; }
die()  { log "ERROR: $*"; exit 1; }

valid_version() {
    printf '%s' "$1" | grep -Eq "$VERSION_REGEX"
}

# Quote arbitrary ref metadata without adding a JSON-tool dependency to builds.
# Bash strings cannot contain NUL; escape every other JSON control byte.
json_string_or_null() {
    local value="$1" char code i
    local LC_ALL=C
    if [ -z "$value" ]; then
        printf null
        return
    fi
    printf '"'
    for ((i = 0; i < ${#value}; i++)); do
        char="${value:i:1}"
        case "$char" in
            '"') printf '\\"' ;;
            '\') printf '\\\\' ;;
            *)
                printf -v code '%d' "'$char"
                # Bash 3.2 may report high UTF-8 bytes as signed values.
                if ((code >= 0 && code < 32)); then
                    printf '\\u%04x' "$code"
                else
                    printf '%s' "$char"
                fi
                ;;
        esac
    done
    printf '"'
}

# Channel of an already-validated version string, for version.json.
channel_name() { printf '%s' "${1#xt-}" | cut -d- -f1; }

git_sha7() {
    # GITHUB_SHA is the exact commit CI checked out; locally use the full SHA
    # (rev-parse --short=7 can return MORE than 7 chars on ambiguous repos,
    # which would break the fixed-width format).
    if [ -n "${GITHUB_SHA:-}" ]; then
        printf '%s' "${GITHUB_SHA:0:7}"
    else
        git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null | cut -c1-7
    fi
}

current_branch() {
    if [ -n "${GITHUB_REF_NAME:-}" ] && [ "${GITHUB_REF_TYPE:-}" = "branch" ]; then
        printf '%s' "$GITHUB_REF_NAME"
    else
        git -C "$REPO_ROOT" symbolic-ref --quiet --short HEAD 2>/dev/null || true
    fi
}

current_tag() {
    if [ -n "${GITHUB_REF_NAME:-}" ] && [ "${GITHUB_REF_TYPE:-}" = "tag" ]; then
        printf '%s' "$GITHUB_REF_NAME"
    else
        # Only an exact tag on HEAD identifies this build; `git describe`
        # without --exact-match would name a nearby older tag.
        git -C "$REPO_ROOT" describe --tags --exact-match HEAD 2>/dev/null || true
    fi
}

cargo_semver() {
    sed -n '/^\[package\]/,/^\[/ s/^version = "\(.*\)"/\1/p' "$REPO_ROOT/Cargo.toml" | head -n 1
}

# Resolved inputs recorded for version.json.
SOURCE=""
BRANCH=""
TAG=""
CHANNEL=""
SEMVER=""      # "X.Y.Z" for stable, "" otherwise
HASH=""        # 7 hex chars for prerelease, "" otherwise
TIMESTAMP=""   # unix seconds for prerelease, "" otherwise

# ── 1. XT_VERSION override ─────────────────────────────────────────────────
if [ -n "${XT_VERSION:-}" ]; then
    valid_version "$XT_VERSION" \
        || die "XT_VERSION='${XT_VERSION}' does not match 版本号规范 v1 §1 (${VERSION_REGEX}); refusing to fall back (§7)"
    VERSION="$XT_VERSION"
    SOURCE="xt-version"
    TAG="$(current_tag)"
    if [ -n "$TAG" ]; then
        case "$TAG" in
            v*) [ "xt-$TAG" = "$VERSION" ] || log "WARNING: tag $TAG and XT_VERSION $VERSION differ; XT_VERSION wins (§2)" ;;
        esac
    fi

# ── 2. git tag → stable ────────────────────────────────────────────────────
else
    TAG="$(current_tag)"
    if [ -n "$TAG" ]; then
        case "$TAG" in
            # The rolling `beta` tag (release.yml recreates it on every main
            # push) marks a channel, not a release: pin the channel here and
            # let the branch derivation below finish the version string.
            beta)
                CHANNEL=beta
                ;;
            v[0-9]*.[0-9]*.[0-9]*)
                SEMVER="${TAG#v}"
                printf '%s' "$SEMVER" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' \
                    || die "tag $TAG is not a plain vX.Y.Z tag"
                VERSION="xt-v$SEMVER"
                SOURCE="git-tag"
                ;;
            *)
                die "tag $TAG cannot produce a legal version (must be vX.Y.Z)"
                ;;
        esac
    fi
fi

# ── 3. branch derivation (XT_CHANNEL may force the channel) ────────────────
if [ -z "${VERSION:-}" ]; then
    BRANCH="$(current_branch)"

    if [ -n "${XT_CHANNEL:-}" ]; then
        case "$XT_CHANNEL" in
            beta|alpha|dev) CHANNEL="$XT_CHANNEL" ;;
            *) die "XT_CHANNEL='${XT_CHANNEL}' is not one of beta|alpha|dev (§1)" ;;
        esac
    elif [ -z "$CHANNEL" ]; then
        # Skipped when a `beta` tag already pinned the channel above.
        case "$BRANCH" in
            release/*) CHANNEL=stable ;;
            beta*)     CHANNEL=beta ;;
            alpha*)    CHANNEL=alpha ;;
            *)         CHANNEL=dev ;;
        esac
    fi

    if [ "$CHANNEL" = "stable" ]; then
        # release/* prepares the version that release.ps1 already wrote into
        # Cargo.toml; a stable string is only authoritative once tagged, so
        # record it as branch-derived.
        SEMVER="$(cargo_semver)"
        [ -n "$SEMVER" ] || die "cannot read [package] version from Cargo.toml for release/* stable derivation"
        printf '%s' "$SEMVER" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' \
            || die "Cargo.toml version '$SEMVER' is not X.Y.Z"
        VERSION="xt-v$SEMVER"
        SOURCE="branch"
    else
        HASH="$(git_sha7 || true)"
        if [ -z "$HASH" ]; then
            # dev 兜底 (§2): only the implicit dev channel may fall back;
            # an explicitly requested beta/alpha must fail instead (§7).
            if [ "$CHANNEL" != "dev" ]; then
                die "channel '$CHANNEL' requested but no git commit hash is available"
            fi
            HASH="0000000"
            SOURCE="dev-fallback"
            log "WARNING: no git metadata — dev fallback hash 0000000"
        else
            SOURCE="branch"
        fi
        TIMESTAMP="$(date -u +%s)"
        VERSION="xt-$CHANNEL-$HASH-T$TIMESTAMP"
    fi
fi

# ── 4. Final self-check (§7): invalid results fail the build, never downgrade ─
valid_version "$VERSION" || die "assembled version '$VERSION' is invalid (this is a bug in version.sh)"

# Fill the recorded fields for version.json from the final string.
if [ -z "$CHANNEL" ]; then
    case "$VERSION" in
        xt-v*)  CHANNEL=stable ;;
        *)      CHANNEL="$(channel_name "$VERSION")" ;;
    esac
fi
if [ "$CHANNEL" = "stable" ]; then
    [ -n "$SEMVER" ] || SEMVER="${VERSION#xt-v}"
else
    [ -n "$HASH" ]      || HASH="$(printf '%s' "${VERSION#xt-}" | cut -d- -f2)"
    [ -n "$TIMESTAMP" ] || TIMESTAMP="$(printf '%s' "${VERSION#xt-}" | cut -d- -f3 | tr -d T)"
fi

GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
JSON_SEMVER="$([ -n "$SEMVER" ] && printf '"%s"' "$SEMVER" || printf null)"
JSON_HASH="$([ -n "$HASH" ] && printf '"%s"' "$HASH" || printf null)"
JSON_TS="$([ -n "$TIMESTAMP" ] && printf '%s' "$TIMESTAMP" || printf null)"
JSON_BRANCH="$(json_string_or_null "$BRANCH")"
JSON_TAG="$(json_string_or_null "$TAG")"

TMP_JSON="$VERSION_JSON.tmp$$"
{
    printf '{\n'
    printf '  "version": "%s",\n'  "$VERSION"
    printf '  "channel": "%s",\n'  "$CHANNEL"
    printf '  "semver": %s,\n'     "$JSON_SEMVER"
    printf '  "hash": %s,\n'       "$JSON_HASH"
    printf '  "timestamp": %s,\n'  "$JSON_TS"
    printf '  "source": "%s",\n'   "$SOURCE"
    printf '  "branch": %s,\n'     "$JSON_BRANCH"
    printf '  "tag": %s,\n'        "$JSON_TAG"
    printf '  "generated_at": "%s"\n' "$GENERATED_AT"
    printf '}\n'
} > "$TMP_JSON"
mv -f "$TMP_JSON" "$VERSION_JSON"

log "resolved $VERSION (source=$SOURCE channel=$CHANNEL)"
printf '%s\n' "$VERSION"
