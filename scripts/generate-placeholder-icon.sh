#!/bin/sh
set -eu

ROOT_DIR="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
ICONSET_DIR="$ROOT_DIR/packaging/macos/AppIcon.iconset"
ICNS_PATH="$ROOT_DIR/packaging/macos/AppIcon.icns"

rm -rf "$ICONSET_DIR"
mkdir -p "$ICONSET_DIR"

cat > "$ICONSET_DIR/icon_512x512.svg" <<'SVG'
<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 512 512">
  <rect width="512" height="512" rx="112" fill="#0f172a"/>
  <rect x="104" y="116" width="304" height="280" rx="40" fill="#f8fafc"/>
  <rect x="140" y="164" width="148" height="24" rx="12" fill="#0f172a" opacity="0.18"/>
  <rect x="140" y="214" width="232" height="24" rx="12" fill="#0f172a" opacity="0.12"/>
  <rect x="140" y="264" width="184" height="24" rx="12" fill="#0f172a" opacity="0.12"/>
  <path d="M318 314l34 34 62-74" fill="none" stroke="#16a34a" stroke-width="28" stroke-linecap="round" stroke-linejoin="round"/>
</svg>
SVG

qlmanage -t -s 1024 -o "$ICONSET_DIR" "$ICONSET_DIR/icon_512x512.svg" >/dev/null
mv "$ICONSET_DIR/icon_512x512.svg.png" "$ICONSET_DIR/icon_512x512@2x.png"
sips -z 512 512 "$ICONSET_DIR/icon_512x512@2x.png" --out "$ICONSET_DIR/icon_512x512.png" >/dev/null
sips -z 256 256 "$ICONSET_DIR/icon_512x512@2x.png" --out "$ICONSET_DIR/icon_256x256.png" >/dev/null
sips -z 512 512 "$ICONSET_DIR/icon_512x512@2x.png" --out "$ICONSET_DIR/icon_256x256@2x.png" >/dev/null
sips -z 128 128 "$ICONSET_DIR/icon_512x512@2x.png" --out "$ICONSET_DIR/icon_128x128.png" >/dev/null
sips -z 256 256 "$ICONSET_DIR/icon_512x512@2x.png" --out "$ICONSET_DIR/icon_128x128@2x.png" >/dev/null
sips -z 32 32 "$ICONSET_DIR/icon_512x512@2x.png" --out "$ICONSET_DIR/icon_32x32.png" >/dev/null
sips -z 64 64 "$ICONSET_DIR/icon_512x512@2x.png" --out "$ICONSET_DIR/icon_32x32@2x.png" >/dev/null
sips -z 16 16 "$ICONSET_DIR/icon_512x512@2x.png" --out "$ICONSET_DIR/icon_16x16.png" >/dev/null
sips -z 32 32 "$ICONSET_DIR/icon_512x512@2x.png" --out "$ICONSET_DIR/icon_16x16@2x.png" >/dev/null
iconutil -c icns "$ICONSET_DIR" -o "$ICNS_PATH"

printf 'Built icon: %s\n' "$ICNS_PATH"
