//! `senken bars <source>:<symbol> <spec> --from … --to …`.
//!
//! This is what turns its own done-criterion into something real: it prints
//! a `plan()` preflight, starts an `ensure()` backfill, and streams progress
//! from `SeriesLoader::subscribe()` until the job reaches a terminal state —
//! through the same locked, buffered stdout writer every other subcommand
//! uses (`print_stdout` is a workspace lint; nothing here uses `println!`).

use std::io::Write;
use std::process::ExitCode;

use anyhow::Context;
use senken_core::{TimeRange, UnixNanos};
use senken_loader::{Phase, Priority};
use senken_marketdata::InstrumentId;
use senken_runtime::Runtime;
use senken_series::{Anchor, BarSpec, Origin, SeriesKey};

use crate::format;

/// Runs one `bars` backfill to completion, printing a preflight line, one
/// line per meaningful progress change, and a final outcome line.
///
/// # Errors
/// Returns an error if the arguments cannot be parsed, or if writing to
/// `out` fails. An unknown instrument or an unregistered bar source is
/// reported on stderr and returned as [`ExitCode::FAILURE`], not an error —
/// the same convention [`crate::execute`] already uses for `search`/
/// `instrument`.
pub(crate) async fn run(
    out: &mut impl Write,
    runtime: &Runtime,
    instrument: &str,
    spec: &str,
    from: &str,
    to: &str,
) -> anyhow::Result<ExitCode> {
    let id = InstrumentId::parse(instrument)
        .with_context(|| format!("`{instrument}` is not a valid `source:symbol`"))?;
    let spec: BarSpec = spec
        .parse()
        .with_context(|| format!("`{spec}` is not a valid bar spec, e.g. `1m`, `1h`, `1d`"))?;
    let from = parse_time(from)?;
    let to = parse_time(to)?;
    let Some(range) = TimeRange::new(from, to) else {
        eprintln!("--from must be strictly before --to");
        return Ok(ExitCode::FAILURE);
    };

    let Some(hit) = runtime
        .marketdata()
        .instrument(&id)
        .await
        .context("looking up the instrument")?
    else {
        eprintln!("no instrument `{id}`");
        return Ok(ExitCode::FAILURE);
    };
    let Some(loader) = runtime.series().loader(id.source()) else {
        eprintln!("no bar source registered for `{}`", id.source());
        return Ok(ExitCode::FAILURE);
    };

    // Always `Origin::Venue`: this command backfills what the venue itself
    // reports, never a locally-derived aggregate.
    let key = SeriesKey::new(id.source(), id.symbol(), Origin::Venue, spec);
    // No CLI flag for a non-UTC anchor yet (the anchor only matters for
    // Day-and-above venue-native series, and every current `BarSource`
    // requests the UTC variant where one exists — see e.g. `senken-plugin-okx`).
    let anchor = Anchor::UTC;

    let requirement = loader
        .plan(&key, range, anchor)
        .context("planning the backfill")?;
    format::bars_plan(out, &requirement)?;
    out.flush()?;

    let handle = loader.ensure(
        &key,
        range,
        anchor,
        hit.instrument.price_scale,
        hit.instrument.qty_scale,
        Priority::Visible,
    );
    let job_id = handle.id();

    // `subscribe()` coalesces to at most one broadcast per 100ms except for
    // a job's first and terminal phase transitions, which always publish
    // immediately — `jobs()`/`job()` reflect the live snapshot
    // regardless, which is what this loop actually reads on every wake.
    let mut updates = loader.subscribe();
    let mut printed = None;
    while let Some(snapshot) = loader.job(job_id) {
        let progress = (
            snapshot.phase,
            snapshot.chunks_done,
            snapshot.last_error.clone(),
        );
        if printed.as_ref() != Some(&progress) {
            format::bars_progress(
                out,
                snapshot.phase,
                snapshot.chunks_done,
                snapshot.chunks_total,
                snapshot.bars_written,
                snapshot.last_error.as_deref(),
            )?;
            out.flush()?;
            printed = Some(progress);
        }
        if snapshot.phase == Phase::Done {
            break;
        }
        if updates.changed().await.is_err() {
            // The loader (and with it every sender) is gone — the last
            // snapshot already read above is as current as this loop can
            // get.
            break;
        }
    }

    let outcome = handle.wait().await;
    format::bars_outcome(out, &outcome)?;

    Ok(match outcome {
        senken_loader::JobOutcome::Completed => ExitCode::SUCCESS,
        senken_loader::JobOutcome::Failed(_) | senken_loader::JobOutcome::Cancelled => {
            ExitCode::FAILURE
        }
    })
}

/// Parses `raw` as RFC 3339 (`2026-08-29T00:00:00Z`) or, for convenience, a
/// bare `YYYY-MM-DD` date (midnight UTC).
fn parse_time(raw: &str) -> anyhow::Result<UnixNanos> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        let nanos = dt
            .timestamp_nanos_opt()
            .with_context(|| format!("`{raw}` is out of range"))?;
        return Ok(UnixNanos::from_nanos(nanos));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        let midnight = date
            .and_hms_opt(0, 0, 0)
            .expect("midnight is always a valid time of day");
        let nanos = midnight
            .and_utc()
            .timestamp_nanos_opt()
            .with_context(|| format!("`{raw}` is out of range"))?;
        return Ok(UnixNanos::from_nanos(nanos));
    }
    anyhow::bail!(
        "`{raw}` is not RFC 3339 (e.g. `2026-08-29T00:00:00Z`) or a plain date (`2026-08-29`)"
    )
}

#[cfg(test)]
mod tests {
    use super::parse_time;
    use senken_core::UnixNanos;

    #[test]
    fn rfc3339_parses_exactly() {
        assert_eq!(
            parse_time("2026-08-30T00:00:00Z").unwrap(),
            UnixNanos::from_secs(1_788_048_000).unwrap()
        );
    }

    #[test]
    fn a_bare_date_means_utc_midnight() {
        assert_eq!(
            parse_time("2026-08-30").unwrap(),
            UnixNanos::from_secs(1_788_048_000).unwrap()
        );
    }

    #[test]
    fn garbage_is_rejected_not_guessed() {
        assert!(parse_time("not a date").is_err());
    }
}
