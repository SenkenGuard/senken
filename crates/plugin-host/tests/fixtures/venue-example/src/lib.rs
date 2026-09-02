//! A minimal, well-behaved venue plugin: spot instruments and 1-minute
//! candles from OKX's own documented shape, fetched entirely through the
//! host's `fetch` — this crate never imports `wasi:sockets` or
//! `wasi:http`, and has no way to.
//!
//! This exists to prove the dynamic venue pipeline end to end against a
//! **recorded** response, not a hand-written one: `crates/plugin-host`'s
//! own tests point this plugin's `fetch` at a mock server serving
//! `plugins/okx/tests/fixtures/instruments.json` and `candles_1m.json`
//! verbatim, and check the bars this plugin returns against the exact
//! values `plugins/okx`'s own, already-tested `OkxBarSource` asserts for
//! the same bytes. Matching that adapter is the point: this plugin
//! implements a deliberately small slice of the same OKX shape (spot
//! instruments, one-minute history candles), not a second, independent
//! parser that happens to agree by coincidence.

wit_bindgen::generate!({
    path: "../../../../../wit/senken.wit",
    world: "venue-plugin",
});

use exports::senken::plugin_api::venue::{Bar, Guest, Instrument, VenueDescriptor, VenueError};
use senken::plugin_api::http;
use senken::plugin_api::types::{BarSpec, BarUnit, Scaled, Volume};

struct ExampleVenue;

impl Guest for ExampleVenue {
    fn descriptor() -> VenueDescriptor {
        VenueDescriptor {
            id: "example-okx".to_owned(),
            name: "Example OKX (proof of pipeline)".to_owned(),
            base_url: "https://www.okx.com".to_owned(),
        }
    }

    fn instruments() -> Result<Vec<Instrument>, VenueError> {
        let body = http::fetch("/api/v5/public/instruments?instType=SPOT", 1)
            .map_err(VenueError::Fetch)?;
        let document: serde_json::Value =
            serde_json::from_slice(&body).map_err(|err| VenueError::Decode(err.to_string()))?;
        if document.get("code").and_then(|c| c.as_str()) != Some("0") {
            let msg = document
                .get("msg")
                .and_then(|m| m.as_str())
                .unwrap_or_default();
            return Err(VenueError::Rejected(msg.to_owned()));
        }
        let rows = document
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        let mut instruments = Vec::new();
        for row in rows {
            let inst_id = row.get("instId").and_then(|v| v.as_str()).unwrap_or("");
            let base = row.get("baseCcy").and_then(|v| v.as_str()).unwrap_or("");
            let quote = row.get("quoteCcy").and_then(|v| v.as_str()).unwrap_or("");
            let tick_sz = row.get("tickSz").and_then(|v| v.as_str()).unwrap_or("");
            let lot_sz = row.get("lotSz").and_then(|v| v.as_str()).unwrap_or("");
            let state = row.get("state").and_then(|v| v.as_str()).unwrap_or("");
            // Skip anything this minimal parser cannot represent, exactly
            // as `plugins/okx`'s own `skip` does — one unusable row is
            // never fatal to the rest of the catalog.
            let (Some((tick_scale, tick_size)), Some((qty_scale, step_size))) = (
                plain_decimal_increment(tick_sz),
                plain_decimal_increment(lot_sz),
            ) else {
                continue;
            };
            if inst_id.is_empty() || base.is_empty() || quote.is_empty() {
                continue;
            }
            if state != "live" {
                continue;
            }
            instruments.push(Instrument {
                symbol: format!("{base}{quote}"),
                source_symbol: inst_id.to_owned(),
                name: format!("{base} / {quote}"),
                base: base.to_owned(),
                quote: quote.to_owned(),
                price_scale: tick_scale,
                tick_size,
                qty_scale,
                step_size,
            });
        }
        Ok(instruments)
    }

    fn supported_specs() -> Vec<BarSpec> {
        vec![BarSpec {
            step: 1,
            unit: BarUnit::Minute,
        }]
    }

    fn max_rows() -> u32 {
        100
    }

