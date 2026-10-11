//! What only this implementation has to prove: where the tool callbacks run, concurrency,
//! dropping and closing, the memory limit, the surface the script sees, and the failures an
//! isolate can have that a worker thread's script cannot.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cyrup_codemode::types::{OutputItem, ToolDeclaration};
use cyrup_core::CancelToken;
use serde_json::{Value, json};

use super::support::*;
use crate::sandbox::protocol::{MIN_MEMORY_LIMIT_BYTES, effective_memory_limit};
use crate::sandbox::{CodemodeSandbox, isolate};
use crate::types::{
    CallStatus, CodemodeResult, CodemodeToolContext, Deadline, ErrorKind, ExecuteOptions,
    SandboxClosed, SandboxOptions,
};

fn never() -> ExecuteOptions {
    ExecuteOptions {
        deadline: Some(Deadline::Never),
        ..ExecuteOptions::default()
    }
}

fn limited(bytes: u64) -> TestSandbox {
    sandbox_with(SandboxOptions {
        memory_limit_bytes: Some(bytes),
        ..SandboxOptions::default()
    })
}

/// A tool that never settles and tells the test each time it is called.
fn hang(
    tokens: &Arc<Mutex<Vec<CodemodeToolContext>>>,
    called: &Arc<tokio::sync::Notify>,
) -> crate::types::CodemodeTool {
    let tokens = Arc::clone(tokens);
    let called = Arc::clone(called);
    tool_with(ToolDeclaration::new("hang"), move |_, ctx| {
        tokens.lock().unwrap().push(ctx);
        called.notify_one();
        Box::pin(std::future::pending())
    })
}

// ------------------------------------------------------------------------------------------------
// Errors the script never looked at
// ------------------------------------------------------------------------------------------------

/// A tool that fails with `kaboom`, and one that succeeds.
fn failing_and_fine() -> Vec<crate::types::CodemodeTool> {
    vec![
        sync_tool("boom", |_| Err("kaboom".to_owned())),
        sync_tool("fine", |_| Ok(Some(json!("fine")))),
    ]
}

/// [CYRUP-DELTA] The result of a script that succeeded names the rejections nobody handled: a failed
/// call the isolate was told of while it ran, and nothing handled; and a call that failed in the host
/// before the script had returned from the line that made it, which the isolate is never told of.
#[tokio::test]
async fn a_failed_call_nobody_handled_is_reported_with_the_result() {
    let sandbox = sandbox(failing_and_fine());

    let heard = run(
        &sandbox,
        "tools.boom({});\nawait tools.fine({});\nreturn 1;",
    )
    .await;
    assert_eq!(value(&heard), Some(json!(1)));
    let (total, shown) = unobserved(&heard);
    assert_eq!(total, 1);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].0.as_deref(), Some("boom"));
    assert!(
        shown[0].1.starts_with("kaboom (codemode.js:1:"),
        "{shown:?}"
    );

    // Nothing waits after the call, so the reply is queued for an isolate that has ended.
    let unheard = run(&sandbox, "tools.boom({}); return 2;").await;
    assert_eq!(value(&unheard), Some(json!(2)));
    assert_eq!(
        unobserved(&unheard),
        (1, vec![(Some("boom".to_owned()), "kaboom".to_owned())])
    );
}

/// The rejections a script handles, however late, are not reported; a call that succeeded and was not
/// awaited has nothing to report.
#[tokio::test]
async fn rejections_the_script_handled_are_not_reported() {
    let sandbox = sandbox(failing_and_fine());
    let result = run(
        &sandbox,
        r#"
        await tools.boom({}).catch(() => text("caught"));
        const [settled] = await Promise.allSettled([tools.boom({})]);
        const kept = tools.boom({});
        tools.fine({});
        await tools.fine({});
        // The call has failed by now and the script ends in the turn that handles it.
        try { await kept; } catch (error) { text(error.message); }
        return settled.status;
    "#,
    )
    .await;
    assert_eq!(value(&result), Some(json!("rejected")));
    assert_eq!(output(&result), [text("caught"), text("kaboom")]);
    assert_eq!(unobserved(&result), (0, Vec::new()));
}

/// [CYRUP-DELTA] A call that failed after the script had ended and that something was waiting on is
/// not an error the script lost: it is named apart, as a fact. The script below ends on the first
/// failure its `Promise.all` hands it, with the other two replies still queued for an isolate that has
/// ended; `Promise.all` handles them, as it would have had the replies come in time. Naming them as
/// "never handled" told a script that awaited every call that it had not.
#[tokio::test]
async fn calls_a_promise_all_took_in_are_not_unhandled_when_they_fail_after_the_script_ends() {
    let sandbox = sandbox(failing_and_fine());
    let result = run(
        &sandbox,
        r#"
        try { await Promise.all([tools.boom({}), tools.boom({}), tools.boom({})]); }
        catch (error) { text("caught " + error.message); }
        return 1;
    "#,
    )
    .await;
    assert_eq!(value(&result), Some(json!(1)));
    assert_eq!(output(&result), [text("caught kaboom")]);
    assert_eq!(unobserved(&result), (0, Vec::new()));
    assert_eq!(
        late_failures(&result),
        (
            2,
            vec![
                (Some("boom".to_owned()), "kaboom".to_owned()),
                (Some("boom".to_owned()), "kaboom".to_owned())
            ]
        )
    );
}

/// A call with a `.catch()` on it is not an error the script lost whenever it fails, even when its
/// reply is not read before the script ends; the call beside it that nothing waits on is.
#[tokio::test]
async fn a_call_with_a_catch_is_not_unhandled_when_it_fails_after_the_script_ends() {
    let sandbox = sandbox(failing_and_fine());
    let kaboom = || (Some("boom".to_owned()), "kaboom".to_owned());

    let caught = run(&sandbox, "tools.boom({}).catch(() => {}); return 2;").await;
    assert_eq!(value(&caught), Some(json!(2)));
    assert_eq!(unobserved(&caught), (0, Vec::new()));
    assert_eq!(late_failures(&caught), (1, vec![kaboom()]));

    let beside = run(
        &sandbox,
        "tools.boom({}).catch(() => {});\ntools.boom({});\nreturn 3;",
    )
    .await;
    assert_eq!(value(&beside), Some(json!(3)));
    assert_eq!(unobserved(&beside), (1, vec![kaboom()]));
    assert_eq!(late_failures(&beside), (1, vec![kaboom()]));
}

