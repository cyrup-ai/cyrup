//! SUBA-210 / SUBA-211 — the async-jobs card as a LIVE widget, driven through the production host.
//!
//! Every event below goes through [`cyrup_ext::ExtensionHost::widget_event`] /
//! [`cyrup_ext::ExtensionHost::terminal_resized`], so it is routed by the owner the host recorded
//! when the extension published the card through its caller-scoped services — the same route the
//! TUI's click and resize take (`cyrup-tui/src/app/pointer.rs`, `app/input.rs`). Nothing here calls
//! the extension's handler directly.
//!
//! Upstream (`pi-subagents` @ad11b7ab): the card is `buildWidgetComponent` (`src/tui/render.ts:2911`)
//! whose `handleMouse` (`:2938-2944`) folds on a left click at `y === 0` with no shift, alt or ctrl
//! and calls `invalidate()`, which resets the layout session (`:2920-2924`); its render reads
//! `process.stdout.rows`/`columns` (`:2536-2547`) and relocks when either moves
//! (`widgetSessionMatches`, `:2549-2553`); and a running job with no step detail counts its
//! launch-time `agents` (`runningLeafAgentCount`, `:2642-2644`; the fleet, `fleet-status.ts:467-473`).

use super::*;

use crate::background::async_status_snapshot::ASYNC_STATUS_SNAPSHOT_WIDGET_KEY;
use crate::tui::fleet_status::FLEET_STATUS_WIDGET_KEY;
use crate::tui::render::{WidgetLayoutSession, WidgetRenderTier};
use cyrup_ext::{
    PointerModifiers, TerminalSize, WidgetEvent, WidgetEventOutcome, WidgetMouse, WidgetPointer,
};

fn size(columns: u16, rows: u16) -> TerminalSize {
    TerminalSize { columns, rows }
}

fn click(row: u16, modifiers: PointerModifiers) -> WidgetEvent {
    WidgetEvent::Mouse(WidgetMouse {
        pointer: WidgetPointer::Click {
            column: 4,
            row,
            count: 1,
        },
        modifiers,
    })
}

fn plain_click_on_header() -> WidgetEvent {
    click(0, PointerModifiers::default())
}

impl Harness {
    /// The terminal the host reports, as the TUI's mirror would.
    fn set_terminal(&self, terminal: Option<TerminalSize>) {
        *self.services.terminal.lock().unwrap() = terminal;
    }

    /// The card's last published lines.
    fn card(&self) -> Vec<String> {
        self.last_widget(ASYNC_STATUS_SNAPSHOT_WIDGET_KEY)
            .expect("the async slot was published")
            .expect("as content")
    }

    /// How many `set_widget` calls the extension has made, so a test can see that an event it
    /// left alone republished nothing.
    fn publishes(&self) -> usize {
        self.widgets().len()
    }

    /// A stand-in for the session a previous render took: a ONE-LINE lock taken at `at`. A matching
    /// one-line session stays one line however tall the terminal is (`render.ts:2775-2777`), so the
    /// card stays one line until something relocks it — which is what makes a relock observable.
    fn lock_one_line_at(&self, at: TerminalSize) {
        self.extension
            .replace_async_widget_session(Some(WidgetLayoutSession {
                expanded: false,
                rows: usize::from(at.rows),
                columns: usize::from(at.columns),
                tier: WidgetRenderTier::SingleLine,
                locked_rows: None,
                root_job_count: None,
                visible_job_keys: Vec::new(),
            }));
    }
}

fn is_one_line_card(card: &[String]) -> bool {
    card.len() == 1 && card[0].contains(" subagents (")
}

