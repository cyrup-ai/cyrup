#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
//! Presentation-fidelity guards for the always-on chrome — footer, status band, loaders, hints.
//! C1–C15 of `TUI-FIDELITY.md` §3C, each anchored to a line of pi **v0.84.1** that was read.
//!
//! Upstream paths, all under `pi/packages/`:
//!   - `coding-agent/src/modes/interactive/components/footer.ts` — the three footer lines.
//!   - `coding-agent/src/modes/interactive/components/status-indicator.ts` — the four spinner states.
//!   - `coding-agent/src/modes/interactive/components/bordered-loader.ts` — the inline loader.
//!   - `coding-agent/src/modes/interactive/components/keybinding-hints.ts` — `keyText` / `keyHint`.
//!   - `coding-agent/src/modes/interactive/interactive-mode.ts` — the startup hint block.
//!   - `tui/src/components/{loader,text,spacer}.ts` — the primitives those compose.
//!   - `tui/src/keybindings.ts`, `coding-agent/src/core/keybindings.ts` — the default key sets.
//!
//! This surface is on screen every frame, so the assertions pin *resolved* cell colours and exact
//! byte-for-byte copy rather than comparing one cyrup accessor to another.

use std::sync::Arc;
use std::time::Duration;

use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{AgentSession, AgentSessionEvent, SessionBuilder, SessionConfig};
use cyrup_tui::{
    compact_onboarding, format_tokens, Action, App, BorderedLoader, IndicatorKind, Key, Keymap,
    LoginDialog, SelectAction, SelectKeymap, StatusIndicator, StatusLine, UiTheme,
    COMPACT_HINT_ROWS, STARTUP_ONBOARDING,
};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::Terminal;
use tempfile::TempDir;
use tokio_stream::StreamExt;

// ------------------------------------------------------------------------------ helpers ----

fn app(w: u16, h: u16) -> App<TestBackend> {
    App::new(TestBackend::new(w, h), UiTheme::dark()).unwrap()
}

fn row_text(app: &App<TestBackend>, y: u16) -> String {
    let buf = app.terminal().backend().buffer();
    (0..buf.area.width).filter_map(|x| buf.cell((x, y))).map(|c| c.symbol()).collect()
}

fn buf_text(app: &App<TestBackend>) -> String {
    let h = app.terminal().backend().buffer().area.height;
    (0..h).map(|y| row_text(app, y)).collect::<Vec<_>>().join("\n")
}

/// The first row whose text contains `needle`.
fn find_row(app: &App<TestBackend>, needle: &str) -> u16 {
    let h = app.terminal().backend().buffer().area.height;
    (0..h)
        .find(|&y| row_text(app, y).contains(needle))
        .unwrap_or_else(|| panic!("no row contains {needle:?}:\n{}", buf_text(app)))
}

/// The foreground colour of the cell at `(x, y)`.
fn fg_at(app: &App<TestBackend>, x: u16, y: u16) -> Option<Color> {
    app.terminal().backend().buffer().cell((x, y)).map(|c| c.fg)
}

/// The foreground of the first non-blank cell on row `y`.
fn fg_of_row(app: &App<TestBackend>, y: u16) -> Option<Color> {
    let buf = app.terminal().backend().buffer();
    (0..buf.area.width)
        .filter_map(|x| buf.cell((x, y)))
        .find(|c| c.symbol() != " ")
        .map(|c| c.fg)
}

/// The CELL column of `needle` in `row`. `str::find` returns a BYTE offset, and these rows carry
/// `↑`/`↓`/`·`/`…`, so the two are not the same number — indexing the buffer with the byte offset
/// samples the wrong cell.
fn col_of(row: &str, needle: &str) -> u16 {
    let byte = row.find(needle).unwrap_or_else(|| panic!("{needle:?} not in [{row}]"));
    row.get(..byte).map_or(0, |p| p.chars().count()) as u16
}

/// [`col_of`] for the LAST occurrence.
fn rcol_of(row: &str, needle: &str) -> u16 {
    let byte = row.rfind(needle).unwrap_or_else(|| panic!("{needle:?} not in [{row}]"));
    row.get(..byte).map_or(0, |p| p.chars().count()) as u16
}

const DIM_DARK: Color = Color::Rgb(0x66, 0x66, 0x66);
const MUTED_DARK: Color = Color::Rgb(0x80, 0x80, 0x80);

// =============================================================== C1 — the context segment ====

/// **C1.** `statsParts.push(contextPercentStr)` at `footer.ts:161` sits outside every `if` — the
/// context segment is UNCONDITIONAL. cyrup gated it on `context_percent` *and* `context_window`
/// both being `Some`, and no production code ever set either, so pi's most-watched footer segment
/// (the only one that turns yellow/red) never rendered.
///
/// FAILS before the fix: a fresh footer drew no `%/` at all.
#[test]
fn c1_context_segment_renders_even_when_nothing_ever_set_it() {
    let mut app = app(100, 12);
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(
        text.contains("0.0%/0"),
        "with `getContextUsage()` undefined pi still prints the segment — `contextWindow = \
         contextUsage?.contextWindow ?? state.model?.contextWindow ?? 0` and `contextPercentValue = \
         contextUsage?.percent ?? 0` (footer.ts:109-110):\n{text}"
    );
}

/// **C1.** The `?` branch: `contextPercent === "?" ? \`?/${formatTokens(contextWindow)}…\``
/// (`footer.ts:150-153`), reached when `getContextUsage()` answers `{tokens: null, percent: null}` —
/// i.e. straight after a compaction, before the next LLM response (`agent-session.ts:3186-3188`).
/// cyrup had no such branch: `context_text()` returned `None` and the segment vanished instead.
#[test]
fn c1_unknown_occupancy_renders_the_question_mark_branch() {
    let mut app = app(100, 12);
    app.status_mut().set_context_usage(None, Some(200_000), true);
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("?/200k (auto)"), "post-compaction `?` branch missing:\n{text}");
    assert!(!text.contains("0.0%/200k"), "an unknown count must not read as 0%:\n{text}");
}

