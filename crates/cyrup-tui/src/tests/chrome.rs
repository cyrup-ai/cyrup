//! Chrome-tail tests (spec/tui/01; Pi `keybinding-hints.ts` / `visual-truncate.ts` /
//! `bordered-loader.ts`) — TestBackend buffer assertions where rendered.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::{
    App, BorderedLoader, Keymap, UiTheme, compact_hints, format_key_text, truncate_to_visual_lines,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

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

#[test]
fn format_key_text_splits_chords_and_alternatives() {
    // `/` separates alternatives, `+` separates chord parts (formatKeyText, keybinding-hints.ts).
    assert_eq!(format_key_text("ctrl+c/ctrl+d", false), {
        // On macOS `alt`→`option`; neither part here is alt, so identity.
        "ctrl+c/ctrl+d".to_string()
    });
    // Capitalize title-cases each part.
    assert_eq!(format_key_text("ctrl+o", true), "Ctrl+O");
}

#[test]
fn format_key_text_rewrites_alt_to_option_on_macos() {
    let got = format_key_text("alt+d", false);
    if cfg!(target_os = "macos") {
        assert_eq!(got, "option+d");
    } else {
        assert_eq!(got, "alt+d");
    }
}

#[test]
fn compact_hints_source_keys_from_the_live_keymap() {
    let km = Keymap::default();
    let hints = compact_hints(&km);
    // Pi order: interrupt, clear/exit, /, !, more.
    let descs: Vec<&str> = hints.iter().map(|(_, d)| d.as_str()).collect();
    assert_eq!(
        descs,
        vec!["interrupt", "clear/exit", "commands", "bash", "more"]
    );
    // Defaults: Escape interrupt, Ctrl+C clear, Ctrl+D exit, Ctrl+O expand. The interrupt key spells
    // out as `escape`: upstream's id is `"app.interrupt": { defaultKeys: "escape" }` (v0.84.1
    // `coding-agent/src/core/keybindings.ts:66`) and `formatKeyText` (`keybinding-hints.ts:17-27`)
    // only splits on `/`+`+` and rewrites `alt`→`option` — it never abbreviates.
    assert_eq!(hints[0].0, "escape");
    assert_eq!(hints[1].0, "ctrl+c/ctrl+d");
    assert_eq!(hints[2].0, "/");
    assert_eq!(hints[4].0, "ctrl+o");
}

#[test]
fn truncate_keeps_last_n_visual_lines_and_reports_skipped() {
    let text = "a\nb\nc\nd\ne";
    let r = truncate_to_visual_lines(text, 2, 80);
    assert_eq!(r.lines, vec!["d".to_string(), "e".to_string()]);
    assert_eq!(r.skipped, 3);
}

#[test]
fn truncate_accounts_for_wrapping() {
    // A 30-char line wraps to three visual lines at width 10; with max 2 the first is skipped.
    let text = "x".repeat(30);
    let r = truncate_to_visual_lines(&text, 2, 10);
    assert_eq!(r.lines.len(), 2);
    assert_eq!(r.skipped, 1);
    assert!(r.lines.iter().all(|l| l.chars().count() == 10));
}

#[test]
fn truncate_no_op_when_under_limit() {
    let r = truncate_to_visual_lines("one\ntwo", 5, 80);
    assert_eq!(r.skipped, 0);
    assert_eq!(r.lines, vec!["one".to_string(), "two".to_string()]);
}

#[test]
fn bordered_loader_renders_message_cancel_hint_and_rules() {
    let theme = UiTheme::dark();
    let loader = BorderedLoader::cancellable("Working on it", "escape/ctrl+c");
    // 7 rows: `DynamicBorder` + `Loader` (2 — `["", ...super.render(width)]`, v0.84.1
    // `tui/src/components/loader.ts:43-45`) + `Spacer(1)` + `Text(keyHint, 1, 0)` + `Spacer(1)` +
    // `DynamicBorder` (`coding-agent/src/modes/interactive/components/bordered-loader.ts:16-39`).
    assert_eq!(loader.height(), 7);
    let mut terminal = Terminal::new(TestBackend::new(40, 7)).unwrap();
    terminal
        .draw(|f| loader.render(f, Rect::new(0, 0, 40, 7), &theme, 0))
        .unwrap();
    let text = buf_string(&terminal);
    assert!(text.contains("Working on it"), "message: {text}");
    assert!(text.contains("cancel"), "cancel hint: {text}");
    assert!(text.contains('─'), "border rule: {text}");
    assert!(text.contains('⠋'), "spinner frame 0: {text}");
}

