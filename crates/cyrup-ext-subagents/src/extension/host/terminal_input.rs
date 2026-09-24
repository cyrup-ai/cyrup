//! UW-7 — the raw-terminal-input half of the always-on fleet-status widget: pi's
//! `ctx.ui.onTerminalInput((data) => this.handleKey(data))`
//! (`pi-subagents/src/tui/fleet-status.ts:577-578` @v0.68.0) and the handler it wires up
//! (`:696-753`).
//!
//! Built to the shape of [`super::shortcuts`] — module doc, then the dispatch function kept beside
//! the registration it belongs to "so the two cannot drift". The declaration lives in
//! [`super::native_impl`]'s `init` (`api.subscribe_terminal_input()`) and the inbound trait method
//! is [`cyrup_ext::NativeExtension::on_terminal_input`]; both delegate here.
//!
//! # Why the handler can be synchronous
//!
//! [`cyrup_ext::NativeExtension::on_terminal_input`] is `fn`, not `async fn` — only the WASM tier
//! is async. So the whole body is: lock the widget, call
//! [`SubagentFleetStatus::handle_key`](crate::tui::fleet_status::SubagentFleetStatus::handle_key),
//! maybe republish, return. No runtime, no `.await`, no `block_on`.
//!
//! # Why `Enter` must SPAWN, and why that is not tidiness
//!
//! [`FleetStatusKeyOutcome::OpenInspector`] leads to the fleet overlay, which reaches
//! [`cyrup_ext::host::HostServices::open_overlay`] — whose contract is, in its own words, *"BLOCK
//! until the user closes it"*. This handler runs inside the TUI's key fold, on the task that
//! services `ui_rx` (`cyrup-tui/src/app/run_action.rs`, `App::on_input_event`), and that task is
//! the only one that can ever drive the modal to a close. Blocking there wedges the entire TUI —
//! the identical self-deadlock the extension-shortcut arm of `dispatch_run_action` spends eighteen
//! lines explaining, except that the shortcut path can escape it by spawning and this one cannot,
//! because it owes its caller a consume-or-deliver answer synchronously.
//!
//! Upstream solves it invisibly and for exactly this reason: it opens the inspector on a DETACHED
//! microtask and consumes the keystroke immediately —
//!
//! ```ts
//! this.inspectorOpen = true;
//! this.refresh();
//! const selectedKey = this.selectedKey;
//! void Promise.resolve()
//!     .then(() => this.openInspector(selectedKey))
//!     .catch((error) => ctx.ui.notify(…, "error"))
//!     .finally(() => { this.inspectorOpen = false; this.refresh(); });
//! return { consume: true };
//! ```
//!
//! (`fleet-status.ts:735-750` @v0.68.0). [`SubagentsExtension::spawn_fleet_inspector`] is that
//! `void Promise.resolve().then(...)`, with `tokio::spawn` in place of the microtask, the `catch`
//! as a `notify`, and the `finally` as an unconditional `set_inspector_open(false)`.
//!
//! # `[CYRUP-DELTA]` — cyrup answers the focus question honestly
//!
//! pi's first guard is `editorHasFocus()` (`fleet-status.ts:965-976`), which STRUCTURALLY
//! DUCK-TYPES `tui.focusedComponent` for five `EditorComponent` methods, excusing itself in source
//! with *"pi-tui exposes focus mutation but no focus getter"* and *"instanceof is unreliable
//! across jiti module boundaries"*. cyrup has neither problem: the answer arrives as a plain
//! `bool` over [`cyrup_ext::host::HostServices::editor_has_focus`], read off the TUI's own routing
//! chain (no overlay, no selector, no loader ⇒ the editor is what the key would reach).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cyrup_ext::TerminalInputResult;
use cyrup_ext::host::{HostServices, NotifyKind};

use super::SubagentsExtension;
use crate::error::SubagentError;
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::requests::StatusViewSelector;
use crate::tui::fleet_status::{
    FLEET_STATUS_WIDGET_KEY, FleetStatusKey, FleetStatusKeyOutcome, FleetViewPlacement,
    SubagentFleetStatus,
};

/// The width the roster is rendered at when the host has not told us the terminal's. Same value,
/// and for the same reason, as [`SubagentsExtension::refresh_fleet_status_widget`]'s: cyrup's
/// `set_widget` is a fire-and-forget payload rather than pi's re-invoked factory, so the payload is
/// laid out once at a conventional width instead of at the live column count.
const FALLBACK_WIDTH: usize = 100;

