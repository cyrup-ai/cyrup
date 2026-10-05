//! SUBA-150 / SUBA-151 — the `workflow` field end-to-end, from the advertised schema through
//! [`SubagentTool::lower_workflow_field`] to a run.
//!
//! A `#[path]` sibling of `routing.rs` for the same reason `routing_tests.rs` is one: every case
//! here drives `Tool::execute` against the boundary in that file, and the boundary needs a live
//! executor, a real session file on disk and the workflow engine.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::*;
use crate::extension::SubagentExecutor;
use crate::extension::testsupport::{
    FixedSessionIdHost, arm_scoped_missions, dispatch_tool, tool_text, workflow_script_path,
};
use cyrup_core::{Content, Message, ToolCall, ToolCallId, Usage};
use cyrup_session::{NewSessionOpts, SessionLayout, SessionManager};
use std::path::Path;
use std::sync::Arc;

/// The assistant turn a model reply is, as persisted: `text` blocks plus the `subagent` tool calls
/// it issued, in the order a provider streams them.
fn assistant_reply(text: &str, workflow_arguments: &[serde_json::Value]) -> Message {
    let mut content = vec![Content::text(text)];
    for (index, workflow) in workflow_arguments.iter().enumerate() {
        content.push(Content::ToolCall(ToolCall {
            id: ToolCallId::from(if index == 0 { "t" } else { "t2" }),
            name: crate::extension::TOOL_NAME.to_string(),
            arguments: serde_json::json!({ "workflow": workflow })
                .as_object()
                .expect("an object")
                .clone()
                .into(),
            thought_signature: None,
            namespace: None,
        }));
    }
    Message::Assistant(cyrup_core::AssistantMessage {
        content,
        provider: "faux".into(),
        model: "faux-1".to_string(),
        api: "faux".into(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: cyrup_core::StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    })
}

/// A tool whose executor reports a LIVE session file holding `reply` as its last assistant turn —
/// the state `workflow: true` reads, and the state the real agent loop leaves behind (the streamed
/// assistant message is persisted on `MessageEnd`, before `execute_tool_calls` runs).
async fn tool_with_reply(dir: &Path, reply: Message) -> SubagentTool {
    let layout = SessionLayout::new(dir.join("sessions"), dir.to_path_buf());
    let mut session = SessionManager::create(dir, &layout, NewSessionOpts::default())
        .expect("create the parent session");
    session
        .append_message(Message::User {
            content: vec![Content::text("go")],
            timestamp: 0,
        })
        .expect("append the user turn");
    session.append_message(reply).expect("append the reply");
    let file = session
        .session_file()
        .expect("the parent session is persisted")
        .to_path_buf();
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir).await;
    executor.set_host_services(Arc::new(FixedSessionIdHost {
        id: Some("session-workflow-field".to_string()),
        file: Some(file),
    }));
    SubagentTool::new(executor, dir.to_path_buf())
}

// ---------------------------------------------------------------------------------------------
// SUBA-150 — `workflow: true`, and the string `"true"` that means the same thing.
// ---------------------------------------------------------------------------------------------

/// THE USER ACTION, and the whole point of pi `0538e14d` (#2588): the model writes the workflow
/// inline in its reply, inside one ` ```js workflow ` block, and calls
/// `subagent({ workflow: true })`. A script passed as a JSON string has every quote and newline
/// escaped, which is a real source of malformed scripts; a fenced block has none of that.
///
/// Mutation killed: resolving `workflow: true` to anything but the reply's block — the script here
/// is only reachable through the persisted reply, so a boundary that did not read the session
/// cannot produce `Return: 42`.
#[tokio::test]
async fn workflow_true_runs_the_js_workflow_block_from_the_same_reply() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = tool_with_reply(
        dir.path(),
        assistant_reply(
            "Here is the plan.\n\n```js workflow\nreturn 42;\n```\nRunning it now.",
            &[serde_json::json!(true)],
        ),
    )
    .await;

    let result = dispatch_tool(&tool, serde_json::json!({ "workflow": true }))
        .await
        .expect("the reply's workflow block must run");
    assert!(
        tool_text(&result).contains("Workflow completed with 0 child run(s). Return: 42"),
        "{}",
        tool_text(&result)
    );
}

