//! `inspector.*` and `project.*` — VL-S6's verb half, over the workspace's one herdr client.
//!
//! [`types`] and [`plugins`] are the CONTRACT: every shared type, error vocabulary, shared
//! literal and injection seam, written before any implementation so that concurrently-written
//! modules agree by construction. The other modules are implementations and own only themselves.
//!
//! **"Shared" is decided by how many modules touch it, not by where it was first written.** Four
//! things started life in implementation modules and were reached across module boundaries from
//! there; they are in [`types`] now:
//!
//! * [`types::INSPECTOR_SUBCOMMAND`] — written by [`actions`], recognised by
//!   `crates/cyrup/src/subagent_inspector_cmd.rs`, rendered by [`runner`];
//! * [`types::inspector_header_line`] and [`types::INSPECTOR_HEADER_PREFIX`] — the dashboard's
//!   first line, PRINTED by [`runner`] and MATCHED by [`herdr::actions`]'s
//!   `pane wait-output --match`. A drift between those two halves times out every
//!   `inspector.open` and no unit test in this subtree notices; see that function;
//! * [`types::HerdrProjectPaneSnapshot`] / [`types::ProjectPaneSnapshots`] — produced by
//!   [`herdr::project_panes`], held live by `extension/executor`, read by `tui::fleet_state` and
//!   `tui::fleet_status`;
//! * [`types::InspectorContext::env`] — pi's `context.env` (`actions.ts:126`), the gate BOTH
//!   backends' `available()` is. On the context rather than on each plugin, so the dispatcher can
//!   drive the not-installed path.

pub mod plugins;
pub mod types;

pub mod actions;
pub mod runner;
pub mod session_roots_codec;
pub mod shell_command;

pub mod ghostty;
pub mod herdr;

pub use plugins::{GhosttyRunner, HerdrClient, InspectorPlugin, builtin_inspector_plugins};
pub use types::{
    CloseDisposition, CommandOutput, HerdrErrorCode, HerdrFocusErrorCode, HerdrInspectorBinding,
    HerdrInspectorKind, HerdrProjectPaneSnapshot, INSPECTOR_HEADER_PREFIX, INSPECTOR_SUBCOMMAND,
    InspectorAction, InspectorContext, InspectorLaunch, InspectorParams, InspectorTarget,
    OpenDisposition, PaneOwnership, Platform, ProjectPaneAction, ProjectPaneErrorCode,
    ProjectPaneSnapshots, ProjectPaneState, SchemaVersion1, inspector_header_line,
};
