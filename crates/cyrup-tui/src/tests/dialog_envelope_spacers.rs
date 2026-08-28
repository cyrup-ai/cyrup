//! SYS-3 / L4 + S20 + E5 + E6 + E7 — the **dialog** half of pi's `Spacer(1)` layout language.
//!
//! pi separates a dialog's structural children with `new Spacer(1)`; cyrup's `Layout::vertical`
//! regions were adjacent and nobody added the blank rows back. This file pins the row order of each
//! envelope against the constructor of the pi component it ports, counted child by child at
//! v0.84.1, plus the two things the spacers alone would not cover: the `ui.editor` closing
//! `DynamicBorder` (E5) and the `ui.input` hint row (E6).
//!
//! Two disciplines run through every test here.
//!
//! **The spacers are per-COMPONENT, never per-engine.** `SelectList`
//! (`packages/tui/src/components/select-list.ts`) emits no blank rows, and two of the components
//! that host one — `show-images-selector.ts:25,41,44` and `theme-selector.ts:35,58,61` — are
//! `DynamicBorder`/list/`DynamicBorder` and nothing else, while
//! `settings-selector.ts:765,873-874` is the same three children for `/settings`. Every "adds
//! spacers" test therefore has a MIRROR asserting a zero-spacer component stayed flush, so a
//! regression that pushes the blank rows down into the shared engine fails here rather than
//! shipping ~10 dialogs a padding pi does not draw.
//!
//! **A short slot shows a PREFIX of the tall render — it never drops a blank.** This is read out of
//! pi, not chosen: a dialog is a plain `Container` (`packages/tui/src/tui.ts:211-245`) whose
//! `render(width)` concatenates its children with no height input, so its `Spacer(1)` children
//! (`packages/tui/src/components/spacer.ts:21-27`) are emitted at every terminal size. The height
//! decision happens in the dock `VStack` above it (`interactive-mode.ts:876-883`, the selector
//! mounted at `:4370-4371`), and `layoutComponent`/`paintBox` then keep the FIRST
//! `allocatedHeight` lines (`packages/tui/src/layout.ts:113,307-310`). So the trailing chrome — the
//! hint row, the bottom `DynamicBorder` — is what a short terminal costs you, and `/resume` and
//! `cyrup config` lead with a blank even at one row, because `session-selector.ts:737` and
//! `config-selector.ts:901` put a `Spacer(1)` above the top border.
//!
//! (Ratatui's constraint solver does NOT do this on its own — a `[Length(1); 4]` stack resolves to
//! `[0,1,0,0]` at height 1, and `[1,1,Min(0),1,1]` resolves to the HINT row alone, which is why
//! these renders carve their rows explicitly through `stack_rows`.)
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::string_slice
)]

use crate::{
    CheckboxSelector, ConfigKind, ConfigRow, ConfigScope, ConfigSelector, Key, ListSelector,
    ModelEntry, ModelSelector, SelectAction, SelectKeymap, Selector, SelectorKind, SessionRow,
    SessionSelector, SettingRow, SettingsSelector, TextInputSelector, TrustSelector, UiTheme,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// Render `sel` into a `w`×`h` buffer and return its rows as trailing-trimmed strings.
fn rows_at(sel: &mut dyn Selector, w: u16, h: u16) -> Vec<String> {
    let theme = UiTheme::dark();
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| sel.render(f, f.area(), &theme)).unwrap();
    let buf = term.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            let mut line = String::new();
            for x in 0..buf.area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    line.push_str(cell.symbol());
                }
            }
            line.trim_end().to_string()
        })
        .collect()
}

/// Render at the selector's own natural height, which is what the chrome gives it when the terminal
/// allows (`App` clamps `desired_height` to the available rows).
fn natural(sel: &mut dyn Selector, w: u16) -> Vec<String> {
    let h = sel.desired_height(w);
    rows_at(sel, w, h)
}

/// The `/thinking` picker over pi's full `THINKING_LEVEL_OPTIONS` ladder (`core/defaults.ts:4-12`),
/// which is what `getAvailableThinkingLevels()` returns with no model resolved
/// (`agent-session.ts:1817`).
fn thinking(current: &str, default_level: &str) -> crate::ThinkingSelector {
    let levels: Vec<String> = ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    crate::ThinkingSelector::new(&levels, current, default_level, "Shift+Tab".to_string())
}

fn is_rule(row: &str) -> bool {
    !row.is_empty() && row.chars().all(|c| c == '─')
}

