//! RPC mode — the headless bidirectional stdio protocol (func-11 R-11-011…016; arch-11 §2.2/§3.5).
//!
//! A persistent line protocol other processes embed: incoming [`SessionCommand`] requests arrive as
//! strict-LF JSONL on a reader; the adapter drives the [`AgentSessionRuntime`] host (Pi
//! `rpc-mode.ts` `runtimeHost`) and emits [`RpcOut`] lines (a `response` per command + the full
//! agent/session event stream) on a writer. Both endpoints are parameters so tests drive an
//! in-memory reader/writer pair and the binary wires real stdio.
//!
//! ## Runtime host (R-11-019…023)
//! The session-replacing commands (`new_session`/`switch_session`/`fork`/`clone`) drive the
//! [`AgentSessionRuntime`] and then **rebind** — re-acquire the now-active session and re-subscribe
//! its event stream — exactly as Pi's `rebindSession()` (rpc-mode.ts:312-360). Every other command
//! operates on the active session (`runtime.session()`), the single integration seam.
//!
//! ## Framing (R-11-011)
//! Records are split on `\n` only (CRLF-tolerant: a trailing `\r` is stripped). We never rely on a
//! generic line reader that also splits on other Unicode separators inside JSON payloads.
//!
//! ## Streaming behaviour (R-11-016)
//! A `prompt` issued while the agent is already streaming MUST carry a `streamingBehavior`
//! (`steer` → queued after the current tool batch; `followUp` → after the agent goes idle); without
//! one it is rejected. While not streaming, `prompt` starts a fresh run. The active session's
//! `prompt_with` performs this preflight (the `input` ext event + steer/follow-up routing).

use std::collections::HashMap;
use std::sync::Arc;

use cyrup_session_svc::{
    AgentSession, AgentSessionEvent, AgentSessionRuntime, BashOptions, Content, EntryId,
    EventStream, ForkPosition, InputSource, NotifyKind, PromptAccepted, PromptOptions, UiEffect,
    UiKind, UiReply, UiRequest, UserInput,
};
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};

use crate::error::ModesError;

mod jsonl;
pub(crate) mod types;

use jsonl::{read_lines, write_out};
use types::queue_mode_str;
pub use types::{QueueModeArg, RpcOut, RpcResponse, SessionCommand};

// ---------------------------------------------------------------------------------------------
// Extension dialogs + run-loop support
// ---------------------------------------------------------------------------------------------

/// A pending extension dialog awaiting its `extension_ui_response` (mirrors Pi's
/// `pendingExtensionRequests` map, rpc-mode.ts:79-82). `kind` is retained so a `{value}`/`{confirmed}`/
/// `{cancelled}` response can be mapped back to the guest's expected reply shape. `select`'s WIT
/// return is now the chosen option STRING (world.wit:259), byte-for-byte the Pi wire `value`
/// (rpc-types.ts:273) — no index translation, so no options bag needs to be retained here.
struct PendingUi {
    kind: UiKind,
    reply: oneshot::Sender<UiReply>,
}

/// Shape a guest [`UiRequest`] into the exact Pi `extension_ui_request` wire object
/// (rpc-types.ts:230-265). `id` correlates the later `extension_ui_response`.
fn extension_ui_request_json(id: &str, req: &UiRequest) -> Value {
    // Serialize a `{timeout}` field only when the guest supplied one (Pi omits it otherwise).
    let with_timeout = |mut v: Value| -> Value {
        if let (Some(ms), Some(obj)) = (req.opts.timeout_ms, v.as_object_mut()) {
            obj.insert("timeout".to_string(), json!(ms));
        }
        v
    };
    match req.kind {
        // Pi `select(title, options, opts)` → `{method:"select", title, options, timeout?}`.
        UiKind::Select => with_timeout(json!({
            "type": "extension_ui_request",
            "id": id,
            "method": "select",
            "title": req.prompt,
            "options": req.options,
        })),
        // Pi `confirm(title, message, opts)` → `{method:"confirm", title, message, timeout?}` (L4
        // review §2.6): the cyrup WIT `confirm(prompt, message, opts-json)` now carries both strings
        // — `req.message` is the guest's actual message body, not a hard-coded empty string.
        UiKind::Confirm => with_timeout(json!({
            "type": "extension_ui_request",
            "id": id,
            "method": "confirm",
            "title": req.prompt,
            "message": req.message,
        })),
        // Pi `input(title, placeholder, opts)` → `{method:"input", title, placeholder?, timeout?}`
        // (rpc-types.ts:233-240; L4 review §2.7): `placeholder` is emitted only when the guest supplied
        // one, matching Pi omitting the wire field for `undefined`.
        UiKind::Input => with_timeout({
            let mut v = json!({
                "type": "extension_ui_request",
                "id": id,
                "method": "input",
                "title": req.prompt,
            });
            if let (Some(placeholder), Some(obj)) = (&req.placeholder, v.as_object_mut()) {
                obj.insert("placeholder".to_string(), json!(placeholder));
            }
            v
        }),
        // Pi `editor(title, prefill)` → `{method:"editor", title, prefill}` (rpc-mode.ts:253-268;
        // rpc-types.ts:241). The cyrup WIT `editor(title, initial)` (world.wit:267; L4 review §2)
        // carries both: `req.prompt` is the real title, `req.message` the seed text — same field
        // mapping `LiveHostServices::editor` uses (`cyrup-session-svc/src/host_services.rs`).
        UiKind::Editor => json!({
            "type": "extension_ui_request",
            "id": id,
            "method": "editor",
            "title": req.prompt,
            "prefill": req.message,
        }),
    }
}

/// Render a [`NotifyKind`] as Pi's exact wire string (`notify`'s `type` param, types.ts:135).
fn notify_kind_str(kind: NotifyKind) -> &'static str {
    match kind {
        NotifyKind::Info => "info",
        NotifyKind::Warning => "warning",
        NotifyKind::Error => "error",
    }
}

