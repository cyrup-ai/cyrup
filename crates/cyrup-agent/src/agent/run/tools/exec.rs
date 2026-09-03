//! The two tool-execution runtimes: a `JoinSet` batch whose `execute` bodies run concurrently
//! while events stay in source order, and the sequential runtime that fully processes each call
//! before starting the next.

use super::{Batch, Finalized, Prep, PreparedCall, ToolRuntimeMsg};
use crate::agent::message::update_value;
use crate::agent::run::{RunCtx, RunFailure};
use crate::agent::util::panic_message;
use crate::event::{AgentEvent, AgentMessage};
use cyrup_core::{AssistantMessage, ToolCall, ToolError, ToolUpdate, ToolUpdateSink};
use futures::future::FutureExt;
use serde_json::Value;
use std::future::{poll_fn, Future};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::Poll;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinSet;

impl RunCtx {
    /// Parallel batch: `tool_execution_start` in source order, `tool_execution_end` in completion
    /// order, tool-result messages + `turn_end.toolResults` in source order (R-02-015/016/017).
    ///
    /// Preparation and execution are two distinct phases. Pi's `executeToolCallsParallel` pushes a
    /// LAZY closure per prepared call while it walks the batch (agent-loop.ts:522-533) and only
    /// invokes them in the `Promise.all` that follows the loop (agent-loop.ts:540-542), so NO tool
    /// body starts until EVERY call in the batch has been prepared. That matters because
    /// `before_tool_call` is where the permission dialog blocks on a human: starting call #1 while
    /// call #2's dialog is still open would let a tool run against state the user has not yet
    /// approved. Deferring the start is not serialization — once the whole batch is prepared the
    /// bodies are spawned together and run concurrently, exactly as `Promise.all` does.
    pub(super) async fn execute_parallel(
        &self,
        assistant: &AssistantMessage,
        ctx_messages: &[Arc<AgentMessage>],
        calls: &[ToolCall],
    ) -> Result<Batch, RunFailure> {
        let n = calls.len();
        let mut finalized: Vec<Option<Finalized>> = (0..n).map(|_| None).collect();
        // AGENT-003 — UNBOUNDED. pi collects every `tool_execution_update` emission into
        // `updateEvents` and awaits them all (`agent-loop.ts:671`, `:681-691`, `:695`/`:699`
        // @v0.83.0); the ONLY upstream drop rule is the `acceptingUpdates` flag (`:672`, `:680`,
        // `:694`, `:698`, `:705`), which `accepting` below mirrors. A bounded channel added a
        // second, silent drop rule: a tool emitting a synchronous burst of >64 updates lost the
        // overflow from the UI and the transcript.
        let (tx, mut rx) = mpsc::unbounded_channel::<ToolRuntimeMsg>();
        let mut joinset: JoinSet<()> = JoinSet::new();
        let mut deferred: Vec<PreparedCall> = Vec::new();

        for (idx, call) in calls.iter().enumerate() {
            self.emit(AgentEvent::ToolExecutionStart {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                args: Value::Object((*call.arguments).clone()),
            })
            .await?;
            match self.prepare(assistant, ctx_messages, call, idx).await {
                Prep::Immediate(fin) => {
                    self.emit(fin.end_event()).await?;
                    if let Some(slot) = finalized.get_mut(idx) {
                        *slot = Some(*fin);
                    }
                }
                // Prepared only — the body is NOT started here. Pi defers it to the
                // post-loop `Promise.all` so a later call's `before_tool_call` cannot
                // still be open while this one runs.
                Prep::Ready(prepared) => deferred.push(prepared),
            }
            if self.cancel.is_cancelled() {
                break;
            }
        }

        // Phase two — every call in the batch is prepared; start them all together
        // (Pi `await Promise.all(finalizedCalls.map(…))`, agent-loop.ts:540-542). Calls
        // deferred before an abort broke the loop are still started, exactly as Pi's
        // already-pushed closures are.
        let mut remaining = deferred.len();
        // The batch's start order. Each call releases the next as soon as its own body has been
        // driven to its first suspension point — which for `write`/`edit` is inside
        // `FileMutationLocks::guard`, so the mutation registrations line up in source order.
        let mut prev_started: Option<oneshot::Receiver<()>> = None;
        for PreparedCall { source_index, tool, args, call_id, tool_name } in deferred {
            let accepting = Arc::new(AtomicBool::new(true));
            let acc2 = accepting.clone();
            let utx = tx.clone();
            let ftx = tx.clone();
            let cid = call_id;
            let child = self.cancel.child();
            let (started_tx, started_rx) = oneshot::channel::<()>();
            let wait_turn = prev_started.replace(started_rx);
            joinset.spawn(async move {
                // Pi invokes every prepared call from `finalizedCalls.map((entry) => entry())`
                // (agent-loop.ts:540-542): `map` walks the array in source order and each async
                // body runs synchronously to its FIRST suspension point before the next closure is
                // invoked. `tokio::spawn` INVERTS that. `schedule_local` puts each newly spawned
                // task in the worker's LIFO slot and pushes the slot's previous occupant to the
                // back of the run queue (tokio 1.52.3
                // runtime/scheduler/multi_thread/worker.rs:1353-1377), and the worker polls the
                // LIFO slot first (:707) — so an unordered batch starts its LAST call first. An
                // `Err` here means the previous call was aborted before it ran; proceed rather
                // than stall.
                if let Some(turn) = wait_turn {
                    let _ = turn.await;
                }

                let mut body = std::pin::pin!(async move {
                    let sink_cid = cid.clone();
                    let on_update: ToolUpdateSink = Box::new(move |u: ToolUpdate| {
                        if acc2.load(Ordering::Acquire) {
                            // AGENT-003 — never drops: the send only fails once the receiver is
                            // gone.
                            let _ = utx.send(ToolRuntimeMsg::Update {
                                call_id: sink_cid.clone(),
                                partial: u,
                            });
                        }
                    });
                    // AGENT-016 — pi wraps EVERY execute in try/catch/finally and converts a throw
                    // into `{ result: createErrorToolResult(...), isError: true }`
                    // (`packages/agent/src/agent-loop.ts:700-703` @v0.83.0, inside
                    // `executePreparedToolCall` at `:666-707`), identically in the parallel and the
                    // sequential batch. Without `catch_unwind` here the spawned task dies before the
                    // `ftx.send` below, `remaining` never reaches zero, the slot stays `None`, and
                    // the batch emits NO tool-result message for this call — so the next request
                    // carries an assistant `tool_use` with no matching `tool_result`.
                    // `AssertUnwindSafe` is sound for the same reason as in `emit`: the tool owns no
                    // managed-state lock across this await (keeps the crate
                    // `#![forbid(unsafe_code)]`).
                    let outcome = match std::panic::AssertUnwindSafe(tool.execute(
                        cid.clone(),
                        args,
                        child,
                        on_update,
                    ))
                    .catch_unwind()
                    .await
                    {
                        Ok(r) => r,
                        Err(payload) => Err(ToolError::new(panic_message(payload.as_ref()))),
                    };
                    accepting.store(false, Ordering::Release);
                    let _ = ftx.send(ToolRuntimeMsg::Finished {
                        call_id: cid,
                        source_index,
                        tool_name,
                        outcome,
                    });
                });

                // Drive this call to its first suspension point — pi's `entry()` — then hand the
                // batch on.
                let first = poll_fn(|cx| Poll::Ready(body.as_mut().poll(cx))).await;
                let _ = started_tx.send(());
                if first.is_pending() {
                    body.await;
                }
            });
        }
        drop(tx);

        while remaining > 0 {
            match rx.recv().await {
                None => break,
                Some(ToolRuntimeMsg::Update { call_id, partial }) => {
                    let (tn, ar) = calls
                        .iter()
                        .find(|c| c.id == call_id)
                        .map(|c| (c.name.clone(), Value::Object((*c.arguments).clone())))
                        .unwrap_or_default();
                    self.emit(AgentEvent::ToolExecutionUpdate {
                        tool_call_id: call_id,
                        tool_name: tn,
                        args: ar,
                        partial_result: update_value(&partial),
                    })
                    .await?;
                }
                Some(ToolRuntimeMsg::Finished { call_id, source_index, tool_name, outcome }) => {
                    let (args, call) = calls
                        .iter()
                        .find(|c| c.id == call_id)
                        .map(|c| (Value::Object((*c.arguments).clone()), c.clone()))
                        .unwrap_or_else(|| {
                            // Defensive: the id always matches a source call; synthesize a stand-in.
                            (Value::Null, ToolCall {
                                id: call_id.clone(),
                                name: tool_name.clone(),
                                arguments: serde_json::Map::new().into(),
                                thought_signature: None,
                            })
                        });
                    let fin =
                        self.finalize(assistant, ctx_messages, &call, source_index, args, outcome).await;
                    self.emit(fin.end_event()).await?;
                    if let Some(slot) = finalized.get_mut(fin.source_index()) {
                        *slot = Some(fin);
                    }
                    remaining -= 1;
                }
            }
        }
        while joinset.join_next().await.is_some() {}

        // AGENT-015 — fold over the slots that were actually FILLED. pi's `finalizedCalls` array
        // (`agent-loop.ts:497` @v0.83.0) holds only entries it pushed, and `shouldTerminateToolBatch`
        // (`:582-584`) is `finalizedCalls.length > 0 && finalizedCalls.every(f => f.result.terminate
        // === true)` over that shortened list. cyrup pre-sizes `finalized` to `calls.len()`, so
        // seeding `all_terminate` from `!finalized.is_empty()` and letting every never-prepared slot
        // veto it made an abort mid-batch run another turn where pi terminates — and made the
        // parallel and sequential modes disagree with each other (the sequential path already folds
        // over its `produced` counter).
        let present: Vec<Finalized> = finalized.into_iter().flatten().collect();
        let all_terminate =
            !present.is_empty() && present.iter().all(|f| f.terminate().requested());
        let mut tool_results = Vec::new();
        for fin in present {
            let message = fin.into_message();
            let msg = AgentMessage::ToolResult(message.clone());
            self.emit(AgentEvent::MessageStart { message: msg.clone() }).await?;
            self.emit(AgentEvent::MessageEnd { message: msg }).await?;
            tool_results.push(message);
        }
        Ok(Batch { messages: tool_results, terminate: all_terminate })
    }

