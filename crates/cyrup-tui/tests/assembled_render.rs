//! **Assembled-app** render tests — the FIX-FIRST gate for the L6 void pass
//! (`spec/gap-analysis/12-cyrup-tui-audit-summary.md`). Per-widget tests pass while the launched app
//! renders a void; these render the WHOLE `App` through a `TestBackend` at 100x30 (+ other sizes) in
//! representative states and assert the buffer is **usable**, not a void:
//!
//! - the inline live region is **content-sized** and pinned at the bottom, not the whole screen
//!   (audit #1: `app.rs` viewport + `Min(1)→Min(0)`);
//! - the footer shows the seeded model + data, never a permanent `no-model` (audit #2/#5);
//! - the editor body row shows the accent prompt glyph `›` + a reverse-video soft cursor every idle
//!   frame (audit #3);
//! - routing holds — Ctrl+D does not quit a non-empty buffer, Esc dismisses an open popup instead of
//!   aborting (audit #4);
//! - the tool-execution surface is the spec block with a state bg tint (audit #6/#7).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{App, AppAction, InputEvent, UiTheme};
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;

fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}
fn ctrl(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::CONTROL))
}

/// The whole rendered buffer as text (every row, including the scrollback band above the viewport).
fn buf_text(app: &App<TestBackend>) -> String {
    rows_text(app, 0, app.terminal().backend().buffer().area.height)
}

/// Only the **live region** — the bottom `viewport_height` rows the app repaints each frame.
fn live_text(app: &App<TestBackend>) -> String {
    let h = app.terminal().backend().buffer().area.height;
    let vh = app.viewport_height().min(h);
    rows_text(app, h - vh, h)
}

fn rows_text(app: &App<TestBackend>, y0: u16, y1: u16) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in y0..y1 {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// True if any cell in the bottom `viewport_height` rows carries the reverse-video modifier (the soft
/// cursor, audit #3) — the hardware cursor is invisible in a headless buffer, so this is how we prove
/// the caret is actually painted.
fn live_has_reversed(app: &App<TestBackend>) -> bool {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let vh = app.viewport_height().min(area.height);
    let start = area.height - vh;
    (start..area.height).any(|y| {
        (0..area.width)
            .filter_map(|x| buf.cell((x, y)))
            .any(|c| c.modifier.contains(Modifier::REVERSED))
    })
}

/// True if any cell anywhere carries the given background color (proves a bg role projected, #6/#7).
fn has_bg(app: &App<TestBackend>, bg: ratatui::style::Color) -> bool {
    app.terminal().backend().buffer().content().iter().any(|c| c.bg == bg)
}

/// Count of fully-blank rows (used to prove the live region did NOT balloon into a void).
fn blank_rows(app: &App<TestBackend>) -> usize {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    (0..area.height)
        .filter(|&y| (0..area.width).filter_map(|x| buf.cell((x, y))).all(|c| c.symbol() == " "))
        .count()
}

// --------------------------------------------------------------- state 1: no-model, empty ----

#[test]
fn assembled_no_model_empty_is_usable_not_a_void_at_100x30() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.draw().unwrap();

    // (#1) The live region is content-sized and small — NOT the ~26-blank-row void the audit found.
    // Empty turn: 1 startup-hint row + 3 editor rows + 2 footer rows = 6 rows, pinned at the bottom.
    assert_eq!(app.viewport_height(), 6, "live region not content-sized:\n{}", buf_text(&app));
    assert!(
        blank_rows(&app) >= 20,
        "the live region ballooned into a void (too few blank scrollback rows):\n{}",
        buf_text(&app)
    );

    // (#1) The chrome sits inline at the BOTTOM; the top of the screen is free scrollback space.
    assert!(
        rows_text(&app, 0, 18).trim().is_empty(),
        "top rows should be empty scrollback space, not live chrome:\n{}",
        buf_text(&app)
    );

    let live = live_text(&app);
    // (#3) The editor body row shows the accent prompt glyph + a reverse-video soft cursor.
    assert!(live.contains('›'), "editor prompt glyph `›` missing from live region:\n{live}");
    assert!(live_has_reversed(&app), "editor soft cursor (reverse cell) missing:\n{live}");
    // The editor's two `─` rules frame the body row.
    assert!(live.contains('─'), "editor rules missing from live region:\n{live}");
    // (#2) With nothing seeded the footer is Pi's literal `no-model` (never blank, never invented).
    assert!(live.contains("no-model"), "footer model cluster missing:\n{live}");
    // The startup hint affordance bar is present just above the editor.
    assert!(live.contains('·') || live.contains("commands"), "startup hints missing:\n{live}");
}

