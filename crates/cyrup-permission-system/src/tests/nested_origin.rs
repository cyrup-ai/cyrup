//! A gated call that another tool made while it ran (`ctx.executeTool`, a `codemode` script's
//! `tools.bash(...)`) passes the same gate as a model-issued one, but its prompt, its headless block
//! reason and its audit entry say where it came from.
//!
//! **[CYRUP-DELTA]** pi-permission-system v0.8.0 predates nested calls (`parentToolCallId`); its
//! prompts read the same for both. The labels are cyrup's, so these tests pin cyrup's wording.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::ToolCallId;
use cyrup_ext::{
    ExtMode, HookOutcome, HostCtx, HostEvent, HostServices, HumanInteractionLock, InitApi,
    NativeExtension,
};
use serde_json::{Value, json};

use crate::{
    AskChannel, AskOutcome, CHILD_ENV_VAR, ExtensionConfig, ManagerPaths, PermissionDecisionState,
    PermissionPromptDecision, PermissionSystemExtension, PromptOpts,
};

struct Registry;
impl HostServices for Registry {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(vec!["codemode".to_string(), "bash".to_string()])
    }
}

/// Answers "Allow Once" and records every prompt text it was shown.
struct Recording(Arc<Mutex<Vec<String>>>);

#[async_trait::async_trait]
impl AskChannel for Recording {
    async fn confirm(&self, _title: &str, message: &str, _opts: PromptOpts) -> AskOutcome {
        self.0.lock().unwrap().push(message.to_string());
        AskOutcome::Decided(PermissionPromptDecision {
            approved: true,
            state: PermissionDecisionState::Once,
            denial_reason: None,
            reject_script: false,
        })
    }
}

#[allow(clippy::unwrap_used)]
fn block_on<F: std::future::Future>(child: bool, body: F) -> F::Output {
    let _pin = child.then(|| crate::envx::pin(CHILD_ENV_VAR, Some("1")));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(body)
}

/// `bash` asks; everything else is allowed, so the parent `codemode` call itself is not prompted.
async fn ext_with_recording_channel(
    agent_dir: &Path,
) -> (PermissionSystemExtension, Arc<Mutex<Vec<String>>>) {
    ext_with_policy(
        agent_dir,
        r#"{ "tools": { "codemode": "allow" }, "bash": { "*": "ask" } }"#,
    )
    .await
}

async fn ext_with_policy(
    agent_dir: &Path,
    policy: &str,
) -> (PermissionSystemExtension, Arc<Mutex<Vec<String>>>) {
    let paths = policy_paths(agent_dir, policy);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let ext = extension_over(
        paths,
        Arc::new(Recording(Arc::clone(&prompts))),
        Arc::new(Registry),
    )
    .await;
    (ext, prompts)
}

fn policy_paths(agent_dir: &Path, policy: &str) -> ManagerPaths {
    let policy_path = agent_dir.join("cyrup-permissions.jsonc");
    std::fs::write(&policy_path, policy).unwrap();
    ManagerPaths {
        global_config_path: policy_path,
        agents_dir: agent_dir.join("agents"),
        project_global_config_path: None,
        project_agents_dir: None,
        legacy_global_settings_path: agent_dir.join("settings.json"),
        global_mcp_config_path: agent_dir.join("mcp.json"),
        mcp_server_names_override: None,
    }
}

async fn extension_over(
    paths: ManagerPaths,
    channel: Arc<dyn AskChannel>,
    services: Arc<dyn HostServices>,
) -> PermissionSystemExtension {
    let ext = PermissionSystemExtension::from_parts(paths, ExtensionConfig::default(), channel);
    ext.set_host_services(services);
    let mut api = InitApi::new();
    ext.init(&mut api).await.unwrap();
    ext
}

fn call(call_id: &str, name: &str, input: Value) -> HostEvent {
    HostEvent::ToolCall {
        call_id: ToolCallId::from(call_id),
        name: name.to_string(),
        input,
    }
}

fn ctx(cwd: &Path) -> HostCtx {
    HostCtx::event(ExtMode::Print, false, cwd.to_path_buf())
}

fn nested_ctx(cwd: &Path, parent: &str) -> HostCtx {
    ctx(cwd).with_parent_tool_call_id(ToolCallId::from(parent))
}

fn bash(command: &str) -> Value {
    json!({ "command": command })
}

