//! The `intercom` tool (`v0.10.1 index.ts:1826+`): `list`/`list-cwd`/`send`/`ask`/`reply`/`pending`/
//! `status` over the shared broker client. `ask` is the one blocking action (single-slot outbound waiter).

use std::sync::Arc;

use async_trait::async_trait;
use cyrup_core::{CancelToken, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};

use crate::format_context::format_context_usage;
use crate::identity::short_session_id;
use crate::inbound::format_attachments;
use crate::session_state::SharedIntercomState;
use crate::transport::client::SendOptions;
use crate::transport::protocol::{Attachment, SessionInfo, now_ms};

use super::{detailed_result, text_result};

/// The `intercom` tool.
pub struct IntercomTool {
    state: Arc<SharedIntercomState>,
    parameters: serde_json::Value,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct IntercomParams {
    action: String,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    attachments: Option<Vec<Attachment>>,
    #[serde(default)]
    reply_to: Option<String>,
    /// `messageId` (`v0.10.1 index.ts:1822-1824`) — the message the `cancel` action withdraws.
    #[serde(default)]
    message_id: Option<String>,
    /// `supersedes` (`v0.10.1 index.ts:1825-1827`) — a previous message id this `send`/`ask`
    /// explicitly replaces. The broker refuses one that does not name a message this sender already
    /// delivered to this same receiver (`v0.10.1 broker/broker.ts:525-534`).
    #[serde(default)]
    supersedes: Option<String>,
    /// `retryOf` (`v0.10.1 index.ts:1828-1830`) — a previous message id this `send`/`ask` retries.
    /// Carried on the envelope for the receiver's delivery-metadata line only; the broker does not
    /// validate it.
    #[serde(default)]
    retry_of: Option<String>,
    /// `cwd` (`v0.10.1 index.ts:1831-1833`) — the working directory to filter `list-cwd` by, and
    /// for `send`/`ask` the directory the target lookup is scoped to (omit `to` to address the sole
    /// live peer there).
    #[serde(default)]
    cwd: Option<String>,
}

/// `DeliveryTarget` (`v0.10.1 index.ts:62-66`) — the id a message is actually sent to plus the
/// label the result echoes back.
///
/// [CYRUP-DELTA] Upstream's third member `projectPane?: ProjectPaneLaunch` is absent: cyrup does
/// not port the Herdr pane launcher (see [`crate::project_target`] for the full reason). Everywhere
/// upstream branches on `target.projectPane` the cyrup code takes the non-pane arm, which is the
/// arm every pane-less call already took upstream.
struct DeliveryTarget {
    id: String,
    label: String,
}

/// `resolveCwdDeliveryTarget(activeClient, options)` (`v0.10.1 index.ts:1192-1217`), minus the
/// `openProjectPaneIfMissing` half.
///
/// The three steps upstream takes before the lookup are load-bearing and all ported: the roster is
/// fetched ONCE and reused (so `to` and `cwd` are resolved against one consistent snapshot), the
/// caller's own row is required to be in it (the target cwd defaults to *its* cwd, not to the
/// locally captured one), and a relative `cwd` resolves against that row's cwd with `"."` meaning
/// "here".
///
/// [CYRUP-DELTA] `:1221` appends `Pass openProjectPaneIfMissing: true to open a Herdr project pane
/// and start Pi there.` to the missing-target error. cyrup omits that sentence: the tool has no
/// such parameter, and telling the model to pass one the schema rejects is worse than the shorter
/// message. Everything before it is upstream's text verbatim.
/// `options.cwd && options.cwd !== "." ? resolvePath(currentSession.cwd, options.cwd) : currentSession.cwd`
/// (`v0.10.1 index.ts:1205-1207`, and the identical expression at `:1903-1907` for `list-cwd`).
///
/// `current_cwd` is the cwd the BROKER reports for this session, not the locally captured one — a
/// relative `cwd` must resolve against the directory peers can actually see this session in.
fn resolve_target_cwd(current_cwd: &str, cwd: &str) -> String {
    match cwd {
        "" | "." => current_cwd.to_string(),
        other => crate::cwd::resolve_path(std::path::Path::new(current_cwd), other)
            .to_string_lossy()
            .to_string(),
    }
}

async fn resolve_cwd_delivery_target(
    client: &crate::transport::client::IntercomClient,
    to: Option<&str>,
    cwd: &str,
) -> Result<DeliveryTarget, ToolError> {
    let sessions = client.list_sessions().await.map_err(to_tool_err)?;
    // `if (!currentSessionId) throw new Error("Current session is not registered with intercom.")`
    let Some(current_session_id) = client.session_id() else {
        return Err(ToolError::new("Current session is not registered with intercom."));
    };
    let Some(current_session) = sessions.iter().find(|s| s.id == current_session_id) else {
        return Err(ToolError::new("Current session is missing from intercom session list."));
    };
    let target_cwd = resolve_target_cwd(&current_session.cwd, cwd);
    let existing =
        crate::project_target::resolve_target_in_cwd(&sessions, &current_session_id, &target_cwd, to)
            .map_err(ToolError::new)?;
    match existing {
        crate::project_target::ProjectTargetResolution::Found { session, .. } => {
            // `options.to || existing.session.name || existing.session.id` — JS `||`, so a blank
            // `to` or a blank name falls through rather than echoing an empty label.
            let label = to
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .or_else(|| session.name.clone().filter(|n| !n.is_empty()))
                .unwrap_or_else(|| session.id.clone());
            Ok(DeliveryTarget { id: session.id.clone(), label })
        }
        crate::project_target::ProjectTargetResolution::Missing { reason, .. } => {
            Err(ToolError::new(reason))
        }
    }
}

impl IntercomTool {
    /// Build the tool over the shared session state.
    #[must_use]
    pub fn new(state: Arc<SharedIntercomState>) -> Self {
        Self { state, parameters: parameters_schema() }
    }

