//! The script contract an agent can rely on, which upstream's sandbox does not give it: limits
//! that bound an unattended run, declarations that do not collide with the sandbox's own, errors
//! that say where they came from, values and console calls that render, and values that cannot
//! cross are refused where they are made. Every test here pins a [CYRUP-DELTA]; the cases of
//! upstream's suite are in `port`.

use std::time::{Duration, Instant};

use serde_json::json;

use super::support::*;
use crate::sandbox::{
    ARGUMENT_COMMA_WEIGHT, ARGUMENT_CONTAINER_WEIGHT, MAX_IMAGE_BYTES, MAX_JSON_DEPTH,
    MAX_OUTPUT_CHARS, MAX_PENDING_ARGUMENT_WEIGHT, MAX_PENDING_CALLS,
};
use crate::types::{CodemodeResult, Deadline, ErrorKind, ExecuteOptions, SandboxOptions};

/// Runs a script with no tools and returns the lines it printed, `text()` items and `console.*`
/// lines alike (a console line is an `OutputItem::Console`, which the tool lays out after the rest).
async fn printed(code: &str) -> Vec<String> {
    let sandbox = sandbox(Vec::new());
    let result = run(&sandbox, code).await;
    assert!(
        matches!(result, CodemodeResult::Completed { .. }),
        "{result:#?}"
    );
    output(&result)
        .iter()
        .map(|item| match item {
            cyrup_codemode::types::OutputItem::Text(text)
            | cyrup_codemode::types::OutputItem::Console(text) => text.clone(),
            other => panic!("not text: {other:?}"),
        })
        .collect()
}

/// A tool that waits `wait` on the caller's runtime before it answers.
fn slow(name: &str, wait: Duration) -> crate::types::CodemodeTool {
    tool_with(
        cyrup_codemode::types::ToolDeclaration::new(name),
        move |_, _| {
            Box::pin(async move {
                tokio::time::sleep(wait).await;
                Ok(Some(json!("slept")))
            })
        },
    )
}

// ------------------------------------------------------------------------------------------------
// Limits
// ------------------------------------------------------------------------------------------------

/// A tool whose calls never end on their own: they end when the execution cancels them.
fn hangs(name: &str) -> crate::types::CodemodeTool {
    tool_with(
        cyrup_codemode::types::ToolDeclaration::new(name),
        |_, context| {
            Box::pin(async move {
                context.cancel.cancelled().await;
                Err("cancelled".to_owned())
            })
        },
    )
}

#[tokio::test]
async fn a_loop_that_starts_calls_without_awaiting_them_is_stopped_by_the_pending_limit() {
    let sandbox = sandbox_with(SandboxOptions {
        tools: vec![hangs("hang")],
        // Far longer than the loop needs: it must be the limit that ends the script.
        deadline: Deadline::After(Duration::from_secs(20)),
        ..SandboxOptions::default()
    });
    let started = Instant::now();
    let result = run(&sandbox, "for (;;) tools.hang({});").await;
    let failure = error(&result);
    assert_eq!(failure.kind, ErrorKind::Script, "{failure:?}");
    assert_eq!(failure.name.as_deref(), Some("RangeError"), "{failure:?}");
    assert!(
        failure.message.starts_with(&format!(
            "More than {MAX_PENDING_CALLS} tool calls are in flight"
        )),
        "{failure:?}"
    );
    // The host was asked for exactly the calls the limit lets through, no more.
    assert_eq!(calls(&result).len(), MAX_PENDING_CALLS);
    assert!(started.elapsed() < Duration::from_secs(30));
}

#[tokio::test]
async fn calls_that_settled_free_their_place_under_the_pending_limit() {
    let sandbox = sandbox(vec![echo()]);
    let result = run(
        &sandbox,
        &format!(
            "const batch = (n) => Promise.all(Array.from({{ length: n }}, (_, i) => tools.echo(i)));\n\
             const full = (await batch({MAX_PENDING_CALLS})).length;\n\
             const again = (await batch(5)).length;\n\
             let refused = null;\n\
             const inFlight = [];\n\
             try {{ for (let i = 0; i <= {MAX_PENDING_CALLS}; i++) inFlight.push(tools.echo(i)); }}\n\
             catch (e) {{ refused = [e.name, inFlight.length]; }}\n\
             await Promise.all(inFlight);\n\
             return {{ full, again, refused }};"
        ),
    )
    .await;
    assert_eq!(
        value(&result).unwrap(),
        json!({ "full": MAX_PENDING_CALLS, "again": 5, "refused": ["RangeError", MAX_PENDING_CALLS] }),
        "{result:?}"
    );
}

