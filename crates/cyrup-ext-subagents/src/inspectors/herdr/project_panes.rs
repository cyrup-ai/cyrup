//! `project.{open,status,close}` — a herdr pane per project root, and the manager behind it.
//!
//! Upstream: `src/inspectors/herdr/project-panes.ts` (730 lines @v0.68.0), ~45 % of the whole
//! `inspectors/` subtree.
//!
//! # What a project pane is, and why it is not an inspector
//!
//! An inspector pane MIRRORS one async run and is read-only. A project pane **runs its own cyrup
//! session** for a project root (`:729`), so subagents launched in it belong to that project. It
//! has its own binding (`<root>/.cyrup-subagents/project-panes/herdr.json`), its own root index so
//! a later session can find every root that has one (`herdr-roots.json`), and a trust status of
//! [`PROJECT_PANE_TRUST_STATUS`] — *"human-verification-required"* (`:17`) — because nothing
//! machine-checkable proves the pane herdr reports is the one this binding meant.
//!
//! # The `legacyToolCompatibility` DOUBLE STANDARD, ported as upstream ships it
//!
//! Upstream runs the SAME manager in two modes (`:135-138`), and collapsing them would break one
//! caller or the other:
//!
//! | | [`ToolCompatibility::Legacy`] — the model-facing verbs | [`ToolCompatibility::Strict`] — the public API |
//! |---|---|---|
//! | binding parse | required keys only (`:428`) | required keys non-empty, optionals typed (`:230-235`) |
//! | binding root ≠ asked root | tolerated (`:444`) | `INVALID_BINDING` (`:445-447`) |
//! | unreadable binding | treated as absent (`:431`) | `BINDING_READ_FAILED` (`:433-435`) |
//! | malformed binding | treated as absent (`:438`) | `INVALID_BINDING` (`:439-441`) |
//! | `INVALID_PANE_RESPONSE` from `pane get` | tolerated: state `open`, runtime `unknown` (`:492-494`, `:566-570`) | propagated (`:502`) |
//! | `open` on an unverifiable pane | reported already-open (`:564`) | `PANE_OWNERSHIP_UNVERIFIED` (`:559-563`) |
//! | split with no pane id | `PANE_GONE` (`:582`) | `INVALID_PANE_RESPONSE` (`:582`) |
//!
//! **Collapsing to strict** makes `project.open` fail on a pane herdr reports oddly — a live
//! session the user can see, refused. **Collapsing to legacy** makes the public API close someone
//! else's pane, because it would accept an unverified binding. Both modes ship, keyed by one flag,
//! exactly as upstream does.
//!
//! `close` is the ONE place both modes agree and neither yields: ownership must be `verified` and
//! the pane must be explicitly `idle` (`:644-653`), in that order. A pane running someone's live
//! session is never closed by a verb.
//!
//! # `[CYRUP-EXCEEDS-UPSTREAM]` — two corrections to pi's reading of herdr
//!
//! 1. **Ownership consults `foreground_cwd`, not only `cwd`.** `projectPaneOwnership:420-425`
//!    compares `runtime.cwd` to the project root — but herdr documents `cwd` as the
//!    *pane/workspace* cwd, used for labels and restored session state, **not the running
//!    process's** (`socket-api.mdx:750-752`). The real one is `PaneInfo::foreground_cwd`
//!    (`crates/cyrup-herdr/src/schema/panes.rs:445`), which pi already reads into
//!    `ProjectPaneRuntime.foregroundCwd` (`:346`) **and then never consults**. cyrup verifies on
//!    either: strictly fewer false `mismatch` refusals, an identical `verified` set.
//! 2. **The summary comes from `state_labels` first.** `paneSummary:323-331` probes seven keys —
//!    `summary`, `state_text`, `stateText`, `token_summary`, `tokens.summary`, `metadata.summary`,
//!    `metadata.tokens.summary`. [`cyrup_herdr::schema::PaneInfo`] has **none** of them except
//!    `tokens` (`schema/panes.rs:427-500`), so only `tokens.summary` can ever hit, and only if
//!    something called `pane.report_metadata --token summary=…`. The fields herdr really
//!    publishes are `state_labels`, `display_agent` and `label`. cyrup tries those three first and
//!    keeps all seven of pi's rungs behind them for cross-version tolerance. Without this, every
//!    project-pane roster row renders a dash forever.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cyrup_core::{Content, ToolError, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::client::{self, HerdrFailure};
use super::focus::{self, FocusFailure};
use crate::inspectors::plugins::HerdrClient;
use crate::inspectors::types::{
    CloseDisposition, HerdrErrorCode, HerdrProjectPaneSnapshot, OpenDisposition, PaneOwnership,
    ProjectPaneAction, ProjectPaneErrorCode, ProjectPaneSnapshots, ProjectPaneState,
    SchemaVersion1,
};

/// The versioned public contract (`project-panes.ts:16`).
pub const PROJECT_PANES_API_VERSION: u8 = 1;

/// `project-panes.ts:17`, verbatim. Not a placeholder: no machine check proves the pane herdr
/// reports is the one a binding meant, so the trust level says a human must confirm.
pub const PROJECT_PANE_TRUST_STATUS: &str = "human-verification-required";

// =================================================================================================
// On-disk records
// =================================================================================================

/// `kind: "herdr-project-pane"` as a one-variant enum, for the reason
/// [`crate::inspectors::types::HerdrInspectorKind`] gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HerdrProjectPaneKind {
    /// The only value.
    #[default]
    #[serde(rename = "herdr-project-pane")]
    HerdrProjectPane,
}

/// `kind: "herdr-project-pane-roots"` — the root index's own discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HerdrProjectPaneRootsKind {
    /// The only value.
    #[default]
    #[serde(rename = "herdr-project-pane-roots")]
    HerdrProjectPaneRoots,
}

/// The binding at `<root>/.cyrup-subagents/project-panes/herdr.json`
/// (`HerdrProjectPaneBinding:19-29`).
///
/// **No `deny_unknown_fields`, and `#[serde(flatten)] extra`**, for exactly the reason the
/// inspector binding carries them: `parseBinding:225-237` validates required keys only and passes
/// the rest through, `focus` REWRITES the binding (`:529-531`), and a struct that dropped unknown
/// keys would make that rewrite silent data loss on a file shared with pi.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrProjectPaneBinding {
    /// Always `1`.
    pub schema_version: SchemaVersion1,
    /// Always `"herdr-project-pane"`.
    pub kind: HerdrProjectPaneKind,
    /// The project root this pane serves.
    pub project_root: PathBuf,
    /// The herdr pane holding it.
    pub pane_id: String,
    /// ISO-8601 open time.
    pub opened_at: String,
    /// ISO-8601 last focus.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_focused_at: Option<String>,
    /// The herdr version that opened it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub herdr_version: Option<String>,
    /// The command the pane runs.
    pub command: String,
    /// The message the session was started with, when there was one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_message: Option<String>,
    /// Every key this build does not know, preserved verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The root index at `<ownerRoot>/.cyrup-subagents/project-panes/herdr-roots.json`
/// (`HerdrProjectPaneRootIndex:140-144`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrProjectPaneRootIndex {
    /// Always `1`.
    pub schema_version: SchemaVersion1,
    /// Always `"herdr-project-pane-roots"`.
    pub kind: HerdrProjectPaneRootsKind,
    /// Every root that has (or had) a project pane, sorted and deduplicated (`:203`).
    pub project_roots: Vec<PathBuf>,
}

// =================================================================================================
// Runtime + snapshot
// =================================================================================================

/// What `pane get` says about a project pane right now (`ProjectPaneRuntime:31-42`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectPaneRuntime {
    /// The pane.
    pub pane_id: String,
    /// herdr's agent label for it.
    pub agent: Option<String>,
    /// Lowercased agent status; `"unknown"` when herdr did not say (`:339`, `:344`).
    pub agent_status: String,
    /// The pane/workspace cwd herdr uses for labels.
    pub cwd: Option<String>,
    /// The FOREGROUND process's cwd — the one that actually proves ownership.
    pub foreground_cwd: Option<String>,
    /// Whether herdr reports it focused.
    pub focused: Option<bool>,
    /// Its tab.
    pub tab_id: Option<String>,
    /// Its workspace.
    pub workspace_id: Option<String>,
    /// A one-line summary for the roster.
    pub summary: Option<String>,
    /// The pane's terminal title.
    pub terminal_title: Option<String>,
}

// `HerdrProjectPaneSnapshot` and `ProjectPaneSnapshots` used to be declared here. They are SHARED
// — `tui/fleet_status.rs`'s `project_pane_entries` and `tui/fleet_state.rs`'s
// `FleetState::herdr_project_panes` both hold them, and the executor's live map
// (`SubagentExecutor::herdr_project_pane_map`) is one — so they live in
// `crate::inspectors::types` with the rest of the contract, beside `HerdrInspectorBinding`. They
// are re-exported through this module's `use` below so every call site in this file reads
// unchanged.

/// `herdrProjectPaneSnapshotFromBinding` (`:287-300`) — everything a binding alone can say.
///
/// `ownership: unknown` and `safeToClose: false` are not pessimism, they are the truth: nothing
/// has asked herdr anything yet.
#[must_use]
pub fn snapshot_from_binding(
    binding: &HerdrProjectPaneBinding,
    now_ms: i64,
) -> HerdrProjectPaneSnapshot {
    HerdrProjectPaneSnapshot {
        project_root: binding.project_root.clone(),
        binding_path: project_pane_binding_path(&binding.project_root),
        pane_id: binding.pane_id.clone(),
        opened_at: binding.opened_at.clone(),
        last_focused_at: binding.last_focused_at.clone(),
        state: ProjectPaneState::Open,
        agent_status: "unknown".to_owned(),
        ownership: PaneOwnership::Unknown,
        safe_to_close: false,
        refreshed_at: now_ms,
        summary: None,
        tab_id: None,
        workspace_id: None,
        terminal_title: None,
        stale_reason: None,
    }
}

/// `restoreHerdrProjectPaneSnapshots` (`:302-309`) — the session-start restore.
///
/// Additive by construction: it starts from what is already there and only SETS, so a root whose
/// binding has gone keeps whatever the session already knew rather than being silently dropped.
/// That is upstream's own shape (`new Map(state.herdrProjectPanes ?? [])`, `:303`).
pub fn restore_herdr_project_pane_snapshots<I, P>(
    panes: &mut ProjectPaneSnapshots,
    project_roots: I,
    now_ms: i64,
) where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    for project_root in project_roots {
        if let Some(binding) = read_herdr_project_pane_binding(project_root.as_ref()) {
            panes.insert(
                binding.project_root.clone(),
                snapshot_from_binding(&binding, now_ms),
            );
        }
    }
}

