//! The ask tier: the dedup lookup, the human prompt itself (live dialog, forwarded child prompt,
//! yolo auto-approve or the fail-closed refusal) and the persistence of its answer.

use cyrup_core::TerminateHint;
use std::sync::Arc;

use serde_json::{Value, json};

use cyrup_ext::{HookOutcome, HostCtx};

use crate::ask::{
    AskChannel, AskOutcome, LocalAskChannel, PermissionDecisionState, PermissionPromptDecision,
    PromptOpts,
};
use crate::dedup::DedupDetails;
use crate::gate;
use crate::types::PermissionCheckResult;

use super::audit::decision_state_str;
use super::decide::dedup_details;
use super::env::is_subagent_child;
use super::{PermissionSystemExtension, guard};

impl PermissionSystemExtension {
    /// Settle an in-flight dedup registration with the decision that resolved it (pi's
    /// `decisionPromise` fulfilling, observed by `rememberPermissionPromptDecision`'s stored promise
    /// at v0.8.0 `index.ts:1633`). A `None` owner is pi's uncacheable case (empty `requestId`,
    /// `createPermissionPromptCacheKey` `index.ts:472-481`) — nothing was registered and nothing is
    /// stored.
    fn resolve_prompt_decision(
        &self,
        owner: Option<crate::dedup::PendingOwner>,
        decision: &PermissionPromptDecision,
    ) {
        if let Some(owner) = owner {
            owner.resolve(&mut guard(&self.dedup), decision.clone());
        }
    }

    /// pi `forgetPermissionPromptDecision` in `promptPermission`'s catch
    /// (v0.8.0 `index.ts:1638-1642`): the prompt never produced a decision, so the in-flight
    /// registration must be dropped rather than left latched — otherwise every later identical
    /// request would await a promise that will never settle.
    fn forget_prompt_decision(&self, owner: Option<crate::dedup::PendingOwner>) {
        if let Some(owner) = owner {
            owner.forget(&mut guard(&self.dedup));
        }
    }

