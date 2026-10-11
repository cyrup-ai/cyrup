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
use super::consts::SHIPPED_REFERENCE_PAGES;
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
        parent_tool_call_id: None,
    }
}

/// How many model-issued calls [`PermissionSystemExtension::recent_calls`] remembers. A model
/// issues a handful of calls per turn and a nested call's parent is one of the newest, so this
/// bounds the list without ever losing a parent that is still running.
const RECENT_CALLS_CAP: usize = 64;

/// Where a gated call came from when it was made by another tool while it ran
/// (`ctx.executeTool`, e.g. a `codemode` script's `tools.bash(...)`): the calling tool call's id and
/// the human-readable label for prompts and block reasons. See [`gate::format_nested_origin_label`].
pub(super) struct NestedOrigin {
    parent_tool_call_id: String,
    label: String,
}

/// [CYRUP-DELTA] Whether `tool_name` is a `read` of a file this process wrote for the model to read
/// back: the full output of a truncated `bash` command or `codemode` script, or an image a script
/// saved (`cyrup_core::spilled_files`). The result that names the file tells the model to `read`
/// it, and the file is in the temp directory, outside the project, so with the external-directory
/// guard armed the model was told to read a file the same policy then asked about, or refused
/// outright. pi has no permission system, so it never had both. Only `read` qualifies, the `read`
/// tool's own policy still applies, and the path must be one that was recorded, compared after the
/// same lexical normalization the guard uses: another file in the same directory is not one.
///
/// A recorded path that has since become a symbolic link is not exempt. The file was created
/// exclusively and readable only by its owner, but its directory is the shared temp directory, and
/// a read of a link would follow it somewhere the record never vouched for.
fn is_spilled_output_read(tool_name: &str, path: &str, cwd: &str) -> bool {
    if tool_name != "read" {
        return false;
    }
    let requested = common::normalize_path_for_comparison(path, cwd);
    if requested.is_empty() {
        return false;
    }
    let recorded = cyrup_core::spilled_files::any_recorded(|spill| {
        common::normalize_path_for_comparison(&spill.to_string_lossy(), cwd) == requested
    });
    recorded
        && match std::fs::symlink_metadata(&requested) {
            Ok(meta) => !meta.file_type().is_symlink(),
            // Gone: the read reports that itself, and there is nothing to follow.
            Err(err) => err.kind() == std::io::ErrorKind::NotFound,
        }
}

impl NestedOrigin {
    /// Mark `details` as the details of a nested call (prompt label + audit parent id).
    pub(super) fn mark(&self, details: &mut DedupDetails) {
        details.mark_nested(&self.parent_tool_call_id, &self.label);
    }

    /// A headless block reason for a nested call: `base` plus the origin and what a script can do.
    pub(super) fn unavailable_reason(&self, base: &str) -> String {
        gate::format_nested_ask_unavailable_reason(base, &self.label)
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
    /// `Some` when another tool made this call while it ran.
    origin: Option<&'a NestedOrigin>,
}

impl PermissionSystemExtension {
    /// Remember that the model issued `call_id` for `tool_name` (see [`Self::recent_calls`]).
    fn remember_call(&self, call_id: &str, tool_name: &str) {
        let mut calls = guard(&self.recent_calls);
        if calls.len() >= RECENT_CALLS_CAP {
            calls.pop_front();
        }
        calls.push_back((call_id.to_string(), tool_name.to_string()));
    }

    /// The tool behind a call id. A nested call's id is `<parent id>/<n>`, so a call a nested call
    /// made is found by dropping trailing segments until a model-issued call matches.
    fn tool_behind_call(&self, call_id: &str) -> Option<String> {
        let calls = guard(&self.recent_calls);
        let mut candidate = call_id;
        loop {
            if let Some((_, tool)) = calls.iter().rev().find(|(id, _)| id == candidate) {
                return Some(tool.clone());
            }
            candidate = candidate.rsplit_once('/')?.0;
        }
    }

    /// `Some` when `ctx` is the context of a call another tool made (`parentToolCallId`).
    fn nested_origin(&self, ctx: &HostCtx) -> Option<NestedOrigin> {
        let parent = ctx.parent_tool_call_id()?;
        let parent_tool = self.tool_behind_call(parent.as_str());
        Some(NestedOrigin {
            parent_tool_call_id: parent.as_str().to_string(),
            label: gate::format_nested_origin_label(parent_tool.as_deref()),
        })
    }

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
        // A call another tool made is gated exactly like a model-issued one (pi: the same
        // `tool_call` event, with `parentToolCallId`); the origin only changes how it is described
        // to a human and to the calling script. Only model-issued calls are recorded as parents.
        let origin = self.nested_origin(ctx);
        if origin.is_none() {
            self.remember_call(call_id, normalized);
        }

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
            origin: origin.as_ref(),
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
            && !self.is_shipped_reference_read(normalized, &path, &cwd)
            && !is_spilled_output_read(normalized, &path, &cwd)
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
                let mut details = dedup_details(call_id, input, &check, agent_name);
                if let Some(origin) = &origin {
                    origin.mark(&mut details);
                }
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
            PermissionState::Ask => {
                self.resolve_ask(call_id, input, &check, origin.as_ref(), ctx)
                    .await
            }
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
            origin,
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
                    let mut details = DedupDetails {
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
                        parent_tool_call_id: None,
                    };
                    if let Some(origin) = origin {
                        origin.mark(&mut details);
                    }
                    let message = details.message.clone();
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
                            let reason = skill::skill_ask_unavailable_reason();
                            return Some(HookOutcome::Block {
                                reason: Some(match origin {
                                    Some(origin) => origin.unavailable_reason(&reason),
                                    None => reason,
                                }),
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

    /// [CYRUP-DELTA] Whether `tool_name` is a `read` of one of the pages cyrup writes under the
    /// agent directory for the model (`SHIPPED_REFERENCE_PAGES`). Compared as paths after the
    /// same lexical normalization the guard uses, so `..` segments, `~` and a relative `path` cannot
    /// name a different file, and only `read` qualifies: the page is for reading, and a write, an
    /// edit or a command that mentions it still meets the guard.
    fn is_shipped_reference_read(&self, tool_name: &str, path: &str, cwd: &str) -> bool {
        if tool_name != "read" {
            return false;
        }
        let requested = common::normalize_path_for_comparison(path, cwd);
        SHIPPED_REFERENCE_PAGES.iter().any(|page| {
            let page = self.agent_dir.join(page);
            common::normalize_path_for_comparison(&page.to_string_lossy(), cwd) == requested
        })
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
            origin,
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
                let mut details = DedupDetails {
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
                    parent_tool_call_id: None,
                };
                if let Some(origin) = origin {
                    origin.mark(&mut details);
                }
                let message = details.message.clone();
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
                        let reason = gate::format_external_directory_unavailable_reason(path);
                        Some(HookOutcome::Block {
                            reason: Some(match origin {
                                Some(origin) => origin.unavailable_reason(&reason),
                                None => reason,
                            }),
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
