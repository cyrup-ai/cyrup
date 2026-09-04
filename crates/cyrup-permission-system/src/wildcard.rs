//! Wildcard pattern compilation + matching (port of pi `wildcard-matcher.ts`). A pattern compiles
//! to an anchored, **dotAll** regex; matching iterates **from the end** (last-match-wins). The
//! dotAll (`s`) flag is load-bearing (`wildcard-matcher.ts:41`, Issue #24): a multi-line bash/heredoc
//! command must be matched by `.*` across newlines, else it bypasses every rule to the default.
//! On Windows the compiled regex is *also* case-insensitive (pi `process.platform === "win32" ?
//! "si" : "s"`, `wildcard-matcher.ts:41`); every other platform stays case-sensitive.

use regex::RegexBuilder;

/// Maximum length of a wildcard permission pattern (pi `MAX_WILDCARD_PATTERN_LENGTH = 500`,
/// `wildcard-matcher.ts:16`).
///
/// **This port buys parity, not hardening — do not read it as a vulnerability fix.** Upstream's cap
/// defends JavaScript's backtracking `RegExp` against catastrophic blowup on a hostile pattern. The
/// `regex` crate compiles to an automaton with guaranteed linear-time matching and enforces its own
/// compiled-size limit, so no 501-character wildcard could "blow up" compilation or matching here in
/// the first place. The cap is ported because it is OBSERVABLE: at v0.8.0 an over-long pattern stops
/// matching anything, and a cyrup that kept matching it would resolve a policy differently from pi
/// given the same config file.
///
/// **Unit**: pi's `pattern.length` counts UTF-16 code units; the exact Rust analog is
/// [`str::encode_utf16`]`().count()`, which is what [`compile_with_case_insensitive`] uses. `len()`
/// (UTF-8 bytes) and `chars().count()` (scalar values) would agree only for ASCII.
const MAX_WILDCARD_PATTERN_LENGTH: usize = 500;

/// A compiled wildcard entry carrying its provenance `state` (pi `CompiledWildcardPattern<TState>`,
/// `wildcard-matcher.ts:3-7`).
pub struct CompiledWildcard<S> {
    pub pattern: String,
    pub state: S,
    /// `None` means **never match**. Two ways to get here:
    ///
    /// 1. `pattern` exceeded [`MAX_WILDCARD_PATTERN_LENGTH`] — pi's `NEVER_MATCH_PATTERN`
    ///    (`wildcard-matcher.ts:18,21-27`).
    /// 2. The (escaped, anchored) regex failed to build — degrade to "no match" rather than
    ///    panicking (no-panic policy).
    ///
    /// The sole reader is [`CompiledWildcard::is_match`], which maps `None` to `false`, so every
    /// consumer sees "this rule does not apply, keep scanning" — never a wildcard admit. Falling
    /// through the scan yields [`crate::types::PermissionState::Ask`] in `evaluate.rs` and the
    /// category default in `manager.rs`, i.e. the safe directions.
    ///
    /// **[CYRUP-DELTA]** pi's `NEVER_MATCH_PATTERN` is `/$^/`, two zero-width assertions, which
    /// does match the EMPTY string (at offset 0 of `""` both `$` and `^` hold). `None` matches
    /// nothing at all, so cyrup is very slightly *stricter*: an oversized rule tested against an
    /// empty subject matches upstream and not here. That is the safe direction (an oversized
    /// `allow` rule cannot allow an empty command) and it is unreachable on the decision path
    /// anyway — `gate::get_pattern_approval_subject` falls back to the tool name, so the subject is
    /// never `""`.
    regex: Option<regex::Regex>,
}

impl<S> CompiledWildcard<S> {
    /// True iff `name` matches this pattern.
    #[must_use]
    pub fn is_match(&self, name: &str) -> bool {
        self.regex
            .as_ref()
            .map(|r| r.is_match(name))
            .unwrap_or(false)
    }
}

/// pi `compileWildcardPattern` (`wildcard-matcher.ts:20-43`): `\\`→`/`, escape regex metachars,
/// `*`→`.*`, `?`→`.`, a trailing `" .*"`→`"( .*)?"` (optional trailing arg), anchored with the dotAll
/// flag.
#[must_use]
pub fn compile<S>(pattern: &str, state: S) -> CompiledWildcard<S> {
    // pi: `process.platform === "win32" ? "si" : "s"` — on Windows the compiled regex is also
    // case-insensitive; every other platform is dotAll only (`wildcard-matcher.ts:41`).
    compile_with_case_insensitive(pattern, state, cfg!(windows))
}