/// Verify 1 — a plain left click on row 0 folds the card to `subagents (…)`, a second unfolds it,
/// and a modified click or a click on another row does nothing (no republish, `Unhandled`, so the
/// click stays the text selection's).
///
/// RED at HEAD: `ExtensionHost::widget_event` and `NativeExtension::on_widget_event` do not exist
/// (the file does not compile). Named mutations, each RED: `on_widget_event` answering
/// `Unhandled` for every key (the card never folds); `CallerScopedServices::set_widget` not
/// recording the owner (the host finds no owner, `Unhandled`); dropping the modifier test from
/// `WidgetMouse::is_plain_click_on_row` (the shift-click folds).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_header_click_folds_and_unfolds_the_card_and_nothing_else_does() {
    let harness = Harness::full().await;
    harness.set_terminal(Some(size(120, 40)));
    harness.track_a_live_run("suba210fold").await;
    repaint(&harness, ExtMode::Tui).await;
    let unfolded = harness.card();
    assert!(
        !is_one_line_card(&unfolded) && unfolded.join("\n").contains("pb8worker"),
        "starts as the full block naming the run: {unfolded:?}"
    );
    assert_eq!(
        harness.host.widget_owner(ASYNC_STATUS_SNAPSHOT_WIDGET_KEY),
        Some(ExtensionId::from(crate::extension::EXTENSION_ID)),
        "the host recorded who published the card"
    );

    let before = harness.publishes();
    for (name, modifiers) in [
        (
            "shift",
            PointerModifiers {
                shift: true,
                ..PointerModifiers::default()
            },
        ),
        (
            "alt",
            PointerModifiers {
                alt: true,
                ..PointerModifiers::default()
            },
        ),
        (
            "ctrl",
            PointerModifiers {
                ctrl: true,
                ..PointerModifiers::default()
            },
        ),
    ] {
        assert_eq!(
            harness
                .host
                .widget_event(ASYNC_STATUS_SNAPSHOT_WIDGET_KEY, click(0, modifiers))
                .await,
            WidgetEventOutcome::Unhandled,
            "a {name}-click on the header is not the card's"
        );
    }
    assert_eq!(
        harness
            .host
            .widget_event(
                ASYNC_STATUS_SNAPSHOT_WIDGET_KEY,
                click(1, PointerModifiers::default())
            )
            .await,
        WidgetEventOutcome::Unhandled,
        "a click on another row is not the card's"
    );
    assert_eq!(
        harness.publishes(),
        before,
        "nothing it left alone republished the card"
    );
    assert_eq!(harness.card(), unfolded, "and the card is unchanged");

    assert_eq!(
        harness
            .host
            .widget_event(ASYNC_STATUS_SNAPSHOT_WIDGET_KEY, plain_click_on_header())
            .await,
        WidgetEventOutcome::Handled
    );
    let folded = harness.card();
    assert!(
        is_one_line_card(&folded) && folded[0].ends_with(" subagents (1/1 running)"),
        "the click folded it to the one-line card: {folded:?}"
    );

    assert_eq!(
        harness
            .host
            .widget_event(ASYNC_STATUS_SNAPSHOT_WIDGET_KEY, plain_click_on_header())
            .await,
        WidgetEventOutcome::Handled
    );
    let again = harness.card();
    assert!(
        !is_one_line_card(&again) && again.join("\n").contains("pb8worker"),
        "a second click unfolded it: {again:?}"
    );

    // The fold lasts across refresh edges, as pi's lasts until the widget unmounts.
    harness
        .host
        .widget_event(ASYNC_STATUS_SNAPSHOT_WIDGET_KEY, plain_click_on_header())
        .await;
    repaint(&harness, ExtMode::Tui).await;
    assert!(
        is_one_line_card(&harness.card()),
        "a refresh edge keeps the folded state: {:?}",
        harness.card()
    );

    // The fleet-status widget answers no pointer upstream.
    assert_eq!(
        harness
            .host
            .widget_event(FLEET_STATUS_WIDGET_KEY, plain_click_on_header())
            .await,
        WidgetEventOutcome::Unhandled
    );
}

/// Verify 1, last clause — each toggle RELOCKS the card: pi's `invalidate()` resets the layout
/// session (`render.ts:2920-2924`). The card is held one line by a matching one-line session (a
/// lock taken earlier, which pi keeps until it no longer matches); folding and unfolding drops
/// it, so the unfolded card is laid out afresh — the progressive card, under `rows`.
///
/// RED at HEAD: does not compile (the event route is new). Named mutation, RED: removing
/// `reset_async_widget_session()` from the fold arm of `handle_async_widget_event` (the stale
/// one-line lock survives the toggle and the unfolded card is still one line).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_toggle_relocks_the_progressive_card() {
    let harness = Harness::start_with(RegistrationMode::Full, |config| {
        config.async_widget_layout = Some(serde_json::Value::String("rows".to_string()));
    })
    .await;
    harness.set_terminal(Some(size(120, 40)));
    harness.track_a_live_run("suba210relock").await;
    harness.lock_one_line_at(size(120, 40));
    repaint(&harness, ExtMode::Tui).await;
    assert!(
        is_one_line_card(&harness.card()),
        "the stale one-line lock holds while it matches: {:?}",
        harness.card()
    );

    harness
        .host
        .widget_event(ASYNC_STATUS_SNAPSHOT_WIDGET_KEY, plain_click_on_header())
        .await;
    harness
        .host
        .widget_event(ASYNC_STATUS_SNAPSHOT_WIDGET_KEY, plain_click_on_header())
        .await;
    let card = harness.card();
    assert_eq!(card.len(), 2, "header + one job line: {card:?}");
    assert!(
        card[0].ends_with(" Async agents · 1 agent running"),
        "relocked to the progressive card: {card:?}"
    );
    assert!(card[1].contains("pb8worker"), "{card:?}");
}

