//! [`Input`] — the single-line text field from pi-tui (`packages/tui/src/components/input.ts`), as
//! far as the handover picker's "Next task" field uses it (`ui/handover-picker.ts:73`,
//! `new Input()`).
//!
//! Ported: insertion (plain, Kitty CSI-u printable, bracketed paste), Backspace/Delete,
//! Left/Right/Home/End, Ctrl+U/Ctrl+K line deletes, and `render`'s horizontal scrolling with the
//! reverse-video fake cursor (`input.ts:419-498`). Not ported, because nothing on the intercom
//! surface reaches them: the kill ring (yank/yank-pop), undo, word motions, `onSubmit`/`onEscape`
//! (the picker owns Enter and Escape before the field ever sees them), the placeholder (the picker
//! passes none) and the hardware-cursor IME marker (the host draws no hardware cursor in overlays).
//!
//! The cursor is a CHAR index into the value, and every motion steps a whole grapheme cluster
//! (pi `Intl.Segmenter`), measured by this module tree's own [`super::next_grapheme_cluster`].

use super::{Keybindings, Theme, grapheme_clusters, truncate_to_width, visible_width};

/// pi-tui `Input` (`input.ts:32`).
#[derive(Clone, Debug, Default)]
pub struct Input {
    value: String,
    /// The cursor, in chars from the start of [`Self::value`].
    cursor: usize,
    /// `isInPaste` / `pasteBuffer` (`input.ts:45-46`): a raw bracketed paste split across chunks.
    paste: Option<String>,
}

/// pi-tui's default prompt (`options.prompt ?? "> "`, `input.ts:58`).
const PROMPT: &str = "> ";
const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

/// pi-tui `decodeKittyPrintable` (`keys.ts:1404-1437`): `CSI <codepoint>[:<shifted>[:<base>]][;<mod>[:<event>]] u`
/// as the text it types, for a plain or Shift-modified printable key only. Lock bits (Caps/Num) are
/// ignored; Alt, Ctrl and every other modifier refuse, as upstream's do.
#[must_use]
pub fn decode_kitty_printable(data: &str) -> Option<String> {
    const SHIFT: u32 = 1;
    const LOCKS: u32 = 64 | 128;
    let body = data.strip_prefix("\x1b[")?.strip_suffix('u')?;
    let (keys, mods) = match body.split_once(';') {
        Some((keys, mods)) => (keys, Some(mods)),
        None => (body, None),
    };
    let mut key_parts = keys.split(':');
    let codepoint: u32 = key_parts.next()?.parse().ok()?;
    let shifted: Option<u32> = key_parts
        .next()
        .filter(|s| !s.is_empty())
        .map(str::parse)
        .transpose()
        .ok()?;
    let mod_value: u32 = match mods {
        Some(mods) => mods.split(':').next()?.parse().ok()?,
        None => 1,
    };
    let modifier = mod_value.checked_sub(1)?;
    if modifier & !(SHIFT | LOCKS) != 0 {
        return None;
    }
    let effective = match shifted {
        Some(shifted) if modifier & SHIFT != 0 => shifted,
        _ => codepoint,
    };
    if effective < 32 {
        return None;
    }
    char::from_u32(effective).map(|c| c.to_string())
}

