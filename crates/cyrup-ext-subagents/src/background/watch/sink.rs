//! Where a formatted completion notification is delivered: the [`CompletionSink`] abstraction, the
//! stderr graceful-degradation default, and the real turn-injecting sink over P-1 host services
//! (R-SA-101). Split out of `background/watch.rs`; ports pi `runs/background/notify.ts:399-412`.

use super::message::CompletionMessage;
use super::results_watcher::DEDUP_TTL;
use crate::background::RunId;
use crate::background::delivery::CompletionDelivery;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where a delivered [`CompletionMessage`] is sent (R-SA-101's turn-re-entry hand-off). Injecting a
/// message into a live session's turn loop needs a session/agent-turn handle this crate does not
/// hold (`HostCtx` exposes no message channel today — see this module's R-SA-101 note and arch-SA
/// §2.1/§12 item 10), so the concrete production sink is threaded in from the host through the
/// extension facade; the crate ships a graceful-degradation default ([`LoggingCompletionSink`]) and
/// a capturing sink for tests.
///
/// `deliver` reports what became of the message as a [`CompletionDelivery`]. The `Delivered` arm
/// carries a [`crate::background::delivery::DeliveryReceipt`] — the authority to destroy the
/// underlying result payload (R-SA-099's delete-last) — and `Deferred` leaves the payload on disk
/// for retry-in-place on the next scan (R-SA-102).
///
/// It used to return a bare `bool`, which named neither of those things: an implementor choosing
/// `true` was choosing irreversible destruction of a background child's only answer without the
/// signature ever saying so.
#[async_trait::async_trait]
pub trait CompletionSink: Send + Sync {
    /// Deliver one completion notification for `run_id`. See the trait doc for the contract.
    async fn deliver(&self, run_id: &RunId, message: CompletionMessage) -> CompletionDelivery;
}

/// The graceful-degradation default sink: emits the formatted notification to stderr and reports it
/// delivered (so the result file is deleted, upholding pi's delete-last contract). Swapping in a
/// live-session turn-injection sink is the remaining outer-layer hand-off (R-SA-101) — until the
/// host threads a message channel through the extension facade, this keeps the watcher's install →
/// scan → format → delete pipeline observable and correct rather than silently discarding
/// completions.
#[derive(Debug, Default)]
pub struct LoggingCompletionSink;

#[async_trait::async_trait]
impl CompletionSink for LoggingCompletionSink {
    async fn deliver(&self, run_id: &RunId, message: CompletionMessage) -> CompletionDelivery {
        // `tracing::info!`, NOT `eprintln!`, for a mechanical reason: in an interactive session
        // stderr IS the terminal, so a raw write paints over the live ratatui frame and corrupts
        // the display. `crates/cyrup/src/bootstrap.rs:285-290` installs the subscriber with
        // `.with_writer(std::io::stderr)` behind an `EnvFilter` whose default floor is `warn`
        // (`debug` under `--verbose`), so `info!` is suppressed by default and recoverable with
        // `RUST_LOG=info`. `warn!`/`error!` would still reach the TTY and reintroduce the problem.
        tracing::info!(target: "subagent_notify", content = %message.content, "subagent completion");
        // Reporting delivery here is a deliberate degradation, not an oversight: with no live
        // session there is nowhere else for a completion to go, and retaining every payload
        // forever would fill the results directory of any headless embedder. The `Delivered`
        // spelling is what makes that choice visible at the call site.
        CompletionDelivery::delivered(run_id.clone())
    }
}

/// The REAL turn-injecting completion sink (R-SA-101): a completed background run's `subagent-notify`
/// message is injected LIVE into the orchestrator session via the P-1
/// [`cyrup_ext::host::HostServices::inject_message`] backend, with `trigger_turn: true` so the
/// completion re-enters the parent's turn loop (pi `sendCompletion`, `notify.ts:404-410` @v0.64.0:
/// `pi.sendMessage({customType, content, display}, {triggerTurn})`) — instead of the
/// stderr-only [`LoggingCompletionSink`] degradation. Installed by
/// [`crate::extension::SubagentExecutor::install_completion_watcher`] whenever the host-services slot
/// is bound (a live session is present); the logging sink remains the no-host-handle default.
///
/// `deliver` returns `true` (delete the result file, R-SA-099's delete-last) only when injection
/// succeeded; a failed `inject_message` returns `false`, leaving the file in place for retry-in-place
/// on the next scan (R-SA-102).
pub struct HostServicesCompletionSink {
    services: std::sync::Arc<dyn cyrup_ext::host::HostServices>,
}

use cyrup_ext::host::InjectOutcome;

impl HostServicesCompletionSink {
    /// Build a sink over the late-bound live capability backend (P-1).
    #[must_use]
    pub fn new(services: std::sync::Arc<dyn cyrup_ext::host::HostServices>) -> Self {
        Self { services }
    }
}

#[async_trait::async_trait]
impl CompletionSink for HostServicesCompletionSink {
    async fn deliver(&self, run_id: &RunId, message: CompletionMessage) -> CompletionDelivery {
        // No `spawn_blocking`: obtaining the receiver is a cheap synchronous send, and the WAIT is
        // an ordinary async await on a oneshot. The blocking bridge this replaces existed only
        // because the seam had no way to report an outcome — it returned `Ok` the moment a task
        // was spawned, and the `true` derived from it authorised destroying results that never
        // reached the session.
        let CompletionMessage {
            custom_type,
            content,
            display,
            trigger_turn,
            suppressible: _,
        } = message;
        let Ok(ack) = self.services.inject_message_ack(
            &content,
            Some(custom_type.as_str()),
            display,
            None,
            trigger_turn,
        ) else {
            // The seam is not wired to a live session. Nothing was consumed; the payload stays.
            return CompletionDelivery::Deferred;
        };
        match ack.await {
            Ok(InjectOutcome::Accepted) => CompletionDelivery::delivered(run_id.clone()),
            // `SessionUnavailable`, or a pump that dropped the obligation: both mean the message
            // is not in the session, so the payload must survive for the next scan.
            Ok(InjectOutcome::SessionUnavailable) | Err(_) => CompletionDelivery::Deferred,
        }
    }
}

// =================================================================================================
// Cross-channel dedup — the inline-answer ledger and its suppressing decorator
// (`ASYNC_NOTIFY_BUG_REPORT` F3)
// =================================================================================================

/// The hold-off ceiling for [`InlineAnsweredSink`]: a wait cannot outlive its own timeout
/// ([`crate::background::wait::DEFAULT_TIMEOUT_MS`]), plus a poll interval of slack — so a leaked
/// claim degrades to "deliver normally" instead of parking a delivery task forever.
const INLINE_CLAIM_MAX_WAIT: Duration =
    Duration::from_millis(crate::background::wait::DEFAULT_TIMEOUT_MS + 1_000);

/// The hard prune bound for `claimed` entries — belt-and-braces behind the RAII release: one full
/// wait timeout ([`crate::background::wait::DEFAULT_TIMEOUT_MS`]) plus one poll interval
/// ([`crate::background::wait::DEFAULT_POLL_INTERVAL_MS`]). A claim that somehow outlived every
/// `Drop` (which runs on timeout, abort and panic alike) is dropped here rather than pinning a
/// delivery until [`INLINE_CLAIM_MAX_WAIT`].
const CLAIM_HARD_TTL: Duration = Duration::from_millis(
    crate::background::wait::DEFAULT_TIMEOUT_MS
        + crate::background::wait::DEFAULT_POLL_INTERVAL_MS,
);

/// The ledger's shared state. `answered` is one-use and [`DEDUP_TTL`]-pruned; `claimed` is
/// refcounted (two concurrent waits — the tool and the headless auto-drain — can cover the same
/// run, and one guard's drop must not release the other's) and hard-pruned past
/// [`CLAIM_HARD_TTL`]. Both maps are bounded by construction.
#[derive(Debug, Default)]
struct LedgerState {
    /// run id → (live claim count, most recent claim instant — the hard-prune clock).
    claimed: HashMap<RunId, (usize, Instant)>,
    /// run id → when its inline answer was recorded.
    answered: HashMap<RunId, Instant>,
}

/// The `Arc`'d interior of [`InlineAnswerLedger`].
#[derive(Debug)]
struct LedgerInner {
    /// The claim/answer maps, behind a `std::sync::Mutex` that is NEVER held across an `.await` —
    /// which keeps [`crate::background::wait::WaitDeps`]'s `#[derive(Clone, Debug)]` working and
    /// the lock uncontended.
    state: Mutex<LedgerState>,
    /// A generation counter bumped on every claim release; [`InlineAnswerLedger::claim_released`]
    /// sleeps on it. `watch` rather than `Notify` so a subscriber taken BEFORE the re-check can
    /// never miss a release that lands between the two (no lost wakeup).
    generation: tokio::sync::watch::Sender<u64>,
}

impl Default for LedgerInner {
    fn default() -> Self {
        let (generation, _) = tokio::sync::watch::channel(0);
        Self {
            state: Mutex::new(LedgerState::default()),
            generation,
        }
    }
}

/// Which runs a live `wait` is about to answer inline, and which it already did.
///
/// # Why a CLAIM and not just an "already answered" set
///
/// The watcher decides to inject BEFORE the wait renders (`ASYNC_NOTIFY_BUG_REPORT` RC4): the
/// injection is enqueued into the session pump by [`HostServicesCompletionSink::deliver`] and only
/// then awaited, so cancelling the await cannot retract it. The only sound suppression point is
/// before the enqueue, which means a delivery must be able to HOLD OFF while a wait covering that
/// run is in flight. A claim is that hold-off; `answered` is the one-use authority to suppress —
/// the same one-use-authority discipline [`crate::background::delivery::DeliveryReceipt`] already
/// enforces for consumption.
///
/// Cloneable and cheap: one `Arc` over the shared state plus a `watch` generation counter the
/// decorator sleeps on.
#[derive(Clone, Debug, Default)]
pub struct InlineAnswerLedger {
    inner: Arc<LedgerInner>,
}

impl InlineAnswerLedger {
    /// Claim `run_ids` for a wait that is about to block on them. The returned guard releases
    /// every still-claimed id on drop — including on turn abort, timeout and panic — so a
    /// delivery is never parked by a wait that is no longer running.
    #[must_use]
    pub fn claim(&self, run_ids: &[RunId]) -> InlineAnswerClaim {
        if !run_ids.is_empty()
            && let Ok(mut state) = self.inner.state.lock()
        {
            let now = Instant::now();
            Self::prune(&mut state, now);
            for run_id in run_ids {
                let entry = state.claimed.entry(run_id.clone()).or_insert((0, now));
                entry.0 = entry.0.saturating_add(1);
                entry.1 = now;
            }
        }
        InlineAnswerClaim {
            ledger: self.clone(),
            claimed: run_ids.to_vec(),
            answered: Vec::new(),
        }
    }

    /// `true` while any live wait holds a claim on `run_id`.
    #[must_use]
    pub fn is_claimed(&self, run_id: &RunId) -> bool {
        self.inner.state.lock().is_ok_and(|mut state| {
            Self::prune(&mut state, Instant::now());
            state.claimed.contains_key(run_id)
        })
    }

    /// Block until no claim remains for `run_id`. Returns immediately when there is none.
    ///
    /// Subscribes to the generation counter BEFORE re-checking [`Self::is_claimed`], so a release
    /// landing between the check and the sleep still wakes this future — no lost wakeup.
    pub async fn claim_released(&self, run_id: &RunId) {
        let mut generation = self.inner.generation.subscribe();
        while self.is_claimed(run_id) {
            if generation.changed().await.is_err() {
                // The sender lives inside our own `Arc`; unreachable in practice. Degrade to
                // "released" so the caller's bounded timeout is the only wait left.
                return;
            }
        }
    }

    /// Take the one-use "already answered inline" authority for `run_id`.
    ///
    /// REMOVES the entry: one inline answer authorises suppressing exactly one standalone
    /// notification, which is also what bounds the map by construction.
    #[must_use]
    pub fn take_answered(&self, run_id: &RunId) -> bool {
        self.inner.state.lock().is_ok_and(|mut state| {
            // Prune FIRST, so an entry past `DEDUP_TTL` can no longer authorise a suppression.
            Self::prune(&mut state, Instant::now());
            state.answered.remove(run_id).is_some()
        })
    }

    /// Release `claimed` (decrementing refcounts) and convert `answered` marks into redeemable
    /// entries, then wake every [`Self::claim_released`] sleeper. Called from
    /// [`InlineAnswerClaim`]'s `Drop` only.
    fn release(&self, claimed: &[RunId], answered: &[RunId]) {
        if let Ok(mut state) = self.inner.state.lock() {
            let now = Instant::now();
            for run_id in claimed {
                if let Some(entry) = state.claimed.get_mut(run_id) {
                    entry.0 = entry.0.saturating_sub(1);
                    if entry.0 == 0 {
                        state.claimed.remove(run_id);
                    }
                }
            }
            for run_id in answered {
                state.answered.insert(run_id.clone(), now);
            }
            Self::prune(&mut state, now);
        }
        // Bump AFTER the state change, so a woken `claim_released` re-check observes the release.
        // `send_modify` notifies regardless of receiver count and cannot fail.
        self.inner
            .generation
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }

    /// TTL prune, run on every mutation: `answered` expires at [`DEDUP_TTL`] exactly like the
    /// watcher's own seen-set; `claimed` is hard-pruned past [`CLAIM_HARD_TTL`] behind the RAII
    /// drop.
    fn prune(state: &mut LedgerState, now: Instant) {
        state
            .answered
            .retain(|_, at| now.duration_since(*at) < DEDUP_TTL);
        state
            .claimed
            .retain(|_, (_, at)| now.duration_since(*at) < CLAIM_HARD_TTL);
    }
}

/// A live wait's claim over the runs it is blocked on — the RAII half of
/// [`InlineAnswerLedger`]. Dropping it releases every claimed id (waking held deliveries) and
/// records an `answered` entry for each id [`Self::answered`] marked.
#[derive(Debug)]
pub struct InlineAnswerClaim {
    ledger: InlineAnswerLedger,
    claimed: Vec<RunId>,
    answered: Vec<RunId>,
}

impl InlineAnswerClaim {
    /// Record that this wait's response actually carries `run_id`'s value. Converted to an
    /// `answered` entry when the claim is released, so the suppression authority becomes
    /// redeemable only once the response text is final. Idempotent per id.
    pub fn answered(&mut self, run_id: &RunId) {
        if !self.answered.iter().any(|id| id == run_id) {
            self.answered.push(run_id.clone());
        }
    }
}

impl Drop for InlineAnswerClaim {
    fn drop(&mut self) {
        let claimed = std::mem::take(&mut self.claimed);
        let answered = std::mem::take(&mut self.answered);
        self.ledger.release(&claimed, &answered);
    }
}

/// Suppresses a standalone completion notification whose value already reached the orchestrator
/// inside a `wait` tool result in the same transcript (`ASYNC_NOTIFY_BUG_REPORT` F3.3).
///
/// Minting the receipt HERE keeps the custody rule intact — sinks are the receipt authority, and
/// [`LoggingCompletionSink`] is the existing precedent for a non-injecting sink that reports
/// `Delivered`. The delivery guarantee is met, on another channel: the value is in the
/// transcript, so destroying the payload is exactly as safe as after an injection.
pub struct InlineAnsweredSink {
    inner: Arc<dyn CompletionSink>,
    ledger: InlineAnswerLedger,
}

impl InlineAnsweredSink {
    /// Wrap `inner`, consulting `ledger` before every suppressible delivery.
    #[must_use]
    pub fn new(inner: Arc<dyn CompletionSink>, ledger: InlineAnswerLedger) -> Self {
        Self { inner, ledger }
    }
}

#[async_trait::async_trait]
impl CompletionSink for InlineAnsweredSink {
    async fn deliver(&self, run_id: &RunId, message: CompletionMessage) -> CompletionDelivery {
        // A delivery-failure report carries information no `wait` could have surfaced — it is
        // never suppressed (`CompletionMessage::suppressible`, F3.2).
        if !message.suppressible {
            return self.inner.deliver(run_id, message).await;
        }
        // Hold off while a wait covering this run is in flight — RC4's race: the enqueue into the
        // session pump is irrevocable, so the only sound suppression point is before it. Bounded
        // so a leaked claim degrades to "deliver normally" instead of parking this task forever.
        // Parking ONE delivery task is harmless only because F1 made deliveries concurrent; this
        // decorator must not be installed without F1.
        let _ = tokio::time::timeout(INLINE_CLAIM_MAX_WAIT, self.ledger.claim_released(run_id))
            .await;
        if self.ledger.take_answered(run_id) {
            return CompletionDelivery::delivered(run_id.clone());
        }
        self.inner.deliver(run_id, message).await
    }
}
