//! Warning normalization and the `<subagent_watchdog>` message body — a 1:1 port of
//! `pi-subagents/src/watchdog/warning-format.ts` (73 lines @v0.43.0).
//!
//! Three exports, consumed by every watchdog role:
//!
//! * `normalizeWatchdogWarningDetails` (`:24-31`) — the ONE widening conversion from the optional
//!   `WatchdogWarning` to the resolved `WatchdogWarningDetails`, defaulting `category` to `other`
//!   and `source` to `main`. Called from `runtime.ts:438,502,516` and from
//!   `createWatchdogWarningMessage` itself.
//! * `formatWatchdogWarningContent` (`:33-60`) — the XML block the MODEL reads. This is the
//!   warning's LLM-visible content: an attribute-carrying `<subagent_watchdog>` element whose
//!   `guidance="weigh, don't blindly obey"` attribute is a literal, not a computed value.
//! * `createWatchdogWarningMessage` (`:62-73`) — the custom transcript message
//!   (`{customType, content, display, details}`) `register-main.ts:371,387` and
//!   `register-child.ts:82` send.
//!
//! ### The `{...warning, category, source, ...extras}` spread, in Rust
//!
//! Upstream's normalizer is one object spread whose LAST term is `extras`, so an extras key wins
//! over both the base warning and the computed defaults, while a key absent from extras leaves the
//! base value alone. [`WatchdogWarningDetailsPatch`] is that `Partial<WatchdogWarningDetails>`:
//! every field is an `Option`, `Some` overrides, `None` defers. The one asymmetry upstream has —
//! `category`/`source` consult `extras` BEFORE falling back to the literal defaults (`:27-28`) and
//! then get overwritten by the same `extras` value again (`:29`) — is a no-op there and is folded
//! into a single `warning ?? extras ?? default` here.
//!
//! ### Escaping
//!
//! `escapeXmlText` (`:8-13`) escapes `&`, `<`, `>` in that order (ampersand first, or the escapes
//! would themselves be escaped); `escapeXmlAttribute` (`:15-17`) adds `"`. Reproduced exactly —
//! this text is concatenated into markup and read back by a model, so the order is load-bearing.

use super::types::{
    WatchdogCategory, WatchdogConfidence, WatchdogSeverity, WatchdogWarning,
    WatchdogWarningDetails, WatchdogWarningMessage, WatchdogWarningSource, WatchdogWarningState,
    SUBAGENT_WATCHDOG_WARNING_TYPE,
};

/// `Partial<WatchdogWarningDetails>` — the `extras` argument of
/// `normalizeWatchdogWarningDetails` (`warning-format.ts:24`) and the `options.details` of
/// `createWatchdogWarningMessage` (`:64`). Every field is optional and, when present, wins over
/// the base warning's own value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchdogWarningDetailsPatch {
    /// Overrides `category` (and supplies the pre-`"other"` fallback).
    pub category: Option<WatchdogCategory>,
    /// Overrides `source` (and supplies the pre-`"main"` fallback).
    pub source: Option<WatchdogWarningSource>,
    /// Overrides `confidence`.
    pub confidence: Option<WatchdogConfidence>,
    /// Overrides `agent`.
    pub agent: Option<String>,
    /// Overrides `runId`.
    pub run_id: Option<String>,
    /// Overrides `stale`.
    pub stale: Option<bool>,
    /// Overrides `autoFollowAttempt`.
    pub auto_follow_attempt: Option<u32>,
    /// Overrides `state`.
    pub state: Option<WatchdogWarningState>,
    /// Sets `identity` (details-only; the base warning has no such field).
    pub identity: Option<String>,
    /// Sets `displayedAt` (details-only).
    pub displayed_at: Option<String>,
    /// Sets `error` (details-only).
    pub error: Option<String>,
    /// Sets `stalemateRepeats` (details-only).
    pub stalemate_repeats: Option<u32>,
}

impl WatchdogWarningDetailsPatch {
    /// The `{ state, source }` extras shape `runtime.ts:438,502,516` builds — the only three
    /// upstream call sites that construct one by hand.
    #[must_use]
    pub fn new(state: WatchdogWarningState, source: WatchdogWarningSource) -> Self {
        Self {
            state: Some(state),
            source: Some(source),
            ..Self::default()
        }
    }

