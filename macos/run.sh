#!/usr/bin/env bash
# Launch KB.app for development with secrets from the env.
#
# GUI apps opened via Finder / `open` inherit launchd's environment, NOT your
# shell's — so API keys exported in .env.local.sh never reach the app, and it
# falls back to the Keychain (which re-prompts on every rebuild because the
# code signature changes). Running the app *binary* directly from this script
# lets it inherit the sourced env. ServerController.resolveKey() prefers
# OPENAI_API_KEY / ANTHROPIC_API_KEY over the Keychain, so no prompt ever fires.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"

# Load local secrets (OPENAI_API_KEY, ANTHROPIC_API_KEY, KB_ROOT) if present.
if [ -f "$ROOT/.env.local.sh" ]; then
    # shellcheck disable=SC1091
    source "$ROOT/.env.local.sh"
fi

BIN="$HERE/build/KB.app/Contents/MacOS/KB"
if [ ! -x "$BIN" ]; then
    echo "✗ $BIN not found — run ./build.sh first" >&2
    exit 1
fi

echo "› launching KB.app with env keys (OPENAI/ANTHROPIC), no Keychain access"
exec "$BIN"
