//! **Assembled-app** render tests — the FIX-FIRST gate for the L6 void pass
//! (`spec/gap-analysis/12-cyrup-tui-audit-summary.md`). Per-widget tests pass while the launched app
//! renders a void; these render the WHOLE `App` through a `TestBackend` at 100x30 (+ other sizes) in
//! representative states and assert the buffer is **usable**, not a void:
//!
//! - the inline live region is **content-sized** and pinned at the bottom, not the whole screen
//!   (audit #1: `app.rs` viewport + `Min(1)→Min(0)`);
//! - the footer shows the seeded model + data, never a permanent `no-model` (audit #2/#5);
//! - the editor body row shows a reverse-video soft cursor every idle frame (audit #3) and carries
//!   NO prompt glyph — pi's `Editor.render` emits none (E1, `editor.ts:482-601`);
//! - routing holds — Ctrl+D does not quit a non-empty buffer, Esc dismisses an open popup instead of
//!   aborting (audit #4);
//! - the tool-execution surface is the spec block with a state bg tint (audit #6/#7).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::harness::*;
use crate::crossterm::event::KeyCode;
use crate::{
    App, AppAction, ConfigKind, ConfigRow, ConfigScope, ConfigSelector, SelectorKind, UiTheme,
};
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;

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
    app.terminal()
        .backend()
        .buffer()
        .content()
        .iter()
        .any(|c| c.bg == bg)
}

/// Count of fully-blank rows (used to prove the live region did NOT balloon into a void).
fn blank_rows(app: &App<TestBackend>) -> usize {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    (0..area.height)
        .filter(|&y| {
            (0..area.width)
                .filter_map(|x| buf.cell((x, y)))
                .all(|c| c.symbol() == " ")
        })
        .count()
}

// --------------------------------------------------------------- state 1: no-model, empty ----

#[test]
fn assembled_no_model_empty_is_usable_not_a_void_at_100x30() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.draw().unwrap();

    // (#1) The live region is content-sized and small — NOT the ~26-blank-row void the audit found.
    // Empty turn: 6 startup-hint rows + 3 editor rows + 2 footer rows = 11 rows, pinned at the
    // bottom. The hint block is 6 rows at this width because pi frames its startup `ExpandableText`
    // with a `Spacer(1)` on each side (v0.84.1 `interactive-mode.ts:960-962`) and the collapsed body
    // is FIVE parts — `${logo}\n${compactInstructions}\n${compactOnboarding}\n\n${onboarding}`
    // (`:952`) — of which cyrup draws the last four: 1 + (1 + 1 + 1 + 1) + 1.
    //
    // Longest row is `onboarding` at 91 columns; the block's content width is `width - paddingX * 2`
    // = 98 (`text.ts:64`), so nothing wraps here. See the sibling test for the widths where it does.
    assert_eq!(
        app.viewport_height(),
        11,
        "live region not content-sized:\n{}",
        buf_text(&app)
    );
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
    // (#3) The editor body row shows a reverse-video soft cursor — the ONLY caret pi's editor
    // draws (`editor.ts:558,563`). E1: and no prompt glyph. `Editor.render` (`editor.ts:482-601`)
    // pushes `${leftPadding}${displayText}${padding}${lineRightPadding}` (`:578`) and nothing else;
    // the chat editor is a bare `new CustomEditor(...)` (`interactive-mode.ts:563-566`) whose class
    // overrides `handleInput` only (`components/custom-editor.ts`, no `render`). The `›` upstream
    // does draw is the SELECTED-ROW cursor of the list selectors (`session-selector.ts:476`,
    // `tree-selector.ts:689`, `user-message-selector.ts:57`) — a different component entirely.
    assert!(
        live_has_reversed(&app),
        "editor soft cursor (reverse cell) missing:\n{live}"
    );
    assert!(
        !live.contains('\u{203a}'),
        "E1: pi's editor draws no prompt glyph:\n{live}"
    );
    // The editor's two `─` rules frame the body row.
    assert!(
        live.contains('─'),
        "editor rules missing from live region:\n{live}"
    );
    // (#2) With nothing seeded the footer is Pi's literal `no-model` (never blank, never invented).
    assert!(
        live.contains("no-model"),
        "footer model cluster missing:\n{live}"
    );
    // The startup hint affordance bar is present just above the editor.
    assert!(
        live.contains('·') || live.contains("commands"),
        "startup hints missing:\n{live}"
    );
}

