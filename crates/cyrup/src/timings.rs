//! Startup timing instrumentation (Pi `core/timings.ts`). A faithful port of `resetTimings`/`time`/
//! `printTimings`: when enabled by `CYRUP_TIMING=1` (or Pi's `PI_TIMING=1`), each [`time`] call
//! records the elapsed milliseconds since the previous mark IN ITS NAMESPACE, and [`print_timings`]
//! writes one titled group per namespace to **stderr** (never stdout — the protocol stream stays
//! clean).
//!
//! # Why this is process-global state (AGENT-027)
//!
//! Pi's namespaces live in a module-level `const timingNamespaces = new Map<TimingLabel,
//! TimingNamespace>()` (`timings.ts:14`), which is what lets two unrelated modules mark into the
//! same table: `main.ts` fills `"main"` while `core/resource-loader.ts:388` resets `"extensions"`
//! and `core/extensions/loader.ts:501/:509/:532` fills it per extension, with no handle passed
//! between them. cyrup's earlier port was a single flat struct whose `print` hardcoded
//! `"Startup Timings: main"`, so a second namespace was unexpressible and the extension-loading
//! phase — the most common cause of a slow start — was invisible.
//!
//! Separately, `PI_STARTUP_BENCHMARK`/`CYRUP_STARTUP_BENCHMARK` (Pi main.ts:800) requests the
//! interactive-init benchmark; the bin gates it to interactive mode via [`startup_benchmark_enabled`]
//! and reports the same "only supports interactive mode" error in the one-shot modes.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Whether startup timings are enabled (`CYRUP_TIMING=1` / `PI_TIMING=1`).
///
/// Pi reads its `ENABLED` once at module load (`timings.ts:6`), so the answer cannot change
/// mid-process; the `OnceLock` reproduces that (and keeps `time` off the env-var path per mark).
fn timing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(std::env::var("CYRUP_TIMING").ok().as_deref(), Some("1"))
            || matches!(std::env::var("PI_TIMING").ok().as_deref(), Some("1"))
    })
}

/// Whether the interactive startup benchmark is requested (`CYRUP_STARTUP_BENCHMARK` / Pi
/// `PI_STARTUP_BENCHMARK`, truthy `1`/`true`/`yes`).
pub fn startup_benchmark_enabled() -> bool {
    fn truthy(v: Option<String>) -> bool {
        matches!(
            v.as_deref().map(str::to_ascii_lowercase).as_deref(),
            Some("1" | "true" | "yes")
        )
    }
    truthy(std::env::var("CYRUP_STARTUP_BENCHMARK").ok())
        || truthy(std::env::var("PI_STARTUP_BENCHMARK").ok())
}

/// Which table a mark lands in (Pi `type TimingLabel = "main" | "extensions"`, timings.ts:12).
/// A closed set upstream, so a closed enum here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingLabel {
    /// The binary's own startup phases (Pi `main.ts`).
    Main,
    /// One entry per extension module import / factory call (Pi `extensions/loader.ts:501-532`).
    Extensions,
}

impl TimingLabel {
    /// The title suffix Pi prints: `Startup Timings: ${namespace}` (timings.ts:47).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Extensions => "extensions",
        }
    }
}

/// One namespace's marks plus the instant the last one was taken (Pi `interface TimingNamespace`,
/// timings.ts:7-10).
#[derive(Debug)]
struct TimingNamespace {
    timings: Vec<(String, u128)>,
    last: Instant,
}

/// Pi's module-level `Map`, in INSERTION ORDER — `printTimings` iterates the Map directly
/// (`timings.ts:46`), and a JS `Map` yields its keys in insertion order, so `main` prints before
/// `extensions` because `main` is reset first.
fn namespaces() -> &'static Mutex<Vec<(TimingLabel, TimingNamespace)>> {
    static NS: OnceLock<Mutex<Vec<(TimingLabel, TimingNamespace)>>> = OnceLock::new();
    NS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Start (or restart) a namespace's table (Pi `resetTimings`, timings.ts:16-19). Inert unless
/// enabled. Pi's default argument is `"main"`; cyrup makes the namespace explicit at every call.
pub fn reset_timings(namespace: TimingLabel) {
    if !timing_enabled() {
        return;
    }
    let Ok(mut ns) = namespaces().lock() else {
        return;
    };
    let fresh = TimingNamespace { timings: Vec::new(), last: Instant::now() };
    match ns.iter_mut().find(|(k, _)| *k == namespace) {
        // `Map.set` on an existing key REPLACES the value and keeps the key's original position.
        Some((_, slot)) => *slot = fresh,
        None => ns.push((namespace, fresh)),
    }
}

