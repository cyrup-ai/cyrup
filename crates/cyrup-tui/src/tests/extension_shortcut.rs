//! Extension keyboard-shortcut dispatch routing (feature #10; R-08-017; Pi `registerShortcut`).
//!
//! Proves the *routed keypress* half the audit called for: a key press that matches an
//! extension-registered shortcut resolves — through the real `App::handle_input` routing chain — to
//! an [`AppAction::ExtensionShortcut`] carrying the key-id, which the run loop hands to
//! `ExtensionHost::run_shortcut` → `LiveExtension::execute_shortcut`. The guest-execution tail is
//! wasm-toolchain-gated (ledger 09 #13); the dispatch decision is closed and asserted here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{App, AppAction, InputEvent, UiTheme};
use ratatui::backend::TestBackend;

fn key(code: KeyCode, mods: KeyModifiers) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, mods))
}

#[test]
fn registered_shortcut_keypress_dispatches_to_ext_host() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    // The host's `shortcut_specs()` returns `(id, description?)` pairs; install them the way the
    // binary does (EXT-040 — it used to install `shortcut_keys()`'s bare ids).
    app.set_extension_shortcuts([
        ("ctrl+j".to_string(), Some("Run the thing".to_string())),
        ("alt+k".to_string(), None),
    ]);

    // A matching press routes to the owning extension via its key-id (the run loop then calls
    // `ExtensionHost::run_shortcut(id)`), NOT into the editor as text.
    assert_eq!(
        app.handle_input(&key(KeyCode::Char('j'), KeyModifiers::CONTROL)),
        AppAction::ExtensionShortcut("ctrl+j".to_string())
    );
    assert_eq!(
        app.handle_input(&key(KeyCode::Char('k'), KeyModifiers::ALT)),
        AppAction::ExtensionShortcut("alt+k".to_string())
    );
    // The editor never saw the shortcut keys (buffer stays empty).
    assert!(app.state().editor.is_empty());
}

/// EXT-039, precedence — an extension shortcut on a NON-reserved built-in key must WIN.
///
/// pi resolves the extension tier first and unconditionally: `CustomEditor.handleInput` opens with
/// `// Check extension-registered shortcuts first` / `if (this.onExtensionShortcut?.(data)) return;`
/// (`modes/interactive/components/custom-editor.ts:31-34` @v0.84.4), *before* `app.clipboard.
/// pasteImage`, `app.interrupt`, `app.exit` and the rest of the action table. Which keys may reach
/// that map is decided earlier, at the gate — `getShortcuts` warns but lets the extension take a
/// non-reserved built-in (`extensions/runner.ts:568-574`).
///
/// `alt+up` is cyrup's `app.message.dequeue` (`core/keybindings.ts:102-105`) and is NOT in
/// `RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS` (`runner.ts:72-90`), so it is exactly the case
/// upstream hands to the extension.
///
/// RED before this pass: the global keymap was consulted first (`app/input.rs`), so this returned
/// `AppAction::Dequeue` and every non-reserved extension binding was advertised and dead. The test
/// this replaced asserted that dead behaviour as the contract; pi's dispatch site refutes it.
#[test]
fn a_non_reserved_builtin_key_goes_to_the_extension() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    assert_eq!(
        app.handle_input(&key(KeyCode::Up, KeyModifiers::ALT)),
        AppAction::Dequeue,
        "precondition: alt+up is a live built-in binding"
    );
    app.set_extension_shortcuts(["alt+up".to_string()]);
    assert_eq!(
        app.handle_input(&key(KeyCode::Up, KeyModifiers::ALT)),
        AppAction::ExtensionShortcut("alt+up".to_string())
    );
}