#[test]
fn a_nested_ask_prompt_names_the_codemode_script_and_a_direct_one_does_not() {
    block_on(true, async {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let (ext, prompts) = ext_with_recording_channel(&cwd).await;
        // The model issues `codemode`; its script then calls `bash` as `<id>/1`.
        let parent = ext
            .on_event(
                &call("call-1", "codemode", json!({ "code": "x" })),
                &ctx(&cwd),
            )
            .await;
        assert!(matches!(parent, HookOutcome::Noop));
        let nested = ext
            .on_event(
                &call("call-1/1", "bash", bash("ls")),
                &nested_ctx(&cwd, "call-1"),
            )
            .await;
        assert!(
            matches!(nested, HookOutcome::Noop),
            "approved once: {nested:?}"
        );
        let direct = ext
            .on_event(&call("call-2", "bash", bash("ls")), &ctx(&cwd))
            .await;
        assert!(matches!(direct, HookOutcome::Noop));

        let shown = prompts.lock().unwrap().clone();
        assert_eq!(shown.len(), 2, "{shown:?}");
        assert!(
            shown[0].ends_with("(from codemode script)"),
            "the nested prompt must say where the call came from: {}",
            shown[0]
        );
        assert!(
            !shown[1].contains("from codemode script"),
            "a direct call's prompt is unchanged: {}",
            shown[1]
        );
    });
}

#[test]
fn a_nested_call_whose_parent_the_gate_never_saw_gets_the_generic_label() {
    block_on(true, async {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let (ext, prompts) = ext_with_recording_channel(&cwd).await;
        let outcome = ext
            .on_event(&call("zz/1", "bash", bash("ls")), &nested_ctx(&cwd, "zz"))
            .await;
        assert!(matches!(outcome, HookOutcome::Noop));
        let shown = prompts.lock().unwrap().clone();
        assert!(shown[0].ends_with("(from a nested tool call)"), "{shown:?}");
    });
}

#[test]
fn a_headless_nested_ask_blocks_with_a_reason_a_script_can_act_on() {
    // No child pin, no UI, no yolo: an `ask` has no one to reach.
    block_on(false, async {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let (ext, prompts) = ext_with_recording_channel(&cwd).await;
        ext.on_event(&call("call-1", "codemode", json!({})), &ctx(&cwd))
            .await;
        let nested = ext
            .on_event(
                &call("call-1/1", "bash", bash("ls")),
                &nested_ctx(&cwd, "call-1"),
            )
            .await;
        let HookOutcome::Block { reason, .. } = nested else {
            panic!("a headless ask must block: {nested:?}");
        };
        let reason = reason.unwrap();
        assert_eq!(
            reason,
            "Running bash command 'ls' requires approval, but no interactive UI is available (from codemode script). A script cannot answer an approval prompt without a UI: allow this call in the permission policy, or run interactively to be asked."
        );
        let direct = ext
            .on_event(&call("call-2", "bash", bash("ls")), &ctx(&cwd))
            .await;
        let HookOutcome::Block { reason, .. } = direct else {
            panic!("a headless direct ask blocks too");
        };
        assert_eq!(
            reason.unwrap(),
            "Running bash command 'ls' requires approval, but no interactive UI is available.",
            "a direct call keeps pi's wording"
        );
        assert!(prompts.lock().unwrap().is_empty(), "no one was asked");
    });
}

#[test]
fn the_audit_entry_of_a_nested_decision_names_the_call_that_made_it() {
    block_on(false, async {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let (ext, _) = ext_with_recording_channel(&cwd).await;
        ext.on_event(&call("call-1", "codemode", json!({})), &ctx(&cwd))
            .await;
        ext.on_event(
            &call("call-1/1", "bash", bash("ls")),
            &nested_ctx(&cwd, "call-1"),
        )
        .await;
        ext.on_event(&call("call-2", "bash", bash("pwd")), &ctx(&cwd))
            .await;
        let log = crate::logging::debug_path(&cwd.join("cyrup-permission-system").join("logs"));
        let entries: Vec<Value> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .filter(|e: &Value| e["event"] == "permission_request.blocked")
            .collect();
        assert_eq!(entries.len(), 2, "{entries:#?}");
        let nested = entries
            .iter()
            .find(|e| e["toolCallId"] == "call-1/1")
            .unwrap();
        assert_eq!(nested["parentToolCallId"], "call-1");
        assert!(
            nested["prompt"]
                .as_str()
                .unwrap()
                .ends_with("(from codemode script)"),
            "{nested}"
        );
        let direct = entries
            .iter()
            .find(|e| e["toolCallId"] == "call-2")
            .unwrap();
        assert!(
            direct.get("parentToolCallId").is_none(),
            "a direct call's entry keeps pi's shape: {direct}"
        );
    });
}

