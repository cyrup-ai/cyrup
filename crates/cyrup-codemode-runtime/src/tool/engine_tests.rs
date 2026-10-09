//! The `codemode` tool over the real V8 sandbox ([`EngineSandboxFactory`]): scripts are JavaScript,
//! and what is under test is the whole path from the model's `code` to the result text. The
//! sandbox's own behaviour is pinned in `sandbox::tests`; the tool's own contract with a scripted
//! sandbox is pinned in `execute::tests`. These are the cases where the two meet.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::Duration;

use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolResult};
use serde_json::{Value, json};

use super::{
    BranchCustomEntry, CodemodeHostSlot, CodemodeTool, CodemodeToolDetails, CodemodeToolOptions,
    EngineSandboxFactory,
};
use crate::testkit::{RecordingHost, StubTool};

fn tool_over(host: &Arc<RecordingHost>) -> CodemodeTool {
    let slot = CodemodeHostSlot::new();
    slot.bind(host.clone());
    CodemodeTool::new(CodemodeToolOptions::new(
        slot,
        Arc::new(EngineSandboxFactory),
    ))
}

fn host() -> Arc<RecordingHost> {
    Arc::new(RecordingHost::new(vec![
        StubTool::new("echo", "Echo text back.").arc(),
        StubTool::new("stats", "Return structured stats")
            .output_schema(json!({
                "type": "object",
                "properties": { "files": { "type": "number" } },
                "required": ["files"]
            }))
            .arc(),
    ]))
}

async fn run(tool: &CodemodeTool, code: &str) -> ToolResult {
    run_with(tool, code, CancelToken::new()).await
}

async fn run_with(tool: &CodemodeTool, code: &str, cancel: CancelToken) -> ToolResult {
    tool.execute(
        ToolCallId::from("call-1"),
        json!({ "code": code }),
        cancel,
        Box::new(|_| {}),
    )
    .await
    .unwrap()
}

fn header(result: &ToolResult) -> String {
    match result.content.first() {
        Some(Content::Text { text, .. }) => text.to_string(),
        other => panic!("no header: {other:?}"),
    }
}

fn body(result: &ToolResult) -> String {
    result.content[1..]
        .iter()
        .map(|block| match block {
            Content::Text { text, .. } => text.to_string(),
            Content::Image { .. } => "<image>".to_owned(),
            other => format!("<{other:?}>"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn details(result: &ToolResult) -> CodemodeToolDetails {
    serde_json::from_value(result.details.clone().unwrap()).unwrap()
}

/// A caller's cancel is an `AbortController.abort()` with no reason, whose reason is a
/// `DOMException` "This operation was aborted"; upstream renders `Script aborted: ${reason.message}`
/// (`execute.ts:255-256`). The model reads exactly this text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_call_reads_script_aborted_this_operation_was_aborted() {
    let tool = tool_over(&host());
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        trigger.cancel();
    });
    let result = run_with(&tool, "text('spinning'); while (true) {}", cancel).await;

    assert!(result.is_error);
    assert!(header(&result).starts_with("Script failed\n"));
    assert_eq!(
        body(&result),
        "spinning\nScript error:\nScript aborted: This operation was aborted\n\nNo tool calls were made."
    );
}

/// A call cancelled before the script starts is the same failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_cancelled_before_it_starts_reads_the_same() {
    let tool = tool_over(&host());
    let cancel = CancelToken::new();
    cancel.cancel();
    let result = run_with(&tool, "return 1", cancel).await;
    assert_eq!(
        body(&result),
        "Script error:\nScript aborted: This operation was aborted\n\nNo tool calls were made."
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_script_calls_tools_and_returns_a_value() {
    let host = host();
    *host.on_nested.lock().unwrap() = Some(Arc::new(|name, args| {
        if name == "stats" {
            let mut outcome = RecordingHost::text_outcome("call-1/2", "2 files", false);
            outcome.result.structured_content = Some(json!({ "files": 2 }));
            outcome
        } else {
            RecordingHost::text_outcome(
                "call-1/1",
                &format!("echo: {}", args["text"].as_str().unwrap()),
                false,
            )
        }
    }));
    let tool = tool_over(&host);
    let result = run(
        &tool,
        r#"
        const [a, stats] = await Promise.all([tools.echo({ text: "one" }), tools.stats({})]);
        console.log("files", stats.files);
        return { a, names: ALL_TOOLS.map((t) => t.name) };
        "#,
    )
    .await;

    assert!(!result.is_error, "{}", body(&result));
    assert!(header(&result).starts_with("Script completed\nWall time "));
    assert_eq!(
        body(&result),
        "{\"a\":\"echo: one\",\"names\":[\"echo\",\"stats\"]}\n<console_output>\nfiles 2\n</console_output>"
    );
    assert_eq!(
        details(&result)
            .calls
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["echo", "stats"]
    );
    // A tool without an output schema resolves to text; the host saw the caller's id.
    assert!(
        host.nested
            .lock()
            .unwrap()
            .iter()
            .all(|(caller, _, _)| caller.as_str() == "call-1")
    );
}

/// Upstream `marks where each text item starts and puts console lines last in one text block`
/// (pi `eb326d265`, `agent-session-codemode.test.ts:396-418` @v1.1.0): providers join adjacent text
/// blocks with nothing or a newline, so the output reaches the model as one block after the header.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_text_item_is_marked_and_console_lines_come_last_in_one_block() {
    let tool = tool_over(&host());
    let result = run(
        &tool,
        "text(\"one\\ntwo\");\nconsole.log(\"a\");\nconsole.log(\"b\");\ntext(\"three\\n\");\nreturn 4;",
    )
    .await;

    assert!(!result.is_error, "{}", body(&result));
    assert_eq!(result.content.len(), 2, "{:?}", result.content);
    assert_eq!(
        body(&result),
        "==> text 1/3 <==\none\ntwo\n==> text 2/3 <==\nthree\n==> text 3/3 <==\n4\n<console_output>\na\nb\n</console_output>"
    );
}

/// Upstream `reports script failures as results that keep partial output and the calls that ran`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_throwing_script_keeps_its_output_and_names_the_calls_that_ran() {
    let tool = tool_over(&host());
    let result = run(
        &tool,
        "text('partial');\nconsole.log('log');\nawait tools.echo({ text: 'x' });\nthrow new Error('boom');",
    )
    .await;

    assert!(result.is_error);
    assert!(header(&result).starts_with("Script failed\n"));
    let body = body(&result);
    assert!(
        body.starts_with(
            "partial\n<console_output>\nlog\n</console_output>\nScript error:\nError: boom\n"
        ),
        "{body}"
    );
    assert!(body.contains("codemode.js:4"), "{body}");
    assert!(body.contains("Tool calls made before the failure (they are not undone): echo (ok)"));
}

