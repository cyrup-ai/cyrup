//! Per-child foreground control state (WORKFLOW_6 §1.2): [`ForegroundChildEntry`] plus the
//! sync/begin/update trio that keeps a [`ForegroundControlEntry`]'s "current child" view derived
//! from its `active_children` map instead of hand-written at each call site. pi's fourth member
//! of this quartet, `finishForegroundChild` (`foreground-control.ts:147-157`), is deliberately NOT
//! ported here: a cyrup foreground SINGLE run's one child is retired by dropping the WHOLE
//! [`ForegroundControlEntry`] (`SubagentExecutor::settle_foreground_run`), never by finishing one
//! child of a still-live multi-child entry, so it would have no caller until a future task lands
//! multi-child foreground steering. Port it verbatim from pi alongside that task's first call
//! site rather than shipping it as dead code ahead of one.
//!
//! Port of [`runs/foreground/foreground-control.ts`
//! (`:39-157`)](../../../../../../workspace/pi-subagents/src/runs/foreground/foreground-control.ts),
//! reduced to the fields cyrup's control entry actually carries.

use crate::extension::executor::notices::ForegroundControlEntry;

/// How to reach ONE live foreground workflow child — pi `ForegroundChildControl.steer`
/// (`shared/types.ts:2163`).
///
/// # Upstream is a closure; this is its Rust equivalent, and the difference is forced
///
/// pi's field is a FUNCTION — `steer?: (input) => Promise<ForegroundSteerOutcome>` — built at
/// `subagent-executor.ts:3974` over `childSessionControls`, the live IN-PROCESS child session
/// object (hence its doc: *"Steer the live in-process child session; undefined until the session
/// exists"*). Upstream simply calls `childSessionControls.steer(message)` and gets an authoritative
/// outcome back. Its foreground path touches no run directory whatsoever — grep
/// `steerInboxDir|steerAcksDir|enqueueStepSteer` across `src/runs/foreground/` and there is not one
/// hit.
///
/// cyrup cannot port that literally: a cyrup foreground child is a REAL spawned OS process
/// (`run_sync` -> `SpawnedChild::spawn`), not an in-process session object, so there is no callable
/// to capture. The child's own steering channel is the directory it watches
/// ([`crate::prompt_runtime::SteeringInbox`], attached to `RunOptions::steer_inbox_dir`), and the
/// parent's direct line to it is writing into that very directory. So this type carries the same
/// SEMANTIC content as upstream's closure — "how to reach this child" — as data plus
/// [`Self::deliver`], which is the idiomatic Rust spelling of a captured-environment callback and
/// keeps [`ForegroundChildEntry`] a plain, cloneable, testable value.
///
/// # Why `inbox_dir` is stored RESOLVED and never re-derived
///
/// The one invariant that matters is *the parent writes where the child reads*. `inbox_dir` is the
/// exact `PathBuf` handed to [`crate::exec::RunOptions::steer_inbox_dir`] for this child, so that
/// invariant holds BY CONSTRUCTION. Storing `(run_dir, index)` and re-deriving the path at write
/// time would make it hold only as long as two independent derivations agree — a second copy of the
/// layout convention [`crate::background::control::step_steer_inbox_dir`] exists to be the single
/// source of. That is not hypothetical: it is exactly how an earlier revision of this handle came to
/// address the runner's `steer-requests/` intake queue (drained only by the background runner's
/// watch loop, which a foreground workflow does not have) instead of the child's own
/// `steer-targets/<index>/`, producing a steer that compiled, reported `pending`, and delivered
/// nothing.
///
/// `pub`, not `pub(crate)`, because it is a field type of
/// [`crate::extension::executor::requests::ForegroundRunRequest`], which `extension/mod.rs`
/// re-exports publicly — a `pub` field of a `pub(crate)` type is a private interface.
#[derive(Clone, Debug)]
pub struct ForegroundChildSteerHandle {
    /// THE path this child's [`crate::prompt_runtime::SteeringInbox`] is watching — the same value
    /// `build_foreground_run_options` put on [`crate::exec::RunOptions::steer_inbox_dir`], carried
    /// rather than recomputed. See the type doc for why that distinction is load-bearing.
    pub inbox_dir: std::path::PathBuf,
    /// The WORKFLOW's run dir — never the child's. Only the ACK half needs it:
    /// [`crate::background::control::take_steer_acks`] scans `<run_dir>/control/steer-acks/*` and
    /// the caller filters by index, which is a run-dir-scoped read by design and is shared verbatim
    /// with the async path.
    pub run_dir: std::path::PathBuf,
    /// This child's flat index WITHIN THE WORKFLOW — pinned onto the request as `target_index` and
    /// used to narrow the ack read.
    ///
    /// ⚠ NOT the key this child occupies in [`ForegroundControlEntry::active_children`]. That key is
    /// the child's index within its OWN foreground run, which is always `0` for the single-child
    /// foreground run a workflow child is. Two namespaces; carrying this one inside the handle is
    /// what keeps them from being conflated.
    pub index: usize,
}

