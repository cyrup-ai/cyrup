//! [`RunDirName`] — a validated path component naming a run directory.
//!
//! Ports pi's `assertSafeRunId` (`workflows/workflow-receipt.ts:19-25`), the guard standing
//! between a workflow receipt's path and a traversal.

use std::path::{Path, PathBuf};

use crate::background::RunId;

/// One validated path component naming a run directory — pi `assertSafeRunId`
/// (`workflow-receipt.ts:19-25`).
///
/// Sibling of [`crate::identity::ResultFileName`], and for the same reason:
/// [`crate::background::RunDir::new`] is a bare `join`, and the run id reaching it can come from
/// a workflow script's own `resolveResume` argument (`WorkflowReceiptResumeReference.workflow_run_id`
/// is a plain `String`, model-authored). `parse` rejects empty/blank, `.`, `..`, anything
/// containing `/`, `\` or NUL, and anything whose [`Path::file_name`] differs from itself (pi's
/// `path.basename(v) !== v`).
///
/// `parse` TRIMS first, as upstream does (`:20`), and stores the trimmed value — the trimmed form
/// is what names the directory.
///
/// # This is a trust boundary
///
/// Without this guard, a script asking to resume `"../../../../etc"` would walk
/// [`crate::workflows::workflow_receipt_path`] out of the async root. `parse` is the ONLY
/// fallible constructor and [`RunDirName::resolve_in`] is the ONLY way to turn one into a path —
/// there is no way to construct a value that fails the check.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct RunDirName(String);

impl RunDirName {
    /// The only fallible constructor. Rejects empty/blank (after trim), `.`, `..`, any value
    /// containing `/`, `\` or NUL, and any value whose [`Path::file_name`] differs from itself.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
            return None;
        }
        if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains('\0') {
            return None;
        }
        if Path::new(trimmed)
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            != Some(trimmed)
        {
            return None;
        }
        Some(Self(trimmed.to_string()))
    }

    /// Infallible: a [`RunId`] minted by [`RunId::new`] is a hex token, which always passes
    /// [`RunDirName::parse`].
    #[must_use]
    pub fn for_run(run_id: &RunId) -> Self {
        Self(run_id.as_str().to_string())
    }

    /// Borrows the run-directory-naming token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The ONLY way to turn this into a path.
    #[must_use]
    pub fn resolve_in(&self, async_root: &Path) -> PathBuf {
        async_root.join(&self.0)
    }
}

impl std::fmt::Display for RunDirName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Deserializes THROUGH [`RunDirName::parse`] — see the type docs for why the read side cannot be
/// transparent (`identity/`'s discipline).
impl<'de> serde::Deserialize<'de> for RunDirName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| {
            serde::de::Error::custom(
                "run directory name must be an exact workflow run id, not a path or prefix",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    #[test]
    fn for_run_wraps_a_minted_run_id() {
        let run_id = RunId::from_token("abc12345");
        assert_eq!(RunDirName::for_run(&run_id).as_str(), "abc12345");
    }

    #[test]
    fn parse_accepts_a_plain_token() {
        assert_eq!(
            RunDirName::parse("abc12345").map(|n| n.as_str().to_string()),
            Some("abc12345".to_string())
        );
    }

    #[test]
    fn parse_trims_and_stores_the_trimmed_value() {
        let name = RunDirName::parse("  abc12345  ").expect("trims to a valid token");
        assert_eq!(name.as_str(), "abc12345");
    }

    #[test]
    fn parse_rejects_parent_traversal() {
        assert!(RunDirName::parse("../../../../etc").is_none());
        assert!(RunDirName::parse("..").is_none());
        assert!(RunDirName::parse(".").is_none());
    }

    #[test]
    fn parse_rejects_any_separator() {
        assert!(RunDirName::parse("a/b").is_none());
        assert!(RunDirName::parse("/abs").is_none());
        assert!(RunDirName::parse("a\\b").is_none());
    }

    #[test]
    fn parse_rejects_blank_and_embedded_nul() {
        assert!(RunDirName::parse("").is_none());
        assert!(RunDirName::parse("   ").is_none());
        assert!(RunDirName::parse("a\0b").is_none());
    }

    #[test]
    fn resolve_in_always_yields_a_direct_child() {
        let name = RunDirName::parse("abc12345").expect("valid");
        let resolved = name.resolve_in(Path::new("/var/tmp/cyrup-subagents"));
        assert_eq!(
            resolved,
            PathBuf::from("/var/tmp/cyrup-subagents/abc12345")
        );
    }

    #[test]
    fn deserialize_rejects_traversal_so_a_model_authored_value_cannot_escape() {
        assert!(serde_json::from_str::<RunDirName>("\"../../../../etc\"").is_err());
        assert!(serde_json::from_str::<RunDirName>("\"a/b\"").is_err());
    }

    #[test]
    fn deserialize_accepts_a_valid_token_and_serialize_stays_transparent() {
        let parsed: RunDirName = serde_json::from_str("\"abc12345\"").expect("valid");
        assert_eq!(parsed.as_str(), "abc12345");
        assert_eq!(
            serde_json::to_string(&parsed).expect("ser"),
            "\"abc12345\""
        );
    }
}
