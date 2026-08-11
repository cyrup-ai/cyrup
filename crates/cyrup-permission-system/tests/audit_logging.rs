//! FULLY-WIRED PROOF that the audit / debug JSONL trail pi writes (`logging.ts` + the
//! `writeReviewEntry` call sites throughout `index.ts`) actually reaches disk — the
//! security-`review` stream unconditionally, the `debug` stream only when an operator sets
//! `"debug": true` in `config.json` — driven through the registered `before_tool_call` gate
//! (`NativeExtension::on_event(ToolCall)`), the SAME entry point the dispatcher drives at runtime,
//! not by calling the logger directly.
//!
//! This is the regression net for the parity gap these tests were written against: `logging.ts` was
//! entirely unported, so `ExtensionConfig::debug` was materialized into `config.json` by this crate
//! itself and then honored by exactly one `notify` on the forwarding path. Setting `"debug": true`
//! produced NO log file, NO decision trail, and no answer to "why was this tool blocked / who
//! approved this" — the first thing an operator reaches for when a permission gate misbehaves.
//!
//! Upstream call sites reproduced here (pi-permission-system v0.7.1):
//! - `index.ts:2422-2439` — main-check `permission_request.blocked` / `resolution: policy_denied`.
//! - `index.ts:2452-2464` — ask-tier `permission_request.blocked` /
//!   `resolution: confirmation_unavailable` when no human is reachable.
//! - `index.ts:2243-2255` — the skill-read `policy_denied` entry, whose `source` is `skill_read`.
//! - `logging.ts:71-77` — the record shape `{timestamp, extension, stream, event, ...details}`.
//! - v0.8.0 `logging.ts:90-93` — ONLY the `debug` stream is gated on `config.debug`; `review`
//!   (`:98-100`) is a bare `writeLine`. v0.7.1 gated both (`:97-100`), which is the gap tests
//!   (2)/(2b) pin.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_core::ToolCallId;
use cyrup_ext::{ExtMode, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension};
use cyrup_permission_system::PermissionSystemExtension;
use serde_json::{json, Value};

/// A scripted [`HostServices`] whose ONLY override is the full tool registry — otherwise the
/// registry / unknown-tool layer (pi `index.ts:2218-2228`) blocks before any permission check is
/// reached and no decision entry would be written at all.
struct RegistryServices {
    names: Vec<String>,
}
impl HostServices for RegistryServices {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(self.names.clone())
    }
}

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

/// `<agent_dir>/cyrup-permission-system/logs/cyrup-permission-system-debug.jsonl` — pi
/// `join(LOGS_DIR, `${EXTENSION_ID}-debug.jsonl`)` (`extension-config.ts:38,52-56`).
fn trail_path(agent_dir: &Path) -> PathBuf {
    agent_dir
        .join("cyrup-permission-system")
        .join("logs")
        .join("cyrup-permission-system-debug.jsonl")
}

/// Every JSONL entry in the trail, parsed. Empty when the file was never created.
fn trail(agent_dir: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(trail_path(agent_dir)).unwrap_or_default();
    text.lines().map(|line| serde_json::from_str::<Value>(line).unwrap()).collect()
}

fn find_event<'a>(entries: &'a [Value], event: &str) -> Option<&'a Value> {
    entries.iter().find(|e| e["event"] == Value::String(event.to_string()))
}

/// Build an installed extension over `agent_dir` with `global` as its global policy and `debug` as
/// the `config.json` toggle. The config is written BEFORE construction because
/// `PermissionSystemExtension::new` loads it eagerly (pi `loadPermissionSystemConfig` at module
/// init, `index.ts:1605`).
async fn ext_with(agent_dir: &Path, global: &str, debug: bool, registry: &[&str]) -> PermissionSystemExtension {
    write(&agent_dir.join("cyrup-permissions.jsonc"), global);
    write(
        &agent_dir.join("cyrup-permission-system").join("config.json"),
        &format!("{{\n  \"debug\": {debug},\n  \"yoloMode\": false,\n  \"forwardedPromptTimeoutSeconds\": 30\n}}\n"),
    );
    let ext = PermissionSystemExtension::new(agent_dir.to_path_buf(), agent_dir.to_path_buf());
    ext.set_host_services(Arc::new(RegistryServices {
        names: registry.iter().map(|s| (*s).to_string()).collect(),
    }));
    let mut api = InitApi::new();
    ext.init(&mut api).await.unwrap();
    ext
}

