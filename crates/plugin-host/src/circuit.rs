//! A per-plugin circuit breaker: repeated traps disable a plugin instead of
//! calling into it again and again on every bar.
//!
//! Same shape as `senken_venue::LimitGroup`'s own circuit — closed by
//! default, counts consecutive failures, and opens once a threshold is
//! crossed — but this crate does not depend on `senken-venue` for it: that
//! breaker trips on HTTP status codes (`429`/`418`) and shares a module
//! with proactive rate windows and an AIMD concurrency gate, none of which
//! apply to a guest trap. Reusing the type would mean either depending on
//! HTTP semantics this crate has nothing to do with, or splitting
//! `LimitGroup` apart to share only its state machine — a bigger change
//! than this crate's own contract calls for. The state machine itself is
//! copied deliberately, not reinvented.
//!
//! **Unlike `LimitGroup`, this breaker never recovers on its own.** A venue
//! failure is transient — a rate limit or an outage clears with time, so a
//! cooldown-then-retry is the right recovery. A guest trap is not: the bug
//! that caused it is deterministic code shipped inside the component, so a
//! cooldown would only mean the same three traps fire again on the very
//! next call, forever, at whatever interval the cooldown names — "the
//! application is up but unusable" with extra steps. Once open, this
//! breaker stays open until [`PluginCircuit::reset`] is called explicitly —
//! the action behind a user re-enabling a plugin from the Plugins page,
//! never a timer.

use std::sync::Mutex;

/// Consecutive traps, on top of not yet recovering from the last one,
/// before a plugin is treated as broken rather than momentarily unlucky.
/// Our own policy choice: three survives one flaky call without giving a
/// truly broken plugin three free bites at every bar.
const CONSECUTIVE_TRAPS_TO_TRIP: u32 = 3;

enum State {
    Closed { consecutive_traps: u32 },
    Open { reason: String },
}

/// A plugin's circuit-breaker state, read without side effects.
///
/// Deliberately a separate type from the internal `State` this reads: a
/// health check must never mutate the breaker merely by looking at it — see
/// this module's own docs for why an open breaker no longer closes itself
/// at all, on a health read or otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitState {
    /// Calls are allowed through.
    Closed,
    /// Disabled after repeated traps, until a user explicitly re-enables
    /// this plugin (see `PluginCircuit::reset`) — never on its own.
    Open {
        /// Why the breaker tripped — the same reason
        /// `PluginCircuit::ensure_closed` fails calls with while this is
        /// current.
        reason: String,
    },
}

/// Shared, per-plugin trap budget. Cheap to check: [`PluginCircuit::ensure_closed`]
/// takes a lock only long enough to read one enum.
pub(crate) struct PluginCircuit {
    state: Mutex<State>,
}

impl PluginCircuit {
    /// A new, closed breaker.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(State::Closed {
                consecutive_traps: 0,
            }),
        }
    }

    /// Fails fast with the reason the breaker was tripped if it is
    /// currently open; otherwise lets the caller proceed. Read-only: unlike
    /// a rate-limit breaker, this never closes itself as a side effect of
    /// being checked — see this module's own docs for why a guest trap gets
    /// no cooldown at all.
    ///
    /// # Errors
    /// The trip reason, if the breaker is open.
    pub(crate) fn ensure_closed(&self) -> Result<(), String> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*state {
            State::Closed { .. } => Ok(()),
            State::Open { reason } => Err(reason.clone()),
        }
    }

    /// A call into the plugin trapped. Returns the trip reason if this
    /// trap is what opened the breaker.
    pub(crate) fn record_trap(&self, message: &str) -> Option<String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &mut *state {
            State::Closed { consecutive_traps } => {
                *consecutive_traps += 1;
                if *consecutive_traps >= CONSECUTIVE_TRAPS_TO_TRIP {
                    let reason = format!(
                        "{CONSECUTIVE_TRAPS_TO_TRIP} consecutive traps, most recently: {message}"
                    );
                    *state = State::Open {
                        reason: reason.clone(),
                    };
                    Some(reason)
                } else {
                    None
                }
            }
            State::Open { reason } => Some(reason.clone()),
        }
    }

    /// This breaker's current state, without any mutating side effect.
    #[must_use]
    pub(crate) fn status(&self) -> CircuitState {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*state {
            State::Closed { .. } => CircuitState::Closed,
            State::Open { reason } => CircuitState::Open {
                reason: reason.clone(),
            },
        }
    }

    /// A call into the plugin succeeded: resets the consecutive-trap count.
    pub(crate) fn record_success(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let State::Closed { consecutive_traps } = &mut *state {
            *consecutive_traps = 0;
        }
    }

    /// Explicitly closes an open breaker, clearing the trap streak — the
    /// only way this breaker ever recovers (see this module's own docs).
    /// The caller (`senken_runtime::DynamicIndicators::set_enabled`, wired
    /// through `LoadedPlugin`/`LoadedCompiledIndicator`) is a user
    /// deliberately re-enabling a plugin they have already read the trip
    /// reason for; nothing in this crate calls this on its own.
    pub(crate) fn reset(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *state = State::Closed {
            consecutive_traps: 0,
        };
    }
}

