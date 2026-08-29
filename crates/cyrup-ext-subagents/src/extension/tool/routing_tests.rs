//! The dispatch table's own tests — every end-to-end `subagent` tool call this crate
//! exercises, from mode routing through the management/control verbs.
//!
//! A `#[path]` sibling rather than an inline `mod tests`: these 31 end-to-end cases are the
//! single largest test block in the tree, and inlining them would push `routing.rs` past
//! twice the size of any other module in `extension/`. The module path is unchanged —
//! `extension::tool::routing::tests` — so every test still lives in the module whose code it
//! exercises.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use super::*;
use cyrup_core::Tool;
use crate::background::control;
use crate::extension::testsupport::arm_scoped_missions;
use crate::extension::testsupport::dispatch_tool;
use crate::extension::testsupport::scoped_missions;
use crate::extension::testsupport::scoped_tool;
use crate::extension::testsupport::seed_running_run;
use crate::extension::testsupport::tool_text;
use crate::registration::SubagentExtensionConfig;
use cyrup_core::ToolCallId;
use std::sync::Arc;

/// Regression (pi `chain-execution.ts:584-596`, dossier "No upfront
/// validateChainOutputBindings for tool/slash chains; duplicate `as` silently overwrites"): a
/// tool `chain[]` call with two steps sharing the SAME `as` name must be rejected up front,
/// before any step (including its own agent-name resolution) is even attempted. Both step
/// agents here are unresolvable (`ghost-one`/`ghost-two`) precisely so that a pre-fix run would
/// instead reach `resolve_plan_personas` and fail with `SubagentError::AgentNotFound` — a
/// DIFFERENT error than this test asserts on — proving the new upfront validation now wins the
/// race.
#[tokio::test]
async fn chain_tool_call_rejects_duplicate_as_names_before_any_agent_resolution() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;

    let err = tool
        .execute(
            ToolCallId::from("t"),
            serde_json::json!({
                "chain": [
                    { "agent": "ghost-one", "task": "do a", "as": "shared" },
                    { "agent": "ghost-two", "task": "do b", "as": "shared" }
                ]
            }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect_err(
            "a duplicate `as` name across two chain[] steps must be rejected up front",
        );
    let message = err.to_string();
    assert!(
        message.contains("Duplicate chain output name 'shared'"),
        "must reject with pi's exact duplicate-output diagnostic, not 'agent not found: \
         ghost-one' (which a pre-fix run would surface instead): {message}"
    );
}

/// Companion regression: an `{outputs.x}` reference to an output NO strictly-earlier step
/// produces must also be rejected up front (pi's "Unknown chain output reference" diagnostic),
/// again proven via unresolvable agent names so a pre-fix run's DIFFERENT failure
/// (`AgentNotFound`, reached only once the referencing step's turn came up) would not
/// accidentally satisfy this assertion.
#[tokio::test]
async fn chain_tool_call_rejects_an_unknown_outputs_reference_before_any_agent_resolution() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;

    let err = tool
        .execute(
            ToolCallId::from("t"),
            serde_json::json!({
                "chain": [
                    { "agent": "ghost-one", "task": "Use {outputs.never_produced}" }
                ]
            }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect_err("an unknown {outputs.x} reference must be rejected up front");
    let message = err.to_string();
    assert!(
        message.contains("Unknown chain output reference '{outputs.never_produced}'"),
        "must reject with pi's exact unknown-reference diagnostic: {message}"
    );
}

/// G92's refusal gate, proved at the DISPATCHER, not at `format_fleet`.
///
/// `format_fleet(child_safe: true)` has a unit test of its own
/// (`fleet_view::tests::empty_fleet_renders_pis_sentinel_and_child_safe_refuses`), but that
/// test calls the function directly with `true`. The thing that actually decides a fanout
/// child cannot enumerate its parent's whole async root is ONE expression in
/// `route_control_action`'s `status` arm — `!self.allow_mutating_management` — and hardcoding
/// that argument to `false` leaves every existing test green while handing a fanout child the
/// full fleet. That is the mutation this test exists to fail on.
///
/// It drives the real `Tool::execute` on both registrations, so it is the argument, not the
/// callee, that is under test.
#[tokio::test]
async fn the_child_safe_registration_refuses_the_fleet_view_through_the_dispatcher() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    let call = serde_json::json!({ "action": "status", "view": "fleet" });

    let child_safe =
        SubagentTool::new_child_safe(executor.clone(), dir.path().to_path_buf());
    let refused = child_safe
        .execute(
            ToolCallId::from("fleet-child"),
            call.clone(),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await;
    let err = refused.expect_err(
        "a fanout child must be REFUSED the fleet view — it has no business enumerating its \
         parent's entire async root",
    );
    assert!(
        err.to_string().contains("Child-safe subagent fleet view is unavailable"),
        "the refusal must be pi's own child-safe fleet text, not some other failure; got: {err}"
    );

    // The control: the SAME call on the orchestrator registration must NOT be refused, so the
    // assertion above is really about the `child_safe` argument and not about the fleet view
    // being broken for everyone.
    let full = SubagentTool::new(executor, dir.path().to_path_buf());
    let allowed = full
        .execute(
            ToolCallId::from("fleet-full"),
            call,
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect("the orchestrator registration renders the fleet");
    let text = allowed
        .content
        .iter()
        .find_map(|c| match c {
            cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .unwrap_or_default();
    assert!(
        text.contains("No active subagent fleet"),
        "the orchestrator must get the real (here: empty) fleet surface; got: {text}"
    );
}

/// END-TO-END for pi `handleList`'s proactive block (`agent-management.ts:765-770,784` @v0.43.0): a real
/// `{ action: "list" }` tool call, over real on-disk agents and a real on-disk skill, must
/// render the `Proactive skill subagent suggestions:` block. This is the seam the recommender
/// was missing — the whole `proactive-skills.ts` port existed but nothing called it, so the
/// tool description's own line ("If { action: "list" } shows proactive skill subagent
/// suggestions, consider a small fresh-context fanout…") pointed at output that never appeared.
#[tokio::test]
async fn tool_list_renders_the_proactive_skill_subagent_suggestions_end_to_end() {
    let dir = tempfile::tempdir().expect("tempdir");
    let agents_dir = dir.path().join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir agents");
    for name in ["auditor-one", "auditor-two"] {
        std::fs::write(
            agents_dir.join(format!("{name}.md")),
            format!("---\nname: {name}\ndescription: An auditor\nskills: audit-trail\n---\nBody.\n"),
        )
        .expect("write agent");
    }
    let skill_dir = dir.path().join(".cyrup").join("skills").join("audit-trail");
    std::fs::create_dir_all(&skill_dir).expect("mkdir skill");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\ndescription: Trace every mutation.\n---\n\nHow to audit.\n",
    )
    .expect("write skill");

    let tool = scoped_tool(dir.path()).await;
    let out = tool
        .execute(
            ToolCallId::from("t"),
            serde_json::json!({ "action": "list" }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect("list is wired");
    let text = out
        .content
        .iter()
        .find_map(|c| match c {
            cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .unwrap_or_default();

    assert!(
        text.contains("Proactive skill subagent suggestions:"),
        "the block must reach the tool's rendered output:\n{text}"
    );
    assert!(
        text.contains("- audit-trail via reviewer (referenced by 2 configured agents/chains; agent:auditor-one, agent:auditor-two) - Trace every mutation."),
        "the recommendation must name the skill, the carrier agent, its reference count and its \
         sources, exactly as `formatProactiveSkillSubagentRecommendations` renders them:\n{text}"
    );
}

/// Dispatch discrimination: management/control/parallel/chain modes are each RECOGNIZED and
/// routed to their own arm rather than mis-parsed as a broken SINGLE call. Management/control
/// still short-circuit at their P1 stubs; parallel/chain now route to REAL execution, proven
/// here without any spawn by using an unresolvable agent so plan-time persona resolution fails
/// (`AgentNotFound`) before any child process — the assertion stays on the dispatch decision.
#[tokio::test]
async fn tool_execute_routes_each_mode_to_its_dispatch_arm() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;

    async fn dispatch(tool: &SubagentTool, params: serde_json::Value) -> Result<ToolResult, ToolError> {
        tool.execute(
            ToolCallId::from("t"),
            params,
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
    }

    // Management action → now wired (C3): `list` succeeds and renders the pi list shape,
    // proving the dispatch reached the real management arm rather than a stub.
    let mgmt_ok = dispatch(&tool, serde_json::json!({ "action": "list" }))
        .await
        .expect("management action 'list' is wired and returns the agent/chain listing");
    let mgmt_text = mgmt_ok
        .content
        .iter()
        .find_map(|c| match c {
            cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .unwrap_or_default();
    assert!(mgmt_text.contains("Executable agents:"), "got: {mgmt_text}");
    assert!(mgmt_text.contains("Chains:"), "got: {mgmt_text}");

    // Control action → now wired (C5): an unknown run id fails with the not-found notice,
    // proving the dispatch reached the real control arm rather than a stub.
    let control_err = dispatch(&tool, serde_json::json!({ "action": "status", "id": "run1" }))
        .await
        .expect_err("control action routes to real status, which fails on the unknown id");
    assert!(
        control_err.to_string().contains("Async run not found"),
        "got: {control_err}"
    );

    // PARALLEL (tasks[]) → parallel arm. Now routes through the REAL plan-execution path, so an
    // unresolvable agent fails at plan-time persona resolution (`AgentNotFound`) BEFORE any
    // spawn — proving the dispatch reached the parallel arm and its real routing, not a stub.
    let parallel_err = dispatch(&tool, serde_json::json!({ "tasks": [{ "agent": "x", "task": "y" }] }))
        .await
        .expect_err("tasks[] routes to real parallel execution, which fails on the unknown agent");
    assert!(
        parallel_err.to_string().contains("agent not found: x"),
        "got: {parallel_err}"
    );

    // CHAIN (chain[]) → chain arm, likewise failing at plan-time persona resolution.
    let chain_err = dispatch(&tool, serde_json::json!({ "chain": [{ "agent": "x", "task": "y" }] }))
        .await
        .expect_err("chain[] routes to real chain execution, which fails on the unknown agent");
    assert!(
        chain_err.to_string().contains("agent not found: x"),
        "got: {chain_err}"
    );

    // Unknown action → explicit unknown-action error listing the valid set.
    let unknown_err = dispatch(&tool, serde_json::json!({ "action": "frobnicate" }))
        .await
        .expect_err("an unknown action is rejected");
    // SUBA-038/SUBA-065: pi's own text (`unknownSubagentActionMessage`,
    // `subagent-executor.ts:195-208` @v0.47.1), not cyrup's former
    // "unknown subagent action '…'; valid actions are …".
    assert!(
        unknown_err.to_string().starts_with("Unknown action: frobnicate."),
        "got: {unknown_err}"
    );
    // The unknown-action message must enumerate the actions that DO dispatch, so a model that
    // guessed wrong is told the real set (SUBA-005 widened it by four).
    for action in crate::discovery::management::MANAGEMENT_ACTIONS {
        assert!(
            unknown_err.to_string().contains(action),
            "the unknown-action error must list '{action}'; got: {unknown_err}"
        );
    }
    // SUBA-038 residual 2: the four `watchdog.*` verbs DO dispatch, and the hand-written list
    // this replaced omitted all four.
    for action in crate::watchdog::tool_actions::WATCHDOG_TOOL_ACTIONS {
        assert!(
            unknown_err.to_string().contains(action),
            "the unknown-action error must list the dispatching '{action}'; got: {unknown_err}"
        );
    }
}

/// SUBA-005 dispatch proof, separated from the omnibus test above because the assertion is on
/// the handler's own text: each new verb reaches `handle_eject`/`handle_disable`/`handle_enable`/
/// `handle_reset` and answers with pi's verbatim "Specify 'agent' for &lt;verb&gt;." validation —
/// which is only reachable through the real handler. Pre-fix, `route_action` had no arm for any
/// of the four and answered "unknown subagent action '&lt;verb&gt;'" instead.
#[tokio::test]
async fn tool_execute_routes_the_four_suba_005_actions_to_their_real_handlers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;

    for verb in ["eject", "disable", "enable", "reset"] {
        let err = tool
            .execute(
                ToolCallId::from("t"),
                serde_json::json!({ "action": verb }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("a management action with no 'agent' is an error outcome");
        assert_eq!(
            err.to_string(),
            format!("Specify 'agent' for {verb}."),
            "action '{verb}' must be serviced by its own handler, not the unknown-action arm"
        );
    }
}

/// T6 regression (pi `MUTATING_MANAGEMENT_ACTIONS`, `subagent-executor.ts:151`): a fanout child
/// is refused ALL SEVEN mutating management actions — including the four SUBA-005 added — and
/// the refusal happens BEFORE any discovery or filesystem access, so a child cannot even probe
/// the parent's config through them. The read-only verbs are unaffected.
#[tokio::test]
async fn child_safe_tool_blocks_all_seven_mutating_management_actions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let child = SubagentTool::new_child_safe(
        Arc::new(SubagentExecutor::new()),
        dir.path().to_path_buf(),
    );

    for action in crate::discovery::management::MUTATING_MANAGEMENT_ACTIONS {
        let result = child
            .execute(
                ToolCallId::from("t"),
                serde_json::json!({ "action": action, "agent": "scout" }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await;
        let err = result.err().unwrap_or_else(|| {
            panic!("child-safe mode must refuse the mutating action '{action}'")
        });
        // SUBA-038: EQUALITY against pi's own text (`subagent-executor.ts:4867` @v0.43.0),
        // not a substring of cyrup's former wording — the substring assertion is what let the
        // divergence sit unnoticed.
        assert_eq!(
            err.to_string(),
            format!("Action '{action}' is not available from child-safe subagent fanout mode."),
            "action '{action}' must be refused by the T6 denylist with pi's exact text"
        );
    }

    // The read-only verbs still work in child-safe mode — the denylist is a denylist, not a
    // blanket management block.
    let listed = child
        .execute(
            ToolCallId::from("t"),
            serde_json::json!({ "action": "list" }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect("child-safe mode still permits the read-only 'list'");
    assert!(!listed.content.is_empty());
}

/// SUBA-064 — the `authorityPolicy` gate on the live `stop`/`steer` verbs.
///
/// THE USER ACTION: an operator writes `"authorityPolicy": {"stopRun": "forbid"}` into
/// `config.json`. Before the port the key was silently dropped — `registration/mod.rs`'s only
/// validator was `validate_missions` — and `{action:"stop", id}` executed anyway. Unlike most
/// gaps in this area the gated actions are already live in cyrup, so this was not a dormant
/// hole: it was a policy surface a user could configure and that did nothing.
///
/// Driven through the real tool so the gate is proven to sit BEFORE dispatch (pi
/// `subagent-executor.ts:4412-4423` @v0.43.0): the run id below does not exist, so an ungated
/// call fails with a run-not-found error — a forbid that produces the authority text instead is
/// proof the gate fired first.
#[tokio::test]
async fn a_forbidding_authority_policy_refuses_stop_before_it_dispatches() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    *executor.config_cell().lock().await = SubagentExtensionConfig {
            authority_policy: Some(crate::registration::authority::AuthorityPolicyConfig {
                stop_run: Some(crate::registration::authority::AuthorityDecision::Forbid),
                ..crate::registration::authority::AuthorityPolicyConfig::default()
            }),
        ..SubagentExtensionConfig::default()
    };
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());

    let err = dispatch_tool(&tool, serde_json::json!({ "action": "stop", "id": "nope" }))
        .await
        .expect_err("a forbidden action must refuse");
    assert_eq!(
        err.to_string(),
        "Authority policy forbids action 'stop'.",
        "pi's exact text (`subagent-executor.ts:4415`)"
    );

    // The mirror, and the half that proves the gate is not simply refusing everything: an
    // UNGATED control verb is untouched by the same policy and still reaches dispatch (which
    // fails on the unknown run, not on authority).
    let untouched = dispatch_tool(&tool, serde_json::json!({ "action": "interrupt", "id": "nope" }))
        .await
        .expect_err("the run does not exist");
    assert!(
        !untouched.to_string().contains("Authority policy"),
        "`interrupt` is not one of pi's AUTHORITY_ACTIONS: {untouched}"
    );

    // ...and `steer` under the SAME policy is likewise ungated, because only `stopRun` was set.
    let steer = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "steer", "id": "nope", "message": "hi" }),
    )
    .await
    .expect_err("the run does not exist");
    assert!(
        !steer.to_string().contains("Authority policy"),
        "an unconfigured action keeps its `auto` default: {steer}"
    );
}

/// SUBA-064's no-UI branch (pi `:4419`): `confirm` with nothing to confirm THROUGH is a
/// refusal, never a silent auto-grant. This test's executor has no host services attached,
/// which is exactly upstream's `!ctx.hasUI`.
#[tokio::test]
async fn a_confirming_authority_policy_refuses_when_the_session_has_no_ui() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    *executor.config_cell().lock().await = SubagentExtensionConfig {
            authority_policy: Some(crate::registration::authority::AuthorityPolicyConfig {
                steer_run: Some(crate::registration::authority::AuthorityDecision::Confirm),
                ..crate::registration::authority::AuthorityPolicyConfig::default()
            }),
        ..SubagentExtensionConfig::default()
    };
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());

    let err = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "steer", "id": "nope", "message": "hi" }),
    )
    .await
    .expect_err("no UI means no authority");
    assert_eq!(
        err.to_string(),
        "Authority policy requires user confirmation for action 'steer', but this session has \
         no interactive UI."
    );
}

/// SUBA-077: the top-level PARALLEL (`tasks: []`) surface used to hard-code its timeout argument
/// to `None`, so an explicit call-site `timeoutMs` was dropped on the floor — never validated,
/// never propagated. An INVALID one is the decisive observable: pre-fix it was silently ignored
/// and the call fell through to agent resolution; post-fix it must be REFUSED with the resolver's
/// own message, before any agent is resolved or any child spawned.
///
/// Both agents are deliberately unresolvable, so a regression cannot pass this by erroring for
/// some other reason — the assertion is on the message, and `AgentNotFound` does not contain it.
#[tokio::test]
async fn an_invalid_timeout_on_a_parallel_call_is_refused_rather_than_dropped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;

    let message = tool
        .execute(
            ToolCallId::from("par-timeout"),
            serde_json::json!({
                "tasks": [{ "agent": "ghost-one", "task": "a" }, { "agent": "ghost-two", "task": "b" }],
                "timeoutMs": 0
            }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect_err("0 is not a valid timeout")
        .to_string();
    assert!(
        message.contains("timeoutMs must be a positive integer."),
        "a parallel call's timeout must be resolved, not discarded; got {message}"
    );

    // The alias is resolved on this surface too, on exactly the same terms as SINGLE.
    let message = tool
        .execute(
            ToolCallId::from("par-timeout-alias"),
            serde_json::json!({
                "tasks": [{ "agent": "ghost-one", "task": "a" }],
                "timeoutMs": 10,
                "maxRuntimeMs": 20
            }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect_err("disagreeing aliases are refused")
        .to_string();
    assert!(
        message.contains("timeoutMs and maxRuntimeMs are aliases"),
        "got {message}"
    );
}

/// SUBA-077: `subagents.timeoutMs` reaches [`SubagentExtensionConfig`] off the wire under pi's own
/// camelCase key, and is carried RAW so an invalid value degrades to the built-in backstop instead
/// of failing the whole config's deserialization and taking every other setting with it.
#[test]
fn the_config_timeout_key_deserializes_raw_and_survives_a_garbage_value() {
    let cfg: SubagentExtensionConfig =
        serde_json::from_value(serde_json::json!({ "timeoutMs": 60_000 })).expect("config parses");
    assert_eq!(
        crate::extension::tool::params::foreground_timeout_default(
            false,
            Option::None,
            cfg.timeout_ms.as_ref()
        ),
        Some(60_000),
        "a valid `subagents.timeoutMs` must replace the built-in backstop"
    );

    let cfg: SubagentExtensionConfig =
        serde_json::from_value(serde_json::json!({ "timeoutMs": -5, "asyncByDefault": true }))
            .expect("a garbage timeoutMs must NOT fail the whole config");
    assert!(
        cfg.async_by_default,
        "the sibling setting must survive alongside the bad one"
    );
    assert_eq!(
        crate::extension::tool::params::foreground_timeout_default(
            false,
            Option::None,
            cfg.timeout_ms.as_ref()
        ),
        Some(crate::exec::DEFAULT_FOREGROUND_TIMEOUT_MS),
        "an invalid value degrades to the built-in backstop"
    );
}

/// SUBA-047's refusal half — pi `validateToolBudgetConfig(params.toolBudget, "toolBudget")`
/// (`runs/background/async-execution.ts:1299` @v0.43.0). A malformed budget must refuse the
/// call with the validator's own message; silently downgrading to "unbudgeted" is the same
/// silent-drop defect one layer down.
#[tokio::test]
async fn a_malformed_tool_budget_is_refused_on_both_single_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;

    for r#async in [false, true] {
        let message = tool
            .execute(
                ToolCallId::from(format!("tb-{async}").as_str()),
                serde_json::json!({
                    "agent": "ghost",
                    "task": "do it",
                    "async": r#async,
                    "toolBudget": { "hard": 0 }
                }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("hard: 0 is not a valid budget")
            .to_string();
        assert!(
            message.contains("toolBudget.hard must be an integer >= 1."),
            "async={async}: a malformed budget must be refused with the validator's own text, \
             not dropped; got {message}"
        );
    }

    // The mirror: a WELL-FORMED budget is not refused at all — it falls through to agent
    // resolution like any other honoured param.
    for r#async in [false, true] {
        let message = tool
            .execute(
                ToolCallId::from(format!("tb-ok-{async}").as_str()),
                serde_json::json!({
                    "agent": "ghost",
                    "task": "do it",
                    "async": r#async,
                    "toolBudget": { "hard": 3 },
                    "outputSchema": { "type": "object" }
                }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("the agent is unresolvable, so the call still errors")
            .to_string();
        assert!(
            message.contains("agent not found"),
            "async={async}: a valid toolBudget/outputSchema pair must reach agent resolution, \
             not be turned away at the router; got {message}"
        );
    }
}

/// R-SA-069 (pi `executeWithSingleDispatchGuard`, `subagent-executor.ts:5327-5348`): a second
/// non-`action` subagent call arriving while a prior one from the SAME tool instance is still in
/// flight is rejected outright with pi's exact text — never queued, never silently allowed to
/// run concurrently. Simulates "a prior dispatch is in progress" by holding the guard's one slot
/// directly (rather than actually racing two `execute` futures), which isolates the assertion to
/// the guard/rejection wiring itself. `action` calls remain unaffected (management/control
/// bypasses the guard entirely, pi's `if (params.action) return execute(...)` early return).
#[tokio::test]
async fn subagent_tool_rejects_a_second_concurrent_dispatch_while_one_is_in_flight() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;

    let _held = tool
        .dispatch_guard
        .try_acquire()
        .expect("the guard's single slot is free before any dispatch has run");

    let err = tool
        .execute(
            ToolCallId::from("t"),
            serde_json::json!({ "agent": "worker", "task": "do it" }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect_err("a second non-action call while one is in flight must be rejected outright");
    assert_eq!(
        err.to_string(),
        "Rejected: a subagent call is already in progress. Issue exactly ONE subagent call per turn."
    );

    // `action` calls are NEVER gated by the guard (pi's early return before the flag check).
    let action_err = tool
        .execute(
            ToolCallId::from("t"),
            serde_json::json!({ "action": "status", "id": "run1" }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect_err("action calls resolve to the real control arm, which fails on the unknown id");
    assert!(
        action_err.to_string().contains("Async run not found"),
        "an `action` call must bypass the dispatch guard entirely, got: {action_err}"
    );
}

/// pi `validateExecutionInput`'s mode-exclusivity gate (`subagent-executor.ts:1736-1754`,
/// `hasChain`/`hasTasks`/`hasSingle` at `2995-2997`): mode is selected by a NON-EMPTY array, not
/// merely the field's presence — an explicit `tasks: []` or `chain: []` (with no `agent`) must
/// fall through to "Provide exactly one mode", never silently execute as an empty parallel run
/// (which would previously report a vacuous "0/0 succeeded") or an empty chain.
#[tokio::test]
async fn subagent_tool_rejects_empty_tasks_and_chain_arrays_as_no_mode_selected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;

    async fn dispatch(tool: &SubagentTool, params: serde_json::Value) -> Result<ToolResult, ToolError> {
        tool.execute(
            ToolCallId::from("t"),
            params,
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
    }

    let empty_tasks_err = dispatch(&tool, serde_json::json!({ "tasks": [] }))
        .await
        .expect_err("an explicit empty tasks[] must error rather than run as an empty parallel group");
    assert!(
        empty_tasks_err.to_string().starts_with("Provide exactly one mode. Agents:"),
        "got: {empty_tasks_err}"
    );

    let empty_chain_err = dispatch(&tool, serde_json::json!({ "chain": [] }))
        .await
        .expect_err("an explicit empty chain[] must error rather than run as an empty chain");
    assert!(
        empty_chain_err.to_string().starts_with("Provide exactly one mode. Agents:"),
        "got: {empty_chain_err}"
    );
}

/// pi `params.id ?? params.runId` (`subagent-executor.ts:2846`): a caller using `runId` alone
/// (no `id`) for `action: "status"` must still resolve to THAT run's own report — surfacing its
/// specific not-found error — rather than silently falling through to the no-id "list active
/// runs" view (which would return an `Ok` empty-list result instead of this `Err`).
#[tokio::test]
async fn control_status_action_uses_run_id_when_id_is_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;
    let err = tool
        .execute(
            ToolCallId::from("t"),
            serde_json::json!({ "action": "status", "runId": "run1" }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect_err("a runId-only status call must resolve to that run's own not-found report");
    assert!(
        err.to_string().contains("Async run not found"),
        "got: {err}; a `runId`-only status call must not silently degrade to the no-id \
         \"list active runs\" view"
    );
}

/// pi `run-status.ts:104-110`: the child-safe fanout tool's `{ action: "status" }` call with no
/// id/runId/dir must hard-error with pi's exact message rather than listing the cwd's active
/// runs. Pre-fix, `SubagentTool::new_child_safe` had no way to signal this to `control_status`,
/// so this dispatch would have returned `Ok` with the "No active async runs." list instead of
/// this `Err`.
#[tokio::test]
async fn child_safe_tool_status_with_no_id_hard_errors_instead_of_listing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = SubagentTool::new_child_safe(Arc::new(SubagentExecutor::new()), dir.path().to_path_buf());
    let err = tool
        .execute(
            ToolCallId::from("t"),
            serde_json::json!({ "action": "status" }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect_err("child-safe no-id status must hard-error");
    assert_eq!(
        err.to_string(),
        "Child-safe subagent status requires an id when no foreground run is active."
    );
}

/// pi `resolveRequestedCwd` (`subagent-executor.ts:348-350,4334` @v0.43.0): an explicit `cwd` param
/// must be resolved and threaded into the dispatch's own discovery, not silently ignored in
/// favor of the tool's construction-time cwd. Proven end-to-end with the read-only `get`
/// management action (no process spawn, so safe to drive to completion): an agent that exists
/// ONLY under a disjoint `cwd` param is found when — and only when — that `cwd` is honored.
#[tokio::test]
async fn subagent_tool_cwd_param_is_resolved_and_threaded_into_dispatch() {
    let dir_a = tempfile::tempdir().expect("tempdir a");
    let dir_b = tempfile::tempdir().expect("tempdir b");
    let agents_dir_b = dir_b.path().join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir_b).expect("mkdir dirB agents");
    std::fs::write(
        agents_dir_b.join("beta.md"),
        "---\nname: beta\ndescription: Only discoverable under dirB\n---\nBody.\n",
    )
    .expect("write dirB agent fixture");

    let tool = scoped_tool(dir_a.path()).await;

    async fn dispatch(tool: &SubagentTool, params: serde_json::Value) -> Result<ToolResult, ToolError> {
        tool.execute(
            ToolCallId::from("t"),
            params,
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
    }

    // Without an explicit `cwd`, discovery runs over the tool's construction-time `self.cwd`
    // (dirA), which has no "beta" agent.
    let without_cwd = dispatch(&tool, serde_json::json!({ "action": "get", "agent": "beta" }))
        .await
        .expect_err("dirA has no 'beta' agent, so 'get' must fail absent an explicit cwd");
    assert!(without_cwd.to_string().contains("not found"), "got: {without_cwd}");

    // With an explicit `cwd` pointing at dirB, discovery must run over dirB instead — finding
    // "beta". Pre-fix, `cwd` was parsed and discarded, so this would ALSO have failed exactly
    // like the call above (self.cwd never changes).
    let ok = dispatch(
        &tool,
        serde_json::json!({
            "action": "get",
            "agent": "beta",
            "cwd": dir_b.path().to_string_lossy(),
        }),
    )
    .await
    .expect("an explicit cwd must be resolved and fed into discovery, finding dirB's agent");
    let text = ok
        .content
        .iter()
        .find_map(|c| match c {
            cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .unwrap_or_default();
    assert!(text.contains("beta"), "got: {text}");
}

// =============================================================================================
// DURABLE MISSIONS (pi-subagents/src/missions/) — the WIRING tests.
//
// `missions/*.rs` carry the unit tests for the subsystem's own behaviour; these prove the
// three production seams that reach it actually reach it: the `mission.*` action arm of
// `route_action`, the launch binding wrapped around `execute`'s three mode arms, and the
// `AgentEnd` goal scan.
// =============================================================================================

/// The six `mission.*` actions are dispatched by a REAL tool call, through the same
/// `SubagentTool::execute` -> `route_action` path every other action uses (pi
/// `subagent-executor.ts:4397-4407`). Pre-wiring this call returned
/// `unknown subagent action 'mission.create'`.
#[tokio::test]
async fn mission_actions_are_dispatched_from_a_real_tool_call() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());

    let created = dispatch_tool(
        &tool,
        serde_json::json!({
            "action": "mission.create",
            "mission": { "title": "Ship the port", "objective": "finish the mission port" },
        }),
    )
    .await
    .expect("mission.create must dispatch");
    let text = tool_text(&created);
    assert!(text.starts_with("Created mission "), "{text}");
    assert!(text.ends_with(": Ship the port"), "{text}");
    let details = created.details.as_ref().expect("details");
    let mission_id =
        details["missionId"].as_str().expect("missionId on details").to_string();
    assert_eq!(details["mode"], "management");
    assert_eq!(details["mission"]["objective"], "finish the mission port");
    // The record really landed on disk, under the rebranded project directory.
    assert!(
        dir.path()
            .join(".cyrup-subagents")
            .join("missions")
            .join(format!("{mission_id}.json"))
            .exists(),
        "mission.create must persist a record"
    );

    let listed = dispatch_tool(&tool, serde_json::json!({ "action": "mission.list" }))
        .await
        .expect("mission.list must dispatch");
    assert!(tool_text(&listed).contains(&mission_id), "{}", tool_text(&listed));

    let shown = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "mission.show", "missionId": mission_id }),
    )
    .await
    .expect("mission.show must dispatch");
    assert!(tool_text(&shown).contains("Title: Ship the port"), "{}", tool_text(&shown));

    let updated = dispatch_tool(
        &tool,
        serde_json::json!({
            "action": "mission.update",
            "missionId": mission_id,
            "missionUpdate": { "summary": "half done", "labels": ["port"] },
        }),
    )
    .await
    .expect("mission.update must dispatch");
    assert!(tool_text(&updated).contains("Summary: half done"), "{}", tool_text(&updated));

    let attached = dispatch_tool(
        &tool,
        serde_json::json!({
            "action": "mission.attach-run",
            "missionId": mission_id,
            "runId": "external-run-1",
            "runMode": "external",
        }),
    )
    .await
    .expect("mission.attach-run must dispatch");
    assert_eq!(
        tool_text(&attached),
        format!("Attached run external-run-1 to mission {mission_id}.")
    );

    let closed = dispatch_tool(
        &tool,
        serde_json::json!({
            "action": "mission.close",
            "missionId": mission_id,
            "missionStatus": "completed",
            "summary": "shipped",
        }),
    )
    .await
    .expect("mission.close must dispatch");
    assert_eq!(tool_text(&closed), format!("Closed mission {mission_id} as completed."));
}

/// A mission action's validation failure surfaces as cyrup's error channel (`Err(ToolError)`)
/// carrying upstream's exact refusal text, not as a silently-successful result.
#[tokio::test]
async fn a_mission_action_validation_failure_is_a_tool_error_with_upstreams_text() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;
    let err = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "mission.create", "mission": { "nope": 1 } }),
    )
    .await
    .expect_err("an unknown mission key must refuse");
    assert_eq!(err.to_string(), "mission.nope is unknown");

    let err = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "mission.list", "missionScope": "galactic" }),
    )
    .await
    .expect_err("an unknown scope must refuse");
    assert_eq!(err.to_string(), "missionScope must be \"project\" or \"global\"");
}

/// T6 child-safe restriction (pi `subagent-executor.ts:4379-4386`, over the four mission
/// actions in `MUTATING_MANAGEMENT_ACTIONS` at `:151`): a fanout child may LIST and SHOW
/// missions but may not create/update/attach/close one.
#[tokio::test]
async fn child_safe_mission_gating_matches_upstreams_mutating_set() {
    let dir = tempfile::tempdir().expect("tempdir");
    let child = SubagentTool::new_child_safe(
        Arc::new(SubagentExecutor::new()),
        dir.path().to_path_buf(),
    );
    for action in ["mission.create", "mission.update", "mission.attach-run", "mission.close"] {
        let err = dispatch_tool(
            &child,
            serde_json::json!({ "action": action, "missionId": "m", "runId": "r",
                                "mission": {"title": "t"}, "missionUpdate": {"summary": "s"} }),
        )
        .await
        .expect_err("a mutating mission action must be refused in child-safe mode");
        assert_eq!(
            err.to_string(),
            format!("Action '{action}' is not available from child-safe subagent fanout mode.")
        );
    }
    // `mission.list` is read-only: it reaches the handler and renders the empty list.
    let listed = dispatch_tool(&child, serde_json::json!({ "action": "mission.list" }))
        .await
        .expect("mission.list is read-only and must be permitted");
    assert_eq!(tool_text(&listed), "No project missions.");
}

/// The LAUNCH binding (pi `subagent-executor.ts:5100-5127`): an execution call carrying an
/// explicit `mission` object creates the mission BEFORE the run and folds the settled result
/// back onto it AFTER. Driven here through the real `execute` seam with a run that fails at
/// agent resolution — which is precisely the case that proves the binding is applied to the
/// ERROR arm too (cyrup's `Err(ToolError)`), where upstream applies it to an `isError` result.
#[tokio::test]
async fn an_execution_call_with_an_explicit_mission_binds_before_the_run_and_settles_after() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());
    let err = dispatch_tool(
        &tool,
        serde_json::json!({
            "agent": "no-such-agent-anywhere",
            "task": "do the thing",
            "mission": { "title": "Bound", "objective": "prove the binding" },
        }),
    )
    .await
    .expect_err("an unresolvable agent still fails the call");
    assert!(err.to_string().contains("no-such-agent-anywhere"), "{err}");

    // The mission was created up front (so the run is attributable even though it failed) and
    // then marked failed by the settle half, with the failure text as its summary.
    let location = crate::missions::resolve_mission_store_location(
        dir.path(),
        Some(&scoped_missions(dir.path())),
        None,
    );
    let listed = crate::missions::list_missions(&location);
    assert_eq!(listed.records.len(), 1, "{:?}", listed.records);
    let record = &listed.records[0];
    assert_eq!(record.title, "Bound");
    assert_eq!(record.objective, "prove the binding");
    assert_eq!(record.status, crate::missions::MissionStatus::Failed);
    assert!(record.summary.as_deref().is_some_and(|s| s.contains("no-such-agent-anywhere")));
}

/// `mission: false` is the explicit per-call opt-out (`missions/lifecycle.ts:63`): no mission
/// is created even though the call carries a task.
#[tokio::test]
async fn mission_false_suppresses_the_automatic_launch_binding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());
    let _ = dispatch_tool(
        &tool,
        serde_json::json!({
            "agent": "no-such-agent-anywhere",
            "task": "do the thing",
            "mission": false,
        }),
    )
    .await;
    let location = crate::missions::resolve_mission_store_location(
        dir.path(),
        Some(&scoped_missions(dir.path())),
        None,
    );
    assert!(crate::missions::list_missions(&location).records.is_empty());
}