/// A call is held by the host with its arguments until it settles, and the count limit says
/// nothing of their size: a loop of calls with 1 MiB arguments has to end at the byte limit, not
/// after thousands of them.
#[tokio::test]
async fn a_loop_of_calls_with_large_arguments_is_stopped_by_the_argument_limit() {
    let sandbox = sandbox_with(SandboxOptions {
        tools: vec![hangs("hang")],
        deadline: Deadline::After(Duration::from_secs(20)),
        ..SandboxOptions::default()
    });
    // The JSON of a string is the string and its two quotes.
    let chars = (1 << 20) + 2;
    let allowed = MAX_PENDING_ARGUMENT_WEIGHT / chars;
    let result = run(
        &sandbox,
        "const big = 'x'.repeat(1 << 20);\n\
         let started = 0;\n\
         try { for (;;) { tools.hang(big); started++; } }\n\
         catch (e) { return { name: e.name, started, message: e.message }; }",
    )
    .await;
    let outcome = value(&result).unwrap();
    assert_eq!(outcome["name"], "RangeError", "{outcome}");
    assert_eq!(outcome["started"], allowed, "{outcome}");
    assert!(
        outcome["message"]
            .as_str()
            .unwrap()
            .starts_with(&format!(
                "The calls in flight hold arguments that weigh {}, and this call's {chars} more would pass the limit of {MAX_PENDING_ARGUMENT_WEIGHT} (",
                allowed * chars
            )),
        "{outcome}"
    );
    // The host was asked for the calls that were let through and for no other.
    assert_eq!(calls(&result).len(), allowed);
}

#[tokio::test]
async fn an_unawaited_loop_of_large_calls_ends_the_script_at_the_argument_limit() {
    let sandbox = sandbox_with(SandboxOptions {
        tools: vec![hangs("hang")],
        deadline: Deadline::After(Duration::from_secs(20)),
        ..SandboxOptions::default()
    });
    let started = Instant::now();
    let result = run(
        &sandbox,
        "const big = 'x'.repeat(1 << 20); for (;;) tools.hang(big);",
    )
    .await;
    let failure = error(&result);
    assert_eq!(failure.kind, ErrorKind::Script, "{failure:?}");
    assert_eq!(failure.name.as_deref(), Some("RangeError"), "{failure:?}");
    assert!(
        failure.message.contains("hold arguments that weigh"),
        "{failure:?}"
    );
    assert_eq!(
        calls(&result).len(),
        MAX_PENDING_ARGUMENT_WEIGHT / ((1 << 20) + 2)
    );
    assert!(started.elapsed() < Duration::from_secs(30));
}

#[tokio::test]
async fn calls_that_settled_free_their_arguments_under_the_argument_limit() {
    let sandbox = sandbox(vec![echo()]);
    // 24 MiB in flight at a time, 96 MiB in all: the limit is on what is unsettled.
    let result = run(
        &sandbox,
        "const big = 'x'.repeat(1 << 20);\n\
         let echoed = 0;\n\
         for (let round = 0; round < 4; round++) {\n\
           const replies = await Promise.all(Array.from({ length: 24 }, () => tools.echo(big)));\n\
           echoed += replies.filter((reply) => reply.length === big.length).length;\n\
         }\n\
         return echoed;",
    )
    .await;
    assert_eq!(value(&result).unwrap(), json!(96), "{result:?}");
}

#[tokio::test]
async fn a_call_whose_arguments_alone_pass_the_limit_is_refused_and_can_be_caught() {
    let sandbox = sandbox(vec![echo()]);
    let result = run(
        &sandbox,
        &format!(
            "const refused = [];\n\
             try {{ tools.echo('x'.repeat({MAX_PENDING_ARGUMENT_WEIGHT})); }} catch (e) {{ refused.push(e.name, e.message); }}\n\
             // The refusal left nothing behind: a call that fits goes through.\n\
             refused.push(await tools.echo('fits'));\n\
             return refused;"
        ),
    )
    .await;
    let outcome = value(&result).unwrap();
    assert_eq!(outcome[0], "RangeError", "{outcome}");
    assert!(
        outcome[1].as_str().unwrap().starts_with(&format!(
            "The arguments of this call weigh {}, more than the limit of {MAX_PENDING_ARGUMENT_WEIGHT} for the calls in flight together (",
            MAX_PENDING_ARGUMENT_WEIGHT + 2
        )),
        "{outcome}"
    );
    assert_eq!(outcome[2], "fits", "{outcome}");
    assert_eq!(
        calls(&result).len(),
        1,
        "only the call that fit reached the host"
    );
}