/// SUBA-150 fold-in (a) / pi `df3b6df1` (#2610, v0.75.0) — some MCP clients send the boolean branch
/// of the `workflow` union as the string `"true"`.
///
/// THE USER ACTION: before the fix the tool looked up a workflow resource named `true` and failed
/// with an error advising `workflow: true`, which the caller had already sent — so no retry could
/// ever succeed. This asserts the two spellings are the SAME call, against the same reply.
#[tokio::test]
async fn the_string_true_runs_the_reply_block_exactly_as_the_boolean_does() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reply = assistant_reply(
        "```js workflow\nreturn 42;\n```",
        &[serde_json::json!("true")],
    );
    let tool = tool_with_reply(dir.path(), reply).await;

    let result = dispatch_tool(&tool, serde_json::json!({ "workflow": "true" }))
        .await
        .expect("the stringified boolean must run the reply block, not look up a resource");
    assert!(
        tool_text(&result).contains("Workflow completed with 0 child run(s). Return: 42"),
        "{}",
        tool_text(&result)
    );
    assert!(
        !tool_text(&result).contains("Unknown workflow resource"),
        "a resource named 'true' must never be resolved: {}",
        tool_text(&result)
    );
}

/// pi `scriptFromReply`'s one-call-per-reply rule (`reply-workflow-script.ts:30-31`), and the
/// reason `df3b6df1` also changed the COUNT: the count includes the stringified form, so a reply
/// cannot smuggle a second script past the rule by sending one of each spelling.
#[tokio::test]
async fn two_reply_workflow_calls_in_one_reply_are_refused_including_the_stringified_form() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = tool_with_reply(
        dir.path(),
        assistant_reply(
            "```js workflow\nreturn 42;\n```",
            &[serde_json::json!(true), serde_json::json!("true")],
        ),
    )
    .await;

    let error = dispatch_tool(&tool, serde_json::json!({ "workflow": true }))
        .await
        .expect_err("two reply-block calls in one reply must be refused");
    assert_eq!(
        error.to_string(),
        "This reply has 2 subagent calls with workflow: true; a reply can carry only one. Pass \
         other scripts as workflow file paths.",
        "upstream's verbatim sentence, with the count"
    );
}

/// pi's fall-through (`reply-workflow-script.ts:25`): a caller that is not a model tool call whose
/// reply carries the block — an RPC spawn, a CLI invocation, a headless embedder with no persisted
/// session — gets the one sentence that names the form that does work for it.
#[tokio::test]
async fn workflow_true_with_no_reply_to_read_names_the_script_path_form() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());

    let error = dispatch_tool(&tool, serde_json::json!({ "workflow": true }))
        .await
        .expect_err("no session file means no reply to read");
    assert_eq!(
        error.to_string(),
        crate::extension::reply_workflow_script::NOT_A_MODEL_TOOL_CALL_REFUSAL,
    );
}

// ---------------------------------------------------------------------------------------------
// SUBA-150 — the two string forms: a script FILE and a named RESOURCE.
// ---------------------------------------------------------------------------------------------

/// pi `isWorkflowScriptPath` + `readWorkflowScriptFile`: a `workflow` value containing a separator
/// is a script file, read from the request cwd, and its text is what runs.
#[tokio::test]
async fn a_workflow_naming_a_path_runs_that_file_resolved_against_the_request_cwd() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());

    std::fs::create_dir_all(dir.path().join("flows")).expect("mkdir");
    std::fs::write(dir.path().join("flows/plan.js"), "return 7;").expect("write the script");

    // RELATIVE, so this also proves the path is resolved against the tool's cwd rather than the
    // process cwd (upstream `resolveRequestedCwd(runtimeCwd, requestedCwd)`).
    let result = dispatch_tool(&tool, serde_json::json!({ "workflow": "flows/plan.js" }))
        .await
        .expect("a script file must run");
    assert!(
        tool_text(&result).contains("Workflow completed with 0 child run(s). Return: 7"),
        "{}",
        tool_text(&result)
    );

    // A missing file reports the RESOLVED path, plus upstream's script-text hint — the hint exists
    // because models put script text in the string.
    let error = dispatch_tool(&tool, serde_json::json!({ "workflow": "flows/missing.js" }))
        .await
        .expect_err("a missing script file must be refused");
    let message = error.to_string();
    assert!(
        message.starts_with(&format!(
            "Failed to read workflow script '{}'",
            dir.path().join("flows/missing.js").display()
        )),
        "{message}"
    );
    assert!(
        message.ends_with(crate::extension::tool::workflow_field::SCRIPT_TEXT_HINT),
        "{message}"
    );
}

