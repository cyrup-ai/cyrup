//! The dispatch table's own tests — every end-to-end `subagent` tool call this crate
//! exercises, from mode routing through the management/control verbs.
//!
//! A `#[path]` sibling rather than an inline `mod tests`: these 31 end-to-end cases are the
//! single largest test block in the tree, and inlining them would push `routing.rs` past
//! twice the size of any other module in `extension/`. The module path is unchanged —
//! `extension::tool::routing::tests` — so every test still lives in the module whose code it
//! exercises.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::*;
use crate::background::control;
use crate::extension::SubagentExecutor;
use crate::extension::testsupport::arm_scoped_missions;
use crate::extension::testsupport::dispatch_tool;
use crate::extension::testsupport::scoped_missions;
use crate::extension::testsupport::scoped_tool;
use crate::extension::testsupport::seed_running_run;
use crate::extension::testsupport::tool_text;
use crate::registration::SubagentExtensionConfig;
use cyrup_core::Tool;
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
        .expect_err("a duplicate `as` name across two chain[] steps must be rejected up front");
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

    let child_safe = SubagentTool::new_child_safe(executor.clone(), dir.path().to_path_buf());
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
        err.to_string()
            .contains("Child-safe subagent fleet view is unavailable"),
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
            format!(
                "---\nname: {name}\ndescription: An auditor\nskills: audit-trail\n---\nBody.\n"
            ),
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

    async fn dispatch(
        tool: &SubagentTool,
        params: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
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
    let control_err = dispatch(
        &tool,
        serde_json::json!({ "action": "status", "id": "run1" }),
    )
    .await
    .expect_err("control action routes to real status, which fails on the unknown id");
    assert!(
        control_err.to_string().contains("Async run not found"),
        "got: {control_err}"
    );

    // PARALLEL (tasks[]) → parallel arm. Now routes through the REAL plan-execution path, so an
    // unresolvable agent fails at plan-time persona resolution (`AgentNotFound`) BEFORE any
    // spawn — proving the dispatch reached the parallel arm and its real routing, not a stub.
    let parallel_err = dispatch(
        &tool,
        serde_json::json!({ "tasks": [{ "agent": "x", "task": "y" }] }),
    )
    .await
    .expect_err("tasks[] routes to real parallel execution, which fails on the unknown agent");
    assert!(
        parallel_err.to_string().contains("agent not found: x"),
        "got: {parallel_err}"
    );

    // CHAIN (chain[]) → chain arm, likewise failing at plan-time persona resolution.
    let chain_err = dispatch(
        &tool,
        serde_json::json!({ "chain": [{ "agent": "x", "task": "y" }] }),
    )
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
        unknown_err
            .to_string()
            .starts_with("Unknown action: frobnicate."),
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
    let child =
        SubagentTool::new_child_safe(Arc::new(SubagentExecutor::new()), dir.path().to_path_buf());

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
    let untouched = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "interrupt", "id": "nope" }),
    )
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
            serde_json::json!({ "agent": "worker", "task": "do it", "async": false }),
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

/// SCOPE_18 (pi `executeWithSingleDispatchGuard`, `subagent-executor.ts:7209-7230` @ the current
/// tag): an effectively-async dispatch is EXEMPT from the single-dispatch guard — pi's second
/// early return (`if (!runsForeground) return execute(...)`) sits BEFORE the `subagentInProgress`
/// check, so a second `async: true` launch arriving while another dispatch is still in flight must
/// be accepted, not rejected with the duplicate-call text; only FOREGROUND dispatches serialize.
/// Same technique as the foreground test above: hold the guard's one slot directly (rather than
/// racing two real futures), isolating the assertion to the guard-gating wiring itself. The agent
/// is `ghost` — deliberately NOT the bundled builtin `worker`, which resolves in this environment
/// and would detach a real child run — so the dispatch errors at agent resolution, which is
/// downstream of the guard and therefore proves the bypass reached real dispatch logic.
#[tokio::test]
async fn subagent_tool_does_not_serialize_a_concurrent_async_dispatch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;

    // Simulate "a prior dispatch is in progress" exactly as the sibling foreground test does.
    let _held = tool
        .dispatch_guard
        .try_acquire()
        .expect("the guard's single slot is free before any dispatch has run");

    let err = tool
        .execute(
            ToolCallId::from("t"),
            serde_json::json!({ "agent": "ghost", "task": "do it", "async": true }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect_err("the agent is unresolvable, so the call still errors downstream");
    assert!(
        !err.to_string().contains("already in progress"),
        "an async dispatch must bypass the guard entirely (pi's `if (!runsForeground) return \
         execute(...)`), even while the guard's slot is held by something else; got: {err}"
    );
    assert!(
        err.to_string().contains("agent not found") || err.to_string().contains("Agent"),
        "having bypassed the guard, the call must reach real agent resolution, not stop early: {err}"
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

    async fn dispatch(
        tool: &SubagentTool,
        params: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
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
        .expect_err(
            "an explicit empty tasks[] must error rather than run as an empty parallel group",
        );
    assert!(
        empty_tasks_err
            .to_string()
            .starts_with("Provide exactly one mode. Agents:"),
        "got: {empty_tasks_err}"
    );

    let empty_chain_err = dispatch(&tool, serde_json::json!({ "chain": [] }))
        .await
        .expect_err("an explicit empty chain[] must error rather than run as an empty chain");
    assert!(
        empty_chain_err
            .to_string()
            .starts_with("Provide exactly one mode. Agents:"),
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
    let tool =
        SubagentTool::new_child_safe(Arc::new(SubagentExecutor::new()), dir.path().to_path_buf());
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

    async fn dispatch(
        tool: &SubagentTool,
        params: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
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
    let without_cwd = dispatch(
        &tool,
        serde_json::json!({ "action": "get", "agent": "beta" }),
    )
    .await
    .expect_err("dirA has no 'beta' agent, so 'get' must fail absent an explicit cwd");
    assert!(
        without_cwd.to_string().contains("not found"),
        "got: {without_cwd}"
    );

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
    let mission_id = details["missionId"]
        .as_str()
        .expect("missionId on details")
        .to_string();
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
    assert!(
        tool_text(&listed).contains(&mission_id),
        "{}",
        tool_text(&listed)
    );

    let shown = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "mission.show", "missionId": mission_id }),
    )
    .await
    .expect("mission.show must dispatch");
    assert!(
        tool_text(&shown).contains("Title: Ship the port"),
        "{}",
        tool_text(&shown)
    );

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
    assert!(
        tool_text(&updated).contains("Summary: half done"),
        "{}",
        tool_text(&updated)
    );

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
    assert_eq!(
        tool_text(&closed),
        format!("Closed mission {mission_id} as completed.")
    );
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
    assert_eq!(
        err.to_string(),
        "missionScope must be \"project\" or \"global\""
    );
}

/// SUBA-085 — `mission.resolve-decision` dispatches end to end through the tool
/// (pi `actions.ts:391-397` @v0.64.0): a decision recorded by `mission.update` is closed by id
/// with the resolution taken from `summary`, the receipt is upstream's `Resolved decision …`
/// line over the re-rendered mission, and an empty summary is refused with upstream's text.
/// Pre-fix `mission.resolve-decision` landed on the unknown-action arm.
#[tokio::test]
async fn mission_resolve_decision_dispatches_and_closes_the_decision() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;
    let created = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "mission.create", "mission": { "title": "Decide" } }),
    )
    .await
    .expect("mission.create must dispatch");
    let mission_id = created.details.as_ref().expect("details")["missionId"]
        .as_str()
        .expect("missionId")
        .to_string();
    let updated = dispatch_tool(
        &tool,
        serde_json::json!({
            "action": "mission.update",
            "missionId": mission_id,
            "missionUpdate": { "decisions": [{ "title": "Ship?" }] },
        }),
    )
    .await
    .expect("mission.update must dispatch");
    let decision_id = updated.details.as_ref().expect("details")["mission"]["decisions"][0]["id"]
        .as_str()
        .expect("decision id")
        .to_string();

    let err = dispatch_tool(
        &tool,
        serde_json::json!({
            "action": "mission.resolve-decision",
            "missionId": mission_id,
            "id": decision_id,
            "summary": "   ",
        }),
    )
    .await
    .expect_err("an empty summary must refuse");
    assert_eq!(
        err.to_string(),
        "mission.resolve-decision requires a non-empty summary"
    );

    let resolved = dispatch_tool(
        &tool,
        serde_json::json!({
            "action": "mission.resolve-decision",
            "missionId": mission_id,
            "id": decision_id,
            "summary": "yes, ship it",
        }),
    )
    .await
    .expect("mission.resolve-decision must dispatch");
    let text = tool_text(&resolved);
    assert!(
        text.starts_with(&format!(
            "Resolved decision {decision_id} for mission {mission_id}.\n\n"
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "  {decision_id}: resolved — Ship?; resolution: yes, ship it"
        )),
        "{text}"
    );
    let decision = &resolved.details.as_ref().expect("details")["mission"]["decisions"][0];
    assert_eq!(decision["status"], "resolved");
    assert_eq!(decision["resolution"], "yes, ship it");
    assert!(decision["resolvedAt"].is_string());
}

