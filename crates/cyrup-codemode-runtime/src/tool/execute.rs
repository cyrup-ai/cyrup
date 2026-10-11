//! Runs one codemode script in the sandbox (pi `executeCodemode`,
//! `extensions/codemode/execute.ts:229-433` @v1.0.1, CODE-009, CODE-012).
//!
//! The steps, in upstream's order: parse the source (`// @options:` line), build the sandbox's
//! tools from the host's callable set and its globals (`searchTools` …, `models.*`), replay the
//! branch's `codemode-store` entries into `load()`, run the script, mark calls the script left
//! running as cancelled, append the script's `store()` writes when it succeeded, apply the output
//! budget and spill, and return the "Script completed" / "Script failed" result with the nested
//! calls as `details`.
//!
//! # Production call path
//!
//! [`CodemodeTool::execute`](cyrup_core::Tool::execute) calls [`execute_codemode`] for every model
//! call of the `codemode` tool.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cyrup_codemode::declarations::render_tool_sample;
use cyrup_codemode::identifier::IdentifierTable;
use cyrup_codemode::js::json_stringify;
use cyrup_codemode::output::{
    DEFAULT_MAX_OUTPUT_TOKENS, format_output, join_adjacent_text, label_images, save_image_output,
    spill_output, truncate_output,
};
use cyrup_codemode::source::parse_codemode_source;
use cyrup_codemode::types::{OutputItem, ToolDeclaration};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::Semaphore;

use super::description::{callable_tools, to_codemode_declaration};
use super::discovery::discovery_globals;
use super::host::{CodemodeHost, NestedOutcome};
use super::models::models_globals;
use super::recorder::{ARGS_PREVIEW_CHARS, ERROR_PREVIEW_CHARS, Recorder, truncate_text};
use super::store::{CodemodeStoreEntryData, read_codemode_store};
use super::{CodemodeNestedCall, CodemodeNestedCallStatus, CodemodeToolOptions};
use crate::types::{
    CallStatus, CodemodeCall, CodemodeError, CodemodeResult, CodemodeTool, CodemodeToolContext,
    DEFAULT_ACTIVE_LIMIT, DEFAULT_TOOL_WALL_TIMEOUT, Deadline, ErrorKind, ExecuteOptions,
    SandboxOptions, ToolCallback, UnobservedErrors,
};

/// Heap limit for the script's isolate: a runaway script must not grow until it takes the session
/// down (`CODEMODE_MEMORY_LIMIT_BYTES`, `execute.ts:55`). An overrun fails inside the script.
pub const CODEMODE_MEMORY_LIMIT_BYTES: u64 = 256 * 1024 * 1024;

/// Nested tool calls one script may have running at once; the rest of a `Promise.all` over
/// thousands of items queue for a slot (the same shape as `MAX_CONCURRENT_MODEL_CALLS`, which
/// upstream caps at 4 for `models.*` and leaves out for tools).
///
/// [CYRUP-DELTA] Upstream runs every nested call at once, which a JavaScript runtime with a
/// garbage collector and shared references survives. Here each running call is a task with its own
/// copy of the session's context, and 3000 parallel `read`s held the process at 3.4 GB. Sixteen is
/// above what a model's own parallel tool calls ever reach and far below what costs memory. A call
/// that is waiting for a slot has no row yet and is cancelled with the script.
pub const MAX_CONCURRENT_NESTED_CALLS: usize = 16;

/// What a script is told when it ended while one of its calls was still waiting for a slot.
const CANCELLED_WHILE_QUEUED: &str = "The tool call was cancelled before it started";

/// The message of the `DOMException` an ordinary `AbortController.abort()` carries as its reason
/// (`signal.reason.message`), which upstream renders as `Script aborted: <reason>`
/// (`execute.ts:255-256`). A [`CancelToken`] carries no reason, and a tool call is cancelled by the
/// user or the run, never with a more specific one.
pub const ABORT_REASON: &str = "This operation was aborted";

