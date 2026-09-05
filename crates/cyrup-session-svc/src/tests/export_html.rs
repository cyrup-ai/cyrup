//! DRIFT-041 — the HTML session export is pi's templated document, not a text dump.
//!
//! Upstream: pi `v0.84.4` `packages/coding-agent/src/core/export-html/` (`index.ts` 316 lines,
//! `template.js` 1864, `template.css` 1066, `template.html` 55, plus the two vendored libraries).
//! `generateHtml` (`index.ts:143-175`) base64-encodes a `SessionData{header, entries, leafId, …}`
//! payload into `<script id="session-data">` and substitutes five placeholders into
//! `template.html`, while `template.css`'s own four carry the ACTIVE theme's colours
//! (`:151-157`, `template.css:2-5`).
//!
//! What the old 131-line renderer did instead, and what each test below pins:
//! * it emitted one `<pre>` per string found under a `"text"` key, so tool-call **arguments**
//!   (bash's `command`, edit's diff) were dropped outright and a tool **result** lost its tool
//!   name, call id, `isError` and `details` — `tool_calls_keep_their_name_arguments_and_result_metadata`;
//! * it ignored `parentId`/`leafId`, interleaving abandoned branches with the active path —
//!   `branching_history_is_exported_as_a_tree_with_a_leaf`;
//! * it hardcoded a `#1e1e2e` palette — `palette_comes_from_the_active_theme_not_a_constant`;
//! * it shipped no markdown renderer and no highlighter — `document_carries_the_template_and_both_vendored_libraries`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use base64::Engine as _;
use cyrup_resources::{ResourceOrigin, ResourceScope, Theme};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::export::{
    CssColor, ExportState, ExportTheme, ExportTool, derive_export_colors, session_jsonl_to_html,
    session_jsonl_to_html_with_theme,
};

/// A transcript exercising every claim in DRIFT-041's Verify: a fenced code block, a `bash` tool
/// call PLUS its result, a custom (extension) tool result, and two branches off one parent.
const FIXTURE: &str = concat!(
    r#"{"type":"session","version":3,"id":"0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000","timestamp":"2026-09-04T10:00:00.000Z","cwd":"/home/dev/proj"}"#,
    "\n",
    r#"{"type":"message","id":"aaaaaaaa","parentId":null,"timestamp":"2026-09-04T10:00:01.000Z","message":{"role":"user","content":"run the build"}}"#,
    "\n",
    r#"{"type":"message","id":"bbbbbbbb","parentId":"aaaaaaaa","timestamp":"2026-09-04T10:00:02.000Z","message":{"role":"assistant","model":"claude","content":[{"type":"text","text":"Here it is:\n\n```rust\nfn main() {}\n```"},{"type":"toolCall","id":"call_1","name":"bash","arguments":{"command":"cargo build --release"}}]}}"#,
    "\n",
    r#"{"type":"message","id":"cccccccc","parentId":"bbbbbbbb","timestamp":"2026-09-04T10:00:03.000Z","message":{"role":"toolResult","toolCallId":"call_1","toolName":"bash","isError":false,"details":{"exitCode":0},"content":[{"type":"text","text":"Finished release"}]}}"#,
    "\n",
    r#"{"type":"message","id":"dddddddd","parentId":"cccccccc","timestamp":"2026-09-04T10:00:04.000Z","message":{"role":"toolResult","toolCallId":"call_2","toolName":"weather","isError":true,"content":[{"type":"text","text":"no such city"}]}}"#,
    "\n",
    r#"{"type":"message","id":"eeeeeeee","parentId":"cccccccc","timestamp":"2026-09-04T10:00:05.000Z","message":{"role":"user","content":"try again"}}"#,
    "\n",
);

/// Decode the `<script id="session-data">` payload back out of the document — the round trip
/// `template.js:8-15` performs in the browser.
fn session_data(html: &str) -> Value {
    let open = "<script id=\"session-data\" type=\"application/json\">";
    let start = html.find(open).expect("session-data script element") + open.len();
    let rest = html.get(start..).expect("payload start in bounds");
    let end = rest.find("</script>").expect("session-data closed");
    let b64 = rest.get(..end).expect("payload end in bounds");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .expect("payload is base64");
    serde_json::from_slice(&bytes).expect("payload is JSON")
}

