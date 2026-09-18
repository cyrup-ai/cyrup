//! SCOPE_10 — the bounded, machine-readable JSON snapshot of the CURRENT session's async runs: the
//! document a host widget renders instead of re-parsing the human status report.
//!
//! Ports pi `runs/background/async-status-snapshot.ts` (48 LOC, `@7fe9dee1`) — which is a
//! RE-EXPORT SHIM: its `buildAsyncStatusSnapshot` is one line delegating to
//! `projectAsyncStatusSnapshot` in `runs/shared/async-status-projection.ts` (565 LOC). The real
//! port is that projection's reachable closure plus this facade, and the split below mirrors it.
//!
//! # File layout — one file, one concern
//!
//! ```text
//! mod.rs     facade: the three wire constants, the two job-list entry points, and this narrative
//! types.rs   the wire shapes + `resolve_caps` + the four `public*` bounding helpers
//! project.rs `project_async_status_snapshot` and every projector, including the byte-budget search
//! state.rs   the `:31`/`:34` session filters — the ONLY session-aware file in the module
//! ```
//!
//! # The three constants are WIRE constants, and the rebrand does not reach them
//!
//! [`ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX`] introduces the line; [`ASYNC_STATUS_SNAPSHOT_KIND`] is
//! serialized INSIDE the JSON that prefix introduces; [`ASYNC_STATUS_SNAPSHOT_VERSION`] tells a
//! reader which shape it is looking at. All three are keyed on by readers outside this crate, so
//! all three keep their upstream spelling verbatim — the identical decision, for the identical
//! reason, that [`crate::background::inspect_rpc`] records for `INSPECT_REPLY_KIND` /
//! `INSPECT_WIDGET_PREFIX`.
//!
//! # Where this module is reached from in production
//!
//! Upstream has exactly two call sites, and cyrup now has both:
//!
//! * **The subagent RPC bridge's `status` method** — pi
//!   `buildAsyncStatusSnapshotForState(options.state, sessionId)` at `extension/rpc.ts:729` (the
//!   in-memory tier) and `:753` (the executor tier) @v0.68.0. cyrup's port calls
//!   [`state::build_async_status_snapshot_for_state`] from
//!   `extension/rpc/mod.rs`'s `SubagentRpcBridge::fleet_and_snapshot`, on BOTH tiers exactly as
//!   upstream calls it on both, so every `status` reply that crosses the inter-extension bus
//!   carries an `asyncSnapshot` block. See [`crate::extension::rpc`] for the client contract.
//! * **The RPC-mode widget slot** — pi
//!   `ctx.ui.setWidget(WIDGET_KEY, encodeAsyncStatusSnapshotWidget(jobs))` (`tui/render.ts:3000`),
//!   reached when `ctx.mode === "rpc"`. cyrup's port calls
//!   [`encode_async_status_snapshot_widget`] from
//!   `SubagentsExtension::publish_async_status_snapshot_widget`
//!   (`extension/host/slash.rs`), driven by the same `SessionStart`/`AgentEnd`/`SessionShutdown`
//!   edges that drive the human fleet-status widget. In `ExtMode::Rpc` the reader of that slot is
//!   a machine, so it gets this document rather than ASCII columns.
//!
//! Both paths run the jobs through [`state::async_status_snapshot_jobs_for_state`] first, so the
//! `:31` session gate applies to both: a state that does not belong to the live session yields an
//! EMPTY snapshot, never an unfiltered one.
//!
//! # `[CYRUP-DELTA]`s, each also recorded at its own seam
//!
//! * **`state.fleetJobs` does not exist** — [`state::async_status_snapshot_jobs_for_state`].
//! * **`job.agents`, `step.label`, `step.children`, `NestedRunSummary.children` and run-level
//!   `currentToolStartedAt` have no source** — [`project`]'s module doc and the individual seams.
//! * **`job.hostSteps` is STORED rather than derived from the workflow graph** —
//!   [`project::project_host_step`]'s own input seam, `host_steps_for_run`.
//! * **`projectAsyncWorkflowRows` and its helpers are NOT ported** (`:102-123`, `:387-543`): they
//!   are not reachable from `projectAsyncStatusSnapshot`, which is this task's entry point.
//!   `workflowGraphStepStatus` (`:449-458`) is the one exception, reachable via
//!   [`project::project_host_step`]'s sibling `projectWorkflowGraphNode`, and is ported.

pub mod project;
pub mod state;
pub mod types;

pub use project::{host_step_snapshot_state, project_async_status_snapshot, project_host_step};
pub use state::{async_status_snapshot_jobs_for_state, build_async_status_snapshot_for_state};
pub use types::{
    AsyncStatusSnapshot, AsyncStatusSnapshotActivity, AsyncStatusSnapshotCaps,
    AsyncStatusSnapshotHostStep, AsyncStatusSnapshotKind, AsyncStatusSnapshotNode,
    AsyncStatusSnapshotOmitted, AsyncStatusSnapshotOptions, AsyncStatusSnapshotState, resolve_caps,
};

use crate::tui::fleet_state::AsyncRunView;

