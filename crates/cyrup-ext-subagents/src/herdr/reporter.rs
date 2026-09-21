//! The reporter task — the only thing in this crate that writes to herdr's socket.
//!
//! Everything above it ([`super::state`], [`super::label`]) is pure. This file is where a
//! `(state, message)` pair and a `(summary, title-suffix)` pair become
//! `pane.report_agent` and `pane.report_metadata` calls on
//! [`cyrup_herdr::HerdrClient`], and where a session's end becomes `pane.release_agent`.
//!
//! # Two lanes, and why they are separate
//!
//! herdr separates semantic state from presentation and says so on the method — *"`state` carries
//! semantic agent state and affects waits, notifications, and rollups. Report display-only values
//! separately through metadata"* (`tmp/herdr/docs/preview/website/src/content/docs/socket-api.mdx:717-718`,
//! herdr @ `d59d060`). So there are two lanes here, not one queue of a sum type:
//!
//! | lane | verb | carries | dropped on release? |
//! |---|---|---|---|
//! | **critical** | `pane.report_agent` | [`super::state::StateReport`] | no |
//! | **decorative** | `pane.report_metadata` | [`Metadata`] | **yes** |
//!
//! Each lane is a [`tokio::sync::watch`] channel, which is a latest-wins slot: a state edge that
//! supersedes a queued one replaces it rather than queueing behind it. That is correct because
//! both lanes carry **levels**, not deltas — the newest value is the whole truth — and it is what
//! keeps a fleet that produces hundreds of edges a minute from opening hundreds of sockets.
//! herdr's socket is **one request per connection** (`handle_connection_with_stop` reads one line,
//! writes one line and returns, `tmp/herdr/src/api/server.rs:156-317`), so a report is an entire
//! connect/write/read/close. The Python client hand-rolls the same two-lane slot at
//! `tmp/code_puppy_core_plugins/.../herdr/client.py:104-120` and explains the lanes at `:21-34`.
//!
//! The select is `biased`, in lane order: release, then critical, then decorative, then the
//! refresh tick. So a release can never be overtaken by a decorative refresh, and a state edge is
//! never held behind a label repaint.
//!
//! # `seq` is assigned at WIRE time
//!
//! Not at enqueue time, and this is not obvious. herdr's ledger is per-source: *"For the same
//! `source`, reports with a sequence number less than or equal to the last accepted sequence are
//! accepted by the API but ignored by the pane state"* (`socket-api.mdx:798`, applied by
//! `accept_hook_report`, `tmp/herdr/src/terminal/state.rs:707-709`). If `seq` were stamped when a
//! report was enqueued, a critical report that overtook a queued decorative one would carry the
//! **lower** number and herdr would silently drop it. The Python client states exactly this at
//! `client.py:353-356`. [`SeqCounter`] is therefore consulted inside the send, after the lane
//! arbitration has already happened.
//!
//! # Best effort, always
//!
//! > reporting agent state must never be able to disturb the agent itself
//!
//! — `tmp/code_puppy_core_plugins/.../herdr/client.py:17-19`, and pi says the same in its own
//! words (*"Herdr integration is best effort"*,
//! `tmp/pi-subagents` @ `v0.68.0` `src/integrations/herdr-status.ts:188-191`). Every send here
//! swallows its error into a `debug` log. What it does **not** do is pretend the report landed:
//! a failure calls [`super::state::StateModel::invalidate_last_report`], so the de-duplicator
//! stops believing herdr holds a value it never received and the next edge — or the next refresh
//! tick — sends it again.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cyrup_herdr::HerdrClient;
use cyrup_herdr::schema::panes::{
    PaneReleaseAgentParams, PaneReportAgentParams, PaneReportMetadataParams,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use super::label::MetadataText;
use super::state::{StateModel, StateReport};
use super::{AGENT, SOURCE};

/// pi `DEFAULT_TTL_MS` (`herdr-status.ts:9`) — how long herdr keeps this pane's metadata without
/// hearing from cyrup again.
///
/// A **bound**, not a hold time. herdr's `ttl_ms` is caller-supplied with a 24 h ceiling
/// (`#[schemars(range(min = 1, max = 86_400_000))]` on `PaneReportMetadataParams::ttl_ms`,
/// `tmp/herdr/src/api/schema/panes.rs:503-505`; `normalize_metadata_ttl`,
/// `tmp/herdr/src/app/api_helpers.rs:229-242`), and **omitting it means no expiry at all** —
/// *"Omit `ttl_ms` for metadata that should stay until replaced, cleared, or the pane/workspace
/// closes"* (`socket-api.mdx:796`).
///
/// It is sent because [`super::AGENT`]'s doc records that nothing else will ever reclaim this
/// pane. A clean exit, and a `kill`, both reach [`Reporter::release`] through cyrup's own signal
/// handler (`crates/cyrup/src/signals.rs:317-330` → `session_shutdown{quit}` → this crate's
/// `SessionShutdown` arm; see [`super`]'s module doc). A `SIGKILL`, and a SECOND signal delivery
/// — which hard-exits, `signals.rs:300-305` — reach nothing. Two minutes of a stale token count
/// is the worst case this bound buys against those two, versus *for ever* without it.
pub const METADATA_TTL_MS: u64 = 120_000;

/// pi `DEFAULT_REFRESH_MS` (`herdr-status.ts:10`) — how often the live label is re-published, so
/// that [`METADATA_TTL_MS`] never expires under a session that is still running.
///
/// The tick is also the retry edge for a **state** report that failed to send (§12's "herdr's
/// server gone" row: keep the desired state, retry on the next edge and on the refresh).
pub const METADATA_REFRESH: Duration = Duration::from_secs(45);

/// How long [`Reporter::release`] waits for the task to finish its release before giving up.
///
/// A session shutdown may not hang on a herdr that stopped answering, and the socket's own
/// per-call deadline ([`cyrup_herdr::DEFAULT_TIMEOUT`]) already bounds the call — this bounds the
/// whole drain, including a send that was already in flight when the release was queued.
pub const RELEASE_TIMEOUT: Duration = Duration::from_secs(5);

/// The `summary` metadata token — pi's own key (`herdr-status.ts:225`).
///
/// Inside herdr's key charset: `^[A-Za-z0-9_-]{1,32}$`, at most 16 keys per request and 32
/// retained per pane (`tmp/herdr/src/app/api_helpers.rs:202-208`, `socket-api.mdx:794`).
pub const TOKEN_SUMMARY: &str = "summary";

/// The `title-suffix` metadata token — pi's own key (`herdr-status.ts:226`). The hyphen is inside
/// herdr's charset.
pub const TOKEN_TITLE_SUFFIX: &str = "title-suffix";

/// The three `state_labels` keys cyrup publishes, all carrying the same text — pi publishes
/// exactly these three (`herdr-status.ts:224-226`: `idle=`, `done=`, `working=`).
///
/// `blocked` is deliberately **not** among them: when the pane is blocked the thing the human must
/// read is the reason, and that travels on [`super::state::StateReport::message`] where herdr's
/// own rollups can see it. Overwriting the `blocked` label with the run summary would bury it.
/// The keys are validated by `normalize_state_labels`
/// (`tmp/herdr/src/app/api/panes.rs:1649-1658`) against `idle|working|blocked|done|unknown`
/// (`socket-api.mdx:780`).
pub const STATE_LABEL_KEYS: [&str; 3] = ["idle", "working", "done"];

/// What the decorative lane carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Metadata {
    /// Publish this text under [`TOKEN_SUMMARY`], [`TOKEN_TITLE_SUFFIX`] and
    /// [`STATE_LABEL_KEYS`], bounded by [`METADATA_TTL_MS`].
    Set(MetadataText),
    /// Nothing is running: clear the keys cyrup owns and leave the rest of the pane alone.
    ///
    /// pi's own clear path (`herdr-status.ts:203-215`) — it does not publish an empty label, it
    /// clears. A `tokens` entry of JSON `null` is herdr's documented clear in the patch
    /// (`socket-api.mdx:782`), which is why this is not `clear_*` for the tokens.
    Clear,
}