/// **C1 — the user action.** Send a message and let the turn finish; the footer must then show the
/// live occupancy. pi gets this for free because `render()` calls `this.session.getContextUsage()`
/// every frame (`footer.ts:108`); cyrup's fold is sync, so the refresh rides the run loop's
/// per-event path — `App::ingest_session_event`, which is exactly what the `events.next()` arm
/// calls.
///
/// FAILS before the fix: `StatusLine::set_context` had no production caller at all (only two tests
/// called it), so no sequence of real session events could ever populate the segment.
#[tokio::test]
async fn c1_a_finished_turn_puts_the_live_context_usage_in_the_footer() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();

    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("hello")],
        cyrup_core::StopReason::Stop,
    )]);
    let provider: Arc<dyn Provider> = faux;
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    let session: Arc<AgentSession> =
        Arc::new(SessionBuilder::new(provider, cfg).build().await.unwrap());

    let mut app = app(100, 12);
    let mut events = session.subscribe();

    // The user action: type a message and press enter.
    let _ = session.prompt("hi").await.unwrap();
    session.wait_for_idle().await;

    // Drive the run loop's own per-event fold over the events the session actually emitted.
    let drained = tokio::time::timeout(Duration::from_secs(5), async {
        let mut saw_end = false;
        while let Some(ev) = events.next().await {
            let end = matches!(ev, AgentSessionEvent::AgentEnd { .. });
            app.ingest_session_event(&ev, &session).await;
            if end {
                saw_end = true;
                break;
            }
        }
        saw_end
    })
    .await
    .expect("the session's event stream should settle");
    assert!(drained, "the turn never reached agent_end");

    app.draw().unwrap();
    let text = buf_text(&app);
    // The faux model declares a 128k window (`faux.rs:61 DEFAULT_CONTEXT_WINDOW`), so the segment
    // must name it — the un-wired footer could only ever have said `0`.
    assert!(text.contains("%/128k"), "live context segment missing after a real turn:\n{text}");
    assert_eq!(
        app.state().status.context_window,
        Some(128_000),
        "the window must come from the live session, not a hardcoded default"
    );
    assert!(
        app.state().status.context_percent.is_some(),
        "a turn with no compaction behind it has a KNOWN occupancy (agent-session.ts:3198-3206)"
    );
}

/// MIRROR: the colour band above the segment still works — `>90` error, `>70` warning
/// (`footer.ts:154-159`). This is the code C1 made reachable; it must not have been broken on the
/// way in.
#[test]
fn c1_mirror_context_colour_bands_still_resolve() {
    let theme = UiTheme::dark();
    for (fraction, want, label) in [
        (0.95_f64, theme.error_style().fg, "above 90% is error"),
        (0.80, theme.warning_style().fg, "above 70% is warning"),
        (0.40, theme.dim_style().fg, "below 70% inherits the outer dim wrapper"),
    ] {
        let mut app = app(100, 12);
        app.status_mut().set_context(fraction, 200_000, true);
        app.draw().unwrap();
        let y = find_row(&app, "%/200k");
        let x = row_text(&app, y).find("%/200k").unwrap() as u16;
        assert_eq!(fg_at(&app, x, y), want, "{label}");
    }
}

/// **C1 — no clamp.** `const percent = (estimate.tokens / contextWindow) * 100`
/// (`agent-session.ts:3211`) and `contextPercentValue.toFixed(1)` (`footer.ts:151`) — nothing caps
/// it at 100. An over-budget context legitimately reads `112.3%`, in `error` red
/// (`footer.ts:154-155`), which is exactly the moment the number matters most.
///
/// FAILS before the fix: `set_context`/`set_context_usage` did `p.clamp(0.0, 1.0)` and
/// `ContextUsage::from_last_assistant` clamped the same figure again upstream of them, so an
/// overflowing context showed a calm `100.0%`.
#[test]
fn c1_context_percent_is_not_clamped_at_one_hundred() {
    let mut over_budget = app(100, 12);
    over_budget.status_mut().set_context_usage(Some(1.123), Some(200_000), false);
    over_budget.draw().unwrap();
    let text = buf_text(&over_budget);
    assert!(text.contains("112.3%/200k"), "an over-budget context reads over 100%:\n{text}");
    assert!(!text.contains("100.0%/200k"), "…not pinned at 100:\n{text}");

    // The same through `set_context`, the other setter, and the colour must be the >90% error band.
    let mut app2 = app(100, 12);
    app2.status_mut().set_context(1.5, 200_000, false);
    app2.draw().unwrap();
    let y = find_row(&app2, "150.0%/200k");
    let x = row_text(&app2, y).find("150.0%").unwrap() as u16;
    assert_eq!(fg_at(&app2, x, y), UiTheme::dark().error_style().fg, "over 90% is `error`");

    // …and the session-side figure it is fed from is unclamped too
    // (`cyrup-session-svc/src/state.rs` `ContextUsage::from_last_assistant`), or the footer would
    // never see a value above 1.0 in production.
    let over = cyrup_session_svc::ContextUsage::from_last_assistant(None, 0);
    assert_eq!(over.fraction, 0.0, "an unknown window is still 0, not NaN");
}