#[test]
fn plain_loader_has_no_cancel_row() {
    let theme = UiTheme::dark();
    let loader = BorderedLoader::plain("Loading");
    // 5 rows: the cancellable pair (`Spacer(1)` + hint `Text`) is skipped, everything else stands
    // (`bordered-loader.ts:34-39`).
    assert_eq!(loader.height(), 5);
    let mut terminal = Terminal::new(TestBackend::new(40, 5)).unwrap();
    terminal
        .draw(|f| loader.render(f, Rect::new(0, 0, 40, 5), &theme, 1))
        .unwrap();
    let text = buf_string(&terminal);
    assert!(text.contains("Loading"));
    assert!(!text.contains("cancel"));
}

/// `truncateToVisualLines` owns no wrapping of its own upstream — it is literally
/// `new Text(text, paddingX, 0).render(width)` (`visual-truncate.ts:37-38`), i.e.
/// `wrapTextWithAnsi(text, width - paddingX * 2)` (`text.ts:64`, `:67`). So the rows it returns are
/// WORD-wrapped and measured in terminal COLUMNS.
///
/// cyrup sliced each logical line into fixed `width`-*char* chunks instead: it broke mid-word
/// (`… output tha` / `t certainly …`, visible on every long `!command` output row), counted a CJK
/// ideograph as one column when it occupies two, and could split a ZWJ sequence or detach a
/// combining mark. Because the `... N more lines` count is the row count, it was wrong too.
#[test]
fn truncate_word_wraps_and_measures_in_columns_not_chars() {
    let text = "a very long line of program output that certainly does not fit";
    let r = truncate_to_visual_lines(text, 20, 38);
    assert_eq!(r.skipped, 0);
    // Word boundaries only — no row may start or end mid-word.
    for row in &r.lines {
        assert!(row.len() <= 38, "row overflows: {row:?}");
        assert!(
            !row.starts_with(' ') && !row.ends_with(' '),
            "untrimmed row: {row:?}"
        );
    }
    let rejoined = r.lines.join(" ");
    assert_eq!(rejoined, text, "words were split: {:?}", r.lines);

    // A double-width script is measured in COLUMNS: four ideographs are eight columns, so a width of
    // 4 fits exactly two per row — a `chars()` chunker would put four on a row twice as wide as the
    // pane.
    let cjk = truncate_to_visual_lines("日本語だ", 20, 4);
    assert_eq!(cjk.lines, vec!["日本".to_string(), "語だ".to_string()]);

    // A ZWJ family is one cluster and is never split across rows.
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
    let zwj = truncate_to_visual_lines(&format!("{family}{family}"), 20, 4);
    for row in &zwj.lines {
        assert!(
            !row.starts_with('\u{200d}') && !row.ends_with('\u{200d}'),
            "ZWJ sequence split: {:?}",
            zwj.lines
        );
    }

    // MIRROR: the tail-truncate and the hidden count still work off the WRAPPED row count.
    let many = truncate_to_visual_lines(text, 2, 20);
    assert_eq!(many.lines.len(), 2);
    assert!(many.skipped > 0, "nothing reported hidden: {many:?}");
}

// ====================================================== TUI-018 — the startup header's logo ====
//
// pi's `BuiltInHeader` bodies are built by `withLogo` (`interactive-mode.ts:1015-1020`), which
// prepends a logo line carrying the version and puts the key hints on the line BELOW it — on BOTH
// of its branches. cyrup drew neither the logo line nor an expanded body, and said so in
// `chrome.rs`'s own words.

/// The version string as the logo line spells it, so a bump cannot silently stale the tests.
fn version_label() -> String {
    format!("Cyrup v{}", env!("CARGO_PKG_VERSION"))
}

fn hint_lines(width: u16, expanded: bool) -> Vec<String> {
    let theme = UiTheme::dark();
    let keymap = Keymap::default();
    let editor_keymap = crate::keymap::EditorKeymap::default();
    crate::compact_hint_lines(
        &theme,
        &keymap,
        &editor_keymap,
        width,
        crate::StartupDetails::Shown,
        expanded,
    )
    .iter()
    .map(|l| {
        l.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    })
    .collect()
}

