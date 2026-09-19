//! [`CompileService`]: turns one indicator's Rust source into a
//! `wasm32-wasip2` component, or a typed reason it could not.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::toolchain::{TARGET, Toolchain, ToolchainError};

/// A source form rejected before `cargo` ever runs, and the product-facing
/// reason shown for it.
///
/// This is a rejection of *form*, not a security boundary — the boundary
/// that actually matters is the WASM sandbox `senken-plugin-host` runs the
/// compiled component inside, which only exists once the component has
/// already been built. What this list closes is narrower: every one of
/// these forms lets the source reach outside the generated crate *while
/// `cargo build` is running*, on the machine that hosts this compile
/// service, before that sandbox exists at all.
const FORBIDDEN_FORMS: &[(&str, &str)] = &[
    (
        "include_str!",
        "reading a file at compile time (`include_str!`)",
    ),
    (
        "include_bytes!",
        "reading a file at compile time (`include_bytes!`)",
    ),
    ("#![feature", "nightly-only language features"),
    ("extern \"C\"", "calling into non-Rust code"),
    ("unsafe", "`unsafe` code"),
    ("std::process", "starting another process"),
    ("std::fs", "reading or writing files"),
    ("std::net", "opening a network connection"),
    ("#[path", "redirecting a module to another file (`#[path]`)"),
];

/// Above this, a produced component is rejected rather than loaded — a
/// coarse ceiling against a runaway or malicious build product, not a
/// tuned limit.
const MAX_WASM_BYTES: usize = 8 * 1024 * 1024;

/// One error-level diagnostic from `cargo build --message-format=json`,
/// with the span this crate could resolve against the user's own
/// `src/lib.rs` (the generated `Cargo.toml` never has one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// `cargo`'s own level string (`"error"` for every diagnostic this
    /// crate collects — warnings never fail a build and are not surfaced).
    pub level: String,
    /// The diagnostic's short message, as `rustc` phrases it.
    pub message: String,
    /// 1-based line in the user's `src/lib.rs`, if `rustc` named one.
    pub line: Option<u32>,
    /// 1-based column, if `rustc` named one.
    pub column: Option<u32>,
    /// `rustc`'s fully rendered diagnostic text (may include an internal
    /// path to the generated crate) — logged server-side, never sent to a
    /// client, since it can name a path on this machine.
    pub rendered: String,
}

/// One indicator's source, ready to compile.
#[derive(Debug, Clone, Copy)]
pub struct CompileRequest<'a> {
    /// The Rust source for the generated crate's `src/lib.rs`.
    pub source: &'a str,
    /// The generated crate's package name (and, with `-` turned into `_`,
    /// its compiled artifact's file stem).
    pub crate_name: &'a str,
}