/// **C1 — narrow-width truncation must keep the colour.** `truncateToWidth(statsLeft, width, "...")`
/// (`footer.ts:175`) walks the ANSI-bearing string and carries each escape across the cut in
/// `pendingAnsi` (`tui/src/utils.ts:1119-1122`); the footer's own comment at `:222-224` says it dims
/// the parts *around* the coloured section precisely so those codes survive.
///
/// FAILS before the fix: the overflow path returned ONE span in `dim_style()`, flattening the >90%
/// error red and >70% warning yellow out of the only segment that has a colour.
#[test]
fn c1_narrow_truncation_keeps_the_per_segment_colour() {
    let theme = UiTheme::dark();
    // 24 columns: `↑12k ↓4.1k R88k W2.1k CH82.6% $0.214 95.0%/200k (auto)` is far wider, so the
    // left cluster overflows and the truncation path runs.
    let mut wide_cluster = app(24, 12);
    {
        let status = wide_cluster.status_mut();
        status.add_usage(&cyrup_core::Usage {
            input: 12_300,
            output: 4_100,
            cache_read: 88_000,
            cache_write: 2_100,
            total_tokens: 106_500,
            cost: cyrup_core::Cost { total: 0.214, ..cyrup_core::Cost::default() },
            ..cyrup_core::Usage::default()
        });
        status.set_context(0.95, 200_000, true);
    }
    wide_cluster.draw().unwrap();

    let y = find_row(&wide_cluster, "↑12k");
    let row = row_text(&wide_cluster, y);
    assert!(row.ends_with("..."), "the cluster must actually be truncated: [{row}]");
    // The first segment keeps `dim`, and the ellipsis is `dim` too (it is inside pi's
    // `theme.fg("dim", statsLeft)` wrapper at `footer.ts:225`).
    let up = col_of(&row, "↑");
    assert_eq!(
        fg_at(&wide_cluster, up, y),
        Some(DIM_DARK),
        "an uncoloured segment stays `dim`: [{row}]"
    );
    let ell = rcol_of(&row, "...");
    assert_eq!(fg_at(&wide_cluster, ell, y), Some(DIM_DARK), "the ellipsis is `dim`: [{row}]");

    // THE POINT OF THE TEST: a width whose cut lands INSIDE the context segment, so part of the
    // coloured run is on screen and part is not. The cluster is `↑12k 95.0%/200k` (15 columns); at
    // 12 columns `truncateToWidth` keeps `12 - visibleWidth("...") = 9` of it — `↑12k 95.0` — and
    // the `95.0` it kept is the head of the >90% `error` run.
    let mut cut_inside = app(12, 12);
    {
        let status = cut_inside.status_mut();
        status.add_usage(&cyrup_core::Usage {
            input: 12_300,
            total_tokens: 12_300,
            ..cyrup_core::Usage::default()
        });
        status.set_context(0.95, 200_000, false);
    }
    cut_inside.draw().unwrap();
    let y2 = find_row(&cut_inside, "95.0");
    let row2 = row_text(&cut_inside, y2);
    assert_eq!(row2, "↑12k 95.0...", "the cut must land inside the context segment: [{row2}]");
    let pct = col_of(&row2, "95.0");
    assert_eq!(
        fg_at(&cut_inside, pct, y2),
        theme.error_style().fg,
        "the >90% error red must survive truncation: [{row2}]"
    );
    // …and the segments on the other side of the cut keep their own styles, not one flat colour.
    assert_eq!(fg_at(&cut_inside, col_of(&row2, "↑"), y2), Some(DIM_DARK), "[{row2}]");
    assert_eq!(fg_at(&cut_inside, rcol_of(&row2, "..."), y2), Some(DIM_DARK), "[{row2}]");

    // The >70% warning band survives the same cut.
    let mut warn = app(12, 12);
    {
        let status = warn.status_mut();
        status.add_usage(&cyrup_core::Usage {
            input: 12_300,
            total_tokens: 12_300,
            ..cyrup_core::Usage::default()
        });
        status.set_context(0.80, 200_000, false);
    }
    warn.draw().unwrap();
    let y3 = find_row(&warn, "80.0");
    assert_eq!(
        fg_at(&warn, col_of(&row_text(&warn, y3), "80.0"), y3),
        theme.warning_style().fg,
        "the >70% warning yellow survives too"
    );
}

/// **C1 — the ` (auto)` suffix needs a refresh path of its own.** Upstream calls
/// `footer.setAutoCompactEnabled` from three places and one of them is the `/settings`
/// auto-compaction toggle (`interactive-mode.ts:4417-4419`), not a turn event — so it cannot ride
/// the six-event predicate that gates the context recompute.
///
/// FAILS before the fix: `auto_compact` was written only inside `refresh_context_usage`, itself
/// gated on `context_usage_may_have_moved`.
#[tokio::test]
async fn c1_auto_compact_has_its_own_refresh_independent_of_the_event_predicate() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    let session: Arc<AgentSession> =
        Arc::new(SessionBuilder::new(provider, cfg).build().await.unwrap());

    let mut app = app(100, 12);
    session.set_auto_compaction_enabled(true);
    app.refresh_auto_compact(&session);
    assert!(app.state().status.auto_compact, "the toggle reaches the footer with no event at all");

    // Toggling it back and folding an event that is NOT in the context predicate still updates it.
    session.set_auto_compaction_enabled(false);
    app.ingest_session_event(&AgentSessionEvent::AgentStart, &session).await;
    assert!(
        !app.state().status.auto_compact,
        "`agent_start` is not a context-usage event, but the flag still refreshes"
    );
}

// ============================================================= C2 — footer base colour ======

/// **C2.** `truncateToWidth(theme.fg("dim", pwd), width, theme.fg("dim", "..."))` (`footer.ts:229`)
/// and `theme.fg("dim", statsLeft) + theme.fg("dim", remainder)` (`:225-227`). `dim` is `#666666`
/// (`dark.json:31` `"dim": "dimGray"`), not `muted` `#808080` (`dark.json:30` `"muted": "gray"`).
///
/// FAILS before the fix: every character of both lines was `muted_style()`, one shade too bright.
#[test]
fn c2_footer_lines_one_and_two_carry_the_dim_token() {
    let mut app = app(100, 12);
    app.status_mut().set_cwd("~/src/cyrup");
    app.status_mut().set_model("anthropic/claude-opus-4-8");
    app.draw().unwrap();

    let loc = find_row(&app, "~/src/cyrup");
    assert_eq!(fg_of_row(&app, loc), Some(DIM_DARK), "footer line 1 must be the `dim` token");
    assert_ne!(fg_of_row(&app, loc), Some(MUTED_DARK), "…and specifically not `muted`");

    // Line 2: the right-aligned model name is inside pi's `dimRemainder` (`footer.ts:226-227`).
    let stats = find_row(&app, "claude-opus-4-8");
    let x = row_text(&app, stats).find("claude").unwrap() as u16;
    assert_eq!(fg_at(&app, x, stats), Some(DIM_DARK), "the model cluster must be `dim`");
}

// ====================================================== C3 — the extension-status line ======