#[test]
fn assembled_no_model_empty_is_usable_at_other_sizes() {
    // The expected height is `hint block + 3 editor + 2 footer`, and the hint block GROWS as the
    // terminal narrows because pi's `Text.render` wraps at `contentWidth = width - paddingX * 2`
    // (`tui/src/components/text.ts:64-67`) instead of clipping. The three text rows are 79
    // (`compactInstructions`), 60 (`compactOnboarding`) and 91 (`onboarding`) columns wide, plus
    // three blanks that never wrap:
    //
    //   w=120 → content 118: 1 + 1 + 1 rows + 3 blanks =  6 → 11
    //   w=80  → content  78: 2 + 1 + 2 rows + 3 blanks =  8 → 13   (79 > 78, 91 > 78)
    //   w=60  → content  58: 2 + 2 + 2 rows + 3 blanks =  9 → 14   (60 > 58 as well)
    for (w, h, want) in [(60u16, 20u16, 14u16), (120, 40, 11), (80, 24, 13)] {
        let mut app = App::new(TestBackend::new(w, h), UiTheme::dark()).unwrap();
        app.draw().unwrap();
        assert_eq!(
            app.viewport_height(),
            want,
            "live region not content-sized at {w}x{h}"
        );
        let live = live_text(&app);
        assert!(
            !live.contains('\u{203a}'),
            "E1: no editor prompt glyph at {w}x{h}:\n{live}"
        );
        assert!(
            live.contains("no-model"),
            "footer missing at {w}x{h}:\n{live}"
        );
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
        cost: Cost {
            total: 0.214,
            ..Cost::default()
        },
        ..Usage::default()
    });
    app.status_mut().set_context(0.412, 200_000, true);

    // A committed exchange + an in-flight streaming turn.
    app.transcript_mut().push_user("refactor the auth module");
    app.transcript_mut()
        .commit_assistant(Some("Done.".to_string()));
    app.transcript_mut()
        .push_assistant_delta("I'll start by reading the implementation");
    app.draw().unwrap();

    let live = live_text(&app);
    // (#2/#5) Footer line 2 = the seeded model + thinking suffix; line 1 = the location.
    assert!(
        live.contains("claude-opus-4-8 • high"),
        "footer model+thinking missing:\n{live}"
    );
    assert!(
        live.contains("~/src/cyrup (david/cyrup)"),
        "footer location line missing:\n{live}"
    );
    assert!(
        live.contains("41.2%/200k (auto)"),
        "footer context segment missing:\n{live}"
    );
    assert!(
        !live.contains("no-model"),
        "footer still shows no-model after seeding:\n{live}"
    );
    // The active streaming turn renders inline in the live region.
    assert!(
        live.contains("I'll start by reading"),
        "active turn missing from live region:\n{live}"
    );
    // Committed history is in native scrollback, not the live region (ADR-0001 / audit #1).
    // X1: no `you: ` label — `user-message.ts:38-58` renders the body only.
    assert!(
        app.scrollback_text().contains("refactor the auth module"),
        "user not flushed"
    );
    assert!(
        !app.scrollback_text().contains("you:"),
        "invented `you: ` label in scrollback"
    );
    assert!(
        !live.contains("refactor the auth module"),
        "committed user leaked into live region:\n{live}"
    );
    // The editor is still present + usable beneath the active turn: its caret is the reverse-video
    // cell, and (E1) it carries no prompt glyph.
    assert!(
        live_has_reversed(&app),
        "editor soft cursor missing with a transcript"
    );
    assert!(
        !live.contains('\u{203a}'),
        "E1: no editor prompt glyph with a transcript:\n{live}"
    );
}

