//! Mirrors `tmp/herdr/src/api/schema/events.rs` — the subscription vocabulary, the two event
//! envelopes, and the params of the one in-band wait cyrup drives.
//!
//! herdr keeps three separate things in this one file and this mirror keeps them in the same one:
//!
//! 1. **The subscription request vocabulary** — [`EventsSubscribeParams`] and [`Subscription`]
//!    (`tmp/herdr/src/api/schema/events.rs:11-83`).
//! 2. **The two event envelopes** — [`EventEnvelope`] and [`SubscriptionEventEnvelope`]
//!    (`:361-365`, `:377-381`), which share one stream and spell their `event` field under two
//!    different conventions. See [`Event`].
//! 3. **The blocking methods' params** — [`PaneWaitForOutputParams`] is one of herdr's four in-band
//!    waits (`:94-105`); it belongs to the read half, not the stream half, because it is still one
//!    request and one answer.
//!
//! `EventsWaitParams`/`EventMatch` (`:87-91`, `:114-190`) are **not** mirrored: `events.wait` has
//! no cyrup caller, and unlike the records above nothing decodes one as a side effect.
//!
//! # The naming trap, which the docs do not mention
//!
//! Two envelope types ride the one subscription stream, and they disagree about how to spell an
//! event name:
//!
//! | flavour | type | `event` on the wire | herdr |
//! |---|---|---|---|
//! | lifecycle (26 kinds) | [`EventEnvelope`] | `pane_created` — `rename_all = "snake_case"` | `events.rs:192-194`, `:361-365` |
//! | the three rich ones | [`SubscriptionEventEnvelope`] | `pane.agent_status_changed` — explicit renames | `:367-381` |
//!
//! Confirmed against the checked-in artifact: `event.$defs.EventKind.enum` is
//! `["workspace_created", …, "pane_agent_status_changed", "layout_updated"]` while
//! `subscription_event.$defs.SubscriptionEventKind.enum` is
//! `["pane.output_matched","pane.agent_status_changed","pane.scroll_changed"]`
//! (`tmp/herdr/docs/next/api/herdr-api.schema.json`). The **request** side uses dots for all of
//! them ([`Subscription`], `events.rs:16-83`).
//!
//! So: **subscribe with dots, receive lifecycle with underscores.** [`Event::decode`] is the one
//! place that has to know it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::common::{AgentStatus, ReadSource, default_true};
use super::panes::{PaneInfo, PaneLayoutSnapshot, PaneReadResult, PaneScrollInfo};
use super::tabs::TabInfo;
use super::workspaces::WorkspaceInfo;
use super::worktrees::WorktreeInfo;

/// `PaneWaitForOutputParams` (`tmp/herdr/src/api/schema/events.rs:94-105`).
///
/// **This method holds the connection open.** herdr polls the pane on its own
/// `CONNECTION_POLL_INTERVAL` tick (`tmp/herdr/src/api/server.rs:28`, consumed at
/// `tmp/herdr/src/api/wait.rs:126`), re-reading it through an internal `pane.read` until the match
/// lands, and only then writes its single answer (`wait.rs:96-110`). It is still one request and
/// one response line, so it rides the same one-shot transport as every other method — what it
/// needs is a **deadline that outlives the wait**.
///
/// **`timeout_ms` absent means herdr waits forever.** The deadline is built with
/// `params.timeout_ms.map(…)` (`wait.rs:30-32`) and every later check is
/// `deadline.is_some_and(…)` (`:113`), so `None` is not "herdr's default" — it is no deadline at
/// all. [`crate::HerdrClient::pane_wait_for_output`] therefore always imposes one of its own.
///
/// The field is `match`, a Rust keyword, spelled `r#match` here exactly as herdr spells it
/// (`events.rs:98`); serde puts `"match"` on the wire either way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneWaitForOutputParams {
    /// The pane to watch.
    pub pane_id: String,
    /// Which region herdr re-reads. Note that herdr narrows it for the match itself
    /// (`output_match_read_source`, `wait.rs:63`).
    pub source: ReadSource,
    /// A line cap on each internal read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
    /// What counts as a match.
    pub r#match: OutputMatch,
    /// herdr's own deadline in milliseconds. **Absent ⇒ herdr never gives up.**
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Strip ANSI escapes before matching. herdr defaults this to `true`.
    pub strip_ansi: bool,
}

