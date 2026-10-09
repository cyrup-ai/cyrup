#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::command_highlight::registry_with_hinted_dynamic;
use crate::editor::*;

// ---- assembled render: highlight + ghost actually reach the frame -----------------------

#[test]
fn render_paints_the_token_in_accent_and_the_ghost_in_dim() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    let theme = UiTheme::default();
    let mut ed = InputEditor::new();
    ed.set_text("/model ");
    let mut term = Terminal::new(TestBackend::new(40, 4)).unwrap();
    term.draw(|f| {
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 4,
        };
        ed.render(f, area, &theme);
    })
    .unwrap();
    let buf = term.backend().buffer();
    // Row 1 is the first text row inside the top-bordered block.
    let accent = theme.accent_style();
    let dim = theme.dim_style();
    // "/model" (6 cells) must all carry the accent foreground.
    for x in 0..6u16 {
        assert_eq!(
            buf.cell((x, 1)).unwrap().fg,
            accent.fg.unwrap(),
            "column {x} of \"/model\" should be accent-colored"
        );
    }
    // Somewhere after the caret, the dim ghost text "<provider/model>" must appear.
    let row1: String = (0..40)
        .map(|x| buf.cell((x, 1)).unwrap().symbol().to_string())
        .collect();
    assert!(
        row1.contains("<provider/model>"),
        "ghost text missing from row: {row1:?}"
    );
    // And at least one of the ghost's cells carries the dim foreground.
    let ghost_start = row1.find('<').expect("ghost text present");
    assert_eq!(
        buf.cell((ghost_start as u16, 1)).unwrap().fg,
        dim.fg.unwrap(),
        "the ghost text must render in dim_style()"
    );
}

#[test]
fn render_shows_no_highlight_or_ghost_for_an_unknown_command() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    let theme = UiTheme::default();
    let mut ed = InputEditor::new();
    ed.set_text("/bogus thing");
    let mut term = Terminal::new(TestBackend::new(40, 4)).unwrap();
    term.draw(|f| {
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 4,
        };
        ed.render(f, area, &theme);
    })
    .unwrap();
    let buf = term.backend().buffer();
    let base = theme.base_style();
    let base_fg = base.fg.unwrap_or(ratatui::style::Color::Reset);
    for x in 0..12u16 {
        assert_eq!(
            buf.cell((x, 1)).unwrap().fg,
            base_fg,
            "column {x} of an unrecognized command must render in the plain base style"
        );
    }
}

#[test]
fn render_never_grows_a_row_for_a_clipped_ghost() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    let theme = UiTheme::default();
    let mut ed = InputEditor::new();
    ed.set_registry(registry_with_hinted_dynamic(
        "flux/aug",
        "todo_file | number_of_agents | additional_instructions",
    ));
    ed.set_text("/flux/aug ");
    // A narrow area: the ghost must clip with "…" rather than wrap onto a second row.
    let mut term = Terminal::new(TestBackend::new(20, 4)).unwrap();
    term.draw(|f| {
        let area = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 4,
        };
        ed.render(f, area, &theme);
    })
    .unwrap();
    let buf = term.backend().buffer();
    let row1: String = (0..20)
        .map(|x| buf.cell((x, 1)).unwrap().symbol().to_string())
        .collect();
    assert!(
        row1.trim_end().ends_with('…'),
        "the ghost should clip with an ellipsis: {row1:?}"
    );
    // Only ONE text row was used — the ghost did not push content onto row 2.
    let row2: String = (0..20)
        .map(|x| buf.cell((x, 2)).unwrap().symbol().to_string())
        .collect();
    assert!(
        row2.trim().is_empty() || row2.trim_start_matches('─').trim().is_empty(),
        "the ghost must not grow the editor's row count: {row2:?}"
    );
}

/// TUI-172 under a command token. Pi drops the fake cursor by deleting only its markers
/// (`resolveFakeCursors`, `line.replace(FOCUSED_FAKE_CURSOR, "$1$2")`, `tui.ts:1478-1484`
/// @f1b2e77f5); `renderFakeCursor` only wraps the grapheme (`tui.ts:204-206`, `editor.ts:591`), so
/// the character under the hardware cursor keeps whatever styling surrounds it. Here that is the
/// accent of `/model`: with the hardware caret on, the caret grapheme stays accent and is not
/// reversed; with it off, the drawn caret is reverse video as before.
#[test]
fn the_caret_grapheme_in_an_accented_token_keeps_the_accent_under_the_hardware_cursor() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;

    let theme = UiTheme::default();
    let mut ed = InputEditor::new();
    ed.set_text("/model ");
    ed.move_home();
    ed.move_right();
    ed.move_right();
    let area = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 4,
    };
    let accent_fg = theme.accent_style().fg.unwrap();

    ed.set_show_hardware_cursor(true);
    let mut term = Terminal::new(TestBackend::new(40, 4)).unwrap();
    term.draw(|f| ed.render(f, area, &theme)).unwrap();
    let cell = term.backend().buffer().cell((2, 1)).unwrap().clone();
    assert_eq!(cell.symbol(), "o", "the caret sits on the `o` of `/model`");
    assert_eq!(
        cell.fg, accent_fg,
        "hardware caret on: the grapheme under it keeps the token's accent"
    );
    assert!(
        !cell.modifier.contains(Modifier::REVERSED),
        "hardware caret on: no drawn caret"
    );
    assert_eq!(
        term.backend().cursor_position(),
        (2, 1).into(),
        "and the terminal cursor is on that cell"
    );

    ed.set_show_hardware_cursor(false);
    let mut term = Terminal::new(TestBackend::new(40, 4)).unwrap();
    term.draw(|f| ed.render(f, area, &theme)).unwrap();
    let cell = term.backend().buffer().cell((2, 1)).unwrap().clone();
    assert_eq!(cell.symbol(), "o");
    assert!(
        cell.modifier.contains(Modifier::REVERSED),
        "hardware caret off: the drawn caret is reverse video as before"
    );
}
