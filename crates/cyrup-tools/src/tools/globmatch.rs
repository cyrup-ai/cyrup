//! Glob semantics for the `find` and `grep` tools (R-03-033, arch-03 §6.7).
//!
//! The two tools do **not** share a rule, because Pi hands each pattern to a different binary:
//!
//! * [`PatternMatcher`] reproduces **fd**'s rule, used by `find` only (find.ts:243-252).
//! * [`RgGlob`] reproduces **ripgrep**'s `--glob` override rule, used by `grep` only
//!   (grep.ts:218 `if (glob) args.push("--glob", glob);` — verbatim pass-through to `rg`).
//!
//! The two rules are near-inverses of one another for the `**/`-prefix question, so mixing them up
//! un-anchors exactly the patterns the other anchors. Keep them apart.

use crate::error;
use cyrup_core::ToolError;
use globset::{Glob, GlobBuilder, GlobMatcher};

/// fd's rule: a pattern without `/` matches basenames; a pattern with `/` enables full-path
/// matching with an auto-prepended `**/` (unless it starts with `/` or `**/`, or is exactly `**`).
///
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

/// ripgrep's rule for a single `--glob` argument.
///
/// Pi's `grep` passes `glob` to real ripgrep untouched (grep.ts:218), so the pattern is parsed as
/// one gitignore-style *override* line. This is a 1:1 port of that parse — `ignore-0.4.33`
/// `src/gitignore.rs:460-539` (`GitignoreBuilder::add_line`) plus the single-glob reduction of
/// `src/overrides.rs:97-110` (`Override::matched`):
///
/// * a leading `#` (or an empty/whitespace-only line) is a comment — no glob is added, so the
///   override set is empty and **every** file passes;
/// * a leading `!` inverts the match (rg's whitelist override): with one negated glob the file is
///   kept iff it does *not* match. `\!`/`\#` escape a literal leading `!`/`#`;
/// * a leading `/` is stripped and anchors the glob to the root;
/// * a trailing `/` means directories only, so no file ever matches;
/// * `**/` is prepended **only when the pattern contains no `/` at all** and is not already
///   `**`-prefixed — the exact opposite of fd's rule above;
/// * a trailing `/**` gains a `/*` so it matches the contents of a directory, not the directory;
/// * the result always compiles with `literal_separator(true)` and always matches the *full* path
///   relative to the override root — never the basename. (`**/foo` covers the basename case.)
pub struct RgGlob {
    matcher: GlobMatcher,
    /// `!`-prefixed: the sense of the match is inverted.
    negated: bool,
    /// Trailing `/`: only directories match, so a file never does.
    only_dir: bool,
}

impl RgGlob {
    /// Compile one `--glob` argument. `Ok(None)` means "no filter at all" — ripgrep's empty
    /// override set, which matches nothing and therefore excludes nothing.
    pub fn build(pattern: &str) -> Result<Option<Self>, ToolError> {
        // gitignore.rs:466-471: `#` comments out the line, and trailing whitespace is trimmed
        // unless it was escaped as `\ `.
        let mut line = pattern;
        if line.starts_with('#') {
            return Ok(None);
        }
        if !line.ends_with("\\ ") {
            line = line.trim_end();
        }
        if line.is_empty() {
            return Ok(None);
        }

        // gitignore.rs:483-499.
        let mut negated = false;
        let mut is_absolute = false;
        if line.starts_with("\\!") || line.starts_with("\\#") {
            line = line.get(1..).unwrap_or_default();
            is_absolute = line.starts_with('/');
        } else {
            if let Some(rest) = line.strip_prefix('!') {
                negated = true;
                line = rest;
            }
            if let Some(rest) = line.strip_prefix('/') {
                line = rest;
                is_absolute = true;
            }
        }

        // gitignore.rs:501-511: a trailing `/` restricts the glob to directories; an escaped
        // trailing slash (`\/`) drops the escape too.
        let mut only_dir = false;
        if let Some(rest) = line.strip_suffix('/') {
            only_dir = true;
            line = rest.strip_suffix('\\').unwrap_or(rest);
        }

        let mut actual = line.to_string();
        // gitignore.rs:513-522. Note the condition: `**/` is added when the glob has NO `/`.
        if !is_absolute && !actual.contains('/') && !(actual.starts_with("**/") || actual == "**") {
            actual = format!("**/{actual}");
        }
        // gitignore.rs:524-527.
        if actual.ends_with("/**") {
            actual = format!("{actual}/*");
        }

        // gitignore.rs:528-536, with `allow_unclosed_class(false)` from overrides.rs:126.
        let glob: Glob = GlobBuilder::new(&actual)
            .literal_separator(true)
            .backslash_escape(true)
            .allow_unclosed_class(false)
            .build()
            .map_err(|e| error::invalid(format!("invalid glob '{pattern}': {e}")))?;
        Ok(Some(Self { matcher: glob.compile_matcher(), negated, only_dir }))
    }

