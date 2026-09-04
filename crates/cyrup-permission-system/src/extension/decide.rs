//! The deciding gate: the `before_tool_call` entry point and the three supplementary layers it
//! runs before the ask tier — the registry / unknown-tool block, the skill-read bypass and the
//! external-directory guard.

use cyrup_core::TerminateHint;
use serde_json::{Value, json};

use cyrup_ext::{HookOutcome, HostCtx};

use crate::ask::{AskOutcome, PermissionDecisionState};
use crate::common::{self, to_record};
use crate::dedup::DedupDetails;
use crate::gate;
use crate::skill;
use crate::types::{PermissionCheckResult, PermissionState};

use super::audit::{decision_state_str, source_str};
use super::{PermissionSystemExtension, guard};

/// Build the [`DedupDetails`] fingerprint inputs (pi `PermissionPromptDetails`, `index.ts:713-726`).
/// `message` is the live prompt (pi `details.message = formatAskPrompt(...)`), so a re-emitted
/// identical `tool_call` fingerprints the same and reuses the cached decision.
pub(super) fn dedup_details(
    call_id: &str,
    input: &Value,
    check: &PermissionCheckResult,
    agent_name: Option<&str>,
) -> DedupDetails {
    DedupDetails {
        request_id: call_id.to_string(),
        source: source_str(check.source).to_string(),
        agent_name: agent_name.map(str::to_string),
        message: gate::format_ask_prompt(check, agent_name, input),
        tool_call_id: Some(call_id.to_string()),
        tool_name: Some(check.tool_name.clone()),
        skill_name: None,
        path: gate::get_path_bearing_tool_path(&check.tool_name, input),
        command: check.command.clone(),
        target: check.target.clone(),
        tool_input: input.clone(),
    }
}

/// The per-`tool_call` identity the layered gate threads into every branch — the borrowed subset of
/// pi's `event` + `ctx` its `writeReviewEntry` records are built from (`toolCallId`, `toolName`,
/// `input`, `ctx.cwd`, `agentName`). Bundled rather than passed loose so the layer resolvers keep a
/// two-argument shape as the audit fields grew.
#[derive(Clone, Copy)]
struct GateCall<'a> {
    /// pi `event.toolCallId` — also the `requestId` of any prompt this call raises.
    call_id: &'a str,
    /// The trimmed tool name (pi `toolName`).
    tool_name: &'a str,
    /// The `cwd`-injected input (pi's `input` after `index.ts:2305-2309`).
    input: &'a Value,
    /// pi `ctx.cwd`.
    cwd: &'a str,
    /// The resolved persona (pi `agentName`), `None` at top level.
    agent_name: Option<&'a str>,
}