/// Shape a fire-and-forget [`UiEffect`] into the Pi `extension_ui_request` wire object, mirroring
/// `createExtensionUIContext`'s `notify`/`setStatus`/`setWidget`/`setTitle`/`setEditorText` handlers
/// (`rpc-mode.ts:149-241`), each of which emits the SAME envelope `confirm`/`input`/`select`/`editor`
/// use — `{type:"extension_ui_request", id, method, ...}` — but never registers the fresh `id` in
/// `pendingExtensionRequests`, since no response is ever awaited (Pi's own comment: "Fire and forget -
/// no response needed"). Returns `None` for `SetHeader`/`SetFooter`/`SetToolsExpanded` — Pi's real RPC
/// mode never forwards THOSE three over the wire either ("not supported in RPC mode - requires TUI
/// access" / "no TUI", rpc-mode.ts:209-215,296-298) — and, for the identical upstream reason, for the
/// four working-indicator effects (`rpc-mode.ts:179-193` @v0.84.2); see their arm below. [`run_rpc`]'s
/// effect-drain arm below only writes out when this returns `Some`.
pub(crate) fn extension_ui_effect_json(effect: &UiEffect) -> Option<Value> {
    Some(match effect {
        // Pi `notify(message, type)` → `{method:"notify", message, notifyType}` (rpc-mode.ts:149-157).
        UiEffect::Notify { message, kind } => json!({
            "type": "extension_ui_request",
            "id": new_request_id(),
            "method": "notify",
            "message": message,
            "notifyType": notify_kind_str(*kind),
        }),
        // Pi `setStatus(key, text?)` → `{method:"setStatus", statusKey, statusText}`
        // (rpc-mode.ts:163-172); `statusText` is OMITTED (not `null`) when `text` is `None`, matching
        // Pi's `JSON.stringify` dropping an `undefined` property.
        UiEffect::SetStatus { key, text } => {
            let mut v = json!({
                "type": "extension_ui_request",
                "id": new_request_id(),
                "method": "setStatus",
                "statusKey": key,
            });
            if let (Some(text), Some(obj)) = (text, v.as_object_mut()) {
                obj.insert("statusText".to_string(), json!(text));
            }
            v
        }
        // SEAM-011 — Pi `setWidget(key, content, options?)` → `{method:"setWidget", widgetKey,
        // widgetLines, widgetPlacement}` (rpc-mode.ts:193-206 @v0.83.0, pinned by the union member at
        // rpc-types.ts:264-271). cyrup emitted a single cyrup-invented `widget` blob because the WIT
        // collapsed pi's three arguments into one opaque payload; the WIT now carries them
        // separately, so this projects them onto pi's field names.
        //
        // Omission follows pi's `JSON.stringify`, which drops an `undefined` property:
        //
        // * `widgetLines` — absent when the extension passed `content: undefined`, i.e. asked for the
        //   widget to be REMOVED. Never `null`.
        // * `widgetPlacement` — pi emits `options?.placement`, so it is absent unless the extension
        //   supplied one. **CYRUP-DELTA**: cyrup's `WidgetPlacement` (cyrup-ext
        //   `host/services.rs:1341`) has no "unset" state — the WIT resolves an absent/malformed
        //   `placement` to the documented default `aboveEditor` (`extensions/types.ts:107-110`
        //   @v0.83.0) before the host sees it — so the default is emitted as an ABSENT key, which is
        //   what pi produces for every extension that does not set one and renders identically for
        //   the one that sets `"aboveEditor"` explicitly.
        UiEffect::SetWidget { widget } => {
            let mut v = json!({
                "type": "extension_ui_request",
                "id": new_request_id(),
                "method": "setWidget",
                "widgetKey": widget.get("key").and_then(Value::as_str).unwrap_or_default(),
            });
            if let Some(obj) = v.as_object_mut() {
                if let Some(lines) = widget.get("lines").filter(|l| !l.is_null()) {
                    obj.insert("widgetLines".to_string(), lines.clone());
                }
                if widget.get("placement").and_then(Value::as_str) == Some("belowEditor") {
                    obj.insert("widgetPlacement".to_string(), json!("belowEditor"));
                }
            }
            v
        }
        // Pi `setTitle(title)` → `{method:"setTitle", title}` (rpc-mode.ts:216-223).
        UiEffect::SetTitle { title } => json!({
            "type": "extension_ui_request",
            "id": new_request_id(),
            "method": "setTitle",
            "title": title,
        }),
        // Pi `setEditorText(text)`/`pasteEditorText(text)` (the latter falling back to the former,
        // rpc-mode.ts:230-232) → `{method:"set_editor_text", text}` (rpc-mode.ts:234-241) — note the
        // snake_case wire method name, unlike this function's other camelCase methods; `is_paste`
        // itself does not ride the wire (Pi's own `pasteToEditor` collapses onto the same handler).
        UiEffect::SetEditorText { text, .. } => json!({
            "type": "extension_ui_request",
            "id": new_request_id(),
            "method": "set_editor_text",
            "text": text,
        }),
        // Intentionally no wire shape — see this function's doc.
        UiEffect::SetHeader { .. }
        | UiEffect::SetFooter { .. }
        | UiEffect::SetToolsExpanded { .. } => {
            return None;
        }
        // TUI-030, the working-indicator family — NOT forwarded, and that is upstream's own
        // decision, not an omission. Pi's RPC `createExtensionUIContext` gives all four EMPTY
        // bodies (`modes/rpc/rpc-mode.ts:179-193` @v0.84.2 — every line in this arm is that tag,
        // NOT the @v0.83.0 the rest of this function cites; the block moved by one line between
        // them): "Working message not supported in RPC
        // mode - requires TUI loader access" (`:180`), the same for visibility (`:184`) and the
        // indicator (`:188`), and "Hidden thinking label not supported in RPC mode - requires TUI
        // message rendering access" (`:192`). There is no `RpcExtensionUIRequest` method for any of
        // them to ride, and inventing one would put a cyrup-only verb on a wire pi's clients parse.
        //
        // This is a deliberate `None`, spelled out because a silent omission HERE would be the same
        // defect this fix closes one layer out: the four now reach the interactive TUI, and an RPC
        // client that saw nothing would have no way to tell "dropped by design" from "never wired".
        UiEffect::SetWorkingMessage { .. }
        | UiEffect::SetWorkingVisible { .. }
        | UiEffect::SetWorkingIndicator { .. }
        | UiEffect::SetHiddenThinkingLabel { .. } => {
            return None;
        }
    })
}

/// The per-kind deny default a dialog resolves to when it is never genuinely answered — Pi's
/// `createDialogPromise` `defaultValue` argument (`select`/`input` → `undefined`, `confirm` → `false`,
/// rpc-mode.ts:136-149): an explicit `{cancelled:true}` reply and an unresponded timeout (§2.2) both
/// settle here. `abort`/`abort_retry` do NOT settle any pending dialog this way (or at all) — see
/// their arms in `handle()`.
fn default_ui_reply(kind: UiKind) -> UiReply {
    match kind {
        UiKind::Confirm => UiReply::Confirm(false),
        UiKind::Input | UiKind::Editor | UiKind::Select => UiReply::Text(None),
    }
}

