//! A bounded, per-plugin log — the second half of the guarantee that a
//! broken plugin cannot take the host down.
//!
//! A plugin cannot reach a socket or the filesystem (see the crate's own
//! docs), but its own `stdout`/`stderr` are still ordinary WASI streams a
//! guest can write to without limit — and a plugin caught in a panic loop
//! can print a line every call. A file that a broken plugin can grow forever
//! is just a slower way to exhaust the disk than the network access it was
//! already denied, so this is a fixed-capacity ring instead: the oldest
//! line is dropped to make room for the newest, and the buffer's size is
//! bounded for the lifetime of the plugin, never a function of how much it
//! has printed.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use senken_core::UnixNanos;
use wasmtime_wasi::cli::{IsTerminal, StdoutStream};
use wasmtime_wasi_io::poll::Pollable;
use wasmtime_wasi_io::streams::{OutputStream, StreamResult};

/// Lines retained per plugin instance, oldest evicted first.
///
/// Not a byte budget: a line-oriented cap keeps the ring's contents
/// readable (a half-written line is never shown as if it were whole), and a
/// few hundred lines is already far more than a human reviewing why a
/// plugin was disabled will read.
const DEFAULT_CAPACITY_LINES: usize = 256;

/// How urgent one recorded line is — what lets a plugin's own routine
/// `stdout` prints be told apart from the lines that actually explain why
/// something broke (its `stderr`, and every line the host itself records
/// for a trap or a circuit trip).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginLogSeverity {
    /// The guest's own `stdout`, or an ordinary host-recorded note.
    Info,
    /// The guest's own `stderr` — where a panic hook writes — or a
    /// host-recorded trap or circuit-breaker event.
    Warn,
}

/// One line in a plugin's ring log.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginLogLine {
    /// When this line was recorded.
    pub timestamp: UnixNanos,
    /// How urgent it is.
    pub severity: PluginLogSeverity,
    /// The line's own text, already stripped of its trailing newline.
    pub message: String,
}

/// Real wall-clock time as a [`UnixNanos`], for stamping a log line as it is
/// recorded.
///
/// This crate has no [`senken_series::Clock`] seam of its own to go through:
/// that trait exists to keep a *deterministic* computation (a backtest, a
/// replay) reproducible, and a log timestamp is pure host-side observation
/// that never feeds back into a plugin's own output — the same reason
/// `senken_runtime`'s own `PluginRecord::activated_at` reads
/// `SystemTime::now()` directly rather than through a `Clock`.
fn now() -> UnixNanos {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // A `Duration` since the epoch fits `i64` nanoseconds until year 2262
    // (`UnixNanos`'s own documented range); saturating rather than
    // panicking is strictly for a clock that can never legitimately produce
    // a value this large.
    UnixNanos::from_nanos(i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX))
}

/// A fixed-capacity, line-oriented log for one plugin instance.
///
/// Cheap to clone: every clone shares the same ring, so the two stdio
/// adapters built from one `PluginLog` (see [`PluginLog::stdout`] and
/// [`PluginLog::stderr`]) and the host's own [`PluginLog::record`] calls
/// all append to the same bounded history.
#[derive(Clone)]
pub(crate) struct PluginLog {
    inner: Arc<Mutex<RingLog>>,
}

struct RingLog {
    lines: VecDeque<PluginLogLine>,
    capacity: usize,
    /// Bytes received since the last `\n`, per stream, so a write that
    /// splits a line across two calls is not recorded as two lines.
    stdout_partial: String,
    stderr_partial: String,
}

impl RingLog {
    fn push_line(&mut self, severity: PluginLogSeverity, message: String) {
        if self.lines.len() >= self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(PluginLogLine {
            timestamp: now(),
            severity,
            message,
        });
    }

    fn push_bytes(&mut self, source: &str, bytes: &[u8]) {
        let severity = if source == "stderr" {
            PluginLogSeverity::Warn
        } else {
            PluginLogSeverity::Info
        };
        {
            let partial = if source == "stdout" {
                &mut self.stdout_partial
            } else {
                &mut self.stderr_partial
            };
            partial.push_str(&String::from_utf8_lossy(bytes));
        }
        loop {
            let partial = if source == "stdout" {
                &mut self.stdout_partial
            } else {
                &mut self.stderr_partial
            };
            let Some(newline_at) = partial.find('\n') else {
                break;
            };
            let line: String = partial.drain(..=newline_at).collect();
            let line = line.trim_end_matches(['\n', '\r']).to_string();
            self.push_line(severity, format!("{source}: {line}"));
        }
    }
}

impl PluginLog {
    /// A new, empty log retaining at most [`DEFAULT_CAPACITY_LINES`] lines.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RingLog {
                lines: VecDeque::with_capacity(DEFAULT_CAPACITY_LINES),
                capacity: DEFAULT_CAPACITY_LINES,
                stdout_partial: String::new(),
                stderr_partial: String::new(),
            })),
        }
    }

    /// Records one line from the host itself — a trap's message, a load
    /// failure, a circuit trip — independent of anything the guest wrote to
    /// its own stdout or stderr.
    ///
    /// This is what guarantees a line lands in a plugin's log for *every*
    /// trap, not only the ones where the guest happened to print something
    /// before it died: a fuel-exhausted or epoch-deadline trap interrupts
    /// execution with no chance for the guest's own panic hook to run.
    pub(crate) fn record(&self, severity: PluginLogSeverity, line: impl Into<String>) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_line(severity, line.into());
    }

    /// A snapshot of the retained lines, oldest first.
    #[must_use]
    pub(crate) fn snapshot(&self) -> Vec<PluginLogLine> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .lines
            .iter()
            .cloned()
            .collect()
    }

    /// A `wasi:cli/stdout` implementation that appends every write to this
    /// log, tagged `stdout`.
    #[must_use]
    pub(crate) fn stdout(&self) -> PluginStdio {
        PluginStdio {
            log: self.clone(),
            source: "stdout",
        }
    }

    /// A `wasi:cli/stderr` implementation that appends every write to this
    /// log, tagged `stderr` — where a guest's default panic hook writes.
    #[must_use]
    pub(crate) fn stderr(&self) -> PluginStdio {
        PluginStdio {
            log: self.clone(),
            source: "stderr",
        }
    }
}