/// T6 child-safe restriction (pi `subagent-executor.ts:4379-4386`, over the five mission
/// actions in `MUTATING_MANAGEMENT_ACTIONS` at `:197` @v0.64.0): a fanout child may LIST and
/// SHOW missions but may not create/update/resolve/attach/close one.
#[tokio::test]
async fn child_safe_mission_gating_matches_upstreams_mutating_set() {
    let dir = tempfile::tempdir().expect("tempdir");
    let child =
        SubagentTool::new_child_safe(Arc::new(SubagentExecutor::new()), dir.path().to_path_buf());
    for action in [
        "mission.create",
        "mission.update",
        "mission.resolve-decision",
        "mission.attach-run",
        "mission.close",
    ] {
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
    assert!(
        record
            .summary
            .as_deref()
            .is_some_and(|s| s.contains("no-such-agent-anywhere"))
    );
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
    assert!(
        err.to_string()
            .starts_with("Mission 'does-not-exist' was not found in "),
        "{err}"
    );
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

    let out = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "status", "view": "fleet" }),
    )
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

    let out = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "stop", "id": "stoptool0001" }),
    )
    .await
    .expect("action='stop' must dispatch");
    assert_eq!(
        tool_text(&out),
        "Stop requested for async run stoptool0001."
    );

    let pending = crate::background::control::pending_stop_request_paths(&paths.run_dir).await;
    assert_eq!(
        pending.len(),
        1,
        "a real stop request must land under {}",
        crate::background::control::stop_requests_dir(&paths.run_dir).display()
    );
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&pending[0]).expect("read")).expect("valid json");
    assert_eq!(raw["type"], serde_json::json!("stop"));
    assert_eq!(raw["source"], serde_json::json!("stop-action"));
    assert!(
        raw.get("targetIndex").is_none(),
        "a plain stop is whole-run"
    );
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

    let out = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "stop", "id": "stoppre" }),
    )
    .await
    .expect("a unique prefix must resolve, not be reported missing");
    assert_eq!(
        tool_text(&out),
        "Stop requested for async run stopprefix01.",
        "the confirmation names the RESOLVED run, never the abbreviation the caller typed"
    );
    assert!(
        crate::background::control::has_pending_stop_request(&paths.run_dir).await,
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
    assert_eq!(
        tool_text(&out),
        "Stop requested for async run childsafestop."
    );
    assert!(
        crate::background::control::has_pending_stop_request(&paths.run_dir).await,
        "the child-safe dispatch must write the SAME real control request the root one does"
    );

    // …and the mutating actions the description DOES name are still refused, so the test above
    // is not passing because the child-safe gate stopped working altogether.
    let err = dispatch_tool(
        &child_safe,
        serde_json::json!({ "action": "delete", "agent": "x" }),
    )
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
        err.to_string()
            .contains("Unknown status view: flee. Valid: fleet, transcript."),
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
        &dispatch_tool(
            &tool,
            serde_json::json!({ "action": "status", "view": "transcript" }),
        )
        .await
        .expect("a lone active run resolves without an id"),
    );
    assert!(text.contains("Run: lonerun00001"), "{text}");
    assert!(text.contains("  only line"), "{text}");

    seed_running_run(dir.path(), "otherrun0001", &["scout"]);
    let err = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "status", "view": "transcript" }),
    )
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
        appended, "Cannot append step: chain[0].acceptance has invalid level 'nonsense'.",
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

/// SUBA-086 — pi `canonicalizeAgentName` (`subagent-executor.ts:2336-2344` @v0.64.0) consults
/// `findBlockingAgentDiagnostic` BEFORE the ambiguity/unknown branches, so a dispatch naming an
/// agent whose highest-ranked definition is malformed is refused with the parse error — never
/// silently routed to the lower-tier (here: bundled builtin) `worker`, and never reported as
/// "not found" when the broken file is the ONLY definition.
#[tokio::test]
async fn dispatch_refuses_an_agent_whose_outranking_definition_is_malformed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project_agents = dir.path().join(".cyrup").join("agents");
    std::fs::create_dir_all(&project_agents).expect("mkdir");
    std::fs::write(
        project_agents.join("worker.md"),
        "---\nname: worker\ndescription: d\ntimeoutMs: 30s\n---\n\nBody\n",
    )
    .expect("write broken worker");
    std::fs::write(
        project_agents.join("ghost.md"),
        "---\nname: ghost\ndescription: d\nfast: yes\n---\n\nBody\n",
    )
    .expect("write broken ghost");
    let tool = scoped_tool(dir.path()).await;

    let worker = dispatch_tool(
        &tool,
        serde_json::json!({ "agent": "worker", "task": "do it" }),
    )
    .await
    .expect_err("the broken project worker outranks the builtin worker")
    .to_string();
    assert!(
        worker.contains(
            "Agent 'worker' has invalid configuration: Agent 'worker' has invalid timeoutMs frontmatter; expected a positive integer."
        ),
        "got: {worker}"
    );

    let ghost = dispatch_tool(
        &tool,
        serde_json::json!({ "agent": "ghost", "task": "do it" }),
    )
    .await
    .expect_err("a name with only a broken definition is refused")
    .to_string();
    assert!(
        ghost.contains("Agent 'ghost' has invalid configuration: Agent 'ghost' has invalid fast frontmatter; expected true or false."),
        "must be the invalid-configuration error, not `agent not found`: {ghost}"
    );

    // The per-site location suffix pi appends for everything but the top-level `agent`.
    let tasks = dispatch_tool(
        &tool,
        serde_json::json!({ "tasks": [
            { "agent": "scout", "task": "a" },
            { "agent": "ghost", "task": "b" }
        ] }),
    )
    .await
    .expect_err("a broken task agent aborts the whole dispatch")
    .to_string();
    assert!(
        tasks.contains("Agent 'ghost' has invalid configuration:") && tasks.contains("(task 2)"),
        "got: {tasks}"
    );
}

// -------------------------------------------------------------------------------------------
// SUBA-087 — child-scoped stop (`childId`), pi `async-stop-action.ts:48-77` @v0.64.0 dispatched
// from `subagent-executor.ts:6163,6184`, schema `extension/schemas.ts:306`.
// -------------------------------------------------------------------------------------------

/// A `childId` reaches `control_stop`'s resolver and writes a TARGETED request — not a whole-run
/// one — and the receipt names the resolved child (`async-stop-action.ts:68,75`).
///
/// Before this change `SubagentToolParams` had no `child_id` field and no `deny_unknown_fields`,
/// so this exact call silently stopped the WHOLE run and answered with the whole-run receipt.
#[tokio::test]
async fn stop_with_child_id_reaches_control_stop_and_writes_a_targeted_request() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = seed_running_run(dir.path(), "childstoptool", &["scout", "reviewer"]);
    let tool = scoped_tool(dir.path()).await;

    let out = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "stop", "id": "childstoptool", "childId": "step:1" }),
    )
    .await
    .expect("a child-scoped stop must dispatch");
    assert_eq!(
        tool_text(&out),
        "Stop requested for child step:1 in async run childstoptool."
    );

    let requests = crate::background::control::consume_child_stop_requests(&paths.run_dir).await;
    assert_eq!(requests.len(), 1, "exactly one TARGETED request lands");
    assert_eq!(requests[0].target_index, Some(1));
    assert_eq!(requests[0].child_id.as_deref(), Some("step:1"));
    assert_eq!(requests[0].source, "stop-action");
    assert!(
        crate::background::control::check_stop_inbox_now(&paths)
            .await
            .expect("probe")
            .is_none(),
        "no whole-run stop request may be written by a child-scoped stop"
    );
}

/// pi `async-stop-action.ts:59-65`: a child that is not `pending`/`running` is refused with the
/// exact sentence, and nothing is written.
#[tokio::test]
async fn stop_refuses_a_child_that_is_not_pending_or_running_with_upstreams_text() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = seed_running_run(dir.path(), "childstopdone", &["scout", "reviewer"]);
    // Settle step 0 as complete in the seeded status.
    let mut status: crate::background::RunStatus =
        serde_json::from_slice(&std::fs::read(&paths.status).expect("read status"))
            .expect("parse status");
    status.steps[0].status = crate::background::StepState::Complete;
    status.steps[1].status = crate::background::StepState::Running;
    status.current_step = Some(1);
    std::fs::write(
        &paths.status,
        serde_json::to_string(&status).expect("serialize"),
    )
    .expect("write status");
    let tool = scoped_tool(dir.path()).await;

    let err = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "stop", "id": "childstopdone", "childId": "step:0" }),
    )
    .await
    .expect_err("a completed child cannot be stopped");
    assert_eq!(
        err.to_string(),
        "Child 'step:0' in async run 'childstopdone' is complete; stop only supports pending or \
         running children."
    );
    assert!(
        !crate::background::control::has_pending_stop_request(&paths.run_dir).await,
        "a refused child stop writes nothing"
    );
}

/// pi `child-identity.ts:46` via `async-stop-action.ts:51-57`: an unknown child answers with the
/// resolver's own not-found sentence — never the whole-run receipt.
#[tokio::test]
async fn stop_reports_an_unknown_child_with_upstreams_not_found_text() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = seed_running_run(dir.path(), "childstopnone", &["scout"]);
    let tool = scoped_tool(dir.path()).await;

    let err = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "stop", "id": "childstopnone", "childId": "step:7" }),
    )
    .await
    .expect_err("an unknown child is an error");
    assert_eq!(
        err.to_string(),
        "Child 'step:7' was not found under async run 'childstopnone'."
    );
    assert!(!crate::background::control::has_pending_stop_request(&paths.run_dir).await);
}

