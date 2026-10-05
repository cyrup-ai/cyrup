//! The startup selector's run loop, generic over the ratatui [`Backend`] and the key source so a
//! `TestBackend` test can step it (SEAM-134).
//!
//! [`crate::startup_selector::run_startup_selector`] used to be one `loop { draw; input.read()? }`
//! whose read was a blocking 3600 s pump, so nothing could reach the selector except a key. pi's
//! startup `--resume` picker is not like that: `SessionSelectorComponent` starts the current-folder
//! load from its constructor (`session-selector.ts:869` @v0.87.1), renders `Loading …` at once and
//! takes each partial set as it lands (`:956-988`), while the TUI keeps running (`cli/session-picker
//! .ts:15-55` hands it two loaders, `main.ts:372-373`). So this loop does three things per turn:
//! serve the load the picker asked for, drain what the loads have reported, and wait a short while
//! for a key.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use cyrup_core::CancelToken;
use cyrup_session_svc::SessionListing;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::crossterm::event::{Event, KeyEventKind};
use ratatui::layout::Rect;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::app::{SessionListMsg, SessionListUpdate, spawn_session_load};
use crate::error::TuiError;
use crate::keymap::SelectKeymap;
use crate::selector::{Selector, SelectorOutcome};
use crate::session_selector::{SessionScope, SessionSelector};
use crate::startup_theme::StartupTheme;
use crate::terminal_query::{COLOR_QUERY_TIMEOUT, TerminalColors};
use crate::theme::UiTheme;

/// How long one turn waits for a key while a listing is still running. The listing reports over a
/// channel the key source cannot wait on, so this bounds how stale the screen can get.
pub(crate) const LOADING_POLL: Duration = Duration::from_millis(25);
/// How long one turn waits for a key when nothing can change the screen but one.
pub(crate) const IDLE_POLL: Duration = Duration::from_secs(3600);

/// Where the loop's keys come from.
pub(crate) trait StartupEvents {
    /// The next terminal event, or `None` if `wait` elapsed first.
    fn next(&mut self, wait: Duration) -> Result<Option<Event>, TuiError>;
}

/// The two loaders a streamed startup `--resume` picker is handed (pi's `currentSessionsLoader` and
/// `allSessionsLoader`, `cli/session-picker.ts:15-19`), as owned listings the blocking pool can run.
#[derive(Clone, Debug)]
pub struct StartupSessionLoads {
    pub current: SessionListing,
    pub all: SessionListing,
}

/// Runs the picker's loads and folds their reports back in — the startup twin of the `/resume`
/// plumbing in `app/session_list.rs`, sharing its [`spawn_session_load`].
pub(crate) struct LoadDriver {
    loads: StartupSessionLoads,
    tx: UnboundedSender<SessionListMsg>,
    rx: UnboundedReceiver<SessionListMsg>,
    /// Bumped whenever the loads in flight are abandoned (a reload after a mutation), so a report
    /// from the old ones is dropped — pi's `isActive()` (`session-selector.ts:955`).
    epoch: u64,
    cancels: Vec<CancelToken>,
    /// Every session's stored cwd any report carried. The caller resolves the picked session's
    /// missing-cwd issue from it (`main.ts:321-332`), which it can no longer read off a finished scan.
    cwds: HashMap<String, String>,
}

