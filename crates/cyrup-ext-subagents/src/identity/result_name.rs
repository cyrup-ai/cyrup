//! [`ResultFileName`] — a results-directory-relative payload file name.
//!
//! Ports the guard pi repeats verbatim at `result-files.ts:260`, `:294` and `:307`.

use std::path::{Path, PathBuf};

use crate::background::RunId;

/// A results-directory-relative payload file name: exactly ONE path component, `.json` suffix.
///
/// # This is a trust boundary
///
/// [`crate::background::result_index::ResultIndexEntry::file`] is deserialized from a JSON file on
/// disk — written by a *different* process, possibly an older or newer build — and is then joined
/// onto the results directory. Without a guard, a `file` of `"../../../etc/passwd.json"` walks out
/// of the results root.
///
/// pi guards it, correctly, at three separate call sites:
///
/// ```text
/// result-files.ts:260   promotePendingResultFile            -> return "none"
/// result-files.ts:294   pendingResultLocationForSessionRun  -> return undefined
/// result-files.ts:307   resultPayloadLocationFromIndex      -> return undefined   (entry.file)
/// ```
///
/// Three copies of one rule is three chances for a fourth reader — added later, by someone who
/// did not know the rule existed — to omit it. Representing it once, *at parse*, removes the
/// possibility rather than the current occurrences: there is no way to construct a
/// `ResultFileName` that fails the check, and [`ResultFileName::resolve_in`] is the only way to
/// turn one into a path.
///
/// # Construction is closed
///
/// `Deserialize` routes through [`ResultFileName::parse`]. `#[serde(transparent)]` on the read
/// side would reintroduce exactly the traversal this type exists to prevent, at exactly the point
/// where the data is least trustworthy. `Serialize` *is* transparent — writing is not a trust
/// boundary, and the on-disk shape must stay a bare JSON string.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(transparent)]
pub struct ResultFileName(String);

impl ResultFileName {
    /// The `.json` suffix every payload file carries. pi `JSON_EXTENSION` (`result-files.ts:15`).
    pub const EXTENSION: &'static str = ".json";

    /// Infallible: a [`RunId`] is a hex token and therefore always a valid stem.
    ///
    /// pi `resultFileName` (`result-files.ts:48-50`).
    #[must_use]
    pub fn for_run(run_id: &RunId) -> Self {
        Self(format!("{}{}", run_id.as_str(), Self::EXTENSION))
    }

    /// The only fallible entry point. Rejects anything that is not a single `.json` component.
    ///
    /// Specifically: empty, `.`, `..`, any value containing a path separator (`/`, and `\` too —
    /// the on-disk format is shared with a Windows-capable implementation, so a backslash must be
    /// rejected here even though Unix would treat it as an ordinary character), any value whose
    /// [`Path::file_name`] differs from itself (pi's `file !== path.basename(file)`), and any
    /// value not ending in `.json`.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.is_empty() || !raw.ends_with(Self::EXTENSION) {
            return None;
        }
        if raw.contains('/') || raw.contains('\\') || raw.contains('\0') {
            return None;
        }
        // pi's `file !== path.basename(file)`. Also rejects `.` and `..`, whose `file_name()` is
        // `None`, and any platform-specific component form the checks above missed.
        if Path::new(raw).file_name().and_then(std::ffi::OsStr::to_str) != Some(raw) {
            return None;
        }
        Some(Self(raw.to_string()))
    }

    /// The stem — the file name with its `.json` suffix removed. pi's
    /// `path.basename(resultPath, ".json")` fallback for deriving a run id (`result-files.ts:130`).
    #[must_use]
    pub fn stem(&self) -> &str {
        self.0.strip_suffix(Self::EXTENSION).unwrap_or(&self.0)
    }

    /// Borrows the file name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The ONLY way to turn this into a path.
    ///
    /// Takes the directory it is relative to, so a caller cannot accidentally resolve it against
    /// the wrong root — and because the value is a validated single component, the result is
    /// always a direct child of `results_dir`.
    #[must_use]
    pub fn resolve_in(&self, results_dir: &Path) -> PathBuf {
        results_dir.join(&self.0)
    }
}

