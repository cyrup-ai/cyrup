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

/// fd's smart case, reproduced for `find` (fd v10.3.0 `src/main.rs:195-202`): the search is
/// case-sensitive **iff** the pattern carries an uppercase character, and case-INSENSITIVE
/// otherwise. pi passes neither `-s/--case-sensitive` nor `-i/--ignore-case` (find.ts:235-267)
/// and its schema has no case parameter (find.ts:29-35), so fd's default is the whole rule.
///
/// fd runs this over the *regex* globset emits for the glob (main.rs:169-172, then
/// `regex_helper::pattern_has_uppercase_char`, which counts uppercase only in HIR literals and
/// class range endpoints). Scanning the glob string itself is equivalent: globset's emitter
/// (`globset-0.4.18` glob.rs:673-790) introduces no uppercase letter — non-ASCII bytes become
/// lowercase `\xNN` escapes — and no glob metacharacter is an uppercase letter, so the uppercase
/// characters of the glob and of its regex are the same set.
///
/// `char::is_uppercase` (Unicode), NOT `is_ascii_uppercase`: fd's check is Unicode-aware.
fn pattern_has_uppercase_char(pattern: &str) -> bool {
    pattern.chars().any(char::is_uppercase)
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
        // fd's smart case. The verdict is taken on `effective` — the string fd itself receives,
        // after pi prepends `**/` (find.ts:257-262) — not on the raw `pattern`. `**/` holds no
        // uppercase character, so the two agree on every input; `effective` is simply the literal
        // equivalent of fd's own input.
        let glob: Glob = GlobBuilder::new(&effective)
            .literal_separator(full_path)
            .case_insensitive(!pattern_has_uppercase_char(&effective))
            .build()
            .map_err(|e| error::invalid(format!("invalid glob '{pattern}': {e}")))?;
        Ok(Self {
            matcher: glob.compile_matcher(),
            full_path,
        })
    }

    /// Test a candidate. In full-path mode `path_posix` must be the **ABSOLUTE** candidate path in
    /// posix form, because that is what fd tests: pi's own in-source note at find.ts:254-256 says
    /// `--full-path` "matches against the absolute candidate path", and find.ts:267
    /// `args.push("--", effectivePattern, searchPath)` hands fd the absolute search path as its
    /// root. `find` relativizes only for OUTPUT (find.ts:321-326).
    ///
    /// Passing a search-root-RELATIVE path here made the `pattern.starts_with('/')` arm in
    /// [`PatternMatcher::build`] dead — a relative posix path can never begin with `/`, so a
    /// leading-slash pattern selected nothing at all rather than anchoring at the filesystem root
    /// the way fd does. `basename` is the candidate's final component (fd's non-`--full-path`
    /// mode), unchanged.
    pub fn is_match(&self, path_posix: &str, basename: &str) -> bool {
        if self.full_path {
            self.matcher.is_match(path_posix)
        } else {
            self.matcher.is_match(basename)
        }
    }
}