/// The host keeps a returned value as the text the script's `JSON.stringify` wrote and prints that
/// (`ReturnValue::into_text`). It used to read the text into a tree and print the tree with the port
/// of `JSON.stringify` (`cyrup_codemode::js::json_stringify`); the two have to print the same bytes
/// for whatever a script can return, or the output changed under the model.
#[tokio::test]
async fn a_returned_value_prints_from_its_text_as_it_printed_from_a_tree() {
    let corpus = [
        // Numbers.
        "0",
        "-0",
        "100",
        "1e21",
        "1e-7",
        "123456789012345680000",
        "0.1 + 0.2",
        "2 ** 53",
        "-1.5e300",
        "5e-324",
        "1 / 3",
        "1.7976931348623157e308",
        "[NaN, Infinity, -Infinity, 1e999]",
        // Strings.
        "''",
        "'plain'",
        "'a\"b\\\\c\\n\\t\\r\\b\\f\\u0001\\u001f\\u007f'",
        "'\\u2028\\u2029'",
        "'\\u00e9\\ud83d\\ude00'",
        "'\\ud83d'",
        "'\\ud83d\\ude00'.slice(0, 1)",
        "'x'.repeat(100000)",
        // Objects and arrays.
        "[]",
        "{}",
        "null",
        "true",
        "false",
        "{ b: 1, 2: 2, 1: 3, a: 4 }",
        "{ 4294967295: 1, 4294967294: 2, '-1': 3, '01': 4, 10: 5 }",
        "{ a: { b: [1, { c: null }] } }",
        "{ '\\u0001': 1, '\\u043a\\u043b\\u044e\\u0447': 2, '': 3 }",
        "[undefined, () => 1, Symbol('s')]",
        "{ u: undefined, f() {}, n: null }",
        "new Date(0)",
        "{ toJSON() { return { x: [1] }; } }",
        "new Map([[1, { a: 2 }]])",
        "new Set([1, 'a'])",
        "10n ** 20n",
        "[new Error('boom')].map((e) => e.message)",
        "Array.from({ length: 200 }, (_, i) => ({ ['k' + i]: i / 7 }))",
        "(() => { let v = 1; for (let i = 0; i < 100; i++) v = [v]; return v; })()",
    ];
    let sandbox = sandbox(Vec::new());
    for expression in corpus {
        let result = run(&sandbox, &format!("return {expression};")).await;
        let CodemodeResult::Completed {
            value: Some(value), ..
        } = &result
        else {
            panic!("{expression}: {result:?}");
        };
        let tree = value.to_value().unwrap();
        let from_a_tree = match &tree {
            serde_json::Value::String(text) => text.clone(),
            other => cyrup_codemode::js::json_stringify(other),
        };
        assert_eq!(value.clone().into_text(), from_a_tree, "{expression}");
    }
}

/// The host holds data made of many small values at many times its text (two calls with 13 MB arrays
/// of `{ a: n }` took it to 3.2 GB), so a comma and an array or object weigh more than a character.
#[tokio::test]
async fn arguments_made_of_many_small_values_weigh_more_than_their_characters() {
    let sandbox = sandbox(vec![hangs("hang"), echo()]);
    // `[{"a":1},{"a":2}]`: 17 characters, one comma, three arrays and objects.
    let small = 17 + ARGUMENT_COMMA_WEIGHT + 3 * ARGUMENT_CONTAINER_WEIGHT;
    let result = run(
        &sandbox,
        &format!(
            "// One call that leaves room for exactly the weight of the small value.\n\
             const rest = 'x'.repeat({MAX_PENDING_ARGUMENT_WEIGHT} - 2 - {small} + 1);\n\
             tools.hang(rest);\n\
             let refused = null;\n\
             try {{ tools.hang([{{ a: 1 }}, {{ a: 2 }}]); }} catch (e) {{ refused = e.message; }}\n\
             return refused;"
        ),
    )
    .await;
    let message = value(&result).unwrap();
    assert!(
        message.as_str().unwrap().starts_with(&format!(
            "The calls in flight hold arguments that weigh {}, and this call's {small} more would pass the limit of {MAX_PENDING_ARGUMENT_WEIGHT} (",
            MAX_PENDING_ARGUMENT_WEIGHT - small + 1
        )),
        "{message}"
    );
    // One less and it fits: the weight is exactly what the message names.
    let result = run(
        &sandbox,
        &format!(
            "const rest = 'x'.repeat({MAX_PENDING_ARGUMENT_WEIGHT} - 2 - {small});\n\
             tools.hang(rest);\n\
             tools.hang([{{ a: 1 }}, {{ a: 2 }}]);\n\
             return 'fits';"
        ),
    )
    .await;
    assert_eq!(value(&result).unwrap(), json!("fits"), "{result:?}");
}

/// A few hundred thousand characters of small objects are far past the limit once weighed, where
/// the same number of characters in one string is nothing.
#[tokio::test]
async fn a_large_array_of_small_objects_is_refused_where_a_string_of_its_length_is_not() {
    let sandbox = sandbox(vec![echo()]);
    let result = run(
        &sandbox,
        "const rows = Array.from({ length: 120000 }, (_, i) => ({ a: i }));\n\
         const characters = JSON.stringify(rows).length;\n\
         const fitsAsText = (await tools.echo('x'.repeat(characters))).length;\n\
         let refused = null;\n\
         try { await tools.echo(rows); } catch (e) { refused = [e.name, e.message]; }\n\
         return { characters, fitsAsText, refused };",
    )
    .await;
    let outcome = value(&result).unwrap();
    // Well under the limit as characters, over it as weight: 120000 objects in one array have
    // 119999 commas and 120001 arrays and objects.
    let characters = outcome["characters"].as_u64().unwrap() as usize;
    assert!(characters < MAX_PENDING_ARGUMENT_WEIGHT / 16, "{outcome}");
    let weight = characters + 119_999 * ARGUMENT_COMMA_WEIGHT + 120_001 * ARGUMENT_CONTAINER_WEIGHT;
    assert!(weight > MAX_PENDING_ARGUMENT_WEIGHT, "{weight}");
    assert_eq!(outcome["fitsAsText"], outcome["characters"], "{outcome}");
    assert_eq!(outcome["refused"][0], "RangeError", "{outcome}");
    assert!(
        outcome["refused"][1]
            .as_str()
            .unwrap()
            .starts_with(&format!(
                "The arguments of this call weigh {weight}, more than"
            )),
        "{outcome}"
    );
}