impl PaneWaitForOutputParams {
    /// Wait for `r#match` on `pane_id`, with herdr's own defaults and no herdr-side deadline.
    ///
    /// Set [`Self::timeout_ms`] unless the caller's own deadline is the intended bound — see the
    /// type doc.
    pub fn new(pane_id: impl Into<String>, source: ReadSource, r#match: OutputMatch) -> Self {
        Self {
            pane_id: pane_id.into(),
            source,
            lines: None,
            r#match,
            timeout_ms: None,
            strip_ansi: default_true(),
        }
    }
}

/// `OutputMatch` (`tmp/herdr/src/api/schema/events.rs:107-112`), internally tagged as
/// `#[serde(tag = "type", rename_all = "snake_case")]`.
///
/// A [`Self::Regex`] that does not compile is `invalid_regex`, refused before the first read
/// (`tmp/herdr/src/api/wait.rs:35-49`) — so a bad pattern fails immediately rather than after the
/// timeout. The dialect is Rust's `regex` crate, herdr's own (`wait.rs:4`, `Regex::new`), which
/// has no backreferences and no lookaround.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputMatch {
    /// A literal substring.
    Substring {
        /// The text to find.
        value: String,
    },
    /// A `regex`-crate pattern.
    Regex {
        /// The pattern.
        value: String,
    },
}

impl OutputMatch {
    /// A literal substring match.
    pub fn substring(value: impl Into<String>) -> Self {
        Self::Substring {
            value: value.into(),
        }
    }