/// Map an `extension_ui_response` body onto the guest's expected [`UiReply`] for `pending` (Pi
/// `parseResponse`, rpc-mode.ts:137-149,257-264). A `{cancelled:true}` yields the per-kind default; a
/// `{confirmed}` a confirm; a `{value}` maps straight to text (input/editor/select) — Pi's
/// `select(...): Promise<string|undefined>` (types.ts:127) passes the chosen STRING straight through
/// to the guest, with NO index translation.
fn map_ui_response(pending: &PendingUi, body: &Value) -> UiReply {
    if body.get("cancelled").and_then(Value::as_bool) == Some(true) {
        return default_ui_reply(pending.kind);
    }
    match pending.kind {
        UiKind::Confirm => UiReply::Confirm(
            body.get("confirmed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
        UiKind::Input | UiKind::Editor | UiKind::Select => {
            UiReply::Text(body.get("value").and_then(Value::as_str).map(str::to_owned))
        }
    }
}

/// The disposition of a dispatched command: just the correlated [`RpcResponse`].
///
/// It used to carry a `rebind: bool` derived from the command NAME. That is exactly SEAM-022: the
/// active session is replaced by the RUNTIME, and only a fraction of the replacements are named by
/// an RPC verb — a loaded extension calling `ctx.newSession()`/`ctx.fork()`/`ctx.switchSession()`/
/// `ctx.reload()` arrives as an ordinary `{"type":"prompt","message":"/mycmd"}`. The loop now
/// observes [`AgentSessionRuntime::watch_generation`] instead, which `install_inner` bumps on EVERY
/// replacement path — the same signal `cyrup-tui`'s run loop already rebinds on.
struct Dispatched {
    response: RpcResponse,
}

/// The host-side `rebindSession` (Pi rpc-mode.ts:316-360, registered at :312-314 and invoked by
/// `finishSessionReplacement`, agent-session-runtime.ts:187-190).
///
/// Re-acquires the now-active session, re-subscribes its event stream (the prior subscription was
/// terminated with `SessionReplaced`, R-11-021), and re-installs the three sinks Pi re-passes to
/// `bindExtensions` on every rebind — the dialog channel, the fire-and-forget effect channel and
/// the `onError` fault listener — because a replacement brings a fresh `LiveHostServices` +
/// extension host. `in_flight` is cleared: the replaced session's run (if any) was disposed and its
/// `agent_settled` will never arrive.
async fn rebind_session(
    runtime: &AgentSessionRuntime,
    session: &mut Arc<AgentSession>,
    events: &mut EventStream<AgentSessionEvent>,
    sinks: &LoopSinks,
    in_flight: &mut bool,
) {
    *session = runtime.session().await;
    *events = session.subscribe();
    session
        .services()
        .host_services
        .set_ui_sink(sinks.ui.clone());
    session
        .services()
        .host_services
        .set_ui_effect_sink(sinks.ui_effect.clone());
    session
        .services()
        .ext_host
        .add_error_listener(error_listener(sinks.error.clone()));
    *in_flight = false;
}

/// The three loop-owned channels every (re)bind installs onto the active session.
struct LoopSinks {
    ui: mpsc::UnboundedSender<UiRequest>,
    ui_effect: mpsc::UnboundedSender<UiEffect>,
    error: mpsc::UnboundedSender<Value>,
}

// ---------------------------------------------------------------------------------------------
// Run loop
// ---------------------------------------------------------------------------------------------

/// Serve the RPC protocol over `reader` (commands in) and `writer` (responses + events out), driving
/// the [`AgentSessionRuntime`] host.
///
/// Reads strict-LF JSONL requests, drives the active session, and streams every
/// [`AgentSessionEvent`] (agent + session-level) back as it occurs. A session-replacing command
/// rebinds: the active session + its event subscription are re-acquired from the runtime (Pi
/// `rebindSession`). Returns once the reader reaches EOF *and* no run is in flight *and* no
/// concurrently-dispatched command is still running. A dedicated reader task keeps line parsing
/// cancel-safe against the concurrent event stream.
///
/// ## Command concurrency (Pi `void handleInputLine`, rpc-mode.ts:782; G1)
/// Blocking commands (`bash`/`compact`/`export_html`) and session-replacing ones
/// (`new_session`/`switch_session`/`fork`/`clone`) are dispatched CONCURRENTLY (a per-command future
/// in a `FuturesUnordered`), so a subsequent `abort`/`abort_bash` line is read + serviced WHILE the
/// blocking command runs and can actually interrupt it — mirroring Node interleaving Pi's fire-and-
/// forget `handleInputLine` promise chains. Fast run-control commands (`prompt`/`steer`/`follow_up`)
/// stay inline (they own `in_flight`). Contained extension faults surface as `extension_error` lines
/// (Pi `onError`, rpc-mode.ts:347-349; G2).
///
/// ## Output decoupling (Pi `writeRawStdout`, output-guard.ts:85-90; G3)
/// Pi's `output()` is FIRE-AND-FORGET: `writeRawStdout` appends the chunk to a
/// `rawStdoutWriteTail` promise chain and returns synchronously (`output-guard.ts:85-90`), so the
/// RPC host's command handling never sits on the actual `write(2)`. Cyrup awaited every emission
/// **inline inside the command `select!`** — `write_out(writer, …).await?` in eight arms — which
/// meant a client that stopped reading its end of the pipe filled the socket buffer, parked the
/// whole loop inside `write_all`, and made `abort` / `abort_bash` / a guest's `ctx.shutdown()`
/// structurally undeliverable: no further stdin line could even be *read*, let alone serviced. The
/// commands that exist to rescue a wedged session were the exact ones a wedged client disabled.
///
/// The writer is therefore driven by [`write_pump`], a SEPARATE future composed here with
/// [`rpc_driver`] (same task — no `Send`/`'static` bound is added to `W`, and `writer` stays a
/// `&mut`). The driver only enqueues onto an unbounded channel, so no arm can ever block on the
/// peer. `write_pump` owns the writer for its whole lifetime and is never dropped mid-`write_all`,
/// which a `select!` arm holding the writer *would* be — that would truncate a JSONL line and
/// corrupt the stream. Because the two run concurrently, output ordering is unchanged (one FIFO
/// channel, one writer).
///
/// Backpressure is unaffected: it lives where Pi puts it — on the AGENT, via
/// `cyrup-session-svc`'s bounded-1024 awaited subscriber channel (Pi's
/// `session.agent.subscribe(async () => await waitForRawStdoutBackpressure())`,
/// rpc-mode.ts:360-362) — not on the command loop, whose emissions Pi never awaits either.
/// Shutdown still flushes everything: the driver returning drops the sender, the pump drains the
/// remaining queue and is awaited before `run_rpc` returns (Pi's `await flushRawStdout()`,
/// rpc-mode.ts:737).
pub async fn run_rpc<R, W>(
    runtime: &AgentSessionRuntime,
    reader: R,
    writer: &mut W,
) -> Result<(), ModesError>
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    // SEAM-033 — pi's RPC host announces the session ITSELF, from `rpc-mode.ts:319`, not from
    // `createAgentSessionRuntime` (which never emits `session_start`,
    // `agent-session-runtime.ts:414-432`). The distinction is load-bearing: `main.ts` applies
    // `--name` (`:650`) and the scoped `--models` (`:742-750`) BETWEEN building the session and
    // running the mode, so announcing at construction time showed every `session_start` handler a
    // session with no display name and the pre-scope model. Idempotent per session
    // (`AgentSession::emit_session_start` latches), so a host that already announced is unaffected.
    runtime.session().await.bind_extensions().await;

    let (out, out_rx) = mpsc::unbounded_channel::<RpcOut>();
    let mut pump = std::pin::pin!(write_pump(writer, out_rx));
    // Set only when the pump finished FIRST, which can only mean a write error (it otherwise runs
    // until the driver drops the sender). Distinguishes "already completed" from "still to await".
    let mut pump_failed: Option<Result<(), ModesError>> = None;
    let driven = {
        let mut driver = std::pin::pin!(rpc_driver(runtime, reader, out));
        tokio::select! {
            res = &mut driver => res,
            res = &mut pump => {
                pump_failed = Some(res);
                Ok(())
            }
        }
    };
    // Leaving the block dropped `driver` and with it the sender, so the pump's channel is now
    // closed and it will return as soon as the backlog is on the wire.
    //
    // The driver's outcome is HELD, not `?`-ed, until that flush has happened: it may be the reader's
    // I/O error, and the responses/events the driver drained onto the queue on its way out are still
    // in flight behind it. Reporting first would drop them. On the (unchanged) success path this is
    // the same sequence as before — pump to completion, then surface whichever half failed, driver
    // first since a severed input is the root cause of anything the writer then saw.
    let pumped = match pump_failed {
        Some(res) => res,
        None => pump.await,
    };
    driven?;
    pumped
}

/// Own `writer` for the whole run and drain [`run_rpc`]'s emission queue onto it — cyrup's spelling
/// of Pi's `rawStdoutWriteTail` chain (`output-guard.ts:11`, `:85-90`), which serializes every
/// `writeRawStdout` behind the previous one while the caller returns immediately.
///
/// Returns `Ok(())` when the sender is dropped (the driver finished) and the queue is empty — i.e.
/// once everything the session ever emitted is flushed, Pi's `await flushRawStdout()` on the
/// shutdown path (rpc-mode.ts:737). A write error ends the pump and is surfaced by `run_rpc`.
///
/// This must be a long-lived future rather than a `select!` arm: `AsyncWriteExt::write_all` is not
/// cancel-safe, so an arm dropped mid-write would leave a half-written JSONL line on the stream.
async fn write_pump<W>(
    writer: &mut W,
    mut queue: mpsc::UnboundedReceiver<RpcOut>,
) -> Result<(), ModesError>
where
    W: AsyncWrite + Unpin,
{
    while let Some(record) = queue.recv().await {
        write_out(writer, &record).await?;
    }
    Ok(())
}

/// The command/event loop proper: everything [`run_rpc`] does except touch the writer. Emissions go
/// to `out` (never awaited — see `run_rpc`'s "Output decoupling"), which [`write_pump`] drains
/// concurrently.
async fn rpc_driver<R>(
    runtime: &AgentSessionRuntime,
    reader: R,
    out: mpsc::UnboundedSender<RpcOut>,
) -> Result<(), ModesError>
where
    R: AsyncBufRead + Unpin + Send + 'static,
{
    // The active session + its event subscription (re-acquired on every replacement).
    let mut session = runtime.session().await;
    let mut events = session.subscribe();

    // The synchronous extension-dialog sink (mode #4): a loaded guest's `ui.{confirm,input,select,
    // editor}` capability blocks on a one-shot while this loop emits an `extension_ui_request` and
    // awaits the client's `extension_ui_response` (Pi `createExtensionUIContext`, rpc-mode.ts:135-160).
    // Installed on the active session's `LiveHostServices` (re-installed on every rebind, since a
    // replacement brings a fresh backend). `pending` mirrors Pi's `pendingExtensionRequests`.
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<UiRequest>();
    session.services().host_services.set_ui_sink(ui_tx.clone());
    let mut pending: HashMap<String, PendingUi> = HashMap::new();

    // The fire-and-forget extension-effect drain: a loaded guest's `ui.{notify,set-status,set-widget,
    // set-header,set-footer,set-title,set-editor-text,paste-editor-text,set-tools-expanded}`
    // capability lands here with NO reply expected (Pi `createExtensionUIContext`'s `notify`/
    // `setStatus`/`setWidget`/`setTitle`/`setEditorText` handlers, rpc-mode.ts:149-241). Installed +
    // re-installed on rebind exactly like `ui_tx` above.
    let (ui_effect_tx, mut ui_effect_rx) = mpsc::unbounded_channel::<UiEffect>();
    session
        .services()
        .host_services
        .set_ui_effect_sink(ui_effect_tx.clone());

    // The contained-extension-fault sink (Pi `bindExtensions({ onError })`, rpc-mode.ts:347-349):
    // every guest handler fault the dispatcher contains + skips (R-08-036) is surfaced to the client
    // as one `extension_error` line on stdout. The listener bridges the dispatcher's synchronous
    // `onError` fan-out (which may fire on any worker thread mid-run) into this loop via a channel;
    // (re)installed on the active session's extension host here and again on every rebind, exactly as
    // Pi re-binds `onError` inside `rebindSession()`.
    let (error_tx, mut error_rx) = mpsc::unbounded_channel::<Value>();
    session
        .services()
        .ext_host
        .add_error_listener(error_listener(error_tx.clone()));
    let sinks = LoopSinks {
        ui: ui_tx,
        ui_effect: ui_effect_tx,
        error: error_tx,
    };

    // SEAM-022: the replacement signal. `AgentSessionRuntime::install_inner` bumps this watch on
    // EVERY path that swaps the active session — the RPC verbs `new_session`/`switch_session`/
    // `fork`/`clone`, AND a loaded extension's `ctx.newSession()`/`ctx.fork()`/`ctx.switchSession()`/
    // `ctx.reload()`, which arrive as an ordinary `prompt` line and therefore cannot be recognized
    // from the command name. This is cyrup's spelling of Pi handing the runtime a `rebindSession`
    // callback that `finishSessionReplacement` invokes (agent-session-runtime.ts:187-190); the TUI
    // run loop already rebinds off this same watch (`cyrup-tui/src/app.rs`).
    let mut gen_rx = runtime.watch_generation();

    // Dedicated reader task → mpsc of raw JSONL lines (strict LF framing; cancel-safe vs. events).
    // The handle is KEPT (not detached) so [`read_lines`]'s own failure — a broken pipe, an `EIO` on a
    // serial/socket fd, a supervisor tearing the input fd down — can be surfaced instead of being
    // indistinguishable from the client closing stdin cleanly.
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(64);
    let mut reader_task = Some(tokio::spawn(read_lines(reader, cmd_tx)));

    let mut reader_open = true;
    // Whether the reader task ENDED BY ITSELF (EOF, receiver dropped, or a read error). The command
    // channel closing is the exact signal of that, since the task owns the only sender. Kept distinct
    // from `reader_open`, which the `ctx.shutdown()` path below ALSO clears while the task is still
    // parked on input — joining the handle is only safe when this flag is set.
    let mut reader_ended = false;
    // True from the moment a run is accepted until its `agent_settled` is observed (SEAM-005 — the
    // whole run, not just the first agent loop).
    let mut in_flight = false;
    // Latches when a loaded extension called `ctx.shutdown()` (Pi's `shutdownRequested` closure
    // variable, set by the `shutdownHandler` bound in `bindExtensions`, rpc-mode.ts:344-346). It is
    // sampled every loop iteration — not only at the settle point — because a session REPLACEMENT
    // (`ctx.newSession()` from another handler) brings a fresh `AgentSession` with a fresh flag, and
    // Pi's closure variable would have survived that. Acted on at the next
    // [`shutdown_checkpoint`], which is Pi's `checkShutdownRequested` call sites (:357 and :786).
    let mut shutdown_requested = false;
    // Whether the loop has reached a point at which Pi calls `checkShutdownRequested()`. Pi has TWO
    // such points, not one: the `agent_settled` arm (rpc-mode.ts:355-358) AND the tail of every
    // handled command (`await checkShutdownRequested()`, rpc-mode.ts:786). The second is
    // load-bearing — Pi's own canonical example of `ctx.shutdown()`
    // (`examples/extensions/shutdown-command.ts`) is a `/quit` COMMAND that exits with no agent run
    // ever having happened, so gating on a settle alone would make that command silently do nothing.
    let mut shutdown_checkpoint = false;

    // In-flight dispatches of the potentially-BLOCKING and session-replacing commands, driven
    // CONCURRENTLY with continued input reading so an `abort`/`abort_bash` line arriving mid-command
    // is read + serviced WHILE that command is still running — Pi's fire-and-forget `void
    // handleInputLine(line)` (rpc-mode.ts:782) interleaves the two promise chains; without this the
    // command that exists to interrupt a hung `bash` structurally cannot be delivered (G1). The fast
    // run-control commands (`prompt`/`steer`/`follow_up`, which only preflight/enqueue then return
    // while the agent streams via `events`) stay INLINE: they own the `in_flight` flag, and running
    // them inline keeps that flag set before the event stream is next polled (no set-true / observe-
    // `agent_end` reordering race that a drain-time set would introduce).
    let mut dispatches = FuturesUnordered::new();

    loop {
        tokio::select! {
            maybe_line = cmd_rx.recv(), if reader_open => {
                match maybe_line {
                    Some(line) => {
                        // Intercept an `extension_ui_response` BEFORE command dispatch (Pi
                        // `handleInputLine`, rpc-mode.ts:739-753): look up the pending dialog by `id`,
                        // resolve its one-shot, and never route it to the command switch. The
                        // intercept is decided by the `type` tag ALONE and always `continue`s
                        // (SEAM-086) — an envelope with a missing/non-string `id`, or one whose id
                        // matches no pending dialog, is swallowed with no output line, exactly as
                        // pi's unconditional `return` does (rpc-mode.ts:763-777 @v0.83.0).
                        if let Some(id) = extension_ui_response_id(&line) {
                            if let Some(p) = id.and_then(|id| pending.remove(&id)) {
                                let body: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                                let reply = map_ui_response(&p, &body);
                                let _ = p.reply.send(reply);
                            }
                            continue;
                        }
                        // SEAM-022: a replacement may have landed since this arm last ran (the
                        // generation arm below is one of several ready branches `select!` picks
                        // between at random). Settle it BEFORE the line is serviced so the command
                        // reaches the session the runtime is actually serving, never the disposed
                        // one — Pi orders it the same way, awaiting `rebindSession` inside
                        // `finishSessionReplacement` before the host handles anything else.
                        if gen_rx.has_changed().unwrap_or(false) {
                            gen_rx.mark_unchanged();
                            rebind_session(
                                runtime, &mut session, &mut events, &sinks, &mut in_flight,
                            ).await;
                        }
                        if is_inline_command(&line) {
                            // Run-control commands are fast (preflight/enqueue) and own `in_flight`;
                            // dispatch inline so the flag is set before `events` is next polled.
                            // They can still REPLACE the session — a `/slash` extension command
                            // arrives as a `prompt` and its handler may call `ctx.newSession()` —
                            // which the generation check at the top of the next iteration settles.
                            let dispatched =
                                dispatch(runtime, &session, &line, &mut in_flight).await;
                            let _ = out.send(RpcOut::Response(dispatched.response));
                            // Pi's post-command `await checkShutdownRequested()` (rpc-mode.ts:786).
                            shutdown_checkpoint = true;
                        } else {
                            // Everything else — including the blocking `bash`/`compact`/`export_html`
                            // and the session-replacing `new_session`/`switch_session`/`fork`/`clone`
                            // — dispatches concurrently so a following `abort`/`abort_bash` is not
                            // queued behind it. The response is written when the future completes,
                            // in the `select_next_some` arm below; any resulting replacement is
                            // picked up by the generation arm.
                            dispatches.push(dispatch_owned(runtime, Arc::clone(&session), line));
                        }
                    }
                    None => {
                        // The sender is gone: `read_lines` returned. Its outcome (clean EOF vs. a
                        // read error) is read from the join handle at the shutdown break below.
                        reader_open = false;
                        reader_ended = true;
                    }
                }
            }
            Some(dispatched) = dispatches.next(), if !dispatches.is_empty() => {
                // A concurrent command finished: emit its correlated response. Responses are written
                // only here + the inline arm, both on this single loop task, so the writer is never
                // shared across the concurrent dispatch futures (they only compute a response). If
                // the command replaced the active session, the generation arm rebinds.
                let _ = out.send(RpcOut::Response(dispatched.response));
                // Pi's post-command `await checkShutdownRequested()` (rpc-mode.ts:786) — the
                // concurrent-dispatch twin of the inline arm above.
                shutdown_checkpoint = true;
            }
            Ok(()) = gen_rx.changed() => {
                // SEAM-022: the runtime replaced the active session — Pi's `rebindSession()`
                // (rpc-mode.ts:316-360), which its runtime invokes from `finishSessionReplacement`
                // for all six replacement paths. Fires for an extension-triggered swap just as much
                // as for an RPC verb, and is what keeps the loop from servicing later commands (and
                // reading later events) through the disposed session.
                rebind_session(runtime, &mut session, &mut events, &sinks, &mut in_flight).await;
            }
            Some(wire) = error_rx.recv() => {
                // A dispatcher-contained extension fault: surface it as an `extension_error` line
                // (Pi `onError` → `output({type:"extension_error", …})`, rpc-mode.ts:347-349).
                let _ = out.send(RpcOut::ExtensionError(wire));
            }
            Some(req) = ui_rx.recv() => {
                // A guest opened a dialog: allocate a correlation id, emit the Pi `extension_ui_request`
                // on stdout, and stash the one-shot until the client's `extension_ui_response` arrives.
                // First prune any entry whose `reply` half [`LiveHostServices::ui_roundtrip`] already
                // gave up on (a §2.2 timeout fired, or the guest's reply channel was otherwise dropped)
                // — cheap (bounded by the open-dialog count) and keeps a long-running session's `pending`
                // map from growing unboundedly across many timed-out dialogs.
                pending.retain(|_, p| !p.reply.is_closed());
                let id = new_request_id();
                let wire = extension_ui_request_json(&id, &req);
                pending.insert(id, PendingUi { kind: req.kind, reply: req.reply });
                let _ = out.send(RpcOut::ExtensionUiRequest(wire));
            }
            Some(effect) = ui_effect_rx.recv() => {
                // A guest pushed a fire-and-forget ui effect: emit it immediately (no correlation
                // bookkeeping — nothing ever replies) — Pi's `notify`/`setStatus`/`setWidget`/
                // `setTitle`/`setEditorText` RPC handlers each just call `output(...)` inline
                // (rpc-mode.ts:149-241). `setHeader`/`setFooter`/`setToolsExpanded` are dropped here
                // (Pi doesn't forward them over RPC either — see `extension_ui_effect_json`'s doc).
                if let Some(wire) = extension_ui_effect_json(&effect) {
                    let _ = out.send(RpcOut::ExtensionUiRequest(wire));
                }
            }
            maybe_ev = events.next() => {
                if let Some(ev) = maybe_ev {
                    // SEAM-005: a run is "in flight" until it SETTLES, not until its first
                    // `agent_end`. `agent_end` fires once per agent loop, so an auto-retry / post-run
                    // compaction / queued continuation produces another one — clearing the flag there
                    // let the EOF shutdown check below fire mid-run and cut the stream (and, on a
                    // fast turn, race the trailing `agent_settled` line off the wire entirely).
                    // `agent_settled` is emitted exactly once, at the end of the whole run, on every
                    // path — including a failed `agent.prompt`, whose settle still runs.
                    if matches!(ev, AgentSessionEvent::AgentSettled) {
                        in_flight = false;
                    }
                    // SEAM-005 + EXT-005: a loaded extension's `ctx.shutdown()` is honoured at the
                    // SETTLE point, never mid-run — Pi checks `shutdownRequested` in exactly this
                    // arm (`if (event.type === "agent_settled") void checkShutdownRequested()`,
                    // rpc-mode.ts:355-358). Waiting for `agent_settled` rather than `agent_end` is
                    // load-bearing: `agent_end` fires again after an auto-retry or a post-run
                    // compaction, so exiting there would cut a run that is still going.
                    let settled = matches!(ev, AgentSessionEvent::AgentSettled);
                    // Only members of pi's own `AgentSessionEvent` union reach stdout. The internal
                    // `SessionReplaced` terminal is a rebind signal, not a pi event; `model_changed`
                    // (SEAM-080) and `session_start`/`session_shutdown` (SEAM-081) are cyrup
                    // super-set members that pi's `session.subscribe` can never deliver. See
                    // [`crate::is_upstream_wire_event`].
                    if crate::is_upstream_wire_event(&ev) {
                        let _ = out.send(RpcOut::Event(Box::new(ev)));
                    }
                    if settled {
                        shutdown_checkpoint = true;
                    }
                }
            }
        }

        shutdown_requested |= session.shutdown_requested();
        if shutdown_requested && shutdown_checkpoint && !in_flight {
            // Pi `checkShutdownRequested` (rpc-mode.ts:363-372): stop reading, let the loop's own
            // teardown flush whatever is buffered, and return.
            reader_open = false;
        }

        if !reader_open && !in_flight && dispatches.is_empty() {
            // The reader is at EOF, no agent run is in flight, and every concurrently-dispatched
            // command has completed (a still-running `bash` at EOF is awaited here, not cut off).
            // Flush any events already buffered on the channel + any extension_error queued during
            // shutdown, then shut down cleanly.
            while let Some(Some(ev)) = events.next().now_or_never() {
                // Same rule as the live arm above (SEAM-080/SEAM-081).
                if crate::is_upstream_wire_event(&ev) {
                    let _ = out.send(RpcOut::Event(Box::new(ev)));
                }
            }
            while let Ok(wire) = error_rx.try_recv() {
                let _ = out.send(RpcOut::ExtensionError(wire));
            }
            // Only NOW — after everything already queued has been handed to `write_pump` — look at
            // how the reader ended. A genuine transport failure is not an EOF: propagating it is what
            // lets `cyrup --mode rpc` exit non-zero (and an embedder awaiting `run_rpc` learn the
            // session was cut off) instead of returning `Ok(())` as if the client had closed stdin.
            // Awaiting cannot block here: `reader_ended` is set from the channel-closed arm ALONE, so
            // the task has already returned. The `ctx.shutdown()` path above, where the task is still
            // parked on input, never sets it — hence no deadlock.
            if reader_ended && let Some(handle) = reader_task.take() {
                match handle.await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => return Err(ModesError::Io(e)),
                    // Aborted or panicked: there is no `io::Error` to report and the input stream is
                    // over either way, so fall through to the clean shutdown.
                    Err(_) => {}
                }
            }
            break;
        }
    }

    // The reader task ends on its own at EOF — in which case it was already taken + joined above.
    // This reaps the other case: the `ctx.shutdown()` path, where it is still parked on input.
    if let Some(handle) = reader_task {
        handle.abort();
    }
    Ok(())
}

