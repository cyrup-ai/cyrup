//! **Editor presentation fidelity** against pi v0.84.1 `packages/tui/src/components/editor.ts`
//! (TUI-FIDELITY E1-E4, E13-E15, E17, plus the wrap fixes W1/W2 and the untested-fix coverage
//! U1-U3).
//!
//! Every test here drives the assembled [`App`] with real keystrokes and reads the rendered
//! `TestBackend` grid, because the editor is interactive: a render-only assertion cannot tell a
//! caret that follows the cursor from one that happens to sit where the cursor started, and cannot
//! see a scroll window at all. (E17's pair is the exception, and deliberately so: its whole claim
//! is that the component caps ITSELF, which can only be shown by handing it a rect the app would
//! never hand it.)
//!
//! The upstream facts these pin, quoted at the site that establishes each:
//!
//! * `render(width)` emits `${leftPadding}${displayText}${padding}${lineRightPadding}` and NOTHING
//!   before it (`editor.ts:578`) — E1.
//! * one `layoutWidth` feeds both `this.lastWidth` and `layoutText()` (`:489-497`) — E2 + E15.
//! * `maxVisibleLines = Math.max(5, Math.floor(terminalRows * 0.3))` (`:499-501`), then
//!   `layoutLines.slice(scrollOffset, scrollOffset + maxVisibleLines)` (`:519`) — E3 + E17.
//! * `createScrollBorder` replaces the plain rule when rows are hidden (`:259-268`, called `:527`
//!   and `:584`), and `scrollOffset` chases the caret (`:507-516`) — E4 + U1.
//! * `layoutLine.hasCursor` — the reverse-video cell — is independent of `focused`, which gates only
//!   the zero-width `CURSOR_MARKER` (`:537`, `:545-564`) — E13.
//! * `wordWrapLine` accumulates `visibleWidth(grapheme)` per CLUSTER (`:139-143`), sets a wrap
//!   opportunity between adjacent CJK (`:191-198`) and force-breaks at the cluster's own start
//!   index (`:157-160`) — W1 + W2.
//! * the caret cell is `afterGraphemes[0].segment` (`:555-559`) and the hardware marker is spliced
//!   into the row string, so the terminal advances by real cell widths (`:546-550`) — U2 + U3.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{App, Component, InputEditor, InputEvent, UiTheme};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;

/// The ZWJ family emoji — ONE extended grapheme cluster, seven `char`s, two display columns.
const FAMILY: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";

fn ch(c: char) -> InputEvent {
    InputEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}
fn code(c: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(c, KeyModifiers::NONE))
}
fn shift_enter() -> InputEvent {
    InputEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT))
}

/// Type `text` one printable keystroke at a time, `Shift+Enter` for each `\n` (the editor's
/// `editor.newLine`; a bare `Enter` would submit).
fn type_text(app: &mut App<TestBackend>, text: &str) {
    for c in text.chars() {
        if c == '\n' {
            app.handle_input(&shift_enter());
        } else {
            app.handle_input(&ch(c));
        }
    }
}

/// Every row of the frame, trailing blanks trimmed.
fn rows(app: &App<TestBackend>) -> Vec<String> {
    let buf = app.terminal().backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            let mut s = String::new();
            for x in 0..buf.area.width {
                s.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            s.trim_end().to_string()
        })
        .collect()
}

/// Whether `row` is one of the editor's rules. A rule either repeats `─` across the full width
/// (`editor.ts:530`, `:587`) **or** is a `createScrollBorder` indicator, which still opens with
/// `─── ` (`:261`) — so "starts with `─`" covers both, where "is entirely `─`" would silently skip
/// the scrolled case and hand the caller the wrong row.
fn is_rule(row: &str) -> bool {
    row.starts_with('─')
}

/// The index of the editor's TOP rule. Every test here keeps the transcript empty, so the editor's
/// pair is the only pair of rules on screen.
fn editor_top_rule(app: &App<TestBackend>) -> usize {
    rows(app)
        .iter()
        .position(|r| is_rule(r))
        .unwrap_or_else(|| panic!("no editor rule in the frame:\n{}", rows(app).join("\n")))
}

/// The index of the editor's BOTTOM rule, i.e. the next rule below the top one.
fn editor_bottom_rule(app: &App<TestBackend>) -> usize {
    let r = rows(app);
    let top = editor_top_rule(app);
    r[top + 1..]
        .iter()
        .position(|l| is_rule(l))
        .map(|i| top + 1 + i)
        .unwrap_or_else(|| panic!("no bottom rule in the frame:\n{}", r.join("\n")))
}