/// pi `resolveWorkflowResource` (`subagent-executor.ts:7941`): any other string is a NAMED
/// workflow resource, resolved through the registry this crate already owns, and its `args` are
/// the resource's — consumed by the resolver, never forwarded as workflow args.
///
/// `review` is one of upstream's two builtins (`workflow-resources.ts:165-168`); it expands to a
/// script that delegates the trimmed, JSON-escaped `task`. Validating the resolved script is what
/// proves the EXPANSION reached the carrier: a boundary that passed the name through would be
/// validating the literal text `review`, which is not a legal script.
#[tokio::test]
async fn a_workflow_naming_a_resource_resolves_through_the_registry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());

    let result = dispatch_tool(
        &tool,
        serde_json::json!({
            "action": "validate",
            "workflow": "review",
            "args": { "task": "check the diff" },
        }),
    )
    .await
    .expect("the resource must resolve and its expansion must validate");
    let details = result.details.as_ref().expect("details");
    assert_eq!(
        details["ok"],
        serde_json::json!(true),
        "the `review` builtin's expansion is a valid workflow script: {}",
        tool_text(&result)
    );

    // An unknown name gets upstream's resource sentence, listing the builtins, plus the hint.
    let error = dispatch_tool(&tool, serde_json::json!({ "workflow": "no-such-flow" }))
        .await
        .expect_err("an unknown resource must be refused");
    assert_eq!(
        error.to_string(),
        format!(
            "Unknown workflow resource 'no-such-flow'. Available resources: review, run-ci.{}",
            crate::extension::tool::workflow_field::SCRIPT_TEXT_HINT
        ),
    );
}

// ---------------------------------------------------------------------------------------------
// SUBA-150 — the two parameters v0.74.0 deleted.
// ---------------------------------------------------------------------------------------------

/// pi `removedModelWorkflowFieldError` (`public-execution.ts:70-74`): both removed parameters are
/// refused BY NAME, with the sentence that names the replacement form.
///
/// THE USER ACTION: a model trained on pi's older tool reference sends `workflowScript`. Silently
/// accepting it would leave the model never learning the new shape; refusing it without naming the
/// replacement would leave it unable to retry. Both sentences do both.
#[tokio::test]
async fn the_two_removed_parameters_are_refused_by_name_with_their_own_sentences() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());

    let error = dispatch_tool(&tool, serde_json::json!({ "workflowScript": "return 1;" }))
        .await
        .expect_err("workflowScript is no longer a tool parameter");
    assert_eq!(
        error.to_string(),
        crate::extension::tool::workflow_field::REMOVED_WORKFLOW_SCRIPT,
    );

    let error = dispatch_tool(
        &tool,
        serde_json::json!({ "workflowScriptPath": "./plan.js" }),
    )
    .await
    .expect_err("workflowScriptPath is no longer a tool parameter");
    assert_eq!(
        error.to_string(),
        crate::extension::tool::workflow_field::REMOVED_WORKFLOW_SCRIPT_PATH,
    );
}

/// pi `:107-109` — `false` is INVALID, not "no workflow", and the one sentence names all three
/// legal forms. A silently-ignored `workflow: false` would run SINGLE/`agent` mode instead.
#[tokio::test]
async fn workflow_false_is_refused_rather_than_treated_as_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());

    let error = dispatch_tool(&tool, serde_json::json!({ "workflow": false }))
        .await
        .expect_err("false is invalid");
    assert_eq!(
        error.to_string(),
        crate::extension::tool::workflow_field::INVALID_WORKFLOW_VALUE,
    );
}

/// pi `:117-119` — `args` without a workflow is refused by name rather than dropped. A dropped
/// `args` is the worst outcome: the run proceeds and the caller never learns its inputs were
/// ignored.
#[tokio::test]
async fn args_without_a_workflow_is_refused_rather_than_dropped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());

    let error = dispatch_tool(
        &tool,
        serde_json::json!({ "agent": "ghost", "task": "t", "args": { "a": 1 } }),
    )
    .await
    .expect_err("args requires a workflow");
    assert_eq!(
        error.to_string(),
        crate::extension::tool::workflow_field::ARGS_REQUIRES_WORKFLOW,
    );
}

