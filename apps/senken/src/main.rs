//! `senken` — one binary for the CLI, the server, and (optionally) the
//! desktop shell.
//!
//! Mode selection happens at **runtime**, never at compile time: shipping a
//! separate "CLI build" and "GUI build" would let their behaviour drift
//! silently. `senken serve` and `senken gui` both start the server through
//! the exact same [`senken_api::serve`] call (see `run_serve`); the
//! pre-existing market-data subcommands are `senken_cli::Command` flattened
//! into this binary's own subcommand list, so they run through the one
//! `senken_cli::run` code path regardless of which binary calls them.

use std::io::{self, IsTerminal, Write as _};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{ArgAction, CommandFactory, Parser, Subcommand};
use senken_api::ServeOptions;
use senken_identity::IdentityStore;
use senken_runtime::DEFAULT_DATA_DIR;

#[cfg(feature = "gui")]
mod gui;
mod logging;

/// `senken`: multi-venue market data, server and desktop shell.
#[derive(Debug, Parser)]
#[command(name = "senken", version, about)]
struct Cli {
    /// Log more (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides this.
    #[arg(short, long, action = ArgAction::Count, global = true)]
    verbose: u8,

    /// Where cached data lives.
    #[arg(long, global = true, env = "SENKEN_DATA_DIR", default_value_os_t = default_data_dir())]
    data_dir: PathBuf,

    #[command(subcommand)]
    command: Option<Command>,
}

/// Where this user's Senken data lives when `--data-dir` and
/// `SENKEN_DATA_DIR` are both unset.
///
/// A distributed binary must not keep its database beside whatever directory
/// the user happened to be in when they ran it: doing that gives one person
/// several unrelated installations depending on their shell's history, and
/// none of them where any backup or uninstall would look.
///
/// | Platform | Directory |
/// |---|---|
/// | Linux | `$XDG_DATA_HOME/senken`, else `~/.local/share/senken` |
/// | macOS | `~/Library/Application Support/senken` |
/// | Windows | `%LOCALAPPDATA%\senken` |
///
/// This is the *local* application-data directory, not the roaming one. On
/// Windows the roaming directory is synchronised between machines when a
/// user signs in, and this holds a Parquet market-data cache that can reach
/// gigabytes — copying it over a network at login would be a fault, not a
/// feature. It is also data rather than configuration, which is why Linux
/// lands on `~/.local/share` rather than `~/.config`.
///
/// Falls back to [`DEFAULT_DATA_DIR`] — a relative `.data` — when no home
/// directory can be determined at all, which is the ordinary case inside a
/// scratch container.
fn default_data_dir() -> PathBuf {
    dirs::data_local_dir().map_or_else(
        || PathBuf::from(DEFAULT_DATA_DIR),
        |base| base.join("senken"),
    )
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Market-data subcommands: `sources`, `source`, `search`, `instrument`,
    /// `refresh` — unchanged from the pre-existing `senken` CLI.
    #[command(flatten)]
    Data(senken_cli::Command),

    /// Start the API server (and, with the default `web` feature, the
    /// embedded web app) without a desktop window.
    Serve {
        /// Interface to bind.
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        /// Port to bind. `0` asks the OS for a free one.
        #[arg(long, default_value_t = 4180)]
        port: u16,
        /// An additional origin allowed to make cross-origin browser
        /// requests. Repeatable. The server's own origin
        /// needs no entry; there is no wildcard option.
        #[arg(long = "cors-origin", value_name = "ORIGIN")]
        cors_origins: Vec<String>,
    },

    /// Start the server on a Tokio thread and open a desktop window onto it.
    #[cfg(feature = "gui")]
    Gui {
        /// Interface the embedded server binds. The window always loads it
        /// through this local server, never a separate asset pipeline.
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        /// Port the embedded server binds. `0` asks the OS for a free one.
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Load the window from this URL instead of the embedded app.
        ///
        /// For development: point it at a Vite dev server to get hot reload
        /// in the desktop window. The embedded server still runs, and the
        /// dev server proxies `/api` back to it.
        #[arg(long, value_name = "URL")]
        ui: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Data(sub)) => run_data(cli.verbose, &cli.data_dir, sub).await,
        Some(Command::Serve {
            host,
            port,
            cors_origins,
        }) => run_serve(cli.verbose, &cli.data_dir, host, port, cors_origins).await,
        #[cfg(feature = "gui")]
        Some(Command::Gui { host, port, ui }) => {
            run_gui(cli.verbose, &cli.data_dir, host, port, ui.as_deref()).await
        }
        // No subcommand: a TTY means someone typed `senken` and wants help;
        // no TTY means the binary was launched by double-clicking it, so it
        // opens the desktop window when that feature is compiled in.
        None if std::io::stdout().is_terminal() => {
            print_help()?;
            Ok(ExitCode::SUCCESS)
        }
        #[cfg(feature = "gui")]
        None => {
            run_gui(
                cli.verbose,
                &cli.data_dir,
                IpAddr::from([127, 0, 0, 1]),
                0,
                None,
            )
            .await
        }
        #[cfg(not(feature = "gui"))]
        None => {
            eprintln!(
                "no terminal attached and this build has no `gui` feature; run `senken --help` from a terminal"
            );
            Ok(ExitCode::FAILURE)
        }
    }
}

