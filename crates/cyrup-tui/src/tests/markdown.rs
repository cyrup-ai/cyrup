//! Markdown + syntax rendering tests (spec/tui/06 §2-§3).
//!
//! Exercises the `pulldown-cmark` walk (`render_markdown`) and the `syntect` fenced-code highlight,
//! plus the streaming partial-fence trim, asserting on the produced styled `Line`s and on a rendered
//! `TestBackend` buffer.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::{
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
    //
    // `image::CAPABILITIES` is process-wide and `tests::image_capabilities` PINS it (to
    // `hyperlinks: true`, then to `false`, then resets) to exercise both branches. Both reads below
    // — the direct one and the one `render_markdown` makes for itself — must therefore see the same
    // value, or this assertion fails on a renderer that is behaving perfectly. That is a real race,
    // not a hypothetical: libtest runs these two tests on parallel threads in one process. The
    // writer already takes this lock; enrolling the reader is what actually closes it.
    let _caps = super::harness::caps_lock();
    let detected = crate::hyperlinks_supported();
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
    // The band is now just the degenerate panes 0 and 1, where `contentWidth` itself floors at
    // `Math.max(1, …)` and a 2-cell cluster cannot fit in the one column there is. It used to also
    // cover 13-15 — the narrowest panes that still draw a 3-column grid — and those are now strict,
    // because the top-level re-wrap post-pass (`markdown.ts:316-326`) catches the overflowing grid
    // rows before they leave `render`. See the pinned width-13 trace below.
    let md = "| 日本語 | \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} | e\u{301}f |\n\
              |---|---|---|\n\
              | \u{65e5} | \u{1f600}\u{1f600} | a\u{308}b\u{308} |";
    const CLUSTER_OVERFLOW_BAND: [usize; 2] = [0, 1];
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
    // That leading empty row and the one-cluster-per-row column are upstream's, glyph for glyph.
    //
    // The rows `renderTable` builds from them, however, are NOT what `render` returns. A grid row
    // whose 1-cell columns each hold a 2-cell cluster measures `13 + 2` (the pad is
    // `Math.max(0, …)`, `markdown.ts:991`, so it clamps at zero and never claws the overflow back),
    // and `render` runs **every** produced line through `wrapTextWithAnsi(line, contentWidth)` once
    // more at `:322` before the margins go on at `:328-340`. That post-pass splits the over-wide
    // rows at their last fitting token boundary and `trimEnd`s each piece (`utils.ts:934`), which is
    // where the bare trailing `│` rows come from:
    //
    // ```text
    // "│ 日 │ 👨‍👩‍👧 │ f │"   15 cells → tokens fill to 13 at "f", the final "│" starts a new row
    // "│ 本 │   │   │"       14 cells → trailing "   " trimmed off the first piece → 10 cells
    // ```
    //
    // An earlier revision of this expectation was traced through `renderTable` alone and stopped
    // there, so it pinned the pre-post-pass rows and read as though upstream shipped a table wider
    // than its own pane. It does not; `:316-326` is the second of upstream's two wraps.
    let pinned = rows(&render_markdown(md, 13, &theme));
    assert_eq!(
        pinned,
        vec![
            "┌───┬───┬───┐".to_string(),
            "│   │   │ e\u{301} │".to_string(),
            "│ 日 │ \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} │ f".to_string(),
            "│".to_string(),
            "│ 本 │   │".to_string(),
            "│".to_string(),
            "│ 語 │   │".to_string(),
            "│".to_string(),
            "├───┼───┼───┤".to_string(),
            "│   │   │ a\u{308} │".to_string(),
            "│ 日 │ \u{1f600} │ b\u{308}".to_string(),
            "│".to_string(),
            "│   │ \u{1f600} │".to_string(),
            "│".to_string(),
            "└───┴───┴───┘".to_string(),
        ],
        "width-13 grid must reproduce upstream's cluster-per-row wrap AND its `:322` re-wrap exactly"
    );
    // …and every one of those rows now fits the pane, which is the point of the post-pass.
    for l in &render_markdown(md, 13, &theme) {
        assert!(l.width() <= 13, "row {:?} is {} cells at a 13-cell pane", line_text(l), l.width());
    }

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

// --- SYS-2: the wrap moved inside `markdown::render` -------------------------------------------
//
// Upstream wraps FIRST and prefixes SECOND, at all three of its wrapping sites:
//   `markdown.ts:322`     `wrapTextWithAnsi(line, contentWidth)`   then `:340` `leftMargin + line`
//   `markdown.ts:594-597` `wrapTextWithAnsi(styledLine, quoteContentWidth)` then `quoteBorder("│ ") + wrappedLine`
//   `markdown.ts:788-791` `wrapTextWithAnsi(line, itemWidth)`      then `linePrefix + wrappedLine`
// cyrup's `render` never wrapped at all; the outer `Paragraph::wrap` reflowed the already-indented
// logical line at full frame width, which is L2/M5/M10.

/// A sentence long enough to wrap several times at every width these tests use.
const LONG: &str =
    "The quick brown fox jumps over the lazy dog and then keeps running for quite a long while.";

/// **L2/M10** — a paragraph wraps at the width `render` was handed and nothing exceeds it.
#[test]
fn sys2_a_paragraph_wraps_at_the_content_width() {
    let theme = UiTheme::dark();
    for width in [5usize, 20, 40, 80] {
        let lines = render_markdown(LONG, width, &theme);
        assert!(lines.len() > 1, "width={width}: nothing wrapped: {:?}", rows(&lines));
        for l in &lines {
            assert!(l.width() <= width, "width={width}: row overflows: {:?}", line_text(l));
        }
    }
    // MIRROR: a paragraph that already fits is ONE row, returned verbatim — `wrapSingleLine`'s
    // `if (visibleLength <= width) return [line]` (`utils.ts:862-865`).
    let short = render_markdown("hello there", 80, &theme);
    assert_eq!(rows(&short), vec!["hello there".to_string()]);
}