/// Whether `line` is a fast run-control command (`prompt`/`steer`/`follow_up`) that must be
/// dispatched INLINE. These own the `in_flight` flag and return quickly (after preflight/enqueue),
/// so running them inline keeps `in_flight` set before the event stream is next polled — avoiding a
/// set-true / observe-`agent_end` reordering race that a concurrent, drain-time set would introduce.
/// Every other command (including the blocking `bash`/`compact`/`export_html` and the session-
/// replacing ones) is dispatched concurrently so `abort`/`abort_bash` can interrupt it (G1). A line
/// that is not parseable / has no `type` is treated as non-inline: its exact parse/unknown error
/// response is still produced by [`dispatch`] on the concurrent path.
fn is_inline_command(line: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .ok()
        .as_ref()
        .and_then(|v| v.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|t| matches!(t, "prompt" | "steer" | "follow_up"))
}

/// Owned-capture wrapper around [`dispatch`] for the concurrent path: holds its own `Arc` clone of
/// the active session (so the loop may reassign/rebind `session` without invalidating in-flight
/// dispatches) and the raw line. Concurrent commands are never the run-control kind, so their
/// `in_flight` mutation is inert — a throwaway flag is threaded through.
async fn dispatch_owned(
    runtime: &AgentSessionRuntime,
    session: Arc<AgentSession>,
    line: String,
) -> Dispatched {
    let mut ignored = false;
    dispatch(runtime, &session, &line, &mut ignored).await
}

