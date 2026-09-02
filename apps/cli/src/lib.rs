//! `senken-cli` — the market data subcommands shared by every front end.
//!
//! This crate is a library, not a binary. The single `senken` binary lives in
//! `apps/senken`; it owns process startup (argument parsing, logging,
//! `#[tokio::main]`) and flattens [`Command`] into its own subcommand list
//! alongside `serve` and `gui`. Keeping the market-data subcommands here
//! means the CLI's behaviour is exactly one code path, reused rather than
//! reimplemented, no matter which binary calls it.

use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Subcommand, ValueEnum};
use senken_marketdata::{InstrumentKind, InstrumentQuery};
use senken_plugin_binance::BinancePlugin;
use senken_plugin_bingx::BingxPlugin;
use senken_plugin_bitfinex::BitfinexPlugin;
use senken_plugin_bitget::BitgetPlugin;
use senken_plugin_bitmart::BitmartPlugin;
use senken_plugin_bitmex::BitmexPlugin;
use senken_plugin_bitstamp::BitstampPlugin;
use senken_plugin_bybit::BybitPlugin;
use senken_plugin_coinbase::CoinbasePlugin;
use senken_plugin_cryptocom::CryptocomPlugin;
use senken_plugin_deribit::DeribitPlugin;
use senken_plugin_gate::GatePlugin;
use senken_plugin_gemini::GeminiPlugin;
use senken_plugin_htx::HtxPlugin;
use senken_plugin_kraken::KrakenPlugin;
use senken_plugin_kucoin::KucoinPlugin;
use senken_plugin_mexc::MexcPlugin;
use senken_plugin_mt5_hedging::adapter::Mt5HedgingPlugin;
use senken_plugin_okx::OkxPlugin;
use senken_plugin_phemex::PhemexPlugin;
use senken_plugin_poloniex::PoloniexPlugin;
use senken_plugin_simulator::SimulatorPlugin;
use senken_plugin_upbit::UpbitPlugin;
use senken_plugin_whitebit::WhitebitPlugin;
use senken_runtime::Runtime;

mod bars;
mod format;

/// The instrument kinds `--kind` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum KindArg {
    /// Immediate exchange of base for quote.
    Spot,
    /// Perpetual swap.
    Perp,
    /// Dated future.
    Future,
    /// Option contract.
    Option,
}

impl From<KindArg> for InstrumentKind {
    fn from(arg: KindArg) -> Self {
        match arg {
            KindArg::Spot => Self::Spot,
            KindArg::Perp => Self::Perpetual,
            KindArg::Future => Self::Future,
            KindArg::Option => Self::Option,
        }
    }
}

/// The market-data subcommands. The binary crate flattens this into its own
/// top-level subcommand list (`#[command(flatten)]` on a newtype variant),
/// so from the user's point of view these sit alongside `serve` and `gui`.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// List registered market data sources.
    Sources,
    /// Show one source with its catalog statistics.
    Source {
        /// Source id, e.g. `okx`.
        id: String,
    },
    /// Search instruments across every source. Prefix with `source:` to
    /// narrow, e.g. `okx:btc`.
    Search {
        /// Free text; matches symbol, base, quote, source id and name.
        query: String,
        /// Only this kind of instrument. Repeat to accept several.
        #[arg(short, long, value_enum)]
        kind: Vec<KindArg>,
        /// Results per page.
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
        /// Zero-based page number.
        #[arg(short, long, default_value_t = 0)]
        page: usize,
    },
    /// Show one instrument by its id, e.g. `okx:BTCUSDT`.
    Instrument {
        /// Fully-qualified instrument id.
        id: String,
    },
    /// Discard a source's cached catalog and refetch it from the venue.
    Refresh {
        /// Source id, e.g. `binance-spot`.
        source: String,
    },
    /// Backfill bars for one instrument, printing progress as it runs
    /// .
    Bars {
        /// `source:symbol`, e.g. `okx:BTCUSDT`.
        instrument: String,
        /// Bar spec, e.g. `1m`, `15m`, `1h`, `1d`.
        spec: String,
        /// Start of the range, inclusive. RFC 3339 or a plain `YYYY-MM-DD`
        /// date (midnight UTC).
        #[arg(long)]
        from: String,
        /// End of the range, exclusive. Same formats as `--from`.
        #[arg(long)]
        to: String,
    },
}

/// Builds a [`Runtime`] with every venue plugin registered.
///
/// Pulled out of the binary's `main` so both the current single-binary
/// front end and any future embedder construct the exact same runtime.
///
/// # Errors
///
/// Returns an error if the runtime fails to start (see [`Runtime::builder`]).
pub fn runtime_with_plugins(data_dir: &Path) -> anyhow::Result<Runtime> {
    build_runtime(Runtime::builder().data_dir(data_dir), data_dir)
}

/// Builds the full venue runtime and reconciles plugin permissions against
/// the identity store during activation.
///
/// # Errors
/// Returns an error if runtime startup fails.
pub fn runtime_with_plugins_and_identity(
    data_dir: &Path,
    identity: std::sync::Arc<senken_identity::IdentityStore>,
) -> anyhow::Result<Runtime> {
    build_runtime(
        Runtime::builder()
            .data_dir(data_dir)
            .identity_store(identity),
        data_dir,
    )
}