fn builtin(json: &str) -> Theme {
    Theme::parse(json, None, ResourceScope::Builtin, ResourceOrigin::Builtin).expect("built-in")
}

// ---------------------------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------------------------

/// pi's template scaffold reaches the reader: the sidebar with its search box and five filters
/// (`template.html:12-40`), the image modal, and BOTH vendored libraries that `template.js`
/// depends on (`marked.parse` at `:1641`, `hljs.highlight` at `:857`/`:1618`). Every one of the
/// nine `{{…}}` placeholders must be gone.
#[test]
fn document_carries_the_template_and_both_vendored_libraries() {
    let html = session_jsonl_to_html(FIXTURE);

    assert!(html.starts_with("<!DOCTYPE html>"), "a full document");
    assert!(html.trim_end().ends_with("</html>"));

    // template.html:15-32 — the sidebar the old renderer had no equivalent of at all.
    assert!(html.contains("id=\"tree-search\""), "sidebar search box");
    for filter in ["default", "no-tools", "user-only", "labeled-only", "all"] {
        assert!(
            html.contains(&format!("data-filter=\"{filter}\"")),
            "filter button {filter}"
        );
    }
    assert!(
        html.contains("id=\"sidebar-resizer\""),
        "resizable tree pane"
    );
    assert!(html.contains("id=\"image-modal\""), "image modal");

    // template.js's own machinery.
    assert!(
        html.contains("function buildTree()"),
        "tree builder shipped"
    );

    // The vendored libraries, by their licence headers (see assets/vendor/README.md).
    assert!(
        html.contains("marked v18.0.5 - a markdown parser"),
        "marked is inlined"
    );
    assert!(
        html.contains("Highlight.js v11.9.0"),
        "highlight.js is inlined"
    );
    assert!(
        html.contains("hljs.highlight(code, { language: lang })"),
        "code blocks are highlighted at render time"
    );

    for placeholder in [
        "{{CSS}}",
        "{{JS}}",
        "{{SESSION_DATA}}",
        "{{MARKED_JS}}",
        "{{HIGHLIGHT_JS}}",
        "{{THEME_VARS}}",
        "{{BODY_BG}}",
        "{{CONTAINER_BG}}",
        "{{INFO_BG}}",
    ] {
        assert!(
            !html.contains(placeholder),
            "{placeholder} was never substituted"
        );
    }
}

