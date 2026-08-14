//! Presentation-fidelity tests for the selection layer against pi v0.84.1 — TUI-FIDELITY SYS-4
//! (`selectedBg` applied inverted) plus S25/S26/S27 (`SelectList` width geometry), S3 (the
//! `ListSelector` hint row), S28 (the `ListSelector` row inset), S10 (`/resume`'s cursor glyph),
//! S24 (`/tree`'s fold state), S37 (`/tree`'s invented `◀ selected` marker), S38
//! (`normalizeToSingleLine`) and S39 (the dead `selected_bg_style`).
//!
//! The load-bearing upstream facts, each verified with `git -C pi show v0.84.1:<path>`:
//!
//! * `packages/tui/src/components/select-list.ts` contains **no** `theme.bg(...)` call anywhere;
//!   `getSelectListTheme()` (`coding-agent/src/modes/interactive/theme/theme.ts:1291-1298`) defines
//!   `selectedText` as a bare `theme.fg("accent", text)`.
//! * `git grep selectedBg v0.84.1 -- packages/*/src` finds exactly two component call sites:
//!   `tree-selector.ts:751-752` and `session-selector.ts:507`.
//! * `select-list.ts:156` (`// -2 for safety`) and `:169` both reserve two right-hand columns;
//!   `:180-184` reduces over `this.filteredItems` with `+ PRIMARY_COLUMN_GAP` folded in; `:151`
//!   truncates the label to `effectivePrimaryColumnWidth - PRIMARY_COLUMN_GAP`; `:98` normalizes
//!   the description to a single line before `renderItem` sees it.
//! * `extension-selector.ts:63-73` composes the hint row and `:87` insets each row by one column —
//!   **both belong to that component**, not to `SelectList`. `thinking-selector.ts:42-69` has
//!   neither.
//! * `session-selector.ts:476` uses U+203A `› `. `tree-selector.ts:722` puts the fold state in the
//!   CONNECTOR (`isFolded ? "⊞" : foldable ? "⊟" : "─"`, dim); `:734`'s accent `⊞ ` marker is the
//!   connector-less fallback, NOT the only site — the audit's S24 row read that guard backwards.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{
    ColumnLayout, ListSelector, SelectItem, SelectKeymap, SelectList, Selector, SelectorKind,
    SessionRow, SessionSelector, TreeKind, TreeNode, TreeSelector, UiTheme,
};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::Terminal;

// ---------------------------------------------------------------------------------------------
// helpers

/// The plain text of a line.
fn text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// The **column** (char) index of `needle` in `haystack` — `str::find` returns a BYTE offset, and
/// the `→ `/`› ` cursors are 3 bytes wide, which silently shifts any column assertion by 2.
fn col_of(haystack: &str, needle: &str) -> Option<usize> {
    let byte = haystack.find(needle)?;
    Some(haystack.get(..byte)?.chars().count())
}

/// The theme's `selectedBg` colour. S39: this used to read it through `selected_bg_style()`, whose
/// only remaining caller was this line — after the SYS-4 swap it had no callers in `src/` at all,
/// and being `pub` nothing would have flagged it. That accessor is deleted; the colour is resolved
/// through `selected_bg_over`, which is the form both real fill sites use.
fn selected_bg(theme: &UiTheme) -> Option<ratatui::style::Color> {
    theme.selected_bg_over(ratatui::style::Style::default()).bg
}

fn render_selector(sel: &mut dyn Selector, w: u16, h: u16) -> Terminal<TestBackend> {
    let theme = UiTheme::dark();
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| sel.render(f, Rect::new(0, 0, w, h), &theme)).unwrap();
    terminal
}

fn screen(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// The row index (0-based) whose text contains `needle`.
fn row_of(terminal: &Terminal<TestBackend>, needle: &str) -> u16 {
    let s = screen(terminal);
    for (y, line) in s.lines().enumerate() {
        if line.contains(needle) {
            return y as u16;
        }
    }
    panic!("row containing {needle:?} not found in:\n{s}");
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// `app.tree.foldOrUp` — `["alt+left","ctrl+left"]` upstream (`keybindings.ts:119-122`, v0.83.0).
/// A bare letter is NOT a tree command: `handleInput`'s final `else`
/// (`tree-selector.ts:1093-1100`) appends any printable key to the tree's text search.
fn tree_fold_key() -> KeyEvent {
    KeyEvent::new(KeyCode::Left, KeyModifiers::ALT)
}

/// `app.tree.toggleLabelTimestamp` — `shift+t` upstream (`keybindings.ts:131-134`, v0.83.0),
/// delivered by the terminal as the shifted letter.
fn tree_toggle_label_time_key() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('T'), KeyModifiers::SHIFT)
}

/// A `ui.select` dialog — one of the four kinds backed by `ExtensionSelectorComponent`, i.e. one of
/// the ones that DOES get a hint row and a one-column inset.
fn extension_select_selector(keymap: &SelectKeymap) -> ListSelector {
    let rows = vec![
        ("a".to_string(), "Alpha".to_string(), None),
        ("b".to_string(), "Beta".to_string(), None),
    ];
    ListSelector::prompt("Pick one".to_string(), rows, 0)
        .with_upstream_chrome(SelectorKind::ExtensionSelect, keymap)
}