/// ripgrep's rule for a single `--glob` argument.
///
/// Pi's `grep` passes `glob` to real ripgrep untouched (grep.ts:223), so the pattern is parsed as
/// one gitignore-style *override* line. This is a 1:1 port of that parse — `ignore-0.4.26`
/// `src/gitignore.rs:447-528` (`GitignoreBuilder::add_line`) plus the single-glob reduction of
/// `src/overrides.rs:97-110` (`Override::matched`):
///
/// * a leading `#` (or an empty/whitespace-only line) is a comment — no glob is added, so the
///   override set is empty and **every** file passes;
/// * a leading `!` inverts the match (rg's whitelist override): with one negated glob the file is
///   kept iff it does *not* match. `\!`/`\#` escape a literal leading `!`/`#`;
/// * a leading `/` is stripped and anchors the glob to the root;
/// * a trailing `/` restricts the glob to DIRECTORIES: no file ever matches it, and a directory
///   matches it exactly as it would without the slash (gitignore.rs:262);
/// * `**/` is prepended **only when the pattern contains no `/` at all** and is not already
///   `**`-prefixed — the exact opposite of fd's rule above;
/// * a trailing `/**` gains a `/*` so it matches the contents of a directory, not the directory;
/// * the result always compiles with `literal_separator(true)` and always matches the *full* path
///   relative to the override root — never the basename. (`**/foo` covers the basename case.)
///
/// The override applies to **directories as well as files**, which is what makes `!node_modules`
/// remove the whole subtree rather than nothing at all — see [`RgGlob::ignores`].
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
        // gitignore.rs:454-462: `#` comments out the line, and trailing whitespace is trimmed
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

        // gitignore.rs:470-487.
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

        // gitignore.rs:488-498: a trailing `/` restricts the glob to directories; an escaped
        // trailing slash (`\/`) drops the escape too.
        let mut only_dir = false;
        if let Some(rest) = line.strip_suffix('/') {
            only_dir = true;
            line = rest.strip_suffix('\\').unwrap_or(rest);
        }

        let mut actual = line.to_string();
        // gitignore.rs:500-508. Note the condition: `**/` is added when the glob has NO `/`.
        if !is_absolute && !actual.contains('/') && !(actual.starts_with("**/") || actual == "**") {
            actual = format!("**/{actual}");
        }
        // gitignore.rs:509-514.
        if actual.ends_with("/**") {
            actual = format!("{actual}/*");
        }

        // gitignore.rs:515-524, with `allow_unclosed_class(false)` from overrides.rs:125.
        let glob: Glob = GlobBuilder::new(&actual)
            .literal_separator(true)
            .backslash_escape(true)
            .allow_unclosed_class(false)
            .build()
            .map_err(|e| error::invalid(format!("invalid glob '{pattern}': {e}")))?;
        Ok(Some(Self {
            matcher: glob.compile_matcher(),
            negated,
            only_dir,
        }))
    }

    /// `Override::matched(path, is_dir)` (ignore-0.4.26 overrides.rs:97-110) reduced to this
    /// crate's single compiled glob, returning `true` when the override says **IGNORE**.
    ///
    /// `is_dir` is the parameter the original port dropped, and it is the whole difference between
    /// `!node_modules` filtering nothing and `!node_modules` removing the subtree.
    ///
    /// `rel_posix` is the candidate path relative to the override root — for ripgrep that root is
    /// its own cwd, not the search path (gitignore.rs:275-304 `strip`), so `grep` must strip the
    /// tool's cwd, not its search root.
    fn ignores(&self, rel_posix: &str, is_dir: bool) -> bool {
        // gitignore.rs:262 (`matched_stripped`, gitignore.rs:248-271): a glob with a trailing `/`
        // is passed over unless the candidate IS a directory.
        let hit = (is_dir || !self.only_dir) && self.matcher.is_match(rel_posix);
        if hit {
            // overrides.rs:105 `.invert()`: in an override set a `!` glob is stored as a gitignore
            // *whitelist*, so a hit on it inverts to Ignore; a plain glob's hit inverts to
            // Whitelist and is kept.
            return self.negated;
        }
        // overrides.rs:106-108: a miss falls through to Ignore only when at least one plain
        // (non-`!`) glob exists AND the candidate is not a directory. That `!is_dir` guard is why
        // a whitelist miss never prunes a directory — rg still descends into it looking for files
        // the glob does match.
        !self.negated && !is_dir
    }

    /// Whether a **file** survives this filter.
    pub fn keeps_file(&self, rel_posix: &str) -> bool {
        !self.ignores(rel_posix, false)
    }

    /// Whether this directory must be PRUNED — dropped together with everything beneath it, the
    /// way `ignore`'s walker drops it via `skip_current_dir()` (walk.rs:1131-1145) when
    /// `should_skip_entry` (walk.rs:1939-1950) sees an Ignore verdict for a directory.
    pub fn prunes_dir(&self, rel_posix: &str) -> bool {
        self.ignores(rel_posix, true)
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
        RgGlob::build(pattern)
            .unwrap()
            .expect("glob")
            .keeps_file(rel)
    }

    fn prunes(pattern: &str, rel: &str) -> bool {
        RgGlob::build(pattern)
            .unwrap()
            .expect("glob")
            .prunes_dir(rel)
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
        assert!(fd.is_match("/repo/vendor/src/a.ts", "a.ts"));
    }

    /// TOOL-011 — the `pattern.starts_with('/')` arm of [`PatternMatcher::build`] is only
    /// reachable against an ABSOLUTE candidate, which is what fd tests in `--full-path` mode
    /// (find.ts:254-256). Against the search-root-relative path it was dead code and a
    /// leading-slash pattern matched nothing at all. RED before; GREEN after.
    #[test]
    fn fd_leading_slash_pattern_anchors_at_the_filesystem_root() {
        let m = PatternMatcher::build("/src/*.ts").unwrap();
        assert!(
            m.full_path,
            "a pattern containing `/` enables fd's --full-path mode"
        );
        assert!(m.is_match("/src/a.ts", "a.ts"));
        // Anchored: a `src/` nested anywhere else does NOT match, exactly as fd's absolute
        // full-path comparison behaves.
        assert!(!m.is_match("/repo/src/a.ts", "a.ts"));
    }

    /// The common case is unchanged by the absolute-candidate switch: both sides prepend `**/`, so
    /// a path-containing pattern still matches at any depth.
    #[test]
    fn fd_path_pattern_still_matches_at_any_depth_on_an_absolute_candidate() {
        let m = PatternMatcher::build("src/**/*.ts").unwrap();
        assert!(m.is_match("/tmp/repo/src/a.ts", "a.ts"));
        assert!(m.is_match("/tmp/repo/src/deep/a.ts", "a.ts"));
        assert!(!m.is_match("/tmp/repo/lib/a.ts", "a.ts"));
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

    /// gitignore.rs:488-492 (`is_only_dir`) and :509-514 (the `/**` → `/**/*` fixup).
    #[test]
    fn trailing_slash_is_directory_only_and_slash_doublestar_takes_contents() {
        assert!(!keeps("src/", "src/a.ts"));
        assert!(keeps("src/**", "src/a.ts"));
        assert!(keeps("src/**", "src/deep/a.ts"));
        assert!(!keeps("src/**", "src"));
    }

    /// The `is_dir` half of `Override::matched` (ignore-0.4.26 overrides.rs:97-110), which the
    /// original port dropped. The full truth table for one compiled glob:
    ///
    /// * a NEGATED glob that hits — file or directory — is Ignore, so the directory is pruned;
    /// * a negated glob that misses is not ignored at all;
    /// * a PLAIN glob that hits is Whitelist, so the directory is descended;
    /// * a plain glob that MISSES is Ignore for a file (the `num_whitelists() > 0` fallback) but
    ///   `Match::None` for a directory — overrides.rs:106 guards that fallback with `!is_dir`, so
    ///   a whitelist miss never prunes;
    /// * a trailing `/` (`is_only_dir`) is skipped unless the candidate IS a directory
    ///   (gitignore.rs:262), which is the only reason `!src/` prunes anything.
    #[test]
    fn a_directory_is_pruned_only_by_a_negated_hit() {
        assert!(prunes("!node_modules", "node_modules"));
        assert!(prunes("!node_modules", "vendor/node_modules"));
        assert!(!prunes("!node_modules", "src"));

        assert!(
            !prunes("*.js", "src"),
            "a whitelist MISS never prunes a directory"
        );
        assert!(!prunes("src/**/*.js", "vendor"));
        assert!(!prunes("src", "src"), "a whitelist HIT is kept, not pruned");

        assert!(prunes("!src/", "src"));
        assert!(prunes("!src/", "vendor/src"));
        // Verified against rg 14.1.0: with a regular FILE named `src` beside a `sub/x.txt`,
        // `rg --hidden --glob '!src/' -- NEEDLE .` prints BOTH. A directory-only glob excludes no
        // file at all — neither one named `src` nor one inside a `src` that was not pruned.
        assert!(
            keeps("!src/", "src"),
            "a FILE named `src` is not a directory-only candidate"
        );
        assert!(
            keeps("!src/", "src/a.ts"),
            "no FILE matches a directory-only glob"
        );
        assert!(
            !prunes("src/", "src"),
            "plain and directory-only: a HIT is a whitelist, kept"
        );
    }
}
