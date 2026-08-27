//! The SCRUB half of pi's `resolveSpawnContext` (TOOL-008 / TOOL-028), in its own test binary
//! because it mutates the process environment.
//!
//! `pi/packages/coding-agent/src/core/tools/bash.ts:164-170` materializes the child environment and
//! then, UNCONDITIONALLY — before and regardless of `exposeSessionEnvironment` — deletes
//! `PI_SESSION_ID`, `PI_SESSION_FILE`, `PI_PROVIDER`, `PI_MODEL` and `PI_REASONING_LEVEL`. That
//! delete is what stops a stale value inherited from a parent harness process from being read by a
//! child script as if it described the current session. A cyrup subagent run is a real re-exec of
//! the `cyrup` binary, so the inheritance path is real, not hypothetical.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, unsafe_code)]

use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolResult, ToolUpdate, ToolUpdateSink};
use cyrup_tools::config::BashOpts;
use cyrup_tools::ops::{Backend, ProcOps};
use cyrup_tools::tools::BashTool;
use std::sync::Arc;

fn proc() -> Arc<dyn ProcOps> {
    Backend::default().proc
}

fn cid() -> ToolCallId {
    ToolCallId::from("tc-bash-env-scrub")
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

async fn run(opts: BashOpts, command: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let bash = BashTool::new(
        proc(),
        dir.path().to_path_buf(),
        opts,
    );
    let r = bash
        .execute(
            cid(),
            serde_json::json!({ "command": command }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();
    first_text(&r)
}

/// Both halves live in ONE test so the process environment is mutated from a single thread: this
/// is the only `#[test]` in this binary, and every `set_var` happens before any child is spawned.
///
/// Part 1 — every session-metadata key is deleted from the child environment before anything is
/// set, so a value the harness process itself inherited can never leak into a `bash` child, with
/// the exposure flag ON or OFF.
/// Part 2 (TOOL-028) — the spawn seam has a real deletion channel, so a hook can remove a variable
/// the parent exported; pi's hook receives a fully materialized env and can `delete` from it
/// (`bash.ts:156`, `docs/extensions.md:2122`).
#[tokio::test]
async fn session_metadata_is_scrubbed_and_hooks_can_delete() {
    // SAFETY: single-threaded with respect to the environment — see the doc comment above.
    unsafe {
        std::env::set_var("CYRUP_SESSION_ID", "stale-cyrup-session");
        std::env::set_var("CYRUP_SESSION_FILE", "/stale/cyrup.jsonl");
        std::env::set_var("CYRUP_PROVIDER", "stale-cyrup-provider");
        std::env::set_var("CYRUP_MODEL", "stale-cyrup-model");
        std::env::set_var("CYRUP_REASONING_LEVEL", "stale-cyrup-level");
        std::env::set_var("PI_SESSION_ID", "stale-pi-session");
        std::env::set_var("PI_SESSION_FILE", "/stale/pi.jsonl");
        std::env::set_var("PI_PROVIDER", "stale-pi-provider");
        std::env::set_var("PI_MODEL", "stale-pi-model");
        std::env::set_var("PI_REASONING_LEVEL", "stale-pi-level");
        std::env::set_var("CYRUP_TOOL008_SECRET", "leaked");
    }

    let probe = r#"for v in CYRUP_SESSION_ID CYRUP_SESSION_FILE CYRUP_PROVIDER CYRUP_MODEL CYRUP_REASONING_LEVEL PI_SESSION_ID PI_SESSION_FILE PI_PROVIDER PI_MODEL PI_REASONING_LEVEL; do
  eval "printf '%s=[%s]\n' \"$v\" \"\${$v-}\""
done"#;

    // No session metadata is available at all (the default `BashOpts`): the child must still see
    // none of the stale values.
    let out = run(BashOpts::default(), probe).await;
    assert!(
        !out.contains("stale-"),
        "a stale value reached the child:\n{out}"
    );

    // And with the exposure flag explicitly OFF — pi deletes before it even consults the flag.
    let out = run(
        BashOpts {
            expose_session_environment: false,
            ..BashOpts::default()
        },
        probe,
    )
    .await;
    assert!(
        !out.contains("stale-"),
        "a stale value reached the child with exposure off:\n{out}"
    );

    // ---- part 2: a spawn hook can delete an inherited variable (TOOL-028) ----
    let probe = "printf '[%s]\\n' \"${CYRUP_TOOL008_SECRET-ABSENT}\"";

    // Without a hook the child inherits it (this is the baseline the redaction must beat).
    let out = run(BashOpts::default(), probe).await;
    assert!(
        out.contains("[leaked]"),
        "fixture: the child should inherit it, got:\n{out}"
    );

    let hook: cyrup_tools::config::BashSpawnHook =
        Arc::new(|mut ctx: cyrup_tools::config::BashSpawnContext| {
            ctx.env_remove.push("CYRUP_TOOL008_SECRET".to_string());
            ctx
        });
    let out = run(
        BashOpts {
            spawn_hook: Some(hook),
            ..BashOpts::default()
        },
        probe,
    )
    .await;
    assert!(
        out.contains("[ABSENT]"),
        "the hook's removal was ignored, got:\n{out}"
    );
}
