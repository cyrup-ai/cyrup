#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::editor::wrap::{display_width, word_wrap_line};
use crate::editor::*;

/// `word_wrap_line` over a `&str`, as `(chunk text)` — the shape `wordWrapLine` returns
/// (`editor.ts:121-213 @v0.85.1` yields `{ text, startIndex, endIndex }`).
fn wrap(s: &str, width: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    word_wrap_line(&chars, width, None)
        .into_iter()
        .map(|(start, len)| chars.iter().skip(start).take(len).collect())
        .collect()
}

/// The chunks tile the line: contiguous, gap-free, and reassembling to the input. Upstream gets
/// this for free from `line.slice(chunkStart, …)` + a final `line.slice(chunkStart)` (`:202`).
fn assert_tiles(s: &str, width: usize) {
    let chars: Vec<char> = s.chars().collect();
    let segs = word_wrap_line(&chars, width, None);
    let mut at = 0usize;
    for (start, len) in &segs {
        assert_eq!(
            *start, at,
            "chunk {segs:?} of {s:?}@{width} is not contiguous"
        );
        at += len;
    }
    assert_eq!(at, chars.len(), "chunks {segs:?} do not cover {s:?}");
}

// -------------------------------------------------------------- wordWrapLine ---------------

/// The two early returns (`editor.ts:115-122`): an empty line is one empty chunk, and a line
/// that already fits is one chunk covering the whole line.
#[test]
fn a_line_that_fits_is_one_chunk() {
    assert_eq!(word_wrap_line(&[], 10, None), vec![(0, 0)]);
    assert_eq!(wrap("hello", 10), vec!["hello"]);
    // "fits" is measured in COLUMNS: 5 ideographs are 10 of them.
    assert_eq!(wrap("日本語です", 10), vec!["日本語です"]);
    assert_eq!(
        wrap("日本語です", 9).len(),
        2,
        "…and 9 columns is one short"
    );
}

/// Whitespace is the primary wrap opportunity, and the break lands AFTER the space run so the
/// trailing space stays on the wrapped row (`wrapOppIndex = next.index`, `editor.ts:187-189`).
#[test]
fn wrapping_breaks_after_the_last_space_that_fits() {
    assert_eq!(wrap("aaa bbb ccc", 5), vec!["aaa ", "bbb ", "ccc"]);
    assert_eq!(
        wrap("aaa  bbb", 5),
        vec!["aaa  ", "bbb"],
        "a run of spaces joins (`:187`)"
    );
    // And it is GREEDY, not balanced: at width 7 the tail `"bbb ccc"` is exactly 7 columns, so
    // the backtrack at the second space finds it already fits and never fires
    // (`currentWidth - wrapOppWidth + gWidth <= maxWidth`, `editor.ts:147`).
    assert_eq!(wrap("aaa bbb ccc", 7), vec!["aaa ", "bbb ccc"]);
    assert_tiles("aaa bbb ccc", 5);
    assert_tiles("aaa bbb ccc", 7);
}

/// A word longer than the width force-breaks at the current grapheme's own start index
/// (`editor.ts:154-160`).
#[test]
fn an_overlong_word_force_breaks_at_the_width() {
    assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    assert_tiles("abcdefghij", 4);
}

/// **The display-width bug.** `visibleWidth(grapheme)` (`editor.ts:139-143`), not a char count:
/// 24 ideographs are 48 columns and cannot be one 39-column row. The break also needs
/// `cjkBreakRegex` (`utils.ts:54`, used at `editor.ts:191-198`) — CJK has no spaces to break at,
/// so without the CJK opportunity the whole run would be one unbreakable "word".
#[test]
fn cjk_is_measured_and_broken_in_columns() {
    let cjk: String = "日本語".chars().cycle().take(24).collect();
    let rows = wrap(&cjk, 39);
    assert_eq!(rows.len(), 2, "48 columns do not fit 39: {rows:?}");
    assert_eq!(
        rows[0].chars().count(),
        19,
        "19 ideographs are 38 columns; a 20th would be 40"
    );
    assert_eq!(rows.concat(), cjk);
    assert_tiles(&cjk, 39);
    for r in &rows {
        assert!(display_width(r) <= 39, "row overflows: {r:?}");
    }
}

