//! `/tree` session-navigator layout tests (spec/tui/05 §5.1; Pi `tree-selector.ts`). Exercises the
//! bespoke layout: connectors, fold markers/behavior, filter modes, glyphs, and navigation — both
//! through the public API and via TestBackend buffer assertions.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::harness::key_event as key;
use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{
    FIELD_SEP, FilterMode, SelectKeymap, Selector, SelectorOutcome, TreeKind, TreeNode,
    TreeSelector, UiTheme,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

fn node(id: &str, depth: usize, label: &str, kind: TreeKind) -> TreeNode {
    let mut n = TreeNode::message(id, depth, label);
    n.kind = kind;
    n
}

/// A small DAG (the connector's middle cell carries the fold state — `tree-selector.ts:721-722`):
///   root (●)
///   ├⊟ model→opus (◆, foldable, expanded)
///   │   └─ "streaming" (●)
///   ├─ 14 tool calls (⚙)
///   └─ "fix footer" (●, labeled)
///        └─ compaction (✓)
fn sample() -> Vec<TreeNode> {
    let mut model = node("m", 1, "model -> opus", TreeKind::ModelChange);
    model.foldable = true;
    let mut footer = node("f", 1, "fix footer", TreeKind::Message);
    footer.has_label = true;
    vec![
        node("root", 0, "initial prompt", TreeKind::Message),
        model,
        node("stream", 2, "wire up streaming", TreeKind::Message),
        node("tools", 1, "14 tool calls", TreeKind::ToolGroup),
        footer,
        node("compact", 2, "compaction", TreeKind::Compaction),
    ]
}

fn buf_string(terminal: &Terminal<TestBackend>) -> String {
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

/// TUI-027: pi's `app.tree.foldOrUp` / `unfoldOrDown` defaults are `["alt+left","ctrl+left"]` /
/// `["alt+right","ctrl+right"]` (`core/keybindings.ts:119-126`).
fn alt(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::ALT)
}

/// pi's `app.tree.editLabel` = `shift+l`, `toggleLabelTimestamp` = `shift+t` (`:127-134`).
fn shift(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)
}

/// pi's seven `app.tree.filter.*` ids are ctrl chords (`core/keybindings.ts:179-206`).
fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

#[test]
fn renders_connectors_glyphs_and_fold_markers() {
    let theme = UiTheme::dark();
    let mut sel = TreeSelector::new(sample());
    let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
    terminal
        .draw(|f| sel.render(f, Rect::new(0, 0, 80, 12), &theme))
        .unwrap();
    let text = buf_string(&terminal);
    assert!(text.contains("Session Tree"), "header: {text}");
    assert!(text.contains("Filter: default"));
    assert!(text.contains('●'), "message glyph");
    assert!(text.contains('◆'), "model-change glyph");
    assert!(text.contains('⚙'), "tool-group glyph");
    assert!(
        text.contains("├─") || text.contains("└─"),
        "connectors: {text}"
    );
    // S24 (corrected): pi draws the fold state INSIDE the connector — `tree-selector.ts:722`
    //   `prefixChars.push(isFolded ? "⊞" : foldable ? "⊟" : "─");`
    // at `posInLevel === 1`, i.e. in place of the `─` of the node's own `├─ `. `model -> opus` is
    // depth-1, foldable and expanded, and is not the last child, so its connector is exactly `├⊟ `.
    // The separate `foldMarker` at `:734` is the connector-LESS fallback (`!showsFoldInConnector`),
    // not evidence that pi never emits `⊟`.
    assert!(
        text.contains("├⊟ "),
        "expanded foldable node must render `├⊟ `: {text}"
    );
    // Nothing is folded, so neither fold glyph may appear as `⊞`.
    assert!(
        !text.contains('\u{229e}'),
        "folded marker `⊞` present with nothing folded: {text}"
    );
    assert!(text.contains("☆labeled"), "label star on the labeled node");
}

#[test]
fn fold_hides_descendants_and_unfold_restores() {
    let mut sel = TreeSelector::new(sample());
    assert_eq!(sel.visible_indices().len(), 6);
    // Select the foldable model node (row 1) and fold it (`alt+left`).
    sel.handle(&key(KeyCode::Down), &SelectKeymap::default()); // -> row 1 (model)
    sel.handle(&alt(KeyCode::Left), &SelectKeymap::default()); // fold
    // The "streaming" child (depth 2 under model) is now hidden.
    let visible = sel.visible_indices();
    assert_eq!(visible.len(), 5, "one descendant hidden by the fold");
    // Unfold (`alt+right`) restores it.
    sel.handle(&alt(KeyCode::Right), &SelectKeymap::default());
    assert_eq!(sel.visible_indices().len(), 6);
}

