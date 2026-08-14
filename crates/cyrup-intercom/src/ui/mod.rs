//! The TUI overlay ports (`pi-intercom/ui/{compose,inline-message,session-list}.ts`) plus the
//! minimal `Theme`/`Keybindings`/width support they need.
//!
//! **What is WIRED this phase (production callers):**
//! - [`inline_message::InlineMessage`] renders the inbound-message card AND builds the
//!   `append_entry("intercom_message", …)` payload that surfaces an inbound broker message to the
//!   human — driven from [`crate::inbound::surface_incoming_message`] on EVERY real inbound message
//!   (`crate::extension`'s `SessionStart` inbound loop). This is the port doc §4.2/§7.2 human
//!   surface, and as of ICOM-028 the card it pre-renders is actually DRAWN: `IntercomExtension`
//!   registers an entry renderer for `intercom_message` and implements
//!   `NativeExtension::render_entry`. Before that the payload was written and consumed by nothing —
//!   the transcript showed a grey `entry appended → intercom_message` line instead of the card.
//! - [`session_list::SessionListOverlay::render`] + [`compose::compose_send`] are driven by the
//!   `/intercom` slash command ([`crate::extension::IntercomExtension::execute_command`]).
//!
//! **What DEGRADES (the port doc §4.3, precise later-phase TODO):** cyrup's native `InitApi` has no
//! `register_message_renderer`, so the live *message* renderer for the injected
//! `intercom_message` custom message is still unreachable — and blocked outside this crate besides,
//! since `HostServices::inject_message` carries no `details` for one to read (ICOM-029/ICOM-024).
//! The *interactive* overlay-host surface — the `alt+m` shortcut opening a live
//! [`compose::ComposeOverlay`]/[`session_list::SessionListOverlay`] with live keystroke handling —
//! is likewise not reachable yet. The overlays' interactive `handle_input` state machines are ported faithfully and
//! exercised by unit tests; they are wired to a real event source only once Phase 6 adds those two
//! `InitApi` hooks (a small arch-08 addition). This is DOCUMENTED, not silently dead: the render/send
//! paths ARE wired via the command + inbound surface above.
//!
//! [`visible_width`]/[`truncate_to_width`] port pi's `visibleWidth`/`truncateToWidth`
//! (`packages/tui/src/utils.ts`) width semantics exactly: ANSI/OSC/APC escape sequences are stripped
//! before measuring (a no-op today since cyrup only ever wires the ANSI-free [`PlainTheme`], but load-
//! bearing the moment a live/colored `Theme` is wired), tabs count 3 columns, and multi-codepoint
//! grapheme clusters (ZWJ emoji sequences, skin-tone modifiers, variation selectors, regional-
//! indicator flag pairs) are measured as one 2-column unit rather than summed per codepoint.

use unicode_width::UnicodeWidthChar;

pub mod compose;
pub mod inline_message;
pub mod session_list;

pub use compose::{ComposeAction, ComposeOverlay, compose_send};
pub use inline_message::InlineMessage;
pub use session_list::SessionListOverlay;

/// A color/emphasis theme (pi `Theme`, `@earendil-works/pi-coding-agent`). Only the two methods the
/// overlays actually call are modeled: `fg(color, text)` and `bold(text)`.
pub trait Theme {
    /// Colorize `text` under the named palette slot (`"accent"`/`"dim"`/`"error"`).
    fn fg(&self, color: &str, text: &str) -> String;
    /// Embolden `text`.
    fn bold(&self, text: &str) -> String;
}

/// The cyrup degrade theme: identity (no ANSI). The overlays render into `append_entry` payloads /
/// slash-command text, neither of which drives a live terminal this phase, so color is dropped and
/// the width math stays exact.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlainTheme;

impl Theme for PlainTheme {
    fn fg(&self, _color: &str, text: &str) -> String {
        text.to_string()
    }
    fn bold(&self, text: &str) -> String {
        text.to_string()
    }
}

/// The keybinding resolver the overlays consult (pi `KeybindingsManager`). `matches` tests a raw
/// input chunk against a named action; `get_keys` yields the display labels for a footer hint.
pub trait Keybindings {
    /// Whether the raw terminal input `data` triggers `action`.
    fn matches(&self, data: &str, action: &str) -> bool;
    /// The display key labels for `action` (footer hint, e.g. `["enter"]`).
    fn get_keys(&self, action: &str) -> Vec<String>;
}

/// The default terminal bindings the overlays fall back to (Enter=confirm, Esc/Ctrl+C=cancel,
/// arrows=up/down, Backspace=delete). Exact-match so an arrow escape (`\x1b[A`) is never mistaken for
/// a bare-Esc cancel (`\x1b`).
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultKeybindings;

