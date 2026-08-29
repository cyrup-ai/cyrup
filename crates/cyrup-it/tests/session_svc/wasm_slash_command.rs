//! WASM SLASH-COMMAND END-TO-END (residual §07 / arch-08b headline for the facade). Proves that a
//! slash command REGISTERED BY A LIVE WASM GUEST executes through the REAL run path — Pi
//! `_tryExecuteExtensionCommand` (agent-session.ts:1148-1172), reached from `prompt` →
//! `prepare` (agent-session.ts:1006-1013). Not a native stub, not a hand-called facade: we
//! build a real `wasm32-wasip2` COMPONENT, load it through the session's host with the session's
//! own `LiveHostServices` injected (the arch-08 §5.6 seam), then drive `AgentSession::prompt("/greet
//! world")` and assert the GUEST handler ran across the WIT boundary (its `ctx.ui().notify(...)`
//! recorded host-side) AND that the slash command short-circuited the prompt (no user message sent).
//!
//! The fixture is the bundled `cyrup-ext-sdk` demo extension (its `example/commands_session.rs`
//! registers the `/greet` command), built to a component via `cargo build -p cyrup-ext-sdk --target wasm32-wasip2`
//! (wasm32-wasip2 emits a component directly). Set `CYRUP_EXT_FIXTURE_COMPONENT` to a prebuilt
//! component to skip the nested build.
// The original `#![cfg(feature = "wasm-host")]` is deliberately GONE. It named
// cyrup-session-svc's own feature, which that crate enables in its `default` — so it was
// always true here. Re-spelled in cyrup-it it would name THIS crate's `wasm-host`, which
// `--features it` does not enable, and every test below would SILENTLY not compile in.
// See the `[[test]]` note in crates/cyrup-it/Cargo.toml.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cyrup_core::{ExtensionId, Message, StopReason};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

// MIGRATION (docs/TEST-ARCHITECTURE.md §3.4): this file used to carry its own `fixture_component()`
// that shelled out to `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` into the SHARED, fixed
// `std::env::temp_dir()/cyrup-session-svc-fixture-target` — one of ten byte-identical copies that
// serialized on each other's cargo build lock and never cleaned up. `cyrup-it`'s `build.rs` now
// builds the component ONCE for the whole suite and exports its path; `CYRUP_EXT_FIXTURE_COMPONENT`
// still overrides it, at that one place instead of ten.
use crate::support::bins;

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    // Disable on-disk extension auto-discovery so ONLY the explicitly-loaded guest is present.
    cfg.no_extensions = true;
    cfg
}

fn faux_with_ok() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    faux
}

