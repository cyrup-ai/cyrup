//! The index-unit-agnostic half of pi's word navigation
//! (`pi/packages/tui/src/word-navigation.ts` @v0.83.0), shared by the multi-line
//! [`InputEditor`](super::InputEditor) and the single-line [`crate::text_input::Input`].
//!
//! Upstream's `findWordBackward`/`findWordForward` take `(text, cursor)` and segment the text
//! themselves. cyrup cannot: the editor is **char-indexed** by design (`editor/mod.rs`, no-panic
//! policy R-00-009) and merges `[paste #N …]` markers into atomic segments, while a selector search
//! field is a plain `String` addressed by **byte** offset with no markers. So the split here is one
//! level lower than pi's — the caller supplies the already-computed [`WordSeg`] list plus the two
//! text predicates (`isWhitespaceChar` and the `PUNCTUATION_REGEX` probe), and the walks below are
//! the shared part: whichever unit `start`/`len` are counted in, the arithmetic is the same.

/// One segment of a line for word navigation — pi's `Intl.SegmentData` after `segmentWithMarkers`
/// has merged the paste markers (`editor.ts:37-90`). `start`/`len` are in whatever index unit the
/// caller segmented with: **char columns** for [`InputEditor`](super::InputEditor), **byte offsets**
/// for [`crate::text_input::Input`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct WordSeg {
    pub(crate) start: usize,
    pub(crate) len: usize,
    /// `Intl.SegmentData.isWordLike`.
    ///
    /// [CYRUP-DELTA] ICU marks a segment word-like when it is made of letters, digits, kana or
    /// ideographs; `unicode-segmentation` (UAX#29, the same algorithm without ICU's flag) exposes no
    /// such bit, so it is recomputed as "contains an alphanumeric character". The two agree on every
    /// segment UAX#29 can produce: a word-bound segment is either a run of letters/digits (with
    /// MidLetter/MidNumLet joiners), a run of punctuation/symbols, or whitespace.
    pub(crate) word_like: bool,
    /// `isAtomicSegment(segment)` — a whole `[paste #N …]` marker (`isPasteMarker`, `editor.ts:27`).
    /// Always `false` for a single-line field, which carries no markers.
    pub(crate) atomic: bool,
}

/// `findWordBackward`'s walk (`word-navigation.ts:22-68` @v0.83.0). `segs` are the segments of the
/// text **before** the cursor, in order (pi's `const textBeforeCursor = text.slice(0, cursor)`,
/// `:25` — segmenting only the PREFIX is why a marker the cursor sits inside is not atomic: it is
/// not whole in this slice).
///
/// Three branches after the whitespace skip: **one atomic segment** whole (`:44-46` — a
/// `[paste #N …]` marker is never entered, which is what makes Ctrl+W delete the marker instead of
/// chewing its closing `]`, TUI-043), **one word-like segment** truncated at its last
/// `PUNCTUATION_REGEX` match (`:47-57`), or **a whole punctuation run** (`:58-66`).
///
/// `is_ws` is pi's `isWhitespaceChar(segment)`; `last_punct_end` returns the offset just **past**
/// the last `PUNCTUATION_REGEX` match inside the segment, segment-local (`:53-56`'s
/// `lastMatch.index + lastMatch[0].length`), and `None` when the segment holds no punctuation.
pub(crate) fn find_word_backward(
    mut segs: Vec<WordSeg>,
    cursor: usize,
    is_ws: &dyn Fn(&WordSeg) -> bool,
    last_punct_end: &dyn Fn(&WordSeg) -> Option<usize>,
) -> usize {
    let mut new_cursor = cursor;

    // "Skip trailing whitespace" (`:31-38`).
    while let Some(last) = segs.last() {
        if last.atomic || !is_ws(last) {
            break;
        }
        new_cursor = new_cursor.saturating_sub(last.len);
        segs.pop();
    }
    // `if (segments.length === 0) return newCursor` (`:40`).
    let Some(&last) = segs.last() else { return new_cursor };

    if last.atomic {
        // "Skip one atomic segment" (`:44-46`).
        new_cursor = new_cursor.saturating_sub(last.len);
    } else if last.word_like {
        // "Skip inside one word-like segment, preserving ASCII punctuation boundaries" (`:47-57`):
        // back up to just after the LAST punctuation character in the segment.
        match last_punct_end(&last) {
            None => new_cursor = new_cursor.saturating_sub(last.len),
            Some(end) => new_cursor = new_cursor.saturating_sub(last.len.saturating_sub(end)),
        }
    } else {
        // "Skip non-word non-whitespace run (punctuation)" (`:58-66`).
        while let Some(last) = segs.last() {
            if last.atomic || last.word_like || is_ws(last) {
                break;
            }
            new_cursor = new_cursor.saturating_sub(last.len);
            segs.pop();
        }
    }
    new_cursor
}

