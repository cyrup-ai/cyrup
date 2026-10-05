//! Links in markdown bodies: pi's `link` token (`markdown.ts:699-713` @v1.0.0).
//!
//! On a terminal that forwards OSC-8 the link text is wrapped in a hyperlink and the URL is not
//! printed; on any other it is followed by ` (href)` unless the text already is the href. marked's
//! GFM `url` tokenizer makes bare `https://…`, `www.…` and `me@x.io` in running text links too.
//!
//! The escapes exist only in a painted `Buffer` (`crate::osc`), so every test paints rows into one,
//! injects, and reads cell symbols back — and the same three paths that inject for tool headers
//! are driven: the inline scrollback flush, the live viewport and the fullscreen document.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::widgets::{Paragraph, Widget};

use crate::ansi::strip_ansi;
use crate::markdown::{MdLinks, render_with_links};
use crate::osc::{LinkSink, inject};
use crate::theme::UiTheme;
use crate::transcript::{ImageOpts, entry_lines};
use crate::{App, Entry};

const CLOSE: &str = "\u{1b}]8;;\u{7}";

fn open(url: &str) -> String {
    format!("\u{1b}]8;;{url}\u{7}")
}

/// Render `text` as markdown with the given OSC-8 capability, paint it, inject the escapes, and
/// return the painted cells of the rows (escapes included) with their styles.
fn painted(text: &str, enabled: bool) -> Buffer {
    let sink = LinkSink::new();
    let lines = render_with_links(
        text,
        60,
        &UiTheme::dark(),
        None,
        false,
        MdLinks {
            enabled,
            sink: Some(&sink),
        },
    );
    paint(lines, &sink)
}

fn paint(lines: Vec<ratatui::text::Line<'static>>, sink: &LinkSink) -> Buffer {
    let area = Rect::new(0, 0, 60, u16::try_from(lines.len()).unwrap().max(1));
    let mut buf = Buffer::empty(area);
    Paragraph::new(lines).render(area, &mut buf);
    inject(&mut buf, sink);
    buf
}

