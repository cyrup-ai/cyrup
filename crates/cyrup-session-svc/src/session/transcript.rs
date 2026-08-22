//! Session naming, export and the JSON / DAG transcript views.
//!
//! Pi `getEntriesJson`/`getTree` + `getUserMessagesForForking`'s display half. The read-only
//! projections a front-end renders — the flat entry list, the tree, and the glyph-tagged
//! [`super::types::SessionDagNode`] rows — plus the JSONL/HTML export seams.

use std::path::Path;

use cyrup_core::{Content, EntryId, Message};
use cyrup_ext::HostEvent;

use crate::error::SessionServiceError;
use crate::event::AgentSessionEvent;

use super::AgentSession;
use super::types::{SessionDagKind, SessionDagNode};

impl AgentSession {
    /// The session's display name, if set (Pi `sessionName` getter, agent-session.ts:865).
    pub async fn session_name(&self) -> Option<String> {
        self.manager.lock().await.session_name()
    }

    /// Set the session's display name, persisting a `session_info` entry (Pi `setSessionName`,
    /// agent-session.ts:2690).
    pub async fn set_session_name(&self, name: &str) -> Result<(), SessionServiceError> {
        let resolved = {
            let mut guard = self.manager.lock().await;
            guard.append_session_info(name)?;
            guard.session_name()
        };
        // Emit `session_info_changed { name }` to every live subscription (Pi `_emit(event)`,
        // agent-session.ts:2714-2715); the `name` is re-read from the manager so it byte-matches Pi's
        // `getSessionName()` (an empty/whitespace name resolves to `None`).
        self.fanout_emit(AgentSessionEvent::SessionInfoChanged { name: resolved.clone() }).await;
        // EXT-011 — the SAME rename is an EXTENSION event upstream (`SessionInfoChangedEvent`,
        // `extensions/types.ts:571-575` @v0.83.0). The kind, the WIT export and the SDK hook all
        // existed; nothing EMITTED it, so a guest could subscribe to `session_info_changed` and
        // never be called. Notify-only, so it cannot block the rename.
        let notify_cancel = self.session_cancel.child_token();
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(&HostEvent::SessionInfoChanged { name: resolved }, &notify_cancel)
            .await;
        Ok(())
    }

    /// Export the current session tree as JSONL (Pi `exportToJsonl`, agent-session.ts:3052). With a
    /// `path` the bytes are written there; otherwise the JSONL text is returned.
    pub async fn export_to_jsonl(
        &self,
        path: Option<&Path>,
    ) -> Result<Option<String>, SessionServiceError> {
        let guard = self.manager.lock().await;
        let mut buf: Vec<u8> = Vec::new();
        guard.export_jsonl(&mut buf)?;
        drop(guard);
        let text = String::from_utf8_lossy(&buf).into_owned();
        match path {
            Some(p) => {
                std::fs::write(p, text).map_err(|e| SessionServiceError::Io(e.to_string()))?;
                Ok(None)
            }
            None => Ok(Some(text)),
        }
    }