/// The first row of the frame carrying a REVERSED cell — the editor's soft caret
/// (`editor.ts:558,563`) — as `(row, column)`.
fn caret_cell(app: &App<TestBackend>) -> Option<(u16, u16)> {
    let buf = app.terminal().backend().buffer();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if buf
                .cell((x, y))
                .unwrap()
                .modifier
                .contains(Modifier::REVERSED)
            {
                return Some((y, x));
            }
        }
    }
    None
}

// -------------------------------------------------------------------- E1 + E2 -------------------

/// **E1.** The editor's first text row starts with the user's own first character, at column
/// `editorPaddingX` — there is no glyph in front of it.
///
/// `Editor.render` (`editor.ts:482-601`) pushes exactly
/// `${leftPadding}${displayText}${padding}${lineRightPadding}` (`:578`) with
/// `leftPadding = " ".repeat(paddingX)` (`:522`), and the chat editor is a bare
/// `new CustomEditor(this.ui, getEditorTheme(), this.keybindings, {...})`
/// (`interactive-mode.ts:563-566`) whose subclass (`components/custom-editor.ts`, 90 lines)
/// overrides `handleInput` and defines no `render`. cyrup prefixed an accent `› ` to visual row 0.
///
/// The `›` upstream DOES draw is the selected-row cursor of the list selectors
/// (`session-selector.ts:476`, `tree-selector.ts:689`, `user-message-selector.ts:57`) — a different
/// component, still drawn, and asserted elsewhere (`tests/selection_fidelity.rs`).
#[test]
fn the_editor_draws_no_prompt_glyph_before_the_typed_text() {
    let mut app = App::new(TestBackend::new(60, 24), UiTheme::dark()).unwrap();
    type_text(&mut app, "hello");
    app.draw().unwrap();
    let r = rows(&app);
    let body = editor_top_rule(&app) + 1;
    assert_eq!(
        r[body],
        "hello",
        "the text row is the text alone: {:?}",
        &r[body - 1..=body]
    );
    assert!(
        !r.join("\n").contains('\u{203a}'),
        "no `›` anywhere in an editor-only frame:\n{}",
        r.join("\n")
    );
}

/// MIRROR of E1: the fix removed a glyph, not the padding it sat behind. `editorPaddingX = 3` still
/// insets the text three columns (`editor.ts:522` `leftPadding`), it just no longer inserts two
/// extra columns of its own on top.
#[test]
fn editor_padding_still_insets_the_text_after_the_glyph_is_gone() {
    let mut app = App::new(TestBackend::new(60, 24), UiTheme::dark()).unwrap();
    app.editor_mut().set_padding_x(3);
    type_text(&mut app, "hello");
    app.draw().unwrap();
    let r = rows(&app);
    let body = editor_top_rule(&app) + 1;
    assert_eq!(
        r[body], "   hello",
        "3 columns of `leftPadding`, then the text: {:?}",
        r[body]
    );
}

/// **E2.** Every visual row of a wrapped line starts at the SAME column.
///
/// pi wraps every row at one `layoutWidth` and prefixes the same `leftPadding` to each
/// (`editor.ts:497`, `:578`), so the left edge is flush. cyrup wrapped at `layout_width` but then
/// prepended a two-column glyph to row 0 only: row 0 ran `2 + view_width` columns wide inside a
/// `view_width` area (its last character clipped, and an end-of-line caret pushed off the right
/// edge) while rows 1..n began two columns to its left — a permanent ragged left edge.
///
/// Driven by keystrokes so the wrap is the editor's own, at its own live `view_width`.
#[test]
fn wrapped_rows_share_one_left_edge_and_the_first_row_is_not_clipped() {
    let mut app = App::new(TestBackend::new(40, 24), UiTheme::dark()).unwrap();
    // Single-character words so the word-wrap breaks land predictably.
    let text = "a b c d e f g h i j k l m n o p q r s t u v w x y z";
    type_text(&mut app, text);
    app.draw().unwrap();
    let r = rows(&app);
    let first = editor_top_rule(&app) + 1;
    let body: Vec<&String> = r[first..].iter().take_while(|l| !is_rule(l)).collect();
    assert!(body.len() >= 2, "the line must wrap at width 40: {body:?}");
    for (i, row) in body.iter().enumerate() {
        assert!(
            !row.starts_with(' '),
            "row {i} must start flush at column 0 like every other row: {body:?}"
        );
    }
    // Nothing was clipped off the first row: rejoining the rows reproduces the buffer.
    let rejoined: String = body
        .iter()
        .map(|s| s.trim_end())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        rejoined.replace("  ", " ").trim(),
        text,
        "a character was clipped: {body:?}"
    );
}

// ------------------------------------------------------------------------ E3 --------------------

