//! EXT-085 — the `cache_warming_decision` hook, both tiers.
//!
//! pi `CacheWarmingDecisionEvent` / `CacheWarmingDecisionEventResult`
//! (`core/cache-warmer.ts:112-120` @v1.0.4), reduced by `emitCacheWarmingDecision`
//! (`core/extensions/runner.ts:1121-1142`): `let action = event.action`, then every handler of
//! every extension runs in snapshot order and `if (result?.action !== undefined) action =
//! result.action` — the LAST override wins, and a handler that throws goes to `emitError` and the
//! loop CONTINUES, so a faulting handler cannot change the action.
//!
//! The rule is the OPPOSITE of `project_trust`'s next door in [`super::aggregation`], which is why
//! these tests exist: the two folds live in the same file and a reader who assumes symmetry gets
//! first-wins plus a silently skipped handler.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::{Arc, Mutex};

use crate::{
    CacheWarmingAction, EventKind, ExtMode, ExtensionHost, HandledValue, HookOutcome, HostConfig,
    HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_core::{CancelToken, ExtensionId};
use serde_json::{Value, json};

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

/// How a native handler answers: a fixed `handled` payload, a `noop`, or a FAULT (an `Err` out of
/// `on_event`'s contained invocation — pi's throwing handler).
enum Answer {
    Handled(Value),
    Noop,
    Fault,
}

/// A native extension subscribed to `cache_warming_decision` that records being called and answers
/// with a scripted [`Answer`].
struct Decider {
    id: ExtensionId,
    answer: Answer,
    /// Every extension id that ran, in order — so a skipped handler is visible, not inferred.
    seen: Arc<Mutex<Vec<String>>>,
    /// The payload the handler was given, for the shape assertions.
    got: Arc<Mutex<Option<HostEvent>>>,
}

fn decider(id: &str, answer: Answer, seen: &Arc<Mutex<Vec<String>>>) -> Arc<Decider> {
    Arc::new(Decider {
        id: id.into(),
        answer,
        seen: seen.clone(),
        got: Arc::new(Mutex::new(None)),
    })
}

#[async_trait::async_trait]
impl NativeExtension for Decider {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.subscribe(&[EventKind::CacheWarmingDecision]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if !matches!(ev, HostEvent::CacheWarmingDecision { .. }) {
            return HookOutcome::Noop;
        }
        self.seen.lock().unwrap().push(self.id.to_string());
        *self.got.lock().unwrap() = Some(ev.clone());
        match &self.answer {
            Answer::Handled(v) => HookOutcome::Handled(HandledValue(v.clone())),
            Answer::Noop => HookOutcome::Noop,
            // A native handler's panic is contained by the dispatcher; panicking here is the
            // closest native shape of pi's throwing handler, and the dispatcher must SKIP it.
            Answer::Fault => panic!("this handler faults on purpose"),
        }
    }
}

/// Dispatch one decision with pi's own action set to `host_action`.
async fn decide(host: &ExtensionHost, host_action: CacheWarmingAction) -> CacheWarmingAction {
    host.aggregate_cache_warming_decision(0.004, 0.42, 1.0, host_action, &CancelToken::new())
        .await
}

// ---------------------------------------------------------------------------
// The discriminant and the catalog.
// ---------------------------------------------------------------------------

#[test]
fn the_kind_is_36_and_count_grew_with_it() {
    assert_eq!(
        EventKind::from_u8(36),
        Some(EventKind::CacheWarmingDecision)
    );
    assert_eq!(
        EventKind::CacheWarmingDecision.name(),
        "cache_warming_decision"
    );
    assert_eq!(
        EventKind::COUNT,
        37,
        "COUNT indexes the 64-bit subscription bitset: left at 36 a guest's `subscribe(36)` is \
         dropped by the gate and the hook never fires"
    );
    const { assert!(EventKind::COUNT <= 64, "the bitset holds 64 kinds") }
}

/// Every kind below COUNT must round-trip, or a guest's `subscribe` byte is silently dropped.
#[test]
fn every_kind_below_count_parses() {
    for v in 0..EventKind::COUNT {
        assert!(
            EventKind::from_u8(v).is_some(),
            "EventKind::from_u8({v}) is None but COUNT is {}",
            EventKind::COUNT
        );
    }
    assert_eq!(EventKind::from_u8(EventKind::COUNT), None);
}

// ---------------------------------------------------------------------------
// The fold: last readable action wins.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_last_extension_to_answer_wins() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let host = ExtensionHost::new(cfg());
    host.load_native(decider(
        "first",
        Answer::Handled(json!({"action": "warm"})),
        &seen,
    ))
    .await
    .unwrap();
    host.load_native(decider(
        "last",
        Answer::Handled(json!({"action": "stop"})),
        &seen,
    ))
    .await
    .unwrap();

    assert_eq!(
        decide(&host, CacheWarmingAction::Warm).await,
        CacheWarmingAction::Stop,
        "pi assigns `action = result.action` INSIDE the loop and never returns early, so the LAST \
         override wins — not the first, as `project_trust` does"
    );
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["first".to_string(), "last".to_string()],
        "EVERY handler runs: upstream has no short-circuit here, and an extension that answers \
         only to observe the economics must still be called"
    );
}