/// **M5** — the list-item hanging indent, the half batch 6 left: `itemWidth = Math.max(1, width -
/// visibleWidth(firstPrefix))` (`markdown.ts:776`) and the wrap loop at `:788-791` that picks
/// `renderedAnyLine ? continuationPrefix : firstPrefix` per produced row.
///
/// Before this, a long bullet emitted ONE logical row and the outer `Paragraph::wrap` broke it with
/// no prefix at all, so row 2 started under the `- ` instead of under the item's text.
#[test]
fn m5_a_wrapped_list_item_hangs_under_its_own_text() {
    let theme = UiTheme::dark();
    let lines = render_markdown(&format!("- {LONG}"), 20, &theme);
    let text = rows(&lines);
    assert!(text.len() > 1, "expected a wrapped item: {text:?}");
    assert!(text[0].starts_with("- "), "row 0 is firstPrefix: {:?}", text[0]);
    for row in &text[1..] {
        // `continuationPrefix = indent + " ".repeat(visibleWidth(marker))` — `visibleWidth("- ")`
        // is 2, and the pad is SPACES, not a re-drawn bullet (`markdown.ts:775`).
        assert!(row.starts_with("  "), "continuation lost the hanging indent: {row:?}");
        assert!(!row.starts_with("- "), "bullet re-drawn on a continuation row: {row:?}");
    }
    for l in &lines {
        assert!(l.width() <= 20, "row overflows itemWidth + prefix: {:?}", line_text(l));
    }

    // An ORDERED marker is wider, so `itemWidth` is narrower and the pad is 3 (`10. ` → 4).
    let ordered = rows(&render_markdown(&format!("1. {LONG}"), 20, &theme));
    assert!(ordered[0].starts_with("1. "), "{ordered:?}");
    assert!(ordered[1].starts_with("   ") && !ordered[1].starts_with("    "), "{ordered:?}");

    // A TASK marker is `bullet + taskMarker` — `- [ ] `, visible width 6 (`markdown.ts:772-773`).
    let task = rows(&render_markdown(&format!("- [ ] {LONG}"), 24, &theme));
    assert!(task[0].starts_with("- [ ] "), "{task:?}");
    assert!(task[1].starts_with("      ") && !task[1].starts_with("       "), "{task:?}");

    // MIRROR: a SHORT item is untouched — one row, still the bullet, no phantom continuation.
    assert_eq!(rows(&render_markdown("- ok", 20, &theme)), vec!["- ok".to_string()]);
}

/// **M5, compound** — nesting adds `"    ".repeat(depth)` (`markdown.ts:758`) ahead of the marker
/// pad, and a blockquote adds a **visible** `│ ` that is re-emitted on every wrapped row
/// (`markdown.ts:596`) where the list pad is spaces (`:775`). Getting those two the same way round
/// is the whole point.
#[test]
fn m5_nested_and_quoted_list_continuations_use_the_right_prefix() {
    let theme = UiTheme::dark();

    // depth 1: indent "    " + marker pad "  " = 6 columns of continuation.
    let nested = rows(&render_markdown(&format!("- outer\n    - {LONG}"), 24, &theme));
    assert_eq!(nested[0], "- outer");
    assert!(nested[1].starts_with("    - "), "nested firstPrefix: {nested:?}");
    for row in &nested[2..] {
        assert!(row.starts_with("      "), "nested continuation: {row:?}");
        assert!(!row.contains('-'), "bullet re-drawn: {row:?}");
    }

    // Inside a blockquote the continuation is the BORDER, re-drawn, plus the marker pad.
    let quoted = render_markdown(&format!("> - {LONG}"), 24, &theme);
    let qtext = rows(&quoted);
    assert!(qtext[0].starts_with("│ - "), "quoted firstPrefix: {qtext:?}");
    for row in &qtext[1..] {
        assert!(row.starts_with("│   "), "quoted continuation: {row:?}");
    }
    for l in &quoted {
        assert!(l.width() <= 24, "quoted row overflows: {:?}", line_text(l));
    }
}

/// **The blockquote wrap cyrup never had** — `quoteContentWidth = Math.max(1, width - 2)`
/// (`markdown.ts:568`), children rendered into it (`:583`), then every wrapped row gets its own
/// `quoteBorder("│ ")` (`:594-597`).
#[test]
fn sys2_a_long_blockquote_redraws_its_border_on_every_row() {
    let theme = UiTheme::dark();
    let lines = render_markdown(&format!("> {LONG}"), 24, &theme);
    let text = rows(&lines);
    assert!(text.len() > 1, "expected a wrapped quote: {text:?}");
    for row in &text {
        assert!(row.starts_with("│ "), "quote row lost its border: {row:?}");
    }
    for l in &lines {
        assert!(l.width() <= 24, "quote row overflows: {:?}", line_text(l));
    }
    // The border is `mdQuoteBorder`, a real glyph with its own colour — not the list pad's spaces.
    assert!(lines[1].spans[0].style.fg.is_some(), "continuation border unstyled: {:?}", lines[1]);
}

/// **M9 mirror — must NOT change.** `render`'s `width` argument IS `contentWidth` in cyrup (every
/// call site already passes `width - outputPad * 2`), so `hr("─".repeat(Math.min(width, 80)))`
/// (`markdown.ts:606`) draws 78 at a pane of 80 with `outputPad = 1`. Subtracting the padding a
/// second time inside `render` would silently narrow every message by two columns.
#[test]
fn m9_the_rule_still_spans_the_full_content_width() {
    let theme = UiTheme::dark();
    let lines = render_markdown("---", 78, &theme);
    assert_eq!(rows(&lines), vec!["─".repeat(78)]);
    // `Math.min(width, 80)` still clamps.
    assert_eq!(rows(&render_markdown("---", 200, &theme)), vec!["─".repeat(80)]);
    // Inside a container the rule is sized to `itemWidth`/`quoteContentWidth` and PREFIXED —
    // `renderToken(itemToken, itemWidth, …)` (`:786`) then `linePrefix + wrappedLine` (`:790`).
    let in_item = rows(&render_markdown("- x\n\n  ---\n", 20, &theme));
    let rule: Vec<&String> = in_item.iter().filter(|r| r.contains('─')).collect();
    // ONE row: `min(itemWidth, 80)` is 18, which fits behind the 2-column continuation prefix. Sized
    // to the component width instead it would be 20, would itself have to wrap, and would spill a
    // second 2-dash row — the shape cyrup produced before `content_width()`.
    assert_eq!(rule.len(), 1, "rule not sized to itemWidth: {in_item:?}");
    assert_eq!(rule[0], &format!("  {}", "─".repeat(18)), "rule not sized/prefixed to the item");
}

