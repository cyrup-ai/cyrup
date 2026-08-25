//! The crate's SINGLE port of pi's `shared/formatters.ts`.
//!
//! These render human-facing strings that appear side by side in the same views (the TUI fleet
//! pane, the text fleet view, the `/subagents-cost` report, the status report), so a divergence
//! between two copies is directly visible to the user as the same number or the same run mode
//! rendered two ways. One definition each.

use crate::background::RunMode;

/// pi `formatTokens` (`shared/formatters.ts`): `< 1000` renders the raw integer, `< 10000` renders
/// one decimal place with a `k` suffix, otherwise a rounded-thousands `k`.
#[must_use]
pub fn format_tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 10_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        format!("{}k", (n as f64 / 1000.0).round() as u64)
    }
}

/// pi `formatModelThinking` (`shared/formatters.ts:19-29`): drop the provider prefix from the model
/// ref, and append `thinking <level>` when a recognised thinking level is known, joined by ` · `.
///
/// cyrup's [`cyrup_core::ModelId`] never carries pi's `:thinking-high` style suffix (the
/// level rides on `StepTelemetry::thinking`), so only the explicit-level half of pi's two sources
/// applies. The result is empty when neither half is known; callers wanting pi's
/// `formatModelThinking(...) || undefined` shape filter the empty string themselves.
#[must_use]
pub fn format_model_thinking(model: Option<&str>, thinking: Option<&str>) -> String {
    const THINKING_LEVELS: [&str; 4] = ["off", "low", "medium", "high"];
    let display_model = model.map(|m| match m.rfind('/') {
        Some(i) => m.get(i.saturating_add(1)..).unwrap_or(m),
        None => m,
    });
    let display_thinking = thinking
        .map(str::trim)
        .filter(|t| THINKING_LEVELS.contains(t));
    [
        display_model.map(str::to_string),
        display_thinking.map(|t| format!("thinking {t}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
}

/// JS `x || undefined` over [`format_model_thinking`]'s output — the `Option` shape pi's
/// `formatModelThinking(...) || undefined` call sites want.
#[must_use]
pub fn format_model_thinking_opt(model: Option<&str>, thinking: Option<&str>) -> Option<String> {
    let joined = format_model_thinking(model, thinking);
    if joined.is_empty() { None } else { Some(joined) }
}

/// The lowercase mode string pi renders (`SubagentRunMode`): `single`/`parallel`/`chain`.
#[must_use]
pub fn run_mode_label(mode: RunMode) -> &'static str {
    match mode {
        RunMode::Single => "single",
        RunMode::Parallel => "parallel",
        RunMode::Chain => "chain",
    }
}
