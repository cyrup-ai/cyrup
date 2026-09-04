//! Persona/agent resolution: discovery configuration, alias resolution, model scope and
//! fork-context resolution.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::Mutex as AsyncMutex;

use crate::discovery::{discover_agents, AgentDiscoveryConfig};
use crate::discovery::types::{AgentDefinition, AgentReadScope};
use crate::error::SubagentError;
use crate::exec::ResolvedAgentPersona;
use crate::exec::model_scope::ModelScopeConfig;
use crate::fork_context::{ContextMode, ForkContext, ForkContextResolver};
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::paths::{
    builtin_agents_dir, enumerate_installed_packages, unreachable_session_manager,
};

impl SubagentExecutor {

    // ---------------------------------------------------------------------------------------
    // Discovery config assembly (bridges HostCtx.cwd -> a real AgentDiscoveryConfig)
    // ---------------------------------------------------------------------------------------

    /// Build a real [`AgentDiscoveryConfig`] scoped to `cwd`, resolving the full pi directory
    /// topology (agents.ts:220-222,1683-1725,1709-1711): an **upward project-root search**
    /// ([`crate::discovery::find_nearest_project_root`]) so a cwd nested below the project root
    /// still finds the project's agents; the legacy `<root>/.agents` project dir plus the preferred
    /// `<root>/.cyrup/agents` dir; the primary `~/.cyrup/agents` plus the "second" `~/.agents` user
    /// dir; **separate** `.cyrup/chains` chain dirs at each scope (never the shared agents dir); the
    /// bundled builtin-persona resource root ([`builtin_agents_dir`], R-SA-020/132/134); and
    /// R-SA-003's `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` extras (prepended, lowest User-tier precedence).
    ///
    /// # Package tier (Tier-2 wire-up)
    ///
    /// The package tier is now populated by enumerating `cyrup-resources`' own persisted
    /// `packages.json` install registries (Global under `<home>/.cyrup/packages.json`, Project under
    /// `<cwd>/.cyrup/packages.json`) via [`enumerate_installed_packages`], so a package that declares
    /// an `agents = [...]` manifest entry (R-SA-020) has its personas discovered at
    /// [`crate::discovery::types::AgentSource::Package`] scope by [`crate::discovery::scan_package_agents`]
    /// and its chain files (chains-share-agents-dir) discovered at Package scope by
    /// [`crate::discovery::scan_package_chain_scopes`]. R-SA-001's four-scope precedence
    /// (package first-seen-wins, then user/project last-seen-wins) now holds over all four populated
    /// tiers rather than three.
    ///
    /// `project_root` is `cwd` (the same base the `.cyrup/agents` project dir is derived from);
    /// `global_dir` is `<home>/.cyrup`. `trusted_project` is fail-closed (`false`): a Project-scope
    /// package's `agents` manifest entries are skipped until a project-trust decision is threaded in
    /// (the same not-yet-threaded seam this file documents for the live session-manager / settings
    /// layering — cyrup-config's DI-11 trust decision has no injection point into this extension
    /// today, so this crate never silently trusts a project's installed packages). Global-scope
    /// packages are always enumerated (trust-independent, matching `cyrup-resources`' own gate).
    pub(crate) fn discovery_dirs_config(
        cwd: &Path,
        roots: &crate::paths::Roots,
    ) -> Result<AgentDiscoveryConfig, SubagentError> {
        let home = roots.home().to_path_buf();
        let global_dir = home.join(".cyrup");
        // Upward project-root search — pi `findConfiguredProjectRoot` (`agents.ts:657-672`), which is
        // what `resolveNearestProjectAgentDirs`/`resolveNearestProjectChainDirs`/
        // `getProjectAgentSettingsPath`/`collectPackageSubagentPaths` all call, NOT the bare
        // `findNearestProjectRoot`: the nearest ancestor of `cwd` holding a `.cyrup` config dir or a
        // legacy `.agents` dir, EXCEPT that `subagents.projectRootResolution: "git-root"` (declared
        // at either the nearest candidate or at the git root itself) pulls resolution out to the
        // enclosing repository root, and `"nearest"` pins it in place. Absent any candidate, fall
        // back to `cwd` (pi's `?? cwd`) so package roots and the project write target still resolve
        // under `cwd/.cyrup`.
        //
        // A malformed `projectRootResolution` at either consulted root ABORTS (R-SA-009) rather than
        // silently degrading to "nearest" — which is why this function now returns a `Result`.
        let project_root = crate::discovery::find_configured_project_root(cwd)?
            .unwrap_or_else(|| cwd.to_path_buf());
        let installed_packages = enumerate_installed_packages(&global_dir, Some(&project_root));
        // Per-scope read dirs from the shared topology helpers (pi resolveNearestProject*Dirs /
        // discoverAgents userDir old+new / getUserChainDir): legacy `.agents` + preferred
        // `.cyrup/agents` for project agents; primary `.cyrup/agents` + second `~/.agents` for user
        // agents; a SEPARATE `.cyrup/chains` dir for each scope's chains (never the agents dir).
        Ok(AgentDiscoveryConfig {
            builtin_agents_dir: Some(builtin_agents_dir()),
            project_agent_dirs: crate::discovery::resolve_project_agent_read_dirs(&project_root),
            project_chain_dirs: crate::discovery::resolve_project_chain_read_dirs(&project_root),
            user_agent_dirs: crate::discovery::resolve_user_agent_read_dirs(&home),
            user_chain_dirs: crate::discovery::resolve_user_chain_read_dirs(&home),
            global_dir,
            project_root: Some(project_root),
            trusted_project: false,
            installed_packages,
            ..AgentDiscoveryConfig::default()
        }
        // R-SA-003: fold in `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` — PREPENDED ahead of the user dirs
        // (extras are the lowest-precedence User-tier stream), so the user's own agents win.
        .with_env_extras())
    }

