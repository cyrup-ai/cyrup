//! The warning renderer — a 1:1 port of `pi-subagents/src/watchdog/render.ts` (54 lines @v0.43.0).
//!
//! Two exports with very different audiences, and the split is the point:
//!
//! * [`format_watchdog_warning_render_text`] (`render.ts:23-38`) is the HUMAN-readable rendering of
//!   a warning — four fixed lines plus up to four conditional ones. It is a pure `details -> String`
//!   function upstream too, and it is what makes the renderer testable without a terminal.
//!   (`warning_format::format_watchdog_warning_content` is the *model*-readable rendering of the
//!   same record; the two must not be confused.)
//! * [`render_watchdog_warning`] (`render.ts:40-54`) turns that text into the collapsed/expanded
//!   component. Collapsed shows the headline plus the `⎿` continuation of line 2 (the evidence);
//!   expanded shows the headline, a blank line, then every remaining line dimmed.
//!
//! The state labels (`stateLabels`, `:14-21`) accumulate in a FIXED order and are joined into a
//! single parenthesized clause on the headline: `displayed`, then `stale · no auto-follow`, then
//! `failed review`, then `stalemate · auto-follow stopped`, then `auto-follow attempt N`. They are
//! not mutually exclusive — a record can be both `displayed` and carry an attempt number — so the
//! order is what makes the headline stable.
//!
//! [CYRUP-DELTA] upstream returns a `pi-tui` `Container` of `Text` children with a
//! `theme.fg(name, value)` string-colouring callback. This crate never depends on `cyrup-tui`
//! (`tui/mod.rs`'s standing crate-boundary rule), so — exactly as its sibling
//! [`crate::tui::render`] does — the component becomes `Vec<Line<'static>>` of styled
//! [`ratatui`] spans and the theme's three colour names (`error`, `warning`, `dim`) become the
//! ratatui styles the owning terminal crate paints. `theme.bold ?? identity` becomes an
//! unconditional [`Modifier::BOLD`] on the headline, which is what every real `pi-tui` theme
//! supplies.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::types::{WatchdogSeverity, WatchdogWarningDetails, WatchdogWarningState};

