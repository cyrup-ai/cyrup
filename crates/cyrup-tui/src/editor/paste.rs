use super::*;

impl InputEditor {
    // ---- large-paste markers (spec/tui/03 §5.5) --------------------------------------------

    /// Handle a (bracketed) paste (`editor.ts:615` `handlePaste`): sanitize, then either collapse a
    /// **large** paste (`> 10` lines or `> 1000` chars) to an atomic `[paste #N …]` marker stored in
    /// [`pastes`](Self::pastes), or insert a small paste verbatim. The marker keeps the buffer compact;
    /// [`expanded_text`](Self::expanded_text) restores the full content on submit.
    pub fn handle_paste(&mut self, raw: &str) {
        let text = sanitize_paste(raw);
        let line_count = text.split('\n').count();
        let char_count = text.chars().count();
        if line_count > 10 || char_count > 1000 {
            // The snapshot is the FIRST thing `handlePaste` does (`editor.ts:1160`), *before* the
            // counter and the registry are touched — so one undo rolls the paste back completely and
            // the next paste re-issues the same id. cyrup pushed it after `pastes.insert` + the
            // increment, which is why paste → undo → paste re-issued `#2` where pi re-issues `#1`
            // (TUI-042's quiet variant).
            self.push_undo_for(LastAction::None);
            self.paste_counter += 1;
            let id = self.paste_counter;
            let marker = if line_count > 1 {
                format!("[paste #{id} +{line_count} lines]")
            } else {
                format!("[paste #{id} {char_count} chars]")
            };
            self.pastes.insert(id, text);
            for c in marker.chars() {
                self.insert_char(c);
            }
            self.last_action = LastAction::None;
            self.reset_preferred_col();
            self.exit_history();
            self.update_autocomplete();
        } else {
            self.insert_str(&text);
        }
    }

    /// The buffer text with every `[paste #N …]` marker expanded back to its stored content
    /// (`expandPasteMarkers`, `editor.ts`). Submission uses this so the model receives the full paste.
    pub fn expanded_text(&self) -> String {
        let text = self.text();
        if self.pastes.is_empty() {
            return text;
        }
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < chars.len() {
            if let Some((_, content, end)) = self.marker_at(&chars, i) {
                out.push_str(content);
                i = end;
            } else if let Some(c) = chars.get(i) {
                out.push(*c);
                i += 1;
            } else {
                break;
            }
        }
        out
    }

    /// If a `[paste #N …]` marker for a known id starts at `chars[i]`, return its `(id, content, end)`
    /// where `end` is the char index just past the closing `]`. Bounds-checked throughout (no-panic).
    ///
    /// The accepted grammar is `PASTE_MARKER_SINGLE` (`editor.ts:24` @v0.83.0), anchored at `i`:
    ///
    /// ```text
    /// /^\[paste #(\d+)( (\+\d+ lines|\d+ chars))?\]$/
    /// ```
    ///
    /// i.e. the id, then **either** an immediate `]`, **or** one space and exactly one of
    /// `+<digits> lines` / `<digits> chars` before the `]` — the two shapes
    /// [`handle_paste`](Self::handle_paste) produces, plus the bare `[paste #N]` the regex allows.
    /// The previous implementation scanned to the first `]` with the body unconstrained, so a
    /// hand-typed `[paste #1 see the file above]` matched and [`expanded_text`](Self::expanded_text)
    /// silently replaced the user's own words with the stored paste (TUI-049). The id must also be
    /// live in [`pastes`](Self::pastes) — pi's `validIds` gate (`segmentWithMarkers`, `:44`).
    pub(super) fn marker_at<'a>(&'a self, chars: &[char], i: usize) -> Option<(u32, &'a str, usize)> {
        let (id, _, end) = marker_span_at(chars, i)?;
        let content = self.pastes.get(&id)?;
        Some((id, content.as_str(), end))
    }

    /// Every **valid** marker span on `chars` as `(start, end, id)`, left to right and
    /// non-overlapping — the marker scan `segmentWithMarkers` runs before merging
    /// (`editor.ts:48-57`: `for (const m of text.matchAll(PASTE_MARKER_REGEX)) { if
    /// (!validIds.has(id)) continue; markers.push(…) }`).
    pub(super) fn marker_spans(&self, chars: &[char]) -> Vec<(usize, usize, u32)> {
        let mut spans = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            match self.marker_at(chars, i) {
                Some((id, _, end)) => {
                    spans.push((i, end, id));
                    i = end;
                }
                None => i += 1,
            }
        }
        spans
    }

    // `marker_covering(col)` — "is `col` inside or on either edge of a marker" — used to be the whole
    // of cyrup's marker atomicity, called from `backspace()` and `delete()` and from nowhere else. It
    // is gone: upstream has no such predicate. Atomicity there is a property of the SEGMENTER
    // (`this.segment(text, "grapheme" | "word")`, `editor.ts:361-363`), which every motion and
    // deletion path already goes through, so the marker is atomic for cursor motion too and the
    // caret can never be parked inside one. See [`marker_grapheme_boundaries`](Self::marker_grapheme_boundaries)
    // and [`word_segments`](Self::word_segments).

    /// Retire the paste a just-backspaced `[paste #N …]` marker owned, then **renumber** — a literal
    /// port of `handleBackspace`'s paste branch (`editor.ts:1293-1315` @v0.83.0):
    ///
    /// ```text
    /// this.pastes.delete(targetId);
    /// this.pasteCounter--;
    /// // Shift registry entries down in ascending id order …
    /// const higherIds = [...this.pastes.keys()].filter((id) => id > targetId).sort((a, b) => a - b);
    /// for (const id of higherIds) { this.pastes.set(id - 1, this.pastes.get(id)!); this.pastes.delete(id); }
    /// // Renumber markers with ids greater than the removed one.
    /// this.state.lines = this.state.lines.map((line) => line.replace(PASTE_MARKER_REGEX, …));
    /// ```
    ///
    /// A `BTreeMap` already iterates ascending, which is what upstream's `.sort()` buys. The text
    /// rewrite runs on the **syntactic** matcher with no `validIds` filter, exactly as upstream's
    /// bare `PASTE_MARKER_REGEX` replace does.
    ///
    /// [CYRUP-DELTA] none — including the hazard: renumbering `#10` → `#9` shortens a line, and
    /// upstream computes the deletion offsets *before* the rewrite and re-reads the line *after* it
    /// (`:1317-1322`), so a two-digit marker earlier on the same line shifts the deletion. That is
    /// upstream's arithmetic and it is reproduced rather than quietly corrected; see the report.
    pub(super) fn drop_paste(&mut self, target: u32) {
        self.pastes.remove(&target);
        self.paste_counter = self.paste_counter.saturating_sub(1);
        let higher: Vec<u32> = self.pastes.keys().copied().filter(|&id| id > target).collect();
        for id in higher {
            if let Some(content) = self.pastes.remove(&id) {
                self.pastes.insert(id.saturating_sub(1), content);
            }
        }
        for line in &mut self.lines {
            *line = renumber_markers(line, target);
        }
    }
}

