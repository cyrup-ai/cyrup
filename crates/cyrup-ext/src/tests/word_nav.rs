//! The shared word segmenter and word walks (`crate::word_nav`, EXT-113) pinned against node
//! 22.22.0 / ICU 77.1's `Intl.Segmenter(undefined, { granularity: "word" })` — the segmenter pi's
//! `word-navigation.ts` uses.
//!
//! Every expectation below is node's output copied literally: segment text, UTF-16 `index` and
//! `isWordLike`. node indexes in UTF-16 code units and [`icu_word_segments`] in UTF-8 bytes, so
//! [`node_to_bytes`] converts explicitly rather than trusting the two to coincide (they do not
//! for any non-ASCII text).
//!
//! **Non-regression guards.** These tests pass against the segmenter as it stood in `cyrup-tui`
//! before the move (TUI-048 had already made it ICU): they guard the move and the helper itself.
//! The red-proof for EXT-113 is in `cyrup-llama`'s `tests::ui::search_input_*`, which drive the
//! `/llama` search box that did NOT use this helper before.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::word_nav::{byte_find_word_backward, byte_find_word_forward, icu_word_segments};

/// node's `[{segment, index, isWordLike}]` for `text`, with `index` converted from UTF-16 code
/// units to a UTF-8 byte offset, as `(byte_start, byte_len, is_word_like)` — the shape
/// [`icu_word_segments`] returns.
fn node_to_bytes(text: &str, node: &[NodeSeg]) -> Vec<(usize, usize, bool)> {
    node.iter()
        .map(|&(segment, utf16_index, word_like)| {
            let mut units = 0usize;
            let mut byte = text.len();
            for (offset, c) in text.char_indices() {
                if units == utf16_index {
                    byte = offset;
                    break;
                }
                units += c.len_utf16();
            }
            assert_eq!(
                text.get(byte..byte + segment.len()),
                Some(segment),
                "node's UTF-16 index {utf16_index} for {segment:?} in {text:?} did not convert"
            );
            (byte, segment.len(), word_like)
        })
        .collect()
}

