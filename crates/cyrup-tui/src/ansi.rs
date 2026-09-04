//! Display sanitization for text the TUI did not produce itself — the port of `stripAnsi`
//! (`coding-agent/src/utils/ansi.ts`) and `sanitizeBinaryOutput` (`utils/shell.ts:144-174`).
//!
//! Pi funnels every tool result through both before rendering: `getTextOutput` is
//! `sanitizeBinaryOutput(stripAnsi(c.text || "")).replace(/\r/g, "")` (`render-utils.ts:48`).
//!
//! ## Why ratatui is not already enough
//! ratatui filters *control* characters out of every grapheme run before it reaches a cell
//! (`Span::styled_graphemes`, ratatui-core `text/span.rs:314`; `Buffer::set_stringn`,
//! `buffer/buffer.rs:351`), so a bare `ESC` can never be written to the terminal and an escape
//! sequence cannot **execute** — no cursor moves, no title rewrite, no hidden text. That is a real
//! guarantee and it is why this is a rendering-fidelity fix, not a security one.
//!
//! It is also all it does. The *rest* of a sequence — `[1;31m`, `]8;;file:///…`, `[?25l` — is
//! ordinary printable text and lands in the transcript verbatim, so a colorized `ls` or `grep`
//! result reads as `[1;31msrc[0m` where Pi shows `src`. Unicode format characters (U+FFF9..U+FFFB)
//! are category `Cf`, not `Cc`, so `char::is_control` does not match them either and they survive
//! all the way to the screen — which is exactly the class `sanitizeBinaryOutput` exists to remove.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// `sanitizeBinaryOutput(stripAnsi(text)).replace(/\r/g, "")` — the whole `getTextOutput` transform
/// (`render-utils.ts:48`) in the order Pi applies it.
///
/// Order matters: stripping ANSI first means an escape sequence's `ESC` is consumed as part of the
/// sequence rather than being dropped on its own by the control-character filter, which would leave
/// the parameter bytes stranded as text.
pub(crate) fn sanitize_display_text(text: &str) -> String {
    sanitize_binary_output(&strip_ansi(text)).replace('\r', "")
}

/// Filter characters that crash width measurement / break terminal rendering (Pi
/// `sanitizeBinaryOutput`, `utils/shell.ts:144-174`): keep tab/newline/CR, drop the other C0
/// control characters (0x00-0x1F) and the Unicode format-character range U+FFF9..=U+FFFB.
///
/// Iterates by Rust `char` (a Unicode scalar value), the same code-point granularity as Pi's
/// `Array.from(str)`; a Rust `str` cannot hold a lone surrogate at all, so Pi's surrogate case has
/// no analog here.
pub(crate) fn sanitize_binary_output(input: &str) -> String {
    input
        .chars()
        .filter(|&c| {
            let code = c as u32;
            if code == 0x09 || code == 0x0A || code == 0x0D {
                return true;
            }
            if code <= 0x1F {
                return false;
            }
            !(0xFFF9..=0xFFFB).contains(&code)
        })
        .collect()
}

/// Strip ANSI escape sequences (Pi `stripAnsi`, `utils/ansi.ts`): OSC sequences (`ESC ] … ST`,
/// non-greedy up to the first terminator) and CSI/related sequences (`ESC`/C1 CSI, optional
/// intermediates, optional numeric params, one final byte).
///
/// A hand-rolled scanner over the exact `ansi-regex` grammar Pi vendors — this crate has no
/// general-purpose regex dependency — using `Chars::as_str()`/`strip_prefix` only, never indexing.
/// A sequence with no valid terminator/final byte does **not** match, exactly as the regex fails at
/// that position, and its characters are emitted unchanged.
pub(crate) fn strip_ansi(input: &str) -> String {
    // Fast path, Pi's own (`ansi.ts`): no introducer, nothing to do.
    if !input.contains('\u{1B}') && !input.contains('\u{9B}') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    loop {
        let mut it = rest.chars();
        let Some(c) = it.next() else { break };
        let tail = it.as_str();

        if c == '\u{1B}' {
            if let Some(after) = rest.strip_prefix("\u{1B}]")
                && let Some(end) = find_osc_terminator(after)
            {
                rest = end;
                continue;
            }
            if let Some(end) = try_csi(rest) {
                rest = end;
                continue;
            }
        } else if c == '\u{9B}'
            && let Some(end) = try_csi(rest)
        {
            rest = end;
            continue;
        }
        out.push(c);
        rest = tail;
    }
    out
}