/// The CJK opportunity is a BOUNDARY rule — it fires when either side is CJK
/// (`editor.ts:194-198`), so Latin text abutting CJK may break between them.
#[test]
fn a_latin_cjk_boundary_is_a_wrap_opportunity() {
    // `word` is 4 columns and each ideograph 2, so at width 4 the opportunity recorded at the
    // `d`→`日` boundary is what puts `word` on a row of its own; the two that follow come from
    // the CJK-to-CJK opportunities.
    let rows = wrap("word日本語", 4);
    assert_eq!(
        rows,
        vec!["word", "日本", "語"],
        "the boundary breaks: {rows:?}"
    );
    assert_tiles("word日本語", 4);
    // Contrast: an all-Latin run of the same length has NO opportunity anywhere, so it
    // force-breaks mid-"word" instead.
    assert_eq!(wrap("wordabcdef", 4), vec!["word", "abcd", "ef"]);
}

/// A grapheme CLUSTER is atomic: the break never lands inside one, whatever the width.
#[test]
fn a_cluster_is_never_split() {
    const FAMILY: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
    let line = format!("{}{FAMILY}", "a".repeat(38));
    let rows = wrap(&line, 39);
    assert_eq!(
        rows,
        vec!["a".repeat(38), FAMILY.to_string()],
        "torn cluster: {rows:?}"
    );
    assert_tiles(&line, 39);

    // …including when the cluster ALONE is wider than the width: it is indivisible, so it takes
    // a row of its own rather than recursing forever the way `editor.ts:170-186` would for a splittable one.
    let rows = wrap(&format!("ab{FAMILY}cd"), 1);
    assert_eq!(rows, vec!["a", "b", FAMILY, "c", "d"], "{rows:?}");
    assert_tiles(&format!("ab{FAMILY}cd"), 1);
}

/// Every produced row fits, and the rows always tile the input — swept over a spread of widths
/// and mixed scripts, because the failure mode of the old char-count wrap was silent overflow
/// rather than a crash.
#[test]
fn every_wrapped_row_fits_its_width() {
    const CASES: [&str; 5] = [
        "the quick brown fox jumps over the lazy dog",
        "日本語のテキストは空白で区切られていません",
        "mixed 日本語 and latin with  double  spaces",
        "e\u{301}combining\u{301}marks\u{301}everywhere",
        "supercalifragilisticexpialidocious",
    ];
    for s in CASES {
        for width in 1..=45usize {
            assert_tiles(s, width);
            for row in wrap(s, width) {
                // A single over-wide cluster is the one legal exception (see above): it cannot
                // be split, so it is emitted alone.
                let clusters = row.graphemes(true).count();
                assert!(
                    display_width(&row) <= width || clusters == 1,
                    "{s:?}@{width}: row {row:?} is {} columns",
                    display_width(&row)
                );
            }
        }
    }
}

// ------------------------------------------------- paste-marker-aware wrapping --------------

/// An editor whose registry holds paste `#1`, so `[paste #1 …]` in a test line passes pi's
/// `validIds` gate (`segmentWithMarkers`, `editor.ts:44`/`:57`) and merges.
fn editor_with_paste_1() -> InputEditor {
    let mut ed = InputEditor::new();
    ed.handle_paste(&"a".repeat(1500));
    assert_eq!(ed.text(), "[paste #1 1500 chars]");
    ed
}

/// `word_wrap_line` over `s` with the MARKER-MERGED segments the real callers pass
/// (`wordWrapLine(line, W, [...this.segment(line, "grapheme")])`, `editor.ts:1020`/`:1837`).
fn wrap_marked(ed: &InputEditor, s: &str, width: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let joined: String = chars.iter().collect();
    let segs = ed.wrap_segments(&chars, &joined);
    word_wrap_line(&chars, width, Some(&segs))
        .into_iter()
        .map(|(start, len)| chars.iter().skip(start).take(len).collect())
        .collect()
}