    /// Chain `identity` onto the patch (`runtime.ts:505,519`).
    #[must_use]
    pub fn with_identity(mut self, identity: impl Into<String>) -> Self {
        self.identity = Some(identity.into());
        self
    }

    /// Chain `displayedAt` onto the patch (`runtime.ts:520`).
    #[must_use]
    pub fn with_displayed_at(mut self, displayed_at: impl Into<String>) -> Self {
        self.displayed_at = Some(displayed_at.into());
        self
    }
}

/// `normalizeWatchdogWarningDetails(warning, extras)` (`warning-format.ts:24-31`).
#[must_use]
pub fn normalize_watchdog_warning_details(
    warning: &WatchdogWarning,
    extras: &WatchdogWarningDetailsPatch,
) -> WatchdogWarningDetails {
    WatchdogWarningDetails {
        severity: warning.severity,
        summary: warning.summary.clone(),
        evidence: warning.evidence.clone(),
        recommended_action: warning.recommended_action.clone(),
        // `warning.category ?? extras.category ?? "other"` (`:27`).
        category: warning
            .category
            .or(extras.category)
            .unwrap_or(WatchdogCategory::Other),
        // `warning.source ?? extras.source ?? "main"` (`:28`).
        source: warning
            .source
            .or(extras.source)
            .unwrap_or(WatchdogWarningSource::Main),
        confidence: extras.confidence.or(warning.confidence),
        agent: extras.agent.clone().or_else(|| warning.agent.clone()),
        run_id: extras.run_id.clone().or_else(|| warning.run_id.clone()),
        stale: extras.stale.or(warning.stale),
        auto_follow_attempt: extras.auto_follow_attempt.or(warning.auto_follow_attempt),
        state: extras.state.or(warning.state),
        identity: extras.identity.clone(),
        displayed_at: extras.displayed_at.clone(),
        error: extras.error.clone(),
        stalemate_repeats: extras.stalemate_repeats,
    }
}

/// The `WatchdogWarningDetails -> WatchdogWarning` narrowing TypeScript gets for free from
/// structural subtyping (`emission-guard.ts`'s `evaluate(warning)` is handed a details value at
/// `runtime.ts:653`, and `createWatchdogWarningMessage(details, …)` at `register-main.ts:371`
/// passes one as its `warning` argument).
#[must_use]
pub fn details_as_warning(details: &WatchdogWarningDetails) -> WatchdogWarning {
    WatchdogWarning {
        severity: details.severity,
        summary: details.summary.clone(),
        evidence: details.evidence.clone(),
        recommended_action: details.recommended_action.clone(),
        category: Some(details.category),
        confidence: details.confidence,
        source: Some(details.source),
        agent: details.agent.clone(),
        run_id: details.run_id.clone(),
        stale: details.stale,
        auto_follow_attempt: details.auto_follow_attempt,
        state: details.state,
    }
}

/// `escapeXmlText` (`warning-format.ts:8-13`) — `&` first, then `<`, then `>`.
fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `escapeXmlAttribute` (`warning-format.ts:15-17`) — text escaping plus `"`.
fn escape_xml_attribute(value: &str) -> String {
    escape_xml_text(value).replace('"', "&quot;")
}

/// `tag(name, value)` (`warning-format.ts:19-22`) — `undefined` yields no line at all.
fn tag(name: &str, value: Option<String>) -> Option<String> {
    let value = value?;
    Some(format!("<{name}>{}</{name}>", escape_xml_text(&value)))
}

