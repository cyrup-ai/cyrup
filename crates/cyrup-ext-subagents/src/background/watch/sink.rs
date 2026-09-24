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
            // This sink never batches (the batching one is `batch::BatchingHostServicesCompletionSink`).
            group_part: _,
            batch_key: _,
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
    crate::background::wait::DEFAULT_TIMEOUT_MS + crate::background::wait::DEFAULT_POLL_INTERVAL_MS,
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
        let _ =
            tokio::time::timeout(INLINE_CLAIM_MAX_WAIT, self.ledger.claim_released(run_id)).await;
        if self.ledger.take_answered(run_id) {
            return CompletionDelivery::delivered(run_id.clone());
        }
        self.inner.deliver(run_id, message).await
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::super::message::{format_completion_message, format_undeliverable_message};
    use super::super::tests::{child_result, result_with_children};
    use super::*;
    use crate::background::RunState;
    use tokio::sync::Mutex as AsyncMutex;

    /// What actually reached the inner sink — i.e. what actually reached the session.
    type Delivered = Arc<AsyncMutex<Vec<CompletionMessage>>>;

    /// A capturing inner sink — the same shape [`super::super::install`]'s tests use, so "the
    /// notification was injected" is observable as a recorded push rather than inferred from a
    /// return value. [`InlineAnsweredSink`] reports `Delivered` for a SUPPRESSED message too (the
    /// value reached the orchestrator on the `wait` channel), so the return value alone cannot
    /// tell suppression from delivery — this can.
    #[derive(Clone, Default)]
    struct CapturingSink {
        delivered: Delivered,
    }

    #[async_trait::async_trait]
    impl CompletionSink for CapturingSink {
        async fn deliver(&self, run_id: &RunId, message: CompletionMessage) -> CompletionDelivery {
            self.delivered.lock().await.push(message);
            CompletionDelivery::delivered(run_id.clone())
        }
    }

    /// A decorated sink plus a handle on what got past it.
    fn wired(ledger: &InlineAnswerLedger) -> (InlineAnsweredSink, Delivered) {
        let inner = CapturingSink::default();
        let delivered = Arc::clone(&inner.delivered);
        (
            InlineAnsweredSink::new(Arc::new(inner), ledger.clone()),
            delivered,
        )
    }

    /// The ordinary value-carrying completion — the ONLY suppressible shape (F3.2). The inner
    /// assertion is load-bearing: if `format_completion_message` ever stopped setting the flag,
    /// every suppression test below would pass vacuously through the pass-through arm.
    fn suppressible(run: &str, output: &str) -> CompletionMessage {
        let result = result_with_children(
            run,
            RunState::Complete,
            true,
            None,
            vec![child_result("worker", Some(output), 0)],
        );
        let message = format_completion_message(&result);
        assert!(
            message.suppressible,
            "F3.2: an ordinary completion is suppressible"
        );
        message
    }

    /// A delivery-FAILURE report — never suppressible, whatever the ledger says.
    fn undeliverable(run: &str, output: &str) -> CompletionMessage {
        let result = result_with_children(
            run,
            RunState::Complete,
            true,
            None,
            vec![child_result("worker", Some(output), 0)],
        );
        let message = format_undeliverable_message(&result);
        assert!(
            !message.suppressible,
            "F3.2: a delivery-failure report is never suppressible"
        );
        message
    }

    // ---------------------------------------------------------------------------------------
    // The one-use suppression authority
    // ---------------------------------------------------------------------------------------

    /// One inline answer authorises suppressing exactly ONE standalone notification — the same
    /// one-use-authority discipline [`crate::background::delivery::DeliveryReceipt`] enforces for
    /// consumption. A `take_answered` that failed to REMOVE the entry would silently swallow every
    /// later completion for the run, which is a worse defect than the duplicate it fixes.
    #[tokio::test]
    async fn an_inline_answer_suppresses_exactly_one_standalone_notification() {
        let ledger = InlineAnswerLedger::default();
        let (sink, delivered) = wired(&ledger);
        let run = RunId::from_token("run-once");

        let mut claim = ledger.claim(std::slice::from_ref(&run));
        claim.answered(&run);
        drop(claim);

        let first = sink.deliver(&run, suppressible("run-once", "VALUE")).await;
        assert!(
            matches!(first, CompletionDelivery::Delivered(_)),
            "the value IS in the transcript, so the payload is consumable"
        );
        assert!(
            delivered.lock().await.is_empty(),
            "the duplicate must not be injected"
        );

        let second = sink.deliver(&run, suppressible("run-once", "VALUE")).await;
        assert!(matches!(second, CompletionDelivery::Delivered(_)));
        assert_eq!(
            delivered.lock().await.len(),
            1,
            "one answer authorises one suppression; the next delivery goes through"
        );
    }

    /// A wait that timed out, aborted or panicked answered NOTHING. Its guard's `Drop` still runs,
    /// and that release must not manufacture an authority — suppressing here would destroy a
    /// background child's only answer.
    #[tokio::test]
    async fn a_claim_released_without_an_answer_lets_the_notification_through() {
        let ledger = InlineAnswerLedger::default();
        let (sink, delivered) = wired(&ledger);
        let run = RunId::from_token("run-unanswered");

        drop(ledger.claim(std::slice::from_ref(&run)));
        assert!(!ledger.is_claimed(&run), "Drop releases the claim");

        let delivery = sink
            .deliver(&run, suppressible("run-unanswered", "VALUE"))
            .await;
        assert!(matches!(delivery, CompletionDelivery::Delivered(_)));
        assert_eq!(
            delivered.lock().await.len(),
            1,
            "an unanswered run is delivered normally"
        );
    }

    // ---------------------------------------------------------------------------------------
    // RC4: the hold-off
    // ---------------------------------------------------------------------------------------

    /// **The end-to-end regression this whole task exists to fix.**
    ///
    /// [`HostServicesCompletionSink::deliver`] enqueues into the session pump and only THEN awaits
    /// the ack, so cancelling the await cannot retract the injection: the only sound suppression
    /// point is before the enqueue. A delivery for a run a live `wait` is blocked on must therefore
    /// HOLD OFF until that wait either answers it inline or releases its claim. Without the
    /// hold-off the watcher wins the race, the message is already in the pump, and the orchestrator
    /// receives the same value twice — the reported defect.
    #[tokio::test]
    async fn a_delivery_holds_off_until_the_covering_wait_releases_its_claim() {
        let ledger = InlineAnswerLedger::default();
        let (sink, delivered) = wired(&ledger);
        let run = RunId::from_token("run-rc4");

        // A live `wait` claims the run BEFORE the watcher's delivery starts — RC4's window.
        let mut claim = ledger.claim(std::slice::from_ref(&run));

        let deliver = sink.deliver(&run, suppressible("run-rc4", "INLINE-VALUE"));
        tokio::pin!(deliver);
        assert!(
            tokio::time::timeout(Duration::from_millis(150), &mut deliver)
                .await
                .is_err(),
            "a delivery must not reach the pump while a wait covering the run is in flight"
        );
        assert!(delivered.lock().await.is_empty());

        // The wait renders the value and returns.
        claim.answered(&run);
        drop(claim);

        let delivery = tokio::time::timeout(Duration::from_secs(5), deliver)
            .await
            .expect("releasing the claim wakes the held delivery");
        assert!(matches!(delivery, CompletionDelivery::Delivered(_)));
        assert!(
            delivered.lock().await.is_empty(),
            "the value reached the orchestrator on the wait channel; the duplicate is suppressed"
        );
    }

    /// The other half of the same race, and the reason the hold-off is not simply a drop: a wait
    /// that holds a claim and then releases it WITHOUT answering must let the held delivery
    /// through, not swallow it.
    #[tokio::test]
    async fn a_held_delivery_is_released_and_delivered_when_the_wait_answers_nothing() {
        let ledger = InlineAnswerLedger::default();
        let (sink, delivered) = wired(&ledger);
        let run = RunId::from_token("run-rc4-timeout");

        let claim = ledger.claim(std::slice::from_ref(&run));
        let deliver = sink.deliver(&run, suppressible("run-rc4-timeout", "VALUE"));
        tokio::pin!(deliver);
        assert!(
            tokio::time::timeout(Duration::from_millis(150), &mut deliver)
                .await
                .is_err()
        );

        drop(claim); // the wait timed out

        tokio::time::timeout(Duration::from_secs(5), deliver)
            .await
            .expect("the release wakes the held delivery");
        assert_eq!(
            delivered.lock().await.len(),
            1,
            "nobody answered this run, so its notification must arrive"
        );
    }

    // ---------------------------------------------------------------------------------------
    // Never suppress, never park
    // ---------------------------------------------------------------------------------------

    /// A delivery-failure report carries information no `wait` could ever have surfaced. The early
    /// return must fire BEFORE both the hold-off and `take_answered` — the trailing assertion is
    /// what pins that ordering: a mis-placed check would burn the one-use authority here and let a
    /// genuinely duplicate notification through later.
    #[tokio::test]
    async fn a_delivery_failure_report_is_never_suppressed_and_never_burns_the_authority() {
        let ledger = InlineAnswerLedger::default();
        let (sink, delivered) = wired(&ledger);
        let run = RunId::from_token("run-undeliverable");

        let mut claim = ledger.claim(std::slice::from_ref(&run));
        claim.answered(&run);
        drop(claim);

        let delivery = sink
            .deliver(&run, undeliverable("run-undeliverable", "VALUE"))
            .await;
        assert!(matches!(delivery, CompletionDelivery::Delivered(_)));
        assert_eq!(
            delivered.lock().await.len(),
            1,
            "a delivery-failure report must always reach the session"
        );
        assert!(
            ledger.take_answered(&run),
            "the non-suppressible path must not consume the inline answer"
        );
    }

    /// A claim that somehow outlived every `Drop` must degrade to "deliver normally" rather than
    /// park a delivery task forever.
    ///
    /// Virtual time: [`InlineAnswerLedger::claim_released`] awaits a `watch` generation, NOT a
    /// timer, so the runtime goes idle and tokio auto-advances straight to
    /// [`INLINE_CLAIM_MAX_WAIT`] — 30 real minutes in ~0 wall-clock.
    #[tokio::test(start_paused = true)]
    async fn a_leaked_claim_degrades_to_delivering_normally_instead_of_parking_forever() {
        let ledger = InlineAnswerLedger::default();
        let (sink, delivered) = wired(&ledger);
        let run = RunId::from_token("run-leaked-claim");

        // Held for the whole test and never answered — the failure this bound exists for.
        let _claim = ledger.claim(std::slice::from_ref(&run));
        assert!(ledger.is_claimed(&run));

        let delivery = sink
            .deliver(&run, suppressible("run-leaked-claim", "VALUE"))
            .await;
        assert!(matches!(delivery, CompletionDelivery::Delivered(_)));
        assert_eq!(
            delivered.lock().await.len(),
            1,
            "past INLINE_CLAIM_MAX_WAIT the delivery must go through, not park"
        );
    }

    // ---------------------------------------------------------------------------------------
    // Bounded by construction
    // ---------------------------------------------------------------------------------------

    /// Two concurrent waits — the `wait` tool and the headless auto-drain — can cover the same run.
    /// One guard's drop must not release the other's claim, or the surviving wait's delivery stops
    /// being held off half-way through.
    #[test]
    fn two_waits_covering_the_same_run_refcount_the_claim() {
        let ledger = InlineAnswerLedger::default();
        let run = RunId::from_token("run-two-waits");

        let first = ledger.claim(std::slice::from_ref(&run));
        let second = ledger.claim(std::slice::from_ref(&run));
        assert!(ledger.is_claimed(&run));

        drop(first);
        assert!(
            ledger.is_claimed(&run),
            "one guard's drop must not release the other wait's claim"
        );

        drop(second);
        assert!(!ledger.is_claimed(&run));
    }

    /// [`InlineAnswerClaim::answered`] is idempotent per id, so a wait that marks the same run
    /// twice still authorises exactly one suppression.
    #[test]
    fn marking_the_same_run_answered_twice_authorises_one_suppression() {
        let ledger = InlineAnswerLedger::default();
        let run = RunId::from_token("run-idempotent");

        let mut claim = ledger.claim(std::slice::from_ref(&run));
        claim.answered(&run);
        claim.answered(&run);
        drop(claim);

        assert!(ledger.take_answered(&run));
        assert!(!ledger.take_answered(&run));
    }

    /// Both maps are bounded by construction.
    ///
    /// Driven with a FUTURE `now` rather than a back-dated `Instant`: [`CLAIM_HARD_TTL`] is 30m1s
    /// and `Instant` is boot-relative, so `Instant::now() - CLAIM_HARD_TTL` panics outright on a
    /// host with under 31 minutes of uptime (routine in CI containers). The boundaries are walked
    /// in TTL order ([`DEDUP_TTL`] 10m < [`CLAIM_HARD_TTL`] 30m1s), which is also the order
    /// production hits them.
    #[test]
    fn both_ledger_maps_are_pruned_at_their_own_ttl() {
        let run = RunId::from_token("run-stale");
        let now = Instant::now();
        let mut state = LedgerState::default();
        state.claimed.insert(run.clone(), (1, now));
        state.answered.insert(run.clone(), now);

        InlineAnswerLedger::prune(&mut state, now + Duration::from_secs(1));
        assert!(
            state.claimed.contains_key(&run),
            "inside both TTLs, nothing is evicted"
        );
        assert!(state.answered.contains_key(&run));

        InlineAnswerLedger::prune(&mut state, now + DEDUP_TTL + Duration::from_secs(1));
        assert!(
            !state.answered.contains_key(&run),
            "an answer past DEDUP_TTL can no longer authorise a suppression"
        );
        assert!(
            state.claimed.contains_key(&run),
            "DEDUP_TTL (10m) must not evict a claim bounded by CLAIM_HARD_TTL (30m1s)"
        );

        InlineAnswerLedger::prune(&mut state, now + CLAIM_HARD_TTL + Duration::from_secs(1));
        assert!(
            !state.claimed.contains_key(&run),
            "a claim that outlived every Drop is hard-pruned rather than pinning a delivery"
        );
    }
}