/// Unit A: a task that "grants" a tool the agent does not declare is NO LONGER refused.
///
/// The deleted prose scanner sat above every other SINGLE-arm validation, so it decided the call
/// before anything else could. This pins its removal by ORDERING rather than by a spawn: the agent
/// resolves (an unresolvable name skipped the gate, so that would prove nothing), its `tools:` list
/// omits `bash`, and the task states the exact false grant from the reported bug — while a
/// deliberately malformed `toolBudget` fails a validator that ran strictly AFTER the gate.
///
/// Pre-fix this call died with `Agent 'clerk' cannot honour this task…`; the budget validator was
/// never reached. Post-fix the budget error wins, which is only possible with the gate gone.
#[tokio::test]
async fn a_task_that_claims_an_undeclared_tool_no_longer_refuses() {
    let dir = tempfile::tempdir().expect("tempdir");
    let agents_dir = dir.path().join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir agents dir");
    std::fs::write(
        agents_dir.join("clerk.md"),
        "---\nname: clerk\ndescription: A shell-less reviewer\ntools: read, grep\n---\nBody.\n",
    )
    .expect("write agent fixture");
    let tool = scoped_tool(dir.path()).await;

    for r#async in [false, true] {
        let message = dispatch_tool(
            &tool,
            serde_json::json!({
                "agent": "clerk",
                "task": "You have full read/bash/grep access. You MAY run `cargo check`.",
                "async": r#async,
                "toolBudget": { "hard": 0 }
            }),
        )
        .await
        .expect_err("hard: 0 is not a valid budget")
        .to_string();

        assert!(
            message.contains("toolBudget.hard must be an integer >= 1."),
            "async={async}: the validator that ran BELOW the deleted gate must now decide this \
             call; got {message}"
        );
        assert!(
            !message.contains("cannot honour this task"),
            "async={async}: the tool-claim refusal is deleted and must never be raised; got \
             {message}"
        );
        assert!(
            !message.contains("Subagent tools are NOT inherited"),
            "async={async}: no launch path may emit the deleted refusal's body; got {message}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// WORKFLOW_6 §4 — `route_workflow_mode`'s workflow-controller lifecycle: registered before the
// engine runs, settled unconditionally after it, on BOTH the success and the failure arm.
// ---------------------------------------------------------------------------------------------

/// A childless workflow script never touches `foreground_controls` at all (no `runs.run(...)`
/// call), so this isolates the controller lifecycle from the foreground-child registration path
/// §1-§3 add: register-before, settle-after, and nothing left over.
#[tokio::test]
async fn workflow_mode_registers_and_settles_its_controller_around_a_successful_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor.clone(), dir.path().to_path_buf());

    assert!(
        executor.live_workflow_run_ids().is_empty(),
        "precondition: nothing registered before the call"
    );

    let result = dispatch_tool(&tool, serde_json::json!({ "workflowScript": "return 42;" }))
        .await
        .expect("a trivial childless workflow script must succeed");

    assert!(
        tool_text(&result).contains("Workflow completed with 0 child run(s). Return: 42"),
        "{}",
        tool_text(&result)
    );
    assert!(
        executor.live_workflow_run_ids().is_empty(),
        "the workflow controller registered before the engine ran must be settled (removed) \
         once the call returns — a leaked entry would make WORKFLOW_10 over-count live workflows \
         forever"
    );
}

/// The FAILURE arm settles the controller exactly as unconditionally as the success arm — §4.4's
/// own point: a workflow that failed is exactly as settled as one that succeeded.
#[tokio::test]
async fn workflow_mode_settles_its_controller_even_when_the_script_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor.clone(), dir.path().to_path_buf());

    let err = dispatch_tool(
        &tool,
        serde_json::json!({ "workflowScript": "throw new Error('boom');" }),
    )
    .await
    .expect_err("a script that throws must fail the call");
    assert!(err.to_string().contains("boom"), "{err}");

    assert!(
        executor.live_workflow_run_ids().is_empty(),
        "the controller must be settled on the FAILURE arm too — leaving it behind would make \
         WORKFLOW_8's dismiss refusal permanent for this id"
    );
}

// ---------------------------------------------------------------------------------------------
// WORKFLOW_19 — `runs.host` is a real verb, and its evidence reaches the receipt.
// ---------------------------------------------------------------------------------------------

/// End to end through the real engine, the real guest realm and the real `tokio::process` runner:
/// a script runs one passing and one failing host command, and the receipt records BOTH, ONCE
/// each, TERMINAL.
///
/// Three distinct regressions are pinned here, and each one has a different failure signature:
///
/// * `supports_host()` false ⇒ `prelude.js:574` deletes the property and the script dies with a
///   `TypeError` on `runs.host` — never the Rust refusal string, which the deleted surface makes
///   unreachable from the guest.
/// * `on_host_step: None` ⇒ the run SUCCEEDS and `hostSteps` is simply ABSENT (it is
///   `skip_serializing_if = "Vec::is_empty"`, so the silent-failure shape is a missing key, not an
///   empty array).
/// * a blind `push` instead of an upsert-by-id ⇒ two nodes per command, which
///   `assert_unique_host_step_ids` rejects — turning this successful workflow into a
///   `ToolError` reading "workflow completed but its receipt is invalid".
///
/// The failing command is caught, not propagated: `run_host_command` turns a non-`ok` result into
/// an `Err`, which is a REJECTED PROMISE in the guest, so an uncaught `runs.host("bad", …)` would
/// fail the whole workflow rather than returning its verdict as a value.
#[tokio::test]
async fn workflow_host_commands_land_in_the_receipt_once_each_and_terminal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor.clone(), dir.path().to_path_buf());

    let script = r#"
        const gate = await runs.host("gate", { kind: "command", command: "exit 0", timeoutMs: 30000, role: "gate" });
        let failure = null;
        try {
            await runs.host("bad", { kind: "command", command: "exit 7", timeoutMs: 30000 });
        } catch (error) {
            failure = String((error && error.message) || error);
        }
        return { state: gate.state, ok: gate.ok, exitCode: gate.exitCode, failure };
    "#;

    let result = dispatch_tool(&tool, serde_json::json!({ "workflowScript": script }))
        .await
        .expect("the host commands are caught, so the workflow itself must succeed");

    let text = tool_text(&result);
    assert!(
        text.contains("\"state\": \"passed\"") || text.contains("\"state\":\"passed\""),
        "the passing command's verdict must reach the script: {text}"
    );
    assert!(
        text.contains("Host command 'bad' failed: Command exited with code 7."),
        "the failing command must reject with upstream's own message: {text}"
    );

    let details = result
        .details
        .as_ref()
        .expect("the settlement folds details");
    let steps = details["workflowReceipt"]["receipt"]["hostSteps"]
        .as_array()
        .expect(
            "`hostSteps` is omitted entirely when empty, so a missing key here IS the \
             `on_host_step: None` regression",
        );

    let mut observed: Vec<(String, String, String)> = steps
        .iter()
        .map(|step| {
            (
                step["id"].as_str().unwrap_or_default().to_string(),
                step["state"].as_str().unwrap_or_default().to_string(),
                step["reasonCode"].as_str().unwrap_or("-").to_string(),
            )
        })
        .collect();
    observed.sort();
    assert_eq!(
        observed,
        vec![
            (
                "bad".to_string(),
                "error".to_string(),
                "command_failed".to_string()
            ),
            ("gate".to_string(), "done".to_string(), "-".to_string()),
        ],
        "two commands, two nodes, both terminal — a `running` entry is the upsert bug and four \
         entries would not have produced a receipt at all"
    );
    assert_eq!(
        steps[0]["verdict"].as_str(),
        Some("pass"),
        "the passing gate carries the settled verdict, in the engine's emission order"
    );
    assert_eq!(steps[0]["exitCode"].as_i64(), Some(0));
    assert_eq!(steps[1]["exitCode"].as_i64(), Some(7));
}

/// The FAILURE arm carries the same evidence. A workflow that lets `runs.host` throw settles
/// through `settle_foreground_workflow`'s error branch with `error.partial` — which carries no
/// host steps of its own — so wiring only the success arm would lose the terminal node in exactly
/// the case it matters most: the workflow died because the command failed.
#[tokio::test]
async fn a_workflow_that_dies_on_a_host_command_still_records_the_step() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor.clone(), dir.path().to_path_buf());

    let script = r#"
        await runs.host("gate", { kind: "command", command: "exit 9", timeoutMs: 30000, role: "gate" });
        return "unreachable";
    "#;

    let error = dispatch_tool(&tool, serde_json::json!({ "workflowScript": script }))
        .await
        .expect_err("an uncaught host-command rejection must fail the workflow");
    assert!(
        error.to_string().contains("Host command 'gate' failed"),
        "{error}"
    );

    let details = error
        .details
        .as_ref()
        .expect("the failure arm folds details too — that is what `with_details` is for");
    let steps = details["workflowReceipt"]["receipt"]["hostSteps"]
        .as_array()
        .expect("the failure arm must carry the host steps");
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0]["id"].as_str(), Some("gate"));
    assert_eq!(
        steps[0]["state"].as_str(),
        Some("error"),
        "terminal, not the `running` node the op emitted first"
    );
    assert_eq!(steps[0]["reasonCode"].as_str(), Some("command_failed"));
    assert_eq!(steps[0]["exitCode"].as_i64(), Some(9));
}