#[tokio::test]
async fn an_extension_can_force_a_warm_the_host_would_have_skipped() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let host = ExtensionHost::new(cfg());
    host.load_native(decider(
        "forcer",
        Answer::Handled(json!({"action": "warm"})),
        &seen,
    ))
    .await
    .unwrap();
    assert_eq!(
        decide(&host, CacheWarmingAction::Stop).await,
        CacheWarmingAction::Warm,
        "the override goes both ways: this is the half that makes the warmer tag the persisted \
         usage entry `extension override`"
    );
}

#[tokio::test]
async fn no_extension_leaves_the_hosts_own_action() {
    let host = ExtensionHost::new(cfg());
    assert_eq!(
        decide(&host, CacheWarmingAction::Warm).await,
        CacheWarmingAction::Warm
    );
    assert_eq!(
        decide(&host, CacheWarmingAction::Stop).await,
        CacheWarmingAction::Stop,
        "pi's `decide` default is `async (event) => event.action` (`cache-warmer.ts:177`)"
    );
}

#[tokio::test]
async fn a_noop_handler_is_no_opinion() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let host = ExtensionHost::new(cfg());
    host.load_native(decider("quiet", Answer::Noop, &seen))
        .await
        .unwrap();
    assert_eq!(
        decide(&host, CacheWarmingAction::Warm).await,
        CacheWarmingAction::Warm,
        "pi's guard is `result?.action !== undefined`: a handler that returns nothing changes \
         nothing"
    );
    assert_eq!(*seen.lock().unwrap(), vec!["quiet".to_string()]);
}

#[tokio::test]
async fn an_unreadable_action_is_no_opinion_and_does_not_displace_an_earlier_one() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let host = ExtensionHost::new(cfg());
    host.load_native(decider(
        "opinion",
        Answer::Handled(json!({"action": "stop"})),
        &seen,
    ))
    .await
    .unwrap();
    // Every shape a sloppy guest can produce: a third action name, a non-string, and no key.
    host.load_native(decider(
        "skip",
        Answer::Handled(json!({"action": "skip"})),
        &seen,
    ))
    .await
    .unwrap();
    host.load_native(decider(
        "numeric",
        Answer::Handled(json!({"action": 1})),
        &seen,
    ))
    .await
    .unwrap();
    host.load_native(decider("empty", Answer::Handled(json!({})), &seen))
        .await
        .unwrap();

    assert_eq!(
        decide(&host, CacheWarmingAction::Warm).await,
        CacheWarmingAction::Stop,
        "`CacheWarmingAction` is `\"warm\" | \"stop\"` at v1.0.4 and nothing else: an action the \
         union does not have is IGNORED rather than forwarded, so a sloppy guest cannot make the \
         host act under a third name — and it must not silently cancel an earlier handler's real \
         answer either"
    );
    assert_eq!(
        seen.lock().unwrap().len(),
        4,
        "all four still ran — ignoring an answer is not skipping a handler"
    );
}

#[tokio::test]
async fn a_faulting_handler_is_skipped_and_the_rest_still_run() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let host = ExtensionHost::new(cfg());
    host.load_native(decider("broken", Answer::Fault, &seen))
        .await
        .unwrap();
    host.load_native(decider(
        "after",
        Answer::Handled(json!({"action": "stop"})),
        &seen,
    ))
    .await
    .unwrap();

    assert_eq!(
        decide(&host, CacheWarmingAction::Warm).await,
        CacheWarmingAction::Stop,
        "pi routes a handler's throw to `emitError` and CONTINUES the loop, so a broken extension \
         neither changes the action nor stops the extension after it"
    );
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["broken".to_string(), "after".to_string()]
    );
}

#[tokio::test]
async fn a_faulting_handler_alone_leaves_the_hosts_action() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let host = ExtensionHost::new(cfg());
    host.load_native(decider("broken", Answer::Fault, &seen))
        .await
        .unwrap();
    assert_eq!(
        decide(&host, CacheWarmingAction::Warm).await,
        CacheWarmingAction::Warm,
        "a broken extension must not be able to disable prompt-cache warming"
    );
}