/// `SessionData{header, entries, leafId}` (`index.ts:263-270`, `:298-304`): the payload rebuilds
/// EXACTLY the JSONL, header line included, with the session line excluded from `entries` the way
/// `getEntries()` excludes it (`session-manager.ts:1301`).
#[test]
fn embedded_session_data_reproduces_the_jsonl_exactly() {
    let html = session_jsonl_to_html(FIXTURE);
    let data = session_data(&html);

    let mut lines = FIXTURE.lines().filter(|l| !l.trim().is_empty());
    let expected_header: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    let expected_entries: Vec<Value> = lines.map(|l| serde_json::from_str(l).unwrap()).collect();

    assert_eq!(data["header"], expected_header);
    assert_eq!(data["entries"].as_array().unwrap(), &expected_entries);
    assert_eq!(
        data["header"]["type"], "session",
        "the header line is the header, not an entry"
    );
    assert!(
        data["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["type"] != "session"),
        "no session line leaks into entries"
    );
    // `_buildIndex` leaves `leafId` at the LAST non-session entry (`session-manager.ts:960-968`),
    // which is `SessionManager::leaf_id()`'s contract too.
    assert_eq!(data["leafId"], "eeeeeeee");
}

/// The defect at the heart of DRIFT-041: `collect_text` harvested only values under a `"text"`
/// key, so a tool call's `arguments` were dropped and a result arrived as an unattributed `<pre>`.
#[test]
fn tool_calls_keep_their_name_arguments_and_result_metadata() {
    let html = session_jsonl_to_html(FIXTURE);
    let data = session_data(&html);
    let entries = data["entries"].as_array().unwrap();

    let call = &entries[1]["message"]["content"][1];
    assert_eq!(call["type"], "toolCall");
    assert_eq!(call["name"], "bash");
    assert_eq!(call["id"], "call_1");
    assert_eq!(
        call["arguments"]["command"], "cargo build --release",
        "the command the old renderer dropped"
    );

    let result = &entries[2]["message"];
    assert_eq!(result["toolName"], "bash");
    assert_eq!(result["toolCallId"], "call_1");
    assert_eq!(result["isError"], false);
    assert_eq!(result["details"]["exitCode"], 0);

    // A custom (extension) tool result keeps its error flag and name too, even though its
    // `renderedTools` card is the module's stated residual.
    let custom = &entries[3]["message"];
    assert_eq!(custom["toolName"], "weather");
    assert_eq!(custom["isError"], true);

    // Assistant markdown travels as source and is rendered by `marked` in the browser, so the
    // fence must survive verbatim rather than being flattened into a `<pre>`.
    let text = entries[1]["message"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("```rust"), "fenced block preserved");
}

/// `parentId`/`leafId` were ignored, so abandoned branches were interleaved with the active path.
/// `dddddddd` and `eeeeeeee` are siblings off `cccccccc`; `template.js:73-111` rebuilds that tree
/// and `:116-142` walks the active path from `leafId`.
#[test]
fn branching_history_is_exported_as_a_tree_with_a_leaf() {
    let html = session_jsonl_to_html(FIXTURE);
    let data = session_data(&html);
    let entries = data["entries"].as_array().unwrap();

    let parent_of = |id: &str| -> Value {
        entries
            .iter()
            .find(|e| e["id"] == id)
            .unwrap_or_else(|| panic!("entry {id}"))["parentId"]
            .clone()
    };
    assert_eq!(parent_of("aaaaaaaa"), Value::Null, "root");
    assert_eq!(parent_of("dddddddd"), "cccccccc");
    assert_eq!(parent_of("eeeeeeee"), "cccccccc");
    assert_eq!(
        data["leafId"], "eeeeeeee",
        "the active path is selected by leafId, not by file order"
    );
}

/// DRIFT-041 review fix — the leaf is the MANAGER's, not the file's last line.
///
/// pi passes `sm.getLeafId()` into `generateHtml` (`core/export-html/index.ts:266` @v0.84.4), and
/// `branch` / `branchWithSummary` reassign that pointer WITHOUT appending
/// (`core/session-manager.ts:1361-1365`, `:1393`), exactly as `SessionManager::branch` does. In
/// `FIXTURE`, `dddddddd` and `eeeeeeee` are siblings off `cccccccc` and `eeeeeeee` is last in the
/// file; a user who runs `/tree` and switches back to the `dddddddd` branch has a manager leaf of
/// `dddddddd` and has appended nothing, so the last-line rule names an entry on the branch they
/// just abandoned and `template.js:116-142` walks the wrong conversation.
///
/// RED before the fix: `session_jsonl_to_html_with_theme` took no leaf and always derived
/// `eeeeeeee` here.
#[test]
fn an_explicit_leaf_wins_over_the_last_file_entry() {
    let html = session_jsonl_to_html_with_theme(
        FIXTURE,
        &ExportTheme::default(),
        &ExportState::live(Some("dddddddd".to_string()), String::new(), Vec::new()),
    );
    let data = session_data(&html);
    assert_eq!(
        data["leafId"], "dddddddd",
        "the manager's leaf, not the last line of the file"
    );
    // Nothing else moves: the whole tree still travels, as pi's `sm.getEntries()` does.
    assert_eq!(data["entries"].as_array().unwrap().len(), 5);

    // And `None` is still pi's `exportFromFile` rule (`index.ts:288-305`), which is all a
    // file-only caller can know.
    let derived = session_data(&session_jsonl_to_html_with_theme(
        FIXTURE,
        &ExportTheme::default(),
        &ExportState::from_file(),
    ));
    assert_eq!(derived["leafId"], "eeeeeeee");
}

/// The shell must actually supply that leaf. Read from the source because nothing in this crate can
/// construct an `AgentSession` (it owns a provider, a registry and a live manager lock) — the same
/// reason `crates/cyrup-tui/src/tests/theme_reapply_on_reload.rs` reads its swap arm from source.
/// Without this guard, `export_leaf_id` could be dropped from `export_to_html` and every test above
/// would still pass while each branched export silently regressed.
#[test]
fn export_to_html_passes_the_live_session_state_to_the_renderer() {
    const TRANSCRIPT_SRC: &str = include_str!("../session/transcript.rs");
    let offset = TRANSCRIPT_SRC
        .find("pub async fn export_to_html")
        .expect("transcript.rs must still define `export_to_html`");
    let body = &TRANSCRIPT_SRC[offset..];
    let call = body
        .find("session_jsonl_to_html_with_theme(")
        .expect("`export_to_html` must still render through the pure renderer");
    let call_args = &body[call..(call + 200).min(body.len())];
    assert!(
        call_args.contains("self.export_state()"),
        "`export_to_html` must pass `self.export_state()` to the renderer — pi passes \
         `sm.getLeafId()` AND `this.state` (`core/export-html/index.ts:263-270`, \
         `agent-session.ts:3439` @v0.84.4). Re-deriving the leaf from the JSONL names an abandoned \
         branch after a `/tree` switch (DRIFT-041), and dropping the state loses the System Prompt \
         and Available Tools sections (DRIFT-054)"
    );
}

/// DRIFT-054 — the LIVE export carries pi's two `AgentState` keys.
///
/// pi's `exportSessionToHtml` sets `systemPrompt: state?.systemPrompt` and
/// `tools: state?.tools?.map((t) => ({ name, description, parameters }))`
/// (`core/export-html/index.ts:267-268` @v0.84.4), and `AgentSession.exportToHtml` ALWAYS passes
/// `this.state` (`agent-session.ts:3439`) — it is the only entry point `/export`, `/share` and RPC
/// `export_html` have. The byte-identical `template.js` this crate ships renders a collapsible
/// **System Prompt** block and an **Available Tools** list from exactly those two keys
/// (`:1403-1452`, destructured at `:15`), so without them every exported document was missing two
/// visible sections.
///
/// RED before the fix: `session_data` inserted `header`, `entries` and `leafId` only, so both
/// lookups were `Value::Null`.
#[test]
fn a_live_export_carries_the_system_prompt_and_the_active_tools() {
    let state = ExportState::live(
        Some("dddddddd".to_string()),
        "You are cyrup.\nBe brief.".to_string(),
        vec![
            ExportTool {
                name: "bash".to_string(),
                description: "Run a shell command".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": { "command": { "type": "string", "description": "the command" } },
                    "required": ["command"],
                }),
            },
            ExportTool {
                name: "read".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({ "type": "object", "properties": {} }),
            },
        ],
    );
    let data = session_data(&session_jsonl_to_html_with_theme(
        FIXTURE,
        &ExportTheme::default(),
        &state,
    ));

    assert_eq!(
        data["systemPrompt"], "You are cyrup.\nBe brief.",
        "`systemPrompt: state?.systemPrompt` (`index.ts:267`) — `template.js:1404` renders the \
         System Prompt block from it"
    );

    let tools = data["tools"]
        .as_array()
        .expect("`tools` must be an array — `template.js:1425` reads `tools.length`");
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0]["name"], "bash");
    assert_eq!(tools[0]["description"], "Run a shell command");
    assert_eq!(
        tools[0]["parameters"]["properties"]["command"]["type"],
        "string"
    );
    assert_eq!(tools[1]["name"], "read");
    // Upstream picks exactly three fields (`index.ts:268`) — nothing else travels.
    assert_eq!(
        {
            let mut keys = tools[0]
                .as_object()
                .expect("a tool entry is an object")
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>();
            // Sorted rather than taken in map order because `serde_json::Map` is NOT a `BTreeMap`
            // in this build: `serde_json` is declared with `preserve_order` at the workspace
            // (`6200b011`), so every map in the workspace is an `IndexMap` and iterates in
            // INSERTION order — here `name`, `description`, `parameters`, the field order of
            // `ExportTool`. Sorting keeps this test asserting the KEY SET it says it asserts
            // instead of the alphabetical accident of the old map type. Same flip, same fix as
            // ICOM-054 (`71fefe30`).
            keys.sort_unstable();
            keys
        },
        vec!["description", "name", "parameters"],
    );
    // The leaf still travels beside them.
    assert_eq!(data["leafId"], "dddddddd");
}

