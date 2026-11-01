#!/bin/bash
# Real Apple packaging verification. No installer -pkg, sudo or system changes.
set -euo pipefail
[[ $(uname -s) == Darwin ]] || { echo 'macOS required'; exit 1; }
repo=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
app="$work/xenterm.app"
mkdir -p "$app/Contents/MacOS"
printf '#!/bin/sh\necho fixture\n' > "$app/Contents/MacOS/xenterm"
chmod +x "$app/Contents/MacOS/xenterm"
sed 's/__VERSION__/0.0.1/g' "$repo/assets/Info.plist" > "$app/Contents/Info.plist"
bash "$repo/packaging/macos/build-pkg.sh" "$app" 0.0.1 "$work/xenterm.pkg"
pkgutil --expand-full "$work/xenterm.pkg" "$work/expanded"
test -f "$work/expanded/app.pkg/Payload/Applications/xenterm.app/Contents/MacOS/xenterm"
test -f "$work/expanded/cli.pkg/Scripts/postinstall"
installer -showChoiceChangesXML -pkg "$work/xenterm.pkg" -target / > "$work/choices.plist"
python3 - "$work/choices.plist" <<'PY'
import plistlib, sys
choices = plistlib.load(open(sys.argv[1], 'rb'))
assert any(c.get('choiceIdentifier') == 'cli' and c.get('choiceAttribute') == 'selected' and c.get('attributeSetting') in (True, 1) for c in choices), choices
PY
python3 - "$work/disable-cli.plist" <<'PYCHOICE'
import plistlib, sys
with open(sys.argv[1], 'wb') as stream:
    plistlib.dump([dict(choiceIdentifier='cli', choiceAttribute='selected', attributeSetting=0)], stream)
PYCHOICE
installer -showChoicesAfterApplyingChangesXML "$work/disable-cli.plist" -pkg "$work/xenterm.pkg" -target / > "$work/disabled.plist"
python3 - "$work/disabled.plist" <<'PYCHECK'
import plistlib, sys
choices = plistlib.load(open(sys.argv[1], 'rb'))
assert any(c.get('choiceIdentifier') == 'cli' and c.get('choiceAttribute') == 'selected' and c.get('attributeSetting') in (False, 0) for c in choices), choices
PYCHECK
printf '%s\n' 'Apple package creation, payload expansion and default choice inspection passed (not installed).'