    /// Build a real, fully-populated [`AgentDiscoveryConfig`] scoped to `cwd`: the directory/package
    /// topology from [`discovery_dirs_config`](Self::discovery_dirs_config) PLUS the `subagents.*`
    /// settings layer read from the user (`~/.cyrup/agents/settings.json`) and project
    /// (`<cwd>/.cyrup/agents/settings.json`) `settings.json` files (C2 wiring). The two scopes are
    /// layered per R-SA-012/133 by [`crate::discovery::load_layered_subagent_settings`] (project wins
    /// over user on every scalar and per-agent override name; a project `disableBuiltins: false`
    /// re-enables what a user `true` disabled), which then drives `merge.rs`'s
    /// `defaultModel`/`disableBuiltins`/`disableThinking`/`agentOverrides` application over the merged
    /// agents.
    ///
    /// # Errors
    ///
    /// Propagates [`SubagentError::MalformedSettings`] (R-SA-009) when either scope's `settings.json`
    /// exists but cannot be read, does not parse, is not a JSON object, or carries a malformed
    /// `subagents.*` field — the malformed-settings MUST-abort contract this crate's discovery
    /// callers rely on.
    pub(crate) fn discovery_config(
        &self,
        cwd: &Path,
        roots: &crate::paths::Roots,
    ) -> Result<AgentDiscoveryConfig, SubagentError> {
        let mut cfg = Self::discovery_config_on_disk(cwd, roots)?;
        // SUBA-084 — pi `discoverAgentsForRuntime` (`extension/index.ts:528-546` @v0.64.0) folds
        // `listRuntimeAgentConfigs(pi)` into every discovery; cyrup hands the executor's registry
        // snapshot to `run_discovery` through the config so EVERY consumer of this config —
        // tool routing, chains, nested control, the management `list`, the doctor — sees the
        // same runtime agents without each wiring its own merge.
        cfg.runtime_agents = self.runtime_agents().list();
        Ok(cfg)
    }

