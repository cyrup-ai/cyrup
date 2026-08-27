//! F2 — a FRACTIONAL `forwardedPromptTimeoutSeconds` reaches the dialog as a fractional duration.
//!
//! Upstream's field is a JS `number | null` and `normalizePermissionSystemConfig` keeps any finite
//! positive value verbatim (v0.8.0 `extension-config.ts:83-84`, `forwardedPromptTimeoutSeconds =
//! rawTimeout`). The forwarded-prompt path then does a plain multiply,
//! `timeoutMs = forwardedPromptTimeoutSeconds * 1000` (v0.8.0 `index.ts:1200-1201`), and interpolates
//! the same raw number into the prompt body and the timeout denial reason (`:1204`, `:1207`).
//!
//! So `45.5` must produce a **45500 ms** dialog timeout and the strings "45.5 seconds" — not 45000 ms
//! and "45 seconds". Cyrup held the field as `Option<u64>` and `normalize` did `Some(n as u64)`,
//! truncating at load; forwarding then re-truncated with `Duration::from_secs`.
//!
//! This drives the REAL production path — `process_forwarded_requests`, the same function
//! `spawn_forwarding_watcher` calls on every wake — against a recording [`HostServices`], and reads
//! the `timeout_ms` that actually crossed the `HostServices::select` boundary.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::{Arc, Mutex};

use cyrup_ext::{DialogOptions, HostServices};
use crate::{
    forwarding::{
        forwarding_location, process_forwarded_requests, ProcessForwardedOptions,
        FORWARDING_AGENT_DIR_ENV,
    },
    ExtensionConfig, ForwardedPermissionRequest, ForwardedPermissionResponse,
};

/// What the parent actually handed the UI for the forwarded prompt.
#[derive(Debug, Clone, Default)]
struct SelectCall {
    prompt: String,
    timeout_ms: Option<u64>,
}

/// A [`HostServices`] that answers the session-id query and records the one `select` the forwarded
/// prompt makes, then returns `None` (the ESC/timeout shape) so the run resolves to a reject and
/// writes a response carrying the timeout denial reason.
struct RecordingServices {
    session_id: String,
    calls: Arc<Mutex<Vec<SelectCall>>>,
}

impl HostServices for RecordingServices {
    fn session_id(&self) -> Option<String> {
        Some(self.session_id.clone())
    }

    fn select(
        &self,
        prompt: &str,
        _options: &serde_json::Value,
        opts: &DialogOptions,
    ) -> Option<String> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(SelectCall { prompt: prompt.to_string(), timeout_ms: opts.timeout_ms });
        None
    }
}

fn now_millis() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

/// A request created NOW, so `resolve_forwarded_decision` skips the expired-on-read auto-deny and
/// actually surfaces the dialog whose timeout this test is about.
fn fresh_request(id: &str, target_session_id: &str) -> String {
    serde_json::to_string(&ForwardedPermissionRequest {
        id: id.to_string(),
        response_nonce: format!("nonce-{id}"),
        created_at: now_millis(),
        requester_session_id: "child-session".to_string(),
        target_session_id: target_session_id.to_string(),
        requester_agent_name: "reviewer".to_string(),
        message: "Subagent wants to run bash".to_string(),
    })
    .expect("the request serializes")
}

/// Drive ONE forwarded request through the production scan with `config`, and return
/// (the recorded select call, the response the parent wrote).
async fn drive(
    agent_dir: &Path,
    session_id: &str,
    request_id: &str,
    config: &ExtensionConfig,
) -> (SelectCall, ForwardedPermissionResponse) {
    let location = forwarding_location(agent_dir, session_id).expect("derives a location");
    std::fs::create_dir_all(&location.requests_dir).expect("creates the inbox");
    std::fs::create_dir_all(&location.responses_dir).expect("creates the outbox");
    std::fs::write(
        location.requests_dir.join(format!("{request_id}.json")),
        fresh_request(request_id, session_id),
    )
    .expect("writes the request");

    let calls = Arc::new(Mutex::new(Vec::new()));
    let services: Arc<dyn HostServices> = Arc::new(RecordingServices {
        session_id: session_id.to_string(),
        calls: Arc::clone(&calls),
    });
    process_forwarded_requests(
        agent_dir,
        session_id,
        &services,
        config,
        ProcessForwardedOptions::preserve_location(),
        // PERM-008/PERM-031: the shared audit trail, and the live `has_ui` pi re-reads per scan.
        &crate::logging::AuditTrail::detached(agent_dir.join("logs")),
        true,
    )
    .await;

    let recorded = calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    assert_eq!(recorded.len(), 1, "the forwarded prompt must reach `select` exactly once");
    let call = recorded.into_iter().next().expect("just asserted one call");

    let response_text =
        std::fs::read_to_string(location.responses_dir.join(format!("{request_id}.json")))
            .expect("the parent wrote a response");
    let response: ForwardedPermissionResponse =
        serde_json::from_str(&response_text).expect("the response parses");
    (call, response)
}

