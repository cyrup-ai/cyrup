//! HTML session-export tests (spec/tui/04; Pi `core/export-html`). `/export <path>` routes by
//! extension; this covers the in-crate HTML body renderer over the session's JSONL.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::session_jsonl_to_html;

#[test]
fn renders_standalone_html_document_with_entries() {
    let jsonl = "{\"name\":\"My Session\",\"cwd\":\"/home/dev/proj\"}\n\
        {\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hello world\"}]}}\n\
        {\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hi there\"}]}}\n";
    let html = session_jsonl_to_html(jsonl);
    assert!(html.starts_with("<!doctype html>"), "must be a full document");
    assert!(html.contains("<title>My Session</title>"));
    assert!(html.contains("/home/dev/proj"));
    assert!(html.contains("hello world"));
    assert!(html.contains("hi there"));
    assert!(html.contains("class=\"entry user\""));
    assert!(html.contains("class=\"entry assistant\""));
    assert!(html.trim_end().ends_with("</html>"));
}

#[test]
fn escapes_html_significant_characters() {
    let jsonl = "{\"name\":\"S\"}\n\
        {\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<script>&\\\"x\\\"\"}]}}\n";
    let html = session_jsonl_to_html(jsonl);
    assert!(html.contains("&lt;script&gt;"), "angle brackets escaped");
    assert!(html.contains("&amp;"), "ampersand escaped");
    assert!(!html.contains("<script>"), "no raw script tag leaks through");
}

#[test]
fn skips_unparseable_and_empty_lines_without_panicking() {
    let jsonl = "{\"name\":\"S\"}\n\nnot json at all\n\
        {\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}\n";
    let html = session_jsonl_to_html(jsonl);
    assert!(html.contains("ok"));
    assert!(html.contains("class=\"entry assistant\""));
}

#[test]
fn empty_input_still_yields_a_document() {
    let html = session_jsonl_to_html("");
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("<title>Session</title>"));
}
