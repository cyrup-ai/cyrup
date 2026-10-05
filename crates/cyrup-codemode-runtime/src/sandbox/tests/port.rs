//! `packages/codemode/test/sandbox.test.ts` @v1.0.1, case by case and in its order. A case that
//! needed adapting says why in a comment headed `ADAPTED`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cyrup_codemode::types::{OutputItem, ToolDeclaration};
use serde_json::{Value, json};

use super::support::*;
use crate::sandbox::{
    CodemodeSandbox, MAX_OUTPUT_CHARS, MAX_OUTPUT_ITEMS, SandboxConfigError, isolate,
};
use crate::types::{
    CallStatus, CodemodeResult, Deadline, ErrorKind, ExecuteOptions, SandboxClosed, SandboxOptions,
};

// Base64 of the leading bytes of each format. image() only inspects the signature.
const PNG: &str = "iVBORw0KGgo=";
const JPEG: &str = "/9j/4A==";
const GIF: &str = "R0lGODlh";
const WEBP: &str = "UklGRgAAAABXRUJQ";

fn globals_of(names: &[&str]) -> Vec<crate::types::CodemodeTool> {
    names
        .iter()
        .map(|name| sync_tool(name, |_| Ok(None)))
        .collect()
}

fn new_with_globals(names: &[&str]) -> Result<CodemodeSandbox, SandboxConfigError> {
    CodemodeSandbox::new(SandboxOptions {
        globals: globals_of(names),
        ..SandboxOptions::default()
    })
}

mod embedded_sources {
    use super::*;

    #[test]
    fn parse_as_javascript() {
        // ADAPTED: `new vm.Script(PRELUDE_SOURCE)` is V8's own parser on the prelude, which is
        // what this does through the engine the sandbox embeds. It also checks that every limit
        // placeholder was substituted.
        let source = isolate::prelude_source();
        assert!(!source.contains("__MAX_"), "unsubstituted placeholder");
        assert!(!source.contains("__IMAGE_HELPER_EXPECTS__"));
        isolate::parses_as_javascript(source).unwrap();
        assert!(isolate::parses_as_javascript("(function () { return ; ; ;").is_err());
    }
}

mod script_execution {
    use super::*;

    #[tokio::test]
    async fn returns_the_scripts_return_value_after_a_json_round_trip() {
        let sandbox = sandbox(vec![]);
        let result = run(&sandbox, "return { a: 1, b: [true, 'x'] }").await;
        assert_eq!(value(&result), Some(json!({"a": 1, "b": [true, "x"]})));
        assert!(output(&result).is_empty());
        assert!(calls(&result).is_empty());
        assert_eq!(
            value(&run(&sandbox, "return 'plain'").await),
            Some(json!("plain"))
        );
        assert_eq!(value(&run(&sandbox, "").await), None);
    }

    #[tokio::test]
    async fn supports_top_level_await() {
        let sandbox = sandbox(vec![]);
        let result = run(
            &sandbox,
            "const x = await Promise.resolve(41); return x + 1",
        )
        .await;
        assert_eq!(value(&result), Some(json!(42)));
    }

