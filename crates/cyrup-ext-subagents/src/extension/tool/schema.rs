//! The advertised JSON-Schema for the `subagent` tool's parameters, built from one `sj_*`
//! fragment per property.

use crate::extension::tool::text::SUBAGENT_ACTIONS;

// -------------------------------------------------------------------------------------------------
// LLM-facing JSON Schema builders (a faithful port of `schemas.ts`'s `SubagentParamsSchema`, C8)
//
// Each helper returns one reusable schema fragment, mirroring the TypeBox `Type.*` fragments the pi
// source composes `SubagentParamsSchema` from (`OutputOverride`, `ReadsOverride`, `SkillOverride`,
// `OutputModeOverride`, `AcceptanceOverride`, `JsonSchemaObject`, `TaskItem`, `ParallelTaskSchema`,
// `DynamicExpandSchema`, `DynamicParallelTemplateSchema`, `DynamicCollectSchema`, `ChainItem`,
// `ControlOverrides`). Nested per-fragment descriptions are omitted to match pi's provider-payload
// pruning (`keepTopLevelParameterDescriptions`, `schemas.ts:8-31`), which keeps only the top-level
// parameter descriptions; the top-level descriptions themselves are kept in [`subagent_tool_parameters`].
// -------------------------------------------------------------------------------------------------

/// `OutputOverride` (`schemas.ts:42-48`): output filename/path (string), or `false` to disable.
fn sj_output_override() -> serde_json::Value {
    serde_json::json!({ "anyOf": [ { "type": "string" }, { "type": "boolean" } ] })
}

/// `ReadsOverride` (`schemas.ts:55-61`): array of filenames to read first, or `false` to disable.
fn sj_reads_override() -> serde_json::Value {
    serde_json::json!({ "anyOf": [ { "type": "array", "items": { "type": "string" } }, { "type": "boolean" } ] })
}

/// `SkillOverride` (`schemas.ts:33-40`): skill name(s) (string / array of strings), or boolean.
fn sj_skill_override() -> serde_json::Value {
    serde_json::json!({ "anyOf": [ { "type": "array", "items": { "type": "string" } }, { "type": "boolean" }, { "type": "string" } ] })
}

/// `OutputModeOverride` (`schemas.ts:50-53`): `inline` (default) or `file-only`.
fn sj_output_mode() -> serde_json::Value {
    serde_json::json!({ "type": "string", "enum": ["inline", "file-only"] })
}

/// `AcceptanceOverride` (`schemas.ts:80-93` @v0.43.0): a level enum, `false`, or an object policy.
///
/// Upstream is a FOUR-branch `anyOf`, and the split between the first two branches is the whole
/// point. The requestable enum is exactly `["auto", "attested", "checked"]` (`schemas.ts:82`);
/// `"reviewed"` lives alone in a second branch marked `deprecated` whose description says
/// "Recognized only so preflight can explain that reviewed is an achieved status"
/// (`schemas.ts:83-88`).
///
/// This crate previously advertised ONE wide enum that also offered `"none"` and `"verified"`.
/// Both are hard-rejected by [`crate::exec::acceptance::lower_acceptance_input`]
/// (`acceptance.ts:183-184`: `none` needs a reason, `verified` needs a non-empty `verify[]`), which
/// is `AcceptanceInput = Exclude<AcceptanceLevel, "none" | "verified">` (`shared/types.ts:684-685`)
/// restated. Advertising a value the dispatch refuses is precisely the advertise-vs-dispatch
/// violation this crate forbids, so the enum is narrowed to upstream's three.
///
/// `"reviewed"` is the ONE deliberate exception, and it is upstream's own: it is still advertised
/// so the model gets the explanatory rejection rather than a bare schema violation. The invariant
/// holds because a dispatch arm exists — it is the explanatory error, not silence.
pub(crate) fn sj_acceptance_override() -> serde_json::Value {
    serde_json::json!({ "anyOf": [
        { "type": "string", "enum": ["auto", "attested", "checked"] },
        {
            "type": "string",
            "enum": ["reviewed"],
            "deprecated": true,
            "description": "Invalid as an explicit policy. Recognized only so preflight can explain that reviewed is an achieved status."
        },
        { "type": "boolean", "enum": [false] },
        { "type": "object", "additionalProperties": true }
    ] })
}

/// `JsonSchemaObject` (`schemas.ts:63-67`): an open JSON Schema object for structured output.
fn sj_json_schema_object() -> serde_json::Value {
    serde_json::json!({ "type": "object", "additionalProperties": true })
}

/// SUBA-008 — `TurnBudgetOverride` (`extension/schemas.ts:104-107` @v0.43.0), including its
/// `additionalProperties: false` and its description, verbatim.
///
/// `maxTurns` is the SOFT limit and `graceTurns` (default 1) is how far past it the child may go
/// before the supervisor aborts it — the description says so in upstream's own words.
/// SUBA-021 — `UsageBudgetOverride` (`extension/schemas.ts` @v0.43.0): two optional metrics, each
/// a `{ soft?, hard }` pair, with `additionalProperties: false` on all three levels because
/// `validateUsageBudgetConfig`/`validateLimit` (`runs/shared/usage-budget.ts:6`/`:18`) refuse an
/// unknown key outright — the schema and the validator have to agree or the model is told a key is
/// acceptable and then refused for using it.
fn sj_usage_budget_override() -> serde_json::Value {
    let metric = |what: &str| {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["hard"],
            "properties": {
                "soft": { "type": "number", "exclusiveMinimum": 0 },
                "hard": { "type": "number", "exclusiveMinimum": 0 }
            },
            "description": format!("Optional {what} budget. soft is advisory; reaching hard ends the run.")
        })
    };
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": { "tokens": metric("token"), "costUsd": metric("cost (USD)") },
        "description": "Optional usage budget for this run, enforced against reported totals. Provide tokens and/or costUsd; reaching a hard limit ends the run."
    })
}

fn sj_turn_budget_override() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["maxTurns"],
        "properties": {
            "maxTurns": { "type": "integer", "minimum": 1 },
            "graceTurns": { "type": "integer", "minimum": 0 }
        },
        "description": "Optional assistant-turn budget. At maxTurns the child is asked to wrap up; after graceTurns additional assistant turns it is aborted and partial output is returned."
    })
}

/// SUBA-047 — `ToolBudgetOverride` (`extension/schemas.ts:116-120` @v0.43.0), including its
/// `additionalProperties: false` and its description, verbatim. `block` is `ToolBudgetBlock`
/// (`:112-117`): either an array of tool names or the literal `"*"`.
fn sj_tool_budget_override() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["hard"],
        "properties": {
            "soft": { "type": "integer", "minimum": 1 },
            "hard": { "type": "integer", "minimum": 1 },
            "block": {
                "anyOf": [
                    { "type": "array", "items": { "type": "string" } },
                    { "type": "string", "enum": ["*"] }
                ]
            }
        },
        "description": "Optional child tool-call budget. soft nudges the child; after hard, block tools (default read/grep/find/ls, or '*' for all tools) are blocked so the child can finalize."
    })
}

/// `TaskItem` (`schemas.ts:78-90`): one top-level PARALLEL `tasks[]` element (agent+task required).
fn sj_task_item() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["agent", "task"],
        "properties": {
            "agent": { "type": "string" },
            "task": { "type": "string" },
            "cwd": { "type": "string" },
            "count": { "type": "integer", "minimum": 1 },
            "output": sj_output_override(),
            "outputMode": sj_output_mode(),
            "reads": sj_reads_override(),
            "progress": { "type": "boolean" },
            "model": { "type": "string" },
            "skill": sj_skill_override(),
            "acceptance": sj_acceptance_override()
        }
    })
}

/// `ParallelTaskSchema` (`schemas.ts:133-152`): a static parallel task inside a chain step (agent
/// required, task optional).
fn sj_parallel_task() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["agent"],
        "properties": {
            "agent": { "type": "string" },
            "task": { "type": "string" },
            "phase": { "type": "string" },
            "label": { "type": "string" },
            "as": { "type": "string" },
            "outputSchema": sj_json_schema_object(),
            "cwd": { "type": "string" },
            "count": { "type": "integer", "minimum": 1 },
            "output": sj_output_override(),
            "outputMode": sj_output_mode(),
            "reads": sj_reads_override(),
            "progress": { "type": "boolean" },
            "skill": sj_skill_override(),
            "model": { "type": "string" },
            "acceptance": sj_acceptance_override()
        }
    })
}