/// `extension/index.ts:864`'s closure, verbatim: how many project panes are currently OPEN.
///
/// This is the seam the herdr status bridge consumes — it folds the count into the pane label as
/// `" · N panes"` (`integrations/herdr-status.ts:162-163`). It is named here so the bridge batch
/// has one function to call rather than a filter to re-derive.
#[must_use]
pub fn open_project_pane_count(panes: &ProjectPaneSnapshots) -> usize {
    panes
        .values()
        .filter(|snapshot| snapshot.state == ProjectPaneState::Open)
        .count()
}

// =================================================================================================
// Paths and the root index
// =================================================================================================

/// `<root>/.cyrup-subagents/project-panes` (`projectPaneDir:172-174`).
#[must_use]
pub fn project_pane_dir(project_root: &Path) -> PathBuf {
    crate::artifacts::project_subagents_dir(project_root).join("project-panes")
}

/// `<root>/.cyrup-subagents/project-panes/herdr.json` (`:176-178`).
#[must_use]
pub fn project_pane_binding_path(project_root: &Path) -> PathBuf {
    project_pane_dir(project_root).join("herdr.json")
}

/// `<ownerRoot>/.cyrup-subagents/project-panes/herdr-roots.json` (`:180-182`).
#[must_use]
pub fn project_pane_root_index_path(owner_root: &Path) -> PathBuf {
    project_pane_dir(owner_root).join("herdr-roots.json")
}

/// `listHerdrProjectPaneRoots` (`:192-200`).
///
/// A missing index is an empty list; a malformed one **throws** upstream and is an `Err` here. The
/// distinction is load-bearing at the one call site that matters: `project.close` propagates the
/// failure (`:706-708`) rather than silently pruning nothing.
///
/// # Errors
/// A root index that is present but not a valid `herdr-project-pane-roots` record.
pub fn list_herdr_project_pane_roots(owner_root: &Path) -> Result<Vec<PathBuf>, String> {
    let path = project_pane_root_index_path(owner_root);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        // ENOENT / ENOTDIR (`:197`).
        Err(cause)
            if matches!(
                cause.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Vec::new());
        }
        Err(cause) => return Err(cause.to_string()),
    };
    let index: HerdrProjectPaneRootIndex = serde_json::from_slice(&bytes)
        .map_err(|_| "Invalid Herdr project pane root index.".to_owned())?;
    if index
        .project_roots
        .iter()
        .any(|root| root.as_os_str().is_empty())
    {
        return Err("Invalid Herdr project pane root index.".to_owned());
    }
    Ok(index.project_roots)
}

/// `writeHerdrProjectPaneRoot` (`:202-209`) — union, sorted.
///
/// # Errors
/// A malformed existing index, or a write that does not land.
pub async fn write_herdr_project_pane_root(
    owner_root: &Path,
    project_root: &Path,
) -> Result<(), String> {
    let mut roots: BTreeSet<PathBuf> = list_herdr_project_pane_roots(owner_root)?
        .into_iter()
        .collect();
    roots.insert(project_root.to_path_buf());
    write_root_index(owner_root, roots.into_iter().collect()).await
}

/// `removeHerdrProjectPaneRoot` (`:211-223`) — and when the last root goes, so does the file.
///
/// # Errors
/// As [`write_herdr_project_pane_root`].
pub async fn remove_herdr_project_pane_root(
    owner_root: &Path,
    project_root: &Path,
) -> Result<(), String> {
    let roots: Vec<PathBuf> = list_herdr_project_pane_roots(owner_root)?
        .into_iter()
        .filter(|root| root != project_root)
        .collect();
    let path = project_pane_root_index_path(owner_root);
    if roots.is_empty() {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(cause) => Err(cause.to_string()),
        };
    }
    write_root_index(owner_root, roots).await
}

async fn write_root_index(owner_root: &Path, project_roots: Vec<PathBuf>) -> Result<(), String> {
    let path = project_pane_root_index_path(owner_root);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|cause| cause.to_string())?;
    }
    let index = HerdrProjectPaneRootIndex {
        schema_version: SchemaVersion1,
        kind: HerdrProjectPaneRootsKind::HerdrProjectPaneRoots,
        project_roots,
    };
    crate::background::atomic::write_atomic_json(&path, &index)
        .await
        .map_err(|cause| cause.to_string())
}

// =================================================================================================
// Binding reads — the two standards
// =================================================================================================

/// Which of the two contracts a manager is running under (`:135-138`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCompatibility {
    /// The model-facing `project.*` verbs: parse loosely, tolerate a pane herdr reports oddly.
    Legacy,
    /// The public API: parse strictly, refuse unverified ownership.
    Strict,
}

impl ToolCompatibility {
    /// Upstream's own flag, which is `legacyToolCompatibility === true`.
    #[must_use]
    pub const fn is_legacy(self) -> bool {
        matches!(self, Self::Legacy)
    }
}

/// `BindingReadResult` (`:239-243`).
#[derive(Debug, Clone, PartialEq)]
enum BindingRead {
    Absent,
    Invalid,
    ReadError(String),
    Valid(Box<HerdrProjectPaneBinding>),
}

/// `parseBinding`'s strict half (`:230-235`): the four required fields non-empty once trimmed.
///
/// The optional-field type checks upstream performs are the serde types here; what serde cannot
/// express is "present but blank", which is what this adds.
fn strictly_populated(binding: &HerdrProjectPaneBinding) -> bool {
    !binding.project_root.as_os_str().is_empty()
        && !binding.pane_id.trim().is_empty()
        && !binding.opened_at.trim().is_empty()
        && !binding.command.trim().is_empty()
}

/// `readBinding` (`:245-261`).
fn read_binding(project_root: &Path, strict: bool) -> BindingRead {
    let path = project_pane_binding_path(project_root);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(cause)
            if matches!(
                cause.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return BindingRead::Absent;
        }
        Err(cause) => return BindingRead::ReadError(cause.to_string()),
    };
    match serde_json::from_slice::<HerdrProjectPaneBinding>(&bytes) {
        Ok(binding) if !strict || strictly_populated(&binding) => {
            BindingRead::Valid(Box::new(binding))
        }
        _ => BindingRead::Invalid,
    }
}

/// `readHerdrProjectPaneBinding` (`:264-267`) — the legacy model-facing reader.
#[must_use]
pub fn read_herdr_project_pane_binding(project_root: &Path) -> Option<HerdrProjectPaneBinding> {
    match read_binding(project_root, false) {
        BindingRead::Valid(binding) => Some(*binding),
        _ => None,
    }
}

/// `readProjectPaneBinding` (`:270-285`) — the strict public reader.
///
/// # Errors
/// `BINDING_READ_FAILED` for an unreadable file, `INVALID_BINDING` for a malformed one. An absent
/// binding is `Ok(None)`, not an error.
pub fn read_project_pane_binding(
    project_root: &Path,
) -> Result<Option<HerdrProjectPaneBinding>, ProjectPaneFailure> {
    match read_binding(project_root, true) {
        BindingRead::Absent => Ok(None),
        BindingRead::Valid(binding) => Ok(Some(*binding)),
        BindingRead::ReadError(cause) => Err(ProjectPaneFailure::new(
            ProjectPaneErrorCode::BindingReadFailed,
            format!(
                "Failed to read project pane binding '{}': {cause}",
                project_pane_binding_path(project_root).display()
            ),
        )),
        BindingRead::Invalid => Err(ProjectPaneFailure::new(
            ProjectPaneErrorCode::InvalidBinding,
            format!(
                "Project pane binding '{}' is malformed.",
                project_pane_binding_path(project_root).display()
            ),
        )),
    }
}

// =================================================================================================
// Errors
// =================================================================================================

/// A project-pane failure: the contract's code plus the sentence a model reads.
///
/// Same shape and same reason as [`HerdrFailure`] — see its doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPaneFailure {
    /// The contract's code.
    pub code: ProjectPaneErrorCode,
    /// The sentence.
    pub message: String,
}

impl ProjectPaneFailure {
    /// A failure with an explicit message.
    #[must_use]
    pub fn new(code: ProjectPaneErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<HerdrFailure> for ProjectPaneFailure {
    fn from(failure: HerdrFailure) -> Self {
        Self {
            code: ProjectPaneErrorCode::Herdr(failure.code),
            message: failure.message,
        }
    }
}

impl From<FocusFailure> for ProjectPaneFailure {
    fn from(failure: FocusFailure) -> Self {
        use crate::inspectors::types::HerdrFocusErrorCode;
        let code = match failure.code {
            HerdrFocusErrorCode::Herdr(code) => ProjectPaneErrorCode::Herdr(code),
            HerdrFocusErrorCode::PaneFocusUnsupported => ProjectPaneErrorCode::PaneFocusUnsupported,
            HerdrFocusErrorCode::InvalidPaneResponse => ProjectPaneErrorCode::InvalidPaneResponse,
        };
        Self {
            code,
            message: failure.message,
        }
    }
}

/// `formatProjectPaneError` (`:168-170`), verbatim.
#[must_use]
pub fn format_project_pane_error(failure: &ProjectPaneFailure) -> String {
    format!(
        "Herdr project pane error ({}): {}",
        failure.code.as_str(),
        failure.message
    )
}

// =================================================================================================
// Runtime extraction
// =================================================================================================

/// `sanitizedSummary` (`:311-315`) — control characters to spaces, runs collapsed, 120 chars.
#[must_use]
pub fn sanitized_summary(value: Option<&str>) -> Option<String> {
    let raw = value?;
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || c == '\u{7f}' {
                ' '
            } else {
                c
            }
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    Some(collapsed.chars().take(120).collect())
}

fn nested<'a>(record: &'a Map<String, Value>, key: &str) -> Option<&'a Map<String, Value>> {
    record.get(key).and_then(Value::as_object)
}

fn text_of<'a>(record: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    record.get(key).and_then(Value::as_str)
}

