//! `/fork` — the "Fork from Message" picker (`components/user-message-selector.ts`, 155 lines).
//!
//! **S22.** cyrup routed `/fork` through `ListSelector::data`, which is a different component in
//! every structural respect: one row per message instead of three lines, a `"→ "` cursor instead of
//! `"› "`, an accent+bold title INSIDE the top rule instead of a plain-bold title above it, no
//! subtitle, and a metadata string of `message 3` instead of `Message 3 of 12`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{
    App, AppCommand, InputEvent, SelectorKind, UiTheme, UserMessageRow, UserMessageSelector,
};
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;

fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
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

fn rows() -> Vec<UserMessageRow> {
    vec![
        UserMessageRow { id: "e1".into(), text: "first question".into() },
        UserMessageRow { id: "e2".into(), text: "second\nquestion".into() },
        UserMessageRow { id: "e3".into(), text: "third question".into() },
    ]
}

fn fork_app() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(78, 24), UiTheme::dark()).unwrap();
    app.open_boxed_selector(
        SelectorKind::UserMessage,
        Box::new(UserMessageSelector::new(rows(), None)),
    );
    app.draw().unwrap();
    app
}

/// Locate the first buffer row containing `needle`, returning `(y, row_text)`.
fn row_with(app: &App<TestBackend>, needle: &str) -> (u16, String) {
    let buf = app.terminal().backend().buffer();
    for y in 0..buf.area.height {
        let mut row = String::new();
        for x in 0..buf.area.width {
            if let Some(c) = buf.cell((x, y)) {
                row.push_str(c.symbol());
            }
        }
        if row.contains(needle) {
            return (y, row);
        }
    }
    panic!("no row contains {needle:?}");
}

/// **S22, the row shape.** `UserMessageList.render` pushes THREE lines per visible entry
/// (`user-message-selector.ts:49-70`): the cursor + message, then
/// `theme.fg("muted", `  Message ${position} of ${n}`)` (`:64-68`), then a blank (`:69`). cyrup drew
/// one two-column `SelectList` row whose right column read `message 3` — lowercase, no ` of N`.
#[test]
fn fork_renders_three_lines_per_message() {
    let app = fork_app();
    let (msg_y, msg) = row_with(&app, "first question");
    assert_eq!(msg.trim_end(), "  first question", "unselected cursor is two spaces (`:57`)");

    let buf = app.terminal().backend().buffer();
    let read = |y: u16| {
        let mut s = String::new();
        for x in 0..buf.area.width {
            s.push_str(buf.cell((x, y)).unwrap().symbol());
        }
        s.trim_end().to_string()
    };
    assert_eq!(read(msg_y + 1), "  Message 1 of 3", "capital M and the ` of N` tail (`:66`)");
    assert_eq!(read(msg_y + 2), "", "a blank line between messages (`:69`)");
    // Every entry gets the triple, including the last.
    assert_eq!(read(msg_y + 3), "  second question");
    assert_eq!(read(msg_y + 4), "  Message 2 of 3");
    assert_eq!(read(msg_y + 5), "");
}

/// The `/fork` cursor is `theme.fg("accent", "› ")` — U+203A (`:57`), not the `"→ "` U+2192 every
/// `SelectList`-backed picker uses — and the selected message is `theme.bold(...)` (`:60`).
#[test]
fn fork_cursor_is_a_bold_single_angle_quote() {
    let app = fork_app();
    let theme = UiTheme::dark();
    // `UserMessageList` preselects the most recent message (`:26`).
    let (y, row) = row_with(&app, "third question");
    assert!(row.starts_with("\u{203a} "), "U+203A cursor, not U+2192: {row:?}");
    let buf = app.terminal().backend().buffer();
    assert_eq!(buf.cell((0, y)).unwrap().fg, theme.accent_style().fg.unwrap(), "accent cursor");
    assert!(
        buf.cell((2, y)).unwrap().modifier.contains(Modifier::BOLD),
        "the highlighted message is bold (`:60`)"
    );
    assert!(
        !buf.cell((2, y - 3)).unwrap().modifier.contains(Modifier::BOLD),
        "an unselected message is not"
    );
    let text = buf_text(&app);
    assert!(!text.contains('\u{2192}'), "no U+2192 anywhere in this dialog: {text}");
}

