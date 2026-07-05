//! The [`PermissionSystemExtension`] `NativeExtension` facade + the `before_tool_call` gate
//! orchestration + the binary wiring entry point (port of pi `index.ts` hook wiring).
//!
//! WIRING (all in this Phase-0 build, no dead primitives):
//! - `init` subscribes `[ToolCall, SessionStart, SessionShutdown]` — so the ToolCall subscription is
//!   honored by the block/mutate dispatcher (`cyrup-ext/src/dispatch.rs`) that `ExtHooks::
//!   before_tool_call` (`hooks.rs:31-44`, R-08-010) drives.
//! - `on_event(ToolCall)` runs [`PermissionSystemExtension::decide`] → `HookOutcome::Block` (deny /
//!   fail-closed ask) or `Noop` (allow), which the dispatcher turns into `BeforeOutcome::Block` /
//!   proceed. THIS is the deciding gate: a real risky tool call (bash/write/edit) is genuinely
//!   intercepted and gated.
//! - `on_event(SessionStart|SessionShutdown)` clears the session store + dedup (pi
//!   `index.ts:2089,2123`).
//! - [`permission_extension_for_env`] is called at all three `crates/cyrup/src/main.rs` session-build
//!   sites (the `.with_native_extension(...)` seam), gated on child-mode (consistent with subagents)
//!   and DI-5 opt-in.
//!
//! LIVE HUMAN DIALOG (this build, P-1/P-3): an `ask` on a tool the interactive human drives now
//! surfaces the real permission dialog. [`PermissionSystemExtension::set_host_services`] captures the
//! late-bound `Arc<dyn HostServices>` (the SAME `LiveHostServices` the builder threads via
//! `load_native_with_services`); when `ctx.has_ui`, `resolve_ask` prompts through
//! [`LocalAskChannel`] (`HostServices::select`/`input`, pi `permission-dialog.ts`). The blocking
//! dialog is held under a [`HostCtx::begin_human_wait`] P-3 guard so the dispatcher's 5s invocation
//! budget is SUSPENDED for the human latency instead of firing and fail-OPENing the gate
//! (reconciliation §2 / port §4). "Allow Always" persists to the session approval store (pi
//! `index.ts:905`); headless / no-UI contexts still fail-CLOSE to `Block`.
//!
//! CHILD→PARENT ASK-FORWARDING (this build, P-4, `forwarding.rs`): a subagent CHILD loads the gate
//! with a [`ForwardingAskChannel`] ([`Self::new_forwarding_child`]) — an ask-tier decision writes a
//! nonce-bound request into the PARENT's spool (addressed by the `CYRUP_SUBAGENT_PARENT_SESSION`
//! anchor `cyrup-ext-subagents` emits, `exec/mod.rs` `PARENT_SESSION_ENV_VAR`) and BLOCKS on the bound
//! response under the P-3 `begin_human_wait` guard. The PARENT ([`Self::new_forwarding_parent`])
//! installs a spawned [`forwarding::spawn_forwarding_watcher`] task on `SessionStart` that surfaces
//! each forwarded prompt to its human (the SAME `select`/`input` dialog + C3 human-interaction lock a
//! local ask uses) and writes the decision back; the child's `apply_decision` then persists an
//! "Allow Always" into the child's session store exactly like a local ask (pi `index.ts:905`).
//!
//! FOUR SUPPLEMENTARY LAYERS (all wired + enforcing, pi `index.ts:2208-2499`): [`Self::decide`] runs
//! the agent + projectAgent policy layers (keyed by [`resolve_agent_name_from_env`], the
//! `CYRUP_SUBAGENT_AGENT_NAME` spawn anchor), the registry / unknown-tool block (against
//! [`cyrup_ext::HostServices::all_tool_names`]), the skill-read bypass (against the `before_agent_start`
//! `<available_skills>` entries, [`crate::skill`]), and the external-directory guard (against
//! [`HostCtx::cwd`]). The `before_agent_start` CONTEXT-HYGIENE layer is ALSO wired (pi
//! `index.ts:2134-2190`, port doc §9): [`Self::on_before_agent_start`] shapes the active tool set
//! ([`Self::should_expose_tool`] → [`cyrup_ext::HostServices::set_active_tools`]), RETURNS the
//! sanitized system prompt as a `[mutate]` ([`crate::sanitize`]), and surfaces the yolo status pill
//! ([`crate::status`]). Every layer — deciding gate AND context-hygiene — is live here, none stubbed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use async_trait::async_trait;
use serde_json::{json, Value};

use cyrup_core::ExtensionId;
use cyrup_ext::{
    EventKind, EventPatch, ExtError, HostCtx, HostEvent, HookOutcome, HostServices, InitApi,
    NativeExtension,
};

use crate::ask::{
    AskChannel, AskOutcome, ForwardingAskChannel, LocalAskChannel, NoOpAskChannel,
    PermissionDecisionState, PermissionPromptDecision, PromptOpts,
};
use crate::common::{self, to_record};
use crate::dedup::{DedupCache, DedupDetails};
use crate::ext_config::ExtensionConfig;
use crate::forwarding;
use crate::gate;
use crate::manager::{ManagerPaths, PermissionManager};
use crate::sanitize;
use crate::skill::{self, SkillPromptEntry};
use crate::status;
use crate::stores::{PermanentApprovalStore, SessionApprovalStore};
use crate::types::{CheckSource, PermissionCheckResult, PermissionState};

