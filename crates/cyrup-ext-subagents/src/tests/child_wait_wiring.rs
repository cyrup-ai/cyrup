//! SUBA-232 / SUBA-231 — a subagent child's OWN `bg_wait`, and the child's `input` edge, through a
//! real [`cyrup_ext::ExtensionHost`].
//!
//! Every runtime here is built by the production [`crate::prompt_runtime::prompt_runtime_from_env`]
//! from the variables a real spawn writes, and every input is sent with
//! [`cyrup_ext::ExtensionHost::emit_input`] — the call the child session's submission pipeline makes
//! for the steering inbox's `send_user_message` — so the runtime's SUBSCRIPTION is under test as
//! well as its handler.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashMap;
use std::sync::Arc;

use cyrup_core::CancelToken;
use cyrup_ext::native::ExtMode;
use cyrup_ext::{InputEventSource, InputReduction, InputStreamingBehavior};

use crate::prompt_runtime::{
    INHERIT_PROJECT_CONTEXT_ENV, SubagentPromptRuntime, prompt_runtime_from_env,
};
use crate::spawn::nested_events::{CHILD_ENV, FANOUT_CHILD_ENV};

/// Upstream's child sentence (`wait-tool.ts:19` @ad11b7ab), its opening clause.
const CHILD_SENTENCE: &str =
    "This child runtime does not install the root session's native completion notifier.";

fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    move |key| map.get(key).cloned()
}

fn host(root: &std::path::Path) -> cyrup_ext::ExtensionHost {
    cyrup_ext::ExtensionHost::new(cyrup_ext::facade::HostConfig {
        mode: ExtMode::Tui,
        has_ui: false,
        cwd: root.to_path_buf(),
    })
}

/// The `CHILD_WATCHDOG_CONFIG_ENV` payload a parent writes for an armed child.
fn child_watchdog_config() -> String {
    serde_json::json!({
        "enabled": true,
        "runId": "run-1",
        "agent": "worker",
        "childIndex": 0,
        "watchdogTailTimeoutMs": 120_000,
        "agentEndTimeoutMs": 5_000,
        "maxWarnings": null,
        "lsp": { "enabled": false, "timeoutMs": 1_000, "maxFiles": 1, "maxDiagnostics": 0 },
        "autoFollowBlockers": true,
        "autoFollowMaxAttempts": 3,
        "stalemateRepeats": 3,
    })
    .to_string()
}

/// SUBA-232 — every real child (`CYRUP_SUBAGENT_CHILD=1`, which the parent writes on every spawn)
/// registers its OWN `bg_wait`, owned by the prompt runtime and described for a child (pi
/// `subagent-prompt-runtime.ts:520`, `wait-tool.ts:19` @ad11b7ab) — fan-out or plain alike.
///
/// **Gutting mutation this fails on:** drop the registration from the runtime's `init` — the
/// registry then has no `bg_wait`.
#[tokio::test]
async fn a_real_child_registers_its_own_bg_wait_with_the_child_description() {
    for fanout in ["0", "1"] {
        let root = tempfile::tempdir().expect("tempdir");
        let host = host(root.path());
        let runtime = prompt_runtime_from_env(&lookup(&[
            (CHILD_ENV, "1"),
            (FANOUT_CHILD_ENV, fanout),
            (INHERIT_PROJECT_CONTEXT_ENV, "1"),
        ]))
        .expect("no tool budget, so the runtime builds")
        .expect("a child env builds the runtime");
        host.load_native(Arc::new(runtime))
            .await
            .expect("the runtime loads");

        let tool = host
            .registry()
            .tool("bg_wait")
            .expect("registry readable")
            .unwrap_or_else(|| panic!("fanout={fanout}: a real child must have bg_wait"));
        assert!(
            tool.description().contains(CHILD_SENTENCE),
            "fanout={fanout}: {}",
            tool.description()
        );
        assert_eq!(
            host.registry()
                .tool_owner("bg_wait")
                .expect("registry readable")
                .map(|id| id.to_string())
                .as_deref(),
            Some(crate::prompt_runtime::PROMPT_RUNTIME_EXTENSION_ID),
            "fanout={fanout}"
        );
    }
}

/// The other half of the gate: a process WITHOUT the child flag — a top-level session, whose
/// `bg_wait` is the subagents extension's `Full` arm — is never handed a second one, even when
/// other child-runtime halves arm.
///
/// **Gutting mutation this fails on:** build the child wait without the `CHILD_ENV` check — this
/// runtime then registers `bg_wait` too.
#[tokio::test]
async fn a_process_without_the_child_flag_registers_no_bg_wait() {
    let root = tempfile::tempdir().expect("tempdir");
    let host = host(root.path());
    let runtime = prompt_runtime_from_env(&lookup(&[(INHERIT_PROJECT_CONTEXT_ENV, "1")]))
        .expect("no tool budget, so the runtime builds")
        .expect("an inherit flag alone arms the runtime");
    host.load_native(Arc::new(runtime))
        .await
        .expect("the runtime loads");
    assert!(
        host.registry()
            .tool("bg_wait")
            .expect("registry readable")
            .is_none(),
        "a process that is not a subagent child must not get the child's bg_wait"
    );
}

