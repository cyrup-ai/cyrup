//! `SESS-067` — the branch-selection hold rule and `pi.virtual-model-state`.
//!
//! Upstream citations are pi @ **v1.0.4**, `packages/coding-agent/src/core/virtual-models.ts`
//! (byte-identical at v1.0.0 and v1.0.1), taken through git objects.
//!
//! Each test drives the real [`SessionManager`], so the walk under test is the one production
//! uses: `branch_path(None)`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;

use cyrup_core::{AssistantMessage, Content, Message, ModelRef, StopReason, Usage};
use serde_json::json;

use crate::virtual_models::{
    VIRTUAL_MODEL_API, VIRTUAL_MODEL_STATE_ENTRY, branch_selection, latest_branch_response,
    virtual_model_state,
};
use crate::{NewSessionOpts, SessionManager};

fn mgr() -> SessionManager {
    SessionManager::in_memory(&PathBuf::from("/proj/vm"), NewSessionOpts::default()).unwrap()
}

fn user(s: &str) -> Message {
    Message::User {
        content: vec![Content::text(s)],
        timestamp: 0,
    }
}

/// An assistant message naming the model that actually answered. `api` is what
/// `isVirtualModel` reads, so a router's own failed attempt is built with
/// [`VIRTUAL_MODEL_API`] here.
fn answered(provider: &str, model: &str, api: &str) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![Content::text("ok")],
        provider: provider.into(),
        model: model.into(),
        api: api.into(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        thinking_level: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    })
}

/// A catalog lookup that reports exactly the named provider/model as virtual.
fn knows_virtual(
    provider: &'static str,
    model: &'static str,
) -> impl Fn(&str, &str) -> Option<ModelRef> {
    move |p: &str, m: &str| {
        (p == provider && m == model).then(|| ModelRef {
            provider: p.into(),
            api: Some(VIRTUAL_MODEL_API.into()),
            model: m.into(),
        })
    }
}

/// A catalog that knows nothing — every lookup misses, as after a router extension unloads.
fn knows_nothing(_p: &str, _m: &str) -> Option<ModelRef> {
    None
}

// ------------------------------------------------------------------ the hold rule --------------

/// `getBranchSelection` (`virtual-models.ts:128-145`): a virtual `model_change` HOLDS across the
/// physical responses it routed to.
#[test]
fn a_virtual_model_change_holds_over_the_physical_model_that_answered() {
    let mut m = mgr();
    m.append_message(user("hi")).unwrap();
    m.append_model_change("router".into(), "auto".into())
        .unwrap();
    m.append_message(answered("anthropic", "claude-opus-5", "anthropic-messages"))
        .unwrap();

    let path = m.branch_path(None);
    let sel = branch_selection(&path, knows_virtual("router", "auto")).expect("a selection");

    assert_eq!(
        sel.provider.as_str(),
        "router",
        "the virtual selection holds"
    );
    assert_eq!(sel.model.as_str(), "auto");
    assert!(sel.api.is_none(), "a model_change names no api");
}

/// Same branch, same walk, but the router is no longer registered: *"A virtual model that is no
/// longer registered does not hold, so the selection falls back to the physical model that
/// answered last."*
#[test]
fn an_unregistered_virtual_model_does_not_hold_and_the_last_response_wins() {
    let mut m = mgr();
    m.append_message(user("hi")).unwrap();
    m.append_model_change("router".into(), "auto".into())
        .unwrap();
    m.append_message(answered("anthropic", "claude-opus-5", "anthropic-messages"))
        .unwrap();

    let path = m.branch_path(None);
    let sel = branch_selection(&path, knows_nothing).expect("a selection");

    assert_eq!(sel.provider.as_str(), "anthropic");
    assert_eq!(sel.model.as_str(), "claude-opus-5");
}