/// Why [`CompileService::compile`] did not return a compiled component.
#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    /// The source used a form this service refuses outright, named in the
    /// message. `cargo` was never invoked.
    #[error("{0}")]
    Rejected(String),
    /// The toolchain this service was built with is not usable.
    #[error(transparent)]
    Toolchain(#[from] ToolchainError),
    /// The build did not finish within its deadline and was killed.
    #[error("compile did not finish within {0:?}")]
    Timeout(Duration),
    /// `cargo build` exited with a failure status.
    #[error("compile failed with {} diagnostic(s)", diagnostics.len())]
    Failed {
        /// Every error-level diagnostic `cargo` reported.
        diagnostics: Vec<Diagnostic>,
        /// The tail of the build's combined stdout/stderr, for the server
        /// log — never sent to a client (see [`Diagnostic::rendered`]).
        log_tail: String,
    },
    /// The produced component exceeded this service's size ceiling.
    #[error("compiled component is larger than the {MAX_WASM_BYTES}-byte limit")]
    TooLarge,
    /// Spawning or driving the `cargo` process itself failed — a problem
    /// with this machine, not with the user's source.
    #[error("running cargo failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Runs one build at a time, in a crate this service itself generates from
/// [`CompileRequest::source`] plus a single dependency on
/// `senken-plugin-api`.
///
/// # Errors
/// See [`CompileError`]'s variants.
pub struct CompileService {
    toolchain: Toolchain,
    /// Where the generated crate is written — always `<work_dir>/crate`,
    /// overwritten on every compile (never two at once; see `BUILD_LOCK`).
    work_dir: PathBuf,
    /// Where `CARGO_TARGET_DIR` points. Defaults to `<work_dir>/target`;
    /// [`Self::with_target_dir`] overrides it, which this crate's own
    /// tests use to share this repository's `target/fixture-wasm` cache
    /// rather than pay for a cold dependency build in a fresh temp
    /// directory on every test run.
    target_dir: PathBuf,
    /// Absolute path to `crates/plugin-api`, written into the generated
    /// crate's `Cargo.toml` as a `path` dependency. Correct for a
    /// developer-machine prototype; the day this SDK is published to a
    /// registry, this one constant is what changes.
    sdk_path: PathBuf,
}

/// Enforces "one build at a time" across the whole process.
///
/// Deliberately not a field: what this protects is the machine's Rust
/// build — its CPU, its disk, and `cargo`'s own lock on a target
/// directory — not one service's scratch space. A per-service lock lets
/// two services run two `cargo build`s at once, which is the thing that
/// must never happen, and it happens by accident as soon as anything
/// constructs more than one service.
static BUILD_LOCK: Mutex<()> = Mutex::const_new(());

impl CompileService {
    /// Builds a service that writes its generated crate under `work_dir`
    /// and points `CARGO_TARGET_DIR` at `<work_dir>/target` by default.
    pub fn new(
        toolchain: Toolchain,
        work_dir: impl Into<PathBuf>,
        sdk_path: impl Into<PathBuf>,
    ) -> Self {
        let work_dir = work_dir.into();
        let target_dir = work_dir.join("target");
        Self {
            toolchain,
            work_dir,
            target_dir,
            sdk_path: sdk_path.into(),
        }
    }

    /// Overrides where `CARGO_TARGET_DIR` points. See the field's own docs
    /// for why this exists.
    #[doc(hidden)]
    #[must_use]
    pub fn with_target_dir(mut self, target_dir: impl Into<PathBuf>) -> Self {
        self.target_dir = target_dir.into();
        self
    }

    /// Compiles `req.source` to a `wasm32-wasip2` component, giving the
    /// build up to `timeout` before killing it.
    ///
    /// # Errors
    /// See [`CompileError`].
    pub async fn compile(
        &self,
        req: CompileRequest<'_>,
        timeout: Duration,
    ) -> Result<Vec<u8>, CompileError> {
        if let Some(reason) = reject_source(req.source) {
            return Err(CompileError::Rejected(reason));
        }

        let _guard = BUILD_LOCK.lock().await;

        let crate_dir = self.work_dir.join("crate");
        let src_dir = crate_dir.join("src");
        tokio::fs::create_dir_all(&src_dir).await?;
        tokio::fs::write(
            crate_dir.join("Cargo.toml"),
            generated_manifest(req.crate_name, &self.sdk_path),
        )
        .await?;
        tokio::fs::write(src_dir.join("lib.rs"), req.source).await?;

        let mut command = Command::new(&self.toolchain.cargo);
        command
            .args([
                "build",
                "--release",
                "--target",
                TARGET,
                "--message-format=json",
            ])
            .current_dir(&crate_dir)
            .env("CARGO_TARGET_DIR", &self.target_dir)
            .env_remove("RUSTFLAGS")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn()?;
        let missing_pipe = || std::io::Error::other("cargo's stdout/stderr was not piped");
        let mut stdout = child.stdout.take().ok_or_else(missing_pipe)?;
        let mut stderr = child.stderr.take().ok_or_else(missing_pipe)?;

        let drive = async {
            let mut stdout_buf = Vec::new();
            let mut stderr_buf = Vec::new();
            let (stdout_result, stderr_result, status) = tokio::join!(
                stdout.read_to_end(&mut stdout_buf),
                stderr.read_to_end(&mut stderr_buf),
                child.wait(),
            );
            stdout_result?;
            stderr_result?;
            let status = status?;
            Ok::<_, std::io::Error>((status, stdout_buf, stderr_buf))
        };

        let (status, stdout_buf, stderr_buf) = match tokio::time::timeout(timeout, drive).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(io_error)) => return Err(CompileError::Io(io_error)),
            Err(_elapsed) => {
                // `drive` only borrowed `child`/`stdout`/`stderr` above (it
                // never took ownership), so they are still ours here — the
                // explicit kill is what the deadline promises, rather than
                // leaving it to `kill_on_drop` on some later drop.
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(CompileError::Timeout(timeout));
            }
        };

        if !status.success() {
            let diagnostics = parse_error_diagnostics(&stdout_buf);
            let combined = [stdout_buf.as_slice(), stderr_buf.as_slice()].concat();
            let log_tail = tail_str(&combined, 4000);
            return Err(CompileError::Failed {
                diagnostics,
                log_tail,
            });
        }

        let artifact_name = format!("{}.wasm", req.crate_name.replace('-', "_"));
        let wasm_path = self
            .target_dir
            .join(TARGET)
            .join("release")
            .join(artifact_name);
        let wasm = tokio::fs::read(&wasm_path).await?;
        if wasm.len() > MAX_WASM_BYTES {
            return Err(CompileError::TooLarge);
        }
        Ok(wasm)
    }
}

