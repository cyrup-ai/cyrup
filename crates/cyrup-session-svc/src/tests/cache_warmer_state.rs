//! Prompt-cache warming: the state machine, its economics and its formatters (SEAM-131).
//!
//! Ported from pi `packages/coding-agent/test/cache-warmer.test.ts` @v1.0.4 (nine `it`s), against
//! `packages/coding-agent/src/core/cache-warmer.ts` @v1.0.4.
//!
//! **Time is virtual.** Every timing test runs under `#[tokio::test(start_paused = true)]` and
//! moves the clock with `tokio::time::advance`, which is this repo's established clock seam (the
//! codemode and herdr suites). Nothing here sleeps for real: the shortest interval the warmer
//! schedules is four and a half minutes and its safety windows are 30 and 60 minutes.
//!
//! The upstream test's two models are reproduced by price, not by name: `adaptive` is
//! `claude-opus-4-6`'s rate card (input 5, output 25, cacheRead 0.5, cacheWrite 6.25 per 1e6) with
//! `compat.forceAdaptiveThinking`, and `budget` is the same card without it. Those four numbers
//! are what make upstream's asserted `missCost ≈ 0.575` and `warmCost ≈ 0.050025` at 100 000
//! prompt tokens come out, so they are asserted here too.

// Tests may unwrap/expect/panic (workspace no-panic policy, `Cargo.toml:143-151`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;

use cyrup_config::CacheWarmingMode;
use cyrup_core::{
    ApiId, AssistantMessage, Content, Cost, EventStream, ModelId, ModelThinkingLevel, ProviderId,
    StopReason, Usage,
};
use cyrup_provider::{
    CacheRetention, Context, Modality, Model, ModelCost, ModelPromptCache, StreamEvent,
    StreamOptions,
};
use cyrup_session::entry::{Entry, EntryBase, KnownEntry};

use crate::cache_warmer::{
    CacheWarmRequest, CacheWarmStream, CacheWarmer, CacheWarmingAction, CacheWarmingDecider,
    CacheWarmingDecision, CacheWarmingDecisionEvent, CacheWarmingHost, CacheWarmingPhase,
    CacheWarmingState, CacheWarmingStatus, IsCurrent, PiDecision, cache_warming_delay_ms,
    format_cache_warming_decision_time, format_cache_warming_status, is_replayable,
    last_prompt_tokens,
};

// ---------------------------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------------------------

fn anthropic_cost() -> ModelCost {
    // claude-opus-4-6's rate card, per 1e6 tokens.
    ModelCost {
        input: 5.0,
        output: 25.0,
        cache_read: 0.5,
        cache_write: 6.25,
        tiers: None,
    }
}

fn model(id: &str, api: &str, adaptive: bool, prompt_cache: Option<ModelPromptCache>) -> Model {
    Model {
        id: ModelId::from(id),
        name: id.to_string(),
        api: ApiId::from(api),
        provider: ProviderId::from("anthropic"),
        base_url: "https://example.invalid".to_string(),
        reasoning: true,
        input: vec![Modality::Text],
        cost: anthropic_cost(),
        prompt_cache,
        context_window: 200_000,
        max_tokens: 32_000,
        sampling_params: None,
        thinking_level_map: None,
        compat: adaptive.then(|| cyrup_provider::api::compat::OpenAiCompletionsCompat {
            force_adaptive_thinking: Some(true),
            ..Default::default()
        }),
        headers: None,
    }
}

/// `{ short: 300, long: 3600 }` — what `apply_prompt_cache_metadata` stamps on direct Anthropic.
fn tiers() -> Option<ModelPromptCache> {
    Some(ModelPromptCache {
        short: Some(300),
        long: Some(3_600),
    })
}

/// Upstream's `adaptiveModel`: replayable even with reasoning on.
fn adaptive_model() -> Model {
    model("claude-opus-4-6", "anthropic-messages", true, tiers())
}

/// Upstream's `budgetModel`: budget-based thinking, so NOT replayable with reasoning on.
fn budget_model() -> Model {
    model("claude-sonnet-4-5", "anthropic-messages", false, tiers())
}

/// Upstream's `unknownModel`: `promptCache: undefined`.
fn unknown_model() -> Model {
    model("claude-opus-4-6", "anthropic-messages", true, None)
}

fn warm_usage() -> Usage {
    Usage {
        input: 0,
        output: 1,
        cache_read: 100,
        cache_write: 0,
        total_tokens: 101,
        cost: Cost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.01,
            cache_write: 0.0,
            total: 0.01,
        },
        ..Default::default()
    }
}

fn assistant(m: &Model, stop_reason: StopReason) -> AssistantMessage {
    // `errored` is the one public constructor that fills every field; the two it sets for an error
    // are overwritten here, which keeps this fixture immune to new optional fields.
    let mut a = AssistantMessage::errored(
        m.provider.clone(),
        m.id.as_str(),
        Some(m.api.clone()),
        stop_reason,
        "",
    );
    a.content = Vec::<Content>::new();
    a.error_message = None;
    a.usage = warm_usage();
    a.timestamp = 0;
    a
}

/// One recorded warm (`appendUsage`'s arguments).
#[derive(Clone, Debug, PartialEq)]
struct RecordedWarm {
    provider: ProviderId,
    model: ModelId,
    usage: Usage,
    note: Option<String>,
}

/// Upstream's `fakeRuntime`: the branch's prompt size, plus the recorded `appendUsage` calls.
struct FakeHost {
    prompt_tokens: Mutex<u64>,
    recorded: Mutex<Vec<RecordedWarm>>,
}

impl FakeHost {
    fn new(prompt_tokens: u64) -> Arc<Self> {
        Arc::new(Self {
            prompt_tokens: Mutex::new(prompt_tokens),
            recorded: Mutex::new(Vec::new()),
        })
    }
    fn warms(&self) -> Vec<RecordedWarm> {
        self.recorded.lock().expect("lock").clone()
    }
}

impl CacheWarmingHost for FakeHost {
    fn last_prompt_tokens(&self) -> BoxFuture<'_, u64> {
        Box::pin(async move { *self.prompt_tokens.lock().expect("lock") })
    }
    fn record_warm(
        &self,
        provider: ProviderId,
        model: ModelId,
        usage: Usage,
        note: Option<String>,
    ) -> BoxFuture<'_, ()> {
        self.recorded.lock().expect("lock").push(RecordedWarm {
            provider,
            model,
            usage,
            note,
        });
        Box::pin(std::future::ready(()))
    }
}

/// What a scripted warm answers with.
#[derive(Clone, Copy)]
enum Script {
    Ok,
    Errored,
    Aborted,
    /// Never terminates until the run's own cancel token fires (upstream's pending promise).
    Pending,
}

/// Upstream's `streamSimple` spy: records every call's options and answers from `script`.
struct RecordingStream {
    calls: Mutex<Vec<StreamOptions>>,
    script: Script,
}

impl RecordingStream {
    fn new(script: Script) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            script,
        })
    }
    fn calls(&self) -> Vec<StreamOptions> {
        self.calls.lock().expect("lock").clone()
    }
    fn len(&self) -> usize {
        self.calls.lock().expect("lock").len()
    }
}

impl CacheWarmStream for RecordingStream {
    fn stream_warm(
        &self,
        model: &Model,
        _ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.calls.lock().expect("lock").push(opts.clone());
        let stop = match self.script {
            Script::Ok => StopReason::Length,
            Script::Errored => StopReason::Error,
            Script::Aborted => StopReason::Aborted,
            Script::Pending => {
                let cancel = opts.cancel.clone();
                return Box::pin(futures::stream::unfold(cancel, |cancel| async move {
                    match cancel.as_ref() {
                        Some(c) => {
                            c.cancelled().await;
                            None
                        }
                        None => None,
                    }
                }));
            }
        };
        let msg = assistant(model, stop);
        Box::pin(futures::stream::iter(vec![StreamEvent::terminal(msg)]))
    }
}

/// A decider that answers a fixed action, optionally after moving the virtual clock (upstream's
/// "rechecks the deadline after an extension decision").
struct ScriptedDecider {
    action: Option<CacheWarmingAction>,
    advance_ms: u64,
    seen: Mutex<Vec<CacheWarmingDecisionEvent>>,
}