fn build_runtime(
    builder: senken_runtime::RuntimeBuilder,
    data_dir: &Path,
) -> anyhow::Result<Runtime> {
    builder
        // The paper broker keeps its books on disk, so unlike every venue
        // plugin below it is constructed with the data directory rather
        // than as a unit struct.
        .plugin(SimulatorPlugin::new(senken_storage::Storage::new(data_dir)))
        // Same reason: a simulated MT5 hedging account keeps its tickets
        // on disk.
        .plugin(Mt5HedgingPlugin::new(senken_storage::Storage::new(
            data_dir,
        )))
        .plugin(BinancePlugin)
        .plugin(UpbitPlugin)
        .plugin(PhemexPlugin)
        .plugin(HtxPlugin)
        .plugin(BitmartPlugin)
        .plugin(BitfinexPlugin)
        .plugin(BingxPlugin)
        .plugin(BitgetPlugin)
        .plugin(BitmexPlugin)
        .plugin(BitstampPlugin)
        .plugin(BybitPlugin)
        .plugin(CoinbasePlugin)
        .plugin(CryptocomPlugin)
        .plugin(DeribitPlugin)
        .plugin(GatePlugin)
        .plugin(GeminiPlugin)
        .plugin(KrakenPlugin)
        .plugin(KucoinPlugin)
        .plugin(MexcPlugin)
        .plugin(OkxPlugin)
        .plugin(PoloniexPlugin)
        .plugin(WhitebitPlugin)
        .build()
        .context("failed to start the Senken runtime")
}

/// Runs one market-data subcommand against `runtime` and prints its result
/// to a locked, buffered stdout handle (diagnostics go to stderr).
///
/// This is the entire behaviour of the pre-existing `senken` CLI, unchanged
/// down to the broken-pipe handling: `senken search … | head` closing the
/// pipe early is the reader saying "enough", not a failure of this program.
///
/// # Errors
///
/// Returns an error if the subcommand itself fails (e.g. an unknown source)
/// or if writing to stdout fails for a reason other than a broken pipe.
pub async fn run(runtime: &Runtime, command: Command) -> anyhow::Result<ExitCode> {
    let mut out = BufWriter::new(io::stdout().lock());
    let exit = execute(&mut out, runtime, command).await;
    let flushed = out.flush().context("failed to write to stdout");

    match exit.and_then(|code| flushed.map(|()| code)) {
        Ok(code) => Ok(code),
        Err(error) if is_broken_pipe(&error) => Ok(ExitCode::SUCCESS),
        Err(error) => Err(error),
    }
}

/// `true` when `error` was caused, at any depth, by a closed pipe.
fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|io| io.kind() == io::ErrorKind::BrokenPipe)
    })
}

async fn execute(
    out: &mut impl Write,
    runtime: &Runtime,
    command: Command,
) -> anyhow::Result<ExitCode> {
    let marketdata = runtime.marketdata();

    match command {
        Command::Sources => {
            for source in marketdata.sources() {
                writeln!(out, "{:<14} {}", source.id, source.name)?;
            }
        }
        Command::Source { id } => {
            let detail = marketdata.source_detail(&id).await?;
            writeln!(out, "{:<14} {}", detail.id, detail.name)?;
            writeln!(out, "instruments    {}", detail.instrument_count)?;
            writeln!(out, "tradable       {}", detail.tradable_count)?;
            writeln!(out, "synced at      {}", detail.synced_at.to_rfc3339())?;
        }
        Command::Search {
            query,
            kind,
            limit,
            page,
        } => {
            let query = kind.into_iter().fold(
                InstrumentQuery::new(&query).with_page(page, limit),
                |query, kind| query.with_kind(kind.into()),
            );
            let results = marketdata.instruments(query).await;
            format::page(out, &results)?;
            if !results.is_complete() {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Instrument { id } => {
            let Some(hit) = marketdata.instrument(id.as_str()).await? else {
                eprintln!("no instrument `{id}`");
                return Ok(ExitCode::FAILURE);
            };
            format::detail(out, &hit)?;
        }
        Command::Refresh { source } => {
            let detail = marketdata.refresh(&source).await?;
            writeln!(
                out,
                "{} refreshed: {} instruments ({} tradable)",
                detail.id, detail.instrument_count, detail.tradable_count
            )?;
        }
        Command::Bars {
            instrument,
            spec,
            from,
            to,
        } => {
            return bars::run(out, runtime, &instrument, &spec, &from, &to).await;
        }
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::{io, is_broken_pipe};
    use crate::format;
    use senken_marketdata::InstrumentPage;
    use std::io::Write;

    /// A reader that hung up, as `| head` does.
    struct ClosedPipe;

    impl Write for ClosedPipe {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn writing_to_a_closed_pipe_reports_broken_pipe() {
        let error = format::page(&mut ClosedPipe, &InstrumentPage::default()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn a_broken_pipe_is_recognised_through_the_error_chain() {
        let wrapped = anyhow::Error::new(io::Error::from(io::ErrorKind::BrokenPipe))
            .context("failed to write to stdout");
        assert!(is_broken_pipe(&wrapped));
    }

    #[test]
    fn other_io_failures_are_still_failures() {
        let denied = anyhow::Error::new(io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(!is_broken_pipe(&denied));
        assert!(!is_broken_pipe(&anyhow::anyhow!("unknown source `nope`")));
    }
}