/// [CYRUP-DELTA] The commonest forgotten `await` is not an error the script handled, though a
/// reaction is attached to the call it made: an `async` function whose `await` nobody awaited
/// (`main();`), a `forEach(async ...)` callback, a `.then(f)` with no `.catch()`. The call has a
/// handler, so it is not "unhandled", and it failed after the script ended, so the script never
/// heard. The result must still say so, or a `write` that failed reads as a write that happened.
#[tokio::test]
async fn a_call_inside_an_unawaited_async_function_is_named_when_it_fails_after_the_script_ends() {
    let sandbox = sandbox(failing_and_fine());
    let kaboom = || (Some("boom".to_owned()), "kaboom".to_owned());

    let main = run(
        &sandbox,
        "async function main() { await tools.boom({}); }\nmain();\nreturn 1;",
    )
    .await;
    assert_eq!(value(&main), Some(json!(1)));
    assert_eq!(unobserved(&main), (0, Vec::new()));
    assert_eq!(late_failures(&main), (1, vec![kaboom()]));

    let for_each = run(
        &sandbox,
        "[1, 2].forEach(async () => { await tools.boom({}); });\nreturn 2;",
    )
    .await;
    assert_eq!(value(&for_each), Some(json!(2)));
    assert_eq!(unobserved(&for_each), (0, Vec::new()));
    assert_eq!(late_failures(&for_each), (2, vec![kaboom(), kaboom()]));

    let chained = run(&sandbox, "tools.boom({}).then((x) => x);\nreturn 3;").await;
    assert_eq!(value(&chained), Some(json!(3)));
    assert_eq!(unobserved(&chained), (0, Vec::new()));
    assert_eq!(late_failures(&chained), (1, vec![kaboom()]));
}

/// A call that succeeded names nothing: only failures are late failures.
#[tokio::test]
async fn calls_that_did_not_fail_are_not_late_failures() {
    let sandbox = sandbox(failing_and_fine());
    let result = run(
        &sandbox,
        "async function main() { await tools.fine({}); }\nmain();\nreturn 4;",
    )
    .await;
    assert_eq!(value(&result), Some(json!(4)));
    assert_eq!(unobserved(&result), (0, Vec::new()));
    assert_eq!(late_failures(&result), (0, Vec::new()));
}

/// A rejection that is not a call's, raised on the last line the script runs, is reported: the
/// engine has not looked at it when the script returns, so the result waits for it.
#[tokio::test]
async fn a_rejection_on_the_last_line_is_reported() {
    let sandbox = sandbox(Vec::new());
    let result = run(&sandbox, "Promise.reject(new TypeError('late')); return 3;").await;
    assert_eq!(value(&result), Some(json!(3)));
    let (total, shown) = unobserved(&result);
    assert_eq!(total, 1);
    assert_eq!(shown[0].0, None);
    assert!(
        shown[0].1.starts_with("TypeError: late (codemode.js:1:"),
        "{shown:?}"
    );
}

/// `exit()` ends the script on the spot, with no event loop turn after it: the rejections the engine
/// has not reported yet are taken from it, and the report is made as it stands.
#[tokio::test]
async fn a_rejection_before_exit_is_reported() {
    let sandbox = sandbox(Vec::new());
    let result = run(
        &sandbox,
        "Promise.reject(new RangeError('before exit')); text('bye'); exit();",
    )
    .await;
    assert_eq!(value(&result), None);
    assert_eq!(output(&result), [text("bye")]);
    let (total, shown) = unobserved(&result);
    assert_eq!(total, 1);
    assert!(
        shown[0]
            .1
            .starts_with("RangeError: before exit (codemode.js:1:"),
        "{shown:?}"
    );
}

/// A script that returned and left a continuation that never stops is reported as it stood at the
/// return, not held for the continuation: the end of a script waits for the engine to look at what
/// the script left behind, which takes one turn of the event loop, and that turn never comes while a
/// microtask loop spins. Before the wait existed the script's result left at the return.
#[tokio::test]
async fn a_microtask_loop_left_running_does_not_hold_the_result_of_a_script_that_returned() {
    let sandbox = sandbox(Vec::new());
    let started = Instant::now();
    let result = run(
        &sandbox,
        "(async function spin() { while (true) await null; })(); text('before'); return 'returned';",
    )
    .await;
    assert_eq!(value(&result), Some(json!("returned")));
    assert_eq!(output(&result), [text("before")]);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the result waited {:?} for a loop that never ends",
        started.elapsed()
    );
}

/// What a leftover continuation of a script that has returned starts or prints is not run, as it was
/// not when the host stopped listening at the return.
#[tokio::test]
async fn a_continuation_that_outlives_the_return_runs_no_calls_and_prints_nothing() {
    let calls = Arc::new(Mutex::new(0_usize));
    let counted = Arc::clone(&calls);
    let sandbox = sandbox(vec![sync_tool("count", move |_| {
        *counted.lock().unwrap() += 1;
        Ok(None)
    })]);
    let result = run(
        &sandbox,
        r#"
        (async () => { await null; await null; text("after"); tools.count({}); })();
        text("before");
        return 4;
    "#,
    )
    .await;
    assert_eq!(value(&result), Some(json!(4)));
    assert_eq!(output(&result), [text("before")]);
    assert_eq!(call_summary(&result), []);
    assert_eq!(*calls.lock().unwrap(), 0);
}

