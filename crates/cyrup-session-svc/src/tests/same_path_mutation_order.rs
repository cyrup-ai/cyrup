//! DoD 1 — a turn emitting `write(p, A)` then `write(p, B)` must leave `p` containing **B**.
//!
//! The whole point of the task, on the whole path: a real `SessionBuilder::build()`, the faux
//! provider issuing both calls in ONE assistant message, `execute_parallel` spawning both bodies,
//! the real `write`/`edit` tools, and the real process-global `FileMutationLocks`. Four separate
//! mechanisms have to hold for this to pass — the oneshot start chain (`exec.rs:177-181`), the
//! `MUTATION_REGISTRATION` chain (`FileMutationLocks::guard`, `cyrup-tools/src/lock.rs`),
//! `enqueue`'s never-yield property (`cyrup-core/src/keyed_lock.rs:164-190`), and `guard()` being
//! the first `.await` of both tool bodies — and each is pinned individually elsewhere. This is
//! where they are pinned TOGETHER, because their composition is the user-visible guarantee and
//! nothing else asserts it.
//!
//! No sleeps and no retries: given all four, the outcome is determined, not likely.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;

use crate::{AgentSession, SessionBuilder, SessionConfig};
use cyrup_core::message::Message;
use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use cyrup_provider::Provider;
use serde_json::{json, Value};
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

/// A session whose provider issues `calls` as ONE assistant message — so the loop takes the
/// PARALLEL batch path — and then stops.
async fn session_issuing(fx: &Fixture, calls: Vec<(&str, Value)>) -> AgentSession {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(
            calls.into_iter().map(|(name, args)| faux_tool_call(name, args)).collect(),
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux as Arc<dyn Provider>;
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    SessionBuilder::new(provider, cfg).build().await.expect("build")
}

/// Every tool result in the transcript as `(tool_name, is_error)`, in transcript order.
fn tool_results(messages: &[Message]) -> Vec<(String, bool)> {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult { tool_name, is_error, .. } => {
                Some((tool_name.clone(), *is_error))
            }
            _ => None,
        })
        .collect()
}

async fn run(fx: &Fixture, calls: Vec<(&str, Value)>) -> Vec<(String, bool)> {
    let session = session_issuing(fx, calls).await;
    let _events = session.prompt("mutate the file").await.expect("prompt");
    session.wait_for_idle().await;
    tool_results(&session.messages().await)
}

/// DoD 1, two calls. The LAST payload the model issued must be the one on disk.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_then_write_to_one_path_leaves_the_second_payload() {
    let fx = fixture();
    let results = run(
        &fx,
        vec![
            ("write", json!({ "path": "out.txt", "content": "A" })),
            ("write", json!({ "path": "out.txt", "content": "B" })),
        ],
    )
    .await;

    assert_eq!(results.len(), 2, "both calls must produce a tool result: {results:?}");
    assert!(
        results.iter().all(|(_, is_error)| !*is_error),
        "a mutation failed, so the content assertion below would prove nothing: {results:?}"
    );
    assert_eq!(
        std::fs::read_to_string(fx.cwd.join("out.txt")).unwrap(),
        "B",
        "the model issued `write(A)` then `write(B)`; the LAST payload must survive \
         (pi file-mutation-queue.ts:5/:33/:46-49). Got `A` ⇒ the two mutations were granted in \
         the wrong order somewhere between `execute_parallel` and `KeyedLocks::enqueue`"
    );
}

/// The same guarantee with THREE calls, so a LIFO inversion is unambiguous: a two-call batch that
/// inverts looks like a swap, a three-call batch that inverts lands on `"A"`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_writes_to_one_path_leave_the_last_payload() {
    let fx = fixture();
    let results = run(
        &fx,
        vec![
            ("write", json!({ "path": "out.txt", "content": "A" })),
            ("write", json!({ "path": "out.txt", "content": "B" })),
            ("write", json!({ "path": "out.txt", "content": "C" })),
        ],
    )
    .await;

    assert_eq!(results.len(), 3, "all three calls must produce a tool result: {results:?}");
    assert!(
        results.iter().all(|(_, is_error)| !*is_error),
        "a mutation failed, so the content assertion below would prove nothing: {results:?}"
    );
    assert_eq!(
        std::fs::read_to_string(fx.cwd.join("out.txt")).unwrap(),
        "C",
        "the model issued `write(A)`, `write(B)`, `write(C)`; the LAST payload must survive. \
         Got `A` ⇒ the batch started its bodies in LIFO order (tokio parks each newly spawned \
         task in the worker's LIFO slot); got `B` ⇒ some other inversion between \
         `execute_parallel` and `KeyedLocks::enqueue`"
    );
}

/// The mixed case, which fails LOUDLY under inversion rather than quietly: an `edit` granted
/// before the `write` that creates its input finds no `L1` at all and returns an error result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_then_edit_of_one_path_applies_the_edit_to_the_write() {
    let fx = fixture();
    let results = run(
        &fx,
        vec![
            ("write", json!({ "path": "notes.txt", "content": "L1\n" })),
            (
                "edit",
                json!({
                    "path": "notes.txt",
                    "edits": [{ "oldText": "L1", "newText": "L2" }],
                }),
            ),
        ],
    )
    .await;

    assert_eq!(results.len(), 2, "both calls must produce a tool result: {results:?}");
    let edit = results
        .iter()
        .find(|(name, _)| name == "edit")
        .unwrap_or_else(|| panic!("the run must contain an `edit` tool result: {results:?}"));
    assert!(
        !edit.1,
        "the `edit` was granted the mutation lock BEFORE the `write` that creates its input, so \
         `L1` was not there to replace: {results:?}"
    );
    assert_eq!(
        std::fs::read_to_string(fx.cwd.join("notes.txt")).unwrap(),
        "L2\n",
        "`write(\"L1\\n\")` then `edit(L1 -> L2)` must compose in that order"
    );
}
