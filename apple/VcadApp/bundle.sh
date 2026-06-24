#!/usr/bin/env bash
# Assemble a runnable vcad.app from the SwiftPM build. RealityKit's Metal view
# renders black from a bare executable; it needs a real .app bundle (Info.plist +
# ad-hoc codesign) to get a working drawable. Run this, then `open dist/vcad.app`.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

echo "swift build…"
swift build --package-path "$here" -c "${CONFIG:-debug}"
bin="$(swift build --package-path "$here" -c "${CONFIG:-debug}" --show-bin-path)/VcadApp"

app="$here/dist/vcad.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/vcad"

cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>vcad</string>
  <key>CFBundleDisplayName</key><string>vcad</string>
  <key>CFBundleIdentifier</key><string>io.vcad.m0</string>
  <key>CFBundleExecutable</key><string>vcad</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleShortVersionString</key><string>0.1</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.graphics-design</string>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$app" >/dev/null 2>&1 || echo "(codesign skipped)"
echo "built: $app"