/// With no virtual model anywhere the rule must be the CURRENT forward last-wins rule, which is
/// what makes landing this ahead of the routing step unobservable (`SESS-067`'s Fix).
#[test]
fn without_virtual_models_the_latest_physical_response_wins() {
    let mut m = mgr();
    m.append_model_change("openai".into(), "gpt-5".into())
        .unwrap();
    m.append_message(answered("openai", "gpt-5", "openai-completions"))
        .unwrap();
    m.append_message(answered("anthropic", "claude-opus-5", "anthropic-messages"))
        .unwrap();

    let path = m.branch_path(None);
    let sel = branch_selection(&path, knows_nothing).expect("a selection");

    assert_eq!(
        sel.provider.as_str(),
        "anthropic",
        "the later response wins"
    );
    assert_eq!(sel.model.as_str(), "claude-opus-5");
}

/// The backwards walk returns a `model_change` immediately (`:131-133`), so a manual switch after
/// a response wins over that response — virtual or not.
#[test]
fn a_model_change_after_a_response_wins_immediately() {
    let mut m = mgr();
    m.append_message(answered("anthropic", "claude-opus-5", "anthropic-messages"))
        .unwrap();
    m.append_model_change("openai".into(), "gpt-5".into())
        .unwrap();

    let path = m.branch_path(None);
    let sel = branch_selection(&path, knows_nothing).expect("a selection");

    assert_eq!(sel.provider.as_str(), "openai");
    assert_eq!(sel.model.as_str(), "gpt-5");
}

/// *"Failed routing leaves the virtual model on its message"* (`:101`), so an assistant message
/// carrying the virtual api is NOT a physical response and must not end the walk.
///
/// Built with NO `model_change` on the branch and a catalog that knows nothing, so the hold rule
/// cannot mask the difference: skipping the virtual message reaches the physical response behind
/// it, while treating it as a response would return the virtual model itself.
#[test]
fn an_assistant_message_from_failed_routing_is_skipped() {
    let mut m = mgr();
    m.append_message(answered("openai", "gpt-5", "openai-completions"))
        .unwrap();
    // The router itself failed: the message names the virtual model.
    m.append_message(answered("router", "auto", VIRTUAL_MODEL_API))
        .unwrap();

    let path = m.branch_path(None);
    let sel = branch_selection(&path, knows_nothing).expect("a selection");

    assert_eq!(
        sel.provider.as_str(),
        "openai",
        "the failed attempt is skipped, so the physical response behind it is the selection"
    );
    assert_eq!(sel.model.as_str(), "gpt-5");
}

/// And the same skip inside the hold rule: a virtual `model_change` still holds when the newest
/// message is a failed routing attempt.
#[test]
fn a_failed_routing_attempt_does_not_break_the_hold() {
    let mut m = mgr();
    m.append_model_change("router".into(), "auto".into())
        .unwrap();
    m.append_message(answered("anthropic", "claude-opus-5", "anthropic-messages"))
        .unwrap();
    m.append_message(answered("router", "auto", VIRTUAL_MODEL_API))
        .unwrap();

    let path = m.branch_path(None);
    let sel = branch_selection(&path, knows_virtual("router", "auto")).expect("a selection");

    assert_eq!(sel.model.as_str(), "auto", "the virtual change still holds");
    assert!(
        sel.api.is_none(),
        "it is the model_change, not the failed message"
    );
}

/// An empty branch has no selection (`:144`).
#[test]
fn an_empty_branch_has_no_selection() {
    let m = mgr();
    let path = m.branch_path(None);
    assert!(branch_selection(&path, knows_nothing).is_none());
}

// --------------------------------------------------------------- router state ------------------

/// `getVirtualModelState` (`:159-167`): the LATEST matching entry wins.
#[test]
fn the_latest_state_entry_for_a_provider_and_model_wins() {
    let mut m = mgr();
    m.append_custom_entry(
        VIRTUAL_MODEL_STATE_ENTRY,
        Some(json!({"provider": "router", "modelId": "auto", "state": {"n": 1}})),
    )
    .unwrap();
    m.append_custom_entry(
        VIRTUAL_MODEL_STATE_ENTRY,
        Some(json!({"provider": "router", "modelId": "auto", "state": {"n": 2}})),
    )
    .unwrap();

    let path = m.branch_path(None);
    let state = virtual_model_state(&path, "router", "auto").expect("a state");

    assert_eq!(state, &json!({"n": 2}), "the later entry wins");
}