/// **The filed half (a).** The collapsed header OPENS with the wordmark and the crate version, and
/// the hint bar is the row BELOW it — `withLogo` returns `${piWordmark()} ${dim v${version}}\n${hints}`
/// (`interactive-mode.ts:1016`), two logical lines with the hints second.
///
/// The space between the mark and the version is PLAIN: upstream closes the mark's styling before
/// it and opens the dim span after it, on both `withLogo` branches (`:1016`, `:1018`).
///
/// FAILS before the fix: there is no logo entry at all, so the first non-blank row is the hint bar
/// and the app name and version appear nowhere in the UI.
#[test]
fn the_startup_header_opens_with_the_wordmark_and_the_crate_version() {
    let theme = UiTheme::dark();
    let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
    terminal
        .draw(|f| {
            crate::render_compact_hints(
                f,
                Rect::new(0, 0, 80, 10),
                &theme,
                &Keymap::default(),
                &crate::keymap::EditorKeymap::default(),
                crate::StartupDetails::Shown,
                false,
            )
        })
        .unwrap();
    let rows: Vec<String> = buf_string(&terminal).lines().map(str::to_string).collect();
    let first = rows
        .iter()
        .position(|r| !r.trim().is_empty())
        .expect("the block drew nothing");

    // One leading column of `paddingX` (`new BuiltInHeader(…, 1, 0)`, `:1068`).
    assert_eq!(
        rows[first].trim_end(),
        format!(" {}", version_label()),
        "the logo line is the mark, a plain space, then `v{{version}}`"
    );
    assert!(
        rows[first + 1].contains("escape interrupt"),
        "`withLogo` puts the hints on the line BELOW the logo: [{}]",
        rows[first + 1]
    );

    // The dim span covers `v{version}` and NOT the space before it (`:1016`).
    let logo = crate::logo_line(&theme);
    let texts: Vec<&str> = logo.spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(
        texts,
        vec!["Cyrup", " ", concat!("v", env!("CARGO_PKG_VERSION"))],
        "three spans: mark, plain space, dim version"
    );
    assert_eq!(
        logo.spans[2].style,
        theme.dim_style(),
        "`theme.fg(\"dim\", `v${{version}}`)` (`:1016`)"
    );
    assert_ne!(
        logo.spans[1].style,
        theme.dim_style(),
        "the space is OUTSIDE the dim span (`:1016`), so it must not carry dim"
    );
    assert_eq!(
        logo.spans[0].style,
        theme
            .accent_style()
            .add_modifier(ratatui::style::Modifier::BOLD),
        "[CYRUP-DELTA] bold accent stands in for pi's fixed brand RGB (`pi-logo.ts:4-6`, :37-39)"
    );
}

/// **The row arithmetic.** `withLogo` makes the collapsed body FIVE logical lines, framed by a
/// `Spacer(1)` either side (`:1075-1077`), so the unwrapped block is seven rows — not the six that
/// encoded cyrup not drawing the logo part.
///
/// Measured at 100 columns, which is the narrowest round width at which NOTHING wraps: the content
/// width is `100 - paddingX * 2 = 98` and the widest logical line is `onboarding` at 91. At 80 the
/// bar (79) and `onboarding` (91) each take two rows and the honest answer is 9, so 80 would be
/// measuring the wrap rather than the constant — [`COMPACT_HINT_ROWS`] is the UNWRAPPED count, and
/// `c13_narrow_terminal_wraps_the_block_instead_of_clipping_it` covers the wrapped widths.
///
/// FAILS before the fix: both sides are 6.
#[test]
fn the_startup_header_is_seven_rows_unwrapped() {
    let theme = UiTheme::dark();
    assert_eq!(crate::COMPACT_HINT_ROWS, 7);
    assert_eq!(
        crate::compact_hint_height(
            &theme,
            &Keymap::default(),
            &crate::keymap::EditorKeymap::default(),
            100,
            crate::StartupDetails::Shown,
            false,
        ),
        7,
        "Spacer + logo + bar + compactOnboarding + blank + onboarding + Spacer"
    );
}