/// The generated crate's `Cargo.toml` — regenerated on every compile,
/// never accepted from a caller, so nothing but `senken-plugin-api` can
/// ever be a dependency of code this service builds.
fn generated_manifest(crate_name: &str, sdk_path: &std::path::Path) -> String {
    // `sdk_path` is a path this server itself controls (never user input),
    // so a plain `Display`-quoted string is enough — not general TOML
    // string escaping.
    format!(
        r#"[workspace]

[package]
name = "{crate_name}"
version = "0.0.0"
edition = "2024"
publish = false

[lib]
crate-type = ["cdylib"]

[dependencies]
senken-plugin-api = {{ path = "{}" }}

[profile.release]
opt-level = "s"
debug = false
"#,
        sdk_path.display()
    )
}

/// The first rejected form found in `source`, as a product-facing
/// sentence, or `None` if it passes every check.
fn reject_source(source: &str) -> Option<String> {
    FORBIDDEN_FORMS
        .iter()
        .find(|(needle, _)| source.contains(needle))
        .map(|(needle, reason)| format!("Indicators may not use {reason} (found `{needle}`)."))
}

/// A `cargo build --message-format=json` line, as much of it as this crate
/// reads. See <https://doc.rust-lang.org/cargo/reference/external-tools.html#json-messages>.
#[derive(Debug, Deserialize)]
struct CargoLine {
    reason: String,
    #[serde(default)]
    message: Option<CompilerMessage>,
}

#[derive(Debug, Deserialize)]
struct CompilerMessage {
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    rendered: Option<String>,
    #[serde(default)]
    spans: Vec<Span>,
}

#[derive(Debug, Deserialize)]
struct Span {
    file_name: String,
    line_start: u32,
    column_start: u32,
}

/// Collects every error-level `compiler-message` in `stdout`, resolving
/// each to the span in the user's own `src/lib.rs` when `rustc` named one.
/// Lines that are not JSON (rare, but `rustc` can still write plain text
/// on a hard crash) are skipped rather than failing the whole parse.
fn parse_error_diagnostics(stdout: &[u8]) -> Vec<Diagnostic> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<CargoLine>(line).ok())
        .filter(|line| line.reason == "compiler-message")
        .filter_map(|line| line.message)
        .filter(|message| message.level.as_deref() == Some("error"))
        .map(|message| {
            let span = message
                .spans
                .iter()
                .find(|span| span.file_name.ends_with("src/lib.rs"));
            Diagnostic {
                level: message.level.unwrap_or_default(),
                message: message.message.unwrap_or_default(),
                line: span.map(|span| span.line_start),
                column: span.map(|span| span.column_start),
                rendered: message.rendered.unwrap_or_default(),
            }
        })
        .collect()
}

/// The last `max_bytes` of `bytes`, decoded lossily — for a log tail that
/// must never panic on a boundary landing mid-UTF-8-sequence.
fn tail_str(bytes: &[u8], max_bytes: usize) -> String {
    let start = bytes.len().saturating_sub(max_bytes);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}
