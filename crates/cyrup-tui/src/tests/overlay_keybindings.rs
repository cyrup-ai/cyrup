//! TUI-126 — an extension reads the HOST's effective keybinding table, not `keybindings.json`.
//!
//! `/llama` resolved its keys with `LlamaKeys::from_agent_dir`, re-reading the user's file once per
//! open. That carried neither of the two things the host's table has: the
//! platform-conditional defaults of `core/keybindings.ts` (which the file does not contain and
//! `cyrup-llama` does not reimplement), and LIVENESS — a rebind between opens was invisible.
//!
//! `HostServices::effective_keybindings(namespace)` answers the merged, live table, published from
//! the app's per-frame readback alongside the editor buffer and the active theme.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::{Arc, Mutex};

use cyrup_ext::host::{
    InteractiveOverlay, OverlayKey, OverlayLine, OverlayOutcome as ExtOverlayOutcome, OverlaySpan,
};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::overlay::ExtensionOverlay;
use crate::{App, InputEvent, UiTheme};

fn app() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.state_mut().show_startup_hints = false;
    app.state_mut().startup_header = crate::StartupHeader::Hidden;
    app
}

/// The mirror answers every namespace the app owns after a frame, each carrying its own ids — and
/// a namespace is an id PREFIX, so `tui.input` is readable in its own right even though those ids
/// are bound in the EDITOR's map (`EditorAction::Submit` and friends).
#[test]
fn the_host_publishes_every_namespace_it_owns() {
    let mut app = app();
    app.draw().unwrap();
    let mirror = app.state().keybinding_mirror.clone();

    let app_ns = mirror.bindings("app");
    assert!(
        app_ns.get("app.interrupt").is_some(),
        "the app namespace carries its own ids: {app_ns}"
    );

    let editor = mirror.bindings("tui.editor");
    assert!(
        editor.get("tui.editor.undo").is_some(),
        "the editor namespace carries its own ids: {editor}"
    );
    assert!(
        editor.get("tui.input.submit").is_none(),
        "a `tui.editor` read is the `tui.editor.` PREFIX, not the editor map's whole table: {editor}"
    );

    // The payoff of filtering by prefix instead of keying by table: these four ids are bound in
    // the EDITOR's keymap (`EditorAction::{NewLine, Submit, Tab, PassThrough}`), so a store keyed
    // by table name could only ever have answered them under `tui.editor` — an extension asking
    // upstream's own `tui.input` would have got `{}`.
    let input = mirror.bindings("tui.input");
    for id in ["tui.input.submit", "tui.input.newLine", "tui.input.tab"] {
        assert!(
            input.get(id).is_some(),
            "`{id}` is readable under its own namespace: {input}"
        );
    }
    // The fourth id, `tui.input.copy`, is bound to nothing by `EditorKeymap::default` ON PURPOSE
    // (TUI-067: cyrup's editor map binds no `ctrl+c`, so the chord already reaches the app tier).
    // The absent-when-unbound rule therefore omits it, which is the right answer, not a gap.
    assert!(
        input.get("tui.input.copy").is_none(),
        "`tui.input.copy` is unbound by default (TUI-067), so it is absent: {input}"
    );

    // The namespaces the four extra keymaps contribute, which `/llama` does not read but an
    // extension may ask for; before this they answered `{}`.
    for (ns, id) in [
        ("app.tree", "app.tree.foldOrUp"),
        ("app.session", "app.session.accept"),
        ("app.models", "app.models.accept"),
        ("app.thinking", "app.thinking.accept"),
        ("tui.altScreen", "tui.altScreen.bottom"),
    ] {
        let table = mirror.bindings(ns);
        assert!(
            !table.as_object().is_none_or(serde_json::Map::is_empty),
            "`{ns}` is published: {table}"
        );
        let _ = id;
    }

    let select = mirror.bindings("tui.select");
    assert!(
        select.get("tui.select.cancel").is_some(),
        "the select namespace carries its own ids: {select}"
    );

    assert_eq!(
        mirror.bindings("no.such.namespace"),
        serde_json::json!({}),
        "an unknown namespace answers an empty table, not a default-filled one"
    );
}

/// The published table is the EFFECTIVE one: a user rebind merged into the live keymap shows up,
/// which is the liveness `from_agent_dir` could not have.
#[test]
fn a_rebind_reaches_the_published_table_on_the_next_frame() {
    let mut app = app();
    app.draw().unwrap();
    let before = app.state().keybinding_mirror.bindings("app");
    let had = before
        .get("app.interrupt")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(had > 0, "fixture: `app.interrupt` starts bound: {before}");

    // Rebind through the same door a user's `keybindings.json` comes in by.
    let issues = app
        .state_mut()
        .keymap
        .merge_json(r#"{ "app.interrupt": ["ctrl+alt+k"] }"#)
        .expect("the merge parses");
    assert!(issues.is_empty(), "fixture: a clean merge: {issues:?}");
    app.draw().unwrap();

    let after = app.state().keybinding_mirror.bindings("app");
    let keys = after
        .get("app.interrupt")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        keys,
        vec![serde_json::Value::String("ctrl+alt+k".to_string())],
        "the rebind must reach the table an extension reads: {after}"
    );
}