/// Same as [`compile`] but with the case-insensitive flag passed explicitly instead of derived
/// from `cfg!(windows)`, so the win32 (`"si"`) branch of pi's platform check
/// (`wildcard-matcher.ts:41`) is unit-testable on every host platform.
#[must_use]
pub fn compile_with_case_insensitive<S>(
    pattern: &str,
    state: S,
    case_insensitive: bool,
) -> CompiledWildcard<S> {
    // pi `wildcard-matcher.ts:21-27`: an over-long pattern short-circuits to `NEVER_MATCH_PATTERN`
    // BEFORE any escaping, and `pattern` is carried through unmodified so `matchedPattern`
    // reporting is unaffected. `> MAX`, so a pattern of exactly 500 still compiles normally.
    // Both `compile` and `compile_entries` funnel through here, so this covers the per-call
    // compiles in `evaluate.rs:27,31` and the pre-compiled tables in `manager.rs` alike.
    if pattern.encode_utf16().count() > MAX_WILDCARD_PATTERN_LENGTH {
        return CompiledWildcard {
            pattern: pattern.to_string(),
            state,
            regex: None,
        };
    }

    let mut escaped = String::with_capacity(pattern.len() * 2 + 2);
    for ch in pattern.chars() {
        match ch {
            // `\\`→`/` FIRST (pi `.replaceAll("\\", "/")`), so no backslash survives to be escaped.
            '\\' => escaped.push('/'),
            // Regex metacharacter class pi escapes: `[.+^${}()|[\]]` (NOT `*`/`?`, handled next).
            '.' | '+' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            '*' => escaped.push_str(".*"),
            '?' => escaped.push('.'),
            other => escaped.push(other),
        }
    }

    // Trailing `" .*"` → `"( .*)?"` (an optional trailing argument): a rule `git *` also matches bare
    // `git` (pi `wildcard-matcher.ts:34-36`).
    if let Some(head) = escaped.strip_suffix(" .*") {
        escaped = format!("{head}( .*)?");
    }

    let anchored = format!("^{escaped}$");
    let regex = RegexBuilder::new(&anchored)
        .dot_matches_new_line(true)
        .case_insensitive(case_insensitive)
        .build()
        .ok();

    CompiledWildcard {
        pattern: pattern.to_string(),
        state,
        regex,
    }
}

/// Compile every `(pattern, state)` entry, preserving order (pi `compileWildcardPatternEntries`,
/// `wildcard-matcher.ts:45-49`).
#[must_use]
pub fn compile_entries<S>(
    entries: impl IntoIterator<Item = (String, S)>,
) -> Vec<CompiledWildcard<S>> {
    entries
        .into_iter()
        .map(|(pattern, state)| compile(&pattern, state))
        .collect()
}

/// pi `findCompiledWildcardMatch` (`wildcard-matcher.ts:57-74`): normalize `\\`→`/` in `name`, then
/// iterate **from the end**, returning the index of the last matching entry (last-match-wins).
#[must_use]
pub fn find_match_index<S>(patterns: &[CompiledWildcard<S>], name: &str) -> Option<usize> {
    let normalized = name.replace('\\', "/");
    for index in (0..patterns.len()).rev() {
        if let Some(p) = patterns.get(index)
            && p.is_match(&normalized)
        {
            return Some(index);
        }
    }
    None
}

