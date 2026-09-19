//! Exercises [`CompileService`] against a real `cargo` where the property
//! under test needs one (a genuine compile, a genuine type error) and a
//! stub standing in for `cargo` where it does not (rejection before any
//! process runs, a deadline, mutual exclusion) — never a description of
//! what `cargo` would do.
//!
//! Every real-`cargo` test, and the `CompileService` under every test
//! here, points `CARGO_TARGET_DIR` at this repository's shared
//! `target/fixture-wasm` (see `crates/plugin-host/tests/support/mod.rs`),
//! so a warm dependency cache is reused instead of every test paying for
//! its own cold build of `wit-bindgen` and friends. All of them are
//! additionally serialized behind one process-wide lock: this machine
//! must never run two Rust builds at once, and `cargo test`'s default
//! parallel harness would otherwise start more than one at a time.

use std::path::{Path, PathBuf};
use std::time::Duration;

use senken_indicator_compile::{CompileError, CompileRequest, CompileService, Toolchain};

static BUILD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn plugin_api_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugin-api")
}

fn shared_target_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixture-wasm")
}

/// A copy of `crates/plugin-host/tests/fixtures/deterministic/src/lib.rs`
/// — a real, already-proven-correct indicator, used here to prove this
/// crate's own compile path rather than inventing a new source to trust.
const VALID_SOURCE: &str = r#"
use std::cell::Cell;

use senken_plugin_api::{
    Bar, Guest, GuestInstance, IndicatorDescriptor, OnBarResult, ParamValue, PlotValue,
};

struct Deterministic;

impl Guest for Deterministic {
    type Instance = Instance;

    fn descriptor() -> IndicatorDescriptor {
        IndicatorDescriptor {
            id: "deterministic".into(),
            title: "Deterministic".into(),
            short_title: "DET".into(),
            legend: String::new(),
            params: vec![],
            plots: vec![],
        }
    }
}

struct Instance {
    bars_seen: Cell<u32>,
}

impl GuestInstance for Instance {
    fn new(_params: Vec<ParamValue>) -> Self {
        Instance {
            bars_seen: Cell::new(0),
        }
    }

    fn handle_bar(&self, bar: Bar) -> OnBarResult {
        self.bars_seen.set(self.bars_seen.get() + 1);
        let mut acc: u64 = 0;
        let iterations = 1000 + (bar.close.value.unsigned_abs() % 1000);
        for i in 0..iterations {
            acc = acc.wrapping_add(i ^ bar.close.value.unsigned_abs());
        }
        OnBarResult {
            plots: vec![PlotValue {
                field: "acc".into(),
                value: acc as f64,
            }],
            drawables: vec![],
        }
    }

    fn initialized(&self) -> bool {
        self.bars_seen.get() > 0
    }

    fn reset(&self) {
        self.bars_seen.set(0);
    }
}

senken_plugin_api::export!(Deterministic);
"#;

/// Same shape as [`VALID_SOURCE`], with one deliberate type error on a
/// known line: assigning a string literal to a `u32` binding.
const TYPE_ERROR_SOURCE: &str = r#"
use senken_plugin_api::{Guest, GuestInstance, IndicatorDescriptor, OnBarResult, Bar, ParamValue};

struct Broken;

impl Guest for Broken {
    type Instance = Instance;

    fn descriptor() -> IndicatorDescriptor {
        let x: u32 = "not a number";
        IndicatorDescriptor {
            id: "broken".into(),
            title: "Broken".into(),
            short_title: "BRK".into(),
            legend: String::new(),
            params: vec![],
            plots: vec![],
        }
    }
}

struct Instance;

impl GuestInstance for Instance {
    fn new(_params: Vec<ParamValue>) -> Self {
        Instance
    }

    fn handle_bar(&self, _bar: Bar) -> OnBarResult {
        OnBarResult { plots: vec![], drawables: vec![] }
    }

    fn initialized(&self) -> bool {
        true
    }

    fn reset(&self) {}
}

senken_plugin_api::export!(Broken);
"#;

/// The 1-based line `TYPE_ERROR_SOURCE`'s bad assignment sits on — counted
/// by hand from the literal above, and asserted against directly so a
/// future edit to the fixture that silently moves the line is caught by
/// this test failing, not by it passing for the wrong reason.
const TYPE_ERROR_LINE: u32 = 10;

