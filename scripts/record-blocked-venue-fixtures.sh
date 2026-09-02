#!/usr/bin/env bash
# Records the venue responses this project's fixtures rule requires, for the
# venues an ordinary egress cannot reach.
#
# Binance answers HTTP 451 ("unavailable for legal reasons") and Bybit's
# CloudFront answers 403 ("blocked access from your country") from a
# restricted jurisdiction, so no fixture for them can be recorded from such a
# network. Run this from one that can reach them — the same one the server
# will run on, or through the proxy it uses:
#
#   HTTPS_PROXY=http://user:pass@host:port ./scripts/record-blocked-venue-fixtures.sh
#
# It writes only into `plugins/*/tests/fixtures/` and refuses to overwrite an
# existing file, so a fixture already recorded from a real response is never
# silently replaced by a worse one.
#
# Every response is saved verbatim. Do not edit the files it produces:
# AGENTS.md's rule is that a fixture is a recording, and hand-written venue
# data has hidden real bugs in this project before (BitMart's `-1` precision,
# Phemex's missing spot tickSize, Gate's zero order_size_min).
set -euo pipefail

cd "$(dirname "$0")/.."
recorded=0
skipped=0
failed=0

record() {
  local path="$1" url="$2"
  if [ -e "$path" ]; then
    echo "  skip    $path (already recorded)"
    skipped=$((skipped + 1))
    return
  fi
  mkdir -p "$(dirname "$path")"
  local code
  code=$(curl -sS -o "$path.tmp" -w '%{http_code}' "$url" || echo "000")
  if [ "$code" != "200" ]; then
    echo "  FAILED  $path — HTTP $code"
    [ "$code" = "451" ] && echo "          451 means this network is still geo-blocked; use the proxy."
    [ "$code" = "403" ] && echo "          403 from CloudFront means the same for Bybit."
    [ "$code" = "418" ] && echo "          418 is a Binance ban. STOP — further requests extend it."
    rm -f "$path.tmp"
    failed=$((failed + 1))
    return
  fi
  mv "$path.tmp" "$path"
  echo "  ok      $path"
  recorded=$((recorded + 1))
}

echo "Binance — spot, USD-M and COIN-M klines"
record plugins/binance/tests/fixtures/klines_usdm_1h.json \
  "https://fapi.binance.com/fapi/v1/klines?symbol=BTCUSDT&interval=1h&limit=3"
record plugins/binance/tests/fixtures/klines_coinm_1h.json \
  "https://dapi.binance.com/dapi/v1/klines?symbol=BTCUSD_PERP&interval=1h&limit=3"
record plugins/binance/tests/fixtures/depth_spot.json \
  "https://api.binance.com/api/v3/depth?symbol=BTCUSDT&limit=5"

echo "Bybit — spot, linear, inverse klines and depth"
for category in spot linear inverse; do
  record "plugins/bybit/tests/fixtures/kline_${category}_1h.json" \
    "https://api.bybit.com/v5/market/kline?category=${category}&symbol=BTCUSDT&interval=60&limit=3"
  record "plugins/bybit/tests/fixtures/orderbook_${category}.json" \
    "https://api.bybit.com/v5/market/orderbook?category=${category}&symbol=BTCUSDT&limit=5"
done

echo
echo "recorded $recorded, skipped $skipped, failed $failed"
if [ "$failed" -gt 0 ]; then
  echo "Some responses could not be recorded. Nothing was written for those."
  exit 1
fi
