//! [`SharedIntercomState`] — the per-session state the tools, the inbound event loop, and the seam
//! channels all share: the live [`IntercomClient`], the inbound [`ReplyTracker`], the outbound
//! single-slot [`OutboundReplyWaiter`], the resolved [`IntercomConfig`], and this session's own id.
//!
//! The [`IntercomClient`] is created on `SessionStart` (after the broker is health-connectable) and
//! stashed here; tools/seams clone the `Arc` out under a short lock (never held across `.await`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_ext::HostServices;

use crate::config::IntercomConfig;
use crate::error::{IntercomError, Result};
use crate::reply_tracker::{OutboundReplyWaiter, ReplyTracker};
use crate::transport::client::{IntercomClient, SendOptions};

/// The shared, session-scoped intercom state.
pub struct SharedIntercomState {
    client: Mutex<Option<Arc<IntercomClient>>>,
    /// The live `HostServices` backend, late-bound via P-1 Route B (`set_host_services` before
    /// `init`, the port doc §4.1). The SAME `Arc` the session mutates via `set_ui_sink`/manager
    /// attach, so the inbound surface + ClarifyChannel human answer observe those through it even
    /// though they run OUTSIDE any `HostCtx`. `None` until the builder late-binds it (or in a
    /// headless/degraded session) → the human surface degrades to a no-op, never blocks.
    host_services: Mutex<Option<Arc<dyn HostServices>>>,
    /// Whether this session has an interactive UI (pi `hasUI`, `index.ts:739-758`). Captured ONCE
    /// from the live `HostCtx::has_ui` at `SessionStart` (a static per-session property) and read by
    /// the inbound delivery policy ([`crate::inbound`]): an interactive session drives/steers a turn
    /// over an inbound message; a non-interactive (`!has_ui`) session instead sends the sender the
    /// "running in non-interactive mode" busy auto-reply. Defaults `false` (no UI) until the
    /// `SessionStart` handler sets it — a headless/degraded session then takes the auto-reply branch.
    has_ui: AtomicBool,
    /// Inbound ask tracking (`ReplyTracker`).
    pub tracker: Mutex<ReplyTracker>,
    /// The outbound single-slot reply waiter (`replyWaiter`).
    pub waiter: OutboundReplyWaiter,
    /// The resolved intercom config.
    pub config: IntercomConfig,
    /// The ask timeout (default 10 min).
    pub ask_timeout_ms: u64,
    /// This session's working directory (captured at construction, like `SubagentsExtension`; used
    /// for the `intercom{list}` "same cwd" tag).
    pub cwd: std::path::PathBuf,
}

impl SharedIntercomState {
    /// Build the shared state with no client connected yet.
    #[must_use]
    pub fn new(config: IntercomConfig, ask_timeout_ms: u64, cwd: std::path::PathBuf) -> Self {
        Self {
            client: Mutex::new(None),
            host_services: Mutex::new(None),
            has_ui: AtomicBool::new(false),
            tracker: Mutex::new(ReplyTracker::new(ask_timeout_ms)),
            waiter: OutboundReplyWaiter::new(),
            config,
            ask_timeout_ms,
            cwd,
        }
    }

    /// Late-bind the live `HostServices` backend (P-1 Route B; called from
    /// [`crate::extension::IntercomExtension`]'s `set_host_services`, which the builder invokes via
    /// `load_native_with_services` BEFORE `init`). Idempotent: a session rebuild rebinds the same
    /// shared `Arc`.
    pub fn set_host_services(&self, services: Arc<dyn HostServices>) {
        *self.host_services.lock().unwrap_or_else(|e| e.into_inner()) = Some(services);
    }