/// `titleCase` (`render.ts:8-10`): split on `-`, upper-case each part's first character, join with a
/// space. Applied only to the category, whose vocabulary is all lower-case ASCII kebab.
#[must_use]
pub fn title_case(value: &str) -> String {
    value
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `stateLabels` (`render.ts:12-21`) — the headline's parenthesized clause, in upstream's fixed
/// accumulation order.
#[must_use]
pub fn state_labels(warning: &WatchdogWarningDetails) -> Vec<String> {
    let mut labels = Vec::new();
    if warning.state == Some(WatchdogWarningState::Displayed) {
        labels.push("displayed".to_string());
    }
    if warning.stale == Some(true) || warning.state == Some(WatchdogWarningState::Stale) {
        labels.push("stale · no auto-follow".to_string());
    }
    if warning.state == Some(WatchdogWarningState::Failed) {
        labels.push("failed review".to_string());
    }
    if warning.state == Some(WatchdogWarningState::Stalemate) {
        labels.push("stalemate · auto-follow stopped".to_string());
    }
    if let Some(attempt) = warning.auto_follow_attempt {
        labels.push(format!("auto-follow attempt {attempt}"));
    }
    labels
}

/// `formatWatchdogWarningRenderText` (`render.ts:23-38`) — the whole human rendering, newline-joined.
///
/// Lines 1-4 always present (headline, evidence, recommended action, the category/source/agent/run
/// provenance line); then, conditionally and in this order, the failure text, the stalemate count
/// (singular/plural correct), and the stale disclaimer.
#[must_use]
pub fn format_watchdog_warning_render_text(warning: &WatchdogWarningDetails) -> String {
    let labels = state_labels(warning);
    let subject = if warning.severity == WatchdogSeverity::Blocker {
        "Blocker"
    } else {
        "Concern"
    };
    let label_clause = if labels.is_empty() {
        String::new()
    } else {
        format!(" ({})", labels.join(", "))
    };
    let agent_clause = warning
        .agent
        .as_deref()
        .map(|agent| format!(" · Agent: {agent}"))
        .unwrap_or_default();
    let run_clause = warning
        .run_id
        .as_deref()
        .map(|run| format!(" · Run: {run}"))
        .unwrap_or_default();
    let mut lines = vec![
        format!(
            "Subagent watchdog {subject}{label_clause}: {}",
            warning.summary
        ),
        format!("Evidence: {}", warning.evidence),
        format!("Recommended action: {}", warning.recommended_action),
        format!(
            "Category: {} · Source: {}{agent_clause}{run_clause}",
            title_case(warning.category.as_str()),
            warning.source.as_str()
        ),
    ];
    if warning.state == Some(WatchdogWarningState::Failed)
        && let Some(error) = warning.error.as_deref()
        && !error.is_empty()
    {
        lines.push(format!("Failure: {error}"));
    }
    if warning.state == Some(WatchdogWarningState::Stalemate)
        && let Some(repeats) = warning.stalemate_repeats
    {
        lines.push(format!(
            "Auto-follow stopped after {repeats} repeated blocker warning{}.",
            if repeats == 1 { "" } else { "s" }
        ));
    }
    if warning.stale == Some(true) || warning.state == Some(WatchdogWarningState::Stale) {
        lines.push(
            "This warning arrived after the watchdog catch-up timeout and must not auto-follow."
                .to_string(),
        );
    }
    lines.join("\n")
}

/// The `theme.fg("error"|"warning")` headline colour (`render.ts:44`): a blocker is an error, a
/// concern is a warning.
#[must_use]
pub const fn severity_style(severity: WatchdogSeverity) -> Style {
    match severity {
        WatchdogSeverity::Blocker => Style::new().fg(Color::Red),
        WatchdogSeverity::Concern => Style::new().fg(Color::Yellow),
    }
}

/// The `theme.fg("dim", ...)` body style.
fn dim_style() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

/// `renderWatchdogWarning` (`render.ts:40-54`).
///
/// `expanded` false renders exactly two lines — the bold, severity-coloured headline and the `⎿`
/// continuation carrying line 2 — and only when line 2 exists at all. `expanded` true renders the
/// headline, a blank spacer line, then every remaining line dimmed.
#[must_use]
pub fn render_watchdog_warning(
    warning: &WatchdogWarningDetails,
    expanded: bool,
) -> Vec<Line<'static>> {
    let text = format_watchdog_warning_render_text(warning);
    let all_lines: Vec<&str> = text.split('\n').collect();
    let headline = all_lines
        .first()
        .copied()
        .filter(|line| !line.is_empty())
        .unwrap_or("Subagent watchdog warning");
    let mut out = vec![Line::from(Span::styled(
        headline.to_string(),
        severity_style(warning.severity).add_modifier(Modifier::BOLD),
    ))];
    if expanded {
        out.push(Line::from(String::new()));
        for line in all_lines.iter().skip(1) {
            out.push(Line::from(Span::styled((*line).to_string(), dim_style())));
        }
    // `else if (lines[1])` (`render.ts:52`) is a TRUTHINESS test, so an empty second line adds no
    // continuation row at all rather than an empty one.
    } else if let Some(second) = all_lines.get(1).filter(|line| !line.is_empty()) {
        out.push(Line::from(Span::styled(
            format!("  ⎿  {second}"),
            dim_style(),
        )));
    }
    out
}

