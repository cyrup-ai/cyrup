//! `deliverMessage` (`v0.16.0 index.ts:1645-1812`) — the ONE delivery path `send` and `handover`
//! share: restriction check, `@`-target parse, confirm gating, the cross-machine arm, local
//! resolution, the self-target and active-ask-turn guards, the inferred-reply inference, the
//! confirm dialog, the send, the audit entry and the result.
//!
//! Upstream names it "Shared by send, handover, and /handover. Never throws; failures are returned
//! as error results." Two kinds of failure are distinguished here for the same reason upstream's
//! `try` does: a failure upstream RETURNS (a refusal, an undelivered ack) is reported verbatim, and
//! a failure upstream THROWS (a roster fetch, a socket error, a pane that never started) reaches its
//! `catch` and is reported as `Failed to send: <reason>` ([`DeliveryFailure`]).
//!
//! ## What `handover` changes, and nothing else (`request.handover`, read at `:1705`, `:1754`,
//! `:1768`)
//!
//! * The pending-ask inference is suppressed (`replyTo || request.handover ? null :
//!   findUniquePendingAskFrom`). A handover is a NEW message; it must not be taken for the answer
//!   to an ask the target happens to have pending.
//! * `signal.aborted` is re-checked immediately before the message leaves, answering `Handover was
//!   cancelled before delivery.`: on the cross-machine path after the confirm and BEFORE the
//!   origin identity is built (`:1705`), and on the local path after the confirm and immediately
//!   before `client.send` (`:1768`). The summary was generated at the caller's expense; a cancel
//!   that arrives while a dialog was open must stop it from being sent.
//!
//! The active-ask-turn mismatch guard (`:1744`) still applies to a handover — it keys on `replyTo`,
//! which a handover never has — so a turn answering peer A's ask cannot hand over to peer B.

use std::sync::Arc;

use cyrup_core::{CancelToken, ToolError, ToolResult};

use crate::handover::HandoverRefusal;
use crate::inbound::format_attachments;
use crate::tools::{detailed_result, text_result};
use crate::transport::client::{IntercomClient, SendOptions};
use crate::transport::protocol::{Attachment, now_ms};

use super::cross_machine::deliver_cross_machine;
use super::{
    CwdDeliveryOptions, DeliveryTarget, IntercomTool, explicit_cross_machine_send_restriction,
    resolve_cwd_delivery_target, to_tool_err,
};

/// The confirm dialog's title at all three upstream sites (`v0.16.0 index.ts:1685,:1696,:1758`).
pub(crate) const CONFIRM_TITLE: &str = "Send message";

/// What is being delivered, which decides which of [`DeliveryRequest`]'s relationships to another
/// message exist at all. A handover has none, so none are representable for it.
pub(super) enum DeliveryKind<'a> {
    /// `intercom({ action: "send" })`.
    Send {
        attachments: Option<&'a [Attachment]>,
        reply_to: Option<&'a str>,
        supersedes: Option<&'a str>,
        retry_of: Option<&'a str>,
    },
    /// `intercom({ action: "handover" })` — `DeliveryRequest.handover: true` (`:93-94`).
    Handover,
}

impl<'a> DeliveryKind<'a> {
    fn is_handover(&self) -> bool {
        matches!(self, Self::Handover)
    }

    fn attachments(&self) -> Option<&'a [Attachment]> {
        match self {
            Self::Send { attachments, .. } => *attachments,
            Self::Handover => None,
        }
    }

    fn reply_to(&self) -> Option<&'a str> {
        match self {
            Self::Send { reply_to, .. } => *reply_to,
            Self::Handover => None,
        }
    }

    fn supersedes(&self) -> Option<&'a str> {
        match self {
            Self::Send { supersedes, .. } => *supersedes,
            Self::Handover => None,
        }
    }

    fn retry_of(&self) -> Option<&'a str> {
        match self {
            Self::Send { retry_of, .. } => *retry_of,
            Self::Handover => None,
        }
    }
}

/// `DeliveryRequest` (`v0.16.0 index.ts:83-95`), with the five message-relationship fields folded
/// into [`DeliveryKind`].
///
/// `to` and `cwd` are already blank-filtered by the action arm, and `message` is the final text
/// (for a handover, the framed summary).
pub(super) struct DeliveryRequest<'a> {
    pub(super) to: Option<&'a str>,
    pub(super) cwd: Option<&'a str>,
    pub(super) open_project_pane_if_missing: bool,
    /// Already defaulted: `params.focus.unwrap_or(true)`.
    pub(super) focus: bool,
    pub(super) message: &'a str,
    pub(super) kind: DeliveryKind<'a>,
}

