#!/usr/bin/env bash
# macOS codesign + notarytool submit + stapler + DMG packaging.
# Called from .github/workflows/release.yml on macos runners.
#
# Required env vars (set from GitHub Secrets in the workflow):
#   APPLE_CERT_P12_BASE64     base64-encoded Developer ID .p12
#   APPLE_CERT_PASSWORD       password for the .p12
#   APPLE_SIGNING_IDENTITY    "Developer ID Application: Jane Doe (TEAMID12)"
#   APPLE_ID                  Apple ID email
#   APPLE_APP_PASSWORD        app-specific password from appleid.apple.com
#   APPLE_TEAM_ID             10-char team ID
#   TARGET                    Rust target triple (set by the matrix)
set -euo pipefail

APP_PATH="bins/hop/target/${TARGET}/release/bundle/osx/Hop.app"

# ── Import Developer ID certificate ─────────────────────────────────────────
echo "$APPLE_CERT_P12_BASE64" | base64 --decode > /tmp/hop-cert.p12
security create-keychain -p hop-ci /tmp/hop-build.keychain
security default-keychain -s /tmp/hop-build.keychain
security unlock-keychain -p hop-ci /tmp/hop-build.keychain
security import /tmp/hop-cert.p12 \
    -k /tmp/hop-build.keychain \
    -P "$APPLE_CERT_PASSWORD" \
    -T /usr/bin/codesign
security set-key-partition-list \
    -S apple-tool:,apple: \
    -s -k hop-ci /tmp/hop-build.keychain
rm /tmp/hop-cert.p12

# ── Codesign ─────────────────────────────────────────────────────────────────
codesign --deep --force --verify \
    --options runtime \
    --timestamp \
    --sign "$APPLE_SIGNING_IDENTITY" \
    "$APP_PATH"

# ── Notarize ─────────────────────────────────────────────────────────────────
ditto -c -k --keepParent "$APP_PATH" /tmp/Hop-notarize.zip
xcrun notarytool submit /tmp/Hop-notarize.zip \
    --apple-id  "$APPLE_ID" \
    --password  "$APPLE_APP_PASSWORD" \
    --team-id   "$APPLE_TEAM_ID" \
    --wait
rm /tmp/Hop-notarize.zip

xcrun stapler staple "$APP_PATH"

# ── Package as .dmg ───────────────────────────────────────────────────────────
ARCH=$(echo "$TARGET" | cut -d- -f1)   # x86_64 or aarch64
DMG_NAME="Hop-${ARCH}.dmg"
hdiutil create \
    -volname "Hop" \
    -srcfolder "$APP_PATH" \
    -ov -format UDZO \
    "$DMG_NAME"

echo "Created $DMG_NAME"