/// **The filed half (b).** The expanded body carries pi's nineteen `expandedInstructions`
/// (`interactive-mode.ts:1024-1048`) in upstream ORDER, still opening with the logo line, and the
/// collapsed body carries none of them.
///
/// The order is asserted by strictly increasing position rather than by equality, so the test
/// states the one property upstream fixes — the sequence — without pinning the key strings, which
/// resolve through the live keymap and are allowed to differ under a rebind.
///
/// FAILS before the fix: `expanded_hints` does not exist and `expanded` is not a parameter, so the
/// expanded body is the collapsed one and sixteen of the nineteen descriptions are absent.
#[test]
fn ctrl_o_expands_the_startup_header_to_pis_nineteen_hints() {
    let expanded = hint_lines(100, true);
    let joined = expanded.join("\n");

    let first = expanded
        .iter()
        .find(|r| !r.trim().is_empty())
        .expect("the expanded block drew nothing");
    assert_eq!(
        first.trim_end(),
        format!(" {}", version_label()),
        "`withLogo(expandedInstructions())` (`:1066`) keeps the logo line first"
    );

    // `interactive-mode.ts:1026-1047`, in file order.
    const PI_ORDER: [&str; 19] = [
        "to interrupt",
        "to clear",
        "to exit",
        "to exit (empty)",
        "to suspend",
        "to delete to end",
        "to cycle thinking level",
        "to cycle models",
        "to select model",
        "to expand tools",
        "to expand thinking",
        "for external editor",
        "for commands",
        "to run bash",
        "to run bash (no context)",
        "to queue follow-up",
        "to edit all queued messages",
        "to paste files on macOS, images, or text",
        "to attach",
    ];
    let mut cursor = 0usize;
    for desc in PI_ORDER {
        let at = joined
            .get(cursor..)
            .and_then(|rest| rest.find(desc).map(|i| i + cursor))
            .unwrap_or_else(|| {
                panic!("{desc:?} missing or out of pi's order after byte {cursor}:\n{joined}")
            });
        cursor = at + desc.len();
    }

    // The collapsed body is the five-item bar, so none of the expanded-only hints leak into it.
    let collapsed = hint_lines(100, false).join("\n");
    for absent in [
        "to queue follow-up",
        "for external editor",
        "to run bash (no context)",
    ] {
        assert!(
            !collapsed.contains(absent),
            "{absent:?} belongs to `expandedInstructions` only:\n{collapsed}"
        );
    }
}

/// **The asymmetry between the two bodies.** `onboarding()` is in BOTH (`:1065`, `:1066`), but
/// `compactOnboarding()` is in the COLLAPSED one only — its whole job is to advertise the expansion
/// that, in the expanded body, has already happened.
///
/// FAILS before the fix for a naive port that reuses the collapsed tail: it keeps both.
#[test]
fn the_expanded_body_keeps_the_closing_onboarding_line_and_drops_the_compact_onboarding() {
    let expanded = hint_lines(100, true).join("\n");
    assert!(
        expanded.contains(crate::STARTUP_ONBOARDING),
        "`onboarding()` is kept in BOTH bodies (`:1065-1066`):\n{expanded}"
    );
    assert!(
        !expanded.contains("to show full startup help"),
        "`compactOnboarding()` is absent from the expanded body (`:1066`):\n{expanded}"
    );

    // …and the collapsed body is the mirror image: both lines present.
    let collapsed = hint_lines(100, false).join("\n");
    assert!(collapsed.contains("to show full startup help"));
    assert!(collapsed.contains(crate::STARTUP_ONBOARDING));
}

/// **Narrow widths.** pi has NO degradation here: `Text` wraps every logical line at
/// `contentWidth = max(1, width - paddingX * 2)` with no continuation indent and no drop policy
/// (`tui/src/components/text.ts:64-76`), so the header simply GROWS. The version must survive in
/// full rather than being truncated by a fixed-width format.
///
/// FAILS before the fix for a `format!` that pads or truncates to a column budget.
#[test]
fn the_startup_header_wraps_rather_than_truncating_the_version_row() {
    let rows = hint_lines(12, false);
    let joined: String = rows.concat();
    assert!(
        joined.contains(concat!("v", env!("CARGO_PKG_VERSION"))),
        "the version wraps but is never clipped: {rows:?}"
    );
    for row in &rows {
        assert!(
            row.chars().count() <= 12,
            "no row may exceed the terminal width: [{row}]"
        );
    }
}

