//! VL-S6 — `inspector.open` as an EXECUTOR verb, for the surfaces that are not the tool.
//!
//! Two things live here, and they are deliberately in the same file:
//!
//! 1. [`SubagentExecutor::inspector_dispatcher_deps`], the ONE production assembly of
//!    [`InspectorDispatcherDeps`]. pi threads a whole `SubagentState` plus module-global `DIRS`
//!    into `handleInspectorAction` (`src/inspectors/actions.ts:106-119` @v0.68.0); cyrup has
//!    neither, so the frozen contract names every field explicitly and this is where production
//!    fills them. Both callers — the `subagent` tool's `inspector.*` arm
//!    ([`crate::extension::SubagentTool::route_action`]) and the fleet overlay's `Enter`/`H` key —
//!    go through it, so the two surfaces cannot disagree about which runs are trusted or which
//!    session roots an inspector pane may read.
//! 2. [`SubagentExecutor::inspector_open`], the `Result<String, String>` shape the fleet overlay
//!    needs.
//!
//! # Why `inspector_open` is shaped like `control_stop` and not like the tool arm
//!
//! `tui/fleet_overlay.rs`'s pending-action arms all feed
//! [`crate::tui::fleet::action_result_from_control`], which takes a `Result<String, String>` — pi's
//! own `firstToolResultText(result, fallback)` (`fleet.ts:324-328`) reduced to the two cases a TUI
//! can render. So this method flattens the dispatcher's `Result<ToolResult, ToolError>` to its
//! text exactly as [`Self::control_stop`] does for its own primitive: `Err(ToolError)` — which is
//! how the frozen contract ports upstream's `isError: true` (`inspectors/types.rs`'s module doc) —
//! becomes `Err(String)`, and an `Ok` result becomes its concatenated text.
//!
//! It does NOT re-implement the verb. Every decision — target resolution, the trusted-directory
//! gate, backend order, the no-backend refusal — stays in
//! [`crate::inspectors::actions::handle_inspector_action`], so the key and the tool answer
//! identically.

use std::path::Path;

use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::paths::{
    default_async_root_in, default_results_dir_in, subagent_session_root, trusted_session_roots,
};
use crate::inspectors::actions::{
    InspectorDispatcherDeps, InspectorRequest, LiveInspectorJob, handle_inspector_action,
};
use crate::inspectors::types::InspectorAction;