/// **C3.** `lines.push(truncateToWidth(statusLine, width, theme.fg("dim", "...")))`
/// (`footer.ts:240`) — the ellipsis carries the colour and the statuses themselves carry none, so an
/// extension's own colours survive. cyrup wrapped the whole row in `dim_style()`.
///
/// FAILS before the fix: the status text rendered `dim` instead of the terminal's body colour.
#[test]
fn c3_extension_status_row_is_unstyled_and_only_its_ellipsis_is_dim() {
    let mut app = app(24, 12);
    app.status_mut().set_extension_status("a", "rust-analyzer is indexing the workspace now");
    app.draw().unwrap();

    let y = find_row(&app, "rust-analyzer");
    let row = row_text(&app, y);
    assert!(row.contains("..."), "a 24-column footer must truncate this status: [{row}]");

    let body_x = row.find("rust").unwrap() as u16;
    assert_eq!(
        fg_at(&app, body_x, y),
        UiTheme::dark().base_style().fg,
        "the status text itself carries no theme call upstream: [{row}]"
    );
    assert_ne!(fg_at(&app, body_x, y), Some(DIM_DARK), "…so it must not be dimmed: [{row}]");

    let ell_x = row.rfind("...").unwrap() as u16;
    assert_eq!(fg_at(&app, ell_x, y), Some(DIM_DARK), "the ellipsis IS `theme.fg(\"dim\", \"...\")`");
}

/// MIRROR: an untruncated status row emits no stray ellipsis.
#[test]
fn c3_mirror_short_status_row_has_no_ellipsis() {
    let mut app = app(100, 12);
    app.status_mut().set_extension_status("a", "ok");
    app.draw().unwrap();
    let row = row_text(&app, find_row(&app, "ok"));
    assert!(!row.contains("..."), "nothing to truncate, so no ellipsis: [{row}]");
}

// ================================================= C4/C5/C12 — the working status band ======

/// **C4.** `Loader extends Text` constructed `super("", 1, 0)` (`loader.ts:35`) — paddingX 1 — and
/// `Text.render` emits `leftMargin + line + rightMargin` (`text.ts:70,76`). The rendered row is
/// ` ⠋ Working... `, inset one column.
///
/// **C5.** `WorkingStatusIndicator` (`status-indicator.ts:29-40`) is constructed with
/// `this.workingMessage ?? this.defaultWorkingMessage` and appends nothing
/// (`interactive-mode.ts:2074-2080`).
///
/// **C12.** `defaultWorkingMessage = "Working..."` — three ASCII dots (`interactive-mode.ts:420`).
///
/// FAILS before the fix on all three: `⠋ Working… (esc to cancel)` at column 0.
#[test]
fn c4_c5_c12_working_band_is_inset_ascii_and_carries_no_cancel_suffix() {
    let theme = UiTheme::dark();
    let mut ind = StatusIndicator::new();
    ind.working();
    let lines = ind.lines_at(Duration::ZERO, &theme, Some("escape"));
    let msg: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();

    assert!(msg.starts_with(' '), "paddingX 1 left inset missing: [{msg}]");
    assert!(msg.ends_with(' '), "paddingX 1 right margin missing: [{msg}]");
    assert!(msg.contains("Working..."), "ASCII `...`, not U+2026: [{msg}]");
    assert!(!msg.contains('…'), "U+2026 is 1 column where pi draws 3: [{msg}]");
    assert!(!msg.contains("to cancel"), "Working takes no cancel suffix: [{msg}]");
}

/// MIRROR (C5): the three states whose upstream constructors DO bake the hint into their copy —
/// retry (`status-indicator.ts:47`), compaction (`:78-82`) and branch summary (`:100`) — keep it.
#[test]
fn c5_mirror_the_other_three_states_still_carry_the_cancel_suffix() {
    let theme = UiTheme::dark();
    for kind in [IndicatorKind::Retry, IndicatorKind::Compaction, IndicatorKind::BranchSummary] {
        let mut ind = StatusIndicator::new();
        ind.set(kind, None);
        let lines = ind.lines_at(Duration::ZERO, &theme, Some("escape"));
        let msg: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(msg.contains("(escape to cancel)"), "{kind:?} must keep its hint: [{msg}]");
    }
}

// ================================================================== C8 — retry countdown ====

/// **C8.** `RetryStatusIndicator` builds a `CountdownTimer(delayMs, …)` whose `onTick` re-`setMessage`s
/// the whole retry line every second (`status-indicator.ts:55-64`, `countdown-timer.ts:18-30`),
/// starting at `Math.ceil(delayMs / 1000)` and decrementing to 0.
///
/// FAILS before the fix: `app.rs` formatted `Retrying (1/3) in 30s…` ONCE, so a 30-second backoff
/// showed a frozen number while only the spinner moved.
#[test]
fn c8_retry_message_counts_down_once_per_second() {
    let theme = UiTheme::dark();
    let mut ind = StatusIndicator::new();
    ind.set_retry(1, 3, 30_000);

    let at = |ind: &StatusIndicator, secs: u64| -> String {
        ind.lines_at(Duration::from_secs(secs), &theme, Some("escape"))[1]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    };
    assert!(at(&ind, 0).contains("Retrying (1/3) in 30s..."), "start = ceil(delayMs/1000)");
    assert!(at(&ind, 1).contains("in 29s..."), "the timer ticks every 1000 ms");
    assert!(at(&ind, 25).contains("in 5s..."), "…all the way down");
    assert!(at(&ind, 30).contains("in 0s..."), "reaches 0, then the timer disposes");
    assert!(at(&ind, 99).contains("in 0s..."), "and never goes negative");
    // The suffix is still pi's, from the live keymap (`status-indicator.ts:47`).
    assert!(at(&ind, 0).contains("(escape to cancel)"));
}