    /// pi `promptPermission` (`index.ts:1794-1902`), the shared prompting core EVERY ask surface goes
    /// through: the dedup cache, then the `canResolveAskPermissionRequest` fail-fast pre-check
    /// (`yolo-mode.ts:21-23`, consulted via `canRequestPermissionConfirmation` BEFORE any prompt/lock
    /// work at `index.ts:2263,2351,2452`) — `hasUI || isSubagent || yoloMode` — then yolo auto-approve
    /// (pi `shouldAutoApprovePermissionState`), the C3 human-interaction lock, the live-vs-fallback
    /// channel selection, and the P-3 dispatch-budget-forgiveness guard held across the BLOCKING
    /// dialog. `AskOutcome::NoLiveChannel` = fail-CLOSED (no reachable human), returned IMMEDIATELY by
    /// the pre-check when none of the three conditions hold — zero lock/dialog work touched, exactly
    /// like pi's early return.
    ///
    /// The DEDUP cache lives here, not in any one caller, because pi puts it inside `promptPermission`
    /// itself (`index.ts:1798-1815` lookup, `:1890-1892` store): all three ask surfaces — skill-read
    /// (`index.ts:2282`), external-directory (`:2369`) and the main check (`:2469`) — are therefore
    /// deduplicated identically, so a re-emitted IDENTICAL `tool_call` renders ZERO additional prompts
    /// on ANY of them (`tests/edit-decision-deduplication-red.test.ts` is upstream's regression proof).
    ///
    /// Also emits pi `promptPermission`'s five audit entries (`index.ts:1805,1820,1843,1855-1857`):
    /// `permission_request.duplicate_reused` (cache hit), `.auto_approved` (yolo), `.waiting` (before
    /// the dialog opens) and `.approved`/`.denied` (after the human answers). `details` is pi's
    /// `PermissionPromptDetails` — `details.message` IS the prompt text, so this takes the record
    /// rather than a bare string, and is also what the cache key is fingerprinted from.
    pub(super) async fn prompt_decision(
        &self,
        details: &DedupDetails,
        ctx: &HostCtx,
    ) -> AskOutcome {
        let message = details.message.as_str();
        let yolo_mode = guard(&self.config).yolo_mode;
        if !(ctx.has_ui || is_subagent_child() || yolo_mode) {
            // The caller's `confirmation_unavailable` entry covers this branch (pi audits it at
            // each of its three `canRequestPermissionConfirmation` sites, not inside
            // `promptPermission`). Ordered BEFORE the cache lookup to match pi, whose callers run
            // `canRequestPermissionConfirmation` before ever entering `promptPermission`.
            return AskOutcome::NoLiveChannel;
        }

        // Dedup hit: reuse the prior decision (collapsed to Allow-Once by `create_duplicate_decision`,
        // so a re-emitted approval never re-persists an `Always` grant) — zero additional prompts.
        //
        // PERM-014 — this is `lookup`, not `get`, and the difference is the whole item. pi's cache
        // stores the still-unsettled `decisionPromise` (`index.ts:1633`, run BEFORE the `await` at
        // `:1637`), so a CONCURRENT identical ask hits `getCachedPermissionPromptDecision`
        // (`:1581-1583`) and `await`s that same promise (`:1585`) instead of opening a second
        // dialog. `get` treated an in-flight entry as a miss, so two concurrently-executing tool
        // calls with the same dedup key each raised their own prompt and the operator answered the
        // same question twice — with nothing making the two answers agree.
        let key = details.cache_key();
        if let Some(k) = &key {
            let cached = guard(&self.dedup).lookup(k);
            let cached = match cached {
                // pi `:1585` `createDuplicatePermissionPromptDecision(await cachedDecision)` — the
                // already-settled arm.
                Some(crate::dedup::Lookup::Ready(decision)) => Some(decision),
                // The same statement's OTHER arm: `cachedDecision` is a pending promise, so the
                // `await` blocks here until the owner settles it. The lock is released first —
                // `lookup` returned an owned `Pending` precisely so nothing is held across it.
                Some(crate::dedup::Lookup::Pending(pending)) => Some(pending.wait().await),
                None => None,
            };
            if let Some(decision) = cached {
                // pi `index.ts:1804-1812`: a reused decision is STILL audited — otherwise a
                // re-emitted tool call looks like it was never gated at all.
                self.review_permission_decision(
                    "permission_request.duplicate_reused",
                    details,
                    json!({
                        "resolution": decision_state_str(decision.state),
                        "denialReason": decision.denial_reason,
                        "denialReasonMetadata":
                            crate::logging::sensitive_log_metadata(decision.denial_reason.as_deref()),
                        "decisionPersistence": "none",
                        "approvalPersistence": "none",
                        "decisionScope": Self::permission_decision_scope(details),
                    }),
                );
                self.logger.flush();
                return AskOutcome::Decided(decision);
            }
        }

        // pi `rememberPermissionPromptDecision(..., decisionPromise)` (v0.8.0 `index.ts:1632-1634`)
        // — registered BEFORE the body below runs, which is why the yolo arm and the dialog arm are
        // BOTH inside the window a concurrent duplicate can join. Settled by
        // `resolve_prompt_decision` on every path that produces a decision, and dropped by
        // `forget_prompt_decision` on the one that does not (pi's catch, `:1638-1642`).
        let owner = key.as_ref().map(|k| guard(&self.dedup).begin_pending(k));

        if yolo_mode {
            // pi `index.ts:1598-1608`.
            self.review_permission_decision(
                "permission_request.auto_approved",
                details,
                json!({
                    "resolution": "auto_response",
                    "decisionPersistence": "none",
                    "decisionScope": "yolo_mode",
                }),
            );
            // PERM-011 half B / pi `emitPermissionStateEvent(details, "approved")`
            // (`index.ts:1606`) — between the review entry and the flush, upstream's position.
            self.emit_permission_state_event(details, "approved");
            self.logger.flush();
            let decision = PermissionPromptDecision {
                approved: true,
                state: PermissionDecisionState::Approved,
                denial_reason: None,
            };
            // pi caches the yolo auto-approval too: `rememberPermissionPromptDecision`
            // (`index.ts:1633`) is handed the SAME `decisionPromise` whose body took the
            // `shouldAutoApprovePermissionState` early return at `:1599-1609`.
            self.resolve_prompt_decision(owner, &decision);
            return AskOutcome::Decided(decision);
        }
        // pi `index.ts:1843` — recorded BEFORE the dialog opens, so a session killed mid-prompt
        // still leaves evidence of what was asked.
        self.review_permission_decision("permission_request.waiting", details, json!({}));
        // PERM-011 half B / pi `emitPermissionStateEvent(details, "waiting")` (`index.ts:1612`),
        // immediately after the review entry and BEFORE the dialog opens — an external observer
        // learns a human is being asked while they are still being asked, which is the whole point
        // of the `waiting` state.
        self.emit_permission_state_event(details, "waiting");
        let human_lock = self.host_services.get().and_then(|s| s.human_interaction_lock());
        let _human_guard = match human_lock {
            Some(lock) => Some(lock.acquire().await),
            None => None,
        };
        let channel: Arc<dyn AskChannel> = match (ctx.has_ui, self.host_services.get()) {
            (true, Some(services)) => Arc::new(LocalAskChannel::new(services.clone())),
            _ => self.ask_channel.clone(),
        };
        let outcome = {
            let _human_wait = ctx.begin_human_wait();
            channel.confirm("Permission Required", message, PromptOpts::default()).await
        };

        // pi `index.ts:1855-1868`: the resolved decision, with the "Allow Always" session-persist
        // intent recorded alongside it.
        if let AskOutcome::Decided(ref d) = outcome {
            let always = d.state == PermissionDecisionState::Always;
            let scope = Self::permission_decision_scope(details);
            self.review_permission_decision(
                if d.approved { "permission_request.approved" } else { "permission_request.denied" },
                details,
                json!({
                    "resolution": decision_state_str(d.state),
                    "denialReason": d.denial_reason,
                    "denialReasonMetadata":
                        crate::logging::sensitive_log_metadata(d.denial_reason.as_deref()),
                    "decisionPersistence": if always { "session" } else { "none" },
                    "approvalPersistence": if d.approved && always { "session" } else { "none" },
                    "decisionScope": scope,
                    "approvalScope": if d.approved && always { scope.clone() } else { Value::Null },
                }),
            );
            // PERM-011 half B / pi `emitPermissionStateEvent(details, decision.approved ?
            // "approved" : "denied")` (`index.ts:1626`).
            self.emit_permission_state_event(details, if d.approved { "approved" } else { "denied" });
            // pi `:1637` — the `decisionPromise` settles, so the entry registered above flips from
            // pending to resolved and BOTH the next identical request and any concurrent follower
            // already awaiting it see this decision.
            self.resolve_prompt_decision(owner, d);
        } else {
            // No decision was produced (no reachable human). pi's `confirmPermission` cannot reach
            // this shape — it always resolves to `{approved:false}` — but cyrup's channel can
            // return `NoLiveChannel`, which the CALLER turns into a fail-closed block. The
            // registration must not be left latched: pi's catch arm is
            // `forgetPermissionPromptDecision` (`:1638-1642`), and a follower blocked on
            // `Pending::wait` fails CLOSED when the owner's sender drops here.
            self.forget_prompt_decision(owner);
        }
        outcome
    }