impl PermissionSystemExtension {
    /// The gate (pi `index.ts:2208-2499`, the deciding subset): resolve `tool_name` + `input`, fold
    /// the approval stores, then `Block` on deny / fail-closed ask, or proceed on allow. Returns the
    /// `HookOutcome` the dispatcher maps to `BeforeOutcome`.
    pub(super) async fn decide(
        &self,
        call_id: &str,
        tool_name: &str,
        input: &Value,
        ctx: &HostCtx,
    ) -> HookOutcome {
        let normalized = tool_name.trim();
        if normalized.is_empty() {
            return HookOutcome::Block {
                reason: Some(gate::format_missing_tool_name_reason()),
                terminate: TerminateHint::Unspecified,
            };
        }
        let agent_name = self.agent_name.as_deref();

        // (2) REGISTRY / unknown-tool gate (pi `index.ts:2218-2228`): ALWAYS runs, unconditionally,
        // against the full registry BEFORE any permission check — pi has no skip path
        // (`checkRequestedToolRegistration(toolName, pi.getAllTools())` is called every time, and
        // `pi.getAllTools()` never returns `undefined`). When the live backend cannot enumerate the
        // registry (`all_tool_names` returns `None` — no backend attached, or a wiring gap on an
        // attached one) this fails CLOSED against an EMPTY registry rather than skipping the gate:
        // exactly what pi would do if its tool registry were ever empty (nothing matches ⇒ every tool
        // is "unregistered"). An unattached/misconfigured host can no longer silently bypass the
        // unknown-tool allowlist.
        let registered = self.registered_tool_names().unwrap_or_default();
        if let Some(reason) = gate::check_requested_tool_registration(normalized, &registered) {
            return HookOutcome::Block {
                reason: Some(reason),
                terminate: TerminateHint::Unspecified,
            };
        }

        // pi `index.ts:2305-2309`: anchor a path-bearing input's resource resolution to the SESSION
        // cwd (`HostCtx.cwd`) when the input carries a `path`/`file_path` but no `cwd` of its own. Used
        // for the skill-read + external-directory + main checks below (pi threads this same `input`).
        let cwd: String = ctx.cwd.to_string_lossy().into_owned();
        let injected = gate::inject_cwd(input, &cwd);
        let input: &Value = &injected;

        // (3) SKILL-READ bypass (pi `index.ts:2230-2303`): a `read` whose path lands on a tracked skill
        // is governed by the SKILL policy (allow → proceed; ask → prompt; deny → block), bypassing the
        // read-tool policy. `None` = no skill matched → fall through to the external-dir + main checks.
        // The per-call identity every gated layer audits against (pi threads `event.toolCallId` /
        // `toolName` / `input` / `ctx.cwd` / `agentName` into each `writeReviewEntry` by hand).
        let call = GateCall {
            call_id,
            tool_name: normalized,
            input,
            cwd: &cwd,
            agent_name,
        };

        if normalized == "read"
            && let Some(outcome) = self.resolve_skill_read(&call, ctx).await
        {
            return outcome;
        }

        // (4) EXTERNAL-DIRECTORY guard (pi `index.ts:2310-2414`): a path-bearing tool targeting a path
        // OUTSIDE the working directory is gated by the `external_directory` special policy first.
        // `None` = allowed / not applicable → fall through to the main check (which uses the SAME
        // `input`); `Some(_)` = a terminal deny / denied-ask / ask-unavailable block.
        if !cwd.is_empty()
            && let Some(path) = gate::get_path_bearing_tool_path(normalized, input)
            && gate::is_path_outside_working_directory(&path, &cwd)
            && let Some(outcome) = self.resolve_external_directory(&call, &path, ctx).await
        {
            return outcome;
        }

        // Main check + store overlay — fully synchronous; every lock is dropped before any await.
        let check = {
            let session_rules = guard(&self.session_approvals).get_rules();
            let raw = guard(&self.manager).check_permission(normalized, input, agent_name);
            gate::apply_pattern_approval_state(raw, input, &session_rules)
        };

        match check.state {
            PermissionState::Deny => {
                // pi `index.ts:2422-2439`: the policy-denied audit entry, then `flush()` before the
                // block is returned (`[CYRUP-DELTA]` — the write is already durable here).
                let details = dedup_details(call_id, input, &check, agent_name);
                self.review_permission_decision(
                    "permission_request.blocked",
                    &details,
                    json!({
                        "source": "tool_call",
                        "resolution": "policy_denied",
                        "decisionPersistence": "none",
                        "decisionScope": Self::permission_decision_scope(&details),
                    }),
                );
                self.logger.flush();
                HookOutcome::Block {
                    reason: Some(gate::format_deny_reason(&check, agent_name)),
                    terminate: TerminateHint::Unspecified,
                }
            }
            PermissionState::Allow => HookOutcome::Noop,
            PermissionState::Ask => self.resolve_ask(call_id, input, &check, ctx).await,
        }
    }

    /// The full registry tool names (pi `pi.getAllTools()`, the `getAllTools` analog) via the captured
    /// live backend, or `None` when no live backend is attached (default host / headless). See
    /// [`cyrup_ext::HostServices::all_tool_names`] for why this is the FULL registry, not the exposed
    /// subset [`cyrup_ext::HostServices::active_tools`] returns.
    fn registered_tool_names(&self) -> Option<Vec<String>> {
        self.host_services.get().and_then(|s| s.all_tool_names())
    }

