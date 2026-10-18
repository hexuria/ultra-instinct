#!/usr/bin/env bash
# Live Chrome smoke for world-context gating (modal, twin focus, cookie hit-test).
# No JEV / Luna. Starts a throwaway Chrome, serves the static pages, runs the
# ignored ultra-instinct-mcp live tests, then tears down.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

PORT="${ULTRA_INSTINCT_SMOKE_PORT:-8766}"
CDP_PORT="${ULTRA_INSTINCT_SMOKE_CDP_PORT:-9334}"
PROFILE="${ULTRA_INSTINCT_SMOKE_PROFILE:-/tmp/ultra-instinct-world-context-chrome}"
CHROME="${ULTRA_INSTINCT_CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
SITE="http://127.0.0.1:${PORT}"
CDP="http://127.0.0.1:${CDP_PORT}"

if [[ ! -x "$CHROME" ]]; then
  echo "Chrome not found at $CHROME" >&2
  exit 1
fi

mkdir -p "$PROFILE"
python3 -m http.server "$PORT" --directory "$ROOT/examples/world-context/site" >/tmp/ultra-instinct-world-context-http.log 2>&1 &
HTTP_PID=$!

cleanup() {
  kill "$HTTP_PID" 2>/dev/null || true
  if [[ -n "${CHROME_PID:-}" ]]; then
    kill "$CHROME_PID" 2>/dev/null || true
    wait "$CHROME_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT

"$CHROME" \
  --remote-debugging-port="$CDP_PORT" \
  --remote-debugging-address=127.0.0.1 \
  --user-data-dir="$PROFILE" \
  --no-first-run \
  --no-default-browser-check \
  --disable-extensions \
  --window-size=1280,800 \
  --headless=new \
  about:blank \
  >/tmp/ultra-instinct-world-context-chrome.log 2>&1 &
CHROME_PID=$!

# Wait for CDP
for _ in $(seq 1 50); do
  if curl -fsS "$CDP/json/version" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done
if ! curl -fsS "$CDP/json/version" >/dev/null 2>&1; then
  echo "Chrome CDP did not come up on $CDP" >&2
  tail -n 40 /tmp/ultra-instinct-world-context-chrome.log >&2 || true
  exit 1
fi

export ULTRA_INSTINCT_LIVE_CDP="$CDP"
export ULTRA_INSTINCT_LIVE_SITE="$SITE"
MATCHER="${ULTRA_INSTINCT_MATCHER:-weighted}"
export ULTRA_INSTINCT_MATCHER="$MATCHER"

FEATURE_ARGS=()
if [[ "$MATCHER" == "hgra" ]]; then
  FEATURE_ARGS+=(--features hgra)
elif [[ "$MATCHER" != "weighted" ]]; then
  echo "unknown ULTRA_INSTINCT_MATCHER=$MATCHER (want weighted|hgra)" >&2
  exit 1
fi

echo "CDP=$CDP SITE=$SITE MATCHER=$MATCHER"
set +e
cargo test -p ultra-instinct-mcp "${FEATURE_ARGS[@]}" --test world_context_live -- --ignored --nocapture --test-threads=1
STATUS=$?
set -e

if [[ $STATUS -eq 0 ]]; then
  echo "world-context live smoke: PASS"
else
  echo "world-context live smoke: FAIL (exit $STATUS)" >&2
  echo "--- chrome log ---" >&2
  tail -n 60 /tmp/ultra-instinct-world-context-chrome.log >&2 || true
fi
exit $STATUS
