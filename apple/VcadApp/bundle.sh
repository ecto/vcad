#!/usr/bin/env bash
# Assemble a runnable vcad.app from the SwiftPM build. RealityKit's Metal view
# renders black from a bare executable; it needs a real .app bundle (Info.plist +
# ad-hoc codesign) to get a working drawable. Run this, then `open dist/vcad.app`.
#
# The bundle declares the `.vcad` document type (and `.loon` source), so Finder
# routes double-clicks here, shows a document icon, and offers "Open With".
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

echo "swift build…"
swift build --package-path "$here" -c "${CONFIG:-debug}"
bin="$(swift build --package-path "$here" -c "${CONFIG:-debug}" --show-bin-path)/VcadApp"

app="$here/dist/vcad.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/vcad"

# App + document icons, drawn by the app's own icon script when missing.
if [ ! -f "$here/dist/AppIcon.icns" ]; then
  swift "$here/make-icon.swift" "$here/dist" >/dev/null 2>&1 || echo "(icon generation skipped)"
fi
for icon in AppIcon DocumentIcon; do
  [ -f "$here/dist/$icon.icns" ] && cp "$here/dist/$icon.icns" "$app/Contents/Resources/"
done

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
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticTermination</key><false/>
  <key>NSLocalNetworkUsageDescription</key><string>Connect to your CNC controller to show machine status and send toolpaths.</string>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.graphics-design</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>vcad Document</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>CFBundleTypeIconFile</key><string>DocumentIcon</string>
      <key>LSHandlerRank</key><string>Owner</string>
      <key>LSItemContentTypes</key><array><string>io.vcad.document</string></array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>loon Source</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key><array><string>io.vcad.loon</string></array>
    </dict>
  </array>
  <key>UTExportedTypeDeclarations</key>
  <array>
    <dict>
      <key>UTTypeIdentifier</key><string>io.vcad.document</string>
      <key>UTTypeDescription</key><string>vcad Document</string>
      <key>UTTypeConformsTo</key><array><string>public.json</string></array>
      <key>UTTypeTagSpecification</key>
      <dict>
        <key>public.filename-extension</key><array><string>vcad</string></array>
        <key>public.mime-type</key><array><string>application/vnd.vcad+json</string></array>
      </dict>
    </dict>
    <dict>
      <key>UTTypeIdentifier</key><string>io.vcad.loon</string>
      <key>UTTypeDescription</key><string>loon Source</string>
      <key>UTTypeConformsTo</key><array><string>public.source-code</string></array>
      <key>UTTypeTagSpecification</key>
      <dict>
        <key>public.filename-extension</key><array><string>loon</string></array>
      </dict>
    </dict>
  </array>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$app" >/dev/null 2>&1 || echo "(codesign skipped)"
echo "built: $app"