    async fn dispatch(&self, params: IntercomParams, cancel: &CancelToken) -> Result<ToolResult, ToolError> {
        // pi routes every tool call through `ensureConnected("tool")` (`index.ts:1477`), not a bare
        // `client` read: a tool call is worth (re)spawning the broker and reconnecting for, so a
        // single earlier connection failure does not make this tool permanently useless.
        let client = crate::connect::ensure_connected(&self.state, crate::connect::ConnectReason::Tool)
            .await
            .map_err(|e| ToolError::new(format!("intercom is not connected to the broker: {e}")))?;
        // `v0.10.1 index.ts:1853`: `syncPresenceIdentity(ctx.sessionManager.getSessionId())`
        // immediately after `ensureConnected("tool")` and before the action `match`. One of pi's
        // three name-sync points; without it a session renamed by `/name`, a branch switch or a
        // title change keeps advertising its startup label to every peer's `intercom{list}` picker.
        self.state.sync_presence_identity();

        match params.action.as_str() {
            "list" => {
                // `index.ts:1478-1507`: split into a "Current session" / "Other sessions" pair, keyed
                // off the broker-reported current session's own `cwd` (not the locally-captured one),
                // and error out (rather than silently rendering a flat list) if the broker's session
                // list is missing this session entirely.
                let self_id = client.session_id();
                let sessions = client.list_sessions().await.map_err(to_tool_err)?;
                let current_session =
                    self_id.as_deref().and_then(|id| sessions.iter().find(|s| s.id == id));
                let Some(current_session) = current_session else {
                    return Err(ToolError::new("Current session is missing from intercom session list."));
                };
                let current_cwd = current_session.cwd.clone();
                // `v0.10.1 index.ts:1872`: the addressable column is a DISTINGUISHING prefix
                // computed over the whole roster, not a fixed 8-char slice. UUIDv7 ids minted in the
                // same millisecond share far more than 8 characters, so the fixed slice printed the
                // same `(abcdef12)` for two peers — and that string was exactly what the model was
                // told to address them by.
                let prefixes = crate::identity::session_id_prefixes(sessions.iter().map(|s| s.id.as_str()));
                let id_prefix = |s: &SessionInfo| {
                    prefixes.get(&s.id).cloned().unwrap_or_else(|| short_session_id(&s.id))
                };
                let current_section = format!(
                    "**Current session:**\n{}",
                    format_session_list_row(current_session, &current_cwd, true, &id_prefix(current_session))
                );
                let other_sessions: Vec<&SessionInfo> = sessions
                    .iter()
                    .filter(|s| self_id.as_deref() != Some(s.id.as_str()))
                    .collect();
                let other_section = if other_sessions.is_empty() {
                    "**Other sessions:**\nNo other sessions connected.".to_string()
                } else {
                    format!(
                        "**Other sessions:**\n{}",
                        other_sessions
                            .iter()
                            .map(|s| format_session_list_row(s, &current_cwd, false, &id_prefix(s)))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                };
                Ok(text_result(format!("{current_section}\n\n{other_section}")))
            }
            // `v0.10.1 index.ts:1895-1941`: the same roster, filtered to one working directory —
            // the common supervisor query ("who else is in this repo?"), which was otherwise
            // unanswerable without knowing every peer's name in advance.
            "list-cwd" => {
                let self_id = client.session_id();
                let sessions = client.list_sessions().await.map_err(to_tool_err)?;
                let current_session =
                    self_id.as_deref().and_then(|id| sessions.iter().find(|s| s.id == id));
                let Some(current_session) = current_session else {
                    return Err(ToolError::new("Current session is missing from intercom session list."));
                };
                // `v0.10.1 index.ts:1903-1907`: default to the current session's cwd; an explicit
                // `cwd` overrides, with a relative path resolved AGAINST the current session's cwd
                // and `"."` meaning the current cwd.
                let filter_cwd = resolve_target_cwd(
                    &current_session.cwd,
                    params.cwd.as_deref().unwrap_or("."),
                );
                let other_sessions: Vec<&SessionInfo> = sessions
                    .iter()
                    .filter(|s| {
                        self_id.as_deref() != Some(s.id.as_str())
                            && crate::cwd::same_cwd(&s.cwd, &filter_cwd)
                    })
                    .collect();
                // `v0.10.1 index.ts:1913-1924`, comment verbatim: "Fail loud: filtering by a
                // directory with no peers while the session's OWN cwd has some otherwise reads as a
                // misleading empty result (common when a caller passes a guessed parent cwd)."
                let mut empty_note = "No other sessions in this directory.".to_string();
                if other_sessions.is_empty()
                    && !crate::cwd::same_cwd(&filter_cwd, &current_session.cwd)
                {
                    let here = sessions
                        .iter()
                        .filter(|s| {
                            self_id.as_deref() != Some(s.id.as_str())
                                && crate::cwd::same_cwd(&s.cwd, &current_session.cwd)
                        })
                        .count();
                    if here > 0 {
                        let plural = if here == 1 { "" } else { "s" };
                        empty_note.push_str(&format!(
                            " Your session's cwd is {} ({here} peer{plural} there) — call list-cwd without a cwd argument to list them.",
                            current_session.cwd
                        ));
                    }
                }
                let prefixes = crate::identity::session_id_prefixes(sessions.iter().map(|s| s.id.as_str()));
                let id_prefix = |s: &SessionInfo| {
                    prefixes.get(&s.id).cloned().unwrap_or_else(|| short_session_id(&s.id))
                };
                let current_section = format!(
                    "**Current session:**\n{}",
                    format_session_list_row(current_session, &current_session.cwd, true, &id_prefix(current_session))
                );
                let other_section = if other_sessions.is_empty() {
                    format!("**Other sessions (cwd: {filter_cwd}):**\n{empty_note}")
                } else {
                    format!(
                        "**Other sessions (cwd: {filter_cwd}):**\n{}",
                        other_sessions
                            .iter()
                            .map(|s| format_session_list_row(s, &current_session.cwd, false, &id_prefix(s)))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                };
                Ok(text_result(format!("{current_section}\n\n{other_section}")))
            }
            // ICOM-017 — `case "cancel"` (`v0.10.1 index.ts:1943-1969`). Placed here, between
            // `list-cwd` and `send`, in upstream's own order.
            //
            // This is the action that makes the whole receipt/control half reachable from the model:
            // without it a stale ask sits in the peer's `pending` list until the ask timeout, and
            // the broker's `handle_cancel_message` (ported with ICOM-010) had no caller at all.
            "cancel" => {
                let Some(message_id) = params.message_id.clone().filter(|v| !v.trim().is_empty())
                else {
                    // Upstream answers `{ text: "Missing 'messageId' parameter", details: { error:
                    // true } }` — a non-error RESULT. cyrup renders every such arm as a `ToolError`
                    // (see the identical `Missing 'to' or 'cwd', or missing 'message' parameter`
                    // guard in `send` below); the text is upstream's, byte for byte.
                    return Err(ToolError::new("Missing 'messageId' parameter"));
                };
                let result = client.cancel_message(&message_id).await.map_err(|e| {
                    // `catch (error) { … \`Failed to cancel message: ${getErrorMessage(error)}\` }`
                    // (`:1964-1968`).
                    ToolError::new(format!("Failed to cancel message: {e}"))
                })?;
                if !result.delivered {
                    // `result.reason ?? "Message may not exist or may belong to another sender."`
                    // (`:1955`) — `??`, so an empty-string reason is KEPT, unlike the `||` fallbacks
                    // elsewhere in this file.
                    let error_text = result.reason.clone().unwrap_or_else(|| {
                        "Message may not exist or may belong to another sender.".to_string()
                    });
                    return Ok(detailed_result(
                        format!("Cancellation for {message_id} was not delivered: {error_text}"),
                        serde_json::json!({
                            "messageId": message_id,
                            "delivered": false,
                            "reason": result.reason,
                        }),
                    ));
                }
                Ok(detailed_result(
                    format!("Cancellation requested for {message_id}"),
                    serde_json::json!({ "messageId": message_id, "delivered": true }),
                ))
            }
            "send" => {
                // `v0.10.1 index.ts:1973-1978`: `if ((!to && !cwd) || !message)` — ONE guard and one
                // message covering all three params, because `cwd` is an alternative addressing mode
                // rather than an extra filter. A `to`-only requirement made cross-directory
                // coordination impossible without knowing the peer's name in advance.
                let to = params.to.clone().filter(|v| !v.trim().is_empty());
                let cwd = params.cwd.clone().filter(|v| !v.trim().is_empty());
                let message = match params.message.clone().filter(|v| !v.trim().is_empty()) {
                    Some(message) if to.is_some() || cwd.is_some() => message,
                    _ => {
                        return Err(ToolError::new(
                            "Missing 'to' or 'cwd', or missing 'message' parameter",
                        ));
                    }
                };
                // `v0.10.1 index.ts:2001-2003`. With a `cwd` the target is resolved inside that
                // directory (`resolveCwdDeliveryTarget`); without one it is
                // `{ id: await resolveSessionTarget(connectedClient, to) ?? to }` — a NON-blocking
                // send that resolves to nothing is NOT refused here. It is handed to the broker as
                // the raw `to`, whose own `findSessions` gets the last word, and an unroutable
                // target comes back as the `Message to "…" was not delivered: …` result below. Only
                // the blocking `ask` refuses up front (`:2103-2110`), because an ask has a waiter to
                // hang.
                let delivery = match cwd.as_deref() {
                    Some(cwd) => resolve_cwd_delivery_target(&client, to.as_deref(), cwd).await?,
                    None => {
                        let to_value = to.clone().unwrap_or_default();
                        DeliveryTarget {
                            id: self
                                .state
                                .resolve_target(&client, &to_value)
                                .await
                                .map_err(to_tool_err)?
                                .unwrap_or_else(|| to_value.clone()),
                            label: to_value,
                        }
                    }
                };
                let DeliveryTarget { id: target, label } = delivery;
                // `const targetDisplay = target.projectPane ? target.label : to ?? target.label;`
                // (`:2004`). Pane-less, that is `to ?? target.label`: an explicit `to` is echoed
                // back verbatim, and a cwd-addressed send reports the peer's resolved name.
                let target_display = to.clone().unwrap_or(label);
                // `v0.10.1 index.ts:2005-2010` — the SAME string as the `ask` and `reply` self-guards
                // (`:2122`, `:2205`). pi has exactly one self-target message across all three arms.
                if client.session_id().as_deref() == Some(target.as_str()) {
                    return Err(ToolError::new("Cannot message the current session"));
                }
                // `v0.10.1 index.ts:2011-2012` (v0.9.3 `5d76146`, CHANGELOG 0.9.3: "Treat a public
                // send to the sole pending asker as its reply"):
                //
                //   const inferredAsk = replyTo ? null : replyTracker.findUniquePendingAskFrom(sendTo);
                //   const effectiveReplyTo = replyTo ?? inferredAsk?.message.id;
                //
                // Without this, answering a peer's ask with the natural `send` phrasing left the ask
                // pending forever: it stayed in `pending`, the flush re-injected it once the run
                // ended, and the asking peer's blocking waiter hung to the full ask timeout.
                //
                // Note the lookup is keyed on `sendTo` — the RESOLVED id — not on the caller's `to`.
                let inferred_ask = match params.reply_to {
                    Some(_) => None,
                    None => self
                        .state
                        .tracker
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .find_unique_pending_ask_from(&target, now_ms()),
                };
                let effective_reply_to = params
                    .reply_to
                    .clone()
                    .or_else(|| inferred_ask.as_ref().map(|c| c.message.id.clone()));
                // confirmSend gate (`index.ts:1524-1536`): only for a non-reply send, only when the
                // config opts in, and only when this session actually has a UI to confirm through.
                if params.reply_to.is_none() && self.state.config.confirm_send && self.state.has_ui()
                    && let Some(services) = self.state.host_services()
                {
                    let attachment_text = params
                        .attachments
                        .as_deref()
                        .filter(|a| !a.is_empty())
                        .map(format_attachments)
                        .unwrap_or_default();
                    let confirmed = services.confirm(
                        "Send Message",
                        // `Send to "${targetDisplay}"` (`v0.10.1 index.ts:2016`) — the human is
                        // asked about the peer the message will actually reach, which for a
                        // cwd-addressed send is a name they never typed.
                        &format!("Send to \"{target_display}\":\n\n{message}{attachment_text}"),
                        &cyrup_ext::DialogOptions::default(),
                    );
                    if !confirmed {
                        return Ok(text_result("Message cancelled by user"));
                    }
                }
                let result = client
                    .send(&target, SendOptions {
                        text: message.clone(),
                        attachments: params.attachments.clone(),
                        reply_to: effective_reply_to.clone(),
                        expects_reply: None,
                        message_id: None,
                        // `supersedes` / `retryOf` are threaded through `send` and `ask` only
                        // (`v0.10.1 index.ts:2029-2030`, `:2144-2145`); the `reply` arm below
                        // deliberately does NOT carry them (`:2217-2221`).
                        supersedes: params.supersedes.clone(),
                        retry_of: params.retry_of.clone(),
                    })
                    .await
                    .map_err(to_tool_err)?;
                if !result.delivered {
                    // `v0.10.1 index.ts:2032-2037`: the failure names the target and keeps pi's
                    // fallback reason. A bare reason string tells the model nothing about which of
                    // several in-flight sends failed.
                    let reason = result
                        .reason
                        .unwrap_or_else(|| "Session may not exist or has disconnected.".to_string());
                    return Err(ToolError::new(format!(
                        "Message to \"{target_display}\" was not delivered: {reason}"
                    )));
                }
                // `index.ts:1549-1557`: the audit entry + markReplied both run ONLY after a confirmed
                // delivery — a failed/undelivered send must leave the original inbound ask pending.
                if let Some(services) = self.state.host_services() {
                    let _ = services.append_entry(
                        "intercom_sent",
                        &serde_json::json!({
                            "to": target_display,
                            "message": {
                                "text": message,
                                "attachments": params.attachments,
                                "replyTo": effective_reply_to,
                            },
                            "messageId": result.id,
                            "timestamp": now_ms(),
                        }),
                    );
                }
                if let Some(reply_to) = &effective_reply_to {
                    // `v0.10.1 index.ts:2044-2046` is `dismissIncomingAsk(effectiveReplyTo)`, NOT a
                    // bare `dismissPendingAsk`: the answered inbound message must also leave the
                    // pending-idle queue, or the flush re-injects it once this run ends.
                    crate::inbound::dismiss_incoming_ask(&self.state, reply_to);
                }
                // `v0.10.1 index.ts:2051-2054`: `Message sent to ${targetDisplay}` — the
                // CALLER-SUPPLIED target, not the resolved id, and with NO trailing period. pi
                // deliberately splits the two (`const sendTo = await resolveSessionTarget(…) ?? to`,
                // `:2002`): it delivers to `sendTo` but reports `to`, so a send addressed to
                // `reviewer` echoes back `reviewer` rather than the raw UUID the name resolved to.
                // When the reply target was INFERRED the result says so, because the model needs to
                // know its plain send just closed an ask.
                // `v0.10.1 index.ts:2054-2060`: `{ messageId, delivered: true, ...(effectiveReplyTo
                // ? { replyTo: effectiveReplyTo } : {}) }` — the spread means `replyTo` is OMITTED,
                // not null, when the send was not a reply.
                let mut details = serde_json::json!({ "messageId": result.id, "delivered": true });
                if let Some(reply_to) = &effective_reply_to
                    && let Some(map) = details.as_object_mut()
                {
                    map.insert("replyTo".to_string(), serde_json::json!(reply_to));
                }
                Ok(detailed_result(
                    if inferred_ask.is_some() {
                        format!("Reply sent to {target_display} (inferred from pending ask)")
                    } else {
                        format!("Message sent to {target_display}")
                    },
                    details,
                ))
            }
            "ask" => {
                // `v0.10.1 index.ts:2071-2076`: the same single `(!to && !cwd) || !message` guard
                // and the same message as `send`.
                let to = params.to.clone().filter(|v| !v.trim().is_empty());
                let cwd = params.cwd.clone().filter(|v| !v.trim().is_empty());
                let message = match params.message.clone().filter(|v| !v.trim().is_empty()) {
                    Some(message) if to.is_some() || cwd.is_some() => message,
                    _ => {
                        return Err(ToolError::new(
                            "Missing 'to' or 'cwd', or missing 'message' parameter",
                        ));
                    }
                };
                // `v0.10.1 index.ts:2103-2114`. With a `cwd` the target is resolved inside that
                // directory, and `resolveCwdDeliveryTarget`'s own "no session there" error already
                // says what happened — the offline refusal below belongs to the `to`-only branch.
                let delivery = match cwd.as_deref() {
                    Some(cwd) => resolve_cwd_delivery_target(&client, to.as_deref(), cwd).await?,
                    None => {
                        let to_value = to.clone().unwrap_or_default();
                        // `v0.10.1 index.ts:2107-2113` (v0.10.0): an ask whose target is offline is
                        // refused UP FRONT with the actionable text, not with the bare
                        // `Session not found: "x"` the shared resolver produces — a blocking ask is
                        // not queued anywhere, so the model has to be told to use `send` or to retry
                        // after the peer reconnects.
                        let Some(resolved) =
                            self.state.resolve_target(&client, &to_value).await.map_err(to_tool_err)?
                        else {
                            return Err(ToolError::new(format!(
                                "Session \"{to_value}\" is not currently connected. Blocking asks are not queued; use send for a non-blocking mailbox delivery or retry after the session reconnects."
                            )));
                        };
                        DeliveryTarget { id: resolved, label: to_value }
                    }
                };
                let DeliveryTarget { id: target, label } = delivery;
                // `const targetDisplay = target.projectPane ? target.label : to ?? target.label;`
                // (`v0.10.1 index.ts:2116`).
                let to = to.unwrap_or(label);
                // `v0.10.1 index.ts:2122-2127` — pi's single self-target string, shared with `send`
                // and `reply`.
                if client.session_id().as_deref() == Some(target.as_str()) {
                    return Err(ToolError::new("Cannot message the current session"));
                }
                let question_id = uuid::Uuid::new_v4().to_string();
                // `v0.10.1 index.ts:2135-2143`: the ask's own send carries the caller's `replyTo`.
                // Dropping it made a counter-ask against a peer's pending ask fail against cyrup's
                // OWN broker with `Reply target does not match a pending ask`.
                let reply_message = self
                    .state
                    .ask_and_wait_with_reply_to(
                        &client,
                        &target,
                        question_id.clone(),
                        message.clone(),
                        params.attachments.clone(),
                        params.reply_to.clone(),
                        params.supersedes.clone(),
                        params.retry_of.clone(),
                        cancel,
                    )
                    .await
                    .map_err(to_tool_err)?;
                let reply_text = reply_message.content.text.clone();
                let reply_attachments = reply_message
                    .content
                    .attachments
                    .as_deref()
                    .filter(|a| !a.is_empty())
                    .map(format_attachments)
                    .unwrap_or_default();
                // `v0.10.1 index.ts:2161-2176`: an audit entry for both the outbound ask and the
                // inbound reply. `ask_and_wait` sends with `message_id: Some(question_id)`, so the
                // delivered send's id is exactly `question_id` (`transport::client::send`,
                // `client.rs:200-204`).
                if let Some(services) = self.state.host_services() {
                    let _ = services.append_entry(
                        "intercom_sent",
                        &serde_json::json!({
                            "to": to,
                            "message": {
                                "text": message,
                                "attachments": params.attachments,
                                "replyTo": params.reply_to,
                            },
                            "messageId": question_id,
                            "timestamp": now_ms(),
                        }),
                    );
                    // `v0.10.1 index.ts:2171-2176`. Three things this entry MUST carry that it used
                    // to drop: the reply's own `messageId`, its `attachments`, and the SENDER's
                    // timestamp (not the local receipt time). The durable record of an exchange has
                    // to match what was exchanged, or the loss is undiscoverable afterwards.
                    let _ = services.append_entry(
                        "intercom_received",
                        &serde_json::json!({
                            "from": to,
                            "message": {
                                "text": reply_text,
                                "attachments": reply_message.content.attachments,
                            },
                            "messageId": reply_message.id,
                            "timestamp": reply_message.timestamp,
                        }),
                    );
                }
                // `v0.10.1 index.ts:2180`: `**Reply from ${targetDisplay}:**\n${replyText}${replyAttachments}`,
                // keyed off the caller-supplied `to`. Without the header a transcript that has asked
                // more than one peer cannot tell which of them answered.
                Ok(text_result(format!("**Reply from {to}:**\n{reply_text}{reply_attachments}")))
            }
            "reply" => {
                let message = require(params.message, "reply requires `message`")?;
                let now = now_ms();
                let target = {
                    let mut tracker = self.state.tracker.lock().unwrap_or_else(|e| e.into_inner());
                    tracker
                        .resolve_reply_target(params.to.as_deref(), params.reply_to.as_deref(), now)
                        .map_err(ToolError::new)?
                };
                // Self-target guard (`index.ts:1686-1691`): a resolved reply target may legitimately
                // be this session's own id (e.g. a stale/misrouted ask entry); pi refuses to loop a
                // reply back to itself.
                if client.session_id().as_deref() == Some(target.from.id.as_str()) {
                    return Err(ToolError::new("Cannot message the current session"));
                }
                // `v0.10.1 index.ts:2211-2215` (v0.10.1 `2ba9f53`, "fix: preserve reply attachments
                // (#100)"): `attachments` is threaded through, not dropped. Before this a reply
                // carrying a file/snippet sent the prose and silently lost the payload — and the
                // audit entry below recorded the same lie.
                let result = client
                    .send(&target.from.id, SendOptions {
                        text: message.clone(),
                        attachments: params.attachments.clone(),
                        reply_to: Some(target.message.id.clone()),
                        expects_reply: None,
                        message_id: None,
                        supersedes: None,
                        retry_of: None,
                    })
                    .await
                    .map_err(to_tool_err)?;
                // `index.ts:1692-1706`: the dismissal runs ONLY on a confirmed delivery (`:1718`); a
                // failed delivery still dismisses when the reason is exactly "Session not found"
                // (`:1711`, the peer is gone — the ask can never be answered) but for any other
                // failure reason leaves the state untouched entirely so the ask remains pending for
                // a retry. Both live branches are pi's `dismissIncomingAsk`, which ALSO splices the
                // message out of the pending-idle queue — without that, replying to a message that
                // arrived mid-run gets it re-injected by the flush when the run ends.
                if result.delivered || result.reason.as_deref() == Some("Session not found") {
                    crate::inbound::dismiss_incoming_ask(&self.state, &target.message.id);
                }
                if result.delivered {
                    if let Some(services) = self.state.host_services() {
                        let _ = services.append_entry(
                            "intercom_sent",
                            &serde_json::json!({
                                "to": target.from.name.clone().unwrap_or_else(|| target.from.id.clone()),
                                "message": {
                                    "text": message,
                                    "attachments": params.attachments,
                                    "replyTo": target.message.id,
                                },
                                "messageId": result.id,
                                "timestamp": now_ms(),
                            }),
                        );
                    }
                    // `v0.10.1 index.ts:2233`: `Reply sent to ${target.from.name || target.from.id}`
                    // — name preferred over id (JS `||`, so a blank name falls through to the id),
                    // and NO trailing period. `:2234` carries `{ messageId, delivered: true,
                    // replyTo: target.message.id }` — here `replyTo` is unconditional, unlike the
                    // `send` arm's spread, because a reply always has one.
                    Ok(detailed_result(
                        format!("Reply sent to {}", display_name(&target.from)),
                        serde_json::json!({
                            "messageId": result.id,
                            "delivered": true,
                            "replyTo": target.message.id,
                        }),
                    ))
                } else {
                    // `v0.10.1 index.ts:2222-2225`: the failure names the peer and keeps pi's
                    // fallback reason.
                    let reason = result
                        .reason
                        .unwrap_or_else(|| "Session may not exist or has disconnected.".to_string());
                    Err(ToolError::new(format!(
                        "Reply to \"{}\" was not delivered: {reason}",
                        display_name(&target.from)
                    )))
                }
            }
            "pending" => {
                let now = now_ms();
                let pending = self
                    .state
                    .tracker
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .list_pending(now);
                if pending.is_empty() {
                    return Ok(text_result("No unresolved inbound asks."));
                }
                // pi `index.ts:1747-1751`:
                // `- ${from.name || from.id} · ${message.id} · ${elapsedSeconds}s ago · ${preview}`
                //
                // The MESSAGE ID is the load-bearing column. `reply_tracker.rs:126` refuses a
                // sender-targeted reply with `Multiple pending asks from "{x}" — use the message id`
                // (upstream's own wording), and the tool documents `replyTo` as the escape. This row
                // used to print the sender's SESSION short-id instead, which is not a valid
                // `replyTo`, so once two asks shared a sender the model was told to use an id that
                // nothing in its output ever showed — an unbreakable loop of failing replies.
                //
                // `preview` collapses whitespace and truncates to 80 chars; the body was previously
                // emitted whole, so one long inbound ask could flood the tool result.
                let rows: Vec<String> = pending
                    .iter()
                    .map(|c| {
                        let who = c.from.name.clone().unwrap_or_else(|| c.from.id.clone());
                        let preview: String = c
                            .message
                            .content
                            .text
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                            .chars()
                            .take(80)
                            .collect();
                        let elapsed = now.saturating_sub(c.received_at) / 1000;
                        format!("- {} · {} · {}s ago · {}", who, c.message.id, elapsed, preview)
                    })
                    .collect();
                Ok(text_result(format!("**Pending asks:**\n{}", rows.join("\n"))))
            }
            "status" => {
                // `index.ts:1765`: a four-line markdown block, not a pipe-delimited one-liner.
                // `Connected: Yes` is a literal upstream — the branch only runs after
                // `ensureConnected` has already succeeded (here: `connect::ensure_connected` above),
                // so there is no "disconnected" rendering to reach. A failing `listSessions` is
                // upstream's `Failed to get status: …` error result, which this crate renders as a
                // `ToolError` throughout (cf. the `list` branch's `Failed to list sessions`).
                let session_id = client.session_id().unwrap_or_else(|| "<none>".to_string());
                let count = client.list_sessions().await.map_err(to_tool_err)?.len();
                Ok(text_result(format!(
                    "**Intercom Status:**\nConnected: Yes\nSession ID: {session_id}\nActive sessions: {count}"
                )))
            }
            other => Err(ToolError::new(format!("unknown intercom action \"{other}\""))),
        }
    }

}

fn require(value: Option<String>, msg: &str) -> Result<String, ToolError> {
    match value {
        Some(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(ToolError::new(msg.to_string())),
    }
}

fn to_tool_err(e: crate::error::IntercomError) -> ToolError {
    ToolError::new(e.to_string())
}

/// `session.name || session.id` (`index.ts:1720,1726`). JS `||` is falsy-based, so an empty name
/// falls through to the id — hence the `filter(|n| !n.is_empty())`.
fn display_name(session: &SessionInfo) -> &str {
    session.name.as_deref().filter(|n| !n.is_empty()).unwrap_or(&session.id)
}

/// `formatSessionListRow` (`v0.10.1 index.ts:448-453`, 6 lines):
///
/// ```text
/// return `• ${name} (${idPrefix}) — ${session.cwd} (${session.model}${formatContextUsage(session)})${suffix}`;
/// ```
///
/// `idPrefix` is a **fourth argument** from v0.9.3 (`72309e0`) — [`crate::identity::session_id_prefixes`]
/// computed once per `list` call over the whole roster, replacing the fixed `shortSessionId(session.id)`
/// that used to sit here. `short_session_id` survives only for the picker label
/// (`formatSessionLabel`, `v0.10.1 index.ts:440-446`), which upstream deliberately kept at 8.
///
/// The `formatContextUsage(session)` term sits INSIDE the model parentheses (`v0.9.2 index.ts:428`)
/// and is the only place upstream surfaces a peer's context usage — `ui/session-list.ts` does not.
/// It renders the empty string whenever `contextPct` is absent, so a peer that reports nothing is
/// byte-for-byte the pre-v0.8.0 row.
fn format_session_list_row(
    session: &SessionInfo,
    current_cwd: &str,
    is_self: bool,
    id_prefix: &str,
) -> String {
    let name = session
        .name
        .as_deref()
        .filter(|n| !n.is_empty())
        .unwrap_or("Unnamed session");
    let mut tags: Vec<String> = Vec::new();
    if is_self {
        tags.push("self".to_string());
    } else if crate::cwd::same_cwd(&session.cwd, current_cwd) {
        // `sameCwd(...)` (`v0.10.1 cwd.ts:29-31`), not a raw byte compare: `/w` and `/w/`, or a
        // symlinked vs realpath'd cwd, are the SAME project, and a byte compare marked every
        // session started through a symlink as a different one.
        tags.push("same cwd".to_string());
    }
    if let Some(status) = &session.status {
        tags.push(status.clone());
    }
    let suffix = if tags.is_empty() { String::new() } else { format!(" [{}]", tags.join(", ")) };
    format!(
        "• {} ({}) — {} ({}{}){}",
        name,
        id_prefix,
        session.cwd,
        session.model,
        format_context_usage(session),
        suffix
    )
}

fn parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["list", "list-cwd", "send", "ask", "reply", "pending", "status", "cancel"],
                "description": "The intercom action to perform."
            },
            // `v0.10.1 index.ts:1831-1833`, minus the sentence about `openProjectPaneIfMissing`
            // (see `resolve_cwd_delivery_target`'s [CYRUP-DELTA]).
            "cwd": {
                "type": "string",
                "description": "Working directory filter for 'list-cwd'. For send/ask, scopes target lookup to that directory; omit 'to' to target the sole live peer there. Absolute, or relative to the current session's cwd; '.' means the current cwd."
            },
            "to": { "type": "string", "description": "Target session name or id (send/ask/reply). Optional for send/ask when 'cwd' is given." },
            "message": { "type": "string", "description": "Message text (send/ask/reply)." },
            "attachments": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "type": { "type": "string", "enum": ["file", "snippet", "context"] },
                        "name": { "type": "string" },
                        "content": { "type": "string" },
                        "language": { "type": "string" }
                    },
                    "required": ["type", "name", "content"]
                }
            },
            "replyTo": { "type": "string", "description": "The ask message id this replies to (reply)." },
            // `v0.10.1 index.ts:1822-1830`, descriptions verbatim.
            "messageId": { "type": "string", "description": "Message ID for actions that operate on an existing message, such as 'cancel'." },
            "supersedes": { "type": "string", "description": "Previous message ID this send/ask explicitly supersedes. Only works for the same sender and receiver." },
            "retryOf": { "type": "string", "description": "Previous message ID this send/ask is a user-authored retry of. Retries always send a new message ID." }
        },
        "required": ["action"]
    })
}