/// With `ask` on `codemode` itself there are two prompt layers: one to run the script, then one per
/// nested call that resolves to `ask`. Neither answer covers the other, and only the second says it
/// came from a script. (The policy semantics are unchanged: each call is gated by its own rule.)
#[test]
fn an_ask_on_codemode_and_on_bash_prompts_twice_and_only_the_nested_prompt_is_labelled() {
    block_on(true, async {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let (ext, prompts) = ext_with_policy(
            &cwd,
            r#"{ "tools": { "codemode": "ask" }, "bash": { "*": "ask" } }"#,
        )
        .await;
        let run = ext
            .on_event(
                &call("call-1", "codemode", json!({ "code": "x" })),
                &ctx(&cwd),
            )
            .await;
        assert!(matches!(run, HookOutcome::Noop), "{run:?}");
        let nested = ext
            .on_event(
                &call("call-1/1", "bash", bash("ls")),
                &nested_ctx(&cwd, "call-1"),
            )
            .await;
        assert!(matches!(nested, HookOutcome::Noop), "{nested:?}");
        let shown = prompts.lock().unwrap().clone();
        assert_eq!(shown.len(), 2, "one prompt per layer: {shown:?}");
        assert!(
            shown[0].contains("codemode") && !shown[0].contains("from codemode script"),
            "{shown:?}"
        );
        assert!(shown[1].ends_with("(from codemode script)"), "{shown:?}");
    });
}

// ---------------------------------------------------------------------------------------------
// [CYRUP-DELTA] "Reject All From This Script"
// ---------------------------------------------------------------------------------------------

/// What a scripted person did with one prompt; a prompt past the script is allowed once.
enum Answer {
    RejectScript,
}

/// Records `(prompt, whether the dialog offered to reject the whole script)` for every prompt, takes
/// `pause` to answer each (so that calls made meanwhile queue behind it), and answers as scripted.
struct Scripted {
    seen: Arc<Mutex<Vec<(String, bool)>>>,
    answers: Mutex<VecDeque<Answer>>,
    pause: Duration,
}

#[async_trait::async_trait]
impl AskChannel for Scripted {
    async fn confirm(&self, _title: &str, message: &str, opts: PromptOpts) -> AskOutcome {
        self.seen
            .lock()
            .unwrap()
            .push((message.to_string(), opts.offer_script_reject));
        tokio::time::sleep(self.pause).await;
        let answer = self.answers.lock().unwrap().pop_front();
        AskOutcome::Decided(match answer {
            Some(Answer::RejectScript) => PermissionPromptDecision {
                approved: false,
                state: PermissionDecisionState::Reject,
                denial_reason: Some("the user rejected this script's tool calls".to_string()),
                reject_script: true,
            },
            None => PermissionPromptDecision {
                approved: true,
                state: PermissionDecisionState::Once,
                denial_reason: None,
                reject_script: false,
            },
        })
    }
}

/// A registry that also hands out the one lock a person's attention is behind, as the live session
/// does: the prompts of calls made together queue on it and open one at a time.
struct LockedRegistry(Arc<HumanInteractionLock>);

impl HostServices for LockedRegistry {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(vec!["codemode".to_string(), "bash".to_string()])
    }
    fn human_interaction_lock(&self) -> Option<Arc<HumanInteractionLock>> {
        Some(Arc::clone(&self.0))
    }
}

type Seen = Arc<Mutex<Vec<(String, bool)>>>;

async fn ext_asking(
    agent_dir: &Path,
    answers: Vec<Answer>,
    pause: Duration,
) -> (PermissionSystemExtension, Seen) {
    let seen: Seen = Arc::default();
    let channel = Scripted {
        seen: Arc::clone(&seen),
        answers: Mutex::new(answers.into()),
        pause,
    };
    let paths = policy_paths(
        agent_dir,
        r#"{ "tools": { "codemode": "allow" }, "bash": { "*": "ask" } }"#,
    );
    let ext = extension_over(
        paths,
        Arc::new(channel),
        Arc::new(LockedRegistry(Arc::new(HumanInteractionLock::new()))),
    )
    .await;
    (ext, seen)
}

fn reason_of(outcome: HookOutcome) -> String {
    match outcome {
        HookOutcome::Block { reason, .. } => reason.unwrap_or_default(),
        other => panic!("expected a block: {other:?}"),
    }
}

/// Measured through the real TUI before this: a script over 25 commands under `bash = ask` was 25
/// dialogs; Esc answered one and the next appeared at once, and Ctrl+C rejected only the one on
/// screen. The dialog of a call a script made offers to refuse the script, and only that dialog does.
#[test]
fn the_dialog_of_a_nested_call_offers_to_reject_the_whole_script_and_a_direct_ones_does_not() {
    block_on(true, async {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let (ext, seen) = ext_asking(&cwd, Vec::new(), Duration::ZERO).await;
        ext.on_event(&call("call-1", "codemode", json!({})), &ctx(&cwd))
            .await;
        ext.on_event(
            &call("call-1/1", "bash", bash("ls")),
            &nested_ctx(&cwd, "call-1"),
        )
        .await;
        ext.on_event(&call("call-2", "bash", bash("pwd")), &ctx(&cwd))
            .await;

        let flags: Vec<bool> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(_, offered)| *offered)
            .collect();
        assert_eq!(flags, [true, false]);
    });
}