/// Read a run of ASCII digits starting at `from`, returning `(value, index just past the run)` — or
/// `None` when there is no digit there (`\d+` in `PASTE_MARKER_SINGLE`).
fn read_digits(chars: &[char], from: usize) -> Option<(u32, usize)> {
    let mut j = from;
    let mut value: u32 = 0;
    let mut count = 0usize;
    while let Some(&c) = chars.get(j).filter(|c| c.is_ascii_digit()) {
        value = value.saturating_mul(10).saturating_add(c.to_digit(10).unwrap_or(0));
        j += 1;
        count += 1;
    }
    (count > 0).then_some((value, j))
}

/// Match `PASTE_MARKER_SINGLE` (`editor.ts:24` @v0.83.0) anchored at `chars[i]`, **syntactically** —
/// without consulting the paste registry. Returns `(id, index just past the id digits, index just
/// past the closing `]`)`.
///
/// ```text
/// /^\[paste #(\d+)( (\+\d+ lines|\d+ chars))?\]$/
/// ```
///
/// The registry-gated form is [`InputEditor::marker_at`] (pi's `validIds` filter,
/// `segmentWithMarkers` `:44`). The ungated form exists because pi's marker RENUMBERING replaces on
/// the bare `PASTE_MARKER_REGEX` with no id filter (`editor.ts:1308-1314`).
///
/// `isPasteMarker`'s extra `segment.length >= 10` guard (`:28`) needs no counterpart: the shortest
/// string this grammar accepts is `[paste #1]`, which is exactly 10 characters.
fn marker_span_at(chars: &[char], i: usize) -> Option<(u32, usize, usize)> {
    const PREFIX: [char; 8] = ['[', 'p', 'a', 's', 't', 'e', ' ', '#'];
    for (k, pc) in PREFIX.iter().enumerate() {
        if chars.get(i + k) != Some(pc) {
            return None;
        }
    }
    let (id, digits_end) = read_digits(chars, i + PREFIX.len())?;
    // `( (\+\d+ lines|\d+ chars))?` then `\]`.
    if chars.get(digits_end) == Some(&']') {
        return Some((id, digits_end, digits_end + 1));
    }
    if chars.get(digits_end) != Some(&' ') {
        return None;
    }
    let mut j = digits_end + 1;
    let plus = chars.get(j) == Some(&'+');
    if plus {
        j += 1;
    }
    let (_, after) = read_digits(chars, j)?;
    j = after;
    let tail: &[char] =
        if plus { &[' ', 'l', 'i', 'n', 'e', 's', ']'] } else { &[' ', 'c', 'h', 'a', 'r', 's', ']'] };
    for (n, tc) in tail.iter().enumerate() {
        if chars.get(j + n) != Some(tc) {
            return None;
        }
    }
    Some((id, digits_end, j + tail.len()))
}

/// Rewrite every syntactic `[paste #x …]` marker on `line` with `x > target` as `x - 1`, keeping its
/// suffix — the `line.replace(PASTE_MARKER_REGEX, …)` of `handleBackspace` (`editor.ts:1308-1314`):
///
/// ```text
/// (fullMatch, idGroup, suffixGroup) => { const x = Number(idGroup); if (x <= targetId) return fullMatch;
///                                        return `[paste #${x - 1}${suffixGroup}]`; }
/// ```
fn renumber_markers(line: &[char], target: u32) -> Vec<char> {
    let mut out: Vec<char> = Vec::with_capacity(line.len());
    let mut i = 0usize;
    while i < line.len() {
        match marker_span_at(line, i) {
            Some((id, digits_end, end)) => {
                if id > target {
                    out.extend(format!("[paste #{}", id.saturating_sub(1)).chars());
                    out.extend(line.get(digits_end..end).unwrap_or(&[]).iter().copied());
                } else {
                    out.extend(line.get(i..end).unwrap_or(&[]).iter().copied());
                }
                i = end;
            }
            None => {
                if let Some(&c) = line.get(i) {
                    out.push(c);
                }
                i += 1;
            }
        }
    }
    out
}

/// Sanitize a bracketed-paste payload (`editor.ts:1142-1179`): normalize `\r\n`/`\r` to `\n`, expand
/// tabs to four spaces, and drop control bytes other than `\n`.
fn sanitize_paste(raw: &str) -> String {
    let unified = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(unified.len());
    for c in unified.chars() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}