/// The text items of a tool result joined with `\n` (`textOf`, `execute.ts:58-63`).
fn text_of(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The value a script receives for a nested call (`toScriptValue`, `execute.ts:303-312`): a tool that
/// declares an output schema resolves to its `structuredContent`, also for error results that carry
/// one (such as MCP results with `isError`); any other tool resolves to its text content. Other
/// failures reject with the tool's error text.
fn to_script_value(tool: &dyn Tool, outcome: &NestedOutcome) -> Result<Option<Value>, String> {
    if tool.output_schema().is_some()
        && let Some(structured) = &outcome.result.structured_content
    {
        return Ok(Some(structured.clone()));
    }
    let text = text_of(&outcome.result);
    if outcome.is_error {
        return Err(if text.is_empty() {
            format!("Tool \"{}\" failed", tool.name())
        } else {
            text
        });
    }
    Ok(Some(Value::String(text)))
}

/// Compact JSON of a call's arguments, truncated for display (`previewArgs`, `execute.ts:40-48`);
/// empty for `undefined`.
fn preview_args(args: Option<&Value>) -> String {
    args.map_or_else(String::new, |args| {
        truncate_text(&json_stringify(args), ARGS_PREVIEW_CHARS)
    })
}

/// `Tool calls made before the failure …` (`formatCallSummary`, `execute.ts:241-245`).
fn format_call_summary(calls: &[CodemodeNestedCall]) -> String {
    if calls.is_empty() {
        return "No tool calls were made.".to_owned();
    }
    let calls: Vec<String> = calls
        .iter()
        .map(|call| {
            let status = match call.status {
                CodemodeNestedCallStatus::Running => "running",
                CodemodeNestedCallStatus::Ok => "ok",
                CodemodeNestedCallStatus::Error => "error",
                CodemodeNestedCallStatus::Cancelled => "cancelled",
            };
            format!("{} ({status})", call.name)
        })
        .collect();
    format!(
        "Tool calls made before the failure (they are not undone): {}",
        calls.join(", ")
    )
}

/// [CYRUP-DELTA] The note a script that succeeded gets when it ended with tool calls still running,
/// which were cancelled: `main();` without an `await`, `forEach(async ...)`, a missing `await`.
/// Upstream marks them `cancelled` in the details and says nothing in the text, so the result read
/// as a plain success, with the read that never came back as "(no output)", or with some of the
/// files a `forEach(async ...)` write loop was meant to make and some not, depending on timing. The
/// model reads only the text. `None` when every call settled.
fn format_unfinished_calls_note(calls: &[CodemodeCall]) -> Option<String> {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for call in calls
        .iter()
        .filter(|call| call.status == CallStatus::Cancelled)
    {
        match counts.iter_mut().find(|(name, _)| *name == call.name) {
            Some((_, count)) => *count += 1,
            None => counts.push((&call.name, 1)),
        }
    }
    let total: usize = counts.iter().map(|(_, count)| count).sum();
    if total == 0 {
        return None;
    }
    let names: Vec<String> = counts
        .iter()
        .map(|(name, count)| {
            if *count == 1 {
                (*name).to_owned()
            } else {
                format!("{name} x{count}")
            }
        })
        .collect();
    let (calls, were) = if total == 1 {
        ("call", "was")
    } else {
        ("calls", "were")
    };
    Some(format!(
        "Note: {total} tool {calls} {were} still running when the script ended and {were} cancelled: {}. Await every call before the script ends (await Promise.all([...]), await main()).",
        names.join(", ")
    ))
}

/// [CYRUP-DELTA] The note a script that succeeded gets when an error in it was never handled: a
/// tool call that failed and was not awaited, an `async` function that threw and was not awaited. A
/// call that is awaited rejects into the script, which fails or catches it; one that is not has no
/// one to tell, and upstream ends the script as a plain success, so a `tools.write(...)` that was
/// not awaited and failed read as a file that was written. `None` when every error was handled.
fn format_unobserved_errors_note(errors: &UnobservedErrors) -> Option<String> {
    if errors.total == 0 {
        return None;
    }
    let listed: Vec<String> = errors
        .shown
        .iter()
        .map(|error| match &error.call {
            Some(call) => format!("{call}: {}", error.message),
            None => error.message.clone(),
        })
        .collect();
    let more = errors.total.saturating_sub(listed.len());
    let (errors_, was) = if errors.total == 1 {
        ("error", "was")
    } else {
        ("errors", "were")
    };
    let rest = if more > 0 {
        format!(" (and {more} more)")
    } else {
        String::new()
    };
    Some(format!(
        "Note: {} {errors_} {was} never handled and did not fail the script: {}{rest}. A tool call or async function that is not awaited loses its error. Await every call before the script ends (await Promise.all([...]), await main()) and catch the errors you expect.",
        errors.total,
        listed.join("; "),
    ))
}

/// [CYRUP-DELTA] The note a script that succeeded gets when calls it had started failed after it
/// ended, and something was waiting on them: the siblings a `Promise.all` had already given up on, a
/// call with a `.catch()`, a call an `async` function awaited that nobody awaited (`main();`,
/// `forEach(async ...)`), a `.then(f)` with no `.catch()`. The engine cannot tell the first two,
/// which the script dealt with, from the last two, which lost the error, so the note says what
/// happened and gives no advice: the "never handled" note's ("await every call") is wrong for a
/// script that did. Without it, `main();` and a failed `write` read as a plain success. `None` when
/// no such call failed.
fn format_late_failures_note(errors: &UnobservedErrors) -> Option<String> {
    if errors.late_total == 0 {
        return None;
    }
    let listed: Vec<String> = errors
        .late_shown
        .iter()
        .map(|error| match &error.call {
            Some(call) => format!("{call}: {}", error.message),
            None => error.message.clone(),
        })
        .collect();
    let more = errors.late_total.saturating_sub(listed.len());
    let (calls, errors_) = if errors.late_total == 1 {
        ("call", "error")
    } else {
        ("calls", "errors")
    };
    let rest = if more > 0 {
        format!(" (and {more} more)")
    } else {
        String::new()
    };
    Some(format!(
        "Note: {} {calls} failed after the script ended, so the script did not see the {errors_}: {}{rest}.",
        errors.late_total,
        listed.join("; "),
    ))
}

/// [CYRUP-DELTA] What a script that ran into one of the default limits is told: how to raise them.
/// An explicit `timeout_ms` is the model's own choice and needs no advice.
fn default_limits_hint() -> String {
    format!(
        " Default limits: {} s of the script's own running time and {} min in all. To allow more, make the first line of the script `// @options: {{\"timeout_ms\": 3600000}}` (milliseconds); an explicit timeout_ms replaces both limits.",
        DEFAULT_ACTIVE_LIMIT.as_secs(),
        DEFAULT_TOOL_WALL_TIMEOUT.as_secs() / 60,
    )
}

/// The failure text after `Script error:` (`formatError`, `execute.ts:247-258`): the script's own
/// stack (or `Name: message`), a timeout, abort or sandbox failure, then the call summary.
/// `default_limits` is whether the script ran under the default limits, not a `timeout_ms` of its
/// own.
fn format_error(
    error: &CodemodeError,
    calls: &[CodemodeNestedCall],
    default_limits: bool,
) -> String {
    let head = match error.kind {
        ErrorKind::Script => error.stack.clone().unwrap_or_else(|| {
            format!(
                "{}: {}",
                error.name.as_deref().unwrap_or("Error"),
                error.message
            )
        }),
        ErrorKind::Timeout if default_limits => {
            format!(
                "Script timed out: {}.{}",
                error.message,
                default_limits_hint()
            )
        }
        ErrorKind::Timeout => format!("Script timed out: {}", error.message),
        ErrorKind::Aborted => format!("Script aborted: {}", error.message),
        ErrorKind::Sandbox => format!("Script sandbox failed: {}", error.message),
    };
    format!("{head}\n\n{}", format_call_summary(calls))
}

/// `(elapsed / 1000).toFixed(1)`: one decimal, ties away from zero as `Number.prototype.toFixed`
/// rounds them (`execute.ts:424`), where Rust's `{:.1}` rounds ties to even.
fn to_fixed_1(seconds: f64) -> String {
    let scaled = seconds * 10.0;
    let floor = scaled.floor();
    let rounded = if scaled - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    };
    format!("{:.1}", rounded / 10.0)
}