/// `missionId` naming a mission that does not exist is FATAL to the call (the caller asked for
/// mission tracking explicitly), and the run never starts.
#[tokio::test]
async fn an_explicit_missing_mission_id_fails_the_call_before_the_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;
    let err = dispatch_tool(
        &tool,
        serde_json::json!({
            "agent": "no-such-agent-anywhere",
            "task": "t",
            "missionId": "does-not-exist",
        }),
    )
    .await
    .expect_err("a missing explicit mission must fail the call");
    assert!(err.to_string().starts_with("Mission 'does-not-exist' was not found in "), "{err}");
}

/// `missionId` and `mission` together are refused before anything runs.
#[tokio::test]
async fn mission_id_and_mission_together_are_refused_at_the_tool_boundary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;
    let err = dispatch_tool(
        &tool,
        serde_json::json!({
            "agent": "a", "task": "t",
            "missionId": "m", "mission": { "title": "t" },
        }),
    )
    .await
    .expect_err("both must refuse");
    assert_eq!(err.to_string(), "Use missionId or mission, not both");
}

/// G92, the whole `view: "fleet"` surface driven from a real tool call. Pre-fix the schema
/// carried no `view` property at all and `SubagentToolParams` had no field to deserialize it
/// into, so this exact call rendered the ordinary `Active async runs:` list — which is what the
/// negative assertions below pin.
#[tokio::test]
async fn status_view_fleet_renders_the_fleet_surface_not_the_plain_active_run_list() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_running_run(dir.path(), "fleetrun0001", &["scout"]);
    let tool = scoped_tool(dir.path()).await;

    let out = dispatch_tool(&tool, serde_json::json!({ "action": "status", "view": "fleet" }))
        .await
        .expect("view=fleet must render");
    let text = tool_text(&out);
    assert!(text.starts_with("Subagent fleet: 1 active"), "{text}");
    assert!(text.contains("Async runs:"), "{text}");
    assert!(
        text.contains("  transcript: subagent({ action: \"status\", id: \"fleetrun0001\", view: \"transcript\" })"),
        "the fleet view must emit pi's per-run transcript command hint: {text}"
    );
    assert!(
        text.contains("  Refresh fleet: subagent({ action: \"status\", view: \"fleet\" })"),
        "{text}"
    );
    assert!(
        !text.contains("Active async runs:"),
        "view=fleet must NOT fall through to the plain no-id list (the pre-fix behaviour): {text}"
    );
}