// ------------------------------------------------------------------------------------------------
// The caller's runtime
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_tool_callback_runs_on_the_callers_runtime() {
    // The callback needs the caller's runtime for `tokio::time::sleep`; it also records where it
    // was polled. The isolate thread has a runtime of its own, which the callback must not see.
    let caller = tokio::runtime::Handle::current().id();
    type Poll = (tokio::runtime::Id, Option<String>);
    let seen: Arc<Mutex<Vec<Poll>>> = Arc::default();
    let recorder = Arc::clone(&seen);
    let probe = tool_with(ToolDeclaration::new("probe"), move |args, _| {
        let recorder = Arc::clone(&recorder);
        Box::pin(async move {
            // Created on the first poll and awaited on the later ones: both run on the caller.
            recorder.lock().unwrap().push((
                tokio::runtime::Handle::current().id(),
                std::thread::current().name().map(str::to_owned),
            ));
            tokio::time::sleep(Duration::from_millis(15)).await;
            recorder.lock().unwrap().push((
                tokio::runtime::Handle::current().id(),
                std::thread::current().name().map(str::to_owned),
            ));
            Ok(args)
        })
    });
    let sandbox = sandbox(vec![probe]);
    let result = run(&sandbox, "return await tools.probe('x')").await;
    assert_eq!(value(&result), Some(json!("x")));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    for (runtime, thread) in seen.iter() {
        assert_eq!(*runtime, caller);
        assert_ne!(thread.as_deref(), Some("codemode-isolate"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn executions_make_progress_at_the_same_time_not_one_after_another() {
    // Two executions that can only finish together: each waits in a tool for the other. Run one
    // after the other (an isolate that blocks the next from starting) they never would.
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let meet = tool_with(ToolDeclaration::new("meet"), move |_, _| {
        let barrier = Arc::clone(&barrier);
        Box::pin(async move {
            barrier.wait().await;
            Ok(Some(json!("met")))
        })
    });
    let sandbox = sandbox(vec![meet]);
    let (a, b) = tokio::join!(
        run_with(&sandbox, "return await tools.meet()", deadline_ms(8000)),
        run_with(&sandbox, "return await tools.meet()", deadline_ms(8000)),
    );
    assert_eq!(value(&a), Some(json!("met")));
    assert_eq!(value(&b), Some(json!("met")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tool_callbacks_run_in_parallel_on_a_multi_thread_caller() {
    let sandbox = sandbox(vec![tool_with(ToolDeclaration::new("nap"), |args, _| {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok(args)
        })
    })]);
    let started = Instant::now();
    let result = run(
        &sandbox,
        "return await Promise.all([1, 2, 3, 4, 5, 6].map((n) => tools.nap(n)))",
    )
    .await;
    assert_eq!(value(&result), Some(json!([1, 2, 3, 4, 5, 6])));
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_panicking_tool_is_an_error_in_the_script_not_a_crash() {
    let boom = tool_with(ToolDeclaration::new("boom"), |_, _| {
        Box::pin(async { panic!("tool bug") })
    });
    let eager = tool_with(ToolDeclaration::new("eager"), |_, _| panic!("sync bug"));
    // Panics after its first suspension, in the task the supervisor spawned for it.
    let late = tool_with(ToolDeclaration::new("late"), |_, _| {
        Box::pin(async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            panic!("late bug")
        })
    });
    let sandbox = sandbox(vec![boom, eager, late, echo()]);
    let result = run(
        &sandbox,
        r#"
        const messages = [];
        for (const name of ["boom", "eager", "late"]) {
            try { await tools[name](); } catch (error) { messages.push(error.message); }
        }
        return [messages, await tools.echo("alive")];
    "#,
    )
    .await;
    assert_eq!(
        value(&result),
        Some(json!([
            [
                "The tool callback panicked",
                "The tool callback panicked",
                "The tool callback panicked"
            ],
            "alive"
        ]))
    );
    assert_eq!(
        call_summary(&result),
        [
            ("boom".to_owned(), CallStatus::Error),
            ("eager".to_owned(), CallStatus::Error),
            ("late".to_owned(), CallStatus::Error),
            ("echo".to_owned(), CallStatus::Ok),
        ]
    );
}

#[tokio::test]
async fn large_arguments_and_results_cross_unchanged() {
    let sandbox = sandbox(vec![echo()]);
    let result = run(
        &sandbox,
        r#"
        const big = "é".repeat(2_000_000);
        const back = await tools.echo({ big, list: [1.5, null, true] });
        return [back.big === big, back.list];
    "#,
    )
    .await;
    assert_eq!(value(&result), Some(json!([true, [1.5, null, true]])));
}

#[tokio::test]
async fn an_unawaited_failing_call_does_not_fail_the_script() {
    let sandbox = sandbox(vec![sync_tool("fail", |_| Err("nope".to_owned()))]);
    let result = run(
        &sandbox,
        r#"
        tools.fail();
        Promise.reject(new Error("nobody listens"));
        for (let i = 0; i < 50000; i++) Promise.reject(i);
        await null;
        return "fine";
    "#,
    )
    .await;
    assert_eq!(value(&result), Some(json!("fine")));
}

#[tokio::test]
async fn a_tool_registered_later_does_not_reach_a_running_execution() {
    let tokens: Arc<Mutex<Vec<CodemodeToolContext>>> = Arc::default();
    let called = Arc::new(tokio::sync::Notify::new());
    let sandbox = Arc::new(sandbox(vec![hang(&tokens, &called)]));
    let running = {
        let sandbox = Arc::clone(&sandbox);
        tokio::spawn(async move { run(&sandbox, "tools.hang(); return Object.keys(tools)").await })
    };
    called.notified().await;
    sandbox.register_tool(echo()).unwrap();
    let result = running.await.unwrap();
    assert_eq!(value(&result), Some(json!(["hang"])));
}

#[tokio::test]
async fn the_deadline_option_overrides_the_sandbox_default() {
    let sandbox = sandbox_with(SandboxOptions {
        deadline: Deadline::After(Duration::from_millis(150)),
        ..SandboxOptions::default()
    });
    let started = Instant::now();
    let result = run(&sandbox, "while (true) {}").await;
    assert_eq!(error(&result).kind, ErrorKind::Timeout);
    assert_eq!(error(&result).message, "Execution timed out after 150 ms");
    assert!(started.elapsed() < Duration::from_secs(5));
    // A longer per-execution deadline wins over the shorter default.
    let long = run_with(
        &sandbox,
        "await new Promise((r) => r()); return 'done'",
        ExecuteOptions {
            deadline: Some(Deadline::After(Duration::from_secs(30))),
            ..ExecuteOptions::default()
        },
    )
    .await;
    assert_eq!(value(&long), Some(json!("done")));
}

// ------------------------------------------------------------------------------------------------
// Failure semantics
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn output_and_calls_survive_a_timeout_and_a_pending_call_is_cancelled() {
    let tokens: Arc<Mutex<Vec<CodemodeToolContext>>> = Arc::default();
    let called = Arc::new(tokio::sync::Notify::new());
    let sandbox = sandbox(vec![hang(&tokens, &called)]);
    let result = run_with(
        &sandbox,
        r#"text("before"); console.log("also"); await tools.hang(); text("after")"#,
        deadline_ms(250),
    )
    .await;
    assert_eq!(error(&result).kind, ErrorKind::Timeout);
    assert_eq!(output(&result), [text("before"), console("also")]);
    assert_eq!(
        call_summary(&result),
        [("hang".to_owned(), CallStatus::Cancelled)]
    );
    assert!(calls(&result)[0].duration_ms >= 100, "{:?}", calls(&result));
    assert!(tokens.lock().unwrap()[0].cancel.is_cancelled());
    wait_until_idle(&sandbox).await;
}

#[tokio::test]
async fn a_failed_execution_reports_no_store_writes_and_keeps_output() {
    // Only a successful execution reports writes: `Failed` has nowhere to carry them, and the
    // script's own `try`/`catch` of a thrown error does not turn a completed run into a failure.
    let sandbox = sandbox(vec![]);
    let failed = run(
        &sandbox,
        r#"store("k", 1); text("kept"); throw new Error("late")"#,
    )
    .await;
    assert!(matches!(failed, CodemodeResult::Failed { .. }));
    assert_eq!(output(&failed), [text("kept")]);
    let timed_out = run_with(
        &sandbox,
        r#"store("k", 1); while (true) {}"#,
        deadline_ms(150),
    )
    .await;
    assert_eq!(error(&timed_out).kind, ErrorKind::Timeout);
    let caught = run(
        &sandbox,
        r#"store("k", 1); try { throw new Error("x") } catch {} return 1"#,
    )
    .await;
    let CodemodeResult::Completed { store_writes, .. } = &caught else {
        panic!("{caught:#?}");
    };
    assert_eq!(store_writes.set.get("k"), Some(&json!(1)));
}

#[tokio::test]
async fn hostile_thrown_values_are_still_reported() {
    let sandbox = sandbox(vec![]);
    for code in [
        // A revoked proxy: `instanceof` on it throws.
        "const r = Proxy.revocable({}, {}); r.revoke(); throw r.proxy;",
        // A `stack` getter that throws.
        "const e = new Error('bad'); Object.defineProperty(e, 'stack', { get() { throw new Error('no stack'); } }); throw e;",
        // A stack formatter that throws. `Error` is frozen (CODE-018), so the assignment does nothing
        // and this now shows that the script's own error is the one reported.
        "Error.prepareStackTrace = () => { throw new Error('formatter'); }; throw new Error('bad');",
        // A message that is not a string.
        "throw Object.assign(new Error('x'), { name: 5, message: { not: 'a string' } });",
    ] {
        let result = run_with(&sandbox, code, deadline_ms(5000)).await;
        let failure = error(&result);
        assert_eq!(failure.kind, ErrorKind::Script, "{code}: {result:?}");
        // The script's own failure is reported, not a stall from a report that was never made.
        assert!(
            !failure.message.contains("can never settle"),
            "{code}: {result:?}"
        );
    }
}

/// `serde_json` stopped a tree at 128 levels, and a returned value used to fail past it; the host
/// keeps the text now (`ReturnValue`), as upstream's `JSON.parse` has no bound.
#[tokio::test]
async fn a_result_nested_deeper_than_serde_json_reads_is_returned_as_it_is() {
    let sandbox = sandbox(vec![]);
    let result = run(
        &sandbox,
        "let v = 1; for (let i = 0; i < 400; i++) v = [v]; return v;",
    )
    .await;
    let CodemodeResult::Completed {
        value: Some(value), ..
    } = &result
    else {
        panic!("{result:#?}");
    };
    assert_eq!(
        value.as_json(),
        format!("{}1{}", "[".repeat(400), "]".repeat(400))
    );
}

// ------------------------------------------------------------------------------------------------
// Cancellation: the caller's token, dropping the future, close()
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_token_cancelled_before_the_start_aborts_without_an_isolate() {
    let sandbox = sandbox(vec![]);
    let token = CancelToken::new();
    token.cancel();
    let started = Instant::now();
    let result = run_with(
        &sandbox,
        "while (true) {}",
        ExecuteOptions {
            cancel: Some(token),
            ..never()
        },
    )
    .await;
    assert_eq!(error(&result).kind, ErrorKind::Aborted);
    assert_eq!(sandbox.live(), 0);
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn cancelling_stops_a_spinning_script() {
    let sandbox = sandbox(vec![]);
    let token = CancelToken::new();
    let canceller = {
        let token = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            token.cancel();
        })
    };
    let result = run_with(
        &sandbox,
        "text('spinning'); while (true) {}",
        ExecuteOptions {
            cancel: Some(token),
            ..never()
        },
    )
    .await;
    canceller.await.unwrap();
    assert_eq!(error(&result).kind, ErrorKind::Aborted);
    assert_eq!(error(&result).message, "Execution aborted");
    assert_eq!(output(&result), [text("spinning")]);
    wait_until_idle(&sandbox).await;
}

/// `host.ts:114-116,230-235`: the abort's message is the signal's reason when it is an `Error`. A
/// `CancelToken` has no reason, so the caller passes it as `cancel_reason`; it applies to an abort
/// before the start and to one while the script runs.
#[tokio::test]
async fn a_cancel_reason_is_the_message_of_the_abort() {
    let sandbox = sandbox(vec![]);
    let before = CancelToken::new();
    before.cancel();
    let result = run_with(
        &sandbox,
        "while (true) {}",
        ExecuteOptions {
            cancel: Some(before),
            cancel_reason: Some("user cancelled".to_owned()),
            ..never()
        },
    )
    .await;
    assert_eq!(error(&result).message, "user cancelled");

    let token = CancelToken::new();
    let canceller = {
        let token = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            token.cancel();
        })
    };
    let result = run_with(
        &sandbox,
        "while (true) {}",
        ExecuteOptions {
            cancel: Some(token),
            cancel_reason: Some("user cancelled".to_owned()),
            ..never()
        },
    )
    .await;
    canceller.await.unwrap();
    assert_eq!(error(&result).kind, ErrorKind::Aborted);
    assert_eq!(error(&result).message, "user cancelled");
    wait_until_idle(&sandbox).await;
}

#[tokio::test]
async fn a_deadline_that_expires_while_the_isolate_is_still_being_built_stops_it_before_the_script()
{
    // The kill arrives before the isolate hands over its handle: the thread must refuse to
    // start the script rather than spin.
    let sandbox = sandbox(vec![]);
    let result = run_with(
        &sandbox,
        "while (true) {}",
        ExecuteOptions {
            deadline: Some(Deadline::After(Duration::ZERO)),
            ..ExecuteOptions::default()
        },
    )
    .await;
    assert_eq!(error(&result).kind, ErrorKind::Timeout);
    wait_until_idle(&sandbox).await;
}

#[tokio::test]
async fn dropping_the_execute_future_cancels_the_execution_and_frees_the_thread() {
    // (1) A spinning script: only `terminate_execution` can stop it.
    let sandbox = sandbox(vec![]);
    let dropped = tokio::time::timeout(
        Duration::from_millis(200),
        sandbox.execute("while (true) {}", never()),
    )
    .await;
    assert!(dropped.is_err(), "the script cannot have finished");
    wait_until_idle(&sandbox).await;

    // (2) A script waiting on a tool: the tool's token fires.
    let tokens: Arc<Mutex<Vec<CodemodeToolContext>>> = Arc::default();
    let called = Arc::new(tokio::sync::Notify::new());
    let sandbox = Arc::new(self::sandbox(vec![hang(&tokens, &called)]));
    let handle = {
        let sandbox = Arc::clone(&sandbox);
        tokio::spawn(async move { sandbox.execute("await tools.hang()", never()).await })
    };
    called.notified().await;
    assert!(!tokens.lock().unwrap()[0].cancel.is_cancelled());
    assert_eq!(sandbox.live(), 2, "one execution and its isolate thread");
    handle.abort();
    assert!(handle.await.unwrap_err().is_cancelled());
    wait_until_idle(&sandbox).await;
    assert!(tokens.lock().unwrap()[0].cancel.is_cancelled());
}

/// An isolate thread stuck in a native built-in never looks at `terminate_execution` and cannot be
/// killed. `close()` used to wait for it without end, so the `codemode` tool call that closed its
/// sandbox after the script had settled hung with it, past the script's own timeout.
#[tokio::test]
async fn close_gives_up_on_a_thread_that_cannot_be_stopped_and_returns() {
    let mut sandbox = CodemodeSandbox::new(SandboxOptions::default()).unwrap();
    sandbox.close_timeout = Duration::from_millis(300);
    // A thread inside a built-in is a guard that has not dropped.
    let stuck = sandbox.hub.running.enter();
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(20), sandbox.close())
        .await
        .expect("close() waited for a thread that never ends");
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert!(started.elapsed() < Duration::from_secs(10));
    // Detached, not gone: still counted, and the sandbox refuses new scripts.
    assert_eq!(sandbox.live(), 1);
    assert!(
        sandbox
            .execute("return 1", ExecuteOptions::default())
            .await
            .is_err()
    );
    drop(stuck);
    assert_eq!(sandbox.live(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_with_executions_in_flight_aborts_all_of_them_and_waits_for_the_threads() {
    let tokens: Arc<Mutex<Vec<CodemodeToolContext>>> = Arc::default();
    let called = Arc::new(tokio::sync::Notify::new());
    let sandbox = Arc::new(sandbox(vec![hang(&tokens, &called), echo()]));
    let spawn = |code: &'static str| {
        let sandbox = Arc::clone(&sandbox);
        tokio::spawn(async move { sandbox.execute(code, never()).await })
    };
    let spinning = spawn("text('spin'); while (true) {}");
    let microtasks = spawn("while (true) await null");
    let waiting_a = spawn("await tools.hang()");
    let waiting_b = spawn("await tools.hang()");
    called.notified().await;
    called.notified().await;
    // Let the spinning scripts start.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(sandbox.live() >= 8, "{}", sandbox.live());

    sandbox.close().await;
    assert_eq!(
        sandbox.live(),
        0,
        "close() returns once every thread has ended"
    );

    for handle in [spinning, microtasks, waiting_a, waiting_b] {
        let result = handle.await.unwrap().unwrap();
        assert_eq!(error(&result).kind, ErrorKind::Aborted);
        assert_eq!(error(&result).message, "Sandbox closed");
    }
    assert!(
        tokens
            .lock()
            .unwrap()
            .iter()
            .all(|ctx| ctx.cancel.is_cancelled())
    );
    assert_eq!(
        sandbox
            .execute("return 1", ExecuteOptions::default())
            .await
            .unwrap_err(),
        SandboxClosed
    );
    // Closing twice, and closing an idle sandbox, are fine.
    sandbox.close().await;
}

// ------------------------------------------------------------------------------------------------
// Concurrency
// ------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_concurrent_executions_on_one_sandbox_do_not_interfere() {
    let echo_slowly = tool_with(ToolDeclaration::new("echo"), |args, _| {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            Ok(args)
        })
    });
    let sandbox = Arc::new(sandbox(vec![echo_slowly]));
    let handles: Vec<_> = (0..24_u64)
        .map(|n| {
            let sandbox = Arc::clone(&sandbox);
            tokio::spawn(async move {
                let mut store = serde_json::Map::new();
                store.insert("n".into(), json!(n));
                let code = match n % 4 {
                    // Completes with its own values.
                    0 => format!(
                        r#"globalThis.mine = {n}; const echoed = await tools.echo({n});
                        text("n=" + {n}); store("seen", load("n")); return [globalThis.mine, echoed, load("n")];"#
                    ),
                    // Throws its own error.
                    1 => format!(r#"text("out{n}"); await tools.echo(1); throw new RangeError("e{n}")"#),
                    // Spins until its own deadline.
                    2 => String::from("while (true) {}"),
                    // Waits on many calls at once.
                    _ => format!(
                        "const all = await Promise.all(Array.from({{ length: 8 }}, (_, i) => tools.echo(i + {n}))); return all"
                    ),
                };
                let options = ExecuteOptions {
                    deadline: Some(Deadline::After(Duration::from_millis(if n % 4 == 2 { 300 } else { 20_000 }))),
                    store,
                    ..ExecuteOptions::default()
                };
                (n, sandbox.execute(&code, options).await.unwrap())
            })
        })
        .collect();
    for handle in handles {
        let (n, result) = handle.await.unwrap();
        match n % 4 {
            0 => {
                assert_eq!(value(&result), Some(json!([n, n, n])), "{n}");
                assert_eq!(output(&result), [text(&format!("n={n}"))]);
                let CodemodeResult::Completed { store_writes, .. } = &result else {
                    panic!("{result:#?}");
                };
                assert_eq!(store_writes.set.get("seen"), Some(&json!(n)));
                assert_eq!(calls(&result).len(), 1);
            }
            1 => {
                assert_eq!(error(&result).message, format!("e{n}"), "{n}");
                assert_eq!(output(&result), [text(&format!("out{n}"))]);
            }
            2 => assert_eq!(error(&result).kind, ErrorKind::Timeout, "{n}"),
            _ => {
                let expected: Vec<u64> = (0..8).map(|i| i + n).collect();
                assert_eq!(value(&result), Some(json!(expected)), "{n}");
                assert_eq!(calls(&result).len(), 8);
            }
        }
    }
    wait_until_idle(&sandbox).await;
}

// ------------------------------------------------------------------------------------------------
// The memory limit
// ------------------------------------------------------------------------------------------------

fn assert_out_of_memory(result: &CodemodeResult) {
    let failure = error(result);
    assert_eq!(failure.kind, ErrorKind::Script, "{result:?}");
    assert_eq!(failure.name.as_deref(), Some("InternalError"), "{result:?}");
    assert_eq!(failure.message, "out of memory", "{result:?}");
    assert!(
        failure
            .stack
            .as_deref()
            .unwrap()
            .starts_with("InternalError: out of memory"),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_runaway_heap_allocation_ends_with_upstreams_failure_and_the_process_survives() {
    let sandbox = limited(64 * 1024 * 1024);
    for code in [
        r#"const a = []; for (;;) a.push("x".repeat(1 << 20) + a.length);"#,
        r#"const a = []; for (;;) a.push(new Array(1 << 16).fill(a.length));"#,
        r#"const o = {}; for (let i = 0; ; i++) o["k" + i] = { i, s: "v" + i };"#,
    ] {
        let started = Instant::now();
        let result = run_with(&sandbox, code, deadline_ms(20_000)).await;
        assert_out_of_memory(&result);
        assert!(started.elapsed() < Duration::from_secs(15), "{code}");
    }
    wait_until_idle(&sandbox).await;
    // The sandbox, and the process, are fine afterwards.
    assert_eq!(value(&run(&sandbox, "return 1 + 1").await), Some(json!(2)));
}

#[tokio::test]
async fn a_limit_below_what_the_engine_needs_to_start_is_raised_not_fatal() {
    // V8 aborts the process when its own bootstrap does not fit the limit (measured at 4 MiB and
    // below); the sandbox raises such a limit to `MIN_MEMORY_LIMIT_BYTES`, so the isolate starts,
    // runs, and still ends a runaway allocation in an error.
    for bytes in [0_u64, 1, 4096, 1 << 20, 4 << 20] {
        let sandbox = limited(bytes);
        let fine = run(&sandbox, "return [1, 2, 3].map((x) => x * 2)").await;
        assert_eq!(value(&fine), Some(json!([2, 4, 6])), "limit {bytes}");
        let runaway = run_with(
            &sandbox,
            r#"const a = []; for (;;) a.push("x".repeat(1 << 20) + a.length);"#,
            deadline_ms(20_000),
        )
        .await;
        assert_out_of_memory(&runaway);
    }
    assert_eq!(effective_memory_limit(1), MIN_MEMORY_LIMIT_BYTES);
    assert_eq!(effective_memory_limit(256 << 20), 256 << 20);
}

#[tokio::test]
async fn heap_out_of_memory_keeps_the_output_printed_before_it() {
    let sandbox = limited(64 * 1024 * 1024);
    let result = run(
        &sandbox,
        r#"text("before"); const a = []; for (;;) a.push("x".repeat(1 << 20) + a.length);"#,
    )
    .await;
    assert_out_of_memory(&result);
    assert_eq!(output(&result), [text("before")]);
}

#[tokio::test]
async fn a_runaway_array_buffer_allocation_is_bounded_by_the_same_limit_and_is_catchable() {
    // The engine's heap limit does not count the memory behind an `ArrayBuffer` or typed array
    // (measured: 3.5 GB in three seconds before the prelude counted them). Each way of creating
    // one is bounded, and the error is the catchable `InternalError` QuickJS throws.
    let sandbox = limited(64 * 1024 * 1024);
    for (label, body) in [
        (
            "typed array",
            "const b = new Uint8Array(1 << 22); b.fill(1); a.push(b);",
        ),
        ("array buffer", "a.push(new ArrayBuffer(1 << 22));"),
        ("float array", "a.push(new Float64Array(1 << 19));"),
        (
            "from array",
            "a.push(Uint8Array.from(new Array(1 << 20).fill(7)));",
        ),
        ("slice", "a.push(seed.slice());"),
        ("map", "a.push(seed.map((x) => x));"),
        (
            "subclass",
            "a.push(new (class extends Uint8Array {})(1 << 22));",
        ),
        (
            "species constructor",
            "a.push(new seed.constructor(1 << 22));",
        ),
        (
            "prototype constructor",
            "a.push(new (Object.getPrototypeOf(seed).constructor)(1 << 22));",
        ),
        ("toSorted", "a.push(seed.toSorted());"),
        ("toReversed", "a.push(seed.toReversed());"),
        ("with", "a.push(seed.with(0, 1));"),
        ("buffer slice", "a.push(seed.buffer.slice(0));"),
        (
            "buffer transfer",
            "a.push(new ArrayBuffer(8).transfer(1 << 22));",
        ),
        (
            "resize",
            "const r = new ArrayBuffer(8, { maxByteLength: 1 << 30 }); r.resize(1 << 22); a.push(r);",
        ),
    ] {
        let code = format!(
            r#"
            const seed = new Uint8Array(1 << 22).fill(3);
            const a = [];
            try {{ for (let i = 0; i < 300; i++) {{ {body} }} return ["no error", a.length]; }} catch (error) {{ return [error.name, error.message, error instanceof Error, a.length > 0]; }}
        "#
        );
        let started = Instant::now();
        let result = run_with(&sandbox, &code, deadline_ms(20_000)).await;
        assert_eq!(
            value(&result),
            Some(json!(["InternalError", "out of memory", true, true])),
            "{label}: {result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(15), "{label}");
    }
    // Uncaught, it is the failure upstream's uncaught `InternalError` produces.
    let uncaught = run(
        &sandbox,
        "const a = []; for (let i = 0; i < 300; i++) a.push(new Uint8Array(1 << 22).fill(1)); return a.length",
    )
    .await;
    assert_out_of_memory(&uncaught);
    // It stays exceeded: another allocation of a buffer while holding them all fails again.
    let sticky = run(
        &sandbox,
        r#"
        const held = [];
        let failures = 0;
        for (let i = 0; i < 40; i++) {
            try { held.push(new Uint8Array(1 << 22).fill(1)); } catch { failures++; }
        }
        return [held.length < 40, failures > 0];
    "#,
    )
    .await;
    assert_eq!(value(&sticky), Some(json!([true, true])));
    wait_until_idle(&sandbox).await;
}

/// The measured species bypass: with `constructor` set to `undefined` on one buffer, `slice`,
/// `map` and `filter` resolved the engine's intrinsic constructor, which the prelude does not
/// guard, and a loop of `seed.slice()` grew the process to 943 MB under a 256 MiB limit with no
/// error (3.6 GB when the harness let it run). The methods now report what they return, whatever
/// constructor made it.
#[tokio::test]
async fn a_buffer_whose_constructor_is_gone_cannot_be_copied_past_the_limit() {
    let sandbox = limited(64 * 1024 * 1024);
    let has_base64 = value(
        &run(
            &sandbox,
            "return typeof Uint8Array.fromBase64 === 'function'",
        )
        .await,
    ) == Some(json!(true));
    let mut cases = vec![
        ("slice", "a.push(seed.slice());"),
        ("map", "a.push(seed.map((x) => x));"),
        ("filter", "a.push(seed.filter(() => true));"),
        ("buffer slice", "a.push(buffer.slice(0));"),
        ("species replaced by undefined", "a.push(species.slice());"),
    ];
    if has_base64 {
        cases.push(("fromBase64", "a.push(Uint8Array.fromBase64(base64));"));
    }
    for (label, body) in cases {
        let code = format!(
            r#"
            const seed = new Uint8Array(1 << 22).fill(3);
            Object.defineProperty(seed, "constructor", {{ value: undefined }});
            const buffer = new ArrayBuffer(1 << 22);
            Object.defineProperty(buffer, "constructor", {{ value: undefined }});
            const species = new Uint8Array(1 << 22).fill(3);
            Object.defineProperty(species, "constructor", {{ value: {{ [Symbol.species]: undefined }} }});
            const base64 = "A".repeat(1 << 22);
            const a = [];
            try {{ for (let i = 0; i < 300; i++) {{ {body} }} return ["no error", a.length]; }} catch (error) {{ return [error.name, error.message, error instanceof Error, a.length > 0]; }}
        "#
        );
        let started = Instant::now();
        let result = run_with(&sandbox, &code, deadline_ms(20_000)).await;
        assert_eq!(
            value(&result),
            Some(json!(["InternalError", "out of memory", true, true])),
            "{label}: {result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(15), "{label}");
    }
    wait_until_idle(&sandbox).await;
}

/// Once the limit is exceeded every allocation is checked again, however many follow, and the
/// first one after the buffers are released goes through. The check is a regular op: it collects
/// garbage inside the call, which a fast call may not do, and it is called enough times here for
/// the engine to compile the loop and take the fast path if the op had one.
#[tokio::test]
async fn an_exceeded_limit_stays_exceeded_for_thousands_of_allocations_until_the_buffers_go() {
    let sandbox = limited(MIN_MEMORY_LIMIT_BYTES as u64);
    let result = run_with(
        &sandbox,
        r#"
        // 31 MiB is checked and fine; 1.5 MiB more is under the stride between checks, so it is
        // let through and the live buffers are over the 32 MiB limit; the next one is refused.
        let held = [new Uint8Array(31 << 20), new Uint8Array(3 << 19)];
        try { new Uint8Array(1 << 20); throw new Error("not refused"); }
        catch (error) { if (error.message !== "out of memory") throw error; }
        let refused = 0;
        for (let i = 0; i < 4000; i++) {
            try { new Uint8Array(8); } catch (error) { if (error.message === "out of memory") refused++; }
        }
        const heldAtEnd = held.length;
        held = null;
        const after = new Uint8Array(8).length;
        return { heldAtEnd, refused, after };
    "#,
        deadline_ms(60_000),
    )
    .await;
    assert_eq!(
        value(&result),
        Some(json!({ "heldAtEnd": 2, "refused": 4000, "after": 8 })),
        "{result:?}"
    );
}

/// The memory check collects garbage inside the call. V8's fast-call contract forbids that, so the
/// op is declared without a fast path (the sticky-limit test above calls it often enough for the
/// engine to use one if it had one, and does not notice the difference, which is why this checks
/// the declaration).
#[test]
fn the_memory_check_is_a_regular_op_because_it_collects_garbage() {
    let ops = isolate::declared_ops();
    let has_fast_path = |name: &str| {
        let op = ops.iter().find(|op| op.name == name).unwrap();
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| op.fast_fn())).is_ok()
    };
    assert!(
        !has_fast_path("op_codemode_memory_exceeded"),
        "op_codemode_memory_exceeded collects garbage and must not be a fast op"
    );
    // The probe does see fast paths: the ops that only post a message have one.
    assert!(has_fast_path("op_codemode_output"));
}

#[tokio::test]
async fn buffers_that_are_dropped_do_not_count_against_the_limit() {
    // 400 MiB allocated over the run, never more than a few MiB alive: the check collects
    // garbage before it judges.
    let sandbox = limited(64 * 1024 * 1024);
    let result = run_with(
        &sandbox,
        r#"
        let sum = 0;
        for (let i = 0; i < 100; i++) { const b = new Uint8Array(1 << 22); b[0] = 1; sum += b[0]; }
        return sum;
    "#,
        deadline_ms(30_000),
    )
    .await;
    assert_eq!(value(&result), Some(json!(100)));
}

#[tokio::test]
async fn typed_arrays_still_work() {
    let sandbox = limited(64 * 1024 * 1024);
    let result = run(
        &sandbox,
        r#"
        const u8 = Uint8Array.from([3, 1, 2]);
        const f = new Float32Array([1.5, 2.5]);
        const buf = new ArrayBuffer(8);
        const view = new DataView(buf);
        view.setInt16(0, -2);
        class Bytes extends Uint8Array {}
        const sub = new Bytes(4);
        return [
            u8 instanceof Uint8Array, u8.constructor === Uint8Array, Uint8Array.name, Uint8Array.BYTES_PER_ELEMENT,
            Array.from(u8.toSorted()), Array.from(u8.slice(1)), Array.from(u8.map((x) => x * 2)),
            f.byteLength, buf.byteLength, view.getInt16(0), ArrayBuffer.isView(f),
            Object.getPrototypeOf(Uint8Array) === Object.getPrototypeOf(Float32Array),
            sub instanceof Bytes && sub instanceof Uint8Array && sub.length === 4,
            new Uint8Array(buf, 2, 3).length, buf.slice(2).byteLength, buf.transfer().byteLength,
            typeof BigInt64Array, new BigUint64Array(2).length,
        ];
    "#,
    )
    .await;
    assert_eq!(
        value(&result),
        Some(json!([
            true,
            true,
            "Uint8Array",
            1,
            [1, 2, 3],
            [1, 2],
            [6, 2, 4],
            8,
            8,
            -2,
            true,
            true,
            true,
            3,
            6,
            8,
            "function",
            2
        ]))
    );
}

#[tokio::test]
async fn without_a_configured_limit_array_buffers_are_still_bounded() {
    let sandbox = sandbox(vec![]);
    // A single request over the default ceiling (1 GiB) is refused before any page is touched.
    // Where the operating system refuses the allocation itself, the engine's own `RangeError`
    // is the equally acceptable answer.
    let result = run(
        &sandbox,
        "try { new Uint8Array(3 * 2 ** 30); return 'allocated'; } catch (error) { return error.name; }",
    )
    .await;
    let name = value(&result).unwrap();
    assert!(
        name == json!("InternalError") || name == json!("RangeError"),
        "{result:?}"
    );
}

// ------------------------------------------------------------------------------------------------
// What the script sees
// ------------------------------------------------------------------------------------------------

/// The ECMAScript built-ins of the engine's global object that stay, and the sandbox's own
/// globals. Everything else the engine defines (`Deno`, `__bootstrap`, `WebAssembly`,
/// `SharedArrayBuffer`, `Intl`, `queueMicrotask`) is removed (`prelude.js`).
const EXPECTED_GLOBALS: &[&str] = &[
    // ECMAScript
    "AggregateError",
    "Array",
    "ArrayBuffer",
    "AsyncDisposableStack",
    "Atomics",
    "BigInt",
    "BigInt64Array",
    "BigUint64Array",
    "Boolean",
    "DataView",
    "Date",
    "DisposableStack",
    "Error",
    "EvalError",
    "FinalizationRegistry",
    "Float16Array",
    "Float32Array",
    "Float64Array",
    "Function",
    "Infinity",
    "Int16Array",
    "Int32Array",
    "Int8Array",
    "Iterator",
    "JSON",
    "Map",
    "Math",
    "NaN",
    "Number",
    "Object",
    "Promise",
    "Proxy",
    "RangeError",
    "ReferenceError",
    "Reflect",
    "RegExp",
    "Set",
    "String",
    "SuppressedError",
    "Symbol",
    "SyntaxError",
    "Temporal",
    "TypeError",
    "URIError",
    "Uint16Array",
    "Uint32Array",
    "Uint8Array",
    "Uint8ClampedArray",
    "WeakMap",
    "WeakRef",
    "WeakSet",
    "decodeURI",
    "decodeURIComponent",
    "encodeURI",
    "encodeURIComponent",
    "escape",
    "eval",
    "globalThis",
    "isFinite",
    "isNaN",
    "parseFloat",
    "parseInt",
    "undefined",
    "unescape",
    // the sandbox's
    "ALL_TOOLS",
    "console",
    "exit",
    "image",
    "load",
    "store",
    "text",
    "tools",
    // a configured global and a namespace
    "attach",
    "models",
];

#[tokio::test]
async fn the_global_object_holds_ecmascript_and_the_sandbox_and_nothing_else() {
    let sandbox = sandbox_with(SandboxOptions {
        globals: vec![
            sync_tool("attach", |_| Ok(None)),
            sync_tool("models.list", |_| Ok(None)),
        ],
        ..SandboxOptions::default()
    });
    let result = run(
        &sandbox,
        "return Object.getOwnPropertyNames(globalThis).sort()",
    )
    .await;
    let actual: BTreeSet<String> = serde_json::from_value(value(&result).unwrap()).unwrap();
    let expected: BTreeSet<String> = EXPECTED_GLOBALS
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    assert_eq!(
        actual.difference(&expected).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "defined but not expected"
    );
    assert_eq!(
        expected.difference(&actual).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "expected but not defined"
    );
    // Symbol-keyed and inherited members too: nothing of the engine's is on the prototype chain.
    let more = run(
        &sandbox,
        r#"
        const proto = Object.getPrototypeOf(globalThis);
        return [
            Object.getOwnPropertyNames(proto).sort().join(","),
            Object.getOwnPropertySymbols(globalThis).length,
            Object.getPrototypeOf(proto) === Object.prototype || Object.getPrototypeOf(proto) === null,
        ];
    "#,
    )
    .await;
    let Some(Value::Array(parts)) = value(&more) else {
        panic!("{more:#?}");
    };
    // The global's prototype holds only its `constructor`, and the engine defines no symbol on the
    // global object.
    assert_eq!(parts, [json!("constructor"), json!(0), json!(true)]);
}

#[tokio::test]
async fn the_sandbox_globals_cannot_be_replaced_or_removed() {
    let sandbox = sandbox_with(SandboxOptions {
        tools: vec![echo()],
        globals: vec![
            sync_tool("attach", |_| Ok(None)),
            sync_tool("models.list", |_| Ok(None)),
        ],
        ..SandboxOptions::default()
    });
    let result = run(
        &sandbox,
        r#"
        const names = ["tools", "ALL_TOOLS", "console", "text", "image", "exit", "store", "load", "attach", "models"];
        return names.map((name) => {
            const d = Object.getOwnPropertyDescriptor(globalThis, name);
            const deleted = Reflect.deleteProperty(globalThis, name);
            const set = Reflect.set(globalThis, name, null);
            return [name, d.writable, d.configurable, d.enumerable, deleted, set, globalThis[name] !== null];
        });
    "#,
    )
    .await;
    let Some(Value::Array(rows)) = value(&result) else {
        panic!("{result:#?}");
    };
    assert_eq!(rows.len(), 10);
    for row in rows {
        let row = row.as_array().unwrap();
        // writable, configurable false; enumerable; delete and set refused; value intact.
        assert_eq!(
            row[1..],
            [
                json!(false),
                json!(false),
                json!(true),
                json!(false),
                json!(false),
                json!(true)
            ],
            "{row:?}"
        );
    }
}

#[tokio::test]
async fn engine_only_builtins_are_removed() {
    let sandbox = sandbox(vec![]);
    let result = run(
        &sandbox,
        r#"
        return [
            typeof Intl, typeof WebAssembly, typeof SharedArrayBuffer, typeof queueMicrotask,
            typeof Deno, typeof __bootstrap, typeof Atomics, typeof Atomics.wait, typeof Atomics.waitAsync,
            // Still there, and working without Intl:
            (1234.5).toLocaleString().length > 0, "a".localeCompare("b"), typeof Date.prototype.toLocaleDateString,
            typeof WeakRef, typeof FinalizationRegistry, typeof Iterator,
            // Reaching for the removed globals another way finds nothing.
            Object.getOwnPropertyNames(globalThis).includes("WebAssembly"),
            Reflect.ownKeys(globalThis).some((key) => typeof key === "string" && key.startsWith("op_")),
        ];
    "#,
    )
    .await;
    assert_eq!(
        value(&result),
        Some(json!([
            "undefined",
            "undefined",
            "undefined",
            "undefined",
            "undefined",
            "undefined",
            "object",
            "undefined",
            "undefined",
            true,
            -1,
            "function",
            "function",
            "function",
            "function",
            false,
            false
        ]))
    );
}

#[test]
fn the_engine_refuses_webassembly_compilation_even_where_the_global_exists() {
    // Without the prelude `WebAssembly` is there, as it is in any V8 isolate; the engine-level
    // refusal is what remains if a script ever reached it.
    let bytes = "new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0])";
    let present = isolate::evaluate_hardened("typeof WebAssembly").unwrap();
    assert_eq!(present, "object");
    let compiled = isolate::evaluate_hardened(&format!(
        "try {{ new WebAssembly.Module({bytes}); 'compiled' }} catch (error) {{ error.name + ': ' + error.message }}"
    ))
    .unwrap();
    assert!(compiled.contains("disallowed"), "{compiled}");
}

#[tokio::test]
async fn nothing_in_the_script_reaches_an_op() {
    let sandbox = sandbox(vec![echo()]);
    let result = run(
        &sandbox,
        r#"
        const found = [];
        const seen = new Set();
        const walk = (value, path, depth) => {
            if (depth > 4 || value === null || (typeof value !== "object" && typeof value !== "function") || seen.has(value)) return;
            seen.add(value);
            let keys = [];
            try { keys = Reflect.ownKeys(value); } catch { return; }
            for (const key of keys) {
                if (typeof key === "string" && /^op_|Deno|__bootstrap|core$/.test(key)) found.push(path + "." + key);
                let next;
                try { next = value[key]; } catch { continue; }
                if (key !== "caller" && key !== "callee" && key !== "arguments") walk(next, path + "." + String(key), depth + 1);
            }
        };
        walk(globalThis, "globalThis", 0);
        return found;
    "#,
    )
    .await;
    assert_eq!(value(&result), Some(json!([])));
}

#[tokio::test]
async fn no_timer_of_any_kind_exists() {
    // `Atomics.waitAsync` would be one; the stall detector would otherwise see a script waiting on
    // a timer as waiting on nothing.
    let sandbox = sandbox(vec![]);
    let result = run(
        &sandbox,
        r#"
        const found = [];
        for (const name of ["setTimeout", "setInterval", "setImmediate", "requestAnimationFrame", "requestIdleCallback", "queueMicrotask"]) {
            if (name in globalThis) found.push(name);
        }
        return found;
    "#,
    )
    .await;
    assert_eq!(value(&result), Some(json!([])));
}

#[tokio::test]
async fn output_items_keep_their_order_across_text_and_image() {
    let sandbox = sandbox(vec![]);
    let result = run(
        &sandbox,
        r#"text("a"); image("data:image/png;base64,iVBORw0KGgo="); console.warn("b"); console.debug(1, 2); console.info(null)"#,
    )
    .await;
    assert_eq!(
        output(&result),
        [
            text("a"),
            OutputItem::Image {
                data: "iVBORw0KGgo=".into(),
                mime_type: "image/png".into()
            },
            console("b"),
            console("1 2"),
            console("null"),
        ]
    );
}
