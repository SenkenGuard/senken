//! The only place in the workspace a tracing subscriber is installed.
//!
//! Library crates emit events and never configure output; this module
//! decides where they go. Adding a TUI later means changing this file only.

use tracing_subscriber::EnvFilter;

/// Workspace crates whose events `-v` controls. Third-party crates (hyper,
/// reqwest, tokio) stay at `warn` unless `RUST_LOG` says otherwise.
///
/// The binary's own target is its *bin* name, not its package name, so it
/// is taken from the build environment rather than typed by hand.
const OUR_CRATES: &[&str] = &[
    env!("CARGO_CRATE_NAME"),
    "senken_cli",
    "senken_api",
    "senken_marketdata",
    "senken_storage",
    "senken_runtime",
    "senken_plugin",
    "senken_plugin_binance",
    "senken_plugin_okx",
];

fn level_for(verbosity: u8) -> &'static str {
    match verbosity {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    }
}

fn filter(verbosity: u8) -> EnvFilter {
    // An explicit RUST_LOG always wins over -v.
    EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        let level = level_for(verbosity);
        let directives = std::iter::once("warn".to_owned())
            .chain(OUR_CRATES.iter().map(|c| format!("{c}={level}")))
            .collect::<Vec<_>>()
            .join(",");
        EnvFilter::new(directives)
    })
}

/// Installs the subscriber. Diagnostics go to stderr so stdout stays clean
/// for piping.
pub(crate) fn init(verbosity: u8) {
    tracing_subscriber::fmt()
        .with_env_filter(filter(verbosity))
        .with_writer(std::io::stderr)
        .with_target(verbosity >= 2)
        .compact()
        .init();
}
