//! What the tool does with the sandbox's inputs and outputs (pi `executeCodemode`,
//! `extensions/codemode/execute.ts` @v1.0.1), driven through [`CodemodeTool`]'s `execute` with a
//! scripted sandbox and a recording host.
//!
//! Upstream's session-level cases for the same behaviour are ported against a real session in
//! `cyrup-session-svc` (`src/tests/codemode.rs`); here the assertions are about the tool's own
//! contract: the result shape, the details, the error texts, the budget and the store.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_codemode::types::OutputItem;
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdate};
use serde_json::{Value, json};

use super::{format_call_summary, format_error, preview_args, to_fixed_1, value_text};
use crate::testkit::{
    RecordingHost, Script, ScriptEnv, ScriptedSandboxFactory, StubTool, completed, failed,
};
use crate::tool::recorder::truncate_text;
use crate::tool::{
    CodemodeHostSlot, CodemodeNestedCall, CodemodeNestedCallStatus, CodemodeTool,
    CodemodeToolDetails, CodemodeToolOptions, UnavailableSandboxFactory,
};
use crate::types::{CodemodeError, CodemodeResult, CodemodeStoreWrites, Deadline, ErrorKind};

const TINY_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(String, ScriptEnv) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = CodemodeResult> + Send + 'static,
{
    Arc::new(move |code, env| Box::pin(f(code, env)))
}

struct Rig {
    tool: CodemodeTool,
    host: Arc<RecordingHost>,
    factory: ScriptedSandboxFactory,
    updates: Arc<Mutex<Vec<ToolUpdate>>>,
}

fn rig(tools: Vec<Arc<dyn Tool>>, script: Script) -> Rig {
    let slot = CodemodeHostSlot::new();
    let host = Arc::new(RecordingHost::new(tools));
    slot.bind(host.clone());
    let factory = ScriptedSandboxFactory::new(script);
    let tool = CodemodeTool::new(CodemodeToolOptions::new(slot, Arc::new(factory.clone())));
    Rig {
        tool,
        host,
        factory,
        updates: Arc::new(Mutex::new(Vec::new())),
    }
}

impl Rig {
    async fn run(&self, code: &str) -> Result<ToolResult, ToolError> {
        self.run_with(code, CancelToken::new()).await
    }

    async fn run_with(&self, code: &str, cancel: CancelToken) -> Result<ToolResult, ToolError> {
        let updates = Arc::clone(&self.updates);
        self.tool
            .execute(
                ToolCallId::from("call-1"),
                json!({ "code": code }),
                cancel,
                Box::new(move |update| updates.lock().unwrap().push(update)),
            )
            .await
    }
}

fn tools() -> Vec<Arc<dyn Tool>> {
    vec![
        StubTool::new("echo", "Echo text back.").arc(),
        StubTool::new("stats", "Return structured stats")
            .output_schema(json!({
                "type": "object",
                "properties": { "files": { "type": "number" } },
                "required": ["files"]
            }))
            .arc(),
    ]
}

