//! TUI-121 — the in-session `/resume` picker's session listing, off the run loop.
//!
//! pi's `/resume` hands `SessionSelectorComponent` two loaders taking `(onProgress, signal)`
//! (`interactive-mode.ts:5549-5566` @v0.87.1). The component opens at once, starts the current
//! folder's load from its constructor (`session-selector.ts:869`), shows `Loading …` /
//! `Loading loaded/total` in the header while it runs, drops each partial set into the list as it
//! arrives (`:956-970`), loads the all-projects set only when `Tab` first asks for it
//! (`:1031-1040`), and aborts whatever is still loading on select, cancel and exit (`cancelLoads`,
//! `:872-883`).
//!
//! cyrup's picker used to be built only after `list_sessions()` and `list_all_sessions()` had
//! read every session file of every project in full, on the run loop's own task: the UI froze
//! between `/resume` and the picker, and keystrokes queued behind the scan. Here the picker opens
//! empty in its loading state, each load runs on the blocking pool, and its reports come back over
//! [`App::install_session_list_channel`]'s channel as [`SessionListMsg`]s, which the run loop
//! applies through [`App::apply_session_list_msg`].
//!
//! Two guards stand in for pi's `isActive()` (`:955`): every message carries the epoch of the
//! picker it was started for, and a message whose epoch is not the open picker's — or that arrives
//! with no `/resume` picker open — is dropped. Closing the picker cancels every load it started;
//! the loader stops before its next file.

use std::ops::ControlFlow;

use cyrup_core::CancelToken;
use cyrup_session_svc::{SessionInfo, SessionListing};

use super::*;
use crate::session_selector::{SessionRowSet, SessionScope};

/// One report from a spawned `/resume` load (pi's `onProgress` calls and the loader's resolved
/// value, `session-selector.ts:956-988` @v0.87.1).
#[derive(Debug)]
pub struct SessionListMsg {
    /// The picker the load was started for; see [`AppState::session_list_epoch`].
    pub epoch: u64,
    /// Which of the picker's two loaders produced it.
    pub scope: SessionScope,
    pub update: SessionListUpdate,
}

/// What a [`SessionListMsg`] reports.
#[derive(Debug)]
pub enum SessionListUpdate {
    /// `onProgress(loaded, total, partialSessions?)` — `partial` on pi's periodic publishes only.
    Progress {
        loaded: usize,
        total: usize,
        partial: Option<SessionRowSet>,
    },
    /// The loader resolved with the full set.
    Done(SessionRowSet),
}

/// The loads the open `/resume` picker can run: pi's two loaders (as owned
/// [`SessionListing`]s, so a load can run on the blocking pool), the running session's id (the
/// `(current)` marker each row carries), and one abort handle per load started — pi's
/// `currentLoad` / `allLoad` controllers.
pub(crate) struct SessionListLoads {
    current: SessionListing,
    all: SessionListing,
    current_id: String,
    cancels: Vec<CancelToken>,
}

/// Project one listing result onto the picker's rows and per-row metadata.
pub(crate) fn session_row_set(sessions: &[SessionInfo], current: &str) -> SessionRowSet {
    SessionRowSet {
        rows: super::execute_session::session_rows(sessions, current),
        cwds: sessions
            .iter()
            .map(|s| (s.path.display().to_string(), s.cwd.clone()))
            .collect(),
        parents: sessions
            .iter()
            .filter_map(|s| {
                s.parent_session_path
                    .as_ref()
                    .map(|p| (s.path.display().to_string(), p.display().to_string()))
            })
            .collect(),
        // `currentSessionFilePath` — resolved from the listing rather than the manager so it is
        // the SAME string the rows carry (a canonicalization mismatch would silently never match).
        current_path: sessions
            .iter()
            .find(|s| s.id.to_string() == current)
            .map(|s| s.path.display().to_string()),
    }
}

/// Run one `/resume` load on the blocking pool and report it over `tx` as [`SessionListMsg`]s
/// tagged with `epoch` (pi's `loadScope` awaiting its loader, `session-selector.ts:941-988`
/// @v0.87.1). Shared by the in-session picker ([`App::start_requested_session_load`]) and the
/// pre-launch `--resume` picker (`startup_loop`, SEAM-134), which differ in who owns the channel
/// and the epoch, not in how a listing is run.
///
/// Blocking file reads, so the blocking pool: the caller's task — and the async workers the rest of
/// the process runs on — never waits on a session file. Needs a tokio runtime. `cancel` stops the
/// listing before its next file and suppresses its `Done`.
pub(crate) fn spawn_session_load(
    listing: SessionListing,
    current_id: String,
    scope: SessionScope,
    epoch: u64,
    tx: tokio::sync::mpsc::UnboundedSender<SessionListMsg>,
    cancel: CancelToken,
) {
    tokio::task::spawn_blocking(move || {
        // pi's `signal?.throwIfAborted()` ahead of the first read (`session-manager.ts:946`).
        if cancel.is_cancelled() {
            return;
        }
        let mut on_progress = |loaded: usize, total: usize, partial: Option<&[SessionInfo]>| {
            if cancel.is_cancelled() {
                return ControlFlow::Break(());
            }
            let update = SessionListUpdate::Progress {
                loaded,
                total,
                partial: partial.map(|p| session_row_set(p, &current_id)),
            };
            // A closed channel means the receiver is gone: nobody will read the rest.
            match tx.send(SessionListMsg {
                epoch,
                scope,
                update,
            }) {
                Ok(()) => ControlFlow::Continue(()),
                Err(_) => ControlFlow::Break(()),
            }
        };
        let sessions = listing.run(Some(&mut on_progress));
        if cancel.is_cancelled() {
            return;
        }
        let _ = tx.send(SessionListMsg {
            epoch,
            scope,
            update: SessionListUpdate::Done(session_row_set(&sessions, &current_id)),
        });
    });
}