/// **E3.** The editor's text rows cap at `max(5, floor(terminalRows * 0.3))` (`editor.ts:499-501`).
///
/// cyrup capped at `avail - 2` instead, so a long paste grew the editor until it owned the terminal
/// minus two rows and collapsed the transcript: at 40 rows pi shows 12 text rows and scrolls, cyrup
/// showed 38.
#[test]
fn the_editor_caps_at_thirty_percent_of_the_terminal_height() {
    for (term_rows, want_text_rows) in [(40u16, 12u16), (24, 7), (10, 5), (100, 30)] {
        let mut app = App::new(TestBackend::new(60, term_rows), UiTheme::dark()).unwrap();
        // 60 newline-separated lines: far more than any of these caps.
        for i in 0..60 {
            type_text(&mut app, &format!("line {i}"));
            app.handle_input(&shift_enter());
        }
        app.draw().unwrap();
        let r = rows(&app);
        let top = editor_top_rule(&app);
        let bottom = editor_bottom_rule(&app);
        assert_eq!(
            bottom - top - 1,
            usize::from(want_text_rows),
            "at a {term_rows}-row terminal pi shows max(5, floor({term_rows} * 0.3)) = \
             {want_text_rows} text rows:\n{}",
            r.join("\n")
        );
        // …and the window is FULL. The box being `want_text_rows` tall does not by itself prove the
        // editor filled it: `region_constraints` reserves the slot and `Component::render` fills it,
        // and only the scroll rule's own count can tell the two apart. 60 `Shift+Enter`s leave 61
        // layout lines with the caret on the last, so the rows hidden above are `61 - shown`.
        assert!(
            r[top].starts_with(&format!("─── ↑ {} more ", 61 - want_text_rows)),
            "the top rule must report `61 - {want_text_rows}` hidden rows — a different number \
             means the editor drew a different count than the slot reserved:\n{}",
            r.join("\n")
        );
    }
}

/// MIRROR of E3. The cap is a CEILING, not a fixed height: a short buffer still renders at its own
/// natural size (pi slices `layoutLines`, which may be shorter than `maxVisibleLines`).
#[test]
fn a_short_buffer_stays_short_under_the_thirty_percent_cap() {
    let mut app = App::new(TestBackend::new(60, 40), UiTheme::dark()).unwrap();
    type_text(&mut app, "one\ntwo");
    app.draw().unwrap();
    let r = rows(&app);
    let top = editor_top_rule(&app);
    assert_eq!(r[top + 1], "one");
    assert_eq!(r[top + 2], "two");
    assert!(
        is_rule(&r[top + 3]),
        "two rows, then the bottom rule: {:?}",
        &r[top..]
    );
}

// ------------------------------------------------------------------------ E4 --------------------

/// **E4.** Overflow SCROLLS and the rules say by how much.
///
/// `createScrollBorder("↑" | "↓", hiddenLineCount, width)` (`editor.ts:259-268`) replaces the plain
/// `─`-repeat at the top when `scrollOffset > 0` (`:526-528`) and at the bottom when rows remain
/// below (`:582-585`); `scrollOffset` is moved to keep the caret's layout line inside the window
/// before the slice (`:507-516`). cyrup drew a plain `Borders::TOP | BOTTOM` and a `Paragraph` with
/// no `.scroll()`, so the surplus rows — the caret's included — were silently clipped.
///
/// Keystroke-driven throughout: the caret is moved with real `Up`/`Down` presses and the assertion
/// is on which rows the frame actually shows.
#[test]
fn overflow_scrolls_to_the_caret_and_the_rules_count_the_hidden_rows() {
    // 24 rows → max(5, 7) = 7 visible text rows.
    let mut app = App::new(TestBackend::new(48, 24), UiTheme::dark()).unwrap();
    for i in 0..12 {
        type_text(&mut app, &format!("line {i}"));
        app.handle_input(&shift_enter());
    }
    app.draw().unwrap();

    // The caret is on the empty 13th layout line, so the window is the LAST 7 rows and 6 are hidden
    // above: the TOP rule carries `↑ 6 more`.
    let r = rows(&app);
    let top = editor_top_rule(&app);
    assert!(
        r[top].starts_with("─── ↑ 6 more "),
        "top scroll rule (`:527`): {:?}",
        r[top]
    );
    assert!(
        r[top].chars().all(|c| c != '↓'),
        "no down indicator while at the bottom: {:?}",
        r[top]
    );
    assert!(
        r.iter().any(|l| l == "line 11"),
        "the last line must be visible:\n{}",
        r.join("\n")
    );
    assert!(
        !r.iter().any(|l| l == "line 5"),
        "line 5 is scrolled off:\n{}",
        r.join("\n")
    );
    assert!(
        caret_cell(&app).is_some(),
        "the caret must be inside the window"
    );

    // Walk the caret to the very top with real Up presses. The window follows it and the rules swap
    // ends: nothing above, six below.
    for _ in 0..40 {
        app.handle_input(&code(KeyCode::Up));
    }
    app.draw().unwrap();
    let r = rows(&app);
    let top = editor_top_rule(&app);
    assert!(
        r[top].chars().all(|c| c == '─'),
        "back at the top the rule is plain: {:?}",
        r[top]
    );
    assert_eq!(
        r[top + 1],
        "line 0",
        "scroll-to-cursor put line 0 back on screen: {:?}",
        &r[top..]
    );
    let bottom = editor_bottom_rule(&app);
    assert_eq!(
        bottom,
        top + 8,
        "7 text rows between the rules: {:?}",
        &r[top..=bottom]
    );
    assert!(
        r[bottom].starts_with("─── ↓ 6 more "),
        "bottom scroll rule (`:584`): {:?}",
        r[bottom]
    );
    assert!(
        caret_cell(&app).is_some(),
        "the caret must still be inside the window"
    );
}