/// **C8 — a SECOND backoff gets a fresh clock.** `StatusIndicator::set` only re-anchors `started`
/// when the *kind* changes, so attempt 2's `AutoRetryStart` — which arrives while the band is
/// already `Retry` — inherited attempt 1's anchor and opened part-way down, or pinned at `0s` once
/// the first delay had elapsed. Upstream cannot have that bug: `showStatusIndicator` disposes the
/// previous `RetryStatusIndicator` (`status-indicator.ts:67-71`) and the replacement builds a brand
/// new `CountdownTimer` (`:55-64`).
///
/// FAILS before the fix: the second message reads `in 18s...` (or lower) instead of `in 20s...`,
/// because the band's `started` anchor was still attempt 1's.
///
/// The assertions read the LIVE clock (`retry_message`, `lines`), not the injected-`elapsed`
/// `lines_at`, because `started` is exactly what the injected form bypasses. The 1.2 s sleep is the
/// smallest that makes the stale anchor visible at 1-second countdown granularity; the passing
/// direction needs only that the assertion runs within 1 s of the second `set_retry`, so CPU
/// contention cannot flip it green.
#[test]
fn c8_a_second_retry_restarts_the_countdown_from_the_top() {
    let theme = UiTheme::dark();
    let mut ind = StatusIndicator::new();

    ind.set_retry(1, 3, 5_000);
    assert_eq!(ind.retry_message().as_deref(), Some("Retrying (1/3) in 5s..."));

    std::thread::sleep(Duration::from_millis(1_200));
    assert_eq!(
        ind.retry_message().as_deref(),
        Some("Retrying (1/3) in 4s..."),
        "sanity: the FIRST backoff is genuinely counting down off `started`"
    );

    // …the attempt fails and attempt 2 is scheduled with its own, longer delay.
    ind.set_retry(2, 3, 20_000);
    assert_eq!(
        ind.retry_message().as_deref(),
        Some("Retrying (2/3) in 20s..."),
        "a fresh `RetryStatusIndicator` starts at `Math.ceil(delayMs / 1000)`, not mid-way"
    );
    let rendered: String = ind.lines(&theme, Some("escape"))[1]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert!(rendered.contains("in 20s..."), "…and the band renders the same number: [{rendered}]");
}

/// MIRROR: a non-retry state has no countdown and keeps the message it was given verbatim.
#[test]
fn c8_mirror_a_plain_message_is_not_rewritten_over_time() {
    let theme = UiTheme::dark();
    let mut ind = StatusIndicator::new();
    ind.set(IndicatorKind::Compaction, Some("Auto-compacting...".to_string()));
    // Span 0 is the spinner (which DOES advance); span 1 is the message.
    let msg = |secs: u64| -> String {
        ind.lines_at(Duration::from_secs(secs), &theme, None)[1].spans[1].content.to_string()
    };
    assert_eq!(msg(0), msg(42), "only a retry backoff re-derives its message");
    assert_eq!(msg(0), "Auto-compacting...");
    // …and entering a new state drops any countdown that was running.
    let mut ind2 = StatusIndicator::new();
    ind2.set_retry(1, 3, 30_000);
    ind2.set(IndicatorKind::Working, None);
    assert!(ind2.retry_message().is_none(), "`set` disposes the timer (status-indicator.ts:67-71)");
}

// ============================================================ C6/C7/C11 — BorderedLoader ====

/// **C6.** `bordered-loader.ts:20-21` and `:28-29` — the spinner is `theme.fg("accent", s)` but the
/// message is `theme.fg("muted", s)`, in BOTH the cancellable and the plain branch.
///
/// **C7.** The children are `DynamicBorder` / `Loader` (**2** rows, `loader.ts:44`) / `Spacer(1)` /
/// `Text(keyHint, 1, 0)` / `Spacer(1)` / `DynamicBorder` = 7 rows (`bordered-loader.ts:16-39`).
///
/// FAILS before the fix: 4 rows, message painted accent.
#[test]
fn c6_c7_bordered_loader_row_count_and_message_colour() {
    let theme = UiTheme::dark();
    let loader = BorderedLoader::cancellable("Creating gist...", "escape/ctrl+c");
    assert_eq!(loader.height(), 7, "1 + 2 + 1 + 1 + 1 + 1 (bordered-loader.ts:16-39)");

    let mut terminal = Terminal::new(TestBackend::new(40, 7)).unwrap();
    terminal.draw(|f| loader.render(f, Rect::new(0, 0, 40, 7), &theme, 0)).unwrap();
    let buf = terminal.backend().buffer().clone();
    let row = |y: u16| -> String {
        (0..buf.area.width).filter_map(|x| buf.cell((x, y))).map(|c| c.symbol()).collect()
    };

    assert!(row(0).contains('─'), "row 0 is the top DynamicBorder: [{}]", row(0));
    assert!(
        row(1).trim().is_empty(),
        "row 1 is the Loader's own leading blank — `[\"\", ...super.render(width)]` \
         (loader.ts:44): [{}]",
        row(1)
    );
    assert!(row(2).contains("Creating gist..."), "row 2 is the spinner line: [{}]", row(2));
    assert!(row(3).trim().is_empty(), "row 3 is Spacer(1) (bordered-loader.ts:35): [{}]", row(3));
    assert!(row(4).contains("cancel"), "row 4 is the key hint: [{}]", row(4));
    assert!(row(5).trim().is_empty(), "row 5 is Spacer(1) (bordered-loader.ts:38): [{}]", row(5));
    assert!(row(6).contains('─'), "row 6 is the closing DynamicBorder: [{}]", row(6));

    let msg_x = row(2).find("Creating").unwrap() as u16;
    assert_eq!(
        buf.cell((msg_x, 2)).map(|c| c.fg),
        theme.muted_style().fg,
        "`messageColorFn = (s) => theme.fg(\"muted\", s)` (bordered-loader.ts:21)"
    );
    let spin_x = row(2).find('⠋').unwrap() as u16;
    assert_eq!(
        buf.cell((spin_x, 2)).map(|c| c.fg),
        theme.accent_style().fg,
        "`spinnerColorFn = (s) => theme.fg(\"accent\", s)` (bordered-loader.ts:20)"
    );
}

