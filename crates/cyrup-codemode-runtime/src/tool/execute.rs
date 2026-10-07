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
use cyrup_codemode::js::json_stringify;
use cyrup_codemode::output::{
    DEFAULT_MAX_OUTPUT_TOKENS, label_images, save_image_output, spill_output, truncate_output,
};
use cyrup_codemode::source::parse_codemode_source;
use cyrup_codemode::types::{OutputItem, ToolDeclaration};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use futures::future::BoxFuture;
use serde_json::Value;

use super::description::{callable_tools, to_codemode_declaration};
use super::discovery::discovery_globals;
use super::host::{CodemodeHost, NestedOutcome};
use super::models::models_globals;
use super::recorder::{ARGS_PREVIEW_CHARS, ERROR_PREVIEW_CHARS, Recorder, truncate_text};
use super::store::{CodemodeStoreEntryData, read_codemode_store};
use super::{CodemodeNestedCall, CodemodeNestedCallStatus, CodemodeToolOptions};
use crate::types::{
    CodemodeError, CodemodeResult, CodemodeTool, CodemodeToolContext, Deadline, ErrorKind,
    ExecuteOptions, SandboxOptions, ToolCallback,
};

/// Heap limit for the script's isolate: a runaway script must not grow until it takes the session
/// down (`CODEMODE_MEMORY_LIMIT_BYTES`, `execute.ts:55`). An overrun fails inside the script.
pub const CODEMODE_MEMORY_LIMIT_BYTES: u64 = 256 * 1024 * 1024;

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

/// Like the script's `text()`: strings as is, other values as compact JSON (`valueText`,
/// `execute.ts:236-239`).
fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => json_stringify(other),
    }
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

/// The failure text after `Script error:` (`formatError`, `execute.ts:247-258`): the script's own
/// stack (or `Name: message`), a timeout, abort or sandbox failure, then the call summary.
fn format_error(error: &CodemodeError, calls: &[CodemodeNestedCall]) -> String {
    let head = match error.kind {
        ErrorKind::Script => error.stack.clone().unwrap_or_else(|| {
            format!(
                "{}: {}",
                error.name.as_deref().unwrap_or("Error"),
                error.message
            )
        }),
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
    host: Option<&Arc<dyn CodemodeHost>>,
    recorder: &Arc<Recorder>,
) -> CodemodeTool {
    let (tool, host, recorder) = (Arc::clone(tool), host.cloned(), Arc::clone(recorder));
    let declaration = ToolDeclaration::new(tool.name()).with_description(sample);
    let execute: ToolCallback =
        Arc::new(move |args: Option<Value>, context: CodemodeToolContext| {
            let (tool, host, recorder) = (Arc::clone(&tool), host.clone(), Arc::clone(&recorder));
            let call: BoxFuture<'static, crate::types::ToolResult> = Box::pin(async move {
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

fn content_of(item: OutputItem) -> Content {
    match item {
        OutputItem::Text(text) => Content::text(text),
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
    let host = options.host.current();
    let recorder = Arc::new(Recorder::new(tool_call_id.clone(), on_update));

    let callable: Vec<Arc<dyn Tool>> = host
        .as_ref()
        .map(|host| callable_tools(&host.callable_tools()))
        .unwrap_or_default();
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
                    ),
                    None,
                ),
            )
        })
        .collect();
    let tools: Vec<CodemodeTool> = callable
        .iter()
        .map(|tool| {
            let sample = samples.get(tool.name()).map_or("", String::as_str);
            nested_tool(tool, sample, host.as_ref(), &recorder)
        })
        .collect();

    let mut globals = discovery_globals(Arc::new(callable), Arc::new(samples));
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
            deadline: parsed.options.timeout_ms.map_or(Deadline::Never, |ms| {
                Deadline::After(Duration::from_millis(ms))
            }),
            memory_limit_bytes: Some(CODEMODE_MEMORY_LIMIT_BYTES),
        })
        .map_err(|error| ToolError::new(error.to_string()))?;

    let store = match &host {
        Some(host) => read_codemode_store(&host.branch_custom_entries().await),
        None => serde_json::Map::new(),
    };
    let settled = sandbox
        .execute(
            &parsed.code,
            ExecuteOptions {
                cancel: Some(cancel),
                cancel_reason: Some(ABORT_REASON.to_owned()),
                deadline: None,
                store,
            },
        )
        .await;
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

    let ok = matches!(result, CodemodeResult::Completed { .. });
    let mut items: Vec<OutputItem>;
    match result {
        CodemodeResult::Completed {
            value,
            output,
            store_writes,
            ..
        } => {
            items = output;
            if let (Some(host), Some(entry)) =
                (&host, CodemodeStoreEntryData::from_writes(store_writes))
            {
                host.append_store_entry(entry).await.map_err(|error| {
                    ToolError::new(format!(
                        "Could not record the script's store() writes: {error}"
                    ))
                })?;
            }
            // pi extension: a returned value is appended like text().
            if let Some(value) = value {
                items.push(OutputItem::Text(value_text(&value)));
            }
        }
        CodemodeResult::Failed { error, output, .. } => {
            items = output;
            items.push(OutputItem::Text(format!(
                "Script error:\n{}",
                format_error(&error, &calls)
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
        items,
        parsed
            .options
            .max_output_tokens
            .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS),
        |text| spill_output(&std::env::temp_dir(), text),
    );
    // After truncation, which joins the text items and moves the images after them, so each path
    // stays next to its image and is never cut (`execute.ts:462-465` @v1.0.3).
    let temp_dir = std::env::temp_dir();
    let output = label_images(truncated.items, |mime_type, bytes| {
        save_image_output(&temp_dir, mime_type, bytes)
    })
    .map_err(|error| ToolError::new(error.to_string()))?;
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