/// EXT-039, the gate — a RESERVED built-in key is refused at install time, with a warning, and the
/// built-in keeps firing.
///
/// This is the half the precedence inversion above rests on: pi skips such a shortcut outright
/// (`Extension shortcut '<key>' from <path> conflicts with built-in shortcut. Skipping.`,
/// `extensions/runner.ts:560-566` @v0.84.4) so it never enters the map the editor consults, and
/// `getShortcutDiagnostics()` (`:589-591`) carries the warning to the `[Extension issues]` panel.
///
/// RED before this pass: the production sites installed `ExtensionHost::shortcut_specs()` — the raw
/// per-extension table — because nothing called `resolve_shortcuts`. The first assertion below
/// re-runs that old line and shows it still hands over `ctrl+c`; only the gated install drops it.
#[test]
fn a_reserved_builtin_key_is_refused_at_install_and_diagnosed() {
    let host = cyrup_ext::ExtensionHost::new(cyrup_ext::HostConfig {
        mode: cyrup_ext::ExtMode::Tui,
        has_ui: true,
        cwd: std::env::temp_dir(),
    });
    // `ctrl+c` is `app.clear`, a RESERVED id (`runner.ts:72`); `alt+up` is `app.message.dequeue`,
    // which is not.
    host.registry()
        .register_shortcut("ext-a".into(), "ctrl+c", Some("steal clear".to_string()))
        .unwrap();
    host.registry()
        .register_shortcut("ext-a".into(), "alt+up", Some("dequeue-ish".to_string()))
        .unwrap();

    // The pre-EXT-039 production line, kept as the contrast: ungated, so the reserved key lands.
    let mut ungated = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    ungated.set_extension_shortcuts(host.shortcut_specs());
    assert_eq!(
        ungated.handle_input(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        AppAction::ExtensionShortcut("ctrl+c".to_string()),
        "the ungated seam is what EXT-039 is about: it hands a reserved key to the extension"
    );

    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.install_extension_shortcuts(&host);
    // Refused: Ctrl+C still runs `app.clear` — which empties the editor buffer and redraws — and
    // `/hotkeys` cannot list what was never installed.
    app.handle_input(&InputEvent::Paste("half a message".to_string()));
    assert!(!app.state().editor.is_empty());
    assert_eq!(
        app.handle_input(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        AppAction::Redraw
    );
    assert!(
        app.state().editor.is_empty(),
        "the built-in `app.clear` must still own its reserved key"
    );
    assert!(
        !app.hotkeys_markdown_for_test().contains("steal clear"),
        "a refused shortcut must not be advertised"
    );
    // The non-reserved sibling survives the same pass and wins its key.
    assert_eq!(
        app.handle_input(&key(KeyCode::Up, KeyModifiers::ALT)),
        AppAction::ExtensionShortcut("alt+up".to_string())
    );

    let diags = host.shortcut_diagnostics();
    assert!(
        diags.iter().any(|d| d.message.contains(
            "Extension shortcut 'ctrl+c' from ext-a conflicts with built-in shortcut. Skipping."
        )),
        "pi's refusal text, verbatim: {diags:?}"
    );
    // The non-reserved override still warns and still hands the key over. Which built-in it NAMES
    // is upstream's own last-write-wins inversion (`builtinKeybindings[normalizedKey] = …`,
    // `runner.ts:107-110`): `alt+up` is both `app.message.dequeue` and, inside the scoped-models
    // selector, `app.models.reorderUp`, and neither is reserved — so the name is not asserted.
    assert!(
        diags.iter().any(|d| {
            d.message
                .starts_with("Extension shortcut conflict: 'alt+up' is built-in shortcut for")
                && d.message.ends_with("Using ext-a.")
        }),
        "the non-reserved override still warns: {diags:?}"
    );
}

/// EXT-039 — `App::effective_keybindings` is cyrup's `KeybindingsManager.getEffectiveConfig()`, and
/// the gate can only refuse a reserved key that appears in it. Every namespace the reserved list
/// spans (`extensions/runner.ts:72-90` @v0.84.4) must therefore be represented, and a user rebind
/// must move the entry — otherwise a rebound Enter would stop being protected.
#[test]
fn effective_keybindings_spans_every_reserved_namespace_and_follows_a_rebind() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    let keys_for = |cfg: &[(String, Vec<String>)], id: &str| {
        cfg.iter()
            .find(|(k, _)| k == id)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("{id} missing from the effective config: {cfg:?}"))
    };
    let cfg = app.effective_keybindings();
    assert_eq!(keys_for(&cfg, "app.exit"), vec!["ctrl+d".to_string()]);
    assert_eq!(keys_for(&cfg, "app.clear"), vec!["ctrl+c".to_string()]);
    assert_eq!(
        keys_for(&cfg, "tui.input.submit"),
        vec!["enter".to_string()]
    );
    assert_eq!(
        keys_for(&cfg, "tui.editor.deleteToLineEnd"),
        vec!["ctrl+k".to_string()]
    );
    assert!(
        keys_for(&cfg, "tui.select.cancel").contains(&"escape".to_string()),
        "{cfg:?}"
    );

    app.load_keybindings_json(r#"{ "app.clear": "ctrl+q" }"#)
        .unwrap();
    assert_eq!(
        keys_for(&app.effective_keybindings(), "app.clear"),
        vec!["ctrl+q".to_string()],
        "the gate must protect the key the user actually bound"
    );
}

#[test]
fn unregistered_key_falls_through_to_the_editor() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.set_extension_shortcuts(["ctrl+j".to_string()]);
    // A plain letter with no registered shortcut is typed into the editor, never captured.
    let out = app.handle_input(&key(KeyCode::Char('h'), KeyModifiers::NONE));
    assert_eq!(out, AppAction::Redraw);
    assert!(!app.state().editor.is_empty());
}

#[test]
fn unparseable_shortcut_ids_are_dropped_not_panicked() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    // A malformed key-id is silently dropped; a valid sibling still routes.
    app.set_extension_shortcuts(["not a key+++".to_string(), "ctrl+j".to_string()]);
    assert_eq!(
        app.handle_input(&key(KeyCode::Char('j'), KeyModifiers::CONTROL)),
        AppAction::ExtensionShortcut("ctrl+j".to_string())
    );
}

/// EXT-040 — an extension's registered DESCRIPTION must reach the `/hotkeys` Extensions table.
///
/// pi stores `ExtensionShortcut { shortcut, description?, handler, extensionPath }`
/// (`extensions/types.ts:1250`, stored at `:1524-1529` @v0.83.0) and renders each row as
/// ``| `${formatKeyText(key, {capitalize:true})}` | ${shortcut.description ?? shortcut.extensionPath} |``
/// (`interactive-mode.ts:6193-6197`).
///
/// RED before this pass: the binary and the session-swap arm both installed
/// `ExtensionHost::shortcut_keys()` — a bare `Vec<String>` — so `description` was always `None`
/// and every Action cell fell through to repeating its own Key cell. `shortcut_specs()` and the
/// `From<(String, Option<String>)>` impl both already existed with no caller.
#[test]
fn hotkeys_renders_the_registered_shortcut_description() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.set_extension_shortcuts([
        ("ctrl+j".to_string(), Some("Run the thing".to_string())),
        ("alt+k".to_string(), None),
    ]);

    let body = app.hotkeys_markdown_for_test();
    assert!(
        body.contains("Run the thing"),
        "the registered description must be the Action cell: {body}"
    );
    // `description ?? extensionPath` — with no description the id is the fallback, which is what
    // cyrup has in place of pi's `extensionPath`.
    assert!(
        body.contains("alt+k"),
        "an undescribed shortcut still lists, labelled by its id: {body}"
    );
}