fn assert_no_ambient_override() {
    assert!(
        crate::envx::var(FORWARDING_AGENT_DIR_ENV).is_none(),
        "this test derives the spool from its own temp agent dir; an ambient \
         {FORWARDING_AGENT_DIR_ENV} override would invalidate it"
    );
}

/// THE ASSERTION: `45.5` seconds is 45500 ms at the dialog boundary, and reads as "45.5 seconds" in
/// both operator-visible strings. Pre-fix every one of these was the truncated `45`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fractional_forwarded_timeout_reaches_the_dialog_unrounded() {
    assert_no_ambient_override();
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let session_id = "parent-session-fractional";

    // The value an operator put in `config.json`, taken through the real normalizer so this pins
    // `normalize` too: upstream keeps any finite positive number as-is (`extension-config.ts:84`).
    let config = ExtensionConfig::normalize(
        &serde_json::json!({ "forwardedPromptTimeoutSeconds": 45.5 }),
    );
    assert_eq!(
        config.forwarded_prompt_timeout_seconds,
        Some(45.5),
        "normalize must keep a finite positive number verbatim, not floor it"
    );

    let (call, response) = drive(agent_dir.path(), session_id, "req-frac", &config).await;

    // pi `timeoutMs = forwardedPromptTimeoutSeconds * 1000` (`index.ts:1201`).
    assert_eq!(
        call.timeout_ms,
        Some(45_500),
        "45.5 s must cross the dialog boundary as 45500 ms; got {:?}",
        call.timeout_ms
    );
    // pi `promptMessage` (`index.ts:1207`) — the raw number interpolated.
    assert!(
        call.prompt.contains("auto-denies after 45.5 seconds"),
        "prompt must name the fractional timeout; got:\n{}",
        call.prompt
    );
    // pi `timeoutDenialReason` (`index.ts:1204`), carried onto the reject decision.
    assert_eq!(
        response.denial_reason.as_deref(),
        Some("permission_timeout: forwarded permission prompt was not answered within 45.5 seconds."),
        "the denial reason must name the fractional timeout"
    );
}

/// MIRROR (must stay green): the fix must not turn whole seconds into floats anywhere an operator
/// can see. The default `30` still crosses as 30000 ms and still reads "30 seconds" — never
/// "30.0 seconds", which is what a naive `{:?}`/`Number::from_f64` rendering would produce.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_whole_second_forwarded_timeout_is_unchanged() {
    assert_no_ambient_override();
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let session_id = "parent-session-whole";

    let config = ExtensionConfig::default();
    assert_eq!(config.forwarded_prompt_timeout_seconds, Some(30.0));

    let (call, response) = drive(agent_dir.path(), session_id, "req-whole", &config).await;

    assert_eq!(call.timeout_ms, Some(30_000));
    assert!(
        call.prompt.contains("auto-denies after 30 seconds"),
        "a whole-second timeout must render without a fraction; got:\n{}",
        call.prompt
    );
    assert_eq!(
        response.denial_reason.as_deref(),
        Some("permission_timeout: forwarded permission prompt was not answered within 30 seconds.")
    );
}

/// MIRROR (must stay green): `null` still means "wait indefinitely" (pi `:1200`'s `!== null` guard,
/// `:1208`'s else-branch message) — no timeout crosses the boundary and no denial reason is attached.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_null_forwarded_timeout_still_waits_indefinitely() {
    assert_no_ambient_override();
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let session_id = "parent-session-null";

    let config = ExtensionConfig::normalize(
        &serde_json::json!({ "forwardedPromptTimeoutSeconds": serde_json::Value::Null }),
    );
    assert_eq!(config.forwarded_prompt_timeout_seconds, None);

    let (call, response) = drive(agent_dir.path(), session_id, "req-null", &config).await;

    assert_eq!(call.timeout_ms, None, "an indefinite wait must set no dialog timeout");
    assert!(
        call.prompt.contains("will wait indefinitely until answered"),
        "got:\n{}",
        call.prompt
    );
    assert_eq!(response.denial_reason, None);
}
