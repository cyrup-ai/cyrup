//! Virtual-model selection and router state on a session branch (`SESS-067`).
//!
//! A **virtual model** is a catalog entry that routes each request to a physical model. The
//! selection a branch records (`model_change`) may name one; everything below the routing step
//! only ever sees physical models, so assistant messages name the physical model that answered.
//!
//! Two consequences live here, and only these two — the catalog half (`withVirtualModels`,
//! `createVirtualModel`, the routing step) is area 01's and the extension registration seam is
//! area 06's, as `SESS-067` records:
//!
//! 1. A branch's **selection** can no longer be "the latest model wins", because a virtual
//!    selection is followed by responses naming the physical models it routed to. See
//!    [`branch_selection`].
//! 2. A router's **state** is persisted as a `custom` session entry, so it survives resume and is
//!    scoped to a branch. See [`virtual_model_state`].
//!
//! Ported from pi `packages/coding-agent/src/core/virtual-models.ts`, read at **v1.0.4** through
//! git objects. That file is byte-identical at v1.0.0, v1.0.1 and v1.0.4 (238 lines at each), so
//! the upstream line numbers below hold at all three.

use cyrup_core::{ApiId, Message, ModelRef};
use serde_json::Value;

use crate::agent_message::AgentMessage;
use crate::entry::{Entry, KnownEntry};

/// Api id of virtual catalog entries — `VIRTUAL_MODEL_API` (`virtual-models.ts:29`).
///
/// A request for a model carrying this api fails unless it is routed first, which is why no
/// provider ever sees it.
///
/// Re-exported from [`cyrup_core`] rather than defined here: the catalog half of the port lives in
/// `cyrup-provider`, which cannot read a literal out of `cyrup-session` (this crate depends on
/// `cyrup-provider`, not the other way round). One definition beside
/// [`cyrup_core::UNRESOLVED_API`] serves both, and the path `virtual_models::VIRTUAL_MODEL_API`
/// is unchanged for every caller.
pub use cyrup_core::VIRTUAL_MODEL_API;

/// `customType` of the entry that stores router state on a branch —
/// `VIRTUAL_MODEL_STATE_ENTRY` (`virtual-models.ts:32`).
///
/// Its `data` is pi's `VirtualModelStateData { provider, modelId, state }` (`:36-40`), written in
/// pi's camelCase, so a pi-written entry is read without translation.
pub const VIRTUAL_MODEL_STATE_ENTRY: &str = "pi.virtual-model-state";

/// Whether an api id names a virtual model — `isVirtualModel` (`virtual-models.ts:102`).
///
/// Upstream takes the whole model (or message) and compares its `api`; cyrup's callers hold the
/// api directly, on [`ModelRef::api`] or on an assistant message's required `api` field.
#[must_use]
pub fn is_virtual_api(api: Option<&ApiId>) -> bool {
    api.is_some_and(|a| a.as_str() == VIRTUAL_MODEL_API)
}

/// The model selection a session branch records — `getBranchSelection`
/// (`virtual-models.ts:128-145`).
///
/// In upstream's own words: *"A virtual `model_change` holds until the next `model_change`,
/// because responses name the physical models it routed to. Otherwise the latest physical response
/// wins, as in sessions without virtual models. A virtual model that is no longer registered does
/// not hold, so the selection falls back to the physical model that answered last."*
///
/// `get_model` resolves a catalog entry's api so the hold test can ask whether the selection is
/// virtual. `SESS-067`'s Fix asks for a `Fn(&str, &str) -> Option<Model>` lookup "so
/// `cyrup-session` does not gain a catalog dependency"; [`ModelRef`] is that stand-in, since
/// `Model` lives in `cyrup-provider` and the api is the only field the rule reads. Only the last
/// `model_change` can hold, so this looks up **at most one** model.
///
/// With no virtual models registered this is provably the current forward last-wins rule: the
/// `is_virtual_api` test fails and the physical response is returned.
///
/// # `get_model` need only answer for VIRTUAL entries
///
/// A caller may hand this a lookup over the virtual-model registry alone and does not have to
/// thread a whole catalog, because the answer is observationally identical. Three facts at v1.0.4,
/// each read directly:
///
/// 1. The lookup's result is consumed ONLY through `isVirtualModel(model)`
///    (`virtual-models.ts:141`), so "absent" and "present but physical" take the same arm — the
///    `response` one.
/// 2. A virtual entry HIDES a same-id physical chat model, because `withVirtualModels`' physical
///    filter drops `isModelType(model, "chat") && ids.has(model.id)` (`:218`). So a full catalog
///    lookup can never answer "physical" for an id a virtual model holds.
/// 3. `registerVirtualModel` REFUSES an id that already names a physical model of that provider —
///    `if (existing && !isVirtualModel(existing)) throw` (`model-runtime.ts:957-961`). So the
///    converse collision cannot be registered either.
///
/// Taken together, for every `(provider, id)` a full catalog and a virtual-only registry give the
/// same `isVirtualModel` verdict. The signature stays `Fn(&str, &str) -> Option<ModelRef>` so a
/// caller that already holds a catalog may still pass one; such a lookup is harmless, not wrong.
#[must_use]
pub fn branch_selection(
    path: &[&Entry],
    get_model: impl Fn(&str, &str) -> Option<ModelRef>,
) -> Option<ModelRef> {
    for (i, e) in path.iter().enumerate().rev() {
        let Entry::Known(k) = e else { continue };
        match k {
            KnownEntry::ModelChange {
                provider, model_id, ..
            } => return Some(selection(provider.as_str(), model_id.as_str())),
            // A failed routing attempt leaves the VIRTUAL model on its assistant message
            // (`virtual-models.ts:101`), so such a message is skipped rather than treated as a
            // physical response.
            KnownEntry::Message {
                message: AgentMessage::Core(Message::Assistant(a)),
                ..
            } if !is_virtual_api(Some(&a.api)) => {
                let response = a.model_ref();
                let change = find_last_model_change(path, i);
                return match change {
                    Some(c)
                        if get_model(c.provider.as_str(), c.model.as_str())
                            .is_some_and(|m| is_virtual_api(m.api.as_ref())) =>
                    {
                        Some(c)
                    }
                    _ => Some(response),
                };
            }
            _ => {}
        }
    }
    None
}

