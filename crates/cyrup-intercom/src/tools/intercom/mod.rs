//! The `intercom` tool (`v0.10.1 index.ts:1826+`): `list`/`list-cwd`/`send`/`ask`/`reply`/`pending`/
//! `status` over the shared broker client. `ask` is the one blocking action (single-slot outbound waiter).
//!
//! ## Layout
//!
//! This file is a facade, on the [`crate::broker`] template. `index.ts` holds the tool as one
//! `switch (action)` and so did this file, until its `dispatch` reached 592 lines — eight
//! independent handlers sharing one scope, because match arms cannot see each other's bindings.
//! The modules below are the seams it was carrying: one per action, in upstream's own `switch`
//! order — `list`, `list_cwd`, `cancel`, `send`, `ask`, `reply`, `pending`, `status` — each a
//! `pub(super) async fn action_*` in its own `impl IntercomTool` block, carrying that arm's
//! `index.ts` citations with the code they annotate. `IntercomTool::dispatch` keeps the shared
//! prelude (`ensureConnected("tool")` + `syncPresenceIdentity`) and is now the action switch and
//! nothing else.
//!
//! This file keeps what the handlers share: the tool itself, `IntercomParams`,
//! `DeliveryTarget`, the target/cwd resolvers, the row and error formatters, the schema and the
//! [`Tool`] impl. Those items are `pub(super)`, which from a child of `intercom` means "visible
//! throughout `intercom`", so a handler reaches them exactly as it did when they shared one file —
//! while the crate's public surface stays [`IntercomTool`] alone.

mod ask;
mod cancel;
mod list;
mod list_cwd;
mod pending;
mod reply;
mod send;
mod status;

use std::sync::Arc;

use async_trait::async_trait;
use cyrup_core::{CancelToken, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};

use crate::format_context::format_context_usage;
use crate::session_state::SharedIntercomState;
use crate::transport::protocol::{Attachment, SessionInfo};

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
    /// `openProjectPaneIfMissing` (`v0.12.0 index.ts:2175-2177`) — for `send`/`ask` with `cwd`,
    /// open a visible Herdr project pane and launch cyrup there when no matching live session is
    /// connected. Rejected without a `cwd` (`:2322-2326`, `:2437-2441`).
    #[serde(default)]
    open_project_pane_if_missing: Option<bool>,
    /// `focus` (`v0.12.0 index.ts:2178-2180`) — focus the new pane. **Defaults to true**
    /// (`project-agent.ts:239` is `input.focus !== false`, so only an explicit `false` unfocuses).
    #[serde(default)]
    focus: Option<bool>,
}

/// `DeliveryTarget` (`v0.10.1 index.ts:62-66`) — the id a message is actually sent to plus the
/// label the result echoes back.
pub(super) struct DeliveryTarget {
    pub(super) id: String,
    pub(super) label: String,
    /// `projectPane?: ProjectPaneLaunch` (`v0.12.0 index.ts:75`). `Some` only when THIS call
    /// launched the pane — every result and `details` branch keys off it.
    pub(super) project_pane: Option<crate::project_pane::ProjectPaneLaunch>,
}

/// `resolveCwdDeliveryTarget`'s `options` (`v0.12.0 index.ts:1500-1506`).
///
/// An options struct rather than a fifth positional argument, because upstream's is one too and
/// because `to`/`cwd` are both `&str` — adjacent same-typed positionals are exactly the shape a
/// caller transposes silently.
pub(super) struct CwdDeliveryOptions<'a> {
    pub(super) to: Option<&'a str>,
    pub(super) cwd: &'a str,
    pub(super) open_project_pane_if_missing: bool,
    /// Already defaulted: `params.focus.unwrap_or(true)`.
    pub(super) focus: bool,
    pub(super) cancel: &'a cyrup_core::CancelToken,
}

/// `options.cwd && options.cwd !== "." ? resolvePath(currentSession.cwd, options.cwd) : currentSession.cwd`
/// (`v0.12.0 index.ts:1517-1519`, and the identical expression at `:2247-2249` for `list-cwd`).
///
/// `current_cwd` is the cwd the BROKER reports for this session, not the locally captured one — a
/// relative `cwd` must resolve against the directory peers can actually see this session in.
pub(super) fn resolve_target_cwd(current_cwd: &str, cwd: &str) -> String {
    match cwd {
        "" | "." => current_cwd.to_string(),
        other => crate::cwd::resolve_path(std::path::Path::new(current_cwd), other)
            .to_string_lossy()
            .to_string(),
    }
}

