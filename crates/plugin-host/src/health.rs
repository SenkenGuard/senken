//! Runtime health counters shared across every instance spawned from one
//! [`crate::host::LoadedPlugin`] or [`crate::host::LoadedCompiledIndicator`]
//! — total traps, how many of those specifically exceeded the wall-clock
//! deadline, and the highest linear-memory size any instance was ever
//! granted.
//!
//! Shared the same way the circuit breaker and the ring log already are:
//! one instance per plugin, cloned (an `Arc`) into every `Store` spawned
//! from it, so a value read here reflects the plugin's whole history, not
//! one ephemeral instance's — a compute call spawns and drops an instance
//! per request, so a counter that lived only on the instance would never
//! accumulate into anything a person could read.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::circuit::CircuitState;

/// A snapshot of one plugin's runtime health, for the management surface
/// `senken_runtime::DynamicIndicators` builds on top of this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginHealth {
    /// Every call that returned a trap, for the lifetime of the plugin —
    /// never reset, unlike the circuit breaker's own *consecutive* count.
    pub trap_count: u64,
    /// The subset of `trap_count` caused specifically by exceeding a
    /// [`crate::ExecutionMode::Live`] wall-clock deadline, as opposed to a
    /// guest panic, a fuel exhaustion, or a denied memory growth.
    pub deadline_exceeded_count: u64,
    /// The highest linear-memory size, in bytes, any single instance of
    /// this plugin was ever granted.
    pub peak_memory_bytes: usize,
    /// Whether the shared circuit breaker currently allows calls through,
    /// and why not if it does not.
    pub circuit: CircuitState,
}

/// Shared, per-plugin counters. See this module's own doc comment for why
/// this outlives any one instance.
pub(crate) struct RuntimeHealth {
    trap_count: AtomicU64,
    deadline_exceeded_count: AtomicU64,
    peak_memory_bytes: AtomicUsize,
}

impl RuntimeHealth {
    pub(crate) fn new() -> Self {
        Self {
            trap_count: AtomicU64::new(0),
            deadline_exceeded_count: AtomicU64::new(0),
            peak_memory_bytes: AtomicUsize::new(0),
        }
    }

    /// Records one trapped call. `deadline_exceeded` narrows it to the
    /// wall-clock-deadline case specifically — see [`PluginHealth::deadline_exceeded_count`].
    pub(crate) fn record_trap(&self, deadline_exceeded: bool) {
        self.trap_count.fetch_add(1, Ordering::Relaxed);
        if deadline_exceeded {
            self.deadline_exceeded_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Records that an instance's linear memory was just grown to
    /// `granted_bytes`, updating the running peak if this is the highest
    /// seen yet.
    pub(crate) fn record_memory_granted(&self, granted_bytes: usize) {
        self.peak_memory_bytes
            .fetch_max(granted_bytes, Ordering::Relaxed);
    }

    /// A snapshot combining these counters with `circuit`'s own current
    /// state — the caller's job, since only the caller (one level up, in
    /// `host.rs`) holds the `PluginCircuit` this health is paired with.
    pub(crate) fn snapshot(&self, circuit: CircuitState) -> PluginHealth {
        PluginHealth {
            trap_count: self.trap_count.load(Ordering::Relaxed),
            deadline_exceeded_count: self.deadline_exceeded_count.load(Ordering::Relaxed),
            peak_memory_bytes: self.peak_memory_bytes.load(Ordering::Relaxed),
            circuit,
        }
    }
}

/// The linear-memory ceiling for one `Store`, wrapping `wasmtime`'s own
/// [`wasmtime::StoreLimits`] for the enforcement `PluginLimits::max_memory_bytes`
/// already relied on, and additionally recording every granted growth into
/// a shared [`RuntimeHealth`] so [`PluginHealth::peak_memory_bytes`] reflects
/// real usage rather than only the ceiling nothing ever measured against.
pub(crate) struct MemoryLimiter {
    inner: wasmtime::StoreLimits,
    health: Arc<RuntimeHealth>,
}

impl MemoryLimiter {
    pub(crate) fn new(max_memory_bytes: usize, health: Arc<RuntimeHealth>) -> Self {
        Self {
            inner: wasmtime::StoreLimitsBuilder::new()
                .memory_size(max_memory_bytes)
                .build(),
            health,
        }
    }
}

impl wasmtime::ResourceLimiter for MemoryLimiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let allowed = self.inner.memory_growing(current, desired, maximum)?;
        if allowed {
            // Only a granted size counts as "used" — a denied attempt never
            // actually became linear memory the plugin could touch.
            self.health.record_memory_granted(desired);
        }
        Ok(allowed)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        self.inner.table_growing(current, desired, maximum)
    }
}

#[cfg(test)]
mod tests {
    use super::RuntimeHealth;
    use crate::circuit::CircuitState;

    #[test]
    fn trap_count_accumulates_and_deadline_count_only_counts_that_cause() {
        let health = RuntimeHealth::new();
        health.record_trap(false);
        health.record_trap(true);
        health.record_trap(true);

        let snapshot = health.snapshot(CircuitState::Closed);
        assert_eq!(snapshot.trap_count, 3);
        assert_eq!(snapshot.deadline_exceeded_count, 2);
    }

    #[test]
    fn peak_memory_tracks_the_highest_grant_not_the_latest() {
        let health = RuntimeHealth::new();
        health.record_memory_granted(1024);
        health.record_memory_granted(4096);
        health.record_memory_granted(2048);

        let snapshot = health.snapshot(CircuitState::Closed);
        assert_eq!(
            snapshot.peak_memory_bytes, 4096,
            "a later, smaller grant must not lower the recorded peak"
        );
    }
}
