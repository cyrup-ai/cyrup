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

use crate::error::SubagentError;
use crate::exec::agent_config::AgentConfig;

// ================================================================================================
// The built-in vocabularies and the coordination exemption
// ================================================================================================

/// The closed set of built-in tool names, as `ToolRegistry::with_builtins` installs them
/// (`cyrup-tools/src/registry.rs:21-30`), in that constant's wire order — the vocabulary a host
/// observation is checked against (pi `PI_BUILTIN_TOOL_NAMES`, `child-tool-plan.ts:64`).
///
/// A LITERAL, not an alias — the same call `cyrup-permission-system` makes for its own
/// `BUILT_IN_TOOL_NAMES` (`manager.rs:38-52`, backed by a TEST-only `cyrup-tools` edge):
/// `tests::host_builtin_tool_names_track_the_tool_registry` fails the build if the two diverge, but
/// an alias would silently adopt any name a future registry adds into a set that decides what a
/// child is authorized to launch with. Widening that without a human reading it is the failure mode
/// worth a test.
///
/// Deliberately NOT [`crate::exec::tool_availability::CORE_CHILD_TOOLS`]: that is pi's SEVEN-name
/// diagnostic FLOOR (`tool_availability.rs:55-57`), it omits `powershell` and is alphabetical
/// rather than in wire order, and it answers a different question ("which builtins may be absent at
/// `agent_start` without it being a bug").
///
/// Deliberately NOT `cyrup-session-svc`'s `ALL_BUILTIN_TOOLS` (`builder.rs:349`) either: the same
/// eight names in a different order, answering "which names may `select_active_tools`' default arm
/// suppress, as opposed to an extension tool that must survive".
///
/// Deliberately NOT a list of "what an unpinned child has" either — that set is
/// `DEFAULT_BUILTIN_TOOLS` (`cyrup-session-svc/src/builder.rs:336`), overridable by the CHILD's own
/// `defaultTools` setting (CFG-079, `builder.rs:429-436`), and is therefore not knowable from the
/// parent at all. See the module doc.
///
/// Deliberately NOT `exec::mcp_direct_tools`' private `BUILTIN_TOOL_NAMES` either: that is a
/// faithful port of a DIFFERENT upstream constant (`mcp-direct-tool-grant.ts:1`) which swaps
/// `powershell` for `mcp`, and upstream's `PI_` prefix on this one exists precisely to keep the two
/// apart. `HOST_` replaces that prefix rather than dropping it, because the bare name is taken.
pub const HOST_BUILTIN_TOOL_NAMES: [&str; 8] = [
    "read",
    "bash",
    "powershell",
    "edit",
    "write",
    "grep",
    "find",
    "ls",
];

/// pi `REPOSITORY_INSPECTION_TOOLS` (`child-tool-plan.ts:65`) — what a review/scout lane cannot work
/// without. A host omission among THESE is fatal for those lanes; any other omission only warns.
///
/// Upstream's consumer is `missingPermittedRepositoryInspectionTools` (`child-tool-plan.ts:74-80`),
/// which intersects it with `unavailableHostBuiltins` and then subtracts the agent's own
/// `excludeTools` — a tool the agent *deliberately* dropped is not a fatal host omission. Paired
/// there with `isReviewOrScoutLaneAgent` (`:66, 70-72`), a `/\b(?:reviewer|scout)\b/i` match on the
/// agent name, which is what scopes "fatal" to those two lanes. Both are ported here —
/// [`missing_permitted_repository_inspection_tools`] and [`is_review_or_scout_lane_agent`] — and
/// are consumed by the diagnostic tail of [`resolve_tool_surface_in`].
///
/// Deliberately NOT asserted as a subset of [`HOST_BUILTIN_TOOL_NAMES`]: upstream keeps the two sets
/// independent (separate `Set` literals with no relation asserted anywhere) and a subset test would
/// couple them for no reason.
pub const REPOSITORY_INSPECTION_TOOLS: [&str; 6] =
    ["read", "grep", "find", "ls", "bash", "powershell"];

/// pi `NATIVE_COORDINATION_TOOL_NAMES` (`child-tool-plan.ts:68`), with upstream's own comment: these
/// providers come from child hooks, not the host's builtin tool registry, so their absence from a
/// host snapshot is meaningless and must never be reported as a host omission.
///
/// This is why no three-valued grant is needed: the uncertain names are ENUMERATED and exempted.
/// Upstream applies the exemption on BOTH sides of the same intersection (`:407-412`) — it ADDS the
/// names back into `declaredBuiltinTools` and it SUBTRACTS them from `unavailableHostBuiltins` — so
/// a missing entry here both prunes the tool AND fabricates a diagnostic about it.
///
/// **[CYRUP-DELTA] FOUR names, not three — a real divergence, not parity.** Upstream's bundled
/// personas reach their supervisor through `contact_supervisor`; cyrup's `reviewer`, `scout`,
/// `oracle` and `researcher` reach it through [`crate::native_supervisor::INTERCOM_TOOL_NAME`].
/// Run the intersection on upstream with an agent declaring `intercom` and it is pruned there too —
/// upstream has the identical hole and simply never hits it. So this is upstream's STATED RULE
/// applied to a fourth provider upstream does not have, not something upstream already does.
///
/// Omit it and the host filter strips `intercom` from `--tools`, which strips it from
/// `REQUIRED_CHILD_TOOLS`, which makes
/// [`crate::native_supervisor::native_child_intercom_fallback_should_register`] return `false`:
/// four of six bundled personas silently lose the supervisor channel they declare, on every launch.
pub const NATIVE_COORDINATION_TOOL_NAMES: [&str; 4] = [
    crate::extension::TOOL_NAME,                            // "subagent"
    crate::native_supervisor::CONTACT_SUPERVISOR_TOOL_NAME, // "contact_supervisor"
    crate::native_supervisor::NATIVE_SUPERVISOR_TOOL_NAME,  // "subagent_supervisor"
    crate::native_supervisor::INTERCOM_TOOL_NAME,           // "intercom"
];

// ================================================================================================
// The live host observation
// ================================================================================================

/// pi `getHostBuiltinToolNames` (`child-tool-plan.ts:349-362`) — the builtin names the LIVE host
/// runtime actually provides, observed in the LAUNCHING process.
///
/// Upstream's filter, ported verbatim, over [`cyrup_ext::host::HostServices::all_tools`] — the real
/// `getAllTools()` analog, whose rows carry `sourceInfo`
/// (`cyrup-session-svc/src/host_services.rs:2075-2120`; anything the extension registry does not
/// claim is labelled `"source": "builtin"` by `builtin_tool_source_info`, `:448-455`). cyrup emits
/// no `"auto"` source today, so that arm is inert; port it anyway — omitting it silently changes
/// meaning the day one appears.
///
/// **[CYRUP-DELTA] the `all_tool_names` fallback.** Some backends implement the bare-names seam
/// and not the rows seam (`cyrup-mcp/src/live.rs:1787` delegates `all_tool_names` and not
/// `all_tools`). There the full registry's NAMES are used UNFILTERED:
/// [`cyrup_ext::host::HostServices::all_tool_names`] is documented as the session's complete
/// registered set, and narrowing it to [`HOST_BUILTIN_TOOL_NAMES`] would discard true evidence and
/// manufacture omissions for every legitimately-registered non-builtin name an agent declares.
/// Erring toward "available", therefore toward launching, is upstream's own stated fail-safe
/// direction.
///
/// **The two observation paths are NOT equivalent.** `all_tools()` reports only rows whose
/// `sourceInfo.source` is `"builtin"`; everything the extension registry claims carries the
/// extension's own id instead (`session/adapters.rs:47-66` supplies the map and
/// `host_services.rs:2110-2115` prefers it over the synthetic builtin label), so `subagent`,
/// `subagent_supervisor`, `intercom` and every MCP proxy tool are ABSENT from the primary
/// observation. That is exactly why [`NATIVE_COORDINATION_TOOL_NAMES`] is load-bearing rather than
/// decorative, and the two must be read together. The `all_tool_names()` fallback
/// (`host_services.rs:2045`), by contrast, reports the whole registry.
///
/// `None` on an empty result is load-bearing (upstream's `builtins.length > 0 ? … : undefined`):
/// a host that reports nothing means availability UNKNOWN, not "everything is missing". Inverting
/// this fails every review lane on any headless host. Three backend shapes reach this: rows
/// (`cyrup-session-svc/src/host_services.rs:2075`), bare names only (`cyrup-mcp/src/live.rs:1787`)
/// and neither (`cyrup-mcp/src/owner.rs:448,450`, which stubs both seams) — the third answers
/// `None` and is the common case for a host with no live session bound.
#[must_use]
pub fn host_builtin_tool_names(
    services: Option<&dyn cyrup_ext::host::HostServices>,
) -> Option<Vec<String>> {
    let services = services?;
    let names: Vec<String> = match services.all_tools() {
        Some(rows) => rows
            .iter()
            .filter_map(|row| {
                let name = row.get("name")?.as_str()?;
                let source = row.get("sourceInfo")?.get("source")?.as_str()?;
                (source == "builtin"
                    || (source == "auto" && HOST_BUILTIN_TOOL_NAMES.contains(&name)))
                .then(|| name.to_string())
            })
            .collect(),
        None => services.all_tool_names()?,
    };
    (!names.is_empty()).then_some(names)
}

