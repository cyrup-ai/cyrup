//! `intercom{action:"handover"}` (`v0.16.0 index.ts:2627-2654`, with `buildHandoverText`,
//! `:1814-1835`) — summarize THIS session with its current model and send the summary through the
//! same delivery `send` uses.
//!
//! The arm's order is upstream's, and it is observable: the parameters are refused before any
//! model is called; the summary is generated BEFORE the delivery's confirm dialog opens, so the
//! dialog and the audit entry carry the full text the receiver will get; and the caller's cancel
//! is checked once more after generation and again by the delivery just before the message leaves.

use std::sync::Arc;

use cyrup_core::{CancelToken, ToolError, ToolResult};

use crate::handover::{
    GitState, HandoverError, HandoverHeader, HandoverRefusal, format_handover_message,
    generate_handover_body, read_git_state,
};
use crate::identity::short_session_id;
use crate::transport::client::IntercomClient;

use super::deliver::{DeliveryKind, DeliveryRequest};
use super::{IntercomParams, IntercomTool};

/// JS truthiness of an optional string parameter: absent and `""` are both "not given".
fn given(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

impl IntercomTool {
    pub(super) async fn action_handover(
        &self,
        params: &IntercomParams,
        client: &Arc<IntercomClient>,
        cancel: &CancelToken,
    ) -> Result<ToolResult, ToolError> {
        let to = params.to.as_deref().filter(|v| !v.trim().is_empty());
        let cwd = params.cwd.as_deref().filter(|v| !v.trim().is_empty());
        // `if (!to && !cwd)` (`:2628`). Unlike `send`, `message` is not required: it is the
        // optional next task.
        if to.is_none() && cwd.is_none() {
            return Err(HandoverRefusal::MissingTarget.into());
        }
        // `if (replyTo || supersedes || retryOf || attachments?.length)` (`:2634`).
        if given(params.reply_to.as_deref())
            || given(params.supersedes.as_deref())
            || given(params.retry_of.as_deref())
            || params.attachments.as_ref().is_some_and(|a| !a.is_empty())
        {
            return Err(HandoverRefusal::UnsupportedFields.into());
        }
        // `crossMachine: Boolean(to?.includes("@"))` (`:2643`).
        let cross_machine = to.is_some_and(|to| to.contains('@'));
        let handover_text = self
            .build_handover_text(client, params.message.as_deref(), cross_machine, cancel)
            .await
            .map_err(HandoverRefusal::from)?;
        self.deliver_message(
            client,
            cancel,
            DeliveryRequest {
                to,
                cwd,
                open_project_pane_if_missing: params.open_project_pane_if_missing.unwrap_or(false),
                focus: params.focus.unwrap_or(true),
                message: &handover_text,
                kind: DeliveryKind::Handover,
            },
        )
        .await
    }

    /// `buildHandoverText(connectedClient, ctx, { goal, crossMachine }, signal)`
    /// (`v0.16.0 index.ts:1814-1835`).
    ///
    /// Generation and the git read run together (`Promise.all`), and the first failure wins without
    /// waiting for the other — `try_join!` short-circuits exactly as `Promise.all` rejects. The
    /// cancel check AFTER both is its own refusal even when generation itself succeeded: a result
    /// that arrived under a fired signal is an abort, not a handover.
    ///
    /// `crossMachine` drops the session-file line, because the path names a file the receiver
    /// cannot open.
    async fn build_handover_text(
        &self,
        client: &Arc<IntercomClient>,
        goal: Option<&str>,
        cross_machine: bool,
        cancel: &CancelToken,
    ) -> Result<String, HandoverError> {
        let services = self.state.host_services();
        let (body, git) = tokio::try_join!(
            generate_handover_body(services.as_deref(), goal, cancel),
            async { Ok::<Option<GitState>, HandoverError>(read_git_state(&self.state.cwd).await) },
        )?;
        if cancel.is_cancelled() {
            return Err(HandoverError::Aborted);
        }
        // `connectedClient.sessionId ?? ctx.sessionManager.getSessionId()` (`:1827`).
        let session_id = client
            .session_id()
            .or_else(|| services.as_ref().and_then(|services| services.session_id()))
            .unwrap_or_default();
        // `pi.getSessionName()?.trim() || sessionId.slice(0, 8)` (`:1829`) — JS `||`, so a blank
        // name falls through to the id prefix.
        let session_name = services
            .as_ref()
            .and_then(|services| services.session_name())
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty());
        let sender_name = session_name.unwrap_or_else(|| short_session_id(&session_id));
        let sender_cwd = self.state.cwd.to_string_lossy();
        let session_file = if cross_machine {
            None
        } else {
            services
                .as_ref()
                .and_then(|services| services.session_file())
                .map(|path| path.to_string_lossy().into_owned())
        };
        Ok(format_handover_message(
            &HandoverHeader {
                sender_name: &sender_name,
                sender_cwd: &sender_cwd,
                session_file: session_file.as_deref(),
                git: git.as_ref(),
            },
            &body,
        ))
    }
}