/// `findWordForward`'s walk (`word-navigation.ts:76-114` @v0.83.0). `segs` are the segments of the
/// text **after** the cursor (`const textAfterCursor = text.slice(cursor)`, `:79`), in order.
///
/// The mirror of [`find_word_backward`], with the atomic skip at `:97-99` and the word-like branch
/// taking the FIRST punctuation match (`:102`, `PUNCTUATION_REGEX.exec(segment)?.index ??
/// segment.length`) — hence `first_punct` returning a segment-local offset, or `None` for "no
/// match", which means the whole segment.
pub(crate) fn find_word_forward(
    segs: &[WordSeg],
    cursor: usize,
    is_ws: &dyn Fn(&WordSeg) -> bool,
    first_punct: &dyn Fn(&WordSeg) -> Option<usize>,
) -> usize {
    let mut idx = 0usize;
    let mut new_cursor = cursor;

    // "Skip leading whitespace" (`:88-93`).
    while let Some(seg) = segs.get(idx) {
        if seg.atomic || !is_ws(seg) {
            break;
        }
        new_cursor = new_cursor.saturating_add(seg.len);
        idx = idx.saturating_add(1);
    }
    // `if (next.done) return newCursor` (`:95`).
    let Some(&next) = segs.get(idx) else { return new_cursor };

    if next.atomic {
        new_cursor = new_cursor.saturating_add(next.len);
    } else if next.word_like {
        let step = first_punct(&next).unwrap_or(next.len);
        new_cursor = new_cursor.saturating_add(step);
    } else {
        while let Some(seg) = segs.get(idx) {
            if seg.atomic || seg.word_like || is_ws(seg) {
                break;
            }
            new_cursor = new_cursor.saturating_add(seg.len);
            idx = idx.saturating_add(1);
        }
    }
    new_cursor
}

/// The ASCII punctuation that sub-divides a word-like segment — a literal port of
/// `PUNCTUATION_REGEX` (`pi/packages/tui/src/utils.ts:821` @v0.83.0):
///
/// ```text
/// /[(){}[\]<>.,;:'"!?+\-=*/\\|&%^$#@~`]/
/// ```
///
/// Deliberately **not** the complement of an `is_alphanumeric() || '_'` word-char test — the two are
/// different sets (that test rejects every non-alphanumeric; this one names 31 specific ASCII
/// characters), and word navigation must use pi's. The old class-run word motion used the former and
/// was replaced wholesale (TUI-043 / TUI-048).
pub(crate) fn is_punctuation(c: char) -> bool {
    matches!(
        c,
        '(' | ')'
            | '{'
            | '}'
            | '['
            | ']'
            | '<'
            | '>'
            | '.'
            | ','
            | ';'
            | ':'
            | '\''
            | '"'
            | '!'
            | '?'
            | '+'
            | '-'
            | '='
            | '*'
            | '/'
            | '\\'
            | '|'
            | '&'
            | '%'
            | '^'
            | '$'
            | '#'
            | '@'
            | '~'
            | '`'
    )
}

/// The word segments of `text` for a **plain single-line string** (no paste markers): pi's
/// `wordSegmenter.segment(text)` (`word-navigation.ts:3`), with `start`/`len` as **byte** offsets
/// into `text` so the caller can slice with them directly.
///
/// The editor's marker-aware twin is `InputEditor::word_segments` (`editor/motion.rs`), which is
/// char-indexed and merges `[paste #N …]` runs; a search field has neither concern.
pub(crate) fn byte_word_segments(text: &str) -> Vec<WordSeg> {
    use unicode_segmentation::UnicodeSegmentation;
    text.split_word_bound_indices()
        .map(|(start, seg)| WordSeg {
            start,
            len: seg.len(),
            word_like: seg.chars().any(char::is_alphanumeric),
            atomic: false,
        })
        .collect()
}

/// `isWhitespaceChar(segment)` = `/\s/.test(segment)` (`utils.ts:826-829`) over a byte-indexed
/// segment of `text` — *contains* whitespace, hence `any`.
pub(crate) fn byte_seg_is_whitespace(text: &str, seg: &WordSeg) -> bool {
    text.get(seg.start..seg.start.saturating_add(seg.len))
        .is_some_and(|s| s.chars().any(char::is_whitespace))
}

/// The byte offset just past the last [`is_punctuation`] char in a byte-indexed segment of `text`
/// (`word-navigation.ts:53-56`), or `None` when the segment holds none.
pub(crate) fn byte_seg_last_punct_end(text: &str, seg: &WordSeg) -> Option<usize> {
    let s = text.get(seg.start..seg.start.saturating_add(seg.len))?;
    s.char_indices()
        .rev()
        .find(|&(_, c)| is_punctuation(c))
        .map(|(i, c)| i.saturating_add(c.len_utf8()))
}

/// The byte offset of the first [`is_punctuation`] char in a byte-indexed segment of `text`
/// (`word-navigation.ts:102`'s `PUNCTUATION_REGEX.exec(segment)?.index`), or `None` when the
/// segment holds none.
pub(crate) fn byte_seg_first_punct(text: &str, seg: &WordSeg) -> Option<usize> {
    let s = text.get(seg.start..seg.start.saturating_add(seg.len))?;
    s.char_indices().find(|&(_, c)| is_punctuation(c)).map(|(i, _)| i)
}
