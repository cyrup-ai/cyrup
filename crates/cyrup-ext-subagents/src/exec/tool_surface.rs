//! The resolved child tool surface — one resolution, published to the parent.
//!
//! # Why this module exists
//!
//! **Subagent tools are not inherited from the parent session, and nothing said so.** A child's
//! tool surface is a pure function of the target agent's own `tools:` declaration, narrowed by a
//! registered capability ceiling's `allowedTools` and by the agent's own `excludeTools`
//! (`spawn_plan::resolve_child_tools`). The launching session's registry is never an input;
//! `context: "fresh"` / `"fork"` selects a conversation context and has zero interaction with the
//! tool plan.
//!
//! That design is correct. What was missing is any way to FIND OUT: a parent could write "you have
//! full read/bash/grep access" into a `reviewer`'s task prompt, and the child — which genuinely has
//! `read, grep, find, ls, intercom` and no shell at all — would silently adapt and mention the gap
//! only in prose the parent may never read. The one existing feedback channel, SUBA-045
//! ([`crate::exec::tool_availability`]), answers the INVERSE question ("the agent declared a tool
//! the child's host never registered") and deletes itself on every healthy run, so the resolved
//! surface was discarded on the success path.
//!
//! This module closes that by resolving the surface ONCE, in `resolve_tool_surface` — hoisted
//! verbatim out of `resolve_child_tools` so the `--tools` CSV, the result projection
//! ([`crate::exec::run_result::SingleResult::tool_surface`]) and every launch reply read the SAME
//! value and cannot drift apart — and by stating the rule itself in `NON_INHERITANCE_NOTICE` and
//! `INSPECT_HINT`, so the bundled skill documentation quotes one wording rather than inventing its
//! own.
//!
//! # What a published surface does and does not prove
//!
//! **An unpinned child's surface is not knowable from the parent.** With no `--tools`/`--no-tools`
//! the child selects `DEFAULT_BUILTIN_TOOLS` (`read`/`bash`/`edit`/`write`,
//! `cyrup-session-svc/src/builder.rs:336`) — itself replaceable by the CHILD's own `defaultTools`
//! setting (CFG-079), read in the child process after the parent has already spawned it. That is
//! what `ResolvedToolSurface::pinned` records.
//!
//! # SUBA-114 — no prediction from the parent's registry
//!
//! Until upstream `b12496b8` (#2289, v0.70.0) the plan also intersected the declared built-ins
//! with the LAUNCHING session's registry (pi `getHostBuiltinToolNames` / `hostAvailableBuiltins`),
//! dropped whatever the parent lacked with a warning, and refused a `reviewer`/`scout` outright.
//! That registry is bounded by the parent's own `--tools`/`--no-tools`, so a narrow dispatcher
//! stripped every child — and every grandchild, compounding — of the tools its agent declared.
//! The prediction is gone here as it is upstream: the child is launched with what its agent
//! declares (narrowed only by the ceiling and `excludeTools`), and the child itself refuses at
//! `agent_start`, before its first model call, when its REAL registry lacks a required tool
//! (`prompt_runtime`'s `refresh_tool_diagnostic`, [`crate::exec::tool_availability`]).

use crate::error::SubagentError;
use crate::exec::agent_config::AgentConfig;

/// The `codemode` tool's name (`cyrup_codemode_runtime::EXTENSION_ID`, which is also the tool's
/// name). This crate does not depend on the runtime crate, so the literal is restated.
const CODEMODE_TOOL_NAME: &str = "codemode";

// ================================================================================================
// ResolvedToolSurface
// ================================================================================================

/// The tool surface one child will actually launch with, resolved from the SAME four inputs
/// `spawn_plan::resolve_child_tools` uses and by the SAME code (see [`resolve_tool_surface`]).
///
/// SUBA-114 — no `unavailableHostBuiltins` and no host-omission `warnings`: both were produced only
/// by the parent-registry prediction upstream deleted in `b12496b8` (v0.70.0, which dropped the
/// audit field with it). A result payload written before that still decodes: this type does not
/// deny unknown fields, so the two old keys are simply ignored
/// (`tests::a_payload_carrying_the_removed_host_fields_still_decodes`).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedToolSurface {
    /// pi's `declaredBuiltinTools` after the ceiling filter and the `excludeTools` subtraction —
    /// exactly the names that reach the `--tools` CSV before the resolved MCP names are appended.
    ///
    /// May contain non-builtin names (`intercom`, `contact_supervisor`): those are DECLARATIONS
    /// that drive SUBA-045's `requiredChildTools`, not enforced grants — see the module doc.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub builtins: Vec<String>,
    /// pi's `explicitToolAllowlist`: `agent.tools.is_some() || ceiling.allowed_tools.is_some()`.
    ///
    /// `false` means nothing pinned the surface and the child keeps its own ambient default —
    /// which the parent CANNOT enumerate. Carried rather than inferred from an empty
    /// [`Self::builtins`], because `Some(vec![])` (an agent that asked for `--no-tools`) is
    /// pinned-and-empty and is the exact opposite grant from unpinned.
    #[serde(default)]
    pub pinned: bool,
    /// The trimmed, de-duplicated `excludeTools` actually applied. Exact on BOTH arms: an unpinned
    /// agent's exclusions still reach the child as `--exclude-tools` (`spawn_plan.rs:830-838`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded: Vec<String>,
    /// pi `toolPlan.fanoutAuthorized` — whether this child may itself delegate.
    #[serde(default)]
    pub fanout_authorized: bool,
    /// pi `effectiveMcpTools` (`child-tool-plan.ts:449-451`) — the RESOLVED direct-MCP names,
    /// ceiling- and exclusion-filtered. Carried rather than recomputed: it has a second consumer in
    /// the `MCP_DIRECT_CHILD_TOOLS` env ([`crate::exec::tool_availability::MCP_DIRECT_CHILD_TOOLS_ENV`],
    /// written by `spawn_plan::env_control_channels`), which is what lets the child's diagnostic
    /// tell a missing MCP tool from a missing extension tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effective_mcp_tools: Vec<String>,
    /// pi `requiredChildTools` (`child-tool-plan.ts:471-478`) — the names the child is expected to
    /// actually HAVE, which is deliberately NOT [`Self::effective_tool_allowlist`].
    ///
    /// Two differences, both upstream's. The declared-builtin and MCP terms are CONDITIONAL on the
    /// AGENT having declared each (`:474-475`) — a surface pinned only by a capability ceiling
    /// contributes neither, because a ceiling bounds what the child MAY use and asserts nothing
    /// about what it HAS. And the runtime-registered coordination names are filtered out (`:477`):
    /// `contact_supervisor` is registered by the child at RUNTIME, and for a cyrup-spawned child
    /// NEITHER provider registers it once `cyrup-intercom` is installed — the native side stands
    /// down on `intercom_supervisor_channel_available` (`native_supervisor.rs:1823-1828`, which
    /// reads `CYRUP_INTERCOM` / `intercom/config.json`) and `cyrup-intercom` stands down on
    /// `CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR` (`cyrup-intercom/src/extension.rs:466-475`), which
    /// the spawn plan always sets for a child with a parent session. Demanding the name at
    /// `agent_start` therefore reports a tool that was never going to be there.
    ///
    /// A stored FIELD, unlike [`Self::effective_tool_allowlist`]'s method, and the asymmetry is
    /// forced rather than chosen: this list needs `agent.tools.is_some()` and
    /// `!mcp_direct_tools.is_empty()` SEPARATELY, and [`Self::pinned`] is their disjunction with
    /// the ceiling — neither is recoverable from the finished surface. Deriving it here would be a
    /// second, different resolution, which is the very drift the method form exists to avoid.
    ///
    /// It is also the only honest view a parent gets of the CSV/required divergence: the `--tools`
    /// CSV never leaves as a list, only as a joined argv string.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_child_tools: Vec<String>,
    /// pi `toolExtensionPaths` (`child-tool-plan.ts:424-430`) — the agent's `tools:` entries that
    /// name an extension file, already emptied under a `denyExtensions` ceiling exactly as upstream
    /// does (upstream filters the PLAN field itself, not just its consumer).
    ///
    /// In practice only a `RunnerConfig` round-trip populates this: neither
    /// `discovery::frontmatter::parse_tool_refs` nor
    /// [`crate::discovery::types::ToolRef::from_tool_string`] can emit a
    /// [`crate::discovery::types::ToolRef::ExtensionPath`] — both are two-arm matches over the
    /// `mcp:` prefix — so the sole production producer is that type's own adjacently-tagged
    /// `Deserialize`.
    ///
    /// `#[serde(skip)]`, not `skip_serializing_if`: this is an argv input, not part of the
    /// parent-facing surface, and `skip` also skips DEserialization — a parent decoding a child's
    /// result must not be able to inject extension paths. The field is consumed by
    /// `push_extension_and_skill_args` before the plan is built, so the lossy round-trip is
    /// unobservable.
    #[serde(skip)]
    pub tool_extension_paths: Vec<String>,
    /// pi `mcpDirectTools` — the RAW `mcp:` selectors as declared, before resolution AND before any
    /// ceiling axis. Deliberately NOT deny-filtered, matching upstream: `pi-args.ts:916-926` reads
    /// `input.mcpDirectTools` raw on its no-ceiling arm.
    ///
    /// Distinct from [`Self::effective_mcp_tools`] (resolved NAMES): `MCP_DIRECT_TOOLS` is the MCP
    /// adapter's own allowlist input and speaks selectors, and `spawn_plan::env_identity_and_depth`
    /// applies its own per-selector ceiling test to this raw list. `#[serde(skip)]` for the same
    /// reason as [`Self::tool_extension_paths`].
    #[serde(skip)]
    pub mcp_direct_tools: Vec<String>,
}

impl ResolvedToolSurface {
    /// The human/model-facing one-liner for a RESOLVED surface — the only prose statement of the
    /// pinned / unpinned / `--no-tools` distinction, which is why it is retained even though no
    /// in-tree caller reads it today.
    ///
    /// Deliberately distinct from `discovery::management::render::tool_list_str`, which renders the
    /// `, tools:` segment of `action: "list"` (`discovery/management/handlers.rs:222`): that reads an
    /// `AgentDefinition` — the DECLARATION, all a listing can honestly show, having no capability
    /// ceiling or `excludeTools` context — where this reads the RESOLUTION.
    #[must_use]
    pub fn render(&self) -> String {
        let mut text = if self.pinned {
            if self.builtins.is_empty() {
                "(none — this agent launches with --no-tools)".to_string()
            } else {
                self.builtins.join(", ")
            }
        } else {
            "(unpinned — this agent declares no `tools:`, so the child keeps its own default \
             built-in set)"
                .to_string()
        };
        if !self.excluded.is_empty() {
            text.push_str(" minus excludeTools: ");
            text.push_str(&self.excluded.join(", "));
        }
        text
    }

