#!/usr/bin/env bash
# Add a signed DMG to the Sparkle appcast served at https://vcad.io/desktop/appcast.xml.
#
#   make-appcast.sh <signed.dmg> <download-url-prefix> [appcast.xml]
#
# download-url-prefix: where the DMG is hosted, ending in "/", e.g.
#   https://github.com/ecto/vcad/releases/download/desktop-v0.10.0/
# appcast.xml defaults to packages/app/public/desktop/appcast.xml (the file
# Vercel serves). Existing items are kept; generate_appcast adds the new one.
#
# EdDSA signing key: by default generate_appcast reads it from the login
# Keychain (where `generate_keys` put it). In CI, set SPARKLE_ED_PRIVATE_KEY
# and it is piped on stdin — never written to disk.
#
# generate_appcast comes from SPARKLE_BIN (a dir), else the Sparkle SwiftPM
# artifact that `swift build` already fetched.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
repo_root="$(cd "$here/../.." && pwd)"

dmg="${1:?usage: make-appcast.sh <signed.dmg> <download-url-prefix> [appcast.xml]}"
prefix="${2:?missing download-url-prefix}"
appcast="${3:-$repo_root/packages/app/public/desktop/appcast.xml}"
case "$prefix" in */) ;; *) prefix="$prefix/" ;; esac

tool_dir="${SPARKLE_BIN:-}"
if [ -z "$tool_dir" ]; then
  tool_dir="$(find "$here/.build/artifacts" -type f -name generate_appcast -path '*bin*' 2>/dev/null | head -1 | xargs -I{} dirname {})"
fi
gen="$tool_dir/generate_appcast"
if [ ! -x "$gen" ]; then
  echo "error: generate_appcast not found (run 'swift build' in $here, or set SPARKLE_BIN)" >&2
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cp "$dmg" "$work/"
cp "$appcast" "$work/appcast.xml"

args=(--download-url-prefix "$prefix" --link "https://vcad.io/desktop/" -o "$work/appcast.xml")
if [ -n "${SPARKLE_ED_PRIVATE_KEY:-}" ]; then
  printf '%s' "$SPARKLE_ED_PRIVATE_KEY" | "$gen" --ed-key-file - "${args[@]}" "$work"
else
  "$gen" "${args[@]}" "$work"
fi

# generate_appcast silently omits the EdDSA signature when the key does not
# match the app's SUPublicEDKey (or the app has none). Sparkle clients would
# then reject the update, so refuse to publish an unsigned item.
before="$(grep -c 'sparkle:edSignature=' "$appcast" || true)"
after="$(grep -c 'sparkle:edSignature=' "$work/appcast.xml" || true)"
if [ "$after" -le "$before" ]; then
  echo "error: new appcast item has no sparkle:edSignature — does the private key match the app's SUPublicEDKey?" >&2
  exit 1
fi

cp "$work/appcast.xml" "$appcast"
echo "updated: $appcast"