    /// [`Self::discovery_config`] MINUS the runtime agent registry: the directory/package topology
    /// plus the `subagents.*` settings layer, which is everything a settings-only reader
    /// ([`Self::resolve_model_scope`]) needs and all an executor-less caller can build.
    ///
    /// # Errors
    ///
    /// As [`Self::discovery_config`].
    pub(crate) fn discovery_config_on_disk(
        cwd: &Path,
        roots: &crate::paths::Roots,
    ) -> Result<AgentDiscoveryConfig, SubagentError> {
        let mut cfg = Self::discovery_dirs_config(cwd, roots)?;
        let user_settings = roots
            .home()
            .join(".cyrup")
            .join("agents")
            .join("settings.json");
        // pi `getProjectAgentSettingsPath` (`agents.ts:678-681`) keys the project settings file on
        // `findConfiguredProjectRoot(cwd)` — the SAME root the project agent/chain dirs came from —
        // not on the bare cwd. Keying it on `cwd` meant a session started in a subdirectory read a
        // settings file that does not exist while its agents came from the real project root, and it
        // made `subagents.projectRootResolution` unobservable: the very setting that MOVES the root
        // lives in the file the root selects.
        let project_settings = crate::discovery::project_settings_path(
            cfg.project_root.as_deref().unwrap_or(cwd),
        );
        // Tier 7: carry BOTH scopes UNFLATTENED (each with its own path) so `merge.rs` can resolve
        // project-beats-user at application time and record the true winning scope + path in
        // provenance (rather than a pre-flattened single scope that always looked like `Project`).
        cfg.override_settings = crate::discovery::load_layered_override_settings(
            &user_settings,
            Some(&project_settings),
        )?;
        Ok(cfg)
    }

    /// Resolve one agent by its fully-qualified runtime name (R-SA-008: exact string equality
    /// only), via the real, on-demand, re-scanned-per-call discovery pipeline (R-SA-019).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::AgentNotFound`] if no delegation-visible agent matches `name`
    /// exactly, or propagates a discovery-time [`SubagentError`] (R-SA-009's malformed-settings
    /// abort).
    pub fn resolve_agent(
        &self,
        cwd: &Path,
        name: &str,
        scope: AgentReadScope,
        roots: &crate::paths::Roots,
    ) -> Result<AgentDefinition, SubagentError> {
        self.resolve_agent_with_model_scope(cwd, name, scope, roots).map(|(agent, _, _)| agent)
    }

    /// [`Self::resolve_agent`] plus the effective `subagents.modelScope` policy this cwd's settings
    /// declare (SUBA-003) — pi's `discoverAgents` hands back `{ agents, modelScope }` together
    /// (`agents.ts:1727,1780` @v0.43.0), and an execution path needs BOTH: the persona to run, and the policy the
    /// model it runs on must satisfy. Returned as one call so the run path does not walk discovery
    /// twice (once for the agent, once for the settings) and can never see a scope read from a
    /// different point in time than the persona it is gating.
    ///
    /// # Errors
    ///
    /// Same as [`Self::resolve_agent`]: [`SubagentError::AgentNotFound`], or a discovery-time
    /// [`SubagentError::MalformedSettings`] — which now also covers a malformed `modelScope` block
    /// (R-SA-009's MUST-abort, rather than silently ignoring an unenforceable policy).
    /// SUBA-078: also returns the effective `subagents.maxThinking` ceiling, which travels on the
    /// SAME discovery result and is needed at the same seam. Deliberately returned here rather than
    /// stamped onto the [`AgentDefinition`] (which is upstream's mechanism): a value that never
    /// appears on the agent struct cannot be authored in frontmatter by an agent raising its own
    /// ceiling, and cannot be round-tripped into an agent file by the management serializer.
    pub fn resolve_agent_with_model_scope(
        &self,
        cwd: &Path,
        name: &str,
        scope: AgentReadScope,
        roots: &crate::paths::Roots,
    ) -> Result<(AgentDefinition, Option<ModelScopeConfig>, Option<String>), SubagentError> {
        let cfg = self.discovery_config(cwd, roots)?;
        let result = discover_agents(&cfg, Some(scope))?;
        let model_scope = result.model_scope.clone();
        let max_thinking = result.max_thinking.clone();
        // pi v0.43.0 routes EVERY execution-path agent lookup through `resolveAgentName`
        // (`subagent-executor.ts:1675-1680`'s `canonicalizeAgentName`, `preflight.ts:228`), so the
        // requested string may be an alias. It is name-first — a real agent named `x` always beats
        // another agent that merely lists `x` as an alias — and a non-unique alias is a HARD error
        // (`Ambiguous agent alias 'x': a, b`), never an arbitrary pick.
        let agent = match crate::discovery::resolve_agent_name(name, &result.agents) {
            crate::discovery::AgentNameResolution::Found(agent) => agent.clone(),
            // `Management` is the "already-exact upstream prose, no prefix" variant — the ambiguity
            // string is pi's own wording and reaches the caller unaltered.
            crate::discovery::AgentNameResolution::Ambiguous(msg) => {
                return Err(SubagentError::Management(msg));
            }
            crate::discovery::AgentNameResolution::NotFound => {
                return Err(SubagentError::AgentNotFound(name.to_string()));
            }
        };
        Ok((agent, model_scope, max_thinking))
    }

