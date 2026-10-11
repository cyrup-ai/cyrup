//! The shared word segmenter and word walks (`crate::word_nav`, EXT-113) pinned against node
//! 22.22.0 / ICU 77.1's `Intl.Segmenter(undefined, { granularity: "word" })` — the segmenter pi's
//! `word-navigation.ts` uses.
//!
//! Every expectation below is node's output copied literally: segment text, UTF-16 `index` and
//! `isWordLike`. node indexes in UTF-16 code units and [`icu_word_segments`] in UTF-8 bytes, so
//! [`node_to_bytes`] converts explicitly rather than trusting the two to coincide (they do not
//! for any non-ASCII text).
//!
//! **Which tests are red-proofs.** [`the_iteration_mark_class_matches_node`],
//! [`the_node_sweep_matches`] and the `々` row of [`segments_and_is_word_like_match_node`] FAIL
//! against bare ICU4X (the helper before `word_nav::icu4c` existed); everything else here passes
//! there and is a non-regression guard for the shared helper and its move out of `cyrup-tui`. The
//! `/llama` red-proof is `cyrup-llama`'s `tests::ui::search_input_*`.
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
    ("々", &[("々", 0, false)]),
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
/// `__` and `‿‿` are; `々` is not. RED against the part-1 helper (bare ICU4X) on `々` only — the
/// rest pass there and are non-regression guards.
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

/// The iteration-mark class, against node 22.22.0 / ICU 77.1: a lone `々` or `〻` (`Script=Han`,
/// `Ideographic=No`) is NOT word-like (no `word.txt` status rule matches one alone); `〆`
/// (`Ideographic=Yes`, not Han) IS (`$Ideographic $ExFm* {400}`); `々々` is TWO words (ICU4C's
/// dictionary programme prices each unknown character at 255) and both are word-like (the
/// `$KanaKanji $KanaKanji {400}` chain); `人々`, `日々`, `ゝ` and `ー` are one word-like segment
/// each. RED against bare ICU4X: `々`, `〻` and `々々` word-like and `々々` one segment, `〆` not
/// word-like.
#[test]
fn the_iteration_mark_class_matches_node() {
    let mut wrong = Vec::new();
    for (text, node) in [
        ("々", vec![(0, 3, false)]),
        ("〻", vec![(0, 3, false)]),
        ("〆", vec![(0, 3, true)]),
        ("々々", vec![(0, 3, true), (3, 3, true)]),
        ("x々y", vec![(0, 1, true), (1, 3, false), (4, 1, true)]),
        ("人々", vec![(0, 6, true)]),
        ("日々", vec![(0, 6, true)]),
        ("ゝ", vec![(0, 3, true)]),
        ("ー", vec![(0, 3, true)]),
    ] {
        let got = icu_word_segments(text);
        if got != node {
            wrong.push(format!("{text:?}: got {got:?}, node {node:?}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} strings disagree with node:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// node 22.22.0 / ICU 77.1's `Intl.Segmenter(undefined, { granularity: "word" })` over 11,882
/// strings, from `word_nav_node_sweep.tsv` (one string per line: the text as a JSON string, a
/// tab, then each segment's UTF-8 byte length with `*` when `isWordLike`). The strings: every
/// code point of U+3000–U+303F, U+3040–U+30FF and U+FF00–U+FFEF, and of printable ASCII and
/// Latin-1, alone, doubled, tripled and beside `日`/`あ`/`カ`/`a`/`1`/space; every pair of the
/// iteration, prolongation and sound marks (`々 〻 ゝ ゞ ヽ ヾ 〆 〇 ー 〃 ｰ ゛ ゜`) alone and beside
/// `日`/`あ`/`カ`/`人`/`時`; and a corpus of Japanese, Chinese and Korean sentences. Produced with
/// `[...seg.segment(t)].map(s => [Buffer.byteLength(t.slice(0, s.index)),
/// Buffer.byteLength(s.segment), s.isWordLike])` on node 22.22.0.
///
/// Excluded as accepted upward drift (`EXT-119`, record-only `[CYRUP-DELTA]`): strings containing U+00B8 CEDILLA, which
/// ICU 77.1 (Unicode 16) segments as `Word_Break=Other` and ICU4X 2.3.0's data (ICU 78 / Unicode
/// 17) as `ALetter` — a Unicode-version difference, not an algorithm one.
///
/// RED against bare ICU4X: 1,223 of the 11,876 strings differed — in boundaries (ICU4X's greedy
/// dictionary match against ICU4C's cost programme: `います` is `いま`/`す` there and `い`/`ます` in
/// node; `々々` one segment against two; `人゛゛` split where node keeps it whole) and in
/// `isWordLike` (`々`, `〻`, `〆`, a lone `＿`).
#[test]
fn the_node_sweep_matches() {
    let data = include_str!("word_nav_node_sweep.tsv");
    let mut checked = 0usize;
    let mut wrong = Vec::new();
    for line in data.lines() {
        let (text, segments) = line.split_once('\t').expect("a tab");
        let text: String = serde_json::from_str(text).expect("a JSON string");
        if text.contains('\u{b8}') {
            continue;
        }
        let mut node = Vec::new();
        let mut start = 0usize;
        for segment in segments.split(' ') {
            let (len, word_like) = match segment.strip_suffix('*') {
                Some(len) => (len, true),
                None => (segment, false),
            };
            let len: usize = len.parse().expect("a length");
            node.push((start, len, word_like));
            start += len;
        }
        checked += 1;
        let got = icu_word_segments(&text);
        if got != node {
            wrong.push(format!("{text:?}: got {got:?}, node {node:?}"));
        }
    }
    assert_eq!(checked, 11_876, "the fixture changed size");
    assert!(
        wrong.is_empty(),
        "{} of {checked} strings disagree with node, first 40:\n{}",
        wrong.len(),
        wrong
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
