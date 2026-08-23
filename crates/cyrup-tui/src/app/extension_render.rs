use super::*;

/// Truncate a one-line summary to a sane length (avoid overrunning the marker line).
/// Detect a `Custom`-role `cyrup_agent::AgentMessage` from its serde projection and return its
/// `(kind, body)` for [`TranscriptView::push_custom_message`](crate::transcript::TranscriptView::push_custom_message).
/// `AgentMessage` is only a dev-dependency here, so the message is inspected through `serde_json`
/// (`{"role":"custom","kind":…,"payload":…}`) instead of a direct pattern match — no dep ripple.
/// Returns `None` for any non-custom (core user/assistant/toolResult) message.
/// Whether the interactive host should exit NOW because a loaded extension called `ctx.shutdown()`
/// (EXT-005 / SEAM-005).
///
/// Pi checks a pending shutdown at exactly two moments, and cyrup's run loop calls this at both:
///
/// * `at_settle` — the `agent_settled` arm, `case "agent_settled": await
///   this.checkShutdownRequested()` (interactive-mode.ts:3137-3138). A settle means the whole run
///   is over (no retry, no post-run compaction, no queued continuation), so no idle re-check is
///   needed or wanted;
/// * otherwise — the `shutdownHandler` Pi binds in `bindExtensions`,
///   `this.shutdownRequested = true; if (this.session.isIdle) { void this.shutdown(); }`
///   (interactive-mode.ts:1753-1757). This is what makes Pi's own canonical example,
///   `examples/extensions/shutdown-command.ts` — a `/quit` COMMAND that never starts a run — exit
///   at all; gating solely on a settle would strand it forever.
///
/// Kept as a named predicate rather than an inline condition so it is testable without driving a
/// real terminal event loop.
pub fn should_honor_extension_shutdown(
    session: &cyrup_session_svc::AgentSession,
    at_settle: bool,
) -> bool {
    session.shutdown_requested() && (at_settle || session.is_idle())
}

/// How long the fold waits for an extension renderer before falling back to the built-in framing.
///
/// A renderer is a presentation concern on the interactive event path; it must never be able to
/// wedge the frame. The call runs on its OWN task (not inline in the `select!` arm) for the same
/// reason `AppAction::ExtensionShortcut` spawns: a guest handler may synchronously block on a
/// `ui.{confirm,input,…}` capability whose reply only THIS loop can deliver, and awaiting it inline
/// would be a genuine self-deadlock. Spawn + bounded wait makes the worst case "the block draws
/// with its built-in renderer", never a hang.
pub(crate) const EXTENSION_RENDER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Ask the loaded extensions to render this event, if any registered a renderer for it (EXT-006).
///
/// * a custom message → the extension that registered `custom_type` (Pi `getMessageRenderer`,
///   runner.ts:579-587, resolved at interactive-mode.ts:3326);
/// * a tool start/end → the extension that declared a renderer for that TOOL NAME (Pi's per-tool
///   `renderCall`/`renderResult`, tool-execution.ts:81-112).
///
/// `None` for every other event, and for any event whose key has no registered renderer — the cheap
/// SYNC `has_*_renderer` pre-check runs first so the common path pays nothing.
pub async fn extension_render(
    ext_host: &Arc<cyrup_ext::ExtensionHost>,
    ev: &AgentSessionEvent,
) -> Option<String> {
    let which = match ev {
        AgentSessionEvent::MessageEnd { .. } => {
            let (kind, _) = custom_message_from_event(ev)?;
            if !ext_host.has_message_renderer(&kind) {
                return None;
            }
            let message = serde_json::to_value(ev).ok()?.get("message")?.clone();
            Which::Message(kind, message)
        }
        AgentSessionEvent::ToolExecutionStart { tool_name, args, .. } => {
            if !ext_host.has_tool_renderer(tool_name) {
                return None;
            }
            Which::ToolCall(tool_name.clone(), args.clone())
        }
        AgentSessionEvent::ToolExecutionEnd { tool_name, result, .. } => {
            if !ext_host.has_tool_renderer(tool_name) {
                return None;
            }
            Which::ToolResult(tool_name.clone(), result.clone())
        }
        _ => return None,
    };
    // A FAULTING renderer collapses to `None` here on purpose: both of this function's surfaces
    // swallow the throw upstream — a custom message falls through to its default `[type] body` box
    // (`custom-message.ts:82-84`, `catch { /* Fall through to default rendering */ }`) and a tool
    // row keeps its built-in shell. The distinction is preserved by the host
    // ([`cyrup_ext::RenderOutcome`]) for the ENTRY surface, which does NOT swallow it — see
    // [`extension_render_entry`].
    run_renderer(ext_host, which).await.into_text()
}

