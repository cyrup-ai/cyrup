//! The read-only report surfaces: doctor, cost and models.

use std::path::{Path, PathBuf};

use cyrup_core::ModelId;

use crate::discovery::types::{AgentDefinition, AgentSource};
use crate::registration::doctor::{build_doctor_report, DoctorReportInput};
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::paths::{dirs_home, format_configured_session_dir};
use crate::extension::host::slash_render::BUILTIN_AGENT_NAMES;
use crate::extension::models::{
    format_model_source, registry_available_models, resolve_default_model_scope,
    resolve_subagent_model_override,
};

impl SubagentExecutor {

    // ---------------------------------------------------------------------------------------
    // Registration surfaces: doctor / cost / profiles (delegates to already-implemented modules)
    // ---------------------------------------------------------------------------------------

    /// `/subagents-doctor` (R-SA-131; pi `buildDoctorReport`, doctor.ts:189-222): render the
    /// user-facing inventory report — a Runtime/session block, a Filesystem block naming the four
    /// scratch directories with each one's existence status, and a Discovery block with per-source
    /// agent/chain counts plus a skills inventory. This is pi's actual `/subagents-doctor` output
    /// (an inventory), distinct from [`crate::registration::doctor::DoctorRunner`]'s structured
    /// Ok/Warn/Fail check matrix (still available for programmatic diagnostics).
    ///
    /// `requested_session_dir` is pi's per-call `sessionDir` override (`paramsWithResolvedCwd.sessionDir`,
    /// `subagent-executor.ts:2828`) — an explicit value wins over the extension's own configured
    /// `default_session_dir`, which in turn wins over the literal `"not configured"` (pi
    /// `formatConfiguredSessionDir`, doctor.ts:108-116).
    pub async fn run_doctor(&self, cwd: &Path, requested_session_dir: Option<&str>) -> String {
        let roots = crate::background::run_artifact_roots(cwd);

        // pi wraps discovery in `lineFromCheck` (doctor.ts:65-71,131-153): a discovery failure (e.g.
        // R-SA-009's malformed-settings abort) must render `- agents/chains: failed — <err>` in the
        // Discovery block below, never a fabricated zero-count success — so the `Result` is
        // propagated all the way to `build_doctor_report`, never collapsed here.
        let discovery_result: Result<crate::discovery::AgentDiscoveryResult, String> =
            match Self::discovery_config(cwd) {
                Ok(discovery_config) => crate::discovery::discover_agents_all(&discovery_config)
                    .map_err(|err| err.to_string()),
                Err(err) => Err(err.to_string()),
            };

        // Session info: prefer the LIVE session manager (pi `ctx.sessionManager.getSessionFile()`/
        // `getSessionId()`, `subagent-executor.ts:2805-2813`) — the SAME live handle
        // `resolve_context` already uses (P-1) — over a per-cwd newest-mtime guess, which can name a
        // DIFFERENT session than the one the caller is actually in (another instance's newer
        // session, or a stale one). `root_parent_session` (captured once at this orchestrator's own
        // `SessionStart` from that same live `session_id()` call) is the state-held fallback pi's
        // `state.currentSessionId` plays (doctor.ts:124: `currentSessionId ?? state.currentSessionId
        // ?? "not available"`). Only when no live host is bound at all (headless/test) does this
        // degrade to the old newest-on-disk-by-mtime scan.
        let (session_file, session_id, session_error) = if let Some(services) = self.host_services()
        {
            let cached_id = self
                .root_parent_session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            (services.session_file(), services.session_id().or(cached_id), None)
        } else {
            let sessions_dir = Self::sessions_dir(cwd);
            match crate::registration::cost::find_latest_session_file_by_mtime(&sessions_dir).await
            {
                Ok(Some(path)) => match cyrup_session::SessionManager::open(&path) {
                    Ok(manager) => (
                        Some(path),
                        Some(manager.session_id().as_str().to_string()),
                        None,
                    ),
                    Err(err) => (Some(path), None, Some(err.to_string())),
                },
                Ok(None) => (None, None, None),
                Err(err) => (None, None, Some(err.to_string())),
            }
        };

        let cfg = self.config_snapshot().await;
        let configured_session_dir = format_configured_session_dir(
            requested_session_dir,
            cfg.default_session_dir.as_deref(),
        );

        let input = DoctorReportInput {
            cwd,
            // A background/async run is a re-exec of this very binary; async is available whenever
            // the current executable path resolves (pi `isAsyncAvailable`'s cyrup analog).
            async_available: std::env::current_exe().is_ok(),
            configured_session_dir,
            current_session_file: session_file,
            current_session_id: session_id,
            session_error,
            temp_root_dir: crate::background::temp_root_dir(),
            async_runs_dir: roots.async_root,
            results_dir: roots.results_dir,
            chain_runs_dir: crate::artifacts::chain_runs_dir(cwd),
            discovered: discovery_result.as_ref().map_err(|err| err.as_str()),
        };
        build_doctor_report(&input)
    }