#[test]
fn assembled_no_model_empty_is_usable_at_other_sizes() {
    for (w, h) in [(60u16, 20u16), (120, 40), (80, 24)] {
        let mut app = App::new(TestBackend::new(w, h), UiTheme::dark()).unwrap();
        app.draw().unwrap();
        assert_eq!(app.viewport_height(), 6, "live region not content-sized at {w}x{h}");
        let live = live_text(&app);
        assert!(live.contains('›'), "prompt glyph missing at {w}x{h}:\n{live}");
        assert!(live.contains("no-model"), "footer missing at {w}x{h}:\n{live}");
        assert!(live_has_reversed(&app), "soft cursor missing at {w}x{h}");
    }
}

// ------------------------------------------------------ state 2: model + short transcript ----

#[test]
fn assembled_model_and_transcript_renders_footer_model_and_active_turn() {
    use cyrup_core::{Cost, Usage};
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    // Seed the footer exactly as the binary's `seed_footer` does (audit #2/#5): without this the
    // footer is stuck on `no-model` and the location line is blank all session.
    app.status_mut().set_model("anthropic/claude-opus-4-8");
    app.status_mut().set_provider(Some("anthropic".to_string()));
    app.status_mut().set_cwd("~/src/cyrup");
    app.status_mut().set_branch(Some("david/cyrup".to_string()));
    app.status_mut().set_reasoning(true);
    app.status_mut().set_thinking_level("high");
    app.status_mut().add_usage(&Usage {
        input: 12_300,
        output: 4_100,
        cache_read: 88_000,
        total_tokens: 104_400,
        cost: Cost { total: 0.214, ..Cost::default() },
        ..Usage::default()
    });
    app.status_mut().set_context(0.412, 200_000, true);

    // A committed exchange + an in-flight streaming turn.
    app.transcript_mut().push_user("refactor the auth module");
    app.transcript_mut().commit_assistant(Some("Done.".to_string()));
    app.transcript_mut().push_assistant_delta("I'll start by reading the implementation");
    app.draw().unwrap();

    let live = live_text(&app);
    // (#2/#5) Footer line 2 = the seeded model + thinking suffix; line 1 = the location.
    assert!(live.contains("claude-opus-4-8 • high"), "footer model+thinking missing:\n{live}");
    assert!(live.contains("~/src/cyrup (david/cyrup)"), "footer location line missing:\n{live}");
    assert!(live.contains("41.2%/200k (auto)"), "footer context segment missing:\n{live}");
    assert!(!live.contains("no-model"), "footer still shows no-model after seeding:\n{live}");
    // The active streaming turn renders inline in the live region.
    assert!(live.contains("I'll start by reading"), "active turn missing from live region:\n{live}");
    // Committed history is in native scrollback, not the live region (ADR-0001 / audit #1).
    assert!(app.scrollback_text().contains("you: refactor the auth module"), "user not flushed");
    assert!(!live.contains("refactor the auth module"), "committed user leaked into live region:\n{live}");
    // The editor is still present + usable beneath the active turn.
    assert!(live.contains('›'), "editor prompt missing with a transcript:\n{live}");
    assert!(live_has_reversed(&app), "editor soft cursor missing with a transcript");
}

