#!/usr/bin/env bash
# Build + bundle the visionOS spike for the Vision Pro SIMULATOR and install it.
# SwiftPM has no visionOS app template, so this drives swift-build with an
# explicit target triple + SDK and assembles the .app by hand (the sibling
# VcadApp/bundle.sh trick, pointed at xrsimulator). Run build-ffi.sh first.
#
#   ./bundle.sh          build + install + launch on the booted simulator
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

# swift-build's bare --triple/--sdk emits ELF-style linker args for the xros
# triple; xcodebuild drives the same package with a proper visionOS toolchain.
echo "xcodebuild (visionOS Simulator)…"
cd "$here"
xcodebuild -scheme VcadVision \
  -destination 'platform=visionOS Simulator,name=Apple Vision Pro' \
  -derivedDataPath "$here/.build/xcode" build -quiet
bin="$here/.build/xcode/Build/Products/Debug-xrsimulator/VcadVision"

app="$here/dist/vcad-vision.app"
rm -rf "$app"
mkdir -p "$app"
cp "$bin" "$app/vcad-vision"

cat > "$app/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>vcad</string>
  <key>CFBundleDisplayName</key><string>vcad</string>
  <key>CFBundleIdentifier</key><string>io.vcad.vision</string>
  <key>CFBundleExecutable</key><string>vcad-vision</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleShortVersionString</key><string>0.1</string>
  <key>MinimumOSVersion</key><string>2.0</string>
  <key>UIDeviceFamily</key><array><integer>7</integer></array>
  <key>UIApplicationSceneManifest</key>
  <dict>
    <key>UIApplicationSupportsMultipleScenes</key><true/>
    <key>UIApplicationPreferredDefaultSceneSessionRole</key>
    <string>UIWindowSceneSessionRoleVolumetricApplication</string>
  </dict>
</dict>
</plist>
PLIST

codesign --force --sign - "$app" >/dev/null
echo "built: $app"

xcrun simctl install booted "$app"
xcrun simctl launch booted io.vcad.vision