/// MIRROR of E4. A buffer that fits shows PLAIN rules on both ends — `createScrollBorder` is called
/// only under `scrollOffset > 0` / `linesBelow > 0`, so an indicator on a short buffer would be as
/// wrong as its absence on a long one.
#[test]
fn a_buffer_that_fits_draws_plain_rules_with_no_indicator() {
    let mut app = App::new(TestBackend::new(48, 24), UiTheme::dark()).unwrap();
    type_text(&mut app, "one\ntwo\nthree");
    app.draw().unwrap();
    let joined = rows(&app).join("\n");
    assert!(
        !joined.contains("more "),
        "no scroll indicator on a fitting buffer:\n{joined}"
    );
    assert!(
        !joined.contains('↑') && !joined.contains('↓'),
        "no arrows either:\n{joined}"
    );
}

// ----------------------------------------------------------------------- E13 --------------------

/// **E13.** The caret survives focus loss.
///
/// `layoutText` sets `hasCursor` from the cursor position alone (`editor.ts:905-960`) and
/// `render` emits the reverse-video cell whenever `layoutLine.hasCursor` (`:545-564`). `focused`
/// gates only `emitCursorMarker` (`:537`), the zero-width `CURSOR_MARKER` used for IME placement —
/// cyrup's counterpart being `InputEditor::cursor_in`, the sole `set_cursor_position` caller.
/// cyrup used to set the caret's row index to `usize::MAX` when unfocused, so clicking away from
/// the terminal erased the caret entirely.
///
/// Keystroke-driven: the caret must be where the typing left it, both before and after `FocusLost`.
#[test]
fn the_caret_stays_visible_when_the_terminal_loses_focus() {
    let mut app = App::new(TestBackend::new(48, 24), UiTheme::dark()).unwrap();
    type_text(&mut app, "hello");
    app.handle_input(&code(KeyCode::Left));
    app.handle_input(&code(KeyCode::Left));
    app.draw().unwrap();
    let focused = caret_cell(&app).expect("a focused editor draws its caret");
    // Two Lefts from the end of `hello` ⇒ the caret is on the `l` at column 3.
    assert_eq!(
        focused.1, 3,
        "the caret follows the arrow keys, got {focused:?}"
    );

    app.handle_input(&InputEvent::FocusLost);
    app.draw().unwrap();
    let blurred = caret_cell(&app);
    assert_eq!(
        blurred,
        Some(focused),
        "E13: pi keeps the reverse-video caret on focus loss (`editor.ts:545`) — it is not gated \
         on `focused`"
    );

    app.handle_input(&InputEvent::FocusGained);
    app.draw().unwrap();
    assert_eq!(
        caret_cell(&app),
        Some(focused),
        "and regaining focus changes nothing about it"
    );
}

/// MIRROR of E13. What `focused` DOES gate is the hardware cursor — pi's `emitCursorMarker`
/// (`editor.ts:537`). With `showHardwareCursor` on, a blurred editor must place no terminal cursor
/// even though its soft caret is still painted; a fix that simply ignored `focused` everywhere
/// would leave a real cursor blinking in an unfocused window.
#[test]
fn focus_loss_still_withdraws_the_hardware_cursor() {
    let mut app = App::new(TestBackend::new(48, 24), UiTheme::dark()).unwrap();
    app.editor_mut().set_show_hardware_cursor(true);
    type_text(&mut app, "hi");
    app.draw().unwrap();
    assert!(
        app.terminal().backend().cursor_visible(),
        "baseline: focused + enabled ⇒ visible"
    );

    app.handle_input(&InputEvent::FocusLost);
    app.draw().unwrap();
    assert!(
        !app.terminal().backend().cursor_visible(),
        "`emitCursorMarker = this.focused` (`editor.ts:537`): no marker while blurred"
    );
    assert!(
        caret_cell(&app).is_some(),
        "…while the SOFT caret stays (E13)"
    );
}

// ----------------------------------------------------------------------- E14 --------------------