/// TUI-027 — the filter modes are reached through pi's seven `app.tree.filter.*` ctrl chords
/// (`core/keybindings.ts:179-206`), and each non-`default` arm is a **toggle** back to `default`
/// (`tree-selector.ts:1039-1063`). The bare digits `1`-`5` cyrup used to accept were a
/// cyrup-original: upstream has no digit arm, and every unclaimed printable character is search
/// input instead.
#[test]
fn filter_modes_change_visible_set() {
    let mut sel = TreeSelector::new(sample());
    // no-tools (ctrl+t) hides the tool group.
    sel.handle(&ctrl('t'), &SelectKeymap::default());
    assert_eq!(sel.filter(), FilterMode::NoTools);
    assert!(!sel.visible_ids().contains(&"tools".to_string()));
    // labeled-only (ctrl+l) keeps only the labeled node.
    sel.handle(&ctrl('l'), &SelectKeymap::default());
    assert_eq!(sel.filter(), FilterMode::LabeledOnly);
    assert_eq!(sel.visible_ids(), vec!["f".to_string()]);
    // user (ctrl+u) keeps only messages.
    sel.handle(&ctrl('u'), &SelectKeymap::default());
    let ids = sel.visible_ids();
    assert!(ids.contains(&"root".to_string()) && ids.contains(&"stream".to_string()));
    assert!(!ids.contains(&"tools".to_string()) && !ids.contains(&"m".to_string()));
    // Pressing the SAME filter key again returns to `default` (`tree-selector.ts:1046-1050`).
    sel.handle(&ctrl('u'), &SelectKeymap::default());
    assert_eq!(sel.filter(), FilterMode::Default);
    // ctrl+d is the one non-toggling arm: it always selects `default` (`:1036-1038`).
    sel.handle(&ctrl('a'), &SelectKeymap::default());
    assert_eq!(sel.filter(), FilterMode::All);
    sel.handle(&ctrl('d'), &SelectKeymap::default());
    assert_eq!(sel.filter(), FilterMode::Default);
    sel.handle(&ctrl('d'), &SelectKeymap::default());
    assert_eq!(
        sel.filter(),
        FilterMode::Default,
        "`filter.default` is not a toggle"
    );
}

