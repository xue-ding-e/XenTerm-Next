#!/bin/bash
# Build on macOS with Apple's tools. This never installs the package.
set -euo pipefail
if [[ $# != 3 ]]; then echo "Usage: $0 xenterm.app VERSION OUTPUT.pkg" >&2; exit 2; fi
[[ $(uname -s) == Darwin ]] || { echo 'pkgbuild/productbuild require macOS' >&2; exit 1; }
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
python3 "$here/prepare.py" "$1" "$2" "$work/stage"
stage="$work/stage"
pkgbuild --root "$stage/payload" --component-plist "$stage/components.plist" \
    --scripts "$stage/app-scripts" --identifier dev.xenterm.installer.app \
    --version "$2" --install-location / "$stage/app.pkg"
pkgbuild --nopayload --scripts "$stage/cli-scripts" --identifier dev.xenterm.installer.cli \
    --version "$2" "$stage/cli.pkg"
args=()
if [[ -n ${XENTERM_INSTALLER_SIGN_IDENTITY:-} ]]; then
    args+=(--sign "$XENTERM_INSTALLER_SIGN_IDENTITY")
fi
productbuild --distribution "$stage/distribution.xml" --resources "$stage/resources" \
    --package-path "$stage" "${args[@]}" "$3"
echo "Built $3; installer signing identity: ${XENTERM_INSTALLER_SIGN_IDENTITY:-none (unsigned)}"
