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

### 1. Developer ID Application certificate

1. Xcode → Settings → Accounts → (team) → Manage Certificates → **+** →
   **Developer ID Application**. (Or developer.apple.com → Certificates → +.)
2. Check it's in the login keychain:
   ```bash
   security find-identity -v -p codesigning | grep "Developer ID Application"
   ```
   The quoted string, e.g. `Developer ID Application: Cam Pedersen (TEAMID)`,
   is `VCAD_SIGN_IDENTITY`.
3. For CI, export it: Keychain Access → My Certificates → right-click the
   cert → Export → `.p12` with a strong password. Then
   `base64 -i DeveloperID.p12 | pbcopy` → secret `APPLE_DEVELOPER_ID_P12`, the
   password → `APPLE_P12_PASSWORD`. Delete the `.p12` file afterwards.

### 2. notarytool credentials

Create an app-specific password at appleid.apple.com → Sign-In and Security →
App-Specific Passwords, then:

```bash
xcrun notarytool store-credentials vcad-notary \
  --apple-id "you@example.com" --team-id "TEAMID" --password "abcd-efgh-ijkl-mnop"
```

This stores a keychain profile named `vcad-notary` (= `VCAD_NOTARY_PROFILE`).
For CI, the same three values go in secrets `NOTARY_APPLE_ID`,
`NOTARY_TEAM_ID`, `NOTARY_PASSWORD`.

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
| `APPLE_DEVELOPER_ID_P12` | secret | base64 of the exported Developer ID `.p12` |
| `APPLE_P12_PASSWORD` | secret | that `.p12`'s password |
| `NOTARY_APPLE_ID` | secret | Apple ID email |
| `NOTARY_TEAM_ID` | secret | 10-char team ID |
| `NOTARY_PASSWORD` | secret | app-specific password |
| `SPARKLE_ED_PRIVATE_KEY` | secret | `generate_keys -x` output |
| `SPARKLE_PUBLIC_ED_KEY` | variable | `generate_keys -p` output |
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
VCAD_SIGN_IDENTITY="Developer ID Application: Cam Pedersen (TEAMID)" \
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