/// Ask the loaded extensions to render an appended custom ENTRY (X15; Pi `addCustomEntryToChat`,
/// `interactive-mode.ts:3431-3436`, resolving `extensionRunner.getEntryRenderer(entry.customType)`
/// at `runner.ts:593-600`).
///
/// This is the ONE surface where the renderer's fault is user-visible, so it is the one that must
/// NOT collapse the three-state [`cyrup_ext::RenderOutcome`] the way [`extension_render`] does:
///
/// * no renderer, or a renderer that drew nothing (`:3433-3435` / `:3438-3440`) →
///   [`crate::transcript::Rendered::None`], and the caller draws NOTHING;
/// * a rendered component (`custom-entry.ts:58-60`) → [`crate::transcript::Rendered::Text`];
/// * a renderer that THREW (`custom-entry.ts:47-52`) → [`crate::transcript::Rendered::Failed`], the failure box.
///
/// Same cheap sync pre-check (`if (!renderer) return;`) and the same spawn + bounded wait as
/// [`extension_render`].
pub async fn extension_render_entry(
    ext_host: &Arc<cyrup_ext::ExtensionHost>,
    custom_type: &str,
    entry: &serde_json::Value,
) -> crate::transcript::Rendered {
    if !ext_host.has_entry_renderer(custom_type) {
        return crate::transcript::Rendered::None;
    }
    run_renderer(ext_host, Which::Entry(custom_type.to_string(), entry.clone())).await
}

/// The `customType` of a serialized session entry — the key an entry renderer is registered under
/// (Pi `entry.customType`, `session-manager.ts` `CustomEntry`; read by `addCustomEntryToChat`,
/// `interactive-mode.ts:3432`).
///
/// Both spellings are accepted because the persisted entry carries `customType` while the event
/// envelope's discriminator is `type`; `"custom"` is the last-resort label for an entry that
/// carries neither, and no renderer will ever claim it.
pub(crate) fn custom_entry_type(entry: &serde_json::Value) -> String {
    entry
        .get("customType")
        .or_else(|| entry.get("custom_type"))
        .or_else(|| entry.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("custom")
        .to_string()
}

/// Which registered renderer to invoke, and with what payload.
pub(crate) enum Which {
    Message(String, serde_json::Value),
    ToolCall(String, serde_json::Value),
    ToolResult(String, serde_json::Value),
    Entry(String, serde_json::Value),
}

/// Resolve an extension's registered MESSAGE renderer for `custom_type` and run it against
/// `payload` (Pi `getMessageRenderer(customType)`, `extensions/runner.ts:579-587`).
///
/// X11 — the REPLAY walk needs this without an `AgentSessionEvent` to hand: a `/resume` reads
/// persisted `AgentMessage`s, not events, and Pi performs the identical lookup there
/// (`interactive-mode.ts:3471`) as on the live path. Same cheap sync `has_message_renderer`
/// pre-check and the same spawn + bounded wait as [`extension_render`], so a wedged guest degrades
/// a replayed block to its built-in framing instead of stalling the resume.
/// Run one renderer on its OWN task under [`EXTENSION_RENDER_TIMEOUT`] — see that constant's doc for
/// why the call must never be awaited inline on the event path.
///
/// X15 — the host now reports three outcomes ([`cyrup_ext::RenderOutcome`]) and this carries all
/// three back; [`extension_render`]/[`extension_render_message`] are what collapse `Failed` into
/// the default framing for their surfaces, because upstream does
/// (`custom-message.ts:82-84` catches and falls through).
pub(crate) async fn run_renderer(
    ext_host: &Arc<cyrup_ext::ExtensionHost>,
    which: Which,
) -> crate::transcript::Rendered {
    use crate::transcript::Rendered;
    let host = ext_host.clone();
    let task = tokio::spawn(async move {
        match which {
            Which::Message(key, payload) => host.render_message_call_outcome(&key, &payload).await,
            Which::ToolCall(key, payload) => host.render_tool_call_outcome(&key, &payload).await,
            Which::ToolResult(key, payload) => host.render_tool_result_outcome(&key, &payload).await,
            Which::Entry(key, payload) => host.render_entry(&key, &payload).await,
        }
    });
    let abort = task.abort_handle();
    match tokio::time::timeout(EXTENSION_RENDER_TIMEOUT, task).await {
        Ok(Ok(cyrup_ext::RenderOutcome::Rendered(v))) => Rendered::Text(rendered_text(&v)),
        // The renderer FAULTED. `cyrup-ext` already contained the fault (native panic caught,
        // guest trap mapped) and kept its message; upstream's `catch` binding is the same value.
        Ok(Ok(cyrup_ext::RenderOutcome::Failed(message))) => Rendered::Failed(message),
        Ok(Ok(cyrup_ext::RenderOutcome::None)) => Rendered::None,
        // The renderer TASK itself panicked — outside the host's `catch_unwind`, so no message
        // survived the unwind. Report it as a fault anyway: something threw, and reporting `None`
        // here is precisely the conflation X15 is about.
        Ok(Err(_)) => Rendered::Failed("renderer task panicked".to_string()),
        Err(_) => {
            // Cancel the wedged call rather than detaching it: dropping a `JoinHandle` only
            // detaches, and a renderer that blocks once will block again on the next event, so
            // detached tasks would pile up behind the instance's store lock.
            abort.abort();
            // NOT a fault: upstream renderers are synchronous and cannot time out, so there is no
            // `catch` to model. A wedged renderer degrades to the built-in framing (and, for an
            // entry, to nothing at all) rather than accusing the extension of throwing.
            Rendered::None
        }
    }
}

/// How deep a widget tree may nest before the flattener gives up (a guest can hand the host any
/// JSON, including a pathologically deep one; the flattener must terminate on adversarial input).
pub(crate) const MAX_WIDGET_DEPTH: usize = 16;

/// Flatten a renderer's returned JSON — a SERIALIZED WIDGET TREE — into the display text the
/// transcript draws.
///
/// This is the host half of the `render-call`/`render-result` contract documented in
/// `cyrup-ext/wit/world.wit`. Pi's renderers return an in-process `pi-tui` `Component` which the
/// interactive mode adds as a child of `CustomMessageComponent`/`ToolExecutionComponent`
/// (`components/custom-message.ts:66-81`, `components/tool-execution.ts:81-112`); nothing is ever
/// stringified. A WASM guest cannot hand back a live object, so cyrup's wire analog is the
/// component tree SERIALIZED, and the host is what turns it back into rows — the exact step that
/// was missing: every non-string return used to be pretty-printed, so a guest following cyrup's own
/// SDK example (`{"widget":"text","text":…}`) drew a raw JSON blob where Pi draws the component.
///
/// The vocabulary mirrors the `pi-tui` components a renderer actually returns (`packages/tui/src/
/// index.ts:13-32`); it is duplicated verbatim in both WIT world copies and constructed by
/// `cyrup_ext_sdk::widget` on the guest side:
///
/// | JSON                                              | Pi component      |
/// |---------------------------------------------------|-------------------|
/// | `"…"` (a bare string)                              | `Text` (degenerate) |
/// | `{"widget":"text","text":"…"}`                     | `Text` — the dominant shape |
/// | `{"widget":"markdown","text":"…"}`                 | `Markdown`        |
/// | `{"widget":"truncated-text","text":"…"}`           | `TruncatedText`   |
/// | `{"widget":"spacer","lines":n}` (default 1)        | `Spacer`          |
/// | `{"widget":"box"\|"container","children":[…]}`     | `Box` / `Container` — stacked |
/// | `{"widget":"hstack","children":[…]}`               | `HStack` — joined on one row |
/// | `[…]` (a bare array)                               | shorthand for a stack |
///
/// Anything the vocabulary does not cover — an unknown `widget` tag, a missing tag, a tree deeper
/// than [`MAX_WIDGET_DEPTH`] — falls back to the pretty-printed JSON rather than being dropped, so
/// an author who mistypes a node SEES the node instead of a blank row. The fallback applies to the
/// WHOLE tree, not the offending node, so the JSON on screen is the one the guest actually returned.
pub(crate) fn rendered_text(v: &serde_json::Value) -> String {
    flatten_widget(v, 0)
        .unwrap_or_else(|| serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()))
}

