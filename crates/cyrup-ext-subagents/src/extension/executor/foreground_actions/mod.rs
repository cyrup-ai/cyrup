//! The three ASYNC-run control-ACTION verbs pi implements one-file-each under
//! `src/runs/foreground/*.ts`: `stop`, `steer`, `dismiss` (WORKFLOW_8). Each verb is an inherent
//! `impl SubagentExecutor` method living in its own file — [`stop`], [`steer`], [`dismiss`] — moved
//! here verbatim out of `extension::executor::control`, which keeps only `interrupt`, `resume` and
//! `append_step`.
//!
//! # `foreground_actions` names pi's SOURCE directory, not a claim about the runs it acts on
//!
//! pi's own tree calls this directory `runs/foreground/` because it holds the FOREGROUND
//! (synchronous, in-process) half of each action's round trip — the part that validates a request
//! and hands it to the async store. It is **not** a claim that the runs these actions ACT ON are
//! themselves foreground: `control_stop`, `control_steer` and `control_dismiss` are every one of
//! them gated on an **async** run having a live directory on disk (`asyncDir`/`resolve_run_id`),
//! and refuse a live FOREGROUND child with their own distinct sentence
//! (`STOP_FOREGROUND_RUN_REFUSAL` / `STEER_FOREGROUND_RUN_REFUSAL`) rather than silently treating
//! it as a missing async run. Steering a live **foreground** child is WORKFLOW_14, created
//! precisely because this directory's name made that gap look already owned — do not close it
//! here; see [`steer`]'s own module doc for the fuller account.
//!
//! # `background::control`'s primitives stay put
//!
//! `background/control.rs`'s [`crate::background::control::stop`],
//! [`crate::background::control::interrupt`], [`crate::background::control::resume`],
//! [`crate::background::control::request_async_stop`],
//! [`crate::background::control::deliver_stop_request`],
//! [`crate::background::control::request_async_steer_with_mode`] and
//! [`crate::background::control::take_steer_acks`] are the control **channel**: the on-disk request
//! shapes, the R-SA-079 reconciliation gate, and the session-membership gate every verb applies
//! before touching disk. This module is the **action** layer one level up — it shapes each
//! primitive's outcome into the exact user-facing sentence pi's own action file returns. Moving a
//! primitive here would break `background`'s own internal callers (`runner_main`, cascade,
//! `child_stop`), which never touch this module at all.
//!
//! # The steering-recovery subsystem is not here
//!
//! See [`steer`]'s own module doc for the full account: `async-steering-action.ts`'s bulk
//! (`:88-250`) is a steering-recovery subsystem this crate has not ported and does not port here
//! either — cyrup's already-landed SUBA-049 ack-file design is a different, smaller mechanism
//! covering the same observable need (knowing whether a queued steer actually landed).

pub(crate) mod dismiss;
pub(crate) mod steer;
pub(crate) mod stop;

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use crate::background::atomic::write_atomic_json;
    use crate::background::control::{self, InterruptOutcome};
    use crate::background::{RunId, RunMode, RunPaths, RunState, RunStatus};

    /// The same S4 gate `stop` applies, on `interrupt`, and the same zero-trace requirement.
    /// Cross-verb by nature (`interrupt`'s own action, `control_interrupt`, stays in
    /// `extension::executor::control` because `interrupt` is not one of this directory's three
    /// verbs) — it belongs here, with the zero-trace family it is one third of, and keeps calling
    /// the [`crate::background::control::interrupt`] primitive directly, exactly as it did before
    /// the move.
    #[tokio::test]
    async fn interrupt_refuses_a_run_owned_by_another_session_and_writes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let async_root = dir.path().join("async");
        let results_dir = dir.path().join("results");
        let run_id = RunId::from_token("foreignintr1");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(&paths.run_dir)
            .await
            .expect("mkdir");

        let mut status =
            RunStatus::queued(run_id.clone(), RunMode::Single, Some(std::process::id()));
        status.state = RunState::Running;
        status.session_id = crate::identity::SessionId::parse("session-OWNER");
        write_atomic_json(&paths.status, &status)
            .await
            .expect("write status");

        let intruder = crate::identity::SessionId::parse("session-INTRUDER");
        let outcome = control::interrupt(
            &async_root,
            &results_dir,
            run_id.as_str(),
            "interrupt-action",
            None,
            intruder.as_ref(),
        )
        .await
        .expect("interrupt resolves");

        assert_eq!(outcome, InterruptOutcome::NotInActiveSession);
        assert!(
            !tokio::fs::try_exists(&paths.control_inbox)
                .await
                .expect("check"),
            "a refused interrupt must not write into another session's control inbox"
        );
    }
}