fn user_texts(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::User { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|c| match c {
                        cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

/// THE headline proof: a live wasm guest registers `/greet`; driving `prompt("/greet world")`
/// through the real `prepare` → `_tryExecuteExtensionCommand` path runs the GUEST handler across
/// the WIT boundary and short-circuits the prompt (no user message sent to the model).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wasm_guest_slash_command_executes_through_the_run_path() {
    let wasm_path = bins::component();
    let bytes = std::fs::read(&wasm_path).expect("read fixture component bytes");

    let fx = fixture();
    let session = SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .unwrap();

    // Load the guest COMPONENT through the session's host, injecting the session's OWN
    // LiveHostServices (arch-08 §5.6 — the same backend `apply_pending_control` drains).
    let ext = session
        .load_wasm_extension(
            ExtensionId::from("demo"),
            &bytes,
            // EXT-059: the grant is now explicit. `host_granted()` is the TOTAL grant these
            // fixtures previously got implicitly from `load_wasm_extension`'s `load_wasm` call.
            &cyrup_ext::Capabilities::host_granted(),
        )
        .await
        .expect("load + init the live wasm extension");

    // The guest's `init` registered `/greet` (cyrup-ext-sdk example/commands_session.rs).
    assert!(
        session.services().ext_host.registry().command_names().unwrap().iter().any(|n| n == "greet"),
        "the guest-registered `/greet` command is in the host command registry"
    );
    // Nothing has invoked the handler yet.
    assert!(
        !ext.guest().notifications().iter().any(|n| n.contains("greet command ran")),
        "guest handler has not run before the prompt"
    );

    // Drive the command through the REAL public entry point (prompt → prepare →
    // _tryExecuteExtensionCommand → the live guest's `execute-command` export).
    let _ = session.prompt("/greet world").await.unwrap();
    session.wait_for_idle().await;

    // The GUEST handler ran across the wasm boundary: its `ctx.ui().notify("greet command ran")`
    // was recorded host-side in the live extension's guest state.
    assert!(
        ext.guest().notifications().iter().any(|n| n.contains("greet command ran")),
        "the wasm guest command handler executed end-to-end: {:?}",
        ext.guest().notifications()
    );

    // The slash command short-circuited the prompt: no `/greet` user message was sent/persisted
    // (Pi `_tryExecuteExtensionCommand` returns `true` ⇒ the prompt is consumed).
    assert!(
        user_texts(&session.messages().await).iter().all(|t| !t.contains("/greet")),
        "the wasm slash command was consumed — no user message went to the model"
    );

    // A `/unknown` command (no guest or native owner) is NOT consumed: it falls through to a
    // normal prompt (Pi `getCommand` returns undefined ⇒ false, agent-session.ts:1184).
    let _ = session.prompt("/unknown please run").await.unwrap();
    session.wait_for_idle().await;
    assert!(
        user_texts(&session.messages().await).iter().any(|t| t.contains("/unknown please run")),
        "an unmatched slash command falls through to normal prompt handling"
    );
}

/// L5 G7+G8 assembled proof: a LIVE wasm guest's `appendEntry`/`setSessionName`/`setLabel`
/// capabilities (previously host-side no-ops) FIRE and mutate the REAL running session (Pi
/// `appendEntry`/`setSessionName`/`setLabel`, agent-session.ts:2265-2279). The demo's `/statedemo`
/// command appends a `demoNote` custom entry, renames the session, and labels that entry; we then
/// observe — in the assembled session, not a stub — that (1) the custom entry is in the tree, (2) the
/// session is renamed, (3) the label is set, and (4) an `entry_appended` event reached a subscriber.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wasm_guest_state_mutations_fire_against_the_live_session() {
    use futures::StreamExt;

    let wasm_path = bins::component();
    let bytes = std::fs::read(&wasm_path).expect("read fixture component bytes");

    let fx = fixture();
    let session = SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .unwrap();
    let _ext = session
        .load_wasm_extension(
            ExtensionId::from("demo"),
            &bytes,
            // EXT-059: the grant is now explicit. `host_granted()` is the TOTAL grant these
            // fixtures previously got implicitly from `load_wasm_extension`'s `load_wasm` call.
            &cyrup_ext::Capabilities::host_granted(),
        )
        .await
        .expect("load + init the live wasm extension");

    // The guest's `init` registered `/statedemo`.
    assert!(
        session
            .services()
            .ext_host
            .registry()
            .command_names()
            .unwrap()
            .iter()
            .any(|n| n == "statedemo"),
        "the guest-registered `/statedemo` command is in the host command registry"
    );

    // Baseline: no `demoNote` entry yet, and the default (unnamed) session.
    let before = session.entries_json().await;
    assert!(
        !before.iter().any(|e| e.get("customType").and_then(|v| v.as_str()) == Some("demoNote")),
        "no guest-appended entry before the command"
    );
    assert_ne!(session.session_name().await.as_deref(), Some("renamed-by-guest"));

    // Observe the fan-out: a persistent subscription must receive `entry_appended`.
    let mut sub = session.subscribe();

    // Drive the guest command through the REAL run path (prompt → _tryExecuteExtensionCommand →
    // execute-command → the guest's append/rename/label capability calls → apply_pending_control).
    let _ = session.prompt("/statedemo").await.unwrap();
    session.wait_for_idle().await;

    // (1) The custom entry is now in the REAL session tree.
    let entries = session.entries_json().await;
    let appended = entries
        .iter()
        .find(|e| e.get("customType").and_then(|v| v.as_str()) == Some("demoNote"))
        .expect("the guest-appended `demoNote` custom entry is in the tree");
    assert_eq!(appended.get("type").and_then(|v| v.as_str()), Some("custom"));
    assert_eq!(
        appended.get("data").and_then(|d| d.get("note")).and_then(|v| v.as_str()),
        Some("from guest"),
        "the guest's entry payload persisted verbatim"
    );
    let appended_id =
        appended.get("id").and_then(|v| v.as_str()).expect("appended entry has an id").to_string();

    // (2) The session was renamed by the guest.
    assert_eq!(
        session.session_name().await.as_deref(),
        Some("renamed-by-guest"),
        "the guest `setSessionName` renamed the live session"
    );

    // (3) The label was set on the appended entry (a persisted `label` entry targets it).
    assert!(
        entries.iter().any(|e| {
            e.get("type").and_then(|v| v.as_str()) == Some("label")
                && e.get("targetId").and_then(|v| v.as_str()) == Some(appended_id.as_str())
                && e.get("label").and_then(|v| v.as_str()) == Some("guest-label")
        }),
        "the guest `setLabel` persisted a label targeting the appended entry: {entries:?}"
    );
    // …and the tree resolves that label onto the node.
    let tree = session.tree_json().await;
    fn find_label(nodes: &[serde_json::Value], id: &str) -> Option<String> {
        for n in nodes {
            if n.get("entry").and_then(|e| e.get("id")).and_then(|v| v.as_str()) == Some(id) {
                return n.get("label").and_then(|v| v.as_str()).map(str::to_string);
            }
            if let Some(children) = n.get("children").and_then(|c| c.as_array())
                && let Some(found) = find_label(children, id)
            {
                return Some(found);
            }
        }
        None
    }
    assert_eq!(
        find_label(&tree, &appended_id).as_deref(),
        Some("guest-label"),
        "the appended entry's node carries the guest-set label"
    );

    // (4) A subscriber observed the `entry_appended` fan-out.
    let mut kinds = Vec::new();
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(500), sub.next()).await {
        kinds.push(ev.kind());
        if kinds.contains(&"entry_appended") {
            break;
        }
    }
    assert!(
        kinds.contains(&"entry_appended"),
        "a live subscriber observed the entry_appended event: {kinds:?}"
    );
}

