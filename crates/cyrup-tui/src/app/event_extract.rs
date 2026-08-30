use super::*;

/// Whether `ev` can have moved `getContextUsage()`'s answer, and so needs a footer refresh.
///
/// Upstream recomputes the segment on every frame, so this predicate exists only to keep cyrup from
/// walking the session's entries on events that provably cannot change it (a keystroke echo, a
/// status line). The answer is a function of the branch's last assistant `usage`, the latest
/// compaction on that branch, and the model's context window — so: a finished message, the end of a
/// turn, a compaction, a model swap, and a session replacement.
pub(crate) fn context_usage_may_have_moved(ev: &AgentSessionEvent) -> bool {
    matches!(
        ev,
        AgentSessionEvent::MessageEnd { .. }
            | AgentSessionEvent::AgentEnd { .. }
            | AgentSessionEvent::CompactionEnd { .. }
            | AgentSessionEvent::ModelChanged { .. }
            | AgentSessionEvent::SessionStart { .. }
            | AgentSessionEvent::SessionReplaced { .. }
    )
}

/// The notice shown under an assistant turn that stopped on `length`, verbatim from Pi v0.84.1
/// `coding-agent/src/modes/interactive/components/assistant-message.ts:180`.
///
/// **Version lag, not a port bug.** Through v0.83.0 (`:153-161`) this read
/// `"Error: Model stopped because it reached the maximum output token limit. The response may be
/// incomplete."`. Upstream shortened it in `32850ef7c` ("fix(coding-agent): resume after
/// context-limited length stops", #7540), whose commit message gives the reason: a `length` stop is
/// no longer necessarily a max-output-token stop — it may be a context overflow that pi then
/// compacts and retries — so the TUI moved to "neutral truncation wording" that does not assert a
/// cause. Note the loss of the `Error: ` prefix is part of that change and is deliberate upstream:
/// only the `error` arm (`:193`) still prefixes.
/// The `error`-styled notice Pi appends after an assistant turn that did not finish cleanly
/// (v0.84.1 `coding-agent/src/modes/interactive/components/assistant-message.ts:174-195`), or `None`
/// for a clean turn.
///
/// * `length` → [`LENGTH_STOP_NOTICE`], emitted **unconditionally**: a length stop can land before a
///   tool call is complete, so it is surfaced even on a tool turn (`:177`).
/// * `aborted` / `error` → emitted only when the message carries NO `toolCall` content (`:182`),
///   because for those the tool-execution component already reports the failure.
/// * `aborted` shows `errorMessage` unless it is the internal `Request was aborted` sentinel, in
///   which case the user-facing wording is `Operation aborted` (`:183-189`).
/// * `error` shows `Error: {errorMessage || "Unknown error"}` (`:190-193`).
pub(crate) fn stop_reason_notice(message: &cyrup_core::AssistantMessage) -> Option<String> {
    use cyrup_core::StopReason;
    if message.stop_reason == StopReason::Length {
        return Some(LENGTH_STOP_NOTICE.to_string());
    }
    let has_tool_calls =
        message.content.iter().any(|c| matches!(c, cyrup_core::Content::ToolCall(_)));
    if has_tool_calls {
        return None;
    }
    match message.stop_reason {
        StopReason::Aborted => Some(match message.error_message.as_deref() {
            Some(m) if !m.is_empty() && m != "Request was aborted" => m.to_string(),
            _ => "Operation aborted".to_string(),
        }),
        StopReason::Error => Some(format!(
            "Error: {}",
            match message.error_message.as_deref() {
                Some(m) if !m.is_empty() => m,
                _ => "Unknown error",
            }
        )),
        // `Pending` is the in-flight sentinel, so it must render like Pi's: Pi's chain is
        // `if (stopReason === "length") … else if (!hasToolCalls) { if ("aborted") … else if
        // ("error") … }` (assistant-message.ts:177-201), and `"pending"` matches none of them —
        // no notice. Grouped explicitly rather than via a `_ =>` so a future variant still breaks
        // this match, which is how this arm got written in the first place.
        //
        // `Deferred` joins it for the same reason, verified the same way: `deferred` appears
        // NOWHERE in `v0.84.1 coding-agent/src/modes/interactive/components/assistant-message.ts`,
        // so Pi's chain falls through it too and renders no notice.
        StopReason::Pending
        | StopReason::Deferred
        | StopReason::Stop
        | StopReason::Length
        | StopReason::ToolUse => None,
    }
}

