#!/usr/bin/env bash
# Build KB.app from Swift sources using the Command Line Tools `swiftc`
# (no full Xcode / xcodebuild required). Assembles the .app bundle by hand.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_NAME="KB"
BUILD_DIR="$HERE/build"
APP="$BUILD_DIR/$APP_NAME.app"
SDK="$(xcrun --show-sdk-path)"
ARCH="$(uname -m)"           # arm64 on Apple Silicon
TARGET="${ARCH}-apple-macos14.0"

echo "› SDK:    $SDK"
echo "› target: $TARGET"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

echo "› compiling…"
# Compile every .swift under Sources/ (recursively).
SOURCES=()
while IFS= read -r f; do SOURCES+=("$f"); done < <(find "$HERE/Sources" -name '*.swift')
swiftc "${SOURCES[@]}" \
    -o "$APP/Contents/MacOS/$APP_NAME" \
    -sdk "$SDK" \
    -target "$TARGET" \
    -framework SwiftUI \
    -framework AppKit \
    -framework PDFKit \
    -framework AVFoundation \
    -parse-as-library \
    -O

cp "$HERE/Info.plist" "$APP/Contents/Info.plist"

# Bundle the engine so the app is self-contained (LOCAL_UI_PRD §2). Prefer the
# release build; fall back to debug.
ENGINE=""
for c in "$HERE/../target/release/kb" "$HERE/../target/debug/kb"; do
    [ -x "$c" ] && ENGINE="$c" && break
done
if [ -n "$ENGINE" ]; then
    cp "$ENGINE" "$APP/Contents/Resources/kb"
    chmod +x "$APP/Contents/Resources/kb"
    echo "› bundled engine: $ENGINE"
else
    echo "  ⚠ no kb binary found under target/ — app will fall back to the dev path at runtime"
fi

# Codesign. A *stable* identity (set KB_SIGN_IDENTITY to a code-signing cert in
# your Keychain) keeps the app's signature constant across rebuilds, so the
# Keychain "Always Allow" sticks. Falling back to ad-hoc ("-") re-signs uniquely
# each build, which makes macOS re-prompt for the OpenAI key every launch.
# Default to ad-hoc. A stable identity (KB_SIGN_IDENTITY) fixes the Keychain
# re-prompt, but on macOS it makes the app subject to TCC for external volumes
# (e.g. /Volumes/x) — which then needs Full Disk Access granted to KB.app.
SIGN_ID="${KB_SIGN_IDENTITY:--}"
# Sign the bundled engine first, then the app (inside-out).
[ -f "$APP/Contents/Resources/kb" ] && codesign --force --sign "$SIGN_ID" "$APP/Contents/Resources/kb" >/dev/null 2>&1
if codesign --force --sign "$SIGN_ID" "$APP" >/dev/null 2>&1; then
    if [ "$SIGN_ID" = "-" ]; then
        echo "  ⚠ ad-hoc signed — Keychain 'Always Allow' won't persist across rebuilds."
        echo "    Create a self-signed Code Signing cert and run: export KB_SIGN_IDENTITY=\"<cert name>\""
    else
        echo "› signed with stable identity: $SIGN_ID"
    fi
else
    echo "  (codesign failed — unsigned bundle still launches locally)"
fi

echo "✓ built $APP"
