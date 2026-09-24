//! SUBA-017 — the completion lead-in debounce: a burst of sibling background successes becomes ONE
//! grouped `subagent-notify` message instead of one per run.
//!
//! Ports `pi-subagents/src/runs/background/completion-batcher.ts` (168 L, byte-identical at v0.43.0
//! and v0.68.0) and the batching half of `notify.ts` (`formatGroupedCompletion` `:472-489`,
//! `sendCompletion` `:524-542`, the routing at `:780-791`, the ownership re-check at `:708-721`,
//! all @v0.68.0).
//!
//! # Why this exists when the session pump already merges
//!
//! The session's injection pump (`cyrup-session-svc`, `merge_injection_batch`) coalesces whatever is
//! ALREADY queued into one turn, so cyrup never paid N turns for N completions. What it could not
//! do is wait: the first completion of a burst reached an idle parent while its siblings were still
//! being scanned, and took a turn alone. This is the lead-in wait.
//!
//! # Where it sits, and why there
//!
//! `InlineAnsweredSink(BatchingHostServicesCompletionSink)` — BELOW the inline-answer decorator and
//! AT the injector (`.flux/todo/COMPLETION_BATCHING.md` §4a):
//!
//! - below the decorator, so a run a live `wait` has claimed never enters a group (it would hold its
//!   siblings hostage for a whole wait timeout, or be announced twice);
//! - at the injector, because only the synchronous [`HostServices::inject_message_ack`] enqueue can
//!   ORDER a failure behind the held successes it flushes. [`CompletionSink::deliver`] fuses enqueue
//!   and ack-await into one future, so a batcher wrapping it could either wait out the group's ack
//!   (delaying the failure by a whole parent turn) or race it;
//! - never ahead of Phase-1 observation (`watch/install.rs`), so `wait`'s completion-bus wake and the
//!   wait-subscription reconcile are NOT delayed by the window. Upstream's are
//!   (`result-watcher.ts:553` awaits delivery before `:589` emits the event); cyrup keeps that edge.
//!
//! # The four guarantees of the shell
//!
//! 1. **Order across group and bypass.** A failure (or a loss notice) flushes its key's held group
//!    and enqueues itself under ONE lock acquisition, as two synchronous `inject_message_ack` calls.
//! 2. **A completion arriving during a flush opens a fresh group.** The group is swapped out under
//!    the lock before anything is enqueued ([`BatchState::flush`], upstream `emitGroup`'s
//!    `pending = []` before `emit`).
//! 3. **No lock across an await.** The lock is a `std::sync::Mutex`; the only awaits are on a
//!    member's own oneshot and on the group ack, both outside it. The group ack can take a whole
//!    parent turn — the pump answers only on `Taken`.
//! 4. **No stale flush.** The flusher holds a [`Weak`] to the sink and skips every member whose
//!    caller is gone (`CompletionWatcherHandle::drop` aborts the delivery fleet, closing their
//!    receivers); ownership is re-checked per member at flush, as upstream re-checks at emit
//!    (`notify.ts:708-721`), so a session switch inside the window defers the held items rather
//!    than injecting them into the next session. A deferred member's payload stays on disk and is
//!    redelivered by the next watcher, which is upstream's `dispose` → `false` → file stays.
//!
//! Members get their receipts only after the ONE group ack, each minted for its own run
//! ([`CompletionDelivery::delivered`]) — sinks are the receipt authority (`sink.rs`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use cyrup_ext::host::{HostServices, InjectOutcome};
use serde_json::Value;
use tokio::time::Instant;

use super::message::CompletionMessage;
use super::results_watcher::RESULTS_DIR_POLL_INTERVAL;
use super::sink::CompletionSink;
use crate::background::RunId;
use crate::background::delivery::{CompletionDelivery, ResultDeliveryOwnership};
use crate::identity::{CompletionOwnerId, SessionId};

// =================================================================================================
// Config
// =================================================================================================

/// Upstream's `DEFAULT_COMPLETION_BATCH_CONFIG` (`completion-batcher.ts:29-36`), in milliseconds:
/// `debounceMs`, `maxWaitMs`, `stragglerDebounceMs`, `stragglerMaxWaitMs`, `stragglerWindowMs`.
pub const UPSTREAM_DEFAULT_MS: [u64; 5] = [150, 1000, 75, 400, 2000];

/// The resolved `completionBatch` config (pi `ResolvedCompletionBatchConfig`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletionBatchConfig {
    /// `false` restores one notification per completion (pi `enabled`).
    pub enabled: bool,
    /// Quiet time after the latest push before a group emits (pi `debounceMs`).
    pub debounce: Duration,
    /// Hard cap from a group's FIRST push (pi `maxWaitMs`).
    pub max_wait: Duration,
    /// [`Self::debounce`] for a straggler group (pi `stragglerDebounceMs`).
    pub straggler_debounce: Duration,
    /// [`Self::max_wait`] for a straggler group (pi `stragglerMaxWaitMs`).
    pub straggler_max_wait: Duration,
    /// A group that opens within this long of the last emit is a straggler group (pi
    /// `stragglerWindowMs`).
    pub straggler_window: Duration,
}

impl CompletionBatchConfig {
    /// cyrup's default.
    ///
    /// [CYRUP-DELTA] Upstream's timers (150/1000/75/400) assume a completion reaches the batcher
    /// within milliseconds of its result file landing (`fs.watch` plus a 50 ms coalescer,
    /// `result-watcher.ts:632-636`). cyrup's results watcher POLLS every
    /// [`RESULTS_DIR_POLL_INTERVAL`] (500 ms), so arrivals are quantised to 500 ms buckets: two
    /// siblings that finish 10 ms apart but either side of a poll reach the batcher 500 ms apart,
    /// long after a 150 ms debounce has already emitted the first alone. Keeping upstream's numbers
    /// would look like parity and group almost nothing. So each of the four TIMERS is upstream's
    /// value shifted by exactly one poll quantum — the window is still upstream's window, measured
    /// from the poll that could still deliver a sibling: debounce 650 ms, max-wait 1500 ms,
    /// straggler debounce 575 ms, straggler max-wait 900 ms. The straggler WINDOW (2000 ms) is a
    /// look-back from an emit, not a wait for an arrival, so it is upstream's unchanged. Derived
    /// from the real constant, so a change to the poll interval moves these with it. A user-set
    /// `completionBatch` field is honoured exactly ([`resolve_completion_batch_config`]).
    pub const DEFAULT: Self = {
        let quantum = RESULTS_DIR_POLL_INTERVAL.as_millis() as u64;
        Self {
            enabled: true,
            debounce: Duration::from_millis(UPSTREAM_DEFAULT_MS[0] + quantum),
            max_wait: Duration::from_millis(UPSTREAM_DEFAULT_MS[1] + quantum),
            straggler_debounce: Duration::from_millis(UPSTREAM_DEFAULT_MS[2] + quantum),
            straggler_max_wait: Duration::from_millis(UPSTREAM_DEFAULT_MS[3] + quantum),
            straggler_window: Duration::from_millis(UPSTREAM_DEFAULT_MS[4]),
        }
    };
}