/// `formatWatchdogWarningContent(details)` (`warning-format.ts:33-60`), on an already-normalized
/// details record. See [`format_watchdog_warning_content`] for the `WatchdogWarning` entry point.
#[must_use]
pub fn format_watchdog_warning_content_from_details(details: &WatchdogWarningDetails) -> String {
    let attrs = [
        format!(
            "severity=\"{}\"",
            escape_xml_attribute(details.severity.as_str())
        ),
        format!(
            "category=\"{}\"",
            escape_xml_attribute(details.category.as_str())
        ),
        format!(
            "source=\"{}\"",
            escape_xml_attribute(details.source.as_str())
        ),
        // A literal upstream, escaped-in-place there only because it flows through the same
        // template; there is nothing to escape in it (`:41`).
        "guidance=\"weigh, don't blindly obey\"".to_string(),
    ];
    let mut lines = vec![
        format!("<subagent_watchdog {}>", attrs.join(" ")),
        format!("<summary>{}</summary>", escape_xml_text(&details.summary)),
        format!("<evidence>{}</evidence>", escape_xml_text(&details.evidence)),
        format!(
            "<recommended_action>{}</recommended_action>",
            escape_xml_text(&details.recommended_action)
        ),
    ];
    // `:43-50` — every optional tag, in upstream's order, dropped when the field is absent.
    let optional = [
        tag(
            "confidence",
            details.confidence.map(|c| c.as_str().to_string()),
        ),
        tag("agent", details.agent.clone()),
        tag("run_id", details.run_id.clone()),
        tag("state", details.state.map(|s| s.as_str().to_string())),
        // `String(true)`/`String(false)` — JS stringifies the boolean, it is not dropped when
        // false; only `undefined` drops the tag.
        tag("stale", details.stale.map(|s| s.to_string())),
        tag(
            "auto_follow_attempt",
            details.auto_follow_attempt.map(|a| a.to_string()),
        ),
    ];
    lines.extend(optional.into_iter().flatten());
    if details.severity == WatchdogSeverity::Blocker {
        lines.push(
            "<blocker_guidance>If this warning changes the outcome, produce a new self-contained final answer after addressing it.</blocker_guidance>"
                .to_string(),
        );
    }
    lines.push("</subagent_watchdog>".to_string());
    lines.join("\n")
}

/// `formatWatchdogWarningContent(warning)` (`warning-format.ts:33-60`) — normalizes first
/// (`:34`), exactly as upstream does, then renders.
#[must_use]
pub fn format_watchdog_warning_content(warning: &WatchdogWarning) -> String {
    let details =
        normalize_watchdog_warning_details(warning, &WatchdogWarningDetailsPatch::default());
    format_watchdog_warning_content_from_details(&details)
}

/// `createWatchdogWarningMessage(warning, { display, details })` (`warning-format.ts:62-73`).
#[must_use]
pub fn create_watchdog_warning_message(
    warning: &WatchdogWarning,
    display: bool,
    details: &WatchdogWarningDetailsPatch,
) -> WatchdogWarningMessage {
    let details = normalize_watchdog_warning_details(warning, details);
    WatchdogWarningMessage {
        custom_type: SUBAGENT_WATCHDOG_WARNING_TYPE.to_string(),
        content: format_watchdog_warning_content_from_details(&details),
        display,
        details,
    }
}

