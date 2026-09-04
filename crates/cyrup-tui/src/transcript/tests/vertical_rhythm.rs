//! Batch 5 — transcript vertical rhythm (TUI-FIDELITY L1, L3, L5, L6, X1, X2, X5, X10).
//!
//! Every assertion here is anchored to a quoted pi v0.84.1 line; each test is paired with a MIRROR
//! assertion covering the shape that must NOT change.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use ratatui::style::{Color, Modifier};

use crate::transcript::*;

fn txt(line: &Line<'static>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}
fn texts(lines: &[Line<'static>]) -> Vec<String> {
    lines.iter().map(txt).collect()
}

fn committed_tool(result: Value) -> Entry {
    let mut view = TranscriptView::new();
    view.push_tool_start("bash", serde_json::json!({ "command": "cat wide.txt" }));
    view.push_tool_end("bash", false, Some(result));
    view.commit_tools();
    view.drain_committed().into_iter().next().unwrap()
}

/// L1 — `box.ts:106-119`: `for (i < paddingY) result.push(this.applyBg("", width))` before the
/// content and again after it, with every caller passing `paddingY = 1`
/// (`tool-execution.ts:68`). Both rows are `applyBg`-filled, so they carry the state tint across
/// the whole width — they are the block's visible top and bottom edge.
///
/// L5 — `box.ts:79-88`: the child renders at `contentWidth = width - paddingX * 2` and is then
/// shifted right by `leftPad`, leaving a one-column tinted gutter on BOTH sides.
#[test]
fn tool_block_has_tinted_padding_rows_and_a_gutter_on_both_sides() {
    let theme = UiTheme::dark();
    let entry = committed_tool(serde_json::json!({
        "content": [{ "type": "text", "text": "abc" }],
    }));
    let lines = entry_lines(&entry, &theme, 30, 1, ImageOpts::default());
    let tint = theme.tool_bg_style(Style::default(), true, false).bg;
    assert!(tint.is_some(), "the dark theme defines a tool tint");

    // Row 0 is the component's own untinted `Spacer(1)` (`tool-execution.ts:63`).
    assert_eq!(txt(&lines[0]), "", "leading Spacer(1)");
    assert_eq!(
        lines[0].style.bg, None,
        "the Spacer is OUTSIDE the Box, so it is untinted"
    );
    // Rows 1 and N-1 are the Box's paddingY rows: blank, full width, tinted.
    let last = lines.len() - 1;
    for i in [1usize, last] {
        assert_eq!(
            txt(&lines[i]).trim(),
            "",
            "row {i} is blank: {:?}",
            txt(&lines[i])
        );
        assert_eq!(lines[i].width(), 30, "row {i} fills the width");
        assert_eq!(lines[i].style.bg, tint, "row {i} carries the tint");
    }
    // Content rows sit at column 1 and are padded to the full width, so the last column is a
    // tinted blank — the right gutter.
    let header = &lines[2];
    assert!(
        txt(header).starts_with(" $ "),
        "1-column inset: {:?}",
        txt(header)
    );
    assert_eq!(header.width(), 30, "content row fills the width");

    // MIRROR: an EMPTY block renders nothing at all — `box.ts:75-77` / `:91-93` return `[]`
    // before any padding row is pushed, so a contentless Box never leaves two stray tinted rows.
    assert!(box_lines(Vec::new(), 30, 1, 1, Style::default().bg(Color::Red)).is_empty());
}

/// L5, the load-bearing half — `box.ts:85` renders the child at `contentWidth`, so a line longer
/// than the frame is broken at `width - paddingX * 2` and the last column of every row stays a
/// tinted blank. Sizing the child at the full `width` instead (what cyrup did) lets output run
/// flush into column N-1 with no gutter on the right.
#[test]
fn tool_block_content_is_sized_to_width_minus_both_paddings() {
    let theme = UiTheme::dark();
    // A single unbroken 25-column token: it is hard-broken at `contentWidth`, so the break
    // point is exactly what `contentWidth` is. At `width - 2` the first row's ink is 1 + 18 = 19
    // and column 19 stays a tinted blank; sized at the full `width` it would be 1 + 20 = 21 and
    // overflow the frame entirely.
    let long = "abcdefghijklmnopqrstuvwxy 0123456789";
    let entry = committed_tool(serde_json::json!({
        "content": [{ "type": "text", "text": long }],
    }));
    let lines = entry_lines(&entry, &theme, 20, 1, ImageOpts::default());
    let body: Vec<&Line<'static>> = lines
        .iter()
        .filter(|l| {
            let t = txt(l);
            t.contains("abcdefghijklmnopqr") || t.contains("stuvwxy")
        })
        .collect();
    assert!(
        body.len() > 1,
        "the long line must wrap inside the Box: {:?}",
        texts(&lines)
    );
    for row in &body {
        assert_eq!(row.width(), 20, "every row still fills the frame");
        // The INK — everything before the right pad — must stop at or before column
        // `paddingX + contentWidth` = 19, leaving column 19 (0-indexed) as the tinted gutter.
        let ink = Line::raw(txt(row).trim_end().to_string()).width();
        assert!(
            ink <= 19,
            "row ran into the right gutter ({ink} cols): {:?}",
            txt(row)
        );
        assert!(
            txt(row).starts_with(' '),
            "row lost its left inset: {:?}",
            txt(row)
        );
    }

    // MIRROR: a SHORT line is not broken and is not indented twice.
    let short = committed_tool(serde_json::json!({
        "content": [{ "type": "text", "text": "zz" }],
    }));
    let sl = entry_lines(&short, &theme, 20, 1, ImageOpts::default());
    assert_eq!(sl.iter().filter(|l| txt(l).contains("zz")).count(), 1);
}

/// L6 — `box.ts:127-131` measures with `visibleWidth(line)`, which counts terminal COLUMNS. The
/// old `chars().count()` under-counts every wide glyph, so the pad was too long and the tinted
/// row overflowed the frame into a spurious extra row.
#[test]
fn tool_block_background_is_measured_in_columns_not_chars() {
    let theme = UiTheme::dark();
    // Eight CJK ideographs: 8 chars, 16 columns.
    let wide = committed_tool(serde_json::json!({
        "content": [{ "type": "text", "text": "日本語のテキスト" }],
    }));
    let lines = entry_lines(&wide, &theme, 30, 1, ImageOpts::default());
    for (i, line) in lines.iter().enumerate() {
        assert!(
            line.width() <= 30,
            "row {i} overflows the frame: {} cols",
            line.width()
        );
    }
    let body = lines.iter().find(|l| txt(l).contains('日')).unwrap();
    assert_eq!(
        body.width(),
        30,
        "the wide row is padded to exactly the width, not past it"
    );

    // MIRROR: the same number of NARROW characters still lands on exactly the width — the fix is
    // a change of measure, not a change of target.
    let narrow = committed_tool(serde_json::json!({
        "content": [{ "type": "text", "text": "abcdefgh" }],
    }));
    let nlines = entry_lines(&narrow, &theme, 30, 1, ImageOpts::default());
    let nbody = nlines.iter().find(|l| txt(l).contains("abcdefgh")).unwrap();
    assert_eq!(nbody.width(), 30);
}

/// L3 — `assistant-message.ts:100-102`. The blank is gated on `hasVisibleContent` (`:96-98`),
/// which is exactly the condition `commit_assistant` / `commit_thinking` already gate the entry
/// on, so an empty turn emits neither entry nor blank.
#[test]
fn assistant_and_thinking_each_lead_with_the_spacer() {
    let theme = UiTheme::dark();
    let a = entry_lines(
        &Entry::Assistant("hi".into()),
        &theme,
        40,
        1,
        ImageOpts::default(),
    );
    assert_eq!(txt(&a[0]), "", "assistant leading Spacer(1)");
    assert_eq!(txt(&a[1]), " hi");

    let t = Entry::Thinking {
        text: "musing".into(),
        hidden: true,
    };
    let tl = entry_lines(&t, &theme, 40, 1, ImageOpts::default());
    assert_eq!(txt(&tl[0]), "", "thinking leading Spacer(1)");
    assert_eq!(txt(&tl[1]), format!(" {HIDDEN_THINKING_LABEL}"));

    // A thinking run followed by the answer reproduces upstream's
    // `[Spacer] thinking [Spacer] text` (`:100-102` + `:166-168`).
    let seq: Vec<String> = tl.iter().chain(a.iter()).map(txt).collect();
    assert_eq!(seq, vec!["", " Thinking...", "", " hi"]);

    // MIRROR: an empty turn commits no entry, so no orphan blank can reach scrollback.
    let mut view = TranscriptView::new();
    view.commit_assistant(Some(String::new()));
    view.commit_thinking(Some("   ".into()));
    assert!(view.pending().is_empty(), "empty content must not commit");
}

/// X2 — `custom-message.ts:94`, `branch-summary-message.ts:37` and
/// `compaction-summary-message.ts:38` each `addChild(new Spacer(1))` right after the label
/// `Text`. `skill-invocation-message.ts` does NOT: `:36-45` is label then `Markdown`, with no
/// spacer between them. The label itself is `theme.fg("customMessageLabel", "\x1b[1m[…]\x1b[22m")`
/// inside a `Box(1, 1, customMessageBg)`, so it is inset one column and banded.
#[test]
fn label_blocks_space_after_the_label_except_skill() {
    let theme = UiTheme::dark();
    let branch = entry_lines(
        // `tools_expanded: true` — this test is about the EXPANDED body's spacer
        // (`branch-summary-message.ts:37` then `:39-45`); X14's collapsed arm is covered by
        // `x14_collapsed_branch_summary_is_one_hint_row`.
        &Entry::BranchSummary {
            summary: "we merged".into(),
        },
        &theme,
        40,
        1,
        ImageOpts {
            tools_expanded: true,
            ..ImageOpts::default()
        },
    );
    let b = texts(&branch);
    assert_eq!(b[0], "", "leading Spacer(1) (interactive-mode.ts:3491)");
    assert_eq!(b[1].trim(), "", "Box top paddingY");
    assert_eq!(b[2].trim_end(), " [branch]", "label, inset 1");
    assert_eq!(
        b[3].trim(),
        "",
        "Spacer(1) after the label (branch-summary-message.ts:37)"
    );
    assert_eq!(b[b.len() - 1].trim(), "", "Box bottom paddingY");

    // MIRROR: `[skill]` has NO spacer after its label — the body follows immediately.
    let skill = entry_lines(
        &Entry::SkillInvocation {
            name: "deploy".into(),
            content: "run it".into(),
            lead_spacer: true,
        },
        &theme,
        40,
        1,
        ImageOpts::default(),
    );
    let s = texts(&skill);
    assert_eq!(s[2].trim_end(), " [skill]", "label, inset 1");
    assert_eq!(
        s[3].trim_end(),
        " deploy",
        "the body starts on the very next row"
    );

    // Every row of either block carries the `customMessageBg` band, padding rows included.
    let band = theme.custom_message_bg_style().bg;
    assert!(band.is_some());
    for (i, line) in branch.iter().enumerate().skip(1) {
        assert_eq!(line.style.bg, band, "branch row {i} is unbanded");
        assert_eq!(line.width(), 40, "branch row {i} does not fill the width");
    }
}

/// X5 — `assistant-message.ts:146-164` renders the reasoning body through a real `Markdown`
/// with `{ color: thinkingText, italic: true }`. Because that pair only reaches
/// `applyDefaultStyle` (`markdown.ts:377-404`), a heading keeps `mdHeading` (`:470-480`) instead
/// of being flattened into the thinking colour.
#[test]
fn thinking_body_is_markdown_not_flat_text() {
    let theme = UiTheme::dark();
    let e = Entry::Thinking {
        text: "## Plan\n\nthen do it".into(),
        hidden: false,
    };
    let lines = entry_lines(&e, &theme, 40, 0, ImageOpts::default());
    let heading = lines.iter().find(|l| txt(l).contains("Plan")).unwrap();
    // The literal `## ` is consumed by the renderer (level < 3 prints no prefix).
    assert_eq!(
        txt(heading),
        "Plan",
        "markdown was not parsed: {:?}",
        txt(heading)
    );
    let hs = heading.spans[0].style;
    assert_eq!(
        hs.fg,
        theme.md_heading_style().fg,
        "heading kept its own colour"
    );

    let prose = lines
        .iter()
        .find(|l| txt(l).contains("then do it"))
        .unwrap();
    let ps = prose.spans[0].style;
    assert_eq!(
        ps.fg,
        theme.thinking_text_style().fg,
        "prose takes the thinkingText colour"
    );
    assert!(
        ps.add_modifier.contains(Modifier::ITALIC),
        "prose takes `italic: true`"
    );

    // MIRROR: the HIDDEN form is still one plain `Text` line (`:141-143`), not markdown.
    let hidden = Entry::Thinking {
        text: "## Plan".into(),
        hidden: true,
    };
    let hl = entry_lines(&hidden, &theme, 40, 0, ImageOpts::default());
    assert_eq!(
        texts(&hl),
        vec!["".to_string(), HIDDEN_THINKING_LABEL.to_string()]
    );
}

/// X10 — `bash.ts:311` and `:317` both build their row as `new Text(`\n${…}`, 0, 0)`. The
/// leading `\n` makes `wrapTextWithAnsi` (`utils.ts:839`) emit an empty first row, so the
/// truncation warning and the `Took Ns` footer are each preceded by a blank.
#[test]
fn bash_tool_warnings_and_duration_each_get_a_leading_blank() {
    let theme = UiTheme::dark();
    let entry = committed_tool(serde_json::json!({
        "content": [{ "type": "text", "text": "out" }],
        "details": { "fullOutputPath": "/tmp/full.txt" },
    }));
    let lines = entry_lines(&entry, &theme, 40, 1, ImageOpts::default());
    let rows = texts(&lines);
    let warn = rows.iter().position(|r| r.contains("Full output")).unwrap();
    assert_eq!(rows[warn - 1].trim(), "", "blank before the warning row");
    let took = rows.iter().position(|r| r.contains("Took ")).unwrap();
    assert_eq!(rows[took - 1].trim(), "", "blank before the duration row");

    // MIRROR: the blanks belong to the warning/duration rows, not to the output — a result with
    // neither still ends on the Box's single bottom padding row.
    let plain = committed_tool(serde_json::json!({
        "content": [{ "type": "text", "text": "out" }],
    }));
    let pl = texts(&entry_lines(&plain, &theme, 40, 1, ImageOpts::default()));
    assert!(!pl.iter().any(|r| r.contains("Full output")));
    assert_eq!(pl[pl.len() - 1].trim(), "");
    assert_ne!(pl[pl.len() - 2].trim(), "", "no doubled trailing blank");
}