/// The row's symbols concatenated, escapes and all.
fn raw_rows(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

/// The same rows as the user reads them.
fn visible(buf: &Buffer) -> Vec<String> {
    raw_rows(buf)
        .iter()
        .map(|r| strip_ansi(r).trim_end().to_string())
        .collect()
}

#[test]
fn an_explicit_link_is_a_hyperlink_and_prints_no_url_on_a_capable_terminal() {
    let buf = painted("see [the docs](https://x.io/d) now", true);
    let raw = raw_rows(&buf).join("\n");
    assert!(
        raw.contains(&format!("{}t", open("https://x.io/d"))),
        "the link opens on its first character: {raw:?}"
    );
    assert!(
        raw.contains(&format!("s{CLOSE}")),
        "and closes on its last: {raw:?}"
    );
    assert_eq!(visible(&buf), ["see the docs now"]);
}

#[test]
fn an_explicit_link_prints_its_url_when_the_terminal_cannot_link() {
    let buf = painted("see [the docs](https://x.io/d) now", false);
    assert_eq!(visible(&buf), ["see the docs (https://x.io/d) now"]);
    assert!(raw_rows(&buf).iter().all(|r| !r.contains('\u{1b}')));
}

#[test]
fn a_bare_url_is_a_link_with_its_trailing_punctuation_outside_it() {
    let buf = painted("go https://x.io/a/b_c_d. Next", true);
    let raw = raw_rows(&buf).join("\n");
    assert!(
        raw.contains(&format!("{}h", open("https://x.io/a/b_c_d"))),
        "the whole URL — underscores included — is one link: {raw:?}"
    );
    assert!(
        raw.contains(&format!("d{CLOSE}. Next")),
        "the full stop is not part of it: {raw:?}"
    );
}

#[test]
fn a_bare_url_is_styled_as_a_link_even_without_the_capability() {
    let buf = painted("go https://x.io/a now", false);
    assert_eq!(
        visible(&buf),
        ["go https://x.io/a now"],
        "text equals href: no suffix"
    );
    let x = 3; // the `h`
    assert!(
        buf[(x, 0)].modifier.contains(Modifier::UNDERLINED),
        "underlined like any link"
    );
    assert!(!buf[(0, 0)].modifier.contains(Modifier::UNDERLINED));
}

#[test]
fn a_www_link_gets_an_http_href_and_a_mailto_for_an_email() {
    let buf = painted("at www.x.io or me@x.io", true);
    let raw = raw_rows(&buf).join("\n");
    assert!(raw.contains(&open("http://www.x.io")), "{raw:?}");
    assert!(raw.contains(&open("mailto:me@x.io")), "{raw:?}");

    // Incapable: `www.` text differs from its href, so the suffix is printed; an email's does not
    // (`markdown.ts:701-707` strips `mailto:` for the comparison).
    let buf = painted("at www.x.io or me@x.io", false);
    assert_eq!(visible(&buf), ["at www.x.io (http://www.x.io) or me@x.io"]);
}

/// marked's `url` tokenizer is not tried inside a code span.
#[test]
fn a_url_in_a_code_span_is_not_a_link() {
    let raw = raw_rows(&painted("run `https://a.io` now", true)).join("\n");
    assert!(!raw.contains('\u{1b}'), "{raw:?}");
}

/// …nor inside a fenced block.
#[test]
fn a_url_in_a_code_fence_is_not_a_link() {
    let raw = raw_rows(&painted("```\nhttps://e.io\n```", true)).join("\n");
    assert!(!raw.contains('\u{1b}'), "{raw:?}");
}

/// …nor inside a link (`!this.state.inLink`): the explicit link is one link, to its own href.
#[test]
fn a_url_in_link_text_does_not_nest_a_second_link() {
    let raw = raw_rows(&painted("[https://b.io](https://c.io)", true)).join("\n");
    assert!(raw.contains(&open("https://c.io")), "{raw:?}");
    assert!(!raw.contains(&open("https://b.io")), "{raw:?}");
}

/// …nor in an image's alt text, which marked's `image` token keeps raw.
#[test]
fn a_url_in_image_alt_text_is_not_a_link() {
    let raw = raw_rows(&painted("![see https://d.io](p.png)", true)).join("\n");
    assert!(!raw.contains('\u{1b}'), "{raw:?}");
}

#[test]
fn links_in_a_heading_and_a_table_cell_keep_their_markers() {
    let buf = painted("## see https://x.io/h", true);
    assert!(raw_rows(&buf).join("\n").contains(&open("https://x.io/h")));

    let buf = painted("| a | b |\n|---|---|\n| [t](https://x.io/t) | y |", true);
    assert!(raw_rows(&buf).join("\n").contains(&open("https://x.io/t")));
}

/// A reasoning run, a summary and a changelog block are markdown too.
#[test]
fn links_in_thinking_and_summary_bodies_are_hyperlinks() {
    let theme = UiTheme::dark();
    let entries = [
        Entry::Thinking {
            text: "read https://x.io/think".into(),
            hidden: false,
        },
        Entry::BranchSummary {
            summary: "see https://x.io/branch".into(),
        },
        Entry::Block {
            title: "T".into(),
            markdown: "see https://x.io/block".into(),
        },
    ];
    for (entry, url) in entries.iter().zip([
        "https://x.io/think",
        "https://x.io/branch",
        "https://x.io/block",
    ]) {
        let sink = LinkSink::new();
        let opts = ImageOpts {
            hyperlinks: true,
            links: Some(&sink),
            tools_expanded: true,
            ..ImageOpts::default()
        };
        let lines = entry_lines(entry, &theme, 60, 0, opts);
        let raw = raw_rows(&paint(lines, &sink)).join("\n");
        assert!(raw.contains(&open(url)), "{url} is not a link: {raw:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// the three renderers
// ---------------------------------------------------------------------------------------------

fn cell_rows(buf: &Buffer) -> String {
    raw_rows(buf).join("\n")
}

/// The fullscreen document: committed assistant and user bodies.
#[test]
fn markdown_links_are_hyperlinks_in_the_fullscreen_document() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.state_mut().show_startup_hints = false;
    app.state_mut().startup_header = crate::StartupHeader::Hidden;
    let _captured = app.enter_fullscreen_captured().expect("renderer builds");
    app.transcript_mut().set_hyperlinks(true);
    app.transcript_mut().push_user("mine: https://x.io/user");
    app.transcript_mut()
        .commit_assistant(Some("see [docs](https://x.io/assistant) here".into()));
    app.draw().unwrap();
    let buf = app
        .altscreen_for_test()
        .unwrap()
        .backend_for_test()
        .buffer()
        .clone();
    let raw = cell_rows(&buf);
    assert!(
        raw.contains(&open("https://x.io/user")),
        "user body: {raw:?}"
    );
    assert!(
        raw.contains(&open("https://x.io/assistant")),
        "assistant body: {raw:?}"
    );
}

/// The fullscreen live region: the streaming reply.
#[test]
fn a_streaming_reply_link_is_a_hyperlink_in_the_fullscreen_document() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.state_mut().show_startup_hints = false;
    app.state_mut().startup_header = crate::StartupHeader::Hidden;
    let _captured = app.enter_fullscreen_captured().expect("renderer builds");
    app.transcript_mut().set_hyperlinks(true);
    app.transcript_mut()
        .push_assistant_delta("streaming https://x.io/live");
    app.draw().unwrap();
    let buf = app
        .altscreen_for_test()
        .unwrap()
        .backend_for_test()
        .buffer()
        .clone();
    assert!(cell_rows(&buf).contains(&open("https://x.io/live")));
}

/// The inline scrollback flush: committed entries are painted into native scrollback with the
/// escapes injected (`app/draw.rs`'s `insert_before` closure).
#[test]
fn markdown_links_are_hyperlinks_in_the_inline_scrollback_flush() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.state_mut().show_startup_hints = false;
    app.transcript_mut().set_hyperlinks(true);
    app.transcript_mut()
        .commit_assistant(Some("see [docs](https://x.io/flushed) here".into()));
    app.draw().unwrap();
    let backend = app.terminal().backend();
    let all = format!(
        "{}\n{}",
        cell_rows(backend.buffer()),
        cell_rows(backend.scrollback())
    );
    assert!(all.contains(&open("https://x.io/flushed")), "{all:?}");
}

/// The inline live viewport: the streaming reply.
#[test]
fn a_streaming_reply_link_is_a_hyperlink_in_the_inline_viewport() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.state_mut().show_startup_hints = false;
    app.transcript_mut().set_hyperlinks(true);
    app.transcript_mut()
        .push_assistant_delta("streaming https://x.io/live");
    app.draw().unwrap();
    let all = cell_rows(app.terminal().backend().buffer());
    assert!(all.contains(&open("https://x.io/live")), "{all:?}");
}
