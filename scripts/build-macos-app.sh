#!/bin/sh
set -eu

ROOT_DIR="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
APP_DIR="$ROOT_DIR/dist/Autohour.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"

cd "$ROOT_DIR"

cargo build --release

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

cp "$ROOT_DIR/packaging/macos/Info.plist" "$CONTENTS_DIR/Info.plist"
cp "$ROOT_DIR/packaging/macos/Autohour" "$MACOS_DIR/Autohour"
cp "$ROOT_DIR/target/release/autohour" "$RESOURCES_DIR/autohour-bin"
cp -R "$ROOT_DIR/holidays" "$RESOURCES_DIR/holidays"
cp "$ROOT_DIR/.env.example" "$RESOURCES_DIR/.env.example"

if [ -f "$ROOT_DIR/packaging/macos/AppIcon.icns" ]; then
  cp "$ROOT_DIR/packaging/macos/AppIcon.icns" "$RESOURCES_DIR/AppIcon.icns"
fi

chmod +x "$MACOS_DIR/Autohour"
chmod +x "$RESOURCES_DIR/autohour-bin"

if [ -f "$ROOT_DIR/.env" ]; then
  cp "$ROOT_DIR/.env" "$RESOURCES_DIR/.env"
fi

mkdir -p "$ROOT_DIR/dist"
ditto -c -k --sequesterRsrc --keepParent "$APP_DIR" "$ROOT_DIR/dist/Autohour.app.zip"

printf 'Built app bundle: %s\n' "$APP_DIR"
printf 'Built zip archive: %s\n' "$ROOT_DIR/dist/Autohour.app.zip"
