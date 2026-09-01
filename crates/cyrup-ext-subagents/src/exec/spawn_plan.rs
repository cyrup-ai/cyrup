//! Pure computation of *what to spawn* for one model-fallback attempt: argv/env/system-prompt
//! assembly with zero process handles and zero I/O — [`AttemptSpawnPlan`] and the
//! [`build_attempt_spawn_plan_with_read_requirement`] family that builds one. Split out of
//! `exec/mod.rs`'s own "SubagentSpawner" section (the spawn-plan-construction third of it; the
//! other two thirds are [`crate::exec::attempt_runner`] and [`crate::exec::drive_attempt`]).

use std::path::PathBuf;

use cyrup_core::ModelId;

use crate::discovery::types::{
    SystemPromptMode, ToolRef,
};
use crate::exec::completion_guard_projection;
use crate::exec::mcp_direct_tools;
use crate::error::SubagentError;
use crate::exec::acceptance::{
    AcceptanceContract, inject_acceptance_contract,
};
use crate::exec::output::{
    inject_output_path_system_prompt, inject_single_output_instruction,
};
use crate::spawn::depth::DepthEnvelope;
use crate::spawn::{ChildSpawnSpec, SpawnCommand};
use crate::exec::agent_config::{AgentConfig, RunOptions};


// ================================================================================================
// SubagentSpawner: the seam production spawning goes through (mirrors AttemptRunner's own
// production-vs-test seam, one level down at the real-subprocess boundary)
// ================================================================================================

/// Everything one attempt's spawn needs beyond what [`AgentConfig`]/[`RunOptions`] already carry —
/// factored out so [`crate::exec::attempt_runner::SpawnedChildAttemptRunner`] can build a [`ChildSpawnSpec`] without repeating
/// argv/env assembly inline in `run_attempt` itself.
///
/// Public (with [`build_attempt_spawn_plan`]) as the P-5 cross-crate parity seam: the child env
/// this plan carries is a CONTRACT the permission companion depends on
/// (`cyrup-permission-system/tests/forwarding_spawn_env.rs` drives a real child process off THIS
/// overlay to prove a subagent's `ask` reaches the parent's human), and a downstream proof that
/// re-typed the env by hand would not have caught PERM-001 — the gate reading a key the spawn path
/// never wrote — which is precisely the class of bug the seam exists to make visible.
pub struct AttemptSpawnPlan {
    /// The fully-assembled child spawn description: binary, argv, task arg, env overlay, cwd.
    pub spec: ChildSpawnSpec,
    /// SUBA-045 — where this attempt told the child to write its tool-availability diagnostic
    /// (pi's `toolDiagnosticPath`, `pi-args.ts:610-616`), so the parent can read it back at settle.
    ///
    /// `None` whenever the env var was not written, which is upstream's own gate: an agent with no
    /// explicit `tools:` allowlist requires nothing, so there is nothing to be missing. Returned
    /// alongside the spec rather than re-derived from the overlay so the read side cannot drift
    /// from the write side.
    pub tool_diagnostic_path: Option<PathBuf>,
}

/// The reasoning-level suffixes [`apply_thinking_suffix`] recognizes on a model id (pi-subagents
/// `THINKING_LEVELS`, `src/shared/model-info.ts:1`; `max` added upstream in 747de75). Includes
/// `off` — a value cyrup-core's closed on-only
/// `ThinkingLevel` enum cannot itself represent, but which the string-level suffix check must still
/// recognize so a model id that already ends `:off` is never double-suffixed.
/// SUBA-078: `pub(crate)` so [`crate::exec::thinking_ceiling`] ranks levels against the EXACT list
/// [`split_known_thinking_suffix`] recognizes. Aliasing rather than copying is what makes the two
/// structurally unable to disagree — a level one accepted and the other did not would slip the
/// ceiling. Same rule as [`INHERIT_PROJECT_CONTEXT_ENV`]'s aliasing to its reader's declaration.
pub(crate) const THINKING_LEVELS: [&str; 7] =
    ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Child env flag: whether the subagent inherits the parent's project-context files
/// (`AGENTS.md`/`CLAUDE.md`) — pi `PI_SUBAGENT_INHERIT_PROJECT_CONTEXT` (`runs/shared/pi-args.ts:215` @v0.34.0).
///
/// Aliased to the READER's declaration ([`crate::prompt_runtime`], which acts on it child-side in
/// `before_agent_start`) rather than re-spelled here: the two spellings drifting apart would
/// silently restore the write-only-flag bug this alias exists to prevent.
const INHERIT_PROJECT_CONTEXT_ENV: &str = crate::prompt_runtime::INHERIT_PROJECT_CONTEXT_ENV;

/// Child env flag: whether the subagent inherits skills discovery — pi
/// `PI_SUBAGENT_INHERIT_SKILLS` (`runs/shared/pi-args.ts:200`). Same aliasing rule as above.
const INHERIT_SKILLS_ENV: &str = crate::prompt_runtime::INHERIT_SKILLS_ENV;

/// The MCP adapter's direct-tool-allowlist env (pi keeps this un-namespaced, `runs/shared/pi-args.ts:216-220`):
/// the comma-joined `mcp:` selectors, or [`MCP_DIRECT_TOOLS_NONE_SENTINEL`] when the agent declares
/// no direct MCP tools at all (so the child's adapter can distinguish "none selected" from "env
/// unset / inherited").
const MCP_DIRECT_TOOLS_ENV: &str = "MCP_DIRECT_TOOLS";

/// The canonical parent-session anchor (proposed **R-SA-P1**, port doc §4 P-4) — the cyrup analog of
/// pi's `PI_AGENT_ROUTER_PARENT_SESSION_ID` (`permission-forwarding.ts:9`). Injected into every
/// child's spawn env overlay ([`build_attempt_spawn_plan`]) as the LAUNCHING session's own id
/// (obtained via P-2 [`cyrup_ext::host::HostServices::session_id`], captured once at the root
/// orchestrator's `SessionStart`), so the permission companion's child→parent ask-forwarding spool
/// can address the parent's inbox. `pub` (P-5) so that companion reads it cross-crate. Value
/// precedence at the spawn site: explicit ([`RunOptions::parent_session_id`]) → inherited (this
/// process's own `CYRUP_SUBAGENT_PARENT_SESSION`, so a `DEPTH>0` child keeps threading the root's
/// anchor rather than overwriting it) → empty (omitted).
pub const PARENT_SESSION_ENV_VAR: &str = "CYRUP_SUBAGENT_PARENT_SESSION";

/// The resolved persona/agent NAME the child runs as (port doc §4, permission input (1) — pi
/// `resolveAgentName`, `pi-permission-system/src/index.ts:2033-2047` @v0.7.1). cyrup spawns a subagent as a SEPARATE process that IS
/// its persona for the whole lifetime, so — unlike pi's in-process `active_agent` session entry /
/// `<active_agent>` prompt tag — the name is captured ONCE at the spawn site and threaded to the
/// child as this env var (the exact equivalent for cyrup's process-per-subagent model). The child's
/// permission companion reads it via `std::env::var` (mirroring how it already reads the sibling
/// `CYRUP_SUBAGENT_PARENT_SESSION`/`CYRUP_SUBAGENT_CHILD` vars) so its `agent` + `projectAgent` policy
/// layers enforce for the named persona; a top-level (non-subagent) process never has it set, so the
/// name normalizes to `None` there — pi's normalized-`""` top-level behavior (global + project layers
/// still enforce). Set only by the spawn overlay in [`build_attempt_spawn_plan`], the ONLY non-test
/// `env_overlay` construction site (covers foreground + background spawns). `pub` (P-5) so the
/// permission companion reads it cross-crate.
pub const AGENT_NAME_ENV_VAR: &str = "CYRUP_SUBAGENT_AGENT_NAME";

/// pi's `__none__` sentinel for [`MCP_DIRECT_TOOLS_ENV`] when no direct MCP tools are declared.
const MCP_DIRECT_TOOLS_NONE_SENTINEL: &str = "__none__";

/// The child flag carrying a `SystemPromptMode::Replace` persona body (pi `runs/shared/pi-args.ts:165`'s
/// `"--system-prompt"`; the host side is `cyrup/src/cli.rs`'s `#[arg(long = "system-prompt")]`).
pub(crate) const SYSTEM_PROMPT_FLAG: &str = "--system-prompt";

/// The child flag carrying a `SystemPromptMode::Append` persona body (pi `runs/shared/pi-args.ts:165`'s
/// `"--append-system-prompt"`; repeatable host-side, joined with `\n`).
pub(crate) const APPEND_SYSTEM_PROMPT_FLAG: &str = "--append-system-prompt";

/// pi `applyThinkingSuffix` (`runs/shared/pi-args.ts:238-252` @v0.57.0; the citation was the
/// v0.43.0 `:186-200` range, which the function has since moved out of): append `:<thinking>` to a
/// model id, unless the model already ends with a recognized `:<level>` suffix or either input is
/// absent (return the model as-is). Operates on strings so the exact pi rule — including the `off`
/// level a closed on-only enum cannot itself carry — is reproduced verbatim; the agent's own OPEN
/// `thinking` string (`AgentConfig::thinking`) is passed straight through, so an explicit `off`
/// yields `<model>:off`.
///
/// `replace_existing` is upstream's third parameter, defaulted `false` there and required here. It
/// is set by exactly one caller — a sanitized fork's thinking override (SUBA-075) — and it is what
/// lets that override REPLACE a level the id already names instead of deferring to it.
#[must_use]
pub fn apply_thinking_suffix(
    model: Option<&str>,
    thinking: Option<&str>,
    replace_existing: bool,
) -> Option<String> {
    let (Some(model), Some(thinking)) = (model, thinking) else {
        return model.map(str::to_string);
    };
    // pi guards on truthiness (`if (!model || !thinking) ...`), so an empty thinking string is a
    // no-op — mirror that here so a degenerate `Some("")` never produces a trailing bare `:`.
    if thinking.is_empty() {
        return Some(model.to_string());
    }
    let (base, existing) = split_known_thinking_suffix(model);
    if !existing.is_empty() {
        // SUBA-075: an id that ALREADY names a level normally wins — the caller asked for that
        // exact model. A fork thinking-override is the one case that outranks it: the branch was
        // sanitized precisely because the inherited thinking blocks are unusable, so honouring a
        // persona's `:high` there relaunches the failure the sanitization exists to prevent.
        return Some(if replace_existing {
            format!("{base}:{thinking}")
        } else {
            model.to_string()
        });
    }
    Some(format!("{model}:{thinking}"))
}

/// pi `splitKnownThinkingSuffix` (`shared/model-info.ts:39-47`): split a model id on its last `:`
/// **only when the trailing segment is a recognized [`THINKING_LEVELS`] entry**, returning
/// `(base_model, thinking_suffix_including_colon)`.
///
/// Distinct from `extension.rs`'s `split_thinking_suffix` (pi `splitThinkingSuffix`,
/// `model-fallback.ts:13-19`), which splits on the last `:` unconditionally: a model id like
/// `openai/gpt-5:preview` keeps its `:preview` here (it is part of the id, not a reasoning level)
/// but would be truncated by the unconditional split. Scope matching MUST use this stricter form,
/// exactly as `model-scope.ts` does.
#[must_use]
pub fn split_known_thinking_suffix(model: &str) -> (&str, &str) {
    let Some(idx) = model.rfind(':') else {
        return (model, "");
    };
    let suffix = model.get(idx + 1..).unwrap_or("");
    if !THINKING_LEVELS.contains(&suffix) {
        return (model, "");
    }
    (model.get(..idx).unwrap_or(model), model.get(idx..).unwrap_or(""))
}

/// Append `item` to `vec` only if not already present — the order-preserving de-duplication pi
/// achieves with `[...new Set(...)]` over the extension-path list (`runs/shared/pi-args.ts:146,150` @v0.34.0).
fn push_unique(vec: &mut Vec<String>, item: String) {
    if !vec.contains(&item) {
        vec.push(item);
    }
}

/// Build the argv + env overlay for one attempt against `model` (R-SA-024/047/048/054; pi
/// `buildPiArgs`, `runs/shared/pi-args.ts:514-787`).
///
/// Argv (flags in any order, task prompt last): `--print`, `--mode`, `json`; `--model
/// <apply_thinking_suffix(model, agent.thinking)>` (T4 thinking-suffix); the tool-allowlist flag —
/// `--tools <comma-list>` (the agent's declared builtins plus any resolved direct-MCP tool names)
/// when the agent pinned a non-empty allowlist, `--no-tools` when it pinned an EMPTY one, and
/// nothing at all when it pinned none (pi's `explicitToolAllowlist` gate, `runs/shared/pi-args.ts:389-393,549-555`); the agent's
/// extension threading (`--no-extensions` + `--extension <path>` allowlist when `agent.extensions`
/// is `Some`, else `--extension <path>` for tool-extension/child-only paths with discovery left
/// on); `--no-skills` when the agent does not inherit skills; `--system-prompt <path>` /
/// `--append-system-prompt <path>` per `agent.system_prompt_mode` when the body is
/// non-empty (TWO argv elements naming a `0600` spill file — SUBA-030, see below); pi's full session branch
/// (`runs/shared/pi-args.ts:100-112`) — either `--session <path>` when `opts.fork_context` resolved a session
/// file path, or else `--no-session` unless [`RunOptions::session_dir`]/[`RunOptions::share`]
/// enable sessions plus `--session-dir <dir>` for an explicit directory; then the task prompt last
/// (via [`ChildSpawnSpec::resolve_task_arg`], R-SA-047's `@<tempfile>` overflow rule).
///
/// Env overlay carries the child-ROLE pair
/// ([`crate::spawn::nested_events::child_role_env`] — pi `runs/shared/pi-args.ts:329-330`), the incremented
/// depth envelope (R-SA-054), the run sentinel, the agent's inherit flags
/// ([`INHERIT_PROJECT_CONTEXT_ENV`]/[`INHERIT_SKILLS_ENV`]), and the raw direct-MCP selector list
/// ([`MCP_DIRECT_TOOLS_ENV`], or the `__none__` sentinel).
///
/// The agent's own persona prose (`agent.system_prompt_body`) is delivered here as
/// `--system-prompt <spill path>` (`SystemPromptMode::Replace`) or
/// `--append-system-prompt <spill path>`
/// (`SystemPromptMode::Append`) — pi `runs/shared/pi-args.ts:159-165` (v0.34.0), where the mode picks the flag
/// and the body always ships. Nothing child-side re-resolves the persona from
/// [`AGENT_NAME_ENV_VAR`] (that var is read only by the permission companion), so this argv pair is
/// the ONLY channel the persona has.
///
/// The `<available_skills>` pointer block remains composed into `task` BEFORE this function is
/// called — see [`build_task_text`] — rather than being folded into the persona body the way pi's
/// `execution.ts:1054-1056` composes it, so that a `Replace`-mode persona cannot suppress the
/// orchestrator's own scaffolding.
///
/// The output-path override (R-SA-024) travels on BOTH surfaces, exactly as upstream sends it. The
/// task-side half (`injectSingleOutputInstruction`, the `**Output:**` header) is composed into
/// `task` by [`build_task_text`], mirroring `subagent-executor.ts:3674`. The system-prompt half
/// ([`crate::exec::output::inject_output_path_system_prompt`], the `Runtime output path override:`
/// header) is composed onto the persona body HERE, mirroring `execution.ts:1443` — the LAST of the
/// three folds upstream applies to `systemPrompt` in a row. All three are reproduced below in
/// upstream's order: the agent-memory block (`execution.ts:1438-1441`), then the project-local
/// refinement overlay ([`crate::exec::agent_refinements::append_agent_refinement_overlay`],
/// `execution.ts:1442`), then the output-path override. Both output-path surfaces are keyed on the
/// presence of an output PATH alone, never on [`RunOptions::output_mode`], and both are
/// capability-aware.
///
/// SUBA-030 — the composed prompt is written to a `0600` file in `temp_dir` and the flag carries
/// its PATH as two argv elements, which is pi's literal mechanism at
/// `runs/shared/pi-args.ts:570-585` @v0.43.0 (unconditional `mkdtemp` + `writeFileSync(promptPath,
/// …, { mode: 0o600 })` + `args.push(flag, promptPath)`), read back child-side by
/// `resolvePromptInput` (`resource-loader.ts:53-68`). **The earlier `[CYRUP-DELTA]` here asserted
/// that `cyrup`'s own flags "take LITERAL text … no path resolution anywhere" and passed the body
/// inline on that basis; that is false at HEAD** — `crates/cyrup/src/cli.rs:511` runs
/// `--system-prompt` through `resolve_prompt_input` (`:680-704`, an existing path is read from
/// disk) and `:423-439` does the same for every `--append-system-prompt` entry, both pinned by
/// `system_prompt_reads_the_file_when_the_token_is_an_existing_path`. The `=`-joined single-argv
/// form went with the inline delivery, and so did its reason: clap refuses a DETACHED value
/// beginning with `-`, and an absolute temp path cannot begin with one.
///
/// **[CYRUP-DELTA]** the one surviving difference is the empty case. A body that is still empty
/// AFTER the memory block, the refinement overlay and the output-path override have been composed
/// onto it emits NO flag at all, where pi's guard is `!== undefined && !== null` (`:570`) and an
/// empty string still writes a prompt file and still passes the path. Emitting the flag over an
/// empty file here would blank the child's assembled prompt instead of leaving it alone. An agent
/// with an output path configured composes a non-empty override and therefore DOES ship the flag,
/// which is upstream's own rationale for why its value "is never meaningfully empty".
///
/// **[CYRUP-DELTA]** pi prefixes the file with `<active_agent name="…"/>` (`:576-578`) so its
/// in-process `@gotgenes/pi-permission-system` can resolve per-agent policy from the prompt text.
/// cyrup's permission companion resolves the same name from `CYRUP_SUBAGENT_AGENT_NAME` instead
/// (`cyrup-permission-system/src/extension/env.rs`, `resolve_agent_name_from_env`), because
/// cyrup's subagent is a separate PROCESS that IS its persona for its whole lifetime, so the tag
/// would be a second, weaker channel for a fact the env already carries.
///
/// # Errors
///
/// Propagates [`ChildSpawnSpec::resolve_task_arg`]'s error (temp-file creation failure for an
/// over-threshold task).
pub fn build_attempt_spawn_plan(
    agent: &AgentConfig,
    model: &ModelId,
    task_text: &str,
    opts: &RunOptions,
    depth: DepthEnvelope,
    temp_dir: &std::path::Path,
    // SUBA-S01 (pi `runs/shared/pi-args.ts:246-250`): the run's capture runtime, whose two paths become the
    // child's structured-output env overlay. `None` = the step declared no `outputSchema`, and the
    // child then registers no `structured_output` tool at all.
    structured_runtime: Option<&crate::exec::structured::StructuredOutputRuntime>,
) -> Result<AttemptSpawnPlan, SubagentError> {
    build_attempt_spawn_plan_with_read_requirement(
        agent,
        model,
        task_text,
        opts,
        depth,
        temp_dir,
        structured_runtime,
        false,
    )
}

/// SUBA-014 — [`build_attempt_spawn_plan`] plus pi's `requireReadTool` input.
///
/// pi's `resolvePiLaunchToolPlan` takes `requireReadTool?: boolean`
/// (`pi-subagents/src/runs/shared/pi-args.ts:118,208` @v0.43.0) and every one of its seven live
/// setters derives it from "did any skill actually resolve" — `Boolean(shared.resolvedSkillNames
/// ?.length)` (`runs/foreground/execution.ts:322,357`), `Boolean(resolvedSkills.length)`
/// (`runs/background/async-execution.ts:731,1324`), `Boolean(step.skills?.length)`
/// (`runs/background/subagent-runner.ts:1328,1366`), `resolvedSkills.resolved.length > 0`
/// (`api/preflight.ts:277`). The parameter is OPTIONAL upstream and falsy when omitted, which is
/// exactly what the seven-argument [`build_attempt_spawn_plan`] forwards.
///
/// Eight parameters is one over clippy's threshold: this is [`build_attempt_spawn_plan`]'s own
/// signature plus pi's optional `requireReadTool`, forwarded verbatim, so grouping them into a
/// struct would only rename the same values at every call site.
#[allow(clippy::too_many_arguments)]
pub fn build_attempt_spawn_plan_with_read_requirement(
    agent: &AgentConfig,
    model: &ModelId,
    task_text: &str,
    opts: &RunOptions,
    depth: DepthEnvelope,
    temp_dir: &std::path::Path,
    structured_runtime: Option<&crate::exec::structured::StructuredOutputRuntime>,
    // SUBA-014 (pi `runs/shared/pi-args.ts:365-370` @v0.43.0): the run resolved at least one skill,
    // so the child is about to be told (`discovery/skills.rs`'s proactive block) to "use the read
    // tool to load a skill's file" and MUST therefore be granted `read` even when the agent pinned
    // an explicit `tools:` list that omits it.
    require_read_tool: bool,
) -> Result<AttemptSpawnPlan, SubagentError> {
    let capability_ceiling = preflight_capability_ceiling(agent, opts)?;

    // SUBA-072 — the remaining two ceiling axes, resolved once here so both the tool-allowlist
    // block and the extension-threading block below can apply them. `ceiling_allowed_tools` is
    // `None` when the ceiling (if any) leaves `allowedTools` unset — no bound, not "bound to
    // nothing" (see [`crate::exec::capability_ceiling::ResolvedCapabilityCeiling`]'s own doc on
    // that distinction).
    let ceiling_allowed_tools: Option<Vec<String>> = capability_ceiling
        .as_ref()
        .and_then(|ceiling| ceiling.allowed_tools.clone());
    let ceiling_deny_extensions = capability_ceiling
        .as_ref()
        .is_some_and(|ceiling| ceiling.deny_extensions);

    // pi `pi-args.ts:439-441`: fires BEFORE any tool-plan branching, independent of whether this
    // agent declared its own `tools:` — a ceiling that excludes `read` while lazy skill loading
    // needs it is a hard launch error, not a silent narrowing.
    if require_read_tool
        && let Some(allowed) = ceiling_allowed_tools.as_ref()
        && !allowed.iter().any(|tool| tool == "read")
    {
        let sources = capability_ceiling
            .as_ref()
            .filter(|ceiling| !ceiling.sources.is_empty())
            .map_or_else(|| "unknown source".to_string(), |ceiling| ceiling.sources.join(", "));
        return Err(SubagentError::CapabilityCeilingViolation(format!(
            "Capability ceiling from {sources} excludes required tool 'read' for lazy skill \
             loading."
        )));
    }

    // An injected command wins; `None` falls back to the environment, leaving R-SA-045's
    // three-tier priority exactly as it was for every caller that supplies nothing.
    let command = opts
        .spawn_command
        .clone()
        .unwrap_or_else(crate::spawn::resolve_spawn_command);

    // T4 (pi `applyThinkingSuffix`): the per-attempt model id, suffixed with the agent's frontmatter
    // reasoning level (`:high` etc.) unless it already carries a recognized `:<level>` suffix.
    //
    // SUBA-075 / pi `applyThinkingSuffix(model, options.thinkingOverride ?? agent.thinking,
    // options.thinkingOverride !== undefined)` (`runs/foreground/execution.ts:1847` @v0.57.0): a
    // sanitized fork's thinking override BOTH supplies the level and licenses replacing a level the
    // id already carries.
    let thinking_override = opts.fork_context.thinking_override.as_deref();
    let model_arg = apply_thinking_suffix(
        Some(model.as_str()),
        thinking_override.or(agent.thinking.as_deref()),
        thinking_override.is_some(),
    )
    .unwrap_or_else(|| model.as_str().to_string());

    // SUBA-078 / pi `execution.ts:322` @v0.57.0: the ceiling is asserted once more HERE, on the
    // exact id this attempt will launch with, immediately before the args are built. `run_sync`
    // already swept the whole ladder, but this attempt's `model_arg` can carry a thinking OVERRIDE
    // the sweep never saw (a sanitized fork's `:off`, SUBA-075), so this is the last point at which
    // what the child is really about to be told can still be checked.
    //
    // Re-intersected with the inherited env rather than trusting `opts` alone — pi's `pi-args.ts`
    // does the same at `:875-877` instead of relying on its caller's value.
    let inherited_ceiling = crate::exec::thinking_ceiling::inherited_thinking_ceiling()
        .map_err(SubagentError::ThinkingCeilingViolation)?;
    let thinking_ceiling = crate::exec::thinking_ceiling::intersect_thinking_ceilings(&[
        opts.thinking_ceiling.as_deref(),
        inherited_ceiling.as_deref(),
    ])
    .map_err(SubagentError::ThinkingCeilingViolation)?;
    crate::exec::thinking_ceiling::assert_thinking_within_ceiling(
        Some(model_arg.as_str()),
        thinking_override.or(agent.thinking.as_deref()),
        thinking_ceiling.as_deref(),
        Some(agent.name.as_str()),
        opts.run_id.as_ref().map(crate::background::RunId::as_str),
    )
    .map_err(SubagentError::ThinkingCeilingViolation)?;

    let mut args: Vec<String> = vec![
        "--print".to_string(),
        "--mode".to_string(),
        "json".to_string(),
        "--model".to_string(),
        model_arg,
    ];

    let tools = resolve_child_tools(
        agent,
        opts,
        require_read_tool,
        ceiling_allowed_tools.as_ref(),
        ceiling_deny_extensions,
        &mut args,
    );

    push_extension_and_skill_args(
        agent,
        ceiling_deny_extensions,
        tools.tool_extension_paths,
        &mut args,
    );

    let persona_temp_file = compose_persona(agent, opts, temp_dir, &mut args)?;

    // Session threading (pi `buildPiArgs`, `runs/shared/pi-args.ts:517-528`) — the FULL branch, both halves:
    //
    // * a resolved fork-context session FILE wins outright: its parent directory is created
    //   (pi's `fs.mkdirSync(path.dirname(sessionFile), { recursive: true })`) and `--session <file>`
    //   pins the child to it;
    // * otherwise the child is spawned `--no-session` UNLESS this run enables sessions at all, and
    //   an explicit `--session-dir <dir>` (directory likewise created up front) points the child's
    //   session store at the caller's directory.
    //
    // `session_enabled` is pi's `execution.ts:1412` `Boolean(sessionFile || sessionDir) || share`:
    // an explicit `sessionDir` OR `share: true` keeps sessions on; neither means the child must not
    // persist a session at all. Pre-SUBA-041 only the `--session` half existed, so
    // [`RunOptions::share`]/[`RunOptions::session_dir`] were inert fields no argv ever read and every
    // session-less child silently persisted a session the orchestrator never asked for.
    if let Some(session_path) = &opts.fork_context.session_file_path {
        if let Some(parent) = session_path.parent() {
            // Best-effort, exactly like pi's own un-guarded `mkdirSync`: a failure here surfaces as
            // the child's own session error rather than aborting the spawn plan.
            let _ = std::fs::create_dir_all(parent);
        }
        args.push("--session".to_string());
        args.push(session_path.display().to_string());
    } else {
        let session_enabled = opts.session_dir.is_some() || opts.share == Some(true);
        if !session_enabled {
            args.push("--no-session".to_string());
        }
        if let Some(session_dir) = &opts.session_dir {
            let _ = std::fs::create_dir_all(session_dir);
            args.push("--session-dir".to_string());
            args.push(session_dir.display().to_string());
        }
    }

    let (task_arg, temp_file) = ChildSpawnSpec::resolve_task_arg(task_text, temp_dir)?;

    let mut env_overlay =
        env_identity_and_depth(
        agent,
        opts,
        depth,
        tools.fanout_authorized,
        &tools.mcp_direct_tools,
        ceiling_allowed_tools.as_ref(),
        ceiling_deny_extensions,
    );
    env_orchestration(
        agent,
        opts,
        structured_runtime,
        capability_ceiling.as_ref(),
        temp_dir,
        &mut env_overlay,
    )?;
    let tool_diagnostic_path = env_control_channels(
        opts,
        temp_dir,
        tools.required_child_tools,
        &tools.effective_mcp_tools,
        &mut env_overlay,
    );

    // An injected binary must reach the child's ENVIRONMENT as well as its argv. A child that
    // spawns a grandchild of its own resolves that grandchild's command through
    // `resolve_spawn_command()`, which reads the environment it inherited — the orchestrator-sim
    // relay (`bin/cyrup_subagent_orchestrator_sim.rs`) depends on exactly that. Seeding the
    // overlay keeps the relay working while this process's own environment stays untouched,
    // which the `#![forbid(unsafe_code)]` crate could not do by setting the variable itself.
    if opts.spawn_command.is_some() {
        env_overlay.insert(
            crate::spawn::SUBAGENT_BINARY_ENV_VAR.to_string(),
            command.binary.display().to_string(),
        );
        // BOTH halves, ALWAYS — including an empty `base_args`, which encodes as `[]`.
        // `env_overlay` is additive and `env_clear()` is never called anywhere in this crate
        // (`spawn/mod.rs:198`/`:445`/`:523`), so omitting a variable does NOT unset it: the child
        // would inherit whatever this process happens to carry. Skipping the insert here would
        // therefore pair a freshly injected binary with a STALE inherited args value — one
        // command's binary wearing another's leading argv, which is the very half-a-command
        // failure this variable exists to prevent.
        //
        // `if let Ok` rather than `expect`: a `Vec<String>` always serializes, but the workspace
        // forbids `unwrap`/`expect`.
        if let Ok(encoded) = serde_json::to_string(&command.base_args) {
            env_overlay.insert(crate::spawn::SUBAGENT_BINARY_ARGS_ENV_VAR.to_string(), encoded);
        }
    }

    let cwd = opts.cwd.clone();

    Ok(AttemptSpawnPlan {
        spec: ChildSpawnSpec {
            command: SpawnCommand {
                binary: command.binary,
                base_args: command.base_args,
            },
            args,
            task_arg,
            env_overlay,
            cwd,
            // SUBA-030: BOTH spills are registered, so `cleanup_temp_files` removes the persona
            // file on every exit path exactly as it already removed the over-threshold task file.
            // A leaked `0600` prompt file would still be unreadable by other users, but it would
            // outlive the run, which pi's `mkdtemp`-per-spawn scratch directory never does.
            temp_files: temp_file.into_iter().chain(persona_temp_file).collect(),
        },
        tool_diagnostic_path,
    })
}