/// The match is on BOTH `provider` and `modelId` inside `data` (`:165`).
#[test]
fn state_is_scoped_to_its_provider_and_model() {
    let mut m = mgr();
    m.append_custom_entry(
        VIRTUAL_MODEL_STATE_ENTRY,
        Some(json!({"provider": "router", "modelId": "auto", "state": "mine"})),
    )
    .unwrap();

    let path = m.branch_path(None);
    assert_eq!(
        virtual_model_state(&path, "router", "auto"),
        Some(&json!("mine"))
    );
    assert!(
        virtual_model_state(&path, "router", "other").is_none(),
        "a different model id does not match"
    );
    assert!(
        virtual_model_state(&path, "other", "auto").is_none(),
        "a different provider does not match"
    );
}

/// A `custom` entry of another type, and one with no `data`, are skipped rather than ending the
/// walk — upstream's `continue` and `data?.` (`:162-166`).
#[test]
fn unrelated_and_dataless_custom_entries_are_skipped() {
    let mut m = mgr();
    m.append_custom_entry(
        VIRTUAL_MODEL_STATE_ENTRY,
        Some(json!({"provider": "router", "modelId": "auto", "state": "kept"})),
    )
    .unwrap();
    m.append_custom_entry("pi.something-else", Some(json!({"provider": "router"})))
        .unwrap();
    m.append_custom_entry(VIRTUAL_MODEL_STATE_ENTRY, None)
        .unwrap();

    let path = m.branch_path(None);
    assert_eq!(
        virtual_model_state(&path, "router", "auto"),
        Some(&json!("kept")),
        "the walk passed over both and found the real entry"
    );
}

/// State is BRANCH-scoped: an entry on an abandoned branch is invisible after a `/tree`
/// navigation, because the walk is over the active path.
#[test]
fn state_on_an_abandoned_branch_is_invisible_after_navigation() {
    let mut m = mgr();
    let fork = m.append_message(user("shared")).unwrap();
    m.append_custom_entry(
        VIRTUAL_MODEL_STATE_ENTRY,
        Some(json!({"provider": "router", "modelId": "auto", "state": "abandoned"})),
    )
    .unwrap();

    // /tree back to the fork point: the state entry above is now off-path.
    m.branch(&fork).unwrap();

    let path = m.branch_path(None);
    assert!(
        virtual_model_state(&path, "router", "auto").is_none(),
        "the abandoned branch's state is not read"
    );

    // A state entry on the new branch is found.
    m.append_custom_entry(
        VIRTUAL_MODEL_STATE_ENTRY,
        Some(json!({"provider": "router", "modelId": "auto", "state": "live"})),
    )
    .unwrap();
    let path = m.branch_path(None);
    assert_eq!(
        virtual_model_state(&path, "router", "auto"),
        Some(&json!("live"))
    );
}

/// A branch with no state entry at all.
#[test]
fn a_branch_with_no_state_entry_reports_none() {
    let mut m = mgr();
    m.append_message(user("hi")).unwrap();
    let path = m.branch_path(None);
    assert!(virtual_model_state(&path, "router", "auto").is_none());
}

// ------------------------------------------------- the lookup bound + the manager accessor ------

