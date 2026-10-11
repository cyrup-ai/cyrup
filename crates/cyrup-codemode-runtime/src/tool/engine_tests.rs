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

/// [CYRUP-DELTA] A script that ends with calls still running succeeded, and the text says what became
/// of them: an `await` forgotten is the commonest mistake in a generated script, and upstream's text
/// reads as a plain success (nothing, or "done", for a script whose writes never happened).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_script_that_ends_with_calls_still_running_says_they_were_cancelled() {
    let host = host();
    // No call gets through, so every one is still running when the script ends.
    *host.gate.lock().unwrap() = Some(Arc::new(tokio::sync::Semaphore::new(0)));
    let tool = tool_over(&host);
    let result = run(
        &tool,
        r#"
        async function main() {
            const a = await tools.echo({ text: "a" });
            text(a);
        }
        main();
        ["x", "y"].forEach(async (n) => { await tools.echo({ text: n }); });
        tools.stats({});
        return "done";
        "#,
    )
    .await;

    assert!(!result.is_error, "{}", body(&result));
    assert_eq!(
        body(&result),
        "done\nNote: 4 tool calls were still running when the script ended and were cancelled: echo x3, stats. Await every call before the script ends (await Promise.all([...]), await main())."
    );
    assert!(
        details(&result)
            .calls
            .iter()
            .all(|call| call.status == crate::tool::CodemodeNestedCallStatus::Cancelled)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_call_left_running_is_named_in_the_singular_and_settled_calls_add_nothing() {
    let host = host();
    let tool = tool_over(&host);
    let awaited = run(
        &tool,
        "await Promise.all([tools.echo({ text: 'a' }), tools.echo({ text: 'b' })]); return 'ok';",
    )
    .await;
    assert_eq!(body(&awaited), "ok");

    *host.gate.lock().unwrap() = Some(Arc::new(tokio::sync::Semaphore::new(0)));
    let left = run(&tool, "tools.stats({}); return 'ok';").await;
    assert_eq!(
        body(&left),
        "ok\nNote: 1 tool call was still running when the script ended and was cancelled: stats. Await every call before the script ends (await Promise.all([...]), await main())."
    );
}

/// A host whose `echo` fails with `ENOENT: no such file` and whose other tools succeed.
fn host_where_echo_fails() -> Arc<RecordingHost> {
    let host = host();
    *host.on_nested.lock().unwrap() = Some(Arc::new(|name, _args| {
        RecordingHost::text_outcome("call-1/1", "ENOENT: no such file", name == "echo")
    }));
    host
}

/// [CYRUP-DELTA] A call that failed and that the script never awaited used to leave nothing in the
/// result: the script succeeded, and a model that forgot an `await` on a `write` or an `edit` was told
/// its side effect had happened. Measured through the real binary before the fix: the model received
/// `Script completed ... done: 3 files` for a script whose `tools.write(...)` had failed. The error
/// is named by the tool, whichever way the call got away from the script.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_call_the_script_never_awaited_is_named_in_the_result() {
    let tool = tool_over(&host_where_echo_fails());
    let note = |tail: &str| {
        format!(
            "Note: 1 error was never handled and did not fail the script: {tail}. A tool call or async function that is not awaited loses its error. Await every call before the script ends (await Promise.all([...]), await main()) and catch the errors you expect."
        )
    };

    // The script waits for another call, so the failure reaches it while it runs and nothing
    // handles it.
    let heard = run(
        &tool,
        "tools.echo({ text: 'x' });\nawait tools.stats({});\nreturn 'done';",
    )
    .await;
    assert!(!heard.is_error, "{}", body(&heard));
    assert_eq!(
        body(&heard),
        format!(
            "done\n{}",
            note("echo: ENOENT: no such file (codemode.js:1:7)")
        )
    );

    // The script ends on the line that made the call: the call fails in the host, and the script is
    // gone before the isolate could be told.
    let unheard = run(&tool, "tools.echo({ text: 'x' });\nreturn 'done';").await;
    assert!(!unheard.is_error, "{}", body(&unheard));
    assert_eq!(
        body(&unheard),
        format!("done\n{}", note("echo: ENOENT: no such file"))
    );

    // A `.then` without a `.catch` hands the rejection on to a promise nobody looks at.
    let chained = run(
        &tool,
        "tools.echo({ text: 'x' }).then((r) => text(r));\nawait tools.stats({});\nreturn 'done';",
    )
    .await;
    assert_eq!(
        body(&chained),
        format!(
            "done\n{}",
            note("echo: ENOENT: no such file (codemode.js:1:7)")
        )
    );
}