/// MIRROR (C7): the plain branch skips only the `Spacer(1)` + hint pair — 5 rows
/// (`bordered-loader.ts:34-39`).
#[test]
fn c7_mirror_plain_loader_is_five_rows_with_no_hint() {
    let theme = UiTheme::dark();
    let loader = BorderedLoader::plain("Loading...");
    assert_eq!(loader.height(), 5);
    let mut terminal = Terminal::new(TestBackend::new(40, 5)).unwrap();
    terminal.draw(|f| loader.render(f, Rect::new(0, 0, 40, 5), &theme, 0)).unwrap();
    let buf = terminal.backend().buffer().clone();
    let text: String = (0..5)
        .map(|y| -> String {
            (0..buf.area.width).filter_map(|x| buf.cell((x, y))).map(|c| c.symbol()).collect()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Loading..."));
    assert!(!text.contains("cancel"), "no hint row when not cancellable:\n{text}");
}

/// **C11.** `keyHint("tui.select.cancel", "cancel")` (`bordered-loader.ts:36`) resolves through
/// `keyText` → `formatKeys(getKeys(kb))` → `formatKeyText(keys.join("/"))`
/// (`keybinding-hints.ts:29-36`): **all** bound keys, joined with `/`. The stock set is
/// `["escape", "ctrl+c"]` (`tui/src/keybindings.ts:149-152`).
///
/// FAILS before the fix: `Keymap::key_label(Action::Interrupt)` — a different action, and only its
/// first key, so the hint read `esc cancel` and silently omitted the second cancel key.
#[test]
fn c11_select_cancel_hint_joins_every_bound_key() {
    let km = SelectKeymap::default();
    assert_eq!(
        km.keys_label(SelectAction::Cancel).as_deref(),
        Some("escape/ctrl+c"),
        "both stock cancel keys, joined by `/`"
    );
    // A rebind flows through, which is the whole point of resolving it from the live keymap.
    let mut rebound = SelectKeymap::default();
    rebound.set_action(SelectAction::Cancel, vec![Key::parse("ctrl+q").unwrap()]);
    assert_eq!(rebound.keys_label(SelectAction::Cancel).as_deref(), Some("ctrl+q"));
    // An unbound action is upstream's `keys.length === 0` → no hint.
    let mut unbound = SelectKeymap::default();
    unbound.set_action(SelectAction::Cancel, Vec::new());
    assert_eq!(unbound.keys_label(SelectAction::Cancel), None);
}

/// MIRROR (C11): `key_label` — the first-key form — is untouched, so callers that genuinely want one
/// key still get one.
#[test]
fn c11_mirror_first_key_label_still_returns_a_single_key() {
    assert_eq!(
        SelectKeymap::default().key_label(SelectAction::Cancel).as_deref(),
        Some("escape"),
        "the single-key accessor is unchanged apart from C10's spelling"
    );
}

// ================================================================= C9 — token rounding ======

/// **C9.** `if (count < 1000000) return \`${Math.round(count / 1000)}k\`` and
/// `return \`${Math.round(count / 1000000)}M\`` (`footer.ts:27`, `:29`) — **rounding**. cyrup used
/// `n / 1_000`, which truncates.
///
/// FAILS before the fix: 45 600 → `45k` (pi: `46k`), 10 900 → `10k` (pi: `11k`).
#[test]
fn c9_token_counts_round_rather_than_truncate() {
    assert_eq!(format_tokens(45_600), "46k", "Math.round(45.6) === 46");
    assert_eq!(format_tokens(10_900), "11k", "Math.round(10.9) === 11");
    assert_eq!(format_tokens(10_500), "11k", "Math.round(10.5) === 11 — half rounds up");
    assert_eq!(format_tokens(999_600), "1000k", "the `<1000000` branch still owns this");
    assert_eq!(format_tokens(10_500_000), "11M", "Math.round(10.5) === 11 in the M branch too");
}

/// MIRROR (C9): the four thresholds and the two `toFixed(1)` branches are unchanged
/// (`footer.ts:25-28`).
#[test]
fn c9_mirror_thresholds_and_one_decimal_branches_are_unchanged() {
    assert_eq!(format_tokens(0), "0");
    assert_eq!(format_tokens(999), "999", "`count < 1000` prints the raw number");
    assert_eq!(format_tokens(1_000), "1.0k", "`count < 10000` → toFixed(1)");
    assert_eq!(format_tokens(4_100), "4.1k");
    assert_eq!(format_tokens(45_400), "45k", "rounding DOWN is still rounding");
    assert_eq!(format_tokens(1_000_000), "1.0M", "`count < 10000000` → toFixed(1)");
}

/// **C9 — the rounding fix must not introduce a panic.** The `M` branch was written
/// `(n + 500_000) / 1_000_000`, which overflows for any `n > u64::MAX - 500_000`: `attempt to add
/// with overflow` in debug, a silently wrapped and absurd `M` count in release. `format_tokens` is
/// `pub` (re-exported at `lib.rs`) and is fed from `StatusLine::usage`, an accumulator, so the
/// argument is not bounded by anything the footer controls. The remainder form
/// `(n / d) + (n % d >= d/2)` is total for every `u64`.
///
/// `u64::MAX / 1_000_000 = 18_446_744_073_709` remainder `551_615`, which is `>= 500_000`, so
/// `Math.round` rounds up to `18_446_744_073_710`.
#[test]
fn c9_the_rounding_fix_is_total_at_the_top_of_the_range() {
    assert_eq!(format_tokens(u64::MAX), "18446744073710M");
    assert_eq!(format_tokens(u64::MAX - 500_000), "18446744073709M", "just below the round-up");
    assert_eq!(format_tokens(u64::MAX / 2), "9223372036855M");
    // The `k` branch is bounded by its own `< 1_000_000` guard, but assert the boundary anyway.
    assert_eq!(format_tokens(999_999), "1000k");
}

// ==================================================== C10/C13 — escape label + hint bar =====

/// **C10.** `"app.interrupt": { defaultKeys: "escape" }` (`core/keybindings.ts:66`) and
/// `formatKeyText` (`keybinding-hints.ts:17-27`) never abbreviates — it only splits on `/` and `+`
/// and rewrites `alt`→`option` on darwin.
///
/// **C13.** The startup block is the collapsed body of an `ExpandableText(…, 1, 0)` — paddingX 1,
/// `interactive-mode.ts:951-957` — and that body is FIVE parts:
/// `${logo}\n${compactInstructions}\n${compactOnboarding}\n\n${onboarding}` (`:952`). It is framed
/// by a `Spacer(1)` above and below (`:960-962`). cyrup drew one un-inset row; a first pass then
/// added `compactInstructions` + `compactOnboarding` but stopped there, dropping the `\n` and
/// `onboarding` (`:947-950`) out of the middle of the same template literal.
///
/// FAILS before the fix: `esc interrupt`, no `Press ctrl+o …` line, no `Cyrup can explain …` line,
/// no inset, no framing blanks.
#[test]
fn c10_c13_startup_hint_block_is_framed_inset_and_names_the_expand_key() {
    // 100 columns: the widest row is `onboarding` at 91 and the content width is 98, so no row
    // wraps and the block's six logical rows are six screen rows. (The wrapping behaviour has its
    // own test below.)
    let mut app = app(100, 20);
    app.draw().unwrap();

    let hints = find_row(&app, "interrupt");
    let row = row_text(&app, hints);
    assert!(row.starts_with(' '), "paddingX 1 left inset missing: [{row}]");
    assert!(row.contains("escape interrupt"), "C10 — the key spells out as `escape`: [{row}]");
    assert!(!row.contains("esc interrupt"), "…never abbreviated: [{row}]");

    // `compactOnboarding` sits on the line directly below (`interactive-mode.ts:943-946`, `:952`).
    let onboarding = row_text(&app, hints + 1);
    assert!(
        onboarding.contains("Press ctrl+o to show full startup help and loaded resources."),
        "C13 — the only line telling a new user the expanded help exists: [{onboarding}]"
    );
    assert!(onboarding.starts_with(' '), "the onboarding line is inset too: [{onboarding}]");

    // …then the body's own `\n\n` blank, then `onboarding` (`:947-950`), rebranded pi→cyrup.
    assert!(row_text(&app, hints + 2).trim().is_empty(), "the body's `\\n\\n` blank is missing");
    let closing = row_text(&app, hints + 3);
    assert!(
        closing.contains(
            "Cyrup can explain its own features and look up its docs. Ask it how to use or extend \
             Cyrup."
        ),
        "C13 — `onboarding` is part of the SAME template literal, not an optional extra: [{closing}]"
    );
    assert_eq!(
        closing.trim_end(),
        format!(" {STARTUP_ONBOARDING}"),
        "…inset by paddingX 1 like every row"
    );

    // `Spacer(1)` on each side of the block (`:960-962`).
    assert!(row_text(&app, hints - 1).trim().is_empty(), "no framing blank above the block");
    assert!(row_text(&app, hints + 4).trim().is_empty(), "no framing blank below the block");
    assert_eq!(
        COMPACT_HINT_ROWS, 6,
        "Spacer + compactInstructions + compactOnboarding + blank + onboarding + Spacer"
    );
}

/// **C13 — the short-terminal regression.** The block degrades from its EDGES INWARD so the hint bar
/// is the last row standing. A fixed-height, top-aligned `Paragraph` that put the framing blank
/// FIRST drew the blank and dropped the bar the moment the budget hit one row — strictly worse than
/// the single un-framed bar that shipped before the block existed.
///
/// FAILS before the fix at heights 1 and 2: the hint bar was absent entirely.
#[test]
fn c13_short_terminal_gives_up_the_edges_and_keeps_the_hint_bar() {
    let theme = UiTheme::dark();
    let keymap = Keymap::default();
    // 100 columns so each logical row is exactly one screen row and the budget is unambiguous.
    let render = |rows: u16| -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(100, rows)).unwrap();
        terminal
            .draw(|f| {
                cyrup_tui::render_compact_hints(f, Rect::new(0, 0, 100, rows), &theme, &keymap)
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..rows)
            .map(|y| -> String {
                (0..buf.area.width).filter_map(|x| buf.cell((x, y))).map(|c| c.symbol()).collect()
            })
            .collect()
    };

    let one = render(1);
    assert!(one[0].contains("escape interrupt"), "one row must BE the hint bar: {one:?}");

    let two = render(2);
    assert!(two[0].contains("escape interrupt"), "the bar comes first at two rows: {two:?}");
    assert!(two[1].contains("Press ctrl+o"), "…then compactOnboarding: {two:?}");

    let three = render(3);
    assert!(three[0].contains("escape interrupt"), "three rows: {three:?}");
    assert!(three[1].contains("Press ctrl+o"), "three rows: {three:?}");
    assert!(three[2].trim().is_empty(), "the body's inner blank outlives `onboarding`: {three:?}");

    // Five rows gives up only the TRAILING blank; the leading one is still there.
    let five = render(5);
    assert!(five[0].trim().is_empty(), "leading Spacer survives at five rows: {five:?}");
    assert!(five[4].contains("Cyrup can explain"), "…and the block ends on `onboarding`: {five:?}");

    // Four gives up the leading blank too, so the bar is on row 0 and the block still ends on
    // `onboarding` — the two outermost rows go before any text does.
    let four = render(4);
    assert!(four[0].contains("escape interrupt"), "four rows: {four:?}");
    assert!(four[3].contains("Cyrup can explain"), "four rows: {four:?}");
}

/// **C13 — wrapping.** `Text.render` wraps at `contentWidth = Math.max(1, width - paddingX * 2)`
/// (`tui/src/components/text.ts:64`, `wrapTextWithAnsi` at `:67`), so a narrow terminal makes the
/// block TALLER; it never truncates. cyrup drew a fixed-height `Paragraph` with no `.wrap()`, which
/// silently threw the overflow away.
///
/// FAILS before the fix: the bar ended at `ctrl+o` with `more` lost, and the row budget never grew.
#[test]
fn c13_narrow_terminal_wraps_the_block_instead_of_clipping_it() {
    let theme = UiTheme::dark();
    let keymap = Keymap::default();
    // `compactInstructions` is 79 columns, `compactOnboarding` 60, `onboarding` 91.
    assert_eq!(
        cyrup_tui::compact_hint_height(&theme, &keymap, 100),
        6,
        "content 98 ≥ 91: nothing wraps"
    );
    assert_eq!(
        cyrup_tui::compact_hint_height(&theme, &keymap, 80),
        8,
        "content 78: the 79-column bar and the 91-column onboarding each take 2 rows"
    );
    assert_eq!(
        cyrup_tui::compact_hint_height(&theme, &keymap, 60),
        9,
        "content 58: all three text rows take 2"
    );

    let mut app = app(80, 24);
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("ctrl+o"), "the tail of the bar must survive:\n{text}");
    assert!(text.contains("more"), "…including the word past the wrap point:\n{text}");
    assert!(
        text.contains("extend Cyrup."),
        "and the tail of the 91-column onboarding line:\n{text}"
    );
}

