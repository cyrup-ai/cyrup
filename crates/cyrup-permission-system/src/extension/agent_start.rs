//! The `before_agent_start` context-hygiene layer: shape the active tool set, return the
//! sanitized system prompt as a `[mutate]`, and surface the yolo status pill.

use serde_json::json;

use cyrup_ext::{EventPatch, HookOutcome, HostCtx};

use crate::agent_start_cache::{self, CachedPromptState, PromptStateKeyInput};
use crate::gate;
use crate::sanitize;
use crate::types::{CheckSource, PermissionCheckResult, PermissionState};

use super::{PermissionSystemExtension, guard};

impl PermissionSystemExtension {
    /// The `before_agent_start` context-hygiene shaping (pi `index.ts:2134-2190`, port doc §9). Runs
    /// three faithful steps and RETURNS the sanitized system prompt as a `[mutate]` (the system-prompt
    /// MUTATE seam, `contract.rs` `EventPatch::SystemPromptAndInject` → `session.rs`
    /// `assemble_run_messages`):
    /// 1. **Active-tools exposure** (pi `setActiveTools`, `:2155`): for every registered tool
    ///    ([`cyrup_ext::HostServices::all_tool_names`], pi `getAllTools`), keep it iff [`Self::should_expose_tool`];
    ///    restrict the live agent's tool set via [`cyrup_ext::HostServices::set_active_tools`] (staged as
    ///    `pending_active_tools`, drained + applied IN-TURN by `AgentSession::assemble_run_messages`, so
    ///    it shapes turn 1 ordered BEFORE the sanitized prompt). Skipped when no live backend can
    ///    enumerate the registry (pi always has `getAllTools`; the default host does not).
    /// 2. **`sanitizeAvailableToolsSection`** ([`crate::sanitize::tools`], `:2174`) over the exposed set.
    /// 3. **`resolveSkillPromptEntries`** ([`crate::sanitize::skills`], `:2175`) — hides `ask`/`deny`
    ///    skills from `<available_skills>` while CACHING the enforcement entries the skill-read gate
    ///    reads at every `tool_call`. ONE parse, both consumers.
    ///
    /// Also syncs the `"yolo"` status pill (pi `syncPermissionSystemStatus`, `:2136`).
    /// **PERM-013 — both cache layers are ported** (pi `before-agent-start-cache.ts`, consumed at
    /// `index.ts:1894-1898` and `:1900-1913`): step 1's `set_active_tools` fires only when the
    /// exposed tool list actually changed, and steps 2+3 are skipped wholesale on a prompt-state
    /// key hit, replaying the cached entries + prompt. Both keys are invalidated together by
    /// [`Self::invalidate_agent_start_cache`].
    ///
    /// The status pill is NOT synced here: pi's `before_agent_start` reaches it through
    /// `refreshExtensionConfig(ctx)` (`index.ts:1877` → `applyExtensionConfigSideEffects`
    /// `:1364-1366`), which cyrup's `BeforeAgentStart` arm now calls before this function
    /// (PERM-024 / PERM-026). Syncing again here would be a second write of the same value.
    pub(super) fn on_before_agent_start(&self, system_prompt: &str, ctx: &HostCtx) -> HookOutcome {
        let cwd = ctx.cwd.to_string_lossy().into_owned();
        let agent = self.agent_name.as_deref();
        let services = self.host_services.get();

        // (1) Active-tools exposure — only when the live backend can enumerate the FULL registry.
        //
        // \[CYRUP-DELTA] pi has no "registry unavailable" case (`pi.getAllTools()` always returns a
        // list), so the `None` arm below is cyrup-only: the exposed set cannot be computed, the
        // tools section is left intact, and the agent-start cache is BYPASSED entirely rather than
        // keyed on an empty tool list — which would be indistinguishable from a registry that
        // legitimately exposes nothing, and would replay the wrong prompt if the backend attached
        // between turns.
        let allowed: Option<Vec<String>> = services.and_then(|s| s.all_tool_names()).map(|tools| {
            tools.into_iter().filter(|name| self.should_expose_tool(name, agent)).collect()
        });

        let Some(allowed) = allowed else {
            return self.shape_agent_start_prompt(system_prompt, system_prompt, agent, &cwd, None);
        };

        // pi `:1894-1898`: `setActiveTools` runs ONLY when the tool-list key changed.
        let active_tools_key = agent_start_cache::create_active_tools_cache_key(&allowed);
        {
            let mut cache = guard(&self.agent_start_cache);
            if agent_start_cache::should_apply_cached_agent_start_state(
                cache.last_active_tools_key.as_deref(),
                &active_tools_key,
            ) {
                if let Some(s) = services {
                    s.set_active_tools(&allowed);
                }
                cache.last_active_tools_key = Some(active_tools_key);
            }
        }

        // pi `:1900-1907`: the prompt-state key. `permissionStamp` is what makes a mid-session
        // policy edit invalidate this — see [`PermissionManager::policy_cache_stamp`].
        let permission_stamp = guard(&self.manager).policy_cache_stamp(agent);
        let prompt_state_key = agent_start_cache::create_prompt_state_key(&PromptStateKeyInput {
            agent_name: agent,
            cwd: &cwd,
            permission_stamp: &permission_stamp,
            system_prompt,
            allowed_tool_names: &allowed,
        });

        // pi `:1908-1913`: on a key HIT with a recorded result, restore the skill entries and
        // return the cached prompt without re-running either sanitizer.
        let cached_hit = {
            let cache = guard(&self.agent_start_cache);
            if agent_start_cache::should_apply_cached_agent_start_state(
                cache.last_prompt_state_key.as_deref(),
                &prompt_state_key,
            ) {
                None
            } else {
                cache.last_prompt_state_result.clone()
            }
        };
        if let Some(cached) = cached_hit {
            *guard(&self.active_skill_entries) = cached.entries;
            return match cached.system_prompt {
                None => HookOutcome::Noop,
                Some(prompt) => HookOutcome::Mutate(EventPatch::SystemPromptAndInject {
                    system: Some(prompt),
                    inject: None,
                }),
            };
        }

        // (2) Strip the "Available tools:" section + denied-tool guideline bullets (pi `:1915`).
        let working_prompt =
            sanitize::tools::sanitize_available_tools_section(system_prompt, &allowed).prompt;
        self.shape_agent_start_prompt(
            system_prompt,
            &working_prompt,
            agent,
            &cwd,
            Some(prompt_state_key),
        )
    }