/// An id bound to NOTHING is absent rather than present-and-empty, which is how upstream's table
/// reads — `app.suspend` is unbound on `win32` (`core/keybindings.ts:96-99`) — so a caller can tell
/// "no keys" from "I was not told".
#[test]
fn an_unbound_id_is_absent_from_the_table() {
    let mut app = app();
    app.state_mut()
        .keymap
        .merge_json(r#"{ "app.interrupt": [] }"#)
        .expect("the merge parses");
    app.draw().unwrap();
    let table = app.state().keybinding_mirror.bindings("app");
    assert!(
        table.get("app.interrupt").is_none(),
        "an id with no keys is omitted: {table}"
    );
    // Without this the test is VACUOUS: an UNPUBLISHED table is also `{}`, so "the id is absent"
    // would pass for the wrong reason. The table must be populated and missing just this one.
    assert!(
        table.get("app.exit").is_some(),
        "the rest of the namespace is still published: {table}"
    );
}

/// An overlay that records every key the host routed to it.
struct Recorder(Arc<Mutex<Vec<OverlayKey>>>);

impl InteractiveOverlay for Recorder {
    fn render(&mut self, _width: usize, _height: usize) -> Vec<OverlayLine> {
        vec![OverlayLine::new(vec![OverlaySpan::raw("overlay")])]
    }

    fn handle_key(&mut self, key: OverlayKey) -> ExtOverlayOutcome {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(key);
        ExtOverlayOutcome::Redraw
    }
}

/// The row's second half: `ctrl+c` — a DEFAULT `tui.select.cancel` key, and globally
/// `app.message.clear` — reaches an open extension overlay before any global handler, pinned
/// through `App` rather than at the `LlamaView` level where the lane had left it.
///
/// A NON-REGRESSION GUARD, not a proof of this change: it passes at HEAD too. The row says the
/// behaviour is already correct and asks only that it be pinned at this level, which is what this
/// does — the routing it covers is `app/input.rs`'s, not anything TUI-126 added.
///
/// `app/input.rs` routes it by POSITION: `if !overlays.is_empty() { return handle_overlay_key(key) }`
/// sits above every global arm, which is pi's own ordering (the overlay stack is consulted first).
#[test]
fn ctrl_c_reaches_an_open_overlay_before_any_global_handler() {
    let mut app = app();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (done, _released) = tokio::sync::oneshot::channel();
    app.state_mut()
        .overlays
        .push(Box::new(ExtensionOverlay::new(
            Box::new(Recorder(Arc::clone(&seen))),
            done,
        )));
    app.draw().unwrap();

    // Type something first, so "the editor was not cleared" is an observable claim rather than a
    // vacuous one: `app.message.clear` is what ctrl+c does with no overlay open.
    app.state_mut().editor.set_text("kept");
    let before = app.state().editor.text();

    app.handle_input(&InputEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));

    let routed = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(
        routed.len(),
        1,
        "exactly one key reached the overlay: {routed:?}"
    );
    assert_eq!(
        app.state().editor.text(),
        before,
        "the global clear must NOT have run: the overlay had the key"
    );
}

/// The row's other half: the PLATFORM-CONDITIONAL defaults, which the file read could not carry.
///
/// `tui.editor.undo` is upstream's three-way branch — `win32 ? "ctrl+z" : windowsKeybindings ?
/// "alt+z" : "ctrl+-"` (`core/keybindings.ts:77-80`, `keymap.rs:2031`). `cyrup-llama`'s own
/// fallback table is FLAT: `LlamaKey::EditorUndo` is `&["ctrl+-"]` on every platform
/// (`cyrup-llama/src/ui.rs:518`), so on Windows and under WSL `/llama` bound a chord upstream
/// moves off. On linux the two agree, which is why this asserts the table for the other two
/// platforms rather than the host's own.
#[test]
fn the_published_editor_table_carries_the_platform_arm_the_llama_fallback_flattens() {
    use crate::keymap::{EditorKeymap, KeybindingPlatform};

    let undo = |platform| {
        EditorKeymap::for_platform(platform)
            .bindings_json()
            .get("tui.editor.undo")
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    };

    let win32 = KeybindingPlatform {
        win32: true,
        darwin: false,
        windows_keybindings: true,
    };
    let wsl = KeybindingPlatform {
        win32: false,
        darwin: false,
        windows_keybindings: true,
    };
    let linux = KeybindingPlatform {
        win32: false,
        darwin: false,
        windows_keybindings: false,
    };

    assert_eq!(undo(win32), serde_json::json!(["ctrl+z"]), "native Windows");
    assert_eq!(undo(wsl), serde_json::json!(["alt+z"]), "WSL");
    assert_eq!(
        undo(linux),
        serde_json::json!(["ctrl+-"]),
        "elsewhere — the one platform where llama's flat fallback happened to be right"
    );
}
