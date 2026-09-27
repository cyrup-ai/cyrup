//! Startup timing instrumentation (Pi `core/timings.ts`). A faithful port of `resetTimings`/`time`/
//! `printTimings`: when enabled by `CYRUP_TIMING=1` (Pi's `PI_TIMING=1`; the legacy spelling is
//! no longer honoured), each [`time`] call
//! records the elapsed milliseconds since the previous mark IN ITS NAMESPACE, and [`print_timings`]
//! writes one titled group per namespace to **stderr** (never stdout — the protocol stream stays
//! clean).
//!
//! # Why the table is process-global and lives in cyrup-core (AGENT-027)
//!
//! Pi's namespaces live in a module-level `const timingNamespaces = new Map<TimingLabel,
//! TimingNamespace>()` (`timings.ts:14`), which is what lets two unrelated modules mark into the
//! same table: `main.ts` fills `"main"` while `core/resource-loader.ts:389` resets `"extensions"`
//! and `core/extensions/loader.ts:553/:568` fills it per extension (@v0.87.1), with no handle passed
//! between them. The recording half ([`reset_timings`], [`time`], the map) is therefore
//! `cyrup_core::timings`, the one crate the extension host and the session builder can both reach;
//! this module re-exports it and keeps the printing half, which only the bin performs.
//!
//! Separately, `CYRUP_STARTUP_BENCHMARK` (Pi main.ts:800, `PI_STARTUP_BENCHMARK`) requests the
//! interactive-init benchmark; the bin gates it to interactive mode via [`startup_benchmark_enabled`]
//! and reports the same "only supports interactive mode" error in the one-shot modes.

pub use cyrup_sdk::core::timings::{TimingLabel, reset_timings, time};

/// Whether the interactive startup benchmark is requested (`CYRUP_STARTUP_BENCHMARK`, truthy
/// `1`/`true`/`yes`; Pi `PI_STARTUP_BENCHMARK`).
pub fn startup_benchmark_enabled() -> bool {
    fn truthy(v: Option<String>) -> bool {
        matches!(
            v.as_deref().map(str::to_ascii_lowercase).as_deref(),
            Some("1" | "true" | "yes")
        )
    }
    truthy(std::env::var("CYRUP_STARTUP_BENCHMARK").ok())
}

/// Print one titled group per namespace to stderr (Pi `printTimings`, timings.ts:45-49). Inert
/// unless enabled.
///
/// Pi's `printTimingGroup` filters `timing.ms >= 0` (`:34`) because its clock is `Date.now()`, which
/// an NTP step can move backwards. cyrup measures with `Instant`, which is monotonic, so the
/// filter is vacuous here and is deliberately not reproduced — every recorded mark is printable.
pub fn print_timings() {
    if !cyrup_sdk::core::timings::enabled() {
        return;
    }
    eprint!("{}", render_timings(&cyrup_sdk::core::timings::snapshot()));
}

/// The text [`print_timings`] writes: Pi's `printTimingGroup` per namespace (timings.ts:34-43),
/// skipping a namespace with no marks (`:36`).
fn render_timings(groups: &[(TimingLabel, Vec<(String, u128)>)]) -> String {
    let mut out = String::new();
    for (label, rows) in groups {
        if rows.is_empty() {
            continue;
        }
        let title = format!("Startup Timings: {}", label.as_str());
        out.push_str(&format!("\n--- {title} ---\n"));
        let mut total = 0u128;
        for (l, ms) in rows {
            out.push_str(&format!("  {l}: {ms}ms\n"));
            total += *ms;
        }
        out.push_str(&format!("  TOTAL: {total}ms\n"));
        out.push_str(&format!("{}\n\n", "-".repeat(title.len() + 8)));
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// AGENT-027 — one titled group per namespace, in table order, byte-for-byte Pi's
    /// `printTimingGroup` layout (`\n--- title ---`, `  label: Nms`, `  TOTAL: Nms`, a dash rule
    /// `title.length + 8` wide, then a blank line); an empty namespace prints nothing.
    #[test]
    fn renders_one_titled_group_per_namespace() {
        let groups = vec![
            (TimingLabel::Main, vec![("parseArgs".to_string(), 3)]),
            (
                TimingLabel::Extensions,
                vec![
                    ("/ext/a module import".to_string(), 5),
                    ("/ext/a factory".to_string(), 2),
                ],
            ),
        ];
        let rule_main = "-".repeat("Startup Timings: main".len() + 8);
        let rule_ext = "-".repeat("Startup Timings: extensions".len() + 8);
        assert_eq!(
            render_timings(&groups),
            format!(
                "\n--- Startup Timings: main ---\n  parseArgs: 3ms\n  TOTAL: 3ms\n{rule_main}\n\n\
                 \n--- Startup Timings: extensions ---\n  /ext/a module import: 5ms\n  \
                 /ext/a factory: 2ms\n  TOTAL: 7ms\n{rule_ext}\n\n"
            )
        );
        assert_eq!(
            render_timings(&[(TimingLabel::Extensions, Vec::new())]),
            "",
            "Pi skips an empty group (timings.ts:36)"
        );
    }
}