/// Flatten a styled [`Line`] into its plain text (concatenated span content).
#[cfg(any(test, feature = "scrollback-accumulator"))]
pub(crate) fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

// The clipboard WRITE lives in [`crate::clipboard`] — Pi's `utils/clipboard.ts` is likewise its own
// module, and the target-gated pair that used to sit here (a `#[cfg(unix)]` CLI probe beside a
// `#[cfg(not(unix))]` no-op) is documented there as what it replaced.

/// Read a system-clipboard image and materialize it as a PNG temp file, returning its path (Pi
/// `readClipboardImage` + the temp-file write of `handleClipboardImagePaste`,
/// interactive-mode.ts:2544-2549 / `utils/clipboard-image.ts`). `arboard` is the faithful Rust analog
/// of Pi's native clipboard module (`utils/clipboard-native.ts`): NSPasteboard on macOS, X11/Wayland on
/// Linux. `get_image` hands back an RGBA8 raster (`arboard::ImageData`), which is re-encoded to PNG with
/// the in-tree `image` crate and written to `cyrup-clipboard-<uuid>.png` in the OS temp dir (Pi's
/// `pi-clipboard-<randomUUID>.<ext>`, always PNG here since the raster is already decoded).
///
/// Returns `None` when the clipboard holds no image (Pi's `clipboard.hasImage()` gate) or on ANY
/// clipboard/decode/encode/IO error — mirroring Pi's `catch {}` silent-ignore (no clipboard access, a
/// headless/permission-denied session, a zero-area raster, …) — so a bare Ctrl+V never disrupts the
/// editor and simply falls through to normal text handling.
pub(crate) fn read_clipboard_image_to_temp() -> Option<std::path::PathBuf> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let img = clipboard.get_image().ok()?;
    let width = u32::try_from(img.width).ok()?;
    let height = u32::try_from(img.height).ok()?;
    // `arboard::ImageData::bytes` is an RGBA8 raster; `from_raw` returns `None` if the buffer length
    // does not match `width * height * 4`, guarding a malformed clipboard payload without panicking.
    let raster = image::RgbaImage::from_raw(width, height, img.bytes.into_owned())?;
    let path =
        std::env::temp_dir().join(format!("cyrup-clipboard-{}.png", uuid::Uuid::now_v7()));
    raster.save_with_format(&path, image::ImageFormat::Png).ok()?;
    Some(path)
}

/// The largest `edit` target that gets a synchronous pre-execution preview.
///
/// Pi's `computeEditsDiff` is `async` and its result lands via `context.invalidate()`, so an
/// enormous file only costs it a late repaint. cyrup's fold
/// ([`App::ingest_event_rendered_owned`]) is synchronous — it mutates `&mut self` from a `select!`
/// arm — so the read+diff happens on the UI thread and an unbounded one would stall the frame. Source files an `edit` targets are orders of
/// magnitude under this; above it the preview is simply skipped and the post-write `details.diff`
/// renders as before, which is the pre-preview behaviour, not a regression.
pub(crate) const MAX_EDIT_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;

