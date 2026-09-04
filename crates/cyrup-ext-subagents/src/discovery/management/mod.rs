//! Agent/chain management CRUD: create/update/delete/rename (func-SA §5.1 R-SA-013/014/019;
//! arch-SA §2.2, §9 coverage row for these three requirements).
//!
//! This module owns exactly three concerns, none of which overlap with `merge.rs` (four-tier
//! precedence merge, not yet written as of this file) or `frontmatter.rs` (parse-only, already
//! written and reused here read-only for round-trip verification):
//!
//! 1. **Read-only-source rejection (R-SA-014).** A create/update/delete/rename targeting a
//!    `Builtin`- or `Package`-sourced agent or chain MUST fail with
//!    [`crate::error::SubagentError::ReadOnlySource`] before any filesystem mutation is
//!    attempted. Only `User`/`Project`-sourced files are writable through this module.
//! 2. **Call-site-dependent `disabled` visibility (R-SA-013).** Three independently testable
//!    views over "the same" underlying agent/chain set:
//!    - [`AgentVisibility::management`] — full, unfiltered (used for CRUD and re-enabling); MUST
//!      include disabled agents.
//!    - [`AgentVisibility::delegation`] — runtime-filtered; MUST exclude disabled agents, since
//!      this is the view actual execution-time selection uses.
//!    - [`AgentVisibility::list`] — filtered independently of the other two (a human-facing list
//!      view defaults to hiding disabled agents but is a *distinct* code path from delegation's
//!      filter, not a shared implementation detail masquerading as two call sites — see that
//!      function's own doc for why it is kept textually separate from `delegation` even though
//!      both currently apply the same predicate).
//! 3. **On-demand, re-scanned-per-call semantics (R-SA-019).** This module holds no cache and no
//!    filesystem watcher; every function here operates on a caller-supplied `&[AgentDefinition]`/
//!    `&[ChainDefinition]` snapshot (the caller — `discovery/mod.rs`'s entry points, once written
//!    — is responsible for re-invoking discovery before each mutating call in a create -> get ->
//!    update -> delete sequence, per R-SA-019's own text: *"Callers that need up-to-date state
//!    across a sequence of management actions... MUST re-invoke discovery before each mutating
//!    action rather than reusing a cached result."* This module does not and cannot violate that
//!    on its own — it simply never introduces a cache to violate it with.
//!
//! # Deferred to later phases (explicitly, per this task's own instructions)
//!
//! - **`merge.rs`** (four-tier Builtin/Package/User/Project precedence merge, R-SA-001/002) is a
//!   sibling file owned by a later/concurrent phase. This module does not merge scopes; it
//!   operates on a flat, already-scoped `&[AgentDefinition]` slice the caller assembled (from
//!   discovery or from a targeted single-scope directory scan) and only needs each entry's
//!   `source`/`name`/`file_path` fields, which `AgentDefinition` already provides regardless of
//!   whether `merge.rs` exists yet.
//! - **`discovery/mod.rs`'s `discover_agents_all`/`discover_agents` entry points** (R-SA-001..004
//!   directory-walk orchestration) are likewise a later phase. This module's CRUD functions take
//!   an explicit `scope_dir: &Path` parameter for exactly the scope (`User`/`Project`) being
//!   mutated, rather than re-deriving cyrup's config-directory resolution itself — that
//!   resolution is `discovery/mod.rs`'s job, not this file's.
//! - **Chain-file management** reuses [`crate::discovery::chains`]'s already-written
//!   `.chain.json`-over-`.chain.md` same-name precedence (R-SA-015) purely as a read-side
//!   discovery helper for `list`/`get`-style callers; this module's own chain CRUD writes
//!   `.chain.json` exclusively (the plain-`serde_json` format, since it has no reason to prefer
//!   the frontmatter-grammar `.chain.md` format when authoring new content) and never attempts to
//!   mutate an existing `.chain.md` file in place — a caller renaming/updating a `.chain.md`-authored
//!   chain gets a fresh `.chain.json` file at the same logical name, which (per R-SA-015) then
//!   takes over same-directory precedence on the next discovery pass.
//!
//! # Layout
//!
//! Split into leaf modules by concern (visibility/read-only-guard, agent CRUD, frontmatter
//! write-back, chain CRUD, small shared helpers, config parsing, discovery lookups, renderers, the
//! six non-tier-aware handlers, the four SUBA-005 tier-aware handlers). This root keeps the public
//! dispatch surface (`ManagementRequest`/`ManagementOutcome`/the action-name consts/
//! [`handle_management_action`]) plus the C3 end-to-end test suite: nearly every handler test below
//! calls [`handle_management_action`] itself (the real dispatch entry point) rather than an
//! individual `handle_list`/`handle_create`/... function directly, so — mirroring `exec/mod.rs`'s
//! own precedent of keeping `run_sync`'s end-to-end tests at the root — these stay here rather than
//! being distributed into `handlers`/`tier_actions`, which carry no tests of their own.

mod agent_crud;
mod chain_crud;
mod config_parse;
mod frontmatter_write;
mod handlers;
mod helpers;
mod lookup;
mod render;
mod test_support;
mod tier_actions;
mod visibility;

pub use agent_crud::{AgentFields, AgentMutationOutcome};
pub use visibility::{AgentVisibility, ChainVisibility};

use super::AgentDiscoveryConfig;

/// pi's `BUILTIN_AGENT_NAMES` (`agents.ts:38-46` @ v0.43.0) — used by [`handlers::handle_models`] to
/// bound the requested filter and to iterate the builtin model mapping in pi's exact stable order.
///
/// SEVEN names, not eight. Upstream `83b9872` ("fix: remove stale bundled roles") deleted the
/// `planner` and `context-builder` roles outright — their `agents/*.md`, their paired prompt
/// templates, and every special case keyed on their names — and `bff9722` added `advisor`, which
/// `34a018f` then demoted from its own `agents/advisor.md` to an ALIAS on `oracle`
/// (`agents/oracle.md:3` @ v0.43.0 carries `aliases: advisor`). `advisor` therefore stays in this
/// list — the roster is the set of names the model-report surface enumerates, and pi keeps listing
/// it — while shipping NO `advisor.md` of its own; the alias is what resolves it.
pub const BUILTIN_AGENT_NAMES: [&str; 7] = [
    "advisor",
    "delegate",
    "oracle",
    "researcher",
    "reviewer",
    "scout",
    "worker",
];

