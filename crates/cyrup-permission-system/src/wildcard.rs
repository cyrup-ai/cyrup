//! Wildcard pattern compilation + matching (port of pi `wildcard-matcher.ts`). A pattern compiles
//! to an anchored, **dotAll** regex; matching iterates **from the end** (last-match-wins). The
//! dotAll (`s`) flag is load-bearing (`wildcard-matcher.ts:27`, Issue #24): a multi-line bash/heredoc
//! command must be matched by `.*` across newlines, else it bypasses every rule to the default.

use regex::RegexBuilder;

/// A compiled wildcard entry carrying its provenance `state` (pi `CompiledWildcardPattern<TState>`,
/// `wildcard-matcher.ts:1-5`).
pub struct CompiledWildcard<S> {
    pub pattern: String,
    pub state: S,
    /// `None` only if the (escaped, anchored) regex failed to build — treated as never-matching so a
    /// pathological pattern degrades to "no match" rather than panicking (no-panic policy).
    regex: Option<regex::Regex>,
}

impl<S> CompiledWildcard<S> {
    /// True iff `name` matches this pattern.
    #[must_use]
    pub fn is_match(&self, name: &str) -> bool {
        self.regex.as_ref().map(|r| r.is_match(name)).unwrap_or(false)
    }
}

/// pi `compileWildcardPattern` (`wildcard-matcher.ts:13-29`): `\\`→`/`, escape regex metachars,
/// `*`→`.*`, `?`→`.`, a trailing `" .*"`→`"( .*)?"` (optional trailing arg), anchored with the dotAll
/// flag.
#[must_use]
pub fn compile<S>(pattern: &str, state: S) -> CompiledWildcard<S> {
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
    // `git` (pi `wildcard-matcher.ts:20-22`).
    if let Some(head) = escaped.strip_suffix(" .*") {
        escaped = format!("{head}( .*)?");
    }

    let anchored = format!("^{escaped}$");
    let regex = RegexBuilder::new(&anchored).dot_matches_new_line(true).build().ok();

    CompiledWildcard { pattern: pattern.to_string(), state, regex }
}

/// Compile every `(pattern, state)` entry, preserving order (pi `compileWildcardPatternEntries`,
/// `wildcard-matcher.ts:31-35`).
#[must_use]
pub fn compile_entries<S>(entries: impl IntoIterator<Item = (String, S)>) -> Vec<CompiledWildcard<S>> {
    entries.into_iter().map(|(pattern, state)| compile(&pattern, state)).collect()
}

/// pi `findCompiledWildcardMatch` (`wildcard-matcher.ts:43-60`): normalize `\\`→`/` in `name`, then
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

/// pi `findCompiledWildcardMatchForNames` (`wildcard-matcher.ts:62-79`): trim+drop-empty the names,
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
        let ps = compile_entries(vec![
            ("*".to_string(), 1u8),
            ("git *".to_string(), 2u8),
        ]);
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
}
