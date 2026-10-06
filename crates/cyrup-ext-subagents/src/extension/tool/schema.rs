//! The advertised JSON-Schema for the `subagent` tool's parameters, built from one `sj_*`
//! fragment per property.

use crate::disabled_features::{DisabledFeatureSurface, SubagentFeature, SubagentSurfaceFeature};
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
        // SUBA-151 — pi added `minProperties: 1` to this object in `71d042f0` (#2596, v0.74.0,
        // `extension/schemas.ts`). The BEHAVIOUR was already here — dispatch refuses an empty
        // block with `usageBudget must include tokens or costUsd.`
        // ([`crate::exec::usage_budget::validate_usage_budget_config`], pi's own
        // `validateUsageBudgetConfig`) — but the ADVERTISED schema admitted `{}`, and this crate's
        // rule is that the schema and the dispatch agree. Without it a model is told `{}` is a
        // legal argument and then refused for sending it.
        "minProperties": 1,
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
        // SUBA-151 — pi's v0.73.0 narrowing (`extension/schemas.ts:104` @v0.75.0): the description
        // STATES `soft <= hard`, which `validate_tool_budget_config` already refuses by name
        // (`exec/tool_budget.rs:171`, `toolBudget.soft must be <= toolBudget.hard.`). Same class
        // of defect as the empty `usageBudget` below: a bound the dispatcher enforces and the
        // schema does not state is a refusal the model could not have avoided.
        "description": "Optional child tool-call budget; soft <= hard. soft nudges the child; after hard, block tools (default read/grep/find/ls, or '*' for all tools) are blocked so the child can finalize."
    })
}