/// Build the `onError` bridge (Pi `bindExtensions({ onError })`, rpc-mode.ts:347-349): a dispatcher
/// [`cyrup_ext::ErrorListener`] that shapes each contained [`cyrup_ext::ExtensionError`] into the Pi
/// `extension_error` wire object (`{type, extensionPath, event, error}`) and forwards it to the run
/// loop over `tx`. `Send + Sync` so the dispatcher can invoke it from any worker thread mid-run.
fn error_listener(tx: mpsc::UnboundedSender<Value>) -> cyrup_ext::ErrorListener {
    Arc::new(move |err: &cyrup_ext::ExtensionError| {
        let _ = tx.send(json!({
            "type": "extension_error",
            "extensionPath": err.extension.as_str(),
            "event": err.event,
            "error": err.error,
        }));
    })
}

/// Decode one request line and apply it, in the same **staged** order Pi's `handleInputLine` +
/// `handleCommand` use (rpc-mode.ts:723-773, 382-689). Side effect: a freshly-started run sets
/// `in_flight`.
///
/// 1. **Parse** the line as JSON (`JSON.parse`, rpc-mode.ts:726). A syntax error is *not* a command:
///    Pi emits `error(undefined, "parse", "Failed to parse command: …")` with **no** id — `JSON.parse`
///    itself failed, so there is no object to recover an id from (rpc-mode.ts:728-734). #6.
/// 2. Recover the `id` from the parsed object (`const id = command.id`, rpc-mode.ts:383), preserved
///    exactly as sent — string **or** number (#10); `null`/absent → no id.
/// 3. **Deserialize** the command. An unknown `type` tag hits Pi's `switch` default:
///    `error(id, command.type, "Unknown command: <type>")` echoing the **real** type (rpc-mode.ts:686-689).
///    #7. A recognized type whose payload is missing/wrong-typed a required field surfaces as a runtime
///    error under `handleCommand`, caught as `error(id, command.type, <message>)` — again the **real**
///    command name, not `"unknown"` (rpc-mode.ts:755-772). #8.
async fn dispatch(
    runtime: &AgentSessionRuntime,
    session: &AgentSession,
    line: &str,
    in_flight: &mut bool,
) -> Dispatched {
    // (1) Parse the raw line. A malformed line is Pi's `"parse"` error with NO id.
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Dispatched {
                response: RpcResponse::err("parse", None, format!("Failed to parse command: {e}")),
            };
        }
    };

    // (2) Recover the id and the `type` discriminant before consuming `value`. The id is preserved
    // as-sent (string or number); the type string is what Pi echoes on the error paths.
    let raw_id = value.get("id").filter(|id| !id.is_null()).cloned();
    let type_str = value.get("type").and_then(Value::as_str).map(str::to_owned);

    // (3) Deserialize the recognized command's payload.
    match serde_json::from_value::<SessionCommand>(value) {
        // Recognized-shape line with an unknown `type` tag (`#[serde(other)]`): echo the real type.
        Ok(SessionCommand::Unknown) => {
            let name = type_str.unwrap_or_default();
            let message = format!("Unknown command: {name}");
            Dispatched {
                response: RpcResponse::err(name, raw_id, message),
            }
        }
        Ok(cmd) => {
            // Whether this command REPLACED the active session is deliberately not inferred here
            // (SEAM-022): the runtime announces every replacement on its generation watch, which the
            // run loop observes directly.
            Dispatched {
                response: handle(runtime, session, cmd, raw_id, in_flight).await,
            }
        }
        // A known `type` whose payload failed validation (missing/wrong-typed required field): echo
        // the real command name + the runtime error, NOT `"unknown"`. A missing/`null` `type` tag
        // (serde: "missing field `type`") has no command name to echo — fall back to Pi's default
        // `Unknown command` shaping so it still correlates.
        Err(e) => match type_str {
            Some(name) => Dispatched {
                response: RpcResponse::err(name, raw_id, e.to_string()),
            },
            None => Dispatched {
                response: RpcResponse::err(String::new(), raw_id, "Unknown command: undefined"),
            },
        },
    }
}