/// `app.tree.filter.cycleForward` / `cycleBackward` walk pi's ordered mode list and wrap
/// (`tree-selector.ts:1064-1076`).
#[test]
fn filter_cycle_walks_pis_mode_order_and_wraps() {
    let mut sel = TreeSelector::new(sample());
    let order = [
        FilterMode::NoTools,
        FilterMode::UserOnly,
        FilterMode::LabeledOnly,
        FilterMode::All,
        FilterMode::Default,
    ];
    for expected in order {
        sel.handle(&ctrl('o'), &SelectKeymap::default());
        assert_eq!(sel.filter(), expected, "cycleForward order");
    }
    // Backward from `default` wraps to the end of the list.
    sel.handle(
        &KeyEvent::new(
            KeyCode::Char('o'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ),
        &SelectKeymap::default(),
    );
    assert_eq!(sel.filter(), FilterMode::All, "cycleBackward wraps");
}

/// TUI-027, the headline — **typing an ordinary word into `/tree` must search, not act.**
///
/// RED before the fix: `t` fired `ToggleLabelTimestamp`, `e` opened the inline label editor, and
/// `x` + the second `t` were swallowed as label text, so `search_query()` did not exist and Enter
/// would have emitted an `Apply` payload that the chrome persists to the session JSONL.
#[test]
fn typing_a_word_searches_and_fires_no_tree_action() {
    let mut sel = TreeSelector::new(sample());
    for c in ['t', 'e', 'x', 't'] {
        assert_eq!(
            sel.handle(&ch(c), &SelectKeymap::default()),
            SelectorOutcome::Redraw,
            "`{c}` is search input"
        );
    }
    assert_eq!(sel.search_query(), "text");
    // No tree action fired: the filter is untouched, nothing folded, no label editor opened.
    assert_eq!(sel.filter(), FilterMode::Default);
    // And the search really filters: no sample row contains "text".
    assert!(
        sel.visible_ids().is_empty(),
        "no row matches `text`: {:?}",
        sel.visible_ids()
    );
    // Enter on an empty result set must NOT produce an `Apply` — the payload `app.rs` persists via
    // `host_services.set_label`.
    let out = sel.handle(&key(KeyCode::Enter), &SelectKeymap::default());
    assert_eq!(
        out,
        SelectorOutcome::Redraw,
        "no Apply, so no label is written: {out:?}"
    );
}

/// The search is whitespace-tokenised, lowercased, and every token must be a substring of the row
/// text (`tree-selector.ts:337`, `:391-393`).
#[test]
fn search_matches_all_tokens_case_insensitively() {
    let mut sel = TreeSelector::new(sample());
    for c in "FIX Foot".chars() {
        sel.handle(&ch(c), &SelectKeymap::default());
    }
    assert_eq!(
        sel.visible_ids(),
        vec!["f".to_string()],
        "both tokens must match one row"
    );
    // Backspace pops one character (`:1078-1084`).
    sel.handle(&key(KeyCode::Backspace), &SelectKeymap::default());
    assert_eq!(sel.search_query(), "FIX Foo");
    assert_eq!(sel.visible_ids(), vec!["f".to_string()]);
}

/// `tui.select.cancel` clears a live query FIRST and only cancels once it is empty
/// (`tree-selector.ts:1031-1037`).
#[test]
fn escape_clears_the_search_before_it_cancels() {
    let mut sel = TreeSelector::new(sample());
    sel.handle(&ch('f'), &SelectKeymap::default());
    assert_eq!(sel.search_query(), "f");
    let out = sel.handle(&key(KeyCode::Esc), &SelectKeymap::default());
    assert_eq!(out, SelectorOutcome::Redraw, "first Esc clears the query");
    assert_eq!(sel.search_query(), "");
    assert_eq!(sel.visible_indices().len(), 6, "all rows back");
    let out = sel.handle(&key(KeyCode::Esc), &SelectKeymap::default());
    assert_eq!(out, SelectorOutcome::Cancel, "second Esc cancels");
}

/// A `keybindings.json` carrying pi's `app.tree.filter.*` ids must take effect — before TUI-027
/// `TreeAction::from_id` knew only the four `app.tree.*` ids and silently dropped these seven.
#[test]
fn a_rebound_tree_filter_id_takes_effect() {
    use crate::{TreeAction, TreeKeymap};
    let mut km = TreeKeymap::default();
    km.merge_json(r#"{"app.tree.filter.noTools": "ctrl+n"}"#)
        .unwrap();
    assert_eq!(
        TreeAction::from_id("app.tree.filter.noTools"),
        Some(TreeAction::FilterNoTools)
    );
    let mut sel = TreeSelector::new(sample());
    sel.set_keymap(km);
    sel.handle(
        &KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
        &SelectKeymap::default(),
    );
    assert_eq!(sel.filter(), FilterMode::NoTools);
}

/// The help row is built from the LIVE keymap (`TreeHelp` over `TREE_HELP_ITEMS`,
/// `tree-selector.ts:1215-1236`), and the standing `Type to search:` prompt (`SearchLine`,
/// `:1165-1172`) is always drawn. Before TUI-027 the row was the hardcoded literal
/// `z/x branch   e label   t label time`.
#[test]
fn help_row_and_search_prompt_come_from_the_live_keymap() {
    let theme = UiTheme::dark();
    let mut sel = TreeSelector::new(sample());
    let mut terminal = Terminal::new(TestBackend::new(120, 14)).unwrap();
    terminal
        .draw(|f| sel.render(f, Rect::new(0, 0, 120, 14), &theme))
        .unwrap();
    let text = buf_string(&terminal);
    assert!(
        text.contains("Type to search:"),
        "standing search prompt: {text}"
    );
    assert!(
        text.contains("alt+←/alt+→ branch"),
        "branch cell from the live keymap: {text}"
    );
    assert!(
        text.contains("shift+l label"),
        "label cell from the live keymap: {text}"
    );
    assert!(
        !text.contains("z/x branch"),
        "the invented `z`/`x` cell is gone: {text}"
    );
}

#[test]
fn enter_confirms_selected_entry_id() {
    let mut sel = TreeSelector::new(sample());
    sel.handle(&key(KeyCode::Down), &SelectKeymap::default()); // row 1 = model "m"
    let out = sel.handle(&key(KeyCode::Enter), &SelectKeymap::default());
    assert_eq!(out, SelectorOutcome::Confirm("m".to_string()));
}

#[test]
fn esc_cancels() {
    let mut sel = TreeSelector::new(sample());
    let out = sel.handle(&key(KeyCode::Esc), &SelectKeymap::default());
    assert_eq!(out, SelectorOutcome::Cancel);
}

fn ch(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

/// F16: pressing `e` opens the inline label editor (no longer a dead no-op). The body shows the
/// `Label (empty to remove):` prompt and the save/cancel hint (Pi `LabelInput.render`,
/// tree-selector.ts:1256-1270).
#[test]
fn shift_l_opens_the_label_editor_overlay() {
    let theme = UiTheme::dark();
    let mut sel = TreeSelector::new(sample());
    let out = sel.handle(&shift('l'), &SelectKeymap::default());
    assert_eq!(
        out,
        SelectorOutcome::Redraw,
        "`shift+l` opens the editor and redraws"
    );
    let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
    terminal
        .draw(|f| sel.render(f, Rect::new(0, 0, 80, 12), &theme))
        .unwrap();
    let text = buf_string(&terminal);
    assert!(
        text.contains("Label (empty to remove):"),
        "label prompt shown: {text}"
    );
    assert!(
        text.contains("enter save"),
        "save/cancel hint shown: {text}"
    );
}

/// While the editor is open it captures ALL keys as literal text — `z` types a `z` instead of folding,
/// digits do not switch filters (Pi's `if (this.labelInput)` route, tree-selector.ts:1373-1379). On
/// confirm it emits an `Apply("{entry_id}\u{1f}{label}")` payload (the persist seam) and stays open,
/// and the node's label star turns on locally.
#[test]
fn label_editor_captures_keys_and_confirms_with_apply_payload() {
    let mut sel = TreeSelector::new(sample());
    // Select the un-labeled root and open the editor.
    assert_eq!(sel.selected_id().as_deref(), Some("root"));
    sel.handle(&shift('l'), &SelectKeymap::default());
    // Type "z1x" — every char is literal; the filter stays default and nothing folds.
    for c in ['z', '1', 'x'] {
        assert_eq!(
            sel.handle(&ch(c), &SelectKeymap::default()),
            SelectorOutcome::Redraw
        );
    }
    assert_eq!(
        sel.filter(),
        FilterMode::Default,
        "digits typed literally, no filter change"
    );
    assert_eq!(
        sel.visible_indices().len(),
        6,
        "`z`/`x` typed literally, no fold/unfold"
    );
    // Confirm → the persist payload carries the entry id + the typed label; the slot stays open.
    let out = sel.handle(&key(KeyCode::Enter), &SelectKeymap::default());
    assert_eq!(out, SelectorOutcome::Apply(format!("root{FIELD_SEP}z1x")));
    // The local star is set so the tree reflects the new label immediately: `root` was NOT labeled in
    // `sample()`, but labeled-only now includes it alongside the pre-labeled `f`.
    sel.set_filter(FilterMode::LabeledOnly);
    let labeled = sel.visible_ids();
    assert!(
        labeled.contains(&"root".to_string()),
        "root gained a label star: {labeled:?}"
    );
    assert!(
        labeled.contains(&"f".to_string()),
        "pre-labeled node still present: {labeled:?}"
    );
}

/// Confirming an EMPTY buffer clears the label (Pi `value || undefined` → remove; the payload's label
/// segment is empty and `apply_label` drops empty labels).
#[test]
fn label_editor_empty_confirm_removes_label() {
    let mut sel = TreeSelector::new(sample());
    // Move to the already-labeled "fix footer" node (`f`).
    while sel.selected_id().as_deref() != Some("f") {
        sel.handle(&key(KeyCode::Down), &SelectKeymap::default());
    }
    sel.handle(&shift('l'), &SelectKeymap::default());
    // Confirm with an empty buffer → remove.
    let out = sel.handle(&key(KeyCode::Enter), &SelectKeymap::default());
    assert_eq!(out, SelectorOutcome::Apply(format!("f{FIELD_SEP}")));
    // The star is cleared locally: labeled-only now excludes `f`.
    sel.set_filter(FilterMode::LabeledOnly);
    assert!(
        !sel.visible_ids().contains(&"f".to_string()),
        "label star cleared after empty confirm"
    );
}

/// Esc inside the editor discards the edit (no `Apply`, no label change) and returns to the tree.
#[test]
fn label_editor_esc_discards() {
    let mut sel = TreeSelector::new(sample());
    // Label the already-labeled node's neighbor to prove nothing persists on cancel.
    sel.handle(&shift('l'), &SelectKeymap::default());
    sel.handle(&ch('x'), &SelectKeymap::default());
    let out = sel.handle(&key(KeyCode::Esc), &SelectKeymap::default());
    assert_eq!(
        out,
        SelectorOutcome::Redraw,
        "esc discards without an Apply"
    );
    // The editor closed: a subsequent `alt+left` folds again (tree keys are live once more).
    sel.handle(&key(KeyCode::Down), &SelectKeymap::default()); // row 1 (foldable model)
    sel.handle(&alt(KeyCode::Left), &SelectKeymap::default());
    assert_eq!(
        sel.visible_indices().len(),
        5,
        "tree keys live again after cancel"
    );
}
