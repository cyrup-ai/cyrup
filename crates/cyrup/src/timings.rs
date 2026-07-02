//! Startup timing instrumentation (Pi `core/timings.ts`). A faithful port of `resetTimings`/`time`/
//! `printTimings`: when enabled by `CYRUP_TIMING=1` (or Pi's `PI_TIMING=1`), each [`Timings::mark`]
//! records the elapsed milliseconds since the previous mark, and [`Timings::print`] writes the
//! grouped table to **stderr** (never stdout — the protocol stream stays clean).
//!
//! Separately, `PI_STARTUP_BENCHMARK`/`CYRUP_STARTUP_BENCHMARK` (Pi main.ts:800) requests the
//! interactive-init benchmark; the bin gates it to interactive mode via [`startup_benchmark_enabled`]
//! and reports the same "only supports interactive mode" error in the one-shot modes.

use std::time::Instant;

/// Whether startup timings are enabled (`CYRUP_TIMING=1` / `PI_TIMING=1`).
fn timing_enabled() -> bool {
    matches!(std::env::var("CYRUP_TIMING").ok().as_deref(), Some("1"))
        || matches!(std::env::var("PI_TIMING").ok().as_deref(), Some("1"))
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

/// A namespace of labelled timing intervals (Pi `TimingNamespace`).
#[derive(Debug)]
pub struct Timings {
    enabled: bool,
    last: Instant,
    entries: Vec<(String, u128)>,
}

impl Default for Timings {
    fn default() -> Self {
        Self::new()
    }
}

impl Timings {
    /// Start a timing run (Pi `resetTimings`). Inert unless `CYRUP_TIMING=1`.
    pub fn new() -> Self {
        Self {
            enabled: timing_enabled(),
            last: Instant::now(),
            entries: Vec::new(),
        }
    }

    /// Record the interval since the previous mark under `label` (Pi `time`).
    pub fn mark(&mut self, label: &str) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        let ms = now.duration_since(self.last).as_millis();
        self.entries.push((label.to_string(), ms));
        self.last = now;
    }

    /// Print the grouped timing table to stderr (Pi `printTimings`). Inert unless enabled.
    pub fn print(&self) {
        if !self.enabled || self.entries.is_empty() {
            return;
        }
        let title = "Startup Timings: main";
        eprintln!("\n--- {title} ---");
        let mut total = 0u128;
        for (label, ms) in &self.entries {
            eprintln!("  {label}: {ms}ms");
            total += *ms;
        }
        eprintln!("  TOTAL: {total}ms");
        eprintln!("{}\n", "-".repeat(title.len() + 8));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn marks_accumulate_when_enabled() {
        // Force-enable via the struct field (env-independent) so the test is deterministic.
        let mut t = Timings {
            enabled: true,
            last: Instant::now(),
            entries: Vec::new(),
        };
        t.mark("parseArgs");
        t.mark("createSession");
        assert_eq!(t.entries.len(), 2);
        assert_eq!(t.entries[0].0, "parseArgs");
        // Printing is side-effect-only; just ensure it does not panic.
        t.print();
    }

    #[test]
    fn disabled_timings_record_nothing() {
        let mut t = Timings {
            enabled: false,
            last: Instant::now(),
            entries: Vec::new(),
        };
        t.mark("x");
        assert!(t.entries.is_empty());
    }
}