/// Not part of the regular suite (`#[ignore]`, run by hand): measures a
/// genuinely cold first compile — a fresh `CARGO_TARGET_DIR` that has
/// never seen `senken-plugin-api`'s own dependency tree, unlike every
/// other test here, which shares the warm `target/fixture-wasm` cache on
/// purpose. Left in the tree so the number in this crate's own report can
/// be reproduced later, not run on every `cargo test` — a cold
/// `wasm32-wasip2` build of `wit-bindgen` and its own dependents costs
/// real time and disk for no benefit once the number has been recorded.
#[tokio::test]
#[ignore = "measures a genuinely cold build; run explicitly with `--ignored`"]
async fn first_compile_in_a_pristine_target_dir_is_slow_second_is_fast() {
    let toolchain = Toolchain::detect().await.unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    // Deliberately *not* `.with_target_dir(shared_target_dir())` — this
    // test's whole point is a target directory nothing has ever built in.
    let service = CompileService::new(toolchain, work_dir.path(), plugin_api_path());

    let started = std::time::Instant::now();
    service
        .compile(
            CompileRequest {
                source: VALID_SOURCE,
                crate_name: "fixture_compile_cold",
            },
            Duration::from_mins(5),
        )
        .await
        .unwrap();
    eprintln!(
        "cold first compile, pristine target dir: {:?}",
        started.elapsed()
    );

    let started_second = std::time::Instant::now();
    service
        .compile(
            CompileRequest {
                source: VALID_SOURCE,
                crate_name: "fixture_compile_cold",
            },
            Duration::from_mins(3),
        )
        .await
        .unwrap();
    eprintln!(
        "second compile, now-warm target dir: {:?}",
        started_second.elapsed()
    );
}

#[tokio::test]
async fn a_valid_indicator_compiles_to_a_component_that_the_host_can_load() {
    let _guard = BUILD_LOCK.lock().await;
    let toolchain = Toolchain::detect()
        .await
        .expect("this workspace's own rust-toolchain.toml pins wasm32-wasip2");
    let work_dir = tempfile::tempdir().unwrap();
    let service = CompileService::new(toolchain, work_dir.path(), plugin_api_path())
        .with_target_dir(shared_target_dir());

    let started = std::time::Instant::now();
    let wasm = service
        .compile(
            CompileRequest {
                source: VALID_SOURCE,
                crate_name: "fixture_compile_valid",
            },
            Duration::from_mins(3),
        )
        .await
        .expect("a real, already-proven indicator must compile");
    let elapsed = started.elapsed();
    eprintln!("first compile (cold or warm cache): {elapsed:?}");

    let host =
        senken_plugin_host::PluginHost::new(senken_plugin_host::PluginLimits::default()).unwrap();
    let loaded = host
        .load(&wasm)
        .expect("the host must load what this crate compiled");
    assert_eq!(loaded.descriptor().id, "deterministic");

    // Compiling the same source again reuses the warm dependency cache in
    // `CARGO_TARGET_DIR` — this is the number the plan's "under 2 seconds"
    // goal is measured against.
    let started_second = std::time::Instant::now();
    service
        .compile(
            CompileRequest {
                source: VALID_SOURCE,
                crate_name: "fixture_compile_valid",
            },
            Duration::from_mins(3),
        )
        .await
        .expect("recompiling identical source must also succeed");
    let elapsed_second = started_second.elapsed();
    eprintln!("second compile (warm cache): {elapsed_second:?}");
}

#[tokio::test]
async fn a_type_error_is_reported_with_its_line_and_column() {
    let _guard = BUILD_LOCK.lock().await;
    let toolchain = Toolchain::detect().await.unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let service = CompileService::new(toolchain, work_dir.path(), plugin_api_path())
        .with_target_dir(shared_target_dir());

    let error = service
        .compile(
            CompileRequest {
                source: TYPE_ERROR_SOURCE,
                crate_name: "fixture_compile_type_error",
            },
            Duration::from_mins(3),
        )
        .await
        .expect_err("a string literal cannot be a `u32`");

    let CompileError::Failed { diagnostics, .. } = error else {
        panic!("expected a `Failed` compile error, got {error:?}");
    };
    assert!(!diagnostics.is_empty(), "the type error must be reported");
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic.line, Some(TYPE_ERROR_LINE));
    assert!(
        diagnostic.message.contains("mismatched types") || diagnostic.message.contains("expected"),
        "unexpected message: {}",
        diagnostic.message
    );
}

#[tokio::test]
async fn include_str_is_rejected_before_cargo_runs() {
    let _guard = BUILD_LOCK.lock().await;
    // A `cargo` that does not exist at all: if `compile` ever tried to
    // spawn it, this would fail as `CompileError::Io`, not `Rejected` —
    // which is exactly the distinction this test exists to prove.
    let toolchain = Toolchain::stub("/nonexistent/cargo-does-not-exist");
    let work_dir = tempfile::tempdir().unwrap();
    let service = CompileService::new(toolchain, work_dir.path(), plugin_api_path());

    let source = r#"
        fn evil() -> &'static str {
            include_str!("/etc/passwd")
        }
    "#;
    let error = service
        .compile(
            CompileRequest {
                source,
                crate_name: "fixture_compile_rejected",
            },
            Duration::from_secs(5),
        )
        .await
        .expect_err("include_str! must be rejected");
    assert!(
        matches!(error, CompileError::Rejected(_)),
        "expected Rejected, got {error:?}"
    );
}

