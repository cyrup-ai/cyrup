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

use cyrup_session_svc::{ExportState, ExportTheme, session_jsonl_to_html_with_theme};

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
    let state = ExportState::from_file();
    let dark =
        session_jsonl_to_html_with_theme(JSONL, &theme(cyrup_resources::BUILTIN_DARK_JSON), &state);
    let light = session_jsonl_to_html_with_theme(
        JSONL,
        &theme(cyrup_resources::BUILTIN_LIGHT_JSON),
        &state,
    );

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

/// DRIFT-041 / DRIFT-054 — the TUI's two HTML export sites must hand the renderer the LIVE session
/// state, not let it re-derive a leaf from the JSONL and not let the agent's two keys go missing.
///
/// pi passes `sm.getLeafId()` AND `this.state` into `exportSessionToHtml`
/// (`core/export-html/index.ts:263-270` @v0.84.4, from `agent-session.ts:3439`). cyrup's manager
/// moves its leaf without appending (`SessionManager::branch`), so after a `/tree` switch with no
/// new message the last line of the exported file belongs to the abandoned branch; and without
/// `systemPrompt`/`tools` the document loses its System Prompt and Available Tools sections
/// (`template.js:1403-1452`). `AgentSession::export_state()` composes all three. Read from the
/// source for the same reason `theme_reapply_on_reload.rs` reads its arm: neither `/export` nor
/// `/share` can be driven from this crate without a live `AgentSession`.
#[test]
fn both_tui_export_sites_pass_the_live_session_state() {
    for (label, src) in [
        ("/export", include_str!("../app/execute_session.rs")),
        ("/share", include_str!("../app/execute_misc.rs")),
    ] {
        let call = src
            .find("session_jsonl_to_html_with_theme(")
            .unwrap_or_else(|| panic!("{label} must still render through the shared renderer"));
        let args = src
            .get(call..(call + 220).min(src.len()))
            .unwrap_or_default();
        assert!(
            args.contains("session.export_state()"),
            "{label} must pass `session.export_state()` — without the leaf the exported document \
             walks the branch the user abandoned (DRIFT-041), and without the agent state it drops \
             the System Prompt and Available Tools sections (DRIFT-054)"
        );
    }
}