/// Everything [`FleetInspectorHandle::open`] needs, as owned handles.
///
/// It exists so the fleet overlay can be opened from TWO places that cannot share a `&self`:
/// `/subagents-fleet`'s command handler, which is `async` and borrows the extension for its whole
/// duration, and UW-7's `Enter`, which must hand the work to a DETACHED task (see the module doc)
/// and therefore needs `'static`. Every field is already an `Arc` on [`SubagentsExtension`], so
/// this is four pointer clones and no duplicated logic — the alternative, a second copy of
/// `show_fleet`'s body inlined into the spawn, is exactly the drift this type prevents.
#[derive(Clone)]
pub(crate) struct FleetInspectorHandle {
    pub(crate) executor: Arc<SubagentExecutor>,
    /// pi's `let fleetOpen = false` re-entrancy guard (`slash/slash-commands.ts:632`).
    pub(crate) fleet_open: Arc<AtomicBool>,
    /// pi's `state.fleetInspectorOpen` (`tui/fleet.ts:844-845`).
    pub(crate) fleet_inspector_open: Arc<AtomicBool>,
    pub(crate) fleet_status: Arc<std::sync::Mutex<SubagentFleetStatus>>,
    /// SUBA-061 — `config.fleetKeybindings`, resolved. PR #151 made [`Self::open`] the single
    /// place the inspector opens (both `/subagents-fleet` and the roster's `Enter`), so this is
    /// the one place the bindings are threaded in (pi `extension/index.ts:502`,
    /// `slash-commands.ts:863` @v0.68.0).
    pub(crate) keybindings: crate::tui::fleet::FleetKeybindings,
}

impl FleetInspectorHandle {
    /// pi's `showFleet(ctx)` (`slash/slash-commands.ts:634-660` @v0.43.0) and, with an
    /// `initial_key`, the `openInspector` callback `SubagentFleetStatus` is constructed with
    /// (`extension/index.ts:497-510` @v0.68.0: `openSubagentFleet(ctx, state, { initialKey:
    /// itemKey, … })`).
    ///
    /// The body moved here verbatim from `SubagentsExtension::show_fleet`, which now delegates;
    /// see this type's doc for why.
    pub(crate) async fn open(
        &self,
        cwd: &Path,
        has_ui: bool,
        initial_key: Option<String>,
    ) -> Result<String, SubagentError> {
        use crate::tui::fleet::{FleetOpenOutcome, FleetViewOptions, open_subagent_fleet};

        // pi reads `fleetOpen` BEFORE it touches anything else that could change it.
        let already_open = self.fleet_open.load(Ordering::Acquire);
        let state = self
            .executor
            .fleet_state(cwd, true, self.fleet_inspector_open.load(Ordering::Acquire))
            .await;

        match open_subagent_fleet(
            has_ui,
            already_open,
            state,
            FleetViewOptions {
                keybindings: self.keybindings.clone(),
                ..FleetViewOptions::default()
            },
            initial_key,
            // `has_actions`: steer/stop route to `control_steer`/`control_stop`.
            true,
            // `has_inspect`: the inspector backends exist now —
            // `inspectors::plugins::builtin_inspector_plugins()` is herdr then ghostty — so
            // `Enter`/`H` route to a real `inspector.open` through
            // `SubagentExecutor::inspector_open` (pi `fleet.ts:1417-1430`). With NO host
            // available the key still answers: the dispatcher's own
            // `NO_INSPECTOR_PLUGIN_AVAILABLE` sentence names `inspector.command` as the way out,
            // which is a better answer than the unavailable notice this used to force.
            true,
        ) {
            FleetOpenOutcome::NoUiFallback => self.text_fallback(cwd).await,
            FleetOpenOutcome::AlreadyOpen => {
                Ok("Subagent fleet inspector is already open.".to_string())
            }
            FleetOpenOutcome::Opened {
                component,
                clear_widget_key,
            } => {
                self.fleet_open.store(true, Ordering::Release);
                self.fleet_inspector_open.store(true, Ordering::Release);
                // pi `ctx.ui.setWidget(FLEET_STATUS_WIDGET_KEY, undefined)` (`tui/fleet.ts:846`):
                // the status widget must be gone before the overlay paints.
                let services = self.executor.host_services();
                if let Some(services) = services.as_ref() {
                    services.set_widget(
                        clear_widget_key,
                        None,
                        cyrup_ext::host::WidgetPlacement::default(),
                    );
                }
                if let Ok(mut widget) = self.fleet_status.lock() {
                    widget.set_inspector_open(true);
                }

                let overlay = crate::tui::fleet_overlay::FleetOverlay::new(
                    *component,
                    Arc::clone(&self.executor),
                    cwd.to_path_buf(),
                    FleetViewOptions::default().refresh_ms,
                    tokio::runtime::Handle::current(),
                );
                // BLOCKS until the human closes the modal — pi's `await ctx.ui.custom(...)`.
                let driven = services
                    .as_ref()
                    .is_some_and(|services| services.open_overlay(Box::new(overlay)));

                // pi's `finally` (`slash-commands.ts:646-647` + `tui/fleet.ts:876-878`): both
                // latches are restored however the overlay ended.
                if let Ok(mut widget) = self.fleet_status.lock() {
                    widget.set_inspector_open(false);
                }
                self.fleet_inspector_open.store(false, Ordering::Release);
                self.fleet_open.store(false, Ordering::Release);

                if driven {
                    // The overlay said everything it had to say on screen; pi's
                    // `ctx.ui.custom<undefined>` likewise resolves with no value, and a non-empty
                    // return here would surface as a redundant notification
                    // (`cyrup-session-svc/src/session.rs`'s `try_execute_extension_command`).
                    return Ok(String::new());
                }
                // No terminal to drive it on — pi's `!ctx.hasUI` outcome, one level later.
                self.text_fallback(cwd).await
            }
        }
    }