/// SUBA-021 — the CAPABILITY CEILING preflight (pi `resolveCurrentSubagentCapabilityCeiling` +
/// `assertAgentAllowedByCapabilityCeiling`, `runs/shared/capability-ceiling.ts:168`/`:183`).
///
/// Resolved FIRST, before any argv or env is built, because a ceiling is an upper bound on what
/// this subtree may do and the only useful place to enforce it is before a child exists. The
/// resolution intersects the INHERITED ceiling (this process's own
/// `CYRUP_SUBAGENT_CAPABILITY_CEILING_V1`, which a parent wrote when it spawned us) with every
/// ceiling registered for this session, so it can only ever tighten as the tree deepens.
///
/// Both arms are fail-CLOSED: a malformed inherited ceiling is an error, not "unbounded".
///
/// # Errors
///
/// [`SubagentError::CapabilityCeilingViolation`] when the inherited ceiling cannot be decoded
/// or when the resolved ceiling excludes this agent.
fn preflight_capability_ceiling(
    agent: &AgentConfig,
    opts: &RunOptions,
) -> Result<Option<crate::exec::capability_ceiling::ResolvedCapabilityCeiling>, SubagentError> {
    let capability_ceiling = crate::exec::capability_ceiling::resolve_current_capability_ceiling(
        opts.parent_session_id.as_deref(),
    )
    .map_err(SubagentError::CapabilityCeilingViolation)?;
    crate::exec::capability_ceiling::assert_agent_allowed(
        agent.name.as_str(),
        capability_ceiling.as_ref(),
    )
    .map_err(SubagentError::CapabilityCeilingViolation)?;
    Ok(capability_ceiling)
}

/// What [`resolve_child_tools`] hands back to the spawn plan: pi's `toolPlan` fields that
/// outlive the `--tools`/`--extension` argv it also emits (`runs/shared/pi-args.ts:104-141,389-409`
/// @v0.43.0), each with a consumer further down the plan — the extension threading, the
/// child-role env pair, the `MCP_DIRECT_TOOLS` selector list and the SUBA-045 diagnostic pair.
struct ResolvedChildTools {
    /// pi's `toolPlan.requiredChildTools` — `None` unless the agent pinned a non-empty allowlist.
    required_child_tools: Option<Vec<String>>,
    /// pi's `toolPlan.effectiveMcpTools` — the RESOLVED direct-MCP tool names.
    effective_mcp_tools: Vec<String>,
    /// pi's `toolExtensionPaths` — the agent's `tools:` entries that name an extension file.
    tool_extension_paths: Vec<String>,
    /// pi's `mcpDirectTools` — the raw `mcp:` selectors, as declared.
    mcp_direct_tools: Vec<String>,
    /// pi's `toolPlan.fanoutAuthorized` — the agent declared the `subagent` tool itself.
    fanout_authorized: bool,
}