/// `resolveCwdDeliveryTarget(activeClient, options)` (`v0.12.0 index.ts:1500-1543`).
///
/// The three steps upstream takes before the lookup are load-bearing and all ported: the roster is
/// fetched ONCE and reused (`:1507`, so `to` and `cwd` resolve against one consistent snapshot), the
/// caller's own row is required to be in it (`:1509-1515` — the target cwd defaults to *its* cwd,
/// not to the locally captured one), and a relative `cwd` resolves against that row's cwd with `"."`
/// meaning "here" (`resolve_target_cwd`).
///
/// The `Missing` arm then carries upstream's two outcomes. Without the flag it is the refusal at
/// `:1529-1531`, whose second sentence names `openProjectPaneIfMissing` as the next step — text this
/// port emits only because `parameters_schema` advertises the parameter and the launcher slot
/// honours it. With the flag it is the launch (`:1533-1542`):
/// [`crate::project_pane::resolve_project_root`] first, so a target that is not a directory costs no
/// process; then the pre-launch roster snapshot (`:1533`) that lets the wait identify the new
/// session by DIFFERENCE rather than by cwd alone; then the backend's
/// [`open`](crate::project_pane::ProjectPaneLauncher::open); then
/// [`crate::project_target::wait_for_project_session`] (`:1535-1541`), polling until the agent that
/// pane started has registered with the broker and is addressable.
///
/// # Errors
/// [`ToolError`] when the caller is not registered or is missing from the roster, when
/// `resolve_target_in_cwd` reports an ambiguity, when the target is not a directory, when the
/// backend cannot open a pane, or when the launched session never registers.
pub(super) async fn resolve_cwd_delivery_target(
    state: &crate::session_state::SharedIntercomState,
    client: &crate::transport::client::IntercomClient,
    options: CwdDeliveryOptions<'_>,
) -> Result<DeliveryTarget, ToolError> {
    let CwdDeliveryOptions {
        to,
        cwd,
        open_project_pane_if_missing,
        focus,
        cancel,
    } = options;
    let sessions = client.list_sessions().await.map_err(to_tool_err)?;
    // `if (!currentSessionId) throw new Error("Current session is not registered with intercom.")`
    let Some(current_session_id) = client.session_id() else {
        return Err(ToolError::new(
            "Current session is not registered with intercom.",
        ));
    };
    let Some(current_session) = sessions.iter().find(|s| s.id == current_session_id) else {
        return Err(ToolError::new(
            "Current session is missing from intercom session list.",
        ));
    };
    let target_cwd = resolve_target_cwd(&current_session.cwd, cwd);
    let existing = crate::project_target::resolve_target_in_cwd(
        &sessions,
        &current_session_id,
        &target_cwd,
        to,
    )
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
            Ok(DeliveryTarget {
                id: session.id.clone(),
                label,
                project_pane: None,
            })
        }
        crate::project_target::ProjectTargetResolution::Missing { reason, .. } => {
            let launcher = state.project_pane_launcher();
            // `v0.12.0 index.ts:1529-1530`. The sentence naming the flag is emitted ONLY because
            // the schema now advertises it — this is the line ICOM-042 exists to make honest.
            if !open_project_pane_if_missing {
                // The NOUN PHRASE is substituted whole, not the vendor name alone: with a backend
                // bound this is upstream's sentence verbatim (`a Herdr project pane`), and with none
                // it degrades to a bare `a project pane` instead of doubling the word.
                let pane_noun = launcher.as_ref().map_or_else(
                    || "project pane".to_string(),
                    |l| format!("{} project pane", l.name()),
                );
                return Err(ToolError::new(format!(
                    "{reason} Pass openProjectPaneIfMissing: true to open a {pane_noun} and start cyrup there."
                )));
            }
            // `resolveProjectRoot` FIRST (`project-agent.ts:233`): a non-directory is refused
            // before any backend is consulted, so a typo costs no process.
            let project_root = crate::project_pane::resolve_project_root(
                std::path::Path::new(&current_session.cwd),
                &target_cwd,
            )
            .map_err(ToolError::new)?;

            // `const beforeSessionIds = new Set(sessions.map(s => s.id))` (`index.ts:1533`) — the
            // snapshot is taken from the roster ALREADY fetched above, before the launch.
            let before: std::collections::HashSet<String> =
                sessions.iter().map(|s| s.id.clone()).collect();

            let launcher = launcher.unwrap_or_else(|| {
                std::sync::Arc::new(crate::project_pane::UnavailableLauncher {
                    reason: "No project pane launcher is configured for this session.".to_string(),
                })
            });
            let launch = launcher
                .open(crate::project_pane::ProjectPaneRequest {
                    project_root: project_root.clone(),
                    focus,
                    cancel,
                })
                .await
                .map_err(|e| ToolError::new(e.to_string()))?;

            let session = crate::project_target::wait_for_project_session(
                client,
                &launch.project_root,
                &current_session_id,
                &before,
                to,
                cancel,
                launcher.name(),
            )
            .await
            .map_err(ToolError::new)?;

            // `{ id: session.id, label: session.name || session.id, projectPane }` (`:1542`) —
            // JS `||`, so a blank name falls through to the id. NOTE the label deliberately does
            // NOT consider `to` here, unlike the `found` arm.
            let label = session
                .name
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| session.id.clone());
            Ok(DeliveryTarget {
                id: session.id,
                label,
                project_pane: Some(launch),
            })
        }
    }
}

