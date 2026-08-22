# cyrup-agent

The turn-based agent loop: an ordered event stream, parallel and sequential tool execution, the
`Hooks` mutating seam alongside the notify-only `EventSubscriber`, steering and follow-up queues,
abort/idle, and managed agent state.

The loop is provider-agnostic. It talks to a `StreamFn`, and `ProviderStreamFn` wraps any
`cyrup_provider::Provider` to satisfy that trait — so the same loop drives a live provider, a proxy
transport, or a scripted fake without changing. The crate is `#![forbid(unsafe_code)]` and runs
under the workspace no-panic policy.

Entry point: `Agent`, built via `AgentBuilder`.

## Quickstart

```rust
use std::sync::Arc;

use cyrup_agent::{Agent, AgentError, ModelRef, Provider, ProviderStreamFn, StopReason, StreamFn};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};

#[tokio::main]
async fn main() -> Result<(), AgentError> {
    // Any `Provider` works here; the faux one keeps this example offline.
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("hello")], StopReason::Stop)]);
    let provider: Arc<dyn Provider> = faux.clone();
    let stream_fn: Arc<dyn StreamFn> = Arc::new(ProviderStreamFn::new(provider));

    let model =
        ModelRef { provider: "faux".into(), api: Some("faux".into()), model: "faux-1".into() };
    let agent = Agent::builder(model, stream_fn).build();

    // `prompt` starts a run and returns immediately; the handle resolves to the NEW messages.
    let handle = agent.prompt("hi").await?;
    let new_messages = handle.finished().await;
    assert_eq!(new_messages.len(), 2); // the user prompt, then the assistant reply

    Ok(())
}
```

## One turn, end to end

A run is one tokio task that owns the cancellation root. Every event emission and hook invocation
happens on that single task, so event order is deterministic; only tool `execute` bodies run
concurrently, on a `JoinSet`. The state lock is taken for the synchronous reducer only and is never
held across a subscriber `await`.

A turn proceeds: any queued steering messages are injected, then the assistant response is streamed
(`message_start` … `message_end`). If the reply carries tool calls they are executed as a batch —
in parallel unless the run or any individual tool asks for `ExecMode::Sequential` — with each call
going through preflight (locate, normalize arguments, validate against the schema, `before_tool_call`)
and finalization (`after_tool_call`). `turn_end` then fires, followed by the two post-turn hooks:
`prepare_next_turn`, whose overrides are sticky for the rest of the run, and `should_stop_after_turn`.
Absent more tool calls or queued messages, the run closes with `agent_end`.

An assistant message truncated by the output token limit fails its whole tool batch rather than
executing calls whose arguments may be incomplete.

## Module map

| Path | Concern |
|---|---|
| `agent/` | `Agent`, `AgentBuilder`, the public facade and the run lifecycle |
| `agent/run/` | one run's working state: the turn driver and the LLM boundary |
| `agent/run/tools/` | batch dispatch, preflight, finalization, the parallel and sequential executors |
| `proxy/` | the `ProxyStreamFn` transport — wire enum, client-side partial rebuild, bearer SSE |
| `hooks.rs` | the mutating `Hooks` seam and its context views |
| `subscriber.rs` | the notify-only `EventSubscriber` |
| `queue.rs` | steering and follow-up queues, and the tool-execution mode |
| `state.rs` | managed state and the event reducer |
| `event.rs` / `error.rs` | the `AgentEvent` stream and the error surface |
| `stream_fn.rs` | the `StreamFn` seam plus `ProviderStreamFn` and `ApiKeyResolver` |
| `loop_fn.rs` | the low-level free-function loop, for callers that do not want `Agent` |

## Extension points

Four seams cover everything a downstream crate implements:

- **`StreamFn`** — the LLM boundary. Implement it to drive the loop from something other than a
  `cyrup_provider::Provider`; `ProviderStreamFn` and `ProxyStreamFn` are the two in-tree impls.
- **`Hooks`** — the mutating seam. Intercept and rewrite tool calls and results, override the model
  or context between turns, or stop a run early.
- **`EventSubscriber`** — notify-only observation of the ordered event stream. Registration returns
  a `Subscription`; dropping it does not detach, call `unsubscribe`.
- **`Tool`** (from `cyrup-core`) — a callable the model can invoke, including its execution mode and
  streaming partial updates.

## Conventions

Comments carrying anchors like `agent.ts:512`, `AGENT-018` or `R-02-045` record fidelity to the
upstream TypeScript implementation this crate is a port of: the file-and-line references point into
that upstream source, and the identifiers are conformance items. They are load-bearing — a change
that alters the behavior they describe should update them rather than leave them stale.

The workspace denies `unwrap_used`, `expect_used`, `panic` and `indexing_slicing` under
`[workspace.lints.clippy]`; test modules waive them locally.