/// `TaskItem` (`schemas.ts:78-90`): one top-level PARALLEL `tasks[]` element (agent+task required).
/// # SUBA-152 — `agent` carries `minLength: 1`; this item does NOT carry `additionalProperties: false`
///
/// `minLength: 1` closes the same defect [`sj_usage_budget_override`]'s `minProperties: 1` note
/// describes. THE DEFECT: the schema called `{"tasks": [{"agent": "", "task": "x"}]}` legal and the
/// dispatcher refuses it — `agent not found: ` — so a model was told an empty agent name was a
/// legal way to name a child and got a discovery error with nothing in it to read back. Upstream's
/// structured task item declares the same `{ type: "string", minLength: 1 }`
/// (`extension/schemas.ts:287`). Note what it does NOT cover: a whitespace-only `"   "` satisfies
/// `minLength: 1` and is refused identically (`agent not found:    `). That gap is upstream's too —
/// its constraint is also a bare `minLength: 1` — and closing it would need a `pattern`, which is
/// not what this change ports.
///
/// **`additionalProperties: false` is deliberately absent, and that is a measurement rather than an
/// omission.** [`sj_chain_item`] carries it, which makes this item look inconsistent; the
/// asymmetry is correct, for two independent reasons.
///
/// 1. **The dispatcher does not refuse an unknown key here**, so the defect class does not apply.
///    [`crate::extension::tool::task_items::ToolTaskItem`] is a plain `serde::Deserialize` with no
///    `deny_unknown_fields`: `{"tasks": [{"agent": "ghost", "task": "x", "bogusKey": 1}]}` parses,
///    `bogusKey` is dropped, and the call then fails on agent resolution with the *identical*
///    `agent not found: ghost` a call without `bogusKey` gets. There is no refusal to advertise.
/// 2. **It would advertise the inverse defect.** This item declares THIRTEEN properties;
///    `ToolTaskItem` parses EIGHTEEN. The five it does not declare are `as`, `outputSchema`,
///    `phase`, `label` and `worktree` — and two of those are not inert: `tool_task_to_spec`
///    (`task_items.rs:345-346`) threads `outputSchema` onto `structured_output_schema` and `as`
///    onto the step's named `output`, so both reach the child from a `tasks[]` element today.
///    `additionalProperties: false` would therefore tell the model that two keys the dispatcher
///    accepts and USES are illegal — the schema refusing what dispatch admits, which is the exact
///    mirror of the defect this change exists to close, and a strictly worse trade than the
///    cosmetic inconsistency with [`sj_chain_item`].
///
/// [`sj_chain_item`] is in a different position precisely because it declares every key its parse
/// honours (`as`, `outputSchema`, `phase`, `label`, `worktree` included), which is why
/// `additionalProperties: false` is safe there and not here. Advertising this item's five missing
/// properties first would make the constraint portable; that is a separate change, and it must come
/// first — `additionalProperties: false` on the current thirteen is not a narrowing, it is a
/// regression.
fn sj_task_item() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["agent", "task"],
        "properties": {
            "agent": { "type": "string", "minLength": 1 },
            "task": { "type": "string" },
            "cwd": { "type": "string" },
            "machine": { "type": "string", "minLength": 1, "maxLength": 128, "description": "Herdr saved machine id or label." },
            "count": { "type": "integer", "minimum": 1 },
            "output": sj_output_override(),
            "outputMode": sj_output_mode(),
            "reads": sj_reads_override(),
            "progress": { "type": "boolean" },
            "model": { "type": "string" },
            "fast": { "type": "boolean", "description": "Opt into priority service tier for supported native OpenAI-Codex child models. This can increase quota or cost." },
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
            "machine": { "type": "string", "minLength": 1, "maxLength": 128, "description": "Herdr saved machine id or label." },
            "count": { "type": "integer", "minimum": 1 },
            "output": sj_output_override(),
            "outputMode": sj_output_mode(),
            "reads": sj_reads_override(),
            "progress": { "type": "boolean" },
            "skill": sj_skill_override(),
            "model": { "type": "string" },
            "fast": { "type": "boolean", "description": "Opt into priority service tier for supported native OpenAI-Codex child models. This can increase quota or cost." },
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
            "machine": { "type": "string", "minLength": 1, "maxLength": 128, "description": "Herdr saved machine id or label." },
            "output": sj_output_override(),
            "outputMode": sj_output_mode(),
            "reads": sj_reads_override(),
            "progress": { "type": "boolean" },
            "skill": sj_skill_override(),
            "model": { "type": "string" },
            "fast": { "type": "boolean", "description": "Opt into priority service tier for supported native OpenAI-Codex child models. This can increase quota or cost." },
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
            "machine": { "type": "string", "minLength": 1, "maxLength": 128, "description": "Herdr saved machine id or label." },
            "output": sj_output_override(),
            "outputMode": sj_output_mode(),
            "reads": sj_reads_override(),
            "progress": { "type": "boolean" },
            "skill": sj_skill_override(),
            "model": { "type": "string" },
            "fast": { "type": "boolean", "description": "Opt into priority service tier for supported native OpenAI-Codex child models. This can increase quota or cost." },
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

/// The `action` description's feature-independent half — pi's *"Management/control only; omit for
/// execution."* (`extension/schemas.ts:163` and `:289` @v0.75.0, both sentences), in this port's
/// own wording.
const ACTION_DESCRIPTION_BASE: &str = "Management/control action. Omit for execution mode.";

/// The `action` description's `workflow-scripts`-owned half: the ONE sentence pi's
/// `createSubagentParamsSchema` reduction removes (`extension/schemas.ts:164` carries it,
/// `:289` does not), because `validate` is the `workflow-scripts` group's only action
/// ([`crate::disabled_features::SUBAGENT_FEATURES`]) and a disabled group's verb must not be
/// described as accepting anything.
const ACTION_VALIDATE_CLAUSE: &str = " validate accepts workflow: true or a script path.";

pub(crate) fn subagent_tool_parameters() -> serde_json::Value {
    // Built via per-property inserts rather than one giant `json!` literal: a single 33-property
    // `json!` object overflows the macro's default `recursion_limit` at expansion time. Each insert
    // below is its own shallow `json!` invocation, and the root wrapper is a 3-key `json!`.
    let mut props = serde_json::Map::new();
    props.insert("agent".to_string(), serde_json::json!({ "type": "string", "description": "Agent name (SINGLE mode) or target for management get/update/delete" }));
    props.insert("task".to_string(), serde_json::json!({ "type": "string", "description": "Task (SINGLE mode, optional for self-contained agents)" }));
    // WORKFLOW_2 — pi `extension/schemas.ts:223-226` @v0.75.0, description TRIMMED to what this
    // build actually offers. An advertised capability that refuses is worse than an unadvertised
    // one.
    //
    // SUBA-150 — pi `0538e14d` (#2588, `feat(workflows)!`, v0.74.0) DELETED `workflowScript` and
    // `workflowScriptPath` from the tool and put one `workflow` field in their place
    // (`extension/schemas.ts:223-226` @v0.75.0). The union is advertised as upstream advertises
    // it — `anyOf: [boolean, string minLength 1]` — and the description names all three forms
    // plus the `false invalid` rule, because a model that cannot tell a path from a resource name
    // writes script text into the string (the defect upstream's `scriptTextHint` exists for).
    //
    // `workflowScript` is NOT advertised any more; it survives as the INTERNAL carrier, exactly as
    // upstream keeps it (`disabled-features.ts:106-110` names it such). The boundary
    // ([`SubagentTool::lower_workflow_field`]) rewrites `workflow` onto it and REFUSES either
    // removed parameter by name, so the advertise-vs-dispatch invariant holds in both directions:
    // nothing advertised is undispatched, and nothing unadvertised is quietly accepted.
    //
    // What the description does NOT carry, and why — carried over from the `workflowScript`
    // description this replaces, with one correction.
    //
    // * Upstream's "Normally async unless asyncByDefault:false" is absent because this build runs
    //   workflows in the FOREGROUND only; `route_workflow_mode` refuses `async: true` outright.
    //   That is a CAPABILITY THIS BUILD LACKS, not a delta — it is named here so the omission is
    //   not read as a style choice.
    // * Mission `state` is absent because the guest surface genuinely OMITS it without a bound
    //   mission (`js/prelude.js:578` in `cyrup-workflow-runtime`, from `options.state.is_some()`),
    //   so advertising it unconditionally would describe a member the model cannot discover.
    // * `runs.host` is absent from the globals list for NOISE, not because it is ungranted: the
    //   previous revision of this comment claimed this build "grants neither state nor runs.host",
    //   which is false — `extension/executor/workflow.rs`'s `supports_host` is unconditionally
    //   `true`, so every workflow script here can call `runs.host`. Upstream registers that op
    //   only for resource-provenance runs; see the SUBA-150 report for the gap that opens.
    // * `runs.steer` is likewise omitted for noise, and that one is present-and-refusing rather
    //   than absent (`prelude.js:288-309` installs it; the engine refuses it).
    props.insert("workflow".to_string(), serde_json::json!({
        "anyOf": [ { "type": "boolean" }, { "type": "string", "minLength": 1 } ],
        "description": "true: run the one ```js workflow block written in this same reply (false \
            invalid). String with '/': script file read from request cwd. Other string: named \
            workflow resource. Script globals: runs.run, runs.all, runs.lanes, runs.ref, \
            runs.refs, runs.status, emit, console, and standard JavaScript only — no filesystem, \
            shell, Pi tools, or host globals. Use explicit return and top-level await; nested \
            async function, arrow, and method helpers are rejected. Runs in the foreground: omit \
            async or pass async:false. Cannot combine with agent, tasks, chain, or action (except \
            action:'validate')."
    }));
    props.insert(
        "action".to_string(),
        serde_json::json!({
            "type": "string",
            // G77: `stop` sits between `steer` and `append-step`, upstream's own position in
            // `SUBAGENT_ACTIONS` (`shared/types.ts:1885` @v0.43.0: `… "interrupt", "resume", "steer",
            // "stop", "append-step", …`). Advertised together with its `route_control_action` dispatch
            // arm (`SubagentExecutor::control_stop`) in this same change, per the crate's
            // advertise-vs-dispatch invariant.
            // SUBA-038: derived from [`SUBAGENT_ACTIONS`], not hand-written — a hand-written copy is
            // exactly what let the two unknown-action messages drift away from what dispatches.
            "enum": SUBAGENT_ACTIONS,
            // SUBA-150 — pi `:184` @v0.75.0 names what `validate` accepts, and names it as the
            // `workflow` field rather than the two deleted parameters.
            // SUBA-151 — composed from the two halves rather than written out, so
            // [`subagent_tool_parameters_for`]'s reduced form (which drops the `validate` clause,
            // upstream's ONLY delta between `:162-164` and `:289` @v0.75.0) cannot word the
            // surviving half differently from this one.
            "description": format!("{ACTION_DESCRIPTION_BASE}{ACTION_VALIDATE_CLAUSE}")
        }),
    );
    // SUBA-104 — pi `capabilities` (`extension/schemas.ts:287` @v0.68.0), right after `action`
    // as upstream, description VERBATIM.
    props.insert("capabilities".to_string(), serde_json::json!({ "type": "boolean", "description": "list: compact capability rows/details without system prompts." }));
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
    // SCOPE_19/A1 [CYRUP-DELTA] — the description no longer scopes `thinking` to
    // `watchdog.configure`: the launch path honours it (rung 2 of the resolution ladder in
    // `SubagentExecutor::run_foreground_impl`) and omitting it inherits the live parent session's
    // level via `SubagentExecutor::remembered_parent_thinking`. Upstream never wired this param
    // into launches at all; cyrup does, so the schema says so.
    props.insert("thinking".to_string(), serde_json::json!({ "anyOf": [{ "type": "string" }, { "type": "boolean", "enum": [false] }], "description": "Reasoning level for the single-agent child (off/minimal/low/medium/high/xhigh/max, or false for off), like model. Omit it: the child inherits this session's level, which keeps model and effort aligned so the shared prompt prefix stays cacheable. Set it only to deliberately spend more or less reasoning than the parent; a model ':level' suffix wins over this parameter. Also configures action='watchdog.configure'." }));
    props.insert("id".to_string(), serde_json::json!({ "type": "string", "description": "Run id or prefix for action='status', action='interrupt', action='stop', action='resume', action='steer', or action='append-step'." }));
    // SCOPE_19/B [CYRUP-DELTA] — `runId` defers to `id` and now says so structurally: it carries
    // `deprecated: true` and one pointer line instead of a description longer than the property it
    // defers to. Dispatch still accepts it everywhere `id` is accepted.
    props.insert("runId".to_string(), serde_json::json!({ "type": "string", "deprecated": true, "description": "Deprecated alias of id for action='interrupt', action='stop', action='resume', action='steer', or action='append-step'; still accepted. Prefer id." }));
    props.insert("dir".to_string(), serde_json::json!({ "type": "string", "description": "Async run directory for action='status', action='stop', action='resume', or action='steer'." }));
    props.insert("index".to_string(), serde_json::json!({ "type": "integer", "minimum": 0, "description": "Zero-based child index for actions that target a specific child or transcript." }));
    // VL-S6 — pi `extension/schemas.ts:318` @v0.68.0, description VERBATIM. Advertised in the
    // same change that gives `route_action` its `inspector.*` and `project.*` arms, both of which
    // read it (`SubagentToolParams::focus` → `InspectorRequest::focus` /
    // `ProjectPaneParams::focus`), so `every_advertised_schema_property_is_read_outside_provided_keys`
    // has a real read to find.
    props.insert("focus".to_string(), serde_json::json!({ "type": "boolean", "description": "Focus inspector.open/project.open pane." }));
    // SUBA-087 — pi `extension/schemas.ts:306` @v0.64.0, description VERBATIM. Advertised because
    // the `stop` dispatch arm threads it into `control_stop`'s resolver in this same change.
    props.insert("childId".to_string(), serde_json::json!({ "type": "string", "minLength": 1, "maxLength": 256, "description": "Stable child identity for child-scoped stop requests." }));
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
    // no enum, no description.
    // SCOPE_19/B [CYRUP-DELTA] — a description is added anyway: a caller-facing property with no
    // description is a question the schema forces the model to spend a call answering. The valid
    // set stays the unknown-topic message's job (`registration::guide::read_subagent_guide`).
    props.insert("topic".to_string(), serde_json::json!({ "type": "string", "description": "Guide topic for action='guide'. Omitted: 'overview' (the packaged README)." }));
    props.insert("message".to_string(), serde_json::json!({ "type": "string", "description": "Follow-up message for action='resume' or non-terminal guidance for action='steer'. Use index to choose a child from multi-child runs." }));
    // SUBA-049 — pi `extension/schemas.ts:283` @v0.43.0, description VERBATIM. Advertised together
    // with its consumer in this same change (`SteerDeliveryMode` is read by `control_steer`, written
    // onto the `SteerRequest`, and honoured by the child-side inbox), per the crate's
    // advertise-vs-dispatch invariant.
    // LANES_2 — the `mode` enum is WIDENED to upstream's five (`extension/schemas.ts:313`
    // @v0.68.0: `["steer", "follow_up", "auto", "plan", "apply"]`), because `worktree.cleanup`
    // addresses this same property and a model told the verb requires `mode='plan'` but shown a
    // three-value enum that excludes `plan` cannot call it at all.
    //
    // Widening is safe for `steer`, and that was checked rather than assumed:
    // `SteerDeliveryMode::parse` (`background/control.rs`) is a CLOSED three-arm match that
    // returns `None` for anything else, so `action='steer'` with `mode='plan'` is still refused
    // at the boundary with its own sentence. The description carries both verbs, as upstream's
    // does.
    props.insert("mode".to_string(), serde_json::json!({
        "type": "string",
        "enum": ["steer", "follow_up", "auto", "plan", "apply"],
        "description": "Delivery mode for action='steer'. steer interrupts at the next safe point (default), follow_up waits for the next turn boundary, and auto follows up mid-turn but delivers immediately between turns. worktree.cleanup supports plan only, no apply/removal."
    }));
    // LANES_2 — the three properties `worktree.cleanup` is addressed through
    // (`extension/schemas.ts:298-300` @v0.68.0, descriptions VERBATIM), advertised in the same
    // change that gives `route_action` its dispatch arm.
    //
    // `handoffPath` is shared with the `lane.*` and `worktree.discard` verbs a sibling change
    // lands; it is advertised here because this arm already reads it (pi `:6228`) and
    // `every_advertised_schema_property_is_read_outside_provided_keys` below would otherwise have
    // nothing to find.
    //
    // `planId` is advertised although the dispatch REFUSES every value: that is upstream's own
    // shape (`schemas.ts:300` says "Reserved; cleanup is plan-only.") and it is the honest one —
    // a model that tries to apply a saved plan gets pi's sentence explaining that apply does not
    // exist yet, instead of a schema rejection it cannot act on.
    props.insert("handoffPath".to_string(), serde_json::json!({ "type": "string", "description": "Existing manifest for worktree/lane actions." }));
    props.insert("repo".to_string(), serde_json::json!({ "type": "string", "description": "worktree.cleanup repo; default cwd." }));
    props.insert(
        "planId".to_string(),
        serde_json::json!({ "type": "string", "description": "Reserved; cleanup is plan-only." }),
    );
    // LANES_2 — the four properties the `worktree.discard` / `lane.*` verbs are addressed
    // through (`extension/schemas.ts:301-303,355` @v0.68.0, descriptions VERBATIM), advertised in
    // the same change that gives `route_action` its arms for them.
    //
    // `merge`/`supersession` are open objects at THIS layer, exactly as upstream declares them
    // (`Type.Unsafe({type:"object", additionalProperties:true})`): the per-field contract belongs
    // to the recorder, whose rejections are sentences a model can act on. Advertising a closed
    // object here would move those rejections into the schema validator, which answers with a
    // path and a keyword rather than "merged tree equivalence was not attested".
    props.insert(
        "laneId".to_string(),
        serde_json::json!({
            "type": "string",
            "minLength": 1,
            "maxLength": 128,
            "description": "Exact manifest run id for lane actions."
        }),
    );
    props.insert("merge".to_string(), serde_json::json!({
        "type": "object",
        "additionalProperties": true,
        "description": "lane.recordMerge evidence: prNumber, reviewedHead, mergeCommit, treeEquivalent, postMergeChecks, attestedBy, attestedAt."
    }));
    props.insert(
        "supersession".to_string(),
        serde_json::json!({
            "type": "object",
            "additionalProperties": true,
            "description": "lane.recordSupersession evidence: supersededBy, attestedBy, attestedAt."
        }),
    );
    // pi `:355` -> the `WorkflowLaneMetadata` TypeBox at `:103-110`. The property is advertised
    // with its real structure because cyrup already HAS the validated record
    // (`crate::workflows::WorkflowLaneMetadata`) and its normalizer, so the schema and the
    // validator cannot drift; the normalizer still answers, with upstream's own sentences, at the
    // dispatch boundary.
    props.insert("lane".to_string(), serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["version", "key"],
        "properties": {
            "version": { "type": "integer", "enum": [1] },
            "key": { "type": "string", "minLength": 1, "maxLength": 128 },
            "mode": { "type": "string", "enum": ["mutation", "review", "scout", "gate"] },
            "sourceRef": { "type": "string", "minLength": 1, "maxLength": 128 },
            "claims": { "type": "array", "maxItems": 20, "items": { "type": "string", "minLength": 1, "maxLength": 160 } },
            "outputPaths": { "type": "array", "maxItems": 10, "items": { "type": "string", "minLength": 1, "maxLength": 256 } }
        },
        "description": "Launch-declared lane metadata: key, mode, sourceRef, claims, outputPaths."
    }));
    props.insert("chainName".to_string(), serde_json::json!({ "type": "string", "description": "Chain name for get/update/delete management actions" }));
    props.insert("config".to_string(), serde_json::json!({
        "anyOf": [ { "type": "object", "additionalProperties": true }, { "type": "string" } ],
        "description": "Agent/chain config for create/update. Object or JSON string; presence of steps creates a chain."
    }));
    // SUBA-152 — `minItems: 1`. THE DEFECT: the schema called `tasks: []` legal and the dispatcher
    // refuses it. Mode is selected by a NON-EMPTY array, so `{"tasks": []}` falls through to
    // "Provide exactly one mode. Agents: …" rather than running a vacuous 0/0 parallel group
    // (`subagent_tool_rejects_empty_tasks_and_chain_arrays_as_no_mode_selected`,
    // `routing_tests.rs:750-770`). Same class as [`sj_usage_budget_override`]'s `minProperties: 1`:
    // the model was told an empty array was a legal way to ask for PARALLEL mode and got a
    // mode-selection error it could not have predicted from the schema. Upstream carries
    // `minItems: 1` on its own reduced pair (`extension/schemas.ts:291` for `tasks`, `:295` for
    // `chain`), though that pair is a different object from this one — see
    // [`subagent_tool_parameters_for`]'s note 4 — so the dispatcher, not upstream, is the authority
    // for porting it here.
    props.insert("tasks".to_string(), serde_json::json!({
        "type": "array",
        "minItems": 1,
        "items": sj_task_item(),
        "description": "PARALLEL mode: [{agent, task, count?, output?, outputMode?, reads?, progress?}, ...]"
    }));
    props.insert("concurrency".to_string(), serde_json::json!({ "type": "integer", "minimum": 1, "description": "Top-level PARALLEL mode only: max concurrent tasks. Defaults to config.parallel.concurrency or 4." }));
    props.insert("worktree".to_string(), serde_json::json!({ "type": "boolean", "description": "Create isolated git worktrees for parallel tasks; requires clean git state." }));
    // SUBA-152 — `minItems: 1`, for the reason the `tasks` entry above records: `{"chain": []}` is
    // refused by the same dispatcher check and the same assertion.
    props.insert("chain".to_string(), serde_json::json!({
        "type": "array",
        "minItems": 1,
        "items": sj_chain_item(),
        "description": "CHAIN mode: sequential steps; each result becomes {previous}. append-step takes one tail step and may use {chain_dir}/{outputs.name}."
    }));
    props.insert("context".to_string(), serde_json::json!({
        "type": "string",
        "enum": ["fresh", "fork", "profile"],
        "description": "'fresh' or 'fork' to branch from parent session, or 'profile' to require the selected agent's declared defaultContext. Explicit fresh/fork overrides every child; profile ignores config defaultSubagentContext and fails when an agent has no defaultContext. If omitted, config defaultSubagentContext wins over each agent defaultContext; implicit fork needs a persisted parent session and leaf, else fresh."
    }));
    props.insert("chainDir".to_string(), serde_json::json!({ "type": "string", "description": "Persistent chain artifact directory; defaults to user-scoped temp storage." }));
    // SCOPE_19/§4.1 [CYRUP-DELTA] — `async` states the completion contract the runtime message
    // (`extension/host/slash_render.rs`'s `format_async_started_message`) only reveals AFTER the
    // caller has already decided. The delivery mechanism is real (completion watcher → session
    // notification), so the schema documents it at decision time.
    props.insert("async".to_string(), serde_json::json!({ "type": "boolean", "description": "Run in background (default: false, or per config). On completion a summary notification is delivered into this session automatically; end your turn instead of polling (use the wait tool only when this turn must block)." }));
    // SUBA-N03: pi's VERBATIM descriptions (`extension/schemas.ts:265-266` @v0.34.0) read
    // "…Alias of maxRuntimeMs." / "Alias of timeoutMs…" — mutually circular, answering neither of
    // the caller's questions (which one to use; what omitting does).
    // SCOPE_19/B [CYRUP-DELTA] — `timeoutMs` is the advertised spelling and states its omitted
    // behaviour; `maxRuntimeMs` carries `deprecated: true` and defers. Dispatch still accepts both
    // and still cross-validates them (`resolve_foreground_timeout`).
    props.insert("timeoutMs".to_string(), serde_json::json!({ "type": "integer", "minimum": 1, "description": "Optional run-level timeout in ms for foreground and async/background runs. Omitted: the agent's or configured default for foreground runs; async runs use the async default. Prefer this over maxRuntimeMs; the two are aliases and must agree." }));
    props.insert("maxRuntimeMs".to_string(), serde_json::json!({ "type": "integer", "minimum": 1, "deprecated": true, "description": "Deprecated alias of timeoutMs; still accepted, and must agree with it." }));
    // SUBA-128 — pi `checkpointBeforeDeadlineMs` (`extension/schemas.ts:363` @v0.71.0), in
    // upstream's position and with its own bounds and description.
    props.insert("checkpointBeforeDeadlineMs".to_string(), serde_json::json!({ "type": "integer", "minimum": 1, "maximum": crate::registration::MAX_CHECKPOINT_BEFORE_DEADLINE_MS, "description": "Async single-agent runs only: the runner requests that the child checkpoint and stop this many ms before the run deadline (best-effort; the deadline kill still applies)." }));
    // CFG-067 — pi `toolTimeoutMs` (`extension/schemas.ts:364` @v0.71.0), description verbatim. It
    // is the PER-TOOL-CALL deadline, not the run's: `timeoutMs` above bounds the whole run, this
    // bounds any single tool call the child makes, and the two are enforced independently.
    props.insert("toolTimeoutMs".to_string(), serde_json::json!({ "type": "integer", "minimum": 1, "description": "Per-tool deadline (ms); fast builtins default 5m." }));
    props.insert("agentScope".to_string(), serde_json::json!({ "type": "string", "description": "Agent discovery scope: 'user', 'project', or 'both' (default: 'both'; project wins on name collisions)" }));
    // SCOPE_19/B [CYRUP-DELTA] — upstream leaves `cwd` undescribed; a property with no description
    // makes a careful caller set it defensively. It is read by every execution mode.
    props.insert("cwd".to_string(), serde_json::json!({ "type": "string", "description": "Working directory for the run (agent discovery root and base for relative paths). Default: the session's cwd." }));
    // SUBA-100 — pi `machine` (`extension/schemas.ts:369` @v0.68.0), right after `cwd` as
    // upstream: with a machine, `cwd` names the directory ON THAT MACHINE.
    props.insert("machine".to_string(), serde_json::json!({ "type": "string", "minLength": 1, "maxLength": 128, "description": "Herdr saved machine id or label; runs the agent there (native cyrup or a built-in Claude, Codex, or Cursor profile). cwd then means the directory on that machine." }));
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
    // PB-9: `clarify` is NOT advertised. It left upstream's schema at `39c37184` (v0.43.0) and
    // every public entry refuses it (`public-execution.ts:143-145` @v0.68.0); the preview UI it
    // described ("Show TUI to preview/edit before execution") was deleted at `ef554d2a` (v0.51.0)
    // and never existed here.
    // SUBA-N05: `control` is advertised again, in pi's v0.34.0 position (after the since-removed
    // `clarify` and before the solo agent overrides, `schemas.ts:278-279` @v0.34.0). It reaches `resolveControlConfig`
    // ([`crate::exec::control::resolve_control_config`]) on the foreground path via
    // `SingleRunOverrides::control` and on the async path via `RunnerConfig::control`, and drives
    // the live attention/notice pipeline in both. pi gives the top-level entry no description of
    // its own.
    // SCOPE_19/§5.2 [CYRUP-DELTA] — one clause is added anyway: a description-less optional
    // property leaves "what does omitting it do" unanswered, which is this task's defect class.
    props.insert("control".to_string(), {
        let mut control = sj_control_overrides();
        if let Some(obj) = control.as_object_mut() {
            obj.insert(
                "description".to_string(),
                serde_json::json!("Live-control thresholds/channels for attention notices on this run. Omitted: the subagents.control config, else built-in defaults."),
            );
        }
        control
    });
    // pi's own description (`schemas.ts:286`) kept its stale "Relative paths resolve against cwd"
    // clause: pi's `resolveSingleOutputPath` (`single-output.ts:64-77`) only falls back to a cwd
    // when no `relativeBaseDir` is supplied, and `runSinglePath` always supplies one
    // (`resolveSingleRunOutputBaseDir`, `:2882`).
    // SCOPE_19/B [CYRUP-DELTA] — the omitted behaviour is stated deliberately (upstream never says
    // it): `outputMode` directly below marks its default and this property did not, which is what
    // makes a careful caller set it defensively.
    props.insert("output".to_string(), serde_json::json!({
        "anyOf": [ { "type": "string" }, { "type": "boolean" } ],
        "description": "Output file for single agent (string), or false to disable. Relative paths resolve against cwd. Omitted: the persona's own output: setting, else no file."
    }));
    props.insert("outputMode".to_string(), serde_json::json!({ "type": "string", "enum": ["inline", "file-only"], "description": "Return saved output inline (default) or only a concise file reference. file-only requires output to be a path." }));
    props.insert("skill".to_string(), serde_json::json!({
        "anyOf": [ { "type": "array", "items": { "type": "string" } }, { "type": "boolean" }, { "type": "string" } ],
        "description": "Skill name(s) to make available (comma-separated), array of strings, or boolean (false disables, true uses default)"
    }));
    props.insert("model".to_string(), serde_json::json!({ "type": "string", "description": "Override model for single agent (e.g. 'anthropic/claude-sonnet-4')" }));
    // SUBA-096 — pi `fast` (`extension/schemas.ts:388` @v0.68.0), right after `model` as upstream.
    props.insert("fast".to_string(), serde_json::json!({ "type": "boolean", "description": "Native OpenAI-Codex priority tier; default false, may cost more/quota." }));
    // SUBA-043 / pi `extension/schemas.ts:351` @v0.43.0 — `outputSchema:
    // Type.Optional(JsonSchemaObject)` is a TOP-LEVEL `SubagentParamsSchema` property, in exactly
    // this position (after `model`, before `agentContract`/`acceptance`) under upstream's "Workflow
    // defaults forwarded to each child" comment. It was advertised only on the `tasks[]`/`chain[]`
    // ITEM schemas here, which made the capability SUBA-S01 landed — schema in, typed JSON out —
    // unreachable from the SINGLE surface a model actually calls: `subagent({agent, task,
    // outputSchema})` parsed (the root schema is `additionalProperties: true`), dropped the schema
    // without error, and returned free prose. The only workaround was a one-item `tasks:[…]`.
    // pi gives the top-level entry no description of its own.
    // SCOPE_19/§5.2 [CYRUP-DELTA] — one clause is added anyway, same rationale as `control` above.
    props.insert("outputSchema".to_string(), {
        let mut output_schema = sj_json_schema_object();
        if let Some(obj) = output_schema.as_object_mut() {
            obj.insert(
                "description".to_string(),
                serde_json::json!("JSON Schema the child's final output must satisfy; the result then carries typed JSON. Omitted: free-form text output."),
            );
        }
        output_schema
    });
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
    // SCOPE_19/A2 — the top-level `acceptance` is produced by the SAME builder the four nested
    // acceptance slots use. It previously hand-inlined its own copy of the wide enum, which is
    // exactly how `sj_acceptance_override`'s narrowing (see its doc: "none" and "verified" are
    // hard-rejected by `validate_acceptance_input`) missed this property. No duplicate of the enum
    // literal survives anywhere, so the next narrowing cannot miss a copy.
    props.insert("acceptance".to_string(), {
        let mut acceptance = sj_acceptance_override();
        if let Some(obj) = acceptance.as_object_mut() {
            obj.insert(
                "description".to_string(),
                serde_json::json!("Optional acceptance policy. Omit it (almost always right): the level is inferred from the task — read-only tasks verify nothing, write tasks require attested evidence. Explicit levels: auto, attested, checked; 'verified' additionally requires a verify[] command list inside an object policy."),
            );
        }
        acceptance
    });
    // The mission surface (`extension/schemas.ts:297-304` @v0.43.0), advertised together with its
    // dispatch arms: `mission.*` in the `action` enum above routes to
    // `crate::missions::handle_mission_action`, and `missionId`/`mission` additionally bind an
    // EXECUTION call to a mission via `SubagentTool::execute`'s launch binding.
    // SCOPE_19/§5.3 [CYRUP-DELTA] — upstream's one-word descriptions ("Mission id.", "Attached
    // run mode.") name no action at all; each now names its `mission.*` action in the first
    // clause, the same convention `scope`/`target`/`lines`/`additional` already follow.
    props.insert(
        "missionId".to_string(),
        serde_json::json!({ "type": "string", "description": "Mission id for mission.show/update/resolve-decision/attach-run/close, or to bind an execution call to a mission." }),
    );
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
    props.insert(
        "missionStatus".to_string(),
        serde_json::json!({ "type": "string", "description": "Mission status for mission.create/close." }),
    );
    props.insert("missionScope".to_string(), serde_json::json!({ "type": "string", "description": "Mission list scope: project (default) or global pointer index." }));
    props.insert(
        "runMode".to_string(),
        serde_json::json!({ "type": "string", "description": "Attached run mode for mission.attach-run. Omitted: 'external'." }),
    );
    props.insert(
        "runStatus".to_string(),
        serde_json::json!({ "type": "string", "description": "Attached run status for mission.attach-run." }),
    );
    props.insert(
        "summary".to_string(),
        serde_json::json!({ "type": "string", "description": "Summary for mission.close, or the resolution text for mission.resolve-decision." }),
    );

    // SUBA-016 — the schedule surface (`extension/schemas.ts:279`, `:311-318`, `:345` @v0.68.0),
    // advertised together with its dispatch arm (`route_action`'s `schedule.*` case) and its
    // reader (`SubagentToolParams::schedule_action_params`), per the crate's
    // advertise-vs-dispatch invariant. `id`/`timeoutMs`/`cwd`/`workflowScript` already exist above
    // and are shared.
    //
    // `on` and `timezone` are DECLARED AND REFUSED, deliberately: upstream answers a calendar
    // schedule with an actionable sentence naming the fixed-interval form to use instead, and a
    // model can only receive that sentence if the schema let the call through in the first place.
    props.insert("name".to_string(), serde_json::json!({ "type": "string", "description": "Display name for schedule.create. Omitted: derived from the workflow target." }));
    props.insert("at".to_string(), serde_json::json!({ "type": "string", "description": "schedule.create: delay (+10m) or zoned ISO timestamp." }));
    props.insert("every".to_string(), serde_json::json!({ "type": "string", "description": "schedule.create interval, e.g. 30m/6h/2d/2w." }));
    props.insert("sessionOnly".to_string(), serde_json::json!({ "type": "boolean", "description": "schedule.create: fire only while the creating session is live. Omitted: the schedule is project-wide and outlives this session." }));
    props.insert("quiet".to_string(), serde_json::json!({ "type": "boolean", "description": "schedule.create (recurring only) or schedule.run: deliver the completion without waking a turn. Omitted: a completion wakes the turn like any other." }));
    props.insert("on".to_string(), serde_json::json!({ "anyOf": [{ "type": "string" }, { "type": "integer" }], "description": "Reserved calendar selector." }));
    props.insert("timezone".to_string(), serde_json::json!({ "type": "string", "description": "Reserved calendar timezone; calendar schedules are not supported yet." }));
    props.insert("overlap".to_string(), serde_json::json!({ "type": "string", "enum": ["skip"], "description": "schedule.create overlap policy. Only skip is supported: a fire while the previous run is still going is recorded as skipped." }));
    props.insert("catchUp".to_string(), serde_json::json!({ "type": "string", "enum": ["none", "latest"], "description": "Missed schedule occurrences; default latest." }));
    props.insert("baseRef".to_string(), serde_json::json!({ "type": "string", "description": "Git ref a scheduled run should execute against. Not honoured yet; schedule.create refuses it rather than running against the wrong tree." }));
    // SUBA-150 fold-in (b) — pi `cfb6f9a8` (#2611, v0.75.0) states the limits in the description
    // and builds them from the ENFORCING constants (`extension/schemas.ts:7,227` @v0.75.0:
    // `${MAX_ARGS_FIELDS} fields/object, ${MAX_ARGS_ITEMS} items/array, depth ${MAX_ARGS_DEPTH},
    // ${MAX_ARGS_BYTES / 1024} KiB total`). Formatted here from
    // [`crate::workflows::MAX_ARGS_FIELDS`] and its three siblings — the very constants
    // `normalize_workflow_args` refuses with — so the advertised bound and the enforced bound
    // cannot drift. `maxProperties` is upstream's too (`:227`), and is the same constant again.
    props.insert(
        "args".to_string(),
        serde_json::json!({
            "type": "object",
            "maxProperties": crate::workflows::MAX_ARGS_FIELDS,
            "additionalProperties": true,
            "description": format!(
                "Plain-JSON args a scheduled workflow runs with; {} fields/object, {} items/array, \
                 depth {}, {} KiB total. Omitted: an empty object.",
                crate::workflows::MAX_ARGS_FIELDS,
                crate::workflows::MAX_ARGS_ITEMS,
                crate::workflows::MAX_ARGS_DEPTH,
                crate::workflows::MAX_ARGS_BYTES / 1024,
            )
        }),
    );

    serde_json::json!({
        "type": "object",
        "additionalProperties": true,
        "properties": serde_json::Value::Object(props),
    })
}