    fn bars(
        source_symbol: String,
        spec: BarSpec,
        range_start: i64,
        range_end: i64,
    ) -> Result<Vec<Bar>, VenueError> {
        if spec.step != 1 || spec.unit != BarUnit::Minute {
            return Err(VenueError::Rejected(format!(
                "unsupported bar spec {spec:?}"
            )));
        }
        let path =
            format!("/api/v5/market/history-candles?instId={source_symbol}&bar=1m&limit=100");
        let body = http::fetch(&path, 5).map_err(VenueError::Fetch)?;
        let document: serde_json::Value =
            serde_json::from_slice(&body).map_err(|err| VenueError::Decode(err.to_string()))?;
        if document.get("code").and_then(|c| c.as_str()) != Some("0") {
            let msg = document
                .get("msg")
                .and_then(|m| m.as_str())
                .unwrap_or_default();
            return Err(VenueError::Rejected(msg.to_owned()));
        }
        let rows: Vec<[String; 9]> = document
            .get("data")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|err| VenueError::Decode(err.to_string()))?
            .unwrap_or_default();

        // The widest decimal precision actually present in this batch,
        // mirroring `senken_venue::common_scale`: every price in one
        // column must share one fixed-point scale.
        let price_scale = rows
            .iter()
            .flat_map(|row| [&row[1], &row[2], &row[3], &row[4]])
            .map(|value| decimal_places(value))
            .max()
            .unwrap_or(0);
        let qty_scale = rows
            .iter()
            .flat_map(|row| [&row[5], &row[6]])
            .map(|value| decimal_places(value))
            .max()
            .unwrap_or(0);

        let mut bars = Vec::with_capacity(rows.len());
        for row in rows {
            let [
                ts,
                open,
                high,
                low,
                close,
                volume,
                quote_volume,
                _unused,
                confirm,
            ] = row;
            // Never persist a still-forming candle — verified present even
            // on this history endpoint, exactly as `plugins/okx`'s own
            // `bars.rs` documents.
            if confirm != "1" {
                continue;
            }
            let Ok(ts_ms) = ts.parse::<i64>() else {
                continue;
            };
            let ts_open = ts_ms.saturating_mul(1_000_000);
            if ts_open < range_start || ts_open >= range_end {
                continue;
            }
            let (Some(open), Some(high), Some(low), Some(close), Some(volume)) = (
                scaled_at(&open, price_scale),
                scaled_at(&high, price_scale),
                scaled_at(&low, price_scale),
                scaled_at(&close, price_scale),
                scaled_at(&volume, qty_scale),
            ) else {
                continue;
            };
            bars.push(Bar {
                ts_open,
                spec,
                open,
                high,
                low,
                close,
                volume: Volume::Real(volume),
                quote_volume: scaled_at(&quote_volume, qty_scale),
                trade_count: None,
                taker_buy_volume: None,
            });
        }
        bars.sort_by_key(|bar| bar.ts_open);
        Ok(bars)
    }
}

/// How many digits follow the decimal point in a plain (non-scientific)
/// decimal string such as `"0.00000001"`. `0` for an integer or an
/// unparsable string — good enough for this fixture's own recorded rows,
/// none of which use scientific notation (unlike `plugins/okx`'s own
/// `Num`, which this fixture deliberately does not depend on — a wasm
/// guest has no reason to pull in `senken-venue`'s HTTP-client machinery
/// for one decimal parser).
fn decimal_places(raw: &str) -> u8 {
    raw.split_once('.')
        .map_or(0, |(_, frac)| frac.trim_end_matches('0').len())
        .try_into()
        .unwrap_or(0)
}

/// Parses `raw` as a fixed-point integer at `scale` decimal places, the
/// same contract `senken_core::parse_scaled` states.
fn scaled_at(raw: &str, scale: u8) -> Option<Scaled> {
    let (whole, frac) = raw.split_once('.').unwrap_or((raw, ""));
    if frac.len() > usize::from(scale) {
        return None;
    }
    let padded = format!("{frac:0<width$}", width = usize::from(scale));
    let digits = format!("{whole}{padded}");
    digits
        .parse::<i64>()
        .ok()
        .map(|value| Scaled { scale, value })
}

/// A tick/lot-size string's own scale and increment, as an integer at that
/// scale — the same shape `senken_marketdata::parse_increment` computes,
/// reimplemented minimally here for the same reason [`decimal_places`] is:
/// this fixture has no dependency on the crate that already does this.
fn plain_decimal_increment(raw: &str) -> Option<(u8, i64)> {
    if raw.is_empty() {
        return None;
    }
    let scale = decimal_places(raw);
    scaled_at(raw, scale).map(|scaled| (scaled.scale, scaled.value))
}

export!(ExampleVenue);