impl ScriptedDecider {
    fn new(action: Option<CacheWarmingAction>) -> Arc<Self> {
        Arc::new(Self {
            action,
            advance_ms: 0,
            seen: Mutex::new(Vec::new()),
        })
    }
    fn advancing(advance_ms: u64) -> Arc<Self> {
        Arc::new(Self {
            action: Some(CacheWarmingAction::Warm),
            advance_ms,
            seen: Mutex::new(Vec::new()),
        })
    }
    fn events(&self) -> Vec<CacheWarmingDecisionEvent> {
        self.seen.lock().expect("lock").clone()
    }
}

impl CacheWarmingDecider for ScriptedDecider {
    fn decide<'a>(
        &'a self,
        event: &'a CacheWarmingDecisionEvent,
    ) -> BoxFuture<'a, CacheWarmingAction> {
        self.seen.lock().expect("lock").push(*event);
        Box::pin(async move {
            if self.advance_ms > 0 {
                tokio::time::advance(Duration::from_millis(self.advance_ms)).await;
            }
            self.action.unwrap_or(event.action)
        })
    }
}

/// A decider that panics — the Rust shape of upstream's "extension failures fall back to pi's own
/// decision" swallow.
struct PanickingDecider;

impl CacheWarmingDecider for PanickingDecider {
    fn decide<'a>(
        &'a self,
        _event: &'a CacheWarmingDecisionEvent,
    ) -> BoxFuture<'a, CacheWarmingAction> {
        Box::pin(async { panic!("extension blew up") })
    }
}

fn always_current() -> IsCurrent {
    Arc::new(|| true)
}

fn request(stream: &Arc<RecordingStream>, m: Model, opts: StreamOptions) -> CacheWarmRequest {
    CacheWarmRequest {
        stream: stream.clone() as Arc<dyn CacheWarmStream>,
        model: m,
        context: Context::default(),
        options: opts,
    }
}

struct Harness {
    warmer: Arc<CacheWarmer>,
    host: Arc<FakeHost>,
    stream: Arc<RecordingStream>,
}

fn harness(
    mode: CacheWarmingMode,
    prompt_tokens: u64,
    script: Script,
    decider: Arc<dyn CacheWarmingDecider>,
) -> Harness {
    let host = FakeHost::new(prompt_tokens);
    let stream = RecordingStream::new(script);
    let warmer = CacheWarmer::new(
        host.clone() as Arc<dyn CacheWarmingHost>,
        mode,
        decider,
        cyrup_core::CancelToken::new(),
    );
    Harness {
        warmer,
        host,
        stream,
    }
}

/// The default harness: upstream's `fakeRuntime()` — `mode: "idle"`, a 100 000-token prompt, a
/// successful warm and pi's own decision.
fn idle_harness() -> Harness {
    harness(
        CacheWarmingMode::Idle,
        100_000,
        Script::Ok,
        Arc::new(PiDecision),
    )
}