fn ctx(cwd: &Path) -> HostCtx {
    // A headless event-tier ctx — the exact shape the dispatcher hands `before_tool_call`.
    HostCtx::event(ExtMode::Print, false, cwd.to_path_buf())
}

fn tool_call(name: &str, input: Value) -> HostEvent {
    HostEvent::ToolCall { call_id: ToolCallId::from("call-1"), name: name.to_string(), input }
}

// ================================================================================================
// (1) A policy DENY under `"debug": true` writes the `permission_request.blocked` review entry.
// ================================================================================================

#[tokio::test]
async fn policy_denied_tool_call_writes_a_review_entry_when_debug_is_on() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    let ext = ext_with(agent_dir, r#"{ "bash": { "*": "deny" } }"#, true, &["bash"]).await;

    let outcome =
        ext.on_event(&tool_call("bash", json!({ "command": "rm -rf /" })), &ctx(agent_dir)).await;
    assert!(matches!(outcome, HookOutcome::Block { .. }), "the deny rule must block: {outcome:?}");

    let entries = trail(agent_dir);
    assert!(!entries.is_empty(), "`\"debug\": true` must produce the audit trail at {:?}", trail_path(agent_dir));

    let blocked = find_event(&entries, "permission_request.blocked")
        .expect("a policy-denied tool call must be audited (pi `index.ts:2422-2439`)");
    // pi `logging.ts:71-77` — the fixed record shape.
    assert_eq!(blocked["extension"], json!("cyrup-permission-system"));
    assert_eq!(blocked["stream"], json!("review"), "a decision belongs to the security-review stream");
    // pi `index.ts:2424-2438` — the decision detail fields.
    assert_eq!(blocked["source"], json!("tool_call"));
    assert_eq!(blocked["resolution"], json!("policy_denied"));
    assert_eq!(blocked["toolName"], json!("bash"));
    assert_eq!(blocked["toolCallId"], json!("call-1"));
    assert_eq!(blocked["command"], json!("rm -rf /"), "the gated command must be recoverable");
    assert_eq!(blocked["decisionPersistence"], json!("none"));
    assert_eq!(blocked["decisionScope"], json!("rm -rf /"), "pi `getPermissionDecisionScope`");
    // pi `createSensitiveLogMetadata` (`index.ts:682-692`) — the digest accompanying the plaintext.
    assert_eq!(blocked["commandMetadata"]["present"], json!(true));
    assert!(blocked["commandMetadata"]["sha256"].as_str().is_some_and(|h| h.len() == 64));
    let ts = blocked["timestamp"].as_str().unwrap();
    assert!(ts.ends_with('Z'), "pi `new Date().toISOString()` shape, got {ts}");
}

// ================================================================================================
// (2) The SAME denied call under the DEFAULT `"debug": false` STILL writes the review entry.
//
// v0.7.1 `logging.ts:97-100` early-returned out of `review` unless `config.debug`; v0.8.0
// `logging.ts:98-100` deletes those lines, so the security-review stream is unconditional. Since
// `"debug": false` is what `default_config_content()` materializes, gating the trail on it meant
// every decision entry was a no-op for every operator who had not first turned on diagnostics —
// i.e. the audit trail was off precisely when it was needed.
// ================================================================================================

#[tokio::test]
async fn debug_off_still_writes_the_review_trail_for_a_denied_call() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    let ext = ext_with(agent_dir, r#"{ "bash": { "*": "deny" } }"#, false, &["bash"]).await;

    let outcome =
        ext.on_event(&tool_call("bash", json!({ "command": "rm -rf /" })), &ctx(agent_dir)).await;
    assert!(matches!(outcome, HookOutcome::Block { .. }), "the deny rule must still block");

    let entries = trail(agent_dir);
    assert!(
        !entries.is_empty(),
        "v0.8.0 `logging.ts:98-100` — the review stream is NOT gated on `debug`; expected a trail \
         at {:?}",
        trail_path(agent_dir)
    );
    let blocked = find_event(&entries, "permission_request.blocked")
        .expect("a policy-denied tool call must be audited regardless of `debug`");
    assert_eq!(blocked["stream"], json!("review"));
    assert_eq!(blocked["resolution"], json!("policy_denied"));
    assert_eq!(blocked["command"], json!("rm -rf /"));
}

// ================================================================================================
// (2b) MIRROR — un-gating `review` must not un-gate `debug`. `config.debug` keeps its upstream
// meaning (v0.8.0 `logging.ts:90-93`), so the diagnostic stream stays opt-in. `session_start` is
// what reaches `refresh_config_and_manager` → the `config.loaded` debug entry (`extension.rs:393`),
// so the same run that proves the review line lands also proves the debug line does not.
// ================================================================================================