impl IntercomTool {
    /// Build the tool over the shared session state.
    #[must_use]
    pub fn new(state: Arc<SharedIntercomState>) -> Self {
        Self {
            state,
            parameters: parameters_schema(),
        }
    }

    /// The action switch (`v0.10.1 index.ts:1854-2271`).
    ///
    /// Every arm is one `action_*` handler in the module named for it; the shared prelude below is
    /// all this function computes for them. `cancel` is threaded to `ask` alone — it is the only
    /// blocking action and so the only arm with a wait to abort (`v0.10.1 index.ts:2144-2153`).
    async fn dispatch(
        &self,
        params: IntercomParams,
        cancel: &CancelToken,
    ) -> Result<ToolResult, ToolError> {
        // pi routes every tool call through `ensureConnected("tool")` (`index.ts:1477`), not a bare
        // `client` read: a tool call is worth (re)spawning the broker and reconnecting for, so a
        // single earlier connection failure does not make this tool permanently useless.
        let client =
            crate::connect::ensure_connected(&self.state, crate::connect::ConnectReason::Tool)
                .await
                .map_err(|e| {
                    ToolError::new(format!("intercom is not connected to the broker: {e}"))
                })?;
        // `v0.10.1 index.ts:1853`: `syncPresenceIdentity(ctx.sessionManager.getSessionId())`
        // immediately after `ensureConnected("tool")` and before the action `match`. One of pi's
        // three name-sync points; without it a session renamed by `/name`, a branch switch or a
        // title change keeps advertising its startup label to every peer's `intercom{list}` picker.
        self.state.sync_presence_identity();

        match params.action.as_str() {
            "list" => self.action_list(&params, &client).await,
            // `list-cwd` sits between `list` and `cancel`, and `cancel` between `list-cwd` and
            // `send`, in upstream's own `switch` order.
            "list-cwd" => self.action_list_cwd(&params, &client).await,
            "cancel" => self.action_cancel(&params, &client).await,
            "send" => self.action_send(&params, &client, cancel).await,
            "ask" => self.action_ask(&params, &client, cancel).await,
            "reply" => self.action_reply(&params, &client).await,
            "pending" => self.action_pending(&params, &client).await,
            "status" => self.action_status(&params, &client).await,
            other => Err(ToolError::new(format!(
                "unknown intercom action \"{other}\""
            ))),
        }
    }
}