// ---------------------------------------------------------------------------------------------
// SUBA-150 fold-in (b) — `action: "validate"` checks `args` against the launch's own limits.
// ---------------------------------------------------------------------------------------------

/// pi `cfb6f9a8` (#2611, v0.75.0) — `validate` with a workflow only checked the SCRIPT, so a call
/// with oversized or overly wide `args` validated and then failed at launch, one limit at a time.
///
/// All four limits, each tripped on its own, each reported beside the script verdict. Mutation
/// killed: dropping the `normalize_workflow_args` call from the `validate` arm, which makes every
/// row here report `ok: true`.
#[tokio::test]
async fn validate_refuses_args_that_exceed_each_of_the_four_limits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());
    let script = workflow_script_path("return 1;");

    // One over each bound, built FROM the constants so a bound change moves the fixture with it.
    let too_many_fields: serde_json::Map<String, serde_json::Value> = (0
        ..=crate::workflows::MAX_ARGS_FIELDS)
        .map(|i| (format!("f{i}"), serde_json::json!(1)))
        .collect();
    let too_many_items: Vec<serde_json::Value> = (0..=crate::workflows::MAX_ARGS_ITEMS)
        .map(|i| serde_json::json!(i))
        .collect();
    let mut too_deep = serde_json::json!(1);
    for _ in 0..=crate::workflows::MAX_ARGS_DEPTH {
        too_deep = serde_json::json!({ "n": too_deep });
    }
    // Over the TOTAL byte bound while every individual string stays under the separate per-string
    // bound, so this row trips `MAX_ARGS_BYTES` and not `MAX_STRING_BYTES`.
    let chunk = "x".repeat(crate::workflows::MAX_ARGS_BYTES / 4);
    let too_big = serde_json::json!({ "blob": vec![chunk; 8] });

    for (label, args, expected) in [
        (
            "fields",
            serde_json::Value::Object(too_many_fields),
            "workflow args contains too many fields.".to_string(),
        ),
        (
            "items",
            serde_json::json!({ "list": too_many_items }),
            "workflow args.list contains too many items.".to_string(),
        ),
        (
            "depth",
            serde_json::json!({ "deep": too_deep }),
            "workflow args.deep.n.n.n.n.n.n.n.n is too deeply nested.".to_string(),
        ),
        (
            "bytes",
            too_big,
            format!(
                "workflow args exceed {} bytes.",
                crate::workflows::MAX_ARGS_BYTES
            ),
        ),
    ] {
        let result = dispatch_tool(
            &tool,
            serde_json::json!({
                "action": "validate",
                "workflow": script.clone(),
                "args": args,
            }),
        )
        .await
        .unwrap_or_else(|error| panic!("{label}: validate answers, it does not fail: {error}"));
        let details = result.details.as_ref().expect("details");
        assert_eq!(
            details["ok"],
            serde_json::json!(false),
            "{label}: an args violation must make the verdict not-ok"
        );
        assert!(
            tool_text(&result).contains(&expected),
            "{label}: expected {expected:?}, got {}",
            tool_text(&result)
        );
    }
}

