#!/usr/bin/env bash
# Build "Launch KB.app" — a double-clickable launcher that starts KB via run.sh,
# which loads your API keys from .env.local.sh into the environment. Because the
# keys arrive via env, the app never reads the Keychain, so there's no "allow
# access" prompt — and that holds across every rebuild (ad-hoc signing and all).
#
# Keep "Launch KB.app" in your Dock/Applications. Re-run this after moving the
# repo (it bakes in the absolute path to run.sh).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"

SCRIPT="$(mktemp).applescript"
cat > "$SCRIPT" <<OSA
if application "KB" is running then
	tell application "KB" to activate
else
	do shell script "nohup '$HERE/run.sh' >/tmp/kb-app.log 2>&1 &"
end if
OSA

rm -rf "$HERE/Launch KB.app"
osacompile -o "$HERE/Launch KB.app" "$SCRIPT"
rm -f "$SCRIPT"
echo "✓ built $HERE/Launch KB.app — double-click it (or add it to the Dock) to launch KB prompt-free."
