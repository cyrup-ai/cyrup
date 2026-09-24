//! The live bridge — the four-state gate, the process-wide handle, and the entry points
//! `extension/host/native_impl.rs` calls.
//!
//! [`state`](super::state) and [`label`](super::label) are pure. [`reporter`](super::reporter),
//! [`consumer`](super::consumer) and [`reporter`](super::reporter) each own one live concern. This
//! file is what joins them to cyrup's own lifecycle, and it is where the decision *whether any of
//! it runs at all* is made.
//!
//! # Four runtime states, and they are not the same
//!
//! | state | detection | behaviour |
//! |---|---|---|
//! | **not in a herdr pane** | [`cyrup_herdr::HerdrPane::discover`] → `None` | **total inertness**: no client, no task, no socket, no observer, no signal handler, nothing allocated beyond the `None` |
//! | **in a pane, headless** (`cyrup -p`) | pane present, `has_ui == false` | detected and then **dropped**: [`HerdrBridge::start`] returns before anything is constructed |
//! | **in a pane, herdr's server gone** | every connect answers `ECONNREFUSED`/`ENOENT` | best effort: `debug` log, keep the desired state, retry on the next edge and on the refresh tick. Never surfaced, never fails a turn |
//! | **the `herdr` binary absent** | only on the CLI fallback, which this bridge does not use | not this file's case — see `crates/cyrup-intercom/src/project_pane.rs` |
//!
//! The gate's first row is the whole of the cost argument: a cyrup outside herdr — which is most
//! of them — must be **byte for byte** what it was before this feature existed. Both conditions
//! are load-bearing, and the conjunction is not a belt-and-braces double check:
//! `PaneLaunchIdentity::OmitPane` (`tmp/herdr/src/pane.rs:170-172`) **removes** `HERDR_PANE_ID`
//! while leaving `HERDR_ENV=1` and `HERDR_SOCKET_PATH` in place, so a process really can sit
//! inside herdr, see a live socket, and own no pane. [`cyrup_herdr::HerdrPane::discover`] holds
//! that conjunction (`crates/cyrup-herdr/src/env.rs:133-147`); nothing here re-derives it.
//!
//! The headless row is pi's own rule, ported (`herdr-status.ts:376-380`,
//! `if (hasUI !== true) return`). A headless cyrup is very often a *child* of an interactive one
//! sharing the same pane — `HERDR_PANE_ID` is inherited by every descendant of the pane's process
//! — and two cyrups reporting different states onto one pane would make the sidebar flicker
//! between them. The interactive parent is the one that owns the pane's lifecycle.
//!
//! # One bridge per process, and why it is a `static` rather than a field
//!
//! The pane identity comes from the process environment, and a signal handler is installed against
//! the process, not against a session. One process runs inside exactly one herdr pane. A
//! per-extension field would model a relationship that does not exist, and two extension instances
//! in one process would then fight over one pane's authority — herdr's 32-distinct-sources-per-pane
//! cap (`socket-api.mdx:798`) makes that failure permanent rather than transient.
//!
//! [`arm`] and [`shutdown`] are the only two functions that touch the slot, they are the
//! production entry points, and both are idempotent.
//!
//! # The human-wait input is the SESSION lock, not a per-native gate
//!
//! [`HumanInteractionLock`] is the one slot every companion that opens a human prompt acquires
//! before opening it, and it is reached off the single [`cyrup_ext::host::HostServices`] backend
//! Arc that `load_native_with_services` clones into every native
//! (`crates/cyrup-session-svc/src/builder.rs:1221-1224`,
//! `crates/cyrup-session-svc/src/host_services.rs:1547`). So an observer that reads THIS sees the
//! permission dialog (`cyrup-permission-system/src/extension/prompt.rs:176`), the ask-forwarder
//! (`cyrup-permission-system/src/forwarding.rs:1231`), MCP's dialog owner
//! (`cyrup-mcp/src/owner.rs:659`), intercom's clarify (`cyrup-intercom/src/seams.rs:369`) and
//! flux's ask tool (`cyrup-flux/src/ask_tool.rs:185`) — every one of them, whichever extension
//! raised it.
//!
//! `cyrup_ext::native::HumanWaitGate` is NOT that seam and this bridge does not read it. Its
//! struct holds one `AtomicUsize`, but `HostCtx::event` allocates a NEW `Arc<HumanWaitGate>` per
//! call (`crates/cyrup-ext/src/native.rs:170-180`) and `ExtensionHost` builds one ctx per native
//! (`crates/cyrup-ext/src/facade.rs:541`), so there are as many gates as there are loaded natives
//! and the subagents extension's own is raised by nothing — `grep -rn begin_human_wait
//! crates/cyrup-ext-subagents/src` finds only this file's tests. Watching it would leave the pane
//! at `working` for the entire life of every permission dialog, which is precisely the state this
//! feature exists to distinguish.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use cyrup_ext::host::HumanInteractionLock;
use cyrup_herdr::{EnvSource, HerdrClient, HerdrPane, ProcessEnv};
use tokio::task::JoinHandle;