/// …and the FILE-only export still omits both keys, because that is what pi's `exportFromFile`
/// produces: `systemPrompt: undefined, tools: undefined` (`core/export-html/index.ts:298-304`
/// @v0.84.4), which `JSON.stringify` drops. `cyrup --export` has no live session to read them from.
#[test]
fn the_file_only_export_omits_both_agent_keys_the_way_pi_does() {
    let data = session_data(&session_jsonl_to_html(FIXTURE));
    let obj = data.as_object().expect("payload is an object");
    assert!(
        !obj.contains_key("systemPrompt"),
        "`exportFromFile` sets it `undefined`, and `JSON.stringify` omits an undefined value"
    );
    assert!(!obj.contains_key("tools"));
    // `renderedTools` is never set on either path (the documented low residual).
    assert!(!obj.contains_key("renderedTools"));
}

/// A live session whose tool set is EMPTY still sends `tools: []`, not nothing: pi's `state.tools`
/// is a required array (`packages/agent/src/types.ts:341-342` @v0.84.4), so `state?.tools?.map(...)`
/// yields `[]`. `template.js:1425`'s `tools && tools.length > 0` renders the two cases identically,
/// but the payload is what this seam owes upstream.
#[test]
fn an_empty_live_tool_set_is_an_empty_array_not_an_absent_key() {
    let data = session_data(&session_jsonl_to_html_with_theme(
        FIXTURE,
        &ExportTheme::default(),
        &ExportState::live(None, String::new(), Vec::new()),
    ));
    assert_eq!(data["tools"], serde_json::json!([]));
    assert_eq!(data["systemPrompt"], "");
}