    /// The effective `subagents.modelScope` policy for `cwd` on its own (SUBA-003), without
    /// resolving any particular agent — for the multi-agent plan paths (`/chain`, `/parallel`,
    /// background runs), which resolve their personas through
    /// [`Self::resolve_plan_personas`] and need the policy as one value covering the whole plan.
    ///
    /// Reads only the two `settings.json` layers (via [`Self::discovery_config`]), not the agent
    /// directory walk.
    ///
    /// # Errors
    ///
    /// Propagates [`SubagentError::MalformedSettings`] (R-SA-009) when either scope's settings file
    /// is unreadable/unparseable or carries a malformed `subagents.*` field — including a malformed
    /// `modelScope` block, which MUST abort rather than degrade to unenforced.
    pub fn resolve_model_scope(
        cwd: &Path,
        roots: &crate::paths::Roots,
    ) -> Result<Option<ModelScopeConfig>, SubagentError> {
        Ok(Self::discovery_config_on_disk(cwd, roots)?.override_settings.model_scope())
    }

    /// Plan-time persona map (T0.1's C13 root-cause seam): resolve every DISTINCT agent named across
    /// a chain/parallel/background plan to its serializable [`ResolvedAgentPersona`], keyed by the
    /// step's `agent` name. This is the orchestrator half of T0.1 that the canonical
    /// [`crate::exec::resolve_step_agent_config`] resolver's own doc describes: the discovery lookup
    /// (name -> [`AgentDefinition`]) is done HERE — `extension.rs` is the ONE place with real
    /// discovery access (`crates/cyrup/src/subagent_runner_cmd.rs`'s hop-2 runner has none, which is
    /// exactly why `background/runner_main.rs`'s `ExecSingleStepExecutor` would otherwise synthesize a
    /// placeholder `AgentConfig{system_prompt_body:"", model:"default", completion_guard:Some(false),
    /// …}`) — then each resolved definition is projected via
    /// [`crate::exec::resolve_step_agent_config`].
    ///
    /// The returned map is stashed into [`crate::background::runner_main::RunnerConfig::resolved_agents`]
    /// (for a background run) or handed straight to
    /// [`crate::background::runner_main::ExecSingleStepExecutor::foreground`] (for a foreground
    /// `/chain`//`/parallel` run), so the runner dispatches the REAL persona (its own system prompt,
    /// model + fallback ladder, completion guard, per-step depth ceiling) and NEVER re-discovers.
    /// Resolving up front also validates every referenced agent EXISTS before any child process is
    /// spawned — matching pi, which validates agent names before starting a `/chain`/`/parallel`
    /// rather than spawning a partial run that dies mid-walk.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::AgentNotFound`] if any named agent resolves to no delegation-visible
    /// agent, or propagates a discovery-time [`SubagentError`] (R-SA-009's malformed-settings abort).
    pub fn resolve_plan_personas(
        &self,
        cwd: &Path,
        agent_names: impl IntoIterator<Item = String>,
        scope: AgentReadScope,
        roots: &crate::paths::Roots,
    ) -> Result<BTreeMap<String, ResolvedAgentPersona>, SubagentError> {
        let mut personas: BTreeMap<String, ResolvedAgentPersona> = BTreeMap::new();
        for name in agent_names {
            if personas.contains_key(&name) {
                continue;
            }
            let agent = self.resolve_agent(cwd, &name, scope, roots)?;
            personas.insert(name, crate::exec::resolve_step_agent_config(&agent));
        }
        Ok(personas)
    }