use super::consumer::{self, ConsumerDeps};
use super::label::{MetadataText, RunLabel, metadata_text};
use super::reporter::{Metadata, Reporter};
use super::state::{RunAttention, StateModel};
use crate::background::{ActivityState, RunId};
use crate::tui::fleet_state::{AsyncRunView, FleetState};

/// How often the human-wait watcher samples [`HumanInteractionLock::is_held`].
///
/// 100 ms because that is herdr's **own** delivery floor: its API server polls subscriptions every
/// `CONNECTION_POLL_INTERVAL = 100 ms` (`tmp/herdr/src/api/server.rs:28`, loop at `:762-778`), so
/// a faster sampler would buy latency the sidebar cannot show. The sample itself is one
/// `Semaphore::available_permits` — [`HumanInteractionLock::is_held`] is exactly that
/// (`crates/cyrup-ext/src/host/services.rs`) — and the watcher exists **only while the bridge is
/// armed**, i.e. only inside a herdr pane with a UI and with a capability backend bound.
///
/// A poll rather than a callback because [`HumanInteractionLock`] has no observer seam today; see
/// this module's [`HerdrBridge::set_human_waiting`], which is the edge sink one would feed.
pub const HUMAN_WAIT_POLL: Duration = Duration::from_millis(100);

/// The one bridge for this process, or `None` when nothing is armed.
///
/// `RwLock` rather than `OnceLock` because a session can end and another begin inside one process
/// (`SessionShutdown` then `SessionStart`), and the second must get a live bridge rather than the
/// released one.
static BRIDGE: RwLock<Option<Arc<HerdrBridge>>> = RwLock::new(None);