impl ForegroundChildSteerHandle {
    /// Deliver one steer request straight into this child's own inbox, returning the minted request
    /// id for the caller to await an acknowledgment against.
    ///
    /// This is pi's `await child.steer({message, mode})` (`workflow-foreground-steering.ts:149`) —
    /// the DIRECT line to the child, not a queue someone else has to drain. `target_index` is pinned
    /// exactly as [`crate::background::control::enqueue_step_steer`] pins it, because a request
    /// sitting in a specific child's inbox is already addressed and the field is what the child-side
    /// reader validates against.
    ///
    /// ⚠ Deliberately NOT [`crate::background::control::request_async_steer_with_mode`]. That
    /// function writes to `<run_dir>/control/steer-requests/`, the BACKGROUND RUNNER's intake queue,
    /// which is drained only by `runner_main::control_watcher::route_steer_requests`. A foreground
    /// workflow runs in this process and has no such loop, so a request written there is never
    /// routed anywhere and the child never sees it.
    ///
    /// # Errors
    ///
    /// [`crate::error::SubagentError::Management`] if `message` is blank (upstream trims and
    /// rejects empty at the same boundary), or [`crate::error::SubagentError::Spawn`] if the inbox
    /// cannot be created or written.
    pub(crate) async fn deliver(
        &self,
        message: &str,
        mode: Option<crate::background::control::SteerDeliveryMode>,
        source: &str,
    ) -> Result<String, crate::error::SubagentError> {
        use crate::background::control;

        let message = message.trim();
        if message.is_empty() {
            return Err(crate::error::SubagentError::Management(
                "steer message must not be empty.".to_string(),
            ));
        }
        let request = control::SteerRequest {
            kind: "steer".to_string(),
            id: control::next_steer_request_id(),
            ts: crate::time::now_epoch_millis(),
            message: message.to_string(),
            // `Steer` is the default and is normalised OFF the wire, matching upstream's own
            // conditional spread (`:149`'s `...(mode && mode !== "steer" ? { mode } : {})`).
            mode: mode.filter(|m| *m != control::SteerDeliveryMode::Steer),
            target_index: Some(self.index),
            source: Some(source.to_string()),
        };
        let id = request.id.clone();
        control::write_steer_request_to_dir(&self.inbox_dir, &request).await?;
        Ok(id)
    }
}

