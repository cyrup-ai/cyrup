//! Batch 6 — the adversarial-review follow-ups on the batch-5 rhythm work.
//!
//! Same rules as [`vertical_rhythm`](super::vertical_rhythm): every assertion is anchored to a
//! quoted pi v0.84.1 line and paired with a MIRROR covering the shape that must NOT change.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use crate::transcript::*;

fn txt(line: &Line<'static>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}
fn texts(lines: &[Line<'static>]) -> Vec<String> {
    lines.iter().map(txt).collect()
}
/// The grapheme clusters of a rendered row set, whitespace dropped — whitespace is exactly what
/// wrapping is allowed to remove (`utils.ts:906`, `:935`), and nothing else is.
fn clusters(rows: &[Line<'static>]) -> Vec<String> {
    rows.iter()
        .flat_map(|r| {
            txt(r).graphemes(true).map(str::to_string).collect::<Vec<_>>()
        })
        .filter(|g| !g.trim().is_empty())
        .collect()
}

/// L6, the half the batch left undone — `wrap_line` must measure and break on GRAPHEME CLUSTERS.
///
/// Upstream never sees a `char`: `splitIntoTokensWithAnsi` builds its tokens from
/// `graphemeSegmenter.segment(...)` (`tui/src/utils.ts:775-798`) and `breakLongWord` re-segments
/// the over-wide token the same way before measuring each piece (`:977-980`, `:994-1012`).
/// Breaking per `char` severs a ZWJ emoji sequence from its joiner and a combining mark from its
/// base, which is a correctness bug rather than a spacing one — and it measures differently from
/// [`apply_bg`](crate::transcript::layout::apply_bg), which is the very disagreement L6 is about.
#[test]
fn wrap_line_breaks_on_graphemes_not_chars() {
    // ONE unbroken token (no spaces) so it takes the `breakLongWord` path, mixing a ZWJ family
    // emoji, a combining-mark sequence and wide CJK.
    let src = "AAAA\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}BBe\u{301}\u{65e5}\u{672c}\u{8a9e}";
    let rows = wrap_line(&Line::raw(src.to_string()), 8);
    assert!(rows.len() > 1, "the token must hard-break at width 8: {:?}", texts(&rows));
    for (i, r) in rows.iter().enumerate() {
        assert!(r.width() <= 8, "row {i} overflows: {} cols {:?}", r.width(), txt(r));
    }
    // No cluster was split: re-segmenting the produced rows yields the source's clusters, in
    // order. A per-`char` break emits `"\u{1f468}"` then a bare `"\u{200d}"`, which does not.
    let want: Vec<String> =
        src.graphemes(true).filter(|g| !g.trim().is_empty()).map(str::to_string).collect();
    assert_eq!(clusters(&rows), want, "a grapheme cluster was torn: {:?}", texts(&rows));
    assert!(
        rows.iter().any(|r| txt(r).contains('\u{1f468}')
            && txt(r).contains('\u{1f466}')),
        "the ZWJ family landed on one row whole: {:?}",
        texts(&rows)
    );
    assert!(
        !rows.iter().any(|r| txt(r).starts_with('\u{200d}') || txt(r).starts_with('\u{301}')),
        "a row began with an orphaned joiner/combining mark: {:?}",
        texts(&rows)
    );

    // A long unbroken run of WIDE characters: 26 CJK ideographs, 52 columns, at width 10.
    let cjk = "\u{8a9e}".repeat(26);
    let wide = wrap_line(&Line::raw(cjk.clone()), 10);
    for (i, r) in wide.iter().enumerate() {
        assert!(r.width() <= 10, "wide row {i} overflows: {} cols", r.width());
    }
    assert_eq!(clusters(&wide).len(), 26, "characters were lost or duplicated");

    // MIRROR: the token-width SUM leg is grapheme-measured too, so a space-separated wide string
    // wraps on its spaces and every row still fits.
    let words = "\u{65e5}\u{672c}\u{8a9e} \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466} abc \u{65e5}\u{672c}\u{8a9e}";
    let ws = wrap_line(&Line::raw(words.to_string()), 9);
    for (i, r) in ws.iter().enumerate() {
        assert!(r.width() <= 9, "row {i} overflows: {} cols {:?}", r.width(), txt(r));
    }
    assert_eq!(
        clusters(&ws),
        words
            .graphemes(true)
            .filter(|g| !g.trim().is_empty())
            .map(str::to_string)
            .collect::<Vec<String>>()
    );

    // MIRROR: the fast path is untouched — a line that already fits comes back byte-identical.
    let fits = Line::raw("short".to_string());
    assert_eq!(texts(&wrap_line(&fits, 20)), vec!["short".to_string()]);
}

/// L3 — the gate is `hasVisibleContent` (`assistant-message.ts:96-98`):
///
/// ```ts
/// const hasVisibleContent = message.content.some(
///   (c) => (c.type === "text" && c.text.trim()) || (c.type === "thinking" && c.thinking.trim()),
/// );
/// if (hasVisibleContent) { this.contentContainer.addChild(new Spacer(1)); }
/// ```
///
/// Both legs are **trimmed**, and `:107` gates the text block's `Markdown` child on the same
/// `content.text.trim()`. A turn of nothing but spaces therefore gets neither a body nor a
/// blank. `commit_assistant` tested `!t.is_empty()`, which let `"   "` through.
#[test]
fn whitespace_only_assistant_turn_is_not_visible_content() {
    let theme = UiTheme::dark();

    // Committed leg: a whitespace-only turn never becomes an entry.
    let mut view = TranscriptView::new();
    view.commit_assistant(Some("   \n\t ".to_string()));
    assert!(view.pending().is_empty(), "whitespace-only turn committed: {:?}", view.pending());

    // …and the render arm refuses it independently, because `Entry::Assistant` is public.
    assert!(entry_lines(&Entry::Assistant("  ".into()), &theme, 40, 1, ImageOpts::default())
        .is_empty());

    // Streaming leg: the live region shows nothing at all until real text arrives.
    let mut live = TranscriptView::new();
    live.push_assistant_delta("  ");
    assert!(live.lines(40, &theme).is_empty(), "{:?}", texts(&live.lines(40, &theme)));

    // MIRROR: real content still gets exactly one leading blank, on both legs.
    let mut ok = TranscriptView::new();
    ok.commit_assistant(Some("hi".to_string()));
    let committed = entry_lines(&ok.pending()[0], &theme, 40, 1, ImageOpts::default());
    assert_eq!(texts(&committed), vec!["".to_string(), " hi".to_string()]);
    live.push_assistant_delta("hi");
    assert_eq!(texts(&live.lines(40, &theme)), vec!["".to_string(), " hi".to_string()]);
}

/// The `hasVisibleContent` fix must not disarm the reasoning leg: a whitespace-only THINKING
/// buffer is invisible for the same reason (`:97`'s `c.thinking.trim()`), but a real one still
/// carries the blank, and the `hasVisibleContentAfter` blank between the two (`:135-137`,
/// `:166-168`) is likewise gated on the trimmed answer text.
#[test]
fn live_thinking_and_answer_spacers_use_the_trimmed_predicate() {
    let theme = UiTheme::dark();
    let mut view = TranscriptView::new();
    view.set_hide_thinking_block(true);

    view.push_thinking_delta("  ");
    assert!(view.lines(40, &theme).is_empty(), "whitespace-only reasoning is not visible");

    view.push_thinking_delta("musing");
    assert_eq!(
        texts(&view.lines(40, &theme)),
        vec!["".to_string(), format!(" {HIDDEN_THINKING_LABEL}")],
        "one leading blank, no trailing one — nothing visible follows"
    );

    // A whitespace-only answer must NOT open the `hasVisibleContentAfter` gap.
    view.push_assistant_delta("   ");
    assert_eq!(
        texts(&view.lines(40, &theme)),
        vec!["".to_string(), format!(" {HIDDEN_THINKING_LABEL}")]
    );

    // MIRROR: a real answer does. (The buffer is now `"   done"`; `Markdown` drops the leading
    // whitespace, so the row is the `outputPad` column plus the text.)
    view.push_assistant_delta("done");
    assert_eq!(
        texts(&view.lines(40, &theme)),
        vec![
            "".to_string(),
            format!(" {HIDDEN_THINKING_LABEL}"),
            "".to_string(),
            " done".to_string(),
        ]
    );
}

/// The leading `Spacer(1)` may not outlive the component it introduces. `box.ts:75-77` and
/// `:91-93` both `return []` for an empty child set, and upstream never reaches the spacer in
/// that case either — `interactive-mode.ts:3499`'s `if (textContent)` skips the whole
/// `case "user"`, spacer included. Prepending it unconditionally left a bare blank row.
#[test]
fn a_component_that_renders_nothing_gets_no_leading_spacer() {
    let theme = UiTheme::dark();
    let empty = Entry::User { text: String::new(), lead_spacer: true };
    assert!(
        entry_lines(&empty, &theme, 40, 1, ImageOpts::default()).is_empty(),
        "orphan blank ahead of an empty user box"
    );
    // The same for the labeled shell, exercised through `box_lines` directly.
    assert!(box_lines(Vec::new(), 40, 1, 1, Style::default()).is_empty());

    // MIRROR: real text still gets the blank, the tinted paddingY row and the inset body.
    let real = Entry::User { text: "hello".into(), lead_spacer: true };
    let rows = entry_lines(&real, &theme, 40, 1, ImageOpts::default());
    assert_eq!(txt(&rows[0]), "", "leading Spacer(1)");
    assert_eq!(rows[0].width(), 0, "the Spacer is outside the Box, so it is not filled");
    assert_eq!(rows[1].width(), 40, "the Box's top paddingY row IS filled");
    assert!(txt(&rows[2]).starts_with(" hello"));
}

/// X18 — `showStatus` (`interactive-mode.ts:3411-3429`) puts a status line in the chat container
/// like any other child:
///
/// ```ts
/// const spacer = new Spacer(1);
/// const text = new Text(theme.fg("dim", message), 1, 0);
/// this.chatContainer.addChild(spacer);
/// this.chatContainer.addChild(text);
/// ```
///
/// So: a leading blank, then a `dim` `Text` at **paddingX 1** — `Text.render` emits
/// `leftMargin + line + rightMargin` (`text.ts:70-76`) and wraps at `width - paddingX * 2`
/// (`:64`). No bullet is interpolated anywhere; the `• ` prefix and the flush-left placement
/// were cyrup inventions.
#[test]
fn status_row_is_a_spacer_plus_a_one_column_inset_dim_text() {
    let theme = UiTheme::dark();
    let rows = entry_lines(&Entry::Status("Model: opus".into()), &theme, 40, 1, ImageOpts::default());
    assert_eq!(texts(&rows), vec!["".to_string(), " Model: opus".to_string()]);
    assert!(!txt(&rows[1]).contains('\u{2022}'), "invented bullet: {:?}", txt(&rows[1]));
    assert_eq!(rows[1].spans[1].style, theme.dim_style(), "`theme.fg(\"dim\", message)`");

    // The inset does not depend on `outputPad` — `new Text(…, 1, 0)` hard-codes paddingX 1.
    let flush = entry_lines(&Entry::Status("Model: opus".into()), &theme, 40, 0, ImageOpts::default());
    assert_eq!(texts(&flush), vec!["".to_string(), " Model: opus".to_string()]);

    // MIRROR: a long status wraps at `contentWidth = width - 2` and every row keeps the inset.
    let long = entry_lines(
        &Entry::Status("aaaa bbbb cccc dddd eeee ffff gggg".into()),
        &theme,
        16,
        1,
        ImageOpts::default(),
    );
    assert!(long.len() > 2, "a long status must wrap: {:?}", texts(&long));
    for row in long.iter().skip(1) {
        assert!(txt(row).starts_with(' '), "row lost its inset: {:?}", txt(row));
        assert!(row.width() <= 16, "row overflows: {:?}", txt(row));
    }
}

/// The first child of a fresh session's chat gets NO leading blank:
/// `interactive-mode.ts:3500` is `if (this.chatContainer.children.length > 0) { …
/// addChild(new Spacer(1)) }`. The neighbouring call sites are deliberately different — `:3484`
/// (compaction), `:3491` (branch) and `:3514` (the user message trailing a skill block) are all
/// UNgated — so the gate is per call site, not a global rule.
#[test]
fn the_first_chat_child_gets_no_leading_spacer() {
    let theme = UiTheme::dark();
    let mut view = TranscriptView::new();
    view.push_user("first");
    let first = view.drain_committed();
    assert!(matches!(first[0], Entry::User { lead_spacer: false, .. }), "{:?}", first[0]);
    let rows = entry_lines(&first[0], &theme, 40, 1, ImageOpts::default());
    assert_eq!(rows[0].width(), 40, "row 0 is the Box's tinted paddingY row, not a Spacer");
    assert!(txt(&rows[1]).starts_with(" first"), "{:?}", texts(&rows));

    // MIRROR: the SECOND message gets one — and it still does after the first was flushed to
    // native scrollback, which is why the answer cannot be read off `pending` at render time.
    view.push_user("second");
    let second = view.drain_committed();
    assert!(matches!(second[0], Entry::User { lead_spacer: true, .. }), "{:?}", second[0]);
    let srows = entry_lines(&second[0], &theme, 40, 1, ImageOpts::default());
    assert_eq!(txt(&srows[0]), "");
    assert_eq!(srows[0].width(), 0, "the Spacer is untinted and unpadded");

    // A live streaming turn is a chat child too (`AssistantMessageComponent` is in
    // `chatContainer`), so a user message that follows one is never "first".
    let mut streamed = TranscriptView::new();
    streamed.push_assistant_delta("hi");
    streamed.push_user("after a stream");
    assert!(matches!(streamed.pending()[0], Entry::User { lead_spacer: true, .. }));

    // `:3500` covers the SKILL component too (`:3506` sits inside it), while the user message
    // that trails the skill block (`:3513-3521`) has its own unconditional spacer.
    let mut skill = TranscriptView::new();
    skill.push_user("<skill name=\"deploy\" location=\"a\">\nrun it\n</skill>\n\nand then?");
    let entries = skill.drain_committed();
    assert!(
        matches!(entries[0], Entry::SkillInvocation { lead_spacer: false, .. }),
        "{:?}",
        entries[0]
    );
    assert!(matches!(entries[1], Entry::User { lead_spacer: true, .. }), "{:?}", entries[1]);
    assert_eq!(
        txt(&entry_lines(&entries[1], &theme, 40, 1, ImageOpts::default())[0]),
        ""
    );

    // MIRROR: the ungated call sites are unaffected — a branch summary opening a fresh session
    // still leads with its blank (`:3491`).
    let branch =
        entry_lines(
            &Entry::BranchSummary { summary: "merged".into() },
            &theme,
            40,
            1,
            ImageOpts { tools_expanded: true, ..ImageOpts::default() },
        );
    assert_eq!(txt(&branch[0]), "");
    assert_eq!(branch[0].width(), 0, "`:3491`'s Spacer is outside the Box");
}

/// **`Entry::Block` — the body is rendered at `width - 2`, not at `width`.**
///
/// The stack is `Markdown(body, 1, 1, theme)` (`interactive-mode.ts:6201`, and the identical
/// `/changelog` site at `:6071`), and `Markdown.render` opens with
/// `const contentWidth = Math.max(1, width - this.paddingX * 2)` (`markdown.ts:284`) — paddingX
/// is 1, so the body wraps at two columns narrower than the rule above it and is then inset by
/// `leftMargin = " ".repeat(this.paddingX)` (`:328`). Rendering the body at the full `width` put
/// a row of body text one column wider than the block it sits in, and the inset then pushed it
/// past the right edge.
#[test]
fn block_body_wraps_at_width_minus_two() {
    let theme = UiTheme::dark();
    // 19 columns of body inside a 20-column block: fits at `width`, must NOT fit at `width - 2`.
    let body = "aaaaaaaaa bbbbbbbbb";
    assert_eq!(Line::raw(body).width(), 19);
    let rows = entry_lines(
        &Entry::Block { title: "T".into(), markdown: body.into() },
        &theme,
        20,
        1,
        ImageOpts::default(),
    );
    let text: Vec<String> = texts(&rows);
    assert!(
        text.iter().any(|r| r.trim() == "aaaaaaaaa") && text.iter().any(|r| r.trim() == "bbbbbbbbb"),
        "the body did not wrap at `width - 2` — it was rendered at the full width: {text:?}"
    );
    for row in &rows {
        assert!(row.width() <= 20, "a row overflowed the block: {:?}", txt(row));
    }
    // `leftMargin` — every body row carries the one-column inset (`markdown.ts:328-340`).
    for row in rows.iter().filter(|r| txt(r).contains('a') || txt(r).contains('b')) {
        assert!(txt(row).starts_with(' '), "body row lost `leftMargin`: {:?}", txt(row));
    }

    // MIRROR — the two `─` rules are the one thing that DOES run edge to edge (`DynamicBorder`
    // is a chat child with no padding at all), so the block is 20 wide even though its body is 18.
    assert_eq!(txt(&rows[1]), "─".repeat(20), "the opening rule is full width");
    assert_eq!(
        txt(rows.last().unwrap()),
        "─".repeat(20),
        "the closing rule is full width"
    );
}

/// **`Entry::Block` — an EMPTY body contributes no rows, not two blank ones.**
///
/// `Markdown.render` returns `[]` on blank text at `markdown.ts:288-296`:
///
/// ```ts
/// if (!text || text.trim() === "") {
///     const result: string[] = [];
///     …
///     return result;
/// }
/// ```
///
/// That early return is BEFORE the `paddingY` block at `:352-361`, so the component's own
/// blank rows above and below the body never materialize. Emitting them anyway left a
/// bodyless block (a `/changelog` with no entries is the live case) four rows tall with a
/// hollow gap the upstream never draws.
#[test]
fn block_with_an_empty_body_emits_no_padding_rows() {
    let theme = UiTheme::dark();
    let rule = "─".repeat(24);
    let empty = entry_lines(
        &Entry::Block { title: "What's New".into(), markdown: String::new() },
        &theme,
        24,
        1,
        ImageOpts::default(),
    );
    assert_eq!(
        texts(&empty),
        vec!["".to_string(), rule.clone(), " What's New".to_string(), String::new(), rule.clone()],
        "an empty body must add nothing between the title's trailing blank and the closing rule"
    );

    // Whitespace-only is the same case — the guard is `text.trim() === ""`, not `!text`.
    let blank = entry_lines(
        &Entry::Block { title: "What's New".into(), markdown: "  \n\n \t".into() },
        &theme,
        24,
        1,
        ImageOpts::default(),
    );
    assert_eq!(texts(&blank), texts(&empty), "a whitespace-only body is a blank body");

    // MIRROR — a real body DOES bring the `paddingY` pair with it (`:352-361`), so the two
    // shapes differ by exactly the body plus its two blanks.
    let full = entry_lines(
        &Entry::Block { title: "What's New".into(), markdown: "hello".into() },
        &theme,
        24,
        1,
        ImageOpts::default(),
    );
    assert_eq!(full.len(), empty.len() + 3, "body + one blank above + one below: {:?}", texts(&full));
    assert_eq!(txt(&full[4]), "", "paddingY row above the body");
    assert_eq!(txt(&full[5]).trim(), "hello");
    assert_eq!(txt(&full[6]), "", "paddingY row below the body");
}