/// The `createWatchdogWarningMessage(details, { display: true, details })` shape every real call
/// site uses (`register-main.ts:371,387`, `register-child.ts:82`): the warning argument and the
/// extras are the SAME already-normalized details record, so normalization is the identity and the
/// message simply carries it.
#[must_use]
pub fn create_watchdog_warning_message_from_details(
    details: &WatchdogWarningDetails,
    display: bool,
) -> WatchdogWarningMessage {
    WatchdogWarningMessage {
        custom_type: SUBAGENT_WATCHDOG_WARNING_TYPE.to_string(),
        content: format_watchdog_warning_content_from_details(details),
        display,
        details: details.clone(),
    }
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

    fn warning() -> WatchdogWarning {
        WatchdogWarning::new(
            WatchdogSeverity::Concern,
            "summary <one>",
            "evidence & more",
            "do the thing",
        )
    }

    #[test]
    fn normalization_defaults_category_to_other_and_source_to_main() {
        let details = normalize_watchdog_warning_details(
            &warning(),
            &WatchdogWarningDetailsPatch::default(),
        );
        assert_eq!(details.category, WatchdogCategory::Other);
        assert_eq!(details.source, WatchdogWarningSource::Main);
        assert_eq!(details.state, None);
    }

    #[test]
    fn the_warnings_own_source_beats_the_extras_source() {
        // Upstream `:28` is `warning.source ?? extras.source ?? "main"` — the WARNING wins.
        let mut w = warning();
        w.source = Some(WatchdogWarningSource::Lsp);
        let details = normalize_watchdog_warning_details(
            &w,
            &WatchdogWarningDetailsPatch::new(
                WatchdogWarningState::Candidate,
                WatchdogWarningSource::Main,
            ),
        );
        assert_eq!(details.source, WatchdogWarningSource::Lsp);
        assert_eq!(details.state, Some(WatchdogWarningState::Candidate));
    }

    #[test]
    fn extras_set_the_details_only_fields() {
        let details = normalize_watchdog_warning_details(
            &warning(),
            &WatchdogWarningDetailsPatch::new(
                WatchdogWarningState::Displayed,
                WatchdogWarningSource::Main,
            )
            .with_identity("id-1")
            .with_displayed_at("1970-01-01T00:00:00.000Z"),
        );
        assert_eq!(details.identity.as_deref(), Some("id-1"));
        assert_eq!(
            details.displayed_at.as_deref(),
            Some("1970-01-01T00:00:00.000Z")
        );
    }

    #[test]
    fn content_escapes_amp_before_the_angle_brackets() {
        let content = format_watchdog_warning_content(&warning());
        assert!(content.contains("<summary>summary &lt;one&gt;</summary>"), "{content}");
        assert!(
            content.contains("<evidence>evidence &amp; more</evidence>"),
            "{content}"
        );
        // A `&` produced by an earlier escape must not be re-escaped into `&amp;lt;`.
        assert!(!content.contains("&amp;lt;"), "{content}");
    }

    #[test]
    fn a_concern_carries_no_blocker_guidance_and_a_blocker_does() {
        let concern = format_watchdog_warning_content(&warning());
        assert!(!concern.contains("<blocker_guidance>"));
        let mut w = warning();
        w.severity = WatchdogSeverity::Blocker;
        let blocker = format_watchdog_warning_content(&w);
        assert!(blocker.contains(
            "<blocker_guidance>If this warning changes the outcome, produce a new self-contained final answer after addressing it.</blocker_guidance>"
        ));
        // The closing tag is last in both.
        assert!(blocker.ends_with("</subagent_watchdog>"));
    }

    #[test]
    fn optional_tags_appear_in_upstream_order_and_absent_ones_are_dropped() {
        let mut w = warning();
        w.confidence = Some(WatchdogConfidence::High);
        w.agent = Some("reviewer".into());
        w.stale = Some(false);
        let content = format_watchdog_warning_content(&w);
        let confidence = content.find("<confidence>").unwrap();
        let agent = content.find("<agent>").unwrap();
        let stale = content.find("<stale>").unwrap();
        assert!(confidence < agent && agent < stale, "{content}");
        assert!(!content.contains("<run_id>"), "{content}");
        // `stale: false` is stringified, not dropped.
        assert!(content.contains("<stale>false</stale>"), "{content}");
    }

    #[test]
    fn the_message_carries_the_custom_type_the_renderer_registers() {
        let details = normalize_watchdog_warning_details(
            &warning(),
            &WatchdogWarningDetailsPatch::default(),
        );
        let message = create_watchdog_warning_message_from_details(&details, true);
        assert_eq!(message.custom_type, SUBAGENT_WATCHDOG_WARNING_TYPE);
        assert!(message.display);
        assert_eq!(message.details, details);
        assert_eq!(
            message.content,
            format_watchdog_warning_content_from_details(&details)
        );
    }

    #[test]
    fn details_round_trip_back_to_a_warning_for_the_emission_guard() {
        let details = normalize_watchdog_warning_details(
            &warning(),
            &WatchdogWarningDetailsPatch::new(
                WatchdogWarningState::Candidate,
                WatchdogWarningSource::Child,
            ),
        );
        let back = details_as_warning(&details);
        assert_eq!(back.summary, details.summary);
        assert_eq!(back.category, Some(details.category));
        assert_eq!(back.source, Some(details.source));
    }
}