    /// pi `effectiveToolAllowlist` (`child-tool-plan.ts:457-463`) — exactly what reaches the
    /// `--tools` CSV and, through it, `REQUIRED_CHILD_TOOLS`.
    ///
    /// A method rather than a stored field because it is DERIVED from [`Self::builtins`] and
    /// [`Self::effective_mcp_tools`]: a stored copy is a third thing that can drift from the two it
    /// summarizes.
    ///
    /// SUBA-092 / upstream's `[...new Set([...])]`: de-duplicated in first-seen order. cyrup's
    /// pre-fold `allowlist.extend(...)` did not dedup, so a resolved MCP name that collides with a
    /// declared builtin used to emit a duplicated `--tools` entry.
    ///
    /// Upstream's third term, `internalTools` (`:456`), IS ported — the run's own
    /// `structured_output` grant, folded into `builtins` by the resolver alongside the
    /// `allowNestedSubagents` re-grant of `subagent`. See the note on `pinned`'s own resolution
    /// below for why it was once absent and what changed.
    #[must_use]
    pub fn effective_tool_allowlist(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.builtins
            .iter()
            .chain(self.effective_mcp_tools.iter())
            .filter(|tool| seen.insert((*tool).clone()))
            .cloned()
            .collect()
    }
}

/// `skip_serializing_if` for [`crate::exec::run_result::SingleResult::tool_surface`] — a free
/// function because `#[serde(skip_serializing_if = "…")]` takes a path, not a method.
#[must_use]
pub fn is_default_surface(surface: &ResolvedToolSurface) -> bool {
    *surface == ResolvedToolSurface::default()
}

// ================================================================================================
// The resolver (hoisted verbatim out of `resolve_child_tools`)
// ================================================================================================

/// pi `resolvePiLaunchToolPlan`'s builtin half (`runs/shared/pi-args.ts:361-371,444-509` @v0.64.0),
/// lifted out of `spawn_plan::resolve_child_tools` so the result projection
/// ([`crate::exec::run_result::SingleResult::tool_surface`]), the launch replies that publish
/// `resolvedTools` and the argv builder all read ONE resolution.
///
/// The bodies below are that function's, moved unchanged with their pi-parity comments (SUBA-014 /
/// SUBA-072 / SUBA-092) — moving rather than summarizing them is what guarantees there is no second
/// resolver to drift.
///
/// `structured_output` is pi's `input.structuredOutput` (`child-tool-plan.ts:456`): the RUN declared
/// an `outputSchema`, so it is entitled to the `structured_output` tool regardless of what the
/// persona declared. A launch passes `structured_runtime.is_some()`; the `action:"list"` display
/// path passes `false`, because a rendered listing is a statement about the AGENT, not about one
/// launch.
///
/// Deliberately NOT an input: the launching session's own tool registry. Upstream predicted a
/// child's tools from it until `b12496b8` (#2289, v0.70.0) — see SUBA-114 in the body.
///
/// # Errors
///
/// [`SubagentError::CapabilityCeilingViolation`] when the ceiling excludes a required `read`
/// (`child-tool-plan.ts:316-320` @v0.71.0), and [`SubagentError::ToolContractUnsatisfiable`] for
/// the fanout refusal (`:342-344`) when the effective surface holds `subagent_supervisor` without
/// fanout authorization. Both are upstream throws and fire in that order.
pub fn resolve_tool_surface(
    agent: &AgentConfig,
    require_read_tool: bool,
    ceiling: Option<&crate::exec::capability_ceiling::ResolvedCapabilityCeiling>,
    structured_output: bool,
    cwd: &std::path::Path,
) -> Result<ResolvedToolSurface, SubagentError> {
    resolve_tool_surface_in(
        agent,
        require_read_tool,
        ceiling,
        structured_output,
        cwd,
        &crate::exec::mcp_direct_tools::McpDirs::from_env(),
    )
}