/// **Edit 4, table half** — `renderTable(token, width, …)` (`markdown.ts:551`) receives the width
/// `renderToken` was called with — `quoteContentWidth` inside a blockquote (`:583`) — so both the
/// too-narrow guard (`:853-861`) and the fitted column widths follow the CONTAINER, not the
/// component.
#[test]
fn m9_a_table_inside_a_blockquote_is_sized_to_the_quote_not_the_pane() {
    let theme = UiTheme::dark();
    let table = "| a | b |\n|---|---|\n| 1 | 2 |";
    let quoted = format!("> {}\n", table.replace('\n', "\n> "));

    // Border overhead for 2 columns is `3n + 1` = 7, so the grid needs >= 9 columns
    // (`availableForCells < numCols` → fallback). At a pane of 10 the top level clears it…
    let top = rows(&render_markdown(table, 10, &theme));
    assert!(top.iter().any(|r| r.contains('┌')), "top level should draw a grid: {top:?}");

    // …but `quoteContentWidth = width - 2` is 8, which does NOT, so upstream degrades to the raw
    // Markdown and `:596` prefixes it. Sized to the pane instead, cyrup drew a 10-column grid inside
    // an 8-column container.
    let nested = rows(&render_markdown(&quoted, 10, &theme));
    assert!(!nested.iter().any(|r| r.contains('┌')), "grid drawn too wide: {nested:?}");
    assert!(nested.iter().any(|r| r.contains("|---")), "no raw fallback: {nested:?}");

    // And when the grid DOES fit, its columns are fitted to `quoteContentWidth`, not to the pane.
    let wide = render_markdown(&quoted, 30, &theme);
    let grid: Vec<&Line<'_>> = wide.iter().filter(|l| line_text(l).contains('┌')).collect();
    assert!(!grid.is_empty(), "no grid: {:?}", rows(&wide));
    for l in grid {
        assert!(
            l.width() <= 28,
            "grid wider than quoteContentWidth: {:?} ({})",
            line_text(l),
            l.width()
        );
    }
}

/// **Edit 5** — a fenced block inside a list item collects the item prefix, and an over-wide code
/// row breaks instead of running off the pane. Upstream a `code` token returns a bare `string[]`
/// (`markdown.ts:520-540`) that `:790` prefixes and `:322` wraps.
///
/// The `  ` code indent (`:521`) is deliberately part of the BODY, so a wrapped code row loses it —
/// `wrapSingleLine` never starts a produced row with whitespace (`utils.ts:912-915`).
#[test]
fn sys2_a_fenced_block_inside_a_list_item_keeps_the_prefix_and_wraps() {
    let theme = UiTheme::dark();
    let src = "- item\n\n  ```\n  let a = 1; let b = 2; let c = 3; let d = 4;\n  ```\n";
    let lines = render_markdown(src, 30, &theme);
    let text = rows(&lines);
    assert_eq!(text[0], "- item");
    let fence = text.iter().find(|r| r.contains("```")).expect("no fence row");
    assert_eq!(fence, "  ```", "fence lost the item prefix: {text:?}");
    let code: Vec<&String> = text.iter().filter(|r| r.contains("let ")).collect();
    assert!(code.len() > 1, "the long code line did not wrap: {text:?}");
    assert_eq!(code[0], "    let a = 1; let b = 2; let", "first code row: {text:?}");
    assert_eq!(code[1], "  c = 3; let d = 4;", "continuation drops the code indent: {text:?}");
    for l in &lines {
        assert!(l.width() <= 30, "code row overflows: {:?}", line_text(l));
    }

    // MIRROR: at top level the block is unprefixed and the `  ` indent is untouched when it fits.
    let top = rows(&render_markdown("```\nlet a = 1;\n```\n", 80, &theme));
    assert_eq!(top, vec!["```".to_string(), "  let a = 1;".to_string(), "```".to_string()]);
}

/// The full width ladder against pathological content — every row must fit, at every width, and no
/// grapheme cluster may be torn apart.
#[test]
fn sys2_width_ladder_holds_for_cjk_zwj_and_combining_marks() {
    let theme = UiTheme::dark();
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
    let cases = [
        "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8}".repeat(6),
        format!("{family} ").repeat(8),
        "e\u{0301}a\u{0308}o\u{0302}".repeat(20),
        "x".repeat(500),
    ];
    for case in &cases {
        for width in [1usize, 2, 5, 20, 40, 200] {
            for src in [case.clone(), format!("- {case}"), format!("> {case}")] {
                let lines = render_markdown(&src, width, &theme);
                let joined: String = rows(&lines).join("");
                // A ZWJ family is never split: no row may end or start mid-sequence.
                assert!(
                    !joined.contains("\u{200d}\u{1f469}") || joined.contains(family),
                    "ZWJ family torn at width={width}: {:?}",
                    rows(&lines)
                );
                // A combining mark never leads a row (it would attach to nothing).
                for l in &lines {
                    let t = line_text(l);
                    assert!(
                        !t.starts_with('\u{0301}') && !t.starts_with('\u{0308}'),
                        "combining mark orphaned at width={width}: {t:?}"
                    );
                }
                // Rows never exceed the pane once a single cluster fits in it. A cluster WIDER than
                // the column overflows by design (`utils.ts:1000` is unguarded upstream), which is
                // only reachable at width 1.
                if width > 2 {
                    for l in &lines {
                        assert!(
                            l.width() <= width,
                            "width={width} overflow: {:?}",
                            line_text(l)
                        );
                    }
                }
            }
        }
    }
}