#[async_trait]
impl Tool for IntercomTool {
    fn name(&self) -> &str {
        "intercom"
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }

    fn description(&self) -> &str {
        "Coordinate with other local agent sessions over the intercom broker: list/list-cwd/send/ask/reply/pending/status/cancel."
    }

    /// `label: "Intercom"` (`v0.10.1 index.ts:1781`).
    ///
    /// Not decoration: the tool-row UI falls back to the raw `name()` when this is `None`, so
    /// omitting it renders `intercom` where upstream renders `Intercom`.
    fn label(&self) -> Option<&str> {
        Some("Intercom")
    }

    /// `promptSnippet` (`v0.10.1 index.ts:1800-1801`), verbatim except for the product name.
    ///
    /// This is the ONLY thing that puts a tool into the default system prompt's "Available tools"
    /// section — upstream builds that list with `tools.filter(name => !!toolSnippets?.[name])`, so a
    /// `None` here means the model is never told in prose that `intercom` exists.
    ///
    /// [CYRUP-DELTA] `v0.10.1 index.ts:1801` reads "other local pi sessions"; this is the same
    /// product-name substitution the whole port applies (`.pi/agent` → `.cyrup`, `PI_*` →
    /// `CYRUP_*`), and the sentence names sessions of the running agent, not of a foreign tool.
    fn prompt_snippet(&self) -> Option<&str> {
        Some(
            "Use to coordinate with other local cyrup sessions: list peers, send updates, ask for \
             help, or check intercom connectivity.",
        )
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let parsed: IntercomParams = serde_json::from_value(params)
            .map_err(|e| ToolError::new(format!("invalid intercom tool call: {e}")))?;
        self.dispatch(parsed, &cancel).await
    }
}