/// Why a delivery did not complete, split the way `deliverMessage`'s `try` splits it.
enum DeliveryFailure {
    /// Upstream `return`s an error result with exactly this text.
    Returned(ToolError),
    /// Upstream `throw`s, and its `catch` reports `Failed to send: <message>`.
    Thrown(ToolError),
}

impl DeliveryFailure {
    fn returned(message: impl Into<String>) -> Self {
        Self::Returned(ToolError::new(message))
    }

    fn thrown(error: impl Into<ToolError>) -> Self {
        Self::Thrown(error.into())
    }
}

impl From<DeliveryFailure> for ToolError {
    fn from(failure: DeliveryFailure) -> Self {
        match failure {
            DeliveryFailure::Returned(error) => error,
            DeliveryFailure::Thrown(error) => {
                Self::new(format!("Failed to send: {}", error.message))
            }
        }
    }
}

impl IntercomTool {
    /// `deliverMessage(connectedClient, ctx, signal, request)` (`v0.16.0 index.ts:1645-1812`).
    pub(super) async fn deliver_message(
        &self,
        client: &Arc<IntercomClient>,
        cancel: &CancelToken,
        request: DeliveryRequest<'_>,
    ) -> Result<ToolResult, ToolError> {
        self.attempt_delivery(client, cancel, &request)
            .await
            .map_err(ToolError::from)
    }