/// The extension's fixed id (pi `EXTENSION_ID`, `extension-config.ts:8`).
pub const EXTENSION_ID: &str = "cyrup-permission-system";

/// The global policy file (pi `pi-permissions.jsonc`; cyrup analog).
const POLICY_FILE: &str = "cyrup-permissions.jsonc";
/// The read-through permanent approvals file (pi `pi-permission-system-approvals.json`; cyrup analog).
const PERMANENT_APPROVALS_FILE: &str = "cyrup-permission-system-approvals.json";
/// The extension config dir + file (`<agent_dir>/cyrup-permission-system/config.json`).
const CONFIG_DIR: &str = "cyrup-permission-system";
const CONFIG_FILE: &str = "config.json";
/// The project-scoped policy dir (pi `<cwd>/.pi/agent`; cyrup `<cwd>/.cyrup/agent`).
const PROJECT_AGENT_SUBDIR: [&str; 2] = [".cyrup", "agent"];

/// The subagent-child env flag (value `"1"`) — the SAME var `cyrup-ext-subagents` sets
/// (`spawn::nested_events::CHILD_ENV = "CYRUP_SUBAGENT_CHILD"`). Read by literal name here to avoid a
/// dependency on that crate for one const (P-5 discusses depending on it later for `control.rs`
/// reuse).
pub const CHILD_ENV_VAR: &str = "CYRUP_SUBAGENT_CHILD";
/// The explicit opt-in flag (DI-5): set truthy to force-install the gate even with no policy file.
pub const INSTALL_ENV_VAR: &str = "CYRUP_PERMISSION_SYSTEM";

/// The permission-system extension: the layered policy engine + two approval stores + prompt dedup +
/// the fail-closed ask channel, gating every tool call via `before_tool_call`.
pub struct PermissionSystemExtension {
    id: ExtensionId,
    manager: Mutex<PermissionManager>,
    session_approvals: Mutex<SessionApprovalStore>,
    permanent_approvals: Mutex<PermanentApprovalStore>,
    dedup: Mutex<DedupCache>,
    /// The extension `config.json` snapshot. `yolo_mode` is read on the live `ask` path (below);
    /// `debug`/`forwarded_prompt_timeout_seconds` are consumed by later phases (logging / forwarding
    /// P-4) — the public fields carry them without any callerless primitive.
    config: ExtensionConfig,
    /// The fail-closed FALLBACK ask channel ([`NoOpAskChannel`] in production; a scripted channel in
    /// unit tests via [`Self::from_parts`]). Used when no live UI is reachable — the live in-session
    /// dialog goes through [`LocalAskChannel`] over [`Self::host_services`] instead.
    ask_channel: Arc<dyn AskChannel>,
    /// The late-bound live capability backend (P-1), captured by [`Self::set_host_services`] BEFORE
    /// `init` (the builder threads its `LiveHostServices` via `load_native_with_services`). `Some` in
    /// an assembled interactive session; the in-session `ask` dialog uses it (guarded by `ctx.has_ui`)
    /// to reach `HostServices::select`/`input`, and the PARENT forwarding watcher uses it (outside any
    /// `HostCtx`) to reach `session_id`/`select`/`input`/`human_interaction_lock`. This is the SAME
    /// `OnceLock` a child's [`ForwardingAskChannel`] shares (so it observes the child's own session id
    /// for the requester metadata). `None` (default host / headless) ⇒ fail-closed / no watcher.
    host_services: Arc<OnceLock<Arc<dyn HostServices>>>,
    /// The agent dir whose `sessions/permission-forwarding/…` subtree is the shared forwarding spool
    /// root (pi `PI_AGENT_DIR`). The parent watcher resolves its inbox under this.
    agent_dir: PathBuf,
    /// Whether this extension is a PARENT (root) that installs the forwarding watcher on
    /// `SessionStart`. `true` only for [`Self::new_forwarding_parent`]; a child ([`ForwardingAskChannel`]
    /// role) and the bare test constructors ([`Self::new`]/[`Self::from_parts`]) leave it `false`.
    install_watcher: bool,
    /// The live forwarding-watcher task handle (parent role), so `SessionShutdown` can `abort()` it
    /// (pi teardown, `index.ts:2131`) and a session rebuild does not double-spawn.
    watcher: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The resolved persona/agent name this process runs as (pi `resolveAgentName`, `index.ts:
    /// 2033-2047`), captured ONCE from the `CYRUP_SUBAGENT_AGENT_NAME` env var at construction (the
    /// child IS its persona for its whole lifetime). `Some` threads the agent + projectAgent policy
    /// LAYERS into `check_permission` / `format_*` / dedup; `None` (a top-level process, which never
    /// has the var) matches pi's normalized-`""` top-level (global + project layers still enforce).
    agent_name: Option<String>,
    /// The active-skill enforcement entries parsed out of the `before_agent_start` system prompt's
    /// `<available_skills>` block (pi `activeSkillEntries`, `index.ts:1558` / `resolveSkillPromptEntries`,
    /// built at `before_agent_start`, read at `tool_call` for the skill-read bypass, `index.ts:2232`).
    active_skill_entries: Mutex<Vec<SkillPromptEntry>>,
    /// Skill names the human explicitly invoked via a `/skill:<name>` slash command (pi
    /// `explicitlyRequestedSkillNames`, `index.ts:1559` / `index.ts:2192-2206`): a direct user action,
    /// so its skill-file reads bypass the skill-read ask/deny even under a hiding agent (`index.ts:2243`).
    explicitly_requested_skill_names: Mutex<HashSet<String>>,
}

impl PermissionSystemExtension {
    /// The bare constructor (test / non-forwarding): derive every policy/store path from `agent_dir` +
    /// the session `cwd`, and fail-close asks through [`NoOpAskChannel`] (the live in-session dialog
    /// still activates via `ctx.has_ui` + a captured backend). Installs NO forwarding watcher — the
    /// wired PARENT uses [`Self::new_forwarding_parent`].
    #[must_use]
    pub fn new(agent_dir: PathBuf, cwd: PathBuf) -> Self {
        let (paths, permanent_path, config) = Self::derive_parts(&agent_dir, cwd);
        Self::from_parts_full(
            paths,
            permanent_path,
            config,
            Arc::new(NoOpAskChannel),
            agent_dir,
            false,
            Arc::new(OnceLock::new()),
        )
    }