impl Default for CompletionBatchConfig {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// pi `parsePositiveInt` (`completion-batcher.ts:38-41`): a finite INTEGER number ≥ 1, else
/// nothing. A JSON `5.0` is an integer to JS (`Number.isInteger(5.0)`), so it is accepted; `1.5`,
/// `0`, `-5`, `"150"` and `NaN` are not.
fn parse_positive_int(value: Option<&Value>) -> Option<u64> {
    let number = value?;
    if let Some(n) = number.as_u64() {
        return (n >= 1).then_some(n);
    }
    let float = number.as_f64()?;
    (float.is_finite() && float.fract() == 0.0 && float >= 1.0 && float <= u64::MAX as f64)
        .then_some(float as u64)
}

/// pi `resolveCompletionBatchConfig(globalConfig)` (`completion-batcher.ts:43-59`), over the RAW
/// `completionBatch` value [`crate::registration::SubagentExtensionConfig::completion_batch`]
/// carries. Lenient PER FIELD, exactly as upstream: an invalid field falls back to its own default
/// and never takes its siblings with it; `enabled` must be a real boolean; a non-object value is
/// ignored whole. Upstream reports none of this (the resolver is silent), so neither does cyrup
/// beyond a `debug` trace naming the field that fell back.
#[must_use]
pub fn resolve_completion_batch_config(raw: Option<&Value>) -> CompletionBatchConfig {
    let defaults = CompletionBatchConfig::DEFAULT;
    let Some(object) = raw.and_then(Value::as_object) else {
        if raw.is_some_and(|value| !value.is_null()) {
            tracing::debug!(target: "subagent_notify", "completionBatch is not an object; using defaults");
        }
        return defaults;
    };
    let field = |key: &str, fallback: Duration| -> Duration {
        match object.get(key) {
            None => fallback,
            Some(value) => match parse_positive_int(Some(value)) {
                Some(ms) => Duration::from_millis(ms),
                None => {
                    tracing::debug!(target: "subagent_notify", field = key, "completionBatch field is not a positive integer; using its default");
                    fallback
                }
            },
        }
    };
    CompletionBatchConfig {
        enabled: object
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(defaults.enabled),
        debounce: field("debounceMs", defaults.debounce),
        max_wait: field("maxWaitMs", defaults.max_wait),
        straggler_debounce: field("stragglerDebounceMs", defaults.straggler_debounce),
        straggler_max_wait: field("stragglerMaxWaitMs", defaults.straggler_max_wait),
        straggler_window: field("stragglerWindowMs", defaults.straggler_window),
    }
}

// =================================================================================================
// The state machine (pure: no tokio runtime, injected instants)
// =================================================================================================

/// pi `createCompletionBatcher`'s state (`completion-batcher.ts:116-167`) as a value: one open group
/// at most, its two timers as deadlines, and the straggler memory.
#[derive(Debug)]
pub struct BatchState<T> {
    pending: Vec<T>,
    debounce_at: Option<Instant>,
    max_wait_at: Option<Instant>,
    straggler: bool,
    last_emit_at: Option<Instant>,
}

impl<T> Default for BatchState<T> {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            debounce_at: None,
            max_wait_at: None,
            straggler: false,
            last_emit_at: None,
        }
    }
}

impl<T> BatchState<T> {
    /// pi `push` (`:143-159`): the straggler flag is decided when the group OPENS; every push
    /// resets the debounce; the max-wait is armed once per group.
    pub fn push(&mut self, item: T, now: Instant, config: &CompletionBatchConfig) {
        if self.pending.is_empty() {
            self.straggler = self
                .last_emit_at
                .is_some_and(|at| now.saturating_duration_since(at) < config.straggler_window);
        }
        self.pending.push(item);
        let (debounce, max_wait) = if self.straggler {
            (config.straggler_debounce, config.straggler_max_wait)
        } else {
            (config.debounce, config.max_wait)
        };
        self.debounce_at = Some(now + debounce);
        if self.max_wait_at.is_none() {
            self.max_wait_at = Some(now + max_wait);
        }
    }