/// The text after the header, which is checked on the way (upstream's `resultText`).
fn result_text(result: &ToolResult) -> String {
    let Some(Content::Text { text: header, .. }) = result.content.first() else {
        panic!("no header: {:?}", result.content);
    };
    let header = header.to_string();
    let lines: Vec<&str> = header.split('\n').collect();
    assert!(
        lines.len() == 4
            && (lines[0] == "Script completed" || lines[0] == "Script failed")
            && lines[1].starts_with("Wall time ")
            && lines[1].ends_with(" seconds")
            && lines[2] == "Output:"
            && lines[3].is_empty(),
        "bad header {header:?}"
    );
    let seconds = lines[1]
        .trim_start_matches("Wall time ")
        .trim_end_matches(" seconds");
    let (whole, tenth) = seconds.split_once('.').unwrap();
    assert!(
        whole.chars().all(|c| c.is_ascii_digit()) && tenth.len() == 1,
        "{seconds}"
    );
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

/// What `image()` leaves in the result since pi `d677d0ee7` (v1.0.3, #10310): the label line
/// `[Image saved to <path>.png (image/png, <n>B)]` before the image. Checks that the file holds
/// `TINY_PNG` and is readable only by its owner, removes it, and replaces the label with `<saved>`
/// (upstream's `checkSavedImages`, `agent-session-codemode.test.ts`).
fn check_saved_images(text: &str) -> String {
    use base64::Engine as _;
    text.split('\n')
        .map(|line| {
            let Some(rest) = line.strip_prefix("[Image saved to ") else {
                return line.to_owned();
            };
            let (path, kind) = rest.split_once(" (image/png, ").expect(line);
            assert!(path.ends_with(".png") && !path.contains(' '), "{line}");
            assert!(
                kind.ends_with("B)]") && kind[..kind.len() - 3].chars().all(|c| c.is_ascii_digit()),
                "{line}"
            );
            let bytes = std::fs::read(path).unwrap();
            assert_eq!(
                bytes,
                base64::engine::general_purpose::STANDARD
                    .decode(TINY_PNG)
                    .unwrap()
            );
            assert_private_file(std::path::Path::new(path));
            std::fs::remove_file(path).unwrap();
            "<saved>".to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Output files can carry private data, so only the user may read them (pi `OUTPUT_FILE_MODE`,
/// `utils/output-files.ts` @v1.0.3). Unix only: Windows has no such mode bits.
fn assert_private_file(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}: mode {mode:o}", path.display());
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn text(s: &str) -> OutputItem {
    OutputItem::Text(s.to_owned())
}

// ------------------------------------------------------------------------------------ results --

/// Upstream `runs nested calls in parallel and returns only the script result`.
#[tokio::test]
async fn runs_nested_calls_in_parallel_and_returns_only_the_script_result() {
    let rig = rig(
        tools(),
        script(|_code, env| async move {
            let (a, b, stats) = tokio::join!(
                env.tool("echo", json!({ "text": "one" })),
                env.tool("echo", json!({ "text": "two" })),
                env.tool("stats", json!({})),
            );
            let stats = stats.unwrap().unwrap();
            completed(
                vec![text(&format!("files {}", stats["files"]))],
                Some(json!({ "a": a.unwrap(), "b": b.unwrap(), "names": stats["names"] })),
            )
        }),
    );
    *rig.host.on_nested.lock().unwrap() = Some(Arc::new(|name, args| {
        if name == "stats" {
            let mut outcome = RecordingHost::text_outcome("call-1/3", "2 files", false);
            outcome.result.structured_content = Some(json!({ "files": 2, "names": ["a", "b"] }));
            outcome
        } else {
            RecordingHost::text_outcome(
                "call-1/1",
                &format!("echo: {}", args["text"].as_str().unwrap()),
                false,
            )
        }
    }));

    let result = rig.run("run").await.unwrap();

    assert!(!result.is_error);
    assert_eq!(
        result_text(&result),
        "files 2\n{\"a\":\"echo: one\",\"b\":\"echo: two\",\"names\":[\"a\",\"b\"]}"
    );
    let details = details(&result);
    assert_eq!(
        details
            .calls
            .iter()
            .map(|call| (call.name.as_str(), call.status))
            .collect::<Vec<_>>(),
        [
            ("echo", CodemodeNestedCallStatus::Ok),
            ("echo", CodemodeNestedCallStatus::Ok),
            ("stats", CodemodeNestedCallStatus::Ok),
        ]
    );
    assert!(
        details
            .calls
            .iter()
            .all(|call| call.id.starts_with("call-1/"))
    );
    assert!(details.calls.iter().all(|call| call.duration_ms.is_some()));
    // Every nested call was made on behalf of the codemode tool call.
    let nested = rig.host.nested.lock().unwrap();
    assert_eq!(nested.len(), 3);
    assert!(
        nested
            .iter()
            .all(|(caller, _, _)| caller.as_str() == "call-1")
    );
}

/// `toScriptValue`: a tool with an output schema resolves to its structured content, also for an
/// error result that carries one (an MCP `CallToolResult` with `isError`); a tool without one
/// resolves to its text, and an error result rejects with that text.
#[tokio::test]
async fn nested_results_resolve_per_the_tools_output_schema() {
    let rig = rig(
        tools(),
        script(|_code, env| async move {
            let structured_error = env.tool("stats", json!({})).await;
            let text_error = env.tool("echo", json!({})).await;
            let empty_error = env.tool("echo", json!({ "empty": true })).await;
            completed(
                Vec::new(),
                Some(json!({
                    "structured": structured_error.unwrap(),
                    "text": text_error.unwrap_err(),
                    "empty": empty_error.unwrap_err(),
                })),
            )
        }),
    );
    *rig.host.on_nested.lock().unwrap() = Some(Arc::new(|name, args| {
        if name == "stats" {
            let mut outcome = RecordingHost::text_outcome("call-1/1", "partial failure", true);
            outcome.result.structured_content = Some(json!({ "files": 0, "isError": true }));
            outcome
        } else if args.get("empty").is_some() {
            RecordingHost::text_outcome("call-1/3", "", true)
        } else {
            RecordingHost::text_outcome("call-1/2", "echo of forbidden text is blocked", true)
        }
    }));

    let result = rig.run("run").await.unwrap();

    assert_eq!(
        serde_json::from_str::<Value>(&result_text(&result)).unwrap(),
        json!({
            "structured": { "files": 0, "isError": true },
            "text": "echo of forbidden text is blocked",
            "empty": "Tool \"echo\" failed",
        })
    );
    let details = details(&result);
    assert_eq!(
        details
            .calls
            .iter()
            .map(|call| call.status)
            .collect::<Vec<_>>(),
        [
            CodemodeNestedCallStatus::Error,
            CodemodeNestedCallStatus::Error,
            CodemodeNestedCallStatus::Error
        ]
    );
    assert_eq!(details.calls[0].error.as_deref(), Some("partial failure"));
    assert_eq!(
        details.calls[2].error.as_deref(),
        Some("Tool \"echo\" failed"),
        "the row says what the script saw when the tool gave no text"
    );
}

/// An error row is `cancelled`, not `error`, when the call's own abort signal was aborted.
#[tokio::test]
async fn a_nested_call_that_failed_after_its_signal_aborted_is_recorded_as_cancelled() {
    let rig = rig(
        tools(),
        script(|_code, env| async move {
            let aborted = CancelToken::new();
            aborted.cancel();
            let cancelled = env.tool_with_cancel("echo", json!({}), aborted).await;
            let live = env.tool("echo", json!({})).await;
            completed(
                Vec::new(),
                Some(json!({ "cancelled": cancelled.unwrap_err(), "live": live.unwrap_err() })),
            )
        }),
    );
    *rig.host.on_nested.lock().unwrap() = Some(Arc::new(|_, _| {
        RecordingHost::text_outcome("call-1/1", "aborted", true)
    }));
    let result = rig.run("run").await.unwrap();
    assert_eq!(
        details(&result)
            .calls
            .iter()
            .map(|c| c.status)
            .collect::<Vec<_>>(),
        [
            CodemodeNestedCallStatus::Cancelled,
            CodemodeNestedCallStatus::Error
        ]
    );
}

/// Upstream `reports script failures as results that keep partial output and the calls that ran`.
#[tokio::test]
async fn reports_script_failures_as_results_that_keep_partial_output_and_the_calls_that_ran() {
    let rig = rig(
        tools(),
        script(|_code, env| async move {
            env.tool("echo", json!({ "text": "x" })).await.unwrap();
            failed(
                ErrorKind::Script,
                "boom",
                Some("Error: boom\n    at codemode.js:3:7"),
                vec![text("partial")],
            )
        }),
    );

    let result = rig.run("text('partial'); ...").await.unwrap();

    assert!(result.is_error);
    let Some(Content::Text { text: header, .. }) = result.content.first() else {
        panic!();
    };
    assert!(header.starts_with("Script failed\n"));
    let body = result_text(&result);
    assert!(
        body.starts_with("partial\nScript error:\nError: boom\n"),
        "{body}"
    );
    assert!(body.contains("codemode.js:3"));
    assert!(body.contains("Tool calls made before the failure (they are not undone): echo (ok)"));
    assert_eq!(
        details(&result)
            .calls
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["echo"]
    );
}

#[test]
fn error_heads_follow_the_failure_kind() {
    let error = |kind, name: Option<&str>, stack: Option<&str>| CodemodeError {
        kind,
        name: name.map(str::to_owned),
        message: "msg".to_owned(),
        stack: stack.map(str::to_owned),
    };
    let none: &[CodemodeNestedCall] = &[];
    assert_eq!(
        format_error(&error(ErrorKind::Script, Some("TypeError"), None), none),
        "TypeError: msg\n\nNo tool calls were made."
    );
    assert_eq!(
        format_error(&error(ErrorKind::Script, None, None), none),
        "Error: msg\n\nNo tool calls were made."
    );
    assert_eq!(
        format_error(
            &error(ErrorKind::Script, Some("X"), Some("X: msg\n  at a")),
            none
        ),
        "X: msg\n  at a\n\nNo tool calls were made."
    );
    assert_eq!(
        format_error(&error(ErrorKind::Timeout, None, None), none),
        "Script timed out: msg\n\nNo tool calls were made."
    );
    assert_eq!(
        format_error(&error(ErrorKind::Aborted, None, None), none),
        "Script aborted: msg\n\nNo tool calls were made."
    );
    assert_eq!(
        format_error(&error(ErrorKind::Sandbox, None, None), none),
        "Script sandbox failed: msg\n\nNo tool calls were made."
    );
}

#[test]
fn the_call_summary_names_every_call_with_its_status() {
    let call = |name: &str, status| CodemodeNestedCall {
        id: String::new(),
        name: name.to_owned(),
        args: String::new(),
        status,
        duration_ms: None,
        error: None,
        cost: None,
    };
    assert_eq!(
        format_call_summary(&[
            call("echo", CodemodeNestedCallStatus::Ok),
            call("stats", CodemodeNestedCallStatus::Error),
            call("slow", CodemodeNestedCallStatus::Cancelled),
        ]),
        "Tool calls made before the failure (they are not undone): echo (ok), stats (error), slow (cancelled)"
    );
}

/// A script that times out, is aborted, or whose sandbox fails is a failed result like any other.
#[tokio::test]
async fn timeouts_aborts_and_sandbox_failures_are_failed_results() {
    for (kind, expected) in [
        (
            ErrorKind::Timeout,
            "Script error:\nScript timed out: out of time\n\nNo tool calls were made.",
        ),
        (
            ErrorKind::Aborted,
            "Script error:\nScript aborted: out of time\n\nNo tool calls were made.",
        ),
        (
            ErrorKind::Sandbox,
            "Script error:\nScript sandbox failed: out of time\n\nNo tool calls were made.",
        ),
    ] {
        let rig = rig(
            Vec::new(),
            script(move |_c, _e| async move { failed(kind, "out of time", None, Vec::new()) }),
        );
        let result = rig.run("while (true) {}").await.unwrap();
        assert!(result.is_error);
        assert_eq!(result_text(&result), expected);
    }
}

/// Cancellation reaches the sandbox, whose aborted result becomes the tool's failed result.
#[tokio::test]
async fn cancelling_the_tool_call_yields_an_aborted_result() {
    let rig = rig(
        Vec::new(),
        script(|_code, env| async move {
            env.cancel.cancelled().await;
            // What a sandbox does with the reason the tool passed.
            let reason = env.cancel_reason.clone().unwrap_or_default();
            failed(ErrorKind::Aborted, &reason, None, vec![text("so far")])
        }),
    );
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        trigger.cancel();
    });
    let result = rig.run_with("await forever()", cancel).await.unwrap();
    assert!(result.is_error);
    assert_eq!(
        result_text(&result),
        "so far\nScript error:\nScript aborted: This operation was aborted\n\nNo tool calls were made."
    );
}

/// Calls still running when the script ends are cancelled in the details (`execute.ts:386-388`).
#[tokio::test]
async fn calls_still_running_when_the_script_ends_are_marked_cancelled() {
    /// A host whose nested calls never settle.
    struct Hanging(RecordingHost);
    #[async_trait::async_trait]
    impl crate::tool::CodemodeHost for Hanging {
        fn mode(&self) -> cyrup_config::CodemodeMode {
            self.0.mode()
        }
        fn inline_budget(&self) -> f64 {
            self.0.inline_budget()
        }
        fn callable_tools(&self) -> Vec<Arc<dyn Tool>> {
            self.0.callable_tools()
        }
        async fn execute_nested(
            &self,
            _caller: &ToolCallId,
            _name: &str,
            _args: Value,
            _cancel: CancelToken,
        ) -> crate::tool::NestedOutcome {
            std::future::pending().await
        }
        async fn branch_custom_entries(&self) -> Vec<crate::tool::BranchCustomEntry> {
            Vec::new()
        }
        async fn append_store_entry(
            &self,
            _data: crate::tool::CodemodeStoreEntryData,
        ) -> Result<(), crate::tool::StoreAppendFailed> {
            Ok(())
        }
        fn has_model_access(&self) -> bool {
            false
        }
        fn models(&self) -> Option<Arc<dyn crate::tool::CodemodeModels>> {
            None
        }
    }
    let slot = CodemodeHostSlot::new();
    slot.bind(Arc::new(Hanging(RecordingHost::new(tools()))));
    let factory = ScriptedSandboxFactory::new(script(|_code, env| async move {
        // Start a call and abandon it, as an unawaited promise is.
        let _ = tokio::time::timeout(Duration::from_millis(5), env.tool("echo", json!({}))).await;
        completed(Vec::new(), None)
    }));
    let tool = CodemodeTool::new(CodemodeToolOptions::new(slot, Arc::new(factory)));
    let result = tool
        .execute(
            ToolCallId::from("c"),
            json!({ "code": "x" }),
            CancelToken::new(),
            Box::new(|_| {}),
        )
        .await
        .unwrap();
    assert!(!result.is_error);
    assert_eq!(
        details(&result)
            .calls
            .iter()
            .map(|c| c.status)
            .collect::<Vec<_>>(),
        [CodemodeNestedCallStatus::Cancelled]
    );
}

// ---------------------------------------------------------------------------- output handling --

/// A returned value is appended like `text()`: strings as is, anything else as compact JSON, and
/// `undefined` adds nothing.
#[tokio::test]
async fn a_returned_value_is_appended_like_text() {
    for (value, expected) in [
        (Some(json!("plain")), "plain"),
        (Some(json!({ "a": [1, 2.5, null] })), "{\"a\":[1,2.5,null]}"),
        (Some(json!(null)), "null"),
        (Some(json!(7)), "7"),
        (None, ""),
    ] {
        let rig = rig(Vec::new(), {
            let value = value.clone();
            script(move |_c, _e| {
                let value = value.clone();
                async move { completed(Vec::new(), value) }
            })
        });
        let result = rig.run("return value").await.unwrap();
        assert_eq!(result_text(&result), expected, "{value:?}");
    }
    assert_eq!(value_text(&json!("s")), "s");
}

/// Upstream `attaches only the images the script passes to image(), in output order, each after its
/// saved path` (@v1.0.3): output items keep their order, images become image blocks, and each image
/// follows a text item naming the file it was saved to. The same image shown twice is saved once.
#[tokio::test]
async fn output_items_keep_their_order_and_each_image_follows_the_path_it_was_saved_to() {
    let shown = || OutputItem::Image {
        data: TINY_PNG.to_owned(),
        mime_type: "image/png".to_owned(),
    };
    let rig = rig(
        Vec::new(),
        script(move |_c, _e| async move {
            completed(
                vec![text("captured"), shown(), shown(), text("after")],
                None,
            )
        }),
    );
    let result = rig.run("x").await.unwrap();
    let body = result_text(&result);
    let lines: Vec<&str> = body.split('\n').collect();
    assert_eq!(
        lines,
        [
            "captured", lines[1], "<image>", lines[1], "<image>", "after"
        ],
        "both labels name one file"
    );
    assert_eq!(check_saved_images(lines[1]), "<saved>");
    assert_eq!(
        result.content[3],
        Content::Image {
            data: TINY_PNG.to_owned(),
            mime_type: "image/png".to_owned()
        }
    );
}

/// pi `saveImages` (`execute.ts` @v1.0.3): a write that fails must not discard the result of a
/// script whose tool calls already ran, so the failure becomes the label. The temp directory is
/// the one the process uses, and a stand-in that cannot be written to is a file where the
/// directory should be.
#[test]
fn a_label_says_so_when_the_image_could_not_be_saved() {
    use cyrup_codemode::output::{ImageLabelError, label_images, save_image_output};
    let blocker = tempfile::NamedTempFile::new().unwrap();
    let items = vec![OutputItem::Image {
        data: TINY_PNG.to_owned(),
        mime_type: "image/png".to_owned(),
    }];
    let labelled = label_images(items, |mime, bytes| {
        save_image_output(blocker.path(), mime, bytes)
    })
    .unwrap();
    let OutputItem::Text(label) = &labelled[0] else {
        panic!("{labelled:?}");
    };
    assert!(
        label.starts_with("[Image (image/png, 70B) could not be saved: ") && label.ends_with(']'),
        "{label}"
    );
    assert!(matches!(labelled[1], OutputItem::Image { .. }));
    let unknown = label_images(
        vec![OutputItem::Image {
            data: TINY_PNG.to_owned(),
            mime_type: "image/bmp".to_owned(),
        }],
        |_, _| panic!("a type with no extension is refused before anything is written"),
    );
    assert!(matches!(unknown, Err(ImageLabelError::NoExtension(mime)) if mime == "image/bmp"));
}

/// Upstream `truncates output to the token budget and spills the full text`.
#[tokio::test]
async fn truncates_output_to_the_token_budget_and_spills_the_full_text() {
    let rows: Vec<OutputItem> = (0..100).map(|i| text(&format!("row {i}"))).collect();
    let rig = rig(Vec::new(), {
        let rows = rows.clone();
        script(move |_c, _e| {
            let mut output = rows.clone();
            output.push(OutputItem::Image {
                data: TINY_PNG.to_owned(),
                mime_type: "image/png".to_owned(),
            });
            async move { completed(output, None) }
        })
    });

    let result = rig
        .run("// @options: {\"max_output_tokens\": 10}\nfor (...) text(...)")
        .await
        .unwrap();

    let spilled = details(&result);
    let path = spilled.full_output_path.expect("a spill file");
    let body = result_text(&result);
    assert!(body.starts_with("Warning: truncated output"), "{body}");
    assert!(body.contains("row 0\n"));
    assert!(body.contains("tokens truncated"));
    assert!(body.contains("row 99\n"));
    assert!(!body.contains("row 50\n"));
    assert!(body.contains(&format!("[Full output: {path} (read with offset/limit)]")));
    // Images follow the truncated text, each after the path it was saved to; the spill file is
    // readable only by its owner.
    assert_private_file(std::path::Path::new(&path));
    let lines: Vec<&str> = body.split('\n').collect();
    assert_eq!(check_saved_images(lines[lines.len() - 2]), "<saved>");
    assert_eq!(lines.last(), Some(&"<image>"));
    assert_eq!(
        result.content.last(),
        Some(&Content::Image {
            data: TINY_PNG.to_owned(),
            mime_type: "image/png".to_owned()
        })
    );
    let full = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        full,
        (0..100)
            .map(|i| format!("row {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    // Within budget: no spill file.
    let small = rig.run("return { ok: true }").await.unwrap();
    assert_eq!(details(&small).full_output_path, None);
}

// ------------------------------------------------------------------------------------ options --

/// Upstream `applies the timeout_ms option and rejects invalid options`: the options line sets the
/// deadline, is replaced by an empty line, and an invalid one is the tool's error text.
#[tokio::test]
async fn the_options_line_sets_the_deadline_and_invalid_options_are_the_error_text() {
    let rig = rig(
        Vec::new(),
        script(|_c, _e| async { completed(Vec::new(), None) }),
    );

    rig.run("// @options: {\"timeout_ms\": 200}\nwhile (true) {}")
        .await
        .unwrap();
    rig.run("text(1)").await.unwrap();
    let runs = rig.factory.runs();
    assert_eq!(
        runs[0].deadline,
        Deadline::After(Duration::from_millis(200))
    );
    assert_eq!(
        runs[0].code, "\nwhile (true) {}",
        "the options line becomes an empty line"
    );
    assert_eq!(runs[1].deadline, Deadline::Never, "unset by default");
    assert_eq!(runs[0].memory_limit_bytes, Some(256 * 1024 * 1024));
    assert!(
        runs.iter().all(|run| run.closed),
        "the sandbox is closed after the script"
    );

    let invalid = rig
        .run("// @options: {\"yield\": 1}\ntext(1)")
        .await
        .unwrap_err();
    assert_eq!(
        invalid.message,
        "@options only supports `max_output_tokens` and `timeout_ms`; got `yield`"
    );
    assert_eq!(
        rig.factory.runs().len(),
        2,
        "no sandbox for a source that does not parse"
    );

    let empty = rig.run("   ").await.unwrap_err();
    assert!(
        empty
            .message
            .starts_with("Expected JavaScript source text (non-empty).")
    );
}

/// "Without a session context scripts cannot call tools, `store()` starts empty, and writes are
/// dropped."
#[tokio::test]
async fn without_a_session_a_script_has_no_tools_and_an_empty_store() {
    let factory = ScriptedSandboxFactory::new(script(|_c, env| async move {
        assert!(env.store.is_empty());
        completed(vec![text("1")], None)
    }));
    let tool = CodemodeTool::new(CodemodeToolOptions::new(
        CodemodeHostSlot::new(),
        Arc::new(factory.clone()),
    ));
    let result = tool
        .execute(
            ToolCallId::from("direct"),
            json!({ "code": "return 1" }),
            CancelToken::new(),
            Box::new(|_| {}),
        )
        .await
        .unwrap();
    assert_eq!(result.content[1], Content::text("1"));
    let run = &factory.runs()[0];
    assert!(run.tool_names.is_empty());
    assert_eq!(
        run.global_names,
        ["searchTools", "describeTool", "describeNamespace"],
        "discovery globals only: no models without a host"
    );
}

/// The sandbox is configured with the callable tools (never `codemode`), the discovery globals, and
/// `models.*` only when the host has model access.
#[tokio::test]
async fn the_sandbox_gets_the_callable_tools_and_the_globals_the_host_supports() {
    let mut all = tools();
    all.push(StubTool::new("codemode", "Run JavaScript.").arc());
    let rig = rig(all, script(|_c, _e| async { completed(Vec::new(), None) }));
    rig.run("x").await.unwrap();
    let run = &rig.factory.runs()[0];
    assert_eq!(run.tool_names, ["echo", "stats"]);
    assert_eq!(
        run.global_names,
        ["searchTools", "describeTool", "describeNamespace"]
    );

    *rig.host.models.lock().unwrap() = Some(Arc::new(crate::testkit::FakeModels::new(
        Vec::new(),
        Arc::new(|_, _| Box::pin(async { panic!("not called") })),
        Arc::new(|_, _| Box::pin(async { panic!("not called") })),
    )));
    rig.run("x").await.unwrap();
    assert_eq!(
        rig.factory.runs()[1].global_names,
        [
            "searchTools",
            "describeTool",
            "describeNamespace",
            "models.getModelsOfType",
            "models.getAvailableOfType",
            "models.getModelOfType",
            "models.classify",
            "models.generateImages",
        ]
    );

    // `models: false` keeps them out even with a capable host.
    let mut off = CodemodeToolOptions::new(rig.tool_options_host(), Arc::new(rig.factory.clone()));
    off.models = false;
    let tool = CodemodeTool::new(off);
    tool.execute(
        ToolCallId::from("c"),
        json!({ "code": "x" }),
        CancelToken::new(),
        Box::new(|_| {}),
    )
    .await
    .unwrap();
    assert!(
        !rig.factory.runs()[2]
            .global_names
            .iter()
            .any(|name| name.starts_with("models.")),
        "models disabled"
    );
}

impl Rig {
    fn tool_options_host(&self) -> CodemodeHostSlot {
        let slot = CodemodeHostSlot::new();
        slot.bind(self.host.clone());
        slot
    }
}

/// A build without an engine refuses every script with a NAMED error, never a silent no-op.
#[tokio::test]
async fn an_unavailable_sandbox_is_a_named_error() {
    let slot = CodemodeHostSlot::new();
    slot.bind(Arc::new(RecordingHost::new(tools())));
    let tool = CodemodeTool::new(CodemodeToolOptions::new(
        slot,
        Arc::new(UnavailableSandboxFactory),
    ));
    let error = tool
        .execute(
            ToolCallId::from("c"),
            json!({ "code": "return 1" }),
            CancelToken::new(),
            Box::new(|_| {}),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.message,
        "codemode sandbox unavailable: this build does not link the JavaScript engine that runs scripts"
    );
}

// --------------------------------------------------------------------------------------- store --

fn writes(set: Value, delete: &[&str]) -> CodemodeStoreWrites {
    CodemodeStoreWrites {
        set: set.as_object().unwrap().clone(),
        delete: delete.iter().map(|k| (*k).to_owned()).collect(),
    }
}

/// Upstream `persists store() writes as custom entries for later calls` and `appends nothing for
/// failed scripts or scripts without writes`.
#[tokio::test]
async fn successful_scripts_append_their_store_writes_and_failed_or_silent_ones_append_nothing() {
    for (result, expected) in [
        (
            CodemodeResult::Completed {
                value: None,
                output: Vec::new(),
                calls: Vec::new(),
                store_writes: writes(json!({ "count": 1 }), &[]),
            },
            vec![json!({ "set": { "count": 1 }, "delete": [] })],
        ),
        (
            CodemodeResult::Completed {
                value: None,
                output: Vec::new(),
                calls: Vec::new(),
                store_writes: writes(json!({}), &["count"]),
            },
            vec![json!({ "set": {}, "delete": ["count"] })],
        ),
        // No writes: nothing appended.
        (completed(Vec::new(), None), Vec::new()),
        // A failed execution reports no writes, and nothing is appended.
        (
            failed(ErrorKind::Script, "boom", None, Vec::new()),
            Vec::new(),
        ),
    ] {
        let rig = rig(Vec::new(), {
            script(move |_c, _e| {
                let result = result.clone();
                async move { result }
            })
        });
        rig.run("x").await.unwrap();
        let appended: Vec<Value> = rig
            .host
            .appended
            .lock()
            .unwrap()
            .iter()
            .map(|entry| serde_json::to_value(entry).unwrap())
            .collect();
        assert_eq!(appended, expected);
    }
}

/// `load()` reads the values on the branch the host reports, ignoring other entry types.
#[tokio::test]
async fn load_sees_the_branch_store_entries() {
    let rig = rig(
        Vec::new(),
        script(|_c, _e| async { completed(Vec::new(), None) }),
    );
    {
        let mut branch = rig.host.branch.lock().unwrap();
        branch.push(crate::tool::BranchCustomEntry {
            custom_type: "codemode-store".to_owned(),
            data: Some(json!({ "set": { "a": 1, "b": 2 }, "delete": [] })),
        });
        branch.push(crate::tool::BranchCustomEntry {
            custom_type: "codemode-store".to_owned(),
            data: Some(json!({ "set": { "a": 3 }, "delete": ["b"] })),
        });
        branch.push(crate::tool::BranchCustomEntry {
            custom_type: "another-extension".to_owned(),
            data: Some(json!({ "set": { "z": 9 }, "delete": [] })),
        });
    }
    rig.run("x").await.unwrap();
    assert_eq!(
        Value::Object(rig.factory.runs()[0].store.clone()),
        json!({ "a": 3 })
    );
}

/// A host that cannot record the writes fails the tool call, naming the cause (upstream's
/// `appendEntry` throwing).
#[tokio::test]
async fn a_failed_store_append_fails_the_call() {
    let rig = rig(
        Vec::new(),
        script(|_c, _e| async {
            CodemodeResult::Completed {
                value: None,
                output: Vec::new(),
                calls: Vec::new(),
                store_writes: writes(json!({ "k": 1 }), &[]),
            }
        }),
    );
    *rig.host.fail_append.lock().unwrap() = Some("session busy".to_owned());
    let error = rig.run("x").await.unwrap_err();
    assert_eq!(
        error.message,
        "Could not record the script's store() writes: session busy"
    );
}

// ------------------------------------------------------------------------------------ updates --

/// Calls are published as they start and finish, so a front end shows them while they run.
#[tokio::test]
async fn nested_calls_are_streamed_as_partial_results() {
    let rig = rig(
        tools(),
        script(|_c, env| async move {
            env.tool("echo", json!({ "text": "hi" })).await.unwrap();
            completed(Vec::new(), None)
        }),
    );
    rig.run("x").await.unwrap();
    let updates = rig.updates.lock().unwrap();
    assert_eq!(
        updates.len(),
        2,
        "one publish when the call starts, one when it ends"
    );
    assert!(updates.iter().all(|u| u.content.is_empty()));
    let statuses: Vec<(String, String)> = updates
        .iter()
        .map(|u| {
            let call = &u.details.as_ref().unwrap()["calls"][0];
            (
                call["status"].as_str().unwrap().to_owned(),
                call["args"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        statuses,
        [
            ("running".to_owned(), "{\"text\":\"hi\"}".to_owned()),
            ("ok".to_owned(), "{\"text\":\"hi\"}".to_owned())
        ]
    );
    // The id is the placeholder until the pipeline names the call.
    assert_eq!(
        updates[0].details.as_ref().unwrap()["calls"][0]["id"],
        "call-1/?"
    );
    assert_eq!(
        updates[1].details.as_ref().unwrap()["calls"][0]["id"],
        "call-1/1"
    );
}

// ----------------------------------------------------------------------------- pure helpers --

#[test]
fn arguments_are_previewed_as_truncated_compact_json() {
    assert_eq!(preview_args(None), "");
    assert_eq!(preview_args(Some(&json!({ "a": 1 }))), "{\"a\":1}");
    let long = json!({ "text": "x".repeat(300) });
    let preview = preview_args(Some(&long));
    assert_eq!(preview.chars().count(), 200);
    assert!(preview.ends_with("..."));
}

#[test]
fn text_is_truncated_by_utf16_units() {
    assert_eq!(truncate_text("short", 10), "short");
    assert_eq!(truncate_text("abcdefghij", 8), "abcde...");
    // An astral character is two UTF-16 units: eleven units, `max - 3` = seven kept, which ends in
    // the first half of a pair. `slice` keeps that lone surrogate; UTF-8 can only show U+FFFD.
    assert_eq!(truncate_text("😀😀😀😀😀a", 10), "😀😀😀\u{FFFD}...");
}

/// `toFixed(1)` rounds an exact tie up (`0.25` → `0.3`), where `{:.1}` would give `0.2`.
#[test]
fn wall_time_rounds_like_to_fixed() {
    assert_eq!(to_fixed_1(0.0), "0.0");
    assert_eq!(to_fixed_1(0.04), "0.0");
    assert_eq!(to_fixed_1(0.25), "0.3");
    assert_eq!(to_fixed_1(0.75), "0.8");
    assert_eq!(to_fixed_1(1.25), "1.3");
    assert_eq!(to_fixed_1(12.34), "12.3");
    assert_eq!(to_fixed_1(59.96), "60.0");
}

// ------------------------------------------------------------------------------------ models --

fn painter_models(
    images: usize,
    usage: Option<cyrup_core::Usage>,
) -> Arc<crate::testkit::FakeModels> {
    use cyrup_provider::{
        AnyModel, AssistantImages, ClassifierResult, ImageModel, Modality, ModelCost,
    };
    let painter = ImageModel {
        id: "painter".into(),
        name: "Painter".into(),
        api: "test-images".into(),
        provider: "scorer".into(),
        base_url: "https://images.test/v1".into(),
        input: vec![Modality::Text],
        output: vec![Modality::Image],
        cost: ModelCost::default(),
        headers: None,
    };
    Arc::new(crate::testkit::FakeModels::new(
        vec![AnyModel::Image(painter)],
        Arc::new(|model, _| {
            Box::pin(async move { ClassifierResult::errored(&model, "not a classifier", false) })
        }),
        Arc::new(move |model, _| {
            let usage = usage.clone();
            Box::pin(async move {
                let mut result = AssistantImages::new(&model);
                result.output = (0..images)
                    .map(|_| Content::Image {
                        data: TINY_PNG.to_owned(),
                        mime_type: "image/png".to_owned(),
                    })
                    .collect();
                result.usage = usage;
                result
            })
        }),
    ))
}

async fn generate(env: &ScriptEnv) -> Value {
    env.global(
        "models.generateImages",
        vec![
            json!({ "provider": "scorer", "id": "painter" }),
            json!({ "input": [{ "type": "text", "text": "a fox" }] }),
        ],
    )
    .await
    .unwrap()
    .unwrap()
}

/// Upstream `notes generated images that the script did not show`, and its plural.
#[tokio::test]
async fn generated_images_the_script_did_not_show_get_a_note() {
    for (images, shown, expected) in [
        (
            1,
            false,
            "stop\nNote: models.generateImages() returned 1 image that the script did not show. Show each image block of result.output with image(block).",
        ),
        (
            2,
            false,
            "stop\nNote: models.generateImages() returned 2 images that the script did not show. Show each image block of result.output with image(block).",
        ),
        // The script showed one: no note.
        (2, true, "<saved>\n<image>\nstop"),
        // Nothing generated: no note.
        (0, false, "stop"),
    ] {
        let rig = rig(
            Vec::new(),
            script(move |_c, env| async move {
                let generated = generate(&env).await;
                let output = if shown {
                    vec![OutputItem::Image {
                        data: TINY_PNG.to_owned(),
                        mime_type: "image/png".to_owned(),
                    }]
                } else {
                    Vec::new()
                };
                completed(output, Some(generated["stopReason"].clone()))
            }),
        );
        *rig.host.models.lock().unwrap() = Some(painter_models(images, None));
        let result = rig.run("x").await.unwrap();
        assert_eq!(
            check_saved_images(&result_text(&result)),
            expected,
            "{images} images, shown {shown}"
        );
    }
}

/// The usage of `models.*` calls is the tool result's usage, and each call is a row with its cost.
#[tokio::test]
async fn model_usage_becomes_the_tool_results_usage() {
    let mut usage = cyrup_core::Usage {
        input: 100,
        total_tokens: 100,
        ..cyrup_core::Usage::default()
    };
    usage.cost.total = 0.04;
    let rig = rig(
        Vec::new(),
        script(|_c, env| async move {
            generate(&env).await;
            generate(&env).await;
            completed(Vec::new(), None)
        }),
    );
    *rig.host.models.lock().unwrap() = Some(painter_models(0, Some(usage)));
    let result = rig.run("x").await.unwrap();
    let usage = result.usage.clone().expect("usage");
    assert_eq!((usage.input, usage.total_tokens), (200, 200));
    assert!((usage.cost.total - 0.08).abs() < 1e-10);
    assert_eq!(
        details(&result)
            .calls
            .iter()
            .map(|c| c.cost)
            .collect::<Vec<_>>(),
        [Some(0.04), Some(0.04)]
    );
}