fn items(n: usize, label_len: usize) -> Vec<SelectItem> {
    (0..n)
        .map(|i| {
            SelectItem::new(
                format!("{}{}", "x".repeat(label_len.saturating_sub(1)), i),
                Some(format!("description of row {i} which is quite long indeed")),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// SYS-4 / S1 — SelectList must NOT fill a selection background

/// **S1.** `select-list.ts` never calls `theme.bg(...)`; the selected row is distinguished purely by
/// the `→ ` cursor and the `accent` foreground `selectedText` applies (`:160-162`,
/// `theme.ts:1293-1294`).
///
/// FAILS before the fix: every span of the selected row carried the `selectedBg` fill, i.e.
/// `bg == Some(#3a3a4a)`, on eight dialogs pi leaves unfilled.
#[test]
fn select_list_never_fills_a_selection_background() {
    let theme = UiTheme::dark();
    let fill = selected_bg(&theme).expect("dark.json defines selectedBg");
    let mut list = SelectList::new(items(4, 8), ColumnLayout::SLASH);
    list.set_selected(1);

    for width in [30u16, 60, 120] {
        for line in list.lines(width, &theme) {
            for span in &line.spans {
                assert_ne!(
                    span.style.bg,
                    Some(fill),
                    "SelectList painted `selectedBg` at width {width} — upstream's select-list.ts \
                     has no theme.bg() call at all"
                );
            }
        }
    }
}

/// **S1, the other half.** Removing the bar must not make the selection invisible: pi's selected row
/// is `accent` foreground across prefix, label and description, where an unselected row is base +
/// `muted` description.
///
/// This is the guard that keeps a half-done SYS-4 swap from shipping.
#[test]
fn select_list_selection_stays_visible_through_accent_foreground() {
    let theme = UiTheme::dark();
    let accent = theme.accent_style().fg.expect("accent role");
    let mut list = SelectList::new(items(4, 8), ColumnLayout::SLASH);
    list.set_selected(2);
    let lines = list.lines(80, &theme);

    for span in &lines[2].spans {
        assert_eq!(
            span.style.fg,
            Some(accent),
            "selected row span {:?} is not accent — selection would be invisible",
            span.content
        );
    }
    // …and the neighbours are not.
    assert_ne!(lines[1].spans[0].style.fg, Some(accent), "unselected row must not be accent");
    assert!(text(&lines[2]).starts_with("→ "), "selected row keeps the `→ ` cursor");
    assert!(text(&lines[1]).starts_with("  "), "unselected rows keep the two-space prefix");
}

// ---------------------------------------------------------------------------------------------
// S25 / S26 / S27 — SelectList width geometry

/// **S25.** `select-list.ts:169` `const maxWidth = width - prefixWidth - 2;` — the single-column arm
/// always leaves two right-hand columns free.
///
/// FAILS before the fix: `width.saturating_sub(prefix_w)` ran the label flush into the last column.
#[test]
fn select_list_single_column_reserves_the_right_safety_gutter() {
    let theme = UiTheme::dark();
    // width <= 40 forces the single-column arm regardless of descriptions (`:149`).
    let list = SelectList::new(vec![SelectItem::label("y".repeat(60))], ColumnLayout::SLASH);
    let width = 30usize;
    let line = &list.lines(width as u16, &theme)[0];
    assert_eq!(
        text(line).chars().count(),
        width - 2,
        "row must stop 2 columns short of the edge (prefix 2 + label {})",
        width - 4
    );
}

/// **S25, two-column arm.** `select-list.ts:156` `const remainingWidth = width - descriptionStart -
/// 2; // -2 for safety`.
#[test]
fn select_list_two_column_reserves_the_right_safety_gutter() {
    let theme = UiTheme::dark();
    let list = SelectList::new(
        vec![SelectItem::new("cmd", Some("z".repeat(200)))],
        ColumnLayout { primary_min: 12, primary_max: 32 },
    );
    let width = 60usize;
    let line = &list.lines(width as u16, &theme)[0];
    assert!(
        text(line).chars().count() <= width - 2,
        "two-column row ran into the safety gutter: {:?}",
        text(line)
    );
}

/// **S26(a).** `select-list.ts:180-184` reduces over **`this.filteredItems`** — the whole list — so
/// the description column is fixed no matter where the window sits. cyrup measured only the visible
/// window, so scrolling past a longer label shifted the description column sideways.
///
/// FAILS before the fix: the column jumps as soon as the long row enters the window.
#[test]
fn select_list_primary_column_is_measured_over_every_row_not_the_window() {
    let theme = UiTheme::dark();
    // Six rows; the long label lives at index 5, outside the initial 3-row window.
    let mut rows: Vec<SelectItem> = (0..5)
        .map(|i| SelectItem::new(format!("s{i}"), Some("a description".to_string())))
        .collect();
    rows.push(SelectItem::new("a-very-long-command-name", Some("a description".to_string())));
    let mut list = SelectList::new(rows, ColumnLayout::DEFAULT);
    list.set_max_visible(3);

    let desc_col = |list: &SelectList| -> usize {
        let lines = list.lines(100, &theme);
        let row = lines.iter().find(|l| text(l).contains("a description")).expect("a visible row");
        col_of(&text(row), "a description").expect("description present")
    };

    list.set_selected(0);
    let top = desc_col(&list);
    list.set_selected(5);
    let bottom = desc_col(&list);
    assert_eq!(
        top, bottom,
        "the description column moved when the window scrolled ({top} -> {bottom})"
    );
}

/// **S26(b) + S27.** When the clamp binds, upstream's column *includes* the gap (`:181`) and the
/// label is truncated to `column - GAP` (`:151`), so at the 32-column `SLASH` cap pi draws a 30-char
/// label and starts the description at `prefix(2) + 32 = 34`.
///
/// FAILS before the fix: cyrup drew a 32-char label and started the description at column 36.
#[test]
fn select_list_folds_the_gap_into_the_clamped_primary_column() {
    let theme = UiTheme::dark();
    let long = "L".repeat(50);
    let list = SelectList::new(
        vec![SelectItem::new(long.clone(), Some("DESCRIPTION".to_string()))],
        ColumnLayout::SLASH, // primary_max = 32
    );
    let line = &list.lines(120, &theme)[0];
    let rendered = text(line);
    let label_run = rendered.chars().filter(|c| *c == 'L').count();
    assert_eq!(label_run, 30, "label must be truncated to 32 - PRIMARY_COLUMN_GAP: {rendered:?}");
    assert_eq!(
        col_of(&rendered, "DESCRIPTION"),
        Some(34),
        "description must start at prefix(2) + column(32): {rendered:?}"
    );
}

/// **S25/S26/S27 arithmetic safety.** Every width budget upstream computes by subtraction
/// (`width - prefixWidth - 4`, `width - descriptionStart - 2`) goes negative on a narrow terminal;
/// in Rust that is an underflow panic unless it saturates. Exercise the degenerate widths and the
/// boundary of the `width > 40` two-column gate.
#[test]
fn select_list_survives_degenerate_widths() {
    let theme = UiTheme::dark();
    let mut list = SelectList::new(items(6, 40), ColumnLayout::SLASH);
    list.set_selected(3);
    for width in [0u16, 1, 2, 3, 5, 6, 7, 39, 40, 41, 42, 45] {
        let lines = list.lines(width, &theme);
        for line in &lines {
            assert!(
                text(line).chars().count() <= usize::from(width).max(2),
                "row overflowed width {width}: {:?}",
                text(line)
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// S3 / S28 — the ListSelector envelope

/// **S3.** `extension-selector.ts:63-73` puts a hint row above the bottom border:
/// `rawKeyHint("↑↓","navigate") + "  " + keyHint("tui.select.confirm","select") + "  " +
/// keyHint("tui.select.cancel","cancel")`. `keyText` joins **all** bound keys with `/`
/// (`keybinding-hints.ts:29-36`), so the stock cancel hint reads `escape/ctrl+c`.
///
/// The row is **opt-in**: only the four kinds backed by `ExtensionSelectorComponent` get it (see
/// `SelectorKind::draws_hint_row`). `ExtensionSelect` is one of them.
#[test]
fn list_selector_draws_pis_keyboard_hint_row() {
    let mut sel = extension_select_selector(&SelectKeymap::default());
    let terminal = render_selector(&mut sel, 80, 14);
    let s = screen(&terminal);
    assert!(s.contains("↑↓ navigate"), "navigate hint missing:\n{s}");
    assert!(s.contains("enter select"), "select hint missing:\n{s}");
    assert!(s.contains("escape/ctrl+c cancel"), "cancel hint missing (all bound keys):\n{s}");

    // Two-tone per pair (`keybinding-hints.ts:42-44`): dim key, muted description.
    let theme = UiTheme::dark();
    let hint_y = row_of(&terminal, "navigate");
    let buf = terminal.backend().buffer();
    let key_cell = &buf[(1, hint_y)]; // first column of `↑↓`, after the 1-column inset
    assert_eq!(key_cell.fg, theme.dim_style().fg.unwrap(), "hint key must use the `dim` token");
    let hint_row = screen(&terminal).lines().nth(hint_y as usize).unwrap().to_string();
    let desc_x = col_of(&hint_row, "navigate").unwrap();
    assert_eq!(
        buf[(desc_x as u16, hint_y)].fg,
        theme.muted_style().fg.unwrap(),
        "hint description must use the `muted` token"
    );
}

/// **S3, live keys.** The hint must name the user's bindings, not a frozen literal.
#[test]
fn list_selector_hint_row_follows_a_rebound_cancel_key() {
    use crate::{Key, SelectAction};
    let mut km = SelectKeymap::default();
    km.set_action(SelectAction::Cancel, vec![Key::ctrl('q')]);
    let mut sel = extension_select_selector(&km);
    let s = screen(&render_selector(&mut sel, 80, 14));
    assert!(s.contains("ctrl+q cancel"), "hint did not follow the rebind:\n{s}");
    assert!(!s.contains("escape/ctrl+c cancel"), "stale default cancel hint:\n{s}");
}

/// **S28.** `extension-selector.ts:87` wraps every row in `new Text(text, 1, 0)`, so the `→ `/`  `
/// prefix starts at column 1 — in line with the title at `:47`.
#[test]
fn list_selector_rows_are_inset_one_column_where_pi_insets_them() {
    let mut sel = extension_select_selector(&SelectKeymap::default());
    let terminal = render_selector(&mut sel, 80, 16);
    let cursor_y = row_of(&terminal, "→ ");
    let buf = terminal.backend().buffer();
    assert_eq!(buf[(0, cursor_y)].symbol(), " ", "column 0 must be the Text left margin");
    assert_eq!(buf[(1, cursor_y)].symbol(), "→", "the cursor glyph belongs at column 1");
}

// ---------------------------------------------------------------------------------------------
// S10 / S2 — /resume

fn session_rows() -> Vec<SessionRow> {
    vec![
        SessionRow {
            path: "/s/a.jsonl".into(),
            label: "Build pipeline".into(),
            name: Some("Build pipeline".into()),
            desc: Some("3 msgs".into()),
            search_text: "a build pipeline".into(),
            recency: 3,
        },
        SessionRow {
            path: "/s/b.jsonl".into(),
            label: "Fix the footer".into(),
            name: None,
            desc: Some("9 msgs".into()),
            search_text: "b fix footer".into(),
            recency: 2,
        },
    ]
}

/// **S10.** `session-selector.ts:476` `const cursor = isSelected ? theme.fg("accent", "› ") : "  ";`
/// — U+203A, not the U+2192 `→ ` `SelectList` uses.
///
/// FAILS before the fix: `/resume` drew `→ `.
#[test]
fn session_selector_uses_pis_single_angle_quote_cursor() {
    let mut sel = SessionSelector::new(session_rows());
    let s = screen(&render_selector(&mut sel, 80, 16));
    assert!(s.contains('\u{203a}'), "U+203A `›` cursor missing from /resume:\n{s}");
    assert!(!s.contains('\u{2192}'), "/resume still draws SelectList's `→` cursor:\n{s}");
}

/// **S2 / SYS-4.** `session-selector.ts:506-508` `if (isSelected) line = theme.bg("selectedBg",
/// line);` over the whole row. This is one of only two places upstream fills a selection background,
/// and cyrup filled neither.
///
/// FAILS before the fix: no cell on the selected row carried `selectedBg`.
#[test]
fn session_selector_fills_the_selected_row_with_selected_bg() {
    let theme = UiTheme::dark();
    let fill = selected_bg(&theme).expect("dark.json defines selectedBg");
    let mut sel = SessionSelector::new(session_rows());
    let terminal = render_selector(&mut sel, 60, 16);
    let sel_y = row_of(&terminal, "\u{203a} ");
    let buf = terminal.backend().buffer();
    for x in 0..60u16 {
        assert_eq!(
            buf[(x, sel_y)].bg,
            fill,
            "selected /resume row is not filled at column {x} — pi fills the whole row width"
        );
    }
    // The row below it is not filled.
    let other_y = row_of(&terminal, "Fix the footer");
    assert_ne!(buf[(2, other_y)].bg, fill, "an unselected row must not be filled");
}

// ---------------------------------------------------------------------------------------------
// S2 / S24 — /tree

fn tree_nodes() -> Vec<TreeNode> {
    let mut foldable = TreeNode::message("m", 1, "model -> opus");
    foldable.kind = TreeKind::ModelChange;
    foldable.foldable = true;
    vec![
        TreeNode::message("root", 0, "initial prompt"),
        foldable,
        TreeNode::message("stream", 2, "wire up streaming"),
    ]
}

/// **S2 / SYS-4.** `tree-selector.ts:750-753` fills gutter and body with `selectedBg`, laid **over**
/// the per-span foregrounds. It is the second of upstream's only two fill sites.
///
/// FAILS before the fix: the `/tree` selected row was accent+BOLD text with no background.
#[test]
fn tree_selector_fills_the_selected_row_with_selected_bg() {
    let theme = UiTheme::dark();
    let fill = selected_bg(&theme).expect("dark.json defines selectedBg");
    let sel = TreeSelector::new(tree_nodes());
    let rows = sel.rows(80, &theme);
    for span in &rows[0].spans {
        assert_eq!(
            span.style.bg,
            Some(fill),
            "selected /tree span {:?} is not filled",
            span.content
        );
    }
    // The fill must not bleed onto the accent foreground it is laid over.
    assert!(
        rows[0].spans.iter().any(|s| s.style.fg == theme.accent_style().fg),
        "the fill replaced the row's foreground instead of layering over it"
    );
    for span in &rows[1].spans {
        assert_ne!(span.style.bg, Some(fill), "an unselected /tree row must not be filled");
    }
}

/// **S24, corrected.** The fold state lives in the **connector**, not in a separate marker:
/// `tree-selector.ts:721-722` writes `isFolded ? "⊞" : foldable ? "⊟" : "─"` into `posInLevel === 1`
/// of the node's own `├─ `/`└─ `. The separate `foldMarker` at `:734` is guarded
/// `isFolded && !showsFoldInConnector`, i.e. it is the FALLBACK for a node that has no connector to
/// carry the state — not evidence that pi never draws `⊟`.
///
/// The connector (including the fold cell) is styled `theme.fg("dim", prefix)` at `:746`.
///
/// FAILS under the revert of the connector fold cell: the connector goes back to a plain `└─ ` and
/// `└⊟ ` is nowhere on screen.
#[test]
fn tree_selector_draws_the_fold_state_inside_the_connector() {
    let theme = UiTheme::dark();
    let mut sel = TreeSelector::new(tree_nodes());
    // `m` is depth-1, foldable, expanded and the last child of the root -> `└⊟ `.
    let expanded: String = sel.rows(80, &theme).iter().map(text).collect();
    assert!(
        expanded.contains("└⊟ "),
        "expanded foldable node must render `└⊟ ` in its connector: {expanded:?}"
    );
    assert!(!expanded.contains('\u{229e}'), "`⊞` drawn with nothing folded: {expanded:?}");
    // The leaf under it is not foldable, so its connector keeps the plain `─`.
    assert!(expanded.contains("└─ "), "a non-foldable node keeps `└─ `: {expanded:?}");

    // Fold it and the SAME cell flips to `⊞` — the row does not grow or shift.
    let km = SelectKeymap::default();
    sel.handle(&key(KeyCode::Down), &km); // -> the foldable node
    sel.handle(&tree_fold_key(), &km); // fold (`app.tree.foldOrUp`, `tree-selector.ts:1002-1009`)
    let rows = sel.rows(80, &theme);
    let folded: String = rows.iter().map(text).collect();
    assert!(folded.contains("└⊞ "), "folded node must render `└⊞ `: {folded:?}");
    assert!(!folded.contains('\u{229f}'), "`⊟` still drawn after folding: {folded:?}");
    let connector = rows
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains('\u{229e}'))
        .expect("the folded connector span");
    assert_eq!(
        connector.style.fg,
        theme.dim_style().fg,
        "the connector (fold cell included) is `theme.fg(\"dim\", prefix)` at :746"
    );
}

/// **S24, the fallback half.** `:733-734` — a node with NO connector cannot carry the fold state in
/// one, so a folded root gets the separate `theme.fg("accent", "⊞ ")` marker instead. An expanded
/// connector-less node gets nothing at all: there is no `⊟` fallback upstream.
#[test]
fn tree_selector_fold_marker_is_the_connectorless_fallback() {
    let theme = UiTheme::dark();
    let mut root = TreeNode::message("root", 0, "initial prompt");
    root.foldable = true;
    let child = TreeNode::message("kid", 1, "a child");
    let mut sel = TreeSelector::new(vec![root, child]);

    // Expanded depth-0 root: no connector AND no marker.
    let rows = sel.rows(80, &theme);
    let row0 = text(&rows[0]);
    assert!(!row0.contains('\u{229f}'), "no `⊟` fallback exists upstream: {row0:?}");
    assert!(!row0.contains('\u{229e}'), "`⊞` drawn with nothing folded: {row0:?}");

    // Fold it: now the accent `⊞ ` fallback appears, because there is no connector to hold it.
    let km = SelectKeymap::default();
    sel.handle(&tree_fold_key(), &km);
    let rows = sel.rows(80, &theme);
    let marker = rows[0]
        .spans
        .iter()
        .find(|s| s.content.contains('\u{229e}'))
        .expect("a folded connector-less root must draw the `⊞ ` fallback");
    assert_eq!(
        marker.content, "⊞ ",
        "the fallback is the two-character `⊞ ` of `:734`, not a connector cell"
    );
    assert_eq!(
        marker.style.fg,
        theme.accent_style().fg,
        "the fallback marker is `theme.fg(\"accent\", …)` upstream, not dim"
    );
}

/// **S37.** `/tree` used to append a right-aligned `◀ selected` marker to the highlighted row.
/// `git grep '◀' v0.84.1 -- packages/` finds nothing in pi, and `renderHorizontalViewport`
/// (`tree-selector.ts:85-91`) emits `row.gutter + row.body` truncated to `width` with **no** right
/// padding — an upstream row is exactly as wide as its content. The selection is indicated by the
/// `› ` cursor of `:689` plus the `selectedBg` fill of `:750-753`, and by nothing else.
#[test]
fn tree_selector_marks_the_selection_pis_way_not_with_an_invented_marker() {
    let theme = UiTheme::dark();
    let sel = TreeSelector::new(tree_nodes());
    let rows = sel.rows(80, &theme);
    let selected = text(&rows[0]);
    assert!(!selected.contains('◀'), "cyrup-only `◀ selected` marker still drawn: {selected:?}");
    assert!(!selected.contains("selected"), "cyrup-only marker text still drawn: {selected:?}");
    // `:689` — U+203A on the selected row, two spaces on the others.
    assert!(selected.starts_with("\u{203a} "), "selected row must open with `› `: {selected:?}");
    assert!(text(&rows[1]).starts_with("  "), "unselected rows open with two spaces");
    // The row is content-wide, not padded out to `width`.
    assert!(
        selected.chars().count() < 80,
        "the row was padded out to the full width: {selected:?}"
    );
}

/// Selection styling on the two filled components must survive a theme that omits `selectedBg`
/// (upstream's `theme.bg()` is a no-op when the role is absent) — no panic, no lost foreground.
#[test]
fn tree_selection_without_a_selected_bg_role_keeps_its_foreground() {
    let mut theme = UiTheme::dark();
    theme.roles.remove("selectedBg");
    let sel = TreeSelector::new(tree_nodes());
    let rows = sel.rows(80, &theme);
    assert!(
        rows[0].spans.iter().any(|s| s.style.fg == theme.accent_style().fg),
        "selected row lost its accent foreground when selectedBg is undefined"
    );
}

// ---------------------------------------------------------------------------------------------
// S3 / S28, the negative half — the chrome is OPT-IN

/// **S3, corrected.** The hint row belongs to the *component*, not to pi's shared list engine.
/// `ThinkingSelectorComponent` is 75 lines — `DynamicBorder` + `SelectList` + `DynamicBorder`
/// (`thinking-selector.ts:42-69`) — with no `keyHint` call anywhere in the file, and the same holds
/// for `show-images-selector.ts:25-44`, `theme-selector.ts:35-61`, `oauth-selector.ts` and
/// `user-message-selector.ts`. Making the row a property of `ListSelector` gave six or seven
/// dialogs a row pi never draws.
///
/// FAILS under the revert that puts the row back on every `ListSelector`.
#[test]
fn list_selector_draws_no_hint_row_where_pi_draws_none() {
    for mut sel in [
        ListSelector::thinking("medium"),
        ListSelector::show_images(true),
        ListSelector::theme("dark"),
    ] {
        let s = screen(&render_selector(&mut sel, 80, 16));
        assert!(!s.contains("navigate"), "invented navigate hint:\n{s}");
        assert!(!s.contains("cancel"), "invented cancel hint:\n{s}");
        assert!(!s.contains("select"), "invented select hint:\n{s}");
    }
}

/// The kind→chrome table itself, read straight off the upstream components.
#[test]
fn only_pis_own_components_opt_into_the_dialog_chrome() {
    for kind in [
        SelectorKind::ExtensionSelect,
        SelectorKind::ExtensionConfirm,
        SelectorKind::BranchSummary,
        SelectorKind::LoginAuthType,
    ] {
        assert!(kind.draws_hint_row(), "{kind:?} is an ExtensionSelectorComponent — it hints");
        assert!(kind.insets_rows(), "{kind:?} is an ExtensionSelectorComponent — it insets");
    }
    // `OAuthSelectorComponent` insets (`oauth-selector.ts:144`) but has no `keyHint` at all.
    for kind in [SelectorKind::Login, SelectorKind::Logout] {
        assert!(!kind.draws_hint_row(), "{kind:?}: oauth-selector.ts contains no keyHint");
        assert!(kind.insets_rows(), "{kind:?}: oauth-selector.ts:144 is TruncatedText(line, 1, 0)");
    }
    // Neither for the components that add a bare `SelectList` / list child.
    for kind in [
        SelectorKind::Thinking,
        SelectorKind::ShowImages,
        SelectorKind::Theme,
        SelectorKind::UserMessage,
    ] {
        assert!(!kind.draws_hint_row(), "{kind:?} has no hint row upstream");
        assert!(!kind.insets_rows(), "{kind:?} adds its list unwrapped upstream");
    }
}

/// **S28, corrected — the width the gate sees.** The inset is not free: `Text`'s content width is
/// `max(1, width - paddingX * 2)` (`text.ts:64`), so insetting narrows the list by **two** columns
/// and moves `select-list.ts:149`'s `width > 40` two-column gate with it.
///
/// `ThinkingSelectorComponent` hands its `SelectList` the container's FULL width
/// (`thinking-selector.ts:66`), so at a 42-column dialog pi is on the two-column side of that gate
/// and the level descriptions render. Insetting unconditionally pushed it to 40 and silently
/// dropped every description at exactly the widths where they matter most.
///
/// FAILS under the revert that insets every `ListSelector`.
#[test]
fn the_inset_does_not_move_the_two_column_gate_on_uninset_dialogs() {
    let mut sel = ListSelector::thinking("medium");
    let s = screen(&render_selector(&mut sel, 42, 16));
    assert!(
        s.contains("Moderate reasoning"),
        "at width 42 pi's SelectList sees 42 > 40 and draws descriptions:\n{s}"
    );
    // The rows also start at column 0, not 1.
    let terminal = render_selector(&mut ListSelector::thinking("medium"), 42, 16);
    let cursor_y = row_of(&terminal, "→ ");
    assert_eq!(
        terminal.backend().buffer()[(0, cursor_y)].symbol(),
        "→",
        "an uninset dialog's cursor belongs at column 0"
    );
}

/// …and the inset kinds really do lay out two columns narrower (`text.ts:64`), which is what makes
/// the leading margin a margin rather than a one-column overdraw.
#[test]
fn an_inset_dialog_lays_out_two_columns_narrower() {
    let rows: Vec<(String, String, Option<String>)> =
        vec![("v".to_string(), "Z".repeat(200), None)];
    let mut sel = ListSelector::prompt("t".to_string(), rows, 0)
        .with_upstream_chrome(SelectorKind::ExtensionSelect, &SelectKeymap::default());
    let terminal = render_selector(&mut sel, 60, 16);
    let row = screen(&terminal).lines().find(|l| l.contains('Z')).unwrap().to_string();
    // 1 (margin) + 2 (cursor) + label, where the label budget is `(60 - 2) - 2 - 2` (`:169`).
    assert_eq!(
        row.trim_end().chars().count(),
        1 + 2 + (60 - 2 - 2 - 2),
        "inset row geometry: {row:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// S26(b) — a test that bites when the clamp does NOT bind

/// **S26(b), unclamped.** `select-list.ts:181` folds `PRIMARY_COLUMN_GAP` into the measured column
/// (`visibleWidth(getDisplayValue(item)) + PRIMARY_COLUMN_GAP`), and `:151` then truncates the label
/// to `effectivePrimaryColumnWidth - PRIMARY_COLUMN_GAP`. Those two cancel out only if the `+ GAP`
/// is there.
///
/// Every existing S25-S27 test uses a label long enough for the 32-column cap to bind, where the
/// `+ GAP` and its absence differ only in where the description starts — so all 15 stayed green
/// when it was reverted. Here the clamp does NOT bind (a 15-char label inside `SLASH`'s `[12, 32]`,
/// on a 120-column terminal), and the `+ GAP` is the whole difference between a label rendered in
/// full and one silently truncated by two characters.
///
/// FAILS under `.map(|i| i.label.chars().count())`: the column measures 15, the label budget is
/// 13, and `chart-of-accounts` comes out as `chart-of-acco`.
#[test]
fn select_list_does_not_truncate_a_label_the_clamp_never_touched() {
    let theme = UiTheme::dark();
    let label = "fifteen-chars-x"; // exactly 15 -> 15 + GAP = 17, strictly inside [12, 32]
    assert_eq!(label.chars().count(), 15);
    let list = SelectList::new(
        vec![SelectItem::new(label, Some("DESCRIPTION".to_string()))],
        ColumnLayout::SLASH,
    );
    let rendered = text(&list.lines(120, &theme)[0]);
    assert!(
        rendered.contains(label),
        "a label the clamp never touched must render in full: {rendered:?}"
    );
    // `descriptionStart = prefixWidth(2) + truncatedValueWidth(15) + spacing(max(1, 17-15)=2)`.
    assert_eq!(
        col_of(&rendered, "DESCRIPTION"),
        Some(19),
        "description must start at prefix(2) + label(15) + spacing(2): {rendered:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// S38 — normalizeToSingleLine

/// **S38.** `select-list.ts:98` normalizes the description *before* `renderItem` ever sees it:
/// `normalizeToSingleLine` (`:9`) is `text.replace(/[\r\n]+/g, " ").trim()`. cyrup passed the raw
/// string, so a description containing a newline broke the row it was measured as one line of.
///
/// FAILS under the revert to `item.description.as_deref()`: the rendered row contains a `\n`.
#[test]
fn select_list_flattens_a_multiline_description() {
    let theme = UiTheme::dark();
    let list = SelectList::new(
        vec![SelectItem::new("cmd", Some("  first line\r\n\nsecond line  ".to_string()))],
        ColumnLayout::SLASH,
    );
    let rendered = text(&list.lines(100, &theme)[0]);
    assert!(!rendered.contains('\n'), "raw newline reached the row: {rendered:?}");
    assert!(!rendered.contains('\r'), "raw carriage return reached the row: {rendered:?}");
    // A RUN of breaks collapses to exactly one space (the regex is `+`), and the value is trimmed.
    assert!(
        rendered.contains("first line second line"),
        "the break run must collapse to one space and the value be trimmed: {rendered:?}"
    );
}

/// An all-whitespace description normalizes to `""`, which is falsy at `:149` — upstream then takes
/// the single-column arm. `Some("")` would not.
#[test]
fn select_list_treats_a_whitespace_only_description_as_absent() {
    let theme = UiTheme::dark();
    let list = SelectList::new(
        vec![SelectItem::new("cmd", Some("\n\n".to_string()))],
        ColumnLayout::SLASH,
    );
    let rendered = text(&list.lines(100, &theme)[0]);
    // The row is the (only, hence selected) one, so the cursor is `→ `; what matters is that no
    // description column follows it.
    assert_eq!(rendered.trim_end(), "→ cmd", "must take the single-column arm: {rendered:?}");
}

// ---------------------------------------------------------------------------------------------
// S26 / S27 — the char-vs-grapheme residual
//
// Upstream measures every `SelectList` width with `visibleWidth` and cuts with `truncateToWidth`:
//
//   * `select-list.ts:147`  `const prefixWidth = visibleWidth(prefix);`
//   * `select-list.ts:153`  `const truncatedValueWidth = visibleWidth(truncatedValue);`
//   * `select-list.ts:181`  `Math.max(widest, visibleWidth(this.getDisplayValue(item)) + PRIMARY_COLUMN_GAP)`
//   * `select-list.ts:159`  `truncateToWidth(descriptionSingleLine, remainingWidth, "")`
//   * `select-list.ts:209`/`:211`  `truncateToWidth(displayValue, maxWidth, "")`
//
// `visibleWidth` (`tui/src/utils.ts:240-295`) segments with `graphemeSegmenter` and sums
// `graphemeWidth`; `truncateToWidth` (`utils.ts:1053-1092`, non-ASCII branch `:1100-1110`) walks
// the same segmenter and only ever keeps a cluster whole. cyrup measured `chars().count()` and cut
// `chars().take(n)`, which is wrong three separate ways: a wide character measured 1 column, a
// combining mark measured 1 column instead of 0, and a multi-codepoint cluster could be cut in
// half.

/// The visible **column** index at which `needle` starts, unlike [`col_of`] which counts chars.
/// The two disagree for exactly the inputs these tests exercise.
fn vis_col_of(haystack: &str, needle: &str) -> Option<usize> {
    let byte = haystack.find(needle)?;
    Some(ratatui::text::Span::raw(haystack.get(..byte)?).width())
}

fn vis_width(s: &str) -> usize {
    ratatui::text::Span::raw(s).width()
}

/// **S26, CJK.** A label of 20 double-width characters measures **40** columns, so the 32-column
/// clamp binds (`:184`), the label is cut to `32 - GAP = 30` columns (`:151`) — 15 CJK characters —
/// and the description starts at `prefixWidth + effectivePrimaryColumnWidth = 2 + 32 = 34`
/// (`:155`).
///
/// FAILS under `chars().count()`: the label measures 20, `clamp(22, 12, 32)` does not bind, the
/// budget is 20 *chars*, the whole 40-column label survives and the description lands at column 44
/// — ten columns past where pi puts it, and past the right edge of a narrow frame.
#[test]
fn select_list_measures_a_cjk_label_in_columns_not_chars() {
    let theme = UiTheme::dark();
    let label: String = "銀".repeat(20); // 20 chars, 40 columns
    assert_eq!(label.chars().count(), 20);
    assert_eq!(vis_width(&label), 40);

    let list = SelectList::new(
        vec![SelectItem::new(label, Some("説明テキスト".to_string()))],
        ColumnLayout::SLASH,
    );
    let rendered = text(&list.lines(120, &theme)[0]);

    let kept = rendered.chars().filter(|c| *c == '銀').count();
    assert_eq!(kept, 15, "label must be cut to 30 COLUMNS = 15 CJK chars: {rendered:?}");
    assert_eq!(
        vis_col_of(&rendered, "説明テキスト"),
        Some(34),
        "description must start at prefix(2) + column(32): {rendered:?}"
    );
}

/// **S27, ZWJ cluster in a label.** `👨\u{200d}👩\u{200d}👧` is ONE extended grapheme cluster of
/// FIVE codepoints occupying TWO columns; `truncateToWidth` keeps it whole or drops it
/// (`utils.ts:1100-1110`).
///
/// The single-column arm budget is `width - prefixWidth - 2` (`:169`) = 16 columns at width 20, so
/// exactly eight of ten families survive.
///
/// FAILS under `chars().take(16)`: 16 codepoints is three whole families plus a lone `👨` — the
/// label is cut mid-cluster, the family dissolves into a stray man emoji, and only 8 of the 16
/// budgeted columns are used.
#[test]
fn select_list_never_splits_a_zwj_cluster_in_a_label() {
    let theme = UiTheme::dark();
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
    let label = family.repeat(10);
    assert_eq!(label.chars().count(), 50, "5 codepoints per family");
    assert_eq!(vis_width(&label), 20, "2 columns per family");

    // width <= 40 forces the single-column arm (`:149`), budget = 20 - 2 - 2 = 16.
    let list = SelectList::new(vec![SelectItem::label(label)], ColumnLayout::SLASH);
    let rendered = text(&list.lines(20, &theme)[0]);
    let body = rendered.strip_prefix("→ ").expect("selected row prefix");

    assert_eq!(vis_width(body), 16, "budget is width(20) - prefix(2) - 2: {rendered:?}");
    assert_eq!(
        body.matches(family).count(),
        8,
        "exactly eight whole families fit in 16 columns: {rendered:?}"
    );
    assert!(
        !body.ends_with('\u{200d}') && !body.ends_with('\u{1f468}'),
        "a cut must never leave a dangling ZWJ or a half-dissolved family: {rendered:?}"
    );
}

/// **S27, combining mark in a label.** `e` + U+0301 is one cluster of **1** column and 2
/// codepoints. The single-column budget at width 20 is 16 columns, so 16 accented `e`s survive.
///
/// FAILS under `chars().take(16)`: 16 codepoints is 8 clusters — half the label vanishes — and the
/// 16th codepoint is a bare `e`, so the accent of the last kept letter is dropped.
#[test]
fn select_list_never_splits_a_combining_mark_in_a_label() {
    let theme = UiTheme::dark();
    let label = "e\u{301}".repeat(30);
    assert_eq!(label.chars().count(), 60);
    assert_eq!(vis_width(&label), 30);

    let list = SelectList::new(vec![SelectItem::label(label)], ColumnLayout::SLASH);
    let rendered = text(&list.lines(20, &theme)[0]);
    let body = rendered.strip_prefix("→ ").expect("selected row prefix");

    assert_eq!(vis_width(body), 16, "budget is width(20) - prefix(2) - 2: {rendered:?}");
    assert_eq!(body.matches("e\u{301}").count(), 16, "16 whole clusters: {rendered:?}");
    assert!(
        !body.ends_with('e'),
        "the last kept letter must keep its accent: {rendered:?}"
    );
}

/// **S27, ZWJ cluster in a description.** `:159` cuts the description with the same
/// `truncateToWidth`. With `SLASH`'s pinned column and an ASCII `cmd` label the description starts
/// at `2 + 3 + max(1, 12 - 3) = 14`, leaving `45 - 14 - 2 = 29` columns — fourteen whole families
/// (28 columns), never fourteen and a half.
///
/// FAILS under `chars().take(29)`: 29 codepoints is five families plus `👨\u{200d}👩\u{200d}` — a
/// half-built family trailing a ZWJ, 25 of the 29 columns left blank.
#[test]
fn select_list_never_splits_a_zwj_cluster_in_a_description() {
    let theme = UiTheme::dark();
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
    let list = SelectList::new(
        vec![SelectItem::new("cmd", Some(family.repeat(20)))],
        ColumnLayout::SLASH,
    );
    let rendered = text(&list.lines(45, &theme)[0]);

    assert_eq!(vis_col_of(&rendered, family), Some(14), "description column: {rendered:?}");
    let start = rendered.find(family).expect("description present");
    let desc = rendered.get(start..).expect("suffix");

    assert_eq!(vis_width(desc), 28, "29-column budget holds 14 two-column families: {rendered:?}");
    assert_eq!(
        desc.matches(family).count(),
        14,
        "fourteen whole families, not thirteen and a half: {rendered:?}"
    );
    assert!(
        !desc.ends_with('\u{200d}') && !desc.ends_with('\u{1f468}'),
        "a cut must never leave a dangling ZWJ or a half-dissolved family: {rendered:?}"
    );
    assert!(
        vis_width(&rendered) <= 43,
        "row must stop 2 columns short of a 45-column frame: {rendered:?}"
    );
}

/// **S27, combining mark in a description.** Same 29-column description budget; 29 accented `e`s
/// are 58 codepoints.
///
/// FAILS under `chars().take(29)`: 14 clusters plus a bare `e` — 15 columns where pi draws 29, and
/// the trailing letter loses its accent.
#[test]
fn select_list_never_splits_a_combining_mark_in_a_description() {
    let theme = UiTheme::dark();
    let list = SelectList::new(
        vec![SelectItem::new("cmd", Some("e\u{301}".repeat(40)))],
        ColumnLayout::SLASH,
    );
    let rendered = text(&list.lines(45, &theme)[0]);
    assert_eq!(vis_col_of(&rendered, "e\u{301}"), Some(14), "description column: {rendered:?}");
    let start = rendered.find("e\u{301}").expect("description present");
    let desc = rendered.get(start..).expect("suffix");

    assert_eq!(vis_width(desc), 29, "description budget is 29 columns: {rendered:?}");
    assert_eq!(desc.matches("e\u{301}").count(), 29, "29 whole clusters: {rendered:?}");
    assert!(!desc.ends_with('e'), "trailing letter kept its accent: {rendered:?}");
}

// ---------------------------------------------------------------------------------------------
// S24 — the remaining two halves: per-role entry colouring, and the timestamp pad
//
// `getEntryDisplayText` (`tree-selector.ts:768-852`) never returns one uniformly-styled string.
// Verified with `git -C pi show v0.84.1:packages/coding-agent/src/modes/interactive/components/tree-selector.ts`:
//
//   :781  result = theme.fg("accent",  "user: ")      + content;
//   :786  result = theme.fg("success", "assistant: ") + textContent;
//   :793  result = theme.fg("success", "assistant: ") + theme.fg("muted", "(no content)");
//   :799  result = theme.fg("muted",   this.formatToolCall(toolCall.name, toolCall.arguments));
//   :805  result = theme.fg("dim",     `[bash]: ${normalize(bashMsg.command ?? "")}`);
//   :824  result = theme.fg("borderAccent", `[compaction: ${tokens}k tokens]`);
//   :828  result = theme.fg("warning", `[branch summary]: `) + normalize(entry.summary);
//   :831  result = theme.fg("dim", `[model: ${entry.modelId}]`);
//   :851  return isSelected ? theme.bold(result) : result;
//
// `git grep -n borderAccent v0.84.1 -- packages/*/src` finds exactly ONE component render site,
// `:824` above — so this row is also the whole of T9's `borderAccent`.

/// The foreground of a style — named so the assertions below read as colour comparisons.
fn fg_of(style: ratatui::style::Style) -> Option<ratatui::style::Color> {
    style.fg
}

/// The style of the first span of `row` whose text contains `needle`.
fn span_style_containing(row: &Line<'_>, needle: &str) -> ratatui::style::Style {
    row.spans
        .iter()
        .find(|s| s.content.contains(needle))
        .unwrap_or_else(|| panic!("no span containing {needle:?} in {:?}", text(row)))
        .style
}

fn roled_nodes() -> Vec<TreeNode> {
    // The label forms are exactly what `cyrup_session_svc::dag_display` (`session.rs:4935-4995`)
    // emits for each entry type — it is the producer this classification is keyed to.
    let user = TreeNode::message("u", 0, "user: port the editor");
    let assistant = TreeNode::message("a", 0, "assistant: on it");
    let empty = TreeNode::message("n", 0, "assistant: (no content)");
    let bash = TreeNode::message("b", 0, "[bash]: cargo check");
    let mut tool = TreeNode::message("t", 0, "[read]");
    tool.kind = TreeKind::ToolGroup;
    let mut compaction = TreeNode::message("c", 0, "compaction");
    compaction.kind = TreeKind::Compaction;
    let mut summary = TreeNode::message("s", 0, "branch summary: dropped the retry loop");
    summary.kind = TreeKind::Compaction;
    let mut model = TreeNode::message("m", 0, "model → opus");
    model.kind = TreeKind::ModelChange;
    vec![user, assistant, empty, bash, tool, compaction, summary, model]
}

/// **S24(b).** Each entry type carries its own colour, and the role prefix carries a *different*
/// colour from the content behind it.
///
/// FAILS before the fix: every row's label was a single `base_style()` span, so a user row, an
/// assistant row, a tool result and a compaction summary were pixel-identical in colour.
#[test]
fn tree_rows_are_coloured_per_role_like_pi() {
    let theme = UiTheme::dark();
    let mut sel = TreeSelector::new(roled_nodes());
    // Park the cursor on a row not under test so no assertion is confounded by `:851`'s bold.
    sel.handle(&key(KeyCode::Down), &SelectKeymap::default());
    let rows = sel.rows(120, &theme);

    // `:781` — `accent` prefix, body text behind it.
    let user = &rows[0];
    assert_eq!(
        fg_of(span_style_containing(user, "user: ")),
        theme.accent_style().fg,
        "`user: ` must be the accent role: {:?}",
        text(user)
    );
    assert_eq!(
        fg_of(span_style_containing(user, "port the editor")),
        theme.base_style().fg,
        "the user's own text is body text, not accent: {:?}",
        text(user)
    );

    // `:793` — `success` prefix, `muted` placeholder body.
    let empty = &rows[2];
    assert_eq!(
        fg_of(span_style_containing(empty, "assistant: ")),
        theme.success_style().fg,
        "`assistant: ` must be the success role: {:?}",
        text(empty)
    );
    assert_eq!(
        fg_of(span_style_containing(empty, "(no content)")),
        theme.muted_style().fg,
        "the empty-assistant placeholder is muted: {:?}",
        text(empty)
    );

    // `:805` — the whole bash row is `dim`.
    assert_eq!(
        fg_of(span_style_containing(&rows[3], "[bash]: cargo check")),
        theme.dim_style().fg,
        "a bash row is dim end to end"
    );
    // `:799` — the whole tool row is `muted`.
    assert_eq!(
        fg_of(span_style_containing(&rows[4], "[read]")),
        theme.muted_style().fg,
        "a tool-result row is muted end to end"
    );
    // `:828` — `warning` prefix.
    assert_eq!(
        fg_of(span_style_containing(&rows[6], "branch summary: ")),
        theme.warning_style().fg,
        "a branch summary's prefix is the warning role"
    );
    // `:831` — `dim`.
    assert_eq!(
        fg_of(span_style_containing(&rows[7], "model → opus")),
        theme.dim_style().fg,
        "a model-change row is dim"
    );

    // The four must not collapse onto one another.
    let four = [
        span_style_containing(&rows[0], "user: ").fg,
        span_style_containing(&rows[1], "assistant: ").fg,
        span_style_containing(&rows[4], "[read]").fg,
        span_style_containing(&rows[5], "compaction").fg,
    ];
    for (i, a) in four.iter().enumerate() {
        for b in four.iter().skip(i + 1) {
            assert_ne!(a, b, "user / assistant / tool / compaction must differ in colour: {four:?}");
        }
    }
}

/// **T9 — `borderAccent`.** `tree-selector.ts:824` is the token's only component render site in all
/// of `packages/*/src`. In `dark.json` it resolves through `vars.cyan` `#00d7ff` (`:5`, `:25`),
/// which is a different colour from `accent` (`vars.accent` `#8abeb7`, `:14`, `:23`) — so a theme
/// author who sets it can actually see it.
///
/// FAILS before the fix: `grep -rn borderAccent crates/cyrup-tui/src` found nothing outside doc
/// comments, and the compaction row rendered in `base_style()`.
#[test]
fn tree_compaction_row_reads_the_border_accent_token() {
    let theme = UiTheme::dark();
    let want = *theme.roles.get("borderAccent").expect("dark.json:25 defines borderAccent");
    let mut sel = TreeSelector::new(roled_nodes());
    sel.handle(&key(KeyCode::Down), &SelectKeymap::default());
    let rows = sel.rows(120, &theme);
    let got = span_style_containing(&rows[5], "compaction").fg;
    assert_eq!(got, Some(want), "the compaction row must read `borderAccent`");
    assert_ne!(got, theme.accent_style().fg, "`borderAccent` is not `accent` (#00d7ff vs #8abeb7)");
    assert_ne!(got, theme.base_style().fg, "the compaction row is not body text");
}

/// **S24(b), the selection.** `:851` `return isSelected ? theme.bold(result) : result;` — the
/// selected row is BOLDED, keeping every role colour underneath. `theme.bold` (`theme.ts:384-386`)
/// emits SGR 1 and nothing else; it does not touch the foreground.
///
/// FAILS before the fix: the selected row's label was repainted `accent + BOLD`, erasing the role
/// colour on exactly the row the user is looking at.
#[test]
fn tree_selected_row_is_bolded_not_repainted_accent() {
    let theme = UiTheme::dark();
    let sel = TreeSelector::new(roled_nodes()); // row 0 (the `user:` row) is selected
    let rows = sel.rows(120, &theme);
    let prefix = span_style_containing(&rows[0], "user: ");
    let body = span_style_containing(&rows[0], "port the editor");

    assert!(prefix.add_modifier.contains(ratatui::style::Modifier::BOLD), "`:851` bolds the row");
    assert!(body.add_modifier.contains(ratatui::style::Modifier::BOLD), "`:851` bolds the row");
    assert_eq!(body.fg, theme.base_style().fg, "the body keeps its own colour under the bold");

    // And an unselected row of the same role is the same colour, minus the bold.
    let mut moved = TreeSelector::new(roled_nodes());
    moved.handle(&key(KeyCode::Down), &SelectKeymap::default());
    let unselected = span_style_containing(&moved.rows(120, &theme)[0], "port the editor");
    assert_eq!(unselected.fg, body.fg, "selection must not change the colour");
    assert!(!unselected.add_modifier.contains(ratatui::style::Modifier::BOLD));
}

/// **S24(a).** The label-timestamp pad is `width - leftWidth - stampWidth - 1`, and `leftWidth` has
/// to be a COLUMN count. Upstream measures the same quantities with `visibleWidth`
/// (`tree-selector.ts:747` `const anchorCol = visibleWidth(prefixPart);`, `:754`
/// `bodyWidth: visibleWidth(body)`).
///
/// The row here is unavoidably unicode even before the preview text: the `●` glyph, and the
/// `☆labeled` star that a labeled row always carries. Add a CJK preview and `chars().count()`
/// under-measures the row by one column per wide character.
///
/// FAILS under `spans.iter().map(|s| s.content.chars().count())`: the pad is computed from 31
/// columns where the row really occupies 41, so the row renders 10 columns too wide and the
/// timestamp is pushed off the right edge of an 80-column frame.
#[test]
fn tree_label_timestamp_pad_is_measured_in_columns_not_chars() {
    let theme = UiTheme::dark();
    let mut labeled = TreeNode::message("e1", 0, "user: 日本語のプレビュー行です");
    labeled.has_label = true;
    labeled.time_label = Some("12:04".to_string());
    let mut sel = TreeSelector::new(vec![labeled]);
    // `shift+t` = `app.tree.toggleLabelTimestamp` (`tree-selector.ts:1090`, bound at
    // `keybindings.ts:131-134`); the column is off by default.
    sel.handle(&tree_toggle_label_time_key(), &SelectKeymap::default());

    let width = 80u16;
    let row = &sel.rows(width, &theme)[0];
    let rendered = text(row);
    assert!(rendered.contains("12:04"), "precondition: the column is on: {rendered:?}");

    assert_eq!(
        row.width(),
        usize::from(width),
        "the padded row must occupy exactly the frame width: {rendered:?} ({} columns)",
        row.width()
    );
    // The stamp is the last thing on the row, so it must end flush against the right edge.
    let stamp_start = row.width() - 5;
    assert_eq!(
        ratatui::text::Span::raw(rendered.trim_end_matches("12:04")).width(),
        stamp_start,
        "the timestamp must end flush right: {rendered:?}"
    );
}