impl std::fmt::Display for ResultFileName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Deserializes **through** [`ResultFileName::parse`] — see the type docs for why the read side
/// cannot be transparent.
impl<'de> serde::Deserialize<'de> for ResultFileName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| {
            serde::de::Error::custom(
                "result file name must be a single path component ending in .json",
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
    fn for_run_builds_the_public_payload_name() {
        let run_id = RunId::from_token("abc123");
        assert_eq!(ResultFileName::for_run(&run_id).as_str(), "abc123.json");
    }

    #[test]
    fn parse_accepts_a_plain_json_component() {
        assert_eq!(ResultFileName::parse("abc123.json").map(|f| f.as_str().to_string()), Some("abc123.json".to_string()));
    }

    // --- the traversal cases upstream re-checks at three call sites ------------------------

    #[test]
    fn parse_rejects_parent_traversal() {
        assert!(ResultFileName::parse("../x.json").is_none());
        assert!(ResultFileName::parse("../../etc/passwd.json").is_none());
        assert!(ResultFileName::parse("..").is_none());
    }

    #[test]
    fn parse_rejects_any_separator() {
        assert!(ResultFileName::parse("a/b.json").is_none());
        assert!(ResultFileName::parse("/abs.json").is_none());
        assert!(ResultFileName::parse("a\\b.json").is_none());
        assert!(ResultFileName::parse("dir/").is_none());
    }

    #[test]
    fn parse_rejects_dot_and_empty() {
        assert!(ResultFileName::parse(".").is_none());
        assert!(ResultFileName::parse("").is_none());
    }

    #[test]
    fn parse_rejects_a_non_json_suffix() {
        assert!(ResultFileName::parse("x.txt").is_none());
        assert!(ResultFileName::parse("x").is_none());
        assert!(ResultFileName::parse("x.json.txt").is_none());
    }

    #[test]
    fn parse_rejects_an_embedded_nul() {
        assert!(ResultFileName::parse("a\0b.json").is_none());
    }

    #[test]
    fn parse_accepts_a_bare_dotfile_that_still_ends_in_json() {
        // `.json` itself is a legal single component; `file_name()` is `Some(".json")`.
        assert!(ResultFileName::parse(".json").is_some());
    }

    // --- resolution ------------------------------------------------------------------------

    #[test]
    fn resolve_in_always_yields_a_direct_child() {
        let name = ResultFileName::parse("abc.json").expect("valid");
        let resolved = name.resolve_in(Path::new("/tmp/results"));
        assert_eq!(resolved, PathBuf::from("/tmp/results/abc.json"));
        assert_eq!(resolved.parent(), Some(Path::new("/tmp/results")));
    }

    #[test]
    fn stem_strips_the_extension() {
        let name = ResultFileName::parse("abc123.json").expect("valid");
        assert_eq!(name.stem(), "abc123");
    }

    // --- serde is a construction path -------------------------------------------------------

    #[test]
    fn deserialize_rejects_traversal_so_an_on_disk_entry_cannot_escape() {
        // The whole point: this value arrives from a file another process wrote.
        assert!(serde_json::from_str::<ResultFileName>("\"../../etc/passwd.json\"").is_err());
        assert!(serde_json::from_str::<ResultFileName>("\"a/b.json\"").is_err());
        assert!(serde_json::from_str::<ResultFileName>("\"x.txt\"").is_err());
    }

    #[test]
    fn deserialize_accepts_a_valid_component_and_serialize_stays_transparent() {
        let parsed: ResultFileName = serde_json::from_str("\"abc.json\"").expect("valid");
        assert_eq!(parsed.as_str(), "abc.json");
        assert_eq!(serde_json::to_string(&parsed).expect("ser"), "\"abc.json\"");
    }

    #[test]
    fn a_struct_field_carrying_a_traversal_fails_to_deserialize_as_a_whole() {
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct Entry {
            file: ResultFileName,
        }
        assert!(serde_json::from_str::<Entry>("{\"file\":\"../x.json\"}").is_err());
        assert!(serde_json::from_str::<Entry>("{\"file\":\"ok.json\"}").is_ok());
    }
}