/// The armed bridge, if there is one. Every edge in `native_impl.rs` goes through this, so a
/// session outside herdr pays one uncontended read lock and an `Option` clone per edge.
#[must_use]
pub fn bridge() -> Option<Arc<HerdrBridge>> {
    BRIDGE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Arm the bridge for this session — `HostEvent::SessionStart`.
///
/// Idempotent: a second `SessionStart` inside one process (a session switch) keeps the bridge that
/// is already reporting, rather than opening a second authority on the same pane.
///
/// Returns `None` in the gate's first two rows (not in a pane; in a pane but headless), and that
/// `None` is the whole of the inertness guarantee — nothing is constructed before it is answered.
///
/// `human_lock` is `HostServices::human_interaction_lock()` off the extension's late-bound
/// capability backend. `None` (a by-value session with no backend bound) arms everything except
/// the human-wait watcher: the pane still tracks turns, runs and attention, it simply never says
/// `blocked` for a dialog, because in that configuration there is no dialog to say it for.
pub fn arm(
    has_ui: bool,
    human_lock: Option<Arc<HumanInteractionLock>>,
) -> Option<Arc<HerdrBridge>> {
    arm_in(&ProcessEnv, has_ui, human_lock)
}

/// [`arm`] against an explicit environment.
///
/// The environment is a parameter for the reason [`cyrup_herdr::EnvSource`] exists at all and
/// states on itself (`crates/cyrup-herdr/src/env.rs:22-33`): `cargo test` runs a binary's tests as
/// parallel threads in one process, so a test that mutates the process environment races every
/// other test in the binary. Production passes [`ProcessEnv`], which is `std::env::var`. This is
/// not a test-only door — it is the same function, and the gate it applies is the same one.
pub fn arm_in(
    env: &impl EnvSource,
    has_ui: bool,
    human_lock: Option<Arc<HumanInteractionLock>>,
) -> Option<Arc<HerdrBridge>> {
    if let Some(existing) = bridge() {
        return Some(existing);
    }
    let bridge = HerdrBridge::start(env, has_ui, human_lock)?;
    let mut slot = BRIDGE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Another thread may have armed between the read above and this write. Its bridge is as good
    // as this one and is already in the slot, so this one is dropped — which stops its tasks.
    if let Some(existing) = slot.as_ref() {
        return Some(Arc::clone(existing));
    }
    *slot = Some(Arc::clone(&bridge));
    Some(bridge)
}

/// Release the pane and disarm — `HostEvent::SessionShutdown`.
///
/// Awaited and bounded ([`super::reporter::RELEASE_TIMEOUT`]). Idempotent, and a no-op when
/// nothing was armed, which is every cyrup outside a herdr pane.
pub async fn shutdown() {
    let taken = BRIDGE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(bridge) = taken {
        bridge.release().await;
    }
}

/// The live bridge: a pane, a model, a reporter, and the tasks watching for the two things that
/// arrive from outside cyrup (a human dialog, and herdr itself).
pub struct HerdrBridge {
    pane: HerdrPane,
    model: Arc<Mutex<StateModel>>,
    reporter: Arc<Reporter>,
    /// Latched by [`consumer`] when herdr says this pane is gone. Every producer below checks it,
    /// so a closed pane stops the bridge rather than turning every edge into a failed connect.
    pane_gone: Arc<AtomicBool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl std::fmt::Debug for HerdrBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HerdrBridge")
            .field("pane_id", &self.pane.pane_id())
            .field("pane_gone", &self.pane_gone.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl HerdrBridge {
    /// The gate, and — only if it passes — everything the bridge owns.
    ///
    /// Nothing is constructed before both rows of the gate are answered. That ordering is the
    /// inertness guarantee, and it is why this is one function rather than a constructor plus an
    /// `enable` call.
    #[must_use]
    pub fn start(
        env: &impl EnvSource,
        has_ui: bool,
        human_lock: Option<Arc<HumanInteractionLock>>,
    ) -> Option<Arc<Self>> {
        // Row 1 — not in a herdr pane. Total inertness.
        let pane = HerdrPane::discover(env)?;
        // Row 2 — in a pane, headless. Detected, never armed: the interactive parent sharing this
        // pane is its lifecycle authority (pi `herdr-status.ts:376-380`).
        if !has_ui {
            tracing::debug!(
                pane_id = %pane.pane_id(),
                "herdr: in a pane but headless; the interactive parent owns this pane"
            );
            return None;
        }

        let client = HerdrClient::for_pane(&pane);
        let model = Arc::new(Mutex::new(StateModel::new()));
        let reporter = Arc::new(Reporter::spawn(
            client.clone(),
            pane.pane_id().to_string(),
            Arc::clone(&model),
            // `[CYRUP-EXCEEDS-UPSTREAM]`, opt-in: the same `EnvSource` the gate above read, never
            // the ambient process environment. See [`super::view`].
            super::view::enabled(env),
        ));
        let pane_gone = Arc::new(AtomicBool::new(false));
        let bridge = Arc::new(Self {
            pane,
            model: Arc::clone(&model),
            reporter: Arc::clone(&reporter),
            pane_gone: Arc::clone(&pane_gone),
            tasks: Mutex::new(Vec::new()),
        });

        // At most TWO tasks, and deliberately not a third. See this module's doc and [`super`]'s:
        // the SIGTERM/SIGHUP guard the AUG called for would be cyrup's SECOND, and on the
        // interactive host — the only host this arms on — the first one already ends in
        // `session_shutdown{quit}`, which is what calls [`shutdown`] below.
        //
        // The second task exists only when a capability backend handed over the session's
        // [`HumanInteractionLock`]. No backend, no dialog to watch for, and no task.
        {
            let mut tasks = bridge
                .tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            tasks.push(tokio::spawn(consumer::consume(ConsumerDeps {
                client,
                pane_id: bridge.pane.pane_id().to_string(),
                model,
                reporter: Arc::clone(&reporter),
                pane_gone,
            })));
            if let Some(lock) = human_lock {
                tasks.push(tokio::spawn(watch_human_wait(
                    lock,
                    Arc::downgrade(&bridge),
                )));
            } else {
                tracing::debug!(
                    pane_id = %bridge.pane.pane_id(),
                    "herdr: no capability backend bound; the pane will not report `blocked` for a dialog"
                );
            }
        }

        tracing::debug!(pane_id = %bridge.pane.pane_id(), "herdr: the status bridge is armed");
        Some(bridge)
    }

    /// This process's pane, e.g. `w1:p1`.
    #[must_use]
    pub fn pane_id(&self) -> &str {
        self.pane.pane_id()
    }

    /// Whether herdr has told this bridge its pane no longer exists.
    #[must_use]
    pub fn is_pane_gone(&self) -> bool {
        self.pane_gone.load(Ordering::Acquire)
    }

    /// The root turn begins — `HostEvent::AgentStart`.
    pub fn agent_start(&self) {
        self.edge(StateModel::agent_start);
    }

    /// The root turn ends — `HostEvent::AgentEnd`.
    pub fn agent_end(&self) {
        self.edge(StateModel::agent_end);
    }

    /// A foreground run entered the foreground-control registry.
    pub fn foreground_run_started(&self) {
        self.edge(StateModel::foreground_run_started);
    }

    /// A foreground run left it.
    pub fn foreground_run_finished(&self) {
        self.edge(StateModel::foreground_run_finished);
    }

    /// A human dialog opened or closed — the held↔unheld edge of the session's
    /// [`HumanInteractionLock`].
    ///
    /// This is the seam the whole feature is named for. Every companion that opens a human prompt
    /// acquires that one slot first — the permission dialog
    /// (`crates/cyrup-permission-system/src/extension/prompt.rs:176`), the ask-forwarder
    /// (`crates/cyrup-permission-system/src/forwarding.rs:1231`), MCP's dialog owner
    /// (`crates/cyrup-mcp/src/owner.rs:659`), intercom's clarify
    /// (`crates/cyrup-intercom/src/seams.rs:369`) and flux's ask tool
    /// (`crates/cyrup-flux/src/ask_tool.rs:185`) — so a `true` here is a *fact* that cyrup is
    /// parked on the human, not a heuristic, and it is a fact whichever extension parked it.
    ///
    /// Fed today by [`watch_human_wait`]; it is a `pub` edge sink rather than a private one so
    /// that an observer on the lock, if one is ever added in `cyrup-ext`, replaces the sampler
    /// without touching anything else.
    pub fn set_human_waiting(&self, waiting: bool) {
        self.edge(|model| model.set_human_waiting(waiting));
    }

    /// A run is asking for the human — the foreground control-notice edge.
    ///
    /// `message` must be a control-notice **reason token**, never a rendered notice body:
    /// `format_control_notice_message` (`extension/executor/notices.rs:322-325`) interpolates the
    /// child's own output, and that text may not reach a shared sidebar. See [`super::label`].
    pub fn raise_attention(&self, run_id: &RunId, message: Option<&str>) {
        self.edge(|model| model.raise_attention(run_id, message));
    }

    /// A run stopped asking, or ended.
    pub fn clear_attention(&self, run_id: &RunId) {
        self.edge(|model| model.clear_attention(run_id));
    }

    /// Re-derive everything that is a *level* rather than an edge, from the fleet projection, and
    /// republish the pane's label — `HostEvent::TurnEnd`, `SessionStart`, and every run-ended bus
    /// event.
    ///
    /// Three things come from here and from nowhere else:
    ///
    /// 1. **Detached background runs**, as a level. A runner started by a *previous* cyrup process
    ///    is still going and will never deliver a "started" edge to this one, so an edge count
    ///    could not see it. [`AsyncRunView::is_active`] is the same `Queued | Running` test the
    ///    fleet UI uses.
    /// 2. **Background attention**, from `activity_state == NeedsAttention` on the run's telemetry
    ///    (`crates/cyrup-ext-subagents/src/background/telemetry.rs:29-31`).
    /// 3. **The label**, rebuilt from [`RunLabel::from_async_run`], which reaches
    ///    `workflow_task_label` and nothing else. [`RunLabel::new`] — the other constructor —
    ///    does take an `Option<&str>` launch label, and it is `pub`; what makes the privacy rule
    ///    structural is not the absence of that parameter but that BOTH constructors route every
    ///    string they emit through `workflow_task_label` (`label.rs:169-172`). This producer
    ///    passes no free text to either.
    ///
    /// `project_panes` is the count of project panes this session has opened; `0` renders no pane
    /// clause at all, exactly as pi does when its callback is absent (`herdr-status.ts:162-163`).
    pub fn sync_fleet(&self, fleet: &FleetState, project_panes: usize) {
        if self.stopped() {
            return;
        }
        let active = active_background_runs(fleet);
        let attention: Vec<RunAttention> = active
            .iter()
            .map(|run| RunAttention {
                run_id: run.status.run_id.clone(),
                needs_attention: run.status.telemetry.activity_state
                    == Some(ActivityState::NeedsAttention),
                message: None,
            })
            .collect();
        let reports = {
            let mut model = self
                .model
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Both inputs are levels, so both are applied before anything is sent: applying them
            // one at a time could emit `working` and then `blocked` for one resync.
            let first = model.set_active_background_runs(active.len());
            let second = model.resync_attention(&attention);
            second.or(first)
        };
        if let Some(report) = reports {
            self.reporter.report(report);
        }
        self.reporter.metadata(
            published_text(&active, project_panes).map_or(Metadata::Clear, Metadata::Set),
        );
    }

    /// Clear the pane's label and release cyrup's authority, then stop every task.
    ///
    /// Bounded and idempotent. See [`Reporter::release`] for why the release is correctness rather
    /// than hygiene, and [`super`]'s module doc for why a `kill` reaches this through cyrup's own
    /// signal handler rather than through one installed here.
    pub async fn release(&self) {
        self.reporter.release().await;
        let tasks = std::mem::take(
            &mut *self
                .tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for task in tasks {
            task.abort();
        }
    }

    /// Whether producing anything further is pointless.
    fn stopped(&self) -> bool {
        self.pane_gone.load(Ordering::Acquire) || self.reporter.is_released()
    }

    /// Apply one edge to the model and put its report on the wire if it changed anything.
    ///
    /// The lock is never held across an `await`: [`Reporter::report`] is a `watch::Sender::send`.
    fn edge(&self, apply: impl FnOnce(&mut StateModel) -> Option<super::state::StateReport>) {
        if self.stopped() {
            return;
        }
        let report = {
            let mut model = self
                .model
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            apply(&mut model)
        };
        if let Some(report) = report {
            self.reporter.report(report);
        }
    }
}

/// Turn the active runs into the text that goes on the pane — **the only function in this crate
/// that does**, and it has exactly one caller: [`HerdrBridge::sync_fleet`], which is what
/// `native_impl.rs` calls.
///
/// One producer is the privacy rule's structural half. There used to be two — `sync_fleet` and a
/// `metadata_for` sibling that re-derived the same text for the tests — and a mutation applied to
/// the production one (passing `AsyncRunView::description`, pi's free-text launch task, as a
/// launch label) left the whole suite green because the test asserted on the sibling. A second
/// renderer is a second place for the rule to be broken, so there is one.
fn published_text(active: &[&AsyncRunView], project_panes: usize) -> Option<MetadataText> {
    let attention = active
        .iter()
        .filter(|run| run.status.telemetry.activity_state == Some(ActivityState::NeedsAttention))
        .count();
    let labels: Vec<RunLabel> = active
        .iter()
        .map(|run| RunLabel::from_async_run(run))
        .collect();
    metadata_text(&labels, project_panes, attention)
}

/// Every active detached background run in `fleet`, de-duplicated by run id.
///
/// `tracked_jobs` (in memory) and `history_jobs` (on disk, not tracked by this process) overlap —
/// pi filters the second against the first before concatenating (`fleet.ts:193,199`) — and a run
/// counted twice would keep the pane at `working` after it ended.
fn active_background_runs(fleet: &FleetState) -> Vec<&AsyncRunView> {
    let mut seen: Vec<&RunId> = Vec::new();
    let mut active: Vec<&AsyncRunView> = Vec::new();
    for run in fleet.tracked_jobs.iter().chain(fleet.history_jobs.iter()) {
        if !run.is_active() {
            continue;
        }
        if seen.iter().any(|id| **id == run.status.run_id) {
            continue;
        }
        seen.push(&run.status.run_id);
        active.push(run);
    }
    active
}

/// Sample the session's human-interaction lock and feed its held↔unheld edges to the bridge.
///
/// Holds a [`std::sync::Weak`] so that a released bridge is actually dropped: the task is aborted
/// by [`HerdrBridge::release`], but an abort is not instantaneous and a strong reference would
/// keep the whole bridge — reporter, model and all — alive until the next scheduler tick.
///
/// [`HumanInteractionLock::is_held`] rather than acquiring: an observer that took the slot would
/// itself be a companion, and would block the very dialog it is reporting.
///
/// [`StateModel::set_human_waiting`] de-duplicates, so sampling a level rather than observing an
/// edge cannot produce a duplicate report: the second `true` in a row returns `None` and nothing
/// reaches the wire.
async fn watch_human_wait(lock: Arc<HumanInteractionLock>, bridge: std::sync::Weak<HerdrBridge>) {
    let mut ticker = tokio::time::interval(HUMAN_WAIT_POLL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last = false;
    loop {
        ticker.tick().await;
        let Some(bridge) = bridge.upgrade() else {
            return;
        };
        if bridge.stopped() {
            return;
        }
        let waiting = lock.is_held();
        if waiting != last {
            last = waiting;
            bridge.set_human_waiting(waiting);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::collections::BTreeMap;

    use cyrup_ext::host::HostServices;

    use super::*;

    /// The agent every fixture run invokes. It is what a production workflow node's label
    /// degrades to, so it is what the published summary must name.
    const AGENT_NAME: &str = "reviewer";

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// A capability backend that owns ONE [`HumanInteractionLock`] and hands out that same
    /// instance on every call, which is exactly what the live backend does
    /// (`crates/cyrup-session-svc/src/host_services.rs:1547`, `Arc::clone(&self.human_interaction)`).
    ///
    /// Everything else is the trait's deny-by-default. The identity is the whole point: a test
    /// that mints a fresh lock for the arm and another for the raise proves nothing, because that
    /// is the shape of the bug this seam replaced — `HostCtx::event` mints a fresh
    /// `Arc<HumanWaitGate>` per native (`crates/cyrup-ext/src/native.rs:170-180`), so the
    /// subagents extension's gate and the permission extension's gate were never the same object.
    #[derive(Debug, Default)]
    struct OneLockHost {
        human: Arc<HumanInteractionLock>,
    }

    impl HostServices for OneLockHost {
        fn human_interaction_lock(&self) -> Option<Arc<HumanInteractionLock>> {
            Some(Arc::clone(&self.human))
        }
    }

    /// The bridge arms with no capability backend bound, which is the `arm(has_ui, None)` row:
    /// everything except the human-wait watcher.
    fn no_lock() -> Option<Arc<HumanInteractionLock>> {
        None
    }

    /// §12 row 1 — the whole inertness guarantee. `HERDR_ENV` unset means nothing is constructed,
    /// however live the socket path looks.
    #[tokio::test]
    async fn no_herdr_env_means_no_bridge() {
        let absent = env(&[
            ("HERDR_PANE_ID", "w1:p1"),
            ("HERDR_SOCKET_PATH", "/definitely/not/a/socket"),
        ]);
        assert!(HerdrBridge::start(&absent, true, no_lock()).is_none());
    }

    /// §12 row 1, the shape a reviewer would not think to write — and one herdr really produces.
    /// `PaneLaunchIdentity::OmitPane` (`tmp/herdr/src/pane.rs:170-172`) leaves `HERDR_ENV=1` and
    /// the socket path set while REMOVING the pane id. A single-condition gate arms here and then
    /// reports against a pane this process does not own.
    #[tokio::test]
    async fn the_omit_pane_shape_is_inert() {
        let omit_pane = env(&[
            ("HERDR_ENV", "1"),
            ("HERDR_SOCKET_PATH", "/definitely/not/a/socket"),
        ]);
        assert!(HerdrBridge::start(&omit_pane, true, no_lock()).is_none());

        let empty_pane_id = env(&[
            ("HERDR_ENV", "1"),
            ("HERDR_PANE_ID", ""),
            ("HERDR_SOCKET_PATH", "/definitely/not/a/socket"),
        ]);
        assert!(HerdrBridge::start(&empty_pane_id, true, no_lock()).is_none());
    }

    /// §12 row 2 — detected, never armed. A headless cyrup shares its parent's pane, and two
    /// authorities on one pane make the sidebar flicker between them (pi `herdr-status.ts:377`).
    #[tokio::test]
    async fn a_headless_session_in_a_pane_is_detected_but_never_armed() {
        let in_pane = env(&[
            ("HERDR_ENV", "1"),
            ("HERDR_PANE_ID", "w1:p1"),
            ("HERDR_SOCKET_PATH", "/definitely/not/a/socket"),
        ]);
        assert!(
            HerdrPane::discover(&in_pane).is_some(),
            "the pane IS detected — the gate's second row is about arming, not detection"
        );
        assert!(HerdrBridge::start(&in_pane, false, no_lock()).is_none());
    }

    /// A socket that does not exist is §12 row 3, not row 1: the bridge arms, reports, fails every
    /// send, and never disturbs the session. This is the case in this container, where herdr is
    /// not installed at all.
    #[tokio::test]
    async fn an_armed_bridge_survives_a_socket_that_is_not_there() {
        let in_pane = env(&[
            ("HERDR_ENV", "1"),
            ("HERDR_PANE_ID", "w1:p1"),
            ("HERDR_SOCKET_PATH", "/definitely/not/a/socket"),
        ]);
        let bridge = HerdrBridge::start(&in_pane, true, no_lock()).expect("armed");
        assert_eq!(bridge.pane_id(), "w1:p1");
        bridge.agent_start();
        bridge.set_human_waiting(true);
        bridge.set_human_waiting(false);
        bridge.agent_end();
        bridge.release().await;
        assert!(bridge.reporter.is_released());
    }

    // ---------------------------------------------------------------------------------------
    // The fleet resync — the two LEVELS, and the privacy rule on the bridge's own path
    // ---------------------------------------------------------------------------------------

    /// An active detached run whose every free-text field carries `sentinel`, and whose workflow
    /// graph is built by **the production builder**.
    ///
    /// `workflow_graph_from_run` (`background/workflow_graph.rs:718`) is the only thing that
    /// populates `status.telemetry.workflow_graph` in production — its one caller is
    /// `background/runner_main/status.rs:44-46` — and it sets `label: None` and `phase: None` on
    /// every `WorkflowTaskSpec` (`:752,:763,:775`), because `SingleStepSpec` carries neither. So a
    /// published node label is the AGENT NAME or `Step N`, and a fixture that hand-built a node
    /// with a rich `label` would be asserting over a shape production cannot emit. This one is
    /// built from a `RunnerStep` list, so what it proves is what a user would see.
    fn active_run(token: &str, needs_attention: bool, sentinel: &str) -> AsyncRunView {
        use crate::background::{RunMode, RunState, RunStatus, StepState, StepStatus};
        use crate::spawn::chain_graph::{RunnerStep, SingleStepSpec};

        let run_id = RunId::from_token(token.to_string());
        let mut step = StepStatus::pending(AGENT_NAME);
        step.status = StepState::Running;
        let mut status = RunStatus::queued(run_id.clone(), RunMode::Chain, None);
        status.steps = vec![step];
        status.state = RunState::Running;
        // The free-text field a careless label would reach for.
        status.error = Some(sentinel.to_string());
        if needs_attention {
            status.telemetry.activity_state = Some(ActivityState::NeedsAttention);
        }
        // The production graph, from the production builder. The step's `task` is the user's
        // prompt, and it is in here precisely so that a builder or a label that started reading it
        // would be caught.
        let steps = vec![RunnerStep::SingleStep(SingleStepSpec {
            machine: None,
            skills: None,
            session_dir: None,
            agent: AGENT_NAME.to_string(),
            task: sentinel.to_string(),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            fast: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        })];
        status.telemetry.workflow_graph =
            Some(crate::background::workflow_graph_from_run(&steps, &status));
        AsyncRunView {
            paths: crate::background::RunPaths::for_run(
                std::path::Path::new("/tmp/async"),
                std::path::Path::new("/tmp/results"),
                &run_id,
            ),
            status,
            session_id: None,
            // pi's `AsyncJobState.description` — the free-text task the launch call supplied.
            description: Some(sentinel.to_string()),
            context: None,
            nested_children: Vec::new(),
        }
    }

    /// Poll the model until it reads `want`, or fail. Real time rather than a paused clock: the
    /// watcher is a spawned task on a real `interval`, and what is being proven is that the edge
    /// actually arrives.
    ///
    /// [`StateModel::desired`] rather than `last_reported`, and that is load-bearing: the socket
    /// in these tests does not exist, so every send fails, and a failed send is required to call
    /// [`StateModel::invalidate_last_report`] (`reporter.rs:388`) so the de-duplicator stops
    /// believing herdr holds a value it never received. Polling `last_reported` would therefore
    /// race the reporter clearing it. `desired()` is the model's own answer and is exactly what
    /// the watcher moves: it is `Blocked` only because `set_human_waiting(true)` reached the
    /// model, so deleting the watcher spawn still leaves this at `Idle` and RED.
    async fn settle(bridge: &HerdrBridge, want: cyrup_herdr::schema::PaneAgentState) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            let seen = bridge.model.lock().unwrap().desired().state;
            if seen == want {
                return;
            }
            tokio::time::sleep(HUMAN_WAIT_POLL / 2).await;
        }
        panic!("the model never reached {want:?}");
    }

    fn armed(env: &BTreeMap<String, String>) -> Arc<HerdrBridge> {
        HerdrBridge::start(env, true, no_lock()).expect("armed")
    }

    fn pane_env() -> BTreeMap<String, String> {
        env(&[
            ("HERDR_ENV", "1"),
            ("HERDR_PANE_ID", "w1:p1"),
            ("HERDR_SOCKET_PATH", "/definitely/not/a/socket"),
        ])
    }

    /// Publish `fleet` through the PRODUCTION producer and read back what landed on the
    /// decorative lane.
    ///
    /// `sync_fleet` is the function `native_impl.rs` calls, `Reporter::queued_metadata` is an
    /// observation of the lane `sync_fleet` filled, and there is nothing in between that any test
    /// here supplies. Assert on this and a mutation to `sync_fleet` is RED; assert on a sibling
    /// that re-derives the same text and it is not.
    fn publish(bridge: &HerdrBridge, fleet: &FleetState, project_panes: usize) -> Option<Metadata> {
        bridge.sync_fleet(fleet, project_panes);
        bridge.reporter.queued_metadata()
    }

    /// The rendered strings of a [`Metadata::Set`], joined — what a human reading the pane sees.
    fn published_strings(metadata: Option<Metadata>) -> String {
        match metadata {
            Some(Metadata::Set(text)) => {
                format!(
                    "{}\n{}",
                    text.summary,
                    text.title_suffix.unwrap_or_default()
                )
            }
            other => panic!("expected a published label, got {other:?}"),
        }
    }

    /// **The privacy rule, on the PRODUCTION producer.** `sync_fleet` is what `native_impl.rs`
    /// calls and `published_text` is the only function that turns runs into published text; this
    /// drives the first and reads what the second put on the lane.
    ///
    /// The run here carries the sentinel in every free-text field a careless label would reach
    /// for: `status.error` and `AsyncRunView::description` — the latter is pi's
    /// `AsyncJobState.description`, the task string the launch call supplied, which `label.rs`
    /// lists under "what may never be sent".
    ///
    /// *Gutted by*: reading `AsyncRunView::description` in `RunLabel::from_async_run`; passing
    /// `run.description.as_deref()` as `RunLabel::new`'s launch label inside `published_text`;
    /// falling back to the description when the workflow graph yields nothing. Each of those is
    /// now a mutation of the ONE renderer, so each is RED here.
    #[tokio::test]
    async fn raw_prompts_never_reach_the_published_label() {
        const SENTINEL: &str = "REFACTOR-THE-BILLING-SECRETS-PROMPT";
        let bridge = armed(&pane_env());
        let fleet = FleetState {
            tracked_jobs: vec![active_run("r1", false, SENTINEL)],
            ..FleetState::default()
        };
        let published = published_strings(publish(&bridge, &fleet, 0));
        assert!(
            !published.contains(SENTINEL),
            "the prompt reached pane metadata: {published}"
        );
        assert!(
            published.contains("reviewer"),
            "the agent name is what a production workflow graph can carry: {published}"
        );
        bridge.release().await;
    }

    /// The background level, and the de-duplication across the two lists. `tracked_jobs` (in
    /// memory) and `history_jobs` (on disk) overlap — pi filters the second against the first
    /// before concatenating (`fleet.ts:193,199`) — and a run counted twice would keep the pane at
    /// `working` after it ended.
    #[tokio::test]
    async fn a_resync_counts_each_active_run_once() {
        let bridge = armed(&pane_env());
        let fleet = FleetState {
            tracked_jobs: vec![active_run("r1", false, "x"), active_run("r2", false, "x")],
            // The same two runs, as the on-disk scan also sees them.
            history_jobs: vec![active_run("r1", false, "x"), active_run("r2", false, "x")],
            ..FleetState::default()
        };
        publish(&bridge, &fleet, 0);
        assert_eq!(
            bridge
                .model
                .lock()
                .unwrap()
                .last_reported()
                .map(|r| r.state),
            Some(cyrup_herdr::schema::PaneAgentState::Working),
            "two active detached runs make the pane working"
        );
        let published = published_strings(bridge.reporter.queued_metadata());
        assert!(
            published.contains("2 subagents"),
            "a run counted twice would read 4: {published}"
        );

        // Nothing active: the pane goes idle and the label is CLEARED rather than emptied.
        let cleared = publish(&bridge, &FleetState::default(), 0);
        assert_eq!(
            bridge
                .model
                .lock()
                .unwrap()
                .last_reported()
                .map(|r| r.state),
            Some(cyrup_herdr::schema::PaneAgentState::Idle)
        );
        assert_eq!(
            cleared,
            Some(Metadata::Clear),
            "pi clears its keys rather than publishing an empty label (herdr-status.ts:203-215)"
        );
        bridge.release().await;
    }

    /// A background run that has tripped the needs-attention heuristic blocks the pane, and the
    /// summary gains the warning mark. This is the SECOND `blocked` source (the human-wait gate is
    /// the first and outranks it): a detached runner asking for the human is exactly the thing the
    /// sidebar has to surface, because nothing else in the UI will.
    ///
    /// Gutted by: dropping the `activity_state` read in `sync_fleet`; ordering `is_running` above
    /// attention in `StateModel::desired`.
    #[tokio::test]
    async fn a_background_run_needing_attention_blocks_the_pane() {
        let bridge = armed(&pane_env());
        let fleet = FleetState {
            tracked_jobs: vec![active_run("r1", true, "x")],
            ..FleetState::default()
        };
        let published = published_strings(publish(&bridge, &fleet, 0));
        assert_eq!(
            bridge
                .model
                .lock()
                .unwrap()
                .last_reported()
                .map(|r| r.state),
            Some(cyrup_herdr::schema::PaneAgentState::Blocked),
        );
        assert!(published.contains('⚠'), "{published}");
        bridge.release().await;
    }

    /// The project-pane clause renders only once that batch's manager exists. At `0` the clause is
    /// absent entirely — pi does the same when its own callback is (`herdr-status.ts:162-163`).
    #[tokio::test]
    async fn the_project_pane_clause_appears_only_when_there_are_panes() {
        let bridge = armed(&pane_env());
        let fleet = FleetState {
            tracked_jobs: vec![active_run("r1", false, "x")],
            ..FleetState::default()
        };
        assert!(!published_strings(publish(&bridge, &fleet, 0)).contains("pane"));
        assert!(published_strings(publish(&bridge, &fleet, 2)).contains("2 panes"));
        bridge.release().await;
    }

    /// Nothing is produced once the pane is released — a `SessionShutdown` racing a late bus event
    /// must not re-claim a pane cyrup has just handed back.
    #[tokio::test]
    async fn a_released_bridge_produces_nothing_further() {
        let bridge = armed(&pane_env());
        bridge.agent_start();
        bridge.release().await;
        let before = bridge.model.lock().unwrap().last_reported().cloned();
        bridge.agent_start();
        bridge.set_human_waiting(true);
        bridge.sync_fleet(
            &FleetState {
                tracked_jobs: vec![active_run("r1", true, "x")],
                ..FleetState::default()
            },
            0,
        );
        assert_eq!(
            bridge.model.lock().unwrap().last_reported().cloned(),
            before,
            "a released bridge must not move the model either"
        );
    }

    /// The watcher turns the lock's level into the model's edge, **across two independent readers
    /// of one backend**. Sampling rather than observing is only sound because
    /// `StateModel::set_human_waiting` de-duplicates — a second `true` must not produce a second
    /// report.
    ///
    /// Both ends take the production route and NEITHER end is handed the other's object:
    ///
    /// * the arm is `services.human_interaction_lock()`, which is literally what
    ///   `native_impl.rs`'s `SessionStart` arm passes to [`arm`];
    /// * the raise is a SECOND, separate `services.human_interaction_lock()` followed by
    ///   `.acquire().await`, which is literally `prompt.rs:174-177`.
    ///
    /// That separation is the assertion. Hand the two ends the same freshly-minted object and the
    /// test still passes while production is broken — which is exactly what the per-native
    /// `HumanWaitGate` version of this test did.
    ///
    /// *Gutted by*: dropping the `watch_human_wait` spawn; making `OneLockHost` mint a fresh lock
    /// per call instead of cloning one (the shape of the bug); `is_held` answering
    /// `available_permits() > 0`.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_human_wait_watcher_follows_the_session_lock() {
        let in_pane = env(&[
            ("HERDR_ENV", "1"),
            ("HERDR_PANE_ID", "w1:p1"),
            ("HERDR_SOCKET_PATH", "/definitely/not/a/socket"),
        ]);
        let services: Arc<dyn HostServices> = Arc::new(OneLockHost::default());

        // `native_impl.rs:478`, verbatim in shape.
        let bridge =
            HerdrBridge::start(&in_pane, true, services.human_interaction_lock()).expect("armed");

        // `prompt.rs:174-177`, verbatim in shape — a different companion, reaching the backend
        // on its own.
        let dialog_lock = services
            .human_interaction_lock()
            .expect("the backend hands out the session lock");
        let guard = dialog_lock.acquire().await;
        settle(&bridge, cyrup_herdr::schema::PaneAgentState::Blocked).await;

        drop(guard);
        settle(&bridge, cyrup_herdr::schema::PaneAgentState::Idle).await;
        bridge.release().await;
    }

    /// With no capability backend bound there is no lock to watch, and the bridge must still arm
    /// and still track everything else. This is the `arm(has_ui, None)` row — a by-value session,
    /// and every test above that passes [`no_lock`].
    #[tokio::test]
    async fn no_capability_backend_still_arms_everything_else() {
        let bridge = armed(&pane_env());
        bridge.agent_start();
        assert_eq!(
            bridge.model.lock().unwrap().desired().state,
            cyrup_herdr::schema::PaneAgentState::Working,
            "turn tracking does not depend on the human-wait watcher"
        );
        bridge.release().await;
    }
}