    /// A `regex`-crate pattern match.
    pub fn regex(value: impl Into<String>) -> Self {
        Self::Regex {
            value: value.into(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The subscription request.
// ---------------------------------------------------------------------------------------------

/// `EventsSubscribeParams` (`tmp/herdr/src/api/schema/events.rs:11-14`).
///
/// An empty `subscriptions` list is accepted by herdr and acknowledged
/// (`stream_subscriptions`, `tmp/herdr/src/api/server.rs:715-757`, iterates an empty vector and
/// writes the ack), which gives a stream that never yields an event. It is not an error and is not
/// refused here either — a caller that wants nothing has said so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventsSubscribeParams {
    /// What to subscribe to.
    pub subscriptions: Vec<Subscription>,
}

/// `Subscription` (`tmp/herdr/src/api/schema/events.rs:16-83`), internally tagged as
/// `#[serde(tag = "type")]` with an **explicit dotted rename on every variant**.
///
/// All 27 are mirrored rather than the handful a first consumer wants, because unlike a method —
/// which is one variant with one caller — a subscription this enum omits cannot be asked for at
/// all, and the stream half would then be unable to express a subscription herdr accepts. The
/// asymmetry is real: the request enum is the *whole* vocabulary of one method.
///
/// **Dots here, underscores on the way back** for the 24 that answer in the lifecycle envelope —
/// see the module doc and
/// [`Event`]. The three that answer in their own envelope ([`Self::PaneOutputMatched`],
/// [`Self::PaneAgentStatusChanged`], [`Self::PaneScrollChanged`]) keep their dots both ways, and
/// they are also the only three that carry parameters.
///
/// Serialize-only: this client asks, it never parses an ask.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum Subscription {
    /// A workspace was created.
    #[serde(rename = "workspace.created")]
    WorkspaceCreated {},
    /// A workspace record changed.
    #[serde(rename = "workspace.updated")]
    WorkspaceUpdated {},
    /// A workspace's reported tokens changed, or one expired. herdr notes that this one is
    /// available to API subscribers but does **not** invoke plugin event hooks
    /// (`socket-api.mdx:790`).
    #[serde(rename = "workspace.metadata_updated")]
    WorkspaceMetadataUpdated {},
    /// A workspace was renamed.
    #[serde(rename = "workspace.renamed")]
    WorkspaceRenamed {},
    /// A workspace was moved to a new position.
    #[serde(rename = "workspace.moved")]
    WorkspaceMoved {},
    /// A block of workspaces was reordered atomically.
    #[serde(rename = "workspace.reordered")]
    WorkspaceReordered {},
    /// A workspace was closed.
    #[serde(rename = "workspace.closed")]
    WorkspaceClosed {},
    /// A workspace took focus.
    #[serde(rename = "workspace.focused")]
    WorkspaceFocused {},
    /// A git worktree was created and opened.
    #[serde(rename = "worktree.created")]
    WorktreeCreated {},
    /// An existing git worktree was opened.
    #[serde(rename = "worktree.opened")]
    WorktreeOpened {},
    /// A git worktree was removed.
    #[serde(rename = "worktree.removed")]
    WorktreeRemoved {},
    /// A tab was created.
    #[serde(rename = "tab.created")]
    TabCreated {},
    /// A tab was closed.
    #[serde(rename = "tab.closed")]
    TabClosed {},
    /// A tab took focus.
    #[serde(rename = "tab.focused")]
    TabFocused {},
    /// A tab was renamed.
    #[serde(rename = "tab.renamed")]
    TabRenamed {},
    /// A tab was moved.
    #[serde(rename = "tab.moved")]
    TabMoved {},
    /// A pane was created.
    #[serde(rename = "pane.created")]
    PaneCreated {},
    /// A pane was closed.
    #[serde(rename = "pane.closed")]
    PaneClosed {},
    /// A pane record changed. Terminal-title changes can emit it, but a spinner-only raw-title
    /// change does not when `terminal_title_stripped` is unchanged (`socket-api.mdx:831`).
    #[serde(rename = "pane.updated")]
    PaneUpdated {},
    /// A pane took focus — including a manual selection by any client attached to this server
    /// (`socket-api.mdx:828-830`). Re-selecting the selected pane does not emit it again.
    #[serde(rename = "pane.focused")]
    PaneFocused {},
    /// A pane was moved.
    #[serde(rename = "pane.moved")]
    PaneMoved {},
    /// A pane's process exited.
    #[serde(rename = "pane.exited")]
    PaneExited {},
    /// herdr's own screen detection identified (or released) an agent in a pane.
    #[serde(rename = "pane.agent_detected")]
    PaneAgentDetected {},
    /// One pane's output matched a pattern. **Parameterised, and answered in its own envelope**
    /// ([`SubscriptionEventKind::PaneOutputMatched`]).
    #[serde(rename = "pane.output_matched")]
    PaneOutputMatched {
        /// The pane to watch.
        pane_id: String,
        /// Which region herdr re-reads; it narrows it for the match itself
        /// (`output_match_read_source`, `tmp/herdr/src/api/wait.rs:63`).
        source: ReadSource,
        /// A line cap on each internal read.
        #[serde(skip_serializing_if = "Option::is_none")]
        lines: Option<u32>,
        /// What counts as a match.
        r#match: OutputMatch,
        /// Strip ANSI escapes before matching. herdr defaults this to `true`.
        strip_ansi: bool,
    },
    /// One pane's observed agent status changed. **Parameterised, and answered in its own
    /// envelope** ([`SubscriptionEventKind::PaneAgentStatusChanged`]).
    ///
    /// Setting `agent_status` narrows the subscription to transitions **into** that status;
    /// leaving it unset reports every transition.
    #[serde(rename = "pane.agent_status_changed")]
    PaneAgentStatusChanged {
        /// The pane to watch.
        pane_id: String,
        /// The status to wait for, or every status when unset.
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_status: Option<AgentStatus>,
    },
    /// One pane's scroll position changed. **Parameterised, and answered in its own envelope**
    /// ([`SubscriptionEventKind::ScrollChanged`]).
    #[serde(rename = "pane.scroll_changed")]
    PaneScrollChanged {
        /// The pane to watch.
        pane_id: String,
    },
    /// A tab's layout changed. The event carries the whole
    /// [`super::panes::PaneLayoutSnapshot`] for one tab, and a client that bootstrapped from
    /// `session.snapshot` replaces the cached layout with the same `workspace_id`/`tab_id`
    /// (`socket-api.mdx:835-838`).
    #[serde(rename = "layout.updated")]
    LayoutUpdated {},
}

impl Subscription {
    /// Watch `pane_id` for `agent_status`, or for every transition when it is `None`.
    #[must_use]
    pub fn pane_agent_status_changed(
        pane_id: impl Into<String>,
        agent_status: Option<AgentStatus>,
    ) -> Self {
        Self::PaneAgentStatusChanged {
            pane_id: pane_id.into(),
            agent_status,
        }
    }

    /// Watch `pane_id` for `r#match` in `source`, with herdr's own `strip_ansi` default.
    #[must_use]
    pub fn pane_output_matched(
        pane_id: impl Into<String>,
        source: ReadSource,
        r#match: OutputMatch,
    ) -> Self {
        Self::PaneOutputMatched {
            pane_id: pane_id.into(),
            source,
            lines: None,
            r#match,
            strip_ansi: super::common::default_true(),
        }
    }

    /// Watch `pane_id`'s scroll position.
    #[must_use]
    pub fn pane_scroll_changed(pane_id: impl Into<String>) -> Self {
        Self::PaneScrollChanged {
            pane_id: pane_id.into(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The two envelopes that share the stream.
// ---------------------------------------------------------------------------------------------

/// `EventKind` (`tmp/herdr/src/api/schema/events.rs:192-221`), `rename_all = "snake_case"`.
///
/// **Closed, and safely so** — unlike [`AgentStatus`], which is required on records this client
/// must decode and therefore carries an open arm. Nothing here is required anywhere: a 27th kind
/// cannot kill the stream, because [`Event::decode`] dispatches on the `event` **string** before
/// it decodes anything, so an unknown name is [`Event::Unrecognised`] carrying the raw line and
/// the subscription keeps running. The closed enum is the *named* half of an already-open
/// dispatch, not the whole of it.
///
/// (The reason that used to stand here — "`#[serde(other)]` is only available on an internally or
/// adjacently tagged enum, so a bare string enum has no forward-compatible arm to offer" — was
/// false as a general claim and has been deleted: variant-level `#[serde(untagged)]` is exactly
/// that arm, and [`AgentStatus`] now uses it. It was never what made this type safe.)
///
/// herdr's own type is closed too, with its CI failing on drift against the published artifact
/// (`src/api/schema/tests.rs:182-207`), so a 27th kind is a protocol change herdr flags at its own
/// gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A workspace was created.
    WorkspaceCreated,
    /// A workspace record changed.
    WorkspaceUpdated,
    /// A workspace's tokens changed or expired.
    WorkspaceMetadataUpdated,
    /// A workspace was closed.
    WorkspaceClosed,
    /// A workspace was renamed.
    WorkspaceRenamed,
    /// A workspace was moved.
    WorkspaceMoved,
    /// Workspaces were reordered.
    WorkspaceReordered,
    /// A workspace took focus.
    WorkspaceFocused,
    /// A worktree was created.
    WorktreeCreated,
    /// A worktree was opened.
    WorktreeOpened,
    /// A worktree was removed.
    WorktreeRemoved,
    /// A tab was created.
    TabCreated,
    /// A tab was closed.
    TabClosed,
    /// A tab was renamed.
    TabRenamed,
    /// A tab was moved.
    TabMoved,
    /// A tab took focus.
    TabFocused,
    /// A pane was created.
    PaneCreated,
    /// A pane was closed.
    PaneClosed,
    /// A pane record changed.
    PaneUpdated,
    /// A pane took focus.
    PaneFocused,
    /// A pane was moved.
    PaneMoved,
    /// A pane's content revision advanced. **There is no [`Subscription`] for this one** — herdr
    /// declares the kind (`events.rs:216`) but not a way to ask for it, and its own hook list
    /// excludes it as high-volume (`PLUGIN_HOOK_EVENT_KINDS`, `:286-310`).
    PaneOutputChanged,
    /// A pane's process exited.
    PaneExited,
    /// herdr's screen detection identified or released an agent.
    PaneAgentDetected,
    /// A pane's observed agent status changed.
    PaneAgentStatusChanged,
    /// A tab's layout changed.
    LayoutUpdated,
}

impl EventKind {
    /// herdr's own `dot_name` (`tmp/herdr/src/api/schema/events.rs:223-252`) — the dotted spelling
    /// a [`Subscription`] uses, which is **not** the snake_case spelling this kind arrives under.
    #[must_use]
    pub const fn dot_name(self) -> &'static str {
        match self {
            Self::WorkspaceCreated => "workspace.created",
            Self::WorkspaceUpdated => "workspace.updated",
            Self::WorkspaceMetadataUpdated => "workspace.metadata_updated",
            Self::WorkspaceClosed => "workspace.closed",
            Self::WorkspaceRenamed => "workspace.renamed",
            Self::WorkspaceMoved => "workspace.moved",
            Self::WorkspaceReordered => "workspace.reordered",
            Self::WorkspaceFocused => "workspace.focused",
            Self::WorktreeCreated => "worktree.created",
            Self::WorktreeOpened => "worktree.opened",
            Self::WorktreeRemoved => "worktree.removed",
            Self::TabCreated => "tab.created",
            Self::TabClosed => "tab.closed",
            Self::TabRenamed => "tab.renamed",
            Self::TabMoved => "tab.moved",
            Self::TabFocused => "tab.focused",
            Self::PaneCreated => "pane.created",
            Self::PaneClosed => "pane.closed",
            Self::PaneUpdated => "pane.updated",
            Self::PaneFocused => "pane.focused",
            Self::PaneMoved => "pane.moved",
            Self::PaneOutputChanged => "pane.output_changed",
            Self::PaneExited => "pane.exited",
            Self::PaneAgentDetected => "pane.agent_detected",
            Self::PaneAgentStatusChanged => "pane.agent_status_changed",
            Self::LayoutUpdated => "layout.updated",
        }
    }
}

/// `EventEnvelope` (`tmp/herdr/src/api/schema/events.rs:361-365`) — a lifecycle event.
///
/// The payload is **doubly tagged**: `event` is the snake_case [`EventKind`] and `data` is an
/// internally tagged [`EventData`] under the same name (`tag = "type"`, `:420-422`), so `data`
/// alone would be sufficient to decode. Both are mirrored because herdr sends both and a consumer
/// matching on one should not have to trust that the other agrees.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EventEnvelope {
    /// The kind, snake_case.
    pub event: EventKind,
    /// The payload.
    pub data: EventData,
}

/// `SubscriptionEventKind` (`tmp/herdr/src/api/schema/events.rs:367-375`) — the three kinds that
/// arrive in their own envelope, under their **dotted** names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum SubscriptionEventKind {
    /// `pane.output_matched`.
    #[serde(rename = "pane.output_matched")]
    PaneOutputMatched,
    /// `pane.agent_status_changed`. Note that a lifecycle [`EventKind::PaneAgentStatusChanged`]
    /// also exists and spells itself `pane_agent_status_changed`; they are different envelopes
    /// with different payloads.
    #[serde(rename = "pane.agent_status_changed")]
    PaneAgentStatusChanged,
    /// `pane.scroll_changed`.
    #[serde(rename = "pane.scroll_changed")]
    ScrollChanged,
}

impl SubscriptionEventKind {
    /// The wire spelling — dotted, and identical to the [`Subscription`] that asked for it.
    #[must_use]
    pub const fn dot_name(self) -> &'static str {
        match self {
            Self::PaneOutputMatched => "pane.output_matched",
            Self::PaneAgentStatusChanged => "pane.agent_status_changed",
            Self::ScrollChanged => "pane.scroll_changed",
        }
    }
}

/// `SubscriptionEventEnvelope` (`tmp/herdr/src/api/schema/events.rs:377-381`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionEventEnvelope {
    /// The kind, dotted.
    pub event: SubscriptionEventKind,
    /// The payload.
    pub data: SubscriptionEventData,
}

