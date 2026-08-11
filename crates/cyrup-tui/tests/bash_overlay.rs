//! `!`/`!!` bash-execution block + `/hotkeys` wiring, headless against a `TestBackend`
//! (bash-execution; `handleHotkeysCommand`, interactive-mode.ts:6090-6205).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{App, AppAction, BashStatus, Entry, InputEvent, UiTheme};
use ratatui::backend::TestBackend;

fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn ctrl(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::CONTROL))
}

fn submit(app: &mut App<TestBackend>, line: &str) -> AppAction {
    app.editor_mut().set_text(line);
    app.handle_input(&key(KeyCode::Enter))
}

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap()
}

fn buf_text(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

#[test]
fn bang_command_opens_a_live_bash_block_and_requests_a_run() {
    let mut app = new_app();
    let action = submit(&mut app, "!echo hi");
    assert_eq!(
        action,
        AppAction::RunBash { command: "echo hi".to_string(), excluded: false }
    );
    assert!(app.state().transcript.has_bash(), "a live bash block is open");
    assert!(app.state().transcript.bash_running(), "it starts running");
    let b = app.state().transcript.bash().unwrap();
    assert_eq!(b.command(), "echo hi");
    assert!(!b.excluded());
}

#[test]
fn double_bang_marks_excluded_from_context() {
    let mut app = new_app();
    let action = submit(&mut app, "!!secret-cmd");
    assert_eq!(
        action,
        AppAction::RunBash { command: "secret-cmd".to_string(), excluded: true }
    );
    assert!(app.state().transcript.bash().unwrap().excluded());
}

#[test]
fn bash_block_streams_output_and_renders_in_the_viewport() {
    let mut app = new_app();
    submit(&mut app, "!echo hi");
    // The run loop normally pumps these; drive the transcript directly here.
    app.transcript_mut().bash_append("hello\nworld\n");
    app.transcript_mut().bash_complete_simple(Some(0), false);
    app.draw().unwrap();
    let screen = buf_text(&app);
    assert!(screen.contains("$ echo hi"), "command header rendered:\n{screen}");
    assert!(screen.contains("hello"), "stdout rendered:\n{screen}");
    assert!(screen.contains("world"), "stdout rendered:\n{screen}");
    assert_eq!(app.state().transcript.bash().unwrap().status(), BashStatus::Complete);
}

#[test]
fn ctrl_o_toggles_bash_expansion() {
    let mut app = new_app();
    submit(&mut app, "!seq 100");
    for i in 1..=40 {
        app.transcript_mut().bash_append(&format!("row{i}\n"));
    }
    app.transcript_mut().bash_complete_simple(Some(0), false);
    assert!(!app.state().transcript.bash().unwrap().expanded());
    app.handle_input(&ctrl(KeyCode::Char('o')));
    assert!(app.state().transcript.bash().unwrap().expanded(), "Ctrl+O expands the bash block");
    app.handle_input(&ctrl(KeyCode::Char('o')));
    assert!(!app.state().transcript.bash().unwrap().expanded(), "Ctrl+O collapses again");
}

#[test]
fn interrupt_cancels_a_running_bash_block() {
    let mut app = new_app();
    submit(&mut app, "!sleep 9");
    app.transcript_mut().bash_append("partial");
    // Esc → Interrupt cancels + commits the block to scrollback.
    let action = app.handle_input(&key(KeyCode::Esc));
    assert_eq!(action, AppAction::Interrupt);
    assert!(!app.state().transcript.has_bash(), "the live block was committed away");
    // The committed block shows the cancelled status once flushed.
    app.draw().unwrap();
    assert!(app.scrollback_text().contains("(cancelled)"), "{}", app.scrollback_text());
}

/// S36 — `/hotkeys` appends a bordered block to the TRANSCRIPT and opens no overlay
/// (`handleHotkeysCommand`, interactive-mode.ts:6197-6203). The block is scrollback: it survives the
/// Esc that used to dismiss the popup, and Esc reaches the app instead of being swallowed.
#[test]
fn hotkeys_renders_into_the_transcript_and_opens_no_overlay() {
    let mut app = new_app();
    submit(&mut app, "/hotkeys");
    assert!(!app.overlay_open(), "/hotkeys must not open a floating overlay");
    assert!(
        app.state().transcript.pending().iter().any(
            |e| matches!(e, Entry::Block { title, .. } if title == "Keyboard Shortcuts")
        ),
        "no `Keyboard Shortcuts` block was appended to the transcript"
    );
    app.draw().unwrap();
    let screen = app.scrollback_text();
    assert!(screen.contains("Keyboard Shortcuts"), "block title:\n{screen}");
    assert!(screen.contains("Send message"), "block lists the submit binding:\n{screen}");
    // A transcript block, not a modal. The discriminator is the envelope, not the presence of box
    // glyphs — the GFM tables in the body legitimately draw their own `┌┬┐`. `DynamicBorder` is a
    // bare full-width `─` rule flush at column 0; a floating overlay was a centered, INSET box.
    let rule = "─".repeat(80);
    assert!(
        screen.lines().filter(|r| r.trim_end() == rule).count() >= 2,
        "two full-width DynamicBorder rules:\n{screen}"
    );

    // Esc no longer has an overlay to dismiss, and the help stays in scrollback.
    app.handle_input(&key(KeyCode::Esc));
    app.draw().unwrap();
    assert!(
        app.scrollback_text().contains("Send message"),
        "the help must persist in scrollback across Esc"
    );
}

/// The table cells are `keyDisplayText(id)` = `formatKeys(getKeys(id), { capitalize: true })`
/// (`keybinding-hints.ts:29-39`) — **every** bound key, `/`-joined, each chord part title-cased.
/// `tui.input.newLine`'s stock binding is two keys (`shift+enter`, `ctrl+j`), so a first-key-only
/// label would print `Shift+enter` and drop `Ctrl+j` entirely.
#[test]
fn hotkeys_key_cells_are_capitalized_and_list_every_bound_key() {
    let mut app = new_app();
    submit(&mut app, "/hotkeys");
    let body = app
        .state()
        .transcript
        .pending()
        .iter()
        .find_map(|e| match e {
            Entry::Block { title, markdown } if title == "Keyboard Shortcuts" => {
                Some(markdown.clone())
            }
            _ => None,
        })
        .expect("hotkeys block");
    // The three GFM tables, verbatim from interactive-mode.ts:6134-6182.
    assert!(body.starts_with("**Navigation**\n| Key | Action |\n|-----|--------|\n"), "{body}");
    assert!(body.contains("\n**Editing**\n| Key | Action |\n"), "{body}");
    assert!(body.contains("\n**Other**\n| Key | Action |\n"), "{body}");
    // Capitalized, and BOTH newLine keys present.
    assert!(body.contains("| `Shift+Enter/Ctrl+J` | New line |"), "newLine cell:\n{body}");
    assert!(body.contains("| `Enter` | Send message |"), "submit cell:\n{body}");
    // Rows cyrup previously omitted outright.
    assert!(body.contains("| Exit (when editor is empty) |"), "exit row:\n{body}");
    assert!(body.contains("| Paste the most-recently-deleted text |"), "yank row:\n{body}");
    assert!(body.contains("| `!!` | Run bash command (excluded from context) |"), "{body}");
    // No em-dash placeholder: upstream renders an unbound id as an EMPTY cell, never a glyph.
    assert!(!body.contains('—'), "no fabricated key placeholder:\n{body}");
}

#[test]
fn page_up_scrolls_the_active_region_and_page_down_returns_to_tail() {
    // A tall streaming partial exceeds the small viewport; PageUp reveals earlier lines, PageDown
    // pins back to the live tail (spec/tui/07 page-scroll over the active region).
    let mut app = App::new(TestBackend::new(40, 8), UiTheme::dark()).unwrap();
    // Paragraph breaks (blank line between) keep each `rowN` on its own rendered line (a single
    // newline is a markdown soft break → collapsed), so the active region is genuinely tall.
    let body: String = (1..=40).map(|i| format!("row{i}")).collect::<Vec<_>>().join("\n\n");
    app.transcript_mut().push_assistant_delta(&body);
    app.draw().unwrap();
    let tail = buf_text(&app);
    assert!(tail.contains("row40"), "tail anchored to newest:\n{tail}");

    app.handle_input(&key(KeyCode::PageUp));
    app.handle_input(&key(KeyCode::PageUp));
    app.draw().unwrap();
    assert!(app.state().transcript.scroll_offset() > 0, "paged up off the tail");

    app.handle_input(&key(KeyCode::PageDown));
    app.handle_input(&key(KeyCode::PageDown));
    app.draw().unwrap();
    assert_eq!(app.state().transcript.scroll_offset(), 0, "PageDown returns to the tail");
    assert!(buf_text(&app).contains("row40"), "tail visible again");
}

#[test]
fn ctrl_g_requests_the_external_editor() {
    // `app.editor.external` (Ctrl+G) surfaces to the run loop, which launches $VISUAL/$EDITOR.
    let mut app = new_app();
    app.editor_mut().set_text("draft text");
    assert_eq!(app.handle_input(&ctrl(KeyCode::Char('g'))), AppAction::OpenExternalEditor);
}

/// The counterpart of the block above: because `/hotkeys` is scrollback and not a modal, arrow keys
/// after it are NOT captured — nothing in cyrup pushes onto the overlay z-stack any more (upstream's
/// only `showOverlay` consumer is the unported extension custom-UI path, interactive-mode.ts:2719).
///
/// This replaced an `overlay_captures_navigation_keys` that asserted the OPPOSITE, and inverting an
/// assertion is not the same as pinning the new behaviour: "no overlay is open" is also true of an
/// app that dropped the arrow key on the floor. So the substantive half is asserted here — the
/// press reaches the EDITOR, which is where upstream's `tui.editor.cursorUp/cursorDown` (the
/// `/hotkeys` table's own "Move cursor / browse history" row, `interactive-mode.ts:6139`) send it
/// when nothing is layered above.
#[test]
fn hotkeys_does_not_capture_navigation_keys() {
    let mut app = new_app();
    submit(&mut app, "/hotkeys");
    assert!(app.state().editor.is_empty(), "submitting cleared the editor");

    // Consumed as an ordinary redraw-worthy edit, not by an overlay.
    assert_eq!(app.handle_input(&key(KeyCode::Down)), AppAction::Redraw);
    assert!(!app.overlay_open(), "no overlay may capture navigation after /hotkeys");

    // …and it genuinely reached the editor: Up now walks the submission history back to `/hotkeys`.
    // An overlay (or a swallowed key) leaves the buffer empty.
    assert_eq!(app.handle_input(&key(KeyCode::Up)), AppAction::Redraw);
    assert_eq!(
        app.state().editor.text(),
        "/hotkeys",
        "arrow keys must reach the editor's history browse, not a modal"
    );

    // Esc likewise reaches the app as an interrupt instead of being spent dismissing a popup, and
    // the help survives it because it is scrollback.
    app.editor_mut().set_text("");
    assert_eq!(app.handle_input(&key(KeyCode::Esc)), AppAction::Interrupt);
    assert!(
        app.state().transcript.pending().iter().any(
            |e| matches!(e, Entry::Block { title, .. } if title == "Keyboard Shortcuts")
        ),
        "Esc must not remove the block"
    );
}

/// S36, the GLOBAL half of the key-display resolution. The editor half is pinned by
/// `hotkeys_key_cells_are_capitalized_and_list_every_bound_key` above; `getAppKeyDisplay`
/// (`interactive-mode.ts:6081-6083`) is a separate closure over a separate map, and it was equally
/// capable of being spelled as a literal.
///
/// Both closures are `keyDisplayText(action)` = `formatKeys(getKeys(action), { capitalize: true })`
/// (`keybinding-hints.ts:29-39`), i.e. a lookup in the LIVE keybindings. The discriminator against a
/// hard-coded string is a rebind: `keybindings.json` moves `app.tools.expand` and
/// `app.editor.external`, and the table has to move with it.
#[test]
fn hotkeys_global_key_cells_resolve_from_the_live_keymap() {
    fn hotkeys_body(app: &App<TestBackend>) -> String {
        app.state()
            .transcript
            .pending()
            .iter()
            .find_map(|e| match e {
                Entry::Block { title, markdown } if title == "Keyboard Shortcuts" => {
                    Some(markdown.clone())
                }
                _ => None,
            })
            .expect("hotkeys block")
    }

    // Stock bindings first — the `**Other**` rows come from the GLOBAL map, capitalized.
    let mut app = new_app();
    submit(&mut app, "/hotkeys");
    let stock = hotkeys_body(&app);
    assert!(stock.contains("| `Ctrl+O` | Toggle tool output expansion |"), "{stock}");
    assert!(stock.contains("| `Ctrl+G` | Edit message in external editor |"), "{stock}");
    assert!(stock.contains("| `Ctrl+D` | Exit (when editor is empty) |"), "{stock}");

    // Rebind two of them and re-issue the command. A literal cell cannot follow.
    let mut rebound = new_app();
    rebound
        .load_keybindings_json(
            r#"{ "app.tools.expand": "ctrl+t", "app.editor.external": ["ctrl+x", "alt+e"] }"#,
        )
        .unwrap();
    submit(&mut rebound, "/hotkeys");
    let body = hotkeys_body(&rebound);
    assert!(
        body.contains("| `Ctrl+T` | Toggle tool output expansion |"),
        "the global cell did not follow the rebind:\n{body}"
    );
    assert!(
        !body.contains("| `Ctrl+O` | Toggle tool output expansion |"),
        "the OLD global key survived the rebind:\n{body}"
    );
    // `formatKeys` joins EVERY bound key with `/` (`keybinding-hints.ts:33-36`), same as the editor
    // half's `Shift+Enter/Ctrl+J`.
    assert!(
        body.contains("| `Ctrl+X/Alt+E` | Edit message in external editor |"),
        "a two-key global binding must list both:\n{body}"
    );
    // MIRROR — an untouched global row is unchanged, so the rebind moved one cell and not the table.
    assert!(body.contains("| `Ctrl+D` | Exit (when editor is empty) |"), "{body}");
}

/// S36 — the **Extensions** table (`interactive-mode.ts:6186-6197`).
///
/// ```ts
/// const shortcuts = extensionRunner.getShortcuts(this.keybindings.getEffectiveConfig());
/// if (shortcuts.size > 0) {
///     hotkeys += `\n**Extensions**\n| Key | Action |\n|-----|--------|\n`;
///     for (const [key, shortcut] of shortcuts) {
///         const description = shortcut.description ?? shortcut.extensionPath;
///         const keyDisplay = formatKeyText(key, { capitalize: true });
///         hotkeys += `| \`${keyDisplay}\` | ${description} |\n`;
///     }
/// }
/// ```
///
/// The claim that cyrup "has no extension-registered-shortcut registry to read" was false: it is
/// `AppState::extension_shortcuts`, installed by `App::set_extension_shortcuts` from
/// `ExtensionHost::shortcut_keys()` and already consulted on every keypress (`app.rs:1501`).
#[test]
fn hotkeys_lists_extension_registered_shortcuts() {
    fn hotkeys_body(app: &App<TestBackend>) -> String {
        app.state()
            .transcript
            .pending()
            .iter()
            .find_map(|e| match e {
                Entry::Block { title, markdown } if title == "Keyboard Shortcuts" => {
                    Some(markdown.clone())
                }
                _ => None,
            })
            .expect("hotkeys block")
    }

    // `if (shortcuts.size > 0)` — with nothing registered there is NO section, not an empty table.
    let mut bare = new_app();
    submit(&mut bare, "/hotkeys");
    let none = hotkeys_body(&bare);
    assert!(!none.contains("**Extensions**"), "an empty registry must emit no section:\n{none}");

    let mut app = new_app();
    app.set_extension_shortcuts([
        ("ctrl+j".to_string(), "Jump to definition".to_string()),
        ("alt+shift+k".to_string(), "Kill the ring".to_string()),
    ]);
    submit(&mut app, "/hotkeys");
    let body = hotkeys_body(&app);
    assert!(
        body.contains("\n**Extensions**\n| Key | Action |\n|-----|--------|\n"),
        "the section header/table head is verbatim upstream's:\n{body}"
    );
    // `formatKeyText(key, { capitalize: true })` — every chord part title-cased.
    assert!(body.contains("| `Ctrl+J` | Jump to definition |"), "{body}");
    assert!(body.contains("| `Alt+Shift+K` | Kill the ring |"), "{body}");
    // The section is LAST — it is appended after the `**Other**` table (`:6188`).
    let other = body.find("**Other**").expect("Other section");
    let ext = body.find("**Extensions**").expect("Extensions section");
    assert!(ext > other, "the Extensions table must trail the built-in ones:\n{body}");
    // And it is a real read of the routing registry, not a parallel list: the same ids still
    // dispatch.
    assert_eq!(
        app.handle_input(&ctrl(KeyCode::Char('j'))),
        AppAction::ExtensionShortcut("ctrl+j".to_string())
    );

    // A registry entry with no description at all still gets a row (upstream always has
    // `extensionPath` to fall back on; cyrup's host surfaces neither field yet, so the key-id
    // stands in rather than a fabricated label).
    let mut bare_desc = new_app();
    bare_desc.set_extension_shortcuts(["ctrl+j".to_string()]);
    submit(&mut bare_desc, "/hotkeys");
    let b = hotkeys_body(&bare_desc);
    assert!(b.contains("**Extensions**"), "{b}");
    assert!(b.contains("| `Ctrl+J` | ctrl+j |"), "{b}");
}