impl LoadDriver {
    pub(crate) fn new(loads: StartupSessionLoads) -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            loads,
            tx,
            rx,
            epoch: 0,
            cancels: Vec::new(),
            cwds: HashMap::new(),
        }
    }

    /// Start the load the picker asked for, if any (`take_load_request`): the current folder's from
    /// its constructor, the all-projects set on the first `Tab`, either again after a mutation.
    pub(crate) fn serve(&mut self, picker: &mut SessionSelector) {
        let Some(scope) = picker.take_load_request() else {
            return;
        };
        let listing = match scope {
            SessionScope::Current => self.loads.current.clone(),
            SessionScope::All => self.loads.all.clone(),
        };
        let cancel = CancelToken::new();
        self.cancels.push(cancel.clone());
        // No session is running pre-launch, so no row is `(current)`.
        spawn_session_load(
            listing,
            String::new(),
            scope,
            self.epoch,
            self.tx.clone(),
            cancel,
        );
    }

    /// Apply every report that has arrived. Whether any did.
    pub(crate) fn drain(&mut self, picker: &mut SessionSelector) -> bool {
        let mut any = false;
        while let Ok(msg) = self.rx.try_recv() {
            if msg.epoch != self.epoch {
                continue;
            }
            any = true;
            match msg.update {
                SessionListUpdate::Progress {
                    loaded,
                    total,
                    partial,
                } => {
                    if let Some(set) = &partial {
                        self.cwds.extend(set.cwds.iter().cloned());
                    }
                    picker.apply_load_progress(msg.scope, loaded, total, partial);
                }
                SessionListUpdate::Done(set) => {
                    self.cwds.extend(set.cwds.iter().cloned());
                    picker.finish_load(msg.scope, set);
                }
            }
        }
        any
    }

    /// pi `refreshSessionsAfterMutation` (`session-selector.ts:1024-1029`), after the host applied a
    /// delete or rename: abandon the loads in flight, and start the one the picker now asks for.
    pub(crate) fn refresh_after_mutation(&mut self, picker: &mut SessionSelector) {
        if !picker.refresh_after_mutation() {
            return;
        }
        self.abandon();
        self.serve(picker);
    }

    /// pi `cancelLoads` (`session-selector.ts:872-883`), on select, cancel and exit.
    pub(crate) fn cancel_all(&mut self, picker: &mut SessionSelector) {
        picker.cancel_loads();
        self.abandon();
    }

    fn abandon(&mut self) {
        for cancel in self.cancels.drain(..) {
            cancel.cancel();
        }
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// The cwds collected so far, leaving the driver empty.
    pub(crate) fn take_cwds(&mut self) -> HashMap<String, String> {
        std::mem::take(&mut self.cwds)
    }
}

impl Drop for LoadDriver {
    /// A future dropped mid-selector must not leave a listing reading files nobody will look at.
    fn drop(&mut self) {
        self.abandon();
    }
}

/// The live theme of a startup selector: pi's `queryStartupTerminalColors` and `onThemePreview`
/// (`cli/startup-ui.ts:117-127`, `:204-209` @v1.0.0) as a loop component.
///
/// The selector is on screen before the terminal has said what its colours are, in the grayscale the
/// pending system theme paints. The colours arrive on [`Self::colors`] — from the input reader, which
/// routes the terminal's reply to the query the run asked — and each batch re-applies the theme. A
/// terminal that answers nothing is given the query's own timeout ([`COLOR_QUERY_TIMEOUT`], pi's
/// `requestTerminalColors` `.then(apply, () => apply({}))`): then the grayscale ends with nothing
/// reported, and a reply that comes later still applies.
pub(crate) struct Retheme {
    theme: StartupTheme,
    colors: UnboundedReceiver<TerminalColors>,
    /// When the wait for the first answer ends; `None` once any colours have been applied.
    deadline: Option<Instant>,
}

impl Retheme {
    /// Start waiting for the terminal's colours at `now`.
    pub(crate) fn new(
        theme: StartupTheme,
        colors: UnboundedReceiver<TerminalColors>,
        now: Instant,
    ) -> Self {
        Self {
            theme,
            colors,
            deadline: now.checked_add(COLOR_QUERY_TIMEOUT),
        }
    }

    /// How long until the first answer is given up on, if it is still awaited.
    fn until_deadline(&self, now: Instant) -> Option<Duration> {
        self.deadline.map(|due| due.saturating_duration_since(now))
    }

    /// Apply whatever the terminal has said by `now`. `Some` with the theme to paint when it moved.
    fn settle(&mut self, now: Instant) -> Option<UiTheme> {
        let mut changed = false;
        while let Ok(colors) = self.colors.try_recv() {
            self.deadline = None;
            changed |= self.theme.apply_colors(colors);
        }
        if self.deadline.is_some_and(|due| due <= now) {
            self.deadline = None;
            changed |= self.theme.apply_colors(TerminalColors::default());
        }
        changed.then(|| self.theme.theme())
    }

    /// `onThemePreview`: the highlight names a theme.
    fn preview(&mut self, name: &str) -> UiTheme {
        self.theme.preview(name);
        self.theme.theme()
    }
}

/// One selector, one terminal, one key source: [`Self::step`] is a turn of the loop.
pub(crate) struct StartupLoop<'a, B: Backend, E: StartupEvents> {
    pub(crate) terminal: &'a mut Terminal<B>,
    pub(crate) events: &'a mut E,
    /// The theme painted now. Replaced when [`Self::retheme`] says the terminal's colours changed
    /// it.
    pub(crate) theme: UiTheme,
    /// Where the theme comes from, if it can change under the selector.
    pub(crate) retheme: Option<Retheme>,
    pub(crate) keymap: &'a SelectKeymap,
    pub(crate) inner: &'a mut dyn Selector,
    pub(crate) loads: Option<LoadDriver>,
}