/// G92: `view: "transcript"` + `lines` really tail the child's output log, and the `lines`
/// budget is really applied. Pre-fix neither property was advertised or parsed.
#[tokio::test]
async fn status_view_transcript_tails_the_child_log_under_the_lines_budget() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = seed_running_run(dir.path(), "tailrun00001", &["scout"]);
    std::fs::write(paths.step_output_log(0), "alpha\nbeta\ngamma\n").expect("write output log");
    let tool = scoped_tool(dir.path()).await;

    let out = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "status", "id": "tailrun00001", "view": "transcript", "lines": 2 }),
    )
    .await
    .expect("view=transcript must render");
    let text = tool_text(&out);
    assert!(text.contains("Run: tailrun00001"), "{text}");
    assert!(text.contains("Step: 0 (scout)"), "{text}");
    assert!(text.contains("  beta"), "{text}");
    assert!(text.contains("  gamma"), "{text}");
    assert!(
        !text.contains("  alpha"),
        "lines=2 must drop the oldest line — a dropped `lines` param would keep it: {text}"
    );
    assert!(
        !text.contains("Progress:"),
        "transcript is a DIFFERENT view, not the ordinary status report: {text}"
    );
}

/// G77 — the LIVE tool path: `subagent({ action: "stop", id })` must reach
/// [`SubagentExecutor::control_stop`] and write a REAL `control/stop.json` into the run's
/// control inbox (pi `stopAsyncRun` → `deliverStopRequest`, `async-stop-action.ts:47`), with
/// pi's verbatim success text.
///
/// This drives `Tool::execute` end to end rather than calling `control_stop` directly, so it
/// covers the whole advertise-then-dispatch chain the schema promises the model.
#[tokio::test]
async fn the_stop_action_dispatches_and_writes_a_real_stop_request() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = seed_running_run(dir.path(), "stoptool0001", &["scout"]);
    let tool = scoped_tool(dir.path()).await;

    let out = dispatch_tool(&tool, serde_json::json!({ "action": "stop", "id": "stoptool0001" }))
        .await
        .expect("action='stop' must dispatch");
    assert_eq!(tool_text(&out), "Stop requested for async run stoptool0001.");

    let request = crate::background::control::stop_request_path(&paths.run_dir);
    assert!(request.exists(), "a real stop request must land at {}", request.display());
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&request).expect("read")).expect("valid json");
    assert_eq!(raw["type"], serde_json::json!("stop"));
    assert_eq!(raw["source"], serde_json::json!("stop-action"));
}

