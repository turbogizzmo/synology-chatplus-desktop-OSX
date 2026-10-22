#!/usr/bin/env bash
# Packages a signed universal release build into release-artifacts/:
#   ChatPlus-Desktop-macOS.dmg              what people download
#   ChatPlus-Desktop-macOS.app.tar.gz(.sig) what installed apps update from
#   latest.json / latest-prerelease.json    the updater manifest
#   SHA256SUMS.txt
# Run after: CHATPLUS_RELEASE_BUILD=1 npm run tauri build -- --target universal-apple-darwin --bundles app
set -euo pipefail

version=$(node -p 'require("./package.json").version')
# Honor CARGO_TARGET_DIR: building outside iCloud-synced folders avoids File Provider
# tagging the .app with FinderInfo, which codesign rejects.
bundle="${CARGO_TARGET_DIR:-src-tauri/target}/universal-apple-darwin/release/bundle/macos"
app="$bundle/ChatPlus Desktop.app"
out=release-artifacts

for required in "$app" "$app.tar.gz" "$app.tar.gz.sig"; do
  [ -e "$required" ] || { echo "Missing $required; build with CHATPLUS_RELEASE_BUILD=1 first." >&2; exit 1; }
done
[ "$(lipo -archs "$app/Contents/MacOS/chatplus-desktop")" = "x86_64 arm64" ] || {
  echo "Expected a universal (x86_64 arm64) binary." >&2
  exit 1
}

rm -rf "$out"
mkdir -p "$out"
cp "$app.tar.gz" "$out/ChatPlus-Desktop-macOS.app.tar.gz"
cp "$app.tar.gz.sig" "$out/ChatPlus-Desktop-macOS.app.tar.gz.sig"

# hdiutil directly: Tauri's bundle_dmg.sh scripts Finder, which fails without a GUI session.
staging=$(mktemp -d)
notes=$(mktemp)
trap 'rm -rf "$staging" "$notes"' EXIT
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
hdiutil create -quiet -volname "ChatPlus Desktop" -srcfolder "$staging" -ov -format UDZO \
  "$out/ChatPlus-Desktop-macOS.dmg"
hdiutil verify -quiet "$out/ChatPlus-Desktop-macOS.dmg"

printf '%s\n' "${RELEASE_NOTES:-ChatPlus Desktop $version for macOS.}" > "$notes"
# One universal archive serves both architectures.
node scripts/updater-metadata.mjs "$out" "$version" "$notes" \
  darwin-aarch64=ChatPlus-Desktop-macOS.app.tar.gz \
  darwin-x86_64=ChatPlus-Desktop-macOS.app.tar.gz
node scripts/checksums.mjs "$out"
ls -la "$out"