/// pi `findCompiledWildcardMatchForNames` (`wildcard-matcher.ts:76-81`): trim+drop-empty the names,
/// then return the first name (in order) that has any match, plus that match's index.
#[must_use]
pub fn find_match_for_names<S>(
    patterns: &[CompiledWildcard<S>],
    names: &[String],
) -> Option<(usize, String)> {
    for name in names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(index) = find_match_index(patterns, trimmed) {
            return Some((index, trimmed.to_string()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn star_matches_dotall_across_newlines() {
        // Issue #24: a multi-line command must be matched by `*`.
        let c = compile("git *", ());
        assert!(c.is_match("git commit -m \"line1\nline2\""));
        assert!(c.is_match("git")); // trailing " .*" → optional
        assert!(c.is_match("git push"));
        assert!(!c.is_match("npm install"));
    }

    #[test]
    fn question_mark_matches_single_char() {
        let c = compile("rm -rf ?", ());
        assert!(c.is_match("rm -rf x"));
        assert!(!c.is_match("rm -rf xy"));
    }

    #[test]
    fn last_match_wins_from_end() {
        let ps = compile_entries(vec![("*".to_string(), 1u8), ("git *".to_string(), 2u8)]);
        // "git status" matches both; the LATER entry ("git *") wins.
        let idx = find_match_index(&ps, "git status").unwrap();
        assert_eq!(ps[idx].state, 2u8);
    }

    #[test]
    fn sibling_directory_does_not_match() {
        let c = compile("read:/safe/*", ());
        assert!(c.is_match("read:/safe/file"));
        assert!(!c.is_match("read:/safe-evil/file"));
    }

    #[test]
    fn windows_flag_makes_pattern_case_insensitive() {
        // pi `wildcard-matcher.ts:41`: `process.platform === "win32" ? "si" : "s"` — on Windows the
        // regex is compiled with BOTH dotAll and case-insensitive ("si"). Pre-fix, cyrup never set
        // `case_insensitive` at all, so this would fail regardless of the flag passed in.
        let c = compile_with_case_insensitive("GIT *", (), true);
        assert!(c.is_match("git commit"));
        assert!(c.is_match("GIT COMMIT"));
    }

    #[test]
    fn non_windows_flag_keeps_pattern_case_sensitive() {
        // Every other platform (pi's "s"-only branch) must stay case-sensitive.
        let c = compile_with_case_insensitive("git *", (), false);
        assert!(c.is_match("git commit"));
        assert!(!c.is_match("GIT COMMIT"));
    }

    // ---- pi `wildcard-matcher.ts:16,18,21-27` — the 500-char cap ----
    //
    // The dangerous direction is "over-long ⇒ match EVERYTHING", which would turn a DoS guard into
    // a permission bypass. These tests pin the opposite: over-long ⇒ match NOTHING. All patterns
    // here are pure ASCII so byte / char / UTF-16 counts coincide and the boundary is unambiguous.

    #[test]
    fn oversized_pattern_never_matches_not_matches_everything() {
        // 501 `*`s: WITHOUT the cap this escapes to `.*` repeated 501 times and matches literally
        // any input, so an oversized `allow` rule would admit everything.
        let oversized = "*".repeat(MAX_WILDCARD_PATTERN_LENGTH + 1);
        assert_eq!(oversized.encode_utf16().count(), 501);
        let c = compile(&oversized, ());
        assert!(
            !c.is_match("rm -rf /"),
            "an oversized pattern must never match"
        );
        assert!(!c.is_match("git status"));
        assert!(
            !c.is_match(""),
            "cyrup is stricter than pi's `/$^/` on the empty subject"
        );
        // `pattern` is carried through unmodified (pi returns `{ pattern, state, regex }`).
        assert_eq!(c.pattern, oversized);
    }

    #[test]
    fn oversized_literal_pattern_never_matches_even_itself() {
        let oversized = "a".repeat(600);
        let c = compile(&oversized, ());
        assert!(!c.is_match(&oversized));
        assert!(!c.is_match("normal command"));
    }

    #[test]
    fn pattern_at_exactly_the_cap_still_matches() {
        // MIRROR CASE — pi's check is `> 500`, so exactly 500 must compile and behave normally.
        let at_cap = format!("{}*", "a".repeat(MAX_WILDCARD_PATTERN_LENGTH - 1));
        assert_eq!(at_cap.encode_utf16().count(), MAX_WILDCARD_PATTERN_LENGTH);
        let c = compile(&at_cap, ());
        assert!(c.is_match(&format!(
            "{} --flag",
            "a".repeat(MAX_WILDCARD_PATTERN_LENGTH - 1)
        )));
        assert!(!c.is_match("git status"));

        // A 500-`*` pattern is still a match-everything wildcard: the cap is a length rule, not a
        // "lots of stars" rule.
        let stars_at_cap = "*".repeat(MAX_WILDCARD_PATTERN_LENGTH);
        assert!(compile(&stars_at_cap, ()).is_match("rm -rf /"));

        // And one char under the cap.
        let under_cap = format!("{}*", "a".repeat(MAX_WILDCARD_PATTERN_LENGTH - 2));
        assert!(
            compile(&under_cap, ())
                .is_match(&format!("{}z", "a".repeat(MAX_WILDCARD_PATTERN_LENGTH - 2)))
        );
    }

    #[test]
    fn find_match_index_skips_oversized_entries() {
        // pi `tests/wildcard-redos.test.ts`: an oversized entry in the list must be ignored, and the
        // normal entry after it must still win.
        let ps = compile_entries(vec![("*".repeat(600), 1u8), ("ls *".to_string(), 2u8)]);
        let idx = find_match_index(&ps, "ls -la").unwrap();
        assert_eq!(ps[idx].state, 2u8);
        assert_eq!(ps[idx].pattern, "ls *");
        // Nothing the oversized entry could have matched: it is last-in-order-but-one, and
        // last-match-wins would have handed it "rm -rf /" if it still matched everything.
        assert!(find_match_index(&ps, "rm -rf /").is_none());
    }

    #[test]
    fn compile_derives_case_insensitivity_from_host_platform() {
        // `compile` must forward `cfg!(windows)` into the case-insensitive flag exactly as pi derives
        // it from `process.platform === "win32"` (`wildcard-matcher.ts:41`).
        let c = compile("git *", ());
        assert_eq!(c.is_match("GIT COMMIT"), cfg!(windows));
    }
}
