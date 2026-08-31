//! Jobs resume for free: coverage lives in
//! filenames and files are immutable, so a crash should
//! lose at most the chunk in flight, and a restart should re-plan from
//! whatever is already on disk. M6/M6.5 stated this as a design property
//! but never tested it with a real crash. This test does: it spawns
//! `backfill_worker` (`crates/loader/src/bin/backfill_worker.rs`) as a
//! genuine OS child process, kills it with a hard `SIGKILL`/`TerminateProcess`
//! (via [`std::process::Child::kill`], not a graceful shutdown signal) partway
//! through a backfill, restarts a fresh instance over the *same* range, and
//! then checks — from outside either process, by reading the store and a
//! shared fetch log — that nothing already written was fetched again and
//! nothing is missing or duplicated.
//!
//! Uses a `tempfile::TempDir` for the data directory, never `.data` (this
//! session's access boundary). No venue network is involved anywhere: the
//! worker's `BarSource` fabricates bars in-memory.

use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use senken_core::{TimeRange, UnixNanos};
use senken_series::{Anchor, BarSpec, BarUnit, Origin, SeriesKey};
use senken_store::Store;
use tempfile::TempDir;

const SOURCE_ID: &str = "test-source";
const SYMBOL: &str = "TESTUSD";
/// The whole range this test backfills: 60 one-minute bars.
const RANGE_END_SECS: i64 = 3600;
/// 5-minute chunks over the hour above split it into 12 chunks — enough
/// that killing after the first one leaves most of the job still to do.
const MAX_ROWS_PER_CHUNK: usize = 5;
/// Deliberately slow enough that the test can reliably observe (and act
/// on) the first `CHUNK` line before the whole backfill would finish, but
/// not so slow that a 12-chunk resumed run makes the suite noticeably slower.
const DELAY_MS_PER_CHUNK: u64 = 120;
/// Generous safety net for the channel reads below: real time only bounds
/// how long a genuine bug is allowed to hang the test, never the property
/// being asserted (see each `recv_timeout` call site).
const CHANNEL_TIMEOUT: Duration = Duration::from_secs(20);

fn key() -> SeriesKey {
    SeriesKey::new(
        SOURCE_ID,
        SYMBOL,
        Origin::Venue,
        BarSpec::new(1, BarUnit::Minute),
    )
}

fn full_range() -> TimeRange {
    TimeRange::new(
        UnixNanos::from_secs(0).unwrap(),
        UnixNanos::from_secs(RANGE_END_SECS).unwrap(),
    )
    .unwrap()
}

/// Spawns `backfill_worker` over the whole test range against `data_dir`,
/// logging every fetched chunk to `fetch_log` (shared, and appended to
/// across both the killed run and the restarted one).
fn spawn_worker(data_dir: &std::path::Path, fetch_log: &std::path::Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_backfill_worker"))
        .arg(data_dir)
        .arg(SOURCE_ID)
        .arg(SYMBOL)
        .arg("0")
        .arg(RANGE_END_SECS.to_string())
        .arg(MAX_ROWS_PER_CHUNK.to_string())
        .arg(DELAY_MS_PER_CHUNK.to_string())
        .arg(fetch_log)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("failed to spawn backfill_worker")
}

/// Streams `child`'s stdout, one line per channel message, from a
/// dedicated thread — so the test can wait for a specific line with a
/// bounded `recv_timeout` instead of a blocking read with no timeout at
/// all, or a fixed sleep that would be a guess rather than a proof.
fn stdout_lines(child: &mut Child) -> Receiver<String> {
    let stdout = child.stdout.take().expect("child stdout was piped");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    rx
}

/// Parses the shared fetch log (`start_ns,end_ns` per line, appended to by
/// every `backfill_worker` process that ever ran against it) into the
/// ranges actually requested from the `BarSource`.
fn read_fetch_log(path: &std::path::Path) -> Vec<(i64, i64)> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (start, end) = line
                .split_once(',')
                .unwrap_or_else(|| panic!("malformed fetch log line: {line}"));
            (
                start.trim().parse().expect("fetch log start_ns"),
                end.trim().parse().expect("fetch log end_ns"),
            )
        })
        .collect()
}