    /// The per-`cwd` session-storage directory (`<home>/.cyrup/sessions/<encoded cwd>`), the same
    /// layout [`Self::fork_resolver`] opens — factored out so `/subagents-doctor` and
    /// `/subagent-cost` locate the session transcript identically.
    fn sessions_dir(cwd: &Path) -> PathBuf {
        cyrup_session::SessionLayout::new(
            dirs_home().join(".cyrup").join("sessions"),
            cwd.to_path_buf(),
        )
        .dir()
    }

    /// `/subagent-cost` (R-SA-140; pi `buildSubagentCostReport`, slash-commands.ts:377-416): walk
    /// this session's TRANSCRIPT (not a background status file) and report the parent's own
    /// assistant-message usage plus a per-child breakdown of every subagent `toolResult` recorded in
    /// the branch — so foreground subagent usage (which never mints a background run) is visible.
    /// Reads the newest on-disk session for `cwd` (pi walks the live `ctx.sessionManager`; cyrup has
    /// no live manager threaded into this extension, so the faithful analog is the same on-disk read
    /// [`Self::fork_resolver`]/`run_doctor` already use). Delegates the actual walk + rendering to
    /// [`crate::registration::cost::build_subagent_cost_report`].
    pub async fn run_cost_report(&self, cwd: &Path) -> String {
        self.cost_report_from_sessions_dir(&Self::sessions_dir(cwd))
            .await
    }

    /// The testable core of [`Self::run_cost_report`]: given a resolved session-storage directory,
    /// open its newest `.jsonl` transcript READ-ONLY (never creating one) and render the cost report
    /// over its branch. An absent/empty session directory renders the well-formed empty-state report
    /// rather than an error.
    async fn cost_report_from_sessions_dir(&self, sessions_dir: &Path) -> String {
        let _ = self; // no executor state needed; a method purely for call-site symmetry/testability.
        match crate::registration::cost::find_latest_session_file_by_mtime(sessions_dir).await {
            Ok(Some(path)) => match cyrup_session::SessionManager::open(&path) {
                Ok(manager) => crate::registration::cost::build_subagent_cost_report(
                    manager.branch_path(None),
                ),
                Err(err) => format!(
                    "subagent-cost: could not open session {}: {err}",
                    path.display()
                ),
            },
            Ok(None) => crate::registration::cost::build_subagent_cost_report(
                std::iter::empty::<&cyrup_session::Entry>(),
            ),
            Err(err) => format!(
                "subagent-cost: could not scan session directory {}: {err}",
                sessions_dir.display()
            ),
        }
    }