/// Guard: the fixture path resolves to a real file (the nested build actually produced a component).
#[test]
fn fixture_component_exists() {
    let p = bins::component();
    assert!(Path::new(&p).exists(), "fixture component missing at {}", p.display());
}

/// SEAM-005 across the WIT boundary: the `events.on-agent-settled` EXPORT this change added to
/// BOTH world.wit copies is actually invoked on a live guest.
///
/// The demo subscribes `agent_settled` (cyrup-ext-sdk `example/hooks.rs`) and notifies with a distinct
/// string. Driving one real turn must produce EXACTLY ONE such notification — proving the host
/// dispatches the synthesised `HostEvent::AgentSettled` into the guest, and that it is a per-RUN
/// event, not a per-agent-loop one (the guest's `agent_start` notification is the control).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wasm_guest_receives_agent_settled_across_the_wit_boundary() {
    let wasm_path = bins::component();
    let bytes = std::fs::read(&wasm_path).expect("read fixture component bytes");

    let fx = fixture();
    let session = SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .unwrap()
        .into_shared(); // bound: the post-run driver (and therefore the settle emit) is live.

    let ext = session
        .load_wasm_extension(
            ExtensionId::from("demo"),
            &bytes,
            // EXT-059: the grant is now explicit. `host_granted()` is the TOTAL grant these
            // fixtures previously got implicitly from `load_wasm_extension`'s `load_wasm` call.
            &cyrup_ext::Capabilities::host_granted(),
        )
        .await
        .expect("load + init the live wasm extension");

    assert!(
        !ext.guest().notifications().iter().any(|n| n.contains("agent settled")),
        "nothing settled before the run"
    );

    let _ = session.prompt("hello").await.unwrap();
    session.wait_for_idle().await;

    let notes = ext.guest().notifications();
    assert_eq!(
        notes.iter().filter(|n| n.contains("demo: agent settled")).count(),
        1,
        "the guest's on-agent-settled export was invoked exactly once for the run: {notes:?}"
    );
    assert!(
        notes.iter().any(|n| n.contains("demo extension active")),
        "control: the guest's agent_start handler ran too, so this is not a dispatch-wide failure"
    );
}
