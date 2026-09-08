//! The display-text normalizer and §A.4's truncating family (SCOPE_3e SUBTASK0) — pi
//! `shared/display-text.ts:1-89`.
//!
//! [`sanitize_display_text`] is a display *data* normalizer with no terminal dependency, which is
//! why it lives here and not in `tui/`: `tui/fleet_transcript.rs`'s `safe_display_text` is a
//! DIFFERENT function with a different job — it ports `safeTerminalText`, which **escapes**
//! control points as `[U+001B]` markers for terminal rendering, while this one **strips** whole
//! control sequences for bounded labels. Substituting one for the other corrupts every checklist
//! label (SCOPE_3e §0.4).
//!
//! [`truncate_display`] / [`truncate_to_bytes`] are the truncating half of SCOPE_3 §A.4's
//! bounded-string split: `workflows/bounded.rs` is the REJECTING family (a type, because a
//! rejected-if-over value carries an invariant), and these two are free functions, because a
//! truncated value has no invariant worth carrying. `bounded.rs`'s own module doc promises they
//! land here, beside it.

/// pi `isWhitespaceOrControl` (`shared/display-text.ts:5-9`): C0 controls and space
/// (`<= 0x20`), DEL plus the C1 range (`0x7f..=0x9f`), and any code point whose `trim()` is
/// empty — i.e. Unicode whitespace, which [`char::is_whitespace`] answers directly.
///
/// Upstream's `0xd800..=0xdfff` arm is a lone-surrogate guard; a Rust `&str` cannot contain one,
/// so that branch has no port ([CYRUP-DELTA, unrepresentable]).
fn is_whitespace_or_control(c: char) -> bool {
    let code_point = c as u32;
    if code_point <= 0x20 || (0x7f..=0x9f).contains(&code_point) {
        return true;
    }
    c.is_whitespace()
}

/// pi `consumeControlString` (`shared/display-text.ts:11-21`): advance past an OSC/DCS/SOS/PM/APC
/// control string's body, returning the index just past its terminator — BEL (only when `osc`),
/// the 8-bit ST `\u{9c}`, or the two-character ESC-`\` ST. An unterminated string consumes to the
/// end, exactly as upstream returns `value.length`.
fn consume_control_string(chars: &[char], mut index: usize, osc: bool) -> usize {
    while let Some(&c) = chars.get(index) {
        if osc && c == '\u{07}' {
            return index + 1;
        }
        if c == '\u{9c}' {
            return index + 1;
        }
        if c == '\u{1b}' && chars.get(index + 1) == Some(&'\\') {
            return index + 2;
        }
        index += 1;
    }
    chars.len()
}

/// pi `consumeCsi` (`shared/display-text.ts:23-31`): advance past a CSI sequence's parameter and
/// intermediate bytes, returning the index just past the final byte (`0x40..=0x7e`). An
/// unterminated sequence consumes to the end.
fn consume_csi(chars: &[char], mut index: usize) -> usize {
    while let Some(&c) = chars.get(index) {
        let code_point = c as u32;
        if (0x40..=0x7e).contains(&code_point) {
            return index + 1;
        }
        index += 1;
    }
    chars.len()
}