fn reasoning(level: ModelThinkingLevel) -> StreamOptions {
    StreamOptions {
        reasoning: level,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------------------------
// pure functions
// ---------------------------------------------------------------------------------------------

/// pi `getCacheWarmingDelayMs` (`cache-warmer.ts:29-32`) and its two asserted triples
/// (`test/cache-warmer.test.ts:137-139`).
///
/// **Red-proved** by replacing the whole body with `Some(ttl_ms.saturating_sub(10_000).max(1))` —
/// i.e. dropping both the 90% arm and the `<= 10_000` refusal. This test failed.
#[test]
fn seam131_delay_is_ninety_percent_but_keeps_ten_seconds() {
    assert_eq!(cache_warming_delay_ms(300_000), Some(270_000));
    assert_eq!(cache_warming_delay_ms(60_000), Some(50_000));
    // 90% of 60 s is 54 s, which leaves only 6 s; the `ttl - 10_000` arm wins.
    assert_eq!(cache_warming_delay_ms(100_000), Some(90_000));
    // At and below ten seconds there is no useful margin at all.
    assert_eq!(cache_warming_delay_ms(10_000), None);
    assert_eq!(cache_warming_delay_ms(0), None);
    // Just above: upstream's `Math.max(1, …)` floor.
    assert_eq!(cache_warming_delay_ms(10_001), Some(1));
}

/// pi `isReplayable` (`cache-warmer.ts:55-58`) and its asserted quadruple
/// (`test/cache-warmer.test.ts:140-145`).
///
/// **Red-proved** by making the first conjunct `if true` — i.e. always replayable. This test
/// failed on the budget-model-with-reasoning case.
#[test]
fn seam131_replayable_refuses_budget_thinking_on_anthropic() {
    // reasoning + anthropic-messages + no adaptive compat → NOT replayable.
    assert!(!is_replayable(
        &budget_model(),
        &reasoning(ModelThinkingLevel::Medium)
    ));
    // Same model, thinking off → replayable.
    assert!(is_replayable(
        &budget_model(),
        &reasoning(ModelThinkingLevel::Off)
    ));
    // `compat.forceAdaptiveThinking` → replayable even with reasoning on.
    assert!(is_replayable(
        &adaptive_model(),
        &reasoning(ModelThinkingLevel::Medium)
    ));
    // A non-Anthropic api derives no budget from `max_tokens`, so a replay is safe.
    let openai = model("gpt-5", "openai-responses", false, tiers());
    assert!(is_replayable(
        &openai,
        &reasoning(ModelThinkingLevel::Medium)
    ));
}

fn message_entry(id: &str, parent: Option<&str>, usage: Usage) -> Entry {
    let m = adaptive_model();
    let mut a = assistant(&m, StopReason::Stop);
    a.usage = usage;
    Entry::known(KnownEntry::Message {
        base: EntryBase {
            id: cyrup_core::EntryId::from(id),
            parent_id: parent.map(cyrup_core::EntryId::from),
            timestamp: "1970-01-01T00:00:00.000Z".to_string(),
            extra: serde_json::Map::new(),
        },
        message: cyrup_session::AgentMessage::Core(cyrup_core::Message::Assistant(a)),
    })
}

/// pi `lastPromptTokens` (`cache-warmer.ts:61-70`): walk BACKWARDS, first assistant wins, answer
/// `input + cacheRead + cacheWrite`.
///
/// **Red-proved** by dropping the `.rev()` so the walk runs forwards. This test failed: the
/// two-assistant case answered the FIRST turn's 11 000 instead of the later turn's 60 000. The
/// `total_tokens` fields below differ from the asserted sums on purpose, so a port that answered
/// `usage.total_tokens` instead of `input + cacheRead + cacheWrite` fails the same assertions.
#[test]
fn seam131_last_prompt_tokens_walks_backwards_over_assistants() {
    let first = Usage {
        input: 1_000,
        output: 500,
        cache_read: 10_000,
        cache_write: 0,
        total_tokens: 11_500,
        ..Default::default()
    };
    let second = Usage {
        input: 2_000,
        output: 7,
        cache_read: 8_000,
        cache_write: 50_000,
        total_tokens: 60_007,
        ..Default::default()
    };
    let a = message_entry("a", None, first);
    let b = message_entry("b", Some("a"), second);
    assert_eq!(last_prompt_tokens(&[&a, &b]), 60_000);
    assert_eq!(last_prompt_tokens(&[&a]), 11_000);
    assert_eq!(last_prompt_tokens(&[]), 0);

    // A branch whose LEAF is a `usage` entry — which is what every warm leaves behind, because
    // `append_usage` advances the leaf — still prices the last real request.
    let warm = Entry::known(KnownEntry::Usage {
        base: EntryBase {
            id: cyrup_core::EntryId::from("c"),
            parent_id: Some(cyrup_core::EntryId::from("b")),
            timestamp: "1970-01-01T00:00:01.000Z".to_string(),
            extra: serde_json::Map::new(),
        },
        kind: "cache_warm".to_string(),
        provider: ProviderId::from("anthropic"),
        model: ModelId::from("claude-opus-4-6"),
        usage: warm_usage(),
        note: None,
    });
    assert_eq!(last_prompt_tokens(&[&a, &b, &warm]), 60_000);
}

// ---------------------------------------------------------------------------------------------
// the state machine, on the virtual clock
// ---------------------------------------------------------------------------------------------

/// Let every spawned timer/refresh task reach its next await point. `advance` wakes the sleeper
/// but does not run it; these tests are single-threaded (`start_paused` implies
/// `current_thread`), so the handoff is explicit.
///
/// This used to be a fixed 16-iteration yield loop and that made the longest chain in this file
/// (bounded decider timeout -> in-flight stream -> `record_warm` -> reschedule) INTERMITTENTLY
/// fail: under compile load a chain needing more than 16 rounds silently left the state machine
/// mid-flight and the next assertion read the earlier state. ~2 failures in ~200 runs, which is a
/// red suite on a feature that is otherwise correct. The cap is now generous enough that
/// exhausting it is a real hang rather than a scheduling race, and [`settle_until`] replaces the
/// count with the observable wherever a test knows what it is waiting for.
const SETTLE_ROUNDS: usize = 4_096;

async fn settle() {
    for _ in 0..SETTLE_ROUNDS {
        tokio::task::yield_now().await;
    }
}

/// Yield until `cond` holds, then once more so the task that satisfied it can park. Panics rather
/// than returning quietly if it never does — a silent timeout is the bug this replaces.
async fn settle_until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..SETTLE_ROUNDS {
        if cond() {
            tokio::task::yield_now().await;
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("the warmer never reached the expected state: {what}");
}

async fn advance(ms: u64) {
    tokio::time::advance(Duration::from_millis(ms)).await;
    settle().await;
}

/// `advance`, but stopping as soon as the observable the caller is waiting for is true.
async fn advance_until(ms: u64, what: &str, cond: impl FnMut() -> bool) {
    tokio::time::advance(Duration::from_millis(ms)).await;
    settle_until(what, cond).await;
}

fn session_request(stream: &Arc<RecordingStream>) -> (CacheWarmRequest, cyrup_core::CancelToken) {
    let caller_cancel = cyrup_core::CancelToken::new();
    let opts = StreamOptions {
        reasoning: ModelThinkingLevel::High,
        session_id: Some(cyrup_core::SessionId::from("s")),
        cancel: Some(caller_cancel.clone()),
        max_tokens: Some(8_000),
        max_retries: Some(3),
        temperature: Some(0.7),
        ..Default::default()
    };
    (request(stream, adaptive_model(), opts), caller_cancel)
}

/// pi "replays profitable requests and preserves options across repeated refreshes"
/// (`test/cache-warmer.test.ts:148-176`).
///
/// A 300 s short-tier lifetime arms at 270 s; the warm carries `maxTokens: 1` and
/// `maxRetries: 0`, its OWN cancel token (not the caller's), and every other option unchanged; the
/// economics match upstream's asserted `missCost ≈ 0.575` / `warmCost ≈ 0.050025`; the entry is
/// recorded with no note; and a second refresh follows 270 s later.
///
/// **Red-proved** by replacing the warm's option overlay with the captured
/// `run.request.options.clone()` — the one mutant that drops all three overrides at once
/// (`maxTokens: 1`, `maxRetries: 0` and the warm's own cancel token). This test failed. The 90%
/// arming is red-proved separately in
/// [`seam131_delay_is_ninety_percent_but_keeps_ten_seconds`].
#[tokio::test(start_paused = true)]
async fn seam131_warms_once_per_ttl_and_preserves_the_request() {
    let h = idle_harness();
    let (req, caller_cancel) = session_request(&h.stream);
    h.warmer.start(req, always_current());

    advance(269_999).await;
    assert_eq!(h.stream.len(), 0, "nothing before 90% of the TTL");
    advance(2).await;
    assert_eq!(h.stream.len(), 1, "exactly one warm at 90% of the TTL");

    let sent = h.stream.calls();
    let sent = sent.first().expect("one warm");
    assert_eq!(sent.max_tokens, Some(1), "pi `maxTokens: 1`");
    assert_eq!(sent.max_retries, Some(0), "pi `maxRetries: 0`");
    // Every other field is the captured request's, byte for byte.
    assert_eq!(sent.reasoning, ModelThinkingLevel::High);
    assert_eq!(
        sent.session_id.as_ref().map(|s| s.to_string()),
        Some("s".to_string())
    );
    assert_eq!(sent.temperature, Some(0.7));
    // pi: `expect(calls[0].options?.signal).not.toBe(signal)` — the warm owns its own abort.
    let warm_cancel = sent
        .cancel
        .clone()
        .expect("the warm carries a cancel token");
    caller_cancel.cancel();
    assert!(
        !warm_cancel.is_cancelled(),
        "the warm must not ride the caller's cancellation"
    );

    // The economics upstream asserts, to the same precision.
    let status = h.warmer.status().await;
    let decision = status.decision.expect("a decision is attached");
    assert!(
        (decision.miss_cost - 0.575).abs() < 1e-9,
        "miss cost {}",
        decision.miss_cost
    );
    assert!(
        (decision.warm_cost - 0.050_025).abs() < 1e-9,
        "warm cost {}",
        decision.warm_cost
    );

    // `appendUsage("cache_warm", provider, model, usage, undefined)`.
    assert_eq!(
        h.host.warms(),
        vec![RecordedWarm {
            provider: ProviderId::from("anthropic"),
            model: ModelId::from("claude-opus-4-6"),
            usage: warm_usage(),
            note: None,
        }]
    );

    advance(270_000).await;
    assert_eq!(h.stream.len(), 2, "a successful warm reschedules");
}

/// pi "does not issue refreshes after their safe deadline"
/// (`test/cache-warmer.test.ts:178-194`): *"A five-minute cache is scheduled for 4m30s and retains
/// 15 seconds of the 30-second expiry margin. Simulate a timer delayed by sleep."*
///
/// The decider is asserted to have been consulted ZERO times, and that assertion is the point:
/// upstream checks the deadline at the TOP of `refresh`, before `evaluate` and before the
/// extension dispatch, so a refresh that is already too late costs nothing at all. Without it the
/// second (post-decision) check would still suppress the request, which is why asserting only
/// "nothing was sent" leaves the first check unproven — a mutant that deleted it survived that
/// weaker test.
///
/// **Red-proved** twice: deleting the top `refresh_deadline_missed` check failed the
/// `the decider is not even consulted` assertion (1 event instead of 0), and deleting BOTH
/// deadline checks sent the warm — a full-price cache write billed as a warm.
#[tokio::test(start_paused = true)]
async fn seam131_refresh_past_its_deadline_sends_nothing() {
    let decider = ScriptedDecider::new(None);
    let h = harness(
        CacheWarmingMode::Idle,
        100_000,
        Script::Ok,
        decider.clone() as Arc<dyn CacheWarmingDecider>,
    );
    let (req, _) = session_request(&h.stream);
    h.warmer.start(req, always_current());
    // 270 000 + floor((300 000 - 270 000)/2) = 285 000.
    h.warmer.disarm_timer_for_test();
    advance(285_001).await;
    assert!(h.warmer.refresh_active_run_for_test().await);
    assert_eq!(h.stream.len(), 0, "nothing is sent");
    assert_eq!(
        decider.events().len(),
        0,
        "the decider is not even consulted: the deadline is checked FIRST"
    );
    let status = h.warmer.status().await;
    assert_eq!(status.state, CacheWarmingState::Inactive);
    assert_eq!(
        status.reason.as_deref(),
        Some("cache refresh deadline missed")
    );
}

/// pi "rechecks the deadline after an extension decision"
/// (`test/cache-warmer.test.ts:196-215`): a slow handler can push the refresh past the deadline,
/// so the deadline is re-checked AFTER the decision, not only before it.
///
/// **Red-proved** by removing the second `refresh_deadline_missed(&run)` call (the one in the
/// `if !self.validate_run(&run) || …` line after `decide`): the warm was sent.
#[tokio::test(start_paused = true)]
async fn seam131_deadline_is_rechecked_after_the_extension_decision() {
    let decider = ScriptedDecider::advancing(285_001);
    let h = harness(
        CacheWarmingMode::Idle,
        100_000,
        Script::Ok,
        decider.clone() as Arc<dyn CacheWarmingDecider>,
    );
    let (req, _) = session_request(&h.stream);
    h.warmer.start(req, always_current());
    h.warmer.disarm_timer_for_test();
    assert!(h.warmer.refresh_active_run_for_test().await);
    assert_eq!(decider.events().len(), 1, "the decider WAS consulted");
    assert_eq!(h.stream.len(), 0, "but the request was not sent");
    assert_eq!(
        h.warmer.status().await.reason.as_deref(),
        Some("cache refresh deadline missed")
    );
}

/// pi "applies economic decisions and extension overrides"
/// (`test/cache-warmer.test.ts:217-253`), all four cases.
///
/// **Red-proved** by forcing the `extension_override` branch of the stop-reason selection to
/// `false`: the vetoed case reported `expected savings below threshold` where pi reports
/// `stopped by extension`, and this test failed.
#[tokio::test(start_paused = true)]
async fn seam131_economics_and_extension_overrides() {
    // (1) unprofitable: a 5 000-token prompt at 15% continuation cannot clear five cents.
    let unprofitable = harness(
        CacheWarmingMode::Idle,
        5_000,
        Script::Ok,
        Arc::new(PiDecision),
    );
    let (req, _) = session_request(&unprofitable.stream);
    unprofitable.warmer.start(req, always_current());
    advance(270_000).await;
    assert_eq!(unprofitable.stream.len(), 0);
    let status = unprofitable.warmer.status().await;
    assert_eq!(status.state, CacheWarmingState::Inactive);
    assert_eq!(
        status.reason.as_deref(),
        Some("expected savings below threshold")
    );
    let decision = status.decision.expect("the stopping decision is attached");
    assert_eq!(decision.action, CacheWarmingAction::Stop);
    assert!(decision.economics_available);
    assert!(!status.extension_override);

    // (2) an extension forces the same unprofitable refresh through.
    let forced = harness(
        CacheWarmingMode::Idle,
        5_000,
        Script::Ok,
        ScriptedDecider::new(Some(CacheWarmingAction::Warm)) as Arc<dyn CacheWarmingDecider>,
    );
    let (req, _) = session_request(&forced.stream);
    forced.warmer.start(req, always_current());
    advance(270_000).await;
    assert_eq!(forced.stream.len(), 1);
    assert_eq!(
        forced.host.warms().first().and_then(|w| w.note.clone()),
        Some("extension override".to_string())
    );

    // (3) an extension vetoes a profitable one.
    let vetoed = harness(
        CacheWarmingMode::Idle,
        100_000,
        Script::Ok,
        ScriptedDecider::new(Some(CacheWarmingAction::Stop)) as Arc<dyn CacheWarmingDecider>,
    );
    let (req, _) = session_request(&vetoed.stream);
    vetoed.warmer.start(req, always_current());
    advance(270_000).await;
    assert_eq!(vetoed.stream.len(), 0);
    let status = vetoed.warmer.status().await;
    assert!(status.extension_override);
    assert_eq!(status.reason.as_deref(), Some("stopped by extension"));

    // (4) economics unavailable: a branch with no priced prompt at all.
    let unavailable = harness(CacheWarmingMode::Idle, 0, Script::Ok, Arc::new(PiDecision));
    let (req, _) = session_request(&unavailable.stream);
    unavailable.warmer.start(req, always_current());
    let status = unavailable.warmer.status().await;
    assert_eq!(status.state, CacheWarmingState::Inactive);
    assert_eq!(
        status.reason.as_deref(),
        Some("cache economics unavailable")
    );
    advance(270_000).await;
    assert_eq!(unavailable.stream.len(), 0);
    assert_eq!(
        unavailable.warmer.status().await.reason.as_deref(),
        Some("cache economics unavailable")
    );
}

/// The `$0.05` minimum-expected-savings threshold, from both sides, and the phase's continuation
/// probability (pi `:388`, `:398`).
///
/// 10 000 prompt tokens, streaming (probability 1): miss cost is
/// `10_000 * (6.25 - 0.5) / 1e6 = 0.0575`, warm cost `10_000 * 0.5/1e6 + 25/1e6 = 0.005025`, so
/// expected savings are `0.052475` — above the line. At 9 000 tokens they are `0.047225` — below.
///
/// The `>=` of upstream's comparison is deliberately NOT asserted, and that is a limit of the
/// surface rather than an oversight: no reachable savings value lands exactly on the `f64` nearest
/// `0.05`, so `>=` and `>` are indistinguishable from outside `evaluate`. What IS pinned is that a
/// threshold exists and sits where upstream puts it.
///
/// **Red-proved** by replacing the threshold with `0.0`: the below-the-line case failed (it
/// reported `Warm`, i.e. every refresh would be sent however little it saved).
#[tokio::test(start_paused = true)]
async fn seam131_savings_threshold_is_inclusive() {
    let above = harness(
        CacheWarmingMode::Streaming,
        10_000,
        Script::Ok,
        Arc::new(PiDecision),
    );
    let (req, _) = session_request(&above.stream);
    above.warmer.start(req, always_current());
    let d = above
        .warmer
        .status()
        .await
        .decision
        .expect("streaming decision");
    assert_eq!(d.continuation_probability, 1.0);
    assert_eq!(d.phase, CacheWarmingPhase::Streaming);
    assert_eq!(d.action, CacheWarmingAction::Warm);
    assert!(d.expected_savings >= 0.05);

    let below = harness(
        CacheWarmingMode::Streaming,
        9_000,
        Script::Ok,
        Arc::new(PiDecision),
    );
    let (req, _) = session_request(&below.stream);
    below.warmer.start(req, always_current());
    let d = below
        .warmer
        .status()
        .await
        .decision
        .expect("streaming decision");
    assert!(d.expected_savings < 0.05);
    assert_eq!(d.action, CacheWarmingAction::Stop);

    // Idle halves nothing — it multiplies by 0.15.
    let idle = harness(
        CacheWarmingMode::Idle,
        10_000,
        Script::Ok,
        Arc::new(PiDecision),
    );
    let (req, _) = session_request(&idle.stream);
    idle.warmer.start(req, always_current());
    // `status` reports the phase the run is in; it only becomes `idle` once the agent settles.
    idle.warmer.on_agent_settled();
    let d = idle.warmer.status().await.decision.expect("idle decision");
    assert_eq!(d.continuation_probability, 0.15);
    assert_eq!(d.phase, CacheWarmingPhase::Idle);
    // The DECISION, not just its inputs. This is the one arithmetic that stops cyrup paying for
    // idle warms that cost more than they save, and asserting `continuation_probability == 0.15`
    // does not pin it: dropping the factor out of `expected_savings` entirely left this test
    // green. At 10k prompt tokens the un-discounted saving is 0.0575 - 0.005025 = $0.0525, which
    // CLEARS the $0.05 floor; discounted by 0.15 it is $0.0036, which does not. So the two lines
    // below are exactly the difference between pi's behaviour and sending a warm every 270s for
    // half an hour on a 6.7x overvaluation.
    assert!(
        d.expected_savings < 0.05,
        "the idle discount must be APPLIED, not merely reported: {} (miss {} warm {})",
        d.expected_savings,
        d.miss_cost,
        d.warm_cost
    );
    assert!(
        (d.expected_savings - (0.15 * d.miss_cost - d.warm_cost)).abs() < 1e-12,
        "expected_savings must be p * missCost - warmCost (`cache-warmer.ts:398` @v1.0.4): {} vs          {}",
        d.expected_savings,
        0.15 * d.miss_cost - d.warm_cost
    );
    assert_eq!(
        d.action,
        CacheWarmingAction::Stop,
        "below the $0.05 floor BECAUSE of the 0.15 discount"
    );
}

/// pi "stops for unsupported requests, context changes, and mode changes"
/// (`test/cache-warmer.test.ts:255-281`), plus the `cacheRetention: "none"` reason upstream has in
/// `start` but does not assert.
///
/// **Red-proved** twice: collapsing the two no-TTL reasons into one made the
/// `cacheRetention: "none"` case read `cache lifetime unavailable`, and dropping the `is_current`
/// clause of `validate_run` lost the `conversation context changed` reason. Each failed this test
/// on its own.
#[tokio::test(start_paused = true)]
async fn seam131_stop_reasons_for_unsupported_requests_and_mode_changes() {
    let h = idle_harness();

    // Mode off: nothing is ever scheduled.
    h.warmer.set_mode(CacheWarmingMode::Off);
    let (req, _) = session_request(&h.stream);
    h.warmer.start(req, always_current());
    assert_eq!(
        h.warmer.status().await.reason.as_deref(),
        Some("cache warming disabled")
    );
    assert!(!h.warmer.has_active_run());
    // No run is a reason to believe nothing CAN be sent; it is not the same as nothing BEING
    // sent, which is the clause SEAM-131's Verify actually makes ("nothing at all is sent with
    // `cacheWarming: "off"`"). Walk past where a warm would have been due and assert the stream
    // was never touched, so the clause is held by an observation of the request path rather than
    // by an inference from the warmer's own bookkeeping.
    advance(270_000).await;
    assert_eq!(h.stream.len(), 0);
    h.warmer.set_mode(CacheWarmingMode::Idle);

    // A model that publishes no prompt-cache lifetime.
    h.warmer.start(
        request(&h.stream, unknown_model(), StreamOptions::default()),
        always_current(),
    );
    assert_eq!(
        h.warmer.status().await.reason.as_deref(),
        Some("cache lifetime unavailable")
    );

    // A request that turned caching off has no entry to keep warm — a DIFFERENT reason.
    h.warmer.start(
        request(
            &h.stream,
            adaptive_model(),
            StreamOptions {
                cache_retention: Some(CacheRetention::None),
                ..Default::default()
            },
        ),
        always_current(),
    );
    assert_eq!(
        h.warmer.status().await.reason.as_deref(),
        Some("request disabled prompt caching")
    );

    // Budget-based thinking on Anthropic: a `maxTokens: 1` replay would change the budget.
    h.warmer.start(
        request(
            &h.stream,
            budget_model(),
            reasoning(ModelThinkingLevel::High),
        ),
        always_current(),
    );
    assert_eq!(
        h.warmer.status().await.reason.as_deref(),
        Some("request cannot be replayed safely")
    );

    // The conversation moved on.
    let still_current = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag = still_current.clone();
    let (req, _) = session_request(&h.stream);
    h.warmer.start(
        req,
        Arc::new(move || flag.load(std::sync::atomic::Ordering::SeqCst)),
    );
    still_current.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        h.warmer.status().await.reason.as_deref(),
        Some("conversation context changed")
    );
    advance(270_000).await;
    assert_eq!(h.stream.len(), 0);

    // The mode flips to off while a timer is armed: the next checkpoint declines.
    let (req, _) = session_request(&h.stream);
    h.warmer.start(req, always_current());
    h.warmer.set_mode(CacheWarmingMode::Off);
    advance(270_000).await;
    assert_eq!(h.stream.len(), 0);

    // `streaming` mode ends the run when the agent settles.
    let streaming = harness(
        CacheWarmingMode::Streaming,
        400_000,
        Script::Ok,
        Arc::new(PiDecision),
    );
    let (req, _) = session_request(&streaming.stream);
    streaming.warmer.start(req, always_current());
    streaming.warmer.on_agent_settled();
    assert_eq!(
        streaming.warmer.status().await.reason.as_deref(),
        Some("agent run settled")
    );
}

/// `idle` keeps warming after the agent settles; flipping the mode back to `streaming` then stops
/// the idle run — pi `onModeChanged` → `getModeStopReason` (`cache-warmer.ts:260-265`, `:371-376`).
///
/// **Red-proved** by making `on_mode_changed` a no-op: the idle run survived the flip and sent a
/// warm 270 s later, which is the "persist-only `/settings` row" bug this hook exists to prevent.
#[tokio::test(start_paused = true)]
async fn seam131_idle_survives_settle_and_a_mode_flip_stops_it() {
    // 400 000 prompt tokens: at the idle phase's 15% continuation probability a smaller prompt
    // would stop on economics before the mode ever mattered (0.15 * missCost must clear $0.05
    // plus the warm's own cost, which needs ~138 000 tokens on this rate card).
    let h = harness(
        CacheWarmingMode::Idle,
        400_000,
        Script::Ok,
        Arc::new(PiDecision),
    );
    let (req, _) = session_request(&h.stream);
    h.warmer.start(req, always_current());
    h.warmer.on_agent_settled();
    assert!(h.warmer.has_active_run(), "idle keeps warming after settle");
    advance(270_000).await;
    assert_eq!(h.stream.len(), 1, "an idle run still warms");

    h.warmer.set_mode(CacheWarmingMode::Streaming);
    h.warmer.on_mode_changed();
    assert_eq!(
        h.warmer.status().await.reason.as_deref(),
        Some("agent run settled")
    );
    advance(600_000).await;
    assert_eq!(h.stream.len(), 1, "nothing after the mode flip");
}

/// The two fixed safety windows, and that successful warms never push them out — pi `schedule`
/// (`cache-warmer.ts:291-295`) and its comment *"warm requests never extend the fixed safety
/// windows"* (`:160`).
///
/// **Red-proved** by anchoring the deadline at `now` instead of `run.started_at`: the idle run
/// below warmed forever (the assertion of exactly six warms failed at seven), and the one-hour
/// case armed instead of stopping.
#[tokio::test(start_paused = true)]
async fn seam131_safety_windows_are_fixed_from_the_first_request() {
    // One hour: a lifetime whose 90% delay lands past `started_at + 1h`.
    let h = harness(
        CacheWarmingMode::Streaming,
        100_000,
        Script::Ok,
        Arc::new(PiDecision),
    );
    let long = model(
        "claude-opus-4-6",
        "anthropic-messages",
        true,
        Some(ModelPromptCache {
            short: Some(5_000),
            long: None,
        }),
    );
    h.warmer.start(
        request(&h.stream, long, StreamOptions::default()),
        always_current(),
    );
    assert_eq!(
        h.warmer.status().await.reason.as_deref(),
        Some("one-hour safety limit reached")
    );
    assert!(!h.warmer.has_active_run());

    // Thirty minutes, idle: the first schedule is inside the one-hour streaming window, and
    // settling re-measures it against the shorter one.
    let idle = idle_harness();
    let medium = model(
        "claude-opus-4-6",
        "anthropic-messages",
        true,
        Some(ModelPromptCache {
            short: Some(2_500),
            long: None,
        }),
    );
    idle.warmer.start(
        request(&idle.stream, medium, StreamOptions::default()),
        always_current(),
    );
    assert!(idle.warmer.has_active_run());
    idle.warmer.on_agent_settled();
    assert_eq!(
        idle.warmer.status().await.reason.as_deref(),
        Some("30-minute idle safety limit reached")
    );

    // Repeated warms do not extend the 30-minute window: 270 s apart, the seventh would land at
    // 1 890 s > 1 800 s.
    let cycling = harness(
        CacheWarmingMode::Idle,
        400_000,
        Script::Ok,
        Arc::new(PiDecision),
    );
    let (req, _) = session_request(&cycling.stream);
    cycling.warmer.start(req, always_current());
    cycling.warmer.on_agent_settled();
    for _ in 0..8 {
        advance(270_000).await;
    }
    assert_eq!(cycling.stream.len(), 6, "six warms inside 30 minutes");
    assert_eq!(
        cycling.warmer.status().await.reason.as_deref(),
        Some("30-minute idle safety limit reached")
    );
}

/// pi "aborts replaced requests and does not record failed refreshes"
/// (`test/cache-warmer.test.ts:283-308`).
///
/// **Red-proved** twice: dropping the `run.cancel.cancel()` from `clear_run` left the replaced
/// request's token un-cancelled; and removing the `stopReason` filter recorded a usage entry for
/// both the errored and the aborted warm, i.e. billed a failed refresh to the session.
#[tokio::test(start_paused = true)]
async fn seam131_replaced_runs_abort_and_failures_are_not_recorded() {
    let pending = harness(
        CacheWarmingMode::Idle,
        100_000,
        Script::Pending,
        Arc::new(PiDecision),
    );
    let (req, _) = session_request(&pending.stream);
    pending.warmer.start(req, always_current());
    // The observable, not a yield count: this chain (bounded decider timeout -> in-flight stream)
    // is the longest in the file and a fixed-round `settle` raced it under load.
    {
        let stream = pending.stream.clone();
        advance_until(270_000, "the warm reaches the stream", move || {
            stream.len() == 1
        })
        .await;
    }
    assert_eq!(pending.stream.len(), 1, "the warm is in flight");
    let (req, _) = session_request(&pending.stream);
    pending.warmer.start(req, always_current());
    let first = pending.stream.calls();
    let first_cancel = first
        .first()
        .and_then(|o| o.cancel.clone())
        .expect("the in-flight warm had a token");
    assert!(
        first_cancel.is_cancelled(),
        "starting a new run aborts the replaced one"
    );
    pending.warmer.cancel();
    advance(600_000).await;
    assert_eq!(pending.stream.len(), 1);
    assert!(pending.host.warms().is_empty());

    for script in [Script::Errored, Script::Aborted] {
        let failed = harness(
            CacheWarmingMode::Idle,
            100_000,
            script,
            Arc::new(PiDecision),
        );
        let (req, _) = session_request(&failed.stream);
        failed.warmer.start(req, always_current());
        {
            let stream = failed.stream.clone();
            advance_until(270_000, "the first warm reaches the stream", move || {
                stream.len() == 1
            })
            .await;
        }
        assert_eq!(failed.stream.len(), 1);
        assert!(
            failed.host.warms().is_empty(),
            "a failed refresh is not recorded"
        );
        // Best-effort: the run keeps going rather than tearing down on a failure. The wait is on
        // the RESCHEDULED warm, which is the state the fixed-round `settle` could miss: the chain
        // is stream completion -> `record_warm` skip -> `schedule` -> timer.
        {
            let stream = failed.stream.clone();
            advance_until(
                270_000,
                "the rescheduled warm reaches the stream",
                move || stream.len() == 2,
            )
            .await;
        }
        assert_eq!(failed.stream.len(), 2, "a failed refresh still reschedules");
    }
}

/// A decider that blows up falls back to pi's own action — pi's `try { … } catch {}` around
/// `this.decide(…)` with the comment *"Extension failures fall back to pi's own decision"*
/// (`cache-warmer.ts:307-317`).
///
/// **Red-proved** by removing the `catch_unwind`: the refresh task unwound, no warm was sent, and
/// the run was left armed with no timer — i.e. a broken extension silently disabled warming.
#[tokio::test(start_paused = true)]
async fn seam131_a_failing_decider_falls_back_to_pis_action() {
    let h = harness(
        CacheWarmingMode::Idle,
        100_000,
        Script::Ok,
        Arc::new(PanickingDecider),
    );
    let (req, _) = session_request(&h.stream);
    h.warmer.start(req, always_current());
    advance(270_000).await;
    assert_eq!(h.stream.len(), 1, "pi's own `warm` still went out");
    assert_eq!(
        h.host.warms().first().and_then(|w| w.note.clone()),
        None,
        "a failed decider is not an extension override"
    );
}

/// The decision event carries exactly pi's four `Pick`ed fields, with pi's values
/// (`cache-warmer.ts:112-115`, `:308-314`).
///
/// **Red-proved** by seeding the event's `action` from a hardcoded `Warm`: the unprofitable case
/// below reported `Warm` where pi reports `Stop`, which is the value an extension is supposed to
/// be able to override.
#[tokio::test(start_paused = true)]
async fn seam131_decision_event_carries_pis_own_numbers() {
    let decider = ScriptedDecider::new(None);
    let h = harness(
        CacheWarmingMode::Idle,
        5_000,
        Script::Ok,
        decider.clone() as Arc<dyn CacheWarmingDecider>,
    );
    let (req, _) = session_request(&h.stream);
    h.warmer.start(req, always_current());
    // The probability is the PHASE's, not the mode's: it is 1 until the agent settles.
    h.warmer.on_agent_settled();
    advance(270_000).await;
    let events = decider.events();
    let event = events.first().expect("one decision event");
    assert_eq!(event.continuation_probability, 0.15);
    assert_eq!(event.action, CacheWarmingAction::Stop);
    assert!((event.warm_cost - (5_000.0 * 0.5 / 1e6 + 25.0 / 1e6)).abs() < 1e-12);
    assert!((event.miss_cost - (5_000.0 * (6.25 - 0.5) / 1e6)).abs() < 1e-12);
    // A decider with no opinion is not an override.
    assert!(!h.warmer.status().await.extension_override);
}

/// Disposal stops warming — pi's `dispose()` tail, `this._cacheWarmer?.cancel()`
/// (`agent-session.ts:1395-1397`), and the module's own rule that a run token is a child of the
/// session's.
///
/// **Red-proved** by giving the run a standalone `CancelToken::new()` instead of
/// `session_cancel.child_token()`: the second half sent a warm after the session was cancelled.
/// The first draft advanced the clock in one 600-second jump and that mutant SURVIVED — the late
/// timer was suppressed by the refresh deadline instead, so the test proved nothing about the
/// token. Hence the 270-second steps.
#[tokio::test(start_paused = true)]
async fn seam131_cancel_and_session_teardown_both_stop_the_run() {
    let h = idle_harness();
    let (req, _) = session_request(&h.stream);
    h.warmer.start(req, always_current());
    h.warmer.cancel();
    assert!(!h.warmer.has_active_run());
    assert_eq!(h.warmer.status().await.reason.as_deref(), Some("inactive"));
    for _ in 0..3 {
        advance(270_000).await;
    }
    assert_eq!(h.stream.len(), 0);

    // The same, driven from the session root rather than from the warmer.
    let host = FakeHost::new(100_000);
    let stream = RecordingStream::new(Script::Ok);
    let session_cancel = cyrup_core::CancelToken::new();
    let warmer = CacheWarmer::new(
        host as Arc<dyn CacheWarmingHost>,
        CacheWarmingMode::Idle,
        Arc::new(PiDecision),
        session_cancel.clone(),
    );
    let (req, _) = session_request(&stream);
    warmer.start(req, always_current());
    assert!(warmer.has_active_run());
    session_cancel.cancel();
    // In 270-second STEPS, not one 600-second jump: a single jump past the refresh deadline would
    // let the deadline check suppress the warm and hide whether the token was cancelled at all —
    // which is exactly how the un-childed mutant below survived a first draft of this test.
    for _ in 0..3 {
        advance(270_000).await;
    }
    assert_eq!(stream.len(), 0, "the session's cancel disarms the timer");
}

// ---------------------------------------------------------------------------------------------
// formatters
// ---------------------------------------------------------------------------------------------

fn decision(
    phase: CacheWarmingPhase,
    probability: f64,
    expected_savings: f64,
    economics_available: bool,
    action: CacheWarmingAction,
) -> CacheWarmingDecision {
    CacheWarmingDecision {
        phase,
        warm_cost: 0.013,
        miss_cost: 0.621,
        continuation_probability: probability,
        expected_savings,
        economics_available,
        action,
    }
}

/// pi `formatCacheWarmingStatus` + `formatCacheWarmingEconomics` + `formatDollars`
/// (`cache-warmer.ts:403-446`), with upstream's own asserted string
/// (`test/cache-warmer.test.ts:311-322`).
///
/// **Red-proved** by rendering `format_dollars` without its sign branch (`format!("${value:.3}")`
/// for every value): this test failed, because the streaming case's negative savings printed
/// `$-0.123` instead of pi's `-$0.123`.
#[test]
fn seam131_status_line_matches_upstream() {
    let d = decision(
        CacheWarmingPhase::Idle,
        0.6,
        0.36,
        true,
        CacheWarmingAction::Warm,
    );
    let scheduled = CacheWarmingStatus {
        state: CacheWarmingState::Scheduled,
        reason: None,
        next_warm_in: Some(Duration::from_millis(222_000)),
        decision: Some(d),
        extension_override: false,
    };
    assert_eq!(
        format_cache_warming_status(&scheduled),
        "Decision in 3m 42s (60% continuation probability, expected savings $0.360 >= $0.050 -> warm)"
    );

    // Streaming phase names the reason the probability is 1.
    let streaming = CacheWarmingStatus {
        decision: Some(decision(
            CacheWarmingPhase::Streaming,
            1.0,
            -0.123,
            true,
            CacheWarmingAction::Stop,
        )),
        ..scheduled.clone()
    };
    assert_eq!(
        format_cache_warming_status(&streaming),
        "Decision in 3m 42s (100% continuation probability while agent is running, expected savings -$0.123 < $0.050 -> stop)"
    );

    // In flight.
    let refreshing = CacheWarmingStatus {
        state: CacheWarmingState::Refreshing,
        ..scheduled.clone()
    };
    assert_eq!(
        format_cache_warming_status(&refreshing),
        "Warming cache (60% continuation probability, expected savings $0.360 >= $0.050 -> warm)"
    );

    // Stopped WITH a decision reports the decision, not the reason.
    let stopped = CacheWarmingStatus {
        state: CacheWarmingState::Inactive,
        reason: Some("expected savings below threshold".to_string()),
        next_warm_in: None,
        ..scheduled.clone()
    };
    assert_eq!(
        format_cache_warming_status(&stopped),
        "Stopped (60% continuation probability, expected savings $0.360 >= $0.050 -> warm)"
    );

    // An extension override replaces the `-> action` tail.
    let overridden = CacheWarmingStatus {
        extension_override: true,
        ..stopped.clone()
    };
    assert_eq!(
        format_cache_warming_status(&overridden),
        "Stopped (extension override, 60% continuation probability, expected savings $0.360 >= $0.050)"
    );

    // No decision at all → the reason.
    let inactive = CacheWarmingStatus {
        state: CacheWarmingState::Inactive,
        reason: Some("waiting for first request".to_string()),
        next_warm_in: None,
        decision: None,
        extension_override: false,
    };
    assert_eq!(
        format_cache_warming_status(&inactive),
        "Inactive (waiting for first request)"
    );
    assert_eq!(
        format_cache_warming_status(&CacheWarmingStatus {
            reason: None,
            ..inactive.clone()
        }),
        "Inactive (unknown reason)"
    );

    // Inactive + economics unavailable + no override → still the reason, not `Stopped (…)`.
    let unavailable = CacheWarmingStatus {
        decision: Some(decision(
            CacheWarmingPhase::Idle,
            0.15,
            0.0,
            false,
            CacheWarmingAction::Stop,
        )),
        reason: Some("cache economics unavailable".to_string()),
        ..inactive.clone()
    };
    assert_eq!(
        format_cache_warming_status(&unavailable),
        "Inactive (cache economics unavailable)"
    );
    // …but the same decision while SCHEDULED prints the economics placeholder.
    assert_eq!(
        format_cache_warming_status(&CacheWarmingStatus {
            state: CacheWarmingState::Scheduled,
            next_warm_in: Some(Duration::from_millis(1_000)),
            ..unavailable.clone()
        }),
        "Decision in 1s (cache economics unavailable -> stop)"
    );
}

/// pi `formatCacheWarmingDecisionTime` (`cache-warmer.ts:418-430`) — the h/m/s elision rules.
///
/// **Red-proved** by flooring instead of ceiling the seconds: this test failed, because the
/// 1 500 ms case printed `1s` and the 1 ms case printed `0s` where pi prints `2s` and `1s`.
#[test]
fn seam131_decision_time_elides_zero_parts() {
    assert_eq!(
        format_cache_warming_decision_time(Some(Duration::from_millis(3_723_000))),
        "Decision in 1h 2m 3s"
    );
    assert_eq!(
        format_cache_warming_decision_time(Some(Duration::from_millis(3_603_000))),
        "Decision in 1h 3s"
    );
    assert_eq!(
        format_cache_warming_decision_time(Some(Duration::from_millis(3_600_000))),
        "Decision in 1h"
    );
    assert_eq!(
        format_cache_warming_decision_time(Some(Duration::from_millis(120_000))),
        "Decision in 2m"
    );
    // `Math.ceil`: a partial second counts.
    assert_eq!(
        format_cache_warming_decision_time(Some(Duration::from_millis(1_500))),
        "Decision in 2s"
    );
    // The `seconds > 0 || parts.length === 0` arm.
    assert_eq!(
        format_cache_warming_decision_time(Some(Duration::from_millis(1))),
        "Decision in 1s"
    );
    assert_eq!(
        format_cache_warming_decision_time(Some(Duration::ZERO)),
        "Decision now"
    );
    assert_eq!(format_cache_warming_decision_time(None), "Decision now");
}

// ---------------------------------------------------------------------------------------------
// EXT-085 — the `cache_warming_decision` extension hook, as the warmer sees it.
// ---------------------------------------------------------------------------------------------

/// A decider that never answers — a guest handler stuck in a loop, or a native one holding a lock.
struct HangingDecider {
    calls: Mutex<usize>,
}

impl HangingDecider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(0),
        })
    }
    fn calls(&self) -> usize {
        *self.calls.lock().expect("lock")
    }
}