// ---------------------------------------------------------------------------
// The payload: pi's `Pick` of exactly four fields.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_native_payload_carries_the_four_picked_fields() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let host = ExtensionHost::new(cfg());
    let ext = decider("observer", Answer::Noop, &seen);
    host.load_native(ext.clone()).await.unwrap();
    host.aggregate_cache_warming_decision(
        0.004_25,
        0.512,
        0.15,
        CacheWarmingAction::Stop,
        &CancelToken::new(),
    )
    .await;

    let got = ext.got.lock().unwrap().clone().expect("the handler ran");
    match got {
        HostEvent::CacheWarmingDecision {
            warm_cost,
            miss_cost,
            continuation_probability,
            action,
        } => {
            assert!((warm_cost - 0.004_25).abs() < f64::EPSILON);
            assert!((miss_cost - 0.512).abs() < f64::EPSILON);
            assert!((continuation_probability - 0.15).abs() < f64::EPSILON);
            assert_eq!(action, "stop", "pi's own verdict, as a handler sees it");
        }
        other => panic!("wrong event: {other:?}"),
    }
}

#[test]
fn the_guest_payload_is_exactly_pis_four_camel_case_keys() {
    let json_str = crate::host::live::cache_warming_decision_json(0.004, 0.42, 0.15, "warm");
    let v: Value = serde_json::from_str(&json_str).unwrap();
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["action", "continuationProbability", "missCost", "warmCost"],
        "pi's `CacheWarmingDecisionEvent` is a `Pick` of exactly these four, camelCase. `phase`, \
         `expectedSavings` and `economicsAvailable` are withheld ON PURPOSE — \"Everything else an \
         extension might want (model, idle state, context size) is on the context\" \
         (`cache-warmer.ts:109-111`). Widening this is not a compile error and extensions would \
         start depending on a field upstream does not promise."
    );
    assert_eq!(v["action"], json!("warm"));
    assert_eq!(v["continuationProbability"], json!(0.15));
}

// ---------------------------------------------------------------------------
// The fold in isolation, over the raw handled values.
// ---------------------------------------------------------------------------

#[test]
fn the_fold_takes_the_last_parseable_action() {
    let handled = vec![
        (
            ExtensionId::from("a"),
            HandledValue(json!({"action": "stop"})),
        ),
        (
            ExtensionId::from("b"),
            HandledValue(json!({"action": "warm"})),
        ),
        // Unreadable: must not displace `b`.
        (
            ExtensionId::from("c"),
            HandledValue(json!({"action": "maybe"})),
        ),
    ];
    assert_eq!(
        crate::fold_cache_warming_decision(&handled),
        Some((ExtensionId::from("b"), CacheWarmingAction::Warm))
    );
    assert_eq!(crate::fold_cache_warming_decision(&[]), None);
}

#[test]
fn the_action_parser_reads_only_pis_two_values() {
    assert_eq!(
        crate::parse_cache_warming_action(&json!({"action": "warm"})),
        Some(CacheWarmingAction::Warm)
    );
    assert_eq!(
        crate::parse_cache_warming_action(&json!({"action": "stop"})),
        Some(CacheWarmingAction::Stop)
    );
    for bad in [
        json!({"action": "Warm"}),
        json!({"action": "skip"}),
        json!({"action": true}),
        json!({"trusted": "yes"}),
        json!("warm"),
    ] {
        assert_eq!(
            crate::parse_cache_warming_action(&bad),
            None,
            "not pi's union: {bad}"
        );
    }
    assert_eq!(CacheWarmingAction::Warm.as_str(), "warm");
    assert_eq!(CacheWarmingAction::Stop.as_str(), "stop");
}

// ---------------------------------------------------------------------------
// The world bump: a 0.17 guest has no such export and must be REFUSED, not linked.
// ---------------------------------------------------------------------------

#[test]
fn a_guest_built_before_the_cache_warming_export_is_refused() {
    let m = crate::ExtensionManifest {
        id: "pre-ext-085".into(),
        version: "1.0.0".into(),
        world: "cyrup:ext@0.17".into(),
        entry: None,
        capabilities: Default::default(),
    };
    let err = m.check_world(crate::HOST_WORLD).unwrap_err();
    assert!(
        matches!(&err, crate::ExtError::WorldVersion { found, required }
            if found == "cyrup:ext@0.17" && required == crate::HOST_WORLD),
        "`events.on-cache-warming-decision` is an EXPORT ADDITION: a 0.17 guest exports nothing \
         under that name and would die inside wasmtime on the link. The gate must turn that into a \
         typed `ExtError::WorldVersion` — which is only possible because HOST_WORLD moved with the \
         WIT package line: {err:?}"
    );
    assert_eq!(crate::HOST_WORLD, "cyrup:ext@0.18");
}