/// Convert one SGR-styled row into ratatui spans.
///
/// Reuses [`try_csi`]'s scanner to find each `ESC [ … m`, applies its parameters to a running
/// [`Style`], and emits the text between codes as spans. Non-SGR CSI and OSC sequences are consumed
/// and ignored — they address the terminal, and a cell grid has nothing to apply them to. An
/// unterminated sequence is emitted as literal text, exactly as [`strip_ansi`] leaves it.
///
/// Supports the subset a [`cyrup_ext::RenderTheme`] emits: `0` (reset), `1` (bold), `2` (dim), `22`
/// (normal intensity), `38;2;r;g;b` (truecolor foreground) and `39` (default foreground). Anything
/// else is parsed and skipped rather than guessed at.
pub(crate) fn sgr_line(input: &str) -> Line<'static> {
    if !input.contains('\u{1B}') && !input.contains('\u{9B}') {
        return Line::from(input.to_string());
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut style = Style::default();
    let mut pending = String::new();
    let mut rest = input;

    // Flush whatever text has accumulated under the style that was in force while it accumulated.
    fn flush(spans: &mut Vec<Span<'static>>, pending: &mut String, style: Style) {
        if !pending.is_empty() {
            spans.push(Span::styled(std::mem::take(pending), style));
        }
    }

    loop {
        let mut it = rest.chars();
        let Some(c) = it.next() else { break };
        let tail = it.as_str();

        if c == '\u{1B}'
            && let Some(after) = rest.strip_prefix("\u{1B}]")
            && let Some(end) = find_osc_terminator(after)
        {
            rest = end;
            continue;
        }
        if (c == '\u{1B}' || c == '\u{9B}')
            && let Some(end) = try_csi(rest)
        {
            // The sequence body is everything the scanner consumed; SGR is the `m`-terminated one.
            // `try_csi` returns a suffix OF `rest`, so stripping it off yields exactly the consumed
            // prefix — and does so without a byte-index slice (`clippy::string_slice` is denied).
            // The `unwrap_or("")` arm is unreachable for that reason; an empty body is inert here.
            let consumed = rest.strip_suffix(end).unwrap_or("");
            if consumed.ends_with('m') {
                flush(&mut spans, &mut pending, style);
                style = apply_sgr(style, consumed);
            }
            rest = end;
            continue;
        }
        pending.push(c);
        rest = tail;
    }
    flush(&mut spans, &mut pending, style);
    Line::from(spans)
}