    /// The wired PARENT (root, `CYRUP_SUBAGENT_DEPTH == 0`) constructor: like [`Self::new`] but marks
    /// `install_watcher` so `on_event(SessionStart)` spawns the [`forwarding::spawn_forwarding_watcher`]
    /// task that services subagent children's forwarded asks.
    #[must_use]
    pub fn new_forwarding_parent(agent_dir: PathBuf, cwd: PathBuf) -> Self {
        let (paths, permanent_path, config) = Self::derive_parts(&agent_dir, cwd);
        Self::from_parts_full(
            paths,
            permanent_path,
            config,
            Arc::new(NoOpAskChannel),
            agent_dir,
            true,
            Arc::new(OnceLock::new()),
        )
    }

    /// The wired CHILD (`CYRUP_SUBAGENT_CHILD`, `DEPTH > 0`) constructor: installs the
    /// [`ForwardingAskChannel`] as the gate's `ask_channel`, so an ask-tier decision forwards UP to the
    /// parent's human (pi `confirmPermission` subagent branch) instead of fail-closing. The channel
    /// shares the extension's `host_services` slot (for the requester session-id metadata) and its wait
    /// bound is [`forwarding::resolve_child_wait_timeout`] (pi's 10-min `PERMISSION_FORWARDING_TIMEOUT`,
    /// ops-overridable). No watcher (a child is a responder to no one).
    #[must_use]
    pub fn new_forwarding_child(agent_dir: PathBuf, cwd: PathBuf) -> Self {
        let (paths, permanent_path, config) = Self::derive_parts(&agent_dir, cwd);
        let host_services: Arc<OnceLock<Arc<dyn HostServices>>> = Arc::new(OnceLock::new());
        let channel: Arc<dyn AskChannel> = Arc::new(ForwardingAskChannel::new(
            agent_dir.clone(),
            forwarding::resolve_child_wait_timeout(),
            host_services.clone(),
        ));
        Self::from_parts_full(paths, permanent_path, config, channel, agent_dir, false, host_services)
    }

    /// Derive the [`ManagerPaths`] + permanent-store path + extension config from `agent_dir` + `cwd`
    /// (shared by every constructor).
    fn derive_parts(agent_dir: &Path, cwd: PathBuf) -> (ManagerPaths, PathBuf, ExtensionConfig) {
        let project_dir = PROJECT_AGENT_SUBDIR.iter().fold(cwd, |acc, seg| acc.join(seg));
        let paths = ManagerPaths {
            global_config_path: agent_dir.join(POLICY_FILE),
            agents_dir: agent_dir.join("agents"),
            project_global_config_path: Some(project_dir.join(POLICY_FILE)),
            project_agents_dir: Some(project_dir.join("agents")),
            legacy_global_settings_path: agent_dir.join("settings.json"),
            global_mcp_config_path: agent_dir.join("mcp.json"),
            mcp_server_names_override: None,
        };
        let permanent_path = agent_dir.join(PERMANENT_APPROVALS_FILE);
        let config = ExtensionConfig::load(&agent_dir.join(CONFIG_DIR).join(CONFIG_FILE));
        (paths, permanent_path, config)
    }

    /// Assemble from explicit parts (used by tests that point the global policy path at a fixture file
    /// / inject a scripted ask channel). Derives `agent_dir` from the policy path's parent; installs no
    /// watcher and a fresh capability slot.
    #[must_use]
    pub fn from_parts(
        paths: ManagerPaths,
        permanent_path: PathBuf,
        config: ExtensionConfig,
        ask_channel: Arc<dyn AskChannel>,
    ) -> Self {
        let agent_dir = paths
            .global_config_path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Self::from_parts_full(
            paths,
            permanent_path,
            config,
            ask_channel,
            agent_dir,
            false,
            Arc::new(OnceLock::new()),
        )
    }

    /// The one true assembler every constructor funnels through.
    #[must_use]
    fn from_parts_full(
        paths: ManagerPaths,
        permanent_path: PathBuf,
        config: ExtensionConfig,
        ask_channel: Arc<dyn AskChannel>,
        agent_dir: PathBuf,
        install_watcher: bool,
        host_services: Arc<OnceLock<Arc<dyn HostServices>>>,
    ) -> Self {
        Self {
            id: ExtensionId::from(EXTENSION_ID),
            manager: Mutex::new(PermissionManager::new(paths)),
            session_approvals: Mutex::new(SessionApprovalStore::new()),
            permanent_approvals: Mutex::new(PermanentApprovalStore::new(permanent_path)),
            dedup: Mutex::new(DedupCache::new()),
            config,
            ask_channel,
            host_services,
            agent_dir,
            install_watcher,
            watcher: Mutex::new(None),
            agent_name: resolve_agent_name_from_env(),
            active_skill_entries: Mutex::new(Vec::new()),
            explicitly_requested_skill_names: Mutex::new(HashSet::new()),
        }
    }