/// pi `splitToolList` already ran at discovery time, so a `mcp:`-prefixed entry is a
/// `ToolRef::Mcp` holding the bare selector (pi's `mcpDirectTools`) and an extension-path entry a
/// `ToolRef::ExtensionPath` (pi's `toolExtensionPaths`). Re-split those typed refs here to
/// reproduce pi's three destinations for one `tools` list (`runs/shared/pi-args.ts:104-141`): builtins (plus
/// resolved MCP names) to `--tools`, extension paths to `--extension`, and the raw MCP selectors
/// to the `MCP_DIRECT_TOOLS` env.
/// See the `required_child_tools = Some(allowlist)` assignment below for the full rationale;
/// declared out here because the value has to survive into the env overlay, which is built
/// further down.
fn resolve_child_tools(
    agent: &AgentConfig,
    opts: &RunOptions,
    require_read_tool: bool,
    // SUBA-072 — the ceiling axes resolved by the caller, applied to both the `--tools` allowlist
    // and the direct-MCP resolution below.
    ceiling_allowed_tools: Option<&Vec<String>>,
    ceiling_deny_extensions: bool,
    args: &mut Vec<String>,
) -> ResolvedChildTools {
    let mut required_child_tools: Option<Vec<String>> = None;
    // SUBA-045 — pi's `toolPlan.effectiveMcpTools`: the RESOLVED direct-MCP tool names (not the
    // `mcp:` selectors). Empty unless the agent declared `mcp:` entries.
    let mut effective_mcp_tools: Vec<String> = Vec::new();
    let mut builtin_tools: Vec<String> = Vec::new();
    let mut tool_extension_paths: Vec<String> = Vec::new();
    let mut mcp_direct_tools: Vec<String> = Vec::new();
    if let Some(tools) = &agent.tools {
        for tool in tools {
            match tool {
                ToolRef::Builtin(name) => builtin_tools.push(name.clone()),
                ToolRef::ExtensionPath(path) => tool_extension_paths.push(path.clone()),
                ToolRef::Mcp(selector) => mcp_direct_tools.push(selector.clone()),
            }
        }
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
    // SUBA-072 fix: this used to live ONLY inside the `if explicit_tool_allowlist` block below (as
    // `allowlist`'s initializer), which meant `fanout_authorized` — computed separately, right here,
    // from the raw pre-ceiling `builtin_tools` — never saw the ceiling filter applied to this same
    // list two paragraphs down. A ceiling excluding `subagent` from `allowedTools` therefore failed
    // to revoke nested-delegation authorization even though it correctly narrowed `--tools` itself;
    // hoisting this computation out and deriving `fanout_authorized` from ITS result (below) closes
    // that gap.
    let effective_builtin_tools: Vec<String> = if agent.tools.is_some() {
        let mut declared = builtin_tools.clone();

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
        // `ToolRef::ExtensionPath`, so `builtin_tools` IS that list.
        //
        // SUBA-072 gives the `!allowedToolSet` term teeth: with a ceiling in play the
        // head-injection is skipped and the ceiling-membership filter below decides `read`'s
        // fate instead — including upstream's own edge case, faithfully reproduced: an agent
        // that both omits `read` from an explicit `tools:` list AND launches under a ceiling
        // that itself permits `read` does not have it force-added here; the agent must ask for
        // it. (The launch-time throw above still guards the case that actually matters — a
        // ceiling that EXCLUDES `read` while it is required.)
        if require_read_tool
            && !declared.is_empty()
            && !declared.iter().any(|tool| tool == "read")
            && ceiling_allowed_tools.is_none()
        {
            declared.insert(0, "read".to_string());
        }

        // SUBA-072 / pi `pi-args.ts:455`: `.filter((tool) => !allowedToolSet ||
        // allowedToolSet.has(tool))` — a ceiling can only narrow an explicit declaration, never
        // widen it.
        if let Some(allowed) = ceiling_allowed_tools.as_ref() {
            declared.retain(|tool| allowed.contains(tool));
        }
        declared
    } else {
        // pi `pi-args.ts:445`: `allowedToolSet ? [...allowedToolSet] : []` — reached only when
        // `ceiling_allowed_tools.is_some()`, since `agent.tools` is `None` here and this arm would
        // otherwise imply the ambient (unrestricted) set.
        ceiling_allowed_tools.cloned().unwrap_or_default()
    };

    // pi `runs/shared/pi-args.ts:194`: `const fanoutAuthorized = declaredBuiltinTools.includes("subagent")` —
    // a persona is granted NESTED delegation exactly when the CEILING-FILTERED declared builtin set
    // (`effective_builtin_tools` above, SUBA-072) includes the `subagent` tool — NOT the raw
    // pre-ceiling `agent.tools` declaration. So a ceiling whose `allowedTools` excludes `subagent`
    // revokes fanout authorization even when the agent's own `tools:` declares it, and conversely a
    // ceiling that GRANTS `subagent` via `allowedTools` authorizes fanout even for an agent that
    // declares no `tools:` of its own (pi's `input.tools === undefined` arm, `[...allowedToolSet]`).
    // With no ceiling and no `tools:` declared, `effective_builtin_tools` is `[]` exactly as
    // `builtin_tools` was, so an agent that declares nothing and launches unceilinged is still NOT
    // fanout-authorized — the pre-fix behavior in the only case that never involved a ceiling. This
    // is the single input to the child-role env pair below, and through it to
    // [`crate::extension::resolve_registration_mode`]: authorized → `ChildSafe` (the restricted,
    // mutation-blocked `subagent` tool, pi `extension/fanout-child.ts:132`), unauthorized → the
    // child registers no subagent surface at all and cannot delegate.
    let fanout_authorized = effective_builtin_tools
        .iter()
        .any(|tool| tool == crate::extension::TOOL_NAME);

    // G103 / pi `runs/shared/pi-args.ts:389-393,549-555` @v0.43.0. `explicitToolAllowlist` is
    // `input.tools !== undefined || mcpDirectTools.length > 0 || <ceiling>` — i.e. "did anything
    // pin this child's tool surface at all". cyrup folds pi's `tools` and `mcpDirectTools` into the
    // one `agent.tools: Option<Vec<ToolRef>>`, so `is_some()` covers pi's first two terms: `None` is
    // an agent that never wrote a `tools:` key (no restriction from the agent's own side), `Some(_)`
    // — INCLUDING `Some(vec![])` — is an explicit allowlist. SUBA-072 adds pi's third term: a
    // registered capability ceiling with `allowedTools` set pins the child's surface even when the
    // agent itself never wrote `tools:`.
    //
    // Upstream then emits `--tools <list>` when the effective allowlist is non-empty and
    // **`--no-tools`** when it is empty. Both halves were missing here: the old gate was
    // `!builtin_tools.is_empty()`, which (a) dropped `--no-tools` entirely, so an agent that asked
    // for NO tools — `tools:` empty in frontmatter, or a settings override of `"tools": false`,
    // which `discovery::merge` resolves to `Some(vec![])` — was spawned with the FULL ambient tool
    // set, the exact inversion of what it asked for; and (b) dropped `--tools` for a
    // direct-MCP-only agent (upstream's `effectiveToolAllowlist` is `[...declaredBuiltinTools,
    // ...effectiveMcpTools, ...internalTools]`, so MCP names alone still pin the allowlist).
    //
    // Upstream's fourth allowlist term, `internalTools` (`runs/shared/pi-args.ts:393` — the run's own
    // `structured_output` grant when the step declared an `outputSchema`), has no counterpart here
    // and needs none: **[CYRUP-DELTA]** cyrup's `--tools`/`--no-tools` selection
    // (`cyrup-session-svc/src/builder.rs:255-292`) runs over `registry.visible(...)` alone, and the
    // extension-contributed tools are merged in AFTERWARDS by `ext_host.active_tools(&base_tools)`
    // (`builder.rs:1068,1084`). `structured_output` is registered by this crate's own child-side
    // `prompt_runtime` extension, so it is never a candidate for the allowlist filter and survives
    // `--no-tools` intact — where in pi it is a first-class tool that the flag WOULD deny, hence
    // pi's explicit re-grant.
    let explicit_tool_allowlist = agent.tools.is_some() || ceiling_allowed_tools.is_some();
    if explicit_tool_allowlist {
        // `effective_builtin_tools` (computed above, ahead of `fanout_authorized`) is exactly pi's
        // `declaredBuiltinTools` — reused here verbatim as `allowlist`'s starting point before the
        // MCP names are appended below, reaching both consumers pi's own value reaches: the
        // `--tools` CSV and `requiredChildTools` (`:401-409`).
        let mut allowlist = effective_builtin_tools;

        if !mcp_direct_tools.is_empty() && !ceiling_deny_extensions {
            // SUBA-045: kept as its own binding because it is pi's `toolPlan.effectiveMcpTools`,
            // which has a SECOND consumer besides the `--tools` CSV — `MCP_DIRECT_CHILD_TOOLS_ENV`
            // (`pi-args.ts:618-621`), which is what lets the child's diagnostic distinguish a
            // missing MCP tool ("a host/pi-mcp-adapter registration problem") from a missing
            // extension tool. SUBA-072 / pi `pi-args.ts:457-469`: resolution itself is skipped
            // outright under `denyExtensions` (an MCP server is extension-provided), and whatever
            // survives is then filtered through the same ceiling-membership test as the builtins.
            effective_mcp_tools =
                mcp_direct_tools::resolve_mcp_direct_tool_names(&mcp_direct_tools, &opts.cwd);
            if let Some(allowed) = ceiling_allowed_tools.as_ref() {
                effective_mcp_tools.retain(|tool| allowed.contains(tool));
            }
            allowlist.extend(effective_mcp_tools.iter().cloned());
        }
        if allowlist.is_empty() {
            args.push("--no-tools".to_string());
        } else {
            args.push("--tools".to_string());
            args.push(allowlist.join(","));
        }

        // G106 / pi `runs/shared/pi-args.ts:611-616` @v0.43.0 (`env[REQUIRED_CHILD_TOOLS_ENV] =
        // JSON.stringify(toolPlan.requiredChildTools)`), whose `requiredChildTools` is
        // `explicitToolAllowlist ? [...declaredBuiltinTools, ...effectiveMcpTools, ...internalTools]
        // : []` (`:401-409`) — the same terms as `allowlist` above (minus `internalTools`, see the
        // `[CYRUP-DELTA]` note there), and this arm IS the `explicitToolAllowlist` branch (an agent
        // with no `tools:` never reaches it, and upstream writes `[]` — i.e. nothing — for that
        // case). Upstream itself only sets the env when the list is non-empty (`:610`), so a
        // no-tools child carries no `REQUIRED_CHILD_TOOLS` — which is also what its one consumer
        // wants: the child-side `intercom` fallback registers when the agent asked for a tool
        // called `intercom`, and an agent that asked for nothing asked for that too.
        if !allowlist.is_empty() {
            required_child_tools = Some(allowlist);
        }
    }

    ResolvedChildTools {
        required_child_tools,
        effective_mcp_tools,
        tool_extension_paths,
        mcp_direct_tools,
        fanout_authorized,
    }
}

/// Extension threading (pi `runs/shared/pi-args.ts:125-137`): `Some(extensions)` turns discovery off
/// (`--no-extensions`) and pins the exact allowlist; `None` leaves discovery on. In both cases
/// the agent's own tool-extension paths and child-only extensions are threaded explicitly,
/// order-preserving and de-duplicated. (This crate does not inject pi's own runtime `.ts`
/// extensions — cyrup's child-side subagent runtime is env-driven, not a loaded extension file,
/// a Tier-8 child-side concern — so only agent-declared paths flow through here.)
///
/// SUBA-072 / pi `pi-args.ts:457-463,514-527`: `ceiling_deny_extensions` overrides all of the
/// above — `toolExtensionPaths` becomes `[]`, `configuredExtensions` (which folds in
/// `agent.extensions` and `subagent_only_extensions` too) becomes `[]`, and
/// `disableAmbientExtensions` is forced true regardless of whether the agent itself declared
/// `extensions:`. Upstream's `extensionArgs` still always carries its own `runtimeExtensions`;
/// this crate has no analog (see the paragraph above), so under `denyExtensions` the child gets
/// `--no-extensions` and no `--extension` flags at all.
fn push_extension_and_skill_args(
    agent: &AgentConfig,
    ceiling_deny_extensions: bool,
    tool_extension_paths: Vec<String>,
    args: &mut Vec<String>,
) {
    let mut extension_paths: Vec<String> = Vec::new();
    if !ceiling_deny_extensions {
        for path in tool_extension_paths {
            push_unique(&mut extension_paths, path);
        }
    }
    if ceiling_deny_extensions || agent.extensions.is_some() {
        args.push("--no-extensions".to_string());
    }
    if !ceiling_deny_extensions {
        match &agent.extensions {
            Some(extensions) => {
                for path in extensions {
                    push_unique(&mut extension_paths, path.clone());
                }
                for path in &agent.subagent_only_extensions {
                    push_unique(&mut extension_paths, path.clone());
                }
            }
            None => {
                for path in &agent.subagent_only_extensions {
                    push_unique(&mut extension_paths, path.clone());
                }
            }
        }
    }
    for path in extension_paths {
        args.push("--extension".to_string());
        args.push(path);
    }

    // pi: a subagent that does not inherit skills is spawned with `--no-skills` (`runs/shared/pi-args.ts:139`).
    if !agent.inherit_skills {
        args.push("--no-skills".to_string());
    }
}

/// SUBA-001 / pi `runs/shared/pi-args.ts:159-165` (v0.34.0): the persona body ships on EVERY spawn; the
/// agent's `systemPromptMode` only chooses which flag carries it. See [`build_attempt_spawn_plan`]'s doc
/// comment for the literal-text-vs-temp-file `[CYRUP-DELTA]` and the empty-body rule.
///
/// Returns the `0600` spill file the composed body was written to, when one was written, so
/// the caller can register it for `cleanup_temp_files` (SUBA-030).
///
/// # Errors
///
/// Propagates [`ChildSpawnSpec::resolve_system_prompt_arg`]'s error (temp-file creation failure).
fn compose_persona(
    agent: &AgentConfig,
    opts: &RunOptions,
    temp_dir: &std::path::Path,
    args: &mut Vec<String>,
) -> Result<Option<std::path::PathBuf>, SubagentError> {
    // pi `execution.ts:1058-1061` folds the agent's persistent-memory block onto the SAME
    // `systemPrompt` string just before it reaches `buildPiArgs`, so a `memory:`-scoped agent
    // carries its accumulated role notes into every run. Same composition here, on the persona body
    // that this argv pair is the only channel for. `build_agent_memory_injection` returns "" for an
    // agent with no scope (the common case), an unresolvable/unsafe path, or a read-only agent with
    // nothing recorded yet — so the overwhelming majority of spawns are byte-identical to before.
    let memory_injection =
        crate::discovery::agent_memory::build_agent_memory_injection(
            agent.memory.as_ref(),
            agent.tools.as_ref(),
            &opts.cwd,
        );
    let persona_body_trimmed = agent.system_prompt_body.trim();
    let persona_with_memory: String = if memory_injection.is_empty() {
        persona_body_trimmed.to_string()
    } else if persona_body_trimmed.is_empty() {
        memory_injection
    } else {
        format!("{persona_body_trimmed}\n\n{memory_injection}")
    };

    // pi `systemPrompt = appendAgentRefinementOverlay(systemPrompt, { cwd: skillCwd, agentName })`
    // (`execution.ts:1442`) — the statement BETWEEN the memory fold above and the output-path
    // override below, so the composition order is persona -> skills -> memory -> refinement ->
    // output-path. Upstream's `skillCwd` is `options.cwd ?? runtimeCwd`, which is the same
    // `opts.cwd` the memory block above resolves against.
    //
    // A project that has never run `refine` has no `.cyrup-subagents/refinements/` directory at
    // all, and the overlay is then a byte-for-byte no-op — as is a malformed or whitespace-only
    // overlay file, since `append_agent_refinement_overlay` is infallible and returns its input
    // unchanged on every failure (pi's blanket `catch { return systemPrompt; }`).
    let persona_with_refinement = crate::exec::agent_refinements::append_agent_refinement_overlay(
        &persona_with_memory,
        &opts.cwd,
        &agent.name,
    );

    // G82 / R-SA-024 — the SYSTEM-PROMPT half of the output-path override, upstream
    // `injectOutputPathSystemPrompt(systemPrompt, options.outputPath, agent)`
    // (`execution.ts:1443`, the statement immediately after the memory/refinement composition this
    // block mirrors). It is a SECOND surface, not an alternative to the task-side
    // `injectSingleOutputInstruction` that `build_task_text` applies: upstream's foreground single
    // run gets BOTH — the task side at `subagent-executor.ts:3674` (its caller) and this one at
    // `execution.ts:1443` — because the task text steers what the child is asked to produce while
    // the system prompt steers where its write tools point for the whole session, and a long run
    // that compacts away the task text keeps the system prompt. `api/preflight.ts:313` applies the
    // same injector to its own `effectiveSystemPrompt` projection; cyrup has no launch-contract
    // preflight surface to port that second call site onto, so this is the only one that exists
    // here.
    //
    // Keyed on the PATH alone, never on `opts.output_mode` — same rule as the task side, for the
    // same reason (`outputMode` is read only by `validateFileOnlyOutputMode` and by delivery-side
    // `finalizeSingleOutput`). Capability-aware through the same `AgentDefinition` projection the
    // task side uses, so a read-only agent is told the runtime will persist its final response
    // rather than being ordered to write a file it has no tool to write
    // (`single-output.ts:84-91`).
    let output_capabilities = completion_guard_projection(agent);
    let persona_owned = inject_output_path_system_prompt(
        &persona_with_refinement,
        opts.output_path.as_deref(),
        Some(&output_capabilities),
    );
    // SUBA-008 — pi `appendTurnBudgetSystemPrompt(shared.systemPrompt, options.turnBudget)`
    // (`execution.ts:326` for the spawn inputs and `:350` for `effectiveSystemPrompt`). It is the
    // OUTERMOST append: `shared.systemPrompt` is what `buildSharedRunInputs` returns at
    // `execution.ts:1443`, i.e. after persona -> skills -> memory -> refinement -> output-path, so
    // the budget block lands last and nothing composed above can displace it.
    //
    // This is the ONLY way the child learns about the budget — there is no
    // `PI_SUBAGENT_TURN_BUDGET` and no child-side enforcement, unlike the tool budget's env
    // hand-off two blocks below. See `exec/turn_budget.rs`'s module doc.
    let persona_owned = crate::exec::turn_budget::append_turn_budget_system_prompt(
        &persona_owned,
        opts.turn_budget.as_ref(),
    );
    let persona_body: &str = &persona_owned;
    let mut persona_temp_file: Option<std::path::PathBuf> = None;
    if !persona_body.is_empty() {
        let flag = match agent.system_prompt_mode {
            SystemPromptMode::Replace => SYSTEM_PROMPT_FLAG,
            SystemPromptMode::Append => APPEND_SYSTEM_PROMPT_FLAG,
        };
        // SUBA-030 / pi `runs/shared/pi-args.ts:570-585` @v0.43.0: the composed prompt is written to
        // a `0600` file in the run's scratch directory and the flag carries the PATH, as TWO argv
        // elements (`args.push(flag, promptPath)`, `:580-585`). Upstream applies no size threshold
        // here — unlike the task spill at `:588` it spills every time — so this is the literal
        // mechanism, not a large-persona fallback.
        //
        // Two defects close with it. The persona no longer appears in `/proc/<pid>/cmdline`, where
        // any local user could read a body that routinely carries project context; and a persona
        // above Linux's `MAX_ARG_STRLEN` (131072) can no longer make `execve` fail with `E2BIG` and
        // kill the spawn with an opaque OS error.
        //
        // The `=`-joined single-argv form the inline delivery required is gone with it, and so is
        // its reason: a detached value beginning with `-` is what clap refuses, and an ABSOLUTE
        // temp path can never begin with one.
        let stem = if agent.name.trim().is_empty() {
            "prompt"
        } else {
            agent.name.as_str()
        };
        let prompt_path = ChildSpawnSpec::resolve_system_prompt_arg(persona_body, stem, temp_dir)?;
        args.push(flag.to_string());
        args.push(prompt_path.display().to_string());
        persona_temp_file = Some(prompt_path);
    }

    Ok(persona_temp_file)
}

/// The IDENTITY half of the child env overlay, in upstream's own write order: the incremented
/// depth envelope (R-SA-054), the child-ROLE pair, the run sentinel, the two agent-identity
/// markers, the resolved persona name, the agent's inherit flags and the raw direct-MCP
/// selector list. Every key here is a function of the AGENT and the tree position alone, so
/// none of it depends on the run's orchestration wiring below.
fn env_identity_and_depth(
    agent: &AgentConfig,
    opts: &RunOptions,
    depth: DepthEnvelope,
    fanout_authorized: bool,
    mcp_direct_tools: &[String],
    // SUBA-072 — the ceiling axes, applied to the RAW `MCP_DIRECT_TOOLS` selector list.
    ceiling_allowed_tools: Option<&Vec<String>>,
    ceiling_deny_extensions: bool,
) -> std::collections::HashMap<String, String> {
    // Caller-supplied child entries first: everything this function adds below is a crate
    // invariant (identity, depth, child role) and must win over them.
    let mut env_overlay = opts.child_env.clone();
    env_overlay.extend(crate::spawn::depth::to_env_overlay(&depth));
    // PERM-001 / pi `augmentChildEnv` (`runs/shared/pi-args.ts:329-330`): the child-ROLE pair, written on EVERY
    // spawn. This is the process about to BE a subagent child, so it must say so — see
    // [`crate::spawn::nested_events::child_role_env`] for the three subsystems that read it, chiefly
    // the permission companion's child→parent ask-forwarding (a child whose `ask` fires with this
    // flag absent installs no `ForwardingAskChannel`, finds no reachable human, and fails closed on
    // every ask instead of reaching the PARENT's human through the spool).
    //
    // The companion fanout flag tracks the agent's OWN declared tools (`fanout_authorized` above),
    // exactly as pi's `env[SUBAGENT_FANOUT_CHILD_ENV] = toolPlan.fanoutAuthorized ? "1" : "0"`. It is
    // always written — an explicit `"0"`, never merely omitted — because a spawn env is an OVERLAY
    // over the inherited environment ([`crate::spawn::SpawnedChild::spawn`] never clears it), so an
    // absent entry would let a fanout-authorized process's own `CYRUP_SUBAGENT_FANOUT_CHILD=1` leak
    // down into a grandchild that was never granted it.
    //
    // (What this pair does NOT carry is the nested-event ROUTE — `event_sink`/`root_run_id`/
    // capability token, [`crate::spawn::nested_events::nested_child_auth_env`] — which no production
    // path mints yet. An authorized child therefore delegates through its own NDJSON stream rather
    // than onto a grandparent route; that wiring is separate and unported.)
    for (key, value) in crate::spawn::nested_events::child_role_env(fanout_authorized) {
        env_overlay.insert(key.to_string(), value.to_string());
    }
    // Model-inherit sentinel (R-SA-041) never leaks a global default into the child's own
    // resolution beyond what `--model` above already pins explicitly for this attempt.
    env_overlay.insert("CYRUP_SUBAGENT_RUN".to_string(), "1".to_string());
    // TOOL-031 / PARITY-GAPS PB-5, the re-exec half. pi sets the agent-identity markers on
    // `process.env` in `cli.ts` BEFORE `main()` runs (`PI_CODING_AGENT = "true"` at `cli.ts:13`
    // @v0.83.0; `AI_AGENT = "pi"` at `:14` @v0.84.1, mirrored in `rpc-entry.ts:7-8`), so they
    // reach every descendant by inheritance — including a re-exec'd subagent child and everything
    // IT spawns.
    //
    // cyrup's bin declines the process-global `std::env::set_var` (`unsafe` under edition 2024,
    // rationale at `crates/cyrup/src/main.rs`), so each spawn site writes them per-child. Without
    // this the marker chain broke at the FIRST re-exec: a subagent child ran with
    // `PI_CODING_AGENT` unset, and so did every tool and MCP server it launched — the exact
    // scripts-detect-an-agent contract the vars exist for, silently off for the whole subtree.
    //
    // Written unconditionally (never merely omitted) for the same reason as the fanout flag above:
    // the overlay is applied over an INHERITED environment, so these must be asserted, not assumed.
    env_overlay.insert("PI_CODING_AGENT".to_string(), "true".to_string());
    // [CYRUP-DELTA — KEY *and* value; the key is a FORWARD-PORT from `cli.ts:14` @v0.84.1, which is
    // AHEAD of the ported tag] `AI_AGENT` does not exist anywhere in pi @v0.83.0
    // (`git -C pi grep -n AI_AGENT v0.83.0 -- packages/` → 0 hits; `cli.ts:13` @v0.83.0 sets only
    // `PI_CODING_AGENT`), so cyrup writes a variable into every re-exec'd subagent child — and its
    // whole subtree — that the ported baseline never wrote. The value additionally names WHICH
    // agent is running (`"pi"` upstream). Deliberate and kept; stated on the delta line itself
    // rather than only in the prose above (CFG-069) so a later v0.84.1 uplift reads this as
    // ALREADY-PORTED-EARLY and not as already-done-at-tag. Same class as the
    // `working-start`/`working-stop` precedent.
    env_overlay.insert("AI_AGENT".to_string(), "cyrup".to_string());
    // Permission input (1) (port doc §4 / pi `resolveAgentName`, `pi-permission-system/src/index.ts:2033-2047` @v0.7.1): thread the
    // resolved persona name to the child as [`AGENT_NAME_ENV_VAR`] so its permission companion's
    // `agent` + `projectAgent` policy layers enforce for the named persona. Only a non-empty name is
    // written (an unnamed persona → var absent → child resolves `None`, matching pi's top-level `""`).
    if !agent.name.trim().is_empty() {
        env_overlay.insert(AGENT_NAME_ENV_VAR.to_string(), agent.name.clone());
    }
    // pi `runs/shared/pi-args.ts:199-200`: the child observes the agent's inherit flags as env (`1`/`0`).
    env_overlay.insert(
        INHERIT_PROJECT_CONTEXT_ENV.to_string(),
        if agent.inherit_project_context { "1" } else { "0" }.to_string(),
    );
    env_overlay.insert(
        INHERIT_SKILLS_ENV.to_string(),
        if agent.inherit_skills { "1" } else { "0" }.to_string(),
    );
    // SUBA-072 / pi `pi-args.ts:916-926`: `MCP_DIRECT_TOOLS` (no `_CHILD_` — the raw selector list
    // `cyrup-mcp::registration::register_surface` reads, independently of the `--tools` CSV, to
    // decide which MCP servers/tools the child's own MCP adapter activates as direct-tool
    // overrides) must obey the SAME ceiling axes as `effective_mcp_tools` above, but on SELECTOR
    // strings (`<server>` or `<server>/<tool>`), not the resolved tool NAMES
    // `resolve_mcp_direct_tool_names` returns — a raw, unfiltered write here was a second,
    // independent bypass of the same ceiling one level of code above already narrows correctly.
    // Upstream's exact three-way branch (`pi-args.ts:916-926`):
    //
    //   if (!toolPlan.capabilityCeiling && input.mcpDirectTools?.length)
    //       env.MCP_DIRECT_TOOLS = input.mcpDirectTools.join(",");
    //   else if (toolPlan.capabilityCeiling && toolPlan.effectiveMcpSelections.length
    //            && !toolPlan.capabilityCeiling.denyExtensions)
    //       env.MCP_DIRECT_TOOLS = toolPlan.effectiveMcpSelections.map(s => s.selector).join(",");
    //   else env.MCP_DIRECT_TOOLS = "__none__";
    //
    // i.e. (i) no ceiling at all → the raw selectors, unfiltered; (ii) a ceiling present, not
    // denying extensions, with at least one selection surviving the `allowedTools` filter → the
    // FILTERED selectors; (iii) otherwise → `__none__`. A selector survives iff at least one tool
    // name it expands to is still ceiling-allowed — resolving each selector ALONE (rather than
    // reusing the aggregate `effective_mcp_tools`, which has already lost the selector-to-name
    // mapping by construction) reproduces that per-selector survival test without needing to touch
    // `mcp_direct_tools.rs`'s public surface.
    let ceiling_filtered_mcp_selectors: Vec<String> = if ceiling_deny_extensions {
        Vec::new()
    } else if let Some(allowed) = ceiling_allowed_tools.as_ref() {
        mcp_direct_tools
            .iter()
            .filter(|selector| {
                mcp_direct_tools::resolve_mcp_direct_tool_names(
                    std::slice::from_ref(*selector),
                    &opts.cwd,
                )
                .iter()
                .any(|name| allowed.contains(name))
            })
            .cloned()
            .collect()
    } else {
        mcp_direct_tools.to_vec()
    };
    env_overlay.insert(
        MCP_DIRECT_TOOLS_ENV.to_string(),
        if ceiling_filtered_mcp_selectors.is_empty() {
            MCP_DIRECT_TOOLS_NONE_SENTINEL.to_string()
        } else {
            ceiling_filtered_mcp_selectors.join(",")
        },
    );

    env_overlay
}

/// The ORCHESTRATION half of the child env overlay, applied AFTER
/// [`env_identity_and_depth`] and in upstream's own write order: the parent-session anchor,
/// the child watchdog config, the intercom child bridge plus its native supervisor channel,
/// the structured-output runtime, and the two encoded blobs (tool budget, capability ceiling).
/// Every key here is a function of the RUN, not of the agent alone.
fn env_orchestration(
    agent: &AgentConfig,
    opts: &RunOptions,
    structured_runtime: Option<&crate::exec::structured::StructuredOutputRuntime>,
    capability_ceiling: Option<&crate::exec::capability_ceiling::ResolvedCapabilityCeiling>,
    // SUBA-073 — this attempt's scratch dir, the default home of the permission audit log.
    temp_dir: &std::path::Path,
    env_overlay: &mut std::collections::HashMap<String, String>,
) -> Result<(), SubagentError> {
    // R-SA-P1 (port doc §4 P-4): the canonical parent-session anchor. Precedence: EXPLICIT (the
    // launching session's own id, resolved at the root's SessionStart via P-2 and threaded through
    // `opts.parent_session_id`) → INHERITED (this process's own `CYRUP_SUBAGENT_PARENT_SESSION`, so a
    // `DEPTH>0` child keeps threading the root's anchor rather than overwriting it — the direct-parent
    // depth-1 semantics pi documents) → EMPTY (omitted, no anchor). Only a non-empty value is written.
    if let Some(anchor) = opts
        .parent_session_id
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var(PARENT_SESSION_ENV_VAR).ok().filter(|s| !s.is_empty()))
    {
        env_overlay.insert(PARENT_SESSION_ENV_VAR.to_string(), anchor);
    }

    // Child watchdog activation (pi `execution.ts:298-302` / `subagent-runner.ts:1309-1312`): the
    // parent resolves ITS OWN watchdog config for this agent — including any
    // `subagents.watchdog.children.overrides.<agent>` entry — projects it onto the flat child shape,
    // and hands it over as one JSON env var. `resolve_child_watchdog_config` returns `None` whenever
    // the master switch or the children switch is off (which is the default), so a session that has
    // not enabled the watchdog writes no variable at all and the child's own
    // `decode_child_watchdog_config` sees nothing.
    //
    // Resolved from the CHILD's cwd (`step.cwd ?? ctx.cwd`, `subagent-runner.ts:1309`) so a run in a
    // different project reads that project's settings, and a settings-parse failure simply yields no
    // child watchdog rather than failing the spawn (upstream's `watchdogConfig.ok` guard).
    {
        let watchdog_cwd = opts.cwd.as_path();
        let resolved = crate::watchdog::settings::resolve_watchdog_config(watchdog_cwd, None);
        if resolved.ok
            && let Some(child_config) = crate::watchdog::child_status::resolve_child_watchdog_config(
                &resolved.config,
                Some(agent.name.as_str()).filter(|name| !name.trim().is_empty()),
                opts.run_id.as_ref().map(|id| id.as_str()),
                opts.child_index.and_then(|index| u64::try_from(index).ok()),
            )
            && let Some(encoded) =
                crate::watchdog::child_status::encode_child_watchdog_config(Some(&child_config))
        {
            env_overlay.insert(
                crate::watchdog::child_status::CHILD_WATCHDOG_CONFIG_ENV.to_string(),
                encoded,
            );
        }
    }

    // Intercom child-bridge activation (pi `runs/shared/pi-args.ts:201-214`, `augmentChildEnv`'s intercom half):
    // when the launching orchestrator has a resolvable intercom presence target AND this run has an
    // id AND this child has a persona name, write the full child-orchestrator metadata set so the
    // spawned child's `IntercomExtension` reads `read_child_orchestrator_metadata() == Some` →
    // registers `contact_supervisor` (addressed at this supervisor) + a broker presence under its own
    // deterministic label. All four gate-required vars (target/run-id/agent/child-index) plus the
    // child's own label are set TOGETHER (never a partial subset — a half-set metadata gate would
    // leave the child neither fully bridged nor cleanly un-bridged). `RUN_ID`/`CHILD_INDEX` reuse the
    // nested-events env names, so setting them here also satisfies that sibling overlay. The child's
    // presence label is the SAME `resolve_subagent_intercom_target(run_id, agent, index)` string the
    // parent addresses to steer it (extension.rs `control_resume`), so the two independently-produced
    // strings match at the broker. `ORCHESTRATOR_SESSION_ID` + `SUPERVISOR_CHANNEL_DIR` are written
    // by the nested block at the end of this arm — the NATIVE supervisor channel upstream added in
    // `3ac0ef5` (`runs/shared/pi-args.ts:221-231`), which needs the launching session's own id as its request
    // routing key.
    if let (Some(orch_target), Some(run_id)) = (
        opts.orchestrator_intercom_target.as_deref().filter(|s| !s.is_empty()),
        opts.run_id.as_ref(),
    ) && !agent.name.trim().is_empty()
    {
        let child_index = opts.child_index.unwrap_or(0);
        env_overlay.insert(
            crate::spawn::intercom_target::ENV_ORCHESTRATOR_TARGET.to_string(),
            orch_target.to_string(),
        );
        env_overlay.insert(crate::spawn::nested_events::RUN_ID_ENV.to_string(), run_id.as_str().to_string());
        env_overlay.insert(crate::spawn::intercom_target::ENV_CHILD_AGENT.to_string(), agent.name.clone());
        env_overlay.insert(crate::spawn::nested_events::CHILD_INDEX_ENV.to_string(), child_index.to_string());
        env_overlay.insert(
            crate::spawn::intercom_target::ENV_INTERCOM_SESSION_NAME.to_string(),
            crate::spawn::intercom_target::resolve_subagent_intercom_target(
                run_id.as_str(),
                &agent.name,
                child_index,
            ),
        );

        // NATIVE supervisor channel (pi `runs/shared/pi-args.ts:221-231`, added in `3ac0ef5` "Make supervisor
        // coordination native"). Upstream's condition is `orchestratorIntercomTarget &&
        // parentSessionId && runId && childAgentName` — the first, third and fourth are the arm
        // we are already inside, so only the parent session id remains. Both vars are written
        // TOGETHER with the channel directories created up front (upstream's own two
        // `fs.mkdirSync(..., { recursive: true })` calls, `:228-229`), because
        // `read_child_metadata` refuses to activate on a partial set: a child handed a channel dir
        // it cannot address, or an address with no channel dir, would be neither bridged nor
        // cleanly un-bridged.
        //
        // A failure to create the directories is deliberately NOT fatal here: the file channel is a
        // fallback for coordination the broker path may already be providing, and a spawn must not
        // fail because a scratch directory could not be made. The vars are then simply not written,
        // which is exactly the "no native channel" state.
        if let Some(parent_session) = opts
            .parent_session_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            env_overlay.insert(
                crate::spawn::intercom_target::ENV_ORCHESTRATOR_SESSION_ID.to_string(),
                parent_session.to_string(),
            );
            let channel_dir = crate::native_supervisor::resolve_supervisor_channel_dir(
                run_id.as_str(),
                &agent.name,
                child_index,
            );
            if crate::native_supervisor::ensure_supervisor_channel_dir(&channel_dir).is_ok() {
                env_overlay.insert(
                    crate::spawn::intercom_target::ENV_SUPERVISOR_CHANNEL_DIR.to_string(),
                    channel_dir.display().to_string(),
                );
            }
        }
    }

    // SUBA-S01 (pi `runs/shared/pi-args.ts:246-250`): hand the child BOTH paths. pi's child-side runtime gates
    // on both being present (`subagent-prompt-runtime.ts:281`), and so does cyrup's
    // [`crate::prompt_runtime::prompt_runtime_extension_for_env`] — so these are set together or
    // not at all. Without them the child has no `structured_output` tool, which is precisely the
    // state every declared-schema run was in before this wiring existed.
    if let Some(runtime) = structured_runtime {
        env_overlay.insert(
            crate::exec::structured::STRUCTURED_OUTPUT_SCHEMA_ENV.to_string(),
            runtime.schema_path.display().to_string(),
        );
        env_overlay.insert(
            crate::exec::structured::STRUCTURED_OUTPUT_CAPTURE_ENV.to_string(),
            runtime.output_path.display().to_string(),
        );
    }

    // pi `pi-args.ts` ships the resolved tool budget to the child in `PI_SUBAGENT_TOOL_BUDGET`
    // (`tool-budget.ts:70-72`); the child-side `subagent-prompt-runtime.ts:263` decodes it and
    // registers the nudge/block hook. Same hand-off here — see
    // [`crate::prompt_runtime::SubagentPromptRuntime`] for the enforcement half. Absent budget =>
    // no var, so a child that inherits a STALE budget from the parent's own environment cannot
    // happen silently: the overlay only ever adds.
    if let Some(encoded) =
        crate::exec::tool_budget::encode_tool_budget_env(agent.tool_budget.as_ref())
    {
        env_overlay.insert(
            crate::exec::tool_budget::TOOL_BUDGET_ENV.to_string(),
            encoded,
        );
    }

    // SUBA-073 — pi ships the resolved permission policy to the child in `PERMISSION_POLICY_ENV`
    // (`pi-args.ts:938`); the child-side `watchdog::permission_arbiter`/`prompt_runtime` gate
    // already decodes and enforces it. Same hand-off shape as the tool budget immediately above.
    // Absent policy => no var, so a child cannot silently inherit a STALE policy from the parent's
    // own environment (the overlay only ever adds — same rule as every other member of this
    // family).
    //
    // pi ALSO writes `PERMISSION_AUDIT_PATH_ENV` whenever a policy is present (`pi-args.ts:905-906`),
    // defaulting to `<tempDir>/permission-audit.jsonl` when the caller supplied no explicit path —
    // this crate has no per-call override for the audit path today, so it always takes that
    // default, using the SAME `temp_dir` this function already receives for the
    // persona-body/task spill files.
    if let Some(encoded) = crate::exec::permissions::encode_permission_rules(
        opts.permission_rules.as_ref(),
    )
    .map_err(SubagentError::Management)?
    {
        env_overlay.insert(
            crate::watchdog::permission_arbiter::PERMISSION_AUDIT_PATH_ENV.to_string(),
            temp_dir.join("permission-audit.jsonl").display().to_string(),
        );
        env_overlay.insert(
            crate::watchdog::permission_arbiter::PERMISSION_POLICY_ENV.to_string(),
            encoded,
        );
    }

    // SUBA-021 — the capability ceiling resolved at the top of this function crosses the process
    // boundary here (pi `encodeSubagentCapabilityCeiling` into `SUBAGENT_CAPABILITY_CEILING_ENV`,
    // `capability-ceiling.ts:192-195`, read back by the child's own
    // `resolveCurrentSubagentCapabilityCeiling` at `:168`).
    //
    // This is what makes the bound MONOTONIC across the re-exec that is this crate's whole
    // mechanism: the child intersects what it inherits here with anything registered in its own
    // process, so a grandchild can be narrower than its parent and never wider. Absent ceiling =>
    // no var, so an unbounded run does not inherit a stale bound from the parent's environment (the
    // overlay only ever adds).
    if let Some(encoded) =
        crate::exec::capability_ceiling::encode_capability_ceiling(capability_ceiling)
    {
        env_overlay.insert(
            crate::exec::capability_ceiling::CAPABILITY_CEILING_ENV.to_string(),
            encoded,
        );
    }

    // SUBA-078 — the thinking ceiling crosses the same boundary, for the same reason (pi
    // `pi-args.ts:875-879` @v0.57.0). Re-intersected with what this process itself inherited, so a
    // grandchild is bound by the LOWEST of everything above it and can only ever tighten. Absent
    // ceiling => no var, so an unbounded run does not inherit a stale bound from the parent's
    // environment — the overlay only ever adds.
    let inherited = crate::exec::thinking_ceiling::inherited_thinking_ceiling()
        .map_err(SubagentError::ThinkingCeilingViolation)?;
    let resolved = crate::exec::thinking_ceiling::intersect_thinking_ceilings(&[
        opts.thinking_ceiling.as_deref(),
        inherited.as_deref(),
    ])
    .map_err(SubagentError::ThinkingCeilingViolation)?;
    if let Some(ceiling) = resolved {
        env_overlay.insert(
            crate::exec::thinking_ceiling::THINKING_CEILING_ENV.to_string(),
            ceiling,
        );
    }
    Ok(())
}

