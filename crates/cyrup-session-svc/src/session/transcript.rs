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
        self.fanout_emit(AgentSessionEvent::SessionInfoChanged {
            name: resolved.clone(),
        })
        .await;
        // EXT-011 — the SAME rename is an EXTENSION event upstream (`SessionInfoChangedEvent`,
        // `extensions/types.ts:571-575` @v0.83.0). The kind, the WIT export and the SDK hook all
        // existed; nothing EMITTED it, so a guest could subscribe to `session_info_changed` and
        // never be called. Notify-only, so it cannot block the rename.
        let notify_cancel = self.session_cancel.child_token();
        self.services
            .ext_host
            .dispatcher()
            .dispatch_notify(
                &HostEvent::SessionInfoChanged { name: resolved },
                &notify_cancel,
            )
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

    /// The palette an export from THIS session renders with — the imperative shell around the pure
    /// [`crate::export::session_jsonl_to_html_with_theme`].
    ///
    /// Pi resolves the theme inside `generateHtml` from module-global state
    /// (`getResolvedThemeColors(themeName)` → `themeName ?? currentThemeName ?? getDefaultTheme()`,
    /// `theme.ts:1065` @v0.84.4). cyrup has no such global: the ACTIVE theme name lives behind the
    /// interactive TUI's [`crate::ThemeAccess`] handle (republished every frame, so a
    /// `/settings → theme` switch made a keystroke ago is visible here) and the theme DOCUMENTS live
    /// in this session's discovered-resource snapshot. Reading both here keeps the renderer a pure
    /// function of `(jsonl, palette)`.
    ///
    /// Unattached — headless `print`/`json`/`rpc` — falls through to
    /// [`crate::ExportTheme::default`], which is pi's `getDefaultTheme()` arm.
    pub fn export_theme(&self) -> crate::export::ExportTheme {
        use cyrup_ext::host::HostServices as _;
        self.services
            .host_services
            .theme()
            .and_then(|name| self.services.resources.themes.get_name(&name))
            .map(crate::export::ExportTheme::from_theme)
            .unwrap_or_default()
    }

    /// The leaf the exported document must be walked from — pi's `sm.getLeafId()`, which
    /// `exportSessionToHtml` reads straight off the live manager (`core/export-html/index.ts:266`
    /// @v0.84.4).
    ///
    /// Threaded through the shell for the same reason [`Self::export_theme`] is: the renderer stays
    /// a pure function of `(jsonl, palette, leaf)` and never re-derives state the manager owns. It
    /// MUST NOT be re-derived from the JSONL — `SessionManager::branch` moves the leaf without
    /// appending and `reset_leaf` clears it, so after a `/tree` branch switch with no new message
    /// the last line of the file belongs to the abandoned branch and `template.js` would walk the
    /// wrong conversation.
    pub async fn export_leaf_id(&self) -> Option<String> {
        let guard = self.manager.lock().await;
        guard.leaf_id().map(|id| id.as_str().to_string())
    }

    /// Export the current session branch to a standalone HTML document (Pi `exportToHtml`,
    /// agent-session.ts:3022 → `exportSessionToHtml`, `core/export-html/index.ts:236-282`
    /// @v0.84.4). With `path` the document is written there; otherwise the Pi default
    /// `cyrup-session-<basename>.html` (in the session cwd, basename = the session-file stem, else the
    /// session id) is used. Returns the resolved output path.
    ///
    /// The document is the templated one pi ships (tree sidebar, markdown, highlighting, the user's
    /// theme) — see [`crate::export`]. Its one residual is `renderedTools`, the pre-rendered
    /// EXTENSION tool cards, which pi's own `exportFromFile` also omits.
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
        let html = crate::export::session_jsonl_to_html_with_theme(
            &jsonl,
            &self.export_theme(),
            self.export_leaf_id().await.as_deref(),
        );
        let out = match path {
            Some(p) => p.to_path_buf(),
            None => {
                let basename = self
                    .session_file()
                    .await
                    .and_then(|f| f.file_stem().map(|s| s.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| self.session_id().as_str().to_string());
                self.services
                    .cwd
                    .join(format!("cyrup-session-{basename}.html"))
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
        guard
            .tree()
            .iter()
            .map(crate::host_services::tree_node_to_json)
            .collect()
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

    /// The FULL, untruncated copy text of one entry — Pi `TreeList.getEntryCopyText`
    /// (`tree-selector.ts:896-922`), reached from `copySelected` (`:627-630`) when `app.message.copy`
    /// fires inside `/tree` (`:1029-1030`).
    ///
    /// `None` for an unknown id and for every entry Pi's switch leaves `undefined` — including a
    /// whitespace-only body, which upstream collapses with `return text?.trim() ? text : undefined`.
    ///
    /// This is deliberately NOT a field on [`SessionDagNode`]. The DAG node carries `label`, which is
    /// a normalized ONE-LINE preview clipped to 80 characters by [`dag_display`]; the copy text is the
    /// whole message body, so materialising it for every node would put the entire session transcript
    /// into the `/tree` selector's node list to serve a single keystroke. The `/tree` chrome resolves
    /// it on demand against the live session instead, exactly as `/copy` resolves
    /// [`Self::last_assistant_text`].
    pub async fn entry_copy_text(&self, entry_id: &EntryId) -> Option<String> {
        let guard = self.manager.lock().await;
        guard
            .entries()
            .iter()
            .find(|e| &e.id() == entry_id)
            .and_then(dag_copy_text)
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
        timestamp: node
            .entry
            .base()
            .map(|b| b.timestamp.clone())
            .unwrap_or_default(),
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
            SessMsg::Core(Message::User { content, .. }) => (
                SessionDagKind::Message,
                clip(format!("user: {}", normalize(&join_text(content)))),
            ),
            SessMsg::Core(Message::Assistant(m)) => {
                let text = normalize(&join_text(&m.content));
                let body = if text.is_empty() {
                    "(no content)".to_string()
                } else {
                    text
                };
                (SessionDagKind::Message, clip(format!("assistant: {body}")))
            }
            SessMsg::Core(Message::ToolResult { tool_name, .. }) => {
                (SessionDagKind::Tool, format!("[{tool_name}]"))
            }
            SessMsg::BashExecution(b) => (
                SessionDagKind::Message,
                clip(format!("[bash]: {}", normalize(&b.command))),
            ),
            SessMsg::Custom(c) => (SessionDagKind::Message, format!("[{}]", c.custom_type)),
            // Pi's `AgentMessage` union also admits the two summary roles inside a `type:"message"`
            // entry; label them like the equivalent standalone entries.
            SessMsg::BranchSummary(b) => (
                SessionDagKind::Compaction,
                clip(format!("branch summary: {}", normalize(&b.summary))),
            ),
            SessMsg::CompactionSummary(_) => (SessionDagKind::Compaction, "compaction".to_string()),
        },
        Entry::Known(KnownEntry::ModelChange { model_id, .. }) => {
            (SessionDagKind::ModelChange, format!("model → {model_id}"))
        }
        Entry::Known(KnownEntry::ThinkingLevelChange { thinking_level, .. }) => (
            SessionDagKind::ThinkingChange,
            format!("thinking → {thinking_level}"),
        ),
        Entry::Known(KnownEntry::Compaction { .. }) => {
            (SessionDagKind::Compaction, "compaction".to_string())
        }
        Entry::Known(KnownEntry::BranchSummary { summary, .. }) => (
            SessionDagKind::Compaction,
            clip(format!("branch summary: {}", normalize(summary))),
        ),
        Entry::Known(KnownEntry::SessionInfo { name, .. }) => (
            SessionDagKind::Other,
            format!("title: {}", name.clone().unwrap_or_default()),
        ),
        Entry::Known(KnownEntry::CustomMessage { custom_type, .. }) => {
            (SessionDagKind::Other, format!("[{custom_type}]"))
        }
        Entry::Known(KnownEntry::Custom { custom_type, .. }) => {
            (SessionDagKind::Other, format!("custom {custom_type}"))
        }
        Entry::Known(KnownEntry::Label { label, .. }) => (
            SessionDagKind::Other,
            format!("label {}", label.clone().unwrap_or_default()),
        ),
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

/// Pi `extractFullContent` (`tree-selector.ts:883-895`) for the JSON-valued `content` fields
/// (`CustomRoleMessage.content` and `KnownEntry::CustomMessage.content`, both mirrored as raw JSON
/// because Pi types them `string | (Text|Image)[]`).
///
/// A JSON string is returned as-is; an array concatenates the `text` of every `{"type":"text"}`
/// block; anything else (including `null`) is the empty string. The typed `Vec<Content>` cases use
/// [`join_text`], which is already exactly this over the parsed representation.
fn extract_full_content(content: &serde_json::Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    let Some(blocks) = content.as_array() else {
        return String::new();
    };
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(serde_json::Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

/// The full copy text of one entry — a mechanical port of Pi's `getEntryCopyText`
/// (`tree-selector.ts:896-922`). Returns `(kind, label)`'s counterpart for the CLIPBOARD, i.e. the
/// untruncated body: no `normalize`, no `clip`, and no role prefix, all of which belong to
/// [`dag_display`] alone.
///
/// Upstream's switch is on `entry.type`, and the `message` arm then tests the message ROLE:
///
/// * `bashExecution` → `entry.message.command` (`:901`)
/// * any role whose message has a `content` key → `extractFullContent(content)`; when that comes out
///   empty **and** the role is `assistant`, fall back to `entry.message.errorMessage` (`:902-907`).
///   In Pi's message union that key set is `user`, `assistant`, `toolResult` and `custom`
///   (`ai/src/types.ts:379,402`, `messages.ts:46-52`) — the `branchSummary` and `compactionSummary`
///   roles carry `summary`, not `content`, so `"content" in entry.message` is FALSE for them and the
///   text stays `undefined` even though the ENTRY type is `message`.
/// * entry type `custom_message` → `extractFullContent(entry.content)` (`:910`)
/// * entry types `compaction` / `branch_summary` → `entry.summary` (`:913`, `:916`)
/// * every other entry type falls out of the switch with `text` still `undefined`.
///
/// Tool CALLS are not rendered here: `formatToolCall` (`:938-994`) is on the row-label path only
/// (reached at `:799`), never on the copy path.
fn dag_copy_text(e: &cyrup_session::Entry) -> Option<String> {
    use cyrup_session::agent_message::AgentMessage as SessMsg;
    use cyrup_session::entry::{Entry, KnownEntry};

    let text: Option<String> = match e {
        Entry::Known(KnownEntry::Message { message, .. }) => match message {
            SessMsg::BashExecution(b) => Some(b.command.clone()),
            SessMsg::Core(Message::User { content, .. })
            | SessMsg::Core(Message::ToolResult { content, .. }) => Some(join_text(content)),
            SessMsg::Core(Message::Assistant(m)) => {
                // `:904-906` — the `errorMessage` fallback fires only when the extracted text is
                // empty AND the role is `assistant`.
                let body = join_text(&m.content);
                if body.is_empty() {
                    m.error_message.clone()
                } else {
                    Some(body)
                }
            }
            SessMsg::Custom(c) => Some(extract_full_content(&c.content)),
            SessMsg::BranchSummary(_) | SessMsg::CompactionSummary(_) => None,
        },
        Entry::Known(KnownEntry::CustomMessage { content, .. }) => {
            Some(extract_full_content(content))
        }
        Entry::Known(KnownEntry::Compaction { summary, .. })
        | Entry::Known(KnownEntry::BranchSummary { summary, .. }) => Some(summary.clone()),
        _ => None,
    };
    // `:921` — `return text?.trim() ? text : undefined`: a whitespace-only body is `undefined`, and
    // the value returned is the UNTRIMMED original.
    text.filter(|t| !t.trim().is_empty())
}

/// The concatenated text of a core `user` message entry, or `None` for any other entry/role.
pub(super) fn user_message_text(e: &cyrup_session::Entry) -> Option<String> {
    use cyrup_session::agent_message::AgentMessage as SessMsg;
    use cyrup_session::entry::{Entry, KnownEntry};
    let Entry::Known(KnownEntry::Message { message, .. }) = e else {
        return None;
    };
    let SessMsg::Core(Message::User { content, .. }) = message else {
        return None;
    };
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