impl<B: Backend> App<B> {
    /// Install the `/resume` listing channel and hand back its receiver (TUI-121).
    ///
    /// [`App::run`] calls this once at startup, like [`Self::install_model_refresh_channel`].
    /// Without it (an embedder, a widget test) `/resume` lists inline, as it did before TUI-121.
    /// `pub` so a test can drive the streamed path without standing up a run loop.
    pub fn install_session_list_channel(
        &mut self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<SessionListMsg> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<SessionListMsg>();
        self.session_list_tx = Some(tx);
        rx
    }

    /// Whether a run loop is servicing the `/resume` listing channel.
    pub(crate) fn has_session_list_channel(&self) -> bool {
        self.session_list_tx.is_some()
    }

    /// Arm the loads for a `/resume` picker that has just opened: a fresh epoch (so nothing a
    /// previous picker started can land in this one), pi's two loaders, then whatever load the
    /// picker asked for — the current folder's, from its constructor.
    pub(crate) fn begin_session_list(
        &mut self,
        current: SessionListing,
        all: SessionListing,
        current_id: String,
    ) {
        self.cancel_session_list_loads();
        self.state.session_list_epoch = self.state.session_list_epoch.wrapping_add(1);
        self.state.session_list = Some(SessionListLoads {
            current,
            all,
            current_id,
            cancels: Vec::new(),
        });
        self.start_requested_session_load();
    }

    /// The open `/resume` picker, if one occupies the input slot.
    pub(crate) fn session_selector_mut(&mut self) -> Option<&mut SessionSelector> {
        self.state
            .selector
            .as_mut()
            .filter(|s| s.kind == SelectorKind::Session)
            .and_then(|s| s.inner.as_session_selector())
    }

    /// Start the load the open picker asked for, if any — pi's `void this.loadScope(scope)`
    /// reaching its `await` (`session-selector.ts:973-975` @v0.87.1). Called after the picker opens
    /// and after every key routed to it (`Tab` onto a scope never loaded asks for one).
    pub(crate) fn start_requested_session_load(&mut self) {
        let Some(scope) = self
            .session_selector_mut()
            .and_then(SessionSelector::take_load_request)
        else {
            return;
        };
        let Some(tx) = self.session_list_tx.clone() else {
            return;
        };
        let epoch = self.state.session_list_epoch;
        let Some(loads) = self.state.session_list.as_mut() else {
            return;
        };
        let listing = match scope {
            SessionScope::Current => loads.current.clone(),
            SessionScope::All => loads.all.clone(),
        };
        let current_id = loads.current_id.clone();
        let cancel = CancelToken::new();
        loads.cancels.push(cancel.clone());
        spawn_session_load(listing, current_id, scope, epoch, tx, cancel);
    }

    /// pi `cancelLoads`' abort half (`session-selector.ts:872-883` @v0.87.1): fire every load the
    /// picker started. Run when the picker closes, whichever way.
    pub(crate) fn cancel_session_list_loads(&mut self) {
        if let Some(loads) = self.state.session_list.take() {
            for cancel in loads.cancels {
                cancel.cancel();
            }
        }
    }

    /// pi `refreshSessionsAfterMutation` (`session-selector.ts:1024-1029` @v0.87.1), after the
    /// host applied a `/resume` delete or rename: the open picker forgets both cached sets and asks
    /// for its scope again; every load it had started is cancelled, and a fresh epoch keeps
    /// anything they still report out of the reload (pi's `cancelLoads` nulls both controllers,
    /// so its `isActive()` fails for them). A no-op without a streamed `/resume` picker open.
    pub(crate) fn refresh_session_list_after_mutation(&mut self) {
        if !self
            .session_selector_mut()
            .is_some_and(SessionSelector::refresh_after_mutation)
        {
            return;
        }
        let Some(loads) = self.state.session_list.as_mut() else {
            return;
        };
        for cancel in loads.cancels.drain(..) {
            cancel.cancel();
        }
        self.state.session_list_epoch = self.state.session_list_epoch.wrapping_add(1);
        self.start_requested_session_load();
    }

    /// Apply one streamed `/resume` load report to the open picker (TUI-121). Dropped when it
    /// belongs to a picker that has since closed or been reopened, or when no `/resume` picker is
    /// open at all — pi's `if (!isActive()) return` (`session-selector.ts:957`, `:976`).
    pub fn apply_session_list_msg(&mut self, msg: SessionListMsg) {
        if msg.epoch != self.state.session_list_epoch || self.state.session_list.is_none() {
            return;
        }
        let Some(picker) = self.session_selector_mut() else {
            return;
        };
        match msg.update {
            SessionListUpdate::Progress {
                loaded,
                total,
                partial,
            } => picker.apply_load_progress(msg.scope, loaded, total, partial),
            SessionListUpdate::Done(set) => picker.finish_load(msg.scope, set),
        }
    }
}