/// G77 — a unique run-id PREFIX stops the run it names, and the confirmation cites the run's
/// full id.
///
/// pi hands `stopAsyncRun` `resolved?.kind === "async" ? resolved.id : targetRunId`
/// (`subagent-executor.ts:4804-4808` @v0.43.0), and `resolveSubagentRunId`'s prefix pass
/// (`run-id-resolver.ts:84-86`) is what makes `resolved.id` the FULL id for an abbreviated
/// selector. Without that substitution the abbreviation is carried straight into the async
/// store, which knows no such run, and a caller who addressed the run by prefix everywhere else
/// (`status`, `interrupt`, `steer` all resolve prefixes) is told their id does not exist.
#[tokio::test]
async fn a_run_id_prefix_stops_the_run_it_names_and_the_confirmation_uses_the_full_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = seed_running_run(dir.path(), "stopprefix01", &["scout"]);
    let tool = scoped_tool(dir.path()).await;

    let out = dispatch_tool(&tool, serde_json::json!({ "action": "stop", "id": "stoppre" }))
        .await
        .expect("a unique prefix must resolve, not be reported missing");
    assert_eq!(
        tool_text(&out),
        "Stop requested for async run stopprefix01.",
        "the confirmation names the RESOLVED run, never the abbreviation the caller typed"
    );
    assert!(
        crate::background::control::stop_request_path(&paths.run_dir).exists(),
        "and the request lands in the resolved run's own control inbox"
    );
}