#[test]
fn assembled_live_tool_block_shows_spec_block_with_state_bg_tint() {
    // (#6/#7) A live tool run renders the spec block tinted by state (`toolPendingBg`/`toolSuccessBg`),
    // not the dead-bg pre-spec one-liner.
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.status_mut().set_model("anthropic/claude-opus-4-8");
    app.transcript_mut().push_assistant_delta("running a tool");
    app.transcript_mut()
        .push_tool_start("read", serde_json::json!({ "path": "src/auth.rs" }));
    app.draw().unwrap();
    let live = live_text(&app);
    // Per-tool `renderCall`: `read <path>` (read.ts:74-77) — Pi has no gear/`read(...)` marker; the
    // running affordance is the pending background tint asserted below.
    assert!(
        live.contains("read src/auth.rs"),
        "tool call header missing:\n{live}"
    );
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
    assert!(
        app.editor_mut().autocomplete_open(),
        "slash completion did not open"
    );
    app.draw().unwrap();
    assert!(
        app.viewport_height() > 6,
        "popup did not grow the live region:\n{}",
        buf_text(&app)
    );
    let live = live_text(&app);
    assert!(
        live.contains('/'),
        "typed slash missing from editor:\n{live}"
    );

    // (#4) Esc dismisses the popup and never aborts the run (returns Redraw, not Interrupt).
    let action = app.handle_input(&key(KeyCode::Esc));
    assert_eq!(
        action,
        AppAction::Redraw,
        "Esc should dismiss the popup, not abort"
    );
    assert!(
        !app.editor_mut().autocomplete_open(),
        "Esc did not close the completion popup"
    );
}

/// S36 — assembled: `/hotkeys` lands in scrollback with the [`Entry::Block`] envelope pi builds
/// (interactive-mode.ts:6197-6203). The `─` rules run edge to edge while the title and the markdown
/// body are inset by ONE column (`Text(…, 1, 0)` / `Markdown(…, 1, 1)`), and the body carries a blank
/// row on each side of it (`paddingY = 1`, `markdown.ts:352-361`).
#[test]
fn assembled_hotkeys_block_lands_in_scrollback_with_pi_envelope() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.editor_mut().set_text("/hotkeys");
    app.handle_input(&key(KeyCode::Enter));
    assert!(!app.overlay_open(), "/hotkeys must not open an overlay");
    app.draw().unwrap();
    let text = app.scrollback_text();
    let rows: Vec<&str> = text.lines().collect();
    let rule = "─".repeat(100);
    let top = rows
        .iter()
        .position(|r| r.trim_end() == rule)
        .expect("opening DynamicBorder rule");
    // Spacer(1) above the opening rule.
    assert!(
        top >= 1 && rows[top - 1].trim().is_empty(),
        "Spacer(1) precedes the rule:\n{text}"
    );
    // Text(bold accent title, paddingX 1) — inset one column, NOT flush left.
    assert_eq!(rows[top + 1], " Keyboard Shortcuts", "title row:\n{text}");
    // Spacer(1), then Markdown's own paddingY blank, then the body.
    assert!(
        rows[top + 2].trim().is_empty(),
        "Spacer(1) after the title:\n{text}"
    );
    assert!(
        rows[top + 3].trim().is_empty(),
        "Markdown paddingY blank:\n{text}"
    );
    assert!(
        rows[top + 4].starts_with(' '),
        "body inset by paddingX 1:\n{text}"
    );
    // The closing rule is preceded by the trailing paddingY blank.
    let bottom = rows
        .iter()
        .rposition(|r| r.trim_end() == rule)
        .expect("closing DynamicBorder rule");
    assert!(bottom > top, "two distinct rules:\n{text}");
    assert!(
        rows[bottom - 1].trim().is_empty(),
        "trailing paddingY blank:\n{text}"
    );
    assert!(
        text.contains("Send message"),
        "shortcut list missing:\n{text}"
    );
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
        para.push_str(&format!(
            "Sentence {i} adds another independent clause to the paragraph body. "
        ));
    }
    para.push_str("And the very final sentence closes with the sentinel token OMEGAEND.");
    para
}

