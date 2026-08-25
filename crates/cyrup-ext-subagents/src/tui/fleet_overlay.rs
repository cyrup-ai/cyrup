//! The **live** subagent fleet inspector — [`super::fleet::SubagentFleetComponent`] wired to a real
//! terminal through the host's interactive-overlay seam ([`cyrup_ext::InteractiveOverlay`]).
//!
//! This module is the missing half of the `fleet.ts` port. `fleet.rs` ports every rule pi's
//! component has — selection, scrolling, the steer line editor, the stop confirmation, the busy
//! latch, the transcript cache — but pi's component is hosted: `openSubagentFleet` hands it to
//! `ctx.ui.custom(factory, { overlay: true, … })` (`fleet.ts:869-875`), and the TUI then paints it,
//! feeds it every keystroke, and re-renders it on the component's own `setInterval`
//! (`fleet.ts:516-521`). Without that host, `handle_input`, `finish_action` and `set_terminal_rows`
//! had no production caller at all: the inspector rendered ONE frame and was dropped, so no key ever
//! moved the selection and `terminal_rows` was permanently its `32` default.
//!
//! [`FleetOverlay`] is the host adapter. It owns the component and three things pi gets for free
//! from being JavaScript:
//!
//! 1. **The refresh tick.** pi's constructor arms `setInterval(() => { this.invalidate();
//!    this.tui.requestRender(); }, refreshMs ?? 750)` (`:516-521`). cyrup's snapshot source is
//!    ASYNC ([`SubagentExecutor::fleet_state`] scans the async root), and
//!    [`cyrup_ext::InteractiveOverlay::tick`] is sync, so the tick SPAWNS the re-collection on the
//!    captured runtime handle and applies whichever result has landed by the next tick. The
//!    component still sees exactly pi's `set_state` + `invalidate()` pair, one tick later.
//! 2. **Action dispatch.** pi's `runAction(...)` (`:585-597`) starts a promise from a synchronous
//!    handler and settles it into `setActionNotice`. `handle_input` returns
//!    [`FleetInputOutcome::RunAction`] instead ([`super::fleet`] documents why), so this adapter is
//!    the "owner" that doc names: it spawns the control op and feeds the answer back through
//!    [`SubagentFleetComponent::finish_action`] — pi's `.then(result => this.setActionNotice(...))`
//!    / `.catch(...)`, including the busy latch that both sides clear.
//! 3. **The terminal height.** pi reads `this.tui.terminal?.rows` inside `render` (`:791`); the
//!    seam reports the host frame's row count on every paint, which
//!    [`SubagentFleetComponent::set_terminal_rows`] consumes before rendering.
//!
//! # Honest deltas vs. pi
//!
//! Both are inherited from `fleet.rs` rather than introduced here, and neither is a behaviour this
//! adapter could supply: the steer delivery mode reaches
//! [`SubagentExecutor::control_steer`], which takes no mode field yet (`fleet.rs` delta 1), and
//! there is no Herdr inspector to route `H` to (`fleet.rs` delta 2), so `has_inspect` is `false`
//! and `H` takes pi's own "unavailable in this context" branch (`fleet.ts:692`).

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_ext::{InteractiveOverlay, OverlayKey, OverlayKeyCode, OverlayLine, OverlayOutcome};

use super::fleet::{
    FleetActionResult, FleetInputOutcome, FleetKey, FleetPendingAction, SubagentFleetComponent,
};
use super::fleet_state::FleetState;
use super::fleet_theme;
use crate::extension::SubagentExecutor;

/// An in-flight async job the sync overlay callbacks started, polled on the next tick.
type Pending<T> = Option<tokio::sync::oneshot::Receiver<T>>;