#[tokio::test]
async fn debug_off_keeps_the_diagnostic_stream_silent_while_the_review_stream_writes() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    let ext = ext_with(agent_dir, r#"{ "bash": { "*": "deny" } }"#, false, &["bash"]).await;

    let _ = ext
        .on_event(&HostEvent::SessionStart { reason: "startup".to_string() }, &ctx(agent_dir))
        .await;
    let _ = ext.on_event(&tool_call("bash", json!({ "command": "rm -rf /" })), &ctx(agent_dir)).await;

    let entries = trail(agent_dir);
    assert!(
        find_event(&entries, "permission_request.blocked").is_some(),
        "the review stream must be live under `\"debug\": false`"
    );
    assert!(
        find_event(&entries, "config.loaded").is_none(),
        "`config.loaded` is a DEBUG-stream entry (`extension.rs:393`) and must stay gated on \
         `config.debug`: {entries:?}"
    );
    assert!(
        entries.iter().all(|e| e["stream"] == json!("review")),
        "no `debug`-stream line may appear under `\"debug\": false`: {entries:?}"
    );
}

// ================================================================================================
// (2c) MIRROR — with `"debug": true` BOTH streams write, so the un-gating did not delete the flag.
// ================================================================================================

#[tokio::test]
async fn debug_on_writes_both_streams() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    let ext = ext_with(agent_dir, r#"{ "bash": { "*": "deny" } }"#, true, &["bash"]).await;

    let _ = ext
        .on_event(&HostEvent::SessionStart { reason: "startup".to_string() }, &ctx(agent_dir))
        .await;
    let _ = ext.on_event(&tool_call("bash", json!({ "command": "rm -rf /" })), &ctx(agent_dir)).await;

    let entries = trail(agent_dir);
    let loaded = find_event(&entries, "config.loaded")
        .expect("`\"debug\": true` must still produce the diagnostic stream");
    assert_eq!(loaded["stream"], json!("debug"));
    assert!(find_event(&entries, "permission_request.blocked").is_some());
}

// ================================================================================================
// (3) An ask-tier call with no reachable human records WHY it was blocked.
// ================================================================================================

#[tokio::test]
async fn ask_with_no_reachable_human_is_audited_as_confirmation_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    // `ask` + a headless ctx (`has_ui == false`, no subagent hint, no yolo) ⇒ fail-CLOSED.
    let ext = ext_with(agent_dir, r#"{ "bash": { "*": "ask" } }"#, true, &["bash"]).await;

    let outcome = ext.on_event(&tool_call("bash", json!({ "command": "ls" })), &ctx(agent_dir)).await;
    assert!(matches!(outcome, HookOutcome::Block { .. }), "a fail-closed ask must block");

    let entries = trail(agent_dir);
    let blocked = find_event(&entries, "permission_request.blocked")
        .expect("an ask with no reachable human must be audited (pi `index.ts:2452-2464`)");
    assert_eq!(blocked["resolution"], json!("confirmation_unavailable"));
    assert_eq!(blocked["source"], json!("tool_call"));
    // pi records the prompt the human never saw, plus its digest.
    assert!(
        blocked["prompt"].as_str().is_some_and(|p| p.contains("ls")),
        "the unanswered prompt must be recoverable: {:?}",
        blocked["prompt"]
    );
    assert_eq!(blocked["promptMetadata"]["present"], json!(true));

    // No decision was reached, so nothing may claim one.
    assert!(
        find_event(&entries, "permission_request.approved").is_none(),
        "a fail-closed ask must never record an approval"
    );
}

// ================================================================================================
// (4) The trail APPENDS across calls — a session's decisions accumulate rather than overwrite.
// ================================================================================================

#[tokio::test]
async fn successive_decisions_append_to_one_trail() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    let ext = ext_with(agent_dir, r#"{ "bash": { "*": "deny" } }"#, true, &["bash"]).await;

    for command in ["one", "two", "three"] {
        let _ = ext.on_event(&tool_call("bash", json!({ "command": command })), &ctx(agent_dir)).await;
    }

    let commands: Vec<String> = trail(agent_dir)
        .iter()
        .filter(|e| e["event"] == json!("permission_request.blocked"))
        .filter_map(|e| e["command"].as_str().map(str::to_string))
        .collect();
    assert_eq!(commands, vec!["one", "two", "three"], "every gated call must leave its own line");
}