/// The other half of the advertise-vs-dispatch invariant for `stop`, under the CHILD-SAFE
/// registration: dropping `stop` from
/// [`CHILD_SAFE_SUBAGENT_TOOL_DESCRIPTION`] is an advertising change and must not become a
/// capability change.
///
/// Upstream reaches the same conclusion by construction — `stop` is absent from
/// `MUTATING_MANAGEMENT_ACTIONS` (`subagent-executor.ts:151` @v0.43.0, all 26 entries), so
/// `allowMutatingManagementActions: false` (`fanout-child.ts:171`) never gates it — but nothing
/// here pinned that, so a "tidy-up" that moved `stop` onto the denylist alongside the
/// description edit would have gone unnoticed. This drives the real `Tool::execute` on a
/// `new_child_safe` tool and asserts a REAL stop request lands on disk.
#[tokio::test]
async fn child_safe_mode_still_dispatches_the_unadvertised_stop_action() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = seed_running_run(dir.path(), "childsafestop", &["scout"]);
    let child_safe =
        SubagentTool::new_child_safe(Arc::new(SubagentExecutor::new()), dir.path().to_path_buf());

    let out = dispatch_tool(
        &child_safe,
        serde_json::json!({ "action": "stop", "id": "childsafestop" }),
    )
    .await
    .expect("a fanout child may stop a run even though the description does not offer it");
    assert_eq!(tool_text(&out), "Stop requested for async run childsafestop.");
    assert!(
        crate::background::control::stop_request_path(&paths.run_dir).exists(),
        "the child-safe dispatch must write the SAME real control request the root one does"
    );

    // …and the mutating actions the description DOES name are still refused, so the test above
    // is not passing because the child-safe gate stopped working altogether.
    let err = dispatch_tool(&child_safe, serde_json::json!({ "action": "delete", "agent": "x" }))
        .await
        .expect_err("a fanout child must not delete an agent");
    // SUBA-038: pi's exact text, asserted by equality.
    assert_eq!(
        err.to_string(),
        "Action 'delete' is not available from child-safe subagent fanout mode.",
        "{err}"
    );
}