/// Pi `getRenderablePreviewInput` (edit.ts:170-192) + the `computeEditsDiff` call its `renderCall`
/// makes (`:377-386`), as one synchronous step.
///
/// The arguments are the RAW ones off the wire, before the agent preflight runs the tool's
/// `prepare_arguments` shim, so both shapes Pi accepts are handled here too: `edits[]` of
/// `{oldText, newText}` string pairs, and the legacy top-level `{oldText, newText}` single edit.
/// The path may arrive as `path` or `file_path`. Anything else yields `None` — no preview, no
/// change in behaviour.
pub(crate) fn edit_preview(
    args: &serde_json::Value,
    cwd: &std::path::Path,
) -> Option<Result<String, String>> {
    let obj = args.as_object()?;
    let path = obj
        .get("path")
        .or_else(|| obj.get("file_path"))
        .and_then(serde_json::Value::as_str)?;

    let str_field = |v: &serde_json::Value, k: &str| {
        v.get(k).and_then(serde_json::Value::as_str).map(str::to_string)
    };
    let edits: Vec<(String, String)> = match obj.get("edits").and_then(serde_json::Value::as_array) {
        // `args.edits.every(edit => typeof edit?.oldText === "string" && ...)` — one malformed
        // entry rejects the whole preview (edit.ts:180-186).
        Some(list) if !list.is_empty() => list
            .iter()
            .map(|e| Some((str_field(e, "oldText")?, str_field(e, "newText")?)))
            .collect::<Option<Vec<_>>>()?,
        // `if (typeof args.oldText === "string" && typeof args.newText === "string")` (`:188-190`).
        _ => vec![(str_field(args, "oldText")?, str_field(args, "newText")?)],
    };

    let absolute = cyrup_tools::path::resolve_to_cwd(path, cwd);
    if std::fs::metadata(&absolute).is_ok_and(|m| m.len() > MAX_EDIT_PREVIEW_BYTES) {
        return None;
    }
    Some(
        cyrup_tools::tools::edit_diff::compute_edits_diff(path, &edits, cwd)
            .map(|p| {
                if p.unapplied.is_empty() {
                    p.diff
                } else {
                    format!("{}\n{}", p.diff, p.unapplied.join("\n"))
                }
            })
            .map_err(|e| e.to_string()),
    )
}

/// The `usage` a `toolResult` message carries, if any (Pi `entry.message.role === "toolResult" &&
/// entry.message.usage`, `footer.ts:99-101`). Read through the same serde projection
/// [`custom_message_from_event`] uses, for the same reason: the `AgentMessage` type lives in
/// `cyrup-agent`, which is only a dev-dependency here.
pub(crate) fn tool_result_usage_from_event(ev: &AgentSessionEvent) -> Option<cyrup_core::Usage> {
    let value = serde_json::to_value(ev).ok()?;
    let message = value.get("message")?;
    if message.get("role").and_then(serde_json::Value::as_str) != Some("toolResult") {
        return None;
    }
    serde_json::from_value(message.get("usage")?.clone()).ok()
}

/// The `role` discriminant of the message an event carries (`"user"`/`"assistant"`/`"toolResult"`/
/// `"custom"`), read through the same serde projection [`custom_message_from_event`] uses and for
/// the same reason: `AgentMessage` lives in `cyrup-agent`, a dev-dependency here.
///
/// This is Pi's `event.message.role` test (`interactive-mode.ts:3122`, `:3181`).
pub(crate) fn message_role_from_event(ev: &AgentSessionEvent) -> Option<String> {
    let value = serde_json::to_value(ev).ok()?;
    Some(value.get("message")?.get("role")?.as_str()?.to_string())
}

/// The text of the USER message a `message_start` carries — Pi's `event.message` handed to
/// `addMessageToChat` (`interactive-mode.ts:2916`). Returns `None` for any other event or role.
///
/// This is what writes the user bubble into the transcript, and the only thing that does for a live
/// submission: a message the session decides to QUEUE produces no `message_start` until the turn
/// that actually carries it begins, which is precisely the distinction TUI-016/TUI-052 turn on.
pub(crate) fn user_message_text_from_event(ev: &AgentSessionEvent) -> Option<String> {
    let value = serde_json::to_value(ev).ok()?;
    let message = value.get("message")?;
    if message.get("role").and_then(serde_json::Value::as_str) != Some("user") {
        return None;
    }
    // Same projection [`assistant_message_from_event`] uses: `cyrup_agent::AgentMessage` is not a
    // direct dependency of this crate, and the serialized form is stable — it is the wire shape the
    // `--json` stream and the session JSONL are both written from.
    let content: Vec<cyrup_core::Content> =
        serde_json::from_value(message.get("content")?.clone()).ok()?;
    Some(crate::transcript::content_text(&content))
}