/// The anti-drift half, and the one worth more than two tests asserting the same number twice:
/// `validate`'s bound and the LAUNCH's bound are the same constant, so a value the launch path
/// accepts cannot be one `validate` refuses, or the other way round.
///
/// Walks the exact boundary of each limit — `MAX_ARGS_FIELDS` fields is legal, one more is not —
/// through BOTH surfaces: `action: "validate"` (the tool call) and
/// [`crate::workflows::normalize_workflow_args`] (what every launch and every persisted schedule
/// runs). A change to either path that did not move the other fails here.
#[tokio::test]
async fn validate_and_the_launch_path_agree_on_the_exact_boundary_of_every_limit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());
    let script = workflow_script_path("return 1;");

    let fields = |n: usize| -> serde_json::Value {
        serde_json::Value::Object(
            (0..n)
                .map(|i| (format!("f{i}"), serde_json::json!(1)))
                .collect(),
        )
    };
    let items = |n: usize| serde_json::json!({ "list": (0..n).collect::<Vec<usize>>() });
    let depth = |n: u32| {
        let mut value = serde_json::json!(1);
        for _ in 0..n {
            value = serde_json::json!({ "n": value });
        }
        serde_json::json!({ "deep": value })
    };
    // Split across eight strings so the per-string bound (a separate limit, same 16 KiB value) is
    // never the one that decides; `n` is the approximate TOTAL encoded size.
    let bytes = |n: usize| serde_json::json!({ "blob": vec!["x".repeat(n / 8); 8] });

    let cases = [
        (
            "fields",
            fields(crate::workflows::MAX_ARGS_FIELDS),
            fields(crate::workflows::MAX_ARGS_FIELDS + 1),
        ),
        (
            "items",
            items(crate::workflows::MAX_ARGS_ITEMS),
            items(crate::workflows::MAX_ARGS_ITEMS + 1),
        ),
        // `validate_plain_json` enters `args` itself at depth 0, so the deepest legal value sits
        // one level shallower than the constant's nominal count.
        (
            "depth",
            depth(crate::workflows::MAX_ARGS_DEPTH - 1),
            depth(crate::workflows::MAX_ARGS_DEPTH + 1),
        ),
        // The byte bound counts the whole encoded object, so the legal fixture leaves room for the
        // `{"blob":""}` envelope.
        (
            "bytes",
            bytes(crate::workflows::MAX_ARGS_BYTES - 128),
            bytes(crate::workflows::MAX_ARGS_BYTES + 128),
        ),
    ];

    for (label, legal, illegal) in cases {
        for (admitted, args) in [(true, legal), (false, illegal)] {
            // Surface 1 — the launch/schedule normaliser.
            let launch = crate::workflows::normalize_workflow_args(Some(&args));
            assert_eq!(
                launch.is_ok(),
                admitted,
                "{label}: the launch normaliser must {} this value",
                if admitted { "admit" } else { "refuse" }
            );
            // Surface 2 — `action: "validate"`, through the real tool call.
            let result = dispatch_tool(
                &tool,
                serde_json::json!({
                    "action": "validate",
                    "workflow": script.clone(),
                    "args": args,
                }),
            )
            .await
            .unwrap_or_else(|error| panic!("{label}: validate answers: {error}"));
            assert_eq!(
                result.details.as_ref().expect("details")["ok"],
                serde_json::json!(admitted),
                "{label}: validate and the launch path must agree; validate said {} — {}",
                result.details.as_ref().expect("details")["ok"],
                tool_text(&result)
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// SUBA-151 — what a caller who used `chain`/`tasks` gets.
// ---------------------------------------------------------------------------------------------

/// SUBA-151 — upstream's v0.74.0 default tool has NO `chain`/`tasks`: `normalizePublicSubagentExecution`
/// answers *"Legacy top-level chain and parallel inputs were removed; use a workflow script"*
/// unless `options.structuredWorkflows` is set, which happens only when `disabledFeatures` lists
/// `workflow-scripts` (`public-execution.ts:179-181` @v0.75.0), and the reduced inputs are then
/// COMPILED into a package-owned script (`structured-workflow-scripts.ts:175`).
///
/// cyrup keeps both engines, so a `chain`/`tasks` caller keeps NATIVE execution — a strict
/// superset of upstream's reduced shape, with the workflow-script route available alongside it.
/// This pins that: the two shapes still dispatch to the native graph, and neither is answered with
/// a legacy-removal refusal. If a later change removes them from the default tool, this test is
/// what will say that a cyrup caller lost a capability rather than gained parity.
#[tokio::test]
async fn chain_and_tasks_still_dispatch_to_the_native_graph_rather_than_a_removal_refusal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());

    for params in [
        serde_json::json!({ "tasks": [{ "agent": "ghost-a", "task": "survey" }] }),
        serde_json::json!({ "chain": [{ "agent": "ghost-a", "task": "survey" }] }),
    ] {
        let error = dispatch_tool(&tool, params.clone())
            .await
            .expect_err("`ghost-a` is not a discoverable agent, so the native graph refuses it");
        let message = error.to_string();
        // The refusal is AGENT RESOLUTION's — i.e. the call reached the native engine.
        assert!(
            message.contains("ghost-a"),
            "{params}: the call must reach native agent resolution; got {message}"
        );
        assert!(
            !message.contains("Legacy top-level chain and parallel inputs were removed"),
            "{params}: cyrup supports these shapes natively and must not answer with upstream's \
             reduced-tool refusal; got {message}"
        );
    }
}