    // ---------------------------------------------------------------------------------------
    // Fork-context resolution (per-call throwaway resolver, see module doc)
    // ---------------------------------------------------------------------------------------

    /// Build a fresh, throwaway [`ForkContextResolver`] scoped to `cwd`. A new `SessionManager`
    /// handle is opened once per call and discarded after use — never retained, never shared, never
    /// mutated in place beyond this one resolution.
    ///
    /// # Fork-context correctness (blocker #4, reconciliation §4 step 5 item 5)
    ///
    /// When `session_file` is `Some` — the REAL live-orchestrator session file obtained from the P-1
    /// [`cyrup_ext::host::HostServices::session_file`] backend — the fork branches from THAT exact
    /// parent session (matching pi threading the real `parentSessionId`/`sessionFile`), opened via
    /// [`cyrup_session::SessionManager::open_with_cwd`]. This replaces the
    /// [`cyrup_session::SessionManager::continue_recent`] most-recent-mtime HEURISTIC, which can
    /// silently pick the WRONG session when a cwd has multiple sessions. The heuristic remains ONLY
    /// as the fallback for `None` (no host handle — the SDK-embedder / headless path), and for the
    /// (rare) case where the supplied session file cannot be opened.
    pub(crate) fn fork_resolver(
        cwd: &Path,
        session_file: Option<&Path>,
        roots: &crate::paths::Roots,
    ) -> ForkContextResolver {
        let sessions_root = roots.home().join(".cyrup").join("sessions");
        let layout = cyrup_session::SessionLayout::new(sessions_root.clone(), cwd.to_path_buf());
        // Blocker #4: prefer the real live-orchestrator session file (P-1) over the mtime heuristic.
        if let Some(path) = session_file
            && let Ok(manager) = cyrup_session::SessionManager::open_with_cwd(path, Some(cwd))
        {
            return ForkContextResolver::new(Arc::new(AsyncMutex::new(manager)), layout);
        }
        // `continue_recent` never fails in a way this resolver cannot itself handle: an absent
        // session directory yields a fresh, unpersisted, leafless in-memory session (R-SA-137's
        // fail-hard path handles that case correctly once `resolve(Fork, _)` is actually called);
        // a genuine I/O error is folded into the SAME "no resolvable session" outcome by treating
        // the resolver's underlying manager as absent — modeled here as an in-memory placeholder
        // so `ForkContextResolver::resolve` still runs its normal fail-hard checks rather than
        // this constructor itself needing to return a `Result` (every caller of this function
        // already only reaches it for a `context: "fork"` request, at which point
        // `resolve`'s own `is_persisted`/`leaf_id` checks are the authoritative fail-hard gate).
        let manager = cyrup_session::SessionManager::continue_recent(cwd, &layout)
            .or_else(|_| cyrup_session::SessionManager::in_memory(cwd, cyrup_session::NewSessionOpts::default()))
            .unwrap_or_else(|_| {
                // Even `in_memory` is documented infallible for a `None` id (see
                // `SessionManager::in_memory`'s own doc: "A `None` id is generated and never
                // fails"), so this arm is unreachable in practice; kept as a last-resort
                // in-memory fallback rather than a panic, matching this crate's no-panic policy.
                cyrup_session::SessionManager::in_memory(cwd, cyrup_session::NewSessionOpts::default())
                    .unwrap_or_else(|_| {
                        // Structurally unreachable (see above) but this crate forbids
                        // unwrap/expect/panic outside tests; the SessionManager type has no
                        // "empty" sentinel constructor, so the only remaining option that upholds
                        // both the no-panic policy and a total function signature is to retry
                        // once more with a definitely-valid cwd. Real production cwds are always
                        // valid paths by construction (HostCtx.cwd), so this loop terminates on
                        // the first or second attempt in every real scenario.
                        cyrup_session::SessionManager::in_memory(
                            Path::new("."),
                            cyrup_session::NewSessionOpts::default(),
                        )
                        .unwrap_or_else(|_| unreachable_session_manager())
                    })
            });
        ForkContextResolver::new(Arc::new(AsyncMutex::new(manager)), layout)
    }