/// A monotonic, wall-clock-seeded sequence.
///
/// Seeded from the clock rather than from zero for the reason pi and the Python client both give
/// (`herdr-status.ts:16-21`, `client.py:129,146-149`): herdr's per-source ledger survives this
/// process, so a restarted cyrup that began again at `1` would have every report ignored until it
/// climbed back past the dead process's last number. Microseconds since the epoch are always
/// ahead of any previous run of this program, and `max(prev + 1, now)` keeps it strictly
/// increasing even if the clock steps backwards.
#[derive(Debug, Default)]
pub struct SeqCounter(AtomicU64);

impl SeqCounter {
    /// The next sequence number, strictly greater than every number this counter has issued.
    pub fn next(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| u64::try_from(elapsed.as_micros()).ok())
            .unwrap_or(0);
        let mut prev = self.0.load(Ordering::Acquire);
        loop {
            let next = now.max(prev.saturating_add(1));
            match self
                .0
                .compare_exchange_weak(prev, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return next,
                Err(observed) => prev = observed,
            }
        }
    }
}

/// The handle the bridge holds: two coalescing lanes, a release latch, and the task draining them.
#[derive(Debug)]
pub struct Reporter {
    state_tx: watch::Sender<Option<StateReport>>,
    meta_tx: watch::Sender<Option<Metadata>>,
    release_tx: watch::Sender<bool>,
    /// Latched by [`Self::release`]. Once set, both lanes are closed to new values, so nothing can
    /// land on the wire after the release.
    released: AtomicBool,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl Reporter {
    /// Spawn the reporter for `pane_id` on `client`, sharing `model` with the edge producers.
    ///
    /// The model is shared rather than owned because de-duplication has to happen where the edge
    /// is (so a no-op edge never reaches a lane at all) while invalidation has to happen where the
    /// failure is (here). One `Mutex<StateModel>`, locked without ever holding it across an
    /// `await`.
    ///
    /// `agent_view` is [`super::view::enabled`], threaded from the bridge's own `EnvSource`. It
    /// rides on this task rather than on one of its own because the view has exactly the same
    /// lifetime as the reporter — installed once when the drain starts, cleared once when it
    /// stops — and because this task already owns a client and already runs the teardown.
    #[must_use]
    pub fn spawn(
        client: HerdrClient,
        pane_id: String,
        model: Arc<Mutex<StateModel>>,
        agent_view: bool,
    ) -> Self {
        let (state_tx, state_rx) = watch::channel(None);
        let (meta_tx, meta_rx) = watch::channel(None);
        let (release_tx, release_rx) = watch::channel(false);
        let task = tokio::spawn(drain(Drain {
            client,
            pane_id,
            model,
            state_rx,
            meta_rx,
            release_rx,
            seq: SeqCounter::default(),
            agent_view,
        }));
        Self {
            state_tx,
            meta_tx,
            release_tx,
            released: AtomicBool::new(false),
            task: Mutex::new(Some(task)),
        }
    }

    /// Queue a state report on the critical lane. A later report replaces an undelivered one.
    pub fn report(&self, report: StateReport) {
        if self.released.load(Ordering::Acquire) {
            return;
        }
        let _ignored = self.state_tx.send(Some(report));
    }

    /// Queue a metadata publish or clear on the decorative lane.
    pub fn metadata(&self, metadata: Metadata) {
        if self.released.load(Ordering::Acquire) {
            return;
        }
        let _ignored = self.meta_tx.send(Some(metadata));
    }

    /// The value the decorative lane is holding right now — what [`Self::metadata`] last put
    /// there, and exactly what the drain will publish (or re-publish on the refresh tick).
    ///
    /// An OBSERVATION of the lane, not a re-derivation: the only way a value gets here is through
    /// [`Self::metadata`], whose only production caller is
    /// [`super::runtime::HerdrBridge::sync_fleet`]. That is why the privacy rule is asserted
    /// through it — reading a lane the producer filled proves what the producer produced, where a
    /// test-only sibling that re-computed the same text from the same inputs would not.
    ///
    /// `None` before anything has been queued. Coalescing means an undelivered value is replaced
    /// rather than queued behind, so this is a level and not a log.
    #[must_use]
    pub fn queued_metadata(&self) -> Option<Metadata> {
        self.meta_tx.borrow().clone()
    }

    /// Whether [`Self::release`] has already run. The bridge reads this so a second shutdown edge
    /// (a `SessionShutdown` racing the signal guard) is a no-op rather than a second release.
    #[must_use]
    pub fn is_released(&self) -> bool {
        self.released.load(Ordering::Acquire)
    }

    /// Release the pane and stop the task, bounded by [`RELEASE_TIMEOUT`]. Idempotent.
    ///
    /// **This is correctness, not hygiene.** herdr never reclaims agent state from a `cyrup:`
    /// source: `hook_authority_is_effective` short-circuits to `true` for a non-`full_lifecycle`
    /// source (`tmp/herdr/src/terminal/state.rs:1812-1817`), `HookAuthority` carries no expiry at
    /// all (`:23`), and the process-exit override only fires for an agent herdr can name
    /// (`:401-407` — and `"cyrup"` deliberately is not one, see [`super::AGENT`]). A cyrup that
    /// stops reporting without releasing leaves a `working` or `blocked` row in the human's
    /// sidebar that only a herdr restart clears.
    pub async fn release(&self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ignored = self.release_tx.send(true);
        let handle = self
            .task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(handle) = handle {
            match tokio::time::timeout(RELEASE_TIMEOUT, handle).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => tracing::debug!(%err, "herdr: the reporter task ended abnormally"),
                Err(_elapsed) => {
                    tracing::debug!(
                        "herdr: the pane release did not complete within {RELEASE_TIMEOUT:?}"
                    );
                }
            }
        }
    }
}