pub(super) fn require(value: Option<String>, msg: &str) -> Result<String, ToolError> {
    match value {
        Some(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(ToolError::new(msg.to_string())),
    }
}

pub(super) fn to_tool_err(e: crate::error::IntercomError) -> ToolError {
    ToolError::new(e.to_string())
}

/// `session.name || session.id` (`index.ts:1720,1726`). JS `||` is falsy-based, so an empty name
/// falls through to the id — hence the `filter(|n| !n.is_empty())`.
pub(super) fn display_name(session: &SessionInfo) -> &str {
    session
        .name
        .as_deref()
        .filter(|n| !n.is_empty())
        .unwrap_or(&session.id)
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
///
/// ICOM-058 added the `tmuxPane` term on the same terms, from `v0.12.0 index.ts:546-553`:
///
/// ```text
/// const pane = session.tmuxPane ? ` · tmux ${session.tmuxPane}` : "";
/// return `• ${name} (${idPrefix}) — ${session.cwd} (${session.model}${formatContextUsage(session)}${pane})${suffix}`;
/// ```
///
/// It follows the context usage inside the same parentheses, and — like it — the overlay
/// (`ui/session-list.ts:36-42`, and [`crate::ui::session_list::session_title`]) deliberately does
/// NOT render it.
pub(super) fn format_session_list_row(
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
    let suffix = if tags.is_empty() {
        String::new()
    } else {
        format!(" [{}]", tags.join(", "))
    };
    // `const pane = session.tmuxPane ? ` · tmux ${session.tmuxPane}` : ""` (`v0.12.0 index.ts:551`).
    // Same empty-string-means-omitted contract [`format_context_usage`] already uses: a session
    // outside tmux renders byte-for-byte the pre-v0.11.0 row — no column, no placeholder, no
    // dangling `·`. The term sits INSIDE the model parentheses, immediately after the context usage.
    //
    // Upstream's check is JS-falsy, so `""` already renders nothing. The `trim()` is a display-only
    // strengthening: a conforming peer cannot send whitespace-only (the producer trims —
    // [`crate::identity::current_tmux_pane`]), but a hostile one can, and ` · tmux    ` is exactly
    // the stray separator this must never print. It changes nothing on the wire.
    let pane = session
        .tmux_pane
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .map_or_else(String::new, |p| format!(" · tmux {p}"));
    format!(
        "• {} ({}) — {} ({}{}{}){}",
        name,
        id_prefix,
        session.cwd,
        session.model,
        format_context_usage(session),
        pane,
        suffix
    )
}

/// `pub(crate)` so the bundled-skill check (`crate::resources`, ICOM-004) can assert that every
/// action the shipped `SKILL.md` tells the model to call is actually advertised here — a skill that
/// documents an unadvertised action instructs the model into a preflight rejection.
pub(crate) fn parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["list", "list-cwd", "send", "ask", "reply", "pending", "status", "cancel"],
                "description": "The intercom action to perform."
            },
            // `v0.12.0 index.ts:1831-1833`.
            "cwd": {
                "type": "string",
                "description": "Working directory filter for 'list-cwd'. For send/ask, scopes target lookup to that directory; omit 'to' to target the sole live peer there. Absolute, or relative to the current session's cwd; '.' means the current cwd."
            },
            // `v0.12.0 index.ts:2175-2180`, verbatim apart from `Pi` -> `cyrup`.
            "openProjectPaneIfMissing": {
                "type": "boolean",
                "description": "For send/ask with cwd, open a visible Herdr project pane and launch cyrup there when no matching live session is connected."
            },
            "focus": {
                "type": "boolean",
                "description": "For openProjectPaneIfMissing, focus the new Herdr pane. Defaults to true."
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
///
/// The two arm-level tests below (`list-cwd`, `pending`) stay here because splitting the file
/// made them writable without a subprocess: each is a direct call to the named `action_*`
/// function over a 40-line in-process `fake_broker` that answers `register` and `list` and
/// nothing else. Neither spawns a binary, so neither is a seam test.
#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use crate::identity::short_session_id;
    use crate::transport::protocol::{Message, MessageContent, now_ms};

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

        assert_eq!(
            tool.label(),
            Some("Intercom"),
            "`v0.10.1 index.ts:1781` `label: \"Intercom\"`"
        );
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
            vec![
                "list", "list-cwd", "send", "ask", "reply", "pending", "status", "cancel"
            ],
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
                format!(
                    "- {} · {} · 0s ago · {}",
                    who, c.message.id, c.message.content.text
                )
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
        assert!(
            preview.starts_with("word spaced out "),
            "whitespace collapsed: {preview:?}"
        );
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
        assert_ne!(
            row_a, row_b,
            "two peers must not print the same addressable id"
        );
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
            let peer = SessionInfo {
                endpoint_epoch: None,
                name: name.map(str::to_string),
                ..session("peer-1", "/w/proj")
            };
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

    /// A minimal in-process stand-in for the broker: it answers the `register` handshake and every
    /// `list` request with the roster it was handed. That is the whole of what `action_list_cwd`
    /// asks of a connection (`client.list_sessions()`), and `action_pending` asks nothing of it at
    /// all — so this replaces the real `cyrup-intercom-broker` subprocess these two arms would
    /// otherwise need, which is why they had no test before the file was split.
    ///
    /// Unix-domain-socket specific (`UnixListener`), matching the socket tests in
    /// [`crate::transport::client`]; the behaviour asserted below is transport-neutral.
    #[cfg(unix)]
    async fn fake_broker(
        self_id: &str,
        sessions: Vec<SessionInfo>,
    ) -> (
        Arc<crate::transport::client::IntercomClient>,
        tempfile::TempDir,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        use crate::transport::framing::{FrameReader, encode_json};
        use crate::transport::protocol::{BrokerMessage, SessionRegistration};

        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("broker.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind");
        let assigned = self_id.to_string();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut reader = FrameReader::new();
            let mut buf = vec![0u8; 8192];
            loop {
                let n = match stream.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                for payload in reader.push(&buf[..n]).expect("frames") {
                    let frame: serde_json::Value = serde_json::from_slice(&payload).expect("json");
                    let reply = match frame["type"].as_str() {
                        Some("register") => BrokerMessage::Registered {
                            session_id: assigned.clone(),
                            features: None,
                        },
                        Some("list") => BrokerMessage::Sessions {
                            request_id: frame["requestId"]
                                .as_str()
                                .expect("a list frame carries requestId")
                                .to_string(),
                            sessions: sessions.clone(),
                        },
                        _ => continue,
                    };
                    let bytes = encode_json(&reply).expect("encodes");
                    stream.write_all(&bytes).await.expect("write");
                }
            }
        });

        let registration = SessionRegistration {
            runtime_fallback_alias: None,
            name: None,
            cwd: "/w/proj".to_string(),
            model: "m".to_string(),
            pid: 1u32.into(),
            started_at: now_ms().into(),
            last_activity: now_ms().into(),
            status: None,
            tmux_pane: None,
            extra: Default::default(),
        };
        let client = Arc::new(
            crate::transport::client::IntercomClient::connect(
                &socket_path,
                registration,
                Some(self_id.to_string()),
            )
            .await
            .expect("registers with the fake broker"),
        );
        (client, dir)
    }

    #[cfg(unix)]
    fn tool() -> IntercomTool {
        IntercomTool::new(Arc::new(SharedIntercomState::new(
            crate::config::IntercomConfig::default(),
            600_000,
            std::path::PathBuf::from("/w/proj"),
        )))
    }

    #[cfg(unix)]
    fn action(value: serde_json::Value) -> IntercomParams {
        serde_json::from_value(value).expect("the schema-shaped params deserialize")
    }

    #[cfg(unix)]
    fn result_text(result: &cyrup_core::ToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|c| match c {
                cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// `v0.10.1 index.ts:1895-1941` — `list-cwd` end to end, the arm no test in the repo reached
    /// before this file was split (the only way in was the crate-private 592-line `dispatch`,
    /// through a JSON `action` string, over a real broker subprocess).
    ///
    /// All three of the arm's decisions are asserted: the default filter is the BROKER-reported cwd
    /// of this session (not the locally captured one), an explicit `cwd` overrides it, and a
    /// filter that matches nothing while the session's own cwd has peers appends `:1913-1924`'s
    /// fail-loud pointer instead of a bare "no sessions" that reads as a misleading empty result.
    #[cfg(unix)]
    #[tokio::test]
    async fn list_cwd_filters_the_roster_to_one_directory_and_points_back_when_it_is_empty() {
        let roster = vec![
            SessionInfo {
                endpoint_epoch: None,
                name: Some("me".to_string()),
                ..session("self-1", "/w/proj")
            },
            SessionInfo {
                endpoint_epoch: None,
                name: Some("peer-here".to_string()),
                ..session("s-here", "/w/proj")
            },
            SessionInfo {
                endpoint_epoch: None,
                name: Some("peer-there".to_string()),
                ..session("s-there", "/w/other")
            },
        ];
        let (client, _dir) = fake_broker("self-1", roster).await;
        let tool = tool();

        // No `cwd`: `"."`, i.e. the current session's own broker-reported cwd.
        let here = result_text(
            &tool
                .action_list_cwd(
                    &action(serde_json::json!({ "action": "list-cwd" })),
                    &client,
                )
                .await
                .expect("list-cwd answers"),
        );
        assert!(
            here.contains("**Other sessions (cwd: /w/proj):**"),
            "{here}"
        );
        assert!(
            here.contains("peer-here"),
            "the peer in this cwd is listed: {here}"
        );
        assert!(
            !here.contains("peer-there"),
            "the peer in another cwd is filtered out: {here}"
        );
        assert!(here.starts_with("**Current session:**\n• me ("), "{here}");

        // An explicit absolute `cwd` overrides the default.
        let there = result_text(
            &tool
                .action_list_cwd(
                    &action(serde_json::json!({ "action": "list-cwd", "cwd": "/w/other" })),
                    &client,
                )
                .await
                .expect("list-cwd answers"),
        );
        assert!(
            there.contains("**Other sessions (cwd: /w/other):**"),
            "{there}"
        );
        assert!(there.contains("peer-there"), "{there}");
        assert!(!there.contains("• peer-here"), "{there}");

        // `:1913-1924` — empty filter + peers in the session's OWN cwd = the fail-loud pointer.
        let empty = result_text(
            &tool
                .action_list_cwd(
                    &action(serde_json::json!({ "action": "list-cwd", "cwd": "/w/nobody" })),
                    &client,
                )
                .await
                .expect("list-cwd answers"),
        );
        assert!(
            empty.contains(
                "No other sessions in this directory. Your session's cwd is /w/proj (1 peer there) \
                 — call list-cwd without a cwd argument to list them."
            ),
            "`v0.10.1 index.ts:1913-1924`'s fail-loud note, verbatim: {empty}"
        );
    }

    /// `v0.10.1 index.ts:1740-1755` — `pending` end to end, the other arm no test in the repo
    /// reached. Both branches: the empty answer, and one row per unresolved inbound ask carrying
    /// the MESSAGE id (`reply_tracker.rs:126` tells the model to quote it back as `replyTo`, so a
    /// row without it is an unbreakable loop of failing replies).
    #[cfg(unix)]
    #[tokio::test]
    async fn pending_answers_empty_then_one_row_per_unresolved_inbound_ask() {
        let (client, _dir) = fake_broker("self-1", Vec::new()).await;
        let tool = tool();
        let params = action(serde_json::json!({ "action": "pending" }));

        let empty = result_text(
            &tool
                .action_pending(&params, &client)
                .await
                .expect("pending answers"),
        );
        assert_eq!(empty, "No unresolved inbound asks.", "the empty branch");

        {
            let mut tracker = tool.state.tracker.lock().unwrap();
            let now = now_ms();
            // Two asks from the SAME sender: the case that forces `replyTo` over a sender name.
            tracker.record_incoming_message(session("s1", "/w/proj"), ask_message("m-first"), now);
            tracker.record_incoming_message(session("s1", "/w/proj"), ask_message("m-second"), now);
        }

        let rows = result_text(
            &tool
                .action_pending(&params, &client)
                .await
                .expect("pending answers"),
        );
        assert!(
            rows.starts_with("**Pending asks:**\n"),
            "pi's header: {rows}"
        );
        assert_eq!(
            rows.lines().count(),
            3,
            "header plus one row per ask: {rows}"
        );
        for id in ["m-first", "m-second"] {
            assert!(
                rows.contains(&format!("- s1 · {id} · 0s ago · hi")),
                "{rows}"
            );
        }
    }

    fn session(id: &str, cwd: &str) -> SessionInfo {
        SessionInfo {
            endpoint_epoch: None,
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
            tmux_pane: None,
            extra: Default::default(),
        }
    }

    fn ask_message(id: &str) -> Message {
        Message {
            id: id.to_string(),
            timestamp: now_ms().into(),
            reply_to: None,
            expects_reply: Some(true),
            content: MessageContent {
                text: "hi".to_string(),
                attachments: None,
                ..Default::default()
            },
            ..Default::default()
        }
    }
}