/// SUBA-151 — pi `createSubagentParamsSchema(disabled)` (`extension/schemas.ts:301-312`
/// @v0.75.0, annotated tag `06f8452c` -> commit `ad56bf92`): the advertised parameter set with
/// every property an operator's `config.disabledFeatures` removed taken OUT, and — when
/// `workflow-scripts` is among them — the `action` description reduced to the half that survives.
///
/// Upstream's own SAFETY note (`:310`, verbatim): *"only optional properties are dropped or
/// added; the executor rejects disabled options and admits chain/tasks at runtime."* Both clauses
/// hold here with ONE caveat. The first clause holds outright: the whole advertised set is
/// optional (the root object declares no `required`), and `chain`/`tasks` are advertised
/// unconditionally rather than added by this reduction (see below).
///
/// # The caveat is CLOSED — the executor now rejects a disabled option
///
/// This block used to read *"[`crate::disabled_features::disabled_feature_use_error`] — the
/// runtime half upstream's SAFETY clause leans on — has **no production caller in this crate**"*,
/// and named the remaining work as one call at the tool boundary. SUBA-152 made that call: it is
/// the first thing [`crate::extension::tool::SubagentTool`]'s `Tool::execute` prologue does, on the
/// RAW request, ahead of every side effect — pi's `disabledFeatureResult(params)` at
/// `runs/foreground/subagent-executor.ts:7921` (the closure at `:5287-5291`). Because cyrup's RPC
/// bridge dispatches through that same `SubagentTool` (`extension/rpc/mod.rs:584`), the one call
/// also covers the path upstream needs `extension/rpc.ts:535` for.
///
/// So BOTH clauses of upstream's SAFETY note now hold, and a reduced schema may be read as what it
/// says it is: a session that disabled `workflow-scripts` stops telling the model `workflow` is a
/// legal field, AND a caller that sends it anyway is refused by name rather than dispatched.
/// `a_disabled_param_is_refused_before_it_dispatches`,
/// `a_disabled_action_is_refused_before_it_dispatches`,
/// `a_disabled_workflow_script_carrier_is_refused_before_the_removal_message` (`routing_tests.rs`)
/// and `rpc_spawn_is_refused_by_the_same_disabled_feature_gate_as_the_tool`
/// (`tests/rpc_bridge_integration.rs`) are the four that hold it.
///
/// # Why a model needs this and not just a runtime refusal
///
/// Without it, a session that disabled a group still advertises that group's parameters, so the
/// model is told a field is legal, sends it, and is refused for a reason it could not have read
/// off the schema. That is this crate's advertise-vs-dispatch invariant in its second direction,
/// and the same defect class [`sj_usage_budget_override`]'s `minProperties` note describes.
///
/// # The four deltas upstream applies, and what each one does HERE
///
/// Upstream's `flatMap` does four things. Two are real work in this port, two are deliberate
/// no-ops, and the no-ops are no-ops for a stated reason rather than by omission:
///
/// 1. **Drop every property named in `disabled.params`** — REAL, and the whole value of this
///    function. It is generic over all 15 groups plus the synthetic `schedules` surface, so
///    disabling `usage-budgets` takes `usageBudget` out, `panes` takes `focus` out, and so on.
///    [`crate::disabled_features::DisabledSurfaceMap`] is the authority for the names; this
///    function never carries a second copy of them.
/// 2. **Reduce `action`'s description when `workflow-scripts` is disabled** — REAL. Upstream's
///    reduced `action` (`:289`) differs from its full one (`:162-164`) in exactly one sentence,
///    the `validate` clause, so that clause and only that clause is dropped
///    ([`ACTION_VALIDATE_CLAUSE`]). Upstream additionally replaces the whole property with a
///    bare `Type.String({minLength: 1})`; this port does NOT, because pi's full `action` is an
///    undiscriminated string and cyrup's carries the `enum` of
///    [`crate::extension::tool::text::SUBAGENT_ACTIONS`]. Throwing the enum away to match a shape
///    pi only has because it has no enum would ADVERTISE LESS than pi does and tell the model to
///    go read `guide topic tool-reference` for a list the schema was already handing it.
/// 3. **Re-describe `task` when `workflow-scripts` is disabled** — NO-OP here, for a structural
///    reason. Upstream's reduced sentence (`:290`) is *"One-child task with agent, or the original
///    request ({task}) with chain/tasks."*: it has to introduce `chain`/`tasks` because in pi they
///    exist ONLY in the reduced schema, and `{task}` because that is how a chain step reaches the
///    original request. In this port `chain`/`tasks` are advertised unconditionally (see 4) and
///    `{task}` substitution is unconditional too (`extension/executor/chain.rs:403`, and
///    `extension/tool/text.rs:38` already shows a `task:"Analyze {task}"` chain step in the tool
///    description). The sentence is therefore equally true with no group disabled, so saying it
///    only in the reduced schema would make the advertised text vary on something that does not
///    vary. `task`'s description is left alone in both forms.
/// 4. **Emit `tasks` and `chain` when `workflow-scripts` is disabled** — NO-OP here, deliberately,
///    and this is SUBA-151's decision of record. pi `0538e14d` (#2588, v0.74.0) left pi with no
///    top-level `chain`/`tasks` at all, and `createSubagentParamsSchema` re-adds a MINIMAL pair
///    (`StructuredWorkflowProperties`, `:287-299`: `{agent, task}` items, nothing else —
///    upstream's own comment at `:285-286` is *"Kept small because every field is sent on every
///    request"*) as the script-free way to compose work. This port kept the native graph instead
///    (option (b); pinned by
///    `chain_and_tasks_still_dispatch_to_the_native_graph_rather_than_a_removal_refusal`), so
///    `chain`/`tasks` are already in the default schema at `:575`/`:582` with item schemas that
///    are strict SUPERSETS of upstream's pair — cyrup's `tasks[]` item carries `agent`/`task` plus
///    `cwd`/`machine`/`count`/`output`/`outputMode`/`reads`/`progress`/`model`/`fast`/`skill`/
///    `acceptance`, and its `chain[]` item carries upstream's `agent`/`task`/`as`/`parallel` plus
///    `phase`/`label`/`outputSchema`/`expand`/`collect`/`concurrency`/`failFast`/`worktree`.
///    Re-emitting the minimal pair here would therefore REMOVE advertised capability the
///    dispatcher still accepts, in the one session shape where the model most needs chain/tasks.
///    `the_reduced_schema_keeps_the_native_chain_and_tasks_rather_than_pis_minimal_pair` pins
///    that the two entries come through the reduction unchanged.
///
/// # The guard is `params`, not `features`
///
/// Upstream returns the full schema when `disabled.params.size === 0` (`:302`), not when
/// `features.size === 0`. The two agree today — every group and the schedule surface owns at least
/// one param ([`crate::disabled_features::disabled_feature_notice`] relies on the same fact) — and
/// upstream's condition is the one ported, so a future params-less group would fall through to the
/// full schema here exactly as it would there.
#[must_use]
pub(crate) fn subagent_tool_parameters_for(surface: &DisabledFeatureSurface) -> serde_json::Value {
    let mut schema = subagent_tool_parameters();
    // pi `if (!disabled || disabled.params.size === 0) return SubagentParams;` (`:302`).
    if surface.params().is_empty() {
        return schema;
    }
    let structured = surface.contains(SubagentSurfaceFeature::Feature(
        SubagentFeature::WorkflowScripts,
    ));
    let Some(props) = schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    else {
        // Unreachable for the literal above, and an early return rather than a panic because this
        // crate's lints forbid one: a reduction that cannot find the property map must hand back
        // the full schema it was given, never a half-reduced one.
        return schema;
    };
    for (param, _) in surface.params().iter() {
        // `shift_remove`, NOT `remove`: with `serde_json/preserve_order` (declared workspace-wide,
        // `Cargo.toml:253`) a `serde_json::Map` is an `IndexMap` and `Map::remove` is
        // `swap_remove`, which would drag the LAST property into the hole. Every property in
        // [`subagent_tool_parameters`] is placed at pi's own position on purpose, and the removal
        // of one must not reorder the rest.
        props.shift_remove(param);
    }
    // pi `if (structured && name === "action") return [[name, StructuredWorkflowProperties.action]]`
    // (`:306`), narrowed to the description half per delta 2 above. `Map::insert` on an existing
    // key keeps that key's position under `preserve_order`, so `action` stays where pi puts it.
    if structured
        && let Some(action) = props
            .get_mut("action")
            .and_then(serde_json::Value::as_object_mut)
    {
        action.insert(
            "description".to_string(),
            serde_json::Value::String(ACTION_DESCRIPTION_BASE.to_string()),
        );
    }
    schema
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

    /// SUBA-151 — the two schema narrowings that landed with pi's `chain`/`tasks` removal window,
    /// ported for the same reason the `usageBudget` one was: each states a bound the DISPATCHER
    /// already refuses by name, and a bound the dispatcher enforces while the schema calls it
    /// legal is a refusal the model could not have avoided.
    ///
    /// Both halves are asserted — the sentence AND the refusal it describes — so neither can be
    /// reworded into something the validator does not do.
    #[test]
    fn the_budget_and_timeout_alias_bounds_are_stated_where_the_dispatcher_enforces_them() {
        let schema = subagent_tool_parameters();
        let props = schema["properties"]
            .as_object()
            .expect("the tool schema must expose an object of properties");

        // pi `extension/schemas.ts:104` @v0.75.0 — `soft <= hard`.
        let tool_budget = props["toolBudget"]["description"]
            .as_str()
            .expect("toolBudget carries a description");
        assert!(tool_budget.contains("soft <= hard"), "got {tool_budget}");
        assert_eq!(
            crate::exec::tool_budget::validate_tool_budget_config(
                Some(&serde_json::json!({ "hard": 2, "soft": 5 })),
                "toolBudget"
            ),
            Err("toolBudget.soft must be <= toolBudget.hard.".to_string()),
            "the stated bound is the one the dispatcher enforces"
        );

        // pi `extension/schemas.ts:241` @v0.75.0 — *"Alias maxRuntimeMs; must agree."*
        for name in ["timeoutMs", "maxRuntimeMs"] {
            let description = props[name]["description"]
                .as_str()
                .expect("the alias carries a description");
            assert!(
                description.contains("must agree"),
                "{name} must state that the aliases have to agree, because \
                 `resolve_foreground_timeout` refuses two different values; got {description}"
            );
        }
        let disagreeing: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "agent": "a", "timeoutMs": 10, "maxRuntimeMs": 20
        }))
        .expect("the params parse");
        assert_eq!(
            crate::extension::tool::params::resolve_foreground_timeout(&disagreeing, None),
            Err(
                "timeoutMs and maxRuntimeMs are aliases; provide only one value or use the same \
                 value for both."
                    .to_string()
            ),
            "the stated bound is the one the dispatcher enforces"
        );
    }

    /// SUBA-150 — the two parameters pi `0538e14d` (#2588, v0.74.0) DELETED from the tool are
    /// gone from the advertised schema, and the one `workflow` field that replaced them is there
    /// in upstream's union shape.
    ///
    /// Asserted against the schema the model actually receives — `self.parameters`, the value
    /// `Tool::parameters` hands the provider — rather than against a hand-maintained name list,
    /// because a list is exactly what goes stale when a property is reintroduced elsewhere in the
    /// builder. THE USER ACTION: a model reading this schema must be told `workflow`, and must not
    /// be shown a parameter the boundary refuses by name.
    #[tokio::test]
    async fn the_removed_workflow_parameters_are_gone_and_workflow_replaces_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = scoped_tool(dir.path()).await;
        let advertised = tool.parameters()["properties"]
            .as_object()
            .expect("the advertised schema has a properties object")
            .clone();

        for removed in ["workflowScript", "workflowScriptPath"] {
            assert!(
                !advertised.contains_key(removed),
                "'{removed}' was deleted from the tool at v0.74.0 and the boundary refuses it by \
                 name, so advertising it would promise a parameter that cannot be used; keys: {:?}",
                advertised.keys().collect::<Vec<_>>()
            );
        }
        let workflow = advertised
            .get("workflow")
            .expect("the one workflow field must be advertised");
        assert_eq!(
            workflow["anyOf"],
            serde_json::json!([
                { "type": "boolean" },
                { "type": "string", "minLength": 1 }
            ]),
            "pi `extension/schemas.ts:223-226` @v0.75.0 advertises the union, not a bare string: a \
             string-only schema makes a provider converter reject `workflow: true`"
        );
        let description = workflow["description"]
            .as_str()
            .expect("the workflow field carries a description");
        for form in ["true", "'/'", "named workflow resource"] {
            assert!(
                description.contains(form),
                "the description must name the {form} form, or a model cannot tell a path from a \
                 resource name; got {description}"
            );
        }
    }

    /// SUBA-150 fold-in (b) / pi `cfb6f9a8` (#2611, v0.75.0) — the `args` description STATES its
    /// limits, and states them from the constants that enforce them.
    ///
    /// The anti-drift assertion is the point: every number in the description is recomputed here
    /// from [`crate::workflows::MAX_ARGS_FIELDS`] and its siblings, AND
    /// [`crate::workflows::normalize_workflow_args`] is made to refuse at exactly those values. A
    /// change to one bound that left the other behind — the literal-in-the-description problem
    /// upstream exported the constants to end — fails here.
    #[test]
    fn the_args_description_states_the_same_limits_the_normaliser_enforces() {
        let schema = subagent_tool_parameters();
        let args = &schema["properties"]["args"];
        assert_eq!(
            args["maxProperties"],
            serde_json::json!(crate::workflows::MAX_ARGS_FIELDS),
            "pi advertises the field bound as `maxProperties` too (`schemas.ts:227`)"
        );
        let description = args["description"]
            .as_str()
            .expect("args carries a description");
        for stated in [
            format!("{} fields/object", crate::workflows::MAX_ARGS_FIELDS),
            format!("{} items/array", crate::workflows::MAX_ARGS_ITEMS),
            format!("depth {}", crate::workflows::MAX_ARGS_DEPTH),
            format!("{} KiB", crate::workflows::MAX_ARGS_BYTES / 1024),
        ] {
            assert!(
                description.contains(&stated),
                "the advertised description must state {stated:?} — the value the normaliser \
                 enforces; got {description}"
            );
        }

        // …and the enforcing side really does refuse at those numbers, so the description is not
        // merely self-consistent.
        let over_fields: serde_json::Map<String, serde_json::Value> = (0
            ..=crate::workflows::MAX_ARGS_FIELDS)
            .map(|i| (format!("f{i}"), serde_json::json!(1)))
            .collect();
        assert!(
            crate::workflows::normalize_workflow_args(Some(&serde_json::Value::Object(
                over_fields
            )))
            .is_err(),
            "one field past the advertised bound must be refused"
        );
        let at_fields: serde_json::Map<String, serde_json::Value> = (0
            ..crate::workflows::MAX_ARGS_FIELDS)
            .map(|i| (format!("f{i}"), serde_json::json!(1)))
            .collect();
        assert!(
            crate::workflows::normalize_workflow_args(Some(&serde_json::Value::Object(at_fields)))
                .is_ok(),
            "exactly the advertised bound must be admitted"
        );
    }

    /// PB-9 — the schema does not advertise `clarify` (upstream dropped it at v0.43.0 and refuses
    /// it at every public entry). Mutation killed: re-inserting the property.
    #[test]
    fn the_schema_does_not_advertise_clarify() {
        let schema = subagent_tool_parameters();
        assert!(
            schema["properties"].get("clarify").is_none(),
            "a key the boundary refuses must not be advertised"
        );
    }

    /// SUBA-096 — pi advertises `fast` on the top level and on the three item schemas
    /// (`extension/schemas.ts:167,199,229,388` @v0.68.0), with upstream's descriptions, and the
    /// parser now reads it. Mutation killed: deleting any one of the four insertions.
    #[test]
    fn fast_is_advertised_on_the_top_level_and_every_item_schema() {
        let schema = subagent_tool_parameters();
        let props = &schema["properties"];
        assert_eq!(props["fast"]["type"], serde_json::json!("boolean"));
        assert_eq!(
            props["fast"]["description"],
            serde_json::json!(
                "Native OpenAI-Codex priority tier; default false, may cost more/quota."
            )
        );
        let item = |value: &serde_json::Value| value["properties"]["fast"]["type"].clone();
        assert_eq!(item(&props["tasks"]["items"]), serde_json::json!("boolean"));
        assert_eq!(item(&props["chain"]["items"]), serde_json::json!("boolean"));
        assert_eq!(item(&sj_parallel_task()), serde_json::json!("boolean"));
        assert_eq!(
            item(&sj_dynamic_parallel_template()),
            serde_json::json!("boolean")
        );
        let parsed: SubagentToolParams =
            serde_json::from_value(serde_json::json!({"agent": "a", "task": "t", "fast": true}))
                .expect("parses");
        assert_eq!(parsed.fast, Some(true));
    }

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
            "agent",
            "task",
            "action",
            "id",
            "runId",
            "dir",
            "index",
            "childId",
            "message",
            "chainName",
            "config",
            "tasks",
            "concurrency",
            "worktree",
            "chain",
            "context",
            "chainDir",
            "async",
            "timeoutMs",
            "maxRuntimeMs",
            "toolTimeoutMs",
            "agentScope",
            "cwd",
            "artifacts",
            "includeProgress",
            "share",
            "sessionDir",
            "control",
            "output",
            "outputMode",
            "skill",
            "model",
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
        // SUBA-151 — pi `minProperties: 1` (`71d042f0`, #2596, v0.74.0). THE USER ACTION: a model
        // sends `usageBudget: {}` because the schema said an all-optional object was legal, and
        // dispatch then refuses the whole call with `usageBudget must include tokens or costUsd.`
        // The advertised schema and the dispatch have to agree, and the dispatch is the one that
        // is already right — so the schema stops advertising the shape it will refuse.
        assert_eq!(
            usage["minProperties"],
            serde_json::json!(1),
            "an empty usageBudget is refused at dispatch, so it must not be advertised as legal"
        );
        assert_eq!(
            crate::exec::usage_budget::validate_usage_budget_config(
                Some(&serde_json::json!({})),
                "usageBudget"
            )
            .expect_err("an empty block is refused"),
            "usageBudget must include tokens or costUsd.",
            "the behaviour the schema now advertises"
        );
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
        assert_eq!(
            control_props["enabled"]["type"],
            serde_json::json!("boolean")
        );
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
                // pi's own indices: upstream reads `… "models", "children.list", "guide",
                // "validate", "create", …` (`shared/types.ts:2801` @v0.68.0). `children.list`
                // dispatches to `background::retained_children` (the settled step rows of this
                // session's workflow status files); SUBA-055 added `guide` with
                // `registration::guide::read_subagent_guide`.
                "list",
                "get",
                "models",
                "children.list",
                "guide",
                // WORKFLOW_2 — pi's own index for `validate`: upstream `SUBAGENT_ACTIONS`
                // (`shared/types.ts:2760`) reads `… "guide", "validate", "create", …`.
                "validate",
                "create",
                "update",
                "delete",
                "eject",
                "disable",
                "enable",
                "reset",
                "status",
                // pi's own index for `debug.run` (`shared/types.ts:2801` @v0.68.0), immediately
                // after `status`.
                "debug.run",
                "grant-spawn-budget",
                "interrupt",
                "resume",
                "steer",
                "stop",
                "dismiss",
                "append-step",
                // SCOPE_12 — [CYRUP-DELTA], and the only entry here with no upstream index:
                // upstream reaches `inspect-rpc.ts` through a slash command only, so `inspect`
                // is absent from its `SUBAGENT_ACTIONS`. It sits at the end of the control band,
                // advertised in the same change that gave `route_action` its dispatch arm.
                "inspect",
                "doctor",
                "mission.create",
                "mission.list",
                "mission.show",
                "mission.update",
                "mission.resolve-decision",
                "mission.attach-run",
                "mission.close",
                // LANES_2 — pi's own indices for the five convergence verbs
                // (`shared/types.ts:2801` @v0.68.0: `… "mission.close", "worktree.discard",
                // "worktree.cleanup", "lane.status", "lane.recordMerge",
                // "lane.recordSupersession", "refine", "refine.show", "refine.rollback",
                // "inspector.open", …`).
                "worktree.discard",
                "worktree.cleanup",
                "lane.status",
                "lane.recordMerge",
                "lane.recordSupersession",
                // VL-S13 — pi's own indices for the three `refine*` verbs, immediately after
                // `lane.recordSupersession` and before `inspector.open`.
                "refine",
                "refine.show",
                "refine.rollback",
                // VL-S6 — the seven inspector/project verbs at pi's own indices
                // (`shared/types.ts:2801` @v0.68.0: `… "refine.rollback", "inspector.open",
                // "inspector.command", "inspector.status", "inspector.close", "project.open",
                // "project.status", "project.close", "status", …`), anchored on the NEIGHBOURS
                // they have in THIS list rather than on upstream's absolute index — see
                // `text.rs`'s `SUBAGENT_ACTIONS` doc for why the two orders are not the same.
                //
                // These seven are pinned present in BOTH this enum and `SUBAGENT_ACTIONS`, and
                // proven to reach their own dispatch arms, by
                // `inspector_actions_dispatch_tests.rs` (a `#[path]` sibling of `routing.rs`).
                "inspector.open",
                "inspector.command",
                "inspector.status",
                "inspector.close",
                "project.open",
                "project.status",
                "project.close",
                "watchdog.status",
                "watchdog.check",
                "watchdog.configure",
                "watchdog.recommend-model",
                // SUBA-016 — pi's own index for the nine `schedule.*` verbs
                // (`shared/types.ts:2760` @v0.68.0 ends `… "watchdog.recommend-model",
                // "schedule.create", … "schedule.delete"`).
                "schedule.create",
                "schedule.list",
                "schedule.show",
                "schedule.history",
                "schedule.pause",
                "schedule.resume",
                "schedule.run",
                "schedule.run-due",
                "schedule.delete"
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
        // SUBA-087 — `schemas.ts:306` @v0.64.0.
        assert_eq!(props["childId"]["minLength"], serde_json::json!(1));
        assert_eq!(props["childId"]["maxLength"], serde_json::json!(256));

        // tasks[] per-task fields the description advertises (output/outputMode/reads/progress),
        // plus count's minimum.
        let task_props = props["tasks"]["items"]["properties"]
            .as_object()
            .expect("tasks[].items has a properties object");
        for per_task in [
            "agent",
            "task",
            "count",
            "output",
            "outputMode",
            "reads",
            "progress",
        ] {
            assert!(
                task_props.contains_key(per_task),
                "tasks[] items must carry the per-task field '{per_task}'"
            );
        }
        assert_eq!(task_props["count"]["minimum"], serde_json::json!(1));
        assert_eq!(task_props["progress"]["type"], serde_json::json!("boolean"));
        assert_eq!(
            props["tasks"]["items"]["required"],
            serde_json::json!(["agent", "task"])
        );

        // chain[] items must be an additionalProperties:false object with the flattened
        // sequential/parallel/dynamic surface (schemas.ts:190-229).
        let chain_item = &props["chain"]["items"];
        assert_eq!(chain_item["type"], serde_json::json!("object"));
        assert_eq!(chain_item["additionalProperties"], serde_json::json!(false));
        let chain_props = chain_item["properties"]
            .as_object()
            .expect("chain[].items has a properties object");
        for chain_field in [
            "agent",
            "parallel",
            "expand",
            "collect",
            "concurrency",
            "failFast",
            "worktree",
        ] {
            assert!(
                chain_props.contains_key(chain_field),
                "chain[] items must carry '{chain_field}'"
            );
        }

        // config/output/skill/acceptance are provider-friendly anyOf unions (no bare top-level type).
        assert!(
            props["config"].get("anyOf").is_some(),
            "config must be an anyOf union"
        );
        assert!(
            props["output"].get("anyOf").is_some(),
            "output must be an anyOf union"
        );
        assert!(
            props["skill"].get("anyOf").is_some(),
            "skill must be an anyOf union"
        );
        assert!(
            props["acceptance"].get("anyOf").is_some(),
            "acceptance must be an anyOf union"
        );

        // SUBA-041: the control fragment is no longer inserted into the advertised schema (no
        // `resolveControlConfig`/notice pipeline in this port), but it is KEPT as the shape record
        // for whichever tier lands that subsystem — so its nested attention thresholds + notify
        // enums are still pinned here, against the fragment rather than against `props`.
        let control_fragment = sj_control_overrides();
        let control_props = control_fragment["properties"]
            .as_object()
            .expect("control has a properties object");
        assert_eq!(
            control_props["needsAttentionAfterMs"]["minimum"],
            serde_json::json!(1)
        );
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
            // Was `"use { action: \"list\" } to inspect configured agents/chains"` — upstream's
            // mandatory-discovery bullet. SCOPE_19/§5.1 replaced it deliberately (see the
            // constant's [CYRUP-DELTA]): the six builtins are compiled in, so the description now
            // names them and scopes `list` to project/user agents, chains, and disabled state.
            // These two needles pin BOTH halves of the replacement — the builtins roster and the
            // re-scoped list guidance — so the divergence stays a recorded, tested fact.
            "delegate (inherit-model lightweight child), oracle (high-context decision consistency), researcher (focused research brief), reviewer (diffs/plans/PR validation), scout (fast codebase recon), worker (implementation)",
            "Use { action: \"list\" } when you need project- or user-defined agents and chains",
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
            (
                "control",
                serde_json::json!({ "control": { "needsAttentionAfterMs": 5000 } }),
            ),
            (
                "includeProgress",
                serde_json::json!({ "includeProgress": true }),
            ),
            ("timeoutMs", serde_json::json!({ "timeoutMs": 60_000 })),
            (
                "maxRuntimeMs",
                serde_json::json!({ "maxRuntimeMs": 60_000 }),
            ),
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
        assert_eq!(
            props["toolBudget"]["additionalProperties"],
            serde_json::json!(false)
        );
        // The shape is the same open `JsonSchemaObject` the `tasks[]` item schema already uses;
        // SCOPE_19/§5.2 additionally merges a top-level description (pi gives none), so the pin
        // compares the SHAPE fields and requires the description separately rather than demanding
        // byte-equality with the bare builder.
        assert_eq!(
            props["outputSchema"]["type"],
            sj_json_schema_object()["type"]
        );
        assert_eq!(
            props["outputSchema"]["additionalProperties"],
            sj_json_schema_object()["additionalProperties"]
        );
        assert!(
            props["outputSchema"]["description"]
                .as_str()
                .is_some_and(|d| d.contains("Omitted:")),
            "outputSchema's description must state its omitted behaviour (SCOPE_19/§5.2)"
        );

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
        assert!(
            props.contains_key("turnBudget"),
            "the enforced param must be advertised"
        );
        // pi `extension/schemas.ts:104-107` @v0.43.0: `maxTurns` REQUIRED, `graceTurns` optional
        // and >= 0 (NOT >= 1 — a zero grace is legal and means "abort at maxTurns"), object closed.
        assert_eq!(
            props["turnBudget"]["required"],
            serde_json::json!(["maxTurns"])
        );
        assert_eq!(
            props["turnBudget"]["additionalProperties"],
            serde_json::json!(false)
        );
        assert_eq!(
            props["turnBudget"]["properties"]["maxTurns"]["minimum"],
            serde_json::json!(1)
        );
        assert_eq!(
            props["turnBudget"]["properties"]["graceTurns"]["minimum"],
            serde_json::json!(0)
        );
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
        assert!(
            parsed.provided_keys().contains(&"turnBudget"),
            "{:?}",
            parsed.provided_keys()
        );
    }

    // ---------------------------------------------------------------------------------------------
    // SUBA-151 — `createSubagentParamsSchema`'s reduction (`extension/schemas.ts:301-312` @v0.75.0)
    // ---------------------------------------------------------------------------------------------

    use crate::disabled_features::{
        SUBAGENT_FEATURES, SubagentFeature, SubagentSurfaceFeature,
        resolve_disabled_feature_surface,
    };

    /// The canonical bytes a built schema advertises. `serde_json::to_vec` is deterministic here
    /// because the workspace declares `serde_json/preserve_order` (`Cargo.toml:253`), so the
    /// property order in the bytes is the insertion order [`subagent_tool_parameters`] builds.
    fn schema_bytes(schema: &serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(schema).expect("a built schema must serialize")
    }

    /// The same canonical bytes as [`schema_bytes`], as text — so an assertion failure prints the
    /// JSON a reviewer has to read rather than a byte array.
    fn schema_text(schema: &serde_json::Value) -> String {
        serde_json::to_string(schema).expect("a built schema must serialize")
    }

    fn schema_digest(schema: &serde_json::Value) -> String {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(schema_bytes(schema));
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn property_names(schema: &serde_json::Value) -> Vec<String> {
        schema["properties"]
            .as_object()
            .expect("the tool schema must expose an object of properties")
            .keys()
            .cloned()
            .collect()
    }

    /// The DEFAULT advertised schema's measured bytes.
    ///
    /// # SUBA-152 RE-BASELINED THIS ON PURPOSE
    ///
    /// Before: `beaab01c62a7139e965803c0b7ec617b1ca6249eb18692c8395021e41bd09798` — the value
    /// SUBA-151 measured on `59186c60`, when this test's job was to prove the reduction changed the
    /// default schema for NOBODY.
    ///
    /// After: `b0e48cee326485971c71662ff9c01192ebc54036ea2fc5715e66fc1784cc402f`.
    ///
    /// Three narrowings moved it, and they move it for EVERY session, disabled features or not.
    /// That is the whole reason they were held back out of SUBA-151 and landed deliberately here:
    ///
    /// * `minItems: 1` on `tasks` (`props.insert("tasks", …)`),
    /// * `minItems: 1` on `chain` (`props.insert("chain", …)`),
    /// * `minLength: 1` on `agent` inside [`sj_task_item`].
    ///
    /// Each one removes a value the schema called legal and the DISPATCHER refuses — `{"tasks":
    /// []}` and `{"chain": []}` fall through to "Provide exactly one mode. Agents: …", and
    /// `{"tasks": [{"agent": "", …}]}` dies on `agent not found: ` — so the advertised surface got
    /// strictly more honest, which is the same trade [`sj_usage_budget_override`]'s
    /// `minProperties: 1` made. No property was added or removed; only three constraints were
    /// added, so nothing a conforming caller could previously send has become unsendable.
    ///
    /// The digest was recomputed the way it is checked — `schema_digest(&subagent_tool_parameters())`,
    /// read off this test's own `assert_eq!` failure — not adjusted to make a red test green.
    /// A fourth narrowing was considered and REJECTED with evidence rather than ported:
    /// `additionalProperties: false` on [`sj_task_item`], whose doc records why it would advertise
    /// the inverse defect. Had it been ported silently, this digest would have moved for that too.
    ///
    /// # The test was renamed with the re-baseline
    ///
    /// It was `the_default_schema_is_byte_identical_to_the_pre_reduction_baseline`. It no longer is
    /// byte-identical to that baseline, on purpose, so the name had to stop saying so — a suite
    /// line asserting a claim its own constant contradicts is the same failure mode as a silently
    /// re-pinned digest, one level up.
    ///
    /// # What the test still pins
    ///
    /// The contract is unchanged, only its baseline: a session that disabled nothing must receive
    /// the bytes this constant names, and [`subagent_tool_parameters_for`] with an EMPTY surface
    /// must return those same bytes — pi's `if (!disabled || disabled.params.size === 0) return
    /// SubagentParams;` (`extension/schemas.ts:302`) is identity, not a rebuild that happens to
    /// look similar.
    ///
    /// A digest rather than a prose comparison because the schema is ~30 properties of nested
    /// JSON: an eyeballed diff is exactly how the `action` description could have gained or lost a
    /// space without anything failing. If this digest changes again, the advertised tool surface
    /// changed for EVERY session, and that is a decision to make deliberately — re-measure it and
    /// say here what moved it and why, as this block does. **A silently re-pinned digest is worse
    /// than no pin at all**: it converts the one automatic signal that the whole fleet's tool
    /// surface shifted into a line of noise in a diff.
    const DEFAULT_SCHEMA_DIGEST_AT_SUBA_152: &str =
        "b0e48cee326485971c71662ff9c01192ebc54036ea2fc5715e66fc1784cc402f";

    /// MUTATION: drop the trailing `.` from [`ACTION_VALIDATE_CLAUSE`] — the digest reads
    /// `62dc0d2e…` against the digest pinned at the time (`beaab01c…`). Observed RED.
    /// MUTATION (SUBA-152), each observed RED against the pinned `b0e48cee…`: drop `"minItems": 1`
    /// from `tasks` and the digest reads `d409cb2d…`; from `chain`, `eeec92f2…`; drop
    /// `"minLength": 1` from [`sj_task_item`]'s `agent` and it reads `027b9076…`. Three distinct
    /// digests, so this pin discriminates between the three narrowings rather than merely noticing
    /// that something moved.
    #[test]
    fn the_default_schema_matches_its_deliberately_rebaselined_digest() {
        let full = subagent_tool_parameters();
        assert_eq!(
            schema_digest(&full),
            DEFAULT_SCHEMA_DIGEST_AT_SUBA_152,
            "the no-disabled-features schema moved; re-measure it and record WHY on \
             DEFAULT_SCHEMA_DIGEST_AT_SUBA_152, never silently re-pin it"
        );

        // Both of pi's two "return the full schema" doors: no surface at all, and a surface that
        // disabled nothing. `scheduledRuns.enabled: true` is passed so the synthetic `schedules`
        // group is not folded in either.
        let nothing_disabled = resolve_disabled_feature_surface(&[], true);
        assert!(nothing_disabled.params().is_empty());
        assert_eq!(
            schema_bytes(&subagent_tool_parameters_for(&nothing_disabled)),
            schema_bytes(&full),
            "an empty surface must return the full schema unchanged"
        );
    }

    /// SUBA-152 — the three narrowings that moved
    /// [`DEFAULT_SCHEMA_DIGEST_AT_SUBA_152`], each named, so the digest is not the only thing
    /// holding them.
    ///
    /// The digest pins "the default schema is exactly these bytes" and nothing about INTENT: it
    /// would go equally red if one of these constraints were deleted and equally green if someone
    /// re-pinned it. These assertions say which constraints this change added and why they are
    /// legitimate, naming for each one the dispatcher refusal it now advertises. The matching
    /// dispatcher behaviour is asserted end-to-end in
    /// `subagent_tool_rejects_empty_tasks_and_chain_arrays_as_no_mode_selected`
    /// (`routing_tests.rs`) and
    /// `an_empty_agent_name_in_a_tasks_item_is_refused_by_the_dispatcher`.
    ///
    /// The fourth candidate, `additionalProperties: false` on the `tasks[]` item, is asserted
    /// ABSENT here with the reason, so a future reader who notices the asymmetry with the
    /// `chain[]` item finds the measurement instead of repeating it — see [`sj_task_item`]'s doc.
    ///
    /// MUTATION: drop `"minItems": 1` from `tasks` — this test names the entry and
    /// [`the_default_schema_matches_its_deliberately_rebaselined_digest`] reads a different digest.
    /// Observed RED.
    #[test]
    fn the_narrowings_advertise_exactly_what_the_dispatcher_refuses() {
        let schema = subagent_tool_parameters();
        let props = &schema["properties"];

        // `{"tasks": []}` / `{"chain": []}` select no mode and are refused.
        assert_eq!(
            props["tasks"]["minItems"],
            serde_json::json!(1),
            "an empty tasks[] is refused by dispatch, so it must not be advertised as legal"
        );
        assert_eq!(
            props["chain"]["minItems"],
            serde_json::json!(1),
            "an empty chain[] is refused by dispatch, so it must not be advertised as legal"
        );

        // `{"tasks": [{"agent": "", …}]}` dies on `agent not found: `.
        assert_eq!(
            props["tasks"]["items"]["properties"]["agent"]["minLength"],
            serde_json::json!(1),
            "an empty agent name is refused by dispatch"
        );

        // NOT ported: the item declares 13 properties and `ToolTaskItem` parses 18, two of which
        // (`as`, `outputSchema`) reach the child. `additionalProperties: false` would advertise
        // those two as illegal — the schema refusing what dispatch admits.
        assert!(
            props["tasks"]["items"]
                .get("additionalProperties")
                .is_none(),
            "the tasks[] item must not claim a closed property set it does not declare in full"
        );
        for honoured in ["as", "outputSchema"] {
            assert!(
                props["tasks"]["items"]["properties"]
                    .get(honoured)
                    .is_none(),
                "if `{honoured}` is ever advertised on the tasks[] item, revisit \
                 additionalProperties: false — this assertion is the only thing recording that the \
                 constraint was blocked on it"
            );
        }
    }

    /// SUBA-151 — the exact set of group parameters this port can drop, measured rather than
    /// asserted in prose.
    ///
    /// The reduction can only remove a property the schema actually advertises, so "what does
    /// disabling a group do here" is answered by intersecting each group's `params` with
    /// [`subagent_tool_parameters`]'s keys. This test pins BOTH sides of that intersection:
    ///
    /// * every group param this port advertises (so a future rename of a schema property silently
    ///   un-gating a group fails here), and
    /// * every group param it does NOT advertise — five names, each a group member with no cyrup
    ///   surface, for which disabling the group is a no-op by design (see
    ///   [`crate::disabled_features`]'s own module doc on why that is the correct outcome and not
    ///   a defect to "fix" by inventing a parameter).
    ///
    /// It corrects a stale claim in that module's doc, which listed `args`, `missionStatus`,
    /// `missionId`, `runMode`, `runStatus`, `summary`, `laneId`, `supersession` and `planId` as
    /// unadvertised. All nine ARE advertised — `args` at [`subagent_tool_parameters`]'s
    /// `props.insert("args", …)` and the other eight alongside the mission/lane surfaces — so
    /// `workflow-scripts` drops TWO of its five params here (`workflow`, `args`), not one.
    /// MUTATION: delete `props.insert("args", …)` from [`subagent_tool_parameters`] — the absent
    /// list grows to six and names `args` first, which is the measurement that corrected the
    /// stale doc. Observed RED.
    #[test]
    fn exactly_five_group_params_have_no_advertised_surface_to_drop() {
        let advertised = property_names(&subagent_tool_parameters());
        let mut absent: Vec<&str> = Vec::new();
        let mut present: Vec<&str> = Vec::new();
        for entry in SUBAGENT_FEATURES {
            for param in entry.params {
                if advertised.iter().any(|name| name == param) {
                    present.push(param);
                } else {
                    absent.push(param);
                }
            }
        }
        absent.sort_unstable();
        absent.dedup();
        present.sort_unstable();
        present.dedup();

        assert_eq!(
            absent,
            [
                "extensionBindings",
                "gate",
                "globalConcurrencyLimit",
                "maxSubagentSpawnsPerRun",
                "preflight",
            ],
            "the group params with no cyrup surface changed"
        );
        assert_eq!(
            present,
            [
                "additional",
                "args",
                "config",
                "control",
                "focus",
                "handoffPath",
                "lane",
                "laneId",
                "machine",
                "merge",
                "mission",
                "missionId",
                "missionScope",
                "missionStatus",
                "missionUpdate",
                "planId",
                "repo",
                "runMode",
                "runStatus",
                "scope",
                "summary",
                "supersession",
                "target",
                "thinking",
                "toolBudget",
                "usageBudget",
                "workflow",
            ],
            "the group params the reduction can actually drop changed"
        );
    }

    /// SUBA-151 — pi `if (disabled.params.has(name)) return [];` (`:305`): disabling a group takes
    /// that group's advertised params OUT, takes nothing else out, and reorders nothing.
    ///
    /// Driven over all 15 groups rather than a sample, so a group whose params stop matching the
    /// schema fails here instead of quietly advertising a disabled feature. The order assertion is
    /// what pins `shift_remove` over `serde_json::Map::remove`: under `preserve_order` the latter
    /// is `swap_remove` and would drag the final property (`args`) into each hole, which no
    /// property-SET assertion can see.
    /// MUTATION 1: replace the loop body with `let _ = param;` — `agent-management` leaves
    /// `config` advertised. Observed RED.
    ///
    /// MUTATION 2: `props.shift_remove(param)` -> `props.remove(param)` — the property SET is
    /// right and the ORDER is not: `args`, the last property, lands in `config`'s hole. Observed
    /// RED, and the reason this test compares ordered `Vec`s rather than sets.
    #[test]
    fn disabling_a_group_removes_exactly_that_groups_advertised_params() {
        let default_order = property_names(&subagent_tool_parameters());
        for entry in SUBAGENT_FEATURES {
            let surface = resolve_disabled_feature_surface(&[entry.feature], true);
            let reduced = property_names(&subagent_tool_parameters_for(&surface));

            // The group's OWN params are the ones resolution attributes to it, which for
            // `preflight`/`workflow-scripts` is not the same as the group's member list — the
            // shared `preflight` param is attributed once. The surface is therefore the authority.
            let disabled: Vec<&str> = surface.params().iter().map(|(name, _)| name).collect();
            let expected: Vec<String> = default_order
                .iter()
                .filter(|name| !disabled.iter().any(|param| param == name))
                .cloned()
                .collect();
            assert_eq!(
                reduced,
                expected,
                "disabling {} must drop exactly its advertised params, in place",
                entry.feature.as_str()
            );
            for param in entry.params {
                assert!(
                    !reduced.iter().any(|name| name == param),
                    "{} left {param} advertised",
                    entry.feature.as_str()
                );
            }
        }
    }

    /// SUBA-151 — pi `const structured = disabled.features.has("workflow-scripts");` (`:303`): the
    /// `action` re-description fires for THAT group and no other.
    ///
    /// Both directions are asserted against the same two constants the builder composes
    /// ([`ACTION_DESCRIPTION_BASE`], [`ACTION_VALIDATE_CLAUSE`]), so the reduced sentence cannot
    /// drift from the full one. The control arm is `usage-budgets`: it disables a param, so the
    /// reduction runs and the schema really is rebuilt — and `action` must come through it with
    /// the `validate` clause intact, because `validate` is still dispatchable in that session.
    /// MUTATION: force `let structured = true;` — the `usage-budgets` arm then reads
    /// `"Management/control action. Omit for execution mode."` where the `validate` clause must
    /// still be. Observed RED.
    #[test]
    fn only_disabling_workflow_scripts_drops_the_validate_clause_from_action() {
        let action_description = |surface: &crate::disabled_features::DisabledFeatureSurface| {
            subagent_tool_parameters_for(surface)["properties"]["action"]["description"]
                .as_str()
                .expect("action must keep a description through the reduction")
                .to_string()
        };

        let scripts_off =
            resolve_disabled_feature_surface(&[SubagentFeature::WorkflowScripts], true);
        assert!(scripts_off.contains(SubagentSurfaceFeature::Feature(
            SubagentFeature::WorkflowScripts
        )));
        assert_eq!(action_description(&scripts_off), ACTION_DESCRIPTION_BASE);
        let reduced = property_names(&subagent_tool_parameters_for(&scripts_off));
        for gone in ["workflow", "args"] {
            assert!(
                !reduced.iter().any(|name| name == gone),
                "disabling workflow-scripts must stop advertising {gone}"
            );
        }

        let budgets_off = resolve_disabled_feature_surface(&[SubagentFeature::UsageBudgets], true);
        assert!(
            !budgets_off.params().is_empty(),
            "the control arm must reach the reduction"
        );
        assert_eq!(
            action_description(&budgets_off),
            format!("{ACTION_DESCRIPTION_BASE}{ACTION_VALIDATE_CLAUSE}"),
            "a group that does not own `validate` must leave the clause in place"
        );
        let budget_reduced = property_names(&subagent_tool_parameters_for(&budgets_off));
        assert!(!budget_reduced.iter().any(|name| name == "usageBudget"));
        for kept in ["workflow", "args"] {
            assert!(
                budget_reduced.iter().any(|name| name == kept),
                "disabling usage-budgets must leave {kept} advertised"
            );
        }
    }

    /// SUBA-151's decision of record (option (b)) at the SCHEMA boundary: the reduced schema keeps
    /// this port's native `chain`/`tasks` and does not substitute pi's minimal
    /// `StructuredWorkflowProperties` pair (`extension/schemas.ts:287-299` @v0.75.0).
    ///
    /// pi re-adds a `{agent, task}`-only pair in the reduced schema because `0538e14d` (#2588,
    /// v0.74.0) left it with no top-level `chain`/`tasks` at all. This port kept the native graph,
    /// so the pair is already advertised in full — and emitting pi's minimal one here would REMOVE
    /// advertised capability the dispatcher still accepts
    /// (`chain_and_tasks_still_dispatch_to_the_native_graph_rather_than_a_removal_refusal` pins
    /// that it does), in the one session shape where a script-less orchestrator needs it most.
    ///
    /// Asserted as byte equality against the default entries plus the presence of item members pi's
    /// pair does not have, so a later "port the pair properly" edit fails here with the reason.
    /// MUTATION: add `props.shift_remove("tasks"); props.shift_remove("chain");` under
    /// `structured` — `tasks` reads `"null"` against the full entry. Observed RED.
    #[test]
    fn the_reduced_schema_keeps_the_native_chain_and_tasks_rather_than_pis_minimal_pair() {
        let full = subagent_tool_parameters();
        let scripts_off =
            resolve_disabled_feature_surface(&[SubagentFeature::WorkflowScripts], true);
        let reduced = subagent_tool_parameters_for(&scripts_off);

        for name in ["tasks", "chain"] {
            assert_eq!(
                schema_text(&reduced["properties"][name]),
                schema_text(&full["properties"][name]),
                "{name} must come through the reduction unchanged"
            );
        }

        // pi's `StructuredTask` is `{agent, task}` with `additionalProperties: false`; this port's
        // `tasks[]` item is a strict superset. Three members pi's pair cannot express:
        for member in ["count", "reads", "acceptance"] {
            assert!(
                reduced["properties"]["tasks"]["items"]["properties"]
                    .get(member)
                    .is_some(),
                "the reduced tasks[] item lost {member}, which pi's minimal pair has no slot for"
            );
        }
        // pi's reduced chain step is `{agent, task?, as?, parallel?}`; this port's carries the
        // dynamic-fanout surface too.
        for member in ["expand", "collect", "worktree"] {
            assert!(
                reduced["properties"]["chain"]["items"]["properties"]
                    .get(member)
                    .is_some(),
                "the reduced chain[] item lost {member}"
            );
        }
    }
}