/// THE REACHABILITY PROOF for `background::async_status_snapshot::project::host_steps_for_run`.
///
/// That projector used to return `&'static []` unconditionally, on the recorded reasoning that a
/// [`crate::workflows::HostStepNode`] "lives on the LIVE workflowScript engine and on a workflow
/// RECEIPT — never on the persisted status a snapshot reads". The producer was already real (the
/// engine emits the nodes and `on_host_step` was already wired); what was missing was the ONE hop
/// that put them on the run, so this asserts that hop through the production path, end to end,
/// touching no surface directly:
///
/// 1. the real dispatch runs a real `runs.host` command through the real engine,
/// 2. the engine's `on_host_step` folds each transition onto the run's `RunStatus` via
///    `record_host_step`, and `settle_foreground_workflow` writes that status to `status.json`,
/// 3. `tui::fleet::collect_fleet_history` — the inspector's own on-disk reader, which knows
///    nothing about host steps — loads it back as an `AsyncRunView`,
/// 4. the projection, handed that view and nothing else, emits the monitor as a `host-step` child
///    of the run.
///
/// Step 3 is what makes this a reachability test rather than a round-trip: the view is read off
/// the FILESYSTEM by a production reader, so the assertion cannot pass on an in-memory record the
/// test itself populated.
#[tokio::test]
async fn a_workflow_host_step_reaches_the_async_status_snapshot() {
    use crate::background::async_status_snapshot::{
        AsyncStatusSnapshotKind, AsyncStatusSnapshotOptions, build_async_status_snapshot,
    };

    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor.clone(), dir.path().to_path_buf());

    let script = r#"
        const gate = await runs.host("gate", { kind: "command", command: "exit 0", timeoutMs: 30000, role: "gate" });
        return gate.state;
    "#;
    let result = dispatch_tool(&tool, serde_json::json!({ "workflowScript": script }))
        .await
        .expect("a passing host command must not fail the workflow");
    let workflow_run_id = result.details.as_ref().expect("details")["workflowRunId"]
        .as_str()
        .expect("the settlement stamps the run id")
        .to_string();

    // The SAME roots arithmetic the dispatch itself used (`routing.rs`'s
    // `default_async_root_in(&cfg.roots, cwd)`), so this reads the tree the run really wrote to.
    let cfg = executor.config_snapshot().await;
    let async_root =
        crate::extension::executor::paths::default_async_root_in(&cfg.roots, dir.path());
    let results_dir =
        crate::extension::executor::paths::default_results_dir_in(&cfg.roots, dir.path());
    let runs = crate::tui::fleet::collect_fleet_history(&async_root, &results_dir, None)
        .await
        .expect("the async root exists — the dispatch created it");
    let job = runs
        .iter()
        .find(|run| run.status.run_id.as_str() == workflow_run_id)
        .expect("the workflow's own status.json must be on disk and readable");

    assert_eq!(
        job.status
            .telemetry
            .host_steps
            .iter()
            .map(|step| step.id.as_str())
            .collect::<Vec<_>>(),
        vec!["gate"],
        "the terminal status.json must carry the monitor — an empty list here IS the unwired          `host_steps_for_run` regression, one hop upstream of the projection"
    );

    let snapshot = build_async_status_snapshot([job], &AsyncStatusSnapshotOptions::default());
    let run = snapshot
        .runs
        .iter()
        .find(|run| run.id == workflow_run_id)
        .expect("the run is projected");
    let host_step = run
        .children
        .as_ref()
        .expect("a run with a host step has children")
        .iter()
        .find(|child| child.kind == AsyncStatusSnapshotKind::HostStep)
        .expect(
            "the projection must emit the monitor as a `host-step` child — this is the \
             assertion a `&'static []` `host_steps_for_run` cannot satisfy",
        );
    assert_eq!(host_step.id, "gate");
    assert_eq!(host_step.label, "gate");
    let metadata = host_step
        .host_step
        .as_ref()
        .expect("the host-step node carries its monitor metadata");
    assert_eq!(metadata.state, crate::workflows::HostStepState::Done);
    assert_eq!(
        metadata.verdict,
        Some(crate::workflows::HostStepVerdict::Pass)
    );
    assert_eq!(metadata.role.as_deref(), Some("gate"));
    assert!(
        host_step.ended_at.is_some(),
        "a settled monitor is terminal, so it carries an endedAt"
    );
}

// ---------------------------------------------------------------------------------------------
// WORKFLOW_20 — `state.get`/`state.set`: the mission scratchpad, bound end to end through the
// real `execute` seam. The binding `tool/mod.rs` already resolved for every mode is what decides
// whether `state` exists at all, so both halves are asserted here: a bound run gets a durable
// file, an unbound one is refused by the ANALYZER (before an isolate is spent) rather than
// discovering a bare `ReferenceError` from inside the guest.
// ---------------------------------------------------------------------------------------------

/// The durability proof, and the one that costs a second process upstream: two SEPARATE tool
/// calls against the same `missionId`, where the second reads what the first wrote. Nothing is
/// shared between them but `<missionDir>/<id>/state.json` — the workflow run id, the isolate, the
/// engine's `RunShared` and the store itself are all freshly built per call.
#[tokio::test]
async fn a_mission_bound_workflow_script_carries_state_across_two_calls() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());

    let location = crate::missions::resolve_mission_store_location(
        dir.path(),
        Some(&scoped_missions(dir.path())),
        None,
    );
    let record = crate::missions::create_mission(
        &location,
        &crate::missions::MissionCreateInput {
            title: "Scratchpad".to_string(),
            objective: "prove the state survives the run".to_string(),
            ..Default::default()
        },
        crate::time::now_epoch_millis(),
        None,
    )
    .expect("the mission the workflow attaches to");

    let first = dispatch_tool(
        &tool,
        serde_json::json!({
            "missionId": record.id,
            "workflowScript": "await state.set(\"seen\", { n: 1 }); return await state.get(\"seen\");",
        }),
    )
    .await
    .expect("a mission-bound workflowScript may use state");
    assert!(
        tool_text(&first).contains("Return: {\"n\":1}"),
        "the value round-tripped through the store within the run: {}",
        tool_text(&first)
    );

    let second = dispatch_tool(
        &tool,
        serde_json::json!({
            "missionId": record.id,
            "workflowScript":
                "const prior = await state.get(\"seen\"); \
                 await state.set(\"seen\", { n: (prior ? prior.n : 0) + 1 }); \
                 return await state.get(\"seen\");",
        }),
    )
    .await
    .expect("the second call binds the same mission and therefore the same scratchpad");
    assert!(
        tool_text(&second).contains("Return: {\"n\":2}"),
        "a SECOND run read the first run's write — if this says n:1 the store is per-run, not \
         per-mission: {}",
        tool_text(&second)
    );

    let state_path =
        crate::missions::mission_state_path(&location, &record.id).expect("state path");
    let raw = std::fs::read_to_string(&state_path).expect("the scratchpad is a real file on disk");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&raw).expect("valid JSON"),
        serde_json::json!({ "seen": { "n": 2 } }),
        "the evidence a workflow can close on: {}",
        state_path.display()
    );
}

/// The 256 KiB ceiling is the ONE invariant the engine does not also enforce (`engine.rs` bounds
/// no value's size), so a refusal carrying this sentence can only have come from the ported
/// `MissionWorkflowState` — it is the end-to-end proof that the call site wired the real store and
/// not a look-alike.
#[tokio::test]
async fn a_mission_bound_workflow_script_still_hits_the_256_kib_ceiling() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());

    let location = crate::missions::resolve_mission_store_location(
        dir.path(),
        Some(&scoped_missions(dir.path())),
        None,
    );
    let record = crate::missions::create_mission(
        &location,
        &crate::missions::MissionCreateInput {
            title: "Overflow".to_string(),
            objective: "prove the ceiling still bites".to_string(),
            ..Default::default()
        },
        crate::time::now_epoch_millis(),
        None,
    )
    .expect("mission");

    let error = dispatch_tool(
        &tool,
        serde_json::json!({
            "missionId": record.id,
            "workflowScript": "await state.set(\"big\", \"x\".repeat(300000)); return \"unreachable\";",
        }),
    )
    .await
    .expect_err("an over-budget set must fail the workflow, not truncate the value");
    assert!(
        error
            .to_string()
            .contains("Mission state exceeds the 256 KiB limit ("),
        "{error}"
    );
    assert!(
        !crate::missions::mission_state_path(&location, &record.id)
            .expect("state path")
            .exists(),
        "the size check runs BEFORE the write, so a refused set leaves no file behind"
    );
}

/// An UNBOUND workflow is refused by the analyzer, before an isolate is spent — and with the
/// message the analyzer actually emits, which names the unavailable global and teaches the rule.
/// The engine's own "Workflow state is unavailable without a mission." op refusal is defence in
/// depth that stays unreachable from the guest: with `state_enabled` false `globalThis.state` is
/// never installed at all, so a run that got this far would see a bare `ReferenceError`. That is
/// exactly what the analyzer exists to prevent.
#[tokio::test]
async fn an_unbound_workflow_script_is_refused_by_the_analyzer_not_the_guest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());

    let expected = "workflowScript referenced an unavailable global 'state'. Available globals \
                    are runs, emit, console, and standard ECMAScript built-ins only. \
                    state.get/state.set require a mission; this run was started with \
                    mission:false.";

    // A bare `workflowScript` never auto-creates a mission: `workflow_objective` reads `task` /
    // `tasks[]` / `chain[]` only and never looks at `workflowScript`, so there is no objective to
    // create one from. Unbound stays unbound.
    let bare = dispatch_tool(
        &tool,
        serde_json::json!({ "workflowScript": "return await state.get(\"seen\");" }),
    )
    .await
    .expect_err("a workflow with no mission may not use state");
    assert!(bare.to_string().contains(expected), "{bare}");
    assert!(
        !bare.to_string().contains("ReferenceError"),
        "the analyzer must win the race with the guest realm: {bare}"
    );

    // `mission: false` is the EXPLICIT opt-out and must land in exactly the same place —
    // `prepare_mission_launch` returns `Ok(None)` for it, so `state_enabled` is false by the same
    // single derivation rather than by a second branch.
    let opted_out = dispatch_tool(
        &tool,
        serde_json::json!({
            "mission": false,
            "workflowScript": "return await state.get(\"seen\");",
        }),
    )
    .await
    .expect_err("mission:false is explicitly unbound, not 'resolve from ambient context'");
    assert!(opted_out.to_string().contains(expected), "{opted_out}");
}

