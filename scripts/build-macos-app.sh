#!/bin/sh
set -eu

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
# Code signing identity (Developer ID Application certificate name).
# Set via environment variable or fall back to auto-detect.
#   export CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
CODESIGN_IDENTITY="${CODESIGN_IDENTITY:-}"

# Apple ID credentials for notarization.
#   export APPLE_ID="you@example.com"
#   export APPLE_TEAM_ID="XXXXXXXXXX"
#   export APPLE_APP_PASSWORD="xxxx-xxxx-xxxx-xxxx"  (app-specific password)
APPLE_ID="${APPLE_ID:-}"
APPLE_TEAM_ID="${APPLE_TEAM_ID:-}"
APPLE_APP_PASSWORD="${APPLE_APP_PASSWORD:-}"

# Set to "1" to skip signing/notarization (useful for local dev builds).
SKIP_SIGNING="${SKIP_SIGNING:-0}"

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
ROOT_DIR="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
APP_DIR="$ROOT_DIR/dist/Autohour.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"
ZIP_PATH="$ROOT_DIR/dist/Autohour.app.zip"

cd "$ROOT_DIR"

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
cargo build --release

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

cp "$ROOT_DIR/packaging/macos/Info.plist" "$CONTENTS_DIR/Info.plist"
cp "$ROOT_DIR/target/release/autohour" "$MACOS_DIR/Autohour"
cp -R "$ROOT_DIR/holidays" "$RESOURCES_DIR/holidays"
cp "$ROOT_DIR/.env.example" "$RESOURCES_DIR/.env.example"

if [ -f "$ROOT_DIR/packaging/macos/AppIcon.icns" ]; then
  cp "$ROOT_DIR/packaging/macos/AppIcon.icns" "$RESOURCES_DIR/AppIcon.icns"
fi

chmod +x "$MACOS_DIR/Autohour"

if [ -f "$ROOT_DIR/.env" ]; then
  cp "$ROOT_DIR/.env" "$RESOURCES_DIR/.env"
fi

# ---------------------------------------------------------------------------
# Code Signing
# ---------------------------------------------------------------------------
if [ "$SKIP_SIGNING" = "1" ]; then
  printf '[sign] Skipping code signing (SKIP_SIGNING=1)\n'
else
  # Auto-detect signing identity if not explicitly provided
  if [ -z "$CODESIGN_IDENTITY" ]; then
    CODESIGN_IDENTITY="$(security find-identity -v -p codesigning | \
      grep 'Developer ID Application' | head -1 | \
      sed 's/.*"\(.*\)".*/\1/' || true)"
  fi

  if [ -z "$CODESIGN_IDENTITY" ]; then
    printf '[sign] WARNING: No Developer ID Application certificate found.\n'
    printf '[sign] The app will not be signed. Users will see Gatekeeper warnings.\n'
    printf '[sign] To fix: install a Developer ID certificate or set CODESIGN_IDENTITY.\n'
  else
    printf '[sign] Signing with: %s\n' "$CODESIGN_IDENTITY"

    # Sign all nested components first, then the app bundle itself.
    # --options runtime enables the hardened runtime (required for notarization).
    # --force re-signs even if already signed.
    # --timestamp uses Apple's timestamp server (required for notarization).
    codesign --force --options runtime --timestamp \
      --sign "$CODESIGN_IDENTITY" \
      "$MACOS_DIR/Autohour"

    codesign --force --options runtime --timestamp \
      --sign "$CODESIGN_IDENTITY" \
      "$APP_DIR"

    # Verify the signature
    codesign --verify --deep --strict "$APP_DIR"
    printf '[sign] Signature verified OK\n'
  fi
fi

# ---------------------------------------------------------------------------
# Create zip archive
# ---------------------------------------------------------------------------
mkdir -p "$ROOT_DIR/dist"
ditto -c -k --sequesterRsrc --keepParent "$APP_DIR" "$ZIP_PATH"

# ---------------------------------------------------------------------------
# Notarization
# ---------------------------------------------------------------------------
if [ "$SKIP_SIGNING" = "1" ]; then
  printf '[notarize] Skipping notarization (SKIP_SIGNING=1)\n'
elif [ -z "${CODESIGN_IDENTITY:-}" ] || [ -z "$APPLE_ID" ] || [ -z "$APPLE_TEAM_ID" ] || [ -z "$APPLE_APP_PASSWORD" ]; then
  printf '[notarize] Skipping notarization (credentials not configured).\n'
  printf '[notarize] Set APPLE_ID, APPLE_TEAM_ID, and APPLE_APP_PASSWORD to enable.\n'
else
  printf '[notarize] Submitting for notarization...\n'

  xcrun notarytool submit "$ZIP_PATH" \
    --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_PASSWORD" \
    --wait

  printf '[notarize] Stapling notarization ticket to app...\n'
  xcrun stapler staple "$APP_DIR"

  # Re-create the zip with the stapled ticket
  rm -f "$ZIP_PATH"
  ditto -c -k --sequesterRsrc --keepParent "$APP_DIR" "$ZIP_PATH"

  printf '[notarize] Notarization complete\n'
fi

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------
printf 'Built app bundle: %s\n' "$APP_DIR"
printf 'Built zip archive: %s\n' "$ZIP_PATH"