/// pi `WIDGET_KEY` (`shared/types.ts:2789` @v0.68.0) — the host widget slot the ASYNC-JOBS
/// document is published into, and the ONLY slot `renderWidget` (`tui/render.ts:2991-3008`) ever
/// writes to or clears.
///
/// It is deliberately NOT [`crate::tui::fleet_status::FLEET_STATUS_WIDGET_KEY`]
/// (`"subagent-fleet-status"`, `tui/fleet-status.ts:14`). Upstream runs the two widgets side by
/// side under two distinct keys, and [`cyrup_ext::host::HostServices::set_widget`] takes the key
/// as its first argument (`cyrup-ext/src/host/services.rs:376`), so nothing forces them to share
/// a slot here either. Publishing the machine document into the fleet-status slot would REPLACE
/// the always-on human widget, and clear it outright whenever the async roster is empty — a
/// default-on regression to a shipped feature for every `--acp`/`--rpc` client.
pub const ASYNC_STATUS_SNAPSHOT_WIDGET_KEY: &str = "subagent-async";
/// pi `ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX` (`async-status-snapshot.ts:24`).
pub const ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX: &str = "PI_SUBAGENT_ASYNC_JSON:";
/// pi `ASYNC_STATUS_SNAPSHOT_KIND` (`async-status-projection.ts:8`).
pub const ASYNC_STATUS_SNAPSHOT_KIND: &str = "pi-subagents.async-status-snapshot";
/// pi `ASYNC_STATUS_SNAPSHOT_VERSION` (`async-status-projection.ts:9`).
pub const ASYNC_STATUS_SNAPSHOT_VERSION: u32 = 1;

/// pi `buildAsyncStatusSnapshot` (`async-status-snapshot.ts:26-28`) — the one-line delegation the
/// shim exists to provide, kept as its own name because it is the name upstream's callers use.
#[must_use]
pub fn build_async_status_snapshot<'a, I>(
    jobs: I,
    options: &AsyncStatusSnapshotOptions,
) -> AsyncStatusSnapshot
where
    I: IntoIterator<Item = &'a AsyncRunView>,
{
    project_async_status_snapshot(jobs, options)
}

/// pi `encodeAsyncStatusSnapshotWidget` (`async-status-snapshot.ts:46-48`) — the snapshot as the
/// single prefixed line a host widget slot carries.
///
/// A `Vec<String>` of exactly one element, matching upstream's `string[]`:
/// [`cyrup_ext::host::HostServices::set_widget`] takes a line array, exactly as pi's `setWidget`
/// does, so the caller passes this straight through (see this module's own note on that caller).
///
/// A serialization failure yields the prefix with an EMPTY body rather than a panic
/// (`lib.rs:19-24` denies `unwrap`/`expect`/`panic` crate-wide) — it cannot happen for
/// [`AsyncStatusSnapshot`], and an empty body is the answer a widget can render.
#[must_use]
pub fn encode_async_status_snapshot_widget<'a, I>(
    jobs: I,
    options: &AsyncStatusSnapshotOptions,
) -> Vec<String>
where
    I: IntoIterator<Item = &'a AsyncRunView>,
{
    let snapshot = build_async_status_snapshot(jobs, options);
    let json = serde_json::to_string(&snapshot).unwrap_or_default();
    vec![format!("{ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX}{json}")]
}

/// Shared fixtures for this module's own tests: the one place an [`AsyncRunView`] is built, so the
/// three test modules below cannot drift about what a run looks like.
#[cfg(test)]
pub(crate) mod testfixtures {
    use super::AsyncRunView;
    use crate::background::{RunId, RunMode, RunPaths, RunState, RunStatus};
    use crate::identity::SessionId;

    /// One tracked background run, owned by `session` and last updated at `last_update`.
    pub(crate) fn run_view(run_id: &str, session: Option<&str>, last_update: i64) -> AsyncRunView {
        let id = RunId::from_token(run_id.to_string());
        let mut status = RunStatus::queued(id.clone(), RunMode::Single, None);
        status.state = RunState::Running;
        status.session_id = SessionId::parse_opt(session);
        status.started_at = 1;
        status.last_update = last_update;
        AsyncRunView {
            paths: RunPaths::for_run(
                std::path::Path::new("/nonexistent/async"),
                std::path::Path::new("/nonexistent/results"),
                &id,
            ),
            session_id: session.map(str::to_string),
            status,
            description: None,
            context: None,
            nested_children: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::testfixtures::run_view;
    use super::*;

    /// pi `ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX` (`async-status-snapshot.ts:24`). A WIRE constant:
    /// the rebrand does not reach it, because a reader outside this repo keys on it to find the
    /// line. Changing it silently breaks every such reader.
    #[test]
    fn the_snapshot_widget_prefix_is_unchanged() {
        assert_eq!(
            ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX,
            "PI_SUBAGENT_ASYNC_JSON:"
        );
        let line =
            &encode_async_status_snapshot_widget([].iter(), &AsyncStatusSnapshotOptions::default())
                [0];
        assert!(
            line.starts_with("PI_SUBAGENT_ASYNC_JSON:{"),
            "the encoded widget line is the prefix immediately followed by the document: {line}"
        );
    }

    /// The same wire argument for the two constants that live INSIDE the JSON the prefix
    /// introduces (`async-status-projection.ts:8-9`) — `kind` is how a reader recognises the
    /// document and `version` is how it decides whether it can parse it.
    #[test]
    fn the_snapshot_kind_and_version_are_unchanged() {
        assert_eq!(
            ASYNC_STATUS_SNAPSHOT_KIND,
            "pi-subagents.async-status-snapshot"
        );
        assert_eq!(ASYNC_STATUS_SNAPSHOT_VERSION, 1);
        let jobs = [run_view("run0aaa00000", Some("session-a"), 10)];
        let snapshot =
            build_async_status_snapshot(jobs.iter(), &AsyncStatusSnapshotOptions::default());
        assert_eq!(snapshot.kind, "pi-subagents.async-status-snapshot");
        assert_eq!(snapshot.version, 1);
        let json = serde_json::to_string(&snapshot).expect("serialize");
        assert!(
            json.contains(r#""kind":"pi-subagents.async-status-snapshot""#),
            "{json}"
        );
        assert!(json.contains(r#""version":1"#), "{json}");
    }
}
