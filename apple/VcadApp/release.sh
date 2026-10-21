#!/usr/bin/env bash
# Build a release DMG of vcad.app: bundle (release config) -> DMG -> optional
# notarize + staple. See docs/mac-release.md.
#
#   VCAD_SIGN_IDENTITY     Developer ID Application identity (bundle.sh signs
#                          the app; this script signs the DMG). Unset = ad-hoc
#                          app in an unsigned DMG, for local testing only.
#   VCAD_NOTARY_PROFILE    notarytool keychain profile (from
#                          `xcrun notarytool store-credentials`). Set = notarize
#                          the DMG and staple the ticket. Requires a signing identity.
#   NOTARY_KEYCHAIN        optional keychain path holding that profile.
#   VCAD_ASC_KEY_PATH, VCAD_ASC_KEY_ID, VCAD_ASC_ISSUER
#                          App Store Connect API key (.p8 path, key ID, issuer
#                          ID). Set all three = notarize with the API key (CI).
#                          Takes precedence over VCAD_NOTARY_PROFILE.
#   SPARKLE_PUBLIC_ED_KEY, VCAD_VERSION, VCAD_BUILD: passed through to bundle.sh.
#
# Output: dist/vcad-<version>.dmg
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
version="${VCAD_VERSION:-0.1}"
sign_identity="${VCAD_SIGN_IDENTITY:-}"
notary_profile="${VCAD_NOTARY_PROFILE:-}"
asc_key="${VCAD_ASC_KEY_PATH:-}"
asc_id="${VCAD_ASC_KEY_ID:-}"
asc_issuer="${VCAD_ASC_ISSUER:-}"

notary_auth=()
if [ -n "$asc_key$asc_id$asc_issuer" ]; then
  if [ -z "$asc_key" ] || [ -z "$asc_id" ] || [ -z "$asc_issuer" ]; then
    echo "error: set all of VCAD_ASC_KEY_PATH, VCAD_ASC_KEY_ID, VCAD_ASC_ISSUER (or none)." >&2
    exit 1
  fi
  notary_auth=(--key "$asc_key" --key-id "$asc_id" --issuer "$asc_issuer")
elif [ -n "$notary_profile" ]; then
  notary_auth=(--keychain-profile "$notary_profile")
  if [ -n "${NOTARY_KEYCHAIN:-}" ]; then notary_auth+=(--keychain "$NOTARY_KEYCHAIN"); fi
fi

if [ ${#notary_auth[@]} -gt 0 ] && [ -z "$sign_identity" ]; then
  echo "error: notarization needs VCAD_SIGN_IDENTITY (Apple rejects ad-hoc signatures)." >&2
  exit 1
fi

CONFIG="${CONFIG:-release}" "$here/bundle.sh"

app="$here/dist/vcad.app"
dmg="$here/dist/vcad-${version}.dmg"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
ditto "$app" "$stage/vcad.app"
ln -s /Applications "$stage/Applications"
rm -f "$dmg"
hdiutil create -volname "vcad" -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$dmg" >/dev/null
echo "dmg: $dmg"

if [ -n "$sign_identity" ]; then
  codesign --force --timestamp --sign "$sign_identity" "$dmg"
fi

if [ ${#notary_auth[@]} -gt 0 ]; then
  echo "notarizing…"
  xcrun notarytool submit "$dmg" "${notary_auth[@]}" --wait
  xcrun stapler staple "$dmg"
  xcrun stapler validate "$dmg"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
fi

echo "release artifact: $dmg"