/// The flip side of the analyzer gate, and the reason `state_enabled` may never be two literals:
/// the very `state.get(...)` reference the previous test refuses must be ADMITTED once a mission
/// is bound. A build that hard-coded `false` at the validate call would reject this before the
/// engine ever saw it, even though the store is right there.
#[tokio::test]
async fn the_analyzer_admits_state_exactly_when_the_run_has_a_mission() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());

    let location = crate::missions::resolve_mission_store_location(
        dir.path(),
        Some(&scoped_missions(dir.path())),
        None,
    );
    let record = crate::missions::create_mission(
        &location,
        &crate::missions::MissionCreateInput {
            title: "Admitted".to_string(),
            objective: "prove the analyzer agrees with the store".to_string(),
            ..Default::default()
        },
        crate::time::now_epoch_millis(),
        None,
    )
    .expect("mission");

    let result = dispatch_tool(
        &tool,
        serde_json::json!({
            "missionId": record.id,
            "workflowScript":
                "const missing = await state.get(\"seen\"); \
                 return { missing: missing === undefined };",
        }),
    )
    .await
    .expect("the identical script an unbound run refuses must pass once a mission is bound");
    assert!(
        tool_text(&result).contains("Return: {\"missing\":true}"),
        "an absent key reaches the guest as `undefined`, not as a refusal: {}",
        tool_text(&result)
    );
}

/// SCOPE_12's advertise-vs-dispatch invariant, pinned for `inspect` — the rule
/// `text.rs::SUBAGENT_ACTIONS` records and `schema.rs:349-360` enforces by DERIVING the JSON
/// Schema enum from that one slice: the enum entry and the dispatch arm land in the same change.
///
/// **Pre-fix this goes red twice over**: without the `SUBAGENT_ACTIONS` entry the verb is not
/// advertised, and without the `route_action` arm the tool answers
/// `Unknown action: inspect. …` instead of a `ToolResult`.
#[tokio::test]
async fn inspect_is_both_advertised_and_dispatched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;
    tool.executor()
        .set_host_services(Arc::new(crate::extension::testsupport::FixedSessionHost(
            "session-a",
        )));
    crate::extension::testsupport::seed_orphaned_run(
        dir.path(),
        "run0inspect0",
        Some("session-a"),
        Some(std::process::id()),
    );

    assert!(
        crate::extension::tool::text::subagent_actions().contains(&"inspect"),
        "the verb must be advertised in the ONE list the schema enum is derived from"
    );

    let result = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "inspect", "id": "run0inspect0" }),
    )
    .await
    .expect("inspect dispatches through the tool");
    let text = tool_text(&result);
    assert!(
        !text.contains("Unknown action"),
        "it must reach the handler, not the did-you-mean arm: {text}"
    );
    assert!(text.contains("Run: run0inspect0"), "{text}");
    assert!(text.contains("State: running"), "{text}");
    // The full wire reply rides along in `details`, so a widget host reads the same object
    // upstream's `encodeInspectReply` would have emitted.
    let details = result.details.expect("the reply object");
    assert_eq!(details["kind"], "pi-subagents.inspect-reply");
    assert_eq!(details["version"], 1);
    assert_eq!(details["asyncId"], "run0inspect0");
    assert!(details.get("error").is_none(), "{details}");

    // A run owned by ANOTHER session is refused at the tool boundary too, with the code — the
    // partition does not stop at `read_output.rs`.
    crate::extension::testsupport::seed_orphaned_run(
        dir.path(),
        "run0foreign0",
        Some("session-b"),
        Some(std::process::id()),
    );
    let result = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "inspect", "id": "run0foreign0" }),
    )
    .await
    .expect("a refusal is still a ToolResult, not a ToolError");
    assert_eq!(
        result.details.expect("the reply object")["error"]["code"],
        "foreign_session"
    );

    // And the verb needs a target: `id` (or `runId`), the `status` arm's own precedence.
    let error = dispatch_tool(&tool, serde_json::json!({ "action": "inspect" }))
        .await
        .expect_err("no id");
    assert!(
        error.to_string().contains("action='inspect' requires id"),
        "{error}"
    );
}

/// A child binary that emits a BLOCKING `contact_supervisor` ask on its NDJSON stdout and then
/// fails — the one shape that produces a genuinely detached [`crate::exec::SingleResult`].
///
/// The ask is an ordinary `tool_execution_start` for the `contact_supervisor` tool carrying
/// `reason: "need_decision"`, which is exactly what `exec::drive_attempt`'s
/// `contact_supervisor_block_prompt` recognises (R-SA-037). The non-zero exit is the second half of
/// the shape under test: `deliver_launch` rejects a launch only for a child that is `!ok`
/// (`engine.rs:1824`), so a detached child that exited cleanly would resolve normally and never
/// park the workflow.
fn write_detaching_child_binary(dir: &std::path::Path) -> PathBuf {
    let script = dir.join("detaching-child.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\n\
printf '%s\\n' '{\"type\":\"agent_start\"}'\n\
printf '%s\\n' '{\"type\":\"tool_execution_start\",\"toolCallId\":\"c1\",\
\"toolName\":\"contact_supervisor\",\"args\":{\"reason\":\"need_decision\",\
\"message\":\"which branch do I target?\"}}'\n\
printf '%s\\n' '{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\
\"content\":[{\"type\":\"text\",\"text\":\"waiting on the supervisor\"}]}}'\n\
printf '%s\\n' '{\"type\":\"agent_settled\"}'\n\
exit 1\n",
    )
    .expect("write the scripted child");
    std::fs::set_permissions(
        &script,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )
    .expect("make the scripted child executable");
    script
}

/// THE REACHABILITY PROOF for
/// [`crate::extension::executor::workflow_detach::reconcile_detached_workflow_child_completion`].
///
/// That reconciler was SCOPE_8's whole deliverable and carried `#[cfg_attr(not(test),
/// allow(dead_code))]` on the recorded reasoning that *"cyrup has no detach hook to fire it"*. The
/// reasoning was wrong: cyrup's drive loop keeps driving a detached child to its real exit, so the
/// detached child's COMPLETION — the very event upstream's `onDetachedExit` closure waits for —
/// arrives synchronously in `WorkflowRunHost::launch`'s return. This drives that hook end to end
/// through the production dispatch, touching no surface directly:
///
/// 1. a REAL `workflowScript` dispatch launches a REAL child through
///    `WorkflowRunHost::launch` → `run_foreground_streaming`;
/// 2. the child emits a blocking `contact_supervisor` ask, so `exec::drive_attempt` marks the
///    attempt detached and `classify_attempt` settles the ladder at `LadderStop::Detached`;
/// 3. `deliver_launch` rejects the launch with `workflowErrorKind: "detached-child"`, the engine
///    hangs that kind on the error, and `route_workflow_mode`'s failure arm parks the run at
///    `Paused` — pi's own shape;
/// 4. `reconcile_detached_workflow_children` drives the reconciler over the detached child's
///    settled result, which supersedes that paused status with a terminal one.
///
/// Step 4's evidence is read back off the FILESYSTEM by production readers — `collect_fleet_history`
/// for the status and the run's own `events.jsonl` for the completion line — so nothing here can
/// pass on an in-memory record the test itself populated. The `reconciledFromDetachedChild` key is
/// written by exactly one function in the crate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_detached_workflow_child_reconciles_the_paused_workflow() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_detaching_child_binary(dir.path());

    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    // A live session id: the reconciled `ResultFile` is session-partitioned, and an unattributable
    // result cannot be indexed (`finish.rs:478-484`'s refusal, reproduced by the reconciler).
    executor.set_host_services(Arc::new(crate::extension::testsupport::FixedSessionHost(
        "session-detach",
    )));
    {
        let mut cfg = executor.config_cell().lock().await;
        cfg.spawn_command = Some(crate::spawn::SpawnCommand {
            binary: script,
            base_args: Vec::new(),
        });
    }
    let tool = SubagentTool::new(executor.clone(), dir.path().to_path_buf());

    let error = dispatch_tool(
        &tool,
        serde_json::json!({
            "workflowScript":
                "await runs.run(\"a\", { agent: \"worker\", task: \"T\", model: \"sonnet\" });\n\
                 return \"unreachable\";"
        }),
    )
    .await
    .expect_err("a detached-child rejection must fail the workflow");

    assert!(
        error.to_string().contains("Run 'a' detached"),
        "the engine must reject the launch with the DETACHED wording, not the plain failure one: \
         {error}"
    );
    let details = error
        .details
        .as_ref()
        .expect("the failure arm folds details, and that is what carries the reconciliation");
    let workflow_run_id = details["workflowRunId"]
        .as_str()
        .expect("the settlement stamps the run id")
        .to_string();
    let reconciled = details["reconciledFromDetachedChildren"].as_array().expect(
        "the dispatch must report the reconciled child run ids — an absent key IS the unwired \
             reconciler",
    );
    assert_eq!(
        reconciled.len(),
        1,
        "one detached child, one reconciliation"
    );
    let child_run_id = reconciled[0]
        .as_str()
        .expect("a reconciled entry is a run id")
        .to_string();

    // The SAME roots arithmetic the dispatch used, so this reads the tree the run really wrote to.
    let cfg = executor.config_snapshot().await;
    let async_root =
        crate::extension::executor::paths::default_async_root_in(&cfg.roots, dir.path());
    let results_dir =
        crate::extension::executor::paths::default_results_dir_in(&cfg.roots, dir.path());

    // 1. The status the inspector's own on-disk reader loads back is TERMINAL, not the `paused` the
    //    settlement wrote a moment earlier. Only the reconciler takes it back out of `paused`.
    let runs = crate::tui::fleet::collect_fleet_history(&async_root, &results_dir, None)
        .await
        .expect("the async root exists — the dispatch created it");
    let job = runs
        .iter()
        .find(|run| run.status.run_id.as_str() == workflow_run_id)
        .expect("the workflow's own status.json must be on disk and readable");
    assert_eq!(
        job.status.state,
        crate::background::RunState::Failed,
        "a workflow left at `paused` is the unreconciled regression; the reconciler promotes it"
    );
    assert!(
        job.status.ended_at.is_some(),
        "a promoted workflow carries the settlement's own endedAt"
    );

    // 2. The `subagent.workflow.completed` line, with the ONE key only this reconciler writes.
    let events = tokio::fs::read_to_string(async_root.join(&workflow_run_id).join("events.jsonl"))
        .await
        .expect("the reconciler appends the workflow's event journal");
    let completed = events
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|event| event["type"] == "subagent.workflow.completed")
        .expect("the promoted workflow emits its completion line");
    assert_eq!(
        completed["reconciledFromDetachedChild"].as_str(),
        Some(child_run_id.as_str()),
        "`reconciledFromDetachedChild` has exactly one writer in this crate — \
         `reconciled_from_detached_child`"
    );
    assert_eq!(completed["runId"].as_str(), Some(workflow_run_id.as_str()));

    // PROVENANCE. The child's run id belongs in `reconciledFromDetachedChild` (asserted above) and
    // deliberately NOT in the prose — a 32-hex token inside a human-readable line is noise. An
    // earlier revision asserted `summary.contains(child_run_id)` and failed against a summary that
    // was already correct, which is the wrong test, not a missing feature.
    //
    // What must hold is that the promoter CARRIES UP the child's failure rather than synthesizing a
    // placeholder: the settled summary is the failure reason itself, and is the same text the
    // event's own `error` carries. A promoter that invented "Workflow script failed." would satisfy
    // neither.
    let summary = completed["summary"]
        .as_str()
        .expect("a settled workflow carries a summary");
    assert_eq!(
        Some(summary),
        completed["error"].as_str(),
        "the settled summary is the carried-up failure reason, not a second synthesized string"
    );
    assert!(
        !summary.trim().is_empty() && summary != "Workflow script failed.",
        "the promoter must carry the detached child's failure up, not a generic placeholder: \
         {summary:?}"
    );

    // 3. The superseding payload: `agent: "workflow"`, literal, never synthesized from the steps —
    //    which is what tells it apart from anything `finish_run` would have published.
    let session_id =
        crate::identity::SessionId::parse_opt(Some("session-detach")).expect("a valid session id");
    let run_id = crate::background::RunId::from_token(workflow_run_id.clone());
    let payload = crate::background::RunPaths::for_run(&async_root, &results_dir, &run_id)
        .resolve_result(&session_id, &run_id)
        .await
        .expect("the reconciler publishes a result file for the settled workflow");
    let result: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&payload).await.expect("read the payload"))
            .expect("the payload is JSON");
    assert_eq!(result["agent"].as_str(), Some("workflow"));
    assert_eq!(result["mode"].as_str(), Some("workflow"));
    assert_eq!(result["success"].as_bool(), Some(false));
    // The rebuilt children carry the reconciled child's OWN settled result, and the child is no
    // longer marked detached — upstream clears it explicitly (`detached: undefined`,
    // `foreground/workflow-detach-reconcile.ts:66` @v0.68.0), because once the reconciler has the
    // child's result the child is settled, not outstanding. An earlier revision of this test
    // asserted `detached == true`, which would have passed only if the reconciler had FAILED to
    // clear the flag — it asserted the defect rather than the fix.
    let children = result["results"]
        .as_array()
        .expect("the settled workflow payload rebuilds its children");
    let reconciled_child = children
        .iter()
        .find(|child| child["childRunId"].as_str() == Some(child_run_id.as_str()))
        .unwrap_or_else(|| panic!("the rebuilt children name the reconciled child: {result}"));
    assert_ne!(
        reconciled_child["detached"],
        serde_json::Value::Bool(true),
        "the reconciler clears `detached` on the child it settled: {reconciled_child}"
    );
    assert_eq!(
        reconciled_child["error"].as_str(),
        completed["error"].as_str(),
        "the rebuilt child carries its own failure, the same one the completion line reports"
    );
    assert_eq!(
        reconciled_child["outputState"].as_str(),
        Some("present"),
        "the reconciled child's output is delivered, not still outstanding: {reconciled_child}"
    );
}