    /// Sequential batch: each call fully processed before the next; abort breaks the loop (R-02-018).
    pub(super) async fn execute_sequential(
        &self,
        assistant: &AssistantMessage,
        ctx_messages: &[Arc<AgentMessage>],
        calls: &[ToolCall],
    ) -> Result<Batch, RunFailure> {
        let mut tool_results = Vec::new();
        let mut all_terminate = !calls.is_empty();
        let mut produced = 0usize;

        for (idx, call) in calls.iter().enumerate() {
            self.emit(AgentEvent::ToolExecutionStart {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                args: Value::Object((*call.arguments).clone()),
            })
            .await?;

            let fin = match self.prepare(assistant, ctx_messages, call, idx).await {
                Prep::Immediate(fin) => *fin,
                Prep::Ready(PreparedCall { source_index, tool, args, .. }) => {
                    // AGENT-003 — UNBOUNDED, same reasoning as the parallel path: pi's only drop
                    // rule is `acceptingUpdates` (`agent-loop.ts:672`/`:680` @v0.83.0).
                    let (utx, mut urx) = mpsc::unbounded_channel::<ToolUpdate>();
                    let accepting = Arc::new(AtomicBool::new(true));
                    let acc2 = accepting.clone();
                    let on_update: ToolUpdateSink = Box::new(move |u| {
                        if acc2.load(Ordering::Acquire) {
                            let _ = utx.send(u);
                        }
                    });
                    let child = self.cancel.child();
                    // AGENT-016 — the same `catch_unwind` the parallel batch takes, so the two
                    // modes match pi's SINGLE try/catch in `executePreparedToolCall`
                    // (`agent-loop.ts:666-707` @v0.83.0, the throw→error-result conversion at
                    // `:700-703`). Sequential already unwound to the run task's own `catch_unwind`
                    // and closed cleanly, but "closed cleanly" is not pi's behaviour either: pi
                    // finishes the batch with an error tool-result and keeps going.
                    let exec = std::panic::AssertUnwindSafe(tool.execute(
                        call.id.clone(),
                        args.clone(),
                        child,
                        on_update,
                    ))
                    .catch_unwind();
                    tokio::pin!(exec);
                    let outcome = loop {
                        tokio::select! {
                            biased;
                            u = urx.recv() => {
                                if let Some(u) = u {
                                    self.emit(AgentEvent::ToolExecutionUpdate {
                                        tool_call_id: call.id.clone(),
                                        tool_name: call.name.clone(),
                                        args: Value::Object((*call.arguments).clone()),
                                        partial_result: update_value(&u),
                                    })
                                    .await?;
                                }
                            }
                            r = &mut exec => break match r {
                                Ok(o) => o,
                                Err(payload) => {
                                    Err(ToolError::new(panic_message(payload.as_ref())))
                                }
                            },
                        }
                    };
                    accepting.store(false, Ordering::Release);
                    // AGENT-003 — pi awaits `Promise.all(updateEvents)` AFTER the execute settles,
                    // on BOTH the success and the throw path (`agent-loop.ts:694-695` / `:698-699`
                    // @v0.83.0), so an update emitted immediately before the tool returned is still
                    // delivered. The `select!` above breaks the instant `exec` completes, which for
                    // a tool that emits synchronously and returns without ever awaiting means the
                    // whole burst is still sitting in the channel — drain it here rather than
                    // dropping it on the floor. `accepting` is already false, so nothing new can
                    // arrive; this terminates.
                    while let Ok(u) = urx.try_recv() {
                        self.emit(AgentEvent::ToolExecutionUpdate {
                            tool_call_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            args: Value::Object((*call.arguments).clone()),
                            partial_result: update_value(&u),
                        })
                        .await?;
                    }
                    self.finalize(assistant, ctx_messages, call, source_index, args, outcome).await
                }
            };

            self.emit(fin.end_event()).await?;
            if !fin.terminate().requested() {
                all_terminate = false;
            }
            let message = fin.into_message();
            let msg = AgentMessage::ToolResult(message.clone());
            self.emit(AgentEvent::MessageStart { message: msg.clone() }).await?;
            self.emit(AgentEvent::MessageEnd { message: msg }).await?;
            tool_results.push(message);
            produced += 1;

            if self.cancel.is_cancelled() {
                break;
            }
        }
        if produced == 0 {
            all_terminate = false;
        }
        Ok(Batch { messages: tool_results, terminate: all_terminate })
    }
}