/// The roster's one-line summary for a pane.
///
/// **cyrup's three rungs first, then pi's seven** — see the module doc's point 2. Six of pi's
/// seven cannot hit against a herdr 0.9.1 `PaneInfo` at all (no such field); the seventh,
/// `tokens.summary`, can, but only for a pane something has reported that token on. All seven are
/// kept behind cyrup's three for cross-version tolerance.
#[must_use]
pub fn pane_summary(pane: &Map<String, Value>) -> Option<String> {
    let agent_status = text_of(pane, "agent_status")
        .or_else(|| text_of(pane, "agentStatus"))
        .unwrap_or("unknown");
    // 1-3: what herdr actually publishes (`crates/cyrup-herdr/src/schema/panes.rs:448,463,468`).
    nested(pane, "state_labels")
        .and_then(|labels| sanitized_summary(text_of(labels, agent_status)))
        .or_else(|| sanitized_summary(text_of(pane, "display_agent")))
        .or_else(|| sanitized_summary(text_of(pane, "label")))
        // 4-10: pi's own probe ladder (`:323-331`), kept verbatim for a herdr this build has not
        // seen — every rung is a key some version might carry.
        .or_else(|| sanitized_summary(text_of(pane, "summary")))
        .or_else(|| sanitized_summary(text_of(pane, "state_text")))
        .or_else(|| sanitized_summary(text_of(pane, "stateText")))
        .or_else(|| sanitized_summary(text_of(pane, "token_summary")))
        .or_else(|| {
            nested(pane, "tokens").and_then(|tokens| sanitized_summary(text_of(tokens, "summary")))
        })
        .or_else(|| {
            nested(pane, "metadata")
                .and_then(|metadata| sanitized_summary(text_of(metadata, "summary")))
        })
        .or_else(|| {
            nested(pane, "metadata")
                .and_then(|metadata| nested(metadata, "tokens"))
                .and_then(|tokens| sanitized_summary(text_of(tokens, "summary")))
        })
}

/// `projectPaneRuntime` (`:333-355`).
#[must_use]
pub fn project_pane_runtime(value: &Value) -> Option<ProjectPaneRuntime> {
    let pane = focus::herdr_pane_record(value)?;
    let target = focus::herdr_pane_focus_target(value);
    let pane_id = target.pane_id?;
    Some(ProjectPaneRuntime {
        pane_id,
        agent: text_of(pane, "agent").map(str::to_owned),
        agent_status: text_of(pane, "agent_status")
            .or_else(|| text_of(pane, "agentStatus"))
            .unwrap_or("unknown")
            .to_ascii_lowercase(),
        cwd: text_of(pane, "cwd").map(str::to_owned),
        foreground_cwd: text_of(pane, "foreground_cwd")
            .or_else(|| text_of(pane, "foregroundCwd"))
            .map(str::to_owned),
        focused: pane.get("focused").and_then(Value::as_bool),
        tab_id: target.tab_id,
        workspace_id: target.workspace_id,
        summary: pane_summary(pane),
        terminal_title: text_of(pane, "terminal_title_stripped")
            .or_else(|| text_of(pane, "terminal_title"))
            .or_else(|| text_of(pane, "terminalTitle"))
            .map(str::to_owned),
    })
}

/// `canonicalRuntimePath` (`:415-418`) — realpath, falling back to a plain resolve.
fn canonical_runtime_path(value: Option<&str>) -> Option<PathBuf> {
    let raw = value?;
    if raw.is_empty() {
        return None;
    }
    let resolved = PathBuf::from(raw);
    Some(std::fs::canonicalize(&resolved).unwrap_or(resolved))
}

/// `projectPaneOwnership` (`:420-425`), widened per the module doc's point 1.
///
/// The widening is strictly one-directional: a root that matches EITHER cwd is `Verified`, and
/// `Mismatch` requires every cwd herdr offered to disagree. Every pane pi called `verified` is
/// still `verified`; some pi called `mismatch` become `verified`, which is the correct answer for
/// a pane whose shell has `cd`-ed into a subdirectory of the project it belongs to.
#[must_use]
pub fn project_pane_ownership(
    runtime: &ProjectPaneRuntime,
    binding: &HerdrProjectPaneBinding,
    project_root: &Path,
) -> PaneOwnership {
    if runtime.pane_id != binding.pane_id {
        return PaneOwnership::Mismatch;
    }
    let candidates: Vec<PathBuf> = [
        canonical_runtime_path(runtime.cwd.as_deref()),
        canonical_runtime_path(runtime.foreground_cwd.as_deref()),
    ]
    .into_iter()
    .flatten()
    .collect();
    if candidates.is_empty() {
        return PaneOwnership::Unknown;
    }
    if candidates.iter().any(|candidate| candidate == project_root) {
        PaneOwnership::Verified
    } else {
        PaneOwnership::Mismatch
    }
}

/// `resolveProjectRoot` (`:390-399`).
fn resolve_project_root(requested: &Path) -> Result<PathBuf, ProjectPaneFailure> {
    let resolved = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(requested)
    };
    match std::fs::metadata(&resolved) {
        Ok(meta) if meta.is_dir() => {
            Ok(std::fs::canonicalize(&resolved).unwrap_or_else(|_| resolved.clone()))
        }
        Ok(_) => Err(ProjectPaneFailure::new(
            ProjectPaneErrorCode::InvalidProjectRoot,
            format!(
                "Project pane target '{}' is not a directory.",
                resolved.display()
            ),
        )),
        Err(cause) => Err(ProjectPaneFailure::new(
            ProjectPaneErrorCode::InvalidProjectRoot,
            format!(
                "Project pane target '{}' is unavailable: {cause}",
                resolved.display()
            ),
        )),
    }
}

/// `projectPaneCommand` (`:409-413`).
///
/// Upstream's `getPiSpawnCommand(args)` is cyrup's [`crate::spawn::resolve_spawn_command`]
/// (`spawn/mod.rs:280`), which answers a binary and the argv that must precede every per-run
/// argument. The pane runs a full cyrup session in the project root, optionally seeded with a
/// first message.
fn project_pane_command(message: Option<&str>) -> String {
    let spawn = crate::spawn::resolve_spawn_command();
    let exe = spawn.binary.to_string_lossy().into_owned();
    let mut args = spawn.base_args.clone();
    if let Some(message) = message.map(str::trim).filter(|m| !m.is_empty()) {
        args.push(message.to_owned());
    }
    crate::inspectors::shell_command::format_shell_command(
        &exe,
        &args,
        crate::inspectors::shell_command::host_platform(),
    )
}

// =================================================================================================
// The manager
// =================================================================================================

/// Everything a `status` look found (`ProjectPaneStatusData:80-87`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectPaneStatusData {
    /// The resolved root.
    pub project_root: PathBuf,
    /// Its binding path.
    pub binding_path: PathBuf,
    /// The API version this record answers under.
    pub api_version: u8,
    /// [`PROJECT_PANE_TRUST_STATUS`].
    pub trust: &'static str,
    /// Absent, open or stale.
    pub state: ProjectPaneState,
    /// The binding, when there is one.
    pub binding: Option<HerdrProjectPaneBinding>,
    /// What herdr says right now.
    pub runtime: Option<ProjectPaneRuntime>,
    /// Whether the pane is provably this binding's.
    pub ownership: PaneOwnership,
    /// `agentStatus == "idle" && ownership == verified` (`:507`).
    pub safe_to_close: bool,
    /// Why the pane is stale.
    pub stale_reason: Option<HerdrFailure>,
}

/// What `open` did (`OpenProjectPaneData:74-78`).
#[derive(Debug, Clone, PartialEq)]
pub struct OpenProjectPaneData {
    /// The resolved root.
    pub project_root: PathBuf,
    /// Its binding path.
    pub binding_path: PathBuf,
    /// Opened, or already open.
    pub disposition: OpenDisposition,
    /// The binding, new or existing.
    pub binding: HerdrProjectPaneBinding,
    /// What herdr said, when it was asked.
    pub runtime: Option<ProjectPaneRuntime>,
}

/// What `close` did (`CloseProjectPaneData:89-93`).
#[derive(Debug, Clone, PartialEq)]
pub struct CloseProjectPaneData {
    /// The resolved root.
    pub project_root: PathBuf,
    /// Its binding path.
    pub binding_path: PathBuf,
    /// Closed, absent, or a stale binding removed.
    pub disposition: CloseDisposition,
    /// The binding that was there.
    pub binding: Option<HerdrProjectPaneBinding>,
    /// What herdr said.
    pub runtime: Option<ProjectPaneRuntime>,
}

/// What `focus` did (`FocusProjectPaneData:95-100`).
#[derive(Debug, Clone, PartialEq)]
pub struct FocusProjectPaneData {
    /// The resolved root.
    pub project_root: PathBuf,
    /// The rewritten binding, carrying the new `lastFocusedAt`.
    pub binding: HerdrProjectPaneBinding,
    /// What herdr said.
    pub runtime: ProjectPaneRuntime,
    /// The pane that took focus.
    pub focused: focus::FocusedPane,
}

/// `ProjectPaneManager` (`:128-133`, built by `createProjectPaneManagerInternal:479`).
///
/// **FOUR methods, not three.** `focus` (`:131`, body `:511-544`) is reached by `project.open`
/// when the pane is already open and `params.focus` is set (`:720-726`); without it,
/// `project.open --focus` on an open pane silently does nothing.
pub struct ProjectPaneManager<'a> {
    client: &'a dyn HerdrClient,
    compatibility: ToolCompatibility,
}

impl<'a> ProjectPaneManager<'a> {
    /// A manager over `client` running under `compatibility`.
    #[must_use]
    pub fn new(client: &'a dyn HerdrClient, compatibility: ToolCompatibility) -> Self {
        Self {
            client,
            compatibility,
        }
    }

    /// `bindingForManager` (`:427-450`) — the double standard, in one function.
    fn binding_for_manager(
        &self,
        project_root: &Path,
    ) -> Result<Option<HerdrProjectPaneBinding>, ProjectPaneFailure> {
        let legacy = self.compatibility.is_legacy();
        let path = project_pane_binding_path(project_root);
        match read_binding(project_root, !legacy) {
            BindingRead::Absent => Ok(None),
            BindingRead::ReadError(cause) => {
                if legacy {
                    return Ok(None);
                }
                Err(ProjectPaneFailure::new(
                    ProjectPaneErrorCode::BindingReadFailed,
                    format!(
                        "Failed to read project pane binding '{}': {cause}",
                        path.display()
                    ),
                ))
            }
            BindingRead::Invalid => {
                if legacy {
                    return Ok(None);
                }
                Err(ProjectPaneFailure::new(
                    ProjectPaneErrorCode::InvalidBinding,
                    format!("Project pane binding '{}' is malformed.", path.display()),
                ))
            }
            BindingRead::Valid(binding) => {
                if !legacy
                    && canonical_runtime_path(binding.project_root.to_str()).as_deref()
                        != Some(project_root)
                {
                    return Err(ProjectPaneFailure::new(
                        ProjectPaneErrorCode::InvalidBinding,
                        format!(
                            "Project pane binding root '{}' does not match '{}'.",
                            binding.project_root.display(),
                            project_root.display()
                        ),
                    ));
                }
                Ok(Some(*binding))
            }
        }
    }