/// One live child of a foreground run — pi `ForegroundChildControl`
/// (`shared/types.ts:2102-2128`), narrowed to the telemetry cyrup's control events actually raise.
#[derive(Clone)]
pub(crate) struct ForegroundChildEntry {
    /// pi `index` — the flat child index; every reader sorts on it.
    pub(crate) index: usize,
    /// pi `agent`.
    pub(crate) agent: String,
    /// pi `sessionName`.
    pub(crate) session_name: Option<String>,
    /// pi `description` — the caller's task text.
    pub(crate) description: Option<String>,
    /// pi `startedAt` / `updatedAt`.
    pub(crate) started_at: i64,
    pub(crate) updated_at: i64,
    /// pi `currentActivityState` / `currentTool` / `currentPath` / `turnCount` / `toolCount` /
    /// `tokens` — the same six the control-event sink already folds onto the parent entry
    /// (`notices.rs:312-345`).
    pub(crate) current_activity_state: Option<crate::background::ActivityState>,
    pub(crate) current_tool: Option<String>,
    pub(crate) current_path: Option<String>,
    pub(crate) turn_count: Option<u64>,
    pub(crate) tool_count: Option<u64>,
    pub(crate) tokens: Option<u64>,
    /// pi `interrupt` — the child's own soft-interrupt handle. The SAME `CancelToken` the parent
    /// entry holds while there is one child; distinct once WORKFLOW_7 lands per-child steering.
    pub(crate) interrupt: cyrup_core::CancelToken,
    /// pi `ForegroundChildControl.steer` (`shared/types.ts`), OPTIONAL upstream and optional here —
    /// a plain foreground single-run child still has none (`foreground.rs:877-879`'s G90 note holds
    /// for it, and only for it).
    ///
    /// `Some` exactly when this child is a WORKFLOW child: the workflow owns a run directory
    /// (WORKFLOW_13), so `step_steer_inbox_dir`/`steer_acks_dir` have somewhere to be rooted. The
    /// handle is the run dir plus this child's flat index, not a channel — delivery is the same
    /// file-drop-then-await-ack the async path uses, and duplicating it as an in-process channel
    /// would give the crate two steer semantics for one verb.
    pub(crate) steer: Option<ForegroundChildSteerHandle>,
}

/// pi `syncCurrentChild` (`foreground-control.ts:39-60`): the live child's state IS the run's
/// state. Every field below is a verbatim line of upstream's assignment list, minus the ones
/// cyrup's entry does not carry (`inputTokens`/`outputTokens`/`window`/`windowPeak`/`model`/
/// `thinking`/`lastActivityAt`/`currentToolStartedAt`/`detach`).
fn sync_current_child(entry: &mut ForegroundControlEntry, child: &ForegroundChildEntry) {
    entry.current_agent = Some(child.agent.clone());
    entry.session_name = child.session_name.clone();
    entry.current_index = Some(child.index);
    entry.description = child.description.clone();
    entry.current_activity_state = child.current_activity_state;
    entry.current_tool = child.current_tool.clone();
    entry.current_path = child.current_path.clone();
    entry.turn_count = child.turn_count;
    entry.tool_count = child.tool_count;
    entry.tokens = child.tokens;
    entry.interrupt = child.interrupt.clone();
}

/// pi `beginForegroundChild` (`:101-137`): insert the child, then derive the entry's "current
/// child" view from it — never hand-write `current_agent`/`current_index`/`description` at the
/// call site.
pub(crate) fn begin_foreground_child(
    entry: &mut ForegroundControlEntry,
    child: ForegroundChildEntry,
) {
    let index = child.index;
    entry.active_children.insert(index, child);
    // `expect`/`unwrap` are DENY workspace-wide (`Cargo.toml:101-104`) — re-read through the map.
    if let Some(child) = entry.active_children.get(&index).cloned() {
        sync_current_child(entry, &child);
    }
}