/// pi `sanitizeDisplayText` (`shared/display-text.ts:33-79`): strip terminal control sequences and
/// collapse every whitespace/control run to a single space, emitting no leading or trailing space.
///
/// Handles, in upstream's order: ESC-CSI (`\x1b[` … final byte `0x40..=0x7e`), ESC-OSC/DCS/SOS/PM/
/// APC (`\x1b]`/`P`/`X`/`^`/`_` … terminated by BEL, ST `\x9c`, or `\x1b\\`), the 8-bit CSI
/// `\x9b`, the 8-bit control-string introducers `\x90`/`\x98`/`\x9d`/`\x9e`/`\x9f`, and everything
/// [`is_whitespace_or_control`] matches. A bare ESC followed by any other character consumes that
/// character too (a two-character escape sequence, upstream's `index += next ? 2 : 1`).
///
/// The `pendingSpace` latch (`:38-46`) is why this is a hand-written scanner and not
/// `split_whitespace().join(" ")`: a space is *recorded* when a run is consumed but only *emitted*
/// when real text follows, so the output never has a leading or trailing space and never needs a
/// second trim pass.
///
/// [CYRUP-DELTA, unrepresentable] upstream's lone-surrogate arm (`:7`) has no port — see
/// [`is_whitespace_or_control`].
#[must_use]
pub fn sanitize_display_text(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut output = String::new();
    let mut pending_space = false;
    let mut index = 0usize;

    // pi `appendSpace` (`:38-40`): record a pending separator only once real text exists.
    macro_rules! append_space {
        () => {
            if !output.is_empty() {
                pending_space = true;
            }
        };
    }

    while let Some(&c) = chars.get(index) {
        if c == '\u{1b}' {
            append_space!();
            match chars.get(index + 1) {
                Some('[') => {
                    index = consume_csi(&chars, index + 2);
                }
                Some(next @ (']' | 'P' | 'X' | '^' | '_')) => {
                    let osc = *next == ']';
                    index = consume_control_string(&chars, index + 2, osc);
                }
                Some(_) => {
                    index += 2;
                }
                None => {
                    index += 1;
                }
            }
            continue;
        }
        if c == '\u{9b}' {
            append_space!();
            index = consume_csi(&chars, index + 1);
            continue;
        }
        if matches!(c, '\u{90}' | '\u{98}' | '\u{9d}' | '\u{9e}' | '\u{9f}') {
            append_space!();
            index = consume_control_string(&chars, index + 1, c == '\u{9d}');
            continue;
        }
        if is_whitespace_or_control(c) {
            append_space!();
        } else {
            if pending_space {
                output.push(' ');
            }
            output.push(c);
            pending_space = false;
        }
        index += 1;
    }

    output
}

/// §A.4's truncating family, UTF-16 code units — pi `text`'s `clean.slice(0, MAX_TEXT)`
/// (`workflow-checklist.ts:121`) and `planLaneLabel`'s `value.slice(0, N - 1)`
/// (`workflow-preflight.ts:241`). A hard cut with NO ellipsis and NO invariant, which is exactly
/// why §A.4 makes it a free function and not a [`crate::workflows::Bounded`] sibling.
///
/// Cuts on a scalar-value boundary: JS slices by UTF-16 code unit and can split a surrogate pair
/// into two lone surrogates, which Rust cannot represent. The port keeps whole scalar values, so a
/// cut that would land mid-astral-character yields one character fewer. Stated because it is the
/// one place this family is not byte-identical to upstream, and it is unrepresentable, not a
/// choice ([CYRUP-DELTA, unrepresentable]).
#[must_use]
pub fn truncate_display(value: &str, max_utf16_units: usize) -> String {
    let mut units = 0usize;
    let mut output = String::new();
    for c in value.chars() {
        let width = c.len_utf16();
        if units + width > max_utf16_units {
            break;
        }
        units += width;
        output.push(c);
    }
    output
}