// =================================================================================================
// VL-S13 — `refine` / `refine.show` / `refine.rollback` through the REAL tool
//
// These drive `SubagentTool::execute`, so each one exercises the whole production chain the model
// reaches: the schema enum (derived from `SUBAGENT_ACTIONS`), `route_action`'s
// `RefinementAction::from_wire` guard arm, the child-safe gate, agent resolution, the existing-file
// read, and — for `refine.rollback` — the serializer, the round-trip guard and the atomic write.
// =================================================================================================

/// Write a real project persona at `<cwd>/.cyrup/agents/<name>.md` so `resolve_one_agent` has
/// something to resolve. Returns the raw persona BODY, which is what `hash_prompt` hashes.
fn seed_refinement_agent(dir: &std::path::Path, name: &str, body: &str) -> String {
    let agents_dir = dir.join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir agents");
    std::fs::write(
        agents_dir.join(format!("{name}.md")),
        format!("---\nname: {name}\ndescription: A probe\n---\n{body}\n"),
    )
    .expect("write agent");
    body.to_string()
}

/// A hand-written overlay, deliberately NOT produced by `serialize_refinement_file`: the point is
/// that the parser, the serializer and a file pi could have written all agree on one format.
fn write_refinement_overlay(
    dir: &std::path::Path,
    agent: &str,
    digest: &str,
    revision: u32,
    current: &str,
    snapshots_json: &str,
) -> std::path::PathBuf {
    let path = crate::exec::agent_refinements::get_agent_refinement_path(dir, agent)
        .expect("a usable agent name");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir refinements");
    let body = format!(
        "<!-- pi-subagents-refinement:v1\n{{\"agent\":\"{agent}\",\"revision\":{revision},\
         \"updatedAt\":\"2026-09-01T00:00:00.000Z\",\"base\":{{\"source\":\"project\",\
         \"filePath\":\"p.md\",\"systemPromptSha256\":\"{digest}\"}},\"evidence\":{{}}}}\n-->\n\n\
         # Current refinement for `{agent}`\n\n```pi-subagents-refinement-current\n{current}\n```\n\n\
         # Snapshots\n\n```pi-subagents-refinement-snapshots-json\n{snapshots_json}\n```\n"
    );
    std::fs::write(&path, body).expect("write overlay");
    path
}

/// T1 — the advertise-vs-dispatch invariant for all three verbs, in the shape
/// `inspect_is_both_advertised_and_dispatched` established.
///
/// Gutted: no `SUBAGENT_ACTIONS` entry → the advertise assertion fails; no dispatch arm → the
/// reply is the did-you-mean text, so the equality on pi `:557`'s sentence fails.
#[tokio::test]
async fn refine_verbs_are_both_advertised_and_dispatched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;
    seed_refinement_agent(dir.path(), "probe", "You are probe.");

    for verb in ["refine", "refine.show", "refine.rollback"] {
        assert!(
            crate::extension::tool::text::subagent_actions().contains(&verb),
            "{verb} must be advertised in the ONE list the schema enum is derived from"
        );
    }

    // pi `:557` — no overlay is an ORDINARY answer for `refine.show`, not an error.
    let result = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "refine.show", "agent": "probe" }),
    )
    .await
    .expect("refine.show dispatches and is not an error with no overlay");
    assert_eq!(
        tool_text(&result),
        "No refinement overlay exists for 'probe'."
    );

    // pi `:578` — the SAME sentence from `refine.rollback` IS an error.
    let error = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "refine.rollback", "agent": "probe" }),
    )
    .await
    .expect_err("rollback with no overlay is an error");
    assert_eq!(
        error.to_string(),
        "No refinement overlay exists for 'probe'."
    );
}

/// T2 — the write/read round trip through the REAL tool, which is the correctness proof the spec
/// asks for: `refine.rollback` serializes, and `parse_refinement_file` reads the bytes back.
///
/// Gutted: drop the serializer's integral-number rule → assertion (7) fails and pi can no longer
/// read the file; pop instead of append → (5)/(6) fail; write `latest.after` instead of
/// `latest.before` → (3) fails; move the metadata comment off byte 0 or lose either fence's
/// leading blank line → (2) fails, for the parser's own documented reason.
#[tokio::test]
async fn refine_rollback_appends_a_snapshot_and_round_trips_the_parser() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;
    seed_refinement_agent(dir.path(), "probe", "You are probe.");
    let path = write_refinement_overlay(
        dir.path(),
        "probe",
        "abc123",
        2,
        "- new",
        "[{\"revision\":2,\"at\":\"2026-09-01T00:00:00.000Z\",\"action\":\"refine\",\
         \"before\":\"- old\",\"after\":\"- new\",\"evidenceIds\":[\"live:a1\"],\
         \"proposalAgent\":\"reviewer\"}]",
    );

    // (1) pi `:589`.
    let result = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "refine.rollback", "agent": "probe" }),
    )
    .await
    .expect("rollback dispatches");
    assert_eq!(
        tool_text(&result),
        format!(
            "Rolled back refinement overlay for 'probe' to revision 3.\nPath: {}",
            path.display()
        )
    );

    let raw = std::fs::read_to_string(&path).expect("the file exists");
    // (2) THE ROUND TRIP.
    let parsed = crate::exec::agent_refinements::parse_refinement_file(&raw, "p")
        .expect("the tool's own output must parse");
    // (3) the rollback restores `latest.before`, not `latest.after`.
    assert_eq!(parsed.current, "- old");
    // (4)
    assert_eq!(parsed.metadata.revision, 3.0);
    // (5) history GREW — a rollback is itself a revision, never a pop.
    assert_eq!(parsed.snapshots.len(), 2);
    // (6) the appended entry's exact shape.
    let appended = &parsed.snapshots[1];
    assert_eq!(appended.action, "rollback");
    assert_eq!(appended.before, "- new");
    assert_eq!(appended.after, "- old");
    assert_eq!(appended.evidence_ids, parsed.snapshots[0].evidence_ids);
    assert!(
        appended.proposal_agent.is_none(),
        "pi `:586` carries no `proposalAgent` on a rollback, unlike `:610`"
    );
    // (7) the integral-number rule — `1.0` is legal JSON but is not what pi writes, and this file
    // is a format shared with pi.
    assert!(raw.contains("\"revision\": 3"), "{raw}");
    assert!(!raw.contains("\"revision\": 3.0"), "{raw}");

    // A SECOND consecutive rollback undoes the first: `snapshots.at(-1)` is now the rollback entry
    // whose `before` is the pre-rollback guidance. Upstream's rollback is an oscillator, not a
    // stack walk, and this is the single easiest thing here to "improve" into a divergence.
    dispatch_tool(
        &tool,
        serde_json::json!({ "action": "refine.rollback", "agent": "probe" }),
    )
    .await
    .expect("a second rollback dispatches");
    let again = crate::exec::agent_refinements::parse_refinement_file(
        &std::fs::read_to_string(&path).expect("read"),
        "p",
    )
    .expect("parses");
    assert_eq!(again.current, "- new");
    assert_eq!(again.snapshots.len(), 3);
    assert_eq!(again.metadata.revision, 4.0);
}