/// The shared degradation contract, and the strongest statement of it: at EVERY height from one row
/// up to the dialog's natural height, the rows rendered are exactly the first `h` rows of the
/// natural render.
///
/// That is pi's behaviour verbatim — `allocatedHeight` clips a `Container`'s already-rendered line
/// array from the bottom (`packages/tui/src/layout.ts:113`, painted at `:307-310`) — and it implies
/// everything the previous, weaker contract said plus two things it did not: that a one-row resize
/// changes exactly one row, and that no `Spacer(1)` is ever traded away for content.
///
/// It REPLACES `assert_degrades_to_the_rule`, which asserted `rows[0]` is the top rule at h ∈
/// {1,3,5}. That was wrong for the two envelopes whose first child is a `Spacer`
/// (`session-selector.ts:737`, `config-selector.ts:901`): upstream's own row 0 there is blank.
fn assert_short_slot_is_a_prefix(label: &str, mut build: impl FnMut() -> Box<dyn Selector>) {
    let natural_h = build().desired_height(60);
    let full = rows_at(build().as_mut(), 60, natural_h);
    for h in 1..=natural_h {
        let mut sel = build();
        let rows = rows_at(sel.as_mut(), 60, h);
        assert_eq!(rows.len(), usize::from(h), "{label} @h={h}: buffer height");
        assert_eq!(
            rows,
            full[..usize::from(h)],
            "{label} @h={h}: must be the first {h} rows of the natural render {full:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// L4 — `ListSelector`, standing in for `extension-selector.ts` and `oauth-selector.ts`
// ---------------------------------------------------------------------------------------------

fn extension_select() -> ListSelector {
    ListSelector::prompt(
        "Pick one".to_string(),
        vec![
            ("a".to_string(), "Alpha".to_string(), None),
            ("b".to_string(), "Beta".to_string(), None),
        ],
        0,
    )
    .with_upstream_chrome(SelectorKind::ExtensionSelect, &SelectKeymap::default())
}

/// `ExtensionSelectorComponent` (`extension-selector.ts:44-75`) adds, in order: `DynamicBorder`(:44),
/// `Spacer`(:45), the title `Text`(:47), `Spacer`(:49), the list container(:61), `Spacer`(:62), the
/// `↑↓ navigate / select / cancel` hint(:63-73), `Spacer`(:74), `DynamicBorder`(:75). **Four blank
/// rows.** cyrup drew none of them.
#[test]
fn extension_selector_envelope_has_the_four_upstream_spacer_rows() {
    let mut sel = extension_select();
    let rows = natural(sel.as_mut_selector(), 60);
    let n = rows.len();
    assert!(is_rule(&rows[0]), "row 0 is the top DynamicBorder: {rows:?}");
    assert_eq!(rows[1], "", "Spacer(1) after the top border (:45): {rows:?}");
    assert!(rows[2].contains("Pick one"), "the title follows it (:47): {rows:?}");
    assert_eq!(rows[3], "", "Spacer(1) after the title (:49): {rows:?}");
    assert!(rows[4].contains("Alpha"), "the list starts here (:61): {rows:?}");

    assert!(is_rule(&rows[n - 1]), "the last row is the bottom DynamicBorder (:75): {rows:?}");
    assert_eq!(rows[n - 2], "", "Spacer(1) after the hint (:74): {rows:?}");
    assert!(rows[n - 3].contains("navigate"), "the hint row (:63-73): {rows:?}");
    assert_eq!(rows[n - 4], "", "Spacer(1) after the list (:62): {rows:?}");

    assert_eq!(
        rows.iter().filter(|r| r.is_empty()).count(),
        4,
        "exactly four blank rows, no more: {rows:?}"
    );
}

/// MIRROR, retargeted to pi 0.84.3. `ThinkingSelectorComponent` is no longer border/list/border:
/// its constructor (`thinking-selector.ts:77-97`) is `DynamicBorder`(:77) · `Spacer`(:78) ·
/// `Text("Thinking Level")`(:79) · `Spacer`(:80) · the cycle-key `Text`(:81) · `Spacer`(:82) ·
/// `Input`(:84-86) · `Spacer`(:87) · `SelectList`(:92) · `Spacer`(:93) · the dim footer(:94) ·
/// `DynamicBorder`(:97). **Five** spacers, and they belong to that component — which is why it is
/// [`crate::ThinkingSelector`] and not a [`ListSelector`] kind. If this envelope ever migrates into
/// the shared engine, the show-images/theme mirrors below go red.
#[test]
fn thinking_selector_draws_pis_0_84_3_envelope() {
    let mut sel = thinking("medium", "medium");
    let rows = natural(sel.as_mut_selector(), 60);
    let n = rows.len();
    assert!(is_rule(&rows[0]), "top rule (:77): {rows:?}");
    assert_eq!(rows[1], "", "Spacer(1) (:78): {rows:?}");
    assert_eq!(rows[2], "Thinking Level", "the title Text (:79): {rows:?}");
    assert_eq!(rows[3], "", "Spacer(1) (:80): {rows:?}");
    assert!(
        rows[4].ends_with("cycles thinking levels in-session"),
        "the app.thinking.cycle sentence (:81): {rows:?}"
    );
    assert_eq!(rows[5], "", "Spacer(1) (:82): {rows:?}");
    // `Input.render`'s prompt (`input.ts:380`) is `"> "`; `rows_at` trims the trailing space off
    // the empty query, so the row is a bare `>`.
    assert_eq!(rows[6], crate::INPUT_PROMPT.trim_end(), "the search Input (:84-86): {rows:?}");
    assert_eq!(rows[7], "", "Spacer(1) (:87): {rows:?}");
    assert!(rows[8].contains("off"), "the first level row (:92): {rows:?}");
    assert!(is_rule(&rows[n - 1]), "bottom rule (:97): {rows:?}");
    assert!(rows[n - 2].contains("set as default"), "the dim footer (:94): {rows:?}");
    assert!(rows[n - 3].is_empty(), "Spacer(1) above it (:93): {rows:?}");
    assert!(rows[n - 4].contains("max"), "and the last level row above that: {rows:?}");
    assert_eq!(
        rows.iter().filter(|r| r.is_empty()).count(),
        5,
        "exactly the five Spacer(1) children at :78, :80, :82, :87, :93: {rows:?}"
    );
}

/// MIRROR. `/login` and `/logout` are `OAuthSelectorComponent` (`oauth-selector.ts:68-96`), which
/// has four spacers but **no hint row** — `:69`, `:74` and `:93` land here, while `:87`'s belongs to
/// the search `Input` cyrup has not ported. So: three blanks, no `navigate` row.
#[test]
fn oauth_selector_envelope_has_three_spacers_and_no_hint_row() {
    let mut sel = ListSelector::data(
        SelectorKind::Login,
        vec![("anthropic".to_string(), "Anthropic".to_string(), None)],
        0,
    )
    .with_upstream_chrome(SelectorKind::Login, &SelectKeymap::default());
    let rows = natural(sel.as_mut_selector(), 60);
    let n = rows.len();
    assert_eq!(rows[1], "", "Spacer(1) after the top border (:69): {rows:?}");
    assert_eq!(rows[3], "", "Spacer(1) after the title (:74): {rows:?}");
    assert_eq!(rows[n - 2], "", "Spacer(1) before the bottom border (:93): {rows:?}");
    assert!(
        !rows.iter().any(|r| r.contains("navigate")),
        "OAuthSelectorComponent contains no keyHint call at all: {rows:?}"
    );
    assert_eq!(rows.iter().filter(|r| r.is_empty()).count(), 3, "three blanks: {rows:?}");
}

#[test]
fn extension_selector_envelope_is_a_prefix_on_a_short_slot() {
    assert_short_slot_is_a_prefix("extension select", || Box::new(extension_select()));
}

/// The list must never be starved BEFORE the trailing chrome is.
///
/// `ListSelector` used to size its body as `area.height - fixed` with `fixed` counting the hint row
/// unconditionally, so on a three-row slot the hint won and the dialog showed
/// `[rule, title, "↑↓ navigate …"]` — zero options, with the row upstream sheds FIRST occupying the
/// space the options should have had. `extension-selector.ts` adds the hint at `:63-73`, after the
/// list at `:61`, and pi keeps the leading lines (`packages/tui/src/layout.ts:113,307-310`), so the
/// hint cannot outlive an option. Here the body takes its natural height and `stack_rows` clips
/// top-first, which reproduces that ordering exactly.
///
/// Note the option appears at h=5, not h=1: `:44` `:45` `:47` `:49` come before `:61`. pi has no
/// floor that would show one sooner, and adding one would have to spend a `Spacer`.
#[test]
fn extension_selector_never_shows_its_hint_row_before_its_options() {
    for h in 1..=5u16 {
        let mut sel = extension_select();
        let rows = rows_at(sel.as_mut_selector(), 60, h);
        let hint_at = rows.iter().position(|r| r.contains("navigate"));
        let option_at = rows.iter().position(|r| r.contains("Alpha"));
        match (hint_at, option_at) {
            (Some(hint), Some(option)) => assert!(option < hint, "@h={h}: {rows:?}"),
            (Some(_), None) => panic!("@h={h}: the hint row displaced the options: {rows:?}"),
            _ => {}
        }
    }
    // The list's first row is child `:61`, i.e. row 4 — the first height that can seat it.
    let mut sel = extension_select();
    let rows = rows_at(sel.as_mut_selector(), 60, 5);
    assert!(rows[4].contains("Alpha"), "the first option, at the first height that fits: {rows:?}");
}

/// Envelope stability across a resize (the property the all-or-nothing spacer gate destroyed).
///
/// That gate flipped all four blanks on or off on a single row of slack, so shrinking a terminal by
/// one line moved 3-5 rows at once. pi cannot do that: its blanks are unconditional children and
/// its layout clips the tail, so one row of shrink is one row of change
/// (`packages/tui/src/layout.ts:113`). Checked here as a diff count between adjacent heights.
#[test]
fn one_row_of_resize_changes_exactly_one_row() {
    let natural_h = extension_select().desired_height(60);
    for h in 2..=natural_h {
        let tall = rows_at(extension_select().as_mut_selector(), 60, h);
        let short = rows_at(extension_select().as_mut_selector(), 60, h - 1);
        let changed = tall
            .iter()
            .zip(short.iter())
            .filter(|(a, b)| a != b)
            .count()
            + 1; // the row that disappeared
        assert_eq!(changed, 1, "h={h} → {}: {tall:?} vs {short:?}", h - 1);
    }
}

// ---------------------------------------------------------------------------------------------
// L4 — `CheckboxSelector` (`/scoped-models`, `scoped-models-selector.ts`)
// ---------------------------------------------------------------------------------------------

fn scoped_models() -> CheckboxSelector {
    CheckboxSelector::scoped_models(
        vec![(
            "claude".to_string(),
            "Claude".to_string(),
            "anthropic".to_string(),
            None,
        )],
        Some(vec!["claude".to_string()]),
    )
}

/// `ScopedModelsSelectorComponent` (`scoped-models-selector.ts:130-156`): `DynamicBorder`(:130),
/// `Spacer`(:131), title(:132), subtitle(:133-135), `Spacer`(:136), search `Input`(:140),
/// `Spacer`(:141), listContainer(:145), `Spacer`(:148), footer(:153-154), `DynamicBorder`(:156).
/// All **four** of its spacers land, and note the footer sits **flush** against the bottom border —
/// unlike `extension-selector.ts:74`, this component has no spacer there.
///
/// The fifth blank row is not an envelope spacer at all: it belongs to the list container, which
/// adds `new Spacer(1)` of its own before the `Model Name:` row (`:271`).
#[test]
fn scoped_models_envelope_has_four_spacers_and_a_flush_footer() {
    let mut sel = scoped_models();
    let rows = natural(sel.as_mut_selector(), 60);
    let n = rows.len();
    assert!(is_rule(&rows[0]), "top border (:130): {rows:?}");
    assert_eq!(rows[1], "", "Spacer(1) after the top border (:131): {rows:?}");
    assert_eq!(rows[2], "Model Configuration", "title (:132): {rows:?}");
    assert_eq!(
        rows[3], "Session-only. ctrl+s to save to settings.",
        "subtitle (:133-135): {rows:?}"
    );
    assert_eq!(rows[4], "", "Spacer(1) after the subtitle (:136): {rows:?}");
    assert!(rows[5].starts_with('>'), "search Input (:140): {rows:?}");
    assert_eq!(rows[6], "", "Spacer(1) after the Input (:141): {rows:?}");
    assert_eq!(rows[7], "→ claude [anthropic] ✓", "the one list row (:245-259): {rows:?}");
    assert_eq!(rows[8], "", "the list container's own Spacer(1) (:271): {rows:?}");
    assert_eq!(rows[9], "  Model Name: Claude", "(:272-278): {rows:?}");
    assert_eq!(rows[10], "", "Spacer(1) between list and footer (:148): {rows:?}");
    assert!(rows[11].starts_with("  enter toggle"), "footer (:153-154): {rows:?}");
    assert!(is_rule(&rows[n - 1]), "bottom border (:156): {rows:?}");
    assert!(rows[n - 2].contains("enabled"), "the footer is FLUSH against it: {rows:?}");
    assert_eq!(
        rows.iter().filter(|r| r.is_empty()).count(),
        5,
        "four envelope Spacers plus the list's own: {rows:?}"
    );
}

#[test]
fn scoped_models_envelope_is_a_prefix_on_a_short_slot() {
    assert_short_slot_is_a_prefix("scoped models", || Box::new(scoped_models()));
}

// ---------------------------------------------------------------------------------------------
// L4 — `/settings` (the audit row that the source contradicts)
// ---------------------------------------------------------------------------------------------

/// MIRROR / audit correction. TUI-FIDELITY's L4 row lists `settings_selector.rs:187-194` among the
/// six envelopes needing `Spacer(1)` rows. The source disagrees and the source wins:
/// `SettingsSelectorComponent`'s constructor adds exactly `new DynamicBorder()`
/// (`settings-selector.ts:765`), the `SettingsList` (`:873`) and `new DynamicBorder()` (`:874`) —
/// **no `Spacer`, and no title `Text` either** (S16).
///
/// The blanks the dialog does show are `SettingsList`'s own, and they are load-bearing:
/// `renderMainList` pushes one under the search `Input` (`settings-list.ts:95`), one above the
/// description block (`:155`) and one above the hint (`:237`). The previous revision of this test
/// asserted **zero** blank rows, which was true only while the search box and the hint blank were
/// missing entirely.
#[test]
fn settings_selector_envelope_is_border_list_border_with_no_title_row() {
    let mut sel = SettingsSelector::new(
        "Settings",
        vec![SettingRow::toggle("terminal.showImages", "Show images", true)],
    );
    let rows = natural(sel.as_mut_selector(), 60);
    let n = rows.len();
    assert!(is_rule(&rows[0]), "DynamicBorder (:765): {rows:?}");
    // The list's FIRST line is the search box — nothing between it and the rule (no Spacer, and
    // no title, which is the row cyrup used to invent).
    assert_eq!(rows[1], ">", "the search Input is flush against the rule (:94): {rows:?}");
    assert!(
        !rows.iter().any(|r| r.trim() == "Settings"),
        "upstream draws no title row for /settings (:765-874): {rows:?}"
    );
    assert_eq!(rows[2], "", "the blank under the search box (:95): {rows:?}");
    assert!(rows[3].contains("Show images"), "the first settings row (:143): {rows:?}");
    assert!(is_rule(&rows[n - 1]), "DynamicBorder (:874): {rows:?}");
    assert_eq!(
        rows[n - 2], "  Type to search · Enter/Space to change · Esc to cancel",
        "addHintLine's search-enabled text, flush against the bottom rule (:242): {rows:?}"
    );
    assert_eq!(rows[n - 3], "", "addHintLine's leading blank (:237): {rows:?}");
    assert_eq!(
        rows.iter().filter(|r| r.is_empty()).count(),
        2,
        "exactly SettingsList's own two blanks here (:95, :237) — no row carries a description, \
         so :155 does not fire: {rows:?}"
    );
}

/// MIRROR for S16 + S33. Everything S16/S33 add — a search `Input`, a description block, the
/// `Type to search …` hint, the `min(30, widest)` label column — belongs to **`SettingsList`**
/// (`packages/tui/src/components/settings-list.ts`) and to nothing else. The components that host a
/// `SelectList` instead get none of it: `show-images-selector.ts:25,41,44` and
/// `theme-selector.ts:35,58,61` are border/list/border with no `Input` and no hint, and
/// `SelectList`'s own column policy is `getPrimaryColumnWidth` (`select-list.ts:178-197`) with the
/// `{12, 32}` slash bounds — NOT `min(30, widest)`.
///
/// The subject used to be the thinking picker. It cannot be any more: at 0.84.3
/// `ThinkingSelectorComponent` grew a search `Input` of its OWN (`thinking-selector.ts:84-86`), so
/// "no Input" is no longer true of it — and it is now [`crate::ThinkingSelector`], not a
/// [`ListSelector`] kind, which is exactly the separation this test exists to protect.
///
/// This is the batch-3 failure mode in miniature: a hint row put on the shared engine reached ~10
/// dialogs pi draws it on 4. If any of these assertions ever flips, the `SettingsList` port has
/// leaked into `SelectList`.
#[test]
fn settings_list_behaviours_do_not_leak_into_the_shared_select_list() {
    let mut sel = ListSelector::show_images(true)
        .with_upstream_chrome(SelectorKind::ShowImages, &SelectKeymap::default());
    let rows = natural(sel.as_mut_selector(), 60);
    assert!(!rows.iter().any(|r| r.starts_with("> ") || r.trim_end() == ">"), "no Input: {rows:?}");
    assert!(!rows.iter().any(|r| r.contains("Type to search")), "no SettingsList hint: {rows:?}");

    // `ColumnLayout::SLASH` still pins the primary column at `[12, 32]` — a 3-column label is
    // padded out to 12 there, which is exactly what `min(30, widest)` must NOT do.
    assert_eq!(crate::ColumnLayout::SLASH.primary_min, 12);
    assert_eq!(crate::ColumnLayout::SLASH.primary_max, 32);
    let list = crate::SelectList::new(
        vec![crate::SelectItem::new("abc", Some("desc".to_string()))],
        crate::ColumnLayout::SLASH,
    );
    let line = list.lines(60, &UiTheme::dark())[0].to_string();
    // Char columns, not byte offsets — the `→` cursor is three bytes wide.
    let col = line[..line.find("desc").unwrap()].chars().count();
    assert_eq!(col, 14, "SelectList: 2 prefix + 12-wide column: {line:?}");

    // And `/trust`, which lives in the same module as `SettingsSelector`, keeps its own shape.
    let mut trust = trust(None);
    let rows = natural(trust.as_mut_selector(), 60);
    assert!(!rows.iter().any(|r| r.trim_end() == ">"), "no search box on /trust: {rows:?}");
    assert!(!rows.iter().any(|r| r.contains("Type to search")), "{rows:?}");
}

/// S16 — the description block: a blank, then the HIGHLIGHTED row's description wrapped at
/// `width - 4` with every wrapped row prefixed `"  "` (`settings-list.ts:152-160`). It tracks the
/// highlight, so moving down swaps the text.
#[test]
fn settings_selector_renders_the_selected_rows_description_block() {
    let mut sel = SettingsSelector::new(
        "Settings",
        vec![
            SettingRow::toggle("autocompact", "Auto-compact", true)
                .with_description("Automatically compact the conversation when it grows too long"),
            SettingRow::toggle("images", "Show images", true).with_description("Render inline"),
        ],
    );
    let rows = natural(sel.as_mut_selector(), 40);
    let block: Vec<&String> = rows.iter().filter(|r| r.starts_with("  Automatically")).collect();
    assert_eq!(block.len(), 1, "the first row's description is shown: {rows:?}");
    // width - 4 = 36, and the "  " prefix goes on AFTER wrapping, so a wrapped row is <= 38 cols.
    // The 60-column description therefore spans two rows rather than being clipped.
    assert!(
        rows.iter().any(|r| r == "  Automatically compact the")
            && rows.iter().any(|r| r == "  conversation when it grows too long"),
        "the description wraps at width-4 rather than clipping: {rows:?}"
    );
    for row in rows.iter().filter(|r| r.starts_with("  ") && !r.starts_with("  Type to")) {
        assert!(row.chars().count() <= 38, "wrapped at width-4 plus the `  ` prefix: {row:?}");
    }
    assert!(!rows.iter().any(|r| r.contains("Render inline")), "only the SELECTED row's: {rows:?}");

    let keymap = crate::SelectKeymap::default();
    sel.handle(
        &crate::crossterm::event::KeyEvent::new(
            crate::crossterm::event::KeyCode::Down,
            crate::crossterm::event::KeyModifiers::NONE,
        ),
        &keymap,
    );
    let rows = natural(sel.as_mut_selector(), 40);
    assert!(rows.iter().any(|r| r == "  Render inline"), "it follows the highlight: {rows:?}");
    assert!(!rows.iter().any(|r| r.starts_with("  Automatically")), "{rows:?}");
}

/// S16 — the search box actually filters, and `Space` is a literal space once the box is non-empty
/// (`settings-list.ts:186-188`).
#[test]
fn settings_selector_search_filters_and_space_types_once_the_box_is_dirty() {
    let mut sel = SettingsSelector::new(
        "Settings",
        vec![
            SettingRow::toggle("autocompact", "Auto-compact", true),
            SettingRow::toggle("images", "Show images", true),
        ],
    );
    let keymap = crate::SelectKeymap::default();
    let ch = |c: char| {
        crate::crossterm::event::KeyEvent::new(
            crate::crossterm::event::KeyCode::Char(c),
            crate::crossterm::event::KeyModifiers::NONE,
        )
    };
    // Space on an empty box activates the row (`data === " " && searchInput.getValue().length === 0`).
    assert!(matches!(sel.handle(&ch(' '), &keymap), crate::SelectorOutcome::Apply(_)));

    for c in "imag".chars() {
        sel.handle(&ch(c), &keymap);
    }
    let rows = natural(sel.as_mut_selector(), 60);
    assert_eq!(rows[1], "> imag", "the query is echoed in the Input (:94): {rows:?}");
    assert!(rows.iter().any(|r| r.contains("Show images")), "the match survives: {rows:?}");
    assert!(!rows.iter().any(|r| r.contains("Auto-compact")), "the non-match is gone: {rows:?}");

    // Now Space is text, not an activation.
    let before = sel.current().map(|r| r.value.clone());
    assert!(matches!(sel.handle(&ch(' '), &keymap), crate::SelectorOutcome::Redraw));
    assert_eq!(sel.current().map(|r| r.value.clone()), before, "no cycle (:187): {:?}", sel.query());
    assert_eq!(sel.query(), "imag ");

    // A query that matches nothing takes the `No matching settings` arm (:107-111), which still
    // carries the hint.
    for c in "zzz".chars() {
        sel.handle(&ch(c), &keymap);
    }
    let rows = natural(sel.as_mut_selector(), 60);
    assert!(rows.iter().any(|r| r == "  No matching settings"), "(:108): {rows:?}");
    assert!(rows.iter().any(|r| r.contains("Type to search")), "addHintLine still runs: {rows:?}");
}

/// S33 — the label column is `Math.min(30, Math.max(...labels))` (`settings-list.ts:121`), measured
/// over ALL items with **no lower bound**. `ColumnLayout::SLASH`'s `{12, 32}` was a different
/// upstream component's policy: it padded short labels out to 12 and capped long ones at 32.
#[test]
fn settings_selector_label_column_hugs_short_labels_and_caps_at_thirty() {
    // Widest label is 3 columns. Upstream pads to 3, not to 12.
    let mut sel = SettingsSelector::new(
        "Settings",
        vec![SettingRow::toggle("a", "abc", true), SettingRow::toggle("b", "xy", false)],
    );
    let rows = natural(sel.as_mut_selector(), 60);
    assert!(rows.iter().any(|r| r == "→ abc  true"), "3-wide column + `  ` separator: {rows:?}");
    assert!(rows.iter().any(|r| r == "  xy   false"), "`xy` padded to 3: {rows:?}");

    // A 40-column label clamps to 30, not 32.
    let long = "l".repeat(40);
    let mut sel = SettingsSelector::new(
        "Settings",
        vec![
            SettingRow::toggle("a", long.clone(), true),
            SettingRow::toggle("b", "short", false),
        ],
    );
    let rows = natural(sel.as_mut_selector(), 60);
    let short_row = rows
        .iter()
        .find(|r| r.contains("short"))
        .unwrap_or_else(|| panic!("no short row: {rows:?}"));
    // `  ` cursor + 30-wide column + `  ` separator ⇒ the value starts at column 34.
    assert_eq!(short_row.find("false"), Some(34), "min(30, …), not 32: {short_row:?}");
}

fn settings_selector() -> SettingsSelector {
    SettingsSelector::new(
        "Settings",
        vec![
            SettingRow::toggle("terminal.showImages", "Show images", true),
            SettingRow::toggle("autoCompact", "Auto compact", false),
            SettingRow::choice(
                "steering",
                "Steering",
                "on",
                vec!["on".to_string(), "off".to_string()],
            ),
        ],
    )
}

/// `/settings`' height ladder — the row this batch edited but left on `Layout::vertical`.
///
/// Ratatui's solver minimises error across constraints rather than honouring an order, so the
/// five-region `[Length(1), Length(1), Min(0), Length(1), Length(1)]` stack it kept resolved, on a
/// one-row slot, to region #4: the dialog rendered its HINT and nothing else — no rule, no title,
/// no settings. Upstream's first child is `new DynamicBorder()` (`settings-selector.ts:765`), and
/// pi paints the first `allocatedHeight` lines of the component (`packages/tui/src/layout.ts:113`,
/// `:307-310`), so one row is that border.
#[test]
fn settings_selector_height_ladder_never_renders_the_hint_instead_of_the_dialog() {
    let natural_h = settings_selector().desired_height(60);
    let full = rows_at(settings_selector().as_mut_selector(), 60, natural_h);
    assert!(is_rule(&full[0]), "DynamicBorder (:765): {full:?}");
    // S16: `SettingsList`'s first line is its search `Input` (`settings-list.ts:94`); upstream
    // draws no title row, so the row that used to read "Settings" is the search box.
    assert_eq!(full[1], ">", "the search Input (:94): {full:?}");
    assert_eq!(full[2], "", "the blank under it (:95): {full:?}");
    assert!(full[3].contains("Show images"), "the first settings row (:143): {full:?}");

    for h in [1u16, 2, 3, 5, natural_h] {
        let mut sel = settings_selector();
        let rows = rows_at(&mut sel, 60, h);
        assert_eq!(rows.len(), usize::from(h));
        assert_eq!(
            rows,
            full[..usize::from(h)],
            "@h={h}: the first {h} rows of the natural render, not a solver's pick: {rows:?}"
        );
        assert!(
            !rows[0].contains("Type to search"),
            "@h={h}: the surviving row is the dialog, never its hint row: {rows:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// L4 + S20 — `/trust` (`trust-selector.ts`, the densest envelope: five spacers)
// ---------------------------------------------------------------------------------------------

fn trust(saved: Option<usize>) -> TrustSelector {
    TrustSelector::new(
        "/home/me/project",
        "trusted (/home/me/project)",
        true,
        vec!["Trust".to_string(), "Trust parent".to_string(), "Do not trust".to_string()],
        0,
    )
    .with_saved_index(saved)
}

/// `TrustSelectorComponent` (`trust-selector.ts:52-87`) in full: `DynamicBorder`(:52),
/// `Spacer`(:53), "Project trust"(:54), the cwd(:55), `Spacer`(:56), "Saved decision: …"(:57-66),
/// "Current session: …"(:67-69), `Spacer`(:70), the option list(:72-73), `Spacer`(:74), the
/// hint(:75-85), `Spacer`(:86), `DynamicBorder`(:87). **Five** blank rows; cyrup drew two of them
/// (`:70` and `:74`).
#[test]
fn trust_envelope_has_all_five_upstream_spacer_rows() {
    let mut sel = trust(None);
    let rows = natural(sel.as_mut_selector(), 60);
    let n = rows.len();
    assert!(is_rule(&rows[0]), "DynamicBorder (:52): {rows:?}");
    assert_eq!(rows[1], "", "Spacer(1) (:53): {rows:?}");
    assert!(rows[2].contains("Project trust"), "title (:54): {rows:?}");
    assert!(rows[3].contains("/home/me/project"), "the cwd is ADJACENT to the title (:55): {rows:?}");
    assert_eq!(rows[4], "", "Spacer(1) (:56) — S20's 'cwd runs straight into Saved decision': {rows:?}");
    assert!(rows[5].contains("Saved decision:"), "(:57-66): {rows:?}");
    assert!(rows[6].contains("Current session:"), "ADJACENT to it (:67-69): {rows:?}");
    assert_eq!(rows[7], "", "Spacer(1) (:70): {rows:?}");
    assert!(rows[8].contains("Trust"), "the option list (:72-73): {rows:?}");

    assert!(is_rule(&rows[n - 1]), "DynamicBorder (:87): {rows:?}");
    assert_eq!(rows[n - 2], "", "Spacer(1) (:86): {rows:?}");
    assert!(rows[n - 3].contains("navigate"), "the hint row (:75-85): {rows:?}");
    assert_eq!(rows[n - 4], "", "Spacer(1) (:74): {rows:?}");
    assert_eq!(rows.iter().filter(|r| r.is_empty()).count(), 5, "exactly five: {rows:?}");
}

/// S20. `const checkmark = isCurrent ? theme.fg("success", " ✓") : ""` (`trust-selector.ts:110`),
/// joined as `` `${prefix}${label}${checkmark}` `` at `:113`. It marks the option matching the
/// PERSISTED decision, not the highlighted one, so with the cursor on row 0 and the saved decision
/// on row 2 the `✓` sits on row 2 and the `→` on row 0.
#[test]
fn trust_marks_the_saved_decision_with_a_success_checkmark() {
    let mut sel = trust(Some(2));
    let rows = natural(sel.as_mut_selector(), 60);
    let marked: Vec<&String> = rows.iter().filter(|r| r.contains('✓')).collect();
    assert_eq!(marked.len(), 1, "exactly one option carries the checkmark: {rows:?}");
    assert!(
        marked[0].contains("Do not trust") && marked[0].ends_with('✓'),
        "the checkmark is appended to the SAVED option's label: {rows:?}"
    );
    assert!(
        !marked[0].contains('→'),
        "and it is independent of the cursor, which is still on row 0: {rows:?}"
    );
    let cursor: Vec<&String> = rows.iter().filter(|r| r.contains('→')).collect();
    assert_eq!(cursor.len(), 1);
    assert!(cursor[0].contains("Trust") && !cursor[0].contains('✓'));
}

/// MIRROR. `isSavedOption` is false for every option when nothing is persisted
/// (`this.savedDecision?.decision === option.trusted` on a `null` decision), so `checkmark` is `""`
/// everywhere — a preselected row 0 is NOT evidence of a saved decision
/// (`selectedIndex = Math.max(0, findIndex(...))`, `:45-48`).
#[test]
fn trust_draws_no_checkmark_when_nothing_is_saved() {
    let mut sel = trust(None);
    let rows = natural(sel.as_mut_selector(), 60);
    assert!(
        !rows.iter().any(|r| r.contains('✓')),
        "no saved decision ⇒ no marker anywhere: {rows:?}"
    );
    assert!(rows.iter().any(|r| r.contains('→')), "the cursor is still drawn: {rows:?}");
}

#[test]
fn trust_envelope_is_a_prefix_on_a_short_slot() {
    assert_short_slot_is_a_prefix("trust", || Box::new(trust(Some(1))));
}

// ---------------------------------------------------------------------------------------------
// L4 — `/model` (`model-selector.ts`)
// ---------------------------------------------------------------------------------------------

fn model_selector() -> ModelSelector {
    ModelSelector::new(vec![ModelEntry {
        id: "claude-opus".to_string(),
        name: "Opus".to_string(),
        provider: "anthropic".to_string(),
        current: true,
        scoped: false,
    }])
}

/// `ModelSelectorComponent` (`model-selector.ts:92-129`): `DynamicBorder`(:92), `Spacer`(:93), the
/// scope/warning `Text`(:96-104), `Spacer`(:105), the search `Input`(:118), `Spacer`(:120), the
/// list(:124), `Spacer`(:126), `DynamicBorder`(:129). cyrup already drew `:105` and `:120`; `:93`
/// and `:126` are what this adds.
#[test]
fn model_selector_envelope_has_the_four_upstream_spacer_rows() {
    let mut sel = model_selector();
    let rows = natural(sel.as_mut_selector(), 70);
    let n = rows.len();
    assert!(is_rule(&rows[0]), "DynamicBorder (:92): {rows:?}");
    assert_eq!(rows[1], "", "Spacer(1) after the top border (:93): {rows:?}");
    // This fixture has no scoped models, so `:101-104`'s `else` branch draws the warning `Text`
    // alone — no scope line, no `getScopeHintText` row (S30). It is a `Text` like any other, so it
    // WRAPS at the dialog width (`text.ts:60-87`) instead of being clipped: the string is 75
    // columns and this terminal is 70.
    assert_eq!(
        rows[2], "Only showing models from configured providers. Use /login to add",
        "the warning `Text` (:102-103), flush at column 0 (S32): {rows:?}"
    );
    assert_eq!(rows[3], "providers.", "…wrapped, not truncated: {rows:?}");
    assert_eq!(rows[4], "", "Spacer(1) after the whole scope block (:105): {rows:?}");
    assert!(is_rule(&rows[n - 1]), "DynamicBorder (:129): {rows:?}");
    assert_eq!(rows[n - 2], "", "Spacer(1) before the bottom border (:126): {rows:?}");
}

#[test]
fn model_selector_envelope_is_a_prefix_on_a_short_slot() {
    assert_short_slot_is_a_prefix("model", || Box::new(model_selector()));
}

// ---------------------------------------------------------------------------------------------
// L4 — `/resume` (`session-selector.ts` `buildBaseLayout`)
// ---------------------------------------------------------------------------------------------

fn session_selector() -> SessionSelector {
    SessionSelector::new(vec![SessionRow {
        path: "/s/a.jsonl".to_string(),
        label: "Build pipeline".to_string(),
        name: Some("Build pipeline".to_string()),
        desc: Some("3 msgs".to_string()),
        search_text: "build pipeline".to_string(),
        recency: 1,
    }])
}

/// `buildBaseLayout` (`session-selector.ts:735-747`): `Spacer`(:737), `DynamicBorder`(:738),
/// `Spacer`(:739), the header(:741), `Spacer`(:742), the content(:744), `Spacer`(:745),
/// `DynamicBorder`(:746). The FIRST spacer sits **above** the top rule — this envelope opens with a
/// blank row, which the extension/oauth/trust envelopes do not.
#[test]
fn session_selector_envelope_opens_with_a_spacer_above_its_top_rule() {
    let mut sel = session_selector();
    let rows = natural(sel.as_mut_selector(), 70);
    let n = rows.len();
    assert_eq!(rows[0], "", "Spacer(1) ABOVE the top border (:737): {rows:?}");
    assert!(is_rule(&rows[1]), "DynamicBorder (:738): {rows:?}");
    assert_eq!(rows[2], "", "Spacer(1) after it (:739): {rows:?}");
    assert!(rows[3].contains("Resume Session"), "the header's line 1 of 3 (:185): {rows:?}");
    assert!(is_rule(&rows[n - 1]), "DynamicBorder (:746): {rows:?}");
    assert_eq!(rows[n - 2], "", "Spacer(1) before it (:745): {rows:?}");
}

/// The header child (`:741`) is a `SessionSelectorHeader`, and its `render` returns **THREE** lines
/// — `[titleRow, hintLine1, hintLine2]` (`session-selector.ts:185`). Only then comes
/// `buildBaseLayout`'s `Spacer(1)` (`:742`), and only then the content child (`:744`), which is
/// `SessionList`, whose OWN first two lines are the search `Input` and a blank (`:418-419` —
/// `lines.push("")`, "Blank line after search").
///
/// cyrup put the `:742` blank and the search box where hint1/hint2 belong and moved the hints to
/// the bottom of the body, shifting every row below the header by two.
#[test]
fn session_selector_header_is_title_then_both_hint_rows_then_the_spacer() {
    let mut sel = session_selector();
    let rows = natural(sel.as_mut_selector(), 70);
    assert_eq!(rows[0], "", "Spacer(1) ABOVE the top border (:737): {rows:?}");
    assert!(is_rule(&rows[1]), "DynamicBorder (:738): {rows:?}");
    assert_eq!(rows[2], "", "Spacer(1) (:739): {rows:?}");
    assert!(rows[3].contains("Resume Session"), "the header's line 1 (:185): {rows:?}");
    assert!(
        rows[4].starts_with("tab scope · re:<pattern> regex"),
        "hintLine1, upstream's own text (:169-170): {rows:?}"
    );
    assert!(
        rows[5].starts_with("ctrl+s sort · ctrl+n named · ctrl+d delete · ctrl+p path (off)"),
        "hintLine2 (:171-180): {rows:?}"
    );
    assert_eq!(rows[6], "", "Spacer(1) BETWEEN header and content (:742): {rows:?}");
    // S31: `Input.render`'s prompt is an unstyled `"> "` at column **0** (`input.ts:380`), and
    // `SessionList` splices the `Input`'s own lines in unmodified (`:418`), so nothing insets it.
    // This used to assert `" >"` — cyrup's accent three-column invention. (`natural` trims the
    // row, so the caret cell after the prompt is not visible here.)
    assert!(rows[7].starts_with('>'), "SessionList's search Input (:418): {rows:?}");
    assert_eq!(rows[8], "", "SessionList's own blank after it (:419): {rows:?}");
    assert!(rows[9].contains("Build pipeline"), "then the session rows: {rows:?}");
    // And nothing repeats the hints below the list any more.
    assert_eq!(
        rows.iter().filter(|r| r.contains("scope")).count(),
        1,
        "the hints live in the header ONLY: {rows:?}"
    );
}

#[test]
fn session_selector_envelope_is_a_prefix_on_a_short_slot() {
    assert_short_slot_is_a_prefix("session", || Box::new(session_selector()));
}

// ---------------------------------------------------------------------------------------------
// L4 — `cyrup config` (`config-selector.ts`)
// ---------------------------------------------------------------------------------------------

fn config_selector() -> ConfigSelector {
    ConfigSelector::new(vec![ConfigRow {
        scope: ConfigScope::User,
        kind: ConfigKind::Skills,
        display_name: "review".to_string(),
        pattern: "skills/review/SKILL.md".to_string(),
        base_dir: "/home/me/.cyrup".to_string(),
        enabled: true,
    }])
}

/// `ConfigSelectorComponent` (`config-selector.ts:901-930`): `Spacer`(:901), `DynamicBorder`(:902),
/// `Spacer`(:903), the header(:905), `Spacer`(:906), the resource list(:926), `Spacer`(:929),
/// `DynamicBorder`(:930) — **four**, none of which cyrup drew, and again the first is above the
/// rule.
///
/// S17: the header at `:905` renders **two** lines, not one (`ConfigSelectorHeader.render`,
/// `:202-218`), and `ResourceList.render` opens with its own search `Input` + blank (`:396-397`).
#[test]
fn config_selector_envelope_has_the_four_upstream_spacer_rows() {
    let mut sel = config_selector();
    let rows = natural(sel.as_mut_selector(), 70);
    let n = rows.len();
    assert_eq!(rows[0], "", "Spacer(1) ABOVE the top border (:901): {rows:?}");
    assert!(is_rule(&rows[1]), "DynamicBorder (:902): {rows:?}");
    assert_eq!(rows[2], "", "Spacer(1) (:903): {rows:?}");
    // Header row 1 (:203-209,216): the bold title, then right-aligned hints. `tab switch mode` is
    // absent because `projectModeAvailable` is off by default (:205).
    assert!(rows[3].starts_with("Global Resources"), "header row 1 title (:203): {rows:?}");
    assert!(rows[3].ends_with("space toggle · esc close"), "right-aligned hint (:208): {rows:?}");
    assert!(!rows[3].contains("switch mode"), "no tab hint without project mode (:205): {rows:?}");
    // Header row 2 (:210-213,217): which settings file is being written.
    assert_eq!(rows[4], "~/.cyrup/agent/settings.json", "header row 2 (:213): {rows:?}");
    assert_eq!(rows[5], "", "Spacer(1) (:906): {rows:?}");
    assert_eq!(rows[6], ">", "ResourceList's search Input (:396): {rows:?}");
    assert_eq!(rows[7], "", "the blank ResourceList pushes under it (:397): {rows:?}");
    assert!(rows[8].contains("User"), "the resource list starts (:926): {rows:?}");
    assert!(is_rule(&rows[n - 1]), "DynamicBorder (:930): {rows:?}");
    assert_eq!(rows[n - 2], "", "Spacer(1) (:929): {rows:?}");
}

/// S17 — the header's project-scope arm, and the `tab switch mode` hint that only appears when
/// `projectModeAvailable` (`config-selector.ts:205`).
#[test]
fn config_selector_header_switches_title_hint_and_scope_path_with_the_write_scope() {
    let mut sel = config_selector();
    sel.set_project_mode_available(true);
    let global = natural(sel.as_mut_selector(), 90);
    assert!(global[3].starts_with("Global Resources"), "{global:?}");
    assert!(
        global[3].ends_with("tab switch mode · space toggle · esc close"),
        "all three hints, joined by ` · ` (:204-208): {global:?}"
    );
    assert_eq!(global[4], "~/.cyrup/agent/settings.json", "(:213): {global:?}");

    sel.set_write_scope(crate::ConfigWriteScope::Project);
    let project = natural(sel.as_mut_selector(), 90);
    assert!(project[3].starts_with("Project Local Resources"), "(:203): {project:?}");
    assert!(
        project[3].ends_with("tab switch mode · space cycle inherit/+/- · esc close"),
        "the project arm of the action hint (:207): {project:?}"
    );
    assert_eq!(
        project[4], ".cyrup/settings.json · inherited global resources are dimmed",
        "(:212): {project:?}"
    );
    // The hint is RIGHT-ALIGNED (`spacing = max(1, width - titleWidth - hintWidth)`, :209), not
    // pinned four columns after the title as cyrup used to draw it.
    assert_eq!(project[3].chars().count(), 90, "the row fills the width: {project:?}");
}

/// S17/S19 — `Tab` flips the write scope, but only when the chrome said project mode exists
/// (`config-selector.ts:495-498` + `:920-925`).
#[test]
fn config_selector_tab_switches_write_scope_only_when_project_mode_is_available() {
    let mut sel = config_selector();
    let keymap = crate::SelectKeymap::default();
    let tab = crate::crossterm::event::KeyEvent::new(
        crate::crossterm::event::KeyCode::Tab,
        crate::crossterm::event::KeyModifiers::NONE,
    );
    sel.handle(&tab, &keymap);
    assert_eq!(sel.write_scope(), crate::ConfigWriteScope::Global, "no project mode, no switch");

    sel.set_project_mode_available(true);
    sel.handle(&tab, &keymap);
    assert_eq!(sel.write_scope(), crate::ConfigWriteScope::Project);
    sel.handle(&tab, &keymap);
    assert_eq!(sel.write_scope(), crate::ConfigWriteScope::Global, "and back (:934)");
}

#[test]
fn config_selector_envelope_is_a_prefix_on_a_short_slot() {
    assert_short_slot_is_a_prefix("config", || Box::new(config_selector()));
}

/// A REALISTIC resource set: 40 skills/prompts/themes across both scopes, which flattens to well
/// over a hundred rows — far more than any terminal.
fn big_config_selector() -> ConfigSelector {
    let mut rows = Vec::new();
    for scope in [ConfigScope::User, ConfigScope::Project] {
        for kind in [ConfigKind::Skills, ConfigKind::Prompts, ConfigKind::Themes] {
            for i in 0..40 {
                rows.push(ConfigRow {
                    scope,
                    kind,
                    display_name: format!("res-{i:02}"),
                    pattern: format!("{}/res-{i:02}", kind.key()),
                    base_dir: match scope {
                        ConfigScope::User => "/home/me/.cyrup".to_string(),
                        ConfigScope::Project => "/repo/.cyrup".to_string(),
                    },
                    enabled: i % 2 == 0,
                });
            }
        }
    }
    ConfigSelector::new(rows)
}

/// The `/config` body is WINDOWED, and to upstream's window exactly.
///
/// `desired_height` used to be `flat.len() + 7` with no cap, so on any resource list worth opening
/// the dialog was arbitrarily taller than the terminal. Upstream's body is windowed:
/// `this.maxVisible = Math.max(5, (terminalHeight ?? 24) - chrome)` with `chrome = 8`
/// (`config-selector.ts:264-266`, fed `ui.terminal.rows` at `cli/config-selector.ts:47`), sliced at
/// `:405-409`. cyrup takes the same input through `Selector::set_terminal_height`.
///
/// S17: upstream's `chrome = 8` counts a **two-line** header and does **not** count the search
/// `Input` + blank `ResourceList.render` pushes at `:396-397`, nor the scroll readout at `:444-449`.
/// pi's dialog therefore overshoots its own terminal by exactly those three rows, and matching that
/// is the point: the visible rows have to be a prefix of PI's render, and a cyrup that "fixed" the
/// overshoot by shrinking the window would show a different number of resources than pi does.
#[test]
fn config_selector_windows_its_body_to_pis_window_including_pis_own_overshoot() {
    let mut sel = big_config_selector();
    assert!(sel.rows().len() >= 240, "a realistic list");

    for terminal_rows in [24u16, 30, 50] {
        sel.set_terminal_height(terminal_rows);
        assert_eq!(
            sel.max_visible(),
            terminal_rows - 8,
            "@{terminal_rows}: Math.max(5, terminalHeight - 8) (:264-266)"
        );
        let want = sel.desired_height(70);
        assert_eq!(
            want,
            terminal_rows + 3,
            "@{terminal_rows}: 8 chrome + 2 search rows (:396-397) + maxVisible + 1 scroll row \
             (:444-449) — pi's own three-row overshoot, wanted {want}"
        );
        // The host gives it `min(desired, terminal)`; render at the natural height to see the whole
        // envelope, which is what pi's `Container` produces before `layout.ts:113` clips it.
        let rows = rows_at(&mut sel, 70, want);
        let n = rows.len();
        assert_eq!(rows[0], "", "Spacer(1) ABOVE the top border (:901): {rows:?}");
        assert!(is_rule(&rows[1]), "DynamicBorder (:902): {rows:?}");
        assert_eq!(rows[2], "", "Spacer(1) (:903): {rows:?}");
        assert!(rows[3].starts_with("Global Resources"), "header row 1 (:203): {rows:?}");
        assert_eq!(rows[4], "~/.cyrup/agent/settings.json", "header row 2 (:213): {rows:?}");
        assert_eq!(rows[5], "", "Spacer(1) (:906): {rows:?}");
        assert_eq!(rows[6], ">", "the search Input (:396): {rows:?}");
        assert_eq!(rows[7], "", "the blank under it (:397): {rows:?}");
        assert!(rows[8].contains("User"), "the resource list starts (:926): {rows:?}");
        assert_eq!(rows[n - 2], "", "Spacer(1) (:929): {rows:?}");
        assert!(is_rule(&rows[n - 1]), "DynamicBorder (:930): {rows:?}");
        assert_eq!(
            rows.iter().filter(|r| r.is_empty()).count(),
            5,
            "the four envelope Spacers plus ResourceList's own blank (:397): {rows:?}"
        );
        // And the body really is windowed, not merely clipped: the list-row count is `maxVisible`.
        let list_rows = n - 8 - 2 - 1; // chrome, the search rows, the scroll readout
        assert_eq!(
            list_rows,
            usize::from(sel.max_visible()),
            "@{terminal_rows} rows: the body term is maxVisible"
        );
    }
}

/// `Math.max(5, …)` (`config-selector.ts:266`): the window never goes below five rows, so on a tiny
/// terminal the dialog is once again taller than the slot — and then, exactly as upstream, it is
/// clipped from the bottom rather than having its blanks removed.
#[test]
fn config_selector_window_floors_at_five_and_then_clips_like_pi() {
    let mut sel = big_config_selector();
    sel.set_terminal_height(8);
    assert_eq!(sel.max_visible(), 5, "the Math.max(5, …) floor");
    assert_eq!(
        sel.desired_height(70),
        16,
        "8 chrome (:264-265, two-line header) + 2 search rows (:396-397) + 5 body rows + 1 scroll \
         row (:444-449)"
    );
    let rows = rows_at(&mut sel, 70, 8);
    assert_eq!(rows[0], "", "still opens with :901's Spacer, not a rule: {rows:?}");
    assert!(is_rule(&rows[1]), "{rows:?}");
    assert_eq!(rows[2], "", "{rows:?}");
}

/// S18 + S19 — the project write scope is a DIFFERENT picture, and cyrup drew it identically to
/// global. `config-selector.ts:417-419` dims an inherited group and appends ` · inherited global`;
/// `:423` dims its subgroup; `:430-432` dims the item and drops its bold; `:639-647` swaps the
/// checkbox for a `success` `[+]` / `warning` `[-]` / dim `[x]`; `:649-655` appends
/// `  project load` / `  project unload` / `  inherited global`.
#[test]
fn config_selector_project_scope_dims_inherited_rows_and_shows_override_markers() {
    let mut sel = ConfigSelector::new(vec![
        ConfigRow {
            scope: ConfigScope::User,
            kind: ConfigKind::Skills,
            display_name: "inherited-skill".to_string(),
            pattern: "skills/inherited-skill/SKILL.md".to_string(),
            base_dir: "/home/me/.cyrup/agent".to_string(),
            enabled: true,
        },
        ConfigRow {
            scope: ConfigScope::User,
            kind: ConfigKind::Skills,
            display_name: "loaded-skill".to_string(),
            pattern: "skills/loaded-skill/SKILL.md".to_string(),
            base_dir: "/home/me/.cyrup/agent".to_string(),
            enabled: false,
        },
        ConfigRow {
            scope: ConfigScope::Project,
            kind: ConfigKind::Skills,
            display_name: "local-skill".to_string(),
            pattern: "skills/local-skill/SKILL.md".to_string(),
            base_dir: "/repo/.cyrup".to_string(),
            enabled: true,
        },
    ]);

    // Global scope: no suffixes, no ` · inherited global`, plain `[x]`/`[ ]`.
    let global = natural(sel.as_mut_selector(), 70);
    assert!(!global.iter().any(|r| r.contains("inherited global")), "{global:?}");
    assert!(!global.iter().any(|r| r.contains("project load")), "{global:?}");
    assert!(global.iter().any(|r| r.contains("[x] inherited-skill")), "(:646): {global:?}");

    sel.set_write_scope(crate::ConfigWriteScope::Project);
    sel.set_override_state(1, crate::ProjectOverrideState::Load);
    sel.set_override_state(2, crate::ProjectOverrideState::Unload);
    let project = natural(sel.as_mut_selector(), 70);

    // The user group header carries the tail; the project group header does not (:417-418).
    assert!(
        project.iter().any(|r| r.trim() == "User (/home/me/.cyrup/agent) · inherited global"),
        "(:418): {project:?}"
    );
    assert!(
        project.iter().any(|r| r.trim() == "Project (/repo/.cyrup)"),
        "a project-scope group is not inherited: {project:?}"
    );
    // Inherit + inherited-global ⇒ dim `[x]` plus the `  inherited global` suffix (:644, :654).
    assert!(
        project.iter().any(|r| r.contains("[x] inherited-skill  inherited global")),
        "(:644,:654): {project:?}"
    );
    // A forced load reports `[+]` and `  project load` even though the resource is disabled
    // globally (:642, :652) — the checkbox tracks the OVERRIDE, not the resolved enable.
    assert!(
        project.iter().any(|r| r.contains("[+] loaded-skill  project load")),
        "(:642,:652): {project:?}"
    );
    // A forced unload reports `[-]` and `  project unload` (:643, :653), and a project-scope row
    // is never "inherited global".
    assert!(
        project.iter().any(|r| r.contains("[-] local-skill  project unload")),
        "(:643,:653): {project:?}"
    );
}

/// S18 is a **colour** defect, so assert the colour: `config-selector.ts:419` picks
/// `theme.fg(inherited ? "dim" : "accent", …)` for the group, `:423` `dim` vs `muted` for the
/// subgroup, `:432` wraps the item name in `dim`. cyrup's group was unconditionally accent and its
/// subgroup unconditionally muted, so in project scope an inherited resource was indistinguishable
/// from a project-local one.
#[test]
fn config_selector_inherited_rows_are_dim_in_project_scope_and_accent_in_global() {
    let mut sel = ConfigSelector::new(vec![ConfigRow {
        scope: ConfigScope::User,
        kind: ConfigKind::Skills,
        display_name: "review".to_string(),
        pattern: "skills/review/SKILL.md".to_string(),
        base_dir: "/home/me/.cyrup/agent".to_string(),
        enabled: true,
    }]);
    let theme = UiTheme::dark();
    let dim = theme.dim_style().fg.unwrap();
    let accent = theme.accent_style().fg.unwrap();
    let muted = theme.muted_style().fg.unwrap();
    assert_ne!(dim, accent, "the two colours must actually differ in this theme");
    assert_ne!(dim, muted);

    // (row index of the group / subgroup / item, colour of a glyph inside each)
    let colours = |sel: &mut ConfigSelector| {
        let h = sel.desired_height(70);
        let mut term = Terminal::new(TestBackend::new(70, h)).unwrap();
        term.draw(|f| sel.render(f, f.area(), &theme)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut out = Vec::new();
        for y in 0..buf.area.height {
            let mut s = String::new();
            for x in 0..buf.area.width {
                s.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            if s.trim_start().starts_with("User (") {
                out.push(("group", buf.cell((2, y)).unwrap().fg));
            } else if s.trim() == "Skills" {
                out.push(("subgroup", buf.cell((4, y)).unwrap().fg));
            } else if s.contains("review") {
                let x = s.find("review").unwrap() as u16;
                out.push(("item", buf.cell((x, y)).unwrap().fg));
            }
        }
        out
    };

    let global = colours(&mut sel);
    assert_eq!(global, vec![("group", accent), ("subgroup", muted), ("item", theme.base_style().fg.unwrap())]);

    sel.set_write_scope(crate::ConfigWriteScope::Project);
    let project = colours(&mut sel);
    assert_eq!(
        project,
        vec![("group", dim), ("subgroup", dim), ("item", dim)],
        "an inherited resource is dim in project scope (:419, :423, :432)"
    );
}

/// S19 — long resource names are truncated with a REAL ellipsis (`truncateToWidth(row, width,
/// "...")`, `config-selector.ts:434-437`). cyrup made no truncation call at all, so the name
/// hard-clipped at the frame edge with no indication anything was cut.
#[test]
fn config_selector_truncates_long_rows_with_an_ellipsis() {
    let mut sel = ConfigSelector::new(vec![ConfigRow {
        scope: ConfigScope::User,
        kind: ConfigKind::Skills,
        display_name: "a".repeat(80),
        pattern: "skills/long/SKILL.md".to_string(),
        base_dir: "/home/me/.cyrup/agent".to_string(),
        enabled: true,
    }]);
    let rows = natural(sel.as_mut_selector(), 40);
    let item = rows
        .iter()
        .find(|r| r.contains("[x]"))
        .unwrap_or_else(|| panic!("no item row: {rows:?}"));
    assert_eq!(item.chars().count(), 40, "truncated to the width: {item:?}");
    assert!(item.ends_with("..."), "with the `...` ellipsis (:437): {item:?}");
}

/// S17 fix #43 — the empty state is `theme.fg("muted", …)` (`config-selector.ts:400`), which cyrup
/// drew dim. Asserted through the style, since the text was already right.
#[test]
fn config_selector_empty_state_is_muted_not_dim() {
    let mut sel = ConfigSelector::new(Vec::new());
    let theme = UiTheme::dark();
    let mut term = Terminal::new(TestBackend::new(40, sel.desired_height(40))).unwrap();
    term.draw(|f| sel.render(f, f.area(), &theme)).unwrap();
    let buf = term.backend().buffer().clone();
    let (y, _) = (0..buf.area.height)
        .map(|y| {
            let mut s = String::new();
            for x in 0..buf.area.width {
                s.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            (y, s)
        })
        .find(|(_, s)| s.contains("No resources found"))
        .unwrap_or_else(|| panic!("no empty state row"));
    let cell = buf.cell((2, y)).unwrap();
    assert_eq!(cell.fg, theme.muted_style().fg.unwrap(), "muted (:400), not dim");
    assert_ne!(cell.fg, theme.dim_style().fg.unwrap());
}

/// S19 — the scroll readout counts ITEMS, not flat entries (`config-selector.ts:445-448`).
/// Group/subgroup headers are in `filteredItems` too; counting those would report a resource index
/// no user could reconcile with what they see.
#[test]
fn config_selector_scroll_row_counts_resources_not_headers() {
    let mut sel = big_config_selector();
    sel.set_terminal_height(24); // maxVisible = 16, far below the flat length
    let want = sel.desired_height(70);
    let rows = rows_at(&mut sel, 70, want);
    let scroll = rows
        .iter()
        .find(|r| r.starts_with("  (") && r.ends_with(')'))
        .unwrap_or_else(|| panic!("no scroll readout: {rows:?}"));
    // 240 resources across both scopes; the highlight starts on the first one.
    assert_eq!(scroll, "  (1/240)", "`  (${{currentItemIndex}}/${{itemCount}})` (:448): {rows:?}");
}

// ---------------------------------------------------------------------------------------------
// E6 + E7 — the extension `ui.input` dialog (`extension-input.ts`)
// ---------------------------------------------------------------------------------------------

/// `ExtensionInputComponent` (`extension-input.ts:47-70`): `DynamicBorder`(:47), `Spacer`(:48), the
/// title(:50-51), `Spacer`(:52), the `Input`(:63-64), `Spacer`(:65), the
/// `submit`/`cancel` hint(:66-68), `Spacer`(:69), `DynamicBorder`(:70) — nine rows. cyrup drew
/// four: rule, title, field, rule.
#[test]
fn extension_input_envelope_has_four_spacers_and_the_hint_row() {
    let mut sel = TextInputSelector::new("Name?".to_string(), None);
    let rows = natural(&mut sel, 60);
    assert_eq!(rows.len(), 9, "nine rows, one per child: {rows:?}");
    assert!(is_rule(&rows[0]), "DynamicBorder (:47): {rows:?}");
    assert_eq!(rows[1], "", "Spacer(1) (:48): {rows:?}");
    assert!(rows[2].contains("Name?"), "the title (:50-51): {rows:?}");
    assert_eq!(rows[3], "", "Spacer(1) (:52): {rows:?}");
    // E10: `Input.render` opens with `const prompt = "> ";` (`input.ts:380`) — TWO columns, at
    // column 0. `ExtensionInputComponent` adds the `Input` as a bare child (`extension-input.ts:
    // 63-64`) with no `Text` wrapper, so nothing insets it; the title and hint rows are the only
    // children carrying `paddingX = 1`. cyrup drew a three-column accent `" > "`, one column in.
    // (The trailing space of `"> "` is the reverse-video caret cell and is trimmed off by the row
    // helper, so this pins the `>` at column 0 — E10's actual claim — not the pair's width.)
    assert!(rows[4].starts_with('>'), "the Input at column 0 (:63-64/input.ts:380): {rows:?}");
    assert_eq!(rows[5], "", "Spacer(1) (:65): {rows:?}");
    // E6: `keyHint("tui.select.confirm","submit")  keyHint("tui.select.cancel","cancel")`, in a
    // `new Text(..., 1, 0)` so it is inset one column.
    assert!(rows[6].starts_with(' '), "the hint is inset one column (paddingX = 1): {rows:?}");
    assert!(rows[6].contains("submit"), "the hint names submit (:67): {rows:?}");
    assert!(rows[6].contains("cancel"), "…and cancel (:67): {rows:?}");
    assert!(rows[6].contains("enter"), "…resolved through the live keymap: {rows:?}");
    assert_eq!(rows[7], "", "Spacer(1) (:69): {rows:?}");
    assert!(is_rule(&rows[8]), "DynamicBorder (:70): {rows:?}");
}

/// MIRROR. The hint row is this component's own, not `ListSelector`'s three-pair
/// `navigate/select/cancel` row — `ExtensionInputComponent` has nothing to navigate and
/// `extension-input.ts:66-68` calls `keyHint` exactly twice.
#[test]
fn extension_input_hint_is_two_pairs_not_the_list_selector_row() {
    let mut sel = TextInputSelector::new("Name?".to_string(), None);
    let rows = natural(&mut sel, 60);
    assert!(
        !rows.iter().any(|r| r.contains("navigate")),
        "no `↑↓ navigate` pair on a single-field input: {rows:?}"
    );
}

#[test]
fn extension_input_envelope_is_a_prefix_on_a_short_slot() {
    assert_short_slot_is_a_prefix("extension input", || {
        Box::new(TextInputSelector::new("Name?".to_string(), None))
    });
}

/// What a five-row `ui.input` slot shows, spelled out against the constructor.
///
/// `ExtensionInputComponent`'s first five children are `DynamicBorder`(`extension-input.ts:47`),
/// `Spacer`(:48), the title(:50-51), `Spacer`(:52), the `Input`(:63-64), and pi paints exactly the
/// first five lines of the component into a five-row box (`packages/tui/src/layout.ts:113`,
/// `:307-310`) — so all five are visible, blanks included, and the hint row and bottom border are
/// what a short terminal costs.
///
/// This REPLACES an assertion that the same five rows are `[rule, title, field, …]` with "not one
/// of the five rows is spent on a spacer". That described the all-or-nothing spacer gate this batch
/// removes, and it is the opposite of upstream: pi cannot drop `:48` or `:52`, because a `Container`
/// has no height input to drop them on.
#[test]
fn extension_input_shows_its_first_five_children_on_a_five_row_slot() {
    let mut sel = TextInputSelector::new("Name?".to_string(), None);
    let rows = rows_at(&mut sel, 60, 5);
    assert!(is_rule(&rows[0]), "DynamicBorder (:47): {rows:?}");
    assert_eq!(rows[1], "", "Spacer(1) (:48): {rows:?}");
    assert!(rows[2].contains("Name?"), "the title (:50-51): {rows:?}");
    assert_eq!(rows[3], "", "Spacer(1) (:52): {rows:?}");
    assert!(rows[4].starts_with('>'), "the input FIELD (:63-64): {rows:?}");
}

/// E6, first paint. `keyHint` resolves through `keyText` → `getKeybindings().getKeys(...)` on every
/// render (`keybinding-hints.ts:34-44`), against the app's one live table — there is no window in
/// which upstream shows a stock default. cyrup's hint row was built from `SelectKeymap::default()`
/// and only corrected inside `handle`, so the very first frame of a `ui.input` dialog — the frame
/// whose entire job is telling the user how to submit — named `enter`/`escape` even for a user who
/// had rebound them. `with_keymap` is what the construction site (`app.rs`, `UiKind::Input`) now
/// passes the live table through.
#[test]
fn extension_input_hint_names_the_users_own_keys_on_the_first_paint() {
    let mut rebound = SelectKeymap::default();
    rebound.set_action(SelectAction::Confirm, vec![Key::ctrl('j')]);
    rebound.set_action(SelectAction::Cancel, vec![Key::ctrl('q')]);

    let mut stock = TextInputSelector::new("Name?".to_string(), None);
    let stock_rows = natural(&mut stock, 60);
    assert!(stock_rows[6].contains("enter"), "baseline: the stock table says enter: {stock_rows:?}");

    let mut sel = TextInputSelector::new("Name?".to_string(), None).with_keymap(&rebound);
    // No `handle` call — this is the FIRST paint, before any keystroke.
    let rows = natural(&mut sel, 60);
    assert!(rows[6].contains("ctrl+j"), "submit names the rebound key (:67): {rows:?}");
    assert!(rows[6].contains("ctrl+q"), "cancel does too (:67): {rows:?}");
    assert!(
        !rows[6].contains("enter") && !rows[6].contains("esc"),
        "and the stock defaults are gone: {rows:?}"
    );
}

/// Helper: `Selector` is object-safe, so `&mut T` coerces — but `natural`/`rows_at` take
/// `&mut dyn Selector`, and a concrete `ListSelector` needs the reborrow spelled out.
trait AsMutSelector {
    fn as_mut_selector(&mut self) -> &mut dyn Selector;
}
impl<T: Selector> AsMutSelector for T {
    fn as_mut_selector(&mut self) -> &mut dyn Selector {
        self
    }
}

// ---------------------------------------------------------------------------------------------
// The clipping direction the envelope docs assert, pinned down
// ---------------------------------------------------------------------------------------------

/// The `Vec<Line>` envelopes (`/resume`, `/model`, `/settings`, `/scoped-models`, `/login`,
/// `/trust`) all hand their whole natural render to one `Paragraph` and rely on it to degrade the
/// same way `stack_rows` does. Four doc comments described that behaviour and two of them said the
/// opposite of the other two ("clips top-first" vs "clips bottom-first"), so this pins the
/// ratatui half of the claim directly rather than through a dialog:
///
/// **a `Paragraph` draws `lines[0..area.height]` and drops the TRAILING rows.** The visible rows
/// are a strict PREFIX of the natural render — which is exactly what batch 4 established about pi
/// (`packages/tui/src/layout.ts:113` allocates a shorter box over the already-rendered line array,
/// `:307-310` paints `box.lines[offset + row - box.rect.y]` from `offset = 0`), and what
/// `assert_short_slot_is_a_prefix` asserts per dialog.
#[test]
fn a_paragraph_keeps_the_first_rows_and_drops_the_rest() {
    use ratatui::text::Line;
    use ratatui::widgets::Paragraph;
    let lines: Vec<Line<'static>> = (0..6).map(|i| Line::from(format!("row{i}"))).collect();
    let mut term = Terminal::new(TestBackend::new(8, 3)).unwrap();
    term.draw(|f| f.render_widget(Paragraph::new(lines), f.area())).unwrap();
    let buf = term.backend().buffer().clone();
    let rows: Vec<String> = (0..3)
        .map(|y| {
            (0..8).map(|x| buf[(x, y)].symbol()).collect::<String>().trim_end().to_string()
        })
        .collect();
    assert_eq!(rows, vec!["row0", "row1", "row2"], "the FIRST rows survive, not the last");
}

/// **`isInheritedGlobalItem`'s second arm** — `config-selector.ts:781-783`:
///
/// ```text
/// getItemScope(item) === "user" || this.inheritedEnabledByKey.has(this.getResourceItemKey(item))
/// ```
///
/// cyrup had reduced it to the scope test. `inheritedEnabledByKey` is keyed by
/// `` `${resourceType}:${canonicalizePath(path)}` `` (`:842-844`) over the **global** resolve
/// (`:262`, `:281-291`) — a resolve upstream runs separately, with `projectTrusted: false`
/// (`package-manager-cli.ts:655-660`) — so a **project**-scope row whose file that resolve also
/// reaches is inherited too. Without the arm such a row loses both the `  inherited global` suffix
/// (`:654`) and the dim state (`:657-663`) and reads as project-local.
#[test]
fn config_selector_marks_a_project_row_present_in_the_global_resolve_as_inherited() {
    let row = ConfigRow {
        scope: ConfigScope::Project,
        kind: ConfigKind::Skills,
        display_name: "shared-skill".to_string(),
        pattern: "skills/shared-skill/SKILL.md".to_string(),
        base_dir: "/repo/.cyrup".to_string(),
        enabled: true,
    };
    let key = ConfigSelector::resource_key(&row);
    assert_eq!(
        key, "skills:/repo/.cyrupskills/shared-skill/SKILL.md",
        "`${{resourceType}}:${{path}}` (:842-844), the path rejoined from base_dir + pattern"
    );

    // Without the global resolve knowing about it: project-local, no suffix, not dim.
    let mut plain = ConfigSelector::new(vec![row.clone()]);
    plain.set_write_scope(crate::ConfigWriteScope::Project);
    let rows = natural(plain.as_mut_selector(), 70);
    assert!(
        rows.iter().any(|r| r.contains("[x] shared-skill") && !r.contains("inherited global")),
        "a project row absent from the global resolve stays local: {rows:?}"
    );

    // With it: the same row is inherited-global — suffix and dim.
    let mut sel = ConfigSelector::new(vec![row]);
    sel.set_write_scope(crate::ConfigWriteScope::Project);
    sel.set_inherited_global_keys([key]);
    let rows = natural(sel.as_mut_selector(), 70);
    assert!(
        rows.iter().any(|r| r.contains("[x] shared-skill  inherited global")),
        "the OR arm must reach the suffix (:654): {rows:?}"
    );

    let theme = UiTheme::dark();
    let dim = theme.dim_style().fg.unwrap();
    let mut term = Terminal::new(TestBackend::new(70, sel.desired_height(70))).unwrap();
    term.draw(|f| sel.render(f, f.area(), &theme)).unwrap();
    let buf = term.backend().buffer().clone();
    let y = (0..buf.area.height)
        .find(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, *y)].symbol())
                .collect::<String>()
                .contains("shared-skill")
        })
        .expect("the resource row");
    let x = (0..buf.area.width)
        .find(|x| buf[(*x, y)].symbol() == "s")
        .expect("the label's first cell");
    assert_eq!(buf[(x, y)].fg, dim, "and the dim state (:657-663, :430-432)");
}
