//! Proves the four confinement mechanisms `crates/plugin-host` exists for,
//! and the circuit breaker and ring log built on top of them, against real
//! compiled `wasm32-wasip2` components with a genuine, deliberate defect
//! each — never against a description of what a broken plugin would do.
//!
//! See `tests/support/mod.rs` for how each fixture is compiled, and
//! `tests/fixtures/*/src/lib.rs` for what each one actually does.

mod support;

use std::time::{Duration, Instant};

use senken_plugin_host::{
    Bar, BarSpec, BarUnit, CircuitState, ExecutionMode, PluginHost, PluginHostError, PluginLimits,
    PluginLogSeverity, Scaled, Volume,
};

fn sample_bar(close: i64) -> Bar {
    Bar {
        ts_open: 1_700_000_000_000_000_000,
        spec: BarSpec {
            step: 1,
            unit: BarUnit::Minute,
        },
        open: Scaled {
            scale: 2,
            value: close,
        },
        high: Scaled {
            scale: 2,
            value: close,
        },
        low: Scaled {
            scale: 2,
            value: close,
        },
        close: Scaled {
            scale: 2,
            value: close,
        },
        volume: Volume::Absent,
        quote_volume: None,
        trade_count: None,
        taker_buy_volume: None,
    }
}

#[test]
fn a_panicking_plugin_returns_err_and_the_host_stays_up() {
    let wasm = std::fs::read(support::build_fixture("panics")).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host
        .load(&wasm)
        .expect("a panicking plugin must still load");

    let mut instance = loaded
        .spawn(&[], ExecutionMode::Backtest { fuel: 10_000_000 })
        .expect("constructing the instance does not itself panic");

    let err = instance
        .handle_bar(sample_bar(100))
        .expect_err("a guest panic must surface as an `Err`, not crash this test process");
    assert!(matches!(err, PluginHostError::Trap(_)), "got {err:?}");

    // One line lands in this instance's own log for the trap, independent
    // of whatever the guest's own panic hook printed to its stderr.
    let logs = instance.logs();
    assert!(
        logs.iter().any(|line| line.message.starts_with("trap:")),
        "expected a host-recorded trap line, got {logs:?}"
    );

    // The host itself is unaffected: it can still load and run an
    // unrelated, well-behaved plugin in the same process.
    let good_wasm = std::fs::read(support::build_fixture("deterministic")).unwrap();
    let good = host.load(&good_wasm).expect("the host must still work");
    let mut good_instance = good
        .spawn(&[], ExecutionMode::Backtest { fuel: 10_000_000 })
        .unwrap();
    good_instance
        .handle_bar(sample_bar(100))
        .expect("an unrelated plugin must be unaffected by another plugin's panic");
}

#[test]
fn an_infinite_loop_is_cut_off_by_the_epoch_deadline() {
    let wasm = std::fs::read(support::build_fixture("loops")).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host.load(&wasm).unwrap();
    let mut instance = loaded
        .spawn(
            &[],
            ExecutionMode::Live {
                deadline: Duration::from_millis(100),
            },
        )
        .unwrap();

    let started = Instant::now();
    let err = instance
        .handle_bar(sample_bar(100))
        .expect_err("a call past its epoch deadline must trap");
    let elapsed = started.elapsed();
    assert!(matches!(err, PluginHostError::Trap(_)), "got {err:?}");
    // Generous relative to the 100ms deadline: proves the call was cut off
    // in the right order of magnitude, not held to the ticker's exact
    // granularity.
    assert!(
        elapsed < Duration::from_secs(2),
        "an infinite loop must be cut off close to its deadline, took {elapsed:?}"
    );

    // The host stays responsive: a second, unrelated call completes
    // immediately right after.
    let good_wasm = std::fs::read(support::build_fixture("deterministic")).unwrap();
    let good = host.load(&good_wasm).unwrap();
    let mut good_instance = good
        .spawn(
            &[],
            ExecutionMode::Live {
                deadline: Duration::from_secs(1),
            },
        )
        .unwrap();
    let responsive_call = Instant::now();
    good_instance.handle_bar(sample_bar(100)).unwrap();
    assert!(
        responsive_call.elapsed() < Duration::from_millis(500),
        "the host must stay responsive to other work while one plugin was looping"
    );
}