/// Set the loop's "a run is in flight" latch ONLY when the session really has work in flight
/// (SEAM-021).
///
/// The latch exists so [`run_rpc`]'s EOF exit waits for the trailing `agent_settled` instead of
/// cutting a live run (SEAM-005) — which means it may only be set when an `agent_settled` is
/// actually coming. `steer`/`follow_up` do NOT start a run: [`AgentSession::steer`]/
/// [`AgentSession::follow_up`] push onto the pending queues and emit `queue_update` (Pi
/// `_queueSteer`/`_queueFollowUp`, agent-session.ts:1249/1266), and Pi's own `case "steer"` /
/// `case "follow_up"` arms (rpc-mode.ts:417-425) carry no in-flight bookkeeping at all. Latching
/// unconditionally there wedged the loop forever on an idle session: nothing would ever emit the
/// `agent_settled` that clears it, so `!reader_open && !in_flight && …` never became true, `run_rpc`
/// never returned, and `run_rpc_dispatch`'s `runtime.dispose()` — hence `session_shutdown` — never
/// ran (SEAM-002's RPC leg).
///
/// [`AgentSession::is_idle`] is the same two-latch readback `wait_for_idle` waits on (the post-run
/// driver plus the agent's own run, Pi `isIdle`, agent-session.ts:759), so a steer that lands ON a
/// live run still holds the EOF exit open — which is the case the latch was written for.
fn latch_if_running(session: &AgentSession, in_flight: &mut bool) {
    if !session.is_idle() {
        *in_flight = true;
    }
}

