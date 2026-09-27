//! Startup timing table (Pi `core/timings.ts`) — the RECORDING half: `resetTimings` / `time` and
//! the namespace map they write. Enabled by `CYRUP_TIMING=1` (Pi's `PI_TIMING=1`; the legacy
//! spelling is no longer honoured); every entry point is inert otherwise.
//!
//! # Why this lives in cyrup-core (AGENT-027)
//!
//! Pi's namespaces are a module-level `const timingNamespaces = new Map<TimingLabel,
//! TimingNamespace>()` (`timings.ts:14` @v0.87.1), which is what lets unrelated modules mark into
//! the same table with no handle passed between them: `main.ts` fills `"main"`, while
//! `core/resource-loader.ts:389` resets `"extensions"` and `core/extensions/loader.ts:553/:568`
//! fill it per extension. cyrup's port sat in the top-level `cyrup` bin crate, which the extension
//! host (`cyrup-ext`) and the session builder (`cyrup-session-svc`) sit BELOW — so the
//! `extensions` namespace existed with no crate able to reach it. The table now lives in the
//! workspace's lowest crate, reachable from every producer; printing (`printTimings`, stderr) stays
//! in the bin, which is the only place that prints it, so this crate keeps its no-I/O charter.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Whether startup timings are enabled (`CYRUP_TIMING=1`).
///
/// Pi reads its `ENABLED` once at module load (`timings.ts:6`), so the answer cannot change
/// mid-process; the `OnceLock` reproduces that (and keeps [`time`] off the env-var path per mark).
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| matches!(std::env::var("CYRUP_TIMING").ok().as_deref(), Some("1")))
}

/// Which table a mark lands in (Pi `type TimingLabel = "main" | "extensions"`, timings.ts:12).
/// A closed set upstream, so a closed enum here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingLabel {
    /// The binary's own startup phases (Pi `main.ts`).
    Main,
    /// One entry per extension module import / factory call (Pi `extensions/loader.ts:553,568`).
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
    if !enabled() {
        return;
    }
    let Ok(mut ns) = namespaces().lock() else {
        return;
    };
    let fresh = TimingNamespace {
        timings: Vec::new(),
        last: Instant::now(),
    };
    match ns.iter_mut().find(|(k, _)| *k == namespace) {
        // `Map.set` on an existing key REPLACES the value and keeps the key's original position.
        Some((_, slot)) => *slot = fresh,
        None => ns.push((namespace, fresh)),
    }
}

/// Record the interval since the previous mark in `namespace` under `label` (Pi `time`,
/// timings.ts:21-32), auto-resetting the namespace on first use exactly as Pi does at `:26-28`.
pub fn time(label: &str, namespace: TimingLabel) {
    if !enabled() {
        return;
    }
    let Ok(mut ns) = namespaces().lock() else {
        return;
    };
    push_mark(&mut ns, namespace, label, Instant::now());
}

fn push_mark(
    ns: &mut Vec<(TimingLabel, TimingNamespace)>,
    namespace: TimingLabel,
    label: &str,
    now: Instant,
) {
    if !ns.iter().any(|(k, _)| *k == namespace) {
        ns.push((
            namespace,
            TimingNamespace {
                timings: Vec::new(),
                last: now,
            },
        ));
    }
    if let Some((_, slot)) = ns.iter_mut().find(|(k, _)| *k == namespace) {
        slot.timings
            .push((label.to_string(), now.duration_since(slot.last).as_millis()));
        slot.last = now;
    }
}

/// Every namespace's `(label, ms)` rows, in Map insertion order — what Pi's `printTimings` iterates
/// (timings.ts:45-49). The bin's `print_timings` formats this; tests assert the label set with it
/// instead of parsing stderr.
pub fn snapshot() -> Vec<(TimingLabel, Vec<(String, u128)>)> {
    namespaces()
        .lock()
        .map(|ns| {
            ns.iter()
                .map(|(label, group)| (*label, group.timings.clone()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn labels(namespace: TimingLabel) -> Vec<String> {
        snapshot()
            .into_iter()
            .find(|(k, _)| *k == namespace)
            .map(|(_, rows)| rows.into_iter().map(|(l, _)| l).collect())
            .unwrap_or_default()
    }

    /// AGENT-027 — two namespaces coexist and keep their own tables. The table is process-global
    /// (as pi's is), so this writes through the ungated `push_mark` rather than the env-gated
    /// public entry points, which would be order-dependent across tests.
    #[test]
    fn namespaces_are_independent_and_keep_insertion_order() {
        {
            let mut ns = namespaces().lock().unwrap();
            push_mark(&mut ns, TimingLabel::Main, "parseArgs", Instant::now());
            push_mark(
                &mut ns,
                TimingLabel::Extensions,
                "/ext/a module import",
                Instant::now(),
            );
            push_mark(
                &mut ns,
                TimingLabel::Extensions,
                "/ext/a factory",
                Instant::now(),
            );
        }
        let main = labels(TimingLabel::Main);
        assert!(main.iter().any(|l| l == "parseArgs"), "{main:?}");
        assert!(
            !main.iter().any(|l| l.contains("module import")),
            "no bleed into main: {main:?}"
        );
        assert_eq!(
            labels(TimingLabel::Extensions),
            vec!["/ext/a module import", "/ext/a factory"]
        );
        let order: Vec<TimingLabel> = snapshot().into_iter().map(|(k, _)| k).collect();
        assert_eq!(order, vec![TimingLabel::Main, TimingLabel::Extensions]);
    }

    /// The titles are Pi's, verbatim (`Startup Timings: ${namespace}`, timings.ts:47).
    #[test]
    fn namespace_titles_match_pi() {
        assert_eq!(TimingLabel::Main.as_str(), "main");
        assert_eq!(TimingLabel::Extensions.as_str(), "extensions");
    }

    /// A disabled process records nothing, whichever namespace is addressed.
    #[test]
    fn disabled_timings_record_nothing() {
        if enabled() {
            // The suite is running under CYRUP_TIMING=1; the invariant under test does not apply.
            return;
        }
        time("never-recorded", TimingLabel::Main);
        assert!(
            !labels(TimingLabel::Main)
                .iter()
                .any(|l| l == "never-recorded"),
            "a disabled `time` must not push a mark"
        );
    }
}
