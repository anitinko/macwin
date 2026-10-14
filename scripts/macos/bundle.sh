#!/usr/bin/env bash
# Builds target/MacWin.app around the release `macwin` binary and signs it.
#
# macOS ties privacy permissions (Accessibility, Input Monitoring, ...) to the
# code signature, so a STABLE identity keeps them across rebuilds:
#   MACWIN_SIGN_IDENTITY="Apple Development: Name (TEAMID)" scripts/macos/bundle.sh
# Without it, the first "Apple Development" identity in the keychain is used,
# and as a last resort an ad-hoc signature (permissions then reset on every
# rebuild). MACWIN_PROFILE picks the Cargo profile (default `release`;
# dmg.sh uses the stripped `dist`).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
APP="$ROOT/target/MacWin.app"

# rust-toolchain.toml pins 1.96.0; use `stable` when it is that version and
# the pinned toolchain is not installed (never auto-install).
if [[ -z "${RUSTUP_TOOLCHAIN:-}" ]] && command -v rustup >/dev/null; then
    if ! rustup toolchain list | grep -q '^1\.96\.0'; then
        if rustup run stable rustc --version 2>/dev/null | grep -q '^rustc 1\.96\.0 '; then
            export RUSTUP_TOOLCHAIN=stable
        fi
    fi
fi

VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
PROFILE="${MACWIN_PROFILE:-release}"
OUT="target/$PROFILE"
[[ "$PROFILE" == dev ]] && OUT=target/debug
cargo build --profile "$PROFILE" -p macwin-agent --bin macwin

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$OUT/macwin" "$APP/Contents/MacOS/macwin"
cp THIRD_PARTY_NOTICES "$APP/Contents/Resources/THIRD_PARTY_NOTICES"
sed "s/@VERSION@/$VERSION/g" scripts/macos/Info.plist.in >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist" >/dev/null

IDENTITY="${MACWIN_SIGN_IDENTITY:-}"
if [[ -z "$IDENTITY" ]]; then
    IDENTITY="$(security find-identity -v -p codesigning | sed -n 's/.*"\(Apple Development: [^"]*\)".*/\1/p' | head -1)"
fi
if [[ -z "$IDENTITY" ]]; then
    echo "WARNING: no signing identity found; signing ad-hoc." >&2
    echo "         macOS will forget the granted permissions after every rebuild." >&2
    IDENTITY="-"
fi

codesign --force --options runtime --timestamp=none --sign "$IDENTITY" "$APP"
codesign --verify --strict --verbose=2 "$APP"
echo "built $APP (version $VERSION, signed with: $IDENTITY)"