/// `DynamicParallelTemplateSchema` (`schemas.ts:165-182`): the single per-item child template used
/// with `expand`/`collect` dynamic fanout.
fn sj_dynamic_parallel_template() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["agent"],
        "properties": {
            "agent": { "type": "string" },
            "task": { "type": "string" },
            "phase": { "type": "string" },
            "label": { "type": "string" },
            "outputSchema": sj_json_schema_object(),
            "cwd": { "type": "string" },
            "output": sj_output_override(),
            "outputMode": sj_output_mode(),
            "reads": sj_reads_override(),
            "progress": { "type": "boolean" },
            "skill": sj_skill_override(),
            "model": { "type": "string" },
            "acceptance": sj_acceptance_override()
        }
    })
}

/// `DynamicExpandSchema` (`schemas.ts:154-163`): the fanout source pointer + bounds.
fn sj_dynamic_expand() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["from"],
        "properties": {
            "from": {
                "type": "object",
                "additionalProperties": false,
                "required": ["output", "path"],
                "properties": {
                    "output": { "type": "string" },
                    "path": { "type": "string" }
                }
            },
            "item": { "type": "string" },
            "key": { "type": "string" },
            "maxItems": { "type": "integer", "minimum": 0 },
            "onEmpty": { "type": "string", "enum": ["skip", "fail"] }
        }
    })
}

/// `DynamicCollectSchema` (`schemas.ts:184-187`): the fanned-in collected-array output binding.
fn sj_dynamic_collect() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["as"],
        "properties": {
            "as": { "type": "string" },
            "outputSchema": sj_json_schema_object()
        }
    })
}

/// `ChainItem` (`schemas.ts:190-229`): one `chain[]` element — sequential `{agent, task?, ...}`,
/// static `{parallel: [...]}`, or dynamic `{expand, parallel: {...}, collect}` fanout (flattened so
/// chain steps need no object-shape `anyOf`/`oneOf` union at the item level, exactly as pi does).
fn sj_chain_item() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "agent": { "type": "string" },
            "task": { "type": "string" },
            "phase": { "type": "string" },
            "label": { "type": "string" },
            "as": { "type": "string" },
            "outputSchema": sj_json_schema_object(),
            "cwd": { "type": "string" },
            "output": sj_output_override(),
            "outputMode": sj_output_mode(),
            "reads": sj_reads_override(),
            "progress": { "type": "boolean" },
            "skill": sj_skill_override(),
            "model": { "type": "string" },
            "acceptance": sj_acceptance_override(),
            "parallel": {
                "anyOf": [
                    { "type": "array", "items": sj_parallel_task() },
                    sj_dynamic_parallel_template()
                ]
            },
            "expand": sj_dynamic_expand(),
            "collect": sj_dynamic_collect(),
            "concurrency": { "type": "number" },
            "failFast": { "type": "boolean" },
            "worktree": { "type": "boolean" }
        }
    })
}

/// `ControlOverrides` (`extension/schemas.ts:242-255` @v0.43.0): per-run subagent-control attention
/// thresholds and notification routing.
///
/// SUBA-041 unhooked this fragment from [`subagent_tool_parameters`] because cyrup had the control
/// CONFIG shape ([`crate::registration::ControlConfig`]) but neither `resolveControlConfig` nor the
/// notice pipeline behind it. SUBA-N05 landed both — [`crate::exec::control`] is a full port of
/// `runs/shared/subagent-control.ts`, [`crate::exec::control::ControlMonitor`] raises real events off
/// the child's NDJSON stream, and [`crate::extension::SubagentExecutor::foreground_control_notifier`] feeds them to
/// [`crate::tui::notices::ControlNoticeState`] — so the fragment is live again and the dispatcher
/// honours the param on both the foreground and the async path.
///
/// Per-property descriptions are pruned, matching how [`subagent_tool_parameters`] treats every
/// other nested object shape (`tasks[]`, `chain[]`): pi's own top-level `control` entry
/// (`schemas.ts:279`) carries no description of its own either, so the union of what the model sees
/// is `{type, minimum, enum}` structure exactly as upstream ships it after
/// `keepTopLevelParameterDescriptions` pruning.
fn sj_control_overrides() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "enabled": { "type": "boolean" },
            "needsAttentionAfterMs": { "type": "integer", "minimum": 1 },
            "activeNoticeAfterMs": { "type": "integer", "minimum": 1 },
            "activeNoticeAfterTurns": { "type": "integer", "minimum": 1 },
            "activeNoticeAfterTokens": { "type": "integer", "minimum": 1 },
            "failedToolAttemptsBeforeAttention": { "type": "integer", "minimum": 1 },
            "notifyOn": { "type": "array", "items": { "type": "string", "enum": ["active_long_running", "needs_attention"] } },
            "notifyChannels": { "type": "array", "items": { "type": "string", "enum": ["event", "async", "intercom"] } }
        }
    })
}

