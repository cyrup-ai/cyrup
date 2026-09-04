//! **S31** — every search box in the TUI draws pi's ONE `Input` prompt.
//!
//! `Input.render` opens with `const prompt = "> ";` (`packages/tui/src/components/input.ts:380`),
//! two columns, and applies no colour to it anywhere in the function (`:379-446`). Every component
//! that owns an `Input` adds it to its container as a **bare child** — no `Text`/`TruncatedText`
//! wrapper to inset it — so the prompt lands at column 0:
//!
//! | component | site |
//! |---|---|
//! | `oauth-selector.ts` (`/login`, `/logout`) | `:86` |
//! | `scoped-models-selector.ts` | `:140` |
//! | `model-selector.ts` (`/model`) | `:118` |
//! | `session-selector.ts` (`/resume`) | `:418` `lines.push(...this.searchInput.render(width))` |
//! | `config-selector.ts` | `:396` |
//! | `settings-list.ts` (`/settings`) | `:94` |
//! | `extension-input.ts` (`ui.input`) | `:64` |
//! | `login-dialog.ts` | `:140`, `:160` |
//! | `tree-selector.ts` (`LabelInput`) | `:1302` — the ONE exception: a literal two-space `indent`
//!   is prefixed **before** the prompt, giving `"  " + "> " + value` |
//!
//! cyrup had three separate inventions instead — `model_selector.rs`'s accent `" ▏"…"▏"` bars
//! (U+258F occurs in no pi TUI source at all), an accent `" > "` in `session_selector.rs` and
//! `login_dialog.rs`, and NO prompt at all in `tree_selector.rs`'s rename box — each one column
//! further right than upstream, and each coloured.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{
    INPUT_PROMPT, LoginDialog, ModelEntry, ModelSelector, SelectKeymap, Selector, SelectorOutcome,
    SessionRow, SessionSelector, TreeNode, TreeSelector, UiTheme,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

fn ch(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

/// Render `sel` at `width` x 24 and return one string per row.
fn rows_of(sel: &mut dyn Selector, width: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
    let theme = UiTheme::dark();
    terminal
        .draw(|f| sel.render(f, Rect::new(0, 0, width, 24), &theme))
        .unwrap();
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            let mut row = String::new();
            for x in 0..buf.area.width {
                row.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            row
        })
        .collect()
}

/// The row carrying the `Input`, and its index.
fn prompt_row(rows: &[String], expected_prefix: &str) -> (usize, String) {
    rows.iter()
        .enumerate()
        .find(|(_, r)| r.starts_with(expected_prefix))
        .map(|(i, r)| (i, r.clone()))
        .unwrap_or_else(|| {
            panic!(
                "no row starts with {expected_prefix:?}:\n{}",
                rows.join("\n")
            )
        })
}

#[test]
fn the_prompt_constant_is_exactly_pis_two_column_marker() {
    assert_eq!(
        INPUT_PROMPT, "> ",
        "`input.ts:380` `const prompt = \"> \";`"
    );
}

/// `/model` — `model-selector.ts:118`. cyrup drew accent `" ▏"…"▏"` bars around the value.
#[test]
fn model_selector_search_box_uses_the_shared_prompt() {
    let mut sel = ModelSelector::new(vec![ModelEntry {
        id: "claude-opus-4-6".to_string(),
        name: "Claude Opus".to_string(),
        provider: "anthropic".to_string(),
        current: true,
        scoped: false,
    }]);
    sel.handle(&ch('o'), &SelectKeymap::default());
    let rows = rows_of(&mut sel, 70);
    let (_, row) = prompt_row(&rows, "> ");
    assert!(
        row.starts_with("> o"),
        "prompt at column 0, then the query: {row:?}"
    );
    let joined = rows.join("\n");
    assert!(
        !joined.contains('\u{258f}'),
        "U+258F appears in no pi TUI source:\n{joined}"
    );
}

/// `/resume` — `session-selector.ts:418` splices `Input.render`'s own lines in unmodified. cyrup
/// drew an accent `" > "`.
#[test]
fn session_selector_search_box_uses_the_shared_prompt() {
    let mut sel = SessionSelector::new(vec![SessionRow {
        path: "/s/a.jsonl".to_string(),
        label: "Build pipeline".to_string(),
        name: Some("Build pipeline".to_string()),
        desc: Some("3 msgs".to_string()),
        search_text: "build pipeline".to_string(),
        recency: 1,
    }]);
    sel.handle(&ch('b'), &SelectKeymap::default());
    let rows = rows_of(&mut sel, 70);
    let (_, row) = prompt_row(&rows, "> ");
    assert!(
        row.starts_with("> b"),
        "prompt at column 0, then the query: {row:?}"
    );
}

