//! PB-8 — the capability advertisement: pi `src/extension/rpc.ts:431-470` @v0.68.0
//! (`sessionData` and `pingData`).
//!
//! # Every key here is a PROMISE
//!
//! `pingData` is both the `ping` reply and the payload of the one-shot
//! [`super::SUBAGENT_RPC_READY_EVENT`], so it is the document a client integrates against. A key
//! advertised and not implemented is a lie the client will act on, which is why three of
//! upstream's capability keys and one of its event keys are DROPPED here rather than copied —
//! each with the missing seam named. Every key that IS advertised is paid for by a production
//! writer or emitter in this crate, named in the doc on [`ping_data`].

use std::path::Path;

use serde_json::Value;

use crate::background::async_status_snapshot::{
    ASYNC_STATUS_SNAPSHOT_KIND, ASYNC_STATUS_SNAPSHOT_VERSION,
};
use crate::background::watch::SUBAGENT_ASYNC_COMPLETE_EVENT;

use crate::background::watch::SUBAGENT_PROCESS_TERMINAL_EVENT;
use crate::extension::SubagentExecutor;

use super::{
    SUBAGENT_RPC_MANAGEMENT_ACTIONS, SUBAGENT_RPC_METHODS, SUBAGENT_RPC_PROTOCOL_VERSION,
    SUBAGENT_RPC_READY_EVENT, SUBAGENT_RPC_REPLY_EVENT_PREFIX, SUBAGENT_RPC_REQUEST_EVENT,
};

/// pi `sessionData(ctx)` (`rpc.ts:431-438`).
///
/// Upstream's `ctx` is the live `ExtensionContext`; cyrup's equivalent handle is the late-bound
/// capability backend (`SubagentExecutor::host_services`, `extension/executor/mod.rs:437`), which
/// is the SAME object the completion sink and the fork-context resolver read the session identity
/// from. `cwd` comes from the extension's captured `self.cwd` rather than from `ctx`, because
/// `NativeExtension::init` carries no `HostCtx` — see `extension/host/mod.rs`'s field doc.
fn session_data(executor: &SubagentExecutor, cwd: &Path) -> Value {
    let Some(services) = executor.host_services() else {
        // `:432` — no context, no session block at all (an empty object, not nulls).
        return serde_json::json!({});
    };
    serde_json::json!({
        "cwd": cwd.display().to_string(),
        "sessionId": services.session_id(),
        // `:436` — `?? null`: an attached-but-unpersisted session reports an explicit null, which
        // a client can tell apart from "no session block".
        "sessionFile": services
            .session_file()
            .map_or(Value::Null, |p| Value::from(p.display().to_string())),
    })
}

/// pi `pingData(ctx)` (`rpc.ts:440-470`).
///
/// # `[CYRUP-DELTA]` — three capability keys and one event key are DROPPED
///
/// * **`nonRecoveringSteer`** (`:452`) advertises that the RPC forces `steeringRecovery: false`.
///   cyrup's steer has no recovery mode to turn off: [`crate::background::control::SteerDeliveryMode`]
///   is `Steer | FollowUp | Auto`, all three of which DELIVER, and no arm parks a run awaiting an
///   acknowledgement it could later be revived from. So there is nothing to promise — see
///   [`super::params::steer_params`]. (A grep for `steering_recovery` is NOT the evidence: its only
///   hits in this crate are this bullet and that function's delta block, so the grep quotes itself.
///   The three-arm enum is the evidence.)
/// * **`launchResolvedExtensions`** / **`runtimeAcknowledgedExtensions`** (`:456-457`) advertise
///   the child extension-resolution reporting surface and the
///   `subagent:acknowledge-extension` child-runtime event. Neither exists here.
/// * **`events.childStatus`** (`:465`) is `subagent:child-status`, emitted only by upstream's
///   inline `stopAsyncRun`. cyrup routes `stop` through the tool arm instead (see
///   [`super`]'s module doc), so nothing emits it.
///
/// `events.asyncComplete` is KEPT, and that is not free: it is a promise this task pays for by
/// registering [`crate::background::watch::BusAnnouncingCompletionObserver`] in the production
/// completion fan-out. Without that registration this key would have to be dropped too, and a
/// delegating host would be forced to poll `status` in a loop to learn a child had finished.
pub(crate) fn ping_data(executor: &SubagentExecutor, cwd: &Path) -> Value {
    serde_json::json!({
        "version": SUBAGENT_RPC_PROTOCOL_VERSION,
        "methods": SUBAGENT_RPC_METHODS,
        "capabilities": {
            "status": true,
            // `:446` — the two-tier projection this bridge implements: an untargeted `status`
            // answers from live in-memory state when the session lines up, and anything with a
            // target (including `view`/`lines`) goes to the executor.
            "statusProjection": {
                "version": 1,
                "untargeted": "in-memory-when-ready",
                "targeted": "executor",
            },
            "managementActions": SUBAGENT_RPC_MANAGEMENT_ACTIONS,
            "fleetStatus": { "version": 1 },
            "asyncStatusSnapshot": {
                "kind": ASYNC_STATUS_SNAPSHOT_KIND,
                "version": ASYNC_STATUS_SNAPSHOT_VERSION,
            },
            "asyncSpawn": true,
            "steer": true,
            "interrupt": true,
            "stop": true,
            "resume": true,
            // `rpc.ts:458` — the process-terminal lifecycle artifact, stamped with the schema
            // generation its event lines carry (`shared/types.ts:629`). Advertising it is a real
            // promise: [`crate::background::process_terminal`] writes the candidate and the proof
            // on every background launch, and the `events.processTerminal` key below names the
            // event a client can tail to learn a proof landed.
            "processTerminalProof": {
                "version": 1,
                "lifecycleArtifactVersion":
                    crate::background::process_terminal::SUBAGENT_LIFECYCLE_ARTIFACT_VERSION,
            },
        },
        "events": {
            "ready": SUBAGENT_RPC_READY_EVENT,
            "request": SUBAGENT_RPC_REQUEST_EVENT,
            "replyPrefix": SUBAGENT_RPC_REPLY_EVENT_PREFIX,
            "asyncComplete": SUBAGENT_ASYNC_COMPLETE_EVENT,
            // `rpc.ts:466` / `shared/types.ts:2356` — `"subagent:process-terminal"`, owned and
            // published by
            // [`crate::background::watch::ProcessTerminalAnnouncingCompletionObserver`]
            // (pi `emitProcessTerminalEvent`, `async-execution.ts:666-672`). Like
            // `asyncComplete`, this key is a promise a registered emitter pays for.
            "processTerminal": SUBAGENT_PROCESS_TERMINAL_EVENT,
        },
        "session": session_data(executor, cwd),
    })
}