pub(crate) fn subagent_tool_parameters() -> serde_json::Value {
    // Built via per-property inserts rather than one giant `json!` literal: a single 33-property
    // `json!` object overflows the macro's default `recursion_limit` at expansion time. Each insert
    // below is its own shallow `json!` invocation, and the root wrapper is a 3-key `json!`.
    let mut props = serde_json::Map::new();
    props.insert("agent".to_string(), serde_json::json!({ "type": "string", "description": "Agent name (SINGLE mode) or target for management get/update/delete" }));
    props.insert("task".to_string(), serde_json::json!({ "type": "string", "description": "Task (SINGLE mode, optional for self-contained agents)" }));
    props.insert("action".to_string(), serde_json::json!({
        "type": "string",
        // G77: `stop` sits between `steer` and `append-step`, upstream's own position in
        // `SUBAGENT_ACTIONS` (`shared/types.ts:1885` @v0.43.0: `… "interrupt", "resume", "steer",
        // "stop", "append-step", …`). Advertised together with its `route_control_action` dispatch
        // arm (`SubagentExecutor::control_stop`) in this same change, per the crate's
        // advertise-vs-dispatch invariant.
        // SUBA-038: derived from [`SUBAGENT_ACTIONS`], not hand-written — a hand-written copy is
        // exactly what let the two unknown-action messages drift away from what dispatches.
        "enum": SUBAGENT_ACTIONS,
        "description": "Management/control action. Omit for execution mode."
    }));
    // G90 (advertise-vs-dispatch, the OTHER direction): these three, plus `message` below, are the
    // schema properties `action='steer'` is addressed through, and all four dropped pi's own
    // `action='steer'` clause (`extension/schemas.ts:224,227,230,238` @v0.34.0, descriptions
    // VERBATIM). With `steer` in the `action` enum and a real dispatch arm, a model told the verb
    // exists but shown no property that mentions it has to guess which of `id`/`runId`/`dir` to
    // address it with — the exact ambiguity these descriptions exist to remove.
    // The three `watchdog.*` properties (`extension/schemas.ts:285-288`, descriptions VERBATIM),
    // added with the four `watchdog.*` enum values and `route_watchdog_action` in the same change,
    // per the crate's advertise-vs-dispatch invariant. Without them a model told
    // `watchdog.configure` exists has no advertised way to say WHAT to configure.
    props.insert("scope".to_string(), serde_json::json!({ "type": "string", "enum": ["session", "user", "project"], "description": "Scope for action='watchdog.configure'. Defaults to session to avoid persistent settings writes unless user/project is explicit." }));
    props.insert("target".to_string(), serde_json::json!({ "type": "string", "enum": ["main", "children", "child"], "description": "Target for watchdog actions." }));
    props.insert("thinking".to_string(), serde_json::json!({ "anyOf": [{ "type": "string" }, { "type": "boolean", "enum": [false] }], "description": "Thinking level for action='watchdog.configure' (off/minimal/low/medium/high/xhigh/max, inherit, or false for off)." }));
    props.insert("id".to_string(), serde_json::json!({ "type": "string", "description": "Run id or prefix for action='status', action='interrupt', action='stop', action='resume', action='steer', or action='append-step'." }));
    props.insert("runId".to_string(), serde_json::json!({ "type": "string", "description": "Target run ID for action='interrupt', action='stop', action='resume', action='steer', or action='append-step'. Defaults to the most recently active controllable run for interrupt. Prefer id for new calls." }));
    props.insert("dir".to_string(), serde_json::json!({ "type": "string", "description": "Async run directory for action='status', action='stop', action='resume', or action='steer'." }));
    props.insert("index".to_string(), serde_json::json!({ "type": "integer", "minimum": 0, "description": "Zero-based child index for actions that target a specific child or transcript." }));
    // G92: `view` + `lines` (pi `extension/schemas.ts:233-237` @v0.34.0, descriptions VERBATIM).
    // Both are read by `route_control_action`'s `status` arm — `view` selects
    // `background::fleet_view::format_fleet` / `format_async_run_transcript`, `lines` is the
    // transcript tail budget. Advertised only because both have a real dispatch arm in this same
    // change (the crate's advertise-vs-dispatch invariant, see
    // `subagent_tool_parameters_pin_pis_shape`).
    props.insert("view".to_string(), serde_json::json!({
        "type": "string",
        "enum": ["fleet", "transcript"],
        "description": "Optional status view. Use view='fleet' for a read-only active foreground/async fleet surface, or view='transcript' with id/dir (and optional index) to tail a run transcript."
    }));
    props.insert("lines".to_string(), serde_json::json!({ "type": "integer", "minimum": 1, "maximum": 500, "description": "Maximum transcript lines for action='status', view='transcript'. Defaults to 80." }));
    // SUBA-055 — pi `extension/schemas.ts:281` @v0.47.1 is `topic: Type.Optional(Type.String())`:
    // no enum, no description. Reproduced exactly, including the absence of the description — an
    // invented one would be cyrup-original model-facing text, and the valid set is already the
    // unknown-topic message's job (`registration::guide::read_subagent_guide`).
    props.insert("topic".to_string(), serde_json::json!({ "type": "string" }));
    props.insert("message".to_string(), serde_json::json!({ "type": "string", "description": "Follow-up message for action='resume' or non-terminal guidance for action='steer'. Use index to choose a child from multi-child runs." }));
    // SUBA-049 — pi `extension/schemas.ts:283` @v0.43.0, description VERBATIM. Advertised together
    // with its consumer in this same change (`SteerDeliveryMode` is read by `control_steer`, written
    // onto the `SteerRequest`, and honoured by the child-side inbox), per the crate's
    // advertise-vs-dispatch invariant.
    props.insert("mode".to_string(), serde_json::json!({
        "type": "string",
        "enum": ["steer", "follow_up", "auto"],
        "description": "Delivery mode for action='steer'. steer interrupts at the next safe point (default), follow_up waits for the next turn boundary, and auto follows up mid-turn but delivers immediately between turns."
    }));
    props.insert("chainName".to_string(), serde_json::json!({ "type": "string", "description": "Chain name for get/update/delete management actions" }));
    props.insert("config".to_string(), serde_json::json!({
        "anyOf": [ { "type": "object", "additionalProperties": true }, { "type": "string" } ],
        "description": "Agent/chain config for create/update. Object or JSON string; presence of steps creates a chain."
    }));
    props.insert("tasks".to_string(), serde_json::json!({
        "type": "array",
        "items": sj_task_item(),
        "description": "PARALLEL mode: [{agent, task, count?, output?, outputMode?, reads?, progress?}, ...]"
    }));
    props.insert("concurrency".to_string(), serde_json::json!({ "type": "integer", "minimum": 1, "description": "Top-level PARALLEL mode only: max concurrent tasks. Defaults to config.parallel.concurrency or 4." }));
    props.insert("worktree".to_string(), serde_json::json!({ "type": "boolean", "description": "Create isolated git worktrees for parallel tasks; requires clean git state." }));
    props.insert("chain".to_string(), serde_json::json!({
        "type": "array",
        "items": sj_chain_item(),
        "description": "CHAIN mode: sequential steps; each result becomes {previous}. append-step takes one tail step and may use {chain_dir}/{outputs.name}."
    }));
    props.insert("context".to_string(), serde_json::json!({
        "type": "string",
        "enum": ["fresh", "fork", "profile"],
        "description": "'fresh' or 'fork' to branch from parent session, or 'profile' to require the selected agent's declared defaultContext. Explicit fresh/fork overrides every child; profile ignores config defaultSubagentContext and fails when an agent has no defaultContext. If omitted, config defaultSubagentContext wins over each agent defaultContext; implicit fork needs a persisted parent session and leaf, else fresh."
    }));
    props.insert("chainDir".to_string(), serde_json::json!({ "type": "string", "description": "Persistent chain artifact directory; defaults to user-scoped temp storage." }));
    props.insert("async".to_string(), serde_json::json!({ "type": "boolean", "description": "Run in background (default: false, or per config)" }));
    // SUBA-N03: pi's VERBATIM descriptions (`extension/schemas.ts:265-266` @v0.34.0). These two
    // read "Optional foreground-only timeout in ms; omit for async/background runs" until now —
    // an instruction to the model that was both false upstream and, once the async branch started
    // refusing the param, a self-fulfilling one. Upstream has always said the opposite.
    props.insert("timeoutMs".to_string(), serde_json::json!({ "type": "integer", "minimum": 1, "description": "Optional run-level timeout in ms for foreground and async/background runs. Alias of maxRuntimeMs." }));
    props.insert("maxRuntimeMs".to_string(), serde_json::json!({ "type": "integer", "minimum": 1, "description": "Alias of timeoutMs for optional run-level timeout in foreground and async/background runs." }));
    props.insert("agentScope".to_string(), serde_json::json!({ "type": "string", "description": "Agent discovery scope: 'user', 'project', or 'both' (default: 'both'; project wins on name collisions)" }));
    props.insert("cwd".to_string(), serde_json::json!({ "type": "string" }));
    props.insert("artifacts".to_string(), serde_json::json!({ "type": "boolean", "description": "Write debug artifacts (default: true)" }));
    // SUBA-N06: `includeProgress` is advertised again, in pi's own position (between `artifacts`
    // and `share`, `schemas.ts:271-273` @v0.34.0) and with pi's description verbatim. It was
    // withheld for exactly one reason — `SingleResult` had no progress object to include or omit —
    // and that reason is gone: `exec::AgentProgress::snapshot` projects the winning attempt's fold
    // into pi's `AgentProgress` shape and `run_sync` publishes it on `SingleResult::progress`
    // under pi's own truthiness gate. Honoured on the foreground path via
    // `SingleRunOverrides::include_progress` and on the async one via
    // `RunnerConfig::include_progress`.
    props.insert("includeProgress".to_string(), serde_json::json!({ "type": "boolean", "description": "Include full progress in result (default: false)" }));
    props.insert("share".to_string(), serde_json::json!({ "type": "boolean", "description": "Upload session to GitHub Gist for sharing (default: false)" }));
    props.insert("sessionDir".to_string(), serde_json::json!({ "type": "string", "description": "Directory to store session logs (default: temp; enables sessions even if share=false)" }));
    props.insert("clarify".to_string(), serde_json::json!({ "type": "boolean", "description": "Show TUI to preview/edit before execution. Explicit clarify: true keeps the run foreground for the clarify UI; omitted clarify can still run in the background when async: true is set." }));
    // SUBA-N05: `control` is advertised again, in pi's own position (between `clarify` and the solo
    // agent overrides, `schemas.ts:278-279` @v0.34.0). It reaches `resolveControlConfig`
    // ([`crate::exec::control::resolve_control_config`]) on the foreground path via
    // `SingleRunOverrides::control` and on the async path via `RunnerConfig::control`, and drives
    // the live attention/notice pipeline in both. pi gives the top-level entry no description of
    // its own, so neither does this.
    props.insert("control".to_string(), sj_control_overrides());
    // pi's own description (`schemas.ts:286`) is kept VERBATIM, including its stale
    // "Relative paths resolve against cwd" clause: pi's `resolveSingleOutputPath`
    // (`single-output.ts:64-77`) only falls back to a cwd when no `relativeBaseDir` is supplied, and
    // `runSinglePath` always supplies one (`resolveSingleRunOutputBaseDir`, `:2882`). Both sides
    // therefore resolve a relative `output` against the run's scoped output dir; the sentence is
    // upstream's inaccuracy, reproduced rather than silently corrected (parity over prose).
    props.insert("output".to_string(), serde_json::json!({
        "anyOf": [ { "type": "string" }, { "type": "boolean" } ],
        "description": "Output file for single agent (string), or false to disable. Relative paths resolve against cwd."
    }));
    props.insert("outputMode".to_string(), serde_json::json!({ "type": "string", "enum": ["inline", "file-only"], "description": "Return saved output inline (default) or only a concise file reference. file-only requires output to be a path." }));
    props.insert("skill".to_string(), serde_json::json!({
        "anyOf": [ { "type": "array", "items": { "type": "string" } }, { "type": "boolean" }, { "type": "string" } ],
        "description": "Skill name(s) to make available (comma-separated), array of strings, or boolean (false disables, true uses default)"
    }));
    props.insert("model".to_string(), serde_json::json!({ "type": "string", "description": "Override model for single agent (e.g. 'anthropic/claude-sonnet-4')" }));
    // SUBA-043 / pi `extension/schemas.ts:351` @v0.43.0 — `outputSchema:
    // Type.Optional(JsonSchemaObject)` is a TOP-LEVEL `SubagentParamsSchema` property, in exactly
    // this position (after `model`, before `agentContract`/`acceptance`) under upstream's "Workflow
    // defaults forwarded to each child" comment. It was advertised only on the `tasks[]`/`chain[]`
    // ITEM schemas here, which made the capability SUBA-S01 landed — schema in, typed JSON out —
    // unreachable from the SINGLE surface a model actually calls: `subagent({agent, task,
    // outputSchema})` parsed (the root schema is `additionalProperties: true`), dropped the schema
    // without error, and returned free prose. The only workaround was a one-item `tasks:[…]`.
    // pi gives the top-level entry no description of its own, so neither does this.
    props.insert("outputSchema".to_string(), sj_json_schema_object());
    // SUBA-047 / pi `extension/schemas.ts:354` @v0.43.0 — `toolBudget:
    // Type.Optional(ToolBudgetOverride)`, shape at `:116-120` (`soft?`, `hard`, `block?`), with
    // upstream's description verbatim. In-baseline since before the ported tag.
    //
    // The ENFORCEMENT half has been complete since SUBA-007 (`exec/tool_budget.rs` + the
    // `TOOL_BUDGET_ENV` hand-off at `exec/mod.rs`), and the frontmatter key is read at
    // `discovery/frontmatter.rs:880` — but the param was never advertised, so the only way to bound
    // a delegation's tool spend was to edit the agent file on disk and a per-call budget passed by
    // an orchestrator was silently discarded.
    // SUBA-008 / pi `extension/schemas.ts:328` @v0.43.0 — `turnBudget:
    // Type.Optional(TurnBudgetOverride)`, the key immediately ABOVE `toolBudget` in upstream's own
    // property order. Advertised together with its enforcement (`exec/turn_budget.rs` +
    // `drive_attempt`'s per-turn fold), never ahead of it — SUBA-047's lesson.
    props.insert("turnBudget".to_string(), sj_turn_budget_override());
    // SUBA-021 / pi `extension/schemas.ts:330` @v0.43.0 — `usageBudget:
    // Type.Optional(UsageBudgetOverride)`, immediately below `turnBudget` in upstream's own
    // property order. Advertised together with its enforcement (`exec/usage_budget.rs` + the
    // terminal check in `run_sync`'s settle), never ahead of it.
    props.insert("usageBudget".to_string(), sj_usage_budget_override());
    props.insert("toolBudget".to_string(), sj_tool_budget_override());
    // SUBA-046 / pi `extension/schemas.ts:283` @v0.43.0 — `additional`, with upstream's description
    // verbatim. `grant-spawn-budget` was advertised in the child-safe tool description while the
    // verb itself landed on the unknown-action arm, and the param it needs was never advertised at
    // all; both halves land together, because advertising either alone is the defect class.
    props.insert("additional".to_string(), serde_json::json!({
        "type": "integer",
        "minimum": 1,
        "description": "Positive launches to add with action='grant-spawn-budget'. Root interactive parent with native user confirmation only; total grants cannot exceed the original configured cap."
    }));
    props.insert("acceptance".to_string(), serde_json::json!({
        "anyOf": [
            { "type": "string", "enum": ["auto", "none", "attested", "checked", "verified", "reviewed"] },
            { "type": "boolean", "enum": [false] },
            { "type": "object", "additionalProperties": true }
        ],
        "description": "Optional acceptance policy. Omitted means auto-inferred; verified requires configured runtime commands."
    }));
    // The mission surface (`extension/schemas.ts:297-304` @v0.43.0), advertised together with its
    // dispatch arms: `mission.*` in the `action` enum above routes to
    // `crate::missions::handle_mission_action`, and `missionId`/`mission` additionally bind an
    // EXECUTION call to a mission via `SubagentTool::execute`'s launch binding. Descriptions are
    // upstream's own, verbatim.
    props.insert("missionId".to_string(), serde_json::json!({ "type": "string", "description": "Mission id." }));
    props.insert("mission".to_string(), serde_json::json!({
        "anyOf": [
            { "type": "object", "additionalProperties": true },
            { "type": "boolean", "enum": [false] }
        ],
        "description": "Mission object, or false for no mission. Use objective for intent; goal:true with budget.tokens enables turn-end continuation notices."
    }));
    props.insert("missionUpdate".to_string(), serde_json::json!({
        "type": "object",
        "additionalProperties": true,
        "description": "Mission update: objective, goal false or {paused:boolean}, budget, summary, labels, decisions, artifacts, or delivery receipts."
    }));
    props.insert("missionStatus".to_string(), serde_json::json!({ "type": "string", "description": "Mission status." }));
    props.insert("missionScope".to_string(), serde_json::json!({ "type": "string", "description": "Mission list scope: project (default) or global pointer index." }));
    props.insert("runMode".to_string(), serde_json::json!({ "type": "string", "description": "Attached run mode." }));
    props.insert("runStatus".to_string(), serde_json::json!({ "type": "string", "description": "Attached run status." }));
    props.insert("summary".to_string(), serde_json::json!({ "type": "string", "description": "Mission close summary." }));

    serde_json::json!({
        "type": "object",
        "additionalProperties": true,
        "properties": serde_json::Value::Object(props),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use crate::extension::executor::SubagentExecutor;
    use crate::extension::testsupport::scoped_tool;
    use crate::extension::tool::SubagentTool;
    use crate::extension::tool::params::SubagentToolParams;
    use crate::extension::tool::text::SUBAGENT_TOOL_DESCRIPTION;
    use cyrup_core::CancelToken;
    use cyrup_core::Tool;
    use cyrup_core::ToolCallId;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// C8: the LLM-facing `subagent` tool schema exposes pi's FULL parameter union
    /// (`schemas.ts:257-357`), not just the pre-C8 5-property single-task shape. Asserts every
    /// top-level pi property name is present, the 11-value management/control `action` enum is
    /// complete and correctly ordered, the `context` fresh/fork enum is present, the `tasks[]`
    /// per-task `output`/`outputMode`/`reads`/`progress` fields exist, and the numeric bounds pi
    /// pins (`concurrency`/`timeoutMs`/`maxRuntimeMs` minimum, `index` minimum 0) are carried — the
    /// Rust analog of pi's own `test/unit/schemas.test.ts`.
    ///
    /// SUBA-041 re-scoped the property list: `includeProgress` and `control` were dropped from the
    /// expected set and asserted ABSENT, because this port had no subsystem behind either and
    /// [`SubagentTool::route_single`] refused them.
    ///
    /// SUBA-N05 moved `control` back into the expected set — the subsystem now exists
    /// ([`crate::exec::control`] + [`crate::tui::notices::ControlNoticeState`]) and the dispatcher
    /// honours the param on both the foreground and the async path — and additionally pins its
    /// nested shape (the two enums and the `minimum: 1` bounds), so a future edit cannot advertise
    /// a `control` object whose fields `parse_control_overrides` would silently discard. See
    /// [`single_mode_accepts_every_wired_override_and_never_silently_drops_an_unwired_one`] for the
    /// other half of that invariant.
    ///
    /// SUBA-N06 moved `includeProgress` back too, and with it the withhold list is EMPTY: the
    /// subsystem now exists ([`crate::exec::AgentProgress::snapshot`] →
    /// [`crate::exec::SingleResult::progress`], under pi's own truthiness gate) and the dispatcher
    /// honours the param on the foreground path (`SingleRunOverrides::include_progress`) and the
    /// async one ([`crate::background::runner_main::RunnerConfig::include_progress`]). The
    /// assertion that used to demand its ABSENCE is inverted below rather than deleted — it is the
    /// same invariant, now discharged in the other direction.
    #[test]
    fn subagent_tool_schema_exposes_the_full_pi_parameter_union() {
        let schema = subagent_tool_parameters();
        let props = schema
            .get("properties")
            .and_then(|p| p.as_object())
            .expect("schema has a properties object");

        // Every top-level pi `SubagentParamsSchema` property (schemas.ts:195-263), in source
        // order. As of SUBA-N06 there are no withholds: the list is pi's, entire.
        let expected_properties = [
            "agent", "task", "action", "id", "runId", "dir", "index", "message", "chainName",
            "config", "tasks", "concurrency", "worktree", "chain", "context", "chainDir", "async",
            "timeoutMs", "maxRuntimeMs", "agentScope", "cwd", "artifacts", "includeProgress",
            "share", "sessionDir", "clarify", "control", "output", "outputMode", "skill", "model",
            "acceptance",
        ];
        for name in expected_properties {
            assert!(
                props.contains_key(name),
                "schema must advertise the pi parameter '{name}'; got keys: {:?}",
                props.keys().collect::<Vec<_>>()
            );
        }

        // SUBA-021 — `usageBudget` (`extension/schemas.ts:330` @v0.43.0) is advertised, its nested
        // shape matches what `validateUsageBudgetConfig`/`validateLimit` accept, and a value that
        // reaches the params struct is really carried rather than dropped by serde.
        //
        // Pre-fix all three failed: `rg 'usage_budget' crates/…/src` was 0, so the key was absent
        // from the schema, absent from `SubagentToolParams`, and therefore silently discarded on
        // the way in — an orchestrator that bounded a delegation's spend got an unbounded run and
        // no diagnostic.
        assert!(props.contains_key("usageBudget"));
        let usage = &props["usageBudget"];
        assert_eq!(usage["additionalProperties"], serde_json::json!(false));
        for metric in ["tokens", "costUsd"] {
            let shape = &usage["properties"][metric];
            assert_eq!(shape["additionalProperties"], serde_json::json!(false));
            assert_eq!(shape["required"], serde_json::json!(["hard"]));
            assert!(shape["properties"]["soft"].is_object());
        }
        let parsed: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "agent": "worker",
            "task": "t",
            "usageBudget": { "tokens": { "soft": 800, "hard": 1000 } }
        }))
        .expect("params parse");
        assert!(
            parsed.provided_keys().contains(&"usageBudget"),
            "the key survives deserialization: {:?}",
            parsed.provided_keys()
        );
        // …and the validator behind it produces upstream's verbatim refusal for a bad one.
        assert_eq!(
            crate::exec::usage_budget::validate_usage_budget_config(
                Some(&serde_json::json!({ "tokens": { "hard": 0 } })),
                "usageBudget"
            )
            .expect_err("refused"),
            "usageBudget.tokens.hard must be a positive number."
        );

        // SUBA-041's core invariant: a param the dispatcher refuses UNCONDITIONALLY must not be
        // advertised. SUBA-N06 emptied the withhold list, so the invariant is now discharged from
        // the other side — this loop asserts that NOTHING is withheld, and it is the assertion
        // that must gain an entry (with a citation) if a future param is ever refused outright.
        const UNCONDITIONALLY_REFUSED: &[&str] = &[];
        for name in UNCONDITIONALLY_REFUSED {
            assert!(
                !props.contains_key(*name),
                "'{name}' is rejected at dispatch, so the schema must NOT advertise it"
            );
        }
        assert!(
            UNCONDITIONALLY_REFUSED.is_empty(),
            "the withhold list is expected to be empty as of SUBA-N06; adding an entry means a \
             param is advertised-and-refused, which needs an upstream citation here"
        );

        // SUBA-N05: `control`'s nested shape, pinned against `ControlOverrides`
        // (`extension/schemas.ts:242-255` @v0.43.0). Every advertised field must be one
        // `crate::exec::control::parse_control_overrides` actually reads, and both string unions
        // must match `ControlEventType`/`ControlNotificationChannel`'s wire spellings exactly —
        // advertising an enum member the lowering drops is the same defect class as advertising a
        // param the dispatcher refuses.
        let control_props = props["control"]["properties"]
            .as_object()
            .expect("control carries a properties object");
        assert_eq!(props["control"]["type"], serde_json::json!("object"));
        for (field, minimum) in [
            ("needsAttentionAfterMs", Some(1)),
            ("activeNoticeAfterMs", Some(1)),
            ("activeNoticeAfterTurns", Some(1)),
            ("activeNoticeAfterTokens", Some(1)),
            ("failedToolAttemptsBeforeAttention", Some(1)),
        ] {
            assert_eq!(
                control_props[field]["type"],
                serde_json::json!("integer"),
                "control.{field} is an integer threshold upstream"
            );
            assert_eq!(control_props[field]["minimum"], serde_json::json!(minimum));
        }
        assert_eq!(control_props["enabled"]["type"], serde_json::json!("boolean"));
        assert_eq!(
            control_props["notifyOn"]["items"]["enum"],
            serde_json::json!(["active_long_running", "needs_attention"])
        );
        assert_eq!(
            control_props["notifyChannels"]["items"]["enum"],
            serde_json::json!(["event", "async", "intercom"])
        );

        // The management/control action enum, exact values AND order, against
        // [`SUBAGENT_ACTIONS`] — pi's own list is `shared/types.ts:1885` @v0.43.0 (53 verbs) and
        // both pi's schema and its unknown-action message read it.
        //
        // This is cyrup's CURRENT surface, not upstream's 53: a verb joins this list only in the
        // same change that gives it a dispatch arm, because advertising a verb the dispatcher
        // rejects is worse than omitting it. SUBA-005 added eject/disable/enable/reset, G90 added
        // `steer` with `SubagentExecutor::control_steer`, and SUBA-046 added `grant-spawn-budget`
        // with `route_grant_spawn_budget` — at pi's own position for it (`shared/types.ts:1885`:
        // `… "reset", … "status", "grant-spawn-budget", "interrupt", …`). The `schedule*` family
        // and the rest of upstream's 53 stay out until their managers exist.
        let action_enum = props
            .get("action")
            .and_then(|a| a.get("enum"))
            .and_then(|e| e.as_array())
            .expect("action property carries an enum array");
        let action_values: Vec<&str> = action_enum.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(
            action_values,
            vec![
                // SUBA-055 added `guide` with `registration::guide::read_subagent_guide`, at pi's
                // own position for it: upstream reads `… "models", "children.list", "guide",
                // "create", …` (`shared/types.ts:2084` @v0.47.1). `children.list` is NOT ported —
                // it lists retained children under a `parentWorkflowRunId` that this build has no
                // concept of — so `guide` follows `models` directly here.
                "list", "get", "models", "guide", "create", "update", "delete", "eject", "disable",
                "enable", "reset", "status", "grant-spawn-budget", "interrupt", "resume", "steer",
                "stop", "dismiss", "append-step", "doctor", "mission.create", "mission.list",
                "mission.show", "mission.update", "mission.resolve-decision", "mission.attach-run",
                "mission.close", "watchdog.status", "watchdog.check", "watchdog.configure",
                "watchdog.recommend-model"
            ],
            "the action enum must be pi's SUBAGENT_ACTIONS in pi's own order, for the verbs cyrup \
             dispatches"
        );
        // Every `watchdog.*` verb the tool advertises must dispatch (`route_watchdog_action`), for
        // the same reason the management loop below checks its own family.
        for action in crate::watchdog::tool_actions::WATCHDOG_TOOL_ACTIONS {
            assert!(
                action_values.contains(&action),
                "watchdog action '{action}' is dispatched but not advertised in the tool schema"
            );
        }
        // Every advertised management verb must actually dispatch: an enum value the tool schema
        // shows the model but `route_action` answers with "unknown subagent action" is a worse
        // defect than the missing action was.
        for action in crate::discovery::management::MANAGEMENT_ACTIONS {
            assert!(
                action_values.contains(&action),
                "management action '{action}' is dispatched but not advertised in the tool schema"
            );
        }
        assert_eq!(props["action"]["type"], serde_json::json!("string"));

        // SUBA-079 / pi `extension/schemas.ts:319-322` @v0.57.0 — three values, `profile` included.
        assert_eq!(props["context"]["type"], serde_json::json!("string"));
        assert_eq!(
            props["context"]["enum"],
            serde_json::json!(["fresh", "fork", "profile"])
        );

        // Top-level numeric bounds pi pins.
        assert_eq!(props["concurrency"]["minimum"], serde_json::json!(1));
        assert_eq!(props["timeoutMs"]["minimum"], serde_json::json!(1));
        assert_eq!(props["maxRuntimeMs"]["minimum"], serde_json::json!(1));
        assert_eq!(props["index"]["minimum"], serde_json::json!(0));

        // tasks[] per-task fields the description advertises (output/outputMode/reads/progress),
        // plus count's minimum.
        let task_props = props["tasks"]["items"]["properties"]
            .as_object()
            .expect("tasks[].items has a properties object");
        for per_task in ["agent", "task", "count", "output", "outputMode", "reads", "progress"] {
            assert!(
                task_props.contains_key(per_task),
                "tasks[] items must carry the per-task field '{per_task}'"
            );
        }
        assert_eq!(task_props["count"]["minimum"], serde_json::json!(1));
        assert_eq!(task_props["progress"]["type"], serde_json::json!("boolean"));
        assert_eq!(props["tasks"]["items"]["required"], serde_json::json!(["agent", "task"]));

        // chain[] items must be an additionalProperties:false object with the flattened
        // sequential/parallel/dynamic surface (schemas.ts:190-229).
        let chain_item = &props["chain"]["items"];
        assert_eq!(chain_item["type"], serde_json::json!("object"));
        assert_eq!(chain_item["additionalProperties"], serde_json::json!(false));
        let chain_props = chain_item["properties"]
            .as_object()
            .expect("chain[].items has a properties object");
        for chain_field in ["agent", "parallel", "expand", "collect", "concurrency", "failFast", "worktree"] {
            assert!(
                chain_props.contains_key(chain_field),
                "chain[] items must carry '{chain_field}'"
            );
        }

        // config/output/skill/acceptance are provider-friendly anyOf unions (no bare top-level type).
        assert!(props["config"].get("anyOf").is_some(), "config must be an anyOf union");
        assert!(props["output"].get("anyOf").is_some(), "output must be an anyOf union");
        assert!(props["skill"].get("anyOf").is_some(), "skill must be an anyOf union");
        assert!(props["acceptance"].get("anyOf").is_some(), "acceptance must be an anyOf union");

        // SUBA-041: the control fragment is no longer inserted into the advertised schema (no
        // `resolveControlConfig`/notice pipeline in this port), but it is KEPT as the shape record
        // for whichever tier lands that subsystem — so its nested attention thresholds + notify
        // enums are still pinned here, against the fragment rather than against `props`.
        let control_fragment = sj_control_overrides();
        let control_props = control_fragment["properties"]
            .as_object()
            .expect("control has a properties object");
        assert_eq!(control_props["needsAttentionAfterMs"]["minimum"], serde_json::json!(1));
        assert_eq!(
            control_props["notifyOn"]["items"]["enum"],
            serde_json::json!(["active_long_running", "needs_attention"])
        );
        assert_eq!(
            control_props["notifyChannels"]["items"]["enum"],
            serde_json::json!(["event", "async", "intercom"])
        );

        // The multi-section description (extension/index.ts:461-495) — the substrings pi's own
        // tool-description executable spec pins (test/unit/tool-description.test.ts).
        let desc = SUBAGENT_TOOL_DESCRIPTION;
        for needle in [
            "use { action: \"list\" } to inspect configured agents/chains",
            "executable/non-disabled",
            "proactive skill subagent suggestions",
            "output?,reads?,progress?",
            "timeoutMs",
            "maxRuntimeMs",
            // Was `"only for foreground runs"` + `"omit for async/background runs"`, labelled
            // "pi-pinned". Neither string exists anywhere in upstream: `git grep "only for
            // foreground" v0.34.0 -- src/` returns NOTHING. They described cyrup's own former
            // refusal, not pi's contract, and pinning them made the test enforce the very
            // divergence SUBA-N03 removed. Upstream says the OPPOSITE, verbatim, in two places
            // (`extension/tool-description.ts:25` and `:73`), and this crate's description is now
            // byte-identical to `:25` — so that is what gets pinned.
            "for foreground and async/background runs",
        ] {
            assert!(
                desc.contains(needle),
                "the tool description must contain the pi-pinned substring {needle:?}"
            );
        }
        assert!(
            !desc.contains("disabled builtins"),
            "the description must NOT contain 'disabled builtins' (pi tool-description.test.ts pins its absence)"
        );
    }

    /// THE GUARD. Every property this tool advertises must actually be read somewhere outside
    /// `provided_keys()`.
    ///
    /// This defect class has now cost four separate fixes (SUBA-041, SUBA-N03, `control`/
    /// `includeProgress`, `chainDir`): a param is advertised in the schema, deserialized, and then
    /// silently eaten by a dispatch seam too narrow to carry it. Nothing failed, because
    /// `provided_keys()` touches every field precisely so the compiler's `dead_code` lint — the one
    /// automatic signal that would have flagged it — stays quiet. That is a real trade (the crate
    /// runs under `-D warnings` with no non-test `#[allow]`), but it costs the only free detector,
    /// so the detection has to be bought back explicitly. This is that purchase.
    ///
    /// It derives the advertised set from `subagent_tool_parameters()` itself rather than a
    /// hand-copied list, because a hand-copied list is exactly what encoded a fabricated
    /// "pi-pinned" substring in the sibling test above.
    #[test]
    fn every_advertised_schema_property_is_read_outside_provided_keys() {
        // Every module under `src/extension/`. The guard scans the WHOLE module tree rather than
        // one file: a property whose only read now lives in a sibling module still counts, and a
        // module added later is covered automatically instead of silently going unscanned — which
        // a hand-listed set of `include_str!` calls could not promise.
        let sources = {
            fn walk(dir: &std::path::Path, out: &mut String) {
                let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
                    .expect("the extension module tree must be readable")
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .collect();
                entries.sort();
                for path in entries {
                    if path.is_dir() {
                        walk(&path, out);
                    } else if path.extension().is_some_and(|ext| ext == "rs") {
                        out.push_str(
                            &std::fs::read_to_string(&path)
                                .expect("every module in the tree must be readable"),
                        );
                        out.push('\n');
                    }
                }
            }
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/extension");
            let mut out = String::new();
            walk(&root, &mut out);
            out
        };
        let src: &str = &sources;

        // Excise EVERY `fn provided_keys()` body — each one's whole purpose is to touch every
        // field, so leaving even one in makes this assertion vacuously true for the fields it
        // names.
        //
        // This used to excise only the FIRST (`SRC.find`), which left
        // `ToolTaskItem::provided_keys` in the scanned text — and that one alone vacuously
        // satisfied SEVEN top-level property names it shares with the outer schema (`task`, `cwd`,
        // `output`, `outputMode`, `model`, `skill`, `acceptance`). Those are exactly the widest,
        // most dispatch-heavy properties this guard exists to police, so the guard was blind
        // precisely where it mattered most. Excise-all is the fix; if a THIRD `provided_keys` is
        // ever added, it is covered automatically.
        let mut scanned = String::with_capacity(src.len());
        let mut rest = src;
        let mut excised = 0usize;
        while let Some(start) = rest.find("fn provided_keys") {
            scanned.push_str(&rest[..start]);
            let body_end = rest[start..]
                .find("\n    }")
                .expect("every provided_keys() must terminate at a method-level closing brace");
            rest = &rest[start + body_end..];
            excised += 1;
        }
        scanned.push_str(rest);
        assert!(
            !scanned.contains("fn provided_keys"),
            "no `fn provided_keys` may survive into the scanned text"
        );
        assert!(
            excised >= 2,
            "expected at least the two known `provided_keys()` bodies (SubagentToolParams and \
             ToolTaskItem) to be excised, excised {excised} — if one was renamed or removed, \
             update this guard rather than letting it silently scan less"
        );

        // A read is `.field` NOT followed by another identifier char, so `.id` does not match
        // `.identity` and `.index` does not match `.indexed`.
        fn reads_field(hay: &str, field: &str) -> bool {
            let needle = format!(".{field}");
            let mut from = 0;
            while let Some(i) = hay[from..].find(&needle) {
                let at = from + i;
                let after = hay[at + needle.len()..].chars().next();
                if !matches!(after, Some(c) if c.is_alphanumeric() || c == '_') {
                    return true;
                }
                from = at + needle.len();
            }
            false
        }

        let schema = subagent_tool_parameters();
        let props = schema["properties"]
            .as_object()
            .expect("the tool schema must expose an object of properties");

        let mut unwired: Vec<&str> = Vec::new();
        for name in props.keys() {
            let mut field = String::new();
            for ch in name.chars() {
                if ch.is_ascii_uppercase() {
                    field.push('_');
                    field.push(ch.to_ascii_lowercase());
                } else {
                    field.push(ch);
                }
            }
            if field == "async" {
                field = "r#async".to_string();
            }
            if !reads_field(&scanned, &field) {
                unwired.push(name.as_str());
            }
        }

        assert!(
            unwired.is_empty(),
            "ADVERTISED but never read outside provided_keys(), so a caller that sets one is \
             silently ignored: {unwired:?}\n\
             Wire it into dispatch, or stop advertising it. Do NOT satisfy this test by adding a \
             mention to provided_keys() — that is the exact move that hid `chainDir`."
        );
    }

    /// G90, the advertise-vs-DESCRIBE half of the crate's advertise-vs-dispatch invariant.
    ///
    /// `steer` is dispatchable: it is in `subagent_tool_parameters()`'s `action` enum, it has a
    /// real `route_control_action` arm, and it is not on the child-safe denylist — so a fanout
    /// child can call it. Every surface that TELLS a model about the action set must therefore
    /// name it, or the model is handed a verb with no description of how to address it. Four
    /// properties carry `action='steer'` upstream (`extension/schemas.ts:272,281,282,283`
    /// @v0.43.0) and the child-safe allowed list carries it too (`extension/fanout-child.ts:179`
    /// @v0.43.0, `:161` @v0.34.0); all five
    /// had silently dropped it.
    #[test]
    fn every_surface_that_describes_steer_actually_names_it() {
        let schema = subagent_tool_parameters();
        let props = schema["properties"]
            .as_object()
            .expect("the tool schema must expose an object of properties");

        // Preconditions — if either of these ever stops holding, the assertions below are about
        // the wrong thing and should be revisited rather than silently passing.
        assert!(
            props["action"]["enum"]
                .as_array()
                .expect("the action property must advertise an enum")
                .iter()
                .any(|v| v.as_str() == Some("steer")),
            "precondition: `steer` is an advertised action"
        );
        assert!(
            !crate::discovery::management::MUTATING_MANAGEMENT_ACTIONS.contains(&"steer"),
            "precondition: `steer` is NOT on the child-safe denylist, so a fanout child dispatches it"
        );

        for name in ["id", "runId", "dir", "message"] {
            let desc = props[name]["description"].as_str().unwrap_or_default();
            assert!(
                desc.contains("steer"),
                "property '{name}' is one of the four the `steer` verb is addressed through, so \
                 its description must name the verb (pi `extension/schemas.ts:224,227,230,238` \
                 @v0.34.0). Got: {desc}"
            );
        }

        let executor = Arc::new(SubagentExecutor::new());
        let child_safe = SubagentTool::new_child_safe(executor, PathBuf::from("/tmp"));
        assert!(
            Tool::description(&child_safe).contains("resume, steer, append-step"),
            "the child-safe allowed list must name `steer` in pi's own position between `resume` \
             and `append-step` (`fanout-child.ts:161` @v0.34.0). Got: {}",
            Tool::description(&child_safe)
        );
    }

    /// SUBA-041 (re-scoped from `single_mode_rejects_unwired_override_params_before_any_agent_resolution`,
    /// which pinned the pre-fix behavior of rejecting all NINE schema-advertised SINGLE-mode
    /// overrides): the params pi's `runSinglePath` honors must be ACCEPTED — a call carrying them
    /// proceeds past dispatch into agent resolution, so the only error left is the unresolvable
    /// agent — while any param the schema does NOT advertise must still be refused LOUDLY by name,
    /// never silently dropped.
    ///
    /// The `"ghost"` agent makes the two outcomes trivially distinguishable: `agent not found:
    /// ghost` proves the param got through dispatch; the named refusal proves it did not. Against
    /// pre-SUBA-041 code every one of the wired params produced the refusal instead, so this fails
    /// there.
    ///
    /// SUBA-N05 re-scoped it again rather than leaving it pinning stale behaviour: `control` was in
    /// the "refused" half and is now genuinely HONOURED (foreground via
    /// `SingleRunOverrides::control` → `resolve_control_config` → `RunOptions::control_config`,
    /// async via `RunnerConfig::control`), so it moved into the accepted half. The test name
    /// deliberately no longer encodes a COUNT — it encoded "seven"/"two" and went stale twice.
    ///
    /// SUBA-N06 emptied the refused half entirely: `includeProgress` is now HONOURED too
    /// (foreground via `SingleRunOverrides::include_progress` → `RunOptions::include_progress` →
    /// `run_sync`'s `SingleResult::progress` assembly, async via `RunnerConfig::include_progress`).
    /// Rather than delete the refusal leg, it is inverted — the loop below now asserts that NO
    /// advertised param is refused, over a table that is exactly the schema's own property list for
    /// the SINGLE-mode overrides, and a `REFUSED_UNCONDITIONALLY` table that must stay empty.
    #[tokio::test]
    async fn single_mode_accepts_every_wired_override_and_never_silently_drops_an_unwired_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = scoped_tool(dir.path()).await;

        // Every SINGLE-mode override this dispatcher wires: each must reach agent resolution.
        let accepted = [
            serde_json::json!({ "share": true }),
            serde_json::json!({ "sessionDir": "~/x" }),
            serde_json::json!({ "artifacts": false }),
            serde_json::json!({ "output": "report.md" }),
            serde_json::json!({ "output": "report.md", "outputMode": "file-only" }),
            serde_json::json!({ "skill": "rust,testing" }),
            serde_json::json!({ "acceptance": "checked" }),
            // SUBA-N05.
            serde_json::json!({ "control": { "enabled": true, "needsAttentionAfterMs": 1500 } }),
            // SUBA-N06 — both truthiness arms, since `run_sync` gates on `Some(true)` exactly.
            serde_json::json!({ "includeProgress": true }),
            serde_json::json!({ "includeProgress": false }),
        ];
        for (i, extra) in accepted.iter().enumerate() {
            let mut params = serde_json::json!({ "agent": "ghost", "task": "do it" });
            for (key, value) in extra.as_object().expect("object literal") {
                params
                    .as_object_mut()
                    .expect("object literal")
                    .insert(key.clone(), value.clone());
            }
            let err = tool
                .execute(
                    ToolCallId::from(format!("accepted-{i}").as_str()),
                    params.clone(),
                    CancelToken::new(),
                    Box::new(|_u: cyrup_core::ToolUpdate| {}),
                )
                .await
                .expect_err("the agent is unresolvable, so the call still errors");
            let message = err.to_string();
            assert!(
                message.contains("agent not found"),
                "{params} must be ACCEPTED at dispatch and fail only on agent resolution: {message}"
            );
            assert!(
                !message.contains("does not support"),
                "{params} must not be refused as an unsupported param: {message}"
            );
        }

        // The other half of the invariant: any param this dispatcher refuses UNCONDITIONALLY must
        // be named LOUDLY (never silently dropped) AND must be absent from the schema. SUBA-N06
        // emptied this table; it exists so that re-introducing a refusal is a deliberate, cited act
        // rather than a silent one. The loop still runs, and still proves both halves, for every
        // entry that is ever added.
        const REFUSED_UNCONDITIONALLY: &[(&str, serde_json::Value)] = &[];
        for (name, value) in REFUSED_UNCONDITIONALLY {
            let mut params = serde_json::json!({ "agent": "ghost", "task": "do it" });
            params
                .as_object_mut()
                .expect("object literal")
                .insert((*name).to_string(), value.clone());
            let message = tool
                .execute(
                    ToolCallId::from("refused"),
                    params,
                    CancelToken::new(),
                    Box::new(|_u: cyrup_core::ToolUpdate| {}),
                )
                .await
                .expect_err("a param with no subsystem behind it must be refused")
                .to_string();
            assert!(message.contains(name), "got: {message}");
            assert!(
                !message.contains("agent not found"),
                "the refusal must fire BEFORE agent resolution ever runs: {message}"
            );
            assert!(
                !subagent_tool_parameters()["properties"]
                    .as_object()
                    .expect("properties object")
                    .contains_key(*name),
                "'{name}' is refused unconditionally, so the schema must not advertise it"
            );
        }
        assert!(
            REFUSED_UNCONDITIONALLY.is_empty(),
            "as of SUBA-N06 no advertised SINGLE-mode param is refused outright; an entry here \
             needs an upstream citation justifying the refusal"
        );

        // A malformed `acceptance` policy is refused up front with pi's own
        // `validateAcceptanceInput` message (`acceptance.ts:181`), not swallowed.
        let bad_acceptance = tool
            .execute(
                ToolCallId::from("bad-acceptance"),
                serde_json::json!({ "agent": "ghost", "task": "do it", "acceptance": "nonsense" }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("an invalid acceptance level must be refused")
            .to_string();
        assert!(
            bad_acceptance.contains("acceptance has invalid level 'nonsense'."),
            "pi's verbatim validation message: {bad_acceptance}"
        );
        assert!(
            !bad_acceptance.contains("agent not found"),
            "acceptance validation must precede agent resolution: {bad_acceptance}"
        );
    }

    /// SUBA-041, the OTHER half of the contract, RE-SCOPED by SUBA-N03.
    ///
    /// This test used to pin the opposite behaviour, under the name
    /// `a_background_single_run_refuses_the_six_foreground_only_overrides_by_name`: it asserted
    /// that a background SINGLE run REFUSED `output`/`outputMode`/`skill`/`share`/`sessionDir`/
    /// `artifacts` by name (and, one guard above them, `timeoutMs`/`maxRuntimeMs`). It was a
    /// correct pin of the behaviour that existed, and it is rewritten rather than deleted because
    /// the contract it guards — "this schema must never advertise a param the router drops" — is
    /// unchanged; only the side of it that is true has flipped.
    ///
    /// What changed: the refusal's stated justification was a FABRICATED upstream citation ("pi's
    /// own timeoutMs + async refusal, `subagent-executor.ts:3022`" — which at v0.34.0 is foreground
    /// intercom-receipt construction, and no such refusal exists anywhere in v0.34.0 `src/`), and
    /// the real reason underneath it — a second-hop `RunnerConfig` narrower than the foreground
    /// `RunOptions` — has been closed. All NINE advertised SINGLE-mode overrides plus the timeout
    /// now reach hop 2.
    ///
    /// This test therefore asserts the complement of what it used to: for each param, a background
    /// SINGLE call carrying it is NOT refused by any foreground-only gate. The unresolvable agent
    /// name means every call still errors — with `agent not found`, which is proof the call got
    /// PAST the router and into agent resolution rather than being turned away at the gate. Its
    /// companions
    /// [`tests::a_background_single_run_honours_the_nine_single_mode_overrides`] and
    /// [`tests::a_background_single_run_carries_the_timeout_and_deadline_into_the_runner_config`]
    /// then prove the params really arrive at the detached runner, not merely that they are
    /// accepted — an accepted-and-dropped param is precisely the defect SUBA-041 exists to prevent,
    /// and "no longer refused" alone would not distinguish the two.
    #[tokio::test]
    async fn a_background_single_run_no_longer_refuses_the_formerly_foreground_only_overrides() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = scoped_tool(dir.path()).await;

        // The exact six the removed gate named, plus the timeout pair its sibling guard named, plus
        // `acceptance`/`control`/`includeProgress` (freed by SUBA-N04/N05/N06) so all nine
        // advertised SINGLE-mode overrides are covered in one place.
        let cases = [
            ("output", serde_json::json!({ "output": "report.md" })),
            ("outputMode", serde_json::json!({ "outputMode": "inline" })),
            ("skill", serde_json::json!({ "skill": "rust" })),
            ("share", serde_json::json!({ "share": true })),
            ("sessionDir", serde_json::json!({ "sessionDir": "~/x" })),
            ("artifacts", serde_json::json!({ "artifacts": false })),
            ("acceptance", serde_json::json!({ "acceptance": "checked" })),
            ("control", serde_json::json!({ "control": { "needsAttentionAfterMs": 5000 } })),
            ("includeProgress", serde_json::json!({ "includeProgress": true })),
            ("timeoutMs", serde_json::json!({ "timeoutMs": 60_000 })),
            ("maxRuntimeMs", serde_json::json!({ "maxRuntimeMs": 60_000 })),
        ];

        for (name, extra) in &cases {
            let mut params =
                serde_json::json!({ "agent": "ghost", "task": "do it", "async": true });
            for (key, value) in extra.as_object().expect("object literal") {
                params
                    .as_object_mut()
                    .expect("object literal")
                    .insert(key.clone(), value.clone());
            }
            let message = tool
                .execute(
                    ToolCallId::from(format!("bg-{name}").as_str()),
                    params.clone(),
                    CancelToken::new(),
                    Box::new(|_u: cyrup_core::ToolUpdate| {}),
                )
                .await
                .expect_err("the agent is unresolvable, so every call here still errors")
                .to_string();
            assert!(
                !message.contains("only supported for foreground"),
                "'{name}' must no longer be refused on the background path: {message}"
            );
            // The positive half: the call reached agent resolution, which is strictly PAST the
            // router. Without this, the assertion above would also pass if the router had started
            // rejecting these calls with some different message.
            assert!(
                message.contains("agent not found"),
                "'{name}' must fall through to agent resolution like any other background run, \
                 not be turned away at the router: {message}"
            );
            // The schema/behaviour invariant this test has always guarded, restated in its new
            // direction: an honoured param MUST be advertised.
            assert!(
                subagent_tool_parameters()["properties"]
                    .as_object()
                    .expect("properties object")
                    .contains_key(*name),
                "'{name}' is honoured on both paths, so the schema must advertise it"
            );
        }
    }

    /// SUBA-043 + SUBA-047 — the two capabilities that were implemented and unadvertised.
    ///
    /// THE USER ACTION: an orchestrator calls `subagent({agent, task, outputSchema:{…}})` to get
    /// typed JSON back, or `subagent({agent, task, toolBudget:{hard:3}})` to bound one delegation's
    /// tool spend. Both were accepted (the root schema is `additionalProperties: true` and
    /// `SubagentToolParams` has no `deny_unknown_fields`), both were dropped without error, and both
    /// capabilities were fully built underneath — `outputSchema` by SUBA-S01, `toolBudget` by
    /// SUBA-007. Reachable only by wrapping a one-item `tasks:[…]`, or by editing the agent file.
    ///
    /// Asserted at the surface: advertised, deserialized, and — via the sibling guard
    /// [`tests::every_advertised_schema_property_is_read_outside_provided_keys`] — consumed.
    #[test]
    fn output_schema_and_tool_budget_are_advertised_and_deserialized_at_the_top_level() {
        let props = subagent_tool_parameters()["properties"]
            .as_object()
            .expect("properties object")
            .clone();
        for name in ["outputSchema", "toolBudget"] {
            assert!(
                props.contains_key(name),
                "'{name}' is honoured on both single paths, so it must be advertised"
            );
        }
        // pi `extension/schemas.ts:116-120`: `hard` is REQUIRED and the object is closed.
        assert_eq!(props["toolBudget"]["required"], serde_json::json!(["hard"]));
        assert_eq!(props["toolBudget"]["additionalProperties"], serde_json::json!(false));
        // pi gives the top-level `outputSchema` no description of its own, and the shape is the
        // same open `JsonSchemaObject` the `tasks[]` item schema already used.
        assert_eq!(props["outputSchema"], sj_json_schema_object());

        let parsed: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "agent": "x",
            "task": "y",
            "outputSchema": { "type": "object", "properties": { "n": { "type": "number" } }, "required": ["n"] },
            "toolBudget": { "hard": 3 }
        }))
        .expect("params parse");
        assert_eq!(
            parsed.output_schema,
            Some(serde_json::json!({
                "type": "object",
                "properties": { "n": { "type": "number" } },
                "required": ["n"]
            })),
            "the schema must survive deserialization, not be eaten by the permissive parse"
        );
        assert_eq!(parsed.tool_budget, Some(serde_json::json!({ "hard": 3 })));
        // `provided_keys()` is what the dispatch surface reports as "you supplied these", so both
        // must appear there too or an unknown-action error would still not explain the failure.
        let keys = parsed.provided_keys();
        assert!(keys.contains(&"outputSchema"), "{keys:?}");
        assert!(keys.contains(&"toolBudget"), "{keys:?}");
    }

    /// SUBA-008 — the `turnBudget` param must be advertised in upstream's exact schema shape AND
    /// survive the permissive parse, or the enforcement in `exec/turn_budget.rs` is unreachable
    /// from the tool surface. Landed together with that enforcement, never ahead of it (SUBA-047's
    /// lesson: an advertised-but-unconsumed param is the defect, not the fix).
    #[test]
    fn turn_budget_is_advertised_with_upstreams_schema_and_deserializes_at_the_top_level() {
        let props = subagent_tool_parameters()["properties"]
            .as_object()
            .expect("properties object")
            .clone();
        assert!(props.contains_key("turnBudget"), "the enforced param must be advertised");
        // pi `extension/schemas.ts:104-107` @v0.43.0: `maxTurns` REQUIRED, `graceTurns` optional
        // and >= 0 (NOT >= 1 — a zero grace is legal and means "abort at maxTurns"), object closed.
        assert_eq!(props["turnBudget"]["required"], serde_json::json!(["maxTurns"]));
        assert_eq!(props["turnBudget"]["additionalProperties"], serde_json::json!(false));
        assert_eq!(props["turnBudget"]["properties"]["maxTurns"]["minimum"], serde_json::json!(1));
        assert_eq!(props["turnBudget"]["properties"]["graceTurns"]["minimum"], serde_json::json!(0));
        assert_eq!(
            props["turnBudget"]["description"],
            serde_json::json!(
                "Optional assistant-turn budget. At maxTurns the child is asked to wrap up; after graceTurns additional assistant turns it is aborted and partial output is returned."
            )
        );

        let parsed: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "agent": "x",
            "task": "y",
            "turnBudget": { "maxTurns": 4, "graceTurns": 2 }
        }))
        .expect("params parse");
        assert_eq!(
            parsed.turn_budget,
            Some(serde_json::json!({ "maxTurns": 4, "graceTurns": 2 }))
        );
        assert!(parsed.provided_keys().contains(&"turnBudget"), "{:?}", parsed.provided_keys());
    }

}