    #[tokio::test]
    async fn collects_text_image_and_console_output_in_order() {
        let sandbox = sandbox(vec![]);
        let code = format!(
            r#"
            console.log("hello", 1, {{ a: 1 }});
            text({{ json: true }});
            text(undefined);
            text(7);
            image("data:image/png;base64,{PNG}");
            image({{ image_url: "data:image/jpeg;base64,{JPEG}" }});
            image({{ type: "image", data: "{GIF}", mimeType: "image/gif" }});
            image("data:image/png;base64,{WEBP}");
            image({{ type: "image", data: "{PNG}" }});
            console.error(new Error("bad"));
            return null;
        "#
        );
        let result = run(&sandbox, &code).await;
        assert_eq!(value(&result), Some(Value::Null));
        let items = output(&result);
        let (last, head) = items.split_last().unwrap();
        assert_eq!(
            head,
            [
                text(r#"hello 1 {"a":1}"#),
                text(r#"{"json":true}"#),
                text("undefined"),
                text("7"),
                image(PNG, "image/png"),
                image(JPEG, "image/jpeg"),
                image(GIF, "image/gif"),
                // The MIME type comes from the data, not from the declared type.
                image(WEBP, "image/webp"),
                image(PNG, "image/png"),
            ]
        );
        let OutputItem::Text(last) = last else {
            panic!("expected text, got {last:?}");
        };
        assert!(last.starts_with("Error: bad"), "{last}");
    }

    #[tokio::test]
    async fn rejects_invalid_text_and_image_arguments() {
        let sandbox = sandbox(vec![]);
        let result = run(
            &sandbox,
            r#"
            const errors = [];
            const circular = {};
            circular.self = circular;
            for (const run of [
                () => text(circular),
                () => image(""),
                () => image("https://example.com/a.png"),
                () => image("data:image/png,raw"),
                () => image({ type: "text", text: "x" }),
                () => image({ type: "image", data: "" }),
                () => image(42),
                () => image("data:image/png;base64,AAAA!"),
                () => image("data:image/png;base64,AAAAA"),
                () => image("data:image/png;base64,AA=A"),
                () => image("data:image/png;base64,"),
                () => image("data:image/png;base64,AAAA\n[Output truncated]"),
                () => image({ type: "image", data: "AAAA!", mimeType: "image/png" }),
                () => image("data:image/png;base64,AAAA"),
                () => image("data:image/png;base64,QUJD"),
                () => image("data:image/jpeg;base64,/9j/9w=="),
            ]) {
                try { run(); errors.push("no error"); } catch (error) { errors.push(error.name + ": " + error.message); }
            }
            return errors;
        "#,
        )
        .await;
        assert!(output(&result).is_empty());
        let Some(Value::Array(errors)) = value(&result) else {
            panic!("{result:#?}");
        };
        let errors: Vec<&str> = errors.iter().map(|e| e.as_str().unwrap()).collect();
        assert!(
            errors[0].starts_with("TypeError: ") && errors[0].contains("circular"),
            "{}",
            errors[0]
        );
        let expects = "TypeError: image expects a non-empty image URL string, an object with image_url, or a raw MCP image block";
        let bad_base64 = "TypeError: invalid image output. The image data is not valid base64 (truncated or corrupted?)";
        let not_an_image = "TypeError: invalid image output. The image data is not a PNG, JPEG, GIF, or WebP image";
        let mut expected = vec![
            expects,
            "TypeError: remote image URLs are not supported in tool outputs. Pass a base64 data URI instead",
            "TypeError: invalid image output. Pass a base64 data URI instead",
            "TypeError: image only accepts MCP image blocks, got \"text\"",
            "TypeError: image expected MCP image data",
            expects,
        ];
        expected.extend([bad_base64; 6]);
        expected.extend([not_an_image; 3]);
        assert_eq!(&errors[1..], expected.as_slice());
    }

    // https://github.com/earendil-works/pi/issues/10215
    #[tokio::test]
    async fn accepts_wrapped_and_large_base64_image_data() {
        let sandbox = sandbox(vec![]);
        let large = format!("iVBORw0KGgoA{}", "QUJD".repeat(256 * 1024));
        let code = format!(
            r#"
            image("data:image/png;base64,iVBORw0K\r\nGgo=\n");
            image("data:image/png;base64,{large}");
        "#
        );
        let result = run(&sandbox, &code).await;
        assert_eq!(value(&result), None);
        assert_eq!(
            output(&result),
            [image(PNG, "image/png"), image(&large, "image/png")]
        );
    }

    #[tokio::test]
    async fn ends_the_script_successfully_on_exit_keeping_output_and_store_writes() {
        let sandbox = sandbox(vec![echo()]);
        let result = run(
            &sandbox,
            r#"
            text("before");
            store("k", 1);
            await tools.echo(1);
            try { exit(); } catch {}
            text("after");
            return "unreachable";
        "#,
        )
        .await;
        let CodemodeResult::Completed {
            value,
            output,
            store_writes,
            ..
        } = &result
        else {
            panic!("{result:#?}");
        };
        assert_eq!(*value, None);
        assert_eq!(*output, [text("before")]);
        assert_eq!(store_writes.set.get("k"), Some(&json!(1)));
        assert!(store_writes.delete.is_empty());
    }

    #[tokio::test]
    async fn keeps_output_produced_before_a_failure() {
        let sandbox = sandbox(vec![]);
        let result = run(&sandbox, "text(\"partial\");\nthrow new Error(\"boom\")").await;
        assert_eq!(error(&result).kind, ErrorKind::Script);
        assert_eq!(output(&result), [text("partial")]);
    }

    #[tokio::test]
    async fn reports_syntax_errors_with_the_scripts_line_number() {
        let sandbox = sandbox(vec![]);
        let result = run(&sandbox, "const a = 1;\nconst b = ;\nreturn a").await;
        let error = error(&result);
        assert_eq!(error.kind, ErrorKind::Script);
        assert_eq!(error.name.as_deref(), Some("SyntaxError"));
        assert!(
            error.stack.as_deref().unwrap().contains("codemode.js:2"),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn reports_thrown_errors_with_the_scripts_line_number() {
        let sandbox = sandbox(vec![]);
        let result = run(&sandbox, "const a = 1;\nthrow new TypeError('boom ' + a)").await;
        let error = error(&result);
        assert_eq!(error.kind, ErrorKind::Script);
        assert_eq!(error.name.as_deref(), Some("TypeError"));
        assert_eq!(error.message, "boom 1");
        assert!(
            error.stack.as_deref().unwrap().contains("codemode.js:2"),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn formats_stacks_like_v8_without_prelude_frames() {
        let sandbox = sandbox(vec![]);
        let result = run(
            &sandbox,
            "console.log(new Error('inner'));\nthrow new RangeError('outer')",
        )
        .await;
        let error = error(&result);
        let stack = error.stack.as_deref().unwrap();
        // /^RangeError: outer\n {4}at .*codemode\.js:2/
        let mut lines = stack.lines();
        assert_eq!(lines.next(), Some("RangeError: outer"));
        let frame = lines.next().unwrap();
        assert!(
            frame.starts_with("    at ") && frame.contains("codemode.js:2"),
            "{stack}"
        );
        assert!(!stack.contains("codemode-prelude.js"), "{stack}");
        let OutputItem::Text(printed) = &output(&result)[0] else {
            panic!("{result:#?}");
        };
        let mut printed_lines = printed.lines();
        assert_eq!(printed_lines.next(), Some("Error: inner"));
        let frame = printed_lines.next().unwrap();
        assert!(
            frame.starts_with("    at ") && frame.contains("codemode.js:1"),
            "{printed}"
        );
        assert!(!format!("{:?}", output(&result)).contains("codemode-prelude.js"));
    }

    #[tokio::test]
    async fn reports_non_error_throws() {
        let sandbox = sandbox(vec![]);
        let result = run(&sandbox, "throw { code: 7 }").await;
        let error = error(&result);
        assert_eq!(error.kind, ErrorKind::Script);
        assert_eq!(error.message, r#"{"code":7}"#);
    }

    #[tokio::test]
    async fn reports_a_non_serializable_return_value_as_a_script_error() {
        let sandbox = sandbox(vec![]);
        let result = run(&sandbox, "return 10n").await;
        let error = error(&result);
        assert_eq!(error.kind, ErrorKind::Script);
        assert_eq!(error.name.as_deref(), Some("TypeError"));
    }
}

mod tools {
    use super::*;

    #[tokio::test]
    async fn exposes_tools_as_async_functions_and_records_calls() {
        let seen: Arc<Mutex<Vec<Option<Value>>>> = Arc::default();
        let recorder = Arc::clone(&seen);
        let sandbox = sandbox(vec![sync_tool("add", move |args| {
            recorder.lock().unwrap().push(args.clone());
            let args = args.unwrap();
            let sum = args["a"].as_i64().unwrap() + args["b"].as_i64().unwrap();
            Ok(Some(json!({ "sum": sum })))
        })]);
        let result = run(
            &sandbox,
            r#"
            const first = await tools.add({ a: 1, b: 2 });
            const second = await tools.add({ a: first.sum, b: 10 });
            return second.sum;
        "#,
        )
        .await;
        assert_eq!(value(&result), Some(json!(13)));
        assert_eq!(
            *seen.lock().unwrap(),
            [
                Some(json!({"a": 1, "b": 2})),
                Some(json!({"a": 3, "b": 10}))
            ]
        );
        assert_eq!(
            call_summary(&result),
            [
                ("add".to_owned(), CallStatus::Ok),
                ("add".to_owned(), CallStatus::Ok)
            ]
        );
        // ADAPTED: `durationMs >= 0` is trivially true of a `u64`; a call that returns at once
        // took far less than the deadline.
        assert!(calls(&result).iter().all(|call| call.duration_ms < 10_000));
    }

    #[tokio::test]
    async fn runs_concurrent_calls_and_lists_tool_names() {
        let delay = tool_with(ToolDeclaration::new("delay"), |args, _| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(args)
            })
        });
        let sandbox = sandbox(vec![echo(), delay]);
        let result = run(
            &sandbox,
            r#"
            const [a, b, c] = await Promise.all([tools.delay(1), tools.delay(2), tools.echo(3)]);
            return { values: [a, b, c], names: Object.keys(tools) };
        "#,
        )
        .await;
        assert_eq!(
            value(&result),
            Some(json!({"values": [1, 2, 3], "names": ["echo", "delay"]}))
        );
    }

    #[tokio::test]
    async fn exposes_tools_under_normalized_identifiers_and_lists_them_in_all_tools() {
        let sandbox = sandbox(vec![
            tool_with(
                ToolDeclaration::new("my-tool").with_description("Dashes"),
                |_, _| Box::pin(async { Ok(Some(json!("dash"))) }),
            ),
            tool_with(
                ToolDeclaration::new("my_tool").with_description("Shadowed"),
                |_, _| Box::pin(async { Ok(Some(json!("underscore"))) }),
            ),
            sync_tool("mcp__docs__search", |_| Ok(Some(json!("mcp")))),
        ]);
        let result = run(
            &sandbox,
            r#"
            try { ALL_TOOLS.push({}); } catch {}
            return {
                all: ALL_TOOLS,
                calls: [await tools.my_tool(), await tools["my-tool"](), await tools.mcp__docs__search()],
            };
        "#,
        )
        .await;
        assert_eq!(
            value(&result),
            Some(json!({
                "all": [
                    {"name": "my_tool", "description": "Dashes"},
                    {"name": "mcp__docs__search", "description": ""},
                ],
                "calls": ["dash", "dash", "mcp"],
            }))
        );
    }

    #[tokio::test]
    async fn passes_undefined_arguments_and_results_through() {
        // ADAPTED: besides the script's view, the host's: an `undefined` argument is `None`, not
        // JSON `null` (types.rs `ToolCallback`).
        let seen: Arc<Mutex<Vec<Option<Value>>>> = Arc::default();
        let recorder = Arc::clone(&seen);
        let sandbox = sandbox(vec![sync_tool("noop", move |args| {
            recorder.lock().unwrap().push(args.clone());
            Ok(args)
        })]);
        let result = run(
            &sandbox,
            "return [await tools.noop(), await tools.noop(null)]",
        )
        .await;
        assert_eq!(value(&result), Some(json!([null, null])));
        assert_eq!(*seen.lock().unwrap(), [None, Some(Value::Null)]);
    }

    #[tokio::test]
    async fn turns_tool_errors_into_catchable_errors_in_the_script() {
        let sandbox = sandbox(vec![sync_tool("fail", |_| Err("tool exploded".to_owned()))]);
        let result = run(
            &sandbox,
            r#"
            try {
                await tools.fail();
                return "no error";
            } catch (error) {
                return { isError: error instanceof Error, message: error.message };
            }
        "#,
        )
        .await;
        assert_eq!(
            value(&result),
            Some(json!({"isError": true, "message": "tool exploded"}))
        );
        assert_eq!(
            call_summary(&result),
            [("fail".to_owned(), CallStatus::Error)]
        );
    }

    #[tokio::test]
    async fn rejects_calls_to_unknown_tools() {
        let sandbox = sandbox(vec![]);
        let result = run(&sandbox, "return await tools.missing()").await;
        let error = error(&result);
        assert_eq!(error.kind, ErrorKind::Script);
        assert_eq!(error.name.as_deref(), Some("TypeError"));
    }

    #[tokio::test]
    async fn aborts_unawaited_calls_when_the_script_returns() {
        // ADAPTED: upstream's `abort` listener runs inside `controller.abort()`, before
        // `execute` resolves. A Rust callback observes its token when its task is next polled, so
        // the test waits for that (bounded) instead of reading a flag at once.
        let (observed_tx, observed_rx) = tokio::sync::oneshot::channel::<()>();
        let observed_tx = Arc::new(Mutex::new(Some(observed_tx)));
        let slow = tool_with(ToolDeclaration::new("slow"), move |_, ctx| {
            let observed_tx = Arc::clone(&observed_tx);
            Box::pin(async move {
                ctx.cancel.cancelled().await;
                if let Some(tx) = observed_tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                Err("aborted".to_owned())
            })
        });
        let sandbox = sandbox(vec![slow]);
        let result = run(&sandbox, "tools.slow(); return 'early'").await;
        assert_eq!(value(&result), Some(json!("early")));
        assert_eq!(
            call_summary(&result),
            [("slow".to_owned(), CallStatus::Cancelled)]
        );
        tokio::time::timeout(Duration::from_secs(5), observed_rx)
            .await
            .expect("the tool never saw its token fire")
            .unwrap();
    }

    #[tokio::test]
    async fn names_close_matches_when_a_script_reads_a_tool_that_does_not_exist() {
        let sandbox = sandbox(vec![
            echo(),
            sync_tool("web-search", |_| Ok(Some(json!("")))),
        ]);
        let attempt = |expression: &'static str| {
            let sandbox = &sandbox;
            async move {
                let result = run(sandbox, &format!("return {expression};")).await;
                match &result {
                    CodemodeResult::Completed { value, .. } => value.clone().unwrap(),
                    CodemodeResult::Failed { error, .. } => json!(error.message),
                }
            }
        };
        assert_eq!(
            attempt("tools.Echo").await,
            json!(
                "tools.Echo does not exist. Did you mean tools.echo? ALL_TOOLS lists every tool; searchTools(query) finds tools by topic. Check for a member with \"Echo\" in tools."
            )
        );
        assert!(
            attempt("tools.websearch")
                .await
                .as_str()
                .unwrap()
                .contains("Did you mean tools.web_search?")
        );
        assert!(
            attempt("tools.nothing")
                .await
                .as_str()
                .unwrap()
                .contains("Available: echo, web_search.")
        );
        assert_eq!(
            attempt("['echo' in tools, 'nothing' in tools, String(tools.toString), JSON.stringify(tools)]")
                .await,
            json!([true, false, "undefined", "{}"])
        );
    }

    #[tokio::test]
    async fn supports_register_and_unregister_between_executions() {
        let sandbox = sandbox(vec![]);
        sandbox.register_tool(echo()).unwrap();
        let again = sandbox.register_tool(echo()).unwrap_err();
        assert!(again.to_string().contains("already registered"), "{again}");
        let names: Vec<_> = sandbox
            .tools()
            .iter()
            .map(|tool| tool.declaration.name.clone())
            .collect();
        assert_eq!(names, ["echo"]);
        assert_eq!(
            value(&run(&sandbox, "return await tools.echo('a')").await),
            Some(json!("a"))
        );
        assert!(sandbox.unregister_tool("echo"));
        assert_eq!(
            value(&run(&sandbox, "return 'echo' in tools").await),
            Some(json!(false))
        );
    }
}

mod store_and_load {
    use super::*;

    #[tokio::test]
    async fn reads_the_snapshot_and_reports_writes() {
        let sandbox = sandbox(vec![]);
        let mut store = serde_json::Map::new();
        store.insert("counter".into(), json!(41));
        store.insert("old".into(), json!("x"));
        let result = run_with(
            &sandbox,
            r#"
            const seen = load("counter");
            store("counter", seen + 1);
            store("list", [1, { a: null }]);
            store("old", undefined);
            return [seen, load("counter"), load("missing"), load("old")];
        "#,
            ExecuteOptions {
                store,
                ..ExecuteOptions::default()
            },
        )
        .await;
        let CodemodeResult::Completed {
            value,
            store_writes,
            ..
        } = &result
        else {
            panic!("{result:#?}");
        };
        // undefined array elements become null in the JSON round trip of the return value.
        assert_eq!(*value, Some(json!([41, 42, null, null])));
        assert_eq!(store_writes.set.get("counter"), Some(&json!(42)));
        assert_eq!(store_writes.set.get("list"), Some(&json!([1, {"a": null}])));
        assert_eq!(store_writes.set.len(), 2);
        assert_eq!(store_writes.delete, ["old"]);
    }

    #[tokio::test]
    async fn returns_copies_so_mutating_a_loaded_value_does_not_change_the_store() {
        let sandbox = sandbox(vec![]);
        let mut store = serde_json::Map::new();
        store.insert("obj".into(), json!({"a": 1}));
        let result = run_with(
            &sandbox,
            r#"const value = load("obj"); value.a = 2; const kept = { b: 1 }; store("kept", kept); kept.b = 2;
            return [load("obj").a, load("kept").b];"#,
            ExecuteOptions {
                store,
                ..ExecuteOptions::default()
            },
        )
        .await;
        let CodemodeResult::Completed {
            value,
            store_writes,
            ..
        } = &result
        else {
            panic!("{result:#?}");
        };
        assert_eq!(*value, Some(json!([1, 1])));
        assert_eq!(store_writes.set.get("kept"), Some(&json!({"b": 1})));
    }

    #[tokio::test]
    async fn rejects_invalid_keys_values_and_oversized_writes_inside_the_script() {
        let sandbox = sandbox(vec![]);
        let result = run(
            &sandbox,
            r#"
            const attempt = (fn) => { try { fn(); return "ok"; } catch (error) { return error.name; } };
            return [
                attempt(() => store(1, "x")),
                attempt(() => load({})),
                attempt(() => store("fn", () => 1)),
                attempt(() => store("big", "x".repeat(300 * 1024))),
                attempt(() => { for (let i = 0; i < 8; i++) store("k" + i, "x".repeat(200 * 1024)); }),
            ];
        "#,
        )
        .await;
        assert_eq!(
            value(&result),
            Some(json!([
                "TypeError",
                "TypeError",
                "TypeError",
                "RangeError",
                "RangeError"
            ]))
        );
    }

    #[tokio::test]
    async fn explains_oversized_writes() {
        let sandbox = sandbox(vec![]);
        let result = run(&sandbox, r#"store("img", "x".repeat(300 * 1024));"#).await;
        let message = &error(&result).message;
        assert!(
            message.contains(r#"store("img") value has 307202 characters of JSON"#),
            "{message}"
        );
        assert!(message.contains("Show images with image()"), "{message}");
    }

    #[test]
    fn reserves_the_store_and_load_names() {
        for name in ["store", "load"] {
            let error = new_with_globals(&[name]).unwrap_err();
            assert!(error.to_string().contains("Invalid global"), "{error}");
        }
    }
}

mod globals {
    use super::*;

    #[tokio::test]
    async fn exposes_globals_as_top_level_functions_without_recording_them_as_calls() {
        let seen: Arc<Mutex<Vec<Option<Value>>>> = Arc::default();
        let recorder = Arc::clone(&seen);
        let sandbox = sandbox_with(SandboxOptions {
            tools: vec![echo()],
            globals: vec![sync_tool("attach", move |args| {
                recorder.lock().unwrap().push(args);
                Ok(None)
            })],
            ..SandboxOptions::default()
        });
        let result = run(
            &sandbox,
            r#"
            await attach({ ref: 1 });
            attach("not awaited");
            return [typeof attach, typeof globalThis.attach, await tools.echo(2)];
        "#,
        )
        .await;
        assert_eq!(value(&result), Some(json!(["function", "function", 2])));
        let names: Vec<_> = calls(&result)
            .iter()
            .map(|call| call.name.as_str())
            .collect();
        assert_eq!(names, ["echo"]);
        // Messages are handled in order, so an unawaited global still runs before the script settles.
        assert_eq!(
            *seen.lock().unwrap(),
            [Some(json!({"ref": 1})), Some(json!("not awaited"))]
        );
    }

    #[tokio::test]
    async fn groups_namespaced_globals_and_spreads_arguments_on_request() {
        let seen: Arc<Mutex<Vec<Option<Value>>>> = Arc::default();
        let recorder = Arc::clone(&seen);
        let list = tool_with(
            ToolDeclaration {
                spread: true,
                ..ToolDeclaration::new("models.list")
            },
            move |args, _| {
                recorder.lock().unwrap().push(args);
                Box::pin(async { Ok(None) })
            },
        );
        let sandbox = sandbox_with(SandboxOptions {
            globals: vec![list, sync_tool("models.first", Ok)],
            ..SandboxOptions::default()
        });
        let result = run(
            &sandbox,
            r#"
            await models.list("classifier", undefined, 3);
            await models.list();
            try { models.extra = 1; } catch {}
            return [Object.keys(models), await models.first("a", "ignored"), "extra" in models];
        "#,
        )
        .await;
        assert_eq!(value(&result), Some(json!([["list", "first"], "a", false])));
        // undefined array elements become null in the JSON round trip.
        assert_eq!(
            *seen.lock().unwrap(),
            [Some(json!(["classifier", null, 3])), Some(json!([]))]
        );
    }

    #[tokio::test]
    async fn names_the_members_of_a_namespace_when_a_script_reads_one_that_does_not_exist() {
        let sandbox = new_with_globals(&["models.classify", "models.generateImages"]).unwrap();
        let result = run(&sandbox, "await models.generateImage();").await;
        assert_eq!(
            error(&result).message,
            "models.generateImage does not exist. Did you mean models.generateImages? Check for a member with \"generateImage\" in models."
        );
    }

    #[test]
    fn rejects_invalid_and_reserved_global_names() {
        for name in ["a.b.c", "a.", ".a", "tools.x", "store.x", "a.not-valid"] {
            let error = new_with_globals(&[name]).unwrap_err();
            assert!(
                error.to_string().contains("Invalid global"),
                "{name}: {error}"
            );
        }
        let conflict = new_with_globals(&["models", "models.list"]).unwrap_err();
        assert!(
            conflict
                .to_string()
                .contains("conflicts with the namespace"),
            "{conflict}"
        );
        for name in ["not-valid", "tools", "console"] {
            let error = new_with_globals(&[name]).unwrap_err();
            assert!(
                error.to_string().contains("Invalid global"),
                "{name}: {error}"
            );
        }
        // Upstream's other two refusals have messages the suite does not pin; they are upstream's.
        assert_eq!(
            new_with_globals(&["a", "a"]).unwrap_err().to_string(),
            "Global \"a\" is already registered"
        );
    }
}

mod limits_and_lifetime {
    use super::*;

    #[tokio::test]
    async fn terminates_a_synchronous_infinite_loop_on_timeout() {
        let sandbox = sandbox(vec![]);
        let started = Instant::now();
        let result = run_with(&sandbox, "while (true) {}", deadline_ms(200)).await;
        assert_eq!(error(&result).kind, ErrorKind::Timeout);
        assert_eq!(error(&result).message, "Execution timed out after 200 ms");
        assert!(started.elapsed() < Duration::from_secs(5));
        // The thread is gone, not spinning on: nothing of the execution is left alive.
        wait_until_idle(&sandbox).await;
    }

    #[tokio::test]
    async fn runs_without_a_deadline_when_the_deadline_is_never() {
        let wait = tool_with(ToolDeclaration::new("wait"), |_, _| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(400)).await;
                Ok(Some(json!("late")))
            })
        });
        let sandbox = sandbox(vec![wait]);
        let result = run_with(
            &sandbox,
            "return await tools.wait()",
            ExecuteOptions {
                deadline: Some(Deadline::Never),
                ..ExecuteOptions::default()
            },
        )
        .await;
        assert_eq!(value(&result), Some(json!("late")));
    }

    #[tokio::test]
    async fn fails_a_script_that_waits_on_a_promise_nothing_can_settle() {
        let sandbox = sandbox(vec![echo()]);
        let never = ExecuteOptions {
            deadline: Some(Deadline::Never),
            ..ExecuteOptions::default()
        };
        let result = run_with(
            &sandbox,
            "await tools.echo(1); await new Promise(() => {}); return 'never'",
            never.clone(),
        )
        .await;
        assert_eq!(error(&result).kind, ErrorKind::Script);
        assert!(error(&result).message.contains("can never settle"));
        // Returning while a call is still pending is not a stall.
        let returned = run_with(&sandbox, "tools.echo(2); return 'early'", never).await;
        assert_eq!(value(&returned), Some(json!("early")));
    }

    #[tokio::test]
    async fn terminates_a_microtask_spinning_loop_on_timeout() {
        let sandbox = sandbox(vec![]);
        let result = run_with(&sandbox, "while (true) await null", deadline_ms(200)).await;
        assert_eq!(error(&result).kind, ErrorKind::Timeout);
        wait_until_idle(&sandbox).await;
    }

    #[tokio::test]
    async fn aborts_via_signal_and_cancels_in_flight_calls() {
        // ADAPTED: `AbortSignal` is a `CancelToken`, which carries no reason. Upstream's message
        // is the reason's message when it is an `Error` and "Execution aborted" otherwise, so the
        // `user cancelled` reason of the original case becomes that default.
        let tokens: Arc<Mutex<Vec<crate::types::CodemodeToolContext>>> = Arc::default();
        let called = Arc::new(tokio::sync::Notify::new());
        let hang = {
            let tokens = Arc::clone(&tokens);
            let called = Arc::clone(&called);
            tool_with(ToolDeclaration::new("hang"), move |_, ctx| {
                tokens.lock().unwrap().push(ctx);
                called.notify_one();
                Box::pin(std::future::pending())
            })
        };
        let sandbox = Arc::new(sandbox(vec![hang]));

        let first = {
            let sandbox = Arc::clone(&sandbox);
            tokio::spawn(async move { run(&sandbox, "await tools.hang(); return 'never'").await })
        };
        called.notified().await;

        let controller = cyrup_core::CancelToken::new();
        controller.cancel();
        assert!(!tokens.lock().unwrap()[0].cancel.is_cancelled());
        let result = run_with(
            &sandbox,
            "await tools.hang()",
            ExecuteOptions {
                cancel: Some(controller),
                ..ExecuteOptions::default()
            },
        )
        .await;
        assert_eq!(error(&result).kind, ErrorKind::Aborted);
        assert_eq!(error(&result).message, "Execution aborted");

        let second = cyrup_core::CancelToken::new();
        let pending = {
            let sandbox = Arc::clone(&sandbox);
            let second = second.clone();
            tokio::spawn(async move {
                run_with(
                    &sandbox,
                    "await tools.hang(); return 'never'",
                    ExecuteOptions {
                        cancel: Some(second),
                        ..ExecuteOptions::default()
                    },
                )
                .await
            })
        };
        called.notified().await;
        second.cancel();
        let aborted = pending.await.unwrap();
        assert_eq!(error(&aborted).kind, ErrorKind::Aborted);
        assert_eq!(
            call_summary(&aborted),
            [("hang".to_owned(), CallStatus::Cancelled)]
        );
        // The token of the cancelled execution's call fired; the first execution's did not.
        {
            let contexts = tokens.lock().unwrap();
            assert!(contexts.iter().any(|ctx| ctx.cancel.is_cancelled()));
            assert!(!contexts[0].cancel.is_cancelled());
        }

        sandbox.close().await;
        let result = first.await.unwrap();
        assert_eq!(error(&result).kind, ErrorKind::Aborted);
        assert_eq!(error(&result).message, "Sandbox closed");
    }

    // #10283: the host keeps all output, so a script that prints in a loop must not grow it without bound.
    #[tokio::test]
    async fn fails_a_script_whose_output_passes_the_limits_even_if_it_catches_the_error() {
        let sandbox = sandbox(vec![]);
        for print in [
            "text(s)",
            "console.log(s)",
            r#"image("data:image/png;base64," + p)"#,
        ] {
            let code = format!(
                r#"
                const s = "x".repeat(1 << 20);
                const p = "iVBORw0KGgoA" + "A".repeat(1 << 20);
                for (;;) {{ try {{ {print}; }} catch {{}} }}
            "#
            );
            let result = run(&sandbox, &code).await;
            let failure = error(&result);
            assert_eq!(failure.kind, ErrorKind::Script, "{print}");
            assert_eq!(failure.name.as_deref(), Some("RangeError"), "{print}");
            assert!(
                failure.message.contains("script output exceeded"),
                "{print}"
            );
            let chars: usize = output(&result)
                .iter()
                .map(|item| match item {
                    OutputItem::Text(text) => text.encode_utf16().count(),
                    OutputItem::Image { data, .. } => data.encode_utf16().count(),
                })
                .sum();
            assert!(chars <= MAX_OUTPUT_CHARS, "{print}: {chars}");
            assert!(chars > MAX_OUTPUT_CHARS - (2 << 20), "{print}: {chars}");
        }

        let empty = run(&sandbox, r#"for (;;) text("");"#).await;
        assert_eq!(error(&empty).name.as_deref(), Some("RangeError"));
        assert_eq!(output(&empty).len(), MAX_OUTPUT_ITEMS);
    }

    #[tokio::test]
    async fn rejects_execute_after_close() {
        let sandbox = sandbox(vec![]);
        sandbox.close().await;
        let refused = sandbox
            .execute("return 1", ExecuteOptions::default())
            .await
            .unwrap_err();
        assert_eq!(refused, SandboxClosed);
        assert!(refused.to_string().contains("closed"));
    }

    #[tokio::test]
    async fn runs_executions_in_parallel_without_sharing_state() {
        let sandbox = sandbox(vec![]);
        let results = futures::future::join_all([
            run(
                &sandbox,
                "globalThis.shared = 'a'; await null; return globalThis.shared",
            ),
            run(
                &sandbox,
                "globalThis.shared = 'b'; await null; return globalThis.shared",
            ),
            run(&sandbox, "return typeof globalThis.shared"),
        ])
        .await;
        let values: Vec<_> = results
            .iter()
            .map(|result| value(result).unwrap())
            .collect();
        assert_eq!(values, [json!("a"), json!("b"), json!("undefined")]);
    }

    #[tokio::test]
    async fn turns_deep_recursion_into_a_catchable_range_error() {
        let sandbox = sandbox(vec![]);
        let result = run(
            &sandbox,
            r#"
            let depth = 0;
            function dive() { depth++; dive(); }
            try { dive(); } catch (error) { return [error.name, depth > 1000]; }
        "#,
        )
        .await;
        assert_eq!(value(&result), Some(json!(["RangeError", true])));
    }

    // ADAPTED, three cases that have no counterpart: `accepts a worker path string`,
    // `reports a missing worker file as a sandbox error` and `reports a failing wasm module as a
    // sandbox error` test `workerUrl` and `wasm`, which exist only because upstream's engine is
    // QuickJS compiled to wasm and run in a Node worker. This sandbox has neither (types.rs
    // `SandboxOptions`). What those cases pin is that an engine failure outside the script is
    // `kind: "sandbox"`, not a script error: a script whose prelude cannot be installed is the
    // reachable instance.
    #[tokio::test]
    async fn reports_an_engine_failure_as_a_sandbox_error() {
        // `undefined` is a valid identifier, so it passes name validation, and the prelude cannot
        // define it on the global object.
        let sandbox = sandbox_with(SandboxOptions {
            globals: globals_of(&["undefined"]),
            ..SandboxOptions::default()
        });
        let result = run(&sandbox, "return 1").await;
        let failure = error(&result);
        assert_eq!(failure.kind, ErrorKind::Sandbox);
        assert!(
            failure.message.starts_with("Failed to set up the sandbox"),
            "{}",
            failure.message
        );
    }
}

mod escape_hatches {
    use super::*;

    #[tokio::test]
    async fn has_no_host_globals() {
        let sandbox = sandbox(vec![]);
        // ADAPTED: `std` and `os` are QuickJS's own modules; the V8 handles this adds are `Deno`,
        // `__bootstrap` and the others after them.
        let result = run(
            &sandbox,
            r#"
            return [
                typeof process, typeof require, typeof module, typeof setTimeout, typeof fetch,
                typeof WebAssembly, typeof std, typeof os, typeof globalThis.constructor,
                typeof Deno, typeof __bootstrap, typeof SharedArrayBuffer, typeof queueMicrotask,
                typeof setInterval, typeof setImmediate, typeof clearTimeout, typeof structuredClone,
                typeof window, typeof self, typeof global,
            ]
        "#,
        )
        .await;
        let mut expected = vec!["undefined"; 20];
        expected[8] = "function";
        assert_eq!(value(&result), Some(json!(expected)));
    }

    #[tokio::test]
    async fn keeps_eval_and_function_inside_the_vm() {
        // Code generation is allowed: it can only produce more code in the same isolate.
        let sandbox = sandbox(vec![echo()]);
        let result = run(
            &sandbox,
            r#"
            return [
                eval("typeof process"),
                new Function("return typeof process")(),
                tools.echo.constructor("return typeof require")(),
                (async () => {}).constructor("return typeof setTimeout")() instanceof Promise,
            ];
        "#,
        )
        .await;
        assert_eq!(
            value(&result),
            Some(json!(["undefined", "undefined", "undefined", true]))
        );
    }

    #[tokio::test]
    async fn rejects_dynamic_import() {
        let sandbox = sandbox(vec![]);
        let result = run(
            &sandbox,
            r#"
            try { await import("node:fs"); return "imported"; } catch (error) { return error.constructor.name; }
        "#,
        )
        .await;
        assert_ne!(value(&result), Some(json!("imported")));
    }

    #[tokio::test]
    async fn keeps_tools_and_console_frozen() {
        let sandbox = sandbox(vec![echo()]);
        let result = run(
            &sandbox,
            r#"
            try { tools.echo = () => 'nope'; } catch {}
            try { tools.extra = () => 'nope'; } catch {}
            try { globalThis.tools = null; } catch {}
            return ["extra" in tools, await tools.echo('still')];
        "#,
        )
        .await;
        assert_eq!(value(&result), Some(json!([false, "still"])));
    }
}