    /// SUBA-079 / pi `canPreferFork(ctx.sessionManager)` (`shared/fork-context.ts:88` @v0.57.0):
    /// may an IMPLICIT `defaultContext: fork` cut a branch right now?
    ///
    /// Opens the parent session to read its leaf, exactly as [`Self::fork_resolver`]'s own consumer
    /// does moments later. Consulted ONLY when the call site named no explicit context, so the
    /// extra open happens only on launches where an implicit fork is genuinely in play.
    pub(crate) async fn can_prefer_fork(&self, cwd: &Path) -> bool {
        let session_file = self.host_services().and_then(|s| s.session_file());
        let roots = self.config_snapshot().await.roots;
        Self::fork_resolver(cwd, session_file.as_deref(), &roots)
            .can_prefer_fork()
            .await
    }

    /// Resolve one task's requested [`ContextMode`] into a concrete [`ForkContext`] (R-SA-137,
    /// fail-hard per DI-SA-2 — never silently downgrades to `Fresh`).
    ///
    /// SUBA-075: `force_thinking_off` is the caller's answer to "does the model this branch's
    /// child will run require reasoning disabled?" — pi's `forceThinkingOffForIndex` callback,
    /// which upstream likewise populates outside the resolver because the model ladder is not
    /// resolved until after the fork is requested. Compute it with
    /// [`crate::fork_context::forked_child_requires_thinking_off`]; pass `true` (upstream's own
    /// `?? true` fallback) when the ladder is not in hand.
    ///
    /// # Errors
    ///
    /// Propagates [`ForkContextResolver::resolve`]'s fail-hard errors.
    pub async fn resolve_context(
        &self,
        cwd: &Path,
        requested: ContextMode,
        force_thinking_off: bool,
    ) -> Result<ForkContext, SubagentError> {
        // Blocker #4: branch from the REAL live-orchestrator session file (P-1), not the mtime guess.
        let session_file = self.host_services().and_then(|s| s.session_file());
        let roots = self.config_snapshot().await.roots;
        let resolver = Self::fork_resolver(cwd, session_file.as_deref(), &roots);
        resolver.resolve(requested, 0, force_thinking_off).await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use crate::extension::testsupport::seed_scope_fixture;
    use crate::fork_context::ContextRequest;
    use cyrup_core::ModelId;

    /// SUBA-079: the executor-level wiring of pi `canPreferFork`. A cwd with no session at all
    /// cannot host a branch, so an INHERITED `defaultContext: fork` must resolve to `Fresh` and the
    /// launch must proceed — the headline behaviour of this item, which used to abort instead.
    #[tokio::test]
    async fn can_prefer_fork_is_false_when_there_is_no_persisted_parent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        assert!(
            !executor.can_prefer_fork(dir.path()).await,
            "no session file and no leaf => no branch can be cut"
        );
        // ...and that is exactly what downgrades the inherited preference.
        assert_eq!(
            crate::fork_context::resolve_effective_context(
                Option::None,
                "worker",
                Some(ContextMode::Fork),
                Option::None,
                executor.can_prefer_fork(dir.path()).await,
            )
            .expect("an inherited preference never errors"),
            ContextMode::Fresh
        );
    }

    // -----------------------------------------------------------------------------------------
    // G97 — alias resolution on the LIVE execution path
    // -----------------------------------------------------------------------------------------