/// The management-relevant subset of the `subagent` tool's parsed parameters (pi `ManagementParams`,
/// `agent-management.ts:45-51`). Borrowed from the caller's already-parsed `SubagentToolParams` so
/// `extension.rs` owns the JSON deserialization and this module owns only the management semantics.
pub struct ManagementRequest<'a> {
    pub agent: Option<&'a str>,
    pub chain_name: Option<&'a str>,
    pub agent_scope: Option<&'a str>,
    pub config: Option<&'a serde_json::Value>,
    /// The live PARENT session model (`provider/id`, from
    /// [`cyrup_ext::host::HostServices::current_model`] — pi's `ctx.model`), threaded in by the
    /// caller so [`handlers::handle_models`]'s `Current session model` line + `formatModelSource`'s
    /// inherit branch render the REAL inherited model instead of `(unavailable)`. `None` (no live
    /// session backend bound / headless) keeps the genuine no-host degrade. Only the `models`
    /// action reads it; the other handlers ignore it.
    pub current_session_model: Option<&'a str>,
    /// The proactive skill-subagent inputs [`handlers::handle_list`] splices in — pi's
    /// `ctx.config?.proactiveSkillSubagents` plus its `discoverAvailableSkills: () =>
    /// discoverAvailableSkills(ctx.cwd)` closure (`agent-management.ts:765-770` @v0.43.0). `None`
    /// means the caller performed no availability scan, which yields no suggestions — the same
    /// outcome upstream reaches when its `discoverAvailableSkills` throws
    /// (`proactive-skills.ts:182-186` catches to `[]`, and an empty availability list matches no
    /// skill). Only the `list` action reads it.
    pub proactive_skills: Option<ProactiveSkillsInput<'a>>,
}

/// The two proactive skill-subagent inputs `handleList` reads off its `ManagementContext`
/// (`agent-management.ts:765-770` @v0.43.0), carried on [`ManagementRequest`] because cyrup's
/// management layer takes a request rather than a context object.
///
/// **Why the availability list is pre-resolved rather than a closure.** Upstream passes a lazy
/// `discoverAvailableSkills: () => AvailableSkill[]` so that a disabled feature performs no
/// filesystem scan. cyrup's [`crate::discovery::skills::discover_available_skills`] is `async`, so
/// the laziness lives one level up rather than inside the handler: the caller checks
/// [`crate::discovery::skills::resolve_proactive_skill_subagents_config`]'s `enabled` first and
/// only then awaits the scan, filling this field. Both upstream properties survive — no scan when
/// disabled, and no suggestions when the scan found nothing.
pub struct ProactiveSkillsInput<'a> {
    /// pi `ctx.config?.proactiveSkillSubagents`. `None` is pi's `undefined` (defaults-on).
    pub setting: Option<&'a crate::discovery::skills::ProactiveSkillSubagentsSetting>,
    /// The already-resolved result of pi's `discoverAvailableSkills(ctx.cwd)` closure.
    pub available_skills: &'a [crate::discovery::skills::AvailableSkill],
}

/// The rendered outcome of a management action — pi's `result(text, isError)`
/// (`agent-management.ts:43-44`). `is_error` mirrors pi's `AgentToolResult.isError`; the caller maps
/// `is_error == true` to a `ToolError` (cyrup surfaces tool failures as `Err`, R-02-024) while still
/// preserving pi's exact human-facing text verbatim.
pub struct ManagementOutcome {
    pub text: String,
    pub is_error: bool,
}

impl ManagementOutcome {
    fn ok(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: false }
    }
    fn err(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: true }
    }
}

/// The ten management actions [`handle_management_action`] dispatches — pi's `ManagementAction`
/// union (`shared/types.ts`), in pi's own declaration order. Exposed so `extension.rs`'s tool schema
/// and its child-safe mutating-action denylist are derived from ONE list rather than three hand-kept
/// copies that can drift apart.
pub const MANAGEMENT_ACTIONS: [&str; 10] = [
    "list", "get", "models", "create", "update", "delete", "eject", "disable", "enable", "reset",
];

/// pi `MUTATING_MANAGEMENT_ACTIONS` (`runs/foreground/subagent-executor.ts:112`): the management
/// actions a child-safe (fanout) tool registration must refuse. `list`/`get`/`models` are read-only
/// and stay permitted; the other seven all write to the parent's on-disk agent config — the four
/// SUBA-005 additions (`eject` writes an agent file, `disable`/`enable`/`reset` write
/// `settings.json`) are mutations exactly as much as `create`/`update`/`delete` are, so they join
/// the same denylist rather than sneaking through as "just management".
pub const MUTATING_MANAGEMENT_ACTIONS: [&str; 7] =
    ["create", "update", "delete", "eject", "disable", "enable", "reset"];