    /// The main-check `ask` branch (pi `:2444-2496` + `confirmPermission :1506-1513`): the shared
    /// [`Self::prompt_decision`] core (pi `promptPermission :1794-1902` — dedup lookup → yolo → C3
    /// human-interaction lock → live dialog under a P-3 budget-forgiveness guard → dedup store) →
    /// fail-CLOSED when no human is reachable → apply (the `Always` session-persist tail). The prompt
    /// subject names the resolved persona (real `agent_name`, pi `formatAskPrompt(check, agentName,
    /// input)`). Dedup is NOT done here: pi keeps it inside `promptPermission` so every ask surface
    /// shares it, and cyrup follows (see [`Self::prompt_decision`]).
    pub(super) async fn resolve_ask(
        &self,
        call_id: &str,
        input: &Value,
        check: &PermissionCheckResult,
        ctx: &HostCtx,
    ) -> HookOutcome {
        let agent_name = self.agent_name.as_deref();

        let details = dedup_details(call_id, input, check, agent_name);

        // pi `formatAskPrompt` (`index.ts:570-590`) — the human-facing prompt (NOT the headless reason).
        // The shared prompting core applies the dedup cache, yolo auto-approve (pi
        // `shouldAutoApprovePermissionState`), the C3 human lock, the live-vs-fallback channel, and the
        // P-3 dispatch-budget guard. `details.message` already IS `format_ask_prompt(check, agent_name,
        // input)` (built by `dedup_details` above), which is what `prompt_decision` prompts with.
        let decision = match self.prompt_decision(&details, ctx).await {
            AskOutcome::Decided(d) => d,
            // Fail-CLOSED: no reachable human (headless / no live UI) → Block, never proceed
            // (pi `confirmPermission` headless `{approved:false}` :1509-1513 / `:2452-2467`).
            AskOutcome::NoLiveChannel => {
                // pi `index.ts:2452-2464`.
                self.review_permission_decision(
                    "permission_request.blocked",
                    &details,
                    json!({ "source": "tool_call", "resolution": "confirmation_unavailable" }),
                );
                return HookOutcome::Block { reason: Some(gate::format_ask_unavailable_reason(check)), terminate: TerminateHint::Unspecified };
            }
        };

        // pi `index.ts:2481-2494`: audit the SESSION persist an approved-Always produces (only when
        // a real subject was recorded), then `flush()`.
        if decision.approved && decision.state == PermissionDecisionState::Always {
            let subject = gate::get_pattern_approval_subject(check, input);
            if !subject.is_empty() {
                self.review_permission_decision(
                    "permission_request.approval_persisted",
                    &details,
                    json!({
                        "source": "tool_call",
                        "resolution": decision_state_str(decision.state),
                        "decisionPersistence": "session",
                        "approvalPersistence": "session",
                        "approvalScope": subject,
                    }),
                );
            }
        }
        self.logger.flush();
        self.apply_decision(decision, check, input)
    }

