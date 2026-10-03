//! The line-hold budget — ADR-0030 F2's *"guarantee not gained"*, implemented as the runtime check
//! it is.
//!
//! # Why this exists and why it is not a type
//!
//! ADR-0030 §2.1's row for invariant 4 classifies it *"**unrepresentable** for the reachable path and
//! for nested commit; **guarded** for a captured handle; + **checked** (debug line-hold budget)"*, and
//! F2 says the rest out loud: *"Withholding the effect capability closes the intended path; a closure
//! capturing an `Arc<ModelClient>` or a `tokio::process::Command` from its environment still compiles
//! and still awaits inside the held line. No Rust construct forbids that."*
//!
//! So this module is the residue, and it is **labelled** as a runtime check rather than dressed as a
//! guarantee. [`LineHold`] is an RAII timer around one commit; on drop it compares the elapsed time
//! against [`budget`] and, when the budget is exceeded, records the overrun in a process-global
//! counter that a test asserts against.
//!
//! # Why it records rather than panics
//!
//! Two reasons, and the second is the load-bearing one. A panic on drop runs during unwinding and in
//! a release build `[profile.release]`'s `panic = "abort"` (`Cargo.toml:447`) turns it into process
//! death — so a panic here would convert a *diagnostic about latency* into a durability incident, in
//! the one window where the Session has already committed. And the workspace denies `clippy::panic`
//! outright, which is the same judgement written as a lint.
//!
//! The gate is therefore a test rather than a crash: [`overruns`] is asserted to be zero by the
//! behaviour suite, and a commit that awaits a model call makes it non-zero. ADR-0030 open question 5
//! is what would let the budget be tightened — *"does the harness ever legitimately need a commit
//! callback to await something that is not a storage read?"*
//!
//! # Why it is compiled out of release builds
//!
//! The check is `#[cfg(debug_assertions)]`-gated at the point where it costs something: in a release
//! build [`LineHold::begin`] takes no clock reading and [`LineHold`]'s `Drop` does nothing. A release
//! binary therefore pays nothing for a check whose only consumer is a test, which is what
//! *"debug/test-build line-hold timer"* means.

use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;

/// The default budget: 50 ms.
///
/// Chosen against the thing it is meant to catch rather than against a percentile. The smallest
/// external effect F2 names is *"one 60-second model call"*; the next smallest is a tool subprocess
/// spawn, which is milliseconds at best. A storage commit on the JSONL backend's strong tier costs
/// ~1.5 µs on the caller thread (PICO5-PLAN S7, via `cyrup-session`'s `SESSION_SYNCER`), so 50 ms is
/// four orders of magnitude of headroom over the legitimate work and two below the cheapest illegal
/// work. It is deliberately **loose**: a budget that fires on a slow CI disk gets raised and then
/// catches nothing.
pub const DEFAULT_BUDGET: Duration = Duration::from_millis(50);

/// The budget, in nanoseconds. `0` means [`DEFAULT_BUDGET`].
static BUDGET_NANOS: AtomicU64 = AtomicU64::new(0);

/// How many holds have exceeded the budget in this process.
static OVERRUNS: AtomicU64 = AtomicU64::new(0);

/// The longest hold observed in this process, in nanoseconds.
static LONGEST_NANOS: AtomicU64 = AtomicU64::new(0);

/// The current budget.
#[must_use]
pub fn budget() -> Duration {
    match BUDGET_NANOS.load(Ordering::Relaxed) {
        0 => DEFAULT_BUDGET,
        nanos => Duration::from_nanos(nanos),
    }
}

/// Set the budget for this process.
///
/// For a test that wants the check to fire deterministically, and for a host whose storage is slow
/// enough that the default is noise. Passing a zero duration restores [`DEFAULT_BUDGET`], because a
/// zero budget would make every hold an overrun and the counter would stop meaning anything.
pub fn set_budget(budget: Duration) {
    let nanos = u64::try_from(budget.as_nanos()).unwrap_or(u64::MAX);
    BUDGET_NANOS.store(nanos, Ordering::Relaxed);
}

/// How many commits have held the line past [`budget`] in this process.
///
/// Always `0` in a release build: the check is `#[cfg(debug_assertions)]`.
#[must_use]
pub fn overruns() -> u64 {
    OVERRUNS.load(Ordering::Relaxed)
}

/// The longest line hold observed in this process.
///
/// Always [`Duration::ZERO`] in a release build.
#[must_use]
pub fn longest() -> Duration {
    Duration::from_nanos(LONGEST_NANOS.load(Ordering::Relaxed))
}

/// Reset the counters. For a test that asserts on them.
pub fn reset() {
    OVERRUNS.store(0, Ordering::Relaxed);
    LONGEST_NANOS.store(0, Ordering::Relaxed);
}

/// An RAII hold on the Session mutation line.
///
/// Created when a commit is admitted and dropped when its outcome is decided — so the window it
/// measures is `spec.md:1521-1526`'s exactly: *"callback execution, preparation, storage settlement,
/// committed baseline adoption, and publication enqueue"*.
#[derive(Debug)]
pub struct LineHold {
    #[cfg(debug_assertions)]
    began: std::time::Instant,
}

impl LineHold {
    /// Begin a hold.
    #[must_use]
    pub fn begin() -> Self {
        Self {
            #[cfg(debug_assertions)]
            began: std::time::Instant::now(),
        }
    }
}

#[cfg(debug_assertions)]
impl Drop for LineHold {
    fn drop(&mut self) {
        let held = self.began.elapsed();
        let nanos = u64::try_from(held.as_nanos()).unwrap_or(u64::MAX);
        LONGEST_NANOS.fetch_max(nanos, Ordering::Relaxed);
        if held > budget() {
            OVERRUNS.fetch_add(1, Ordering::Relaxed);
        }
    }
}