    /// `inspectPane` (`:401-407`).
    async fn inspect_pane(&self, pane_id: &str) -> Result<ProjectPaneRuntime, ProjectPaneFailure> {
        let live = client::call(self.client, &["pane", "get", pane_id]).await?;
        project_pane_runtime(&live).ok_or_else(|| {
            ProjectPaneFailure::new(
                ProjectPaneErrorCode::InvalidPaneResponse,
                format!("Herdr pane get returned no pane runtime for '{pane_id}'."),
            )
        })
    }

    /// `status` (`:482-509`).
    ///
    /// # Errors
    /// An unresolvable root, a binding this mode refuses, or a `pane get` failure this mode does
    /// not tolerate.
    pub async fn status(&self, cwd: &Path) -> Result<ProjectPaneStatusData, ProjectPaneFailure> {
        let project_root = resolve_project_root(cwd)?;
        let binding_path = project_pane_binding_path(&project_root);
        let blank = ProjectPaneStatusData {
            project_root: project_root.clone(),
            binding_path: binding_path.clone(),
            api_version: PROJECT_PANES_API_VERSION,
            trust: PROJECT_PANE_TRUST_STATUS,
            state: ProjectPaneState::Absent,
            binding: None,
            runtime: None,
            // Nothing has asked herdr anything yet, and `safeToClose: true` on an ABSENT pane is
            // upstream's own answer (`:489`) — there is nothing there to be unsafe about.
            ownership: PaneOwnership::Unknown,
            safe_to_close: true,
            stale_reason: None,
        };
        let Some(existing) = self.binding_for_manager(&project_root)? else {
            return Ok(blank);
        };
        match self.inspect_pane(&existing.pane_id).await {
            Ok(runtime) => {
                let ownership = project_pane_ownership(&runtime, &existing, &project_root);
                let safe = runtime.agent_status == "idle" && ownership == PaneOwnership::Verified;
                Ok(ProjectPaneStatusData {
                    state: ProjectPaneState::Open,
                    binding: Some(existing),
                    runtime: Some(runtime),
                    ownership,
                    safe_to_close: safe,
                    ..blank
                })
            }
            // The legacy tolerance (`:492-494`): herdr answered, the shape was odd, and the pane
            // is demonstrably there. A model-facing verb reports it open with an unknown runtime
            // rather than failing on a presentation detail.
            Err(failure)
                if failure.code == ProjectPaneErrorCode::InvalidPaneResponse
                    && self.compatibility.is_legacy() =>
            {
                let runtime = ProjectPaneRuntime {
                    pane_id: existing.pane_id.clone(),
                    agent_status: "unknown".to_owned(),
                    ..ProjectPaneRuntime::default()
                };
                Ok(ProjectPaneStatusData {
                    state: ProjectPaneState::Open,
                    binding: Some(existing),
                    runtime: Some(runtime),
                    ownership: PaneOwnership::Unknown,
                    safe_to_close: false,
                    ..blank
                })
            }
            Err(failure) if is_gone(&failure) => Ok(ProjectPaneStatusData {
                state: ProjectPaneState::Stale,
                binding: Some(existing),
                runtime: None,
                ownership: PaneOwnership::Unknown,
                safe_to_close: false,
                stale_reason: Some(HerdrFailure::new(gone_code(&failure), failure.message)),
                ..blank
            }),
            Err(failure) => Err(failure),
        }
    }

    /// `focus` (`:511-544`) — status, then ONE `pane focus`, then rewrite `lastFocusedAt`.
    ///
    /// # Errors
    /// `PANE_GONE` when there is nothing open, `PANE_OWNERSHIP_UNVERIFIED` when the pane is not
    /// provably this binding's — focus is a user-visible jump, so it never lands on a pane the
    /// manager cannot vouch for — plus the focus failure itself and `BINDING_WRITE_FAILED`.
    pub async fn focus(&self, cwd: &Path) -> Result<FocusProjectPaneData, ProjectPaneFailure> {
        let status = self.status(cwd).await?;
        let (Some(binding), Some(runtime)) = (status.binding.clone(), status.runtime.clone())
        else {
            return Err(ProjectPaneFailure::new(
                ProjectPaneErrorCode::Herdr(HerdrErrorCode::PaneGone),
                format!(
                    "No open Herdr project pane binding exists for '{}'.",
                    status.project_root.display()
                ),
            ));
        };
        if status.state != ProjectPaneState::Open {
            return Err(ProjectPaneFailure::new(
                ProjectPaneErrorCode::Herdr(HerdrErrorCode::PaneGone),
                format!(
                    "No open Herdr project pane binding exists for '{}'.",
                    status.project_root.display()
                ),
            ));
        }
        if status.ownership != PaneOwnership::Verified {
            return Err(unverified(
                &binding.pane_id,
                status.ownership,
                &status.project_root,
            ));
        }
        let focused = focus::focus_herdr_pane(self.client, &binding.pane_id).await?;
        let refreshed = HerdrProjectPaneBinding {
            last_focused_at: Some(crate::time::format_iso8601_millis(
                crate::time::now_epoch_millis(),
            )),
            ..binding
        };
        write_project_pane_binding(&status.binding_path, &refreshed)
            .await
            .map_err(|cause| {
                ProjectPaneFailure::new(
                    ProjectPaneErrorCode::BindingWriteFailed,
                    format!(
                        "Failed to update project pane binding '{}': {cause}",
                        status.binding_path.display()
                    ),
                )
            })?;
        Ok(FocusProjectPaneData {
            project_root: status.project_root,
            binding: refreshed,
            runtime,
            focused,
        })
    }

    /// `open` (`:546-623`).
    ///
    /// # Errors
    /// The version gate, a binding this mode refuses, ownership this mode will not accept, the
    /// split, the launch, or a binding write that does not land — and that last one CLOSES the
    /// pane it just opened, because a pane with no binding is invisible to every later verb.
    pub async fn open(
        &self,
        cwd: &Path,
        message: Option<&str>,
        focus_requested: bool,
    ) -> Result<OpenProjectPaneData, ProjectPaneFailure> {
        let project_root = resolve_project_root(cwd)?;
        let binding_path = project_pane_binding_path(&project_root);
        let detected = client::detect_herdr(self.client).await?;

        if let Some(existing) = self.binding_for_manager(&project_root)? {
            match self.inspect_pane(&existing.pane_id).await {
                Ok(runtime) => {
                    let ownership = project_pane_ownership(&runtime, &existing, &project_root);
                    if !self.compatibility.is_legacy() && ownership != PaneOwnership::Verified {
                        return Err(unverified(&existing.pane_id, ownership, &project_root));
                    }
                    return Ok(OpenProjectPaneData {
                        project_root,
                        binding_path,
                        disposition: OpenDisposition::AlreadyOpen,
                        binding: existing,
                        runtime: Some(runtime),
                    });
                }
                Err(failure)
                    if failure.code == ProjectPaneErrorCode::InvalidPaneResponse
                        && self.compatibility.is_legacy() =>
                {
                    let pane_id = existing.pane_id.clone();
                    return Ok(OpenProjectPaneData {
                        project_root,
                        binding_path,
                        disposition: OpenDisposition::AlreadyOpen,
                        binding: existing,
                        runtime: Some(ProjectPaneRuntime {
                            pane_id,
                            agent_status: "unknown".to_owned(),
                            ..ProjectPaneRuntime::default()
                        }),
                    });
                }
                // A pane herdr no longer has is stale, and a stale binding is replaced by opening
                // a new pane — in BOTH modes (`:572-575`).
                Err(failure) if is_gone(&failure) => {}
                Err(failure) => {
                    if !self.compatibility.is_legacy() {
                        return Err(failure);
                    }
                }
            }
        }

        let root = project_root.to_string_lossy().into_owned();
        let split = [
            "pane".to_owned(),
            "split".to_owned(),
            "--current".to_owned(),
            "--direction".to_owned(),
            "right".to_owned(),
            "--cwd".to_owned(),
            root,
            if focus_requested {
                "--focus".to_owned()
            } else {
                "--no-focus".to_owned()
            },
        ];
        let split_argv: Vec<&str> = split.iter().map(String::as_str).collect();
        let split_result = client::call(self.client, &split_argv).await?;
        let Some(pane_id) = focus::pane_id_of(&split_result) else {
            // The one place the two modes disagree on the CODE for the same fact (`:582`).
            return Err(ProjectPaneFailure::new(
                if self.compatibility.is_legacy() {
                    ProjectPaneErrorCode::Herdr(HerdrErrorCode::PaneGone)
                } else {
                    ProjectPaneErrorCode::InvalidPaneResponse
                },
                "Herdr pane split returned no pane id.",
            ));
        };

        let startup_message = message.map(str::trim).filter(|m| !m.is_empty());
        let command = project_pane_command(startup_message);
        if let Err(failure) = client::call(self.client, &["pane", "run", &pane_id, &command]).await
        {
            let _ = client::call(self.client, &["pane", "close", &pane_id]).await;
            return Err(failure.into());
        }

        let now = crate::time::format_iso8601_millis(crate::time::now_epoch_millis());
        let binding = HerdrProjectPaneBinding {
            schema_version: SchemaVersion1,
            kind: HerdrProjectPaneKind::HerdrProjectPane,
            project_root: project_root.clone(),
            pane_id: pane_id.clone(),
            opened_at: now.clone(),
            last_focused_at: focus_requested.then_some(now),
            herdr_version: Some(detected.version_text),
            command,
            startup_message: startup_message.map(str::to_owned),
            extra: Map::new(),
        };
        if let Err(cause) = write_project_pane_binding(&binding_path, &binding).await {
            let cleanup = match client::call(self.client, &["pane", "close", &pane_id]).await {
                Ok(_) => format!(" The newly opened pane '{pane_id}' was closed."),
                Err(_) => format!(" Cleanup could not close the newly opened pane '{pane_id}'."),
            };
            return Err(ProjectPaneFailure::new(
                ProjectPaneErrorCode::BindingWriteFailed,
                format!(
                    "Failed to persist project pane binding '{}': {cause}.{cleanup}",
                    binding_path.display()
                ),
            ));
        }
        Ok(OpenProjectPaneData {
            project_root,
            binding_path,
            disposition: OpenDisposition::Opened,
            binding,
            runtime: None,
        })
    }