/// Upstream `applies the timeout_ms option and rejects invalid options`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_timeout_option_stops_a_spinning_script() {
    let tool = tool_over(&host());
    let result = run(&tool, "// @options: {\"timeout_ms\": 200}\nwhile (true) {}").await;
    assert!(result.is_error);
    assert!(
        body(&result).contains("Script error:\nScript timed out"),
        "{}",
        body(&result)
    );
}

/// Upstream `persists store() writes as custom entries for later calls` and `loads the values
/// written on the current branch`, over the real `store()`/`load()` globals.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_and_load_go_through_the_branchs_entries() {
    let host = host();
    let tool = tool_over(&host);
    let increment = "const next = (load('count') ?? 0) + 1;\nstore('count', next);\nreturn next;";

    assert_eq!(body(&run(&tool, increment).await), "1");
    let appended = host.appended.lock().unwrap().clone();
    assert_eq!(appended.len(), 1);
    assert_eq!(
        serde_json::to_value(&appended[0]).unwrap(),
        json!({ "set": { "count": 1 }, "delete": [] })
    );
    // The session replays what the host appended onto the branch.
    host.branch.lock().unwrap().push(BranchCustomEntry {
        custom_type: "codemode-store".to_owned(),
        data: Some(serde_json::to_value(&appended[0]).unwrap()),
    });
    assert_eq!(body(&run(&tool, increment).await), "2");

    // A failed script commits nothing.
    let before = host.appended.lock().unwrap().len();
    let failed = run(&tool, "store('count', 99); throw new Error('boom');").await;
    assert!(failed.is_error);
    assert_eq!(host.appended.lock().unwrap().len(), before);
}

/// The discovery globals reach the real sandbox.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_globals_are_callable_from_a_script() {
    let tool = tool_over(&host());
    let result = run(
        &tool,
        "const found = await searchTools('structured');\nconst sample = await describeTool('echo');\nreturn { found: found.map((t) => t.name), declared: sample.includes('tools: { echo(args:'), missing: (await describeTool('nope')) === undefined };",
    )
    .await;
    assert!(!result.is_error, "{}", body(&result));
    assert_eq!(
        serde_json::from_str::<Value>(&body(&result)).unwrap(),
        json!({ "found": ["stats"], "declared": true, "missing": true })
    );
}

/// An invalid `searchTools()` argument rejects with upstream's text, inside the script.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rejected_global_is_an_error_the_script_can_catch() {
    let tool = tool_over(&host());
    let result = run(
        &tool,
        "try { await searchTools('x', { limit: 0 }); } catch (e) { return e.message; }",
    )
    .await;
    assert_eq!(
        body(&result),
        "searchTools() limit must be a positive integer"
    );
}

/// Upstream `limits script memory so runaway allocations fail inside the script`: the tool passes
/// upstream's 256 MiB (`execute.ts:57`), and a runaway allocation ends the script with
/// `InternalError: out of memory`. Unlike QuickJS, V8's out-of-memory is fatal to the isolate, so the
/// script's own `catch` does not see it: the result is the failed one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runaway_allocation_ends_the_script_with_out_of_memory() {
    let tool = tool_over(&host());
    let result = run(
        &tool,
        "// @options: {\"timeout_ms\": 60000}\nlet a = [];\ntry { while (true) a.push('x'.repeat(1 << 20) + a.length); } catch (error) { return 'caught'; }",
    )
    .await;
    assert!(result.is_error);
    assert!(
        body(&result).contains("InternalError: out of memory"),
        "{}",
        body(&result)
    );
}
