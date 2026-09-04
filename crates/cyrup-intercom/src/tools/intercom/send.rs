//! `intercom{action:"send"}` (`v0.10.1 index.ts:1971-2061`) — the non-blocking mailbox delivery,
//! including the confirm gate, the inferred-reply inference and the audit entry.

use std::sync::Arc;

use cyrup_core::{CancelToken, ToolError, ToolResult};

use crate::inbound::format_attachments;
use crate::tools::{detailed_result, text_result};
use crate::transport::client::{IntercomClient, SendOptions};
use crate::transport::protocol::now_ms;

use super::{
    CwdDeliveryOptions, DeliveryTarget, IntercomParams, IntercomTool, resolve_cwd_delivery_target,
    to_tool_err,
};

impl IntercomTool {
    pub(super) async fn action_send(
        &self,
        params: &IntercomParams,
        client: &Arc<IntercomClient>,
        cancel: &CancelToken,
    ) -> Result<ToolResult, ToolError> {
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
        let open_pane = params.open_project_pane_if_missing.unwrap_or(false);
        // `v0.12.0 index.ts:2322-2326` — verbatim, and BEFORE the confirm, so a flag typo never
        // costs a dialog.
        if open_pane && cwd.is_none() {
            return Err(ToolError::new(
                "openProjectPaneIfMissing requires a target cwd.",
            ));
        }

        // `const confirmSend = !replyTo && config.confirmSend && ctx.hasUI` (`:2328`), hoisted
        // above the resolution because a pane LAUNCH is a side effect the human approves BEFORE it
        // happens, not after. `attachment_text` comes with it so both branches share one copy.
        let confirm_send =
            params.reply_to.is_none() && self.state.config.confirm_send && self.state.has_ui();
        let attachment_text = params
            .attachments
            .as_deref()
            .filter(|a| !a.is_empty())
            .map(format_attachments)
            .unwrap_or_default();
        let launch_possible = cwd.is_some() && open_pane;

        // `v0.12.0 index.ts:2330-2341`: the label is `to ?? cwd` — there is no resolved peer name
        // yet, and if the launch fails there never will be one. Asking here is what makes the
        // dialog a veto on the SIDE EFFECT rather than an acknowledgement after the fact.
        if confirm_send
            && launch_possible
            && let Some(services) = self.state.host_services()
        {
            let label = to.clone().or_else(|| cwd.clone()).unwrap_or_default();
            if !services.confirm(
                "Send Message",
                &format!("Send to \"{label}\":\n\n{message}{attachment_text}"),
                &cyrup_ext::DialogOptions::default(),
            ) {
                return Ok(text_result("Message cancelled by user"));
            }
        }

        // `v0.10.1 index.ts:2001-2003`. With a `cwd` the target is resolved inside that
        // directory (`resolveCwdDeliveryTarget`); without one it is
        // `{ id: await resolveSessionTarget(connectedClient, to) ?? to }` — a NON-blocking
        // send that resolves to nothing is NOT refused here. It is handed to the broker as
        // the raw `to`, whose own `findSessions` gets the last word, and an unroutable
        // target comes back as the `Message to "…" was not delivered: …` result below. Only
        // the blocking `ask` refuses up front (`:2103-2110`), because an ask has a waiter to
        // hang.
        let delivery = match cwd.as_deref() {
            Some(cwd) => {
                resolve_cwd_delivery_target(
                    &self.state,
                    client,
                    CwdDeliveryOptions {
                        to: to.as_deref(),
                        cwd,
                        open_project_pane_if_missing: open_pane,
                        focus: params.focus.unwrap_or(true),
                        cancel,
                    },
                )
                .await?
            }
            None => {
                let to_value = to.clone().unwrap_or_default();
                DeliveryTarget {
                    id: self
                        .state
                        .resolve_target(client, &to_value)
                        .await
                        .map_err(to_tool_err)?
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
            to.clone().unwrap_or(label)
        };
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
        // config opts in, and only when this session actually has a UI to confirm through — and
        // only when the launch branch above did NOT already ask. Nobody is confirmed twice.
        if confirm_send
            && !launch_possible
            && let Some(services) = self.state.host_services()
        {
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
            .send(
                &target,
                SendOptions {
                    text: message.clone(),
                    attachments: params.attachments.clone(),
                    reply_to: effective_reply_to.clone(),
                    expects_reply: None,
                    message_id: None,
                    // `supersedes` / `retryOf` are threaded through `send` and `ask` only
                    // (`v0.10.1 index.ts:2029-2030`, `:2144-2145`); the `reply` arm (now `reply.rs`)
                    // deliberately does NOT carry them (`:2217-2221`).
                    supersedes: params.supersedes.clone(),
                    retry_of: params.retry_of.clone(),
                    provenance: None,
                },
            )
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
            // The call stays a statement rather than a `&&` let-chain in the outer guard's
            // condition (clippy::collapsible_if): the audit append has a side effect.
            let appended = services.append_entry(
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
        let mut details = serde_json::json!({ "messageId": result.id, "delivered": true });
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
}