/// Refusing the script refuses the calls it makes afterwards without a dialog, for that script only.
#[test]
fn rejecting_the_script_refuses_its_later_calls_without_asking_and_no_other_script() {
    block_on(true, async {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let (ext, seen) = ext_asking(&cwd, vec![Answer::RejectScript], Duration::ZERO).await;
        ext.on_event(&call("call-1", "codemode", json!({})), &ctx(&cwd))
            .await;
        ext.on_event(&call("call-9", "codemode", json!({})), &ctx(&cwd))
            .await;

        let first = ext
            .on_event(
                &call("call-1/1", "bash", bash("echo 1")),
                &nested_ctx(&cwd, "call-1"),
            )
            .await;
        assert!(
            reason_of(first).contains("the user rejected this script's tool calls"),
            "the answer that refused the script"
        );
        let later = ext
            .on_event(
                &call("call-1/2", "bash", bash("echo 2")),
                &nested_ctx(&cwd, "call-1"),
            )
            .await;
        let later = reason_of(later);
        assert!(
            later.contains("echo 2")
                && later.contains("the user rejected this script's tool calls"),
            "a later call of the same script is refused with the user's refusal: {later}"
        );
        assert_eq!(seen.lock().unwrap().len(), 1, "and was not asked about");

        // Another script, and a direct call, are asked as before.
        let other = ext
            .on_event(
                &call("call-9/1", "bash", bash("echo 3")),
                &nested_ctx(&cwd, "call-9"),
            )
            .await;
        assert!(matches!(other, HookOutcome::Noop), "{other:?}");
        let direct = ext
            .on_event(&call("call-3", "bash", bash("echo 4")), &ctx(&cwd))
            .await;
        assert!(matches!(direct, HookOutcome::Noop), "{direct:?}");
        assert_eq!(seen.lock().unwrap().len(), 3);
    });
}

/// A script that starts its calls together (`Promise.allSettled`) has them queue behind the one
/// dialog on screen; the answer that refuses the script reaches the ones already waiting, not only
/// the ones that come later.
#[test]
fn rejecting_the_script_reaches_the_calls_already_waiting_for_their_turn() {
    block_on(true, async {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let (ext, seen) =
            ext_asking(&cwd, vec![Answer::RejectScript], Duration::from_millis(80)).await;
        ext.on_event(&call("call-1", "codemode", json!({})), &ctx(&cwd))
            .await;

        let nested = |n: u32| {
            let (ext, cwd) = (&ext, &cwd);
            async move {
                ext.on_event(
                    &call(&format!("call-1/{n}"), "bash", bash(&format!("echo {n}"))),
                    &nested_ctx(cwd, "call-1"),
                )
                .await
            }
        };
        let (a, b, c, d, e) = tokio::join!(nested(1), nested(2), nested(3), nested(4), nested(5));
        let outcomes = [a, b, c, d, e];

        assert_eq!(seen.lock().unwrap().len(), 1, "one dialog for five calls");
        assert!(
            outcomes
                .into_iter()
                .all(|outcome| reason_of(outcome)
                    .contains("the user rejected this script's tool calls")),
            "every call of the script was refused"
        );
    });
}

/// The scripts remembered as refused are bounded, newest kept: a session does not grow a set for
/// every script a person ever refused.
#[test]
fn only_the_most_recently_refused_scripts_are_remembered() {
    block_on(true, async {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let answers = (0..100).map(|_| Answer::RejectScript).collect();
        let (ext, seen) = ext_asking(&cwd, answers, Duration::ZERO).await;
        for n in 0..100 {
            let parent = format!("s{n}");
            let outcome = ext
                .on_event(
                    &call(&format!("{parent}/1"), "bash", bash(&format!("echo {n}"))),
                    &nested_ctx(&cwd, &parent),
                )
                .await;
            assert!(matches!(outcome, HookOutcome::Block { .. }), "{outcome:?}");
        }
        assert_eq!(seen.lock().unwrap().len(), 100);

        // The newest is still refused without a dialog; the oldest has been forgotten and is asked.
        let newest = ext
            .on_event(
                &call("s99/2", "bash", bash("echo again")),
                &nested_ctx(&cwd, "s99"),
            )
            .await;
        assert!(matches!(newest, HookOutcome::Block { .. }), "{newest:?}");
        assert_eq!(seen.lock().unwrap().len(), 100, "s99 was not asked about");
        let oldest = ext
            .on_event(
                &call("s0/2", "bash", bash("echo again")),
                &nested_ctx(&cwd, "s0"),
            )
            .await;
        assert!(matches!(oldest, HookOutcome::Noop), "{oldest:?}");
        assert_eq!(seen.lock().unwrap().len(), 101, "s0 was asked about again");
    });
}