    /// Override the resolved persona name (deterministic tests / an embedder that resolves the name
    /// itself). Production leaves the env-sourced value from [`resolve_agent_name_from_env`] in place.
    /// Trims; empty → `None` (pi `normalizeAgentName`, `index.ts:277-284`).
    #[must_use]
    pub fn with_agent_name(mut self, agent_name: Option<String>) -> Self {
        self.agent_name = agent_name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        self
    }

    /// The gate (pi `index.ts:2208-2499`, the deciding subset): resolve `tool_name` + `input`, fold
    /// the approval stores, then `Block` on deny / fail-closed ask, or proceed on allow. Returns the
    /// `HookOutcome` the dispatcher maps to `BeforeOutcome`.
    async fn decide(&self, call_id: &str, tool_name: &str, input: &Value, ctx: &HostCtx) -> HookOutcome {
        let normalized = tool_name.trim();
        if normalized.is_empty() {
            return HookOutcome::Block { reason: Some(gate::format_missing_tool_name_reason()) };
        }
        let agent_name = self.agent_name.as_deref();

        // (2) REGISTRY / unknown-tool gate (pi `index.ts:2218-2228`): a tool not in the full registry
        // (`getAllTools`) is blocked BEFORE any permission check. Enforced only when the live backend
        // can enumerate the registry (`all_tool_names`); with no live backend it returns `None` and the
        // gate is skipped — the gate cannot false-block when it cannot see the tool set (pi always has
        // `getAllTools`; the default host does not).
        if let Some(registered) = self.registered_tool_names()
            && let Some(reason) = gate::check_requested_tool_registration(normalized, &registered)
        {
            return HookOutcome::Block { reason: Some(reason) };
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
        if normalized == "read"
            && let Some(outcome) = self.resolve_skill_read(input, agent_name, &cwd, ctx).await
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
            && let Some(outcome) =
                self.resolve_external_directory(normalized, &path, &cwd, agent_name, ctx).await
        {
            return outcome;
        }

        // Main check + store overlay — fully synchronous; every lock is dropped before any await.
        let check = {
            let session_rules = guard(&self.session_approvals).get_rules();
            let permanent_rules = guard(&self.permanent_approvals).get_rules();
            let raw = guard(&self.manager).check_permission(normalized, input, agent_name);
            gate::apply_pattern_approval_state(raw, input, &session_rules, &permanent_rules)
        };

        match check.state {
            PermissionState::Deny => {
                HookOutcome::Block { reason: Some(gate::format_deny_reason(&check, agent_name)) }
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
    async fn resolve_skill_read(
        &self,
        input: &Value,
        agent_name: Option<&str>,
        cwd: &str,
        ctx: &HostCtx,
    ) -> Option<HookOutcome> {
        let read_path =
            to_record(input).get("path").and_then(Value::as_str).unwrap_or("").to_string();
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
                    .check_permission("skill", &json!({ "name": inferred.name.clone() }), agent_name)
                    .state;
                inferred
            }
        };

        let explicitly_requested =
            guard(&self.explicitly_requested_skill_names).contains(&read_skill.name);
        if !explicitly_requested {
            match read_skill.state {
                PermissionState::Deny => {
                    return Some(HookOutcome::Block {
                        reason: Some(skill::format_skill_path_deny_reason(&read_skill, agent_name)),
                    });
                }
                PermissionState::Ask => {
                    let message =
                        skill::format_skill_path_ask_prompt(&read_skill, &read_path, agent_name);
                    match self.prompt_decision(&message, ctx).await {
                        AskOutcome::NoLiveChannel => {
                            return Some(HookOutcome::Block {
                                reason: Some(skill::skill_ask_unavailable_reason()),
                            });
                        }
                        AskOutcome::Decided(d) if !d.approved => {
                            return Some(HookOutcome::Block {
                                reason: Some(skill::format_skill_user_denied_reason(
                                    d.denial_reason.as_deref(),
                                )),
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
    /// special policy for `{path, cwd}` (with the session/permanent overlay applied on an `ask`): `deny`
    /// → block; `ask` → live prompt (fail-closed / user-deny → block; approved-Always → session-persist,
    /// then fall through); `allow` → fall through. `None` = allowed (proceed to the main check).
    async fn resolve_external_directory(
        &self,
        tool_name: &str,
        path: &str,
        cwd: &str,
        agent_name: Option<&str>,
        ctx: &HostCtx,
    ) -> Option<HookOutcome> {
        let ext_input = json!({ "path": path, "cwd": cwd });
        let raw = guard(&self.manager).check_permission("external_directory", &ext_input, agent_name);
        // pi `:2319-2321`: the session/permanent overlay is applied ONLY on an `ask` result.
        let ext_check = if raw.state == PermissionState::Ask {
            let session_rules = guard(&self.session_approvals).get_rules();
            let permanent_rules = guard(&self.permanent_approvals).get_rules();
            gate::apply_pattern_approval_state(raw, &ext_input, &session_rules, &permanent_rules)
        } else {
            raw
        };

        match ext_check.state {
            PermissionState::Deny => Some(HookOutcome::Block {
                reason: Some(gate::format_external_directory_deny_reason(
                    tool_name, path, cwd, agent_name,
                )),
            }),
            PermissionState::Ask => {
                let message =
                    gate::format_external_directory_ask_prompt(tool_name, path, cwd, agent_name);
                match self.prompt_decision(&message, ctx).await {
                    AskOutcome::NoLiveChannel => Some(HookOutcome::Block {
                        reason: Some(gate::format_external_directory_unavailable_reason(path)),
                    }),
                    AskOutcome::Decided(d) if !d.approved => Some(HookOutcome::Block {
                        reason: Some(gate::format_external_directory_user_denied_reason(
                            tool_name,
                            path,
                            d.denial_reason.as_deref(),
                        )),
                    }),
                    AskOutcome::Decided(d) => {
                        // pi `persistPatternApprovalDecision` (`:2391`): an approved-Always persists an
                        // allow rule to the SESSION store, then the call FALLS THROUGH to the main check.
                        if d.state == PermissionDecisionState::Always {
                            let subject = gate::get_pattern_approval_subject(&ext_check, &ext_input);
                            if !subject.is_empty() {
                                guard(&self.session_approvals)
                                    .approve_always(&ext_check.tool_name, &subject);
                            }
                        }
                        None
                    }
                }
            }
            PermissionState::Allow => None,
        }
    }

    /// Prompt the human for a bespoke `message` (the skill-read + external-dir asks, which manage their
    /// own persistence — no dedup/`Always` tail): yolo auto-approve (pi `shouldAutoApprovePermissionState`),
    /// then the C3 human-interaction lock, the live-vs-fallback channel selection, and the P-3 dispatch-
    /// budget-forgiveness guard held across the BLOCKING dialog — the SAME machinery [`Self::resolve_ask`]
    /// uses. `AskOutcome::NoLiveChannel` = fail-CLOSED (no reachable human).
    async fn prompt_decision(&self, message: &str, ctx: &HostCtx) -> AskOutcome {
        if self.config.yolo_mode {
            return AskOutcome::Decided(PermissionPromptDecision {
                approved: true,
                state: PermissionDecisionState::Approved,
                denial_reason: None,
            });
        }
        let human_lock = self.host_services.get().and_then(|s| s.human_interaction_lock());
        let _human_guard = match human_lock {
            Some(lock) => Some(lock.acquire().await),
            None => None,
        };
        let channel: Arc<dyn AskChannel> = match (ctx.has_ui, self.host_services.get()) {
            (true, Some(services)) => Arc::new(LocalAskChannel::new(services.clone())),
            _ => self.ask_channel.clone(),
        };
        let _human_wait = ctx.begin_human_wait();
        channel.confirm("Permission Required", message, PromptOpts::default()).await
    }

    /// The main-check `ask` branch (pi `:2444-2496` + `promptPermission :1794-1902` + `confirmPermission
    /// :1506-1513`): dedup → the shared [`Self::prompt_decision`] core (yolo → C3 human-interaction lock
    /// → live dialog under a P-3 budget-forgiveness guard) → fail-CLOSED when no human is reachable →
    /// remember + apply (the `Always` session-persist tail). The prompt subject now names the resolved
    /// persona (real `agent_name`, pi `formatAskPrompt(check, agentName, input)`).
    async fn resolve_ask(
        &self,
        call_id: &str,
        input: &Value,
        check: &PermissionCheckResult,
        ctx: &HostCtx,
    ) -> HookOutcome {
        let agent_name = self.agent_name.as_deref();

        let details = dedup_details(call_id, input, check, agent_name);
        let key = details.cache_key();

        // Dedup hit: reuse the prior decision (collapsed to Allow-Once) — zero additional prompts.
        if let Some(k) = &key {
            let cached = guard(&self.dedup).get(k);
            if let Some(decision) = cached {
                return self.apply_decision(decision, check, input);
            }
        }

        // pi `formatAskPrompt` (`index.ts:570-590`) — the human-facing prompt (NOT the headless reason).
        // The shared prompting core applies yolo auto-approve (pi `shouldAutoApprovePermissionState`),
        // the C3 human lock, the live-vs-fallback channel, and the P-3 dispatch-budget guard.
        let message = gate::format_ask_prompt(check, agent_name, input);
        let decision = match self.prompt_decision(&message, ctx).await {
            AskOutcome::Decided(d) => d,
            // Fail-CLOSED: no reachable human (headless / no live UI) → Block, never proceed
            // (pi `confirmPermission` headless `{approved:false}` :1509-1513 / `:2452-2467`).
            AskOutcome::NoLiveChannel => {
                return HookOutcome::Block { reason: Some(gate::format_ask_unavailable_reason(check)) };
            }
        };

        if let Some(k) = &key {
            guard(&self.dedup).remember(k, decision.clone());
        }
        self.apply_decision(decision, check, input)
    }

    /// Apply a resolved decision (pi `:2478-2495`): not-approved → Block (`formatUserDeniedReason`);
    /// approved-Always → persist an allow rule to the SESSION store (pi `index.ts:905`; §8.2: the
    /// permanent on-disk store is NEVER written at runtime — it is read-through only). The `Always`
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

    /// The `before_agent_start` context-hygiene shaping (pi `index.ts:2134-2190`, port doc §9). Runs
    /// three faithful steps and RETURNS the sanitized system prompt as a `[mutate]` (the system-prompt
    /// MUTATE seam, `contract.rs` `EventPatch::SystemPromptAndInject` → `session.rs`
    /// `assemble_run_messages`):
    /// 1. **Active-tools exposure** (pi `setActiveTools`, `:2155`): for every registered tool
    ///    ([`HostServices::all_tool_names`], pi `getAllTools`), keep it iff [`Self::should_expose_tool`];
    ///    restrict the live agent's tool set via [`HostServices::set_active_tools`] (staged as
    ///    `pending_active_tools`, drained + applied IN-TURN by `AgentSession::assemble_run_messages`, so
    ///    it shapes turn 1 ordered BEFORE the sanitized prompt). Skipped when no live backend can
    ///    enumerate the registry (pi always has `getAllTools`; the default host does not).
    /// 2. **`sanitizeAvailableToolsSection`** ([`crate::sanitize::tools`], `:2174`) over the exposed set.
    /// 3. **`resolveSkillPromptEntries`** ([`crate::sanitize::skills`], `:2175`) — hides `ask`/`deny`
    ///    skills from `<available_skills>` while CACHING the enforcement entries the skill-read gate
    ///    reads at every `tool_call`. ONE parse, both consumers.
    ///
    /// Also syncs the `"yolo"` status pill (pi `syncPermissionSystemStatus`, `:2136`).
    fn on_before_agent_start(&self, system_prompt: &str, ctx: &HostCtx) -> HookOutcome {
        let cwd = ctx.cwd.to_string_lossy().into_owned();
        let agent = self.agent_name.as_deref();
        let services = self.host_services.get();

        // (1) Active-tools exposure — only when the live backend can enumerate the FULL registry.
        let working_prompt = match services.and_then(|s| s.all_tool_names()) {
            Some(tools) => {
                let allowed: Vec<String> = tools
                    .into_iter()
                    .filter(|name| self.should_expose_tool(name, agent))
                    .collect();
                if let Some(s) = services {
                    s.set_active_tools(&allowed);
                }
                // (2) Strip the "Available tools:" section + denied-tool guideline bullets.
                sanitize::tools::sanitize_available_tools_section(system_prompt, &allowed).prompt
            }
            // No live registry (default host / headless): cannot compute the exposed set → leave the
            // tools section intact (pi always has `getAllTools`); still resolve/hide skills below.
            None => system_prompt.to_string(),
        };

        // (3) Hide ask/deny skills from `<available_skills>` + cache the enforcement entries. ONE
        // parse feeds both the enforcement cache (read at every `tool_call`) and the hidden prompt.
        let resolution = {
            let mut mgr = guard(&self.manager);
            sanitize::skills::resolve_skill_prompt_entries(&working_prompt, &mut mgr, agent, &cwd)
        };
        *guard(&self.active_skill_entries) = resolution.entries;

        // pi `syncPermissionSystemStatus` (`:2136`): reflect yolo on the live status bar.
        if let Some(s) = services {
            status::sync_status(s, &self.config);
        }

        // Return the sanitized prompt as a [mutate] ONLY when it differs from the original (pi
        // `skillPromptResult.prompt !== event.systemPrompt ? { systemPrompt } : {}`, `:2185-2189`).
        if resolution.prompt == system_prompt {
            HookOutcome::Noop
        } else {
            HookOutcome::Mutate(EventPatch::SystemPromptAndInject {
                system: Some(resolution.prompt),
                inject: None,
            })
        }
    }

    /// pi `shouldExposeTool` (`index.ts:2049-2075`): keep a tool exposed iff its TOOL-LEVEL permission
    /// ([`PermissionManager::get_tool_permission`]) — with the session/permanent approval overlay (pi
    /// `applyPatternApprovalState(..., {}, ...)`) — is not `deny`. A `deny` `read` is still exposed when
    /// the agent has allowed skills ([`PermissionManager::has_allowed_skills`], pi `:2070`) so it can
    /// reach skill files; a `deny` `bash` is still exposed when the agent has an explicitly allowed bash
    /// command ([`PermissionManager::get_bash_permissions`]) so the agent keeps its permitted commands
    /// (the gate re-checks each — the mandate-directed analog of the read/skills bypass).
    fn should_expose_tool(&self, tool_name: &str, agent_name: Option<&str>) -> bool {
        let session_rules = guard(&self.session_approvals).get_rules();
        let permanent_rules = guard(&self.permanent_approvals).get_rules();
        let mut mgr = guard(&self.manager);

        let raw = PermissionCheckResult {
            tool_name: tool_name.to_string(),
            state: mgr.get_tool_permission(tool_name, agent_name),
            matched_pattern: None,
            command: None,
            target: None,
            source: CheckSource::Tool,
        };
        let state =
            gate::apply_pattern_approval_state(raw, &json!({}), &session_rules, &permanent_rules)
                .state;
        if state != PermissionState::Deny {
            return true;
        }
        if tool_name == "read" && mgr.has_allowed_skills(agent_name) {
            return true;
        }
        if tool_name == "bash" && mgr.get_bash_permissions(agent_name).any_allow() {
            return true;
        }
        false
    }

    /// pi `startForwardedPermissionPolling` (`index.ts:1904-2031`): in the PARENT role (`install_watcher`),
    /// on a session WITH a UI and a captured live backend, spawn the forwarding watcher ONCE. No-op for a
    /// child, a headless session (pi's `!ctx.hasUI` early return, `:1361`/`:1912`), a missing backend, or
    /// when a watcher is already running (a session rebuild must not double-spawn).
    fn maybe_start_forwarding_watcher(&self, ctx: &HostCtx) {
        if !self.install_watcher || !ctx.has_ui {
            return;
        }
        let Some(services) = self.host_services.get() else {
            return;
        };
        let mut slot = guard(&self.watcher);
        // Re-entrancy guard: keep a live watcher; only replace a finished one.
        if slot.as_ref().is_some_and(|h| !h.is_finished()) {
            return;
        }
        *slot = Some(forwarding::spawn_forwarding_watcher(
            self.agent_dir.clone(),
            services.clone(),
            self.config.clone(),
        ));
    }

    /// pi watcher teardown (`index.ts:2131`): abort the forwarding watcher task on session shutdown.
    fn stop_forwarding_watcher(&self) {
        if let Some(handle) = guard(&self.watcher).take() {
            handle.abort();
        }
    }
}

/// Build the [`DedupDetails`] fingerprint inputs (pi `PermissionPromptDetails`, `index.ts:713-726`).
/// `message` is the live prompt (pi `details.message = formatAskPrompt(...)`), so a re-emitted
/// identical `tool_call` fingerprints the same and reuses the cached decision.
fn dedup_details(
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

fn source_str(s: CheckSource) -> &'static str {
    match s {
        CheckSource::Tool => "tool",
        CheckSource::Bash => "bash",
        CheckSource::Mcp => "mcp",
        CheckSource::Skill => "skill",
        CheckSource::Special => "special",
        CheckSource::Default => "default",
    }
}

/// Lock a `Mutex`, recovering from poison rather than panicking (no-panic policy). Held only across
/// synchronous sections — never across an `.await`.
fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// pi `resolveAgentName` (`index.ts:2033-2047`) for cyrup's process-per-subagent model: the resolved
/// persona name this process was spawned as, read from the `CYRUP_SUBAGENT_AGENT_NAME` env var
/// (`cyrup_ext_subagents::AGENT_NAME_ENV_VAR`) — captured ONCE (the child IS its persona for its whole
/// lifetime), the exact equivalent of pi's in-process `active_agent` session entry / `<active_agent>`
/// prompt tag for a separate-process subagent. Trimmed; empty/absent → `None` (pi `normalizeAgentName`
/// null-normalization + the normalized-`""` top-level: a top-level process has no such var, so the
/// agent + projectAgent layers no-op and global + project still enforce). This is the SAME
/// `std::env::var` pattern the crate already uses for the sibling `CYRUP_SUBAGENT_*` anchors
/// (`ask.rs` `PARENT_SESSION_ENV_VAR`, `is_subagent_child`).
fn resolve_agent_name_from_env() -> Option<String> {
    std::env::var(cyrup_ext_subagents::AGENT_NAME_ENV_VAR)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[async_trait]
impl NativeExtension for PermissionSystemExtension {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    /// P-1 (reconciliation §2 item 1): capture the late-bound live capability backend BEFORE `init`
    /// (the builder threads its `LiveHostServices` via `load_native_with_services`). The in-session
    /// `ask` dialog (`resolve_ask`) prompts through it via [`LocalAskChannel`]. Set-once; a second
    /// bind is ignored (the session's backend is stable).
    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        let _ = self.host_services.set(services);
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        // ToolCall is the deciding gate (honored by the block/mutate dispatcher `ExtHooks` drives).
        // BeforeAgentStart runs the context-hygiene shaping (pi `index.ts:2134-2190`): it shapes the
        // active tool set (`setActiveTools`), sanitizes the system prompt (tools section + denied
        // guideline bullets, and hides ask/deny skills while caching the skill-read enforcement entries,
        // pi `:2174-2176`), and syncs the yolo pill — returning the sanitized prompt as a `[mutate]`.
        // Input captures `/skill:<name>` explicit requests (pi `:2192-2206`). Session{Start,Shutdown}
        // clear the in-session store + dedup + skill state (and set/clear the status pill). No
        // LLM-visible tool/command is registered — the gate is invisible to the model (pi none).
        api.subscribe(&[
            EventKind::ToolCall,
            EventKind::BeforeAgentStart,
            EventKind::Input,
            EventKind::SessionStart,
            EventKind::SessionShutdown,
        ]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::ToolCall { call_id, name, input } => {
                self.decide(call_id.as_str(), name, input, ctx).await
            }
            HostEvent::BeforeAgentStart { system_prompt, .. } => {
                // pi `before_agent_start` (`index.ts:2134-2190`): shape the active tool set
                // (`setActiveTools`), sanitize the system prompt (tools section + denied guideline
                // bullets, and hide ask/deny skills while caching the enforcement entries the skill-read
                // gate reads at every `tool_call`), and sync the yolo status pill — returning the
                // sanitized prompt as a `[mutate]`.
                self.on_before_agent_start(system_prompt, ctx)
            }
            HostEvent::Input { text, .. } => {
                // pi `index.ts:2192-2206`: a `/skill:<name>` slash command is a direct user action —
                // remember it so its skill-file reads bypass the skill-read ask/deny (pi `:2243`).
                if let Some(name) = skill::extract_skill_name_from_input(text) {
                    guard(&self.explicitly_requested_skill_names).insert(name);
                }
                HookOutcome::Noop
            }
            HostEvent::SessionStart { .. } => {
                // pi `index.ts:2089,2092`: clear session store + dedup + explicit-skill set; refresh.
                guard(&self.session_approvals).clear();
                guard(&self.dedup).clear();
                guard(&self.explicitly_requested_skill_names).clear();
                guard(&self.active_skill_entries).clear();
                // pi `startForwardedPermissionPolling` (`index.ts:1904-2031`): in the PARENT role, on a
                // session WITH a UI, spawn the forwarding watcher (a detached tokio task, OUTSIDE the 5s
                // dispatch budget) that services subagent children's forwarded asks.
                self.maybe_start_forwarding_watcher(ctx);
                // pi `syncPermissionSystemStatus` (`index.ts:2091` via `refreshSessionRuntimeState`):
                // reflect the yolo pill on the live status bar at session start.
                if let Some(s) = self.host_services.get() {
                    status::sync_status(s, &self.config);
                }
                HookOutcome::Noop
            }
            HostEvent::SessionShutdown { .. } => {
                // pi `index.ts:2122,2123,2128,2130,2131`: clear the status pill + stores + dedup + skill
                // state; tear down watcher.
                if let Some(s) = self.host_services.get() {
                    status::clear_status(s);
                }
                guard(&self.session_approvals).clear();
                guard(&self.dedup).clear();
                guard(&self.explicitly_requested_skill_names).clear();
                guard(&self.active_skill_entries).clear();
                self.stop_forwarding_watcher();
                HookOutcome::Noop
            }
            _ => HookOutcome::Noop,
        }
    }
}

// ================================================================================= binary wiring

fn is_subagent_child() -> bool {
    std::env::var(CHILD_ENV_VAR).ok().as_deref() == Some("1")
}

fn env_truthy(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref().map(str::trim),
        Some("1") | Some("true") | Some("on") | Some("yes")
    )
}

/// DI-5 "installed" detection (opt-in): the gate attaches only when the user has installed it —
/// either the [`INSTALL_ENV_VAR`] is truthy, or a policy/config file exists. NOT installed → zero
/// gating (unchanged core behavior); installed → default-ASK per category (faithful to pi
/// `permission-manager.ts:44-50`). This keeps the crate compiled + wired at all three sites while
/// never bricking the default (policy-less) app with fail-closed asks on every tool.
#[must_use]
pub fn is_installed(agent_dir: &Path, cwd: &Path) -> bool {
    if env_truthy(INSTALL_ENV_VAR) {
        return true;
    }
    let project_dir = PROJECT_AGENT_SUBDIR.iter().fold(cwd.to_path_buf(), |acc, seg| acc.join(seg));
    [
        agent_dir.join(POLICY_FILE),
        project_dir.join(POLICY_FILE),
        agent_dir.join(CONFIG_DIR).join(CONFIG_FILE),
    ]
    .iter()
    .any(|p| p.exists())
}

/// The binary-side wiring entry point `crates/cyrup/src/main.rs` calls at each of its three
/// session-build sites (mirrors `cyrup_ext_subagents::extension::subagent_extension_for_env`).
///
/// Role is selected by the `CYRUP_SUBAGENT_CHILD` / depth signal (port doc §3.1 item 4, pi's `hasUI`
/// vs `isSubagentExecutionContext` split, `index.ts:1506-1519`):
/// - **CHILD** (`CYRUP_SUBAGENT_CHILD`): loads the gate with a [`ForwardingAskChannel`]
///   ([`PermissionSystemExtension::new_forwarding_child`]) — an ask-tier decision FORWARDS up to the
///   parent's human via the spool instead of dying. (P-4; previously this returned `None`, leaving a
///   child's `ask` with no reachable human — the exact gap this build closes.)
/// - **PARENT** (root, `DEPTH == 0`): loads the gate with the [`LocalAskChannel`] in-session dialog +
///   the forwarding WATCHER ([`PermissionSystemExtension::new_forwarding_parent`]).
///
/// Returns `None` (attach nothing → DI-5 zero gating) only when the gate is not installed.
#[must_use]
pub fn permission_extension_for_env(
    agent_dir: PathBuf,
    cwd: PathBuf,
) -> Option<Arc<dyn NativeExtension>> {
    if !is_installed(&agent_dir, &cwd) {
        return None;
    }
    if is_subagent_child() {
        // CHILD: forward asks up to the parent (§7.4). The parent-session anchor
        // `CYRUP_SUBAGENT_PARENT_SESSION` (emitted by `cyrup-ext-subagents`, `exec/mod.rs`
        // `PARENT_SESSION_ENV_VAR`) addresses the parent's inbox; the `ForwardingAskChannel` reads it.
        return Some(Arc::new(PermissionSystemExtension::new_forwarding_child(agent_dir, cwd)));
    }
    // PARENT: in-session dialog + the forwarding watcher (installed on SessionStart).
    Some(Arc::new(PermissionSystemExtension::new_forwarding_parent(agent_dir, cwd)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn not_installed_without_policy_or_env_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        // No policy file, env not set → DI-5 zero gating.
        assert!(!is_installed(dir.path(), dir.path()));
    }

    #[test]
    fn installed_when_policy_file_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(POLICY_FILE), "{}").unwrap();
        assert!(is_installed(dir.path(), dir.path()));
    }
}