    async fn attempt_delivery(
        &self,
        client: &Arc<IntercomClient>,
        cancel: &CancelToken,
        request: &DeliveryRequest<'_>,
    ) -> Result<ToolResult, DeliveryFailure> {
        let DeliveryRequest {
            to,
            cwd,
            open_project_pane_if_missing: open_pane,
            focus,
            message,
            ref kind,
        } = *request;
        let handover = kind.is_handover();
        let reply_to = kind.reply_to();

        // `v0.16.0 index.ts:1655-1673`, the FIRST thing `deliverMessage` does. A `to` containing
        // `@` is a cross-machine target, and the two refusals below are checked before anything is
        // resolved, confirmed or spawned: the restriction, then the target's shape.
        //
        // The shape check is not redundant with `discover_remote_agent`'s identical call. Upstream
        // runs it here too (`:1666`) because `parseCrossMachineTarget` refuses `a@b@c` and
        // `rev iewer@ws`, and those must fail with the TARGET error rather than reaching the relay
        // — which is the only reason `reviewer@` is not simply a session name that does not exist.
        let cross_machine_target = to.is_some_and(|to| to.contains('@'));
        if let Some(restriction) = explicit_cross_machine_send_restriction(
            to,
            cwd,
            open_pane,
            kind.attachments(),
            reply_to,
            kind.supersedes(),
            kind.retry_of(),
        ) {
            return Err(DeliveryFailure::returned(restriction));
        }
        if cross_machine_target
            && let Some(to) = to
            && let Err(error) = crate::cross_machine::parse_cross_machine_target(to)
        {
            return Err(DeliveryFailure::returned(error.to_string()));
        }
        // `v0.12.0 index.ts:2322-2326` — verbatim, and BEFORE the confirm, so a flag typo never
        // costs a dialog.
        if open_pane && cwd.is_none() {
            return Err(DeliveryFailure::returned(
                "openProjectPaneIfMissing requires a target cwd.",
            ));
        }

        // `const confirmSend = !replyTo && config.confirmSend && ctx.hasUI` (`:1675`), hoisted above
        // the resolution because a pane LAUNCH is a side effect the human approves BEFORE it
        // happens, not after. `attachment_text` comes with it so every dialog shares one copy.
        let confirm_send =
            reply_to.is_none() && self.state.config.confirm_send && self.state.has_ui();
        let attachment_text = kind
            .attachments()
            .filter(|attachments| !attachments.is_empty())
            .map(format_attachments)
            .unwrap_or_default();
        let launch_possible = cwd.is_some() && open_pane;

        // `v0.12.0 index.ts:2330-2341`: the label is `to ?? cwd` — there is no resolved peer name
        // yet, and if the launch fails there never will be one. Asking here is what makes the
        // dialog a veto on the SIDE EFFECT rather than an acknowledgement after the fact.
        if confirm_send && launch_possible {
            let label = to.or(cwd).unwrap_or_default();
            if !self.confirm_send(&format!(
                "Send to \"{label}\":\n\n{message}{attachment_text}"
            )) {
                return Ok(text_result("Message cancelled by user"));
            }
        }
        // `:1695-1703` — the cross-machine dialog names the CALLER's target (nothing is resolved
        // yet) and carries no attachment text, since attachments are already refused.
        if confirm_send
            && cross_machine_target
            && let Some(to) = to
            && !self.confirm_send(&format!("Send to \"{to}\":\n\n{message}"))
        {
            return Ok(text_result("Message cancelled by user"));
        }

        // ICOM-074 — `v0.16.0 index.ts:1704-1733`. Sits after `confirmSend` is computed (the
        // remote confirm uses it) and before any local resolution, because `name@machine` names no
        // local session and `resolve_target` would hand the raw string to the broker as a target
        // that does not exist.
        if cross_machine_target && let Some(to) = to {
            // `:1705-1707`, BEFORE `buildPresenceIdentity`: a cancel that arrived while the dialog
            // was open stops the handover before anything about this session is put on a wire.
            if handover && cancel.is_cancelled() {
                return Err(DeliveryFailure::Returned(
                    HandoverRefusal::CancelledBeforeDelivery.into(),
                ));
            }
            // `{ name: identity.name, sessionId: connectedClient.sessionId ?? ctx.sessionManager
            // .getSessionId(), machine: config.crossMachine.machineName }` (`:1708-1713`): WHO this
            // session claims to be, in the remote host's terms. The name is
            // `buildPresenceIdentity`'s, so the origin matches the address local peers already hold.
            let origin = crate::cross_machine::CrossMachineOrigin {
                name: crate::connect::presence_identity_name(&self.state, None).unwrap_or_default(),
                session_id: client
                    .session_id()
                    .or_else(|| crate::connect::resolved_intercom_session_id(&self.state))
                    .unwrap_or_default(),
                machine: self.state.config.cross_machine.machine_name.clone(),
            };
            let runner = self.state.cross_machine_runner();
            return deliver_cross_machine(&self.state, runner.as_ref(), to, message, origin)
                .await
                .map_err(DeliveryFailure::Returned);
        }

        // `v0.10.1 index.ts:2001-2003`. With a `cwd` the target is resolved inside that
        // directory (`resolveCwdDeliveryTarget`); without one it is
        // `{ id: await resolveSessionTarget(connectedClient, to) ?? to }` — a NON-blocking
        // send that resolves to nothing is NOT refused here. It is handed to the broker as
        // the raw `to`, whose own `findSessions` gets the last word, and an unroutable
        // target comes back as the `Message to "…" was not delivered: …` result below. Only
        // the blocking `ask` refuses up front (`:2103-2110`), because an ask has a waiter to
        // hang.
        let delivery = match cwd {
            Some(cwd) => resolve_cwd_delivery_target(
                &self.state,
                client,
                CwdDeliveryOptions {
                    to,
                    cwd,
                    open_project_pane_if_missing: open_pane,
                    focus,
                    cancel,
                },
            )
            .await
            .map_err(DeliveryFailure::thrown)?,
            None => {
                let to_value = to.unwrap_or_default().to_string();
                DeliveryTarget {
                    id: self
                        .state
                        .resolve_target(client, &to_value)
                        .await
                        .map_err(|e| DeliveryFailure::thrown(to_tool_err(e)))?
                        .unwrap_or_else(|| to_value.clone()),
                    label: to_value,
                    project_pane: None,
                }
            }
        };
        let DeliveryTarget {
            id: target,
            label,
            project_pane,
        } = delivery;
        // `const targetDisplay = target.projectPane ? target.label : to ?? target.label;`
        // (`v0.12.0 index.ts:2346`). Pane-less, that is `to ?? target.label`: an explicit `to` is
        // echoed back verbatim, and a cwd-addressed send reports the peer's resolved name. With a
        // pane, the LAUNCHED session's own name wins over the caller's `to`, because `to` may have
        // been a bare filter that never named this session.
        let target_display = if project_pane.is_some() {
            label
        } else {
            to.map_or(label, str::to_string)
        };
        // `v0.10.1 index.ts:2005-2010` — the SAME string as the `ask` and `reply` self-guards
        // (`:2122`, `:2205`). pi has exactly one self-target message across all three arms.
        if client.session_id().as_deref() == Some(target.as_str()) {
            return Err(DeliveryFailure::returned(
                "Cannot message the current session",
            ));
        }
        // `v0.13.0 index.ts:2320-2328` (v0.12.1 `5fe0ee3` #119 "fix: guard active intercom
        // replies", issue #117):
        //
        //   const activeReplyMismatch = replyTo ? null : replyTracker.findActiveReplyTargetMismatch(sendTo);
        //
        // Sits AFTER the self-target guard and BEFORE the inferred-reply lookup, exactly as
        // upstream orders it. When this turn was triggered by peer A's ask, a send whose RESOLVED
        // target (`sendTo`, keyed the same way as the inferred lookup below) is anyone but A is
        // refused instead of delivered: with cwd-addressing (ICOM-042) live, `cwd` alone or a
        // roster guess can resolve to a parent/root session that never asked anything, and
        // upstream's fix note names that exact misdirection. An explicit `replyTo` bypasses the
        // guard — the caller has said which ask it answers, and `resolve_reply_target` polices it.
        // A handover has no `replyTo`, so the guard applies to it in full.
        //
        // `from.name || from.id` — JS `||`, so an EMPTY name falls back to the id too.
        // `details.replyTo` has no home on `ToolError` (message only); the id is in the text.
        if reply_to.is_none()
            && let Some(active) = self
                .state
                .tracker
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .find_active_reply_target_mismatch(&target, now_ms())
        {
            let sender_label = active
                .from
                .name
                .as_deref()
                .filter(|n| !n.is_empty())
                .unwrap_or(active.from.id.as_str());
            return Err(DeliveryFailure::returned(format!(
                "This turn is responding to an intercom ask from \"{sender_label}\". Use intercom({{ action: \"reply\", message: \"...\" }}) or set replyTo: \"{}\". Refusing non-reply send to \"{target_display}\" to avoid a misdirected reply.",
                active.message.id
            )));
        }
        // `v0.10.1 index.ts:2011-2012` (v0.9.3 `5d76146`, CHANGELOG 0.9.3: "Treat a public send to
        // the sole pending asker as its reply"):
        //
        //   const inferredAsk = replyTo || request.handover ? null : replyTracker.findUniquePendingAskFrom(sendTo);
        //   const effectiveReplyTo = replyTo ?? inferredAsk?.message.id;
        //
        // Without the inference, answering a peer's ask with the natural `send` phrasing left the
        // ask pending forever: it stayed in `pending`, the flush re-injected it once the run ended,
        // and the asking peer's blocking waiter hung to the full ask timeout. A HANDOVER is never
        // that answer (`request.handover`, `:1754`), so it skips the lookup.
        //
        // Note the lookup is keyed on `sendTo` — the RESOLVED id — not on the caller's `to`.
        let inferred_ask = if reply_to.is_some() || handover {
            None
        } else {
            self.state
                .tracker
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .find_unique_pending_ask_from(&target, now_ms())
        };
        let effective_reply_to = reply_to
            .map(str::to_string)
            .or_else(|| inferred_ask.as_ref().map(|c| c.message.id.clone()));
        // confirmSend gate (`index.ts:1524-1536`): only for a non-reply send, only when the
        // config opts in, and only when this session actually has a UI to confirm through — and
        // only when the launch branch above did NOT already ask. Nobody is confirmed twice.
        if confirm_send
            && !launch_possible
            // `Send to "${targetDisplay}"` (`v0.10.1 index.ts:2016`) — the human is asked about the
            // peer the message will actually reach, which for a cwd-addressed send is a name they
            // never typed.
            && !self.confirm_send(&format!(
                "Send to \"{target_display}\":\n\n{message}{attachment_text}"
            ))
        {
            return Ok(text_result("Message cancelled by user"));
        }
        // `:1768-1770` — immediately before `client.send`, so a cancel that landed during the
        // dialog or the pane launch is honoured and the summary never leaves.
        if handover && cancel.is_cancelled() {
            return Err(DeliveryFailure::Returned(
                HandoverRefusal::CancelledBeforeDelivery.into(),
            ));
        }
        let result = client
            .send(
                &target,
                SendOptions {
                    text: message.to_string(),
                    attachments: kind.attachments().map(<[Attachment]>::to_vec),
                    reply_to: effective_reply_to.clone(),
                    expects_reply: None,
                    message_id: None,
                    // `supersedes` / `retryOf` are threaded through `send` and `ask` only
                    // (`v0.10.1 index.ts:2029-2030`, `:2144-2145`); the `reply` arm (now `reply.rs`)
                    // deliberately does NOT carry them (`:2217-2221`).
                    supersedes: kind.supersedes().map(str::to_string),
                    retry_of: kind.retry_of().map(str::to_string),
                    provenance: None,
                },
            )
            .await
            .map_err(|e| DeliveryFailure::thrown(to_tool_err(e)))?;
        if !result.delivered {
            // `v0.10.1 index.ts:2032-2037`: the failure names the target and keeps pi's
            // fallback reason. A bare reason string tells the model nothing about which of
            // several in-flight sends failed.
            let reason = result
                .reason
                .unwrap_or_else(|| "Session may not exist or has disconnected.".to_string());
            return Err(DeliveryFailure::returned(format!(
                "Message to \"{target_display}\" was not delivered: {reason}"
            )));
        }
        // `index.ts:1549-1557`: the audit entry + markReplied both run ONLY after a confirmed
        // delivery — a failed/undelivered send must leave the original inbound ask pending.
        // `message: { text, attachments, replyTo, supersedes, retryOf }` (`:1783-1788`) is
        // `JSON.stringify`d, which omits every `undefined` member; so does this.
        if let Some(services) = self.state.host_services() {
            let mut entry_message = serde_json::Map::new();
            entry_message.insert("text".to_string(), serde_json::json!(message));
            if let Some(attachments) = kind.attachments() {
                entry_message.insert("attachments".to_string(), serde_json::json!(attachments));
            }
            if let Some(reply_to) = &effective_reply_to {
                entry_message.insert("replyTo".to_string(), serde_json::json!(reply_to));
            }
            if let Some(supersedes) = kind.supersedes() {
                entry_message.insert("supersedes".to_string(), serde_json::json!(supersedes));
            }
            if let Some(retry_of) = kind.retry_of() {
                entry_message.insert("retryOf".to_string(), serde_json::json!(retry_of));
            }
            let appended = services.append_entry(
                "intercom_sent",
                &serde_json::json!({
                    "to": target_display,
                    "message": entry_message,
                    "messageId": result.id,
                    "timestamp": now_ms(),
                }),
            );
            if let Err(e) = appended {
                tracing::warn!(error = %e, kind = "intercom_sent", "intercom: failed to append audit entry");
            }
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
        // ICOM-054 — `details: { ...deliveryDetails(result), … }` (`v0.13.0 index.ts:2373`), which
        // replaced the bare `{ messageId, delivered: true }` pair.
        let mut details = crate::tools::delivery_details(&result);
        if let Some(reply_to) = &effective_reply_to
            && let Some(map) = details.as_object_mut()
        {
            map.insert("replyTo".to_string(), serde_json::json!(reply_to));
        }
        // `v0.12.0 index.ts:2390-2401` — the pane facts ride on the SAME `details` object.
        if let Some(pane) = &project_pane
            && let Some(map) = details.as_object_mut()
        {
            map.insert("openedProjectPane".to_string(), serde_json::json!(true));
            map.insert("paneId".to_string(), serde_json::json!(pane.pane_id));
            map.insert(
                "projectRoot".to_string(),
                serde_json::json!(pane.project_root),
            );
        }
        Ok(detailed_result(
            // The pane branch OUTRANKS the inferred-reply branch upstream (`:2392-2396`): a
            // freshly launched session cannot have a pending ask to infer against anyway.
            if let Some(pane) = &project_pane {
                // `index.ts:2394` hard-codes `Herdr`; here the name rides on the launch, so this
                // names the backend that opened THIS pane rather than whatever the slot holds by
                // the time the string is built.
                format!(
                    "Opened {} project pane {} for {} and sent message to {target_display}",
                    pane.launcher_name, pane.pane_id, pane.project_root
                )
            } else if inferred_ask.is_some() {
                format!("Reply sent to {target_display} (inferred from pending ask)")
            } else {
                format!("Message sent to {target_display}")
            },
            details,
        ))
    }

    /// `ctx.ui.confirm("Send message", body)`: whether the human approved. With no host services
    /// bound there is nobody to ask, which is also what `ctx.hasUI` being false means upstream, and
    /// the send proceeds.
    fn confirm_send(&self, body: &str) -> bool {
        let Some(services) = self.state.host_services() else {
            return true;
        };
        services.confirm(CONFIRM_TITLE, body, &cyrup_ext::DialogOptions::default())
    }
}