/// **Items 1 + 2 — `emit_table` bypassed the prefix machinery.**
///
/// Upstream `renderTable` returns a bare `string[]` (`markdown.ts:1005`) and its CALLER decorates
/// every entry: `this.theme.quoteBorder("│ ") + wrappedLine` inside a blockquote
/// (`markdown.ts:592-598`) and `linePrefix + wrappedLine` inside a list item (`:786-793`), where
/// `linePrefix` is `renderedAnyLine ? continuationPrefix : firstPrefix` (`:789`).
///
/// cyrup pushed the `┌┬┐`/`└┴┘` frame and every `│ … │` grid row straight onto `self.out`, which
/// cost two separate things: the blockquote border vanished for the whole grid, and — because only
/// `open_line()` consumes a queued `pending_marker` — a table that was the first block of a list
/// item swallowed the bullet outright, the next `Start(Item)` overwriting it before it was ever
/// emitted. `Event::Rule` and `emit_code_block` had the identical bug and were already routed
/// through `emit_prefixed`; this is the third caller.
#[test]
fn emit_table_routes_the_grid_through_the_container_prefix() {
    let theme = UiTheme::dark();
    let table = "| a | b |\n|---|---|\n| 1 | 2 |";

    // ── Item 2: inside a blockquote, EVERY row keeps `│ `, frame rows included.
    let quoted = format!("> {}\n", table.replace('\n', "\n> "));
    let lines = render_markdown(&quoted, 30, &theme);
    let r = rows(&lines);
    assert!(r.iter().any(|l| l.contains('┌')), "no grid drawn at width 30:\n{r:?}");
    for l in r.iter().filter(|l| !l.trim().is_empty()) {
        assert!(l.starts_with("│ "), "grid row escaped the blockquote border:\n{r:?}");
    }
    // …and it is the real `mdQuoteBorder` prefix, not the table's own (unstyled) `│` bar.
    let border_fg = theme.md_quote_border_style().fg;
    for l in lines.iter().filter(|l| !line_text(l).trim().is_empty()) {
        assert_eq!(l.spans[0].content.as_ref(), "│ ", "row {:?}", line_text(l));
        assert_eq!(l.spans[0].style.fg, border_fg, "prefix is not mdQuoteBorder: {:?}", line_text(l));
    }
    // The grid is still sized to `quoteContentWidth` and nothing overflows the pane.
    assert_all_rows_fit(&lines, 30, "quoted table");

    // ── Item 1: inside a list item, the bullet lands on the FIRST produced row (the top border)
    // and every later row gets the 2-cell continuation pad (`markdown.ts:774-775`, `:789`).
    let in_list = format!("- {}\n", table.replace('\n', "\n  "));
    let lr = rows(&render_markdown(&in_list, 30, &theme));
    let body: Vec<&String> = lr.iter().filter(|l| !l.trim().is_empty()).collect();
    assert!(body.len() > 4, "expected a full grid inside the item:\n{lr:?}");
    assert!(
        body[0].starts_with("- ┌"),
        "the item's bullet was swallowed by the table:\n{lr:?}"
    );
    for l in body.iter().skip(1) {
        assert!(l.starts_with("  "), "grid row fell back to column 0:\n{lr:?}");
        assert!(!l.starts_with("- "), "bullet re-emitted on a continuation row:\n{lr:?}");
    }

    // MIRROR — the bullet is emitted exactly ONCE and is not lost for the item that follows.
    let two = rows(&render_markdown(&format!("{in_list}- after\n"), 30, &theme));
    assert_eq!(
        two.iter().filter(|l| l.starts_with("- ")).count(),
        2,
        "one bullet per item, no more and no fewer:\n{two:?}"
    );
    assert!(two.iter().any(|l| l == "- after"), "the NEXT item lost its bullet:\n{two:?}");

    // MIRROR — at top level the grid is unprefixed, exactly as before.
    let top = rows(&render_markdown(table, 30, &theme));
    assert!(top.iter().any(|l| l.starts_with('┌')), "top-level grid gained a prefix:\n{top:?}");
}

/// **Item 3 — `blank()` punched a hole in the blockquote border.**
///
/// A separator inside a blockquote is one of `renderedQuoteLines` like any other, and
/// `markdown.ts:592-598` prepends `quoteBorder("│ ")` to every entry it walks, so `> a\n>\n> b`
/// draws an unbroken border down all three rows. `blank()` pushed a bare `Line::default()`
/// regardless of container.
///
/// The counterpart is `:587-590`, `while (renderedQuoteLines.at(-1) === "") pop()` — the trailing
/// separator never reaches `:592`, so the block does not END on a dangling `│ `; the one blank
/// after a blockquote is `:599-601`'s BARE `""`.
#[test]
fn a_blank_inside_a_blockquote_keeps_the_border() {
    let theme = UiTheme::dark();

    let r = rows(&render_markdown("> a\n>\n> b\n", 20, &theme));
    assert_eq!(r, vec!["│ a".to_string(), "│ ".to_string(), "│ b".to_string()], "{r:?}");
    // The separator's border is the real `mdQuoteBorder` span, not incidental text.
    let lines = render_markdown("> a\n>\n> b\n", 20, &theme);
    assert_eq!(lines[1].spans.len(), 1, "{r:?}");
    assert_eq!(lines[1].spans[0].content.as_ref(), "│ ");
    assert_eq!(lines[1].spans[0].style.fg, theme.md_quote_border_style().fg);

    // A block-level child that emits its own trailing blank takes the same path.
    let code = rows(&render_markdown("> a\n>\n> ```\n> x\n> ```\n", 20, &theme));
    assert!(code.iter().any(|l| l == "│ "), "code-block separator lost the border:\n{code:?}");
    assert!(!code.iter().any(|l| l.is_empty()), "bare blank inside the quote:\n{code:?}");

    // A loose list inside the quote: `renderList`'s `:800` gap is bare, but `:596` still borders it.
    let list = rows(&render_markdown("> - a\n>\n> - b\n", 20, &theme));
    assert_eq!(list, vec!["│ - a".to_string(), "│ ".to_string(), "│ - b".to_string()], "{list:?}");

    // MIRROR — `:587-590`: the quote does not end on `│ `, and the blank AFTER it is bare.
    let tail = rows(&render_markdown("> a\n\ntail\n", 20, &theme));
    assert_eq!(tail, vec!["│ a".to_string(), String::new(), "tail".to_string()], "{tail:?}");

    // MIRROR — outside a blockquote a separator stays bare (`:800` / `:619-621`), no stray glyph.
    let plain = rows(&render_markdown("a\n\nb\n", 20, &theme));
    assert_eq!(plain, vec!["a".to_string(), String::new(), "b".to_string()], "{plain:?}");
}

/// **Item 6 — a marker-only list item rendered NOTHING.**
///
/// `if (!renderedAnyLine) { lines.push(firstPrefix); }` (`markdown.ts:796-798`): an item whose
/// children produced no row still emits its `firstPrefix` — `indent + listBullet(marker)` (`:774`)
/// — alone, on a row of its own. The trailing space of `"- "` survives because the top-level
/// re-wrap early-returns a fitting line verbatim (`utils.ts:862-865`), *before* the `trimEnd` at
/// `:934`.
///
/// `flush_line`'s empty-`cur` guard fires before `pending_marker` is ever materialised, so the row
/// was dropped AND the marker leaked into the next `Start(Item)`, which overwrote it.
#[test]
fn a_marker_only_list_item_still_emits_its_bullet() {
    let theme = UiTheme::dark();

    let r = rows(&render_markdown("- \n- x\n", 20, &theme));
    assert_eq!(r, vec!["- ".to_string(), "- x".to_string()], "{r:?}");
    // The bullet carries `mdListBullet` (`markdown.ts:774`), not prose styling.
    let lines = render_markdown("- \n- x\n", 20, &theme);
    assert_eq!(lines[0].spans.len(), 1, "{r:?}");
    assert_eq!(lines[0].spans[0].style.fg, theme.md_list_bullet_style().fg);

    // Ordered lists renumber across the empty item, exactly as `startNumber + i` does (`:762`).
    let ord = rows(&render_markdown("1. \n2. x\n", 20, &theme));
    assert_eq!(ord, vec!["1. ".to_string(), "2. x".to_string()], "{ord:?}");

    // `marker = bullet + taskMarker` (`:770-773`) — the whole thing is the `firstPrefix`.
    let task = rows(&render_markdown("- [ ]\n- x\n", 20, &theme));
    assert_eq!(task, vec!["- [ ] ".to_string(), "- x".to_string()], "{task:?}");

    // MIRROR — an item whose ONLY child is a nested list legitimately drops its own bullet:
    // `:779-783` pushes the sublist's rows directly and sets `renderedAnyLine = true`, so `:796`
    // never fires. Emitting a bullet here would be a divergence in the other direction.
    let nested = rows(&render_markdown("- \n  - x\n", 20, &theme));
    assert_eq!(nested, vec!["    - x".to_string()], "{nested:?}");

    // MIRROR — an item WITH content is untouched (one row, one bullet).
    let normal = rows(&render_markdown("- x\n- y\n", 20, &theme));
    assert_eq!(normal, vec!["- x".to_string(), "- y".to_string()], "{normal:?}");
}