/// `SubscriptionEventData` (`tmp/herdr/src/api/schema/events.rs:383-389`).
///
/// **herdr declares this `#[serde(untagged)]` and this mirror does not decode it that way** — the
/// same `[CYRUP-EXCEEDS-UPSTREAM]` as [`super::response::WireResponse`], for the same premise: an
/// untagged decode discards each arm's real error and reports the single string *"data did not
/// match any variant of untagged enum SubscriptionEventData"* at `line: 0, column: 0`. The `event`
/// field sitting beside it already names the arm, so [`Event::decode`] dispatches on that and
/// decodes **one** struct, and serde's own ``missing field `workspace_id` `` survives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionEventData {
    /// [`SubscriptionEventKind::PaneOutputMatched`]'s payload.
    PaneOutputMatched(PaneOutputMatchedEvent),
    /// [`SubscriptionEventKind::PaneAgentStatusChanged`]'s payload.
    PaneAgentStatusChanged(PaneAgentStatusChangedEvent),
    /// [`SubscriptionEventKind::ScrollChanged`]'s payload.
    ScrollChanged(PaneScrollChangedEvent),
}

/// `PaneOutputMatchedEvent` (`tmp/herdr/src/api/schema/events.rs:391-396`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaneOutputMatchedEvent {
    /// The pane that matched.
    pub pane_id: String,
    /// The line that matched.
    pub matched_line: String,
    /// The read the match was found in. Its `revision` is **`0`**, like every other
    /// pane-read-derived `revision` at this pin: `ActiveOutputMatchedSubscription::poll`
    /// (`tmp/herdr/src/api/subscriptions.rs:295-320`) builds this event from `pane_read`
    /// (`:493-515`), which dispatches `Method::PaneRead` to the one handler that hard-codes
    /// `revision: 0` (`tmp/herdr/src/app/api/panes.rs:1540`). See
    /// [`super::panes::PaneReadResult`] — and [`super::panes::PaneInfo::revision`] for the one
    /// herdr does advance.
    pub read: PaneReadResult,
}

