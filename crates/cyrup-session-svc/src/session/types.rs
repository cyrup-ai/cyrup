//! The value types the seam's signatures name — fork/tree anchors, navigation options, the
//! scoped-model rows and the session-file delete method.
//!
//! Split out of `session.rs` verbatim; these are the types [`crate`] re-exports alongside
//! [`super::AgentSession`] itself.

// Doc-only: the types below document themselves against the seam that produces them, but none of
// them names `AgentSession` in code. `cfg(doc)` keeps the intra-doc links resolvable without an
// `unused_imports` warning in a normal build.
#[cfg(doc)]
use super::AgentSession;

use cyrup_core::{EntryId, ModelThinkingLevel, SessionId};
use cyrup_provider::Model;

/// Where a fork anchors relative to the selected entry (Pi `fork(entryId, {position})`,
/// agent-session-runtime.ts:259). `Before` anchors at the selected *user* message's parent and
/// extracts its text (for re-editing); `At` anchors at the selected entry itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ForkPosition {
    #[default]
    Before,
    At,
}

impl ForkPosition {
    /// pi's own literal — `position: "before" | "at"` (`core/extensions/types.ts:585-589`
    /// @v0.83.0), carried on `session_before_fork`. SEAM-012.
    pub const fn as_str(self) -> &'static str {
        match self {
            ForkPosition::Before => "before",
            ForkPosition::At => "at",
        }
    }
}

/// The outcome of an entry-anchored fork (Pi returns `{cancelled, selectedText}`,
/// agent-session-runtime.ts:262).
#[derive(Clone, Debug, Default)]
pub struct ForkOutcome {
    /// The new branched session id (the forked file's session id), if a new file was created.
    pub session_id: Option<SessionId>,
    /// For `position:"before"`, the selected user message's text (so a UI can pre-fill the editor).
    pub selected_text: Option<String>,
}

/// A single user message anchor for the `/tree`/`/fork` pickers (Pi `getUserMessagesForForking`,
/// agent-session.ts:2901).
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkAnchor {
    pub entry_id: EntryId,
    pub text: String,
}

/// The entry-type classification of a [`SessionDagNode`], mirroring the glyph switch Pi's tree
/// selector keys off (`tree-selector.ts:567-611`, `:762`). Kept UI-agnostic (a plain tag) so the
/// TUI maps it to its own `TreeKind` glyph without this layer depending on cyrup-tui.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionDagKind {
    /// A user/assistant/bash message entry (`●`).
    Message,
    /// A `tool_result` message entry (`⚙`).
    Tool,
    /// A `model_change` entry (`◆`).
    ModelChange,
    /// A `thinking_level_change` entry (`◇`).
    ThinkingChange,
    /// A `compaction` or `branch_summary` entry (`✓`).
    Compaction,
    /// Anything else (`session_info`/`label`/`custom`/unknown) — rendered as a message.
    Other,
}

/// One node of the **flattened session DAG** (feature #2): the flat-tree getter the `/tree` selector
/// was starved for. Produced by [`AgentSession::session_dag`] by walking the manager's real branch
/// tree (`SessionManager::tree`) in pre-order, carrying each node's parent link, depth, display label,
/// kind, fold-ability (has children), leaf-ness (the active branch tip), user-label, and timestamp —
/// exactly the `FlatNode` fields Pi's `flattenTree` computes (`tree-selector.ts:27-35`, `:199-320`).
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDagNode {
    /// The entry id (the branch/summarize target on confirm).
    pub entry_id: EntryId,
    /// The parent entry id (`None` for a root).
    pub parent_id: Option<EntryId>,
    /// Pre-order tree depth (0 = a root; roots get no connector, spec/tui/05 §5.1).
    pub depth: usize,
    /// The one-line display label (role-prefixed message text / `model → id` / `thinking → level` / …).
    pub label: String,
    /// The entry-type classification driving the row glyph.
    pub kind: SessionDagKind,
    /// Whether this node has descendants (renders the foldable `⊟`/`⊞` marker).
    pub foldable: bool,
    /// Whether this node is the current branch leaf (the active tip).
    pub is_leaf: bool,
    /// Whether the entry carries a user label (renders the `☆` star).
    pub has_label: bool,
    /// The entry's RFC3339 timestamp (drives the right-aligned time column).
    pub timestamp: String,
}

/// Options for the unified `/tree` navigation op (Pi `navigateTree(targetId, options)`,
/// agent-session.ts:2704). `summarize` runs the branch summarizer over the abandoned branch;
/// `custom_instructions`/`replace_instructions` steer that summary prompt (Pi
/// `branch-summarization.ts:318-336`); `label` is attached to the resulting summary entry (or, when
/// not summarizing, to the navigation target).
#[derive(Clone, Debug, Default)]
pub struct NavigateTreeOptions {
    pub summarize: bool,
    pub custom_instructions: Option<String>,
    pub replace_instructions: bool,
    pub label: Option<String>,
}