impl Input {
    /// An empty field.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// pi `getValue()`.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    fn char_len(&self) -> usize {
        self.value.chars().count()
    }

    /// The byte offset of char index `chars`.
    fn byte_at(&self, chars: usize) -> usize {
        self.value
            .char_indices()
            .nth(chars)
            .map_or(self.value.len(), |(i, _)| i)
    }

    /// The char length of each grapheme cluster, in order.
    fn cluster_lengths(text: &str) -> Vec<usize> {
        grapheme_clusters(text)
            .into_iter()
            .map(|(cluster, _)| cluster.chars().count())
            .collect()
    }

    fn insert(&mut self, text: &str) {
        let at = self.byte_at(self.cursor);
        self.value.insert_str(at, text);
        self.cursor += text.chars().count();
    }

    /// pi `handlePaste` (`input.ts:404-413`): newlines dropped, tabs become four spaces.
    pub fn handle_paste(&mut self, pasted: &str) {
        let clean = pasted
            .replace("\r\n", "")
            .replace(['\r', '\n'], "")
            .replace('\t', "    ");
        self.insert(&clean);
    }

    /// pi `handleInput(data)` (`input.ts:69-224`), over the subset described in the module doc.
    pub fn handle_input(&mut self, keybindings: &dyn Keybindings, data: &str) {
        let mut data = data.to_string();
        if data.contains(PASTE_START) {
            self.paste = Some(String::new());
            data = data.replacen(PASTE_START, "", 1);
        }
        if let Some(buffer) = self.paste.as_mut() {
            buffer.push_str(&data);
            if let Some(end) = buffer.find(PASTE_END) {
                let content = buffer.get(..end).unwrap_or_default().to_string();
                let remaining = buffer
                    .get(end + PASTE_END.len()..)
                    .unwrap_or_default()
                    .to_string();
                self.paste = None;
                self.handle_paste(&content);
                if !remaining.is_empty() {
                    self.handle_input(keybindings, &remaining);
                }
            }
            return;
        }
        // Escape and Enter belong to the field's owner (`onEscape`/`onSubmit`, unset here).
        if keybindings.matches(&data, "tui.select.cancel")
            || keybindings.matches(&data, "tui.input.submit")
            || data == "\r"
            || data == "\n"
        {
            return;
        }
        if keybindings.matches(&data, "tui.editor.deleteCharBackward") {
            if self.cursor > 0 {
                let before: String = self.value.chars().take(self.cursor).collect();
                let len = Self::cluster_lengths(&before).last().copied().unwrap_or(1);
                let (from, to) = (self.byte_at(self.cursor - len), self.byte_at(self.cursor));
                self.value.replace_range(from..to, "");
                self.cursor -= len;
            }
            return;
        }
        if keybindings.matches(&data, "tui.editor.deleteCharForward") {
            if self.cursor < self.char_len() {
                let after: String = self.value.chars().skip(self.cursor).collect();
                let len = Self::cluster_lengths(&after).first().copied().unwrap_or(1);
                let (from, to) = (self.byte_at(self.cursor), self.byte_at(self.cursor + len));
                self.value.replace_range(from..to, "");
            }
            return;
        }
        if keybindings.matches(&data, "tui.editor.deleteToLineStart") {
            let at = self.byte_at(self.cursor);
            self.value.replace_range(..at, "");
            self.cursor = 0;
            return;
        }
        if keybindings.matches(&data, "tui.editor.deleteToLineEnd") {
            let at = self.byte_at(self.cursor);
            self.value.truncate(at);
            return;
        }
        if keybindings.matches(&data, "tui.editor.cursorLeft") {
            if self.cursor > 0 {
                let before: String = self.value.chars().take(self.cursor).collect();
                self.cursor -= Self::cluster_lengths(&before).last().copied().unwrap_or(1);
            }
            return;
        }
        if keybindings.matches(&data, "tui.editor.cursorRight") {
            if self.cursor < self.char_len() {
                let after: String = self.value.chars().skip(self.cursor).collect();
                self.cursor += Self::cluster_lengths(&after).first().copied().unwrap_or(1);
            }
            return;
        }
        if keybindings.matches(&data, "tui.editor.cursorLineStart") {
            self.cursor = 0;
            return;
        }
        if keybindings.matches(&data, "tui.editor.cursorLineEnd") {
            self.cursor = self.char_len();
            return;
        }
        if let Some(printable) = decode_kitty_printable(&data) {
            self.insert(&printable);
            return;
        }
        // `input.ts:216-223`: C0, DEL and C1 refuse the whole chunk.
        let has_control = data
            .chars()
            .any(|c| (c as u32) < 32 || c == '\x7f' || ('\u{80}'..='\u{9f}').contains(&c));
        if !has_control {
            self.insert(&data);
        }
    }

    /// pi `render(width)[0]` (`input.ts:419-498`): the prompt, the visible window of the value with
    /// the reverse-video fake cursor, padded to `width`.
    #[must_use]
    pub fn render(&self, theme: &dyn Theme, width: usize) -> String {
        let prompt_width = visible_width(PROMPT);
        if width <= prompt_width {
            return truncate_to_width(PROMPT, width);
        }
        let available = width - prompt_width;
        let clusters = grapheme_clusters(&self.value);
        // The cursor as a cluster index.
        let mut cursor_cluster = 0usize;
        let mut chars_seen = 0usize;
        for (cluster, _) in &clusters {
            if chars_seen >= self.cursor {
                break;
            }
            chars_seen += cluster.chars().count();
            cursor_cluster += 1;
        }
        let total_width: usize = clusters.iter().map(|(_, w)| w).sum();
        let (visible, cursor_display): (Vec<(String, usize)>, usize) = if total_width < available {
            (clusters.clone(), cursor_cluster)
        } else {
            let scroll_width = if cursor_cluster == clusters.len() {
                available - 1
            } else {
                available
            };
            let cursor_col: usize = clusters.iter().take(cursor_cluster).map(|(_, w)| w).sum();
            if scroll_width > 0 {
                let half = scroll_width / 2;
                let start_col = if cursor_col < half {
                    0
                } else if cursor_col > total_width.saturating_sub(half) {
                    total_width.saturating_sub(scroll_width)
                } else {
                    cursor_col.saturating_sub(half)
                };
                // pi `sliceByColumn(value, startCol, scrollWidth, strict = true)`: whole clusters
                // inside `[start_col, start_col + scroll_width)`, a straddling wide one excluded.
                let mut col = 0usize;
                let mut visible = Vec::new();
                let mut before_cursor = 0usize;
                for (index, (cluster, w)) in clusters.iter().enumerate() {
                    if col >= start_col && col + w <= start_col + scroll_width {
                        visible.push((cluster.clone(), *w));
                        if index < cursor_cluster {
                            before_cursor += 1;
                        }
                    }
                    col += w;
                }
                (visible, before_cursor)
            } else {
                (Vec::new(), 0)
            }
        };
        let before: String = visible
            .iter()
            .take(cursor_display)
            .map(|(c, _)| c.as_str())
            .collect();
        let at_cursor = visible
            .get(cursor_display)
            .map_or(" ", |(c, _)| c.as_str())
            .to_string();
        let after: String = visible
            .iter()
            .skip(cursor_display + 1)
            .map(|(c, _)| c.as_str())
            .collect();
        let text_with_cursor = format!("{before}{}{after}", theme.inverse(&at_cursor));
        let padding = available.saturating_sub(visible_width(&text_with_cursor));
        format!("{PROMPT}{text_with_cursor}{}", " ".repeat(padding))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use crate::ui::{DefaultKeybindings, PlainTheme};

    #[test]
    fn typing_editing_and_cursor_motion() {
        let kb = DefaultKeybindings;
        let mut input = Input::new();
        input.handle_input(&kb, "port");
        input.handle_input(&kb, " the fix");
        assert_eq!(input.value(), "port the fix");
        input.handle_input(&kb, "\x1b[H");
        input.handle_input(&kb, "\x1b[3~");
        assert_eq!(input.value(), "ort the fix");
        input.handle_input(&kb, "\x1b[F");
        input.handle_input(&kb, "\x7f");
        assert_eq!(input.value(), "ort the fi");
        input.handle_input(&kb, "\x1b[D");
        input.handle_input(&kb, "X");
        assert_eq!(input.value(), "ort the fXi");
        // A control chunk is refused whole.
        input.handle_input(&kb, "a\x07b");
        assert_eq!(input.value(), "ort the fXi");
        input.handle_input(&kb, "\x15");
        assert_eq!(input.value(), "i");
    }

    #[test]
    fn bracketed_and_split_pastes_and_kitty_printables_insert_text() {
        let kb = DefaultKeybindings;
        let mut input = Input::new();
        input.handle_input(&kb, "\x1b[200~port the\n fix");
        assert_eq!(input.value(), "", "buffered until the end marker");
        input.handle_input(&kb, "\x1b[201~");
        assert_eq!(input.value(), "port the fix");
        input.handle_input(&kb, "\x1b[32u");
        input.handle_input(&kb, "\x1b[97u");
        assert_eq!(input.value(), "port the fix a");
        // Shift picks the shifted code point; Ctrl refuses.
        input.handle_input(&kb, "\x1b[97:65;2u");
        input.handle_input(&kb, "\x1b[97;5u");
        assert_eq!(input.value(), "port the fix aA");
    }

    #[test]
    fn render_is_exactly_the_width_and_scrolls_to_the_cursor() {
        let kb = DefaultKeybindings;
        let mut input = Input::new();
        for width in [1usize, 2, 3, 10, 40] {
            assert_eq!(visible_width(&input.render(&PlainTheme, width)), width);
        }
        input.handle_input(&kb, "a long next task that does not fit");
        for width in [1usize, 2, 3, 10, 40] {
            assert_eq!(visible_width(&input.render(&PlainTheme, width)), width);
        }
        let line = input.render(&PlainTheme, 12);
        assert!(line.starts_with("> "));
        assert!(
            line.contains("fit"),
            "the window follows the cursor: {line:?}"
        );
    }
}