/// Track `count` live runs, each its own single-step run.
async fn track_runs(harness: &Harness, prefix: &str, count: usize) {
    for index in 0..count {
        harness.track_a_live_run(&format!("{prefix}{index}")).await;
    }
}

/// Verify 3 — the default `adaptive` layout now chooses its tier from the REAL terminal, so the two
/// tiers the 30-row fallback made unreachable are reached.
///
/// The brief's literal "on a 20-row terminal the default layout publishes the progressive card"
/// is not upstream's rule and is not what this asserts: pi's `estimateAvailableWidgetRows` is
/// `max(1, rows - 19)` (`render.ts:2536-2539`), so 20 rows leave ONE row and
/// `fitAdaptiveWidgetLines` takes the one-line tier (`availableRows <= 2`, `:2806-2809`). The
/// progressive card needs at least three available rows (22+) and a full block that does not fit
/// them; the full block folds to at most five lines, so that is a 22- to 23-row terminal. Both
/// are asserted, each against the 30-row fallback's answer (the full block).
///
/// RED at HEAD: the default layout never leaves the full tier (both assertions fail; the file
/// also does not compile there, `terminal_size` being new). Named mutation, RED: the publisher
/// passing `ASYNC_WIDGET_FALLBACK_ROWS` instead of `services.terminal_size()` (the old
/// `slash.rs` line) — both cards are the full block again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_default_layout_follows_the_real_terminal_height() {
    let harness = Harness::full().await;
    track_runs(&harness, "suba211tier", 6).await;

    harness.set_terminal(Some(size(120, 20)));
    repaint(&harness, ExtMode::Tui).await;
    let short = harness.card();
    assert!(
        is_one_line_card(&short) && short[0].ends_with(" subagents (6/6 running)"),
        "a 20-row terminal leaves one widget row: the one-line card: {short:?}"
    );

    harness.extension.replace_async_widget_session(None);
    harness.set_terminal(Some(size(120, 23)));
    repaint(&harness, ExtMode::Tui).await;
    let card = harness.card();
    assert!(
        card[0].ends_with(" Async agents · 6 agents running"),
        "23 rows leave four, and the full block is five lines (four runs and its `… +2 more` fold): the progressive card: {card:?}"
    );
    assert!(card.len() <= 4, "within the four available rows: {card:?}");

    // Non-vacuity: the same six runs on the 30-row fallback (eleven rows) are the full block.
    harness.extension.replace_async_widget_session(None);
    harness.set_terminal(None);
    repaint(&harness, ExtMode::Tui).await;
    let fallback = harness.card();
    assert!(
        !is_one_line_card(&fallback) && !fallback[0].contains("Async agents"),
        "with no terminal attached, pi's `|| 30` keeps the full block: {fallback:?}"
    );
}