/// The authoritative [`AssistantMessage`](cyrup_core::AssistantMessage) a `message_end` carries, via
/// the same projection. `AgentMessage::Assistant` is an internally-tagged newtype variant, so the
/// serialized object is the assistant message's own fields plus `role` — which deserializes
/// straight back into `AssistantMessage`.
pub(crate) fn assistant_message_from_event(ev: &AgentSessionEvent) -> Option<cyrup_core::AssistantMessage> {
    let value = serde_json::to_value(ev).ok()?;
    let message = value.get("message")?;
    if message.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
        return None;
    }
    serde_json::from_value(message.clone()).ok()
}

pub(crate) fn custom_message_from_event(ev: &AgentSessionEvent) -> Option<(String, String)> {
    let value = serde_json::to_value(ev).ok()?;
    let message = value.get("message")?;
    if message.get("role").and_then(serde_json::Value::as_str) != Some("custom") {
        return None;
    }
    let kind =
        message.get("kind").and_then(serde_json::Value::as_str).unwrap_or("custom").to_string();
    let body = message.get("payload").map(custom_message_text).unwrap_or_default();
    Some((kind, body))
}

/// Extract display text from a `Custom` message payload (`string | (Text|Image)[]`, mirroring Pi's
/// `getCustomMessageText`): a JSON string is used verbatim; an array joins its `{text}` parts; any
/// other shape yields the empty string (rendered as a bare label).
pub(crate) fn custom_message_text(payload: &serde_json::Value) -> String {
    match payload {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Collapse a multi-line string to a single space-joined line, truncated to 80 graphemes with an
/// ellipsis (`truncateSummary` — selector descriptions, tree previews).
pub(crate) fn truncate_summary(s: &str) -> String {
    const MAX: usize = 80;
    let one_line = s.replace(['\n', '\r', '\t'], " ");
    if one_line.chars().count() <= MAX {
        one_line
    } else {
        let head: String = one_line.chars().take(MAX.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// Project a flattened [`SessionDagNode`] (feature #2) into the tree selector's [`TreeNode`]: map the
/// UI-agnostic [`SessionDagKind`] to the render [`TreeKind`] glyph, carry depth/label/fold/leaf/label,
/// and use the leaf marker (`◀`) as the time label so the active branch tip is visible in the row.
/// Build the `/model` picker rows from the live session (feature #1): the FULL multi-provider registry
/// filtered to CONFIGURED providers (Pi `modelRegistry.getAvailable()`, model-selector.ts:152 +
/// model-registry.ts:644), each row tagged with its provider, whether it is the active model, and
/// whether it is in the scoped set (drives the `⇥` scope filter). `together` appears once
/// `TOGETHER_API_KEY` is set; the offline faux default stays selectable. Shared by the bare picker and
/// the `/model <text>` exact-match/pre-filter path so both see the identical catalog.
pub(crate) fn model_entries(session: &AgentSession) -> Vec<ModelEntry> {
    let current = session.model();
    let scoped: std::collections::HashSet<String> =
        session.scoped_models().into_iter().map(|sm| sm.model.id.to_string()).collect();
    session
        .available_model_catalog()
        .iter()
        .map(|m| ModelEntry {
            id: m.id.to_string(),
            name: m.name.clone(),
            provider: m.provider.to_string(),
            // No model selected ⇒ no row is marked current (pi renders the `/model` list against
            // the optional `session.model`).
            current: current.as_ref().is_some_and(|c| {
                m.id.as_str() == c.model.as_str() && m.provider.as_str() == c.provider.as_str()
            }),
            scoped: scoped.contains(m.id.as_str()),
        })
        .collect()
}


pub(crate) const LENGTH_STOP_NOTICE: &str = "Response was truncated before completion.";