/// **The torn-marker bug.** `wordWrapLine` takes its segments from the caller
/// (`preSegmented`, `editor.ts:121`/`:132`) and both real callers hand it the MARKER-MERGED list
/// (`:1020`, `:1837`: `wordWrapLine(line, W, [...this.segment(line, "grapheme")])`). A
/// `[paste #N …]` marker is therefore ONE segment, so the wrap breaks before it or after it —
/// never inside it.
///
/// `"prefix [paste #1 1500 chars]"` is 28 columns; at width 24 the whole marker must move to the
/// second row. With plain grapheme segments the last whitespace opportunity is the space INSIDE
/// the marker (abs col 22) and the rows come out `"prefix [paste #1 1500 "` / `"chars]"`.
#[test]
fn a_paste_marker_is_never_torn_across_visual_lines() {
    let mut ed = InputEditor::new();
    ed.insert_str("prefix ");
    ed.handle_paste(&"a".repeat(1500));
    assert_eq!(ed.text(), "prefix [paste #1 1500 chars]");
    ed.set_view_width(24);
    let map = ed.visual_line_map();
    assert_eq!(map.len(), 2, "{map:?}");
    assert_eq!(map[1].start, 7, "the marker starts row 2: {map:?}");
    assert_eq!(map[1].len, 21, "…and the whole of it is there: {map:?}");
    // Height measurement runs through the same segmentation (`visual_line_count` is the
    // `buildVisualLineMap(width).length` the box is sized from) — if it kept plain graphemes the
    // editor would be drawn at a different row count than it measures.
    assert_eq!(ed.visual_line_count(24), 2);
}

/// A marker too wide for the frame IS split — but by upstream's recursion (`editor.ts:170-186`),
/// which re-wraps the composite segment at plain-grapheme granularity and resumes at its last
/// sub-chunk. The 21-column marker does not fit 20 columns, so it breaks at its own inner space;
/// what must hold is that the recursion TERMINATES, loses no characters, and leaves every row
/// inside the width.
#[test]
fn an_over_wide_paste_marker_is_re_wrapped_not_dropped() {
    let mut ed = InputEditor::new();
    ed.insert_str("prefix ");
    ed.handle_paste(&"a".repeat(1500));
    ed.set_view_width(20);
    let map = ed.visual_line_map();
    let line: Vec<char> = ed.text().chars().collect();
    let mut at = 0usize;
    for vl in &map {
        assert_eq!(vl.logical, 0);
        assert_eq!(vl.start, at, "rows do not tile: {map:?}");
        at += vl.len;
        let row: String = line.iter().skip(vl.start).take(vl.len).collect();
        assert!(
            display_width(&row) <= 20,
            "row {row:?} is {} columns",
            display_width(&row)
        );
    }
    assert_eq!(at, line.len(), "rows do not cover the line: {map:?}");
    assert_eq!(ed.visual_line_count(20), map.len());
}

/// The two marker guards inside the loop, each isolated by a width that makes it decide the break.
///
/// `:147` — `isWs` excludes a merged marker. `"[paste #1 1500 chars]tail"` at width 23: the marker
/// is NOT whitespace, so no opportunity is recorded after it and the overflow at `i` force-breaks
/// mid-`tail`. Drop the `!seg.atomic &&` term and the marker (which contains two spaces) reads as
/// whitespace, an opportunity lands at `tail`, and the rows become `["[paste #1 1500 chars]",
/// "tail"]` — a break pi does not make.
///
/// `:196` — `isPasteMarker(next.segment) ||`. `"xx [paste #1 1500 chars]yyy"` at width 25: the
/// space at col 2 is a break opportunity even though what follows it is a marker whose own text is
/// full of spaces. Drop the `next.atomic ||` term and no opportunity is recorded there, so the
/// overflow three columns later force-breaks inside the marker instead.
#[test]
fn the_marker_guards_decide_where_the_break_lands() {
    let ed = editor_with_paste_1();
    assert_eq!(
        wrap_marked(&ed, "[paste #1 1500 chars]tail", 23),
        vec!["[paste #1 1500 chars]ta", "il"],
        "a marker is not whitespace (`editor.ts:147`)"
    );
    assert_eq!(
        wrap_marked(&ed, "xx [paste #1 1500 chars]yyy", 25),
        vec!["xx ", "[paste #1 1500 chars]yyy"],
        "a space before a marker still breaks (`editor.ts:196`)"
    );
    // And with the marker unregistered there is nothing to merge: pi's `validIds` gate (`:44`)
    // leaves plain graphemes, which break at the marker's inner spaces like any other text.
    let bare = InputEditor::new();
    assert_eq!(
        wrap_marked(&bare, "xx [paste #1 1500 chars]yyy", 25),
        wrap("xx [paste #1 1500 chars]yyy", 25)
    );
}