/// **The drop rank.** The hint bar is rank 0 and is never given up; the logo/version row is rank 1,
/// so it is the LAST text dropped before the bar but still yields to it at a one-row budget.
///
/// FAILS before the fix if the logo row is given rank 0 (two rank-0 groups, and a one-row budget
/// shows the version instead of the bar) or rank 2 (it would be dropped before `compactOnboarding`).
#[test]
fn a_one_row_budget_still_shows_the_hint_bar_not_the_logo() {
    let theme = UiTheme::dark();
    let mut terminal = Terminal::new(TestBackend::new(100, 1)).unwrap();
    terminal
        .draw(|f| {
            crate::render_compact_hints(
                f,
                Rect::new(0, 0, 100, 1),
                &theme,
                &Keymap::default(),
                &crate::keymap::EditorKeymap::default(),
                crate::StartupDetails::Shown,
                false,
            )
        })
        .unwrap();
    let row = buf_string(&terminal);
    assert!(
        row.contains("commands"),
        "one row must BE the hint bar: [{row}]"
    );
    assert!(
        !row.contains(&version_label()),
        "…not the logo, which yields to it: [{row}]"
    );
}

/// **The disjunct the 2026-09-14 attempt dropped, and the reason it was held open.**
/// `getStartupExpansionState()` (`interactive-mode.ts:1418-1420`) is
/// `this.options.verbose || this.toolOutputExpanded`. `toolOutputExpanded` is `false` at
/// construction, so `options.verbose` is the ONLY term ever true at boot: `pi --verbose` opens with
/// the header already expanded. Passing `transcript.tool_expanded()` alone compiles, renders and
/// passes every collapsed-path test while making `--verbose` boot collapsed.
///
/// Asserted with NO key press, through the real `App` and a real frame.
///
/// FAILS before the fix: the expanded hints are absent under `--verbose`.
#[test]
fn verbose_boot_opens_the_startup_header_already_expanded() {
    // Tall enough for the expanded block (logo + 19 hints + blank + onboarding + two Spacers).
    let mut app = App::new(TestBackend::new(100, 40), UiTheme::dark()).unwrap();
    app.set_verbose_startup(true);
    app.draw().unwrap();
    let text = crate::tests::harness::buf_text(&app);
    assert!(
        text.contains("to queue follow-up"),
        "`--verbose` boots expanded per `:1419`, with no key press:\n{text}"
    );

    // The negative control: the same App without the flag is collapsed.
    let mut quiet = App::new(TestBackend::new(100, 40), UiTheme::dark()).unwrap();
    quiet.set_verbose_startup(false);
    quiet.draw().unwrap();
    let quiet_text = crate::tests::harness::buf_text(&quiet);
    assert!(
        !quiet_text.contains("to queue follow-up"),
        "…and a default boot is NOT expanded:\n{quiet_text}"
    );
}

/// **The Verify line's second and third clauses.** `Ctrl+O` shows the expanded hints; a second
/// `Ctrl+O` collapses. Upstream shares ONE flag between the tool output and the header —
/// `setToolsExpanded` calls `activeHeader.setExpanded(expanded)` (`:4561-4577`) — so the header
/// follows the same toggle rather than owning a second one.
///
/// FAILS before the fix, and also fails for a one-way flag that expands but never collapses.
#[test]
fn ctrl_o_toggles_the_startup_header_both_ways() {
    use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = App::new(TestBackend::new(100, 40), UiTheme::dark()).unwrap();
    let ctrl_o = crate::InputEvent::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    let shown = |app: &mut App<TestBackend>| -> bool {
        app.draw().unwrap();
        crate::tests::harness::buf_text(app).contains("to queue follow-up")
    };

    assert!(!shown(&mut app), "a default boot is collapsed");
    app.handle_input(&ctrl_o);
    assert!(shown(&mut app), "the first ctrl+o expands the header");
    app.handle_input(&ctrl_o);
    assert!(
        !shown(&mut app),
        "…and the second collapses it again (`:4561-4577` is a toggle, not a latch)"
    );
}