/// Prints `--help` to stdout without the `println!`/`print!` macros that
/// `clippy::print_stdout` (workspace lint,) forbids.
fn print_help() -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    Cli::command().write_help(&mut out)?;
    out.write_all(b"\n")?;
    Ok(())
}

/// Runs one market-data subcommand — the pre-existing CLI, unchanged.
async fn run_data(
    verbose: u8,
    data_dir: &std::path::Path,
    sub: senken_cli::Command,
) -> anyhow::Result<ExitCode> {
    logging::init(verbose);
    let runtime = senken_cli::runtime_with_plugins(data_dir)?;
    let exit = senken_cli::run(&runtime, sub).await;
    runtime.shutdown().context("shutdown was not clean")?;
    exit
}

/// Opens (creating if absent) the accounts database reserved at
/// `<data_dir>/accounts/accounts.db`.
fn open_identity_store(
    data_dir: &std::path::Path,
) -> anyhow::Result<std::sync::Arc<senken_identity::IdentityStore>> {
    let path = data_dir.join("accounts").join("accounts.db");
    Ok(std::sync::Arc::new(
        IdentityStore::open(&path).with_context(|| {
            format!("failed to open the accounts database at {}", path.display())
        })?,
    ))
}

/// Builds the runtime `serve`/`gui` hand to `senken-api`:
/// every venue plugin registered, the same set `senken_cli::runtime_with_plugins`
/// already assembles for the market-data subcommands — one wiring path, not
/// two, for the same reason `run_serve`/`run_gui` already share [`senken_api::serve`]
/// itself.
fn open_runtime(
    data_dir: &std::path::Path,
) -> anyhow::Result<std::sync::Arc<senken_runtime::Runtime>> {
    Ok(std::sync::Arc::new(senken_cli::runtime_with_plugins(
        data_dir,
    )?))
}

/// `senken serve`: the server alone, run until `Ctrl-C`.
async fn run_serve(
    verbose: u8,
    data_dir: &std::path::Path,
    host: IpAddr,
    port: u16,
    cors_origins: Vec<String>,
) -> anyhow::Result<ExitCode> {
    // Stays at the shared default: `announce` below reports the address
    // unconditionally, so raising the log level would only duplicate it in a
    // second format. `-v` is still what turns on the tracing detail.
    logging::init(verbose);
    let identity = open_identity_store(data_dir)?;
    let runtime = open_runtime(data_dir)?;
    let handle = senken_api::serve(
        ServeOptions {
            host,
            port,
            allowed_origins: cors_origins,
        },
        identity,
        runtime,
    )
    .await
    .context("failed to start the server")?;
    announce(handle.local_addr(), host);

    tokio::signal::ctrl_c()
        .await
        .context("failed to listen for Ctrl-C")?;
    tracing::info!("senken serve: shutting down");

    handle
        .shutdown()
        .await
        .context("server did not shut down cleanly")?;
    Ok(ExitCode::SUCCESS)
}

/// Reports where the server can actually be reached.
///
/// This is program output, not a diagnostic, so it bypasses the verbosity
/// filter entirely: a server that does not tell you its URL is unusable, and
/// `RUST_LOG=error` must not be able to hide it. It goes to stderr so stdout
/// stays clean for piping, matching the rest of the binary.
///
/// `bound` is what the OS gave us — it differs from the requested address
/// whenever port `0` was asked for, which is `gui`'s default.
fn announce(bound: SocketAddr, requested_host: IpAddr) {
    let mut err = io::stderr().lock();
    let port = bound.port();

    if requested_host.is_unspecified() {
        // `0.0.0.0` binds every interface, so no single URL is the whole
        // truth; name the loopback one and say the rest exist.
        let _ = writeln!(err, "senken listening on http://127.0.0.1:{port}");
        let _ = writeln!(
            err,
            "  also reachable on this machine's other addresses (bound {bound})"
        );
    } else {
        let _ = writeln!(err, "senken listening on http://{bound}");
    }
}

/// `senken gui`: the same server, started on this Tokio runtime, with a
/// desktop window opened onto it. Closing the window shuts the server down
/// and releases the port before the process exits.
#[cfg(feature = "gui")]
async fn run_gui(
    verbose: u8,
    data_dir: &std::path::Path,
    host: IpAddr,
    port: u16,
    ui: Option<&str>,
) -> anyhow::Result<ExitCode> {
    logging::init(verbose);
    let identity = open_identity_store(data_dir)?;
    let runtime = open_runtime(data_dir)?;
    let handle = senken_api::serve(
        ServeOptions {
            host,
            port,
            allowed_origins: Vec::new(),
        },
        identity,
        runtime,
    )
    .await
    .context("failed to start the embedded server")?;
    let addr = handle.local_addr();
    announce(addr, host);

    // Tauri's event loop must run on this thread and blocks it until the
    // window closes; the server keeps running on the Tokio runtime's other
    // worker threads in the meantime.
    let runtime_handle = tokio::runtime::Handle::current();
    let window_result = tokio::task::block_in_place(|| gui::run_window(runtime_handle, addr, ui));

    handle
        .shutdown()
        .await
        .context("server did not shut down cleanly")?;

    window_result.context("desktop window failed")?;
    Ok(ExitCode::SUCCESS)
}
