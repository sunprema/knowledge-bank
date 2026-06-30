#!/usr/bin/env bash
# Launch KB.app for development with secrets from the env.
#
# Two requirements pull against each other here:
#  1. The app needs our shell's API keys. GUI apps launched via Finder/`open`
#     normally inherit launchd's environment, NOT your shell's — so keys exported
#     in .env.local.sh wouldn't reach the app, and it would fall back to the
#     Keychain (which re-prompts on every rebuild as the ad-hoc cdhash changes).
#  2. The app must be launched through LaunchServices, NOT by exec'ing its binary
#     directly. WKWebView's helper processes (WebContent/GPU/Networking) can only
#     be spawned by a LaunchServices-launched app; a directly-exec'd app can't
#     create them, so Reader mode and the Notes preview fail with a
#     "-50, the application can't be opened" popup.
#
# `open --env` satisfies both: it launches via LaunchServices (so WebKit works)
# and forwards the named env vars (so ServerController.resolveKey() finds the
# keys and never touches the Keychain).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"

# Load local secrets (OPENAI_API_KEY, ANTHROPIC_API_KEY, KB_ROOT) if present.
if [ -f "$ROOT/.env.local.sh" ]; then
    # shellcheck disable=SC1091
    source "$ROOT/.env.local.sh"
fi

APP="$HERE/build/KB.app"
if [ ! -d "$APP" ]; then
    echo "✗ $APP not found — run ./build.sh first" >&2
    exit 1
fi

# Forward only the vars the app/engine actually consume, and only if set.
# (Deliberately NOT KB_SIGN_IDENTITY — that's a build-time concern.)
ENV_ARGS=()
for v in OPENAI_API_KEY ANTHROPIC_API_KEY KB_ROOT KB_API_KEY; do
    if [ -n "${!v:-}" ]; then ENV_ARGS+=(--env "$v=${!v}"); fi
done

echo "› launching KB.app via LaunchServices (open --env): WebKit helpers work, keys forwarded, no Keychain"
# -n: always start a fresh instance (dev rebuilds). The app runs under launchd,
# so this script returns immediately; use Console.app / `log stream` for logs.
exec open -n "$APP" "${ENV_ARGS[@]}"