    /// (3) The skill-read bypass (pi `index.ts:2230-2303`). Resolves the `read` path against the
    /// active-skill entries (exact/base-dir match) and, failing that, an inferred skills-root entry;
    /// then, unless the skill was explicitly `/skill:`-requested, enforces its policy: `deny` → block,
    /// `ask` → live prompt (fail-closed / user-deny → block), `allow`/approved → proceed. Returns
    /// `Some(HookOutcome)` when a skill matched (a terminal decision, allow via `Noop`), `None` when no
    /// skill matched (the caller falls through to the external-dir + main checks).
    async fn resolve_skill_read(&self, call: &GateCall<'_>, ctx: &HostCtx) -> Option<HookOutcome> {
        let GateCall {
            call_id,
            tool_name,
            input,
            cwd,
            agent_name,
        } = *call;
        let read_path = to_record(input)
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let normalized_read_path = common::normalize_path_for_comparison(&read_path, cwd);

        // A tracked-entry match (pi `findSkillPathMatch`), else an inferred skills-root entry whose
        // state comes from a fresh `checkPermission("skill", {name}, agentName)` (pi `:2236-2241`).
        let matched = {
            let entries = guard(&self.active_skill_entries);
            skill::find_skill_path_match(&normalized_read_path, &entries).cloned()
        };
        let read_skill = match matched {
            Some(m) => m,
            None => {
                let agent_dir = self.agent_dir.to_string_lossy().into_owned();
                // No skill matched (tracked or inferred) → `?` returns `None` so the caller falls
                // through to the external-dir + main checks (pi `:2300` — no `readSkill`).
                let mut inferred = skill::infer_skill_entry_from_read_path(
                    &read_path,
                    cwd,
                    &agent_dir,
                    PermissionState::Ask,
                )?;
                inferred.state = guard(&self.manager)
                    .check_permission(
                        "skill",
                        &json!({ "name": inferred.name.clone() }),
                        agent_name,
                    )
                    .state;
                inferred
            }
        };

        let explicitly_requested =
            guard(&self.explicitly_requested_skill_names).contains(&read_skill.name);
        if !explicitly_requested {
            match read_skill.state {
                PermissionState::Deny => {
                    // pi `index.ts:2243-2255`.
                    self.write_review_entry(
                        "permission_request.blocked",
                        &json!({
                            "source": "skill_read",
                            "toolCallId": call_id,
                            "toolName": tool_name,
                            "skillName": read_skill.name,
                            "agentName": agent_name,
                            "path": read_path,
                            "toolInput": input,
                            "resolution": "policy_denied",
                        }),
                    );
                    return Some(HookOutcome::Block {
                        reason: Some(skill::format_skill_path_deny_reason(
                            &read_skill,
                            agent_name,
                        )),
                        terminate: TerminateHint::Unspecified,
                    });
                }
                PermissionState::Ask => {
                    let message =
                        skill::format_skill_path_ask_prompt(&read_skill, &read_path, agent_name);
                    // pi `index.ts:2282-2291`'s `promptPermission` details record.
                    let details = DedupDetails {
                        request_id: call_id.to_string(),
                        source: "skill_read".to_string(),
                        agent_name: agent_name.map(str::to_string),
                        message: message.clone(),
                        tool_call_id: Some(call_id.to_string()),
                        tool_name: Some(tool_name.to_string()),
                        skill_name: Some(read_skill.name.clone()),
                        path: Some(read_path.clone()),
                        command: None,
                        target: None,
                        tool_input: input.clone(),
                    };
                    match self.prompt_decision(&details, ctx).await {
                        AskOutcome::NoLiveChannel => {
                            // pi `index.ts:2262-2276`.
                            self.write_review_entry(
                                "permission_request.blocked",
                                &json!({
                                    "source": "skill_read",
                                    "toolCallId": call_id,
                                    "toolName": tool_name,
                                    "skillName": read_skill.name,
                                    "agentName": agent_name,
                                    "path": read_path,
                                    "prompt": message,
                                    "promptMetadata": crate::logging::sensitive_log_metadata(Some(&message)),
                                    "toolInput": input,
                                    "resolution": "confirmation_unavailable",
                                }),
                            );
                            return Some(HookOutcome::Block {
                                reason: Some(skill::skill_ask_unavailable_reason()),
                                terminate: TerminateHint::Unspecified,
                            });
                        }
                        AskOutcome::Decided(d) if !d.approved => {
                            return Some(HookOutcome::Block {
                                reason: Some(skill::format_skill_user_denied_reason(
                                    d.denial_reason.as_deref(),
                                )),
                                terminate: TerminateHint::Unspecified,
                            });
                        }
                        AskOutcome::Decided(_) => {}
                    }
                }
                PermissionState::Allow => {}
            }
        }
        // A skill matched → allow the read, bypassing the read-tool policy (pi `:2300-2302`).
        Some(HookOutcome::Noop)
    }