/// Apply one `ESC [ … m` sequence's parameters to `style`. `seq` is the whole consumed sequence,
/// introducer and final `m` included; anything between them that is not a recognised code is
/// skipped rather than guessed at.
fn apply_sgr(mut style: Style, seq: &str) -> Style {
    let body = seq
        .trim_start_matches('\u{1B}')
        .trim_start_matches('\u{9B}')
        .trim_start_matches('[')
        .trim_end_matches('m');
    // An empty body (`ESC[m`) is `ESC[0m` — a reset (ECMA-48 default parameter).
    if body.is_empty() {
        return Style::default();
    }
    let codes: Vec<&str> = body.split(&[';', ':'][..]).collect();
    let mut i = 0;
    while i < codes.len() {
        // Read through `get` rather than `[]`: the `while` guard already proves `i` is in range,
        // but `clippy::indexing_slicing` is denied crate-wide and a `let ... else` carries the
        // proof in the types.
        let Some(code) = codes.get(i) else { break };
        match *code {
            "0" | "" => style = Style::default(),
            "1" => style = style.add_modifier(Modifier::BOLD),
            "2" => style = style.add_modifier(Modifier::DIM),
            "22" => style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            "39" => style.fg = None,
            // An extended foreground. The tail is consumed by the FORM actually matched, never by a
            // fixed stride: `38;2;r;g;b` is five codes and `38;5;n` is three, so advancing four for
            // both swallows whatever follows an indexed colour (`ESC[38;5;196;1m` would lose the
            // bold).
            //
            // The two paths differ, and each is deliberate:
            //
            // * a RECOGNISED form consumes its whole tail even when truncated — `38;2;1` still
            //   advances four and `38;5` still advances two — so a malformed colour is skipped
            //   entire rather than half-applied from a partial parse;
            // * an UNRECOGNISED or absent form (`38;9`, or a trailing bare `38`) consumes only the
            //   introducer, so whatever follows is still read as ordinary codes instead of being
            //   swallowed by a stride guessed for a form that never matched.
            "38" => match codes.get(i + 1) {
                // `38;2;r;g;b` — truecolor.
                Some(&"2") => {
                    if let (Some(r), Some(g), Some(b)) = (
                        codes.get(i + 2).and_then(|v| v.parse::<u8>().ok()),
                        codes.get(i + 3).and_then(|v| v.parse::<u8>().ok()),
                        codes.get(i + 4).and_then(|v| v.parse::<u8>().ok()),
                    ) {
                        style = style.fg(Color::Rgb(r, g, b));
                    }
                    i += 4;
                }
                // `38;5;n` — 256-colour index, which ratatui carries natively.
                Some(&"5") => {
                    if let Some(n) = codes.get(i + 2).and_then(|v| v.parse::<u8>().ok()) {
                        style = style.fg(Color::Indexed(n));
                    }
                    i += 2;
                }
                _ => {}
            },
            _ => {}
        }
        i += 1;
    }
    style
}

/// Scan past an OSC sequence's terminator (BEL, `ESC \`, or C1 ST 0x9C), non-greedy — `after` is
/// everything following the `ESC ]` introducer. Returns the slice past the terminator, or `None`
/// when none exists before the end of the string.
fn find_osc_terminator(after: &str) -> Option<&str> {
    let mut rest = after;
    loop {
        let mut it = rest.chars();
        let c = it.next()?;
        let tail = it.as_str();
        match c {
            '\u{07}' | '\u{9C}' => return Some(tail),
            '\u{1B}' => {
                if let Some(t2) = tail.strip_prefix('\u{5C}') {
                    return Some(t2);
                }
                rest = tail;
            }
            _ => rest = tail,
        }
    }
}

/// Match a CSI/related sequence starting at `rest`'s first char (already known to be the ESC/0x9B
/// introducer). Returns the slice past the match, or `None` when there is no valid final byte.
fn try_csi(rest: &str) -> Option<&str> {
    let mut it = rest.chars();
    it.next()?; // the introducer itself, already checked by the caller
    let mut cur = it.as_str();

    // Intermediates: zero or more of `[ ] ( ) # ; ?` (the regex's `[[\]()#;?]*`).
    loop {
        let mut it2 = cur.chars();
        match it2.next() {
            Some('[' | ']' | '(' | ')' | '#' | ';' | '?') => cur = it2.as_str(),
            _ => break,
        }
    }

    // Optional numeric params: `(?:\d{1,4}(?:[;:]\d{0,4})*)?`, then exactly one final byte.
    //
    // The params run MUST be able to give ground. Upstream's grammar is a single regex whose
    // final-byte class STARTS with `\d`:
    //     [][[\]()#;?]*(?:\d{1,4}(?:[;:]\d{0,4})*)?[\dA-PR-TZcf-nq-uy=><~]
    // so on `ESC[31` the engine matches params as `3` and the final byte as `1`, and the whole
    // sequence is stripped. A parser that commits to the longest params run and then demands a
    // final byte returns no-match and emits `ESC[31` as literal text — leaving the user staring at
    // `[31`, which is precisely the residue this function exists to remove, and it is not exotic:
    // truncated SGR sequences are routine in streamed shell and tool output.
    //
    // Emulate the backtracking directly: try every legal params length, longest first, and accept
    // the first one followed by a valid final byte.
    let params_full = consume_params(cur);
    let params_len = cur.len() - params_full.len();
    for end in (0..=params_len).rev() {
        if !cur.is_char_boundary(end) {
            continue;
        }
        let (params, tail) = cur.split_at(end);
        if !is_valid_params(params) {
            continue;
        }
        let mut it3 = tail.chars();
        if let Some(final_byte) = it3.next()
            && is_csi_final_byte(final_byte)
        {
            return Some(it3.as_str());
        }
    }
    None
}