/// The last `model_change` strictly before `before` — `findLastModelChange`
/// (`virtual-models.ts:147-157`).
fn find_last_model_change(path: &[&Entry], before: usize) -> Option<ModelRef> {
    for e in path.get(..before)?.iter().rev() {
        if let Entry::Known(KnownEntry::ModelChange {
            provider, model_id, ..
        }) = e
        {
            return Some(selection(provider.as_str(), model_id.as_str()));
        }
    }
    None
}

/// A `model_change` names a provider and a model id and no api, which is what
/// `SessionManager::build_context` already records for one.
fn selection(provider: &str, model_id: &str) -> ModelRef {
    ModelRef {
        provider: provider.into(),
        api: None,
        model: model_id.into(),
    }
}

/// The latest router state a branch stores for one virtual model — `getVirtualModelState`
/// (`virtual-models.ts:159-167`).
///
/// Walks the branch backwards and returns the `state` of the newest `pi.virtual-model-state` entry
/// whose `data` matches both `provider` and `modelId`. An entry with no `data`, or with a
/// different provider or model, is skipped rather than ending the walk — upstream's `data?.` and
/// `continue` do the same. An entry on an abandoned branch is invisible because `path` is the
/// active branch.
#[must_use]
pub fn virtual_model_state<'a>(
    path: &[&'a Entry],
    provider: &str,
    model_id: &str,
) -> Option<&'a Value> {
    for e in path.iter().rev() {
        let Entry::Known(KnownEntry::Custom {
            custom_type, data, ..
        }) = e
        else {
            continue;
        };
        if custom_type.as_str() != VIRTUAL_MODEL_STATE_ENTRY {
            continue;
        }
        let Some(d) = data.as_ref() else { continue };
        if d.get("provider").and_then(Value::as_str) == Some(provider)
            && d.get("modelId").and_then(Value::as_str) == Some(model_id)
        {
            return d.get("state");
        }
    }
    None
}

/// The latest SUCCESSFUL response on a branch — `findLatestResponse`
/// (`virtual-models.ts:109-118`), in upstream's own words: *"Latest successful response. Its model
/// is physical: failed or aborted requests, including failed routing, are skipped."*
///
/// Walks backwards and returns the newest assistant message whose `stop_reason` is neither
/// [`StopReason::Error`](cyrup_core::StopReason::Error) nor
/// [`StopReason::Aborted`](cyrup_core::StopReason::Aborted). This is the anchor of the limits rule
/// `docs/virtual-models.md:29` states: *"context usage uses the limits of the physical model that
/// produced the latest response, even if that response came before switching to the virtual
/// model"*. Nothing here stops at a `model_change`, which is what makes a response that PRE-DATES
/// the switch to a virtual model still count.
///
/// # Three things this deliberately does NOT do
///
/// 1. It does **not** test the api. Upstream's walk has no `isVirtualModel` clause either: a
///    failed routing attempt is skipped because it carries `stop_reason: error`, not because it
///    names the virtual model. The api test lives one step later, in the caller's
///    `getPhysicalModel` lookup (`model-runtime.ts:1026-1030`), which answers `None` for a virtual
///    row — so a virtual-named message that somehow settled cleanly yields "no routed model"
///    rather than the virtual model's own limits.
/// 2. It does **not** skip [`StopReason::Deferred`](cyrup_core::StopReason::Deferred), which has
///    no upstream counterpart. Upstream skips exactly two stop reasons and so does this. A
///    deferred turn names a real physical model, which is all the limits rule asks of it — unlike
///    [`crate::SessionManager`]-side USAGE readers, which skip it because its usage is empty.
/// 3. It does **not** take the branch's own messages apart. Upstream reads
///    `agent.state.messages`, the live transcript; cyrup answers context questions from
///    `branch_path(None)` because the manager is its authority for the active path (the same basis
///    `AgentSession::context_usage` already uses). `[CYRUP-DELTA]` in basis, not in rule: the two
///    agree because cyrup's three transcript re-seed sites assign exactly that list.
#[must_use]
pub fn latest_branch_response<'a>(path: &[&'a Entry]) -> Option<&'a cyrup_core::AssistantMessage> {
    use cyrup_core::StopReason;

    path.iter().copied().rev().find_map(|e| match e {
        Entry::Known(KnownEntry::Message {
            message: AgentMessage::Core(Message::Assistant(a)),
            ..
        }) if !matches!(a.stop_reason, StopReason::Error | StopReason::Aborted) => Some(a),
        _ => None,
    })
}