/// One nested tool as the sandbox calls it (`sandboxTools`, `execute.ts:337-361`): the call is
/// recorded as a row, run through the host's pipeline, and its outcome converted to the script's
/// value.
fn nested_tool(
    tool: &Arc<dyn Tool>,
    sample: &str,
    identifiers: &IdentifierTable,
    host: Option<&Arc<dyn CodemodeHost>>,
    recorder: &Arc<Recorder>,
    slots: &Arc<Semaphore>,
) -> CodemodeTool {
    let (tool, host, recorder, slots) = (
        Arc::clone(tool),
        host.cloned(),
        Arc::clone(recorder),
        Arc::clone(slots),
    );
    let declaration = ToolDeclaration::new(tool.name())
        .with_identifier(identifiers.get(tool.name()))
        .with_description(sample);
    let execute: ToolCallback =
        Arc::new(move |args: Option<Value>, context: CodemodeToolContext| {
            let (tool, host, recorder, slots) = (
                Arc::clone(&tool),
                host.clone(),
                Arc::clone(&recorder),
                Arc::clone(&slots),
            );
            let call: BoxFuture<'static, crate::types::ToolResult> = Box::pin(async move {
                // Held until the call ends. The script ending fires the call's token, so a queued
                // call does not keep waiting for a slot whose result nobody will read. A free slot
                // wins over a token that is already fired: such a call still runs, as upstream's
                // does, and the pipeline answers it as aborted.
                let _slot = tokio::select! {
                    biased;
                    slot = slots.acquire() => slot.map_err(|error| error.to_string())?,
                    () = context.cancel.cancelled() => {
                        return Err(CANCELLED_WHILE_QUEUED.to_owned());
                    }
                };
                let row = recorder.begin(CodemodeNestedCall {
                    id: format!("{}/?", recorder.tool_call_id()),
                    name: tool.name().to_owned(),
                    args: preview_args(args.as_ref()),
                    status: CodemodeNestedCallStatus::Running,
                    duration_ms: None,
                    error: None,
                    cost: None,
                });
                let started = Instant::now();
                // Only tools the host lists are callable, so a host is set here.
                let Some(host) = host else {
                    return Err("Tool calls need a session".to_owned());
                };
                let outcome = host
                    .execute_nested(
                        recorder.tool_call_id(),
                        tool.name(),
                        args.unwrap_or(Value::Null),
                        context.cancel.clone(),
                    )
                    .await;
                let converted = to_script_value(tool.as_ref(), &outcome);
                recorder.update(row, |call| {
                    call.id = outcome.call_id.to_string();
                    call.duration_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
                    if outcome.is_error {
                        call.status = if context.cancel.is_cancelled() {
                            CodemodeNestedCallStatus::Cancelled
                        } else {
                            CodemodeNestedCallStatus::Error
                        };
                        let text = text_of(&outcome.result);
                        let text = if text.is_empty() {
                            format!("Tool \"{}\" failed", tool.name())
                        } else {
                            text
                        };
                        call.error = Some(truncate_text(&text, ERROR_PREVIEW_CHARS));
                    } else {
                        call.status = CodemodeNestedCallStatus::Ok;
                    }
                });
                converted
            });
            call
        });
    CodemodeTool {
        declaration,
        execute,
    }
}