/// Does `s` match `(?:\d{1,4}(?:[;:]\d{0,4})*)?` exactly? Used to reject the params prefixes that
/// regex backtracking would never produce (a run ending mid-separator, or a digit group longer than
/// four), so the give-ground loop above explores only decompositions upstream would also try.
fn is_valid_params(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    let mut rest = s;
    let after_first = consume_digits(rest, 4);
    if after_first.len() == rest.len() {
        return false; // `\d{1,4}` requires at least one digit
    }
    rest = after_first;
    loop {
        let mut it = rest.chars();
        match it.next() {
            Some(';' | ':') => rest = consume_digits(it.as_str(), 4),
            Some(_) => return false,
            None => return true,
        }
    }
}

/// `(?:\d{1,4}(?:[;:]\d{0,4})*)?` — present only if the FIRST char is a digit.
fn consume_params(input: &str) -> &str {
    if !input.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return input;
    }
    let mut cur = consume_digits(input, 4);
    loop {
        let mut it = cur.chars();
        match it.next() {
            Some(';' | ':') => cur = consume_digits(it.as_str(), 4),
            _ => break,
        }
    }
    cur
}

/// Consume up to `max` ASCII digits, returning the slice past them.
fn consume_digits(input: &str, max: usize) -> &str {
    let mut cur = input;
    let mut count = 0;
    while count < max {
        let mut it = cur.chars();
        match it.next() {
            Some(c) if c.is_ascii_digit() => {
                cur = it.as_str();
                count += 1;
            }
            _ => break,
        }
    }
    cur
}

