//! A session replacement (`/new`, RPC `new_session`, a second ACP `session/new`) over the real launch
//! wiring.
//!
//! The runtime builds the replacement first, binds its own `LiveHostServices` to the SAME native
//! extension objects the first session used, shuts the outgoing session down and starts the new one.
//! An extension that kept the first backend it was given shaped, injected into and notified the
//! session that had been replaced. What the model is offered is the visible edge of that: the
//! permission system re-derives the active tool set at the start of every prompt, on the backend it
//! holds.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use cyrup_core::StopReason;
use cyrup_provider::faux::{FauxProvider, FauxResponseStep, faux_assistant_message, faux_text};
use cyrup_session_svc::AgentSessionRuntime;

use super::tests::factory_with;

/// A provider that answers `turns` times with text, recording the tool names each request declared.
fn recording_provider(declared: &Arc<Mutex<Vec<Vec<String>>>>, turns: usize) -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(
        (0..turns)
            .map(|_| {
                let seen = Arc::clone(declared);
                FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                    let mut names: Vec<String> =
                        ctx.tools.iter().map(|tool| tool.name.clone()).collect();
                    names.sort();
                    seen.lock().unwrap().push(names);
                    faux_assistant_message(vec![faux_text("noted")], StopReason::Stop)
                })
            })
            .collect(),
    );
    faux
}

/// With a permission policy that denies `bash`, the tool set the model is offered is shaped by the
/// permission system at the start of each prompt. After a session replacement the new session is
/// offered the same set the first one was: `codemode`, `grep`, `find`, `ls`, `powershell` and
/// `tool_search` stay, and the denied `bash` is not declared again. Measured through the real binary
/// before the fix: the first request declared eleven tools (no `bash`), the request after
/// `new_session` declared `read, bash, edit, write, mcp, ask_user_question`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replaced_session_is_offered_the_tool_set_the_permission_policy_shapes() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("cyrup-permissions.jsonc"),
        r#"{ "tools": { "bash": "deny" } }"#,
    )
    .unwrap();

    let declared: Arc<Mutex<Vec<Vec<String>>>> = Arc::default();
    let provider = recording_provider(&declared, 2);
    let (factory, target) = factory_with(provider, &agent_dir, &cwd, false, |_| {});
    let runtime: Arc<AgentSessionRuntime> =
        AgentSessionRuntime::create(factory, target).await.unwrap();

    let first = runtime.session().await;
    let _ = first.prompt("hello").await.unwrap();
    first.wait_for_idle().await;
    let swapped = runtime.new_session().await.unwrap();
    assert!(!swapped.cancelled, "no extension vetoed the replacement");
    let second = runtime.session().await;
    let _ = second.prompt("hello again").await.unwrap();
    second.wait_for_idle().await;

    let declared = declared.lock().unwrap().clone();
    assert_eq!(declared.len(), 2, "one request per prompt: {declared:?}");
    assert!(
        declared[0].iter().any(|name| name == "codemode"),
        "an armed permission system turns codemode on: {:?}",
        declared[0]
    );
    assert!(
        !declared[0].iter().any(|name| name == "bash"),
        "the denied tool is not declared: {:?}",
        declared[0]
    );
    assert_eq!(
        declared[1], declared[0],
        "the replacement session is offered the tools the first one was"
    );
}
