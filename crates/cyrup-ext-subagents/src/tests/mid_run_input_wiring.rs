//! SUBA-192 / SUBA-198 — an operator's mid-run message reaches BOTH parts of this extension that
//! must know it, through the host's real `input` dispatch.
//!
//! Every test here loads the real [`crate::extension::SubagentsExtension`] into a real
//! [`cyrup_ext::ExtensionHost`] and sends the input with [`cyrup_ext::ExtensionHost::emit_input`] —
//! the call the session's submission pipeline makes for every steer, follow-up and prompt. That
//! puts the extension's SUBSCRIPTION under test as well as its handler: the host delivers an event
//! only to an extension that subscribed to its kind, so a handler with no `EventKind::Input` in
//! `init` is dead code however correct it is, and a test calling `on_event` directly could never
//! tell.
//!
//! The two consumers are read where they keep their state: the executor's open `bg_wait` set
//! ([`crate::background::wait::UserInputWaits`]) and the main watchdog's scope record.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use crate::extension::SubagentsExtension;
use cyrup_core::CancelToken;
use cyrup_ext::native::ExtMode;
use cyrup_ext::{InputEventSource, InputReduction, InputStreamingBehavior};

/// A real host with the real orchestrator-side extension loaded, and a handle on that extension.
async fn loaded() -> (
    tempfile::TempDir,
    cyrup_ext::ExtensionHost,
    Arc<SubagentsExtension>,
) {
    let root = tempfile::tempdir().expect("tempdir");
    let host = cyrup_ext::ExtensionHost::new(cyrup_ext::facade::HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: root.path().to_path_buf(),
    });
    let extension = Arc::new(SubagentsExtension::with_config_and_cwd(
        Default::default(),
        root.path().to_path_buf(),
    ));
    host.load_native(extension.clone())
        .await
        .expect("the subagents extension loads");
    (root, host, extension)
}

/// A steer typed while the agent streams ends the open `bg_wait` call AND joins the main
/// watchdog's scope record — from one `input` event, through the one shared admission rule — and
/// the extension hands the input on unchanged.
///
/// **Gutting mutations this fails on:** drop `EventKind::Input` from the extension's
/// subscriptions (the host never delivers the event: the wait stays open, the scope stays empty);
/// drop either consumer's call from the extension's `Input` arm (that consumer's assertion
/// fails).
#[tokio::test]
async fn a_steer_reaches_the_open_wait_and_the_watchdog_scope_through_the_host() {
    let (_root, host, extension) = loaded().await;
    let wait = extension.executor().user_input_waits().open();

    let reduction = host
        .emit_input(
            "Also fix the tests.",
            Vec::new(),
            InputEventSource::Interactive,
            Some(InputStreamingBehavior::Steer),
            &CancelToken::new(),
        )
        .await;

    assert!(
        matches!(reduction, InputReduction::Continue),
        "the extension observes the input; it must not handle, block or rewrite it: \
         {reduction:?}"
    );
    assert!(
        wait.token().is_cancelled(),
        "the steer must end the open bg_wait call"
    );
    assert_eq!(
        extension.watchdog().scope_prompts(),
        vec!["Also fix the tests."],
        "the steer must join the scope record the next review reads"
    );
}

/// The follow-up half, from an RPC client: a different source and a different streaming behavior,
/// the same outcome.
#[tokio::test]
async fn an_rpc_follow_up_reaches_both_consumers_too() {
    let (_root, host, extension) = loaded().await;
    let wait = extension.executor().user_input_waits().open();

    host.emit_input(
        "Then purge them.",
        Vec::new(),
        InputEventSource::Rpc,
        Some(InputStreamingBehavior::FollowUp),
        &CancelToken::new(),
    )
    .await;

    assert!(wait.token().is_cancelled());
    assert_eq!(
        extension.watchdog().scope_prompts(),
        vec!["Then purge them."]
    );
}

/// Input an extension sent, and input sent while the agent was idle, change NOTHING in either
/// consumer: no open wait ends, and nothing joins the scope record (an idle prompt is recorded by
/// `before_agent_start` instead, once).
///
/// **Gutting mutations this fails on:** make the shared rule (`mid_run_input::admits`) ignore the
/// source — the extension's steer then ends the wait and enters the scope; or ignore the streaming
/// behavior — the idle inputs do. Both consumers ask the one rule, so either change reaches both.
#[tokio::test]
async fn extension_and_idle_input_change_nothing_in_either_consumer() {
    let (_root, host, extension) = loaded().await;
    let wait = extension.executor().user_input_waits().open();

    for (text, source, behavior) in [
        (
            "Extension text.",
            InputEventSource::Extension,
            Some(InputStreamingBehavior::Steer),
        ),
        ("Idle prompt.", InputEventSource::Interactive, None),
        ("Idle RPC prompt.", InputEventSource::Rpc, None),
    ] {
        let reduction = host
            .emit_input(text, Vec::new(), source, behavior, &CancelToken::new())
            .await;
        assert!(matches!(reduction, InputReduction::Continue), "{text}");
    }

    assert!(
        !wait.token().is_cancelled(),
        "neither an extension's message nor idle input may end a wait"
    );
    assert!(
        extension.watchdog().scope_prompts().is_empty(),
        "neither may enter the scope record: {:?}",
        extension.watchdog().scope_prompts()
    );
}

/// SUBA-235 — a steer recorded before a compaction is still in the scope record the next review
/// reads. Upstream's compaction reset passes only `clearActivity` (`register-main.ts:418`
/// @ad11b7ab), so the prompts that authorized the work in progress keep authorizing it. Red at
/// the base, whose `SessionCompact` arm passed `clear_scope: true`: the record was empty.
#[tokio::test]
async fn a_steer_recorded_before_a_compaction_is_still_in_scope_after_it() {
    let (_root, host, extension) = loaded().await;
    host.emit_input(
        "Also fix the tests.",
        Vec::new(),
        InputEventSource::Interactive,
        Some(InputStreamingBehavior::Steer),
        &CancelToken::new(),
    )
    .await;
    assert_eq!(
        extension.watchdog().scope_prompts(),
        vec!["Also fix the tests."]
    );

    host.dispatcher()
        .dispatch_notify(
            &cyrup_ext::HostEvent::SessionCompact {
                compaction_entry: serde_json::json!({}),
                from_extension: false,
                reason: "threshold".to_string(),
                will_retry: false,
            },
            &CancelToken::new(),
        )
        .await;

    assert_eq!(
        extension.watchdog().scope_prompts(),
        vec!["Also fix the tests."],
        "a compaction must not forget what authorized the work in progress"
    );
}

/// The control for the test above, green at the base and after: a session switch DOES abandon the
/// scope record (`register-main.ts:416` @ad11b7ab passes `clearScope`), so keeping it across a
/// compaction is the compaction arm's own change and not a reset that clears nothing.
#[tokio::test]
async fn a_session_switch_still_clears_the_scope_record() {
    let (_root, host, extension) = loaded().await;
    host.emit_input(
        "Also fix the tests.",
        Vec::new(),
        InputEventSource::Interactive,
        Some(InputStreamingBehavior::Steer),
        &CancelToken::new(),
    )
    .await;
    host.dispatcher()
        .dispatch_notify(
            &cyrup_ext::HostEvent::SessionBeforeSwitch {
                reason: "new".to_string(),
                target_session_file: None,
            },
            &CancelToken::new(),
        )
        .await;
    assert!(extension.watchdog().scope_prompts().is_empty());
}