    /// The tail of [`Self::on_before_agent_start`] (pi `index.ts:1916-1930`): resolve the skill
    /// prompt entries over `working_prompt`, install them as the enforcement cache, record the
    /// `CachedPromptStateResult` under `prompt_state_key` when one was computed, and return the
    /// sanitized prompt as a `[mutate]` only when it differs from the ORIGINAL `system_prompt`
    /// (pi `skillPromptResult.prompt !== event.systemPrompt`, `:1922`).
    ///
    /// `prompt_state_key: None` is the cyrup-only registry-unavailable path: shape, but record no
    /// cache entry.
    fn shape_agent_start_prompt(
        &self,
        system_prompt: &str,
        working_prompt: &str,
        agent: Option<&str>,
        cwd: &str,
        prompt_state_key: Option<String>,
    ) -> HookOutcome {
        // (3) Hide ask/deny skills from `<available_skills>` + cache the enforcement entries. ONE
        // parse feeds both the enforcement cache (read at every `tool_call`) and the hidden prompt.
        let resolution = {
            let mut mgr = guard(&self.manager);
            sanitize::skills::resolve_skill_prompt_entries(working_prompt, &mut mgr, agent, cwd)
        };
        // pi `:1919` `activeSkillEntries = skillPromptResult.entries`.
        *guard(&self.active_skill_entries) = resolution.entries.clone();

        // pi `:1921-1924`: `systemPrompt` is ABSENT (not null) when the sanitizers changed nothing,
        // which is what decides between `{ systemPrompt }` and `{}` on both the fresh and the
        // cached path.
        let changed = (resolution.prompt != system_prompt).then_some(resolution.prompt);
        if let Some(key) = prompt_state_key {
            let mut cache = guard(&self.agent_start_cache);
            cache.last_prompt_state_key = Some(key);
            cache.last_prompt_state_result = Some(CachedPromptState {
                entries: resolution.entries,
                system_prompt: changed.clone(),
            });
        }

        match changed {
            None => HookOutcome::Noop,
            Some(prompt) => HookOutcome::Mutate(EventPatch::SystemPromptAndInject {
                system: Some(prompt),
                inject: None,
            }),
        }
    }

