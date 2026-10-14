#!/usr/bin/env bash
# Builds target/macwin-<version>-windows-x64.zip: macwin.exe (command line),
# macwin-tray.exe (the tray app, no console window), install.ps1,
# uninstall.ps1, README.txt and the third-party notices. Cross-builds from a
# Mac or Linux with MinGW (macOS: brew install mingw-w64):
#
#   scripts/windows/package.sh
#
# MACWIN_WINDOWS_TARGET picks another target (x86_64-pc-windows-msvc when
# building on Windows from Git Bash).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# rust-toolchain.toml pins 1.96.0; use `stable` when it is that version and
# the pinned toolchain is not installed (never auto-install).
if [[ -z "${RUSTUP_TOOLCHAIN:-}" ]] && command -v rustup >/dev/null; then
    if ! rustup toolchain list | grep -q '^1\.96\.0'; then
        if rustup run stable rustc --version 2>/dev/null | grep -q '^rustc 1\.96\.0 '; then
            export RUSTUP_TOOLCHAIN=stable
        fi
    fi
fi

TARGET="${MACWIN_WINDOWS_TARGET:-x86_64-pc-windows-gnu}"
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
NAME="macwin-$VERSION-windows-x64"
OUT="target/$TARGET/dist"
ZIP="$ROOT/target/$NAME.zip"

cargo build --profile dist --target "$TARGET" -p macwin-agent --bins

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
mkdir "$STAGE/$NAME"
cp "$OUT/macwin.exe" "$OUT/macwin-tray.exe" "$STAGE/$NAME/"
# Windows tools expect CRLF line ends in text files.
crlf() { sed 's/$/\r/' "$1" >"$2"; }
crlf scripts/windows/install.ps1 "$STAGE/$NAME/install.ps1"
crlf scripts/windows/uninstall.ps1 "$STAGE/$NAME/uninstall.ps1"
crlf scripts/windows/README.txt "$STAGE/$NAME/README.txt"
crlf THIRD_PARTY_NOTICES "$STAGE/$NAME/THIRD_PARTY_NOTICES.txt"

rm -f "$ZIP"
if command -v zip >/dev/null; then
    (cd "$STAGE" && zip -qr "$ZIP" "$NAME")
else
    (cd "$STAGE" && python3 -m zipfile -c "$ZIP" "$NAME")
fi
echo "built $ZIP"