/// `PaneAgentStatusChangedEvent` (`tmp/herdr/src/api/schema/events.rs:398-411`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaneAgentStatusChangedEvent {
    /// The pane.
    pub pane_id: String,
    /// Its workspace.
    pub workspace_id: String,
    /// The new observed status — five values, `Done` included.
    pub agent_status: AgentStatus,
    /// The agent herdr believes occupies the pane.
    #[serde(default)]
    pub agent: Option<String>,
    /// The reported title.
    #[serde(default)]
    pub title: Option<String>,
    /// The reported display name.
    #[serde(default)]
    pub display_agent: Option<String>,
    /// Per-state label overrides.
    #[serde(default)]
    pub state_labels: BTreeMap<String, String>,
}

/// `PaneScrollChangedEvent` (`tmp/herdr/src/api/schema/events.rs:413-418`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaneScrollChangedEvent {
    /// The pane.
    pub pane_id: String,
    /// Its workspace.
    pub workspace_id: String,
    /// Where the viewport now sits.
    pub scroll: PaneScrollInfo,
}

/// `EventData` (`tmp/herdr/src/api/schema/events.rs:420-556`), internally tagged as
/// `#[serde(tag = "type", rename_all = "snake_case")]`.
///
/// All 26 payloads, because an [`EventKind`] this enum omitted would decode as a failure on a
/// stream the consumer explicitly subscribed to — the one shape that is worse than not offering
/// the subscription at all.
///
/// **No `#[serde(other)]` arm here, deliberately.** The forward-compatible arm belongs one level
/// up, on [`Event`], which dispatches on the `event` name: a payload whose `type` this client does
/// not know while its `event` name *is* known means herdr changed a payload under a name it kept,
/// and folding that into a catch-all would report a changed event as a readable one. It is
/// [`crate::HerdrError::Malformed`] instead, with serde's own message naming the field.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum EventData {
    /// A workspace was created, with optional worktree provenance on the record.
    WorkspaceCreated {
        /// The new workspace.
        workspace: WorkspaceInfo,
    },
    /// A workspace record changed.
    WorkspaceUpdated {
        /// The workspace as it now reads.
        workspace: WorkspaceInfo,
    },
    /// A workspace's tokens changed or a TTL expired.
    WorkspaceMetadataUpdated {
        /// The workspace as it now reads.
        workspace: WorkspaceInfo,
    },
    /// A workspace was closed. herdr includes a final snapshot when it can still identify the
    /// workspace before removal (`socket-api.mdx:820`).
    WorkspaceClosed {
        /// The workspace that closed.
        workspace_id: String,
        /// Its last record, when herdr could still produce one.
        #[serde(default)]
        workspace: Option<WorkspaceInfo>,
    },
    /// A workspace was renamed.
    WorkspaceRenamed {
        /// The workspace.
        workspace_id: String,
        /// Its new label.
        label: String,
    },
    /// A workspace was moved, with the authoritative ordered list after the move.
    WorkspaceMoved {
        /// The workspace that moved.
        workspace_id: String,
        /// Where it was asked to land.
        insert_index: usize,
        /// Every workspace, in order, after the move.
        workspaces: Vec<WorkspaceInfo>,
    },
    /// A block of workspaces was reordered atomically.
    WorkspaceReordered {
        /// The moved block, in order.
        workspace_ids: Vec<String>,
        /// The anchor it was placed before, when there was one.
        #[serde(default)]
        before_workspace_id: Option<String>,
        /// Every workspace, in order, after the move.
        workspaces: Vec<WorkspaceInfo>,
    },
    /// A workspace took focus.
    WorkspaceFocused {
        /// The focused workspace.
        workspace_id: String,
    },
    /// A git worktree was created and opened.
    WorktreeCreated {
        /// The workspace it opened in.
        workspace: WorkspaceInfo,
        /// The checkout.
        worktree: WorktreeInfo,
    },
    /// An existing git worktree was opened.
    WorktreeOpened {
        /// The workspace it opened in.
        workspace: WorkspaceInfo,
        /// The checkout.
        worktree: WorktreeInfo,
        /// Whether it was already open.
        already_open: bool,
    },
    /// A git worktree was removed.
    WorktreeRemoved {
        /// The workspace it was open in.
        workspace_id: String,
        /// That workspace's last record, when herdr could still produce one.
        #[serde(default)]
        workspace: Option<WorkspaceInfo>,
        /// The checkout that was removed.
        worktree: WorktreeInfo,
        /// Whether the removal was forced.
        forced: bool,
    },
    /// A tab was created.
    TabCreated {
        /// The new tab.
        tab: TabInfo,
    },
    /// A tab was closed.
    TabClosed {
        /// The tab.
        tab_id: String,
        /// Its workspace.
        workspace_id: String,
    },
    /// A tab was renamed.
    TabRenamed {
        /// The tab.
        tab_id: String,
        /// Its workspace.
        workspace_id: String,
        /// Its new label.
        label: String,
    },
    /// A tab was moved, with that workspace's ordered tabs after the move.
    TabMoved {
        /// The tab that moved.
        tab_id: String,
        /// Its workspace.
        workspace_id: String,
        /// Where it was asked to land.
        insert_index: usize,
        /// Every tab of that workspace, in order, after the move.
        tabs: Vec<TabInfo>,
    },
    /// A tab took focus.
    TabFocused {
        /// The focused tab.
        tab_id: String,
        /// Its workspace.
        workspace_id: String,
    },
    /// A pane was created.
    PaneCreated {
        /// The new pane.
        pane: PaneInfo,
    },
    /// A pane was closed.
    PaneClosed {
        /// The pane.
        pane_id: String,
        /// Its workspace.
        workspace_id: String,
    },
    /// A pane record changed.
    PaneUpdated {
        /// The pane as it now reads.
        pane: PaneInfo,
    },
    /// A pane took focus.
    PaneFocused {
        /// The focused pane.
        pane_id: String,
        /// Its workspace.
        workspace_id: String,
    },
    /// A pane was moved, possibly creating or closing a workspace or tab on the way.
    ///
    /// `pane` is boxed in herdr too (`events.rs:511`) and for the same reason: it is by far the
    /// largest field of the largest variant.
    PaneMoved {
        /// The pane's id before the move.
        previous_pane_id: String,
        /// Its workspace before the move.
        previous_workspace_id: String,
        /// Its tab before the move.
        previous_tab_id: String,
        /// The pane as it now reads.
        pane: Box<PaneInfo>,
        /// A workspace the move created.
        #[serde(default)]
        created_workspace: Option<WorkspaceInfo>,
        /// A tab the move created.
        #[serde(default)]
        created_tab: Option<TabInfo>,
        /// A workspace the move emptied and closed.
        #[serde(default)]
        closed_workspace_id: Option<String>,
        /// A tab the move emptied and closed.
        #[serde(default)]
        closed_tab_id: Option<String>,
    },
    /// A pane's content revision advanced. High-volume: herdr excludes it from plugin hooks
    /// (`events.rs:286-310`) and offers no [`Subscription`] for it.
    PaneOutputChanged {
        /// The pane.
        pane_id: String,
        /// Its workspace.
        workspace_id: String,
        /// The new terminal revision — the same counter
        /// [`super::panes::PaneInfo::revision`] carries, not the dead `0` of a
        /// [`super::panes::PaneReadResult`]. Mirrored as declared; herdr constructs this variant
        /// nowhere outside its own tests at this pin
        /// (`tmp/herdr/src/app/api/plugins/mod.rs:3515` is a `#[test]`), which is the other half
        /// of why there is no [`Subscription`] to ask for it.
        revision: u64,
    },
    /// A pane's process exited.
    PaneExited {
        /// The pane.
        pane_id: String,
        /// Its workspace.
        workspace_id: String,
    },
    /// herdr's own screen detection identified an agent in a pane — or, with `released`, let go
    /// of one.
    PaneAgentDetected {
        /// The pane.
        pane_id: String,
        /// Its workspace.
        workspace_id: String,
        /// The agent herdr believes it saw.
        #[serde(default)]
        agent: Option<String>,
        /// Whether this is a release rather than a detection.
        #[serde(default)]
        released: bool,
        /// The status the released agent ended on.
        #[serde(default)]
        final_status: Option<AgentStatus>,
    },
    /// A pane's observed agent status changed.
    ///
    /// The **lifecycle** flavour, spelled `pane_agent_status_changed`. The dotted
    /// [`SubscriptionEventKind::PaneAgentStatusChanged`] carries the same information in a
    /// different envelope; see the module doc.
    PaneAgentStatusChanged {
        /// The pane.
        pane_id: String,
        /// Its workspace.
        workspace_id: String,
        /// The new observed status.
        agent_status: AgentStatus,
        /// The agent herdr believes occupies the pane.
        #[serde(default)]
        agent: Option<String>,
        /// The reported title.
        #[serde(default)]
        title: Option<String>,
        /// The reported display name.
        #[serde(default)]
        display_agent: Option<String>,
        /// Per-state label overrides.
        #[serde(default)]
        state_labels: BTreeMap<String, String>,
    },
    /// A tab's layout changed; replace the cached layout with the same `workspace_id`/`tab_id`.
    LayoutUpdated {
        /// The tab's whole layout.
        layout: PaneLayoutSnapshot,
    },
}

