//! Markdown + syntax rendering tests (spec/tui/06 §2-§3).
//!
//! Exercises the `pulldown-cmark` walk (`render_markdown`) and the `syntect` fenced-code highlight,
//! plus the streaming partial-fence trim, asserting on the produced styled `Line`s and on a rendered
//! `TestBackend` buffer.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::{
    render_markdown, render_markdown_with_hyperlinks, trim_partial_closing_fence, App, UiTheme,
};
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};
use ratatui::text::Line;

/// The dark theme's body/prose foreground (`text` = `#d4d4d4`) — the colour Pi's *unstyled* table
/// chrome inherits, since `markdown.ts:956/971/976/1003` pass no theme function at all.
const BODY_FG: Color = Color::Rgb(0xd4, 0xd4, 0xd4);
/// `mdHr` = `gray` `#808080` (`dark.json:56`) — what the table frame must NOT be.
const MD_HR: Color = Color::Rgb(0x80, 0x80, 0x80);
/// `mdHeading` = `#f0c674` (`dark.json:52`) — what a table header cell must NOT be.
const MD_HEADING: Color = Color::Rgb(0xf0, 0xc6, 0x74);

/// The effective foreground of a line: its own line style, else the first span that sets one.
fn line_fg(line: &Line<'_>) -> Option<Color> {
    line.style.fg.or_else(|| line.spans.iter().find_map(|s| s.style.fg))
}

/// The first line whose text contains `needle`.
fn find_row<'a, 'b>(lines: &'a [Line<'b>], needle: &str) -> &'a Line<'b> {
    lines
        .iter()
        .find(|l| line_text(l).contains(needle))
        .unwrap_or_else(|| panic!("no row containing {needle:?} in {:?}", rows(lines)))
}

/// True when the row is blank (no spans, or only whitespace).
fn is_blank(line: &Line<'_>) -> bool {
    line_text(line).trim().is_empty()
}