/// **C11 — the app-tier twin.** `hint(kb, desc)` and the status band's
/// `(${keyText("app.interrupt")} to cancel)` (`status-indicator.ts:47`) resolve through the same
/// `keyText` as the select tier, so `Keymap` needs the same all-keys accessor.
///
/// FAILS before the fix: `Keymap::keys_label` did not exist and three further call sites
/// (`compact_hints`, `compact_onboarding`, the status band) used the first-key `key_label`.
#[test]
fn c11_app_keymap_keys_label_joins_every_bound_key() {
    let mut km = Keymap::default();
    assert_eq!(km.keys_label(Action::Interrupt).as_deref(), Some("escape"), "one stock key");
    km.set_action(Action::Interrupt, vec![Key::parse("escape").unwrap(), Key::ctrl('q')]);
    assert_eq!(
        km.keys_label(Action::Interrupt).as_deref(),
        Some("escape/ctrl+q"),
        "`formatKeyText(keys.join(\"/\"))` (keybinding-hints.ts:31)"
    );
    assert_eq!(km.key_label(Action::Interrupt).as_deref(), Some("escape"), "first-key form intact");

    // …and it reaches the hint bar and the onboarding sentence, which are `hint(…)`/`keyText(…)`.
    let mut expand = Keymap::default();
    expand.set_action(Action::ToolsExpand, vec![Key::ctrl('o'), Key::ctrl('e')]);
    assert_eq!(cyrup_tui::compact_hints(&expand)[4].0, "ctrl+o/ctrl+e", "the `more` hint");
    assert_eq!(
        compact_onboarding(&expand),
        "Press ctrl+o/ctrl+e to show full startup help and loaded resources."
    );

    // The band's cancel hint is `keyText("app.interrupt")` too (`status-indicator.ts:47,78,100`).
    let mut app = app(100, 16);
    app.state_mut().keymap = km;
    app.state_mut().indicator.set(IndicatorKind::Compaction, None);
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(
        text.contains("(escape/ctrl+q to cancel)"),
        "the band must name BOTH interrupt keys:\n{text}"
    );

    // …and the login dialog, whose hints are `keyHint("tui.select.cancel"/"tui.select.confirm", …)`
    // (`login-dialog.ts:141`, `:163`, `:199`, `:210`) — the fourth site, on the SELECT tier.
    let mut dialog = LoginDialog::new("Login to anthropic", &SelectKeymap::default());
    dialog.show_waiting("Waiting for the browser…");
    let rendered =
        dialog.lines().iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join("\n");
    assert!(
        rendered.contains("(escape/ctrl+c to cancel)"),
        "the stock cancel set is `[\"escape\", \"ctrl+c\"]` (tui/src/keybindings.ts:149-152):\n\
         {rendered}"
    );
}

