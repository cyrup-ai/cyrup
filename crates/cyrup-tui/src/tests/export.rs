//! HTML session-export tests (spec/tui/04; Pi `core/export-html`). `/export <path>` routes by
//! extension; this covers the seam the TUI reaches for when the target is not `.jsonl`.
//!
//! The renderer itself is `cyrup-session-svc`'s and is pinned there
//! (`cyrup-session-svc/src/tests/export_html.rs`, DRIFT-041). What is this crate's to prove is that
//! the re-export still resolves to that renderer and that a theme reaches the document — the two
//! `/export` and `/share` call sites pass `session.export_theme()`, so a regression that dropped
//! the palette would otherwise be invisible from here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cyrup_resources::{ResourceOrigin, ResourceScope, Theme};

use cyrup_session_svc::{ExportTheme, session_jsonl_to_html_with_theme};

use crate::export::session_jsonl_to_html;

const JSONL: &str = concat!(
    r#"{"type":"session","version":3,"id":"0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000","cwd":"/home/dev/proj"}"#,
    "\n",
    r#"{"type":"message","id":"aaaaaaaa","parentId":null,"message":{"role":"user","content":"hello world"}}"#,
    "\n",
);

/// The re-export still lands on pi's templated document, not on a text dump: the sidebar scaffold
/// (`template.html:15-32`) and the embedded payload (`:42`) are both present.
#[test]
fn the_reexport_renders_pis_templated_document() {
    let html = session_jsonl_to_html(JSONL);
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(html.contains("<script id=\"session-data\" type=\"application/json\">"));
    assert!(html.contains("id=\"tree-search\""), "sidebar search box");
    assert!(html.contains("id=\"tree-container\""), "tree pane");
    assert!(
        !html.contains("{{SESSION_DATA}}"),
        "placeholders substituted"
    );
    assert!(html.trim_end().ends_with("</html>"));
}

/// `/export` and `/share` hand the renderer `session.export_theme()`; both built-ins produce a
/// DIFFERENT document, which is what makes that argument load-bearing rather than decorative.
#[test]
fn the_active_theme_changes_the_exported_palette() {
    let theme = |json: &str| {
        ExportTheme::from_theme(
            &Theme::parse(json, None, ResourceScope::Builtin, ResourceOrigin::Builtin).unwrap(),
        )
    };
    let dark = session_jsonl_to_html_with_theme(JSONL, &theme(cyrup_resources::BUILTIN_DARK_JSON));
    let light =
        session_jsonl_to_html_with_theme(JSONL, &theme(cyrup_resources::BUILTIN_LIGHT_JSON));

    assert!(dark.contains("--body-bg: #18181e;"));
    assert!(light.contains("--body-bg: #f8f8f8;"));
    assert_ne!(dark, light);
    // The constant palette the pre-DRIFT-041 renderer hardcoded.
    assert!(!dark.contains("#1e1e2e"));
}

/// A transcript with nothing in it still opens (the `/export` path never refuses).
#[test]
fn empty_input_still_yields_a_document() {
    let html = session_jsonl_to_html("");
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(html.contains("id=\"messages\""));
}