/// The plain text of a line (span contents concatenated).
fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// All produced lines as text rows.
fn rows(lines: &[Line<'_>]) -> Vec<String> {
    lines.iter().map(line_text).collect()
}

/// True if any span across all lines has exactly `content` with foreground `color`.
fn has_span_colored(lines: &[Line<'_>], content: &str, color: Color) -> bool {
    lines
        .iter()
        .flat_map(|l| &l.spans)
        .any(|s| s.content == content && s.style.fg == Some(color))
}

/// True if any span has exactly `content` carrying the given modifier.
fn has_span_mod(lines: &[Line<'_>], content: &str, m: Modifier) -> bool {
    lines
        .iter()
        .flat_map(|l| &l.spans)
        .any(|s| s.content == content && s.style.add_modifier.contains(m))
}

#[test]
fn headings_drop_hash_for_h1_h2_and_keep_it_for_h3_plus() {
    let theme = UiTheme::dark();
    let lines = render_markdown("## Plan\n\nbody\n", 80, &theme);
    let text = rows(&lines).join("\n");
    // H2: no `#` prefix, bold mdHeading (#f0c674 in the dark theme).
    assert!(text.contains("Plan"), "heading text missing:\n{text}");
    assert!(!text.contains("## Plan"), "H2 kept its hash prefix:\n{text}");
    assert!(
        has_span_colored(&lines, "Plan", Color::Rgb(0xf0, 0xc6, 0x74)),
        "heading not in mdHeading color:\n{text}"
    );
    assert!(has_span_mod(&lines, "Plan", Modifier::BOLD), "heading not bold");

    // H4 keeps a literal `#### ` prefix (markdown.ts:336-362).
    let h4 = render_markdown("#### Deep\n", 80, &theme);
    assert!(rows(&h4).join("\n").contains("#### Deep"), "H4 lost its hash prefix");

    // Item #9 — H1 is heading + bold + UNDERLINE (Pi markdown.ts:344-345); H2 is bold only, NOT
    // underlined.
    let h1 = render_markdown("# Title\n", 80, &theme);
    assert!(has_span_mod(&h1, "Title", Modifier::BOLD), "H1 not bold");
    assert!(has_span_mod(&h1, "Title", Modifier::UNDERLINED), "H1 must be underlined");
    assert!(!has_span_mod(&lines, "Plan", Modifier::UNDERLINED), "H2 must NOT be underlined");
}

#[test]
fn unordered_and_ordered_lists_render_markers() {
    let theme = UiTheme::dark();
    let ul = rows(&render_markdown("- one\n- two\n", 80, &theme));
    assert!(ul.iter().any(|r| r.starts_with("- one")), "unordered bullet missing:\n{ul:?}");

    // Ordered lists renumber from the list's start, even when the source repeats `1.`.
    let ol = rows(&render_markdown("1. a\n1. b\n", 80, &theme));
    assert!(ol.iter().any(|r| r.starts_with("1. a")), "ordered #1 missing:\n{ol:?}");
    assert!(ol.iter().any(|r| r.starts_with("2. b")), "ordered renumber to 2 missing:\n{ol:?}");
}

#[test]
fn blockquote_prefixes_border_and_hr_is_a_rule() {
    let theme = UiTheme::dark();
    let bq = render_markdown("> note\n", 80, &theme);
    let bq_rows = rows(&bq);
    assert!(bq_rows.iter().any(|r| r.starts_with("│ ")), "blockquote border missing:\n{bq_rows:?}");
    assert!(bq_rows.iter().any(|r| r.contains("note")), "blockquote body missing:\n{bq_rows:?}");

    let hr = render_markdown("---\n", 80, &theme);
    assert!(rows(&hr).iter().any(|r| r.chars().all(|c| c == '─') && !r.is_empty()), "hr missing");
}

#[test]
fn inline_code_bold_and_links() {
    let theme = UiTheme::dark();
    // inline code → mdCode (= accent #8abeb7), no surrounding backticks.
    let code = render_markdown("write `out.json` now\n", 80, &theme);
    assert!(
        has_span_colored(&code, "out.json", Color::Rgb(0x8a, 0xbe, 0xb7)),
        "inline code not in mdCode/accent color:\n{:?}",
        rows(&code)
    );
    assert!(!rows(&code).join("").contains('`'), "inline code kept its backticks");

    // bold.
    let bold = render_markdown("a **b** c\n", 80, &theme);
    assert!(has_span_mod(&bold, "b", Modifier::BOLD), "bold span not bold");

    // link → text underlined + trailing ` (url)` on a terminal WITHOUT OSC-8 (`markdown.ts:697-707`).
    // Driven through the explicit-capability entry point so the assertion does not depend on the
    // TERM_PROGRAM/TMUX of whoever runs the suite.
    let src = "[docs](https://example.com)\n";
    let link = render_markdown_with_hyperlinks(src, 80, &theme, false);
    let lt = rows(&link).join("\n");
    assert!(has_span_mod(&link, "docs", Modifier::UNDERLINED), "link text not underlined");
    assert!(lt.contains("(https://example.com)"), "link url not appended:\n{lt}");

    // …and the DEFAULT entry point must agree with the explicit one at the capability it actually
    // reads. `render_markdown` → `render_with_default_style` → `crate::image::hyperlinks_supported()`
    // (`markdown.rs`), so pinning it to `render_markdown_with_hyperlinks(…, hyperlinks_supported())`
    // keeps `render_markdown` itself under test: moving the link arm to a test-only override, or
    // changing which capability `render` dispatches on, breaks this and nothing else here.
    let detected = cyrup_tui::hyperlinks_supported();
    assert_eq!(
        render_markdown(src, 80, &theme),
        render_markdown_with_hyperlinks(src, 80, &theme, detected),
        "render_markdown must render exactly as the explicit entry point at the detected \
         OSC-8 capability ({detected}) — spans and styles, not just text"
    );
}

#[test]
fn m14_empty_link_text_still_gets_the_url_suffix() {
    // The fallback test upstream is EXACTLY `token.text === token.href || token.text ===
    // hrefForComparison` (`markdown.ts:701-702`) — there is no `text.length > 0` clause. `[](href)`
    // has `token.text === ""`, which equals neither, so upstream DOES emit ` (href)`. An emptiness
    // guard here swallowed the link entirely: the row rendered as nothing at all.
    let theme = UiTheme::dark();
    let plain = rows(&render_markdown_with_hyperlinks("see [](https://example.com/x) here\n", 200, &theme, false))
        .join("\n");
    assert_eq!(plain, "see  (https://example.com/x) here", "empty-texted link lost its URL:\n{plain}");

    // MIRROR — an OSC-8-capable terminal still prints no inline URL (`markdown.ts:692-696`), so the
    // empty text really is empty there.
    let capable = rows(&render_markdown_with_hyperlinks("see [](https://example.com/x) here\n", 200, &theme, true))
        .join("\n");
    assert_eq!(capable, "see  here", "OSC-8 terminal must not print the URL inline:\n{capable}");

    // MIRROR — a non-empty text that differs from the href is unchanged, and one that equals the
    // href still gets no suffix.
    let named = rows(&render_markdown_with_hyperlinks("[docs](https://example.com/x)\n", 200, &theme, false))
        .join("\n");
    assert_eq!(named, "docs (https://example.com/x)", "{named}");
    let bare = rows(&render_markdown_with_hyperlinks("<https://example.com/x>\n", 200, &theme, false))
        .join("\n");
    assert_eq!(bare, "https://example.com/x", "{bare}");
}

#[test]
fn fenced_code_block_highlights_known_language() {
    let theme = UiTheme::dark();
    let md = "```rust\nfn main() {}\n```\n";
    let lines = render_markdown(md, 80, &theme);
    let text = rows(&lines).join("\n");
    // Literal fence lines are emitted (markdown.ts:380,393).
    assert!(text.contains("```rust"), "opening fence line missing:\n{text}");
    // The `fn` keyword is highlighted with syntaxKeyword (#569CD6 in the dark theme).
    assert!(
        has_span_colored(&lines, "fn", Color::Rgb(0x56, 0x9C, 0xD6)),
        "rust keyword not syntax-highlighted:\n{text}"
    );
}

#[test]
fn unknown_language_code_renders_flat() {
    let theme = UiTheme::dark();
    // No info string → flat mdCodeBlock (green) body, 2-space indented, no keyword coloring.
    let lines = render_markdown("```\nsome plain text\n```\n", 80, &theme);
    let body = lines
        .iter()
        .find(|l| line_text(l).contains("some plain text"))
        .expect("code body line missing");
    assert!(line_text(body).starts_with("  some plain text"), "code body not 2-space indented");
    // mdCodeBlock = "green" var in the dark theme; assert the body span is not the keyword blue.
    assert!(
        !has_span_colored(&lines, "some", Color::Rgb(0x56, 0x9C, 0xD6)),
        "flat code wrongly highlighted as a keyword"
    );
}

#[test]
fn trim_partial_closing_fence_keeps_open_block_stable() {
    // An open code block whose last streamed line is a partial fence (`` `` `` shorter than the ```` ``` ````
    // opener) has that partial line stripped so the renderer does not flip open/closed (markdown.ts:25-48).
    let streaming = "```rust\nfn main() {}\n``";
    let trimmed = trim_partial_closing_fence(streaming);
    assert!(!trimmed.contains("``\n") && !trimmed.ends_with("``"), "partial fence not trimmed: {trimmed:?}");
    assert!(trimmed.contains("fn main"), "code body lost during trim");

    // A complete block is untouched.
    let complete = "```rust\nfn main() {}\n```";
    assert_eq!(trim_partial_closing_fence(complete), complete);
    // Plain prose (no open fence) is untouched.
    assert_eq!(trim_partial_closing_fence("just text"), "just text");
}

#[test]
fn markdown_assistant_entry_reaches_scrollback_multiline() {
    // A committed assistant turn with markdown lands in native scrollback as multiple lines.
    // X1: no `assistant: ` label — `assistant-message.ts:104-114` adds one `Markdown` child per text
    // block and nothing else.
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    app.transcript_mut().commit_assistant(Some("## Plan\n\n- step one\n- step two".to_string()));
    app.draw().unwrap();
    let sb = app.scrollback_text();
    assert!(!sb.contains("assistant:"), "invented assistant label in scrollback:\n{sb}");
    assert!(sb.contains("Plan"), "heading missing from scrollback:\n{sb}");
    assert!(sb.contains("- step one"), "list item missing from scrollback:\n{sb}");
    assert!(sb.contains("- step two"), "second list item missing from scrollback:\n{sb}");
}

#[test]
fn streaming_markdown_partial_renders_in_viewport() {
    // A live (uncommitted) assistant partial with markdown renders inline in the viewport (no box),
    // markdown-formatted, with the soft cursor on the last line.
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    app.transcript_mut().push_assistant_delta("# Title\n\nstreaming body");
    app.draw().unwrap();
    let buf = app.terminal().backend().buffer();
    let mut text = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(c) = buf.cell((x, y)) {
                text.push_str(c.symbol());
            }
        }
        text.push('\n');
    }
    assert!(text.contains("Title"), "streaming heading missing from viewport:\n{text}");
    assert!(text.contains("streaming body"), "streaming body missing from viewport:\n{text}");
    // X1: pi draws no streaming caret. The `▌` was cyrup-only — `git grep "▌" v0.84.1 -- packages/`
    // finds one hit and it is the pupil of an eye in `examples/extensions/custom-header.ts:22`.
    assert!(!text.contains('▌'), "invented soft streaming cursor:\n{text}");
}

#[test]
fn tables_render_as_a_box_drawing_grid() {
    // gap 12: tables render as a full `┌┬┐ ├┼┤ └┴┘ │ ─` grid (markdown.ts:803-851), not ` │ `-joined.
    let theme = UiTheme::dark();
    let md = "| Name | Role |\n|------|------|\n| Ada | math |\n| Alan | logic |";
    let lines = render_markdown(md, 40, &theme);
    let text = rows(&lines).join("\n");
    assert!(text.contains('┌') && text.contains('┐'), "top corners present:\n{text}");
    assert!(text.contains('├') && text.contains('┼') && text.contains('┤'), "separator row:\n{text}");
    assert!(text.contains('└') && text.contains('┴') && text.contains('┘'), "bottom corners:\n{text}");
    // Header + body cells survive, framed by `│`.
    assert!(text.contains("Name") && text.contains("Role"), "header cells:\n{text}");
    assert!(text.contains("Ada") && text.contains("logic"), "body cells:\n{text}");
    assert!(text.contains('│'), "vertical bars:\n{text}");
}

// ── batch 7: presentation fidelity M1-M4, M6, M8, M11, M14, M16, M17 ──────────────────────────────

#[test]
fn m1_m2_table_frame_and_header_take_body_colour_not_mdhr_or_mdheading() {
    // M1: every grid row upstream is a plain template string — `` `┌─${…join("─┬─")}─┐` ``
    // (`markdown.ts:956`), `` `│ ${rowParts.join(" │ ")} │` `` (`:971`), `` `├─…─┼─…─┤` `` (`:976`),
    // `` `└─…─┴─…─┘` `` (`:1003`). None of the four is passed through `this.theme.*`, so the frame
    // is body-coloured, never `mdHr`.
    // M2: header cells are `this.theme.bold(padded)` (`:966-970`) = `theme.bold` = `chalk.bold`
    // (`theme.ts:384-386`, `:1264`) — SGR-1 only, adding no foreground of its own.
    let theme = UiTheme::dark();
    let md = "| Name | Role |\n|------|------|\n| Ada | math |";
    let lines = render_markdown(md, 40, &theme);

    for glyph in ["┌", "├", "└"] {
        let row = find_row(&lines, glyph);
        assert_eq!(
            line_fg(row),
            Some(BODY_FG),
            "border row {glyph} must be body-coloured, got {:?}",
            line_fg(row)
        );
        assert_ne!(line_fg(row), Some(MD_HR), "border row {glyph} is still mdHr grey");
    }

    // The `│` separators of the header band and of a body row.
    for needle in ["Name", "Ada"] {
        let row = find_row(&lines, needle);
        for bar in row.spans.iter().filter(|s| s.content.contains('│')) {
            assert_eq!(
                bar.style.fg,
                Some(BODY_FG),
                "`│` separator around {needle:?} must be body-coloured, got {:?}",
                bar.style.fg
            );
        }
    }

    // The header cell: bold, body foreground, NOT mdHeading amber.
    let header_cell = find_row(&lines, "Name")
        .spans
        .iter()
        .find(|s| s.content.contains("Name"))
        .expect("header cell span");
    assert!(header_cell.style.add_modifier.contains(Modifier::BOLD), "header cell not bold");
    assert_eq!(header_cell.style.fg, Some(BODY_FG), "header cell must be body-coloured");
    assert_ne!(header_cell.style.fg, Some(MD_HEADING), "header cell is still mdHeading amber");

    // A body cell stays un-bold — the header/body difference is exactly SGR-1.
    let body_cell = find_row(&lines, "Ada")
        .spans
        .iter()
        .find(|s| s.content.contains("Ada"))
        .expect("body cell span");
    assert!(!body_cell.style.add_modifier.contains(Modifier::BOLD), "body cell wrongly bold");

    // MIRROR — the roles the table stopped using are untouched elsewhere: a `---` rule is still
    // `mdHr` (`markdown.ts:606`) and a heading is still `mdHeading` + bold (`:472-474`).
    let hr = render_markdown("---\n\nx\n", 80, &theme);
    let rule = find_row(&hr, "───");
    assert_eq!(line_fg(rule), Some(MD_HR), "the `---` rule must stay mdHr grey");
    let head = render_markdown("## Plan\n", 80, &theme);
    assert!(has_span_colored(&head, "Plan", MD_HEADING), "heading must stay mdHeading");
    assert!(has_span_mod(&head, "Plan", Modifier::BOLD), "heading must stay bold");
}

#[test]
fn m3_nested_lists_indent_four_columns_per_level() {
    // `const indent = "    ".repeat(depth);` — FOUR spaces per nesting level (`markdown.ts:758`).
    let theme = UiTheme::dark();
    let md = "- top\n  - one\n    - two\n";
    let r = rows(&render_markdown(md, 80, &theme));
    assert!(r.iter().any(|l| l == "- top"), "depth 0 must be flush left:\n{r:?}");
    assert!(r.iter().any(|l| l == "    - one"), "depth 1 must indent 4 columns:\n{r:?}");
    assert!(r.iter().any(|l| l == "        - two"), "depth 2 must indent 8 columns:\n{r:?}");

    // MIRROR — a flat list is unindented at every item.
    let flat = rows(&render_markdown("- a\n- b\n", 80, &theme));
    assert!(flat.iter().any(|l| l == "- a") && flat.iter().any(|l| l == "- b"), "{flat:?}");
}

#[test]
fn m4_task_list_items_keep_the_bullet_before_the_box() {
    // `const marker = bullet + taskMarker` where `bullet = "- "` and `taskMarker = "[x] "`
    // (`markdown.ts:765-773`), the whole marker in `listBullet` (`:774`).
    let theme = UiTheme::dark();
    let lines = render_markdown("- [ ] todo\n- [x] done\n", 80, &theme);
    let r = rows(&lines);
    assert!(r.iter().any(|l| l == "- [ ] todo"), "unchecked marker must be `- [ ] `:\n{r:?}");
    assert!(r.iter().any(|l| l == "- [x] done"), "checked marker must be `- [x] `:\n{r:?}");
    // The bullet and the box are one `listBullet` run, not two differently-styled spans.
    assert!(has_span_colored(&lines, "- [x] ", theme.md_list_bullet_style().fg.unwrap()), "{r:?}");

    // MIRROR — a plain list item is still just `- `, with no box invented.
    let plain = rows(&render_markdown("- plain\n", 80, &theme));
    assert!(plain.iter().any(|l| l == "- plain"), "plain item changed:\n{plain:?}");
    assert!(!plain.join("\n").contains('['), "plain item grew a task box:\n{plain:?}");

    // MIRROR — an ordered task list keeps its number as the bullet.
    let ol = rows(&render_markdown("1. [x] first\n", 80, &theme));
    assert!(ol.iter().any(|l| l == "1. [x] first"), "ordered task marker:\n{ol:?}");
}

#[test]
fn m6_horizontal_rule_is_followed_by_exactly_one_blank_row() {
    // `case "hr": lines.push(hr(…)); if (nextTokenType && nextTokenType !== "space") lines.push("")`
    // (`markdown.ts:605-610`); a following `space` token supplies the blank instead (`:619-622`).
    // Either way: exactly one.
    let theme = UiTheme::dark();
    let lines = render_markdown("before\n\n---\nafter the rule\n", 80, &theme);
    let r = rows(&lines);
    let rule_at =
        r.iter().position(|l| l.starts_with('─')).unwrap_or_else(|| panic!("no rule:\n{r:?}"));
    assert!(
        lines.get(rule_at + 1).map(is_blank).unwrap_or(false),
        "no blank row after the rule:\n{r:?}"
    );
    assert!(
        !lines.get(rule_at + 2).map(is_blank).unwrap_or(true),
        "TWO blank rows after the rule:\n{r:?}"
    );
    assert_eq!(r.get(rule_at + 2).map(String::as_str), Some("after the rule"), "{r:?}");

    // MIRROR — a blank source line around the rule still yields ONE blank, not two.
    let spaced = render_markdown("before\n\n---\n\nafter\n", 80, &theme);
    let sr = rows(&spaced);
    let at = sr.iter().position(|l| l.starts_with('─')).expect("no rule");
    assert!(spaced.get(at + 1).map(is_blank).unwrap_or(false), "{sr:?}");
    assert!(!spaced.get(at + 2).map(is_blank).unwrap_or(true), "doubled blank:\n{sr:?}");
}

#[test]
fn m8_soft_line_break_inside_a_paragraph_stays_a_row_break() {
    // marked keeps the `\n` inside the text token — which is why `renderInlineTokens` splits and
    // rejoins on `\n` (`markdown.ts:638-641`) — and `wrapTextWithAnsi` then splits the rendered line
    // on `/\r\n|\r|\n/`, one output row per source line (`utils.ts:839`).
    let theme = UiTheme::dark();
    let r = rows(&render_markdown("alpha\nbeta\ngamma\n", 200, &theme));
    assert_eq!(r, vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()], "{r:?}");
    assert!(!r.iter().any(|l| l.contains("alpha beta")), "soft break collapsed to a space:\n{r:?}");

    // MIRROR — a paragraph with no source break is still ONE logical row (the wrap layer, not the
    // renderer, breaks it), and a blank line still separates paragraphs.
    let one = rows(&render_markdown("alpha beta gamma\n", 200, &theme));
    assert_eq!(one, vec!["alpha beta gamma".to_string()], "{one:?}");
    let two = rows(&render_markdown("alpha\n\nbeta\n", 200, &theme));
    assert_eq!(two, vec!["alpha".to_string(), String::new(), "beta".to_string()], "{two:?}");
}