/// **Item 8 — the narrow-table fallback leaked `> ` source markers.**
///
/// marked's `blockquote` tokenizer strips the `>` markers before re-lexing the quote body, so a
/// nested table reaches `renderTable` with a `token.raw` that carries none and `markdown.ts:856`'s
/// `wrapTextWithAnsi(token.raw, availableWidth)` prints clean Markdown. pulldown-cmark's offset
/// range is a slice of the untouched source, so cyrup's raw still had every `> ` on it — and this
/// batch's narrowing of the guard from `self.width` to `content_width()` newly routed blockquoted
/// tables down that path at moderate widths, making the leak reachable in ordinary panes.
#[test]
fn the_narrow_table_fallback_strips_blockquote_source_markers() {
    let theme = UiTheme::dark();
    let table = "| Name | Role |\n|------|------|\n| Ada | math |";
    let quoted = format!("> {}\n", table.replace('\n', "\n> "));

    // A 2-column grid needs `3n + 1 + n` = 9 cells of `quoteContentWidth = width - 2`
    // (`markdown.ts:850-853`), so panes 8..=10 take the fallback and 11 upward draw the grid.
    for width in [8usize, 9, 10] {
        let r = rows(&render_markdown(&quoted, width, &theme));
        assert!(!r.iter().any(|l| l.contains('┌')), "width {width} drew a grid:\n{r:?}");
        for l in &r {
            // The ONLY `│` allowed is the quote border cyrup draws; no `>` from the source.
            let body = l.strip_prefix("│ ").unwrap_or(l);
            assert!(
                !body.contains('>'),
                "width {width}: fallback leaked a `>` source marker:\n{r:?}"
            );
        }
        assert!(r.iter().any(|l| l.contains("Name")), "width {width}: no fallback body:\n{r:?}");
    }

    // Doubly nested: both levels of marker come off, and both borders stay on.
    let deep = rows(&render_markdown("> > | a | b |\n> > |---|---|\n> > | 1 | 2 |\n", 10, &theme));
    for l in deep.iter().filter(|l| !l.trim().is_empty()) {
        assert!(l.starts_with("│ │ "), "lost a border level:\n{deep:?}");
        assert!(!l.contains('>'), "leaked a `>` source marker:\n{deep:?}");
    }

    // MIRROR — at top level there is no marker to strip and a leading `>` in CELL TEXT survives.
    let top = rows(&render_markdown(table, 8, &theme));
    assert!(top.iter().any(|l| l.contains("|---")), "raw fallback missing:\n{top:?}");
    let gt = rows(&render_markdown("| a | b |\n|---|---|\n| > x | 2 |", 8, &theme));
    assert!(gt.iter().any(|l| l.contains('>')), "stripped a `>` that was content:\n{gt:?}");
}

/// **Item 7 — upstream's top-level re-wrap post-pass (`markdown.ts:316-326`).**
///
/// Upstream wraps TWICE: `renderList` at `itemWidth` (`:788`) and the blockquote arm at
/// `quoteContentWidth` (`:594`) wrap a child and prefix it, then `render()` runs **every** produced
/// line through `wrapTextWithAnsi(line, contentWidth)` once more before the margins go on at
/// `:328-340`. cyrup only had the inner wrap, so a row whose accumulated prefix was as wide as the
/// pane — `avail` floors at `Math.max(1, width - prefix_w)` (`:776`) — came out longer than `width`.
#[test]
fn the_top_level_rewrap_bounds_every_row_when_the_prefix_eats_the_pane() {
    let theme = UiTheme::dark();
    // Three quote levels are 6 cells of border before a single character of body.
    for width in 1..=8usize {
        let lines = render_markdown("> > > alpha beta\n", width, &theme);
        assert!(!lines.is_empty(), "width {width} produced nothing");
        assert_all_rows_fit(&lines, width, "triply quoted paragraph");
    }
    // Deep list nesting does the same through `indent` + marker rather than borders.
    let deep = "- a\n  - b\n    - c\n      - d\n        - e\n          - f\n";
    for width in 1..=12usize {
        assert_all_rows_fit(&render_markdown(deep, width, &theme), width, "deep nested list");
    }
    // …and a quoted deep list, where both prefixes stack.
    for width in 1..=14usize {
        let src: String = deep.lines().map(|l| format!("> {l}\n")).collect();
        assert_all_rows_fit(&render_markdown(&src, width, &theme), width, "quoted deep list");
    }

    // MIRROR — the post-pass is a NO-OP for rows that already fit: ordinary prose, a fitting grid
    // and a marker-only bullet all come back byte-identical, trailing space included.
    let prose = rows(&render_markdown("alpha beta gamma\n\n- x\n", 40, &theme));
    assert_eq!(prose, vec!["alpha beta gamma".to_string(), String::new(), "- x".to_string()]);
    let marker_only = rows(&render_markdown("- \n", 40, &theme));
    assert_eq!(marker_only, vec!["- ".to_string()], "post-pass trimmed a fitting row");
    let grid = rows(&render_markdown("| a | b |\n|---|---|\n| 1 | 2 |", 40, &theme));
    assert_eq!(grid[0], "┌───┬───┐", "{grid:?}");
}

// ── batch 10: markdown internals — M15 (table minimum column width), M7 (inline formatting in
// table cells) ─────────────────────────────────────────────────────────────────────────────────