/// **E14.** The autocomplete popup lives inside the editor's padding frame.
///
/// pi renders it at `contentWidth` and prefixes the same `leftPadding` every text row gets
/// (`editor.ts:591-597`). cyrup drew it into a sibling full-width region at column 0, so with
/// `editorPaddingX` 1-3 — the values `/settings` cycles — the completions were flush left while the
/// text they complete was inset.
#[test]
fn the_autocomplete_popup_is_indented_under_the_editor_padding() {
    let mut app = App::new(TestBackend::new(70, 24), UiTheme::dark()).unwrap();
    app.editor_mut().set_padding_x(3);
    type_text(&mut app, "/hotk");
    app.draw().unwrap();
    let r = rows(&app);
    let top = editor_top_rule(&app);
    assert_eq!(
        r[top + 1],
        "   /hotk",
        "the text is inset 3 (`:522`): {:?}",
        r[top + 1]
    );
    let popup = &r[top + 3];
    assert!(
        popup.contains("hotkeys"),
        "the popup must be open: {:?}",
        &r[top..]
    );
    assert!(
        popup.starts_with("   "),
        "E14: the popup carries the same `leftPadding` as the text (`:596`): {popup:?}"
    );
}

/// MIRROR of E14. At the default `editorPaddingX = 0` the popup is flush at column 0 — the inset is
/// the padding, not a new constant one.
#[test]
fn the_popup_is_flush_at_the_default_zero_padding() {
    let mut app = App::new(TestBackend::new(70, 24), UiTheme::dark()).unwrap();
    type_text(&mut app, "/hotk");
    app.draw().unwrap();
    let r = rows(&app);
    let top = editor_top_rule(&app);
    let popup = &r[top + 3];
    assert!(
        popup.contains("hotkeys"),
        "the popup must be open: {:?}",
        &r[top..]
    );
    assert!(
        !popup.starts_with(' '),
        "no inset at paddingX = 0: {popup:?}"
    );
}

// ----------------------------------------------------------------------- E15 --------------------

/// **E15.** The slot is measured at the width the editor renders at.
///
/// pi derives one `layoutWidth` and feeds it to both `this.lastWidth` and `layoutText()`
/// (`editor.ts:489-497`), so the rows the container reserves are by construction the rows the
/// editor draws. cyrup measured with `visual_line_count(width - 1)` while rendering at
/// `layout_width(width)` = `width - 2 * paddingX`: with `editorPaddingX > 0` the render wrapped
/// NARROWER than the measurement and produced rows the slot had no space for — clipped from the
/// bottom, so the last rows of a long line, and with them the caret, simply vanished.
///
/// Asserted where it bites: type a line long enough to wrap at the padded width but NOT at the
/// unpadded one, then require that the caret (which sits at the end of the buffer) is on screen.
#[test]
fn a_padded_editor_reserves_every_row_it_renders() {
    const W: u16 = 40;
    const PAD: u16 = 6;
    let mut app = App::new(TestBackend::new(W, 24), UiTheme::dark()).unwrap();
    app.editor_mut().set_padding_x(PAD.into());
    // `layout_width` is `W - 2*PAD` = 28 with padding, and `W - 1` = 39 without: 34 characters wrap
    // at the real width and do not at the width the old measurement used.
    let text = "abcdefghij klmnopqrst uvwxyz 01234";
    assert_eq!(text.len(), 34);
    type_text(&mut app, text);
    app.draw().unwrap();

    let r = rows(&app);
    let top = editor_top_rule(&app);
    let bottom = editor_bottom_rule(&app);
    assert_eq!(
        bottom - top - 1,
        2,
        "34 chars wrap to 2 rows at width 28:\n{}",
        r.join("\n")
    );
    assert!(
        r[top + 2].contains("01234"),
        "the LAST row must be on screen: {:?}",
        &r[top..=bottom]
    );
    let caret = caret_cell(&app).expect("the end-of-buffer caret must be inside the slot");
    assert_eq!(
        caret.0 as usize,
        top + 2,
        "the caret rides the last wrapped row, got {caret:?}"
    );
}

/// MIRROR of E15. The unpadded case — where measurement and render always agreed — is unchanged:
/// `layout_width(width)` is `width - 1` when `paddingX == 0` (`editor.ts:489`, the reserved
/// end-of-line cursor column), so a 39-character line still occupies exactly one row at width 40.
#[test]
fn an_unpadded_editor_still_reserves_the_end_of_line_cursor_column() {
    let mut app = App::new(TestBackend::new(40, 24), UiTheme::dark()).unwrap();
    let text = "a".repeat(39);
    type_text(&mut app, &text);
    app.draw().unwrap();
    let r = rows(&app);
    let top = editor_top_rule(&app);
    assert_eq!(
        r[top + 1],
        text,
        "39 chars fit the 39-column layout width: {:?}",
        &r[top..]
    );
    assert!(
        is_rule(&r[top + 2]),
        "one row, then the rule: {:?}",
        &r[top..]
    );
}