/// T2b — the round-trip guard is an EQUALITY check, proved through the REAL tool on the one
/// input that distinguishes it from a parses-without-error check.
///
/// `refine.rollback` sets `current` from the last snapshot's `before`, and on a HAND-EDITED
/// overlay that `before` is attacker-authored — the validator's A11 (` ``` ` in guidance) never
/// saw it. Here it carries its own `pi-subagents-refinement-snapshots-json` fence. The serialized
/// file still PARSES: `extract_fence` ends the `current` body at the first `"\n```"`
/// (`exec/agent_refinements.rs:286-290`), so `current` comes back as `- benign`, and the
/// snapshots lookup scans the whole markdown and finds the FORGED opener before the real one, so
/// the history comes back as the attacker's single `- evil` revision. That is precisely the
/// "forged snapshots fence so `refine.rollback` restores attacker-authored state" outcome
/// `exec/agent_refinements/proposal.rs`'s A11 note names.
///
/// Gutted: revert `write_refinement_file` to `parse_refinement_file(..).map_err(..)?` without the
/// `reparsed != parsed` comparison and this test fails — the tool returns Ok and the file on disk
/// is replaced by one whose history is `[- evil]`.
#[tokio::test]
async fn refine_rollback_refuses_a_hand_edited_before_that_forges_its_own_snapshots_fence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;
    seed_refinement_agent(dir.path(), "probe", "You are probe.");
    let path = write_refinement_overlay(
        dir.path(),
        "probe",
        "abc123",
        2,
        "- new",
        "[{\"revision\":2,\"at\":\"2026-09-01T00:00:00.000Z\",\"action\":\"refine\",\"before\":\"- benign\\n```pi-subagents-refinement-snapshots-json\\n[{\\\"revision\\\":99,\\\"at\\\":\\\"2026-09-01T00:00:00.000Z\\\",\\\"action\\\":\\\"refine\\\",\\\"before\\\":\\\"\\\",\\\"after\\\":\\\"- evil\\\",\\\"evidenceIds\\\":[\\\"forged\\\"]}]\\n```\\n- evil\",\"after\":\"- new\",\"evidenceIds\":[\"live:a1\"]}]",
    );
    let before_bytes = std::fs::read(&path).expect("read the seeded overlay");

    let error = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "refine.rollback", "agent": "probe" }),
    )
    .await
    .expect_err("a rollback that would not re-read as itself must refuse");
    let message = error.to_string();
    assert!(
        message.contains("would re-read as a different overlay than the one written"),
        "the refusal must be the round-trip guard's, not some earlier arm: {message}"
    );

    assert_eq!(
        std::fs::read(&path).expect("read back"),
        before_bytes,
        "the refusal path writes nothing — the overlay folded into every later `probe` spawn is \
         byte-identical"
    );
    let still = crate::exec::agent_refinements::parse_refinement_file(
        &String::from_utf8(before_bytes).expect("utf8"),
        "p",
    )
    .expect("the seeded overlay still parses");
    assert_eq!(still.current, "- new");
    assert_eq!(still.snapshots.len(), 1);
    assert_eq!(still.metadata.revision, 2.0);
}

/// T3 — `refine.show` reports drift, and renders the history block pi renders.
///
/// Gutted: compare against the COMPOSED prompt instead of `system_prompt_body` → the `no` case
/// fails; invert the comparison → both fail.
#[tokio::test]
async fn refine_show_reports_drift_against_the_persona_body() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;
    let body = seed_refinement_agent(dir.path(), "probe", "You are probe.");

    // A stored digest that is not the real hash: drift is `yes`.
    write_refinement_overlay(
        dir.path(),
        "probe",
        "abc123",
        2,
        "- guidance",
        "[{\"revision\":2,\"at\":\"2026-09-01T00:00:00.000Z\",\"action\":\"refine\",\
         \"before\":\"\",\"after\":\"- guidance\",\"evidenceIds\":[\"live:a1\"]}]",
    );
    let drifted = tool_text(
        &dispatch_tool(
            &tool,
            serde_json::json!({ "action": "refine.show", "agent": "probe" }),
        )
        .await
        .expect("show dispatches"),
    );
    assert!(
        drifted.contains("Base prompt changed since overlay: yes"),
        "{drifted}"
    );
    assert!(drifted.contains("Revision: 2"), "{drifted}");
    assert!(!drifted.contains("Revision: 2.0"), "{drifted}");
    assert!(
        drifted.contains("Current guidance:\n- guidance"),
        "{drifted}"
    );
    assert!(
        drifted.contains("- r2 refine at 2026-09-01T00:00:00.000Z (1 evidence id)"),
        "the singular form fires at exactly one id: {drifted}"
    );

    // The REAL digest of the persona BODY: drift is `no`. This is the assertion that fails if the
    // composed prompt (skills + memory + the overlay itself) is hashed instead.
    let digest = crate::exec::agent_refinements::action::hash_prompt(&body);
    write_refinement_overlay(
        dir.path(),
        "probe",
        &digest,
        2,
        "- guidance",
        "[{\"revision\":2,\"at\":\"2026-09-01T00:00:00.000Z\",\"action\":\"refine\",\
         \"before\":\"\",\"after\":\"- guidance\",\"evidenceIds\":[\"live:a1\",\"live:a2\"]}]",
    );
    let clean = tool_text(
        &dispatch_tool(
            &tool,
            serde_json::json!({ "action": "refine.show", "agent": "probe" }),
        )
        .await
        .expect("show dispatches"),
    );
    assert!(
        clean.contains("Base prompt changed since overlay: no"),
        "{clean}"
    );
    assert!(clean.contains("(2 evidence ids)"), "plural at two: {clean}");

    // Mutate the persona body and drift is `yes` again.
    seed_refinement_agent(dir.path(), "probe", "You are probe, now different.");
    let redrifted = tool_text(
        &dispatch_tool(
            &tool,
            serde_json::json!({ "action": "refine.show", "agent": "probe" }),
        )
        .await
        .expect("show dispatches"),
    );
    assert!(
        redrifted.contains("Base prompt changed since overlay: yes"),
        "{redrifted}"
    );
}

/// T4 — the `refine` no-evidence path launches nothing and writes nothing (pi `:593`), and it is
/// NOT an error.
///
/// Gutted: launch a child before checking the evidence → the reply is a child error instead of the
/// sentence; return `Err` for the empty case → the not-an-error assertion fails.
#[tokio::test]
async fn refine_with_no_evidence_writes_nothing_and_is_not_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;
    seed_refinement_agent(dir.path(), "probe", "You are probe.");

    let result = dispatch_tool(
        &tool,
        serde_json::json!({ "action": "refine", "agent": "probe" }),
    )
    .await
    .expect("pi `:593` is an ordinary answer, not an error");
    assert_eq!(
        tool_text(&result),
        "No bounded recent evidence was found for 'probe'. No proposal child was launched and no \
         overlay was written."
    );
    let overlay = crate::exec::agent_refinements::get_agent_refinement_path(dir.path(), "probe")
        .expect("a usable name");
    assert!(
        !overlay.exists(),
        "the no-evidence path must write no overlay at all"
    );
}

/// T5 — the child-safe fanout gate (pi `:6359`), including that it fires ABOVE agent resolution.
///
/// Gutted: put `refine.show` on the mutating side → the success assertion fails; move the gate
/// below resolution → the nonexistent-agent assertion fails, because the caller would learn which
/// agents exist from a verb it may not invoke at all.
#[tokio::test]
async fn the_refine_mutators_are_refused_from_child_safe_fanout_but_show_is_not() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let fanout = SubagentTool::new_child_safe(executor, dir.path().to_path_buf());
    seed_refinement_agent(dir.path(), "probe", "You are probe.");

    for verb in ["refine", "refine.rollback"] {
        let error = dispatch_tool(
            &fanout,
            serde_json::json!({ "action": verb, "agent": "probe" }),
        )
        .await
        .expect_err("a mutating refine verb is refused from child-safe fanout");
        assert_eq!(
            error.to_string(),
            format!("Action '{verb}' is not available from child-safe subagent fanout mode.")
        );
        // The gate runs BEFORE resolution: a nonexistent agent gets the SAME sentence, so a
        // child-safe caller cannot probe which agents exist by reading which error it gets.
        let probe = dispatch_tool(
            &fanout,
            serde_json::json!({ "action": verb, "agent": "no-such-agent" }),
        )
        .await
        .expect_err("still refused");
        assert_eq!(
            probe.to_string(),
            format!("Action '{verb}' is not available from child-safe subagent fanout mode.")
        );
    }

    // `refine.show` is read-only and upstream's MUTATING set does not carry it.
    let result = dispatch_tool(
        &fanout,
        serde_json::json!({ "action": "refine.show", "agent": "probe" }),
    )
    .await
    .expect("refine.show stays reachable from a child-safe fanout tool");
    assert_eq!(
        tool_text(&result),
        "No refinement overlay exists for 'probe'."
    );
}