/// One node `Intl.SegmentData`: `(segment, utf16 index, isWordLike)`.
type NodeSeg = (&'static str, usize, bool);

/// `(text, node segments)` from `node-segments.txt` (node 22.22.0, ICU 77.1).
const NODE: &[(&str, &[NodeSeg])] = &[
    (
        "日本語のモデル",
        &[("日本語", 0, true), ("の", 3, true), ("モデル", 4, true)],
    ),
    ("½", &[("½", 0, false)]),
    ("²", &[("²", 0, false)]),
    ("①", &[("①", 0, false)]),
    ("__", &[("__", 0, true)]),
    ("‿‿", &[("‿‿", 0, true)]),
    (
        "hello world",
        &[("hello", 0, true), (" ", 5, false), ("world", 6, true)],
    ),
    ("foo.bar", &[("foo.bar", 0, true)]),
    (
        "日本語のモデル foo_bar ½x",
        &[
            ("日本語", 0, true),
            ("の", 3, true),
            ("モデル", 4, true),
            (" ", 7, false),
            ("foo_bar", 8, true),
            (" ", 15, false),
            ("½", 16, false),
            ("x", 17, true),
        ],
    ),
];

/// Boundaries AND `isWordLike` for every pinned string match node: `日本語のモデル` is the three
/// dictionary words `日本語` / `の` / `モデル`, all word-like; `½`, `²` and `①` are not word-like;
/// `__` and `‿‿` are. `々` is the one pinned string that does NOT match — see
/// [`a_lone_iteration_mark_is_word_like_under_icu4x_unlike_node`]. Non-regression guard (see the
/// module doc).
#[test]
fn segments_and_is_word_like_match_node() {
    let mut wrong = Vec::new();
    for &(text, node) in NODE {
        let want = node_to_bytes(text, node);
        let got = icu_word_segments(text);
        if got != want {
            wrong.push(format!("{text:?}: got {got:?}, node {want:?}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} strings disagree with node:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// Every caret stop of `walk` from `start` until it stops moving.
fn stops(text: &str, start: usize, walk: fn(&str, usize) -> usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut at = start;
    loop {
        let next = walk(text, at);
        if next == at {
            return out;
        }
        out.push(next);
        at = next;
    }
}

/// pi's `findWordBackward` / `findWordForward` over the shared segmenter, against a JS port of
/// `word-navigation.ts` run on node 22.22.0 (UTF-16 stops converted to bytes). Non-regression
/// guard (see the module doc).
#[test]
fn byte_word_walks_match_node() {
    for (text, back, fwd) in [
        ("日本語のモデル", vec![12, 9, 0], vec![9, 12, 21]),
        (
            "日本語のモデル foo_bar ½x",
            vec![32, 30, 22, 12, 9, 0],
            vec![9, 12, 21, 29, 32, 33],
        ),
        ("x①①", vec![1, 0], vec![1, 7]),
        ("a.__", vec![2, 1, 0], vec![1, 2, 4]),
        ("a ‿‿", vec![2, 0], vec![1, 8]),
        (
            "foo bar.baz qux",
            vec![12, 8, 7, 4, 0],
            vec![3, 7, 8, 11, 15],
        ),
    ] {
        assert_eq!(
            stops(text, text.len(), byte_find_word_backward),
            back,
            "backward over {text:?}"
        );
        assert_eq!(
            stops(text, 0, byte_find_word_forward),
            fwd,
            "forward over {text:?}"
        );
    }
}

/// A cursor that is not on a char boundary, or past the end, never panics: backward leaves it
/// where it is, forward clamps a past-the-end cursor to the end.
#[test]
fn byte_word_walks_tolerate_bad_cursors() {
    let text = "日本語";
    assert_eq!(byte_find_word_backward(text, 1), 1);
    assert_eq!(byte_find_word_forward(text, 1), 1);
    assert_eq!(byte_find_word_forward(text, 99), text.len());
    assert_eq!(byte_find_word_backward("", 0), 0);
}

/// **KNOWN DIVERGENCE, not pi's answer.** node 22.22.0 / ICU 77.1 says a lone `々` (U+3005
/// IDEOGRAPHIC ITERATION MARK) is one segment with `isWordLike == false`; ICU4X 2.3.0's
/// `WordSegmenter::new_auto` says `true`. This test pins ICU4X's CURRENT answer so that a change
/// (an `icu_segmenter` upgrade, or a fix) turns it red and forces the ledger to be looked at; when
/// it is fixed, flip the assertion to node's `false` and fold the string back into
/// [`segments_and_is_word_like_match_node`].
///
/// The cause, read in both sources: ICU4C (`icu4c/source/data/brkitr/rules/word.txt` at
/// `release-77-1`) gives a dictionary range the rule status of the RULE pass over that range —
/// `$KanaKanji $KanaKanji {400}` for a run of two or more, and for one character only
/// `$Katakana`/`$Hiragana`/`$Ideographic {400}`; `々` is `Script=Han` but `Ideographic=No`, so alone
/// it matches no status rule and is 0 (not word-like). ICU4X tags every segment its dictionary pass
/// produces `WordType::Letter` unconditionally (`ComplexWord::BREAK_STATUS`,
/// `icu_segmenter-2.3.0/src/word.rs`). Probed further against node: `〻` (also `Script=Han`,
/// `Ideographic=No`) diverges the same way (true here, node false); `〆` diverges the OTHER way
/// (false here, node true — it is `Script=Common`, `Ideographic=Yes`, so not dictionary text in
/// either, and ICU4C's `$Ideographic {400}` rule tags it where ICU4X's rule table does not); `々々`
/// is one segment here where node gives two; `人々`, `日々`, `ゝ` and `ー` agree. The difference is in the segmenter, so it is the
/// same in the main editor (TUI-048) as in the overlay search boxes — EXT-113's one-segmenter
/// change neither introduced nor fixed it.
#[test]
fn a_lone_iteration_mark_is_word_like_under_icu4x_unlike_node() {
    assert_eq!(icu_word_segments("々"), vec![(0, 3, true)]);
}