/// The plain text of a rendered warning, line by line.
///
/// Assertion surface only — it has no production caller and no upstream counterpart (`render.ts`
/// returns styled strings and nothing re-flattens them). It is the sibling of
/// [`crate::tui::render::lines_to_plain_text`]. Note that a test asserting ONLY through this
/// function asserts no colour: [`render_watchdog_warning`] carries the severity styling, so a
/// plain-text assertion cannot catch a repaint regression on its own.
#[must_use]
pub fn render_watchdog_warning_plain(
    warning: &WatchdogWarningDetails,
    expanded: bool,
) -> Vec<String> {
    crate::tui::render::lines_to_plain_text(&render_watchdog_warning(warning, expanded))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::watchdog::types::{WatchdogCategory, WatchdogWarningSource};

    fn details() -> WatchdogWarningDetails {
        WatchdogWarningDetails {
            severity: WatchdogSeverity::Concern,
            summary: "the summary".into(),
            evidence: "the evidence".into(),
            recommended_action: "do the thing".into(),
            category: WatchdogCategory::MissedConstraint,
            source: WatchdogWarningSource::Main,
            confidence: None,
            agent: None,
            run_id: None,
            stale: None,
            auto_follow_attempt: None,
            state: None,
            identity: None,
            displayed_at: None,
            error: None,
            stalemate_repeats: None,
        }
    }

    #[test]
    fn title_case_splits_on_hyphens() {
        assert_eq!(title_case("missed-constraint"), "Missed Constraint");
        assert_eq!(title_case("other"), "Other");
        assert_eq!(title_case("a--b"), "A  B");
    }

    #[test]
    fn the_four_base_lines_are_always_present() {
        assert_eq!(
            format_watchdog_warning_render_text(&details()),
            concat!(
                "Subagent watchdog Concern: the summary\n",
                "Evidence: the evidence\n",
                "Recommended action: do the thing\n",
                "Category: Missed Constraint · Source: main",
            )
        );
    }

    #[test]
    fn agent_and_run_extend_the_provenance_line_in_order() {
        let mut warning = details();
        warning.agent = Some("reviewer".into());
        warning.run_id = Some("run-7".into());
        assert!(
            format_watchdog_warning_render_text(&warning).contains(
                "Category: Missed Constraint · Source: main · Agent: reviewer · Run: run-7"
            )
        );
    }

    #[test]
    fn state_labels_accumulate_in_upstream_order() {
        let mut warning = details();
        warning.state = Some(WatchdogWarningState::Displayed);
        warning.stale = Some(true);
        warning.auto_follow_attempt = Some(2);
        assert_eq!(
            state_labels(&warning),
            vec![
                "displayed".to_string(),
                "stale · no auto-follow".to_string(),
                "auto-follow attempt 2".to_string(),
            ]
        );
        assert!(format_watchdog_warning_render_text(&warning).starts_with(
            "Subagent watchdog Concern (displayed, stale · no auto-follow, auto-follow attempt 2): "
        ));
    }

    #[test]
    fn a_stale_record_appends_the_no_auto_follow_disclaimer() {
        let mut warning = details();
        warning.state = Some(WatchdogWarningState::Stale);
        let text = format_watchdog_warning_render_text(&warning);
        assert!(text.ends_with(
            "This warning arrived after the watchdog catch-up timeout and must not auto-follow."
        ));
    }

    #[test]
    fn a_failed_record_prints_its_error_only_when_it_has_one() {
        let mut warning = details();
        warning.state = Some(WatchdogWarningState::Failed);
        assert!(!format_watchdog_warning_render_text(&warning).contains("Failure:"));
        warning.error = Some("model refused".into());
        assert!(format_watchdog_warning_render_text(&warning).contains("Failure: model refused"));
    }

    #[test]
    fn the_stalemate_line_is_singular_for_one_repeat() {
        let mut warning = details();
        warning.state = Some(WatchdogWarningState::Stalemate);
        warning.stalemate_repeats = Some(1);
        assert!(
            format_watchdog_warning_render_text(&warning)
                .contains("Auto-follow stopped after 1 repeated blocker warning.")
        );
        warning.stalemate_repeats = Some(3);
        assert!(
            format_watchdog_warning_render_text(&warning)
                .contains("Auto-follow stopped after 3 repeated blocker warnings.")
        );
    }

    #[test]
    fn collapsed_renders_the_headline_and_the_evidence_continuation_only() {
        let plain = render_watchdog_warning_plain(&details(), false);
        assert_eq!(
            plain,
            vec![
                "Subagent watchdog Concern: the summary".to_string(),
                "  ⎿  Evidence: the evidence".to_string(),
            ]
        );
    }

    #[test]
    fn expanded_renders_a_spacer_then_every_remaining_line() {
        let plain = render_watchdog_warning_plain(&details(), true);
        assert_eq!(plain.len(), 5);
        assert_eq!(plain[1], "");
        assert_eq!(plain[2], "Evidence: the evidence");
        assert_eq!(plain[4], "Category: Missed Constraint · Source: main");
    }

    #[test]
    fn a_blocker_headline_is_red_and_a_concern_headline_is_yellow() {
        let mut blocker = details();
        blocker.severity = WatchdogSeverity::Blocker;
        let rendered = render_watchdog_warning(&blocker, false);
        assert_eq!(rendered[0].spans[0].style.fg, Some(Color::Red));
        assert!(
            rendered[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            rendered[0].spans[0]
                .content
                .starts_with("Subagent watchdog Blocker: "),
            "the subject switches with the severity"
        );
        let concern = render_watchdog_warning(&details(), false);
        assert_eq!(concern[0].spans[0].style.fg, Some(Color::Yellow));
    }
}
