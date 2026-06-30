# Developer ID Notarization — Distributing KB.app Outside the App Store

## Context

KB ships as a hand-assembled `.app` bundle (`macos/build.sh` compiles the Swift
sources with `swiftc` and lays out the bundle by hand — no Xcode project). For
distribution we are going with **Developer ID + notarization**, *not* the Mac
App Store. The App Store would force the App Sandbox, which fights three core
design choices: the bundled `kb` engine running as a local server, file
**Watches**, and the BYO-API-key model. Developer ID keeps that architecture
intact and stays close to our current `swiftc` build.

Goal: a user downloads `KB.app` (or a `.dmg`), and Gatekeeper opens it cleanly
("Apple checked it for malicious software") — no right-click-to-open, no
"unidentified developer" wall.

> Status: **planning only.** Not started. Pick up when we're ready to cut a
> public build.

## What's in the bundle (everything that must be signed)

Notarization requires **every** Mach-O in the bundle to be signed with the same
Developer ID identity, with the hardened runtime enabled. KB's bundle (see
`macos/build.sh`) contains three executables:

- `Contents/MacOS/KB` — the SwiftUI app (main executable)
- `Contents/Resources/kb` — the Rust engine (`kb-serve`), bundled from
  `target/release/kb`
- `Contents/Resources/kb-ocr` — the Swift Vision/PDFKit OCR sidecar

Plus the system-provided **WKWebView `WebContent` helper** that macOS launches
at runtime (Reader/Notes render via WKWebView). This is why a *valid* signature
matters today — `build.sh` already notes that a broken seal makes WebContent
fail with the `-50` "application can't be opened" popup. Hardened runtime adds
entitlement requirements on top.

## Prerequisites (one-time)

1. **Apple Developer Program** membership ($99/yr) — same cost as the App Store
   path, already assumed from the pricing discussion.
2. **Developer ID Application** certificate, created in the Developer portal and
   installed in the login keychain. (Note: this is *Developer ID Application*,
   distinct from the *Apple Distribution* / *Mac App Distribution* certs the App
   Store uses.) Confirm with:
   `security find-identity -v -p codesigning` → look for
   `Developer ID Application: <Name> (<TEAMID>)`.
3. **notarytool credentials.** Preferred: store an app-specific password (from
   appleid.apple.com) once as a keychain profile:
   `xcrun notarytool store-credentials "KB-NOTARY" --apple-id <id> --team-id <TEAMID> --password <app-specific-pw>`
   Then later runs just pass `--keychain-profile "KB-NOTARY"`.

## The signing order (critical)

Sign **inside-out**: inner executables first, the `.app` last. `--deep` is
deprecated/unreliable for this — sign each Mach-O explicitly.

1. Sign `Contents/Resources/kb` (engine) — hardened runtime + entitlements.
2. Sign `Contents/Resources/kb-ocr` (sidecar) — hardened runtime + entitlements.
3. Sign `Contents/MacOS/KB` (app binary) — hardened runtime + entitlements.
4. Sign the outer `KB.app`.
5. Verify: `codesign --verify --deep --strict --verbose=2 KB.app` and
   `codesign -dvvv KB.app` (check `flags=runtime`, the Developer ID authority,
   and a Team ID).

All four use:
`codesign --force --timestamp --options runtime --sign "$DEV_ID" [--entitlements <plist>] <path>`

The `--timestamp` (secure timestamp) and `--options runtime` (hardened runtime)
flags are both **required** for notarization to pass.

## Entitlements to work out

The hardened runtime blocks some behaviors unless explicitly entitled. We need
to determine the minimum set that lets KB still function. Likely candidates:

- `com.apple.security.cs.allow-jit` and/or
  `com.apple.security.cs.allow-unsigned-executable-memory` — **TBD**: check
  whether WKWebView's JS engine needs these under hardened runtime. (Usually the
  system WebContent process is fine, but verify Reader/Notes still render.)
- `com.apple.security.cs.disable-library-validation` — **TBD**: only if the app
  loads the bundled `kb`/`kb-ocr` in a way library validation rejects. Prefer to
  leave this OFF and confirm the inner binaries are signed with the *same* Team
  ID (they will be), which satisfies validation without disabling it.
- Network client access is allowed by default under Developer ID (no sandbox),
  so no network entitlement needed — unlike the App Store path.

Action item: start with an **empty/minimal** entitlements plist, build, run,
and add entitlements only for what actually breaks. Don't over-grant.

## Notarize + staple

1. Zip for submission (notarytool takes a `.zip`, `.dmg`, or `.pkg`):
   `ditto -c -k --keepParent KB.app KB.zip`
2. Submit and wait:
   `xcrun notarytool submit KB.zip --keychain-profile "KB-NOTARY" --wait`
3. On `status: Accepted`, **staple the original `.app`** (not the zip):
   `xcrun stapler staple KB.app`
4. Verify the end-user experience:
   `spctl -a -vvv KB.app` → expect `accepted` / `source=Notarized Developer ID`.
5. If rejected, pull the log:
   `xcrun notarytool log <submission-id> --keychain-profile "KB-NOTARY"`
   (Usually flags an unsigned/un-hardened inner binary or a missing timestamp.)

## Distribution wrapper (decide later)

- **.dmg** is the conventional Developer ID delivery (drag-to-Applications).
  Notarize the `.dmg` itself *after* it contains the already-signed app, then
  staple the `.dmg`. Or staple the app and distribute the `.dmg` un-stapled —
  decide which.
- Consider a `create-dmg` step in the build.
- Decide whether to staple the `.app`, the `.dmg`, or both.

## Proposed script: `macos/notarize.sh`

A new sibling to `build.sh` that runs **after** a successful `build.sh`:

1. Require `KB_DEV_ID` (the `Developer ID Application: …` string) and
   `KB_NOTARY_PROFILE` (keychain profile name) in the env; fail fast if missing.
2. Sign inner binaries → app, inside-out, with `--options runtime --timestamp`
   and the entitlements plist.
3. `codesign --verify --deep --strict` gate.
4. `ditto` zip → `notarytool submit --wait` → `stapler staple` → `spctl`
   verify.
5. (Optional) build + notarize + staple a `.dmg`.

Keep `build.sh`'s existing `KB_SIGN_IDENTITY` ad-hoc path for local dev runs;
`notarize.sh` is only for cutting a public build. They shouldn't interfere — dev
builds stay ad-hoc, release builds go through `notarize.sh`.

## Open questions to resolve when we pick this up

- [ ] Which entitlements does WKWebView actually need under hardened runtime?
      (Empirical — build minimal, add only on failure.)
- [ ] `.dmg` vs raw `.zip` for distribution; what to staple.
- [ ] Where do public builds get hosted? (GitHub Releases is the obvious fit.)
- [ ] Version/build bump flow — `Info.plist` `CFBundleShortVersionString`
      (currently `0.1`) and `CFBundleVersion` (currently `1`) need a real
      release cadence.
- [ ] Auto-update story (Sparkle?) — out of scope for first notarized build,
      but the appcast/EdDSA-key decision is easier to make now than later.

## References

- `macos/build.sh` — current hand-rolled bundle + ad-hoc signing
- `macos/Info.plist` — bundle id `com.sunprema.kb`, `LSUIElement`, macOS 14 min
- Apple: "Notarizing macOS software before distribution", `notarytool`,
  `stapler`, Hardened Runtime entitlements