// ------------------------------------------------------------------ W1 + W2 ---------------------

/// **W1 — `wordWrapLine` measures DISPLAY WIDTH, not char count.**
///
/// Upstream accumulates `visibleWidth(grapheme)` per cluster (`editor.ts:139-143`) and records a
/// wrap opportunity between two adjacent CJK graphemes (`:191-198`), because CJK text carries no
/// spaces to break at. cyrup compared `n - start <= width` over a `&[char]`, and
/// `visual_line_count`/`visual_line_map` are both built on that function.
///
/// 24 CJK ideographs are 24 `char`s and **48 columns**. At an editor 40 columns wide the layout
/// width is 39 (`editor.ts:489` reserves one column for the end-of-line cursor), so the char count
/// said "fits", the map reported ONE visual line, the last nine ideographs rendered past the right
/// edge, and — because the end-of-line caret is emitted after them — the caret left the frame
/// entirely. There was no visible cursor anywhere on screen.
#[test]
fn cjk_wraps_on_display_columns_and_keeps_the_caret_in_the_frame() {
    let cjk: String = "日本語".chars().cycle().take(24).collect();
    assert_eq!(cjk.chars().count(), 24, "24 chars…");

    let mut app = App::new(TestBackend::new(40, 24), UiTheme::dark()).unwrap();
    type_text(&mut app, &cjk);
    app.draw().unwrap();
    let r = rows(&app);
    let top = editor_top_rule(&app);
    let bottom = editor_bottom_rule(&app);

    assert_eq!(
        bottom - top - 1,
        2,
        "48 columns of CJK cannot be one 39-column row:\n{}",
        r.join("\n")
    );
    // ratatui parks a filler blank in the second cell of every wide grapheme, so read the rows back
    // through their non-blank cells — the CJK content itself has no spaces.
    let glyphs = |row: &str| {
        row.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
    };
    // The break lands where the columns run out: 19 ideographs are 38 columns, a 20th would be 40.
    assert_eq!(
        glyphs(&r[top + 1]).chars().count(),
        19,
        "row 0 fills the width in COLUMNS: {:?}",
        r[top + 1]
    );
    assert_eq!(
        glyphs(&r[top + 2]).chars().count(),
        5,
        "the remainder: {:?}",
        r[top + 2]
    );
    assert_eq!(
        format!("{}{}", glyphs(&r[top + 1]), glyphs(&r[top + 2])),
        cjk,
        "the two rows must reproduce the buffer exactly — nothing clipped, nothing duplicated"
    );
    // The caret is what actually goes missing: it rides the end of the buffer, i.e. the SECOND row,
    // at the display column after five ideographs.
    assert_eq!(
        caret_cell(&app),
        Some(((top + 2) as u16, 10)),
        "the end-of-buffer caret must be on the last wrapped row, at 5 ideographs = 10 columns:\n{}",
        r.join("\n")
    );
}

/// **W2 — a grapheme cluster is never torn across the wrap boundary.**
///
/// Same root cause as W1. `Component::render` slices each visual row out of its logical line by
/// CHAR index (`line[vl.start .. vl.start + vl.len]`), so a boundary that a char-count wrap put
/// mid-cluster split the cluster: 38 ASCII characters plus a ZWJ family emoji, at layout width 39,
/// broke after char 39 — putting the lone `👨` at the end of row 0 and an orphaned
/// `\u{200d}👩‍👧‍👦` at the head of row 1.
///
/// Upstream cannot produce that: it iterates `graphemeSegmenter.segment(line)` and force-breaks at
/// `seg.index`, the cluster's OWN start (`editor.ts:154-160`), so every chunk boundary is a cluster
/// boundary. With the wrap fixed, the render's char slice is grapheme-aligned by construction.
#[test]
fn a_grapheme_cluster_is_never_torn_across_the_wrap_boundary() {
    let head = "a".repeat(38);
    let mut app = App::new(TestBackend::new(40, 24), UiTheme::dark()).unwrap();
    type_text(&mut app, &format!("{head}{FAMILY}"));
    app.draw().unwrap();
    let r = rows(&app);
    let top = editor_top_rule(&app);

    assert_eq!(
        r[top + 1],
        head,
        "row 0 is the ASCII run alone — the emoji does not fit: {:?}",
        r[top + 1]
    );
    assert!(
        r[top + 2].starts_with(FAMILY),
        "W2: the family emoji must arrive on row 1 WHOLE, not as a bare `\\u{{200d}}` tail: {:?}",
        r[top + 2]
    );
    // Belt and braces: no fragment of the cluster stayed behind on row 0.
    assert!(
        !r[top + 1].contains('\u{1f468}'),
        "the leading `👨` was left on row 0 — the cluster was split: {:?}",
        r[top + 1]
    );
}