    /// `close` (`:625-662`).
    ///
    /// # Errors
    /// `PANE_OWNERSHIP_UNVERIFIED` then `PANE_NOT_IDLE`, **in that order and in both modes** —
    /// this is the one place the double standard does not apply, because the cost of getting it
    /// wrong is closing a pane running someone's live session.
    pub async fn close(&self, cwd: &Path) -> Result<CloseProjectPaneData, ProjectPaneFailure> {
        let project_root = resolve_project_root(cwd)?;
        let binding_path = project_pane_binding_path(&project_root);
        let Some(existing) = self.binding_for_manager(&project_root)? else {
            return Ok(CloseProjectPaneData {
                project_root,
                binding_path,
                disposition: CloseDisposition::Absent,
                binding: None,
                runtime: None,
            });
        };
        let runtime = match self.inspect_pane(&existing.pane_id).await {
            Ok(runtime) => runtime,
            Err(failure) if is_gone(&failure) => {
                remove_project_pane_binding(&binding_path)?;
                return Ok(CloseProjectPaneData {
                    project_root,
                    binding_path,
                    disposition: CloseDisposition::StaleBindingRemoved,
                    binding: Some(existing),
                    runtime: None,
                });
            }
            Err(failure) => return Err(failure),
        };
        let ownership = project_pane_ownership(&runtime, &existing, &project_root);
        if ownership != PaneOwnership::Verified {
            return Err(unverified(&existing.pane_id, ownership, &project_root));
        }
        if runtime.agent_status != "idle" {
            return Err(ProjectPaneFailure::new(
                ProjectPaneErrorCode::PaneNotIdle,
                format!(
                    "Project pane '{}' is '{}', not explicitly idle.",
                    existing.pane_id, runtime.agent_status
                ),
            ));
        }
        let closed = client::call(self.client, &["pane", "close", &existing.pane_id]).await;
        let disposition = match &closed {
            Ok(_) => CloseDisposition::Closed,
            Err(failure)
                if failure.code == HerdrErrorCode::NotFound
                    || failure.code == HerdrErrorCode::PaneGone =>
            {
                CloseDisposition::StaleBindingRemoved
            }
            Err(failure) => return Err(failure.clone().into()),
        };
        remove_project_pane_binding(&binding_path)?;
        Ok(CloseProjectPaneData {
            project_root,
            binding_path,
            disposition,
            binding: Some(existing),
            runtime: Some(runtime),
        })
    }
}

/// `PANE_OWNERSHIP_UNVERIFIED`'s sentence (`:520`, `:560`, `:645`), verbatim.
fn unverified(pane_id: &str, ownership: PaneOwnership, project_root: &Path) -> ProjectPaneFailure {
    let word = match ownership {
        PaneOwnership::Verified => "verified",
        PaneOwnership::Unknown => "unknown",
        PaneOwnership::Mismatch => "mismatch",
    };
    ProjectPaneFailure::new(
        ProjectPaneErrorCode::PaneOwnershipUnverified,
        format!(
            "Project pane '{pane_id}' ownership is '{word}' for '{}'.",
            project_root.display()
        ),
    )
}

/// `NOT_FOUND`/`PANE_GONE` — the two codes that mean "herdr does not have that pane".
fn is_gone(failure: &ProjectPaneFailure) -> bool {
    matches!(
        failure.code,
        ProjectPaneErrorCode::Herdr(HerdrErrorCode::NotFound)
            | ProjectPaneErrorCode::Herdr(HerdrErrorCode::PaneGone)
    )
}

fn gone_code(failure: &ProjectPaneFailure) -> HerdrErrorCode {
    match failure.code {
        ProjectPaneErrorCode::Herdr(code) => code,
        _ => HerdrErrorCode::PaneGone,
    }
}

async fn write_project_pane_binding(
    path: &Path,
    binding: &HerdrProjectPaneBinding,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|cause| cause.to_string())?;
    }
    crate::background::atomic::write_atomic_json(path, binding)
        .await
        .map_err(|cause| cause.to_string())
}

/// `removeProjectPaneBinding` (`:467-477`).
fn remove_project_pane_binding(path: &Path) -> Result<(), ProjectPaneFailure> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(cause) => Err(ProjectPaneFailure::new(
            ProjectPaneErrorCode::BindingRemoveFailed,
            format!(
                "Failed to remove project pane binding '{}': {cause}",
                path.display()
            ),
        )),
    }
}

// =================================================================================================
// The model-facing verbs
// =================================================================================================

/// The three parameters `project.*` accepts (`ProjectPaneParams:146-150`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectPaneParams {
    /// The project root; `deps.cwd` when absent or blank (`:687`).
    pub cwd: Option<PathBuf>,
    /// A first message for the new session.
    pub message: Option<String>,
    /// Focus the pane.
    pub focus: Option<bool>,
}

/// What the dispatcher hands the verbs (`ProjectPaneDeps:152-158`).
pub struct ProjectPaneDeps<'a> {
    /// The session's own cwd — the OWNER root, whose index tracks every project that has a pane.
    pub cwd: PathBuf,
    /// The herdr seam.
    pub client: &'a dyn HerdrClient,
    /// The session's LIVE project-pane map, when the host keeps one — pi's `deps.state`
    /// (`extension/index.ts:864` threads `state.herdrProjectPanes` into this handler and reads it
    /// back live through `getProjectPaneCount`).
    ///
    /// A `&Mutex<..>` rather than a `&mut ..` because the handler `await`s between reading a
    /// status and remembering it, and a `&mut` borrow of the executor's map cannot cross those
    /// `await`s. Every write through it is one short synchronous lock inside [`remember`], never
    /// held across an `await`.
    ///
    /// `None` is "do not remember" — the shape a unit test that is not about the map uses. It is
    /// NOT what production passes: this map has two readers that would otherwise freeze at
    /// whatever `SessionStart` found (the roster's project-pane section through
    /// `FleetState::herdr_project_panes`, and the herdr pane label's `" · N panes"` suffix through
    /// `open_herdr_project_pane_count`).
    pub panes: Option<&'a std::sync::Mutex<ProjectPaneSnapshots>>,
}

fn ok(text: impl Into<String>) -> Result<ToolResult, ToolError> {
    Ok(ToolResult {
        content: vec![Content::text(text.into())],
        ..Default::default()
    })
}

fn err(text: impl Into<String>) -> Result<ToolResult, ToolError> {
    Err(ToolError::new(text.into()))
}

/// `herdrProjectPaneSnapshotFromStatus` (`:357-376`) folded into `rememberProjectPane` (`:378-384`).
fn remember(
    panes: Option<&std::sync::Mutex<ProjectPaneSnapshots>>,
    data: &ProjectPaneStatusData,
    now_ms: i64,
) {
    let Some(panes) = panes else { return };
    let mut panes = panes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (Some(binding), false) = (
        data.binding.as_ref(),
        data.state == ProjectPaneState::Absent,
    ) else {
        panes.remove(&data.project_root);
        return;
    };
    panes.insert(
        data.project_root.clone(),
        HerdrProjectPaneSnapshot {
            project_root: data.project_root.clone(),
            binding_path: data.binding_path.clone(),
            pane_id: binding.pane_id.clone(),
            opened_at: binding.opened_at.clone(),
            last_focused_at: binding.last_focused_at.clone(),
            state: data.state,
            agent_status: data
                .runtime
                .as_ref()
                .map_or_else(|| "unknown".to_owned(), |r| r.agent_status.clone()),
            ownership: data.ownership,
            safe_to_close: data.safe_to_close,
            refreshed_at: now_ms,
            summary: data.runtime.as_ref().and_then(|r| r.summary.clone()),
            tab_id: data.runtime.as_ref().and_then(|r| r.tab_id.clone()),
            workspace_id: data.runtime.as_ref().and_then(|r| r.workspace_id.clone()),
            terminal_title: data.runtime.as_ref().and_then(|r| r.terminal_title.clone()),
            stale_reason: data.stale_reason.as_ref().map(|f| f.message.clone()),
        },
    );
}