/// The name says "no caret" on purpose: this test asserts the ABSENCE of `▌` (X1), so a name
/// promising caret coverage would read as the opposite of what it checks.
#[test]
fn assembled_long_streaming_paragraph_shows_newest_text_and_grows_without_a_caret() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.status_mut().set_model("anthropic/claude-opus-4-8");
    // Stream the whole paragraph as one delta (no hard newlines): one logical `Line`, many wrapped
    // display rows.
    app.transcript_mut()
        .push_assistant_delta(&long_single_paragraph());
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
    // (a) X1: pi draws NO streaming caret — the only caret in the TUI is the editor's reverse-video
    // cell (`editor.ts:545-564`), and `git grep "▌" v0.84.1 -- packages/` finds one hit, the pupil of
    // an eye in `examples/extensions/custom-header.ts:22`.
    assert!(
        !live.contains('▌'),
        "invented stream caret `▌` in live region:\n{live}"
    );
    // X1: and no `assistant: ` label (`assistant-message.ts:104-114` is the Markdown body alone).
    assert!(
        !live.contains("assistant:"),
        "invented assistant label:\n{live}"
    );
    // Sanity: earlier text renders too (it is the whole paragraph, wrapped).
    assert!(
        live.contains("Sentence 1 adds"),
        "earlier text missing:\n{live}"
    );
}

#[test]
fn assembled_committed_long_paragraph_flushes_full_wrapped_text_to_scrollback() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.status_mut().set_model("anthropic/claude-opus-4-8");
    app.transcript_mut()
        .push_assistant_delta(&long_single_paragraph());
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
    assert!(
        !live.contains("OMEGAEND"),
        "committed paragraph leaked into live region:\n{live}"
    );
    // The in-memory accumulator carries the full text too (test-visible mirror of the flush payload).
    assert!(
        app.scrollback_text().contains("OMEGAEND"),
        "flush accumulator missing the full text"
    );
}

#[test]
fn assembled_ctrl_d_does_not_quit_a_non_empty_buffer_but_exits_when_empty() {
    // (#4) Ctrl+D is forward-delete while text remains; it only exits on an empty buffer.
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.editor_mut().set_text("hello");
    // Cursor at end → put it at start so forward-delete actually removes a char.
    app.handle_input(&key(KeyCode::Home));
    let action = app.handle_input(&ctrl(KeyCode::Char('d')));
    assert_ne!(
        action,
        AppAction::Quit,
        "Ctrl+D quit a non-empty buffer (audit #4 regression)"
    );
    assert_eq!(
        app.editor_mut().text(),
        "ello",
        "Ctrl+D did not forward-delete in the editor"
    );

    // Drain the buffer; now Ctrl+D exits (Pi `app.exit` only fires on empty).
    app.editor_mut().set_text("");
    let action = app.handle_input(&ctrl(KeyCode::Char('d')));
    assert_eq!(
        action,
        AppAction::Quit,
        "Ctrl+D on an empty buffer must exit"
    );
}

