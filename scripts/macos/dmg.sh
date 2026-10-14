#!/usr/bin/env bash
# Builds target/MacWin-<version>.dmg: the signed MacWin.app (stripped `dist`
# build) next to a link to /Applications, for drag-and-drop installation.
#
#   scripts/macos/dmg.sh
#
# The app is signed like by bundle.sh. Other Macs open it without warnings
# only when it is signed with a "Developer ID Application" identity and
# notarized (xcrun notarytool submit ... --wait; xcrun stapler staple),
# which needs a paid Apple developer account and is not done here.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

MACWIN_PROFILE=dist scripts/macos/bundle.sh
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
DMG="target/MacWin-$VERSION.dmg"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
ditto target/MacWin.app "$STAGE/MacWin.app"
ln -s /Applications "$STAGE/Applications"

rm -f "$DMG"
hdiutil create -volname "MacWin $VERSION" -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov "$DMG" >/dev/null
hdiutil verify "$DMG" >/dev/null
echo "built $DMG"