/// **S22, the envelope.** The header sits ABOVE the top rule and the title is `theme.bold(...)`
/// with **no** `theme.fg` (`:122-133`): `Spacer` / bold title / muted subtitle / `Spacer` /
/// `DynamicBorder`. cyrup put an accent+bold title INSIDE the border and had no subtitle at all.
#[test]
fn fork_header_sits_above_the_top_rule_with_a_subtitle() {
    let app = fork_app();
    let theme = UiTheme::dark();
    let (title_y, title) = row_with(&app, "Fork from Message");
    let (rule_y, _) = row_with(&app, "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}");
    assert!(title_y < rule_y, "title above the top rule (`:123` before `:132`)");
    assert!(title.starts_with(" Fork from Message"), "`Text(…, 1, 0)` inset: {title:?}");

    let buf = app.terminal().backend().buffer();
    let bold_cell = buf.cell((1, title_y)).unwrap();
    assert!(bold_cell.modifier.contains(Modifier::BOLD), "`theme.bold(title)` (`:123`)");
    assert_ne!(
        bold_cell.fg,
        theme.accent_style().fg.unwrap(),
        "`:123` has NO `theme.fg` — unlike every other picker title"
    );

    let (sub_y, sub) = row_with(&app, "Select a user message");
    assert_eq!(sub_y, title_y + 1, "the subtitle follows the title with no blank between");
    assert_eq!(
        buf.cell((1, sub_y)).unwrap().fg,
        theme.muted_style().fg.unwrap(),
        "muted subtitle (`:126`)"
    );
    assert!(
        sub.contains("copy the active path up to that point into a new"),
        "verbatim `:126` copy: {sub:?}"
    );
}

/// `message.text.replace(/\n/g, " ").trim()` (`:54`) — a newline in a user message becomes a space
/// rather than breaking the row.
#[test]
fn fork_normalizes_a_multiline_message_to_one_row() {
    let app = fork_app();
    let (_, row) = row_with(&app, "second");
    assert_eq!(row.trim_end(), "  second question");
}

/// Navigation wraps (`:84-90`) and confirm carries the entry id.
#[test]
fn fork_wraps_and_confirms_the_entry_id() {
    let mut app = fork_app();
    // Preselected on the newest (index 2); Down wraps to the oldest.
    app.handle_input(&key(KeyCode::Down));
    let action = app.handle_input(&key(KeyCode::Enter));
    assert_eq!(app.active_selector_kind(), None, "confirm closes the picker");
    match action {
        cyrup_tui::AppAction::Command(AppCommand::ConfirmSelection { kind, value }) => {
            assert_eq!(kind, SelectorKind::UserMessage);
            assert_eq!(value, "e1", "Down from the last row wraps to the first (`:89`)");
        }
        other => panic!("expected ConfirmSelection command, got {other:?}"),
    }
}

/// A `(i/N)` scroll row appears once the list is longer than `maxVisible = 10` (`:19`, `:73-76`).
#[test]
fn fork_shows_a_scroll_indicator_past_ten_messages() {
    let msgs: Vec<UserMessageRow> = (0..14)
        .map(|i| UserMessageRow { id: format!("e{i}"), text: format!("msg {i}") })
        .collect();
    let mut app = App::new(TestBackend::new(78, 40), UiTheme::dark()).unwrap();
    app.open_boxed_selector(
        SelectorKind::UserMessage,
        Box::new(UserMessageSelector::new(msgs, None)),
    );
    app.draw().unwrap();
    let (_, row) = row_with(&app, "(14/14)");
    assert_eq!(row.trim_end(), "  (14/14)", "muted `  (i/N)` (`:74`)");
}
