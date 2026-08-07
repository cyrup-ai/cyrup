//! The INJECTION half of pi's `resolveSpawnContext` (TOOL-008).
//!
//! `pi/packages/coding-agent/src/core/tools/bash.ts:171-181` repopulates the child environment from
//! the live session when `exposeSessionEnvironment && ctx` — session id, session file (only when
//! the session is persisted), the provider/model pair (only when a model is selected) and the
//! reasoning level. `bash.ts:322` defaults the flag to TRUE, and `bash.ts:329-331` advertises the
//! family in the prompt guidelines. `docs/environment-variables.md:27` pins the timing: "The values
//! are resolved when each command starts. Switching models or changing the reasoning level
//! therefore affects the next bash command without restarting Pi."
//!
//! This binary never mutates the process environment (see `bash_env_scrub.rs` for that half).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolResult, ToolUpdate, ToolUpdateSink};
use cyrup_tools::config::{BashOpts, SessionEnvHandle, SessionEnvInfo};
use cyrup_tools::ops::{Backend, ProcOps, ShellConfig};
use cyrup_tools::tools::BashTool;
use std::path::PathBuf;
use std::sync::Arc;

fn proc() -> Arc<dyn ProcOps> {
    Backend::default().proc
}

fn cid() -> ToolCallId {
    ToolCallId::from("tc-bash-session-env")
}

fn noop_sink() -> ToolUpdateSink {
    Box::new(|_u: ToolUpdate| {})
}

fn first_text(r: &ToolResult) -> String {
    for c in &r.content {
        if let Content::Text { text, .. } = c {
            return text.clone();
        }
    }
    String::new()
}

/// Prints the five variables as `[..][..][..][..][..]`, with an unset variable rendering empty.
const PROBE: &str = r#"printf '[%s][%s][%s][%s][%s]\n' \
  "${CYRUP_SESSION_ID-}" "${CYRUP_SESSION_FILE-}" "${CYRUP_PROVIDER-}" \
  "${CYRUP_MODEL-}" "${CYRUP_REASONING_LEVEL-}""#;

async fn run(opts: BashOpts) -> String {
    let dir = tempfile::tempdir().unwrap();
    let bash = BashTool::new(proc(), ShellConfig::detect(), dir.path().to_path_buf(), opts);
    let r = bash
        .execute(cid(), serde_json::json!({ "command": PROBE }), CancelToken::new(), noop_sink())
        .await
        .unwrap();
    first_text(&r)
}

fn seeded_handle() -> SessionEnvHandle {
    SessionEnvHandle::new(SessionEnvInfo {
        session_id: Some("sess-abc123".to_string()),
        session_file: Some(PathBuf::from("/sessions/sess-abc123.jsonl")),
        provider: Some("anthropic".to_string()),
        model: Some("claude-opus-5".to_string()),
        reasoning_level: Some("medium".to_string()),
    })
}

/// A `bash` child sees the live session's metadata, by default.
#[tokio::test]
async fn bash_child_sees_the_live_session_metadata() {
    let handle = seeded_handle();
    let out = run(BashOpts { session_env: Some(handle), ..BashOpts::default() }).await;
    assert!(
        out.contains(
            "[sess-abc123][/sessions/sess-abc123.jsonl][anthropic][claude-opus-5][medium]"
        ),
        "got: {out}"
    );
}

/// The values are resolved when the command STARTS, so switching model or reasoning level affects
/// the very next `bash` call with no rebuild of the tool (environment-variables.md:27).
#[tokio::test]
async fn switching_model_affects_the_next_command() {
    let handle = seeded_handle();
    let opts = BashOpts { session_env: Some(handle.clone()), ..BashOpts::default() };

    let dir = tempfile::tempdir().unwrap();
    let bash = BashTool::new(proc(), ShellConfig::detect(), dir.path().to_path_buf(), opts);
    let call = async |bash: &BashTool| {
        let r = bash
            .execute(cid(), serde_json::json!({ "command": PROBE }), CancelToken::new(), noop_sink())
            .await
            .unwrap();
        first_text(&r)
    };

    let before = call(&bash).await;
    assert!(before.contains("[anthropic][claude-opus-5][medium]"), "got: {before}");

    handle.set_model("openai", "gpt-9");
    handle.set_reasoning_level("high");

    let after = call(&bash).await;
    assert!(
        after.contains("[openai][gpt-9][high]"),
        "the tool baked the metadata in at construction; got: {after}"
    );
}

/// An ephemeral (in-memory) session has no session file, and pi leaves the variable unset rather
/// than empty (`if (sessionFile)`, bash.ts:174; "unset for ephemeral sessions",
/// environment-variables.md:22). The remaining four are still published.
#[tokio::test]
async fn an_ephemeral_session_publishes_no_session_file() {
    let handle = SessionEnvHandle::new(SessionEnvInfo {
        session_id: Some("sess-ephemeral".to_string()),
        session_file: None,
        provider: Some("anthropic".to_string()),
        model: Some("claude-opus-5".to_string()),
        reasoning_level: Some("off".to_string()),
    });
    let out = run(BashOpts { session_env: Some(handle), ..BashOpts::default() }).await;
    assert!(out.contains("[sess-ephemeral][][anthropic][claude-opus-5][off]"), "got: {out}");
}

/// `exposeSessionEnvironment: false` suppresses the injection entirely (bash.ts:171).
#[tokio::test]
async fn the_exposure_flag_suppresses_the_injection() {
    let out = run(BashOpts {
        session_env: Some(seeded_handle()),
        expose_session_environment: false,
        ..BashOpts::default()
    })
    .await;
    assert!(out.contains("[][][][][]"), "got: {out}");
}

/// The prompt guideline is gated by the same flag, and defaults ON (bash.ts:322,329-331).
#[test]
fn the_prompt_guideline_tracks_the_exposure_flag() {
    let cwd = std::env::temp_dir();
    let on = BashTool::new(proc(), ShellConfig::detect(), cwd.clone(), BashOpts::default());
    assert_eq!(
        on.prompt_guidelines(),
        &["Inspect CYRUP_* environment variables for current model and session details."],
    );

    let off = BashTool::new(
        proc(),
        ShellConfig::detect(),
        cwd,
        BashOpts { expose_session_environment: false, ..BashOpts::default() },
    );
    assert!(off.prompt_guidelines().is_empty());
}