/// `/tree`'s rename box — `LabelInput.render` (`tree-selector.ts:1297-1310`) is the one place
/// upstream prefixes anything: a literal two-space `indent` (`:1299`) in FRONT of
/// `input.render(...)` (`:1302`), so the row reads `"  " + "> " + value`. cyrup drew the indent and
/// dropped the prompt entirely.
#[test]
fn tree_rename_box_indents_then_uses_the_shared_prompt() {
    let mut sel = TreeSelector::new(vec![TreeNode::message("root", 0, "initial prompt")]);
    // `shift+l` (`app.tree.editLabel`, `keybindings.ts:127-130` at v0.83.0) opens the inline label
    // editor; a bare letter would be swallowed by the tree's text search instead
    // (`tree-selector.ts:1093-1100`).
    let shift_l = KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT);
    assert_eq!(
        sel.handle(&shift_l, &SelectKeymap::default()),
        SelectorOutcome::Redraw
    );
    sel.handle(&ch('x'), &SelectKeymap::default());
    let rows = rows_of(&mut sel, 80);
    let (_, row) = prompt_row(&rows, "  > ");
    assert!(
        row.starts_with("  > x"),
        "`indent` + prompt + value (`:1299,1302`): {row:?}"
    );
}

/// The prompt carries no colour of its own — `Input.render` never calls `theme.fg` (`input.ts:
/// 379-446`). Asserted on `/model`, whose old prompt was accent-coloured.
#[test]
fn the_prompt_is_unstyled() {
    let mut sel = ModelSelector::new(vec![ModelEntry {
        id: "gpt-5.1".to_string(),
        name: "GPT".to_string(),
        provider: "openai".to_string(),
        current: true,
        scoped: false,
    }]);
    let theme = UiTheme::dark();
    let mut terminal = Terminal::new(TestBackend::new(70, 24)).unwrap();
    terminal
        .draw(|f| sel.render(f, Rect::new(0, 0, 70, 24), &theme))
        .unwrap();
    let buf = terminal.backend().buffer();
    let y = (0..buf.area.height)
        .find(|y| buf.cell((0, *y)).unwrap().symbol() == ">")
        .expect("prompt row");
    assert_eq!(
        buf.cell((0, y)).unwrap().fg,
        theme.base_style().fg.unwrap(),
        "`>` is unstyled"
    );
    assert_ne!(
        theme.base_style().fg,
        theme.accent_style().fg,
        "base and accent must differ or this test proves nothing"
    );
}

/// `/login`'s manual-input + prompt dialogs — `LoginDialogComponent` adds its `Input` to
/// `contentContainer` as a bare child (`login-dialog.ts:140`, `:160`), with no `Text` wrapper, so
/// the row is `Input.render`'s unstyled `"> "` at column 0 like every other search box. cyrup drew
/// an accent `" > "`: one column in, and coloured. This row of the S31 table had no test.
#[test]
fn login_dialog_input_uses_the_shared_prompt_unstyled() {
    let mut dialog = LoginDialog::new("Anthropic", &SelectKeymap::default());
    dialog.show_manual_input("Paste the authorization code");
    for c in "abc".chars() {
        dialog.handle(&ch(c), &SelectKeymap::default());
    }
    let theme = UiTheme::dark();
    let mut terminal = Terminal::new(TestBackend::new(70, 24)).unwrap();
    terminal
        .draw(|f| dialog.render(f, Rect::new(0, 0, 70, 24), &theme))
        .unwrap();
    let buf = terminal.backend().buffer();
    let rows: Vec<String> = (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf.cell((x, y)).unwrap().symbol())
                .collect::<String>()
        })
        .collect();
    let (y, row) = prompt_row(&rows, "> ");
    assert!(
        row.starts_with("> abc"),
        "prompt at column 0, then the value: {row:?}"
    );
    assert_eq!(
        buf.cell((0, y as u16)).unwrap().fg,
        theme.base_style().fg.unwrap(),
        "`>` is unstyled (`input.ts:379-446` never calls `theme.fg`)"
    );
    assert_ne!(
        theme.base_style().fg,
        theme.accent_style().fg,
        "or this proves nothing"
    );
}