#[tokio::test]
async fn a_spinning_script_stops_at_the_active_limit_long_before_the_deadline() {
    let sandbox = sandbox_with(SandboxOptions {
        deadline: Deadline::After(Duration::from_secs(60)),
        active_limit: Some(Duration::from_millis(300)),
        ..SandboxOptions::default()
    });
    let started = Instant::now();
    let result = run(&sandbox, "text('before'); while (true) {}").await;
    let failure = error(&result);
    assert_eq!(failure.kind, ErrorKind::Timeout, "{failure:?}");
    assert_eq!(
        failure.message,
        "Script ran for its limit of 300 ms of its own time (waiting for tool calls does not count)"
    );
    assert_eq!(output(&result), [text("before")]);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    wait_until_idle(&sandbox).await;
}

#[tokio::test]
async fn time_spent_waiting_for_tool_calls_does_not_count_against_the_active_limit() {
    let sandbox = sandbox_with(SandboxOptions {
        tools: vec![slow("nap", Duration::from_millis(700))],
        deadline: Deadline::After(Duration::from_secs(60)),
        active_limit: Some(Duration::from_millis(400)),
        ..SandboxOptions::default()
    });
    // Two naps are 1.4 s, more than three times the limit.
    let started = Instant::now();
    let result = run(
        &sandbox,
        "const a = await tools.nap(); const b = await tools.nap(); return [a, b]",
    )
    .await;
    assert_eq!(value(&result), Some(json!(["slept", "slept"])));
    assert!(started.elapsed() >= Duration::from_millis(1400));
    // The wait is not credited to the script, nor held against it: once awake it still has its
    // whole allowance to spin through.
    let started = Instant::now();
    let result = run(&sandbox, "await tools.nap(); while (true) {}").await;
    assert_eq!(error(&result).kind, ErrorKind::Timeout, "{result:?}");
    assert!(started.elapsed() >= Duration::from_millis(1000));
    wait_until_idle(&sandbox).await;
}

#[tokio::test]
async fn without_an_active_limit_a_busy_script_runs_until_the_deadline() {
    // Upstream's behaviour, and the sandbox's own default: the limit belongs to the tool.
    let sandbox = sandbox_with(SandboxOptions {
        deadline: Deadline::After(Duration::from_millis(900)),
        ..SandboxOptions::default()
    });
    let result = run(&sandbox, "while (true) {}").await;
    assert_eq!(
        error(&result).message,
        "Execution timed out after 900 ms",
        "{result:?}"
    );
}

#[tokio::test]
async fn a_returned_value_counts_against_the_output_limit() {
    let sandbox = sandbox(Vec::new());
    // A string of exactly the limit is MAX + 2 characters of JSON (the quotes): over.
    let result = run(&sandbox, &format!("return 'x'.repeat({MAX_OUTPUT_CHARS})")).await;
    let failure = error(&result);
    assert_eq!(failure.kind, ErrorKind::Script, "{failure:?}");
    assert_eq!(failure.name.as_deref(), Some("RangeError"), "{failure:?}");
    assert!(
        failure.message.starts_with(&format!(
            "script output exceeded the limit of {MAX_OUTPUT_CHARS} characters: the returned value is {} characters of JSON",
            MAX_OUTPUT_CHARS + 2
        )),
        "{failure:?}"
    );
    assert!(
        failure.message.contains("write large data to a file"),
        "{failure:?}"
    );

    // Structured data counts as its JSON text, which is what the host has to read.
    let result = run(
        &sandbox,
        &format!(
            "return Array.from({{ length: {} }}, (_, i) => ({{ a: i }}))",
            MAX_OUTPUT_CHARS / 8
        ),
    )
    .await;
    assert_eq!(
        error(&result).name.as_deref(),
        Some("RangeError"),
        "{:?}",
        error(&result).message
    );
}

#[tokio::test]
async fn a_returned_value_just_under_the_output_limit_is_delivered() {
    let sandbox = sandbox(Vec::new());
    let result = run(
        &sandbox,
        &format!("return 'x'.repeat({})", MAX_OUTPUT_CHARS - 2),
    )
    .await;
    let delivered = value(&result).unwrap();
    assert_eq!(delivered.as_str().map(str::len), Some(MAX_OUTPUT_CHARS - 2));
}

#[tokio::test]
async fn a_returned_value_shares_the_output_limit_with_what_the_script_printed() {
    let sandbox = sandbox(Vec::new());
    // Each half fits on its own; together the host would hold more than the limit.
    let half = MAX_OUTPUT_CHARS / 2;
    let result = run(
        &sandbox,
        &format!("text('p'.repeat({half})); return 'r'.repeat({half})"),
    )
    .await;
    let failure = error(&result);
    assert_eq!(failure.name.as_deref(), Some("RangeError"), "{failure:?}");
    assert!(
        failure.message.contains(&format!(
            "after {half} characters of text(), image(), and console output"
        )),
        "{failure:?}"
    );
    // What the script printed before the failure is still reported.
    assert_eq!(output(&result).len(), 1);

    // The same return value alone is delivered.
    let result = run(&sandbox, &format!("return 'r'.repeat({half})")).await;
    assert_eq!(value(&result).unwrap().as_str().map(str::len), Some(half));
}

