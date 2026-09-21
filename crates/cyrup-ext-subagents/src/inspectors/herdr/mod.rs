//! The herdr inspector backend, over `cyrup_herdr` — the workspace's one herdr client.
//!
//! Upstream: `src/inspectors/herdr/` @v0.68.0 — `plugin.ts` (20), `client.ts` (130),
//! `actions.ts` (155), `focus.ts` (55), `project-panes.ts` (730).
//!
//! # The five modules, and the line between them
//!
//! | module | upstream | what it owns |
//! |---|---|---|
//! | [`client`] | `client.ts` | the argv → [`cyrup_herdr`] adapter, the version gate, the error pair |
//! | [`focus`] | `focus.ts` | the pane record probes, and ONE `pane focus` call |
//! | [`actions`] | `actions.ts` | the inspector binding and `inspector.{open,status,close}` |
//! | [`plugin`] | `plugin.ts` | the [`InspectorPlugin`](crate::inspectors::plugins::InspectorPlugin) impl and the env gate |
//! | [`project_panes`] | `project-panes.ts` | the project-pane manager, both compatibility modes, the root index and the session-start restore |
//!
//! # There is exactly one herdr client in this workspace, and it is not here
//!
//! [`client`] opens no socket and spawns no process. Every herdr operation in this subtree ends at
//! a typed [`cyrup_herdr::HerdrClient`] method (`crates/cyrup-herdr/src/client.rs:93-532`).
//! `inspectors/plugins.rs:50-51` states the rule and this module obeys it.
//!
//! # What works with no herdr installed
//!
//! `inspector.status`, `inspector.close`, `project.status` and `project.close` all answer, as
//! plain non-errors, with **zero** herdr calls — their answers come from the binding files.
//! `inspector.open` and `project.open` refuse with upstream's own install sentence. The gate is a
//! pure environment read ([`plugin`]), so nothing can hang: there is no binary to time out on.

pub mod actions;
pub mod client;
pub mod focus;
pub mod plugin;
pub mod project_panes;

pub use actions::{
    binding_path, close_herdr_inspector, open_herdr_inspector, read_herdr_inspector_binding,
    read_herdr_inspector_binding_for_target, status_herdr_inspector,
};
pub use client::{
    DetectedHerdr, HerdrFailure, HerdrVersion, SocketHerdrClient, detect_herdr,
    inspector_error_text, parse_herdr_version, supports_raw_panes,
};
pub use focus::{FocusFailure, FocusedPane, focus_herdr_pane, herdr_pane_focus_target, pane_id_of};
pub use plugin::HerdrInspectorPlugin;
// `HerdrProjectPaneSnapshot` and `ProjectPaneSnapshots` are NOT re-exported here: they moved to
// `crate::inspectors::types`, the frozen contract, because four modules outside this one hold
// them. Re-exporting them would leave two public paths to one type and put this module back in
// the middle of a seam it does not own.
pub use project_panes::{
    CloseProjectPaneData, HerdrProjectPaneBinding, OpenProjectPaneData, PROJECT_PANE_TRUST_STATUS,
    PROJECT_PANES_API_VERSION, ProjectPaneDeps, ProjectPaneFailure, ProjectPaneManager,
    ProjectPaneParams, ProjectPaneRuntime, ProjectPaneStatusData, ToolCompatibility,
    handle_herdr_project_pane_action, list_herdr_project_pane_roots, open_project_pane_count,
    project_pane_binding_path, read_herdr_project_pane_binding, read_project_pane_binding,
    restore_herdr_project_pane_snapshots,
};