// ---------------------------------------------------------------------------
// The GUEST tier, through a real component: the new `events.on-cache-warming-decision` export is
// subscribable, reached, and its `handled` answer folds like a native's.
// ---------------------------------------------------------------------------

#[cfg(feature = "wasm-host")]
mod wasm {
    use super::*;
    use crate::manifest::Capabilities;
    use crate::tests::wat_guest::{Lowered, REGISTRATION_FLAG_AND_UNSUBSCRIBE, WatGuest, wat_str};

    fn le32(v: u32) -> String {
        v.to_le_bytes()
            .iter()
            .map(|b| format!("\\{b:02x}"))
            .collect()
    }

    const SUBSCRIBE: Lowered = Lowered {
        core_name: "subscribe",
        component_func: "$subscribe",
        core_sig: "(param i32 i32)",
        needs_realloc: false,
    };

    /// A guest that subscribes `cache_warming_decision` and answers
    /// `hook-outcome::handled(<answer>)`.
    fn answering_guest(answer: &str) -> Vec<u8> {
        WatGuest {
            component: REGISTRATION_FLAG_AND_UNSUBSCRIBE.to_string(),
            lowered: vec![SUBSCRIBE],
            overrides: vec![
                (
                    "init",
                    "    (func (export \"init\") (result i32) \
                     (call $subscribe (i32.const 16384) (i32.const 1)) i32.const 16"
                        .to_string()
                        + ")",
                ),
                (
                    "on-cache-warming-decision",
                    "    (func (export \"on-cache-warming-decision\") (param i32 i32) (result i32) \
                     i32.const 16400)"
                        .to_string(),
                ),
            ],
            data: vec![
                (
                    16384,
                    format!("\\{:02x}", EventKind::CacheWarmingDecision as u8),
                ),
                // hook-outcome is `noop | block | mutate | handled`, so `handled` is case 3; the
                // payload string pointer+len sit at +4 (the `mutate` layout `post_baseline_events`
                // uses with case 2).
                (
                    16400,
                    format!(
                        "\\03\\00\\00\\00{}{}",
                        le32(16416),
                        le32(answer.len() as u32)
                    ),
                ),
                (16416, wat_str(answer)),
            ],
        }
        .build()
    }

    #[tokio::test]
    async fn a_guests_handled_action_overrides_the_hosts_verdict() {
        let host = ExtensionHost::with_wasm(cfg()).unwrap();
        host.load_wasm_with_caps(
            "warm-guest".into(),
            &answering_guest(r#"{"action":"stop"}"#),
            Arc::new(crate::DenyServices),
            &Capabilities::host_granted(),
        )
        .await
        .unwrap();

        assert_eq!(
            decide(&host, CacheWarmingAction::Warm).await,
            CacheWarmingAction::Stop,
            "the guest tier reaches the SAME fold as the native tier — this is the leg the world \
             bump bought, and the only one that proves `events.on-cache-warming-decision` is \
             subscribable (kind 36 must survive `EventKind::from_u8`), reached by the dispatcher, \
             and decoded back through `hook-outcome::handled`"
        );
    }

    #[tokio::test]
    async fn a_guest_forcing_warm_reaches_the_fold_too() {
        let host = ExtensionHost::with_wasm(cfg()).unwrap();
        host.load_wasm_with_caps(
            "warm-guest".into(),
            &answering_guest(r#"{"action":"warm"}"#),
            Arc::new(crate::DenyServices),
            &Capabilities::host_granted(),
        )
        .await
        .unwrap();
        assert_eq!(
            decide(&host, CacheWarmingAction::Stop).await,
            CacheWarmingAction::Warm
        );
    }

    #[tokio::test]
    async fn a_guests_unreadable_action_leaves_the_hosts_verdict() {
        let host = ExtensionHost::with_wasm(cfg()).unwrap();
        host.load_wasm_with_caps(
            "sloppy-guest".into(),
            &answering_guest(r#"{"action":"skip"}"#),
            Arc::new(crate::DenyServices),
            &Capabilities::host_granted(),
        )
        .await
        .unwrap();
        // The host's own action is `Stop` here so a forwarded third name would be VISIBLE: it
        // would have to read as `Warm` and send a request the economics refused.
        assert_eq!(
            decide(&host, CacheWarmingAction::Stop).await,
            CacheWarmingAction::Stop,
            "EXT-085's ledger Verify clause names `{{action: \"skip\"}}` as suppressing the warm; \
             that is WRONG against v1.0.4, where the union is `\"warm\" | \"stop\"` and nothing \
             else. A third name is ignored, not honoured."
        );
    }
}