/// pi `updateForegroundChild` (`:139-145`) — write onto the CHILD, then sync up. This replaces the
/// direct entry writes [`crate::extension::executor::notices`]'s control-event sink used to make;
/// the observable result is identical while there is one child, and correct once there is more
/// than one.
///
/// Preserves `notices.rs`'s "`None` never clobbers" rule verbatim (a control event reports what it
/// observed, not a full snapshot) and its `entry.updated_at` bump — the FleetView's newest-first
/// sort key — which is distinct from the CHILD's own `updated_at`.
pub(crate) fn update_foreground_child(
    entry: &mut ForegroundControlEntry,
    index: usize,
    event: &crate::exec::control::ControlEvent,
) {
    let now = crate::time::now_epoch_millis();
    if let Some(child) = entry.active_children.get_mut(&index) {
        child.current_activity_state = Some(event.to);
        if event.current_tool.is_some() {
            child.current_tool = event.current_tool.clone();
        }
        if event.current_path.is_some() {
            child.current_path = event.current_path.clone();
        }
        if let Some(turns) = event.turns {
            child.turn_count = Some(turns);
        }
        if let Some(tools) = event.tool_count {
            child.tool_count = Some(u64::from(tools));
        }
        if let Some(tokens) = event.tokens {
            child.tokens = Some(tokens);
        }
        child.updated_at = now;
    }
    if let Some(child) = entry.active_children.get(&index).cloned() {
        sync_current_child(entry, &child);
    }
    entry.updated_at = now;
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use cyrup_core::CancelToken;

    fn base_entry() -> ForegroundControlEntry {
        ForegroundControlEntry {
            interrupt: CancelToken::new(),
            current_agent: None,
            current_index: None,
            current_activity_state: None,
            mode: crate::background::RunMode::Single,
            description: None,
            current_tool: None,
            current_path: None,
            turn_count: None,
            tool_count: None,
            tokens: None,
            started_at: 0,
            updated_at: 0,
            session_id: None,
            parent_workflow_run_id: None,
            workflow_key: None,
            cwd: None,
            session_name: None,
            active_children: std::collections::BTreeMap::new(),
        }
    }

    fn child(index: usize, agent: &str, updated_at: i64) -> ForegroundChildEntry {
        ForegroundChildEntry {
            index,
            agent: agent.to_string(),
            session_name: None,
            description: Some(format!("task-{index}")),
            started_at: updated_at,
            updated_at,
            current_activity_state: None,
            current_tool: None,
            current_path: None,
            turn_count: None,
            tool_count: None,
            tokens: None,
            interrupt: CancelToken::new(),
            // A plain (non-workflow) foreground child: no run directory, so no handle.
            steer: None,
        }
    }

    /// `begin_foreground_child` must derive `current_agent`/`current_index`/`description` from the
    /// map it just inserted into — never leave them at whatever the caller happened not to set.
    #[test]
    fn begin_foreground_child_derives_the_current_child_view_from_the_map() {
        let mut entry = base_entry();
        entry.interrupt = CancelToken::new();
        begin_foreground_child(&mut entry, child(0, "scout", 100));

        assert_eq!(entry.current_agent.as_deref(), Some("scout"));
        assert_eq!(entry.current_index, Some(0));
        assert_eq!(entry.description.as_deref(), Some("task-0"));
        assert!(entry.active_children.contains_key(&0));
    }

    fn event_for(
        run_id: &str,
        index: u32,
        current_tool: Option<&str>,
        turns: Option<u64>,
    ) -> crate::exec::control::ControlEvent {
        crate::exec::control::build_control_event(
            crate::background::ActivityState::NeedsAttention,
            crate::exec::control::ControlEventInput {
                event_type: Some(crate::registration::ControlEventType::NeedsAttention),
                ts: 1_700_000_000_000,
                run_id: run_id.to_string(),
                agent: "scout".to_string(),
                index: Some(index),
                current_tool: current_tool.map(str::to_string),
                turns,
                ..Default::default()
            },
        )
    }

    /// `update_foreground_child` folds telemetry onto the CHILD and syncs the entry from it, and a
    /// `None` field on a later event must not clobber a value an earlier event supplied.
    #[test]
    fn update_foreground_child_folds_onto_the_child_without_clobbering_and_bumps_updated_at() {
        let mut entry = base_entry();
        begin_foreground_child(&mut entry, child(0, "scout", 100));
        let entry_updated_at_at_begin = entry.updated_at;

        let first = event_for("run-1", 0, Some("bash"), Some(3));
        update_foreground_child(&mut entry, 0, &first);
        assert_eq!(entry.current_tool.as_deref(), Some("bash"));
        assert_eq!(entry.turn_count, Some(3));
        assert!(
            entry.updated_at >= entry_updated_at_at_begin,
            "the ENTRY's own updated_at must be bumped on every event"
        );

        // A second event with `current_tool: None` must not erase the first event's value.
        let second = event_for("run-1", 0, None, None);
        update_foreground_child(&mut entry, 0, &second);
        assert_eq!(
            entry.current_tool.as_deref(),
            Some("bash"),
            "None on a later event must not clobber a value a previous event supplied"
        );
        assert_eq!(
            entry.turn_count,
            Some(3),
            "None on a later event must not clobber turn_count either"
        );
        assert_eq!(
            entry
                .active_children
                .get(&0)
                .map(|c| c.current_tool.clone()),
            Some(Some("bash".to_string())),
            "the CHILD's own record must carry the same value the entry was synced from"
        );
    }
}