    /// pi `shouldExposeTool` (`index.ts:1791-1816` @v0.8.0; `:2049-2075` @v0.7.1 — the two are the
    /// same function, only `permanentApprovals` was dropped from the `applyPatternApprovalState`
    /// call): keep a tool exposed iff its TOOL-LEVEL permission
    /// ([`crate::PermissionManager::get_tool_permission`]) — with the session approval overlay (pi
    /// `applyPatternApprovalState(..., {}, ...)`, `:1795-1803`) — is not `deny`. There is **exactly
    /// one** bypass below that: a `deny` `read` is still exposed when the agent has allowed skills
    /// ([`crate::PermissionManager::has_allowed_skills`], pi `:1811-1813`) so it can reach skill files.
    /// Everything else denied at the tool level falls through to `false` (pi `:1815`).
    ///
    /// **No bash arm — deliberately (`PERM-009`).** Cyrup previously carried
    /// `if tool_name == "bash" && get_bash_permissions(agent).any_allow() { return true; }` here.
    /// Neither upstream tag has any such branch, and it was a **live permission bypass in the
    /// shipped binary**, reproduced end-to-end (`docs/gap-analysis/REPRO-LOG.md` §`PERM-009`):
    /// `tools.bash: deny` alone correctly withheld the tool, but adding the strictly NARROWER
    /// `bash: {"git status": "allow"}` to the same file re-exposed `bash`, and
    /// [`crate::PermissionManager::check_permission`]'s bash arm then resolved that command rule ABOVE the
    /// tool-level deny (`manager.rs`, pi `permission-manager.ts:944-959`), so real `git status`
    /// output came back. A rule that can only ever NARROW an allow must never widen a deny. The
    /// command-rule-over-`toolMatch` precedence in `manager.rs` is pi's and stays as-is; **this
    /// exposure check is the only thing that made a tool-level `bash` deny stick**, which is why
    /// the arm had to go rather than the precedence.
    ///
    /// The deleted arm's sole justification was an `R-NN-NNN` requirement id whose `spec/` tree is
    /// unrecoverable; `docs/adr/ADR-0008` retires such citations as authority (see its OQ-6, which
    /// independently found this branch justified by prose alone).
    fn should_expose_tool(&self, tool_name: &str, agent_name: Option<&str>) -> bool {
        let session_rules = guard(&self.session_approvals).get_rules();
        let mut mgr = guard(&self.manager);

        let raw = PermissionCheckResult {
            tool_name: tool_name.to_string(),
            state: mgr.get_tool_permission(tool_name, agent_name),
            matched_pattern: None,
            command: None,
            target: None,
            source: CheckSource::Tool,
        };
        let state = gate::apply_pattern_approval_state(raw, &json!({}), &session_rules).state;
        if state != PermissionState::Deny {
            return true;
        }
        // pi `:1811-1813` — the ONE bypass. Do not add a second (see the doc comment: PERM-009).
        if tool_name == "read" && mgr.has_allowed_skills(agent_name) {
            return true;
        }
        // pi `:1815`.
        false
    }
}