/// [CYRUP-DELTA] A call that failed after the script ended, and that something was waiting on, is
/// named in a note of its own that gives no advice. The siblings of a `Promise.all` that had already
/// rejected were awaited, in a `try`/`catch`: telling that script to "await every call" was wrong, and
/// it is the note the earlier lane gave. The same state is a forgotten `await` in `main();` and in a
/// `forEach(async ...)` callback: both have a reaction attached to the call and both would read as a
/// plain success without the note, a `write` that failed told as a write that happened.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_call_something_was_waiting_on_is_named_without_advice() {
    let tool = tool_over(&host_where_echo_fails());
    let late = |count: usize, tail: &str| {
        let (calls, errors) = if count == 1 {
            ("call", "error")
        } else {
            ("calls", "errors")
        };
        format!(
            "Note: {count} {calls} failed after the script ended, so the script did not see the {errors}: {tail}."
        )
    };
    let no_advice = |text: &str| {
        assert!(!text.contains("never handled"), "{text}");
        assert!(!text.contains("Await every call"), "{text}");
    };

    // The script awaited every call and caught the failure it was handed.
    let all = run(
        &tool,
        r#"
        try { await Promise.all([tools.echo({ text: "a" }), tools.echo({ text: "b" }), tools.echo({ text: "c" })]); }
        catch (e) { text("caught " + e.message); }
        return "done";
        "#,
    )
    .await;
    assert!(!all.is_error, "{}", body(&all));
    assert_eq!(
        body(&all),
        format!(
            "==> text 1/2 <==\ncaught ENOENT: no such file\n==> text 2/2 <==\ndone\n{}",
            late(2, "echo: ENOENT: no such file; echo: ENOENT: no such file")
        )
    );
    no_advice(&body(&all));

    // The call is inside an `async` function nobody awaited.
    let main = run(
        &tool,
        "async function main() { await tools.echo({ text: 'x' }); }\nmain();\nreturn 'done';",
    )
    .await;
    assert!(!main.is_error, "{}", body(&main));
    assert_eq!(
        body(&main),
        format!("done\n{}", late(1, "echo: ENOENT: no such file"))
    );
    no_advice(&body(&main));

    let for_each = run(
        &tool,
        "['x', 'y'].forEach(async (n) => { await tools.echo({ text: n }); });\nreturn 'done';",
    )
    .await;
    assert_eq!(
        body(&for_each),
        format!(
            "done\n{}",
            late(2, "echo: ENOENT: no such file; echo: ENOENT: no such file")
        )
    );

    // A call nothing waits on is still the "never handled" note, and the two do not hide each other.
    let both = run(
        &tool,
        "async function main() { await tools.echo({ text: 'x' }); }\nmain();\ntools.echo({ text: 'y' });\nreturn 'done';",
    )
    .await;
    assert_eq!(
        body(&both),
        format!(
            "done\nNote: 1 error was never handled and did not fail the script: echo: ENOENT: no such file. A tool call or async function that is not awaited loses its error. Await every call before the script ends (await Promise.all([...]), await main()) and catch the errors you expect.\n{}",
            late(1, "echo: ENOENT: no such file")
        )
    );

    // Nothing failed late, nothing to say.
    let fine = run(
        &tool,
        "async function main() { await tools.stats({}); }\nmain();\nreturn 'done';",
    )
    .await;
    assert_eq!(body(&fine), "done");
}

/// An error the script handled is its own business: a `try`/`catch`, a `.catch`, an `allSettled`, and
/// a call whose promise was kept and awaited after the call had already failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn errors_the_script_handled_are_not_reported() {
    let tool = tool_over(&host_where_echo_fails());
    let result = run(
        &tool,
        r#"
        try { await tools.echo({ text: "a" }); } catch (e) { text("caught " + e.message); }
        await tools.echo({ text: "b" }).catch((e) => text("then-caught " + e.message));
        const settled = await Promise.allSettled([tools.echo({ text: "c" }), tools.stats({})]);
        text(settled.map((r) => r.status).join(","));
        const kept = tools.echo({ text: "d" });
        await tools.stats({});
        try { await kept; } catch (e) { text("late " + e.message); }
        return "done";
        "#,
    )
    .await;
    assert!(!result.is_error, "{}", body(&result));
    assert_eq!(
        body(&result),
        "==> text 1/5 <==\ncaught ENOENT: no such file\n==> text 2/5 <==\nthen-caught ENOENT: no such file\n==> text 3/5 <==\nrejected,fulfilled\n==> text 4/5 <==\nlate ENOENT: no such file\n==> text 5/5 <==\ndone"
    );
}

/// A rejection that is not a call's is reported too: an `async` function that threw and was not
/// awaited, and a `Promise.reject` on the script's last line, which the engine has not looked at yet
/// when the script returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_async_function_that_threw_unawaited_is_named_in_the_result() {
    let tool = tool_over(&host());
    let thrown = run(
        &tool,
        "async function f() { throw new Error('boom'); }\nf();\nawait tools.stats({});\nreturn 'survived';",
    )
    .await;
    assert!(!thrown.is_error, "{}", body(&thrown));
    assert_eq!(
        body(&thrown),
        "survived\nNote: 1 error was never handled and did not fail the script: Error: boom (f (codemode.js:1:28)). A tool call or async function that is not awaited loses its error. Await every call before the script ends (await Promise.all([...]), await main()) and catch the errors you expect."
    );

    let last_line = run(&tool, "Promise.reject(new Error('late'));\nreturn 'done';").await;
    assert!(!last_line.is_error, "{}", body(&last_line));
    assert!(
        body(&last_line).starts_with("done\nNote: 1 error was never handled and did not fail the script: Error: late (codemode.js:1:16)."),
        "{}",
        body(&last_line)
    );
}