// ------------------------------------------------------ untested-fix coverage (item 3) ----------

/// **U1 (E4's hardware-cursor half).** `cursor_in`'s ROW is the caret's position inside the SCROLLED
/// window, `vi - scrollOffset`, because `render` slices `layoutLines` from `scrollOffset`
/// (`editor.ts:519`) and the caret marker is emitted into that slice (`:545-550`).
///
/// This landed with the rest of E4 and nothing exercised it: reverting it to the absolute `vi` left
/// the whole suite green. Absolute rows are clamped by `y.min(max_y)` to the BOTTOM RULE, one row
/// below where the caret is actually painted, so with `showHardwareCursor` on the terminal cursor
/// (and with it the IME candidate window) sat on the rule instead of on the text.
#[test]
fn the_hardware_cursor_rides_the_scrolled_window_not_the_absolute_row() {
    // 24 rows ⇒ max(5, floor(24 * 0.3)) = 7 visible text rows.
    let mut app = App::new(TestBackend::new(48, 24), UiTheme::dark()).unwrap();
    app.editor_mut().set_show_hardware_cursor(true);
    for i in 0..12 {
        type_text(&mut app, &format!("line {i}"));
        app.handle_input(&shift_enter());
    }
    app.draw().unwrap();

    let top = editor_top_rule(&app);
    let bottom = editor_bottom_rule(&app);
    assert_eq!(
        bottom - top - 1,
        7,
        "precondition: a 7-row window over 13 layout lines"
    );
    assert!(
        rows(&app)[top].starts_with("─── ↑ "),
        "precondition: the window really scrolled"
    );

    let pos = app.terminal().backend().cursor_position();
    let soft = caret_cell(&app).expect("the soft caret is inside the window");
    assert_eq!(
        (usize::from(pos.y), pos.x),
        (bottom - 1, soft.1),
        "the hardware cursor must sit on the caret's row INSIDE the window (the last text row, \
         {}), not on the bottom rule at {bottom} where an absolute `vi` gets clamped:\n{}",
        bottom - 1,
        rows(&app).join("\n")
    );
    assert_eq!(
        usize::from(pos.y),
        usize::from(soft.0),
        "and it agrees with the soft caret"
    );
}

/// **U2 (the wide-character caret column).** `cursor_in`'s COLUMN is the DISPLAY WIDTH of the text
/// before the caret, not its char count.
///
/// Upstream never does this arithmetic — it splices a zero-width `CURSOR_MARKER` into the row string
/// at `layoutLine.cursorPos` (`editor.ts:546-550`) and lets the terminal advance by real cell
/// widths — so a char-count offset is a cyrup-only defect. It also landed untested: reverting
/// `before_width` to the raw `vcol` left the suite green, because the one test that reads
/// `cursor_position().x` (`settings_inert_keys::hardware_cursor_respects_editor_padding`) types
/// pure ASCII, where the two are equal by definition.
///
/// `日本` is 2 chars / 4 columns and the ZWJ family is 7 chars / 2 columns, so the caret at the end
/// of `日本👨‍👩‍👧‍👦x` is at column **7** and char offset **10** — and the reverse-video cell it is
/// supposed to coincide with is at 7.
#[test]
fn the_hardware_cursor_column_is_display_width_not_char_count() {
    let text = format!("日本{FAMILY}x");
    assert_eq!(text.chars().count(), 10, "10 chars…");

    let mut app = App::new(TestBackend::new(60, 24), UiTheme::dark()).unwrap();
    app.editor_mut().set_show_hardware_cursor(true);
    type_text(&mut app, &text);
    app.draw().unwrap();

    let pos = app.terminal().backend().cursor_position();
    assert_eq!(pos.x, 7, "…and 4 + 2 + 1 = 7 COLUMNS: {pos:?}");
    assert_eq!(
        (pos.y, pos.x),
        caret_cell(&app).expect("the soft caret"),
        "the hardware cursor and the reverse-video cell must be the same cell — that is the whole \
         point of computing the column in display units"
    );
}