/// **M15 — a column is floored at its longest unbroken word, capped at 30.**
///
/// `const maxUnbrokenWordWidth = 30` (`markdown.ts:863`) feeds `minWordWidths[i] = Math.max(1,
/// this.getLongestWordWidth(headerText, maxUnbrokenWordWidth))` (`:871`) and the row pass at
/// `:877-880`. Those per-column minima ARE `minColumnWidths` (`:884`); the all-1s collapse at `:888`
/// is the exception, taken only when their sum exceeds `availableForCells` (`:887`). cyrup shrank
/// straight to a floor of 1 in every over-wide case, so a table one cell too wide shredded a word
/// that upstream keeps whole.
///
/// A user sees this in any assistant message with a table narrower than its natural width — the
/// `/hotkeys` block below is the same path through a real command.
#[test]
fn m15_a_table_column_is_floored_at_its_longest_word_capped_at_thirty() {
    let theme = UiTheme::dark();
    // Two columns, border overhead 3*2+1 = 7, so a pane of 37 leaves availableForCells = 30.
    //
    // Hand-traced: naturalWidths = [20, 39]; minWordWidths = [20, 1] (`:871`, `:877-880`), whose sum
    // 21 <= 30, so NO collapse (`:887`). totalNatural 59 + 7 > 37 → the shrink arm (`:920-934`):
    // totalGrowPotential = (20-20) + (39-1) = 38, extraWidth = 30 - 21 = 9, so column A grows by
    // floor(0/38 * 9) = 0 and stays at its 20-cell floor while B takes all 9 → [20, 10].
    let word = "supercalifragilistic"; // exactly 20 cells
    assert_eq!(word.chars().count(), 20);
    let filler = "a a a a a a a a a a a a a a a a a a a a"; // 20 tokens, 39 cells
    let md = format!("| A | B |\n|---|---|\n| {word} | {filler} |");
    let r = rows(&render_markdown(&md, 37, &theme));

    // The word survives on ONE row. Under the old floor-at-1 arithmetic the columns came out
    // [11, 19] and it was hard-broken into `supercalifr` / `agilistic`.
    assert!(
        r.iter().any(|l| l.contains(word)),
        "the 20-cell word was broken instead of being floored at its own width:\n{r:?}"
    );
    assert_eq!(
        r.iter().filter(|l| l.contains(word)).count(),
        1,
        "the word must occupy exactly one row:\n{r:?}"
    );
    // The columns themselves, read off the frame: `┌─` + 20 + `─┬─` + 10 + `─┐` (`:956`).
    assert_eq!(
        r.first().map(String::as_str),
        Some(&*format!("┌{}┬{}┐", "─".repeat(22), "─".repeat(12))),
        "column widths are not [20, 10]:\n{r:?}"
    );
    assert_all_rows_fit(&render_markdown(&md, 37, &theme), 37, "m15 floored table");

    // MIRROR — the floor is CAPPED at 30 (`:863`), and when the capped minima no longer fit,
    // `:888-908` collapses to all-1s and hands the slack back by weight. A 40-cell word therefore
    // still breaks: minWordWidths = [30, 1] sums to 31 > 30, so minColumnWidths becomes [1, 1] and
    // the 28 spare cells go to column A by weight → [29, 1].
    let long = "abcdefghijklmnopqrstuvwxyzabcdefghijklmn"; // 40 cells
    assert_eq!(long.chars().count(), 40);
    let md2 = format!("| A | B |\n|---|---|\n| {long} | {filler} |");
    let r2 = rows(&render_markdown(&md2, 37, &theme));
    assert!(
        !r2.iter().any(|l| l.contains(long)),
        "a word wider than maxUnbrokenWordWidth must still break:\n{r2:?}"
    );
    assert_eq!(
        r2.first().map(String::as_str),
        Some(&*format!("┌{}┬{}┐", "─".repeat(31), "─".repeat(3))),
        "the capped-then-collapsed widths are not [29, 1]:\n{r2:?}"
    );
    assert_all_rows_fit(&render_markdown(&md2, 37, &theme), 37, "m15 capped table");
}

/// **M7 — a table cell is rendered by `renderInlineTokens`, not printed as plain text.**
///
/// `markdown.ts:960` and `:983` both call `this.renderInlineTokens(cell.tokens || [], styleContext)`
/// — the identical call a paragraph makes at `:492` — so `**bold**`, `` `code` ``, `[a](b)` and
/// `~~del~~` keep their styling inside the grid, and the widths at `:870`/`:876` are
/// `visibleWidth()` of that styled string.
///
/// cyrup pushed the cell's raw text into a `String`, so every inline style was dropped; worse, the
/// link arm's ` (url)` suffix went through `push_text` into the ROW buffer instead of the cell and
/// surfaced glued to the table's top border.
#[test]
fn m7_inline_formatting_survives_inside_a_table_cell() {
    let theme = UiTheme::dark();
    let md = "| Name | Note |\n|------|------|\n| **Ada** | `fn` |";
    let lines = render_markdown(md, 40, &theme);

    // `theme.bold` — SGR-1 over the cell's own run, adding no foreground (`:673-676`).
    let bold = find_row(&lines, "Ada")
        .spans
        .iter()
        .find(|s| s.content.contains("Ada"))
        .expect("the `Ada` cell span");
    assert!(
        bold.style.add_modifier.contains(Modifier::BOLD),
        "`**Ada**` lost its bold inside the cell: {:?}",
        bold.style
    );

    // `case "codespan": result += this.theme.code(token.text)` (`:685-687`).
    let code = find_row(&lines, "fn")
        .spans
        .iter()
        .find(|s| s.content.contains("fn"))
        .expect("the `fn` cell span");
    assert_eq!(
        code.style.fg,
        theme.md_code_style().fg,
        "`` `fn` `` lost mdCode inside the cell: {:?}",
        code.style
    );

    // …and the body cell around them is NOT bold, so the header/body difference still reads.
    let plain_header = find_row(&lines, "Name")
        .spans
        .iter()
        .find(|s| s.content.contains("Name"))
        .expect("header cell");
    assert!(plain_header.style.add_modifier.contains(Modifier::BOLD), "header band lost bold");

    // MIRROR — the link arm. `token.text !== token.href`, so the incapable-terminal fallback prints
    // `styledLink + this.theme.linkUrl(" (href)")` (`:697-706`), and BOTH halves belong to the
    // CELL. Before this batch the suffix was pushed onto the row buffer and came out fused to the
    // grid's top border.
    let linked = "| Ref |\n|-----|\n| [doc](https://ex.com) |";
    // Pinned to the incapable branch: the ` (url)` suffix exists ONLY there (`markdown.ts:544-554`
    // @v0.83.0), so leaving this to the ambient `TERM_PROGRAM` made the assertion a property of the
    // developer's terminal rather than of the renderer. Upstream pins the same way for the same
    // reason, in this very table block — `packages/tui/test/markdown.test.ts:469-470` @v0.83.0,
    // "Pin to no-hyperlinks so width checks work on plain text without OSC 8 sequences." The rest
    // of this file already honours that convention (see the note at `:132-134`).
    let plain = render_markdown_with_hyperlinks(linked, 40, &theme, false);
    let lr = rows(&plain);
    assert!(
        lr.iter().any(|l| l.starts_with('┌')),
        "the top border was polluted by the link suffix:\n{lr:?}"
    );
    assert!(
        lr.iter().any(|l| l.contains("doc (https://ex.com)")),
        "the link text and its url suffix must both be inside the cell:\n{lr:?}"
    );
    let link_span = find_row(&plain, "doc")
        .spans
        .iter()
        .find(|s| s.content.contains("doc"))
        .map(|s| s.style)
        .expect("link cell span");
    assert_eq!(link_span.fg, theme.md_link_style().fg, "link text lost mdLink inside the cell");

    // MIRROR — the OSC-8 branch, which used to be whichever one the host happened to provide.
    // Upstream prints the link text alone there, "regardless of whether it matches href"
    // (`markdown.ts:540-543` @v0.83.0), so the cell holds `doc`, the column is 3 wide, and nothing
    // leaks onto the frame on this branch either.
    let cap = rows(&render_markdown_with_hyperlinks(linked, 40, &theme, true));
    assert!(
        cap.iter().any(|l| l.starts_with('┌')),
        "the capable branch polluted the top border:\n{cap:?}"
    );
    assert!(cap.iter().any(|l| l.contains("doc")), "the capable branch lost the link text:\n{cap:?}");
    assert!(
        !cap.iter().any(|l| l.contains("ex.com")),
        "the capable branch printed the url inline:\n{cap:?}"
    );
}