#[test]
fn unbounded_allocation_is_denied_by_the_memory_limit_not_a_killed_process() {
    let wasm = std::fs::read(support::build_fixture("allocates")).unwrap();
    let host = PluginHost::new(PluginLimits {
        max_memory_bytes: 4 * 1024 * 1024,
    })
    .unwrap();
    let loaded = host.load(&wasm).unwrap();
    let mut instance = loaded
        .spawn(&[], ExecutionMode::Backtest { fuel: u64::MAX })
        .unwrap();

    let err = instance
        .handle_bar(sample_bar(100))
        .expect_err("growth past the memory ceiling must be denied, surfacing as a trap");
    assert!(matches!(err, PluginHostError::Trap(_)), "got {err:?}");

    // This test process itself is still alive and can keep going — the
    // property under test. A denial that had actually killed the process
    // would have ended this test with no further assertions ever running.
    let good_wasm = std::fs::read(support::build_fixture("deterministic")).unwrap();
    host.load(&good_wasm)
        .expect("the host process must still be usable after denying one plugin's allocation");
}

#[test]
fn repeated_traps_open_the_circuit_breaker_and_disable_the_plugin() {
    let wasm = std::fs::read(support::build_fixture("panics")).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host.load(&wasm).unwrap();
    let mut instance = loaded
        .spawn(&[], ExecutionMode::Backtest { fuel: 10_000_000 })
        .unwrap();

    // Three consecutive traps on the same instance trip the shared,
    // per-plugin breaker.
    for _ in 0..3 {
        let err = instance.handle_bar(sample_bar(1)).unwrap_err();
        assert!(matches!(err, PluginHostError::Trap(_)), "got {err:?}");
    }

    // The now-open breaker fails the next call immediately, with a
    // readable reason, without attempting to call into the guest again.
    let err = instance.handle_bar(sample_bar(1)).unwrap_err();
    match err {
        PluginHostError::CircuitOpen(reason) => {
            assert!(reason.contains("consecutive"), "got {reason:?}");
        }
        other => panic!("expected CircuitOpen once the breaker trips, got {other:?}"),
    }

    // The breaker is shared per plugin, not per instance: a brand new
    // instance of the same plugin is refused too.
    let err = loaded
        .spawn(&[], ExecutionMode::Backtest { fuel: 10_000_000 })
        .expect_err("a tripped breaker must refuse new instances of the same plugin");
    assert!(
        matches!(err, PluginHostError::CircuitOpen(_)),
        "got {err:?}"
    );
}

#[test]
fn a_plugin_reaching_for_a_socket_fails_to_load_not_to_run() {
    let wasm = std::fs::read(support::build_fixture("tries-socket")).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let err = host
        .load(&wasm)
        .expect_err("a component whose imports reach `wasi:sockets` must fail to load");
    assert!(matches!(err, PluginHostError::Load(_)), "got {err:?}");
}

#[test]
fn a_plugin_reaching_for_the_filesystem_fails_to_load_not_to_run() {
    let wasm = std::fs::read(support::build_fixture("tries-fs")).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let err = host
        .load(&wasm)
        .expect_err("a component whose imports reach `wasi:filesystem` must fail to load");
    assert!(matches!(err, PluginHostError::Load(_)), "got {err:?}");
}

#[test]
fn malformed_bytes_fail_to_load_without_taking_down_the_caller() {
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let err = host
        .load(b"not a real component")
        .expect_err("garbage bytes must fail to load, not panic");
    assert!(matches!(err, PluginHostError::Load(_)), "got {err:?}");

    // Startup (or whatever called `load`) must be able to carry on.
    let good_wasm = std::fs::read(support::build_fixture("deterministic")).unwrap();
    host.load(&good_wasm)
        .expect("one bad plugin must not take down loading of the next one");
}

