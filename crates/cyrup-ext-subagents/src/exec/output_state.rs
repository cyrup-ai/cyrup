//! Whether a child produced substantive output — pi `SubagentOutputState` (`shared/types.ts:399`)
//! and its one derivation, ported from `subagent-runner.ts:1442-1447` (full form) /
//! `run-child-session.ts:549`, `subagent-runner.ts:927` (the simple two-branch form, which is this
//! module's [`derive_output_state`] with its last two arguments `None`).

/// Whether the child produced substantive output before its process ended — pi
/// `SubagentOutputState` (`shared/types.ts:399`).
///
/// Three-valued rather than a `bool` because "produced nothing" and "we could not tell" are
/// different answers to a parent deciding whether to retry: a run whose output went to a saved
/// file is [`Self::Unknown`], not [`Self::Absent`].
///
/// `#[derive(Default)]` with `#[default] Unknown` gives `#[serde(default)]` the honest decode for
/// a record written before this field existed — such a record genuinely does not know.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentOutputState {
    Present,
    Absent,
    #[default]
    Unknown,
}

/// pi's full derivation (`subagent-runner.ts:1442-1447`), ported branch for branch.
///
/// Ordering is load-bearing and is upstream's: structured output or non-blank text ⇒ `Present`
/// FIRST, so a run that both saved a file and returned prose reports `Present` rather than
/// `Unknown`; only then does a saved path yield `Unknown` (the text exists, it just is not in
/// hand); otherwise `Absent`.
///
/// The simpler two-branch form upstream also uses (`run-child-session.ts:549`,
/// `subagent-runner.ts:927`) is this function with the last two arguments `None`, so there is one
/// implementation rather than two that can drift.
#[must_use]
pub fn derive_output_state(
    final_output: Option<&str>,
    structured_output: Option<&serde_json::Value>,
    saved_output_path: Option<&str>,
) -> SubagentOutputState {
    if structured_output.is_some() || final_output.is_some_and(|o| !o.trim().is_empty()) {
        return SubagentOutputState::Present;
    }
    if saved_output_path.is_some() {
        return SubagentOutputState::Unknown;
    }
    SubagentOutputState::Absent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_blank_text_is_present() {
        assert_eq!(
            derive_output_state(Some("answer"), None, None),
            SubagentOutputState::Present
        );
    }

    #[test]
    fn structured_output_is_present_even_with_blank_text() {
        assert_eq!(
            derive_output_state(Some("  \n"), Some(&serde_json::json!({"k": 1})), None),
            SubagentOutputState::Present
        );
    }

    #[test]
    fn present_wins_over_a_saved_path() {
        // Upstream's ordering: a run that both saved a file and returned prose reports `Present`.
        assert_eq!(
            derive_output_state(Some("prose"), None, Some("/tmp/out.md")),
            SubagentOutputState::Present
        );
    }

    #[test]
    fn a_saved_path_alone_is_unknown_not_absent() {
        assert_eq!(
            derive_output_state(None, None, Some("/tmp/out.md")),
            SubagentOutputState::Unknown
        );
        assert_eq!(
            derive_output_state(Some("   "), None, Some("/tmp/out.md")),
            SubagentOutputState::Unknown
        );
    }

    #[test]
    fn nothing_at_all_is_absent() {
        assert_eq!(
            derive_output_state(None, None, None),
            SubagentOutputState::Absent
        );
        assert_eq!(
            derive_output_state(Some(""), None, None),
            SubagentOutputState::Absent
        );
    }

    #[test]
    fn serializes_lowercase_and_defaults_to_unknown() {
        assert_eq!(
            serde_json::to_string(&SubagentOutputState::Present).unwrap_or_default(),
            "\"present\""
        );
        let decoded: SubagentOutputState = serde_json::from_str("\"absent\"").unwrap_or_default();
        assert_eq!(decoded, SubagentOutputState::Absent);
        assert_eq!(SubagentOutputState::default(), SubagentOutputState::Unknown);
    }
}