/// Upstream's own `#10198` regression guard — *"the selection must not cost one catalog lookup per
/// assistant message"* (`test/virtual-models.test.ts:66-87`).
///
/// Only the LAST `model_change` can hold, which is what
/// [`crate::virtual_models::branch_selection`]'s doc claims, so a branch with a hundred responses
/// must cost exactly ONE lookup — and in the physical shape that lookup is of the `model_change`
/// that did NOT win.
///
/// RED-PROVE: hoist `find_last_model_change`'s call out of the response arm so the response itself
/// is also looked up, or call `get_model` once per entry while walking — the counts become 101 and
/// 2 respectively and both assertions fail.
#[test]
fn looks_up_only_the_last_model_change() {
    use std::cell::RefCell;

    let counting = |m: &SessionManager| {
        let seen = RefCell::new(Vec::<String>::new());
        let selection = branch_selection(&m.branch_path(None), |p, id| {
            seen.borrow_mut().push(format!("{p}/{id}"));
            knows_virtual("router", "auto")(p, id)
        });
        (selection, seen.into_inner())
    };

    // Physical shape: a `model_change` to `faux/small`, then a hundred `faux/large` answers. The
    // response wins, and the one lookup is of `faux/small`.
    let mut m = mgr();
    m.append_model_change("faux".into(), "small".into())
        .unwrap();
    for _ in 0..100 {
        m.append_message(answered("faux", "large", "openai-completions"))
            .unwrap();
    }
    let (selection, seen) = counting(&m);
    let sel = selection.expect("a selection");
    assert_eq!(sel.provider.as_str(), "faux");
    assert_eq!(sel.model.as_str(), "large");
    assert_eq!(
        seen,
        vec!["faux/small".to_string()],
        "exactly one lookup, of the last model_change"
    );

    // Routed shape: the held virtual `model_change`, then a hundred answers. Same single lookup.
    let mut m = mgr();
    m.append_model_change("faux".into(), "small".into())
        .unwrap();
    m.append_message(answered("faux", "small", "openai-completions"))
        .unwrap();
    m.append_model_change("router".into(), "auto".into())
        .unwrap();
    for _ in 0..100 {
        m.append_message(answered("faux", "large", "openai-completions"))
            .unwrap();
    }
    let (selection, seen) = counting(&m);
    let sel = selection.expect("a selection");
    assert_eq!(sel.provider.as_str(), "router");
    assert_eq!(sel.model.as_str(), "auto");
    assert_eq!(
        seen,
        vec!["router/auto".to_string()],
        "exactly one lookup, of the held virtual model_change"
    );
}

/// [`SessionManager::branch_selection`] is the free function over `branch_path(None)` — pi's
/// `getBranchSelection(sessionManager.getBranch(), …)` call shape (`sdk.ts:207-211`,
/// `agent-session.ts:620`), and the accessor the restore path and `_recordSelection` both use.
///
/// RED-PROVE: have the accessor walk `entries()` instead of `branch_path(None)` — the off-branch
/// answer below is then returned and the assertion fails.
#[test]
fn the_manager_accessor_walks_the_active_branch_only() {
    let mut m = mgr();
    let root = m.append_message(user("hi")).unwrap();
    m.append_model_change("router".into(), "auto".into())
        .unwrap();
    m.append_message(answered("faux", "large", "openai-completions"))
        .unwrap();
    assert_eq!(
        m.branch_selection(knows_virtual("router", "auto"))
            .expect("a selection")
            .provider
            .as_str(),
        "router",
        "the held virtual selection, through the accessor"
    );

    // Navigate back to the root and answer on a new branch with a DIFFERENT physical model. The
    // abandoned branch's `model_change` and response must be invisible.
    m.branch(&root).unwrap();
    m.append_message(answered("faux", "small", "openai-completions"))
        .unwrap();
    let sel = m
        .branch_selection(knows_virtual("router", "auto"))
        .expect("a selection");
    assert_eq!(sel.provider.as_str(), "faux");
    assert_eq!(
        sel.model.as_str(),
        "small",
        "the active branch has no model_change at all, so the response wins"
    );
}

/// `SessionManager::build_context`'s model pass is DELIBERATELY still the forward last-wins scan,
/// because upstream's `getSessionContextSettings` is byte-identical at v0.87.1, v1.0.0 and v1.0.4
/// (`sha256 1f011ecc8400ee6a…` over `session-manager.ts:418-433` at all three) and
/// `existingSession.model` has no reader anywhere in `packages/coding-agent/src` at v1.0.4. This
/// pins the divergence BOTH ways, so a future reader neither "repairs" the projection toward the
/// selection nor assumes the two are interchangeable.
///
/// RED-PROVE: make `build_context` report `branch_selection`'s answer instead — the first
/// assertion fails with `router` where `faux` is required. (That is the change `SESS-067`'s Fix
/// clause asks for and which the byte evidence above says upstream never made.)
#[test]
fn build_context_still_reports_the_model_that_answered() {
    let mut m = mgr();
    m.append_message(user("hi")).unwrap();
    m.append_model_change("router".into(), "auto".into())
        .unwrap();
    m.append_message(answered("faux", "large", "openai-completions"))
        .unwrap();

    let projected = m.build_context().model.expect("a projected model");
    assert_eq!(projected.provider.as_str(), "faux");
    assert_eq!(
        projected.model.as_str(),
        "large",
        "the projection reports the model that ANSWERED (pi getSessionContextSettings:418-433)"
    );
    assert_eq!(
        m.branch_selection(knows_virtual("router", "auto"))
            .expect("a selection")
            .provider
            .as_str(),
        "router",
        "…while the SELECTION is the router that held — the two differ, and must"
    );
}