/// Writes an executable shell script at `dir/name` and returns its path.
fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[tokio::test]
async fn a_build_that_exceeds_the_deadline_is_killed() {
    let _guard = BUILD_LOCK.lock().await;
    let scripts_dir = tempfile::tempdir().unwrap();
    // `exec` replaces the script's own process image with `sleep`, so the
    // process this crate spawns and the process that is actually sleeping
    // are the same PID — killing the child we hold kills the sleep
    // directly, rather than orphaning a grandchild that would keep
    // running for the full 30 seconds regardless.
    // The stub records its own pid before `exec` replaces its image, and
    // `exec` keeps the pid — so the file names the very process this test
    // spawned. Asking the process list for "sleep 30" instead would ask
    // about the whole machine: any unrelated sleep, from a build script or
    // another test run, would fail this test while the property it names
    // held perfectly.
    let pid_path = scripts_dir.path().join("child.pid");
    let cargo_stub = write_script(
        scripts_dir.path(),
        "cargo-sleep",
        &format!("echo $$ > \"{}\"\nexec sleep 30", pid_path.display()),
    );
    let toolchain = Toolchain::stub(&cargo_stub);
    let work_dir = tempfile::tempdir().unwrap();
    let service = CompileService::new(toolchain, work_dir.path(), plugin_api_path());

    let error = service
        .compile(
            CompileRequest {
                source: "// harmless, never reaches a real compiler",
                crate_name: "fixture_compile_timeout",
            },
            // Long enough that the child is certainly running before the
            // deadline lands. At 200ms the budget was spent writing the
            // crate and spawning, so the sleep never started — and the
            // assertion below then held for the wrong reason, because
            // there was nothing alive to kill.
            Duration::from_secs(2),
        )
        .await
        .expect_err("a 30-second sleep must not finish within the deadline");
    assert!(
        matches!(error, CompileError::Timeout(_)),
        "expected Timeout, got {error:?}"
    );

    // Give the kill a moment to actually land, then confirm the process
    // is gone by process list — not by inferring it from `compile`
    // returning, which only proves this crate *asked* it to die.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let pid = std::fs::read_to_string(&pid_path)
        .expect("the stub must have recorded its pid before exec")
        .trim()
        .to_owned();
    let still_running = std::process::Command::new("kill")
        .args(["-0", &pid])
        .output()
        .is_ok_and(|output| output.status.success());
    assert!(
        !still_running,
        "the killed build's own process (pid {pid}) must not still be running"
    );
}

#[tokio::test]
async fn two_compiles_do_not_run_concurrently() {
    let _guard = BUILD_LOCK.lock().await;
    let scripts_dir = tempfile::tempdir().unwrap();
    let log_path = scripts_dir.path().join("timeline.log");
    let cargo_stub = write_script(
        scripts_dir.path(),
        "cargo-timeline",
        &format!(
            r#"echo "start $(date +%s%N)" >> "{log}"
sleep 0.3
echo "end $(date +%s%N)" >> "{log}""#,
            log = log_path.display()
        ),
    );
    let toolchain = Toolchain::stub(&cargo_stub);
    let work_dir = tempfile::tempdir().unwrap();
    let service = CompileService::new(toolchain, work_dir.path(), plugin_api_path());

    let request_a = CompileRequest {
        source: "// a",
        crate_name: "fixture_compile_timeline",
    };
    let request_b = CompileRequest {
        source: "// b",
        crate_name: "fixture_compile_timeline",
    };
    // Neither call is expected to return `Ok` — the stub never produces a
    // `.wasm` — only that both attempted to build, one after the other.
    let _ = tokio::join!(
        service.compile(request_a, Duration::from_secs(5)),
        service.compile(request_b, Duration::from_secs(5)),
    );

    let log = std::fs::read_to_string(&log_path).unwrap();
    let events: Vec<(&str, u64)> = log
        .lines()
        .map(|line| {
            let (kind, nanos) = line.split_once(' ').unwrap();
            (kind, nanos.parse().unwrap())
        })
        .collect();
    assert_eq!(
        events.len(),
        4,
        "both builds must have run to completion: {log}"
    );
    // "start, end, start, end" — never "start, start, end, end", which is
    // what two builds actually running at the same time would produce.
    assert_eq!(events[0].0, "start");
    assert_eq!(events[1].0, "end");
    assert_eq!(events[2].0, "start");
    assert_eq!(events[3].0, "end");
    assert!(
        events[1].1 <= events[2].1,
        "the first build's end must not be after the second build's start: {events:?}"
    );
}