/// Base64 is why nothing on this path is HTML-escaped (`index.ts:159-160`): no transcript byte can
/// close the `<script>` element it travels in.
#[test]
fn transcript_text_cannot_break_out_of_the_script_element() {
    let hostile = r#"</script><script>alert(1)</script>"#;
    let jsonl = format!(
        "{{\"type\":\"session\",\"id\":\"s\"}}\n{{\"type\":\"message\",\"id\":\"a\",\"parentId\":null,\"message\":{{\"role\":\"user\",\"content\":{}}}}}\n",
        serde_json::to_string(hostile).unwrap()
    );
    let html = session_jsonl_to_html(&jsonl);
    assert!(
        !html.contains(hostile),
        "the hostile string never appears literally"
    );
    let data = session_data(&html);
    assert_eq!(
        data["entries"][0]["message"]["content"], hostile,
        "…but it round-trips intact inside the payload"
    );
}

/// Same tolerance the old renderer had, and the same as pi's own `parseSessionEntryLine`
/// (`session-manager.ts:503-511`): a blank or corrupt line costs that entry, not the export.
#[test]
fn malformed_and_empty_input_still_produce_a_document() {
    let html = session_jsonl_to_html(
        "{\"type\":\"session\",\"id\":\"s\"}\n\nnot json at all\n{\"type\":\"message\",\"id\":\"a\",\"parentId\":null,\"message\":{\"role\":\"user\",\"content\":\"ok\"}}\n",
    );
    let data = session_data(&html);
    assert_eq!(data["entries"].as_array().unwrap().len(), 1);
    assert_eq!(data["leafId"], "a");

    let empty = session_jsonl_to_html("");
    let data = session_data(&empty);
    assert_eq!(data["header"], Value::Null);
    assert_eq!(data["entries"].as_array().unwrap().len(), 0);
    assert_eq!(data["leafId"], Value::Null);
    assert!(empty.contains("id=\"tree-container\""), "still a document");
}

// ---------------------------------------------------------------------------------------------
// The palette
// ---------------------------------------------------------------------------------------------