    /// pi's `!ctx.hasUI` branch: the SAME `control_status_view(view: "fleet")` entry point the
    /// `subagent({ action: "status", view: "fleet" })` tool call uses, so the two surfaces can
    /// never render different fleets (R-SA-130).
    async fn text_fallback(&self, cwd: &Path) -> Result<String, SubagentError> {
        self.executor
            .control_status_view(
                cwd,
                None,
                None,
                false,
                StatusViewSelector {
                    view: Some("fleet"),
                    ..StatusViewSelector::default()
                },
            )
            .await
            .map_err(SubagentError::Management)
    }
}

impl SubagentsExtension {
    /// The owned view of this extension the fleet overlay can be opened through, including from a
    /// detached task. See [`FleetInspectorHandle`].
    pub(crate) fn fleet_inspector_handle(&self) -> FleetInspectorHandle {
        FleetInspectorHandle {
            executor: Arc::clone(&self.executor),
            fleet_open: Arc::clone(&self.fleet_open),
            fleet_inspector_open: Arc::clone(&self.fleet_inspector_open),
            fleet_status: Arc::clone(&self.fleet_status),
            keybindings: self.fleet_keybindings.clone(),
        }
    }

    /// UW-7 — one raw terminal chunk, folded through the fleet-status widget. pi's
    /// `handleKey(data)` (`tui/fleet-status.ts:696-753` @v0.68.0) plus the two things the Rust
    /// port deliberately made the OWNER's job.
    ///
    /// # The owner's two jobs
    ///
    /// 1. **Supply the guards' inputs.** `handle_key` takes `editor_has_focus` and `editor_text`
    ///    as parameters where pi reads them off a live `ExtensionContext` (`:701`, `:708`); this
    ///    is where they come from — [`HostServices::editor_has_focus`] and
    ///    [`HostServices::editor_text`], the two interactive readbacks the TUI republishes on the
    ///    key path itself.
    /// 2. **Republish.** pi calls `this.refresh()` inline on every arm that moves the selection
    ///    (`:711`, `:720`, `:726`); the port moved that to the owner (this module's
    ///    `fleet_status` module doc says so), so a `Consume` that moved the roster would otherwise
    ///    leave a STALE widget on screen until the next `SessionStart`/`AgentEnd` edge. The
    ///    republish is gated on
    ///    [`render_key`](crate::tui::fleet_status::SubagentFleetStatus::render_key) actually
    ///    having moved, which is upstream's own change detector.
    ///
    /// `set_widget` is fire-and-forget (its own contract), so the republish is safe from a sync
    /// handler. The widget lock is released BEFORE any host call, so a host that re-enters cannot
    /// deadlock on it.
    ///
    /// Returns pi's `undefined` (`None`) for [`FleetStatusKeyOutcome::Pass`] — "I looked at it and
    /// did nothing", and the keystroke falls through to the editor.
    pub(crate) fn dispatch_terminal_input(&self, data: &str) -> Option<TerminalInputResult> {
        // pi's `getActiveUiContext()` guard (`:698`): with no capability backend bound there is no
        // UI to read the editor off, and nothing to publish a widget to.
        let services = self.executor.host_services()?;
        let key = FleetStatusKey::from_terminal_data(data);
        let now = crate::time::now_epoch_millis();

        let (outcome, republish, selected_key) = {
            let Ok(mut widget) = self.fleet_status.lock() else {
                return None;
            };
            let before = widget.render_key(now);
            let outcome =
                widget.handle_key(&key, services.editor_has_focus(), &services.editor_text());
            // The change detector, read on both sides of the same lock so no other writer can slip
            // between them.
            let republish = (widget.render_key(now) != before)
                .then(|| (widget.widget_lines(FALLBACK_WIDTH, now), widget.placement()));
            (outcome, republish, widget.selected_key().to_string())
        };

        if let Some((lines, placement)) = republish {
            let placement = match placement {
                FleetViewPlacement::BelowEditor => cyrup_ext::host::WidgetPlacement::BelowEditor,
                FleetViewPlacement::AboveEditor => cyrup_ext::host::WidgetPlacement::AboveEditor,
            };
            services.set_widget(FLEET_STATUS_WIDGET_KEY, lines.as_deref(), placement);
        }

        match outcome {
            FleetStatusKeyOutcome::Pass => None,
            FleetStatusKeyOutcome::Consume => Some(consume()),
            FleetStatusKeyOutcome::OpenInspector => {
                // `handle_key` has ALREADY set `inspector_open = true` (pi `:736`); the spawned
                // task owns clearing it, which is pi's `finally`.
                self.spawn_fleet_inspector(services, selected_key);
                Some(consume())
            }
        }
    }