/// pi's `handleManagementAction` (`agent-management.ts:1242-1256`): dispatch a management `action` to
/// its handler. Discovery is re-run per call inside each handler (R-SA-019), never cached across a
/// create -> get -> update -> delete sequence.
///
/// # Errors
///
/// Propagates a discovery-time [`SubagentError`](crate::error::SubagentError) (R-SA-009's
/// malformed-settings abort) or a genuine filesystem failure from a create/update/delete write.
/// pi's `isError: true` outcomes (not-found, read-only, validation) are
/// `Ok(ManagementOutcome { is_error: true, .. })`, not `Err`.
pub async fn handle_management_action(
    cfg: &AgentDiscoveryConfig,
    action: &str,
    req: &ManagementRequest<'_>,
) -> Result<ManagementOutcome, crate::error::SubagentError> {
    match action {
        "list" => handlers::handle_list(cfg, req),
        "get" => handlers::handle_get(cfg, req),
        "models" => handlers::handle_models(cfg, req),
        "create" => handlers::handle_create(cfg, req),
        "update" => handlers::handle_update(cfg, req),
        "delete" => handlers::handle_delete(cfg, req),
        // SUBA-005 (pi `agent-management.ts:1046-1049`): the tier-aware / settings-writing four.
        "eject" => tier_actions::handle_eject(cfg, req),
        "disable" => tier_actions::handle_disable(cfg, req).await,
        "enable" => tier_actions::handle_enable(cfg, req).await,
        "reset" => tier_actions::handle_reset(cfg, req).await,
        other => Ok(ManagementOutcome::err(format!("Unknown action: {other}"))),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::path::{Path, PathBuf};

    use super::agent_crud::{create_agent, AgentFields};
    use super::*;
    use crate::discovery::types::AgentSource;

    fn mgmt_cfg(tmp: &Path) -> AgentDiscoveryConfig {
        AgentDiscoveryConfig {
            builtin_agents_dir: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources")),
            user_agent_dirs: vec![tmp.join("user/agents")],
            user_chain_dirs: vec![tmp.join("user/chains")],
            project_agent_dirs: vec![tmp.join("project/agents")],
            project_chain_dirs: vec![tmp.join("project/chains")],
            ..AgentDiscoveryConfig::default()
        }
    }

    fn mreq<'a>(
        agent: Option<&'a str>,
        chain: Option<&'a str>,
        scope: Option<&'a str>,
        config: Option<&'a serde_json::Value>,
    ) -> ManagementRequest<'a> {
        ManagementRequest {
            agent,
            chain_name: chain,
            agent_scope: scope,
            config,
            current_session_model: None,
            proactive_skills: None,
        }
    }

    fn write_agent_md(dir: &Path, file: &str, body: &str) {
        std::fs::create_dir_all(dir).expect("mkdir");
        std::fs::write(dir.join(file), body).expect("write agent file");
    }

    // ---- pi `handleList`'s proactive skill-subagent block (`agent-management.ts:765-770,784` @v0.43.0) ----

    /// Two user agents that both name the same skill, so the skill clears the default
    /// `minReferences: 2`. Returns the request-side availability list that makes it recommendable.
    fn seed_two_agents_sharing_a_skill(cfg: &AgentDiscoveryConfig) -> Vec<crate::discovery::skills::AvailableSkill> {
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "auditor-one.md",
            "---\nname: auditor-one\ndescription: First auditor\nskills: audit-trail\n---\nBody.\n",
        );
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "auditor-two.md",
            "---\nname: auditor-two\ndescription: Second auditor\nskills: audit-trail\n---\nBody.\n",
        );
        vec![crate::discovery::skills::AvailableSkill {
            name: "audit-trail".to_string(),
            description: Some("Trace every mutation.".to_string()),
        }]
    }

    /// The block upstream splices at `agent-management.ts:784` must actually appear in `list`
    /// output, positioned AFTER the `Chains:` block and BEFORE `Chain diagnostics:`, with the
    /// blank-line separator upstream's `["", ...proactiveSuggestions]` prepends.
    #[tokio::test]
    async fn list_emits_the_proactive_skill_subagent_block_in_pis_position() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let available = seed_two_agents_sharing_a_skill(&cfg);

        let mut req = mreq(None, None, None, None);
        req.proactive_skills = Some(ProactiveSkillsInput {
            setting: None, // pi's `undefined` — defaults on
            available_skills: &available,
        });
        let out = handle_management_action(&cfg, "list", &req).await.expect("list ok");
        assert!(!out.is_error, "{}", out.text);
        let t = out.text;

        assert!(
            t.contains("Proactive skill subagent suggestions:"),
            "the block upstream splices at `agent-management.ts:784` is missing:\n{t}"
        );
        assert!(
            t.contains("- audit-trail via reviewer (referenced by 2 configured agents/chains; agent:auditor-one, agent:auditor-two) - Trace every mutation."),
            "the recommendation line must match `formatProactiveSkillSubagentRecommendations`:\n{t}"
        );
        assert!(
            t.contains("Guardrails: use these for broad tasks"),
            "the guardrails footer must ship with the block:\n{t}"
        );
        let chains_at = t.find("Chains:").unwrap_or(usize::MAX);
        let block_at = t.find("Proactive skill subagent suggestions:").unwrap_or(usize::MIN);
        assert!(chains_at < block_at, "the block must follow `Chains:`:\n{t}");
        assert!(
            t.contains("\n\nProactive skill subagent suggestions:"),
            "upstream prepends one blank line to the block:\n{t}"
        );
    }

    /// pi reads `ctx.config?.proactiveSkillSubagents`; the literal `false` disables the feature
    /// entirely (`resolveProactiveSkillSubagentsConfig`, `proactive-skills.ts:38-59`). A setting
    /// that stopped being threaded through would silently stop disabling anything.
    #[tokio::test]
    async fn list_honours_an_explicit_proactive_skill_subagents_false() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let available = seed_two_agents_sharing_a_skill(&cfg);

        let disabled = crate::discovery::skills::ProactiveSkillSubagentsSetting::Disabled;
        let mut req = mreq(None, None, None, None);
        req.proactive_skills = Some(ProactiveSkillsInput {
            setting: Some(&disabled),
            available_skills: &available,
        });
        let out = handle_management_action(&cfg, "list", &req).await.expect("list ok");
        assert!(
            !out.text.contains("Proactive skill subagent suggestions:"),
            "an explicit `false` must suppress the block:\n{}",
            out.text
        );
    }

    /// A caller that ran no availability scan (`proactive_skills: None`) emits no block — the same
    /// outcome upstream reaches when its `discoverAvailableSkills` throws
    /// (`proactive-skills.ts:182-186` catches to `[]`, which matches no skill).
    #[tokio::test]
    async fn list_emits_no_proactive_block_without_an_availability_scan() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let _available = seed_two_agents_sharing_a_skill(&cfg);

        let out = handle_management_action(&cfg, "list", &mreq(None, None, None, None)).await.expect("list ok");
        assert!(
            !out.text.contains("Proactive skill subagent suggestions:"),
            "{}",
            out.text
        );
        // ...and an availability scan that found nothing likewise recommends nothing.
        let empty: Vec<crate::discovery::skills::AvailableSkill> = Vec::new();
        let mut req = mreq(None, None, None, None);
        req.proactive_skills = Some(ProactiveSkillsInput {
            setting: None,
            available_skills: &empty,
        });
        let out = handle_management_action(&cfg, "list", &req).await.expect("list ok");
        assert!(
            !out.text.contains("Proactive skill subagent suggestions:"),
            "{}",
            out.text
        );
    }

    /// Upstream's splice is `...(proactiveSuggestions.length ? ["", ...proactiveSuggestions] : [])`
    /// (`agent-management.ts:784` @v0.43.0). The FALSE branch contributes NOTHING — not even the
    /// separator — so a listing with no suggestions must be byte-identical to one that never asked
    /// for them. Pinned here because pi's `lines.join("\n")` layout makes a stray `""` a real
    /// rendering defect: it trails the whole listing with a blank line, or doubles the single blank
    /// line that introduces `Chain diagnostics:`.
    #[tokio::test]
    async fn list_emits_no_separator_when_the_proactive_block_is_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        // Agents that DO share a skill, so only the empty availability list suppresses the block —
        // the recommender really runs and really returns nothing.
        let _available = seed_two_agents_sharing_a_skill(&cfg);
        let empty: Vec<crate::discovery::skills::AvailableSkill> = Vec::new();

        let mut asked = mreq(None, None, None, None);
        asked.proactive_skills = Some(ProactiveSkillsInput {
            setting: None,
            available_skills: &empty,
        });
        let with_scan = handle_management_action(&cfg, "list", &asked).await.expect("list ok");
        let without_scan =
            handle_management_action(&cfg, "list", &mreq(None, None, None, None)).await.expect("list ok");

        assert_eq!(
            with_scan.text, without_scan.text,
            "an empty suggestion list must contribute NO lines at all — upstream's `: []` branch \
             appends neither the block nor its blank-line separator"
        );
        assert!(
            !with_scan.text.ends_with('\n'),
            "a spurious separator would trail the listing with a blank line:\n{:?}",
            with_scan.text
        );

        // ...and with chain diagnostics present, the ONE blank line that introduces them must stay
        // one: an unconditional separator would render `\n\n\nChain diagnostics:`.
        std::fs::create_dir_all(&cfg.user_chain_dirs[0]).expect("mkdir chains");
        std::fs::write(
            cfg.user_chain_dirs[0].join("broken.chain.json"),
            "{ this is not json",
        )
        .expect("write broken chain");
        let with_diags = handle_management_action(&cfg, "list", &asked).await.expect("list ok");
        assert!(
            with_diags.text.contains("\n\nChain diagnostics:"),
            "the diagnostics block keeps its single leading blank line:\n{}",
            with_diags.text
        );
        assert!(
            !with_diags.text.contains("\n\n\n"),
            "an empty proactive block must not double the diagnostics separator:\n{:?}",
            with_diags.text
        );
    }

    /// pi counts a CHAIN's step skills as references exactly like an agent's `skills`
    /// (`proactive-skills.ts:132-140`: every skill `collectStepSkills` gathers for a chain adds one
    /// `chain:<name>` source), and the `sources.size >= config.minReferences` filter
    /// (`proactive-skills.ts:72-90`) then sees agent and chain sources on equal footing. So one agent
    /// plus one chain naming the same skill is enough to clear the default `minReferences: 2`.
    ///
    /// This pins BOTH ends of `handle_list`'s chain wiring: that `chains` reaches the recommender at
    /// all (upstream passes its own post-filter `chains` local, `agent-management.ts:767`), and that
    /// `collect_chain_step_skills` recurses into nested `parallel` steps
    /// (`proactive-skills.ts:77-89`).
    #[tokio::test]
    async fn list_counts_chain_step_skills_toward_min_references() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        // One agent per skill: neither reaches `minReferences: 2` on agents alone.
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "auditor-one.md",
            "---\nname: auditor-one\ndescription: First auditor\nskills: audit-trail\n---\nBody.\n",
        );
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "diver.md",
            "---\nname: diver\ndescription: Deep diver\nskills: deep-dive\n---\nBody.\n",
        );
        // One chain supplies the second reference for each: `audit-trail` on a top-level step and
        // `deep-dive` only inside a nested `parallel` child.
        std::fs::create_dir_all(&cfg.user_chain_dirs[0]).expect("mkdir chains");
        std::fs::write(
            cfg.user_chain_dirs[0].join("audit-run.chain.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "name": "audit-run",
                "description": "Audit then dive",
                "chain": [
                    { "agent": "auditor-one", "task": "audit it", "skills": ["audit-trail"] },
                    { "parallel": [
                        { "agent": "diver", "task": "dive in", "skills": ["deep-dive"] }
                    ] }
                ]
            }))
            .expect("serialize chain"),
        )
        .expect("write chain");

        let available = vec![
            crate::discovery::skills::AvailableSkill {
                name: "audit-trail".to_string(),
                description: Some("Trace every mutation.".to_string()),
            },
            crate::discovery::skills::AvailableSkill {
                name: "deep-dive".to_string(),
                description: None,
            },
        ];
        let mut req = mreq(None, None, None, None);
        req.proactive_skills = Some(ProactiveSkillsInput {
            setting: None, // pi's `undefined` — `minReferences` defaults to 2
            available_skills: &available,
        });
        let out = handle_management_action(&cfg, "list", &req).await.expect("list ok");
        assert!(!out.is_error, "{}", out.text);
        let t = out.text;

        assert!(
            t.contains("- audit-trail via reviewer (referenced by 2 configured agents/chains; agent:auditor-one, chain:audit-run) - Trace every mutation."),
            "a chain step's `skills` must count as a `chain:<name>` source toward `minReferences`:\n{t}"
        );
        assert!(
            t.contains("- deep-dive via reviewer (referenced by 2 configured agents/chains; agent:diver, chain:audit-run)"),
            "`collectStepSkills` recurses into nested `parallel` children, so a skill named only \
             there still contributes its chain source:\n{t}"
        );
    }

    /// The extension-config shape (`config.json`'s `proactiveSkillSubagents`) must reach the
    /// recommender's own setting shape without losing the disable — the bridge is what
    /// `extension.rs::route_management_action` calls.
    ///
    /// Not a direct-call test on any function this module defines — pinned here (rather than in a
    /// leaf module) because it exercises `crate::discovery::skills`/`crate::registration` only.
    #[test]
    fn the_extension_config_bridge_preserves_disable_and_the_tuning_knobs() {
        use crate::discovery::skills::{
            ProactiveSkillSubagentsSetting, resolve_proactive_skill_subagents_config,
        };
        use crate::registration::ProactiveSkillSubagents;

        let off = ProactiveSkillSubagentsSetting::from_extension_config(
            &ProactiveSkillSubagents::Toggle(false),
        );
        assert!(!resolve_proactive_skill_subagents_config(Some(&off)).enabled);

        let on = ProactiveSkillSubagentsSetting::from_extension_config(
            &ProactiveSkillSubagents::Toggle(true),
        );
        assert!(resolve_proactive_skill_subagents_config(Some(&on)).enabled);

        let tuned = ProactiveSkillSubagentsSetting::from_extension_config(
            &ProactiveSkillSubagents::Config(crate::registration::ProactiveSkillSubagentsConfig {
                enabled: Some(true),
                min_references: Some(1),
                max_recommendations: Some(2),
                preferred_agent: Some("scout".to_string()),
            }),
        );
        let resolved = resolve_proactive_skill_subagents_config(Some(&tuned));
        assert_eq!(resolved.min_references, 1);
        assert_eq!(resolved.max_recommendations, 2);
        assert_eq!(resolved.preferred_agent, "scout");
    }

    /// pi's `positiveInteger` (`proactive-skills.ts:32-36` — five lines) returns `undefined` for
    /// anything that is not a finite integer `>= 1`, and `resolveProactiveSkillSubagentsConfig`
    /// (`:50,:53`) then falls through to `DEFAULT_MIN_REFERENCES` / `DEFAULT_MAX_RECOMMENDATIONS`.
    /// A guard that let `0` through would set `minReferences: 0` (every skill named even once gets
    /// recommended) and `maxRecommendations: 0` (`Math.min(0, 5)` → `slice(0, 0)` → the block
    /// silently disappears), so both directions are pinned. The negative cases are reachable only
    /// through this `i64` shape — the `config.json` bridge narrows to `u32` — which is exactly the
    /// signedness the port's guard is written against.
    ///
    /// Same rationale as the test above for living here rather than in a leaf module.
    #[test]
    fn proactive_config_rejects_non_positive_min_and_max_like_pis_positive_integer() {
        use crate::discovery::skills::{
            ProactiveSkillSubagentsConfig, ProactiveSkillSubagentsSetting,
            resolve_proactive_skill_subagents_config,
        };

        for (min, max) in [(Some(0), Some(0)), (Some(-1), Some(-7)), (Some(-100), Some(0))] {
            let setting = ProactiveSkillSubagentsSetting::Config(ProactiveSkillSubagentsConfig {
                enabled: None,
                min_references: min,
                max_recommendations: max,
                preferred_agent: None,
            });
            let resolved = resolve_proactive_skill_subagents_config(Some(&setting));
            assert_eq!(
                resolved.min_references, 2,
                "minReferences={min:?} is not a positive integer, so pi's DEFAULT_MIN_REFERENCES applies"
            );
            assert_eq!(
                resolved.max_recommendations, 3,
                "maxRecommendations={max:?} is not a positive integer, so pi's \
                 DEFAULT_MAX_RECOMMENDATIONS applies"
            );
        }

        // `1` is the boundary pi keeps, and the cap still clamps a large one to MAX_RECOMMENDATION_CAP.
        let boundary = ProactiveSkillSubagentsSetting::Config(ProactiveSkillSubagentsConfig {
            enabled: None,
            min_references: Some(1),
            max_recommendations: Some(1),
            preferred_agent: None,
        });
        let resolved = resolve_proactive_skill_subagents_config(Some(&boundary));
        assert_eq!(resolved.min_references, 1);
        assert_eq!(resolved.max_recommendations, 1);

        let huge = ProactiveSkillSubagentsSetting::Config(ProactiveSkillSubagentsConfig {
            enabled: None,
            min_references: None,
            max_recommendations: Some(99),
            preferred_agent: None,
        });
        assert_eq!(
            resolve_proactive_skill_subagents_config(Some(&huge)).max_recommendations,
            5,
            "`Math.min(maxRecommendations, MAX_RECOMMENDATION_CAP)`, `proactive-skills.ts:54`"
        );
    }

    #[tokio::test]
    async fn list_includes_builtins_and_discovered_with_pi_shape() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        create_agent(&cfg.user_agent_dirs[0], AgentSource::User, "my-user-agent", "A user agent", &AgentFields::default())
            .expect("no error")
            .expect("not skipped");
        create_agent(&cfg.project_agent_dirs[0], AgentSource::Project, "my-project-agent", "A project agent", &AgentFields::default())
            .expect("no error")
            .expect("not skipped");

        let out = handle_management_action(&cfg, "list", &mreq(None, None, None, None)).await.expect("list ok");
        assert!(!out.is_error);
        let t = out.text;
        // pi list header shape (`agent-management.ts:553-560`).
        assert!(t.contains("Executable agents:"), "{t}");
        assert!(t.contains("Chains:"), "{t}");
        // The 8 R-SA-132 builtins load from resources/agents alongside the discovered agents.
        assert!(t.contains("- reviewer (builtin"), "{t}");
        assert!(t.contains("- scout (builtin"), "{t}");
        // Discovered user/project agents render with the exact pi line shape.
        assert!(t.contains("- my-user-agent (user): A user agent"), "{t}");
        assert!(t.contains("- my-project-agent (project): A project agent"), "{t}");
        // No chains authored -> the empty-chains sentinel.
        assert!(t.contains("Chains:\n- (none)"), "{t}");
        // Agents section precedes the chains section.
        let agents_idx = t.find("Executable agents:").expect("has agents header");
        let chains_idx = t.find("Chains:").expect("has chains header");
        assert!(agents_idx < chains_idx);
    }

    #[tokio::test]
    async fn list_scope_filter_narrows_to_project_but_keeps_builtins() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        create_agent(&cfg.user_agent_dirs[0], AgentSource::User, "my-user-agent", "A user agent", &AgentFields::default())
            .expect("ok").expect("not skipped");
        create_agent(&cfg.project_agent_dirs[0], AgentSource::Project, "my-project-agent", "A project agent", &AgentFields::default())
            .expect("ok").expect("not skipped");

        let out = handle_management_action(&cfg, "list", &mreq(None, None, Some("project"), None)).await.expect("list ok");
        let t = out.text;
        assert!(t.contains("- my-project-agent (project)"), "{t}");
        assert!(!t.contains("- my-user-agent (user)"), "project scope must hide user agents: {t}");
        // Builtins remain visible under any named scope (they are orthogonal to the user/project axis).
        assert!(t.contains("- reviewer (builtin"), "{t}");
    }

    #[tokio::test]
    async fn create_get_update_delete_round_trip_user_scope() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());

        let create_cfg = serde_json::json!({
            "name": "Recon Scout",
            "description": "Fast recon",
            "systemPrompt": "Inspect the tree.",
            "tools": "read, grep, ls"
        });
        let created = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&create_cfg))).await.expect("create ok");
        assert!(!created.is_error, "{}", created.text);
        assert!(created.text.starts_with("Created agent 'recon-scout' at "), "{}", created.text);
        let file = cfg.user_agent_dirs[0].join("recon-scout.md");
        assert!(file.exists());

        let got = handle_management_action(&cfg, "get", &mreq(Some("recon-scout"), None, None, None)).await.expect("get ok");
        assert!(!got.is_error, "{}", got.text);
        assert!(got.text.contains("Agent: recon-scout (user)"), "{}", got.text);
        assert!(got.text.contains("Description: Fast recon"), "{}", got.text);
        assert!(got.text.contains("Tools: read, grep, ls"), "{}", got.text);
        assert!(got.text.contains("System prompt mode: replace"), "{}", got.text);
        assert!(got.text.contains("System Prompt:\nInspect the tree."), "{}", got.text);

        let update_cfg = serde_json::json!({ "description": "Faster recon" });
        let updated = handle_management_action(&cfg, "update", &mreq(Some("recon-scout"), None, None, Some(&update_cfg))).await.expect("update ok");
        assert!(!updated.is_error, "{}", updated.text);
        assert!(updated.text.starts_with("Updated agent 'recon-scout' at "), "{}", updated.text);
        let got2 = handle_management_action(&cfg, "get", &mreq(Some("recon-scout"), None, None, None)).await.expect("get ok");
        assert!(got2.text.contains("Description: Faster recon"), "{}", got2.text);
        // The un-touched tools survive the merge-update (field-level patch, not a full replace).
        assert!(got2.text.contains("Tools: read, grep, ls"), "{}", got2.text);

        let deleted = handle_management_action(&cfg, "delete", &mreq(Some("recon-scout"), None, None, None)).await.expect("delete ok");
        assert!(!deleted.is_error, "{}", deleted.text);
        assert!(deleted.text.starts_with("Deleted agent 'recon-scout' at "), "{}", deleted.text);
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn create_and_delete_round_trip_project_scope_with_collision_guard() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let create_cfg = serde_json::json!({ "name": "proj-only", "description": "Project agent", "scope": "project" });
        let created = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&create_cfg))).await.expect("create ok");
        assert!(!created.is_error, "{}", created.text);
        let file = cfg.project_agent_dirs[0].join("proj-only.md");
        assert!(file.exists());

        // Re-create is rejected (name already exists in the same scope).
        let again = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&create_cfg))).await.expect("no discovery error");
        assert!(again.is_error);
        assert!(again.text.contains("already exists in project scope"), "{}", again.text);

        let deleted = handle_management_action(&cfg, "delete", &mreq(Some("proj-only"), None, None, None)).await.expect("delete ok");
        assert!(!deleted.is_error, "{}", deleted.text);
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn update_and_delete_reject_builtin_agents_with_read_only_message() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let upd_cfg = serde_json::json!({ "description": "hijack" });
        let upd = handle_management_action(&cfg, "update", &mreq(Some("reviewer"), None, None, Some(&upd_cfg))).await.expect("no discovery error");
        assert!(upd.is_error);
        assert!(upd.text.contains("Agent 'reviewer' is read-only and cannot be modified"), "{}", upd.text);

        let del = handle_management_action(&cfg, "delete", &mreq(Some("reviewer"), None, None, None)).await.expect("no discovery error");
        assert!(del.is_error);
        assert!(del.text.contains("Agent 'reviewer' is read-only and cannot be modified"), "{}", del.text);
        // The bundled builtin file was NOT removed.
        assert!(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/agents/reviewer.md").exists());
    }

    #[tokio::test]
    async fn create_rejects_invalid_package_with_pi_message() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let bad = serde_json::json!({ "name": "Scout", "package": "!!!", "description": "x", "scope": "project" });
        let out = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&bad))).await.expect("no discovery error");
        assert!(out.is_error);
        assert!(out.text.contains("config.package is invalid"), "{}", out.text);
    }

    #[tokio::test]
    async fn create_rejects_non_boolean_completion_guard_with_exact_pi_message() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let bad = serde_json::json!({ "name": "test-runner", "description": "Run tests", "scope": "project", "completionGuard": "false" });
        let out = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&bad))).await.expect("no discovery error");
        assert!(out.is_error);
        assert!(out.text.contains("config.completionGuard must be a boolean"), "{}", out.text);
    }

    #[tokio::test]
    async fn create_surfaces_json_parse_errors_for_string_config() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let bad = serde_json::json!("{\"name\":");
        let out = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&bad))).await.expect("no discovery error");
        assert!(out.is_error);
        assert!(out.text.contains("config must be valid JSON:"), "{}", out.text);
    }

    #[tokio::test]
    async fn create_delegate_gets_name_sensitive_defaults_and_shadow_note() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let c = serde_json::json!({ "name": "delegate", "description": "Delegate helper", "scope": "project" });
        let created = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&c))).await.expect("create ok");
        assert!(!created.is_error, "{}", created.text);
        assert!(created.text.contains("shadows the builtin agent 'delegate'"), "{}", created.text);

        let got = handle_management_action(&cfg, "get", &mreq(Some("delegate"), None, None, None)).await.expect("get ok");
        // The custom project delegate wins over the builtin and shows delegate's name-sensitive defaults.
        assert!(got.text.contains("Agent: delegate (project)"), "{}", got.text);
        assert!(got.text.contains("System prompt mode: append"), "{}", got.text);
        assert!(got.text.contains("Inherit project context: true"), "{}", got.text);
        assert!(got.text.contains("Inherit skills: false"), "{}", got.text);
    }

    #[tokio::test]
    async fn get_unknown_agent_is_a_not_found_error_listing_available() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let out = handle_management_action(&cfg, "get", &mreq(Some("nope"), None, None, None)).await.expect("no discovery error");
        assert!(out.is_error);
        assert!(out.text.contains("Agent 'nope' not found. Available: "), "{}", out.text);
        assert!(out.text.contains("reviewer"), "available list must include the builtins: {}", out.text);
    }

    #[tokio::test]
    async fn create_chain_appears_in_list_and_get_renders_steps() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let chain_cfg = serde_json::json!({
            "name": "Review Flow",
            "description": "Scout then review",
            "scope": "project",
            "steps": [
                { "agent": "scout", "task": "Find targets" },
                { "agent": "reviewer", "task": "Review {previous}", "model": "fast" }
            ]
        });
        let created = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&chain_cfg))).await.expect("create ok");
        assert!(!created.is_error, "{}", created.text);
        assert!(created.text.starts_with("Created chain 'review-flow' at "), "{}", created.text);
        // scout + reviewer are builtins, so no unknown-agent warning is appended.
        assert!(!created.text.contains("unknown agents"), "{}", created.text);

        let listed = handle_management_action(&cfg, "list", &mreq(None, None, None, None)).await.expect("list ok");
        assert!(listed.text.contains("- review-flow (project): Scout then review"), "{}", listed.text);

        let got = handle_management_action(&cfg, "get", &mreq(None, Some("review-flow"), None, None)).await.expect("get ok");
        assert!(!got.is_error, "{}", got.text);
        assert!(got.text.contains("Chain: review-flow (project)"), "{}", got.text);
        assert!(got.text.contains("1. scout"), "{}", got.text);
        assert!(got.text.contains("   Task: Find targets"), "{}", got.text);
        assert!(got.text.contains("2. reviewer"), "{}", got.text);
        assert!(got.text.contains("   Model: fast"), "{}", got.text);
    }

    #[tokio::test]
    async fn create_chain_warns_on_unknown_step_agents() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let chain_cfg = serde_json::json!({
            "name": "mystery",
            "description": "refs a ghost",
            "scope": "user",
            "steps": [ { "agent": "ghost-agent", "task": "boo" } ]
        });
        let created = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&chain_cfg))).await.expect("create ok");
        assert!(!created.is_error, "{}", created.text);
        assert!(created.text.contains("Warning: chain steps reference unknown agents: ghost-agent."), "{}", created.text);
    }

    #[tokio::test]
    async fn models_lists_builtin_mapping_without_a_live_session_degrades_to_unavailable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let out = handle_management_action(&cfg, "models", &mreq(None, None, None, None)).await.expect("models ok");
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.starts_with("Builtin subagent models"), "{}", out.text);
        for name in BUILTIN_AGENT_NAMES {
            assert!(out.text.contains(name), "missing builtin {name}: {}", out.text);
        }
        // (d) No live session model bound (`current_session_model: None`) ⇒ the genuine no-host
        // degrade, exactly as before this seam existed.
        assert!(out.text.contains("Current session model:\n  (unavailable)"), "{}", out.text);
    }

    #[tokio::test]
    async fn models_renders_the_live_inherited_session_model_when_bound() {
        // With a live parent session model threaded in (pi `ctx.model`), the report shows the REAL
        // `provider/id` on the `Current session model` line, and an inheriting builtin (no own
        // `model`) falls back to it as its effective model / "inherits current session model" source.
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let model = "together/zai-org/GLM-5.2";
        let req = ManagementRequest {
            agent: None,
            chain_name: None,
            agent_scope: None,
            config: None,
            current_session_model: Some(model),
            proactive_skills: None,
        };
        let out = handle_management_action(&cfg, "models", &req).await.expect("models ok");
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains(&format!("Current session model:\n  {model}")),
            "the live inherited model must render instead of (unavailable): {}",
            out.text
        );
        assert!(
            !out.text.contains("(unavailable)"),
            "no (unavailable) degrade when a live session model is bound: {}",
            out.text
        );
    }

    #[tokio::test]
    async fn models_rejects_unknown_builtin_filter() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let out = handle_management_action(&cfg, "models", &mreq(Some("not-a-builtin"), None, None, None)).await.expect("no discovery error");
        assert!(out.is_error);
        assert!(out.text.contains("Builtin agent 'not-a-builtin' not found"), "{}", out.text);
    }

    // -----------------------------------------------------------------------------------------
    // G97 — aliases through the real management surface
    // -----------------------------------------------------------------------------------------

    /// `aliases:` must survive a serialize -> re-parse round-trip — the same silent-deletion trap
    /// `memory:`/`toolBudget:` had: both spellings are now `KNOWN_FIELDS`, so a key the serializer
    /// never emits is dropped the first time management rewrites the file.
    ///
    /// An UPDATE that does not mention `aliases` must not delete an existing `alias:`/`aliases:`
    /// line — pi's preserve set covers both spellings (`agent-serializer.ts:60`).
    #[tokio::test]
    async fn an_unrelated_update_preserves_an_existing_alias_line() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "seer.md",
            "---\nname: seer\ndescription: Sees\nalias: prophet\n---\n\nBody\n",
        );

        let config = serde_json::json!({ "description": "Sees further" });
        let out = handle_management_action(&cfg, "update", &mreq(Some("seer"), None, None, Some(&config)))
            .await.expect("update ok");
        assert!(!out.is_error, "{}", out.text);

        let written = std::fs::read_to_string(cfg.user_agent_dirs[0].join("seer.md")).expect("read");
        assert!(
            written.contains("aliases: prophet"),
            "an update that never mentioned aliases must not drop them:\n{written}"
        );
    }

    /// `config.aliases` sets / clears the list, and rejects a wrong-typed value with pi's message
    /// (`agent-management.ts:411-421`).
    #[tokio::test]
    async fn config_aliases_sets_clears_and_validates() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "seer.md",
            "---\nname: seer\ndescription: Sees\n---\n\nBody\n",
        );

        // String (CSV) form, with the agent's own name filtered out.
        let set = serde_json::json!({ "aliases": "prophet, seer , oracle-lite" });
        let out = handle_management_action(&cfg, "update", &mreq(Some("seer"), None, None, Some(&set)))
            .await.expect("update ok");
        assert!(!out.is_error, "{}", out.text);
        let written = std::fs::read_to_string(cfg.user_agent_dirs[0].join("seer.md")).expect("read");
        assert!(
            written.contains("aliases: prophet, oracle-lite"),
            "the agent's own name must be filtered out of its aliases:\n{written}"
        );

        // Array form, de-duplicated.
        let arr = serde_json::json!({ "aliases": ["prophet", "prophet", " diviner "] });
        handle_management_action(&cfg, "update", &mreq(Some("seer"), None, None, Some(&arr)))
            .await.expect("update ok");
        let written = std::fs::read_to_string(cfg.user_agent_dirs[0].join("seer.md")).expect("read");
        assert!(written.contains("aliases: prophet, diviner"), "{written}");

        // `false` clears. pi's serializer emits the line only when there IS a value or when the
        // preserve set still carries the key — and `preservedAgentFrontmatterFields` REMOVES both
        // spellings for an update that set `aliases` (`agent-management.ts:287`) — so a clear drops
        // the line entirely rather than writing an empty one.
        let clear = serde_json::json!({ "aliases": false });
        handle_management_action(&cfg, "update", &mreq(Some("seer"), None, None, Some(&clear)))
            .await.expect("update ok");
        let written = std::fs::read_to_string(cfg.user_agent_dirs[0].join("seer.md")).expect("read");
        assert!(!written.contains("aliases:"), "a cleared alias list writes no line:\n{written}");
        let reparsed = crate::discovery::frontmatter::parse_agent_file(
            &written,
            AgentSource::User,
            Path::new("/seer.md"),
        )
        .expect("reparses");
        assert!(reparsed.aliases.is_empty());

        // Wrong type -> pi's exact validation message.
        let bad = serde_json::json!({ "aliases": 7 });
        let out = handle_management_action(&cfg, "update", &mreq(Some("seer"), None, None, Some(&bad)))
            .await.expect("no discovery error");
        assert!(out.is_error);
        assert_eq!(
            out.text,
            "config.aliases must be a comma-separated string, string array, or false when provided."
        );
    }

    /// `list` renders `, aliases: …` and `get` renders an `Aliases:` line
    /// (`agent-management.ts:672,774` @v0.43.0); `get` is also reachable BY the alias.
    #[tokio::test]
    async fn list_and_get_render_aliases_and_get_resolves_by_alias() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "seer.md",
            "---\nname: seer\ndescription: Sees\naliases: prophet, diviner\n---\n\nBody\n",
        );

        let list = handle_management_action(&cfg, "list", &mreq(None, None, None, None)).await.expect("list ok");
        assert!(
            list.text.contains("- seer (user, aliases: prophet, diviner): Sees"),
            "{}",
            list.text
        );

        let by_alias = handle_management_action(&cfg, "get", &mreq(Some("prophet"), None, None, None))
            .await.expect("get ok");
        assert!(!by_alias.is_error, "{}", by_alias.text);
        assert!(by_alias.text.contains("Agent: seer (user)"), "{}", by_alias.text);
        assert!(by_alias.text.contains("Aliases: prophet, diviner"), "{}", by_alias.text);
    }

    /// Two agents claiming the SAME alias make every management path that would have to pick one
    /// refuse, with pi's `Ambiguous agent alias or name` wording (`agent-management.ts:624-626,880-882` @v0.43.0).
    #[tokio::test]
    async fn an_ambiguous_alias_is_refused_by_get_update_and_disable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "seer.md",
            "---\nname: seer\ndescription: Sees\naliases: prophet\n---\n\nBody\n",
        );
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "augur.md",
            "---\nname: augur\ndescription: Augurs\naliases: prophet\n---\n\nBody\n",
        );

        let get = handle_management_action(&cfg, "get", &mreq(Some("prophet"), None, None, None))
            .await.expect("no discovery error");
        assert!(get.is_error);
        assert_eq!(get.text, "Ambiguous agent alias or name 'prophet': augur, seer");

        let config = serde_json::json!({ "description": "changed" });
        let update =
            handle_management_action(&cfg, "update", &mreq(Some("prophet"), None, None, Some(&config)))
                .await.expect("no discovery error");
        assert!(update.is_error);
        assert_eq!(update.text, "Ambiguous agent alias or name 'prophet': augur, seer");

        // `disable` goes through `resolve_effective_agent`, whose ambiguity message is
        // `resolveAgentName`'s own (`agents.ts:526`), surfaced verbatim.
        let disable =
            handle_management_action(&cfg, "disable", &mreq(Some("prophet"), None, Some("user"), None))
                .await.expect("no discovery error");
        assert!(disable.is_error);
        assert_eq!(disable.text, "Ambiguous agent alias 'prophet': augur, seer");
        assert!(
            !disable.text.contains("not found"),
            "an ambiguous alias must NEVER be reported as not found: {}",
            disable.text
        );
    }

    /// `disable`/`enable` reach their target BY alias and write the override under the agent's
    /// CANONICAL name (`agent-management.ts:987-991`).
    #[tokio::test]
    async fn disable_by_alias_writes_the_override_under_the_canonical_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut cfg = mgmt_cfg(tmp.path());
        cfg.override_settings.user_settings_path = tmp.path().join("user/agents/settings.json");
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "seer.md",
            "---\nname: seer\ndescription: Sees\naliases: prophet\n---\n\nBody\n",
        );

        let out = handle_management_action(&cfg, "disable", &mreq(Some("prophet"), None, Some("user"), None))
            .await.expect("no discovery error");
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("Disabled agent 'seer'"), "{}", out.text);

        let settings = std::fs::read_to_string(&cfg.override_settings.user_settings_path)
            .expect("settings written");
        let value: serde_json::Value = serde_json::from_str(&settings).expect("valid json");
        assert_eq!(
            value["subagents"]["agentOverrides"]["seer"]["disabled"],
            serde_json::Value::Bool(true),
            "the override must be keyed on the canonical name, not the alias: {settings}"
        );
    }

    /// A chain step that names an ALIAS is a known agent — pi swapped the `Set(names)` membership
    /// test for `resolveAgentName` in v0.43.0 (`agent-management.ts:169-174`).
    #[tokio::test]
    async fn a_chain_step_naming_an_alias_does_not_warn_as_unknown() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        write_agent_md(
            &cfg.user_agent_dirs[0],
            "seer.md",
            "---\nname: seer\ndescription: Sees\naliases: prophet\n---\n\nBody\n",
        );

        let config = serde_json::json!({
            "name": "foresee",
            "description": "A chain",
            "scope": "user",
            "steps": [{ "agent": "prophet", "task": "look ahead" }],
        });
        let out = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&config)))
            .await.expect("create ok");
        assert!(!out.is_error, "{}", out.text);
        assert!(
            !out.text.contains("unknown agents"),
            "an alias-named step must not be reported as unknown: {}",
            out.text
        );

        // Control: a step naming nothing at all still warns, so the assertion above is really
        // measuring alias resolution and not a broken warning path.
        let ghost = serde_json::json!({
            "name": "haunted",
            "description": "A chain",
            "scope": "user",
            "steps": [{ "agent": "ghost-agent", "task": "boo" }],
        });
        let out = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&ghost)))
            .await.expect("create ok");
        assert!(
            out.text.contains("Warning: chain steps reference unknown agents: ghost-agent."),
            "{}",
            out.text
        );
    }

    /// G99: the roster is the SEVEN names pi declares at v0.43.0 (`agents.ts:38-46`), and the
    /// all-agents model report walks EXACTLY that static list.
    ///
    /// `advisor` is in the roster but ships no `advisor.md` — upstream `34a018f` demoted it to an
    /// `oracle` ALIAS — and `handleModels` looks builtins up by EXACT name
    /// (`agent-management.ts:850`, `builtinByName.get(name)`), never through `resolveAgentName`. So
    /// `advisor` renders the missing row upstream too, and this pins that the alias is not silently
    /// promoted into a seventh definition.
    #[tokio::test]
    async fn the_models_report_walks_the_seven_name_roster_including_the_fileless_advisor() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let out = handle_management_action(&cfg, "models", &mreq(None, None, None, None))
            .await.expect("models ok");
        assert!(!out.is_error, "{}", out.text);

        assert_eq!(
            BUILTIN_AGENT_NAMES,
            ["advisor", "delegate", "oracle", "researcher", "reviewer", "scout", "worker"]
        );
        for name in BUILTIN_AGENT_NAMES {
            assert!(out.text.contains(&format!("\n{name}\n")), "{name} row missing:\n{}", out.text);
        }
        for gone in ["planner", "context-builder"] {
            assert!(
                !out.text.contains(&format!("\n{gone}\n")),
                "the removed role {gone} must not be reported:\n{}",
                out.text
            );
        }
        assert!(
            out.text.contains("advisor\n  model:\n    (builtin definition not found)\n  source: missing"),
            "advisor ships no file of its own and must render the missing row:\n{}",
            out.text
        );
        // The six roles that DO ship a file resolve to a real definition.
        assert!(
            !out.text.contains("oracle\n  model:\n    (builtin definition not found)"),
            "{}",
            out.text
        );
    }

    #[tokio::test]
    async fn unknown_action_is_reported() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let out = handle_management_action(&cfg, "frobnicate", &mreq(None, None, None, None)).await.expect("no discovery error");
        assert!(out.is_error);
        assert!(out.text.contains("Unknown action: frobnicate"), "{}", out.text);
    }

    #[tokio::test]
    async fn get_renders_packaged_agent_local_name_and_package() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        let create_cfg = serde_json::json!({
            "name": "Scout",
            "package": "Code Analysis",
            "description": "Fast recon",
            "scope": "project"
        });
        let created = handle_management_action(&cfg, "create", &mreq(None, None, None, Some(&create_cfg))).await.expect("create ok");
        assert!(!created.is_error, "{}", created.text);
        assert!(created.text.starts_with("Created agent 'code-analysis.scout' at "), "{}", created.text);

        let got = handle_management_action(&cfg, "get", &mreq(Some("code-analysis.scout"), None, None, None)).await.expect("get ok");
        assert!(got.text.contains("Agent: code-analysis.scout (project)"), "{}", got.text);
        assert!(got.text.contains("Local name: scout"), "{}", got.text);
        assert!(got.text.contains("Package: code-analysis"), "{}", got.text);
    }


    // -----------------------------------------------------------------------------------------
    // SUBA-086: `list`/`models` render `Invalid agent definitions:`; `get` refuses a blocked name
    // -----------------------------------------------------------------------------------------

    #[tokio::test]
    async fn list_and_models_render_invalid_agent_definitions_and_get_refuses_a_blocked_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = mgmt_cfg(tmp.path());
        std::fs::create_dir_all(&cfg.project_agent_dirs[0]).expect("mkdir project agents");
        // `worker` also exists as a bundled builtin — the broken project file must OUTRANK it.
        std::fs::write(
            cfg.project_agent_dirs[0].join("worker.md"),
            "---\nname: worker\ndescription: d\ntimeoutMs: 30s\n---\n\nBody\n",
        )
        .expect("write broken worker");
        // `ghost` exists ONLY as a broken file.
        std::fs::write(
            cfg.project_agent_dirs[0].join("ghost.md"),
            "---\nname: ghost\ndescription: d\noutputMode: file\n---\n\nBody\n",
        )
        .expect("write broken ghost");

        let listed = handle_management_action(&cfg, "list", &mreq(None, None, None, None))
            .await
            .expect("list ok");
        assert!(!listed.is_error);
        assert!(
            listed.text.contains(
                "\n\nInvalid agent definitions:\n- ghost (project): Agent 'ghost' has invalid outputMode frontmatter; expected 'inline' or 'file-only'.\n- worker (project): Agent 'worker' has invalid timeoutMs frontmatter; expected a positive integer."
            ),
            "{}",
            listed.text
        );
        let executable = listed.text.split("\n\nChains:").next().expect("the agents section");
        assert!(
            executable.contains("- worker (builtin") && !executable.contains("- worker (project"),
            "the broken project file must not be listed as an executable agent (the bundled \
             builtin still is):\n{executable}"
        );
        // pi `handleList` (`agent-management.ts:946` @v0.64.0) hands the block `d.agentDiagnostics`
        // UNFILTERED, so a user-scoped listing still reports the broken project file.
        let user_listed = handle_management_action(&cfg, "list", &mreq(None, None, Some("user"), None))
            .await
            .expect("list ok");
        assert!(user_listed.text.contains("Invalid agent definitions:"), "{}", user_listed.text);

        let got = handle_management_action(&cfg, "get", &mreq(Some("worker"), None, None, None))
            .await
            .expect("get ok");
        assert!(got.is_error, "{}", got.text);
        assert_eq!(
            got.text,
            "Agent 'worker' has invalid configuration: Agent 'worker' has invalid timeoutMs frontmatter; expected a positive integer."
        );
        let ghost = handle_management_action(&cfg, "get", &mreq(Some("ghost"), None, None, None))
            .await
            .expect("get ok");
        assert!(ghost.is_error);
        assert!(
            ghost.text.starts_with("Agent 'ghost' has invalid configuration:"),
            "a name with ONLY a broken definition is invalid, not `not found`: {}",
            ghost.text
        );

        let models = handle_management_action(&cfg, "models", &mreq(None, None, None, None))
            .await
            .expect("models ok");
        assert!(
            models.text.ends_with(
                "Invalid agent definitions:\n- ghost (project): Agent 'ghost' has invalid outputMode frontmatter; expected 'inline' or 'file-only'.\n- worker (project): Agent 'worker' has invalid timeoutMs frontmatter; expected a positive integer."
            ),
            "{}",
            models.text
        );
    }
}
