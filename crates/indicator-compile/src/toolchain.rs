//! [`Toolchain`]: whether this machine can build a `wasm32-wasip2`
//! component at all, and the `cargo` this crate runs to do it.

use std::path::PathBuf;
use std::process::Stdio;

use tokio::process::Command;

/// Why [`Toolchain::detect`] could not confirm a usable toolchain.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ToolchainError {
    /// No `cargo` executable could be run at all.
    #[error("no working `cargo` was found on this machine")]
    CargoNotFound,
    /// `cargo` runs, but the target this crate always builds for is not
    /// installed (or `rustup` itself, which is how this project's own
    /// `rust-toolchain.toml` installs it, could not be asked).
    #[error("the `{target}` target is not installed — {hint}")]
    TargetMissing {
        /// The Rust target triple that is missing.
        target: &'static str,
        /// What to run to fix it, shown to whoever operates this server.
        hint: String,
    },
}

/// The target every user indicator is built for — a `wasm32-wasip2`
/// component `senken-plugin-host` can load, the same target this
/// project's own dynamic-indicator fixtures build to.
pub const TARGET: &str = "wasm32-wasip2";

/// A confirmed-usable Rust toolchain: `cargo` runs, and it can target
/// [`TARGET`].
#[derive(Debug, Clone)]
pub struct Toolchain {
    pub(crate) cargo: PathBuf,
    /// `cargo --version`'s own output, kept only to show whoever operates
    /// this server which toolchain answered `detect`.
    pub rustc_version: String,
}

impl Toolchain {
    /// Confirms `cargo` runs and that `wasm32-wasip2` is installed for it,
    /// the same target `rust-toolchain.toml` pins for this workspace's own
    /// build.
    ///
    /// # Errors
    /// [`ToolchainError::CargoNotFound`] if no `cargo` on `PATH` runs at
    /// all; [`ToolchainError::TargetMissing`] if `rustup target list
    /// --installed` does not list [`TARGET`] (including if `rustup` itself
    /// could not be run — this project's own toolchain pin assumes rustup
    /// manages targets, so a machine without it is treated the same as one
    /// missing the target).
    pub async fn detect() -> Result<Self, ToolchainError> {
        let version_output = Command::new("cargo")
            .arg("--version")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await
            .map_err(|_| ToolchainError::CargoNotFound)?;
        if !version_output.status.success() {
            return Err(ToolchainError::CargoNotFound);
        }
        let rustc_version = String::from_utf8_lossy(&version_output.stdout)
            .trim()
            .to_owned();

        let targets_output = Command::new("rustup")
            .args(["target", "list", "--installed"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await;
        let has_target = targets_output.is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line.trim() == TARGET)
        });
        if !has_target {
            return Err(ToolchainError::TargetMissing {
                target: TARGET,
                hint: format!("run `rustup target add {TARGET}`"),
            });
        }

        Ok(Self {
            cargo: PathBuf::from("cargo"),
            rustc_version,
        })
    }

    /// Builds a `Toolchain` pointing at an arbitrary executable in place
    /// of a real `cargo`, bypassing every check [`detect`](Self::detect)
    /// performs.
    ///
    /// Exists for this crate's own tests, which substitute a script that
    /// sleeps (to prove a deadline kills it) or a path that does not exist
    /// at all (to prove a rejected source never reaches a process spawn).
    /// Production code always goes through [`detect`](Self::detect).
    #[doc(hidden)]
    #[must_use]
    pub fn stub(cargo: impl Into<PathBuf>) -> Self {
        Self {
            cargo: cargo.into(),
            rustc_version: "stub".to_owned(),
        }
    }
}