impl Default for PluginLog {
    fn default() -> Self {
        Self::new()
    }
}

/// One end of a [`PluginLog`], wired to the guest as either its stdout or
/// its stderr.
#[derive(Clone)]
pub(crate) struct PluginStdio {
    log: PluginLog,
    source: &'static str,
}

impl IsTerminal for PluginStdio {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdoutStream for PluginStdio {
    fn async_stream(&self) -> Box<dyn tokio::io::AsyncWrite + Send + Sync> {
        // Never reached: `wasmtime-wasi`'s own `wasi:cli/stdout` and
        // `wasi:cli/stderr` host implementations call `p2_stream` (below),
        // not this method — see `p2::stdio::stdout::Host::get_stdout` in
        // that crate. A real sink is provided anyway so this type upholds
        // `StdoutStream`'s contract on its own terms rather than one that
        // happens to hold only because of how one caller behaves today.
        Box::new(tokio::io::sink())
    }

    fn p2_stream(&self) -> Box<dyn OutputStream> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl Pollable for PluginStdio {
    async fn ready(&mut self) {}
}

impl OutputStream for PluginStdio {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.log
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_bytes(self.source, &bytes);
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        // A ring never fills — the oldest line makes room for the newest —
        // so a guest writing to its own stdout or stderr is never blocked
        // or refused here.
        Ok(usize::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::{PluginLog, PluginLogSeverity};

    #[test]
    fn a_full_line_is_recorded_once_it_sees_a_newline() {
        let log = PluginLog::new();
        let mut stdout = log.stdout();
        wasmtime_wasi_io::streams::OutputStream::write(&mut stdout, "hello\n".into()).unwrap();
        let snapshot = log.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].message, "stdout: hello");
        assert_eq!(snapshot[0].severity, PluginLogSeverity::Info);
    }

    #[test]
    fn a_write_split_across_two_calls_is_still_one_line() {
        let log = PluginLog::new();
        let mut stdout = log.stdout();
        wasmtime_wasi_io::streams::OutputStream::write(&mut stdout, "hel".into()).unwrap();
        wasmtime_wasi_io::streams::OutputStream::write(&mut stdout, "lo\n".into()).unwrap();
        let snapshot = log.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].message, "stdout: hello");
    }

    #[test]
    fn stdout_and_stderr_share_one_log_but_keep_their_own_tag_and_severity() {
        let log = PluginLog::new();
        let mut stdout = log.stdout();
        let mut stderr = log.stderr();
        wasmtime_wasi_io::streams::OutputStream::write(&mut stdout, "out\n".into()).unwrap();
        wasmtime_wasi_io::streams::OutputStream::write(&mut stderr, "err\n".into()).unwrap();
        let snapshot = log.snapshot();
        assert_eq!(snapshot[0].message, "stdout: out");
        assert_eq!(snapshot[0].severity, PluginLogSeverity::Info);
        assert_eq!(snapshot[1].message, "stderr: err");
        assert_eq!(
            snapshot[1].severity,
            PluginLogSeverity::Warn,
            "a guest's own stderr is where its panic hook writes, so it is \
             surfaced as at least a warning"
        );
    }

    #[test]
    fn the_oldest_line_is_evicted_once_the_ring_is_full() {
        let log = PluginLog::new();
        // Proves the eviction actually fires rather than merely being
        // declared: fill the ring one past capacity and check the first
        // line entered is the one that is gone, not an arbitrary one.
        for i in 0..=super::DEFAULT_CAPACITY_LINES {
            log.record(PluginLogSeverity::Info, format!("line {i}"));
        }
        let snapshot = log.snapshot();
        assert_eq!(snapshot.len(), super::DEFAULT_CAPACITY_LINES);
        assert_eq!(snapshot.first().unwrap().message, "line 1");
        assert_eq!(
            snapshot.last().unwrap().message,
            format!("line {}", super::DEFAULT_CAPACITY_LINES)
        );
    }

    #[test]
    fn record_lands_independently_of_any_guest_output() {
        let log = PluginLog::new();
        log.record(PluginLogSeverity::Warn, "trap: guest panicked");
        let snapshot = log.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].message, "trap: guest panicked");
        assert_eq!(snapshot[0].severity, PluginLogSeverity::Warn);
    }

    #[test]
    fn every_line_carries_a_nonzero_timestamp() {
        // Not a specific value — only that a real wall-clock reading was
        // taken, so a log line displayed in the user's own zone (see this
        // crate's own callers) has something real to convert.
        let log = PluginLog::new();
        log.record(PluginLogSeverity::Info, "hello");
        let snapshot = log.snapshot();
        assert!(snapshot[0].timestamp.as_nanos() > 0);
    }
}