/// §A.4's truncating family, UTF-8 bytes — pi `boundedText` (`host-command.ts:45-50`): values over
/// `max_bytes` are cut so that the result INCLUDING a literal `"..."` fits in `max_bytes`.
///
/// Upstream walks `end` down one UTF-16 unit at a time re-measuring `Buffer.byteLength` each step
/// (`:48`), which is quadratic; the port seeks straight to the greatest
/// [`str::is_char_boundary`] index at or below `max_bytes - 3`. Same result, one pass
/// ([CYRUP-DELTA, mechanism]).
#[must_use]
pub fn truncate_to_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let budget = max_bytes.saturating_sub(3);
    let mut end = budget.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let kept = value.get(..end).unwrap_or_default();
    format!("{kept}...")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// The CSI arm: the whole `\x1b[2J` sequence is consumed and stands in for one separator.
    /// Contrast `tui/fleet_transcript.rs`'s `safe_display_text`, whose own test asserts
    /// `"a[U+001B][2Jb"` for this input — the two functions must never be conflated (§0.4).
    #[test]
    fn strips_csi_sequences_to_a_single_separator() {
        assert_eq!(sanitize_display_text("a\u{1b}[2Jb"), "a b");
        assert_eq!(sanitize_display_text("a\u{1b}[31;1mb"), "a b");
        // 8-bit CSI introducer.
        assert_eq!(sanitize_display_text("a\u{9b}2Jb"), "a b");
    }

    /// OSC terminated by BEL, by 8-bit ST, and by ESC-backslash; DCS/SOS/PM/APC via their 8-bit
    /// introducers; an unterminated string consumes to the end.
    #[test]
    fn strips_control_strings_with_all_three_terminators() {
        assert_eq!(sanitize_display_text("a\u{1b}]0;title\u{07}b"), "a b");
        assert_eq!(sanitize_display_text("a\u{1b}]0;title\u{9c}b"), "a b");
        assert_eq!(sanitize_display_text("a\u{1b}Pdata\u{1b}\\b"), "a b");
        assert_eq!(sanitize_display_text("a\u{9d}0;title\u{07}b"), "a b");
        assert_eq!(sanitize_display_text("a\u{90}data\u{9c}b"), "a b");
        assert_eq!(sanitize_display_text("a\u{1b}]never terminated"), "a");
    }

    /// The `pendingSpace` latch: no leading space when the input starts with a sequence or
    /// whitespace, no trailing space when it ends with one, and runs collapse to one space.
    #[test]
    fn latch_emits_no_leading_or_trailing_space() {
        assert_eq!(sanitize_display_text("\u{1b}[31mred"), "red");
        assert_eq!(sanitize_display_text("  a \t\n b  "), "a b");
        assert_eq!(sanitize_display_text("a\u{1}\u{2}b"), "a b");
        assert_eq!(sanitize_display_text("trail \u{1b}[0m"), "trail");
        assert_eq!(sanitize_display_text(""), "");
        assert_eq!(sanitize_display_text("\u{1b}"), "");
    }

    /// A bare ESC followed by an ordinary character consumes that character too — upstream's
    /// `index += next ? 2 : 1` two-character escape arm.
    #[test]
    fn bare_escape_consumes_its_follower() {
        assert_eq!(sanitize_display_text("a\u{1b}cb"), "a b");
    }

    /// UTF-16 units, not bytes and not chars: '𝄞' is two units, 'é' is one.
    #[test]
    fn truncate_display_counts_utf16_units_and_keeps_whole_scalars() {
        assert_eq!(truncate_display("abc", 2), "ab");
        assert_eq!(truncate_display("abc", 10), "abc");
        assert_eq!(truncate_display("𝄞x", 2), "𝄞");
        // A cut landing mid-surrogate-pair keeps one character fewer (the documented delta).
        assert_eq!(truncate_display("𝄞", 1), "");
        assert_eq!(truncate_display("éé", 1), "é");
    }

    /// pi `boundedText`: the result INCLUDING `"..."` fits `max_bytes`; a cut never splits a
    /// UTF-8 character.
    #[test]
    fn truncate_to_bytes_reserves_room_for_the_ellipsis() {
        assert_eq!(truncate_to_bytes("abcdef", 10), "abcdef");
        assert_eq!(truncate_to_bytes("abcdef", 5), "ab...");
        // 'é' is 2 bytes: a 5-byte limit leaves a 2-byte budget, which fits exactly one 'é'.
        assert_eq!(truncate_to_bytes("ééé", 5), "é...");
        // A budget that would split a character backs up to the boundary.
        assert_eq!(truncate_to_bytes("aééé", 6), "aé...");
        assert_eq!(truncate_to_bytes("abcdef", 2), "...");
    }
}