    /// `/subagents-models` (pi `handleModels`, agent-management.ts:802-869; slash dispatch
    /// slash-commands.ts:802-823): report the RUNTIME builtin-agent -> model mapping — each
    /// discovered builtin persona's effective model + the provenance of that model — NOT a dump of
    /// the full static provider catalog. `requested_agent` filters to a single builtin (pi's
    /// single-agent form), erroring with the available-builtins list when the name is not a
    /// discovered builtin.
    ///
    /// The live PARENT session model IS now threaded into this extension — read from the bound P-1
    /// [`cyrup_ext::host::HostServices`] backend via [`Self::inherited_session_model`] (pi's
    /// `ctx.model`) — so the "Current session model" line and an inheriting persona's effective model
    /// render the REAL `provider/id` instead of "(unavailable)". A persona that declares its own
    /// `model` still shows that (frontmatter / settings override / settings default); a persona with
    /// no model shows the inherited session model when a live session is bound, and only degrades to
    /// "(unavailable)"/"(inherits current session model)" when there is genuinely no live host
    /// (headless / SDK-embedder / no active model yet).
    #[must_use]
    pub fn run_models_report(&self, cwd: &Path, requested_agent: Option<&str>) -> String {
        // The live parent session model (pi `ctx.model`) an inheriting builtin resolves to; `None`
        // when no live session backend is bound (headless / SDK-embedder) — then the display degrades
        // to "(unavailable)" exactly as before this seam existed.
        let current_model = self.inherited_session_model().map(|m| m.as_str().to_string());
        let current_model = current_model.as_deref();
        // pi `ctx.model?.provider` (agent-management.ts:810) / the `ParentModel` a `model: undefined`
        // (or the `"inherit"` sentinel) resolves to (`resolveSubagentModelOverride`,
        // model-fallback.ts:196-220): both split off the SAME live `provider/id` string.
        let preferred_provider = current_model
            .and_then(|m| m.split_once('/'))
            .map(|(provider, _)| provider);
        let parent_model = current_model.and_then(|m| m.split_once('/'));
        let available_models = registry_available_models();

        // Both fall-backs are best-effort here (this surface reports, it does not run anything), so a
        // malformed `settings.json` OR a malformed `projectRootResolution` degrades to the bare
        // default config rather than aborting the whole report.
        let cfg = Self::discovery_config(cwd)
            .or_else(|_| Self::discovery_dirs_config(cwd))
            .unwrap_or_default();
        let default_model_scope = resolve_default_model_scope(&cfg.override_settings);
        let discovered = match crate::discovery::discover_agents_all(&cfg) {
            Ok(discovered) => discovered,
            Err(err) => return format!("subagents-models: discovery failed: {err}"),
        };

        let builtin_by_name: std::collections::HashMap<&str, &AgentDefinition> = discovered
            .agents
            .iter()
            .filter(|agent| agent.source == AgentSource::Builtin)
            .map(|agent| (agent.name.as_str(), agent))
            .collect();

        // pi `params.agent?.trim()` (agent-management.ts:803): a whitespace-only/empty `agent`
        // string is JS-falsy and treated as "no agent requested" — it falls through to the
        // all-agents view below, it is NOT looked up as a builtin named "".
        let requested_agent = requested_agent.map(str::trim).filter(|s| !s.is_empty());

        if let Some(requested) = requested_agent {
            // pi's first gate (agent-management.ts:581-583) checks the STATIC `BUILTIN_AGENT_NAMES`
            // list, not whatever discovery happened to find.
            if !BUILTIN_AGENT_NAMES.contains(&requested) {
                return format!(
                    "Builtin agent '{requested}' not found. Available: {}.",
                    BUILTIN_AGENT_NAMES.join(", ")
                );
            }
            // pi's second gate (agent-management.ts:589-590): the name is a real builtin name, but
            // discovery didn't resolve it (a broken/incomplete build) — a DIFFERENT message with no
            // "Available" suffix, since the name was already validated above.
            let Some(agent) = builtin_by_name.get(requested).copied() else {
                return format!("Builtin agent '{requested}' not found.");
            };

            let requested_model_str = agent.model.as_ref().map(ModelId::as_str);
            let resolved_model = resolve_subagent_model_override(
                requested_model_str,
                parent_model,
                &available_models,
                preferred_provider,
            );
            let mut lines = vec![
                "Builtin subagent model".to_string(),
                String::new(),
                format!("Agent: {requested}"),
                "Effective model:".to_string(),
                format!("  {}", resolved_model.as_deref().unwrap_or("(unresolved)")),
                format!(
                    "Source: {}",
                    format_model_source(agent, current_model, default_model_scope)
                ),
            ];
            if let Some(override_info) = &agent.override_info {
                lines.push("Override file:".to_string());
                lines.push(format!("  {}", override_info.settings_path.display()));
            }
            // pi `agent.model && resolvedModel && agent.model !== resolvedModel`
            // (agent-management.ts:596-599): only shown when the persona declared a raw setting AND
            // it differs from the resolved model (e.g. a bare id resolved to its full `provider/id`,
            // or an explicit override that isn't the resolved value).
            if let (Some(raw), Some(resolved)) = (requested_model_str, resolved_model.as_deref())
                && raw != resolved
            {
                lines.push("Requested model setting:".to_string());
                lines.push(format!("  {raw}"));
            }
            if agent.disabled == Some(true) {
                lines.push("Disabled: true".to_string());
            }
            lines.push("Current session model:".to_string());
            lines.push(format!("  {}", current_model.unwrap_or("(unavailable)")));
            // SUBA-035: the policy that decides whether the resolved model above is even allowed to
            // run. Without it this report can show a model that enforcement will refuse, and say
            // nothing about why.
            lines.push("Model scope:".to_string());
            lines.push(crate::exec::model_scope::model_scope_summary_line(
                discovered.model_scope.as_ref(),
            ));
            return lines.join("\n");
        }

        let mut lines = vec![
            "Builtin subagent models".to_string(),
            String::new(),
            "Current session model:".to_string(),
            format!("  {}", current_model.unwrap_or("(unavailable)")),
            // SUBA-035 — see the single-agent view above; the same line, on the same report.
            "Model scope:".to_string(),
            crate::exec::model_scope::model_scope_summary_line(discovered.model_scope.as_ref()),
            String::new(),
        ];
        // pi's all-agents view walks the fixed `BUILTIN_AGENT_NAMES` list (agent-management.ts:608),
        // not whatever discovery happened to find — a name discovery didn't resolve gets its own
        // "missing" row rather than silently shrinking the report.
        for name in BUILTIN_AGENT_NAMES {
            let Some(agent) = builtin_by_name.get(name).copied() else {
                lines.push(name.to_string());
                lines.push("  model:".to_string());
                lines.push("    (builtin definition not found)".to_string());
                lines.push("  source: missing".to_string());
                lines.push(String::new());
                continue;
            };
            let requested_model_str = agent.model.as_ref().map(ModelId::as_str);
            let resolved_model = resolve_subagent_model_override(
                requested_model_str,
                parent_model,
                &available_models,
                preferred_provider,
            );
            let disabled_suffix = if agent.disabled == Some(true) {
                "; disabled"
            } else {
                ""
            };
            lines.push(name.to_string());
            lines.push("  model:".to_string());
            lines.push(format!(
                "    {}",
                resolved_model.as_deref().unwrap_or("(unresolved)")
            ));
            lines.push(format!(
                "  source: {}{disabled_suffix}",
                format_model_source(agent, current_model, default_model_scope)
            ));
            lines.push(String::new());
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    // ---------------------------------------------------------------------------------------
    // `/subagent-cost` walks the SESSION TRANSCRIPT (pi `buildSubagentCostReport`,
    // slash-commands.ts:377-416), not a background status file: this drives the REAL production
    // command path (`SubagentExecutor::run_cost_report` -> `cost_report_from_sessions_dir`) end to
    // end over a real on-disk session (created + appended via `cyrup_session::SessionManager`,
    // reloaded via `SessionManager::open`), proving the command sums the parent's own assistant
    // usage plus every subagent child's usage from the transcript. (The recursive nested-run
    // accumulator `registration::cost::compute_recursive_cost` remains a separate capability with
    // its own exhaustive unit tests in that module.)
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn run_cost_report_walks_the_session_transcript() {
        let tmp = tempfile::tempdir().expect("real tempdir");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).expect("mkdir cwd");

        // A real, persisted session: one parent assistant turn (usage 200/100, $0.02) + one subagent
        // toolResult carrying a child result (usage 50/25, $0.005). `SessionManager::create` +
        // `append_message` write these to disk exactly as a live run would.
        let layout = cyrup_session::SessionLayout::new(tmp.path().join("sessions"), cwd.clone());
        let mut manager = cyrup_session::SessionManager::create(
            &cwd,
            &layout,
            cyrup_session::NewSessionOpts::default(),
        )
        .expect("create session");

        let user: cyrup_core::Message = serde_json::from_value(serde_json::json!({
            "role": "user",
            "content": [{ "type": "text", "text": "go" }],
            "timestamp": 0,
        }))
        .expect("user message");
        manager.append_message(user).expect("append user");

        let assistant: cyrup_core::Message = serde_json::from_value(serde_json::json!({
            "role": "assistant",
            "content": [{ "type": "text", "text": "ok" }],
            "provider": "anthropic",
            "model": "claude-sonnet-4",
            "usage": {
                "input": 200, "output": 100, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 300,
                "cost": { "input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.02 },
            },
            "stopReason": "stop",
            "timestamp": 1,
        }))
        .expect("assistant message");
        manager.append_message(assistant).expect("append assistant");

        let tool_result: cyrup_core::Message = serde_json::from_value(serde_json::json!({
            "role": "toolResult",
            "toolCallId": "call-1",
            "toolName": "subagent",
            "content": [{ "type": "text", "text": "done" }],
            "details": {
                "mode": "single",
                "results": [{
                    "agent": "worker",
                    "usage": {
                        "input": 50, "output": 25, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 75,
                        "cost": { "input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.005 },
                    },
                }],
            },
            "timestamp": 2,
        }))
        .expect("tool result message");
        manager.append_message(tool_result).expect("append tool result");

        let executor = SubagentExecutor::new();
        let report = executor.cost_report_from_sessions_dir(&layout.dir()).await;

        assert!(report.starts_with("Subagent cost\n"), "{report}");
        assert!(report.contains("Parent: ↑200 ↓100"), "parent assistant usage: {report}");
        assert!(report.contains("Child 1 (worker)"), "per-child breakdown: {report}");
        assert!(report.contains("Children: ↑50 ↓25"), "child subtotal: {report}");
        // Parent (200/100) + child (50/25) summed into the grand Total (250/125), with cost summed.
        assert!(report.contains("Total: ↑250 ↓125"), "parent+child total: {report}");
        assert!(report.contains("$0.0250"), "total cost sums parent+child: {report}");
    }

    // ---------------------------------------------------------------------------------------
    // `/subagents-models` reports the RUNTIME builtin-agent -> model mapping (pi `handleModels`),
    // NOT the static provider catalog. Env-agnostic: asserts the mapping header/shape and the
    // unknown-builtin rejection, which hold whether or not the bundled builtins resolve in this
    // ambient test environment.
    // ---------------------------------------------------------------------------------------

    #[test]
    fn run_models_report_renders_runtime_mapping_and_rejects_unknown_builtin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();

        let full = executor.run_models_report(dir.path(), None);
        assert!(
            full.starts_with("Builtin subagent models\n"),
            "the report must be the runtime builtin->model mapping, not a catalog dump: {full}"
        );
        assert!(full.contains("Current session model:"), "{full}");
        // SUBA-035: the report must say which scope policy is in force, not only which model
        // resolved. With no `subagents.modelScope` configured that is the explicit "none" line —
        // asserted here rather than only in the configured case so a silently-dropped block fails.
        assert!(
            full.contains("Model scope:\n  (none configured"),
            "the models report must surface the active modelScope policy: {full}"
        );
        // The old behavior dumped the static provider catalog ("... — context {n}k, reasoning=...");
        // the runtime mapping must not.
        assert!(
            !full.contains("reasoning="),
            "must not dump the static provider catalog: {full}"
        );

        let unknown = executor.run_models_report(dir.path(), Some("definitely-not-a-builtin"));
        assert!(
            unknown
                .contains("Builtin agent 'definitely-not-a-builtin' not found. Available:"),
            "an unknown builtin name must be rejected with the available list: {unknown}"
        );
    }

    // ---------------------------------------------------------------------------------------
    // pi-parity regressions (agent-management.ts `handleModels`/`formatModelSource`): the
    // "(unresolved)" placeholder (not a bespoke "inherits current session model" string), the
    // `"inherit"` sentinel resolving through the parent model instead of printing verbatim, the
    // "Requested model setting" block, and `formatModelSource`'s model-equality / scope gates.
    // ---------------------------------------------------------------------------------------

    #[test]
    fn run_models_report_uses_pi_unresolved_placeholder_not_inherits_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        // No host bound (no live session model) and `delegate.md` declares no `model:` of its own,
        // so pi's `resolveSubagentModelOverride` has nothing to resolve to and `handleModels`
        // renders its exact `"(unresolved)"` placeholder (agent-management.ts:813) — pre-fix this
        // crate rendered the bespoke, non-pi "(inherits current session model)" text instead.
        let report = executor.run_models_report(dir.path(), Some("delegate"));
        assert!(
            report.contains("Effective model:\n  (unresolved)"),
            "must render pi's exact '(unresolved)' placeholder when nothing can be resolved: {report}"
        );
        assert!(
            !report.contains("inherits current session model"),
            "must not render the bespoke non-pi placeholder text: {report}"
        );
    }

    #[test]
    fn run_models_report_gates_override_provenance_on_actual_model_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings_dir = dir.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&settings_dir).expect("mkdir settings dir");
        // This override only ever touches `disabled` — it never applies to `model` — so the
        // resolved model provenance must NOT claim an "override" changed it (pi
        // `agent.override && agent.model !== agent.override.base.model`, agent-management.ts:788-790).
        std::fs::write(
            settings_dir.join("settings.json"),
            r#"{"subagents":{"agentOverrides":{"scout":{"disabled":true}}}}"#,
        )
        .expect("write settings.json");