/// Everything the task owns for its whole life.
struct Drain {
    client: HerdrClient,
    pane_id: String,
    model: Arc<Mutex<StateModel>>,
    state_rx: watch::Receiver<Option<StateReport>>,
    meta_rx: watch::Receiver<Option<Metadata>>,
    release_rx: watch::Receiver<bool>,
    seq: SeqCounter,
    /// Whether this session installs the opt-in sidebar projection ([`super::view`]).
    agent_view: bool,
}

/// The task body: arbitrate the two lanes, then release once and return.
///
/// The `biased` select is the ordering contract of this file. Without it `tokio::select!` picks a
/// ready branch at random, and a release could be overtaken by a metadata repaint queued in the
/// same instant — which would leave presentation on a pane whose agent had just been released.
async fn drain(mut drain: Drain) {
    let mut refresh = tokio::time::interval(METADATA_REFRESH);
    refresh.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // `tokio::time::interval` fires immediately on its first tick; the refresh is a *re*-publish,
    // so the first one is consumed here rather than duplicating whatever the arming edge sent.
    refresh.tick().await;
    let mut last_metadata: Option<Metadata> = None;

    // `[CYRUP-EXCEEDS-UPSTREAM]`, opt-in — see [`super::view`] for why it is a sort with no filter
    // and why it is off by default. Before the loop so the ordering is in place for the FIRST
    // report this session sends; a failure is logged and nothing else, because a sidebar that is
    // ordered the way the user had it is a degraded view and not a broken session.
    //
    // It rides on this task, so it is SERIALIZED ahead of the first report exactly as any other
    // call on this client would be — the whole file is one request at a time by construction, and
    // the cost of a slow herdr here is one request's latency on a session whose reports are
    // equally slow. It is not spawned separately: a second task would race the release below and
    // could install the view after this session had already cleared it.
    if drain.agent_view {
        match drain.client.set_agent_view(super::view::set_params()).await {
            Ok(view) => tracing::debug!(
                active = view.active,
                source = ?view.source,
                "herdr: the fleet sidebar projection is installed"
            ),
            Err(err) => tracing::debug!(%err, "herdr: agent.view.set did not land"),
        }
    }

    loop {
        tokio::select! {
            biased;

            result = drain.release_rx.changed() => {
                // A send error means the handle was dropped without a release being asked for —
                // the bridge is gone, so there is nobody left to release for.
                if result.is_err() {
                    return;
                }
                break;
            }

            result = drain.state_rx.changed() => {
                if result.is_err() {
                    return;
                }
                let report = drain.state_rx.borrow_and_update().clone();
                if let Some(report) = report {
                    send_state(&drain.client, &drain.pane_id, &drain.seq, &drain.model, &report)
                        .await;
                }
            }

            result = drain.meta_rx.changed() => {
                if result.is_err() {
                    return;
                }
                let metadata = drain.meta_rx.borrow_and_update().clone();
                if let Some(metadata) = metadata {
                    send_metadata(&drain.client, &drain.pane_id, &drain.seq, &metadata).await;
                    last_metadata = Some(metadata);
                }
            }

            _ = refresh.tick() => {
                if let Some(metadata) = last_metadata.clone() {
                    send_metadata(&drain.client, &drain.pane_id, &drain.seq, &metadata).await;
                }
                // §12's "in a pane, herdr's server gone" row: a report that never landed is
                // re-sent here, because `invalidate_last_report` cleared the de-duplicator and
                // `resend` is what turns that back into a value.
                let pending = {
                    let mut model = drain
                        .model
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if model.last_reported().is_none() {
                        model.resend()
                    } else {
                        None
                    }
                };
                if let Some(report) = pending {
                    send_state(&drain.client, &drain.pane_id, &drain.seq, &drain.model, &report)
                        .await;
                }
            }
        }
    }

    // The clear goes FIRST and the release LAST. pi's own `dispose()` clears its metadata and
    // never releases at all (`herdr-status.ts:388-398` — its host process is herdr's registered
    // lifecycle authority, `tmp/herdr/src/detect/mod.rs:327-337`, so it has nothing of its own to
    // hand back). cyrup is its own authority and must release; doing it before the clear would
    // publish presentation onto a pane record whose agent had already been retired.
    if last_metadata.is_some() {
        send_metadata(&drain.client, &drain.pane_id, &drain.seq, &Metadata::Clear).await;
    }
    send_release(&drain.client, &drain.pane_id, &drain.seq).await;
    // LAST, and ownership-scoped. The projection is server-wide rather than pane-scoped, so it is
    // not part of the pane's own clear-then-release ordering above; and naming the source means a
    // session whose view was already replaced leaves the replacement alone
    // (`socket-api.mdx:494`).
    if drain.agent_view
        && let Err(err) = drain
            .client
            .clear_agent_view(super::view::clear_params())
            .await
    {
        tracing::debug!(%err, "herdr: agent.view.clear did not land");
    }
}

