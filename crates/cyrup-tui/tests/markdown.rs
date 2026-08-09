//! Markdown + syntax rendering tests (spec/tui/06 §2-§3).
//!
//! Exercises the `pulldown-cmark` walk (`render_markdown`) and the `syntect` fenced-code highlight,
//! plus the streaming partial-fence trim, asserting on the produced styled `Line`s and on a rendered
//! `TestBackend` buffer.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::{render_markdown, trim_partial_closing_fence, App, UiTheme};
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};
use ratatui::text::Line;

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

    // link → text underlined + trailing ` (url)`.
    let link = render_markdown("[docs](https://example.com)\n", 80, &theme);
    let lt = rows(&link).join("\n");
    assert!(has_span_mod(&link, "docs", Modifier::UNDERLINED), "link text not underlined");
    assert!(lt.contains("(https://example.com)"), "link url not appended:\n{lt}");
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
