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
//! (`packages/tui/src/components/select-list.ts`) emits no blank rows, and three of the components
//! that host one — `thinking-selector.ts:42,66,69`, `show-images-selector.ts:25,41,44`,
//! `theme-selector.ts:35,58,61` — are `DynamicBorder`/list/`DynamicBorder` and nothing else, while
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
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::{
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

/// MIRROR. `ThinkingSelectorComponent` is `DynamicBorder`(`thinking-selector.ts:42`) +
/// `SelectList`(:66) + `DynamicBorder`(:69) — **zero** spacers. The blank rows above must come from
/// the per-kind gate, never from `ListSelector` itself; if they migrate into the shared engine this
/// test goes red while the one above stays green.
#[test]
fn thinking_selector_envelope_stays_flush_against_its_rules() {
    let mut sel = ListSelector::thinking("medium")
        .with_upstream_chrome(SelectorKind::Thinking, &SelectKeymap::default());
    let rows = natural(sel.as_mut_selector(), 60);
    let n = rows.len();
    assert!(is_rule(&rows[0]), "top rule: {rows:?}");
    assert!(rows[1].contains("off"), "the first level row is flush against the rule: {rows:?}");
    assert!(is_rule(&rows[n - 1]), "bottom rule: {rows:?}");
    assert!(rows[n - 2].contains("max"), "the last level row is flush against it: {rows:?}");
    assert_eq!(
        rows.iter().filter(|r| r.is_empty()).count(),
        0,
        "thinking draws no Spacer(1) at all: {rows:?}"
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
/// `Spacer`(:131), title(:132), subtitle(:133), `Spacer`(:136), search `Input`(:140),
/// `Spacer`(:141), list(:145), `Spacer`(:148), footer(:154), `DynamicBorder`(:156). Three of its
/// four spacers land here (`:141` belongs to the unported search `Input`), and note the footer sits
/// **flush** against the bottom border — unlike `extension-selector.ts:74`, this component has no
/// spacer there.
#[test]
fn scoped_models_envelope_has_three_spacers_and_a_flush_footer() {
    let mut sel = scoped_models();
    let rows = natural(sel.as_mut_selector(), 60);
    let n = rows.len();
    assert_eq!(rows[1], "", "Spacer(1) after the top border (:131): {rows:?}");
    assert!(rows[2].contains("Scoped Models"), "title (:132): {rows:?}");
    assert_eq!(rows[3], "", "Spacer(1) after the title (:136): {rows:?}");
    assert!(is_rule(&rows[n - 1]), "bottom border (:156): {rows:?}");
    assert!(rows[n - 2].contains("toggle"), "the footer is FLUSH against it (:154): {rows:?}");
    assert_eq!(rows[n - 3], "", "Spacer(1) between list and footer (:148): {rows:?}");
    assert_eq!(rows.iter().filter(|r| r.is_empty()).count(), 3, "three blanks: {rows:?}");
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
/// no `Spacer` anywhere — and `SettingsList.renderMainList`
/// (`packages/tui/src/components/settings-list.ts:90-166`) emits none around the rows either.
/// `/settings` must stay flush.
#[test]
fn settings_selector_envelope_draws_no_spacer_rows() {
    let mut sel = SettingsSelector::new(
        "Settings",
        vec![SettingRow::toggle("terminal.showImages", "Show images", true)],
    );
    let rows = natural(sel.as_mut_selector(), 60);
    assert!(is_rule(&rows[0]), "top rule: {rows:?}");
    assert!(rows[1].contains("Settings"), "the title is flush against the rule: {rows:?}");
    assert_eq!(
        rows.iter().filter(|r| r.is_empty()).count(),
        0,
        "upstream `/settings` has no Spacer children: {rows:?}"
    );
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
    assert!(full[1].contains("Settings"), "the title: {full:?}");
    assert!(full[2].contains("Show images"), "the first settings row: {full:?}");

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
            !rows[0].contains("navigate"),
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
    assert!(!rows[2].is_empty(), "the scope header follows (:96-104): {rows:?}");
    assert_eq!(rows[3], "", "Spacer(1) (:105): {rows:?}");
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
    assert!(rows[3].contains("Resume Session"), "the header (:741): {rows:?}");
    assert!(is_rule(&rows[n - 1]), "DynamicBorder (:746): {rows:?}");
    assert_eq!(rows[n - 2], "", "Spacer(1) before it (:745): {rows:?}");
}

/// `buildBaseLayout` puts a `Spacer(1)` at `session-selector.ts:742` between the header child
/// (`:741`) and the content child (`:744`), and the content child is `SessionList`, whose OWN first
/// three lines are the search `Input`, a blank (`:418-419` — `lines.push("")`, "Blank line after
/// search") and then the rows.
///
/// cyrup rendered the search box immediately under the title, i.e. `:742` was missing, and the
/// blank it did draw below the input was `:419` — a different `Spacer` that does not discharge it.
/// Three of four, reported as four of four.
#[test]
fn session_selector_has_a_blank_between_its_header_and_its_search_box() {
    let mut sel = session_selector();
    let rows = natural(sel.as_mut_selector(), 70);
    assert_eq!(rows[0], "", "Spacer(1) ABOVE the top border (:737): {rows:?}");
    assert!(is_rule(&rows[1]), "DynamicBorder (:738): {rows:?}");
    assert_eq!(rows[2], "", "Spacer(1) (:739): {rows:?}");
    assert!(rows[3].contains("Resume Session"), "the header (:741): {rows:?}");
    assert_eq!(rows[4], "", "Spacer(1) BETWEEN header and content (:742): {rows:?}");
    assert!(rows[5].starts_with(" >"), "SessionList's search Input (:418): {rows:?}");
    assert_eq!(rows[6], "", "SessionList's own blank after it (:419): {rows:?}");
    assert!(rows[7].contains("Build pipeline"), "then the session rows: {rows:?}");
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
#[test]
fn config_selector_envelope_has_the_four_upstream_spacer_rows() {
    let mut sel = config_selector();
    let rows = natural(sel.as_mut_selector(), 70);
    let n = rows.len();
    assert_eq!(rows[0], "", "Spacer(1) ABOVE the top border (:901): {rows:?}");
    assert!(is_rule(&rows[1]), "DynamicBorder (:902): {rows:?}");
    assert_eq!(rows[2], "", "Spacer(1) (:903): {rows:?}");
    assert!(rows[3].contains("Resource Configuration"), "the header (:905): {rows:?}");
    assert_eq!(rows[4], "", "Spacer(1) (:906): {rows:?}");
    assert!(rows[5].contains("User"), "the resource list starts (:926): {rows:?}");
    assert!(is_rule(&rows[n - 1]), "DynamicBorder (:930): {rows:?}");
    assert_eq!(rows[n - 2], "", "Spacer(1) (:929): {rows:?}");
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

/// The `/config` spacers must actually REACH the screen on a real machine.
///
/// `desired_height` used to be `flat.len() + 7` with no cap, so on any resource list worth opening
/// the dialog was taller than the terminal, the host clamped the slot, and the four blank rows the
/// batch added were never drawn — the fix did not fix. Upstream never has that problem: its body is
/// windowed, `this.maxVisible = Math.max(5, (terminalHeight ?? 24) - chrome)`
/// (`config-selector.ts:264-266`, fed `ui.terminal.rows` at `cli/config-selector.ts:47`), sliced at
/// `:405-409`. cyrup takes the same input through `Selector::set_terminal_height`, which the
/// startup host calls every frame.
#[test]
fn config_selector_windows_its_body_so_the_whole_envelope_fits_the_terminal() {
    let mut sel = big_config_selector();
    assert!(sel.rows().len() >= 240, "a realistic list");

    for terminal_rows in [24u16, 30, 50] {
        sel.set_terminal_height(terminal_rows);
        let want = sel.desired_height(70);
        assert!(
            want <= terminal_rows,
            "@{terminal_rows} rows: the dialog must fit the terminal, wanted {want}"
        );
        // The host gives it `min(desired, terminal)` (`startup_selector.rs`), i.e. all of `want`.
        let rows = rows_at(&mut sel, 70, want);
        let n = rows.len();
        assert_eq!(rows[0], "", "Spacer(1) ABOVE the top border (:901): {rows:?}");
        assert!(is_rule(&rows[1]), "DynamicBorder (:902): {rows:?}");
        assert_eq!(rows[2], "", "Spacer(1) (:903): {rows:?}");
        assert!(rows[3].contains("Resource Configuration"), "the header (:905): {rows:?}");
        assert_eq!(rows[4], "", "Spacer(1) (:906): {rows:?}");
        assert!(rows[5].contains("User"), "the resource list starts (:926): {rows:?}");
        assert_eq!(rows[n - 2], "", "Spacer(1) (:929): {rows:?}");
        assert!(is_rule(&rows[n - 1]), "DynamicBorder (:930): {rows:?}");
        assert_eq!(
            rows.iter().filter(|r| r.is_empty()).count(),
            4,
            "all four blanks, and no accidental fifth: {rows:?}"
        );
        // And the body really is windowed, not merely clipped: the row count is `maxVisible`.
        assert_eq!(
            want - 7,
            sel.max_visible(),
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
    assert_eq!(sel.desired_height(70), 12, "5 body rows + 7 chrome");
    let rows = rows_at(&mut sel, 70, 8);
    assert_eq!(rows[0], "", "still opens with :901's Spacer, not a rule: {rows:?}");
    assert!(is_rule(&rows[1]), "{rows:?}");
    assert_eq!(rows[2], "", "{rows:?}");
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
