//! The demo extension's MODEL CALLS — the guest tier of pi `ctx.modelRegistry.complete()`,
//! `stream()` and `streamSimple()` (`core/model-registry.ts` @v1.0.4; EXT-086), live.
//!
//! Every command takes `<provider>/<id> <prompt>`, sends the prompt as one user message through the
//! session's providers, and reports what came back through `ui.notify` — the reply's text, or the
//! host's verbatim refusal, so the same command proves both directions of the `modelCalls` gate.
//!
//! Three more commands exist to prove what the host does when a GUEST misbehaves around an open
//! stream: one that never returns ([`HANG_COMMAND`]), one that panics ([`PANIC_COMMAND`]) and one
//! that forgets its handle ([`LEAK_COMMAND`]). None of them may leave a provider request running.

use serde_json::{Value, json};

use crate::{CommandDescriptor, ExtensionApi, ModelStream, message_text};

/// `/modeldemo <provider>/<id> <prompt>` — `models.complete`; notifies `model text: <reply>`.
pub const COMPLETE_COMMAND: &str = "modeldemo";
/// `/modelstreamdemo <provider>/<id> <prompt>` — `models.stream-simple`, drained event by event;
/// notifies `model stream polls: <p> deltas: <n> terminal: <type> text: <joined deltas>`.
pub const STREAM_COMMAND: &str = "modelstreamdemo";
/// `/modelstreamhang <provider>/<id>` — opens a stream, then never returns (the host's epoch
/// deadline traps it).
pub const HANG_COMMAND: &str = "modelstreamhang";
/// `/modelstreampanic <provider>/<id>` — opens a stream, then panics (a wasm trap).
pub const PANIC_COMMAND: &str = "modelstreampanic";
/// `/modelstreamleak <provider>/<id>` — opens a stream and returns WITHOUT closing it.
pub const LEAK_COMMAND: &str = "modelstreamleak";

/// Install the model-call commands.
pub fn install(api: &mut ExtensionApi) {
    api.register_command(
        COMPLETE_COMMAND,
        CommandDescriptor::new("Complete a prompt through the session's providers (demo)."),
        |args: &str, ctx: &crate::CommandCtx| {
            let report = match parse(args) {
                Err(e) => format!("model denied: {e}"),
                Ok((model, context)) => match ctx.models().complete(&model, &context, &Value::Null)
                {
                    Err(e) => format!("model denied: {e}"),
                    Ok(message) => describe_settled(&message),
                },
            };
            ctx.ui().notify(&report);
            Ok(Some(report))
        },
    );

    api.register_command(
        STREAM_COMMAND,
        CommandDescriptor::new("Stream a prompt through the session's providers (demo)."),
        |args: &str, ctx: &crate::CommandCtx| {
            let report = match parse(args) {
                Err(e) => format!("model denied: {e}"),
                Ok((model, context)) => {
                    match ctx.models().stream_simple(&model, &context, &Value::Null) {
                        Err(e) => format!("model denied: {e}"),
                        Ok(stream) => drain(stream),
                    }
                }
            };
            ctx.ui().notify(&report);
            Ok(Some(report))
        },
    );

    api.register_command(
        HANG_COMMAND,
        CommandDescriptor::new("Open a model stream and never return (demo)."),
        |args: &str, ctx: &crate::CommandCtx| {
            let stream = open(args, ctx)?;
            ctx.ui()
                .notify(&format!("model stream opened: {}", stream.handle()));
            // Hold the handle and spin past the host's epoch deadline: a guest that hangs.
            let _held = stream;
            loop {
                std::hint::spin_loop();
            }
        },
    );

    api.register_command(
        PANIC_COMMAND,
        CommandDescriptor::new("Open a model stream and panic (demo)."),
        |args: &str, ctx: &crate::CommandCtx| {
            let stream = open(args, ctx)?;
            ctx.ui()
                .notify(&format!("model stream opened: {}", stream.handle()));
            // A panic lowers to a wasm trap. `unreachable!` rather than `panic!` because the
            // workspace denies `clippy::panic`; the trap is identical. The handle is still held.
            let _held = stream;
            unreachable!("demo: a guest that panics with a model stream open")
        },
    );

    api.register_command(
        LEAK_COMMAND,
        CommandDescriptor::new("Open a model stream and forget it (demo)."),
        |args: &str, ctx: &crate::CommandCtx| {
            let stream = open(args, ctx)?;
            let handle = stream.handle();
            // Forget it: no `close-stream` ever reaches the host.
            std::mem::forget(stream);
            ctx.ui().notify(&format!("model stream leaked: {handle}"));
            Ok(Some(format!("leaked {handle}")))
        },
    );
}

/// `<provider>/<id> <prompt>` -> `(model address, one-message context)`.
fn parse(args: &str) -> Result<(Value, Value), String> {
    let args = args.trim();
    let (address, prompt) = args.split_once(char::is_whitespace).unwrap_or((args, "hi"));
    let (provider, id) = address
        .split_once('/')
        .ok_or_else(|| format!("expected `<provider>/<id> <prompt>`, got {args:?}"))?;
    Ok((
        json!({ "provider": provider, "id": id }),
        json!({ "messages": [{ "role": "user", "content": prompt.trim(), "timestamp": 0 }] }),
    ))
}

fn open(args: &str, ctx: &crate::CommandCtx) -> Result<ModelStream, String> {
    let (model, context) = parse(args)?;
    ctx.models().stream(&model, &context, &Value::Null)
}

/// One line for a settled `AssistantMessage`.
fn describe_settled(message: &Value) -> String {
    match message.get("stopReason").and_then(Value::as_str) {
        Some("error" | "aborted") => format!(
            "model failed: {}",
            message
                .get("errorMessage")
                .and_then(Value::as_str)
                .unwrap_or("(no message)")
        ),
        _ => format!("model text: {}", message_text(message)),
    }
}

/// Drain a stream batch by batch, counting the polls that carried events, the text deltas, and the
/// terminal — the evidence that it really arrived in pieces and really ended.
fn drain(mut stream: ModelStream) -> String {
    let (mut polls, mut deltas, mut terminal, mut text) =
        (0u32, 0u32, String::new(), String::new());
    loop {
        match stream.poll() {
            Err(e) => return format!("model stream failed: {e}"),
            Ok(None) => break,
            Ok(Some(batch)) => {
                if !batch.is_empty() {
                    polls += 1;
                }
                for event in batch {
                    match event.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            deltas += 1;
                            text.push_str(event.get("delta").and_then(Value::as_str).unwrap_or(""));
                        }
                        Some(kind @ ("done" | "error")) => terminal = kind.to_string(),
                        _ => {}
                    }
                }
            }
        }
    }
    stream.close();
    format!("model stream polls: {polls} deltas: {deltas} terminal: {terminal} text: {text}")
}
