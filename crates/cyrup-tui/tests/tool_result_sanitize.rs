//! Tool-result text is sanitized before it is rendered — Pi `getTextOutput`,
//! `render-utils.ts:48`: `sanitizeBinaryOutput(stripAnsi(c.text || "")).replace(/\r/g, "")`.
//!
//! Why this is not covered by ratatui: ratatui filters *control* characters out of every grapheme
//! run it lays into cells (`Span::styled_graphemes`, ratatui-core `text/span.rs:314`;
//! `Buffer::set_stringn`, `buffer/buffer.rs:351`), so the `ESC` introducer never reaches the
//! terminal and an escape sequence cannot execute. What it does NOT remove is the rest of the
//! sequence — `[1;31m`, `]8;;file:///…`, `[?25l` are ordinary printable characters — so an
//! unstripped tool result renders as literal garbage in the transcript. Unicode format characters
//! (U+FFF9..U+FFFB) are not control characters either and survive the same way.
//!
//! `bash` output is already sanitized at capture time (`cyrup-session-svc/src/bash.rs:292`
//! `sanitize_chunk`); every OTHER tool — `read`, `ls`, `find`, `grep`, and any extension tool —
//! reaches the transcript raw, which is the path these tests cover.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_core::ToolCallId;
use cyrup_session_svc::AgentSessionEvent;
use cyrup_tui::{App, UiTheme};
use ratatui::backend::TestBackend;
use serde_json::json;

/// Run one `ls` to completion and return the committed transcript text.
///
/// A finished tool leaves the viewport on the next draw (`commit_finished_leading_tools` →
/// `flush_committed`), so scrollback is where the settled block lives.
fn ls_result_text(output: &str) -> String {
    let mut app = App::new(TestBackend::new(100, 24), UiTheme::dark()).unwrap();
    app.ingest_event(&AgentSessionEvent::ToolExecutionStart {
        tool_call_id: ToolCallId::from("call-ls"),
        tool_name: "ls".to_string(),
        args: json!({ "path": "." }),
    });
    app.ingest_event(&AgentSessionEvent::ToolExecutionEnd {
        tool_call_id: ToolCallId::from("call-ls"),
        tool_name: "ls".to_string(),
        result: json!({ "content": [{ "type": "text", "text": output }] }),
        is_error: false,
    });
    app.draw().unwrap();
    app.scrollback_text()
}

/// `stripAnsi` — the SGR/CSI half. ratatui eats the `ESC`; the parameter bytes are printable and
/// would otherwise be read as file names.
#[test]
fn sgr_sequences_do_not_survive_as_literal_text() {
    let text = ls_result_text("\u{1b}[1;31msrc\u{1b}[0m\nREADME.md\n\u{1b}[?25lCargo.toml");

    assert!(text.contains("src"), "content lost:\n{text}");
    assert!(text.contains("README.md"), "content lost:\n{text}");
    assert!(text.contains("Cargo.toml"), "content lost:\n{text}");

    assert!(!text.contains("[1;31m"), "SGR parameters rendered as text:\n{text}");
    assert!(!text.contains("[0m"), "SGR reset rendered as text:\n{text}");
    assert!(!text.contains("[?25l"), "cursor-hide sequence rendered as text:\n{text}");
}

/// `stripAnsi` — the OSC half (`ESC ] … BEL` / `ESC ] … ESC \`). An OSC-8 hyperlink is what a
/// colorizing `ls` or a `grep` wrapper actually emits, and its payload is a whole URL.
#[test]
fn osc_sequences_do_not_survive_as_literal_text() {
    let bel = ls_result_text("\u{1b}]8;;file:///tmp/x\u{7}linked\u{1b}]8;;\u{7}\nplain.txt");
    assert!(bel.contains("linked"), "content lost:\n{bel}");
    assert!(bel.contains("plain.txt"), "content lost:\n{bel}");
    assert!(!bel.contains("8;;"), "OSC payload rendered as text:\n{bel}");
    assert!(!bel.contains("file:///tmp/x"), "OSC URL rendered as text:\n{bel}");

    // String terminator form `ESC \` rather than BEL.
    let st = ls_result_text("\u{1b}]0;window title\u{1b}\\kept.txt");
    assert!(st.contains("kept.txt"), "content lost:\n{st}");
    assert!(!st.contains("window title"), "OSC payload rendered as text:\n{st}");
}

/// `sanitizeBinaryOutput` (`utils/shell.ts:144-174`). U+FFF9..U+FFFB are Unicode *format*
/// characters, not control characters, so ratatui's control filter does not touch them — they are
/// exactly the class Pi filters because they break width measurement.
#[test]
fn unicode_format_characters_are_filtered() {
    let text = ls_result_text("before\u{fff9}mid\u{fffa}dle\u{fffb}after.txt");
    assert!(text.contains("after.txt"), "content lost:\n{text}");
    for bad in ['\u{fff9}', '\u{fffa}', '\u{fffb}'] {
        assert!(!text.contains(bad), "U+{:04X} survived:\n{text}", bad as u32);
    }
}

/// MIRROR — green with or without the sanitizer. Ordinary output that merely *looks* like it
/// contains escape syntax must be left completely alone; nothing here depends on stripping. Its job
/// is to show the assertions above are not vacuous, and to catch an over-eager stripper that eats
/// real content.
#[test]
fn mirror_ordinary_text_is_untouched() {
    let text = ls_result_text("items[0m1].rs\nsrc/main.rs\nnotes ]8;; draft.md");

    assert!(text.contains("items[0m1].rs"), "bracket text mangled:\n{text}");
    assert!(text.contains("src/main.rs"), "path mangled:\n{text}");
    assert!(text.contains("notes ]8;; draft.md"), "literal ]8;; mangled:\n{text}");
}
