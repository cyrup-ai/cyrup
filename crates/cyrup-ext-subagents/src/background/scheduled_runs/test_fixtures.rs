//! Shared record builders for this module's three test suites.
//!
//! One builder, not three: `store.rs`, `schedule.rs` and `ceiling_gate.rs` all need a
//! fully-populated [`ScheduleRecord`], and three copies of it would drift the moment a field is
//! added — which is exactly the drift the round-trip tests exist to catch.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;

use super::schedule::{
    ScheduleCatchUp, ScheduleDueReason, ScheduleId, ScheduleOverlapSkip, ScheduleRecord,
    ScheduleRunId, ScheduleRunRecord, ScheduleRunState, ScheduleTarget, ScheduleTrigger,
    ScheduleVersion,
};

/// A record with EVERY optional field populated — the shape part B will actually write.
///
/// A round-trip fixture that left `timeout_ms`, `session_only`, `owner_session_file`,
/// `active_run_id`, `last_run_id` or `target.base_ref` unset would not be testing the format at
/// all; it would be testing the subset this task happens to construct.
pub(super) fn full_record(id: &str, cwd: &Path) -> ScheduleRecord {
    let mut args = serde_json::Map::new();
    args.insert("task".to_string(), serde_json::Value::from("nightly sweep"));
    args.insert("retries".to_string(), serde_json::Value::from(3));
    ScheduleRecord {
        schema_version: ScheduleVersion,
        id: ScheduleId::parse(id).expect("fixture id is legal"),
        name: "nightly sweep".to_string(),
        cwd: cwd.to_path_buf(),
        trigger: ScheduleTrigger::Interval {
            every: "6h".to_string(),
            every_ms: 21_600_000,
            anchor_at: "2026-09-15T00:00:00.000Z".to_string(),
            next_run_at: "2026-09-15T06:00:00.000Z".to_string(),
        },
        target: ScheduleTarget {
            workflow_script: "return await runs.host('ci');".to_string(),
            args,
            base_ref: Some("refs/heads/main".to_string()),
        },
        overlap: ScheduleOverlapSkip,
        catch_up: ScheduleCatchUp::Latest,
        timeout_ms: Some(900_000),
        paused: false,
        session_only: Some(true),
        quiet: Some(true),
        owner_session_file: Some(cwd.join("session.jsonl")),
        created_at: "2026-09-15T00:00:00.000Z".to_string(),
        updated_at: "2026-09-15T00:00:00.000Z".to_string(),
        active_run_id: Some(ScheduleRunId::mint()),
        last_run_id: Some(ScheduleRunId::mint()),
    }
}

/// One fired run of `schedule_id`, in `state`.
pub(super) fn run_record(
    schedule_id: &ScheduleId,
    run_id: ScheduleRunId,
    state: ScheduleRunState,
) -> ScheduleRunRecord {
    ScheduleRunRecord {
        schema_version: ScheduleVersion,
        id: run_id,
        schedule_id: schedule_id.clone(),
        planned_at: "2026-09-15T06:00:00.000Z".to_string(),
        due_reason: ScheduleDueReason::Timer,
        state,
        started_at: Some("2026-09-15T06:00:00.100Z".to_string()),
        completed_at: None,
        async_id: None,
        async_dir: None,
        error: None,
    }
}
