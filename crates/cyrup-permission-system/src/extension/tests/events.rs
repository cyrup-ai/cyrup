//! The two published surfaces: the yolo runtime-API registration and the `permission-request`
//! event channel.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use cyrup_ext::{HostEvent, HostServices, NativeExtension};

use crate::yolo_api::YoloModeControlOptions;

use super::support::*;
use crate::ask::AskOutcome;
use crate::dedup::DedupDetails;
use crate::extension::{PERMISSION_REQUEST_EVENT_CHANNEL, PermissionSystemExtension, guard};

/// A [`HostServices`] that records every [`HostServices::emit_event`] — the consumer's whole
/// view of the bus, standing in for a second extension's subscription.
#[derive(Default)]
struct RecordingBus {
    events: Mutex<Vec<(String, Value)>>,
}

impl RecordingBus {
    fn taken(&self) -> Vec<(String, Value)> {
        guard(&self.events).clone()
    }
}

impl HostServices for RecordingBus {
    fn emit_event(&self, topic: &str, payload: &Value) {
        guard(&self.events).push((topic.to_string(), payload.clone()));
    }
}

/// PERM-011 half A — the publish seam, end to end through the extension.
///
/// **Pre-fix this test cannot compile, let alone pass**: `crate::runtime_api` did not exist and
/// there was no spelling under which a holder of nothing but the crate could reach
/// `yolo_mode`/`set_yolo_mode`/`toggle_yolo_mode`. The assertion that makes it a behaviour test
/// rather than an API-shape test is the last one before shutdown: flipping the flag through the
/// PUBLISHED handle must move the live config the gate itself reads.
#[test]
fn init_publishes_the_yolo_control_surface_and_shutdown_retracts_it() {
    // The registry lock is taken in this SYNCHRONOUS frame, not inside the async body, so the
    // guard is never captured in the future's state (`clippy::await_holding_lock`).
    let _registry = crate::runtime_api::test_registry_lock();
    block_on(init_publishes_the_yolo_control_surface_and_shutdown_retracts_it_body());
}

async fn init_publishes_the_yolo_control_surface_and_shutdown_retracts_it_body() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().to_path_buf();
    // `into_shared` is what production uses (`permission_extension_for_env`); it installs the
    // `Weak` the published handle borrows through.
    let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone()).into_shared();

    assert!(
        crate::runtime_api::runtime_api().is_none(),
        "nothing is published before activation (pi registers inside the extension body)"
    );
    init_ext(&ext).await;

    // From here on, the test holds ONLY the module path — no extension handle is used to drive
    // the API, which is exactly the position a second extension is in.
    let api = crate::runtime_api::runtime_api().expect("init must publish the runtime API");
    assert!(
        !api.get_yolo_mode(),
        "pi `getYoloMode` reads the live config"
    );

    let result = api.toggle_yolo_mode(&YoloModeControlOptions::transient("second-extension"));
    assert!(
        result.changed && result.yolo_mode,
        "the toggle must report the move: {result:?}"
    );
    assert!(
        !result.persisted,
        "`persist: false` is in-memory only (index.ts:1433)"
    );
    assert!(
        ext.yolo_mode(),
        "the flip must reach the config the GATE reads, not a copy — this is the half of \
         PERM-011 that makes the published methods more than a shape"
    );

    let ctx = event_ctx(agent_dir.clone());
    let _ = ext
        .on_event(
            &HostEvent::SessionShutdown {
                reason: "exit".to_string(),
                target_session_file: None,
            },
            &ctx,
        )
        .await;
    assert!(
        crate::runtime_api::runtime_api().is_none(),
        "pi `unregisterPiPermissionSystemRuntimeApi(runtimeApi)` (index.ts:1868) — a finished \
         session must not leave a live control surface published"
    );
}

/// PERM-011 half B — the permission-request event channel, on the yolo auto-approval path
/// (pi `emitPermissionStateEvent(details, "approved")`, `index.ts:1606`).
///
/// Pre-fix, `grep -rn 'events.emit\|emit_event' crates/cyrup-permission-system/src` returned
/// zero and this test saw an EMPTY recording: no observer could tell that a tool call had been
/// gated at all. It asserts the topic, the state and the full projection of the details, since
/// the payload IS the interface.
#[test]
fn a_gated_request_is_published_on_the_permission_request_channel() {
    block_on(a_gated_request_is_published_on_the_permission_request_channel_body());
}

async fn a_gated_request_is_published_on_the_permission_request_channel_body() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().to_path_buf();
    let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
    let bus = Arc::new(RecordingBus::default());
    ext.set_host_services(bus.clone());
    // Yolo on: `prompt_decision` takes pi's `shouldAutoApprovePermissionState` arm
    // (`index.ts:1598-1608`), which needs no human and still emits.
    guard(&ext.config).yolo_mode = true;

    let details = DedupDetails {
        request_id: "req-perm011".to_string(),
        source: "tool_call".to_string(),
        agent_name: Some("reviewer".to_string()),
        message: "Allow bash?".to_string(),
        tool_call_id: Some("call-1".to_string()),
        tool_name: Some("bash".to_string()),
        skill_name: None,
        path: None,
        command: Some("git status".to_string()),
        target: None,
        tool_input: json!({ "command": "git status" }),
    };
    let ctx = headless_ctx(dir.path());
    let outcome = ext.prompt_decision(&details, &ctx).await;
    assert!(
        matches!(outcome, AskOutcome::Decided(ref d) if d.approved),
        "fixture precondition: the yolo arm must approve, or nothing reaches the emit"
    );

    let events = bus.taken();
    assert_eq!(
        events.len(),
        1,
        "exactly one state event per decision: {events:?}"
    );
    let (topic, payload) = &events[0];
    assert_eq!(topic, PERMISSION_REQUEST_EVENT_CHANNEL);
    assert_eq!(payload["state"], json!("approved"));
    assert_eq!(payload["requestId"], json!("req-perm011"));
    assert_eq!(payload["source"], json!("tool_call"));
    assert_eq!(payload["message"], json!("Allow bash?"));
    assert_eq!(payload["toolCallId"], json!("call-1"));
    assert_eq!(payload["toolName"], json!("bash"));
    assert_eq!(payload["command"], json!("git status"));
    assert_eq!(payload["agentName"], json!("reviewer"));
    assert_eq!(payload["toolInput"], json!({ "command": "git status" }));
    // pi's optional fields are `undefined`; cyrup's are `null` (the CYRUP-DELTA on
    // `emit_permission_state_event`) — pinned so the mapping is not "tidied" into absent keys.
    assert_eq!(payload["skillName"], Value::Null);
    assert_eq!(payload["path"], Value::Null);
    assert_eq!(payload["target"], Value::Null);
}