#[test]
fn m11_table_too_narrow_for_its_grid_falls_back_to_raw_markdown() {
    // `const availableForCells = availableWidth - borderOverhead; if (availableForCells < numCols) {
    // return token.raw ? wrapTextWithAnsi(token.raw, availableWidth) : []; }`
    // (`markdown.ts:850-861`) — degrade to the source text rather than draw a grid wider than the
    // pane. borderOverhead = 3n + 1, so a 2-column table needs 3*2+1 + 2 = 9 columns.
    let theme = UiTheme::dark();
    let md = "| Name | Role |\n|------|------|\n| Ada | math |";

    let narrow = render_markdown(md, 8, &theme);
    let nr = rows(&narrow);
    assert!(!nr.iter().any(|l| l.contains('┌')), "narrow pane still drew a grid:\n{nr:?}");
    // The fallback is `wrapTextWithAnsi(token.raw, availableWidth)` (`markdown.ts:856`) — WRAPPED to
    // the pane, so at width 8 the 15-cell source rows are broken up rather than emitted whole. The
    // raw markdown must therefore survive *in full and in order* across the wrapped rows (upstream
    // drops only whitespace at a break point: `currentLine.trimEnd()` at `utils.ts:905` and the
    // "don't start new line with whitespace" arm at `:911-913`), and no row may exceed the pane.
    let squashed: String = nr.iter().filter(|l| !l.trim().is_empty()).flat_map(|l| l.chars()).filter(|c| !c.is_whitespace()).collect();
    let source: String = md.chars().filter(|c| !c.is_whitespace()).collect();
    assert_eq!(squashed, source, "raw fallback lost or reordered source text:\n{nr:?}");
    assert!(nr.iter().any(|l| l.contains("------")), "raw delimiter row missing:\n{nr:?}");
    assert!(nr.iter().any(|l| l.contains("Ada")), "raw body row missing:\n{nr:?}");
    assert_all_rows_fit(&narrow, 8, "too-narrow fallback");

    // MIRROR — one column wider and the grid is back.
    let wide = rows(&render_markdown(md, 9, &theme));
    assert!(wide.iter().any(|l| l.contains('┌')), "width 9 must still draw the grid:\n{wide:?}");
    assert!(!wide.iter().any(|l| l.contains("|------|")), "grid must not print raw:\n{wide:?}");
}

