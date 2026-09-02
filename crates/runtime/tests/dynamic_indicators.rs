//! Proves `senken_runtime::DynamicIndicators` against real compiled `.wasm`
//! components, not a description of what one would do: an uploaded
//! indicator's own output must match the equivalent built-in bar-for-bar,
//! break the same way a built-in does when fed bars out of order, respect
//! the display-object cap with a message rather than silently, and follow
//! its own enabled/disabled flag.

mod support;

use senken_core::UnixNanos;
use senken_indicators::{DisplayList, Drawable, Ema, Indicator, MovingAverage};
use senken_plugin_host::CircuitState;
use senken_runtime::plugin_host::{DynamicIndicatorState, PluginOrigin};
use senken_runtime::{DynamicIndicatorError, DynamicIndicators, reject_if_over_display_cap};
use senken_series::{Bar, BarSpec, BarUnit, Volume};

fn bar(ts: i64, open: i64, high: i64, low: i64, close: i64, volume: i64) -> Bar {
    Bar {
        ts_open: UnixNanos::from_nanos(ts),
        open,
        high,
        low,
        close,
        volume: Volume::Real(volume),
        quote_volume: None,
        trade_count: None,
        taker_buy_volume: None,
    }
}

/// Varies every OHLCV field on every bar, so an order-sensitive indicator
/// (this fixture's EMA among them) computes a different final reading when
/// fed in reverse — the same fixture shape
/// `crates/indicators/src/indicator.rs`'s own reversed-order test uses.
fn bars() -> Vec<Bar> {
    vec![
        bar(0, 100, 105, 95, 100, 10),
        bar(1, 101, 110, 98, 108, 20),
        bar(2, 108, 112, 100, 95, 5),
        bar(3, 95, 100, 90, 98, 15),
        bar(4, 98, 120, 96, 115, 30),
        bar(5, 115, 118, 108, 110, 8),
        bar(6, 110, 116, 104, 112, 12),
        bar(7, 112, 130, 111, 128, 40),
    ]
}

fn one_minute() -> BarSpec {
    BarSpec::new(1, BarUnit::Minute)
}