    /// pi's `void Promise.resolve().then(() => this.openInspector(selectedKey)).catch(…)
    /// .finally(…)` (`tui/fleet-status.ts:741-750` @v0.68.0).
    ///
    /// **The spawn is mandatory, not stylistic.** See this module's doc: awaiting the overlay here
    /// would block the TUI's own run loop on a modal only that run loop can close.
    fn spawn_fleet_inspector(&self, services: Arc<dyn HostServices>, initial_key: String) {
        let handle = self.fleet_inspector_handle();
        let cwd = self.cwd.clone();
        // `Handle::try_current` rather than a bare `tokio::spawn`: the host dispatches
        // `on_terminal_input` from inside an async fold, so a runtime is always present in
        // production — but a caller that is not (a sync unit test, an embedder driving the
        // extension by hand) deserves the `finally` to run and a notification, not a panic
        // swallowed by the dispatcher's `catch_unwind`.
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            Self::clear_inspector_open(&handle);
            services.notify(
                "Cannot open the subagent fleet inspector: no async runtime on this thread.",
                NotifyKind::Error,
            );
            return;
        };
        rt.spawn(async move {
            // pi's `.catch((error) => ctx.ui.notify(…, "error"))`.
            if let Err(error) = handle.open(&cwd, true, Some(initial_key)).await {
                services.notify(&error.to_string(), NotifyKind::Error);
            }
            // pi's `.finally(() => { this.inspectorOpen = false; this.refresh(); })`. Run
            // unconditionally, and OUTSIDE `open`'s own `Opened` arm, because the `NoUiFallback`
            // and `AlreadyOpen` outcomes never reach that arm's restore and would otherwise leave
            // the widget latched shut for the rest of the session.
            Self::clear_inspector_open(&handle);
        });
    }

    /// pi's `finally` body, minus the `refresh()` the owner's own poll supplies.
    fn clear_inspector_open(handle: &FleetInspectorHandle) {
        if let Ok(mut widget) = handle.fleet_status.lock() {
            widget.set_inspector_open(false);
        }
    }
}