    /// Whether a **file** survives this filter. `rel_posix` is the candidate path relative to the
    /// override root — for ripgrep that root is its own cwd, not the search path
    /// (gitignore.rs:286-315 `strip`), so `grep` must strip the tool's cwd, not its search root.
    pub fn keeps_file(&self, rel_posix: &str) -> bool {
        // overrides.rs:105-109: a whitelist hit inverts to Ignore (dropped); a miss with at least
        // one non-whitelist glob present inverts to Ignore as well (dropped).
        let hit = !self.only_dir && self.matcher.is_match(rel_posix);
        hit != self.negated
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::{PatternMatcher, RgGlob};

    fn keeps(pattern: &str, rel: &str) -> bool {
        RgGlob::build(pattern).unwrap().expect("glob").keeps_file(rel)
    }

    /// The defect this type exists to fix: fd's rule prepends `**/` to a path-containing pattern,
    /// which un-anchors it. ripgrep prepends `**/` only to a pattern with NO `/`, so a
    /// path-containing pattern stays anchored at the override root
    /// (ignore-0.4.33 gitignore.rs:513-522).
    #[test]
    fn path_glob_is_anchored_unlike_fds_rule() {
        assert!(keeps("src/**/*.ts", "src/a.ts"));
        assert!(keeps("src/**/*.ts", "src/deep/a.ts"));
        assert!(!keeps("src/**/*.ts", "vendor/src/a.ts"));
        assert!(!keeps("src/**/*.ts", "third_party/src/b.ts"));

        // fd's rule (still correct for `find`) does the opposite, and that is exactly why `grep`
        // must not share it.
        let fd = PatternMatcher::build("src/**/*.ts").unwrap();
        assert!(fd.is_match("vendor/src/a.ts", "a.ts"));
    }

    /// gitignore.rs:492-498: a leading `/` is STRIPPED and anchors the glob. Handing it to globset
    /// verbatim instead matches a relative candidate path that can never start with `/`, so the
    /// pattern selects nothing at all.
    #[test]
    fn leading_slash_is_stripped_and_anchors() {
        assert!(keeps("/src/*.ts", "src/a.ts"));
        assert!(!keeps("/src/*.ts", "vendor/src/a.ts"));
        assert!(!keeps("/src/*.ts", "src/deep/a.ts"));
    }

    /// gitignore.rs:513-522: a pattern with no `/` gets the `**/` prefix and so matches a basename
    /// at any depth — the behavior `*.ts` already had, kept as a regression guard.
    #[test]
    fn bare_pattern_matches_basename_at_any_depth() {
        assert!(keeps("*.ts", "a.ts"));
        assert!(keeps("*.ts", "src/deep/a.ts"));
        assert!(!keeps("*.ts", "src/a.js"));
        // `**` is already double-star prefixed, so it is not rewritten and matches everything.
        assert!(keeps("**", "src/a.ts"));
    }

    /// overrides.rs:105-109 reduced to one glob: a `!`-prefixed override keeps everything it does
    /// NOT match. `\!` escapes a literal leading `!`.
    #[test]
    fn negation_inverts_and_backslash_escapes_it() {
        assert!(!keeps("!*.ts", "a.ts"));
        assert!(keeps("!*.ts", "a.rs"));
        assert!(keeps("\\!important.ts", "!important.ts"));
        assert!(!keeps("\\!important.ts", "important.ts"));
    }

    /// gitignore.rs:466-471: a `#` or empty line adds no glob, leaving an empty override set that
    /// filters nothing.
    #[test]
    fn comment_or_empty_pattern_is_no_filter() {
        assert!(RgGlob::build("#nope").unwrap().is_none());
        assert!(RgGlob::build("   ").unwrap().is_none());
    }

    /// gitignore.rs:501-505 (`is_only_dir`) and :524-527 (the `/**` → `/**/*` fixup).
    #[test]
    fn trailing_slash_is_directory_only_and_slash_doublestar_takes_contents() {
        assert!(!keeps("src/", "src/a.ts"));
        assert!(keeps("src/**", "src/a.ts"));
        assert!(keeps("src/**", "src/deep/a.ts"));
        assert!(!keeps("src/**", "src"));
    }
}