impl SubagentExecutor {
    /// pi's `handleInspectorAction` deps (`subagent-executor.ts:6322-6329` @v0.68.0), assembled
    /// from this process's real state.
    ///
    /// `current_session` is upstream's `deps.state.currentSessionId = resolveCurrentSessionId(
    /// ctx.sessionManager)` (`:6321`), which upstream assigns IMMEDIATELY before the call for
    /// exactly this reason: the id must be the live one, not whatever the state was last left
    /// holding. [`Self::current_session_id`] reads the bound host backend on every call, so it is.
    pub(crate) async fn inspector_dispatcher_deps(&self, cwd: &Path) -> InspectorDispatcherDeps {
        let cfg = self.config_snapshot().await;
        let parent_session_file = self
            .host_services()
            .and_then(|services| services.session_file());

        // pi `[...state.asyncJobs.values(), ...state.fleetJobs.values()]` (`actions.ts:38`,
        // `:93`), flattened. cyrup has ONE tracker rather than two maps, so the concatenation
        // upstream performs is already done and the "asyncJobs first" ordering
        // `LiveInspectorJob`'s doc asks for is trivially satisfied.
        //
        // This list is what makes `trusted_dir`'s rung 1 real (`inspectors/actions.rs`'s own
        // doc): a run launched under a NON-default `async_dir_root` stays inspectable because
        // this process is tracking it. `session_root` is per-run, derived from the run's OWN
        // recorded session file by the same rule `trusted_session_roots` applies to the parent's;
        // a run that persisted no session file contributes no root, exactly as upstream's absent
        // `job.sessionRoot` does.
        let live_jobs: Vec<LiveInspectorJob> = self
            .tracker()
            .snapshot()
            .into_iter()
            .map(|job| LiveInspectorJob {
                run_id: job.run_id.as_str().to_string(),
                async_dir: job.paths.run_dir.clone(),
                session_root: job.last_status.as_ref().and_then(|status| {
                    status
                        .session_file
                        .as_deref()
                        .or_else(|| {
                            status
                                .steps
                                .iter()
                                .find_map(|step| step.session_file.as_deref())
                        })
                        .map(subagent_session_root)
                }),
            })
            .collect();

        InspectorDispatcherDeps {
            cwd: cwd.to_path_buf(),
            async_dir_root: default_async_root_in(&cfg.roots, cwd),
            results_dir: default_results_dir_in(&cfg.roots, cwd),
            current_session: crate::identity::SessionId::parse_opt(
                self.current_session_id().as_deref(),
            ),
            live_jobs,
            // SUBA-091 / pi `deps.sessionRoots ?? state.trustedSessionRoots` (`actions.ts:94`) —
            // the SAME two rungs `SubagentExecutor::fleet_state` seeds, from the same helper, so
            // an inspector pane may read exactly the transcripts the fleet detail pane may and
            // not one byte more.
            session_roots: trusted_session_roots(
                cfg.default_session_dir.as_deref(),
                parent_session_file.as_deref(),
            ),
            missions: cfg.missions.clone(),
            // pi has no counterpart: the override exists so tests can point the mission store
            // somewhere other than the real agent dir.
            agent_dir_override: None,
            authority_policy: cfg.authority_policy,
            // pi `plugins: createBuiltinInspectorPlugins()` (`:6327`) — herdr, then ghostty.
            plugins: crate::inspectors::plugins::builtin_inspector_plugins(),
            // pi `env: deps.env ?? process.env` (`actions.ts:126`). Production is the process
            // environment, and this is the ONE place it is read for the whole inspector family —
            // both backends' `available()` gates are a lookup in the map this fills, not a read of
            // the ambient process.
            env: crate::inspectors::actions::process_env(),
        }
    }

    /// VL-S6 — open an inspector pane for one async run, for a caller that renders a plain string.
    ///
    /// The fleet overlay's `Enter`/`H` binding (pi `inspect: ["return", "H"]`, `fleet.ts:45`,
    /// dispatched at `fleet.ts:1417-1430`). `focus` is `true` from that key and it is the point of
    /// the key: the user pressed it to LOOK at this child.
    ///
    /// # Errors
    ///
    /// Whatever the dispatcher refuses with, as its sentence — including
    /// [`crate::inspectors::actions::NO_INSPECTOR_PLUGIN_AVAILABLE`] when no backend's host is
    /// present, which is the answer a stock Linux box gets and the one that points at
    /// `inspector.command`.
    pub async fn inspector_open(
        &self,
        cwd: &Path,
        target: Option<&str>,
        dir: Option<&str>,
        index: Option<usize>,
        focus: bool,
    ) -> Result<String, String> {
        let deps = self.inspector_dispatcher_deps(cwd).await;
        let request = InspectorRequest {
            id: target.map(str::to_string),
            run_id: None,
            dir: dir.map(std::path::PathBuf::from),
            // `usize` here, `i64` on the contract: this caller holds an already-validated child
            // index off a rendered roster row, so there is no negative to preserve. The
            // saturation matches the tool edge's — a value this large lands on the dispatcher's
            // out-of-range refusal rather than silently addressing child 0.
            index: index.map(|raw| i64::try_from(raw).unwrap_or(i64::MAX)),
            focus: Some(focus),
            pane_id: None,
        };
        match handle_inspector_action(InspectorAction::Open, &request, &deps).await {
            Ok(result) => Ok(result
                .content
                .iter()
                .filter_map(|content| match content {
                    cyrup_core::Content::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")),
            Err(error) => Err(error.to_string()),
        }
    }
}