#[test]
fn fuel_consumption_is_deterministic_across_two_separate_instances() {
    let wasm = std::fs::read(support::build_fixture("deterministic")).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host.load(&wasm).unwrap();

    let mut first = loaded
        .spawn(&[], ExecutionMode::Backtest { fuel: 50_000_000 })
        .unwrap();
    let mut second = loaded
        .spawn(&[], ExecutionMode::Backtest { fuel: 50_000_000 })
        .unwrap();

    first.handle_bar(sample_bar(4_242)).unwrap();
    second.handle_bar(sample_bar(4_242)).unwrap();

    let first_fuel = first.last_fuel_consumed().expect("fuel is always tracked");
    let second_fuel = second.last_fuel_consumed().expect("fuel is always tracked");
    assert!(
        first_fuel > 0,
        "the fixture does real work and must cost fuel"
    );
    assert_eq!(
        first_fuel, second_fuel,
        "the same bar on two separate instances must cost exactly the same fuel"
    );
}

/// The ring log and health counters must be scoped to the *plugin*
/// (`LoadedPlugin`), not to one ephemeral instance — a chart's compute
/// request spawns and drops an instance per call, so a Plugins page reading
/// only the instance would see nothing survive past that one request.
#[test]
fn logs_and_health_persist_on_the_loaded_plugin_across_separate_instances() {
    let wasm = std::fs::read(support::build_fixture("panics")).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host.load(&wasm).unwrap();

    assert_eq!(loaded.health().trap_count, 0);
    assert!(loaded.logs().is_empty());

    // First instance: one trap, then it is dropped (ordinary scope exit —
    // no explicit teardown call, the same guarantee `PluginInstance`'s own
    // `Drop` doc comment describes).
    {
        let mut instance = loaded
            .spawn(&[], ExecutionMode::Backtest { fuel: 10_000_000 })
            .unwrap();
        instance.handle_bar(sample_bar(1)).unwrap_err();
    }
    assert_eq!(
        loaded.health().trap_count,
        1,
        "the trap must be visible on the plugin itself once its instance is gone"
    );
    assert!(
        loaded
            .logs()
            .iter()
            .any(|line| line.message.starts_with("trap:")),
        "the trap line must survive on the plugin's own log after the instance that recorded it is dropped"
    );

    // A second, brand new instance shares the same accumulated history —
    // this is what makes the log a *plugin's* log rather than one
    // instance's own scratch buffer.
    let mut second = loaded
        .spawn(&[], ExecutionMode::Backtest { fuel: 10_000_000 })
        .unwrap();
    let logs_before = loaded.logs().len();
    assert_eq!(
        second.logs().len(),
        logs_before,
        "a fresh instance of the same plugin must see the plugin's existing history"
    );
    second.handle_bar(sample_bar(1)).unwrap_err();
    assert_eq!(loaded.health().trap_count, 2);
}

/// `PluginHealth::deadline_exceeded_count` must count only the wall-clock
/// deadline case, not every kind of trap — proven by tripping one of each
/// on the same plugin and checking they land in different buckets.
#[test]
fn deadline_exceeded_count_is_narrower_than_the_total_trap_count() {
    let wasm = std::fs::read(support::build_fixture("loops")).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host.load(&wasm).unwrap();

    // One deadline trap (the infinite loop, bounded live).
    let mut live = loaded
        .spawn(
            &[],
            ExecutionMode::Live {
                deadline: Duration::from_millis(100),
            },
        )
        .unwrap();
    live.handle_bar(sample_bar(1)).unwrap_err();

    let health = loaded.health();
    assert_eq!(health.trap_count, 1);
    assert_eq!(
        health.deadline_exceeded_count, 1,
        "an epoch-deadline trap must be counted as a deadline exceedance"
    );

    // A second plugin whose trap is a plain guest panic, never a deadline.
    let panicking_wasm = std::fs::read(support::build_fixture("panics")).unwrap();
    let panicking = host.load(&panicking_wasm).unwrap();
    let mut instance = panicking
        .spawn(&[], ExecutionMode::Backtest { fuel: 10_000_000 })
        .unwrap();
    instance.handle_bar(sample_bar(1)).unwrap_err();
    let panic_health = panicking.health();
    assert_eq!(panic_health.trap_count, 1);
    assert_eq!(
        panic_health.deadline_exceeded_count, 0,
        "a plain guest panic must not be counted as a deadline exceedance"
    );
}