/// One line pushed on a live `events.subscribe` stream.
///
/// Three arms, and the third is why a herdr upgrade cannot sever a running subscription:
///
/// - [`Self::Lifecycle`] — one of the 26 [`EventKind`]s, spelled snake_case.
/// - [`Self::Subscription`] — one of the three [`SubscriptionEventKind`]s, spelled with dots.
/// - [`Self::Unrecognised`] — an `event` name this client does not know, carried **verbatim** as
///   the raw JSON value so a consumer can log exactly what arrived. It is not an error and it does
///   not end the stream.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A lifecycle event (`pane_created`, `workspace_closed`, `layout_updated`, …).
    Lifecycle(Box<EventEnvelope>),
    /// One of the three rich subscription events.
    Subscription(Box<SubscriptionEventEnvelope>),
    /// An `event` name this client does not know, kept as it arrived.
    Unrecognised(serde_json::Value),
}

impl Event {
    /// Decode one pushed line.
    ///
    /// **Dispatches on the `event` name before decoding anything**, which is what keeps the two
    /// spelling conventions (module doc) and forward compatibility from fighting: a name this
    /// client does not know is [`Self::Unrecognised`], while a name it does know decodes exactly
    /// one struct, so a payload change surfaces as serde's own ``missing field `workspace_id` ``
    /// rather than an untagged enum's contentless refusal.
    ///
    /// # Errors
    /// The `serde_json::Error` of the arm the `event` name selected, or of the line itself when it
    /// is not a JSON object.
    pub fn decode(line: &str) -> serde_json::Result<Self> {
        let value: serde_json::Value = serde_json::from_str(line)?;
        let Some(name) = value.get("event").and_then(serde_json::Value::as_str) else {
            return Ok(Self::Unrecognised(value));
        };
        match name {
            "pane.output_matched" => decode_subscription(
                value,
                SubscriptionEventKind::PaneOutputMatched,
                SubscriptionEventData::PaneOutputMatched,
            ),
            "pane.agent_status_changed" => decode_subscription(
                value,
                SubscriptionEventKind::PaneAgentStatusChanged,
                SubscriptionEventData::PaneAgentStatusChanged,
            ),
            "pane.scroll_changed" => decode_subscription(
                value,
                SubscriptionEventKind::ScrollChanged,
                SubscriptionEventData::ScrollChanged,
            ),
            _ => {
                // The name is the discriminator, so an unknown one is answered before the payload
                // is looked at: `EventKind` is closed (see its doc) and decoding it first is what
                // turns a 27th kind into `Unrecognised` instead of a dead stream.
                if serde_json::from_value::<EventKind>(serde_json::Value::String(name.to_owned()))
                    .is_err()
                {
                    return Ok(Self::Unrecognised(value));
                }
                serde_json::from_value(value).map(|envelope| Self::Lifecycle(Box::new(envelope)))
            }
        }
    }

    /// The `event` name as it arrived — snake_case for a lifecycle event, dotted for the three
    /// rich ones, and the raw string for an unrecognised one.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Lifecycle(envelope) => envelope.event.dot_name(),
            Self::Subscription(envelope) => envelope.event.dot_name(),
            Self::Unrecognised(value) => value
                .get("event")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<no event name>"),
        }
    }
}

/// Decode a [`SubscriptionEventEnvelope`]'s `data` as exactly one struct, chosen by the `event`
/// name the caller already read.
fn decode_subscription<T: serde::de::DeserializeOwned>(
    value: serde_json::Value,
    event: SubscriptionEventKind,
    wrap: fn(T) -> SubscriptionEventData,
) -> serde_json::Result<Event> {
    #[derive(Deserialize)]
    struct DataOnly<T> {
        data: T,
    }
    let DataOnly { data } = serde_json::from_value::<DataOnly<T>>(value)?;
    Ok(Event::Subscription(Box::new(SubscriptionEventEnvelope {
        event,
        data: wrap(data),
    })))
}