impl CacheWarmingDecider for HangingDecider {
    fn decide<'a>(
        &'a self,
        _event: &'a CacheWarmingDecisionEvent,
    ) -> BoxFuture<'a, CacheWarmingAction> {
        *self.calls.lock().expect("lock") += 1;
        Box::pin(async {
            // Longer than any TTL this suite uses, and longer than the refresh deadline.
            tokio::time::sleep(Duration::from_secs(86_400)).await;
            CacheWarmingAction::Warm
        })
    }
}

/// EXT-085: a decision handler is third-party code once the hook is wired, so the decide call is
/// BOUNDED by the time left to this run's refresh deadline.
///
/// pi's `await this.decide(...)` has no deadline, because a hung JS handler still yields its event
/// loop and the runner's own per-handler try/catch is about throws, not hangs. A hung Rust future
/// does not yield: without the bound the refresh task never returns and the run sits armed with no
/// timer, no stop reason and a status stuck on `scheduled` — a single broken extension silently
/// disabling warming for the rest of the session, with nothing in `/session` to say so.
///
/// Past the refresh deadline a "warm" is a full-price cache write anyway, so that deadline is the
/// only bound worth having: on elapse pi's own action stands and the post-decision deadline check
/// stops the run with upstream's `"cache refresh deadline missed"`.
///
/// **Red-proved** by removing the `tokio::time::timeout` wrapper around the decide call: the run
/// came back `Scheduled` where it must be `Inactive`, with `reason: None` — the production symptom
/// exactly, a run frozen mid-refresh that `/session` reports as healthy and that no later real
/// request can replace a stop reason for.
#[tokio::test(start_paused = true)]
async fn ext085_a_hanging_decider_cannot_hang_a_warm() {
    let decider = HangingDecider::new();
    let h = harness(
        CacheWarmingMode::Idle,
        100_000,
        Script::Ok,
        decider.clone() as Arc<dyn CacheWarmingDecider>,
    );
    let (req, _) = session_request(&h.stream);
    h.warmer.start(req, always_current());

    // 270 000 arms the refresh; the deadline is 285 000. Walk past both.
    advance(270_001).await;
    assert_eq!(decider.calls(), 1, "the handler WAS consulted");
    advance(20_000).await;

    assert_eq!(
        h.stream.len(),
        0,
        "nothing is sent: the bound elapsed, pi's own action stood, and the post-decision \
         deadline check stopped the run"
    );
    let status = h.warmer.status().await;
    assert_eq!(status.state, CacheWarmingState::Inactive);
    assert_eq!(
        status.reason.as_deref(),
        Some("cache refresh deadline missed"),
        "a hung handler must leave a STOP REASON a user can read in `/session`, not a run frozen \
         on `scheduled` forever"
    );
    assert!(
        !h.warmer.has_active_run(),
        "the run is cleared, so the next real request starts a fresh one"
    );
}