/// The same branch with NOTHING registered: the two agree exactly. This is the
/// "a session with no virtual models behaves as before" pin at the `cyrup-session` level — the
/// projection and the selection are the same answer, so switching a caller from one to the other
/// is observationally a no-op for every session that has no virtual models.
///
/// RED-PROVE: make `branch_selection` return the last `model_change` unconditionally (drop the
/// response arm) — the selection becomes `router/auto` while the projection stays `faux/large`,
/// and the equality fails.
#[test]
fn without_virtual_models_the_selection_equals_the_projection() {
    let mut m = mgr();
    m.append_message(user("hi")).unwrap();
    m.append_model_change("router".into(), "auto".into())
        .unwrap();
    m.append_message(answered("faux", "large", "openai-completions"))
        .unwrap();
    m.append_message(user("again")).unwrap();
    m.append_message(answered("faux", "small", "openai-completions"))
        .unwrap();

    let projected = m.build_context().model.expect("a projected model");
    let selection = m.branch_selection(knows_nothing).expect("a selection");
    assert_eq!(projected.provider, selection.provider);
    assert_eq!(projected.model, selection.model);
    assert_eq!(selection.model.as_str(), "small");
}

// ------------------------------------------------- the latest response (the limits anchor) -----

/// Like [`answered`] but with a chosen stop reason, so the `findLatestResponse` skip is testable.
fn answered_with(provider: &str, model: &str, api: &str, stop_reason: StopReason) -> Message {
    let Message::Assistant(mut a) = answered(provider, model, api) else {
        unreachable!("answered builds an assistant message")
    };
    a.stop_reason = stop_reason;
    Message::Assistant(a)
}

/// `findLatestResponse` (`virtual-models.ts:109-118`) skips an ERRORED and an ABORTED response and
/// returns the newest SETTLED one. This is the anchor of the whole limits rule: whatever this
/// answers is the model whose context window the conversation is measured against.
///
/// RED-PROVE: drop the `StopReason::Error` arm of the skip in
/// [`crate::virtual_models::latest_branch_response`] — the walk then returns the errored
/// `faux/broken` turn instead of `faux/large`, and the provider/model assertions fail. Drop the
/// `Aborted` arm instead and it returns `faux/stopped`.
#[test]
fn latest_branch_response_skips_errored_and_aborted_responses() {
    let mut m = mgr();
    m.append_message(user("hi")).unwrap();
    m.append_message(answered("faux", "large", "openai-completions"))
        .unwrap();
    m.append_message(answered_with(
        "faux",
        "broken",
        "openai-completions",
        StopReason::Error,
    ))
    .unwrap();
    m.append_message(answered_with(
        "faux",
        "stopped",
        "openai-completions",
        StopReason::Aborted,
    ))
    .unwrap();

    let path = m.branch_path(None);
    let latest = latest_branch_response(&path).expect("a settled response");

    assert_eq!(latest.provider.as_str(), "faux");
    assert_eq!(
        latest.model, "large",
        "the newest SETTLED response, not the newest response"
    );
}