/// Unit tests only. The nine action-level proofs that used to live here — `list`/`send`/`ask`/
/// `reply`/`status` driven end to end — each spawned the real `cyrup-intercom-broker` binary as a
/// subprocess, which makes them seam tests; they now live in
/// `crates/cyrup-it/tests/intercom/tool_actions.rs`, where `build.rs` resolves that binary instead
/// of a `current_exe()`-relative guess. See docs/TEST-ARCHITECTURE.md §9.1.
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

    use crate::transport::protocol::{Message, MessageContent};

    use super::*;

    /// The `intercom` tool's PROMPT SURFACE — the three `Tool` accessors that default to
    /// `None`/`None`/`Vec::new()` (`cyrup-core/src/tool.rs`) and therefore compile, run and look
    /// correct while contributing nothing.
    ///
    /// `prompt_snippet` is the sole gate on the default system prompt's "Available tools" section
    /// (`tools.filter(name => !!toolSnippets?.[name])`): `None` means the model is never told in
    /// prose that this tool exists. `label` is the tool-row UI's display name; `None` falls back to
    /// the raw `name()`. Pinned against `v0.10.1 index.ts:1780-1801` — which declares `label` and
    /// `promptSnippet` and, deliberately, NO `promptGuidelines`.
    #[test]
    fn the_intercom_tool_declares_pis_label_and_prompt_snippet() {
        let tool = IntercomTool::new(Arc::new(SharedIntercomState::new(
            crate::config::IntercomConfig::default(),
            600_000,
            std::path::PathBuf::from("/w"),
        )));

        assert_eq!(tool.label(), Some("Intercom"), "`v0.10.1 index.ts:1781` `label: \"Intercom\"`");
        assert_eq!(
            tool.prompt_snippet(),
            Some(
                "Use to coordinate with other local cyrup sessions: list peers, send updates, ask \
                 for help, or check intercom connectivity."
            ),
            "`v0.10.1 index.ts:1800-1801` verbatim, with the port's product-name substitution"
        );
        // Absence is load-bearing too: upstream gives `intercom` no `promptGuidelines`, so a future
        // edit that invents some is a divergence, not an improvement.
        assert!(
            tool.prompt_guidelines().is_empty(),
            "`v0.10.1 index.ts:1779-1802` declares no promptGuidelines for `intercom`"
        );
    }

    /// ICOM-017 — the tool's SCHEMA is what decides whether the model can reach the cancel path at
    /// all, and it is the half that stayed unported after the broker's `handle_cancel_message`
    /// landed: `cancel_message` had no caller, so the broker code was dead.
    ///
    /// Pinned against `v0.10.1 index.ts:1810-1830`: the action enum ends with `"cancel"` in pi's own
    /// order, and the three message-id parameters exist with pi's descriptions. A `cancel` arm
    /// without the enum entry is unreachable (the agent preflight validates the call against this
    /// schema before `execute` runs), and `messageId` without the enum entry is decoration.
    #[test]
    fn the_schema_advertises_cancel_and_the_three_message_id_parameters() {
        let schema = parameters_schema();
        let actions: Vec<&str> = schema["properties"]["action"]["enum"]
            .as_array()
            .expect("the action property must carry an enum")
            .iter()
            .map(|v| v.as_str().expect("every action must be a string"))
            .collect();
        // Presence before absence: assert the WHOLE list, in pi's order, so a rewrite that drops an
        // existing action to add `cancel` is red too.
        assert_eq!(
            actions,
            vec!["list", "list-cwd", "send", "ask", "reply", "pending", "status", "cancel"],
            "`v0.10.1 index.ts:1810-1812` — pi's enum, in pi's order, with `cancel` last"
        );
        for (key, needle) in [
            ("messageId", "such as 'cancel'"),
            ("supersedes", "explicitly supersedes"),
            ("retryOf", "user-authored retry"),
        ] {
            let description = schema["properties"][key]["description"]
                .as_str()
                .unwrap_or_else(|| panic!("`{key}` must be declared with a description"));
            assert!(
                description.contains(needle),
                "`{key}`'s description must be pi's ({needle:?}); got {description:?}"
            );
        }
    }

    /// pi `index.ts:1747-1751`. The MESSAGE ID is the load-bearing column, not decoration:
    /// `reply_tracker.rs:126` refuses a sender-targeted reply with upstream's own wording
    /// `Multiple pending asks from "{x}" — use the message id`, and the tool documents `replyTo` as
    /// the escape hatch. This row previously printed the sender's SESSION short-id, which is not a
    /// valid `replyTo` — so once two asks shared a sender the model was told to use an id that
    /// nothing in its own output had ever shown it, and every reply attempt failed.
    #[test]
    fn pending_rows_carry_the_message_id_so_reply_to_is_reachable() {
        let mut tracker = crate::reply_tracker::ReplyTracker::new(600_000);
        let now = now_ms();
        // Two asks from the SAME sender: the exact case that forces `replyTo`.
        tracker.record_incoming_message(session("s1", "/tmp/a"), ask_message("m-first"), now);
        tracker.record_incoming_message(session("s1", "/tmp/a"), ask_message("m-second"), now);

        let pending = tracker.list_pending(now);
        assert_eq!(pending.len(), 2, "both asks are pending");

        let rows: Vec<String> = pending
            .iter()
            .map(|c| {
                let who = c.from.name.clone().unwrap_or_else(|| c.from.id.clone());
                format!("- {} · {} · 0s ago · {}", who, c.message.id, c.message.content.text)
            })
            .collect();
        let rendered = format!("**Pending asks:**\n{}", rows.join("\n"));

        assert!(rendered.starts_with("**Pending asks:**"), "pi's header");
        for id in ["m-first", "m-second"] {
            assert!(
                rendered.contains(id),
                "every row must name its message id so `replyTo: {id}` is reachable:\n{rendered}"
            );
        }
    }

    /// pi collapses whitespace and slices the preview to 80 chars (`index.ts:1748`); the body used
    /// to be emitted whole, so one long inbound ask could flood the tool result.
    #[test]
    fn a_long_pending_body_is_whitespace_collapsed_and_truncated_to_80_chars() {
        let raw = format!("word\n\tspaced   out {}", "x".repeat(200));
        let preview: String = raw
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(80)
            .collect();
        assert_eq!(preview.chars().count(), 80, "sliced to 80 chars");
        assert!(preview.starts_with("word spaced out "), "whitespace collapsed: {preview:?}");
        assert!(!preview.contains('\n') && !preview.contains('\t'));
    }

    /// ICOM-039 (`v0.10.1 index.ts:448-453` + `:387-406`): the addressable column is a
    /// DISTINGUISHING prefix over the whole roster, not `short_session_id`'s fixed 8 chars.
    ///
    /// Red against the pre-fix row builder: two UUIDv7 ids minted in the same millisecond share far
    /// more than 8 leading characters, so both rows printed the identical `(0192f3c1)` — and that
    /// string is exactly what the model is told to address them by, which then failed with
    /// `Multiple sessions match …`.
    #[test]
    fn list_rows_print_distinguishing_id_prefixes_not_a_fixed_slice() {
        let a = session("0192f3c1-9a10-7000-8000-aaaaaaaaaaaa", "/w");
        let b = session("0192f3c1-9a10-7000-8000-bbbbbbbbbbbb", "/w");
        let ids = [a.id.as_str(), b.id.as_str()];
        let prefixes = crate::identity::session_id_prefixes(ids);

        let row_a = format_session_list_row(&a, "/w", false, prefixes.get(&a.id).expect("a"));
        let row_b = format_session_list_row(&b, "/w", false, prefixes.get(&b.id).expect("b"));
        assert!(row_a.contains("(0192f3c1-9a10-7000-8000-a)"), "{row_a}");
        assert!(row_b.contains("(0192f3c1-9a10-7000-8000-b)"), "{row_b}");
        assert_ne!(row_a, row_b, "two peers must not print the same addressable id");
        // The fixed 8-char slice — which upstream deliberately KEPT for the picker label
        // (`formatSessionLabel`, `v0.10.1 index.ts:440-446`) — would have collided.
        assert_eq!(short_session_id(&a.id), short_session_id(&b.id));
    }

    /// ICOM-018 (`v0.10.1 cwd.ts:29-31`): the "same cwd" tag is a NORMALIZED comparison, so a peer
    /// whose cwd differs only by a trailing slash is still the same project. The raw byte compare
    /// this replaced marked every symlink-started session as a different one.
    #[test]
    fn the_same_cwd_tag_normalizes_before_comparing() {
        let peer = session("peer-1", "/definitely/not/here/");
        let row = format_session_list_row(&peer, "/definitely/not/here", false, "peer-1");
        assert!(row.contains("[same cwd]"), "{row}");
    }

    /// ICOM-042 / `v0.10.1 index.ts:1205-1207`. `send`/`ask`/`list-cwd` share ONE target-cwd rule:
    /// omitted or `"."` means the current session's own broker-reported cwd, a relative path
    /// resolves against it, an absolute path replaces it.
    #[test]
    fn target_cwd_defaults_to_the_current_session_and_resolves_relatives_against_it() {
        assert_eq!(resolve_target_cwd("/w/proj", "."), "/w/proj");
        assert_eq!(resolve_target_cwd("/w/proj", ""), "/w/proj");
        assert_eq!(resolve_target_cwd("/w/proj", "sub"), "/w/proj/sub");
        assert_eq!(resolve_target_cwd("/w/proj", "../other"), "/w/other");
        assert_eq!(resolve_target_cwd("/w/proj", "/abs"), "/abs");
    }

    /// ICOM-042 / `v0.10.1 index.ts:1214`. The echoed label is `to || session.name || session.id`
    /// with JS-falsy fallthrough, which is what `send`'s `targetDisplay` reports back to the model.
    /// A blank `to` and a blank name must both fall through rather than echo an empty string.
    #[test]
    fn cwd_delivery_label_falls_through_blank_to_and_blank_name() {
        let label = |to: Option<&str>, name: Option<&str>| {
            let peer = SessionInfo { name: name.map(str::to_string), ..session("peer-1", "/w/proj") };
            to.map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .or_else(|| peer.name.clone().filter(|n| !n.is_empty()))
                .unwrap_or_else(|| peer.id.clone())
        };
        assert_eq!(label(Some("reviewer"), Some("worker")), "reviewer");
        assert_eq!(label(None, Some("worker")), "worker");
        assert_eq!(label(Some("   "), Some("worker")), "worker");
        assert_eq!(label(None, Some("")), "peer-1");
        assert_eq!(label(None, None), "peer-1");
    }

    fn session(id: &str, cwd: &str) -> SessionInfo {
        SessionInfo {
            id: id.to_string(),
            name: Some(id.to_string()),
            runtime_fallback_alias: None,
            cwd: cwd.to_string(),
            model: "m".to_string(),
            pid: 1u32.into(),
            started_at: now_ms().into(),
            last_activity: now_ms().into(),
            status: None,
            peer_uid: None,
            trusted_local: None,
            context_pct: None,
            context_tokens: None,
            context_window: None,
            extra: Default::default(),
        }
    }

    fn ask_message(id: &str) -> Message {
        Message {
            id: id.to_string(),
            timestamp: now_ms().into(),
            reply_to: None,
            expects_reply: Some(true),
            content: MessageContent { text: "hi".to_string(), attachments: None, ..Default::default() },
            ..Default::default()
        }
    }
}
