//! PB-8 — the capability advertisement: pi `src/extension/rpc.ts:431-470` @v0.68.0
//! (`sessionData` and `pingData`).
//!
//! # Every key here is a PROMISE
//!
//! `pingData` is both the `ping` reply and the payload of the one-shot
//! [`super::SUBAGENT_RPC_READY_EVENT`], so it is the document a client integrates against. A key
//! advertised and not implemented is a lie the client will act on, which is why five of upstream's
//! capability keys and two of its event keys are DROPPED here rather than copied — each with the
//! missing seam named.

use std::path::Path;

use serde_json::Value;

use crate::background::async_status_snapshot::{
    ASYNC_STATUS_SNAPSHOT_KIND, ASYNC_STATUS_SNAPSHOT_VERSION,
};
use crate::background::watch::SUBAGENT_ASYNC_COMPLETE_EVENT;
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
/// # `[CYRUP-DELTA]` — five capability keys and two event keys are DROPPED
///
/// * **`nonRecoveringSteer`** (`:452`) advertises that the RPC forces `steeringRecovery: false`.
///   cyrup's steer has no recovery mode to turn off (`grep -rn 'steering_recovery' crates/` is
///   empty), so there is nothing to promise — see [`super::params::steer_params`].
/// * **`launchResolvedExtensions`** / **`runtimeAcknowledgedExtensions`** (`:456-457`) advertise
///   the child extension-resolution reporting surface and the
///   `subagent:acknowledge-extension` child-runtime event. Neither exists here.
/// * **`processTerminalProof`** (`:458`) advertises the process-terminal lifecycle artifact.
///   cyrup has no process-terminal artifact at all — already recorded at
///   `background/active_async_capacity/key.rs:93-99` and `.../inspect.rs:33`.
/// * **`events.childStatus`** (`:465`) is `subagent:child-status`, emitted only by upstream's
///   inline `stopAsyncRun`. cyrup routes `stop` through the tool arm instead (see
///   [`super`]'s module doc), so nothing emits it.
/// * **`events.processTerminal`** (`:466`) is the same missing artifact as above.
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
        },
        "events": {
            "ready": SUBAGENT_RPC_READY_EVENT,
            "request": SUBAGENT_RPC_REQUEST_EVENT,
            "replyPrefix": SUBAGENT_RPC_REPLY_EVENT_PREFIX,
            "asyncComplete": SUBAGENT_ASYNC_COMPLETE_EVENT,
        },
        "session": session_data(executor, cwd),
    })
}
