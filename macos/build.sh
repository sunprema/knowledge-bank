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

# Build the kb-ocr sidecar (Vision + PDFKit OCR for image-only PDFs). It's a
# standalone CLI (its own `main`), so it compiles separately from the app's
# -parse-as-library bundle. The engine finds it as a sibling, so we drop a copy
# next to the engine in both the bundle's Resources/ and the dev target/ dir.
echo "› compiling kb-ocr sidecar…"
swiftc "$HERE/Tools/kb-ocr.swift" \
    -o "$APP/Contents/Resources/kb-ocr" \
    -sdk "$SDK" \
    -target "$TARGET" \
    -framework PDFKit \
    -framework Vision \
    -O
chmod +x "$APP/Contents/Resources/kb-ocr"
# Dev sibling: when the engine runs from target/{release,debug}/kb (not bundled),
# it looks for kb-ocr next to itself there too.
for d in "$HERE/../target/release" "$HERE/../target/debug"; do
    [ -d "$d" ] && cp "$APP/Contents/Resources/kb-ocr" "$d/kb-ocr" && chmod +x "$d/kb-ocr"
done

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

# Codesign. Ad-hoc ("-") by default: the app reads its API keys from the
# environment (see run.sh / "Launch KB.app") and never touches the Keychain, so
# there's no "Always Allow" grant to preserve and a per-build signature is fine.
# Set KB_SIGN_IDENTITY to a Keychain code-signing cert only if you specifically
# need a stable signature (e.g. distribution). Sign inside-out: engine, sidecar, app.
SIGN_ID="${KB_SIGN_IDENTITY:--}"
# Sign the whole bundle in one --deep pass. On external volumes (/Volumes/x) the
# just-copied 21MB engine may not be fully flushed when codesign enumerates the
# bundle, producing an inconsistent seal — so flush and retry a couple of times.
sync
signed=""
for attempt in 1 2 3; do
    if codesign --force --deep --sign "$SIGN_ID" "$APP" 2>/dev/null \
       && codesign --verify "$APP" 2>/dev/null; then
        signed=1; break
    fi
    sync; sleep 0.5
done
if [ -n "$signed" ]; then
    [ "$SIGN_ID" = "-" ] && echo "› ad-hoc signed" || echo "› signed with identity: $SIGN_ID"
else
    echo "  (codesign failed — unsigned bundle still launches locally)"
fi

echo "✓ built $APP"
