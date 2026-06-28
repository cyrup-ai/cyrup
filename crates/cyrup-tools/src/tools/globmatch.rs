//! Shared glob semantics for `find` and `grep` (R-03-033, arch-03 §6.7).
//!
//! A pattern without `/` matches basenames; a pattern with `/` enables full-path matching with an
//! auto-prepended `**/` (unless it starts with `/` or `**/`, or is exactly `**`).

use crate::error;
use cyrup_core::ToolError;
use globset::{Glob, GlobBuilder, GlobMatcher};

/// A compiled pattern + whether it matches against the full relative path (vs the basename).
pub struct PatternMatcher {
    pub matcher: GlobMatcher,
    pub full_path: bool,
}

impl PatternMatcher {
    pub fn build(pattern: &str) -> Result<Self, ToolError> {
        let full_path = pattern.contains('/');
        let effective = if full_path
            && !pattern.starts_with('/')
            && !pattern.starts_with("**/")
            && pattern != "**"
        {
            format!("**/{pattern}")
        } else {
            pattern.to_string()
        };
        let glob: Glob = GlobBuilder::new(&effective)
            .literal_separator(full_path)
            .build()
            .map_err(|e| error::invalid(format!("invalid glob '{pattern}': {e}")))?;
        Ok(Self { matcher: glob.compile_matcher(), full_path })
    }

    /// Test a candidate. `rel_posix` is the path relative to the search root (posix separators);
    /// `basename` is its final component.
    pub fn is_match(&self, rel_posix: &str, basename: &str) -> bool {
        if self.full_path {
            self.matcher.is_match(rel_posix)
        } else {
            self.matcher.is_match(basename)
        }
    }
}

/// Convert a path to a posix-separated string.
pub fn to_posix(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '/' {
        s.into_owned()
    } else {
        s.replace(std::path::MAIN_SEPARATOR, "/")
    }
}