/// The `cache_warming_decision` hook end to end: a native extension's `{action}` reaches the
/// warmer through [`crate::ExtensionCacheWarmingDecider`] and changes what is sent.
///
/// This is the seam the builder installs, so it is tested through the REAL `ExtensionHost` fold
/// rather than a scripted decider — the adapter is two enum conversions and a `Weak`, and both
/// legs of the conversion are the kind of thing that compiles while mapping `Warm` to `Stop`.
///
/// **Red-proved** by neutering the adapter to `PiDecision`'s body (`event.action` unchanged): the
/// warm was sent and this test failed. Note that swapping the two arms of the OUTBOUND
/// `match event.action` conversion does NOT fail this test, nor the forcing one — whenever an
/// extension answers, the host's own action only feeds the override comparison. That mutant is
/// killed by [`ext085_an_observing_extension_leaves_the_economics_verdict`], which is the only
/// case where the inbound action is the answer.
#[tokio::test(start_paused = true)]
async fn ext085_an_extensions_veto_reaches_the_warmer() {
    use cyrup_ext::{
        EventKind, ExtMode, ExtensionHost, HandledValue, HookOutcome, HostConfig, HostCtx,
        HostEvent, InitApi, NativeExtension,
    };

    struct Veto;
    #[async_trait::async_trait]
    impl NativeExtension for Veto {
        fn id(&self) -> cyrup_core::ExtensionId {
            "veto".into()
        }
        async fn init(&self, api: &mut InitApi) -> Result<(), cyrup_ext::ExtError> {
            api.subscribe(&[EventKind::CacheWarmingDecision]);
            Ok(())
        }
        async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
            match ev {
                HostEvent::CacheWarmingDecision { .. } => {
                    HookOutcome::Handled(HandledValue(serde_json::json!({"action": "stop"})))
                }
                _ => HookOutcome::Noop,
            }
        }
    }

    let ext_host = Arc::new(ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }));
    ext_host
        .load_native(Arc::new(Veto))
        .await
        .expect("native loads");

    let cancel = cyrup_core::CancelToken::new();
    let decider = Arc::new(crate::cache_warmer::ExtensionCacheWarmingDecider::new(
        &ext_host,
        cancel.clone(),
    ));
    // A 100 000-token prompt in the streaming phase clears the threshold easily, so pi's own
    // action here is `Warm` — the extension is the only reason nothing is sent.
    let h = harness(
        CacheWarmingMode::Idle,
        100_000,
        Script::Ok,
        decider as Arc<dyn CacheWarmingDecider>,
    );
    let (req, _) = session_request(&h.stream);
    h.warmer.start(req, always_current());
    advance(270_001).await;

    assert_eq!(
        h.stream.len(),
        0,
        "the extension's `stop` suppressed the warm"
    );
    let status = h.warmer.status().await;
    assert_eq!(
        status.reason.as_deref(),
        Some("stopped by extension"),
        "an override to `stop` reports upstream's own reason, not the economics one"
    );
    assert!(
        status.extension_override,
        "`/session` must say the decision was an extension's"
    );
    assert!(
        h.host.warms().is_empty(),
        "no usage entry for a warm never sent"
    );
}