/// One `pane.report_agent`. Errors are swallowed; the de-duplicator is told the truth.
async fn send_state(
    client: &HerdrClient,
    pane_id: &str,
    seq: &SeqCounter,
    model: &Arc<Mutex<StateModel>>,
    report: &StateReport,
) {
    let mut params = PaneReportAgentParams::new(pane_id, SOURCE, AGENT, report.state);
    params.message.clone_from(&report.message);
    params.seq = Some(seq.next());
    if let Err(err) = client.report_agent(params).await {
        tracing::debug!(%err, state = ?report.state, "herdr: pane.report_agent did not land");
        model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .invalidate_last_report();
    }
}

/// One `pane.report_metadata`. Errors are swallowed and nothing is invalidated: the decorative
/// lane's own refresh tick is its retry.
async fn send_metadata(client: &HerdrClient, pane_id: &str, seq: &SeqCounter, metadata: &Metadata) {
    let params = metadata_params(pane_id, metadata, seq.next());
    if let Err(err) = client.report_metadata(params).await {
        tracing::debug!(%err, "herdr: pane.report_metadata did not land");
    }
}

/// One `pane.release_agent`.
async fn send_release(client: &HerdrClient, pane_id: &str, seq: &SeqCounter) {
    let mut params = PaneReleaseAgentParams::new(pane_id, SOURCE, AGENT);
    params.seq = Some(seq.next());
    if let Err(err) = client.release_agent(params).await {
        tracing::debug!(%err, "herdr: pane.release_agent did not land");
    }
}