/// The outcome of [`AgentSession::navigate_tree`] (Pi navigateTree return,
/// agent-session.ts:2710): `editor_text` is the re-editable text when the target is a user/custom
/// message; `cancelled` is set when the op was a no-op or an extension vetoed it; `aborted` is set
/// when an in-flight summarization was cancelled; `summary_entry` is the appended branch summary.
#[derive(Clone, Debug, Default)]
pub struct NavigateTreeOutcome {
    pub editor_text: Option<String>,
    pub cancelled: bool,
    pub aborted: bool,
    pub summary_entry: Option<cyrup_session::compaction::BranchSummaryEntry>,
}

/// Which summarization a [`ReplayItem::CompactionCost`] is reporting — pi's
/// `CompactionCostNotice.kind`, which carries the source `entry.type`
/// (`modes/interactive/interactive-mode.ts:3790-3792` @v0.83.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionCostKind {
    /// A context compaction (`entry.type === "compaction"`).
    Compaction,
    /// A branch summarization (`entry.type === "branch_summary"`).
    BranchSummary,
}

/// One item of a replayable conversation — the raw-context message stream plus the two derived
/// notices a front-end cannot reconstruct from the messages alone.
///
/// This is pi's `RenderSessionItem` (`interactive-mode.ts:217`), minus its `custom` entry variant
/// (cyrup's raw projection already carries custom messages as
/// [`cyrup_session::agent_message::AgentMessage::Custom`]) and with the cache-miss notice
/// materialised as an item instead of being looked up by `AssistantMessage` object identity during
/// the walk (`:3694-3696`, `:3753-3755`).
///
/// The derivation has to live here, not in the front-end, because it bridges two index spaces the
/// front-end never sees at once: [`cyrup_provider::cache_stats::collect_cache_misses`] keys misses
/// by index into the FLAT entry list, while the replay stream is the CURRENT BRANCH's
/// post-compaction-admission projection. [`cyrup_session::context::build_context_agent_messages_tagged`]
/// carries the [`cyrup_core::EntryId`] that joins them.
#[derive(Clone, Debug)]
pub enum ReplayItem {
    /// A raw-context message, in stream order. Boxed: an `AgentMessage` is an order of magnitude
    /// wider than either notice, and an unboxed variant would pad every notice in the stream out to
    /// its size (`clippy::large_enum_variant`).
    Message(Box<cyrup_session::agent_message::AgentMessage>),
    /// A counted prompt-cache miss on the assistant message immediately preceding it — pi's
    /// `cacheMisses.get(message)` re-injection (`interactive-mode.ts:3753-3755`).
    CacheMiss(cyrup_provider::cache_stats::CacheMiss),
    /// What the compaction / branch summary immediately preceding it cost — pi's synthesised
    /// `{type: "compaction_cost", kind, usage}` (`interactive-mode.ts:3790-3792`).
    CompactionCost {
        kind: CompactionCostKind,
        usage: cyrup_core::Usage,
    },
}

/// A scoped model in the `cycle_model` set (Pi `{model, thinkingLevel?}`, agent-session.ts:870). An
/// explicit `thinking_level` overrides the session level when cycled to; `None` inherits it.
#[derive(Clone, Debug)]
pub struct ScopedModel {
    pub model: Model,
    pub thinking_level: Option<ModelThinkingLevel>,
}

/// The typed result of [`AgentSession::cycle_model`] (Pi `ModelCycleResult`, agent-session.ts:1471).
/// `is_scoped` distinguishes the scoped-set path from the full-catalog path.
#[derive(Clone, Debug)]
pub struct ModelCycleResult {
    pub model: Model,
    pub thinking_level: ModelThinkingLevel,
    pub is_scoped: bool,
}

/// Options for [`AgentSession::bind_extensions_with`] — pi's `bindExtensions({ … })` argument
/// (`modes/print-mode.ts:73-101` @v0.83.0). Only the key cyrup did not already satisfy elsewhere is
/// modelled; see that method for where `mode` and `commandContextActions` live. SEAM-006.
#[derive(Default)]
pub struct BindOptions {
    /// pi's `onError` (`print-mode.ts:98-100`):
    ///
    /// ```text
    /// onError: (err) => {
    ///     console.error(`Extension error (${err.extensionPath}): ${err.error}`);
    /// },
    /// ```
    ///
    /// Registered on the session's [`cyrup_ext::ExtensionHost`] via `add_error_listener`, the same
    /// channel the RPC host already used (`cyrup-modes/src/rpc.rs`). `None` keeps the previous
    /// behaviour — a contained fault is logged and nothing else.
    pub on_error: Option<cyrup_ext::ErrorListener>,
}

/// How a session file was removed — Pi's `{ method: "trash" | "unlink" }`
/// (`modes/interactive/components/session-selector.ts:646` @v0.83.0, identical at v0.84.1). The
/// caller renders a different status line for each (`:846`), so this is on the wire of the seam
/// rather than an implementation detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteMethod {
    /// The OS `trash` CLI accepted it — the conversation is recoverable from the desktop trash.
    Trash,
    /// Permanent `unlink`, Pi's fallback when `trash` is absent or refused.
    Unlink,
}

impl DeleteMethod {
    /// Pi's own status text: `result.method === "trash" ? "Session moved to trash" : "Session
    /// deleted"` (`session-selector.ts:846`).
    pub const fn status_message(self) -> &'static str {
        match self {
            Self::Trash => "Session moved to trash",
            Self::Unlink => "Session deleted",
        }
    }
}