/// `PluginHealth::peak_memory_bytes` must reflect real granted growth, not
/// stay at zero just because a ceiling exists.
#[test]
fn peak_memory_bytes_reflects_real_growth() {
    let wasm = std::fs::read(support::build_fixture("allocates")).unwrap();
    let host = PluginHost::new(PluginLimits {
        max_memory_bytes: 4 * 1024 * 1024,
    })
    .unwrap();
    let loaded = host.load(&wasm).unwrap();
    let mut instance = loaded
        .spawn(&[], ExecutionMode::Backtest { fuel: u64::MAX })
        .unwrap();

    // Denied by the ceiling, same as `unbounded_allocation_is_denied_by_the_memory_limit_not_a_killed_process`.
    instance.handle_bar(sample_bar(100)).unwrap_err();

    let health = loaded.health();
    assert!(
        health.peak_memory_bytes > 0,
        "growth that succeeded before the ceiling was hit must be recorded, got {}",
        health.peak_memory_bytes
    );
    assert!(
        health.peak_memory_bytes <= 4 * 1024 * 1024,
        "the recorded peak must never exceed what was actually granted"
    );
}

/// The circuit breaker's state must be readable through
/// `LoadedPlugin::health` without the read itself closing an open breaker —
/// the same non-mutation property `crate::circuit`'s own unit test proves
/// one layer down, checked here through the public, real-component path.
#[test]
fn health_reports_the_open_circuit_with_its_reason_and_reading_it_does_not_close_it() {
    let wasm = std::fs::read(support::build_fixture("panics")).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host.load(&wasm).unwrap();
    let mut instance = loaded
        .spawn(&[], ExecutionMode::Backtest { fuel: 10_000_000 })
        .unwrap();

    for _ in 0..3 {
        instance.handle_bar(sample_bar(1)).unwrap_err();
    }

    for _ in 0..2 {
        match loaded.health().circuit {
            CircuitState::Open { reason } => assert!(reason.contains("consecutive")),
            CircuitState::Closed => panic!("a freshly tripped breaker must read as open"),
        }
    }
    // The breaker really is still open to a real call, not just reported so.
    let err = instance.handle_bar(sample_bar(1)).unwrap_err();
    assert!(
        matches!(err, PluginHostError::CircuitOpen(_)),
        "got {err:?}"
    );

    // The line the breaker itself recorded when it tripped is also in the
    // plugin's own log, at warning severity.
    assert!(
        loaded
            .logs()
            .iter()
            .any(|line| line.message.starts_with("circuit open:")
                && line.severity == PluginLogSeverity::Warn)
    );
}

/// A tripped breaker must never clear itself — repeated `spawn`/`health`
/// calls, with no cooldown left to wait out, all still see it open. Only
/// [`LoadedPlugin::reset_circuit_breaker`] (the primitive
/// `senken_runtime::DynamicIndicators::set_enabled` calls on a user's own
/// "re-enable") lets a fresh instance spawn again.
#[test]
fn a_tripped_breaker_stays_open_until_explicitly_reset() {
    let wasm = std::fs::read(support::build_fixture("panics")).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host.load(&wasm).unwrap();
    let mut instance = loaded
        .spawn(&[], ExecutionMode::Backtest { fuel: 10_000_000 })
        .unwrap();
    for _ in 0..3 {
        instance.handle_bar(sample_bar(1)).unwrap_err();
    }

    // No timer to wait out any more: this loop is the whole proof that
    // nothing here closes the breaker on its own.
    for _ in 0..5 {
        let err = loaded
            .spawn(&[], ExecutionMode::Backtest { fuel: 10_000_000 })
            .expect_err("a still-open breaker must keep refusing new instances");
        assert!(matches!(err, PluginHostError::CircuitOpen(_)));
        assert!(matches!(loaded.health().circuit, CircuitState::Open { .. }));
    }

    loaded.reset_circuit_breaker();
    assert_eq!(loaded.health().circuit, CircuitState::Closed);
    loaded
        .spawn(&[], ExecutionMode::Backtest { fuel: 10_000_000 })
        .expect("resetting the breaker must let a fresh instance spawn again");
}
