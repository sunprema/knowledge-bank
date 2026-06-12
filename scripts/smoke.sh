#!/usr/bin/env bash
# End-to-end smoke test (PRD §15): real arXiv + embedding API calls.
# Gated so CI/regular test runs never hit the network:
#   ENABLE_E2E=1 OPENAI_API_KEY=sk-... ./scripts/smoke.sh
set -euo pipefail

if [[ "${ENABLE_E2E:-}" != "1" ]]; then
  echo "skipped (set ENABLE_E2E=1 to run; needs network, pandoc, OPENAI_API_KEY)"
  exit 0
fi
: "${OPENAI_API_KEY:?OPENAI_API_KEY must be set}"

cd "$(dirname "$0")/.."
cargo build -q
KB=./target/debug/kb
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
echo "KB root: $ROOT"

echo "== kb add 2504.19874 (TurboQuant)"
"$KB" --root "$ROOT" add 2504.19874

echo "== folder structure"
test -f "$ROOT/2504.19874/metadata.json"
test -f "$ROOT/2504.19874/paper.pdf"
test -f "$ROOT/2504.19874/sections.md"
test -f "$ROOT/2504.19874/notes.md"
test -f "$ROOT/.arxiv-kb/index.tv"
test -f "$ROOT/.arxiv-kb/meta.db"

echo "== kb verify"
"$KB" --root "$ROOT" verify --deep

echo "== kb search quantization"
OUT="$("$KB" --root "$ROOT" --format json search "quantization")"
echo "$OUT" | grep -qi "TurboQuant" || { echo "FAIL: TurboQuant not in results"; exit 1; }

echo "== kb reindex (rebuild from canonical files; cache ⇒ no API spend)"
"$KB" --root "$ROOT" reindex --yes
"$KB" --root "$ROOT" verify --deep

echo "== re-search after reindex"
OUT="$("$KB" --root "$ROOT" --format json search "quantization")"
echo "$OUT" | grep -qi "TurboQuant" || { echo "FAIL: TurboQuant lost after reindex"; exit 1; }

echo "== kb remove"
"$KB" --root "$ROOT" remove 2504.19874 --yes
test ! -d "$ROOT/2504.19874"

echo "SMOKE OK"