impl Keybindings for DefaultKeybindings {
    fn matches(&self, data: &str, action: &str) -> bool {
        match action {
            "tui.select.cancel" => data == "\x1b" || data == "\x03",
            "tui.select.confirm" => data == "\r" || data == "\n",
            "tui.select.up" => data == "\x1b[A",
            "tui.select.down" => data == "\x1b[B",
            "tui.editor.deleteCharBackward" => data == "\x7f" || data == "\x08",
            _ => false,
        }
    }
    fn get_keys(&self, action: &str) -> Vec<String> {
        match action {
            "tui.select.confirm" => vec!["enter".to_string()],
            "tui.select.cancel" => vec!["escape".to_string(), "ctrl+c".to_string()],
            "tui.select.up" => vec!["up".to_string()],
            "tui.select.down" => vec!["down".to_string()],
            _ => Vec::new(),
        }
    }
}

/// Display-column width of `c` (pi `wcwidth`); control/zero-width chars count 0.
#[must_use]
pub fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Whether `c` is a Unicode regional-indicator symbol (`U+1F1E6..=U+1F1FF`, the "flag" halves).
/// Pi's `graphemeWidth` (`packages/tui/src/utils.ts:189-194`) forces these to 2 columns
/// unconditionally — even a lone, unpaired half — "to avoid terminal auto-wrap drift artifacts"
/// when a flag pair is split across a stream boundary.
fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// Whether `c` is an emoji skin-tone modifier (`U+1F3FB..=U+1F3FF`), which attaches to a preceding
/// emoji base and does not add its own column width (pi treats the whole modified-emoji grapheme
/// cluster as one 2-column unit).
fn is_skin_tone_modifier(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

/// Extract the length (in `chars`) of the ANSI/OSC/APC escape sequence starting at `chars[pos]`, or
/// `None` if `chars[pos]` is not `ESC` or the sequence never terminates. A 1:1 port of pi's
/// `extractAnsiCode` (`packages/tui/src/utils.ts:290-328`): CSI (`ESC [ ... ` up to the first of
/// `m`/`G`/`K`/`H`/`J`), OSC (`ESC ] ...` up to `BEL` or `ESC \`), and APC (`ESC _ ...` up to `BEL` or
/// `ESC \`) — same guards, same order, same "unterminated sequence is not consumed" fallback.
fn extract_ansi_code_len(chars: &[char], pos: usize) -> Option<usize> {
    if chars.get(pos) != Some(&'\x1b') {
        return None;
    }
    match chars.get(pos + 1) {
        Some('[') => {
            let mut j = pos + 2;
            while chars.get(j).is_some_and(|c| !matches!(c, 'm' | 'G' | 'K' | 'H' | 'J')) {
                j += 1;
            }
            if j < chars.len() { Some(j + 1 - pos) } else { None }
        }
        Some(']') | Some('_') => {
            let mut j = pos + 2;
            while let Some(&c) = chars.get(j) {
                if c == '\x07' {
                    return Some(j + 1 - pos);
                }
                if c == '\x1b' && chars.get(j + 1) == Some(&'\\') {
                    return Some(j + 2 - pos);
                }
                j += 1;
            }
            None
        }
        _ => None,
    }
}

/// Strip ANSI/OSC/APC escape sequences from `s` (pi `visibleWidth`'s escape-stripping pass,
/// `packages/tui/src/utils.ts:237-253`). A no-op when `s` has no `ESC` byte (the only case cyrup's
/// current ANSI-free [`PlainTheme`] ever produces), but load-bearing once a real ANSI-emitting
/// `Theme` is wired.
fn strip_ansi_sequences(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while let Some(&c) = chars.get(i) {
        if let Some(len) = extract_ansi_code_len(&chars, i) {
            i += len;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Parse the single grapheme cluster starting at `chars[start]` (mirrors pi's
/// `graphemeWidth`-over-`Intl.Segmenter` pass, `packages/tui/src/utils.ts:167-210, 257-258`, for the
/// cases that matter without a full Unicode grapheme-break table): a tab is its own 3-column cluster;
/// a regional-indicator half (optionally paired with a second half into a flag) is a 2-column
/// cluster; and a base codepoint followed by a variation selector (`U+FE0F`), skin-tone modifier, or
/// a `ZWJ`-joined continuation (family/couple emoji sequences) collapses into one 2-column cluster
/// rather than summing each codepoint's own width. Returns the cluster text, its display width, and
/// the index immediately following it. `start` must be `< chars.len()`; out-of-range yields an empty
/// cluster of width 0 at `start`.
fn next_grapheme_cluster(chars: &[char], start: usize) -> (String, usize, usize) {
    let Some(&base) = chars.get(start) else {
        return (String::new(), 0, start);
    };
    if base == '\t' {
        return ("\t".to_string(), 3, start + 1);
    }
    let mut i = start + 1;
    let mut forced_wide = is_regional_indicator(base);
    if forced_wide && chars.get(i).is_some_and(|&next| is_regional_indicator(next)) {
        i += 1; // Consume the flag's second half.
    }
    loop {
        match chars.get(i) {
            Some('\u{fe0f}') => {
                forced_wide = true;
                i += 1;
            }
            Some('\u{fe0e}') => {
                // VS15 (explicit text presentation): attaches without forcing emoji width.
                i += 1;
            }
            Some(c) if is_skin_tone_modifier(*c) => {
                forced_wide = true;
                i += 1;
            }
            Some('\u{200d}') => {
                // ZWJ: joins the current cluster to whatever follows (family/couple emoji).
                forced_wide = true;
                i += 1;
                if i < chars.len() {
                    i += 1;
                }
            }
            _ => break,
        }
    }
    let width = if forced_wide { 2 } else { char_width(base) };
    let cluster = chars.get(start..i).map_or_else(String::new, |slice| slice.iter().collect());
    (cluster, width, i)
}

/// Segment ANSI-free `text` into grapheme clusters paired with their display width — see
/// [`next_grapheme_cluster`].
fn grapheme_clusters(text: &str) -> Vec<(String, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let (cluster, width, next_i) = next_grapheme_cluster(&chars, i);
        out.push((cluster, width));
        i = next_i;
    }
    out
}

/// Display-column width of `s` (pi `visibleWidth`). Strips ANSI/OSC/APC escapes before measuring and
/// sums per grapheme cluster rather than per codepoint — see [`grapheme_clusters`].
#[must_use]
pub fn visible_width(s: &str) -> usize {
    let clean = strip_ansi_sequences(s);
    grapheme_clusters(&clean).into_iter().map(|(_, w)| w).sum()
}

/// The longest prefix of `text` whose display width is `<= max_width` (pi `truncateToWidth(text,
/// max_width, "")` — no ellipsis, `packages/tui/src/utils.ts:915-1051`). Never splits a grapheme
/// cluster (never splits a flag/ZWJ/skin-tone emoji sequence in half, matching pi's
/// segment-then-measure truncation loop). ANSI/OSC/APC escape codes never count toward width, but —
/// matching pi's `pendingAnsi` accumulate-then-flush exactly — they are NOT stripped from the kept
/// output: a code is buffered and only emitted once immediately followed by a grapheme cluster that
/// still fits; codes preceding the first cluster that overflows are dropped along with it.
#[must_use]
pub fn truncate_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if visible_width(text) <= max_width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut pending_ansi = String::new();
    let mut kept_width = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        if let Some(len) = extract_ansi_code_len(&chars, i) {
            if let Some(code) = chars.get(i..i + len) {
                pending_ansi.extend(code);
            }
            i += len;
            continue;
        }
        let (cluster, cw, next_i) = next_grapheme_cluster(&chars, i);
        if kept_width + cw > max_width {
            break;
        }
        if !pending_ansi.is_empty() {
            out.push_str(&pending_ansi);
            pending_ansi.clear();
        }
        out.push_str(&cluster);
        kept_width += cw;
        i = next_i;
    }
    out
}

/// Word-wrap `text` to `width` display columns (pi `wrapTextWithAnsi`, ANSI-free input): greedily
/// pack whole space-separated words, hard-breaking a word longer than `width`. Preserves explicit
/// `\n` breaks. Always returns at least one (possibly empty) line.
#[must_use]
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let mut current = String::new();
        let mut current_w = 0usize;
        for word in raw.split(' ') {
            let word_w = visible_width(word);
            if word_w > width {
                // Hard-break the over-long word across as many lines as needed.
                if !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                }
                let mut chunk = String::new();
                let mut chunk_w = 0usize;
                for c in word.chars() {
                    let cw = char_width(c);
                    if chunk_w + cw > width && !chunk.is_empty() {
                        lines.push(std::mem::take(&mut chunk));
                        chunk_w = 0;
                    }
                    chunk.push(c);
                    chunk_w += cw;
                }
                current = chunk;
                current_w = chunk_w;
                continue;
            }
            let sep = usize::from(!current.is_empty());
            if current_w + sep + word_w > width {
                lines.push(std::mem::take(&mut current));
                current.push_str(word);
                current_w = word_w;
            } else {
                if sep == 1 {
                    current.push(' ');
                    current_w += 1;
                }
                current.push_str(word);
                current_w += word_w;
            }
        }
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Middle-truncate `text` to `max_width` with a `…` join (pi `middleTruncate`, `ui/session-list.ts`).
#[must_use]
pub fn middle_truncate(text: &str, max_width: usize) -> String {
    if visible_width(text) <= max_width {
        return text.to_string();
    }
    if max_width <= 3 {
        return truncate_to_width(text, max_width);
    }
    let target_side = std::cmp::max(1, (max_width - 1) / 2);

    let mut left = String::new();
    for c in text.chars() {
        let mut candidate = left.clone();
        candidate.push(c);
        if visible_width(&candidate) > target_side {
            break;
        }
        left = candidate;
    }

    let mut right = String::new();
    for c in text.chars().rev() {
        let mut candidate = String::new();
        candidate.push(c);
        candidate.push_str(&right);
        if visible_width(&candidate) > target_side {
            break;
        }
        right = candidate;
    }

    truncate_to_width(&format!("{left}…{right}"), max_width)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn visible_width_counts_wide_chars() {
        assert_eq!(visible_width("abc"), 3);
        // 📨 (U+1F4E8) is a wide (2-column) glyph, exactly as pi's `visibleWidth` sees it.
        assert_eq!(visible_width("📨"), 2);
    }

    #[test]
    fn truncate_never_exceeds_width() {
        assert_eq!(truncate_to_width("hello", 3), "hel");
        assert_eq!(truncate_to_width("hi", 10), "hi");
        // A wide char that would overflow is dropped whole (never split below a column).
        assert_eq!(truncate_to_width("a📨", 2), "a");
    }

    #[test]
    fn wrap_packs_words_and_hard_breaks() {
        let lines = wrap_text("alpha beta gamma", 11);
        assert!(lines.iter().all(|l| visible_width(l) <= 11));
        assert!(lines.join(" ").contains("gamma"));
        // A word longer than the width is hard-broken.
        let broken = wrap_text("supercalifragilistic", 5);
        assert!(broken.len() > 1);
        assert!(broken.iter().all(|l| visible_width(l) <= 5));
    }

    #[test]
    fn middle_truncate_keeps_both_ends() {
        let out = middle_truncate("/Users/envvar/.config/ghostty", 15);
        assert!(visible_width(&out) <= 15);
        assert!(out.contains('…'));
    }

    #[test]
    fn visible_width_strips_ansi_escapes() {
        // pi's `visibleWidth` strips SGR/CSI codes before measuring (utils.ts:237-253); a naive
        // `UnicodeWidthStr::width` over the raw string would count the escape bytes and report 9
        // instead of 3. This is a regression proof for the moment cyrup wires a real ANSI-emitting
        // `Theme` (today's `PlainTheme` never emits these, so this exercises the escape path directly).
        let colored = "\x1b[1;31mfoo\x1b[0m";
        assert_eq!(visible_width(colored), 3);
        assert_eq!(truncate_to_width(colored, 2), "\x1b[1;31mfo");
    }

    #[test]
    fn visible_width_collapses_zwj_emoji_sequence_to_one_cluster() {
        // A ZWJ family emoji (man + ZWJ + woman + ZWJ + girl) is ONE grapheme cluster in pi's
        // `Intl.Segmenter`-based `visibleWidth`, measured as 2 columns total (graphemeWidth's RGI
        // emoji check, utils.ts:178). Summing each codepoint's own `unicode_width` (2+0+2+0+2)
        // instead gives 6 — the pre-fix behavior this test would fail against.
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        assert_eq!(visible_width(family), 2);
        // Truncating to width 1 must drop the whole cluster, never split it mid-sequence.
        assert_eq!(truncate_to_width(family, 1), "");
        assert_eq!(truncate_to_width(family, 2), family);
    }

    #[test]
    fn visible_width_forces_two_columns_for_isolated_regional_indicator() {
        // pi forces a lone (unpaired) regional-indicator half to 2 columns unconditionally
        // (utils.ts:189-194, "even when isolated during streaming"). This crate's `unicode-width`
        // dependency happens to assign width 1 to a standalone RI codepoint, so without the explicit
        // pi guard this would measure 1, not 2.
        assert_eq!(visible_width("\u{1F1FA}"), 2);
    }

    #[test]
    fn default_keybindings_distinguish_esc_from_arrows() {
        let kb = DefaultKeybindings;
        assert!(kb.matches("\x1b", "tui.select.cancel"));
        assert!(!kb.matches("\x1b[A", "tui.select.cancel"));
        assert!(kb.matches("\x1b[A", "tui.select.up"));
        assert!(kb.matches("\r", "tui.select.confirm"));
        assert!(kb.matches("\x7f", "tui.editor.deleteCharBackward"));
    }
}