/// The injectable core of [`resolve_tool_surface`] — identical behaviour, but reading the direct-MCP
/// config/cache from an explicit [`crate::exec::mcp_direct_tools::McpDirs`] instead of the process
/// environment, so tests can drive a hermetic layout without `unsafe` env mutation (edition 2024).
///
/// Mirrors this crate's established seam pattern —
/// [`crate::exec::mcp_direct_tools::resolve_mcp_direct_tool_names_in`], `paths::home_dir_from`,
/// `paths::resolve_agent_dir_from`, `spawn::depth::resolve_effective_depth_from`. Both production
/// callers — `spawn_plan::build_attempt_spawn_plan_with_read_requirement` and the
/// `action:"list"` renderer in `extension::tool::routing` — use the thin wrapper above; only tests
/// call this directly.
///
/// `structured_output` carries the same meaning as in [`resolve_tool_surface`].
///
/// # Errors
///
/// Same two refusals as [`resolve_tool_surface`].
pub fn resolve_tool_surface_in(
    agent: &AgentConfig,
    require_read_tool: bool,
    ceiling: Option<&crate::exec::capability_ceiling::ResolvedCapabilityCeiling>,
    structured_output: bool,
    cwd: &std::path::Path,
    dirs: &crate::exec::mcp_direct_tools::McpDirs,
) -> Result<ResolvedToolSurface, SubagentError> {
    // pi `child-tool-plan.ts:371-374`'s `allowedToolSet`. The WHOLE ceiling is threaded in so the
    // refusal below can name its `sources`; every other consumer below needs only this one axis,
    // and deriving it once here is what keeps them from re-deriving it differently.
    let allowed = ceiling.and_then(|c| c.allowed_tools.as_ref());
    // SUBA-072 — the ceiling's OTHER axis, read here rather than passed in: it gates both the
    // direct-MCP resolution (pi `:431-433`) and `tool_extension_paths` (pi `:424-430`), which are
    // now both resolved in this function. `spawn_plan`'s own local of the same name survives on
    // purpose: it still feeds `--no-extensions` and the `MCP_DIRECT_TOOLS` three-way branch, which
    // this surface does not carry.
    let deny_extensions = ceiling.is_some_and(|c| c.deny_extensions);

    // pi `splitToolList` already ran at discovery time, so a `mcp:`-prefixed entry is a
    // `ToolRef::Mcp` holding the bare selector (pi's `mcpDirectTools`) and an extension-path entry
    // a `ToolRef::ExtensionPath` (pi's `toolExtensionPaths`). Re-split those typed refs here to
    // reproduce pi's THREE destinations for one `tools` list (`child-tool-plan.ts:379-383`,
    // `:424-433`) in the one function that owns the whole plan.
    let mut requested_builtin_tools: Vec<String> = Vec::new();
    let mut tool_extension_paths: Vec<String> = Vec::new();
    let mut mcp_direct_tools: Vec<String> = Vec::new();
    for tool in agent.tools.iter().flatten() {
        match tool {
            crate::discovery::types::ToolRef::Builtin(name) => {
                requested_builtin_tools.push(name.clone());
            }
            crate::discovery::types::ToolRef::ExtensionPath(path) => {
                tool_extension_paths.push(path.clone());
            }
            crate::discovery::types::ToolRef::Mcp(selector) => {
                mcp_direct_tools.push(selector.clone());
            }
        }
    }

    // pi `child-tool-plan.ts:390-394` (v0.71.0 `:316-320`), moved here from `spawn_plan.rs`'s
    // `build_attempt_spawn_plan_with_read_requirement` so the one resolver owns every tool-plan
    // refusal. Text and `"unknown source"` fallback unchanged by the move.
    //
    // SUBA-114: this is now the ONLY `read` refusal. Upstream's HOST twin ("Host runtime does not
    // provide required tool 'read' …", `:384-389` @v0.68.0) went with the rest of the host
    // prediction in `b12496b8` (v0.70.0): whether the child really has `read` is the child's own
    // registry's answer, checked at its `agent_start`.
    //
    // `ceiling` is already `Option<&_>`, so it is `.filter(…)` directly where the old site needed
    // `capability_ceiling.as_ref().filter(…)` around an owned `Option`. This reproduces upstream's
    // `capabilityCeiling?.sources.join(", ") || "unknown source"` exactly: the `||` fires both when
    // the ceiling is absent and when `join` yields `""`.
    if require_read_tool
        && let Some(allowed) = allowed
        && !allowed.iter().any(|tool| tool == "read")
    {
        let sources = ceiling
            .filter(|ceiling| !ceiling.sources.is_empty())
            .map_or_else(
                || "unknown source".to_string(),
                |ceiling| ceiling.sources.join(", "),
            );
        return Err(SubagentError::CapabilityCeilingViolation(format!(
            "Capability ceiling from {sources} excludes required tool 'read' for lazy skill \
             loading."
        )));
    }

    // pi `pi-args.ts:444-455`: `declaredBuiltinTools`. Computed UNCONDITIONALLY here (not gated
    // behind `explicit_tool_allowlist` below), mirroring upstream exactly: pi's own
    // `declaredBuiltinTools` ternary — and the `fanoutAuthorized` that reads it — both run before
    // `explicitToolAllowlist` is even checked. On the `tools !== undefined` arm (an agent that DID
    // write a `tools:` key) start from its declared builtins; on the `tools === undefined` arm —
    // reachable only because a ceiling pins the surface instead — the ceiling's own `allowedTools`
    // set becomes the declared set outright, never the ambient ("no restriction at all") set this
    // arm otherwise implies.
    //
    // SUBA-072 fix: this used to live ONLY inside the `if explicit_tool_allowlist` block (as
    // `allowlist`'s initializer), which meant `fanout_authorized` — computed separately from the raw
    // pre-ceiling `requested_builtin_tools` — never saw the ceiling filter applied to this same
    // list two paragraphs down. A ceiling excluding `subagent` from `allowedTools` therefore failed
    // to revoke nested-delegation authorization even though it correctly narrowed `--tools` itself;
    // hoisting this computation out and deriving `fanout_authorized` from ITS result (below) closes
    // that gap.
    //
    // SUBA-114 / pi `b12496b8` (v0.70.0): `declaredBuiltinTools = ceilingFilteredBuiltinTools`. The
    // ceiling-filtered declaration IS what the child is launched with; nothing here intersects it
    // with the LAUNCHING session's registry any more. That registry is bounded by the parent's own
    // `--tools`/`--no-tools`, so a dispatcher started as `--tools subagent,read` used to strip
    // `bash`/`edit`/`grep` from every child it launched — and, because each child's allowlist
    // became its own registry, the loss compounded at every hop. A child is a separate session
    // that builds its own tools; it validates its REAL registry against `requiredChildTools` at
    // `agent_start` (`prompt_runtime`'s `refresh_tool_diagnostic`) and names what is missing.
    let declared: Vec<String> = if agent.tools.is_some() {
        let mut declared = requested_builtin_tools;

        // SUBA-014 / pi `runs/shared/pi-args.ts:361-371` @v0.43.0. Upstream's `declaredBuiltinTools`
        // is
        //
        //   input.tools === undefined
        //     ? (ceiling ? [...ceiling] : [])
        //     : (requireReadTool && requestedBuiltinTools.length > 0
        //         && !requestedBuiltinTools.includes("read") && !allowedToolSet
        //         ? ["read", ...requestedBuiltinTools]
        //         : requestedBuiltinTools).filter(...)
        //
        // i.e. `read` is injected at the HEAD of the declared builtins — never appended, never
        // deduplicated away — under a three-way condition, and only on the `tools !== undefined`
        // arm, which is exactly this branch. `requestedBuiltinTools` is pi's `tools` minus the
        // extension-path entries (`/`, `.ts`, `.js`), which cyrup already split out as
        // `ToolRef::ExtensionPath`, so `requested_builtin_tools` IS that list.
        //
        // SUBA-072 gives the `!allowedToolSet` term teeth: with a ceiling in play the
        // head-injection is skipped and the ceiling-membership filter below decides `read`'s
        // fate instead — including upstream's own edge case, faithfully reproduced: an agent
        // that both omits `read` from an explicit `tools:` list AND launches under a ceiling
        // that itself permits `read` does not have it force-added here; the agent must ask for
        // it. (The launch-time throw in `build_attempt_spawn_plan` still guards the case that
        // actually matters — a ceiling that EXCLUDES `read` while it is required.)
        if require_read_tool
            && !declared.is_empty()
            && !declared.iter().any(|tool| tool == "read")
            && allowed.is_none()
        {
            declared.insert(0, "read".to_string());
        }

        // SUBA-072 / pi `pi-args.ts:455`: `.filter((tool) => !allowedToolSet ||
        // allowedToolSet.has(tool))` — a ceiling can only narrow an explicit declaration, never
        // widen it.
        if let Some(allowed) = allowed {
            declared.retain(|tool| allowed.contains(tool));
        }
        declared
    } else {
        // pi `pi-args.ts:445`: `allowedToolSet ? [...allowedToolSet] : []` — reached only when
        // `allowed.is_some()`, since `agent.tools` is `None` here and this arm would otherwise
        // imply the ambient (unrestricted) set.
        allowed.cloned().unwrap_or_default()
    };

    // SUBA-092 / pi `runs/shared/pi-args.ts:502-504` @v0.64.0:
    //
    //   const excludeTools = [...new Set((input.excludeTools ?? []).map((tool) => tool.trim()).filter(Boolean))];
    //   const excludedToolSet = new Set(excludeTools);
    //   const effectiveDeclaredBuiltinTools = declaredBuiltinTools.filter((tool) => !excludedToolSet.has(tool));
    //
    // The agent's own `excludeTools` is trimmed, de-duplicated (first-seen order) and SUBTRACTED
    // from the declared builtin set AFTER the ceiling filter above and BEFORE `fanout_authorized`
    // and the `--tools` CSV read it — so it narrows an explicit `tools:` allowlist, the ceiling's
    // own `allowedTools` set, and (via `--exclude-tools`) the ambient set of an agent that declared
    // no `tools:` at all. Pre-fix, `excludeTools:` was demoted to `extra_fields` and a declared
    // exclusion had no effect whatsoever.
    let exclude_tools: Vec<String> = {
        let mut seen = std::collections::HashSet::new();
        agent
            .exclude_tools
            .iter()
            .map(|tool| tool.trim())
            .filter(|tool| !tool.is_empty())
            .filter(|tool| seen.insert(tool.to_string()))
            .map(str::to_string)
            .collect()
    };
    let is_excluded = |tool: &str| exclude_tools.iter().any(|excluded| excluded == tool);
    // `mut` for the two RUN-level re-grants below (`subagent` under `allowNestedSubagents`, and
    // `internalTools`' `structured_output`), which append AFTER `fanout_authorized` is final.
    let mut effective_builtin_tools: Vec<String> = declared
        .into_iter()
        .filter(|tool| !is_excluded(tool))
        .collect();

    // pi `runs/shared/pi-args.ts:194`: `const fanoutAuthorized = declaredBuiltinTools.includes("subagent")` —
    // a persona is granted NESTED delegation exactly when the CEILING-FILTERED declared builtin set
    // (`effective_builtin_tools` above, SUBA-072) includes the `subagent` tool — NOT the raw
    // pre-ceiling `agent.tools` declaration. So a ceiling whose `allowedTools` excludes `subagent`
    // revokes fanout authorization even when the agent's own `tools:` declares it, and conversely a
    // ceiling that GRANTS `subagent` via `allowedTools` authorizes fanout even for an agent that
    // declares no `tools:` of its own (pi's `input.tools === undefined` arm, `[...allowedToolSet]`).
    // With no ceiling and no `tools:` declared, `effective_builtin_tools` is `[]` exactly as
    // `requested_builtin_tools` was, so an agent that declares nothing and launches unceilinged is still NOT
    // fanout-authorized — the pre-fix behavior in the only case that never involved a ceiling. This
    // is the single input to the child-role env pair, and through it to
    // [`crate::extension::resolve_registration_mode`]: authorized → `ChildSafe` (the restricted,
    // mutation-blocked `subagent` tool, pi `extension/fanout-child.ts:132`), unauthorized → the
    // child registers no subagent surface at all and cannot delegate.
    //
    // SUBA-092 / pi `pi-args.ts:505-509` @v0.64.0 — the full expression is now
    //
    //   effectiveDeclaredBuiltinTools.includes("subagent") || (
    //     input.allowNestedSubagents === true &&
    //     !excludedToolSet.has("subagent") &&
    //     (!allowedToolSet || allowedToolSet.has("subagent")))
    //
    // i.e. `allowNestedSubagents: true` is an INDEPENDENT grant for an agent that never named
    // `subagent` in a `tools:` allowlist — the only way such an agent could ever delegate before
    // this landed — but it is still subordinate to both subtractive bounds: the agent's own
    // `excludeTools` and a ceiling whose `allowedTools` omits `subagent` each veto it. And because
    // the first disjunct reads the EXCLUSION-filtered list, `excludeTools: [subagent]` now revokes
    // the grant an explicit `tools: [subagent]` would otherwise confer.
    let subagent_within_ceiling = allowed.is_none_or(|allowed| {
        allowed
            .iter()
            .any(|tool| tool == crate::extension::TOOL_NAME)
    });
    let fanout_authorized = effective_builtin_tools
        .iter()
        .any(|tool| tool == crate::extension::TOOL_NAME)
        || (agent.allow_nested_subagents == Some(true)
            && !is_excluded(crate::extension::TOOL_NAME)
            && subagent_within_ceiling);

    // ---- the two RUN-level re-grants: what the RUN grants that the PERSONA did not ----
    //
    // Both append to `effective_builtin_tools` HERE: after `fanout_authorized` is final (just above)
    // and before the `subagent_supervisor` refusal below. `fanout_authorized` is a bound `bool`, so
    // neither push can widen it, and the refusal's `!fanout_authorized` leg is untouched — a ceiling
    // that admitted `subagent_supervisor` but not `subagent` is still refused, exactly as before.

    // [CYRUP-DELTA, deliberate — upstream has this hole and cyrup must not.]
    //
    // `allowNestedSubagents: true` authorizes fanout WITHOUT putting `subagent` in the declared list
    // (the second disjunct above), and `subagent` is an EXTENSION tool
    // (`extension/host/native_impl.rs:48`). Before this task that was invisible, because `--tools`
    // never filtered extension tools; now it does, so such a child would launch authorized to
    // delegate and have the very tool its authorization is about filtered out of its own session.
    //
    // cyrup cannot carry upstream's hole here, because
    // `extension::resolve_registration_mode(child, fanout_authorized)`
    // (`extension/host/registration.rs:41-49`) reads exactly this flag to decide whether to register
    // the child's `subagent` surface at all: an authorized plan that then denies the tool is
    // internally inconsistent.
    if fanout_authorized
        && !effective_builtin_tools
            .iter()
            .any(|tool| tool == crate::extension::TOOL_NAME)
    {
        effective_builtin_tools.push(crate::extension::TOOL_NAME.to_string());
    }

    // pi `internalTools` (`child-tool-plan.ts:456`):
    //
    //   const internalTools = (input.structuredOutput ? ["structured_output"] : [])
    //       .filter((tool) => !excludedToolSet.has(tool));
    //
    // The run's OWN `structured_output` grant when the step declared an `outputSchema`. Nothing in a
    // persona's `tools:` list ever names it — it is granted by the RUN, not the agent — which is
    // exactly why upstream needs a separate allowlist term for it.
    //
    // Ported as of this task. It previously had no counterpart here and needed none, on a premise
    // that no longer holds: `structured_output` is registered by this crate's child-side
    // `prompt_runtime` (`:1837`), so it is an EXTENSION tool, and it used to escape `--tools`
    // entirely. Now that the allowlist is enforced over extension tools, a pinned child with an
    // `outputSchema` would be told to call a tool it no longer has and the run would return no
    // structured output at all.
    //
    // `excludeTools` still wins, exactly as upstream's own `.filter(…)` does: an operator who
    // explicitly excluded `structured_output` gets no structured output, not a silent re-grant.
    //
    // NOT ceiling-filtered, deliberately: upstream's `internalTools` is subtracted only by
    // `excludedToolSet` and never intersected with `allowedToolSet` — a capability ceiling bounds
    // what the PERSONA may ask for, not the run's own capture channel.
    //
    // Bound ONCE and read TWICE, exactly as upstream: `internalTools` is a term of
    // `effectiveToolAllowlist` (`:461`, reached here via the fold into `builtins` below) AND an
    // UNCONDITIONAL term of `requiredChildTools` (`:476`). Restating the condition at the second
    // site would be two copies of one expression — the drift this module exists to prevent. The
    // membership test lives in the loop body rather than the condition because it is a de-dup guard
    // on the PUSH, not part of upstream's `internalTools` expression, whose value the required list
    // needs whole.
    let internal_tools: Vec<String> =
        if structured_output && !is_excluded(crate::prompt_runtime::STRUCTURED_OUTPUT_TOOL_NAME) {
            vec![crate::prompt_runtime::STRUCTURED_OUTPUT_TOOL_NAME.to_string()]
        } else {
            Vec::new()
        };
    for tool in &internal_tools {
        if !effective_builtin_tools.iter().any(|t| t == tool) {
            effective_builtin_tools.push(tool.clone());
        }
    }

    // pi `child-tool-plan.ts:421-423`. Placed exactly here: it reads `fanout_authorized`, so it
    // fires precisely when a child would otherwise launch holding a PARENT-side supervisor tool it
    // has no authority to use. Upstream's own position, too — `:421` sits between `fanoutAuthorized`
    // (`:416-420`) and `toolExtensionPaths`/`mcpResolution` (`:424`/`:431`).
    //
    // Three paths reach it: a ceiling whose `allowedTools` admits `subagent_supervisor` but not
    // `subagent` (`subagent_within_ceiling` is then `false` too, so `allowNestedSubagents` cannot
    // rescue it), an `excludeTools: [subagent]`, and an agent that simply declared the supervisor
    // tool without `subagent`.
    if effective_builtin_tools
        .iter()
        .any(|tool| tool == crate::native_supervisor::NATIVE_SUPERVISOR_TOOL_NAME)
        && !fanout_authorized
    {
        return Err(SubagentError::ToolContractUnsatisfiable(
            "Tool 'subagent_supervisor' requires fanout authorization: include 'subagent' in the \
             effective tools allowlist or enable allowNestedSubagents."
                .to_string(),
        ));
    }

    // SUBA-045: kept as its own binding because it is pi's `toolPlan.effectiveMcpTools`, which has
    // a SECOND consumer besides the `--tools` CSV — `MCP_DIRECT_CHILD_TOOLS_ENV`
    // (`pi-args.ts:618-621`), which is what lets the child's diagnostic distinguish a missing MCP
    // tool ("a host/pi-mcp-adapter registration problem") from a missing extension tool. SUBA-072 /
    // pi `pi-args.ts:457-469`: resolution itself is skipped outright under `denyExtensions` (an MCP
    // server is extension-provided), and whatever survives is then filtered through the same
    // ceiling-membership test as the builtins.
    let mut effective_mcp_tools: Vec<String> = if deny_extensions {
        // pi `child-tool-plan.ts:431-433`: under `denyExtensions` the resolution is skipped
        // OUTRIGHT — upstream substitutes the literal `{ selections: [], unresolvedSelectors: [] }`
        // rather than resolving and then emptying, so the file-backed resolver is never consulted.
        Vec::new()
    } else {
        crate::exec::mcp_direct_tools::resolve_mcp_direct_tool_names_in(
            &mcp_direct_tools,
            cwd,
            dirs,
        )
    };
    if let Some(allowed) = allowed {
        effective_mcp_tools.retain(|tool| allowed.contains(tool));
    }
    // SUBA-092 / pi `pi-args.ts:478`: `.filter((selection) => !excludedToolSet.has(selection.name))`
    // — a resolved direct-MCP name is subject to the same exclusion as a builtin.
    effective_mcp_tools.retain(|tool| !is_excluded(tool));

    // pi `explicitToolAllowlist` (`child-tool-plan.ts:452-455`). Bound here — upstream's own
    // position, immediately after `effectiveMcpTools` (`:449-451`) — rather than inline in the
    // literal below, because `required_child_tools` gates on the SAME value (`:471`), and two
    // spellings of one predicate is exactly the drift `resolve_child_tools` stopped doing when it
    // started reading this field instead of recomputing it.
    let pinned = agent.tools.is_some() || allowed.is_some();

    // pi `requiredChildTools` (`child-tool-plan.ts:466-478`). NOT `effective_tool_allowlist()`
    // minus two names: upstream builds a SECOND list whose first two terms are CONDITIONAL, and
    // only then applies the filter. See the field's doc for why the two lists answer two questions.
    //
    // `legacySupervisorPairing` reads the FULL declared set (`:470`), OUTSIDE the `input.tools`
    // gate — so the pairing is detected even when term 1 contributes nothing.
    let legacy_supervisor_pairing = effective_builtin_tools
        .iter()
        .any(|tool| tool == crate::native_supervisor::CONTACT_SUPERVISOR_TOOL_NAME);
    let required_child_tools: Vec<String> = if pinned {
        let mut terms: Vec<&String> = Vec::new();
        // `:474` — only an agent that WROTE a `tools:` key contributes its builtins, which is
        // exactly `agent.tools.is_some()`: pi's `splitToolList` (`agents/agents.ts:715-728`) emits
        // `tools` — possibly `[]` — whenever the frontmatter had the key, `mcp:`-only lists
        // included, so the two predicates coincide. A surface pinned SOLELY by a ceiling
        // contributes none of it.
        //
        // cyrup's `effective_builtin_tools` is a superset of upstream's
        // `effectiveDeclaredBuiltinTools` by the `allowNestedSubagents` re-grant of `subagent`
        // above — deliberately, and it belongs here too: `fanout_authorized` is the sole input to
        // `extension::resolve_registration_mode`, so an authorized child DOES register `subagent`
        // and the requirement is satisfiable by construction. Do not subtract it back out.
        if agent.tools.is_some() {
            terms.extend(effective_builtin_tools.iter());
        }
        // `:475` — likewise gated on the agent having DECLARED direct-MCP selectors. The RAW
        // selector list is the gate, not the resolved names: upstream tests
        // `input.mcpDirectTools?.length`, so a declared selector that resolved to nothing still
        // opens the term (and contributes nothing through it).
        if !mcp_direct_tools.is_empty() {
            terms.extend(effective_mcp_tools.iter());
        }
        // `:476` — UNCONDITIONAL, and the second read of the binding above. Already present in
        // `effective_builtin_tools` on the `agent.tools.is_some()` arm; `seen` collapses that,
        // exactly as upstream's `[...new Set(...)]` does.
        terms.extend(internal_tools.iter());

        // `:477`'s filter, then `:472`'s de-dup. Upstream filters BEFORE the `Set`; the order is
        // immaterial to the result and this way reads line-for-line against the source.
        let mut seen = std::collections::HashSet::new();
        terms
            .into_iter()
            .filter(|tool| {
                tool.as_str() != crate::native_supervisor::CONTACT_SUPERVISOR_TOOL_NAME
                    && !(legacy_supervisor_pairing
                        && tool.as_str() == crate::native_supervisor::INTERCOM_TOOL_NAME)
            })
            .filter(|tool| seen.insert((*tool).clone()))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    // [CYRUP-DELTA] `codemode` is an ambient built-in, and a child under `--no-extensions` (an agent
    // that pins `extensions:`, or this very ceiling) loads it only when its `--tools` names it
    // (`cyrup-session-svc`'s `child_keeps_codemode`). A ceiling that denies extensions must keep it
    // out, so the name is left off the list the child is launched with. pi-subagents' child
    // session registers the host's codemode factory only when `!capabilityCeiling?.denyExtensions`
    // (`child-session.ts` @HEAD `0c33ec7c`). `required_child_tools` above is deliberately computed
    // BEFORE this subtraction: the agent still declared the tool, so the child's `agent_start`
    // check reports it missing by name instead of the run silently proceeding without it.
    let builtins: Vec<String> = if deny_extensions {
        effective_builtin_tools
            .into_iter()
            .filter(|tool| tool != CODEMODE_TOOL_NAME)
            .collect()
    } else {
        effective_builtin_tools
    };

    Ok(ResolvedToolSurface {
        builtins,
        // G103 / pi `runs/shared/pi-args.ts:389-393` @v0.43.0: `explicitToolAllowlist` is "did
        // anything pin this child's tool surface at all". cyrup folds pi's `tools` and
        // `mcpDirectTools` into the one `agent.tools`, so `is_some()` covers pi's first two terms
        // — `None` is an agent that never wrote a `tools:` key, `Some(_)` INCLUDING `Some(vec![])`
        // is an explicit allowlist. SUBA-072 adds pi's third term: a ceiling with `allowedTools`
        // set pins the surface even when the agent itself never wrote `tools:`.
        //
        // Upstream's THIRD `effectiveToolAllowlist` term, `internalTools`
        // (`child-tool-plan.ts:456` — the run's own `structured_output` grant when the step declared
        // an `outputSchema`), is PARITY, not a divergence: it is folded into `builtins` above,
        // beside the `allowNestedSubagents` re-grant of `subagent`.
        //
        // It was once absent, on a premise that no longer holds. cyrup's `--tools`/`--no-tools`
        // selection (`cyrup-session-svc/src/builder.rs`'s `select_active_tools`, `:381-444`, called
        // at `:1075`) used to run over `registry.visible(...)` alone, with the
        // extension-contributed tools merged in AFTERWARDS and UNFILTERED by
        // `ext_host.active_tools(&base_tools)` (`:1499`). `structured_output` is registered by this
        // crate's own child-side `prompt_runtime` extension (`:1837`), so it was never a candidate
        // for the allowlist filter and survived `--no-tools` intact — where in pi it is a
        // first-class tool the flag WOULD deny, hence pi's explicit re-grant.
        //
        // That gap was pi regression #2835, and closing it (`ext_host.active_tools_filtered`) is
        // what made the term necessary here too: a pinned child with an `outputSchema` would
        // otherwise be told to call a tool the allowlist had just taken away.
        pinned,
        excluded: exclude_tools,
        fanout_authorized,
        effective_mcp_tools,
        required_child_tools,
        // pi `:424-430` deny-filters the PLAN field itself, not just its consumer. cyrup used to
        // collect raw and drop them later inside `push_extension_and_skill_args`; filtering here
        // makes the plan honest (the double filter there is idempotent, and its bool is still
        // load-bearing for `--no-extensions`).
        tool_extension_paths: if deny_extensions {
            Vec::new()
        } else {
            tool_extension_paths
        },
        // RAW — deliberately NOT deny-filtered, and NOT ceiling-filtered. Upstream never stores it
        // filtered either (`pi-args.ts:916-926` reads `input.mcpDirectTools` raw on the no-ceiling
        // arm), and `spawn_plan::env_identity_and_depth` reproduces that whole three-way branch
        // itself — including its own `denyExtensions` arm and its own PER-SELECTOR ceiling test.
        // Emptying it here would be behaviour-neutral today (both roads end at the `__none__`
        // sentinel) but it would make this field's doc a lie the day that branch changes.
        mcp_direct_tools,
    })
}

// ================================================================================================
// The message
// ================================================================================================

/// The non-inheritance rule, stated once. This paragraph is the actual deliverable of the whole
/// change: a parent that reads it must not need to open the source to understand what happened.
pub const NON_INHERITANCE_NOTICE: &str = "Subagent tools are NOT inherited from the parent \
     session. A child's surface is pinned by the agent definition's own `tools:` list, narrowed by \
     its `excludeTools` and by any capability ceiling — the launching session's tools are never \
     consulted, and `context: \"fresh\"` / `\"fork\"` does not change this.";

/// How to inspect a surface before writing a prompt that assumes one.
pub const INSPECT_HINT: &str = "Inspect any agent's surface before you write a prompt: \
     subagent({ action: \"get\", agent: \"<name>\" }).";

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::discovery::types::{AgentSource, ToolRef};
    use crate::spawn::depth::DepthEnvelope;

    // ---- the pre-`structured_output` arity, preserved for the cases that predate it ----
    //
    // A locally-defined item SHADOWS a glob import, so these two retarget every existing case in
    // this module without touching it. None of them is about the run's own `structured_output`
    // grant, so they all pass `false`; the cases that ARE about it call `super::resolve_tool_surface`
    // explicitly. Same test-shim pattern as `cyrup-session-svc`'s `builder.rs` `fn selected(…)`.

    fn resolve_tool_surface(
        agent: &AgentConfig,
        require_read_tool: bool,
        ceiling: Option<&crate::exec::capability_ceiling::ResolvedCapabilityCeiling>,
        cwd: &std::path::Path,
    ) -> Result<ResolvedToolSurface, SubagentError> {
        super::resolve_tool_surface(agent, require_read_tool, ceiling, false, cwd)
    }

    fn resolve_tool_surface_in(
        agent: &AgentConfig,
        require_read_tool: bool,
        ceiling: Option<&crate::exec::capability_ceiling::ResolvedCapabilityCeiling>,
        cwd: &std::path::Path,
        dirs: &crate::exec::mcp_direct_tools::McpDirs,
    ) -> Result<ResolvedToolSurface, SubagentError> {
        super::resolve_tool_surface_in(agent, require_read_tool, ceiling, false, cwd, dirs)
    }

    /// The crate's house pattern for building an execution-ready persona in a test: parse real
    /// frontmatter, then project it exactly as the run path does (`spawn_plan.rs:2823-2828`). No
    /// hand-built `AgentConfig` literal, so a discovery-side change to how `tools:` is parsed
    /// reaches these tests instead of being papered over.
    fn agent_from_frontmatter(content: &str) -> AgentConfig {
        let def = crate::discovery::frontmatter::parse_agent_file(
            content,
            AgentSource::User,
            std::path::Path::new("t.md"),
        )
        .expect("agent file parses");
        AgentConfig::from_agent_definition(
            &def,
            DepthEnvelope {
                current_depth: 0,
                max_depth: 5,
            },
        )
    }

    fn pinned(tools: &[&str]) -> ResolvedToolSurface {
        ResolvedToolSurface {
            builtins: tools.iter().map(|t| (*t).to_string()).collect(),
            pinned: true,
            excluded: Vec::new(),
            fanout_authorized: false,
            effective_mcp_tools: Vec::new(),
            // Independent of `builtins` on purpose: this helper's callers are wire-shape and
            // allowlist assertions, and an empty required list keeps a clean surface's payload
            // byte-identical (`skip_serializing_if = "Vec::is_empty"`).
            required_child_tools: Vec::new(),
            tool_extension_paths: Vec::new(),
            mcp_direct_tools: Vec::new(),
        }
    }

    /// No `mcp:` selectors means the resolver short-circuits before it touches a file
    /// (`mcp_direct_tools.rs:488-490`), so every pre-existing test can hand it a path that does not
    /// exist. Named rather than repeated 19 times so the claim is stated once.
    fn no_cwd() -> &'static std::path::Path {
        std::path::Path::new("/nonexistent")
    }

    /// A `ResolvedCapabilityCeiling` for tests. `allowed` is `None` for "no tool bound" and
    /// `Some(&[])` for "bound to nothing" — the distinction the real type exists to preserve.
    ///
    /// A helper rather than fifteen literals because the type has NO `Default` and no constructor
    /// (no `impl` block at all), so every field is spelled out at every use site otherwise.
    fn ceiling(
        allowed: Option<&[&str]>,
        sources: &[&str],
    ) -> crate::exec::capability_ceiling::ResolvedCapabilityCeiling {
        crate::exec::capability_ceiling::ResolvedCapabilityCeiling {
            version: crate::exec::capability_ceiling::CAPABILITY_CEILING_VERSION,
            allowed_tools: allowed.map(|a| a.iter().map(|s| (*s).to_string()).collect()),
            allowed_agents: None,
            deny_extensions: false,
            sources: sources.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    fn reviewer() -> ResolvedToolSurface {
        pinned(&["read", "grep", "find", "ls", "intercom"])
    }

    // ---- resolve_tool_surface(): parity with resolve_child_tools ----

    /// `agent_with` with an explicit NAME — `worker`, `scout` and `reviewer` are what SUBA-114's
    /// cases are about.
    fn agent_named(name: &str, tools: Option<&str>) -> AgentConfig {
        let tools_line = tools.map_or_else(String::new, |t| format!("tools: {t}\n"));
        agent_from_frontmatter(&format!(
            "---\nname: {name}\ndescription: Review things\n{tools_line}---\n\nBody.\n"
        ))
    }

    /// `Some(list)` writes a real `tools:` line; `None` omits the key entirely (the unpinned arm).
    fn agent_with(tools: Option<&str>) -> AgentConfig {
        agent_named("reviewer", tools)
    }

    #[test]
    fn resolve_reads_the_agents_own_declaration_and_nothing_else() {
        let agent = agent_with(Some("read, grep, find, ls, intercom"));
        let surface = resolve_tool_surface(&agent, false, None, no_cwd()).expect("resolves");
        assert!(surface.pinned);
        assert_eq!(surface.builtins, ["read", "grep", "find", "ls", "intercom"]);
    }

    #[test]
    fn resolve_leaves_an_agent_without_a_tools_key_unpinned() {
        let surface =
            resolve_tool_surface(&agent_with(None), false, None, no_cwd()).expect("resolves");
        assert!(!surface.pinned);
        assert!(surface.builtins.is_empty());
    }

    /// SUBA-014: the `read` head-injection, preserved by the hoist.
    #[test]
    fn resolve_head_injects_read_when_a_skill_required_it() {
        let agent = agent_with(Some("bash, edit"));
        let surface = resolve_tool_surface(&agent, true, None, no_cwd()).expect("resolves");
        assert_eq!(surface.builtins, ["read", "bash", "edit"]);
    }

    /// SUBA-072: a ceiling narrows, never widens, and suppresses the head-injection.
    #[test]
    fn resolve_narrows_to_a_capability_ceiling() {
        let agent = agent_with(Some("read, bash, edit"));
        let c = ceiling(Some(&["read", "grep"]), &[]);
        let surface = resolve_tool_surface(&agent, true, Some(&c), no_cwd()).expect("resolves");
        assert_eq!(surface.builtins, ["read"]);
    }

    /// SUBA-072: a ceiling alone pins the surface of an agent that declared no `tools:`.
    #[test]
    fn resolve_is_pinned_by_a_ceiling_alone() {
        let c = ceiling(Some(&["read"]), &[]);
        let surface =
            resolve_tool_surface(&agent_with(None), false, Some(&c), no_cwd()).expect("resolves");
        assert!(surface.pinned);
        assert_eq!(surface.builtins, ["read"]);
    }

    /// SUBA-092: `excludeTools` subtracts from an explicit allowlist.
    #[test]
    fn resolve_subtracts_exclude_tools() {
        let mut agent = agent_with(Some("read, bash"));
        agent.exclude_tools = vec!["  bash  ".to_string(), "bash".to_string(), String::new()];
        let surface = resolve_tool_surface(&agent, false, None, no_cwd()).expect("resolves");
        assert_eq!(surface.builtins, ["read"]);
        assert_eq!(surface.excluded, ["bash"], "trimmed and de-duplicated");
    }

    /// SUBA-092: the fanout disjunction survives the hoist intact.
    #[test]
    fn resolve_carries_fanout_authorization() {
        let mut declared = agent_with(Some(&format!("read, {}", crate::extension::TOOL_NAME)));
        assert!(
            resolve_tool_surface(&declared, false, None, no_cwd())
                .expect("resolves")
                .fanout_authorized
        );

        declared.exclude_tools = vec![crate::extension::TOOL_NAME.to_string()];
        assert!(
            !resolve_tool_surface(&declared, false, None, no_cwd())
                .expect("resolves")
                .fanout_authorized
        );

        let mut granted = agent_with(Some("read"));
        granted.allow_nested_subagents = Some(true);
        assert!(
            resolve_tool_surface(&granted, false, None, no_cwd())
                .expect("resolves")
                .fanout_authorized
        );

        let c = ceiling(Some(&["read"]), &[]);
        assert!(
            !resolve_tool_surface(&granted, false, Some(&c), no_cwd())
                .expect("resolves")
                .fanout_authorized
        );
    }

    // ---- SUBA-114: the child launches with what its agent declares (pi `b12496b8`) ----

    /// The declared built-ins that a `--tools subagent,read` dispatcher's own registry would have
    /// reported "missing" before SUBA-114 — every one of them now reaches the plan.
    fn worker_tools() -> Vec<String> {
        ["read", "bash", "edit"]
            .iter()
            .map(|t| (*t).to_string())
            .collect()
    }

    /// Upstream's replacement case, *"keeps every declared core tool, so the child registry
    /// decides what exists"* (`test/unit/child-tool-plan.test.ts` @v0.71.0). The resolver has no
    /// parent-registry input at all any more, so nothing a parent session was started with can
    /// narrow this.
    #[test]
    fn every_declared_core_tool_is_kept_so_the_child_registry_decides() {
        let agent = agent_named("worker", Some("read, bash, edit"));
        let surface = resolve_tool_surface(&agent, false, None, no_cwd()).expect("resolves");
        assert_eq!(surface.builtins, worker_tools());
        assert_eq!(surface.effective_tool_allowlist(), worker_tools());
        assert_eq!(
            surface.required_child_tools,
            worker_tools(),
            "every declared tool is REQUIRED of the child's own registry, which is where a \
             genuinely missing one is refused"
        );
    }

    /// Upstream's *"production launch path keeps declared child tools"* runs the same plan for
    /// `test-agent` AND `scout`: the review/scout lane check is gone, so a lane agent resolves
    /// exactly like any other. `reviewer` is the second lane name the deleted refusal matched.
    #[test]
    fn review_and_scout_lanes_resolve_like_any_other_agent() {
        let repository_tools: Vec<String> = ["read", "grep", "find", "ls", "bash"]
            .iter()
            .map(|t| (*t).to_string())
            .collect();
        for name in ["worker", "scout", "reviewer", "code-reviewer"] {
            let agent = agent_named(name, Some("read, grep, find, ls, bash"));
            let surface = resolve_tool_surface(&agent, false, None, no_cwd())
                .unwrap_or_else(|err| panic!("{name} must launch: {err}"));
            assert_eq!(surface.builtins, repository_tools, "{name}");
            assert_eq!(surface.required_child_tools, repository_tools, "{name}");
        }
    }

    /// Upstream's *"respects a capability ceiling"* (the rewrite of *"respects both capability
    /// ceiling and host availability"*): the ceiling is still the one narrowing input besides
    /// `excludeTools`, and an EMPTIED ceiling allowlist on a lane agent is simply an empty surface.
    #[test]
    fn a_capability_ceiling_is_still_honoured() {
        let agent = agent_named("worker", Some("read, grep, bash, write"));
        let c = ceiling(Some(&["read", "bash"]), &["test"]);
        let surface = resolve_tool_surface(&agent, false, Some(&c), no_cwd()).expect("resolves");
        assert_eq!(surface.builtins, ["read", "bash"]);

        let empty = ceiling(Some(&[]), &["plan-mode"]);
        let surface = resolve_tool_surface(
            &agent_with(Some("read, grep")),
            false,
            Some(&empty),
            no_cwd(),
        )
        .expect("an emptied ceiling allowlist is not a refusal");
        assert!(surface.builtins.is_empty());
        assert!(surface.required_child_tools.is_empty());
    }

    /// An intentionally empty review allowlist (`Some(vec![])`, the shape `"tools": false` merges
    /// to) is pinned-and-empty: `--no-tools`, nothing required.
    #[test]
    fn an_intentionally_empty_review_allowlist_launches_with_no_tools() {
        let mut agent = agent_named("scout", None);
        agent.tools = Some(Vec::new());
        let surface = resolve_tool_surface(&agent, false, None, no_cwd()).expect("resolves");
        assert!(surface.pinned);
        assert!(surface.builtins.is_empty());
        assert!(surface.required_child_tools.is_empty());
    }

    /// A `toolSurface` written by a parent from before SUBA-114 carried `unavailableHostBuiltins`
    /// and `warnings`. Neither field exists now, and the type does not deny unknown fields, so such
    /// a payload must still decode — to the same surface minus the two removed keys.
    #[test]
    fn a_payload_carrying_the_removed_host_fields_still_decodes() {
        let mut json = serde_json::to_value(reviewer()).expect("serializes");
        let object = json.as_object_mut().expect("an object");
        object.insert(
            "unavailableHostBuiltins".to_string(),
            serde_json::json!(["bash"]),
        );
        object.insert(
            "warnings".to_string(),
            serde_json::json!(["Agent 'reviewer': host runtime tool availability omitted [bash]."]),
        );
        let decoded: ResolvedToolSurface =
            serde_json::from_value(json).expect("an old payload still decodes");
        assert_eq!(decoded, reviewer());
        let back = serde_json::to_string(&decoded).expect("serializes");
        assert!(!back.contains("unavailableHostBuiltins"), "{back}");
        assert!(!back.contains("\"warnings\""), "{back}");
    }

    // ---- the `read` throw ----

    /// The moved throw, proven behaviour-preserving at resolver level. `spawn_plan.rs`'s
    /// `a_capability_ceiling_excluding_read_fails_the_launch_when_read_is_required` proves the same
    /// message still reaches the operator through the real plan path.
    #[test]
    fn the_ceiling_read_throw_still_fires_from_the_resolver() {
        let agent = agent_with(Some("grep"));
        let c = ceiling(Some(&["grep"]), &["org-policy"]);
        let err = resolve_tool_surface(&agent, true, Some(&c), no_cwd())
            .expect_err("a ceiling excluding `read` must refuse a launch that requires it");
        assert!(
            matches!(err, SubagentError::CapabilityCeilingViolation(_)),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "Capability ceiling from org-policy excludes required tool 'read' for lazy skill \
             loading."
        );
    }

    /// Upstream's `capabilityCeiling?.sources.join(", ") || "unknown source"` — the `||` fires both
    /// when the ceiling is absent AND when `join` yields `""`. Pins the fallback the move carried.
    #[test]
    fn an_empty_sources_ceiling_says_unknown_source() {
        let agent = agent_with(Some("grep"));
        let c = ceiling(Some(&["grep"]), &[]);
        let err = resolve_tool_surface(&agent, true, Some(&c), no_cwd()).expect_err("must refuse");
        assert_eq!(
            err.to_string(),
            "Capability ceiling from unknown source excludes required tool 'read' for lazy skill \
             loading."
        );
    }

    // ---- the folded direct-MCP half (pi `child-tool-plan.ts:431-451`) ----

    /// The hyphen-free server, deliberately. `sanitize_server_prefix`
    /// (`mcp_direct_tools.rs:1230-1247`) PRESERVES `-` — that is MCP-370 — so `chrome-devtools`
    /// resolves to `chrome-devtools_take_screenshot`, not `chrome_devtools_…`. Using `github` keeps
    /// these assertions unable to re-encode the bug either way.
    fn github_fixture() -> crate::exec::testsupport::McpFixture {
        let fixture = crate::exec::testsupport::make_fixture();
        crate::exec::testsupport::write_mcp_fixture(
            &fixture,
            "github",
            serde_json::json!({ "command": "github-mcp" }),
            None,
            vec!["search_repositories", "create_issue"],
            vec![],
            None,
            None,
        );
        fixture
    }

    /// An `McpDirs` whose every field points inside a directory that does not exist — proof that a
    /// code path never consulted the file-backed resolver at all.
    fn unreachable_dirs() -> crate::exec::mcp_direct_tools::McpDirs {
        let root = std::path::Path::new("/nonexistent/flux-toolcon-4");
        crate::exec::mcp_direct_tools::McpDirs {
            agent_dir: root.join("agent"),
            generic_global_config_path: root.join("config").join("mcp.json"),
            home: root.to_path_buf(),
        }
    }

    /// The whole point of the fold: `effective_mcp_tools` is resolved by the SAME function that
    /// produces `builtins`, so `effective_tool_allowlist()` — which is what reaches `--tools` — can
    /// name both halves. Before it, the resolver returned builtins only and `action: "list"`
    /// under-reported an `mcp:`-declaring agent's real surface.
    #[test]
    fn the_resolver_returns_resolved_mcp_names() {
        let fixture = github_fixture();
        let agent = agent_with(Some("read, mcp:github/search_repositories"));
        let surface =
            resolve_tool_surface_in(&agent, false, None, &fixture.project_dir, &fixture.dirs)
                .expect("resolves");
        assert_eq!(
            surface.effective_mcp_tools,
            vec!["github_search_repositories".to_string()]
        );
        assert_eq!(
            surface.effective_tool_allowlist(),
            vec!["read".to_string(), "github_search_repositories".to_string()]
        );
        // The RAW selector survives untouched for the `MCP_DIRECT_TOOLS` env, which speaks selectors.
        assert_eq!(
            surface.mcp_direct_tools,
            vec!["github/search_repositories".to_string()]
        );
        // `mcp:` never reaches the builtin half (the T4 bug).
        assert_eq!(surface.builtins, vec!["read".to_string()]);
    }

    /// pi `child-tool-plan.ts:431-433` substitutes the literal `{ selections: [] }` under
    /// `denyExtensions` rather than resolving and then emptying — an MCP server is
    /// extension-provided. The unreachable `McpDirs` is the proof: if the resolver were consulted at
    /// all this would still pass, so the assertion is paired with a layout that COULD have resolved
    /// (`the_resolver_returns_resolved_mcp_names`, same agent, same selector).
    #[test]
    fn mcp_resolution_is_skipped_under_deny_extensions() {
        let agent = agent_with(Some("read, mcp:github/search_repositories"));
        let c = crate::exec::capability_ceiling::ResolvedCapabilityCeiling {
            deny_extensions: true,
            ..ceiling(None, &["test"])
        };
        let surface = resolve_tool_surface_in(
            &agent,
            false,
            Some(&c),
            std::path::Path::new("/nonexistent"),
            &unreachable_dirs(),
        )
        .expect("resolves");
        assert!(surface.effective_mcp_tools.is_empty());
        assert_eq!(surface.effective_tool_allowlist(), vec!["read".to_string()]);
    }

    /// SUBA-072 / pi `:443-447`: a resolved direct-MCP NAME narrows against the same `allowedTools`
    /// set the builtins do. Pre-fold this filter lived in `spawn_plan`, one function away from the
    /// surface that published the result.
    #[test]
    fn a_ceiling_narrows_resolved_mcp_names() {
        let fixture = github_fixture();
        let agent = agent_with(Some("read, mcp:github/search_repositories"));
        let c = ceiling(Some(&["read"]), &["org-policy"]);
        let surface =
            resolve_tool_surface_in(&agent, false, Some(&c), &fixture.project_dir, &fixture.dirs)
                .expect("resolves");
        assert!(surface.effective_mcp_tools.is_empty());
        assert_eq!(surface.effective_tool_allowlist(), vec!["read".to_string()]);
    }

    /// SUBA-092 / pi `:448`: `excludeTools` subtracts a resolved direct-MCP name exactly as it
    /// subtracts a builtin.
    #[test]
    fn exclude_tools_subtracts_a_resolved_mcp_name() {
        let fixture = github_fixture();
        let agent = agent_from_frontmatter(
            "---\nname: reviewer\ndescription: Review things\ntools: read, \
             mcp:github/search_repositories\nexcludeTools: github_search_repositories\n---\n\nBody.\n",
        );
        let surface =
            resolve_tool_surface_in(&agent, false, None, &fixture.project_dir, &fixture.dirs)
                .expect("resolves");
        assert!(surface.effective_mcp_tools.is_empty());
        assert_eq!(surface.effective_tool_allowlist(), vec!["read".to_string()]);
    }

    /// The whole justification for folding the resolution into the DISPLAY path too: an agent with
    /// no `mcp:` selectors short-circuits at `mcp_direct_tools.rs:488-490`, BEFORE `load_mcp_config`
    /// and `load_metadata_cache`, so a non-existent cwd and an unreachable `McpDirs` still resolve.
    /// `action: "list"` therefore costs zero syscalls for every bundled persona.
    #[test]
    fn no_mcp_selectors_touches_no_files() {
        let agent = agent_with(Some("read, grep"));
        let surface = resolve_tool_surface_in(
            &agent,
            false,
            None,
            std::path::Path::new("/nonexistent"),
            &unreachable_dirs(),
        )
        .expect("resolves without touching the filesystem");
        assert!(surface.effective_mcp_tools.is_empty());
        assert!(surface.mcp_direct_tools.is_empty());
        assert_eq!(
            surface.effective_tool_allowlist(),
            vec!["read".to_string(), "grep".to_string()]
        );
    }

    /// pi `:457-463` is `[...new Set([...])]`. cyrup's pre-fold `allowlist.extend(...)` did not
    /// dedup, so declaring a resolved MCP name as a builtin too emitted it TWICE in the `--tools`
    /// CSV. Pathological, but the CSV is a contract.
    #[test]
    fn the_allowlist_de_duplicates_a_builtin_and_mcp_name_collision() {
        let fixture = github_fixture();
        let agent = agent_with(Some(
            "github_search_repositories, mcp:github/search_repositories",
        ));
        let surface =
            resolve_tool_surface_in(&agent, false, None, &fixture.project_dir, &fixture.dirs)
                .expect("resolves");
        assert_eq!(
            surface.builtins,
            vec!["github_search_repositories".to_string()]
        );
        assert_eq!(
            surface.effective_mcp_tools,
            vec!["github_search_repositories".to_string()]
        );
        assert_eq!(
            surface.effective_tool_allowlist(),
            vec!["github_search_repositories".to_string()],
            "upstream's `new Set` collapses the collision to ONE entry"
        );
    }

    /// pi `:424-430` deny-filters `toolExtensionPaths` on the PLAN, not just at its consumer.
    ///
    /// Built as an `AgentConfig` field rather than from frontmatter ON PURPOSE:
    /// `discovery::frontmatter::parse_tool_refs` and [`ToolRef::from_tool_string`] are both two-arm
    /// matches over the `mcp:` prefix and can NEVER emit a `ToolRef::ExtensionPath` — the only
    /// production producer is that type's adjacently-tagged `Deserialize` (a `RunnerConfig`
    /// round-trip). `agent_with()` cannot express this case at all.
    #[test]
    fn deny_extensions_empties_tool_extension_paths() {
        let mut agent = agent_with(Some("read"));
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::ExtensionPath("./custom-tool.ts".to_string()),
        ]);

        let open = resolve_tool_surface_in(&agent, false, None, no_cwd(), &unreachable_dirs())
            .expect("resolves");
        assert_eq!(
            open.tool_extension_paths,
            vec!["./custom-tool.ts".to_string()]
        );
        // The extension path is NOT a builtin and never reaches the `--tools` CSV.
        assert_eq!(open.builtins, vec!["read".to_string()]);
        assert_eq!(open.effective_tool_allowlist(), vec!["read".to_string()]);

        let c = crate::exec::capability_ceiling::ResolvedCapabilityCeiling {
            deny_extensions: true,
            ..ceiling(None, &["test"])
        };
        let denied =
            resolve_tool_surface_in(&agent, false, Some(&c), no_cwd(), &unreachable_dirs())
                .expect("resolves");
        assert!(
            denied.tool_extension_paths.is_empty(),
            "pi empties the PLAN field under `denyExtensions`, not just the argv it feeds"
        );
    }

    /// A subagent's `tools:` may name `codemode`; the child keeps it across `--no-extensions` when
    /// the list reaches it. A ceiling that denies extensions must keep it out of that list, and the
    /// agent's own declaration still makes the child require it, so the child names the tool as
    /// missing at `agent_start` instead of dropping it without a word.
    #[test]
    fn deny_extensions_keeps_codemode_out_of_the_launch_list_but_in_the_required_list() {
        let mut agent = agent_with(Some("read"));
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::Builtin("codemode".to_string()),
        ]);

        let open = resolve_tool_surface_in(&agent, false, None, no_cwd(), &unreachable_dirs())
            .expect("resolves");
        assert_eq!(
            open.builtins,
            vec!["read".to_string(), "codemode".to_string()]
        );

        let c = crate::exec::capability_ceiling::ResolvedCapabilityCeiling {
            deny_extensions: true,
            ..ceiling(None, &["test"])
        };
        let denied =
            resolve_tool_surface_in(&agent, false, Some(&c), no_cwd(), &unreachable_dirs())
                .expect("resolves");
        assert_eq!(denied.builtins, vec!["read".to_string()]);
        assert_eq!(
            denied.required_child_tools,
            vec!["read".to_string(), "codemode".to_string()]
        );
    }

    /// The other half of the asymmetry, and the one a "tidy-up" would erase: `mcp_direct_tools` is
    /// RAW. Upstream never stores it filtered (`pi-args.ts:916-926` reads `input.mcpDirectTools`
    /// raw), and `spawn_plan::env_identity_and_depth` reproduces the whole three-way branch itself
    /// — including its own `denyExtensions` arm and its own PER-SELECTOR ceiling test, which needs
    /// the selectors, not the names.
    #[test]
    fn mcp_direct_tools_stays_raw_under_deny_extensions() {
        let agent = agent_with(Some("read, mcp:github/search_repositories"));
        let c = crate::exec::capability_ceiling::ResolvedCapabilityCeiling {
            deny_extensions: true,
            ..ceiling(None, &["test"])
        };
        let surface =
            resolve_tool_surface_in(&agent, false, Some(&c), no_cwd(), &unreachable_dirs())
                .expect("resolves");
        assert_eq!(
            surface.mcp_direct_tools,
            vec!["github/search_repositories".to_string()],
            "the raw selector list is NOT deny-filtered — `MCP_DIRECT_TOOLS` owns that decision"
        );
        assert!(surface.effective_mcp_tools.is_empty());
    }

    // ---- the `subagent_supervisor` fanout refusal (pi `child-tool-plan.ts:421-423`) ----

    /// Upstream's message, asserted in FULL rather than by substring: the two-clause remedy is the
    /// operator-facing half and a truncation of it is a silent regression.
    const FANOUT_REFUSAL: &str = "Tool 'subagent_supervisor' requires fanout authorization: \
                                  include 'subagent' in the effective tools allowlist or enable \
                                  allowNestedSubagents.";

    #[test]
    fn subagent_supervisor_without_fanout_is_refused() {
        let agent = agent_with(Some("read, subagent_supervisor"));
        let err = resolve_tool_surface(&agent, false, None, no_cwd()).expect_err(
            "a child holding the PARENT-side supervisor tool with no children must be refused",
        );
        assert_eq!(err.to_string(), FANOUT_REFUSAL);
    }

    /// Both grants clear it: naming `subagent` in `tools:`, and `allowNestedSubagents: true` on its
    /// own (pi's independent grant, `:417-419`).
    #[test]
    fn subagent_supervisor_with_fanout_launches() {
        let declared = agent_with(Some("read, subagent, subagent_supervisor"));
        let surface = resolve_tool_surface(&declared, false, None, no_cwd()).expect("resolves");
        assert!(surface.fanout_authorized);

        let granted = agent_from_frontmatter(
            "---\nname: reviewer\ndescription: Review things\ntools: read, \
             subagent_supervisor\nallowNestedSubagents: true\n---\n\nBody.\n",
        );
        let surface = resolve_tool_surface(&granted, false, None, no_cwd())
            .expect("allowNestedSubagents is an independent grant");
        assert!(surface.fanout_authorized);
    }

    /// **Reachability path 1.** The ceiling `retain` is NOT exemption-guarded, so a ceiling that
    /// admits `subagent_supervisor` but not `subagent` revokes fanout while leaving the supervisor
    /// tool in place. `subagent_within_ceiling` is `false` at the same time, so
    /// `allowNestedSubagents` cannot rescue it — asserted as the second half.
    #[test]
    fn a_ceiling_that_omits_subagent_revokes_the_supervisor_tool() {
        let agent = agent_with(Some("read, subagent, subagent_supervisor"));
        let c = ceiling(Some(&["read", "subagent_supervisor"]), &["test"]);
        let err = resolve_tool_surface(&agent, false, Some(&c), no_cwd())
            .expect_err("the ceiling dropped `subagent`, so fanout is revoked");
        assert_eq!(err.to_string(), FANOUT_REFUSAL);

        let rescued = agent_from_frontmatter(
            "---\nname: reviewer\ndescription: Review things\ntools: read, subagent, \
             subagent_supervisor\nallowNestedSubagents: true\n---\n\nBody.\n",
        );
        let err = resolve_tool_surface(&rescued, false, Some(&c), no_cwd()).expect_err(
            "`allowNestedSubagents` is subordinate to the ceiling (pi `:419`), so it cannot rescue \
             this",
        );
        assert_eq!(err.to_string(), FANOUT_REFUSAL);
    }

    /// **Reachability path 2.** `excludeTools: [subagent]` removes `subagent` from
    /// `effective_builtin_tools`, and `fanout_authorized`'s second disjunct re-checks
    /// `!is_excluded(subagent)` explicitly.
    #[test]
    fn exclude_tools_on_subagent_revokes_the_supervisor_tool() {
        let agent = agent_from_frontmatter(
            "---\nname: reviewer\ndescription: Review things\ntools: read, subagent, \
             subagent_supervisor\nexcludeTools: subagent\n---\n\nBody.\n",
        );
        let err = resolve_tool_surface(&agent, false, None, no_cwd())
            .expect_err("`excludeTools: [subagent]` revokes the grant `tools:` conferred");
        assert_eq!(err.to_string(), FANOUT_REFUSAL);
    }

    /// The two coordination tools are ASYMMETRIC and the refusal must guard only one.
    /// `subagent_supervisor` is the PARENT-side tool (answering blocked children);
    /// [`crate::native_supervisor::INTERCOM_TOOL_NAME`] is the CHILD->supervisor direction, which
    /// every child needs regardless of fanout. Guarding `intercom` symmetrically would refuse the
    /// launch of the four bundled personas that declare it, on every unfanned-out run.
    #[test]
    fn intercom_is_never_guarded_by_the_fanout_throw() {
        let agent = agent_with(Some("read, intercom"));
        let surface = resolve_tool_surface(&agent, false, None, no_cwd())
            .expect("`intercom` is not a fanout-gated capability");
        assert!(!surface.fanout_authorized);
        assert!(surface.builtins.iter().any(|t| t == "intercom"));
    }

    // ---- the wire shape ----

    #[test]
    fn the_default_surface_is_omitted_from_the_wire() {
        assert!(is_default_surface(&ResolvedToolSurface::default()));
        assert!(!is_default_surface(&reviewer()));
        let json = serde_json::to_string(&reviewer()).unwrap();
        assert!(json.contains("\"builtins\""), "{json}");
        assert!(json.contains("\"pinned\":true"), "{json}");
        assert!(!json.contains("\"excluded\""), "{json}");
        // Same for the folded direct-MCP names; the two argv/env fields carry `#[serde(skip)]` and
        // so can never appear at all, populated or not.
        assert!(!json.contains("effectiveMcpTools"), "{json}");
        assert!(!json.contains("toolExtensionPaths"), "{json}");
        assert!(!json.contains("mcpDirectTools"), "{json}");
        let loaded = ResolvedToolSurface {
            effective_mcp_tools: vec!["github_search_repositories".to_string()],
            tool_extension_paths: vec!["./custom-tool.ts".to_string()],
            mcp_direct_tools: vec!["github/search_repositories".to_string()],
            ..pinned(&["read"])
        };
        let json = serde_json::to_string(&loaded).unwrap();
        assert!(
            json.contains("\"effectiveMcpTools\":[\"github_search_repositories\"]"),
            "{json}"
        );
        assert!(!json.contains("toolExtensionPaths"), "{json}");
        assert!(!json.contains("mcpDirectTools"), "{json}");
    }

    /// The parent's actual read path: `render_single_result` serializes a `SubagentUpdatePayload`
    /// whose `results: vec![result]` embeds the whole [`crate::exec::SingleResult`]
    /// (`tui/events.rs`'s `single_final`), so the surface reaches a caller as
    /// `details.results[0].toolSurface`. This pins the wire name and the omit-when-absent behaviour
    /// that keeps a clean result byte-for-byte what it was before the field existed.
    #[test]
    fn the_surface_reaches_the_parent_under_its_camel_case_wire_name() {
        // A pre-change result payload: the key is not present, and it must still decode.
        let legacy = serde_json::json!({
            "agent": "reviewer",
            "task": "",
            "exitCode": 0,
            "usage": cyrup_core::Usage::default(),
            "model": null,
            "attemptedModels": [],
            "modelAttempts": [],
            "finalOutput": null,
            "structuredOutput": null,
            "acceptance": null,
            "detached": false,
            "interrupted": false,
            "timedOut": false,
            "error": null,
            "toolCalls": [],
            "outputTruncated": false,
            "controlEvents": [],
            "progress": null,
        });
        let mut result: crate::exec::SingleResult =
            serde_json::from_value(legacy).expect("a pre-change result payload still decodes");
        assert_eq!(result.tool_surface, ResolvedToolSurface::default());

        // A clean run adds no key at all — no noise on the hot path.
        let clean = serde_json::to_value(&result).expect("serializes");
        assert!(clean.get("toolSurface").is_none(), "{clean}");

        // A settled native run publishes it.
        result.tool_surface = reviewer();
        let published = serde_json::to_value(&result).expect("serializes");
        assert_eq!(
            published["toolSurface"]["builtins"],
            serde_json::json!(["read", "grep", "find", "ls", "intercom"])
        );
        assert_eq!(published["toolSurface"]["pinned"], serde_json::json!(true));

        let back: crate::exec::SingleResult =
            serde_json::from_value(published).expect("round-trips");
        assert_eq!(back, result);
    }

    // ---- the RUN-level re-grants (#2835 fallout: what `--tools` revokes once it is enforced) ----

    /// B2. `allowNestedSubagents: true` authorizes fanout WITHOUT the persona ever naming
    /// `subagent`, and `subagent` is an EXTENSION tool — so once `--tools` is enforced over
    /// extension tools, an authorized child would have the very tool its authorization is about
    /// filtered out of its own session. The allowlist must carry it.
    #[test]
    fn a_fanout_authorized_child_keeps_the_subagent_tool() {
        let agent = agent_from_frontmatter(
            "---\nname: reviewer\ndescription: Review things\ntools: read\n\
             allowNestedSubagents: true\n---\n\nBody.\n",
        );
        let surface = resolve_tool_surface(&agent, false, None, no_cwd()).expect("resolves");

        assert!(surface.fanout_authorized);
        assert!(
            surface
                .effective_tool_allowlist()
                .iter()
                .any(|t| t == crate::extension::TOOL_NAME),
            "a fanout-authorized child must carry `subagent` in the allowlist that becomes its \
             `--tools` CSV, or `resolve_registration_mode` registers a surface the session then \
             filters away; got {:?}",
            surface.effective_tool_allowlist()
        );
    }

    /// The re-grant must not manufacture authorization it was not given: an unauthorized agent
    /// gains nothing, and `excludeTools: [subagent]` still revokes the `allowNestedSubagents` grant
    /// (the `!is_excluded` leg of `fanout_authorized`), so nothing is pushed.
    #[test]
    fn the_subagent_regrant_does_not_invent_authorization() {
        let plain = agent_with(Some("read"));
        let surface = resolve_tool_surface(&plain, false, None, no_cwd()).expect("resolves");
        assert!(!surface.fanout_authorized);
        assert!(
            !surface
                .effective_tool_allowlist()
                .iter()
                .any(|t| t == crate::extension::TOOL_NAME)
        );

        let mut excluded = agent_from_frontmatter(
            "---\nname: reviewer\ndescription: Review things\ntools: read\n\
             allowNestedSubagents: true\n---\n\nBody.\n",
        );
        excluded.exclude_tools = vec![crate::extension::TOOL_NAME.to_string()];
        let surface = resolve_tool_surface(&excluded, false, None, no_cwd()).expect("resolves");
        assert!(!surface.fanout_authorized);
        assert!(
            !surface
                .effective_tool_allowlist()
                .iter()
                .any(|t| t == crate::extension::TOOL_NAME),
            "`excludeTools: [subagent]` revokes the grant, so the re-grant must not resurrect it"
        );
    }

    /// B1 / pi `internalTools` (`child-tool-plan.ts:456`). `structured_output` is registered by the
    /// child-side `prompt_runtime`, i.e. as an EXTENSION tool, and NO persona declares it — it is
    /// granted by the RUN's `outputSchema`. A pinned child would otherwise be told to call a tool
    /// the allowlist had just taken away.
    ///
    /// Also pins upstream's exclusion precedence: the `.filter(!excludedToolSet.has(tool))` on that
    /// same line means an explicit `excludeTools` wins over the re-grant.
    #[test]
    fn a_structured_output_run_keeps_its_tool() {
        let agent = agent_with(Some("read"));
        let surface =
            super::resolve_tool_surface(&agent, false, None, true, no_cwd()).expect("resolves");
        assert!(
            surface
                .effective_tool_allowlist()
                .iter()
                .any(|t| t == crate::prompt_runtime::STRUCTURED_OUTPUT_TOOL_NAME),
            "a run declaring an `outputSchema` must carry `structured_output`; got {:?}",
            surface.effective_tool_allowlist()
        );

        let mut excluded = agent_with(Some("read"));
        excluded.exclude_tools =
            vec![crate::prompt_runtime::STRUCTURED_OUTPUT_TOOL_NAME.to_string()];
        let surface =
            super::resolve_tool_surface(&excluded, false, None, true, no_cwd()).expect("resolves");
        assert!(
            !surface
                .effective_tool_allowlist()
                .iter()
                .any(|t| t == crate::prompt_runtime::STRUCTURED_OUTPUT_TOOL_NAME),
            "`excludeTools` wins over the re-grant, exactly as upstream's own `.filter(…)` does"
        );
    }

    /// The other half of the bit: a run that declared NO `outputSchema` is granted nothing. This is
    /// the arity the `action:"list"` renderer (`extension::tool::routing`) passes — a listing is a
    /// statement about the AGENT, and no listing has an `outputSchema`.
    #[test]
    fn a_listing_surface_grants_no_structured_output() {
        let agent = agent_with(Some("read"));
        let surface =
            super::resolve_tool_surface(&agent, false, None, false, no_cwd()).expect("resolves");
        assert_eq!(surface.effective_tool_allowlist(), ["read"]);
    }

    /// B3, end to end. Every bundled persona declares a supervisor tool, and after #2835 a child
    /// keeps ONLY the one it named: the `intercom` alias is a full second registration of the same
    /// channel, so the four `intercom` personas stay reachable even though the canonical
    /// `contact_supervisor` registration is filtered out of their sessions.
    ///
    /// Reads the SHIPPED frontmatter rather than a fixture: the failure this guards is someone
    /// editing a persona's `tools:` line, which a hand-built agent would never see.
    #[test]
    fn every_bundled_persona_keeps_its_supervisor_tool() {
        let agents_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("agents");
        let mut checked = 0_usize;
        for entry in std::fs::read_dir(&agents_dir).expect("bundled agents dir is readable") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(std::ffi::OsStr::to_str) != Some("md") {
                continue;
            }
            let content = std::fs::read_to_string(&path).expect("persona is readable");
            let agent = agent_from_frontmatter(&content);
            let declared = agent
                .tools
                .as_ref()
                .expect("every bundled persona pins a `tools:` list")
                .clone();
            let supervisor = declared
                .iter()
                .filter_map(|tool| match tool {
                    ToolRef::Builtin(name) => Some(name.as_str()),
                    ToolRef::Mcp(_) | ToolRef::ExtensionPath(_) => None,
                })
                .find(|name| {
                    *name == crate::native_supervisor::INTERCOM_TOOL_NAME
                        || *name == "contact_supervisor"
                })
                .unwrap_or_else(|| panic!("{} declares no supervisor tool at all", path.display()))
                .to_string();

            let surface = resolve_tool_surface(&agent, false, None, no_cwd()).expect("resolves");
            assert!(
                surface.effective_tool_allowlist().contains(&supervisor),
                "{} declares `{supervisor}` but the resolved allowlist drops it, so the persona \
                 loses its supervisor channel on every launch; got {:?}",
                path.display(),
                surface.effective_tool_allowlist()
            );
            checked += 1;
        }
        assert_eq!(
            checked, 6,
            "expected the six bundled personas (worker/delegate on `contact_supervisor`, \
             reviewer/scout/oracle/researcher on `intercom`)"
        );
    }
}