/// G92: pi validates the view name before anything else (`run-status.ts:192-198`), so a typo
/// reports the typo instead of silently rendering the ordinary report.
#[tokio::test]
async fn an_unknown_status_view_is_rejected_with_pis_message() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_running_run(dir.path(), "typorun00001", &["scout"]);
    let tool = scoped_tool(dir.path()).await;
    let err = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "status", "id": "typorun00001", "view": "flee" }),
    )
    .await
    .expect_err("an unknown view must be refused");
    assert!(
        err.to_string().contains("Unknown status view: flee. Valid: fleet, transcript."),
        "{err}"
    );
}

/// G92: with no id and more than one active run, `view: "transcript"` cannot guess
/// (`run-status.ts:213-219`) — and with exactly one it resolves to that run.
#[tokio::test]
async fn no_id_transcript_resolves_a_lone_run_and_refuses_an_ambiguous_fleet() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = seed_running_run(dir.path(), "lonerun00001", &["scout"]);
    std::fs::write(paths.step_output_log(0), "only line\n").expect("write output log");
    let tool = scoped_tool(dir.path()).await;

    let text = tool_text(
        &dispatch_tool(&tool, serde_json::json!({ "action": "status", "view": "transcript" }))
            .await
            .expect("a lone active run resolves without an id"),
    );
    assert!(text.contains("Run: lonerun00001"), "{text}");
    assert!(text.contains("  only line"), "{text}");

    seed_running_run(dir.path(), "otherrun0001", &["scout"]);
    let err = dispatch_tool(&tool, serde_json::json!({ "action": "status", "view": "transcript" }))
        .await
        .expect_err("two active runs cannot be disambiguated");
    assert!(
        err.to_string()
            .contains("Transcript view requires an id when 2 active async runs exist."),
        "{err}"
    );
}

