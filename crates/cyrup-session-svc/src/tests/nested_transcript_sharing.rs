//! CODE-045 — the calls a tool makes while it runs are shown the agent's transcript as the SAME
//! shared handle, not as a copy each.
//!
//! `Agent::nested_context` hands out one allocation of the transcript (and of the system prompt,
//! which this agent leaves empty: the session writes it into the transcript as system rows) until
//! the transcript changes (`cyrup-agent` `tests/nested_context.rs` pins that primitive). What that test cannot see
//! is whether the session's nested host asks for it: `session/nested.rs` had used
//! `Agent::snapshot()`, which copies the whole transcript and wraps every message again for every
//! call, so a script that made 3000 calls held the process at 3.4 GB. Reverting the host to the
//! snapshot passes every other test, because a copy and a shared handle read the same.
//!
//! The one place a nested call's context is visible is the hook it runs through, so this test makes
//! two nested calls AT THE SAME TIME through a hook that records what it is handed and waits for
//! the other call. Two copies are two live allocations at two addresses; the shared handle is one.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use cyrup_agent::{BeforeOutcome, BeforeToolCall, Hooks, NestedToolCallOptions};
use cyrup_core::{
    CancelToken, Content, StopReason, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::Barrier;

use crate::{AgentSession, SessionBuilder, SessionConfig};

/// What a nested call's hook was handed: the address of the transcript and its length.
#[derive(Clone, Copy, Debug)]
struct Seen {
    messages: usize,
    length: usize,
}

/// Records the context of every nested call, then waits until the other call has recorded its own,
/// so that both are alive at once.
struct Capture {
    seen: Mutex<Vec<Seen>>,
    both_here: Barrier,
}

#[async_trait::async_trait]
impl Hooks for Capture {
    async fn before_tool_call(
        &self,
        ctx: BeforeToolCall<'_>,
        _cancel: CancelToken,
    ) -> BeforeOutcome {
        self.seen.lock().unwrap().push(Seen {
            messages: ctx.context.messages.as_ptr() as usize,
            length: ctx.context.messages.len(),
        });
        self.both_here.wait().await;
        BeforeOutcome::Proceed
    }
}

type SessionSlot = Arc<OnceLock<Arc<AgentSession>>>;

fn schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": true })
}

/// A tool that does nothing, for the nested calls to call.
struct Leaf {
    params: Value,
}

#[async_trait::async_trait]
impl Tool for Leaf {
    fn name(&self) -> &str {
        "leaf"
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            content: vec![Content::text("leaf ran")],
            ..Default::default()
        })
    }
}

/// The tool the model calls. While it runs it makes two nested calls to `leaf` at the same time,
/// through the capturing hook, as a codemode script's `Promise.all` does.
struct Probe {
    session: SessionSlot,
    capture: Arc<Capture>,
    outcomes: Arc<Mutex<Vec<bool>>>,
    params: Value,
}

#[async_trait::async_trait]
impl Tool for Probe {
    fn name(&self) -> &str {
        "probe"
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        call_id: ToolCallId,
        _params: Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let session = self.session.get().expect("session is set before the run");
        let call = || {
            session.execute_nested_tool_through(
                Arc::clone(&self.capture) as Arc<dyn Hooks>,
                &call_id,
                "leaf",
                json!({}),
                NestedToolCallOptions {
                    cancel: Some(cancel.clone()),
                    on_update: None,
                },
            )
        };
        // A call that never reaches the barrier (the other was serialised away) must fail the
        // test, not hang it.
        let both = tokio::time::timeout(Duration::from_secs(20), async {
            tokio::join!(call(), call())
        })
        .await
        .expect("both nested calls reached the hook at the same time");
        let mut outcomes = self.outcomes.lock().unwrap();
        outcomes.push(!both.0.is_error);
        outcomes.push(!both.1.is_error);
        Ok(ToolResult {
            content: vec![Content::text("probe done")],
            ..Default::default()
        })
    }
}

fn model_calls_probe_then_answers() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        FauxResponseStep::factory(|_ctx, _o, _s, _m| {
            faux_assistant_message(
                vec![faux_tool_call("probe".to_string(), json!({}))],
                StopReason::ToolUse,
            )
        }),
        FauxResponseStep::factory(|_ctx, _o, _s, _m| {
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
        }),
    ]);
    faux
}

#[tokio::test]
async fn nested_calls_made_against_one_transcript_are_shown_the_same_allocation_of_it() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();

    let slot: SessionSlot = Arc::new(OnceLock::new());
    let capture = Arc::new(Capture {
        seen: Mutex::new(Vec::new()),
        both_here: Barrier::new(2),
    });
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg.custom_tools = vec![
        Arc::new(Probe {
            session: Arc::clone(&slot),
            capture: Arc::clone(&capture),
            outcomes: Arc::clone(&outcomes),
            params: schema(),
        }) as Arc<dyn Tool>,
        Arc::new(Leaf { params: schema() }),
    ];
    let session = SessionBuilder::new(model_calls_probe_then_answers() as Arc<dyn Provider>, cfg)
        .build()
        .await
        .unwrap()
        .into_shared();
    let _ = slot.set(Arc::clone(&session));
    let mut names = session.active_tool_names();
    names.extend(["probe".to_string(), "leaf".to_string()]);
    session.set_active_tools_by_name(&names).await;

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    assert_eq!(
        *outcomes.lock().unwrap(),
        [true, true],
        "both nested calls ran their tool"
    );
    let seen = capture.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    // The transcript the calls were shown is the model's turn (at least the prompt and the
    // assistant message the call is attributed to), not an empty stand-in.
    assert!(seen[0].length >= 2, "{seen:?}");
    assert_eq!(seen[0].length, seen[1].length, "{seen:?}");
    assert_eq!(
        seen[0].messages, seen[1].messages,
        "two nested calls were each shown a copy of the transcript: {seen:?}"
    );
}
