//! [`WorkflowKey`] — the ONE declaration of the workflow key grammar (SCOPE_3 §A.3).

/// A workflow lane / resource / receipt / host-grant key — pi's key pattern constant
/// (`scripted-workflow.ts:12` and ten identical copies across the upstream tree; SCOPE_3 §A.3).
///
/// Grammar: `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$` — first char ASCII alphanumeric, then 0..=127 of
/// ASCII alphanumerics, `.`, `_` or `-`, so 1..=128 characters (equivalently bytes: the alphabet
/// is pure ASCII) total. Parsed ONCE at the boundary; every downstream signature takes
/// `&WorkflowKey` and performs no check, because there is no way to obtain one that has not been
/// checked.
///
/// No `regex` dependency: the grammar is a first-char test plus a `chars().all(..)` over a bounded
/// count. No `Deserialize` derive — a derived impl would bypass [`WorkflowKey::parse`] and
/// reintroduce every one of upstream's eleven validation sites; the hand-written impl below is the
/// only read path (SCOPE_3d §0.9, matching [`crate::identity::SessionId`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct WorkflowKey(String);

impl WorkflowKey {
    /// The ONLY fallible constructor.
    ///
    /// # Errors
    ///
    /// [`WorkflowKeyError`] when `value` does not match the grammar. The error carries nothing —
    /// the grammar has one failure mode from a caller's perspective, and upstream uses three
    /// different wordings for the same rejection (`"workflowChildren childId is invalid."`,
    /// `"Workflow definition requires a safe resource name."`, `"workflow must use a safe
    /// resource name."`), so the parser owns the grammar and each call site owns its wording.
    pub fn parse(value: &str) -> Result<Self, WorkflowKeyError> {
        let mut chars = value.chars();
        let Some(first) = chars.next() else {
            return Err(WorkflowKeyError);
        };
        if !first.is_ascii_alphanumeric() {
            return Err(WorkflowKeyError);
        }
        let mut tail = 0usize;
        for c in chars {
            tail += 1;
            if tail > 127 || !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
                return Err(WorkflowKeyError);
            }
        }
        Ok(Self(value.to_string()))
    }

    /// Borrows the key for comparison, display and serialization boundaries.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// pi `` key.startsWith(`${root}.`) `` — the lane-root relation. A property OF the grammar,
    /// so it lives with it rather than being re-derived in `preflight.rs` (SCOPE_3e) and
    /// `child_summary.rs` (SCOPE_3d) separately. `workflowPreflightLaneForRuntimeKey`
    /// (`workflow-preflight.ts:165`) wants the LONGEST such root; that selection is the caller's,
    /// this is the predicate.
    #[must_use]
    pub fn is_descendant_of(&self, root: &WorkflowKey) -> bool {
        self.0
            .strip_prefix(root.as_str())
            .is_some_and(|rest| rest.starts_with('.'))
    }
}

impl std::fmt::Display for WorkflowKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Deserializes THROUGH [`WorkflowKey::parse`] — SCOPE_3d §0.9. NOT a derive, and NOT a
/// per-field serde attribute: this is the crate's established idiom (`identity/session_id.rs`,
/// `identity/result_name.rs`, and eleven more hand-written impls), and it is the only form with
/// no bypass — a new deserialized field cannot forget an attribute that does not exist.
impl<'de> serde::Deserialize<'de> for WorkflowKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// The single failure mode of [`WorkflowKey::parse`]: the value does not match the grammar.
///
/// Deliberately carries nothing (see [`WorkflowKey::parse`]'s doc) — each call site maps it to
/// its own upstream message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("workflow key must match ^[A-Za-z0-9][A-Za-z0-9._-]{{0,127}}$")]
pub struct WorkflowKeyError;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// pi's key pattern: first char alphanumeric, tail of alphanumerics/`.`/`_`/`-`, 1..=128
    /// chars.
    #[test]
    fn parse_accepts_the_grammar_and_nothing_else() {
        for ok in ["a", "A9", "lane.stage-1_x", "0", &"a".repeat(128)] {
            assert!(WorkflowKey::parse(ok).is_ok(), "{ok:?} should parse");
        }
        for bad in [
            "",
            ".a", // first char must be alphanumeric
            "-a",
            "_a",
            "a b", // space is outside the alphabet
            "a/b",
            "ключ", // non-ASCII
            &"a".repeat(129),
        ] {
            assert!(
                WorkflowKey::parse(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    /// `key.startsWith(`root.`)` — strict descendant, never reflexive, and never a plain prefix
    /// without the dot.
    #[test]
    fn is_descendant_of_is_the_dotted_prefix_relation() {
        let root = WorkflowKey::parse("lane").expect("valid");
        let child = WorkflowKey::parse("lane.stage").expect("valid");
        let sibling = WorkflowKey::parse("lane2").expect("valid");
        assert!(child.is_descendant_of(&root));
        assert!(!root.is_descendant_of(&root), "not reflexive");
        assert!(
            !sibling.is_descendant_of(&root),
            "prefix without dot is not descent"
        );
        assert!(!root.is_descendant_of(&child));
    }

    /// §0.9: serde reads route through `parse`; writes stay transparent.
    #[test]
    fn deserialize_goes_through_parse() {
        let ok: WorkflowKey = serde_json::from_str("\"lane.1\"").expect("valid key deserializes");
        assert_eq!(ok.as_str(), "lane.1");
        assert!(serde_json::from_str::<WorkflowKey>("\".bad\"").is_err());
        assert!(serde_json::from_str::<WorkflowKey>("\"\"").is_err());
        assert_eq!(
            serde_json::to_string(&ok).expect("serializes"),
            "\"lane.1\""
        );
    }
}