#[test]
fn assembled_live_tool_block_shows_spec_block_with_state_bg_tint() {
    // (#6/#7) A live tool run renders the spec block tinted by state (`toolPendingBg`/`toolSuccessBg`),
    // not the dead-bg pre-spec one-liner.
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.status_mut().set_model("anthropic/claude-opus-4-8");
    app.transcript_mut().push_assistant_delta("running a tool");
    app.transcript_mut().push_tool_start("read", Some("src/auth.rs".to_string()));
    app.draw().unwrap();
    let live = live_text(&app);
    assert!(live.contains("⚙ read(src/auth.rs)"), "tool call header missing:\n{live}");
    // Dark `toolPendingBg` = #282832 must reach real cells (the bg is the affordance, audit #6).
    assert!(
        has_bg(&app, ratatui::style::Color::Rgb(0x28, 0x28, 0x32)),
        "tool-pending bg tint did not reach any cell:\n{live}"
    );
}

// ------------------------------------------------------------------- state 3: popups open ----

#[test]
fn assembled_completion_popup_open_dismisses_on_esc_not_abort() {
    // Typing `/` opens the slash completion popup inside the live region (not an overlay).
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.handle_input(&key(KeyCode::Char('/')));
    assert!(app.editor_mut().autocomplete_open(), "slash completion did not open");
    app.draw().unwrap();
    assert!(app.viewport_height() > 6, "popup did not grow the live region:\n{}", buf_text(&app));
    let live = live_text(&app);
    assert!(live.contains('/'), "typed slash missing from editor:\n{live}");

    // (#4) Esc dismisses the popup and never aborts the run (returns Redraw, not Interrupt).
    let action = app.handle_input(&key(KeyCode::Esc));
    assert_eq!(action, AppAction::Redraw, "Esc should dismiss the popup, not abort");
    assert!(!app.editor_mut().autocomplete_open(), "Esc did not close the completion popup");
}

#[test]
fn assembled_hotkeys_overlay_opens_and_dismisses_on_esc() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.editor_mut().set_text("/hotkeys");
    app.handle_input(&key(KeyCode::Enter));
    assert!(app.overlay_open(), "hotkeys overlay did not open");
    // (#1) The viewport expands to full height so the modal can float over the live region.
    app.draw().unwrap();
    assert_eq!(app.viewport_height(), 30, "overlay should expand the viewport to full height");
    let screen = buf_text(&app);
    assert!(screen.contains("Keyboard Shortcuts"), "overlay title missing:\n{screen}");
    assert!(screen.contains("Send message"), "overlay shortcut list missing:\n{screen}");

    // (#4) Esc dismisses the overlay (Redraw), never leaks to the editor / aborts.
    let action = app.handle_input(&key(KeyCode::Esc));
    assert_eq!(action, AppAction::Redraw, "Esc should close the overlay");
    assert!(!app.overlay_open(), "Esc did not close the overlay");
}

// -------------------------------------- state 4: long single-paragraph PROSE-WRAP (no void) ----

/// A single long paragraph with NO hard newlines (30 sentences). `markdown::render` emits ONE
/// un-wrapped `Line` for it, so the pre-fix `content_height`/`flush_committed` under-counted the
/// wrapped display rows: the live region was sized to a single row (newest text + the `▌` stream
/// caret clipped away), and the committed answer was truncated to 1 row in native scrollback. This
/// asserts the WRAP-AWARE height fix: the streaming turn grows to its true wrapped height + stays
/// tail-anchored (newest sentence + caret visible, spec/tui/01 §3 overflow; Pi `assistant-message.ts`
/// wraps its `Markdown` body), and the committed flush lands the full wrapped answer in scrollback.
fn long_single_paragraph() -> String {
    // No `\n` anywhere — one logical paragraph. The last sentence carries a unique single-word token
    // (`OMEGAEND`) that word-wrap can never split across a row boundary, so a substring search proves
    // the NEWEST text survived to the buffer.
    let mut para = String::new();
    for i in 1..=29 {
        para.push_str(&format!("Sentence {i} adds another independent clause to the paragraph body. "));
    }
    para.push_str("And the very final sentence closes with the sentinel token OMEGAEND.");
    para
}

