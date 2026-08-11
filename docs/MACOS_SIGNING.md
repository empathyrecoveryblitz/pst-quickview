# macOS Signing And Notarization Plan

PST QuickView `0.2.0-beta.3` is an unsigned public-beta candidate. Do not claim it is signed or notarized
until every verification step below succeeds.

Apple references:

- [Developer ID certificates](https://developer.apple.com/help/account/certificates/create-developer-id-certificates/)
- [Hardened Runtime](https://developer.apple.com/documentation/security/hardened-runtime)
- [Notarizing macOS software before distribution](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)

## Requirements

- Active Apple Developer Program membership.
- A Developer ID Application certificate and private key in the signing keychain.
- Current Xcode command-line tools.
- App-specific password or App Store Connect API credentials for `notarytool`.
- Final review of entitlements and Tauri capabilities.

The Tauri configuration enables the hardened runtime. PST QuickView currently has no custom
entitlements file; add only entitlements that a verified feature requires.

## Build

```sh
npm ci
npm run tauri build -- --target universal-apple-darwin
bash scripts/verify-macos-release.sh
```

Example paths:

```sh
APP="src-tauri/target/universal-apple-darwin/release/bundle/macos/PST QuickView.app"
DMG="src-tauri/target/universal-apple-darwin/release/bundle/dmg/PST QuickView_0.2.0-beta.3_universal.dmg"
IDENTITY="Developer ID Application: Example Company (TEAMID)"
```

## Signing Order

Sign nested executable code before signing the containing app:

```sh
codesign --force --options runtime --timestamp \
  --sign "$IDENTITY" "$APP/Contents/MacOS/readpst"

codesign --force --options runtime --timestamp \
  --sign "$IDENTITY" "$APP"
```

Do not use `codesign --deep` as a substitute for identifying and signing nested code correctly.
Use it only during verification where appropriate.

Verify:

```sh
codesign --verify --deep --strict --verbose=4 "$APP"
codesign -dv --verbose=4 "$APP"
codesign -d --entitlements :- "$APP"
```

After the app is signed, recreate the DMG or sign the final DMG:

```sh
codesign --force --timestamp --sign "$IDENTITY" "$DMG"
codesign --verify --verbose=4 "$DMG"
```

## Notarization

Store credentials once:

```sh
xcrun notarytool store-credentials "pst-quickview-notary" \
  --apple-id "APPLE_ID" \
  --team-id "TEAM_ID" \
  --password "APP_SPECIFIC_PASSWORD"
```

Submit the final signed DMG:

```sh
xcrun notarytool submit "$DMG" \
  --keychain-profile "pst-quickview-notary" \
  --wait
```

Do not use deprecated `altool`.

## Stapling And Gatekeeper Verification

```sh
xcrun stapler staple "$DMG"
xcrun stapler validate "$DMG"

hdiutil attach "$DMG"
spctl --assess --type execute --verbose=4 \
  "/Volumes/PST QuickView/PST QuickView.app"
codesign --verify --deep --strict --verbose=4 \
  "/Volumes/PST QuickView/PST QuickView.app"
```

Also install and launch on clean Intel and Apple Silicon Macs with no Homebrew.

## Unsigned Internal Beta

For the current unsigned beta:

1. Copy `PST QuickView.app` to `/Applications`.
2. Right-click the app and choose **Open**.
3. Do not disable Gatekeeper globally.
4. If quarantine removal is approved for the internal test, scope it only to this app:

```sh
xattr -dr com.apple.quarantine "/Applications/PST QuickView.app"
```