    /// `resolve_agent` is the production lookup every single/plan launch goes through
    /// (`run_foreground`, `spawn_background`, `resolve_plan_personas`). It must accept an ALIAS —
    /// and the bundled personas really carry them (`resources/agents/oracle.md` declares
    /// `aliases: advisor`, `worker.md` declares `developer, coder, implementer, develop`), so this
    /// exercises the shipped resource root, not a synthetic fixture.
    #[tokio::test]
    async fn resolve_agent_accepts_a_bundled_personas_alias() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        for (alias, canonical) in [
            ("advisor", "oracle"),
            ("developer", "worker"),
            ("coder", "worker"),
            ("implementer", "worker"),
            ("develop", "worker"),
        ] {
            let agent = executor
                .resolve_agent(dir.path(), alias, AgentReadScope::Both, &crate::paths::Roots::from_env())
                .unwrap_or_else(|e| panic!("alias {alias:?} must resolve: {e}"));
            assert_eq!(agent.name, canonical, "alias {alias:?} resolved to the wrong agent");
        }
        // The canonical names still resolve to themselves.
        assert_eq!(
            executor
                .resolve_agent(dir.path(), "oracle", AgentReadScope::Both, &crate::paths::Roots::from_env())
                .expect("oracle resolves")
                .name,
            "oracle"
        );
    }

    /// An alias claimed by two DISTINCT agents refuses the launch outright, carrying pi's exact
    /// wording — never an arbitrary pick, and never a "not found".
    #[tokio::test]
    async fn resolve_agent_refuses_an_ambiguous_alias_on_the_live_path() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let project_agents = dir.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&project_agents).expect("mkdir");
        std::fs::write(
            project_agents.join("seer.md"),
            "---\nname: seer\ndescription: Sees\naliases: prophet\n---\n\nBody\n",
        )
        .expect("write");
        std::fs::write(
            project_agents.join("augur.md"),
            "---\nname: augur\ndescription: Augurs\naliases: prophet\n---\n\nBody\n",
        )
        .expect("write");

        // Each on its own still resolves, so the refusal below is about the collision and not about
        // the fixtures failing to be discovered at all.
        assert_eq!(
            executor
                .resolve_agent(dir.path(), "seer", AgentReadScope::Both, &crate::paths::Roots::from_env())
                .expect("seer resolves by name")
                .name,
            "seer"
        );

        let err = executor
            .resolve_agent(dir.path(), "prophet", AgentReadScope::Both, &crate::paths::Roots::from_env())
            .expect_err("an ambiguous alias must refuse");
        assert_eq!(err.to_string(), "Ambiguous agent alias 'prophet': augur, seer");
        assert!(
            !matches!(err, SubagentError::AgentNotFound(_)),
            "an ambiguous alias must NOT be reported as not found"
        );
    }

    #[tokio::test]
    async fn resolve_agent_returns_not_found_for_an_unknown_name() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let err = executor
            .resolve_agent(dir.path(), "no-such-agent", AgentReadScope::Both, &crate::paths::Roots::from_env())
            .expect_err("unknown agent must error");
        assert!(matches!(err, SubagentError::AgentNotFound(_)));
    }

    /// The thinking suffix must not defeat the policy: `<allowed>:max` is still the allowed model
    /// (pi strips a KNOWN suffix before matching), while `<disallowed>:max` is still refused and is
    /// REPORTED under its base id. `:max` is the 7th thinking level added by commit 6d29542.
    #[tokio::test]
    async fn a_thinking_suffix_neither_smuggles_a_model_in_nor_hides_one_from_the_report() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_scope_fixture(
            dir.path(),
            "scoped",
            Some(r#"{"subagents":{"modelScope":{"enforce":true,"allow":["anthropic/claude-opus-4"]}}}"#),
        );
        let executor = SubagentExecutor::new();

        let err = executor
            .run_foreground(
                dir.path(),
                "scoped",
                "t",
                Some(ContextRequest::Fresh),
                Some(ModelId::from("openai/gpt-5-nano:max")),
                None,
            )
            .await
            .expect_err("a thinking suffix must not smuggle an out-of-scope model past the gate");
        assert_eq!(
            err.to_string(),
            "Model 'openai/gpt-5-nano' is outside the configured subagent model scope. Allowed \
             patterns: anthropic/claude-opus-4.",
            "the reported model must be the BASE id, with the thinking suffix stripped"
        );

        // The mirror case is asserted at the decision boundary rather than through `run_foreground`,
        // because an ALLOWED model proceeds to a real subprocess spawn (this crate never fakes that).
        let scope = SubagentExecutor::resolve_model_scope(dir.path(), &crate::paths::Roots::from_env())
            .expect("settings parse")
            .expect("a modelScope block is configured");
        let mut available = Vec::new();
        let allowed = ModelId::from("anthropic/claude-opus-4:max");
        assert!(
            crate::exec::fallback::resolve_model_inheritance(
                Some(&allowed),
                None,
                None,
                &mut available,
                Some(&scope),
            )
            .is_ok(),
            "an ALLOWED model carrying a known thinking suffix must pass the gate unchanged"
        );
    }

    /// The refusal must be a REFUSAL, not a downgrade: the identical call with no `modelScope`
    /// configured must not produce a scope error at all, and the armed policy must never rewrite
    /// the requested model into an allowed one.
    #[tokio::test]
    async fn enforcement_is_off_without_a_policy_and_never_substitutes_an_allowed_model() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_scope_fixture(dir.path(), "scoped", None);
        assert_eq!(
            SubagentExecutor::resolve_model_scope(dir.path(), &crate::paths::Roots::from_env()).expect("settings parse"),
            None,
            "no settings block means no policy — enforcement stays off"
        );

        // With no policy, the exact model the caller asked for is what resolves.
        let mut available = Vec::new();
        let requested = ModelId::from("openai/gpt-5-nano");
        let resolved = crate::exec::fallback::resolve_model_inheritance(
            Some(&requested),
            None,
            None,
            &mut available,
            None,
        )
        .expect("no policy configured, so nothing can be refused");
        assert_eq!(resolved, crate::exec::fallback::ModelOverride::Explicit(requested.clone()));

        // With a policy that REFUSES it, the outcome is an error — never `Ok(<some other model>)`.
        let scope = crate::exec::model_scope::ModelScopeConfig {
            enforce: Some(true),
            strict: None,
            allow: Some(vec!["anthropic/*".to_string()]),
        };
        let refused = crate::exec::fallback::resolve_model_inheritance(
            Some(&requested),
            None,
            None,
            &mut available,
            Some(&scope),
        );
        assert!(
            refused.is_err(),
            "fail closed: an out-of-scope explicit model may not resolve to ANY model, {refused:?}"
        );
        assert!(
            available.is_empty(),
            "a refused resolution must not have mutated the availability set"
        );
    }

    /// R-SA-009: a malformed `modelScope` block ABORTS discovery rather than degrading to an
    /// unenforced policy — the fail-closed posture applied to the settings read itself. Before the
    /// fix, `SubagentSettings` had no such field and serde discarded every one of these silently.
    #[test]
    fn a_malformed_model_scope_block_aborts_discovery_instead_of_silently_disarming() {
        for (label, json) in [
            ("enforce without allow", r#"{"subagents":{"modelScope":{"enforce":true}}}"#),
            ("non-object", r#"{"subagents":{"modelScope":[]}}"#),
            ("non-boolean enforce", r#"{"subagents":{"modelScope":{"enforce":"yes"}}}"#),
            (
                "non-string allow entries",
                r#"{"subagents":{"modelScope":{"enforce":true,"allow":[1]}}}"#,
            ),
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            seed_scope_fixture(dir.path(), "scoped", Some(json));
            let err = SubagentExecutor::resolve_model_scope(dir.path(), &crate::paths::Roots::from_env())
                .expect_err(&format!("{label} must abort, not silently disarm the policy"));
            assert!(
                matches!(err, SubagentError::MalformedSettings(_)),
                "{label}: expected MalformedSettings, got {err:?}"
            );
        }
    }

    /// A well-formed block is actually READ (the SUBA-003 root cause: it was parsed by nothing),
    /// with project scope winning over user scope exactly as every other `subagents.*` scalar does.
    #[test]
    fn a_well_formed_model_scope_block_is_read_and_normalized() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_scope_fixture(
            dir.path(),
            "scoped",
            Some(r#"{"subagents":{"modelScope":{"enforce":true,"allow":["  anthropic/*  "]}}}"#),
        );
        let scope = SubagentExecutor::resolve_model_scope(dir.path(), &crate::paths::Roots::from_env())
            .expect("settings parse")
            .expect("the configured block must be read, not dropped");
        assert_eq!(scope.enforce, Some(true));
        assert_eq!(scope.allow, Some(vec!["anthropic/*".to_string()]), "patterns are trimmed");
        assert!(scope.is_armed());
    }

}