/// pi resolves `{{THEME_VARS}}`/`{{BODY_BG}}`/`{{CONTAINER_BG}}`/`{{INFO_BG}}` from the ACTIVE
/// theme (`index.ts:151-157`); cyrup used to emit a constant `#1e1e2e` stylesheet.
#[test]
fn palette_comes_from_the_active_theme_not_a_constant() {
    let dark = ExportTheme::from_theme(&builtin(cyrup_resources::BUILTIN_DARK_JSON));
    let light = ExportTheme::from_theme(&builtin(cyrup_resources::BUILTIN_LIGHT_JSON));
    assert_ne!(dark, light, "the two built-ins are different palettes");

    let dark_html = session_jsonl_to_html_with_theme(FIXTURE, &dark, &ExportState::from_file());
    let light_html = session_jsonl_to_html_with_theme(FIXTURE, &light, &ExportState::from_file());

    // The explicit `export` blocks of the two built-ins (`cyrup-resources/src/theme.rs:602-606`,
    // `:689-693`), which pi prefers over the derived triple (`index.ts:155-157`).
    assert!(dark_html.contains("--body-bg: #18181e;"), "dark page bg");
    assert!(dark_html.contains("--container-bg: #1e1e24;"));
    assert!(dark_html.contains("--info-bg: #3c3728;"));
    assert!(light_html.contains("--body-bg: #f8f8f8;"), "light page bg");
    assert!(light_html.contains("--container-bg: #ffffff;"));
    assert!(light_html.contains("--info-bg: #fffae6;"));

    // `generateThemeVars` emits every role as a CSS custom property (`index.ts:113-116`).
    assert!(dark_html.contains("--exportPageBg: #18181e;"));
    assert!(
        dark_html.contains("--accent: "),
        "roles reach the stylesheet"
    );
    assert!(
        dark_html.contains("--userMessageBg: #343541;"),
        "a var reference is resolved through `vars`"
    );

    // …and the constant the old renderer hardcoded is nowhere in either document.
    assert!(!dark_html.contains("#1e1e2e"), "no hardcoded palette");
    assert!(!light_html.contains("#1e1e2e"));
}

/// `withThemeColorFallbacks` (`theme.ts:332-346`): four aliases the schema leaves optional but the
/// stylesheet still references.
#[test]
fn optional_role_aliases_fall_back_the_way_pi_does() {
    let dark = ExportTheme::from_theme(&builtin(cyrup_resources::BUILTIN_DARK_JSON));
    for (alias, source) in [
        ("scrollbarThumb", "selectedBg"),
        ("searchMatchBg", "selectedBg"),
        ("searchMatchText", "text"),
    ] {
        assert_eq!(
            dark.role(alias),
            dark.role(source),
            "{alias} falls back to {source}"
        );
        assert!(dark.role(alias).is_some(), "{alias} is emitted at all");
    }
    // `thinkingMax` IS declared by both built-ins, so the alias must NOT overwrite it.
    assert_eq!(dark.role("thinkingMax"), "#ff5fff".parse::<CssColor>().ok());
}

/// `deriveExportColors` (`index.ts:81-106`) — the arm a theme with no `export` block takes.
/// Expected values computed from upstream's arithmetic, not from this implementation.
#[test]
fn derived_backdrops_match_pi_arithmetic() {
    // Dark base (`luminance <= 0.5`): 0.7 / 0.85 / (+20, +15, +0).
    let d = derive_export_colors(Some(CssColor::from_rgb(0x2a, 0x2a, 0x33)));
    assert_eq!(d.page_bg.to_string(), "#1d1d24");
    assert_eq!(d.card_bg.to_string(), "#24242b");
    assert_eq!(d.info_bg.to_string(), "#3e3933");

    // Light base (`luminance > 0.5`): 0.96 / identity / (+10, +5, −20).
    let l = derive_export_colors(Some(CssColor::from_rgb(0xff, 0xff, 0xff)));
    assert_eq!(l.page_bg.to_string(), "#f5f5f5");
    assert_eq!(l.card_bg.to_string(), "#ffffff");
    assert_eq!(l.info_bg.to_string(), "#ffffeb");

    // pi's `if (!parsed)` constant triple (`index.ts:83-88`).
    let none = derive_export_colors(None);
    assert_eq!(none.page_bg, CssColor::from_rgb(24, 24, 30));
    assert_eq!(none.card_bg, CssColor::from_rgb(30, 30, 36));
    assert_eq!(none.info_bg, CssColor::from_rgb(60, 55, 40));
}