/// The other direction: an extension forcing `warm` on economics that say `stop` sends ONE request
/// and tags the persisted entry `extension override` (pi `cache-warmer.ts:336-343`).
#[tokio::test(start_paused = true)]
async fn ext085_an_extension_can_force_a_warm_through_the_real_fold() {
    use cyrup_ext::{
        EventKind, ExtMode, ExtensionHost, HandledValue, HookOutcome, HostConfig, HostCtx,
        HostEvent, InitApi, NativeExtension,
    };

    struct Force;
    #[async_trait::async_trait]
    impl NativeExtension for Force {
        fn id(&self) -> cyrup_core::ExtensionId {
            "force".into()
        }
        async fn init(&self, api: &mut InitApi) -> Result<(), cyrup_ext::ExtError> {
            api.subscribe(&[EventKind::CacheWarmingDecision]);
            Ok(())
        }
        async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
            match ev {
                HostEvent::CacheWarmingDecision { .. } => {
                    HookOutcome::Handled(HandledValue(serde_json::json!({"action": "warm"})))
                }
                _ => HookOutcome::Noop,
            }
        }
    }

    let ext_host = Arc::new(ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }));
    ext_host.load_native(Arc::new(Force)).await.expect("loads");
    let decider = Arc::new(crate::cache_warmer::ExtensionCacheWarmingDecider::new(
        &ext_host,
        cyrup_core::CancelToken::new(),
    ));
    // A 5 000-token prompt cannot clear five cents, so pi's own action is `Stop`.
    let h = harness(
        CacheWarmingMode::Idle,
        5_000,
        Script::Ok,
        decider as Arc<dyn CacheWarmingDecider>,
    );
    let (req, _) = session_request(&h.stream);
    h.warmer.start(req, always_current());
    advance(270_001).await;

    assert_eq!(h.stream.len(), 1, "the extension forced the refresh");
    let warms = h.host.warms();
    assert_eq!(warms.len(), 1);
    assert_eq!(
        warms[0].note.as_deref(),
        Some("extension override"),
        "pi tags a forced warm's usage entry, so `/session` and the transcript say whose decision \
         the spend was"
    );
}