impl<B: Backend, E: StartupEvents> StartupLoop<'_, B, E> {
    /// The picker behind `inner`, when it is the session picker and loads are wired.
    fn picker_and_loads(&mut self) -> Option<(&mut SessionSelector, &mut LoadDriver)> {
        let loads = self.loads.as_mut()?;
        Some((self.inner.as_session_selector()?, loads))
    }

    pub(crate) fn draw(&mut self) -> Result<(), TuiError> {
        let Self {
            terminal,
            theme,
            inner,
            ..
        } = self;
        let theme = &*theme;
        terminal
            .draw(|frame| {
                let area = frame.area();
                // Pi passes `ui.terminal.rows` into `ConfigSelectorComponent`
                // (`cli/config-selector.ts:47`), which turns it into the body window
                // (`config-selector.ts:266`). Doing it here rather than at construction keeps the
                // window correct across a resize; it is a no-op for every other selector.
                inner.set_terminal_height(area.height);
                let height = inner.desired_height(area.width).min(area.height).max(1);
                let slot = Rect {
                    x: area.x,
                    y: area.y,
                    width: area.width,
                    height,
                };
                inner.render(frame, slot, theme);
            })
            .map_err(|e| TuiError::Backend(e.to_string()))?;
        Ok(())
    }

    /// One turn: serve the load the picker asked for, apply what the loads reported, paint, then wait
    /// for a key (briefly while a listing is running). `Some` once the selector confirms or cancels.
    pub(crate) async fn step(
        &mut self,
        on_apply: &mut impl AsyncFnMut(&str),
    ) -> Result<Option<SelectorOutcome>, TuiError> {
        if let Some(theme) = self.retheme.as_mut().and_then(|r| r.settle(Instant::now())) {
            self.theme = theme;
        }
        let mut loading = false;
        if let Some((picker, loads)) = self.picker_and_loads() {
            loads.serve(picker);
            loads.drain(picker);
            loading =
                picker.is_loading(SessionScope::Current) || picker.is_loading(SessionScope::All);
        }
        self.draw()?;
        let mut wait = if loading { LOADING_POLL } else { IDLE_POLL };
        // The first colour answer is not waited on past its timeout (`requestTerminalColors`).
        if let Some(due) = self
            .retheme
            .as_ref()
            .and_then(|r| r.until_deadline(Instant::now()))
        {
            wait = wait.min(due);
        }
        // Ignore key-release events (Kitty protocol) so a single press is not double-counted.
        let Some(Event::Key(key)) = self.events.next(wait)? else {
            return Ok(None);
        };
        if key.kind == KeyEventKind::Release {
            return Ok(None);
        }
        let outcome = self.inner.handle(&key, self.keymap);
        match outcome {
            SelectorOutcome::Confirm(_) | SelectorOutcome::Cancel => {
                if let Some((picker, loads)) = self.picker_and_loads() {
                    loads.cancel_all(picker);
                }
                Ok(Some(outcome))
            }
            SelectorOutcome::Apply(payload) => {
                on_apply(&payload).await;
                if let Some((picker, loads)) = self.picker_and_loads() {
                    loads.refresh_after_mutation(picker);
                }
                Ok(None)
            }
            // Never produced by the startup selectors (`OpenExternalEditor` is only
            // `ExtensionEditorSelector`'s; `OpenSubmenu` is only the `/settings` grid's;
            // `ConfirmDefault` is only the model/thinking pickers', neither of which runs
            // pre-launch — and there is no session here to persist through anyway) —
            // treated as a no-op like `Redraw`'s siblings.
            // The first-run wizard's theme step (`onThemePreview`, `startup-ui.ts:204-209`): the
            // dialog repaints in the theme under the highlight.
            SelectorOutcome::Preview(name) => {
                if let Some(retheme) = self.retheme.as_mut() {
                    self.theme = retheme.preview(&name);
                }
                Ok(None)
            }
            SelectorOutcome::Redraw
            | SelectorOutcome::Ignored
            | SelectorOutcome::OpenExternalEditor
            | SelectorOutcome::ConfirmDefault(_)
            | SelectorOutcome::OpenSubmenu(_) => Ok(None),
        }
    }

    pub(crate) async fn run(
        mut self,
        mut on_apply: impl AsyncFnMut(&str),
    ) -> Result<(SelectorOutcome, HashMap<String, String>), TuiError> {
        loop {
            if let Some(outcome) = self.step(&mut on_apply).await? {
                let cwds = self
                    .loads
                    .as_mut()
                    .map(LoadDriver::take_cwds)
                    .unwrap_or_default();
                return Ok((outcome, cwds));
            }
        }
    }
}