/// Record the interval since the previous mark in `namespace` under `label` (Pi `time`,
/// timings.ts:21-32), auto-resetting the namespace on first use exactly as Pi does at `:26-28`.
pub fn time(label: &str, namespace: TimingLabel) {
    if !timing_enabled() {
        return;
    }
    let now = Instant::now();
    let Ok(mut ns) = namespaces().lock() else {
        return;
    };
    if !ns.iter().any(|(k, _)| *k == namespace) {
        ns.push((namespace, TimingNamespace { timings: Vec::new(), last: now }));
    }
    if let Some((_, slot)) = ns.iter_mut().find(|(k, _)| *k == namespace) {
        slot.timings.push((label.to_string(), now.duration_since(slot.last).as_millis()));
        slot.last = now;
    }
}

/// Print one titled group per namespace to stderr (Pi `printTimings`, timings.ts:45-49). Inert
/// unless enabled.
///
/// Pi's `printTimingGroup` filters `timing.ms >= 0` (`:34`) because its clock is `Date.now()`, which
/// an NTP step can move backwards. cyrup measures with [`Instant`], which is monotonic, so the
/// filter is vacuous here and is deliberately not reproduced — every recorded mark is printable.
pub fn print_timings() {
    if !timing_enabled() {
        return;
    }
    let Ok(ns) = namespaces().lock() else {
        return;
    };
    for (label, group) in ns.iter() {
        if group.timings.is_empty() {
            continue;
        }
        let title = format!("Startup Timings: {}", label.as_str());
        eprintln!("\n--- {title} ---");
        let mut total = 0u128;
        for (l, ms) in &group.timings {
            eprintln!("  {l}: {ms}ms");
            total += *ms;
        }
        eprintln!("  TOTAL: {total}ms");
        eprintln!("{}\n", "-".repeat(title.len() + 8));
    }
}

/// Snapshot of a namespace's labels, in order — for tests and for anything that wants to assert the
/// startup phase set without parsing stderr.
pub fn recorded_labels(namespace: TimingLabel) -> Vec<String> {
    namespaces()
        .lock()
        .ok()
        .map(|ns| {
            ns.iter()
                .find(|(k, _)| *k == namespace)
                .map(|(_, g)| g.timings.iter().map(|(l, _)| l.clone()).collect())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// The namespace map is process-global (as pi's is), so these exercise the table directly rather
    /// than through the env-gated public entry points, which would be order-dependent across tests.
    fn push(namespace: TimingLabel, label: &str) {
        let mut ns = namespaces().lock().unwrap();
        let now = Instant::now();
        if !ns.iter().any(|(k, _)| *k == namespace) {
            ns.push((namespace, TimingNamespace { timings: Vec::new(), last: now }));
        }
        let slot = &mut ns.iter_mut().find(|(k, _)| *k == namespace).unwrap().1;
        slot.timings.push((label.to_string(), now.duration_since(slot.last).as_millis()));
        slot.last = now;
    }

    /// AGENT-027 — two namespaces coexist and keep their own tables. The old flat `Timings` struct
    /// could not express this at all: a second instance printed under the hardcoded title "main".
    #[test]
    fn namespaces_are_independent_and_titled_separately() {
        push(TimingLabel::Main, "parseArgs");
        push(TimingLabel::Extensions, "/ext/a module import");
        push(TimingLabel::Extensions, "/ext/a factory");

        let main = recorded_labels(TimingLabel::Main);
        let ext = recorded_labels(TimingLabel::Extensions);
        assert!(main.iter().any(|l| l == "parseArgs"), "{main:?}");
        assert!(!main.iter().any(|l| l.contains("module import")), "no bleed into main: {main:?}");
        assert_eq!(ext, vec!["/ext/a module import", "/ext/a factory"], "{ext:?}");
        // Printing is side-effect-only; just ensure it does not panic with two groups present.
        print_timings();
    }

    /// The titles are Pi's, verbatim (`Startup Timings: ${namespace}`, timings.ts:47).
    #[test]
    fn namespace_titles_match_pi() {
        assert_eq!(TimingLabel::Main.as_str(), "main");
        assert_eq!(TimingLabel::Extensions.as_str(), "extensions");
    }

    /// A disabled build records nothing, whichever namespace is addressed.
    #[test]
    fn disabled_timings_record_nothing() {
        if timing_enabled() {
            // The suite is running under CYRUP_TIMING=1; the invariant under test does not apply.
            return;
        }
        time("never-recorded", TimingLabel::Main);
        assert!(
            !recorded_labels(TimingLabel::Main).iter().any(|l| l == "never-recorded"),
            "a disabled `time` must not push a mark"
        );
    }
}