/// M7 through a real command: `/hotkeys` writes `| \`Ctrl+A\` | Start of line |` rows
/// (`app.rs:1847` `hotkeys_markdown`, Pi `handleHotkeysCommand`, interactive-mode.ts:6090-6205), so
/// every key cell is a codespan and upstream renders it through `theme.code`. With cells captured as
/// plain text the whole `/hotkeys` table came out in body colour.
#[test]
fn m7_hotkeys_key_cells_render_as_code_spans() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.editor_mut().set_text("/hotkeys");
    app.handle_input(&crate::InputEvent::Key(
        crate::crossterm::event::KeyEvent::new(
            crate::crossterm::event::KeyCode::Enter,
            crate::crossterm::event::KeyModifiers::NONE,
        ),
    ));
    app.draw().unwrap();
    let theme = UiTheme::dark();
    let code_fg = theme.md_code_style().fg.expect("mdCode has a foreground");
    let buf = app.terminal().backend().buffer();
    let mut code_cells = 0usize;
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(c) = buf.cell((x, y))
                && c.fg == code_fg
                && c.symbol().trim() != ""
            {
                code_cells += 1;
            }
        }
    }
    assert!(
        code_cells > 0,
        "no `/hotkeys` key cell rendered in mdCode — table cells are still plain text:\n{}",
        app.scrollback_text()
    );
}

/// **M12 — inline LaTeX is tokenized and typeset, and falls back to its RAW source.**
///
/// `markdown.ts:123-144` registers the `latex` inline tokenizer; `:645-652` renders it as
/// `renderLatex(latexToken.text) ?? latexToken.raw`. Two things were broken before this batch:
/// nothing typeset math at all, and `\[` / `\]` were eaten by CommonMark's backslash escapes, so
/// `\[x\]` printed as `[x]` — the delimiters vanished and the reader could not even tell it had
/// been math.
#[test]
fn m12_inline_math_is_typeset_and_falls_back_to_its_raw_source() {
    let theme = UiTheme::dark();

    let r = rows(&render_markdown("Euler: $e^{i\\pi}+1=0$ indeed.\n", 60, &theme));
    assert_eq!(
        r,
        vec!["Euler: e^(iπ)+1 = 0 indeed.".to_string()],
        "inline `$…$` was not typeset:\n{r:?}"
    );

    // `\(…\)` and `\[…\]` are the other two inline openers (`markdown.ts:97-103`).
    let paren = rows(&render_markdown("a \\(\\alpha\\) b\n", 60, &theme));
    assert_eq!(paren, vec!["a α b".to_string()], "`\\(…\\)` not tokenized:\n{paren:?}");
    let bracket = rows(&render_markdown("a \\[\\alpha\\] b\n", 60, &theme));
    assert_eq!(
        bracket,
        vec!["a α b".to_string()],
        "`\\[…\\]` was consumed as two CommonMark escapes:\n{bracket:?}"
    );

    // MIRROR — `renderLatex` declines, so the RAW source prints, delimiters and all (`:650`).
    let raw = rows(&render_markdown("unsupported $x + \\unknown{y}$ here\n", 60, &theme));
    assert_eq!(
        raw,
        vec!["unsupported $x + \\unknown{y}$ here".to_string()],
        "an unsupported expression must print verbatim, never half-rendered:\n{raw:?}"
    );
}

/// **M12 — a block token renders with `{ display: true }`, i.e. stacked.**
///
/// `renderLatex(latexToken.text, { display: true }) ?? latexToken.raw.trim()` (`markdown.ts:505-512`),
/// then `for (const line of rendered.split("\n")) lines.push(...)` (`:511-513`) — one output row per
/// rendered row, which is what makes a stacked fraction legible.
#[test]
fn m12_block_math_stacks_in_display_mode() {
    let theme = UiTheme::dark();
    let r = rows(&render_markdown("\\[\n\\frac{x^2+1}{x-1}\n\\]\n", 60, &theme));
    assert_eq!(
        r,
        vec!["x²+1".to_string(), "────".to_string(), "x-1".to_string()],
        "block math did not stack:\n{r:?}"
    );

    // `$$…$$` is the other block opener, and a limit operator stacks the same way.
    let sum = rows(&render_markdown("$$\\sum_{i=0}^n x_i$$\n", 60, &theme));
    assert_eq!(
        sum,
        vec![" n".to_string(), " ∑  xᵢ".to_string(), "i=0".to_string()],
        "`$$…$$` did not stack its limits:\n{sum:?}"
    );

    // MIRROR — the SAME expression inline is one row, unstacked (`{ display: false }`).
    let inline = rows(&render_markdown("x $\\sum_{i=0}^n x_i$ y\n", 60, &theme));
    assert_eq!(
        inline,
        vec!["x ∑ᵢ₌₀ⁿ xᵢ y".to_string()],
        "inline math must not stack:\n{inline:?}"
    );

    // A block token nested in a list item keeps the item's indent; the `{0,3}` leading spaces the
    // tokenizer swallows have to come back or the rows fall out of the item.
    let in_list = rows(&render_markdown("- item\n\n  $$\\frac{1}{2}$$\n\n- next\n", 60, &theme));
    assert!(
        in_list.iter().any(|l| l == "  ─"),
        "block math lost its list-item indent:\n{in_list:?}"
    );
    assert!(in_list.iter().any(|l| l == "- next"), "the next item was swallowed:\n{in_list:?}");
}