/// The result names the first few and counts the rest, and each is cut to a line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn many_unhandled_errors_are_counted_and_the_first_few_named() {
    let tool = tool_over(&host());
    let result = run(
        &tool,
        "for (let i = 0; i < 8; i++) Promise.reject(new Error('e' + i + '\\n' + 'x'.repeat(400)));\nawait tools.stats({});\nreturn 'done';",
    )
    .await;
    assert!(!result.is_error, "{}", body(&result));
    let body = body(&result);
    assert!(
        body.contains(
            "Note: 8 errors were never handled and did not fail the script: Error: e0 xxx"
        ),
        "{body}"
    );
    assert!(body.contains("(and 3 more)"), "{body}");
    assert!(body.contains("\u{2026}"), "{body}");
    assert_eq!(body.matches("Error: e").count(), 5, "{body}");
    assert!(!body.contains("Error: e5"), "{body}");
    assert!(!body.contains(&"x".repeat(301)), "{body}");
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

/// [CYRUP-DELTA] Measured: a blank line before the options line made the line a comment, so
/// `timeout_ms` was dropped and the loop ran until the process was killed at 40 s.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_options_line_after_a_blank_line_or_inside_a_fence_still_sets_the_timeout() {
    let tool = tool_over(&host());
    for code in [
        "\n// @options: {\"timeout_ms\": 200}\nwhile (true) {}",
        "```js\n// @options: {\"timeout_ms\": 200}\nwhile (true) {}\n```",
    ] {
        let started = std::time::Instant::now();
        let result = run(&tool, code).await;
        assert!(result.is_error, "{code:?}");
        assert!(
            body(&result)
                .contains("Script error:\nScript timed out: Execution timed out after 200 ms"),
            "{code:?}: {}",
            body(&result)
        );
        // An explicit timeout_ms is the model's own: no advice about the defaults.
        assert!(
            !body(&result).contains("Default limits"),
            "{}",
            body(&result)
        );
        assert!(started.elapsed() < Duration::from_secs(10), "{code:?}");
    }
}

/// [CYRUP-DELTA] A script wrapped in a fence used to reach the engine as a tagged template and fail
/// with `TypeError: "" is not a function` at `codemode.js:1:31`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_script_in_a_markdown_fence_runs_and_an_unclosed_one_is_refused() {
    let tool = tool_over(&host());
    let result = run(&tool, "```js\ntext('fenced');\n```").await;
    assert!(!result.is_error, "{}", body(&result));
    assert_eq!(body(&result), "fenced");
    let error = tool
        .execute(
            ToolCallId::from("call-2"),
            json!({ "code": "```js\ntext(1)" }),
            CancelToken::new(),
            Box::new(|_| {}),
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("never closed"), "{}", error.message);
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

/// Two MCP servers `a-b` and `a_b` each with a tool `search` normalise to one identifier. Upstream
/// registers the first and the script can never reach the second; here `tools.<id>`, `ALL_TOOLS`,
/// `searchTools()` and `describeTool()` agree on one identifier per tool, and each call reaches the
/// tool it names.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tools_with_one_normalised_identifier_are_each_callable_from_a_script() {
    let host = Arc::new(RecordingHost::new(vec![
        StubTool::new("mcp__a-b__search", "Search a-b.").arc(),
        StubTool::new("mcp__a_b__search", "Search a_b.").arc(),
    ]));
    *host.on_nested.lock().unwrap() = Some(Arc::new(|name, _args| {
        RecordingHost::text_outcome("call-1/1", &format!("called {name}"), false)
    }));
    let tool = tool_over(&host);
    let result = run(
        &tool,
        r#"
        const found = (await searchTools("search")).map((t) => t.name).sort();
        return {
          all: ALL_TOOLS.map((t) => t.name).sort(),
          found,
          plain: await tools.mcp__a_b__search({}),
          renamed: await tools.mcp__a_b__search_2({}),
          bracket: typeof tools["mcp__a-b__search"],
          described: (await describeTool("mcp__a_b__search_2")).includes("mcp__a_b__search_2(args:"),
        };
        "#,
    )
    .await;
    assert!(!result.is_error, "{}", body(&result));
    assert_eq!(
        serde_json::from_str::<Value>(&body(&result)).unwrap(),
        json!({
            "all": ["mcp__a_b__search", "mcp__a_b__search_2"],
            "found": ["mcp__a_b__search", "mcp__a_b__search_2"],
            "plain": "called mcp__a_b__search",
            "renamed": "called mcp__a-b__search",
            "bracket": "function",
            "described": true,
        })
    );
}