#[allow(clippy::too_many_lines)] // a faithful 1:1 of Pi's `handleCommand` switch (rpc-mode.ts:385).
async fn handle(
    runtime: &AgentSessionRuntime,
    session: &AgentSession,
    cmd: SessionCommand,
    raw_id: Option<Value>,
    in_flight: &mut bool,
) -> RpcResponse {
    // Pi reads the id once at the top of `handleCommand` (`const id = command.id`, rpc-mode.ts:383);
    // cyrup recovered it in `dispatch` and threads it in as `raw_id`. Each arm clones it into the
    // reply (string or number, preserved as-sent).
    match cmd {
        // -------------------------------------------------------------- Prompting ----
        SessionCommand::Prompt {
            message,
            images,
            streaming_behavior,
        } => {
            let id = raw_id.clone();
            let input = user_input(message, images);
            match session
                .prompt_with(input, PromptOptions { streaming_behavior })
                .await
            {
                Ok(accepted) => {
                    if !matches!(accepted, PromptAccepted::Handled) {
                        *in_flight = true;
                    }
                    RpcResponse::ok("prompt", id, None)
                }
                Err(e) => RpcResponse::err("prompt", id, e.to_string()),
            }
        }
        SessionCommand::Steer { message, images } => {
            let id = raw_id.clone();
            match session.steer(user_input(message, images)).await {
                Ok(_) => {
                    latch_if_running(session, in_flight);
                    RpcResponse::ok("steer", id, None)
                }
                Err(e) => RpcResponse::err("steer", id, e.to_string()),
            }
        }
        SessionCommand::FollowUp { message, images } => {
            let id = raw_id.clone();
            match session.follow_up(user_input(message, images)).await {
                Ok(_) => {
                    latch_if_running(session, in_flight);
                    RpcResponse::ok("follow_up", id, None)
                }
                Err(e) => RpcResponse::err("follow_up", id, e.to_string()),
            }
        }
        SessionCommand::Abort => {
            // SEAM-024: Pi is `await session.abort(); return success(id, "abort")`
            // (rpc-mode.ts:427-430) and its `abort()` ends in `await this.waitForIdle()`
            // (agent-session.ts:1545) — so the success reply means "the run has stopped", not
            // "the cancel was requested". Replying before settlement made a client that
            // immediately re-prompts race the dying run. `abort` is dispatched CONCURRENTLY (it
            // is not in `is_inline_command`), so this await never stops the loop pumping events.
            session.abort_and_settle().await;
            // `abort` never touches an open dialog. Pi's `session.abort()` (agent-session.ts) only
            // cancels the run; `rpc-mode.ts`'s `case "abort"` never reaches into
            // `pendingExtensionRequests`. Dismissal of an open `confirm`/`input`/`select` dialog is
            // opt-in ONLY, through the extension itself binding a `signal_id` (Pi
            // `ExtensionUIDialogOptions.signal`, types.ts:320-321) and later calling
            // `ctx.abortSignal(id)` — nothing wires that binding to "the turn got aborted" by default
            // (independently confirmed: no first-party Pi call site does this either). A dialog left
            // open here settles only via a genuine `extension_ui_response` or its own `timeout_ms`.
            RpcResponse::ok("abort", raw_id.clone(), None)
        }
        SessionCommand::NewSession { parent_session } => {
            let id = raw_id.clone();
            let options = cyrup_session_svc::NewSessionOptions { parent_session };
            match runtime.new_session_with(options).await {
                Ok(result) => RpcResponse::ok(
                    "new_session",
                    id,
                    Some(json!({ "cancelled": result.cancelled })),
                ),
                Err(e) => RpcResponse::err("new_session", id, e.to_string()),
            }
        }

        // ------------------------------------------------------------------ State ----
        SessionCommand::GetState => {
            RpcResponse::ok("get_state", raw_id.clone(), Some(state_view(session).await))
        }

        // ------------------------------------------------------------------ Model ----
        SessionCommand::SetModel { provider, model_id } => {
            let id = raw_id.clone();
            // Resolve against the FULL auth-filtered registry, not just the active provider's own
            // catalog (Pi `session.modelRuntime.getAvailable()`, rpc-mode.ts:468 → model-registry.ts
            // `getAvailable()` = `getAll().filter(hasConfiguredAuth)`), so an embedder can switch to
            // a model owned by a DIFFERENT configured provider.
            let found = session
                .available_model_catalog()
                .iter()
                .find(|m| m.provider.as_str() == provider && m.id.as_str() == model_id)
                .cloned();
            match found {
                Some(model) => {
                    let model_json = serde_json::to_value(&model).unwrap_or(Value::Null);
                    match session.set_model_resolved(model).await {
                        Ok(_) => RpcResponse::ok("set_model", id, Some(model_json)),
                        Err(e) => RpcResponse::err("set_model", id, e.to_string()),
                    }
                }
                None => RpcResponse::err(
                    "set_model",
                    id,
                    format!("Model not found: {provider}/{model_id}"),
                ),
            }
        }
        SessionCommand::CycleModel => {
            let id = raw_id.clone();
            match session.cycle_model(true).await {
                Ok(Some(result)) => RpcResponse::ok(
                    "cycle_model",
                    id,
                    Some(json!({
                        "model": serde_json::to_value(&result.model).unwrap_or(Value::Null),
                        "thinkingLevel": result.thinking_level,
                        "isScoped": result.is_scoped,
                    })),
                ),
                Ok(None) => RpcResponse::ok("cycle_model", id, Some(Value::Null)),
                Err(e) => RpcResponse::err("cycle_model", id, e.to_string()),
            }
        }
        SessionCommand::GetAvailableModels => {
            // The full auth-filtered registry (Pi `session.modelRuntime.getAvailable()`,
            // rpc-mode.ts:486), NOT the active provider's own catalog.
            let models =
                serde_json::to_value(session.available_model_catalog()).unwrap_or(json!([]));
            RpcResponse::ok(
                "get_available_models",
                raw_id.clone(),
                Some(json!({ "models": models })),
            )
        }

        // --------------------------------------------------------------- Thinking ----
        SessionCommand::SetThinkingLevel { level } => {
            let id = raw_id.clone();
            match session.set_thinking_level(level).await {
                Ok(_) => RpcResponse::ok("set_thinking_level", id, None),
                Err(e) => RpcResponse::err("set_thinking_level", id, e.to_string()),
            }
        }
        SessionCommand::CycleThinkingLevel => {
            let id = raw_id.clone();
            match session.cycle_thinking_level().await {
                Ok(Some(level)) => {
                    RpcResponse::ok("cycle_thinking_level", id, Some(json!({ "level": level })))
                }
                Ok(None) => RpcResponse::ok("cycle_thinking_level", id, Some(Value::Null)),
                Err(e) => RpcResponse::err("cycle_thinking_level", id, e.to_string()),
            }
        }
        // Pi `rpc-mode.ts:507-510` @v0.83.0:
        //   `const levels = session.getAvailableThinkingLevels();`
        //   `return success(id, "get_available_thinking_levels", { levels });`
        // Infallible upstream and here — the backing accessor is synchronous and always answers.
        SessionCommand::GetAvailableThinkingLevels => RpcResponse::ok(
            "get_available_thinking_levels",
            raw_id.clone(),
            Some(json!({ "levels": session.available_thinking_levels() })),
        ),

        // ------------------------------------------------------------ Queue modes ----
        SessionCommand::SetSteeringMode { mode } => {
            session.set_steering_mode(mode.into());
            RpcResponse::ok("set_steering_mode", raw_id.clone(), None)
        }
        SessionCommand::SetFollowUpMode { mode } => {
            session.set_follow_up_mode(mode.into());
            RpcResponse::ok("set_follow_up_mode", raw_id.clone(), None)
        }

        // ------------------------------------------------------------- Compaction ----
        SessionCommand::Compact {
            custom_instructions,
        } => {
            let id = raw_id.clone();
            match session.compact(custom_instructions).await {
                Ok(result) => RpcResponse::ok(
                    "compact",
                    id,
                    Some(serde_json::to_value(result).unwrap_or(Value::Null)),
                ),
                Err(e) => RpcResponse::err("compact", id, e.to_string()),
            }
        }
        SessionCommand::SetAutoCompaction { enabled } => {
            session.set_auto_compaction_enabled(enabled);
            RpcResponse::ok("set_auto_compaction", raw_id.clone(), None)
        }

        // ------------------------------------------------------------------ Retry ----
        SessionCommand::SetAutoRetry { enabled } => {
            session.set_auto_retry_enabled(enabled);
            RpcResponse::ok("set_auto_retry", raw_id.clone(), None)
        }
        SessionCommand::AbortRetry => {
            session.abort_retry();
            // Same as `abort` above: no dialog dismissal — Pi's `session.abortRetry()` never touches
            // `pendingExtensionRequests` either.
            RpcResponse::ok("abort_retry", raw_id.clone(), None)
        }

        // ------------------------------------------------------------------- Bash ----
        SessionCommand::Bash {
            command,
            exclude_from_context,
        } => {
            let id = raw_id.clone();
            // A genuine backend failure (spawn error, missing cwd, …) must NOT be reported as a
            // success — Pi's `executeBashWithOperations` only catches the abort case; every other
            // error `throw`s (`bash-executor.ts:154`) straight through `executeBash` to the RPC
            // dispatcher's `catch` (`rpc-mode.ts:787-796` at pi HEAD), which emits an `error(...)`
            // response with no history entry ever recorded. Mirror that via the same `Ok`/`Err`
            // pattern every other fallible command here uses (e.g. `compact` above).
            // Pi threads the JSON-RPC request id into `executeBash`'s options (`rpc-mode.ts:575`,
            // `id`), so every `bash_execution_update` it emits carries the id of the request whose
            // output it belongs to. The wire id may be a string OR a number (see `raw_id`'s doc);
            // render a non-string id with its JSON spelling rather than dropping it.
            let bash_id = raw_id
                .as_ref()
                .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string));
            // An RPC-issued bash is a USER bash: it must fire the `user_bash` extension event with
            // `{command, excludeFromContext ?? false, cwd}` before executing, so an extension
            // observing user bash sees RPC-issued commands too, and a handler returning a full
            // `UserBashEventResult.result` short-circuits execution (Pi `rpc-mode.ts:558-579`'s
            // `case "bash"`, given its `emitUserBash` by pi `5d548ae9`, 2026-07-28, "fix: rpc bash
            // no longer bypass user_bash", #7214). `execute_bash_with_user_event` is the shared
            // emit-then-execute wrapper the interactive `!`/`!!` front-end uses as well — calling
            // the bare `execute_bash` here would bypass the event exactly as pre-#7214 Pi did.
            // The `None` third argument is `on_chunk`, and it is a FAITHFUL port, not an omission:
            // pi passes `undefined` in the same position (`session.executeBash(command.command,
            // undefined, {…})`, `rpc-mode.ts:573` @v0.83.0) because the RPC front-end observes output
            // through the `bash_execution_update` events keyed by `id` above, not through a callback.
            //
            // Pi's sibling `operations` override (`UserBashEventResult.operations`,
            // `extensions/types.ts:1078-1080`) is NOT threadable from HERE, and that is a shape
            // difference rather than a carve-out: pi emits `user_bash` AT this call site, so
            // `rpc-mode.ts:577` still holds `eventResult` and can pass `operations` down; cyrup emits
            // inside the shared `execute_bash_with_user_event` wrapper (so the interactive `!`/`!!`
            // front-end and this one cannot drift on WHETHER they emit), and the event result never
            // surfaces here. The override therefore has to be honored inside that wrapper.
            //
            // The CONSUMPTION half is now built and this `operations: None` is upstream's absent
            // `operations`, not a dropped one: `BashOptions::operations` exists,
            // `execute_bash_with_user_event` forwards it and `execute_bash` resolves pi's
            // `options?.operations ?? createLocalBashOperations({ shellPath })`
            // (`agent-session.ts:2782`), pinned by
            // `cyrup-session-svc/src/tests/round9_l5res.rs`'s three
            // `..._operations_override_...` tests. **ONE half is left, and it is not in this file
            // either:** cyrup's extension I/O is serde values (ADR-0002), so a WASM guest cannot
            // RETURN a backend — `emit_user_bash_event`'s reduction payload can carry the
            // `operations` KEY but never a callable behind it. Closing it is the
            // `register-bash-operations` import + keyed `bash-operations-exec` export round-trip
            // designed in full in the CYRUP-DELTA register in `crates/cyrup-ext/src/lib.rs`, plus
            // its guest half in `crates/cyrup-ext-sdk` and a `HOST_WORLD` minor bump. When that
            // lands, the wrapper sets one field and this arm is unchanged.
            // DRIFT-004 / SEAM-015.
            match session
                .execute_bash_with_user_event(
                    &command,
                    BashOptions {
                        exclude_from_context,
                        id: bash_id,
                        operations: None,
                    },
                    None,
                )
                .await
            {
                Ok(result) => RpcResponse::ok(
                    "bash",
                    id,
                    Some(serde_json::to_value(result).unwrap_or(Value::Null)),
                ),
                Err(e) => RpcResponse::err("bash", id, e.to_string()),
            }
        }
        SessionCommand::AbortBash => {
            session.abort_bash();
            RpcResponse::ok("abort_bash", raw_id.clone(), None)
        }

        // ---------------------------------------------------------------- Session ----
        SessionCommand::GetSessionStats => {
            let stats = serde_json::to_value(session.session_stats().await).unwrap_or(Value::Null);
            RpcResponse::ok("get_session_stats", raw_id.clone(), Some(stats))
        }
        SessionCommand::ExportHtml { output_path } => {
            let id = raw_id.clone();
            let path = output_path.map(std::path::PathBuf::from);
            match session.export_to_html(path.as_deref()).await {
                Ok(out) => RpcResponse::ok(
                    "export_html",
                    id,
                    Some(json!({ "path": out.display().to_string() })),
                ),
                Err(e) => RpcResponse::err("export_html", id, e.to_string()),
            }
        }
        SessionCommand::SwitchSession { session_path } => {
            let id = raw_id.clone();
            match runtime.switch_session(session_path).await {
                Ok(result) => RpcResponse::ok(
                    "switch_session",
                    id,
                    Some(json!({ "cancelled": result.cancelled })),
                ),
                Err(e) => RpcResponse::err("switch_session", id, e.to_string()),
            }
        }
        SessionCommand::Fork { entry_id } => {
            let id = raw_id.clone();
            match runtime
                .fork(EntryId::from(entry_id.as_str()), ForkPosition::Before)
                .await
            {
                Ok(result) => RpcResponse::ok(
                    "fork",
                    id,
                    Some(json!({ "text": result.selected_text, "cancelled": result.cancelled })),
                ),
                Err(e) => RpcResponse::err("fork", id, e.to_string()),
            }
        }
        SessionCommand::Clone => {
            let id = raw_id.clone();
            let leaf = session.leaf_id().await;
            match leaf {
                None => RpcResponse::err(
                    "clone",
                    id,
                    "Cannot clone session: no current entry selected",
                ),
                Some(leaf) => match runtime.fork(leaf, ForkPosition::At).await {
                    Ok(result) => {
                        RpcResponse::ok("clone", id, Some(json!({ "cancelled": result.cancelled })))
                    }
                    Err(e) => RpcResponse::err("clone", id, e.to_string()),
                },
            }
        }
        SessionCommand::GetForkMessages => {
            let messages: Vec<Value> = session
                .user_messages_for_forking()
                .await
                .into_iter()
                .map(|a| json!({ "entryId": a.entry_id.as_str(), "text": a.text }))
                .collect();
            RpcResponse::ok(
                "get_fork_messages",
                raw_id.clone(),
                Some(json!({ "messages": messages })),
            )
        }
        SessionCommand::GetEntries { since } => {
            let id = raw_id.clone();
            let mut entries = session.entries_json().await;
            if let Some(since) = since {
                match entries
                    .iter()
                    .position(|e| e.get("id").and_then(Value::as_str) == Some(since.as_str()))
                {
                    Some(idx) => entries = entries.split_off(idx + 1),
                    None => {
                        return RpcResponse::err(
                            "get_entries",
                            id,
                            format!("Entry not found: {since}"),
                        );
                    }
                }
            }
            let leaf = session.leaf_id().await.map(|l| l.as_str().to_string());
            RpcResponse::ok(
                "get_entries",
                id,
                Some(json!({ "entries": entries, "leafId": leaf })),
            )
        }
        SessionCommand::GetTree => {
            let tree = session.tree_json().await;
            let leaf = session.leaf_id().await.map(|l| l.as_str().to_string());
            RpcResponse::ok(
                "get_tree",
                raw_id.clone(),
                Some(json!({ "tree": tree, "leafId": leaf })),
            )
        }
        SessionCommand::GetLastAssistantText => {
            let text = session.last_assistant_text().await;
            RpcResponse::ok(
                "get_last_assistant_text",
                raw_id.clone(),
                Some(json!({ "text": text })),
            )
        }
        SessionCommand::SetSessionName { name } => {
            let id = raw_id.clone();
            let trimmed = name.trim();
            if trimmed.is_empty() {
                return RpcResponse::err("set_session_name", id, "Session name cannot be empty");
            }
            match session.set_session_name(trimmed).await {
                Ok(_) => RpcResponse::ok("set_session_name", id, None),
                Err(e) => RpcResponse::err("set_session_name", id, e.to_string()),
            }
        }

        // --------------------------------------------------------------- Messages ----
        SessionCommand::GetMessages => {
            let id = raw_id.clone();
            match serde_json::to_value(session.messages().await) {
                Ok(v) => RpcResponse::ok("get_messages", id, Some(json!({ "messages": v }))),
                Err(e) => RpcResponse::err("get_messages", id, e.to_string()),
            }
        }

        // --------------------------------------------------------------- Commands ----
        SessionCommand::GetCommands => RpcResponse::ok(
            "get_commands",
            raw_id.clone(),
            Some(json!({ "commands": session.slash_command_catalog() })),
        ),

        // Unreachable: `dispatch` intercepts the `#[serde(other)]` unknown-type variant before it
        // reaches `handle` (Pi's `switch` default, rpc-mode.ts:686-689). Kept for exhaustiveness —
        // defensively echoes the id rather than panicking (R-00-009).
        SessionCommand::Unknown => {
            RpcResponse::err(String::new(), raw_id.clone(), "Unknown command: undefined")
        }
    }
}