/// Assert that EVERY row of `lines` fits a pane of `w` columns.
///
/// The `┌` frame row alone proves nothing: a body row is `borderOverhead + Σ columnWidths` and the
/// top border is the same arithmetic, so they overflow or fit together — while the too-narrow
/// FALLBACK emits no frame row at all and was, until this batch, pushed through unwrapped.
///
/// `w.max(1)` because upstream floors the column at one cell: `wrapCellText` is
/// `wrapTextWithAnsi(text, Math.max(1, maxWidth))` (`markdown.ts:829-831`).
fn assert_all_rows_fit(lines: &[Line<'_>], w: usize, what: &str) {
    for l in lines {
        assert!(
            l.width() <= w.max(1),
            "{what}: at pane width {w}, row {:?} is {} cells wide\nall rows: {:?}",
            line_text(l),
            l.width(),
            rows(lines)
        );
    }
}

#[test]
fn m11_narrow_tables_never_panic_on_column_arithmetic() {
    // Column arithmetic is the hazard of this batch: sweep every width from 0 up past the grid
    // threshold and assert on EVERY emitted row, not just the frame.
    let theme = UiTheme::dark();

    // ── ASCII: the grid and the raw fallback must BOTH fit the pane at every width. The fallback
    // is `wrapTextWithAnsi(token.raw, availableWidth)` (`markdown.ts:856`) — wrapped, not dumped.
    let ascii = "| Name | Role |\n|------|------|\n| Ada | math |\n| Alan | logic |";
    for w in 0..=60usize {
        let lines = render_markdown(ascii, w, &theme);
        assert!(!lines.is_empty(), "width {w} produced nothing");
        assert_all_rows_fit(&lines, w, "ascii table");
    }
    // The widths the brief calls out explicitly, incl. narrower than the column count (2 cols).
    for w in [1usize, 2, 5, 20] {
        let lines = render_markdown(ascii, w, &theme);
        assert_all_rows_fit(&lines, w, "ascii table");
    }

    // ── Five columns: a grid needs 3*5+1 + 5 = 21 cells, so 1/2/5/20 are all fallback widths and
    // every one of them is narrower than the column count.
    let five = "| a | b | c | d | e |\n|--|--|--|--|--|\n| 1 | 2 | 3 | 4 | 5 |";
    for w in 0..=40usize {
        let lines = render_markdown(five, w, &theme);
        assert!(!lines.is_empty(), "width {w} produced nothing");
        assert_all_rows_fit(&lines, w, "five-column table");
        if w < 3 * 5 + 1 + 5 {
            let r = rows(&lines);
            assert!(!r.iter().any(|l| l.contains('┌')), "width {w} drew an oversized grid:\n{r:?}");
        }
    }

    // ── CJK (2 cells), a ZWJ emoji family (one cluster, 2 cells) and combining marks.
    //
    // Rows fit at every width EXCEPT the band where the fitted column drops below the width of a
    // single grapheme cluster. Upstream overflows there too, BY CONSTRUCTION: `breakLongWord`
    // flushes the current line and then emits the oversize cluster whole rather than splitting
    // below the cluster (`tui/src/utils.ts:1000-1010`), and the cell pad is
    // `" ".repeat(Math.max(0, columnWidths[colIdx] - visibleWidth(text)))` (`markdown.ts:991`),
    // which clamps at zero and never truncates. Truncating here would be a divergence, so the band
    // is excluded from the fit assertion and pinned exactly instead, below.
    //
    // The band is exactly the widths at which some column is fitted to 1 cell: 0 and 1 (the raw
    // fallback, wrapped at `Math.max(1, …)`) and 13-15 (the narrowest panes that still draw a
    // 3-column grid). Everywhere else the fit is strict.
    let md = "| 日本語 | \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} | e\u{301}f |\n\
              |---|---|---|\n\
              | \u{65e5} | \u{1f600}\u{1f600} | a\u{308}b\u{308} |";
    const CLUSTER_OVERFLOW_BAND: [usize; 5] = [0, 1, 13, 14, 15];
    const NUM_COLS: usize = 3;
    for w in 0..=40usize {
        let lines = render_markdown(md, w, &theme);
        assert!(!lines.is_empty(), "width {w} produced nothing");
        // Universal bound, asserted at EVERY width including the band: a column can only overflow
        // by `widest cluster - column floor` = 2 - 1 = 1 cell, and there are three of them. This is
        // what makes the exclusion below an exception of bounded size rather than a blanket
        // amnesty — a genuine runaway in the column arithmetic still fails here.
        for l in &lines {
            assert!(
                l.width() <= w.max(1) + NUM_COLS,
                "at pane width {w}, row {:?} is {} cells — beyond even the per-column \
                 cluster-overflow bound\nall rows: {:?}",
                line_text(l),
                l.width(),
                rows(&lines)
            );
        }
        if !CLUSTER_OVERFLOW_BAND.contains(&w) {
            assert_all_rows_fit(&lines, w, "cjk/zwj/combining table");
        }
    }

    // Width 13 is the narrowest pane that still draws a 3-column grid (3*3+1 + 3), so every column
    // is fitted to exactly 1 cell. Hand-traced against upstream: `minColumnWidths` collapses to
    // `[1,1,1]` (`markdown.ts:887-890`), `extraWidth` is 0 (`:925`), and each header cell then goes
    // through `breakLongWord` at width 1 — "日本語" → `["", "日", "本", "語"]` because the FIRST
    // cluster already exceeds the column and flushes an empty `currentLine` (`utils.ts:1000-1007`).
    // That leading empty row, the one-cluster-per-row column and the resulting overflow are all
    // upstream's, reproduced glyph for glyph.
    let pinned = rows(&render_markdown(md, 13, &theme));
    assert_eq!(
        pinned,
        vec![
            "┌───┬───┬───┐".to_string(),
            "│   │   │ e\u{301} │".to_string(),
            "│ 日 │ \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} │ f │".to_string(),
            "│ 本 │   │   │".to_string(),
            "│ 語 │   │   │".to_string(),
            "├───┼───┼───┤".to_string(),
            "│   │   │ a\u{308} │".to_string(),
            "│ 日 │ \u{1f600} │ b\u{308} │".to_string(),
            "│   │ \u{1f600} │   │".to_string(),
            "└───┴───┴───┘".to_string(),
        ],
        "width-13 grid must reproduce upstream's cluster-per-row wrap exactly"
    );

    // MIRROR — a grapheme cluster is never SPLIT, at any width: the ZWJ family survives whole and
    // each combining mark stays welded to its base (`utils.ts:977-979` segments before measuring).
    for w in 0..=40usize {
        let joined = rows(&render_markdown(md, w, &theme)).join("\n");
        let zwj_pieces = joined.matches('\u{200d}').count();
        assert_eq!(zwj_pieces, 2, "width {w} split the ZWJ family:\n{joined}");
        for mark in ['\u{301}', '\u{308}'] {
            for (i, _) in joined.match_indices(mark) {
                let base = joined.get(..i).and_then(|s| s.chars().next_back());
                assert!(
                    matches!(base, Some('e' | 'a' | 'b')),
                    "width {w}: combining mark {mark:?} was detached from its base:\n{joined}"
                );
            }
        }
    }
}

#[test]
fn m14_inline_url_suffix_is_gated_on_terminal_hyperlink_support() {
    // `if (getCapabilities().hyperlinks) { result += hyperlink(styledLink, token.href) + …; }` — the
    // URL is NOT printed inline on a capable terminal, "regardless of whether it matches href"
    // (`markdown.ts:692-696`); the ` (url)` form is the fallback branch (`:697-707`).
    let theme = UiTheme::dark();
    let src = "See [the docs](https://example.com/a/very/long/path) now.\n";

    let capable = rows(&render_markdown_with_hyperlinks(src, 200, &theme, true)).join("\n");
    assert_eq!(capable, "See the docs now.", "OSC-8 terminal must not print the URL inline");

    // MIRROR — the incapable branch is unchanged, and the link text is underlined in BOTH.
    let plain_lines = render_markdown_with_hyperlinks(src, 200, &theme, false);
    let plain = rows(&plain_lines).join("\n");
    assert_eq!(plain, "See the docs (https://example.com/a/very/long/path) now.", "{plain}");
    assert!(has_span_mod(&plain_lines, "the docs", Modifier::UNDERLINED), "{plain}");
    let cap_lines = render_markdown_with_hyperlinks(src, 200, &theme, true);
    assert!(has_span_mod(&cap_lines, "the docs", Modifier::UNDERLINED), "{capable}");

    // MIRROR — a link whose text already IS the href never gets a suffix on either branch
    // (`markdown.ts:701-703`).
    for hl in [true, false] {
        let bare = rows(&render_markdown_with_hyperlinks(
            "<https://example.com/x>\n",
            200,
            &theme,
            hl,
        ))
        .join("\n");
        assert_eq!(bare, "https://example.com/x", "hyperlinks={hl} duplicated the bare URL");
    }
}

#[test]
fn m16_single_tilde_is_not_strikethrough() {
    // Pi installs a `StrictStrikethroughTokenizer` whose `del()` only matches
    // `/^(~~)(?=[^\s~])((?:\\.|[^\\])*?(?:\\.|[^\s~\\]))\1(?=[^~]|$)/` (`markdown.ts:7-24`,
    // `:171-174`), so a single-tilde run is never a `del` token.
    let theme = UiTheme::dark();
    let lines = render_markdown("open ~/notes~ and a~b~c here\n", 200, &theme);
    let text = rows(&lines).join("\n");
    assert_eq!(text, "open ~/notes~ and a~b~c here", "single tildes were eaten:\n{text}");
    assert!(
        !lines.iter().flat_map(|l| &l.spans).any(|s| s.style.add_modifier.contains(Modifier::CROSSED_OUT)),
        "single-tilde run was struck through:\n{text}"
    );

    // MIRROR — the double-tilde form still strikes, and still drops its delimiters.
    let strong = render_markdown("this ~~gone~~ stays\n", 200, &theme);
    let st = rows(&strong).join("\n");
    assert_eq!(st, "this gone stays", "double-tilde delimiters must be dropped:\n{st}");
    assert!(has_span_mod(&strong, "gone", Modifier::CROSSED_OUT), "`~~gone~~` not struck:\n{st}");

    // MIRROR — inline formatting inside a literal single-tilde run still renders.
    let mixed = render_markdown("~a **b** c~\n", 200, &theme);
    assert_eq!(rows(&mixed).join("\n"), "~a b c~", "{:?}", rows(&mixed));
    assert!(has_span_mod(&mixed, "b", Modifier::BOLD), "bold inside a literal `~…~` lost");
}

#[test]
fn m8_soft_break_inside_a_list_item_indents_past_the_marker() {
    // The regression M8 introduced: making `SoftBreak` flush the line is right for a paragraph, but
    // inside a list item every row after the first then started at column 0. Upstream builds TWO
    // prefixes per item and picks between them per row:
    //   `const firstPrefix = indent + this.theme.listBullet(marker);`
    //   `const continuationPrefix = indent + " ".repeat(visibleWidth(marker));`   (`markdown.ts:774-775`)
    //   `const linePrefix = renderedAnyLine ? continuationPrefix : firstPrefix;`  (`:789`)
    let theme = UiTheme::dark();

    // `- ` is 2 cells, so the continuation row is 2 spaces.
    let r = rows(&render_markdown("- alpha\n  beta\n- gamma\n", 200, &theme));
    assert_eq!(r, vec!["- alpha".to_string(), "  beta".to_string(), "- gamma".to_string()], "{r:?}");

    // An ordered marker is 3 cells (`10. ` would be 4) — the padding is the MARKER's width, not a
    // constant.
    let ol = rows(&render_markdown("1. alpha\n   beta\n", 200, &theme));
    assert_eq!(ol, vec!["1. alpha".to_string(), "   beta".to_string()], "{ol:?}");
    let wide = rows(&render_markdown("9. alpha\n   beta\n10. gamma\n    delta\n", 200, &theme));
    assert_eq!(
        wide,
        vec![
            "9. alpha".to_string(),
            "   beta".to_string(),
            "10. gamma".to_string(),
            "    delta".to_string()
        ],
        "the pad must track `visibleWidth(marker)`, and `10. ` is one wider than `9. `:\n{wide:?}"
    );

    // A task marker is part of `marker` (`marker = bullet + taskMarker`, `:772-773`), so the
    // continuation clears the box too: `- [ ] ` is 6 cells.
    let task = rows(&render_markdown("- [ ] alpha\n      beta\n", 200, &theme));
    assert_eq!(task, vec!["- [ ] alpha".to_string(), "      beta".to_string()], "{task:?}");

    // Nesting: the depth indent AND the marker pad compose — `"    ".repeat(1)` + 2.
    let nested = rows(&render_markdown("- top\n  - one\n    two\n", 200, &theme));
    assert_eq!(
        nested,
        vec!["- top".to_string(), "    - one".to_string(), "      two".to_string()],
        "{nested:?}"
    );

    // A hard break (two trailing spaces) takes the same path.
    let hard = rows(&render_markdown("- alpha  \n  beta\n", 200, &theme));
    assert_eq!(hard, vec!["- alpha".to_string(), "  beta".to_string()], "{hard:?}");

    // MIRROR — the paragraph case M8 was actually about is untouched: NO padding outside a list.
    let para = rows(&render_markdown("alpha\nbeta\n", 200, &theme));
    assert_eq!(para, vec!["alpha".to_string(), "beta".to_string()], "{para:?}");
    // MIRROR — a nested list's own rows carry only `indent`, never the PARENT item's marker pad:
    // `renderList(itemToken, depth + 1, …)` is pushed directly, bypassing `linePrefix` (`:781`).
    let deep = rows(&render_markdown("- top\n  - one\n    - two\n", 200, &theme));
    assert!(deep.iter().any(|l| l == "    - one"), "depth 1 must be indent-only:\n{deep:?}");
    assert!(deep.iter().any(|l| l == "        - two"), "depth 2 must be indent-only:\n{deep:?}");
    // MIRROR — a prose row AFTER the list has no residue of the item's marker.
    let after = rows(&render_markdown("- alpha\n  beta\n\ntail\n", 200, &theme));
    assert_eq!(after.last().map(String::as_str), Some("tail"), "{after:?}");
}

#[test]
fn m11_narrow_table_fallback_keeps_the_blockquote_and_list_prefixes() {
    // Upstream's fallback is a plain `string[]` return (`markdown.ts:854-861`); the CALLER prefixes
    // it — `this.theme.quoteBorder("│ ") + wrappedLine` for a blockquote (`:596`), `linePrefix +
    // wrappedLine` for a list item (`:790`). Pushing the raw rows straight onto the output dropped
    // both, so a too-narrow table silently escaped its container.
    let theme = UiTheme::dark();
    let table = "| Name | Role |\n|------|------|\n| Ada | math |";

    // Inside a blockquote: every fallback row keeps its `│ ` border, in mdQuoteBorder.
    let md = format!("> {}\n", table.replace('\n', "\n> "));
    let lines = render_markdown(&md, 8, &theme);
    let r = rows(&lines);
    assert!(!r.iter().any(|l| l.contains('┌')), "width 8 must not draw a grid:\n{r:?}");
    let body: Vec<&String> = r.iter().filter(|l| !l.trim().is_empty()).collect();
    assert!(!body.is_empty(), "fallback produced nothing inside a blockquote:\n{r:?}");
    for l in &body {
        assert!(l.starts_with("│ "), "fallback row escaped the blockquote border:\n{r:?}");
    }
    let border = theme.md_quote_border_style().fg;
    assert!(
        lines.iter().flat_map(|l| &l.spans).any(|s| s.content == "│ " && s.style.fg == border),
        "the `│ ` prefix is not in mdQuoteBorder:\n{r:?}"
    );

    // Inside a list item: every fallback row keeps the item's prefix — `- ` on the first, the
    // 2-cell continuation pad on the rest (`markdown.ts:774-775`, `:789-790`).
    let in_list = format!("- {}\n", table.replace('\n', "\n  "));
    let lr = rows(&render_markdown(&in_list, 8, &theme));
    let lbody: Vec<&String> = lr.iter().filter(|l| !l.trim().is_empty()).collect();
    assert!(!lbody.is_empty(), "fallback produced nothing inside a list item:\n{lr:?}");
    assert!(
        lbody.first().is_some_and(|l| l.starts_with("- ")),
        "the item's bullet was dropped:\n{lr:?}"
    );
    for l in lbody.iter().skip(1) {
        assert!(l.starts_with("  "), "fallback row fell back to column 0:\n{lr:?}");
    }

    // MIRROR — at top level the fallback is unprefixed, and the raw markdown still shows through.
    let top = rows(&render_markdown(table, 8, &theme));
    assert!(top.iter().any(|l| l.contains("|---")), "raw fallback missing:\n{top:?}");
    assert!(!top.iter().any(|l| l.starts_with("│ ")), "invented a quote border:\n{top:?}");
}

#[test]
fn fenced_code_block_emits_no_trailing_indent_only_row() {
    // marked's `fences` tokenizer is
    // `^ {0,3}(`{3,}(?=[^`\n]*\n)|~{3,})([^\n]*)(?:\n|$)(?:|([\s\S]*?)(?:\n|$))(?: {0,3}\1[~`]* *(?=\n|$)|$)`
    // — the `(?:\n|$)` after the body capture eats the newline BEFORE the closing fence, so
    // `token.text.split("\n")` (`markdown.ts:530`) is ONE line for a one-line body. pulldown-cmark
    // keeps that newline in its `Text` events, which grew a spurious `  ` row in every block.
    let theme = UiTheme::dark();

    // Flat path (no info string).
    let flat = rows(&render_markdown("```\nplain\n```\n", 80, &theme));
    assert_eq!(flat, vec!["```".to_string(), "  plain".to_string(), "```".to_string()], "{flat:?}");

    // Highlighted path (syntect) — same shape, and uniform with the flat path.
    let hl = rows(&render_markdown("```rust\nfn main() {}\n```\n", 80, &theme));
    assert_eq!(
        hl,
        vec!["```rust".to_string(), "  fn main() {}".to_string(), "```".to_string()],
        "{hl:?}"
    );

    // A genuinely blank FINAL code line is content and must survive — `strip_suffix`, not `trim_end`.
    let blank = rows(&render_markdown("```\na\n\n```\n", 80, &theme));
    assert_eq!(
        blank,
        vec!["```".to_string(), "  a".to_string(), "  ".to_string(), "```".to_string()],
        "a deliberate trailing blank code line was eaten:\n{blank:?}"
    );

    // MIRROR — a multi-line body keeps every one of its lines.
    let multi = rows(&render_markdown("```\na\nb\nc\n```\n", 80, &theme));
    assert_eq!(
        multi,
        vec![
            "```".to_string(),
            "  a".to_string(),
            "  b".to_string(),
            "  c".to_string(),
            "```".to_string()
        ],
        "{multi:?}"
    );
}

#[test]
fn m17_code_fence_keeps_the_whole_info_string() {
    // `lines.push(this.theme.codeBlockBorder(`\`\`\`${token.lang || ""}`))` (`markdown.ts:522`).
    // marked's `token.lang` is the trimmed *info string*, not just the language — which is why every
    // consumer that wants the bare language splits it itself, e.g.
    // `token.lang?.trim().split(/\s+/, 1)[0]?.toLowerCase() === "mermaid"` (`mermaid.ts:15`).
    let theme = UiTheme::dark();
    let lines = render_markdown("```js title=\"server.ts\"\nconst x = 1;\n```\n", 200, &theme);
    let r = rows(&lines);
    assert!(r.iter().any(|l| l == "```js title=\"server.ts\""), "info string truncated:\n{r:?}");

    // The same unsplit string reaches the highlighter, so — exactly like Pi's
    // `supportsLanguage('js title="server.ts"')` returning false (`theme.ts:1268-1274`) — the body
    // falls back to a flat block rather than being highlighted as JavaScript.
    assert!(
        !has_span_colored(&lines, "const", Color::Rgb(0x56, 0x9C, 0xD6)),
        "a multi-word info string must not highlight:\n{r:?}"
    );

    // MIRROR — a bare language is unchanged: printed in full AND highlighted.
    let bare = render_markdown("```rust\nfn main() {}\n```\n", 200, &theme);
    assert!(rows(&bare).iter().any(|l| l == "```rust"), "{:?}", rows(&bare));
    assert!(
        has_span_colored(&bare, "fn", Color::Rgb(0x56, 0x9C, 0xD6)),
        "bare `rust` lost its highlighting:\n{:?}",
        rows(&bare)
    );
    // MIRROR — an info-less fence still prints a bare ``` line.
    let none = render_markdown("```\nplain\n```\n", 200, &theme);
    assert!(rows(&none).iter().any(|l| l == "```"), "{:?}", rows(&none));
}

/// M14, both disjuncts: pi's fallback test is `token.text === token.href || token.text ===
/// hrefForComparison` (`v0.84.1 markdown.ts:701-702`). Testing only the stripped form misses a link
/// whose text is the FULL `mailto:` href, which upstream treats as self-describing.
#[test]
fn m14_a_link_whose_text_is_the_full_mailto_href_gets_no_suffix() {
    let theme = UiTheme::dark();
    let lines = render_markdown_with_hyperlinks("[mailto:a@b.com](mailto:a@b.com)", 80, &theme, false);
    let text: String = lines.iter().map(|l| l.to_string()).collect();
    assert!(
        !text.contains('('),
        "text == href exactly, so pi's first disjunct suppresses the suffix: {text}"
    );

    // MIRROR 1 — the stripped form still matches (pi's SECOND disjunct), which is what an
    // autolinked email produces: text `a@b.com`, href `mailto:a@b.com`.
    let stripped = render_markdown_with_hyperlinks("[a@b.com](mailto:a@b.com)", 80, &theme, false);
    let stripped_text: String = stripped.iter().map(|l| l.to_string()).collect();
    assert!(
        !stripped_text.contains('('),
        "the mailto-stripped disjunct must still suppress the suffix: {stripped_text}"
    );

    // MIRROR 2 — a genuinely different text still gets the suffix, so this is not a blanket
    // suppression.
    let differing = render_markdown_with_hyperlinks("[email me](mailto:a@b.com)", 80, &theme, false);
    let differing_text: String = differing.iter().map(|l| l.to_string()).collect();
    assert!(
        differing_text.contains("(mailto:a@b.com)"),
        "differing text must still print the url: {differing_text}"
    );
}