// ------------------------------------------------------------------------------------------------
// Declarations and positions
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_script_may_declare_tools_and_console_itself() {
    let pick = sync_tool("pick", Ok);
    let sandbox = sandbox_with(SandboxOptions {
        tools: vec![echo()],
        globals: vec![pick],
        deadline: Deadline::After(Duration::from_secs(10)),
        ..SandboxOptions::default()
    });
    // The agent's own pattern: `const tools = await searchTools(...)`, which the wrapper's
    // parameters turned into "Identifier 'tools' has already been declared".
    let result = run(
        &sandbox,
        "const tools = await pick([1, 2]); const console = tools.map((n) => n * 2); text('t'); return [tools, console]",
    )
    .await;
    assert_eq!(value(&result), Some(json!([[1, 2], [2, 4]])), "{result:?}");
    assert_eq!(output(&result), [text("t")]);
    // `var` and a function declaration redeclare a parameter without error; they must keep working.
    let result = run(
        &sandbox,
        "var tools = 1; function console() { return 2 } return [tools, console()]",
    )
    .await;
    assert_eq!(value(&result), Some(json!([1, 2])), "{result:?}");
    // And the real ones are still there when the script does not shadow them.
    let result = run(
        &sandbox,
        "console.log(typeof tools.echo); return typeof tools",
    )
    .await;
    assert_eq!(value(&result), Some(json!("object")));
    assert_eq!(output(&result), [console("function")]);
}

#[tokio::test]
async fn positions_in_errors_are_those_of_the_script_as_written() {
    let sandbox = sandbox(Vec::new());
    // `new` is the seventh character of the first line; the wrapper's prefix is not counted.
    let result = run(&sandbox, "throw new Error('x')").await;
    let stack = error(&result).stack.clone().unwrap();
    assert!(stack.ends_with("at codemode.js:1:7"), "{stack}");
    // Later lines were never shifted.
    let result = run(&sandbox, "\n\n  throw new Error('x')").await;
    let stack = error(&result).stack.clone().unwrap();
    assert!(stack.ends_with("at codemode.js:3:9"), "{stack}");
    // A syntax error is reported at the offending token, also on the first line.
    let result = run(&sandbox, "let x = ;").await;
    let failure = error(&result);
    assert_eq!(failure.name.as_deref(), Some("SyntaxError"));
    assert!(
        failure
            .stack
            .as_deref()
            .unwrap()
            .ends_with("at codemode.js:1:9"),
        "{failure:?}"
    );
}

// ------------------------------------------------------------------------------------------------
// Errors that say where they came from
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_rejected_call_carries_the_script_frame_that_made_it() {
    let fail = sync_tool("fail", |_| Err("boom".to_owned()));
    let sandbox = sandbox(vec![fail, echo()]);
    // Under Promise.all the model sees one error; its frame must name the failing call.
    let result = run(
        &sandbox,
        "const ok = tools.echo(1);\nconst bad = tools.fail();\nawait Promise.all([ok, bad]);",
    )
    .await;
    let failure = error(&result);
    assert_eq!(failure.kind, ErrorKind::Script);
    assert_eq!(failure.message, "boom");
    let stack = failure.stack.clone().unwrap();
    assert!(
        stack.starts_with("Error: boom\n    at codemode.js:2:"),
        "{stack}"
    );
    assert!(!stack.contains("codemode-prelude"), "{stack}");

    // Each rejection has its own call's position, also when caught and inspected.
    let result = run(
        &sandbox,
        "const first = tools.fail();\nconst second = tools.fail();\n\
         const settled = await Promise.allSettled([first, second]);\n\
         return settled.map((s) => s.reason.stack.split('\\n')[1].trim())",
    )
    .await;
    let frames = value(&result).unwrap();
    let frames: Vec<&str> = frames
        .as_array()
        .unwrap()
        .iter()
        .map(|frame| frame.as_str().unwrap())
        .collect();
    assert!(frames[0].starts_with("at codemode.js:1:"), "{frames:?}");
    assert!(frames[1].starts_with("at codemode.js:2:"), "{frames:?}");
}

// ------------------------------------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------------------------------------