    /// (4) The external-directory guard (pi `index.ts:2312-2413`). Checks the `external_directory`
    /// special policy for `{path, cwd}` (with the session overlay applied on an `ask`): `deny`
    /// → block; `ask` → live prompt (fail-closed / user-deny → block; approved-Always → session-persist,
    /// then fall through); `allow` → fall through. `None` = allowed (proceed to the main check).
    async fn resolve_external_directory(
        &self,
        call: &GateCall<'_>,
        path: &str,
        ctx: &HostCtx,
    ) -> Option<HookOutcome> {
        let GateCall {
            call_id,
            tool_name,
            input,
            cwd,
            agent_name,
        } = *call;
        let ext_input = json!({ "path": path, "cwd": cwd });
        let raw =
            guard(&self.manager).check_permission("external_directory", &ext_input, agent_name);
        // pi `:2319-2321`: the session overlay is applied ONLY on an `ask` result.
        let ext_check = if raw.state == PermissionState::Ask {
            let session_rules = guard(&self.session_approvals).get_rules();
            gate::apply_pattern_approval_state(raw, &ext_input, &session_rules)
        } else {
            raw
        };

        match ext_check.state {
            PermissionState::Deny => {
                // pi `index.ts:2323-2333`.
                self.write_review_entry(
                    "permission_request.blocked",
                    &json!({
                        "source": "tool_call",
                        "toolCallId": call_id,
                        "toolName": tool_name,
                        "agentName": agent_name,
                        "path": path,
                        "toolInput": input,
                        "resolution": "policy_denied",
                    }),
                );
                Some(HookOutcome::Block {
                    reason: Some(gate::format_external_directory_deny_reason(
                        tool_name, path, cwd, agent_name,
                    )),
                    terminate: TerminateHint::Unspecified,
                })
            }
            PermissionState::Ask => {
                let message =
                    gate::format_external_directory_ask_prompt(tool_name, path, cwd, agent_name);
                // pi `index.ts:2368-2377`'s `promptPermission` details record — note `source` is
                // `"tool_call"` here, not `"skill_read"`, and no `skillName`/`command`/`target`.
                let details = DedupDetails {
                    request_id: call_id.to_string(),
                    source: "tool_call".to_string(),
                    agent_name: agent_name.map(str::to_string),
                    message: message.clone(),
                    tool_call_id: Some(call_id.to_string()),
                    tool_name: Some(tool_name.to_string()),
                    skill_name: None,
                    path: Some(path.to_string()),
                    command: None,
                    target: None,
                    tool_input: input.clone(),
                };
                match self.prompt_decision(&details, ctx).await {
                    AskOutcome::NoLiveChannel => {
                        // pi `index.ts:2351-2362`.
                        self.write_review_entry(
                            "permission_request.blocked",
                            &json!({
                                "source": "tool_call",
                                "toolCallId": call_id,
                                "toolName": tool_name,
                                "agentName": agent_name,
                                "path": path,
                                "prompt": message,
                                "promptMetadata": crate::logging::sensitive_log_metadata(Some(&message)),
                                "toolInput": input,
                                "resolution": "confirmation_unavailable",
                            }),
                        );
                        Some(HookOutcome::Block {
                            reason: Some(gate::format_external_directory_unavailable_reason(path)),
                            terminate: TerminateHint::Unspecified,
                        })
                    }
                    AskOutcome::Decided(d) if !d.approved => Some(HookOutcome::Block {
                        reason: Some(gate::format_external_directory_user_denied_reason(
                            tool_name,
                            path,
                            d.denial_reason.as_deref(),
                        )),
                        terminate: TerminateHint::Unspecified,
                    }),
                    AskOutcome::Decided(d) => {
                        // pi `persistPatternApprovalDecision` (`:2391`): an approved-Always persists an
                        // allow rule to the SESSION store, then the call FALLS THROUGH to the main check.
                        if d.state == PermissionDecisionState::Always {
                            let subject =
                                gate::get_pattern_approval_subject(&ext_check, &ext_input);
                            if !subject.is_empty() {
                                guard(&self.session_approvals)
                                    .approve_always(&ext_check.tool_name, &subject);
                                // pi `index.ts:2397-2409`: the persist is audited only when a
                                // subject was actually recorded, and names the SPECIAL tool
                                // `external_directory` rather than the calling tool.
                                self.write_review_entry(
                                    "permission_request.approval_persisted",
                                    &json!({
                                        "source": "tool_call",
                                        "toolCallId": call_id,
                                        "toolName": "external_directory",
                                        "agentName": agent_name,
                                        "path": path,
                                        "toolInput": input,
                                        "resolution": decision_state_str(d.state),
                                        "decisionPersistence": "session",
                                        "approvalPersistence": "session",
                                        "approvalScope": subject,
                                    }),
                                );
                                self.logger.flush();
                            }
                        }
                        None
                    }
                }
            }
            PermissionState::Allow => None,
        }
    }
}