/// The regex's `[\dA-PR-TZcf-nq-uy=><~]` final-byte class.
fn is_csi_final_byte(c: char) -> bool {
    c.is_ascii_digit()
        || matches!(c,
            'A'..='P' | 'R'..='T' | 'Z' | 'c'..='n' | 'q'..='u' | 'y' | '=' | '>' | '<' | '~'
        )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn strips_sgr_and_cursor_sequences() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m text"), "red text");
        assert_eq!(strip_ansi("\u{1b}[2J\u{1b}[1;1Hcleared"), "cleared");
        assert_eq!(strip_ansi("\u{1b}[?25lhidden\u{1b}[?25h"), "hidden");
    }

    #[test]
    fn strips_osc_with_either_terminator() {
        assert_eq!(strip_ansi("\u{1b}]0;title\u{07}body"), "body");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{1b}\\body"), "body");
        assert_eq!(
            strip_ansi("\u{1b}]8;;http://x\u{07}link\u{1b}]8;;\u{07}"),
            "link"
        );
    }

    #[test]
    fn passthrough_when_no_introducer() {
        // The fast path, and the guarantee the MIRROR test in `tests/tool_result_sanitize.rs`
        // asserts end-to-end: escape-looking text that has no ESC is not touched.
        let s = "items[0m1].rs\nnotes ]8;; draft.md";
        assert_eq!(strip_ansi(s), s);
    }

    #[test]
    fn a_truncated_params_run_gives_ground_to_the_final_byte() {
        // This test previously asserted `strip_ansi("ESC[31") == "ESC[31"` — i.e. that a truncated
        // SGR survives — and the parser was written to match. Both were wrong, and asserting a
        // divergence as though it were upstream behaviour is worse than the divergence: it makes
        // the parity claim self-certifying.
        //
        // Upstream's final-byte class STARTS with `\d`
        // (`[][[\]()#;?]*(?:\d{1,4}(?:[;:]\d{0,4})*)?[\dA-PR-TZcf-nq-uy=><~]`), so the engine
        // gives back digits until one serves as the final byte. Verified against the vendored
        // pattern itself, not merely reasoned about.
        assert_eq!(strip_ansi("\u{1b}[31"), "", "params `3`, final byte `1`");
        assert_eq!(
            strip_ansi("\u{1b}[1;31"),
            "",
            "a truncated SGR is still stripped"
        );
        assert_eq!(
            strip_ansi("\u{1b}[3abc"),
            "abc",
            "params empty, final byte `3`"
        );
        assert_eq!(
            strip_ansi("\u{1b}7abc"),
            "abc",
            "no intermediates, final byte `7`"
        );
        // Four digits is the `\d{1,4}` ceiling, so the fifth digit is the final byte and `99m`
        // survives — cyrup and pi already agreed here, which is what makes it a useful control.
        assert_eq!(strip_ansi("\u{1b}[99999999m"), "999m");
    }

    #[test]
    fn a_sequence_with_no_reachable_final_byte_is_left_alone() {
        // The genuine no-match case: `X`, `Y`, `Z` are not in the final-byte class and there are no
        // digits to give back, so neither alternative matches and every character survives.
        assert_eq!(strip_ansi("\u{1b}]XYZrest"), "\u{1b}]XYZrest");
    }

    #[test]
    fn an_unterminated_osc_can_still_match_as_csi() {
        // A faithful-to-the-regex quirk, not a bug, and the same one
        // `cyrup-session-svc/src/bash.rs` documents: with no ST the `osc` alternative fails, then
        // `csi` matches at the same position — `]` and `;` are CSI intermediate bytes and `n` is a
        // valid final byte, so `ESC ];;n` is consumed. Pi's vendored `ansi-regex` behaves
        // identically (alternation tries `osc` first, falls back to `csi`).
        assert_eq!(strip_ansi("\u{1b}]8;;no-terminator"), "o-terminator");
        assert_eq!(strip_ansi("\u{1b}]unterminated"), "nterminated");
    }

    #[test]
    fn binary_filter_keeps_whitespace_drops_controls_and_format_chars() {
        assert_eq!(sanitize_binary_output("a\tb\nc\rd"), "a\tb\nc\rd");
        assert_eq!(sanitize_binary_output("a\u{0}b\u{7}c\u{1f}d"), "abcd");
        assert_eq!(
            sanitize_binary_output("a\u{fff9}b\u{fffa}c\u{fffb}d"),
            "abcd"
        );
        // U+FFF8 and U+FFFC are outside the filtered range and must survive.
        assert_eq!(
            sanitize_binary_output("\u{fff8}\u{fffc}"),
            "\u{fff8}\u{fffc}"
        );
    }

    #[test]
    fn display_transform_applies_all_three_steps() {
        assert_eq!(
            sanitize_display_text("\u{1b}[32mok\u{1b}[0m\r\n\u{fffb}done\u{0}"),
            "ok\ndone"
        );
    }

    #[test]
    fn never_panics_on_adversarial_input() {
        for s in [
            "\u{1b}",
            "\u{9b}",
            "\u{1b}]",
            "\u{1b}[",
            "\u{1b}[;;;;;;;;",
            "\u{1b}]\u{1b}",
            "\u{1b}[99999999m",
            "é\u{1b}[31mé",
        ] {
            let _ = sanitize_display_text(s);
        }
    }
}
