//! TOOL-008 end-to-end: the `bash` tool a real `AgentSession` registers publishes THIS session's
//! metadata to its child, and republishes it when the model or reasoning level changes.
//!
//! Pi gets this for free: `resolveSpawnContext` reads `ctx.sessionManager.getSessionId()` /
//! `getSessionFile()` / `ctx.model` / `ctx.thinkingLevel` off the per-call `ExtensionContext` every
//! time a command spawns (`pi/packages/coding-agent/src/core/tools/bash.ts:158-184`), which is what
//! `pi/packages/coding-agent/docs/environment-variables.md:27` means by "The values are resolved
//! when each command starts. Switching models or changing the reasoning level therefore affects the
//! next bash command without restarting Pi."
//!
//! cyrup's `Tool::execute` takes no session context, so the session PUSHES into a shared handle.
//! `crates/cyrup-tools/tests/bash_session_env.rs` proves the tool half in isolation; this file
//! proves the wiring — that the handle the builder gives the registered `BashTool` is the same one
//! `set_model_*` / `set_thinking_level` update, driven through a real scripted tool-call round trip
//! rather than by poking the tool directly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::{ModelId, ProviderId, StopReason};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use cyrup_provider::Provider;
use crate::{
    AgentSession, InputSource, SessionBuilder, SessionConfig, UserInput,
};
use tempfile::TempDir;

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
    cfg
}

/// The `bash` command the scripted assistant issues: dump the five variables into `out`, one
/// `KEY=value` per line, with an unset variable rendering as an empty value.
fn probe_command(out: &str) -> String {
    format!(
        r#"{{ for v in CYRUP_SESSION_ID CYRUP_SESSION_FILE CYRUP_PROVIDER CYRUP_MODEL CYRUP_REASONING_LEVEL; do
  eval "printf '%s=%s\n' \"$v\" \"\${{$v-}}\""
done ; }} > {out}"#
    )
}

/// Drive one scripted turn whose assistant message calls `bash` with the probe, then read the file
/// the child wrote back as a key/value map.
async fn probe(session: &Arc<AgentSession>, faux: &Arc<FauxProvider>, fx: &Fixture, out: &str) -> Vec<(String, String)> {
    faux.set_responses(vec![
        faux_assistant_message(
            vec![faux_tool_call("bash", serde_json::json!({ "command": probe_command(out) }))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let _ = session.prompt(UserInput::text("probe", InputSource::Sdk)).await.expect("prompt");
    session.wait_for_idle().await;

    let text = std::fs::read_to_string(fx.cwd.join(out))
        .unwrap_or_else(|e| panic!("the bash child did not write {out}: {e}"));
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn get<'a>(kv: &'a [(String, String)], key: &str) -> &'a str {
    kv.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).unwrap_or_else(|| {
        panic!("the probe did not report {key}; got {kv:?}")
    })
}

#[tokio::test]
async fn bash_children_see_this_sessions_metadata_and_track_model_changes() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = faux.clone();
    let session = Arc::new(
        SessionBuilder::new(provider, base_config(&fx)).build().await.expect("build session"),
    );

    // ---- 1. the child sees THIS session's identity, not a stale or empty value ----
    let kv = probe(&session, &faux, &fx, "probe1.txt").await;
    assert_eq!(
        get(&kv, "CYRUP_SESSION_ID"),
        session.session_id().to_string(),
        "the bash child did not see the live session id; got {kv:?}"
    );
    let expected_file =
        session.session_file().await.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    assert_eq!(
        get(&kv, "CYRUP_SESSION_FILE"),
        expected_file,
        "the bash child did not see the live session file; got {kv:?}"
    );
    assert!(
        !get(&kv, "CYRUP_PROVIDER").is_empty() && !get(&kv, "CYRUP_MODEL").is_empty(),
        "the provider/model pair must be published; got {kv:?}"
    );
    assert!(
        !get(&kv, "CYRUP_REASONING_LEVEL").is_empty(),
        "the reasoning level must be published; got {kv:?}"
    );

    // ---- 2. a model change reaches the NEXT command with no rebuild ----
    // environment-variables.md:27. `set_model_id` is the no-resolution setter, so this asserts the
    // republish rather than the registry.
    session
        .set_model_id(ProviderId::from("acme"), ModelId::from("acme-model-9"))
        .await
        .expect("set model");
    let kv = probe(&session, &faux, &fx, "probe2.txt").await;
    assert_eq!(get(&kv, "CYRUP_PROVIDER"), "acme", "model change did not reach the child: {kv:?}");
    assert_eq!(get(&kv, "CYRUP_MODEL"), "acme-model-9", "got {kv:?}");
    // The identity half is unchanged by a model swap.
    assert_eq!(get(&kv, "CYRUP_SESSION_ID"), session.session_id().to_string());
}

/// A fork mutates the session manager IN PLACE (`create_branched_session`), giving the session a new
/// id and a new file. Pi re-reads both off the manager on every spawn, so a `bash` child run after a
/// fork reports the POST-fork identity; cyrup must republish to match.
#[tokio::test]
async fn a_fork_republishes_the_session_identity_to_bash_children() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = faux.clone();
    let session = Arc::new(
        SessionBuilder::new(provider, base_config(&fx)).build().await.expect("build session"),
    );

    let before = probe(&session, &faux, &fx, "before.txt").await;
    let id_before = get(&before, "CYRUP_SESSION_ID").to_string();
    let file_before = get(&before, "CYRUP_SESSION_FILE").to_string();

    let forked = session.fork().await.expect("fork");
    assert_ne!(forked.to_string(), id_before, "fixture: the fork must mint a new session id");

    let after = probe(&session, &faux, &fx, "after.txt").await;
    assert_eq!(
        get(&after, "CYRUP_SESSION_ID"),
        forked.to_string(),
        "a bash child run after /fork still reported the PRE-fork session id: {after:?}"
    );
    assert_ne!(
        get(&after, "CYRUP_SESSION_FILE"),
        file_before,
        "a bash child run after /fork still pointed at the pre-fork session file: {after:?}"
    );
    assert_eq!(
        get(&after, "CYRUP_SESSION_FILE"),
        session.session_file().await.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
    );
}