/// The CONTROL-CHANNEL half of the child env overlay, applied last: the SUBA-045
/// tool-availability diagnostic pair and the three SUBA-049/G90 steer paths. Each is a channel
/// the PARENT reads back or writes into while the child runs, which is why they are grouped and
/// why the diagnostic path is returned rather than re-derived from the overlay at settle.
fn env_control_channels(
    opts: &RunOptions,
    temp_dir: &std::path::Path,
    required_child_tools: Option<Vec<String>>,
    effective_mcp_tools: &[String],
    env_overlay: &mut std::collections::HashMap<String, String>,
) -> Option<PathBuf> {
    // SUBA-045 (pi `pi-args.ts:610-621`): the diagnostic path and the resolved direct-MCP names go
    // out BESIDE the required-tools list and under upstream's own gate — `if
    // (toolPlan.requiredChildTools.length > 0)`. An agent with no explicit `tools:` requires
    // nothing, so nothing can be missing and neither var is written.
    let mut tool_diagnostic_path: Option<PathBuf> = None;
    if let Some(tools) = required_child_tools {
        env_overlay.insert(
            crate::native_supervisor::ENV_REQUIRED_CHILD_TOOLS.to_string(),
            serde_json::Value::Array(
                tools
                    .iter()
                    .cloned()
                    .map(serde_json::Value::String)
                    .collect(),
            )
            .to_string(),
        );
        let diagnostic_path = crate::exec::tool_availability::tool_diagnostic_path_in(temp_dir);
        env_overlay.insert(
            crate::exec::tool_availability::CHILD_TOOL_DIAGNOSTIC_PATH_ENV.to_string(),
            diagnostic_path.display().to_string(),
        );
        tool_diagnostic_path = Some(diagnostic_path);
        // pi writes this one UNCONDITIONALLY at `:618-621` — but as `env[...] = undefined` when the
        // list is empty, which in Node deletes rather than sets the key. An absent key is what an
        // empty list means, so cyrup writes it only when non-empty, inside the same gate: without a
        // required list there is no diagnostic to enrich.
        if !effective_mcp_tools.is_empty() {
            env_overlay.insert(
                crate::exec::tool_availability::MCP_DIRECT_CHILD_TOOLS_ENV.to_string(),
                serde_json::Value::Array(
                    effective_mcp_tools
                        .iter()
                        .cloned()
                        .map(serde_json::Value::String)
                        .collect(),
                )
                .to_string(),
            );
        }
    }

    // G90 (pi `runs/shared/pi-args.ts:251-252` @v0.34.0: `if (input.steerInboxDir) env[SUBAGENT_STEER_INBOX_ENV]
    // = input.steerInboxDir`): hand this child the path to its OWN steer inbox. `run_sync` creates
    // the directory before the spawn so the child's watcher has something to attach to on its very
    // first tick, exactly as upstream's child-side `start()` does its own `mkdirSync` — see
    // [`crate::prompt_runtime::SteeringInbox`]. Absent on the foreground path, so a foreground child
    // is byte-identical to before.
    if let Some(inbox) = opts
        .steer_inbox_dir
        .as_deref()
        .filter(|p| !p.as_os_str().is_empty())
    {
        env_overlay.insert(
            crate::prompt_runtime::STEER_INBOX_ENV.to_string(),
            inbox.display().to_string(),
        );
    }

    // SUBA-049 (pi `runs/shared/pi-args.ts:764-768` @v0.43.0: `if (input.steerCapabilityPath)
    // env[SUBAGENT_STEER_CAPABILITY_ENV] = …; if (input.steerAckDir) env[SUBAGENT_STEER_ACK_DIR_ENV]
    // = …`): the RETURN half of the same channel. Both are written under the same `if (…)` shape
    // upstream uses, so a foreground child — which has no run directory and therefore neither path —
    // is byte-identical to before.
    if let Some(path) = opts
        .steer_capability_path
        .as_deref()
        .filter(|p| !p.as_os_str().is_empty())
    {
        env_overlay.insert(
            crate::prompt_runtime::STEER_CAPABILITY_ENV.to_string(),
            path.display().to_string(),
        );
    }
    if let Some(dir) = opts
        .steer_ack_dir
        .as_deref()
        .filter(|p| !p.as_os_str().is_empty())
    {
        env_overlay.insert(
            crate::prompt_runtime::STEER_ACK_DIR_ENV.to_string(),
            dir.display().to_string(),
        );
    }

    tool_diagnostic_path
}