#[test]
fn assembled_backslash_enter_soft_newline_routes_as_edit_not_submit() {
    // (#5) A routed keypress: typing `foo\` then Enter must NOT submit — the trailing backslash is
    // deleted and a newline inserted (Pi editor.ts:796-802, spec/tui/03 §5.7). A plain Enter submits.
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    for c in "foo\\".chars() {
        app.handle_input(&key(KeyCode::Char(c)));
    }
    let action = app.handle_input(&key(KeyCode::Enter));
    assert_eq!(
        action,
        AppAction::Redraw,
        "backslash-Enter must edit (redraw), never submit"
    );
    assert_eq!(
        app.editor_mut().text(),
        "foo\n",
        "backslash not converted to a soft newline"
    );

    // A following plain Enter (no trailing backslash) now submits the buffer as a prompt (trimmed).
    let action = app.handle_input(&key(KeyCode::Enter));
    assert_eq!(
        action,
        AppAction::Submit("foo".to_string()),
        "plain Enter should submit"
    );
}

// --------------------------------------------------------- the per-frame terminal height --------

/// **U6.** `App::draw` publishes the SCREEN height to the open selector on every frame, and a
/// selector that windows its own body must resize with it.
///
/// Pi feeds `ui.terminal.rows` into `ConfigSelectorComponent` (`cli/config-selector.ts:47`), which
/// becomes `this.maxVisible = Math.max(5, (terminalHeight ?? 24) - chrome)`
/// (`config-selector.ts:264-266`) and slices the body at `:405-409`. `Selector::set_terminal_height`
/// is cyrup's port of that input, and until now the ONLY caller was the standalone
/// `startup_selector` loop — so a selector opened inside the running app kept the `?? 24` default
/// forever, no matter how tall the terminal was.
///
/// Assembled on purpose: the whole claim is about `App::draw` making the call, so a direct
/// `sel.set_terminal_height(60)` would test the thing that already worked.
#[test]
fn an_open_selector_is_told_the_terminals_height_on_every_frame() {
    // 40 resources ⇒ 40 body rows once the terminal allows them. A 24-row default allows only
    // `24 - 7 = 17`.
    let rows: Vec<ConfigRow> = (0..40)
        .map(|i| ConfigRow {
            scope: ConfigScope::User,
            kind: ConfigKind::Skills,
            display_name: format!("res-{i:02}"),
            pattern: format!("skills/res-{i:02}"),
            base_dir: "/home/me/.cyrup".to_string(),
            enabled: true,
        })
        .collect();

    let mut app = App::new(TestBackend::new(80, 60), UiTheme::dark()).unwrap();
    app.open_boxed_selector(SelectorKind::Settings, Box::new(ConfigSelector::new(rows)));
    app.draw().unwrap();

    let screen = buf_text(&app);
    let shown = (0..40)
        .filter(|i| screen.contains(&format!("res-{i:02}")))
        .count();
    assert_eq!(
        shown, 40,
        "U6: on a 60-row terminal the body window is `60 - 7 = 53` rows, so all 40 resources are          on screen; a hardcoded 24 shows 17 of them:\n{screen}"
    );
    assert!(
        screen.contains("res-39"),
        "the LAST resource, the one a 24-row window drops"
    );
}

/// MIRROR of U6. The window really is DERIVED from the terminal, in both directions: the same
/// selector on a 24-row terminal is windowed at `24 - 7 = 17` body rows, so the tail is off-screen.
/// Without the pairing, "shows all 40" would be satisfied by a selector that windows at nothing.
#[test]
fn the_same_selector_is_windowed_on_a_short_terminal() {
    let rows: Vec<ConfigRow> = (0..40)
        .map(|i| ConfigRow {
            scope: ConfigScope::User,
            kind: ConfigKind::Skills,
            display_name: format!("res-{i:02}"),
            pattern: format!("skills/res-{i:02}"),
            base_dir: "/home/me/.cyrup".to_string(),
            enabled: true,
        })
        .collect();

    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.open_boxed_selector(SelectorKind::Settings, Box::new(ConfigSelector::new(rows)));
    app.draw().unwrap();

    let screen = buf_text(&app);
    assert!(
        screen.contains("res-00"),
        "the head of the list is on screen:\n{screen}"
    );
    assert!(
        !screen.contains("res-39"),
        "a 24-row terminal cannot show 40 body rows:\n{screen}"
    );
}