/// One node of [`rendered_text`]'s widget tree. `None` = "not a widget I know", which the caller
/// turns into the whole-tree JSON fallback.
pub(crate) fn flatten_widget(v: &serde_json::Value, depth: usize) -> Option<String> {
    use serde_json::Value;
    if depth > MAX_WIDGET_DEPTH {
        return None;
    }
    match v {
        // A bare string is the degenerate `Text` node: a renderer that just wants to hand back the
        // lines it wants drawn should not have to wrap them.
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => flatten_children(items, depth, "\n"),
        Value::Object(o) => {
            let text = || o.get("text").and_then(Value::as_str).unwrap_or("").to_string();
            let children = |sep: &str| match o.get("children") {
                Some(Value::Array(items)) => flatten_children(items, depth, sep),
                // A container with no children is an empty row, not an error (Pi's `Container`
                // renders nothing until something is added to it).
                None => Some(String::new()),
                Some(_) => None,
            };
            match o.get("widget").and_then(Value::as_str)? {
                "text" | "markdown" | "truncated-text" => Some(text()),
                "spacer" => {
                    // `n` blank rows = a string of `n - 1` newlines (one empty row needs no
                    // separator). Clamped so a guest cannot ask the transcript for a million rows.
                    let n = o.get("lines").and_then(Value::as_u64).unwrap_or(1).min(64) as usize;
                    Some("\n".repeat(n.saturating_sub(1)))
                }
                "box" | "container" => children("\n"),
                "hstack" => children(""),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Flatten every child, joining with `sep`. One unrecognized child fails the WHOLE tree so the
/// caller's JSON fallback shows the guest's actual return rather than a half-rendered tree with a
/// silently missing row.
pub(crate) fn flatten_children(items: &[serde_json::Value], depth: usize, sep: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        out.push(flatten_widget(item, depth.saturating_add(1))?);
    }
    Some(out.join(sep))
}

pub async fn extension_render_message(
    ext_host: &Arc<cyrup_ext::ExtensionHost>,
    custom_type: &str,
    payload: &serde_json::Value,
) -> Option<String> {
    if !ext_host.has_message_renderer(custom_type) {
        return None;
    }
    // Same collapse as [`extension_render`]: `custom-message.ts:82-84` swallows the throw.
    run_renderer(ext_host, Which::Message(custom_type.to_string(), payload.clone()))
        .await
        .into_text()
}