/// pi's `{ consume: true }` — the widget handled the key; do not forward it.
fn consume() -> TerminalInputResult {
    TerminalInputResult {
        consume: Some(true),
        data: None,
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    //! UW-7's extension half: the seam handler, its two guards, its republish, and the spawn that
    //! keeps `Enter` off the blocking path.

    use std::sync::Mutex;
    use std::time::Duration;

    use cyrup_ext::host::WidgetPlacement;
    use cyrup_ext::native::{ExtMode, NativeExtension};

    use super::*;
    use crate::background::RunMode;
    use crate::registration::SubagentExtensionConfig;

    /// SUBA-061 — `config.fleetKeybindings` reaches the ONE place the inspector opens (PR #151's
    /// `FleetInspectorHandle`), resolved per-action. Mutation killed: resolving the bindings to
    /// `FleetKeybindings::default()` at construction (the configured stop key is then lost).
    #[test]
    fn configured_fleet_keybindings_reach_the_inspector_handle() {
        use crate::tui::fleet::{FleetAction, FleetKey};
        let dir = tempfile::tempdir().expect("tempdir");
        let config = SubagentExtensionConfig {
            fleet_keybindings: Some(serde_json::json!({"stop": ["ctrl+x"]})),
            ..SubagentExtensionConfig::default()
        };
        let ext = SubagentsExtension::with_config_and_cwd(config, dir.path().to_path_buf());
        let handle = ext.fleet_inspector_handle();
        assert!(
            handle
                .keybindings
                .matches(FleetAction::Stop, FleetKey::Ctrl('x'))
        );
        assert!(
            !handle
                .keybindings
                .matches(FleetAction::Stop, FleetKey::Char('D'))
        );
    }
    use crate::tui::fleet_state::{FleetState, ForegroundControlView};
    use crate::tui::fleet_status::SubagentFleetStatus;

    /// A capability backend with the two readbacks UW-7's guards consult canned, recording every
    /// `set_widget` — the pattern `slash_inspect_rpc.rs`'s `WidgetRecorder` establishes, plus the
    /// blocking `open_overlay` the spawn test needs.
    #[derive(Default)]
    struct FleetKeyServices {
        editor_text: String,
        /// An `AtomicBool` rather than a plain one because
        /// [`crate::extension::SubagentExecutor::set_host_services`] is IDEMPOTENT — a second
        /// backend cannot replace the first — so a test that needs focus to CHANGE has to move it
        /// on the backend the extension already holds, exactly as the live TUI's mirror does.
        editor_has_focus: std::sync::atomic::AtomicBool,
        widgets: Mutex<Vec<(String, Option<Vec<String>>)>>,
        notifications: Mutex<Vec<String>>,
        /// Signalled once `open_overlay` has been entered, so a test can prove the spawned task
        /// really ran rather than inferring it from a fast return.
        entered_overlay: Mutex<Option<std::sync::mpsc::Sender<()>>>,
        /// Held by `open_overlay` until the test releases it — the stand-in for a human who has
        /// not closed the modal yet, which is precisely what would wedge the run loop.
        release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    }

    impl FleetKeyServices {
        fn focused(editor_text: &str) -> Self {
            Self {
                editor_text: editor_text.to_string(),
                editor_has_focus: std::sync::atomic::AtomicBool::new(true),
                ..Self::default()
            }
        }

        /// Move the focus readback, the way `App::publish_extension_readbacks` does when a
        /// selector mounts.
        fn set_focus(&self, has_focus: bool) {
            self.editor_has_focus.store(has_focus, Ordering::Release);
        }

        fn widgets(&self) -> Vec<(String, Option<Vec<String>>)> {
            self.widgets
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    impl HostServices for FleetKeyServices {
        fn editor_text(&self) -> String {
            self.editor_text.clone()
        }
        fn editor_has_focus(&self) -> bool {
            self.editor_has_focus.load(Ordering::Acquire)
        }
        fn set_widget(&self, key: &str, lines: Option<&[String]>, _placement: WidgetPlacement) {
            if let Ok(mut g) = self.widgets.lock() {
                g.push((key.to_string(), lines.map(<[String]>::to_vec)));
            }
        }
        fn notify(&self, message: &str, _kind: NotifyKind) {
            if let Ok(mut g) = self.notifications.lock() {
                g.push(message.to_string());
            }
        }
        fn open_overlay(
            &self,
            _overlay: Box<dyn cyrup_ext::host::overlay::InteractiveOverlay>,
        ) -> bool {
            if let Some(tx) = self
                .entered_overlay
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                let _ = tx.send(());
            }
            if let Some(rx) = self
                .release
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                // pi's `await ctx.ui.custom(...)`: BLOCKS until the human closes the modal.
                let _ = rx.recv_timeout(Duration::from_secs(30));
            }
            true
        }
    }

    fn control(run_id: &str, agent: &str) -> ForegroundControlView {
        ForegroundControlView {
            run_id: run_id.to_string(),
            mode: RunMode::Single,
            started_at: 0,
            updated_at: 0,
            current_agent: Some(agent.to_string()),
            tokens: Some(1500),
            ..ForegroundControlView::default()
        }
    }

    fn busy() -> FleetState {
        FleetState {
            foreground_controls: vec![control("a", "coder")],
            ..FleetState::default()
        }
    }

    /// An extension with a live capability backend and its fleet widget ARMED — the state a
    /// session reaches once `refresh_fleet_status_widget` has published a collapsed status line
    /// for at least one running agent. Without that, `handle_key`'s `entries.is_empty()` guard
    /// declines everything and no key test says anything.
    fn armed(services: Arc<FleetKeyServices>) -> (tempfile::TempDir, SubagentsExtension) {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig::default(),
            dir.path().to_path_buf(),
        );
        ext.executor().set_host_services(services);
        {
            let mut widget = ext.fleet_status.lock().expect("widget lock");
            widget.set_ui_available(true);
            widget.refresh(&busy(), crate::time::now_epoch_millis());
        }
        (dir, ext)
    }

    fn is_active(widget: &Mutex<SubagentFleetStatus>) -> bool {
        widget.lock().expect("widget lock").is_active()
    }

    /// **T8** — `↓` on an empty editor consumes the key, expands the roster, AND republishes the
    /// widget. The republish is the half the Rust port made the owner's job: upstream calls
    /// `this.refresh()` inline on that arm (`fleet-status.ts:711`), so without it the roster
    /// expands invisibly and the user sees the collapsed line ignore their keypress.
    ///
    /// MUTATION: delete the `if let Some((lines, placement)) = republish` block — the outcome is
    /// still `Consume` and the widget is still active, but no `set_widget` is recorded and the
    /// last two assertions fail. Observed RED.
    #[test]
    fn down_arrow_expands_the_roster_and_republishes() {
        let services = Arc::new(FleetKeyServices::focused(""));
        let (_dir, ext) = armed(Arc::clone(&services));

        let result = ext.dispatch_terminal_input("\x1b[B").expect("consumed");
        assert_eq!(result.consume, Some(true));
        assert!(is_active(&ext.fleet_status));

        let widgets = services.widgets();
        assert_eq!(widgets.len(), 1, "exactly one republish, got {widgets:?}");
        assert_eq!(widgets[0].0, FLEET_STATUS_WIDGET_KEY);
        let lines = widgets[0].1.as_ref().expect("the roster, not a removal");
        let text = lines.join("\n");
        assert!(
            text.contains("coder"),
            "the republished payload must be the EXPANDED roster, got:\n{text}"
        );
        assert!(
            !text.contains("↓/← to inspect"),
            "the collapsed hint line means the roster never expanded, got:\n{text}"
        );
    }

    /// **T9** — the empty-editor gate. `↓` with text in the buffer is NOT consumed: pi refuses to
    /// activate unless `ctx.ui.getEditorText() === ""` (`fleet-status.ts:708`), because otherwise
    /// the widget steals the cursor key from a user mid-sentence.
    ///
    /// MUTATION: pass `""` to `handle_key` instead of `services.editor_text()` — the key is
    /// consumed, the roster activates behind a half-typed prompt, and both assertions fail.
    /// Observed RED.
    #[test]
    fn down_with_text_in_the_editor_is_not_consumed() {
        let services = Arc::new(FleetKeyServices::focused("hi"));
        let (_dir, ext) = armed(Arc::clone(&services));

        assert!(
            ext.dispatch_terminal_input("\x1b[B").is_none(),
            "pi returns `undefined` here, which is `None`"
        );
        assert!(!is_active(&ext.fleet_status));
        assert!(
            services.widgets().is_empty(),
            "nothing moved, so nothing republishes"
        );
    }

    /// **T10** — losing focus deactivates an OPEN roster (pi `if (!this.editorHasFocus()) { if
    /// (this.active) this.deactivate(); return undefined; }`, `fleet-status.ts:701-704`).
    ///
    /// The TUI half of this pair — that the fold still runs, and reports `false`, while a
    /// selector is mounted — is
    /// `cyrup_tui::app::terminal_input::tests::the_fold_still_runs_and_reports_no_focus_behind_a_selector`.
    /// Together they are why the fold sits ahead of `handle_input`'s guards rather than inside
    /// them.
    ///
    /// MUTATION: pass `true` to `handle_key` instead of `services.editor_has_focus()` — the
    /// roster stays active behind the selector, the second assertion fails, and the first key
    /// after the picker closes is eaten by a widget the user cannot see. Observed RED.
    #[test]
    fn losing_focus_deactivates() {
        let focused = Arc::new(FleetKeyServices::focused(""));
        let (_dir, ext) = armed(Arc::clone(&focused));
        ext.dispatch_terminal_input("\x1b[B").expect("activated");
        assert!(is_active(&ext.fleet_status));

        // The selector opened: the SAME backend, now reporting that the editor is not the
        // focused component — which is what the TUI's per-key readback republish does.
        focused.set_focus(false);
        assert!(
            ext.dispatch_terminal_input("\x1b[B").is_none(),
            "with focus elsewhere the key is never ours"
        );
        assert!(!is_active(&ext.fleet_status));
    }

    /// An unrecognised chunk is pi's catch-all, not a no-op: it DEACTIVATES the roster and lets
    /// the key through (`fleet-status.ts:752-753`).
    ///
    /// MUTATION: give `dispatch_terminal_input` an early `if key.code ==
    /// FleetStatusKeyCode::Other { return None; }` — a `Pass` WITHOUT deactivating, which is what
    /// treating an unrecognised chunk as "nothing happened" amounts to. The roster survives a
    /// keystroke upstream closes it on, and the last assertion fails. Observed RED.
    #[test]
    fn an_unmatched_key_deactivates_and_falls_through() {
        let services = Arc::new(FleetKeyServices::focused(""));
        let (_dir, ext) = armed(Arc::clone(&services));
        ext.dispatch_terminal_input("\x1b[B").expect("activated");
        assert!(is_active(&ext.fleet_status));

        // Tab: a real key, deliberately outside the shared table's vocabulary.
        assert!(ext.dispatch_terminal_input("\t").is_none());
        assert!(!is_active(&ext.fleet_status));
    }

    /// **T11** — `Enter` on a roster row consumes the key and returns IMMEDIATELY, with the
    /// inspector opening on a detached task.
    ///
    /// This is the deadlock guard. `open_overlay` BLOCKS until the human closes the modal, and in
    /// production this handler runs on the task that services `ui_rx` — the only task that can
    /// ever deliver that close. Awaiting it inline wedges the whole TUI, keyboard included.
    /// Upstream avoids it with `void Promise.resolve().then(...)` (`fleet-status.ts:741-750`);
    /// `spawn_fleet_inspector` is that microtask.
    ///
    /// The bounded `timeout` is the honest way to pin it: the failure mode IS a hang, so the test
    /// must be able to fail by not finishing in time rather than by an assertion.
    ///
    /// MUTATION: replace `rt.spawn(async move { … })` with `rt.block_on(handle.open(…))` — the
    /// `timeout` below elapses and this fails. That elapsed timeout is the run-loop deadlock,
    /// reproduced. Observed RED.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn enter_opens_the_inspector_without_blocking_the_input_path() {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let services = Arc::new(FleetKeyServices {
            editor_has_focus: std::sync::atomic::AtomicBool::new(true),
            entered_overlay: Mutex::new(Some(entered_tx)),
            release: Mutex::new(Some(release_rx)),
            ..FleetKeyServices::default()
        });
        let (_dir, ext) = armed(Arc::clone(&services));
        let ext = Arc::new(ext);

        // main -> the first roster row, so `Enter` opens rather than deactivating (pi `:732-734`).
        ext.dispatch_terminal_input("\x1b[B").expect("activated");
        ext.dispatch_terminal_input("\x1b[B").expect("moved");
        assert_ne!(
            ext.fleet_status.lock().expect("lock").selected_key(),
            "main"
        );

        let enter = Arc::clone(&ext);
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::task::spawn_blocking(move || enter.dispatch_terminal_input("\r")),
        )
        .await
        .expect(
            "`on_terminal_input` must return while the overlay is still open — awaiting it inline \
             is the run-loop self-deadlock this spawn exists to prevent",
        )
        .expect("join");
        assert_eq!(
            result.expect("Enter is consumed").consume,
            Some(true),
            "pi returns `{{ consume: true }}` immediately (`fleet-status.ts:750`)"
        );

        // The spawned task really did reach the overlay — a fast return with nothing behind it
        // would pass the timeout above and prove nothing.
        entered_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the detached task must reach `open_overlay`");
        let _ = release_tx.send(());
    }

    /// **T12** — the subscription is declared only when the fleet view is enabled, asserted
    /// through the REAL registration path rather than by inspecting `InitApi`.
    ///
    /// pi's gate is structural: with `fleetView: false` it never constructs a
    /// `SubagentFleetStatus` at all (`extension/index.ts:497-511`), so there is nothing for a
    /// keystroke to reach. Subscribing anyway would put an `.await` and a `String` on every
    /// keypress of a session that has opted the widget out.
    ///
    /// MUTATION: drop the `if self.fleet_view_enabled` guard in `init` — the second assertion
    /// fails. Observed RED.
    #[tokio::test]
    async fn the_seam_is_subscribed_only_when_the_fleet_view_is_enabled() {
        for (fleet_view, expected) in [(true, true), (false, false)] {
            let dir = tempfile::tempdir().expect("tempdir");
            let host = cyrup_ext::ExtensionHost::new(cyrup_ext::HostConfig {
                mode: ExtMode::Tui,
                has_ui: true,
                cwd: dir.path().to_path_buf(),
            });
            let config = SubagentExtensionConfig {
                fleet_view,
                ..SubagentExtensionConfig::default()
            };
            host.load_native(Arc::new(SubagentsExtension::with_config_and_cwd(
                config,
                dir.path().to_path_buf(),
            )))
            .await
            .expect("load");
            assert_eq!(
                host.has_terminal_input_subscribers(),
                expected,
                "fleet_view = {fleet_view}"
            );
        }
    }

    /// With no capability backend bound there is no UI to read the editor off — pi's
    /// `getActiveUiContext()` returning `null` (`fleet-status.ts:698`). The handler must decline
    /// rather than panic or guess.
    ///
    /// MUTATION: replace `self.executor.host_services()?` with an `unwrap` — this panics, and the
    /// dispatcher's `catch_unwind` turns a boot-time keystroke into a warning line. Observed RED.
    #[test]
    fn without_a_capability_backend_every_key_falls_through() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig::default(),
            dir.path().to_path_buf(),
        );
        assert!(ext.dispatch_terminal_input("\x1b[B").is_none());
    }

    /// The trait method really delegates here, so the host's own dispatch reaches this logic.
    /// `dispatch_terminal_input` is what every test above drives; this is the one line that ties
    /// it to `NativeExtension::on_terminal_input`.
    ///
    /// MUTATION: make `on_terminal_input` return `None` unconditionally — the extension is
    /// subscribed, the host folds every keystroke through it, and nothing ever happens. Observed
    /// RED.
    #[test]
    fn the_trait_method_delegates_to_the_dispatcher() {
        let services = Arc::new(FleetKeyServices::focused(""));
        let (_dir, ext) = armed(services);
        let result = NativeExtension::on_terminal_input(&ext, "\x1b[B").expect("consumed");
        assert_eq!(result.consume, Some(true));
        assert!(is_active(&ext.fleet_status));
    }

    /// A release event is ignored outright, before any other guard — pi
    /// `isKeyRelease(data)` (`fleet-status.ts:699`). Without it, a terminal negotiating the kitty
    /// keyboard protocol with event types would move the roster twice per keypress.
    ///
    /// MUTATION: drop `is_release` from `from_terminal_data`'s decoded arm — the roster activates
    /// on the RELEASE of `↓` as well as the press, and this fails. Observed RED.
    #[test]
    fn a_key_release_is_ignored() {
        let services = Arc::new(FleetKeyServices::focused(""));
        let (_dir, ext) = armed(Arc::clone(&services));
        // The kitty event-type-3 form of `↓`, which is what `encode_terminal_key` emits for a
        // release.
        assert!(ext.dispatch_terminal_input("\x1b[1;1:3B").is_none());
        assert!(!is_active(&ext.fleet_status));
        // And the press still works, so the guard is not simply refusing arrows.
        assert!(ext.dispatch_terminal_input("\x1b[B").is_some());
        assert!(is_active(&ext.fleet_status));
    }
}