/// Whatever upstream renders, it renders the same: JSON values, strings and primitives go through
/// `String` or `JSON.stringify` untouched by the replacement for Errors, Maps, Sets and BigInts.
#[tokio::test]
async fn json_values_render_exactly_as_before() {
    let sandbox = sandbox(Vec::new());
    let result = run(
        &sandbox,
        r#"
        const values = [
          null, true, 0, -0, 1.5, 1e21, "plain", "quo\"te\n ",
          [], {}, [1, [2, [3]]], { a: { b: [1, "x", null] }, "k y": "v" },
          { toJSON() { return 7; } }, new Date(0), [undefined, () => 1, Symbol.iterator],
          { u: undefined, f() {} },
        ];
        for (const value of values) text(value);
        console.log(...values.slice(0, 12));
        return values;
        "#,
    )
    .await;
    let expected: Vec<String> = [
        "null",
        "true",
        "0",
        "0",
        "1.5",
        "1e+21",
        "plain",
        "quo\"te\n\u{2028}",
        "[]",
        "{}",
        "[1,[2,[3]]]",
        r#"{"a":{"b":[1,"x",null]},"k y":"v"}"#,
        "7",
        r#""1970-01-01T00:00:00.000Z""#,
        "[null,null,null]",
        "{}",
    ]
    .iter()
    .map(|text| (*text).to_owned())
    .collect();
    let texts: Vec<String> = output(&result)
        .iter()
        .take(expected.len())
        .map(|item| match item {
            cyrup_codemode::types::OutputItem::Text(text) => text.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(texts, expected);
    // console.log joins its arguments with a space, strings raw, the rest as JSON.
    let logged = match &output(&result)[expected.len()] {
        cyrup_codemode::types::OutputItem::Console(text) => text.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(logged, expected[..12].join(" "));
    // A returned JSON value is unchanged too.
    assert!(
        matches!(result, CodemodeResult::Completed { .. }),
        "{result:?}"
    );
}

#[tokio::test]
async fn errors_maps_sets_and_bigints_render_instead_of_vanishing_or_failing_the_script() {
    let lines = printed(
        r#"
        text(new RangeError("out"));
        text(new Map([["a", 1], ["b", [2]]]));
        text(new Set([1, "two"]));
        text({ nested: new Map([[1, new Set([2])]]), n: 10n });
        text(10n);
        console.log(new Error("logged"), new Set([3]), 4n);
        "#,
    )
    .await;
    assert!(
        lines[0].starts_with("RangeError: out\n    at codemode.js:2:14"),
        "{lines:?}"
    );
    assert_eq!(lines[1], r#"[["a",1],["b",[2]]]"#);
    assert_eq!(lines[2], r#"[1,"two"]"#);
    assert_eq!(lines[3], r#"{"nested":[[1,[2]]],"n":"10"}"#);
    assert_eq!(lines[4], "10");
    assert!(
        lines[5].starts_with("Error: logged\n    at codemode.js:7:"),
        "{lines:?}"
    );
    assert!(lines[5].ends_with(" [3] 4"), "{lines:?}");
}

#[tokio::test]
async fn a_returned_error_map_set_or_bigint_is_a_value_not_a_failure() {
    let sandbox = sandbox(Vec::new());
    let returned = |code: &'static str| {
        let sandbox = &sandbox;
        async move { value(&run(sandbox, code).await) }
    };
    assert_eq!(returned("return 5n").await, Some(json!("5")));
    assert_eq!(
        returned("return new Map([['a', 1]])").await,
        Some(json!([["a", 1]]))
    );
    assert_eq!(
        returned("return new Set([1, 2])").await,
        Some(json!([1, 2]))
    );
    assert_eq!(
        returned("return { big: 2n ** 70n }").await,
        Some(json!({ "big": "1180591620717411303424" }))
    );
    let error_text = returned("return new TypeError('bad')").await.unwrap();
    assert!(
        error_text
            .as_str()
            .unwrap()
            .starts_with("TypeError: bad\n    at codemode.js:1:8"),
        "{error_text}"
    );
}

#[tokio::test]
async fn the_console_methods_scripts_reach_for_exist() {
    let lines = printed(
        r#"
        console.group("outer");
        console.log("a\nb");
        console.group();
        console.info("deep");
        console.groupEnd();
        console.groupEnd();
        console.groupEnd();
        console.log("flat");
        console.assert(true, "silent");
        console.assert(false, "failed", { n: 1 });
        console.assert(0);
        console.count(); console.count("x"); console.count();
        console.countReset(); console.count();
        console.dir({ a: [1] });
        console.time("t"); console.timeLog("t", "mid"); console.timeEnd("t"); console.timeEnd("t");
        "#,
    )
    .await;
    assert_eq!(lines.len(), 14, "{lines:?}");
    assert_eq!(
        lines[..11],
        [
            "outer",
            "  a\n  b",
            "    deep",
            "flat",
            "Assertion failed: failed {\"n\":1}",
            "Assertion failed",
            "default: 1",
            "x: 1",
            "default: 2",
            "default: 1",
            "{\"a\":[1]}",
        ]
    );
    // Timers print the label and whole milliseconds; the second timeEnd warns.
    assert!(
        lines[11].starts_with("t: ") && lines[11].ends_with("ms mid"),
        "{lines:?}"
    );
    assert!(
        lines[12].starts_with("t: ") && lines[12].ends_with("ms"),
        "{lines:?}"
    );
    assert_eq!(
        lines[13],
        "Warning: No such label 't' for console.timeEnd()"
    );
}

#[tokio::test]
async fn console_table_prints_a_markdown_table() {
    let lines = printed(
        r#"
        console.table([{ a: 1, b: "x|y" }, { a: 2 }, 3]);
        console.table({ r1: { c: 1 }, r2: { c: [2] } }, ["c"]);
        console.table(new Map([["k", 1]]));
        console.table(5);
        "#,
    )
    .await;
    assert_eq!(
        lines[0],
        "| (index) | a | b | Values |\n| --- | --- | --- | --- |\n| 0 | 1 | x\\|y |  |\n| 1 | 2 |  |  |\n| 2 |  |  | 3 |"
    );
    assert_eq!(
        lines[1],
        "| (index) | c |\n| --- | --- |\n| r1 | 1 |\n| r2 | [2] |"
    );
    assert_eq!(lines[2], "| (index) | Values |\n| --- | --- |\n| k | 1 |");
    assert_eq!(lines[3], "5");
}

// ------------------------------------------------------------------------------------------------
// Values that cannot cross
// ------------------------------------------------------------------------------------------------

const DEEP: &str = "function nested(levels) { let value = 1; for (let i = 0; i < levels; i++) value = [value]; return value; }";

#[tokio::test]
async fn a_store_value_nested_too_deep_is_a_range_error_at_the_call() {
    let sandbox = sandbox(vec![echo()]);
    let result = run(
        &sandbox,
        &format!(
            "{DEEP}\nawait tools.echo('ran');\n\
             let caught;\n\
             try {{ store('deep', nested({})) }} catch (e) {{ caught = e.name + ': ' + e.message }}\n\
             store('fits', nested({MAX_JSON_DEPTH}));\n\
             store('text', '[[[[' + '{{'.repeat(500));\n\
             return caught;",
            MAX_JSON_DEPTH + 1
        ),
    )
    .await;
    // The script completes: the execution is not ended as a broken bridge after its tool call.
    let caught = value(&result).unwrap();
    assert!(
        caught.as_str().unwrap().starts_with(&format!(
            "RangeError: store(\"deep\") value is nested more than {MAX_JSON_DEPTH} levels deep"
        )),
        "{caught}"
    );
    let CodemodeResult::Completed {
        store_writes,
        calls,
        ..
    } = &result
    else {
        panic!("{result:?}");
    };
    assert_eq!(calls.len(), 1);
    assert!(store_writes.set.contains_key("fits") && store_writes.set.contains_key("text"));
    assert!(!store_writes.set.contains_key("deep"));
}

#[tokio::test]
async fn tool_arguments_nested_too_deep_reject_before_the_call_is_made() {
    let sandbox = sandbox(vec![echo()]);
    let result = run(
        &sandbox,
        &format!(
            "{DEEP}\nconst fits = await tools.echo(nested({MAX_JSON_DEPTH}));\n\
             try {{ await tools.echo(nested({})) }} catch (e) {{ return [e.name, e.message, typeof fits] }}",
            MAX_JSON_DEPTH + 1
        ),
    )
    .await;
    let caught = value(&result).unwrap();
    assert_eq!(caught[0], "RangeError", "{result:?}");
    assert!(
        caught[1].as_str().unwrap().starts_with(&format!(
            "the argument is nested more than {MAX_JSON_DEPTH} levels deep"
        )),
        "{caught}"
    );
    assert_eq!(caught[2], "object", "the call at the limit went through");
    assert_eq!(
        calls(&result).len(),
        1,
        "the refused call never reached the host"
    );
}

fn png(decoded_bytes: usize) -> String {
    // 3 bytes per 4 characters; the signature occupies the first characters.
    let groups = decoded_bytes.div_ceil(3);
    let mut data = String::from("iVBORw0KGg");
    data.push_str(&"A".repeat(groups * 4 - data.len()));
    match decoded_bytes % 3 {
        1 => {
            data.truncate(data.len() - 2);
            data.push_str("==");
        }
        2 => {
            data.truncate(data.len() - 1);
            data.push('=');
        }
        _ => {}
    }
    data
}

#[tokio::test]
async fn an_image_over_the_size_cap_is_refused_with_a_range_error() {
    let sandbox = sandbox(Vec::new());
    let at_cap = png(MAX_IMAGE_BYTES);
    let over = png(MAX_IMAGE_BYTES + 1);
    let code = format!(
        "image('data:image/png;base64,{at_cap}');\n\
         try {{ image('data:image/png;base64,{over}') }} catch (e) {{ return [e.name, e.message] }}"
    );
    let result = run(&sandbox, &code).await;
    assert_eq!(
        output(&result).len(),
        1,
        "the image at the cap is shown, the one over it is not"
    );
    let caught = value(&result).unwrap();
    assert_eq!(caught[0], "RangeError");
    let message = caught[1].as_str().unwrap();
    assert!(
        message
            .starts_with("image is 5.0 MiB, more than the limit of 5 MiB that providers accept."),
        "{message}"
    );
}

// ------------------------------------------------------------------------------------------------
// Lone surrogates
// ------------------------------------------------------------------------------------------------

/// JavaScript for a string whose last code unit is the first half of an emoji: what
/// `'ab😀cd'.slice(0, 3)` is. V8's `JSON.stringify` writes it as `\ud83d`, which `serde_json`
/// refuses.
const LONE: &str = "'ab😀cd'.slice(0, 3)";
/// What the host holds for [`LONE`].
const LONE_AS_HELD: &str = "ab\u{FFFD}";

/// A tool that records the arguments it was called with.
fn recorder(
    seen: &std::sync::Arc<std::sync::Mutex<Vec<Option<serde_json::Value>>>>,
) -> crate::types::CodemodeTool {
    let seen = std::sync::Arc::clone(seen);
    sync_tool("record", move |args| {
        seen.lock().unwrap().push(args);
        Ok(None)
    })
}

#[tokio::test]
async fn a_lone_surrogate_in_the_return_value_arrives_as_the_replacement_character() {
    let sandbox = sandbox(Vec::new());
    let result = run(
        &sandbox,
        &format!("const s = {LONE}; return [s, {{ ['k' + s]: s }}, s.length];"),
    )
    .await;
    assert_eq!(
        value(&result),
        Some(json!([
            LONE_AS_HELD,
            { "kab\u{FFFD}": LONE_AS_HELD },
            3
        ]))
    );
}

#[tokio::test]
async fn a_lone_surrogate_in_tool_arguments_reaches_the_tool_as_the_replacement_character() {
    let seen = std::sync::Arc::default();
    let sandbox = sandbox(vec![recorder(&seen)]);
    let result = run(
        &sandbox,
        &format!(
            "const s = {LONE}; await tools.record({{ text: s, ['k' + s]: [s] }}); return 'sent';"
        ),
    )
    .await;
    assert_eq!(value(&result), Some(json!("sent")), "{result:#?}");
    assert_eq!(
        *seen.lock().unwrap(),
        [Some(
            json!({ "text": LONE_AS_HELD, "kab\u{FFFD}": [LONE_AS_HELD] })
        )]
    );
    assert_eq!(call_summary(&result).len(), 1);
}

#[tokio::test]
async fn a_lone_surrogate_in_a_store_value_or_key_is_stored_and_does_not_fail_the_script() {
    let sandbox = sandbox(Vec::new());
    let result = run(
        &sandbox,
        &format!(
            "const s = {LONE}; store('k' + s, s); store('ok', 1);\n\
             return load('k' + s);"
        ),
    )
    .await;
    // The script reads back what it stored, and the host holds it under the key it will see next.
    assert_eq!(value(&result), Some(json!(LONE_AS_HELD)), "{result:#?}");
    let CodemodeResult::Completed { store_writes, .. } = &result else {
        panic!("{result:#?}");
    };
    assert_eq!(
        store_writes.set.get("kab\u{FFFD}"),
        Some(&json!(LONE_AS_HELD))
    );
    assert_eq!(
        store_writes.set.get("ok"),
        Some(&json!(1)),
        "later writes survive"
    );
}

#[tokio::test]
async fn load_finds_a_value_stored_under_a_key_that_held_a_lone_surrogate() {
    // The host holds such a key with U+FFFD in it, so that is what the next script finds.
    let sandbox = sandbox(Vec::new());
    let mut store = serde_json::Map::new();
    store.insert("kab\u{FFFD}".to_owned(), json!(7));
    let result = run_with(
        &sandbox,
        &format!("return load('k' + {LONE});"),
        ExecuteOptions {
            store,
            ..ExecuteOptions::default()
        },
    )
    .await;
    assert_eq!(value(&result), Some(json!(7)), "{result:#?}");
}

#[tokio::test]
async fn a_lone_surrogate_in_a_thrown_error_is_the_scripts_error_not_a_broken_bridge() {
    let sandbox = sandbox(Vec::new());
    let result = run(&sandbox, &format!("throw new Error({LONE} + ' bad');")).await;
    let failure = error(&result);
    assert_eq!(failure.kind, ErrorKind::Script, "{result:#?}");
    assert_eq!(failure.message, format!("{LONE_AS_HELD} bad"));
}

#[tokio::test]
async fn a_lone_surrogate_in_text_of_a_value_prints_the_same_as_in_text_of_the_string() {
    assert_eq!(
        printed(&format!(
            "const s = {LONE}; text(s); text({{ k: s }}); console.log([s]);"
        ))
        .await,
        [
            LONE_AS_HELD.to_owned(),
            format!(r#"{{"k":"{LONE_AS_HELD}"}}"#),
            format!(r#"["{LONE_AS_HELD}"]"#)
        ]
    );
}

#[tokio::test]
async fn only_a_lone_surrogate_is_replaced_the_text_of_an_escape_and_whole_emoji_are_untouched() {
    let sandbox = sandbox(Vec::new());
    // JavaScript sources: the escape's text after one backslash and after two, an emoji, and the
    // text of an escape followed by a real lone surrogate.
    let result = run(
        &sandbox,
        r"return ['\\ud83d', '\\\\ud83d', '😀', '\\ud83d' + '😀'[0], 'a\\' + '😀'[0], '😀'[1]];",
    )
    .await;
    assert_eq!(
        value(&result),
        Some(json!([
            "\\ud83d",
            "\\\\ud83d",
            "😀",
            "\\ud83d\u{FFFD}",
            "a\\\u{FFFD}",
            "\u{FFFD}"
        ])),
        "{result:#?}"
    );
}