// ================================================================================================
// ResolvedToolSurface
// ================================================================================================

/// The tool surface one child will actually launch with, resolved from the SAME four inputs
/// `spawn_plan::resolve_child_tools` uses and by the SAME code (see [`resolve_tool_surface`]).
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
    /// pi `unavailableHostBuiltins` (`child-tool-plan.ts:223`) — builtins this agent declared (after
    /// the ceiling filter) that the host runtime does not provide. Empty whenever availability is
    /// unknown: absence of evidence is never reported as evidence of absence.
    ///
    /// Deliberately NOT narrowed by `excludeTools`, matching upstream's raw emission at `:616`: its
    /// consumer subtracts them itself (`missingPermittedRepositoryInspectionTools`, `:532`), and
    /// doing it here would make an excluded-and-host-missing tool invisible to the lane contract.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unavailable_host_builtins: Vec<String>,
    /// pi `PiLaunchToolPlan.warnings` (`:221`) — non-fatal launch diagnostics; they change no
    /// behaviour. Declared here so the wire shape settles in one commit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
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
/// # Errors
///
/// [`SubagentError::ToolContractUnsatisfiable`] when the host runtime does not provide a required
/// `read`; [`SubagentError::CapabilityCeilingViolation`] when the ceiling excludes it. Both are
/// upstream throws (`child-tool-plan.ts:384-389`, `:390-394`) and fire in that order.
/// [`SubagentError::ToolContractUnsatisfiable`] again for the fanout refusal (`:421-423`) when the
/// effective surface holds `subagent_supervisor` without fanout authorization, and a third time
/// for the review-lane contract (`:531-543`) when a `reviewer`/`scout` launch's permitted,
/// non-excluded [`REPOSITORY_INSPECTION_TOOLS`] are host-missing.
pub fn resolve_tool_surface(
    agent: &AgentConfig,
    require_read_tool: bool,
    ceiling: Option<&crate::exec::capability_ceiling::ResolvedCapabilityCeiling>,
    host_available_builtins: Option<&[String]>,
    structured_output: bool,
    cwd: &std::path::Path,
) -> Result<ResolvedToolSurface, SubagentError> {
    resolve_tool_surface_in(
        agent,
        require_read_tool,
        ceiling,
        host_available_builtins,
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
/// Same three refusals as [`resolve_tool_surface`].
pub fn resolve_tool_surface_in(
    agent: &AgentConfig,
    require_read_tool: bool,
    ceiling: Option<&crate::exec::capability_ceiling::ResolvedCapabilityCeiling>,
    host_available_builtins: Option<&[String]>,
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

    // pi `child-tool-plan.ts:384-389` — FIRST of the two `read` throws, and the reason the ceiling
    // throw below had to MOVE here out of `spawn_plan`: upstream reports the HOST failure ahead of
    // the CEILING one, and a caller that hits both must see this message.
    //
    // [CYRUP-DELTA] upstream's `agentLabel` is conditional (`input.agentName ? … : ""`) because its
    // input carries an optional name; `AgentConfig::name` is always present, so the label is always
    // emitted and the `""` arm is unreachable here.
    if require_read_tool
        && let Some(available) = host_available_builtins
        && !available.iter().any(|t| t == "read")
    {
        let agent_label = format!(" for agent '{}'", agent.name);
        return Err(SubagentError::ToolContractUnsatisfiable(format!(
            "Host runtime does not provide required tool 'read'{agent_label} for lazy skill loading."
        )));
    }

    // pi `child-tool-plan.ts:390-394`, moved here from `spawn_plan.rs`'s
    // `build_attempt_spawn_plan_with_read_requirement` so it fires AFTER the host throw above,
    // matching upstream's order. Text and `"unknown source"` fallback unchanged by the move.
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
    // `requested_builtin_tools` is CLONED rather than moved: it is pi's own `requestedBuiltinTools`
    // (`child-tool-plan.ts:379-383`) — the raw, pre-ceiling, pre-host, pre-exclude list — and the
    // `Requested tool names:` segment of upstream's host-omission warning (`:522-530`) reads it
    // AFTER every narrowing below has run, so the raw list has to survive this stage.
    let ceiling_filtered: Vec<String> = if agent.tools.is_some() {
        let mut declared = requested_builtin_tools.clone();

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

    // pi `child-tool-plan.ts:407-412`. The ceiling-filtered set is what the agent (and any ceiling)
    // ASKED FOR; intersecting it with the host's live registry is what the child will actually get.
    // `NATIVE_COORDINATION_TOOL_NAMES` is exempt in BOTH directions — those providers come from
    // child hooks, never the builtin registry (see `host_builtin_tool_names`' own doc: the primary
    // `all_tools()` observation cannot see them at all), so a host snapshot says nothing about them.
    //
    // ONE `partition`, not two `filter`s: upstream runs two complementary predicates (`:408` and
    // `:411`), and a partition cannot drift out of complement the way two hand-written ones can.
    //
    // The residue is deliberately NOT narrowed by `exclude_tools` below, matching upstream's raw
    // emission at `:616` — its consumer subtracts them itself
    // (`missingPermittedRepositoryInspectionTools`, `:532`), pinned by upstream's own "does not
    // treat excluded repository tools as a missing review-lane contract" test. Adding a
    // `retain(|t| !is_excluded(t))` after the exclusion filter would look like a tidy-up and would
    // silently make an excluded-and-host-missing tool invisible to that contract.
    let (declared, unavailable_host_builtins): (Vec<String>, Vec<String>) =
        match host_available_builtins {
            Some(available) => ceiling_filtered.into_iter().partition(|tool| {
                available.iter().any(|a| a == tool)
                    || NATIVE_COORDINATION_TOOL_NAMES.contains(&tool.as_str())
            }),
            // Upstream's `hostAvailableSet === undefined` is UNKNOWN, not empty: no filter, no
            // omissions. `Some(vec![])` is the opposite — a real observation of nothing (an empty
            // JS `Set` is truthy), so it prunes everything.
            None => (ceiling_filtered, Vec::new()),
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
    // NOT reachable through the host intersection: `subagent` is itself in
    // `NATIVE_COORDINATION_TOOL_NAMES`, so a host snapshot omitting it prunes nothing (upstream is
    // identical, `:68` and `:408`). The three paths that DO reach it are a ceiling whose
    // `allowedTools` admits `subagent_supervisor` but not `subagent` (the `declared.retain` above is
    // NOT exemption-guarded, and `subagent_within_ceiling` is then `false` too, so
    // `allowNestedSubagents` cannot rescue it), an `excludeTools: [subagent]`, and an agent that
    // simply declared the supervisor tool without `subagent`.
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
    // pi `requestedToolNames` (`child-tool-plan.ts:522-530`) reads `resolvedMcpSelections` (`:440`),
    // NOT `effectiveMcpTools` (`:449-451`) — the message names what was ASKED FOR, before either
    // narrowing below. Bound here, at upstream's own point in the sequence, rather than recovered
    // later from a value that has already lost the distinction.
    //
    // Read by the `Requested tool names:` segment of both diagnostics at the end of this function.
    let resolved_mcp_selections = effective_mcp_tools.clone();
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

    // Built BEFORE the diagnostic tail below rather than returned directly: every input that tail
    // needs is either moved into this literal (`excluded`, `unavailable_host_builtins`) or is a
    // METHOD on the finished value (`effective_tool_allowlist`, the whole reason TOOLCON_4 folded
    // the MCP half in). Reading them back off `surface` is what keeps the diagnostics describing
    // the SAME surface the child launches with.
    let mut surface = ResolvedToolSurface {
        builtins: effective_builtin_tools,
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
        unavailable_host_builtins,
        warnings: Vec::new(),
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
    };

    // pi `requestedToolNames` (`child-tool-plan.ts:522-530`): the union of the RAW requested
    // builtins and the PRE-narrowing resolved MCP names, de-duplicated in first-seen order —
    // `[...new Set([...])]`. `None` exactly when the agent wrote no `tools:` key (`:523`), which is
    // what makes both messages say "not explicitly specified" instead of inventing `[]`.
    //
    // Note these are the RAW lists, not `surface.builtins` / `surface.effective_mcp_tools`: the
    // segment names what was ASKED FOR, before the ceiling, the host intersection and the
    // exclusions each narrowed it. Naming the narrowed lists would make the message agree with
    // itself and tell the operator nothing.
    let requested_tool_names: Option<Vec<String>> = agent.tools.as_ref().map(|_| {
        let mut seen = std::collections::HashSet::new();
        requested_builtin_tools
            .iter()
            .chain(resolved_mcp_selections.iter())
            .filter(|tool| seen.insert((*tool).clone()))
            .cloned()
            .collect()
    });
    let effective_tool_allowlist = surface.effective_tool_allowlist();
    let ceiling_sources: &[String] = ceiling.map_or(&[], |ceiling| &ceiling.sources);

    // pi `:531-533`. The `input.tools !== undefined` guard is what keeps an UNPINNED agent silent:
    // it never asked for a minimum, so it has none to be missing. This suppresses only the
    // REFUSAL — the warning below still fires for an unpinned agent whose ceiling-derived surface
    // the host cannot provide, which is upstream's own "does not invent an explicit request" case.
    let missing_permitted_repository_tools = if agent.tools.is_some() {
        missing_permitted_repository_inspection_tools(
            &surface.unavailable_host_builtins,
            &surface.excluded,
        )
    } else {
        Vec::new()
    };
    // pi `:534-543`. Scoped to the two lanes BY NAME: a review or scout that cannot read the
    // repository cannot produce a review, and returning one anyway would report success for work
    // that never happened — the same fail-closed argument
    // [`SubagentError::ToolContractUnsatisfiable`]'s own doc makes.
    if !missing_permitted_repository_tools.is_empty() && is_review_or_scout_lane_agent(&agent.name)
    {
        return Err(SubagentError::ToolContractUnsatisfiable(
            format_review_lane_tool_contract_failure(
                Some(&agent.name),
                &missing_permitted_repository_tools,
                requested_tool_names.as_deref(),
                &effective_tool_allowlist,
                ceiling_sources,
                &surface.excluded,
            ),
        ));
    }

    // pi `:544-556`, with upstream's own comment: host pruning also happens WITHOUT a ceiling (and
    // therefore without an audit), so this is a non-fatal launch warning rather than an invented
    // ceiling or a requested allowlist treated as a minimum requirement.
    //
    // Fires even when the refusal above did NOT — for a non-lane agent, for an unpinned one, and
    // for tools the agent itself excluded: an exclusion means "not a contract breach", never "not
    // an omission".
    //
    // CONCATENATED, not joined: each optional segment carries its own TRAILING SPACE inside it
    // (`:552`, `:553`), which is exactly why this cannot reuse the failure's `join(" ")`. And the
    // ceiling-sources rule is the OTHER one — emitted whenever a ceiling exists AT ALL, with
    // upstream's `|| "unknown source"` for empty `sources`, where the failure omits the whole
    // segment. Both halves are pinned by tests.
    if !surface.unavailable_host_builtins.is_empty() {
        let mut warning = format!(
            "{}: host runtime tool availability omitted [{}]. Requested tool names: {}; \
             effective tool allowlist: [{}]. ",
            diagnostic_subject(Some(&agent.name)),
            surface.unavailable_host_builtins.join(", "),
            format_requested_tool_names(requested_tool_names.as_deref()),
            effective_tool_allowlist.join(", ")
        );
        if let Some(ceiling) = ceiling {
            // pi `:552`'s `capabilityCeiling.sources.join(", ") || "unknown source"` — the JS `||`
            // fires on the EMPTY STRING, so a ceiling registered with no sources still names
            // itself. Same idiom as the ceiling `read` refusal above.
            let joined = ceiling.sources.join(", ");
            let sources = if joined.is_empty() {
                "unknown source"
            } else {
                joined.as_str()
            };
            warning.push_str(&format!("Active capability ceiling sources: [{sources}]. "));
        }
        if !surface.excluded.is_empty() {
            warning.push_str(&format!(
                "Explicit excludeTools: [{}]. ",
                surface.excluded.join(", ")
            ));
        }
        warning.push_str(
            "This is a non-fatal tool-plan diagnostic, not verification of the child's runtime \
             tool menu.",
        );
        surface.warnings.push(warning);
    }

    Ok(surface)
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

// ================================================================================================
// The review-lane contract and the host-omission diagnostic
// ================================================================================================

/// pi `isReviewOrScoutLaneAgent` (`child-tool-plan.ts:70-72`). Thin re-export of
/// [`crate::exec::task_intent::is_review_or_scout_lane_agent`]: the regex primitives it is built
/// from (`boundary_before` / `alt_word` / `any_match`, the `\b` atoms every ported source pattern
/// in this crate shares) live in that module and stay there, but the lane rule's only consumer is
/// this one, so this is where it is named.
#[must_use]
pub fn is_review_or_scout_lane_agent(agent_name: &str) -> bool {
    crate::exec::task_intent::is_review_or_scout_lane_agent(agent_name)
}

/// pi `missingPermittedRepositoryInspectionTools` (`child-tool-plan.ts:74-80`) — the omissions that
/// actually break a review lane: [`REPOSITORY_INSPECTION_TOOLS`] the host did not provide and the
/// agent did NOT itself exclude.
///
/// The `excludeTools` subtraction is the whole reason
/// [`ResolvedToolSurface::unavailable_host_builtins`] is emitted RAW (see that field's doc): a tool
/// the agent deliberately dropped is an omission worth reporting but never a contract breach.
#[must_use]
pub fn missing_permitted_repository_inspection_tools(
    unavailable_host_builtins: &[String],
    exclude_tools: &[String],
) -> Vec<String> {
    unavailable_host_builtins
        .iter()
        .filter(|tool| REPOSITORY_INSPECTION_TOOLS.contains(&tool.as_str()))
        .filter(|tool| !exclude_tools.iter().any(|excluded| excluded == *tool))
        .cloned()
        .collect()
}

/// The `Agent '<name>'` / `Subagent` subject both diagnostics open with (pi `:90` and `:548` — the
/// SAME ternary, written out twice upstream). One helper so the two cannot drift.
///
/// [`crate::exec::agent_config::AgentConfig::name`] is a non-optional `String`, so
/// [`resolve_tool_surface_in`] always reaches the `Some` arm on both paths; `None` is reachable
/// only from a caller that passes it. The arm is kept because the ported message is a public
/// formatting contract, not an internal one — pinned by
/// `the_diagnostic_subject_names_an_agent_or_falls_back_to_subagent`.
fn diagnostic_subject(agent_name: Option<&str>) -> String {
    agent_name.map_or_else(|| "Subagent".to_string(), |name| format!("Agent '{name}'"))
}

/// pi `formatReviewLaneToolContractFailure` (`child-tool-plan.ts:82-98`) — verbatim.
///
/// Segments are joined by a SINGLE SPACE (`:97`) and an optional segment is omitted ENTIRELY when
/// its list is empty (`:94`, `:95`). Contrast the host-omission warning in
/// [`resolve_tool_surface_in`], which CONCATENATES segments that each carry their own TRAILING
/// SPACE, and whose ceiling-sources rule is deliberately different — see that code's comment.
///
/// `ceiling_sources` is upstream's `capabilityCeiling?.sources` (`:540`): pass an empty slice both
/// when there is no ceiling and when its `sources` are empty, since `:94`'s `?.length` collapses
/// the two.
#[must_use]
pub fn format_review_lane_tool_contract_failure(
    agent_name: Option<&str>,
    missing_tools: &[String],
    requested_tools: Option<&[String]>,
    effective_tools: &[String],
    ceiling_sources: &[String],
    exclude_tools: &[String],
) -> String {
    let mut segments = vec![
        format!(
            "{}: tool contract could not be satisfied; host runtime does not provide permitted \
             required repository tools [{}].",
            diagnostic_subject(agent_name),
            missing_tools.join(", ")
        ),
        format!(
            "Requested tool names: {}; effective tool allowlist: [{}].",
            format_requested_tool_names(requested_tools),
            effective_tools.join(", ")
        ),
    ];
    // pi `:94` — `input.ceilingSources?.length ? [...] : []`. Emitted only when NON-EMPTY, with no
    // `"unknown source"` fallback. The warning's rule is deliberately different (pi `:552`), which
    // is why a ceiling carrying `sources: []` warns `[unknown source]` but contributes no segment
    // here. That asymmetry is upstream-observable and is pinned by
    // `the_failure_omits_an_empty_ceiling_sources_segment`.
    if !ceiling_sources.is_empty() {
        segments.push(format!(
            "Active capability ceiling sources: [{}].",
            ceiling_sources.join(", ")
        ));
    }
    if !exclude_tools.is_empty() {
        segments.push(format!(
            "Explicit excludeTools: [{}].",
            exclude_tools.join(", ")
        ));
    }
    segments.push(
        "This is a lane infrastructure failure, not a completed review/scout result.".to_string(),
    );
    segments.join(" ")
}

/// pi's `requestedToolNames ? \`[${...join(", ")}]\` : "not explicitly specified"` — written once
/// because BOTH diagnostics interpolate it identically (`:93` and `:551`).
///
/// `None` is not `[]`: an agent that never wrote a `tools:` key stated no request at all, and
/// printing an empty list would claim it asked for nothing.
fn format_requested_tool_names(requested_tools: Option<&[String]>) -> String {
    requested_tools.map_or_else(
        || "not explicitly specified".to_string(),
        |tools| format!("[{}]", tools.join(", ")),
    )
}

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
        host_available_builtins: Option<&[String]>,
        cwd: &std::path::Path,
    ) -> Result<ResolvedToolSurface, SubagentError> {
        super::resolve_tool_surface(
            agent,
            require_read_tool,
            ceiling,
            host_available_builtins,
            false,
            cwd,
        )
    }

    fn resolve_tool_surface_in(
        agent: &AgentConfig,
        require_read_tool: bool,
        ceiling: Option<&crate::exec::capability_ceiling::ResolvedCapabilityCeiling>,
        host_available_builtins: Option<&[String]>,
        cwd: &std::path::Path,
        dirs: &crate::exec::mcp_direct_tools::McpDirs,
    ) -> Result<ResolvedToolSurface, SubagentError> {
        super::resolve_tool_surface_in(
            agent,
            require_read_tool,
            ceiling,
            host_available_builtins,
            false,
            cwd,
            dirs,
        )
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

    /// ANTI-DRIFT. [`HOST_BUILTIN_TOOL_NAMES`] and `cyrup_tools::BUILTIN_NAMES` are separately
    /// stated on purpose (see the const's doc); this fails the build the day they diverge.
    #[test]
    fn host_builtin_tool_names_track_the_tool_registry() {
        for name in cyrup_tools::BUILTIN_NAMES {
            assert!(
                HOST_BUILTIN_TOOL_NAMES.contains(&name),
                "`{name}` is installed by ToolRegistry::with_builtins but is not in \
                 HOST_BUILTIN_TOOL_NAMES, so a task claiming it against a pinned agent that lacks \
                 it would go unreported"
            );
        }
        for name in HOST_BUILTIN_TOOL_NAMES {
            assert!(
                cyrup_tools::BUILTIN_NAMES.contains(&name),
                "HOST_BUILTIN_TOOL_NAMES claims `{name}` is a built-in the registry installs, but \
                 the registry never installs it, so a host observation would be validated against \
                 a name that can never appear in one"
            );
        }
        assert_eq!(
            HOST_BUILTIN_TOOL_NAMES.len(),
            cyrup_tools::BUILTIN_NAMES.len()
        );
    }

    /// pi `REPOSITORY_INSPECTION_TOOLS` (`child-tool-plan.ts:65`) verbatim, ORDER included.
    #[test]
    fn repository_inspection_tools_match_upstream() {
        assert_eq!(
            REPOSITORY_INSPECTION_TOOLS,
            ["read", "grep", "find", "ls", "bash", "powershell"]
        );
    }

    /// The guard for the FOUR-name `[CYRUP-DELTA]`: if someone "restores parity" by dropping to
    /// upstream's three, this fails. Also pins that the three portable names are wired from the
    /// crate constants rather than restated as strings, so a rename over there reaches here.
    #[test]
    fn native_coordination_names_match_the_crate_constants() {
        assert_eq!(
            NATIVE_COORDINATION_TOOL_NAMES.len(),
            4,
            "cyrup needs a FOURTH coordination name (`intercom`) that upstream does not have; \
             dropping to three prunes it from four of six bundled personas on every launch"
        );
        assert!(NATIVE_COORDINATION_TOOL_NAMES.contains(&crate::extension::TOOL_NAME));
        assert!(NATIVE_COORDINATION_TOOL_NAMES.contains(&"contact_supervisor"));
        assert!(
            NATIVE_COORDINATION_TOOL_NAMES
                .contains(&crate::native_supervisor::NATIVE_SUPERVISOR_TOOL_NAME)
        );
        assert!(
            NATIVE_COORDINATION_TOOL_NAMES.contains(&crate::native_supervisor::INTERCOM_TOOL_NAME)
        );
    }

    /// Every SHIPPED persona's declared builtin names must be either a host builtin or
    /// coordination-exempt, or TOOLCON_3's host filter will strip it on every launch. Iterates the
    /// bundled directory rather than a hard-coded list, so a persona added later is covered too.
    #[test]
    fn every_intercom_persona_is_covered_by_the_coordination_exemption() {
        let dir = crate::registration::resources::bundled_resources_dir().join("agents");
        let mut checked = 0usize;
        for entry in std::fs::read_dir(&dir).expect("bundled agents dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let content = std::fs::read_to_string(&path).expect("persona reads");
            let def = crate::discovery::frontmatter::parse_agent_file(
                &content,
                AgentSource::Builtin,
                &path,
            )
            .expect("shipped persona parses");
            for tool in def.tools.iter().flatten() {
                if let ToolRef::Builtin(name) = tool {
                    let n = name.as_str();
                    assert!(
                        HOST_BUILTIN_TOOL_NAMES.contains(&n)
                            || NATIVE_COORDINATION_TOOL_NAMES.contains(&n),
                        "shipped persona {} declares `{n}`, which is neither a host builtin nor \
                         coordination-exempt — the host filter will strip it on every launch",
                        path.display()
                    );
                }
            }
            checked += 1;
        }
        assert_eq!(checked, 6, "all six bundled personas must be inspected");
    }

    // ---- host_builtin_tool_names(): the live host observation ----

    /// The ROWS-seam [`cyrup_ext::host::HostServices`] double. Defined in
    /// [`crate::exec::testsupport`] rather than here because two modules now need it: this file's
    /// observer tests and `extension::executor`'s seam test.
    use crate::exec::testsupport::RowsHost;

    /// A double answering only the BARE-NAMES seam — the `cyrup-mcp/src/live.rs:1787` shape, which
    /// implements `all_tool_names` and not `all_tools`.
    struct BareNamesHost(Option<Vec<String>>);
    impl cyrup_ext::host::HostServices for BareNamesHost {
        fn all_tool_names(&self) -> Option<Vec<String>> {
            self.0.clone()
        }
    }

    /// A double answering NEITHER seam — the `cyrup-mcp/src/owner.rs:448,450` shape, and the common
    /// case for any host with no live session bound. Both trait defaults return `None`.
    struct SilentHost;
    impl cyrup_ext::host::HostServices for SilentHost {}

    #[test]
    fn host_builtin_tool_names_is_none_without_a_host() {
        assert_eq!(host_builtin_tool_names(None), None);
    }

    /// `None`, NOT `Some(vec![])`: a host that reports nothing means availability UNKNOWN, not
    /// "everything is missing". Inverting this fails every review lane on a headless host.
    #[test]
    fn host_builtin_tool_names_is_none_for_an_empty_registry() {
        assert_eq!(host_builtin_tool_names(Some(&RowsHost(Some(vec![])))), None);
        assert_eq!(
            host_builtin_tool_names(Some(&BareNamesHost(Some(vec![])))),
            None
        );
    }

    /// The third backend shape: neither seam answered, so availability is unknown.
    #[test]
    fn host_builtin_tool_names_is_none_when_the_host_answers_neither_seam() {
        assert_eq!(host_builtin_tool_names(Some(&SilentHost)), None);
    }

    /// The `sourceInfo.source` filter, both halves of the `"auto"` arm included: an `"auto"` row
    /// whose name IS a host builtin is kept, one whose name is not is dropped.
    #[test]
    fn host_builtin_tool_names_reads_source_info() {
        let rows = vec![
            serde_json::json!({"name": "read", "sourceInfo": {"source": "builtin"}}),
            serde_json::json!({"name": "some_ext_tool", "sourceInfo": {"source": "demo-ext"}}),
            serde_json::json!({"name": "bash", "sourceInfo": {"source": "auto"}}),
            serde_json::json!({"name": "web_search", "sourceInfo": {"source": "auto"}}),
        ];
        assert_eq!(
            host_builtin_tool_names(Some(&RowsHost(Some(rows)))),
            Some(vec!["read".to_string(), "bash".to_string()])
        );
    }

    /// The fallback arm does NOT narrow to the eight: `intercom` survives, because `all_tool_names`
    /// is the session's complete registered set and discarding it would manufacture omissions.
    #[test]
    fn host_builtin_tool_names_falls_back_to_bare_names_unfiltered() {
        let host = BareNamesHost(Some(vec!["read".to_string(), "intercom".to_string()]));
        assert_eq!(
            host_builtin_tool_names(Some(&host)),
            Some(vec!["read".to_string(), "intercom".to_string()])
        );
    }

    /// Upstream's `try { … } catch` degrades to "not observed", never to "absent": a row missing
    /// `name`, missing `sourceInfo`, or carrying a non-string `source` is skipped, not fatal.
    #[test]
    fn a_malformed_tool_row_is_skipped_not_fatal() {
        let rows = vec![
            serde_json::json!({"sourceInfo": {"source": "builtin"}}),
            serde_json::json!({"name": "missing_source_info"}),
            serde_json::json!({"name": "non_string_source", "sourceInfo": {"source": 7}}),
            serde_json::json!({"name": "read", "sourceInfo": {"source": "builtin"}}),
        ];
        assert_eq!(
            host_builtin_tool_names(Some(&RowsHost(Some(rows)))),
            Some(vec!["read".to_string()])
        );
    }

    fn pinned(tools: &[&str]) -> ResolvedToolSurface {
        ResolvedToolSurface {
            builtins: tools.iter().map(|t| (*t).to_string()).collect(),
            pinned: true,
            excluded: Vec::new(),
            fanout_authorized: false,
            unavailable_host_builtins: Vec::new(),
            warnings: Vec::new(),
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

    /// `agent_with` with an explicit NAME.
    ///
    /// Needed because `agent_with`'s `reviewer` is a review-LANE name: any test that host-prunes a
    /// [`REPOSITORY_INSPECTION_TOOLS`] entry from a `reviewer` is a lane-contract REFUSAL, not a
    /// resolution. Tests about the host INTERSECTION — a name-independent mechanic — must therefore
    /// not use a lane name; upstream's own equivalent uses `worker` for exactly this reason
    /// (`child-tool-plan-diagnostics.test.ts`, "keeps host-pruned warnings for non-review agents").
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
        let surface = resolve_tool_surface(&agent, false, None, None, no_cwd()).expect("resolves");
        assert!(surface.pinned);
        assert_eq!(surface.builtins, ["read", "grep", "find", "ls", "intercom"]);
    }

    #[test]
    fn resolve_leaves_an_agent_without_a_tools_key_unpinned() {
        let surface =
            resolve_tool_surface(&agent_with(None), false, None, None, no_cwd()).expect("resolves");
        assert!(!surface.pinned);
        assert!(surface.builtins.is_empty());
    }

    /// SUBA-014: the `read` head-injection, preserved by the hoist.
    #[test]
    fn resolve_head_injects_read_when_a_skill_required_it() {
        let agent = agent_with(Some("bash, edit"));
        let surface = resolve_tool_surface(&agent, true, None, None, no_cwd()).expect("resolves");
        assert_eq!(surface.builtins, ["read", "bash", "edit"]);
    }

    /// SUBA-072: a ceiling narrows, never widens, and suppresses the head-injection.
    #[test]
    fn resolve_narrows_to_a_capability_ceiling() {
        let agent = agent_with(Some("read, bash, edit"));
        let c = ceiling(Some(&["read", "grep"]), &[]);
        let surface =
            resolve_tool_surface(&agent, true, Some(&c), None, no_cwd()).expect("resolves");
        assert_eq!(surface.builtins, ["read"]);
    }

    /// SUBA-072: a ceiling alone pins the surface of an agent that declared no `tools:`.
    #[test]
    fn resolve_is_pinned_by_a_ceiling_alone() {
        let c = ceiling(Some(&["read"]), &[]);
        let surface = resolve_tool_surface(&agent_with(None), false, Some(&c), None, no_cwd())
            .expect("resolves");
        assert!(surface.pinned);
        assert_eq!(surface.builtins, ["read"]);
    }

    /// SUBA-092: `excludeTools` subtracts from an explicit allowlist.
    #[test]
    fn resolve_subtracts_exclude_tools() {
        let mut agent = agent_with(Some("read, bash"));
        agent.exclude_tools = vec!["  bash  ".to_string(), "bash".to_string(), String::new()];
        let surface = resolve_tool_surface(&agent, false, None, None, no_cwd()).expect("resolves");
        assert_eq!(surface.builtins, ["read"]);
        assert_eq!(surface.excluded, ["bash"], "trimmed and de-duplicated");
    }

    /// SUBA-092: the fanout disjunction survives the hoist intact.
    #[test]
    fn resolve_carries_fanout_authorization() {
        let mut declared = agent_with(Some(&format!("read, {}", crate::extension::TOOL_NAME)));
        assert!(
            resolve_tool_surface(&declared, false, None, None, no_cwd())
                .expect("resolves")
                .fanout_authorized
        );

        declared.exclude_tools = vec![crate::extension::TOOL_NAME.to_string()];
        assert!(
            !resolve_tool_surface(&declared, false, None, None, no_cwd())
                .expect("resolves")
                .fanout_authorized
        );

        let mut granted = agent_with(Some("read"));
        granted.allow_nested_subagents = Some(true);
        assert!(
            resolve_tool_surface(&granted, false, None, None, no_cwd())
                .expect("resolves")
                .fanout_authorized
        );

        let c = ceiling(Some(&["read"]), &[]);
        assert!(
            !resolve_tool_surface(&granted, false, Some(&c), None, no_cwd())
                .expect("resolves")
                .fanout_authorized
        );
    }

    // ---- the host intersection (pi `child-tool-plan.ts:407-412`) ----

    /// Upstream: *"does not invent host omissions when availability is unknown"*. `None` is UNKNOWN,
    /// not empty — absence of evidence is never reported as evidence of absence.
    #[test]
    fn host_availability_unknown_prunes_nothing() {
        let agent = agent_with(Some("read, grep"));
        let surface = resolve_tool_surface(&agent, false, None, None, no_cwd()).expect("resolves");
        assert_eq!(surface.builtins, ["read", "grep"]);
        assert!(surface.unavailable_host_builtins.is_empty());
        // Upstream's "does not invent host omissions when availability is unknown", first input:
        // no omission means no diagnostic either.
        assert!(surface.warnings.is_empty());
    }

    /// The `None` / `Some([])` split: an empty JS `Set` is TRUTHY, so upstream runs the filter and
    /// everything becomes unavailable. `host_builtin_tool_names` never returns `Some(vec![])` by
    /// design, so this arm is only reachable from an explicit empty slice — exactly this call.
    ///
    /// `worker`, not the default `reviewer`: `read` and `grep` are both
    /// [`REPOSITORY_INSPECTION_TOOLS`], so pruning them from a LANE agent is a contract refusal
    /// rather than a resolution. The mechanic under test is name-independent.
    #[test]
    fn an_explicitly_empty_host_set_is_a_real_observation() {
        let agent = agent_named("worker", Some("read, grep"));
        let surface =
            resolve_tool_surface(&agent, false, None, Some(&[]), no_cwd()).expect("resolves");
        assert!(surface.builtins.is_empty());
        assert_eq!(surface.unavailable_host_builtins, ["read", "grep"]);
    }

    /// A tool the CEILING pruned never reaches the intersection, so it is not a HOST omission. The
    /// two subtractions are reported separately because only one of them is the host's fault.
    ///
    /// This is simultaneously upstream's *"does not invent host omissions … [when] a ceiling alone
    /// prunes tools"* third input AND its *"does not reject an intentionally empty or
    /// ceiling-restricted review allowlist"* second case: `agent_with` IS a `reviewer`, so a
    /// ceiling-emptied allowlist on a LANE agent must resolve silently — nothing became
    /// unavailable, so there is nothing to be missing and nothing to warn about.
    #[test]
    fn a_ceiling_emptied_allowlist_is_not_a_host_omission() {
        let agent = agent_with(Some("read, grep"));
        let c = ceiling(Some(&[]), &[]);
        let surface =
            resolve_tool_surface(&agent, false, Some(&c), Some(&[]), no_cwd()).expect("resolves");
        assert!(surface.builtins.is_empty());
        assert!(
            surface.unavailable_host_builtins.is_empty(),
            "the ceiling pruned these, not the host: {:?}",
            surface.unavailable_host_builtins
        );
        assert!(surface.effective_tool_allowlist().is_empty());
        assert!(surface.warnings.is_empty());
    }

    /// The one structural insertion TOOLCON_3 made: ceiling filter -> HOST filter -> exclusions.
    /// `write` was ceiling-pruned so it is in neither list; `bash` was host-present so only the
    /// exclusion removed it; `grep` survived the ceiling but the host does not provide it.
    ///
    /// `worker`, not the default `reviewer`: `grep` is a [`REPOSITORY_INSPECTION_TOOLS`] entry and
    /// `exclude_tools` does not cover it, so a LANE agent would be refused here. Ordering is a
    /// name-independent mechanic.
    #[test]
    fn the_host_filter_runs_after_the_ceiling_and_before_exclusions() {
        let mut agent = agent_named("worker", Some("read, grep, bash, write"));
        agent.exclude_tools = vec!["bash".to_string()];
        let c = ceiling(Some(&["read", "grep", "bash"]), &[]);
        let host = ["read".to_string(), "bash".to_string()];
        let surface =
            resolve_tool_surface(&agent, false, Some(&c), Some(&host), no_cwd()).expect("resolves");
        assert_eq!(surface.builtins, ["read"]);
        assert_eq!(surface.unavailable_host_builtins, ["grep"]);
    }

    /// ANTI-REGRESSION for the review-lane contract. `unavailable_host_builtins` is emitted RAW,
    /// exactly as upstream does at `:616` — its consumer
    /// (`missingPermittedRepositoryInspectionTools`, `:532`) subtracts `excludeTools` itself.
    /// Pinned by upstream's own *"does not treat excluded repository tools as a missing review-lane
    /// contract"* test. Narrowing it here would look like a tidy-up and would make an
    /// excluded-and-host-missing tool invisible to that contract.
    ///
    /// This IS that upstream case's second half: the agent is a `reviewer` and BOTH omissions are
    /// repository-inspection tools, so the only thing keeping the launch alive is the
    /// `excludeTools` subtraction inside [`missing_permitted_repository_inspection_tools`] — the
    /// `.expect("resolves")` below is a real assertion, not scaffolding. The omission is still
    /// WARNED about, because an exclusion means "not a contract breach", never "not an omission".
    #[test]
    fn an_excluded_tool_is_still_reported_as_a_host_omission() {
        let mut agent = agent_with(Some("read, grep"));
        agent.exclude_tools = vec!["read".to_string(), "grep".to_string()];
        let surface = resolve_tool_surface(&agent, false, None, Some(&[]), no_cwd())
            .expect("the exclusion means this is not a lane-contract breach");
        assert!(surface.builtins.is_empty());
        assert_eq!(
            surface.unavailable_host_builtins,
            ["read", "grep"],
            "excluded AND host-missing must still be reported as a host omission"
        );
        assert_eq!(surface.warnings.len(), 1, "{:?}", surface.warnings);
        assert!(
            surface.warnings[0].contains("host runtime tool availability omitted [read, grep]"),
            "{}",
            surface.warnings[0]
        );
        assert!(
            surface.warnings[0].contains("Explicit excludeTools: [read, grep]. "),
            "{}",
            surface.warnings[0]
        );
    }

    /// The §1.1 regression guard at resolver level, pairing with
    /// [`native_coordination_names_match_the_crate_constants`]. `intercom` is never in the builtin
    /// registry — its provider is a child hook — so a host snapshot that omits it says nothing, and
    /// the exemption must hold in BOTH directions: kept in `builtins`, absent from the omissions.
    ///
    /// Driven off the real [`HOST_BUILTIN_TOOL_NAMES`] rather than a copy of its contents, so this
    /// keeps guarding the day that constant changes. Collected into a `Vec<String>` because the
    /// constant is `[&str; 8]` and the parameter is `&[String]`.
    #[test]
    fn coordination_tools_survive_a_host_that_does_not_list_them() {
        let host: Vec<String> = HOST_BUILTIN_TOOL_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        let agent = agent_with(Some("read, grep, find, ls, intercom"));
        let surface =
            resolve_tool_surface(&agent, false, None, Some(&host), no_cwd()).expect("resolves");
        assert!(
            surface.builtins.contains(&"intercom".to_string()),
            "{:?}",
            surface.builtins
        );
        assert!(surface.unavailable_host_builtins.is_empty());
    }

    // ---- the two `read` throws, and their order ----

    /// pi `child-tool-plan.ts:384-389` fires BEFORE `:390-394`. This is the whole reason the ceiling
    /// throw had to MOVE out of `spawn_plan` rather than be duplicated: a caller that violates both
    /// must see the HOST message, and only the resolver knows both facts.
    #[test]
    fn the_host_read_throw_precedes_the_ceiling_read_throw() {
        let agent = agent_with(Some("grep"));
        let c = ceiling(Some(&["grep"]), &["org-policy"]);
        let err = resolve_tool_surface(&agent, true, Some(&c), Some(&[]), no_cwd())
            .expect_err("a host without `read` must refuse a launch that requires it");
        assert!(
            matches!(err, SubagentError::ToolContractUnsatisfiable(_)),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "Host runtime does not provide required tool 'read' for agent 'reviewer' for lazy \
             skill loading."
        );
    }

    /// The moved throw, proven behaviour-preserving at resolver level. `spawn_plan.rs`'s
    /// `a_capability_ceiling_excluding_read_fails_the_launch_when_read_is_required` proves the same
    /// message still reaches the operator through the real plan path.
    #[test]
    fn the_ceiling_read_throw_still_fires_from_the_resolver() {
        let agent = agent_with(Some("grep"));
        let c = ceiling(Some(&["grep"]), &["org-policy"]);
        let err = resolve_tool_surface(&agent, true, Some(&c), None, no_cwd())
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
        let err =
            resolve_tool_surface(&agent, true, Some(&c), None, no_cwd()).expect_err("must refuse");
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
        let surface = resolve_tool_surface_in(
            &agent,
            false,
            None,
            None,
            &fixture.project_dir,
            &fixture.dirs,
        )
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
            None,
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
        let surface = resolve_tool_surface_in(
            &agent,
            false,
            Some(&c),
            None,
            &fixture.project_dir,
            &fixture.dirs,
        )
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
        let surface = resolve_tool_surface_in(
            &agent,
            false,
            None,
            None,
            &fixture.project_dir,
            &fixture.dirs,
        )
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
        let surface = resolve_tool_surface_in(
            &agent,
            false,
            None,
            None,
            &fixture.project_dir,
            &fixture.dirs,
        )
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

        let open =
            resolve_tool_surface_in(&agent, false, None, None, no_cwd(), &unreachable_dirs())
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
            resolve_tool_surface_in(&agent, false, Some(&c), None, no_cwd(), &unreachable_dirs())
                .expect("resolves");
        assert!(
            denied.tool_extension_paths.is_empty(),
            "pi empties the PLAN field under `denyExtensions`, not just the argv it feeds"
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
            resolve_tool_surface_in(&agent, false, Some(&c), None, no_cwd(), &unreachable_dirs())
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
        let err = resolve_tool_surface(&agent, false, None, None, no_cwd()).expect_err(
            "a child holding the PARENT-side supervisor tool with no children must be refused",
        );
        assert_eq!(err.to_string(), FANOUT_REFUSAL);
    }

    /// Both grants clear it: naming `subagent` in `tools:`, and `allowNestedSubagents: true` on its
    /// own (pi's independent grant, `:417-419`).
    #[test]
    fn subagent_supervisor_with_fanout_launches() {
        let declared = agent_with(Some("read, subagent, subagent_supervisor"));
        let surface =
            resolve_tool_surface(&declared, false, None, None, no_cwd()).expect("resolves");
        assert!(surface.fanout_authorized);

        let granted = agent_from_frontmatter(
            "---\nname: reviewer\ndescription: Review things\ntools: read, \
             subagent_supervisor\nallowNestedSubagents: true\n---\n\nBody.\n",
        );
        let surface = resolve_tool_surface(&granted, false, None, None, no_cwd())
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
        let err = resolve_tool_surface(&agent, false, Some(&c), None, no_cwd())
            .expect_err("the ceiling dropped `subagent`, so fanout is revoked");
        assert_eq!(err.to_string(), FANOUT_REFUSAL);

        let rescued = agent_from_frontmatter(
            "---\nname: reviewer\ndescription: Review things\ntools: read, subagent, \
             subagent_supervisor\nallowNestedSubagents: true\n---\n\nBody.\n",
        );
        let err = resolve_tool_surface(&rescued, false, Some(&c), None, no_cwd()).expect_err(
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
        let err = resolve_tool_surface(&agent, false, None, None, no_cwd())
            .expect_err("`excludeTools: [subagent]` revokes the grant `tools:` conferred");
        assert_eq!(err.to_string(), FANOUT_REFUSAL);
    }

    /// **The inverse — this pins the exemption, and it is why the refusal's comment must NOT claim
    /// the host intersection can reach it.** `subagent` is itself in
    /// [`NATIVE_COORDINATION_TOOL_NAMES`], so a host snapshot omitting it prunes nothing and
    /// `fanout_authorized` stays `true`. Upstream is identical (`child-tool-plan.ts:68` lists
    /// `subagent`, and `:408`/`:411` both apply the exemption).
    #[test]
    fn a_host_that_omits_subagent_does_not_revoke_the_supervisor_tool() {
        let agent = agent_with(Some("read, subagent, subagent_supervisor"));
        let host = vec!["read".to_string()];
        let surface = resolve_tool_surface(&agent, false, None, Some(&host), no_cwd())
            .expect("the coordination exemption keeps `subagent`, so fanout survives");
        assert!(surface.fanout_authorized);
        assert!(surface.builtins.iter().any(|t| t == "subagent"));
        assert!(surface.builtins.iter().any(|t| t == "subagent_supervisor"));
        assert!(
            surface.unavailable_host_builtins.is_empty(),
            "an exempt name must never be reported as a host omission either"
        );
    }

    /// The two coordination tools are ASYMMETRIC and the refusal must guard only one.
    /// `subagent_supervisor` is the PARENT-side tool (answering blocked children);
    /// [`crate::native_supervisor::INTERCOM_TOOL_NAME`] is the CHILD->supervisor direction, which
    /// every child needs regardless of fanout. Guarding `intercom` symmetrically would refuse the
    /// launch of the four bundled personas that declare it, on every unfanned-out run.
    #[test]
    fn intercom_is_never_guarded_by_the_fanout_throw() {
        let agent = agent_with(Some("read, intercom"));
        let surface = resolve_tool_surface(&agent, false, None, None, no_cwd())
            .expect("`intercom` is not a fanout-gated capability");
        assert!(!surface.fanout_authorized);
        assert!(surface.builtins.iter().any(|t| t == "intercom"));
    }

    // ---- the review-lane contract and the host-omission warning ----
    //
    // 1:1 port of upstream's `test/unit/child-tool-plan-diagnostics.test.ts` (8 cases). Four of
    // those already live above as TOOLCON_3 tests and were EXTENDED rather than duplicated:
    // `host_availability_unknown_prunes_nothing` (#5a),
    // `a_ceiling_emptied_allowlist_is_not_a_host_omission` (#5c and #6b) and
    // `an_excluded_tool_is_still_reported_as_a_host_omission` (#7b).

    /// The five repository-inspection tools every lane test declares, as `String`s.
    fn repo_tools() -> Vec<String> {
        ["read", "grep", "find", "ls", "bash"]
            .iter()
            .map(|t| (*t).to_string())
            .collect()
    }

    /// Upstream #1: *"fails a review/scout launch when host pruning drops permitted repository
    /// tools"*.
    ///
    /// Built the way upstream builds it — the expectation comes from the formatter itself — PLUS a
    /// literal tail assertion, so a formatter regression cannot move both sides together.
    #[test]
    fn a_review_lane_fails_when_host_pruning_drops_permitted_repository_tools() {
        let agent = agent_named("scout", Some("read, grep, find, ls, bash"));
        let err = resolve_tool_surface(&agent, false, None, Some(&[]), no_cwd())
            .expect_err("a scout with no repository access cannot produce a scout result");
        assert_eq!(
            err.to_string(),
            format_review_lane_tool_contract_failure(
                Some("scout"),
                &repo_tools(),
                Some(&repo_tools()),
                &[],
                &[],
                &[],
            )
        );
        assert!(
            err.to_string().ends_with(
                "This is a lane infrastructure failure, not a completed review/scout result."
            ),
            "{err}"
        );
    }

    /// Upstream #2: *"fails a reviewer when a ceiling-permitted repository tool is host-missing"*.
    ///
    /// The ceiling is built by CALLING [`crate::exec::capability_ceiling::intersect_capability_ceilings`]
    /// rather than by hand-writing its result: `[parent-policy, plan-mode]` and `[read, write]` are
    /// sorted by production code (upstream `capability-ceiling.ts:148,157`), so hand-writing them
    /// would assert this test's arithmetic instead of the crate's.
    ///
    /// Note `Requested tool names:` is the RAW declaration `[read, grep, bash, write]`, not the
    /// ceiling-filtered `[read, write]` — the segment names what was ASKED FOR.
    #[test]
    fn a_reviewer_fails_when_a_ceiling_permitted_repository_tool_is_host_missing() {
        let c = crate::exec::capability_ceiling::intersect_capability_ceilings(&[
            Some(ceiling(Some(&["read", "bash", "write"]), &["plan-mode"])),
            Some(ceiling(
                Some(&["read", "grep", "write"]),
                &["parent-policy"],
            )),
        ])
        .expect("two ceilings intersect");
        assert_eq!(
            c.allowed_tools.as_deref(),
            Some(&["read".to_string(), "write".to_string()][..])
        );
        assert_eq!(c.sources, ["parent-policy", "plan-mode"]);

        let mut agent = agent_with(Some("read, grep, bash, write"));
        agent.exclude_tools = vec!["write".to_string()];
        let host = ["bash".to_string(), "write".to_string()];
        let err = resolve_tool_surface(&agent, false, Some(&c), Some(&host), no_cwd())
            .expect_err("`read` survived the ceiling but the host does not provide it");
        assert_eq!(
            err.to_string(),
            "Agent 'reviewer': tool contract could not be satisfied; host runtime does not provide \
             permitted required repository tools [read]. Requested tool names: [read, grep, bash, \
             write]; effective tool allowlist: []. Active capability ceiling sources: \
             [parent-policy, plan-mode]. Explicit excludeTools: [write]. This is a lane \
             infrastructure failure, not a completed review/scout result."
        );
    }

    /// Upstream #3: *"keeps host-pruned warnings for non-review agents, including an empty
    /// effective menu"*. The expected string is upstream's own, pasted verbatim — it also uses
    /// `worker`, so it transfers unchanged.
    #[test]
    fn host_pruned_warnings_survive_for_non_review_agents() {
        let agent = agent_named("worker", Some("read, grep, find, ls, bash"));
        let surface = resolve_tool_surface(&agent, false, None, Some(&[]), no_cwd())
            .expect("a non-lane agent warns rather than refusing");
        assert_eq!(
            surface.warnings,
            [
                "Agent 'worker': host runtime tool availability omitted [read, grep, find, ls, bash]. \
             Requested tool names: [read, grep, find, ls, bash]; effective tool allowlist: []. \
             This is a non-fatal tool-plan diagnostic, not verification of the child's runtime \
             tool menu."
            ]
        );
        assert!(surface.effective_tool_allowlist().is_empty());
    }

    /// Upstream #4: *"does not invent an explicit request, agent name, or ceiling source when
    /// absent"*. One test, three separate proofs:
    ///
    /// 1. an agent with NO `tools:` key **still warns** — upstream's `input.tools !== undefined`
    ///    guard (`:531`) suppresses only the lane REFUSAL, never this diagnostic. The surface here
    ///    is entirely ceiling-derived, and the host cannot provide it.
    /// 2. the request segment is `not explicitly specified`, not an invented `[]`.
    /// 3. the warning's ceiling-sources rule falls back to `[unknown source]` for empty `sources`.
    ///
    /// **[CYRUP-DELTA]** upstream asserts the subject is `Subagent:` because its `agentName` is
    /// optional. [`crate::exec::agent_config::AgentConfig::name`] is a non-optional `String`, so
    /// the resolver can only ever emit `Agent '<name>'`; the `Subagent` arm is pinned instead by
    /// [`the_diagnostic_subject_names_an_agent_or_falls_back_to_subagent`].
    #[test]
    fn no_explicit_request_or_ceiling_source_is_invented() {
        let agent = agent_with(None);
        let c = ceiling(Some(&["read"]), &[]);
        let surface = resolve_tool_surface(&agent, false, Some(&c), Some(&[]), no_cwd())
            .expect("an unpinned agent has no minimum to miss, so it warns rather than refusing");
        assert_eq!(
            surface.warnings,
            [
                "Agent 'reviewer': host runtime tool availability omitted [read]. Requested tool \
             names: not explicitly specified; effective tool allowlist: []. Active capability \
             ceiling sources: [unknown source]. This is a non-fatal tool-plan diagnostic, not \
             verification of the child's runtime tool menu."
            ]
        );
    }

    /// The `Subagent` arm of [`diagnostic_subject`], which the resolver cannot reach because
    /// `AgentConfig::name` is non-optional. Kept because the ported message is a public formatting
    /// contract: a caller outside this crate may format one without an agent name.
    #[test]
    fn the_diagnostic_subject_names_an_agent_or_falls_back_to_subagent() {
        let missing = vec!["read".to_string()];
        assert!(
            format_review_lane_tool_contract_failure(None, &missing, None, &[], &[], &[])
                .starts_with("Subagent: tool contract could not be satisfied")
        );
        assert!(
            format_review_lane_tool_contract_failure(Some("scout"), &missing, None, &[], &[], &[])
                .starts_with("Agent 'scout': tool contract could not be satisfied")
        );
    }

    /// Upstream #5: *"does not invent host omissions when availability is unknown or a ceiling
    /// alone prunes tools"* — the second input (a host observation that COVERS what was declared).
    /// The first and third inputs are pinned by `host_availability_unknown_prunes_nothing` and
    /// `a_ceiling_emptied_allowlist_is_not_a_host_omission` above.
    #[test]
    fn a_host_that_provides_everything_declared_omits_nothing() {
        let agent = agent_with(Some("read"));
        let host = ["read".to_string()];
        let surface =
            resolve_tool_surface(&agent, false, None, Some(&host), no_cwd()).expect("resolves");
        assert!(surface.unavailable_host_builtins.is_empty());
        assert!(surface.warnings.is_empty());
    }

    /// Upstream #6: *"does not reject an intentionally empty or ceiling-restricted review
    /// allowlist"* — the FIRST case. `Some(vec![])` is the shape `discovery::merge` resolves a
    /// settings `"tools": false` to (see [`ResolvedToolSurface::pinned`]'s doc): pinned-and-empty,
    /// the exact opposite grant from unpinned. A `scout` that asked for nothing cannot be missing
    /// anything.
    #[test]
    fn an_intentionally_empty_review_allowlist_is_not_rejected() {
        let mut agent = agent_named("scout", None);
        agent.tools = Some(Vec::new());
        let surface = resolve_tool_surface(&agent, false, None, Some(&[]), no_cwd())
            .expect("an empty allowlist declares no repository requirement");
        assert!(surface.pinned);
        assert!(surface.builtins.is_empty());
        assert!(surface.effective_tool_allowlist().is_empty());
        assert!(surface.unavailable_host_builtins.is_empty());
        assert!(surface.warnings.is_empty());
    }

    /// Upstream #7: *"does not treat excluded repository tools as a missing review-lane
    /// contract"* — the FIRST case, where the host provides everything and the exclusion is the
    /// only subtraction. The second case is
    /// `an_excluded_tool_is_still_reported_as_a_host_omission` above.
    #[test]
    fn excluded_repository_tools_are_not_a_missing_lane_contract() {
        let mut agent = agent_named("scout", Some("read, grep, bash"));
        agent.exclude_tools = vec!["read".to_string(), "grep".to_string()];
        let host = ["read".to_string(), "grep".to_string(), "bash".to_string()];
        let surface =
            resolve_tool_surface(&agent, false, None, Some(&host), no_cwd()).expect("resolves");
        assert_eq!(surface.effective_tool_allowlist(), ["bash"]);
        assert!(surface.unavailable_host_builtins.is_empty());
        assert!(surface.warnings.is_empty());
    }

    /// Upstream #8: *"keeps a scout launch when only a non-repository requested tool is
    /// host-pruned"*. `write` is NOT in [`REPOSITORY_INSPECTION_TOOLS`], so its absence warns but
    /// does not breach the lane contract — the intersection in
    /// [`missing_permitted_repository_inspection_tools`] is what makes the refusal narrow.
    #[test]
    fn a_scout_launch_survives_a_host_pruned_non_repository_tool() {
        let agent = agent_named("scout", Some("read, grep, find, ls, bash, write"));
        let surface = resolve_tool_surface(&agent, false, None, Some(&repo_tools()), no_cwd())
            .expect("`write` is not a repository-inspection tool");
        assert_eq!(surface.effective_tool_allowlist(), repo_tools());
        assert_eq!(surface.unavailable_host_builtins, ["write"]);
        assert_eq!(surface.warnings.len(), 1, "{:?}", surface.warnings);
        assert!(
            surface.warnings[0].contains("host runtime tool availability omitted [write]"),
            "{}",
            surface.warnings[0]
        );
    }

    /// `/\b(?:reviewer|scout)\b/i`, ported through `task_intent`'s `boundary_before` + `alt_word`.
    /// `researcher` and `oracle` are the two bundled personas nearest the boundary — `researcher`
    /// is matched by `task_intent`'s OTHER pattern (`research(?:er)?`) and must not be dragged into
    /// this one; `discover` guards against an unanchored substring search.
    #[test]
    fn the_lane_pattern_is_word_bounded_and_case_insensitive() {
        for name in [
            "reviewer",
            "code-reviewer",
            "Scout",
            "my-scout-agent",
            "REVIEWER",
        ] {
            assert!(is_review_or_scout_lane_agent(name), "{name}");
        }
        for name in [
            "scouting",
            "discover",
            "worker",
            "researcher",
            "oracle",
            "delegate",
        ] {
            assert!(!is_review_or_scout_lane_agent(name), "{name}");
        }
    }

    /// The ceiling-sources ASYMMETRY, both directions in one place.
    ///
    /// The FAILURE omits the segment entirely when `sources` is empty (pi `:94`'s `?.length`); the
    /// WARNING emits it whenever a ceiling exists at all, falling back to `[unknown source]` (pi
    /// `:552`'s `|| "unknown source"`, where the JS `||` fires on the empty string). Same ceiling,
    /// two different renderings — upstream-observable, and easy to "tidy" into one rule.
    #[test]
    fn the_failure_omits_an_empty_ceiling_sources_segment() {
        let agent = agent_with(Some("read, grep"));
        let c = ceiling(Some(&["read", "grep"]), &[]);
        let err = resolve_tool_surface(&agent, false, Some(&c), Some(&[]), no_cwd())
            .expect_err("a reviewer with no repository access is refused");
        assert!(
            !err.to_string()
                .contains("Active capability ceiling sources:"),
            "the FAILURE omits the segment for empty `sources`: {err}"
        );
        assert_eq!(
            err.to_string(),
            "Agent 'reviewer': tool contract could not be satisfied; host runtime does not provide \
             permitted required repository tools [read, grep]. Requested tool names: [read, \
             grep]; effective tool allowlist: []. This is a lane infrastructure failure, not a \
             completed review/scout result."
        );

        // ...while the WARNING for the very same empty-`sources` ceiling DOES emit one. Driven off
        // a non-lane agent so the launch survives to produce a warning at all.
        let worker = agent_named("worker", Some("read, grep"));
        let surface = resolve_tool_surface(&worker, false, Some(&c), Some(&[]), no_cwd())
            .expect("a non-lane agent warns");
        assert!(
            surface.warnings[0].contains("Active capability ceiling sources: [unknown source]. "),
            "the WARNING emits the segment with a fallback: {}",
            surface.warnings[0]
        );
    }

    /// **The assertion TOOLCON_4's fold exists to make possible.** `effective tool allowlist:`
    /// interpolates [`ResolvedToolSurface::effective_tool_allowlist`], which is
    /// `builtins ∪ effective_mcp_tools` — so a resolved direct-MCP name reaches the diagnostic.
    /// Under the pre-fold factoring the MCP half lived in `spawn_plan` and this segment would have
    /// named only `[read]`, describing a child that actually launches with the MCP tool too.
    ///
    /// `worker`, not a lane name: `grep` is a repository-inspection tool, so a `reviewer` here
    /// would be refused before any warning was produced.
    #[test]
    fn the_warning_names_resolved_mcp_tools_in_the_effective_allowlist() {
        let fixture = github_fixture();
        let agent = agent_named("worker", Some("read, grep, mcp:github/search_repositories"));
        let host = ["read".to_string()];
        let surface = resolve_tool_surface_in(
            &agent,
            false,
            None,
            Some(&host),
            &fixture.project_dir,
            &fixture.dirs,
        )
        .expect("a non-lane agent warns");
        assert_eq!(surface.unavailable_host_builtins, ["grep"]);
        assert_eq!(
            surface.effective_tool_allowlist(),
            ["read".to_string(), "github_search_repositories".to_string()]
        );
        assert_eq!(
            surface.warnings,
            [
                "Agent 'worker': host runtime tool availability omitted [grep]. Requested tool names: \
             [read, grep, github_search_repositories]; effective tool allowlist: [read, \
             github_search_repositories]. This is a non-fatal tool-plan diagnostic, not \
             verification of the child's runtime tool menu."
            ]
        );
    }

    /// A host observation that covers everything declared adds nothing to the wire: `warnings` and
    /// `unavailable_host_builtins` both carry `skip_serializing_if = "Vec::is_empty"`, so a clean
    /// run's payload is byte-identical to what it was before either field existed.
    #[test]
    fn a_clean_run_adds_no_warning_to_the_wire() {
        let host: Vec<String> = HOST_BUILTIN_TOOL_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        let agent = agent_with(Some("read, grep"));
        let surface =
            resolve_tool_surface(&agent, false, None, Some(&host), no_cwd()).expect("resolves");
        assert!(surface.warnings.is_empty());
        assert!(surface.unavailable_host_builtins.is_empty());
        let json = serde_json::to_string(&surface).expect("serializes");
        assert!(!json.contains("\"warnings\""), "{json}");
        assert!(!json.contains("unavailableHostBuiltins"), "{json}");
    }

    #[test]
    fn host_omissions_reach_the_parent_under_their_camel_case_wire_name() {
        let surface = ResolvedToolSurface {
            unavailable_host_builtins: vec!["bash".to_string()],
            ..pinned(&["read"])
        };
        let json = serde_json::to_string(&surface).expect("serializes");
        assert!(
            json.contains("\"unavailableHostBuiltins\":[\"bash\"]"),
            "{json}"
        );
    }

    #[test]
    fn the_default_surface_is_omitted_from_the_wire() {
        assert!(is_default_surface(&ResolvedToolSurface::default()));
        assert!(!is_default_surface(&reviewer()));
        let json = serde_json::to_string(&reviewer()).unwrap();
        assert!(json.contains("\"builtins\""), "{json}");
        assert!(json.contains("\"pinned\":true"), "{json}");
        assert!(!json.contains("\"excluded\""), "{json}");
        // The two fields TOOLCON_3 added carry `skip_serializing_if = "Vec::is_empty"`, so a clean
        // surface's payload stays byte-identical to what it was before they existed.
        assert!(!json.contains("unavailableHostBuiltins"), "{json}");
        assert!(!json.contains("\"warnings\""), "{json}");
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
        let surface = resolve_tool_surface(&agent, false, None, None, no_cwd()).expect("resolves");

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
        let surface = resolve_tool_surface(&plain, false, None, None, no_cwd()).expect("resolves");
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
        let surface =
            resolve_tool_surface(&excluded, false, None, None, no_cwd()).expect("resolves");
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
        let surface = super::resolve_tool_surface(&agent, false, None, None, true, no_cwd())
            .expect("resolves");
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
        let surface = super::resolve_tool_surface(&excluded, false, None, None, true, no_cwd())
            .expect("resolves");
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
        let surface = super::resolve_tool_surface(&agent, false, None, None, false, no_cwd())
            .expect("resolves");
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
                .unwrap_or_else(|| {
                    panic!(
                        "{} declares no supervisor tool at all; the bundled personas are the \
                         reason `NATIVE_COORDINATION_TOOL_NAMES` carries four names",
                        path.display()
                    )
                })
                .to_string();

            let surface =
                resolve_tool_surface(&agent, false, None, None, no_cwd()).expect("resolves");
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