    /// Export the current session branch to a standalone HTML document (Pi `exportToHtml`,
    /// agent-session.ts:3022). With `path` the document is written there; otherwise the Pi default
    /// `cyrup-session-<basename>.html` (in the session cwd, basename = the session-file stem, else the
    /// session id) is used. Returns the resolved output path. The rich per-tool HTML cards
    /// (`export-html/tool-renderer.ts`) remain the one L5 residual; the document is a real transcript.
    pub async fn export_to_html(
        &self,
        path: Option<&Path>,
    ) -> Result<std::path::PathBuf, SessionServiceError> {
        let jsonl = {
            let guard = self.manager.lock().await;
            let mut buf: Vec<u8> = Vec::new();
            guard.export_jsonl(&mut buf)?;
            String::from_utf8_lossy(&buf).into_owned()
        };
        let html = crate::export::session_jsonl_to_html(&jsonl);
        let out = match path {
            Some(p) => p.to_path_buf(),
            None => {
                let basename = self
                    .session_file()
                    .await
                    .and_then(|f| f.file_stem().map(|s| s.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| self.session_id().as_str().to_string());
                self.services.cwd.join(format!("cyrup-session-{basename}.html"))
            }
        };
        std::fs::write(&out, html).map_err(|e| SessionServiceError::Io(e.to_string()))?;
        Ok(out)
    }

    /// EVERY persisted entry except the session header, serialized (pi `get_entries`,
    /// rpc-mode.ts:609 → `SessionManager.getEntries()`, `core/session-manager.ts:1301`: "Get all
    /// session entries (excludes header)"). NOT the current branch — that is `getBranch()`
    /// (`:1260`), which the extension seam exposes separately as
    /// [`cyrup_ext::HostServices::branch`]. The body has always called `entries()`; only this line
    /// said "on the current branch".
    pub async fn entries_json(&self) -> Vec<serde_json::Value> {
        let guard = self.manager.lock().await;
        guard
            .entries()
            .iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect()
    }

    /// The session tree as `{entry, children, label?, labelTimestamp?}` nodes (Pi `get_tree`,
    /// rpc-mode.ts:622 → `SessionTreeNode`, `core/session-manager.ts:159-166`).
    ///
    /// SEAM-060 — `labelTimestamp` used to be dropped from every node, with an in-tree comment
    /// declaring the omission deliberate. It is not vestigial upstream: `labelTimestampsById` is
    /// maintained at `session-manager.ts:865`, `:970` and `:1247-1250` and read into the node at
    /// `:1318`, and the wire contract names the type directly (`modes/rpc/rpc-types.ts:202-208`).
    /// Without it a client cannot sort or age branch labels, cannot render "renamed 2 days ago",
    /// and cannot spot a label that predates the entries beneath it. Emitted with the same
    /// omit-when-`None` insert as `label`, so an unlabelled node still carries neither key.
    ///
    /// The per-node serializer is [`crate::host_services::tree_node_to_json`], shared with
    /// [`cyrup_ext::HostServices::tree`]: both must emit pi's one `SessionTreeNode` shape, and a
    /// second private copy here is precisely how the SEAM-060 omission survived on one side.
    pub async fn tree_json(&self) -> Vec<serde_json::Value> {
        let guard = self.manager.lock().await;
        guard.tree().iter().map(crate::host_services::tree_node_to_json).collect()
    }

    /// The **flattened session DAG** for the `/tree` selector (feature #2): the manager's real branch
    /// tree (`SessionManager::tree`) walked in pre-order into [`SessionDagNode`]s carrying parent/depth/
    /// label/kind/fold/leaf/label/timestamp — the flat-DAG getter the connector/fold/filter engine in
    /// `cyrup-tui::tree_selector` was data-starved for (audit: `/tree` showed a flat user-message
    /// list). Mirrors Pi `flattenTree` over `SessionManager.getTree()` (`tree-selector.ts:199-320`).
    pub async fn session_dag(&self) -> Vec<SessionDagNode> {
        let guard = self.manager.lock().await;
        let leaf = guard.leaf_id().cloned();
        let mut out = Vec::new();
        for root in guard.tree() {
            flatten_dag_node(&root, None, 0, leaf.as_ref(), &mut out);
        }
        out
    }
}

/// Recursively flatten a [`cyrup_session::manager::TreeNode`] into pre-order [`SessionDagNode`]s
/// (feature #2 helper; Pi `flattenTree` DFS, `tree-selector.ts:199-320`). Children are already
/// timestamp-sorted by the manager (`build_node`). `depth` is the pre-order tree depth (0 = root).
fn flatten_dag_node(
    node: &cyrup_session::manager::TreeNode,
    parent_id: Option<EntryId>,
    depth: usize,
    leaf: Option<&EntryId>,
    out: &mut Vec<SessionDagNode>,
) {
    let id = node.entry.id();
    let (kind, label) = dag_display(&node.entry);
    let label = match &node.label {
        Some(l) => format!("[{l}] {label}"),
        None => label,
    };
    out.push(SessionDagNode {
        entry_id: id.clone(),
        parent_id,
        depth,
        label,
        kind,
        foldable: !node.children.is_empty(),
        is_leaf: leaf == Some(&id),
        has_label: node.label.is_some(),
        timestamp: node.entry.base().map(|b| b.timestamp.clone()).unwrap_or_default(),
    });
    for child in &node.children {
        flatten_dag_node(child, Some(id.clone()), depth + 1, leaf, out);
    }
}

/// Classify an entry and derive its one-line tree label (Pi `getEntryDisplayText`,
/// `tree-selector.ts:762-830`, condensed to a single normalized line). Returns `(kind, label)`.
fn dag_display(e: &cyrup_session::Entry) -> (SessionDagKind, String) {
    use cyrup_session::agent_message::AgentMessage as SessMsg;
    use cyrup_session::entry::{Entry, KnownEntry};

    let normalize = |s: &str| s.replace(['\n', '\t'], " ").trim().to_string();
    let clip = |s: String| -> String {
        let out: String = s.chars().take(80).collect();
        out
    };
    match e {
        Entry::Known(KnownEntry::Message { message, .. }) => match message {
            SessMsg::Core(Message::User { content, .. }) => {
                (SessionDagKind::Message, clip(format!("user: {}", normalize(&join_text(content)))))
            }
            SessMsg::Core(Message::Assistant(m)) => {
                let text = normalize(&join_text(&m.content));
                let body = if text.is_empty() { "(no content)".to_string() } else { text };
                (SessionDagKind::Message, clip(format!("assistant: {body}")))
            }
            SessMsg::Core(Message::ToolResult { tool_name, .. }) => {
                (SessionDagKind::Tool, format!("[{tool_name}]"))
            }
            SessMsg::BashExecution(b) => {
                (SessionDagKind::Message, clip(format!("[bash]: {}", normalize(&b.command))))
            }
            SessMsg::Custom(c) => (SessionDagKind::Message, format!("[{}]", c.custom_type)),
            // Pi's `AgentMessage` union also admits the two summary roles inside a `type:"message"`
            // entry; label them like the equivalent standalone entries.
            SessMsg::BranchSummary(b) => (
                SessionDagKind::Compaction,
                clip(format!("branch summary: {}", normalize(&b.summary))),
            ),
            SessMsg::CompactionSummary(_) => {
                (SessionDagKind::Compaction, "compaction".to_string())
            }
        },
        Entry::Known(KnownEntry::ModelChange { model_id, .. }) => {
            (SessionDagKind::ModelChange, format!("model → {model_id}"))
        }
        Entry::Known(KnownEntry::ThinkingLevelChange { thinking_level, .. }) => {
            (SessionDagKind::ThinkingChange, format!("thinking → {thinking_level}"))
        }
        Entry::Known(KnownEntry::Compaction { .. }) => {
            (SessionDagKind::Compaction, "compaction".to_string())
        }
        Entry::Known(KnownEntry::BranchSummary { summary, .. }) => {
            (SessionDagKind::Compaction, clip(format!("branch summary: {}", normalize(summary))))
        }
        Entry::Known(KnownEntry::SessionInfo { name, .. }) => {
            (SessionDagKind::Other, format!("title: {}", name.clone().unwrap_or_default()))
        }
        Entry::Known(KnownEntry::CustomMessage { custom_type, .. }) => {
            (SessionDagKind::Other, format!("[{custom_type}]"))
        }
        Entry::Known(KnownEntry::Custom { custom_type, .. }) => {
            (SessionDagKind::Other, format!("custom {custom_type}"))
        }
        Entry::Known(KnownEntry::Label { label, .. }) => {
            (SessionDagKind::Other, format!("label {}", label.clone().unwrap_or_default()))
        }
        Entry::Unknown(_) => (SessionDagKind::Other, "(entry)".to_string()),
    }
}

/// Join the `text` parts of a content vector (helper for [`dag_display`]).
fn join_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// The concatenated text of a core `user` message entry, or `None` for any other entry/role.
pub(super) fn user_message_text(e: &cyrup_session::Entry) -> Option<String> {
    use cyrup_session::agent_message::AgentMessage as SessMsg;
    use cyrup_session::entry::{Entry, KnownEntry};
    let Entry::Known(KnownEntry::Message { message, .. }) = e else { return None };
    let SessMsg::Core(Message::User { content, .. }) = message else { return None };
    let text: String = content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    Some(text)
}

/// The text of a `custom_message` entry (Pi `agent-session.ts:2833-2840`): a raw string is used as
/// is; an array is filtered to its `text` parts and joined.
pub(super) fn custom_message_text(content: &serde_json::Value) -> String {
    use serde_json::Value;
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|c| {
                if c.get("type").and_then(Value::as_str) == Some("text") {
                    c.get("text").and_then(Value::as_str).map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}