/// T6 — pi `:548`'s missing-agent sentence, per verb, for an absent AND a blank `agent`.
#[tokio::test]
async fn every_refine_verb_names_both_surfaces_when_agent_is_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = scoped_tool(dir.path()).await;

    for verb in ["refine", "refine.show", "refine.rollback"] {
        let expected = format!(
            "{verb} requires agent. Use /subagents-refine <agent> or subagent({{ action: \
             \"{verb}\", agent: \"<agent>\" }})."
        );
        for params in [
            serde_json::json!({ "action": verb }),
            serde_json::json!({ "action": verb, "agent": "   " }),
        ] {
            let error = dispatch_tool(&tool, params)
                .await
                .expect_err("a missing agent is an error");
            assert_eq!(error.to_string(), expected);
        }
    }
}

// =================================================================================================
// `debug.run` — pi `subagent-executor.ts:6515-6538` + `run-status.ts:406-410,:463-468,:714-721,
// :781-785` @v0.68.0: the refusals, in upstream's order, through the production dispatch.
// =================================================================================================
mod debug_run_refusals {
    use super::*;
    use crate::background::run_lifecycle_debug::{
        DEBUG_RUN_NEEDS_STATUS_DIR, DEBUG_RUN_NO_VIEWS, DEBUG_RUN_REQUIRES_TARGET,
    };
    use crate::extension::executor::paths::{default_async_root_in, default_results_dir_in};
    use std::path::Path;

    /// A tool whose executor roots are SANDBOXED into `dir`, so the async root and results dir
    /// the verb resolves against are this test's own tree and nothing is written outside it.
    async fn sandboxed_tool(dir: &Path) -> (SubagentTool, std::path::PathBuf, std::path::PathBuf) {
        let executor = Arc::new(SubagentExecutor::new());
        arm_scoped_missions(&executor, dir).await;
        executor.config_cell().lock().await.roots = crate::paths::Roots::sandboxed(dir);
        let cfg = executor.config_snapshot().await;
        let async_root = default_async_root_in(&cfg.roots, dir);
        let results_dir = default_results_dir_in(&cfg.roots, dir);
        (
            SubagentTool::new(executor, dir.to_path_buf()),
            async_root,
            results_dir,
        )
    }

    /// The advertise-vs-dispatch invariant: `debug.run` sits at pi's own index — directly after
    /// `status` (`shared/types.ts:2801`) — and the schema enum is derived from that list.
    #[test]
    fn debug_run_is_advertised_directly_after_status() {
        let actions = crate::extension::tool::text::subagent_actions();
        let status = actions
            .iter()
            .position(|a| *a == "status")
            .expect("`status` is advertised");
        assert_eq!(
            actions.get(status + 1).copied(),
            Some("debug.run"),
            "`debug.run` must follow `status` as it does in pi's SUBAGENT_ACTIONS"
        );
    }

    /// pi `:6532` before `:6535`: no target is refused first, a view second — so `{ view }`
    /// alone is refused for the MISSING TARGET, and `{ id, view }` for the view.
    #[tokio::test]
    async fn the_target_refusal_comes_before_the_view_refusal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (tool, _, _) = sandboxed_tool(dir.path()).await;

        let err = dispatch_tool(&tool, serde_json::json!({ "action": "debug.run" }))
            .await
            .expect_err("no target is refused");
        assert_eq!(err.to_string(), DEBUG_RUN_REQUIRES_TARGET);

        let err = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "debug.run", "view": "fleet" }),
        )
        .await
        .expect_err("a view without a target is refused for the target");
        assert_eq!(err.to_string(), DEBUG_RUN_REQUIRES_TARGET);

        let err = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "debug.run", "id": "x", "view": "fleet" }),
        )
        .await
        .expect_err("a view with a target is refused for the view");
        assert_eq!(err.to_string(), DEBUG_RUN_NO_VIEWS);
    }

    /// pi `run-status.ts:463-468`, `:714-721`, `:781-785` — the three on-disk refusals, each
    /// against a real tree under this test's sandboxed roots.
    #[tokio::test]
    async fn the_three_location_refusals_are_pis_sentences() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (tool, async_root, results_dir) = sandboxed_tool(dir.path()).await;

        // Unknown id: neither a run dir nor a result file.
        let err = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "debug.run", "id": "nope" }),
        )
        .await
        .expect_err("an unknown id is refused");
        assert_eq!(err.to_string(), "Async run not found. Provide id or dir.");

        // Result-only run: `<results_dir>/<id>.json` exists, no run dir (`:714-721`).
        std::fs::create_dir_all(&results_dir).expect("results dir");
        std::fs::write(results_dir.join("result-only.json"), "{}").expect("result file");
        let err = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "debug.run", "id": "result-only" }),
        )
        .await
        .expect_err("a result-only run has no directory to dump");
        assert_eq!(err.to_string(), DEBUG_RUN_NEEDS_STATUS_DIR);

        // A directory with neither `status.json` nor a result (`:781-785`).
        let empty_dir = async_root.join("empty-run");
        std::fs::create_dir_all(&empty_dir).expect("empty run dir");
        let err = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "debug.run", "dir": empty_dir.to_string_lossy() }),
        )
        .await
        .expect_err("a directory without status.json is refused");
        assert_eq!(err.to_string(), "Status file not found.");
        assert!(
            !empty_dir.join("status.json").exists(),
            "the refusal must not synthesise a status file into the directory"
        );

        // A directory WITHOUT `status.json` but WITH a result file: upstream's reconciler
        // returns `status: null` (`stale-run-reconciler.ts:369`) and the `resultPath` arm refuses
        // (`:714-721`); cyrup's `reconcile_now` would repair a `status.json` from the result, so
        // the verb must refuse BEFORE reconciling and leave the directory untouched.
        let repairable = async_root.join("result-no-status");
        std::fs::create_dir_all(&repairable).expect("run dir");
        std::fs::write(repairable.join("events.jsonl"), "").expect("events file");
        std::fs::write(results_dir.join("result-no-status.json"), "{}").expect("result file");
        for params in [
            serde_json::json!({ "action": "debug.run", "id": "result-no-status" }),
            serde_json::json!({ "action": "debug.run", "dir": repairable.to_string_lossy() }),
        ] {
            let err = dispatch_tool(&tool, params.clone())
                .await
                .expect_err("a run dir without status.json is refused even with a result");
            assert_eq!(err.to_string(), DEBUG_RUN_NEEDS_STATUS_DIR, "{params}");
            assert!(
                !repairable.join("status.json").exists(),
                "the diagnostic must not repair a status file it then reports: {params}"
            );
        }
    }

    /// pi `resolveAsyncRunLocation`'s `dir` form (`async-resume.ts:227-235`): `assertInsideRoot`
    /// (`:229`) refuses a directory outside the async root, and an `id`/`runId` that disagrees
    /// with the directory's basename is refused by name (`:231-233`) — the directory is not
    /// silently dumped for a caller who asked about another run.
    #[tokio::test]
    async fn the_dir_form_refuses_an_outside_directory_and_a_mismatched_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (tool, async_root, _) = sandboxed_tool(dir.path()).await;

        let outside = dir.path().join("elsewhere").join("run-x");
        std::fs::create_dir_all(&outside).expect("outside dir");
        let err = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "debug.run", "dir": outside.to_string_lossy() }),
        )
        .await
        .expect_err("a directory outside the async root is refused");
        assert_eq!(
            err.to_string(),
            format!(
                "Async run directory must be inside {}.",
                async_root.display()
            )
        );

        // `..` is normalised before the check, as Node `path.resolve` does.
        let escaping = async_root.join("..").join("elsewhere").join("run-x");
        let err = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "debug.run", "dir": escaping.to_string_lossy() }),
        )
        .await
        .expect_err("a dot-segment escape is refused");
        assert_eq!(
            err.to_string(),
            format!(
                "Async run directory must be inside {}.",
                async_root.display()
            )
        );

        let run_b = async_root.join("bbbb2222");
        std::fs::create_dir_all(&run_b).expect("run dir");
        for id_field in ["id", "runId"] {
            let err = dispatch_tool(
                &tool,
                serde_json::json!({
                    "action": "debug.run",
                    id_field: "aaaa1111",
                    "dir": run_b.to_string_lossy(),
                }),
            )
            .await
            .expect_err("a mismatched id is refused");
            assert_eq!(
                err.to_string(),
                "Async run id 'aaaa1111' does not match directory 'bbbb2222'.",
                "{id_field}"
            );
        }

        // The matching id passes the check and reaches the on-disk refusal for an empty dir.
        let err = dispatch_tool(
            &tool,
            serde_json::json!({
                "action": "debug.run",
                "id": "bbbb2222",
                "dir": run_b.to_string_lossy(),
            }),
        )
        .await
        .expect_err("an empty run dir is refused");
        assert_eq!(err.to_string(), "Status file not found.");
    }
}