#[test]
fn assembled_long_streaming_paragraph_shows_newest_text_and_caret_and_grows() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.status_mut().set_model("anthropic/claude-opus-4-8");
    // Stream the whole paragraph as one delta (no hard newlines): one logical `Line`, many wrapped
    // display rows.
    app.transcript_mut().push_assistant_delta(&long_single_paragraph());
    app.draw().unwrap();

    let live = live_text(&app);
    // (a) The live region grew PAST the 6-row empty-turn minimum to fit the wrapped paragraph (the
    // pre-fix bug sized it to ~1 content row because `content_height` counted logical lines).
    assert!(
        app.viewport_height() > 6,
        "live region did not grow for the wrapped paragraph (still {}):\n{}",
        app.viewport_height(),
        buf_text(&app)
    );
    // (a) The NEWEST text (last sentence's sentinel) is visible — the turn is tail-anchored, not
    // clipped to its first wrapped row.
    assert!(
        live.contains("OMEGAEND"),
        "newest text (last sentence) missing from live region — PROSE-WRAP truncation:\n{live}"
    );
    // (a) The `▌` stream caret trails the newest grapheme and is visible.
    assert!(live.contains('▌'), "stream caret `▌` missing from live region:\n{live}");
    // Sanity: the accent label + some earlier text render too (it is the whole paragraph, wrapped).
    assert!(live.contains("assistant:"), "assistant label missing:\n{live}");
}

#[test]
fn assembled_committed_long_paragraph_flushes_full_wrapped_text_to_scrollback() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.status_mut().set_model("anthropic/claude-opus-4-8");
    app.transcript_mut().push_assistant_delta(&long_single_paragraph());
    // Commit the streaming turn → it drains into native scrollback via `insert_before` on the next
    // draw (`flush_committed`).
    app.transcript_mut().commit_assistant(None);
    app.draw().unwrap();

    // (b) The full wrapped answer reached native scrollback: the sentinel (last sentence) is painted
    // into the TestBackend buffer ABOVE the live region. Pre-fix, `insert_before(height = 1, …)` with
    // no `.wrap()` truncated the answer to its first row and the sentinel was lost.
    let screen = buf_text(&app);
    assert!(
        screen.contains("OMEGAEND"),
        "committed answer truncated: last sentence missing from flushed scrollback:\n{screen}"
    );
    // The committed turn is NOT in the live region anymore (streaming is done); it lives in scrollback.
    let live = live_text(&app);
    assert!(!live.contains("OMEGAEND"), "committed paragraph leaked into live region:\n{live}");
    // The in-memory accumulator carries the full text too (test-visible mirror of the flush payload).
    assert!(app.scrollback_text().contains("OMEGAEND"), "flush accumulator missing the full text");
}

#[test]
fn assembled_ctrl_d_does_not_quit_a_non_empty_buffer_but_exits_when_empty() {
    // (#4) Ctrl+D is forward-delete while text remains; it only exits on an empty buffer.
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.editor_mut().set_text("hello");
    // Cursor at end → put it at start so forward-delete actually removes a char.
    app.handle_input(&key(KeyCode::Home));
    let action = app.handle_input(&ctrl(KeyCode::Char('d')));
    assert_ne!(action, AppAction::Quit, "Ctrl+D quit a non-empty buffer (audit #4 regression)");
    assert_eq!(app.editor_mut().text(), "ello", "Ctrl+D did not forward-delete in the editor");

    // Drain the buffer; now Ctrl+D exits (Pi `app.exit` only fires on empty).
    app.editor_mut().set_text("");
    let action = app.handle_input(&ctrl(KeyCode::Char('d')));
    assert_eq!(action, AppAction::Quit, "Ctrl+D on an empty buffer must exit");
}