    /// The live `HostServices` backend, if bound (the inbound surface + ClarifyChannel human answer
    /// load it per call and degrade to a no-op when absent).
    #[must_use]
    pub fn host_services(&self) -> Option<Arc<dyn HostServices>> {
        self.host_services.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Record whether this session has an interactive UI (pi `hasUI`). Called ONCE from the
    /// `SessionStart` handler with the live `HostCtx::has_ui`; the inbound delivery policy reads it
    /// via [`Self::has_ui`].
    pub fn set_has_ui(&self, has_ui: bool) {
        self.has_ui.store(has_ui, Ordering::SeqCst);
    }

    /// Whether this session has an interactive UI (pi `hasUI`, `index.ts:739-758`) — the inbound
    /// idle-vs-busy delivery policy's static gate. `false` (the default) until the `SessionStart`
    /// handler binds it, so a headless/degraded session takes the non-interactive auto-reply branch.
    #[must_use]
    pub fn has_ui(&self) -> bool {
        self.has_ui.load(Ordering::SeqCst)
    }

    /// Resolve a name/id/unique-prefix `to` to a single session id against the live session list
    /// (`resolveSessionTarget`, `index.ts:856-879`). `Ok(None)` = no match; an ambiguous match is an
    /// `Err`.
    ///
    /// # Errors
    /// [`IntercomError::Client`] on a `list` failure or an ambiguous match.
    pub async fn resolve_target(&self, client: &Arc<IntercomClient>, name_or_id: &str) -> Result<Option<String>> {
        let sessions = client.list_sessions().await?;
        let entries: Vec<(String, Option<String>)> =
            sessions.into_iter().map(|s| (s.id, s.name)).collect();
        let ids = crate::broker::routing::find_session_ids(&entries, name_or_id);
        match ids.len() {
            0 => Ok(None),
            1 => Ok(ids.into_iter().next()),
            _ => Err(IntercomError::Client(format!(
                "Multiple sessions match \"{name_or_id}\". Use the session ID instead."
            ))),
        }
    }

    /// Stash the live client (on connect) or clear it (on disconnect).
    pub fn set_client(&self, client: Option<Arc<IntercomClient>>) {
        *self.client.lock().unwrap_or_else(|e| e.into_inner()) = client;
    }

    /// The live client, if connected.
    #[must_use]
    pub fn client(&self) -> Option<Arc<IntercomClient>> {
        self.client.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// This session's own broker-assigned id, if connected (for the self-target guard).
    #[must_use]
    pub fn self_session_id(&self) -> Option<String> {
        self.client().and_then(|c| c.session_id())
    }

    /// Issue a blocking ask over the broker and await the reply (`waitForReply` +
    /// `client.send(expectsReply)`, `index.ts:1295-1373,1613-1616`): register the outbound single
    /// slot, send with `expects_reply`, then race the reply against the ask timeout and the tool's
    /// cancellation. On every non-reply exit the slot is cleared and the broker ask edge cancelled.
    ///
    /// # Errors
    /// [`IntercomError::Client`] on "Already waiting for a reply", a non-delivered send, a timeout,
    /// or cancellation.
    pub async fn ask_and_wait(
        &self,
        client: &Arc<IntercomClient>,
        target: &str,
        question_id: String,
        text: String,
        attachments: Option<Vec<crate::transport::protocol::Attachment>>,
        cancel: &cyrup_core::CancelToken,
    ) -> Result<String> {
        // Single-slot guard (`if replyWaiter → "Already waiting for a reply"`).
        let rx = self
            .waiter
            .register(target.to_string(), question_id.clone())
            .map_err(IntercomError::Client)?;

        let send_result = client
            .send(target, SendOptions {
                text,
                attachments,
                reply_to: None,
                expects_reply: Some(true),
                message_id: Some(question_id.clone()),
            })
            .await;

        match send_result {
            Ok(result) if result.delivered => {}
            Ok(result) => {
                self.waiter.clear_matching(&question_id);
                return Err(IntercomError::Client(
                    result.reason.unwrap_or_else(|| "ask was not delivered".to_string()),
                ));
            }
            Err(e) => {
                self.waiter.clear_matching(&question_id);
                return Err(e);
            }
        }

        let timeout = Duration::from_millis(self.ask_timeout_ms);
        tokio::select! {
            reply = rx => {
                match reply {
                    // Inline the reply's attachments into the visible body, exactly as pi does
                    // (`replyText + formatAttachments(replyMessage.content.attachments)`,
                    // `index.ts:1646-1649,1354-1357`) — never silently drop them.
                    Ok(message) => Ok(inline_reply_attachments(message.content.text, message.content.attachments.as_deref())),
                    Err(_) => {
                        // The slot's sender was dropped (cleared elsewhere).
                        self.waiter.clear_matching(&question_id);
                        client.cancel_ask(&question_id);
                        Err(IntercomError::Client("reply waiter cancelled".to_string()))
                    }
                }
            }
            () = tokio::time::sleep(timeout) => {
                self.waiter.clear_matching(&question_id);
                client.cancel_ask(&question_id);
                Err(IntercomError::Client(format!(
                    "No reply from \"{target}\" within {}",
                    describe_timeout(self.ask_timeout_ms)
                )))
            }
            () = cancel.cancelled() => {
                self.waiter.clear_matching(&question_id);
                client.cancel_ask(&question_id);
                Err(IntercomError::Client("Cancelled".to_string()))
            }
        }
    }
}

/// Inline a reply's attachments into its visible text (pi `replyText + formatAttachments(...)`,
/// `index.ts:1646-1649` (ask) and `index.ts:1354-1357` (contact_supervisor)) — attachments the
/// replying session sent back must never be silently dropped.
fn inline_reply_attachments(text: String, attachments: Option<&[crate::transport::protocol::Attachment]>) -> String {
    let attachment_text = attachments
        .filter(|a| !a.is_empty())
        .map(crate::inbound::format_attachments)
        .unwrap_or_default();
    format!("{text}{attachment_text}")
}

/// `askTimeoutMs % 60000 === 0 ? "N minutes" : "Nms"` (`index.ts:471`).
fn describe_timeout(ask_timeout_ms: u64) -> String {
    if ask_timeout_ms.is_multiple_of(60_000) {
        format!("{} minutes", ask_timeout_ms / 60_000)
    } else {
        format!("{ask_timeout_ms}ms")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn describe_timeout_formats_minutes_and_ms() {
        assert_eq!(describe_timeout(600_000), "10 minutes");
        assert_eq!(describe_timeout(60_000), "1 minutes");
        assert_eq!(describe_timeout(5000), "5000ms");
    }

    #[test]
    fn set_and_clear_client() {
        let state = SharedIntercomState::new(IntercomConfig::default(), 600_000, std::path::PathBuf::from("/w"));
        assert!(state.client().is_none());
        assert!(state.self_session_id().is_none());
    }

    /// Regression proof for the "ask/contact_supervisor replies drop attachments" divergence
    /// (pi `index.ts:1646-1649,1354-1357`): before the fix, `ask_and_wait` returned
    /// `message.content.text` verbatim, discarding `content.attachments` entirely. The reply text
    /// must now carry the same `📎 name\n content` block pi's `formatAttachments` inlines.
    #[test]
    fn inline_reply_attachments_appends_pi_formatted_block() {
        use crate::transport::protocol::{Attachment, AttachmentKind};

        let text = inline_reply_attachments(
            "Looks good".to_string(),
            Some(&[Attachment {
                kind: AttachmentKind::Snippet,
                name: "patch.diff".to_string(),
                content: "+1 line".to_string(),
                language: Some("diff".to_string()),
            }]),
        );
        assert_eq!(text, "Looks good\n\n---\n📎 patch.diff\n~~~diff\n+1 line\n~~~");
    }

    /// No attachments ⇒ the reply text passes through unchanged (pi: `replyAttachments = ""` when
    /// `replyMessage.content.attachments?.length` is falsy).
    #[test]
    fn inline_reply_attachments_passes_through_when_none() {
        assert_eq!(inline_reply_attachments("no attachments here".to_string(), None), "no attachments here");
        assert_eq!(inline_reply_attachments("empty vec".to_string(), Some(&[])), "empty vec");
    }
}
