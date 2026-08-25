//! The SINGLE package-identifier normalizer/validator for `discovery/` (R-SA-006, pi
//! `identity.ts::normalizePackageName`).
//!
//! Three call sites need this grammar with three different error shapes — frontmatter parsing
//! (a failed normalization is a whole-file skip, so `Result<_, ()>`), chain-file parsing (a failed
//! normalization is a message-carrying error, `Result<_, String>`), and agent CRUD (a failed
//! normalization is a silent no-op, `Option`). They differ ONLY in that error shaping, so the
//! grammar itself lives here once and each caller wraps it.
//!
//! `discovery/management.rs` used to carry its own copy with a comment defending the duplication
//! ("this module owns its own file and must not require edits to `frontmatter.rs` to build"). That
//! rationale is retired: the grammar is no longer a private helper of one sibling module but a
//! module of its own that any sibling may import, so importing it introduces no coupling between
//! `management.rs` and `frontmatter.rs` at all.

/// Normalize a raw package identifier per R-SA-006's grammar: trim; lowercase; whitespace runs ->
/// a single `-`; strip anything outside `[a-z0-9.-]`; collapse repeated `-` runs, then repeated
/// `.` runs; trim leading/trailing `-`/`.`.
///
/// Returns `None` when the input is empty/whitespace-only, or when normalization leaves nothing.
/// Validation is deliberately NOT applied here — see [`is_valid_package_identifier`] — because the
/// callers disagree about what an invalid-but-non-empty identifier means.
#[must_use]
pub(crate) fn normalize_package_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lowered = trimmed.to_lowercase();

    // Whitespace runs -> single "-".
    let mut collapsed_ws = String::with_capacity(lowered.len());
    let mut last_was_ws = false;
    for ch in lowered.chars() {
        if ch.is_whitespace() {
            if !last_was_ws {
                collapsed_ws.push('-');
            }
            last_was_ws = true;
        } else {
            collapsed_ws.push(ch);
            last_was_ws = false;
        }
    }

    // Strip anything outside [a-z0-9.-].
    let filtered: String = collapsed_ws
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();

    // Collapse repeated "-" runs, then repeated "." runs, then trim leading/trailing "-"/".".
    let collapsed_hyphen = collapse_repeated_char(&filtered, '-');
    let collapsed_dot = collapse_repeated_char(&collapsed_hyphen, '.');
    let final_name = collapsed_dot
        .trim_start_matches(['-', '.'])
        .trim_end_matches(['-', '.'])
        .to_string();

    if final_name.is_empty() {
        None
    } else {
        Some(final_name)
    }
}

/// [`normalize_package_name`] followed by [`is_valid_package_identifier`] — `None` for an absent,
/// empty, or invalid-after-normalization identifier.
#[must_use]
pub(crate) fn normalize_valid_package_name(value: &str) -> Option<String> {
    normalize_package_name(value).filter(|name| is_valid_package_identifier(name))
}

/// Collapse runs of `target` down to a single occurrence.
pub(crate) fn collapse_repeated_char(s: &str, target: char) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_was_target = false;
    for ch in s.chars() {
        if ch == target {
            if !prev_was_target {
                out.push(ch);
            }
            prev_was_target = true;
        } else {
            out.push(ch);
            prev_was_target = false;
        }
    }
    out
}

/// `^[a-z0-9][a-z0-9-]*(?:\.[a-z0-9][a-z0-9-]*)*$` — lowercase alphanumeric/hyphen segments,
/// dot-separated, each segment starting with an alphanumeric character (R-SA-006).
#[must_use]
pub(crate) fn is_valid_package_identifier(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    name.split('.').all(|segment| {
        let mut chars = segment.chars();
        match chars.next() {
            Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
            _ => return false,
        }
        chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    })
}