/// A child with no other half armed — no inherited context, no budget, no watchdog — still gets
/// the runtime and its `bg_wait`: the wait is a job of its own, and upstream loads the runtime
/// into every child.
///
/// **Gutting mutation this fails on:** leave the wait out of `is_inert` — such a child then gets
/// no runtime at all, and so no `bg_wait`.
#[tokio::test]
async fn a_child_with_nothing_else_armed_still_gets_its_bg_wait() {
    let root = tempfile::tempdir().expect("tempdir");
    let host = host(root.path());
    let runtime = prompt_runtime_from_env(&lookup(&[(CHILD_ENV, "1")]))
        .expect("no tool budget, so the runtime builds")
        .expect("the child's wait alone arms the runtime");
    host.load_native(Arc::new(runtime))
        .await
        .expect("the runtime loads");
    assert!(
        host.registry()
            .tool("bg_wait")
            .expect("registry readable")
            .is_some(),
        "a real child must have bg_wait even with nothing else armed"
    );
}

/// A real child, armed with a watchdog, and the handle on it.
async fn armed_child(
    root: &std::path::Path,
) -> (cyrup_ext::ExtensionHost, Arc<SubagentPromptRuntime>) {
    let config = child_watchdog_config();
    let runtime = Arc::new(
        prompt_runtime_from_env(&lookup(&[
            (CHILD_ENV, "1"),
            (FANOUT_CHILD_ENV, "1"),
            (
                crate::watchdog::child_status::CHILD_WATCHDOG_CONFIG_ENV,
                config.as_str(),
            ),
        ]))
        .expect("no tool budget, so the runtime builds")
        .expect("a child env builds the runtime"),
    );
    let host = host(root);
    host.load_native(runtime.clone())
        .await
        .expect("the runtime loads");
    (host, runtime)
}

/// SUBA-232 + SUBA-231 — a parent's steer, delivered into the child while it streams, ends the
/// child's open `bg_wait` (upstream's own `wait-tool.ts:31-35` listener in a child) AND joins an
/// armed child watchdog's scope record (beyond upstream: `register-child.ts:99-134` subscribes no
/// `input`) — from one `input` event, through the one shared admission rule — and the runtime hands
/// the input on unchanged.
///
/// The steering inbox delivers with `send_user_message`, which the host labels `interactive`; that
/// is the source sent here.
///
/// **Gutting mutations this fails on:** drop `EventKind::Input` from the runtime's subscriptions
/// (the host never delivers it: the wait stays open, the scope stays empty); drop either call from
/// the runtime's `Input` arm (that consumer's assertion fails).
#[tokio::test]
async fn a_parents_steer_ends_the_childs_open_wait_and_joins_its_watchdogs_scope() {
    let root = tempfile::tempdir().expect("tempdir");
    let (host, runtime) = armed_child(root.path()).await;
    let wait = runtime
        .child_user_input_waits()
        .expect("a real child carries the wait's user-input set")
        .open();

    let reduction = host
        .emit_input(
            "also update the docs",
            Vec::new(),
            InputEventSource::Interactive,
            Some(InputStreamingBehavior::Steer),
            &CancelToken::new(),
        )
        .await;

    assert!(
        matches!(reduction, InputReduction::Continue),
        "the runtime observes the input; it must not handle, block or rewrite it: {reduction:?}"
    );
    assert!(
        wait.token().is_cancelled(),
        "the parent's steer must end the child's open bg_wait"
    );
    assert_eq!(
        runtime
            .watchdog()
            .expect("the child watchdog is armed")
            .runtime()
            .scope_prompts(),
        vec!["also update the docs"],
        "the steer must join the child's scope record"
    );
}

/// The shared rule's other half, in the child: input an EXTENSION sent, and input with no streaming
/// behavior (an idle child, or a busy submission the host refuses), end no child wait and record
/// nothing.
///
/// **Gutting mutation this fails on:** end every open wait in the `Input` arm without asking the
/// shared rule — the extension's steer then ends the wait.
#[tokio::test]
async fn extension_and_idle_input_change_nothing_in_the_child() {
    let root = tempfile::tempdir().expect("tempdir");
    let (host, runtime) = armed_child(root.path()).await;
    let wait = runtime
        .child_user_input_waits()
        .expect("a real child carries the wait's user-input set")
        .open();

    for (source, behavior) in [
        (
            InputEventSource::Extension,
            Some(InputStreamingBehavior::Steer),
        ),
        (InputEventSource::Interactive, None),
    ] {
        host.emit_input(
            "not a mid-run message from the operator",
            Vec::new(),
            source,
            behavior,
            &CancelToken::new(),
        )
        .await;
    }

    assert!(!wait.token().is_cancelled(), "neither input ends the wait");
    assert!(
        runtime
            .watchdog()
            .expect("the child watchdog is armed")
            .runtime()
            .scope_prompts()
            .is_empty(),
        "neither input joins the scope record"
    );
}