/// The [`CssColor`] parse boundary — pi `parseColor` (`index.ts:43-61`) accepts exactly two
/// spellings, and both regexes are anchored.
#[test]
fn css_color_parses_only_pis_two_spellings() {
    assert_eq!(
        "#1e1e2e".parse::<CssColor>().unwrap(),
        CssColor::from_rgb(0x1e, 0x1e, 0x2e)
    );
    assert_eq!(
        "rgb(24, 24, 30)".parse::<CssColor>().unwrap(),
        CssColor::from_rgb(24, 24, 30)
    );
    assert_eq!(
        "rgb (  1,2 ,3 )".parse::<CssColor>().unwrap(),
        CssColor::from_rgb(1, 2, 3),
        "upstream's `\\s*` are everywhere the regex puts them"
    );
    for bad in [
        "",
        "#fff",
        "#1e1e2",
        "#gggggg",
        "red",
        "rgb(1,2)",
        "rgb(1,2,3,4)",
        "rgba(1,2,3,1)",
        "rgb(1,-2,3)",
        "#1e1e2e ",
    ] {
        assert!(
            bad.parse::<CssColor>().is_err(),
            "{bad:?} is not a pi colour"
        );
    }
    // Round trip: everything this type emits, it can read back.
    let c = CssColor::from_rgb(0, 128, 255);
    assert_eq!(c.to_string().parse::<CssColor>().unwrap(), c);
}

/// `adjustBrightness` (`index.ts:73-78`): `min(255, max(0, round(c * factor)))`, so it saturates
/// rather than wrapping, both ways.
#[test]
fn adjust_brightness_saturates_at_both_ends() {
    let white = "#ffffff".parse::<CssColor>().unwrap();
    assert_eq!(white.adjust_brightness(2.0).to_string(), "#ffffff");
    assert_eq!(white.adjust_brightness(0.0).to_string(), "#000000");
    // JS `Math.round` and Rust's agree on non-negative halves (both go up).
    assert_eq!(
        CssColor::from_rgb(1, 1, 1)
            .adjust_brightness(0.5)
            .to_string(),
        "#010101"
    );
    assert!(!"#000000".parse::<CssColor>().unwrap().is_light());
    assert!(white.is_light());
}

// ---------------------------------------------------------------------------------------------
// Asset provenance
// ---------------------------------------------------------------------------------------------

/// The five embedded assets are byte-identical copies of pi v0.84.4's
/// `packages/coding-agent/src/core/export-html/`. A local edit — a "small fix" to `template.js`,
/// a re-minified vendor drop — fails here instead of silently forking cyrup's export from
/// upstream's. Re-derive with
/// `git -C tmp/pi show v0.84.4:packages/coding-agent/src/core/export-html/<file> | sha256sum`.
#[test]
fn embedded_assets_are_byte_identical_to_pi_v0_84_4() {
    let pins = [
        (
            "template.html",
            include_str!("../export/assets/template.html"),
            "916782b1184a9597527605ad751e2b3af30fcea23ba2194002969cd217a06881",
        ),
        (
            "template.css",
            include_str!("../export/assets/template.css"),
            "28c16e3827c23a62eef8283cac316b478f946e023093adad25ff9c9b891d41af",
        ),
        (
            "template.js",
            include_str!("../export/assets/template.js"),
            "1893cdb77587f592eef5717905391269886d6d7a4dc6a488417a73da374d9226",
        ),
        (
            "vendor/marked.min.js",
            include_str!("../export/assets/vendor/marked.min.js"),
            "d5487edc7258b404bfa74c393d74a6393155f02517bd5e7e77cd64f8187f39a0",
        ),
        (
            "vendor/highlight.min.js",
            include_str!("../export/assets/vendor/highlight.min.js"),
            "837a6fa5b0c736b52bbde2b2b6190f305da3fc9ed41681db5321507057b5c846",
        ),
    ];
    for (name, bytes, expected) in pins {
        let got = format!("{:x}", Sha256::digest(bytes.as_bytes()));
        assert_eq!(got, expected, "{name} drifted from pi v0.84.4");
    }
}