/// A torn-down session's extension host is pi's `extensionRunnerRef.current` being absent, which
/// upstream covers with `?? event.action`. The adapter holds the host WEAKLY for exactly that, so
/// a warm that outlives the host still follows the economics instead of panicking or stalling.
#[tokio::test(start_paused = true)]
async fn ext085_a_dropped_extension_host_falls_back_to_pis_own_action() {
    use cyrup_ext::{ExtMode, ExtensionHost, HostConfig};

    let decider = {
        let ext_host = Arc::new(ExtensionHost::new(HostConfig {
            mode: ExtMode::Tui,
            has_ui: true,
            cwd: std::path::PathBuf::from("."),
        }));
        Arc::new(crate::cache_warmer::ExtensionCacheWarmingDecider::new(
            &ext_host,
            cyrup_core::CancelToken::new(),
        ))
        // `ext_host` drops here: the `Weak` is now dead.
    };
    let h = harness(
        CacheWarmingMode::Idle,
        100_000,
        Script::Ok,
        decider as Arc<dyn CacheWarmingDecider>,
    );
    let (req, _) = session_request(&h.stream);
    h.warmer.start(req, always_current());
    advance(270_001).await;

    assert_eq!(
        h.stream.len(),
        1,
        "pi's own `Warm` stands: no host, no override"
    );
    assert_eq!(h.host.warms()[0].note, None, "and no override note");
}