/// G90: the whole point of the verb — a tool call really writes a request into the run's
/// control inbox, where the runner's steer router picks it up. Pre-fix `steer` was not in the
/// action enum and `route_action` answered "unknown subagent action 'steer'".
#[tokio::test]
async fn steer_action_writes_a_control_inbox_request_for_a_running_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = seed_running_run(dir.path(), "steerrun0001", &["scout"]);
    let tool = scoped_tool(dir.path()).await;

    let text = tool_text(
        &dispatch_tool(
            &tool,
            serde_json::json!({
                "action": "steer", "id": "steerrun0001", "message": "  prefer the smaller diff  "
            }),
        )
        .await
        .expect("steering a running run must be accepted"),
    );
    // SUBA-049 replaced the cyrup-original "Steering queued for async run … Delivery requires a
    // live Cyrup child session …" with upstream's own sentence — `Steering ${state} for async
    // run ${status.runId} (request ${requestId}).` (`runs/foreground/async-steering-action.ts:138`,
    // `:148`). This test was left on the old string and is corrected here rather than deleted,
    // because the thing it exists to prove (the request really lands in the control inbox) is
    // still asserted below.
    //
    // The state is `pending`: no runner is alive in this test to write an ack, so
    // `await_steer_ack` polls out its 3 s window — which is the honest answer, and is exactly
    // the distinction the old text could not draw, since it claimed "queued" unconditionally.
    // The request id is minted per call (a monotonic sequence plus a uuid), so the assertion
    // matches on the parts that are contractual and not on the id's bytes.
    assert!(
        text.starts_with("Steering pending for async run steerrun0001 (request "),
        "{text}"
    );
    assert!(text.ends_with(").",), "{text}");

    let queue = control::steer_requests_dir(&paths.run_dir);
    let written: Vec<_> = std::fs::read_dir(&queue)
        .expect("the steer queue directory must exist")
        .filter_map(Result::ok)
        .collect();
    assert_eq!(written.len(), 1, "exactly one request file must be written");
    let raw = std::fs::read_to_string(written[0].path()).expect("read request");
    let request: control::SteerRequest = serde_json::from_str(&raw).expect("parse request");
    assert_eq!(request.kind, "steer");
    assert_eq!(
        request.message, "prefer the smaller diff",
        "the message must be stored TRIMMED"
    );
    assert_eq!(request.target_index, None);
    assert_eq!(request.source.as_deref(), Some("steer-action"));
}