        let executor = SubagentExecutor::new();
        let report = executor.run_models_report(dir.path(), Some("scout"));
        assert!(
            !report.contains("Source: project override"),
            "an override that never touched `model` must not claim '{{scope}} override' \
             provenance for the model: {report}"
        );
        assert!(
            report.contains("Source: inherit requested, but no current session model is available"),
            "with no model configured anywhere and no live session model, pi's unresolved-\
             fallback text must still apply: {report}"
        );
    }

    #[test]
    fn run_models_report_scopes_default_model_provenance_by_settings_scope() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings_dir = dir.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&settings_dir).expect("mkdir settings dir");
        std::fs::write(
            settings_dir.join("settings.json"),
            r#"{"subagents":{"defaultModel":"acme/shared-default"}}"#,
        )
        .expect("write settings.json");

        let executor = SubagentExecutor::new();
        let report = executor.run_models_report(dir.path(), Some("scout"));
        // pi `agent.modelSource.type === "subagents.defaultModel"` renders `${scope} defaultModel`
        // (agent-management.ts:791-793); the pre-fix text hardcoded the unscoped "settings
        // defaultModel" regardless of which settings scope actually supplied the default.
        assert!(
            report.contains("Source: project defaultModel"),
            "a project-scope subagents.defaultModel must render scope-qualified provenance, not \
             the bespoke unscoped 'settings defaultModel' text: {report}"
        );
        assert!(
            !report.contains("settings defaultModel"),
            "must not render the bespoke unscoped provenance text: {report}"
        );
    }

    /// PROV-007: `/subagents-models` resolves a BARE model id against the real built-in model
    /// registry (pi `resolveModelCandidate` over `ctx.modelRegistry.getAvailable()`,
    /// model-fallback.ts:148-164), so a persona configured with a bare id from ANY registered
    /// provider renders its `provider/id`. The retired 2-model seed stub could only ever resolve
    /// `claude-sonnet-4-5`/`gpt-4o`; every other bare id fell through pi's "no match" fallback and
    /// rendered verbatim.
    ///
    /// The subject model is picked from the registry itself — the first model whose bare id is
    /// registry-unique and whose provider is neither `anthropic` nor `openai` — so a catalog
    /// refresh cannot rot this test.
    #[test]
    fn run_models_report_resolves_a_bare_id_from_any_registered_provider() {
        // Read the fixture straight from `cyrup-provider` (NOT through `registry_models`) so this
        // test fails on the RENDERED REPORT when the binding regresses, not on fixture selection.
        let registry = cyrup_provider::catalog::builtin_catalog();
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for m in registry {
            *counts.entry(m.id.as_str()).or_default() += 1;
        }
        let subject = registry
            .iter()
            .find(|m| {
                counts.get(m.id.as_str()).copied() == Some(1)
                    && !matches!(m.provider.as_str(), "anthropic" | "openai")
                    // A bare id containing ':' cannot resolve in strict mode, and that is
                    // UPSTREAM behaviour, not a cyrup gap: `model-resolver.ts:203-256` splits on
                    // the LAST colon, and when the suffix is not a valid thinking level the strict
                    // path returns no model rather than guessing (`:238-243`). cyrup's
                    // `parse_pattern` mirrors it exactly. Since `amazon-bedrock` was ported the
                    // registry carries such ids — `amazon.nova-2-lite-v1:0` — and this `find`
                    // picked one, which is what made this test start failing. Excluding them keeps
                    // the test about bare-id EXPANSION, which is what it is named for.
                    && !m.id.as_str().contains(':')
            })
            .expect("the registry must carry a unique, colon-free bare id outside anthropic/openai");
        let bare = subject.id.as_str();
        let full = format!("{}/{}", subject.provider.as_str(), bare);

        let dir = tempfile::tempdir().expect("tempdir");
        let settings_dir = dir.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&settings_dir).expect("mkdir settings dir");
        std::fs::write(
            settings_dir.join("settings.json"),
            format!(r#"{{"subagents":{{"defaultModel":"{bare}"}}}}"#),
        )
        .expect("write settings.json");

        let executor = SubagentExecutor::new();
        let report = executor.run_models_report(dir.path(), Some("scout"));
        assert!(
            report.contains(&format!("Effective model:\n  {full}")),
            "a bare id from '{}' must resolve to its full provider/id against the real registry: \
             {report}",
            subject.provider.as_str()
        );
        // pi surfaces the raw declared setting once it differs from the resolved model
        // (agent-management.ts:596-599) — proof the resolution really happened.
        assert!(
            report.contains(&format!("Requested model setting:\n  {bare}")),
            "the raw bare id must still be surfaced alongside the resolved full id: {report}"
        );
    }

    /// Divergence regression: pre-fix, `run_doctor` never consulted `params.sessionDir` at all —
    /// the report's `- configured session dir:` line was always the hardcoded computed sessions
    /// directory. This drives the REAL `SubagentExecutor::run_doctor` (not just the pure formatter)
    /// end to end and fails against that pre-fix behavior.
    #[tokio::test]
    async fn run_doctor_report_honors_a_per_call_session_dir_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();

        let report = executor.run_doctor(dir.path(), Some("/abs/custom-sessions")).await;
        assert!(
            report.contains("- configured session dir: /abs/custom-sessions"),
            "an explicit per-call sessionDir must be rendered verbatim (resolved): {report}"
        );

        let report_default = executor.run_doctor(dir.path(), None).await;
        assert!(
            report_default.contains("- configured session dir: not configured"),
            "with no per-call override and no configured default_session_dir, pi's literal \
             \"not configured\" must render, not the always-on computed sessions dir: \
             {report_default}"
        );

        {
            let mut cfg = executor.config_cell().lock().await;
            cfg.default_session_dir = Some(PathBuf::from("/abs/configured-default"));
        }
        let report_configured_default = executor.run_doctor(dir.path(), None).await;
        assert!(
            report_configured_default
                .contains("- configured session dir: /abs/configured-default"),
            "the extension's own configured default_session_dir must be consulted when no \
             per-call override is present: {report_configured_default}"
        );
    }

}