/// Build the `pane.report_metadata` params for one [`Metadata`].
///
/// Two departures from pi, both deliberate:
///
/// 1. **No `applies_to_source`.** pi sends `--applies-to-source herdr:pi`
///    (`herdr-status.ts:222`) because its *host* process is herdr's registered authority and the
///    subagents extension is decorating that record. cyrup reports under its own source
///    ([`super::SOURCE`]) and owns its own record, so redirecting the write would be claiming a
///    registration cyrup does not have.
/// 2. **A `None` title suffix is sent as an explicit clear.** pi omits the key when there is no
///    suffix (`herdr-status.ts:226`, a spread on a ternary), which leaves the previous suffix on
///    the pane. herdr's token patch is documented as *"string sets, JSON `null` clears, omitted
///    keys unchanged"* (`socket-api.mdx:782`), so omitting is not "no suffix", it is "keep the old
///    one". `[CYRUP-EXCEEDS-UPSTREAM]` — a strictly better use of the same documented patch.
fn metadata_params(pane_id: &str, metadata: &Metadata, seq: u64) -> PaneReportMetadataParams {
    let mut params = PaneReportMetadataParams::new(pane_id, SOURCE);
    params.agent = Some(AGENT.to_string());
    params.seq = Some(seq);
    match metadata {
        Metadata::Set(text) => {
            for key in STATE_LABEL_KEYS {
                params
                    .state_labels
                    .insert(key.to_string(), text.summary.clone());
            }
            params
                .tokens
                .insert(TOKEN_SUMMARY.to_string(), Some(text.summary.clone()));
            params
                .tokens
                .insert(TOKEN_TITLE_SUFFIX.to_string(), text.title_suffix.clone());
            params.ttl_ms = Some(METADATA_TTL_MS);
        }
        Metadata::Clear => {
            // `clear_state_labels` with an EMPTY `state_labels` map. Setting and clearing the same
            // field in one call is `invalid_metadata_request`
            // (`tmp/herdr/src/app/api/panes.rs:1659-1668`), and a call that neither sets nor
            // clears anything is the same error (`:1669-1682`) — which is why the token nulls are
            // here too rather than this being a bare `clear_state_labels`.
            params.clear_state_labels = true;
            params.tokens.insert(TOKEN_SUMMARY.to_string(), None);
            params.tokens.insert(TOKEN_TITLE_SUFFIX.to_string(), None);
        }
    }
    params
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    fn text(summary: &str, suffix: Option<&str>) -> MetadataText {
        MetadataText {
            summary: summary.to_string(),
            title_suffix: suffix.map(str::to_string),
        }
    }

    /// The seam herdr's own method doc draws (`socket-api.mdx:717-718`): presentation may never
    /// ride on `pane.report_agent`, and semantic state may never ride on `pane.report_metadata`.
    /// A refactor that folds the two verbs into one still *looks* right on screen while breaking
    /// herdr's waits and rollups, so the shape is pinned on both sides.
    #[test]
    fn state_and_presentation_are_built_by_different_verbs() {
        let report = StateReport {
            state: cyrup_herdr::schema::PaneAgentState::Blocked,
            message: Some("awaiting approval".to_string()),
        };
        let mut agent = PaneReportAgentParams::new("w1:p1", SOURCE, AGENT, report.state);
        agent.message.clone_from(&report.message);
        let agent = serde_json::to_value(&agent).unwrap();
        assert!(agent.get("state").is_some(), "the state verb carries state");
        assert!(agent.get("tokens").is_none());
        assert!(agent.get("state_labels").is_none());
        assert!(agent.get("title").is_none());

        let meta = serde_json::to_value(metadata_params(
            "w1:p1",
            &Metadata::Set(text("⏳ 2 subagents", Some("⏳2"))),
            7,
        ))
        .unwrap();
        assert!(
            meta.get("state").is_none(),
            "the metadata verb carries no state"
        );
        assert!(meta.get("tokens").is_some());
        assert!(meta.get("state_labels").is_some());
    }

    /// pi publishes the same text under `idle`, `done` and `working` (`herdr-status.ts:224-226`)
    /// and under the `summary` token. `blocked` is left alone so the blocked *reason* stays
    /// readable.
    #[test]
    fn a_publish_sets_three_state_labels_and_two_tokens() {
        let params = metadata_params(
            "w1:p1",
            &Metadata::Set(text("⏳ 1 subagent (docs)", Some("⏳docs"))),
            9,
        );
        assert_eq!(params.state_labels.len(), 3);
        for key in STATE_LABEL_KEYS {
            assert_eq!(
                params.state_labels.get(key).map(String::as_str),
                Some("⏳ 1 subagent (docs)")
            );
        }
        assert!(!params.state_labels.contains_key("blocked"));
        assert_eq!(
            params.tokens.get(TOKEN_SUMMARY).cloned().flatten(),
            Some("⏳ 1 subagent (docs)".to_string())
        );
        assert_eq!(
            params.tokens.get(TOKEN_TITLE_SUFFIX).cloned().flatten(),
            Some("⏳docs".to_string())
        );
        assert_eq!(params.ttl_ms, Some(METADATA_TTL_MS));
        assert_eq!(params.agent.as_deref(), Some(AGENT));
        assert_eq!(params.applies_to_source, None);
    }

    /// `[CYRUP-EXCEEDS-UPSTREAM]` — a run that loses its label must clear the suffix, not inherit
    /// the previous one. herdr's patch treats an omitted key as "unchanged"
    /// (`socket-api.mdx:782`), so the explicit `null` is the difference between a correct pane and
    /// a stale title.
    #[test]
    fn a_publish_without_a_suffix_clears_the_previous_one() {
        let params = metadata_params("w1:p1", &Metadata::Set(text("⏳ 1 subagent", None)), 9);
        assert_eq!(
            params.tokens.get(TOKEN_TITLE_SUFFIX),
            Some(&None),
            "the key is present and null — present-and-null CLEARS, absent KEEPS"
        );
        let wire = serde_json::to_value(&params).unwrap();
        assert!(wire["tokens"][TOKEN_TITLE_SUFFIX].is_null());
    }

    /// The clear path never sends an empty label. herdr rejects a metadata call that neither sets
    /// nor clears (`tmp/herdr/src/app/api/panes.rs:1669-1682`), and rejects one that does both to
    /// the same field (`:1659-1668`) — so the clear is `clear_state_labels` with an EMPTY map,
    /// plus null token entries.
    #[test]
    fn the_clear_path_clears_labels_and_nulls_both_tokens() {
        let params = metadata_params("w1:p1", &Metadata::Clear, 11);
        assert!(params.clear_state_labels);
        assert!(
            params.state_labels.is_empty(),
            "setting and clearing the same field in one call is invalid_metadata_request"
        );
        assert_eq!(params.tokens.get(TOKEN_SUMMARY), Some(&None));
        assert_eq!(params.tokens.get(TOKEN_TITLE_SUFFIX), Some(&None));
        assert_eq!(
            params.ttl_ms, None,
            "nothing is being set, so nothing expires"
        );
    }

    /// Every token key and the source must satisfy herdr's charsets, or the whole call is
    /// refused (`invalid_metadata_token` / `invalid_metadata_source`,
    /// `tmp/herdr/src/app/api/panes.rs:1633,1638`; charsets at `socket-api.mdx:794`).
    #[test]
    fn the_token_keys_and_the_source_are_inside_herdrs_charsets() {
        for key in [TOKEN_SUMMARY, TOKEN_TITLE_SUFFIX] {
            assert!((1..=32).contains(&key.len()), "{key} length");
            assert!(
                key.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "{key} charset"
            );
        }
        assert!(SOURCE.len() <= 80);
        assert!(
            SOURCE
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '_' | '-')),
            "source charset"
        );
    }

    /// herdr ignores a report whose `seq` is not greater than the last one it accepted for the
    /// same source (`socket-api.mdx:798`). Strictly increasing is therefore the whole contract,
    /// and the clock seed is what keeps a restarted cyrup from being ignored until it catches up.
    #[test]
    fn the_sequence_is_strictly_increasing_and_clock_seeded() {
        let seq = SeqCounter::default();
        let first = seq.next();
        assert!(first > 1_600_000_000_000_000, "seeded from the wall clock");
        let mut previous = first;
        for _ in 0..1_000 {
            let next = seq.next();
            assert!(next > previous, "{next} must exceed {previous}");
            previous = next;
        }
    }
}
