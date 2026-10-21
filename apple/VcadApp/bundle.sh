#!/usr/bin/env bash
# Assemble a runnable vcad.app from the SwiftPM build. RealityKit's Metal view
# renders black from a bare executable; it needs a real .app bundle (Info.plist +
# ad-hoc codesign) to get a working drawable. Run this, then `open dist/vcad.app`.
#
# The bundle declares the `.vcad` document type (and `.loon` source), so Finder
# routes double-clicks here, shows a document icon, and offers "Open With".
#
# Release knobs (all optional; without them the bundle is ad-hoc signed, as
# before). See docs/mac-release.md.
#   VCAD_SIGN_IDENTITY     Developer ID Application identity -> hardened-runtime
#                          signing with VcadApp.entitlements. Makes this a
#                          release build: SPARKLE_PUBLIC_ED_KEY becomes required.
#   SPARKLE_PUBLIC_ED_KEY  EdDSA public key for update verification (SUPublicEDKey).
#   VCAD_VERSION           CFBundleShortVersionString (default 0.1)
#   VCAD_BUILD             CFBundleVersion, monotonically increasing (default 1)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

version="${VCAD_VERSION:-0.1}"
build_number="${VCAD_BUILD:-1}"
sign_identity="${VCAD_SIGN_IDENTITY:-}"
ed_key="${SPARKLE_PUBLIC_ED_KEY:-}"
ed_key="${ed_key//[[:space:]]/}"
release_build=false
if [ -n "$sign_identity" ]; then
  release_build=true
  if [ -z "$ed_key" ]; then
    echo "error: VCAD_SIGN_IDENTITY is set (release build) but SPARKLE_PUBLIC_ED_KEY is empty." >&2
    echo "       A release without the EdDSA public key could never verify its updates." >&2
    echo "       Print it with: generate_keys -p   (see docs/mac-release.md)" >&2
    exit 1
  fi
fi

echo "swift build…"
swift build --package-path "$here" -c "${CONFIG:-debug}"
bin="$(swift build --package-path "$here" -c "${CONFIG:-debug}" --show-bin-path)/VcadApp"

app="$here/dist/vcad.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/vcad"

# irys (the in-process path tracer) is dlopen()ed, not linked: bundle the
# dylib when one is built, and the app finds it in Contents/Frameworks.
# IRYS_FFI overrides the location; without it the Path-Traced View says why.
irys_dylib="${IRYS_FFI:-$HOME/Developer/irys/target/release/libirys_ffi.dylib}"
if [ -f "$irys_dylib" ]; then
  mkdir -p "$app/Contents/Frameworks"
  cp "$irys_dylib" "$app/Contents/Frameworks/libirys_ffi.dylib"
else
  echo "(irys not bundled: $irys_dylib missing — Path-Traced View disabled)"
fi

# Sparkle (auto-update). SwiftPM leaves the framework next to the binary; the
# executable's rpath points at Contents/Frameworks.
bin_dir="$(dirname "$bin")"
if [ ! -d "$bin_dir/Sparkle.framework" ]; then
  echo "error: Sparkle.framework not found in $bin_dir" >&2
  exit 1
fi
mkdir -p "$app/Contents/Frameworks"
ditto "$bin_dir/Sparkle.framework" "$app/Contents/Frameworks/Sparkle.framework"

# Sample projects for the welcome window (symlinks in the checkout are
# copied as the files they point at).
if [ -f "$here/Samples/samples.json" ]; then
  mkdir -p "$app/Contents/Resources/samples"
  cp -RL "$here/Samples/." "$app/Contents/Resources/samples/"
fi

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
  <key>CFBundleVersion</key><string>@BUILD@</string>
  <key>CFBundleShortVersionString</key><string>@VERSION@</string>
  <key>SUFeedURL</key><string>https://vcad.io/desktop/appcast.xml</string>
  <key>SUEnableAutomaticChecks</key><true/>
  <key>SUScheduledCheckInterval</key><integer>86400</integer>
  <key>VCADReleaseBuild</key><@RELEASE@/>@EDKEY@
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

plist="$app/Contents/Info.plist"
edkey_entry=""
if [ -n "$ed_key" ]; then
  edkey_entry="
  <key>SUPublicEDKey</key><string>${ed_key}</string>"
fi
python3 - "$plist" "$version" "$build_number" "$release_build" "$edkey_entry" <<'PY'
import sys
path, version, build, release, edkey = sys.argv[1:]
s = open(path).read()
s = (s.replace("@VERSION@", version).replace("@BUILD@", build)
      .replace("@RELEASE@", "true" if release == "true" else "false")
      .replace("@EDKEY@", edkey))
open(path, "w").write(s)
PY
plutil -lint "$plist" >/dev/null

if [ -z "$sign_identity" ]; then
  codesign --force --deep --sign - "$app" >/dev/null 2>&1 || echo "(codesign skipped)"
  echo "built: $app"
  exit 0
fi

# Developer ID signing, inside-out (no --deep: it mis-signs Sparkle's XPC
# services). Hardened runtime + secure timestamp are notarization requirements.
echo "signing with: $sign_identity"
sign() { codesign --force --timestamp --options runtime --sign "$sign_identity" "$@"; }
sparkle="$app/Contents/Frameworks/Sparkle.framework"
sign "$sparkle/Versions/B/XPCServices/Installer.xpc"
sign --preserve-metadata=entitlements "$sparkle/Versions/B/XPCServices/Downloader.xpc"
sign "$sparkle/Versions/B/Autoupdate"
sign "$sparkle/Versions/B/Updater.app"
sign "$sparkle"
if [ -f "$app/Contents/Frameworks/libirys_ffi.dylib" ]; then
  sign "$app/Contents/Frameworks/libirys_ffi.dylib"
fi
sign --entitlements "$here/VcadApp.entitlements" "$app"
codesign --verify --strict --deep --verbose=2 "$app"
echo "built (signed): $app"