/// **U3 (the grapheme-cluster caret cell).** The highlighted cell is one whole CLUSTER.
///
/// `render` takes `afterGraphemes[0].segment` and slices the rest past it by that segment's length
/// (`editor.ts:555-559`), so `\x1b[7m…\x1b[0m` wraps the entire cluster. This landed with E13 and
/// was likewise untested: reverting it to `seg.get(vcol)` — one `char` — kept the suite green,
/// while on screen it inverts a lone `👨` and leaves `\u{200d}👩‍👧‍👦` un-highlighted beside it.
///
/// Keystroke-driven: type past the emoji, then walk the cursor back ONTO it with `Left`, which is
/// itself grapheme-aware, so the assertion also pins that the caret's cell and the cursor's own
/// motion unit agree.
#[test]
fn the_soft_caret_inverts_a_whole_grapheme_cluster() {
    let mut app = App::new(TestBackend::new(48, 24), UiTheme::dark()).unwrap();
    type_text(&mut app, &format!("{FAMILY}x"));
    app.handle_input(&code(KeyCode::Left));
    app.handle_input(&code(KeyCode::Left));
    app.draw().unwrap();

    let (y, x) = caret_cell(&app).expect("the caret is on the emoji");
    assert_eq!(
        x, 0,
        "two Lefts over two grapheme clusters put the caret at column 0"
    );
    let buf = app.terminal().backend().buffer();
    assert_eq!(
        buf.cell((x, y)).unwrap().symbol(),
        FAMILY,
        "U3: the inverted cell is the WHOLE cluster, not its leading `👨`"
    );
    // The cluster is two columns wide, so the `x` that follows it starts at column 2 and is NOT
    // inverted — i.e. the highlight covers the cluster and stops.
    assert_eq!(
        buf.cell((2, y)).unwrap().symbol(),
        "x",
        "the cluster occupies both of its columns"
    );
    assert!(
        !buf.cell((2, y))
            .unwrap()
            .modifier
            .contains(Modifier::REVERSED)
    );
}

// ----------------------------------------------------------------------- E17 --------------------

/// **E17.** The editor's row budget is INTRINSIC to the component.
///
/// `Editor.render(width)` takes no height at all; it reads `this.tui.terminal.rows` and computes
/// `maxVisibleLines = Math.max(5, Math.floor(terminalRows * 0.3))` itself (`editor.ts:499-501`),
/// then slices `layoutLines` to it (`:519`). cyrup derived the window solely from `area.height - 2`
/// — right only for as long as the single caller happened to size the slot from the same formula,
/// and silently uncapped for any other caller.
///
/// Rendered here into a rect DELIBERATELY taller than the budget: a 24-row terminal buys 7 text
/// rows, and a 20-row rect must not turn that into 18.
#[test]
fn the_editor_caps_itself_at_thirty_percent_regardless_of_the_rect_it_is_given() {
    let mut ed = InputEditor::new();
    ed.set_terminal_height(24);
    ed.set_text(
        &(0..30)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    let mut term = Terminal::new(TestBackend::new(40, 20)).unwrap();
    term.draw(|f| ed.render(f, f.area(), &UiTheme::dark()))
        .unwrap();
    let buf = term.backend().buffer();
    let row = |y: u16| {
        let mut s = String::new();
        for x in 0..buf.area.width {
            s.push_str(buf.cell((x, y)).unwrap().symbol());
        }
        s.trim_end().to_string()
    };

    // `scrollOffset` chases the caret (`editor.ts:507-516`), which `set_text` leaves at the end of
    // the buffer, so the window is the LAST 7 lines and 23 are hidden above.
    assert_eq!(
        row(1),
        "line 23",
        "the window is the 7 rows around the caret"
    );
    assert_eq!(row(7), "line 29", "max(5, floor(24 * 0.3)) = 7 text rows");
    assert_eq!(
        row(8),
        "",
        "E17: row 8 must be EMPTY — the rect has 18 text rows of space and the editor is entitled \
         to 7 of them"
    );
    // And the rows it declined to draw are announced, not silently dropped.
    assert!(
        row(0).starts_with("─── ↑ 23 more "),
        "the top rule counts the hidden rows (`editor.ts:526-528`): {:?}",
        row(0)
    );
}

/// MIRROR of E17. The rect is still in the `min`: a slot CLIPPED shorter than the intrinsic budget
/// wins, so the editor never overdraws a container that gave it less than it asked for.
#[test]
fn a_rect_shorter_than_the_intrinsic_budget_still_wins() {
    let mut ed = InputEditor::new();
    ed.set_terminal_height(100); // intrinsic budget 30
    ed.set_text(
        &(0..30)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    let mut term = Terminal::new(TestBackend::new(40, 6)).unwrap();
    term.draw(|f| ed.render(f, f.area(), &UiTheme::dark()))
        .unwrap();
    let buf = term.backend().buffer();
    let row = |y: u16| {
        let mut s = String::new();
        for x in 0..buf.area.width {
            s.push_str(buf.cell((x, y)).unwrap().symbol());
        }
        s.trim_end().to_string()
    };
    assert!(
        row(5).chars().all(|c| c == '─'),
        "nothing below the caret: {:?}",
        row(5)
    );
    assert_eq!(
        row(1),
        "line 26",
        "the window is the 4 rows the RECT allows, around the caret"
    );
    assert_eq!(
        row(4),
        "line 29",
        "4 text rows in a 6-row rect, not the intrinsic 30"
    );
}