/// `handleHerdrProjectPaneAction` (`:686-730`).
///
/// **Runs the manager in [`ToolCompatibility::Legacy`]** (`:689`), which is the whole point of the
/// double standard: a model-facing verb must answer about a pane herdr reports oddly rather than
/// refusing it.
///
/// # Errors
/// Every failure, as `Herdr project pane error ({code}): {message}`. Note what is NOT an error:
/// `project.status` and `project.close` with no binding both answer
/// *"No Herdr project pane binding exists for {root}."* as a plain `Ok`, with **no herdr call at
/// all** — neither verb consults `detectHerdr`, so both are answerable on a box with no herdr
/// installed.
pub async fn handle_herdr_project_pane_action(
    action: ProjectPaneAction,
    params: &ProjectPaneParams,
    deps: ProjectPaneDeps<'_>,
) -> Result<ToolResult, ToolError> {
    let requested = params
        .cwd
        .as_deref()
        .filter(|cwd| !cwd.as_os_str().is_empty())
        .unwrap_or(deps.cwd.as_path())
        .to_path_buf();
    let owner_root = std::fs::canonicalize(&deps.cwd).unwrap_or_else(|_| deps.cwd.clone());
    let manager = ProjectPaneManager::new(deps.client, ToolCompatibility::Legacy);
    let now_ms = crate::time::now_epoch_millis();
    let panes = deps.panes;

    match action {
        ProjectPaneAction::Status => {
            let status = match manager.status(&requested).await {
                Ok(status) => status,
                Err(failure) => return err(format_project_pane_error(&failure)),
            };
            remember(panes, &status, now_ms);
            match status.state {
                ProjectPaneState::Absent => ok(format!(
                    "No Herdr project pane binding exists for {}.",
                    status.project_root.display()
                )),
                ProjectPaneState::Stale => {
                    let reason = status.stale_reason.unwrap_or_else(|| {
                        HerdrFailure::new(HerdrErrorCode::PaneGone, "The pane is gone.")
                    });
                    err(format!(
                        "{}\nBinding: {}",
                        format_project_pane_error(&ProjectPaneFailure::from(reason)),
                        status.binding_path.display()
                    ))
                }
                ProjectPaneState::Open => {
                    let pane_id = status
                        .binding
                        .as_ref()
                        .map_or_else(String::new, |b| b.pane_id.clone());
                    ok(format!(
                        "Herdr project pane {pane_id} is open for {}.\nBinding: {}",
                        status.project_root.display(),
                        status.binding_path.display()
                    ))
                }
            }
        }

        ProjectPaneAction::Close => {
            let closed = match manager.close(&requested).await {
                Ok(closed) => closed,
                Err(failure) => return err(format_project_pane_error(&failure)),
            };
            if let Some(panes) = panes {
                panes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&closed.project_root);
            }
            // `:706` runs BEFORE the absent check at `:709`: a faithful port prunes the root index
            // even on the absent path, or the session-start restore re-reads dead roots forever.
            if let Err(cause) =
                remove_herdr_project_pane_root(&owner_root, &closed.project_root).await
            {
                return err(format!(
                    "Herdr project pane error (BINDING_REMOVE_FAILED): Failed to remove project \
                     pane root index '{}': {cause}",
                    project_pane_root_index_path(&owner_root).display()
                ));
            }
            if closed.disposition == CloseDisposition::Absent {
                return ok(format!(
                    "No Herdr project pane binding exists for {}.",
                    closed.project_root.display()
                ));
            }
            let pane_id = closed
                .binding
                .as_ref()
                .map_or_else(String::new, |b| b.pane_id.clone());
            ok(format!(
                "Closed Herdr project pane {pane_id} for {}.",
                closed.project_root.display()
            ))
        }

        ProjectPaneAction::Open => {
            let opened = match manager
                .open(
                    &requested,
                    params.message.as_deref(),
                    params.focus == Some(true),
                )
                .await
            {
                Ok(opened) => opened,
                Err(failure) => return err(format_project_pane_error(&failure)),
            };
            if let Err(cause) =
                write_herdr_project_pane_root(&owner_root, &opened.project_root).await
            {
                return err(format!(
                    "Herdr project pane error (BINDING_WRITE_FAILED): Failed to persist project \
                     pane root index '{}': {cause}",
                    project_pane_root_index_path(&owner_root).display()
                ));
            }
            if let Ok(status) = manager.status(&opened.project_root).await {
                remember(panes, &status, now_ms);
            }
            let pane_id = opened.binding.pane_id.clone();
            let root = opened.project_root.display().to_string();
            if opened.disposition == OpenDisposition::AlreadyOpen {
                if params.focus != Some(true) {
                    return ok(format!(
                        "Herdr project pane {pane_id} is already open for {root}."
                    ));
                }
                return match manager.focus(&opened.project_root).await {
                    Ok(focused) => {
                        if let Ok(status) = manager.status(&opened.project_root).await {
                            remember(panes, &status, now_ms);
                        }
                        let where_to = focused.focused.tab_id.as_ref().map_or_else(
                            || {
                                focused.focused.workspace_id.as_ref().map_or_else(
                                    || format!("pane {}", focused.focused.pane_id),
                                    |workspace| format!("workspace {workspace}"),
                                )
                            },
                            |tab| format!("tab {tab}"),
                        );
                        ok(format!(
                            "Herdr project pane {pane_id} is already open for {root}. Focused \
                             {where_to}."
                        ))
                    }
                    Err(failure) => err(format!(
                        "Herdr project pane {pane_id} is already open for {root}. {}",
                        format_project_pane_error(&failure)
                    )),
                };
            }
            ok(format!(
                "Opened Herdr project pane {pane_id} for {root}. The pane runs its own Pi \
                 session; subagents launched there belong to that project."
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::sync::Mutex;

    use serde_json::json;

    use super::*;

    #[derive(Default)]
    struct FakeHerdrClient {
        calls: Mutex<Vec<Vec<String>>>,
        script: Mutex<Vec<Result<Value, HerdrErrorCode>>>,
    }

    impl FakeHerdrClient {
        fn scripted(script: Vec<Result<Value, HerdrErrorCode>>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                script: Mutex::new(script),
            }
        }

        fn always(answer: Result<Value, HerdrErrorCode>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                script: Mutex::new(vec![answer; 32]),
            }
        }

        fn verbs(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|call| call.join(" "))
                .collect()
        }

        fn is_untouched(&self) -> bool {
            self.calls.lock().unwrap().is_empty()
        }
    }

    #[async_trait::async_trait]
    impl HerdrClient for FakeHerdrClient {
        async fn run(&self, args: &[&str]) -> Result<Value, HerdrErrorCode> {
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|a| (*a).to_owned()).collect());
            let mut script = self.script.lock().unwrap();
            if script.is_empty() {
                return Ok(json!({}));
            }
            script.remove(0)
        }
    }

    fn pane_envelope(pane_id: &str, cwd: &str, agent_status: &str) -> Value {
        json!({ "pane": {
            "pane_id": pane_id,
            "terminal_id": "t-1",
            "workspace_id": "w1",
            "tab_id": "w1:t1",
            "focused": false,
            "cwd": cwd,
            "agent_status": agent_status,
            "revision": 3,
        }})
    }

    fn text_of(result: &Result<ToolResult, ToolError>) -> String {
        match result {
            Ok(value) => value
                .content
                .iter()
                .map(|content| match content {
                    Content::Text { text, .. } => text.to_string(),
                    other => format!("{other:?}"),
                })
                .collect::<Vec<_>>()
                .join(""),
            Err(error) => error.to_string(),
        }
    }

    /// **T-PROJ-2.** GUT either verb into calling `detect_herdr` first — the symmetric-looking
    /// mistake, since `open` does — and both rows fail. This is the whole not-installed contract
    /// for `project.*`, and it is what makes two of the three verbs work on this container.
    #[tokio::test]
    async fn project_status_and_close_answer_with_no_herdr_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();

        for action in [ProjectPaneAction::Status, ProjectPaneAction::Close] {
            let herdr = FakeHerdrClient::always(Err(HerdrErrorCode::Unavailable));
            let reply = handle_herdr_project_pane_action(
                action,
                &ProjectPaneParams::default(),
                ProjectPaneDeps {
                    cwd: root.clone(),
                    client: &herdr,
                    panes: None,
                },
            )
            .await;
            assert!(reply.is_ok(), "{} must not be an error", action.as_str());
            assert_eq!(
                text_of(&reply),
                format!(
                    "No Herdr project pane binding exists for {}.",
                    root.display()
                )
            );
            assert!(
                herdr.is_untouched(),
                "{} must not invoke the herdr seam at all",
                action.as_str()
            );
        }
    }

    /// **T-PROJ-3.** GUT the `detect_herdr` call out of `open` and a box with no herdr silently
    /// tries to split a pane instead of telling the user to install it.
    #[tokio::test]
    async fn project_open_with_no_herdr_carries_upstreams_install_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let herdr = FakeHerdrClient::always(Err(HerdrErrorCode::Unavailable));
        let reply = handle_herdr_project_pane_action(
            ProjectPaneAction::Open,
            &ProjectPaneParams::default(),
            ProjectPaneDeps {
                cwd: dir.path().to_path_buf(),
                client: &herdr,
                panes: None,
            },
        )
        .await;
        assert_eq!(
            text_of(&reply),
            "Herdr project pane error (HERDR_UNAVAILABLE): Herdr is not installed or is not on \
             PATH. Install Herdr 0.7.5+ or set HERDR_BIN."
        );
        assert!(reply.is_err());
        // The SENTENCE alone does not prove the detect: an unavailable seam answers every call
        // with `HERDR_UNAVAILABLE`, so deleting `detect_herdr` from `open` leaves the same text
        // coming back off the first `pane get` instead. What the detect buys is that the refusal
        // is upstream's FIRST act — instant, and before anything touches the binding file or
        // tries to split. That is what this pins.
        let argv = herdr.verbs();
        assert_eq!(
            argv.first().map(String::as_str),
            Some("--version"),
            "`--version` is the first thing `project.open` does; saw {argv:?}"
        );
    }

    /// **T-PROJ-1.** The full round trip, and the idle gate. GUT the `agent_status != "idle"`
    /// check and `project.close` closes a pane running someone's live session; GUT the root-index
    /// write and session-start restore never finds this project again.
    #[tokio::test]
    async fn project_open_status_close_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let panes = std::sync::Mutex::new(ProjectPaneSnapshots::new());

        // ---- open ----
        let herdr = FakeHerdrClient::scripted(vec![
            Ok(Value::String("0.9.1".into())), // --version
            Ok(pane_envelope("w1:p5", &root.to_string_lossy(), "idle")), // pane split
            Ok(json!({})),                     // pane run
            Ok(pane_envelope("w1:p5", &root.to_string_lossy(), "idle")), // status re-look
        ]);
        let reply = handle_herdr_project_pane_action(
            ProjectPaneAction::Open,
            &ProjectPaneParams::default(),
            ProjectPaneDeps {
                cwd: root.clone(),
                client: &herdr,
                panes: Some(&panes),
            },
        )
        .await;
        assert_eq!(
            text_of(&reply),
            format!(
                "Opened Herdr project pane w1:p5 for {}. The pane runs its own Pi session; \
                 subagents launched there belong to that project.",
                root.display()
            )
        );
        assert!(herdr.verbs().iter().any(|verb| verb.contains(&format!(
            "pane split --current --direction right --cwd {} --no-focus",
            root.display()
        ))));
        assert!(project_pane_binding_path(&root).exists());
        assert_eq!(
            list_herdr_project_pane_roots(&root).unwrap(),
            vec![root.clone()]
        );
        assert_eq!(open_project_pane_count(&panes.lock().unwrap()), 1);
        assert_eq!(
            panes.lock().unwrap().get(&root).unwrap().ownership,
            PaneOwnership::Verified
        );
        assert!(panes.lock().unwrap().get(&root).unwrap().safe_to_close);

        // ---- status ----
        let herdr = FakeHerdrClient::scripted(vec![Ok(pane_envelope(
            "w1:p5",
            &root.to_string_lossy(),
            "working",
        ))]);
        let reply = handle_herdr_project_pane_action(
            ProjectPaneAction::Status,
            &ProjectPaneParams::default(),
            ProjectPaneDeps {
                cwd: root.clone(),
                client: &herdr,
                panes: Some(&panes),
            },
        )
        .await;
        assert_eq!(
            text_of(&reply),
            format!(
                "Herdr project pane w1:p5 is open for {}.\nBinding: {}",
                root.display(),
                project_pane_binding_path(&root).display()
            )
        );
        assert!(!panes.lock().unwrap().get(&root).unwrap().safe_to_close);

        // ---- close, refused while not idle (V16's literal, wrapped) ----
        let herdr = FakeHerdrClient::scripted(vec![Ok(pane_envelope(
            "w1:p5",
            &root.to_string_lossy(),
            "working",
        ))]);
        let reply = handle_herdr_project_pane_action(
            ProjectPaneAction::Close,
            &ProjectPaneParams::default(),
            ProjectPaneDeps {
                cwd: root.clone(),
                client: &herdr,
                panes: Some(&panes),
            },
        )
        .await;
        assert_eq!(
            text_of(&reply),
            "Herdr project pane error (PANE_NOT_IDLE): Project pane 'w1:p5' is 'working', not \
             explicitly idle."
        );
        assert!(project_pane_binding_path(&root).exists());
        assert!(
            !herdr
                .verbs()
                .iter()
                .any(|verb| verb.starts_with("pane close"))
        );

        // ---- close, once idle ----
        let herdr = FakeHerdrClient::scripted(vec![
            Ok(pane_envelope("w1:p5", &root.to_string_lossy(), "idle")),
            Ok(json!({})),
        ]);
        let reply = handle_herdr_project_pane_action(
            ProjectPaneAction::Close,
            &ProjectPaneParams::default(),
            ProjectPaneDeps {
                cwd: root.clone(),
                client: &herdr,
                panes: Some(&panes),
            },
        )
        .await;
        assert_eq!(
            text_of(&reply),
            format!("Closed Herdr project pane w1:p5 for {}.", root.display())
        );
        assert!(herdr.verbs().contains(&"pane close w1:p5".to_owned()));
        assert!(!project_pane_binding_path(&root).exists());
        assert!(list_herdr_project_pane_roots(&root).unwrap().is_empty());
        assert!(panes.lock().unwrap().is_empty());
    }

    /// **T-PROJ-5.** GUT the ownership gate in `close` and a pane belonging to a DIFFERENT project
    /// is closed by a verb aimed at this one.
    #[tokio::test]
    async fn ownership_mismatch_refuses_close_and_leaves_the_binding() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        seed_binding(&root, "w1:p5").await;

        let herdr =
            FakeHerdrClient::scripted(vec![Ok(pane_envelope("w1:p5", "/somewhere/else", "idle"))]);
        let reply = handle_herdr_project_pane_action(
            ProjectPaneAction::Close,
            &ProjectPaneParams::default(),
            ProjectPaneDeps {
                cwd: root.clone(),
                client: &herdr,
                panes: None,
            },
        )
        .await;
        assert_eq!(
            text_of(&reply),
            format!(
                "Herdr project pane error (PANE_OWNERSHIP_UNVERIFIED): Project pane 'w1:p5' \
                 ownership is 'mismatch' for '{}'.",
                root.display()
            )
        );
        assert!(project_pane_binding_path(&root).exists());
    }

    /// **T-PROJ-6.** `:706` runs before `:709`. GUT the ordering and a root index that never
    /// shrinks makes session-start restore re-read dead roots forever.
    #[tokio::test]
    async fn project_close_with_no_binding_still_prunes_the_root_index() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let other = root.join("other");
        std::fs::create_dir_all(&other).unwrap();
        write_herdr_project_pane_root(&root, &root).await.unwrap();
        write_herdr_project_pane_root(&root, &other).await.unwrap();
        assert_eq!(list_herdr_project_pane_roots(&root).unwrap().len(), 2);

        // No binding for `root` at all — the ABSENT path.
        let herdr = FakeHerdrClient::default();
        let reply = handle_herdr_project_pane_action(
            ProjectPaneAction::Close,
            &ProjectPaneParams::default(),
            ProjectPaneDeps {
                cwd: root.clone(),
                client: &herdr,
                panes: None,
            },
        )
        .await;
        assert!(reply.is_ok());
        assert_eq!(
            text_of(&reply),
            format!(
                "No Herdr project pane binding exists for {}.",
                root.display()
            )
        );
        assert_eq!(list_herdr_project_pane_roots(&root).unwrap(), vec![other]);
    }

    /// **T-SUM-1** — the `[CYRUP-EXCEEDS-UPSTREAM]` summary. GUT the three cyrup rungs and every
    /// project-pane roster row renders a dash forever, because none of pi's seven keys exists on
    /// herdr 0.9.1's `PaneInfo`.
    #[test]
    fn a_project_pane_summary_comes_from_state_labels_first() {
        let with_labels = json!({ "pane": {
            "pane_id": "w1:p5",
            "agent_status": "working",
            "state_labels": { "working": "refactoring auth", "idle": "waiting" },
            "display_agent": "cyrup",
            "label": "repo",
        }});
        let runtime = project_pane_runtime(&with_labels).unwrap();
        assert_eq!(runtime.summary.as_deref(), Some("refactoring auth"));

        // No matching label → `display_agent`, then `label`.
        let no_match = json!({ "pane": {
            "pane_id": "w1:p5", "agent_status": "idle",
            "state_labels": { "working": "x" }, "display_agent": "cyrup",
        }});
        assert_eq!(
            project_pane_runtime(&no_match).unwrap().summary.as_deref(),
            Some("cyrup")
        );
        let only_label = json!({ "pane": { "pane_id": "w1:p5", "label": "repo" } });
        assert_eq!(
            project_pane_runtime(&only_label)
                .unwrap()
                .summary
                .as_deref(),
            Some("repo")
        );

        // pi's rungs still work, so a differently-shaped herdr is not lost.
        let pi_shape =
            json!({ "pane": { "pane_id": "w1:p5", "tokens": { "summary": "from tokens" } } });
        assert_eq!(
            project_pane_runtime(&pi_shape).unwrap().summary.as_deref(),
            Some("from tokens")
        );

        // Sanitisation (`:311-315`): control chars out, runs collapsed, 120 chars.
        assert_eq!(
            sanitized_summary(Some("a\u{1}b\n\n  c ")).as_deref(),
            Some("a b c")
        );
        assert_eq!(sanitized_summary(Some("   ")), None);
        assert_eq!(sanitized_summary(None), None);
        assert_eq!(
            sanitized_summary(Some(&"x".repeat(200))).map(|s| s.chars().count()),
            Some(120)
        );
    }

    /// The `[CYRUP-EXCEEDS-UPSTREAM]` ownership widening. GUT the `foreground_cwd` candidate and
    /// a pane whose shell has `cd`-ed into a subdirectory of its own project is refused as
    /// someone else's.
    #[test]
    fn ownership_verifies_on_either_cwd_and_mismatches_only_when_both_disagree() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let binding = binding_for(&root, "w1:p5");

        let runtime = |cwd: Option<&str>, foreground: Option<&str>| ProjectPaneRuntime {
            pane_id: "w1:p5".to_owned(),
            agent_status: "idle".to_owned(),
            cwd: cwd.map(str::to_owned),
            foreground_cwd: foreground.map(str::to_owned),
            ..ProjectPaneRuntime::default()
        };
        let root_str = root.to_string_lossy().into_owned();

        assert_eq!(
            project_pane_ownership(&runtime(Some(&root_str), None), &binding, &root),
            PaneOwnership::Verified
        );
        // pi would call this a mismatch; herdr's own docs say `cwd` is the LABEL cwd.
        assert_eq!(
            project_pane_ownership(
                &runtime(Some("/elsewhere"), Some(&root_str)),
                &binding,
                &root
            ),
            PaneOwnership::Verified
        );
        assert_eq!(
            project_pane_ownership(&runtime(Some("/a"), Some("/b")), &binding, &root),
            PaneOwnership::Mismatch
        );
        assert_eq!(
            project_pane_ownership(&runtime(None, None), &binding, &root),
            PaneOwnership::Unknown
        );
        // A different pane is always a mismatch, whatever the cwd says (`:421`).
        let other = ProjectPaneRuntime {
            pane_id: "w1:p9".to_owned(),
            cwd: Some(root_str),
            ..ProjectPaneRuntime::default()
        };
        assert_eq!(
            project_pane_ownership(&other, &binding, &root),
            PaneOwnership::Mismatch
        );
    }

    /// The DOUBLE STANDARD itself. GUT `binding_for_manager`'s `legacy` branches and the
    /// model-facing verbs start refusing a binding they should tolerate; GUT the strict branches
    /// and the public API starts accepting one it must not.
    #[tokio::test]
    async fn the_two_compatibility_modes_disagree_exactly_where_upstream_does() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(project_pane_dir(&root)).unwrap();
        std::fs::write(project_pane_binding_path(&root), b"{ not json").unwrap();

        let herdr = FakeHerdrClient::default();
        let legacy = ProjectPaneManager::new(&herdr, ToolCompatibility::Legacy);
        let strict = ProjectPaneManager::new(&herdr, ToolCompatibility::Strict);

        // A malformed binding: absent to the verbs, an error to the API.
        assert_eq!(
            legacy.status(&root).await.unwrap().state,
            ProjectPaneState::Absent
        );
        let failure = strict.status(&root).await.unwrap_err();
        assert_eq!(failure.code, ProjectPaneErrorCode::InvalidBinding);
        assert_eq!(failure.code.as_str(), "INVALID_BINDING");

        // A binding whose root is a DIFFERENT project: tolerated by the verbs, refused by the API.
        let mut foreign = binding_for(&root, "w1:p5");
        foreign.project_root = PathBuf::from("/some/other/project");
        std::fs::write(
            project_pane_binding_path(&root),
            serde_json::to_vec(&foreign).unwrap(),
        )
        .unwrap();
        let herdr =
            FakeHerdrClient::always(Ok(pane_envelope("w1:p5", &root.to_string_lossy(), "idle")));
        let legacy = ProjectPaneManager::new(&herdr, ToolCompatibility::Legacy);
        let strict = ProjectPaneManager::new(&herdr, ToolCompatibility::Strict);
        assert_eq!(
            legacy.status(&root).await.unwrap().state,
            ProjectPaneState::Open
        );
        assert_eq!(
            strict.status(&root).await.unwrap_err().code,
            ProjectPaneErrorCode::InvalidBinding
        );
    }

    /// `:492-494` / `:566-570`: a pane herdr answers for with an unreadable shape is OPEN to the
    /// verbs and an error to the API. GUT the legacy arm and `project.status` fails on a live
    /// pane the user can see.
    #[tokio::test]
    async fn an_invalid_pane_response_is_tolerated_only_in_legacy_mode() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        seed_binding(&root, "w1:p5").await;

        let herdr = FakeHerdrClient::always(Ok(json!({ "pane": {} })));
        let legacy = ProjectPaneManager::new(&herdr, ToolCompatibility::Legacy);
        let status = legacy.status(&root).await.unwrap();
        assert_eq!(status.state, ProjectPaneState::Open);
        assert_eq!(status.ownership, PaneOwnership::Unknown);
        assert!(!status.safe_to_close);
        assert_eq!(
            status.runtime.as_ref().unwrap().agent_status.as_str(),
            "unknown"
        );

        let strict = ProjectPaneManager::new(&herdr, ToolCompatibility::Strict);
        assert_eq!(
            strict.status(&root).await.unwrap_err().code,
            ProjectPaneErrorCode::InvalidPaneResponse
        );
    }

    /// `:496-500` — a pane herdr no longer has is STALE, not an error, and the binding survives so
    /// `project.close` can clean it up. GUT the stale arm and a user whose terminal restarted gets
    /// a hard failure with no way to clear it.
    #[tokio::test]
    async fn a_pane_herdr_no_longer_has_is_stale_in_both_modes() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        seed_binding(&root, "w1:p5").await;

        let herdr = FakeHerdrClient::always(Err(HerdrErrorCode::NotFound));
        for mode in [ToolCompatibility::Legacy, ToolCompatibility::Strict] {
            let manager = ProjectPaneManager::new(&herdr, mode);
            let status = manager.status(&root).await.unwrap();
            assert_eq!(status.state, ProjectPaneState::Stale);
            assert!(status.stale_reason.is_some());
            assert!(!status.safe_to_close);
        }

        // `project.status` renders it as an error carrying the binding path (`:696-699`).
        let reply = handle_herdr_project_pane_action(
            ProjectPaneAction::Status,
            &ProjectPaneParams::default(),
            ProjectPaneDeps {
                cwd: root.clone(),
                client: &herdr,
                panes: None,
            },
        )
        .await;
        assert!(reply.is_err());
        assert!(text_of(&reply).contains("Herdr project pane error (NOT_FOUND)"));
        assert!(text_of(&reply).contains(&format!(
            "Binding: {}",
            project_pane_binding_path(&root).display()
        )));

        // And `project.close` removes the stale binding rather than failing.
        let reply = handle_herdr_project_pane_action(
            ProjectPaneAction::Close,
            &ProjectPaneParams::default(),
            ProjectPaneDeps {
                cwd: root.clone(),
                client: &herdr,
                panes: None,
            },
        )
        .await;
        assert_eq!(
            text_of(&reply),
            format!("Closed Herdr project pane w1:p5 for {}.", root.display())
        );
        assert!(!project_pane_binding_path(&root).exists());
    }

    /// **V26** — the manager's FOURTH method. GUT the `focus` call out of `project.open` and
    /// `--focus` on an already-open pane silently does nothing, which is the bug V26 names.
    #[tokio::test]
    async fn project_open_focus_on_an_open_pane_focuses_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        seed_binding(&root, "w1:p5").await;
        let live = pane_envelope("w1:p5", &root.to_string_lossy(), "idle");

        let herdr = FakeHerdrClient::scripted(vec![
            Ok(Value::String("0.9.1".into())), // --version
            Ok(live.clone()),                  // open → pane get
            Ok(live.clone()),                  // status re-look → pane get
            Ok(live.clone()),                  // focus → status → pane get
            Ok(live.clone()),                  // focus → pane focus
            Ok(live.clone()),                  // post-focus status → pane get
        ]);
        let params = ProjectPaneParams {
            focus: Some(true),
            ..ProjectPaneParams::default()
        };
        let reply = handle_herdr_project_pane_action(
            ProjectPaneAction::Open,
            &params,
            ProjectPaneDeps {
                cwd: root.clone(),
                client: &herdr,
                panes: None,
            },
        )
        .await;
        assert_eq!(
            text_of(&reply),
            format!(
                "Herdr project pane w1:p5 is already open for {}. Focused tab w1:t1.",
                root.display()
            )
        );
        assert!(herdr.verbs().contains(&"pane focus w1:p5".to_owned()));
        assert!(
            !herdr
                .verbs()
                .iter()
                .any(|verb| verb.starts_with("pane split"))
        );
        // The focus is recorded on the binding (`:529-531`).
        assert!(
            read_herdr_project_pane_binding(&root)
                .unwrap()
                .last_focused_at
                .is_some()
        );
    }

    /// The session-start restore, and the one seam the status bridge consumes. GUT
    /// `restore_herdr_project_pane_snapshots`' insert and the bridge reports `· 0 panes` forever.
    #[tokio::test]
    async fn the_session_start_restore_reads_every_bound_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let bound = root.join("bound");
        let unbound = root.join("unbound");
        std::fs::create_dir_all(&bound).unwrap();
        std::fs::create_dir_all(&unbound).unwrap();
        seed_binding(&bound, "w1:p7").await;

        let mut panes = ProjectPaneSnapshots::new();
        restore_herdr_project_pane_snapshots(&mut panes, [&bound, &unbound], 1_700_000_000_000);

        assert_eq!(panes.len(), 1);
        let snapshot = panes.get(&bound).unwrap();
        assert_eq!(snapshot.pane_id, "w1:p7");
        assert_eq!(snapshot.state, ProjectPaneState::Open);
        assert_eq!(snapshot.ownership, PaneOwnership::Unknown);
        assert!(!snapshot.safe_to_close);
        assert_eq!(snapshot.refreshed_at, 1_700_000_000_000);
        assert_eq!(open_project_pane_count(&panes), 1);

        // A restore is ADDITIVE (`:303`): what the session already knew is not dropped.
        restore_herdr_project_pane_snapshots(&mut panes, [&unbound], 1_700_000_001_000);
        assert_eq!(panes.len(), 1);
    }

    /// The root index's own contract. GUT the sort/dedup and the file grows a duplicate on every
    /// re-open; GUT the delete-when-empty and an empty index file outlives the last pane.
    #[tokio::test]
    async fn the_root_index_is_a_sorted_set_that_deletes_itself_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let owner = std::fs::canonicalize(dir.path()).unwrap();
        let a = owner.join("a");
        let b = owner.join("b");

        assert!(list_herdr_project_pane_roots(&owner).unwrap().is_empty());
        write_herdr_project_pane_root(&owner, &b).await.unwrap();
        write_herdr_project_pane_root(&owner, &a).await.unwrap();
        write_herdr_project_pane_root(&owner, &a).await.unwrap();
        assert_eq!(
            list_herdr_project_pane_roots(&owner).unwrap(),
            vec![a.clone(), b.clone()]
        );

        remove_herdr_project_pane_root(&owner, &a).await.unwrap();
        assert_eq!(
            list_herdr_project_pane_roots(&owner).unwrap(),
            vec![b.clone()]
        );
        remove_herdr_project_pane_root(&owner, &b).await.unwrap();
        assert!(list_herdr_project_pane_roots(&owner).unwrap().is_empty());
        assert!(!project_pane_root_index_path(&owner).exists());

        // A malformed index is an ERROR, not an empty list (`:198`) — `project.close` propagates
        // it rather than silently pruning nothing.
        std::fs::create_dir_all(project_pane_dir(&owner)).unwrap();
        std::fs::write(project_pane_root_index_path(&owner), b"[]").unwrap();
        assert!(list_herdr_project_pane_roots(&owner).is_err());
    }

    /// `resolveProjectRoot` (`:390-399`). GUT the `is_dir` check and a `project.open` aimed at a
    /// FILE splits a pane whose cwd herdr will reject.
    #[test]
    fn a_project_root_must_be_a_directory_that_exists() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a-file");
        std::fs::write(&file, b"x").unwrap();

        let failure = resolve_project_root(&file).unwrap_err();
        assert_eq!(failure.code, ProjectPaneErrorCode::InvalidProjectRoot);
        assert!(failure.message.ends_with("is not a directory."));

        let missing = dir.path().join("nope");
        let failure = resolve_project_root(&missing).unwrap_err();
        assert_eq!(failure.code, ProjectPaneErrorCode::InvalidProjectRoot);
        assert!(failure.message.contains("is unavailable:"));

        assert!(resolve_project_root(dir.path()).is_ok());
    }

    /// The binding is shared with pi, so an unknown key survives the `focus` rewrite. GUT the
    /// `#[serde(flatten)] extra` and `project.open --focus` silently deletes it.
    #[test]
    fn an_unknown_key_survives_the_project_pane_binding_round_trip() {
        let raw = json!({
            "schemaVersion": 1,
            "kind": "herdr-project-pane",
            "projectRoot": "/a",
            "paneId": "w1:p5",
            "openedAt": "2026-09-21T00:00:00.000Z",
            "command": "pi",
            "piOnlyKey": 42,
        });
        let binding: HerdrProjectPaneBinding = serde_json::from_value(raw).unwrap();
        assert_eq!(binding.extra.get("piOnlyKey").unwrap(), 42);
        let back = serde_json::to_value(&binding).unwrap();
        assert_eq!(back["piOnlyKey"], 42);
        assert_eq!(back["kind"], "herdr-project-pane");
        assert_eq!(back["schemaVersion"], 1);
        // Absent optionals are OMITTED, never `null` — pi's own parse rejects `null`.
        assert!(back.get("lastFocusedAt").is_none());
        assert!(back.get("startupMessage").is_none());
    }

    /// The strict parse's extra rung (`:230-235`): present but blank is not valid.
    #[test]
    fn the_strict_reader_refuses_a_blank_required_field() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(project_pane_dir(root)).unwrap();
        let mut binding = binding_for(root, "w1:p5");
        binding.pane_id = "   ".to_owned();
        std::fs::write(
            project_pane_binding_path(root),
            serde_json::to_vec(&binding).unwrap(),
        )
        .unwrap();

        // Legacy accepts it, strict does not — the double standard, at the parse rung.
        assert!(read_herdr_project_pane_binding(root).is_some());
        assert_eq!(
            read_project_pane_binding(root).unwrap_err().code,
            ProjectPaneErrorCode::InvalidBinding
        );
    }

    fn binding_for(root: &Path, pane_id: &str) -> HerdrProjectPaneBinding {
        HerdrProjectPaneBinding {
            schema_version: SchemaVersion1,
            kind: HerdrProjectPaneKind::HerdrProjectPane,
            project_root: root.to_path_buf(),
            pane_id: pane_id.to_owned(),
            opened_at: "2026-09-21T00:00:00.000Z".to_owned(),
            last_focused_at: None,
            herdr_version: Some("0.9.1".to_owned()),
            command: "cyrup".to_owned(),
            startup_message: None,
            extra: Map::new(),
        }
    }

    async fn seed_binding(root: &Path, pane_id: &str) {
        write_project_pane_binding(
            &project_pane_binding_path(root),
            &binding_for(root, pane_id),
        )
        .await
        .unwrap();
    }
}