/// What the result says when the script produced no output.
const NO_OUTPUT: &str = "(no output)";

fn content_of(item: OutputItem) -> Content {
    match item {
        OutputItem::Text(text) | OutputItem::Console(text) => Content::text(text),
        OutputItem::Image { data, mime_type } => Content::Image { data, mime_type },
    }
}

/// Run one script. Without a session context (an empty host slot) scripts cannot call tools,
/// `store()` starts empty and writes are dropped (`execute.ts:299-300`).
///
/// # Errors
///
/// * the source did not parse: the [`CodemodeSourceError`](cyrup_codemode::source::CodemodeSourceError)
///   text, which the model reads back;
/// * no sandbox could be created ([`SandboxUnavailable`](super::factory::SandboxUnavailable));
/// * the session refused to record the script's `store()` writes.
///
/// A script that fails is not an `Err`: it is a normal result with `is_error` set.
pub async fn execute_codemode(
    tool_call_id: &ToolCallId,
    code: &str,
    cancel: CancelToken,
    on_update: ToolUpdateSink,
    options: &CodemodeToolOptions,
) -> Result<ToolResult, ToolError> {
    let started = Instant::now();
    let parsed = parse_codemode_source(code).map_err(|error| ToolError::new(error.to_string()))?;
    let explicit_timeout = parsed.options.timeout_ms.is_some();
    let host = options.host.current();
    let recorder = Arc::new(Recorder::new(tool_call_id.clone(), on_update));

    let callable: Vec<Arc<dyn Tool>> = host
        .as_ref()
        .map(|host| callable_tools(&host.callable_tools()))
        .unwrap_or_default();
    // [CYRUP-DELTA] One identifier per tool, shared by the declarations, `ALL_TOOLS`, the discovery
    // globals and `tools.<id>`; upstream registers only the first of two tools with one identifier.
    let identifiers = IdentifierTable::assign(callable.iter().map(|tool| tool.name()));
    // ALL_TOOLS entries carry the declaration.
    let samples: BTreeMap<String, String> = callable
        .iter()
        .map(|tool| {
            (
                tool.name().to_owned(),
                render_tool_sample(
                    &to_codemode_declaration(
                        tool.as_ref(),
                        &cyrup_core::normalized_prompt_guidelines(tool.as_ref()),
                        &identifiers,
                    ),
                    None,
                ),
            )
        })
        .collect();
    let slots = Arc::new(Semaphore::new(MAX_CONCURRENT_NESTED_CALLS));
    let tools: Vec<CodemodeTool> = callable
        .iter()
        .map(|tool| {
            let sample = samples.get(tool.name()).map_or("", String::as_str);
            nested_tool(tool, sample, &identifiers, host.as_ref(), &recorder, &slots)
        })
        .collect();

    let mut globals =
        discovery_globals(Arc::new(callable), Arc::new(samples), Arc::new(identifiers));
    if options.models
        && let Some(models) = host.as_ref().and_then(|host| host.models())
    {
        globals.extend(models_globals(
            models,
            Arc::clone(&recorder),
            &options.docs_path,
        ));
    }

    let sandbox = options
        .sandboxes
        .create(SandboxOptions {
            tools,
            globals,
            // [CYRUP-DELTA] Upstream runs a script without `timeout_ms` for ever. Here it gets the
            // default limits, which an explicit `timeout_ms` replaces.
            deadline: parsed
                .options
                .timeout_ms
                .map_or(Deadline::After(DEFAULT_TOOL_WALL_TIMEOUT), |ms| {
                    Deadline::After(Duration::from_millis(ms))
                }),
            active_limit: (!explicit_timeout).then_some(DEFAULT_ACTIVE_LIMIT),
            memory_limit_bytes: Some(CODEMODE_MEMORY_LIMIT_BYTES),
        })
        .map_err(|error| ToolError::new(error.to_string()))?;

    let store = match &host {
        Some(host) => read_codemode_store(&host.branch_custom_entries().await),
        None => serde_json::Map::new(),
    };
    // Snapshots of the calls go out at most once per interval; this publishes the ones an interval
    // held back, for as long as the script runs.
    let settled = tokio::select! {
        biased;
        settled = sandbox.execute(
            &parsed.code,
            ExecuteOptions {
                cancel: Some(cancel),
                cancel_reason: Some(ABORT_REASON.to_owned()),
                deadline: None,
                store,
            },
        ) => settled,
        never = recorder.publish_until_dropped() => match never {},
    };
    sandbox.close().await;
    let result = settled.unwrap_or_else(|closed| CodemodeResult::Failed {
        error: CodemodeError {
            kind: ErrorKind::Sandbox,
            name: None,
            message: closed.to_string(),
            stack: None,
        },
        output: Vec::new(),
        calls: Vec::new(),
    });
    // Calls still marked running were cut off by the script ending, a timeout, or an abort.
    let calls = recorder.finish();
    // The last state reaches the subscribers even when no interval was left to carry it.
    recorder.flush();

    let ok = matches!(result, CodemodeResult::Completed { .. });
    // The returned value is laid out with the script's own output; the error and the notes below
    // are not script output, so they get no `==> text N/M <==` line (`execute.ts:502-512` @v1.1.0).
    let mut items: Vec<OutputItem>;
    match result {
        CodemodeResult::Completed {
            value,
            mut output,
            calls: ran,
            store_writes,
            unobserved,
        } => {
            if let (Some(host), Some(entry)) =
                (&host, CodemodeStoreEntryData::from_writes(store_writes))
            {
                host.append_store_entry(entry).await.map_err(|error| {
                    ToolError::new(format!(
                        "Could not record the script's store() writes: {error}"
                    ))
                })?;
            }
            // pi extension: a returned value is appended like text() (`valueText`,
            // `execute.ts:236-239`): a string as it is, anything else as compact JSON.
            if let Some(value) = value {
                output.push(OutputItem::Text(value.into_text()));
            }
            items = format_output(output);
            if let Some(note) = format_unfinished_calls_note(&ran) {
                items.push(OutputItem::Text(note));
            }
            if let Some(note) = format_unobserved_errors_note(&unobserved) {
                items.push(OutputItem::Text(note));
            }
            if let Some(note) = format_late_failures_note(&unobserved) {
                items.push(OutputItem::Text(note));
            }
        }
        CodemodeResult::Failed { error, output, .. } => {
            items = format_output(output);
            items.push(OutputItem::Text(format!(
                "Script error:\n{}",
                format_error(&error, &calls, !explicit_timeout)
            )));
        }
    }
    let generated_images = recorder.generated_images();
    if generated_images > 0
        && !items
            .iter()
            .any(|item| matches!(item, OutputItem::Image { .. }))
    {
        items.push(OutputItem::Text(format!(
            "Note: models.generateImages() returned {generated_images} image{} that the script did not show. Show each image block of result.output with image(block).",
            if generated_images == 1 { "" } else { "s" }
        )));
    }

    let truncated = truncate_output(
        join_adjacent_text(items),
        parsed
            .options
            .max_output_tokens
            .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS),
        |text| spill_output(&std::env::temp_dir(), text),
    );
    // After truncation, which joins the text items and moves the images after them, so each path
    // stays next to its image and is never cut (`execute.ts:519-526` @v1.1.0). Each label is
    // then joined to the text around it, so adjacent text reaches the model as one block.
    let temp_dir = std::env::temp_dir();
    let output = join_adjacent_text(
        label_images(truncated.items, |mime_type, bytes| {
            save_image_output(&temp_dir, mime_type, bytes)
        })
        .map_err(|error| ToolError::new(error.to_string()))?,
    );
    let wall_time = to_fixed_1(started.elapsed().as_secs_f64());
    let header = format!(
        "{}\nWall time {wall_time} seconds\nOutput:\n",
        if ok {
            "Script completed"
        } else {
            "Script failed"
        }
    );
    let mut details = recorder.snapshot();
    details.full_output_path = truncated
        .full_output_path
        .as_ref()
        .map(|path| path.display().to_string());
    let mut content = vec![Content::text(header)];
    // [CYRUP-DELTA] A script that printed nothing and returned nothing used to leave a bare
    // "Output:", which reads like a result that was cut off.
    if output.is_empty() {
        content.push(Content::text(NO_OUTPUT));
    }
    content.extend(output.into_iter().map(content_of));
    Ok(ToolResult {
        content,
        details: Some(
            serde_json::to_value(details).map_err(|error| ToolError::new(error.to_string()))?,
        ),
        usage: recorder.model_usage(),
        is_error: !ok,
        ..ToolResult::default()
    })
}

#[cfg(test)]
mod tests;
