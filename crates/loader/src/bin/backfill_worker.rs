//! A tiny, single-purpose worker used only by
//! `crates/loader/tests/cross_process_resumption.rs`
//!  to prove "jobs resume for free" as a real, observed property rather
//! than a design claim: that test spawns this binary as a genuine OS child
//! process, kills it mid-backfill with a hard `SIGKILL`-equivalent, and
//! restarts a fresh instance of it over the same range.
//!
//! Deliberately minimal: one [`senken_loader::SeriesLoader`] backed by an
//! in-memory, no-network [`senken_loader::BarSource`] (per this session's
//! access boundary — no venue network at all), driven to completion over a
//! caller-specified range, printing one `CHUNK <n>` line to stdout every
//! time a chunk is durably written and `DONE`/`FAILED <reason>` at the end.
//! The test kills this process right after the first `CHUNK` line, so it
//! never needs a fixed sleep to time the kill.
//!
//! # Usage
//! `backfill_worker <data_dir> <source_id> <symbol> <range_start_secs>
//! <range_end_secs> <max_rows_per_chunk> <delay_ms_per_chunk> <fetch_log_path>`

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use senken_core::{TimeRange, UnixNanos};
use senken_loader::{
    BarSource, FetchError, JobOutcome, Phase, Priority, SeriesLoaderBuilder, SystemClock,
};
use senken_series::{Bar, BarSpec, BarUnit, Origin, SeriesKey};
use senken_store::Store;

/// A [`BarSource`] that fabricates one bar per minute of the requested
/// range (no network — this session's access boundary forbids it) and
/// appends `<start_ns>,<end_ns>` to a shared log file for every chunk it is
/// asked for, *after* simulating `delay` of fetch latency. The log is what
/// lets the test prove a chunk already written before the kill is never
/// asked for again by the restarted process: every distinct chunk range
/// this source is ever asked to fetch, across both runs, is recorded here.
struct LoggingSource {
    max_rows: usize,
    delay: Duration,
    log_path: PathBuf,
}

#[async_trait::async_trait]
impl BarSource for LoggingSource {
    fn source_id(&self) -> &'static str {
        "cross-process-test"
    }

    fn max_rows(&self) -> usize {
        self.max_rows
    }

    async fn bars(
        &self,
        _symbol: &str,
        spec: BarSpec,
        range: TimeRange,
    ) -> Result<Vec<Bar>, FetchError> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .map_err(|error| FetchError::Rejected(format!("opening fetch log: {error}")))?;
        writeln!(
            file,
            "{},{}",
            range.start().as_nanos(),
            range.end().as_nanos()
        )
        .map_err(|error| FetchError::Rejected(format!("writing fetch log: {error}")))?;
        Ok(one_bar_per_minute(range, spec))
    }
}

/// One synthetic bar per `spec`-aligned bucket start covering `range`,
/// ascending — deterministic content, since this test only cares about
/// *which ranges* were fetched and written, never about realistic prices.
fn one_bar_per_minute(range: TimeRange, spec: BarSpec) -> Vec<Bar> {
    let step = spec
        .duration_nanos()
        .expect("this worker only ever runs at a fixed-duration spec (one minute)");
    let mut bars = Vec::new();
    let mut t = range.start().as_nanos();
    while t < range.end().as_nanos() {
        bars.push(Bar {
            ts_open: UnixNanos::from_nanos(t),
            open: 1,
            high: 1,
            low: 1,
            close: 1,
            volume: 1,
            quote_volume: None,
            trade_count: None,
            taker_buy_volume: None,
        });
        t += step;
    }
    bars
}

/// Writes one line to stdout and flushes immediately. `write!`/`writeln!`
/// against an explicit handle, not `println!` (the workspace's
/// `clippy::print_stdout` lint forbids that macro directly) — the same
/// locked-stdout convention `apps/senken`'s CLI already uses.
fn flushed_println(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

async fn run(args: Args) -> i32 {
    let store = Store::new(args.data_dir);
    if let Err(error) = store.init() {
        flushed_println(&format!("FAILED store init: {error}"));
        return 1;
    }

    let spec = BarSpec::new(1, BarUnit::Minute);
    let key = SeriesKey::new(args.source_id, args.symbol, Origin::Venue, spec);
    let Some(range) = TimeRange::new(
        UnixNanos::from_secs(args.range_start_secs).expect("range_start_secs fits UnixNanos"),
        UnixNanos::from_secs(args.range_end_secs).expect("range_end_secs fits UnixNanos"),
    ) else {
        flushed_println("FAILED range_end before range_start");
        return 1;
    };

    let source = Arc::new(LoggingSource {
        max_rows: args.max_rows_per_chunk,
        delay: Duration::from_millis(args.delay_ms_per_chunk),
        log_path: args.fetch_log_path,
    });
    let loader = SeriesLoaderBuilder::new(store, source, Arc::new(SystemClock), spec).build();

    let handle = loader.ensure(
        &key,
        range,
        senken_series::Anchor::UTC,
        0,
        0,
        Priority::Visible,
    );
    let id = handle.id();

    // Report each durably-written chunk as it lands, so the test can react
    // the instant one exists rather than guessing how long that takes.
    let mut last_reported = 0u32;
    loop {
        if let Some(snapshot) = loader.job(id) {
            if snapshot.chunks_done > last_reported {
                last_reported = snapshot.chunks_done;
                flushed_println(&format!("CHUNK {last_reported}"));
            }
            if matches!(snapshot.phase, Phase::Done) {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    match handle.wait().await {
        JobOutcome::Completed => {
            flushed_println("DONE");
            0
        }
        other => {
            flushed_println(&format!("FAILED {other:?}"));
            1
        }
    }
}

struct Args {
    data_dir: PathBuf,
    source_id: String,
    symbol: String,
    range_start_secs: i64,
    range_end_secs: i64,
    max_rows_per_chunk: usize,
    delay_ms_per_chunk: u64,
    fetch_log_path: PathBuf,
}

impl Args {
    fn parse(mut raw: impl Iterator<Item = String>) -> Self {
        raw.next(); // argv[0]
        let mut next = |what: &str| {
            raw.next()
                .unwrap_or_else(|| panic!("missing argument: {what}"))
        };
        Self {
            data_dir: PathBuf::from(next("data_dir")),
            source_id: next("source_id"),
            symbol: next("symbol"),
            range_start_secs: next("range_start_secs")
                .parse()
                .expect("range_start_secs must be an integer"),
            range_end_secs: next("range_end_secs")
                .parse()
                .expect("range_end_secs must be an integer"),
            max_rows_per_chunk: next("max_rows_per_chunk")
                .parse()
                .expect("max_rows_per_chunk must be an integer"),
            delay_ms_per_chunk: next("delay_ms_per_chunk")
                .parse()
                .expect("delay_ms_per_chunk must be an integer"),
            fetch_log_path: PathBuf::from(next("fetch_log_path")),
        }
    }
}

fn main() {
    let args = Args::parse(std::env::args());
    // A current-thread runtime with only the timer driver enabled — this
    // worker needs no I/O reactor, and pulling one in would need cargo
    // features this crate's production `tokio` dependency deliberately
    // does not enable (`enable_all()` would silently need them).
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("failed to build a current-thread tokio runtime");
    let exit_code = runtime.block_on(run(args));
    std::process::exit(exit_code);
}