    /// Apply a resolved decision (pi `:2478-2495`): not-approved → Block (`formatUserDeniedReason`);
    /// approved-Always → persist an allow rule to the SESSION store — the ONLY approval sink there is
    /// (pi v0.8.0 `index.ts:610`, `persistSessionApprovalDecision`; the cross-session
    /// `PermanentApprovalStore` was deleted upstream in v0.8.0, see [`crate::stores`]). The `Always`
    /// persist branch fires on a real dialog returning "Allow Always" ([`LocalAskChannel`]); a later
    /// same-subject call then auto-allows via the store overlay with no second dialog (proven by
    /// `tests/human_dialog.rs`). `Once`/`Approved` (yolo) approve without persisting.
    fn apply_decision(
        &self,
        decision: PermissionPromptDecision,
        check: &PermissionCheckResult,
        input: &Value,
    ) -> HookOutcome {
        if !decision.approved {
            return HookOutcome::Block {
                reason: Some(gate::format_user_denied_reason(check, decision.denial_reason.as_deref())),
                terminate: TerminateHint::Unspecified,
            };
        }
        if decision.state == PermissionDecisionState::Always {
            let subject = gate::get_pattern_approval_subject(check, input);
            if !subject.is_empty() {
                guard(&self.session_approvals).approve_always(&check.tool_name, &subject);
            }
        }
        HookOutcome::Noop
    }
}