/// Verify 4 — a resize RELOCKS the card, for rows and for columns, and the card is republished on
/// the resize itself (no refresh edge in between), as pi's mounted component re-renders on one.
///
/// Rows: 40 rows lay out the full block; a resize to 20 rows republishes the one-line card; back
/// to 40, the full block. Columns: a one-line lock taken at 40×120 holds while it matches; a
/// resize to 40×100 changes only the columns, and the card is relocked (pi `widgetSessionMatches`
/// compares both, `:2549-2553`).
///
/// RED at HEAD: does not compile (`terminal_resized` is new). Named mutations, RED: the `Resize`
/// arm of `handle_async_widget_event` answering `Unhandled` without republishing (the card stays
/// as it was after every resize); `WidgetLayoutSession::matches` comparing rows only (the
/// columns-only resize leaves the one-line lock in place).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resize_relocks_the_card_for_rows_and_for_columns() {
    let harness = Harness::full().await;
    harness.set_terminal(Some(size(120, 40)));
    track_runs(&harness, "suba211resize", 2).await;
    repaint(&harness, ExtMode::Tui).await;
    assert!(!is_one_line_card(&harness.card()), "{:?}", harness.card());

    let before = harness.publishes();
    harness.set_terminal(Some(size(120, 20)));
    harness.host.terminal_resized(size(120, 20)).await;
    assert!(
        harness.publishes() > before,
        "the resize itself republished the card"
    );
    assert!(
        is_one_line_card(&harness.card()),
        "relocked to the one-line tier: {:?}",
        harness.card()
    );

    harness.set_terminal(Some(size(120, 40)));
    harness.host.terminal_resized(size(120, 40)).await;
    assert!(
        !is_one_line_card(&harness.card()),
        "and back to the full block: {:?}",
        harness.card()
    );

    // Columns only.
    harness.lock_one_line_at(size(120, 40));
    harness.host.terminal_resized(size(120, 40)).await;
    assert!(
        is_one_line_card(&harness.card()),
        "a matching lock holds: {:?}",
        harness.card()
    );
    harness.set_terminal(Some(size(100, 40)));
    harness.host.terminal_resized(size(100, 40)).await;
    assert!(
        !is_one_line_card(&harness.card()),
        "a columns-only resize relocked it: {:?}",
        harness.card()
    );
}

/// Verify 5 — a running parallel launch whose `status.json` has no steps yet counts its AGENTS in
/// both the widget header and the fleet line. The roster is the one the launch records in the
/// production tracker (`JobTracker::set_launch_roster`, called by `spawn_background_steps`), read
/// through the production `SubagentExecutor::fleet_state`.
///
/// RED at HEAD: `LaunchRoster` / `set_launch_roster` do not exist (does not compile); the
/// behaviour it replaces counted `1 agent running` / `1 active agent`. Named mutations, RED:
/// `fleet_state` not copying `job.launch_roster` onto the view (both surfaces read `1`);
/// `AsyncJobSnapshot::from_run_view` ignoring the roster (the widget reads `1`, the fleet `3` —
/// the disagreement the shared roster exists to prevent); the fleet's synthesis arm removed (the
/// fleet reads `1`, the widget `3`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_step_less_parallel_launch_counts_its_agents_in_the_widget_and_the_fleet() {
    use crate::background::tracker::LaunchRoster;
    use crate::background::{RunId, RunMode, RunPaths, RunState, RunStatus};

    let harness = Harness::start_with(RegistrationMode::Full, |config| {
        config.async_widget_layout = Some(serde_json::Value::String("rows".to_string()));
    })
    .await;
    let roots = crate::background::run_artifact_roots_in(&harness.config.roots, &harness.cwd);
    let id = RunId::from_token("suba211roster".to_string());
    let paths = RunPaths::for_run(&roots.async_root, &roots.results_dir, &id);
    tokio::fs::create_dir_all(&paths.run_dir).await.unwrap();
    let mut status = RunStatus::queued(id.clone(), RunMode::Parallel, None);
    status.state = RunState::Running;
    status.session_id = crate::identity::SessionId::parse(TEST_SESSION_ID);
    status.started_at = crate::time::now_epoch_millis();
    status.last_update = status.started_at;
    assert!(
        status.steps.is_empty(),
        "the runner has written no step yet"
    );
    crate::background::atomic::write_atomic_json(&paths.status, &status)
        .await
        .unwrap();
    let tracker = harness.extension.executor().tracker().clone();
    tracker.track(id.clone(), paths, None).await;
    tracker.set_launch_roster(
        &id,
        LaunchRoster {
            agents: vec!["scout".into(), "planner".into(), "worker".into()],
            parallel_group: true,
        },
    );
    tracker.tick_once().await;
    repaint(&harness, ExtMode::Tui).await;

    let card = harness.card();
    assert!(
        card[0].ends_with(" Async agents · 3 agents running"),
        "the widget header counts the launch's agents: {card:?}"
    );
    let fleet = harness
        .last_widget(FLEET_STATUS_WIDGET_KEY)
        .expect("the fleet line was published")
        .expect("as content")
        .join("\n");
    assert!(
        fleet.contains("3 active agents"),
        "the fleet line counts the same agents: {fleet}"
    );
}