/// The sentence this function exists for, from `docs/virtual-models.md:29`: the limits come from
/// *"the physical model that produced the latest response, even if that response came before
/// switching to the virtual model"*. So the walk must NOT stop at a `model_change` — upstream's
/// loop has no `model_change` clause at all (`virtual-models.ts:111-116`), unlike
/// [`crate::virtual_models::branch_selection`]'s, which does.
///
/// RED-PROVE: add a `KnownEntry::ModelChange => return None` arm (i.e. "only look at responses
/// since the current selection") — the walk stops at the switch to `router/auto` and answers
/// `None`, so a freshly switched session would report the VIRTUAL model's declared window instead
/// of the 50k one it is actually talking to.
#[test]
fn latest_branch_response_returns_a_response_that_predates_the_model_change() {
    let mut m = mgr();
    m.append_message(user("hi")).unwrap();
    m.append_message(answered("faux", "large", "openai-completions"))
        .unwrap();
    // The user switches to the router AFTER that answer and has not prompted since.
    m.append_model_change("router".into(), "auto".into())
        .unwrap();

    let path = m.branch_path(None);
    let latest = latest_branch_response(&path).expect("the pre-switch response still counts");

    assert_eq!(latest.model, "large");
}

/// BRANCH-scoped, like every other walk in this module: a response on an abandoned branch is
/// invisible after a `/tree` navigation, because the basis is `branch_path(None)` and not the flat
/// entry store.
///
/// RED-PROVE: scan `entries()` instead of the branch path — the abandoned `faux/small` answer is
/// newer in the store, so it is returned and the assertion fails.
#[test]
fn latest_branch_response_walks_the_branch_and_not_the_flat_store() {
    let mut m = mgr();
    m.append_message(user("shared")).unwrap();
    let fork = m
        .append_message(answered("faux", "large", "openai-completions"))
        .unwrap();
    m.append_message(answered("faux", "small", "openai-completions"))
        .unwrap();

    // /tree back onto the `large` answer: the `small` answer is now off-path but still in the store.
    m.branch(&fork).unwrap();

    let path = m.branch_path(None);
    let latest = latest_branch_response(&path).expect("a settled response");
    assert_eq!(
        latest.model, "large",
        "the off-branch answer must not be seen"
    );

    // And the two bases genuinely DISAGREE on this session, which is what makes the production
    // caller's choice of `branch_path(None)` (see `AgentSession::routed_model`) load-bearing
    // rather than incidental.
    let flat: Vec<&crate::entry::Entry> = m.entries().iter().collect();
    assert_eq!(
        latest_branch_response(&flat).expect("a response").model,
        "small",
        "the flat entry store still holds the abandoned answer, and it is NEWER"
    );
}

/// The deliberate NON-feature, and the reason the api test lives in the CALLER: upstream's walk
/// has no `isVirtualModel` clause, so a virtual-named message that settled cleanly IS returned
/// here. What makes a failed routing invisible is its `stop_reason: error`
/// (covered above), and what makes any virtual-named row unusable as a limits model is
/// `getPhysicalModel`'s own rejection one step later (`model-runtime.ts:1026-1030`).
///
/// RED-PROVE: add a `!is_virtual_api(Some(&a.api))` conjunct to the walk — this test fails, and so
/// does the layering it documents. The OBSERVABLE end state (a routing failure never changes the
/// window) is pinned at the session level by
/// `cyrup-session-svc`'s `the_limits_survive_a_routing_failure`, which is green either way — which
/// is exactly why this test is written at this layer and not that one.
#[test]
fn latest_branch_response_does_not_itself_test_the_api() {
    let mut m = mgr();
    m.append_message(answered("faux", "large", "openai-completions"))
        .unwrap();
    m.append_message(answered("router", "auto", VIRTUAL_MODEL_API))
        .unwrap();

    let path = m.branch_path(None);
    let latest = latest_branch_response(&path).expect("a response");
    assert_eq!(
        latest.api.as_str(),
        VIRTUAL_MODEL_API,
        "the walk is stop-reason-only; the api is the caller's test"
    );
}

/// A branch with nothing settled on it has no latest response, which is the `??` arm of
/// `_limitsModel` — the limits then fall back to the ones DECLARED on the virtual model.
#[test]
fn a_branch_with_no_settled_response_has_no_latest_response() {
    let mut m = mgr();
    m.append_message(user("hi")).unwrap();
    m.append_message(answered_with(
        "router",
        "auto",
        VIRTUAL_MODEL_API,
        StopReason::Error,
    ))
    .unwrap();
    let path = m.branch_path(None);
    assert!(latest_branch_response(&path).is_none());
}