/// The case that pins the OUTBOUND half of the adapter's enum conversion: an extension subscribed
/// to the event but answering `noop` — pi's `result?.action !== undefined` guard — must leave the
/// economics verdict exactly as it was. This is the only path where the host's own action is both
/// the input to the fold and its answer, so it is the only one where mapping `Warm` to
/// `cyrup_ext::CacheWarmingAction::Stop` is visible.
///
/// **Red-proved** by swapping the two arms of the outbound `match event.action` conversion: pi's
/// `Warm` went in as `Stop`, came back as `Stop`, and nothing was sent — a subscribed-but-silent
/// extension would have disabled warming. (Neither the veto nor the forcing test catches that, and
/// both passed under the mutant.)
#[tokio::test(start_paused = true)]
async fn ext085_an_observing_extension_leaves_the_economics_verdict() {
    use cyrup_ext::{
        EventKind, ExtMode, ExtensionHost, HookOutcome, HostConfig, HostCtx, HostEvent, InitApi,
        NativeExtension,
    };

    struct Observe;
    #[async_trait::async_trait]
    impl NativeExtension for Observe {
        fn id(&self) -> cyrup_core::ExtensionId {
            "observe".into()
        }
        async fn init(&self, api: &mut InitApi) -> Result<(), cyrup_ext::ExtError> {
            api.subscribe(&[EventKind::CacheWarmingDecision]);
            Ok(())
        }
        async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
            HookOutcome::Noop
        }
    }

    let ext_host = Arc::new(ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }));
    ext_host
        .load_native(Arc::new(Observe))
        .await
        .expect("loads");
    let decider = Arc::new(crate::cache_warmer::ExtensionCacheWarmingDecider::new(
        &ext_host,
        cyrup_core::CancelToken::new(),
    ));

    // (a) economics say WARM: a silent handler must not suppress it.
    let warm = harness(
        CacheWarmingMode::Idle,
        100_000,
        Script::Ok,
        decider.clone() as Arc<dyn CacheWarmingDecider>,
    );
    let (req, _) = session_request(&warm.stream);
    warm.warmer.start(req, always_current());
    advance(270_001).await;
    assert_eq!(warm.stream.len(), 1, "the economics verdict stands");
    assert_eq!(
        warm.host.warms()[0].note,
        None,
        "and it is NOT an extension override"
    );

    // (b) economics say STOP: a silent handler must not force one either.
    let stop = harness(
        CacheWarmingMode::Idle,
        5_000,
        Script::Ok,
        decider as Arc<dyn CacheWarmingDecider>,
    );
    let (req, _) = session_request(&stop.stream);
    stop.warmer.start(req, always_current());
    advance(270_001).await;
    assert_eq!(stop.stream.len(), 0, "nothing forced");
    assert_eq!(
        stop.warmer.status().await.reason.as_deref(),
        Some("expected savings below threshold"),
        "the stop reason is the ECONOMICS one, not `stopped by extension` — a silent handler is \
         not an override"
    );
}