/// A fresh correlation id for an `extension_ui_request` (Pi `crypto.randomUUID`, rpc-mode.ts:98). A
/// process-monotonic counter suffices: the id is opaque and only has to be unique among the dialogs
/// in flight on this loop, and the client echoes it back verbatim on the `extension_ui_response`.
fn new_request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("ext-ui-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// If `line` is an `extension_ui_response` envelope, return `Some(id)` — where the inner `Option`
/// is the correlation `id`, `None` when the envelope carries no *string* `id`. Returns the outer
/// `None` for any other line, so only non-`extension_ui_response` lines fall through to the normal
/// command path.
///
/// SEAM-086 — the `type` discriminant alone decides the intercept, exactly as pi's does. pi's
/// `handleInputLine` (`packages/coding-agent/src/modes/rpc/rpc-mode.ts:763-777` @v0.83.0) tests
/// `parsed.type === "extension_ui_response"`, looks the id up in `pendingExtensionRequests`, and
/// `return`s **unconditionally** — a malformed or unmatched envelope produces no output line at all.
/// Deciding the intercept on the id instead let a malformed one fall into `dispatch`, which answered
/// it with `{"type":"response","command":"extension_ui_response","success":false,"error":"Unknown
/// command: extension_ui_response"}` — an extra stdout line pi never writes.
fn extension_ui_response_id(line: &str) -> Option<Option<String>> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("extension_ui_response") {
        return None;
    }
    Some(value.get("id").and_then(Value::as_str).map(str::to_owned))
}

/// Build an RPC-sourced [`UserInput`] from text + optional image content blocks.
fn user_input(text: String, images: Vec<Content>) -> UserInput {
    UserInput {
        text,
        images,
        source: InputSource::Rpc,
        expand_templates: true,
    }
}

/// The full `get_state` snapshot (Pi `RpcSessionState`, rpc-types.ts:94-107).
async fn state_view(session: &AgentSession) -> Value {
    let model_ref = session.model();
    // The full `Model` (Pi `session.model` is already the resolved `Model` object), resolved out of
    // the FULL auth-filtered registry so a model owned by a non-active provider still carries its
    // real metadata; only a genuinely unknown model degrades to a minimal `{provider, id}`.
    // `RpcSessionState.model` is `Model | undefined` (rpc-types.ts:95) because
    // `AgentSession.model` is (agent-session.ts:866-868), so a modelless session OMITS the key
    // rather than reporting a synthesized address (see the insertion note below).
    let model = model_ref.map(|model_ref| {
        session
            .available_model_catalog()
            .iter()
            .find(|m| m.provider == model_ref.provider && m.id == model_ref.model)
            .and_then(|m| serde_json::to_value(m).ok())
            .unwrap_or_else(|| {
                json!({
                    "provider": model_ref.provider.as_str(),
                    "id": model_ref.model.as_str(),
                })
            })
    });
    // SEAM-053 — the three OPTIONAL members (`model?`, `sessionFile?`, `sessionName?`,
    // rpc-types.ts:95/102/104) are built by insertion rather than by `json!`, because pi builds the
    // object as a TS literal and `JSON.stringify` DROPS an `undefined` property: pi's line for an
    // unnamed ephemeral session contains neither key, where a `json!` `None` emits an explicit
    // `null`. A client using `"sessionName" in state` — the natural idiom for an optional property —
    // took the wrong branch against cyrup. The required members keep their `null`-free types and are
    // always present, exactly as upstream.
    let mut state = serde_json::Map::new();
    if let Some(model) = model {
        state.insert("model".to_string(), model);
    }
    state.insert(
        "thinkingLevel".to_string(),
        json!(session.thinking_level().await),
    );
    state.insert(
        "isStreaming".to_string(),
        json!(session.is_streaming().await),
    );
    state.insert("isCompacting".to_string(), json!(session.is_compacting()));
    state.insert(
        "steeringMode".to_string(),
        json!(queue_mode_str(session.steering_mode())),
    );
    state.insert(
        "followUpMode".to_string(),
        json!(queue_mode_str(session.follow_up_mode())),
    );
    if let Some(file) = session.session_file().await {
        state.insert("sessionFile".to_string(), json!(file.display().to_string()));
    }
    state.insert(
        "sessionId".to_string(),
        json!(session.session_id().as_str()),
    );
    if let Some(name) = session.session_name().await {
        state.insert("sessionName".to_string(), json!(name));
    }
    state.insert(
        "autoCompactionEnabled".to_string(),
        json!(session.auto_compaction_enabled()),
    );
    state.insert(
        "messageCount".to_string(),
        json!(session.messages().await.len()),
    );
    state.insert(
        "pendingMessageCount".to_string(),
        json!(session.pending_message_count()),
    );
    Value::Object(state)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use cyrup_ext::DialogOptions;
    use cyrup_session_svc::ModelThinkingLevel;

    /// PROV-002: the RPC surface must accept `max`. `SessionCommand` is serde-driven, so this
    /// pins that an RPC client can actually reach the top rung (and that a bogus level still
    /// produces a clean error rather than a silent lower level).
    #[test]
    fn set_thinking_level_accepts_max_over_rpc() {
        let cmd: SessionCommand =
            serde_json::from_str(r#"{"type":"set_thinking_level","level":"max"}"#)
                .expect("`max` must deserialize");
        assert!(matches!(
            cmd,
            SessionCommand::SetThinkingLevel {
                level: ModelThinkingLevel::Max
            }
        ));
        assert!(
            serde_json::from_str::<SessionCommand>(
                r#"{"type":"set_thinking_level","level":"ultra"}"#
            )
            .is_err(),
            "an unknown level is a serde error, not a silent downgrade"
        );
    }

    fn editor_request(title: &str, initial: &str) -> UiRequest {
        let (reply, _rx) = oneshot::channel();
        UiRequest {
            kind: UiKind::Editor,
            prompt: title.to_string(),
            options: Value::Null,
            message: initial.to_string(),
            placeholder: None,
            opts: DialogOptions::default(),
            reply,
        }
    }

    /// L4 review §2 (`ui.editor` WIT signature drops Pi's `title` param; RPC hardcodes `"title":
    /// ""`): the wire request sent to an RPC client for `ui.editor` must carry the guest's REAL
    /// title (Pi `editor(title, prefill)` → `{method:"editor", title, prefill}`,
    /// `rpc-mode.ts:253-268`, `rpc-types.ts:241`) — never the pre-fix hardcoded `""`, and `prefill`
    /// must be the seed text, not the title again.
    #[test]
    fn editor_wire_request_carries_the_real_title_not_a_hardcoded_empty_string() {
        let req = editor_request("edit the changelog", "## seed content");
        let wire = extension_ui_request_json("req-1", &req);
        assert_eq!(wire["method"], "editor");
        assert_eq!(
            wire["title"], "edit the changelog",
            "the real guest title must reach the wire"
        );
        assert_ne!(
            wire["title"], "",
            "title must never be the pre-fix hardcoded empty string"
        );
        assert_eq!(
            wire["prefill"], "## seed content",
            "prefill carries the seed text, not the title"
        );
    }
}