impl Default for PluginCircuit {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for PluginCircuit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginCircuit").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{CircuitState, PluginCircuit};

    #[test]
    fn a_closed_breaker_never_blocks() {
        let circuit = PluginCircuit::new();
        for _ in 0..10 {
            circuit.ensure_closed().unwrap();
        }
    }

    #[test]
    fn status_reports_open_with_the_trip_reason_and_never_mutates() {
        let circuit = PluginCircuit::new();
        circuit.record_trap("boom");
        circuit.record_trap("boom");
        circuit.record_trap("boom");

        // Reading status twice in a row must report the same thing both
        // times — proving the read itself has no side effect.
        for _ in 0..2 {
            match circuit.status() {
                CircuitState::Open { reason } => assert!(reason.contains("boom")),
                CircuitState::Closed => panic!("a freshly tripped breaker must read as open"),
            }
        }
        // The breaker is still genuinely open to a real call too.
        assert!(circuit.ensure_closed().is_err());
    }

    #[test]
    fn status_reports_closed_before_any_trap() {
        let circuit = PluginCircuit::new();
        assert_eq!(circuit.status(), CircuitState::Closed);
    }

    #[test]
    fn repeated_traps_open_the_breaker() {
        let circuit = PluginCircuit::new();
        assert!(circuit.record_trap("boom").is_none());
        assert!(circuit.record_trap("boom").is_none());
        let reason = circuit
            .record_trap("boom")
            .expect("the third consecutive trap must open the breaker");
        assert!(reason.contains("boom"));
        let err = circuit.ensure_closed().unwrap_err();
        assert!(err.contains("boom"));
    }

    #[test]
    fn a_success_resets_the_trap_streak() {
        let circuit = PluginCircuit::new();
        assert!(circuit.record_trap("a").is_none());
        assert!(circuit.record_trap("a").is_none());
        circuit.record_success();
        // The streak was reset, so two more traps alone must not trip it.
        assert!(circuit.record_trap("a").is_none());
        assert!(circuit.record_trap("a").is_none());
        circuit.ensure_closed().unwrap();
    }

    #[test]
    fn once_open_every_call_fails_fast_with_the_readable_reason() {
        let circuit = PluginCircuit::new();
        circuit.record_trap("a");
        circuit.record_trap("a");
        circuit.record_trap("a");
        let first = circuit.ensure_closed().unwrap_err();
        let second = circuit.ensure_closed().unwrap_err();
        assert_eq!(first, second, "the reason must stay readable and stable");
    }

    /// The property this revision exists to prove: a deterministic bug
    /// traps the same way on every retry, so a breaker that recovered on a
    /// timer would simply re-trip on its very next probe, forever. This
    /// checks the breaker stays open across many repeated reads and calls
    /// with no elapsed-time mechanism to wait out — there is none any more.
    #[test]
    fn an_open_breaker_never_recovers_on_its_own_no_matter_how_many_times_it_is_checked() {
        let circuit = PluginCircuit::new();
        circuit.record_trap("boom");
        circuit.record_trap("boom");
        circuit.record_trap("boom");

        for _ in 0..1000 {
            assert!(circuit.ensure_closed().is_err());
            assert!(matches!(circuit.status(), CircuitState::Open { .. }));
        }
    }

    /// [`PluginCircuit::reset`] is the *only* way out of an open breaker —
    /// the explicit "user re-enabled it" action, never a timer.
    #[test]
    fn reset_closes_an_open_breaker_and_clears_the_trap_streak() {
        let circuit = PluginCircuit::new();
        circuit.record_trap("boom");
        circuit.record_trap("boom");
        circuit.record_trap("boom");
        assert!(circuit.ensure_closed().is_err());

        circuit.reset();
        assert_eq!(circuit.status(), CircuitState::Closed);
        circuit.ensure_closed().unwrap();

        // The trap streak was cleared too, not merely the `Open` marker:
        // two more traps alone must not immediately re-trip it.
        assert!(circuit.record_trap("a").is_none());
        assert!(circuit.record_trap("a").is_none());
        circuit.ensure_closed().unwrap();
    }
}
