//! The TUI overlay ports (`pi-intercom/ui/{compose,inline-message,session-list}.ts`) plus the
//! minimal `Theme`/`Keybindings`/width support they need.
//!
//! **What is WIRED this phase (production callers):**
//! - [`inline_message::InlineMessage`] renders the inbound-message card AND builds the
//!   `append_entry("intercom_message", …)` payload that surfaces an inbound broker message to the
//!   human — driven from [`crate::inbound::surface_incoming_message`] on EVERY real inbound message
//!   (`crate::extension`'s `SessionStart` inbound loop). This is the port doc §4.2/§7.2 human surface.
//! - [`session_list::SessionListOverlay::render`] + [`compose::compose_send`] are driven by the
//!   `/intercom` slash command ([`crate::extension::IntercomExtension::execute_command`]).
//!
//! **What DEGRADES (the port doc §4.3, precise later-phase TODO):** cyrup's native `InitApi` has no
//! `register_message_renderer` / `register_shortcut`, so the *interactive* overlay-host surface — the
//! `alt+m` shortcut opening a live [`compose::ComposeOverlay`]/[`session_list::SessionListOverlay`]
//! with live keystroke handling, and the live `intercom_message` inline-card renderer — is not
//! reachable yet. The overlays' interactive `handle_input` state machines are ported faithfully and
//! exercised by unit tests; they are wired to a real event source only once Phase 6 adds those two
//! `InitApi` hooks (a small arch-08 addition). This is DOCUMENTED, not silently dead: the render/send
//! paths ARE wired via the command + inbound surface above.
//!
//! All width math assumes NO embedded ANSI (cyrup degrades to [`PlainTheme`], and the surfaced entry
//! is plain text), so [`visible_width`] is a straight display-column count — pi's `visibleWidth`
//! strips ANSI first, which is a no-op on plain input.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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

/// Display-column width of `s` (pi `visibleWidth`, ANSI-free input).
#[must_use]
pub fn visible_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// The longest prefix of `text` whose display width is `<= max_width` (pi `truncateToWidth(text,
/// max_width, "")` — no ellipsis). Never splits below a char boundary.
#[must_use]
pub fn truncate_to_width(text: &str, max_width: usize) -> String {
    if visible_width(text) <= max_width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut width = 0usize;
    for c in text.chars() {
        let cw = char_width(c);
        if width + cw > max_width {
            break;
        }
        out.push(c);
        width += cw;
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
    fn default_keybindings_distinguish_esc_from_arrows() {
        let kb = DefaultKeybindings;
        assert!(kb.matches("\x1b", "tui.select.cancel"));
        assert!(!kb.matches("\x1b[A", "tui.select.cancel"));
        assert!(kb.matches("\x1b[A", "tui.select.up"));
        assert!(kb.matches("\r", "tui.select.confirm"));
        assert!(kb.matches("\x7f", "tui.editor.deleteCharBackward"));
    }
}