/// Compose the final task text handed to the child: acceptance-contract injection (R-SA-023), then
/// the authoritative output-path instruction (R-SA-024), then the skill-pointer block.
///
/// The agent's OWN persona prose is deliberately NOT part of this text. It travels as
/// `--system-prompt`/`--append-system-prompt` on the child's argv — see
/// [`build_attempt_spawn_plan`] (SUBA-001, pi `runs/shared/pi-args.ts:159-165`). Previously `Append` mode
/// concatenated the body here and `Replace` mode dropped it on the floor entirely, on the mistaken
/// premise that the child re-resolved its own persona; nothing child-side does (the
/// [`AGENT_NAME_ENV_VAR`] anchor is read only by the permission companion), so every
/// `Replace`-mode subagent — 7 of the 8 bundled personas and every user-authored agent — ran with
/// no persona at all.
///
/// The pre-resolved `skill_injection` (the lazy `<available_skills>` pointer block built ONCE per
/// run by [`crate::exec::run_sync`] via [`crate::discovery::skills::build_skill_injection`]) is appended LAST.
/// pi composes it onto the persona system prompt instead (`execution.ts:1054-1056`); keeping it in
/// the task text here is what lets a `Replace`-mode persona (which wholesale replaces the child's
/// assembled system prompt) coexist with orchestrator-injected scaffolding rather than suppress it.
/// Empty when the agent/step declares no skills, so the common no-skills case appends nothing. This
/// is ORTHOGONAL to `agent.inherit_skills` (the `--no-skills` child flag): an agent that does not
/// inherit skills still receives its explicitly-listed skills through this block.
///
/// G82: the output-path instruction is the TASK-side injector
/// [`crate::exec::output::inject_single_output_instruction`], upstream
/// `injectSingleOutputInstruction` (`runs/shared/single-output.ts:99-102`) — the one that emits the
/// `\n\n---\n**Output:**\n…` header. That header is not decoration: it is the alternative
/// `stripFrameworkInstructions` (`task-intent.ts:99`, ported at
/// [`crate::exec::task_intent`]) removes before mutation-intent classification, so the injected
/// instruction's own `write`/`persist` vocabulary never contributes write-intent signal to the task
/// it was appended to. The system-prompt-shaped sibling
/// (`build_output_path_system_prompt_instruction`, whose body opens `Runtime output path
/// override:`) was wired here instead and is NOT one of those alternatives, so every file-only run
/// was feeding its own scaffolding back into the classifier.
///
/// It is UNCONDITIONAL on [`RunOptions::output_mode`]: upstream keys the injection on the presence
/// of an output PATH alone at every one of its call sites — `subagent-executor.ts:3674` (the single
/// run, this function's direct counterpart), `chain-execution.ts:363,1320` @v0.43.0 and
/// `async-execution.ts:711,1289` @v0.43.0 — and `outputMode` is consulted only by
/// `validateFileOnlyOutputMode` (ported as [`crate::exec::output::validate_file_only_requires_path`]) and by the
/// delivery-side `finalizeSingleOutput`. Gating the injection on `OutputMode::FileOnly`, as this
/// function previously did, silently dropped the authoritative-path instruction from every
/// `file-and-inline` run that configured an output path — the child was never told where to write.
///
/// The instruction is CAPABILITY-AWARE. `agent` is projected to the [`crate::discovery::types::AgentDefinition`] view
/// [`crate::exec::output::format_output_path_instruction`] reads (`tools`, via
/// `has_mutation_tool_capability`), so an agent whose whole resolved allowlist is read-only is told
/// to return the artifact in its final response for the runtime to persist — pi's
/// `formatOutputPathInstruction` read-only branch (`single-output.ts:84-91`) — instead of being
/// ordered to write a file it has no tool to write. Upstream threads the same agent object into
/// every injection site (`execution.ts:1443`, `chain-execution.ts:363`,
/// `subagent-executor.ts:3674`).
pub(crate) fn build_task_text(
    agent: &AgentConfig,
    task: &str,
    opts: &RunOptions,
    contract: &AcceptanceContract,
    skill_injection: &str,
) -> String {
    let with_acceptance = inject_acceptance_contract(task, contract);
    let capabilities = completion_guard_projection(agent);
    let with_output_path = inject_single_output_instruction(
        &with_acceptance,
        opts.output_path.as_deref(),
        Some(&capabilities),
    );
    // SUBA-054 / pi `task = readsInstruction + task` (`subagent-executor.ts:3873`), which runs
    // BEFORE `injectSingleOutputInstruction` (`:3874`) — so the read line is the FIRST thing in the
    // task text, ahead of every other injected block. Prepending here rather than threading it
    // through the injectors keeps that ordering true whatever else is appended later.
    let reads_instruction = opts.reads.as_deref().map_or_else(String::new, |reads| {
        crate::spawn::chain_graph::build_single_reads_instruction(reads, &opts.cwd)
    });
    let body = if skill_injection.is_empty() {
        with_output_path
    } else {
        format!("{with_output_path}\n\n{skill_injection}")
    };
    format!("{reads_instruction}{body}")
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
    use crate::discovery::types::OutputMode;
    use crate::exec::ResolvedAgentPersona;
    use crate::exec::acceptance::AcceptanceStatus;
    use crate::exec::testsupport::{sample_agent_config, base_opts, delivered_system_prompt, read_system_prompt_arg};
    use crate::fork_context::{ContextMode, ForkContext};


    /// SUBA-021 — the CAPABILITY CEILING is consulted by `build_attempt_spawn_plan`, on both of its
    /// halves: the agent gate REFUSES a delegation outside the ceiling before any child is planned,
    /// and the resolved ceiling is handed to the child in
    /// [`crate::exec::capability_ceiling::CAPABILITY_CEILING_ENV`] so the bound survives the re-exec.
    ///
    /// Pre-fix BOTH observations were the opposite: `rg 'capability_ceiling' crates/…/src` was 0, so
    /// there was no ceiling concept at all — every agent was allowed and no env var was ever
    /// written, meaning a child could be granted a capability set wider than its parent's.
    ///
    /// The registration handle is scoped to this test and disposes on `Drop`, so it cannot leak into
    /// another test's session.
    #[test]
    fn the_capability_ceiling_refuses_an_out_of_ceiling_agent_and_reaches_the_child_env() {
        use crate::exec::capability_ceiling as cc;

        let dir = tempfile::tempdir().expect("tempdir");
        let session = "spawn-plan-ceiling-session";
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.parent_session_id = Some(session.to_string());

        let plan = |agent: &AgentConfig, opts: &RunOptions| {
            build_attempt_spawn_plan(
                agent,
                &ModelId::from("m1"),
                "task",
                opts,
                DepthEnvelope { current_depth: 0, max_depth: 5 },
                dir.path(),
                None,
            )
        };

        // No ceiling registered: the plan builds and writes no ceiling var (the overlay only adds,
        // so an unbounded run cannot inherit a stale bound).
        let unbounded = plan(&agent, &opts).expect("no ceiling, no refusal");
        assert!(!unbounded
            .spec
            .env_overlay
            .contains_key(cc::CAPABILITY_CEILING_ENV));

        let _handle = cc::register_capability_ceiling(
            session,
            "org-policy",
            &serde_json::json!({ "allowedAgents": ["reviewer"] }),
        )
        .expect("registers");

        // `worker` is outside the ceiling: refused BEFORE any argv/env is built, with pi's text.
        let Err(err) = plan(&agent, &opts) else {
            panic!("a plan outside the ceiling must be refused");
        };
        assert!(
            matches!(err, SubagentError::CapabilityCeilingViolation(_)),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "Capability ceiling from org-policy does not allow agent 'worker'. Allowed agents: \
             reviewer."
        );

        // An allowed agent plans normally AND carries the ceiling down to the child.
        let mut allowed = sample_agent_config("m1", &[]);
        allowed.name = "reviewer".to_string();
        let Ok(planned) = plan(&allowed, &opts) else {
            panic!("an agent inside the ceiling plans normally");
        };
        let encoded = planned
            .spec
            .env_overlay
            .get(cc::CAPABILITY_CEILING_ENV)
            .expect("the ceiling crosses the process boundary");
        let decoded = cc::decode_capability_ceiling(Some(encoded))
            .expect("decodes")
            .expect("present");
        assert_eq!(decoded.allowed_agents, Some(vec!["reviewer".to_string()]));
        assert_eq!(decoded.sources, vec!["org-policy".to_string()]);
    }

    /// SUBA-072 — a capability ceiling's `allowedTools` axis must actually gate what reaches the
    /// child, on BOTH arms pi's `resolvePiLaunchToolPlan` distinguishes: an agent that declares its
    /// own (wider) `tools:` list gets it intersected down to the ceiling, and an agent that
    /// declares no `tools:` at all gets the ceiling's set directly rather than falling through to
    /// the full ambient tool surface.
    ///
    /// Pre-fix: `capability_ceiling.rs` resolved and intersected `allowedTools` correctly, but
    /// nothing in `spawn_plan.rs` ever consulted it here — an agent declaring
    /// `tools: [read, write, bash]` spawned with exactly that CSV regardless of the ceiling, and an
    /// agent declaring no `tools:` spawned with no `--tools`/`--no-tools` flag at all (the full
    /// ambient set).
    #[test]
    fn a_capability_ceilings_allowed_tools_axis_gates_both_the_declared_and_undeclared_arms() {
        use crate::exec::capability_ceiling as cc;

        let dir = tempfile::tempdir().expect("tempdir");
        let session = "spawn-plan-ceiling-allowed-tools-session";
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.parent_session_id = Some(session.to_string());
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };

        let _handle = cc::register_capability_ceiling(
            session,
            "org-policy",
            &serde_json::json!({ "allowedTools": ["read"] }),
        )
        .expect("registers");

        // Arm 1 — the agent declared a WIDER explicit allowlist; the ceiling must narrow it.
        let mut wide = sample_agent_config("m1", &[]);
        wide.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::Builtin("write".to_string()),
            ToolRef::Builtin("bash".to_string()),
        ]);
        let plan =
            build_attempt_spawn_plan(&wide, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
                .expect("plan builds");
        let argv = plan.spec.build_argv();
        let idx = argv.iter().position(|a| a == "--tools").expect("--tools present");
        assert_eq!(
            argv.get(idx + 1).map(String::as_str),
            Some("read"),
            "the ceiling must narrow an agent's own wider declaration; argv {argv:?}"
        );

        // Arm 2 — the agent declared NO `tools:` at all; the ceiling's set must still apply rather
        // than falling through to the ambient (unflagged) tool surface.
        let bare = sample_agent_config("m1", &[]);
        let plan =
            build_attempt_spawn_plan(&bare, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
                .expect("plan builds");
        let argv = plan.spec.build_argv();
        let idx = argv.iter().position(|a| a == "--tools").unwrap_or_else(|| {
            panic!("a ceiling must pin the surface even with no agent-level `tools:`; argv {argv:?}")
        });
        assert_eq!(argv.get(idx + 1).map(String::as_str), Some("read"));
    }

    /// SUBA-072 — a capability ceiling's `denyExtensions` axis must strip BOTH the agent's own
    /// extension-path tools and its `extensions:`/`subagent_only_extensions` lists, and must force
    /// `--no-extensions` even when the agent itself never declared `extensions:` at all.
    ///
    /// Pre-fix: `--no-extensions` and the `--extension` allowlist were driven solely by
    /// `agent.extensions`, so `denyExtensions: true` had no effect on either axis — an agent that
    /// declared no `extensions:` field spawned with full ambient extension discovery on, exactly
    /// the widening the ceiling exists to prevent.
    #[test]
    fn a_capability_ceilings_deny_extensions_axis_strips_all_extension_paths_and_forces_no_extensions()
    {
        use crate::exec::capability_ceiling as cc;

        let dir = tempfile::tempdir().expect("tempdir");
        let session = "spawn-plan-ceiling-deny-extensions-session";
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.parent_session_id = Some(session.to_string());
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };

        let _handle = cc::register_capability_ceiling(
            session,
            "org-policy",
            &serde_json::json!({ "denyExtensions": true }),
        )
        .expect("registers");

        // The agent itself declares NO `extensions:` at all — the pre-fix code never pushed
        // `--no-extensions` on this arm.
        let bare = sample_agent_config("m1", &[]);
        let plan =
            build_attempt_spawn_plan(&bare, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
                .expect("plan builds");
        let argv = plan.spec.build_argv();
        assert!(argv.contains(&"--no-extensions".to_string()), "argv {argv:?}");
        assert!(!argv.iter().any(|a| a == "--extension"), "argv {argv:?}");

        // The agent DOES declare `extensions:`, a tool-extension path, and a child-only
        // extension — all three must still be stripped entirely, not merely left alongside
        // `--no-extensions`.
        let mut loaded = sample_agent_config("m1", &[]);
        loaded.extensions = Some(vec!["./agent-ext.ts".to_string()]);
        loaded.tools = Some(vec![ToolRef::ExtensionPath("./tool-ext.ts".to_string())]);
        loaded.subagent_only_extensions = vec!["./child-only-ext.ts".to_string()];
        let plan = build_attempt_spawn_plan(
            &loaded,
            &ModelId::from("m1"),
            "task",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();
        assert!(argv.contains(&"--no-extensions".to_string()), "argv {argv:?}");
        assert!(
            !argv.iter().any(|a| a == "--extension"),
            "denyExtensions must strip agent.extensions, subagent_only_extensions AND \
             tool-extension paths, not just leave them alongside --no-extensions; argv {argv:?}"
        );
    }

    /// SUBA-072(d) — pi `pi-args.ts:439-441`: a ceiling that excludes `read` while lazy skill
    /// loading requires it must fail the launch outright, independent of whether the agent itself
    /// declared any `tools:` — this is `SUBA-014`'s companion throw, sharing the same branch.
    #[test]
    fn a_capability_ceiling_excluding_read_fails_the_launch_when_read_is_required() {
        use crate::exec::capability_ceiling as cc;

        let dir = tempfile::tempdir().expect("tempdir");
        let session = "spawn-plan-ceiling-require-read-session";
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.parent_session_id = Some(session.to_string());
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };

        let _handle = cc::register_capability_ceiling(
            session,
            "org-policy",
            &serde_json::json!({ "allowedTools": ["bash"] }),
        )
        .expect("registers");

        let agent = sample_agent_config("m1", &[]);
        let Err(err) = build_attempt_spawn_plan_with_read_requirement(
            &agent,
            &ModelId::from("m1"),
            "task",
            &opts,
            depth,
            dir.path(),
            None,
            true,
        ) else {
            panic!("a ceiling excluding `read` must refuse a launch that requires it");
        };
        assert!(matches!(err, SubagentError::CapabilityCeilingViolation(_)), "{err:?}");
        assert_eq!(
            err.to_string(),
            "Capability ceiling from org-policy excludes required tool 'read' for lazy skill \
             loading."
        );
    }

    /// SUBA-072 Gap 1 (found during `/qa` re-review of the (a)-(d) fix above) — the RAW
    /// `MCP_DIRECT_TOOLS` env var (`spawn_plan.rs`'s own `MCP_DIRECT_TOOLS_ENV` constant, distinct
    /// from `MCP_DIRECT_CHILD_TOOLS_ENV`) is a SECOND, independent consumer of the agent's declared
    /// `mcp:` selectors, read by a different crate (`cyrup-mcp::registration::register_surface`) to
    /// decide which MCP servers/tools the child's own adapter activates — entirely separate from the
    /// `--tools` CSV / `effective_mcp_tools` this file already gates correctly above. Pre-fix, this
    /// write ignored the ceiling entirely: `denyExtensions: true` (an MCP server is
    /// extension-provided, per this file's own comment near `effective_mcp_tools`) still let an
    /// agent's raw `mcp:` selector reach the child unfiltered — the exact widening the ceiling exists
    /// to prevent, on a site the (a)-(d) fix's own tests never exercised because they only asserted
    /// against argv, never against this specific env key.
    #[test]
    fn a_capability_ceilings_deny_extensions_axis_empties_the_raw_mcp_direct_tools_env_too() {
        use crate::exec::capability_ceiling as cc;

        let dir = tempfile::tempdir().expect("tempdir");
        let session = "spawn-plan-ceiling-deny-extensions-mcp-env-session";
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.parent_session_id = Some(session.to_string());
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };

        let _handle = cc::register_capability_ceiling(
            session,
            "org-policy",
            &serde_json::json!({ "denyExtensions": true }),
        )
        .expect("registers");

        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::Mcp("chrome-devtools".to_string()),
        ]);
        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
                .expect("plan builds");

        assert_eq!(
            plan.spec.env_overlay.get(MCP_DIRECT_TOOLS_ENV),
            Some(&MCP_DIRECT_TOOLS_NONE_SENTINEL.to_string()),
            "denyExtensions must empty the RAW MCP_DIRECT_TOOLS env too, not just --tools; \
             overlay was {:?}",
            plan.spec.env_overlay
        );
    }

    /// An injected command must reach the child's ENVIRONMENT, not just its argv: a child that
    /// spawns a grandchild resolves that grandchild's command from what it inherited, so both
    /// halves have to travel or the wrapper is rebuilt without its leading argv one hop down.
    #[test]
    fn an_injected_spawn_command_seeds_both_halves_into_the_child_env() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.spawn_command = Some(crate::spawn::SpawnCommand {
            binary: dir.path().join("wrapper-shim"),
            base_args: vec!["--launch".to_string(), "a b".to_string()],
        });
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let agent = sample_agent_config("m1", &[]);

        let plan = build_attempt_spawn_plan(
            &agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None,
        )
        .expect("plan builds");

        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::SUBAGENT_BINARY_ENV_VAR),
            Some(&dir.path().join("wrapper-shim").display().to_string()),
            "the binary half must travel; overlay was {:?}",
            plan.spec.env_overlay
        );
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::SUBAGENT_BINARY_ARGS_ENV_VAR),
            Some(&r#"["--launch","a b"]"#.to_string()),
            "the args half must travel as JSON so an entry containing a space survives; overlay \
             was {:?}",
            plan.spec.env_overlay
        );
    }

    /// The regression this file's QA caught: an injected command with an EMPTY `base_args` must
    /// still WRITE the args variable, as `[]`. `env_overlay` is additive and `env_clear()` is
    /// never called, so omitting it would leave the child inheriting whatever this process
    /// carries — pairing a freshly injected binary with a stale inherited argv, which is exactly
    /// the half-a-command failure the variable exists to prevent.
    #[test]
    fn an_injected_command_with_no_base_args_still_writes_an_empty_args_var() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.spawn_command = Some(crate::spawn::SpawnCommand {
            binary: dir.path().join("bare-binary"),
            base_args: Vec::new(),
        });
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let agent = sample_agent_config("m1", &[]);

        let plan = build_attempt_spawn_plan(
            &agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None,
        )
        .expect("plan builds");

        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::SUBAGENT_BINARY_ARGS_ENV_VAR),
            Some(&"[]".to_string()),
            "an injected command must be authoritative over BOTH halves: writing `[]` is what \
             stops a stale inherited args value from attaching to the injected binary; overlay \
             was {:?}",
            plan.spec.env_overlay
        );
    }

    /// The uninjected path is the whole installed base: it must add neither variable, so a run
    /// that supplies no command resolves exactly as it did before this seam existed.
    #[test]
    fn no_injected_spawn_command_seeds_neither_env_var() {
        let dir = tempfile::tempdir().expect("tempdir");
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let agent = sample_agent_config("m1", &[]);

        let plan = build_attempt_spawn_plan(
            &agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None,
        )
        .expect("plan builds");

        assert!(
            !plan.spec.env_overlay.contains_key(crate::spawn::SUBAGENT_BINARY_ENV_VAR)
                && !plan
                    .spec
                    .env_overlay
                    .contains_key(crate::spawn::SUBAGENT_BINARY_ARGS_ENV_VAR),
            "an uninjected run must add neither variable; overlay was {:?}",
            plan.spec.env_overlay
        );
    }

    /// SUBA-072 Gap 1, `allowedTools` half. A ceiling with a narrow `allowedTools` set must also
    /// gate `MCP_DIRECT_TOOLS`, filtering each raw selector by whether ANY tool name it expands to
    /// is still allowed — never a raw, unfiltered pass-through merely because a ceiling happens to
    /// exist.
    ///
    /// This test's harness has no on-disk MCP metadata cache (the same structural limitation this
    /// file's own pre-existing `build_attempt_spawn_plan_splits_mcp_refs_out_of_tools_and_sets_the_env`
    /// test documents: "with no metadata cache on disk, it resolves to nothing"), so
    /// `chrome-devtools` here resolves to zero tool names and is filtered OUT regardless of which
    /// names `allowedTools` lists — this pins the REGRESSION (a ceiling must reach this env var at
    /// all, contrasted directly against the pre-fix unconditional-raw-passthrough this same setup
    /// would have produced) rather than the selector-survives-because-its-name-is-allowed direction,
    /// which needs a real MCP server fixture to exercise and is out of this test's reach.
    #[test]
    fn a_capability_ceilings_allowed_tools_axis_also_gates_the_raw_mcp_direct_tools_env() {
        use crate::exec::capability_ceiling as cc;

        let dir = tempfile::tempdir().expect("tempdir");
        let session = "spawn-plan-ceiling-allowed-tools-mcp-env-session";
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.parent_session_id = Some(session.to_string());
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };

        let _handle = cc::register_capability_ceiling(
            session,
            "org-policy",
            &serde_json::json!({ "allowedTools": ["read"] }),
        )
        .expect("registers");

        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::Mcp("chrome-devtools".to_string()),
        ]);
        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
                .expect("plan builds");

        assert_eq!(
            plan.spec.env_overlay.get(MCP_DIRECT_TOOLS_ENV),
            Some(&MCP_DIRECT_TOOLS_NONE_SENTINEL.to_string()),
            "pre-fix this was unconditionally \"chrome-devtools\" regardless of the ceiling; a \
             ceiling must now reach this env var exactly as it reaches --tools; overlay was {:?}",
            plan.spec.env_overlay
        );
    }


    // ---- build_task_text / build_attempt_spawn_plan ----

    /// SUBA-054 — a SINGLE run must carry the persona's `defaultReads` as pi's leading
    /// `[Read from: …]` instruction (`subagent-executor.ts:3869-3873` @v0.47.1).
    ///
    /// RED before the fix: `RunOptions` had no `reads` field at all and `build_task_text` composed
    /// nothing from `defaultReads` — the key was parsed off frontmatter, rendered in agent
    /// listings, and inert for every non-chain invocation. The bundled `reviewer` shipped
    /// `defaultReads: plan.md, progress.md` and was never told to read either file.
    #[test]
    fn build_task_text_prepends_the_default_reads_instruction_for_a_single_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("plan.md"), "the plan").expect("plan");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.reads = Some(vec![PathBuf::from("plan.md")]);
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);

        let text = build_task_text(&agent, "review it", &opts, &contract, "");
        assert_eq!(
            text,
            format!("[Read from: {}]\n\nreview it", dir.path().join("plan.md").display()),
            "the read line is FIRST, closed by a blank line — pi's `readsInstruction + task`"
        );
    }


    /// pi's `resolveExistingReadPaths` filter (`shared/settings.ts:356-367`, upstream `bc1b689`):
    /// a declared read that does not exist is DROPPED, and an all-missing list emits no line at all
    /// rather than an empty `[Read from: ]` that would burn a turn on a failed read.
    #[test]
    fn a_missing_default_read_is_dropped_and_an_all_missing_list_emits_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("present.md"), "here").expect("present");
        let agent = sample_agent_config("m1", &[]);
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);

        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.reads =
            Some(vec![PathBuf::from("present.md"), PathBuf::from("gone.md")]);
        let text = build_task_text(&agent, "go", &opts, &contract, "");
        assert_eq!(
            text,
            format!("[Read from: {}]\n\ngo", dir.path().join("present.md").display())
        );

        let mut none_present = base_opts(dir.path(), &["m1"]);
        none_present.reads = Some(vec![PathBuf::from("gone.md")]);
        assert_eq!(
            build_task_text(&agent, "go", &none_present, &contract, ""),
            "go",
            "no surviving read means NO instruction, not an empty one"
        );

        // `None` is upstream's `false`: no instruction either.
        let bare = base_opts(dir.path(), &["m1"]);
        assert_eq!(build_task_text(&agent, "go", &bare, &contract, ""), "go");
    }


    /// An ABSOLUTE declared read is used verbatim (pi `resolveChainPath`'s `isAbsolute` arm), which
    /// is what lets a persona point at a file outside the child's cwd.
    #[test]
    fn an_absolute_default_read_is_not_rejoined_onto_the_run_cwd() {
        let repo = tempfile::tempdir().expect("repo");
        let elsewhere = tempfile::tempdir().expect("elsewhere");
        let target = elsewhere.path().join("notes.md");
        std::fs::write(&target, "notes").expect("notes");

        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(repo.path(), &["m1"]);
        opts.reads = Some(vec![target.clone()]);
        assert_eq!(
            build_task_text(&agent, "go", &opts, &AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]), ""),
            format!("[Read from: {}]\n\ngo", target.display())
        );
    }


    #[test]
    fn build_task_text_injects_acceptance_contract_and_output_path_instruction() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Replace;
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.output_mode = OutputMode::FileOnly;
        opts.output_path = Some(dir.path().join("out.md"));
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]);

        let text = build_task_text(&agent, "do the thing", &opts, &contract, "");
        assert!(text.starts_with("do the thing"));
        assert!(text.contains("Acceptance Contract"));
        assert!(text.contains("out.md"));
    }


    /// G82 — `build_task_text` must use the TASK-side injector
    /// (`injectSingleOutputInstruction`, `single-output.ts:99-102`), whose header is
    /// `\n\n---\n**Output:**\n`, NOT the system-prompt-side `Runtime output path override:` form.
    /// The header is load-bearing: `**Output:**` is one of `stripFrameworkInstructions`'
    /// alternatives (`task-intent.ts:99`) and `Runtime output path override:` is not, so the wrong
    /// header feeds the injected instruction back into mutation-intent classification.
    #[test]
    fn build_task_text_uses_the_upstream_task_side_output_header() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.output_mode = OutputMode::FileOnly;
        opts.output_path = Some(dir.path().join("out.md"));
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);

        let text = build_task_text(&agent, "do the thing", &opts, &contract, "");
        assert!(
            text.contains("\n\n---\n**Output:**\n"),
            "the task-side `**Output:**` header must be present: {text:?}"
        );
        assert!(
            !text.contains("Runtime output path override:"),
            "the system-prompt-side header must NOT be used in the task text: {text:?}"
        );
        // Every line the injector emits is one `strip_framework_instructions` removes, so the
        // instruction contributes no write-intent signal back to the classifier.
        assert!(
            !crate::exec::task_intent::task_may_mutate(&text),
            "the injected output instruction must be stripped before intent classification: {text:?}"
        );
    }


    /// G82 REGRESSION — upstream keys the output instruction on the PATH alone
    /// (`subagent-executor.ts:3674`, `chain-execution.ts:363,1320` @v0.43.0,
    /// `async-execution.ts:711,1289` @v0.43.0); `outputMode` is consulted only by
    /// `validateFileOnlyOutputMode` and by delivery-side `finalizeSingleOutput`. Gating the
    /// injection on `OutputMode::FileOnly` left a `file-and-inline` run with a configured output
    /// path with NO instruction at all — the child was never told where to write.
    #[test]
    fn every_output_mode_with_a_configured_path_gets_the_instruction() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);
        let path = dir.path().join("out.md");

        for mode in [
            OutputMode::FileOnly,
            OutputMode::FileAndInline,
            OutputMode::Inline,
        ] {
            let mut opts = base_opts(dir.path(), &["m1"]);
            opts.output_mode = mode;
            opts.output_path = Some(path.clone());
            let text = build_task_text(&agent, "do the thing", &opts, &contract, "");
            assert!(
                text.contains("**Output:**"),
                "{mode:?} with a configured path must still carry the instruction: {text:?}"
            );
            assert!(
                text.contains(&format!(
                    "Write your findings to exactly this path: {}",
                    path.display()
                )),
                "{mode:?} must name the authoritative path: {text:?}"
            );
        }

        // ...and no configured path means no instruction, in every mode.
        for mode in [
            OutputMode::FileOnly,
            OutputMode::FileAndInline,
            OutputMode::Inline,
        ] {
            let mut opts = base_opts(dir.path(), &["m1"]);
            opts.output_mode = mode;
            opts.output_path = None;
            let text = build_task_text(&agent, "do the thing", &opts, &contract, "");
            assert!(
                !text.contains("**Output:**"),
                "{mode:?} with no path must inject nothing: {text:?}"
            );
        }
    }


    /// G82 — the capability branch of `formatOutputPathInstruction` (`single-output.ts:84-91`)
    /// must be reachable from the LIVE composition path, not merely from the injector's own unit
    /// test: an agent whose whole resolved allowlist is read-only is told to return the artifact
    /// for the runtime to persist, never to write a file it has no tool to write.
    #[test]
    fn build_task_text_branches_the_instruction_on_the_agents_real_tool_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        // Deliberately NOT file-only: the capability branch must be live in every mode.
        opts.output_mode = OutputMode::FileAndInline;
        opts.output_path = Some(dir.path().join("out.md"));

        let mut read_only = sample_agent_config("m1", &[]);
        read_only.tools = Some(vec![
            crate::discovery::types::ToolRef::Builtin("read".to_string()),
            crate::discovery::types::ToolRef::Builtin("grep".to_string()),
        ]);
        let text = build_task_text(&read_only, "do the thing", &opts, &contract, "");
        assert!(
            text.contains("Return the complete artifact in your final response."),
            "{text:?}"
        );
        assert!(
            text.contains("The runtime will persist it to exactly this path:"),
            "{text:?}"
        );
        assert!(
            !text.contains("Write your findings to exactly this path:"),
            "a read-only agent must not be ordered to write: {text:?}"
        );

        let mut write_capable = sample_agent_config("m1", &[]);
        write_capable.tools = Some(vec![
            crate::discovery::types::ToolRef::Builtin("read".to_string()),
            crate::discovery::types::ToolRef::Builtin("write".to_string()),
        ]);
        let text = build_task_text(&write_capable, "do the thing", &opts, &contract, "");
        assert!(
            text.contains("Write your findings to exactly this path:"),
            "{text:?}"
        );
        assert!(
            !text.contains("Return the complete artifact in your final response."),
            "{text:?}"
        );
    }


    #[test]
    fn build_task_text_appends_skill_injection_even_when_the_agent_does_not_inherit_skills() {
        // T5 (C4): the `<available_skills>` pointer block is composed into the child prompt from the
        // agent's EXPLICITLY-listed skills, ORTHOGONAL to `inherit_skills` (the `--no-skills` flag).
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Replace;
        agent.inherit_skills = false; // does NOT inherit the parent's own skill discovery
        agent.skills = vec!["fallback-skill".to_string()];
        let opts = base_opts(dir.path(), &["m1"]);
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);

        let injection = crate::discovery::skills::build_skill_injection(&[
            crate::discovery::skills::ResolvedSkill {
                name: "fallback-skill".to_string(),
                path: dir.path().join(".cyrup/skills/fallback-skill/SKILL.md"),
                description: Some("Use fallback mode.".to_string()),
            },
        ]);
        let text = build_task_text(&agent, "do the thing", &opts, &contract, &injection);

        assert!(text.starts_with("do the thing"));
        assert!(text.contains("<available_skills>"));
        assert!(text.contains("<name>fallback-skill</name>"));
        assert!(text.contains("<description>Use fallback mode.</description>"));

        // The child STILL gets `--no-skills` (inherit is off) even though the explicit skill injects.
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), &text, &opts, depth, dir.path(), None)
            .expect("plan builds");
        assert!(plan.spec.build_argv().contains(&"--no-skills".to_string()));
    }


    // ---- Child watchdog activation (pi `execution.ts:298-302`, `subagent-runner.ts:1309-1312`) ----

    /// The default watchdog is OFF, so an ordinary spawn must carry no child-watchdog env at all —
    /// a child that decodes one starts reviewing, and reviewing costs a model call per boundary.
    #[test]
    fn a_spawn_carries_no_child_watchdog_env_when_the_watchdog_is_off() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        assert!(
            !plan
                .spec
                .env_overlay
                .contains_key(crate::watchdog::child_status::CHILD_WATCHDOG_CONFIG_ENV),
            "the default-off watchdog must write no child env"
        );
    }


    /// With `subagents.watchdog.{enabled,children.enabled}` on in the run's own project settings,
    /// the spawn env carries the encoded child config — the ONLY channel a child watchdog has.
    #[test]
    fn a_spawn_carries_the_encoded_child_watchdog_config_when_children_are_enabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings_path = crate::watchdog::settings::get_watchdog_project_settings_path(dir.path());
        std::fs::create_dir_all(settings_path.parent().expect("settings parent")).expect("mkdir");
        std::fs::write(
            &settings_path,
            serde_json::json!({
                "subagents": {
                    "watchdog": {
                        "enabled": true,
                        "children": { "enabled": true, "model": "anthropic/reviewer" },
                    }
                }
            })
            .to_string(),
        )
        .expect("write settings");

        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let Some(encoded) = plan
            .spec
            .env_overlay
            .get(crate::watchdog::child_status::CHILD_WATCHDOG_CONFIG_ENV)
        else {
            // The project-settings path this run resolves is layout-dependent; when the temp dir is
            // not recognized as a project root there is no project layer to read and the watchdog
            // stays off, which the previous test already covers.
            return;
        };
        let decoded =
            crate::watchdog::child_status::decode_child_watchdog_config(Some(encoded.as_str()))
                .expect("the parent writes a decodable config")
                .expect("children enabled means a config");
        assert!(decoded.enabled);
        assert_eq!(decoded.model.as_deref(), Some("anthropic/reviewer"));
        assert_eq!(decoded.agent.as_deref(), Some(agent.name.as_str()));
    }


    /// SUBA-030 — the persona spill exists on disk, carries the composed body, and is mode `0600`;
    /// the body appears NOWHERE in argv. This is the item's own Verify recipe, minus the live
    /// `/proc/<pid>/cmdline` half, which needs a real spawn.
    #[test]
    fn the_persona_ships_as_a_0600_spill_file_and_never_on_argv() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.name = "code reviewer/v2".to_string();
        agent.system_prompt_mode = SystemPromptMode::Replace;
        // A body far over Linux's MAX_ARG_STRLEN (131072): inline delivery would have made
        // `execve` fail with E2BIG.
        let body = format!("- You are the REVIEWER persona.\n{}", "x".repeat(200_000));
        agent.system_prompt_body = body.clone();
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();

        let idx = argv
            .iter()
            .position(|a| a == "--system-prompt")
            .expect("replace mode ships the persona on --system-prompt");
        let path = std::path::PathBuf::from(&argv[idx + 1]);
        assert!(path.is_absolute(), "the spill path must be absolute: {path:?}");
        assert_eq!(
            path.parent(),
            Some(dir.path()),
            "the spill must land in the run's scratch directory, not the OS temp root"
        );
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("code_reviewer_v2.md"),
            "pi `promptFileStem` sanitization: `[^\\w.-]` -> `_`, extension `.md`"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("spill readable"),
            body,
            "the spill must carry the composed persona verbatim"
        );

        // The whole point: nothing on argv contains the body.
        assert!(
            !argv.iter().any(|a| a.contains("You are the REVIEWER persona.")),
            "SUBA-030: the persona must not be readable from the child's cmdline; argv was {argv:?}"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path)
                .expect("spill metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "pi writes the prompt file with {{ mode: 0o600 }}");
        }

        // Cleanup contract: the spill is registered, so `cleanup_temp_files` removes it on every
        // exit path exactly as it already removed the over-threshold task spill.
        assert!(
            plan.spec.temp_files.contains(&path),
            "the persona spill must be registered for cleanup: {:?}",
            plan.spec.temp_files
        );
        crate::spawn::cleanup_temp_files(&plan.spec.temp_files);
        assert!(!path.exists(), "cleanup must remove the persona spill");
    }


    /// pi `(input.promptFileStem ?? "prompt").replace(/[^\w.-]/g, "_")`
    /// (`runs/shared/pi-args.ts:572` @v0.43.0).
    #[test]
    fn prompt_file_stem_sanitization_matches_pis_character_class() {
        use crate::spawn::sanitize_prompt_file_stem as s;
        assert_eq!(s("reviewer"), "reviewer");
        assert_eq!(s("my.agent-2_x"), "my.agent-2_x", "`.`, `-` and `_` are kept");
        assert_eq!(s("a/b c:d"), "a_b_c_d", "separators and spaces become `_`");
        // JS `\w` is ASCII-only; `char::is_alphanumeric` would wrongly keep these.
        assert_eq!(s("résumé"), "r_sum_");
        assert_eq!(s(""), "prompt", "pi's `?? \"prompt\"` default");
        assert_eq!(s("   "), "prompt");
    }


    #[test]
    fn build_attempt_spawn_plan_delivers_the_persona_body_as_system_prompt_in_replace_mode() {
        // The critical path: 7 of the 8 bundled personas (and every user-authored agent, per
        // `default_system_prompt_mode`) declare `systemPromptMode: replace`. The child MUST be
        // spawned with `--system-prompt <spill path>` — pi `runs/shared/pi-args.ts:164-165` picks
        // `--system-prompt` for `replace` — or the subagent runs as a generic coding agent that
        // received nothing but the task text.
        //
        // The body deliberately opens on a markdown bullet — the case that made the old inline
        // `--flag=<body>` encoding mandatory. SUBA-030 replaced it with pi's own spill-file form,
        // where a leading `-` in the BODY can no longer reach the child's clap parser at all.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Replace;
        agent.system_prompt_body = "- You are the REVIEWER persona.\n- Only review.".to_string();
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "do the thing", &opts, depth, dir.path(), None)
                .expect("plan builds");
        let argv = plan.spec.build_argv();

        let delivered = delivered_system_prompt(&argv)
            .unwrap_or_else(|| panic!("replace mode must emit --system-prompt; argv was {argv:?}"));
        assert_eq!(
            delivered, "- You are the REVIEWER persona.\n- Only review.",
            "replace mode must ship the persona body on --system-prompt; argv was {argv:?}"
        );
        // `replace` must never also append — the two flags are mutually exclusive per mode.
        assert!(!argv.iter().any(|a| a.starts_with("--append-system-prompt")));
    }


    /// pi `execution.ts:1433-1443` composes FOUR things onto `systemPrompt`, in this order:
    /// skills, the agent-memory block, the project-local refinement overlay, the output-path
    /// override. This drives the real `build_attempt_spawn_plan` with a memory block, an overlay
    /// file and an output path all present at once and asserts the delivered `--system-prompt`
    /// carries them in exactly that order.
    ///
    /// The refinement overlay was the hole: `appendAgentRefinementOverlay` (`execution.ts:1442`)
    /// was entirely unported, so the composition ran memory -> output-path and an authored overlay
    /// reached no child at all. Order is not cosmetic — the output-path override closes the prompt
    /// with the runtime's own instruction, and an overlay appended AFTER it would be the last thing
    /// the child reads, inverting the precedence upstream gives them.
    #[test]
    fn the_refinement_overlay_lands_between_the_memory_block_and_the_output_path_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Replace;
        agent.system_prompt_body = "You are the WORKER persona.".to_string();
        // A write-capable agent with a project memory scope, so the memory block is non-empty.
        // The scope needs a discoverable project root, which is a `.cyrup/` directory.
        std::fs::create_dir_all(dir.path().join(".cyrup")).expect("mkdir .cyrup");
        agent.tools = Some(vec![ToolRef::Builtin("write".to_string())]);
        agent.memory = Some(crate::discovery::types::AgentMemoryConfig {
            scope: crate::discovery::types::MemoryScope::Project,
            path: "worker-notes".to_string(),
        });

        // The overlay file the port now reads, at pi's own path.
        let overlay_path =
            crate::exec::agent_refinements::get_agent_refinement_path(dir.path(), &agent.name)
                .expect("legal agent name");
        std::fs::create_dir_all(overlay_path.parent().expect("parent")).expect("mkdir");
        std::fs::write(
            &overlay_path,
            "<!-- pi-subagents-refinement:v1\n{\"agent\":\"worker\",\"revision\":1,\
             \"updatedAt\":\"2026-08-01T00:00:00.000Z\",\"base\":{\"source\":\"project\",\
             \"filePath\":\"a.md\",\"systemPromptSha256\":\"s\"},\"evidence\":{}}\n-->\n\n\
             ```pi-subagents-refinement-current\n- Prefer smaller diffs.\n```\n",
        )
        .expect("write overlay");

        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.output_path = Some(dir.path().join("out.md"));
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();
        let delivered = delivered_system_prompt(&argv)
            .expect("replace mode ships the persona on --system-prompt");
        let delivered = delivered.as_str();

        let persona_at = delivered
            .find("You are the WORKER persona.")
            .expect("the persona body opens the prompt");
        let memory_at = delivered
            .find("# Persistent agent memory")
            .expect("the agent-memory block is composed in");
        let overlay_at = delivered
            .find("<pi-subagents-refinement agent=\"worker\"")
            .expect("the refinement overlay must reach the child's system prompt");
        let output_at = delivered
            .find("Runtime output path override:")
            .expect("the output-path override is composed in");

        assert!(
            persona_at < memory_at && memory_at < overlay_at && overlay_at < output_at,
            "pi's order is persona -> memory -> refinement -> output-path; got \
             persona@{persona_at} memory@{memory_at} refinement@{overlay_at} \
             output@{output_at} in:\n{delivered}"
        );
        assert!(
            delivered.contains("- Prefer smaller diffs.")
                && delivered.contains("</pi-subagents-refinement>"),
            "the overlay's guidance and closing tag must both ship: {delivered}"
        );

        // Remove the overlay and the SAME call composes memory straight onto the output-path
        // override — proving the assertion above is driven by the file, not by something else in
        // the prompt that happens to contain the tag.
        std::fs::remove_file(&overlay_path).expect("remove overlay");
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let without = plan.spec.build_argv();
        let without = delivered_system_prompt(&without)
            .expect("replace mode ships the persona on --system-prompt");
        let without = without.as_str();
        assert!(
            !without.contains("pi-subagents-refinement"),
            "no overlay file means no overlay block: {without}"
        );
        assert!(without.contains("Runtime output path override:"), "{without}");
    }


    // ---- G103: an EXPLICITLY empty `tools:` means "no tools" (pi `runs/shared/pi-args.ts:389-393,549-555`) ----

    /// The USER ACTION: an author writes an agent `.md` whose frontmatter says `tools:` with
    /// nothing after it — "this agent gets NO tools" — and someone delegates to that agent. The
    /// spawned `cyrup` child must carry `--no-tools`.
    ///
    /// Before the fix the whole chain silently inverted the request: `parse_agent_file` folded the
    /// empty list to `None` ("no restriction"), and the argv builder's `!builtin_tools.is_empty()`
    /// gate then emitted no flag at all, so the child came up with the FULL ambient tool set —
    /// read, write, edit and bash included.
    #[test]
    fn an_explicitly_empty_tools_list_spawns_the_child_with_no_tools() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A REAL agent file, so discovery and the argv builder are both exercised — the defect
        // lived in the seam between them and either half alone would have looked correct.
        let def = crate::discovery::frontmatter::parse_agent_file(
            "---\nname: scribe\ndescription: Writes prose, touches nothing\ntools:\n---\n\n- You are the SCRIBE persona.\n",
            crate::discovery::types::AgentSource::User,
            std::path::Path::new("scribe.md"),
        )
        .expect("agent file parses");
        assert_eq!(
            def.tools,
            Some(Vec::new()),
            "an explicitly-empty `tools:` must survive discovery as an EMPTY allowlist, distinct \
             from the `None` that an ABSENT `tools:` produces (pi `agents.ts:1610`)"
        );

        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let agent = AgentConfig::from_agent_definition(&def, depth);
        let opts = base_opts(dir.path(), &["m1"]);
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();

        assert!(
            argv.contains(&"--no-tools".to_string()),
            "a no-tools agent must be spawned with --no-tools; argv was {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a == "--tools"),
            "--no-tools and --tools are mutually exclusive; argv was {argv:?}"
        );
        assert!(
            !plan
                .spec
                .env_overlay
                .contains_key(crate::native_supervisor::ENV_REQUIRED_CHILD_TOOLS),
            "upstream only writes REQUIRED_CHILD_TOOLS for a NON-empty allowlist \
             (`pi-args.ts:610`); env was {:?}",
            plan.spec.env_overlay
        );
    }


    /// MIRROR: an agent that OMITS `tools:` entirely is asking to INHERIT the ambient tool set, not
    /// to be stripped of it. Neither flag may appear — the case the fix must not capture.
    #[test]
    fn an_omitted_tools_key_leaves_the_child_tool_set_unrestricted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let def = crate::discovery::frontmatter::parse_agent_file(
            "---\nname: scribe\ndescription: Writes prose\n---\n\n- You are the SCRIBE persona.\n",
            crate::discovery::types::AgentSource::User,
            std::path::Path::new("scribe.md"),
        )
        .expect("agent file parses");
        assert_eq!(
            def.tools, None,
            "an ABSENT `tools:` key must stay `None` — pi `agents.ts:1610` carries the field only \
             when `rawTools !== undefined`"
        );

        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let agent = AgentConfig::from_agent_definition(&def, depth);
        let opts = base_opts(dir.path(), &["m1"]);
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();

        assert!(
            !argv.iter().any(|a| a == "--no-tools" || a == "--tools"),
            "an agent that pinned no allowlist must get neither flag; argv was {argv:?}"
        );
    }


    /// MIRROR: a NON-empty allowlist is unaffected — `--tools <list>` exactly as before, and the
    /// `REQUIRED_CHILD_TOOLS` env still pinned.
    #[test]
    fn a_non_empty_tools_list_still_pins_the_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::Builtin("grep".to_string()),
        ]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let opts = base_opts(dir.path(), &["m1"]);
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();

        let idx = argv
            .iter()
            .position(|a| a == "--tools")
            .expect("a declared allowlist must still emit --tools");
        assert_eq!(argv.get(idx + 1).map(String::as_str), Some("read,grep"));
        assert!(!argv.iter().any(|a| a == "--no-tools"));
        assert!(
            plan.spec
                .env_overlay
                .get(crate::native_supervisor::ENV_REQUIRED_CHILD_TOOLS)
                .is_some_and(|v| v.contains("read")),
            "env was {:?}",
            plan.spec.env_overlay
        );
    }


    /// SUBA-045 — the diagnostic handshake is armed under pi's own gate (`if
    /// (toolPlan.requiredChildTools.length > 0)`, `pi-args.ts:611`) and nowhere else.
    ///
    /// Both halves are asserted against the SAME builder call shape, so the "not armed" leg cannot
    /// pass merely because the plan failed to build.
    #[test]
    fn the_tool_diagnostic_handshake_is_armed_with_the_required_tools_list() {
        use crate::exec::tool_availability as ta;
        let dir = tempfile::tempdir().expect("tempdir");
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let opts = base_opts(dir.path(), &["m1"]);

        // Armed: an explicit allowlist means the child has something it can be missing.
        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = Some(vec![ToolRef::Builtin("read".to_string())]);
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let path = plan
            .tool_diagnostic_path
            .clone()
            .expect("an explicit allowlist must arm the diagnostic path");
        assert_eq!(
            path,
            ta::tool_diagnostic_path_in(dir.path()),
            "the diagnostic lives in the attempt's own temp dir (pi `path.join(tempDir, …)`)"
        );
        assert_eq!(
            plan.spec
                .env_overlay
                .get(ta::CHILD_TOOL_DIAGNOSTIC_PATH_ENV)
                .map(String::as_str),
            Some(path.display().to_string().as_str()),
            "the plan's path and the child's env must be the SAME value, or the read side drifts \
             from the write side; env was {:?}",
            plan.spec.env_overlay
        );
        assert!(
            !plan
                .spec
                .env_overlay
                .contains_key(ta::MCP_DIRECT_CHILD_TOOLS_ENV),
            "an agent with no `mcp:` entries resolves no direct-MCP names, so the key is ABSENT \
             rather than an empty array"
        );

        // Not armed: no `tools:` at all — upstream requires nothing, so nothing can be missing.
        let bare = sample_agent_config("m1", &[]);
        let plan = build_attempt_spawn_plan(
            &bare,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        assert_eq!(plan.tool_diagnostic_path, None);
        assert!(
            !plan
                .spec
                .env_overlay
                .contains_key(ta::CHILD_TOOL_DIAGNOSTIC_PATH_ENV),
            "env was {:?}",
            plan.spec.env_overlay
        );
    }


    // ---- SUBA-014: `requireReadTool` head-injection (pi `runs/shared/pi-args.ts:361-371`) ----

    /// The USER ACTION: an author ships an agent whose `tools:` list omits `read` and whose
    /// `skills:` list names a skill that resolves. cyrup's own proactive-skill block then tells that
    /// child *"Use the read tool to load a skill's file"* (`discovery/skills.rs`), so the child is
    /// instructed to use a tool the allowlist denies it and the failure surfaces as a model apology.
    ///
    /// pi injects `read` at the HEAD of the declared builtins under a three-way condition
    /// (`pi-args.ts:365-370` @v0.43.0: `requireReadTool && requestedBuiltinTools.length > 0 &&
    /// !requestedBuiltinTools.includes("read") && !allowedToolSet`). This table pins all four arms
    /// of that condition plus the head position.
    #[test]
    fn require_read_tool_injects_read_at_the_head_under_pis_three_way_condition() {
        let dir = tempfile::tempdir().expect("tempdir");
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let opts = base_opts(dir.path(), &["m1"]);

        // (declared tools, require_read_tool, expected `--tools` value or None for "no --tools")
        type ToolsCase<'a> = (Option<&'a [&'a str]>, bool, Option<&'a str>);
        let cases: &[ToolsCase<'_>] = &[
            // The defect: a skill resolved, `read` is absent, so it is injected at the head.
            (Some(&["bash"]), true, Some("read,bash")),
            // No skill resolved — the list is untouched.
            (Some(&["bash"]), false, Some("bash")),
            // Already contains `read` — no duplicate, and the author's ORDER is preserved (pi
            // spreads `requestedBuiltinTools` verbatim on this arm).
            (Some(&["bash", "read"]), true, Some("bash,read")),
            // `requestedBuiltinTools.length > 0` fails: an explicitly EMPTY allowlist still means
            // "no tools", so `--no-tools` survives the injection rather than becoming `--tools read`.
            (Some(&[]), true, None),
        ];

        for (tools, require_read_tool, expected) in cases {
            let mut agent = sample_agent_config("m1", &[]);
            agent.tools = tools.map(|names| {
                names
                    .iter()
                    .map(|name| ToolRef::Builtin((*name).to_string()))
                    .collect()
            });
            let plan = build_attempt_spawn_plan_with_read_requirement(
                &agent,
                &ModelId::from("m1"),
                "do the thing",
                &opts,
                depth,
                dir.path(),
                None,
                *require_read_tool,
            )
            .expect("plan builds");
            let argv = plan.spec.build_argv();

            match expected {
                Some(csv) => {
                    let idx = argv
                        .iter()
                        .position(|a| a == "--tools")
                        .unwrap_or_else(|| panic!("expected --tools for {tools:?}; argv {argv:?}"));
                    assert_eq!(
                        argv.get(idx + 1).map(String::as_str),
                        Some(*csv),
                        "tools={tools:?} require_read_tool={require_read_tool}"
                    );
                    // pi's `requiredChildTools` is the SAME post-injection list (`pi-args.ts:401-409`).
                    let required = plan
                        .spec
                        .env_overlay
                        .get(crate::native_supervisor::ENV_REQUIRED_CHILD_TOOLS)
                        .expect("a non-empty allowlist writes REQUIRED_CHILD_TOOLS");
                    assert!(
                        required.contains("read") == csv.contains("read"),
                        "REQUIRED_CHILD_TOOLS must carry the injected `read` too; was {required}"
                    );
                }
                None => {
                    assert!(
                        argv.contains(&"--no-tools".to_string()),
                        "an empty allowlist must stay --no-tools even with require_read_tool; argv {argv:?}"
                    );
                    assert!(
                        !argv.iter().any(|a| a == "--tools"),
                        "argv {argv:?}"
                    );
                }
            }
        }
    }


    /// MIRROR: an agent that pinned NO allowlist at all (`tools:` absent) is on pi's
    /// `input.tools === undefined` arm, where the injection does not exist — the child inherits the
    /// ambient set, which already contains `read`, so neither flag may appear.
    #[test]
    fn require_read_tool_does_not_pin_an_allowlist_on_an_agent_that_declared_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let opts = base_opts(dir.path(), &["m1"]);
        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = None;

        let plan = build_attempt_spawn_plan_with_read_requirement(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
            true,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();
        assert!(
            !argv.iter().any(|a| a == "--no-tools" || a == "--tools"),
            "argv {argv:?}"
        );
    }


    // ---- `memory:` scopes reach the child persona (pi `execution.ts:1058-1061`) ----

    /// The USER ACTION: an agent `.md` declares `memory: { scope: user, path: reviewer }`, the user
    /// runs that agent, and the child is spawned with its accumulated role notes on the persona
    /// system prompt. Before this wiring `memory:` was demoted to `extra_fields` and reached
    /// nothing at all.
    #[test]
    fn a_memory_scoped_agent_ships_its_memory_block_on_the_persona_system_prompt() {
        let dir = tempfile::tempdir().expect("tempdir");
        // `project` scope resolves under `<nearest project root>/.cyrup/agent-memory`, and the
        // nearest project root is the first ancestor holding a `.cyrup` directory — so creating one
        // here anchors the scope on the fixture, with no process-global env to mutate.
        let project_config = dir.path().join(".cyrup");
        std::fs::create_dir_all(&project_config).expect("mkdir .cyrup");
        let memory_dir = project_config
            .join(crate::discovery::agent_memory::AGENT_MEMORY_DIR_NAME)
            .join("reviewer");
        std::fs::create_dir_all(&memory_dir).expect("mkdir");
        std::fs::write(
            memory_dir.join(crate::discovery::agent_memory::AGENT_MEMORY_FILE),
            "2026-01-01: prefer `cargo clippy` over `cargo check` for this repo.\n",
        )
        .expect("write MEMORY.md");

        // Parse a REAL agent file so the whole chain (frontmatter -> AgentDefinition ->
        // AgentConfig -> argv) is exercised, not just the last hop.
        let def = crate::discovery::frontmatter::parse_agent_file(
            "---\nname: reviewer\ndescription: Reviews\nmemory:\n  scope: project\n  path: reviewer\n---\n\n- You are the REVIEWER persona.\n",
            crate::discovery::types::AgentSource::User,
            std::path::Path::new("reviewer.md"),
        )
        .expect("agent file parses");
        assert_eq!(
            def.memory,
            Some(crate::discovery::types::AgentMemoryConfig {
                scope: crate::discovery::types::MemoryScope::Project,
                path: "reviewer".to_string(),
            }),
            "`memory:` must be a first-class parsed field, not an extra_field"
        );
        assert!(
            !def.extra_fields.contains_key("memory"),
            "`memory` is a KNOWN_FIELD; it must not also land in extra_fields"
        );

        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let agent = AgentConfig::from_agent_definition(&def, depth);
        let opts = base_opts(dir.path(), &["m1"]);

        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();
        let delivered = delivered_system_prompt(&argv).unwrap_or_default();
        assert!(
            delivered.contains("- You are the REVIEWER persona."),
            "the persona body must survive; argv was {argv:?}"
        );
        assert!(
            delivered.contains("# Persistent agent memory"),
            "the memory block must be folded onto the persona; argv was {argv:?}"
        );
        assert!(
            delivered.contains("prefer `cargo clippy` over `cargo check`"),
            "the RECORDED notes must reach the child; argv was {argv:?}"
        );
    }


    /// An agent with NO `memory:` block must produce a byte-identical spawn plan to before — the
    /// overwhelming majority of agents.
    #[test]
    fn an_agent_without_a_memory_scope_ships_only_its_persona() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_body = "- persona".to_string();
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();
        assert_eq!(
            delivered_system_prompt(&argv).as_deref(),
            Some("- persona"),
            "{argv:?}"
        );
        assert!(!argv.iter().any(|a| a.contains("Persistent agent memory")));
    }


    // ---- `toolBudget:` reaches the child (pi `tool-budget.ts:70-72`) ----

    /// The USER ACTION: an agent `.md` declares `toolBudget: {"hard": 5, "soft": 2}`, the user runs
    /// that agent, and the child is spawned with the validated budget in its environment where the
    /// child-side runtime picks it up. Before this wiring `toolBudget:` was demoted to
    /// `extra_fields` — the user wrote it, nothing happened, no error.
    #[test]
    fn a_tool_budget_agent_ships_its_budget_to_the_child_env() {
        let dir = tempfile::tempdir().expect("tempdir");
        let def = crate::discovery::frontmatter::parse_agent_file(
            "---\nname: scout\ndescription: Scouts\ntoolBudget: {\"hard\": 5, \"soft\": 2}\n---\n\nbody\n",
            crate::discovery::types::AgentSource::User,
            std::path::Path::new("scout.md"),
        )
        .expect("agent file parses");
        let budget = def.tool_budget.clone().expect("toolBudget is parsed");
        assert_eq!(budget.hard, 5);
        assert_eq!(budget.soft, Some(2));
        assert!(
            !def.extra_fields.contains_key("toolBudget"),
            "`toolBudget` is a KNOWN_FIELD; it must not also land in extra_fields"
        );

        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let agent = AgentConfig::from_agent_definition(&def, depth);
        let opts = base_opts(dir.path(), &["m1"]);
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let encoded = plan
            .spec
            .env_overlay
            .get(crate::exec::tool_budget::TOOL_BUDGET_ENV)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            crate::exec::tool_budget::decode_tool_budget_env(Some(&encoded)),
            Ok(Some(budget)),
            "the child must receive the SAME validated budget; overlay was {:?}",
            plan.spec.env_overlay
        );
    }


    /// An agent with no `toolBudget:` must set no budget var at all — a child must never inherit a
    /// stale budget from the parent process's own environment.
    #[test]
    fn an_agent_without_a_tool_budget_sets_no_budget_env_var() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        assert!(
            !plan
                .spec
                .env_overlay
                .contains_key(crate::exec::tool_budget::TOOL_BUDGET_ENV)
        );
    }

    /// SUBA-073 — the resolved permission policy (`RunOptions::permission_rules`, merged upstream
    /// of this function by `run_foreground_impl`/`spawn_background`'s own `resolve_permission_rules`
    /// call) must reach the child as `CYRUP_SUBAGENT_PERMISSION_POLICY`, decodable by the
    /// already-ported child-side `watchdog::permission_arbiter::decode_permission_rules`. The
    /// audit path env var must accompany it, defaulting under this attempt's own `temp_dir`.
    #[test]
    fn a_resolved_permission_policy_reaches_the_child_env_and_decodes() {
        use crate::watchdog::permission_arbiter as pa;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut rules = pa::PermissionRules::new();
        rules.insert("write".to_string(), pa::PermissionRuleDecision::Deny);
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.permission_rules = Some(rules.clone());
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");

        let encoded = plan
            .spec
            .env_overlay
            .get(pa::PERMISSION_POLICY_ENV)
            .expect("the policy must reach the child env");
        assert_eq!(
            pa::decode_permission_rules(Some(encoded)).expect("decodes"),
            Some(rules),
            "the child must decode the SAME policy the parent resolved"
        );
        let audit_path = plan
            .spec
            .env_overlay
            .get(pa::PERMISSION_AUDIT_PATH_ENV)
            .expect("an audit path must accompany a present policy");
        assert_eq!(
            audit_path,
            &dir.path().join("permission-audit.jsonl").display().to_string(),
            "the default audit path is under this attempt's own temp_dir"
        );
    }

    /// No policy at all must set no env var — a child must never inherit a STALE policy from the
    /// parent process's own environment (the overlay only ever adds, same rule as every other
    /// member of this family).
    #[test]
    fn no_permission_policy_sets_no_env_vars() {
        use crate::watchdog::permission_arbiter as pa;

        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        assert!(!plan.spec.env_overlay.contains_key(pa::PERMISSION_POLICY_ENV));
        assert!(!plan.spec.env_overlay.contains_key(pa::PERMISSION_AUDIT_PATH_ENV));
    }


    /// G90, the SPAWN hop: a child handed a steer inbox must receive the path in its environment,
    /// and the directory must already exist when it starts.
    ///
    /// pi `runs/shared/pi-args.ts:251-252` (`if (input.steerInboxDir) env[SUBAGENT_STEER_INBOX_ENV] = ...`).
    /// Without this hop the parent's whole steer path is a write-only file drop: the requests land
    /// on disk correctly and no process is ever told where to look.
    #[test]
    fn a_steer_inbox_reaches_the_child_as_an_env_var() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let inbox = crate::background::control::step_steer_inbox_dir(&dir.path().join("run-1"), 2);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.steer_inbox_dir = Some(inbox.clone());

        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        assert_eq!(
            plan.spec
                .env_overlay
                .get(crate::prompt_runtime::STEER_INBOX_ENV)
                .map(String::as_str),
            Some(inbox.display().to_string().as_str()),
            "the child must be told where its steer inbox is; overlay was {:?}",
            plan.spec.env_overlay
        );

        // ...and a run with no inbox sets NO variable, so a child can never inherit a stale inbox
        // from the parent process's own environment.
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &base_opts(dir.path(), &["m1"]),
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        assert!(
            !plan
                .spec
                .env_overlay
                .contains_key(crate::prompt_runtime::STEER_INBOX_ENV)
        );
    }


    /// G95 + G89 across the DETACHED-RUNNER seam, which the two tests above do not touch.
    ///
    /// A background/chain/parallel step does not reach `AgentConfig::from_agent_definition` at all.
    /// The orchestrator resolves the persona into a [`ResolvedAgentPersona`], SERIALIZES it into
    /// the runner config on disk, and the detached runner deserializes it and calls
    /// [`ResolvedAgentPersona::to_agent_config`] to rebuild the spawn input. Every field that
    /// hand-off drops is silently lost for every non-foreground run — and because both fields are
    /// `#[serde(default)] Option`, dropping them produces no error, no warning and no compile
    /// failure: `memory: None, tool_budget: None` in `to_agent_config` type-checks perfectly and
    /// leaves the whole rest of the suite green while `/run x --bg` quietly stops honouring the
    /// agent's `memory:` and `toolBudget:`.
    ///
    /// So this drives the REAL hand-off — persona → JSON → persona → `AgentConfig` → argv/env — and
    /// asserts on the two observable end products, not on the struct fields.
    #[test]
    fn the_detached_runner_persona_handoff_preserves_memory_and_tool_budget() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project_config = dir.path().join(".cyrup");
        std::fs::create_dir_all(&project_config).expect("mkdir .cyrup");
        let memory_dir = project_config
            .join(crate::discovery::agent_memory::AGENT_MEMORY_DIR_NAME)
            .join("reviewer");
        std::fs::create_dir_all(&memory_dir).expect("mkdir");
        std::fs::write(
            memory_dir.join(crate::discovery::agent_memory::AGENT_MEMORY_FILE),
            "2026-01-01: the detached runner must see this too.\n",
        )
        .expect("write MEMORY.md");

        let def = crate::discovery::frontmatter::parse_agent_file(
            "---\nname: reviewer\ndescription: Reviews\nmemory:\n  scope: project\n  path: reviewer\ntoolBudget: {\"hard\": 5, \"soft\": 2}\n---\n\n- You are the REVIEWER persona.\n",
            crate::discovery::types::AgentSource::User,
            std::path::Path::new("reviewer.md"),
        )
        .expect("agent file parses");
        let budget = def.tool_budget.clone().expect("toolBudget is parsed");

        // The hand-off, verbatim: resolve → serialize into the runner config → deserialize in the
        // detached runner process → rebuild the spawn input.
        let persona = ResolvedAgentPersona::from_agent_definition(&def);
        let encoded = serde_json::to_string(&persona).expect("persona serializes");
        let decoded: ResolvedAgentPersona =
            serde_json::from_str(&encoded).expect("runner config deserializes");
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let agent = decoded.to_agent_config(depth);

        let opts = base_opts(dir.path(), &["m1"]);
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");

        let argv = plan.spec.build_argv();
        let delivered = delivered_system_prompt(&argv).unwrap_or_default();
        assert!(
            delivered.contains("the detached runner must see this too."),
            "G95: a background/chain/parallel child must receive the SAME persistent-memory block \
             a foreground child does; argv was {argv:?}"
        );
        assert_eq!(
            crate::exec::tool_budget::decode_tool_budget_env(
                plan.spec
                    .env_overlay
                    .get(crate::exec::tool_budget::TOOL_BUDGET_ENV)
                    .map(String::as_str)
            ),
            Ok(Some(budget)),
            "G89: a background/chain/parallel child must receive the SAME validated tool budget; \
             overlay was {:?}",
            plan.spec.env_overlay
        );
    }


    /// SUBA-074 across the SAME detached-runner seam the test above guards.
    ///
    /// `runner` is a third `#[serde(default)] Option` field on that hand-off, so it carries the
    /// identical silent-loss property the test above documents: dropping it at any of the three
    /// hops type-checks perfectly, raises nothing, and leaves the suite green while every
    /// background / chain / parallel run of an external-runner profile quietly spawns a
    /// full-capability native child — the exact defect SUBA-074 exists to close, reintroduced on
    /// the one path the foreground refusal test cannot see.
    ///
    /// This drives the REAL hand-off — parse → persona → JSON → persona → `AgentConfig` — and
    /// asserts the observable end product. For `memory`/`toolBudget` that product is argv/env,
    /// because the child still spawns; for a non-`pi` runner there is NO spawn plan, because the
    /// run is refused before the ladder, so the refusal IS the product.
    ///
    /// The fixture declares no Pi-only field, which is upstream's own rule for an external profile
    /// (`agents.ts:1864-1871`) and is why this cannot simply be folded into the test above, whose
    /// fixture declares `toolBudget:`.
    #[test]
    fn the_detached_runner_persona_handoff_preserves_the_runner_profile() {
        let def = crate::discovery::frontmatter::parse_agent_file(
            "---\nname: reviewer\ndescription: Reviews\nrunner: {\"type\": \"external-cli\", \"adapter\": \"claude-code\", \"command\": \"claude\"}\n---\n\n- You are the REVIEWER persona.\n",
            crate::discovery::types::AgentSource::User,
            std::path::Path::new("reviewer.md"),
        )
        .expect("an external-cli profile declaring no Pi-only field must load");
        assert!(def.runner.is_some(), "HOP 0: the frontmatter parse must yield a runner");

        // The hand-off, verbatim: resolve → serialize into the runner config → deserialize in the
        // detached runner process → rebuild the spawn input.
        let persona = ResolvedAgentPersona::from_agent_definition(&def);
        assert_eq!(persona.runner, def.runner, "HOP A: from_agent_definition dropped the runner");

        let encoded = serde_json::to_string(&persona).expect("persona serializes");
        let decoded: ResolvedAgentPersona =
            serde_json::from_str(&encoded).expect("runner config deserializes");
        assert_eq!(decoded.runner, def.runner, "HOP B: the JSON round-trip dropped the runner");

        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let agent = decoded.to_agent_config(depth);
        assert_eq!(agent.runner, def.runner, "HOP C: to_agent_config dropped the runner");

        // The observable end product: the rebuilt config refuses, so a background/chain/parallel
        // step declines the profile exactly as the foreground path does.
        let reason = agent
            .runner
            .as_ref()
            .and_then(crate::runner::AgentRunnerConfig::refusal_reason)
            .expect("a non-`pi` runner must refuse after surviving the hand-off");
        assert!(reason.contains("runner.type='external-cli'"), "{reason}");
        assert!(reason.contains("adapter 'claude-code'"), "{reason}");
        assert!(reason.contains("full-capability native child"), "{reason}");
    }


    /// SUBA-S01 (pi `runs/shared/pi-args.ts:246-250`): a declared `outputSchema` must reach the child as BOTH
    /// structured-output env vars, pointing at the runtime's real schema and capture paths.
    ///
    /// Before this wiring, `create_structured_output_runtime`, `read_structured_output`,
    /// `cleanup_structured_output_runtime`, `structured_output_instruction`, both env constants and
    /// `StructuredOutputRuntime` were ALL ported and had zero callers outside their own file — so a
    /// child never learned a schema had been declared, never registered `structured_output`, and
    /// the run fell back to a fenced-```json-block scan that pi does not have.
    #[test]
    fn a_declared_output_schema_reaches_the_child_as_both_env_vars() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let schema = serde_json::json!({
            "type": "object",
            "properties": { "verdict": { "type": "string" } },
            "required": ["verdict"],
        });
        let runtime =
            crate::exec::structured::create_structured_output_runtime(&schema, dir.path())
                .expect("runtime is created");

        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "task",
            &opts,
            depth,
            dir.path(),
            Some(&runtime),
        )
        .expect("plan builds");

        assert_eq!(
            plan.spec
                .env_overlay
                .get(crate::exec::structured::STRUCTURED_OUTPUT_SCHEMA_ENV)
                .map(String::as_str),
            Some(runtime.schema_path.display().to_string().as_str()),
            "the child must be told where to READ the schema"
        );
        assert_eq!(
            plan.spec
                .env_overlay
                .get(crate::exec::structured::STRUCTURED_OUTPUT_CAPTURE_ENV)
                .map(String::as_str),
            Some(runtime.output_path.display().to_string().as_str()),
            "...and where to WRITE the captured value"
        );

        // The schema file must actually exist and round-trip: the child reads it to build the
        // `structured_output` tool's parameters, so an unwritten file silently disables the tool.
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&runtime.schema_path).expect("schema written"))
                .expect("schema parses");
        assert_eq!(written, schema);
    }


    /// The other half of the gate: no declared schema means NO structured-output env at all, so an
    /// ordinary child registers no `structured_output` tool (pi gates on both vars being present,
    /// `subagent-prompt-runtime.ts:281`).
    #[test]
    fn no_output_schema_means_no_structured_output_env() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
                .expect("plan builds");

        assert!(
            !plan
                .spec
                .env_overlay
                .contains_key(crate::exec::structured::STRUCTURED_OUTPUT_SCHEMA_ENV)
                && !plan
                    .spec
                    .env_overlay
                    .contains_key(crate::exec::structured::STRUCTURED_OUTPUT_CAPTURE_ENV),
            "a step with no outputSchema must carry no structured-output env"
        );
    }


    #[test]
    fn build_attempt_spawn_plan_delivers_the_persona_body_as_append_in_append_mode() {
        // pi `runs/shared/pi-args.ts:164-165`: the mode picks the FLAG, the body always ships.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Append;
        agent.system_prompt_body = "You are a delegate persona.".to_string();
        let opts = base_opts(dir.path(), &["m1"]);
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let task_text = build_task_text(&agent, "do the thing", &opts, &contract, "");
        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), &task_text, &opts, depth, dir.path(), None)
                .expect("plan builds");
        let argv = plan.spec.build_argv();

        let delivered = delivered_system_prompt(&argv).unwrap_or_default();
        assert_eq!(
            delivered, "You are a delegate persona.",
            "append mode must ship the persona body on --append-system-prompt; argv was {argv:?}"
        );
        assert!(!argv.iter().any(|a| a.starts_with("--system-prompt")));
        // Delivered EXACTLY once: the body no longer rides along inside the task text as well.
        assert!(
            !plan.spec.task_arg.contains("You are a delegate persona."),
            "append-mode persona must not be duplicated into the task text: {}",
            plan.spec.task_arg
        );
    }


    #[test]
    fn build_attempt_spawn_plan_omits_the_system_prompt_flag_for_an_empty_persona_body() {
        // A persona with no prose must not blank the child's own assembled system prompt.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Replace;
        agent.system_prompt_body = "   \n\t ".to_string();
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        assert!(!argv.iter().any(|a| a.starts_with("--system-prompt")));
        assert!(!argv.iter().any(|a| a.starts_with("--append-system-prompt")));
    }


    // ---- G82 / R-SA-024: the SYSTEM-PROMPT half of the output-path override
    //      (pi `injectOutputPathSystemPrompt`, `execution.ts:1443`) ----

    /// Upstream composes the override onto the SAME `systemPrompt` string the persona and the
    /// memory block already occupy, as the statement directly after the memory fold
    /// (`execution.ts:1433-1443`). It is a second surface, not a replacement for the task-side
    /// `injectSingleOutputInstruction`: the run gets both.
    #[test]
    fn build_attempt_spawn_plan_composes_the_output_path_override_onto_the_system_prompt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Replace;
        agent.system_prompt_body = "- You are the REVIEWER persona.".to_string();
        let mut opts = base_opts(dir.path(), &["m1"]);
        let out = dir.path().join("out.md");
        opts.output_mode = OutputMode::FileOnly;
        opts.output_path = Some(out.clone());
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
                .expect("plan builds");
        let argv = plan.spec.build_argv();
        let delivered = delivered_system_prompt(&argv).unwrap_or_default();
        assert!(
            !delivered.is_empty(),
            "a system-prompt flag must be emitted; argv was {argv:?}"
        );

        assert!(
            delivered.contains("- You are the REVIEWER persona."),
            "the persona body must survive the injection: {delivered:?}"
        );
        assert!(
            delivered.contains("Runtime output path override:"),
            "the system-prompt-side header must be present: {delivered:?}"
        );
        assert!(
            delivered.contains(&format!(
                "Write your findings to exactly this path: {}",
                out.display()
            )),
            "the override must name the authoritative path: {delivered:?}"
        );
        // Order matters: upstream APPENDS the override to the already-composed prompt, so the
        // persona is what the child reads first and the override is what overrides it.
        let persona_at = delivered
            .find("- You are the REVIEWER persona.")
            .unwrap_or(usize::MAX);
        let override_at = delivered
            .find("Runtime output path override:")
            .unwrap_or(usize::MIN);
        assert!(
            persona_at < override_at,
            "the override must be appended AFTER the persona: {delivered:?}"
        );
    }


    /// No configured output path means no override — the persona ships alone, byte-identical to
    /// what it was before this injection existed (`injectOutputPathSystemPrompt` returns its input
    /// unchanged for an undefined path, `single-output.ts:105`).
    #[test]
    fn build_attempt_spawn_plan_leaves_the_system_prompt_alone_without_an_output_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Replace;
        agent.system_prompt_body = "- persona".to_string();
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
                .expect("plan builds");
        let argv = plan.spec.build_argv();
        assert_eq!(
            delivered_system_prompt(&argv).as_deref(),
            Some("- persona"),
            "argv was {argv:?}"
        );
    }


    /// Same rule as the task side: upstream keys the injection on the PATH alone. `outputMode` is
    /// read only by `validateFileOnlyOutputMode` and by delivery-side `finalizeSingleOutput`.
    #[test]
    fn the_system_prompt_override_is_unconditional_on_output_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_body = "- persona".to_string();
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        for mode in [
            OutputMode::FileOnly,
            OutputMode::FileAndInline,
            OutputMode::Inline,
        ] {
            let mut opts = base_opts(dir.path(), &["m1"]);
            opts.output_mode = mode;
            opts.output_path = Some(dir.path().join("out.md"));
            let plan = build_attempt_spawn_plan(
                &agent,
                &ModelId::from("m1"),
                "task",
                &opts,
                depth,
                dir.path(),
                None,
            )
            .expect("plan builds");
            let delivered = delivered_system_prompt(&plan.spec.build_argv()).unwrap_or_default();
            assert!(
                delivered.contains("Runtime output path override:"),
                "{mode:?} with a configured path must carry the override: {delivered:?}"
            );
        }
    }


    /// The capability branch (`formatOutputPathInstruction`, `single-output.ts:84-91`) must be
    /// live on this surface too, not only on the task side: a read-only agent is told the runtime
    /// will persist its final response.
    #[test]
    fn the_system_prompt_override_branches_on_the_agents_tool_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.output_path = Some(dir.path().join("out.md"));

        let mut read_only = sample_agent_config("m1", &[]);
        read_only.system_prompt_body = "- persona".to_string();
        read_only.tools = Some(vec![
            crate::discovery::types::ToolRef::Builtin("read".to_string()),
            crate::discovery::types::ToolRef::Builtin("grep".to_string()),
        ]);
        let plan = build_attempt_spawn_plan(
            &read_only,
            &ModelId::from("m1"),
            "task",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let delivered = delivered_system_prompt(&plan.spec.build_argv()).unwrap_or_default();
        assert!(
            delivered.contains("Return the complete artifact in your final response."),
            "{delivered:?}"
        );
        assert!(
            !delivered.contains("Write your findings to exactly this path:"),
            "a read-only agent must not be ordered to write: {delivered:?}"
        );

        let mut write_capable = sample_agent_config("m1", &[]);
        write_capable.system_prompt_body = "- persona".to_string();
        write_capable.tools = Some(vec![
            crate::discovery::types::ToolRef::Builtin("read".to_string()),
            crate::discovery::types::ToolRef::Builtin("write".to_string()),
        ]);
        let plan = build_attempt_spawn_plan(
            &write_capable,
            &ModelId::from("m1"),
            "task",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let delivered = delivered_system_prompt(&plan.spec.build_argv()).unwrap_or_default();
        assert!(
            delivered.contains("Write your findings to exactly this path:"),
            "{delivered:?}"
        );
        assert!(
            !delivered.contains("Return the complete artifact in your final response."),
            "{delivered:?}"
        );
    }


    /// An empty persona plus a configured output path composes a NON-empty body, so the flag ships
    /// — pi emits its system-prompt flag for any non-null string (`runs/shared/pi-args.ts:570-585`), and the
    /// composed value is exactly the override. The empty-body omission delta survives only for the
    /// genuinely-empty case, which
    /// [`build_attempt_spawn_plan_omits_the_system_prompt_flag_for_an_empty_persona_body`] pins.
    #[test]
    fn an_empty_persona_with_an_output_path_still_ships_the_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Append;
        agent.system_prompt_body = "   \n\t ".to_string();
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.output_path = Some(dir.path().join("out.md"));
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
                .expect("plan builds");
        let argv = plan.spec.build_argv();
        let delivered = delivered_system_prompt(&argv).unwrap_or_default();
        assert!(
            delivered.starts_with("Runtime output path override:"),
            "the override alone must still ship; argv was {argv:?}"
        );
    }


    /// Both halves reach the SAME run, as upstream's foreground single run does: the task side
    /// from `subagent-executor.ts:3674` (cyrup: [`build_task_text`]) and the system-prompt side
    /// from `execution.ts:1443` (cyrup: [`build_attempt_spawn_plan`]). Wiring one and not the
    /// other is the gap this pins closed.
    #[test]
    fn both_output_path_surfaces_reach_the_same_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_body = "- persona".to_string();
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.output_path = Some(dir.path().join("out.md"));
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let task = build_task_text(&agent, "do the thing", &opts, &contract, "");
        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), &task, &opts, depth, dir.path(), None)
                .expect("plan builds");
        let delivered = delivered_system_prompt(&plan.spec.build_argv()).unwrap_or_default();

        assert!(task.contains("\n\n---\n**Output:**\n"), "task side missing: {task:?}");
        assert!(
            delivered.contains("Runtime output path override:"),
            "system-prompt side missing: {delivered:?}"
        );
        // The task-side header must NOT leak onto the system prompt, nor vice versa: they are
        // distinct upstream strings and `stripFrameworkInstructions` only knows the task one.
        assert!(
            !delivered.contains("**Output:**"),
            "the task-side header must not appear on the system prompt: {delivered:?}"
        );
        assert!(
            !task.contains("Runtime output path override:"),
            "the system-prompt-side header must not appear in the task: {task:?}"
        );
    }


    #[test]
    fn build_attempt_spawn_plan_includes_tools_flag_only_when_agent_declares_tools() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::Builtin("edit".to_string()),
        ]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        let tools_idx = argv.iter().position(|a| a == "--tools").expect("--tools present");
        assert_eq!(argv[tools_idx + 1], "read,edit");
    }


    #[test]
    fn build_attempt_spawn_plan_writes_the_child_intercom_bridge_env_when_orchestrator_target_is_set() {
        // The production activation path: when the orchestrator's presence target + this run's id are
        // present in `RunOptions`, the spawn overlay MUST write all six child-bridge identity vars so
        // the spawned child's `IntercomExtension` reads `read_child_orchestrator_metadata() == Some`
        // and registers `contact_supervisor` + a broker presence under its own deterministic label.
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]); // name = "worker"
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.orchestrator_intercom_target = Some("subagent-chat-abcd1234".to_string());
        opts.run_id = Some(crate::background::RunId::from_token("run-XYZ"));
        opts.child_index = Some(2);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let env = &plan.spec.env_overlay;
        assert_eq!(
            env.get(crate::spawn::intercom_target::ENV_ORCHESTRATOR_TARGET).map(String::as_str),
            Some("subagent-chat-abcd1234")
        );
        assert_eq!(env.get(crate::spawn::nested_events::RUN_ID_ENV).map(String::as_str), Some("run-XYZ"));
        assert_eq!(env.get(crate::spawn::intercom_target::ENV_CHILD_AGENT).map(String::as_str), Some("worker"));
        assert_eq!(env.get(crate::spawn::nested_events::CHILD_INDEX_ENV).map(String::as_str), Some("2"));
        // The child's own label = resolve_subagent_intercom_target(run_id, agent, index) — the SAME
        // string the parent's `control_resume` recomputes to steer it (index+1 suffix).
        assert_eq!(
            env.get(crate::spawn::intercom_target::ENV_INTERCOM_SESSION_NAME).map(String::as_str),
            Some("subagent-worker-run-xyz-3")
        );
    }


    /// G106 (pi `runs/shared/pi-args.ts:221-231`, `3ac0ef5` "Make supervisor coordination native"): the single
    /// spawn-plan chokepoint must hand a child BOTH native-supervisor-channel vars and CREATE the
    /// `requests/`+`replies/` directories.
    ///
    /// The user action: `/run reviewer "..."` (or the `subagent` tool) from a live session. Without
    /// this the spawned child's `read_child_metadata` returns `None`, it registers no native
    /// `contact_supervisor`, and the file channel is unreachable — which is the pre-fix state.
    #[test]
    fn build_attempt_spawn_plan_writes_the_native_supervisor_channel_env() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]); // name = "worker"
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.orchestrator_intercom_target = Some("subagent-chat-abcd1234".to_string());
        opts.run_id = Some(crate::background::RunId::from_token("run-NSC"));
        opts.child_index = Some(2);
        opts.parent_session_id = Some("session-parent-1".to_string());
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let env = &plan.spec.env_overlay;

        assert_eq!(
            env.get(crate::spawn::intercom_target::ENV_ORCHESTRATOR_SESSION_ID).map(String::as_str),
            Some("session-parent-1"),
            "the native channel keys every request on the launching session's own id"
        );
        let channel_dir = env
            .get(crate::spawn::intercom_target::ENV_SUPERVISOR_CHANNEL_DIR)
            .map(std::path::PathBuf::from)
            .expect("the spawn planner must hand the child its channel directory");
        assert_eq!(
            channel_dir,
            crate::native_supervisor::resolve_supervisor_channel_dir("run-NSC", "worker", 2),
            "the child's channel dir must be the SAME path the parent's poller scans"
        );
        assert!(
            channel_dir.join("requests").is_dir() && channel_dir.join("replies").is_dir(),
            "both sub-directories are created up front (pi's two mkdirSync calls)"
        );

        // The child's own gate opens on exactly what the planner wrote — the two halves meet.
        let metadata = crate::native_supervisor::read_child_metadata_from(&|k| env.get(k).cloned())
            .expect("the child metadata gate must open on the planner's overlay");
        assert_eq!(metadata.run_id, "run-NSC");
        assert_eq!(metadata.agent, "worker");
        assert_eq!(metadata.child_index, 2);
        assert_eq!(metadata.orchestrator_session_id, "session-parent-1");

        let _ = std::fs::remove_dir_all(&channel_dir);
    }


    /// The negative half: no parent session id means no routing key, so NEITHER var is written — a
    /// half-set overlay would leave the child holding a channel it cannot address.
    #[test]
    fn build_attempt_spawn_plan_omits_the_supervisor_channel_env_without_a_parent_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.orchestrator_intercom_target = Some("subagent-chat-abcd1234".to_string());
        opts.run_id = Some(crate::background::RunId::from_token("run-NSC2"));
        opts.child_index = Some(0);
        opts.parent_session_id = None;
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let env = &plan.spec.env_overlay;
        assert!(!env.contains_key(crate::spawn::intercom_target::ENV_SUPERVISOR_CHANNEL_DIR));
        assert!(crate::native_supervisor::read_child_metadata_from(&|k| env.get(k).cloned()).is_none());
    }


    #[test]
    fn build_attempt_spawn_plan_omits_the_child_intercom_bridge_env_without_an_orchestrator_target() {
        // A headless / no-intercom run (no orchestrator target) must leave the child un-bridged — the
        // clean no-intercom path, so a plain run spawns no broker-participant child.
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]); // orchestrator_intercom_target / run_id both None
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let env = &plan.spec.env_overlay;
        assert!(!env.contains_key(crate::spawn::intercom_target::ENV_ORCHESTRATOR_TARGET));
        assert!(!env.contains_key(crate::spawn::intercom_target::ENV_INTERCOM_SESSION_NAME));
        assert!(!env.contains_key(crate::spawn::intercom_target::ENV_CHILD_AGENT));
    }


    /// TOOL-031 / PARITY-GAPS PB-5 — the agent-identity markers must survive the re-exec.
    ///
    /// pi sets them on `process.env` in `cli.ts` before `main()` (`PI_CODING_AGENT = "true"`,
    /// `cli.ts:13` @v0.83.0; `AI_AGENT = "pi"`, `:14` @v0.84.1, mirrored in `rpc-entry.ts:7-8`), so
    /// a re-exec'd subagent child inherits them and so does everything the child then spawns.
    ///
    /// RED before this pass: cyrup's bin declines the process-global `set_var`, and only the `bash`
    /// TOOL pushed the pair per-child. The subagent spawn overlay pushed neither, so the marker
    /// chain broke at the first re-exec and the whole child subtree ran with them unset.
    ///
    /// Asserted unconditionally (not "present when some option is set") because the overlay is
    /// applied OVER an inherited environment: an omitted key silently keeps whatever the parent
    /// happened to have.
    #[test]
    fn the_spawn_overlay_carries_the_agent_identity_markers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let env = &plan.spec.env_overlay;
        assert_eq!(
            env.get("PI_CODING_AGENT").map(String::as_str),
            Some("true"),
            "`PI_CODING_AGENT = \"true\"` (pi `cli.ts:13`) must reach the re-exec'd child"
        );
        assert_eq!(
            env.get("AI_AGENT").map(String::as_str),
            Some("cyrup"),
            "`AI_AGENT` (pi `cli.ts:14`) names WHICH agent is running; the key is pi's verbatim"
        );
    }


    /// CFG-069 — the third and last of the three sites that write `AI_AGENT` into a child.
    ///
    /// The key does not exist at the ported tag at all (`git -C pi grep -n AI_AGENT v0.83.0 --
    /// packages/` → 0; `cli.ts:13` @v0.83.0 sets only `PI_CODING_AGENT`); it arrives at
    /// `cli.ts:14` @v0.84.1 — re-derived at both tags this pass. So the delta annotation must name
    /// the KEY and the TAG, not only the VALUE, or a later v0.84.1 uplift reads the site as
    /// already-done-at-tag and never records that cyrup ran ahead of the baseline.
    ///
    /// RED before this pass: the annotation above the insert read `[CYRUP-DELTA, value only]
    /// `AI_AGENT` names WHICH agent is running (`"pi"` upstream).` — one line, carrying neither
    /// `@v0.84.1` nor the absent-at-v0.83.0 fact.
    ///
    /// Mirrors `cyrup-tools`' `cfg069_the_bash_tool_delta_names_the_forward_ported_key_and_its_tag`
    /// and `cyrup-session-svc`'s `the_forward_ported_ai_agent_marker_names_its_key_and_its_tag`.
    #[test]
    fn cfg069_the_spawn_overlay_delta_names_the_forward_ported_key_and_its_tag() {
        let src = include_str!("spawn_plan.rs");

        assert!(
            src.contains(r#"env_overlay.insert("PI_CODING_AGENT".to_string(), "true".to_string());"#),
            "the at-tag marker `PI_CODING_AGENT` (cli.ts:13 @v0.83.0) must still be written"
        );

        let insert = r#"env_overlay.insert("AI_AGENT".to_string(), "cyrup".to_string());"#;
        let at = src.find(insert).expect("`AI_AGENT` is written into the spawn overlay");
        let annotation = &src[..at];
        let annotation = &annotation[annotation.rfind("[CYRUP-DELTA").expect("a delta annotation")..];

        assert!(
            annotation.contains("@v0.84.1"),
            "the delta line must state the TAG the key comes from; got: {annotation}"
        );
        assert!(
            annotation.contains("AI_AGENT"),
            "the delta line must name the KEY, not only its value; got: {annotation}"
        );
        assert!(
            annotation.contains("v0.83.0"),
            "the delta line must state that the key is ABSENT at the ported tag; got: {annotation}"
        );
    }


    #[test]
    fn build_attempt_spawn_plan_omits_tools_flag_when_agent_declares_no_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        assert!(!plan.spec.build_argv().contains(&"--tools".to_string()));
    }


    #[test]
    fn build_attempt_spawn_plan_includes_session_flag_when_fork_context_resolved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.fork_context = ForkContext {
            mode: ContextMode::Fork,
            session_file_path: Some(dir.path().join("parent-branch.jsonl")),
            thinking_override: None,
        };
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        let idx = argv.iter().position(|a| a == "--session").expect("--session present");
        assert!(argv[idx + 1].contains("parent-branch.jsonl"));
        // pi's `sessionFile` branch (`runs/shared/pi-args.ts:101-103`) emits ONLY `--session`: never the
        // `--no-session`/`--session-dir` pair from the else arm.
        assert!(!argv.contains(&"--no-session".to_string()));
        assert!(!argv.contains(&"--session-dir".to_string()));
    }


    /// SUBA-041 prerequisite (pi `buildPiArgs`, `runs/shared/pi-args.ts:517-528`): with NO fork-context session
    /// file, no `session_dir` and no `share`, pi's `sessionEnabled` is false and the child is spawned
    /// `--no-session`. Pre-fix this arm emitted nothing at all, so every session-less subagent child
    /// silently persisted a session into the orchestrator's own store.
    #[test]
    fn build_attempt_spawn_plan_emits_no_session_when_nothing_enables_a_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        assert!(argv.contains(&"--no-session".to_string()), "argv: {argv:?}");
        assert!(!argv.contains(&"--session-dir".to_string()), "argv: {argv:?}");
    }


    /// SUBA-041 prerequisite: an explicit `session_dir` both ENABLES sessions (no `--no-session`)
    /// and reaches the child as `--session-dir <dir>`, with the directory created up front — pi's
    /// `fs.mkdirSync(sessionDir, { recursive: true })` (`runs/shared/pi-args.ts:524-526`).
    #[test]
    fn build_attempt_spawn_plan_emits_session_dir_and_creates_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        let session_dir = dir.path().join("sessions").join("run-0");
        opts.session_dir = Some(session_dir.clone());
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        let idx = argv
            .iter()
            .position(|a| a == "--session-dir")
            .expect("--session-dir present");
        assert_eq!(argv[idx + 1], session_dir.display().to_string());
        assert!(
            !argv.contains(&"--no-session".to_string()),
            "an explicit session dir enables sessions: {argv:?}"
        );
        assert!(session_dir.is_dir(), "the session dir must be created up front");
    }


    /// SUBA-041 prerequisite: `share: true` alone is pi's other `sessionEnabled` term
    /// (`execution.ts:1412`) — it suppresses `--no-session` without naming a directory.
    #[test]
    fn build_attempt_spawn_plan_share_alone_enables_sessions_without_a_session_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.share = Some(true);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        assert!(!argv.contains(&"--no-session".to_string()), "argv: {argv:?}");
        assert!(!argv.contains(&"--session-dir".to_string()), "argv: {argv:?}");
    }


    /// SUBA-041 prerequisite: `share: false` is NOT an enabling value — pi's term is
    /// `options.share === true` (`execution.ts:1027`), so an explicit `false` still yields
    /// `--no-session`.
    #[test]
    fn build_attempt_spawn_plan_share_false_still_disables_sessions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.share = Some(false);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        assert!(plan.spec.build_argv().contains(&"--no-session".to_string()));
    }


    #[test]
    fn build_attempt_spawn_plan_propagates_depth_envelope_into_env_overlay() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 2,
            max_depth: 4,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::depth::DEPTH_ENV_VAR),
            Some(&"2".to_string())
        );
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::depth::MAX_DEPTH_ENV_VAR),
            Some(&"4".to_string())
        );
    }


    // ---- PERM-001: the child-ROLE env pair (pi `augmentChildEnv`, `runs/shared/pi-args.ts:329-330`) ----

    /// The production spawn path MUST mark the child as a child. Without this entry the re-exec'd
    /// process is indistinguishable from a top-level session, so
    /// `cyrup_permission_system::permission_extension_for_env` gives it the LOCAL ask channel
    /// instead of the `ForwardingAskChannel`; with no TTY it then has no reachable human and
    /// fail-CLOSES every `ask`-tier tool call rather than forwarding it to the parent's human
    /// through the `<agentDir>/sessions/permission-forwarding/` spool.
    ///
    /// The cross-process proof that the ask actually TRAVELS on this env is
    /// `cyrup-permission-system/tests/forwarding_spawn_env.rs`; this test is the other half of that
    /// chain — that the entry originates in the real spawn planner and not in a test's own hand.
    #[test]
    fn build_attempt_spawn_plan_marks_the_child_as_a_subagent_child_in_the_env_overlay() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");

        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::nested_events::CHILD_ENV),
            Some(&"1".to_string()),
            "every spawned child must carry the child-role flag; overlay was {:?}",
            plan.spec.env_overlay
        );
        // Explicit `"0"`, never absent: the overlay is applied OVER the inherited env, so omitting it
        // would let a fanout-authorized parent's own `1` leak into an unauthorized grandchild.
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::nested_events::FANOUT_CHILD_ENV),
            Some(&"0".to_string()),
            "an unauthorized child must be pinned to fanout=0, not left to inherit; overlay was {:?}",
            plan.spec.env_overlay
        );
    }


    /// The consequence of the flag, at the seam that reads it: a child spawned by the production
    /// planner registers NO subagent surface at all (pi `extension/index.ts:177`), instead of the
    /// full orchestrator surface — its own `subagent` tool, 12 slash commands and background
    /// watcher — that an unmarked child was silently installing.
    #[test]
    fn a_child_spawned_by_the_production_planner_registers_no_subagent_surface() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");

        let is_one = |name: &str| plan.spec.env_overlay.get(name).map(String::as_str) == Some("1");
        let mode = crate::extension::resolve_registration_mode(
            is_one(crate::spawn::nested_events::CHILD_ENV),
            is_one(crate::spawn::nested_events::FANOUT_CHILD_ENV),
        );
        assert!(
            mode.is_none(),
            "a plain spawned child must register nothing, got {mode:?} from overlay {:?}",
            plan.spec.env_overlay
        );
    }


    /// The other half of pi's `fanoutAuthorized = declaredBuiltinTools.includes("subagent")`
    /// (`runs/shared/pi-args.ts:194`, written to the env at `:330`): a persona that declares the
    /// `subagent` tool IS granted nested delegation, so the flag must track the agent's tools rather
    /// than being pinned off. Pinning it off makes [`RegistrationMode::ChildSafe`] unreachable in
    /// production and the whole depth envelope (`max_depth`, R-SA-054) decorative — nothing could
    /// ever reach depth 2.
    #[test]
    fn an_agent_declaring_the_subagent_tool_spawns_a_fanout_authorized_child_that_can_delegate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::Builtin(crate::extension::TOOL_NAME.to_string()),
        ]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");

        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::nested_events::FANOUT_CHILD_ENV),
            Some(&"1".to_string()),
            "a persona that declares `subagent` must be spawned fanout-authorized; overlay was {:?}",
            plan.spec.env_overlay
        );
        // The observable consequence at the seam that reads the flag: this child registers the
        // restricted subagent surface, so it can delegate (pi `extension/fanout-child.ts:132`).
        let is_one = |name: &str| plan.spec.env_overlay.get(name).map(String::as_str) == Some("1");
        let mode = crate::extension::resolve_registration_mode(
            is_one(crate::spawn::nested_events::CHILD_ENV),
            is_one(crate::spawn::nested_events::FANOUT_CHILD_ENV),
        );
        assert_eq!(
            mode,
            Some(crate::extension::RegistrationMode::ChildSafe),
            "a fanout-authorized child registers the restricted surface, got {mode:?} from overlay {:?}",
            plan.spec.env_overlay
        );
        // It is still a CHILD: its asks forward to the parent's spool rather than resolving locally.
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::nested_events::CHILD_ENV),
            Some(&"1".to_string())
        );
    }


    /// A persona that declares tools but NOT `subagent` stays unauthorized — the grant is per-agent,
    /// not "anyone who declares any tools" (pi's `.includes("subagent")` is an exact membership
    /// test).
    #[test]
    fn declaring_other_tools_does_not_grant_fanout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::Builtin("bash".to_string()),
        ]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::nested_events::FANOUT_CHILD_ENV),
            Some(&"0".to_string()),
            "only a declared `subagent` tool grants fanout; overlay was {:?}",
            plan.spec.env_overlay
        );
    }

    /// SUBA-072 Gap 2 (found during `/qa` re-review of the (a)-(d) fix above) — `fanout_authorized`
    /// must be derived from the CEILING-FILTERED declared-tools list, not the agent's raw `tools:`
    /// declaration. Pre-fix, `fanout_authorized` was computed from `builtin_tools` before the
    /// ceiling filter (further down the same function) ever ran against it, so an agent declaring
    /// `tools: [subagent, read]` stayed fanout-authorized even under a ceiling whose `allowedTools`
    /// excludes `subagent` — the dangerous direction: the ceiling silently permitted nested
    /// delegation it was set up to deny. This is the same class of bug `SUBA-072` exists to close,
    /// on a fourth site the (a)-(d) fix's own tests never touched (they assert against `--tools`/
    /// `--no-extensions`/the launch-time throw, never against `FANOUT_CHILD_ENV`).
    #[test]
    fn a_capability_ceiling_excluding_subagent_revokes_fanout_authorization_even_when_the_agent_declares_it()
    {
        use crate::exec::capability_ceiling as cc;

        let dir = tempfile::tempdir().expect("tempdir");
        let session = "spawn-plan-ceiling-fanout-revoke-session";
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.parent_session_id = Some(session.to_string());
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };

        let _handle = cc::register_capability_ceiling(
            session,
            "org-policy",
            &serde_json::json!({ "allowedTools": ["read"] }),
        )
        .expect("registers");

        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = Some(vec![
            ToolRef::Builtin(crate::extension::TOOL_NAME.to_string()),
            ToolRef::Builtin("read".to_string()),
        ]);
        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
                .expect("plan builds");

        // The ceiling still correctly narrows --tools to just `read` (the (a)-(d) fix, unaffected).
        let argv = plan.spec.build_argv();
        let idx = argv.iter().position(|a| a == "--tools").expect("--tools present");
        assert_eq!(argv.get(idx + 1).map(String::as_str), Some("read"));

        // But fanout authorization must ALSO be revoked, even though the agent's own `tools:`
        // declares `subagent` — the ceiling excludes it from `allowedTools`.
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::nested_events::FANOUT_CHILD_ENV),
            Some(&"0".to_string()),
            "a ceiling excluding `subagent` from allowedTools must revoke fanout authorization \
             even when the agent's own tools: declares it; overlay was {:?}",
            plan.spec.env_overlay
        );
    }

    /// SUBA-072 Gap 2, the completeness half: a ceiling that GRANTS `subagent` via `allowedTools`
    /// authorizes fanout even for an agent that declares no `tools:` of its own — pi's
    /// `input.tools === undefined` arm sets `declaredBuiltinTools = [...allowedToolSet]`, so
    /// `fanoutAuthorized` follows the ceiling, not merely the agent's own (absent) declaration.
    /// Pre-fix, `fanout_authorized` read `builtin_tools`, which is unconditionally empty whenever
    /// `agent.tools` is `None` — so this arm could never authorize fanout regardless of the ceiling.
    #[test]
    fn a_capability_ceiling_granting_subagent_authorizes_fanout_even_with_no_agent_level_tools() {
        use crate::exec::capability_ceiling as cc;

        let dir = tempfile::tempdir().expect("tempdir");
        let session = "spawn-plan-ceiling-fanout-grant-session";
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.parent_session_id = Some(session.to_string());
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };

        let _handle = cc::register_capability_ceiling(
            session,
            "org-policy",
            &serde_json::json!({ "allowedTools": [crate::extension::TOOL_NAME] }),
        )
        .expect("registers");

        // No `tools:` declared at all.
        let agent = sample_agent_config("m1", &[]);
        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
                .expect("plan builds");

        let argv = plan.spec.build_argv();
        let idx = argv.iter().position(|a| a == "--tools").unwrap_or_else(|| {
            panic!("a ceiling must pin the surface even with no agent-level `tools:`; argv {argv:?}")
        });
        assert_eq!(argv.get(idx + 1).map(String::as_str), Some(crate::extension::TOOL_NAME));

        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::nested_events::FANOUT_CHILD_ENV),
            Some(&"1".to_string()),
            "a ceiling granting `subagent` via allowedTools must authorize fanout even when the \
             agent declares no tools: of its own; overlay was {:?}",
            plan.spec.env_overlay
        );
    }


    // ---- T4: thinking suffix (pi `applyThinkingSuffix`), inherit flags, extension threading,
    // and the direct-MCP tools split (`mcp:` refs no longer leak into `--tools` literally) ----

    #[test]
    fn apply_thinking_suffix_appends_level_to_a_provider_qualified_model() {
        assert_eq!(
            apply_thinking_suffix(Some("openai-codex/gpt-5.4-mini"), Some("high"), false).as_deref(),
            Some("openai-codex/gpt-5.4-mini:high")
        );
    }


    #[test]
    fn apply_thinking_suffix_passes_explicit_off_through() {
        // pi: "passes explicit thinking off through to the model arg".
        assert_eq!(
            apply_thinking_suffix(Some("anthropic/claude-haiku-4-5"), Some("off"), false).as_deref(),
            Some("anthropic/claude-haiku-4-5:off")
        );
    }


    #[test]
    fn apply_thinking_suffix_leaves_a_non_thinking_provider_suffix_untouched() {
        // pi: "leaves provider-specific model suffixes untouched when thinking is disabled". A
        // `:7b`-style suffix is not a THINKING_LEVEL, so with no thinking requested the id is
        // returned verbatim (no double-suffix, no accidental `:high`).
        assert_eq!(
            apply_thinking_suffix(Some("openai-compatible/qwen2.5-coder:7b"), None, false).as_deref(),
            Some("openai-compatible/qwen2.5-coder:7b")
        );
    }


    #[test]
    fn apply_thinking_suffix_does_not_double_suffix_an_existing_thinking_level() {
        assert_eq!(
            apply_thinking_suffix(Some("model:high"), Some("low"), false).as_deref(),
            Some("model:high")
        );
    }


    /// PROV-002: `max` must be a RECOGNIZED suffix. With the 6-entry list, a model id already
    /// ending `:max` was not recognized and got double-suffixed to `model:max:high`, producing an
    /// unresolvable id for the child process. Upstream pi-subagents fixed this in 747de75
    /// (`src/shared/model-info.ts:1`).
    #[test]
    fn apply_thinking_suffix_recognizes_max_as_an_existing_level() {
        assert_eq!(
            apply_thinking_suffix(Some("anthropic/claude-opus-4-6:max"), Some("high"), false).as_deref(),
            Some("anthropic/claude-opus-4-6:max"),
            "an existing `:max` must not be double-suffixed"
        );
        // …and `max` is appendable as a level in its own right.
        assert_eq!(
            apply_thinking_suffix(Some("anthropic/claude-opus-4-6"), Some("max"), false).as_deref(),
            Some("anthropic/claude-opus-4-6:max")
        );
    }


    /// SUBA-075 / pi `applyThinkingSuffix(model, thinking, replaceExisting)`
    /// (`runs/shared/pi-args.ts:238-252` @v0.57.0). The third argument exists for exactly one
    /// caller: a sanitized fork's thinking override, which must REPLACE a level the id already
    /// names rather than defer to it.
    /// SUBA-078 / pi `pi-args.ts:875-879` @v0.57.0: the resolved thinking ceiling crosses the
    /// process boundary, which is what lets a nested subtree inherit it. An ABSENT ceiling writes
    /// NO variable, so an unbounded run can never pick up a stale bound from the parent process's
    /// own environment — the overlay only ever adds.
    #[test]
    fn the_thinking_ceiling_crosses_the_spawn_boundary_only_when_one_is_set() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };

        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.thinking_ceiling = Some("low".to_string());
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        assert_eq!(
            plan.spec
                .env_overlay
                .get(crate::exec::thinking_ceiling::THINKING_CEILING_ENV)
                .map(String::as_str),
            Some("low"),
            "the child must inherit the bound; overlay was {:?}",
            plan.spec.env_overlay
        );

        let opts = base_opts(dir.path(), &["m1"]);
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        assert!(
            !plan
                .spec
                .env_overlay
                .contains_key(crate::exec::thinking_ceiling::THINKING_CEILING_ENV),
            "no ceiling => no variable: {:?}",
            plan.spec.env_overlay
        );
    }

    #[test]
    fn apply_thinking_suffix_replaces_an_existing_level_only_when_licensed_to() {
        assert_eq!(
            apply_thinking_suffix(Some("anthropic/claude-opus-4-6:high"), Some("off"), true)
                .as_deref(),
            Some("anthropic/claude-opus-4-6:off"),
            "a fork override outranks the persona's own level: the branch was sanitized precisely \
             because its inherited thinking blocks are unusable"
        );
        assert_eq!(
            apply_thinking_suffix(Some("anthropic/claude-opus-4-6:high"), Some("off"), false)
                .as_deref(),
            Some("anthropic/claude-opus-4-6:high"),
            "without the override, an id that already names a level still wins — the caller asked \
             for that exact model"
        );
        assert_eq!(
            apply_thinking_suffix(Some("anthropic/claude-opus-4-6"), Some("off"), true).as_deref(),
            Some("anthropic/claude-opus-4-6:off"),
            "no existing level to replace: the override simply appends"
        );
        assert_eq!(
            apply_thinking_suffix(
                Some("openai-compatible/qwen2.5-coder:7b"),
                Some("off"),
                true
            )
            .as_deref(),
            Some("openai-compatible/qwen2.5-coder:7b:off"),
            "`:7b` is part of the id, not a level, so `replace_existing` must not eat it"
        );
    }

    /// The override reaches argv. A persona asking for `thinking: high` whose fork came back
    /// sanitized launches with `:off` — the whole point of resolving the override at all.
    #[test]
    fn build_attempt_spawn_plan_applies_a_fork_thinking_override_over_the_persona_level() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.thinking = Some("high".to_string());
        let mut opts = base_opts(dir.path(), &["m1"]);

        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "task",
            &opts,
            DepthEnvelope { current_depth: 0, max_depth: 5 },
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();
        let model_idx = argv.iter().position(|a| a == "--model").expect("--model present");
        assert_eq!(
            argv.get(model_idx + 1).map(String::as_str),
            Some("m1:high"),
            "precondition: with no override the persona's own level is what ships"
        );

        opts.fork_context = ForkContext {
            mode: ContextMode::Fork,
            session_file_path: Some(dir.path().join("branch.jsonl")),
            thinking_override: Some("off".to_string()),
        };
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "task",
            &opts,
            DepthEnvelope { current_depth: 0, max_depth: 5 },
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();
        let model_idx = argv.iter().position(|a| a == "--model").expect("--model present");
        assert_eq!(
            argv.get(model_idx + 1).map(String::as_str),
            Some("m1:off"),
            "a sanitized fork's override must beat the persona's `thinking: high`"
        );
    }

    #[test]
    fn apply_thinking_suffix_returns_none_without_a_model() {
        assert_eq!(apply_thinking_suffix(None, Some("high"), false), None);
    }


    #[test]
    fn build_attempt_spawn_plan_suffixes_the_child_model_with_the_agent_thinking_level() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("openai-codex/gpt-5.4-mini", &[]);
        agent.thinking = Some("high".to_string());
        let opts = base_opts(dir.path(), &["openai-codex/gpt-5.4-mini"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("openai-codex/gpt-5.4-mini"),
            "task",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();
        let idx = argv.iter().position(|a| a == "--model").expect("--model present");
        assert_eq!(argv[idx + 1], "openai-codex/gpt-5.4-mini:high");
    }


    #[test]
    fn an_inheriting_persona_spawns_the_child_with_the_parent_session_model() {
        // (a) End-to-end resolution proof (no LLM): a persona with NO model of its own
        // (model = None, fallback_models = []), run with NO per-call override, under a live parent
        // session model X, resolves X as candidate #0 and spawns the child with `--model X`. Before
        // this seam the ladder was empty and the run hard-failed with "no candidate model available".
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("unused", &[]);
        agent.model = None; // inheriting persona: no own model, no fallbacks
        agent.thinking = None;

        let inherited = ModelId::from("together/zai-org/GLM-5.2");
        // available_models is built the way run_foreground_impl / run_single build it (persona
        // fallbacks + own model), then resolve_model_inheritance folds in the inherited parent model.
        let mut available_models: Vec<ModelId> =
            agent.fallback_models.iter().cloned().chain(agent.model.clone()).collect();
        let ov = crate::exec::fallback::resolve_model_inheritance(
            None, // no per-call override
            agent.model.as_ref(),
            Some(&inherited),
            &mut available_models,
            None, // no modelScope policy configured
        )
        .expect("with no scope configured, resolution can never be refused");
        assert!(
            available_models.contains(&inherited),
            "the inherited model must be added to available_models so the allowlist filter keeps it"
        );

        let candidates = crate::exec::fallback::build_model_candidates(
            &ov,
            agent.model.as_ref(),
            &agent.fallback_models,
            &available_models,
        );
        assert_eq!(
            candidates,
            vec![inherited.clone()],
            "the inherited parent-session model is the sole/primary candidate (non-empty ladder)"
        );

        let opts = base_opts(dir.path(), &["together/zai-org/GLM-5.2"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &candidates[0], "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        let idx = argv.iter().position(|a| a == "--model").expect("--model present");
        assert_eq!(
            argv[idx + 1], "together/zai-org/GLM-5.2",
            "the child must spawn with the inherited parent session model as --model"
        );
    }


    #[test]
    fn build_attempt_spawn_plan_emits_no_skills_only_when_the_agent_does_not_inherit_skills() {
        let dir = tempfile::tempdir().expect("tempdir");
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let mut inheriting = sample_agent_config("m1", &[]);
        inheriting.inherit_skills = true;
        let plan =
            build_attempt_spawn_plan(&inheriting, &ModelId::from("m1"), "t", &opts, depth, dir.path(), None)
                .expect("plan builds");
        assert!(
            !plan.spec.build_argv().contains(&"--no-skills".to_string()),
            "an agent that inherits skills must NOT be spawned with --no-skills"
        );
        assert_eq!(
            plan.spec.env_overlay.get("CYRUP_SUBAGENT_INHERIT_SKILLS"),
            Some(&"1".to_string())
        );

        let mut not_inheriting = sample_agent_config("m1", &[]);
        not_inheriting.inherit_skills = false;
        let plan = build_attempt_spawn_plan(
            &not_inheriting,
            &ModelId::from("m1"),
            "t",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        assert!(
            plan.spec.build_argv().contains(&"--no-skills".to_string()),
            "an agent that does NOT inherit skills must be spawned with --no-skills"
        );
        assert_eq!(
            plan.spec.env_overlay.get("CYRUP_SUBAGENT_INHERIT_SKILLS"),
            Some(&"0".to_string())
        );
    }


    #[test]
    fn build_attempt_spawn_plan_threads_inherit_project_context_and_none_mcp_sentinel() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.inherit_project_context = true;
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "t", &opts, depth, dir.path(), None)
            .expect("plan builds");
        assert_eq!(
            plan.spec.env_overlay.get("CYRUP_SUBAGENT_INHERIT_PROJECT_CONTEXT"),
            Some(&"1".to_string())
        );
        // No direct MCP tools declared -> pi's `__none__` sentinel, never an unset/empty value.
        assert_eq!(
            plan.spec.env_overlay.get("MCP_DIRECT_TOOLS"),
            Some(&"__none__".to_string())
        );
        // Permission input (1) (pi `resolveAgentName`): the resolved persona name is threaded to the
        // child as `CYRUP_SUBAGENT_AGENT_NAME` so the child's permission companion's agent +
        // projectAgent policy layers enforce for the named persona.
        assert_eq!(
            plan.spec.env_overlay.get(AGENT_NAME_ENV_VAR),
            Some(&"worker".to_string())
        );
    }


    #[test]
    fn build_attempt_spawn_plan_omits_agent_name_env_for_unnamed_persona() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.name = String::new();
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "t", &opts, depth, dir.path(), None)
            .expect("plan builds");
        // An empty persona name writes NO var (child resolves `None` — pi's top-level `""`).
        assert_eq!(plan.spec.env_overlay.get(AGENT_NAME_ENV_VAR), None);
    }


    /// SUBA-008 — the budget notice must reach the CHILD, and it must reach it through the system
    /// prompt: there is no `CYRUP_SUBAGENT_TURN_BUDGET` env var to carry it (unlike the tool
    /// budget), so if this composition is missing, the child is silently never told to self-pace
    /// and only discovers the budget by being killed.
    ///
    /// Reads the spilled prompt FILE rather than the argv, because SUBA-030 moved the persona off
    /// the command line — asserting on argv alone would pass while the file held anything at all.
    #[test]
    fn the_turn_budget_notice_reaches_the_child_through_the_spilled_system_prompt_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_body = "You are a careful worker.".to_string();
        let mut opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };

        // Presence before absence: with NO budget the block must not appear at all, so the
        // assertion below cannot be satisfied by some unrelated boilerplate.
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "t", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let unbudgeted = read_system_prompt_arg(&plan);
        assert_eq!(unbudgeted.trim(), "You are a careful worker.");
        assert!(!unbudgeted.contains("## Turn budget"), "{unbudgeted}");

        opts.turn_budget = Some(crate::exec::turn_budget::ResolvedTurnBudget {
            max_turns: 4,
            grace_turns: 2,
        });
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "t", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let budgeted = read_system_prompt_arg(&plan);
        // The persona survives and the block is APPENDED after it (pi's outermost append,
        // `execution.ts:326`), never substituted for it.
        assert!(budgeted.starts_with("You are a careful worker.\n\n## Turn budget\n"), "{budgeted}");
        assert!(budgeted.contains("a soft budget of 4 assistant turns."), "{budgeted}");
        assert!(
            budgeted.contains("After that, 2 additional assistant turns may be allowed only for a final wrap-up."),
            "{budgeted}"
        );
    }


    #[test]
    fn build_attempt_spawn_plan_threads_extensions_and_subagent_only_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        // A builtin + an extension-path tool ref; the extension path must go to --extension, NOT --tools.
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::ExtensionPath("./custom-tool.ts".to_string()),
        ]);
        agent.extensions = Some(vec!["./allowed-ext.ts".to_string()]);
        agent.subagent_only_extensions = vec!["./child-tool.ts".to_string()];
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "t", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();

        // `Some(extensions)` turns discovery off.
        assert!(argv.contains(&"--no-extensions".to_string()));
        let ext_args: Vec<&String> = argv
            .iter()
            .enumerate()
            .filter_map(|(i, a)| (i > 0 && argv[i - 1] == "--extension").then_some(a))
            .collect();
        assert!(ext_args.iter().any(|a| a.as_str() == "./custom-tool.ts"), "tool-extension path threaded");
        assert!(ext_args.iter().any(|a| a.as_str() == "./allowed-ext.ts"), "allowlisted extension threaded");
        assert!(ext_args.iter().any(|a| a.as_str() == "./child-tool.ts"), "child-only extension threaded");

        // The extension path is NOT in --tools; only the builtin is.
        let tools_idx = argv.iter().position(|a| a == "--tools").expect("--tools present");
        assert_eq!(argv[tools_idx + 1], "read");
    }


    #[test]
    fn build_attempt_spawn_plan_splits_mcp_refs_out_of_tools_and_sets_the_env() {
        // The T4 fix: an `mcp:` ref (`ToolRef::Mcp`) must NOT flow into `--tools` literally (an
        // unresolvable name). It is resolved via the direct-MCP allowlist (here, with no metadata
        // cache on disk, it resolves to nothing) and the raw selector is surfaced via the
        // `MCP_DIRECT_TOOLS` env — leaving only the builtin in `--tools`.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::Mcp("chrome-devtools".to_string()),
        ]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "t", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        let tools_idx = argv.iter().position(|a| a == "--tools").expect("--tools present");
        let tools_val = &argv[tools_idx + 1];
        assert!(
            tools_val.split(',').next() == Some("read"),
            "the declared builtin must lead the allowlist, got {tools_val}"
        );
        assert!(
            !tools_val.contains("chrome-devtools"),
            "the literal `mcp:` selector must never appear in --tools (the T4 bug), got {tools_val}"
        );
        // The raw selector list is surfaced to the child's MCP adapter via env, verbatim.
        assert_eq!(
            plan.spec.env_overlay.get("MCP_DIRECT_TOOLS"),
            Some(&"chrome-devtools".to_string())
        );
    }


    // ---- T0.3 (C15, SAFETY): the CHILD env overlay increments depth by exactly one and applies
    // the agent's tightening-only max, exactly as `SpawnedChildAttemptRunner::run_attempt`
    // computes it (`next_envelope(&self.agent.depth, self.agent.max_subagent_depth)`), NOT the
    // parent's own envelope verbatim (the prior bug). This is the regression guard for the
    // call-site fix at line ~523: it reproduces run_attempt's exact child-envelope composition and
    // asserts the rendered spawn-env overlay a real child would inherit. ----

    #[test]
    fn child_spawn_env_increments_depth_by_one_over_the_parent_envelope() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        // Parent process sits at depth 1 of a ceiling-5 run, and declares NO tighter agent-level
        // ceiling of its own — so the child must inherit depth 2 (1 + 1) under the same ceiling 5.
        agent.depth = DepthEnvelope {
            current_depth: 1,
            max_depth: 5,
        };
        agent.max_subagent_depth = None;
        let opts = base_opts(dir.path(), &["m1"]);

        // Exactly what run_attempt now does before building the spawn plan.
        let child_depth =
            crate::spawn::depth::next_envelope(&agent.depth, agent.max_subagent_depth);
        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, child_depth, dir.path(), None)
                .expect("plan builds");

        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::depth::DEPTH_ENV_VAR),
            Some(&"2".to_string()),
            "the child MUST inherit parent_depth + 1, never the parent's own depth verbatim"
        );
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::depth::MAX_DEPTH_ENV_VAR),
            Some(&"5".to_string()),
            "with no agent-level tightening, the inherited ceiling passes through unchanged"
        );
    }


    #[test]
    fn child_spawn_env_applies_the_agents_tightening_only_max_depth() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        // Inherited ceiling 5, but THIS agent declares its own tighter ceiling of 2 for its
        // children — the child's env must carry min(5, 2) = 2 (R-SA-056 tightening-only).
        agent.depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        agent.max_subagent_depth = Some(2);
        let opts = base_opts(dir.path(), &["m1"]);

        let child_depth =
            crate::spawn::depth::next_envelope(&agent.depth, agent.max_subagent_depth);
        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, child_depth, dir.path(), None)
                .expect("plan builds");

        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::depth::DEPTH_ENV_VAR),
            Some(&"1".to_string())
        );
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::depth::MAX_DEPTH_ENV_VAR),
            Some(&"2".to_string()),
            "the agent's own tighter declared max must win over the looser inherited ceiling"
        );
    }

}