/// The live fleet inspector: [`SubagentFleetComponent`] plus the host-driving glue pi's
/// `ctx.ui.custom(…, { overlay: true })` supplies.
pub struct FleetOverlay {
    component: SubagentFleetComponent,
    executor: Arc<SubagentExecutor>,
    cwd: PathBuf,
    /// The runtime the sync seam callbacks spawn their async work on. Captured at construction,
    /// which always happens inside the extension's own async command handler, so a handle exists.
    handle: tokio::runtime::Handle,
    /// pi `options.refreshMs ?? REFRESH_MS` (`fleet.ts:520`).
    refresh_ms: u64,
    /// An in-flight [`SubagentExecutor::fleet_state`] re-collection (pi's synchronous
    /// `collectFleetSnapshot` inside `invalidate()`).
    state_job: Pending<FleetState>,
    /// An in-flight control op (pi's `runAction` promise).
    action_job: Pending<FleetActionResult>,
}

impl FleetOverlay {
    /// Wrap a constructed component. `refresh_ms` is
    /// [`super::fleet::FleetViewOptions::refresh_ms`], carried separately because the component
    /// keeps its options private.
    #[must_use]
    pub fn new(
        component: SubagentFleetComponent,
        executor: Arc<SubagentExecutor>,
        cwd: PathBuf,
        refresh_ms: u64,
        handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            component,
            executor,
            cwd,
            handle,
            refresh_ms,
            state_job: None,
            action_job: None,
        }
    }

    /// Read-only access to the driven component (tests, and an owner that wants to inspect the
    /// live selection).
    #[must_use]
    pub fn component(&self) -> &SubagentFleetComponent {
        &self.component
    }

    /// Start the next `fleet_state` re-collection, unless one is already in flight.
    fn spawn_state_refresh(&mut self) {
        if self.state_job.is_some() {
            return;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let executor = Arc::clone(&self.executor);
        let cwd = self.cwd.clone();
        self.handle.spawn(async move {
            // `include_history: true` and `fleet_inspector_open: true` — pi's inspector lists
            // finished runs too (`fleet.ts:192-203`) and its `state.fleetInspectorOpen` latch is
            // raised for as long as the overlay is up (`:844-845`).
            let state = executor.fleet_state(&cwd, true, true).await;
            let _ = tx.send(state);
        });
        self.state_job = Some(rx);
    }

    /// Start one control op (pi `runAction`, `fleet.ts:585-597`).
    fn spawn_action(&mut self, action: FleetPendingAction) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let executor = Arc::clone(&self.executor);
        let cwd = self.cwd.clone();
        self.handle.spawn(async move {
            let result = run_fleet_action(&executor, &cwd, action).await;
            let _ = tx.send(result);
        });
        self.action_job = Some(rx);
    }

    /// Poll both in-flight jobs; returns `true` when either landed and changed the frame.
    fn drain_jobs(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = self.state_job.as_mut() {
            match rx.try_recv() {
                Ok(state) => {
                    self.state_job = None;
                    // pi's `invalidate()` (`fleet.ts:832-835`), verbatim: drop the transcript cache,
                    // then re-fold the snapshot over the fresh state.
                    self.component.set_state(state);
                    self.component.invalidate();
                    changed = true;
                }
                // The spawned task was dropped without answering (runtime shutting down): clear the
                // slot so the next tick can start a fresh scan rather than waiting forever.
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => self.state_job = None,
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = self.action_job.as_mut() {
            match rx.try_recv() {
                Ok(result) => {
                    self.action_job = None;
                    // pi `.then(result => this.setActionNotice(result))` + `.finally(() => {
                    // this.actionBusy = false; … })` (`fleet.ts:591-596`) — `finish_action` is both.
                    self.component.finish_action(result);
                    changed = true;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.action_job = None;
                    // pi's `.catch(error => this.setActionNotice({ text: …, isError: true }))`
                    // (`:592`): a control op that vanished without answering must still clear the
                    // busy latch, or every later action is silently refused.
                    self.component
                        .finish_action(FleetActionResult::error("Fleet action was cancelled."));
                    changed = true;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }
        }
        changed
    }
}

impl InteractiveOverlay for FleetOverlay {
    fn render(&mut self, width: usize, height: usize) -> Vec<OverlayLine> {
        // pi `const rows = this.tui.terminal?.rows ?? 32` (`fleet.ts:791`) — the one place the
        // component learns how tall the terminal is. Before the seam existed this stayed at its
        // `32` default forever, so the roster never grew or shrank with the window.
        self.component.set_terminal_rows(height);
        let frame = self.component.render(width, crate::time::now_epoch_millis());
        fleet_theme::to_overlay_lines(&frame)
    }

    fn handle_key(&mut self, key: OverlayKey) -> OverlayOutcome {
        let Some(mapped) = to_fleet_key(key) else { return OverlayOutcome::Ignored };
        match self.component.handle_input(mapped) {
            FleetInputOutcome::Ignored => OverlayOutcome::Ignored,
            FleetInputOutcome::Rerender => OverlayOutcome::Redraw,
            FleetInputOutcome::Close => OverlayOutcome::Close,
            FleetInputOutcome::RunAction(action) => {
                self.spawn_action(*action);
                OverlayOutcome::Redraw
            }
        }
    }

    fn refresh_ms(&self) -> u64 {
        self.refresh_ms
    }

    fn tick(&mut self) -> bool {
        // Apply whatever landed since the last tick FIRST, then start the next scan — so a slow
        // filesystem scan can never stack up more than one in-flight job.
        let changed = self.drain_jobs();
        self.spawn_state_refresh();
        changed
    }
}

/// Perform one [`FleetPendingAction`] against the executor — pi's `FleetActionHandlers` bundle
/// (`fleet.ts:847-867`), whose fallback strings are reproduced verbatim.
async fn run_fleet_action(
    executor: &SubagentExecutor,
    cwd: &std::path::Path,
    action: FleetPendingAction,
) -> FleetActionResult {
    match action {
        FleetPendingAction::Steer { target, message, mode } => {
            // SUBA-049: `fleet.rs` delta 1 is CLOSED. The `Tab` cycle's mode used to be logged and
            // then dropped — `control_steer` had no delivery-mode argument, so all three settings
            // delivered identically and the prompt lied about what it was about to do. It is now
            // carried through to the `SteerRequest` and honoured by the child's inbox.
            tracing::debug!(
                target: "cyrup_ext_subagents::fleet",
                run_id = %target.run_id,
                mode = mode.as_str(),
                "fleet steer dispatched"
            );
            let fallback = format!("Failed to steer async run {}.", target.run_id);
            let dir = target.async_dir.to_str();
            super::fleet::action_result_from_control(
                executor
                    .control_steer(
                        cwd,
                        Some(target.run_id.as_str()),
                        dir,
                        Some(message.as_str()),
                        None,
                        target.index,
                        Some(mode.as_str()),
                    )
                    .await,
                &fallback,
            )
        }
        FleetPendingAction::Stop { target } => {
            let fallback = format!("Failed to stop async run {}.", target.run_id);
            super::fleet::action_result_from_control(
                executor
                    .control_stop(cwd, Some(target.run_id.as_str()), target.async_dir.to_str())
                    .await,
                &fallback,
            )
        }
        // pi makes `inspect` OPTIONAL on the handler bundle (`fleet.ts:51`) and the component
        // refuses `H` outright when it is absent (`:692`), so this is unreachable from
        // `handle_input` while `has_inspect` is false. Answered with pi's own message rather than a
        // panic, because "unreachable" is a property of the caller, not of this function.
        FleetPendingAction::Inspect { target } => FleetActionResult::error(format!(
            "Failed to open Herdr inspector for async run {}.",
            target.run_id
        )),
    }
}

/// A host [`OverlayKey`] as the [`FleetKey`] pi's `handleInput` matches on (`fleet.ts:606-713`), or
/// `None` when the key is not one the inspector binds.
///
/// `Shift+K`/`Shift+J` arrive as `Char('K')`/`Char('J')` — the seam ships the terminal's own
/// shift-resolved character, which is exactly what `matchesKey(data, Key.shift("k"))` distinguishes
/// off.
#[must_use]
pub fn to_fleet_key(key: OverlayKey) -> Option<FleetKey> {
    Some(match key.code {
        OverlayKeyCode::Up => FleetKey::Up,
        OverlayKeyCode::Down => FleetKey::Down,
        OverlayKeyCode::Home => FleetKey::Home,
        OverlayKeyCode::End => FleetKey::End,
        OverlayKeyCode::PageUp => FleetKey::PageUp,
        OverlayKeyCode::PageDown => FleetKey::PageDown,
        OverlayKeyCode::Enter => FleetKey::Enter,
        OverlayKeyCode::Escape => FleetKey::Escape,
        OverlayKeyCode::Tab => FleetKey::Tab,
        OverlayKeyCode::Backspace => FleetKey::Backspace,
        // pi matches the CONTROL forms by name (`matchesKey(data, "ctrl+c")`, `"ctrl+o"`); every
        // other Ctrl-chord is unbound and must not fall through as a printable character, or
        // `Ctrl+S` would start typing an `s` into a steer draft.
        OverlayKeyCode::Char(c) if key.ctrl => match c.to_ascii_lowercase() {
            'c' => FleetKey::CtrlC,
            'o' => FleetKey::CtrlO,
            _ => return None,
        },
        // An Alt-chord is likewise unbound upstream.
        OverlayKeyCode::Char(_) if key.alt => return None,
        OverlayKeyCode::Char(c) => FleetKey::Char(c),
        OverlayKeyCode::Delete
        | OverlayKeyCode::BackTab
        | OverlayKeyCode::Left
        | OverlayKeyCode::Right
        | OverlayKeyCode::Insert
        | OverlayKeyCode::F(_) => return None,
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::tui::fleet::SteerDeliveryMode;

    fn plain(code: OverlayKeyCode) -> OverlayKey {
        OverlayKey::plain(code)
    }

    #[test]
    fn every_key_upstream_binds_crosses_the_seam() {
        assert_eq!(to_fleet_key(plain(OverlayKeyCode::Up)), Some(FleetKey::Up));
        assert_eq!(to_fleet_key(plain(OverlayKeyCode::Down)), Some(FleetKey::Down));
        assert_eq!(to_fleet_key(plain(OverlayKeyCode::Home)), Some(FleetKey::Home));
        assert_eq!(to_fleet_key(plain(OverlayKeyCode::End)), Some(FleetKey::End));
        assert_eq!(to_fleet_key(plain(OverlayKeyCode::PageUp)), Some(FleetKey::PageUp));
        assert_eq!(to_fleet_key(plain(OverlayKeyCode::PageDown)), Some(FleetKey::PageDown));
        assert_eq!(to_fleet_key(plain(OverlayKeyCode::Enter)), Some(FleetKey::Enter));
        assert_eq!(to_fleet_key(plain(OverlayKeyCode::Escape)), Some(FleetKey::Escape));
        assert_eq!(to_fleet_key(plain(OverlayKeyCode::Tab)), Some(FleetKey::Tab));
        assert_eq!(to_fleet_key(plain(OverlayKeyCode::Backspace)), Some(FleetKey::Backspace));
    }

    #[test]
    fn shift_k_and_k_stay_distinguishable_across_the_seam() {
        assert_eq!(
            to_fleet_key(plain(OverlayKeyCode::Char('K'))),
            Some(FleetKey::Char('K')),
            "Shift+k must remain the detail-scroll binding, not the selection one"
        );
        assert_eq!(to_fleet_key(plain(OverlayKeyCode::Char('k'))), Some(FleetKey::Char('k')));
    }

    #[test]
    fn the_two_control_chords_upstream_binds_map_and_the_rest_are_dropped() {
        assert_eq!(
            to_fleet_key(OverlayKey::ctrl(OverlayKeyCode::Char('c'))),
            Some(FleetKey::CtrlC)
        );
        assert_eq!(
            to_fleet_key(OverlayKey::ctrl(OverlayKeyCode::Char('o'))),
            Some(FleetKey::CtrlO)
        );
        assert_eq!(
            to_fleet_key(OverlayKey::ctrl(OverlayKeyCode::Char('O'))),
            Some(FleetKey::CtrlO),
            "the terminal may report Ctrl+Shift+O; upstream matches the chord by name"
        );
        assert_eq!(
            to_fleet_key(OverlayKey::ctrl(OverlayKeyCode::Char('s'))),
            None,
            "an unbound Ctrl-chord must never reach the steer draft as a printable character"
        );
    }

    #[test]
    fn keys_upstream_does_not_bind_never_reach_the_component() {
        for code in [
            OverlayKeyCode::Delete,
            OverlayKeyCode::BackTab,
            OverlayKeyCode::Left,
            OverlayKeyCode::Right,
            OverlayKeyCode::Insert,
            OverlayKeyCode::F(5),
        ] {
            assert_eq!(to_fleet_key(plain(code)), None, "{code:?} is unbound upstream");
        }
        let alt = OverlayKey { code: OverlayKeyCode::Char('x'), ctrl: false, alt: true, shift: false };
        assert_eq!(to_fleet_key(alt), None);
    }

    #[test]
    fn the_steer_mode_names_upstreams_three_wire_tokens() {
        assert_eq!(SteerDeliveryMode::Steer.as_str(), "steer");
        assert_eq!(SteerDeliveryMode::FollowUp.as_str(), "follow_up");
        assert_eq!(SteerDeliveryMode::Auto.as_str(), "auto");
    }

    // -----------------------------------------------------------------------------------------
    // The hosted component (the half that had no production caller at all)
    // -----------------------------------------------------------------------------------------

    fn background_state(run_token: &str, agent: &str) -> super::FleetState {
        use crate::background::{RunId, RunMode, RunPaths, RunState, RunStatus, StepStatus};
        use crate::tui::fleet_state::AsyncRunView;
        let run_id = RunId::from_token(run_token.to_string());
        let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, None);
        status.state = RunState::Running;
        status.steps = vec![StepStatus::pending(agent.to_string())];
        super::FleetState {
            tracked_jobs: vec![AsyncRunView {
                paths: RunPaths::for_run(
                    std::path::Path::new("/tmp/async"),
                    std::path::Path::new("/tmp/results"),
                    &run_id,
                ),
                status,
                session_id: None,
                description: None,
                context: None,
                nested_children: Vec::new(),
            }],
            ..super::FleetState::default()
        }
    }

    fn overlay_over(state: super::FleetState, cwd: PathBuf) -> FleetOverlay {
        let component = SubagentFleetComponent::new(
            state,
            crate::tui::fleet::FleetViewOptions::default(),
            None,
            true,
            false,
        );
        FleetOverlay::new(
            component,
            Arc::new(SubagentExecutor::new()),
            cwd,
            crate::tui::fleet::REFRESH_MS,
            tokio::runtime::Handle::current(),
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_overlay_paints_styled_lines_sized_from_the_hosts_reported_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut overlay = overlay_over(super::FleetState::default(), dir.path().to_path_buf());

        // pi `bodyHeight = max(2, floor(rows * 0.85) - 6)` + 6 chrome rows (`fleet.ts:791-792`):
        // the whole frame is `floor(rows * 0.85)` tall, and it is `set_terminal_rows` — the method
        // that had no caller — that makes it track the terminal at all.
        let short = InteractiveOverlay::render(&mut overlay, 80, 24);
        let tall = InteractiveOverlay::render(&mut overlay, 80, 60);
        assert_eq!(short.len(), 24 * 85 / 100);
        assert_eq!(tall.len(), 60 * 85 / 100);
        assert_ne!(short.len(), tall.len(), "the frame must track the terminal height");

        // Style crosses the seam structurally, not as text.
        let corner = &tall[0].spans[0];
        assert!(corner.text.starts_with('╭'), "first row is the box top: {:?}", corner.text);
        assert_eq!(
            corner.fg,
            Some(cyrup_ext::OverlayColor::DarkGray),
            "the border keeps its colour crossing the overlay seam"
        );
        let title = tall[1]
            .spans
            .iter()
            .find(|s| s.text.contains("Subagent fleet inspector"))
            .expect("the title row");
        assert!(title.bold, "the title keeps its bold crossing the overlay seam");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn refresh_ms_is_upstreams_cadence_and_a_tick_applies_a_refreshed_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut overlay = overlay_over(super::FleetState::default(), dir.path().to_path_buf());
        assert_eq!(InteractiveOverlay::refresh_ms(&overlay), crate::tui::fleet::REFRESH_MS);

        // The first tick has nothing to apply yet — it STARTS the scan (pi's `setInterval` body is
        // `invalidate()` + `requestRender()`; cyrup's scan is async, so it lands a tick later).
        assert!(!overlay.tick(), "nothing has landed yet");
        for _ in 0..200 {
            if overlay.tick() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the spawned fleet_state refresh never landed");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_steer_keystroke_dispatches_a_control_op_and_its_answer_reaches_the_component() {
        let dir = tempfile::tempdir().unwrap();
        let mut overlay = overlay_over(background_state("ovlrun00001", "coder"), dir.path().to_path_buf());
        assert!(overlay.component().action_notice().is_none());

        // `s` opens the steer draft, characters type into it, Enter dispatches — every one of these
        // reaching `handle_input`, which had no production caller before this module existed.
        assert_eq!(
            overlay.handle_key(plain(OverlayKeyCode::Char('s'))),
            OverlayOutcome::Redraw
        );
        assert_eq!(overlay.component().steer_draft(), Some(""));
        for c in "go".chars() {
            assert_eq!(overlay.handle_key(plain(OverlayKeyCode::Char(c))), OverlayOutcome::Redraw);
        }
        assert_eq!(overlay.component().steer_draft(), Some("go"));
        assert_eq!(overlay.handle_key(plain(OverlayKeyCode::Enter)), OverlayOutcome::Redraw);

        // The control op runs off-thread; its answer lands through `finish_action` on a later tick.
        for _ in 0..400 {
            overlay.tick();
            if overlay.component().action_notice().is_some() {
                // The run does not exist on disk, so the honest answer is an error notice — what
                // matters is that an answer arrived at all and the busy latch cleared.
                let notice = overlay.component().action_notice().expect("notice");
                assert!(notice.is_error, "an unknown run must answer with an error notice");
                assert!(overlay.component().steer_draft().is_none(), "the draft is reset");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the dispatched steer never answered");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn escape_closes_and_an_unbound_key_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let mut overlay = overlay_over(background_state("ovlrun00002", "coder"), dir.path().to_path_buf());
        assert_eq!(overlay.handle_key(plain(OverlayKeyCode::Insert)), OverlayOutcome::Ignored);
        assert_eq!(overlay.handle_key(plain(OverlayKeyCode::Escape)), OverlayOutcome::Close);
    }

    #[tokio::test]
    async fn an_inspect_action_answers_upstreams_herdr_failure_text() {
        // `has_inspect` is false, so `handle_input` never emits this — but the handler must still
        // answer rather than panic if it ever does.
        let target = crate::tui::fleet::FleetActionTarget {
            run_id: "abc123".into(),
            async_dir: PathBuf::from("/tmp/none"),
            index: None,
        };
        let executor = crate::extension::SubagentExecutor::new();
        let result = run_fleet_action(
            &executor,
            std::path::Path::new("/tmp"),
            FleetPendingAction::Inspect { target },
        )
        .await;
        assert!(result.is_error);
        assert_eq!(result.text, "Failed to open Herdr inspector for async run abc123.");
    }
}