/// MIRROR (C13): the onboarding copy is built from the LIVE keymap, so a rebind of
/// `app.tools.expand` renames the key in both the bar and the sentence.
#[test]
fn c13_mirror_onboarding_copy_follows_a_rebind() {
    let mut km = Keymap::default();
    km.set_action(Action::ToolsExpand, vec![Key::ctrl('t')]);
    assert_eq!(
        compact_onboarding(&km),
        "Press ctrl+t to show full startup help and loaded resources.",
        "the sentence is not frozen at compile time"
    );
    assert_eq!(
        compact_onboarding(&Keymap::default()),
        "Press ctrl+o to show full startup help and loaded resources."
    );
}

// ================================================================ C14 — no queue segment ====

/// **C14.** `footer.ts:129-164` builds exactly `↑ ↓ R W CH% $cost`, the context segment and `xp`.
/// There is no queue segment upstream under any name — `git grep -n "queued" v0.84.1 --
/// packages/coding-agent/src/modes/interactive/components/footer.ts` matches nothing. The extra
/// segment pushed the right-aligned model name over, or truncated it at narrow widths.
///
/// FAILS before the fix: `3 queued` appeared between the cost and the model.
#[test]
fn c14_footer_has_no_queued_segment() {
    let mut status = StatusLine::new("anthropic/claude-opus-4-8");
    status.set_queued(3);
    assert_eq!(status.queued, 3, "the depth is still TRACKED — only the footer segment is gone");
    assert!(
        !status.usage_cluster().contains("queued"),
        "left cluster: [{}]",
        status.usage_cluster()
    );
}

/// MIRROR (C14): every segment pi DOES build is still there, in pi's order
/// (`footer.ts:130-161`).
#[test]
fn c14_mirror_the_segments_pi_does_build_are_all_present_and_ordered() {
    use cyrup_core::{Cost, Usage};
    let mut status = StatusLine::new("anthropic/claude-opus-4-8");
    status.add_usage(&Usage {
        input: 12_300,
        output: 4_100,
        cache_read: 88_000,
        cache_write: 2_100,
        total_tokens: 106_500,
        cost: Cost { total: 0.214, ..Cost::default() },
        ..Usage::default()
    });
    status.set_context(0.412, 200_000, true);
    status.set_experimental(true);
    let cluster = status.usage_cluster();
    let want = ["↑12k", "↓4.1k", "R88k", "W2.1k", "CH", "$0.214", "41.2%/200k (auto)", "• xp"];
    let mut at = 0usize;
    for seg in want {
        let found = cluster[at..]
            .find(seg)
            .unwrap_or_else(|| panic!("segment {seg:?} missing or out of order in [{cluster}]"));
        at += found + seg.len();
    }
}