/// **M12 — the tokenizer must not fire inside code, and must not eat currency.**
///
/// marked never re-lexes a fenced block's body, and its inline extensions are offered the text at
/// the backtick (where `tokenizeInlineLatex` declines) before `codespan` swallows the span. The
/// `$`-specific guards at `markdown.ts:110-118` are what keep `$5` and `` $`x`$ `` out of math.
#[test]
fn m12_math_is_not_tokenized_inside_code_or_in_prices() {
    let theme = UiTheme::dark();

    let fenced = rows(&render_markdown("```\n$\\alpha$\n```\n", 60, &theme));
    assert!(
        fenced.iter().any(|l| l.contains("$\\alpha$")),
        "a fenced block's body was tokenized as math:\n{fenced:?}"
    );
    assert!(!fenced.iter().any(|l| l.contains('α')), "math leaked into a code fence:\n{fenced:?}");

    let span = rows(&render_markdown("code `$\\alpha$` span\n", 60, &theme));
    assert!(
        span.iter().any(|l| l.contains("$\\alpha$")),
        "an inline code span was tokenized as math:\n{span:?}"
    );

    // `/^\d/.test(after)` (`markdown.ts:112`) — `$5 and $10` is a price, not a math span.
    let price = rows(&render_markdown("It costs $5 and $10 today.\n", 60, &theme));
    assert_eq!(
        price,
        vec!["It costs $5 and $10 today.".to_string()],
        "a price was eaten as math:\n{price:?}"
    );

    // `/\s$/.test(inner)` (`markdown.ts:111`) — a trailing space inside the delimiters disqualifies.
    let spaced = rows(&render_markdown("a $x $ b\n", 60, &theme));
    assert_eq!(spaced, vec!["a $x $ b".to_string()], "trailing-space guard lost:\n{spaced:?}");
}

/// M12 through the user action that reaches it: an assistant message containing math, committed to
/// the transcript, lands typeset in scrollback.
#[test]
fn m12_assistant_math_reaches_scrollback_typeset() {
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    app.transcript_mut()
        .commit_assistant(Some("The identity $e^{i\\pi}+1=0$ closes it.".to_string()));
    app.draw().unwrap();
    let sb = app.scrollback_text();
    assert!(sb.contains("e^(iπ)+1 = 0"), "assistant math was not typeset:\n{sb}");
    assert!(!sb.contains("$e^"), "the raw delimiters survived:\n{sb}");
}

/// **M7 — a STYLED cell that WRAPS keeps its style on every row it wraps onto.**
///
/// This is the claim the batch that moved table cells from `String` to `Vec<Span>` actually made —
/// "so styles survive the break" — and the one thing none of its tests exercised: every M7 case
/// used a cell short enough that `wrap_cell`'s `visibleLength <= width` early return
/// (`utils.ts:862-865`) handed the cell back untouched, so the break path never ran.
///
/// Upstream a cell is `renderInlineTokens(cell.tokens, styleContext)` (`markdown.ts:960`, `:983`)
/// fed to `wrapCellText` = `wrapTextWithAnsi` (`:829-831`), whose `breakLongWord`/`AnsiCodeTracker`
/// machinery exists for exactly this: `tracker.getActiveCodes()` is re-emitted at the head of each
/// continuation row (`utils.ts:845`, `:1007`) so a bold cell stays bold past the fold.
#[test]
fn m7_a_styled_cell_keeps_its_style_across_a_wrap() {
    let theme = UiTheme::dark();
    // The Action column cannot hold `alpha beta gamma delta` at width 30, so the cell wraps.
    let src = "| Key | Action |\n|-----|--------|\n| `x` | **alpha beta gamma delta** |\n";
    let out = render_markdown(src, 30, &theme);
    let r = rows(&out);
    let first = find_row(&out, "alpha");
    let later = find_row(&out, "delta");
    assert!(
        !std::ptr::eq(first, later),
        "the cell did not wrap — this test proves nothing unless it does:\n{r:?}"
    );
    for (label, row) in [("first", first), ("continuation", later)] {
        let word = if label == "first" { "alpha" } else { "delta" };
        let span = row
            .spans
            .iter()
            .find(|s| s.content.contains(word))
            .unwrap_or_else(|| panic!("no span carrying {word:?} on the {label} row: {row:?}"));
        assert!(
            span.style.add_modifier.contains(Modifier::BOLD),
            "the {label} row lost `**…**` across the break — \
             a plain-`str` cell wrapper cannot carry a style over a fold: {row:?}"
        );
    }
}

/// **M7 (separator styling) — the space between two differently-styled words is NOT the preceding
/// word's style.**
///
/// `wrapSingleLine` never *creates* a separator: `splitIntoTokensWithAnsi` (`utils.ts:775-798`)
/// emits the whitespace run as its own token and `currentLine += token` (`:923`) appends it
/// verbatim, with whatever ANSI state the source put around it. `renderInlineTokens` puts the space
/// after `**alpha**` OUTSIDE the SGR-1/22 pair, so it is ambient. Splitting the cell on `/\s+/` and
/// re-inserting `" "` with `line.last()`'s style bolded that gap (and collapsed `a  b` to `a b`).
#[test]
fn m7_the_gap_between_a_bold_word_and_a_plain_one_is_not_bold() {
    let theme = UiTheme::dark();
    // Wide enough that `alpha beta` share a row, narrow enough that the cell still wraps.
    let src = "| Key | Action |\n|-----|--------|\n| `x` | **alpha** beta gamma delta |\n";
    let out = render_markdown(src, 30, &theme);
    let r = rows(&out);
    let row = find_row(&out, "alpha");
    assert!(line_text(row).contains("alpha beta"), "need both words on one row:\n{r:?}");
    let bold: Vec<&str> = row
        .spans
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(bold, vec!["alpha"], "the separator was swept into the bold run: {row:?}");
}

/// **M7 (separator preservation) — a run of whitespace inside a wrapping cell is kept, not
/// collapsed.** `splitIntoTokensWithAnsi` keeps `"  "` as ONE token and `wrapSingleLine` appends it
/// unchanged (`utils.ts:923`); only a fold consumes it (`:911-913` "Don't start new line with
/// whitespace") and only a line end trims it (`:935`).
#[test]
fn m7_interior_whitespace_runs_survive_a_wrapping_cell() {
    let theme = UiTheme::dark();
    let src = "| Key | Action |\n|-----|--------|\n| `x` | alpha  beta gamma delta |\n";
    let out = render_markdown(src, 30, &theme);
    let r = rows(&out);
    let row = find_row(&out, "alpha");
    assert!(
        line_text(row).contains("alpha  beta"),
        "the double space was re-packed to a single one:\n{r:?}"
    );
}