    /// When the open group is due: the earlier of its two timers. `None` with nothing held.
    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        match (self.debounce_at, self.max_wait_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The group, if a timer has fired by `now` — emitted (swapped out) as upstream's timer does.
    pub fn due(&mut self, now: Instant) -> Option<Vec<T>> {
        if self.deadline().is_some_and(|deadline| deadline <= now) {
            let group = self.flush(now);
            (!group.is_empty()).then_some(group)
        } else {
            None
        }
    }

    /// pi `emitGroup` (`:133-140`) / `flush`: clear both timers, swap the group out, and stamp the
    /// emit instant — unless nothing was held, which stamps nothing (upstream returns early).
    pub fn flush(&mut self, now: Instant) -> Vec<T> {
        self.debounce_at = None;
        self.max_wait_at = None;
        if self.pending.is_empty() {
            return Vec::new();
        }
        self.last_emit_at = Some(now);
        std::mem::take(&mut self.pending)
    }

    /// pi `dispose` (`:161-166`): clear the timers and hand back what was never emitted, WITHOUT
    /// emitting it.
    pub fn dispose(&mut self) -> Vec<T> {
        self.debounce_at = None;
        self.max_wait_at = None;
        std::mem::take(&mut self.pending)
    }

    /// Whether anything is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

// =================================================================================================
// The grouped notice (pi `formatGroupedCompletion`)
// =================================================================================================

/// What a completion contributes to a GROUPED notice — built by
/// [`super::message::format_completion_message`] from the same fields its single-notice text uses,
/// so the grouped renderer never re-parses `content`. Present only on a message the batcher may
/// hold: an ordinary, `completed` background success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupPart {
    /// The run's agent (`unknown` when empty), pi `detail.agent`.
    pub agent: String,
    /// `(label, id)` of the schedule that fired the run — label is the name, else the id.
    pub schedule: Option<(String, String)>,
    /// The display summary, `(no output)` when empty — pi `formatResultPreview`.
    pub preview: String,
    /// The session line as upstream re-renders it (`formatSessionLine`): the label LOWERCASED by
    /// `parseSubagentNotifyContent`, then `: <value>`.
    pub session_line: Option<String>,
    /// The session the result belongs to — the batch key and the ownership re-check's input.
    pub session_id: Option<SessionId>,
    /// The completion owner the result names — the ownership re-check's other input.
    pub owner_id: Option<CompletionOwnerId>,
}

/// pi `formatGroupedCompletion` (`notify.ts:472-489` @v0.68.0), over the fields cyrup carries.
/// `taskInfo` has no separate field here: cyrup folds a child's position into the summary itself.
#[must_use]
pub fn format_grouped_completion(parts: &[GroupPart]) -> String {
    let header = format!(
        "Background tasks completed ({}): {}",
        parts.len(),
        parts
            .iter()
            .map(|part| format!("**{}**", part.agent))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut blocks: Vec<String> = vec![header, String::new()];
    for (index, part) in parts.iter().enumerate() {
        let schedule = part
            .schedule
            .as_ref()
            .map(|(label, id)| format!(" — scheduled run from {label} (schedule {id})"))
            .unwrap_or_default();
        blocks.push(format!("{}. {}{schedule}", index + 1, part.agent));
        blocks.push(part.preview.clone());
        if let Some(line) = &part.session_line {
            blocks.push(line.clone());
        }
        blocks.push(String::new());
    }
    blocks.join("\n").trim_end().to_string()
}

/// pi `completionBatchKey` (`notify.ts:544-549`): `session:<id>`, else `cwd:<cwd>`, else
/// `unknown`.
pub(crate) fn batch_key(session: Option<&SessionId>, cwd: Option<&std::path::Path>) -> String {
    if let Some(session) = session.filter(|session| !session.as_str().trim().is_empty()) {
        return format!("session:{}", session.as_str().trim());
    }
    match cwd.map(|cwd| cwd.display().to_string()) {
        Some(cwd) if !cwd.trim().is_empty() => format!("cwd:{}", cwd.trim()),
        _ => "unknown".to_string(),
    }
}

// =================================================================================================
// The shell
// =================================================================================================

/// One held completion.
struct Member {
    run_id: RunId,
    part: GroupPart,
    /// The single-notice text, used verbatim when the group turns out to hold only this member
    /// (pi `formatSingleCompletion` for a one-item group).
    content: String,
    display: bool,
    trigger_turn: bool,
    reply: tokio::sync::oneshot::Sender<CompletionDelivery>,
}

/// One key's group and its flusher's wake-up.
struct Group {
    state: BatchState<Member>,
    /// Wakes this key's flusher when a push may have moved the deadline EARLIER than the one it is
    /// sleeping on (a fresh straggler group after a bypass flush).
    wake: Arc<tokio::sync::Notify>,
    /// Whether a flusher task is alive for this key.
    flusher: bool,
}

struct Shared {
    services: Arc<dyn HostServices>,
    config: CompletionBatchConfig,
    ownership: ResultDeliveryOwnership,
    groups: Mutex<HashMap<String, Group>>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Group>> {
        self.groups
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Enqueue one emitted group — CALLED UNDER THE LOCK, and synchronous: members whose caller is
    /// gone are skipped, ownership is re-checked per member, the survivors go out as ONE message,
    /// and one spawned task fans the ack out into per-member receipts.
    fn inject_group(&self, members: Vec<Member>) {
        let snapshot = self.ownership.snapshot();
        let mut accepted: Vec<Member> = Vec::new();
        for member in members {
            if member.reply.is_closed() {
                // Its delivery task was aborted (watcher teardown): the payload is still on disk
                // and the next watcher delivers it. Injecting it here would announce it twice.
                continue;
            }
            let owned = member
                .part
                .session_id
                .as_ref()
                .is_some_and(|session| snapshot.owns(session, member.part.owner_id.as_ref()));
            if owned {
                accepted.push(member);
            } else {
                tracing::debug!(target: "subagent_notify", run_id = %member.run_id, "emit_not_owned");
                let _ = member.reply.send(CompletionDelivery::Deferred);
            }
        }
        if accepted.is_empty() {
            return;
        }
        let content = match accepted.as_slice() {
            [only] => only.content.clone(),
            many => format_grouped_completion(
                &many
                    .iter()
                    .map(|member| member.part.clone())
                    .collect::<Vec<_>>(),
            ),
        };
        let display = accepted.iter().any(|member| member.display);
        let trigger_turn = accepted.iter().any(|member| member.trigger_turn);
        let replies: Vec<(RunId, tokio::sync::oneshot::Sender<CompletionDelivery>)> = accepted
            .into_iter()
            .map(|member| (member.run_id, member.reply))
            .collect();
        match self.services.inject_message_ack(
            &content,
            Some(GROUP_CUSTOM_TYPE),
            display,
            None,
            trigger_turn,
        ) {
            Ok(ack) => {
                tokio::spawn(async move {
                    let accepted = matches!(ack.await, Ok(InjectOutcome::Accepted));
                    tracing::debug!(
                        target: "subagent_notify",
                        members = replies.len(),
                        "{}",
                        if accepted { "send_accepted" } else { "send_failed" }
                    );
                    for (run_id, reply) in replies {
                        let _ = reply.send(if accepted {
                            CompletionDelivery::delivered(run_id)
                        } else {
                            CompletionDelivery::Deferred
                        });
                    }
                });
            }
            Err(_) => {
                for (_, reply) in replies {
                    let _ = reply.send(CompletionDelivery::Deferred);
                }
            }
        }
    }
}

/// The custom type every completion notice carries (pi `customType: "subagent-notify"`).
const GROUP_CUSTOM_TYPE: &str = "subagent-notify";

/// The batching, turn-injecting completion sink: [`super::sink::HostServicesCompletionSink`] with
/// upstream's lead-in debounce in front of the enqueue. See the module doc.
pub struct BatchingHostServicesCompletionSink {
    shared: Arc<Shared>,
}

impl BatchingHostServicesCompletionSink {
    /// Build over the live capability backend, the resolved config, and the SAME ownership handle
    /// the watcher consumes against (clones share state, so a session switch is seen by both).
    #[must_use]
    pub fn new(
        services: Arc<dyn HostServices>,
        config: CompletionBatchConfig,
        ownership: ResultDeliveryOwnership,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                services,
                config,
                ownership,
                groups: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Inject one message on its own — the disabled path and the bypass path's own item.
    fn inject_single(
        &self,
        message: &CompletionMessage,
    ) -> Option<tokio::sync::oneshot::Receiver<InjectOutcome>> {
        self.shared
            .services
            .inject_message_ack(
                &message.content,
                Some(message.custom_type.as_str()),
                message.display,
                None,
                message.trigger_turn,
            )
            .ok()
    }

    /// The one flusher per key: sleeps until the group is due (re-reading the deadline whenever a
    /// push wakes it), emits, and exits once the group is empty. Holds only a [`Weak`], so it
    /// never keeps a torn-down sink alive and can never flush for one.
    fn spawn_flusher(shared: &Arc<Shared>, key: String, wake: Arc<tokio::sync::Notify>) {
        let weak: Weak<Shared> = Arc::downgrade(shared);
        tokio::spawn(async move {
            loop {
                let deadline = {
                    let Some(shared) = weak.upgrade() else { return };
                    let mut groups = shared.lock();
                    let Some(group) = groups.get_mut(&key) else {
                        return;
                    };
                    match group.state.deadline() {
                        Some(deadline) => deadline,
                        None => {
                            group.flusher = false;
                            return;
                        }
                    }
                };
                tokio::select! {
                    () = tokio::time::sleep_until(deadline) => {}
                    () = wake.notified() => continue,
                }
                let Some(shared) = weak.upgrade() else { return };
                let mut groups = shared.lock();
                if let Some(group) = groups.get_mut(&key)
                    && let Some(members) = group.state.due(Instant::now())
                {
                    shared.inject_group(members);
                }
            }
        });
    }
}

#[async_trait::async_trait]
impl CompletionSink for BatchingHostServicesCompletionSink {
    async fn deliver(&self, run_id: &RunId, message: CompletionMessage) -> CompletionDelivery {
        let config = self.shared.config;
        // pi `if (!config.enabled) emit([item])` — one notification per completion, no flush.
        if !config.enabled {
            return await_single(self.inject_single(&message), run_id).await;
        }
        let Some(part) = message.group_part.clone().filter(|_| message.suppressible) else {
            // BYPASS (pi `:785-788`): a failure, a pause, a stop, or a loss notice. Flush this
            // key's held successes FIRST and enqueue this item SECOND, under one lock, so the
            // pump's insertion-ordered merge keeps that order in the turn. A keyless item (a
            // missing-payload notice, whose report has no session) flushes every held group:
            // shortening another group's hold costs nothing, and delaying behind it is forbidden.
            let receiver = {
                let mut groups = self.shared.lock();
                let key = message.batch_key.clone();
                let now = Instant::now();
                let flushed: Vec<Vec<Member>> = groups
                    .iter_mut()
                    .filter(|(group_key, _)| key.as_ref().is_none_or(|key| key == *group_key))
                    .map(|(_, group)| group.state.flush(now))
                    .filter(|members| !members.is_empty())
                    .collect();
                for members in flushed {
                    self.shared.inject_group(members);
                }
                self.inject_single(&message)
            };
            return await_single(receiver, run_id).await;
        };
        // HELD (pi `batcher.push(item)`, traced `batch_deferred`).
        tracing::debug!(target: "subagent_notify", %run_id, "batch_deferred");
        let key = message
            .batch_key
            .clone()
            .unwrap_or_else(|| batch_key(part.session_id.as_ref(), None));
        let (reply, receiver) = tokio::sync::oneshot::channel();
        {
            let mut groups = self.shared.lock();
            let group = groups.entry(key.clone()).or_insert_with(|| Group {
                state: BatchState::default(),
                wake: Arc::new(tokio::sync::Notify::new()),
                flusher: false,
            });
            group.state.push(
                Member {
                    run_id: run_id.clone(),
                    part,
                    content: message.content,
                    display: message.display,
                    trigger_turn: message.trigger_turn,
                    reply,
                },
                Instant::now(),
                &config,
            );
            if group.flusher {
                group.wake.notify_one();
            } else {
                group.flusher = true;
                Self::spawn_flusher(&self.shared, key, Arc::clone(&group.wake));
            }
        }
        // A dropped sender (the sink torn down with this member still held) is a deferral: the
        // payload stays on disk for the next watcher.
        receiver.await.unwrap_or(CompletionDelivery::Deferred)
    }
}

/// Await one directly-injected message's ack, as [`super::sink::HostServicesCompletionSink`] does.
async fn await_single(
    receiver: Option<tokio::sync::oneshot::Receiver<InjectOutcome>>,
    run_id: &RunId,
) -> CompletionDelivery {
    let Some(receiver) = receiver else {
        return CompletionDelivery::Deferred;
    };
    match receiver.await {
        Ok(InjectOutcome::Accepted) => CompletionDelivery::delivered(run_id.clone()),
        Ok(InjectOutcome::SessionUnavailable) | Err(_) => CompletionDelivery::Deferred,
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
    //! SUBA-017. Every test names the mutation that kills it. The pure core runs on injected
    //! instants; the shell on paused tokio time with a recording `HostServices` whose acks the test
    //! controls.

    use super::super::message::{
        format_completion_message, format_missing_payload_message, format_undeliverable_message,
    };
    use super::super::tests::{child_result, result_with_children, test_owner, test_session};
    use super::*;
    use crate::background::{RunState, ScheduleOrigin};

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Upstream's own numbers, for porting upstream's timing tests verbatim.
    fn upstream() -> CompletionBatchConfig {
        CompletionBatchConfig {
            enabled: true,
            debounce: ms(150),
            max_wait: ms(1000),
            straggler_debounce: ms(75),
            straggler_max_wait: ms(400),
            straggler_window: ms(2000),
        }
    }

    // ---------------------------------------------------------------------------------------
    // The pure state machine (ports of `completion-batcher.test.ts`)
    // ---------------------------------------------------------------------------------------

    /// #1 — three pushes inside the debounce emit as ONE group, at the debounce after the LAST.
    /// Kills: emitting on every push; not resetting the debounce on push.
    #[test]
    fn a_burst_inside_the_debounce_emits_one_group() {
        let cfg = upstream();
        let t0 = Instant::now();
        let mut state = BatchState::default();
        state.push("a", t0, &cfg);
        state.push("b", t0 + ms(100), &cfg);
        state.push("c", t0 + ms(200), &cfg);
        assert_eq!(
            state.due(t0 + ms(300)),
            None,
            "debounce was reset by the last push"
        );
        assert_eq!(state.due(t0 + ms(350)), Some(vec!["a", "b", "c"]));
        assert!(state.is_empty());
    }

    /// #2 — `completion-batcher.test.ts:114`: pushes every 100 ms keep resetting the debounce, but
    /// the max-wait from the FIRST push emits at 1000 ms. Kills: re-arming max-wait on every push.
    #[test]
    fn the_max_wait_cap_emits_while_items_keep_arriving() {
        let cfg = upstream();
        let t0 = Instant::now();
        let mut state = BatchState::default();
        for i in 0..10u64 {
            state.push(i, t0 + ms(i * 100), &cfg);
            assert_eq!(state.due(t0 + ms(i * 100)), None);
        }
        assert_eq!(state.due(t0 + ms(999)), None);
        assert_eq!(state.due(t0 + ms(1000)).map(|g| g.len()), Some(10));
    }

    /// #3 — `:182`: a sibling inside the straggler window after an emit opens a STRAGGLER group,
    /// governed by the shorter timers. Kills: `straggler` hard-wired `false`.
    #[test]
    fn a_sibling_inside_the_straggler_window_uses_straggler_timers() {
        let cfg = upstream();
        let t0 = Instant::now();
        let mut state = BatchState::default();
        state.push("first", t0, &cfg);
        assert_eq!(state.due(t0 + ms(150)), Some(vec!["first"]));
        state.push("late", t0 + ms(500), &cfg);
        assert_eq!(state.due(t0 + ms(574)), None);
        assert_eq!(
            state.due(t0 + ms(575)),
            Some(vec!["late"]),
            "75 ms straggler debounce"
        );
    }

    /// #4 — `:207`: past the straggler window a fresh NORMAL group starts. Kills: the straggler
    /// flag never recomputed when a group opens (it would stay `true` from the previous group).
    #[test]
    fn after_the_straggler_window_a_fresh_normal_group_starts() {
        let cfg = upstream();
        let t0 = Instant::now();
        let mut state = BatchState::default();
        state.push("first", t0, &cfg);
        assert!(state.due(t0 + ms(150)).is_some());
        state.push("straggler", t0 + ms(300), &cfg);
        assert!(state.due(t0 + ms(375)).is_some());
        // 2000 ms after the last emit (375) the window is closed.
        let t1 = t0 + ms(375) + ms(2000);
        state.push("fresh", t1, &cfg);
        assert_eq!(
            state.due(t1 + ms(75)),
            None,
            "not a straggler group any more"
        );
        assert_eq!(state.due(t1 + ms(150)), Some(vec!["fresh"]));
    }

    /// `dispose` hands back what was held WITHOUT emitting it, and a flush of nothing stamps no
    /// emit instant (so it cannot turn the next group into a straggler group).
    #[test]
    fn dispose_returns_the_held_items_and_an_empty_flush_stamps_nothing() {
        let cfg = upstream();
        let t0 = Instant::now();
        let mut state = BatchState::default();
        assert!(state.flush(t0).is_empty());
        state.push("x", t0 + ms(10), &cfg);
        assert_eq!(
            state.due(t0 + ms(84)),
            None,
            "a normal group: an empty flush is not an emit"
        );
        assert_eq!(state.dispose(), vec!["x"]);
        assert_eq!(state.deadline(), None);
    }

    // ---------------------------------------------------------------------------------------
    // Config resolution
    // ---------------------------------------------------------------------------------------

    /// #6 — `:60`/`:69`: every invalid field falls back to ITS OWN default and a valid sibling is
    /// kept; `enabled` must be a real bool. Kills: accepting `0`, a fraction or the string
    /// `"false"`; failing the whole key on one bad field.
    #[test]
    fn resolve_falls_back_per_field_on_invalid_values() {
        let defaults = CompletionBatchConfig::DEFAULT;
        let resolved = resolve_completion_batch_config(Some(&serde_json::json!({
            "enabled": "false",
            "debounceMs": 0,
            "maxWaitMs": -5,
            "stragglerDebounceMs": 1.5,
            "stragglerMaxWaitMs": "400",
            "stragglerWindowMs": 3000,
        })));
        assert_eq!(
            resolved,
            CompletionBatchConfig {
                straggler_window: ms(3000),
                ..defaults
            }
        );
        assert!(resolved.enabled, "the string \"false\" is not a boolean");
        assert_eq!(
            resolve_completion_batch_config(Some(&serde_json::json!(7))),
            defaults
        );
        assert_eq!(resolve_completion_batch_config(None), defaults);
        assert_eq!(
            resolve_completion_batch_config(Some(&serde_json::json!({"debounceMs": 5.0}))).debounce,
            ms(5),
            "JS Number.isInteger(5.0)"
        );
    }

    /// A user-set value is honoured EXACTLY — including upstream's 150 ms, which cyrup's default
    /// does not use. Kills: clamping a user value to the poll-derived default.
    #[test]
    fn a_user_set_completion_batch_is_honoured_exactly() {
        let resolved = resolve_completion_batch_config(Some(&serde_json::json!({
            "enabled": false,
            "debounceMs": 150,
            "maxWaitMs": 1000,
            "stragglerDebounceMs": 75,
            "stragglerMaxWaitMs": 400,
            "stragglerWindowMs": 2000,
        })));
        assert_eq!(
            resolved,
            CompletionBatchConfig {
                enabled: false,
                ..upstream()
            }
        );
    }

    /// The [CYRUP-DELTA] default: every timer is upstream's plus exactly one results-watcher poll
    /// quantum, derived from the real constant. Kills: reverting to upstream's 150 ms (which can
    /// never group siblings that land in different 500 ms polls), or hard-coding a second number
    /// that drifts from the poll interval.
    #[test]
    fn the_default_window_is_upstreams_shifted_by_one_poll_quantum() {
        let quantum = RESULTS_DIR_POLL_INTERVAL;
        let cfg = CompletionBatchConfig::DEFAULT;
        assert_eq!(cfg.debounce, ms(150) + quantum);
        assert_eq!(cfg.max_wait, ms(1000) + quantum);
        assert_eq!(cfg.straggler_debounce, ms(75) + quantum);
        assert_eq!(cfg.straggler_max_wait, ms(400) + quantum);
        assert_eq!(cfg.straggler_window, ms(2000));
        assert!(
            cfg.debounce > quantum,
            "two siblings one poll apart must fall inside one debounce"
        );
    }

    /// #7 — a malformed `completionBatch` does not take the rest of `config.json` down: the key is
    /// carried RAW and resolved here. Kills: a typed `u64` field in place of the raw value.
    #[test]
    fn a_malformed_completion_batch_key_does_not_fail_the_config_parse() {
        let config: crate::registration::SubagentExtensionConfig =
            serde_json::from_value(serde_json::json!({
                "asyncByDefault": true,
                "completionBatch": { "debounceMs": -5, "enabled": true },
            }))
            .expect("the whole config still parses");
        assert!(config.async_by_default, "a sibling key survived");
        assert_eq!(
            resolve_completion_batch_config(config.completion_batch.as_ref()).debounce,
            CompletionBatchConfig::DEFAULT.debounce
        );
    }

    // ---------------------------------------------------------------------------------------
    // The grouped notice
    // ---------------------------------------------------------------------------------------

    fn part(agent: &str, preview: &str) -> GroupPart {
        GroupPart {
            agent: agent.to_string(),
            schedule: None,
            preview: preview.to_string(),
            session_line: None,
            session_id: Some(test_session()),
            owner_id: Some(test_owner()),
        }
    }

    /// #8 — `notify.test.ts:457`/`:602`: the header names every agent and each block is numbered.
    /// Kills: rendering the group as `"\n\n"`-joined single notices.
    #[test]
    fn grouped_notice_matches_upstream_layout() {
        let text = format_grouped_completion(&[
            part("alpha", "alpha done"),
            part("beta", "beta done"),
            part("gamma", "gamma done"),
        ]);
        assert!(
            text.starts_with(
                "Background tasks completed (3): **alpha**, **beta**, **gamma**\n\n1. alpha\nalpha done\n\n2. beta\nbeta done\n\n3. gamma\ngamma done"
            ),
            "{text}"
        );
        assert!(!text.ends_with('\n'), "trimEnd");
    }

    /// #10 — `notify.test.ts:970`: a scheduled run keeps its attribution inside a group, and the
    /// session line is re-rendered with upstream's lowercased label. Kills: dropping the
    /// `— scheduled run from` suffix.
    #[test]
    fn a_scheduled_run_keeps_its_attribution_in_a_group() {
        let mut result = result_with_children(
            "sched-1",
            RunState::Complete,
            true,
            Some(std::path::PathBuf::from("/s/one.jsonl")),
            vec![child_result("worker", Some("nightly done"), 0)],
        );
        result.schedule_origin = Some(ScheduleOrigin {
            id: "nightly".to_string(),
            name: Some("Nightly sweep".to_string()),
            quiet: None,
        });
        let scheduled = format_completion_message(&result)
            .group_part
            .expect("batchable");
        let text = format_grouped_completion(&[scheduled, part("beta", "beta done")]);
        assert!(
            text.contains("1. worker — scheduled run from Nightly sweep (schedule nightly)\n"),
            "{text}"
        );
        assert!(text.contains("session file: /s/one.jsonl"), "{text}");
    }

    // ---------------------------------------------------------------------------------------
    // The shell
    // ---------------------------------------------------------------------------------------

    #[derive(Debug, Clone, PartialEq)]
    struct Injected {
        content: String,
        display: bool,
        trigger_turn: bool,
    }

    /// A `HostServices` that records every enqueue and holds every ack until the test answers it
    /// (or answers at once with `auto`).
    #[derive(Default)]
    struct Recorder {
        injected: std::sync::Mutex<Vec<Injected>>,
        acks: std::sync::Mutex<Vec<tokio::sync::oneshot::Sender<InjectOutcome>>>,
        auto: std::sync::Mutex<Option<InjectOutcome>>,
    }

    impl Recorder {
        fn auto_accepting() -> Arc<Self> {
            let recorder = Self::default();
            *recorder.auto.lock().unwrap() = Some(InjectOutcome::Accepted);
            Arc::new(recorder)
        }
        fn injected(&self) -> Vec<Injected> {
            self.injected.lock().unwrap().clone()
        }
        fn answer_all(&self, outcome: InjectOutcome) {
            for ack in self.acks.lock().unwrap().drain(..) {
                let _ = ack.send(outcome.clone());
            }
        }
    }

    impl HostServices for Recorder {
        fn inject_message_ack(
            &self,
            content: &str,
            _custom_type: Option<&str>,
            display: bool,
            _details: Option<&Value>,
            trigger_turn: bool,
        ) -> Result<tokio::sync::oneshot::Receiver<InjectOutcome>, String> {
            self.injected.lock().unwrap().push(Injected {
                content: content.to_string(),
                display,
                trigger_turn,
            });
            let (tx, rx) = tokio::sync::oneshot::channel();
            match self.auto.lock().unwrap().clone() {
                Some(outcome) => {
                    let _ = tx.send(outcome);
                }
                None => self.acks.lock().unwrap().push(tx),
            }
            Ok(rx)
        }
    }

    fn success(run: &str, agent: &str, output: &str) -> (RunId, CompletionMessage) {
        let mut result = result_with_children(
            run,
            RunState::Complete,
            true,
            None,
            vec![child_result(agent, Some(output), 0)],
        );
        result.agent = agent.to_string();
        (RunId::from_token(run), format_completion_message(&result))
    }

    fn failure(run: &str, agent: &str) -> (RunId, CompletionMessage) {
        let mut result = result_with_children(
            run,
            RunState::Failed,
            false,
            None,
            vec![child_result(agent, Some("boom"), 1)],
        );
        result.agent = agent.to_string();
        (RunId::from_token(run), format_completion_message(&result))
    }

    fn sink_over(
        recorder: &Arc<Recorder>,
        cfg: CompletionBatchConfig,
    ) -> Arc<BatchingHostServicesCompletionSink> {
        Arc::new(BatchingHostServicesCompletionSink::new(
            Arc::clone(recorder) as Arc<dyn HostServices>,
            cfg,
            super::super::tests::owning(),
        ))
    }

    fn spawn_deliver(
        sink: &Arc<BatchingHostServicesCompletionSink>,
        (run, message): (RunId, CompletionMessage),
    ) -> tokio::task::JoinHandle<CompletionDelivery> {
        let sink = Arc::clone(sink);
        tokio::spawn(async move { sink.deliver(&run, message).await })
    }

    fn delivered_run(delivery: &CompletionDelivery) -> Option<String> {
        match delivery {
            CompletionDelivery::Delivered(receipt) => Some(receipt.run_id().as_str().to_string()),
            CompletionDelivery::Deferred => None,
        }
    }

    /// Let spawned tasks run without moving the paused clock.
    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    /// #11 + #9 — five sibling successes inside the window inject EXACTLY ONE message, hidden
    /// (plain successes, pi's OR'd `display`) and turn-triggering. Kills: the batcher bypassed
    /// (five injects); `display`/`trigger_turn` AND-ed or taken from the first member only.
    #[tokio::test(start_paused = true)]
    async fn five_completions_within_the_window_inject_exactly_one_message() {
        let recorder = Recorder::auto_accepting();
        let sink = sink_over(&recorder, CompletionBatchConfig::DEFAULT);
        let mut handles = Vec::new();
        for i in 0..5 {
            handles.push(spawn_deliver(
                &sink,
                success(&format!("r{i}"), &format!("agent{i}"), "done"),
            ));
            tokio::time::sleep(ms(100)).await;
        }
        assert!(recorder.injected().is_empty(), "still inside the debounce");
        for handle in handles {
            assert!(delivered_run(&handle.await.unwrap()).is_some());
        }
        let injected = recorder.injected();
        assert_eq!(injected.len(), 1, "{injected:?}");
        assert!(
            injected[0]
                .content
                .starts_with("Background tasks completed (5): ")
        );
        assert!(!injected[0].display, "a group of plain successes is hidden");
        assert!(injected[0].trigger_turn);
    }

    /// #9's other half: a displayed (scheduled) member displays the group, a quiet member does not
    /// stop a loud one waking the turn. Kills: AND instead of OR.
    #[tokio::test(start_paused = true)]
    async fn a_groups_display_and_trigger_are_ored() {
        let recorder = Recorder::auto_accepting();
        let sink = sink_over(&recorder, CompletionBatchConfig::DEFAULT);
        let (run_a, mut quiet) = success("q1", "quiet", "done");
        quiet.trigger_turn = false;
        quiet.display = true;
        let a = spawn_deliver(&sink, (run_a, quiet));
        let b = spawn_deliver(&sink, success("q2", "loud", "done"));
        a.await.unwrap();
        b.await.unwrap();
        let injected = recorder.injected();
        assert_eq!(injected.len(), 1);
        assert!(injected[0].display && injected[0].trigger_turn);
    }

    /// #12 — `notify.test.ts:439`: a failure flushes the held success FIRST and goes out
    /// immediately, both before any time passes; nothing arrives later. Kills: pushing the failure
    /// into the group; enqueuing the bypass before the flush.
    #[tokio::test(start_paused = true)]
    async fn a_failure_flushes_held_successes_first_and_is_not_delayed() {
        let recorder = Recorder::auto_accepting();
        let sink = sink_over(&recorder, CompletionBatchConfig::DEFAULT);
        let ok = spawn_deliver(&sink, success("ok-1", "ok-1", "ok-1 done"));
        settle().await;
        let failed = spawn_deliver(&sink, failure("fail-1", "fail-1"));
        settle().await;
        let injected = recorder.injected();
        assert_eq!(
            injected.len(),
            2,
            "both went out with no time advanced: {injected:?}"
        );
        assert!(
            injected[0]
                .content
                .starts_with("Background task completed: **ok-1**")
        );
        assert!(
            injected[1]
                .content
                .starts_with("Background task failed: **fail-1**")
        );
        assert!(delivered_run(&ok.await.unwrap()).is_some());
        assert!(delivered_run(&failed.await.unwrap()).is_some());
        tokio::time::sleep(ms(5000)).await;
        assert_eq!(recorder.injected().len(), 2, "no deferred emission later");
    }

    /// #12's scoping half — `notify.test.ts:493`: ANOTHER session's failure does not flush this
    /// session's group. Kills: a bypass that flushes every group when it has a key.
    #[tokio::test(start_paused = true)]
    async fn another_sessions_failure_does_not_flush_this_group() {
        let recorder = Recorder::auto_accepting();
        let sink = sink_over(&recorder, CompletionBatchConfig::DEFAULT);
        let _held = spawn_deliver(&sink, success("mine", "mine", "done"));
        settle().await;
        let (run, mut other) = failure("theirs", "theirs");
        other.batch_key = Some(batch_key(
            Some(&SessionId::parse("other-session").unwrap()),
            None,
        ));
        assert_ne!(
            other.batch_key,
            format_completion_message(&result_with_children(
                "x",
                RunState::Complete,
                true,
                None,
                Vec::new(),
            ))
            .batch_key
        );
        drop(spawn_deliver(&sink, (run, other)));
        settle().await;
        let injected = recorder.injected();
        assert_eq!(injected.len(), 1, "{injected:?}");
        assert!(
            injected[0]
                .content
                .starts_with("Background task failed: **theirs**")
        );
    }

    /// #13 — loss notices are `suppressible: false` and must never wait in a group. Kills:
    /// routing on outcome alone (an undeliverable report of a COMPLETED run would be held).
    #[tokio::test(start_paused = true)]
    async fn a_loss_notice_bypasses_the_group() {
        let recorder = Recorder::auto_accepting();
        let sink = sink_over(&recorder, CompletionBatchConfig::DEFAULT);
        let result = result_with_children(
            "lost-1",
            RunState::Complete,
            true,
            None,
            vec![child_result("worker", Some("value"), 0)],
        );
        let undeliverable = format_undeliverable_message(&result);
        assert!(
            undeliverable.group_part.is_some(),
            "a completed run: only `suppressible` can route it out"
        );
        drop(spawn_deliver(
            &sink,
            (RunId::from_token("lost-1"), undeliverable),
        ));
        settle().await;
        assert_eq!(
            recorder.injected().len(),
            1,
            "sent at once, not after the debounce"
        );

        let missing = format_missing_payload_message(&super::super::results_watcher::LossReport {
            run_id: RunId::from_token("lost-2"),
            agent: "worker".to_string(),
            async_dir: None,
            recovered: Vec::new(),
            ended_at: None,
        });
        drop(spawn_deliver(&sink, (RunId::from_token("lost-2"), missing)));
        settle().await;
        assert_eq!(recorder.injected().len(), 2);
    }

    /// #14 — members resolve only after the ONE group ack, each with a receipt for its OWN run;
    /// a refused ack defers them all. Kills: minting at push or at enqueue; one receipt reused.
    #[tokio::test(start_paused = true)]
    async fn members_get_their_own_receipts_only_after_the_group_ack() {
        let recorder = Arc::new(Recorder::default());
        let sink = sink_over(&recorder, CompletionBatchConfig::DEFAULT);
        let a = spawn_deliver(&sink, success("run-a", "a", "a done"));
        let b = spawn_deliver(&sink, success("run-b", "b", "b done"));
        tokio::time::sleep(ms(2000)).await;
        assert_eq!(recorder.injected().len(), 1, "the group is enqueued");
        assert!(
            !a.is_finished() && !b.is_finished(),
            "no member resolves before the ack"
        );
        recorder.answer_all(InjectOutcome::Accepted);
        assert_eq!(delivered_run(&a.await.unwrap()).as_deref(), Some("run-a"));
        assert_eq!(delivered_run(&b.await.unwrap()).as_deref(), Some("run-b"));

        let c = spawn_deliver(&sink, success("run-c", "c", "c done"));
        let d = spawn_deliver(&sink, success("run-d", "d", "d done"));
        tokio::time::sleep(ms(2000)).await;
        recorder.answer_all(InjectOutcome::SessionUnavailable);
        assert!(matches!(c.await.unwrap(), CompletionDelivery::Deferred));
        assert!(matches!(d.await.unwrap(), CompletionDelivery::Deferred));
    }

    /// Hazard 2 — a completion that arrives while a flushed group's ack is still pending opens a
    /// FRESH (straggler) group; it neither joins the in-flight message nor waits on its ack.
    /// Kills: emitting without swapping the group out (the late item would re-send the first).
    #[tokio::test(start_paused = true)]
    async fn an_arrival_during_a_flush_opens_a_fresh_group() {
        let recorder = Arc::new(Recorder::default());
        let sink = sink_over(&recorder, CompletionBatchConfig::DEFAULT);
        let first = spawn_deliver(&sink, success("f1", "first", "done"));
        tokio::time::sleep(ms(700)).await;
        assert_eq!(
            recorder.injected().len(),
            1,
            "the first group is out, ack pending"
        );
        let late = spawn_deliver(&sink, success("f2", "late", "done"));
        tokio::time::sleep(ms(700)).await;
        let injected = recorder.injected();
        assert_eq!(injected.len(), 2, "{injected:?}");
        assert!(
            injected[1]
                .content
                .starts_with("Background task completed: **late**")
        );
        recorder.answer_all(InjectOutcome::Accepted);
        assert!(delivered_run(&first.await.unwrap()).is_some());
        assert!(delivered_run(&late.await.unwrap()).is_some());
    }

    /// #15 / hazard 3a — tearing down the watcher mid-window (its delivery tasks aborted, its sink
    /// dropped) releases the sink at once and injects NOTHING, so every payload stays on disk for
    /// the next watcher. Kills: the flusher holding a strong `Arc` (it keeps the dead sink — and
    /// the host services it holds — alive for the rest of the window).
    #[tokio::test(start_paused = true)]
    async fn tearing_down_the_sink_mid_window_injects_nothing() {
        let recorder = Recorder::auto_accepting();
        let sink = sink_over(&recorder, CompletionBatchConfig::DEFAULT);
        let a = spawn_deliver(&sink, success("t1", "a", "done"));
        let b = spawn_deliver(&sink, success("t2", "b", "done"));
        settle().await;
        a.abort();
        b.abort();
        drop(sink);
        settle().await;
        assert_eq!(
            Arc::strong_count(&recorder),
            1,
            "nothing — the flusher included — keeps the torn-down sink alive"
        );
        tokio::time::sleep(ms(5000)).await;
        assert!(recorder.injected().is_empty(), "{:?}", recorder.injected());
    }

    /// Hazard 3b — the callers are aborted but something still holds the sink: their members are
    /// skipped at flush. Kills: not skipping closed members (the aborted runs would be announced
    /// now AND again by the next watcher).
    #[tokio::test(start_paused = true)]
    async fn members_whose_callers_are_gone_are_skipped_at_flush() {
        let recorder = Recorder::auto_accepting();
        let sink = sink_over(&recorder, CompletionBatchConfig::DEFAULT);
        let gone = spawn_deliver(&sink, success("gone", "gone", "done"));
        let kept = spawn_deliver(&sink, success("kept", "kept", "done"));
        settle().await;
        gone.abort();
        settle().await;
        assert!(delivered_run(&kept.await.unwrap()).is_some());
        let injected = recorder.injected();
        assert_eq!(injected.len(), 1);
        assert!(
            injected[0]
                .content
                .starts_with("Background task completed: **kept**"),
            "{injected:?}"
        );
    }

    /// #16 — `notify.test.ts:222`: ownership is re-checked per member AT FLUSH, and a member the
    /// sink's ownership does not cover is DEFERRED (its payload stays for the watcher that does own
    /// it) instead of injected. In cyrup a session switch reinstalls the watcher with a new
    /// ownership and aborts the old callers (covered by the two teardown tests above); this is the
    /// residual the re-check guards — a held member whose session this sink's ownership does not
    /// cover when the group emits. Kills: no ownership re-check at flush.
    #[tokio::test(start_paused = true)]
    async fn a_member_this_session_no_longer_owns_is_deferred_at_flush() {
        let recorder = Recorder::auto_accepting();
        let sink = Arc::new(BatchingHostServicesCompletionSink::new(
            Arc::clone(&recorder) as Arc<dyn HostServices>,
            CompletionBatchConfig::DEFAULT,
            crate::background::delivery::ResultDeliveryOwnership::new(
                Some(SessionId::parse("the-next-session").unwrap()),
                Some(test_owner()),
            ),
        ));
        let held = spawn_deliver(&sink, success("s1", "a", "done"));
        tokio::time::sleep(ms(5000)).await;
        assert!(matches!(held.await.unwrap(), CompletionDelivery::Deferred));
        assert!(recorder.injected().is_empty());
    }

    /// Disabled: one notification per completion, immediately. Kills: `enabled` ignored.
    #[tokio::test(start_paused = true)]
    async fn disabled_config_emits_each_item_immediately() {
        let recorder = Recorder::auto_accepting();
        let sink = sink_over(
            &recorder,
            CompletionBatchConfig {
                enabled: false,
                ..CompletionBatchConfig::DEFAULT
            },
        );
        let a = spawn_deliver(&sink, success("d1", "a", "done"));
        let b = spawn_deliver(&sink, success("d2", "b", "done"));
        a.await.unwrap();
        b.await.unwrap();
        assert_eq!(recorder.injected().len(), 2);
        assert_eq!(tokio::time::Instant::now().elapsed(), Duration::ZERO);
    }

    /// Siblings one POLL apart group under the default — the reason for the [CYRUP-DELTA]. Kills:
    /// upstream's 150 ms debounce as the default (the two would be two messages).
    #[tokio::test(start_paused = true)]
    async fn siblings_in_adjacent_polls_group_under_the_default() {
        let recorder = Recorder::auto_accepting();
        let sink = sink_over(&recorder, CompletionBatchConfig::DEFAULT);
        let a = spawn_deliver(&sink, success("p1", "a", "done"));
        tokio::time::sleep(RESULTS_DIR_POLL_INTERVAL).await;
        let b = spawn_deliver(&sink, success("p2", "b", "done"));
        a.await.unwrap();
        b.await.unwrap();
        let injected = recorder.injected();
        assert_eq!(injected.len(), 1, "{injected:?}");
        assert!(
            injected[0]
                .content
                .starts_with("Background tasks completed (2): ")
        );
    }

    /// #17 — wired exactly as production (`InlineAnswered(Batching(..))`): a run a live `wait`
    /// has claimed never enters a group and does not hold its sibling; once the wait answers it,
    /// it is suppressed. Kills: the wiring order swapped (batcher outside the decorator — the
    /// sibling would wait on the claim, or the claimed run would be announced in the group).
    #[tokio::test(start_paused = true)]
    async fn an_inline_answered_run_never_enters_a_group_and_does_not_hold_siblings() {
        let recorder = Recorder::auto_accepting();
        let ledger = super::super::sink::InlineAnswerLedger::default();
        let wired: Arc<dyn CompletionSink> = Arc::new(super::super::sink::InlineAnsweredSink::new(
            sink_over(&recorder, CompletionBatchConfig::DEFAULT) as Arc<dyn CompletionSink>,
            ledger.clone(),
        ));
        let (run_a, message_a) = success("claimed", "claimed", "A VALUE");
        let mut claim = ledger.claim(std::slice::from_ref(&run_a));
        let a = {
            let wired = Arc::clone(&wired);
            tokio::spawn(async move { wired.deliver(&run_a, message_a).await })
        };
        let b = {
            let wired = Arc::clone(&wired);
            let (run_b, message_b) = success("sibling", "sibling", "B VALUE");
            tokio::spawn(async move { wired.deliver(&run_b, message_b).await })
        };
        assert!(
            delivered_run(&b.await.unwrap()).is_some(),
            "the sibling did not wait on the claim"
        );
        let injected = recorder.injected();
        assert_eq!(injected.len(), 1);
        assert!(
            injected[0]
                .content
                .starts_with("Background task completed: **sibling**")
        );
        assert!(!injected[0].content.contains("A VALUE"));
        claim.answered(&RunId::from_token("claimed"));
        drop(claim);
        assert!(
            delivered_run(&a.await.unwrap()).is_some(),
            "suppressed against a receipt"
        );
        tokio::time::sleep(ms(5000)).await;
        assert_eq!(
            recorder.injected().len(),
            1,
            "the answered run is never announced"
        );
    }

    /// An observer that records every completion it is shown, with a wall-clock stamp.
    #[derive(Default)]
    struct StampingObserver {
        seen: std::sync::Mutex<Vec<(String, std::time::Instant)>>,
    }

    #[async_trait::async_trait]
    impl super::super::observer::CompletionObserver for StampingObserver {
        async fn observe(
            &self,
            notification: &super::super::results_watcher::CompletionNotification,
        ) -> bool {
            self.seen.lock().unwrap().push((
                notification.result.run_id.as_str().to_string(),
                std::time::Instant::now(),
            ));
            true
        }
    }

    /// #18 + #15, through the REAL watcher over a real results dir. Phase-1 observation (the
    /// `CompletionBus` edge `wait` selects on) is NOT delayed by the batch window — cyrup observes
    /// before it delivers, where upstream's async-complete event waits on delivery
    /// (`result-watcher.ts:553`→`:589`). And tearing the watcher down mid-window injects nothing
    /// and leaves the payload on disk for the next watcher. Killing mutations: moving the batching
    /// ahead of observation (the observer would be seen only after the window); a flusher that
    /// flushes for a dropped watcher.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn observation_is_not_delayed_by_the_batch_and_teardown_injects_nothing() {
        let (_dir, results_dir) = super::super::tests::temp_results_dir();
        tokio::fs::create_dir_all(&results_dir).await.unwrap();
        let recorder = Recorder::auto_accepting();
        let window = CompletionBatchConfig {
            debounce: ms(4_000),
            max_wait: ms(8_000),
            ..CompletionBatchConfig::DEFAULT
        };
        let observer = Arc::new(StampingObserver::default());
        let handle = super::super::install::install_completion_watcher_with_observer(
            results_dir.clone(),
            sink_over(&recorder, window) as Arc<dyn CompletionSink>,
            Some(Arc::clone(&observer) as Arc<dyn super::super::observer::CompletionObserver>),
            super::super::tests::owning(),
        )
        .expect("watcher installs");
        let result = result_with_children(
            "observed-1",
            RunState::Complete,
            true,
            None,
            vec![child_result("worker", Some("done"), 0)],
        );
        let payload = super::super::tests::published_path(&results_dir, &result);
        let published = std::time::Instant::now();
        super::super::tests::publish_result(&results_dir, &result).await;

        let deadline = published + Duration::from_secs(3);
        while observer.seen.lock().unwrap().is_empty() {
            assert!(std::time::Instant::now() < deadline, "never observed");
            tokio::time::sleep(ms(25)).await;
        }
        assert!(
            recorder.injected().is_empty(),
            "observed while the notice is still held in the window"
        );

        drop(handle);
        tokio::time::sleep(ms(9_000)).await;
        assert!(recorder.injected().is_empty(), "{:?}", recorder.injected());
        assert!(payload.exists(), "the payload stays for the next watcher");
    }

    /// A live-session backend: a session id (the ownership the watcher consumes against) and an
    /// auto-accepting, recording injector.
    struct SessionRecorder {
        recorder: Arc<Recorder>,
    }

    impl HostServices for SessionRecorder {
        fn session_id(&self) -> Option<String> {
            Some("wiring-session".to_string())
        }
        fn inject_message_ack(
            &self,
            content: &str,
            custom_type: Option<&str>,
            display: bool,
            details: Option<&Value>,
            trigger_turn: bool,
        ) -> Result<tokio::sync::oneshot::Receiver<InjectOutcome>, String> {
            self.recorder
                .inject_message_ack(content, custom_type, display, details, trigger_turn)
        }
    }

    /// #11, at the WIRING: the production `SubagentExecutor::install_completion_watcher`, with a
    /// live host bound, resolves `completionBatch` from the extension config and batches a burst of
    /// two real published completions into ONE injected message. Killing mutations: the host arm
    /// of `effective_completion_sink` left on the per-run `HostServicesCompletionSink` (two
    /// messages); the config key not resolved (the 650 ms default would still group these two, so
    /// the test also asserts the window it configured was the one honoured).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_host_arm_batches_a_burst_through_the_production_install() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let config = crate::registration::SubagentExtensionConfig {
            roots: crate::paths::Roots::sandboxed(home.path()),
            completion_batch: Some(serde_json::json!({ "debounceMs": 2500, "maxWaitMs": 6000 })),
            ..crate::registration::SubagentExtensionConfig::default()
        };
        let executor = crate::extension::SubagentExecutor::with_config(config.clone());
        let recorder = Recorder::auto_accepting();
        executor.set_host_services(Arc::new(SessionRecorder {
            recorder: Arc::clone(&recorder),
        }));
        executor.install_completion_watcher(cwd.path()).await;
        let results_dir =
            crate::background::run_artifact_roots_in(&config.roots, cwd.path()).results_dir;
        for (run, agent) in [("wire-a", "alpha"), ("wire-b", "beta")] {
            let mut result = result_with_children(
                run,
                RunState::Complete,
                true,
                None,
                vec![child_result(agent, Some("done"), 0)],
            );
            result.agent = agent.to_string();
            result.session_id = SessionId::parse("wiring-session");
            result.completion_owner_id = Some(crate::identity::current_completion_owner_id());
            super::super::tests::publish_result(&results_dir, &result).await;
        }
        let published = std::time::Instant::now();
        let deadline = published + Duration::from_secs(12);
        while recorder.injected().is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "nothing was ever injected"
            );
            tokio::time::sleep(ms(50)).await;
        }
        assert!(
            published.elapsed() >= ms(2000),
            "the configured 2.5 s window was honoured, not the default: {:?}",
            published.elapsed()
        );
        tokio::time::sleep(ms(1500)).await;
        let injected = recorder.injected();
        assert_eq!(injected.len(), 1, "{injected:?}");
        assert!(
            injected[0]
                .content
                .starts_with("Background tasks completed (2): "),
            "{injected:?}"
        );
    }
}