#[test]
fn a_dynamic_ema_matches_the_native_ema_bar_for_bar() {
    let period = 3_u64;
    let catalog = DynamicIndicators::new().unwrap();
    let wasm = std::fs::read(support::build_fixture("dyn-ema")).unwrap();
    let info = catalog.register(&wasm).unwrap();
    assert_eq!(info.id, "DynEma");

    let mut dynamic = catalog
        .spawn("DynEma", &format!(r#"{{"period":{period}}}"#))
        .unwrap();
    let mut native = Ema::new(usize::try_from(period).unwrap());

    for bar in bars() {
        native.handle_bar(&bar);
        let on_bar = dynamic.handle_bar(&bar, one_minute()).unwrap();
        let dynamic_initialized = dynamic.initialized().unwrap();

        assert_eq!(
            dynamic_initialized,
            native.initialized(),
            "the fixture tracks its own warm-up the same way `Ema` does"
        );
        if native.initialized() {
            let value = on_bar
                .plots
                .iter()
                .find(|(field, _)| field == "value")
                .map(|(_, value)| *value)
                .expect("the fixture always reports a `value` field");
            assert!(
                (value - native.value()).abs() < 1e-9,
                "dynamic EMA {value} must match native EMA {} \
                 bar-for-bar — both call the exact same compiled `Ema`",
                native.value()
            );
        }
    }
}

#[test]
fn feeding_the_dynamic_ema_bars_in_reverse_changes_its_final_value() {
    let catalog = DynamicIndicators::new().unwrap();
    let wasm = std::fs::read(support::build_fixture("dyn-ema")).unwrap();
    catalog.register(&wasm).unwrap();

    let run = |order: Vec<Bar>| {
        let mut instance = catalog.spawn("DynEma", r#"{"period":3}"#).unwrap();
        let mut last = 0.0;
        for bar in order {
            let on_bar = instance.handle_bar(&bar, one_minute()).unwrap();
            last = on_bar
                .plots
                .into_iter()
                .find(|(field, _)| field == "value")
                .map(|(_, value)| value)
                .unwrap();
        }
        last
    };

    let forward = run(bars());
    let reversed = run(bars().into_iter().rev().collect());
    assert!(
        (forward - reversed).abs() > 1e-6,
        "an incremental EMA is order-sensitive: forward {forward} and \
         reversed {reversed} must disagree, or the bridge is silently \
         batching instead of streaming bars one at a time"
    );
}

#[test]
fn disabling_removes_the_entry_from_the_catalog_and_enabling_restores_it() {
    let catalog = DynamicIndicators::new().unwrap();
    let wasm = std::fs::read(support::build_fixture("dyn-ema")).unwrap();
    let info = catalog.register(&wasm).unwrap();

    assert!(catalog.catalog().iter().any(|entry| entry.id == info.id));

    catalog.set_enabled(&info.id, false).unwrap();
    assert!(
        !catalog.catalog().iter().any(|entry| entry.id == info.id),
        "a disabled plugin must disappear from the catalog"
    );
    // The registration itself — including its declared parameters — is
    // kept, not discarded, so re-enabling needs no re-upload.
    let still_registered = catalog
        .info(&info.id)
        .expect("registration survives disable");
    assert_eq!(still_registered, info);

    catalog.set_enabled(&info.id, true).unwrap();
    let restored = catalog
        .catalog()
        .into_iter()
        .find(|entry| entry.id == info.id)
        .expect("enabling again must restore the entry, with its params intact");
    assert_eq!(restored, info);
}

#[test]
fn an_id_colliding_with_a_builtin_is_rejected_at_registration() {
    let catalog = DynamicIndicators::new().unwrap();
    let wasm = std::fs::read(support::build_fixture("dyn-collide")).unwrap();

    let err = catalog.register(&wasm).unwrap_err();
    assert!(matches!(
        err,
        DynamicIndicatorError::CollidesWithBuiltin(id) if id == "Sma"
    ));
    assert!(
        catalog.info("Sma").is_none(),
        "a rejected registration must not leave a partial entry behind"
    );
}

#[test]
fn an_unknown_or_disabled_plugin_is_refused_with_a_clear_reason() {
    let catalog = DynamicIndicators::new().unwrap();
    let err = catalog.spawn("NoSuchPlugin", "{}").unwrap_err();
    assert!(matches!(err, DynamicIndicatorError::UnknownPlugin(id) if id == "NoSuchPlugin"));

    let wasm = std::fs::read(support::build_fixture("dyn-ema")).unwrap();
    let info = catalog.register(&wasm).unwrap();
    catalog.set_enabled(&info.id, false).unwrap();
    let err = catalog.spawn(&info.id, r#"{"period":3}"#).unwrap_err();
    assert!(matches!(err, DynamicIndicatorError::Disabled(id) if id == info.id));
}

/// The display-object cap: proves the mechanism `crates/api/src/
/// indicator_handlers.rs::compute_dynamic_indicator` relies on actually
/// bites against a real, over-producing plugin, and rejects with a
/// message rather than truncating.
#[test]
fn a_plugin_that_emits_too_many_display_objects_is_rejected_with_a_message() {
    let catalog = DynamicIndicators::new().unwrap();
    let wasm = std::fs::read(support::build_fixture("dyn-overload")).unwrap();
    catalog.register(&wasm).unwrap();

    let mut instance = catalog.spawn("DynOverload", "{}").unwrap();
    let mut display = DisplayList::new(senken_runtime::DYNAMIC_INDICATOR_MAX_DISPLAY_OBJECTS);
    for bar in bars() {
        // 50 new `Level`s per bar (see the fixture) against 8 bars is 400
        // — already past the 500 cap once a couple more runs accumulate,
        // so two short ranges make the point without an enormous fixture.
        let on_bar = instance.handle_bar(&bar, one_minute()).unwrap();
        for drawable in on_bar.drawables {
            display.push(drawable);
        }
    }
    for bar in bars() {
        let on_bar = instance.handle_bar(&bar, one_minute()).unwrap();
        for drawable in on_bar.drawables {
            display.push(drawable);
        }
    }

    assert!(
        display.discarded_objects() > 0,
        "16 bars * 50 levels = 800 objects must exceed the {} cap",
        senken_runtime::DYNAMIC_INDICATOR_MAX_DISPLAY_OBJECTS
    );
    let err = reject_if_over_display_cap(
        "DynOverload",
        display.drawables().count(),
        display.discarded_objects(),
    )
    .unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("DynOverload") && message.contains("display objects"),
        "the rejection must name the offending indicator and explain \
         why, not just fail silently: got {message:?}"
    );

    // Sanity: a display list that never exceeds the cap must not be
    // rejected at all — proves the guard is not vacuously tripping on
    // every non-empty display.
    let small = DisplayList::new(senken_runtime::DYNAMIC_INDICATOR_MAX_DISPLAY_OBJECTS);
    assert!(
        reject_if_over_display_cap(
            "DynOverload",
            small.drawables().count(),
            small.discarded_objects()
        )
        .is_ok()
    );
}

/// One `Drawable::Level` really did cross the boundary intact — proves
/// `senken_runtime::plugin_host`'s drawable bridge, not just the plot
/// values `a_dynamic_ema_matches_the_native_ema_bar_for_bar` already
/// covers.
#[test]
fn the_overload_fixtures_drawables_survive_the_bridge_as_levels() {
    let catalog = DynamicIndicators::new().unwrap();
    let wasm = std::fs::read(support::build_fixture("dyn-overload")).unwrap();
    catalog.register(&wasm).unwrap();
    let mut instance = catalog.spawn("DynOverload", "{}").unwrap();

    let on_bar = instance.handle_bar(&bars()[0], one_minute()).unwrap();
    assert_eq!(on_bar.drawables.len(), 50);
    assert!(
        on_bar
            .drawables
            .iter()
            .all(|drawable| matches!(drawable, Drawable::Level { .. }))
    );
}

/// A component that loads under neither world must not simply vanish: it
/// has to stay visible on `all()`, with the combined reason both attempts
/// gave, so an uploaded file that failed does not disappear without
/// explanation. Re-uploading the exact same broken bytes replaces the
/// earlier failed entry rather than piling up duplicates — the same
/// contract a successful registration already has.
#[test]
fn a_registration_that_fails_to_load_is_kept_visible_with_its_reason() {
    let catalog = DynamicIndicators::new().unwrap();
    let garbage = b"not a real component";

    let err = catalog.register(garbage).unwrap_err();
    assert!(matches!(err, DynamicIndicatorError::Host(_)), "got {err:?}");

    let statuses = catalog.all();
    assert_eq!(
        statuses.len(),
        1,
        "the failed registration must be recorded, not discarded"
    );
    let status = &statuses[0];
    assert_eq!(
        status.info, None,
        "a component that never loaded has no descriptor"
    );
    assert_eq!(status.origin, PluginOrigin::Uploaded);
    assert!(status.health.is_none());
    assert!(status.logs.is_empty());
    match &status.state {
        DynamicIndicatorState::FailedToLoad { reason } => {
            assert!(
                reason.contains("indicator-plugin") && reason.contains("compiled-indicator"),
                "the reason must name both worlds that were tried: {reason:?}"
            );
        }
        other => panic!("expected FailedToLoad, got {other:?}"),
    }

    // Re-registering the exact same broken bytes must replace the earlier
    // failed entry, not add a second row.
    catalog.register(garbage).unwrap_err();
    assert_eq!(
        catalog.all().len(),
        1,
        "re-uploading the same broken bytes must not pile up duplicate entries"
    );
}

/// A component naming a `senken:plugin-api` version this host does not
/// support must be reported as `Incompatible`, distinctly from an ordinary
/// `FailedToLoad` — the two need different fixes from a plugin author, and
/// conflating them was exactly what this slice exists to stop.
#[test]
fn a_component_naming_an_unsupported_api_version_is_recorded_as_incompatible() {
    let catalog = DynamicIndicators::new().unwrap();
    // A hand-written component whose only import names a real WIT
    // interface at a version no build of this host ever supports — see
    // `crates/plugin-host/src/host.rs`'s own unit tests for why the
    // component's *shape* beyond that name does not matter here.
    let wasm = br#"(component
        (import "senken:plugin-api/types@9.9.9" (instance))
    )"#;

    let err = catalog.register(wasm).unwrap_err();
    let (found, supported) = match err {
        DynamicIndicatorError::Host(senken_plugin_host::PluginHostError::Incompatible {
            found,
            supported,
        }) => (found, supported),
        other => panic!("expected Host(Incompatible), got {other:?}"),
    };
    assert_eq!(found, "9.9.9");

    let statuses = catalog.all();
    assert_eq!(statuses.len(), 1);
    match &statuses[0].state {
        DynamicIndicatorState::Incompatible {
            found_version,
            supported_version,
        } => {
            assert_eq!(found_version, "9.9.9");
            assert_eq!(*supported_version, supported);
        }
        other => panic!("expected Incompatible, got {other:?}"),
    }
}

/// `register_with_origin` must record the origin it was given, and
/// `all()` must report it back unchanged — the primitive an integrator
/// wires a built-in or a data-directory scan through, distinct from the
/// plain `register` every upload path already uses.
#[test]
fn register_with_origin_is_reported_back_on_the_status() {
    let catalog = DynamicIndicators::new().unwrap();
    let wasm = std::fs::read(support::build_fixture("dyn-ema")).unwrap();
    let info = catalog
        .register_with_origin(&wasm, PluginOrigin::DataDirectory)
        .unwrap();

    let status = catalog
        .all()
        .into_iter()
        .find(|status| status.id == info.id)
        .unwrap();
    assert_eq!(status.origin, PluginOrigin::DataDirectory);
    assert_eq!(status.state, DynamicIndicatorState::Active);
}

/// A plugin whose circuit breaker trips from repeated traps must be
/// reported as `AutoDisabled` with the breaker's own reason — a state a
/// user never chose, distinct from `Disabled`, and reversible only by the
/// user re-enabling it (see this crate's own design record). Also proves
/// `all()`'s `logs` and `health` fields against a real trap, not only the
/// happy path the other tests here already cover.
#[test]
fn a_plugin_whose_breaker_trips_is_reported_auto_disabled_with_the_reason() {
    let catalog = DynamicIndicators::new().unwrap();
    let wasm = std::fs::read(support::build_fixture("dyn-panics")).unwrap();
    let info = catalog.register(&wasm).unwrap();

    let status_before = catalog
        .all()
        .into_iter()
        .find(|status| status.id == info.id)
        .unwrap();
    assert_eq!(status_before.state, DynamicIndicatorState::Active);

    // Three consecutive traps trip the shared, per-plugin breaker — same
    // threshold `crates/plugin-host/src/circuit.rs` documents. All three go
    // through the *same* instance: a fresh `spawn` on each iteration would
    // itself be a successful constructor call, which resets the breaker's
    // consecutive-trap count before the next trap could accumulate against
    // it (`crate::circuit::PluginCircuit::record_success`'s own contract).
    let mut instance = catalog.spawn(&info.id, "{}").unwrap();
    for _ in 0..3 {
        instance.handle_bar(&bars()[0], one_minute()).unwrap_err();
    }

    let status_after = catalog
        .all()
        .into_iter()
        .find(|status| status.id == info.id)
        .unwrap();
    match &status_after.state {
        DynamicIndicatorState::AutoDisabled { reason } => {
            assert!(reason.contains("consecutive"), "got {reason:?}");
        }
        other => panic!("expected AutoDisabled, got {other:?}"),
    }
    let health = status_after
        .health
        .expect("a plugin that has run at least once has runtime health");
    assert_eq!(health.trap_count, 3);
    assert!(matches!(health.circuit, CircuitState::Open { .. }));
    assert!(
        status_after
            .logs
            .iter()
            .any(|line| line.message.starts_with("trap:")),
        "the plugin's own trap lines must be visible through `all()`"
    );

    // A tripped breaker is not the same thing as a user's own disable —
    // `catalog()` (the chart-addable list) still reflects the user's
    // `enabled` flag, which nothing here has touched.
    assert!(catalog.catalog().iter().any(|entry| entry.id == info.id));
}

/// The breaker must never clear itself — the property `crates/plugin-host/
/// src/circuit.rs` documents as the whole point of this revision: a
/// deterministic bug traps the same way on every retry, so a cooldown would
/// just mean the same three traps fire again on the very next call. Proves
/// it by calling `spawn` (a fresh instance, which is itself a successful
/// constructor call were the breaker closed) many times over — with no
/// elapsed-time mechanism left to wait out, "many times over" is the whole
/// proof — and only `set_enabled(id, true)` actually clears it.
#[test]
fn an_auto_disabled_plugin_stays_auto_disabled_until_explicitly_re_enabled() {
    let catalog = DynamicIndicators::new().unwrap();
    let wasm = std::fs::read(support::build_fixture("dyn-panics")).unwrap();
    let info = catalog.register(&wasm).unwrap();

    let mut instance = catalog.spawn(&info.id, "{}").unwrap();
    for _ in 0..3 {
        instance.handle_bar(&bars()[0], one_minute()).unwrap_err();
    }
    let is_auto_disabled = |catalog: &DynamicIndicators| {
        matches!(
            catalog
                .all()
                .into_iter()
                .find(|status| status.id == info.id)
                .unwrap()
                .state,
            DynamicIndicatorState::AutoDisabled { .. }
        )
    };
    assert!(is_auto_disabled(&catalog));

    // No timer to wait out any more — repeatedly failing to spawn a fresh
    // instance is itself the proof that nothing closes this on its own.
    for _ in 0..5 {
        assert!(catalog.spawn(&info.id, "{}").is_err());
        assert!(is_auto_disabled(&catalog));
    }

    // The explicit remedy: a user re-enabling the plugin (the same call
    // `POST /api/indicators/plugins/{name}/enabled` makes with
    // `enabled: true`) resets the breaker and lets a fresh instance spawn
    // and run again.
    catalog.set_enabled(&info.id, true).unwrap();
    assert!(!is_auto_disabled(&catalog));
    let status = catalog
        .all()
        .into_iter()
        .find(|status| status.id == info.id)
        .unwrap();
    assert_eq!(status.state, DynamicIndicatorState::Active);

    let mut instance = catalog.spawn(&info.id, "{}").unwrap();
    // `dyn-panics` traps on every `on-bar` call regardless — the point here
    // is only that the breaker let a fresh call through at all, which it
    // would refuse outright while still open.
    let _ = instance.handle_bar(&bars()[0], one_minute());
}