/// SUBA-N04: pi validates a control action's acceptance too, with its own prefix, BEFORE the
/// action touches disk — `appendStepToRun` (`subagent-executor.ts:791-798`) and `resumeAsyncRun`
/// (`:1145-1152`) @v0.34.0. cyrup validated neither, which was invisible while the appended
/// step's policy was being dropped by the runner anyway; now that it is honoured, a malformed
/// one must be refused rather than enqueued.
#[tokio::test]
async fn a_control_action_refuses_a_malformed_acceptance_with_pis_own_prefix() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;

    let appended = tool
        .execute(
            ToolCallId::from("append-bad-acceptance"),
            serde_json::json!({
                "action": "append-step",
                "id": "run00000000",
                "chain": [{ "agent": "worker", "task": "t", "acceptance": "nonsense" }]
            }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect_err("a malformed appended-step acceptance must be refused")
        .to_string();
    assert_eq!(
        appended,
        "Cannot append step: chain[0].acceptance has invalid level 'nonsense'.",
        "pi's own prefix and per-site path label, and it must fire before the run lookup"
    );

    let resumed = tool
        .execute(
            ToolCallId::from("resume-bad-acceptance"),
            serde_json::json!({
                "action": "resume",
                "id": "run00000000",
                "message": "continue",
                "chain": [{ "agent": "worker", "task": "t", "acceptance": { "bogus": 1 } }]
            }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect_err("a malformed attach-chain acceptance must be refused")
        .to_string();
    assert_eq!(
        resumed,
        "Cannot resume: chain[0].acceptance.bogus is not supported."
    );
}
