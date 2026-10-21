# Releasing vcad for Mac

The native app (`apple/VcadApp`) ships as a Developer ID–signed, notarized DMG
with Sparkle 2 auto-update.

| Piece | Where |
|---|---|
| Updater | `Sources/VcadApp/Updater.swift` (Sparkle controller + "Check for Updates…"), `Sources/VcadUpdate/UpdaterConfig.swift` (rules) |
| Bundle + sign | `apple/VcadApp/bundle.sh` |
| DMG + notarize | `apple/VcadApp/release.sh` |
| Appcast item | `apple/VcadApp/scripts/make-appcast.sh` |
| Feed | `packages/app/public/desktop/appcast.xml` → `https://vcad.io/desktop/appcast.xml` |
| Download page | `packages/app/public/desktop/index.html` → `https://vcad.io/desktop/` |
| DMGs | GitHub Releases tagged `desktop-v<version>` (linked from the appcast) |
| CI | `.github/workflows/desktop-release.yml` |

Without any of the env vars below, `bundle.sh` behaves as before: an ad-hoc
signed `dist/vcad.app`, no update key, updater idle.

## One-time setup (Cam)

> **Signing waits on the Municipal Robotics Apple organization enrollment.**
> The Developer ID certificate and App Store Connect API key below belong to
> that organization's team; until enrollment completes, CI releases stop at
> the "Import Developer ID certificate" step and local builds stay ad-hoc.

Signing and notarization use the **shared ecto org setup**, the same
certificate and API key as ecto/tang (see tang's
`docs/release/macos-signing.md`). If those org secrets already exist on
ecto, steps 1 and 2 are done; vcad only needs the Sparkle and
tang-compress items.

### 1. Developer ID Application certificate (shared)

1. Xcode → Settings → Accounts → (Municipal Robotics team) → Manage
   Certificates → **+** → **Developer ID Application**.
2. `security find-identity -v -p codesigning | grep "Developer ID Application"`
   prints the full name, e.g.
   `Developer ID Application: Municipal Robotics (TEAMID1234)`. That's
   `MACOS_SIGN_IDENTITY` (and `VCAD_SIGN_IDENTITY` locally).
3. Keychain Access → My Certificates → right-click the cert → Export →
   `.p12` with a strong password. `base64 -i cert.p12 | pbcopy` →
   `MACOS_CERT_P12`; the password → `MACOS_CERT_PASSWORD`. Delete the file
   (or keep it in a password manager).

### 2. Notarization: App Store Connect API key (shared)

appstoreconnect.apple.com → Users and Access → Integrations → App Store
Connect API → Team Keys → **+** → name "notary", access "Developer".
Download `AuthKey_<id>.p8` (one chance). Note the Key ID and the Issuer ID.
`base64 -i AuthKey_<id>.p8 | pbcopy` → `ASC_KEY_P8`; the IDs →
`ASC_KEY_ID`, `ASC_ISSUER_ID`.

Add all six as **organization secrets on ecto** (Settings → Secrets and
variables → Actions), shared with ecto/tang and ecto/vcad.

Locally, `release.sh` notarizes either way:

- API key: `VCAD_ASC_KEY_PATH=~/keys/AuthKey_X.p8 VCAD_ASC_KEY_ID=X VCAD_ASC_ISSUER=…`
- keychain profile: store one once with
  `xcrun notarytool store-credentials vcad-notary --key ~/keys/AuthKey_X.p8 --key-id X --issuer …`
  and set `VCAD_NOTARY_PROFILE=vcad-notary`.

### 3. Sparkle EdDSA keys

`generate_keys` ships in the Sparkle SwiftPM artifact once the package has
been built (`swift build --package-path apple/VcadApp`):

```bash
apple/VcadApp/.build/artifacts/sparkle/Sparkle/bin/generate_keys
```

It creates the private key **in your login Keychain** and prints the public
key. The private key never leaves the Keychain and is never committed.

- Public key: `generate_keys -p` prints it again. Set it as the repository
  **variable** `SPARKLE_PUBLIC_ED_KEY` (it's public; it's baked into each
  app's Info.plist as `SUPublicEDKey`).
- Private key, for CI only: `generate_keys -x sparkle_private.key`, paste the
  file's contents into secret `SPARKLE_ED_PRIVATE_KEY`, then `rm` the file.
  Back up the Keychain item — losing it means shipped apps can never verify
  another update.

### 4. tang-compress source for CI

The app links tang-compress's decoder (`build-tc-ffi.sh`). Set repository
variable `TANG_COMPRESS_REPO` (`owner/name`); if that repo is private, add a
read-only token as secret `TANG_COMPRESS_TOKEN`.

### Secrets and variables summary

| Name | Kind | Value |
|---|---|---|
| `MACOS_CERT_P12` | ecto org secret (shared) | base64 of the Developer ID `.p12` |
| `MACOS_CERT_PASSWORD` | ecto org secret (shared) | that `.p12`'s password |
| `MACOS_SIGN_IDENTITY` | ecto org secret (shared) | full identity name |
| `ASC_KEY_ID` | ecto org secret (shared) | App Store Connect API key ID |
| `ASC_ISSUER_ID` | ecto org secret (shared) | issuer ID |
| `ASC_KEY_P8` | ecto org secret (shared) | base64 of `AuthKey_<id>.p8` |
| `SPARKLE_ED_PRIVATE_KEY` | vcad repo secret | `generate_keys -x` output |
| `SPARKLE_PUBLIC_ED_KEY` | vcad repo variable | `generate_keys -p` output |
| `TANG_COMPRESS_REPO` | variable | `owner/name` of tang-compress |
| `TANG_COMPRESS_TOKEN` | secret (optional) | read token if that repo is private |

## Cutting a release

**CI (preferred):** push a tag `desktop-v0.10.0`, or run the
`desktop-release` workflow manually with `version: 0.10.0`. It builds,
signs, notarizes, staples, uploads the DMG to a GitHub Release, and opens a
PR updating the appcast. **Merging that PR is what ships the update** (Vercel
redeploys vcad.io). PRs opened with the workflow token don't trigger other
workflows; close/reopen it if you want CI on it.

**Locally:**

```bash
cd apple/VcadApp
./build-ffi.sh && ./build-tc-ffi.sh
VCAD_SIGN_IDENTITY="Developer ID Application: Municipal Robotics (TEAMID1234)" \
VCAD_NOTARY_PROFILE=vcad-notary \
SPARKLE_PUBLIC_ED_KEY="$(.build/artifacts/sparkle/Sparkle/bin/generate_keys -p)" \
VCAD_VERSION=0.10.0 VCAD_BUILD=100 \
./release.sh
gh release create desktop-v0.10.0 dist/vcad-0.10.0.dmg --title "vcad for Mac 0.10.0"
scripts/make-appcast.sh dist/vcad-0.10.0.dmg \
  https://github.com/ecto/vcad/releases/download/desktop-v0.10.0/
# commit packages/app/public/desktop/appcast.xml via a PR
```

`VCAD_BUILD` (CFBundleVersion) must increase with every release — Sparkle
compares it. CI uses the workflow run number; if you mix local releases in,
use a number above the latest run.

## Guarantees

- `bundle.sh` with `VCAD_SIGN_IDENTITY` but no `SPARKLE_PUBLIC_ED_KEY` exits 1.
- A bundle marked `VCADReleaseBuild` with an empty key refuses to start the
  updater (`UpdaterConfig`, unit-tested).
- `make-appcast.sh` exits 1 if the new item lacks an EdDSA signature (key
  mismatch with the app's `SUPublicEDKey`).
- `release.sh` refuses to notarize an ad-hoc build.