/// Spawns the worker, waits for its first `CHUNK` line (proving at least
/// one chunk was durably written — `chunks_done` only advances after
/// `Store::write` returns `Ok`), then kills it with a hard, ungraceful
/// signal — `Child::kill` sends `SIGKILL` on Unix and `TerminateProcess`
/// on Windows, neither of which the worker can catch or clean up after,
/// which is the actual crash this test needs to prove resumption against.
#[test]
fn a_hard_killed_backfill_resumes_with_no_chunk_refetched_and_nothing_lost_or_duplicated() {
    let dir = TempDir::new().unwrap();
    let fetch_log = dir.path().join("fetch_log.csv");

    let mut first_run = spawn_worker(dir.path(), &fetch_log);
    let first_lines = stdout_lines(&mut first_run);
    loop {
        let line = first_lines
            .recv_timeout(CHANNEL_TIMEOUT)
            .expect("the first run produced no output before the safety timeout");
        assert!(
            !line.starts_with("FAILED"),
            "the first run failed before any chunk completed: {line}"
        );
        if line.starts_with("CHUNK ") {
            break;
        }
    }

    // A hard kill, not a graceful shutdown request — this is the actual
    // crash this must be proven resumable from.
    first_run.kill().expect("failed to kill the first run");
    let _ = first_run.wait();

    let store = Store::new(dir.path());
    let coverage_after_kill = store.coverage(&key(), Anchor::UTC).unwrap();
    let covered_secs_after_kill: i64 = coverage_after_kill
        .iter()
        .map(|r| (r.end().as_nanos() - r.start().as_nanos()) / 1_000_000_000)
        .sum();
    assert!(
        covered_secs_after_kill > 0,
        "at least one chunk must have been durably written before the kill took effect"
    );
    assert!(
        covered_secs_after_kill < RANGE_END_SECS,
        "the kill must have interrupted the backfill before it finished, or this test proves nothing"
    );

    let fetched_before_restart = read_fetch_log(&fetch_log);
    assert!(
        !fetched_before_restart.is_empty(),
        "the first run must have logged at least the one fetch behind its first CHUNK line"
    );

    // Restart: a fresh process, same arguments, same (shared) fetch log —
    // exactly "the same job, resumed" rather than a different request.
    let mut second_run = spawn_worker(dir.path(), &fetch_log);
    let second_lines = stdout_lines(&mut second_run);
    loop {
        let line = second_lines
            .recv_timeout(CHANNEL_TIMEOUT)
            .expect("the restarted run produced no output before the safety timeout");
        assert!(
            !line.starts_with("FAILED"),
            "the restarted run must complete, not fail: {line}"
        );
        if line == "DONE" {
            break;
        }
    }
    let status = second_run
        .wait()
        .expect("failed to wait on the restarted run");
    assert!(
        status.success(),
        "the restarted worker must exit successfully"
    );

    // Property 1: the store now fully covers the requested range.
    let final_coverage = store.coverage(&key(), Anchor::UTC).unwrap();
    assert!(
        full_range().subtract(&final_coverage).is_empty(),
        "the full range must be covered after resumption; coverage was {final_coverage:?}"
    );

    // Property 2 (the one this test exists to prove): no chunk that was
    // already fetched and durably written before the kill was fetched
    // again by the restarted process. The log is shared and appended to
    // by both processes, so a duplicate range anywhere in it means a
    // refetch of already-written data.
    let fetched_total = read_fetch_log(&fetch_log);
    let mut seen = std::collections::HashSet::new();
    for &(start, end) in &fetched_total {
        assert!(
            seen.insert((start, end)),
            "chunk [{start}, {end}) in nanoseconds was fetched more than once across the killed and restarted runs — an already-written chunk was refetched"
        );
    }

    // Property 3: nothing was lost or double-planned either — every
    // fetched chunk, from both runs combined, tiles the requested range
    // exactly once with no gap and no overlap.
    let mut sorted = fetched_total.clone();
    sorted.sort_unstable();
    let mut cursor = 0i64;
    for &(start, end) in &sorted {
        assert_eq!(
            start, cursor,
            "fetched chunks must tile the whole range with no gap and no overlap: {sorted:?}"
        );
        cursor = end;
    }
    assert_eq!(
        cursor,
        RANGE_END_SECS * 1_000_000_000,
        "the fetched chunks must exactly cover the whole requested range: {sorted:?}"
    );

    // Property 4: the persisted bar content itself has no missing or
    // duplicated minute — the outcome a user actually sees.
    let mut bars = Vec::new();
    for batch in store.read_range(&key(), Anchor::UTC, full_range()).unwrap() {
        bars.extend(senken_store::bars_from_batch(&batch.unwrap()).unwrap());
    }
    assert_eq!(
        bars.len(),
        usize::try_from(RANGE_END_SECS / 60).unwrap(),
        "every one of the 60 minutes must be present exactly once in the final store"
    );
}
